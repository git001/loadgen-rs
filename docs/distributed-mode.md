# Distributed Mode

Distribute benchmark load across multiple machines with statistically correct histogram merging.

## Architecture

```
Controller (Deno TypeScript)
├── health check workers (GET /health)
├── split -c evenly across workers
├── calculate start_at = now + 500ms
├── POST /run to each worker (concurrent)
│
│  Worker 1 (Deno HTTP)        Worker 2 (Deno HTTP)
│  ├── sleep until start_at    ├── sleep until start_at
│  ├── LoadgenFFI.run(config)  ├── LoadgenFFI.run(config)
│  └── return RunReport+hist   └── return RunReport+hist
│
├── collect all RunReports
├── FFI: loadgen_merge_reports(reports_json)
└── return DistributedResult { merged_report, worker_reports }
```

## Quick Start

### 1. Build the FFI library

```bash
cargo build --release -p loadgen-ffi
```

### 2. Start workers

On each worker machine (or locally for testing):

```bash
# Single worker
deno run --allow-ffi --allow-net --allow-read --allow-env \
  ts/worker-agent.ts --listen 0.0.0.0:9091 --lib ./target/release/libloadgen_ffi.so

# Or use the helper script for local testing (starts 2 workers)
bash examples/worker-start.sh
```

### 3. Run the controller

```bash
deno run --allow-ffi --allow-net --allow-read --allow-env \
  examples/distributed.ts http://localhost:9091 http://localhost:9092
```

Environment variables for the example:

| Variable | Default | Description |
|----------|---------|-------------|
| `TARGET_URL` | `https://bench.local:8082/?s=256k` | Benchmark target |
| `INSECURE` | `true` | Skip TLS verification |
| `CLIENTS` | `100` | Total clients (split across workers) |
| `DURATION_S` | `10` | Benchmark duration |
| `THREADS` | `4` | Threads per worker |
| `MAX_STREAMS` | `1` | Max H2/H3 streams per connection |

## Worker Agent

The worker-agent is a Deno HTTP server exposing three endpoints:

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Returns `{ status, abi_version }` |
| `/status` | GET | Returns `{ status }` (`idle`/`running`/`finished`/`error`) |
| `/run` | POST | Runs a benchmark and returns `RunReport` |
| `/run-script` | POST | Runs a scripted scenario and returns `ScriptRunResult` |

### POST /run body

```json
{
  "config": {
    "url": "https://target:8443/",
    "protocol": "h2",
    "clients": 50,
    "threads": 4,
    "duration_s": 10,
    "insecure": true,
    "export_histograms": true
  },
  "start_at": "2025-01-15T10:00:00.500Z"
}
```

- `export_histograms` is forced to `true` by the worker (needed for merge).
- `start_at` (ISO 8601) enables coordinated start across workers.
- Rejects concurrent runs with HTTP 409.

### POST /run-script body

```json
{
  "config": {
    "vus": 3,
    "duration_s": 10,
    "execution_mode": "ffi-step",
    "use_cookies": true,
    "step_session_config": {
      "protocol": "h2",
      "insecure": true,
      "response_headers": true,
      "response_body_limit": 2000000
    },
    "steps": [
      {
        "name": "get-page",
        "method": "GET",
        "url": "https://example.com/",
        "capture_body": true,
        "checks": { "status_in": [200] },
        "extract": [{ "type": "dom", "selector": "h1", "as": "title" }]
      }
    ]
  },
  "start_at": "2025-01-15T10:00:00.500Z"
}
```

- The worker forces `execution_mode: "ffi-step"` and injects the local `lib` path.
- Returns a `ScriptRunResult` with iteration counts, check summaries, and step errors.
- See [loadgen-script.md](loadgen-script.md) for the full script configuration reference.

## Controller API

```typescript
import { Controller } from "./ts/mod.ts";
import type { DistributedConfig } from "./ts/mod.ts";

const dc: DistributedConfig = {
  workers: ["http://worker1:9091", "http://worker2:9091"],
  config: {
    url: "https://target:8443/",
    protocol: "h2",
    clients: 100,     // split: 50 per worker
    threads: 4,
    duration_s: 30,
    insecure: true,
  },
  start_delay_ms: 500,   // default: 500ms
  timeout_ms: 600_000,   // default: 10 minutes
};

const controller = new Controller(dc);
const result = await controller.run();

console.log(result.merged_report);     // merged RunReport
console.log(result.worker_reports);    // individual worker reports
console.log(result.worker_errors);     // any failed workers
```

## Work Distribution

- **Clients (`-c`)**: Split evenly. E.g., 100 clients / 3 workers = 34, 33, 33.
- **Duration mode**: All workers run for the same duration.
- **Count mode**: Total requests split evenly across workers.
- **Threads**: Capped to `min(assigned_clients, config.threads)` per worker.

## Coordinated Start

The controller calculates `start_at = now + start_delay_ms` and sends this timestamp to all workers. Each worker sleeps until the timestamp before starting. Clock skew < 10ms is acceptable for benchmarks.

## Histogram Merging

Worker reports include HdrHistogram V2-Deflate encoded as base64 strings (`latency_hist_b64`, `ttfb_hist_b64`, `connect_hist_b64`). The controller merges these via the `loadgen_merge_reports()` FFI function, which:

1. Deserializes each histogram from base64 → V2Deflate → HdrHistogram
2. Merges with `Histogram::add()` (statistically correct — no percentile averaging)
3. Recomputes all percentile fields from the merged histogram

This is the same merge algorithm used internally for worker-thread metrics.
