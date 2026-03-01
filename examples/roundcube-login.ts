/**
 * Distributed scripted scenario: Roundcube Webmail login/logout verification.
 *
 * Tests the full login flow on each worker:
 *   1. GET login page, extract CSRF _token
 *   2. POST login form with credentials, verify empty mailbox
 *   3. GET logout, verify redirect back to login page
 *
 * Usage:
 *   deno run --allow-net examples/roundcube-login.ts \
 *     http://worker1:9091 http://worker2:9091
 *
 * Environment:
 *   TARGET_URL       (required)  — Roundcube base URL, e.g. https://mail.example.com/mail/
 *   PROTOCOL         (default: h2)  — h1, h2, or h3
 *   VUS              (default: 1)
 *   DURATION_S       (default: 10)
 *   ROUNDCUBE_USER   (required)  — login username
 *   ROUNDCUBE_PASS   (required)  — login password
 */

import type { ScriptScenarioConfig, ScriptRunResult } from "../ts/types.ts";

const workerUrls = Deno.args.filter((a) => a.startsWith("http"));
if (workerUrls.length === 0) {
  console.error("Usage: roundcube-login.ts <worker-url> [worker-url...]");
  Deno.exit(1);
}

const targetUrl = Deno.env.get("TARGET_URL");
if (!targetUrl) {
  console.error("TARGET_URL is required (e.g. https://mail.example.com/mail/)");
  Deno.exit(1);
}
const protocol = (Deno.env.get("PROTOCOL") ?? "h2") as "h1" | "h2" | "h3";
const vus = Number(Deno.env.get("VUS") ?? "1");
const durationS = Number(Deno.env.get("DURATION_S") ?? "10");
const user = Deno.env.get("ROUNDCUBE_USER");
const pass = Deno.env.get("ROUNDCUBE_PASS");
if (!user || !pass) {
  console.error("ROUNDCUBE_USER and ROUNDCUBE_PASS are required");
  Deno.exit(1);
}

// URL-encode credentials for form body
const encodedUser = encodeURIComponent(user);
const encodedPass = encodeURIComponent(pass);

const baseUrl = targetUrl.replace(/\/+$/, "");

const config: ScriptScenarioConfig = {
  vus,
  duration_s: durationS,
  execution_mode: "ffi-step",
  use_cookies: true,
  redirect_policy: "follow",
  step_session_config: {
    protocol,
    insecure: true,
    cookie_jar: true,
    response_headers: true,
    response_body_limit: 2_000_000,
  },
  steps: [
    {
      name: "login-page",
      method: "GET",
      url: baseUrl + "/",
      capture_body: true,
      headers: {
        "user-agent":
          "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
        accept:
          "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        "accept-language": "en-US,en;q=0.5",
      },
      checks: {
        status_in: [200],
        body_includes: ["Roundcube Webmail :: Welcome to Roundcube Webmail"],
      },
      extract: [
        {
          type: "dom",
          selector: 'input[name="_token"]',
          attribute: "value",
          as: "token",
        },
      ],
    },
    {
      name: "login-submit",
      method: "POST",
      url: baseUrl + "/?_task=login",
      capture_body: true,
      headers: {
        "content-type": "application/x-www-form-urlencoded",
        "user-agent":
          "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
      },
      body: `_task=login&_action=login&_timezone=Europe%2FVienna&_url=&_token={{token}}&_user=${encodedUser}&_pass=${encodedPass}`,
      checks: {
        status_in: [200],
        body_includes: [
          "Roundcube Webmail :: Inbox",
          'data-label-msg="The list is empty."',
        ],
      },
      extract: [
        {
          type: "regex",
          pattern: '"request_token":"([^"]+)"',
          group: 1,
          as: "logout_token",
        },
      ],
    },
    {
      name: "logout",
      method: "GET",
      url: baseUrl + "/?_task=logout&_token={{logout_token}}",
      capture_body: true,
      headers: {
        "user-agent":
          "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
      },
      checks: {
        status_in: [200],
        body_includes: ["Roundcube Webmail :: Welcome to Roundcube Webmail"],
      },
    },
  ],
};

// --- Health check all workers ---
console.log(
  `roundcube-login: ${workerUrls.length} workers, ${vus} VUs, ${durationS}s, protocol ${protocol}`,
);
console.log(`target: ${baseUrl}\n`);

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
console.log("\nstarting roundcube login/logout scenario on all workers...\n");

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
    "login_ok".padStart(12) +
    "logout_ok".padStart(12) +
    "  ok?",
);
console.log("-".repeat(82));

let totalIterations = 0;
let totalLoginOk = 0;
let totalLogoutOk = 0;
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

  // body_includes checks cover both login-submit and logout steps
  const bodyCheck = s.check_summary["body_includes"];
  // Each iteration has 3 body_includes checks: login-page title, mailbox text, logout title
  // login_ok = mailbox text found, logout_ok = logout title found
  // Since body_includes aggregates all steps, we use passed/total ratio
  const bodyPassed = bodyCheck?.passed ?? 0;
  const bodyTotal = bodyCheck?.total ?? 0;

  // With 3 body_includes checks per iteration, all passing means full success
  const allPassed = bodyPassed === bodyTotal && bodyTotal > 0;
  const loginOk = allPassed ? iterations : Math.floor(bodyPassed / 3);
  const logoutOk = allPassed ? iterations : Math.floor(bodyPassed / 3);

  totalIterations += iterations;
  totalLoginOk += loginOk;
  totalLogoutOk += logoutOk;

  console.log(
    `  ${workerUrls[i].padEnd(38)}` +
      `${iterations}`.padStart(12) +
      `${loginOk}`.padStart(12) +
      `${logoutOk}`.padStart(12) +
      `  ${allPassed ? "PASS" : "FAIL"}`,
  );

  // Print check details
  if (s.check_summary) {
    for (const [name, counter] of Object.entries(s.check_summary)) {
      if (counter.failed > 0) {
        console.log(
          `    check ${name}: ${counter.passed}/${counter.total} passed (${counter.failed} failed)`,
        );
      }
    }
  }

  // Print step errors
  if (s.step_errors) {
    for (const [msg, count] of Object.entries(s.step_errors)) {
      if (count > 0) {
        console.log(`    step_error (${count}x): ${msg}`);
      }
    }
  }
}

console.log("-".repeat(82));
const allPass =
  totalErrors === 0 &&
  totalIterations > 0 &&
  totalLoginOk === totalIterations &&
  totalLogoutOk === totalIterations;
console.log(
  `  ${"TOTAL".padEnd(38)}` +
    `${totalIterations}`.padStart(12) +
    `${totalLoginOk}`.padStart(12) +
    `${totalLogoutOk}`.padStart(12) +
    `  ${allPass ? "PASS" : "FAIL"}`,
);

if (totalErrors > 0) {
  console.log(`\n${totalErrors} worker(s) failed`);
  Deno.exit(1);
}
