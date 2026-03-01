import { LoadgenFFI } from "../ts/mod.ts";

const targetUrl = "https://quickpizza.grafana.com/api/users/token/login";
const payload = JSON.stringify({
  username: "default",
  password: "12345678",
});
const requestHeaders = {
  "content-type": "application/json",
};

const bench = new LoadgenFFI({
  url: targetUrl,
  protocol: "h2",
  method: "POST",
  headers: [
    { name: "content-type", value: "application/json" },
  ],
  body: payload,
  duration_s: 2,
  clients: 4,
  threads: 2,
  max_streams: 1,
  requests: 1,
  insecure: false,
});

try {
  const singleResponse = await fetch(targetUrl, {
    method: "POST",
    headers: requestHeaders,
    body: payload,
  });
  const singleResponseBody = await singleResponse.text();
  console.log(`response_status: ${singleResponse.status}`);
  console.log("response_body:");
  console.log(singleResponseBody);

  const report = await bench.run();
  console.log(
    JSON.stringify(
      {
        target: report.url,
        proto: report.proto,
        mode: report.mode,
        completed: report.requests_completed,
        ok: report.ok,
        err_total: report.err_total,
        rps: report.rps,
        p99_us: report.latency_p99_us,
      },
      null,
      2,
    ),
  );
} catch (error) {
  console.error(`benchmark failed: ${error instanceof Error ? error.message : String(error)}`);
  Deno.exit(1);
} finally {
  bench.close();
}
