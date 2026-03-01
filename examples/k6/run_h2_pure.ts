import http from "k6/http";
import { Counter } from "k6/metrics";

const TARGET_URL = __ENV.TARGET_URL ?? "https://bench.local:8082/?s=256k";

const benchReqs = new Counter("bench_reqs");

export const options = {
  vus: Number(__ENV.VUS ?? 4),
  duration: __ENV.DURATION ?? "2s",
  insecureSkipTLSVerify: true,
  summaryTrendStats: ["min", "med", "avg", "p(90)", "p(99)", "max"],
};

export default function () {
  http.get(TARGET_URL, { tags: { name: "bench_h2_pure" } });
  benchReqs.add(1);
}
