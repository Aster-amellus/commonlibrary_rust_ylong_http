#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "$0")/../.." && pwd)
CERT_DIR="$ROOT_DIR/target/https_proxy_bench/certs"
OUT_DIR="$ROOT_DIR/target/https_proxy_bench/results"

if [[ "$#" -ne 1 || "$1" == "--help" || "$1" == "-h" ]]; then
    cat <<'USAGE'
usage: tools/https_proxy_bench/run_https_proxy_scenario.sh tools/https_proxy_bench/scenarios/<name>.env

The script prints the fixture command and benchmark command by default.
Set RUN_BENCH=1 to run the benchmark. Start the printed fixture command in
another terminal first.
USAGE
    exit 0
fi

SCENARIO_FILE="$1"
if [[ ! -f "$SCENARIO_FILE" ]]; then
    echo "scenario file not found: $SCENARIO_FILE" >&2
    exit 2
fi

set -a
# shellcheck source=/dev/null
source "$SCENARIO_FILE"
set +a

: "${SCENARIO_NAME:?SCENARIO_NAME is required}"
: "${SCENARIO_LAYER:?SCENARIO_LAYER is required}"
: "${URL:?URL is required}"
: "${PROXY:?PROXY is required}"
: "${REQUESTS:?REQUESTS is required}"
: "${WARMUP:?WARMUP is required}"
: "${CONCURRENCY:?CONCURRENCY is required}"
: "${READ_BUFFER_SIZE:?READ_BUFFER_SIZE is required}"
: "${RESPONSE_SIZE:?RESPONSE_SIZE is required}"

mkdir -p "$CERT_DIR" "$OUT_DIR"

if [[ ! -f "$CERT_DIR/ca.pem" ]]; then
    "$ROOT_DIR/tools/https_proxy_bench/generate_certs.sh" "$CERT_DIR" >/dev/null
fi

fixture_args=(
    --cert-file target/https_proxy_bench/certs/server.pem
    --key-file target/https_proxy_bench/certs/server.key
    --response-size "$RESPONSE_SIZE"
)
if [[ "${ORIGIN_TLS:-0}" == "1" ]]; then
    fixture_args+=(--origin-tls)
fi
if [[ -n "${ORIGIN_DELAY_MS:-}" ]]; then
    fixture_args+=(--origin-delay-ms "$ORIGIN_DELAY_MS")
fi
if [[ -n "${RESPONSE_SIZE_SEQUENCE:-}" ]]; then
    fixture_args+=(--response-size-sequence "$RESPONSE_SIZE_SEQUENCE")
fi
if [[ -n "${ORIGIN_CLOSE_EVERY_N_REQUESTS:-}" ]]; then
    fixture_args+=(--origin-close-every-n-requests "$ORIGIN_CLOSE_EVERY_N_REQUESTS")
fi
if [[ "${PROXY_REQUIRE_CLIENT_CERT:-0}" == "1" ]]; then
    fixture_args+=(--ca-file target/https_proxy_bench/certs/ca.pem --require-client-cert)
fi

bench_args=(
    --url "$URL"
    --proxy "$PROXY"
    --proxy-ca-file "$CERT_DIR/ca.pem"
    --requests "$REQUESTS"
    --warmup-requests "$WARMUP"
    --concurrency "$CONCURRENCY"
    --read-buffer-size "$READ_BUFFER_SIZE"
)
if [[ "${ORIGIN_TLS:-0}" == "1" ]]; then
    bench_args+=(--origin-ca-file "$CERT_DIR/ca.pem")
fi
if [[ "${PROXY_REQUIRE_CLIENT_CERT:-0}" == "1" ]]; then
    bench_args+=(
        --proxy-client-cert "${PROXY_CLIENT_CERT:-$CERT_DIR/client.pem}"
        --proxy-client-key "${PROXY_CLIENT_KEY:-$CERT_DIR/client.key}"
    )
fi
if [[ -n "${DURATION_SECONDS:-}" ]]; then
    bench_args+=(--duration-seconds "$DURATION_SECONDS")
fi

print_command() {
    printf '%q' "$1"
    shift
    for arg in "$@"; do
        printf ' %q' "$arg"
    done
    printf '\n'
}

bench_env=(
    FRAME_POINTERS="${FRAME_POINTERS:-1}"
    CLIENT_FILTER="${CLIENT_FILTER:-both}"
    YLONG_CLIENT_MODE="${YLONG_CLIENT_MODE:-shared}"
    YLONG_PHASE_METRICS="${YLONG_PHASE_METRICS:-0}"
    YLONG_MAX_H1_CONN_NUMBER="${YLONG_MAX_H1_CONN_NUMBER:-}"
    PROFILE="${PROFILE:-}"
    PERF_EVENTS="${PERF_EVENTS:-task-clock,cycles,instructions,cache-misses,context-switches,cpu-migrations,page-faults}"
)

printf 'scenario=%s\n' "$SCENARIO_NAME"
printf 'layer=%s\n\n' "$SCENARIO_LAYER"

printf 'Start fixture in another terminal:\n\n'
print_command tools/https_proxy_bench/run_https_proxy_fixture_rs.sh "${fixture_args[@]}"

printf '\nLow-level benchmark command for this scenario:\n\n'
print_command env "${bench_env[@]}" tools/https_proxy_bench/run_https_proxy_bench.sh "${bench_args[@]}"

printf '\nRun governed scenario with:\n\n'
print_command RUN_BENCH=1 tools/https_proxy_bench/run_https_proxy_scenario.sh "$SCENARIO_FILE"

if [[ "${RUN_BENCH:-0}" != "1" ]]; then
    exit 0
fi

OUT_FILE="$OUT_DIR/${SCENARIO_NAME}_$(date +%Y%m%d_%H%M%S).jsonl"
PERF_FILE="$OUT_FILE.perf"
repeat="${REPEAT:-1}"

{
    printf '{"event":"scenario_start","scenario":"%s","layer":"%s","repeat":%s}\n' \
        "$SCENARIO_NAME" "$SCENARIO_LAYER" "$repeat"
    for i in $(seq 1 "$repeat"); do
        printf '{"event":"scenario_iteration","scenario":"%s","iteration":%s}\n' \
            "$SCENARIO_NAME" "$i"
        env "${bench_env[@]}" "$ROOT_DIR/tools/https_proxy_bench/run_https_proxy_bench.sh" \
            "${bench_args[@]}"
    done
} > >(tee "$OUT_FILE") 2> >(tee "$PERF_FILE" >&2)

printf 'scenario_result=%s\n' "$OUT_FILE" >&2
