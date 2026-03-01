import { LoadgenFFI, printK6LikeSummary, runScriptScenario } from "../ts/mod.ts";
import type { BenchProtocol, RunReport, ScriptRunResult } from "../ts/mod.ts";

const targetUrl = Deno.env.get("TARGET_URL") ??
  "https://bench.local:8082/?s=256k";
const vus = Number(Deno.env.get("VUS") ?? "1");
const durationS = Number(Deno.env.get("DURATION_S") ?? "2");
const insecure = (Deno.env.get("INSECURE") ?? "true").toLowerCase() !== "false";
const threads = Number(Deno.env.get("THREADS") ?? String(Math.max(1, vus)));
const maxStreams = Number(Deno.env.get("MAX_STREAMS") ?? "1");

async function runScriptLogic(proto: BenchProtocol): Promise<ScriptRunResult> {
  return await runScriptScenario({
    vus,
    duration_s: durationS,
    request_timeout_s: 10,
    continue_on_error: false,
    use_cookies: true,
    redirect_policy: "follow",
    execution_mode: "ffi-step",
    step_session_config: {
      protocol: proto,
      insecure,
      request_timeout_s: 10,
      cookie_jar: true,
      redirect_policy: "follow",
      response_headers: true,
      response_body_limit: 16 * 1024,
    },
    steps: [
      {
        name: "capture_response_headers",
        method: "GET",
        url: targetUrl,
        expected_status: 200,
        extract: [
          {
            type: "header",
            name: "content-length",
            as: "content_len",
          },
        ],
        checks: {
          header_exists: ["content-length", "x-rsp"],
        },
      },
      {
        name: "assert_same_content_length",
        method: "GET",
        url: targetUrl,
        expected_status: 200,
        checks: {
          header_equals: {
            "content-length": "{{content_len}}",
          },
          header_exists: ["content-length", "x-rsp"],
        },
      },
    ],
  });
}

async function runBench(proto: BenchProtocol): Promise<RunReport> {
  const bench = new LoadgenFFI({
    url: targetUrl,
    protocol: proto,
    insecure,
    duration_s: durationS,
    clients: vus,
    threads,
    max_streams: maxStreams,
    requests: 1,
  });

  try {
    return await bench.run() as RunReport;
  } finally {
    bench.close();
  }
}

function printScriptSummary(proto: BenchProtocol, result: ScriptRunResult): void {
  const checkPct = result.checks_total > 0
    ? (result.checks_passed / result.checks_total) * 100
    : 0;
  console.log(`script_mode (${proto}):`);
  console.log(
    `  checks=${result.checks_passed}/${result.checks_total} (${checkPct.toFixed(2)}%)`,
  );
  console.log(`  errors_total=${result.errors_total}`);
  console.log(`  iterations=${result.iterations}`);
  console.log(`  steps_executed=${result.steps_executed}`);
}

async function runProtocolPair(proto: BenchProtocol): Promise<void> {
  console.log(`\n=== protocol ${proto} ===`);
  const scriptResult = await runScriptLogic(proto);
  printScriptSummary(proto, scriptResult);

  const report = await runBench(proto);
  printK6LikeSummary(report, {
    scriptPath: "examples/protocol-matrix.ts",
    expectedProtocol: proto,
  });
}

console.log("Protocol matrix run");
console.log(
  JSON.stringify(
    {
      target: targetUrl,
      vus,
      duration_s: durationS,
      threads,
      max_streams: maxStreams,
      insecure,
    },
    null,
    2,
  ),
);

await runProtocolPair("h1");
await runProtocolPair("h2");
await runProtocolPair("h3");
