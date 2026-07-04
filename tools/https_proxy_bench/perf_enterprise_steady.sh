#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "$0")/../.." && pwd)
CERT_DIR="$ROOT_DIR/target/https_proxy_bench/certs"
OUT_DIR="$ROOT_DIR/target/https_proxy_bench/results"

URL=${URL:-https://127.0.0.1:18080/}
PROXY=${PROXY:-https://localhost:18443}
REQUESTS=${REQUESTS:-50000}
WARMUP=${WARMUP:-4096}
CONCURRENCY=${CONCURRENCY:-128}
READ_BUFFER_SIZE=${READ_BUFFER_SIZE:-65536}

mkdir -p "$CERT_DIR" "$OUT_DIR"

if [[ ! -f "$CERT_DIR/ca.pem" ]]; then
    "$ROOT_DIR/tools/https_proxy_bench/generate_certs.sh" "$CERT_DIR" >/dev/null
fi

cat <<EOF
Start the Rust HTTPS proxy fixture in another terminal:

tools/https_proxy_bench/run_https_proxy_fixture_rs.sh \\
  --cert-file target/https_proxy_bench/certs/server.pem \\
  --key-file target/https_proxy_bench/certs/server.key \\
  --origin-tls \\
  --response-size 1024

Then run this protocol with RUN_BENCH=1.
EOF

if [[ "${RUN_BENCH:-0}" != "1" ]]; then
    exit 0
fi

OUT_FILE="$OUT_DIR/enterprise_steady_$(date +%Y%m%d_%H%M%S).jsonl"
PERF_FILE="$OUT_FILE.perf"

FRAME_POINTERS=1 PROFILE=perf-stat "$ROOT_DIR/tools/https_proxy_bench/run_https_proxy_bench.sh" \
    --url "$URL" \
    --proxy "$PROXY" \
    --proxy-ca-file "$CERT_DIR/ca.pem" \
    --origin-ca-file "$CERT_DIR/ca.pem" \
    --requests "$REQUESTS" \
    --warmup-requests "$WARMUP" \
    --concurrency "$CONCURRENCY" \
    --read-buffer-size "$READ_BUFFER_SIZE" \
    > >(tee "$OUT_FILE") \
    2> >(tee "$PERF_FILE" >&2)
