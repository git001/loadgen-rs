/**
 * loadgen worker-agent: HTTP server that accepts benchmark jobs from a controller.
 *
 * Usage:
 *   deno run --allow-ffi --allow-net --allow-read ts/worker-agent.ts \
 *     [--listen host:port] [--lib path/to/libloadgen_ffi.so]
 *
 * Endpoints:
 *   GET  /health      → { status, abi_version }
 *   GET  /status      → { status }
 *   POST /run         → RunReport (blocks until benchmark completes)
 *   POST /run-script  → ScriptRunResult (blocks until scenario completes)
 */

import { LoadgenFFI } from "./ffi.ts";
import { runScriptScenario } from "./script_mode.ts";
import type {
  BenchConfig,
  RunReport,
  ScriptScenarioConfig,
  WorkerHealthResponse,
  WorkerRunRequest,
  WorkerStatus,
} from "./types.ts";

function parseArgs(): { hostname: string; port: number; lib: string } {
  const args = Deno.args;
  let listen = "0.0.0.0:9090";
  let lib = "./target/release/libloadgen_ffi.so";

  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--listen" && i + 1 < args.length) {
      listen = args[++i];
    } else if (args[i] === "--lib" && i + 1 < args.length) {
      lib = args[++i];
    }
  }

  const [hostname, portStr] = listen.includes(":")
    ? [listen.substring(0, listen.lastIndexOf(":")), listen.substring(listen.lastIndexOf(":") + 1)]
    : ["0.0.0.0", listen];

  return { hostname, port: parseInt(portStr, 10), lib };
}

let currentStatus: WorkerStatus = "idle";
let lastReport: RunReport | null = null;

const { hostname, port, lib } = parseArgs();

function jsonResponse(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: { "content-type": "application/json" },
  });
}

async function handleRun(req: Request): Promise<Response> {
  if (currentStatus === "running") {
    return jsonResponse({ error: "worker is already running a benchmark" }, 409);
  }

  let body: WorkerRunRequest;
  try {
    body = (await req.json()) as WorkerRunRequest;
  } catch {
    return jsonResponse({ error: "invalid JSON body" }, 400);
  }

  if (!body.config?.url) {
    return jsonResponse({ error: "config.url is required" }, 400);
  }

  const config: BenchConfig = { ...body.config, export_histograms: true };

  // Coordinated start: sleep until start_at timestamp
  if (body.start_at) {
    const startTime = Temporal.Instant.from(body.start_at).epochMilliseconds;
    const now = Temporal.Now.instant().epochMilliseconds;
    if (startTime > now) {
      await new Promise((resolve) => setTimeout(resolve, startTime - now));
    }
  }

  const proto = config.protocol ?? "h2";
  const mode = config.duration_s ? `${config.duration_s}s` : `${config.requests ?? 1} reqs`;
  console.log(`run: ${mode}, ${config.clients ?? 1} clients, protocol ${proto}, target: ${config.url}`);

  currentStatus = "running";
  lastReport = null;
  const t0 = performance.now();

  const bench = new LoadgenFFI(config, lib);
  try {
    const report = await bench.run();
    lastReport = report;
    currentStatus = "finished";
    const elapsed = ((performance.now() - t0) / 1000).toFixed(2);
    console.log(`run: finished in ${elapsed}s, ${report.requests_completed} reqs, ${report.rps.toFixed(1)} rps`);
    return jsonResponse(report);
  } catch (e) {
    currentStatus = "error";
    const elapsed = ((performance.now() - t0) / 1000).toFixed(2);
    const msg = e instanceof Error ? e.message : String(e);
    console.error(`run: failed after ${elapsed}s — ${msg}`);
    return jsonResponse({ error: msg }, 500);
  } finally {
    bench.close();
  }
}

async function handleRunScript(req: Request): Promise<Response> {
  if (currentStatus === "running") {
    return jsonResponse({ error: "worker is already running a benchmark" }, 409);
  }

  let body: { config: ScriptScenarioConfig; start_at?: string };
  try {
    body = (await req.json()) as { config: ScriptScenarioConfig; start_at?: string };
  } catch {
    return jsonResponse({ error: "invalid JSON body" }, 400);
  }

  if (!body.config?.steps || body.config.steps.length === 0) {
    return jsonResponse({ error: "config.steps is required and must not be empty" }, 400);
  }

  // Force ffi-step execution mode and inject lib path
  const config: ScriptScenarioConfig = {
    ...body.config,
    execution_mode: "ffi-step",
    step_session_config: {
      ...body.config.step_session_config,
      response_headers: true,
      response_body_limit: body.config.step_session_config?.response_body_limit ?? 1_000_000,
    },
  };

  // Coordinated start
  if (body.start_at) {
    const startTime = Temporal.Instant.from(body.start_at).epochMilliseconds;
    const now = Temporal.Now.instant().epochMilliseconds;
    if (startTime > now) {
      await new Promise((resolve) => setTimeout(resolve, startTime - now));
    }
  }

  const stepNames = config.steps.map((s) => s.name).join(", ");
  const proto = config.step_session_config?.protocol ?? "h2";
  const targets = [...new Set(config.steps.map((s) => s.url))].join(", ");
  console.log(`run-script: ${config.vus} VUs, ${config.duration_s}s, protocol ${proto}, steps: [${stepNames}], target: ${targets}`);

  currentStatus = "running";
  const t0 = performance.now();

  try {
    const result = await runScriptScenario(config, lib);
    currentStatus = "finished";
    const elapsed = ((performance.now() - t0) / 1000).toFixed(2);
    console.log(`run-script: finished in ${elapsed}s, ${result.iterations} iterations, ${result.checks_passed}/${result.checks_total} checks passed`);
    return jsonResponse(result);
  } catch (e) {
    currentStatus = "error";
    const elapsed = ((performance.now() - t0) / 1000).toFixed(2);
    const msg = e instanceof Error ? e.message : String(e);
    console.error(`run-script: failed after ${elapsed}s — ${msg}`);
    return jsonResponse({ error: msg }, 500);
  }
}

const server = Deno.serve(
  { hostname, port },
  (req: Request): Response | Promise<Response> => {
    const url = new URL(req.url);

    if (req.method === "GET" && url.pathname === "/health") {
      // Probe ABI version via a temporary FFI open
      let abiVersion = 0;
      try {
        const probe = new LoadgenFFI({ url: "http://probe", requests: 0 }, lib);
        abiVersion = probe.abiVersion();
        probe.close();
      } catch {
        // ABI version unknown
      }
      const resp: WorkerHealthResponse = {
        status: currentStatus === "running" ? "running" : "idle",
        abi_version: abiVersion,
      };
      return jsonResponse(resp);
    }

    if (req.method === "GET" && url.pathname === "/status") {
      return jsonResponse({ status: currentStatus });
    }

    if (req.method === "POST" && url.pathname === "/run") {
      return handleRun(req);
    }

    if (req.method === "POST" && url.pathname === "/run-script") {
      return handleRunScript(req);
    }

    return jsonResponse({ error: "not found" }, 404);
  },
);

console.log(`loadgen worker-agent listening on ${hostname}:${port}`);

await server.finished;
