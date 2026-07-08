# HTTPS Proxy Benchmark 报告

## 1. 结论

本提交提供可复现的 HTTPS proxy benchmark 框架。正式性能结论必须基于
`tools/https_proxy_bench/scenarios/enterprise_mixed_pressure.env` 的多轮结果，
并记录环境、命令、原始 JSONL 和 perf sidecar 文件。

当前交付分支不把实验性测量代码或临时优化开关作为主线能力。
这些工具可用于后续专项分析，但不能作为初赛默认复现路径。

## 2. 测试对象

| 对象 | 说明 |
| --- | --- |
| ylong | `ylong_http_client` async client，HTTP/1.1，OpenSSL FFI |
| libcurl | C harness，使用系统 libcurl 和 OpenSSL |
| fixture | Rust HTTPS proxy + origin server |
| 主场景 | HTTPS origin over HTTPS proxy |

链路：

```text
ylong/libcurl -> TLS -> HTTPS proxy -> CONNECT -> TLS -> origin
```

## 3. 环境记录

每次正式报告必须记录：

```bash
git rev-parse HEAD
git status --short
rustc --version
cargo --version
curl --version
openssl version -a
uname -a
lscpu
```

如果使用 `perf stat`，还要记录：

```bash
sysctl kernel.perf_event_paranoid
sysctl kernel.kptr_restrict
```

## 4. 正确性 Smoke

先 dry-run 场景，确认 fixture 命令和 benchmark 命令：

```bash
tools/https_proxy_bench/run_https_proxy_scenario.sh \
  tools/https_proxy_bench/scenarios/correctness_https_proxy.env
```

启动打印出的 fixture 后运行：

```bash
RUN_BENCH=1 tools/https_proxy_bench/run_https_proxy_scenario.sh \
  tools/https_proxy_bench/scenarios/correctness_https_proxy.env
```

该层只证明 harness 可运行，不能用来声明性能领先。

本地 smoke 记录：

```text
date: 2026-07-09
rustc: rustc 1.95.0 (59807616e 2026-04-14)
cargo: cargo 1.95.0 (f2d3ce0bd 2026-03-21)
curl: curl 8.18.0, libcurl/8.18.0, OpenSSL/3.5.5
path: client -> TLS -> HTTPS proxy -> CONNECT -> TLS -> origin
requests: 200
warmup: 20
concurrency: 16
payload: 1024 bytes
errors: 0 for ylong and libcurl
```

低层 smoke 命令使用备用端口，避免影响本机已有 fixture：

```bash
tools/https_proxy_bench/run_https_proxy_fixture_rs.sh \
  --cert-file target/https_proxy_bench/certs/server.pem \
  --key-file target/https_proxy_bench/certs/server.key \
  --response-size 1024 \
  --origin-tls \
  --origin-delay-ms 0 \
  --origin-port 28080 \
  --proxy-port 28443

CLIENT_FILTER=both YLONG_CLIENT_MODE=shared YLONG_MAX_H1_CONN_NUMBER=64 \
tools/https_proxy_bench/run_https_proxy_bench.sh \
  --url https://127.0.0.1:28080/ \
  --proxy https://127.0.0.1:28443 \
  --proxy-ca-file target/https_proxy_bench/certs/ca.pem \
  --origin-ca-file target/https_proxy_bench/certs/ca.pem \
  --requests 200 \
  --warmup-requests 20 \
  --concurrency 16 \
  --read-buffer-size 65536
```

结果摘要：

| Client | Completed | Errors | Bytes | RPS | p99 |
| --- | ---: | ---: | ---: | ---: | ---: |
| ylong | 200 | 0 | 204800 | 60551.014 | 508 us |
| libcurl | 200 | 0 | 204800 | 38774.719 | 827 us |

该结果只用于证明 DEMO 和 comparator 链路可运行。正式性能排名仍需使用第 5 节
的多轮 enterprise 场景。

## 5. 正式对比协议

推荐命令：

```bash
FIXTURE_CPU_LIST=0-3 BENCH_CPU_LIST=4-15 \
tools/https_proxy_bench/run_https_proxy_scenario.sh \
  tools/https_proxy_bench/scenarios/enterprise_mixed_pressure.env
```

启动打印出的 fixture 后运行：

```bash
RUN_BENCH=1 BENCH_CPU_LIST=4-15 \
tools/https_proxy_bench/run_https_proxy_scenario.sh \
  tools/https_proxy_bench/scenarios/enterprise_mixed_pressure.env
```

默认场景参数：

| 参数 | 值 |
| --- | --- |
| URL | `https://127.0.0.1:18080/` |
| Proxy | `https://127.0.0.1:18443` |
| Requests | `300000` |
| Warmup | `32768` |
| Concurrency | `1024` |
| Read buffer | `65536` |
| Payload sequence | `1024,4096,16384,65536,262144` |
| Repeat | `5` |
| Comparator | ylong + libcurl |

## 6. 指标口径

| 指标 | 说明 |
| --- | --- |
| RPS | 完成请求数 / 测量时间 |
| p50/p90/p95/p99/p999 | 单请求端到端延迟分位值 |
| errors | 客户端错误数，必须为 0 |
| bytes | 响应体读取字节数，用于检查两端 workload 是否一致 |
| perf counters | `task-clock`、`cycles`、`instructions`、`cache-misses`、`context-switches` |

正式结论使用 5 轮结果的 median，并报告 min/max。单次结果只能作为诊断证据。

## 7. 已知风险

- loopback 环境不代表真实 NIC、队列、DMA 和跨主机 RTT。
- 不同 Rust/LLVM、OpenSSL、libcurl 版本会改变优化结果，不能直接横向比较。
- CPU governor、后台负载和 NUMA 会影响尾延迟。
- TLS session reuse、连接池容量和 libcurl cache 模式必须写入报告。
- `perf stat` 在部分机器上会因为内核权限降级；这种运行不能作为热点证据。

## 8. 结果模板

```text
commit:
rustc:
openssl:
libcurl:
scenario:
cpu pinning:

ylong median:
  rps:
  p99:
  bytes:
  errors:

libcurl median:
  rps:
  p99:
  bytes:
  errors:

delta:
  rps:
  p99:

raw files:
  jsonl:
  perf:
```

## 9. 后续优化入口

如果正式结果需要继续优化，先用 `hot_path_steady_1k.env` 和 `perf stat/record`
确认热点。只有当目标路径在 profile 中占比足够高、预期收益超过 benchmark 噪声时，
才进入代码优化。
