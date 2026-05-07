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

//! HTTPS proxy benchmark client for comparison with libcurl.

use std::env;
use std::sync::Arc;
use std::time::Instant;

use tokio::task::JoinHandle;

use ylong_http_client::async_impl::{Body, ClientBuilder, Request};
use ylong_http_client::{HttpClientError, Proxy, TlsConfig, TlsFileType};

#[cfg(feature = "__c_openssl")]
use openssl as _;

struct Config {
    url: String,
    proxy: String,
    requests: usize,
    concurrency: usize,
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
    errors: usize,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = match parse_args(env::args().skip(1).collect()) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("{}", usage());
            std::process::exit(2);
        }
    };

    let client = Arc::new(build_client(&config)?);
    let started = Instant::now();
    let mut handles = Vec::with_capacity(config.concurrency);

    for worker in 0..config.concurrency {
        let count = requests_for_worker(config.requests, config.concurrency, worker);
        let client = client.clone();
        let url = config.url.clone();
        handles.push(tokio::spawn(
            async move { run_worker(client, url, count).await },
        ));
    }

    let mut latencies = Vec::with_capacity(config.requests);
    let mut bytes = 0;
    let mut errors = 0;
    for handle in handles {
        let result = join_worker(handle).await;
        latencies.extend(result.latencies_us);
        bytes += result.bytes;
        errors += result.errors;
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
        "{{\"client\":\"ylong_http_client\",\"url\":\"{}\",\"proxy\":\"{}\",\"requests\":{},\"completed\":{},\"errors\":{},\"concurrency\":{},\"bytes\":{},\"elapsed_ms\":{:.3},\"rps\":{:.3},\"latency_us_p50\":{},\"latency_us_p95\":{},\"latency_us_p99\":{}}}",
        escape_json(&config.url),
        escape_json(&config.proxy),
        config.requests,
        completed,
        errors,
        config.concurrency,
        bytes,
        elapsed_ms,
        rps,
        percentile(&latencies, 50),
        percentile(&latencies, 95),
        percentile(&latencies, 99),
    );

    Ok(())
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

async fn run_worker(
    client: Arc<ylong_http_client::async_impl::Client>,
    url: String,
    count: usize,
) -> WorkerResult {
    let mut result = WorkerResult {
        latencies_us: Vec::with_capacity(count),
        bytes: 0,
        errors: 0,
    };

    for _ in 0..count {
        let started = Instant::now();
        match request_once(&client, &url).await {
            Ok(bytes) => {
                result.latencies_us.push(started.elapsed().as_micros());
                result.bytes += bytes;
            }
            Err(_) => result.errors += 1,
        }
    }

    result
}

async fn request_once(
    client: &ylong_http_client::async_impl::Client,
    url: &str,
) -> Result<u64, HttpClientError> {
    let request = Request::builder().url(url).body(Body::empty())?;
    let mut response = client.request(request).await?;
    if !response.status().is_successful() {
        return Err(HttpClientError::other(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unexpected non-2xx response",
        )));
    }

    let mut bytes = 0;
    let mut buf = [0; 16 * 1024];
    loop {
        let size = response.data(&mut buf).await?;
        if size == 0 {
            break;
        }
        bytes += size as u64;
    }
    Ok(bytes)
}

async fn join_worker(handle: JoinHandle<WorkerResult>) -> WorkerResult {
    match handle.await {
        Ok(result) => result,
        Err(_) => WorkerResult {
            latencies_us: Vec::new(),
            bytes: 0,
            errors: 1,
        },
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

fn parse_args(args: Vec<String>) -> Result<Config, String> {
    let mut config = Config {
        url: String::new(),
        proxy: String::new(),
        requests: 1000,
        concurrency: 16,
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
            "--concurrency" => {
                config.concurrency =
                    parse_usize(&next_value(&mut iter, "--concurrency")?, "--concurrency")?
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
    "usage: async_https_proxy_bench --url URL --proxy https://PROXY[:PORT] [--requests N] [--concurrency N] [--proxy-ca-file PEM] [--proxy-client-cert PEM] [--proxy-client-key PEM] [--origin-ca-file PEM] [--insecure-proxy] [--insecure-origin] [--proxy-user-pass user:pass]"
}
