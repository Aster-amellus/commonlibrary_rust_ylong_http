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
  --method GET|POST
  --body-size N
  --proxy-ca-file PEM
  --proxy-client-cert PEM
  --proxy-client-key PEM
  --origin-ca-file PEM
  --insecure-proxy
  --insecure-origin
  --proxy-user-pass user:pass

Profiling:
  REPEAT=N          run each client N times
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

REPEAT="${REPEAT:-1}"
if ! [[ "$REPEAT" =~ ^[0-9]+$ ]] || [[ "$REPEAT" -eq 0 ]]; then
    echo "REPEAT must be a positive integer" >&2
    exit 2
fi

printf '{"kind":"bench_environment","repeat":%s,' "$REPEAT"
printf '"git":"%s",' "$(json_escape "$(git -C "$ROOT_DIR" rev-parse --short HEAD 2>/dev/null || printf unavailable)")"
printf '"rustc":"%s",' "$(json_escape "$(version_of rustc --version)")"
printf '"cargo":"%s",' "$(json_escape "$(version_of cargo --version)")"
printf '"curl":"%s",' "$(json_escape "$(version_of curl --version)")"
printf '"openssl":"%s",' "$(json_escape "$(version_of openssl version)")"
printf '"uname":"%s"}\n' "$(json_escape "$(uname -a 2>/dev/null || printf unavailable)")"

run_repeated() {
    local name="$1"
    local bin="$2"
    shift 2
    local run
    for run in $(seq 1 "$REPEAT"); do
        echo "== ${name} run ${run}/${REPEAT} =="
        "${prefix[@]}" "$bin" "$@"
    done
}

run_repeated "ylong_http_client" "$YLONG_BIN" "$@"

if [[ -n "$CURL_BIN" ]]; then
    run_repeated "libcurl" "$CURL_BIN" "$@"
fi
