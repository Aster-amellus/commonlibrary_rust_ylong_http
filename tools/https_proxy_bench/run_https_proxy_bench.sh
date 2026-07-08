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

Common options:
  --requests N
  --warmup-requests N
  --duration-seconds N
  --concurrency N
  --read-buffer-size N
  --proxy-ca-file PEM
  --proxy-client-cert PEM
  --proxy-client-key PEM
  --origin-ca-file PEM
  --insecure-proxy
  --insecure-origin
  --proxy-user-pass user:pass
  --http-version h1|h2|negotiate
  --tls13-ciphers LIST
  --tls-groups LIST   libcurl-only comparator option; ylong rejects it

Environment:
  PROFILE=perf-stat    run each client under perf stat when available
  PROFILE=time         run each client under /usr/bin/time -v when available
  PERF_EVENTS=EVENTS   perf stat events, comma-separated
  BENCH_CPU_LIST=LIST pin measured client commands with taskset -c LIST
  CLIENT_FILTER=both|ylong|libcurl
  YLONG_CLIENT_MODE=shared|per-worker
  YLONG_MAX_H1_CONN_NUMBER=N ylong-only max HTTP/1 connections per pool key
  YLONG_MAX_H2_CONN_NUMBER=N ylong-only max HTTP/2 connections per pool key
  YLONG_ALLOWED_CACHE_FRAME_SIZE=N ylong-only HTTP/2 response frame channel capacity
  LIBCURL_SHARE_CONNECTIONS=0|1
  LIBCURL_MODE=easy-threads|multi
  LIBCURL_PIPEWAIT=0|1
  LIBCURL_MAX_HOST_CONNECTIONS=N
  LIBCURL_MAX_TOTAL_CONNECTIONS=N
  LIBCURL_MAX_CONCURRENT_STREAMS=N
  OPENSSL_LIB_DIR      OpenSSL library directory; auto-detected with pkg-config
  OPENSSL_INCLUDE_DIR  OpenSSL include directory; auto-detected with pkg-config
USAGE
    exit 0
fi

client_filter="${CLIENT_FILTER:-both}"
case "$client_filter" in
    both|ylong|libcurl)
        ;;
    *)
        echo "unknown CLIENT_FILTER=${client_filter}" >&2
        exit 2
        ;;
esac

if [[ "${FRAME_POINTERS:-0}" == "1" ]]; then
    export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C force-frame-pointers=yes"
fi

if command -v pkg-config >/dev/null 2>&1; then
    if [[ -z "${OPENSSL_LIB_DIR:-}" ]]; then
        openssl_lib_dir=$(pkg-config --variable=libdir openssl 2>/dev/null || true)
        if [[ -n "$openssl_lib_dir" ]]; then
            export OPENSSL_LIB_DIR="$openssl_lib_dir"
        fi
    fi
    if [[ -z "${OPENSSL_INCLUDE_DIR:-}" ]]; then
        openssl_include_dir=$(pkg-config --variable=includedir openssl 2>/dev/null || true)
        if [[ -n "$openssl_include_dir" ]]; then
            export OPENSSL_INCLUDE_DIR="$openssl_include_dir"
        fi
    fi
fi

ylong_features="async http1_1 tokio_base c_openssl_3_0"
args=("$@")
for ((i = 0; i < ${#args[@]}; i++)); do
    if [[ "${args[$i]}" == "--http-version" && $((i + 1)) -lt ${#args[@]} ]]; then
        case "${args[$((i + 1))]}" in
            h2|http2|http/2|negotiate|alpn)
                ylong_features="$ylong_features http2"
                ;;
            *)
                ;;
        esac
    fi
done
if [[ "$client_filter" != "libcurl" ]]; then
    cargo build -p ylong_http_client --example async_https_proxy_bench \
        --features "$ylong_features" --release
fi

if [[ "$client_filter" != "ylong" ]] && command -v curl-config >/dev/null 2>&1 && command -v cc >/dev/null 2>&1; then
    cc -O2 -Wall -Wextra -pthread -o "$CURL_BIN" \
        "$ROOT_DIR/tools/https_proxy_bench/libcurl_harness.c" \
        $(curl-config --cflags --libs)
else
    if [[ "$client_filter" != "ylong" ]]; then
        echo "skip libcurl harness: curl-config or cc is not available" >&2
    fi
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
            perf_events="${PERF_EVENTS:-task-clock,context-switches,cpu-migrations,page-faults}"
            if perf stat -e "$perf_events" true >/dev/null 2>&1; then
                prefix=(perf stat -e "$perf_events")
            else
                echo "PROFILE=perf-stat requested but perf events are unavailable; running without perf stat" >&2
            fi
        else
            echo "PROFILE=perf-stat requested but perf is unavailable" >&2
        fi
        ;;
    *)
        echo "unknown PROFILE=${PROFILE}" >&2
        exit 2
        ;;
esac

if [[ -n "${BENCH_CPU_LIST:-}" ]]; then
    if command -v taskset >/dev/null 2>&1; then
        prefix=(taskset -c "$BENCH_CPU_LIST" "${prefix[@]}")
    else
        echo "BENCH_CPU_LIST requested but taskset is unavailable" >&2
        exit 2
    fi
fi

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

ylong_args=("$@")
curl_args=("$@")
if [[ -n "${YLONG_CLIENT_MODE:-}" ]]; then
    ylong_args+=(--client-mode "$YLONG_CLIENT_MODE")
fi
if [[ -n "${YLONG_MAX_H1_CONN_NUMBER:-}" ]]; then
    ylong_args+=(--max-h1-conn-number "$YLONG_MAX_H1_CONN_NUMBER")
fi
if [[ -n "${YLONG_MAX_H2_CONN_NUMBER:-}" ]]; then
    ylong_args+=(--max-h2-conn-number "$YLONG_MAX_H2_CONN_NUMBER")
fi
if [[ -n "${YLONG_ALLOWED_CACHE_FRAME_SIZE:-}" ]]; then
    ylong_args+=(--allowed-cache-frame-size "$YLONG_ALLOWED_CACHE_FRAME_SIZE")
fi
if [[ "${LIBCURL_SHARE_CONNECTIONS:-0}" == "1" ]]; then
    curl_args+=(--share-connections)
fi
if [[ -n "${LIBCURL_MODE:-}" ]]; then
    curl_args+=(--libcurl-mode "$LIBCURL_MODE")
fi
if [[ "${LIBCURL_PIPEWAIT:-0}" == "1" ]]; then
    curl_args+=(--libcurl-pipewait)
fi
if [[ -n "${LIBCURL_MAX_HOST_CONNECTIONS:-}" ]]; then
    curl_args+=(--libcurl-max-host-connections "$LIBCURL_MAX_HOST_CONNECTIONS")
fi
if [[ -n "${LIBCURL_MAX_TOTAL_CONNECTIONS:-}" ]]; then
    curl_args+=(--libcurl-max-total-connections "$LIBCURL_MAX_TOTAL_CONNECTIONS")
fi
if [[ -n "${LIBCURL_MAX_CONCURRENT_STREAMS:-}" ]]; then
    curl_args+=(--libcurl-max-concurrent-streams "$LIBCURL_MAX_CONCURRENT_STREAMS")
fi

if [[ "$client_filter" != "libcurl" ]]; then
    "${prefix[@]}" "$YLONG_BIN" "${ylong_args[@]}"
fi
if [[ "$client_filter" != "ylong" && -n "$CURL_BIN" ]]; then
    "${prefix[@]}" "$CURL_BIN" "${curl_args[@]}"
fi
