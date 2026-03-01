import { LoadgenFFI, printK6LikeSummary } from "../ts/mod.ts";
import type { RunReport } from "../ts/mod.ts";

const targetUrl = Deno.env.get("TARGET_URL") ??
  "https://bench.local:8082/?s=256k";
const vus = Number(Deno.env.get("VUS") ?? "4");
const durationS = Number(Deno.env.get("DURATION_S") ?? "10");
const threads = Number(Deno.env.get("THREADS") ?? String(vus));
const maxStreams = Number(Deno.env.get("MAX_STREAMS") ?? "1");
const insecure = (Deno.env.get("INSECURE") ?? "true").toLowerCase() !== "false";

const bench = new LoadgenFFI({
  url: targetUrl,
  protocol: "h2",
  insecure,
  duration_s: durationS,
  clients: vus,
  threads,
  max_streams: maxStreams,
  requests: 1,
});

try {
  const report = await bench.run() as RunReport;
  printK6LikeSummary(report, {
    scriptPath: "examples/ab-compare.ts",
    expectedProtocol: "h2",
  });
} finally {
  bench.close();
}
