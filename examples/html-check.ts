/**
 * Distributed scripted scenario: HTML content + header verification.
 *
 * Sends requests to a target URL from each worker, verifies response body
 * and headers using configurable checks. All check counts should match
 * the iteration count — any mismatch indicates a parsing or check bug.
 *
 * Usage:
 *   deno run --allow-net --allow-env examples/html-check.ts \
 *     http://worker1:9091 http://worker2:9091
 *
 * Environment:
 *   TARGET_URL      (required) — URL to check
 *   BODY_INCLUDES   (optional) — substring to verify in response body
 *   HEADER_KEY      (optional) — header name to check (e.g. "set-cookie")
 *   HEADER_SUBSTR   (optional) — substring to verify in header value
 *   PROTOCOL        (default: h2)  — h1, h2, or h3
 *   VUS             (default: 3)
 *   DURATION_S      (default: 5)
 */

import type { ScriptScenarioConfig, ScriptRunResult } from "../ts/types.ts";

const workerUrls = Deno.args.filter((a) => a.startsWith("http"));
if (workerUrls.length === 0) {
  console.error("Usage: html-check.ts <worker-url> [worker-url...]");
  Deno.exit(1);
}

const targetUrl = Deno.env.get("TARGET_URL");
if (!targetUrl) {
  console.error("TARGET_URL is required");
  Deno.exit(1);
}
const bodyIncludes = Deno.env.get("BODY_INCLUDES");
const headerKey = Deno.env.get("HEADER_KEY");
const headerSubstr = Deno.env.get("HEADER_SUBSTR");
const protocol = (Deno.env.get("PROTOCOL") ?? "h2") as "h1" | "h2" | "h3";
const vus = Number(Deno.env.get("VUS") ?? "3");
const durationS = Number(Deno.env.get("DURATION_S") ?? "5");

// Build checks dynamically
const checks: Record<string, unknown> = { status_in: [200, 403] };
if (bodyIncludes) {
  checks.body_includes = [bodyIncludes];
}
if (headerKey && headerSubstr) {
  checks.header_includes = { [headerKey]: headerSubstr };
}

const config: ScriptScenarioConfig = {
  vus,
  duration_s: durationS,
  execution_mode: "ffi-step",
  use_cookies: false,
  step_session_config: {
    protocol,
    insecure: true,
    response_headers: true,
    response_body_limit: 2_000_000,
  },
  steps: [
    {
      name: "html-get",
      method: "GET",
      url: targetUrl,
      capture_body: true,
      headers: {
        "user-agent":
          "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
        accept:
          "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        "accept-language": "en-US,en;q=0.5",
      },
      checks: checks as ScriptScenarioConfig["steps"][0]["checks"],
    },
  ],
};

// --- Health check all workers ---
console.log(`html-check: ${workerUrls.length} workers, ${vus} VUs, ${durationS}s, protocol ${protocol}`);
console.log(`target: ${targetUrl}\n`);

for (const url of workerUrls) {
  try {
    const resp = await fetch(`${url}/health`);
    const h = await resp.json();
    console.log(`  ${url}: ${h.status} (abi=${h.abi_version})`);
  } catch (e) {
    console.error(`  ${url}: UNREACHABLE - ${e}`);
    Deno.exit(1);
  }
}

// --- Send script to all workers concurrently ---
console.log("\nstarting scripted scenario on all workers...\n");

const startAt = Temporal.Now.instant().add({ milliseconds: 500 }).toString();

const results = await Promise.allSettled(
  workerUrls.map(async (url) => {
    const resp = await fetch(`${url}/run-script`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ config, start_at: startAt }),
      signal: AbortSignal.timeout((durationS + 30) * 1000),
    });
    if (!resp.ok) {
      const err = await resp.text();
      throw new Error(`HTTP ${resp.status}: ${err}`);
    }
    return (await resp.json()) as ScriptRunResult;
  }),
);

// --- Print per-worker results ---
console.log("=== per-worker results ===\n");
console.log(
  "  worker".padEnd(40) +
    "iterations".padStart(12) +
    "html_match".padStart(12) +
    "header_match".padStart(14) +
    "  ok?",
);
console.log("-".repeat(85));

let totalIterations = 0;
let totalHtmlMatch = 0;
let totalHeaderMatch = 0;
let totalErrors = 0;

for (let i = 0; i < workerUrls.length; i++) {
  const r = results[i];
  if (r.status === "rejected") {
    console.log(`  ${workerUrls[i].padEnd(38)} ERROR: ${r.reason}`);
    totalErrors++;
    continue;
  }

  const s = r.value;
  const iterations = s.iterations;
  const htmlCheck = s.check_summary["body_includes"];
  const headerCheck = s.check_summary["header_includes"];
  const htmlPassed = htmlCheck?.passed ?? iterations;
  const headerPassed = headerCheck?.passed ?? iterations;
  const allMatch = htmlPassed === iterations && headerPassed === iterations;

  totalIterations += iterations;
  totalHtmlMatch += htmlPassed;
  totalHeaderMatch += headerPassed;

  console.log(
    `  ${workerUrls[i].padEnd(38)}` +
      `${iterations}`.padStart(12) +
      `${htmlPassed}`.padStart(12) +
      `${headerPassed}`.padStart(14) +
      `  ${allMatch ? "PASS" : "FAIL"}`,
  );
}

console.log("-".repeat(85));
console.log(
  `  ${"TOTAL".padEnd(38)}` +
    `${totalIterations}`.padStart(12) +
    `${totalHtmlMatch}`.padStart(12) +
    `${totalHeaderMatch}`.padStart(14) +
    `  ${totalIterations === totalHtmlMatch && totalIterations === totalHeaderMatch ? "PASS" : "FAIL"}`,
);

if (totalErrors > 0) {
  console.log(`\n${totalErrors} worker(s) failed`);
  Deno.exit(1);
}
