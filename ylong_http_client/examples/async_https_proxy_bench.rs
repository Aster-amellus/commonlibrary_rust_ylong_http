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
use std::sync::Arc;
use std::time::Instant;

#[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
use tokio::runtime::Builder;
#[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
use tokio::sync::Barrier;
#[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
use tokio::task::JoinHandle;
#[cfg(all(feature = "ylong_base", not(feature = "tokio_base")))]
use ylong_runtime::builder::RuntimeBuilder;
#[cfg(all(feature = "ylong_base", not(feature = "tokio_base")))]
use ylong_runtime::sync::{mpsc, Waiter};
#[cfg(all(feature = "ylong_base", not(feature = "tokio_base")))]
use ylong_runtime::task::JoinHandle;

use ylong_http_client::async_impl::{Body, ClientBuilder, Request};
use ylong_http_client::{HttpClientError, Proxy, TlsConfig, TlsFileType};

#[cfg(feature = "__c_openssl")]
use openssl as _;

struct Config {
    url: String,
    proxy: String,
    requests: usize,
    warmup_requests: usize,
    concurrency: usize,
    runtime_threads: usize,
    read_buffer_size: usize,
    client_per_worker: bool,
    prebuilt_requests: bool,
    trace_summary: bool,
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

struct WorkerResult {
    latencies_us: Vec<u128>,
    bytes: u64,
    body_reads: u64,
    errors: usize,
    elapsed_us: u128,
    trace: TraceSamples,
}

#[derive(Default)]
struct TraceSamples {
    request_ready_us: Vec<u128>,
    connect_us: Vec<u128>,
    transfer_us: Vec<u128>,
    body_first_byte_us: Vec<u128>,
    body_drain_us: Vec<u128>,
    body_read_wait_us: Vec<u128>,
    body_eof_wait_us: Vec<u128>,
    body_reads_per_request: Vec<u128>,
    bytes_per_request: Vec<u128>,
}

impl TraceSamples {
    fn merge(&mut self, mut other: TraceSamples) {
        self.request_ready_us.append(&mut other.request_ready_us);
        self.connect_us.append(&mut other.connect_us);
        self.transfer_us.append(&mut other.transfer_us);
        self.body_first_byte_us
            .append(&mut other.body_first_byte_us);
        self.body_drain_us.append(&mut other.body_drain_us);
        self.body_read_wait_us.append(&mut other.body_read_wait_us);
        self.body_eof_wait_us.append(&mut other.body_eof_wait_us);
        self.body_reads_per_request
            .append(&mut other.body_reads_per_request);
        self.bytes_per_request.append(&mut other.bytes_per_request);
    }

    fn push(&mut self, trace: ResponseTrace) {
        self.request_ready_us.push(trace.request_ready_us);
        if let Some(connect_us) = trace.connect_us {
            self.connect_us.push(connect_us);
        }
        if let Some(transfer_us) = trace.transfer_us {
            self.transfer_us.push(transfer_us);
        }
        if let Some(body_first_byte_us) = trace.body_first_byte_us {
            self.body_first_byte_us.push(body_first_byte_us);
        }
        self.body_drain_us.push(trace.body_drain_us);
        self.body_read_wait_us.extend(trace.body_read_wait_us);
        if let Some(body_eof_wait_us) = trace.body_eof_wait_us {
            self.body_eof_wait_us.push(body_eof_wait_us);
        }
        self.body_reads_per_request.push(trace.body_reads as u128);
        self.bytes_per_request.push(trace.bytes as u128);
    }

    fn sort(&mut self) {
        self.request_ready_us.sort_unstable();
        self.connect_us.sort_unstable();
        self.transfer_us.sort_unstable();
        self.body_first_byte_us.sort_unstable();
        self.body_drain_us.sort_unstable();
        self.body_read_wait_us.sort_unstable();
        self.body_eof_wait_us.sort_unstable();
        self.body_reads_per_request.sort_unstable();
        self.bytes_per_request.sort_unstable();
    }
}

struct ResponseTrace {
    request_ready_us: u128,
    connect_us: Option<u128>,
    transfer_us: Option<u128>,
    body_first_byte_us: Option<u128>,
    body_drain_us: u128,
    body_read_wait_us: Vec<u128>,
    body_eof_wait_us: Option<u128>,
    bytes: u64,
    body_reads: u64,
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
        RuntimeBuilder::new_multi_thread()
            .worker_num(config.runtime_threads)
            .build_global()?;
        ylong_runtime::block_on(run(config))
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
            run_worker(client, config, warmup_count, measured_count, gate).await
        }));
    }

    start.wait_all().await;
    let started = Instant::now();

    let mut latencies = Vec::with_capacity(config.requests);
    let mut bytes = 0;
    let mut body_reads = 0;
    let mut errors = 0;
    let mut worker_elapsed_us = Vec::with_capacity(config.concurrency);
    let mut trace = TraceSamples::default();
    for handle in handles {
        let result = join_worker(handle).await;
        latencies.extend(result.latencies_us);
        bytes += result.bytes;
        body_reads += result.body_reads;
        errors += result.errors;
        if result.elapsed_us != 0 {
            worker_elapsed_us.push(result.elapsed_us);
        }
        trace.merge(result.trace);
    }
    let elapsed = started.elapsed();

    latencies.sort_unstable();

    let completed = latencies.len();
    let elapsed_ms = elapsed.as_secs_f64() * 1000.0;
    let rps = if elapsed.as_secs_f64() == 0.0 {
        0.0
    } else {
        completed as f64 / elapsed.as_secs_f64()
    };

    println!(
        "{{\"client\":\"ylong_http_client\",\"url\":\"{}\",\"proxy\":\"{}\",\"method\":\"{}\",\"body_size\":{},\"requests\":{},\"warmup_requests\":{},\"completed\":{},\"errors\":{},\"concurrency\":{},\"runtime_threads\":{},\"read_buffer_size\":{},\"client_per_worker\":{},\"prebuilt_requests\":{},\"trace_summary\":{},\"bytes\":{},\"body_reads\":{},\"avg_body_read_size\":{:.3},\"elapsed_ms\":{:.3},\"rps\":{:.3},\"latency_us_p50\":{},\"latency_us_p90\":{},\"latency_us_p95\":{},\"latency_us_p99\":{},\"worker_elapsed_us_min\":{},\"worker_elapsed_us_max\":{}}}",
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
        config.read_buffer_size,
        config.client_per_worker,
        config.prebuilt_requests,
        config.trace_summary,
        bytes,
        body_reads,
        average_read_size(bytes, body_reads),
        elapsed_ms,
        rps,
        percentile(&latencies, 50),
        percentile(&latencies, 90),
        percentile(&latencies, 95),
        percentile(&latencies, 99),
        min_u128(&worker_elapsed_us),
        max_u128(&worker_elapsed_us),
    );

    if config.trace_summary {
        print_trace_summary(&config, completed, errors, &mut trace, &worker_elapsed_us);
    }

    Ok(())
}

#[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
type StartGate = Arc<Barrier>;

#[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
struct StartCoordinator {
    barrier: Arc<Barrier>,
}

#[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
impl StartCoordinator {
    fn new(workers: usize) -> Self {
        Self {
            barrier: Arc::new(Barrier::new(workers + 1)),
        }
    }

    fn worker_gate(&self) -> StartGate {
        self.barrier.clone()
    }

    async fn wait_all(&mut self) {
        self.barrier.wait().await;
    }
}

#[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
async fn worker_ready_and_wait(gate: StartGate) {
    gate.wait().await;
}

#[cfg(all(feature = "tokio_base", not(feature = "ylong_base")))]
fn spawn_worker<F>(future: F) -> JoinHandle<WorkerResult>
where
    F: std::future::Future<Output = WorkerResult> + Send + 'static,
{
    tokio::spawn(future)
}

#[cfg(all(feature = "ylong_base", not(feature = "tokio_base")))]
#[derive(Clone)]
struct StartGate {
    ready_tx: mpsc::UnboundedSender<()>,
    start: Arc<Waiter>,
}

#[cfg(all(feature = "ylong_base", not(feature = "tokio_base")))]
struct StartCoordinator {
    workers: usize,
    ready_rx: mpsc::UnboundedReceiver<()>,
    ready_tx: mpsc::UnboundedSender<()>,
    start: Arc<Waiter>,
}

#[cfg(all(feature = "ylong_base", not(feature = "tokio_base")))]
impl StartCoordinator {
    fn new(workers: usize) -> Self {
        let (ready_tx, ready_rx) = mpsc::unbounded_channel();
        Self {
            workers,
            ready_rx,
            ready_tx,
            start: Arc::new(Waiter::new()),
        }
    }

    fn worker_gate(&self) -> StartGate {
        StartGate {
            ready_tx: self.ready_tx.clone(),
            start: self.start.clone(),
        }
    }

    async fn wait_all(&mut self) {
        for _ in 0..self.workers {
            if self.ready_rx.recv().await.is_err() {
                break;
            }
        }
        for _ in 0..self.workers {
            self.start.wake_one();
        }
    }
}

#[cfg(all(feature = "ylong_base", not(feature = "tokio_base")))]
async fn worker_ready_and_wait(gate: StartGate) {
    let _ = gate.ready_tx.send(());
    gate.start.wait().await;
}

#[cfg(all(feature = "ylong_base", not(feature = "tokio_base")))]
fn spawn_worker<F>(future: F) -> JoinHandle<WorkerResult>
where
    F: std::future::Future<Output = WorkerResult> + Send + 'static,
{
    ylong_runtime::spawn(future)
}

fn build_client(config: &Config) -> Result<ylong_http_client::async_impl::Client, HttpClientError> {
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

    let mut builder = ClientBuilder::new()
        .proxy(proxy.build()?)
        .max_h1_conn_number(config.concurrency);
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
                    elapsed_us: 0,
                    trace: TraceSamples::default(),
                };
            }
        },
    };
    let mut result = WorkerResult {
        latencies_us: Vec::with_capacity(measured_count),
        bytes: 0,
        body_reads: 0,
        errors: 0,
        elapsed_us: 0,
        trace: TraceSamples::default(),
    };
    let mut read_buffer = vec![0; config.read_buffer_size];

    for _ in 0..warmup_count {
        if request_once(
            &client,
            &config.url,
            &config.method,
            config.body_size,
            &mut read_buffer,
            false,
        )
        .await
        .is_err()
        {
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
                    worker_ready_and_wait(gate).await;
                    return result;
                }
            }
        }
        Some(requests)
    } else {
        None
    };

    worker_ready_and_wait(gate).await;

    let measured_started = Instant::now();
    if let Some(requests) = measured_requests {
        for request in requests {
            let started = Instant::now();
            match send_request(&client, request, &mut read_buffer, config.trace_summary).await {
                Ok(stats) => {
                    result.latencies_us.push(started.elapsed().as_micros());
                    result.bytes += stats.bytes;
                    result.body_reads += stats.body_reads;
                    if let Some(trace) = stats.trace {
                        result.trace.push(trace);
                    }
                }
                Err(_) => result.errors += 1,
            }
        }
    } else {
        for _ in 0..measured_count {
            let started = Instant::now();
            match request_once(
                &client,
                &config.url,
                &config.method,
                config.body_size,
                &mut read_buffer,
                config.trace_summary,
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
                }
                Err(_) => result.errors += 1,
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

struct ResponseStats {
    bytes: u64,
    body_reads: u64,
    trace: Option<ResponseTrace>,
}

async fn request_once(
    client: &ylong_http_client::async_impl::Client,
    url: &str,
    method: &str,
    body_size: usize,
    read_buffer: &mut [u8],
    trace_summary: bool,
) -> Result<ResponseStats, HttpClientError> {
    let request = build_request(url, method, body_size)?;
    send_request(client, request, read_buffer, trace_summary).await
}

async fn send_request(
    client: &ylong_http_client::async_impl::Client,
    request: Request,
    read_buffer: &mut [u8],
    trace_summary: bool,
) -> Result<ResponseStats, HttpClientError> {
    let request_started = Instant::now();
    let mut response = client.request(request).await?;
    let request_ready_us = request_started.elapsed().as_micros();
    if !response.status().is_successful() {
        return Err(HttpClientError::other(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unexpected non-2xx response",
        )));
    }

    let connect_us = trace_summary
        .then(|| {
            response
                .time_group()
                .connect_duration()
                .map(|d| d.as_micros())
        })
        .flatten();
    let transfer_us = trace_summary
        .then(|| {
            response
                .time_group()
                .transfer_duration()
                .map(|d| d.as_micros())
        })
        .flatten();
    let body_started = Instant::now();
    let mut body_first_byte_us = None;
    let mut body_read_wait_us = Vec::new();
    let mut body_eof_wait_us = None;
    let mut bytes = 0;
    let mut body_reads = 0;
    loop {
        let read_started = Instant::now();
        let size = response.data(read_buffer).await?;
        let read_wait_us = read_started.elapsed().as_micros();
        if size == 0 {
            if trace_summary {
                body_eof_wait_us = Some(read_wait_us);
            }
            break;
        }
        if trace_summary {
            body_read_wait_us.push(read_wait_us);
            if body_first_byte_us.is_none() {
                body_first_byte_us = Some(body_started.elapsed().as_micros());
            }
        }
        bytes += size as u64;
        body_reads += 1;
    }
    let trace = trace_summary.then(|| ResponseTrace {
        request_ready_us,
        connect_us,
        transfer_us,
        body_first_byte_us,
        body_drain_us: body_started.elapsed().as_micros(),
        body_read_wait_us,
        body_eof_wait_us,
        bytes,
        body_reads,
    });
    Ok(ResponseStats {
        bytes,
        body_reads,
        trace,
    })
}

async fn join_worker(handle: JoinHandle<WorkerResult>) -> WorkerResult {
    match handle.await {
        Ok(result) => result,
        Err(_) => WorkerResult {
            latencies_us: Vec::new(),
            bytes: 0,
            body_reads: 0,
            errors: 1,
            elapsed_us: 0,
            trace: TraceSamples::default(),
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

fn print_trace_summary(
    config: &Config,
    completed: usize,
    errors: usize,
    trace: &mut TraceSamples,
    worker_elapsed_us: &[u128],
) {
    trace.sort();
    println!(
        "{{\"kind\":\"request_trace_summary\",\"client\":\"ylong_http_client\",\"url\":\"{}\",\"proxy\":\"{}\",\"completed\":{},\"errors\":{},\"concurrency\":{},\"runtime_threads\":{},\"request_ready_p50_us\":{},\"request_ready_p90_us\":{},\"request_ready_p99_us\":{},\"connect_samples\":{},\"connect_p99_us\":{},\"transfer_samples\":{},\"transfer_p99_us\":{},\"body_first_byte_p99_us\":{},\"body_drain_p50_us\":{},\"body_drain_p90_us\":{},\"body_drain_p99_us\":{},\"body_read_wait_samples\":{},\"body_read_wait_avg_us\":{:.3},\"body_read_wait_p90_us\":{},\"body_read_wait_p99_us\":{},\"body_read_wait_max_us\":{},\"body_eof_wait_p99_us\":{},\"body_reads_per_request_p50\":{},\"body_reads_per_request_p99\":{},\"bytes_per_request_p50\":{},\"worker_elapsed_us_min\":{},\"worker_elapsed_us_max\":{}}}",
        escape_json(&config.url),
        escape_json(&config.proxy),
        completed,
        errors,
        config.concurrency,
        config.runtime_threads,
        percentile(&trace.request_ready_us, 50),
        percentile(&trace.request_ready_us, 90),
        percentile(&trace.request_ready_us, 99),
        trace.connect_us.len(),
        percentile(&trace.connect_us, 99),
        trace.transfer_us.len(),
        percentile(&trace.transfer_us, 99),
        percentile(&trace.body_first_byte_us, 99),
        percentile(&trace.body_drain_us, 50),
        percentile(&trace.body_drain_us, 90),
        percentile(&trace.body_drain_us, 99),
        trace.body_read_wait_us.len(),
        average_u128(&trace.body_read_wait_us),
        percentile(&trace.body_read_wait_us, 90),
        percentile(&trace.body_read_wait_us, 99),
        max_u128(&trace.body_read_wait_us),
        percentile(&trace.body_eof_wait_us, 99),
        percentile(&trace.body_reads_per_request, 50),
        percentile(&trace.body_reads_per_request, 99),
        percentile(&trace.bytes_per_request, 50),
        min_u128(worker_elapsed_us),
        max_u128(worker_elapsed_us),
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
        read_buffer_size: 64 * 1024,
        client_per_worker: false,
        prebuilt_requests: false,
        trace_summary: false,
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
            "--read-buffer-size" => {
                config.read_buffer_size = parse_usize(
                    &next_value(&mut iter, "--read-buffer-size")?,
                    "--read-buffer-size",
                )?
            }
            "--client-per-worker" => config.client_per_worker = true,
            "--trace-summary" => config.trace_summary = true,
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
    if config.proxy.is_empty() {
        return Err("missing --proxy".to_string());
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
    if config.read_buffer_size == 0 {
        return Err("--read-buffer-size must be greater than 0".to_string());
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
    "usage: async_https_proxy_bench --url URL --proxy https://PROXY[:PORT] [--requests N] [--warmup-requests N] [--concurrency N] [--runtime-threads N] [--read-buffer-size N] [--client-per-worker] [--trace-summary] [--method GET|POST] [--body-size N] [--proxy-ca-file PEM] [--proxy-client-cert PEM] [--proxy-client-key PEM] [--origin-ca-file PEM] [--insecure-proxy] [--insecure-origin] [--proxy-user-pass user:pass]"
}
