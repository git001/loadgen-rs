[![CI](https://github.com/git001/loadgen-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/git001/loadgen-rs/actions/workflows/ci.yml)

# loadgen-rs

h2load-compatible HTTP benchmark client written in Rust, supporting HTTP/1.1, HTTP/2, and HTTP/3 (QUIC) with commandline mode and distributed mode.

## Quick Start

```bash
# HTTP/1.1, 100 requests, 4 clients, 2 threads
loadgen-rs -n 100 -c 4 -t 2 --h1 http://localhost:8080/
# HTTPS
loadgen-rs -n 100 -c 4 -t 2 --h1 https://localhost:8080/

# HTTP/2, duration-based, 10 seconds
loadgen-rs -n 0 -D 10s -c 8 -t 4 --alpn h2 https://localhost:8443/

# HTTP/3 (QUIC)
loadgen-rs -n 0 -D 10s -c 4 -t 2 --alpn-list=h3 https://localhost:8443/
```

## Features

- **h2load-compatible CLI** — drop-in replacement for common h2load flags
- **HTTP/1.1** via raw tokio TCP/TLS path (no hyper)
- **HTTP/2** via direct `h2` client (no hyper)
- **HTTP/3** via `quinn` transport backend
- **Duration mode** (`-D`) and **count mode** (`-n`)
- **JSONL/CSV output** for machine-readable results (Python/Matplotlib-friendly)
- **HdrHistogram** latency statistics (p50/p90/p99)
- **Per-worker metrics** — no locks in the hot path
- **Deno FFI SDK (`ts/`)** with typed wrappers for batch runs and step runs
- **k6-like terminal summary** via `printK6LikeSummary()` for comparable output
- **Script scenario engine** with checks/extractors, cookies, and redirect policy
- **Distributed mode** — controller splits work across worker-agents with coordinated start and statistically correct histogram merging
- **Distributed scripted scenarios** — workers execute multi-step request sequences with checks via `POST /run-script`

## Architecture

```
├── src/                        # Rust benchmark core (CLI binary)
│   ├── main.rs                 #   Tokio runtime setup, orchestration
│   ├── cli.rs                  #   clap CLI parsing & validation
│   ├── lib.rs                  #   Library entry point (shared with FFI)
│   ├── bench.rs                #   Benchmark run logic
│   ├── metrics.rs              #   HdrHistogram, counters, merge logic
│   ├── runner.rs               #   Scheduler (count/duration mode, -c/-m)
│   ├── output.rs               #   JSONL/CSV serialization
│   ├── tls.rs                  #   rustls/quinn TLS configuration
│   └── driver/
│       ├── mod.rs              #   Connection enum dispatch
│       ├── h1_raw.rs           #   HTTP/1.1 raw TCP/TLS driver
│       ├── h2.rs               #   HTTP/2 driver (h2 crate, multiplexing)
│       └── h3.rs               #   HTTP/3 driver (quinn)
│
├── crates/loadgen-ffi/         # Rust → C dynamic library for Deno FFI
│   └── src/lib.rs              #   FFI exports: batch run, step run, histogram merge
│
├── ts/                         # Deno TypeScript SDK + worker-agent
│   ├── mod.ts                  #   Public API re-exports
│   ├── types.ts                #   Shared type definitions (configs, results, script types)
│   ├── ffi.ts                  #   LoadgenFFI class — typed wrapper for batch runs
│   ├── step_ffi.ts             #   StepClient — per-request FFI for scripted steps
│   ├── merge_ffi.ts            #   Distributed histogram merging via FFI
│   ├── script_mode.ts          #   Script scenario engine (checks, extractors, cookies, templates)
│   ├── distributed.ts          #   Controller: split work across workers, merge results
│   ├── summary.ts              #   k6-like terminal summary formatter
│   ├── worker-agent.ts         #   HTTP server for distributed workers (/health, /run, /run-script)
│   └── deno.json               #   Deno package config and task definitions
│
├── examples/                   # Ready-to-run Deno scripts
│   ├── simple.ts               #   Minimal batch benchmark example
│   ├── run_h2.ts               #   HTTP/2 benchmark with blog-page scraping
│   ├── distributed.ts          #   Distributed batch run across workers
│   ├── correlation_mvp.ts      #   Scripted scenario: extract + correlate across steps
│   ├── html-check.ts           #   Scripted scenario: HTML body/header checks
│   ├── roundcube-login.ts      #   Scripted scenario: Roundcube Webmail login/logout
│   ├── cookie_redirect_local_mvp.ts  # Cookie jar + redirect policy demo
│   └── ...                     #   More examples (ab-compare, protocol-matrix, etc.)
│
├── docs/                       # Human-readable documentation
│   ├── loadgen-script.md       #   Comprehensive script mode guide
│   ├── distributed-mode.md     #   Distributed architecture and API reference
│   ├── script-mode-ffi.md      #   fetch vs ffi-step execution modes
│   ├── ffi-contract.md         #   Rust ↔ Deno FFI interface contract
│   └── ffi-step-api.md         #   Step-level FFI API reference
│
├── scripts/                    # Shell scripts for benchmarking workflows
│   ├── h2_m_sweep.sh           #   HTTP/2 max-streams parameter sweep
│   ├── h3_m_sweep.sh           #   HTTP/3 parameter sweep
│   ├── ab_compare_report.sh    #   A/B comparison report generator
│   └── ...                     #   More sweep/profiling scripts
│
├── ansible/                    # Worker deployment automation
│   ├── deploy-workers.yml      #   Deploy worker-agents to remote hosts
│   └── stop-workers.yml        #   Stop running worker-agents
│
├── terraform/                  # Infrastructure as Code (Hetzner Cloud)
│   ├── workers.tf              #   Worker VM definitions
│   ├── network.tf              #   VPC / network setup
│   ├── firewall.tf             #   Firewall rules
│   └── ...                     #   Variables, outputs, state
│
└── Containerfile               # Multi-stage Podman/Docker build
```

### Connection Strategy

- `-c N` = N parallel connections/sessions
  - H1: N TCP(+TLS) connections
  - H2: N TCP(+TLS) connections with multiplexing
  - H3: N QUIC connections
- `-m N` = max in-flight requests per connection
  - H2/H3: max concurrent request streams
  - H1: max in-flight per connection
- Total in-flight ≈ `c × m` (with backpressure via semaphores)

### Scheduler

- **Count mode** (`-n`): Shared atomic counter decremented by each worker
- **Duration mode** (`-D`): CancellationToken triggered after the specified duration
- `-D` takes priority over `-n` (h2load semantics)
- **RPS mode** (`--rps`): h2load-compatible per-client request start rate limiter
  in the measurement phase.
  Aggregate target is `--rps * -c`.
  `rps` in output remains completion-based for backward compatibility, while
  `started_rps` reports actual start-rate.

## Build

```bash
cargo build --release
```

## Usage

```bash
# HTTP/1.1, 100 requests, 4 clients, 2 threads
loadgen-rs -n 100 -c 4 -t 2 --h1 http://localhost:8080/

# HTTP/2, duration-based, 10 seconds
loadgen-rs -n 0 -D 10s -c 8 -t 4 --alpn h2 https://localhost:8443/

# HTTP/3 (QUIC)
loadgen-rs -n 0 -D 10s -c 4 -t 2 --alpn-list=h3 https://localhost:8443/

# With custom headers and body
loadgen-rs -n 1000 -c 4 --method POST \
  -H "Content-Type: application/json" \
  -d '{"key":"value"}' \
  --h1 http://localhost:8080/api

# CSV output to file
loadgen-rs -n 1000 -c 4 --format csv -o results.csv --h1 http://localhost:8080/

# Machine-only output (no human summary)
loadgen-rs -n 1000 -c 4 --no-human --format jsonl --h1 http://localhost:8080/

# Tail-friendly mode (better p99, usually less peak throughput)
loadgen-rs -n 0 -D 30s -c 512 -t 8 --tail-friendly --h1 https://127.0.0.1:8082/?s=256k

# Insecure (skip TLS verification)
loadgen-rs -n 100 -c 4 -k --alpn-list=h2 https://self-signed.example.com/
```

## CLI Reference

| Flag | Description | Default |
|------|-------------|---------|
| `-n <N>` | Total requests (0 = unlimited with `-D`) | 1 |
| `-D, --duration <dur>` | Duration-based run (e.g. `10s`, `1m`, `500ms`) | — |
| `--warm-up-time <dur>` | Warm-up duration before measurements start | 0s |
| `--ramp-up-time <dur>` | Ramp-up duration for gradually activating clients/lanes | 0s |
| `-c <N>` | Concurrent clients/connections | 1 |
| `-t <N>` | Worker threads (tokio worker_threads) | 1 |
| `-m <N>` | Max in-flight per connection | 1 |
| `--h1` | Force HTTP/1.1 | — |
| `--alpn-list <h2\|h3>` | Protocol via ALPN (alias: `--alpn`) | — |
| `-4, --v4` | Force IPv4 for DNS resolution/connect | false |
| `-6, --v6` | Force IPv6 for DNS resolution/connect | false |
| `--connect-timeout <dur>` | Connection timeout | 10s |
| `--request-timeout <dur>` | Per-request timeout | 30s |
| `--tcp-quickack` | Enable TCP_QUICKACK on Linux for H1/H2 sockets (best-effort) | false |
| `--method <METHOD>` | HTTP method | GET |
| `-H, --header <H>` | Additional headers (repeatable) | — |
| `-d, --data <DATA>` | Request body | — |
| `--data-file <PATH>` | Request body from file | — |
| `--rps <RPS>` | Per-client target requests/s (aggregate ≈ `RPS * -c`) | — |
| `-k, --insecure` | Skip TLS cert verification | false |
| `--tls-ciphers <SUITES>` | TLS cipher suites (comma-separated) | — |
| `--tls-ca <PATH>` | Additional trusted CA file/dir (PEM/CRT/CER) | — |
| `-o, --output <PATH>` | Output file (default: stdout) | — |
| `--format <jsonl\|csv>` | Output format | jsonl |
| `--no-human` | Suppress human-readable summary, emit only machine output | false |
| `--tail-friendly` | Favor p99/latency fairness over peak throughput | false |
| `--metrics-sample <N>` | Record latency/TTFB for every Nth success (1 = all) | 1 |

## Output Format

### JSONL (default)

One JSON line per run. All fields from `RunReport`:

```json
{
  "proto": "h1",
  "url": "https://example.com/",
  "clients": 4,
  "threads": 2,
  "max_streams": 1,
  "mode": "count",
  "tls_protocol": "TLSv1.3",
  "tls_cipher": "TLS_AES_256_GCM_SHA384",
  "duration_s": 0.0,
  "metrics_sample": 1,
  "requests_target": 100,
  "requests_started": 100,
  "requests_completed": 100,
  "ok": 100,
  "err_total": 0,
  "status_counts": { "200": 100 },
  "status_2xx": 100,
  "status_3xx": 0,
  "status_4xx": 0,
  "status_5xx": 0,
  "rps": 321.25,
  "started_rps": 321.25,
  "bytes_in": 52800,
  "bytes_out": 3700,
  "mbps_in": 1.36,
  "mbps_out": 0.10,
  "latency_min_us": 8020,
  "latency_p50_us": 11783,
  "latency_p90_us": 14999,
  "latency_p99_us": 22399,
  "latency_mean_us": 11986.22,
  "latency_max_us": 26463,
  "latency_stdev_us": 3056.52,
  "ttfb_min_us": 7860,
  "ttfb_p50_us": 11783,
  "ttfb_p90_us": 14983,
  "ttfb_p99_us": 22383,
  "ttfb_mean_us": 11960.54,
  "ttfb_max_us": 26447,
  "ttfb_stdev_us": 3058.22,
  "connect_min_us": 8880,
  "connect_p50_us": 9879,
  "connect_p90_us": 13007,
  "connect_p99_us": 13007,
  "connect_mean_us": 11034.0,
  "connect_max_us": 13007,
  "connect_stdev_us": 1705.48,
  "connect_v4_count": 0,
  "connect_v6_count": 4,
  "connect_addr_counts": { "[2606:4700::6812:1b78]:443": 4 },
  "err_connect": 0,
  "err_tls": 0,
  "err_timeout": 0,
  "err_http": 0,
  "elapsed_s": 0.311286
}
```

Optional fields (omitted when absent): `h3_backend`, `rps_target`, `rps_target_achieved_pct`,
`latency_hist_b64`, `ttfb_hist_b64`, `connect_hist_b64` (V2-Deflate base64 histograms for distributed merge).

### CSV

Stable header + one data row per run. Same fields as JSONL.

## Container

### Build

```bash
podman build -t loadgen-rs -f Containerfile .
```

### Run

```bash
podman run --rm --network host \
  --cap-add=SYS_ADMIN \
  --security-opt seccomp=unconfined \
  loadgen-rs -n 1000 -c 10 -t 4 --h1 http://target:8080/
```

The container runs as root to allow io_uring. `--network host` is required for benchmarking without NAT overhead.
If you target `localhost` / `127.0.0.1` from inside the container, it points to the container itself (not the host).

## Notes From Real-World Runs

- `--alpn` is a supported alias for `--alpn-list` (e.g. `--alpn h2`).
- If all requests fail with TLS errors on `https://...`, check certificate validity first.
  - Example seen in logs: `InvalidCertificate(ExpiredContext ...)`.
  - For local/self-signed/expired test certs, use `--insecure`.
- The tool logs one first-error hint at `WARN` level to make root cause visible without enabling debug logs.
- Human-readable summary is printed first and machine-readable JSON is printed at the end on stdout.
- Throughput/transfer summary uses dynamic units (`KB`, `MB`, `GB`, and `/s`) for easier reading.
- For fair k6 comparisons: `loadgen-rs --rps` is **per-client**. Total target is
  `rps_per_client * clients`. `examples/ab-compare-rps.ts` normalizes from total
  `RPS` to per-client automatically.
- `data_sent_estimate` in Deno summaries is request-byte estimation from the run
  report and is not strictly equivalent to k6 `data_sent`.

Thread tuning sweep for raw H1 (`-t` comparison, default `8,16`):

```bash
scripts/h1_raw_thread_sweep.sh \
  --url "https://127.0.0.1:8082/?s=256k" \
  --threads-list 8,16 \
  --repeats 3
```

Thread sweep in tail-friendly mode:

```bash
scripts/h1_raw_thread_sweep.sh \
  --url "https://127.0.0.1:8082/?s=256k" \
  --threads-list 8,16 \
  --repeats 3 \
  -- --tail-friendly
```

H2 repeated runs (variance reduction, same config multiple times):

```bash
scripts/h2_repeat_summary.sh \
  --url "https://127.0.0.1:8082/?s=256k" \
  --repeats 5 \
  --max-streams 1
```

H2 `-m` sweep (multiplexing effect):

```bash
scripts/h2_m_sweep.sh \
  --url "https://127.0.0.1:8082/?s=256k" \
  --max-streams-list 1,2,4,8 \
  --repeats 3
```

H2 CPU/runtime profiling (`perf stat` around one benchmark run):

```bash
scripts/h2_profile_perf.sh \
  --url "https://127.0.0.1:8082/?s=256k" \
  --max-streams 1
```

## Performance Tuning Applied

- HTTP/1.1 keep-alive reuse enabled (removed forced idle-pool disable).
- HTTP/1.1 response body is drained in a streaming manner instead of collecting full body buffers.
- Duration-mode measurement now starts after client ramp-up synchronization (closer to h2load behavior).
- Fast path for `-m 1` avoids semaphore overhead in the hot loop.
- Request headers are parsed once at startup into typed header structures and reused for all requests.

These changes significantly improved H1 throughput in practice. Small deltas versus `h2load` can still remain due to implementation/runtime differences.

## Deno FFI SDK

The Deno TypeScript SDK (`ts/`) provides typed wrappers around the Rust core via FFI.

### Build the FFI Library

```bash
cargo build --release -p loadgen-ffi
```

This produces `target/release/libloadgen_ffi.so` (Linux).

### Run a Deno Example

```bash
deno run --allow-ffi --allow-net examples/simple.ts https://example.com/
```

### Script Scenarios

Use `runScriptScenario()` (see `ts/script_mode.ts`) for multi-step HTTP workflows with:
- `execution_mode: "fetch"` for pure Deno transport
- `execution_mode: "ffi-step"` for native step transport (`LoadgenStepFFI`)

Reference files: `docs/loadgen-script.md`, `docs/script-mode-ffi.md`, `docs/ffi-step-api.md`

### Stage-Style Load Profile (k6-Like)

```bash
STAGES_JSON='[{"duration_s":5,"target":10},{"duration_s":10,"target":10},{"duration_s":5,"target":0}]' \
STAGE_STEP_S=1 \
RPS=5000 \
deno run --allow-ffi --allow-net=bench.local \
--allow-env=TARGET_URL,INSECURE,MAX_STREAMS,THREADS_CAP,STAGE_STEP_S,INITIAL_CLIENTS,RPS,STAGES_JSON \
examples/ab-stages.ts
```

`examples/ab-stages.ts` emulates stage ramps by splitting stages into short
segments and running them sequentially.

### Deno + k6 Comparison Workflow

Side-by-side comparison scripts to benchmark k6 and Deno+loadgen-rs with
aligned settings and comparable summaries.

#### Fixed-RPS Comparison

```bash
# k6
RPS=5000 DURATION=10s CONCURRENCY=64 INSECURE=true \
  k6 run examples/k6/ab-compare-rps.ts

# Deno + loadgen-rs (same total target)
RPS=5000 DURATION_S=10 CLIENTS=64 THREADS=8 MAX_STREAMS=1 INSECURE=true \
  deno run --allow-ffi --allow-net=bench.local \
  --allow-env=TARGET_URL,DURATION_S,RPS,RPS_PER_CLIENT,CLIENTS,THREADS,MAX_STREAMS,INSECURE \
  examples/ab-compare-rps.ts
```

#### Closed-Model Comparison

```bash
# k6
VUS=4 DURATION=10s INSECURE=true \
  k6 run examples/k6/ab-compare.ts

# Deno + loadgen-rs
VUS=4 DURATION_S=10 THREADS=4 MAX_STREAMS=1 INSECURE=true \
  deno run --allow-ffi --allow-net=bench.local \
  --allow-env=TARGET_URL,VUS,DURATION_S,THREADS,MAX_STREAMS,INSECURE \
  examples/ab-compare.ts
```

#### One-Command Report

```bash
scripts/ab_compare_report.sh \
  --mode rps \
  --target-url "https://bench.local:8082/?s=256k" \
  --duration 10s \
  --rps 5000 \
  --concurrency 64 \
  --clients 64 \
  --threads 8 \
  --max-streams 1
```

The report prints `req/s`, `avg(ms)`, `p99(ms)`, `failed(%)`, and protocol
check status (`h2-check`) for both tools.

## Distributed Mode

Distribute benchmark load across multiple machines with coordinated start and
statistically correct histogram merging via HdrHistogram V2-Deflate.

```
Controller (Deno TS)                 Worker 1              Worker 2
    │                                    │                     │
    ├── GET /health ────────────────────►│                     │
    ├── GET /health ──────────────────────────────────────────►│
    ├── split -c evenly                  │                     │
    ├── start_at = now + 500ms           │                     │
    ├── POST /run ──────────────────────►│                     │
    ├── POST /run ────────────────────────────────────────────►│
    │                        sleep until start_at              │
    │                        LoadgenFFI.run(config)            │
    │◄── RunReport ──────────────────────┤                     │
    │◄── RunReport ───────────────────────────────────────────┤
    ├── FFI: merge histograms            │                     │
    └── DistributedResult                │                     │
```

### Quick Start

```bash
# 1. Build the FFI library
cargo build --release -p loadgen-ffi

# 2. Start 2 local workers (ports 9091, 9092)
bash examples/worker-start.sh

# 3. Run distributed benchmark (in a separate terminal)
TARGET_URL="https://bench.local:8082/?s=1k" CLIENTS=100 DURATION_S=10 \
  deno run --allow-ffi --allow-net --allow-read --allow-env \
  examples/distributed.ts http://localhost:9091 http://localhost:9092
```

### How It Works

- **Client splitting**: `-c 100` across 3 workers → 34, 33, 33 clients each
- **Duration mode**: all workers run for the same duration
- **Count mode**: total requests (`-n`) split evenly across workers
- **Coordinated start**: controller sends an ISO 8601 timestamp; workers sleep
  until that moment before starting
- **Histogram merge**: each worker returns V2-Deflate encoded HdrHistograms as
  base64. The controller merges via `loadgen_merge_reports()` FFI — statistically
  correct (no percentile averaging)

### Worker Agent

Start a worker on each machine:

```bash
deno run --allow-ffi --allow-net --allow-read --allow-env \
  ts/worker-agent.ts --listen 0.0.0.0:9091 --lib ./target/release/libloadgen_ffi.so
```

Endpoints: `GET /health`, `GET /status`, `POST /run`, `POST /run-script`.

### Scripted Scenarios on Workers

Workers also support scripted scenarios via `POST /run-script`. This uses the
same `runScriptScenario()` engine as local script mode, but executed remotely
on each worker — including checks, extractors, cookie handling, and all step
session features.

```bash
# Run scripted scenario across 2 remote workers
TARGET_URL=https://example.com deno run --allow-net --allow-env \
  examples/html-check.ts \
  http://worker1:9091 http://worker2:9091
```

The included `html-check.ts` example verifies HTML content and headers
from a target URL across all workers:

- Sends `GET $TARGET_URL` via `ffi-step` with configurable protocol
- Checks `body_includes` for a configurable substring
- Checks `header_includes` for a configurable header value
- Verifies that all check counts match the iteration count

```
=== per-worker results ===

  worker                                  iterations  html_match  header_match  ok?
-------------------------------------------------------------------------------------
  http://worker1:9091                             62          62            62  PASS
  http://worker2:9091                             47          47            47  PASS
-------------------------------------------------------------------------------------
  TOTAL                                          109         109           109  PASS
```

Environment variables: `TARGET_URL` (required), `BODY_INCLUDES`, `HEADER_KEY`,
`HEADER_SUBSTR`, `PROTOCOL` (default h2), `VUS` (default 3), `DURATION_S` (default 5).

A second example, `roundcube-login.ts`, demonstrates a multi-step
login/logout workflow against a Roundcube Webmail instance — including CSRF
token extraction, form submission, session cookies, and logout verification:

```bash
deno run --allow-net --allow-env \
  examples/roundcube-login.ts \
  http://worker1:9091 http://worker2:9091
```

Each iteration performs three steps:

1. **GET login page** — checks title, extracts CSRF `_token` via DOM selector
2. **POST login form** — submits URL-encoded credentials with `{{token}}`,
   checks for inbox title and empty mailbox indicator, extracts `request_token`
   for logout
3. **GET logout** — uses `{{logout_token}}` in the URL, verifies redirect back
   to login page

```
=== per-worker results ===

  worker                                  iterations    login_ok   logout_ok  ok?
----------------------------------------------------------------------------------
  http://worker1:9091                             45          45          45  PASS
  http://worker2:9091                             45          45          45  PASS
----------------------------------------------------------------------------------
  TOTAL                                           90          90          90  PASS
```

Environment variables: `TARGET_URL` (required), `PROTOCOL`, `VUS` (default 1),
`DURATION_S` (default 10), `ROUNDCUBE_USER` (required), `ROUNDCUBE_PASS` (required).

### Programmatic API

```typescript
import { Controller } from "./ts/mod.ts";

const controller = new Controller({
  workers: ["http://worker1:9091", "http://worker2:9091"],
  config: {
    url: "https://target:8443/",
    protocol: "h2",
    clients: 100,
    threads: 4,
    duration_s: 30,
    insecure: true,
  },
});

const result = await controller.run();
// result.merged_report  — combined RunReport
// result.worker_reports — individual worker reports
// result.worker_errors  — any failed workers
```

### Infrastructure with Terraform + Ansible

For cloud-based distributed benchmarks, Terraform provisions Hetzner Cloud
machines and generates the Ansible inventory automatically.

```bash
# 1. Provision infrastructure
cd terraform
cp terraform.tfvars.example terraform.tfvars
# Edit terraform.tfvars — set hcloud_token, ssh_key_name, worker_count

terraform init
terraform apply

# 2. Generate Ansible inventory from Terraform outputs
./generate-inventory.sh
# → writes ansible/inventory.ini with public IPs

# 3. Deploy workers + run benchmark + tear down
cd ..
scripts/distributed-remote.sh \
  -i ansible/inventory.ini \
  --target-url "https://bench.target:8443/?s=256k" \
  -c 500 -D 30 --stop-after

# 4. Destroy infrastructure when done
cd terraform && terraform destroy
```

### Remote Deployment via Ansible (Manual Inventory)

If you already have machines, create the inventory manually:

```bash
cp ansible/inventory.example.ini ansible/inventory.ini
# Edit ansible/inventory.ini — add your worker hosts

# Deploy + run + stop in one command
scripts/distributed-remote.sh \
  -i ansible/inventory.ini \
  --target-url "https://bench.local:8082/?s=256k" \
  -c 200 -D 30 --stop-after

# Or step by step:
scripts/distributed-remote.sh -i ansible/inventory.ini --deploy-only
scripts/distributed-remote.sh -i ansible/inventory.ini --skip-deploy \
  --target-url "https://target:8443/" -c 200 -D 30
ansible-playbook -i ansible/inventory.ini ansible/stop-workers.yml
```

The playbook handles: Deno installation, file sync, systemd service unit
(or tmux fallback), and health checks. See `ansible/inventory.example.ini`
for configuration.

Full documentation: `docs/distributed-mode.md`

## Dependencies

### Core binary (`loadgen-rs`)

| Crate | Purpose |
|-------|---------|
| tokio, tokio-util | Async runtime |
| h2, http, httparse | HTTP/2 client + HTTP/1.1 raw driver |
| bytes, futures-util | Byte buffers + stream utilities |
| tokio-rustls, rustls | TLS (ring crypto backend, TLS 1.2 + 1.3) |
| rustls-pemfile, webpki-roots | PEM parsing + Mozilla CA roots |
| quinn, h3, h3-quinn | HTTP/3 / QUIC backend |
| clap | CLI parsing (derive) |
| hdrhistogram | Latency statistics (p50/p90/p99) |
| serde, serde_json | JSONL/CSV serialization |
| base64 | V2-Deflate histogram encoding for distributed merge |
| tracing, tracing-subscriber | Structured logging with env-filter |
| anyhow | Error handling |
| mimalloc | Global memory allocator for lower allocation overhead |
| socket2 | Socket options (TCP_QUICKACK, SO_REUSEADDR) |
| libc | Low-level syscall access |

### FFI crate (`loadgen-ffi`)

| Crate | Purpose |
|-------|---------|
| loadgen-rs | Core library (path dependency) |
| reqwest | Step-level HTTP client (rustls-tls, cookies, HTTP/2, HTTP/3) |
| tokio | Async runtime for step execution |
| serde, serde_json | FFI JSON interface |
