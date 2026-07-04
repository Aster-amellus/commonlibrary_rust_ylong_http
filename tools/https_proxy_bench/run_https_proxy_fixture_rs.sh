#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "$0")/../.." && pwd)
MANIFEST="$ROOT_DIR/tools/https_proxy_bench/fixture-rs/Cargo.toml"

cargo build --manifest-path "$MANIFEST" --release

exec "$ROOT_DIR/tools/https_proxy_bench/fixture-rs/target/release/https_proxy_bench_fixture" "$@"
