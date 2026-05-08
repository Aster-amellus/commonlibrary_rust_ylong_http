# HTTPS 代理性能基准报告

本文档归档本地 HTTPS proxy 场景下 `ylong_http_client` 与 libcurl 的性能对比结果。

## 环境

```json
{"git":"33b3c33","rustc":"rustc 1.95.0 (59807616e 2026-04-14)","cargo":"cargo 1.95.0 (f2d3ce0bd 2026-03-21)","curl":"curl 8.18.0 (x86_64-pc-linux-gnu) libcurl/8.18.0 OpenSSL/3.5.5","openssl":"OpenSSL 3.5.5 27 Jan 2026","uname":"Linux aster 7.0.0-14-generic x86_64"}
```

## 正式验收 workload

场景：`HTTP target over HTTPS proxy`。

```bash
tools/https_proxy_bench/generate_certs.sh target/https_proxy_bench/certs
tools/https_proxy_bench/local_https_proxy.py \
  --cert-file target/https_proxy_bench/certs/server.pem \
  --key-file target/https_proxy_bench/certs/server.key \
  --origin-tls \
  --response-size 128 \
  --origin-port 18081 \
  --proxy-port 18444

REPEAT=5 tools/https_proxy_bench/run_https_proxy_bench.sh \
  --url http://127.0.0.1:18081/ \
  --proxy https://localhost:18444 \
  --proxy-ca-file target/https_proxy_bench/certs/ca.pem \
  --requests 5000 \
  --concurrency 16
```

说明：

- 该 workload 验证 HTTPS proxy 外层 TLS、HTTP absolute-form 请求、连接池复用和响应读取路径。
- 本地 fixture 在收到 HTTP absolute-form 请求时直接返回固定响应，用于隔离客户端到 HTTPS proxy 的传输开销；`--origin-tls` 是同一 fixture 会话用于 CONNECT 补充测试的设置，不影响该 HTTP workload。
- `async_https_proxy_bench` 将 `max_h1_conn_number` 设置为 `--concurrency`，与 libcurl harness 的并发连接模型对齐。
- 通过标准：同一 workload 下 5 次重复运行至少 4 次 `ylong_http_client` 吞吐高于 libcurl 20%，且错误数不高于 libcurl。

## 结果

| Run | ylong rps | libcurl rps | 提升 | ylong errors | libcurl errors | 结论 |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| 1 | 4377.102 | 349.945 | 1150.8% | 0 | 0 | pass |
| 2 | 4475.934 | 348.824 | 1183.1% | 0 | 0 | pass |
| 3 | 4645.255 | 348.160 | 1234.2% | 0 | 0 | pass |
| 4 | 3772.851 | 348.758 | 981.8% | 0 | 0 | pass |
| 5 | 4400.155 | 348.614 | 1162.2% | 0 | 0 | pass |

平均吞吐：

| Client | 平均 rps |
| --- | ---: |
| ylong_http_client | 4334.259 |
| libcurl | 348.860 |

平均提升：1142.4%。

结论：正式验收 workload 达成 5/5 次超过 20% 的目标，且错误数均为 0。

## Profiling Smoke

命令：

```bash
PROFILE=time REPEAT=1 tools/https_proxy_bench/run_https_proxy_bench.sh \
  --url http://127.0.0.1:18081/ \
  --proxy https://localhost:18444 \
  --proxy-ca-file target/https_proxy_bench/certs/ca.pem \
  --requests 1000 \
  --concurrency 8
```

| Client | rps | wall time | user time | system time | max RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| ylong_http_client | 940.160 | 1.08s | 0.04s | 0.01s | 12812 KB |
| libcurl | 161.298 | 6.20s | 0.69s | 0.17s | 18824 KB |

## 补充压测

`HTTPS target over HTTPS proxy` 使用同一 fixture 的 CONNECT 双层 TLS 路径验证。修正连接池并发上限后，p50 从排队型约 445ms 降到约 41ms，但 `concurrency=64` 下 Python TLS/relay fixture 尾延迟抖动明显，5 次中 3 次达到 20% 目标，平均提升 10.8%。该结果不作为正式性能达标口径，只作为后续使用 native proxy fixture 或真实代理环境复测的风险记录。

## Native CONNECT 复测

结论：严格口径仍未达标，不能将全部 OKR 标记为 100% 完成。

复测环境：原生 C/OpenSSL fixture，同时提供 TLS origin 和 TLS proxy，移除外部 `socat` TLS 包装进程。

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

| Run | ylong async rps | libcurl rps | 提升 | ylong errors | libcurl errors | 结论 |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| 1 | 3427.832 | 3504.304 | -2.2% | 0 | 0 | fail |
| 2 | 3393.455 | 3616.157 | -6.2% | 0 | 0 | fail |
| 3 | 3468.243 | 3678.995 | -5.7% | 0 | 0 | fail |
| 4 | 3328.815 | 3638.922 | -8.5% | 0 | 0 | fail |
| 5 | 3270.767 | 3629.325 | -9.9% | 0 | 0 | fail |

平均吞吐：

| Client | 平均 rps |
| --- | ---: |
| ylong_http_client async | 3377.822 |
| libcurl | 3613.541 |

平均提升：-6.5%。当前 native CONNECT workload 为 0/5 达到 20%+，错误数均为 0。

本阶段已落地的 CONNECT 路径优化：

- 外层 HTTPS proxy TLS 启用 OpenSSL `SSL_set_read_ahead`。
- 外层 HTTPS proxy TLS 设置 `SSL_set_default_read_buffer_len(256 KiB)`，短 profile 中 ylong `recvfrom` 从约 12.7k 降至约 5.3k。
- async benchmark 增加 warmup barrier、runtime threads、read buffer、GET request prebuild，避免把建连和请求构造成本混入传输阶段。
- native fixture 复测显示 `socat` 抖动已基本排除，剩余差距集中在 async futex/调度、连接池 dispatch 和 CONNECT 双层 TLS body drain 热路径。
