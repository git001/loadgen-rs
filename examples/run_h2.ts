// run_h2.ts
import { LoadgenFFI, printK6LikeSummary } from "../ts/mod.ts";
import { DOMParser } from "jsr:@b-fuze/deno-dom";
import type { RunReport } from "../ts/types.ts";

const BLOG_URL = Deno.env.get("BLOG_URL") ?? "https://example.com/";
const TARGET_URL = "https://bench.local:8082/?s=256k";
const VUS = 4;
const THREADS = 2;
const MAX_STREAMS = 1;
const DURATION_S = 2;

async function collectAnchorClassLinkHrefs(pageUrl: string): Promise<string[]> {
  const res = await fetch(pageUrl);
  if (!res.ok) {
    throw new Error(`failed to fetch ${pageUrl}: HTTP ${res.status}`);
  }

  const html = await res.text();
  const doc = new DOMParser().parseFromString(html, "text/html");
  if (!doc) {
    throw new Error(`failed to parse HTML from ${pageUrl}`);
  }

  const anchors = doc.querySelectorAll("a.link");
  const hrefs = new Set<string>();

  for (const anchor of anchors) {
    const href = anchor.getAttribute("href");
    if (!href) {
      continue;
    }

    try {
      hrefs.add(new URL(href, pageUrl).toString());
    } catch {
      // skip invalid href values
    }
  }

  return [...hrefs];
}

const bench = new LoadgenFFI({
  url: TARGET_URL,
  protocol: "h2",
  insecure: true, // entspricht --insecure
  // tls_ca: "/path/to/ca.crt", // optional: custom CA certificate
  duration_s: DURATION_S, // entspricht --duration 2s
  clients: VUS, // -c 4
  threads: THREADS, // -t 2
  max_streams: MAX_STREAMS, // -m 1
  requests: 1, // wird in duration mode ignoriert
});

try {
  const links = await collectAnchorClassLinkHrefs(BLOG_URL);
  const report = await bench.run() as RunReport;
  printK6LikeSummary(report, {
    scriptPath: "examples/run_h2.ts",
    expectedProtocol: "h2",
  });
  //console.log(`\n<a class=\"link\"> hrefs (${links.length}):`);
  //console.log(JSON.stringify(links, null, 2));
} finally {
  bench.close();
}
