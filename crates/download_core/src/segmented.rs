use crate::{
    ByteRange, Download, Engine, Error, Result, ResumeReason, Status, WorkerPhase,
    adaptive::Adaptive,
    connections,
    engine::Stop,
    ranges,
    telemetry::{RateSample, WorkerStats},
    transfer,
};
use reqwest::{StatusCode, header};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncSeekExt, AsyncWriteExt},
    sync::watch,
    task::JoinSet,
};

const MIN_RANGE: u64 = 2 * 1024 * 1024;

fn identity(d: &Download, response: &reqwest::Response, total: u64) -> Result<()> {
    if d.total != Some(total)
        || transfer::text(response, header::ETAG) != d.etag
        || d.last_modified.as_ref().is_some_and(|old| {
            transfer::text(response, header::LAST_MODIFIED).as_ref() != Some(old)
        })
        || d.resolved_url
            .as_ref()
            .is_some_and(|old| response.url().as_str() != old)
    {
        return Err(Error::Protocol(
            "Remote resource identity or size changed; no ranges were combined".into(),
        ));
    }
    Ok(())
}
fn validate_encoding(response: &reqwest::Response) -> Result<()> {
    if transfer::text(response, header::CONTENT_ENCODING)
        .is_some_and(|e| !e.eq_ignore_ascii_case("identity"))
    {
        return Err(Error::Protocol("Encoded range response is unsafe".into()));
    }
    Ok(())
}
async fn probe(
    engine: &Engine,
    d: &mut Download,
    stop: &mut watch::Receiver<Stop>,
) -> Result<bool> {
    let mut guard =
        connections::send(engine, d, Some("bytes=0-0".into()), d.etag.as_deref(), stop).await?;
    let response = &mut guard.response;
    d.content_type =
        crate::transfer::text(response, header::CONTENT_TYPE).filter(|s| s.len() < 256);
    if !d.category_manual {
        d.category_id =
            Some(crate::organization::classify(&d.filename, d.content_type.as_deref()).into());
    }

    validate_encoding(response)?;
    if response.status() == StatusCode::OK {
        d.resume.ignored(response, d.etag.is_some());
        if d.segmented {
            return Err(Error::Protocol(
                "Range support or resource identity changed; segmented recovery stopped".into(),
            ));
        }
        d.notice = Some("Server did not validate byte ranges; using one stream".into());
        return Ok(false);
    }
    if response.status() != StatusCode::PARTIAL_CONTENT {
        return Err(Error::Protocol(
            "Range probe did not return 200 or 206".into(),
        ));
    }
    let content_range = transfer::text(response, header::CONTENT_RANGE)
        .ok_or_else(|| Error::Protocol("Probe missing Content-Range".into()))?;
    let unknown_total = content_range == "bytes 0-0/*";
    let (start, end, total) = if unknown_total {
        (0, 0, 0)
    } else {
        transfer::parse_range(&content_range)?
    };
    if start != 0 || end != 0 || response.content_length().is_some_and(|len| len != 1) {
        return Err(Error::Protocol("Invalid range probe boundaries".into()));
    }
    let mut received = 0usize;
    loop {
        let chunk = tokio::select! {chunk=response.chunk()=>chunk?, _=stop.changed()=>return Err(Error::Stopped)};
        let Some(chunk) = chunk else {
            break;
        };
        let mut consumed = 0;
        while consumed < chunk.len() {
            consumed += engine
                .inner
                .bandwidth
                .acquire(&d.id, chunk.len() - consumed, stop)
                .await?;
        }
        received += chunk.len();
        if received > 1 {
            return Err(Error::Protocol("Oversized range probe".into()));
        }
    }
    if received != 1 {
        return Err(Error::Protocol("Truncated range probe".into()));
    }
    if unknown_total {
        d.resume.unknown(ResumeReason::UnknownTotal);
        if d.segmented {
            return Err(Error::Protocol(
                "Segmented resource lost its known total".into(),
            ));
        }
        d.notice = Some("Using one stream: range probe has unknown total size".into());
        return Ok(false);
    }
    let etag = transfer::text(response, header::ETAG)
        .filter(|v| v.len() >= 2 && v.starts_with('"') && v.ends_with('"'));
    if d.segmented {
        if let Err(error) = identity(d, response, total) {
            d.resume.unknown(ResumeReason::IdentityChanged);
            return Err(error);
        }
    } else {
        if d.durable_bytes > 0
            && (d.etag != etag
                || d.total != Some(total)
                || d.last_modified.as_ref().is_some_and(|old| {
                    transfer::text(response, header::LAST_MODIFIED).as_ref() != Some(old)
                }))
        {
            d.resume.unknown(ResumeReason::IdentityChanged);
            return Err(Error::Protocol(
                "Remote identity changed since the durable checkpoint".into(),
            ));
        }
        d.resume.range(response, total);
        if etag.is_none() || total < MIN_RANGE * 2 {
            d.notice = Some("Using one stream: resource is small or has no strong ETag".into());
            return Ok(false);
        }
        d.total = Some(total);
        d.etag = etag;
        d.last_modified = transfer::text(response, header::LAST_MODIFIED);
        d.resolved_url = Some(response.url().as_str().into());
        if d.durable_bytes > 0 && d.completed_ranges.is_empty() {
            d.completed_ranges.push(ByteRange {
                start: 0,
                end: d.durable_bytes,
            });
        }
        d.segmented = true;
    }
    d.resume.range(response, total);
    Ok(true)
}

struct Shared {
    wire_bytes: AtomicU64,
}
async fn fetch(
    engine: Engine,
    d: Download,
    range: ByteRange,
    mut stop: watch::Receiver<Stop>,
    stats: Arc<WorkerStats>,
    shared: Arc<Shared>,
) -> Result<()> {
    fetch_once(&engine, &d, range, &mut stop, &stats, &shared).await
}
async fn fetch_once(
    engine: &Engine,
    d: &Download,
    range: ByteRange,
    stop: &mut watch::Receiver<Stop>,
    stats: &Arc<WorkerStats>,
    shared: &Shared,
) -> Result<()> {
    let mut guard = connections::send_tracked(
        engine,
        d,
        Some(format!("bytes={}-{}", range.start, range.end - 1)),
        d.etag.as_deref(),
        stop,
        Some(stats.clone()),
    )
    .await?;
    let response = &mut guard.response;
    validate_encoding(response)?;
    if response.status() != StatusCode::PARTIAL_CONTENT {
        return Err(Error::Protocol(
            "A segment returned a non-206 response; transfer stopped without fallback".into(),
        ));
    }
    let (start, end, total) = transfer::parse_range(
        &transfer::text(response, header::CONTENT_RANGE)
            .ok_or_else(|| Error::Protocol("Segment missing Content-Range".into()))?,
    )?;
    if start != range.start
        || end != range.end - 1
        || response
            .content_length()
            .is_some_and(|len| len != range.len())
    {
        return Err(Error::Protocol(
            "Segment boundaries or length mismatch".into(),
        ));
    }
    if let Err(error) = identity(d, response, total) {
        stats.identity_changed.store(true, Ordering::Relaxed);
        return Err(error);
    }
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let mut file = options.open(&d.partial_path).await?;
    file.seek(std::io::SeekFrom::Start(range.start)).await?;
    stats.phase(WorkerPhase::Transferring);
    let mut written = 0;
    loop {
        if *stop.borrow() != Stop::Running {
            return Err(Error::Stopped);
        }
        let chunk = tokio::select! {chunk=response.chunk()=>chunk?,_=stop.changed()=>return Err(Error::Stopped)};
        let Some(chunk) = chunk else {
            break;
        };
        if written + chunk.len() as u64 > range.len() {
            return Err(Error::Protocol("Segment exceeded its reservation".into()));
        }
        let mut consumed = 0;
        while consumed < chunk.len() {
            let granted = engine
                .inner
                .bandwidth
                .acquire(&d.id, chunk.len() - consumed, stop)
                .await?;
            file.write_all(&chunk[consumed..consumed + granted]).await?;
            file.flush().await?;
            consumed += granted;
            written += granted as u64;
            shared
                .wire_bytes
                .fetch_add(granted as u64, Ordering::Relaxed);
            stats.progress.store(written, Ordering::Relaxed);
        }
    }
    if written != range.len() {
        return Err(Error::Protocol("Truncated segment body".into()));
    }
    stats.phase(WorkerPhase::Synchronizing);
    file.flush().await?;
    file.sync_all().await?;
    stats.phase(WorkerPhase::Completed);
    Ok(())
}

pub(crate) async fn run(
    engine: &Engine,
    d: &mut Download,
    mut stop: watch::Receiver<Stop>,
) -> Result<()> {
    if !probe(engine, d, &mut stop).await? {
        return transfer::single(engine, d, stop).await;
    }
    let total = d
        .total
        .ok_or_else(|| Error::Protocol("Missing segment total".into()))?;
    ranges::validate(&d.completed_ranges, total)?;
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let file = options.open(&d.partial_path).await?;
    let required = d.completed_ranges.iter().map(|r| r.end).max().unwrap_or(0);
    if file.metadata().await?.len() < required {
        return Err(Error::Disk(std::io::Error::other(
            "Partial file is shorter than a completed range",
        )));
    }
    // No preallocation. Sparse holes and file length never advance progress.
    engine.persist(d).await?;
    let range_size = (total / 256).clamp(MIN_RANGE, 8 * 1024 * 1024);
    let mut adaptive = Adaptive::new(d.config.max_connections, d.config.optimize);
    if d.retries > 0 && d.target_connections > 0 {
        adaptive.resume_retry(d.target_connections);
    }
    let shared = Arc::new(Shared {
        wire_bytes: AtomicU64::new(0),
    });
    let mut tasks = JoinSet::new();
    let mut active: HashMap<u64, (ByteRange, Arc<WorkerStats>)> = HashMap::new();
    let mut interval = tokio::time::interval(Duration::from_millis(200));
    let mut sampled = Instant::now();
    let mut last_bytes = 0;
    let mut rate = RateSample::new(0);
    let mut next_worker_id = 1;
    let (worker_stop, worker_rx) = watch::channel(Stop::Running);
    let mut failure = None;
    loop {
        let reason = *stop.borrow();
        if reason != Stop::Running {
            worker_stop.send_replace(reason);
            break;
        }
        d.target_connections = adaptive.target;
        engine
            .inner
            .connections
            .target(&d.id, Some(adaptive.target));
        while tasks.len() < (adaptive.target as usize) {
            let occupied: Vec<_> = active.values().map(|(range, _)| *range).collect();
            let Some(range) = ranges::next(&d.completed_ranges, &occupied, total, range_size)
            else {
                break;
            };
            let stats = Arc::new(WorkerStats::new(next_worker_id, range));
            next_worker_id += 1;
            active.insert(range.start, (range, stats.clone()));
            let (engine, download, rx, shared) =
                (engine.clone(), d.clone(), worker_rx.clone(), shared.clone());
            tasks.spawn(async move {
                (
                    range,
                    fetch(engine, download, range, rx, stats, shared).await,
                )
            });
        }
        if tasks.is_empty() {
            break;
        }
        tokio::select! {
            _ = stop.changed() => {
                worker_stop.send_replace(*stop.borrow());
                break;
            }
            _ = interval.tick() => {
                capture(engine, d, &active);
                let wire_sample = shared.wire_bytes.load(Ordering::Relaxed);
                d.speed = rate.sample(wire_sample);
                d.telemetry.rate_sequence = d.telemetry.rate_sequence.saturating_add(1);
                let elapsed = sampled.elapsed();
                let wire = shared.wire_bytes.load(Ordering::Relaxed);
                let adaptive_speed = (wire - last_bytes) as f64 / elapsed.as_secs_f64().max(0.001);
                if elapsed >= Duration::from_secs(2) {
                    adaptive.sample_capped(adaptive_speed, elapsed, total.saturating_sub(d.downloaded), false, engine.inner.bandwidth.is_capped(&d.id));
                    sampled = Instant::now();
                    last_bytes = wire;
                }
                engine.publish(d).await;
            }
            completed = tasks.join_next() => {
                match completed {
                    Some(Ok((range, result))) => {
                        let worker = active.remove(&range.start);
                        if let Err(error) = result {
                            if worker.is_some_and(|(_, stats)| stats.identity_changed.load(Ordering::Relaxed)) {
                                d.resume.unknown(ResumeReason::IdentityChanged);
                            }
                            failure = Some(error);
                            worker_stop.send_replace(Stop::Pause);
                            break;
                        }
                        ranges::complete(&mut d.completed_ranges, range);
                        d.durable_bytes = ranges::bytes(&d.completed_ranges);
                        capture(engine, d, &active);
                        if let Err(error) = engine.persist(d).await {
                            failure = Some(error);
                            worker_stop.send_replace(Stop::Pause);
                            break;
                        }
                    }
                    Some(Err(error)) => {
                        failure = Some(Error::Worker(error.to_string()));
                        worker_stop.send_replace(Stop::Pause);
                        break;
                    }
                    None => break,
                }
            }
        }
    }

    // Always await every writer before pause, retry, cancellation, or shutdown returns.
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok((range, Ok(()))) => ranges::complete(&mut d.completed_ranges, range),
            Ok((_, Err(Error::Stopped))) => {}
            Ok((_, Err(error))) => {
                if failure.is_none() {
                    failure = Some(error);
                }
            }
            Err(error) => {
                if failure.is_none() {
                    failure = Some(Error::Worker(error.to_string()));
                }
            }
        }
    }
    d.telemetry.clear();
    d.durable_bytes = ranges::bytes(&d.completed_ranges);
    d.downloaded = d.durable_bytes;
    d.speed = 0.0;
    d.active_connections = 0;
    if failure.is_some() {
        adaptive.sample_capped(
            0.0,
            Duration::ZERO,
            total.saturating_sub(d.downloaded),
            true,
            engine.inner.bandwidth.is_capped(&d.id),
        );
        d.target_connections = adaptive.target;
    }
    engine.persist(d).await?;
    if let Some(error) = failure {
        return Err(error);
    }
    if *stop.borrow() != Stop::Running {
        d.status = if *stop.borrow() == Stop::Cancel {
            Status::Cancelled
        } else {
            Status::Paused
        };
        return engine.persist(d).await;
    }
    if d.durable_bytes != total {
        return Err(Error::Protocol("Incomplete range coverage".into()));
    }
    file.set_len(total).await?;
    transfer::finalize(engine, d, file, stop).await
}

/// Sample each counter once; both public workers and aggregate use these values.
fn capture(
    engine: &Engine,
    d: &mut Download,
    active: &HashMap<u64, (ByteRange, Arc<WorkerStats>)>,
) {
    d.telemetry.sequence = d.telemetry.sequence.saturating_add(1);
    let (workers, requests) = engine.inner.connections.sample_workers(
        &d.id,
        d.telemetry
            .generation
            .as_deref()
            .expect("transfer generation"),
        d.telemetry.sequence,
        active.values().map(|(_, stats)| stats),
    );
    d.downloaded = d.durable_bytes + workers.iter().map(|w| w.written_bytes).sum::<u64>();
    d.active_connections = requests;
    d.telemetry.workers = workers;
}

#[cfg(test)]
mod telemetry_tests {
    use super::*;
    #[tokio::test]
    #[ignore = "release-mode sampling microbenchmark, run explicitly"]
    async fn sampling_cost() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(dir.path().join("metadata")).await.unwrap();
        let mut d = engine
            .create(
                "http://localhost/file".into(),
                dir.path().into(),
                "sample".into(),
            )
            .await
            .unwrap();
        d.telemetry.begin();
        for count in [1, 4, 8, 16, 32] {
            let active: HashMap<_, _> = (0..count)
                .map(|i| {
                    let range = ByteRange {
                        start: i * 2_097_152,
                        end: (i + 1) * 2_097_152,
                    };
                    (
                        range.start,
                        (range, Arc::new(WorkerStats::new(i + 1, range))),
                    )
                })
                .collect();
            let start = Instant::now();
            for _ in 0..10_000 {
                std::hint::black_box(
                    active
                        .values()
                        .map(|(_, s)| s.progress.load(Ordering::Relaxed))
                        .sum::<u64>(),
                );
                std::hint::black_box(engine.inner.connections.active(&d.id));
            }
            let baseline = start.elapsed().as_nanos() / 10_000;
            let start = Instant::now();
            for _ in 0..10_000 {
                capture(&engine, &mut d, &active);
                std::hint::black_box(&d.telemetry);
            }
            println!(
                "SAMPLING workers={count} baseline_ns={baseline} telemetry_ns={}",
                start.elapsed().as_nanos() / 10_000
            );
        }
        engine.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn one_sample_accounts_unequal_workers_and_completion_without_regression() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(dir.path().join("metadata")).await.unwrap();
        let mut d = engine
            .create(
                "http://localhost/file".into(),
                dir.path().into(),
                "sample".into(),
            )
            .await
            .unwrap();
        d.telemetry.begin();
        let generation = d.telemetry.generation.clone();
        let mut active = HashMap::new();
        for (id, start, end, bytes) in [(1, 0, 100, 100), (2, 100, 300, 120), (3, 300, 600, 0)] {
            let range = ByteRange { start, end };
            let stats = Arc::new(WorkerStats::new(id, range));
            stats.progress.store(bytes, Ordering::Relaxed);
            active.insert(start, (range, stats));
        }
        capture(&engine, &mut d, &active);
        assert_eq!(d.downloaded, 220);
        assert_eq!(
            d.telemetry
                .workers
                .iter()
                .map(|w| w.written_bytes)
                .collect::<Vec<_>>(),
            [100, 120, 0]
        );
        let sequence = d.telemetry.sequence;
        assert!(
            d.telemetry
                .workers
                .iter()
                .all(|w| w.sample_sequence == sequence)
        );
        active.remove(&0);
        d.completed_ranges.push(ByteRange { start: 0, end: 100 });
        d.durable_bytes = 100;
        capture(&engine, &mut d, &active);
        assert_eq!(
            d.downloaded, 220,
            "Completion must not drop the other worker's live bytes"
        );
        assert_eq!(d.telemetry.generation, generation);
        assert_eq!(d.telemetry.workers[0].worker_id, 2);
        assert!(d.telemetry.sequence > sequence);
        d.telemetry.begin();
        assert_ne!(d.telemetry.generation, generation);
        assert!(d.telemetry.workers.is_empty());
        engine.shutdown().await.unwrap();
    }
}
