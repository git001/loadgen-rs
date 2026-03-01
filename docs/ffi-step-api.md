# FFI Step API Design (Native Correlation Path)

## Goal

Enable k6-like correlation and dynamic data with native loadgen drivers by adding a step-level FFI API.

## ABI Versioning

- Keep existing batch API (`loadgen_create/run/...`) untouched.
- Introduce `loadgen_step_abi_version()` and separate step-session handles.
- Start with step ABI version `1`.

## Proposed C ABI

```c
uint32_t loadgen_step_abi_version(void);

void* loadgen_step_session_create(const char* session_config_json);

char* loadgen_step_execute(void* session_handle, const char* step_request_json);

char* loadgen_step_snapshot(void* session_handle);

void  loadgen_step_session_reset(void* session_handle);

void  loadgen_step_session_destroy(void* session_handle);

char* loadgen_last_error(void);
void  loadgen_free_string(char* ptr);
```

## Session Config JSON

```json
{
  "protocol": "h2",
  "connect_timeout_s": 10,
  "request_timeout_s": 30,
  "insecure": false,
  "tls_ca": "/path/to/ca.crt",
  "cookie_jar": true,
  "redirect_policy": "follow",
  "response_body_limit": 65536,
  "response_headers": true
}
```

## Step Request JSON

```json
{
  "name": "login",
  "method": "POST",
  "url": "https://example.test/login",
  "headers": {
    "content-type": "application/json"
  },
  "body": "{\"username\":\"u\",\"password\":\"p\"}",
  "redirect_policy": "manual",
  "capture_body": true
}
```

## Step Response JSON

```json
{
  "ok": true,
  "status": 200,
  "url_final": "https://example.test/login",
  "http_version": "h2",
  "latency_us": 842,
  "ttfb_us": 301,
  "bytes_in": 512,
  "bytes_out": 128,
  "headers": {
    "content-type": "application/json"
  },
  "body": "{\"token\":\"abc\"}",
  "body_truncated": false,
  "redirect_count": 0
}
```

## Behavioral Requirements

1. Per-session cookie jar with RFC-aware domain/path/expiry handling.
2. Per-step redirect policy (`follow|manual|error`) with max redirect cap.
3. Optional body capture (`capture_body`) with size limit to control overhead.
4. Deterministic error shape:
   - transport/TLS/timeout/protocol categories
   - stable code + message
5. Session reuse of native connections when possible for realistic sequence performance.

## TS Integration Plan

1. Add `LoadgenStepFFI` wrapper in `ts/`.
2. Script engine uses `LoadgenStepFFI` instead of `fetch` when `mode: "native-step"`.
3. Keep current `fetch` script mode as fallback.

## Incremental Rollout

1. v1: H1/H2 native-step + status/headers/body capture.
2. v2: H3 native-step (implemented).
3. v3: stream bodies / partial reads for large payload workflows.
