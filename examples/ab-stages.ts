import { LoadgenFFI, printK6LikeSummary } from "../ts/mod.ts";
import type { BenchConfig, RunReport } from "../ts/mod.ts";

interface Stage {
  duration_s: number;
  target: number;
}

interface Segment {
  stageIndex: number;
  segmentIndex: number;
  durationS: number;
  clients: number;
}

const targetUrl = Deno.env.get("TARGET_URL") ??
  "https://bench.local:8082/?s=256k";
const insecure = (Deno.env.get("INSECURE") ?? "true").toLowerCase() !== "false";
const maxStreams = Number(Deno.env.get("MAX_STREAMS") ?? "1");
const threadsCap = Number(Deno.env.get("THREADS_CAP") ?? "8");
const stepS = Number(Deno.env.get("STAGE_STEP_S") ?? "5");
const initialClients = Number(Deno.env.get("INITIAL_CLIENTS") ?? "1");
const rpsTotal = Deno.env.get("RPS") ? Number(Deno.env.get("RPS")) : undefined;

const defaultStages: Stage[] = [
  { duration_s: 30, target: 8 },
  { duration_s: 30, target: 8 },
  { duration_s: 20, target: 0 },
];

function parseStagesFromEnv(): Stage[] {
  const raw = Deno.env.get("STAGES_JSON");
  if (!raw) {
    return defaultStages;
  }
  const parsed = JSON.parse(raw) as unknown;
  if (!Array.isArray(parsed) || parsed.length === 0) {
    throw new Error("STAGES_JSON must be a non-empty array");
  }
  return parsed.map((s, idx) => {
    if (typeof s !== "object" || s === null) {
      throw new Error(`stage[${idx}] must be an object`);
    }
    const rec = s as Record<string, unknown>;
    const duration_s = Number(rec.duration_s);
    const target = Number(rec.target);
    if (!Number.isFinite(duration_s) || duration_s <= 0) {
      throw new Error(`stage[${idx}].duration_s must be > 0`);
    }
    if (!Number.isFinite(target) || target < 0) {
      throw new Error(`stage[${idx}].target must be >= 0`);
    }
    return { duration_s, target };
  });
}

function buildSegments(
  stages: Stage[],
  startClients: number,
  stepSeconds: number,
): Segment[] {
  const segments: Segment[] = [];
  let prevTarget = Math.max(0, Math.round(startClients));

  for (let i = 0; i < stages.length; i++) {
    const stage = stages[i];
    const steps = Math.max(1, Math.ceil(stage.duration_s / stepSeconds));
    const segDuration = stage.duration_s / steps;

    for (let s = 0; s < steps; s++) {
      const t = (s + 1) / steps;
      const interpolated = prevTarget + (stage.target - prevTarget) * t;
      const clients = Math.max(0, Math.round(interpolated));
      segments.push({
        stageIndex: i,
        segmentIndex: s,
        durationS: segDuration,
        clients,
      });
    }

    prevTarget = stage.target;
  }

  return segments;
}

function buildBenchConfig(segment: Segment): BenchConfig {
  const clients = segment.clients;
  const threads = Math.max(1, Math.min(threadsCap, clients));
  const rpsPerClient = rpsTotal !== undefined && clients > 0
    ? rpsTotal / clients
    : undefined;

  return {
    url: targetUrl,
    protocol: "h2",
    insecure,
    duration_s: segment.durationS,
    clients,
    threads,
    max_streams: maxStreams,
    rps: rpsPerClient,
    requests: 1,
  };
}

function printSegmentHeader(segment: Segment): void {
  console.log(
    `\n=== stage ${segment.stageIndex + 1} segment ${
      segment.segmentIndex + 1
    } ===`,
  );
  console.log(
    `duration=${
      segment.durationS.toFixed(2)
    }s clients=${segment.clients} max_streams=${maxStreams}`,
  );
}

async function runSegment(segment: Segment): Promise<RunReport | null> {
  if (segment.clients <= 0) {
    console.log(
      `\n=== stage ${segment.stageIndex + 1} segment ${
        segment.segmentIndex + 1
      } ===`,
    );
    console.log(
      `duration=${
        segment.durationS.toFixed(2)
      }s clients=0 -> idle segment (no requests)`,
    );
    await new Promise((resolve) =>
      setTimeout(resolve, segment.durationS * 1000)
    );
    return null;
  }

  const config = buildBenchConfig(segment);
  const bench = new LoadgenFFI(config);
  try {
    printSegmentHeader(segment);
    if (config.rps !== undefined) {
      const total = config.rps * segment.clients;
      console.log(
        `rps_target_total=${total.toFixed(2)} rps_per_client=${
          config.rps.toFixed(4)
        }`,
      );
    }
    const report = await bench.run() as RunReport;
    printK6LikeSummary(report, {
      scriptPath: "examples/ab-stages.ts",
      expectedProtocol: "h2",
    });
    return report;
  } finally {
    bench.close();
  }
}

function summarizeTotals(reports: RunReport[]): void {
  const totalCompleted = reports.reduce(
    (acc, r) => acc + r.requests_completed,
    0,
  );
  const totalStarted = reports.reduce((acc, r) => acc + r.requests_started, 0);
  const totalErrors = reports.reduce((acc, r) => acc + r.err_total, 0);
  const totalElapsed = reports.reduce((acc, r) => acc + r.elapsed_s, 0);
  const avgRps = totalElapsed > 0 ? totalCompleted / totalElapsed : 0;

  console.log("\n=== staged totals ===");
  console.log(`segments (with traffic): ${reports.length}`);
  console.log(`elapsed_s: ${totalElapsed.toFixed(3)}`);
  console.log(`requests_started: ${totalStarted}`);
  console.log(`requests_completed: ${totalCompleted}`);
  console.log(`err_total: ${totalErrors}`);
  console.log(`avg_rps: ${avgRps.toFixed(2)}`);
}

const stages = parseStagesFromEnv();
const segments = buildSegments(stages, initialClients, Math.max(0.5, stepS));

console.log("stages:");
console.log(JSON.stringify(stages, null, 2));
console.log(`segments_total=${segments.length} step_s=${Math.max(0.5, stepS)}`);

const reports: RunReport[] = [];
for (const segment of segments) {
  const report = await runSegment(segment);
  if (report) {
    reports.push(report);
  }
}

summarizeTotals(reports);
