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

Then run this bottleneck matrix with RUN_BENCH=1.
EOF

if [[ "${RUN_BENCH:-0}" != "1" ]]; then
    exit 0
fi

OUT_FILE="$OUT_DIR/bottleneck_matrix_$(date +%Y%m%d_%H%M%S).jsonl"
PERF_FILE="$OUT_FILE.perf"

common_args=(
    --url "$URL"
    --proxy "$PROXY"
    --proxy-ca-file "$CERT_DIR/ca.pem"
    --origin-ca-file "$CERT_DIR/ca.pem"
    --requests "$REQUESTS"
    --warmup-requests "$WARMUP"
    --concurrency "$CONCURRENCY"
    --read-buffer-size "$READ_BUFFER_SIZE"
)

run_row() {
    local row="$1"
    local client_filter="$2"
    local ylong_mode="$3"
    local curl_share="$4"
    local curl_cache="per-thread"
    if [[ "$curl_share" == "1" ]]; then
        curl_cache="shared"
    fi

    printf '{"event":"bottleneck_matrix_row","row":"%s","client_filter":"%s","ylong_client_mode":"%s","libcurl_connection_cache":"%s"}\n' \
        "$row" "$client_filter" "$ylong_mode" "$curl_cache"

    FRAME_POINTERS="${FRAME_POINTERS:-1}" \
    CLIENT_FILTER="$client_filter" \
    YLONG_CLIENT_MODE="$ylong_mode" \
    LIBCURL_SHARE_CONNECTIONS="$curl_share" \
    "$ROOT_DIR/tools/https_proxy_bench/run_https_proxy_bench.sh" "${common_args[@]}"
}

{
    run_row "ylong-shared-client" "ylong" "shared" "0"
    run_row "ylong-per-worker-client" "ylong" "per-worker" "0"
    run_row "libcurl-per-thread-cache" "libcurl" "shared" "0"
    run_row "libcurl-shared-cache" "libcurl" "shared" "1"
} > >(tee "$OUT_FILE") 2> >(tee "$PERF_FILE" >&2)

printf 'matrix_result=%s\n' "$OUT_FILE" >&2
