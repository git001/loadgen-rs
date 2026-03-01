#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/ab_compare_report.sh [options]

Compares k6 and Deno+loadgen-rs outputs with aligned settings and prints
a compact side-by-side summary.

Options:
  --mode <rps|closed>        Benchmark mode (default: rps)
  --target-url <url>         Target URL (default: https://bench.local:8082/?s=256k)
  --duration <dur>           k6 duration, e.g. 10s / 2m (default: 10s)
  --insecure <true|false>    Skip TLS verification (default: true)

RPS mode options:
  --rps <n>                  Total target RPS for both tools (default: 5000)
  --concurrency <n>          k6 preAllocated/max VUs (default: 64)
  --clients <n>              Deno clients (default: --concurrency)
  --threads <n>              Deno threads (default: 8)
  --max-streams <n>          Deno max streams per client (default: 1)

Closed mode options:
  --vus <n>                  k6 and Deno client count (default: 4)
  --threads <n>              Deno threads (default: --vus)
  --max-streams <n>          Deno max streams per client (default: 1)

Advanced:
  --allow-net <list>         Deno allow-net list (default: host from target URL)
  --k6-script <path>         Override k6 script
  --deno-script <path>       Override Deno script
  --keep-tmp                 Keep raw outputs in temp dir
  -h, --help                 Show help
EOF
}

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

duration_to_seconds() {
  local raw="$1"
  if [[ "$raw" =~ ^([0-9]+([.][0-9]+)?)([smh]?)$ ]]; then
    local num="${BASH_REMATCH[1]}"
    local unit="${BASH_REMATCH[3]}"
    case "$unit" in
      ""|"s") awk -v n="$num" 'BEGIN { printf "%.6f", n }' ;;
      "m") awk -v n="$num" 'BEGIN { printf "%.6f", n * 60 }' ;;
      "h") awk -v n="$num" 'BEGIN { printf "%.6f", n * 3600 }' ;;
      *) return 1 ;;
    esac
    return 0
  fi
  return 1
}

host_from_url() {
  local url="$1"
  local authority
  authority="$(printf '%s' "$url" | sed -E 's#^[a-zA-Z][a-zA-Z0-9+.-]*://([^/?#]+).*#\1#')"
  if [[ "$authority" =~ ^\[([0-9a-fA-F:]+)\](:[0-9]+)?$ ]]; then
    printf '%s\n' "${BASH_REMATCH[1]}"
    return
  fi
  printf '%s\n' "${authority%%:*}"
}

to_ms() {
  local value="$1"
  awk -v v="$value" '
    BEGIN {
      if (v == "" || v == "-") {
        print "-";
        exit;
      }
      if (v ~ /ms$/) { sub(/ms$/, "", v); printf "%.3f", v; exit }
      if (v ~ /µs$/) { sub(/µs$/, "", v); printf "%.3f", v / 1000; exit }
      if (v ~ /us$/) { sub(/us$/, "", v); printf "%.3f", v / 1000; exit }
      if (v ~ /s$/)  { sub(/s$/,  "", v); printf "%.3f", v * 1000; exit }
      printf "%.3f", v
    }
  '
}

fmt_num() {
  local value="$1"
  awk -v v="$value" '
    BEGIN {
      if (v == "" || v == "-") {
        print "-";
      } else {
        printf "%.2f", v;
      }
    }
  '
}

extract_rate() {
  local line="$1"
  awk '
    {
      for (i = 1; i <= NF; i++) {
        if ($i ~ /\/s$/) {
          gsub(/\/s$/, "", $i);
          print $i;
          exit
        }
      }
    }
  ' <<<"$line"
}

extract_failed_pct() {
  local line="$1"
  awk '
    {
      for (i = 1; i <= NF; i++) {
        if ($i ~ /%$/) {
          gsub(/%$/, "", $i);
          print $i;
          exit
        }
      }
    }
  ' <<<"$line"
}

MODE="rps"
TARGET_URL="https://bench.local:8082/?s=256k"
DURATION="10s"
INSECURE="true"
RPS="5000"
CONCURRENCY="64"
CLIENTS=""
THREADS=""
MAX_STREAMS="1"
VUS="4"
ALLOW_NET=""
K6_SCRIPT=""
DENO_SCRIPT=""
KEEP_TMP=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode) MODE="$2"; shift 2 ;;
    --target-url) TARGET_URL="$2"; shift 2 ;;
    --duration) DURATION="$2"; shift 2 ;;
    --insecure) INSECURE="$2"; shift 2 ;;
    --rps) RPS="$2"; shift 2 ;;
    --concurrency) CONCURRENCY="$2"; shift 2 ;;
    --clients) CLIENTS="$2"; shift 2 ;;
    --threads) THREADS="$2"; shift 2 ;;
    --max-streams) MAX_STREAMS="$2"; shift 2 ;;
    --vus) VUS="$2"; shift 2 ;;
    --allow-net) ALLOW_NET="$2"; shift 2 ;;
    --k6-script) K6_SCRIPT="$2"; shift 2 ;;
    --deno-script) DENO_SCRIPT="$2"; shift 2 ;;
    --keep-tmp) KEEP_TMP=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown argument: $1" >&2; usage; exit 1 ;;
  esac
done

if [[ "$MODE" != "rps" && "$MODE" != "closed" ]]; then
  echo "--mode must be one of: rps, closed" >&2
  exit 1
fi

require_cmd k6
require_cmd deno
require_cmd rg
require_cmd awk
require_cmd sed

DURATION_S="$(duration_to_seconds "$DURATION" || true)"
if [[ -z "${DURATION_S:-}" ]]; then
  echo "Unsupported --duration format: $DURATION (expected like 10s, 2m, 0.5s)" >&2
  exit 1
fi

if [[ -z "$ALLOW_NET" ]]; then
  ALLOW_NET="$(host_from_url "$TARGET_URL")"
fi

if [[ "$MODE" == "rps" ]]; then
  K6_SCRIPT="${K6_SCRIPT:-examples/k6/ab-compare-rps.ts}"
  DENO_SCRIPT="${DENO_SCRIPT:-examples/ab-compare-rps.ts}"
  CLIENTS="${CLIENTS:-$CONCURRENCY}"
  THREADS="${THREADS:-8}"
else
  K6_SCRIPT="${K6_SCRIPT:-examples/k6/ab-compare.ts}"
  DENO_SCRIPT="${DENO_SCRIPT:-examples/ab-compare.ts}"
  CLIENTS="${CLIENTS:-$VUS}"
  THREADS="${THREADS:-$VUS}"
fi

tmp_dir="$(mktemp -d)"
cleanup() {
  if [[ "$KEEP_TMP" -eq 0 ]]; then
    rm -rf "$tmp_dir"
  else
    echo "Keeping raw output in: $tmp_dir"
  fi
}
trap cleanup EXIT

K6_OUT="$tmp_dir/k6.out"
DENO_OUT="$tmp_dir/deno.out"

echo "Running k6 ($MODE)..."
if [[ "$MODE" == "rps" ]]; then
  env \
    TARGET_URL="$TARGET_URL" \
    RPS="$RPS" \
    DURATION="$DURATION" \
    CONCURRENCY="$CONCURRENCY" \
    INSECURE="$INSECURE" \
    k6 run "$K6_SCRIPT" >"$K6_OUT" 2>&1
else
  env \
    TARGET_URL="$TARGET_URL" \
    VUS="$VUS" \
    DURATION="$DURATION" \
    INSECURE="$INSECURE" \
    k6 run "$K6_SCRIPT" >"$K6_OUT" 2>&1
fi

echo "Running Deno+loadgen-rs ($MODE)..."
if [[ "$MODE" == "rps" ]]; then
  env \
    TARGET_URL="$TARGET_URL" \
    DURATION_S="$DURATION_S" \
    RPS="$RPS" \
    CLIENTS="$CLIENTS" \
    THREADS="$THREADS" \
    MAX_STREAMS="$MAX_STREAMS" \
    INSECURE="$INSECURE" \
    deno run \
      --allow-ffi \
      --allow-net="$ALLOW_NET" \
      --allow-env=TARGET_URL,DURATION_S,RPS,RPS_PER_CLIENT,CLIENTS,THREADS,MAX_STREAMS,INSECURE \
      "$DENO_SCRIPT" >"$DENO_OUT" 2>&1
else
  env \
    TARGET_URL="$TARGET_URL" \
    DURATION_S="$DURATION_S" \
    VUS="$VUS" \
    THREADS="$THREADS" \
    MAX_STREAMS="$MAX_STREAMS" \
    INSECURE="$INSECURE" \
    deno run \
      --allow-ffi \
      --allow-net="$ALLOW_NET" \
      --allow-env=TARGET_URL,DURATION_S,VUS,THREADS,MAX_STREAMS,INSECURE \
      "$DENO_SCRIPT" >"$DENO_OUT" 2>&1
fi

k6_reqs_line="$(rg -m1 'bench_reqs\.*:|http_reqs\.*:' "$K6_OUT" || true)"
k6_dur_line="$(rg -m1 'http_req_duration\.*:' "$K6_OUT" || true)"
k6_fail_line="$(rg -m1 'http_req_failed\.*:' "$K6_OUT" || true)"
k6_rps_raw="$(extract_rate "$k6_reqs_line")"
k6_failed_pct="$(extract_failed_pct "$k6_fail_line")"
k6_avg_token="$(sed -nE 's/.*avg=([^ ]+).*/\1/p' <<<"$k6_dur_line")"
k6_p99_token="$(sed -nE 's/.*p\(99\)=([^ ]+).*/\1/p' <<<"$k6_dur_line")"
k6_avg_ms="$(to_ms "$k6_avg_token")"
k6_p99_ms="$(to_ms "$k6_p99_token")"
k6_proto="n/a"
if rg -q 'protocol is HTTP/2' "$K6_OUT"; then
  if rg -q '✓ protocol is HTTP/2' "$K6_OUT"; then
    k6_proto="pass"
  else
    k6_proto="fail"
  fi
fi

deno_reqs_line="$(rg -m1 'http_reqs\.*' "$DENO_OUT" || true)"
deno_dur_line="$(rg -m1 'http_req_duration\.*' "$DENO_OUT" || true)"
deno_fail_line="$(rg -m1 'http_req_failed\.*' "$DENO_OUT" || true)"
deno_proto_line="$(rg -m1 'protocol ==' "$DENO_OUT" || true)"
deno_rps_raw="$(extract_rate "$deno_reqs_line")"
deno_failed_pct="$(extract_failed_pct "$deno_fail_line")"
deno_avg_token="$(sed -nE 's/.*avg=([^ ]+).*/\1/p' <<<"$deno_dur_line")"
deno_p99_token="$(sed -nE 's/.*p\(99\)=([^ ]+).*/\1/p' <<<"$deno_dur_line")"
deno_avg_ms="$(to_ms "$deno_avg_token")"
deno_p99_ms="$(to_ms "$deno_p99_token")"
deno_proto="n/a"
if [[ -n "$deno_proto_line" ]]; then
  if sed -nE 's/.*\(([0-9.]+%)\).*/\1/p' <<<"$deno_proto_line" | rg -q '^100(\.0+)?%$'; then
    deno_proto="pass"
  else
    deno_proto="fail"
  fi
fi

printf '\n'
echo "=== AB Compare Report (${MODE}) ==="
echo "target: $TARGET_URL"
echo "duration: $DURATION"
if [[ "$MODE" == "rps" ]]; then
  echo "k6: rate=$RPS/s, concurrency=$CONCURRENCY"
  echo "deno: rate=$RPS/s, clients=$CLIENTS, threads=$THREADS, max_streams=$MAX_STREAMS"
else
  echo "k6: vus=$VUS"
  echo "deno: clients=$CLIENTS, threads=$THREADS, max_streams=$MAX_STREAMS"
fi
printf '\n'
printf '%-16s %12s %14s %14s %12s %10s\n' "tool" "req/s" "avg(ms)" "p99(ms)" "failed(%)" "h2-check"
printf '%-16s %12s %14s %14s %12s %10s\n' "k6" "$(fmt_num "$k6_rps_raw")" "$k6_avg_ms" "$k6_p99_ms" "$(fmt_num "$k6_failed_pct")" "$k6_proto"
printf '%-16s %12s %14s %14s %12s %10s\n' "deno+loadgen" "$(fmt_num "$deno_rps_raw")" "$deno_avg_ms" "$deno_p99_ms" "$(fmt_num "$deno_failed_pct")" "$deno_proto"
printf '\n'
echo "Note: Deno summary reports request bytes as data_sent_estimate; this is not 1:1 with k6 data_sent."
echo "Raw outputs:"
echo "  k6:   $K6_OUT"
echo "  deno: $DENO_OUT"
