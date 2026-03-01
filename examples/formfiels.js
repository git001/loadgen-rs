import { LoadgenFFI, printK6LikeSummary } from "../ts/mod.ts";

const formUrl = "https://test.k6.io/my_messages.php";

function extractRedirValue(html) {
  const inputTagMatch = html.match(/<input\b[^>]*\bname=["']redir["'][^>]*>/i);
  if (!inputTagMatch) {
    throw new Error('could not find <input name="redir">');
  }

  const inputTag = inputTagMatch[0];
  const valueMatch = inputTag.match(/\bvalue=["']([^"']*)["']/i);
  if (!valueMatch) {
    throw new Error("could not extract value attribute for redir");
  }

  return valueMatch[1];
}

const res = await fetch(formUrl);
if (!res.ok) {
  throw new Error(`failed to fetch form page: HTTP ${res.status}`);
}

const html = await res.text();
const redirValue = extractRedirValue(html);
console.log(`The value of the hidden field redir is: ${redirValue}`);

const bench = new LoadgenFFI({
  url: `${formUrl}?redir=${encodeURIComponent(redirValue)}`,
  protocol: "h1",
  clients: 1,
  threads: 1,
  max_streams: 1,
  requests: 1,
  insecure: false,
});

try {
  const report = await bench.run();
  printK6LikeSummary(report, {
    scriptPath: "examples/formfiels.js",
    expectedProtocol: "h1",
  });
} finally {
  bench.close();
}

await new Promise((resolve) => setTimeout(resolve, 1000));
