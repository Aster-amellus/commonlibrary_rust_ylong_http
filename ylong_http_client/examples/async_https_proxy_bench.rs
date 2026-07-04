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

//! Minimal async HTTPS proxy benchmark client for comparison with libcurl.

use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Barrier;
use ylong_http_client::async_impl::{Body, Client, ClientBuilder, Request, Response};
use ylong_http_client::{HttpClientError, Proxy, TlsConfig, TlsFileType};

#[derive(Clone, Copy, Debug)]
enum ClientMode {
    Shared,
    PerWorker,
}

impl ClientMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::PerWorker => "per-worker",
        }
    }
}

#[derive(Clone, Debug)]
struct Config {
    url: String,
    proxy: String,
    requests: usize,
    warmup_requests: usize,
    duration_seconds: Option<u64>,
    concurrency: usize,
    read_buffer_size: usize,
    client_mode: ClientMode,
    max_h1_conn_number: Option<usize>,
    proxy_ca_file: Option<String>,
    proxy_client_cert: Option<String>,
    proxy_client_key: Option<String>,
    origin_ca_file: Option<String>,
    insecure_proxy: bool,
    insecure_origin: bool,
    proxy_user_pass: Option<String>,
}

#[derive(Default)]
struct WorkerResult {
    latencies_us: Vec<u128>,
    bytes: u64,
    errors: usize,
}

fn usage(program: &str) {
    eprintln!(
        "usage: {program} --url URL --proxy https://HOST:PORT \
         [--requests N] [--warmup-requests N] [--concurrency N] \
         [--duration-seconds N] \
         [--read-buffer-size N] [--proxy-ca-file PEM] \
         [--proxy-client-cert PEM] [--proxy-client-key PEM] \
         [--origin-ca-file PEM] [--insecure-proxy] [--insecure-origin] \
         [--proxy-user-pass user:pass] [--client-mode shared|per-worker] \
         [--max-h1-conn-number N]"
    );
}

fn next_value(args: &[String], index: &mut usize, name: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{name} requires a value"))
}

fn parse_args() -> Result<Config, String> {
    let args = env::args().collect::<Vec<_>>();
    if args.len() == 1 || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        usage(&args[0]);
        std::process::exit(0);
    }

    parse_args_from(args)
}

fn parse_args_from(args: Vec<String>) -> Result<Config, String> {
    let mut config = Config {
        url: String::new(),
        proxy: String::new(),
        requests: 1000,
        warmup_requests: 0,
        duration_seconds: None,
        concurrency: 16,
        read_buffer_size: 64 * 1024,
        client_mode: ClientMode::Shared,
        max_h1_conn_number: None,
        proxy_ca_file: None,
        proxy_client_cert: None,
        proxy_client_key: None,
        origin_ca_file: None,
        insecure_proxy: false,
        insecure_origin: false,
        proxy_user_pass: None,
    };

    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--url" => config.url = next_value(&args, &mut index, "--url")?,
            "--proxy" => config.proxy = next_value(&args, &mut index, "--proxy")?,
            "--requests" => {
                config.requests = parse_usize(next_value(&args, &mut index, "--requests")?)?
            }
            "--warmup-requests" => {
                config.warmup_requests =
                    parse_usize(next_value(&args, &mut index, "--warmup-requests")?)?
            }
            "--duration-seconds" => {
                let value = parse_u64(next_value(&args, &mut index, "--duration-seconds")?)?;
                if value == 0 {
                    return Err("--duration-seconds must be > 0".to_string());
                }
                config.duration_seconds = Some(value);
            }
            "--concurrency" => {
                config.concurrency = parse_usize(next_value(&args, &mut index, "--concurrency")?)?
            }
            "--read-buffer-size" => {
                config.read_buffer_size =
                    parse_usize(next_value(&args, &mut index, "--read-buffer-size")?)?
            }
            "--proxy-ca-file" => {
                config.proxy_ca_file = Some(next_value(&args, &mut index, "--proxy-ca-file")?)
            }
            "--proxy-client-cert" => {
                config.proxy_client_cert =
                    Some(next_value(&args, &mut index, "--proxy-client-cert")?)
            }
            "--proxy-client-key" => {
                config.proxy_client_key = Some(next_value(&args, &mut index, "--proxy-client-key")?)
            }
            "--origin-ca-file" => {
                config.origin_ca_file = Some(next_value(&args, &mut index, "--origin-ca-file")?)
            }
            "--proxy-user-pass" => {
                config.proxy_user_pass = Some(next_value(&args, &mut index, "--proxy-user-pass")?)
            }
            "--client-mode" => {
                config.client_mode =
                    parse_client_mode(next_value(&args, &mut index, "--client-mode")?)?
            }
            "--max-h1-conn-number" => {
                config.max_h1_conn_number = Some(parse_usize(next_value(
                    &args,
                    &mut index,
                    "--max-h1-conn-number",
                )?)?)
            }
            "--insecure-proxy" => config.insecure_proxy = true,
            "--insecure-origin" => config.insecure_origin = true,
            unknown => return Err(format!("unknown argument: {unknown}")),
        }
        index += 1;
    }

    if config.url.is_empty() {
        return Err("--url is required".to_string());
    }
    if config.proxy.is_empty() {
        return Err("--proxy is required".to_string());
    }
    if !config.proxy.starts_with("https://") && !config.proxy.starts_with("http://") {
        return Err("--proxy must start with http:// or https://".to_string());
    }
    if config.requests == 0 && config.duration_seconds.is_none() {
        return Err("--requests must be > 0 unless --duration-seconds is set".to_string());
    }
    if config.concurrency == 0 || config.read_buffer_size == 0 {
        return Err("--concurrency and --read-buffer-size must be > 0".to_string());
    }
    if config.max_h1_conn_number == Some(0) {
        return Err("--max-h1-conn-number must be > 0".to_string());
    }
    if config.proxy_client_cert.is_some() != config.proxy_client_key.is_some() {
        return Err("--proxy-client-cert and --proxy-client-key must be set together".to_string());
    }

    Ok(config)
}

fn parse_client_mode(value: String) -> Result<ClientMode, String> {
    match value.as_str() {
        "shared" => Ok(ClientMode::Shared),
        "per-worker" => Ok(ClientMode::PerWorker),
        _ => Err(format!("invalid --client-mode: {value}")),
    }
}

fn parse_usize(value: String) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("invalid integer: {value}"))
}

fn parse_u64(value: String) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("invalid integer: {value}"))
}

fn proxy_tls_config(config: &Config) -> Result<TlsConfig, HttpClientError> {
    let mut builder = TlsConfig::builder();
    if let Some(path) = &config.proxy_ca_file {
        builder = builder.ca_file(path);
    }
    if let Some(path) = &config.proxy_client_cert {
        builder = builder.certificate_chain_file(path);
    }
    if let Some(path) = &config.proxy_client_key {
        builder = builder.private_key_file(path, TlsFileType::PEM);
    }
    if config.insecure_proxy {
        builder = builder
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true);
    }
    builder.build()
}

fn build_client(config: &Config) -> Result<Client, HttpClientError> {
    let mut proxy_builder =
        Proxy::all(config.proxy.as_str()).proxy_tls_config(proxy_tls_config(config)?);
    if let Some(user_pass) = &config.proxy_user_pass {
        let (user, pass) = user_pass
            .split_once(':')
            .ok_or_else(|| HttpClientError::other("proxy credentials must be user:pass"))?;
        proxy_builder = proxy_builder.basic_auth(user, pass);
    }

    let mut builder = ClientBuilder::new().proxy(proxy_builder.build()?);
    if let Some(max_h1_conn_number) = config.max_h1_conn_number {
        builder = builder.max_h1_conn_number(max_h1_conn_number);
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

async fn drain_body(mut response: Response, buf: &mut [u8]) -> Result<u64, HttpClientError> {
    let mut bytes = 0u64;
    loop {
        let n = response.data(&mut *buf).await?;
        if n == 0 {
            return Ok(bytes);
        }
        bytes += n as u64;
    }
}

async fn one_request(client: &Client, url: &str, buf: &mut [u8]) -> Result<u64, HttpClientError> {
    let request = Request::builder().url(url).body(Body::empty())?;
    let response = client.request(request).await?;
    drain_body(response, buf).await
}

async fn worker(
    client: Arc<Client>,
    config: Config,
    count: usize,
    warmup_count: usize,
    duration: Option<Duration>,
    ready: Arc<Barrier>,
    start: Arc<Barrier>,
) -> WorkerResult {
    let mut result = WorkerResult::default();
    let mut read_buf = vec![0u8; config.read_buffer_size];

    for _ in 0..warmup_count {
        if one_request(&client, &config.url, &mut read_buf)
            .await
            .is_err()
        {
            result.errors += 1;
        }
    }

    ready.wait().await;
    start.wait().await;

    if let Some(duration) = duration {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            let started = Instant::now();
            match one_request(&client, &config.url, &mut read_buf).await {
                Ok(bytes) => {
                    result.bytes += bytes;
                    result.latencies_us.push(started.elapsed().as_micros());
                }
                Err(_) => result.errors += 1,
            }
        }
        return result;
    }

    for _ in 0..count {
        let started = Instant::now();
        match one_request(&client, &config.url, &mut read_buf).await {
            Ok(bytes) => {
                result.bytes += bytes;
                result.latencies_us.push(started.elapsed().as_micros());
            }
            Err(_) => result.errors += 1,
        }
    }

    result
}

async fn worker_with_owned_client(
    config: Config,
    count: usize,
    warmup_count: usize,
    duration: Option<Duration>,
    ready: Arc<Barrier>,
    start: Arc<Barrier>,
) -> WorkerResult {
    let client = match build_client(&config) {
        Ok(client) => Arc::new(client),
        Err(_) => {
            ready.wait().await;
            start.wait().await;
            return WorkerResult {
                errors: warmup_count + if duration.is_some() { 1 } else { count },
                ..WorkerResult::default()
            };
        }
    };

    worker(client, config, count, warmup_count, duration, ready, start).await
}

fn percentile(values: &[u128], pct: usize) -> u128 {
    if values.is_empty() {
        0
    } else {
        values[((values.len() - 1) * pct) / 100]
    }
}

fn percentile_permille(values: &[u128], permille: usize) -> u128 {
    if values.is_empty() {
        0
    } else {
        values[((values.len() - 1) * permille) / 1000]
    }
}

fn json_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn option_usize_json(value: Option<usize>) -> String {
    value.map_or_else(|| "null".to_string(), |value| value.to_string())
}

fn option_u64_json(value: Option<u64>) -> String {
    value.map_or_else(|| "null".to_string(), |value| value.to_string())
}

#[tokio::main]
async fn main() {
    let config = match parse_args() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let shared_client = if matches!(config.client_mode, ClientMode::Shared) {
        match build_client(&config) {
            Ok(client) => Some(Arc::new(client)),
            Err(e) => {
                eprintln!("failed to build client: {e:?}");
                std::process::exit(2);
            }
        }
    } else {
        None
    };

    let ready = Arc::new(Barrier::new(config.concurrency + 1));
    let start = Arc::new(Barrier::new(config.concurrency + 1));
    let duration = config.duration_seconds.map(Duration::from_secs);
    let mut handles = Vec::with_capacity(config.concurrency);
    for worker_id in 0..config.concurrency {
        let count = config.requests / config.concurrency
            + usize::from(worker_id < config.requests % config.concurrency);
        let warmup_count = config.warmup_requests / config.concurrency
            + usize::from(worker_id < config.warmup_requests % config.concurrency);
        match config.client_mode {
            ClientMode::Shared => {
                let Some(client) = shared_client.as_ref() else {
                    eprintln!("shared client mode requires a prebuilt client");
                    std::process::exit(2);
                };
                handles.push(tokio::spawn(worker(
                    Arc::clone(client),
                    config.clone(),
                    count,
                    warmup_count,
                    duration,
                    Arc::clone(&ready),
                    Arc::clone(&start),
                )));
            }
            ClientMode::PerWorker => handles.push(tokio::spawn(worker_with_owned_client(
                config.clone(),
                count,
                warmup_count,
                duration,
                Arc::clone(&ready),
                Arc::clone(&start),
            ))),
        }
    }

    ready.wait().await;
    #[cfg(feature = "__bench_phase_metrics")]
    ylong_http_client::reset_bench_phase_metrics();
    let measured_start = Instant::now();
    start.wait().await;

    let latency_capacity = if config.duration_seconds.is_some() {
        config.concurrency.saturating_mul(1024)
    } else {
        config.requests
    };
    let mut latencies = Vec::with_capacity(latency_capacity);
    let mut bytes = 0u64;
    let mut errors = 0usize;
    for handle in handles {
        match handle.await {
            Ok(mut result) => {
                bytes += result.bytes;
                errors += result.errors;
                latencies.append(&mut result.latencies_us);
            }
            Err(_) => errors += 1,
        }
    }
    let elapsed = measured_start.elapsed();
    latencies.sort_unstable();

    let completed = latencies.len();
    let elapsed_us = elapsed.as_micros();
    let rps = if elapsed_us == 0 {
        0.0
    } else {
        completed as f64 * 1_000_000.0 / elapsed_us as f64
    };

    println!(
        "{{\"client\":\"ylong_http_client_async\",\"url\":\"{}\",\"proxy\":\"{}\",\
         \"requests\":{},\"warmup_requests\":{},\"duration_seconds\":{},\"completed\":{},\"errors\":{},\
         \"concurrency\":{},\"read_buffer_size\":{},\"client_mode\":\"{}\",\
         \"max_h1_conn_number\":{},\"bytes\":{},\
         \"elapsed_ms\":{:.3},\"rps\":{:.3},\"latency_us_p50\":{},\
         \"latency_us_p90\":{},\"latency_us_p95\":{},\"latency_us_p99\":{},\
         \"latency_us_p999\":{}}}",
        json_escape(&config.url),
        json_escape(&config.proxy),
        config.requests,
        config.warmup_requests,
        option_u64_json(config.duration_seconds),
        completed,
        errors,
        config.concurrency,
        config.read_buffer_size,
        config.client_mode.as_str(),
        option_usize_json(config.max_h1_conn_number),
        bytes,
        elapsed.as_secs_f64() * 1000.0,
        rps,
        percentile(&latencies, 50),
        percentile(&latencies, 90),
        percentile(&latencies, 95),
        percentile(&latencies, 99),
        percentile_permille(&latencies, 999)
    );

    #[cfg(feature = "__bench_phase_metrics")]
    println!("{}", ylong_http_client::snapshot_bench_phase_metrics_json());

    if errors > 0 {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::parse_args_from;

    fn required_args() -> Vec<String> {
        vec![
            "bench".to_string(),
            "--url".to_string(),
            "https://127.0.0.1:18080/".to_string(),
            "--proxy".to_string(),
            "https://127.0.0.1:18443".to_string(),
        ]
    }

    #[test]
    fn parses_duration_seconds() {
        let mut args = required_args();
        args.push("--duration-seconds".to_string());
        args.push("60".to_string());

        let config = parse_args_from(args).unwrap();

        assert_eq!(config.duration_seconds, Some(60));
    }

    #[test]
    fn rejects_zero_duration_seconds() {
        let mut args = required_args();
        args.push("--duration-seconds".to_string());
        args.push("0".to_string());

        let err = parse_args_from(args).unwrap_err();

        assert_eq!(err, "--duration-seconds must be > 0");
    }
}
