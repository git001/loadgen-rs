import http from "k6/http";
import { check } from "k6";
import { parseHTML } from "k6/html";
import { Counter } from "k6/metrics";

const TARGET_URL = __ENV.TARGET_URL ?? "https://bench.local:8082/?s=256k";
const BLOG_URL = __ENV.BLOG_URL ?? "https://example.com/";
const benchReqs = new Counter("bench_reqs");

export const options = {
  vus: Number(__ENV.VUS ?? 4),
  duration: __ENV.DURATION ?? "2s",
  insecureSkipTLSVerify: true,
  summaryTrendStats: ["min", "med", "avg", "p(90)", "p(99)", "max"],
};

export function setup() {
  const res = http.get(BLOG_URL, { tags: { name: "blog_scan" } });
  check(res, {
    "blog page status is 200": (r) => r.status === 200,
  });

  const doc = parseHTML(res.body);
  const links = doc
    .find("a.link")
    .toArray()
    .map((el) => el.attr("href"))
    .filter((href) => typeof href === "string" && href.length > 0)
    .map((href) => {
      try {
        return new URL(href, BLOG_URL).toString();
      } catch {
        return "";
      }
    })
    .filter((href) => href.length > 0);

  return { blogLinks: [...new Set(links)] };
}

export default function () {
  const res = http.get(TARGET_URL, { tags: { name: "bench_h2" } });
  benchReqs.add(1);
  check(res, {
    "status 200": (r) => r.status === 200,
    "http2 negotiated": (r) => r.proto === "HTTP/2.0",
  });
}

// export function teardown(data) {
//   console.log(`\n<a class="link"> hrefs (${data.blogLinks.length}):`);
//   console.log(JSON.stringify(data.blogLinks, null, 2));
// }
