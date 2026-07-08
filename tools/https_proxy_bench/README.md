# HTTPS Proxy Benchmark

This directory contains the benchmark harness used to compare
`ylong_http_client` with libcurl in HTTPS proxy scenarios.

The benchmark path is:

```text
client -> TLS -> HTTPS proxy -> CONNECT tunnel -> TLS -> origin
```

The fixture is written in Rust. Python fixtures and internal instrumentation
features are intentionally excluded from this deliverable so that benchmark
results measure the proxy data path instead of auxiliary tooling.

## Tools

- `fixture-rs/`: Rust origin and HTTPS proxy fixture.
- `run_https_proxy_fixture_rs.sh`: builds and starts the fixture.
- `run_https_proxy_bench.sh`: builds and runs ylong and/or libcurl clients.
- `run_https_proxy_scenario.sh`: expands a governed scenario and optionally
  records JSONL results.
- `scenarios/`: correctness, hot-path, enterprise-pressure, and degradation
  scenario definitions.
- `libcurl_harness.c`: libcurl comparator used by the common runner.
- `generate_certs.sh`: local CA, server certificate, and client certificate
  generator for proxy TLS and mTLS tests.

Generated files under `target/`, `perf.data`, and fixture build outputs are not
source artifacts and must not be committed.

## Build Check

```bash
export OPENSSL_LIB_DIR=$(pkg-config --variable=libdir openssl)
export OPENSSL_INCLUDE_DIR=$(pkg-config --variable=includedir openssl)

tools/https_proxy_bench/run_https_proxy_bench.sh --help

cargo check -p ylong_http_client \
  --features "async,http1_1,tokio_base,c_openssl_3_0" \
  --example async_https_proxy_bench
```

## Local HTTPS-over-HTTPS Run

Generate local certificates:

```bash
tools/https_proxy_bench/generate_certs.sh
```

Start the Rust fixture in one terminal:

```bash
tools/https_proxy_bench/run_https_proxy_fixture_rs.sh \
  --cert-file target/https_proxy_bench/certs/server.pem \
  --key-file target/https_proxy_bench/certs/server.key \
  --origin-tls \
  --response-size 1024
```

Run the comparator in another terminal:

```bash
tools/https_proxy_bench/run_https_proxy_bench.sh \
  --url https://127.0.0.1:18080/ \
  --proxy https://127.0.0.1:18443 \
  --proxy-ca-file target/https_proxy_bench/certs/ca.pem \
  --origin-ca-file target/https_proxy_bench/certs/ca.pem \
  --requests 10000 \
  --warmup-requests 512 \
  --concurrency 64
```

For HTTP origin over HTTPS proxy, omit `--origin-tls` when starting the fixture
and omit `--origin-ca-file` from the runner.

## Scenario Runner

Dry-run a scenario to print the fixture command and the exact benchmark command:

```bash
tools/https_proxy_bench/run_https_proxy_scenario.sh \
  tools/https_proxy_bench/scenarios/enterprise_mixed_pressure.env
```

After starting the printed fixture command, run the scenario:

```bash
RUN_BENCH=1 tools/https_proxy_bench/run_https_proxy_scenario.sh \
  tools/https_proxy_bench/scenarios/enterprise_mixed_pressure.env
```

Results are written to
`target/https_proxy_bench/results/<scenario>_<timestamp>.jsonl`. Build and
profiler stderr are written to the adjacent `.perf` sidecar file.

For lower-noise repeated runs, pin the fixture and measured clients to separate
CPU sets:

```bash
FIXTURE_CPU_LIST=0-3 BENCH_CPU_LIST=4-15 \
  tools/https_proxy_bench/run_https_proxy_scenario.sh \
  tools/https_proxy_bench/scenarios/enterprise_mixed_pressure.env
```

`FIXTURE_CPU_LIST` is printed into the fixture command. `BENCH_CPU_LIST` is
forwarded to the low-level runner and pins each measured client with
`taskset -c`.

## Correctness Evidence

Correctness evidence is separate from performance evidence. Use the scenario
dry runs to confirm fixture configuration and cargo tests to prove client
behavior:

```bash
tools/https_proxy_bench/run_https_proxy_scenario.sh \
  tools/https_proxy_bench/scenarios/correctness_http_target_over_https_proxy.env

tools/https_proxy_bench/run_https_proxy_scenario.sh \
  tools/https_proxy_bench/scenarios/correctness_https_proxy.env

tools/https_proxy_bench/run_https_proxy_scenario.sh \
  tools/https_proxy_bench/scenarios/correctness_https_proxy_mtls.env
```

Run the client tests before using benchmark results:

```bash
export OPENSSL_LIB_DIR=$(pkg-config --variable=libdir openssl)
export OPENSSL_INCLUDE_DIR=$(pkg-config --variable=includedir openssl)

cargo test -p ylong_http_client --test sdv_async_https_proxy \
  --features "async http1_1 ylong_base c_openssl_3_0" --release

cargo test -p ylong_http_client --test sdv_async_http_proxy \
  --features "async http1_1 ylong_base" --release

cargo test -p ylong_http_client --lib ut_proxy_route \
  --features "async http1_1 tokio_base" --release

cargo test -p ylong_http_client --lib ut_proxy_route \
  --features "async http1_1 tokio_base c_openssl_3_0" --release
```

The HTTPS proxy tests cover HTTPS proxy request forwarding, CONNECT tunneling,
proxy TLS verification, proxy mTLS, proxy authentication, TLS failure context,
and the explicit `no_tls + https proxy` error.

## Scenario Layers

- `correctness_*.env`: small ylong/libcurl smoke scenarios for HTTP origin,
  HTTPS origin, and proxy mTLS.
- `hot_path_steady_1k.env`: steady-state HTTPS-over-HTTPS run with small
  payloads. Use it to check whether a local code path is hot enough to justify
  optimization.
- `enterprise_mixed_pressure.env`: primary ylong-vs-libcurl comparison. It uses
  HTTPS origin over HTTPS proxy, mixed response sizes, high concurrency, warmup,
  perf counters when available, and repeated runs.
- `degradation_churn.env`: stress scenario with connection churn and high
  concurrency. Use it to find tail-latency cliffs; do not use it as the primary
  headline comparator.

## Result Protocol

A performance claim must record:

- repository commit and local diff status;
- `rustc --version`, `cargo --version`, libcurl version, OpenSSL version, OS,
  kernel, CPU model, and CPU governor;
- exact scenario file, command line, feature flags, and environment variables;
- request count, warmup count, concurrency, response-size distribution, TLS
  verification mode, and client connection limits;
- at least five repeated runs for the primary enterprise scenario;
- median, min/max, p95, p99, error count, and fixture counters;
- whether `PROFILE=perf-stat` succeeded or fell back because the host disabled
  perf events.

Do not compare results collected with different Rust versions, OpenSSL versions,
CPU pinning policy, or fixture configuration as if they were the same run.
The default scenarios avoid `--tls-groups` because the current ylong public TLS
configuration API does not expose curve-group selection; use that option only
for libcurl-only diagnosis.

## Output

The runner prints one environment JSON line and one metrics JSON line for each
enabled client. Primary fields are:

- `rps`
- `latency_us_p50`
- `latency_us_p90`
- `latency_us_p95`
- `latency_us_p99`
- `latency_us_p999`
- `completed`
- `errors`
- `bytes`

Use `CLIENT_FILTER=ylong`, `CLIENT_FILTER=libcurl`, or `CLIENT_FILTER=both` to
select measured clients. Use `LIBCURL_MODE=multi` only when the report states
that the libcurl multi interface is the chosen comparator.
