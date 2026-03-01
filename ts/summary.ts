import type { RunReport } from "./types.ts";

export interface K6LikeSummaryOptions {
  scriptPath?: string;
  expectedProtocol?: string;
  statusCheckLabel?: string;
  statusChecksPassed?: number;
  statusChecksTotal?: number;
}

export function formatRatePerSec(value: number): string {
  if (value >= 1_000_000_000) {
    return `${(value / 1_000_000_000).toFixed(2)} GB/s`;
  }
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(2)} MB/s`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(2)} KB/s`;
  return `${value.toFixed(2)} B/s`;
}

export function formatPerSec(value: number): string {
  return `${value.toFixed(2)}/s`;
}

export function formatMsFromUs(us: number): string {
  return `${(us / 1000).toFixed(3)}ms`;
}

export function formatPct(numerator: number, denominator: number): string {
  if (denominator <= 0) return "0.00%";
  return `${((numerator / denominator) * 100).toFixed(2)}%`;
}

export function printK6LikeSummary(
  report: RunReport,
  options: K6LikeSummaryOptions = {},
): void {
  const elapsed = report.elapsed_s;
  const reqRate = elapsed > 0 ? report.requests_completed / elapsed : 0;
  const startedRate = elapsed > 0 ? report.requests_started / elapsed : 0;
  const dataRecvRate = elapsed > 0 ? report.bytes_in / elapsed : 0;
  const dataSendRate = elapsed > 0 ? report.bytes_out / elapsed : 0;

  const statusChecksPassed = options.statusChecksPassed ?? report.ok;
  const statusChecksTotal = options.statusChecksTotal ??
    report.requests_completed;
  const statusCheckLabel = options.statusCheckLabel ?? "status == 200";

  const expectedProtocol = options.expectedProtocol ?? report.proto;
  const protocolChecksPassed = report.proto === expectedProtocol
    ? statusChecksTotal
    : 0;

  const failed = report.err_total;
  const failedPct = statusChecksTotal > 0 ? failed / statusChecksTotal : 0;

  console.log("");
  console.log("Deno+FFI benchmark load test tool summary report");
  console.log("");
  console.log("  execution: local");
  console.log(`     script: ${options.scriptPath ?? "-"}`);
  console.log(`     target: ${report.url}`);
  console.log(`   protocol: ${report.proto}`);
  console.log("");
  console.log("  scenario: default");
  console.log(`      mode: ${report.mode}`);
  console.log(
    `       vus: ${report.clients} (threads=${report.threads}, max_streams=${report.max_streams})`,
  );
  console.log(`  duration: ${elapsed.toFixed(3)}s`);
  console.log("");
  console.log("  checks:");
  console.log(
    `    ${
      statusCheckLabel.padEnd(24, ".")
    } ${statusChecksPassed} / ${statusChecksTotal} (${
      formatPct(statusChecksPassed, statusChecksTotal)
    })`,
  );
  console.log(
    `    protocol == ${expectedProtocol} ${
      "".padEnd(Math.max(0, 10 - expectedProtocol.length), ".")
    } ${protocolChecksPassed} / ${statusChecksTotal} (${
      formatPct(protocolChecksPassed, statusChecksTotal)
    })`,
  );
  console.log("");
  console.log("  metrics:");
  console.log(
    `    http_reqs............... ${report.requests_completed} ${
      formatPerSec(reqRate)
    }`,
  );
  console.log(
    `    http_req_failed......... ${
      (failedPct * 100).toFixed(2)
    }% (${failed}/${statusChecksTotal})`,
  );
  console.log(
    `    started_reqs............ ${report.requests_started} ${
      formatPerSec(startedRate)
    }`,
  );
  console.log(
    `    data_received........... ${report.bytes_in} B ${
      formatRatePerSec(dataRecvRate)
    }`,
  );
  console.log(
    `    data_sent_estimate...... ${report.bytes_out} B ${
      formatRatePerSec(dataSendRate)
    }`,
  );
  console.log(
    `    http_req_duration....... avg=${
      formatMsFromUs(report.latency_mean_us)
    } ` +
      `min=${formatMsFromUs(report.latency_min_us)} med=${
        formatMsFromUs(report.latency_p50_us)
      } ` +
      `p(90)=${formatMsFromUs(report.latency_p90_us)} p(99)=${
        formatMsFromUs(report.latency_p99_us)
      } max=${formatMsFromUs(report.latency_max_us)}`,
  );
}
