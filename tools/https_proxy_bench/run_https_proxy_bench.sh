#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "$0")/../.." && pwd)
OUT_DIR="$ROOT_DIR/target/https_proxy_bench"
YLONG_BIN="$ROOT_DIR/target/release/examples/async_https_proxy_bench"
CURL_BIN="$OUT_DIR/libcurl_harness"

mkdir -p "$OUT_DIR"

if [[ "$#" -eq 0 || "$*" == *"--help"* ]]; then
    cat <<'USAGE'
usage: tools/https_proxy_bench/run_https_proxy_bench.sh --url URL --proxy http[s]://PROXY[:PORT] [options]

Options forwarded to both clients:
  --requests N
  --warmup-requests N
  --concurrency N
  --read-buffer-size N
  --proxy-ca-file PEM
  --proxy-client-cert PEM
  --proxy-client-key PEM
  --origin-ca-file PEM
  --insecure-proxy
  --insecure-origin
  --proxy-user-pass user:pass

Environment:
  PROFILE=perf-stat    run each client under perf stat when available
  PROFILE=time         run each client under /usr/bin/time -v when available
USAGE
    exit 0
fi

cargo build -p ylong_http_client --example async_https_proxy_bench \
    --features "async http1_1 tokio_base c_openssl_3_0" --release

if command -v curl-config >/dev/null 2>&1 && command -v cc >/dev/null 2>&1; then
    cc -O2 -Wall -Wextra -pthread -o "$CURL_BIN" \
        "$ROOT_DIR/tools/https_proxy_bench/libcurl_harness.c" \
        $(curl-config --cflags --libs)
else
    echo "skip libcurl harness: curl-config or cc is not available" >&2
    CURL_BIN=""
fi

prefix=()
case "${PROFILE:-}" in
    "")
        ;;
    time)
        if command -v /usr/bin/time >/dev/null 2>&1; then
            prefix=(/usr/bin/time -v)
        else
            echo "PROFILE=time requested but /usr/bin/time is unavailable" >&2
        fi
        ;;
    perf-stat)
        if command -v perf >/dev/null 2>&1; then
            prefix=(perf stat)
        else
            echo "PROFILE=perf-stat requested but perf is unavailable" >&2
        fi
        ;;
    *)
        echo "unknown PROFILE=${PROFILE}" >&2
        exit 2
        ;;
esac

json_escape() {
    printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

version_of() {
    local command="$1"
    shift
    if command -v "$command" >/dev/null 2>&1; then
        "$command" "$@" 2>/dev/null | head -n 1
    else
        printf 'unavailable'
    fi
}

printf '{"event":"bench_environment","rustc":"%s","cargo":"%s","curl":"%s"}\n' \
    "$(json_escape "$(version_of rustc --version)")" \
    "$(json_escape "$(version_of cargo --version)")" \
    "$(json_escape "$(version_of curl --version)")"

"${prefix[@]}" "$YLONG_BIN" "$@"
if [[ -n "$CURL_BIN" ]]; then
    "${prefix[@]}" "$CURL_BIN" "$@"
fi
