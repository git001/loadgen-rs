use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use hdrhistogram::Histogram;
use hdrhistogram::serialization::{Deserializer as HistDeserializer, Serializer as _, V2DeflateSerializer};
use serde::{Deserialize, Serialize};

const STATUS_FASTPATH_MAX: usize = 600;

/// Per-worker metrics collected during a benchmark run.
/// Each worker/connection task owns one of these — no locks needed.
pub struct WorkerMetrics {
    pub latency_hist: Histogram<u64>,
    pub ttfb_hist: Histogram<u64>,
    pub connect_hist: Histogram<u64>,
    pub requests_started: u64,
    pub requests_completed: u64,
    pub ok: u64,
    pub err_connect: u64,
    pub err_tls: u64,
    pub err_timeout: u64,
    pub err_http: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub connect_v4_count: u64,
    pub connect_v6_count: u64,
    pub connect_addr_counts: HashMap<String, u64>,
    pub status_counts_fast: [u64; STATUS_FASTPATH_MAX],
    pub status_counts_other: HashMap<u16, u64>,
    pub tls_protocol: Option<String>,
    pub tls_cipher: Option<String>,
}

impl WorkerMetrics {
    pub fn new() -> Self {
        Self {
            // Record latencies up to 60 seconds with 3 significant digits
            latency_hist: Histogram::new_with_bounds(1, 60_000_000, 3).unwrap(),
            ttfb_hist: Histogram::new_with_bounds(1, 60_000_000, 3).unwrap(),
            connect_hist: Histogram::new_with_bounds(1, 60_000_000, 3).unwrap(),
            requests_started: 0,
            requests_completed: 0,
            ok: 0,
            err_connect: 0,
            err_tls: 0,
            err_timeout: 0,
            err_http: 0,
            bytes_in: 0,
            bytes_out: 0,
            connect_v4_count: 0,
            connect_v6_count: 0,
            connect_addr_counts: HashMap::new(),
            status_counts_fast: [0; STATUS_FASTPATH_MAX],
            status_counts_other: HashMap::new(),
            tls_protocol: None,
            tls_cipher: None,
        }
    }

    /// Record a successful request.
    pub fn record_success(
        &mut self,
        status: u16,
        latency_us: u64,
        ttfb_us: u64,
        bytes_in: u64,
        bytes_out: u64,
        record_timing: bool,
    ) {
        self.requests_completed += 1;
        self.ok += 1;
        if record_timing {
            let _ = self.latency_hist.record(latency_us);
            let _ = self.ttfb_hist.record(ttfb_us);
        }
        self.bytes_in += bytes_in;
        self.bytes_out += bytes_out;
        let idx = status as usize;
        if idx < STATUS_FASTPATH_MAX {
            self.status_counts_fast[idx] += 1;
        } else {
            *self.status_counts_other.entry(status).or_insert(0) += 1;
        }
    }

    /// Record a connection timing.
    pub fn record_connect(&mut self, connect_us: u64, remote_addr: Option<SocketAddr>) {
        let _ = self.connect_hist.record(connect_us);
        if let Some(addr) = remote_addr {
            match addr {
                SocketAddr::V4(_) => self.connect_v4_count += 1,
                SocketAddr::V6(_) => self.connect_v6_count += 1,
            }
            *self
                .connect_addr_counts
                .entry(addr.to_string())
                .or_insert(0) += 1;
        }
    }

    /// Record a failed request with error classification.
    pub fn record_error(&mut self, class: ErrorClass) {
        self.requests_completed += 1;
        match class {
            ErrorClass::Connect => self.err_connect += 1,
            ErrorClass::Tls => self.err_tls += 1,
            ErrorClass::Timeout => self.err_timeout += 1,
            ErrorClass::Http => self.err_http += 1,
        }
    }

    pub fn record_tls_info(&mut self, protocol: Option<&str>, cipher: Option<&str>) {
        if self.tls_protocol.is_none()
            && let Some(protocol) = protocol
        {
            self.tls_protocol = Some(protocol.to_string());
        }
        if self.tls_cipher.is_none()
            && let Some(cipher) = cipher
        {
            self.tls_cipher = Some(cipher.to_string());
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ErrorClass {
    Connect,
    Tls,
    Timeout,
    Http,
}

/// Aggregated metrics from all workers for the final report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    // Configuration
    pub proto: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub h3_backend: Option<String>,
    pub clients: usize,
    pub threads: usize,
    pub max_streams: u32,
    pub mode: String,
    pub tls_protocol: Option<String>,
    pub tls_cipher: Option<String>,
    pub duration_s: f64,
    pub metrics_sample: u32,
    pub requests_target: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rps_target: Option<f64>,

    // Counters
    pub requests_started: u64,
    pub requests_completed: u64,
    pub ok: u64,
    pub err_total: u64,
    pub status_counts: HashMap<u16, u64>,
    pub status_2xx: u64,
    pub status_3xx: u64,
    pub status_4xx: u64,
    pub status_5xx: u64,

    // Performance
    pub rps: f64,
    pub started_rps: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rps_target_achieved_pct: Option<f64>,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub mbps_in: f64,
    pub mbps_out: f64,

    // Latency (microseconds)
    pub latency_min_us: u64,
    pub latency_p50_us: u64,
    pub latency_p90_us: u64,
    pub latency_p99_us: u64,
    pub latency_mean_us: f64,
    pub latency_max_us: u64,
    pub latency_stdev_us: f64,

    // TTFB (microseconds)
    pub ttfb_min_us: u64,
    pub ttfb_p50_us: u64,
    pub ttfb_p90_us: u64,
    pub ttfb_p99_us: u64,
    pub ttfb_mean_us: f64,
    pub ttfb_max_us: u64,
    pub ttfb_stdev_us: f64,

    // Connect (microseconds)
    pub connect_min_us: u64,
    pub connect_p50_us: u64,
    pub connect_p90_us: u64,
    pub connect_p99_us: u64,
    pub connect_mean_us: f64,
    pub connect_max_us: u64,
    pub connect_stdev_us: f64,
    pub connect_v4_count: u64,
    pub connect_v6_count: u64,
    pub connect_addr_counts: HashMap<String, u64>,

    // Errors
    pub err_connect: u64,
    pub err_tls: u64,
    pub err_timeout: u64,
    pub err_http: u64,

    // Actual elapsed time
    pub elapsed_s: f64,

    // Optional V2-Deflate histograms encoded as base64 (for distributed merge)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub latency_hist_b64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ttfb_hist_b64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub connect_hist_b64: Option<String>,
}

/// Serialize an HdrHistogram to V2-Deflate format, then base64-encode.
pub fn serialize_histogram_b64(hist: &Histogram<u64>) -> Option<String> {
    if hist.is_empty() {
        return None;
    }
    let mut buf = Vec::new();
    let mut serializer = V2DeflateSerializer::new();
    if serializer.serialize(hist, &mut buf).is_ok() {
        Some(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &buf))
    } else {
        None
    }
}

/// Decode a base64-encoded V2-Deflate histogram back into an HdrHistogram.
pub fn deserialize_histogram_b64(b64: &str) -> Result<Histogram<u64>, String> {
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64)
        .map_err(|e| format!("base64 decode error: {e}"))?;
    let mut deserializer = HistDeserializer::new();
    deserializer
        .deserialize(&mut &bytes[..])
        .map_err(|e| format!("histogram V2Deflate decode error: {e}"))
}

/// Merge multiple worker metrics into a single RunReport.
pub fn merge_metrics(
    workers: Vec<WorkerMetrics>,
    elapsed: Duration,
    proto: &str,
    url: &str,
    h3_backend: Option<&str>,
    clients: usize,
    threads: usize,
    max_streams: u32,
    mode: &str,
    duration_s: f64,
    metrics_sample: u32,
    requests_target: u64,
    rps_target: Option<f64>,
    export_histograms: bool,
) -> RunReport {
    let mut latency = Histogram::new_with_bounds(1, 60_000_000, 3).unwrap();
    let mut ttfb = Histogram::new_with_bounds(1, 60_000_000, 3).unwrap();
    let mut connect = Histogram::new_with_bounds(1, 60_000_000, 3).unwrap();
    let mut requests_started = 0u64;
    let mut requests_completed = 0u64;
    let mut ok = 0u64;
    let mut err_connect = 0u64;
    let mut err_tls = 0u64;
    let mut err_timeout = 0u64;
    let mut err_http = 0u64;
    let mut bytes_in = 0u64;
    let mut bytes_out = 0u64;
    let mut connect_v4_count = 0u64;
    let mut connect_v6_count = 0u64;
    let mut connect_addr_counts: HashMap<String, u64> = HashMap::new();
    let mut status_counts: HashMap<u16, u64> = HashMap::new();
    let mut tls_protocol: Option<String> = None;
    let mut tls_cipher: Option<String> = None;

    for w in workers {
        let _ = latency.add(&w.latency_hist);
        let _ = ttfb.add(&w.ttfb_hist);
        let _ = connect.add(&w.connect_hist);
        requests_started += w.requests_started;
        requests_completed += w.requests_completed;
        ok += w.ok;
        err_connect += w.err_connect;
        err_tls += w.err_tls;
        err_timeout += w.err_timeout;
        err_http += w.err_http;
        bytes_in += w.bytes_in;
        bytes_out += w.bytes_out;
        connect_v4_count += w.connect_v4_count;
        connect_v6_count += w.connect_v6_count;
        if tls_protocol.is_none() {
            tls_protocol = w.tls_protocol.clone();
        }
        if tls_cipher.is_none() {
            tls_cipher = w.tls_cipher.clone();
        }
        for status in 0..STATUS_FASTPATH_MAX {
            let count = w.status_counts_fast[status];
            if count > 0 {
                *status_counts.entry(status as u16).or_insert(0) += count;
            }
        }
        for (status, count) in w.status_counts_other {
            *status_counts.entry(status).or_insert(0) += count;
        }
        for (addr, count) in w.connect_addr_counts {
            *connect_addr_counts.entry(addr).or_insert(0) += count;
        }
    }

    let mut status_2xx = 0u64;
    let mut status_3xx = 0u64;
    let mut status_4xx = 0u64;
    let mut status_5xx = 0u64;
    for (&code, &count) in &status_counts {
        match code {
            200..=299 => status_2xx += count,
            300..=399 => status_3xx += count,
            400..=499 => status_4xx += count,
            500..=599 => status_5xx += count,
            _ => {}
        }
    }

    let elapsed_s = elapsed.as_secs_f64();
    let rps = if elapsed_s > 0.0 {
        requests_completed as f64 / elapsed_s
    } else {
        0.0
    };
    let started_rps = if elapsed_s > 0.0 {
        requests_started as f64 / elapsed_s
    } else {
        0.0
    };
    let rps_target_achieved_pct = rps_target.map(|target| {
        if target > 0.0 {
            (started_rps / target) * 100.0
        } else {
            0.0
        }
    });
    let mbps_in = if elapsed_s > 0.0 {
        (bytes_in as f64 * 8.0) / (elapsed_s * 1_000_000.0)
    } else {
        0.0
    };
    let mbps_out = if elapsed_s > 0.0 {
        (bytes_out as f64 * 8.0) / (elapsed_s * 1_000_000.0)
    } else {
        0.0
    };

    RunReport {
        proto: proto.to_string(),
        url: url.to_string(),
        h3_backend: h3_backend.map(str::to_string),
        clients,
        threads,
        max_streams,
        mode: mode.to_string(),
        tls_protocol,
        tls_cipher,
        duration_s,
        metrics_sample,
        requests_target,
        rps_target,
        requests_started,
        requests_completed,
        ok,
        err_total: err_connect + err_tls + err_timeout + err_http,
        status_counts,
        status_2xx,
        status_3xx,
        status_4xx,
        status_5xx,
        rps,
        started_rps,
        rps_target_achieved_pct,
        bytes_in,
        bytes_out,
        mbps_in,
        mbps_out,
        latency_min_us: latency.min(),
        latency_p50_us: latency.value_at_quantile(0.50),
        latency_p90_us: latency.value_at_quantile(0.90),
        latency_p99_us: latency.value_at_quantile(0.99),
        latency_mean_us: latency.mean(),
        latency_max_us: latency.max(),
        latency_stdev_us: latency.stdev(),
        ttfb_min_us: ttfb.min(),
        ttfb_p50_us: ttfb.value_at_quantile(0.50),
        ttfb_p90_us: ttfb.value_at_quantile(0.90),
        ttfb_p99_us: ttfb.value_at_quantile(0.99),
        ttfb_mean_us: ttfb.mean(),
        ttfb_max_us: ttfb.max(),
        ttfb_stdev_us: ttfb.stdev(),
        connect_min_us: connect.min(),
        connect_p50_us: connect.value_at_quantile(0.50),
        connect_p90_us: connect.value_at_quantile(0.90),
        connect_p99_us: connect.value_at_quantile(0.99),
        connect_mean_us: connect.mean(),
        connect_max_us: connect.max(),
        connect_stdev_us: connect.stdev(),
        connect_v4_count,
        connect_v6_count,
        connect_addr_counts,
        err_connect,
        err_tls,
        err_timeout,
        err_http,
        elapsed_s,
        latency_hist_b64: if export_histograms { serialize_histogram_b64(&latency) } else { None },
        ttfb_hist_b64: if export_histograms { serialize_histogram_b64(&ttfb) } else { None },
        connect_hist_b64: if export_histograms { serialize_histogram_b64(&connect) } else { None },
    }
}

/// Merge multiple distributed `RunReport`s (each from a different worker) into one.
///
/// Histograms are deserialized from `*_hist_b64` fields, merged with `Histogram::add()`,
/// and the merged report's percentile fields are recomputed from the combined histogram.
/// Counters are summed. `elapsed_s` is taken as max across workers.
/// The returned report has `*_hist_b64: None` (merged percentiles are sufficient).
pub fn merge_distributed_reports(reports: Vec<RunReport>) -> Result<RunReport, String> {
    if reports.is_empty() {
        return Err("no reports to merge".to_string());
    }
    if reports.len() == 1 {
        let mut r = reports.into_iter().next().unwrap();
        r.latency_hist_b64 = None;
        r.ttfb_hist_b64 = None;
        r.connect_hist_b64 = None;
        return Ok(r);
    }

    let mut latency = Histogram::<u64>::new_with_bounds(1, 60_000_000, 3)
        .map_err(|e| format!("histogram init: {e}"))?;
    let mut ttfb = Histogram::<u64>::new_with_bounds(1, 60_000_000, 3)
        .map_err(|e| format!("histogram init: {e}"))?;
    let mut connect = Histogram::<u64>::new_with_bounds(1, 60_000_000, 3)
        .map_err(|e| format!("histogram init: {e}"))?;

    let mut requests_started = 0u64;
    let mut requests_completed = 0u64;
    let mut ok = 0u64;
    let mut err_connect = 0u64;
    let mut err_tls = 0u64;
    let mut err_timeout = 0u64;
    let mut err_http = 0u64;
    let mut bytes_in = 0u64;
    let mut bytes_out = 0u64;
    let mut connect_v4_count = 0u64;
    let mut connect_v6_count = 0u64;
    let mut connect_addr_counts: HashMap<String, u64> = HashMap::new();
    let mut status_counts: HashMap<u16, u64> = HashMap::new();
    let mut elapsed_s: f64 = 0.0;
    let mut total_clients: usize = 0;
    let mut total_threads: usize = 0;

    // Take config from first report
    let first = &reports[0];
    let proto = first.proto.clone();
    let url = first.url.clone();
    let h3_backend = first.h3_backend.clone();
    let max_streams = first.max_streams;
    let mode = first.mode.clone();
    let tls_protocol = first.tls_protocol.clone();
    let tls_cipher = first.tls_cipher.clone();
    let duration_s = first.duration_s;
    let metrics_sample = first.metrics_sample;
    let rps_target = first.rps_target;

    for r in &reports {
        // Merge histograms
        if let Some(ref b64) = r.latency_hist_b64 {
            let h = deserialize_histogram_b64(b64)?;
            latency.add(&h).map_err(|e| format!("latency merge: {e}"))?;
        }
        if let Some(ref b64) = r.ttfb_hist_b64 {
            let h = deserialize_histogram_b64(b64)?;
            ttfb.add(&h).map_err(|e| format!("ttfb merge: {e}"))?;
        }
        if let Some(ref b64) = r.connect_hist_b64 {
            let h = deserialize_histogram_b64(b64)?;
            connect.add(&h).map_err(|e| format!("connect merge: {e}"))?;
        }

        // Sum counters
        requests_started += r.requests_started;
        requests_completed += r.requests_completed;
        ok += r.ok;
        err_connect += r.err_connect;
        err_tls += r.err_tls;
        err_timeout += r.err_timeout;
        err_http += r.err_http;
        bytes_in += r.bytes_in;
        bytes_out += r.bytes_out;
        connect_v4_count += r.connect_v4_count;
        connect_v6_count += r.connect_v6_count;
        total_clients += r.clients;
        total_threads += r.threads;

        for (&code, &count) in &r.status_counts {
            *status_counts.entry(code).or_insert(0) += count;
        }
        for (addr, &count) in &r.connect_addr_counts {
            *connect_addr_counts.entry(addr.clone()).or_insert(0) += count;
        }

        if r.elapsed_s > elapsed_s {
            elapsed_s = r.elapsed_s;
        }
    }

    let requests_target: u64 = reports.iter().map(|r| r.requests_target).sum();

    let mut status_2xx = 0u64;
    let mut status_3xx = 0u64;
    let mut status_4xx = 0u64;
    let mut status_5xx = 0u64;
    for (&code, &count) in &status_counts {
        match code {
            200..=299 => status_2xx += count,
            300..=399 => status_3xx += count,
            400..=499 => status_4xx += count,
            500..=599 => status_5xx += count,
            _ => {}
        }
    }

    let rps = if elapsed_s > 0.0 { requests_completed as f64 / elapsed_s } else { 0.0 };
    let started_rps = if elapsed_s > 0.0 { requests_started as f64 / elapsed_s } else { 0.0 };
    let rps_target_achieved_pct = rps_target.map(|target| {
        if target > 0.0 { (started_rps / target) * 100.0 } else { 0.0 }
    });
    let mbps_in = if elapsed_s > 0.0 { (bytes_in as f64 * 8.0) / (elapsed_s * 1_000_000.0) } else { 0.0 };
    let mbps_out = if elapsed_s > 0.0 { (bytes_out as f64 * 8.0) / (elapsed_s * 1_000_000.0) } else { 0.0 };

    Ok(RunReport {
        proto,
        url,
        h3_backend,
        clients: total_clients,
        threads: total_threads,
        max_streams,
        mode,
        tls_protocol,
        tls_cipher,
        duration_s,
        metrics_sample,
        requests_target,
        rps_target,
        requests_started,
        requests_completed,
        ok,
        err_total: err_connect + err_tls + err_timeout + err_http,
        status_counts,
        status_2xx,
        status_3xx,
        status_4xx,
        status_5xx,
        rps,
        started_rps,
        rps_target_achieved_pct,
        bytes_in,
        bytes_out,
        mbps_in,
        mbps_out,
        latency_min_us: latency.min(),
        latency_p50_us: latency.value_at_quantile(0.50),
        latency_p90_us: latency.value_at_quantile(0.90),
        latency_p99_us: latency.value_at_quantile(0.99),
        latency_mean_us: latency.mean(),
        latency_max_us: latency.max(),
        latency_stdev_us: latency.stdev(),
        ttfb_min_us: ttfb.min(),
        ttfb_p50_us: ttfb.value_at_quantile(0.50),
        ttfb_p90_us: ttfb.value_at_quantile(0.90),
        ttfb_p99_us: ttfb.value_at_quantile(0.99),
        ttfb_mean_us: ttfb.mean(),
        ttfb_max_us: ttfb.max(),
        ttfb_stdev_us: ttfb.stdev(),
        connect_min_us: connect.min(),
        connect_p50_us: connect.value_at_quantile(0.50),
        connect_p90_us: connect.value_at_quantile(0.90),
        connect_p99_us: connect.value_at_quantile(0.99),
        connect_mean_us: connect.mean(),
        connect_max_us: connect.max(),
        connect_stdev_us: connect.stdev(),
        connect_v4_count,
        connect_v6_count,
        connect_addr_counts,
        err_connect,
        err_tls,
        err_timeout,
        err_http,
        elapsed_s,
        latency_hist_b64: None,
        ttfb_hist_b64: None,
        connect_hist_b64: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic_worker_metrics() -> WorkerMetrics {
        let mut w = WorkerMetrics::new();
        w.requests_started = 20;
        w.requests_completed = 10;
        w.ok = 10;
        w.bytes_in = 1024;
        w.bytes_out = 128;
        w.status_counts_fast[200] = 10;
        let _ = w.latency_hist.record(100);
        let _ = w.ttfb_hist.record(50);
        w.record_connect(25, Some(SocketAddr::from(([127, 0, 0, 1], 443))));
        w
    }

    #[test]
    fn merge_metrics_reports_started_rps_and_target_pct() {
        let report = merge_metrics(
            vec![basic_worker_metrics()],
            Duration::from_secs(2),
            "h1",
            "https://example.test/",
            None,
            1,
            1,
            1,
            "count",
            0.0,
            1,
            10,
            Some(12.5),
            false,
        );

        assert!((report.rps - 5.0).abs() < 1e-9);
        assert!((report.started_rps - 10.0).abs() < 1e-9);
        assert_eq!(report.rps_target_achieved_pct, Some(80.0));
        assert_eq!(report.connect_v4_count, 1);
        assert_eq!(report.connect_v6_count, 0);
        assert_eq!(
            report.connect_addr_counts.get("127.0.0.1:443").copied(),
            Some(1)
        );
    }

    #[test]
    fn merge_metrics_target_pct_absent_without_target() {
        let report = merge_metrics(
            vec![basic_worker_metrics()],
            Duration::from_secs(2),
            "h1",
            "https://example.test/",
            None,
            1,
            1,
            1,
            "count",
            0.0,
            1,
            10,
            None,
            false,
        );

        assert_eq!(report.rps_target, None);
        assert_eq!(report.rps_target_achieved_pct, None);
    }
}
