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
- 本阶段没有保留 `SSL_pending` ready-drain 或内层 origin TLS read-ahead 试验：两者均未降低 16 KiB body read 粒度，也没有稳定提升吞吐。
