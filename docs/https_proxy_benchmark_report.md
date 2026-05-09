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

## Native CONNECT perf profiling

结论：本轮已经完成真实 `perf stat` 和 `perf record` profiling。严格口径仍未达标，且热点不再指向 async-only 调度问题，而是 ylong async/sync 共同的响应读取和内存拷贝路径。

环境：

```text
git: de2552a
rustc: rustc 1.95.0 (59807616e 2026-04-14)
cargo: cargo 1.95.0 (f2d3ce0bd 2026-03-21)
curl/libcurl: curl 8.18.0 libcurl/8.18.0 OpenSSL/3.5.5
openssl: OpenSSL 3.5.5 27 Jan 2026
kernel: Linux aster 7.0.0-14-generic x86_64
perf: perf version 7.0.0
perf_event_paranoid: -1
kptr_restrict: 0
nmi_watchdog: 0
```

构建参数：

```bash
RUSTFLAGS="-C debuginfo=1 -C force-frame-pointers=yes" \
cargo build -p ylong_http_client --example async_https_proxy_bench \
  --features "async http1_1 tokio_base c_openssl_3_0" --release

RUSTFLAGS="-C debuginfo=1 -C force-frame-pointers=yes" \
cargo build -p ylong_http_client --example sync_https_proxy_bench \
  --features "sync http1_1 tokio_base c_openssl_3_0" --release

cc -O2 -g -fno-omit-frame-pointer -Wall -Wextra -pthread \
  -o target/https_proxy_bench/libcurl_harness \
  tools/https_proxy_bench/libcurl_harness.c \
  $(curl-config --cflags --libs)
```

`requests=300`、`REPEAT=5` 基线复测：

| Client | 平均 rps | 平均提升 | errors |
| --- | ---: | ---: | ---: |
| ylong_http_client async | 3487.151 | -7.9% | 0 |
| libcurl | 3785.779 | baseline | 0 |

`requests=10000`、`perf stat -r 3` 结果：

| Client | 平均 rps | elapsed | task-clock | cycles | instructions | IPC | L1 miss | context switches |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ylong_http_client async | 3547.965 | 2.893s | 14.944s | 63.878B | 74.626B | 1.168 | 3.895B | 7742 |
| ylong_http_client sync | 3522.249 | 2.998s | 16.344s | 69.460B | 81.147B | 1.168 | 3.829B | 18938 |
| libcurl | 3823.429 | 2.685s | 14.667s | 62.391B | 71.905B | 1.153 | 2.279B | 14865 |

`perf record -F 997 -g --call-graph dwarf` 结果：

| Client | requests | samples | rps | p99 | top self hotspots |
| --- | ---: | ---: | ---: | ---: | --- |
| ylong_http_client async | 10000 | 16555 | 3531.805 | 34.807ms | `__memmove_avx_unaligned_erms` 16.10%, `_copy_to_iter` 10.09% |
| ylong_http_client sync | 10000 | 19176 | 3470.777 | 28.701ms | `__memmove_avx_unaligned_erms` 14.90%, `_copy_to_iter` 10.13% |
| libcurl | 10000 | 16072 | 3765.447 | 25.950ms | `_copy_to_iter` 8.77%, `__memmove_avx_unaligned_erms` 5.87% |

归因：

- ylong async 和 sync 在 10k workload 下都落后于 libcurl，因此当前主要瓶颈不是 Tokio 调度独有问题。
- ylong 的用户态 `memmove` 占比约为 libcurl 的 2.5 到 2.7 倍，且 L1 data cache miss 明显高于 libcurl，说明响应读取路径存在额外 copy/cache 压力。
- kernel `_copy_to_iter` 三者都较高，这是 loopback TCP 大 body 读取的共同成本，不是 ylong 独有。
- libcrypto 热点符号多数来自系统 OpenSSL stripped symbols，当前只能确认 TLS 加解密参与明显；下一步需要用 body/read path 优化先降低 ylong 额外内存拷贝，再复测。

下一阶段优化入口：

- 优先检查 `ylong_http_client/src/async_impl/http_body.rs` 和 `ylong_http_client/src/sync_impl/http_body.rs` 的 `Content-Length` body 读取路径，减少从 stream 到用户 buffer 之间的额外 copy。
- 检查 CONNECT 双层 TLS 下 `StreamData::poll_read/read` 是否存在中间缓冲复制或较小 read chunk。
- 优化完成后必须重新运行本节同一组 `perf stat`、`perf record` 和 `REPEAT=5` 正式验收。

## Native CONNECT read-ahead 复核

结论：`HTTPS target over HTTPS proxy` 的 CONNECT 路径不再对外层 proxy TLS 启用 OpenSSL read-ahead。HTTP target over HTTPS proxy 仍保留 read-ahead，因为该路径外层 TLS 直接承载 HTTP 响应体；CONNECT 路径则是外层 TLS 承载内层 origin TLS，read-ahead 会增加双层 `SSL_read` 的内部缓冲与复制压力。

变更 commit：

```text
f5ea840 perf(proxy): avoid TLS read-ahead for CONNECT proxy layer
```

行为验证：

```bash
cargo test -p ylong_http_client --test sdv_sync_https_proxy \
  --features "sync http1_1 tokio_base c_openssl_3_0"

cargo test -p ylong_http_client --test sdv_async_https_proxy \
  --features "async http1_1 ylong_base c_openssl_3_0"
```

结果：

| Test | Result |
| --- | --- |
| `sdv_sync_https_proxy` | 11 passed |
| `sdv_async_https_proxy` with `ylong_base` | 11 passed |
| `sdv_async_https_proxy` with `tokio_base` | 0 tests by cfg; this test file is gated on `ylong_base` |

`requests=300`、`REPEAT=5`、`concurrency=64`、`runtime_threads=16`、`response=1 MiB` 复测：

| Run | ylong async rps | libcurl rps | 提升 | errors | 结论 |
| --- | ---: | ---: | ---: | ---: | --- |
| 1 | 3703.397 | 3806.624 | -2.7% | 0 / 0 | fail |
| 2 | 3313.453 | 3694.991 | -10.3% | 0 / 0 | fail |
| 3 | 3312.537 | 3535.526 | -6.3% | 0 / 0 | fail |
| 4 | 3367.504 | 3613.239 | -6.8% | 0 / 0 | fail |
| 5 | 3524.712 | 3708.053 | -4.9% | 0 / 0 | fail |

平均吞吐：

| Client | 平均 rps |
| --- | ---: |
| ylong_http_client async | 3444.321 |
| libcurl | 3671.687 |

平均提升：-6.2%。严格 native CONNECT 口径仍为 0/5 达标。

`requests=10000`、`perf stat -r 3` 结果：

| Client | 平均 rps | elapsed | task-clock | cycles | instructions | cache-misses | context switches |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ylong_http_client async | 3597.749 | 2.965s | 14.409s | 61.664B | 73.326B | 359.402M | 8287 |
| libcurl | 3740.015 | 2.853s | 15.156s | 64.575B | 74.605B | 494.565M | 15806 |

`perf record -F 997 -g --call-graph fp` 观察：

| Client | requests | rps | p99 | top self hotspots |
| --- | ---: | ---: | ---: | --- |
| ylong_http_client async | 5000 | 3272.663 | 51.801ms | `_copy_to_iter` 6.44%, `__memmove_avx_unaligned_erms` 3.84% |
| libcurl | 5000 | 3641.210 | 26.569ms | `_copy_to_iter` 8.79%, `__memmove_avx_unaligned_erms` 5.69% |

归因更新：

- 关闭 CONNECT 外层 read-ahead 后，ylong 的 `memmove` 自身占比明显下降；此前 CONNECT profile 中 ylong async 的 `__memmove_avx_unaligned_erms` 约为 16.10%。
- 当前 ylong 的 CPU 指标不比 libcurl 差，甚至 cycles、instructions、cache misses 和 context switches 都更低；但 wall time 和 p99 仍落后。
- 瓶颈已经从明显的用户态复制热点，转为 CONNECT 双 TLS 读取链路的调度/唤醒/尾延迟问题。下一轮不能再只看 top self CPU hotspot，需要补 off-CPU、scheduler latency、Tokio task migration、per-connection progress 分布。

## Native CONNECT read-ahead buffer 调优

结论：完全关闭 CONNECT 外层 proxy TLS read-ahead 会减少 `memmove`，但会显著增加 `recvfrom` 系统调用次数。当前折中方案是在 CONNECT 外层 proxy TLS 上启用 read-ahead，但把 OpenSSL 默认 read buffer 从 HTTP target 路径的 256 KiB 降为 64 KiB。

变更 commit：

```text
3680b7b perf(proxy): tune CONNECT proxy TLS read-ahead buffer
```

行为验证：

```bash
cargo build -p ylong_http_client --example async_https_proxy_bench \
  --features "async http1_1 tokio_base c_openssl_3_0" --release

cargo build -p ylong_http_client --example sync_https_proxy_bench \
  --features "sync http1_1 tokio_base c_openssl_3_0" --release

cargo test -p ylong_http_client --test sdv_async_https_proxy \
  --features "async http1_1 ylong_base c_openssl_3_0"

cargo test -p ylong_http_client --test sdv_sync_https_proxy \
  --features "sync http1_1 tokio_base c_openssl_3_0"
```

结果：

| Test | Result |
| --- | --- |
| `async_https_proxy_bench` release build | passed |
| `sync_https_proxy_bench` release build | passed |
| `sdv_async_https_proxy` with `ylong_base` | 11 passed |
| `sdv_sync_https_proxy` | 11 passed |

`requests=300`、`REPEAT=5`、`concurrency=64`、`runtime_threads=16`、`response=1 MiB` 复测：

| Run | ylong async rps | libcurl rps | 提升 | errors | 结论 |
| --- | ---: | ---: | ---: | ---: | --- |
| 1 | 3757.962 | 3775.627 | -0.5% | 0 / 0 | fail |
| 2 | 3603.846 | 3831.467 | -5.9% | 0 / 0 | fail |
| 3 | 3684.468 | 3648.437 | 1.0% | 0 / 0 | fail |
| 4 | 3701.017 | 3734.037 | -0.9% | 0 / 0 | fail |
| 5 | 3782.198 | 3854.951 | -1.9% | 0 / 0 | fail |

平均吞吐：

| Client | 平均 rps |
| --- | ---: |
| ylong_http_client async | 3705.898 |
| libcurl | 3768.904 |

平均提升：-1.7%。64 KiB read-ahead buffer 明显优于完全关闭 read-ahead 的 -6.2%，但严格 native CONNECT 口径仍为 0/5 达到 20%+，因此 O4 仍未完成。

调优结论：

- CONNECT 外层 proxy TLS 不能直接沿用 HTTP target 路径的 256 KiB read buffer；128 KiB 复测更差，32 KiB p99 更差。
- 64 KiB 是当前本地 native CONNECT fixture 下的最好折中：减少完全关闭 read-ahead 带来的 syscall 压力，同时避免 256 KiB 下明显的嵌套 TLS 预读和复制放大。
- 下一步优化不能只调 OpenSSL read buffer，需要定位剩余 p99/调度差距：off-CPU、scheduler latency、Tokio task migration、每连接读取进度和 TLS/BIO read 次数。

## Native CONNECT body-read instrumentation

结论：应用层响应体读取 chunk 大小已经与 libcurl 对齐，不能继续把严格 CONNECT 未达标归因为 benchmark drain buffer 不一致。

变更 commit：

```text
75eaf31 bench(proxy): report response body read chunks
```

`requests=300`、`REPEAT=5`、`concurrency=64`、`runtime_threads=16`、`response=1 MiB`、`read_buffer_size=64 KiB` 复测：

| Run | ylong async rps | libcurl rps | 提升 | ylong body_reads | libcurl body_reads | avg read size | 结论 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 1 | 3265.911 | 3425.792 | -4.7% | 19200 | 19200 | 16384 | fail |
| 2 | 3282.499 | 3439.026 | -4.6% | 19200 | 19200 | 16384 | fail |
| 3 | 3221.091 | 3340.980 | -3.6% | 19200 | 19200 | 16384 | fail |
| 4 | 3408.323 | 3265.590 | 4.4% | 19200 | 19200 | 16384 | fail |
| 5 | 3182.171 | 3434.695 | -7.4% | 19200 | 19200 | 16384 | fail |

平均吞吐：

| Client | 平均 rps |
| --- | ---: |
| ylong_http_client async | 3271.999 |
| libcurl | 3381.217 |

平均提升：-3.2%。严格 native CONNECT 口径仍为 0/5 达到 20%+，错误数均为 0。

补充复核：

- `ylong_http_client` 和 libcurl 都以 16 KiB 应用层 chunk drain 1 MiB response：每轮 300 个请求均为 `19200` 次 body read。
- `runtime_threads=8` 下平均 ylong 约 3165 rps，libcurl 约 3282 rps，仍未达标。
- sync client 平均约 2904 rps，低于 async 和 libcurl，不是当前严格 CONNECT 的优先优化对象。
- 下一步需要继续沿 TLS/BIO read 次数、OpenSSL 内部 buffer、off-CPU scheduler latency、连接池 dispatch 和 per-connection progress 分布 profiling。

## Native CONNECT ylong-runtime 对照

结论：新增 `ylong_base` async benchmark 入口后，ylong runtime 在严格 CONNECT workload 下比 tokio runtime 更接近 libcurl，但仍不能满足 4/5 次 20%+ 的验收口径。runtime 切换不是充分优化。

变更 commit：

```text
86f985d bench(proxy): add ylong runtime async harness
```

新增入口：

```bash
cargo build -p ylong_http_client --example async_ylong_https_proxy_bench \
  --features "async http1_1 ylong_base c_openssl_3_0" --release

YLONG_CLIENT=async-ylong REPEAT=1 tools/https_proxy_bench/run_https_proxy_bench.sh \
  --url https://127.0.0.1:38081/ \
  --proxy https://localhost:38444 \
  --proxy-ca-file target/https_proxy_bench/certs/ca.pem \
  --origin-ca-file target/https_proxy_bench/certs/ca.pem \
  --requests 100 \
  --warmup-requests 16 \
  --concurrency 16 \
  --runtime-threads 8 \
  --read-buffer-size 65536
```

`requests=300`、`concurrency=64`、`runtime_threads=16`、`response=1 MiB` 的 3-run probe：

| Run | ylong async-ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 3730.592 | 3661.394 | 1.9% | 43.195ms | 23.752ms |
| 2 | 3798.952 | 3736.083 | 1.7% | 40.307ms | 25.956ms |
| 3 | 3787.848 | 3752.533 | 0.9% | 34.219ms | 23.535ms |

平均吞吐：

| Client | 平均 rps |
| --- | ---: |
| ylong_http_client async-ylong | 3772.464 |
| libcurl | 3716.670 |

平均提升：1.5%。该 probe 说明 ylong runtime 可缓解部分调度成本，但离 20%+ 目标仍有明显距离，且 p99 仍弱于 libcurl。

TLS/BIO trace 对照：

| Client | SSL_read calls | SSL_read errors | BIO_read calls | BIO_read errors | body_reads |
| --- | ---: | ---: | ---: | ---: | ---: |
| ylong_http_client async-ylong | 73016 | 1723 | 63352 | 2101 | 19200 |
| libcurl | 75726 | 2020 | 62076 | 2384 | 19200 |

归因更新：

- ylong runtime 入口下 `SSL_read` 次数不高于 libcurl，当前差距不能简单归因为 ylong 调用更多 OpenSSL read。
- `BIO_read` 次数仍略高，但差异小于前序 tokio 入口；需要继续结合 off-CPU 和 per-connection progress 观察等待时间，而不是只看 CPU top self。
- `perf stat` 的 1000-request probe 显示 ylong runtime cycles、instructions、context switches、cache misses 均低于 libcurl 或接近，但 wall time 只小幅领先，说明严格 CONNECT 剩余问题更像尾延迟/调度进度分布问题。
- 本阶段没有保留 `SSL_pending` ready-drain；内层 origin TLS read-ahead 后续收窄到 `HttpsOverProxy` 路径单独复测，结果见后文 P16。

## Native CONNECT worker elapsed instrumentation

结论：新增 worker 级 measured elapsed range 后，能看到 ylong 和 libcurl 的最慢 worker 都基本贴近总 wall time；该指标没有单独解释 ylong request p99 更高的问题。下一步需要更细的 per-request/connection id 或真正的 off-CPU scheduler trace。

变更 commit：

```text
e75ead5 bench(proxy): report worker elapsed ranges
```

`requests=300`、`REPEAT=3`、`concurrency=64`、`runtime_threads=16`、`response=1 MiB` 的 probe：

| Run | Client | rps | latency p99 | worker elapsed min | worker elapsed max |
| --- | --- | ---: | ---: | ---: | ---: |
| 1 | ylong async-ylong | 3918.874 | 36.971ms | 36.852ms | 75.930ms |
| 1 | libcurl | 3617.552 | 24.376ms | 16.923ms | 82.302ms |
| 2 | ylong async-ylong | 3434.853 | 53.662ms | 33.021ms | 87.232ms |
| 2 | libcurl | 3503.036 | 26.306ms | 38.135ms | 85.054ms |
| 3 | ylong async-ylong | 3385.951 | 40.191ms | 35.876ms | 87.744ms |
| 3 | libcurl | 3637.289 | 23.406ms | 22.245ms | 81.865ms |

观察：

- worker max 与总 elapsed 同量级，说明总吞吐主要受最慢 worker/连接批次收尾影响；这一点 ylong 和 libcurl 都成立。
- ylong 的 request p99 持续高于 libcurl，但 worker max 没有明显更差，说明问题可能在单连接内的请求完成分布、runtime wake 时机或嵌套 TLS record 读取进度，而不是简单的某个 worker 完全卡死。
- tracefs sched event 当前仍为 root-only，普通用户无法读取 `/sys/kernel/tracing/events/sched/sched_switch/id`；如需 `perf sched`，需要以 root 运行或放开 tracefs sched event 读权限。

## Native CONNECT request trace summary

结论：新增 request trace summary 后，strict CONNECT 的首要尾延迟来源进一步收敛到 response first-byte wait。`connect`/pool 侧 p99 为微秒级，不是当前主因；`body_drain` 仍有长尾，但低于 `response_wait` 对 request p99 的影响。

变更 commit：

```text
55d88f3 bench(proxy): add CONNECT trace summary histograms
7cc2ddf bench(proxy): route trace summary to ylong only
4df43a5 bench(proxy): split HTTP1 response wait timing
4b057ba fix(http1): flush request before response read
```

strict CONNECT trace probe：

```text
requests=300
warmup=64
concurrency=64
runtime_threads=16
read_buffer_size=65536
client=async-ylong
```

代表性 trace summary：

```json
{"kind":"request_trace_summary","completed":300,"errors":0,"request_ready_p99_us":43645,"connect_p99_us":4,"request_write_p99_us":8707,"response_wait_p99_us":43577,"transfer_p99_us":43601,"body_first_byte_p99_us":2603,"body_drain_p99_us":18261,"body_read_wait_p99_us":151,"body_read_wait_max_us":24833}
```

观察：

- `connect_p99_us` 只有微秒级，且 `--client-per-worker` 对照更慢，因此当前不优先做 pool direct handoff 或 sticky connection。
- `response_wait_p99_us` 基本贴近 `transfer_p99_us`，说明 `request_ready` 尾部主要发生在 request 写完到首个响应字节之间。
- 显式 flush request 后，strict CONNECT 仍未达 20% 目标；该改动保留为 HTTP/1 async 路径的正确性保护。
- `body_drain_p99_us` 仍有十毫秒级长尾，后续仍需要 nested TLS readiness / off-CPU trace，但它不是本轮 trace 中最大的 p99 来源。

flush 后 strict CONNECT 5-run：

| Run | ylong async-ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 3686.770 | 3671.342 | +0.4% | 51.795ms | 23.089ms |
| 2 | 3741.358 | 3790.415 | -1.3% | 40.437ms | 23.334ms |
| 3 | 3905.096 | 3726.060 | +4.8% | 36.217ms | 23.741ms |
| 4 | 3664.173 | 3752.768 | -2.4% | 36.238ms | 24.805ms |
| 5 | 3928.185 | 3849.213 | +2.1% | 53.612ms | 23.889ms |

平均：ylong async-ylong 约 3785.1 rps，libcurl 约 3758.0 rps，平均提升约 0.7%。严格 CONNECT 仍为 0/5 达到 20%+。

HTTP target regression smoke：

```text
url=http://127.0.0.1:38081/
proxy=https://localhost:38444
requests=20
concurrency=4
```

结果：ylong async-ylong 约 4753 rps，libcurl 约 1787 rps，HTTP target over HTTPS proxy 仍保持明显优势。

## Native CONNECT ready-drain 与热连接复用实验

结论：body ready-drain 能显著降低 ylong 应用层 body read 次数，但 strict CONNECT 吞吐仍未达到 20%+ 目标。当前剩余差距不再适合继续靠增大 ready-drain 预算解决。

变更 commit：

```text
a074348 perf(proxy): reduce CONNECT body poll churn
```

代码改动：

- `HttpBody` 的 Content-Length / until-close 路径在单次 poll 中最多连续消费 8 次 ready read，减少每个 TLS record 都返回到上层 future 的频率。
- HTTP/1 idle dispatcher 查找改为反向扫描，偏向复用最近仍然热的连接。

严格 CONNECT no-trace 5-run：

```text
url=https://127.0.0.1:38081/
proxy=https://localhost:38444
requests=300
warmup=64
concurrency=64
runtime_threads=16
read_buffer_size=262144
client=async-ylong
trace_summary=false
```

| Run | ylong async-ylong rps | libcurl rps | 提升 | ylong body reads | libcurl body reads | 结论 |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| 1 | 3832.280 | 3762.039 | +1.9% | 2532 | 19200 | fail |
| 2 | 3822.962 | 3686.727 | +3.7% | 2644 | 19200 | fail |
| 3 | 3606.209 | 3395.278 | +6.2% | 2556 | 19200 | fail |
| 4 | 3791.958 | 3628.535 | +4.5% | 2559 | 19200 | fail |
| 5 | 3831.178 | 3847.880 | -0.4% | 2672 | 19200 | fail |

平均：

| Client | 平均 rps | 平均 body reads |
| --- | ---: | ---: |
| ylong_http_client async-ylong | 3776.917 | 2592.6 |
| libcurl | 3664.092 | 19200.0 |

平均提升约 +3.1%，但严格 CONNECT 仍为 0/5 达到 20%+。

预算对照：

- `BODY_READY_DRAIN_READS=4`：短测 body reads 约 5k，吞吐基本与 libcurl 持平。
- `BODY_READY_DRAIN_READS=8`：body reads 约 2.6k，no-trace 5-run 平均约 +3.1%，当前保留。
- `BODY_READY_DRAIN_READS=16`：body reads 约 1.3k，但单次 read wait 变长，吞吐退化，不保留。

本轮验证：

```bash
rustfmt --check ylong_http_client/src/async_impl/http_body.rs ylong_http_client/src/async_impl/pool.rs
cargo check -p ylong_http_client --example async_ylong_https_proxy_bench --features "async http1_1 ylong_base c_openssl_3_0"
cargo check -p ylong_http_client --example async_https_proxy_bench --features "async http1_1 tokio_base c_openssl_3_0"
cargo test -p ylong_http_client --test sdv_async_https_proxy --features "async http1_1 ylong_base c_openssl_3_0"
cargo test -p ylong_http_client --test sdv_async_http_body_io --features "async http1_1 ylong_base"
```

下一步 profiling 重点：

- 正式吞吐验收不启用 `--trace-summary`，避免 ylong-only instrumentation 污染对比。
- trace 模式继续用于定位，但重点转向 off-CPU scheduler latency、Pending/wake 间隔、任务迁移和双层 TLS readiness 传播。
- 在没有上述证据前，不再继续盲目增大 body ready-drain 预算或 TLS read buffer。

## Native CONNECT origin TLS read-ahead 与负实验

结论：CONNECT 内层 origin TLS 现在单独启用 8 KiB read-ahead，范围仅限 `HttpsOverProxy` 的 inner origin TLS，不影响直连 HTTPS、HTTP target over HTTPS proxy 或 CONNECT 外层 proxy TLS 策略。该改动能减少双层 TLS body drain 中的读取推进次数，但 strict CONNECT 吞吐仍未达到 20%+ 目标。

变更 commit：

```text
c565843 perf(proxy): enable CONNECT origin TLS read-ahead
```

策略冻结：

- `HTTP target over HTTPS proxy`：外层 proxy TLS read-ahead 保持开启，OpenSSL read buffer 为 256 KiB。
- `HTTPS target over HTTPS proxy / CONNECT`：外层 proxy TLS read-ahead 保持开启，OpenSSL read buffer 为 64 KiB。
- `HTTPS target over HTTPS proxy / CONNECT`：内层 origin TLS 使用 8 KiB read-ahead。
- 非 CONNECT origin TLS 路径保持原来的无 read-ahead 行为，避免扩大优化范围。

严格 CONNECT no-trace 5-run：

```text
url=https://127.0.0.1:38081/
proxy=https://localhost:38444
requests=300
warmup=64
concurrency=64
runtime_threads=16
read_buffer_size=262144
client=async-ylong
trace_summary=false
```

| Run | ylong async-ylong rps | libcurl rps | 提升 | ylong body reads | libcurl body reads | 结论 |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| 1 | 3734.354 | 3406.613 | +9.6% | 2646 | 19200 | fail |
| 2 | 3561.242 | 3799.825 | -6.3% | 2637 | 19200 | fail |
| 3 | 3801.274 | 3803.438 | -0.1% | 2658 | 19200 | fail |
| 4 | 3747.396 | 3799.970 | -1.4% | 2665 | 19200 | fail |
| 5 | 3703.583 | 3821.121 | -3.1% | 2634 | 19200 | fail |

平均：

| Client | 平均 rps | 平均 body reads |
| --- | ---: | ---: |
| ylong_http_client async-ylong | 3709.570 | 2648.0 |
| libcurl | 3726.193 | 19200.0 |

平均提升约 -0.4%，strict CONNECT 仍为 0/5 达到 20%+。

本轮不保留的负实验：

- `SSL_pending` poll 内 drain：短测显示吞吐和 p99 退化，说明直接在通用 `AsyncSslStream::poll_read` 中循环消费 OpenSSL pending 数据会拉长单次 poll 或破坏双层 TLS 的推进节奏；不保留。
- `BODY_READY_DRAIN_READS=16`：body reads 降到约 1.3k，但 5-run 平均约 -1.9%，p99 明显高于 libcurl；不保留。
- `--client-per-worker` sticky client：吞吐低于共享 client/pool 模式，不支持“连接池 acquire/release 是主因”的假设。

本轮验证：

```bash
rustfmt --check ylong_http_client/src/async_impl/proxy.rs \
  ylong_http_client/src/sync_impl/proxy.rs \
  ylong_http_client/src/async_impl/connector/mod.rs \
  ylong_http_client/src/sync_impl/connector.rs

cargo check -p ylong_http_client --example async_ylong_https_proxy_bench \
  --features "async http1_1 ylong_base c_openssl_3_0"

cargo check -p ylong_http_client --example sync_https_proxy_bench \
  --features "sync http1_1 tokio_base c_openssl_3_0"

cargo test -p ylong_http_client --test sdv_async_https_proxy \
  --features "async http1_1 ylong_base c_openssl_3_0"

cargo test -p ylong_http_client --test sdv_sync_https_proxy \
  --features "sync http1_1 tokio_base c_openssl_3_0"
```

结果：async/sync HTTPS proxy SDV 各 11 passed。严格 CONNECT 性能仍未达标。

## Native CONNECT off-CPU profiling 尝试

结论：当前系统允许普通 `perf stat`，但不允许当前用户读取 scheduler tracepoint id，因此 `perf sched record` 不能直接运行。可用的降级证据继续指向调度/尾延迟：ylong 的 CPU 指标不差，但 p99 仍显著高于 libcurl。

阻塞点：

```text
perf sched record -o target/https_proxy_bench/profiles/perf-sched-smoke.data -- sleep 0.1

event syntax error: 'sched:sched_switch'
can't access trace events
No permissions to read /sys/kernel/tracing//events/sched/sched_switch
```

当前权限状态：

```text
/proc/sys/kernel/perf_event_paranoid = -1
/proc/sys/kernel/kptr_restrict = 0
/proc/sys/kernel/nmi_watchdog = 0
/sys/kernel/tracing/events/sched/sched_switch/id = -r--r----- root root
```

这说明还需要让当前用户可读 tracefs 的 sched tracepoint，例如由管理员执行：

```bash
sudo mount -o remount,mode=755 /sys/kernel/tracing
sudo chmod -R a+rX /sys/kernel/tracing/events/sched
```

在 tracefs 未放开前，本轮用 `/usr/bin/time -v` 和 `perf stat` 降级观测 `requests=10000` strict CONNECT：

| Client | rps | p99 | user | sys | voluntary cs | involuntary cs | max RSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ylong async-ylong | 3607.187 | 46.489ms | 10.63s | 2.93s | 1830 | 5984 | 40644 KB |
| libcurl | 3699.668 | 26.418ms | 10.81s | 3.83s | 14700 | 1913 | 36588 KB |

同一 workload 的 `perf stat` probe：

| Client | rps | p99 | task-clock | cycles | instructions | cache-misses | context switches | cpu migrations |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ylong async-ylong | 3730.365 | 44.118ms | 13.496s | 57.748B | 68.648B | 408.983M | 7897 | 675 |
| libcurl | 3509.012 | 27.123ms | 14.576s | 62.135B | 71.771B | 488.180M | 15589 | 1204 |

归因更新：

- `perf stat` 下 ylong 的吞吐可高于 libcurl，但 p99 仍明显更差，说明 O4b 的剩余问题不是平均 CPU 开销。
- `/usr/bin/time -v` 显示 ylong 自愿上下文切换少、非自愿上下文切换多；这更像 worker 被抢占或 wake/progress 分布不均，而不是连接池 acquire/release。
- 下一步必须拿到 `sched_switch`/`sched_wakeup` 级证据，或者在 ylong runtime/benchmark 内部补等价的 Pending-to-Ready gap histogram；否则继续改 TLS buffer、ready-drain 或通用 `SSL_pending` drain 会继续低效。
