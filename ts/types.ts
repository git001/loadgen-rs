export type BenchProtocol = "h1" | "h2" | "h3";

export interface BenchHeader {
  name: string;
  value: string;
}

export interface BenchConfig {
  url: string;
  protocol?: BenchProtocol;
  method?: string;
  headers?: BenchHeader[];
  body?: string;
  requests?: number;
  duration_s?: number;
  warm_up_time_s?: number;
  ramp_up_time_s?: number;
  clients?: number;
  threads?: number;
  max_streams?: number;
  connect_timeout_s?: number;
  request_timeout_s?: number;
  rps?: number;
  insecure?: boolean;
  tls_ciphers?: string;
  tls_ca?: string;
  tail_friendly?: boolean;
  metrics_sample?: number;
  tcp_quickack?: boolean;
  v4?: boolean;
  v6?: boolean;
  export_histograms?: boolean;
}

export interface RunReport {
  proto: string;
  url: string;
  mode: string;
  clients: number;
  threads: number;
  max_streams: number;
  requests_started: number;
  requests_completed: number;
  ok: number;
  err_total: number;
  rps: number;
  bytes_in: number;
  bytes_out: number;
  latency_min_us: number;
  latency_p50_us: number;
  latency_p90_us: number;
  latency_p99_us: number;
  latency_mean_us: number;
  latency_max_us: number;
  elapsed_s: number;
  [key: string]: unknown;
}

export interface StepSessionConfig {
  protocol?: BenchProtocol;
  connect_timeout_s?: number;
  request_timeout_s?: number;
  insecure?: boolean;
  tls_ca?: string;
  cookie_jar?: boolean;
  redirect_policy?: ScriptRedirectPolicy;
  response_body_limit?: number;
  response_headers?: boolean;
  [key: string]: unknown;
}

export interface StepRequest {
  name: string;
  method?: string;
  url: string;
  headers?: Record<string, string>;
  body?: string;
  redirect_policy?: ScriptRedirectPolicy;
  capture_body?: boolean;
  use_cookies?: boolean;
  [key: string]: unknown;
}

export interface StepError {
  code: string;
  message: string;
  [key: string]: unknown;
}

export interface StepResponse {
  ok: boolean;
  status: number | null;
  url_final: string | null;
  http_version: string | null;
  latency_us: number | null;
  ttfb_us: number | null;
  bytes_in: number;
  bytes_out: number;
  headers: Record<string, string>;
  body: string | null;
  body_truncated: boolean;
  redirect_count: number;
  step_name?: string;
  error?: StepError;
  [key: string]: unknown;
}

export type ScriptRedirectPolicy = "follow" | "error" | "manual";

export interface ScriptStepChecks {
  status_in?: number[];
  body_includes?: string[];
  header_exists?: string[];
  header_equals?: Record<string, string>;
  header_includes?: Record<string, string>;
  json_path_exists?: string[];
  json_path_equals?: Record<string, string | number | boolean | null>;
  regex_match?: string[];
}

export interface ScriptStep {
  name: string;
  method?: string;
  url: string;
  headers?: Record<string, string>;
  body?: string;
  expected_status?: number | number[];
  extract?: ScriptExtractor[];
  checks?: ScriptStepChecks;
  capture_body?: boolean;
  use_cookies?: boolean;
  redirect_policy?: ScriptRedirectPolicy;
}

export type ScriptExtractor =
  | {
      type: "json";
      path: string;
      as: string;
    }
  | {
      type: "header";
      name: string;
      as: string;
    }
  | {
      type: "regex";
      pattern: string;
      group?: number;
      flags?: string;
      as: string;
    }
  | {
      type: "dom";
      selector: string;
      attribute?: string;
      as: string;
    };

export interface ScriptScenarioConfig {
  vus: number;
  duration_s: number;
  request_timeout_s?: number;
  continue_on_error?: boolean;
  steps: ScriptStep[];
  use_cookies?: boolean;
  redirect_policy?: ScriptRedirectPolicy;
  execution_mode?: ScriptExecutionMode;
  step_session_config?: StepSessionConfig;
}

export type ScriptExecutionMode = "fetch" | "ffi-step";

export type ScriptState = Record<string, string>;

export type ScriptStepStats = Record<string, number>;
export type ScriptStepErrorMap = Record<string, number>;

export interface ScriptCheckCounter {
  total: number;
  passed: number;
  failed: number;
}

export type ScriptCheckSummary = Record<string, ScriptCheckCounter>;

export interface ScriptRunResult {
  vus: number;
  duration_target_s: number;
  elapsed_s: number;
  started_at: string;
  finished_at: string;
  iterations: number;
  steps_executed: number;
  iteration_rate: number;
  step_rate: number;
  checks_total: number;
  checks_passed: number;
  checks_failed: number;
  errors_total: number;
  step_stats: ScriptStepStats;
  step_errors: ScriptStepErrorMap;
  check_summary: ScriptCheckSummary;
}

// --- Distributed mode types ---

export type WorkerStatus = "idle" | "running" | "finished" | "error";

export interface WorkerHealthResponse {
  status: WorkerStatus;
  abi_version: number;
}

export interface WorkerRunRequest {
  config: BenchConfig;
  start_at?: string;
}

export interface DistributedConfig {
  workers: string[];
  config: BenchConfig;
  start_delay_ms?: number;
  timeout_ms?: number;
}

export interface DistributedResult {
  merged_report: RunReport;
  worker_reports: RunReport[];
  worker_errors: WorkerError[];
}

export interface WorkerError {
  worker_url: string;
  error: string;
}
