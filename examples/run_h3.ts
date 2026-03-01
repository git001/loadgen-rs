// run_h3.ts
import { LoadgenFFI } from "../ts/mod.ts";

const bench = new LoadgenFFI({
  url: "https://bench.local:8082/?s=256k",
  protocol: "h3",
  insecure: true, // entspricht --insecure
  tls_ca: "/datadisk/git-repos/server-benchmark/tls/ca.crt", // entspricht --tls-ca
  duration_s: 2,  // entspricht --duration 2s
  clients: 4,     // -c 4
  threads: 2,     // -t 2
  max_streams: 1, // -m 1
  requests: 1,    // wird in duration mode ignoriert
});

try {
  const report = await bench.run();
  console.log(JSON.stringify(report));
} finally {
  bench.close();
}
