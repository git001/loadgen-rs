use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use loadgen_rs::{BenchConfig, metrics::RunReport, run_from_config};
use reqwest::cookie::Jar;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, LOCATION};
use reqwest::redirect::Policy;
use reqwest::{Client, Method, Url, Version};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

struct BenchHandle {
    config: BenchConfig,
    last_report: Option<RunReport>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum StepProtocol {
    H1,
    H2,
    H3,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum StepRedirectPolicy {
    Follow,
    Error,
    Manual,
}

impl Default for StepRedirectPolicy {
    fn default() -> Self {
        Self::Follow
    }
}

#[derive(Debug, Clone, Deserialize)]
struct StepSessionConfig {
    #[serde(default)]
    protocol: Option<StepProtocol>,
    #[serde(default = "default_connect_timeout_s")]
    connect_timeout_s: f64,
    #[serde(default = "default_request_timeout_s")]
    request_timeout_s: f64,
    #[serde(default)]
    insecure: bool,
    #[serde(default)]
    tls_ca: Option<String>,
    #[serde(default = "default_cookie_jar")]
    cookie_jar: bool,
    #[serde(default)]
    redirect_policy: StepRedirectPolicy,
    #[serde(default = "default_response_body_limit")]
    response_body_limit: usize,
    #[serde(default = "default_response_headers")]
    response_headers: bool,
}

fn default_connect_timeout_s() -> f64 {
    10.0
}

fn default_request_timeout_s() -> f64 {
    30.0
}

fn default_cookie_jar() -> bool {
    true
}

fn default_response_body_limit() -> usize {
    65_536
}

fn default_response_headers() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
struct StepRequest {
    name: String,
    #[serde(default)]
    method: Option<String>,
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    redirect_policy: Option<StepRedirectPolicy>,
    #[serde(default)]
    capture_body: Option<bool>,
    #[serde(default)]
    use_cookies: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
struct StepError {
    code: String,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
struct StepResponse {
    ok: bool,
    status: Option<u16>,
    url_final: Option<String>,
    http_version: Option<String>,
    latency_us: Option<u64>,
    ttfb_us: Option<u64>,
    bytes_in: u64,
    bytes_out: u64,
    headers: HashMap<String, String>,
    body: Option<String>,
    body_truncated: bool,
    redirect_count: u32,
    step_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<StepError>,
}

impl StepResponse {
    fn error(step_name: &str, url: &str, code: &str, message: String) -> Self {
        Self {
            ok: false,
            status: None,
            url_final: Some(url.to_string()),
            http_version: None,
            latency_us: None,
            ttfb_us: None,
            bytes_in: 0,
            bytes_out: 0,
            headers: HashMap::new(),
            body: None,
            body_truncated: false,
            redirect_count: 0,
            step_name: step_name.to_string(),
            error: Some(StepError {
                code: code.to_string(),
                message,
            }),
        }
    }
}

#[derive(Debug)]
struct StepExecError {
    code: &'static str,
    message: String,
}

struct StepClients {
    follow: Client,
    error: Client,
    manual: Client,
}

impl StepClients {
    fn for_policy(&self, policy: StepRedirectPolicy) -> Client {
        match policy {
            StepRedirectPolicy::Follow => self.follow.clone(),
            StepRedirectPolicy::Error => self.error.clone(),
            StepRedirectPolicy::Manual => self.manual.clone(),
        }
    }
}

struct StepSessionHandle {
    config: StepSessionConfig,
    runtime: tokio::runtime::Runtime,
    cookie_jar: Option<Arc<Jar>>,
    cookie_clients: StepClients,
    plain_clients: StepClients,
    last_step_response_json: Option<String>,
}

fn last_error_slot() -> &'static Mutex<Option<String>> {
    static LAST_ERROR: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    LAST_ERROR.get_or_init(|| Mutex::new(None))
}

fn set_last_error(message: impl Into<String>) {
    if let Ok(mut guard) = last_error_slot().lock() {
        *guard = Some(message.into());
    }
}

fn clear_last_error() {
    if let Ok(mut guard) = last_error_slot().lock() {
        *guard = None;
    }
}

fn make_c_string_ptr(value: String) -> *mut c_char {
    let sanitized = value.replace('\0', "\\0");
    match CString::new(sanitized) {
        Ok(s) => s.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

fn parse_config(config_json: *const c_char) -> Result<BenchConfig, String> {
    if config_json.is_null() {
        return Err("config_json is null".to_string());
    }
    let config_str = unsafe { CStr::from_ptr(config_json) }
        .to_str()
        .map_err(|e| format!("config_json is not valid UTF-8: {e}"))?;
    serde_json::from_str::<BenchConfig>(config_str)
        .map_err(|e| format!("config_json parse failed: {e}"))
}

fn parse_json_arg(arg_ptr: *const c_char, arg_name: &str) -> Result<Value, String> {
    if arg_ptr.is_null() {
        return Err(format!("{arg_name} is null"));
    }
    let arg_str = unsafe { CStr::from_ptr(arg_ptr) }
        .to_str()
        .map_err(|e| format!("{arg_name} is not valid UTF-8: {e}"))?;
    serde_json::from_str::<Value>(arg_str).map_err(|e| format!("{arg_name} parse failed: {e}"))
}

fn parse_step_session_config(config_json: *const c_char) -> Result<StepSessionConfig, String> {
    let value = parse_json_arg(config_json, "session_config_json")?;
    let config: StepSessionConfig = serde_json::from_value(value)
        .map_err(|e| format!("session_config_json parse failed: {e}"))?;

    if !config.connect_timeout_s.is_finite() || config.connect_timeout_s <= 0.0 {
        return Err("session_config.connect_timeout_s must be > 0".to_string());
    }
    if !config.request_timeout_s.is_finite() || config.request_timeout_s <= 0.0 {
        return Err("session_config.request_timeout_s must be > 0".to_string());
    }

    Ok(config)
}

fn parse_step_request(step_request_json: *const c_char) -> Result<StepRequest, String> {
    let value = parse_json_arg(step_request_json, "step_request_json")?;
    let request: StepRequest = serde_json::from_value(value)
        .map_err(|e| format!("step_request_json parse failed: {e}"))?;

    if request.name.trim().is_empty() {
        return Err("step_request.name must not be empty".to_string());
    }
    if request.url.trim().is_empty() {
        return Err("step_request.url must not be empty".to_string());
    }

    Ok(request)
}

fn apply_protocol(
    builder: reqwest::ClientBuilder,
    protocol: Option<StepProtocol>,
) -> Result<reqwest::ClientBuilder, String> {
    match protocol {
        Some(StepProtocol::H1) => Ok(builder.http1_only()),
        Some(StepProtocol::H2) | None => Ok(builder),
        Some(StepProtocol::H3) => Ok(builder.http3_prior_knowledge()),
    }
}

fn add_custom_ca(
    builder: reqwest::ClientBuilder,
    path: &str,
) -> Result<reqwest::ClientBuilder, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("failed to read tls_ca '{}': {e}", path))?;

    if let Ok(cert) = reqwest::Certificate::from_pem(&bytes) {
        return Ok(builder.add_root_certificate(cert));
    }
    if let Ok(cert) = reqwest::Certificate::from_der(&bytes) {
        return Ok(builder.add_root_certificate(cert));
    }

    Err(format!(
        "failed to parse tls_ca '{}': expected PEM or DER certificate",
        path
    ))
}

fn redirect_policy(policy: StepRedirectPolicy) -> Policy {
    match policy {
        StepRedirectPolicy::Follow => Policy::limited(10),
        StepRedirectPolicy::Error | StepRedirectPolicy::Manual => Policy::none(),
    }
}

fn build_client(
    config: &StepSessionConfig,
    policy: StepRedirectPolicy,
    cookie_jar: Option<Arc<Jar>>,
) -> Result<Client, String> {
    let mut builder = reqwest::Client::builder()
        .danger_accept_invalid_certs(config.insecure)
        .connect_timeout(Duration::from_secs_f64(config.connect_timeout_s))
        .timeout(Duration::from_secs_f64(config.request_timeout_s))
        .redirect(redirect_policy(policy));

    builder = apply_protocol(builder, config.protocol)?;

    if let Some(path) = config.tls_ca.as_deref() {
        builder = add_custom_ca(builder, path)?;
    }

    if let Some(jar) = cookie_jar {
        builder = builder.cookie_provider(jar);
    }

    builder
        .build()
        .map_err(|e| format!("failed to build step http client: {e}; debug={e:?}"))
}

fn build_clients(
    config: &StepSessionConfig,
    cookie_jar: Option<Arc<Jar>>,
) -> Result<StepClients, String> {
    Ok(StepClients {
        follow: build_client(config, StepRedirectPolicy::Follow, cookie_jar.clone())?,
        error: build_client(config, StepRedirectPolicy::Error, cookie_jar.clone())?,
        manual: build_client(config, StepRedirectPolicy::Manual, cookie_jar)?,
    })
}

fn create_step_session(config: StepSessionConfig) -> Result<StepSessionHandle, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("failed to build tokio runtime for step session: {e}"))?;

    let cookie_jar = if config.cookie_jar {
        Some(Arc::new(Jar::default()))
    } else {
        None
    };

    let (cookie_clients, plain_clients) = {
        let _guard = runtime.enter();
        (
            build_clients(&config, cookie_jar.clone())?,
            build_clients(&config, None)?,
        )
    };

    Ok(StepSessionHandle {
        config,
        runtime,
        cookie_jar,
        cookie_clients,
        plain_clients,
        last_step_response_json: None,
    })
}

fn rebuild_step_session(session: &mut StepSessionHandle) -> Result<(), String> {
    session.cookie_jar = if session.config.cookie_jar {
        Some(Arc::new(Jar::default()))
    } else {
        None
    };
    let (cookie_clients, plain_clients) = {
        let _guard = session.runtime.enter();
        (
            build_clients(&session.config, session.cookie_jar.clone())?,
            build_clients(&session.config, None)?,
        )
    };
    session.cookie_clients = cookie_clients;
    session.plain_clients = plain_clients;
    session.last_step_response_json = None;
    Ok(())
}

fn collect_response_headers(headers: &HeaderMap) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    for (name, value) in headers {
        let key = name.as_str().to_string();
        let value_str = value.to_str().unwrap_or_default().to_string();
        if let Some(existing) = out.get_mut(&key) {
            existing.push_str(", ");
            existing.push_str(&value_str);
        } else {
            out.insert(key, value_str);
        }
    }
    out
}

fn map_http_version(version: Version) -> String {
    match version {
        Version::HTTP_09 => "http/0.9".to_string(),
        Version::HTTP_10 => "http/1.0".to_string(),
        Version::HTTP_11 => "http/1.1".to_string(),
        Version::HTTP_2 => "h2".to_string(),
        Version::HTTP_3 => "h3".to_string(),
        _ => format!("{version:?}"),
    }
}

fn estimate_bytes_out(
    method: &str,
    url: &str,
    headers: &HashMap<String, String>,
    body: Option<&str>,
) -> u64 {
    let parsed = Url::parse(url).ok();
    let path_query_len = parsed
        .as_ref()
        .map(|u| {
            let mut s = u.path().to_string();
            if let Some(q) = u.query() {
                s.push('?');
                s.push_str(q);
            }
            s.len()
        })
        .unwrap_or(url.len());

    let mut size = method.len() as u64 + 1 + path_query_len as u64 + 10;
    for (k, v) in headers {
        size += (k.len() + v.len() + 4) as u64;
    }
    if let Some(body) = body {
        size += body.len() as u64;
    }
    size
}

fn capture_body_text(bytes: &[u8], limit: usize) -> (Option<String>, bool) {
    if limit == 0 {
        return (Some(String::new()), !bytes.is_empty());
    }

    if bytes.len() <= limit {
        return (Some(String::from_utf8_lossy(bytes).into_owned()), false);
    }

    let text = String::from_utf8_lossy(&bytes[..limit]).into_owned();
    (Some(text), true)
}

async fn execute_native_step(
    client: Client,
    config: StepSessionConfig,
    request: StepRequest,
    policy: StepRedirectPolicy,
) -> Result<StepResponse, StepExecError> {
    let method_str = request
        .method
        .as_deref()
        .unwrap_or(if request.body.is_some() {
            "POST"
        } else {
            "GET"
        });

    let method = Method::from_bytes(method_str.as_bytes()).map_err(|e| StepExecError {
        code: "invalid_method",
        message: format!("invalid HTTP method '{method_str}': {e}"),
    })?;

    let parsed_url = Url::parse(&request.url).map_err(|e| StepExecError {
        code: "invalid_url",
        message: format!("invalid URL '{}': {e}", request.url),
    })?;

    if matches!(config.protocol, Some(StepProtocol::H3)) && parsed_url.scheme() != "https" {
        return Err(StepExecError {
            code: "invalid_url",
            message: format!(
                "step session protocol 'h3' requires an https URL, got '{}'",
                request.url
            ),
        });
    }

    let mut req_builder = client.request(method.clone(), parsed_url.clone());

    for (name, value) in &request.headers {
        let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|e| StepExecError {
            code: "invalid_header_name",
            message: format!("invalid header name '{}': {e}", name),
        })?;
        let header_value = HeaderValue::from_str(value).map_err(|e| StepExecError {
            code: "invalid_header_value",
            message: format!("invalid value for header '{}': {e}", name),
        })?;
        req_builder = req_builder.header(header_name, header_value);
    }

    if let Some(body) = &request.body {
        req_builder = req_builder.body(body.clone());
    }

    let started = Instant::now();
    let response = req_builder.send().await.map_err(|e| {
        let code = if e.is_timeout() {
            "timeout"
        } else if e.is_connect() {
            "connect"
        } else if e.is_request() {
            "request"
        } else if e.is_body() {
            "body"
        } else {
            "http"
        };
        StepExecError {
            code,
            message: format!("{e}; debug={e:?}"),
        }
    })?;
    let ttfb_us = started.elapsed().as_micros() as u64;

    let status = response.status().as_u16();

    if matches!(policy, StepRedirectPolicy::Error)
        && (300..400).contains(&status)
        && response.headers().contains_key(LOCATION)
    {
        return Err(StepExecError {
            code: "redirect_not_allowed",
            message: format!("received redirect status {status} while redirect policy is 'error'"),
        });
    }

    let final_url = response.url().to_string();
    let version = map_http_version(response.version());
    let headers = if config.response_headers {
        collect_response_headers(response.headers())
    } else {
        HashMap::new()
    };

    let body_bytes = response.bytes().await.map_err(|e| StepExecError {
        code: "body_read",
        message: format!("{e}; debug={e:?}"),
    })?;

    let latency_us = started.elapsed().as_micros() as u64;
    let bytes_in = body_bytes.len() as u64;
    let capture_body = request.capture_body.unwrap_or(false);
    let (body, body_truncated) = if capture_body {
        capture_body_text(&body_bytes, config.response_body_limit)
    } else {
        (None, false)
    };

    let bytes_out = estimate_bytes_out(
        method_str,
        &request.url,
        &request.headers,
        request.body.as_deref(),
    );
    let redirect_count = if request.url != final_url { 1 } else { 0 };

    Ok(StepResponse {
        ok: true,
        status: Some(status),
        url_final: Some(final_url),
        http_version: Some(version),
        latency_us: Some(latency_us),
        ttfb_us: Some(ttfb_us),
        bytes_in,
        bytes_out,
        headers,
        body,
        body_truncated,
        redirect_count,
        step_name: request.name,
        error: None,
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn loadgen_abi_version() -> u32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn loadgen_step_abi_version() -> u32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn loadgen_create(config_json: *const c_char) -> *mut c_void {
    match std::panic::catch_unwind(|| parse_config(config_json)) {
        Ok(Ok(config)) => {
            clear_last_error();
            let handle = Box::new(BenchHandle {
                config,
                last_report: None,
            });
            Box::into_raw(handle).cast::<c_void>()
        }
        Ok(Err(e)) => {
            set_last_error(e);
            ptr::null_mut()
        }
        Err(_) => {
            set_last_error("panic in loadgen_create");
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn loadgen_run(handle: *mut c_void) -> *mut c_char {
    match std::panic::catch_unwind(|| {
        if handle.is_null() {
            return Err("handle is null".to_string());
        }

        let handle = unsafe { &mut *handle.cast::<BenchHandle>() };
        let report = run_from_config(handle.config.clone()).map_err(|e| e.to_string())?;
        let report_json = serde_json::to_string(&report)
            .map_err(|e| format!("failed to serialize report: {e}"))?;
        handle.last_report = Some(report);
        Ok(report_json)
    }) {
        Ok(Ok(json)) => {
            clear_last_error();
            make_c_string_ptr(json)
        }
        Ok(Err(e)) => {
            set_last_error(e);
            ptr::null_mut()
        }
        Err(_) => {
            set_last_error("panic in loadgen_run");
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn loadgen_metrics_snapshot(handle: *mut c_void) -> *mut c_char {
    match std::panic::catch_unwind(|| {
        if handle.is_null() {
            return Err("handle is null".to_string());
        }
        let handle = unsafe { &mut *handle.cast::<BenchHandle>() };
        let payload = match handle.last_report.as_ref() {
            Some(report) => serde_json::to_string(report)
                .map_err(|e| format!("failed to serialize snapshot report: {e}"))?,
            None => "{\"state\":\"idle\"}".to_string(),
        };
        Ok(payload)
    }) {
        Ok(Ok(json)) => {
            clear_last_error();
            make_c_string_ptr(json)
        }
        Ok(Err(e)) => {
            set_last_error(e);
            ptr::null_mut()
        }
        Err(_) => {
            set_last_error("panic in loadgen_metrics_snapshot");
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn loadgen_last_error() -> *mut c_char {
    let message = match last_error_slot().lock() {
        Ok(guard) => guard.clone(),
        Err(_) => Some("failed to lock error state".to_string()),
    };
    match message {
        Some(m) => make_c_string_ptr(m),
        None => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn loadgen_free_string(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(CString::from_raw(ptr));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn loadgen_destroy(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(handle.cast::<BenchHandle>()));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn loadgen_step_session_create(session_config_json: *const c_char) -> *mut c_void {
    match std::panic::catch_unwind(|| {
        let config = parse_step_session_config(session_config_json)?;
        create_step_session(config)
    }) {
        Ok(Ok(handle)) => {
            clear_last_error();
            Box::into_raw(Box::new(handle)).cast::<c_void>()
        }
        Ok(Err(e)) => {
            set_last_error(e);
            ptr::null_mut()
        }
        Err(_) => {
            set_last_error("panic in loadgen_step_session_create");
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn loadgen_step_execute(
    session_handle: *mut c_void,
    step_request_json: *const c_char,
) -> *mut c_char {
    match std::panic::catch_unwind(|| {
        if session_handle.is_null() {
            return Err("session_handle is null".to_string());
        }

        let session = unsafe { &mut *session_handle.cast::<StepSessionHandle>() };
        let request = parse_step_request(step_request_json)?;

        let step_name = request.name.clone();
        let step_url = request.url.clone();
        let policy = request
            .redirect_policy
            .unwrap_or(session.config.redirect_policy);
        let use_cookies = request.use_cookies.unwrap_or(session.config.cookie_jar);

        let client = if use_cookies {
            session.cookie_clients.for_policy(policy)
        } else {
            session.plain_clients.for_policy(policy)
        };

        let config = session.config.clone();
        let response = session
            .runtime
            .block_on(execute_native_step(client, config, request, policy));

        let step_response = match response {
            Ok(success) => success,
            Err(err) => StepResponse::error(&step_name, &step_url, err.code, err.message),
        };

        let payload = serde_json::to_string(&step_response)
            .map_err(|e| format!("failed to serialize step response: {e}"))?;
        session.last_step_response_json = Some(payload.clone());
        Ok(payload)
    }) {
        Ok(Ok(payload)) => {
            clear_last_error();
            make_c_string_ptr(payload)
        }
        Ok(Err(e)) => {
            set_last_error(e);
            ptr::null_mut()
        }
        Err(_) => {
            set_last_error("panic in loadgen_step_execute");
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn loadgen_step_snapshot(session_handle: *mut c_void) -> *mut c_char {
    match std::panic::catch_unwind(|| {
        if session_handle.is_null() {
            return Err("session_handle is null".to_string());
        }
        let session = unsafe { &mut *session_handle.cast::<StepSessionHandle>() };
        let payload = match session.last_step_response_json.as_ref() {
            Some(v) => v.clone(),
            None => json!({ "state": "idle", "kind": "step_session" }).to_string(),
        };
        Ok(payload)
    }) {
        Ok(Ok(payload)) => {
            clear_last_error();
            make_c_string_ptr(payload)
        }
        Ok(Err(e)) => {
            set_last_error(e);
            ptr::null_mut()
        }
        Err(_) => {
            set_last_error("panic in loadgen_step_snapshot");
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn loadgen_step_session_reset(session_handle: *mut c_void) {
    let _ = std::panic::catch_unwind(|| {
        if session_handle.is_null() {
            set_last_error("session_handle is null");
            return;
        }

        let session = unsafe { &mut *session_handle.cast::<StepSessionHandle>() };
        match rebuild_step_session(session) {
            Ok(()) => clear_last_error(),
            Err(e) => set_last_error(e),
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn loadgen_step_session_destroy(session_handle: *mut c_void) {
    if session_handle.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(session_handle.cast::<StepSessionHandle>()));
    }
}

/// Merge multiple distributed RunReports (JSON array) into one merged RunReport (JSON string).
/// Input: `reports_json` — a JSON array of RunReport objects (with `*_hist_b64` fields).
/// Returns: merged RunReport as a JSON C-string, or null on error (check `loadgen_last_error`).
#[unsafe(no_mangle)]
pub extern "C" fn loadgen_merge_reports(reports_json: *const c_char) -> *mut c_char {
    match std::panic::catch_unwind(|| {
        if reports_json.is_null() {
            return Err("reports_json is null".to_string());
        }
        let json_str = unsafe { CStr::from_ptr(reports_json) }
            .to_str()
            .map_err(|e| format!("reports_json is not valid UTF-8: {e}"))?;
        let reports: Vec<RunReport> = serde_json::from_str(json_str)
            .map_err(|e| format!("reports_json parse failed: {e}"))?;
        let merged = loadgen_rs::metrics::merge_distributed_reports(reports)?;
        serde_json::to_string(&merged)
            .map_err(|e| format!("failed to serialize merged report: {e}"))
    }) {
        Ok(Ok(json)) => {
            clear_last_error();
            make_c_string_ptr(json)
        }
        Ok(Err(e)) => {
            set_last_error(e);
            ptr::null_mut()
        }
        Err(_) => {
            set_last_error("panic in loadgen_merge_reports");
            ptr::null_mut()
        }
    }
}
