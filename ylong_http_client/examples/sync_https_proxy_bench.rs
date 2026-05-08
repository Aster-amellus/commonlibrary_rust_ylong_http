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

//! Synchronous HTTPS proxy benchmark client for comparison with libcurl.

use std::env;
use std::sync::{Arc, Barrier};
use std::thread::JoinHandle;
use std::time::Instant;

use ylong_http_client::sync_impl::{
    Body, Client, ClientBuilder, Connector, HttpBody, Response, TextBody,
};
use ylong_http_client::{EmptyBody, HttpClientError, Proxy, Request, TlsConfig, TlsFileType};

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
    prebuilt_requests: bool,
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
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = match parse_args(env::args().skip(1).collect()) {
        Ok(config) => Arc::new(config),
        Err(message) => {
            eprintln!("{message}");
            eprintln!("{}", usage());
            std::process::exit(2);
        }
    };

    let result = run_workload(config.clone());

    let mut latencies = result.latencies_us;
    latencies.sort_unstable();
    let completed = latencies.len();
    let elapsed_ms = result.elapsed.as_secs_f64() * 1000.0;
    let rps = if result.elapsed.as_secs_f64() == 0.0 {
        0.0
    } else {
        completed as f64 / result.elapsed.as_secs_f64()
    };

    println!(
        "{{\"client\":\"ylong_http_client_sync\",\"url\":\"{}\",\"proxy\":\"{}\",\"method\":\"{}\",\"body_size\":{},\"requests\":{},\"warmup_requests\":{},\"completed\":{},\"errors\":{},\"concurrency\":{},\"runtime_threads\":{},\"read_buffer_size\":{},\"prebuilt_requests\":{},\"bytes\":{},\"body_reads\":{},\"avg_body_read_size\":{:.3},\"elapsed_ms\":{:.3},\"rps\":{:.3},\"latency_us_p50\":{},\"latency_us_p90\":{},\"latency_us_p95\":{},\"latency_us_p99\":{}}}",
        escape_json(&config.url),
        escape_json(&config.proxy),
        escape_json(&config.method),
        config.body_size,
        config.requests,
        config.warmup_requests,
        completed,
        result.errors,
        config.concurrency,
        config.runtime_threads,
        config.read_buffer_size,
        config.prebuilt_requests,
        result.bytes,
        result.body_reads,
        average_read_size(result.bytes, result.body_reads),
        elapsed_ms,
        rps,
        percentile(&latencies, 50),
        percentile(&latencies, 90),
        percentile(&latencies, 95),
        percentile(&latencies, 99),
    );

    Ok(())
}

struct WorkloadResult {
    latencies_us: Vec<u128>,
    bytes: u64,
    body_reads: u64,
    errors: usize,
    elapsed: std::time::Duration,
}

fn run_workload(config: Arc<Config>) -> WorkloadResult {
    let barrier = Arc::new(Barrier::new(config.concurrency + 1));
    let mut handles = Vec::with_capacity(config.concurrency);
    for worker in 0..config.concurrency {
        let measured_count = requests_for_worker(config.requests, config.concurrency, worker);
        let warmup_count = requests_for_worker(config.warmup_requests, config.concurrency, worker);
        let config = config.clone();
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || {
            run_worker(config, warmup_count, measured_count, barrier)
        }));
    }

    barrier.wait();
    let started = Instant::now();

    let mut latencies_us = Vec::with_capacity(config.requests);
    let mut bytes = 0;
    let mut body_reads = 0;
    let mut errors = 0;
    for handle in handles {
        let result = join_worker(handle);
        latencies_us.extend(result.latencies_us);
        bytes += result.bytes;
        body_reads += result.body_reads;
        errors += result.errors;
    }

    WorkloadResult {
        latencies_us,
        bytes,
        body_reads,
        errors,
        elapsed: started.elapsed(),
    }
}

fn build_client(config: &Config) -> Result<Client<impl Connector>, HttpClientError> {
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

    let mut builder = ClientBuilder::new().proxy(proxy.build()?);
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

fn run_worker(
    config: Arc<Config>,
    warmup_count: usize,
    measured_count: usize,
    barrier: Arc<Barrier>,
) -> WorkerResult {
    let Ok(client) = build_client(&config) else {
        barrier.wait();
        return WorkerResult {
            latencies_us: Vec::new(),
            bytes: 0,
            body_reads: 0,
            errors: warmup_count + measured_count,
        };
    };
    let mut read_buffer = vec![0; config.read_buffer_size];
    let mut errors = 0;
    for _ in 0..warmup_count {
        if request_once(&client, &config, &mut read_buffer).is_err() {
            errors += 1;
        }
    }

    let measured_requests = if config.prebuilt_requests {
        let mut requests = Vec::with_capacity(measured_count);
        for _ in 0..measured_count {
            match Request::get(config.url.as_str()).body(EmptyBody) {
                Ok(request) => requests.push(request),
                Err(_) => {
                    barrier.wait();
                    return WorkerResult {
                        latencies_us: Vec::new(),
                        bytes: 0,
                        body_reads: 0,
                        errors: errors + measured_count,
                    };
                }
            }
        }
        Some(requests)
    } else {
        None
    };

    barrier.wait();

    let mut result = WorkerResult {
        latencies_us: Vec::with_capacity(measured_count),
        bytes: 0,
        body_reads: 0,
        errors,
    };
    if let Some(requests) = measured_requests {
        for request in requests {
            let started = Instant::now();
            match send_get_request(&client, request, &mut read_buffer) {
                Ok(stats) => {
                    result.latencies_us.push(started.elapsed().as_micros());
                    result.bytes += stats.bytes;
                    result.body_reads += stats.body_reads;
                }
                Err(_) => result.errors += 1,
            }
        }
    } else {
        for _ in 0..measured_count {
            let started = Instant::now();
            match request_once(&client, &config, &mut read_buffer) {
                Ok(stats) => {
                    result.latencies_us.push(started.elapsed().as_micros());
                    result.bytes += stats.bytes;
                    result.body_reads += stats.body_reads;
                }
                Err(_) => result.errors += 1,
            }
        }
    }
    result
}

fn request_once<C>(
    client: &Client<C>,
    config: &Config,
    read_buffer: &mut [u8],
) -> Result<ResponseStats, HttpClientError>
where
    C: Connector,
{
    if config.method == "POST" {
        let body = vec![b'x'; config.body_size];
        let request = Request::builder()
            .method("POST")
            .url(config.url.as_str())
            .body(TextBody::from_bytes(body.as_slice()))
            .map_err(HttpClientError::other)?;
        let mut response = client.request(request)?;
        drain_response(&mut response, read_buffer)
    } else {
        let request = Request::get(config.url.as_str())
            .body(EmptyBody)
            .map_err(HttpClientError::other)?;
        send_get_request(client, request, read_buffer)
    }
}

fn send_get_request<C>(
    client: &Client<C>,
    request: Request<EmptyBody>,
    read_buffer: &mut [u8],
) -> Result<ResponseStats, HttpClientError>
where
    C: Connector,
{
    let mut response = client.request(request)?;
    drain_response(&mut response, read_buffer)
}

fn drain_response(
    response: &mut Response<HttpBody>,
    read_buffer: &mut [u8],
) -> Result<ResponseStats, HttpClientError> {
    if !response.status().is_successful() {
        return Err(HttpClientError::other(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unexpected non-2xx response",
        )));
    }

    let mut bytes = 0;
    let mut body_reads = 0;
    loop {
        let size = response.body_mut().data(read_buffer)?;
        if size == 0 {
            break;
        }
        bytes += size as u64;
        body_reads += 1;
    }
    Ok(ResponseStats { bytes, body_reads })
}

struct ResponseStats {
    bytes: u64,
    body_reads: u64,
}

fn join_worker(handle: JoinHandle<WorkerResult>) -> WorkerResult {
    handle.join().unwrap_or(WorkerResult {
        latencies_us: Vec::new(),
        bytes: 0,
        body_reads: 0,
        errors: 1,
    })
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

fn average_read_size(bytes: u64, body_reads: u64) -> f64 {
    if body_reads == 0 {
        0.0
    } else {
        bytes as f64 / body_reads as f64
    }
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
        prebuilt_requests: false,
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
    "usage: sync_https_proxy_bench --url URL --proxy https://PROXY[:PORT] [--requests N] [--warmup-requests N] [--concurrency N] [--runtime-threads N] [--read-buffer-size N] [--method GET|POST] [--body-size N] [--proxy-ca-file PEM] [--proxy-client-cert PEM] [--proxy-client-key PEM] [--origin-ca-file PEM] [--insecure-proxy] [--insecure-origin] [--proxy-user-pass user:pass]"
}
