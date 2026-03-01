import { LoadgenFFI } from "../ts/mod.ts";

const targetUrl = Deno.args[0] ?? "https://example.com/";

const bench = new LoadgenFFI({
  url: targetUrl,
  protocol: "h1",
  requests: 10,
  clients: 1,
  threads: 1,
  max_streams: 1,
  insecure: false,
});

try {
  const report = await bench.run();
  console.log(
    JSON.stringify(
      {
        url: report.url,
        proto: report.proto,
        completed: report.requests_completed,
        ok: report.ok,
        err_total: report.err_total,
        rps: report.rps,
        latency_p99_us: report.latency_p99_us,
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
