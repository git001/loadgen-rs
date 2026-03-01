import { LoadgenFFI, printK6LikeSummary } from "../ts/mod.ts";
import type { RunReport } from "../ts/mod.ts";

const targetUrl = "https://quickpizza.grafana.com/api/ratings";
const authHeaderValue = "Token abcdef0123456789";

interface CheckSummary {
  total: number;
  passed: number;
  failed: number;
  failures: string[];
}

function createCheckSummary(): CheckSummary {
  return { total: 0, passed: 0, failed: 0, failures: [] };
}

async function describe(
  name: string,
  fn: () => Promise<void> | void,
): Promise<void> {
  console.log(`describe: ${name}`);
  await fn();
}

function check<T>(
  value: T,
  predicates: Record<string, (v: T) => boolean>,
  summary: CheckSummary,
): boolean {
  let ok = true;
  for (const [label, predicate] of Object.entries(predicates)) {
    let passed = false;
    try {
      passed = Boolean(predicate(value));
    } catch {
      passed = false;
    }

    summary.total += 1;
    if (passed) {
      summary.passed += 1;
    } else {
      summary.failed += 1;
      summary.failures.push(label);
      ok = false;
    }
  }
  return ok;
}

async function runIntegrationChecks(): Promise<CheckSummary> {
  const summary = createCheckSummary();

  await describe("Hello world!", async () => {
    const response = await fetch(targetUrl, {
      method: "GET",
      headers: {
        authorization: authHeaderValue,
      },
    });

    check(
      response,
      {
        "response status is 200": (r) => r.status === 200,
      },
      summary,
    );

    const bodyText = await response.text();
    let parsed: unknown = null;

    const jsonOk = check(
      { bodyText },
      {
        "response has valid json body": ({ bodyText }) => {
          try {
            parsed = JSON.parse(bodyText);
            return true;
          } catch {
            return false;
          }
        },
      },
      summary,
    );

    if (jsonOk) {
      check(
        parsed,
        {
          "ratings list is an array": (value) =>
            typeof value === "object" &&
            value !== null &&
            Array.isArray((value as Record<string, unknown>).ratings),
        },
        summary,
      );
    }
  });

  if (summary.failed > 0) {
    throw new Error(
      `integration checks failed (${summary.failed}/${summary.total}): ${
        summary.failures.join(", ")
      }`,
    );
  }

  return summary;
}

const bench = new LoadgenFFI({
  url: targetUrl,
  protocol: "h2",
  method: "GET",
  headers: [
    { name: "authorization", value: authHeaderValue },
  ],
  clients: 1,
  threads: 1,
  max_streams: 1,
  requests: 1,
  insecure: false,
});

try {
  const integrationSummary = await runIntegrationChecks();
  console.log(
    `integration checks: ${integrationSummary.passed}/${integrationSummary.total} passed`,
  );

  const report = await bench.run() as RunReport;
  printK6LikeSummary(report, {
    scriptPath: "examples/basic-integration-test.ts",
    expectedProtocol: "h2",
  });
} finally {
  bench.close();
}
