/**
 * Distributed benchmark example.
 *
 * Usage:
 *   # Terminal 1: start workers
 *   bash examples/worker-start.sh
 *
 *   # Terminal 2: run controller
 *   deno run --allow-ffi --allow-net --allow-read examples/distributed.ts \
 *     http://localhost:9091 http://localhost:9092
 */

import { Controller, printK6LikeSummary } from "../ts/mod.ts";
import type { DistributedConfig } from "../ts/mod.ts";

const workerUrls = Deno.args.filter((a) => a.startsWith("http"));
if (workerUrls.length === 0) {
  console.error("Usage: distributed.ts <worker-url> [worker-url...]");
  console.error("  e.g.: distributed.ts http://localhost:9091 http://localhost:9092");
  Deno.exit(1);
}

const targetUrl = Deno.env.get("TARGET_URL") ?? "https://bench.local:8082/?s=256k";
const protocol = (Deno.env.get("PROTOCOL") ?? "h2") as "h1" | "h2" | "h3";
const insecure = (Deno.env.get("INSECURE") ?? "true").toLowerCase() !== "false";
const clients = Number(Deno.env.get("CLIENTS") ?? "100");
const durationS = Number(Deno.env.get("DURATION_S") ?? "10");
const threads = Number(Deno.env.get("THREADS") ?? "4");
const maxStreams = Number(Deno.env.get("MAX_STREAMS") ?? "1");

const dc: DistributedConfig = {
  workers: workerUrls,
  config: {
    url: targetUrl,
    protocol,
    insecure,
    clients,
    threads,
    max_streams: maxStreams,
    duration_s: durationS,
    requests: 1,
  },
  start_delay_ms: 500,
  timeout_ms: durationS * 1000 + 30_000,
};

console.log(`controller: ${workerUrls.length} workers, ${clients} total clients, ${durationS}s`);
console.log(`target: ${targetUrl}`);
console.log(`workers: ${workerUrls.join(", ")}`);

const controller = new Controller(dc);

// Health check
const health = await controller.healthCheck();
for (let i = 0; i < workerUrls.length; i++) {
  const h = health[i];
  console.log(
    `  ${workerUrls[i]}: ${h ? `ok (abi=${h.abi_version})` : "UNREACHABLE"}`,
  );
}

console.log("\nstarting distributed benchmark...\n");
const result = await controller.run();

if (result.worker_errors.length > 0) {
  console.log("worker errors:");
  for (const e of result.worker_errors) {
    console.log(`  ${e.worker_url}: ${e.error}`);
  }
  console.log();
}

console.log(`=== merged results (${result.worker_reports.length} workers) ===\n`);
printK6LikeSummary(result.merged_report, {
  scriptPath: "examples/distributed.ts",
  expectedProtocol: protocol,
});

console.log("\n=== per-worker summary ===");
for (let i = 0; i < result.worker_reports.length; i++) {
  const r = result.worker_reports[i];
  console.log(
    `  worker ${i + 1}: clients=${r.clients} rps=${r.rps.toFixed(1)} ` +
      `p99=${r.latency_p99_us}us completed=${r.requests_completed} elapsed=${r.elapsed_s.toFixed(2)}s`,
  );
}
