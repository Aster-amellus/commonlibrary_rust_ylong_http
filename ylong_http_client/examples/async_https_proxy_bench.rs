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
use std::time::Instant;

use tokio::sync::Barrier;
use ylong_http_client::async_impl::{Body, Client, ClientBuilder, Request, Response};
use ylong_http_client::{HttpClientError, Proxy, TlsConfig, TlsFileType};

#[derive(Clone)]
struct Config {
    url: String,
    proxy: String,
    requests: usize,
    warmup_requests: usize,
    concurrency: usize,
    read_buffer_size: usize,
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
         [--read-buffer-size N] [--proxy-ca-file PEM] \
         [--proxy-client-cert PEM] [--proxy-client-key PEM] \
         [--origin-ca-file PEM] [--insecure-proxy] [--insecure-origin] \
         [--proxy-user-pass user:pass]"
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

    let mut config = Config {
        url: String::new(),
        proxy: String::new(),
        requests: 1000,
        warmup_requests: 0,
        concurrency: 16,
        read_buffer_size: 64 * 1024,
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
    if config.requests == 0 || config.concurrency == 0 || config.read_buffer_size == 0 {
        return Err("--requests, --concurrency, and --read-buffer-size must be > 0".to_string());
    }
    if config.proxy_client_cert.is_some() != config.proxy_client_key.is_some() {
        return Err("--proxy-client-cert and --proxy-client-key must be set together".to_string());
    }

    Ok(config)
}

fn parse_usize(value: String) -> Result<usize, String> {
    value
        .parse::<usize>()
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

async fn drain_body(
    mut response: Response,
    read_buffer_size: usize,
) -> Result<u64, HttpClientError> {
    let mut buf = vec![0u8; read_buffer_size];
    let mut bytes = 0u64;
    loop {
        let n = response.data(&mut buf).await?;
        if n == 0 {
            return Ok(bytes);
        }
        bytes += n as u64;
    }
}

async fn one_request(
    client: &Client,
    url: &str,
    read_buffer_size: usize,
) -> Result<u64, HttpClientError> {
    let request = Request::builder().url(url).body(Body::empty())?;
    let response = client.request(request).await?;
    drain_body(response, read_buffer_size).await
}

async fn worker(
    client: Arc<Client>,
    config: Config,
    count: usize,
    warmup_count: usize,
    ready: Arc<Barrier>,
    start: Arc<Barrier>,
) -> WorkerResult {
    let mut result = WorkerResult::default();

    for _ in 0..warmup_count {
        if one_request(&client, &config.url, config.read_buffer_size)
            .await
            .is_err()
        {
            result.errors += 1;
        }
    }

    ready.wait().await;
    start.wait().await;

    for _ in 0..count {
        let started = Instant::now();
        match one_request(&client, &config.url, config.read_buffer_size).await {
            Ok(bytes) => {
                result.bytes += bytes;
                result.latencies_us.push(started.elapsed().as_micros());
            }
            Err(_) => result.errors += 1,
        }
    }

    result
}

fn percentile(values: &[u128], pct: usize) -> u128 {
    if values.is_empty() {
        0
    } else {
        values[((values.len() - 1) * pct) / 100]
    }
}

fn json_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
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
    let client = match build_client(&config) {
        Ok(client) => Arc::new(client),
        Err(e) => {
            eprintln!("failed to build client: {e:?}");
            std::process::exit(2);
        }
    };

    let ready = Arc::new(Barrier::new(config.concurrency + 1));
    let start = Arc::new(Barrier::new(config.concurrency + 1));
    let mut handles = Vec::with_capacity(config.concurrency);
    for worker_id in 0..config.concurrency {
        let count = config.requests / config.concurrency
            + usize::from(worker_id < config.requests % config.concurrency);
        let warmup_count = config.warmup_requests / config.concurrency
            + usize::from(worker_id < config.warmup_requests % config.concurrency);
        handles.push(tokio::spawn(worker(
            Arc::clone(&client),
            config.clone(),
            count,
            warmup_count,
            Arc::clone(&ready),
            Arc::clone(&start),
        )));
    }

    ready.wait().await;
    let measured_start = Instant::now();
    start.wait().await;

    let mut latencies = Vec::with_capacity(config.requests);
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
         \"requests\":{},\"warmup_requests\":{},\"completed\":{},\"errors\":{},\
         \"concurrency\":{},\"read_buffer_size\":{},\"bytes\":{},\
         \"elapsed_ms\":{:.3},\"rps\":{:.3},\"latency_us_p50\":{},\
         \"latency_us_p90\":{},\"latency_us_p95\":{},\"latency_us_p99\":{}}}",
        json_escape(&config.url),
        json_escape(&config.proxy),
        config.requests,
        config.warmup_requests,
        completed,
        errors,
        config.concurrency,
        config.read_buffer_size,
        bytes,
        elapsed.as_secs_f64() * 1000.0,
        rps,
        percentile(&latencies, 50),
        percentile(&latencies, 90),
        percentile(&latencies, 95),
        percentile(&latencies, 99)
    );

    if errors > 0 {
        std::process::exit(1);
    }
}
