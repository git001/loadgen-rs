#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/h3_m_sweep.sh --url <URL> [options] [-- <extra loadgen args>]

Runs HTTP/3 benchmark across max-streams values (-m sweep).

Defaults:
  --runner podman
  --image loadgen-rs
  --bin ./target/release/loadgen-rs
  --duration 30s
  --clients 512
  --threads 8
  --max-streams-list 1,2,4,8
  --method GET
  --metrics-sample 1
  --repeats 3
  --network host
  --insecure (enabled)
  --warm-up-time 0s
  --ramp-up-time 0s
  --cooldown-seconds 0
EOF
}

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

IMAGE="loadgen-rs"
RUNNER="podman"
BIN="./target/release/loadgen-rs"
URL=""
DURATION="30s"
CLIENTS=512
THREADS=8
MAX_STREAMS_LIST="1,2,4,8"
METHOD="GET"
METRICS_SAMPLE=1
REPEATS=3
NETWORK="host"
INSECURE=1
WARM_UP_TIME="0s"
RAMP_UP_TIME="0s"
COOLDOWN_SECONDS=0
EXTRA_ARGS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --url) URL="$2"; shift 2 ;;
    --runner) RUNNER="$2"; shift 2 ;;
    --image) IMAGE="$2"; shift 2 ;;
    --bin) BIN="$2"; shift 2 ;;
    --duration) DURATION="$2"; shift 2 ;;
    -c|--clients) CLIENTS="$2"; shift 2 ;;
    -t|--threads) THREADS="$2"; shift 2 ;;
    --max-streams-list) MAX_STREAMS_LIST="$2"; shift 2 ;;
    --method) METHOD="$2"; shift 2 ;;
    --metrics-sample) METRICS_SAMPLE="$2"; shift 2 ;;
    --repeats) REPEATS="$2"; shift 2 ;;
    --network) NETWORK="$2"; shift 2 ;;
    --warm-up-time) WARM_UP_TIME="$2"; shift 2 ;;
    --ramp-up-time) RAMP_UP_TIME="$2"; shift 2 ;;
    --cooldown-seconds) COOLDOWN_SECONDS="$2"; shift 2 ;;
    --insecure) INSECURE=1; shift ;;
    --no-insecure) INSECURE=0; shift ;;
    --help|-h) usage; exit 0 ;;
    --) shift; EXTRA_ARGS=("$@"); break ;;
    *) echo "Unknown argument: $1" >&2; usage; exit 1 ;;
  esac
done

if [[ -z "$URL" ]]; then
  echo "--url is required" >&2
  usage
  exit 1
fi

require_cmd jq

if [[ "$RUNNER" != "podman" && "$RUNNER" != "local" ]]; then
  echo "Invalid --runner: $RUNNER (expected: podman|local)" >&2
  exit 1
fi

if ! [[ "$COOLDOWN_SECONDS" =~ ^[0-9]+$ ]]; then
  echo "Invalid --cooldown-seconds: $COOLDOWN_SECONDS (expected non-negative integer)" >&2
  exit 1
fi

if [[ "$RUNNER" == "podman" ]]; then
  require_cmd podman
else
  if [[ ! -x "$BIN" ]]; then
    echo "Binary not found or not executable: $BIN" >&2
    exit 1
  fi
fi

tmp_json="$(mktemp)"
trap 'rm -f "$tmp_json"' EXIT

IFS=',' read -r -a M_VALUES <<<"$MAX_STREAMS_LIST"

TOTAL_RUNS=$(( ${#M_VALUES[@]} * REPEATS ))
RUN_INDEX=0

for m in "${M_VALUES[@]}"; do
  m="$(echo "$m" | xargs)"
  for ((i = 1; i <= REPEATS; i++)); do
    echo
    echo "=== h3 m=${m} run ${i}/${REPEATS} ==="
    out_file="$(mktemp)"
    if [[ "$RUNNER" == "podman" ]]; then
      cmd=(
        podman run --rm --network "$NETWORK"
        "$IMAGE"
        --no-human
        --alpn-list h3
        --duration "$DURATION"
        --method "$METHOD"
        -c "$CLIENTS"
        -t "$THREADS"
        -m "$m"
        --metrics-sample "$METRICS_SAMPLE"
        --warm-up-time "$WARM_UP_TIME"
        --ramp-up-time "$RAMP_UP_TIME"
      )
    else
      cmd=(
        "$BIN"
        --no-human
        --alpn-list h3
        --duration "$DURATION"
        --method "$METHOD"
        -c "$CLIENTS"
        -t "$THREADS"
        -m "$m"
        --metrics-sample "$METRICS_SAMPLE"
        --warm-up-time "$WARM_UP_TIME"
        --ramp-up-time "$RAMP_UP_TIME"
      )
    fi
    if [[ "$INSECURE" -eq 1 ]]; then
      cmd+=(--insecure)
    fi
    if [[ ${#EXTRA_ARGS[@]} -gt 0 ]]; then
      cmd+=("${EXTRA_ARGS[@]}")
    fi
    cmd+=("$URL")
    "${cmd[@]}" 2>&1 | tee "$out_file"
    json_line="$(grep -E '^\{.*\}$' "$out_file" | tail -n1 || true)"
    rm -f "$out_file"
    if [[ -z "$json_line" ]]; then
      echo "Could not find JSON result line (m=${m}, round=${i})" >&2
      exit 1
    fi
    jq -c --argjson max_streams "$m" --argjson round "$i" \
      '. + {
        max_streams_actual: $max_streams,
        round: $round
      }' <<<"$json_line" >>"$tmp_json"

    RUN_INDEX=$((RUN_INDEX + 1))
    if [[ "$COOLDOWN_SECONDS" -gt 0 && "$RUN_INDEX" -lt "$TOTAL_RUNS" ]]; then
      echo "Cooling down ${COOLDOWN_SECONDS}s before next run..."
      sleep "$COOLDOWN_SECONDS"
    fi
  done
done

echo
echo "=== Per-run summary ==="
jq -rs '
  ["m","round","rps","mbps_in","latency_p50_us","latency_p90_us","latency_p99_us","ttfb_p50_us","elapsed_s"],
  (.[] | [
    (.max_streams_actual|tostring),
    (.round|tostring),
    (.rps|tostring),
    (.mbps_in|tostring),
    (.latency_p50_us|tostring),
    (.latency_p90_us|tostring),
    (.latency_p99_us|tostring),
    (.ttfb_p50_us|tostring),
    (.elapsed_s|tostring)
  ]) | @tsv
' "$tmp_json" | column -t -s $'\t'

echo
echo "=== max-streams averages ==="
jq -rs '
  sort_by(.max_streams_actual)
  | group_by(.max_streams_actual)
  | map({
      m: .[0].max_streams_actual,
      runs: length,
      rps_avg: (map(.rps) | add / length),
      rps_min: (map(.rps) | min),
      rps_max: (map(.rps) | max),
      mbps_in_avg: (map(.mbps_in) | add / length),
      latency_p50_avg: (map(.latency_p50_us) | add / length),
      latency_p90_avg: (map(.latency_p90_us) | add / length),
      latency_p99_avg: (map(.latency_p99_us) | add / length),
      ttfb_p50_avg: (map(.ttfb_p50_us) | add / length),
      elapsed_avg: (map(.elapsed_s) | add / length)
    })
  | sort_by(.m)
  | (["m","runs","rps_avg","rps_min","rps_max","mbps_in_avg","lat_p50_avg_us","lat_p90_avg_us","lat_p99_avg_us","ttfb_p50_avg_us","elapsed_avg_s"]),
    (.[] | [
      (.m|tostring),
      (.runs|tostring),
      (.rps_avg|tostring),
      (.rps_min|tostring),
      (.rps_max|tostring),
      (.mbps_in_avg|tostring),
      (.latency_p50_avg|tostring),
      (.latency_p90_avg|tostring),
      (.latency_p99_avg|tostring),
      (.ttfb_p50_avg|tostring),
      (.elapsed_avg|tostring)
    ]) | @tsv
' "$tmp_json" | column -t -s $'\t'
