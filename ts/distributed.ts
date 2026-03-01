/**
 * Distributed benchmark controller.
 *
 * Distributes a BenchConfig across multiple worker-agents, coordinates
 * a synchronized start, collects RunReports, and merges them via FFI
 * for statistically correct histogram aggregation.
 */

import { mergeReports } from "./merge_ffi.ts";
import type {
  BenchConfig,
  DistributedConfig,
  DistributedResult,
  RunReport,
  WorkerError,
  WorkerHealthResponse,
  WorkerRunRequest,
} from "./types.ts";

function splitEvenly(total: number, parts: number): number[] {
  const base = Math.floor(total / parts);
  const remainder = total % parts;
  const result: number[] = [];
  for (let i = 0; i < parts; i++) {
    result.push(base + (i < remainder ? 1 : 0));
  }
  return result;
}

export class Controller {
  private readonly workers: string[];
  private readonly config: BenchConfig;
  private readonly startDelayMs: number;
  private readonly timeoutMs: number;
  private readonly libraryPath: string;

  constructor(dc: DistributedConfig, libraryPath?: string) {
    if (dc.workers.length === 0) {
      throw new Error("at least one worker URL is required");
    }
    this.workers = dc.workers.map((w) => w.replace(/\/$/, ""));
    this.config = dc.config;
    this.startDelayMs = dc.start_delay_ms ?? 500;
    this.timeoutMs = dc.timeout_ms ?? 600_000;
    this.libraryPath = libraryPath ?? "./target/release/libloadgen_ffi.so";
  }

  /** Health-check all workers. Returns array of results (null = unreachable). */
  async healthCheck(): Promise<(WorkerHealthResponse | null)[]> {
    return Promise.all(
      this.workers.map(async (url) => {
        try {
          const resp = await fetch(`${url}/health`, {
            signal: AbortSignal.timeout(5000),
          });
          if (!resp.ok) return null;
          return (await resp.json()) as WorkerHealthResponse;
        } catch {
          return null;
        }
      }),
    );
  }

  /** Run the distributed benchmark. */
  async run(): Promise<DistributedResult> {
    const workerCount = this.workers.length;

    // Health check
    const health = await this.healthCheck();
    const unhealthy = health
      .map((h, i) => (h === null ? this.workers[i] : null))
      .filter((x) => x !== null);
    if (unhealthy.length > 0) {
      throw new Error(
        `unreachable workers: ${unhealthy.join(", ")}`,
      );
    }

    // Split clients evenly
    const totalClients = this.config.clients ?? 1;
    const clientSplits = splitEvenly(totalClients, workerCount);

    // Split request count if in count mode
    const totalRequests = this.config.requests ?? 1;
    const isCountMode = this.config.duration_s === undefined || this.config.duration_s === null;
    const requestSplits = isCountMode
      ? splitEvenly(totalRequests, workerCount)
      : new Array(workerCount).fill(totalRequests);

    // Coordinated start
    const startAt = Temporal.Now.instant().add({ milliseconds: this.startDelayMs }).toString();

    // Build per-worker configs
    const requests: Promise<{ report?: RunReport; error?: string }>[] =
      this.workers.map(async (workerUrl, i) => {
        const workerConfig: BenchConfig = {
          ...this.config,
          clients: clientSplits[i],
          requests: requestSplits[i],
          threads: Math.max(1, Math.min(clientSplits[i], this.config.threads ?? 1)),
          export_histograms: true,
        };

        // Skip workers with 0 clients
        if (workerConfig.clients === 0) {
          return { error: `worker ${workerUrl}: 0 clients assigned, skipped` };
        }

        const body: WorkerRunRequest = {
          config: workerConfig,
          start_at: startAt,
        };

        try {
          const resp = await fetch(`${workerUrl}/run`, {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify(body),
            signal: AbortSignal.timeout(this.timeoutMs),
          });

          if (!resp.ok) {
            const text = await resp.text();
            return { error: `worker ${workerUrl}: HTTP ${resp.status} — ${text}` };
          }

          const report = (await resp.json()) as RunReport;
          return { report };
        } catch (e) {
          const msg = e instanceof Error ? e.message : String(e);
          return { error: `worker ${workerUrl}: ${msg}` };
        }
      });

    const results = await Promise.all(requests);

    const workerReports: RunReport[] = [];
    const workerErrors: WorkerError[] = [];

    for (let i = 0; i < results.length; i++) {
      const r = results[i];
      if (r.report) {
        workerReports.push(r.report);
      }
      if (r.error) {
        workerErrors.push({ worker_url: this.workers[i], error: r.error });
      }
    }

    if (workerReports.length === 0) {
      throw new Error(
        `all workers failed:\n${workerErrors.map((e) => `  ${e.worker_url}: ${e.error}`).join("\n")}`,
      );
    }

    // Merge via FFI (statistically correct histogram merge)
    const mergedReport = mergeReports(workerReports, this.libraryPath);

    return {
      merged_report: mergedReport,
      worker_reports: workerReports,
      worker_errors: workerErrors,
    };
  }
}
