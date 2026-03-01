#!/usr/bin/env bash
# Start N loadgen worker-agents locally for testing distributed mode.
#
# Usage:
#   bash examples/worker-start.sh          # 2 workers on ports 9091, 9092
#   bash examples/worker-start.sh 3        # 3 workers on ports 9091-9093
#   LIB=./target/debug/libloadgen_ffi.so bash examples/worker-start.sh

set -euo pipefail

NUM_WORKERS="${1:-2}"
BASE_PORT="${BASE_PORT:-9091}"
LIB="${LIB:-./target/release/libloadgen_ffi.so}"
DENO="${DENO:-deno}"

if [[ ! -f "$LIB" ]]; then
  echo "ERROR: $LIB not found. Run: cargo build --release -p loadgen-ffi"
  exit 1
fi

PIDS=()

cleanup() {
  echo ""
  echo "stopping workers..."
  for pid in "${PIDS[@]}"; do
    kill "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null
  echo "all workers stopped."
}
trap cleanup EXIT INT TERM

for i in $(seq 0 $((NUM_WORKERS - 1))); do
  PORT=$((BASE_PORT + i))
  echo "starting worker $((i + 1)) on 0.0.0.0:${PORT}..."
  "$DENO" run --allow-ffi --allow-net --allow-read --allow-env \
    ts/worker-agent.ts --listen "0.0.0.0:${PORT}" --lib "$LIB" &
  PIDS+=($!)
done

echo ""
echo "${NUM_WORKERS} workers started (ports ${BASE_PORT}–$((BASE_PORT + NUM_WORKERS - 1)))"
echo "press Ctrl+C to stop all workers"
echo ""

wait
