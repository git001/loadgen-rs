# loadgen-ffi Contract (MVP)

ABI version: `1`
Step ABI version: `1`

Exports:

- `u32 loadgen_abi_version(void)`
- `void* loadgen_create(const char* config_json)`
- `char* loadgen_run(void* handle)`
- `char* loadgen_metrics_snapshot(void* handle)`
- `u32 loadgen_step_abi_version(void)`
- `void* loadgen_step_session_create(const char* session_config_json)`
- `char* loadgen_step_execute(void* session_handle, const char* step_request_json)`
- `char* loadgen_step_snapshot(void* session_handle)`
- `void loadgen_step_session_reset(void* session_handle)`
- `void loadgen_step_session_destroy(void* session_handle)`
- `char* loadgen_merge_reports(const char* reports_json)` — distributed merge
- `char* loadgen_last_error(void)`
- `void loadgen_free_string(char* ptr)`
- `void loadgen_destroy(void* handle)`

Rules:

- All returned `char*` values must be freed with `loadgen_free_string`.
- `loadgen_create` returns `NULL` on error; call `loadgen_last_error`.
- `loadgen_run` returns `NULL` on error; call `loadgen_last_error`.
- `loadgen_step_session_create` returns `NULL` on error; call `loadgen_last_error`.
- `loadgen_step_execute` returns a structured step response (`ok` plus optional `error` object).
- `loadgen_destroy(NULL)` and `loadgen_free_string(NULL)` are no-ops.
- `loadgen_metrics_snapshot` currently returns either the last completed report JSON or `{"state":"idle"}`.
- `loadgen_merge_reports` takes a JSON array of `RunReport` objects (with `*_hist_b64` fields) and returns a merged `RunReport` JSON string. Returns `NULL` on error; call `loadgen_last_error`. Set `export_histograms: true` in `BenchConfig` to include histogram fields in reports.
