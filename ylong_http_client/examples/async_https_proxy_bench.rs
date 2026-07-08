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
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Barrier;
use ylong_http_client::async_impl::{Body, Client, ClientBuilder, Request, Response};
use ylong_http_client::{HttpClientError, Proxy, TlsConfig, TlsFileType, Uri};

const MAX_ERROR_SAMPLES: usize = 8;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HttpVersionMode {
    Http1,
    #[cfg(feature = "http2")]
    Http2,
    Negotiate,
}

impl HttpVersionMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Http1 => "h1",
            #[cfg(feature = "http2")]
            Self::Http2 => "h2",
            Self::Negotiate => "negotiate",
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
    http_version: HttpVersionMode,
    max_h1_conn_number: Option<usize>,
    max_h2_conn_number: Option<usize>,
    allowed_cache_frame_size: Option<usize>,
    tls13_ciphers: Option<String>,
    tls_groups: Option<String>,
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
    error_samples: Vec<String>,
}

impl WorkerResult {
    fn with_capacity(latency_capacity: usize) -> Self {
        Self {
            // PERF: The benchmark should measure the client, not repeated
            // latency-sample reallocations in the harness. Fixed-request runs
            // know the per-worker upper bound, and duration runs match the
            // libcurl harness by starting with 1024 slots per worker.
            latencies_us: Vec::with_capacity(latency_capacity),
            bytes: 0,
            errors: 0,
            error_samples: Vec::with_capacity(MAX_ERROR_SAMPLES),
        }
    }
}

#[derive(Clone)]
struct RequestTemplate {
    url: String,
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
         [--http-version h1|h2|negotiate] [--max-h1-conn-number N] \
         [--max-h2-conn-number N] [--allowed-cache-frame-size N] \
         [--tls13-ciphers LIST] [--tls-groups LIST]"
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
        http_version: HttpVersionMode::Http1,
        max_h1_conn_number: None,
        max_h2_conn_number: None,
        allowed_cache_frame_size: None,
        tls13_ciphers: None,
        tls_groups: None,
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
            "--http-version" => {
                config.http_version =
                    parse_http_version_mode(next_value(&args, &mut index, "--http-version")?)?
            }
            "--max-h1-conn-number" => {
                config.max_h1_conn_number = Some(parse_usize(next_value(
                    &args,
                    &mut index,
                    "--max-h1-conn-number",
                )?)?)
            }
            "--max-h2-conn-number" => {
                config.max_h2_conn_number = Some(parse_usize(next_value(
                    &args,
                    &mut index,
                    "--max-h2-conn-number",
                )?)?)
            }
            "--allowed-cache-frame-size" => {
                config.allowed_cache_frame_size = Some(parse_usize(next_value(
                    &args,
                    &mut index,
                    "--allowed-cache-frame-size",
                )?)?)
            }
            "--tls13-ciphers" => {
                config.tls13_ciphers = Some(next_value(&args, &mut index, "--tls13-ciphers")?)
            }
            "--tls-groups" => {
                config.tls_groups = Some(next_value(&args, &mut index, "--tls-groups")?)
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
    if config.max_h2_conn_number == Some(0) {
        return Err("--max-h2-conn-number must be > 0".to_string());
    }
    if config.allowed_cache_frame_size == Some(0) {
        return Err("--allowed-cache-frame-size must be > 0".to_string());
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

fn parse_http_version_mode(value: String) -> Result<HttpVersionMode, String> {
    match value.as_str() {
        "h1" | "http1" | "http/1.1" => Ok(HttpVersionMode::Http1),
        "h2" | "http2" | "http/2" => {
            #[cfg(feature = "http2")]
            {
                Ok(HttpVersionMode::Http2)
            }
            #[cfg(not(feature = "http2"))]
            {
                Err("--http-version h2 requires the http2 feature".to_string())
            }
        }
        "negotiate" | "alpn" => Ok(HttpVersionMode::Negotiate),
        _ => Err(format!("invalid --http-version: {value}")),
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
    if let Some(list) = &config.tls13_ciphers {
        builder = builder.cipher_suite(list);
    }
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
    if config.tls_groups.is_some() {
        return Err(HttpClientError::other(
            "--tls-groups is not supported by ylong_http_client",
        ));
    }

    let mut proxy_builder =
        Proxy::all(config.proxy.as_str()).proxy_tls_config(proxy_tls_config(config)?);
    if let Some(user_pass) = &config.proxy_user_pass {
        let (user, pass) = user_pass
            .split_once(':')
            .ok_or_else(|| HttpClientError::other("proxy credentials must be user:pass"))?;
        proxy_builder = proxy_builder.basic_auth(user, pass);
    }

    let mut builder = ClientBuilder::new().proxy(proxy_builder.build()?);
    builder = match config.http_version {
        HttpVersionMode::Http1 => builder.http1_only(),
        #[cfg(feature = "http2")]
        HttpVersionMode::Http2 => builder.http2_prior_knowledge(),
        HttpVersionMode::Negotiate => builder,
    };
    if let Some(max_h1_conn_number) = config.max_h1_conn_number {
        builder = builder.max_h1_conn_number(max_h1_conn_number);
    }
    #[cfg(feature = "http2")]
    if let Some(max_h2_conn_number) = config.max_h2_conn_number {
        builder = builder.max_h2_conn_number(max_h2_conn_number);
    }
    #[cfg(feature = "http2")]
    if let Some(allowed_cache_frame_size) = config.allowed_cache_frame_size {
        builder = builder.allowed_cache_frame_size(allowed_cache_frame_size);
    }
    if let Some(list) = &config.tls13_ciphers {
        builder = builder.tls_cipher_suite(list);
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

fn build_request_template(config: &Config) -> Result<RequestTemplate, HttpClientError> {
    let uri = Uri::try_from(config.url.as_str()).map_err(|err| {
        HttpClientError::other(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid URL: {err}"),
        ))
    })?;
    Ok(RequestTemplate {
        url: uri.to_string(),
    })
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

async fn one_request(
    client: &Client,
    template: &RequestTemplate,
    buf: &mut [u8],
) -> Result<u64, HttpClientError> {
    let request = Request::builder()
        .url(template.url.as_str())
        .body(Body::empty())?;
    let response = client.request(request).await?;
    drain_body(response, buf).await
}

async fn worker(
    client: Arc<Client>,
    config: Config,
    template: RequestTemplate,
    count: usize,
    warmup_count: usize,
    duration: Option<Duration>,
    ready: Arc<Barrier>,
    start: Arc<Barrier>,
) -> WorkerResult {
    let latency_capacity = if duration.is_some() { 1024 } else { count };
    let mut result = WorkerResult::with_capacity(latency_capacity);
    let mut read_buf = vec![0u8; config.read_buffer_size];

    for _ in 0..warmup_count {
        if let Err(err) = one_request(&client, &template, &mut read_buf).await {
            record_error(&mut result, format!("{err:?}"));
        }
    }

    ready.wait().await;
    start.wait().await;

    if let Some(duration) = duration {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            let started = Instant::now();
            match one_request(&client, &template, &mut read_buf).await {
                Ok(bytes) => {
                    result.bytes += bytes;
                    result.latencies_us.push(started.elapsed().as_micros());
                }
                Err(err) => record_error(&mut result, format!("{err:?}")),
            }
        }
        return result;
    }

    for _ in 0..count {
        let started = Instant::now();
        match one_request(&client, &template, &mut read_buf).await {
            Ok(bytes) => {
                result.bytes += bytes;
                result.latencies_us.push(started.elapsed().as_micros());
            }
            Err(err) => record_error(&mut result, format!("{err:?}")),
        }
    }

    result
}

async fn worker_with_owned_client(
    config: Config,
    template: RequestTemplate,
    count: usize,
    warmup_count: usize,
    duration: Option<Duration>,
    ready: Arc<Barrier>,
    start: Arc<Barrier>,
) -> WorkerResult {
    let client = match build_client(&config) {
        Ok(client) => Arc::new(client),
        Err(err) => {
            ready.wait().await;
            start.wait().await;
            let total_errors = warmup_count + if duration.is_some() { 1 } else { count };
            let mut result = WorkerResult {
                errors: total_errors,
                ..WorkerResult::default()
            };
            if total_errors != 0 {
                result
                    .error_samples
                    .push(format!("failed to build client: {err:?}"));
            }
            return result;
        }
    };

    worker(
        client,
        config,
        template,
        count,
        warmup_count,
        duration,
        ready,
        start,
    )
    .await
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

fn record_error(result: &mut WorkerResult, error: String) {
    result.errors += 1;
    if result.error_samples.len() < MAX_ERROR_SAMPLES {
        result.error_samples.push(error);
    }
}

fn json_string_array(values: &[String]) -> String {
    let mut json = String::from("[");
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            json.push(',');
        }
        json.push('"');
        json.push_str(&json_escape(value));
        json.push('"');
    }
    json.push(']');
    json
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
    let request_template = match build_request_template(&config) {
        Ok(template) => template,
        Err(e) => {
            eprintln!("failed to build request template: {e:?}");
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
                    request_template.clone(),
                    count,
                    warmup_count,
                    duration,
                    Arc::clone(&ready),
                    Arc::clone(&start),
                )));
            }
            ClientMode::PerWorker => handles.push(tokio::spawn(worker_with_owned_client(
                config.clone(),
                request_template.clone(),
                count,
                warmup_count,
                duration,
                Arc::clone(&ready),
                Arc::clone(&start),
            ))),
        }
    }

    ready.wait().await;
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
    let mut error_samples = Vec::new();
    for handle in handles {
        match handle.await {
            Ok(mut result) => {
                bytes += result.bytes;
                errors += result.errors;
                latencies.append(&mut result.latencies_us);
                for sample in result.error_samples {
                    if error_samples.len() < MAX_ERROR_SAMPLES {
                        error_samples.push(sample);
                    }
                }
            }
            Err(err) => {
                errors += 1;
                if error_samples.len() < MAX_ERROR_SAMPLES {
                    error_samples.push(format!("worker task failed: {err:?}"));
                }
            }
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
         \"requests\":{},\"warmup_requests\":{},\"duration_seconds\":{},\"completed\":{},\"errors\":{},\"error_samples\":{},\
         \"concurrency\":{},\"read_buffer_size\":{},\"client_mode\":\"{}\",\"http_version\":\"{}\",\
         \"max_h1_conn_number\":{},\"max_h2_conn_number\":{},\"allowed_cache_frame_size\":{},\"bytes\":{},\
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
        json_string_array(&error_samples),
        config.concurrency,
        config.read_buffer_size,
        config.client_mode.as_str(),
        config.http_version.as_str(),
        option_usize_json(config.max_h1_conn_number),
        option_usize_json(config.max_h2_conn_number),
        option_usize_json(config.allowed_cache_frame_size),
        bytes,
        elapsed.as_secs_f64() * 1000.0,
        rps,
        percentile(&latencies, 50),
        percentile(&latencies, 90),
        percentile(&latencies, 95),
        percentile(&latencies, 99),
        percentile_permille(&latencies, 999)
    );

    if errors > 0 {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_client, parse_args_from, record_error, HttpVersionMode, WorkerResult,
        MAX_ERROR_SAMPLES,
    };

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

    #[test]
    fn parses_tls13_ciphers() {
        let mut args = required_args();
        args.push("--tls13-ciphers".to_string());
        args.push("TLS_AES_128_GCM_SHA256".to_string());

        let config = parse_args_from(args).unwrap();

        assert_eq!(
            config.tls13_ciphers.as_deref(),
            Some("TLS_AES_128_GCM_SHA256")
        );
    }

    #[test]
    fn parses_tls_groups() {
        let mut args = required_args();
        args.push("--tls-groups".to_string());
        args.push("X25519".to_string());

        let config = parse_args_from(args).unwrap();

        assert_eq!(config.tls_groups.as_deref(), Some("X25519"));
    }

    #[test]
    fn rejects_tls_groups_when_building_ylong_client() {
        let mut config = parse_args_from(required_args()).unwrap();
        config.tls_groups = Some("X25519".to_string());

        let err = match build_client(&config) {
            Ok(_) => panic!("build_client unexpectedly accepted --tls-groups"),
            Err(err) => err,
        };

        assert!(err
            .to_string()
            .contains("--tls-groups is not supported by ylong_http_client"));
    }

    #[test]
    fn parses_http1_version_mode() {
        let mut args = required_args();
        args.push("--http-version".to_string());
        args.push("h1".to_string());

        let config = parse_args_from(args).unwrap();

        assert_eq!(config.http_version, HttpVersionMode::Http1);
    }

    #[cfg(feature = "http2")]
    #[test]
    fn parses_http2_version_mode() {
        let mut args = required_args();
        args.push("--http-version".to_string());
        args.push("h2".to_string());

        let config = parse_args_from(args).unwrap();

        assert_eq!(config.http_version, HttpVersionMode::Http2);
    }

    #[test]
    fn rejects_unknown_http_version_mode() {
        let mut args = required_args();
        args.push("--http-version".to_string());
        args.push("spdy".to_string());

        let err = parse_args_from(args).unwrap_err();

        assert_eq!(err, "invalid --http-version: spdy");
    }

    #[test]
    fn records_error_samples_with_cap() {
        let mut result = WorkerResult::default();

        for index in 0..(MAX_ERROR_SAMPLES + 2) {
            record_error(&mut result, format!("err-{index}"));
        }

        assert_eq!(result.errors, MAX_ERROR_SAMPLES + 2);
        assert_eq!(result.error_samples.len(), MAX_ERROR_SAMPLES);
        assert_eq!(result.error_samples[0], "err-0");
        assert_eq!(
            result.error_samples[MAX_ERROR_SAMPLES - 1],
            format!("err-{}", MAX_ERROR_SAMPLES - 1)
        );
    }

    #[test]
    fn parses_allowed_cache_frame_size() {
        let mut args = required_args();
        args.push("--allowed-cache-frame-size".to_string());
        args.push("64".to_string());

        let config = parse_args_from(args).unwrap();

        assert_eq!(config.allowed_cache_frame_size, Some(64));
    }

    #[test]
    fn parses_max_h2_conn_number() {
        let mut args = required_args();
        args.push("--max-h2-conn-number".to_string());
        args.push("4".to_string());

        let config = parse_args_from(args).unwrap();

        assert_eq!(config.max_h2_conn_number, Some(4));
    }
}
