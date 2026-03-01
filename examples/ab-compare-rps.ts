import { LoadgenFFI, printK6LikeSummary } from "../ts/mod.ts";
import type { RunReport } from "../ts/mod.ts";

const targetUrl = Deno.env.get("TARGET_URL") ??
  "https://bench.local:8082/?s=256k";
const durationS = Number(Deno.env.get("DURATION_S") ?? "10");
const clients = Number(Deno.env.get("CLIENTS") ?? "64");
const threads = Number(Deno.env.get("THREADS") ?? "8");
const maxStreams = Number(Deno.env.get("MAX_STREAMS") ?? "1");
const insecure = (Deno.env.get("INSECURE") ?? "true").toLowerCase() !== "false";
const totalRps = Number(Deno.env.get("RPS") ?? "5000");
const rpsPerClient = Number(
  Deno.env.get("RPS_PER_CLIENT") ?? String(totalRps / Math.max(1, clients)),
);

const bench = new LoadgenFFI({
  url: targetUrl,
  protocol: "h2",
  insecure,
  duration_s: durationS,
  clients,
  threads,
  max_streams: maxStreams,
  rps: rpsPerClient,
  requests: 1,
});

try {
  console.log(
    `rps_config: total_target=${totalRps.toFixed(2)} per_client=${
      rpsPerClient.toFixed(4)
    } clients=${clients}`,
  );
  const report = await bench.run() as RunReport;
  printK6LikeSummary(report, {
    scriptPath: "examples/ab-compare-rps.ts",
    expectedProtocol: "h2",
  });
} finally {
  bench.close();
}
