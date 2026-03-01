#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/h2_profile_perf.sh --url <URL> [options] [-- <extra loadgen args>]

Runs one HTTP/2 benchmark under `perf stat` to inspect CPU/runtime behavior.

Defaults:
  --image loadgen-rs
  --duration 30s
  --clients 512
  --threads 8
  --max-streams 1
  --method GET
  --metrics-sample 1
  --network host
  --insecure (enabled)
EOF
}

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

IMAGE="loadgen-rs"
URL=""
DURATION="30s"
CLIENTS=512
THREADS=8
MAX_STREAMS=1
METHOD="GET"
METRICS_SAMPLE=1
NETWORK="host"
INSECURE=1
EXTRA_ARGS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --url) URL="$2"; shift 2 ;;
    --image) IMAGE="$2"; shift 2 ;;
    --duration) DURATION="$2"; shift 2 ;;
    -c|--clients) CLIENTS="$2"; shift 2 ;;
    -t|--threads) THREADS="$2"; shift 2 ;;
    -m|--max-streams) MAX_STREAMS="$2"; shift 2 ;;
    --method) METHOD="$2"; shift 2 ;;
    --metrics-sample) METRICS_SAMPLE="$2"; shift 2 ;;
    --network) NETWORK="$2"; shift 2 ;;
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

require_cmd perf
require_cmd podman

cmd=(
  podman run --rm --network "$NETWORK"
  "$IMAGE"
  --no-human
  --alpn-list h2
  --duration "$DURATION"
  --method "$METHOD"
  -c "$CLIENTS"
  -t "$THREADS"
  -m "$MAX_STREAMS"
  --metrics-sample "$METRICS_SAMPLE"
)
if [[ "$INSECURE" -eq 1 ]]; then
  cmd+=(--insecure)
fi
if [[ ${#EXTRA_ARGS[@]} -gt 0 ]]; then
  cmd+=("${EXTRA_ARGS[@]}")
fi
cmd+=("$URL")

echo "Running perf stat on:"
printf '  %q ' "${cmd[@]}"
echo
echo

perf stat -d -- "${cmd[@]}"
