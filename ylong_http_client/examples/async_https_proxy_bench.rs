// Copyright (c) 2026 Huawei Device Co., Ltd.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

/// HTTPS proxy benchmark client for comparison with libcurl.
use std::env;
use std::future::{poll_fn, Future};
use std::sync::Arc;
#[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
use tokio::runtime::Builder;
#[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
use tokio::sync::Barrier;
#[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
use tokio::task::JoinHandle;
#[cfg(all(feature = "ylong_base", not(feature = "tokio_base")))]
use ylong_runtime::builder::RuntimeBuilder;
#[cfg(all(feature = "ylong_base", not(feature = "tokio_base")))]
use ylong_runtime::sync::{mpsc, watch};
#[cfg(all(feature = "ylong_base", not(feature = "tokio_base")))]
use ylong_runtime::task::JoinHandle;

use ylong_http_client::async_impl::{Body, ClientBuilder, Request};
use ylong_http_client::{HttpClientError, Proxy, TlsConfig, TlsFileType};

#[cfg(feature = "__c_openssl")]
use openssl as _;

#[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
const CLIENT_NAME: &str = "ylong_http_client_async";
#[cfg(all(feature = "ylong_base", not(feature = "tokio_base")))]
const CLIENT_NAME: &str = "ylong_http_client_async_ylong";

#[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
const RUNTIME_BACKEND: &str = "tokio";
#[cfg(all(feature = "ylong_base", not(feature = "tokio_base")))]
const RUNTIME_BACKEND: &str = "ylong";

struct Config {
    url: String,
    proxy: String,
    requests: usize,
    warmup_requests: usize,
    concurrency: usize,
    runtime_threads: usize,
    runtime_mode: RuntimeMode,
    runtime_affinity: bool,
    read_buffer_size: usize,
    ylong_read_chunk_size: usize,
    client_per_worker: bool,
    prebuilt_requests: bool,
    trace_summary: bool,
    phase_summary: bool,
    yield_after_body_read: bool,
    method: String,
    body_size: usize,
    proxy_ca_file: Option<String>,
    proxy_client_cert: Option<String>,
    proxy_client_key: Option<String>,
    origin_ca_file: Option<String>,
    insecure_proxy: bool,
    insecure_origin: bool,
    proxy_user_pass: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuntimeMode {
    MultiThread,
    CurrentThreadPerWorker,
    CurrentThreadSharded,
}

impl RuntimeMode {
    fn as_str(self) -> &'static str {
        match self {
            RuntimeMode::MultiThread => "multi-thread",
            RuntimeMode::CurrentThreadPerWorker => "current-thread-per-worker",
            RuntimeMode::CurrentThreadSharded => "current-thread-sharded",
        }
    }
}

struct WorkerResult {
    latencies_us: Vec<u128>,
    bytes: u64,
    body_reads: u64,
    errors: usize,
    start_delay_us: u128,
    elapsed_us: u128,
    trace: TraceSamples,
    phase: PhaseSamples,
}

#[derive(Default)]
struct PhaseSamples {
    connect_us: Vec<u128>,
    request_write_us: Vec<u128>,
    response_wait_us: Vec<u128>,
    transfer_us: Vec<u128>,
    body_first_byte_us: Vec<u128>,
    body_drain_us: Vec<u128>,
    body_reads_per_request: Vec<u128>,
    bytes_per_request: Vec<u128>,
}

impl PhaseSamples {
    fn merge(&mut self, mut other: PhaseSamples) {
        self.connect_us.append(&mut other.connect_us);
        self.request_write_us.append(&mut other.request_write_us);
        self.response_wait_us.append(&mut other.response_wait_us);
        self.transfer_us.append(&mut other.transfer_us);
        self.body_first_byte_us
            .append(&mut other.body_first_byte_us);
        self.body_drain_us.append(&mut other.body_drain_us);
        self.body_reads_per_request
            .append(&mut other.body_reads_per_request);
        self.bytes_per_request.append(&mut other.bytes_per_request);
    }

    fn push(&mut self, phase: ResponsePhase) {
        if let Some(connect_us) = phase.connect_us {
            self.connect_us.push(connect_us);
        }
        if let Some(request_write_us) = phase.request_write_us {
            self.request_write_us.push(request_write_us);
        }
        if let Some(response_wait_us) = phase.response_wait_us {
            self.response_wait_us.push(response_wait_us);
        }
        if let Some(transfer_us) = phase.transfer_us {
            self.transfer_us.push(transfer_us);
        }
        if let Some(body_first_byte_us) = phase.body_first_byte_us {
            self.body_first_byte_us.push(body_first_byte_us);
        }
        self.body_drain_us.push(phase.body_drain_us);
        self.body_reads_per_request.push(phase.body_reads as u128);
        self.bytes_per_request.push(phase.bytes as u128);
    }

    fn sort(&mut self) {
        self.connect_us.sort_unstable();
        self.request_write_us.sort_unstable();
        self.response_wait_us.sort_unstable();
        self.transfer_us.sort_unstable();
        self.body_first_byte_us.sort_unstable();
        self.body_drain_us.sort_unstable();
        self.body_reads_per_request.sort_unstable();
        self.bytes_per_request.sort_unstable();
    }
}

#[derive(Default)]
struct TraceSamples {
    request_ready_us: Vec<u128>,
    request_poll_count: Vec<u128>,
    request_pending_count: Vec<u128>,
    request_pending_gap_us: Vec<u128>,
    request_pending_gap_same_thread_us: Vec<u128>,
    request_pending_gap_migrated_us: Vec<u128>,
    connect_us: Vec<u128>,
    request_write_us: Vec<u128>,
    response_wait_us: Vec<u128>,
    transfer_us: Vec<u128>,
    body_first_byte_us: Vec<u128>,
    body_drain_us: Vec<u128>,
    body_poll_count: Vec<u128>,
    body_pending_count: Vec<u128>,
    body_pending_gap_us: Vec<u128>,
    body_pending_gap_same_thread_us: Vec<u128>,
    body_pending_gap_migrated_us: Vec<u128>,
    body_read_wait_us: Vec<u128>,
    body_eof_wait_us: Vec<u128>,
    body_reads_per_request: Vec<u128>,
    bytes_per_request: Vec<u128>,
    request_max_pending_gap: Option<PendingGapSample>,
    body_max_pending_gap: Option<PendingGapSample>,
}

impl TraceSamples {
    fn merge(&mut self, mut other: TraceSamples) {
        self.request_ready_us.append(&mut other.request_ready_us);
        self.request_poll_count
            .append(&mut other.request_poll_count);
        self.request_pending_count
            .append(&mut other.request_pending_count);
        self.request_pending_gap_us
            .append(&mut other.request_pending_gap_us);
        self.request_pending_gap_same_thread_us
            .append(&mut other.request_pending_gap_same_thread_us);
        self.request_pending_gap_migrated_us
            .append(&mut other.request_pending_gap_migrated_us);
        self.update_request_max_pending_gap(other.request_max_pending_gap.take());
        self.connect_us.append(&mut other.connect_us);
        self.request_write_us.append(&mut other.request_write_us);
        self.response_wait_us.append(&mut other.response_wait_us);
        self.transfer_us.append(&mut other.transfer_us);
        self.body_first_byte_us
            .append(&mut other.body_first_byte_us);
        self.body_drain_us.append(&mut other.body_drain_us);
        self.body_poll_count.append(&mut other.body_poll_count);
        self.body_pending_count
            .append(&mut other.body_pending_count);
        self.body_pending_gap_us
            .append(&mut other.body_pending_gap_us);
        self.body_pending_gap_same_thread_us
            .append(&mut other.body_pending_gap_same_thread_us);
        self.body_pending_gap_migrated_us
            .append(&mut other.body_pending_gap_migrated_us);
        self.update_body_max_pending_gap(other.body_max_pending_gap.take());
        self.body_read_wait_us.append(&mut other.body_read_wait_us);
        self.body_eof_wait_us.append(&mut other.body_eof_wait_us);
        self.body_reads_per_request
            .append(&mut other.body_reads_per_request);
        self.bytes_per_request.append(&mut other.bytes_per_request);
    }

    fn push(&mut self, mut trace: ResponseTrace) {
        self.request_ready_us.push(trace.request_ready_us);
        self.request_poll_count
            .push(trace.request_poll_trace.polls as u128);
        self.request_pending_count
            .push(trace.request_poll_trace.pending as u128);
        self.request_pending_gap_us
            .extend(trace.request_poll_trace.pending_gap_us);
        self.request_pending_gap_same_thread_us
            .extend(trace.request_poll_trace.pending_gap_same_thread_us);
        self.request_pending_gap_migrated_us
            .extend(trace.request_poll_trace.pending_gap_migrated_us);
        self.update_request_max_pending_gap(trace.request_poll_trace.max_pending_gap.take());
        if let Some(connect_us) = trace.connect_us {
            self.connect_us.push(connect_us);
        }
        if let Some(request_write_us) = trace.request_write_us {
            self.request_write_us.push(request_write_us);
        }
        if let Some(response_wait_us) = trace.response_wait_us {
            self.response_wait_us.push(response_wait_us);
        }
        if let Some(transfer_us) = trace.transfer_us {
            self.transfer_us.push(transfer_us);
        }
        if let Some(body_first_byte_us) = trace.body_first_byte_us {
            self.body_first_byte_us.push(body_first_byte_us);
        }
        self.body_drain_us.push(trace.body_drain_us);
        self.body_poll_count
            .push(trace.body_poll_trace.polls as u128);
        self.body_pending_count
            .push(trace.body_poll_trace.pending as u128);
        self.body_pending_gap_us
            .extend(trace.body_poll_trace.pending_gap_us);
        self.body_pending_gap_same_thread_us
            .extend(trace.body_poll_trace.pending_gap_same_thread_us);
        self.body_pending_gap_migrated_us
            .extend(trace.body_poll_trace.pending_gap_migrated_us);
        self.update_body_max_pending_gap(trace.body_poll_trace.max_pending_gap.take());
        self.body_read_wait_us.extend(trace.body_read_wait_us);
        if let Some(body_eof_wait_us) = trace.body_eof_wait_us {
            self.body_eof_wait_us.push(body_eof_wait_us);
        }
        self.body_reads_per_request.push(trace.body_reads as u128);
        self.bytes_per_request.push(trace.bytes as u128);
    }

    fn sort(&mut self) {
        self.request_ready_us.sort_unstable();
        self.request_poll_count.sort_unstable();
        self.request_pending_count.sort_unstable();
        self.request_pending_gap_us.sort_unstable();
        self.request_pending_gap_same_thread_us.sort_unstable();
        self.request_pending_gap_migrated_us.sort_unstable();
        self.connect_us.sort_unstable();
        self.request_write_us.sort_unstable();
        self.response_wait_us.sort_unstable();
        self.transfer_us.sort_unstable();
        self.body_first_byte_us.sort_unstable();
        self.body_drain_us.sort_unstable();
        self.body_poll_count.sort_unstable();
        self.body_pending_count.sort_unstable();
        self.body_pending_gap_us.sort_unstable();
        self.body_pending_gap_same_thread_us.sort_unstable();
        self.body_pending_gap_migrated_us.sort_unstable();
        self.body_read_wait_us.sort_unstable();
        self.body_eof_wait_us.sort_unstable();
        self.body_reads_per_request.sort_unstable();
        self.bytes_per_request.sort_unstable();
    }

    fn update_request_max_pending_gap(&mut self, sample: Option<PendingGapSample>) {
        update_max_pending_gap(&mut self.request_max_pending_gap, sample);
    }

    fn update_body_max_pending_gap(&mut self, sample: Option<PendingGapSample>) {
        update_max_pending_gap(&mut self.body_max_pending_gap, sample);
    }
}

struct ResponseTrace {
    request_ready_us: u128,
    request_poll_trace: FuturePollTrace,
    connect_us: Option<u128>,
    request_write_us: Option<u128>,
    response_wait_us: Option<u128>,
    transfer_us: Option<u128>,
    body_first_byte_us: Option<u128>,
    body_drain_us: u128,
    body_poll_trace: FuturePollTrace,
    body_read_wait_us: Vec<u128>,
    body_eof_wait_us: Option<u128>,
    bytes: u64,
    body_reads: u64,
}

struct ResponsePhase {
    connect_us: Option<u128>,
    request_write_us: Option<u128>,
    response_wait_us: Option<u128>,
    transfer_us: Option<u128>,
    body_first_byte_us: Option<u128>,
    body_drain_us: u128,
    bytes: u64,
    body_reads: u64,
}

#[derive(Default)]
struct FuturePollTrace {
    polls: u64,
    pending: u64,
    pending_gap_us: Vec<u128>,
    pending_gap_same_thread_us: Vec<u128>,
    pending_gap_migrated_us: Vec<u128>,
    max_pending_gap: Option<PendingGapSample>,
}

impl FuturePollTrace {
    fn merge(&mut self, mut other: FuturePollTrace) {
        self.polls += other.polls;
        self.pending += other.pending;
        self.pending_gap_us.append(&mut other.pending_gap_us);
        self.pending_gap_same_thread_us
            .append(&mut other.pending_gap_same_thread_us);
        self.pending_gap_migrated_us
            .append(&mut other.pending_gap_migrated_us);
        update_max_pending_gap(&mut self.max_pending_gap, other.max_pending_gap.take());
    }
}

#[derive(Clone, Copy)]
struct TraceContext {
    worker_id: usize,
    request_index: usize,
    stage: &'static str,
}

impl TraceContext {
    fn stage(self, stage: &'static str) -> Self {
        Self { stage, ..self }
    }
}

struct PendingGapSample {
    gap_us: u128,
    worker_id: usize,
    request_index: usize,
    stage: &'static str,
    pending_poll: u64,
    resume_poll: u64,
    pending_thread: String,
    resume_thread: String,
}

fn update_max_pending_gap(
    current: &mut Option<PendingGapSample>,
    sample: Option<PendingGapSample>,
) {
    let Some(sample) = sample else {
        return;
    };
    if current
        .as_ref()
        .map(|current| sample.gap_us > current.gap_us)
        .unwrap_or(true)
    {
        *current = Some(sample);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = match parse_args(env::args().skip(1).collect()) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("{}", usage());
            std::process::exit(2);
        }
    };

    #[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
    {
        let runtime = Builder::new_multi_thread()
            .worker_threads(config.runtime_threads)
            .enable_all()
            .build()?;
        runtime.block_on(run(config))
    }

    #[cfg(all(feature = "ylong_base", not(feature = "tokio_base")))]
    {
        match config.runtime_mode {
            RuntimeMode::MultiThread => {
                RuntimeBuilder::new_multi_thread()
                    .worker_num(config.runtime_threads)
                    .is_affinity(config.runtime_affinity)
                    .build_global()?;
                ylong_runtime::block_on(run(config))
            }
            RuntimeMode::CurrentThreadPerWorker => {
                #[cfg(feature = "__ylong_current_thread_runtime")]
                {
                    run_current_thread_per_worker(config)
                }
                #[cfg(not(feature = "__ylong_current_thread_runtime"))]
                {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "--runtime-mode current-thread-per-worker requires __ylong_current_thread_runtime",
                    )
                    .into())
                }
            }
            RuntimeMode::CurrentThreadSharded => {
                #[cfg(feature = "__ylong_current_thread_runtime")]
                {
                    run_current_thread_sharded(config)
                }
                #[cfg(not(feature = "__ylong_current_thread_runtime"))]
                {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "--runtime-mode current-thread-sharded requires __ylong_current_thread_runtime",
                    )
                    .into())
                }
            }
        }
    }
}

async fn run(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let config = Arc::new(config);
    let shared_client = if config.client_per_worker {
        None
    } else {
        Some(Arc::new(build_client(&config)?))
    };
    let mut handles = Vec::with_capacity(config.concurrency);
    let mut start = StartCoordinator::new(config.concurrency);
    for worker in 0..config.concurrency {
        let measured_count = requests_for_worker(config.requests, config.concurrency, worker);
        let warmup_count = requests_for_worker(config.warmup_requests, config.concurrency, worker);
        let client = shared_client.clone();
        let config = config.clone();
        let gate = start.worker_gate();
        handles.push(spawn_worker(async move {
            run_worker(client, config, worker, warmup_count, measured_count, gate).await
        }));
    }

    let started = start.wait_all().await;

    let mut latencies = Vec::with_capacity(config.requests);
    let mut bytes = 0;
    let mut body_reads = 0;
    let mut errors = 0;
    let mut worker_start_delay_us = Vec::with_capacity(config.concurrency);
    let mut worker_elapsed_us = Vec::with_capacity(config.concurrency);
    let mut trace = TraceSamples::default();
    let mut phase = PhaseSamples::default();
    for handle in handles {
        let result = join_worker(handle).await;
        latencies.extend(result.latencies_us);
        bytes += result.bytes;
        body_reads += result.body_reads;
        errors += result.errors;
        worker_start_delay_us.push(result.start_delay_us);
        if result.elapsed_us != 0 {
            worker_elapsed_us.push(result.elapsed_us);
        }
        trace.merge(result.trace);
        phase.merge(result.phase);
    }
    let elapsed = started.elapsed();

    print_results(
        &config,
        latencies,
        bytes,
        body_reads,
        errors,
        worker_start_delay_us,
        worker_elapsed_us,
        trace,
        phase,
        elapsed,
    );

    Ok(())
}

#[cfg(all(
    feature = "ylong_base",
    feature = "__ylong_current_thread_runtime",
    not(feature = "tokio_base")
))]
fn run_current_thread_per_worker(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let config = Arc::new(config);
    let ready = Arc::new(std::sync::Barrier::new(config.concurrency + 1));
    let start = Arc::new(std::sync::Barrier::new(config.concurrency + 1));
    let started = Arc::new(std::sync::Mutex::new(None));
    let mut handles = Vec::with_capacity(config.concurrency);

    for worker in 0..config.concurrency {
        let measured_count = requests_for_worker(config.requests, config.concurrency, worker);
        let warmup_count = requests_for_worker(config.warmup_requests, config.concurrency, worker);
        let config = config.clone();
        let gate = StartGate::Thread {
            ready: ready.clone(),
            start: start.clone(),
            started: started.clone(),
        };
        handles.push(thread::spawn(move || {
            let mut builder = RuntimeBuilder::new_current_thread();
            match builder.build() {
                Ok(runtime) => runtime.block_on(run_worker(
                    None,
                    config,
                    worker,
                    warmup_count,
                    measured_count,
                    gate,
                )),
                Err(_) => {
                    let measured_started = thread_gate_ready_and_wait(gate);
                    WorkerResult {
                        latencies_us: Vec::new(),
                        bytes: 0,
                        body_reads: 0,
                        errors: warmup_count + measured_count,
                        start_delay_us: measured_started.elapsed().as_micros(),
                        elapsed_us: 0,
                        trace: TraceSamples::default(),
                        phase: PhaseSamples::default(),
                    }
                }
            }
        }));
    }

    ready.wait();
    let started_instant = Instant::now();
    *started.lock().unwrap() = Some(started_instant);
    start.wait();

    let mut latencies = Vec::with_capacity(config.requests);
    let mut bytes = 0;
    let mut body_reads = 0;
    let mut errors = 0;
    let mut worker_start_delay_us = Vec::with_capacity(config.concurrency);
    let mut worker_elapsed_us = Vec::with_capacity(config.concurrency);
    let mut trace = TraceSamples::default();
    let mut phase = PhaseSamples::default();
    for handle in handles {
        match handle.join() {
            Ok(result) => {
                latencies.extend(result.latencies_us);
                bytes += result.bytes;
                body_reads += result.body_reads;
                errors += result.errors;
                worker_start_delay_us.push(result.start_delay_us);
                if result.elapsed_us != 0 {
                    worker_elapsed_us.push(result.elapsed_us);
                }
                trace.merge(result.trace);
                phase.merge(result.phase);
            }
            Err(_) => errors += 1,
        }
    }

    let elapsed = started_instant.elapsed();
    print_results(
        &config,
        latencies,
        bytes,
        body_reads,
        errors,
        worker_start_delay_us,
        worker_elapsed_us,
        trace,
        phase,
        elapsed,
    );

    Ok(())
}

#[cfg(all(
    feature = "ylong_base",
    feature = "__ylong_current_thread_runtime",
    not(feature = "tokio_base")
))]
struct WorkerAssignment {
    worker_id: usize,
    warmup_count: usize,
    measured_count: usize,
    gate: StartGate,
}

#[cfg(all(
    feature = "ylong_base",
    feature = "__ylong_current_thread_runtime",
    not(feature = "tokio_base")
))]
fn run_current_thread_sharded(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let config = Arc::new(config);
    let shard_count = config.runtime_threads.min(config.concurrency).max(1);
    let mut start = StartCoordinator::new(config.concurrency);
    let mut shards: Vec<Vec<WorkerAssignment>> = (0..shard_count).map(|_| Vec::new()).collect();
    for worker in 0..config.concurrency {
        let measured_count = requests_for_worker(config.requests, config.concurrency, worker);
        let warmup_count = requests_for_worker(config.warmup_requests, config.concurrency, worker);
        shards[worker % shard_count].push(WorkerAssignment {
            worker_id: worker,
            warmup_count,
            measured_count,
            gate: start.worker_gate(),
        });
    }

    let mut handles = Vec::with_capacity(shard_count);
    for assignments in shards {
        let config = config.clone();
        handles.push(thread::spawn(move || {
            run_current_thread_shard(config, assignments)
        }));
    }

    let mut builder = RuntimeBuilder::new_current_thread();
    let coordinator = builder.build()?;
    let started = coordinator.block_on(start.wait_all());

    let mut latencies = Vec::with_capacity(config.requests);
    let mut bytes = 0;
    let mut body_reads = 0;
    let mut errors = 0;
    let mut worker_start_delay_us = Vec::with_capacity(config.concurrency);
    let mut worker_elapsed_us = Vec::with_capacity(config.concurrency);
    let mut trace = TraceSamples::default();
    let mut phase = PhaseSamples::default();
    for handle in handles {
        match handle.join() {
            Ok(results) => {
                for result in results {
                    latencies.extend(result.latencies_us);
                    bytes += result.bytes;
                    body_reads += result.body_reads;
                    errors += result.errors;
                    worker_start_delay_us.push(result.start_delay_us);
                    if result.elapsed_us != 0 {
                        worker_elapsed_us.push(result.elapsed_us);
                    }
                    trace.merge(result.trace);
                    phase.merge(result.phase);
                }
            }
            Err(_) => errors += 1,
        }
    }

    let elapsed = started.elapsed();
    print_results(
        &config,
        latencies,
        bytes,
        body_reads,
        errors,
        worker_start_delay_us,
        worker_elapsed_us,
        trace,
        phase,
        elapsed,
    );

    Ok(())
}

#[cfg(all(
    feature = "ylong_base",
    feature = "__ylong_current_thread_runtime",
    not(feature = "tokio_base")
))]
fn run_current_thread_shard(
    config: Arc<Config>,
    assignments: Vec<WorkerAssignment>,
) -> Vec<WorkerResult> {
    let mut builder = RuntimeBuilder::new_current_thread();
    let runtime = match builder.build() {
        Ok(runtime) => runtime,
        Err(_) => {
            return assignments
                .into_iter()
                .map(worker_assignment_error_result)
                .collect()
        }
    };

    let shared_client = build_client(&config).ok().map(Arc::new);
    let mut handles = Vec::with_capacity(assignments.len());
    for assignment in assignments {
        let client = shared_client.clone();
        let config = config.clone();
        handles.push(runtime.spawn(run_worker(
            client,
            config,
            assignment.worker_id,
            assignment.warmup_count,
            assignment.measured_count,
            assignment.gate,
        )));
    }

    runtime.block_on(async move {
        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            results.push(join_worker(handle).await);
        }
        results
    })
}

#[cfg(all(
    feature = "ylong_base",
    feature = "__ylong_current_thread_runtime",
    not(feature = "tokio_base")
))]
fn worker_assignment_error_result(assignment: WorkerAssignment) -> WorkerResult {
    signal_gate_ready(assignment.gate);
    WorkerResult {
        latencies_us: Vec::new(),
        bytes: 0,
        body_reads: 0,
        errors: assignment.warmup_count + assignment.measured_count,
        start_delay_us: 0,
        elapsed_us: 0,
        trace: TraceSamples::default(),
        phase: PhaseSamples::default(),
    }
}

fn print_results(
    config: &Config,
    mut latencies: Vec<u128>,
    bytes: u64,
    body_reads: u64,
    errors: usize,
    worker_start_delay_us: Vec<u128>,
    worker_elapsed_us: Vec<u128>,
    mut trace: TraceSamples,
    mut phase: PhaseSamples,
    elapsed: Duration,
) {
    latencies.sort_unstable();

    let completed = latencies.len();
    let elapsed_ms = elapsed.as_secs_f64() * 1000.0;
    let rps = if elapsed.as_secs_f64() == 0.0 {
        0.0
    } else {
        completed as f64 / elapsed.as_secs_f64()
    };

    println!(
        "{{\"client\":\"{}\",\"runtime_backend\":\"{}\",\"runtime_mode\":\"{}\",\"url\":\"{}\",\"proxy\":\"{}\",\"method\":\"{}\",\"body_size\":{},\"requests\":{},\"warmup_requests\":{},\"completed\":{},\"errors\":{},\"concurrency\":{},\"runtime_threads\":{},\"runtime_affinity\":{},\"read_buffer_size\":{},\"ylong_read_chunk_size\":{},\"client_per_worker\":{},\"prebuilt_requests\":{},\"trace_summary\":{},\"phase_summary\":{},\"yield_after_body_read\":{},\"bytes\":{},\"body_reads\":{},\"avg_body_read_size\":{:.3},\"elapsed_ms\":{:.3},\"rps\":{:.3},\"latency_us_p50\":{},\"latency_us_p90\":{},\"latency_us_p95\":{},\"latency_us_p99\":{},\"worker_start_delay_us_min\":{},\"worker_start_delay_us_max\":{},\"worker_elapsed_us_min\":{},\"worker_elapsed_us_max\":{}}}",
        CLIENT_NAME,
        RUNTIME_BACKEND,
        config.runtime_mode.as_str(),
        escape_json(&config.url),
        escape_json(&config.proxy),
        escape_json(&config.method),
        config.body_size,
        config.requests,
        config.warmup_requests,
        completed,
        errors,
        config.concurrency,
        config.runtime_threads,
        config.runtime_affinity,
        config.read_buffer_size,
        config.ylong_read_chunk_size,
        config.client_per_worker,
        config.prebuilt_requests,
        config.trace_summary,
        config.phase_summary,
        config.yield_after_body_read,
        bytes,
        body_reads,
        average_read_size(bytes, body_reads),
        elapsed_ms,
        rps,
        percentile(&latencies, 50),
        percentile(&latencies, 90),
        percentile(&latencies, 95),
        percentile(&latencies, 99),
        min_u128(&worker_start_delay_us),
        max_u128(&worker_start_delay_us),
        min_u128(&worker_elapsed_us),
        max_u128(&worker_elapsed_us),
    );

    if config.trace_summary {
        print_trace_summary(
            &config,
            completed,
            errors,
            &mut trace,
            &worker_start_delay_us,
            &worker_elapsed_us,
        );
    }

    if config.phase_summary {
        print_phase_summary(&config, completed, errors, &mut phase);
    }
}

#[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
#[derive(Clone)]
struct StartGate {
    ready: Arc<Barrier>,
    start: Arc<Barrier>,
    started: Arc<Mutex<Option<Instant>>>,
}

#[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
struct StartCoordinator {
    ready: Arc<Barrier>,
    start: Arc<Barrier>,
    started: Arc<Mutex<Option<Instant>>>,
}

#[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
impl StartCoordinator {
    fn new(workers: usize) -> Self {
        Self {
            ready: Arc::new(Barrier::new(workers + 1)),
            start: Arc::new(Barrier::new(workers + 1)),
            started: Arc::new(Mutex::new(None)),
        }
    }

    fn worker_gate(&self) -> StartGate {
        StartGate {
            ready: self.ready.clone(),
            start: self.start.clone(),
            started: self.started.clone(),
        }
    }

    async fn wait_all(&mut self) -> Instant {
        self.ready.wait().await;
        let started = Instant::now();
        *self.started.lock().unwrap() = Some(started);
        self.start.wait().await;
        started
    }
}

#[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
async fn worker_ready_and_wait(gate: StartGate) -> Instant {
    gate.ready.wait().await;
    gate.start.wait().await;
    gate.started.lock().unwrap().unwrap()
}

#[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
fn spawn_worker<F>(future: F) -> JoinHandle<WorkerResult>
where
    F: std::future::Future<Output = WorkerResult> + Send + 'static,
{
    tokio::spawn(future)
}

#[cfg(all(feature = "ylong_base", not(feature = "tokio_base")))]
enum StartGate {
    Async {
        ready_tx: mpsc::UnboundedSender<()>,
        start_rx: watch::Receiver<Option<Instant>>,
    },
    #[cfg(feature = "__ylong_current_thread_runtime")]
    Thread {
        ready: Arc<std::sync::Barrier>,
        start: Arc<std::sync::Barrier>,
        started: Arc<std::sync::Mutex<Option<Instant>>>,
    },
}

#[cfg(all(feature = "ylong_base", not(feature = "tokio_base")))]
struct StartCoordinator {
    workers: usize,
    ready_rx: mpsc::UnboundedReceiver<()>,
    ready_tx: mpsc::UnboundedSender<()>,
    start_tx: watch::Sender<Option<Instant>>,
}

#[cfg(all(feature = "ylong_base", not(feature = "tokio_base")))]
impl StartCoordinator {
    fn new(workers: usize) -> Self {
        let (ready_tx, ready_rx) = mpsc::unbounded_channel();
        let (start_tx, _) = watch::channel(None);
        Self {
            workers,
            ready_rx,
            ready_tx,
            start_tx,
        }
    }

    fn worker_gate(&mut self) -> StartGate {
        StartGate::Async {
            ready_tx: self.ready_tx.clone(),
            start_rx: self.start_tx.subscribe(),
        }
    }

    async fn wait_all(&mut self) -> Instant {
        for _ in 0..self.workers {
            if self.ready_rx.recv().await.is_err() {
                break;
            }
        }
        let started = Instant::now();
        // A single watch broadcast avoids O(workers) start-channel sends in the
        // timed window; worker receivers are versioned, so they cannot miss it.
        let _ = self.start_tx.send(Some(started));
        started
    }
}

#[cfg(all(feature = "ylong_base", not(feature = "tokio_base")))]
async fn worker_ready_and_wait(gate: StartGate) -> Instant {
    match gate {
        StartGate::Async {
            ready_tx,
            mut start_rx,
        } => {
            let _ = ready_tx.send(());
            if start_rx.notified().await.is_ok() {
                start_rx.borrow_notify().unwrap_or_else(Instant::now)
            } else {
                Instant::now()
            }
        }
        #[cfg(feature = "__ylong_current_thread_runtime")]
        StartGate::Thread {
            ready,
            start,
            started,
        } => {
            ready.wait();
            start.wait();
            started.lock().unwrap().unwrap_or_else(Instant::now)
        }
    }
}

#[cfg(all(
    feature = "ylong_base",
    feature = "__ylong_current_thread_runtime",
    not(feature = "tokio_base")
))]
fn thread_gate_ready_and_wait(gate: StartGate) -> Instant {
    match gate {
        StartGate::Thread {
            ready,
            start,
            started,
        } => {
            ready.wait();
            start.wait();
            started.lock().unwrap().unwrap_or_else(Instant::now)
        }
        StartGate::Async { .. } => Instant::now(),
    }
}

#[cfg(all(
    feature = "ylong_base",
    feature = "__ylong_current_thread_runtime",
    not(feature = "tokio_base")
))]
fn signal_gate_ready(gate: StartGate) {
    match gate {
        StartGate::Async { ready_tx, .. } => {
            let _ = ready_tx.send(());
        }
        StartGate::Thread { .. } => {}
    }
}

#[cfg(all(feature = "ylong_base", not(feature = "tokio_base")))]
fn spawn_worker<F>(future: F) -> JoinHandle<WorkerResult>
where
    F: std::future::Future<Output = WorkerResult> + Send + 'static,
{
    ylong_runtime::spawn(future)
}

fn build_client(config: &Config) -> Result<ylong_http_client::async_impl::Client, HttpClientError> {
    let mut builder = ClientBuilder::new().max_h1_conn_number(config.concurrency);
    if !config.proxy.is_empty() {
        let mut proxy_tls = TlsConfig::builder();
        if let Some(path) = &config.proxy_ca_file {
            proxy_tls = proxy_tls.ca_file(path);
        }
        if let Some(path) = &config.proxy_client_cert {
            proxy_tls = proxy_tls.certificate_chain_file(path);
        }
        if let Some(path) = &config.proxy_client_key {
            proxy_tls = proxy_tls.private_key_file(path, TlsFileType::PEM);
        }
        if config.insecure_proxy {
            proxy_tls = proxy_tls
                .danger_accept_invalid_certs(true)
                .danger_accept_invalid_hostnames(true);
        }

        let mut proxy = Proxy::all(&config.proxy).proxy_tls_config(proxy_tls.build()?);
        if let Some(user_pass) = &config.proxy_user_pass {
            let (username, password) = user_pass.split_once(':').ok_or_else(|| {
                HttpClientError::other(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "proxy user pass must be username:password",
                ))
            })?;
            proxy = proxy.basic_auth(username, password);
        }
        builder = builder.proxy(proxy.build()?);
    }
    if let Some(path) = &config.origin_ca_file {
        builder = builder.tls_ca_file(path);
    }
    if config.insecure_origin {
        builder = builder
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true);
    }
    builder.build()
}

async fn run_worker(
    client: Option<Arc<ylong_http_client::async_impl::Client>>,
    config: Arc<Config>,
    worker_id: usize,
    warmup_count: usize,
    measured_count: usize,
    gate: StartGate,
) -> WorkerResult {
    let client = match client {
        Some(client) => client,
        None => match build_client(&config) {
            Ok(client) => Arc::new(client),
            Err(_) => {
                worker_ready_and_wait(gate).await;
                return WorkerResult {
                    latencies_us: Vec::new(),
                    bytes: 0,
                    body_reads: 0,
                    errors: warmup_count + measured_count,
                    start_delay_us: 0,
                    elapsed_us: 0,
                    trace: TraceSamples::default(),
                    phase: PhaseSamples::default(),
                };
            }
        },
    };
    let mut result = WorkerResult {
        latencies_us: Vec::with_capacity(measured_count),
        bytes: 0,
        body_reads: 0,
        errors: 0,
        start_delay_us: 0,
        elapsed_us: 0,
        trace: TraceSamples::default(),
        phase: PhaseSamples::default(),
    };
    let mut read_buffer = vec![0; config.read_buffer_size];

    for warmup_index in 0..warmup_count {
        if let Err(err) = request_once(
            &client,
            &config.url,
            &config.method,
            config.body_size,
            &mut read_buffer,
            config.ylong_read_chunk_size,
            None,
            false,
            config.yield_after_body_read,
        )
        .await
        {
            log_request_error(worker_id, "warmup", warmup_index, &err);
            result.errors += 1;
        }
    }

    let measured_requests = if config.prebuilt_requests {
        let mut requests = Vec::with_capacity(measured_count);
        for _ in 0..measured_count {
            match build_request(&config.url, &config.method, config.body_size) {
                Ok(request) => requests.push(request),
                Err(_) => {
                    result.errors += measured_count;
                    let measured_started = worker_ready_and_wait(gate).await;
                    result.start_delay_us = measured_started.elapsed().as_micros();
                    return result;
                }
            }
        }
        Some(requests)
    } else {
        None
    };

    let measured_started = worker_ready_and_wait(gate).await;
    result.start_delay_us = measured_started.elapsed().as_micros();
    if let Some(requests) = measured_requests {
        for (request_index, request) in requests.into_iter().enumerate() {
            let started = Instant::now();
            match send_request(
                &client,
                request,
                &mut read_buffer,
                config.ylong_read_chunk_size,
                trace_context(config.trace_summary, worker_id, request_index),
                config.phase_summary,
                config.yield_after_body_read,
            )
            .await
            {
                Ok(stats) => {
                    result.latencies_us.push(started.elapsed().as_micros());
                    result.bytes += stats.bytes;
                    result.body_reads += stats.body_reads;
                    if let Some(trace) = stats.trace {
                        result.trace.push(trace);
                    }
                    if let Some(phase) = stats.phase {
                        result.phase.push(phase);
                    }
                }
                Err(err) => {
                    log_request_error(worker_id, "measured", request_index, &err);
                    result.errors += 1;
                }
            }
        }
    } else {
        for request_index in 0..measured_count {
            let started = Instant::now();
            match request_once(
                &client,
                &config.url,
                &config.method,
                config.body_size,
                &mut read_buffer,
                config.ylong_read_chunk_size,
                trace_context(config.trace_summary, worker_id, request_index),
                config.phase_summary,
                config.yield_after_body_read,
            )
            .await
            {
                Ok(stats) => {
                    result.latencies_us.push(started.elapsed().as_micros());
                    result.bytes += stats.bytes;
                    result.body_reads += stats.body_reads;
                    if let Some(trace) = stats.trace {
                        result.trace.push(trace);
                    }
                    if let Some(phase) = stats.phase {
                        result.phase.push(phase);
                    }
                }
                Err(err) => {
                    log_request_error(worker_id, "measured", request_index, &err);
                    result.errors += 1;
                }
            }
        }
    }

    result.elapsed_us = measured_started.elapsed().as_micros();
    result
}

fn build_request(url: &str, method: &str, body_size: usize) -> Result<Request, HttpClientError> {
    let body = if method == "POST" {
        Body::slice(vec![b'x'; body_size])
    } else {
        Body::empty()
    };
    Request::builder().method(method).url(url).body(body)
}

fn trace_context(enabled: bool, worker_id: usize, request_index: usize) -> Option<TraceContext> {
    enabled.then_some(TraceContext {
        worker_id,
        request_index,
        stage: "request",
    })
}

fn log_request_error(worker_id: usize, phase: &str, request_index: usize, err: &HttpClientError) {
    if env::var_os("BENCH_LOG_ERRORS").is_some() {
        eprintln!(
            "request error: worker={worker_id} phase={phase} request={request_index} error={err:?}"
        );
    }
}

struct ResponseStats {
    bytes: u64,
    body_reads: u64,
    trace: Option<ResponseTrace>,
    phase: Option<ResponsePhase>,
}

async fn request_once(
    client: &ylong_http_client::async_impl::Client,
    url: &str,
    method: &str,
    body_size: usize,
    read_buffer: &mut [u8],
    read_chunk_size: usize,
    trace_context: Option<TraceContext>,
    phase_summary: bool,
    yield_after_body_read: bool,
) -> Result<ResponseStats, HttpClientError> {
    let request = build_request(url, method, body_size)?;
    send_request(
        client,
        request,
        read_buffer,
        read_chunk_size,
        trace_context,
        phase_summary,
        yield_after_body_read,
    )
    .await
}

async fn send_request(
    client: &ylong_http_client::async_impl::Client,
    request: Request,
    read_buffer: &mut [u8],
    read_chunk_size: usize,
    trace_context: Option<TraceContext>,
    phase_summary: bool,
    yield_after_body_read: bool,
) -> Result<ResponseStats, HttpClientError> {
    let Some(trace_context) = trace_context else {
        return send_request_no_trace(
            client,
            request,
            read_buffer,
            read_chunk_size,
            phase_summary,
            yield_after_body_read,
        )
        .await;
    };

    let request_started = Instant::now();
    let (response, request_poll_trace) = measure_future(
        client.request(request),
        Some(trace_context.stage("request")),
    )
    .await;
    let mut response = response?;
    let request_ready_us = request_started.elapsed().as_micros();
    if !response.status().is_successful() {
        return Err(HttpClientError::other(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unexpected non-2xx response",
        )));
    }

    let connect_us = response
        .time_group()
        .connect_duration()
        .map(|d| d.as_micros());
    let request_write_us = response
        .time_group()
        .request_write_duration()
        .map(|d| d.as_micros());
    let response_wait_us = response
        .time_group()
        .response_wait_duration()
        .map(|d| d.as_micros());
    let transfer_us = response
        .time_group()
        .transfer_duration()
        .map(|d| d.as_micros());
    let body_started = Instant::now();
    let mut body_first_byte_us = None;
    let mut body_poll_trace = FuturePollTrace::default();
    let mut body_read_wait_us = Vec::new();
    let body_eof_wait_us;
    let mut bytes = 0;
    let mut body_reads = 0;
    let read_chunk_len = body_read_chunk_len(read_buffer.len(), read_chunk_size);
    loop {
        let read_started = Instant::now();
        let (size, read_poll_trace) = measure_future(
            response.data(&mut read_buffer[..read_chunk_len]),
            Some(trace_context.stage("body")),
        )
        .await;
        let size = size?;
        let read_wait_us = read_started.elapsed().as_micros();
        if size == 0 {
            body_eof_wait_us = Some(read_wait_us);
            body_poll_trace.merge(read_poll_trace);
            break;
        }
        body_poll_trace.merge(read_poll_trace);
        body_read_wait_us.push(read_wait_us);
        if body_first_byte_us.is_none() {
            body_first_byte_us = Some(body_started.elapsed().as_micros());
        }
        bytes += size as u64;
        body_reads += 1;
        maybe_yield_after_body_read(yield_after_body_read).await;
    }
    let body_drain_us = body_started.elapsed().as_micros();
    let phase = Some(ResponsePhase {
        connect_us,
        request_write_us,
        response_wait_us,
        transfer_us,
        body_first_byte_us,
        body_drain_us,
        bytes,
        body_reads,
    });
    let trace = Some(ResponseTrace {
        request_ready_us,
        request_poll_trace,
        connect_us,
        request_write_us,
        response_wait_us,
        transfer_us,
        body_first_byte_us,
        body_drain_us,
        body_poll_trace,
        body_read_wait_us,
        body_eof_wait_us,
        bytes,
        body_reads,
    });
    Ok(ResponseStats {
        bytes,
        body_reads,
        trace,
        phase,
    })
}

async fn send_request_no_trace(
    client: &ylong_http_client::async_impl::Client,
    request: Request,
    read_buffer: &mut [u8],
    read_chunk_size: usize,
    phase_summary: bool,
    yield_after_body_read: bool,
) -> Result<ResponseStats, HttpClientError> {
    let mut response = client.request(request).await?;
    if !response.status().is_successful() {
        return Err(HttpClientError::other(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unexpected non-2xx response",
        )));
    }

    let connect_us = phase_summary
        .then(|| {
            response
                .time_group()
                .connect_duration()
                .map(|d| d.as_micros())
        })
        .flatten();
    let request_write_us = phase_summary
        .then(|| {
            response
                .time_group()
                .request_write_duration()
                .map(|d| d.as_micros())
        })
        .flatten();
    let response_wait_us = phase_summary
        .then(|| {
            response
                .time_group()
                .response_wait_duration()
                .map(|d| d.as_micros())
        })
        .flatten();
    let transfer_us = phase_summary
        .then(|| {
            response
                .time_group()
                .transfer_duration()
                .map(|d| d.as_micros())
        })
        .flatten();
    let body_started = phase_summary.then(Instant::now);
    let mut body_first_byte_us = None;
    let mut bytes = 0;
    let mut body_reads = 0;
    let read_chunk_len = body_read_chunk_len(read_buffer.len(), read_chunk_size);
    loop {
        let size = response.data(&mut read_buffer[..read_chunk_len]).await?;
        if size == 0 {
            break;
        }
        if body_first_byte_us.is_none() {
            if let Some(started) = body_started {
                body_first_byte_us = Some(started.elapsed().as_micros());
            }
        }
        bytes += size as u64;
        body_reads += 1;
        maybe_yield_after_body_read(yield_after_body_read).await;
    }

    let phase = body_started.map(|started| ResponsePhase {
        connect_us,
        request_write_us,
        response_wait_us,
        transfer_us,
        body_first_byte_us,
        body_drain_us: started.elapsed().as_micros(),
        bytes,
        body_reads,
    });

    Ok(ResponseStats {
        bytes,
        body_reads,
        trace: None,
        phase,
    })
}

async fn maybe_yield_after_body_read(enabled: bool) {
    if !enabled {
        return;
    }

    #[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
    tokio::task::yield_now().await;

    #[cfg(all(feature = "ylong_base", not(feature = "tokio_base")))]
    ylong_runtime::task::yield_now().await;
}

fn body_read_chunk_len(buffer_len: usize, read_chunk_size: usize) -> usize {
    if read_chunk_size == 0 {
        buffer_len
    } else {
        read_chunk_size.min(buffer_len)
    }
}

async fn measure_future<F>(
    future: F,
    trace_context: Option<TraceContext>,
) -> (F::Output, FuturePollTrace)
where
    F: Future,
{
    let Some(trace_context) = trace_context else {
        return (future.await, FuturePollTrace::default());
    };

    let mut future = Box::pin(future);
    let mut trace = FuturePollTrace::default();
    let mut last_pending: Option<(Instant, String, u64)> = None;
    let output = poll_fn(|cx| {
        trace.polls += 1;
        if let Some((instant, pending_thread, pending_poll)) = last_pending.take() {
            let resume_thread = current_thread_label();
            let gap_us = instant.elapsed().as_micros();
            if pending_thread == resume_thread {
                trace.pending_gap_same_thread_us.push(gap_us);
            } else {
                trace.pending_gap_migrated_us.push(gap_us);
            }
            let sample = PendingGapSample {
                gap_us,
                worker_id: trace_context.worker_id,
                request_index: trace_context.request_index,
                stage: trace_context.stage,
                pending_poll,
                resume_poll: trace.polls,
                pending_thread,
                resume_thread,
            };
            trace.pending_gap_us.push(sample.gap_us);
            update_max_pending_gap(&mut trace.max_pending_gap, Some(sample));
        }
        match future.as_mut().poll(cx) {
            std::task::Poll::Ready(value) => std::task::Poll::Ready(value),
            std::task::Poll::Pending => {
                trace.pending += 1;
                last_pending = Some((Instant::now(), current_thread_label(), trace.polls));
                std::task::Poll::Pending
            }
        }
    })
    .await;

    (output, trace)
}

async fn join_worker(handle: JoinHandle<WorkerResult>) -> WorkerResult {
    match handle.await {
        Ok(result) => result,
        Err(_) => WorkerResult {
            latencies_us: Vec::new(),
            bytes: 0,
            body_reads: 0,
            errors: 1,
            start_delay_us: 0,
            elapsed_us: 0,
            trace: TraceSamples::default(),
            phase: PhaseSamples::default(),
        },
    }
}

fn average_read_size(bytes: u64, body_reads: u64) -> f64 {
    if body_reads == 0 {
        0.0
    } else {
        bytes as f64 / body_reads as f64
    }
}

fn requests_for_worker(total: usize, concurrency: usize, worker: usize) -> usize {
    let base = total / concurrency;
    let extra = total % concurrency;
    if worker < extra {
        base + 1
    } else {
        base
    }
}

fn min_u128(values: &[u128]) -> u128 {
    values.iter().copied().min().unwrap_or_default()
}

fn max_u128(values: &[u128]) -> u128 {
    values.iter().copied().max().unwrap_or_default()
}

fn average_u128(values: &[u128]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<u128>() as f64 / values.len() as f64
    }
}

fn current_thread_label() -> String {
    format!("{:?}", thread::current().id())
}

fn pending_gap_sample_json(sample: Option<&PendingGapSample>) -> String {
    match sample {
        Some(sample) => format!(
            "{{\"stage\":\"{}\",\"worker\":{},\"request\":{},\"gap_us\":{},\"pending_poll\":{},\"resume_poll\":{},\"pending_thread\":\"{}\",\"resume_thread\":\"{}\"}}",
            sample.stage,
            sample.worker_id,
            sample.request_index,
            sample.gap_us,
            sample.pending_poll,
            sample.resume_poll,
            escape_json(&sample.pending_thread),
            escape_json(&sample.resume_thread),
        ),
        None => "null".to_string(),
    }
}

fn print_trace_summary(
    config: &Config,
    completed: usize,
    errors: usize,
    trace: &mut TraceSamples,
    worker_start_delay_us: &[u128],
    worker_elapsed_us: &[u128],
) {
    trace.sort();
    let request_pending_gap_max_sample =
        pending_gap_sample_json(trace.request_max_pending_gap.as_ref());
    let body_pending_gap_max_sample = pending_gap_sample_json(trace.body_max_pending_gap.as_ref());
    println!(
        "{{\"kind\":\"request_trace_summary\",\"client\":\"{}\",\"runtime_backend\":\"{}\",\"runtime_mode\":\"{}\",\"url\":\"{}\",\"proxy\":\"{}\",\"completed\":{},\"errors\":{},\"concurrency\":{},\"runtime_threads\":{},\"runtime_affinity\":{},\"request_ready_p50_us\":{},\"request_ready_p90_us\":{},\"request_ready_p99_us\":{},\"request_poll_count_p99\":{},\"request_pending_count_p99\":{},\"request_pending_gap_p99_us\":{},\"request_pending_gap_max_us\":{},\"request_pending_gap_same_thread_samples\":{},\"request_pending_gap_same_thread_p99_us\":{},\"request_pending_gap_migrated_samples\":{},\"request_pending_gap_migrated_p99_us\":{},\"request_pending_gap_max_sample\":{},\"connect_samples\":{},\"connect_p99_us\":{},\"request_write_samples\":{},\"request_write_p99_us\":{},\"response_wait_samples\":{},\"response_wait_p99_us\":{},\"transfer_samples\":{},\"transfer_p99_us\":{},\"body_first_byte_p99_us\":{},\"body_drain_p50_us\":{},\"body_drain_p90_us\":{},\"body_drain_p99_us\":{},\"body_poll_count_p99\":{},\"body_pending_count_p99\":{},\"body_pending_gap_p99_us\":{},\"body_pending_gap_max_us\":{},\"body_pending_gap_same_thread_samples\":{},\"body_pending_gap_same_thread_p99_us\":{},\"body_pending_gap_migrated_samples\":{},\"body_pending_gap_migrated_p99_us\":{},\"body_pending_gap_max_sample\":{},\"body_read_wait_samples\":{},\"body_read_wait_avg_us\":{:.3},\"body_read_wait_p90_us\":{},\"body_read_wait_p99_us\":{},\"body_read_wait_max_us\":{},\"body_eof_wait_p99_us\":{},\"body_reads_per_request_p50\":{},\"body_reads_per_request_p99\":{},\"bytes_per_request_p50\":{},\"worker_start_delay_us_min\":{},\"worker_start_delay_us_max\":{},\"worker_elapsed_us_min\":{},\"worker_elapsed_us_max\":{}}}",
        CLIENT_NAME,
        RUNTIME_BACKEND,
        config.runtime_mode.as_str(),
        escape_json(&config.url),
        escape_json(&config.proxy),
        completed,
        errors,
        config.concurrency,
        config.runtime_threads,
        config.runtime_affinity,
        percentile(&trace.request_ready_us, 50),
        percentile(&trace.request_ready_us, 90),
        percentile(&trace.request_ready_us, 99),
        percentile(&trace.request_poll_count, 99),
        percentile(&trace.request_pending_count, 99),
        percentile(&trace.request_pending_gap_us, 99),
        max_u128(&trace.request_pending_gap_us),
        trace.request_pending_gap_same_thread_us.len(),
        percentile(&trace.request_pending_gap_same_thread_us, 99),
        trace.request_pending_gap_migrated_us.len(),
        percentile(&trace.request_pending_gap_migrated_us, 99),
        request_pending_gap_max_sample,
        trace.connect_us.len(),
        percentile(&trace.connect_us, 99),
        trace.request_write_us.len(),
        percentile(&trace.request_write_us, 99),
        trace.response_wait_us.len(),
        percentile(&trace.response_wait_us, 99),
        trace.transfer_us.len(),
        percentile(&trace.transfer_us, 99),
        percentile(&trace.body_first_byte_us, 99),
        percentile(&trace.body_drain_us, 50),
        percentile(&trace.body_drain_us, 90),
        percentile(&trace.body_drain_us, 99),
        percentile(&trace.body_poll_count, 99),
        percentile(&trace.body_pending_count, 99),
        percentile(&trace.body_pending_gap_us, 99),
        max_u128(&trace.body_pending_gap_us),
        trace.body_pending_gap_same_thread_us.len(),
        percentile(&trace.body_pending_gap_same_thread_us, 99),
        trace.body_pending_gap_migrated_us.len(),
        percentile(&trace.body_pending_gap_migrated_us, 99),
        body_pending_gap_max_sample,
        trace.body_read_wait_us.len(),
        average_u128(&trace.body_read_wait_us),
        percentile(&trace.body_read_wait_us, 90),
        percentile(&trace.body_read_wait_us, 99),
        max_u128(&trace.body_read_wait_us),
        percentile(&trace.body_eof_wait_us, 99),
        percentile(&trace.body_reads_per_request, 50),
        percentile(&trace.body_reads_per_request, 99),
        percentile(&trace.bytes_per_request, 50),
        min_u128(worker_start_delay_us),
        max_u128(worker_start_delay_us),
        min_u128(worker_elapsed_us),
        max_u128(worker_elapsed_us),
    );
}

fn print_phase_summary(config: &Config, completed: usize, errors: usize, phase: &mut PhaseSamples) {
    phase.sort();
    println!(
        "{{\"kind\":\"phase_summary\",\"client\":\"{}\",\"runtime_backend\":\"{}\",\"runtime_mode\":\"{}\",\"url\":\"{}\",\"proxy\":\"{}\",\"completed\":{},\"errors\":{},\"concurrency\":{},\"runtime_threads\":{},\"runtime_affinity\":{},\"connect_samples\":{},\"connect_us_p50\":{},\"connect_us_p90\":{},\"connect_us_p99\":{},\"request_write_samples\":{},\"request_write_us_p50\":{},\"request_write_us_p90\":{},\"request_write_us_p99\":{},\"response_wait_samples\":{},\"response_wait_us_p50\":{},\"response_wait_us_p90\":{},\"response_wait_us_p99\":{},\"transfer_samples\":{},\"transfer_us_p50\":{},\"transfer_us_p90\":{},\"transfer_us_p99\":{},\"body_first_byte_us_p50\":{},\"body_first_byte_us_p90\":{},\"body_first_byte_us_p99\":{},\"body_drain_us_p50\":{},\"body_drain_us_p90\":{},\"body_drain_us_p99\":{},\"body_reads_per_request_p50\":{},\"body_reads_per_request_p99\":{},\"bytes_per_request_p50\":{}}}",
        CLIENT_NAME,
        RUNTIME_BACKEND,
        config.runtime_mode.as_str(),
        escape_json(&config.url),
        escape_json(&config.proxy),
        completed,
        errors,
        config.concurrency,
        config.runtime_threads,
        config.runtime_affinity,
        phase.connect_us.len(),
        percentile(&phase.connect_us, 50),
        percentile(&phase.connect_us, 90),
        percentile(&phase.connect_us, 99),
        phase.request_write_us.len(),
        percentile(&phase.request_write_us, 50),
        percentile(&phase.request_write_us, 90),
        percentile(&phase.request_write_us, 99),
        phase.response_wait_us.len(),
        percentile(&phase.response_wait_us, 50),
        percentile(&phase.response_wait_us, 90),
        percentile(&phase.response_wait_us, 99),
        phase.transfer_us.len(),
        percentile(&phase.transfer_us, 50),
        percentile(&phase.transfer_us, 90),
        percentile(&phase.transfer_us, 99),
        percentile(&phase.body_first_byte_us, 50),
        percentile(&phase.body_first_byte_us, 90),
        percentile(&phase.body_first_byte_us, 99),
        percentile(&phase.body_drain_us, 50),
        percentile(&phase.body_drain_us, 90),
        percentile(&phase.body_drain_us, 99),
        percentile(&phase.body_reads_per_request, 50),
        percentile(&phase.body_reads_per_request, 99),
        percentile(&phase.bytes_per_request, 50),
    );
}

fn parse_args(args: Vec<String>) -> Result<Config, String> {
    let mut config = Config {
        url: String::new(),
        proxy: String::new(),
        requests: 1000,
        warmup_requests: 0,
        concurrency: 16,
        runtime_threads: 0,
        runtime_mode: RuntimeMode::MultiThread,
        runtime_affinity: false,
        read_buffer_size: 64 * 1024,
        ylong_read_chunk_size: 0,
        client_per_worker: false,
        prebuilt_requests: false,
        trace_summary: false,
        phase_summary: false,
        yield_after_body_read: false,
        method: "GET".to_string(),
        body_size: 0,
        proxy_ca_file: None,
        proxy_client_cert: None,
        proxy_client_key: None,
        origin_ca_file: None,
        insecure_proxy: false,
        insecure_origin: false,
        proxy_user_pass: None,
    };

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--url" => config.url = next_value(&mut iter, "--url")?,
            "--proxy" => config.proxy = next_value(&mut iter, "--proxy")?,
            "--requests" => {
                config.requests = parse_usize(&next_value(&mut iter, "--requests")?, "--requests")?
            }
            "--warmup-requests" => {
                config.warmup_requests = parse_usize(
                    &next_value(&mut iter, "--warmup-requests")?,
                    "--warmup-requests",
                )?
            }
            "--concurrency" => {
                config.concurrency =
                    parse_usize(&next_value(&mut iter, "--concurrency")?, "--concurrency")?
            }
            "--runtime-threads" => {
                config.runtime_threads = parse_usize(
                    &next_value(&mut iter, "--runtime-threads")?,
                    "--runtime-threads",
                )?
            }
            "--runtime-mode" => {
                config.runtime_mode = parse_runtime_mode(&next_value(&mut iter, "--runtime-mode")?)?
            }
            "--runtime-affinity" => config.runtime_affinity = true,
            "--read-buffer-size" => {
                config.read_buffer_size = parse_usize(
                    &next_value(&mut iter, "--read-buffer-size")?,
                    "--read-buffer-size",
                )?
            }
            "--ylong-read-chunk-size" => {
                config.ylong_read_chunk_size = parse_usize(
                    &next_value(&mut iter, "--ylong-read-chunk-size")?,
                    "--ylong-read-chunk-size",
                )?
            }
            "--client-per-worker" => config.client_per_worker = true,
            "--trace-summary" => config.trace_summary = true,
            "--phase-summary" => config.phase_summary = true,
            "--yield-after-body-read" => config.yield_after_body_read = true,
            "--method" => config.method = next_value(&mut iter, "--method")?.to_ascii_uppercase(),
            "--body-size" => {
                config.body_size =
                    parse_usize(&next_value(&mut iter, "--body-size")?, "--body-size")?
            }
            "--proxy-ca-file" => {
                config.proxy_ca_file = Some(next_value(&mut iter, "--proxy-ca-file")?)
            }
            "--proxy-client-cert" => {
                config.proxy_client_cert = Some(next_value(&mut iter, "--proxy-client-cert")?)
            }
            "--proxy-client-key" => {
                config.proxy_client_key = Some(next_value(&mut iter, "--proxy-client-key")?)
            }
            "--origin-ca-file" => {
                config.origin_ca_file = Some(next_value(&mut iter, "--origin-ca-file")?)
            }
            "--insecure-proxy" => config.insecure_proxy = true,
            "--insecure-origin" => config.insecure_origin = true,
            "--proxy-user-pass" => {
                config.proxy_user_pass = Some(next_value(&mut iter, "--proxy-user-pass")?)
            }
            "--help" | "-h" => return Err(String::new()),
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    if config.url.is_empty() {
        return Err("missing --url".to_string());
    }
    if config.requests == 0 {
        return Err("--requests must be greater than 0".to_string());
    }
    if config.concurrency == 0 {
        return Err("--concurrency must be greater than 0".to_string());
    }
    if config.runtime_threads == 0 {
        config.runtime_threads = config.concurrency;
    }
    #[cfg(not(all(
        feature = "ylong_base",
        feature = "__ylong_current_thread_runtime",
        not(feature = "tokio_base")
    )))]
    if matches!(
        config.runtime_mode,
        RuntimeMode::CurrentThreadPerWorker | RuntimeMode::CurrentThreadSharded
    ) {
        return Err(
            "--runtime-mode current-thread modes require async ylong current-thread build"
                .to_string(),
        );
    }
    match config.runtime_mode {
        RuntimeMode::MultiThread => {}
        RuntimeMode::CurrentThreadPerWorker => {
            if config.runtime_affinity {
                return Err("--runtime-affinity requires --runtime-mode multi-thread".to_string());
            }
            config.runtime_threads = config.concurrency;
            config.client_per_worker = true;
        }
        RuntimeMode::CurrentThreadSharded => {
            if config.runtime_affinity {
                return Err("--runtime-affinity requires --runtime-mode multi-thread".to_string());
            }
            config.runtime_threads = config.runtime_threads.min(config.concurrency);
        }
    }
    if config.read_buffer_size == 0 {
        return Err("--read-buffer-size must be greater than 0".to_string());
    }
    if config.ylong_read_chunk_size == 0 {
        config.ylong_read_chunk_size = config.read_buffer_size;
    }
    if config.method != "GET" && config.method != "POST" {
        return Err("--method must be GET or POST".to_string());
    }
    if config.method == "GET" && config.body_size != 0 {
        return Err("--body-size is only supported with --method POST".to_string());
    }
    config.prebuilt_requests = config.method == "GET";

    Ok(config)
}

fn next_value<I>(iter: &mut I, name: &str) -> Result<String, String>
where
    I: Iterator<Item = String>,
{
    iter.next()
        .ok_or_else(|| format!("{name} requires a value"))
}

fn parse_usize(value: &str, name: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("{name} must be an integer"))
}

fn parse_runtime_mode(value: &str) -> Result<RuntimeMode, String> {
    match value {
        "multi-thread" => Ok(RuntimeMode::MultiThread),
        "current-thread-per-worker" => Ok(RuntimeMode::CurrentThreadPerWorker),
        "current-thread-sharded" => Ok(RuntimeMode::CurrentThreadSharded),
        _ => Err(
            "--runtime-mode must be multi-thread, current-thread-per-worker, or current-thread-sharded"
                .to_string(),
        ),
    }
}

fn percentile(sorted: &[u128], pct: usize) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let index = ((sorted.len() - 1) * pct) / 100;
    sorted[index]
}

fn escape_json(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn usage() -> &'static str {
    "usage: async_https_proxy_bench --url URL [--proxy http[s]://PROXY[:PORT]] [--requests N] [--warmup-requests N] [--concurrency N] [--runtime-threads N] [--runtime-mode multi-thread|current-thread-per-worker|current-thread-sharded] [--runtime-affinity] [--read-buffer-size N] [--ylong-read-chunk-size N] [--client-per-worker] [--trace-summary] [--phase-summary] [--yield-after-body-read] [--method GET|POST] [--body-size N] [--proxy-ca-file PEM] [--proxy-client-cert PEM] [--proxy-client-key PEM] [--origin-ca-file PEM] [--insecure-proxy] [--insecure-origin] [--proxy-user-pass user:pass]"
}
