# HTTPS Proxy Benchmark

This directory contains the minimal benchmark harness for comparing
`ylong_http_client` with libcurl in HTTP/1.1 HTTPS proxy scenarios.

The harness intentionally excludes runtime patches, trace preload libraries, and
historical result reports. Use it to collect a clean baseline before doing any
performance optimization.

## Build Check

```bash
tools/https_proxy_bench/run_https_proxy_bench.sh --help
cargo check -p ylong_http_client \
  --features "async,http1_1,tokio_base,c_openssl_3_0" \
  --example async_https_proxy_bench
```

## Local Run

Generate local certificates:

```bash
tools/https_proxy_bench/generate_certs.sh
```

Start a local HTTP origin and HTTPS proxy:

```bash
tools/https_proxy_bench/local_https_proxy.py \
  --cert-file target/https_proxy_bench/certs/server.pem \
  --key-file target/https_proxy_bench/certs/server.key \
  --response-size 1024
```

The fixture prints JSON with `http_url` and `https_proxy`. Use those values with
the runner:

```bash
tools/https_proxy_bench/run_https_proxy_bench.sh \
  --url http://127.0.0.1:18080/ \
  --proxy https://localhost:18443 \
  --proxy-ca-file target/https_proxy_bench/certs/ca.pem \
  --requests 10000 \
  --warmup-requests 64 \
  --concurrency 64
```

For HTTPS origin over HTTPS proxy, start the fixture with `--origin-tls` and add
`--origin-ca-file target/https_proxy_bench/certs/ca.pem` to the runner.

## Output

The runner prints one JSON environment line, one JSON metrics line for
`ylong_http_client_async`, and one JSON metrics line for `libcurl` when
`curl-config` and `cc` are available. Both clients use the same URL, proxy,
TLS verification settings, request count, warmup count, concurrency, and read
buffer size.

Primary fields are:

- `rps`
- `latency_us_p50`
- `latency_us_p90`
- `latency_us_p95`
- `latency_us_p99`
- `completed`
- `errors`

For low-noise runs, pin the process externally with `taskset` or `numactl` and
collect CPU counters with:

```bash
PROFILE=perf-stat tools/https_proxy_bench/run_https_proxy_bench.sh ...
```
