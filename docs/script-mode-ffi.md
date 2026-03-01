# Script Mode: fetch vs ffi-step

`runScriptScenario()` supports two execution modes. Both share the same check,
extractor, template, and cookie evaluation logic — only the HTTP transport differs.

For the full script engine reference, see [loadgen-script.md](loadgen-script.md).

## Modes

| Mode | Transport | Cookie Handling | Protocols | Requires |
|------|-----------|-----------------|-----------|----------|
| `fetch` | Deno `fetch()` | TypeScript CookieJar (RFC-aware) | h1, h2 | `--allow-net` |
| `ffi-step` | Rust reqwest via `LoadgenStepFFI` | Native reqwest jar | h1, h2, h3 | `--allow-ffi --allow-net` |

## Config

```ts
await runScriptScenario({
  vus: 2,
  duration_s: 5,
  execution_mode: "ffi-step",
  use_cookies: true,
  redirect_policy: "follow",
  step_session_config: {
    protocol: "h2",
    request_timeout_s: 10,
    cookie_jar: true,
    redirect_policy: "follow",
    response_headers: true,
    response_body_limit: 64 * 1024,
  },
  steps: [/* ... */],
});
```

## Shared Behavior

- All 4 extractor types work in both modes: `json`, `header`, `regex`, `dom`
- All 8 check types work in both modes
- Template substitution (`{{var}}`) works identically
- Body capture is enabled automatically when checks or extractors need it

## ffi-step Specifics

- One native step session is created per VU and closed automatically on completion
- `step_session_config.protocol` supports `h1`, `h2`, and `h3`
- Cookie management is handled by the native reqwest jar (`cookie_jar: true`)
- Connection pooling and TLS session reuse across steps within a VU
- `dom` extractors still run in TypeScript (deno-dom parses the captured body)

## fetch Specifics

- Uses Deno's built-in `fetch()` with `AbortSignal.timeout`
- Cookie management via the TypeScript `CookieJar` class (RFC 6265 domain/path/expiry matching)
- Only supports h1 and h2 (no h3)
- Redirect handling via fetch's native `redirect` option

## Example

`examples/correlation_mvp.ts` supports env switch:

- `EXEC_MODE=fetch` (default)
- `EXEC_MODE=ffi-step`

```bash
deno run --allow-net=quickpizza.grafana.com --allow-env \
  examples/correlation_mvp.ts

EXEC_MODE=ffi-step deno run --allow-ffi --allow-net=quickpizza.grafana.com --allow-env \
  examples/correlation_mvp.ts
```
