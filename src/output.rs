use std::io::Write;
use std::path::Path;

use anyhow::Result;

use crate::cli::OutputFormat;
use crate::metrics::RunReport;

/// Write the run report to the specified output (file or stdout).
pub fn write_report(
    report: &RunReport,
    format: OutputFormat,
    output_path: Option<&Path>,
) -> Result<()> {
    let formatted = match format {
        OutputFormat::Jsonl => format_jsonl(report)?,
        OutputFormat::Csv => format_csv(report)?,
    };

    if let Some(path) = output_path {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(file, "{formatted}")?;
    } else {
        println!("{formatted}");
    }

    Ok(())
}

fn format_jsonl(report: &RunReport) -> Result<String> {
    Ok(serde_json::to_string(report)?)
}

fn format_csv(report: &RunReport) -> Result<String> {
    // CSV header + data line
    let header = "proto,url,h3_backend,clients,threads,max_streams,mode,tls_protocol,tls_cipher,duration_s,metrics_sample,requests_target,\
                  requests_started,requests_completed,ok,err_total,\
                  rps,bytes_in,bytes_out,mbps_in,mbps_out,\
                  latency_min_us,latency_p50_us,latency_p90_us,latency_p99_us,latency_mean_us,latency_max_us,latency_stdev_us,\
                  ttfb_min_us,ttfb_p50_us,ttfb_p90_us,ttfb_p99_us,ttfb_mean_us,ttfb_max_us,ttfb_stdev_us,\
                  connect_min_us,connect_p50_us,connect_p90_us,connect_p99_us,connect_mean_us,connect_max_us,connect_stdev_us,\
                  err_connect,err_tls,err_timeout,err_http,elapsed_s,started_rps,rps_target_achieved_pct,\
                  connect_v4_count,connect_v6_count,connect_addrs";

    let data = vec![
        report.proto.clone(),
        report.url.clone(),
        report.h3_backend.clone().unwrap_or_default(),
        report.clients.to_string(),
        report.threads.to_string(),
        report.max_streams.to_string(),
        report.mode.clone(),
        report.tls_protocol.clone().unwrap_or_default(),
        report.tls_cipher.clone().unwrap_or_default(),
        report.duration_s.to_string(),
        report.metrics_sample.to_string(),
        report.requests_target.to_string(),
        report.requests_started.to_string(),
        report.requests_completed.to_string(),
        report.ok.to_string(),
        report.err_total.to_string(),
        format!("{:.2}", report.rps),
        report.bytes_in.to_string(),
        report.bytes_out.to_string(),
        format!("{:.4}", report.mbps_in),
        format!("{:.4}", report.mbps_out),
        report.latency_min_us.to_string(),
        report.latency_p50_us.to_string(),
        report.latency_p90_us.to_string(),
        report.latency_p99_us.to_string(),
        format!("{:.2}", report.latency_mean_us),
        report.latency_max_us.to_string(),
        format!("{:.2}", report.latency_stdev_us),
        report.ttfb_min_us.to_string(),
        report.ttfb_p50_us.to_string(),
        report.ttfb_p90_us.to_string(),
        report.ttfb_p99_us.to_string(),
        format!("{:.2}", report.ttfb_mean_us),
        report.ttfb_max_us.to_string(),
        format!("{:.2}", report.ttfb_stdev_us),
        report.connect_min_us.to_string(),
        report.connect_p50_us.to_string(),
        report.connect_p90_us.to_string(),
        report.connect_p99_us.to_string(),
        format!("{:.2}", report.connect_mean_us),
        report.connect_max_us.to_string(),
        format!("{:.2}", report.connect_stdev_us),
        report.err_connect.to_string(),
        report.err_tls.to_string(),
        report.err_timeout.to_string(),
        report.err_http.to_string(),
        format!("{:.6}", report.elapsed_s),
        format!("{:.2}", report.started_rps),
        report
            .rps_target_achieved_pct
            .map(|v| format!("{v:.2}"))
            .unwrap_or_default(),
        report.connect_v4_count.to_string(),
        report.connect_v6_count.to_string(),
        format_connect_addrs(&report.connect_addr_counts),
    ]
    .join(",");

    Ok(format!("{header}\n{data}"))
}

/// Print a human-readable summary to stdout (always shown).
pub fn print_summary(report: &RunReport) {
    let in_rate_bps = if report.elapsed_s > 0.0 {
        report.bytes_in as f64 / report.elapsed_s
    } else {
        0.0
    };
    let out_rate_bps = if report.elapsed_s > 0.0 {
        report.bytes_out as f64 / report.elapsed_s
    } else {
        0.0
    };
    let total_bytes = report.bytes_in.saturating_add(report.bytes_out);

    println!();
    println!("=== loadgen-rs results ===");
    println!("Protocol:         {}", report.proto);
    if let Some(h3_backend) = report.h3_backend.as_deref() {
        println!("H3 Backend:       {}", h3_backend);
    }
    println!("URL:              {}", report.url);
    println!(
        "Config:           {} clients, {} threads, {} max streams",
        report.clients, report.threads, report.max_streams
    );
    if let Some(protocol) = report.tls_protocol.as_deref() {
        println!("TLS Protocol:     {}", protocol);
    }
    if let Some(cipher) = report.tls_cipher.as_deref() {
        println!("Cipher:           {}", cipher);
    }
    if report.metrics_sample > 1 {
        println!(
            "Metrics sample:   1/{} successful requests",
            report.metrics_sample
        );
    }
    println!("Mode:             {}", report.mode);
    println!("Elapsed:          {:.3}s", report.elapsed_s);
    println!();
    println!(
        "Requests:         {} completed ({} ok, {} errors)",
        report.requests_completed, report.ok, report.err_total
    );
    println!("RPS (completed):  {:.2}", report.rps);
    println!("RPS (started):    {:.2}", report.started_rps);
    if let Some(target) = report.rps_target {
        let per_client_target = if report.clients > 0 {
            target / report.clients as f64
        } else {
            target
        };
        if let Some(achieved) = report.rps_target_achieved_pct {
            println!(
                "RPS target:       {:.2} ({:.2}/client, started-achieved: {:.2}%)",
                target, per_client_target, achieved
            );
        } else {
            println!(
                "RPS target:       {:.2} ({:.2}/client)",
                target, per_client_target
            );
        }
    }
    println!(
        "Transfer:         in {}  out {}  total {}",
        format_bytes(report.bytes_in),
        format_bytes(report.bytes_out),
        format_bytes(total_bytes),
    );
    println!(
        "Throughput:       in {}  out {}",
        format_rate(in_rate_bps),
        format_rate(out_rate_bps),
    );
    println!();
    println!(
        "Latency (us):     min={} p50={} p90={} p99={} mean={:.0} max={} sd={:.0}",
        report.latency_min_us,
        report.latency_p50_us,
        report.latency_p90_us,
        report.latency_p99_us,
        report.latency_mean_us,
        report.latency_max_us,
        report.latency_stdev_us,
    );
    println!(
        "TTFB (us):        min={} p50={} p90={} p99={} mean={:.0} max={} sd={:.0}",
        report.ttfb_min_us,
        report.ttfb_p50_us,
        report.ttfb_p90_us,
        report.ttfb_p99_us,
        report.ttfb_mean_us,
        report.ttfb_max_us,
        report.ttfb_stdev_us,
    );
    println!(
        "Connect (us):     min={} p50={} p90={} p99={} mean={:.0} max={} sd={:.0}",
        report.connect_min_us,
        report.connect_p50_us,
        report.connect_p90_us,
        report.connect_p99_us,
        report.connect_mean_us,
        report.connect_max_us,
        report.connect_stdev_us,
    );
    println!(
        "Connect family:   v4={} v6={}",
        report.connect_v4_count, report.connect_v6_count
    );
    if !report.connect_addr_counts.is_empty() {
        println!(
            "Connect addrs:    {}",
            format_connect_addrs(&report.connect_addr_counts)
        );
    }
    println!();
    if report.err_total > 0 {
        println!(
            "Errors:           connect={} tls={} timeout={} http={}",
            report.err_connect, report.err_tls, report.err_timeout, report.err_http,
        );
    }
    if !report.status_counts.is_empty() {
        println!(
            "Status classes:   2xx={} 3xx={} 4xx={} 5xx={}",
            report.status_2xx, report.status_3xx, report.status_4xx, report.status_5xx,
        );
        let mut statuses: Vec<_> = report.status_counts.iter().collect();
        statuses.sort_by_key(|(k, _)| *k);
        let status_str: Vec<String> = statuses.iter().map(|(k, v)| format!("{k}={v}")).collect();
        println!("Status detail:    {}", status_str.join(", "));
    }
    println!("============================");
}

fn format_connect_addrs(connect_addr_counts: &std::collections::HashMap<String, u64>) -> String {
    if connect_addr_counts.is_empty() {
        return String::new();
    }
    let mut addrs: Vec<_> = connect_addr_counts.iter().collect();
    addrs.sort_by(|(addr_a, count_a), (addr_b, count_b)| {
        count_b
            .cmp(count_a)
            .then_with(|| addr_a.as_str().cmp(addr_b.as_str()))
    });
    addrs
        .into_iter()
        .map(|(addr, count)| format!("{addr}={count}"))
        .collect::<Vec<_>>()
        .join("|")
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1_000.0;
    const MB: f64 = 1_000_000.0;
    const GB: f64 = 1_000_000_000.0;
    let b = bytes as f64;

    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.2} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

fn format_rate(bytes_per_sec: f64) -> String {
    const KB: f64 = 1_000.0;
    const MB: f64 = 1_000_000.0;
    const GB: f64 = 1_000_000_000.0;

    if bytes_per_sec >= GB {
        format!("{:.2} GB/s", bytes_per_sec / GB)
    } else if bytes_per_sec >= MB {
        format!("{:.2} MB/s", bytes_per_sec / MB)
    } else if bytes_per_sec >= KB {
        format!("{:.2} KB/s", bytes_per_sec / KB)
    } else {
        format!("{:.2} B/s", bytes_per_sec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::RunReport;
    use std::collections::HashMap;

    fn sample_report() -> RunReport {
        RunReport {
            proto: "h1".to_string(),
            url: "https://example.test/".to_string(),
            h3_backend: None,
            clients: 1,
            threads: 1,
            max_streams: 1,
            mode: "count".to_string(),
            tls_protocol: Some("TLSv1.3".to_string()),
            tls_cipher: Some("TLS_AES_256_GCM_SHA384".to_string()),
            duration_s: 0.0,
            metrics_sample: 1,
            requests_target: 1,
            rps_target: Some(100.0),
            requests_started: 1,
            requests_completed: 1,
            ok: 1,
            err_total: 0,
            status_counts: HashMap::from([(200u16, 1u64)]),
            status_2xx: 1,
            status_3xx: 0,
            status_4xx: 0,
            status_5xx: 0,
            rps: 95.0,
            started_rps: 100.0,
            rps_target_achieved_pct: Some(100.0),
            bytes_in: 1,
            bytes_out: 1,
            mbps_in: 0.0,
            mbps_out: 0.0,
            latency_min_us: 1,
            latency_p50_us: 1,
            latency_p90_us: 1,
            latency_p99_us: 1,
            latency_mean_us: 1.0,
            latency_max_us: 1,
            latency_stdev_us: 0.0,
            ttfb_min_us: 1,
            ttfb_p50_us: 1,
            ttfb_p90_us: 1,
            ttfb_p99_us: 1,
            ttfb_mean_us: 1.0,
            ttfb_max_us: 1,
            ttfb_stdev_us: 0.0,
            connect_min_us: 1,
            connect_p50_us: 1,
            connect_p90_us: 1,
            connect_p99_us: 1,
            connect_mean_us: 1.0,
            connect_max_us: 1,
            connect_stdev_us: 0.0,
            connect_v4_count: 1,
            connect_v6_count: 0,
            connect_addr_counts: HashMap::from([("127.0.0.1:443".to_string(), 1u64)]),
            err_connect: 0,
            err_tls: 0,
            err_timeout: 0,
            err_http: 0,
            elapsed_s: 1.0,
            latency_hist_b64: None,
            ttfb_hist_b64: None,
            connect_hist_b64: None,
        }
    }

    #[test]
    fn csv_contains_new_rps_columns_at_end() {
        let csv = format_csv(&sample_report()).expect("format_csv should succeed");
        let mut lines = csv.lines();
        let header = lines.next().expect("header line should exist");
        let data = lines.next().expect("data line should exist");
        assert!(header.ends_with(
            "elapsed_s,started_rps,rps_target_achieved_pct,connect_v4_count,connect_v6_count,connect_addrs"
        ));
        assert!(data.ends_with(",100.00,100.00,1,0,127.0.0.1:443=1"));
    }
}
