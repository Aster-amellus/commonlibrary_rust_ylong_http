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

The runner builds `async_https_proxy_bench`, compiles `libcurl_harness.c` when `curl-config` and `cc` are available, prints one JSON environment line, then prints one JSON metrics line per client run. Set `REPEAT=5` to run both clients five times with the same workload. Formal comparisons default to `BENCH_ORDER=paired`, so each ylong run is followed by a libcurl run under the same current machine state. Set `BENCH_ORDER=grouped` to run all ylong repeats first and all libcurl repeats afterwards.

Set `YLONG_CLIENT=async`, `YLONG_CLIENT=sync`, or `YLONG_CLIENT=both` to select benchmark clients. The async ylong benchmark client sets its HTTP/1 connection-pool limit to the requested concurrency so the comparison matches libcurl's worker model. `--client-per-worker` is an async ylong-only diagnostic mode that builds one client per worker and is filtered out before invoking libcurl. Use `--warmup-requests N` to pre-establish reusable proxy/origin connections before timing long-connection workloads. Use `--runtime-threads N` to size the ylong Tokio runtime for CPU-bound TLS transfer tests; when omitted it defaults to the requested concurrency. Use `--read-buffer-size N` to apply the same response read buffer size to ylong body draining and libcurl `CURLOPT_BUFFERSIZE`. JSON output includes `warmup_requests`, `runtime_threads`, `read_buffer_size`, and ylong `prebuilt_requests` for GET workloads.

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

Primary metrics are requests/sec, P50/P90/P95/P99 latency, errors, and CPU counters from the selected profiler. The contest target is met when `ylong_http_client` throughput is at least 20% higher than libcurl for the same proxy, origin, method, body size, response size, request count, concurrency, and TLS verification settings. For formal runs, use `REPEAT=5`; at least four runs should meet the 20% throughput target and ylong's error rate must not exceed libcurl's.
