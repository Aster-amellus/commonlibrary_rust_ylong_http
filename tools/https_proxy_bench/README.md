# HTTPS Proxy Benchmark

This directory contains the minimal benchmark harness for comparing
`ylong_http_client` with libcurl in HTTP/1.1 HTTPS proxy scenarios.

The harness intentionally excludes runtime patches, trace preload libraries, and
historical result reports. Use it to collect a clean baseline before doing any
performance optimization.

## Benchmark Governance

Performance work is split into four layers:

- Correctness tests prove HTTPS proxy behavior, CONNECT, proxy TLS verification,
  mTLS, `no_tls + https proxy` rejection, and existing HTTP proxy behavior.
- Hot-path tests isolate local costs such as pool lookup, HTTP/1 dispatcher
  checkout, HTTP/1 encode/read, `SSL_write`, `SSL_read`, BIO callbacks, and
  runtime I/O polling.
- Enterprise-pressure tests compare ylong with libcurl under mixed payloads,
  high concurrency, optional RTT shaping, and connection churn.
- Degradation tests find tail-latency cliffs under pool pressure, reconnects,
  long soak runs, and larger payloads.

Final performance claims require both hot-path evidence and enterprise-pressure
evidence. A single loopback run is diagnostic evidence only.

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

Start a local origin and HTTPS proxy:

```bash
tools/https_proxy_bench/run_https_proxy_fixture_rs.sh \
  --cert-file target/https_proxy_bench/certs/server.pem \
  --key-file target/https_proxy_bench/certs/server.key \
  --origin-tls \
  --response-size 1024
```

The fixture prints JSON with `origin_url` and `https_proxy`. Use those values
with the runner:

```bash
tools/https_proxy_bench/run_https_proxy_bench.sh \
  --url https://127.0.0.1:18080/ \
  --proxy https://127.0.0.1:18443 \
  --proxy-ca-file target/https_proxy_bench/certs/ca.pem \
  --origin-ca-file target/https_proxy_bench/certs/ca.pem \
  --requests 10000 \
  --warmup-requests 64 \
  --concurrency 64
```

For HTTP origin over HTTPS proxy, omit `--origin-tls` when starting the fixture
and omit `--origin-ca-file` from the runner.

`local_https_proxy.py` is retained only for functional smoke checks. Do not use
it for final performance claims because it is Python-based and its HTTP proxy
path does not model the final benchmark path.

### Tool Retention

- `fixture-rs/` is the benchmark fixture for performance evidence.
- `run_https_proxy_bench.sh` is the low-level common runner.
- `run_https_proxy_scenario.sh` is the scenario runner for governed runs.
- `libcurl_harness.c` is the libcurl comparator.
- `local_https_proxy.py` is retained only for functional smoke checks when the
  Rust fixture cannot cover a case. It must not be used for final performance
  claims.
- Files under `target/`, `perf.data`, and fixture build outputs are generated
  artifacts and must not be committed.

## Scenario Runner

Use the scenario runner for governed benchmark runs:

```bash
tools/https_proxy_bench/run_https_proxy_scenario.sh \
  tools/https_proxy_bench/scenarios/hot_path_steady_1k.env
```

The script prints the fixture command, the fully expanded low-level benchmark
command, and the governed scenario command, then exits by default. Start the
printed fixture command in a separate terminal, then rerun with `RUN_BENCH=1`.
Results are written to `target/https_proxy_bench/results/<scenario>_*.jsonl`
with build and profiler stderr in the adjacent `.perf` file.

## Correctness Evidence Protocol

Correctness evidence is separate from performance evidence. Use scenario dry
runs to confirm the fixture path and cargo tests to prove client behavior:

```bash
tools/https_proxy_bench/run_https_proxy_scenario.sh \
  tools/https_proxy_bench/scenarios/correctness_http_target_over_https_proxy.env

tools/https_proxy_bench/run_https_proxy_scenario.sh \
  tools/https_proxy_bench/scenarios/correctness_https_proxy.env

tools/https_proxy_bench/run_https_proxy_scenario.sh \
  tools/https_proxy_bench/scenarios/correctness_https_proxy_mtls.env
```

The first scenario exercises HTTP origin over HTTPS proxy. The second exercises
HTTPS origin over HTTPS proxy through CONNECT. The third requires proxy mTLS and
passes the generated client certificate to both ylong and libcurl.

Run the client behavior tests before using performance results as evidence:

```bash
cargo test -p ylong_http_client --test sdv_async_https_proxy \
  --features "async http1_1 ylong_base c_openssl_3_0" --release

cargo test -p ylong_http_client --lib ut_proxy_route \
  --features "async http1_1 tokio_base" --release
```

These tests cover HTTPS proxy request forwarding, CONNECT tunneling, proxy
TLS server verification, proxy mTLS, proxy auth propagation, TLS failure
context, and the explicit `no_tls + https proxy` error.

## Primary Enterprise Steady-State Protocol

Use `perf_enterprise_steady.sh` for the primary enterprise steady-state run. It
generates certificates only when `target/https_proxy_bench/certs/ca.pem` is
missing, prints the Rust fixture command, and exits without running the benchmark
unless `RUN_BENCH=1` is set.

In one terminal, start the Rust fixture:

```bash
tools/https_proxy_bench/run_https_proxy_fixture_rs.sh \
  --cert-file target/https_proxy_bench/certs/server.pem \
  --key-file target/https_proxy_bench/certs/server.key \
  --origin-tls \
  --response-size 1024
```

In another terminal, run the protocol:

```bash
RUN_BENCH=1 tools/https_proxy_bench/perf_enterprise_steady.sh
```

Defaults are `URL=https://127.0.0.1:18080/`,
`PROXY=https://localhost:18443`, `REQUESTS=50000`, `WARMUP=4096`,
`CONCURRENCY=128`, and `READ_BUFFER_SIZE=65536`. The script runs the benchmark
with `FRAME_POINTERS=1 PROFILE=perf-stat`, the generated CA for both proxy and
origin TLS, and writes JSON lines to
`target/https_proxy_bench/results/enterprise_steady_YYYYmmdd_HHMMSS.jsonl`.
Build and profiler stderr are written to the adjacent `.jsonl.perf` sidecar
file.

The runner auto-detects `OPENSSL_LIB_DIR` and `OPENSSL_INCLUDE_DIR` with
`pkg-config` when they are not already set. If `PROFILE=perf-stat` is requested
but the host disallows perf events, the runner records the warning in stderr and
continues without perf so the run can still serve as a functional smoke check.
Do not treat that run as hot-path evidence.

Fixture counters should show `proxy_tls_handshakes` and `connect_requests`
close to the connection count, not the request count.

## Bottleneck Matrix

Use `run_https_proxy_bottleneck_matrix.sh` after the primary baseline shows a
stable gap and you need to separate benchmark-shape effects from ylong
implementation costs.

In one terminal, start the same Rust fixture used by the primary protocol. In a
second terminal, run:

```bash
RUN_BENCH=1 tools/https_proxy_bench/run_https_proxy_bottleneck_matrix.sh
```

The matrix writes JSON lines to
`target/https_proxy_bench/results/bottleneck_matrix_YYYYmmdd_HHMMSS.jsonl` and
runs four rows:

- ylong with one shared `Client`.
- ylong with one `Client` per worker.
- libcurl with one easy handle and connection cache per worker thread.
- libcurl with a shared connection cache protected by libcurl share callbacks.

Interpretation:

- If ylong per-worker is much faster than ylong shared, shared client pool or
  HTTP/1 dispatcher contention is implicated.
- If libcurl shared-cache is much slower than libcurl per-thread, global
  connection-cache locking is expensive for this workload and should not be used
  as the default comparator.
- If ylong per-worker remains far behind libcurl per-thread, investigate
  HTTP/1 allocation/copy cost and the double TLS data path next.

The common runner also accepts targeted experiment variables:

```bash
CLIENT_FILTER=ylong YLONG_CLIENT_MODE=per-worker \
  tools/https_proxy_bench/run_https_proxy_bench.sh ...

CLIENT_FILTER=libcurl LIBCURL_SHARE_CONNECTIONS=1 \
  tools/https_proxy_bench/run_https_proxy_bench.sh ...
```

## ylong Phase Metrics

Use phase metrics after the bottleneck matrix shows ylong shared-client mode is
slower than ylong per-worker mode. This splits the ylong connection checkout
path into global pool lookup, HTTP/1 dispatcher checkout, semaphore waiting, and
new connection fallback.

In one terminal, start the Rust fixture. In another terminal, run:

```bash
RUN_BENCH=1 CLIENT_FILTER=ylong YLONG_CLIENT_MODE=shared YLONG_PHASE_METRICS=1 \
tools/https_proxy_bench/run_https_proxy_bench.sh \
  --url https://127.0.0.1:18080/ \
  --proxy https://127.0.0.1:18443 \
  --proxy-ca-file target/https_proxy_bench/certs/ca.pem \
  --origin-ca-file target/https_proxy_bench/certs/ca.pem \
  --requests 50000 \
  --warmup-requests 4096 \
  --concurrency 128
```

The ylong benchmark prints one extra JSON line:

```json
{"event":"ylong_phase_metrics", ...}
```

Interpretation:

- High `pool_get_lock_wait_avg_ns` means the global `Pool<HashMap>` lock is
  contended.
- High `h1_list_lock_wait_avg_ns` means the HTTP/1 dispatcher list lock is
  contended.
- High `h1_list_lock_hold_avg_ns` plus high `h1_scanned_dispatchers` means the
  `Vec` checkout scan is expensive.
- High `h1_semaphore_wait_avg_ns` means max connection permits are limiting
  concurrency.
- High `connector_connect_avg_ns` together with high `h1_new_connect` means the
  run is not measuring steady-state reuse.

### HTTP/1 Connection Budget Sweep

When `h1_semaphore_wait_avg_ns` dominates shared-client runs, sweep the HTTP/1
connection budget before changing pool internals:

```bash
for budget in 6 16 32 64 128 256; do
  RUN_BENCH=1 CLIENT_FILTER=ylong YLONG_CLIENT_MODE=shared \
    YLONG_PHASE_METRICS=1 YLONG_MAX_H1_CONN_NUMBER="$budget" \
    tools/https_proxy_bench/run_https_proxy_bench.sh \
      --url https://127.0.0.1:18080/ \
      --proxy https://127.0.0.1:18443 \
      --proxy-ca-file target/https_proxy_bench/certs/ca.pem \
      --origin-ca-file target/https_proxy_bench/certs/ca.pem \
      --requests 50000 \
      --warmup-requests 4096 \
      --concurrency 128
done
```

If higher budgets collapse `h1_semaphore_wait_avg_ns` and improve RPS, the first
bottleneck is the per-client HTTP/1 connection budget. If dispatcher lock or
scan metrics become large after the budget increases, they become the next
candidate for pool-structure optimization.

## Hot-Path Evidence Protocol

Use `hot_path_steady_1k.env` to decide whether a code path is worth optimizing.
The run must include phase metrics and perf counters:

```bash
tools/https_proxy_bench/run_https_proxy_scenario.sh \
  tools/https_proxy_bench/scenarios/hot_path_steady_1k.env
```

Start the printed fixture command, then run:

```bash
RUN_BENCH=1 tools/https_proxy_bench/run_https_proxy_scenario.sh \
  tools/https_proxy_bench/scenarios/hot_path_steady_1k.env
```

For a patch to pass this layer, the target phase must move beyond observed
noise, adjacent phases must not erase the gain, `errors` must remain zero, and
fixture metrics must show no proxy or origin errors.

## Enterprise Comparator Protocol

Use `enterprise_mixed_pressure.env` for the primary ylong-vs-libcurl comparison.
The comparator must record:

- ylong shared-client mode and configured HTTP/1 connection budget.
- libcurl connection cache mode.
- proxy and origin TLS verification mode.
- payload distribution.
- concurrency, warmup, request count or duration.
- CPU pinning policy.
- fixture counters for handshakes, CONNECT requests, forwarded requests, and
  proxy/origin errors.

When validating libcurl's HTTP/2 multi interface as a possible stronger
baseline, record whether `--libcurl-pipewait` is enabled and any configured
`--libcurl-max-host-connections`, `--libcurl-max-total-connections`, or
`--libcurl-max-concurrent-streams` values. These knobs are comparator controls;
they are not part of the default ylong-vs-libcurl easy-threaded comparison.

Run order should alternate ylong and libcurl, or the scenario should be repeated
enough to estimate run-to-run noise. Do not use libcurl shared-cache mode as the
default comparator unless the experiment is specifically about shared cache
locking.

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
- `latency_us_p999`
- `completed`
- `errors`

For low-noise runs, pin the process externally with `taskset` or `numactl` and
collect CPU counters with:

```bash
PERF_EVENTS=cycles,instructions,cache-misses,branch-misses \
PROFILE=perf-stat tools/https_proxy_bench/run_https_proxy_bench.sh ...
```
