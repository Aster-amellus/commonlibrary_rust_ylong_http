#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "$0")/../.." && pwd)
OUT_DIR="$ROOT_DIR/target/https_proxy_bench"
YLONG_BIN="$ROOT_DIR/target/release/examples/async_https_proxy_bench"
CURL_BIN="$OUT_DIR/libcurl_harness"

mkdir -p "$OUT_DIR"

if [[ "$#" -eq 0 || "$*" == *"--help"* ]]; then
    cat <<'USAGE'
usage: tools/https_proxy_bench/run_https_proxy_bench.sh --url URL --proxy https://PROXY[:PORT] [options]

Common options forwarded to both clients:
  --requests N
  --concurrency N
  --proxy-ca-file PEM
  --proxy-client-cert PEM
  --proxy-client-key PEM
  --origin-ca-file PEM
  --insecure-proxy
  --insecure-origin
  --proxy-user-pass user:pass

Profiling:
  PROFILE=time      run each client under /usr/bin/time -v when available
  PROFILE=perf-stat run each client under perf stat when available
USAGE
    exit 0
fi

cargo build -p ylong_http_client --example async_https_proxy_bench \
    --features "async http1_1 tokio_base c_openssl_3_0" --release

if command -v curl-config >/dev/null 2>&1 && command -v cc >/dev/null 2>&1; then
    cc -O2 -pthread -o "$CURL_BIN" \
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

echo "== ylong_http_client =="
"${prefix[@]}" "$YLONG_BIN" "$@"

if [[ -n "$CURL_BIN" ]]; then
    echo "== libcurl =="
    "${prefix[@]}" "$CURL_BIN" "$@"
fi
