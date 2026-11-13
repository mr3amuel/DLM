//! Replacement URLs are never identities. Compare ALL retained bytes against one
//! validator-pinned representation, or refuse. No sampling-based success path.
use crate::{
    ByteRange, Download, Engine, Error, Result, Status, connections, engine::Stop, transfer,
};
use reqwest::header;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    sync::watch,
};

const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_RANGES: usize = 256;
const LEASE: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryOutcome {
    SafeToResume,
    Incompatible,
    CannotVerify,
    RangeUnsupported,
    TemporaryFailure,
}
#[derive(Debug, Clone)]
pub struct VerificationReport {
    pub outcome: RecoveryOutcome,
    pub reason: String,
    pub token: Option<String>,
    pub compared_bytes: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryPin {
    pub etag: String,
    pub resolved_url: String,
    pub total: u64,
    pub ranges: Vec<ByteRange>,
    pub local_hash: String,
}
pub(crate) struct Entry {
    token: String,
    busy: bool,
    created: Instant,
    old: Download,
    url: String,
    pin: Option<RecoveryPin>,
}
struct Lease<'a> {
    engine: &'a Engine,
    id: String,
    token: String,
}
impl Drop for Lease<'_> {
    fn drop(&mut self) {
        let mut entries = self.engine.inner.recovery.lock().unwrap();
        if entries
            .get(&self.id)
            .is_some_and(|e| e.busy && e.token == self.token)
        {
            entries.remove(&self.id);
        }
    }
}
fn eligible(d: &Download) -> bool {
    matches!(
        d.status,
        Status::Paused | Status::Failed | Status::Cancelled
    )
}
fn same(a: &Download, b: &Download) -> bool {
    a.status == b.status
        && a.url == b.url
        && a.partial_path == b.partial_path
        && a.destination == b.destination
        && a.durable_bytes == b.durable_bytes
        && a.total == b.total
        && a.completed_ranges == b.completed_ranges
}
fn report(outcome: RecoveryOutcome, reason: &str, compared_bytes: u64) -> VerificationReport {
    VerificationReport {
        outcome,
        reason: reason.into(),
        token: None,
        compared_bytes,
    }
}
pub(crate) fn url(value: &str) -> Result<String> {
    let mut parsed = reqwest::Url::parse(value.trim())
        .map_err(|_| Error::Invalid("Enter a valid HTTP or HTTPS URL".into()))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(Error::Invalid(
            "Enter a valid HTTP or HTTPS URL without credentials".into(),
        ));
    }
    parsed.set_fragment(None);
    Ok(parsed.to_string())
}
impl Engine {
    /// Called under the ordinary command lock. Ready tickets are invalidated on
    /// lifecycle changes; verification never holds the global lock during I/O.
    pub(crate) fn guard_recovery(&self, id: &str) -> Result<()> {
        if let Some(m) = self.inner.file_ops.lock().unwrap().get(id) {
            return Err(Error::Busy(
                if *m.done.borrow() {
                    "A file move needs recovery; restart DLM"
                } else {
                    "A file move is in progress"
                }
                .into(),
            ));
        }
        let mut entries = self.inner.recovery.lock().unwrap();
        if entries.get(id).is_some_and(|e| e.busy) {
            return Err(Error::Busy("Link verification is in progress".into()));
        }
        entries.remove(id);
        Ok(())
    }
    pub async fn verify_download_link(
        &self,
        id: &str,
        replacement: &str,
    ) -> Result<VerificationReport> {
        let replacement = url(replacement)?;
        let (old, token) = {
            let _lock = self.inner.commands.lock().await;
            self.ensure_open()?;
            self.guard_recovery(id)?;
            self.manual_override_locked(id).await?;
            if !eligible(&self.get(id).await?) {
                return Err(Error::Busy(
                    "Pause the download before updating its link".into(),
                ));
            }
            self.settle_worker(id).await?;
            let old = self.get(id).await?;
            if !eligible(&old) {
                return Err(Error::Busy(
                    "Pause the download before updating its link".into(),
                ));
            }
            let token = uuid::Uuid::new_v4().to_string();
            let mut entries = self.inner.recovery.lock().unwrap();
            entries.retain(|_, e| e.busy || e.created.elapsed() < LEASE);
            entries.insert(
                id.into(),
                Entry {
                    token: token.clone(),
                    busy: true,
                    created: Instant::now(),
                    old: old.clone(),
                    url: replacement.clone(),
                    pin: None,
                },
            );
            (old, token)
        };
        let _lease = Lease {
            engine: self,
            id: id.into(),
            token: token.clone(),
        };
        let inspected =
            tokio::time::timeout(Duration::from_secs(120), inspect(self, &old, &replacement)).await;
        let (mut result, pin) = match inspected {
            Ok(Ok(value)) => value,
            Ok(Err(Error::Network(_))) | Err(_) => (
                report(
                    RecoveryOutcome::TemporaryFailure,
                    "Verification timed out or the network failed. Partial data is unchanged; try again.",
                    0,
                ),
                None,
            ),
            Ok(Err(Error::Http {
                status: 429 | 500..=599,
                ..
            })) => (
                report(
                    RecoveryOutcome::TemporaryFailure,
                    "The replacement server is temporarily unavailable or rate limited. Try again later.",
                    0,
                ),
                None,
            ),
            Ok(Err(Error::Http { .. })) => (
                report(
                    RecoveryOutcome::CannotVerify,
                    "The replacement link is inaccessible. It may be expired or require permission; HTTP status does not prove expiration.",
                    0,
                ),
                None,
            ),
            Ok(Err(Error::Disk(_))) => (
                report(
                    RecoveryOutcome::CannotVerify,
                    "The partial file is missing, inaccessible or truncated. No partial data was changed.",
                    0,
                ),
                None,
            ),
            Ok(Err(_)) => (
                report(
                    RecoveryOutcome::CannotVerify,
                    "The response could not be safely validated. Partial data is unchanged.",
                    0,
                ),
                None,
            ),
        };
        let _lock = self.inner.commands.lock().await;
        self.ensure_open()?;
        if !same(&old, &self.get(id).await?) {
            return Err(Error::Busy(
                "Download changed; verify the link again".into(),
            ));
        }
        let mut entries = self.inner.recovery.lock().unwrap();
        let entry = entries
            .get_mut(id)
            .ok_or_else(|| Error::Invalid("Verification expired".into()))?;
        entry.busy = false;
        entry.pin = pin;
        entry.created = Instant::now();
        result.token = Some(token);
        Ok(result)
    }
    pub async fn apply_download_link(&self, id: &str, token: &str, restart: bool) -> Result<()> {
        let _lock = self.inner.commands.lock().await;
        self.ensure_open()?;
        let (old, replacement, pin) = {
            let entries = self.inner.recovery.lock().unwrap();
            let e = entries
                .get(id)
                .filter(|e| !e.busy && e.token == token && e.created.elapsed() < LEASE)
                .ok_or_else(|| {
                    Error::Invalid("Verification expired or changed; verify again".into())
                })?;
            (e.old.clone(), e.url.clone(), e.pin.clone())
        };
        let mut d = self.get(id).await?;
        if !eligible(&d) || !same(&old, &d) {
            return Err(Error::Invalid("Download changed; verify again".into()));
        }
        self.manual_override_locked(id).await?;
        if !restart {
            let pin = pin.ok_or_else(|| {
                Error::Invalid("Compatibility is not verified; partial data was preserved".into())
            })?;
            if hash_ranges(&d.partial_path, &pin.ranges).await? != pin.local_hash {
                self.inner.recovery.lock().unwrap().remove(id);
                return Err(Error::Invalid(
                    "Partial data changed after verification; verify again".into(),
                ));
            }
            d.url = replacement;
            d.etag = Some(pin.etag.clone());
            d.resolved_url = Some(pin.resolved_url.clone());
            d.total = Some(pin.total);
            d.last_modified = None;
            d.sha256 = None;
            d.recovery_pin = Some(pin);
        } else {
            // New path + SQLite commit first: a crash never pairs reset metadata
            // with old bytes, or old durable metadata with a truncated file.
            let old_path = d.partial_path.clone();
            let parent = std::path::Path::new(&d.destination)
                .parent()
                .ok_or_else(|| Error::Invalid("Missing destination directory".into()))?;
            match tokio::fs::symlink_metadata(&d.destination).await {
                Ok(_) => {
                    return Err(Error::Invalid(
                        "Destination already exists; no files were changed".into(),
                    ));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            let path = parent.join(format!(".dlm-{}.part", uuid::Uuid::new_v4()));
            let file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .await?;
            file.sync_all().await?;
            transfer::sync_parent(&path).await?;
            d.partial_path = path.to_string_lossy().into();
            d.url = replacement;
            d.downloaded = 0;
            d.durable_bytes = 0;
            d.completed_ranges.clear();
            d.total = None;
            d.segmented = false;
            d.etag = None;
            d.last_modified = None;
            d.resolved_url = None;
            d.sha256 = None;
            d.recovery_pin = None;
            d.status = Status::Ready;
            reset(&mut d);
            if let Err(e) = self.persist(&d).await {
                drop(file);
                let _ = tokio::fs::remove_file(&path).await;
                return Err(e);
            }
            self.inner.recovery.lock().unwrap().remove(id);
            // Cleanup failure is explicit, with a usable Ready record and no lost
            // reference to the old partial: retain its path in the notice.
            match tokio::fs::remove_file(&old_path).await {
                Ok(()) => transfer::sync_parent(std::path::Path::new(&old_path)).await?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    d.notice = Some(format!(
                        "Restart prepared; old partial cleanup requires attention: {old_path}"
                    ));
                    self.persist(&d).await?;
                    return Err(Error::Invalid("Restart prepared, but old partial cleanup failed. Inspect download details before starting.".into()));
                }
            }
            return Ok(());
        }
        d.status = Status::Paused;
        reset(&mut d);
        self.persist(&d).await?;
        self.inner.recovery.lock().unwrap().remove(id);
        Ok(())
    }
}
fn reset(d: &mut Download) {
    d.notice = None;
    d.target_connections = 0;
    d.queue_position = None;
    d.retry_remaining_seconds = None;
    d.error = None;
    d.error_kind = None;
    d.last_http_status = None;
    d.telemetry = Default::default();
    d.resume = Default::default();
    d.speed = 0.0;
    d.current_speed = 0.0;
    d.smoothed_speed = 0.0;
    d.active_connections = 0;
    d.eta_seconds = None;
    d.retries = 0;
}
async fn hash_ranges(path: &str, ranges: &[ByteRange]) -> Result<String> {
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let mut file = options.open(path).await?;
    if !file.metadata().await?.is_file() {
        return Err(Error::Invalid("Partial is not a regular file".into()));
    }
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    for r in ranges {
        hash.update(r.start.to_le_bytes());
        hash.update(r.end.to_le_bytes());
        file.seek(std::io::SeekFrom::Start(r.start)).await?;
        let mut left = r.len();
        while left > 0 {
            let n = left.min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..n]).await?;
            hash.update(&buffer[..n]);
            left -= n as u64;
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}
async fn inspect(
    engine: &Engine,
    old: &Download,
    replacement: &str,
) -> Result<(VerificationReport, Option<RecoveryPin>)> {
    use RecoveryOutcome::*;
    let no = |kind, reason| Ok((report(kind, reason, 0), None));
    if old.status == Status::Cancelled || old.durable_bytes == 0 {
        return no(
            CannotVerify,
            "No resumable checkpoint is retained. Use the explicit restart option to begin from zero.",
        );
    }
    let Some(total) = old.total else {
        return no(
            CannotVerify,
            "The original file size is unknown; compatibility cannot be established.",
        );
    };
    let ranges = if old.segmented {
        old.completed_ranges.clone()
    } else {
        vec![ByteRange {
            start: 0,
            end: old.durable_bytes,
        }]
    };
    crate::ranges::validate(&ranges, total)?;
    if crate::ranges::bytes(&ranges) != old.durable_bytes
        || old.durable_bytes > MAX_BYTES
        || ranges.len() > MAX_RANGES
    {
        return no(
            CannotVerify,
            "Verification requires all checkpoint bytes. The 256 MiB or 256-range safety budget was exceeded; sampled matches are not sufficient.",
        );
    }
    let before = hash_ranges(&old.partial_path, &ranges).await?;
    let mut candidate = old.clone();
    candidate.url = replacement.into();
    let (_sender, mut stop) = watch::channel(Stop::Running);
    let mut probe =
        connections::send_verification(engine, &candidate, "bytes=0-0".into(), None, &mut stop)
            .await?;
    let response = &mut probe.response;
    if response.status() == reqwest::StatusCode::OK {
        return no(
            RangeUnsupported,
            "The replacement server ignored Range. Continuing safely is unavailable; restart requires confirmation.",
        );
    }
    if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        return no(
            CannotVerify,
            "The replacement response is not a valid partial representation.",
        );
    }
    if transfer::text(response, header::CONTENT_ENCODING)
        .is_some_and(|e| !e.eq_ignore_ascii_case("identity"))
    {
        return no(
            CannotVerify,
            "Encoded responses cannot be safely combined with the existing checkpoint.",
        );
    }
    let Ok((a, b, size)) =
        transfer::parse_range(&transfer::text(response, header::CONTENT_RANGE).unwrap_or_default())
    else {
        return no(
            CannotVerify,
            "The replacement returned malformed range boundaries or length.",
        );
    };
    if a != 0 || b != 0 || response.content_length().is_some_and(|n| n != 1) {
        return no(
            CannotVerify,
            "The replacement returned malformed range boundaries or length.",
        );
    }
    if size != total {
        return no(
            Incompatible,
            "The replacement file size differs from the retained checkpoint.",
        );
    }
    let Some(etag) = crate::resume::strong_etag(response) else {
        return no(
            CannotVerify,
            "The replacement lacks a strong validator. Matching names, sizes or samples cannot prove compatibility.",
        );
    };
    let resolved = response.url().to_string();
    let mut probe_bytes = 0;
    while let Some(chunk) = response.chunk().await? {
        probe_bytes += chunk.len();
        if probe_bytes > 1 {
            return no(CannotVerify, "The range probe body has an invalid length.");
        }
    }
    if probe_bytes != 1 {
        return no(CannotVerify, "The range probe body has an invalid length.");
    }
    drop(probe);
    let mut file = tokio::fs::File::open(&old.partial_path).await?;
    let mut buffer = vec![0; 64 * 1024];
    let mut compared = 0;
    let mut compared_hash = Sha256::new();
    for r in &ranges {
        compared_hash.update(r.start.to_le_bytes());
        compared_hash.update(r.end.to_le_bytes());
        let mut position = r.start;
        while position < r.end {
            if engine.is_shutdown() {
                return Err(Error::Closed);
            }
            let end = r.end.min(position.saturating_add(8 * 1024 * 1024));
            let mut reply = connections::send_verification(
                engine,
                &candidate,
                format!("bytes={position}-{}", end - 1),
                Some(&etag),
                &mut stop,
            )
            .await?;
            let res = &mut reply.response;
            if res.status() != reqwest::StatusCode::PARTIAL_CONTENT
                || crate::resume::strong_etag(res).as_ref() != Some(&etag)
                || res.url().as_str() != resolved
                || transfer::text(res, header::CONTENT_ENCODING)
                    .is_some_and(|e| !e.eq_ignore_ascii_case("identity"))
            {
                return no(
                    CannotVerify,
                    "The replacement resource changed during verification. No data was modified.",
                );
            }
            if !transfer::parse_range(
                &transfer::text(res, header::CONTENT_RANGE).unwrap_or_default(),
            )
            .is_ok_and(|range| range == (position, end - 1, total))
                || res.content_length().is_some_and(|n| n != end - position)
            {
                return no(
                    CannotVerify,
                    "The replacement returned malformed range boundaries or length.",
                );
            }
            file.seek(std::io::SeekFrom::Start(position)).await?;
            let mut received = 0;
            while let Some(chunk) = res.chunk().await? {
                if received + chunk.len() as u64 > end - position {
                    return no(
                        CannotVerify,
                        "The range comparison body has an invalid length.",
                    );
                }
                for block in chunk.chunks(buffer.len()) {
                    // Verification traffic obeys the same global/local payload cap.
                    let mut consumed = 0;
                    while consumed < block.len() {
                        consumed += engine
                            .inner
                            .bandwidth
                            .acquire(&old.id, block.len() - consumed, &mut stop)
                            .await?;
                    }
                    file.read_exact(&mut buffer[..block.len()]).await?;
                    if buffer[..block.len()] != *block {
                        return no(
                            Incompatible,
                            "Stored checkpoint bytes differ from the replacement. Corruption or a different resource was detected.",
                        );
                    }
                    compared_hash.update(block);
                    compared += block.len() as u64;
                }
                received += chunk.len() as u64;
            }
            if received != end - position {
                return no(
                    CannotVerify,
                    "The range comparison body has an invalid length.",
                );
            }
            position = end;
        }
    }
    if hash_ranges(&old.partial_path, &ranges).await? != before
        || format!("{:x}", compared_hash.finalize()) != before
    {
        return no(
            CannotVerify,
            "Partial data changed during verification. Verify again.",
        );
    }
    Ok((
        report(
            SafeToResume,
            "Every retained checkpoint byte matches one strong-validator-pinned replacement representation. Resume will preserve the checkpoint.",
            compared,
        ),
        Some(RecoveryPin {
            etag,
            resolved_url: resolved,
            total,
            ranges,
            local_hash: before,
        }),
    ))
}
/// Persisted pin survives a crash and prevents a verified single-stream resume
/// from using the legacy silent-200 restart behavior.
pub(crate) async fn check_pin(
    engine: &Engine,
    d: &Download,
    mut stop: watch::Receiver<Stop>,
) -> Result<()> {
    let Some(pin) = &d.recovery_pin else {
        return Ok(());
    };
    if hash_ranges(&d.partial_path, &pin.ranges).await? != pin.local_hash {
        return Err(Error::Protocol(
            "Verified checkpoint changed locally; partial data preserved".into(),
        ));
    }
    if d.total == Some(d.durable_bytes) {
        return Ok(());
    }
    let mut guard =
        connections::send_verification(engine, d, "bytes=0-0".into(), Some(&pin.etag), &mut stop)
            .await?;
    let res = &mut guard.response;
    if res.status() != reqwest::StatusCode::PARTIAL_CONTENT
        || crate::resume::strong_etag(res).as_ref() != Some(&pin.etag)
        || res.url().as_str() != pin.resolved_url
        || transfer::text(res, header::CONTENT_ENCODING)
            .is_some_and(|e| !e.eq_ignore_ascii_case("identity"))
        || transfer::parse_range(&transfer::text(res, header::CONTENT_RANGE).unwrap_or_default())?
            != (0, 0, pin.total)
    {
        return Err(Error::Protocol(
            "Verified replacement identity changed; partial data preserved. Update the link again."
                .into(),
        ));
    }
    let mut count = 0;
    while let Some(chunk) = res.chunk().await? {
        count += chunk.len();
        if count > 1 {
            break;
        }
    }
    if count != 1 {
        return Err(Error::Protocol(
            "Verified replacement probe body is invalid".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn oversized_checkpoints_are_not_approved_by_sampling() {
        let root = tempfile::tempdir().unwrap();
        let engine = Engine::open(root.path().join("metadata")).await.unwrap();
        let mut d = engine
            .create(
                "http://127.0.0.1:9/file".into(),
                root.path().into(),
                "fixture.bin".into(),
            )
            .await
            .unwrap();
        d.status = Status::Paused;
        d.total = Some(MAX_BYTES * 2);
        d.durable_bytes = MAX_BYTES + 1;
        let (r, pin) = inspect(&engine, &d, &d.url).await.unwrap();
        assert_eq!(r.outcome, RecoveryOutcome::CannotVerify);
        assert!(pin.is_none());
        assert_eq!(r.compared_bytes, 0);
        engine.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn network_error_source_chains_never_include_signed_urls() {
        let url = "http://127.0.0.1:0/file?token=sensitive-value";
        let raw = reqwest::get(url).await.unwrap_err();
        let error = Error::from(raw);
        use std::error::Error as _;
        assert!(!error.to_string().contains("sensitive-value"));
        assert!(
            !error
                .source()
                .unwrap()
                .to_string()
                .contains("sensitive-value")
        );
    }
}
