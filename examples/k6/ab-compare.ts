import http from "k6/http";
import { check } from "k6";
import { Counter } from "k6/metrics";

const TARGET_URL = __ENV.TARGET_URL ?? "https://bench.local:8082/?s=256k";
const VUS = Number(__ENV.VUS ?? 4);
const DURATION = __ENV.DURATION ?? "10s";
const INSECURE = (__ENV.INSECURE ?? "true").toLowerCase() !== "false";

const benchReqs = new Counter("bench_reqs");

export const options = {
  scenarios: {
    default: {
      executor: "constant-vus",
      vus: VUS,
      duration: DURATION,
      gracefulStop: "0s",
    },
  },
  insecureSkipTLSVerify: INSECURE,
  summaryTrendStats: ["min", "med", "avg", "p(90)", "p(99)", "max"],
};

export default function () {
  const res = http.get(TARGET_URL, { tags: { name: "ab_compare" } });
  benchReqs.add(1);
  check(res, {
    "status is 200": (r) => r.status === 200,
    "protocol is HTTP/2": (r) => r.proto === "HTTP/2.0",
  });
}
