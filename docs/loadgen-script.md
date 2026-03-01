# Script Mode Reference

loadgen-rs includes a k6-inspired script scenario engine for multi-step HTTP
workflows with checks, extractors, cookies, and template substitution. Scenarios
run locally or distributed across remote workers.

## Quick Start

```typescript
import { runScriptScenario } from "./ts/script_mode.ts";

const result = await runScriptScenario({
  vus: 2,
  duration_s: 10,
  steps: [
    {
      name: "homepage",
      url: "https://example.com/",
      checks: { status_in: [200] },
    },
  ],
});

console.log(`${result.iterations} iterations, ${result.checks_passed}/${result.checks_total} checks passed`);
```

Run locally:

```bash
deno run --allow-net my-scenario.ts
```

Run on workers (distributed):

```bash
# POST the config to each worker's /run-script endpoint
deno run --allow-net --allow-env examples/html-check.ts \
  http://worker1:9091 http://worker2:9091
```

## Scenario Configuration

```typescript
interface ScriptScenarioConfig {
  vus: number;                          // Virtual users (concurrent)
  duration_s: number;                   // How long to run (seconds)
  steps: ScriptStep[];                  // Ordered list of HTTP steps
  execution_mode?: "fetch" | "ffi-step"; // Transport (default: "fetch")
  use_cookies?: boolean;                // Enable cookies (default: true)
  redirect_policy?: "follow" | "error" | "manual"; // Default: "follow"
  request_timeout_s?: number;           // Per-step timeout
  continue_on_error?: boolean;          // Keep going after step failure
  step_session_config?: StepSessionConfig; // Native transport settings (ffi-step only)
}
```

Each VU runs the full step sequence in a loop until `duration_s` expires.
Iterations are counted per complete pass through all steps.

### Execution Modes

| Mode | Transport | H3 Support | Cookie Jar | Requires |
|------|-----------|------------|------------|----------|
| `fetch` | Deno `fetch()` | No | TypeScript (RFC 6265) | `--allow-net` |
| `ffi-step` | Rust reqwest via FFI | Yes | Native reqwest | `--allow-ffi --allow-net` |

For `ffi-step`, configure the native session:

```typescript
step_session_config: {
  protocol: "h2",           // "h1", "h2", or "h3"
  connect_timeout_s: 10,
  request_timeout_s: 30,
  insecure: false,           // Skip TLS verification
  cookie_jar: true,          // Native cookie management
  redirect_policy: "follow",
  response_headers: true,    // Capture response headers
  response_body_limit: 65536, // Max body bytes to capture
}
```

**When to use which:**

- **`fetch`** — Prototyping, debugging, or when you don't need the Rust library.
  Simpler setup (no `.so` to build), easier stack traces, works out of the box.
  Only supports H1/H2.
- **`ffi-step`** — Production runs and distributed workers. Better per-request
  performance (lower overhead than Deno fetch), native TLS control, H3/QUIC
  support, and connection pooling across steps via reqwest. Workers use this
  mode automatically.

In practice: start with `fetch` locally, switch to `ffi-step` for production
and distributed runs.

See [script-mode-ffi.md](script-mode-ffi.md) for detailed mode comparison.

## Steps

Each step is one HTTP request. Steps execute sequentially within a VU.

```typescript
interface ScriptStep {
  name: string;                    // Step identifier (used in stats/errors)
  url: string;                     // Target URL (supports {{templates}})
  method?: string;                 // HTTP method (default: GET, or POST if body is set)
  headers?: Record<string, string>; // Request headers (support {{templates}})
  body?: string;                   // Request body (supports {{templates}})
  checks?: ScriptStepChecks;       // Assertions on the response
  extract?: ScriptExtractor[];     // Extract values for later steps
  capture_body?: boolean;          // Force body capture (auto-enabled when needed)
  use_cookies?: boolean;           // Override scenario cookie setting for this step
  redirect_policy?: "follow" | "error" | "manual"; // Override scenario default
}
```

**Method defaults:** If `body` is set and no `method` is specified, the method
defaults to `POST`. Otherwise it defaults to `GET`.

**Body capture:** Automatically enabled when any check or extractor needs the
response body (`body_includes`, `json_path_*`, `regex_match`, `json`/`regex`/`dom`
extractors). Set `capture_body: true` to force capture even without checks.

## Template Engine

Use `{{variable}}` in URLs, headers, request bodies, and extractor patterns.
Variables are populated by extractors from previous steps and stored per-VU.

```typescript
steps: [
  {
    name: "login",
    url: "https://api.example.com/login",
    method: "POST",
    body: '{"user":"admin","pass":"secret"}',
    extract: [{ type: "json", path: "token", as: "auth_token" }],
  },
  {
    name: "profile",
    url: "https://api.example.com/me",
    headers: { "authorization": "Bearer {{auth_token}}" },
    checks: { status_in: [200] },
  },
]
```

Template rules:
- Pattern: `{{name}}` (whitespace tolerant: `{{ name }}` also works)
- Variable names: `[a-zA-Z0-9_.-]+`
- Throws an error if a referenced variable is not defined
- All extracted values are strings

## Extractors

Extractors capture values from HTTP responses and store them in per-VU state
for use in subsequent steps via `{{variable}}` templates.

### json

Extract from JSON response bodies using dot-notation paths.

```typescript
{ type: "json", path: "data.user.id", as: "user_id" }
{ type: "json", path: "items.0.name", as: "first_item" }
```

- Supports array indices: `items.0.name`
- Throws if path not found or value is null
- Response body is parsed lazily (once per step, even with multiple json extractors)

### header

Extract from response headers (case-insensitive lookup).

```typescript
{ type: "header", name: "x-request-id", as: "req_id" }
{ type: "header", name: "location", as: "redirect_url" }
```

- Does not require body capture
- Throws if header is missing

### regex

Extract using a regular expression with capture groups.

```typescript
{ type: "regex", pattern: 'csrf_token="([^"]+)"', as: "csrf" }
{ type: "regex", pattern: "session=([a-f0-9]+)", group: 1, flags: "i", as: "session" }
```

| Field | Default | Description |
|-------|---------|-------------|
| `pattern` | required | ECMAScript regex |
| `group` | `1` | Capture group number (0 = entire match) |
| `flags` | `""` | Regex flags: `i`, `g`, `m`, etc. |

- Pattern supports `{{templates}}`
- Throws if no match or group not found

### dom

Extract from HTML using CSS selectors via [deno-dom](https://jsr.io/@b-fuze/deno-dom).

```typescript
{ type: "dom", selector: 'input[name="_token"]', attribute: "value", as: "csrf_token" }
{ type: "dom", selector: "h1.title", as: "page_title" }
{ type: "dom", selector: "a.logout", attribute: "href", as: "logout_url" }
```

| Field | Default | Description |
|-------|---------|-------------|
| `selector` | required | CSS selector (`querySelector` syntax) |
| `attribute` | _(none)_ | Attribute to extract. If omitted, extracts `textContent` |

- Selector supports `{{templates}}`
- DOM is parsed lazily (once per step, even with multiple dom extractors)
- Throws if no element matches or attribute/textContent is empty
- More robust than regex for structured HTML

### Comparison

| Type | Best For | Needs Body | Example Use Case |
|------|----------|------------|------------------|
| `json` | API responses | Yes | Extract JWT token from login response |
| `header` | Response metadata | No | Capture `Location` redirect URL |
| `regex` | Unstructured text, JS variables | Yes | Extract token from inline JavaScript |
| `dom` | HTML forms, links, elements | Yes | Read CSRF token from hidden input field |

## Checks

Checks assert conditions on the response. Failed checks are counted but do not
stop execution unless the step throws (e.g., extractor failure).

### status_in

```typescript
checks: { status_in: [200, 201] }
```

Response status code must be one of the listed values.

### body_includes

```typescript
checks: { body_includes: ["Welcome", "logged in"] }
```

Each string must appear as a substring in the response body. Supports `{{templates}}`.

### header_exists

```typescript
checks: { header_exists: ["set-cookie", "x-request-id"] }
```

Each header must be present in the response (case-insensitive).

### header_equals

```typescript
checks: { header_equals: { "content-type": "application/json" } }
```

Header value must match exactly (case-insensitive header name, exact value match).

### header_includes

```typescript
checks: { header_includes: { "set-cookie": "__cf_bm=" } }
```

Header value must contain the given substring. Useful for `Set-Cookie` headers
where the full value includes expiry, path, and other attributes.

### json_path_exists

```typescript
checks: { json_path_exists: ["data.user", "data.items.0"] }
```

The JSON path must resolve to a non-null value. Uses the same dot-notation as
the `json` extractor.

### json_path_equals

```typescript
checks: { json_path_equals: { "data.status": "active", "data.count": 42 } }
```

The value at the JSON path must match. Type-aware comparison: strings, numbers,
booleans, and null are all supported.

### regex_match

```typescript
checks: { regex_match: ["session_id=[a-f0-9]{32}", "csrf_token="] }
```

Each pattern must match somewhere in the response body. Supports `{{templates}}`.

## Cookie Handling

Cookies persist across steps within a VU iteration, enabling session-based
workflows like login/logout.

**Scenario level:** `use_cookies: true` (default) enables cookie management.

**Per-step override:** Set `use_cookies: false` on individual steps to skip
cookie send/receive for that request.

**fetch mode:** A TypeScript `CookieJar` handles RFC 6265 semantics — domain
matching, path scoping, Secure flag, `Max-Age`/`Expires` parsing (including
RFC 1123 dates via the Temporal API).

**ffi-step mode:** The native Rust reqwest cookie jar manages cookies when
`step_session_config.cookie_jar: true`. Cookies are handled entirely on the
Rust side for these requests.

**Iteration boundaries:** The cookie jar persists across iterations within the
same VU. If your scenario logs out (clearing the session server-side), the
server will reject the stale cookie on the next iteration's first request,
causing the login flow to work naturally.

## Redirect Policy

Three modes, configurable at scenario level and per-step:

| Policy | Behavior |
|--------|----------|
| `follow` (default) | Automatically follow redirects |
| `error` | Throw an error on redirect |
| `manual` | Return the redirect response as-is |

The response includes `redirect_count` and `url_final` (the final URL after
following redirects).

## Results

`runScriptScenario()` returns a `ScriptRunResult`:

```typescript
interface ScriptRunResult {
  vus: number;                    // VUs configured
  duration_target_s: number;      // Requested duration
  elapsed_s: number;              // Actual elapsed time
  started_at: string;             // ISO 8601 start timestamp
  finished_at: string;            // ISO 8601 end timestamp
  iterations: number;             // Complete iteration count
  steps_executed: number;         // Total step executions
  iteration_rate: number;         // iterations / elapsed_s
  step_rate: number;              // steps_executed / elapsed_s
  checks_total: number;           // Total check evaluations
  checks_passed: number;
  checks_failed: number;
  errors_total: number;           // Step execution errors
  step_stats: Record<string, number>;  // Per-step execution counts
  step_errors: Record<string, number>; // Per-step error counts
  check_summary: Record<string, {     // Per-check-type tallies
    total: number;
    passed: number;
    failed: number;
  }>;
}
```

Example access:

```typescript
const result = await runScriptScenario(config);

// Overall
console.log(`${result.iterations} iterations in ${result.elapsed_s}s`);
console.log(`${result.checks_passed}/${result.checks_total} checks passed`);

// Per check type
const bodyCheck = result.check_summary["body_includes"];
console.log(`body_includes: ${bodyCheck.passed}/${bodyCheck.total}`);

// Per step errors
for (const [step, count] of Object.entries(result.step_errors)) {
  if (count > 0) console.log(`${step}: ${count} errors`);
}
```

## Examples

| Example | What it demonstrates |
|---------|----------------------|
| `examples/correlation_mvp.ts` | Login → extract JWT → use in follow-up request (fetch + ffi-step) |
| `examples/cookie_redirect_local_mvp.ts` | Cookie jar and redirect policy testing |
| `examples/html-check.ts` | Distributed: HTML content + cookie verification across workers |
| `examples/roundcube-login.ts` | Distributed: multi-step login/logout with DOM extractor, form POST, session cookies |
| `examples/native_step_skeleton.ts` | Minimal `LoadgenStepFFI` usage template |

### Roundcube login flow (multi-step with state)

This example chains three dependent steps with extractors passing state:

```
Step 1: GET /mail/
  ├─ check: title contains "Welcome to Roundcube Webmail"
  └─ extract (dom): input[name="_token"] → {{token}}

Step 2: POST /mail/?_task=login
  ├─ body: _task=login&_token={{token}}&_user=...&_pass=...
  ├─ check: title contains "Inbox", body contains empty mailbox indicator
  └─ extract (regex): "request_token":"..." → {{logout_token}}

Step 3: GET /mail/?_task=logout&_token={{logout_token}}
  └─ check: title contains "Welcome to Roundcube Webmail" (back to login)
```

Cookies flow automatically between steps. After logout, the session is
invalidated server-side and the next iteration starts a fresh login.
