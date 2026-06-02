#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "$0")/../.." && pwd)
OUT_DIR="$ROOT_DIR/target/https_proxy_bench"
YLONG_ASYNC_BIN="$ROOT_DIR/target/release/examples/async_https_proxy_bench"
YLONG_ASYNC_YLONG_BIN="$ROOT_DIR/target/release/examples/async_ylong_https_proxy_bench"
YLONG_SYNC_BIN="$ROOT_DIR/target/release/examples/sync_https_proxy_bench"
CURL_BIN="$OUT_DIR/libcurl_harness"

mkdir -p "$OUT_DIR"

if [[ "$#" -eq 0 || "$*" == *"--help"* ]]; then
    cat <<'USAGE'
usage: tools/https_proxy_bench/run_https_proxy_bench.sh --url URL [--proxy http[s]://PROXY[:PORT]] [options]

Common options forwarded to both clients:
  --requests N
  --warmup-requests N
  --concurrency N
  --runtime-threads N
  --read-buffer-size N
  --method GET|POST
  --body-size N
  --proxy-ca-file PEM
  --proxy-client-cert PEM
  --proxy-client-key PEM
  --origin-ca-file PEM
  --insecure-proxy
  --insecure-origin
  --proxy-user-pass user:pass

ylong-only options:
  --client-per-worker
  --runtime-mode current-thread-per-worker|current-thread-sharded
  --runtime-affinity
  --ylong-read-chunk-size N
  --trace-summary
  --phase-summary
  --yield-after-body-read

Profiling:
  REPEAT=N          run each client N times
  YLONG_CLIENT=async|async-ylong|sync|both
  BENCH_ORDER=paired|grouped
  BENCH_TARGET_PCT=20
  BENCH_TARGET_MIN_PASSES=4
  BENCH_ENFORCE_TARGET=1
  BENCH_CLIENT_TIMEOUT=SECONDS
  YLONG_RUNTIME_PATH=/path/to/commonlibrary_rust_ylong_runtime[/ylong_runtime]
  PROFILE=time      run each client under /usr/bin/time -v when available
  PROFILE=perf-stat run each client under perf stat when available
USAGE
    exit 0
fi

cargo_config=()
ylong_runtime_path=""
build_env=()
if [[ -n "${YLONG_RUNTIME_PATH:-}" ]]; then
    if [[ -f "$YLONG_RUNTIME_PATH/ylong_runtime/Cargo.toml" ]]; then
        ylong_runtime_path=$(cd "$YLONG_RUNTIME_PATH/ylong_runtime" && pwd)
    elif [[ -f "$YLONG_RUNTIME_PATH/Cargo.toml" ]]; then
        ylong_runtime_path=$(cd "$YLONG_RUNTIME_PATH" && pwd)
    else
        echo "YLONG_RUNTIME_PATH must point to a ylong_runtime package or repository root" >&2
        exit 2
    fi
    if ! grep -q '^name = "ylong_runtime"$' "$ylong_runtime_path/Cargo.toml"; then
        echo "YLONG_RUNTIME_PATH resolved to '$ylong_runtime_path', but it is not the ylong_runtime package" >&2
        exit 2
    fi

    cargo_config=(
        --config
        "patch.\"https://gitcode.com/openharmony/commonlibrary_rust_ylong_runtime.git\".ylong_runtime.path=\"$ylong_runtime_path\""
    )
    build_env=(
        env
        "RUSTFLAGS=${RUSTFLAGS:-} -A dangerous_implicit_autorefs"
    )
fi

BENCH_CLIENT_TIMEOUT="${BENCH_CLIENT_TIMEOUT:-0}"
if ! [[ "$BENCH_CLIENT_TIMEOUT" =~ ^[0-9]+$ ]]; then
    echo "BENCH_CLIENT_TIMEOUT must be a non-negative integer number of seconds" >&2
    exit 2
fi
if [[ "$BENCH_CLIENT_TIMEOUT" -gt 0 ]] && ! command -v timeout >/dev/null 2>&1; then
    echo "BENCH_CLIENT_TIMEOUT requested but timeout is unavailable" >&2
    exit 2
fi

common_args=()
ylong_args=()
ylong_current_thread_runtime=0
args=("$@")
index=0
while [[ "$index" -lt "${#args[@]}" ]]; do
    arg="${args[$index]}"
    case "$arg" in
        --client-per-worker)
            ylong_args+=("$arg")
            ;;
        --runtime-mode)
            index=$((index + 1))
            if [[ "$index" -ge "${#args[@]}" ]]; then
                echo "--runtime-mode requires a value" >&2
                exit 2
            fi
            value="${args[$index]}"
            ylong_args+=("$arg" "$value")
            if [[ "$value" == "current-thread-per-worker" || "$value" == "current-thread-sharded" ]]; then
                ylong_current_thread_runtime=1
            fi
            ;;
        --runtime-affinity)
            ylong_args+=("$arg")
            ;;
        --ylong-read-chunk-size)
            index=$((index + 1))
            if [[ "$index" -ge "${#args[@]}" ]]; then
                echo "--ylong-read-chunk-size requires a value" >&2
                exit 2
            fi
            ylong_args+=("$arg" "${args[$index]}")
            ;;
        --trace-summary)
            ylong_args+=("$arg")
            ;;
        --phase-summary)
            ylong_args+=("$arg")
            ;;
        --yield-after-body-read)
            ylong_args+=("$arg")
            ;;
        *)
            common_args+=("$arg")
            ylong_args+=("$arg")
            ;;
    esac
    index=$((index + 1))
done

YLONG_CLIENT="${YLONG_CLIENT:-async}"
if [[ "$ylong_current_thread_runtime" -eq 1 && "$YLONG_CLIENT" != "async-ylong" ]]; then
    echo "--runtime-mode current-thread modes require YLONG_CLIENT=async-ylong" >&2
    exit 2
fi

YLONG_ASYNC_YLONG_FEATURES="async http1_1 ylong_base c_openssl_3_0"
if [[ "$ylong_current_thread_runtime" -eq 1 ]]; then
    YLONG_ASYNC_YLONG_FEATURES+=" __ylong_current_thread_runtime"
fi

case "$YLONG_CLIENT" in
    async)
        "${build_env[@]}" cargo build "${cargo_config[@]}" -p ylong_http_client --example async_https_proxy_bench \
            --features "async http1_1 tokio_base c_openssl_3_0" --release
        ;;
    async-ylong)
        "${build_env[@]}" cargo build "${cargo_config[@]}" -p ylong_http_client --example async_ylong_https_proxy_bench \
            --features "$YLONG_ASYNC_YLONG_FEATURES" --release
        ;;
    sync)
        "${build_env[@]}" cargo build "${cargo_config[@]}" -p ylong_http_client --example sync_https_proxy_bench \
            --features "sync http1_1 tokio_base c_openssl_3_0" --release
        ;;
    both)
        "${build_env[@]}" cargo build "${cargo_config[@]}" -p ylong_http_client --example async_https_proxy_bench \
            --features "async http1_1 tokio_base c_openssl_3_0" --release
        "${build_env[@]}" cargo build "${cargo_config[@]}" -p ylong_http_client --example async_ylong_https_proxy_bench \
            --features "$YLONG_ASYNC_YLONG_FEATURES" --release
        "${build_env[@]}" cargo build "${cargo_config[@]}" -p ylong_http_client --example sync_https_proxy_bench \
            --features "sync http1_1 tokio_base c_openssl_3_0" --release
        ;;
    *)
        echo "YLONG_CLIENT must be async, async-ylong, sync, or both" >&2
        exit 2
        ;;
esac

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

timeout_prefix=()
if [[ "$BENCH_CLIENT_TIMEOUT" -gt 0 ]]; then
    timeout_prefix=(timeout "${BENCH_CLIENT_TIMEOUT}s")
fi

json_escape() {
    printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

print_prefixed_env_json() {
    local prefix="$1"
    local first=1
    local key
    local value
    printf '{'
    while IFS='=' read -r key value; do
        case "$key" in
            "$prefix"*)
                if [[ "$first" -eq 0 ]]; then
                    printf ','
                fi
                first=0
                printf '"%s":"%s"' "$(json_escape "$key")" "$(json_escape "$value")"
                ;;
        esac
    done < <(env | LC_ALL=C sort)
    printf '}'
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

METRICS_LOG=$(mktemp "$OUT_DIR/bench_metrics.XXXXXX")
trap 'rm -f "$METRICS_LOG"' EXIT
RUN_STATUS=0

printf '{"kind":"bench_environment","repeat":%s,' "$REPEAT"
printf '"client_timeout_s":%s,' "$BENCH_CLIENT_TIMEOUT"
printf '"git":"%s",' "$(json_escape "$(git -C "$ROOT_DIR" rev-parse --short HEAD 2>/dev/null || printf unavailable)")"
printf '"rustc":"%s",' "$(json_escape "$(version_of rustc --version)")"
printf '"cargo":"%s",' "$(json_escape "$(version_of cargo --version)")"
printf '"curl":"%s",' "$(json_escape "$(version_of curl --version)")"
printf '"openssl":"%s",' "$(json_escape "$(version_of openssl version)")"
printf '"ylong_runtime_path":"%s",' "$(json_escape "$ylong_runtime_path")"
printf '"ylong_runtime_env":'
print_prefixed_env_json "YLONG_RUNTIME_"
printf ','
printf '"uname":"%s"}\n' "$(json_escape "$(uname -a 2>/dev/null || printf unavailable)")"

run_client_command() {
    local bin="$1"
    shift
    local out
    local status
    out=$(mktemp "$OUT_DIR/client_output.XXXXXX")
    if "${timeout_prefix[@]}" "${prefix[@]}" "$bin" "$@" 2>&1 | tee "$out"; then
        awk '/^\{/ && /"client"/ && /"rps"/ { print }' "$out" >>"$METRICS_LOG"
        rm -f "$out"
        return 0
    else
        status=$?
        awk '/^\{/ && /"client"/ && /"rps"/ { print }' "$out" >>"$METRICS_LOG" || true
        rm -f "$out"
        return "$status"
    fi
}

run_client_checked() {
    local status=0
    if [[ "$BENCH_CLIENT_TIMEOUT" -eq 0 ]]; then
        run_client_command "$@"
        return
    fi

    run_client_command "$@" || status=$?
    if [[ "$status" -eq 0 ]]; then
        return 0
    fi

    if [[ "$RUN_STATUS" -eq 0 ]]; then
        RUN_STATUS="$status"
    fi
    if [[ "$status" -eq 124 ]]; then
        echo "client command timed out after ${BENCH_CLIENT_TIMEOUT}s: $1" >&2
    else
        echo "client command failed with status ${status}: $1" >&2
    fi
    return 0
}

print_bench_summary() {
    if [[ ! -s "$METRICS_LOG" ]]; then
        return 0
    fi
    if ! command -v python3 >/dev/null 2>&1; then
        echo "skip bench summary: python3 is not available" >&2
        if [[ "${BENCH_ENFORCE_TARGET:-0}" == "1" ]]; then
            return 2
        fi
        return 0
    fi

    BENCH_TARGET_PCT="${BENCH_TARGET_PCT:-20}" \
    BENCH_TARGET_MIN_PASSES="${BENCH_TARGET_MIN_PASSES:-4}" \
    BENCH_ENFORCE_TARGET="${BENCH_ENFORCE_TARGET:-0}" \
    python3 - "$METRICS_LOG" <<'PY'
import json
import os
import sys

path = sys.argv[1]
target_pct = float(os.environ.get("BENCH_TARGET_PCT", "20"))
min_passes = int(os.environ.get("BENCH_TARGET_MIN_PASSES", "4"))
enforce = os.environ.get("BENCH_ENFORCE_TARGET") == "1"
target_epsilon = 1e-9

metrics = []
with open(path, "r", encoding="utf-8") as handle:
    for line in handle:
        line = line.strip()
        if not line:
            continue
        try:
            metric = json.loads(line)
        except json.JSONDecodeError:
            continue
        if "client" in metric and "rps" in metric and "errors" in metric:
            metrics.append(metric)

def numeric_values(runs, field):
    values = []
    for run in runs:
        if field not in run:
            continue
        try:
            values.append(float(run[field]))
        except (TypeError, ValueError):
            continue
    return values

def add_numeric_summary(summary, runs, field, prefix=""):
    values = numeric_values(runs, field)
    if not values:
        return
    summary[f"{prefix}avg_{field}"] = round(sum(values) / len(values), 3)
    summary[f"{prefix}min_{field}"] = round(min(values), 3)
    summary[f"{prefix}max_{field}"] = round(max(values), 3)

def int_field(run, field):
    try:
        return int(run[field])
    except (KeyError, TypeError, ValueError):
        return None

def add_workload_summary(summary, runs):
    if not runs:
        return
    first = runs[0]
    if "runtime_mode" in first:
        summary["runtime_mode"] = first["runtime_mode"]
    for field in (
        "requests",
        "warmup_requests",
        "concurrency",
        "runtime_threads",
        "read_buffer_size",
    ):
        value = int_field(first, field)
        if value is not None:
            summary[field] = value

    warmup_requests = int_field(first, "warmup_requests")
    concurrency = int_field(first, "concurrency")
    if warmup_requests is not None and concurrency is not None:
        summary["warmup_covers_workers"] = warmup_requests >= concurrency

groups = {}
for metric in metrics:
    groups.setdefault(metric["client"], []).append(metric)

baseline = groups.get("libcurl", [])
all_passed = True

for client, runs in groups.items():
    if client == "libcurl":
        continue

    avg_rps = sum(float(run["rps"]) for run in runs) / len(runs)
    client_errors = sum(int(run["errors"]) for run in runs)
    summary = {
        "kind": "bench_summary",
        "client": client,
        "baseline_client": "libcurl",
        "runs": len(runs),
        "baseline_runs": len(baseline),
        "target_improvement_pct": target_pct,
        "target_min_passes": min_passes,
        "avg_rps": round(avg_rps, 3),
        "errors": client_errors,
    }
    add_workload_summary(summary, runs)
    add_numeric_summary(summary, runs, "latency_us_p99")
    add_numeric_summary(summary, runs, "worker_start_delay_us_max")
    add_numeric_summary(summary, runs, "worker_elapsed_us_max")

    if not baseline:
        summary.update({
            "comparisons": 0,
            "passes": 0,
            "formal_pass": False,
            "reason": "missing_baseline",
        })
        all_passed = False
        print(json.dumps(summary, separators=(",", ":")))
        continue

    comparisons = min(len(runs), len(baseline))
    improvements = []
    passes = 0
    baseline_errors = 0
    for idx in range(comparisons):
        run = runs[idx]
        base = baseline[idx]
        base_rps = float(base["rps"])
        improvement = 0.0 if base_rps == 0.0 else (float(run["rps"]) / base_rps - 1.0) * 100.0
        improvements.append(improvement)
        run_errors = int(run["errors"])
        base_errors = int(base["errors"])
        baseline_errors += base_errors
        if improvement + target_epsilon >= target_pct and run_errors <= base_errors:
            passes += 1

    baseline_avg = sum(float(run["rps"]) for run in baseline) / len(baseline)
    formal_pass = comparisons >= min_passes and passes >= min_passes
    if not formal_pass:
        all_passed = False

    add_numeric_summary(summary, baseline, "latency_us_p99", "baseline_")
    add_numeric_summary(summary, baseline, "worker_start_delay_us_max", "baseline_")
    add_numeric_summary(summary, baseline, "worker_elapsed_us_max", "baseline_")
    summary.update({
        "comparisons": comparisons,
        "passes": passes,
        "formal_pass": formal_pass,
        "baseline_avg_rps": round(baseline_avg, 3),
        "baseline_errors": baseline_errors,
        "avg_improvement_pct": round(sum(improvements) / comparisons, 3),
        "min_improvement_pct": round(min(improvements), 3),
        "max_improvement_pct": round(max(improvements), 3),
    })
    print(json.dumps(summary, separators=(",", ":")))

if enforce and not all_passed:
    sys.exit(1)
PY
}

run_repeated() {
    local name="$1"
    local bin="$2"
    shift 2
    local run
    for run in $(seq 1 "$REPEAT"); do
        echo "== ${name} run ${run}/${REPEAT} =="
        run_client_checked "$bin" "$@"
    done
}

run_ylong_once() {
    local run="$1"
    case "$YLONG_CLIENT" in
        async)
            echo "== ylong_http_client_async run ${run}/${REPEAT} =="
            run_client_checked "$YLONG_ASYNC_BIN" "${ylong_args[@]}"
            ;;
        async-ylong)
            echo "== ylong_http_client_async_ylong run ${run}/${REPEAT} =="
            run_client_checked "$YLONG_ASYNC_YLONG_BIN" "${ylong_args[@]}"
            ;;
        sync)
            echo "== ylong_http_client_sync run ${run}/${REPEAT} =="
            run_client_checked "$YLONG_SYNC_BIN" "${common_args[@]}"
            ;;
        both)
            echo "== ylong_http_client_async run ${run}/${REPEAT} =="
            run_client_checked "$YLONG_ASYNC_BIN" "${ylong_args[@]}"
            echo "== ylong_http_client_async_ylong run ${run}/${REPEAT} =="
            run_client_checked "$YLONG_ASYNC_YLONG_BIN" "${ylong_args[@]}"
            echo "== ylong_http_client_sync run ${run}/${REPEAT} =="
            run_client_checked "$YLONG_SYNC_BIN" "${common_args[@]}"
            ;;
    esac
}

BENCH_ORDER="${BENCH_ORDER:-paired}"
case "$BENCH_ORDER" in
    paired)
        for run in $(seq 1 "$REPEAT"); do
            run_ylong_once "$run"
            if [[ -n "$CURL_BIN" ]]; then
                echo "== libcurl run ${run}/${REPEAT} =="
                run_client_checked "$CURL_BIN" "${common_args[@]}"
            fi
        done
        ;;
    grouped)
        case "$YLONG_CLIENT" in
            async)
                run_repeated "ylong_http_client_async" "$YLONG_ASYNC_BIN" "${ylong_args[@]}"
                ;;
            async-ylong)
                run_repeated "ylong_http_client_async_ylong" "$YLONG_ASYNC_YLONG_BIN" "${ylong_args[@]}"
                ;;
            sync)
                run_repeated "ylong_http_client_sync" "$YLONG_SYNC_BIN" "${common_args[@]}"
                ;;
            both)
                run_repeated "ylong_http_client_async" "$YLONG_ASYNC_BIN" "${ylong_args[@]}"
                run_repeated "ylong_http_client_async_ylong" "$YLONG_ASYNC_YLONG_BIN" "${ylong_args[@]}"
                run_repeated "ylong_http_client_sync" "$YLONG_SYNC_BIN" "${common_args[@]}"
                ;;
        esac

        if [[ -n "$CURL_BIN" ]]; then
            run_repeated "libcurl" "$CURL_BIN" "${common_args[@]}"
        fi
        ;;
    *)
        echo "BENCH_ORDER must be paired or grouped" >&2
        exit 2
        ;;
esac

SUMMARY_STATUS=0
print_bench_summary || SUMMARY_STATUS=$?
if [[ "$RUN_STATUS" -ne 0 ]]; then
    exit "$RUN_STATUS"
fi
if [[ "$SUMMARY_STATUS" -ne 0 ]]; then
    exit "$SUMMARY_STATUS"
fi
