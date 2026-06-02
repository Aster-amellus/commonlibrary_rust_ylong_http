# HTTPS Proxy Benchmark

This harness compares `ylong_http_client` with libcurl in HTTPS proxy scenarios without adding Rust dependencies.

## Build and Run

Start or provide an HTTPS proxy, then run both clients with the same workload:

```bash
tools/https_proxy_bench/run_https_proxy_bench.sh \
  --url http://127.0.0.1:18080/ \
  --proxy https://localhost:18443 \
  --proxy-ca-file target/https_proxy_bench/certs/ca.pem \
  --requests 10000 \
  --concurrency 64
```

The runner builds `async_https_proxy_bench`, compiles `libcurl_harness.c` when `curl-config` and `cc` are available, prints one JSON environment line, then prints one JSON metrics line per client run and one `bench_summary` JSON line per ylong client. Async metrics use distinct client labels for `ylong_http_client_async` and `ylong_http_client_async_ylong`, with a `runtime_backend` field for the Tokio or ylong runtime build. Set `REPEAT=5` to run both clients five times with the same workload. Formal comparisons default to `BENCH_ORDER=paired`, so each ylong run is followed by a libcurl run under the same current machine state. Set `BENCH_ORDER=grouped` to run all ylong repeats first and all libcurl repeats afterwards. The summary keeps the formal pass rule based on throughput and errors, repeats the key workload fields, reports whether warmup requests cover every worker, and also reports average/min/max P99 latency, worker start-delay max, and worker elapsed max for each client plus the libcurl baseline.

The libcurl harness selects `CURLPROXY_HTTP` or `CURLPROXY_HTTPS` from the
`--proxy` URL scheme, so the same tooling can run strict HTTPS proxy benchmarks
and plain HTTP proxy CONNECT isolation probes. Omit `--proxy` for direct-origin
isolation probes; in that mode the libcurl harness disables environment proxy
variables by setting an empty proxy.

Set `YLONG_CLIENT=async`, `YLONG_CLIENT=async-ylong`, `YLONG_CLIENT=sync`, or `YLONG_CLIENT=both` to select benchmark clients. The async ylong benchmark client sets its HTTP/1 connection-pool limit to the requested concurrency so the comparison matches libcurl's worker model. `--client-per-worker` is an async ylong-only diagnostic mode that builds one client per worker and is filtered out before invoking libcurl. `--runtime-mode current-thread-per-worker` is an async-ylong-only diagnostic mode that builds with `ylong_runtime/current_thread_runtime`, runs one current-thread runtime per OS worker thread, and forces one client per worker. `--runtime-mode current-thread-sharded` is an async-ylong-only diagnostic mode that builds with `ylong_runtime/current_thread_runtime`, runs `--runtime-threads` current-thread runtimes, and assigns benchmark workers across those shards. `--runtime-affinity` is ylong-runtime-only and enables core affinity on runtime worker threads. `--ylong-read-chunk-size N` is a ylong-only diagnostic that caps the slice passed to `response.data()` while leaving libcurl's `CURLOPT_BUFFERSIZE` at `--read-buffer-size`; omit it for the formal same-buffer comparison. `--trace-summary` is also ylong-only; it prints an additional `request_trace_summary` JSON line with request-ready, connect, request-write, response-wait, body-drain, and body-read wait histograms. `--phase-summary` is a lighter ylong-only diagnostic that skips per-poll tracing and prints response `TimeGroup` phase percentiles plus body drain percentiles. `--yield-after-body-read` is a ylong-only diagnostic that yields after every successful response-body read to test application-level fairness during large downloads. Use `--warmup-requests N` to pre-establish reusable proxy/origin connections before timing long-connection workloads. Use `--runtime-threads N` to size the ylong Tokio runtime for CPU-bound TLS transfer tests or the current-thread shard count for `current-thread-sharded`; when omitted it defaults to the requested concurrency. Use `--read-buffer-size N` to apply the same response read buffer size to ylong body draining and libcurl `CURLOPT_BUFFERSIZE`. JSON output includes `warmup_requests`, `runtime_threads`, `runtime_mode`, `runtime_affinity`, `read_buffer_size`, `ylong_read_chunk_size`, worker start-delay min/max, worker elapsed min/max, and ylong `prebuilt_requests` for GET workloads. Libcurl metrics also include diagnostic `CURLINFO_*_TIME_T` percentiles for connect, appconnect, start-transfer, total transfer, and derived body-transfer time; these fields do not change the formal throughput pass rule.

Set `YLONG_RUNTIME_PATH=/path/to/commonlibrary_rust_ylong_runtime` or
`YLONG_RUNTIME_PATH=/path/to/commonlibrary_rust_ylong_runtime/ylong_runtime`
to build the ylong-runtime benchmark against a local runtime checkout. The
runner passes a Cargo `patch` override for the existing git dependency and
prints the resolved path in `bench_environment.ylong_runtime_path`, so runtime
wake/scheduler experiments can be compared without changing this repository's
manifest. In this mode the runner also appends
`-A dangerous_implicit_autorefs` to `RUSTFLAGS`; Cargo normally caps dependency
lints for the git dependency, but a local path override is checked as local code
by current rustc. The environment JSON also includes a sorted
`ylong_runtime_env` object for active `YLONG_RUNTIME_*` flags, so scheduling
experiments remain tied to their exact runtime knobs. Client stderr is merged
into each per-client output stream, so runtime trace JSON or profiler output
emitted on stderr is preserved when the runner's stdout is redirected to a log
file.

Set `BENCH_CLIENT_TIMEOUT=SECONDS` to bound each individual client command.
The default `0` disables the timeout and keeps the old fail-fast behavior. When
enabled, the runner still extracts any metrics JSON emitted before a client
failure or timeout, prints the final summary, and exits nonzero afterwards.
Set `BENCH_LOG_ERRORS=1` when invoking the async ylong client directly to print
per-request error phase, worker, and request index on stderr; the runner keeps it
off by default to avoid timing noise.

POST upload workloads use the same flags for both clients:

```bash
REPEAT=5 tools/https_proxy_bench/run_https_proxy_bench.sh \
  --url http://127.0.0.1:18080/ \
  --proxy https://localhost:18443 \
  --proxy-ca-file target/https_proxy_bench/certs/ca.pem \
  --method POST \
  --body-size 1048576 \
  --requests 10000 \
  --warmup-requests 64 \
  --runtime-threads 64 \
  --read-buffer-size 65536 \
  --concurrency 64
```

## Local Fixture

Generate short-lived local certificates:

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

The fixture prints JSON containing `http_url` and `https_proxy`. Use those values in `run_https_proxy_bench.sh`.

To benchmark `HTTPS target over HTTPS proxy`, start the same fixture with an HTTPS origin:

```bash
tools/https_proxy_bench/local_https_proxy.py \
  --cert-file target/https_proxy_bench/certs/server.pem \
  --key-file target/https_proxy_bench/certs/server.key \
  --origin-tls \
  --response-size 1024 \
  --origin-port 18080 \
  --proxy-port 18443

tools/https_proxy_bench/run_https_proxy_bench.sh \
  --url https://127.0.0.1:18080/ \
  --proxy https://localhost:18443 \
  --proxy-ca-file target/https_proxy_bench/certs/ca.pem \
  --origin-ca-file target/https_proxy_bench/certs/ca.pem \
  --requests 10000 \
  --concurrency 64
```

For lower-noise CONNECT profiling, build and run the native OpenSSL fixture. It terminates TLS inside the C fixture instead of relying on external `socat` wrappers:

```bash
cc -O2 -Wall -Wextra -pthread \
  -o target/https_proxy_bench/native_proxy_fixture \
  tools/https_proxy_bench/native_proxy_fixture.c \
  $(pkg-config --cflags --libs openssl)

target/https_proxy_bench/native_proxy_fixture \
  --origin-port 38081 \
  --proxy-port 38444 \
  --response-size 1048576 \
  --origin-tls \
  --proxy-tls \
  --cert-file target/https_proxy_bench/certs/server.pem \
  --key-file target/https_proxy_bench/certs/server.key

REPEAT=5 tools/https_proxy_bench/run_https_proxy_bench.sh \
  --url https://127.0.0.1:38081/ \
  --proxy https://localhost:38444 \
  --proxy-ca-file target/https_proxy_bench/certs/ca.pem \
  --origin-ca-file target/https_proxy_bench/certs/ca.pem \
  --requests 300 \
  --warmup-requests 64 \
  --concurrency 64 \
  --runtime-threads 16 \
  --read-buffer-size 65536
```

For proxy mTLS smoke/performance runs:

```bash
tools/https_proxy_bench/local_https_proxy.py \
  --cert-file target/https_proxy_bench/certs/server.pem \
  --key-file target/https_proxy_bench/certs/server.key \
  --ca-file target/https_proxy_bench/certs/ca.pem \
  --require-client-cert

tools/https_proxy_bench/run_https_proxy_bench.sh \
  --url "$HTTP_URL" \
  --proxy "$HTTPS_PROXY" \
  --proxy-ca-file target/https_proxy_bench/certs/ca.pem \
  --proxy-client-cert target/https_proxy_bench/certs/client.pem \
  --proxy-client-key target/https_proxy_bench/certs/client.key \
  --requests 10000 \
  --concurrency 64
```

## Profiling

Use the same workload and wrap both clients:

```bash
PROFILE=time tools/https_proxy_bench/run_https_proxy_bench.sh ...
PROFILE=perf-stat tools/https_proxy_bench/run_https_proxy_bench.sh ...
```

For OpenSSL read/write diagnostics without tracefs privileges, build
`ssl_trace_preload.so` and run a single client with `LD_PRELOAD`. The helper
prints SSL/BIO call histograms plus `SSL_get_error` histograms split by the
last SSL operation; error-code histogram indexes use OpenSSL's numeric error
codes, for example `2 = SSL_ERROR_WANT_READ` and `3 = SSL_ERROR_WANT_WRITE`.
It also records WANT retry gaps from `SSL_get_error` to the next same-`SSL*`
operation, splits nested CONNECT `SSL_read` retries by depth, and reports
same-thread vs migrated retries using pthread identity. Each retry-gap class
also includes max-sample fields for the SSL pointer and the WANT/retry pthread
ids, which help distinguish repeated long gaps on one TLS layer from a broader
worker migration pattern.

Primary metrics are requests/sec, P50/P90/P95/P99 latency, errors, and CPU counters from the selected profiler. The contest target is met when `ylong_http_client` throughput is at least 20% higher than libcurl for the same proxy, origin, method, body size, response size, request count, concurrency, and TLS verification settings. For formal runs, use `REPEAT=5`; at least four runs should meet the 20% throughput target and ylong's error rate must not exceed libcurl's. The runner encodes this in `bench_summary.formal_pass` with defaults `BENCH_TARGET_PCT=20` and `BENCH_TARGET_MIN_PASSES=4`; set `BENCH_ENFORCE_TARGET=1` to make the script exit nonzero when the summary does not pass.
