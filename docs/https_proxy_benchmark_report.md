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

## Native CONNECT origin TLS read-ahead 负实验

结论：CONNECT 内层 origin TLS 8 KiB read-ahead 曾短期保留在 `HttpsOverProxy` 路径，用于减少双层 TLS body drain 中的读取推进次数；但 5-run 吞吐平均为 -0.4%，不支持最终 20%+ 目标。当前已撤回该 tuning，CONNECT 内层 origin TLS 回到普通 origin TLS 路径，不启用 OpenSSL read-ahead。

变更 commit：

```text
c565843 perf(proxy): enable CONNECT origin TLS read-ahead
```

当前策略：

- `HTTP target over HTTPS proxy`：外层 proxy TLS read-ahead 保持开启，OpenSSL read buffer 为 256 KiB。
- `HTTPS target over HTTPS proxy / CONNECT`：外层 proxy TLS read-ahead 保持开启，OpenSSL read buffer 为 64 KiB。
- `HTTPS target over HTTPS proxy / CONNECT`：内层 origin TLS 不启用 read-ahead，与非 CONNECT origin TLS 路径一致，避免为了降低 body read 次数引入负吞吐收益。

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

因此，本轮后续代码移除了 `ORIGIN_TLS_READ_AHEAD_BUFFER`，async/sync CONNECT 的内层 origin TLS 都改回 `connect_tls(...)`。该变更不是 20%+ 的充分修复，只是撤掉已证伪的优化，避免把降低 body read 次数误当成吞吐收益。

撤回后的 native CONNECT 单轮 smoke（非正式验收，`requests=300`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=256 KiB`）：

| Client | rps | p99 | body reads | errors |
| --- | ---: | ---: | ---: | ---: |
| ylong async-ylong | 3655.302 | 35.564ms | 2703 | 0 |
| libcurl | 3663.227 | 27.972ms | 19200 | 0 |

`bench_summary.formal_pass=false`，单轮提升 -0.216%。该 smoke 只证明撤回后功能路径和 runner gate 正常；严格 CONNECT 的 20%+ 目标仍未完成。

### Client-side first-byte path audit

撤回内层 origin TLS read-ahead 后，又复核了严格 CONNECT 中可能影响 `response_wait_p99_us` / `request_pending_gap_p99_us` 的客户端侧路径：

- `HttpStream::conn_data()` / `ConnData::clone()`：发生在连接建立后转入 pool/dispatcher，或 HTTP target 代理鉴权 header 生成；严格 CONNECT 的已测 `connect_p99_us` 为微秒级，主尾延迟发生在请求写完后的 Future Pending gap，因此这里不是当前瓶颈。
- `TimeGroup::update_transport_conn_time()`：存在重复更新 TCP 时间字段的问题，但它是 request/response 时间统计搬运，不参与 I/O poll、wake 或 response first-byte 读取；本轮不把它作为性能补丁处理。
- HTTP/1 空 body 编码路径：已通过 `Body::is_empty()` 跳过非 chunked 空 body 的无意义 body encoder/read；后续 trace 仍显示尾延迟主要在 response first-byte wait，而不是 request body 读取。

结论：当前 repo-local 客户端侧没有发现新的、小而确定的 strict CONNECT first-byte 性能补丁。现有证据仍指向 ylong_runtime I/O wake / worker queue / task migration 侧；继续扩大 TLS read-ahead、body drain 或统计路径改动都不符合本轮测量结果。

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

## Native CONNECT future Pending gap trace

结论：在 `--trace-summary` 中加入 Future poll wrapper 后，strict CONNECT 的 p99 进一步定位到单次 request future Pending 后的长 gap。request future 的 p99 Pending 次数只有 1 次，但 Pending 到下一次 poll 的 p99 gap 约 36ms，基本等于 `response_wait_p99`；body data future 也存在较长 Pending gap，但它是次要问题。

变更 commit：

```text
a238a18 bench(proxy): trace CONNECT future pending gaps
```

trace probe：

```bash
REPEAT=1 YLONG_CLIENT=async-ylong tools/https_proxy_bench/run_https_proxy_bench.sh \
  --url https://127.0.0.1:38081/ \
  --proxy https://localhost:38444 \
  --proxy-ca-file target/https_proxy_bench/certs/ca.pem \
  --origin-ca-file target/https_proxy_bench/certs/ca.pem \
  --requests 300 \
  --warmup-requests 64 \
  --concurrency 64 \
  --runtime-threads 16 \
  --read-buffer-size 262144 \
  --trace-summary
```

核心输出：

| Metric | Value |
| --- | ---: |
| ylong rps | 3744.020 |
| ylong p99 | 42.907ms |
| libcurl rps | 3776.530 |
| libcurl p99 | 23.272ms |
| request_ready_p99_us | 36264 |
| response_wait_p99_us | 36195 |
| request_poll_count_p99 | 2 |
| request_pending_count_p99 | 1 |
| request_pending_gap_p99_us | 36127 |
| request_pending_gap_max_us | 44362 |
| body_pending_count_p99 | 14 |
| body_pending_gap_p99_us | 14567 |
| body_pending_gap_max_us | 35574 |

归因更新：

- request future 在写完请求后通常只 Pending 一次；长尾来自这次 Pending 到下一次 poll 的等待，而不是频繁 poll churn。
- `connect_p99_us` 仍只有 5us，继续排除连接池 acquire 作为主因。
- `request_write_p99_us` 为 11311us，但 `response_wait_p99_us` 与 `request_pending_gap_p99_us` 基本重合；下一步应沿 request future Pending 的 wake 来源继续查，而不是扩大 body drain/read-ahead。
- 在 tracefs 未开放前，库内或 benchmark 内的下一步证据应记录 wake 来源、连接 id、worker id、thread id，以及 Pending 返回时底层 TLS/BIO 的 WANT_READ/WANT_WRITE 状态。

## Native CONNECT Pending gap 迁移拆分

结论：`--trace-summary` 已补充 worker/request id、Pending/Resume poll 序号、Pending/Resume thread id，并拆分 same-thread 与 migrated Pending gap。短测显示 request future 的长 gap 大多数发生在迁移恢复路径上，但 same-thread gap 也能达到十毫秒级；因此剩余问题不能简单归因为“任务迁移”单点，需要继续定位 wake/progress 分布与 runtime 调度之间的关系。

变更内容：

- `request_pending_gap_max_sample` / `body_pending_gap_max_sample` 输出最大 Pending gap 的 stage、worker、request、poll 序号和 thread id。
- `request_pending_gap_same_thread_*` / `request_pending_gap_migrated_*` 输出 same-thread 与 migrated gap 的样本数和 p99。
- 非 `--trace-summary` 模式不记录 thread id，不影响正式吞吐对比。
- benchmark 证书生成脚本默认有效期从 1 天改为 `DAYS=${DAYS:-3650}`，避免旧本地证书导致 trace probe 全部握手失败；正式复测仍应在开始前重新执行 `tools/https_proxy_bench/generate_certs.sh`。

本轮短测使用本地 native fixture、`--insecure-proxy --insecure-origin` 绕过已过期旧证书，目的只验证 trace 字段与调度现象，不作为 20% 性能验收口径：

```bash
target/https_proxy_bench/native_proxy_fixture \
  --origin-port 39081 \
  --proxy-port 39444 \
  --response-size 1048576 \
  --origin-tls \
  --proxy-tls \
  --cert-file target/https_proxy_bench/certs/server.pem \
  --key-file target/https_proxy_bench/certs/server.key

target/release/examples/async_ylong_https_proxy_bench \
  --url https://127.0.0.1:39081/ \
  --proxy https://localhost:39444 \
  --proxy-ca-file target/https_proxy_bench/certs/ca.pem \
  --origin-ca-file target/https_proxy_bench/certs/ca.pem \
  --insecure-proxy \
  --insecure-origin \
  --requests 300 \
  --warmup-requests 64 \
  --concurrency 64 \
  --runtime-threads 16 \
  --read-buffer-size 262144 \
  --trace-summary
```

代表性输出：

| Metric | Value |
| --- | ---: |
| ylong rps | 3351.037 |
| ylong p99 | 38.282ms |
| request_pending_gap_p99_us | 32727 |
| request_pending_gap_max_us | 38372 |
| request_pending_gap_same_thread_samples | 21 |
| request_pending_gap_same_thread_p99_us | 18304 |
| request_pending_gap_migrated_samples | 275 |
| request_pending_gap_migrated_p99_us | 31469 |
| body_pending_gap_same_thread_samples | 313 |
| body_pending_gap_same_thread_p99_us | 876 |
| body_pending_gap_migrated_samples | 646 |
| body_pending_gap_migrated_p99_us | 8799 |

`runtime_threads` 对照短测：

| runtime_threads | rps | ylong p99 | request gap p99 | same-thread samples / p99 | migrated samples / p99 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 8 | 3413.436 | 45.960ms | 44.337ms | 41 / 32.235ms | 258 / 44.337ms |
| 16 | 3440.557 | 39.023ms | 30.562ms | 19 / 30.209ms | 279 / 30.076ms |
| 64 | 3498.292 | 34.760ms | 31.770ms | 10 / 11.245ms | 285 / 31.770ms |

归因更新：

- request future 仍通常只 Pending 一次，且 `request_pending_gap_p99_us` 继续贴近 `response_wait_p99_us`。
- migrated gap 样本数远高于 same-thread gap；`runtime_threads=64` 能降低 same-thread p99，但 migrated p99 仍在 30ms 量级。
- body path 的 migrated p99 也明显高于 same-thread p99，但低于 request first-byte wait；优先级仍是 request wait / wake 来源。
- 下一步应补更接近底层的 wake 证据：连接 id、底层 `poll_read` 返回 Pending 的线程、恢复 poll 线程、以及 OpenSSL WANT_READ/WANT_WRITE 状态；如果 tracefs sched 权限可用，再用 `sched_switch`/`sched_wakeup` 交叉验证。

## Native CONNECT OpenSSL WANT_READ probe

结论：`ssl_trace_preload` 已扩展 `SSL_get_error` 分类，能按最近一次 OpenSSL 操作拆分错误码 histogram。短测显示 ylong 与 libcurl 的 read-side 非成功返回几乎全部是 `SSL_ERROR_WANT_READ`，没有 write-side WANT 压力；因此 strict CONNECT 的 request first-byte 长尾更像 socket read wake / runtime resume 分布问题，而不是 TLS 写阻塞或 WANT_WRITE。

变更内容：

- preload helper 现在 hook `SSL_connect`、`SSL_shutdown`、`SSL_get_error`，并用 thread-local 最近操作将错误码归因到 read/write/connect/shutdown。
- JSON 新增 `ssl_get_error_calls`、`ssl_connect_*`、`ssl_shutdown_*`，以及 `ssl_*_error_code_hist`。histogram 下标即 OpenSSL error code；本节主要关注下标 `2 = SSL_ERROR_WANT_READ`、`3 = SSL_ERROR_WANT_WRITE`、`5 = SSL_ERROR_SYSCALL`。
- 该 helper 仍只用于诊断；正式吞吐验收不使用 LD_PRELOAD。

本轮 probe 使用重新生成的有效证书，不再需要 `--insecure-proxy` / `--insecure-origin`：

```bash
./tools/https_proxy_bench/generate_certs.sh
cc -Wall -Wextra -O2 -fPIC -shared \
  -o target/https_proxy_bench/ssl_trace_preload.so \
  tools/https_proxy_bench/ssl_trace_preload.c -ldl -pthread

target/https_proxy_bench/native_proxy_fixture \
  --origin-port 39091 \
  --proxy-port 39454 \
  --response-size 1048576 \
  --origin-tls \
  --proxy-tls \
  --cert-file target/https_proxy_bench/certs/server.pem \
  --key-file target/https_proxy_bench/certs/server.key
```

ylong trace run:

```bash
env LD_PRELOAD="$PWD/target/https_proxy_bench/ssl_trace_preload.so" \
  YLONG_SSL_TRACE_FILE="$PWD/target/https_proxy_bench/ssl_get_error_probe_20260525_021609.jsonl" \
  YLONG_SSL_TRACE_LABEL=ylong_pending_probe \
  target/release/examples/async_ylong_https_proxy_bench \
  --url https://127.0.0.1:39091/ \
  --proxy https://localhost:39454 \
  --proxy-ca-file target/https_proxy_bench/certs/ca.pem \
  --origin-ca-file target/https_proxy_bench/certs/ca.pem \
  --requests 300 \
  --warmup-requests 64 \
  --concurrency 64 \
  --runtime-threads 16 \
  --read-buffer-size 262144 \
  --trace-summary
```

代表性输出：

| Metric | ylong | libcurl |
| --- | ---: | ---: |
| rps | 3493.933 | 3635.967 |
| p99 | 36.301ms | 31.504ms |
| ylong `response_wait_p99_us` | 31033 | n/a |
| ylong `request_pending_gap_p99_us` | 29859 | n/a |
| `ssl_read_errors` | 3235 | 3885 |
| `ssl_read_error_code_hist[2]` | 3235 | 3885 |
| `ssl_write_error_code_hist[3]` | 0 | 0 |
| `ssl_connect_error_code_hist[2]` | 128 | 125 |
| `ssl_shutdown_error_code_hist[5]` | 0 | 64 |

归因更新：

- ylong request future 的长 Pending gap 仍贴近 `response_wait_p99_us`；OpenSSL 侧对应的是 read path WANT_READ，不是 write path WANT_WRITE。
- libcurl 同样以 read WANT_READ 为主，但 p99 更低，说明仅统计 OpenSSL WANT 类型还不足以解释差距；下一步需要把 WANT_READ 与具体连接/线程的底层 `poll_read` Pending/Ready 时间连接起来。
- 更有价值的下一轮证据是：在 ylong `AsyncSslStream` 或 runtime socket 层按连接记录 `poll_read -> Pending` 的线程、恢复线程、elapsed、以及该 poll 之后的 `SSL_get_error` 类型；这样才能区分“内层 origin TLS 未唤醒”“外层 proxy TLS 未唤醒”和“runtime 已唤醒但 task 未及时恢复”。

## Native CONNECT SSL retry gap 与 runtime affinity

结论：`ssl_trace_preload` 继续扩展为按 `SSL*` 记录 `SSL_get_error=WANT_READ` 到下一次同一 `SSL*` retry 的间隔，并按嵌套 `SSL_read` depth 拆分。CONNECT 下 depth 1 对应内层 origin TLS retry，depth 2 对应外层 HTTPS proxy TLS retry。短测显示 ylong 的 depth 1 与 depth 2 最大 retry gap 基本相同，说明长 gap 发生在 task 再次 poll 之前；不是单独某一层 TLS 卡住。

变更内容：

- `ssl_retry_gap_read_depth1_*`：内层 origin TLS read retry gap。
- `ssl_retry_gap_read_depth2_*`：外层 proxy TLS read retry gap。
- `ssl_retry_gap_connect_*`：TLS handshake WANT_READ retry gap。
- `ssl_retry_gap_want_write_*`：WANT_WRITE retry gap；本轮仍为 0。
- 每类 retry gap 还输出 `_same_thread_*` / `_migrated_*`，按 `SSL_get_error`
  记录线程和下一次同一 `SSL*` retry 线程的 pthread identity 拆分；这是诊断字段，
  不改变客户端行为。
- benchmark 新增 ylong-runtime-only `--runtime-affinity`，用于显式启用 `RuntimeBuilder::is_affinity(true)`；runner 会过滤该参数，不传给 libcurl。

retry-depth probe 使用 `requests=300`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=256 KiB`、`--trace-summary` 与 LD_PRELOAD：

| Metric | ylong | libcurl |
| --- | ---: | ---: |
| rps | 3324.368 | 3423.173 |
| p99 | 34.396ms | 32.500ms |
| ylong `response_wait_p99_us` | 31212 | n/a |
| ylong `request_pending_gap_p99_us` | 31129 | n/a |
| `ssl_retry_gap_read_depth1_samples` | 1364 | 2069 |
| `ssl_retry_gap_read_depth1_max_us` | 32660 | 31033 |
| `ssl_retry_gap_read_depth2_samples` | 1166 | 1896 |
| `ssl_retry_gap_read_depth2_max_us` | 32661 | 31035 |
| `ssl_retry_gap_connect_max_us` | 30246 | 30413 |
| `ssl_retry_gap_want_write_samples` | 0 | 0 |

thread-split smoke probe 使用 `requests=128`、`warmup_requests=32`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=256 KiB`、`--trace-summary` 与 LD_PRELOAD（非正式验收跑）：

| Metric | ylong | libcurl |
| --- | ---: | ---: |
| rps | 2301.940 | 2508.623 |
| p99 | 46.872ms | 47.784ms |
| ylong request pending same/migrated samples | 34 / 289 | n/a |
| ylong request pending same/migrated p99 | 7.353ms / 27.519ms | n/a |
| depth1 same/migrated samples | 470 / 530 | 792 / 0 |
| depth1 same/migrated max | 7.396ms / 15.221ms | 14.888ms / 0 |
| depth2 same/migrated samples | 449 / 378 | 646 / 0 |
| depth2 same/migrated max | 7.397ms / 14.467ms | 14.892ms / 0 |
| connect same/migrated samples | 5 / 123 | 124 / 0 |
| connect same/migrated max | 5.457ms / 34.819ms | 9.499ms / 0 |
| WANT_WRITE samples | 0 | 0 |

这个 probe 把 Rust future 层的迁移 gap 和 OpenSSL retry 边界对上了：ylong 在内层 TLS、外层 TLS、handshake retry 上都存在同一 `SSL*` 从一个 pthread 的 WANT_READ 恢复到另一个 pthread 的情况；libcurl 在同一 workload 下 migrated retry 为 0。libcurl 的 same-thread read retry max 也能到约 15ms，所以迁移不是唯一尾延迟来源，但它已经是 strict CONNECT 下最清晰的差异信号。

runtime affinity no-trace probe：

| Config | Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| rt16 affinity | 1 | 3500.995 | 3633.369 | -3.6% | 36.789ms | 35.183ms |
| rt16 affinity | 2 | 3787.105 | 3625.816 | +4.4% | 31.936ms | 29.823ms |
| rt16 affinity | 3 | 3829.476 | 3652.345 | +4.8% | 28.227ms | 28.396ms |
| rt64 affinity | 1 | 3527.070 | 3718.301 | -5.1% | 35.541ms | 33.498ms |
| rt64 affinity | 2 | 3663.503 | 3271.538 | +12.0% | 31.320ms | 34.595ms |
| rt64 affinity | 3 | 3521.068 | 3732.875 | -5.7% | 37.724ms | 27.854ms |

归因更新：

- `--runtime-affinity` 可以作为诊断/调优开关保留，但 rt16 三轮平均只约 +1.9%，rt64 三轮约持平；严格 CONNECT 仍没有 20%+ 通过样本。
- depth 1 / depth 2 retry gap 同时出现长尾，符合“任务没有及时被再次 poll”的模型；继续调 TLS read-ahead、ready-drain 或 worker affinity 都不是当前最高收益方向。
- 下一步应检查 ylong runtime 的 net driver / reactor wake 到 task enqueue 的路径，或在 client 内加入连接级 poll-read resume trace，确认 wake 是否已经到达 runtime 但 task 被排队延迟。

## Native CONNECT runtime wake 复核与参数扫测

结论：继续检查 ylong runtime 源码后，strict CONNECT 的长尾模型更清晰，但仍没有找到不伤害生产语义的 client 侧 20%+ 修复。ylong runtime 当前使用 `EPOLLET`，`AsyncSource` 只在 `WouldBlock` 后清 readiness；I/O driver 是共享 driver，由 worker 在 `run_once()` / park 路径里轮询。driver 收到 readiness 后通过当前轮询 driver 的 worker `Context` 调度被唤醒任务，因此 woken request 容易进入轮询 worker 的 local queue，而不是原 request worker。这个模型与 trace 中 migrated pending gap 占主导一致。

未采用的方案：在 TLS `poll_read` 遇到 `WANT_READ` / `WouldBlock` 时主动 `wake_by_ref()` 自唤醒。该方法可能在本地 fixture 上降低 tail gap，但在真实慢网络上会把 socket Pending 变成 busy-poll；作为生产库默认行为风险过高，本轮只记录为反例，不落代码。

`requests=300`、`warmup_requests=64`、`concurrency=64`、`response=1 MiB`、`read_buffer_size=64 KiB` 的 runtime thread sweep：

| runtime_threads | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 1173.721 | 3493.328 | -66.4% | 73.205ms | 27.984ms |
| 2 | 2007.191 | 3442.499 | -41.7% | 52.321ms | 31.267ms |
| 4 | 3140.628 | 3442.222 | -8.8% | 47.133ms | 38.260ms |
| 8 | 3294.026 | 3378.645 | -2.5% | 42.891ms | 32.252ms |
| 16 | 3499.941 | 3477.374 | +0.6% | 42.637ms | 30.665ms |
| 32 | 3754.230 | 3678.634 | +2.1% | 39.230ms | 28.177ms |
| 64 | 3765.556 | 3536.026 | +6.5% | 39.749ms | 28.144ms |

后续参数短测：

| Config | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| --- | ---: | ---: | ---: | ---: | ---: |
| rt64, 64 KiB, client-per-worker | 3050.453 | 3351.094 | -9.0% | 37.321ms | 31.462ms |
| rt64, 128 KiB | 3438.757 | 3339.047 | +3.0% | 41.035ms | 32.759ms |
| rt64, 256 KiB | 3626.903 | 3549.372 | +2.2% | 40.210ms | 29.331ms |
| rt64, 512 KiB | 3621.456 | 3462.284 | +4.6% | 37.932ms | 34.975ms |
| rt64, 512 KiB, affinity | 3051.586 | 3414.018 | -10.6% | 50.945ms | 32.869ms |

`rt64 + 512 KiB` 的 5-run 复测：

| Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 3609.601 | 3370.900 | +7.1% | 45.016ms | 48.589ms |
| 2 | 3525.214 | 3566.164 | -1.1% | 35.437ms | 33.258ms |
| 3 | 3482.064 | 3411.766 | +2.1% | 37.825ms | 35.082ms |
| 4 | 3573.181 | 3660.903 | -2.4% | 41.750ms | 28.750ms |
| 5 | 3582.002 | 3569.304 | +0.4% | 42.322ms | 28.539ms |
| avg | 3554.412 | 3515.807 | +1.1% | 40.470ms | 34.844ms |

同配置 trace 复核：

| Metric | Value |
| --- | ---: |
| ylong rps | 3601.413 |
| ylong p99 | 37.645ms |
| `response_wait_p99_us` | 29621 |
| `request_pending_gap_p99_us` | 29400 |
| `request_pending_gap_same_thread_samples / p99` | 11 / 17.940ms |
| `request_pending_gap_migrated_samples / p99` | 275 / 29.400ms |
| `body_pending_gap_migrated_samples / p99` | 323 / 9.359ms |

1 MiB response 的 concurrency sweep（`runtime_threads=concurrency`、`read_buffer_size=512 KiB`）：

| concurrency | requests | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 32 | 128 | 3207.801 | 3651.613 | -12.2% | 24.205ms | 13.771ms |
| 64 | 256 | 3362.587 | 3450.925 | -2.6% | 38.155ms | 34.648ms |
| 128 | 512 | 3542.718 | 3372.437 | +5.0% | 81.114ms | 62.163ms |
| 256 | 1024 | 3519.869 | 3507.354 | +0.4% | 189.389ms | 104.327ms |

small-response strict CONNECT（native fixture `response_size=1024`、`requests=10000`、`concurrency=64`、`runtime_threads=64`）：

| Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 104208.671 | 96336.329 | +8.2% | 3.427ms | 1.110ms |
| 2 | 108068.431 | 95826.745 | +12.8% | 3.302ms | 1.229ms |
| 3 | 101197.314 | 83441.808 | +21.3% | 3.601ms | 1.866ms |
| 4 | 100817.399 | 95193.672 | +5.9% | 4.022ms | 1.219ms |
| 5 | 106705.550 | 94046.835 | +13.5% | 3.762ms | 1.251ms |
| avg | 104199.473 | 92969.078 | +12.1% | 3.623ms | 1.335ms |

small-response concurrency sweep 单轮结果显示 `concurrency=128` 可到约 +14.7%，但 p99 继续明显高于 libcurl，仍不是 20%+ 的稳定验收口径。

ylong start-gate `wake_all()` probe（非保留实验，`requests=300`、`warmup_requests=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=256 KiB`）：

| Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 3405.773 | 3611.456 | -5.7% | 48.130ms | 28.579ms |
| 2 | 3485.353 | 3634.778 | -4.1% | 41.915ms | 27.812ms |
| avg | 3445.563 | 3623.117 | -4.9% | 45.023ms | 28.196ms |

结论：把 ylong benchmark 的 `Waiter` start gate 从逐个 `wake_one()` 改成一次 `wake_all()` 没有降低启动噪声，反而让 strict CONNECT 吞吐和 p99 都退化。该改动已撤回；后续不把 benchmark start-gate 释放方式作为完成 20%+ 的优化方向。

归因更新：

- 增大 `read_buffer_size` 能显著降低 ylong body read 次数，但 5-run 平均只 +1.1%；strict CONNECT 的主要差距仍在 request first-byte wake/resume，而不是 body drain copy。
- `client-per-worker`、runtime affinity、thread/concurrency 扫测、start-gate `wake_all()` 都没有形成稳定 20%+；继续只调 benchmark 参数不能完成严格 native CONNECT 目标。
- 下一步如果要继续推进 20%+，优先级应转到 ylong runtime I/O driver：减少共享 driver 轮询延迟，或者让 I/O wake 更接近被唤醒 task 原 worker / 全局队列策略；在 `ylong_http_client` 内继续做 TLS buffer 或 busy-poll 类修补收益低且风险高。

### Runtime scheduling patch experiments

为验证上面的 runtime 归因，使用 `cargo --config patch."https://gitcode.com/openharmony/commonlibrary_rust_ylong_runtime.git".ylong_runtime.path=...` 对本地 `target/` 下的 ylong_runtime 副本做临时 patch，并只重建 `async_ylong_https_proxy_bench`。这些改动未进入当前仓库源码，只用于判断方向。

实验 1：把 `TaskHandle::wake_by_ref()` 的 `get_scheduled(false)` 改为 `get_scheduled(true)`，让 I/O wake 使用 runtime 现有 LIFO slot。

- 首次运行在 `async_pool.rs:190` 触发 `RefCell already borrowed`。原因是 `Worker::get_task()` 中的 `lifo.borrow_mut()` 生命周期覆盖了后续 `scheduler.dequeue()`，而 `dequeue()` 内可能 `driver.run_once()` 并再次尝试写同一 LIFO slot。
- 修复实验版 borrow 范围后可以运行，但 3-run 平均 ylong 3593.925 rps、libcurl 3613.485 rps，提升 -0.5%；ylong/libcurl p99 平均 39.806ms / 26.567ms。
- 结论：naive all-wake LIFO 不是有效方向；即使修正 borrow 范围，strict CONNECT 仍没有接近 20%+。

实验 2：保持 `wake_by_ref(false)`，但在 `MultiThreadScheduler::enqueue_under_ctx()` 中把 `!lifo` wake 放入 global queue，避免 I/O driver 所在 worker 把 woken task 放到自己的 local queue。

| Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 3480.923 | 3640.865 | -4.4% | 34.456ms | 26.887ms |
| 2 | 3526.079 | 3694.172 | -4.6% | 34.956ms | 27.980ms |
| 3 | 3471.213 | 3517.205 | -1.3% | 37.858ms | 33.831ms |
| avg | 3492.738 | 3617.414 | -3.4% | 35.757ms | 29.566ms |

结论：简单改成 global queue 会降低吞吐；global/local 队列策略不能粗暴替换。

实验 3：保持默认 wake 入队策略，但让 `Inner::periodic_check()` 只在当前 worker local queue 和 LIFO slot 都为空时才 `driver.run_once()`，避免忙 worker 轮询共享 I/O driver 后把 I/O wake 压到已有本地工作之后。

| Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 3472.255 | 3613.239 | -3.9% | 48.012ms | 27.982ms |
| 2 | 3690.404 | 3541.369 | +4.2% | 41.112ms | 26.754ms |
| 3 | 3478.011 | 3234.048 | +7.5% | 39.902ms | 42.253ms |
| avg | 3546.890 | 3462.885 | +2.4% | 43.009ms | 32.330ms |

结论：该实验受 libcurl 第 3 轮慢跑影响，不能视为稳定收益；ylong p99 仍明显偏高。继续推进需要更细粒度的 runtime 证据，例如区分 readiness 由 periodic driver 还是 park/dequeue driver 发现，并记录 woken task 在 local/global/lifo 队列中的实际排队时间。

## HTTP/1 empty-body encode cleanup

代码复核时发现 async HTTP/1 路径对 GET / empty body 请求仍会进入 `TextBody` encoder，并执行一次立即完成的空 body read。该路径不会解释 strict CONNECT 的 30ms 级 response first-byte wake gap，但属于高频请求上的无效 CPU 工作。

本轮将 `Body::is_empty()` 从 HTTP/2-only helper 改为 async request body 通用 helper，并在 HTTP/1 request body 非 chunked 且已为空时直接跳过 body encoder。保留 chunked 空 body 原路径，避免丢失 `0\r\n\r\n` 终止块。

验证：

- `rustfmt --check ylong_http_client/src/async_impl/request.rs ylong_http_client/src/async_impl/conn/http1.rs`
- `cargo check -p ylong_http_client --features "async http1_1 ylong_base c_openssl_3_0"`
- `cargo check -p ylong_http_client --features "async http1_1 tokio_base c_openssl_3_0"`
- `cargo test -p ylong_http_client --test sdv_async_https_proxy --features "async http1_1 ylong_base c_openssl_3_0"`: 11 passed
- `cargo test -p ylong_http_client --test sdv_sync_https_proxy --features "sync http1_1 tokio_base c_openssl_3_0"`: 11 passed

## Content-Length body decoder cleanup

继续复核 response body 热路径时，`Content-Length` 固定长度 body 仍通过 `TextBodyDecoder` 在每次读取时更新剩余长度。该路径不是 strict CONNECT 的首字节等待主因，但在 1 MiB body drain 中属于每次 body read 都会执行的 CPU 工作。

本轮将 async/sync `HttpBody::Text` 改为直接维护 `remaining: u64`：

- 保留原有行为：超过 `Content-Length` 的 pre-buffer 或 stream bytes 仍返回 `BodyDecode` 错误，并关闭对应 IO。
- chunked 与 until-close body 不变。
- async HTTP/2/HTTP/3 的“body 长度已够但 stream 还未 end-stream”行为不变，仍保留 IO 等待 fin。

native CONNECT 单轮 smoke（非正式验收，`requests=300`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=256 KiB`）：

| Client | rps | p99 | body reads | errors |
| --- | ---: | ---: | ---: | ---: |
| ylong async-ylong | 3714.463 | 35.030ms | 2827 | 0 |
| libcurl | 3757.562 | 29.106ms | 19200 | 0 |

`bench_summary.formal_pass=false`，单轮提升 -1.147%。结论：该变更只减少固定长度 body decode 的通用 CPU 分支，不改变 strict CONNECT 的主瓶颈；20%+ 目标仍需 runtime I/O wake / queue 侧继续推进。

验证：

- `rustfmt --check ylong_http_client/src/async_impl/http_body.rs ylong_http_client/src/sync_impl/http_body.rs`
- `cargo check -p ylong_http_client --features "async http1_1 ylong_base c_openssl_3_0"`
- `cargo check -p ylong_http_client --features "sync http1_1 tokio_base c_openssl_3_0"`
- `cargo test -p ylong_http_client --test sdv_async_https_proxy --features "async http1_1 ylong_base c_openssl_3_0"`: 11 passed
- `cargo test -p ylong_http_client --test sdv_sync_https_proxy --features "sync http1_1 tokio_base c_openssl_3_0"`: 11 passed
- `cargo test -p ylong_http_client --features "async http1_1 ylong_base" ut_http_body_text`: passed
- `cargo test -p ylong_http_client --features "sync http1_1 tokio_base" ut_http_body_text`: passed

## CONNECT request flush and transport audit

继续复核 proxy transport 后发现 async/sync CONNECT helper 在 `write_all()` 代理 CONNECT 请求后立即读取响应，而普通 HTTP/1 request path 会在请求写完后 flush。对 `TcpStream` / 当前 OpenSSL BIO 路径通常不改变系统调用边界，但对泛型 `AsyncWrite` / `Write` transport 来说，读响应前 flush CONNECT request 是正确语义。

本轮将 async/sync `tunnel()` 改为：

- `write_all(CONNECT ...)` 后调用 `flush()`，再读取 proxy 响应。
- 增加 test-only flush-gated stream：只有收到 `flush()` 后才返回 `HTTP/1.1 200`，覆盖 async 和 sync tunnel。
- unit test 中用现有 dev-dependency `openssl as _` 触发 C-OpenSSL test binary 链接；不新增依赖。

同轮 transport / dispatch audit：

- async/sync TCP connect 后已经设置 `TCP_NODELAY`，strict CONNECT 的 response first-byte gap 不是 Nagle 延迟。
- OpenSSL async wrapper 将 inner `Poll::Pending` 映射为 `WouldBlock`，BIO 将 `WouldBlock` 标为 retry，`AsyncSslStream` 再转回 `Poll::Pending`；本轮随后补充了 BIO stale error cleanup，见下一节。
- HTTP/1 pool dispatch 不额外 spawn request I/O task；`Http1Dispatcher` 只用 `occupied` 管理独占 handle，实际 request write/read 在调用方 future 中完成。严格 CONNECT trace 中的 request pending gap 不来自 client 侧 HTTP/1 dispatcher channel。
- Linux-only socket hints（例如 `TCP_QUICKACK`）不直接针对已观测到的 wake-to-repoll / queue delay，且会引入平台特化行为；本轮不加入。

native CONNECT 单轮 smoke（非正式验收，`requests=300`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=256 KiB`，日志 `target/https_proxy_bench/connect_flush_smoke_current.log`）：

| Client | rps | p99 | body reads | errors |
| --- | ---: | ---: | ---: | ---: |
| ylong async-ylong | 3715.797 | 32.889ms | 2750 | 0 |
| libcurl | 3538.612 | 28.180ms | 19200 | 0 |

`bench_summary.formal_pass=false`，单轮提升 +5.007%。结论：flush 是正确的 CONNECT 边界修复，但单轮仍远低于 20%+；严格 native CONNECT 的剩余主因仍指向 runtime I/O wake / queue / worker 恢复延迟。

验证：

- `rustfmt --check ylong_http_client/src/async_impl/proxy.rs ylong_http_client/src/sync_impl/proxy.rs`
- `cargo test -p ylong_http_client --lib --features "async http1_1 ylong_base c_openssl_3_0" ut_async_tunnel_flushes_connect_request`: passed
- `cargo test -p ylong_http_client --lib --features "sync http1_1 tokio_base c_openssl_3_0" ut_sync_tunnel_flushes_connect_request`: passed
- `cargo check -p ylong_http_client --features "async http1_1 ylong_base c_openssl_3_0"`: passed
- `cargo check -p ylong_http_client --features "sync http1_1 tokio_base c_openssl_3_0"`: passed
- `cargo test -p ylong_http_client --test sdv_async_https_proxy --features "async http1_1 ylong_base c_openssl_3_0"`: 11 passed
- `cargo test -p ylong_http_client --test sdv_sync_https_proxy --features "sync http1_1 tokio_base c_openssl_3_0"`: 11 passed

## OpenSSL BIO stale error cleanup

复核 async OpenSSL wrapper 时发现 custom BIO 的 `StreamState::error` 会在底层 I/O 返回错误时保存给后续 `SSL_get_error()` 使用，但成功的 `BIO_read` / `BIO_write` / `BIO_CTRL_FLUSH` 没有显式清理旧错误。该状态一般会在 `SSL_get_error()` 中被 `take()` 掉，但成功路径保留 stale error 不符合 BIO 当前操作语义，也可能让后续 OpenSSL 错误分类读取到过期的 `WouldBlock`。

本轮修复：

- `BIO_read` / `BIO_write` 成功时清空 `state.error`。
- `BIO_CTRL_FLUSH` 成功时清空 `state.error`。
- 新增 `ut_bread_clears_stale_error`、`ut_bwrite_clears_stale_error`、`ut_ctrl_flush_clears_stale_error`，覆盖 seeded stale `WouldBlock` 在成功操作后被清除。

strict CONNECT 两轮 smoke（非正式验收，`requests=300`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=256 KiB`）：

| Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 3261.624 | 3603.863 | -9.5% | 43.598ms | 29.074ms |
| 2 | 3739.211 | 3266.871 | +14.5% | 37.896ms | 42.742ms |
| avg | 3500.417 | 3435.367 | +2.5% | 40.747ms | 35.908ms |

`bench_summary.formal_pass=false`，2 轮中 0 轮达到 +20%。结论：该补丁是 OpenSSL BIO 状态正确性修复，但不能单独完成 strict native CONNECT 的 20%+ 性能目标。

验证：

- `rustfmt --check ylong_http_client/src/util/c_openssl/bio.rs`
- `cargo test -p ylong_http_client --lib --features "async http1_1 ylong_base c_openssl_3_0" clears_stale_error`: 3 passed
- `cargo check -p ylong_http_client --example async_ylong_https_proxy_bench --features "async http1_1 ylong_base c_openssl_3_0"`: passed

## ylong runtime dependency and multi-instance probe

复核日期：2026-05-25。`Cargo.lock` 中 `ylong_runtime` / `ylong_io` / `ylong_runtime_macros` 固定在 `c9bb0e3a20554995847983bca558f6c6c36ebad5`；`git ls-remote` 显示 upstream `HEAD`、`master`、`OpenHarmony-6.1-Release`、`OpenHarmony-7.0-Beta1` 当前都指向同一提交，因此没有可直接升级的更新版本。

当前 `ylong_base` feature tree 只启用 `net`、`sync`、`fs`、`macros`、`time` 及其传递依赖，没有启用 `multi_instance_runtime`。源码 audit 显示 benchmark 当前用 `RuntimeBuilder::build_global()` 配置全局 runtime，并通过 `ylong_runtime::spawn()` 投递 worker；`RuntimeBuilder::build()` 与 `Runtime::spawn()` 只有在 runtime crate 的 `multi_instance_runtime` feature 下可用。

为了验证“独立 runtime 实例是否降低 strict CONNECT wake / queue delay”，临时增加内部 feature 暴露 `ylong_runtime/multi_instance_runtime`，并只让 `async_ylong_https_proxy_bench` 使用 `RuntimeBuilder::build()` + `Runtime::spawn()`。该实验编译通过，但 native CONNECT 单轮 smoke 退化：

| Client | rps | p99 | body reads | errors |
| --- | ---: | ---: | ---: | ---: |
| ylong async-ylong multi-instance probe | 3551.981 | 60.428ms | 2716 | 0 |
| libcurl | 3596.562 | 31.494ms | 19200 | 0 |

`bench_summary.formal_pass=false`，单轮提升 -1.240%。实验变更已撤回；继续把严格 native CONNECT 的瓶颈归因保持在 ylong_runtime I/O wake / worker queue / task resume 行为，而不是 repo 内 benchmark 使用全局 runtime 这一点。

验证：

- `git ls-remote https://gitcode.com/openharmony/commonlibrary_rust_ylong_runtime.git 'HEAD' 'refs/heads/*'`
- `cargo tree -p ylong_http_client --features "async http1_1 ylong_base c_openssl_3_0" -i ylong_runtime --edges features`
- probe 期间：`cargo check -p ylong_http_client --example async_ylong_https_proxy_bench --features "async http1_1 ylong_base __ylong_multi_instance_runtime c_openssl_3_0"`: passed
- 撤回后：`cargo check -p ylong_http_client --example async_ylong_https_proxy_bench --features "async http1_1 ylong_base c_openssl_3_0"`: passed

## HTTP/1 head buffer probe

为排除每请求 16 KiB HTTP/1 header/status 临时 buffer 的分配与 pre-read copy 对 strict CONNECT 的影响，临时将 `TEMP_BUF_SIZE` 从 16 KiB 改为 4 KiB，只在 `target/https_proxy_bench/temp_buf_4k_probe_current.log` 中做对比，随后恢复 16 KiB。该实验没有进入最终源码。

`response=1 MiB`、`requests=300`、`warmup_requests=64`、`concurrency=64`、`runtime_threads=64`、`read_buffer_size=512 KiB`：

| Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 3691.166 | 3403.367 | +8.5% | 40.681ms | 37.031ms |
| 2 | 3598.222 | 3463.723 | +3.9% | 42.493ms | 34.139ms |
| 3 | 3455.968 | 3686.274 | -6.2% | 48.215ms | 26.997ms |
| avg | 3581.785 | 3517.788 | +1.8% | 43.796ms | 32.722ms |

结论：缩小 header/status 临时 buffer 不能稳定改善 strict CONNECT，也没有改善 ylong p99。继续沿这个方向调 buffer 常量收益不足，因此保留原 16 KiB。

## Runtime queue-delay trace probe

为验证 “I/O wake 已经发生，但被唤醒 task 在 runtime 队列中等待过久” 的假设，基于 Cargo cache 中的 `ylong_runtime` 建立了一个新的临时 patch 副本 `target/ylong_runtime_trace`。该 patch 只用于诊断，没有进入当前仓库源码：

- 在 task header 上记录 enqueue 时间、queue kind（local/global/lifo）和 enqueue worker。
- 在 worker 即将执行 task 前记录 dequeue worker，并输出 local/global/lifo 的样本数、平均排队时间、最大排队时间、same/migrated worker 拆分和粗粒度 histogram。
- instrumentation 使用 diagnostic-only atomic fields；enqueue timestamp 用 Release store，dequeue 侧用 Acquire swap，统计计数使用 Relaxed。

rt16 probe（`requests=300`、`warmup_requests=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=256 KiB`、`--trace-summary`，日志 `target/https_proxy_bench/runtime_queue_trace_probe_current.log`）：

| Metric | Value |
| --- | ---: |
| ylong rps | 3309.966 |
| ylong p99 | 48.239ms |
| `response_wait_p99_us` | 39031 |
| `request_pending_gap_p99_us` | 38964 |
| request pending same/migrated samples | 13 / 263 |
| runtime local queue samples | 1128 |
| runtime local queue avg/max | 1.785ms / 34.691ms |
| runtime local same/migrated worker | 831 / 297 |
| runtime local hist `<10us,<100us,<1ms,<10ms,<50ms,>=50ms` | `[565,133,177,173,80,0]` |
| runtime global queue avg/max | 0.421ms / 10.140ms |
| runtime lifo samples | 1 |

rt64 + 512 KiB probe（`requests=300`、`warmup_requests=64`、`concurrency=64`、`runtime_threads=64`、`read_buffer_size=512 KiB`、`--trace-summary`，日志 `target/https_proxy_bench/runtime_queue_trace_rt64_rb512k_current.log`）：

| Metric | Value |
| --- | ---: |
| ylong rps | 3519.222 |
| ylong p99 | 42.587ms |
| `response_wait_p99_us` | 23983 |
| `request_pending_gap_p99_us` | 23532 |
| request pending same/migrated samples | 8 / 284 |
| runtime local queue samples | 1039 |
| runtime local queue avg/max | 0.716ms / 15.981ms |
| runtime local same/migrated worker | 687 / 352 |
| runtime local hist `<10us,<100us,<1ms,<10ms,<50ms,>=50ms` | `[576,138,174,136,15,0]` |
| runtime global queue avg/max | 0.261ms / 1.641ms |
| runtime lifo samples | 6 |

结论：

- runtime local queue delay 与 request future 的 Pending gap 同量级，rt16 最大 local queue delay 34.691ms，rt64/512 KiB 最大 local queue delay 15.981ms；这比 client 内 header/body 编码、pool lookup 或 TLS 配置开销更接近 strict CONNECT 的 p99 差距。
- local queue 的 migrated-worker 样本数量很高，说明 I/O wake 经常把 task 放到发现 readiness 的 worker 本地队列，随后由不同 worker 恢复执行；这与 OpenSSL retry same/migrated trace 和 request trace 一致。
- coarse runtime patch（all-wake LIFO、non-LIFO global、skip busy periodic driver）已经验证为负向或不稳定；下一步不应继续在 `ylong_http_client` 内做 busy-poll 或 buffer 常量调参，而应在 ylong_runtime 侧设计更细的 I/O wake queue 策略，例如只针对 net I/O wake 记录原 worker/affinity，或让 I/O wake 避免排在忙 worker local queue 的已有工作之后。

## Runtime I/O-wake scheduling experiments

在 `target/ylong_runtime_trace` 的诊断 patch 上继续加两个 env-gated 实验，只影响 `ScheduleIO::wake0()` 调用 `waker.wake_by_ref()` 时产生的 net I/O wake，不影响普通 channel/timer/join wake：

- `YLONG_RUNTIME_IO_LIFO=1`：只让 net I/O wake 走现有 LIFO slot。
- `YLONG_RUNTIME_IO_GLOBAL=1`：只让 net I/O wake 进入 global queue。

I/O-LIFO 首次运行仍触发 `RefCell already borrowed`，位置为 `target/ylong_runtime_trace/ylong_runtime/src/executor/async_pool.rs:191`。根因是当前 upstream `Worker::get_task()` 的 `lifo.borrow_mut()` 生命周期覆盖了后续 `scheduler.dequeue()`；而 `dequeue()` 内可能 `driver.run_once()` 并重新进入 `enqueue_under_ctx()` 写同一个 LIFO slot。诊断 patch 中把 `Worker::get_task()` 的 LIFO borrow 收窄后，I/O-LIFO 可运行。

I/O-LIFO 单轮 trace（`runtime_threads=64`、`read_buffer_size=512 KiB`，日志 `target/https_proxy_bench/runtime_io_lifo_trace_rt64_rb512k_current.log`）：

| Metric | Value |
| --- | ---: |
| ylong rps | 3579.350 |
| ylong p99 | 37.330ms |
| `response_wait_p99_us` | 25990 |
| `request_pending_gap_p99_us` | 25915 |
| request pending same/migrated samples | 4 / 269 |
| runtime local queue avg/max | 1.747ms / 15.634ms |
| runtime local same/migrated worker | 89 / 340 |
| runtime lifo samples avg/max | 441 / 0.631ms / 20.080ms |

I/O-LIFO 5-run paired（日志 `target/https_proxy_bench/runtime_io_lifo_rt64_rb512k_repeat_current.log`）：

| Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 3580.301 | 3200.649 | +11.9% | 39.901ms | 36.351ms |
| 2 | 3554.438 | 3725.551 | -4.6% | 37.441ms | 30.721ms |
| 3 | 3577.384 | 3619.516 | -1.2% | 33.390ms | 29.673ms |
| 4 | 3759.435 | 3328.156 | +13.0% | 40.470ms | 32.273ms |
| 5 | 3546.388 | 3695.947 | -4.0% | 37.832ms | 27.214ms |
| avg | 3603.589 | 3513.964 | +2.6% | 37.807ms | 31.246ms |

I/O-global 单轮 trace（日志 `target/https_proxy_bench/runtime_io_global_trace_rt64_rb512k_current.log`）：

| Metric | Value |
| --- | ---: |
| ylong rps | 3634.357 |
| ylong p99 | 38.424ms |
| `response_wait_p99_us` | 28538 |
| `request_pending_gap_p99_us` | 28124 |
| request pending same/migrated samples | 10 / 274 |
| runtime local queue samples | 0 |
| runtime global queue avg/max | 0.850ms / 7.377ms |
| runtime global same/migrated/unknown worker | 144 / 594 / 183 |

I/O-global 5-run paired（日志 `target/https_proxy_bench/runtime_io_global_rt64_rb512k_repeat_current.log`）：

| Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 3499.350 | 3636.275 | -3.8% | 43.716ms | 30.847ms |
| 2 | 3587.925 | 3672.105 | -2.3% | 38.633ms | 31.595ms |
| 3 | 3563.807 | 3691.444 | -3.5% | 35.184ms | 25.969ms |
| 4 | 3642.330 | 3187.048 | +14.3% | 35.767ms | 34.589ms |
| 5 | 3346.765 | 3495.933 | -4.3% | 41.898ms | 58.817ms |
| avg | 3528.035 | 3536.561 | -0.2% | 39.040ms | 36.363ms |

I/O-global + global-first dequeue probe（直接使用 `target/ylong_runtime_trace` 诊断 runtime build，`requests=300`、`warmup_requests=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=256 KiB`）：

| Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | runtime global avg/max |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 3529.016 | 3162.755 | +11.6% | 33.660ms | 32.696ms | 1.252ms / 11.113ms |
| 2 | 3486.220 | 3540.742 | -1.5% | 30.724ms | 33.368ms | 1.793ms / 32.386ms |
| avg | 3507.618 | 3351.749 | +4.7% | 32.192ms | 33.032ms | - |

该实验在 runtime 诊断副本中增加 `YLONG_RUNTIME_GLOBAL_FIRST=1`，让 worker 在 dequeue 开始时优先从 global queue 取任务；运行时同时设置 `YLONG_RUNTIME_IO_GLOBAL=1`，因此 net I/O wake 不再进入 local queue。结论仍为负向：即使把 I/O wake 统一放入 global queue 并优先取 global，strict CONNECT 单轮结果也没有稳定达到 +20%，第二轮已回落到 -1.5%。该改动没有进入当前仓库源码。

I/O spill-busy probe（直接使用 `target/ylong_runtime_trace` 诊断 runtime build，`requests=300`、`warmup_requests=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=256 KiB`）：

| Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | runtime local avg/max | runtime global avg/max |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 3236.725 | 3621.832 | -10.6% | 37.539ms | 32.201ms | 0.005ms / 0.458ms | 1.811ms / 18.300ms |
| 2 | 3411.456 | 3476.246 | -1.9% | 35.970ms | 29.797ms | 0.014ms / 1.659ms | 2.230ms / 27.161ms |
| avg | 3324.091 | 3549.039 | -6.3% | 36.755ms | 30.999ms | - | - |

该实验在 runtime 诊断副本中增加 `YLONG_RUNTIME_IO_SPILL_BUSY=1`：只有当 net I/O wake 发生在当前 worker local queue 非空时，才把被唤醒 task spill 到 global queue；local queue 空时仍走原本 local 入队路径。结论仍为负向：conditional spill 降低了 local queue 延迟，但 global queue avg/max 上升到 1.8-2.2ms / 18-27ms，吞吐没有接近 +20%。该改动没有进入当前仓库源码。

driver-first polling probe（直接使用 `target/ylong_runtime_trace` 诊断 runtime build，`requests=300`、`warmup_requests=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=256 KiB`）：

| Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | runtime local avg/max | runtime global avg/max |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 3500.677 | 3626.605 | -3.5% | 43.446ms | 32.502ms | 1.481ms / 22.261ms | 0.518ms / 1.934ms |
| 2 | 3467.108 | 3312.063 | +4.7% | 45.392ms | 51.571ms | 1.114ms / 32.434ms | 0.461ms / 1.539ms |
| avg | 3483.893 | 3469.334 | +0.4% | 44.419ms | 42.037ms | - | - |

该实验在 runtime 诊断副本中增加 `YLONG_RUNTIME_DRIVER_FIRST=1`：每轮 worker loop 在取下一个 task 前都尝试 `Driver::run_once()`，用于验证“忙 worker 轮询 I/O driver 频率不足”是否是 strict CONNECT 主因。结论仍为负向：吞吐平均只 +0.4%，且 ylong p99 仍在 44ms 量级；更频繁地轮询共享 driver 不能单独解决 wake-to-resume 长尾，额外的 `try_lock` / zero-timeout poll 也可能抵消收益。该改动没有进入当前仓库源码。

driver-source attribution probe（直接使用 `target/ylong_runtime_trace` 诊断 runtime build，`requests=300`、`warmup_requests=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=256 KiB`、`--trace-summary`）：

| Source | samples | avg queue delay | max queue delay | migrated samples |
| --- | ---: | ---: | ---: | ---: |
| unknown | 192 | 15.628ms | 46.975ms | 0 |
| periodic driver | 37 | 0.679ms | 10.284ms | 6 |
| dequeue driver | 909 | 1.785ms | 44.768ms | 279 |
| park driver | 225 | 0.044ms | 2.922ms | 37 |

同轮 ylong `rps=3596.569`、`p99=52.252ms`、`request_pending_gap_p99_us=42093`。结论：长队列延迟主要不是 park path；最值得继续看的 runtime 路径是 worker 在 `dequeue()` 中发现无任务后 `Driver::run_once()`，该路径唤醒的任务仍可能在 local queue/steal 之后出现 44ms 级 delay。`unknown` 主要对应 benchmark task 初始 global enqueue，不能直接解释 CONNECT request wake。

driver-duration attribution probe（同样使用诊断 runtime，额外记录每次 `IoDriver::drive()` 从 poll 到 dispatch 完成的时间和 dispatch event 数）：

| Driver source | samples | avg driver time | max driver time | avg events | max events |
| --- | ---: | ---: | ---: | ---: | ---: |
| periodic | 51 | 3.039us | 39us | 2.980 | 24 |
| dequeue | 937 | 7.721us | 3.106ms | 3.300 | 52 |
| park | 514 | 49.790us | 11.746ms | 1.601 | 19 |

同轮 source queue delay 仍显示 `source_dequeue` avg/max 为 `1.408ms / 31.175ms`，而 `driver_dequeue` avg/max 只有 `7.721us / 3.106ms`。结论：strict CONNECT 的长尾不是由 `dequeue()` 路径一次 epoll/dispatch 调用本身占用几十毫秒造成；主要延迟发生在 I/O wake 已经入队之后的 task 恢复/队列竞争阶段。

dequeue-after-driver probe（诊断 runtime 中增加 `YLONG_RUNTIME_DEQUEUE_AFTER_DRIVER=1`：`dequeue()` 路径 `Driver::run_once()` 后立刻尝试从队列取出刚唤醒的 task，而不是先返回外层 worker loop）：

| Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| trace | 3645.205 | 3591.868 | +1.5% | 39.655ms | 29.449ms |
| no-trace 1 | 3444.465 | 3441.867 | +0.1% | 39.896ms | 34.944ms |
| no-trace 2 | 3451.689 | 3565.147 | -3.2% | 47.631ms | 25.498ms |
| no-trace avg | 3448.077 | 3503.507 | -1.6% | 43.764ms | 30.221ms |

该 probe 能在 trace 跑中把 `request_pending_gap_p99_us` 从 source-attribution 跑的 42.093ms 降到 34.362ms，但 no-trace 吞吐仍未改善，且 p99 仍弱于 libcurl。结论：`dequeue()` 后即时取队列可以作为 runtime 侧局部优化候选继续评估，但它不是当前严格 CONNECT 20%+ 的充分修复；还需要区分 driver 批量 dispatch 时间、local queue steal、global 初始任务与 TLS read retry 的组合影响。该改动没有进入当前仓库源码。

dequeue-only I/O LIFO probe（诊断 runtime 中增加 `YLONG_RUNTIME_DEQUEUE_IO_LIFO=1`：仅当 net I/O wake 发生在 `dequeue()` driver source 内时才走 LIFO；park/periodic source 保持默认入队）：

| Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 3450.260 | 3604.859 | -4.3% | 40.787ms | 32.279ms |
| 2 | 3331.816 | 3474.756 | -4.1% | 45.854ms | 31.609ms |
| avg | 3391.038 | 3539.808 | -4.2% | 43.321ms | 31.944ms |

结论：只把 `dequeue()` source 的 I/O wake 放入 LIFO 仍然退化吞吐和 p99；前面的 all-I/O LIFO 负/不稳定结果不是由 park path 混入造成的。该改动没有进入当前仓库源码。

optimistic-readiness probe（诊断 runtime 中增加 `YLONG_RUNTIME_OPTIMISTIC_READ=1`：`AsyncSource::poll_io()` 在 `Interest::READABLE` 路径先尝试一次 socket read，只有 `WouldBlock` 时才进入原 `poll_readiness()` 流程）：

| Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 3548.402 | 3515.804 | +0.9% | 43.797ms | 27.881ms |
| 2 | 3397.840 | 3680.891 | -7.7% | 70.085ms | 29.172ms |
| avg | 3473.121 | 3598.348 | -3.5% | 56.941ms | 28.527ms |

该 probe 验证了 “readiness bit 为空但 socket 已有响应字节” 这个客户端侧绕过共享 driver 的可能性。结果为负向：吞吐平均退化，ylong p99 仍明显弱于 libcurl，第二轮尾延迟反而扩大。说明严格 CONNECT 的主要长尾不是简单地在 response first-byte 首次 poll 时漏掉了一次可立即成功的 socket read；后续恢复仍需要依赖 runtime driver wake / task queue。该改动没有进入当前仓库源码。

TCP_QUICKACK probe（临时在 async connector 的 TCP connect 后保留 `TCP_NODELAY`，并额外 best-effort 设置 Linux `TCP_QUICKACK`，随后复测 strict CONNECT；该改动已撤回）：

| Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 3571.814 | 3689.674 | -3.2% | 37.710ms | 29.184ms |
| 2 | 3582.058 | 3481.894 | +2.9% | 40.595ms | 36.954ms |
| avg | 3576.936 | 3585.784 | -0.2% | 39.153ms | 33.069ms |

结论：`TCP_QUICKACK` 不是当前严格 CONNECT 的有效生产补丁。它没有稳定改善吞吐或 p99，也与此前 “`TCP_NODELAY` 已开启，response first-byte gap 不是 Nagle 延迟” 的结论一致。该改动没有进入当前仓库源码。

I/O global-front probe（诊断 runtime 中增加 `YLONG_RUNTIME_IO_GLOBAL_FRONT=1`：net I/O wake 进入 global queue 时插到下一次 `pop_back()` 会优先取到的位置；第二组额外叠加 `YLONG_RUNTIME_GLOBAL_FIRST=1`，用于排除 worker 不及时检查 global queue 的影响）：

| Mode | Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| global-front | 1 | 3421.816 | 3534.735 | -3.2% | 35.065ms | 26.284ms |
| global-front + global-first | 1 | 3530.155 | 3316.420 | +6.4% | 42.224ms | 31.782ms |
| global-front + global-first | 2 | 3429.697 | 3456.978 | -0.8% | 39.439ms | 29.589ms |
| global-front + global-first avg | - | 3479.926 | 3386.699 | +2.8% | 40.832ms | 30.686ms |

结论：优先插入 global queue 不是当前严格 CONNECT 的有效 runtime 补丁。单独 global-front 退化吞吐；叠加 global-first 后平均仍只有 +2.8%，且 ylong p99 仍明显弱于 libcurl，不能接近 20%+ 目标。该改动没有进入当前仓库源码。

sync strict CONNECT audit（复测同步客户端是否能绕过 async runtime queue 长尾，并临时验证 sync CONNECT 内层 origin TLS 64 KiB read-ahead）：

| Mode | Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | ylong avg body read |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| sync baseline | 1 | 3155.144 | 3637.157 | -13.3% | 15.042ms | 28.759ms | 16 KiB |
| sync baseline | 2 | 3147.757 | 3330.558 | -5.5% | 12.799ms | 32.329ms | 16 KiB |
| sync baseline avg | - | 3151.451 | 3483.858 | -9.5% | 13.921ms | 30.544ms | 16 KiB |
| sync origin read-ahead probe | 1 | 3137.641 | 3322.112 | -5.6% | 15.496ms | 45.301ms | 16 KiB |
| sync corrected start-gate | 1 | 3023.427 | 3540.199 | -14.6% | 19.542ms | 29.999ms | 16 KiB |

补充修正：首次 sync worker-elapsed 输出显示 wall time 与 worker max 不一致，因此将 sync benchmark 的启动门从单 `Barrier` 调整为 ready/start 双门，并用共享 `Instant` 统计 worker elapsed。修正后 sync `elapsed_ms=99.225ms`、`worker_elapsed_us_max=95.108ms`，与 libcurl `elapsed_ms=84.741ms`、`worker_elapsed_us_max=84.129ms` 同量级，说明该指标现在可用于横向比较。

结论：同步客户端能避开 async runtime 的 request Pending 长尾，因此 p99 低于 libcurl；但修正启动门后吞吐仍低于 libcurl，不能作为 20%+ 完成口径。sync CONNECT 内层 origin TLS 64 KiB read-ahead 没有增加 `SSL_read` 返回体量（平均仍为 16 KiB），也没有改善吞吐；该改动已撤回。

补充 sync body bounded drain probe（临时让 sync `Content-Length` body 每次 `data()` 最多连续读 8 次，日志 `target/https_proxy_bench/sync_body_ready_drain_probe_current.log`）：

| Mode | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | ylong body reads | ylong avg body read |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| sync body drain 8 | 3036.529 | 3666.048 | -17.2% | 23.449ms | 27.990ms | 2400 | 128 KiB |

该 probe 证明 sync 路径可以通过应用层循环把 body read 次数从 19200 降到 2400，但吞吐进一步退化；在 blocking `Body::data()` 中强行合并多次底层 read 还会改变流式读取的返回节奏。因此该实验不保留，strict CONNECT 的完成路径仍不应转向 sync body coalescing。

async start-gate correction（同样将 async benchmark 的启动门调整为 ready/start 双门，并用共享 `Instant` 统计 wall time 与 worker elapsed；tokio/ylong 两个 cfg 路径均已编译检查）：

| Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | ylong worker max | libcurl worker max |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| corrected gate rt16 | 3532.732 | 3468.368 | +1.9% | 45.076ms | 26.989ms | 84.838ms | 85.686ms |
| corrected gate rt64 | 3544.151 | 3486.588 | +1.7% | 40.069ms | 29.591ms | 84.575ms | 85.346ms |

结论：修正后 async ylong 的 wall time 与 worker max 对齐，说明 benchmark wall/worker 计时口径已经一致；该修正没有改变性能结论，strict CONNECT 仍远低于 +20%，且 ylong p99 仍明显弱于 libcurl。修正计时后重新抽测 `runtime_threads=64` 也只有 +1.7%，不支持把运行时线程数作为完成 20%+ 的配置解。

结论：

- 仅对 net I/O wake 使用 LIFO、global FIFO 或 global-front 都不能稳定达到 +20%，也没有可靠改善 ylong p99。
- I/O-global、global-first、global-front、spill-busy、driver-first、dequeue-after-driver、dequeue-only LIFO、optimistic-readiness、TCP_QUICKACK 和 sync origin read-ahead 证明 local queue、global queue 优先级、readiness pre-check、TCP ACK hint 或简单 OpenSSL read-ahead 都不是唯一问题：即使 local queue 样本归零或接近归零，或者每个 worker loop 都主动尝试轮询 driver，request future Pending gap / runtime queue delay 仍可到数十毫秒量级。driver-duration trace 进一步排除 `dequeue()` driver call 本身是几十毫秒瓶颈，说明全局队列竞争、worker steal/恢复以及 TLS/driver 交互仍需要更细的 runtime 级设计。
- `Worker::get_task()` 的 LIFO borrow 范围问题是一个真实 runtime 隐患；如果 ylong_runtime 侧继续做 I/O-wake 策略，先收窄该 borrow 是必要前置修复。

## HTTP/1 pool locality audit

为复核 “libcurl 每个 pthread 一个 easy handle / 连接，而 ylong async 共享 client pool 是否破坏连接局部性” 这一假设，本轮重新检查了 async HTTP/1 pool 和 dispatcher：

- `ConnPool::connect_to()` 按 `connector.pool_key(uri)` 进入同一个 `PoolKey`；HTTPS proxy 的 proxy 配置已进入 pool key，不会跨不同 proxy/TLS 配置复用连接。
- HTTP/1 `Conns::exist_h1_conn()` 在 `Mutex<Vec<ConnDispatcher<_>>>` 中按 LIFO 查找未 shutdown 且未 occupied 的 dispatcher；`Http1Dispatcher` 只用 `occupied: AtomicBool` 独占同一条连接，没有额外 request I/O task、channel 或后台 dispatcher。
- 新建连接时 `dispatch_h1_conn()` 会先取得当前 dispatcher 的 handle，再把 dispatcher 放回 list；请求 write/read 和 body drain 都在调用方 future 上执行。
- 已有 `--client-per-worker` 诊断模式正好对应 libcurl 的 “每个 worker 固定一个 client/连接池” 模型；该模式在 `rt64, 64 KiB` 短测中为 -9.0%，且此前 trace 的 `connect_p99_us` 为微秒级。

结论：当前证据不支持新增 pool-affinity 或 sticky-connection 生产开关。strict CONNECT 的剩余 p99 仍发生在 request write 后到 response first-byte 的 Pending/resume 路径，而不是 HTTP/1 pool acquire/release 或连接复用局部性。

## HTTPS proxy 需求覆盖复核

复核日期：2026-05-25。结论：HTTPS proxy 的功能性需求在当前源码和 SDV 覆盖中已经闭环；剩余未完成项仍是严格 native CONNECT 口径下稳定 +20% 的性能目标。

实现覆盖：

- API 层通过 `ProxyBuilder::proxy_tls_config(TlsConfig)` 为 HTTPS proxy 独立设置 TLS 配置；该配置存储在 `ProxyInfo::tls_config`，不复用 origin TLS 配置。
- async connector 对 `HTTP target over HTTPS proxy` 使用 `proxy_tls_config` 建立外层 `ProxyHttps` TLS；对 `HTTPS target over HTTPS proxy` 先使用 `proxy_tls_config` 建立外层 proxy TLS，再发送 CONNECT，最后使用 client origin TLS 配置建立内层 origin TLS。
- sync connector 与 async connector 保持同样的分层：外层 HTTPS proxy TLS 使用 proxy TLS 配置，CONNECT 后的 origin TLS 使用 client TLS 配置。
- 代理元数据、匹配、no-proxy、pool key 统一在 `util::proxy`，transport 细节拆到 `async_impl::proxy` / `sync_impl::proxy`，connector 只负责组合 HTTP、HTTPS proxy、CONNECT 和 origin TLS 路径；该结构保留新增代理协议继续扩展的入口。

测试覆盖：

- `sdv_async_https_proxy.rs` / `sdv_sync_https_proxy.rs` 覆盖 HTTP target over HTTPS proxy、HTTPS target over HTTPS proxy、proxy basic auth、proxy CA 校验、proxy hostname mismatch、proxy mTLS 成功、缺少 client cert 失败、错误 client cert/key 失败、client cert/key mismatch build 失败、proxy TLS version mismatch、TLS 1.3 cipher-suite mismatch、以及 insecure proxy verify 不影响 origin verify。
- `ut_proxy_tls_config` 覆盖 builder 层 proxy TLS 配置写入；`ut_proxy_pool_key` 覆盖不同 proxy 实例进入不同连接池 key，避免不同 proxy/TLS 配置复用同一连接池条目。

状态判断：

- 子任务 1 和子任务 2 按当前源码复核没有发现新的功能缺口。
- 子任务 3 仍只在 `HTTP target over HTTPS proxy` 正式 workload 达到 20%+；`HTTPS target over HTTPS proxy / CONNECT` 在 native fixture、trace、runtime scheduling 实验下仍没有稳定达到 20%+，当前证据指向 ylong_runtime I/O wake / queue delay，而不是 proxy TLS 配置或模块拆分缺失。

## Benchmark target gate

为减少后续性能调优时的人工判读误差，`run_https_proxy_bench.sh` 现在会收集每个 client 的 metrics JSON，并在所有 run 结束后输出 `bench_summary` JSON：

- 默认目标为 `BENCH_TARGET_PCT=20`、`BENCH_TARGET_MIN_PASSES=4`。
- summary 按同一 client 的第 N 次 run 对比 libcurl 第 N 次 run，适配 `BENCH_ORDER=paired` 和 `BENCH_ORDER=grouped`。
- `formal_pass=true` 表示比较次数不少于目标 pass 数，且至少目标 pass 数达到 20%+，同时 ylong 该轮错误数不高于 libcurl。
- 设置 `BENCH_ENFORCE_TARGET=1` 时，未通过目标会让 runner 返回非 0，方便把该标准接入后续 CI 或本地验收脚本。

该变更不改变当前性能结论：它只是把现有文档中的 4/5、20%+、错误数约束固化为工具输出。严格 native CONNECT 仍需要后续 runtime 级优化才能让 `bench_summary.formal_pass` 稳定为 true。

补充修正：`async_https_proxy_bench` 的 metrics JSON 现在按编译 runtime 输出不同 client label：Tokio build 为 `ylong_http_client_async`，ylong runtime build 为 `ylong_http_client_async_ylong`，并新增 `runtime_backend` 字段。否则 `YLONG_CLIENT=both` 时 runner 会按 `"client"` 聚合 metrics，把两个 async runtime 的结果合并到同一个 `bench_summary`，导致正式比较口径失真。

libcurl baseline 也同步修正为 ready/start 双 barrier：所有 worker 完成 warmup 后先进入 ready barrier，主线程记录共享起始时间，再通过 start barrier 同时放行。这样 libcurl、async benchmark 和 sync benchmark 都使用同一类计时边界，避免 worker 从 barrier 提前返回而主线程尚未记录 `started` 的误差。

验证 smoke（非性能验收，`requests=1`、`concurrency=1`、`response_size=1024`、`YLONG_CLIENT=both`）确认输出 3 个独立 summary：

- `ylong_http_client_async`
- `ylong_http_client_async_ylong`
- `ylong_http_client_sync`

严格 native CONNECT 后续短测结论仍未变：

- Tokio backend 单轮 `requests=300`、`runtime_threads=16`、`read_buffer_size=256 KiB`：约 +3.8%，不足 20%。
- ylong backend 单轮 `requests=300`、`runtime_threads=16`、`read_buffer_size=1 MiB`：修正 libcurl start gate 后约 +10.0%，不足 20%。
- ylong backend 单轮 `requests=300`、`runtime_threads=64`、`read_buffer_size=1 MiB`：约 -4.7%，说明增加 runtime worker 数不是稳定收益。
- 诊断 runtime 的 `YLONG_RUNTIME_OPTIMISTIC_READ=1` 在 `read_buffer_size=256 KiB` 下约 -15.2%，不保留。
- CONNECT 外层 proxy TLS read-ahead 从 64 KiB 增到 256 KiB、以及 CONNECT 内层 origin TLS read-ahead 256 KiB 均为短测负向，不保留。

补充 corrected runtime trace（日志：
`target/https_proxy_bench/runtime_trace_corrected_probe_current.log`、
`target/https_proxy_bench/runtime_trace_corrected_libcurl_current.log`、
`target/https_proxy_bench/runtime_trace_corrected_probe_1m_current.log`、
`target/https_proxy_bench/runtime_trace_corrected_libcurl_1m_current.log`）：

| read buffer | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | request pending gap p99 | runtime local avg/max |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 256 KiB | 3611.343 | 3688.903 | -2.1% | 37.993ms | 27.052ms | 35.045ms | 1.391ms / 26.112ms |
| 1 MiB | 3222.091 | 3432.219 | -6.1% | 51.006ms | 30.668ms | 41.659ms | 2.807ms / 32.514ms |

256 KiB trace 中 `request_pending_gap_migrated_samples=255`、same-thread samples 只有 26，`response_wait_p99_us=35099`；runtime source 侧 `source_dequeue` 为 960 个样本、平均/最大 `1.460ms / 26.112ms`，而 `driver_dequeue` 调用本身平均/最大只有 `4.816us / 252us`。这与前面的结论一致：strict CONNECT 的剩余长尾主要发生在 I/O wake 入队后的 task 恢复/迁移路径，而不是 `dequeue()` 内一次 driver poll 调用本身。

当前可保留的进展是 benchmark 口径更可靠：client label 不再合并，libcurl 计时边界与 Rust benchmark 对齐。性能目标仍未完成，后续应继续定位 ylong runtime I/O wake / task resume，而不是扩大 TLS read-ahead 或 client-side optimistic read。

补充 async idle-interceptor skip probe（临时给默认 `IdleInterceptor` 增加标记，并让 async `HttpBody` 在默认 no-op interceptor 下跳过 body `intercept_output()` 动态分发；日志 `target/https_proxy_bench/idle_interceptor_skip_probe_current.log`）：

| Mode | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | request pending gap p99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| idle-interceptor skip | 3173.414 | 3256.834 | -2.6% | 55.309ms | 36.107ms | 51.997ms |

结论：默认 interceptor 动态分发不是 strict CONNECT 的主要瓶颈。跳过 body `intercept_output()` 没有降低 request Pending 长尾，吞吐也没有接近 +20%；该实验已撤回。

补充 repeat-3 复测（临时把 interceptor skip 扩大到 async HTTP/1 header/status 和 fixed-length/chunked/until-close body 读取路径；日志 `target/https_proxy_bench/strict_idle_interceptor_fastpath_repeat3.log`）：

```text
response_size=1 MiB
requests=128
warmup_requests=64
concurrency=64
runtime_threads=16
read_buffer_size=64 KiB
client=async-ylong
```

| Metric | Value |
| --- | ---: |
| ylong avg rps | 3667.407 |
| libcurl avg rps | 3357.273 |
| avg improvement | +9.237% |
| passes / comparisons | 0 / 3 |
| ylong avg p99 | 28.987ms |
| libcurl avg p99 | 22.595ms |

该版本需要给 public `Interceptor` trait 增加 enablement 方法来区分默认 no-op interceptor。虽然短测吞吐高于 libcurl，但仍远低于 20%+ 验收，且 ylong p99 仍弱于 libcurl；因此该 public API 变更不保留。结论维持：interceptor 动态分发不是 strict CONNECT 的主修复方向。

补充 source_dequeue queue-kind cross-tab probe（诊断 runtime 中额外输出 `source_dequeue_local/global/lifo/unknown_queue`，日志
`target/https_proxy_bench/source_dequeue_queue_trace_ylong_current.log`、
`target/https_proxy_bench/source_dequeue_queue_trace_libcurl_current.log`）：

| Mode | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | request pending gap p99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| source_dequeue x queue trace | 3291.086 | 3470.977 | -5.2% | 43.699ms | 31.123ms | 35.307ms |

该 probe 进一步拆开 `source_dequeue` 的队列归属：`source_dequeue` 共 892 个样本，全部落在 `source_dequeue_local`，`source_dequeue_global/lifo/unknown_queue` 均为 0；其中 local same/migrated worker 为 `663 / 229`，avg/max queue delay 为 `1.588ms / 31.770ms`。同轮 `driver_dequeue` avg/max 只有 `4.828us / 581us`。结论：`dequeue()` driver source 的剩余长尾不是隐藏在 global queue 或 LIFO slot 中，而是 I/O wake 放入发现 readiness 的 worker local queue 后，被同 worker 排队或被其他 worker steal 恢复造成的 local-queue resume 延迟。该诊断改动仅存在于 ignored `target/ylong_runtime_trace`，没有进入当前仓库源码。

补充 I/O last-worker targeted wake probe（诊断 runtime 中给 task header 记录 last-run worker；net I/O wake 时将 task 放入 global queue front，并额外 unpark 该 last-run worker。没有跨线程写入其他 worker local queue，因为 runtime local queue 是 SPMC，跨 worker local push 不安全。日志：
`target/https_proxy_bench/io_last_worker_probe_ylong_current.log`、
`target/https_proxy_bench/io_last_worker_probe_libcurl_current.log`、
`target/https_proxy_bench/io_last_worker_no_global_first_probe_ylong_current.log`、
`target/https_proxy_bench/io_last_worker_no_global_first_probe_libcurl_current.log`）：

| Mode | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | request pending gap p99 | runtime global avg/max |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| last-worker + global-front + global-first | 3526.413 | 3451.688 | +2.2% | 32.740ms | 28.002ms | 25.088ms | 1.408ms / 11.567ms |
| last-worker + global-front | 3367.337 | 3567.140 | -5.6% | 46.058ms | 30.614ms | 40.090ms | 1.885ms / 37.528ms |

第一次实现仅 targeted unpark、且抑制正常 random wake，会让本地 300-request probe 超过 15s 仍无输出；该实现已作为诊断反例修正为 targeted unpark 后仍保留正常 wake。修正后，叠加 `YLONG_RUNTIME_GLOBAL_FIRST=1` 可以把 request Pending p99 从前一轮 cross-tab 的约 35.3ms 降到约 25.1ms，但吞吐仍只有 +2.2%，p99 仍弱于 libcurl；不叠加 global-first 则退化到 -5.6%。结论：仅把 I/O wake task 投递到 global front 并唤醒 last-run worker 不是严格 CONNECT 的充分 runtime 修复；如果后续做 runtime 生产设计，需要更直接且安全的 per-worker injection/ownership 机制，而不是复用当前 global queue。

补充 per-worker injection queue probe（诊断 runtime 中新增每个 worker 一个 Mutex-backed injection queue，net I/O wake 直接投递到 last-run worker 的 injection queue，并让 worker 在 local/global queue 前先取 injection queue；日志
`target/https_proxy_bench/io_inject_worker_probe_ylong_current.log`、
`target/https_proxy_bench/io_inject_worker_probe_libcurl_current.log`）：

| Mode | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | request pending gap p99 | inject avg/max |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| per-worker injection queue | 2389.855 | 3528.457 | -32.3% | 100.545ms | 30.637ms | 98.533ms | 4.238ms / 116.183ms |

该 probe 完全消除了 request/body trace 中的 migrated gap（request migrated samples 为 0，runtime inject same/migrated 为 `809 / 0`），但 same-worker injection queue 反而出现更长排队：`source_dequeue_inject` avg/max 为 `2.417ms / 116.183ms`，`source_park` avg/max 为 `6.322ms / 100.137ms`。结论：单纯把 I/O wake 固定回 last-run worker 会牺牲负载均衡，导致部分 worker 的 injection queue 堆积；它解释了“迁移不是唯一尾延迟来源”，也不应作为生产补丁。后续 runtime 方向需要在 locality 和 work-sharing 之间做更细粒度的策略，而不是强制 same-worker injection。

补充 idle-only injection probe（在上一个 per-worker injection queue 诊断基础上增加 `YLONG_RUNTIME_IO_INJECT_IDLE_ONLY=1`：只有 last-run worker 已进入 sleeper idle list 时才投递到该 worker 的 injection queue，否则回落到默认 I/O wake 入队路径；日志
`target/https_proxy_bench/io_inject_idle_probe_ylong_current.log`、
`target/https_proxy_bench/io_inject_idle_probe_libcurl_current.log`）：

| Mode | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | request pending gap p99 | inject samples / avg / max |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| idle-only injection queue | 3293.608 | 3534.402 | -6.8% | 39.085ms | 27.550ms | 33.530ms | 86 / 0.416ms / 7.688ms |

该 hybrid 避免了 always-inject 的 100ms 级 same-worker 堆积，但因为只有 86 个 runtime 样本走 injection queue，`source_dequeue_local` 仍有 883 个样本、avg/max 为 `2.293ms / 24.604ms`，request migrated samples 仍有 253。结论：只在 last-run worker 已 idle 时保持 locality 过于保守，不能显著改变 strict CONNECT 的主队列形态；它比 always-inject 安全，但也不能接近 20%+。

补充 local-depth spill probe（诊断 runtime 中新增 `YLONG_RUNTIME_IO_SPILL_LOCAL_LEN=N`：net I/O wake 发生在 worker 上下文内时，只有当前 worker local queue 近似长度达到阈值才把 task spill 到 global queue；本轮叠加 `YLONG_RUNTIME_GLOBAL_FIRST=1`，避免 spilled task 因 worker 很少检查 global queue 被额外延后。日志：
`target/https_proxy_bench/io_spill_len16_global_first_ylong_current.log`、
`target/https_proxy_bench/io_spill_len16_global_first_libcurl_current.log`、
`target/https_proxy_bench/io_spill_len4_global_first_ylong_current.log`、
`target/https_proxy_bench/io_spill_len4_global_first_libcurl_current.log`）：

| Mode | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | request pending gap p99 | source_dequeue local/global |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| spill local len >= 16 + global-first | 3467.474 | 3617.334 | -4.1% | 41.033ms | 27.719ms | 39.574ms | 750 / 61 |
| spill local len >= 4 + global-first | 3509.932 | 3546.477 | -1.0% | 55.012ms | 29.225ms | 42.232ms | 655 / 217 |

阈值 16 只把少量 `source_dequeue` wake 从 local queue 移到 global queue，`source_dequeue_local` avg/max 仍为 `2.184ms / 25.058ms`，request Pending p99 退化到 39.574ms。阈值 4 更积极，`source_dequeue_global` 增到 217 个样本且 global avg/max 为 `1.058ms / 7.163ms`，但 request p99 反而到 55.012ms，说明把“local queue 已有一定积压”的 I/O wake 改投 global queue 只是在 local/global 之间转移排队，并没有解决 strict CONNECT 的 request first-byte 长尾。该 runtime 诊断策略不进入当前仓库源码。

补充 CONNECT outer-proxy isolation probe：为确认严格 CONNECT 的主尾延迟是否必须依赖外层 HTTPS proxy TLS，本轮修正 `libcurl_harness.c`，让它按 `--proxy` URL scheme 选择 `CURLPROXY_HTTP` 或 `CURLPROXY_HTTPS`；此前 hardcode `CURLPROXY_HTTPS` 会导致 `http://` proxy isolation run 的 libcurl 侧挂住。随后用 native fixture 跑 `HTTPS target over HTTP proxy`（origin TLS 开启、proxy TLS 关闭，日志：
`target/https_proxy_bench/connect_plain_proxy_ylong_current.log`、
`target/https_proxy_bench/connect_plain_proxy_libcurl_current.log`）：

| Mode | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | request pending gap p99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| HTTPS target over HTTP proxy | 4257.758 | 4442.865 | -4.2% | 45.646ms | 26.185ms | 37.285ms |

去掉外层 HTTPS proxy TLS 后，ylong 吞吐和 libcurl 都提高，但相对差距仍为负，且 request Pending gap p99 仍有 37.285ms、migrated samples 仍为 254。结论：严格 CONNECT 的 request first-byte 长尾不是外层 proxy TLS 独有；plain CONNECT + inner origin TLS 仍会触发同类 ylong_runtime wake/resume 分布问题。后续优化不应只围绕 proxy TLS read-ahead 或双层 TLS copy，仍应继续看 CONNECT response first-byte 的 runtime resume 策略。

补充 direct HTTPS isolation probe：为进一步拆分 “CONNECT 本身” 与 “origin TLS/readiness runtime” 的影响，本轮把 async/sync benchmark 和 libcurl harness 的 `--proxy` 改为可选；省略 `--proxy` 时 ylong 不安装 proxy，libcurl harness 显式设置空 proxy 以屏蔽环境变量中的 `http_proxy/https_proxy/all_proxy`。随后用同一 native fixture 的 HTTPS origin 直连（日志：
`target/https_proxy_bench/direct_https_ylong_current.log`、
`target/https_proxy_bench/direct_https_libcurl_current.log`）：

| Mode | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | request pending gap p99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Direct HTTPS origin | 6961.729 | 6920.096 | +0.6% | 28.758ms | 19.599ms | 25.809ms |

直连 HTTPS 下 ylong 吞吐与 libcurl 基本持平，但 p99 仍更高，request Pending gap p99 仍有 25.809ms、migrated samples 为 242。结合上一轮 `HTTPS target over HTTP proxy` 的 -4.2% 和 strict `HTTPS target over HTTPS proxy` 的负结果，当前分层结论是：ylong_runtime 的 TLS read wake/resume 尾延迟是基础问题，CONNECT/proxy 层在此基础上进一步放大或增加固定开销。后续如果只优化 CONNECT 外层 proxy TLS，不足以达成严格 HTTPS proxy +20%；需要把 direct/origin TLS pending gap 降下来，或在 CONNECT 复用路径上绕开这类 runtime resume 尾延迟。

补充 direct HTTPS runtime/client split probe（同一 native fixture、顺序运行，避免并发 probe 互相干扰；日志：
`target/https_proxy_bench/direct_https_split_async_ylong_current.log`、
`target/https_proxy_bench/direct_https_split_async_tokio_current.log`、
`target/https_proxy_bench/direct_https_split_sync_current.log`、
`target/https_proxy_bench/direct_https_split_libcurl_current.log`）：

| Client | rps | vs libcurl | p99 | request pending gap p99 | body pending gap p99 | body reads |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| async ylong runtime | 6898.424 | -3.1% | 27.707ms | 24.575ms | 11.357ms | 2597 |
| async Tokio runtime | 6606.777 | -7.2% | 31.347ms | 10.245ms | 16.335ms | 2768 |
| sync | 6057.631 | -14.9% | 7.898ms | n/a | n/a | 19200 |
| libcurl | 7116.931 | baseline | 19.290ms | n/a | n/a | 19200 |

该 split 说明 direct HTTPS 下不是单一 TLS copy/应用层 body chunk 问题：sync 路径 p99 最低但吞吐低于 libcurl；Tokio 的 request first-byte Pending gap 明显低于 ylong runtime，但 body drain gap 更高、总吞吐仍低于 ylong runtime；ylong runtime 的 body coalescing/ready-drain 让 body reads 远少于 libcurl，但 request first-byte gap 仍是尾延迟主因。对严格 CONNECT 而言，继续优化方向应优先区分 “request first-byte resume” 与 “body drain throughput” 两条路径；只增加 body 合并或只切 runtime 都不足以达成 HTTPS proxy +20%。

补充 strict CONNECT runtime/client split probe（同一 native fixture、顺序运行，严格 `HTTPS target over HTTPS proxy` 当前口径；日志：
`target/https_proxy_bench/strict_split_async_ylong_current.log`、
`target/https_proxy_bench/strict_split_async_tokio_current.log`、
`target/https_proxy_bench/strict_split_sync_current.log`、
`target/https_proxy_bench/strict_split_libcurl_current.log`）：

| Client | rps | vs libcurl | p99 | request pending gap p99 | body pending gap p99 | body reads |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| async ylong runtime | 3545.626 | -4.0% | 37.709ms | 31.414ms | 14.263ms | 2683 |
| async Tokio runtime | 3407.083 | -7.7% | 46.138ms | 22.941ms | 11.846ms | 2608 |
| sync | 3162.673 | -14.4% | 13.457ms | n/a | n/a | 19200 |
| libcurl | 3692.489 | baseline | 30.607ms | n/a | n/a | 19200 |

严格 CONNECT 下 async ylong runtime 仍是当前 ylong 最优吞吐路径，但只到 libcurl 的 -4.0%，距离 +20% 目标约 24 个百分点；Tokio 的 request Pending gap 比 ylong 小，但吞吐和 p99 更差；sync p99 低但吞吐更低。该结果把完成路径进一步压缩到：不能简单切 Tokio 或 sync，也不能只靠 body coalescing。需要让 async ylong 的 request first-byte resume gap 接近 Tokio/libcurl 水平，同时保持 ylong 当前较好的 body 吞吐，或找到 CONNECT 级路径减少每轮 request first-byte wake 依赖。

补充 strict CONNECT read-buffer sweep（同一 native fixture、严格 `HTTPS target over HTTPS proxy`，`requests=300`、`warmup=64`、`concurrency=64`、`runtime_threads=16`，日志：
`target/https_proxy_bench/strict_rb_sweep_65536_current.log`、
`target/https_proxy_bench/strict_rb_sweep_131072_current.log`、
`target/https_proxy_bench/strict_rb_sweep_262144_current.log`、
`target/https_proxy_bench/strict_rb_sweep_524288_current.log`、
`target/https_proxy_bench/strict_rb_sweep_1048576_current.log`）：

| read buffer | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | ylong body reads | libcurl body reads |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 64 KiB | 3689.462 | 3231.888 | +14.2% | 39.188ms | 51.590ms | 5071 | 19200 |
| 128 KiB | 3276.354 | 3314.661 | -1.2% | 41.225ms | 35.424ms | 2649 | 19200 |
| 256 KiB | 3508.464 | 3663.406 | -4.2% | 45.989ms | 26.311ms | 2617 | 19200 |
| 512 KiB | 3484.426 | 3164.824 | +10.1% | 44.087ms | 43.908ms | 2725 | 19200 |
| 1 MiB | 3362.719 | 3242.262 | +3.7% | 46.558ms | 33.585ms | 2618 | 19200 |

64 KiB 是本轮 sweep 的最佳单点，但仍不到 +20%；更大的 read buffer 没有稳定收益。对 64 KiB 做正式 `REPEAT=5` paired 复测（日志 `target/https_proxy_bench/strict_rb64_repeat5_current.log`）后，ylong 平均 3655.357 rps、libcurl 平均 3520.910 rps，平均提升只有 +3.845%，`passes=0/5`、`formal_pass=false`。同点 trace（日志 `target/https_proxy_bench/strict_rb64_trace_current.log`）显示 request Pending gap p99 仍为 37.603ms，migrated samples 为 262，body Pending gap p99 为 23.906ms。结论：当前严格 CONNECT 目标不能靠调整 response read buffer 达成；64 KiB 可以作为后续对比默认点，但不是可提交的性能修复。

补充 client/runtime placement probe（同一 64 KiB 严格 CONNECT 点，日志：
`target/https_proxy_bench/strict_rb64_client_per_worker_trace_current.log`、
`target/https_proxy_bench/strict_rb64_runtime_affinity_trace_current.log`）：

| Mode | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | request pending gap p99 | body pending gap p99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| client per worker | 2952.034 | 3513.539 | -16.0% | 46.023ms | 26.856ms | 38.805ms | 28.240ms |
| runtime affinity | 3557.222 | 3550.884 | +0.2% | 34.310ms | 27.416ms | 28.841ms | 16.991ms |

`--client-per-worker` 反而降低吞吐，并且没有改善 request Pending gap，说明共享 `Client`/pool 不是严格 CONNECT 的主要瓶颈。`--runtime-affinity` 能把一次 trace 的 request Pending gap p99 从 37.603ms 降到 28.841ms、body Pending gap p99 从 23.906ms 降到 16.991ms，但吞吐仍只与 libcurl 持平，离 +20% 很远。结论：线程亲和性可作为 runtime resume 诊断信号，但不能作为完成目标的性能方案。

补充 strict CONNECT runtime thread-count sweep（同一 64 KiB 严格 CONNECT 点，日志：
`target/https_proxy_bench/strict_rb64_threads_4_current.log`、
`target/https_proxy_bench/strict_rb64_threads_8_current.log`、
`target/https_proxy_bench/strict_rb64_threads_32_current.log`）：

| runtime threads | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 4 | 2890.493 | 3549.876 | -18.6% | 64.640ms | 29.128ms |
| 8 | 3363.414 | 3482.986 | -3.4% | 42.651ms | 33.355ms |
| 32 | 3677.005 | 3397.816 | +8.2% | 35.099ms | 40.497ms |

结合已有 `runtime_threads=16` 的 `REPEAT=5` 复测（平均 +3.845%）和此前 `runtime_threads=64` 负向短测，当前没有一个简单 runtime worker 数设置能稳定达到 +20%。32 线程单点好于 4/8，但仍不足目标，且后续不应把完成路径压在单轮噪声上。

补充 body ready-drain depth probe：临时把 async `BODY_READY_DRAIN_READS` 从 8 分别改为 16 和 4，验证是否能通过改变单次 body poll 中连续 drain 的 TLS read 次数降低 runtime 排队；改动已撤回，源码仍保持 8。日志：
`target/https_proxy_bench/strict_rb64_body_drain16_trace_current.log`、
`target/https_proxy_bench/strict_rb64_body_drain4_trace_current.log`：

| BODY_READY_DRAIN_READS | ylong rps | libcurl rps | 提升 | request pending gap p99 | body pending gap p99 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 16 | 3562.659 | 3579.055 | -0.5% | 51.185ms | 9.410ms |
| 4 | 3605.900 | 3594.020 | +0.3% | 42.467ms | 21.513ms |

把 ready-drain 深度调大确实降低 body Pending gap，但 request first-byte Pending gap 明显变差，吞吐不升；调小也只到基本持平。结论：body drain coalescing 深度不是当前严格 CONNECT +20% 的生产修复点。

补充 strict CONNECT concurrency / placement sweep（同一 1 MiB response、64 KiB read buffer；日志：
`target/https_proxy_bench/strict_rb64_concurrency_16_current.log`、
`target/https_proxy_bench/strict_rb64_concurrency_32_current.log`、
`target/https_proxy_bench/strict_rb64_concurrency_128_current.log`、
`target/https_proxy_bench/strict_rb64_threads32_affinity_trace_current.log`）：

| Mode | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 |
| --- | ---: | ---: | ---: | ---: | ---: |
| concurrency 16, runtime_threads 16 | 3512.256 | 3272.465 | +7.3% | 9.568ms | 8.533ms |
| concurrency 32, runtime_threads 16 | 3331.963 | 3397.278 | -1.9% | 22.923ms | 22.854ms |
| concurrency 128, runtime_threads 16 | 2759.207 | 2571.950 | +7.3% | 99.391ms | 110.927ms |
| concurrency 64, runtime_threads 32 + affinity | 3471.184 | 3563.284 | -2.6% | 41.795ms | 26.414ms |

16/128 并发下 ylong 相对 libcurl 有约 +7% 单轮优势，但仍远低于 20%；32 并发和 32-thread affinity 组合为负。结论：严格 CONNECT 目标不是只要换一个简单并发/worker/affinity 形状就能稳定达成。

补充 response-size scaling probe：把 native fixture response 从 1 MiB 提到 4 MiB，保持 `concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`，用 `requests=100` 控制总传输量；日志 `target/https_proxy_bench/strict_rb64_resp4m_trace_current.log`：

| response size | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | request pending gap p99 | body pending gap p99 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 4 MiB | 916.216 | 936.575 | -2.2% | 100.090ms | 91.316ms | 85.878ms | 69.531ms |

更大 body 没有把 ylong 的较少 body read 次数转化为 +20% 吞吐；request 和 body 的 migrated Pending gap 都随传输量变大而放大。结论：当前严格 CONNECT 缺口仍不能靠“更大下载响应摊薄 CONNECT/first-byte 成本”解决，后续重点仍应放在 ylong_runtime I/O wake/resume 和 TLS read task 恢复路径。

补充严格 CONNECT 64 KiB `perf stat` / `perf record` probe：保持 native fixture、`requests=300`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`，先用 `PROFILE=perf-stat` 运行 paired 短测，再对 ylong async runtime 和 libcurl 各采一个 `perf record -F 997 -g` 样本。日志：
`target/https_proxy_bench/strict_rb64_perf_stat_current.log`、
`target/https_proxy_bench/strict_rb64_ylong_perf.data`、
`target/https_proxy_bench/strict_rb64_libcurl_perf.data`：

| Client | rps | vs libcurl | p99 | task-clock | cycles | instructions | ctx switches | migrations |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| async ylong runtime | 3637.233 | +3.4% | 42.409ms | 668.57ms | 2.792B | 3.307B | 1366 | 280 |
| libcurl | 3517.081 | baseline | 31.933ms | 764.55ms | 3.115B | 3.533B | 2758 | 558 |

同点 `perf record` 是单次采样，会受 perf 开销和 run-to-run 噪声影响；该轮 ylong 为 3661.989 rps、libcurl 为 3703.183 rps。DSO-level CPU 分布如下：

| Client | libcrypto | kernel | libc | libssl | client/lib |
| --- | ---: | ---: | ---: | ---: | ---: |
| async ylong runtime | 52.67% | 25.71% | 8.87% | 6.21% | 4.48% |
| libcurl | 53.00% | 27.55% | 6.64% | 5.97% | 4.95% |

Top symbol 也基本一致：ylong 侧为 `_copy_to_iter` 7.93%、`__memmove_avx_unaligned_erms` 7.37%；libcurl 侧为 `_copy_to_iter` 8.80%、`__memmove_avx_unaligned_erms` 5.21%。结论：当前严格 CONNECT 64 KiB 点没有暴露出一个 ylong_http_client 专属的 CPU 热点；ylong 在 perf-stat 里 cycles、instructions、context switches、migrations 都低于 libcurl，但吞吐仍只有 +3.4%，并且 p99 更差。剩余差距更符合前面 trace 的判断：不是简单平均 CPU 工作量问题，而是 TLS/socket readiness 后的任务恢复分布和尾延迟问题。该证据不足以支持继续在本仓库里做新的 client-side 猜测性优化；下一步如果要继续冲击 +20%，需要把 runtime I/O wake/resume 作为明确设计对象，或引入新的可验证 client 层机制来绕开 request first-byte wake 长尾。

补充 ylong async start-gate hardening：此前 async ylong benchmark 已经采用 ready/start 双阶段，但 ylong cfg 下 start release 仍通过共享 `Waiter::wake_one()` 循环完成。为避免 start signal 语义和 wake 顺序成为 ylong/libcurl 对比变量，本轮把 ylong cfg 改成每个 worker 一个 start channel；主任务在所有 worker warmup 后记录共享 `Instant`，再向每个 worker 发送同一个 start instant。同时 metrics JSON 增加 `worker_start_delay_us_min/max`，trace summary 也输出同一字段。Tokio cfg 仍使用 barrier；两个 async cfg 均通过编译检查。

用同一严格 CONNECT 64 KiB 点复测（native fixture、`requests=300`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`REPEAT=5`，日志 `target/https_proxy_bench/strict_rb64_start_gate_repeat5_current.log`）：

| Client | avg rps | vs libcurl | passes | formal pass | p99 range | worker start delay max |
| --- | ---: | ---: | ---: | --- | ---: | ---: |
| async ylong runtime | 3532.296 | +1.247% | 0/5 | false | 40.262-47.945ms | 0.603-2.656ms |
| libcurl | 3489.428 | baseline | n/a | n/a | 25.462-51.485ms | n/a |

结论：start-gate hardening 让 ylong release skew 可见，并排除了“启动门 wake 方式隐藏 20%+”这个解释；本轮最大 ylong worker start delay 只有 2.656ms，平均吞吐仍只有 +1.247%，`bench_summary.formal_pass=false`。严格 CONNECT 剩余缺口依旧不是 benchmark 启动门问题，后续仍应集中在 runtime I/O wake/resume 或新的 request first-byte 绕行机制。

补充 CONNECT 内层 origin TLS 64 KiB read-ahead probe：此前 256 KiB origin read-ahead 已经负向，本轮只在 `HTTPS target over HTTPS proxy` 的 CONNECT 内层 origin TLS 上临时启用 64 KiB read-ahead，外层 HTTPS proxy TLS 仍保持 64 KiB，验证是否能用更小 buffer 改善 strict CONNECT body/readiness 分布。该改动已撤回。日志：
`target/https_proxy_bench/strict_origin_readahead64_probe_current.log`、
`target/https_proxy_bench/strict_origin_readahead64_repeat5_current.log`：

| Probe | ylong avg rps | libcurl avg rps | avg uplift | passes | formal pass | ylong p99 range |
| --- | ---: | ---: | ---: | ---: | --- | ---: |
| origin read-ahead 64 KiB, 3 runs | 3704.222 | 3571.053 | +3.762% | 0/3 | false | 45.179-60.332ms |
| origin read-ahead 64 KiB, 5 runs | 3639.133 | 3556.717 | +2.405% | 0/5 | false | 43.902-50.411ms |

结论：64 KiB origin read-ahead 不是可提交的 strict CONNECT 性能修复。它没有改变 body read 数量级（仍约 5k reads / 300 MiB），没有改善 ylong p99，且 5-run 中仍有负向 run；因此当前源码继续保持 CONNECT 内层 origin TLS 不启用 read-ahead。

补充 bench summary 诊断字段：本轮没有新增客户端侧性能补丁。同步 benchmark 与 libcurl harness 现在也输出 `worker_start_delay_us_min/max`，与 async benchmark 的 start-gate 指标对齐；runner 的 `bench_summary` 在不改变 `formal_pass` 判定口径的前提下，额外汇总当前 client 与 libcurl baseline 的 `latency_us_p99`、`worker_start_delay_us_max`、`worker_elapsed_us_max` 平均/最小/最大值。同时修正了浮点比较边界，避免精确 20% 提升因二进制浮点误差显示为 `20.0` 但未计入 pass。这样后续严格 CONNECT 复测可以直接从 summary 区分吞吐未达标、P99 长尾和启动门偏斜，而不需要再手工回扫每轮 metrics JSON。

该变更不改变当前目标状态：严格 `HTTPS target over HTTPS proxy` 64 KiB 点最近正式复测仍为 `formal_pass=false`，当前仓库侧保留的结论仍是缺口主要在 ylong_runtime I/O wake / task resume 分布，尚未达成 HTTPS proxy 场景 +20%。

补充 ylong runtime 本地覆盖能力：由于本仓库只把 `ylong_runtime` 作为 git dependency 引入，不包含 runtime 源码，之前的 runtime wake/queue probe 只能依赖 ignored `target/ylong_runtime_trace` / `target/ylong_runtime_lifo` 手工构建，复现实验容易漏掉实际依赖路径。本轮 runner 增加 `YLONG_RUNTIME_PATH` 环境变量，支持指向 runtime 仓库根目录或其中的 `ylong_runtime` package；构建 ylong benchmark 时会通过 Cargo `patch."https://gitcode.com/openharmony/commonlibrary_rust_ylong_runtime.git".ylong_runtime.path=...` 覆盖现有 git dependency，并在 `bench_environment.ylong_runtime_path` 记录解析后的路径。实测本地 path override 会让当前 rustc 对 runtime 源码直接触发 `dangerous_implicit_autorefs` deny-by-default lint，而 git dependency 平时由 Cargo cap lints；因此 runner 只在 `YLONG_RUNTIME_PATH` 模式下追加 `-A dangerous_implicit_autorefs`，保证 A/B 运行不会在进入 benchmark 前失败。

这个能力不改变正式目标口径，也不会让当前 strict CONNECT 自动通过；它把后续 runtime I/O wake / task resume 修复的 A/B 验证纳入同一 `run_https_proxy_bench.sh`、`bench_summary.formal_pass` 和 P99/start-delay 诊断输出里，避免继续用不可追踪的手工替换路径比较。

补充 runner 日志收敛：`run_client_command()` 现在把 client stderr 合并到 per-client `tee` 输出中，再从同一个临时日志提取 metrics JSON。这样 `YLONG_RUNTIME_TRACE`、`PROFILE=time` 或 `PROFILE=perf-stat` 这类 stderr 诊断会进入普通 stdout 日志；metrics 聚合仍只读取含 `"client"` 和 `"rps"` 的 JSON 行，因此不改变 `bench_summary` 判定口径。

用该能力对 `target/ylong_runtime_lifo` 的小补丁做一次 bounded smoke（非正式验收；补丁内容仅为缩短 LIFO slot borrow 作用域，并在 local/LIFO 仍有 work 时跳过 periodic driver `run_once()`；native fixture、`requests=128`、`warmup=32`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`）：

| Runtime path | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | ylong start delay max |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `target/ylong_runtime_lifo` | 2161.930 | 2394.224 | -9.702% | 50.381ms | 50.012ms | 2.856ms |

这次 smoke 的价值主要是验证 local runtime override 可用；结果本身不支持把该 LIFO 补丁作为 strict CONNECT +20% 完成路径。后续 runtime 方向仍需要更直接地处理 I/O wake task 的入队/恢复分布，而不是只减少 worker 已有 local/LIFO work 时的 driver poll 干扰。

继续用 `target/ylong_runtime_trace` 复核此前最接近的 runtime 方向：I/O wake task 记录 last-run worker，wake 时投递到 global front，并让 worker 优先检查 global queue（`YLONG_RUNTIME_IO_LAST_WORKER=1`、`YLONG_RUNTIME_GLOBAL_FIRST=1`、`YLONG_RUNTIME_TRACE=last_worker_global_first`）。同样采用 bounded smoke（native fixture、`requests=128`、`warmup=32`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`）：

| Runtime mode | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | ylong start delay max |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| last-worker global-front + global-first | 2333.959 | 2297.489 | +1.587% | 50.206ms | 51.194ms | 1.139ms |

runtime trace 同轮显示 `source_dequeue_global` 有 265 个样本，avg/max 为 `507us / 3.135ms`，same/migrated 为 `28 / 237`；`source_park` same/migrated 为 `58 / 341`。该策略能把 local queue delay 转成更短的 global queue delay，但仍保留大量 worker migration，吞吐只小幅高于 libcurl，远低于 +20%。因此它也不是可直接提交的完成路径；后续需要的是更明确的 runtime ownership/work-sharing 设计，而不是在 global queue 前后继续调小策略。

补充 current-state bounded refresh（native fixture、strict CONNECT、`requests=128`、`warmup=32`、`concurrency=64`、`read_buffer_size=64 KiB`、`REPEAT=3`、带 `--trace-summary`，日志：
`target/https_proxy_bench/strict_current_refresh_repeat3.log`、
`target/https_proxy_bench/strict_current_threads32_repeat3.log`）：

| Runtime threads | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg |
| ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: |
| 16 | 2530.888 | 2343.785 | +8.005% | 0/3 | false | 45.782ms | 50.616ms |
| 32 | 2444.167 | 2398.435 | +2.089% | 0/3 | false | 47.850ms | 49.883ms |

该 refresh 使用当前 runner 的 stderr 合并日志能力重新确认：短 bounded 点下 ylong 的 p99 可低于 libcurl，但吞吐仍只到 +8% 左右；增加 runtime worker 到 32 反而降低收益。严格目标仍不能靠 worker 数配置达成。

补充两个 isolated runtime waker micro-probe（只在 ignored scratch runtime path 中修改，未进入当前仓库源码；同一 bounded strict CONNECT 点，`YLONG_RUNTIME_PATH` 覆盖构建，日志：
`target/https_proxy_bench/strict_runtime_wake_consume_repeat3.log`、
`target/https_proxy_bench/strict_runtime_wake_inline_repeat3.log`）：

| Runtime scratch path | 变更 | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass |
| --- | --- | ---: | ---: | ---: | ---: | --- |
| `target/ylong_runtime_wake_consume` | `ScheduleIO::wake0` 对已取出的 waker 调 `wake()` 而不是 `wake_by_ref()` | 2402.811 | 2421.641 | -0.754% | 0/3 | false |
| `target/ylong_runtime_wake_inline` | reader/writer waker 留在栈上，只有 waiter-list 路径使用 `Vec` | 2514.028 | 2407.491 | +4.689% | 0/3 | false |

结论：`wake_by_ref()` 的额外引用路径和 `wake0` 常见 reader/writer case 的 `Vec` 分配都不是当前 +20% 缺口的主因。后续 runtime 方向不应再停留在单个 waker 调用点微调；仍需要围绕 I/O readiness 归属、task resume ownership、worker migration/steal 策略做成体系的 runtime 设计和复测。

补充 current-state read-buffer sweep（native fixture、strict CONNECT、`requests=128`、`warmup=32`、`concurrency=64`、`runtime_threads=16`、`REPEAT=2`、带 `--trace-summary`，日志 `target/https_proxy_bench/strict_current_rb_sweep_repeat2.log`）：

| read buffer | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg |
| ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: |
| 16 KiB | 2403.115 | 2390.944 | +0.661% | 0/2 | false | 47.498ms | 49.354ms |
| 32 KiB | 2621.963 | 2411.716 | +8.758% | 0/2 | false | 44.685ms | 48.354ms |
| 64 KiB | 2556.558 | 2033.232 | +26.630% | 1/2 | false | 44.683ms | 58.399ms |
| 128 KiB | 2612.045 | 2396.281 | +9.176% | 0/2 | false | 44.397ms | 49.644ms |

64 KiB 在该短 sweep 中平均超过 +20%，但只有 `1/2` run 达标，且 libcurl baseline 明显偏低；因此只把它作为需要正式复核的候选点。随后对同一 64 KiB bounded 点做 `REPEAT=5` 复核，分别带 trace 与不带 trace（日志：
`target/https_proxy_bench/strict_current_rb64_repeat5.log`、
`target/https_proxy_bench/strict_current_rb64_repeat5_notrace.log`）：

| Mode | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg |
| --- | ---: | ---: | ---: | ---: | --- | ---: | ---: |
| 64 KiB, trace summary | 2558.612 | 2392.619 | +7.547% | 1/5 | false | 45.860ms | 49.479ms |
| 64 KiB, no trace | 2611.638 | 2468.662 | +5.923% | 0/5 | false | 45.302ms | 47.740ms |
| 64 KiB, no trace, trace timers skipped | 2515.338 | 2386.719 | +5.421% | 0/5 | false | 43.875ms | 49.394ms |
| 64 KiB, no trace fast path | 2614.957 | 2400.763 | +8.955% | 0/5 | false | 44.624ms | 48.266ms |
| 64 KiB, no trace fast path, warmup 64 | 3579.869 | 3359.907 | +6.793% | 0/3 | false | 30.331ms | 24.327ms |

`no trace fast path` 指 `async_https_proxy_bench` 在 `--trace-summary=false` 时直接执行 request/body read，不再经过 trace wrapper 或空 trace 结构；这是基准程序自身的合理低开销路径。该改动把 repeat-5 平均提升到 +8.955%，最佳单轮到 +18.895%，但仍为 `passes=0/5`、`formal_pass=false`。

`requests=128`、`concurrency=64` 时，`warmup=32` 只覆盖一半 worker；因此又用 `warmup=64` 复核了一次每个 worker 至少一个 warmup request 的形状。该配置明显降低双方 p99，但 ylong 平均提升仍只有 +6.793%，且单轮最高 +12.689%，仍不是 20%+ 完成路径。

同一 fast path 下补充短测：

| Probe | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass |
| --- | ---: | ---: | ---: | ---: | --- |
| 32 KiB, rt16, repeat2 | 2531.112 | 2584.399 | -2.133% | 0/2 | false |
| 64 KiB, rt16, repeat2 | 2394.349 | 2436.213 | -1.524% | 0/2 | false |
| 128 KiB, rt16, repeat2 | 2573.281 | 2425.982 | +6.073% | 0/2 | false |
| 256 KiB, rt16, repeat2 | 2429.269 | 2431.753 | -0.059% | 0/2 | false |
| 64 KiB, rt32, repeat3 | 2636.617 | 2453.617 | +7.572% | 0/3 | false |
| 64 KiB, rt64, repeat3 | 2551.641 | 2467.151 | +3.463% | 0/3 | false |

同时，`run_https_proxy_bench.sh` 的 `bench_summary` 现在重复输出 `requests`、`warmup_requests`、`concurrency`、`runtime_threads`、`read_buffer_size`，并新增 `warmup_covers_workers`，避免后续把半数 worker 冷连接的 bounded 结果和全 worker warmed 结果混在一起解读。smoke 日志 `target/https_proxy_bench/strict_summary_warmup_fields_smoke.log` 已验证该字段输出。

结论：当前代码下 64 KiB 仍是 bounded strict CONNECT 的合理默认对比点，但它的短 sweep +26.6% 不可复现为正式通过；去掉 ylong-only trace instrumentation、跳过 no-trace 计时、再拆出 no-trace fast path 后，正式 repeat-5 也只有 +9.0%，`formal_pass=false`；把 warmup 提到覆盖全部 worker 后也只有 +6.8%。因此 read-buffer tuning、benchmark 统计开销、warmup 覆盖和简单 runtime thread 数调整都不是当前 +20% 完成路径。

补充 runtime wake-all 负实验：在 ignored scratch runtime `target/ylong_runtime_trace` 中短暂增加 `YLONG_RUNTIME_IO_WAKE_ALL`，让 net I/O wake 入队后唤醒所有 parked workers，并与 `YLONG_RUNTIME_IO_GLOBAL_FRONT=1`、`YLONG_RUNTIME_GLOBAL_FIRST=1` 组合运行同一 bounded strict CONNECT 点（日志 `target/https_proxy_bench/strict_runtime_io_wake_all_repeat2.log`）。该实验在第一轮 ylong run 超过 60s 后仍未输出 metrics JSON，已终止并撤回 scratch patch。结论：粗暴 wake-all 会破坏当前 bounded benchmark 的进度，不是可提交的 runtime 方向；后续仍需要更精细的 I/O wake ownership / worker 恢复设计，而不是扩大唤醒范围。

补充 runner 超时保护：`run_https_proxy_bench.sh` 现在支持 `BENCH_CLIENT_TIMEOUT=SECONDS`，默认 `0` 保持原 fail-fast 行为。设置超时后，每个 client command 会被单独限时；若某轮 client 失败或超时，runner 仍会提取该轮已输出的 metrics JSON、继续打印最终 `bench_summary`，随后以第一个 client 非 0 状态退出。该路径也修正了原 `if pipeline; then ... fi; status=$?` 会丢失失败 pipeline 状态的问题，确保 timeout 或 client 非 0 返回不会被误记为成功。`bench_environment` 新增 `client_timeout_s` 字段记录该设置。该改动不改变 formal pass 判定，只用于让类似 wake-all 这种会卡住 bounded benchmark 的 runtime 实验保留可审计日志和 summary。

补充 runtime env 记录：`bench_environment` 现在新增 `ylong_runtime_env` 对象，按名称排序记录当前进程中所有 `YLONG_RUNTIME_*` 环境变量。此前 runtime wake/queue probe 需要靠手工描述 `YLONG_RUNTIME_IO_GLOBAL_FRONT`、`YLONG_RUNTIME_GLOBAL_FIRST`、`YLONG_RUNTIME_TRACE` 等组合；该字段把调度实验开关直接写入同一条环境 JSON，避免后续复测日志和实际 runtime knob 脱节。该改动不改变 client 行为或 `formal_pass` 判定，只提升性能证据的可审计性。

用该字段复测一个组合 runtime probe：在 ignored scratch runtime `target/ylong_runtime_trace` 上同时设置 `YLONG_RUNTIME_IO_LAST_WORKER=1`、`YLONG_RUNTIME_IO_GLOBAL_FRONT=1`、`YLONG_RUNTIME_GLOBAL_FIRST=1`、`YLONG_RUNTIME_DEQUEUE_AFTER_DRIVER=1`，验证“last-run worker/global-front 降低 p99”与“driver 后立即取队列”叠加是否能补足吞吐。bounded strict CONNECT 日志为 `target/https_proxy_bench/strict_runtime_combo_dequeue_after_driver.log`，`bench_environment.ylong_runtime_env` 已记录完整 knob：

| Mode | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | request pending gap p99 |
| --- | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: |
| last-worker + global-front + global-first + dequeue-after-driver | 3750.575 | 3454.957 | +8.558% | 0/2 | false | 28.254ms | 25.112ms | 25.130-26.235ms |

该组合把 `source_dequeue_local` 样本归零，`source_dequeue_global` avg/max 为约 `0.88-1.17ms / 9.81-12.72ms`，request Pending gap p99 也低于此前 35ms 级基线；但 migrated samples 仍约 113/114，吞吐平均只有 +8.6%，没有接近 +20%。结论：把 I/O wake 从 local queue 改入 global/front、并让 driver 后立即取队列，只能改善一部分 p99，不能单独成为完成路径；剩余 gap 更像 global queue 竞争、worker migration 和 TLS retry 恢复组合问题。

继续拆分该组合的 instrumentation 开销。先去掉 ylong benchmark 的 `--trace-summary`，但保留 `YLONG_RUNTIME_TRACE` runtime enqueue/dequeue 统计，日志 `target/https_proxy_bench/strict_runtime_combo_dequeue_after_driver_notrace.log`：ylong 平均 3458.045 rps，libcurl 平均 3409.270 rps，平均提升 +1.359%，`passes=0/3`、`formal_pass=false`。随后完全去掉 `YLONG_RUNTIME_TRACE`，只保留行为开关 `YLONG_RUNTIME_IO_LAST_WORKER=1`、`YLONG_RUNTIME_IO_GLOBAL_FRONT=1`、`YLONG_RUNTIME_GLOBAL_FIRST=1`、`YLONG_RUNTIME_DEQUEUE_AFTER_DRIVER=1`，日志 `target/https_proxy_bench/strict_runtime_combo_dequeue_after_driver_behavior_only.log`：ylong 平均 3487.707 rps，libcurl 平均 3274.502 rps，平均提升 +6.631%，`passes=0/3`、`formal_pass=false`。结论不变：即使排除 ylong request trace 和 runtime trace 统计开销，这组 runtime 调度行为也不能稳定达到 +20%。

补充 HTTP/1 request 临时 buffer 负实验：代码复核时注意到 async HTTP/1 `request()` 每次都会分配并清零一个 16 KiB `Vec` 作为 request encode / response header decode 临时 buffer。为验证该 per-request allocation 是否是 strict CONNECT 的剩余吞吐缺口，临时把它改为 `[0u8; 16 * 1024]` 固定数组，使 buffer 随 request future 存放而不是单独 heap allocation。该改动已撤回，因为它会增大 async request future 尺寸，且 bounded strict CONNECT 结果没有接近目标。日志 `target/https_proxy_bench/strict_stackbuf_probe_repeat3.log`：

| Probe | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg |
| --- | ---: | ---: | ---: | ---: | --- | ---: | ---: |
| HTTP/1 fixed temp buffer, warmup 64 | 3472.274 | 3401.820 | +2.270% | 0/3 | false | 30.984ms | 24.768ms |

结论：去掉 HTTP/1 16 KiB 临时 buffer 的单独 heap allocation 不是当前 +20% 完成路径；在没有稳定吞吐收益的情况下，不应为这个目标保留会放大 async future 尺寸的改动。当前 strict CONNECT 缺口仍集中在 request/response 首字节等待和 ylong_runtime I/O wake / task resume 分布，而不是该请求级临时 buffer allocation。

补充 HTTP/1 response temp-buffer size 负实验：为验证 response header/status 首次读取只能携带 16 KiB pre-body 是否拖慢 strict CONNECT，临时把 async HTTP/1 `TEMP_BUF_SIZE` 从 16 KiB 调到 64 KiB，并在同一 bounded strict CONNECT 点运行 `REPEAT=2`、`warmup=64`、`read_buffer_size=64 KiB`、`--phase-summary`（日志 `target/https_proxy_bench/strict_tempbuf64_probe_repeat2.log`）：

| Probe | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg |
| --- | ---: | ---: | ---: | ---: | --- | ---: | ---: |
| HTTP/1 temp buffer 64 KiB | 3559.278 | 3408.282 | +4.316% | 0/2 | false | 31.931ms | 23.882ms |

同轮 ylong `response_wait_p99_us` 仍为约 28ms，libcurl `starttransfer_us_p99` 为约 21-22ms。扩大 HTTP/1 首次读取 buffer 没有把 first-byte 长尾拉近 libcurl，也会增加每个 request future 的 per-request allocation；该改动已撤回。

补充 libcurl phase timing 诊断字段：为让后续 libcurl baseline 不只是一个总延迟数字，本轮扩展 `libcurl_harness.c`，在每个成功 measured request 后读取 `CURLINFO_CONNECT_TIME_T`、`CURLINFO_APPCONNECT_TIME_T`、`CURLINFO_STARTTRANSFER_TIME_T` 和 `CURLINFO_TOTAL_TIME_T`，并输出 p50/p90/p99；`body_transfer_us_*` 由 `total - starttransfer` 计算。该字段只用于诊断，不参与 `bench_summary.formal_pass` 判定。

smoke 结果：

| Log | Workload | 结果 |
| --- | --- | --- |
| `target/https_proxy_bench/libcurl_phase_fields_smoke.log` | direct libcurl harness, `requests=8`, `warmup=4`, `concurrency=2` | JSON 含 `starttransfer_us_p99=97`、`total_time_us_p99=1168`、`body_transfer_us_p99=1026` |
| `target/https_proxy_bench/libcurl_phase_runner_smoke.log` | runner, async-ylong + libcurl, `requests=8`, `warmup=4`, `concurrency=2` | runner 正常输出 `bench_summary`，libcurl metrics 保留新增 phase 字段 |

同一 bounded strict CONNECT 点做 3-run refresh（native fixture、`requests=128`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`，日志 `target/https_proxy_bench/strict_libcurl_phase_repeat3.log`）：

| Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | libcurl starttransfer p99 | libcurl body-transfer p99 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 3577.300 | 3341.426 | +7.059% | 29.286ms | 29.334ms | 26.286ms | 8.082ms |
| 2 | 3258.389 | 3428.051 | -4.949% | 33.414ms | 25.974ms | 21.266ms | 9.107ms |
| 3 | 3504.397 | 3312.544 | +5.792% | 31.422ms | 26.122ms | 24.192ms | 13.283ms |

`bench_summary.formal_pass=false`，平均提升只有 +2.634%。Libcurl 在 warm connection 下 `connect_us_p99=0`、`appconnect_us_p99=0`，其 p99 总延迟主要落在 `starttransfer`，body-transfer p99 明显更小。该结果与 ylong trace 中反复出现的 request first-byte / Pending-resume 长尾一致：严格 CONNECT 的剩余缺口继续优先指向请求写完到首字节之间的 runtime/TLS readiness 恢复，而不是 connection setup 或单纯 body drain。新增字段不会让当前 strict CONNECT 自动通过；它把后续正式/trace 对比中的 libcurl baseline 拆成 connect/appconnect、首字节和 body transfer 三段，避免继续只凭 ylong 的 `request_trace_summary` 推断 libcurl 在哪个阶段更快。

补充 ylong 轻量 phase summary：为避免每次看 ylong 分段耗时都必须开启重型 `--trace-summary`，本轮给 async benchmark 新增 ylong-only `--phase-summary`。该模式不记录 future poll Pending gap，只在成功 measured request 后读取 response `TimeGroup` 中的 connect / request-write / response-wait / transfer duration，并记录 body first-byte / body-drain 分布；runner 会过滤该参数，不传给 libcurl。默认不启用，因此不改变正式 no-trace 验收路径。

smoke 日志 `target/https_proxy_bench/phase_summary_runner_smoke.log` 验证了 runner 会输出独立 `phase_summary` JSON，且 `bench_summary` 仍只读取 client metrics 行。随后在同一 bounded strict CONNECT 点做 3-run refresh（日志 `target/https_proxy_bench/strict_phase_summary_repeat3.log`）：

| Run | ylong rps | libcurl rps | 提升 | ylong response-wait p99 | libcurl starttransfer p99 | ylong body-drain p99 | libcurl body-transfer p99 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 3504.155 | 3091.713 | +13.340% | 26.370ms | 23.259ms | 17.679ms | 13.368ms |
| 2 | 3345.980 | 3452.834 | -3.095% | 25.990ms | 23.163ms | 14.536ms | 8.311ms |
| 3 | 3386.015 | 3233.058 | +4.731% | 29.616ms | 22.665ms | 23.350ms | 8.507ms |

`bench_summary.formal_pass=false`，平均提升 +4.992%。轻量 phase 结果与前面的 libcurl phase 字段对齐：ylong warm connection 下 connect p99 只有 4-7us，request-write 大多很小但偶有 ms 级尾部，主要长尾仍在 response-wait；body-drain p99 也有波动，但不是唯一差距。该证据继续支持“先把 request first-byte / response-wait resume 分布做下来，再谈 +20%”的方向，同时给后续 runtime A/B 提供一个比 `--trace-summary` 更低干扰的阶段指标。

补充 `ssl_trace_preload` max-sample 字段：OpenSSL preload 诊断现在会在每类 WANT retry gap 的最大样本上输出 `*_max_ssl`、`*_max_want_thread`、`*_max_retry_thread` 和 `*_max_same_thread`。这不改变 benchmark 或客户端行为，只是把此前“最大 gap 是哪个 SSL 对象、是否同一 pthread 恢复”的证据保留下来，便于后续区分单一 TLS 层反复长尾和广泛 worker migration。

smoke 验证：

| Log | Workload | 结果 |
| --- | --- | --- |
| `target/https_proxy_bench/ssl_trace_preload_smoke.jsonl` | `LD_PRELOAD` + `openssl version` | JSON shape 正常，新增字段在无 SSL traffic 时为 `0x0` / `0` |
| `target/https_proxy_bench/ssl_trace_max_sample_smoke.jsonl` | native HTTPS proxy fixture，async-ylong，`requests=16`、`warmup=8`、`concurrency=4` | nested TLS traffic 下 max-sample 字段有实际 SSL pointer 和 pthread id |

代表性 smoke 字段：

| Field | Value |
| --- | --- |
| `ssl_retry_gap_read_depth1_samples` | 540 |
| `ssl_retry_gap_read_depth1_max_us` | 1770 |
| `ssl_retry_gap_read_depth1_max_same_thread` | false |
| `ssl_retry_gap_read_depth2_samples` | 527 |
| `ssl_retry_gap_read_depth2_max_us` | 758 |
| `ssl_retry_gap_read_depth2_max_same_thread` | true |
| `ssl_retry_gap_connect_samples` | 8 |
| `ssl_retry_gap_connect_max_us` | 1768 |
| `ssl_retry_gap_connect_max_same_thread` | false |

该 smoke 太小，不作为性能结论；它只证明新增字段可用于下一轮 strict CONNECT/runtime A-B 复测。当前严格 HTTPS proxy CONNECT 的 20%+ 目标仍未完成。

随后用相同 preload 字段跑一组 strict-shaped 单轮对照（native HTTPS proxy fixture、`requests=128`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`；日志：
`target/https_proxy_bench/strict_ssl_max_sample_ylong.log`、
`target/https_proxy_bench/strict_ssl_max_sample_ylong.jsonl`、
`target/https_proxy_bench/strict_ssl_max_sample_libcurl.log`、
`target/https_proxy_bench/strict_ssl_max_sample_libcurl.jsonl`）：

| Client | rps | p99 / first-byte p99 | depth1 max | depth1 migrated | depth2 max | depth2 migrated | connect max | connect migrated |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ylong async-ylong | 3188.537 | p99 34.179ms / response-wait 31.863ms | 45.175ms | 605 / 1150 | 32.614ms | 451 / 968 | 52.212ms | 118 / 128 |
| libcurl | 3235.510 | p99 24.454ms / start-transfer 23.110ms | 36.684ms | 0 / 1108 | 24.281ms | 0 / 936 | 36.610ms | 0 / 127 |

本轮新增 max-sample 字段显示：ylong 的最大 depth1、depth2、connect retry gap 都跨 pthread 恢复，且最大样本分别落在不同 `SSL*`；libcurl 同 workload 下所有 retry gap 都是 same-thread，migrated 样本为 0。libcurl 也有 24-36ms 级 same-thread retry gap，因此“线程迁移”不是唯一延迟来源；但 ylong 的 first-byte p99 更高，且迁移覆盖 inner TLS、outer TLS 和 handshake 三类边界，继续支持剩余缺口是 ylong runtime I/O wake/task resume ownership 问题，而不是单一 proxy TLS 层、单一 OpenSSL read-ahead 参数或 HTTP/1 buffer 大小问题。

补充 per-worker current-thread runtime 负实验：为验证“libcurl 每个 worker 在固定 OS thread 上推进请求，因此 ylong 可以通过 current-thread runtime 消除迁移尾部”的假设，async-ylong benchmark 增加了 ylong-only `--runtime-mode current-thread-per-worker`。该模式只在显式传参时通过内部 feature 启用 `ylong_runtime/current_thread_runtime`，每个 OS worker thread 创建一个 current-thread runtime，并强制 `client_per_worker=true`；runner 会要求 `YLONG_CLIENT=async-ylong`，不把该参数传给 libcurl。

在同一 bounded strict CONNECT 点运行 `REPEAT=2`、`warmup=64`、`concurrency=64`、`read_buffer_size=64 KiB`、`--phase-summary`（日志 `target/https_proxy_bench/strict_current_thread_per_worker_probe_repeat2.log`）：

| Run | ylong rps | libcurl rps | 提升 | ylong p99 | libcurl p99 | ylong response-wait p99 | libcurl starttransfer p99 | worker start-delay max |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 2447.828 | 3457.310 | -29.198% | 20.516ms | 22.921ms | 19.152ms | 20.824ms | 30.906ms |
| 2 | 2314.987 | 3395.766 | -31.827% | 11.345ms | 28.765ms | 9.707ms | 25.916ms | 39.289ms |

`bench_summary.formal_pass=false`，平均提升 -30.513%。current-thread 模式确实把 ylong 请求/response-wait p99 拉低，但 64 个 current-thread runtime/OS thread 的启动与调度尾部把总 elapsed 拉长，吞吐显著低于 libcurl；该方向不能作为 strict HTTPS proxy CONNECT 的 +20% 完成路径。当前结论保持不变：需要在 ylong_runtime 的 multi-thread I/O wake / queue ownership / task resume 策略上降低 first-byte 尾延迟，而不是在客户端 benchmark 内改成每 worker 独立 current-thread runtime。

补充 response-size scaling 复核：为避免把 strict CONNECT 的 1 MiB 失败继续误判为“纯 first-byte 问题”，在同一 HTTPS origin over HTTPS proxy native fixture 上固定 `concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`，只改变响应体大小并启用轻量 `--phase-summary`。

| Response | requests | ylong avg rps | libcurl avg rps | 平均提升 | passes | ylong p99 avg | libcurl p99 avg | ylong body-drain p99 | libcurl body-transfer p99 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 B | 512 | 80604.394 | 60664.544 | +33.130% | 3/3 | 2.578ms | 1.517ms | 0.411ms | 0.286ms |
| 64 KiB | 512 | 30354.042 | 25883.181 | +17.014% | 2/3 | 6.900ms | 4.456ms | 2.466ms | 1.001ms |
| 256 KiB | 256 | 10796.494 | 9615.676 | +12.226% | 1/3 | 14.491ms | 10.409ms | 8.338ms | 4.010ms |
| 1 MiB | 128 | 3412.050 | 3259.202 | +4.992% | 0/3 | 30.583ms | 24.523ms | 18.522ms | 10.062ms |

日志：

- `target/https_proxy_bench/strict_resp1_first_byte_repeat3.log`
- `target/https_proxy_bench/strict_resp64k_repeat3.log`
- `target/https_proxy_bench/strict_resp256k_repeat3.log`
- `target/https_proxy_bench/strict_rb64_repeat5_current.log`

结论更新：ylong 在 tiny response strict CONNECT 上已经有明显吞吐优势，说明代理 TLS 配置、CONNECT 建链、请求写入和基础 first-byte 路径不是整体缺口的唯一解释；但随着 response body 从 1 B 放大到 64 KiB / 256 KiB / 1 MiB，优势快速被 body transfer 阶段吞掉。接下来的有效方向应同时约束两件事：保住小响应场景的 first-byte 优势，并降低大响应 body drain 的 tail/elapsed 成本。单纯继续调 CONNECT 建链、current-thread runtime 或只看 first-byte trace 都不足以完成原始 20%+ 目标。

补充 current-tree 512-request 复核：为排除 128-request 短测噪声，在当前源码下固定 `response_size=1 MiB`、`concurrency=64`、`runtime_threads=16`，将 measured requests 提高到 512，并关闭 trace/phase instrumentation。

| read buffer | ylong avg rps | libcurl avg rps | 平均提升 | passes | ylong p99 avg | libcurl p99 avg | 日志 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 64 KiB | 3529.143 | 3525.276 | +0.079% | 0/3 | 45.794ms | 30.609ms | `target/https_proxy_bench/strict_resp1m_requests512_repeat3.log` |
| 1 MiB | 3435.624 | 3621.780 | -5.082% | 0/3 | 45.741ms | 29.112ms | `target/https_proxy_bench/strict_resp1m_requests512_rb1m_repeat3.log` |

结论：更长的 1 MiB workload 没有隐藏 +20% 通过样本，反而显示 current-tree strict CONNECT 大响应吞吐基本与 libcurl 持平或落后，且 ylong p99 仍显著弱于 libcurl。把应用层 read buffer 从 64 KiB 放大到 1 MiB 会减少 ylong body read 次数，但不能改善吞吐；继续单独合并 body reads 不是完成路径。

补充 internal `InterceptorContext` 复测：此前扩大 interceptor skip 的短测有吞吐信号，但要求给 public `Interceptor` trait 增加 enablement 方法，因此已撤回。本轮改为内部封装：默认 client 持有 `InterceptorContext::none()`，热路径在没有用户 interceptor 时直接跳过 no-op 动态分发；用户通过 builder 安装 interceptor 时仍按原 trait 方法完整调用。该变更不新增 public API。

| Workload | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | 日志 |
| --- | ---: | ---: | ---: | ---: | --- | ---: | ---: | --- |
| 128 requests, `--phase-summary` | 3628.778 | 3334.432 | +9.043% | 0/3 | false | 28.991ms | 25.854ms | `target/https_proxy_bench/strict_internal_interceptor_context_repeat3.log` |
| 512 requests, no trace/phase | 3655.914 | 3467.766 | +5.537% | 0/3 | false | 44.215ms | 35.256ms | `target/https_proxy_bench/strict_internal_interceptor_context_requests512_repeat3.log` |

结论：内部默认 no-op skip 是比 public trait enablement 更小的可保留 CPU cleanup，但它没有改变严格 1 MiB CONNECT 的目标状态：短测平均只有 +9.0%，512-request 复核只有 +5.5%，均为 `formal_pass=false`，且 ylong p99 仍弱于 libcurl。后续完成 +20% 仍需解决大响应 body drain / runtime I/O resume 分布，而不是继续扩大 interceptor 路径。

补充 HTTP/1 pre-body buffer ownership 负实验：为验证 response header decode 后 `pre` body slice 的 `to_vec()` 是否是大响应 drain 的剩余开销，临时让 `HttpBody` 直接持有 HTTP/1 16 KiB 临时 buffer 和 body 起始 offset，避免为 `pre` 再分配和复制一份 `Vec`。该改动已撤回，因为 512-request 复核没有收益。

| Workload | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | 日志 |
| --- | ---: | ---: | ---: | ---: | --- | ---: | ---: | --- |
| 128 requests, `--phase-summary` | 3627.250 | 3369.298 | +7.685% | 0/3 | false | 29.013ms | 24.737ms | `target/https_proxy_bench/strict_prebody_buffer_repeat3.log` |
| 512 requests, no trace/phase | 3544.848 | 3548.416 | -0.056% | 0/3 | false | 42.300ms | 29.270ms | `target/https_proxy_bench/strict_prebody_buffer_requests512_repeat3.log` |

结论：去掉 HTTP/1 `pre` slice 构造时的一次分配/复制不是 strict 1 MiB CONNECT 的完成路径；长跑平均基本持平且 ylong p99 仍明显弱于 libcurl。保持现有更简单的 `Cursor<Vec<u8>>` pre-buffer 结构，避免为无收益的微优化增加 body 状态复杂度。

补充 Tokio backend 与 runtime inject-worker 复核：为确认当前缺口是否可以通过切换 async runtime 或把 net I/O wake 定向回 task 上次运行 worker 来绕过，本轮在同一 bounded strict CONNECT 点固定 `response_size=1 MiB`、`requests=128`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`、`REPEAT=3`。Tokio backend 使用当前仓库源码；inject-worker 两组只使用 ignored scratch runtime `target/ylong_runtime_trace`，没有进入当前仓库源码。

| Mode | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | 日志 |
| --- | ---: | ---: | ---: | ---: | --- | ---: | ---: | --- |
| Tokio backend, current source | 3290.430 | 3382.040 | -2.701% | 0/3 | false | 32.666ms | 24.918ms | `target/https_proxy_bench/strict_tokio_current_rt16_rb64_repeat3.log` |
| `YLONG_RUNTIME_IO_INJECT_WORKER=1` | 2892.564 | 3382.902 | -14.323% | 0/3 | false | 41.817ms | 25.156ms | `target/https_proxy_bench/strict_runtime_io_inject_worker_repeat3.log` |
| `YLONG_RUNTIME_IO_INJECT_WORKER=1` + idle-only | 3467.705 | 3359.207 | +3.209% | 0/3 | false | 31.433ms | 24.903ms | `target/https_proxy_bench/strict_runtime_io_inject_idle_only_repeat3.log` |

结论：Tokio backend 在当前源码下不能作为 strict HTTPS proxy CONNECT 的 20%+ 完成口径；它的 p99 仍明显弱于 libcurl。直接把 net I/O wake 注入 task 上次运行 worker 的专用队列会显著退化吞吐和 p99，说明“恢复到 last-run worker”不能不加约束地替代现有队列策略。idle-only 注入避免了最坏退化，但平均只有 +3.2%，仍远低于目标。后续 runtime 方向应避免粗粒度 worker ownership 强制迁移，转向更细的 wake-to-run 延迟削减或减少大响应 body 阶段对其他 request 的恢复干扰。

补充 body-read 后显式 yield 负实验：为验证 benchmark/application drain loop 是否在每个 ready body chunk 后立刻再次 await `response.data()`、从而压制其他 request future 的恢复，本轮给 async benchmark 增加 ylong-only `--yield-after-body-read`。该参数只在 ylong 客户端每次成功读取 body chunk 后调用 runtime `yield_now().await`，runner 不传给 libcurl；默认不启用，因此不改变正式验收路径。

在同一 bounded strict CONNECT 点固定 `response_size=1 MiB`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`、`REPEAT=3`：

| Workload | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | 日志 |
| --- | ---: | ---: | ---: | ---: | --- | ---: | ---: | --- |
| 128 requests, `--yield-after-body-read` | 3480.335 | 3293.461 | +5.679% | 0/3 | false | 29.588ms | 26.418ms | `target/https_proxy_bench/strict_yield_after_body_read_repeat3.log` |
| 512 requests, `--yield-after-body-read` | 3400.481 | 3572.536 | -4.780% | 0/3 | false | 40.128ms | 30.457ms | `target/https_proxy_bench/strict_yield_after_body_read_requests512_repeat3.log` |

结论：应用层每个 body read 后显式让出执行权不是 strict 1 MiB CONNECT 的完成路径。短测仍只有 +5.7%，512-request 复核转为落后 libcurl 4.8%，且 ylong p99 仍高出约 10ms。后续不应在 benchmark 层插入固定 yield 作为性能修复；若继续优化 body drain，应定位 runtime I/O wake-to-run 延迟、OpenSSL read loop 批量化和大响应阶段的公平性边界，而不是让应用每个 chunk 主动让步。

补充 ylong runtime worker affinity 负实验：为验证“worker 迁移导致 strict CONNECT 首字节/大响应 tail”的剩余假设，使用现有 benchmark `--runtime-affinity` 开关启用 ylong runtime worker core affinity。该实验不改客户端源码，只测试 runtime worker 固定到 CPU 后是否能降低迁移相关尾延迟。

同一 bounded strict CONNECT 点固定 `response_size=1 MiB`、`requests=128`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`、`REPEAT=3`、`--phase-summary`（日志 `target/https_proxy_bench/strict_runtime_affinity_repeat3.log`）：

| Mode | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | ylong response-wait p99 | libcurl starttransfer p99 |
| --- | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: |
| `--runtime-affinity` | 3424.273 | 3405.353 | +0.620% | 0/3 | false | 29.774ms | 25.329ms | 27.139ms | 22.943ms |

结论：固定 ylong runtime worker affinity 不能作为 strict HTTPS proxy CONNECT 的完成路径。它没有稳定吞吐收益，ylong p99 和 response-wait p99 仍弱于 libcurl；因此剩余问题不是简单的 OS CPU 迁移开关可以解决，仍需要 runtime I/O wake-to-run / task ownership 策略级别的改动或新的可验证客户端机制。

补充 current-thread sharded runtime 负实验：此前 `current-thread-per-worker` 能降低部分 response-wait 尾部，但 64 个 current-thread runtime/OS thread 导致总吞吐明显退化。本轮增加 `--runtime-mode current-thread-sharded` 诊断模式：用 `--runtime-threads` 个 current-thread runtime 承载 64 个 benchmark worker，并为共享同一 current-thread runtime 的 worker 使用 async start gate，避免 blocking barrier 在单线程 runtime 内造成死锁。该模式只影响 benchmark harness，不改客户端语义；runner 会自动为该模式启用 `__ylong_current_thread_runtime`。

同一 bounded strict CONNECT 点固定 `response_size=1 MiB`、`requests=128`、`warmup=64`、`concurrency=64`、`read_buffer_size=64 KiB`、`--phase-summary`：

| Mode | repeats | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | 日志 |
| --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | --- |
| `current-thread-sharded`, 16 shards | 3 | 3107.595 | 3369.871 | -7.785% | 0/3 | false | 31.180ms | 25.847ms | `target/https_proxy_bench/strict_current_thread_sharded_repeat3.log` |
| `current-thread-sharded`, 8 shards | 1 | 3168.885 | 3289.558 | -3.668% | 0/1 | false | 29.284ms | 25.033ms | `target/https_proxy_bench/strict_current_thread_sharded_rt8_probe.log` |
| `current-thread-sharded`, 32 shards | 1 | 2921.735 | 3320.708 | -12.015% | 0/1 | false | 27.575ms | 27.295ms | `target/https_proxy_bench/strict_current_thread_sharded_rt32_probe.log` |

结论：把 current-thread runtime 从“每 worker 一个”改成分片共享仍不能完成 strict HTTPS proxy CONNECT 的 20%+ 目标。8/16/32 shards 均没有通过样本；16-shard repeat3 平均落后 libcurl 7.8%，p99 仍弱于 libcurl。该结果说明剩余缺口不是简单通过隔离/分片 current-thread runtime 就能解决；继续方向应回到 ylong runtime multi-thread I/O wake-to-run、任务恢复队列策略或大响应 body drain 阶段的公平性，而不是继续在 benchmark 层切分 runtime。

补充 runtime I/O wake round-robin injection 负实验：此前 local queue、global queue、last-worker injection、idle-only injection 都不能完成 strict CONNECT 目标。本轮在 ignored scratch runtime `target/ylong_runtime_trace` 中新增 `YLONG_RUNTIME_IO_INJECT_ROUND_ROBIN=1` 诊断：I/O readiness wake 不再把任务排到当前 driver worker 的 local/LIFO 队列，也不强制回 last-run worker，而是轮询投递到每个 worker 的 injection queue 并唤醒对应 worker。随后又临时验证了 injection queue `push_front` 变体；当前 scratch runtime 保留为 `YLONG_RUNTIME_IO_INJECT_ROUND_ROBIN_FRONT=1` 可复现的开关。

同一 bounded strict CONNECT 点固定 `response_size=1 MiB`、`requests=128`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`、`--phase-summary`：

| Runtime experiment | repeats | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | 日志 |
| --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | --- |
| I/O inject round-robin, injection back | 2 | 3310.838 | 3347.373 | -0.958% | 0/2 | false | 32.118ms | 23.988ms | `target/https_proxy_bench/strict_runtime_io_inject_round_robin_repeat2.log` |
| I/O inject round-robin, injection front | 2 | 3258.189 | 3349.158 | -2.712% | 0/2 | false | 35.428ms | 27.531ms | `target/https_proxy_bench/strict_runtime_io_inject_round_robin_front_repeat2.log` |

诊断价值：round-robin injection 基本消除了 runtime trace 中的 local queue migrated-worker 样本，但 tail 没有消失；I/O-woken task 改为 `inject` 队列后，`source_dequeue_inject` 仍出现 16-47ms 级最大排队延迟。`push_front` 变体反而扩大 p99。结论：剩余 strict CONNECT 缺口不是单纯“把 I/O wake 分散到 worker”即可解决；需要继续定位 injection/global/local 队列之后的 worker 恢复时机、park/unpark 与 driver polling 交互，或者设计更细的 I/O-ready task 优先级，而不是继续换队列容器。

补充 current-worker injection 负实验：为区分“轮询分散投递造成跨 worker 抢占”与“同一 I/O driver worker 排队过深”，在 ignored scratch runtime `target/ylong_runtime_trace` 中新增 `YLONG_RUNTIME_IO_INJECT_CURRENT=1` 诊断：I/O readiness wake 发生在 worker 上下文时，把 task 投递到当前 worker 的 private injection queue 并唤醒该 worker；`YLONG_RUNTIME_IO_INJECT_CURRENT_FRONT=1` 可切换为队头插入。本轮只复测 injection back。

本轮证书复测还修正了一个无效样本：`target/https_proxy_bench/strict_runtime_io_inject_current_repeat2.log` 误用 `server.pem` 作为 trust anchor；当前 fixture 证书已由本地 CA 签发，因此 ylong 侧证书校验失败，`completed=0/errors=384`，该日志不作为性能结论。正确复测使用 `target/https_proxy_bench/certs/ca.pem` 作为 `--proxy-ca-file` 和 `--origin-ca-file`。

同一 bounded strict CONNECT 点固定 `response_size=1 MiB`、`requests=128`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`、`--phase-summary`：

| Runtime experiment | repeats | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | 日志 |
| --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | --- |
| I/O inject current-worker, injection back | 2 | 3264.184 | 3393.729 | -3.836% | 0/2 | false | 34.290ms | 26.749ms | `target/https_proxy_bench/strict_runtime_io_inject_current_ca_repeat2.log` |

诊断价值：current-worker injection 让 `inject` 队列样本全部保持 `same_worker`（两轮分别 558/558、641/641，`migrated_worker=0`），但 `inject` 最大 wake-to-run 仍为 22.754ms / 24.644ms，第二轮 `source_periodic` 最大也达到 24.644ms。ylong p99 仍弱于 libcurl，吞吐平均落后 3.8%。结论：把 I/O-woken task 固定到当前 worker 可以消除迁移样本，但不能消除 busy worker / periodic / dequeue 路径后的排队尾部；strict CONNECT 的剩余缺口仍在 runtime worker 恢复时机和 I/O-ready task 优先级，而不是单纯 worker migration。

补充 injection-before-LIFO 与 periodic modulo 负实验：继续在 ignored scratch runtime `target/ylong_runtime_trace` 中拆分两个队列优先级假设。

- `YLONG_RUNTIME_INJECT_BEFORE_LIFO=1`：worker 在 LIFO slot 之前先检查 private injection queue，用来验证 current-worker injection 是否仍被 LIFO work 压住。
- `YLONG_RUNTIME_PERIODIC_MODULO=1`：把 worker periodic driver check 从现有 bitwise `count & 61 == 0` 改为与 global queue polling 一致的 `count % 61 == 0`，用来验证 periodic driver 频率是否是 strict CONNECT 抖动来源。

同一 bounded strict CONNECT 点固定 `response_size=1 MiB`、`requests=128`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`、`--phase-summary`：

| Runtime experiment | repeats | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | 日志 |
| --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | --- |
| I/O inject current-worker + inject before LIFO | 2 | 3166.226 | 3449.262 | -8.117% | 0/2 | false | 35.868ms | 25.831ms | `target/https_proxy_bench/strict_runtime_io_inject_current_before_lifo_repeat2.log` |
| Periodic driver check uses modulo | 5 | 3498.597 | 3408.924 | +2.683% | 0/5 | false | 30.472ms | 25.335ms | `target/https_proxy_bench/strict_runtime_periodic_modulo_repeat5.log` |

诊断价值：inject-before-LIFO 反而退化，且 `source_dequeue_inject` / `source_park` 仍出现 21-25ms 级最大排队延迟，说明 LIFO 优先级不是 current-worker injection 的主要剩余阻塞点。periodic modulo 的短 repeat2 曾出现一个 +25.8% paired sample，但 repeat5 没有任何 +20% pass，平均只有 +2.7%，p99 仍弱于 libcurl；因此 bitwise-vs-modulo periodic check 也不能作为完成路径。当前结论保持：需要更直接地降低 I/O-ready task 入队后的恢复尾部，或重新设计大响应阶段的 runtime fairness，而不是继续调 LIFO/periodic 这类单点优先级。

补充 ylong body read chunk cap 诊断：为验证大响应阶段是否因 ylong 单次 `response.data()` 使用 64 KiB buffer、而 libcurl 实际更接近 16 KiB body read 粒度导致公平性/尾部差异，本轮给 benchmark 增加 ylong-only `--ylong-read-chunk-size N`。该参数只限制传给 ylong `response.data()` 的 slice 长度，libcurl 仍保持 `--read-buffer-size` 对应的 `CURLOPT_BUFFERSIZE`；默认不传该参数，正式 same-buffer 对比仍按 `--read-buffer-size` 执行。

同一 bounded strict CONNECT 点固定 `response_size=1 MiB`、`requests=128`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`、`--phase-summary`：

| ylong read chunk cap | repeats | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | 日志 |
| ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | --- |
| 8 KiB | 2 | 3661.604 | 3485.614 | +5.074% | 0/2 | false | 29.939ms | 23.846ms | `target/https_proxy_bench/strict_ylong_read_chunk8k_repeat2.log` |
| 16 KiB | 3 | 3581.012 | 3399.521 | +5.354% | 0/3 | false | 29.344ms | 24.084ms | `target/https_proxy_bench/strict_ylong_read_chunk16k_repeat3.log` |
| 32 KiB | 2 | 3607.366 | 3284.244 | +9.924% | 0/2 | false | 29.840ms | 26.353ms | `target/https_proxy_bench/strict_ylong_read_chunk32k_repeat2.log` |

诊断价值：限制 ylong body read chunk 能降低部分 body first-byte/body-drain p99，并在短 repeat 中带来 5-10% 的吞吐信号，其中 32 KiB 是本组最好样本；但全部样本仍为 0 pass，`formal_pass=false`，ylong response-wait/p99 仍弱于 libcurl。该结果说明 body read 粒度会影响 strict 1 MiB CONNECT 的公平性，但 ylong-only chunk cap 既不是正式 same-buffer 口径，也不能完成 20%+ 目标；后续仍需回到 runtime I/O-ready task 恢复尾部、OpenSSL read loop 批量化边界或更系统的大响应公平性设计。

补充 partial-signal 叠加复测：上面几个方向各自有小幅正向信号，但都不能单独完成目标。本轮只复测三个组合，验证这些信号是否能叠加出稳定 20%+：

- ylong-only `--ylong-read-chunk-size 32768`，保留 `read_buffer_size=64 KiB`。
- ignored scratch runtime `target/ylong_runtime_trace` 上的 `YLONG_RUNTIME_IO_LAST_WORKER=1`、`YLONG_RUNTIME_IO_GLOBAL_FRONT=1`、`YLONG_RUNTIME_GLOBAL_FIRST=1`、`YLONG_RUNTIME_DEQUEUE_AFTER_DRIVER=1`。
- `runtime_threads=32`。

同一 bounded strict CONNECT 点固定 `response_size=1 MiB`、`requests=128`、`warmup=64`、`concurrency=64`、`read_buffer_size=64 KiB`、`--phase-summary`：

| Mode | repeats | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | max/min 提升 | 日志 |
| --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | --- |
| runtime combo + 32 KiB chunk, rt16 | 3 | 3633.735 | 3306.846 | +10.423% | 1/3 | false | 28.298ms | 27.322ms | +24.242% / -0.587% | `target/https_proxy_bench/strict_runtime_combo_read_chunk32k_repeat3.log` |
| current runtime + 32 KiB chunk, rt32 | 3 | 3563.524 | 3315.413 | +7.559% | 0/3 | false | 30.569ms | 28.443ms | +13.669% / +1.523% | `target/https_proxy_bench/strict_rt32_read_chunk32k_repeat3.log` |
| runtime combo + 32 KiB chunk, rt32 | 3 | 3285.402 | 2875.382 | +16.813% | 1/3 | false | 31.774ms | 32.863ms | +44.212% / -9.032% | `target/https_proxy_bench/strict_runtime_combo_rt32_read_chunk32k_repeat3.log` |

诊断价值：`runtime combo + 32 KiB chunk` 在 rt16 下能把平均提升推到约 10%，并出现一个达标样本，但仍只有 `1/3` pass。rt32 与 chunk cap 在当前 runtime 下只有 +7.6%，没有通过样本。rt32 再叠加 scratch runtime combo 的平均值看起来接近目标，但主要来自一次 libcurl baseline 明显变慢的 +44% 样本；同组最差样本为 -9.0%，仍只有 `1/3` pass。结论：这些 partial signals 不能可靠叠加成完成路径；继续把目标押在 ylong-only chunk cap、简单线程数调整或现有 scratch runtime 队列组合上会误导判断。剩余工作仍应转向更系统的 I/O-ready task 优先级和 worker 恢复设计，或者找到新的客户端层机制，并用 repeat-5/正式 pass 规则验证。

补充 current-thread-per-worker 长 workload 复核：此前 `current-thread-per-worker` 在 `requests=128` 下能降低部分 response-wait p99，但 64 个 current-thread runtime / OS worker 的启动与调度尾部使吞吐落后约 30%。本轮把 measured requests 提高到 512，验证该模式是否只是短 workload 中启动开销未摊薄。

同一 bounded strict CONNECT 点固定 `response_size=1 MiB`、`requests=512`、`warmup=64`、`concurrency=64`、`read_buffer_size=64 KiB`、`--runtime-mode current-thread-per-worker`、`--phase-summary`，日志 `target/https_proxy_bench/strict_current_thread_per_worker_requests512_repeat2.log`：

| Mode | repeats | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | ylong worker start-delay max avg |
| --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: |
| current-thread-per-worker, 512 requests | 2 | 1761.555 | 2220.376 | -20.402% | 0/2 | false | 63.885ms | 62.713ms | 155.777ms |

诊断价值：更长 workload 没有摊平 current-thread-per-worker 的成本；ylong 吞吐仍明显低于 libcurl，worker start-delay max 平均仍达到 155.8ms，且 response-wait p99 在第二轮升到 60ms 级。结论：同线程 ownership 本身不能作为 strict CONNECT 完成路径；即使绕开 multi-thread runtime 的部分迁移问题，64 个 current-thread runtime / worker 的启动、调度和 body transfer 组合成本仍过高。后续不应继续把 +20% 目标押在 current-thread-per-worker 或 current-thread-sharded 这类 benchmark runtime 切分上。

补充 dedicated I/O driver 负实验：为验证 remaining gap 是否来自“没有 worker 及时进入 `Driver::run()` 发现 epoll readiness”，本轮在 ignored scratch runtime `target/ylong_runtime_trace` 中新增 `YLONG_RUNTIME_DEDICATED_DRIVER=1` 诊断：multi-thread runtime 创建一个单独 I/O driver thread，循环持有共享 driver 执行 `Driver::run()`；runtime cancel 时额外 wake driver，避免 driver thread 长期阻塞。该改动只用于 runtime path override 复测，没有进入当前仓库源码。

第一轮 dedicated-driver only 短测发现：driver thread 位于 worker context 之外，因此 I/O wake 会走 scheduler 的无 worker context fallback，默认进入 global FIFO；这等价于又把 I/O-ready task 放进全局队列，不能保证优先恢复。为拆分该因素，又补一个 scratch 变体：当无 worker context 的 wake 仍处于 `IO_WAKE_DEPTH` 且 `YLONG_RUNTIME_IO_GLOBAL_FRONT=1` 时，将 task 插入 global front，并叠加 `YLONG_RUNTIME_GLOBAL_FIRST=1` 让 worker 优先取 global queue。

同一 bounded strict CONNECT 点固定 `response_size=1 MiB`、`requests=128`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`：

| Runtime experiment | repeats | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | max/min 提升 | 日志 |
| --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | --- |
| dedicated driver only, phase summary | 2 | 2123.357 | 2091.676 | +1.698% | 0/2 | false | 51.897ms | 44.009ms | +5.542% / -2.146% | `target/https_proxy_bench/strict_runtime_dedicated_driver_repeat2.log` |
| dedicated driver + out-of-worker global-front + global-first, phase summary | 2 | 2111.808 | 1857.700 | +13.833% | 0/2 | false | 54.206ms | 54.357ms | +19.355% / +8.310% | `target/https_proxy_bench/strict_runtime_dedicated_driver_global_front_repeat2.log` |
| dedicated driver + out-of-worker global-front + global-first, no phase | 5 | 2052.922 | 1994.519 | +3.253% | 0/5 | false | 53.968ms | 47.141ms | +11.205% / -6.661% | `target/https_proxy_bench/strict_runtime_dedicated_driver_global_front_repeat5.log` |

验证：

- `rustfmt target/ylong_runtime_trace/ylong_runtime/src/executor/async_pool.rs target/ylong_runtime_trace/ylong_runtime/src/trace.rs`
- `cargo --config 'patch."https://gitcode.com/openharmony/commonlibrary_rust_ylong_runtime.git".ylong_runtime.path="target/ylong_runtime_trace/ylong_runtime"' check -p ylong_http_client --example async_ylong_https_proxy_bench --features "async http1_1 ylong_base c_openssl_3_0"` passed with existing warnings and `RUSTFLAGS="-A dangerous_implicit_autorefs"`.

诊断价值：dedicated driver 可以降低 ylong worker start-delay，但没有把 strict CONNECT 大响应吞吐推到 +20%，正式 repeat-5 仍为 `0/5` pass。短 repeat2 的 +13.8% 主要来自 libcurl baseline 变慢，no-phase repeat5 回落到 +3.3%，且 ylong p99 仍弱于 libcurl。结论：单独把 epoll polling 移到 dedicated thread，或把 dedicated driver 产生的无 worker context I/O wake 改成 global-front/global-first，都不是完成路径；剩余缺口仍在 I/O-ready task 恢复后的 worker scheduling/body transfer 交互，不能只靠更早发现 readiness 解决。

补充 CONNECT 后关闭外层 proxy TLS read-ahead 负实验：为区分“CONNECT 握手阶段需要 read-ahead”和“隧道 body 阶段不应继续 read-ahead”，本轮临时在 OpenSSL stream 上暴露 crate-private `set_read_ahead(false)`，并只在 `HTTPS target over HTTPS proxy` 完成 CONNECT 后关闭外层 proxy TLS read-ahead；内层 origin TLS 仍保持普通 `connect_tls(...)`。该改动已撤回。

同一 bounded strict CONNECT 点固定 `response_size=1 MiB`、`requests=128`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`、`--phase-summary`：

| Client experiment | repeats | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | max/min 提升 | 日志 |
| --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | --- |
| disable outer proxy TLS read-ahead after CONNECT | 2 | 2046.664 | 1998.780 | +2.617% | 0/2 | false | 53.117ms | 48.275ms | +9.570% / -4.336% | `target/https_proxy_bench/strict_connect_outer_read_ahead_off_after_connect_repeat2.log` |

验证：

- `cargo check -p ylong_http_client --example async_ylong_https_proxy_bench --features "async http1_1 ylong_base c_openssl_3_0"` passed with existing warnings.
- `rustfmt --check ylong_http_client/src/util/c_openssl/ssl/stream.rs ylong_http_client/src/async_impl/ssl_stream/c_ssl_stream.rs ylong_http_client/src/async_impl/connector/mod.rs ylong_http_client/src/sync_impl/connector.rs` passed with existing rustfmt-config warnings.

诊断价值：只在 CONNECT 后关闭外层 proxy TLS read-ahead 没有复现完全关闭 read-ahead 的 syscall 退化，但也没有带来足够收益；repeat2 平均只有 +2.6%，一个 paired sample 仍为负，ylong p99 仍弱于 libcurl。因此当前源码继续保留 CONNECT 外层 proxy TLS 64 KiB read-ahead，并保持内层 origin TLS 不启用 read-ahead；后续不应再沿“CONNECT 后切换 OpenSSL read-ahead”作为主线。

补充 ylong-only 48 KiB body read chunk 复测：此前 8/16/32 KiB chunk cap 都有小幅信号但未通过，本轮只补一个 32 KiB 和默认 64 KiB 之间的中点，验证是否存在更平衡的 body drain 粒度。该复测只使用 benchmark 的 `--ylong-read-chunk-size 49152`，没有修改库源码。

同一 bounded strict CONNECT 点固定 `response_size=1 MiB`、`requests=128`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`、`--phase-summary`：

| ylong read chunk cap | repeats | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | max/min 提升 | 日志 |
| ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | --- |
| 48 KiB | 2 | 2584.399 | 2526.699 | +4.368% | 0/2 | false | 43.427ms | 38.383ms | +17.115% / -8.380% | `target/https_proxy_bench/strict_ylong_read_chunk48k_repeat2.log` |

诊断价值：48 KiB 没有优于已记录的 32 KiB 方向，且仍有负向 paired sample；因此 body read chunk 粒度目前只算影响项，不是完成 strict 1 MiB CONNECT +20% 的独立修复点。源码继续保持默认由调用方 buffer 决定读取粒度，不引入内部固定 cap。

补充 TCP receive buffer 负实验：为验证 strict CONNECT 大响应是否受客户端 TCP receive buffer 限制，本轮临时在 async connector 建立 TCP 连接并设置 `TCP_NODELAY` 后调用 Linux `setsockopt(SO_RCVBUF=4 MiB)`；该改动已撤回。

同一 bounded strict CONNECT 点固定 `response_size=1 MiB`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`：

| Client experiment | requests | repeats | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | max/min 提升 | 日志 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | --- |
| `SO_RCVBUF=4 MiB`, phase summary | 128 | 2 | 3577.399 | 3273.949 | +9.294% | 0/2 | false | 28.400ms | 26.128ms | +12.469% / +6.119% | `target/https_proxy_bench/strict_tcp_rcvbuf_4m_repeat2.log` |
| `SO_RCVBUF=4 MiB`, no phase | 512 | 3 | 3513.635 | 3614.662 | -2.750% | 0/3 | false | 46.007ms | 31.142ms | +1.198% / -5.166% | `target/https_proxy_bench/strict_tcp_rcvbuf_4m_requests512_repeat3.log` |

验证：

- `cargo check -p ylong_http_client --example async_ylong_https_proxy_bench --features "async http1_1 ylong_base c_openssl_3_0"` passed with existing warnings.
- `rustfmt --check ylong_http_client/src/async_impl/connector/mod.rs` passed with existing rustfmt-config warnings.

诊断价值：larger client receive buffer can make a short 128-request phase run look better, but the longer no-phase filter turns negative and ylong p99 remains well above libcurl. Therefore socket receive-buffer tuning is not a strict CONNECT completion path, and keeping a hard-coded `SO_RCVBUF` would add platform-specific behavior without meeting the +20% requirement.

补充 HTTP/1 request-phase `TimeGroup` 负实验：为验证 benchmark 默认 no-phase 路径是否仍为每个 HTTP/1 request 支付 request-phase 计时代价，本轮临时去掉 async HTTP/1 请求路径中的 `set_transfer_start(Instant::now())`、`set_request_write_end(Instant::now())` 和首个响应字节处的 `set_transfer_end(Instant::now())`；连接阶段 `TimeGroup` 保持不变。该改动会让 phase summary 中 request-write / response-wait / transfer 样本为空，仅用于判断热路径 timestamp 开销是否足以解释 strict CONNECT 缺口；源码已恢复。

同一 bounded strict CONNECT 点固定 `response_size=1 MiB`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`：

| Client experiment | requests | repeats | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | max/min 提升 | 日志 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | --- |
| remove HTTP/1 request-phase TimeGroup timestamps, phase summary | 128 | 2 | 3408.109 | 3203.405 | +6.527% | 0/2 | false | 30.079ms | 26.183ms | +8.487% / +4.567% | `target/https_proxy_bench/no_http1_request_timing_128.log` |
| remove HTTP/1 request-phase TimeGroup timestamps, no phase | 512 | 3 | 3596.344 | 3587.467 | +0.253% | 0/3 | false | 47.815ms | 33.561ms | +5.707% / -4.002% | `target/https_proxy_bench/no_http1_request_timing_512.log` |

验证：

- `cargo check -p ylong_http_client --example async_ylong_https_proxy_bench --features "async http1_1 ylong_base c_openssl_3_0"` passed with existing warnings while the temporary timing removal was active.

诊断价值：HTTP/1 request-phase timestamp overhead is not the missing +20% lever. A short phase run still stayed below target and the stricter 512-request no-phase run was effectively flat (+0.253%) with worse ylong p99 than libcurl. Because `Response::time_group()` is public timing metadata and the probe does not produce a stable throughput win, there is no basis to add an API-level timing opt-out or remove these timestamps for normal clients.

补充 HTTP/1 pool permit fast path：为减少热连接复用路径上不必要的 async semaphore future 构造/轮询，本轮在 HTTP/1 connection pool 获取容量许可时先调用 `try_acquire()`，只有容量已满时才进入原来的 `acquire().await`。该改动保留“先预留容量、再扫描 dispatcher list”的顺序，避免并发任务同时复用同一个 idle-capacity slot 后突破 `max_conn_num`。

同一 native HTTPS-over-HTTPS-proxy fixture 下的短复测：

| Client experiment | requests | warmup | concurrency | runtime threads | repeats | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: |
| HTTP/1 permit `try_acquire` fast path | 64 | 16 | 16 | 8 | 3 | 3184.992 | 2871.110 | +11.159% | 0/3 | false | 7.984ms | 9.201ms |
| HTTP/1 permit `try_acquire` fast path | 128 | 64 | 64 | 16 | 3 | 2686.313 | 2679.302 | +0.224% | 0/3 | false | 40.659ms | 34.131ms |

同一 strict 点追加 `--trace-summary` 单轮显示 ylong `2921.002 rps`、libcurl `2876.211 rps`、`+1.557%`，但 `request_pending_gap_p99_us=34905`、`request_pending_gap_max_us=35888`，且 migrated pending-gap samples 仍占主导。这说明该 fast path 能减少无竞争 pool 许可路径开销，在低并发短测中有正向信号；但 strict 64 并发 1 MiB CONNECT 的剩余缺口仍来自 request future 从 Pending 到下一次 poll 的 runtime scheduling tail，而不是 HTTP/1 pool semaphore future 本身。

验证：

- `cargo check -p ylong_http_client --example async_ylong_https_proxy_bench --features "async http1_1 ylong_base c_openssl_3_0"` passed with existing warnings.
- `cargo check -p ylong_http_client --example async_https_proxy_bench --features "async http1_1 tokio_base c_openssl_3_0"` passed with existing warnings.
- `cargo test -p ylong_http_client ut_try_acquire_respects_capacity_and_releases_on_drop --features "async http1_1 ylong_base"` passed with existing warnings.
- `cargo test -p ylong_http_client ut_try_acquire_respects_capacity_and_releases_on_drop --features "async http1_1 tokio_base"` passed with existing warnings.
- The same targeted test with `c_openssl_3_0` enabled was not usable as a validation gate because existing crate-test binaries fail to link raw OpenSSL C symbols; the TLS benchmark examples above still build and link successfully.

结论：保留该改动作为低风险 HTTP/1 hot-pool 微优化和容量许可语义测试，但它不是 strict CONNECT +20% completion path。后续主线仍应集中在 I/O-ready request task 恢复尾部、worker migration/fairness，或更系统的大响应 transfer 调度机制。

补充 URI formatter no-op fast path：`RequestFormatter` 每个 request 都会调用 `UriFormatter::format()`；GET benchmark 已用 prebuilt requests 避开 request 构造，但 measured `client.request()` 内仍会重建已包含 scheme、host、显式 port 和 path 的 URI。本轮为这种已规范化 URI 增加 no-op fast path，避免热路径上重复 clone host、format authority 和重建 URI；缺省端口、缺 path、缺 scheme/host 或非法 port 仍走原来的规范化逻辑，行为边界不变。

同一 native HTTPS-over-HTTPS-proxy fixture，`requests=128`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`：

| Client experiment | repeats | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg |
| --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: |
| normalized URI no-op fast path, phase summary | 3 | 3042.044 | 2860.906 | +6.231% | 0/3 | false | 36.184ms | 34.765ms |
| normalized URI no-op fast path, trace summary | 1 | 2871.595 | 2728.164 | +5.257% | 0/1 | false | 38.061ms | 37.407ms |

trace-summary 仍显示 `request_pending_gap_p99_us=36610`、`request_pending_gap_max_us=36916`，request pending migrated samples 为 `118`、same-thread samples 为 `9`；body pending gap p99 为 `15475us`。因此该改动只消除已规范化 request 的无效 URI 分配/格式化，不改变 strict CONNECT 的主尾延迟模型。

验证：

- `cargo test -p ylong_http_client ut_uri_format_keeps_already_normalized_uri --features "async http1_1 ylong_base"` passed with existing warnings.
- `cargo check -p ylong_http_client --example async_ylong_https_proxy_bench --features "async http1_1 ylong_base c_openssl_3_0"` passed with existing warnings.
- `cargo check -p ylong_http_client --example async_https_proxy_bench --features "async http1_1 tokio_base c_openssl_3_0"` passed with existing warnings.

结论：保留该改动作为通用 request hot-path cleanup；它有小幅 strict 信号，但仍不是 20%+ completion path。后续不要继续沿 request construction/URI formatting 假设追主目标，除非有 profiler 证明 CPU allocation 已成为新瓶颈。

补充 HTTP/1 pool round-robin scan 负实验：为验证 shared pool 复用阶段是否因 `exist_h1_conn()` 每次从 newest dispatcher 倒扫并全量 `retain` shutdown entries 而形成锁内热点，本轮临时把 HTTP/1 dispatcher list 改成带 cursor 的 round-robin scan，并将 shutdown pruning 移到周期/未命中路径；同时加单测确认 cursor 轮转。该改动已撤回。

同一 native HTTPS-over-HTTPS-proxy fixture，`requests=128`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`、`--phase-summary`：

| Client experiment | repeats | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg |
| --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: |
| HTTP/1 pool round-robin scan + deferred shutdown prune | 3 | 2779.368 | 2714.919 | +2.120% | 0/3 | false | 38.285ms | 32.336ms |

验证：

- `cargo test -p ylong_http_client ut_dispatch_rotates_reuse_start --features "async http1_1 ylong_base"` passed while the temporary patch was active.
- `cargo check -p ylong_http_client --example async_ylong_https_proxy_bench --features "async http1_1 ylong_base c_openssl_3_0"` passed while the temporary patch was active.
- `cargo check -p ylong_http_client --example async_https_proxy_bench --features "async http1_1 tokio_base c_openssl_3_0"` passed while the temporary patch was active.

诊断价值：round-robin scan did not improve strict CONNECT; p99 regressed and one paired sample was negative. The fixed newest-first reuse pattern is likely preserving useful connection/cache locality, and the lock/list cleanup cost is not the dominant tail source at this workload. Do not reintroduce H1 pool cursoring as a primary completion path without fresh profiler evidence.

补充 HTTP/1 request encoder allocation trim：`RequestEncoder::new` 原先总是同时生成 origin-form 和 absolute-form URI bytes，即使普通 direct 请求与 HTTPS-over-HTTPS-proxy CONNECT 后的 origin 请求只会编码 origin-form；header value 编码也经 `HeaderValue::to_string().unwrap().into_bytes()` 中转。本轮改为只在 `absolute_uri(true)` 真正编码时懒生成 absolute-form，并直接从 `Uri` path/query 与 `HeaderValue::to_vec()` 组装 wire bytes，避免热请求行/头部编码中的无用 `String` 中转；同时保留一条注释说明 absolute-form 懒生成的 hot-path 边界。该改动还避免对 RFC 允许的 `0x80..=0xff` header field value 先构造潜在非 UTF-8 `String`。

同一 native HTTPS-over-HTTPS-proxy fixture，`requests=128`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`、`--phase-summary`：

| Client experiment | repeats | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | max/min 提升 | 日志 |
| --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | --- |
| HTTP/1 request encoder allocation trim | 3 | 3078.878 | 2966.573 | +4.111% | 0/3 | false | 35.287ms | 35.086ms | +12.390% / -0.700% | `target/https_proxy_bench/strict_request_encoder_fast_path_repeat3.log` |

验证：

- `rustfmt --check ylong_http/src/h1/request/encoder.rs` passed with existing rustfmt-config warnings.
- `cargo test -p ylong_http --features http1_1 ut_request_encoder -- --nocapture` passed: 5 encoder tests, including lazy absolute URI and raw header-value bytes.
- `cargo check -p ylong_http_client --example async_ylong_https_proxy_bench --features "async http1_1 ylong_base c_openssl_3_0"` passed with existing warnings.
- `cargo check -p ylong_http_client --example async_https_proxy_bench --features "async http1_1 tokio_base c_openssl_3_0"` passed with existing warnings.
- `cargo check -p ylong_http_client --example sync_https_proxy_bench --features "sync http1_1 tokio_base c_openssl_3_0"` passed with existing warnings.

结论：保留该改动作为通用 HTTP/1 request encoding hot-path cleanup 和 header-value raw-byte correctness cleanup；它在 strict CONNECT 下只有小幅吞吐信号且 p99 没有明显优于 libcurl，仍不是 20%+ completion path。后续不要把 request encoder allocation 作为主线继续追 strict CONNECT，除非 profiler 显示请求编码重新成为主要 CPU/allocator 热点。

补充 HTTP/1 response header raw-byte fast path：`ResponseDecoder` 原先在 header insert 时把已解析的 name/value bytes 通过 `String::from_utf8_unchecked` 转成 `String`/`&str`，再交给 `Headers::append` 重新验证和复制；同时 value 右侧 OWS 修剪会把 `take_value()` 得到的 Vec 再切片复制一次。本轮给已验证 HTTP/1 parser 路径增加 crate-private unsafe raw append：header name 在原 Vec 上 ASCII lowercase 后直接构造 `HeaderName`，header value 直接构造 `HeaderValue`，并把右侧 OWS 修剪改成原地 `truncate`。unsafe 边界由 `get_header_name` / `get_header_value` 的逐字节验证保证，并在调用点注释；新增 obs-text 测试覆盖 `0x80` raw byte 不经过 UTF-8 `String` 往返。

同一 native HTTPS-over-HTTPS-proxy fixture，`requests=128`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`、`--phase-summary`：

| Client experiment | repeats | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | max/min 提升 | 日志 |
| --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | --- |
| HTTP/1 response header raw-byte fast path | 3 | 3073.505 | 2945.349 | +4.339% | 0/3 | false | 36.175ms | 31.476ms | +6.265% / +1.762% | `target/https_proxy_bench/strict_response_header_raw_repeat3.log` |

验证：

- `rustfmt --check ylong_http/src/headers.rs ylong_http/src/h1/response/decoder.rs` passed with existing rustfmt-config warnings.
- `cargo test -p ylong_http --features http1_1 ut_response_decoder -- --nocapture` passed: 4 response decoder tests, including raw obs-text header value.
- `cargo test -p ylong_http --features http1_1 ut_headers -- --nocapture` passed: 13 header tests.
- `cargo check -p ylong_http_client --example async_ylong_https_proxy_bench --features "async http1_1 ylong_base c_openssl_3_0"` passed with existing warnings.
- `cargo check -p ylong_http_client --example async_https_proxy_bench --features "async http1_1 tokio_base c_openssl_3_0"` passed with existing warnings.
- `cargo check -p ylong_http_client --example sync_https_proxy_bench --features "sync http1_1 tokio_base c_openssl_3_0"` passed with existing warnings.

结论：保留该改动作为通用 HTTP/1 response decoding hot-path cleanup 和 obs-text correctness cleanup；strict CONNECT repeat3 仍只有 +4.339%、0/3 passes、formal_pass=false，p99 仍高于 libcurl，因此它不是 20%+ completion path。后续主线仍应回到 request future ready 后的 task resume / I/O wake tail，而不是继续深挖 header parser allocation。

补充 HTTP/1 response metadata raw parse：客户端在收到响应后仍会为 `Transfer-Encoding`、`Content-Length` 和 `Connection` 调用 `HeaderValue::to_string()`，即使这些字段只需要 ASCII 子串匹配或十进制整数解析。本轮把 async/sync HTTP/1 响应元数据解析改为直接遍历 `HeaderValue` 的 raw byte parts：`chunked` / `close` / `keep-alive` 仍保持原有大小写敏感匹配语义，`Content-Length` 只接受单个非空十进制值并用 checked arithmetic 拒绝溢出；重复值、obs-text 和非数字会继续走错误路径。该改动避免热响应路径上的 UTF-8/String 中转，同时用测试覆盖非法 obs-text、重复 `Content-Length` 和溢出。

同一 native HTTPS-over-HTTPS-proxy fixture，`requests=128`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`、`--phase-summary`：

| Client experiment | repeats | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | max/min 提升 | 日志 |
| --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | --- |
| HTTP/1 response metadata raw parse | 3 | 2518.975 | 2429.275 | +4.038% | 0/3 | false | 39.822ms | 38.901ms | +10.764% / -3.577% | `target/https_proxy_bench/strict_header_value_raw_parse_repeat3.log` |

验证：

- `rustfmt --check ylong_http_client/src/util/normalizer.rs ylong_http_client/src/async_impl/conn/http1.rs ylong_http_client/src/sync_impl/conn/http1.rs` passed with existing rustfmt-config warnings.
- `git diff --check` passed.
- `cargo test -p ylong_http_client ut_header_value_raw_helpers --features "async http1_1 ylong_base" -- --nocapture` passed.
- `cargo test -p ylong_http_client ut_body_length_parser --features "async http1_1 ylong_base" -- --nocapture` passed.
- `cargo check -p ylong_http_client --example async_ylong_https_proxy_bench --features "async http1_1 ylong_base c_openssl_3_0"` passed with existing warnings.
- `cargo check -p ylong_http_client --example async_https_proxy_bench --features "async http1_1 tokio_base c_openssl_3_0"` passed with existing warnings.
- `cargo check -p ylong_http_client --example sync_https_proxy_bench --features "sync http1_1 tokio_base c_openssl_3_0"` passed with existing warnings.

结论：保留该改动作为通用 response metadata hot-path cleanup；它减少每个响应的短生命周期分配和 UTF-8/String 转换，但 strict CONNECT repeat3 仍只有 +4.038%、0/3 passes、formal_pass=false，不能作为 20%+ completion path。后续不要继续沿 header metadata allocation 假设追主目标，除非 profiler 证明该路径重新成为主要 CPU/allocator 热点。

补充 async HTTP/1 request body encoding decision cleanup：`encode_various_body()` 原先在每个 request 上先把 `Content-Length` 转成 `String` 并解析成整数，只为了区分两个最终都走 `TextBody::from_async_reader()` 的非 chunked 分支；GET / empty body 虽然会跳过 body encoder，但仍会支付这段前置 header 检查成本。本轮把决策收敛为私有 `RequestBodyEncoding::{None, Text, Chunk}`：只用 raw header bytes 判断 `Transfer-Encoding: chunked`，非 chunked 空 body 直接返回，非 chunked 非空 body 一律走 `TextBody`。注释说明 `Content-Length` 不影响 HTTP/1 encoder 选择；新增测试覆盖 empty+content-length、empty+chunked 和 non-empty 三个分支。

同一 native HTTPS-over-HTTPS-proxy fixture，`requests=128`、`warmup=64`、`concurrency=64`、`runtime_threads=16`、`read_buffer_size=64 KiB`、`--phase-summary`：

| Client experiment | repeats | ylong avg rps | libcurl avg rps | 平均提升 | passes | formal_pass | ylong p99 avg | libcurl p99 avg | max/min 提升 | 日志 |
| --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | --- |
| async HTTP/1 request body encoding decision cleanup | 3 | 3023.922 | 2895.973 | +4.602% | 0/3 | false | 35.808ms | 32.257ms | +12.693% / -6.985% | `target/https_proxy_bench/strict_request_body_encoding_repeat3.log` |

验证：

- `rustfmt --check ylong_http_client/src/async_impl/conn/http1.rs` passed with existing rustfmt-config warnings.
- `git diff --check` passed.
- `cargo test -p ylong_http_client ut_request_body_encoding --features "async http1_1 ylong_base" -- --nocapture` passed: 3 tests.
- `cargo check -p ylong_http_client --example async_ylong_https_proxy_bench --features "async http1_1 ylong_base c_openssl_3_0"` passed with existing warnings.
- `cargo check -p ylong_http_client --example async_https_proxy_bench --features "async http1_1 tokio_base c_openssl_3_0"` passed with existing warnings.

结论：保留该改动作为 async HTTP/1 request hot-path cleanup；它删除无行为价值的 `Content-Length` String parse，并让 empty-body 跳过路径更直接。但 strict CONNECT repeat3 仍只有 +4.602%、0/3 passes、formal_pass=false，不能作为 20%+ completion path。后续不要继续沿 empty request body encoder 判断追主目标，除非 profiler 显示 request-send CPU 开销重新成为主要瓶颈。
