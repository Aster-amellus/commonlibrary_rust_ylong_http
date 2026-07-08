# 知识产权与第三方依赖说明

## 1. 原创性声明

本次 HTTPS proxy 实现、测试、benchmark harness 和文档由参赛团队基于
OpenHarmony `commonlibrary_rust_ylong_http` 代码库开发。新增代码遵循仓库现有
Apache-2.0 许可证声明和文件头格式。

未在交付代码中复制第三方项目源码。libcurl 仅作为本地 benchmark 对照库，通过系统
`curl-config` 链接，不随本仓库分发二进制。

## 2. 主许可证

仓库根目录包含 `LICENSE`，许可证为 Apache License 2.0。新增 Rust、C、Shell、
Markdown 和 SVG 文件按同一项目许可证交付。

`ylong_http_client/Cargo.toml` 中的 `license = "Apache-2.0"` 保持不变。

## 3. 直接依赖

| 依赖 | 位置 | 用途 | 许可证核查方式 |
| --- | --- | --- | --- |
| `ylong_http` | workspace path dependency | HTTP 基础协议组件 | 仓库同许可证 |
| `tokio` | `ylong_http_client` optional dependency | Tokio async runtime | `cargo metadata` |
| `ylong_runtime` | Git dependency | OpenHarmony ylong runtime | 上游仓库许可证 |
| `libc` | optional dependency | OpenSSL/BoringSSL FFI | `cargo metadata` |
| `quiche` | optional dependency | HTTP/3 | `cargo metadata` |
| `openssl` | dev dependency / fixture dependency | 测试和 Rust fixture TLS | `cargo metadata` |
| `tokio-openssl` | dev dependency / fixture dependency | fixture async TLS | `cargo metadata` |
| `h2` | fixture dependency | fixture HTTP/2 origin | `cargo metadata` |
| `http` | fixture dependency | fixture HTTP types | `cargo metadata` |
| `bytes` | fixture dependency | fixture buffer handling | `cargo metadata` |
| `serde_json` | fixture dependency | fixture startup/counter JSON | `cargo metadata` |
| libcurl | system library | 性能对照客户端 | `curl --version` / system package metadata |
| OpenSSL | system library | TLS implementation | `openssl version -a` / system package metadata |

## 4. 建议生成依赖清单

正式提交前建议在交付环境生成第三方依赖报告：

```bash
cargo metadata --format-version 1 > target/cargo-metadata.json
```

如果环境安装了许可证工具，可补充：

```bash
cargo install cargo-about
cargo about generate about.hbs > THIRD_PARTY_LICENSES.md
```

或使用组织指定的 SPDX/CycloneDX 工具生成 SBOM。生成文件应记录工具版本和生成日期。

## 5. 外部资产

本提交新增架构图为仓库内自绘 SVG/PNG，未使用外部图片、图标或字体文件。

benchmark 证书由 `tools/https_proxy_bench/generate_certs.sh` 在本地生成，仅用于测试，
不作为生产证书分发。

## 6. AI 辅助说明

文档结构参考公开开源项目文档规范，并经过人工审查；代码实现、测试范围和性能协议以
仓库当前源码、测试结果和 benchmark 输出为准。

## 7. 参考规范

- GitHub Docs: About READMEs
- The Cargo Book: Manifest Format
- Rust API Guidelines: Documentation
- ACM Artifact Review and Badging
- REUSE Specification 3.3
- C4 Model
