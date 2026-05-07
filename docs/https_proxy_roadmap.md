# HTTPS 代理项目 Roadmap 与 OKR

本文档记录 HTTPS 代理能力的 OKR、阶段状态、验收标准、测试命令和后续风险。

## OKR 总览

| Objective | Key Results | 当前状态 |
| --- | --- | --- |
| O1：补齐 HTTPS proxy 基础能力 | OpenSSL 支持 proxy TLS/mTLS；独立 proxy TLS 配置；async/sync HTTP target over HTTPS proxy；async/sync HTTPS target over HTTPS proxy；SDV 测试覆盖成功和失败路径 | 已完成 |
| O2：代理功能模块化 | CONNECT tunnel 从 connector 内抽出；async/sync proxy transport 独立模块；代理元数据统一沉淀到 `util::proxy`；文档记录新增协议扩展入口 | 已完成 v1 |
| O3：规范、测试、可用性文档 | 对齐 RFC 9110、RFC 9112、RFC 8446、OpenSSL、libcurl；提供 API 文档、使用指南、架构图、测试命令 | 已完成 |
| O4：性能对比和 profiling 体系 | 提供 ylong/libcurl HTTPS proxy benchmark harness；支持高并发、请求数、HTTP/HTTPS target、CA/mTLS 参数；记录 20%+ 目标的测量方法 | 进行中 |

## 阶段架构图

### P0 开始规划时

```mermaid
flowchart LR
    Client[Client] --> Connector[Connector]
    Connector --> Tcp[TCP]
    Connector --> HttpProxy[HTTP proxy]
    HttpProxy --> Connect[CONNECT for HTTPS target]
    Connector -. missing .-> HttpsProxy[HTTPS proxy TLS]
    Connector -. mixed .-> TunnelHelpers[inline tunnel helpers]
```

### P1 子任务 1 完成后

```mermaid
flowchart LR
    Client[Client] --> ProxyMatch[Proxy match]
    ProxyMatch --> HttpTarget[HTTP target]
    ProxyMatch --> HttpsTarget[HTTPS target]
    HttpTarget --> ProxyTls[Proxy TLS when proxy URL is https]
    ProxyTls --> AbsReq[absolute-form request]
    HttpsTarget --> ProxyTls2[Proxy TLS when proxy URL is https]
    ProxyTls2 --> Connect[CONNECT]
    Connect --> OriginTls[Origin TLS]
```

### P2 模块化完成后

```mermaid
flowchart TB
    UtilProxy[util::proxy<br/>ProxyInfo/Auth/TLS config] --> AsyncConnector[async HttpConnector]
    UtilProxy --> SyncConnector[sync HttpConnector]
    AsyncConnector --> AsyncProxy[async_impl::proxy<br/>connect_tls/tunnel]
    SyncConnector --> SyncProxy[sync_impl::proxy<br/>connect_tls/tunnel]
    AsyncProxy --> AsyncMix[Async MixStream variants]
    SyncProxy --> SyncMix[Sync MixStream variants]
```

### P3 benchmark/profiling 目标形态

```mermaid
flowchart LR
    Harness[bench script] --> Fixture[local origin + HTTPS proxy]
    Harness --> Ylong[ylong async bench client]
    Harness --> Curl[libcurl C harness]
    Ylong --> Metrics[req/s p50 p95 p99 errors]
    Curl --> Metrics
    Metrics --> Report[20%+ comparison report]
```

## M1 TLS 配置补齐

状态：已完成。

实现：

- 增加 OpenSSL FFI：`SSL_CTX_use_PrivateKey_file`、`SSL_CTX_check_private_key`、`SSL_CTX_set_ciphersuites`。
- 增加 `TlsConfigBuilder::private_key_file`。
- 增加 `TlsConfigBuilder::cipher_suite`。
- 增加 async/sync `ClientBuilder::tls_cipher_suite`。
- 构建 `TlsConfig` 时检查客户端证书和私钥匹配。
- 增加 TLS config 单元测试。

验收：

```bash
cargo check -p ylong_http_client --features "async http1_1 tokio_base c_openssl_3_0"
```

## M2 HTTPS Proxy 主路径

状态：已完成 async/sync。

实现：

- `ProxyBuilder::proxy_tls_config` 支持为代理服务器设置独立 TLS 配置。
- `http://` proxy 保持兼容。
- `https://` proxy 支持 HTTP target absolute-form 请求。
- `https://` proxy 支持 HTTPS target 的 proxy TLS + CONNECT + origin TLS。
- HTTP target 经过代理时写入 `Proxy-Authorization`。
- sync 和 async 均支持 `ProxyHttps` 与 `HttpsOverProxy` stream。

验收：

```bash
cargo check -p ylong_http_client --features "async http1_1 tokio_base c_openssl_3_0"
cargo check -p ylong_http_client --features "async http1_1 ylong_base c_openssl_3_0"
cargo test -p ylong_http_client --features "async http1_1 ylong_base c_openssl_3_0" --test sdv_async_https_proxy -- --nocapture
cargo test -p ylong_http_client --features "async http1_1 ylong_base" --test sdv_async_http_proxy -- --nocapture
cargo check -p ylong_http_client --features "sync http1_1 tokio_base c_openssl_3_0"
cargo test -p ylong_http_client --features "sync http1_1 tokio_base c_openssl_3_0" --test sdv_sync_https_proxy -- --nocapture
```

## M3 代理模块化

状态：已完成 v1。

实现：

- `async_impl::proxy` 承载 async proxy TLS 和 CONNECT tunnel。
- `sync_impl::proxy` 承载 sync proxy TLS 和 CONNECT tunnel。
- `util::proxy::ProxyInfo` 统一保存 proxy scheme、authority、basic auth 和 proxy TLS config。
- connector 只负责根据目标 scheme 和代理 scheme 编排 transport 顺序。

新增代理协议的推荐路径：

1. 在 `util::proxy` 扩展代理协议元数据和 builder API。
2. 在 `async_impl::proxy` / `sync_impl::proxy` 增加协议握手函数。
3. 在 connector 中新增协议分支，只做编排，不内联协议细节。
4. 为每种目标 scheme 增加 SDV 测试和失败路径测试。

## M4 文档与示例

状态：已完成基础文档和 async 示例。

交付物：

- `docs/architecture.md`：项目架构、请求生命周期、切入点。
- `docs/https_proxy_design.md`：设计思路、API、数据流、错误边界、规范和测试。
- `docs/https_proxy_roadmap.md`：OKR、阶段状态、Mermaid 架构图、验收命令。
- `docs/user_guide.md`：HTTPS proxy 使用说明。
- `ylong_http_client/examples/async_https_proxy.rs`：代理 TLS/mTLS 示例。

## M5 性能对比

状态：benchmark harness 待落地。

目标：HTTPS proxy 场景下 ylong_http_client HTTP 请求性能比 libcurl 高 20%+。

benchmark 场景：

- 本地 HTTPS proxy + 本地 HTTP origin。
- 本地 HTTPS proxy + 本地 HTTPS origin。
- keep-alive 开启，分别测连接复用和短连接。
- 并发级别：1、16、64、256。
- 请求数：1k、10k、100k。
- 响应体：空 body、小 body、固定 16 KiB body。

指标：

- 吞吐量 requests/sec。
- P50/P95/P99 latency。
- 错误数。
- CPU 使用率。
- TCP/TLS/CONNECT 建连次数。
- 连接池复用率。

实现要求：

- 不引入第三方 Rust crate。
- ylong_http_client 和 libcurl 使用相同 origin/proxy/并发/请求数。
- libcurl 优先使用 C harness 和 libcurl API；若系统缺少 `curl-config` 或 header，脚本应跳过并说明原因。
- profiling 建议使用 `perf stat`、`perf record` 或 `/usr/bin/time -v` 包裹同一 workload。

## 风险与后续

- HTTP/2 over HTTPS proxy 需要单独设计 ALPN 和代理请求编码，不能混入当前 HTTP/1.1 tunnel 实现。
- SOCKS/HTTP2 proxy 需要扩展 URI scheme 或新增 proxy protocol enum，目前 v1 仅保留模块边界。
- proxy TLS 和 origin TLS 的证书配置必须保持隔离。
- 长时间高压 benchmark 不应进入默认 CI；CI 只跑短耗时 smoke。
