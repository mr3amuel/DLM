use crate::{
    Download, Engine, Error, Result, ResumeReason, Status, engine::Stop, telemetry::RateSample,
};
use reqwest::{Response, StatusCode, header};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::watch,
};

const CHECKPOINT_BYTES: u64 = 1024 * 1024;

pub(crate) async fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).await?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(crate) fn text(response: &Response, name: header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}
pub(crate) fn parse_range(value: &str) -> Result<(u64, u64, u64)> {
    let invalid = || Error::Protocol("Malformed Content-Range".into());
    let (range, total) = value
        .strip_prefix("bytes ")
        .ok_or_else(invalid)?
        .split_once('/')
        .ok_or_else(invalid)?;
    let (start, end) = range.split_once('-').ok_or_else(invalid)?;
    let (start, end, total) = (
        start.parse::<u64>().map_err(|_| invalid())?,
        end.parse::<u64>().map_err(|_| invalid())?,
        total.parse::<u64>().map_err(|_| invalid())?,
    );
    if start > end || end >= total {
        return Err(invalid());
    }
    Ok((start, end, total))
}
async fn checkpoint(engine: &Engine, d: &mut Download, file: &mut File) -> Result<()> {
    file.flush().await?;
    file.sync_all().await?;
    d.durable_bytes = d.downloaded;
    if !d.segmented {
        d.completed_ranges = if d.durable_bytes > 0 {
            vec![crate::ByteRange {
                start: 0,
                end: d.durable_bytes,
            }]
        } else {
            vec![]
        };
    }
    engine.persist(d).await
}
async fn stopped(engine: &Engine, d: &mut Download, file: &mut File, stop: Stop) -> Result<()> {
    d.status = if stop == Stop::Cancel {
        Status::Cancelled
    } else {
        Status::Paused
    };
    d.speed = 0.0;
    checkpoint(engine, d, file).await
}

async fn single_run(
    engine: &Engine,
    d: &mut Download,
    mut stop: watch::Receiver<Stop>,
) -> Result<()> {
    d.target_connections = 1;
    engine.inner.connections.target(&d.id, Some(1));
    let metadata = tokio::fs::symlink_metadata(&d.partial_path).await?;
    if !metadata.is_file() || metadata.len() < d.durable_bytes {
        return Err(Error::Invalid(
            "Partial file is missing, replaced, or truncated".into(),
        ));
    }
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let mut file = options.open(&d.partial_path).await?;
    let mut offset = d.durable_bytes;
    d.downloaded = offset;
    if d.total == Some(offset) {
        file.set_len(offset).await?;
        return finalize(engine, d, file, stop).await;
    }
    let validator = d.etag.clone();
    // A timestamp alone is not a sufficiently strong resource identity.
    // Without a strong ETag, restart conservatively.
    if offset > 0 && validator.is_none() {
        if d.automation_strict_resume {
            return Err(Error::Protocol("Scheduled resume lacks a strong validator; checkpoint preserved. Resume manually or update the link.".into()));
        }
        offset = 0;
    }
    let range = (offset > 0).then(|| format!("bytes={offset}-"));
    let response = crate::connections::send(
        engine,
        d,
        range,
        if offset > 0 {
            validator.as_deref()
        } else {
            None
        },
        &mut stop,
    )
    .await?;
    let mut response_guard = response;
    let response = &mut response_guard.response;
    d.content_type =
        crate::transfer::text(response, header::CONTENT_TYPE).filter(|s| s.len() < 256);
    if !d.category_manual {
        d.category_id =
            Some(crate::organization::classify(&d.filename, d.content_type.as_deref()).into());
    }

    if let Some(encoding) = text(response, header::CONTENT_ENCODING)
        && !encoding.eq_ignore_ascii_case("identity")
    {
        return Err(Error::Protocol(
            "Server returned an encoded representation despite Accept-Encoding: identity".into(),
        ));
    }
    let etag = crate::resume::strong_etag(response);
    let modified = text(response, header::LAST_MODIFIED);
    let expected_end;
    match response.status() {
        StatusCode::PARTIAL_CONTENT => {
            if offset == 0 {
                return Err(Error::Protocol("Unsolicited partial response".into()));
            }
            let (start, end, total) = parse_range(
                &text(response, header::CONTENT_RANGE)
                    .ok_or_else(|| Error::Protocol("Missing Content-Range".into()))?,
            )?;
            if start != offset || end != total - 1 || d.total.is_some_and(|old| old != total) {
                return Err(Error::Protocol(
                    "Content-Range does not match the requested remainder".into(),
                ));
            }
            if d.resolved_url
                .as_ref()
                .is_some_and(|old| response.url().as_str() != old)
                || d.etag
                    .as_ref()
                    .is_some_and(|old| etag.as_ref() != Some(old))
                || (d
                    .last_modified
                    .as_ref()
                    .is_some_and(|old| modified.as_ref() != Some(old)))
            {
                d.resume.unknown(ResumeReason::IdentityChanged);
                return Err(Error::Protocol(
                    "Remote resource identity changed during resume".into(),
                ));
            }
            if response
                .content_length()
                .is_some_and(|len| len != end - start + 1)
            {
                return Err(Error::Protocol("Range length mismatch".into()));
            }
            d.total = Some(total);
            d.resume.range(response, total);
            expected_end = Some(total);
        }
        StatusCode::OK => {
            if d.recovery_pin.is_some() || (d.automation_strict_resume && offset > 0) {
                return Err(Error::Protocol("Verified replacement changed or ignored Range; partial data preserved. Update the link again or explicitly restart.".into()));
            }
            d.resume.full(response, offset > 0);
            // Commit a zero checkpoint BEFORE discarding old bytes. A crash can
            // never leave metadata claiming bytes that have already been removed.
            d.downloaded = 0;
            d.durable_bytes = 0;
            d.total = response.content_length();
            d.etag = etag;
            d.last_modified = modified;
            d.resolved_url = Some(response.url().to_string());
            d.sha256 = None;
            d.completed_ranges.clear();
            engine.persist(d).await?;
            file.set_len(0).await?;
            offset = 0;
            expected_end = d.total;
        }
        code => {
            return Err(Error::Protocol(format!(
                "HTTP {code}; transfer left resumable"
            )));
        }
    }
    file.set_len(offset).await?;
    file.seek(std::io::SeekFrom::Start(offset)).await?;
    d.downloaded = offset;
    let mut last_checkpoint = Instant::now();
    let mut rate = RateSample::new(offset);
    let mut samples = tokio::time::interval(Duration::from_millis(200));
    samples.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let reason = *stop.borrow();
        if reason != Stop::Running {
            return stopped(engine, d, &mut file, reason).await;
        }
        let chunk = tokio::select! {
            chunk = response.chunk() => chunk?,
            _ = stop.changed() => { let reason = *stop.borrow(); return stopped(engine, d, &mut file, reason).await; }
            _ = samples.tick() => {
                sample_single(d, &mut rate);
                engine.publish(d).await;
                continue;
            }
        };
        let Some(chunk) = chunk else {
            break;
        };
        let next = d
            .downloaded
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| Error::Protocol("Byte count overflow".into()))?;
        if expected_end.is_some_and(|end| next > end) {
            return Err(Error::Protocol("Response exceeded declared length".into()));
        }
        let mut consumed = 0;
        while consumed < chunk.len() {
            let grant = {
                let id = d.id.clone();
                let acquire =
                    engine
                        .inner
                        .bandwidth
                        .acquire(&id, chunk.len() - consumed, &mut stop);
                tokio::pin!(acquire);
                loop {
                    tokio::select! {
                        result = &mut acquire => break result,
                        _ = samples.tick() => {
                            sample_single(d, &mut rate);
                            engine.publish(d).await;
                        }
                    }
                }
            };
            let granted = match grant {
                Ok(granted) => granted,
                Err(Error::Stopped) => {
                    let reason = *stop.borrow();
                    return stopped(engine, d, &mut file, reason).await;
                }
                Err(error) => return Err(error),
            };
            file.write_all(&chunk[consumed..consumed + granted]).await?;
            file.flush().await?;
            consumed += granted;
            d.downloaded += granted as u64;
            if d.downloaded - d.durable_bytes >= CHECKPOINT_BYTES
                || last_checkpoint.elapsed() >= Duration::from_secs(1)
            {
                checkpoint(engine, d, &mut file).await?;
                last_checkpoint = Instant::now();
            }
        }
    }
    if expected_end.is_some_and(|end| d.downloaded != end) {
        return Err(Error::Protocol(
            "Response ended before the expected length".into(),
        ));
    }
    let reason = *stop.borrow();
    if reason != Stop::Running {
        return stopped(engine, d, &mut file, reason).await;
    }
    finalize(engine, d, file, stop).await
}

pub(crate) async fn finalize(
    engine: &Engine,
    d: &mut Download,
    mut file: File,
    stop: watch::Receiver<Stop>,
) -> Result<()> {
    checkpoint(engine, d, &mut file).await?;
    if file.metadata().await?.len() != d.downloaded {
        return Err(Error::Protocol("On-disk length mismatch".into()));
    }
    d.sha256 = Some(hash_file(Path::new(&d.partial_path)).await?);
    let reason = *stop.borrow();
    if reason != Stop::Running {
        return stopped(engine, d, &mut file, reason).await;
    }
    d.total = Some(d.downloaded);
    d.speed = 0.0;
    d.active_connections = 0;
    d.status = Status::Finalizing;
    engine.persist(d).await?;
    drop(file);
    // Atomic no-replace rename delegates OS differences to a portable filesystem
    // dependency. Never use its non-atomic fallback or std's overwriting rename.
    let source = d.partial_path.clone();
    let destination = d.destination.clone();
    tokio::task::spawn_blocking(move || renamore::rename_exclusive(source, destination))
        .await
        .map_err(|e| Error::Worker(e.to_string()))??;
    sync_parent(Path::new(&d.destination)).await?;
    d.status = Status::Completed;
    engine.persist(d).await?;
    // Publication is already committed. Cleanup failure must not turn a
    // verified completed download back into a failed transfer.
    if tokio::fs::remove_file(&d.partial_path).await.is_ok() {
        let _ = sync_parent(Path::new(&d.destination)).await;
    }
    Ok(())
}

pub(crate) async fn sync_parent(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let parent = path
            .parent()
            .ok_or_else(|| Error::Invalid("Missing parent directory".into()))?
            .to_owned();
        tokio::task::spawn_blocking(move || std::fs::File::open(parent)?.sync_all())
            .await
            .map_err(|e| Error::Worker(e.to_string()))??;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub(crate) async fn run(
    engine: &Engine,
    d: &mut Download,
    stop: watch::Receiver<Stop>,
) -> Result<()> {
    crate::recovery::check_pin(engine, d, stop.clone()).await?;
    // A complete durable representation needs no new remote bytes. Preserve
    // finalization even when a signed source URL has expired after the last sync.
    if d.total == Some(d.durable_bytes)
        && (!d.segmented
            || (crate::ranges::validate(&d.completed_ranges, d.durable_bytes).is_ok()
                && crate::ranges::bytes(&d.completed_ranges) == d.durable_bytes))
    {
        return single_run(engine, d, stop).await;
    }
    if d.segmented || d.config.max_connections > 1 {
        crate::segmented::run(engine, d, stop).await
    } else {
        single_run(engine, d, stop).await
    }
}
pub(crate) async fn single(
    engine: &Engine,
    d: &mut Download,
    stop: watch::Receiver<Stop>,
) -> Result<()> {
    single_run(engine, d, stop).await
}

fn sample_single(d: &mut Download, rate: &mut RateSample) {
    d.telemetry.sequence = d.telemetry.sequence.saturating_add(1);
    d.speed = rate.sample(d.downloaded);
    d.telemetry.rate_sequence = d.telemetry.rate_sequence.saturating_add(1);
}
