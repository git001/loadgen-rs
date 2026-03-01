#!/usr/bin/env bash
# distributed-remote.sh — End-to-end distributed benchmark via Ansible.
#
# Deploys worker-agents to remote machines, runs the controller locally,
# then optionally stops the workers afterward.
#
# Usage:
#   scripts/distributed-remote.sh [options]
#
# Options:
#   --inventory, -i   PATH    Ansible inventory file (default: ansible/inventory.ini)
#   --target-url      URL     Benchmark target (default: env TARGET_URL)
#   --protocol        PROTO   h1|h2|h3 (default: h2)
#   --clients, -c     N       Total clients across all workers (default: 100)
#   --duration, -D    SEC     Duration in seconds (default: 10)
#   --threads, -t     N       Threads per worker (default: 4)
#   --max-streams, -m N       Max streams per connection (default: 1)
#   --insecure, -k            Skip TLS verification (default: true)
#   --lib             PATH    Local path to libloadgen_ffi.so
#   --deploy-only             Deploy workers but don't run benchmark
#   --stop-after              Stop workers after benchmark (default: false)
#   --skip-deploy             Skip deployment, assume workers are already running
#   --help, -h                Show this help
#
# Environment:
#   TARGET_URL        Benchmark target URL
#   ANSIBLE_OPTS      Extra options passed to ansible-playbook
#
# Example:
#   scripts/distributed-remote.sh \
#     -i ansible/inventory.ini \
#     --target-url "https://bench.local:8082/?s=256k" \
#     -c 200 -D 30 --stop-after

set -euo pipefail

# --- Defaults ---
INVENTORY="ansible/inventory.ini"
TARGET_URL="${TARGET_URL:-}"
PROTOCOL="h2"
CLIENTS=100
DURATION_S=10
THREADS=4
MAX_STREAMS=1
INSECURE=true
LIB_PATH="./target/release/libloadgen_ffi.so"
DEPLOY_ONLY=false
STOP_AFTER=false
SKIP_DEPLOY=false
DENO="${DENO:-deno}"
ANSIBLE_OPTS="${ANSIBLE_OPTS:-}"

# --- Argument parsing ---
usage() {
  sed -n '/^# Usage:/,/^# Example:/p' "$0" | sed 's/^# \?//' | head -n -1
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --inventory|-i)   INVENTORY="$2"; shift 2 ;;
    --target-url)     TARGET_URL="$2"; shift 2 ;;
    --protocol)       PROTOCOL="$2"; shift 2 ;;
    --clients|-c)     CLIENTS="$2"; shift 2 ;;
    --duration|-D)    DURATION_S="$2"; shift 2 ;;
    --threads|-t)     THREADS="$2"; shift 2 ;;
    --max-streams|-m) MAX_STREAMS="$2"; shift 2 ;;
    --insecure|-k)    INSECURE=true; shift ;;
    --lib)            LIB_PATH="$2"; shift 2 ;;
    --deploy-only)    DEPLOY_ONLY=true; shift ;;
    --stop-after)     STOP_AFTER=true; shift ;;
    --skip-deploy)    SKIP_DEPLOY=true; shift ;;
    --help|-h)        usage ;;
    *)
      echo "Unknown option: $1" >&2
      echo "Run with --help for usage." >&2
      exit 1
      ;;
  esac
done

if [[ -z "$TARGET_URL" && "$DEPLOY_ONLY" == "false" ]]; then
  echo "ERROR: --target-url or TARGET_URL is required." >&2
  exit 1
fi

if [[ ! -f "$INVENTORY" ]]; then
  echo "ERROR: Inventory file not found: $INVENTORY" >&2
  echo "  Copy ansible/inventory.example.ini to $INVENTORY and configure your workers." >&2
  exit 1
fi

# --- Helper: extract worker URLs from inventory ---
get_worker_urls() {
  # Parse ansible inventory to build http://host:port URLs
  ansible-inventory -i "$INVENTORY" --list 2>/dev/null | \
    python3 -c "
import sys, json
inv = json.load(sys.stdin)
hosts = inv.get('workers', {}).get('hosts', [])
hostvars = inv.get('_meta', {}).get('hostvars', {})
for h in hosts:
    v = hostvars.get(h, {})
    host = v.get('ansible_host', h)
    port = v.get('worker_port', 9091)
    print(f'http://{host}:{port}')
"
}

# --- Step 1: Deploy workers ---
if [[ "$SKIP_DEPLOY" == "false" ]]; then
  echo "=== Deploying worker-agents ==="
  echo ""

  # Build .so if not present
  if [[ ! -f "$LIB_PATH" ]]; then
    echo "Building libloadgen_ffi.so..."
    cargo build --release -p loadgen-ffi
  fi

  ansible-playbook \
    -i "$INVENTORY" \
    ansible/deploy-workers.yml \
    -e "lib_path=$LIB_PATH" \
    $ANSIBLE_OPTS

  echo ""
  echo "=== Workers deployed and running ==="
  echo ""
fi

if [[ "$DEPLOY_ONLY" == "true" ]]; then
  echo "Deploy-only mode. Workers are running. Use the following to run a benchmark:"
  URLS=$(get_worker_urls | tr '\n' ' ')
  echo ""
  echo "  TARGET_URL=\"$TARGET_URL\" CLIENTS=$CLIENTS DURATION_S=$DURATION_S \\"
  echo "    $DENO run --allow-ffi --allow-net --allow-read --allow-env \\"
  echo "    examples/distributed.ts $URLS"
  echo ""
  echo "To stop workers:"
  echo "  ansible-playbook -i $INVENTORY ansible/stop-workers.yml"
  exit 0
fi

# --- Step 2: Run distributed benchmark ---
echo "=== Running distributed benchmark ==="
echo "  target:     $TARGET_URL"
echo "  protocol:   $PROTOCOL"
echo "  clients:    $CLIENTS (split across workers)"
echo "  duration:   ${DURATION_S}s"
echo "  threads:    $THREADS per worker"
echo "  streams:    $MAX_STREAMS per connection"
echo ""

WORKER_URLS=$(get_worker_urls)
WORKER_URLS_INLINE=$(echo "$WORKER_URLS" | tr '\n' ' ')
WORKER_COUNT=$(echo "$WORKER_URLS" | wc -l)

echo "  workers ($WORKER_COUNT): $WORKER_URLS_INLINE"
echo ""

TIMEOUT_MS=$(( (DURATION_S + 60) * 1000 ))

TARGET_URL="$TARGET_URL" \
PROTOCOL="$PROTOCOL" \
INSECURE="$INSECURE" \
CLIENTS="$CLIENTS" \
DURATION_S="$DURATION_S" \
THREADS="$THREADS" \
MAX_STREAMS="$MAX_STREAMS" \
  "$DENO" run --allow-ffi --allow-net --allow-read --allow-env \
  examples/distributed.ts $WORKER_URLS_INLINE

BENCH_EXIT=$?

# --- Step 3: Stop workers (optional) ---
if [[ "$STOP_AFTER" == "true" ]]; then
  echo ""
  echo "=== Stopping workers ==="
  ansible-playbook \
    -i "$INVENTORY" \
    ansible/stop-workers.yml \
    $ANSIBLE_OPTS
fi

exit $BENCH_EXIT
