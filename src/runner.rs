use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Barrier;
use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures_util::stream::{FuturesUnordered, StreamExt};

use crate::driver::{Connection, ConnectionFactory, RequestConfig, RequestError, RequestResult};
use crate::metrics::{ErrorClass, WorkerMetrics};

static FIRST_REQUEST_ERROR_LOGGED: Once = Once::new();
const IDLE_SLEEP: Duration = Duration::from_millis(1);
const RPS_PACER_MAX_TOKENS: f64 = 2.0;

#[derive(Debug, Clone)]
struct RpsPacer {
    rate: f64,
    tokens: f64,
    last_refill: Instant,
    max_tokens: f64,
}

impl RpsPacer {
    fn new(rate: f64, max_tokens: f64, now: Instant) -> Self {
        Self {
            rate,
            tokens: 0.0,
            last_refill: now,
            max_tokens,
        }
    }

    fn activate(&mut self, now: Instant) {
        self.last_refill = now;
        self.tokens = 0.0;
    }

    fn refill(&mut self, now: Instant) {
        if self.rate <= 0.0 {
            return;
        }
        let dt = now
            .saturating_duration_since(self.last_refill)
            .as_secs_f64();
        self.last_refill = now;
        self.tokens = (self.tokens + dt * self.rate).min(self.max_tokens);
    }

    fn try_acquire(&mut self, now: Instant) -> bool {
        if self.rate <= 0.0 {
            return true;
        }
        self.refill(now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    fn time_until_next_token(&mut self, now: Instant) -> Duration {
        if self.rate <= 0.0 {
            return Duration::ZERO;
        }
        self.refill(now);
        if self.tokens >= 1.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64((1.0 - self.tokens) / self.rate)
        }
    }
}

fn maybe_activate_pacer(
    pacer: &mut Option<RpsPacer>,
    pacer_active: &mut bool,
    measuring: bool,
    now: Instant,
) {
    if !measuring || *pacer_active {
        return;
    }
    if let Some(p) = pacer.as_mut() {
        p.activate(now);
        *pacer_active = true;
    }
}

/// Run the benchmark with one current-thread tokio runtime per worker thread.
/// Returns per-worker metrics and the measured elapsed time.
#[allow(clippy::too_many_arguments)]
pub fn run_benchmark(
    factory: Arc<ConnectionFactory>,
    request_config: Arc<RequestConfig>,
    clients: usize,
    max_streams: u32,
    worker_threads: usize,
    tail_friendly: bool,
    metrics_sample: u32,
    total_requests: u64,
    duration: Option<Duration>,
    warm_up_time: Duration,
    ramp_up_time: Duration,
    rps_limit: Option<f64>,
) -> (Vec<WorkerMetrics>, Duration) {
    if clients == 0 {
        return (Vec::new(), Duration::ZERO);
    }

    let workers = worker_threads.max(1).min(clients);
    let is_duration_mode = duration.is_some();
    let measure_starts_in = if is_duration_mode {
        warm_up_time.saturating_add(ramp_up_time)
    } else {
        Duration::ZERO
    };
    let ramp_for_lanes = if is_duration_mode {
        ramp_up_time
    } else {
        Duration::ZERO
    };
    let lanes_per_client = if tail_friendly {
        1usize
    } else if factory.supports_multiplexed_lanes() {
        max_streams.max(1) as usize
    } else {
        1usize
    };
    let total_lanes = clients.saturating_mul(lanes_per_client).max(1);

    if is_duration_mode && measure_starts_in > Duration::ZERO {
        tracing::info!(
            "Sharded mode warm-up: warm_up={:.3}s ramp_up={:.3}s before {:.3}s measurement",
            warm_up_time.as_secs_f64(),
            ramp_up_time.as_secs_f64(),
            duration.map(|d| d.as_secs_f64()).unwrap_or(0.0),
        );
    }

    let worker_clients = split_u64_even(clients as u64, workers)
        .into_iter()
        .map(|v| v as usize)
        .collect::<Vec<_>>();
    let worker_budgets = if is_duration_mode {
        vec![0u64; workers]
    } else {
        split_u64_even(total_requests, workers)
    };

    let start_barrier = Arc::new(Barrier::new(workers.saturating_add(1)));
    let mut handles = Vec::with_capacity(workers);
    let mut lane_base = 0usize;

    for worker_id in 0..workers {
        let factory = factory.clone();
        let config = request_config.clone();
        let start_barrier = start_barrier.clone();
        let worker_clients = worker_clients[worker_id];
        let worker_budget = worker_budgets[worker_id];
        let worker_lane_base = lane_base;
        lane_base = lane_base.saturating_add(worker_clients.saturating_mul(lanes_per_client));

        let handle = std::thread::Builder::new()
            .name(format!("loadgen-worker-{worker_id}"))
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build worker current-thread runtime");
                rt.block_on(run_worker_eventloop(
                    factory,
                    config,
                    worker_clients,
                    lanes_per_client,
                    metrics_sample,
                    tail_friendly,
                    is_duration_mode,
                    worker_budget,
                    duration,
                    ramp_for_lanes,
                    measure_starts_in,
                    worker_lane_base,
                    total_lanes,
                    rps_limit,
                    start_barrier,
                ))
            })
            .expect("failed to spawn worker thread");
        handles.push(handle);
    }

    start_barrier.wait();
    let started = Instant::now();

    let mut all_metrics = Vec::with_capacity(workers);
    for handle in handles {
        match handle.join() {
            Ok(metrics) => all_metrics.push(metrics),
            Err(_) => tracing::error!("Worker thread panicked"),
        }
    }

    let total_elapsed = started.elapsed();
    let measured_elapsed = if is_duration_mode {
        total_elapsed.saturating_sub(measure_starts_in)
    } else {
        total_elapsed
    };
    (all_metrics, measured_elapsed)
}

#[allow(clippy::too_many_arguments)]
async fn run_worker_eventloop(
    factory: Arc<ConnectionFactory>,
    config: Arc<RequestConfig>,
    worker_clients: usize,
    lanes_per_client: usize,
    metrics_sample: u32,
    tail_friendly: bool,
    is_duration_mode: bool,
    worker_budget: u64,
    duration: Option<Duration>,
    ramp_up_time: Duration,
    measure_starts_in: Duration,
    worker_lane_base: usize,
    total_lanes: usize,
    rps_per_client: Option<f64>,
    start_barrier: Arc<Barrier>,
) -> WorkerMetrics {
    let mut worker_metrics = WorkerMetrics::new();
    let is_h3 = matches!(factory.as_ref(), ConnectionFactory::H3(_));

    if is_h3 {
        let mut conns: Vec<Option<Connection>> = Vec::with_capacity(worker_clients);
        for _ in 0..worker_clients {
            conns.push(timed_connect(&factory, &config, &mut worker_metrics).await);
        }

        start_barrier.wait();

        if conns.is_empty() {
            return worker_metrics;
        }

        return run_worker_loop_h3(
            factory,
            config,
            conns,
            worker_metrics,
            lanes_per_client.max(1),
            metrics_sample,
            tail_friendly,
            is_duration_mode,
            worker_budget,
            duration,
            ramp_up_time,
            measure_starts_in,
            worker_lane_base,
            total_lanes,
            rps_per_client,
        )
        .await;
    }

    let mut conns: Vec<Option<Connection>> =
        Vec::with_capacity(worker_clients.saturating_mul(lanes_per_client));

    for _ in 0..worker_clients {
        match timed_connect(&factory, &config, &mut worker_metrics).await {
            Some(conn) => {
                conns.push(Some(conn));
                for _ in 1..lanes_per_client {
                    let lane_conn = conns
                        .last()
                        .and_then(|slot| slot.as_ref())
                        .and_then(Connection::clone_stream_handle);
                    conns.push(lane_conn);
                }
            }
            None => {
                for _ in 0..lanes_per_client {
                    conns.push(None);
                }
            }
        }
    }

    start_barrier.wait();

    if conns.is_empty() {
        return worker_metrics;
    }

    run_worker_loop(
        factory,
        config,
        conns,
        worker_metrics,
        metrics_sample,
        tail_friendly,
        is_duration_mode,
        worker_budget,
        duration,
        ramp_up_time,
        measure_starts_in,
        worker_lane_base,
        total_lanes,
        lanes_per_client,
        rps_per_client,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_worker_loop_h3(
    factory: Arc<ConnectionFactory>,
    config: Arc<RequestConfig>,
    conns: Vec<Option<Connection>>,
    mut worker_metrics: WorkerMetrics,
    max_streams_per_client: usize,
    metrics_sample: u32,
    tail_friendly: bool,
    is_duration_mode: bool,
    worker_budget: u64,
    duration: Option<Duration>,
    ramp_up_time: Duration,
    measure_starts_in: Duration,
    worker_lane_base: usize,
    total_lanes: usize,
    rps_per_client: Option<f64>,
) -> WorkerMetrics {
    let started_at = Instant::now();
    let run_deadline = duration.map(|d| measure_starts_in.saturating_add(d));
    // For H3 use client-based ramp-up offsets (h2load-like), not per-stream.
    let total_clients = (total_lanes / max_streams_per_client.max(1)).max(1);
    let worker_client_base = worker_lane_base / max_streams_per_client.max(1);
    let budget = Arc::new(AtomicU64::new(worker_budget));

    let mut actors = FuturesUnordered::new();
    for (client_idx, conn) in conns.into_iter().enumerate() {
        let factory = factory.clone();
        let config = config.clone();
        let budget = budget.clone();
        let start_offset = scale_duration(
            ramp_up_time,
            worker_client_base.saturating_add(client_idx),
            total_clients,
        );
        actors.push(run_h3_client_actor(
            client_idx,
            factory,
            config,
            conn,
            max_streams_per_client,
            metrics_sample,
            tail_friendly,
            is_duration_mode,
            measure_starts_in,
            run_deadline,
            start_offset,
            rps_per_client,
            budget,
            started_at,
        ));
    }

    while let Some(actor_metrics) = actors.next().await {
        merge_worker_metrics(&mut worker_metrics, actor_metrics);
    }

    worker_metrics
}

#[allow(clippy::too_many_arguments)]
async fn run_h3_client_actor(
    client_idx: usize,
    factory: Arc<ConnectionFactory>,
    config: Arc<RequestConfig>,
    mut conn: Option<Connection>,
    max_streams_per_client: usize,
    metrics_sample: u32,
    tail_friendly: bool,
    is_duration_mode: bool,
    measure_starts_in: Duration,
    run_deadline: Option<Duration>,
    start_offset: Duration,
    rps_per_client: Option<f64>,
    budget: Arc<AtomicU64>,
    started_at: Instant,
) -> WorkerMetrics {
    let mut metrics = WorkerMetrics::new();
    let sample = metrics_sample as u64;
    let mut ok_seq: u64 = 0;
    let mut retry_at = Instant::now();
    let mut inflight = FuturesUnordered::new();
    let mut rps_pacer =
        rps_per_client.map(|rate| RpsPacer::new(rate, RPS_PACER_MAX_TOKENS, Instant::now()));
    let mut rps_pacer_active = false;

    loop {
        let loop_now = Instant::now();
        let elapsed = loop_now.duration_since(started_at);
        if let Some(deadline) = run_deadline
            && elapsed >= deadline
        {
            break;
        }
        let mut rps_blocked = false;

        // Immediate top-up to m streams per client (h2load-like behavior).
        while inflight.len() < max_streams_per_client {
            let now = Instant::now();
            let elapsed = now.duration_since(started_at);
            if let Some(deadline) = run_deadline
                && elapsed >= deadline
            {
                break;
            }
            if elapsed < start_offset {
                break;
            }
            if now < retry_at {
                break;
            }

            let measuring = elapsed >= measure_starts_in;
            maybe_activate_pacer(&mut rps_pacer, &mut rps_pacer_active, measuring, now);

            if conn.is_none() {
                let connect_start = Instant::now();
                match factory.create_connection().await {
                    Ok(mut c) => {
                        c.prepare_request_template(&config);
                        let connect_us = connect_start.elapsed().as_micros() as u64;
                        metrics.record_connect(connect_us, Some(c.remote_addr()));
                        if let Some(info) = c.tls_info() {
                            metrics
                                .record_tls_info(info.protocol.as_deref(), info.cipher.as_deref());
                        }
                        conn = Some(c);
                    }
                    Err(e) => {
                        if measuring {
                            handle_request_error(&mut metrics, &e);
                        } else {
                            tracing::debug!("Warm-up request error: {e}");
                        }
                        retry_at = now + Duration::from_millis(50);
                        break;
                    }
                }
            }

            if measuring && !is_duration_mode && budget.load(Ordering::Relaxed) == 0 {
                break;
            }
            if measuring
                && let Some(pacer) = rps_pacer.as_mut()
                && !pacer.try_acquire(now)
            {
                rps_blocked = true;
                break;
            }
            if measuring && !is_duration_mode && !try_take_budget(&budget) {
                break;
            }
            if measuring {
                metrics.requests_started += 1;
            }

            let lane_conn = if conn.is_some() {
                conn.as_ref()
                    .and_then(Connection::clone_stream_handle)
                    .or_else(|| conn.take())
            } else {
                None
            };
            let lane_conn = if let Some(c) = lane_conn { c } else { break };

            inflight.push(run_h3_lane_once(
                client_idx,
                config.clone(),
                lane_conn,
                measuring,
            ));

            if tail_friendly {
                break;
            }
        }

        if inflight.is_empty() {
            let now = Instant::now();
            let elapsed = now.duration_since(started_at);
            let measuring = elapsed >= measure_starts_in;
            let budget_empty =
                !is_duration_mode && measuring && budget.load(Ordering::Relaxed) == 0;
            if budget_empty {
                break;
            }
            if rps_blocked {
                if let Some(pacer) = rps_pacer.as_mut() {
                    tokio::time::sleep(pacer.time_until_next_token(Instant::now())).await;
                } else {
                    tokio::time::sleep(IDLE_SLEEP).await;
                }
            } else {
                tokio::time::sleep(IDLE_SLEEP).await;
            }
            continue;
        }

        let done = if let Some(deadline) = run_deadline {
            let now = Instant::now();
            let elapsed = now.duration_since(started_at);
            let remaining = deadline.saturating_sub(elapsed);
            if remaining == Duration::ZERO {
                break;
            }
            match tokio::time::timeout(remaining, inflight.next()).await {
                Ok(Some(done)) => done,
                Ok(None) => continue,
                Err(_) => break,
            }
        } else {
            inflight
                .next()
                .await
                .expect("inflight actor future should exist")
        };

        if let Some(connect_us) = done.connect_us {
            metrics.record_connect(connect_us, done.connect_addr);
        }
        metrics.record_tls_info(done.tls_protocol.as_deref(), done.tls_cipher.as_deref());
        let done_now = Instant::now();

        match done.result {
            Ok(result) => {
                if conn.is_none() {
                    conn = done.conn;
                }
                if done.measuring {
                    let record_timing = if sample <= 1 {
                        true
                    } else {
                        ok_seq = ok_seq.wrapping_add(1);
                        ok_seq.is_multiple_of(sample)
                    };
                    metrics.record_success(
                        result.status,
                        result.latency_us,
                        result.ttfb_us,
                        result.bytes_in,
                        result.bytes_out,
                        record_timing,
                    );
                }
            }
            Err(e) => {
                if done.measuring {
                    handle_request_error(&mut metrics, &e);
                } else {
                    tracing::debug!("Warm-up request error: {e}");
                }
                conn = None;
                if matches!(e.class, ErrorClass::Connect | ErrorClass::Tls) {
                    retry_at = done_now + Duration::from_millis(50);
                }
            }
        }

        if tail_friendly {
            tokio::task::yield_now().await;
        }
    }

    metrics
}

async fn run_h3_lane_once(
    lane_idx: usize,
    config: Arc<RequestConfig>,
    mut conn: Connection,
    measuring: bool,
) -> LaneCompletion {
    let result = conn.send_request(&config).await;
    if result.is_err() {
        return LaneCompletion {
            lane_idx,
            conn: None,
            result,
            measuring,
            connect_us: None,
            connect_addr: None,
            tls_protocol: None,
            tls_cipher: None,
        };
    }
    LaneCompletion {
        lane_idx,
        conn: Some(conn),
        result,
        measuring,
        connect_us: None,
        connect_addr: None,
        tls_protocol: None,
        tls_cipher: None,
    }
}

fn try_take_budget(budget: &AtomicU64) -> bool {
    budget
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
            if v > 0 { Some(v - 1) } else { None }
        })
        .is_ok()
}

fn merge_worker_metrics(into: &mut WorkerMetrics, mut from: WorkerMetrics) {
    let _ = into.latency_hist.add(&from.latency_hist);
    let _ = into.ttfb_hist.add(&from.ttfb_hist);
    let _ = into.connect_hist.add(&from.connect_hist);
    into.requests_started += from.requests_started;
    into.requests_completed += from.requests_completed;
    into.ok += from.ok;
    into.err_connect += from.err_connect;
    into.err_tls += from.err_tls;
    into.err_timeout += from.err_timeout;
    into.err_http += from.err_http;
    into.bytes_in += from.bytes_in;
    into.bytes_out += from.bytes_out;
    into.connect_v4_count += from.connect_v4_count;
    into.connect_v6_count += from.connect_v6_count;
    for (dst, src) in into
        .status_counts_fast
        .iter_mut()
        .zip(from.status_counts_fast.iter())
    {
        *dst += *src;
    }
    for (status, count) in from.status_counts_other.drain() {
        *into.status_counts_other.entry(status).or_insert(0) += count;
    }
    for (addr, count) in from.connect_addr_counts.drain() {
        *into.connect_addr_counts.entry(addr).or_insert(0) += count;
    }
    if into.tls_protocol.is_none() {
        into.tls_protocol = from.tls_protocol.take();
    }
    if into.tls_cipher.is_none() {
        into.tls_cipher = from.tls_cipher.take();
    }
}

struct LaneCompletion {
    lane_idx: usize,
    conn: Option<Connection>,
    result: Result<RequestResult, RequestError>,
    measuring: bool,
    connect_us: Option<u64>,
    connect_addr: Option<SocketAddr>,
    tls_protocol: Option<String>,
    tls_cipher: Option<String>,
}

async fn run_lane_once(
    lane_idx: usize,
    factory: Arc<ConnectionFactory>,
    config: Arc<RequestConfig>,
    measuring: bool,
    mut conn: Option<Connection>,
) -> LaneCompletion {
    let mut connect_us = None;
    let mut connect_addr = None;
    let mut tls_protocol = None;
    let mut tls_cipher = None;

    if conn.is_none() {
        let connect_start = Instant::now();
        match factory.create_connection().await {
            Ok(mut c) => {
                c.prepare_request_template(&config);
                connect_us = Some(connect_start.elapsed().as_micros() as u64);
                connect_addr = Some(c.remote_addr());
                if let Some(info) = c.tls_info() {
                    tls_protocol = info.protocol.clone();
                    tls_cipher = info.cipher.clone();
                }
                conn = Some(c);
            }
            Err(e) => {
                return LaneCompletion {
                    lane_idx,
                    conn: None,
                    result: Err(e),
                    measuring,
                    connect_us,
                    connect_addr,
                    tls_protocol,
                    tls_cipher,
                };
            }
        }
    }

    let result = conn
        .as_mut()
        .expect("lane connection should be available")
        .send_request(&config)
        .await;

    if result.is_err() {
        conn = None;
    }

    LaneCompletion {
        lane_idx,
        conn,
        result,
        measuring,
        connect_us,
        connect_addr,
        tls_protocol,
        tls_cipher,
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_worker_loop(
    factory: Arc<ConnectionFactory>,
    config: Arc<RequestConfig>,
    mut conns: Vec<Option<Connection>>,
    mut worker_metrics: WorkerMetrics,
    metrics_sample: u32,
    tail_friendly: bool,
    is_duration_mode: bool,
    worker_budget: u64,
    duration: Option<Duration>,
    ramp_up_time: Duration,
    measure_starts_in: Duration,
    worker_lane_base: usize,
    total_lanes: usize,
    lanes_per_client: usize,
    rps_per_client: Option<f64>,
) -> WorkerMetrics {
    let lane_count = conns.len();
    let mut lane_busy = vec![false; lane_count];
    let mut lane_retry_at = vec![Instant::now(); lane_count];
    let lanes_per_client = lanes_per_client.max(1);
    let worker_clients = lane_count.div_ceil(lanes_per_client);
    let mut rps_pacers = rps_per_client.map(|rate| {
        (0..worker_clients)
            .map(|_| RpsPacer::new(rate, RPS_PACER_MAX_TOKENS, Instant::now()))
            .collect::<Vec<_>>()
    });
    let mut rps_pacer_active = vec![false; worker_clients];
    let lane_start_offsets: Vec<Duration> = (0..lane_count)
        .map(|idx| {
            scale_duration(
                ramp_up_time,
                worker_lane_base.saturating_add(idx),
                total_lanes,
            )
        })
        .collect();
    let started_at = Instant::now();
    let run_deadline = duration.map(|d| measure_starts_in.saturating_add(d));

    let sample = metrics_sample as u64;
    let mut ok_seq: u64 = 0;
    let mut inflight = FuturesUnordered::new();
    let mut lane_cursor = 0usize;
    let mut budget_remaining = worker_budget;
    let mut budget_exhausted = !is_duration_mode && budget_remaining == 0;

    loop {
        let now = Instant::now();
        let elapsed = now.duration_since(started_at);
        if let Some(deadline) = run_deadline
            && elapsed >= deadline
        {
            // h2load-style hard stop for timed runs.
            break;
        }

        let mut scheduled_any = false;
        let mut rps_blocked_client = None;
        for _ in 0..lane_count {
            let lane_idx = lane_cursor;
            lane_cursor = (lane_cursor + 1) % lane_count;
            let client_idx = lane_idx / lanes_per_client;

            if lane_busy[lane_idx] {
                continue;
            }
            if elapsed < lane_start_offsets[lane_idx] {
                continue;
            }
            if now < lane_retry_at[lane_idx] {
                continue;
            }

            let measuring = elapsed >= measure_starts_in;
            maybe_activate_client_pacer(
                &mut rps_pacers,
                &mut rps_pacer_active,
                client_idx,
                measuring,
                now,
            );
            if measuring && !is_duration_mode && budget_remaining == 0 {
                budget_exhausted = true;
                continue;
            }

            if measuring
                && let Some(pacers) = rps_pacers.as_mut()
                && let Some(pacer) = pacers.get_mut(client_idx)
                && !pacer.try_acquire(now)
            {
                rps_blocked_client = Some(client_idx);
                break;
            }

            if measuring && !is_duration_mode {
                budget_remaining = budget_remaining.saturating_sub(1);
            }
            if measuring {
                worker_metrics.requests_started += 1;
            }
            lane_busy[lane_idx] = true;
            let lane_conn = conns[lane_idx].take();
            inflight.push(run_lane_once(
                lane_idx,
                factory.clone(),
                config.clone(),
                measuring,
                lane_conn,
            ));
            scheduled_any = true;
            if tail_friendly {
                break;
            }
        }

        if !is_duration_mode && budget_exhausted && inflight.is_empty() {
            break;
        }

        if inflight.is_empty() {
            if !scheduled_any {
                if let Some(client_idx) = rps_blocked_client {
                    if let Some(pacers) = rps_pacers.as_mut() {
                        let pacer = pacers.get_mut(client_idx).expect("valid client pacer");
                        tokio::time::sleep(pacer.time_until_next_token(Instant::now())).await;
                    } else {
                        tokio::time::sleep(IDLE_SLEEP).await;
                    }
                } else {
                    tokio::time::sleep(IDLE_SLEEP).await;
                }
            }
            continue;
        }

        let done = if let Some(deadline) = run_deadline {
            let now = Instant::now();
            let elapsed = now.duration_since(started_at);
            let remaining = deadline.saturating_sub(elapsed);
            if remaining == Duration::ZERO {
                break;
            }
            match tokio::time::timeout(remaining, inflight.next()).await {
                Ok(Some(done)) => done,
                Ok(None) => continue,
                Err(_) => break,
            }
        } else {
            inflight
                .next()
                .await
                .expect("inflight lane future should exist")
        };

        lane_busy[done.lane_idx] = false;
        if let Some(connect_us) = done.connect_us {
            worker_metrics.record_connect(connect_us, done.connect_addr);
        }
        worker_metrics.record_tls_info(done.tls_protocol.as_deref(), done.tls_cipher.as_deref());
        let done_now = Instant::now();
        conns[done.lane_idx] = done.conn;

        match done.result {
            Ok(result) => {
                if done.measuring {
                    let record_timing = if sample <= 1 {
                        true
                    } else {
                        ok_seq = ok_seq.wrapping_add(1);
                        ok_seq.is_multiple_of(sample)
                    };
                    worker_metrics.record_success(
                        result.status,
                        result.latency_us,
                        result.ttfb_us,
                        result.bytes_in,
                        result.bytes_out,
                        record_timing,
                    );
                }
            }
            Err(e) => {
                if done.measuring {
                    handle_request_error(&mut worker_metrics, &e);
                } else {
                    tracing::debug!("Warm-up request error: {e}");
                }
                conns[done.lane_idx] = None;
                if matches!(e.class, ErrorClass::Connect | ErrorClass::Tls) {
                    lane_retry_at[done.lane_idx] = done_now + Duration::from_millis(50);
                }
            }
        }

        if tail_friendly {
            tokio::task::yield_now().await;
        }
    }

    worker_metrics
}

fn handle_request_error(metrics: &mut WorkerMetrics, e: &RequestError) {
    tracing::debug!("Request error: {e}");
    metrics.record_error(e.class);
    FIRST_REQUEST_ERROR_LOGGED.call_once(|| {
        tracing::warn!(
            "First request error observed: class={:?}, details={}",
            e.class,
            e
        );
    });
}

/// Create a connection and record the connect time in metrics.
async fn timed_connect(
    factory: &ConnectionFactory,
    config: &RequestConfig,
    metrics: &mut WorkerMetrics,
) -> Option<Connection> {
    let connect_start = Instant::now();
    match factory.create_connection().await {
        Ok(conn) => {
            let connect_us = connect_start.elapsed().as_micros() as u64;
            metrics.record_connect(connect_us, Some(conn.remote_addr()));
            let mut conn = conn;
            conn.prepare_request_template(config);
            if let Some(info) = conn.tls_info() {
                metrics.record_tls_info(info.protocol.as_deref(), info.cipher.as_deref());
            }
            Some(conn)
        }
        Err(e) => {
            tracing::error!("Failed to create connection: {e}");
            metrics.record_error(e.class);
            None
        }
    }
}

fn scale_duration(total: Duration, index: usize, count: usize) -> Duration {
    if total == Duration::ZERO || count <= 1 || index == 0 {
        return Duration::ZERO;
    }
    let denominator = (count - 1) as u128;
    let numerator = index.min(count - 1) as u128;
    let nanos = total.as_nanos().saturating_mul(numerator) / denominator;
    Duration::from_nanos(nanos.min(u64::MAX as u128) as u64)
}

fn split_u64_even(total: u64, parts: usize) -> Vec<u64> {
    if parts == 0 {
        return Vec::new();
    }
    let base = total / parts as u64;
    let rem = total % parts as u64;
    (0..parts)
        .map(|i| base + u64::from((i as u64) < rem))
        .collect()
}

fn maybe_activate_client_pacer(
    pacers: &mut Option<Vec<RpsPacer>>,
    pacer_active: &mut [bool],
    client_idx: usize,
    measuring: bool,
    now: Instant,
) {
    if !measuring {
        return;
    }
    if pacer_active.get(client_idx).copied().unwrap_or(true) {
        return;
    }
    let Some(pacers) = pacers.as_mut() else {
        return;
    };
    let Some(pacer) = pacers.get_mut(client_idx) else {
        return;
    };
    pacer.activate(now);
    if let Some(active) = pacer_active.get_mut(client_idx) {
        *active = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rps_pacer_no_token_before_interval_then_token_available() {
        let base = Instant::now();
        let mut pacer = RpsPacer::new(10.0, 2.0, base);
        pacer.activate(base);

        assert!(!pacer.try_acquire(base));
        assert!(!pacer.try_acquire(base + Duration::from_millis(99)));
        assert!(pacer.try_acquire(base + Duration::from_millis(100)));
    }

    #[test]
    fn rps_pacer_respects_burst_cap() {
        let base = Instant::now();
        let mut pacer = RpsPacer::new(1000.0, 2.0, base);
        pacer.activate(base);
        pacer.refill(base + Duration::from_secs(10));

        assert!((pacer.tokens - 2.0).abs() < 1e-9);
        assert!(pacer.try_acquire(base + Duration::from_secs(10)));
        assert!(pacer.try_acquire(base + Duration::from_secs(10)));
        assert!(!pacer.try_acquire(base + Duration::from_secs(10)));
    }

    #[test]
    fn rps_pacer_time_until_next_token_shrinks() {
        let base = Instant::now();
        let mut pacer = RpsPacer::new(100.0, 2.0, base);
        pacer.activate(base);

        let wait_1 = pacer.time_until_next_token(base + Duration::from_millis(1));
        let wait_2 = pacer.time_until_next_token(base + Duration::from_millis(5));
        assert!(wait_2 < wait_1);
    }

    #[test]
    fn pacer_activation_is_measurement_gated() {
        let base = Instant::now();
        let mut pacer = Some(RpsPacer::new(100.0, 2.0, base));
        let mut active = false;

        maybe_activate_pacer(
            &mut pacer,
            &mut active,
            false,
            base + Duration::from_millis(50),
        );
        assert!(!active);
        assert_eq!(pacer.as_ref().map(|p| p.tokens), Some(0.0));

        maybe_activate_pacer(
            &mut pacer,
            &mut active,
            true,
            base + Duration::from_millis(50),
        );
        assert!(active);
        assert_eq!(pacer.as_ref().map(|p| p.tokens), Some(0.0));
    }
}
