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

The runner builds `async_https_proxy_bench`, compiles `libcurl_harness.c` when `curl-config` and `cc` are available, prints one JSON environment line, then prints one JSON metrics line per client run. Set `REPEAT=5` to run both clients five times with the same workload. The ylong benchmark client sets its HTTP/1 connection-pool limit to the requested concurrency so the comparison matches libcurl's worker model.

POST upload workloads use the same flags for both clients:

```bash
REPEAT=5 tools/https_proxy_bench/run_https_proxy_bench.sh \
  --url http://127.0.0.1:18080/ \
  --proxy https://localhost:18443 \
  --proxy-ca-file target/https_proxy_bench/certs/ca.pem \
  --method POST \
  --body-size 1048576 \
  --requests 10000 \
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
