mod cli;
mod driver;
mod metrics;
mod output;
mod runner;
mod tls;

use std::sync::Arc;

use anyhow::{Context, Result};
use bytes::Bytes;
use clap::Parser;

use cli::{Cli, Protocol};
use driver::{ConnectionFactory, RequestConfig};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(false)
        .init();

    let protocol = cli.protocol();
    let url: http::Uri = cli.url.parse().context("Invalid URL")?;

    // Validate URL scheme
    match url.scheme_str() {
        Some("http") | Some("https") => {}
        Some(s) => anyhow::bail!("Unsupported URL scheme: {s}"),
        None => anyhow::bail!("URL must have a scheme (http:// or https://)"),
    }

    // Use a lightweight control runtime; worker loops run on dedicated
    // current-thread runtimes in `runner`.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("Failed to build tokio runtime")?;

    runtime.block_on(async_main(cli, protocol, url))
}

async fn async_main(cli: Cli, protocol: Protocol, url: http::Uri) -> Result<()> {
    maybe_warn_container_localhost_target(&url);
    maybe_warn_tcp_quickack(protocol, cli.tcp_quickack);

    // Parse request config
    let method: http::Method = cli.method.parse().context("Invalid HTTP method")?;
    let headers = cli.parsed_headers()?;
    let body = cli.request_body()?.map(Bytes::from);

    let request_config = Arc::new(RequestConfig {
        url: url.clone(),
        method,
        headers,
        body,
        request_timeout: cli.request_timeout,
        tail_friendly: cli.tail_friendly,
    });

    // Build connection factory based on protocol
    let factory = build_factory(&cli, protocol, &url)?;
    let factory = Arc::new(factory);

    // Determine mode
    let (total_requests, duration) = if cli.is_duration_mode() {
        (u64::MAX, cli.duration) // unlimited requests in duration mode
    } else {
        (cli.requests, None)
    };

    let mode_str = if cli.is_duration_mode() {
        "duration"
    } else {
        "count"
    };
    let duration_s = cli.duration.map(|d| d.as_secs_f64()).unwrap_or(0.0);

    tracing::info!(
        "Starting benchmark: proto={protocol}, clients={}, threads={}, max_streams={}, mode={mode_str}",
        cli.clients,
        cli.threads,
        cli.max_streams,
    );

    // Run the benchmark
    let (worker_metrics, elapsed) = runner::run_benchmark(
        factory,
        request_config,
        cli.clients,
        cli.max_streams,
        cli.threads,
        cli.tail_friendly,
        cli.metrics_sample,
        total_requests,
        duration,
        cli.warm_up_time,
        cli.ramp_up_time,
        cli.rps,
    );

    // Merge and report
    let report = metrics::merge_metrics(
        worker_metrics,
        elapsed,
        &protocol.to_string(),
        &cli.url,
        (protocol == Protocol::H3).then_some("quinn"),
        cli.clients,
        cli.threads,
        cli.max_streams,
        mode_str,
        duration_s,
        cli.metrics_sample,
        cli.requests,
        cli.rps.map(|per_client| per_client * cli.clients as f64),
        false,
    );

    // Print human-readable summary unless disabled.
    if !cli.no_human {
        output::print_summary(&report);
    }

    // Write machine-readable output
    output::write_report(&report, cli.format, cli.output.as_deref())?;

    Ok(())
}

fn maybe_warn_container_localhost_target(url: &http::Uri) {
    let host = match url.host() {
        Some(host) => host,
        None => return,
    };

    let is_local_target = matches!(host, "localhost" | "127.0.0.1" | "::1");
    if !is_local_target || !running_in_container() || !likely_isolated_container_network() {
        return;
    }

    tracing::warn!(
        "Target host '{}' is loopback inside a container with likely isolated networking. If you intended the host machine, use --network host or a non-loopback host/IP.",
        host
    );
}

fn running_in_container() -> bool {
    std::env::var_os("container").is_some()
        || std::path::Path::new("/.dockerenv").exists()
        || std::path::Path::new("/run/.containerenv").exists()
}

/// Best-effort heuristic:
/// isolated container networking usually exposes only `lo` and one veth-style interface.
/// host networking typically exposes additional host interfaces.
fn likely_isolated_container_network() -> bool {
    let entries = match std::fs::read_dir("/sys/class/net") {
        Ok(entries) => entries,
        Err(_) => return false,
    };

    let mut non_loopback = 0usize;
    for entry in entries.flatten() {
        if let Some(name) = entry.file_name().to_str() {
            if name != "lo" {
                non_loopback += 1;
            }
        }
    }

    non_loopback <= 1
}

fn build_factory(cli: &Cli, protocol: Protocol, url: &http::Uri) -> Result<ConnectionFactory> {
    let forced_ip_version = if cli.v4 {
        Some(driver::IpVersion::V4)
    } else if cli.v6 {
        Some(driver::IpVersion::V6)
    } else {
        None
    };

    match protocol {
        Protocol::H1 => {
            let tls_config = tls::build_rustls_config(
                Protocol::H1,
                cli.insecure,
                cli.tls_ciphers.as_deref(),
                cli.tls_ca.as_deref(),
            )?;
            Ok(ConnectionFactory::H1Raw(
                driver::h1_raw::H1RawFactory::from_url(
                    url,
                    tls_config,
                    cli.connect_timeout,
                    cli.tcp_quickack,
                    forced_ip_version,
                )?,
            ))
        }
        Protocol::H2 => {
            let tls_config = tls::build_rustls_config(
                Protocol::H2,
                cli.insecure,
                cli.tls_ciphers.as_deref(),
                cli.tls_ca.as_deref(),
            )?;
            Ok(ConnectionFactory::H2(driver::h2::H2Factory::from_url(
                url,
                tls_config,
                cli.connect_timeout,
                cli.tcp_quickack,
                forced_ip_version,
            )?))
        }
        Protocol::H3 => {
            let factory = driver::h3::H3Factory::from_url(
                url,
                cli.connect_timeout,
                cli.insecure,
                cli.tls_ciphers.as_deref(),
                cli.tls_ca.as_deref(),
                forced_ip_version,
            )?;
            Ok(ConnectionFactory::H3(factory))
        }
    }
}

fn maybe_warn_tcp_quickack(protocol: Protocol, enabled: bool) {
    if !enabled {
        return;
    }

    if protocol == Protocol::H3 {
        tracing::warn!("--tcp-quickack applies only to TCP (H1/H2); ignored for h3");
    }

    #[cfg(not(target_os = "linux"))]
    tracing::warn!("--tcp-quickack is currently only supported on Linux");
}
