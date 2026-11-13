use crate::model::{PolicySettings, Priority};
use crate::{
    Download, Error, QueueConfig, Result, Status, TransferConfig, connections::Connections, ranges,
    storage::Storage, transfer,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{
    sync::{Mutex, watch},
    task::JoinHandle,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    Running,
    Pause,
    Cancel,
}
struct Active {
    stop: watch::Sender<Stop>,
    task: JoinHandle<Result<()>>,
}
pub(crate) struct Inner {
    pub client: reqwest::Client,
    pub storage: Storage,
    pub(crate) downloads: Mutex<BTreeMap<String, Download>>,
    pub(crate) automation: Mutex<crate::automation::AutomationState>,
    pub(crate) organization: Mutex<crate::organization::OrganizationConfig>,
    pub(crate) file_ops: std::sync::Mutex<BTreeMap<String, crate::file_operations::MoveLease>>,
    pub(crate) clock: Arc<dyn crate::automation::Clock>,
    pub(crate) automation_effective: Mutex<Option<u64>>,
    pub(crate) automation_error: Mutex<Option<String>>,
    active: Mutex<BTreeMap<String, Active>>,
    pub(crate) commands: Mutex<()>,
    pub(crate) recovery: std::sync::Mutex<BTreeMap<String, crate::recovery::Entry>>,
    closed: AtomicBool,
    queue_config: Mutex<QueueConfig>,
    pub(crate) policy: Mutex<PolicySettings>,
    pub connections: Arc<Connections>,
    pub bandwidth: crate::bandwidth::Bandwidth,
    pub(crate) events: watch::Sender<Vec<Download>>,
    _lock: std::fs::File,
    scheduler: Mutex<Option<JoinHandle<()>>>,
}
#[derive(Clone)]
pub struct Engine {
    pub(crate) inner: Arc<Inner>,
}
impl Engine {
    pub async fn open(data_directory: impl AsRef<Path>) -> Result<Self> {
        Self::open_internal(
            data_directory,
            Arc::new(crate::automation::SystemClock),
            true,
            None,
        )
        .await
    }
    /// The platform shell supplies its current IANA zone before any admissions.
    pub async fn open_in_timezone(data_directory: impl AsRef<Path>, zone: &str) -> Result<Self> {
        Self::open_internal(
            data_directory,
            Arc::new(crate::automation::SystemClock),
            true,
            Some(zone),
        )
        .await
    }
    /// Caller-controlled clock and explicit automation ticks for deterministic tests/embedding.
    pub async fn open_with_clock(
        data_directory: impl AsRef<Path>,
        clock: Arc<dyn crate::automation::Clock>,
    ) -> Result<Self> {
        Self::open_internal(data_directory, clock, false, None).await
    }
    /// Controlled clock plus an explicit shell zone, including startup transitions.
    pub async fn open_with_clock_in_timezone(
        data_directory: impl AsRef<Path>,
        clock: Arc<dyn crate::automation::Clock>,
        zone: &str,
    ) -> Result<Self> {
        Self::open_internal(data_directory, clock, false, Some(zone)).await
    }
    async fn open_internal(
        data_directory: impl AsRef<Path>,
        clock: Arc<dyn crate::automation::Clock>,
        automatic: bool,
        timezone: Option<&str>,
    ) -> Result<Self> {
        tokio::fs::create_dir_all(data_directory.as_ref()).await?;
        let directory = tokio::fs::canonicalize(data_directory.as_ref()).await?;
        let lock_path = directory.join("engine.lock");
        let lock = tokio::task::spawn_blocking(move || -> Result<std::fs::File> {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(lock_path)?;
            fs2::FileExt::try_lock_exclusive(&file)
                .map_err(|_| Error::Busy("another engine owns this metadata directory".into()))?;
            Ok(file)
        })
        .await
        .map_err(|e| Error::Worker(e.to_string()))??;
        let storage = Storage::open(directory.join("downloads.sqlite3")).await?;
        let organization = storage.organization(None).await?;
        organization.validate()?;
        crate::file_operations::recover_moves(&storage).await?;
        let mut automation = storage.automation(None).await?;
        automation.validate()?;
        let previous = automation.clone();
        let zone = timezone
            .unwrap_or(automation.system_timezone.as_deref().unwrap_or("UTC"))
            .to_owned();
        automation.follow_timezone(&zone, clock.now_ms())?;
        automation.validate()?;
        let mut downloads = BTreeMap::new();
        for mut d in storage.load().await? {
            if d.category_id.is_none() {
                d.category_id = Some(
                    crate::organization::classify(&d.filename, d.content_type.as_deref()).into(),
                );
            }
            if !organization.contains(d.category_id.as_deref().unwrap_or("other")) {
                return Err(Error::Storage(
                    "Download references an unknown category".into(),
                ));
            }
            if matches!(
                d.status,
                Status::Running | Status::Finalizing | Status::Completed
            ) {
                automation.record_started(&d.id);
            }
            if d.pause_origin.is_none() {
                d.pause_origin = Some(if d.status == Status::Paused && !automation.owns(&d.id) {
                    crate::PauseOrigin::User
                } else {
                    crate::PauseOrigin::Recovery
                });
            }
            d.downloaded = d.durable_bytes;
            d.speed = 0.0;
            d.current_speed = 0.0;
            d.smoothed_speed = 0.0;
            d.eta_seconds = None;
            d.retry_remaining_seconds = None;
            d.active_connections = 0;
            d.target_connections = 0;
            d.queue_position = None;
            d.config.validate()?;
            if d.segmented {
                let total = d
                    .total
                    .ok_or_else(|| Error::Storage("Segmented record missing total".into()))?;
                ranges::validate(&d.completed_ranges, total)?;
                if ranges::bytes(&d.completed_ranges) != d.durable_bytes {
                    return Err(Error::Storage("Range ledger/progress mismatch".into()));
                }
            }
            if !matches!(d.status, Status::Completed | Status::Cancelled) && d.sha256.is_some() {
                let valid = match &d.sha256 {
                    Some(expected) => transfer::hash_file(Path::new(&d.destination))
                        .await
                        .is_ok_and(|hash| &hash == expected),
                    None => false,
                };
                if valid {
                    d.status = Status::Completed;
                    let _ = tokio::fs::remove_file(&d.partial_path).await;
                } else {
                    d.status = Status::Paused;
                }
            } else if !matches!(d.status, Status::Completed | Status::Cancelled) {
                d.status = Status::Paused;
            }
            if matches!(d.status, Status::Completed | Status::Cancelled) {
                match tokio::fs::remove_file(&d.partial_path).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        d.error = Some(format!("Partial-file cleanup failed: {error}"));
                    }
                }
            }
            if d.status == Status::Cancelled
                && matches!(tokio::fs::symlink_metadata(&d.partial_path).await, Err(e) if e.kind() == std::io::ErrorKind::NotFound)
            {
                d.downloaded = 0;
                d.durable_bytes = 0;
                d.completed_ranges.clear();
            }
            if d.status == Status::Paused {
                match tokio::fs::metadata(&d.partial_path).await {
                    Ok(m)
                        if m.is_file()
                            && m.len()
                                >= d.completed_ranges
                                    .iter()
                                    .map(|r| r.end)
                                    .max()
                                    .unwrap_or(d.durable_bytes) => {}
                    _ => {
                        d.status = Status::Failed;
                        d.error = Some(
                            "Partial file is missing or shorter than its durable checkpoint".into(),
                        );
                    }
                }
            }
            storage.save(&d).await?;
            downloads.insert(d.id.clone(), d);
        }
        automation.detect_missed_on_open(clock.now_ms(), &downloads);
        automation.validate()?;
        if automation != previous {
            storage
                .automation(Some((automation.clone(), vec![], None)))
                .await?;
        }
        let queue_config = storage.config(None).await?;
        let policy = storage.policy(None).await?;
        policy.retry_policy.validate()?;
        validate_bandwidth(policy.global_bandwidth_limit)?;
        let bandwidth = crate::bandwidth::Bandwidth::new();
        bandwidth
            .set_global(automation.effective_limit(clock.now_ms(), policy.global_bandwidth_limit));
        for d in downloads.values_mut() {
            validate_bandwidth(d.bandwidth_limit)?;
            bandwidth.set_local(&d.id, d.bandwidth_limit);
            d.bandwidth_limited = bandwidth.is_capped(&d.id);
        }
        let initial = downloads.values().cloned().collect();
        let (events, _) = watch::channel(initial);
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .http1_only()
            .pool_max_idle_per_host(32)
            .pool_idle_timeout(std::time::Duration::from_secs(15))
            .connect_timeout(std::time::Duration::from_secs(20))
            .read_timeout(std::time::Duration::from_secs(30))
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .build()?;
        let effective_limit =
            automation.effective_limit(clock.now_ms(), policy.global_bandwidth_limit);
        let engine = Self {
            inner: Arc::new(Inner {
                client,
                storage,
                downloads: Mutex::new(downloads),
                automation: Mutex::new(automation),
                organization: Mutex::new(organization),
                file_ops: Default::default(),
                clock,
                automation_error: Mutex::new(None),
                automation_effective: Mutex::new(effective_limit),
                active: Mutex::new(BTreeMap::new()),
                commands: Mutex::new(()),
                recovery: Default::default(),
                closed: AtomicBool::new(false),
                queue_config: Mutex::new(queue_config.clone()),
                policy: Mutex::new(policy),
                connections: Connections::new(queue_config),
                bandwidth,
                events,
                _lock: lock,
                scheduler: Mutex::new(None),
            }),
        };
        for d in engine.list().await {
            if let Some(until) = d.retry_not_before_ms {
                let remaining = until.saturating_sub(crate::connections::deadline_ms(0));
                if remaining > 0
                    && let Ok(url) =
                        reqwest::Url::parse(d.resolved_url.as_deref().unwrap_or(&d.url))
                {
                    engine.inner.connections.throttle(
                        url.host_str().unwrap_or_default(),
                        std::time::Duration::from_millis(remaining.min(86400 * 365 * 1000)),
                    );
                }
            }
        }
        let weak = Arc::downgrade(&engine.inner);
        let scheduler = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut automation_tick =
                tokio::time::Instant::now() - std::time::Duration::from_secs(2);
            loop {
                interval.tick().await;
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                let engine = Engine { inner };
                if engine.is_shutdown() {
                    break;
                }
                let _command = engine.inner.commands.lock().await;
                if !engine.is_shutdown() {
                    if automatic && automation_tick.elapsed() >= std::time::Duration::from_secs(1) {
                        automation_tick = tokio::time::Instant::now();
                        let result = engine.automation_tick_locked().await;
                        *engine.inner.automation_error.lock().await =
                            result.err().map(|e| e.to_string());
                    }
                    if let Err(error) = engine.schedule_locked().await {
                        *engine.inner.automation_error.lock().await = Some(error.to_string());
                    }
                }
            }
        });
        *engine.inner.scheduler.lock().await = Some(scheduler);
        Ok(engine)
    }
    pub(crate) fn ensure_open(&self) -> Result<()> {
        if self.inner.closed.load(Ordering::Acquire) {
            Err(Error::Closed)
        } else {
            Ok(())
        }
    }
    pub async fn list(&self) -> Vec<Download> {
        let mut items: Vec<_> = self
            .inner
            .downloads
            .lock()
            .await
            .values()
            .cloned()
            .collect();
        items.sort_by_key(|d| d.queue_order);
        items
    }
    pub async fn get(&self, id: &str) -> Result<Download> {
        self.inner
            .downloads
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| Error::NotFound(id.into()))
    }
    pub fn is_shutdown(&self) -> bool {
        self.inner.closed.load(Ordering::Acquire)
    }
    pub fn subscribe(&self) -> watch::Receiver<Vec<Download>> {
        self.inner.events.subscribe()
    }
    pub(crate) async fn publish(&self, download: &Download) {
        let mut map = self.inner.downloads.lock().await;
        let mut download = download.clone();
        let previous = map.get(&download.id);
        if let Some(previous) = previous {
            download.schedule_state = previous.schedule_state.clone();
            download.bandwidth_limit = previous.bandwidth_limit;
            if previous.category_manual {
                download.category_id = previous.category_id.clone();
                download.category_manual = true;
            }
        }
        download.bandwidth_limited = self.inner.bandwidth.is_capped(&download.id);
        download.current_speed = if download.status == Status::Running && download.speed.is_finite()
        {
            download.speed.max(0.0)
        } else {
            0.0
        };
        download.smoothed_speed = if download.current_speed > 0.0 {
            let old = previous.map_or(0.0, |d| d.smoothed_speed);
            let same_sample = download.telemetry.rate_sequence > 0
                && previous.is_some_and(|d| {
                    d.telemetry.generation == download.telemetry.generation
                        && d.telemetry.rate_sequence == download.telemetry.rate_sequence
                });
            if same_sample {
                old
            } else if old > 0.0 {
                old * 0.75 + download.current_speed * 0.25
            } else {
                download.current_speed
            }
        } else {
            0.0
        };
        download.retry_remaining_seconds = download
            .retry_not_before_ms
            .filter(|_| download.status == Status::Running)
            .and_then(|deadline| deadline.checked_sub(crate::connections::deadline_ms(0)))
            .filter(|remaining| *remaining > 0)
            .map(|remaining| remaining.div_ceil(1000));
        download.eta_seconds = if download.status == Status::Running
            && download.retry_remaining_seconds.is_none()
            && download.smoothed_speed > 0.0
        {
            download.total.map(|total| {
                ((total.saturating_sub(download.downloaded)) as f64 / download.smoothed_speed)
                    .ceil() as u64
            })
        } else {
            None
        };
        if download.status == Status::Completed {
            download.eta_seconds = Some(0);
        }
        if download.telemetry.workers.is_empty() {
            download.active_connections = self.inner.connections.active(&download.id);
        }
        map.insert(download.id.clone(), download);
        let mut queued: Vec<_> = map
            .values()
            .filter(|d| d.status == Status::Queued)
            .map(|d| (queue_key(d), d.id.clone()))
            .collect();
        queued.sort();
        for d in map.values_mut() {
            d.queue_position = None;
        }
        for (index, (_, id)) in queued.iter().enumerate() {
            if let Some(d) = map.get_mut(id) {
                d.queue_position = Some(index as u32 + 1);
            }
        }
        self.inner
            .events
            .send_replace(map.values().cloned().collect());
    }
    pub(crate) async fn persist(&self, download: &Download) -> Result<()> {
        let mut download = download.clone();
        let map = self.inner.downloads.lock().await;
        if let Some(current) = map.get(&download.id) {
            download.bandwidth_limit = current.bandwidth_limit;
            if current.category_manual {
                download.category_id = current.category_id.clone();
                download.category_manual = true;
            }
        }
        self.inner.storage.save(&download).await?;
        drop(map);
        self.publish(&download).await;
        Ok(())
    }
    pub async fn create(
        &self,
        url: String,
        directory: PathBuf,
        filename: String,
    ) -> Result<Download> {
        let _command = self.inner.commands.lock().await;
        self.ensure_open()?;
        self.create_with_settings_locked(
            url,
            directory,
            filename,
            crate::CreationSettings::default(),
        )
        .await
    }
    pub async fn create_with_settings(
        &self,
        url: String,
        directory: PathBuf,
        filename: String,
        settings: crate::CreationSettings,
    ) -> Result<Download> {
        let _command = self.inner.commands.lock().await;
        self.ensure_open()?;
        self.create_with_settings_locked(url, directory, filename, settings)
            .await
    }
    async fn create_with_settings_locked(
        &self,
        url: String,
        directory: PathBuf,
        filename: String,
        settings: crate::CreationSettings,
    ) -> Result<Download> {
        settings.transfer.validate()?;
        validate_bandwidth(settings.bandwidth_limit)?;
        let (directory, filename, category) = self
            .prepare_creation(&url, directory, filename, &settings)
            .await?;
        let mut d = self.create_record_locked(url, directory, filename).await?;
        d.category_id = Some(category);
        d.category_manual = settings.category_id.is_some();
        d.content_type = settings.content_type_hint;
        d.config = settings.transfer;
        d.bandwidth_limit = settings.bandwidth_limit;
        if let Err(error) = self.inner.storage.save(&d).await {
            let _ = tokio::fs::remove_file(&d.partial_path).await;
            return Err(error);
        }
        self.inner.bandwidth.set_local(&d.id, d.bandwidth_limit);
        self.publish(&d).await;
        Ok(d)
    }
    /// Caller holds commands; the candidate remains invisible until its transaction commits.
    pub(crate) async fn create_record_locked(
        &self,
        url: String,
        directory: PathBuf,
        filename: String,
    ) -> Result<Download> {
        let parsed = reqwest::Url::parse(&url).map_err(|e| Error::Invalid(e.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(Error::Invalid(
                "Use an HTTP/HTTPS URL without embedded credentials".into(),
            ));
        }
        crate::organization::validate_filename(&filename)?;
        let directory = tokio::fs::canonicalize(directory).await?;
        if !tokio::fs::metadata(&directory).await?.is_dir() {
            return Err(Error::Invalid("Destination must be a directory".into()));
        }
        let destination = directory.join(&filename);
        self.guard_move_target(&destination)?;
        match tokio::fs::symlink_metadata(&destination).await {
            Ok(_) => {
                return Err(Error::Invalid(
                    "Destination already exists; choose another filename".into(),
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        if self.list().await.iter().any(|d| {
            d.destination.to_lowercase() == destination.to_string_lossy().to_lowercase()
                && d.status != Status::Cancelled
        }) {
            return Err(Error::Invalid(
                "Another download already uses this destination".into(),
            ));
        }
        if self.list().await.len() >= 10_000 {
            return Err(Error::Busy(
                "Download history limit reached (10,000)".into(),
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let partial_path = directory.join(format!(".dlm-{id}.part"));
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial_path)
            .await?;
        file.sync_all().await?;
        transfer::sync_parent(&partial_path).await?;
        let d = Download {
            category_id: Some(crate::organization::classify(&filename, None).into()),
            category_manual: false,
            content_type: None,
            ever_started: false,
            pause_origin: None,
            automation_strict_resume: false,
            schedule_state: None,
            recovery_pin: None,
            last_http_status: None,
            created_at_ms: Some(crate::connections::deadline_ms(0)),
            telemetry: Default::default(),
            resume: Default::default(),
            priority: Priority::Normal,
            admission_bypasses: 0,
            bandwidth_limit: None,
            current_speed: 0.0,
            smoothed_speed: 0.0,
            eta_seconds: None,
            retry_remaining_seconds: None,
            error_kind: None,
            bandwidth_limited: false,
            config: TransferConfig::default(),
            completed_ranges: vec![],
            segmented: false,
            resolved_url: None,
            queue_order: self
                .list()
                .await
                .iter()
                .map(|d| d.queue_order)
                .max()
                .unwrap_or(0)
                + 1,
            queue_position: None,
            active_connections: 0,
            target_connections: 0,
            retries: 0,
            notice: None,
            retry_not_before_ms: None,
            id,
            url,
            filename,
            destination: destination.to_string_lossy().into(),
            partial_path: partial_path.to_string_lossy().into(),
            status: Status::Ready,
            downloaded: 0,
            durable_bytes: 0,
            total: None,
            speed: 0.0,
            etag: None,
            last_modified: None,
            sha256: None,
            error: None,
        };
        Ok(d)
    }
    pub async fn configure_download(&self, id: &str, config: TransferConfig) -> Result<()> {
        let _command = self.inner.commands.lock().await;
        self.ensure_open()?;
        config.validate()?;
        self.guard_recovery(id)?;
        let mut d = self.get(id).await?;
        if matches!(
            d.status,
            Status::Running | Status::Finalizing | Status::Queued
        ) {
            return Err(Error::Busy(
                "Pause before changing transfer settings".into(),
            ));
        }
        self.settle_worker(id).await?;
        d = self.get(id).await?;
        d.config = config;
        self.persist(&d).await
    }
    pub async fn queue_config(&self) -> QueueConfig {
        self.inner.queue_config.lock().await.clone()
    }
    pub async fn retry_policy(&self) -> crate::RetryPolicy {
        self.inner.policy.lock().await.retry_policy.clone()
    }
    pub async fn global_bandwidth_limit(&self) -> Option<u64> {
        self.inner.policy.lock().await.global_bandwidth_limit
    }
    pub async fn set_global_bandwidth_limit(&self, limit: Option<u64>) -> Result<()> {
        let _command = self.inner.commands.lock().await;
        self.ensure_open()?;
        validate_bandwidth(limit)?;
        let mut policy = self.inner.policy.lock().await;
        let mut updated = policy.clone();
        updated.global_bandwidth_limit = limit;
        let mut state = self.inner.automation.lock().await.clone();
        state.bandwidth_override = Some(state.bandwidth_key(self.inner.clock.now_ms()));
        self.inner
            .storage
            .automation(Some((state.clone(), vec![], Some(updated.clone()))))
            .await?;
        let effective = state.effective_limit(self.inner.clock.now_ms(), limit);
        self.inner.bandwidth.set_global(effective);
        *self.inner.automation_effective.lock().await = effective;
        *self.inner.automation.lock().await = state;
        *policy = updated;
        drop(policy);
        // Refresh only policy-derived fields under the authoritative map lock.
        // Re-publishing cloned snapshots can roll back concurrent worker progress.
        let mut map = self.inner.downloads.lock().await;
        for d in map.values_mut() {
            d.bandwidth_limited = self.inner.bandwidth.is_capped(&d.id);
        }
        self.inner
            .events
            .send_replace(map.values().cloned().collect());
        Ok(())
    }
    pub async fn set_download_bandwidth_limit(&self, id: &str, limit: Option<u64>) -> Result<()> {
        let _command = self.inner.commands.lock().await;
        self.ensure_open()?;
        validate_bandwidth(limit)?;
        self.guard_recovery(id)?;
        let mut map = self.inner.downloads.lock().await;
        let mut d = map
            .get(id)
            .cloned()
            .ok_or_else(|| Error::NotFound(id.into()))?;
        d.bandwidth_limit = limit;
        self.inner.storage.save(&d).await?;
        self.inner.bandwidth.set_local(id, limit);
        d.bandwidth_limited = self.inner.bandwidth.is_capped(id);
        map.insert(id.into(), d.clone());
        self.inner
            .events
            .send_replace(map.values().cloned().collect());
        Ok(())
    }
    pub async fn configure_retry_policy(&self, retry_policy: crate::RetryPolicy) -> Result<()> {
        let _command = self.inner.commands.lock().await;
        self.ensure_open()?;
        retry_policy.validate()?;
        let mut policy = self.inner.policy.lock().await;
        let mut updated = policy.clone();
        updated.retry_policy = retry_policy;
        self.inner.storage.policy(Some(updated.clone())).await?;
        *policy = updated;
        Ok(())
    }
    pub async fn configure_queue(&self, config: QueueConfig) -> Result<()> {
        let _command = self.inner.commands.lock().await;
        self.ensure_open()?;
        config.validate()?;
        let old = self.queue_config().await;
        self.inner.connections.configure(config.clone())?;
        if let Err(error) = self.inner.storage.config(Some(config.clone())).await {
            let _ = self.inner.connections.configure(old);
            return Err(error);
        }
        *self.inner.queue_config.lock().await = config;
        self.schedule_locked().await
    }
    pub async fn start(&self, id: &str) -> Result<()> {
        let _command = self.inner.commands.lock().await;
        let mut d = self.prepare_enqueue_locked(id).await?;
        d.pause_origin = None;
        self.commit_manual_start(d).await?;
        self.schedule_locked().await
    }
    pub(crate) async fn prepare_enqueue_locked(&self, id: &str) -> Result<Download> {
        self.ensure_open()?;
        self.guard_recovery(id)?;
        let mut d = self.get(id).await?;
        if matches!(
            d.status,
            Status::Running | Status::Finalizing | Status::Queued
        ) {
            return Err(Error::Busy(id.into()));
        }
        if matches!(d.status, Status::Completed | Status::Cancelled) {
            return Err(Error::Invalid(
                "Completed/cancelled downloads cannot start".into(),
            ));
        }
        self.settle_worker(id).await?;
        d = self.get(id).await?;
        if self
            .list()
            .await
            .iter()
            .filter(|d| d.status == Status::Queued)
            .count()
            >= 1000
        {
            return Err(Error::Busy("Queue limit reached (1,000)".into()));
        }
        d.queue_order = self
            .list()
            .await
            .iter()
            .map(|d| d.queue_order)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("Queue order exhausted".into()))?;
        d.status = Status::Queued;
        d.error = None;
        d.error_kind = None;
        Ok(d)
    }
    pub(crate) async fn settle_worker(&self, id: &str) -> Result<()> {
        let previous = self.inner.active.lock().await.remove(id);
        if let Some(previous) = previous {
            previous
                .task
                .await
                .map_err(|e| Error::Worker(e.to_string()))??;
        }
        Ok(())
    }
    pub(crate) async fn schedule_locked(&self) -> Result<()> {
        let mut active = self.inner.active.lock().await;
        let finished: Vec<_> = active
            .iter()
            .filter(|(_, a)| a.task.is_finished())
            .map(|(id, _)| id.clone())
            .collect();
        for id in finished {
            let task = active.remove(&id).expect("known worker");
            if let Err(error) = task
                .task
                .await
                .map_err(|e| Error::Worker(e.to_string()))
                .and_then(|result| result)
            {
                let mut d = self.get(&id).await?;
                d.status = Status::Failed;
                d.error = Some(error.to_string());
                self.publish(&d).await;
            }
        }
        if self.is_shutdown() {
            return Ok(());
        }
        let maximum = self.inner.queue_config.lock().await.max_downloads as usize;
        let mut automation = self.inner.automation.lock().await.clone();
        let permitted = automation.permitted(self.inner.clock.now_ms());
        let mut queued: Vec<_> = self
            .list()
            .await
            .into_iter()
            .filter(|d| {
                d.status == Status::Queued
                    && (automation.holds.contains(&d.id)
                        || permitted.get(&d.id).copied().unwrap_or(true))
            })
            .collect();
        for _ in 0..maximum.saturating_sub(active.len()) {
            if queued.is_empty() {
                break;
            }
            queued.sort_by_key(queue_key);
            let mut d = queued.remove(0);
            if active.contains_key(&d.id) {
                continue;
            }
            d.status = Status::Running;
            d.ever_started = true;
            d.admission_bypasses = 0;
            d.error = None;
            d.retries = 0;
            // Every admission ages all waiting records. After eight bypasses a
            // record is admitted FIFO ahead of priorities, even across restarts.
            for waiting in &mut queued {
                waiting.admission_bypasses = waiting.admission_bypasses.saturating_add(1);
            }
            let mut changed = queued.clone();
            changed.push(d.clone());
            automation.record_started(&d.id);
            self.commit_automation(automation.clone(), changed, None)
                .await?;
            let (stop, rx) = watch::channel(Stop::Running);
            let engine = self.clone();
            let id = d.id.clone();
            let task = tokio::spawn(async move {
                let mut rx = rx;
                let mut attempt = 1;
                let result = loop {
                    if *rx.borrow() != Stop::Running {
                        break Err(Error::Stopped);
                    }
                    if let Some(deadline) = d.retry_not_before_ms {
                        let now = crate::connections::deadline_ms(0);
                        if deadline > now {
                            d.speed = 0.0;
                            d.current_speed = 0.0;
                            d.active_connections = 0;
                            engine.publish(&d).await;
                            tokio::select! {
                                _ = rx.changed() => { break Err(Error::Stopped); }
                                _ = tokio::time::sleep(std::time::Duration::from_millis((deadline - now).min(250))) => {}
                            }
                            continue;
                        }
                    }
                    d.retry_not_before_ms = None;
                    d.retry_remaining_seconds = None;
                    d.error = None;
                    d.error_kind = None;
                    d.last_http_status = None;
                    d.telemetry.begin();
                    d.resume
                        .invalidate(crate::ResumeReason::RevalidationRequired);
                    engine.publish(&d).await;
                    let invocation = transfer::run(&engine, &mut d, rx.clone()).await;
                    d.telemetry.clear();
                    match invocation {
                        Ok(()) => break Ok(()),
                        Err(error) => {
                            d.last_http_status = match &error {
                                Error::Http { status, .. } => Some(*status),
                                _ => None,
                            };
                            if matches!(
                                &error,
                                Error::Http {
                                    status: 401 | 403,
                                    ..
                                }
                            ) {
                                d.resume
                                    .unknown(crate::ResumeReason::AuthenticationRequired);
                            } else if matches!(&error, Error::Protocol(_))
                                && !matches!(
                                    d.resume.reason,
                                    crate::ResumeReason::IdentityChanged
                                        | crate::ResumeReason::ConditionalFullResponse
                                )
                            {
                                d.resume.unknown(crate::ResumeReason::InvalidRangeResponse);
                            }
                            // Retain server cooldown even when this attempt exhausts
                            // the budget. Explicit Retry/restart must not bypass it.
                            if let Error::Http {
                                retry_after_secs, ..
                            } = &error
                                && *retry_after_secs > 0
                                && *retry_after_secs <= crate::retry::MAX_RETRY_AFTER_SECS
                            {
                                d.retry_not_before_ms = Some(
                                    crate::connections::deadline_ms(*retry_after_secs)
                                        .max(d.retry_not_before_ms.unwrap_or(0)),
                                );
                            }
                            d.error_kind = Some(match &error {
                                Error::Network(_) => crate::ErrorKind::Network,
                                Error::Http {
                                    status: 401 | 403, ..
                                } => crate::ErrorKind::Authentication,
                                Error::Http {
                                    status: 429 | 503, ..
                                } => crate::ErrorKind::RateLimited,
                                Error::Http { .. } => crate::ErrorKind::Http,
                                Error::Disk(_) => crate::ErrorKind::Disk,
                                Error::Storage(_) => crate::ErrorKind::Storage,
                                Error::Protocol(_) => crate::ErrorKind::Integrity,
                                Error::Invalid(_) => crate::ErrorKind::Invalid,
                                _ => crate::ErrorKind::Internal,
                            });
                            let policy = engine.retry_policy().await;
                            let sample = uuid::Uuid::new_v4().as_u128() as u64;
                            let now = crate::connections::deadline_ms(0);
                            let Some(deadline) = policy.deadline_ms(&error, attempt, now, sample)
                            else {
                                break Err(error);
                            };
                            d.retry_not_before_ms =
                                Some(deadline.max(d.retry_not_before_ms.unwrap_or(0)));
                            d.retries = d.retries.saturating_add(1);
                            d.error = Some(error.to_string());
                            d.downloaded = d.durable_bytes;
                            d.speed = 0.0;
                            d.active_connections = 0;
                            if let Err(error) = engine.persist(&d).await {
                                break Err(error);
                            }
                            attempt += 1;
                        }
                    }
                };
                let persistence = if let Err(error) = result {
                    d.status = if matches!(error, Error::Stopped) {
                        if *rx.borrow() == Stop::Cancel {
                            Status::Cancelled
                        } else {
                            Status::Paused
                        }
                    } else {
                        d.error = Some(error.to_string());
                        Status::Failed
                    };
                    d.speed = 0.0;
                    d.current_speed = 0.0;
                    d.eta_seconds = None;
                    d.retry_remaining_seconds = None;
                    if *rx.borrow() == Stop::Cancel {
                        d.retry_not_before_ms = None;
                    }
                    d.downloaded = d.durable_bytes;
                    d.active_connections = 0;
                    engine.persist(&d).await
                } else {
                    Ok(())
                };
                engine.inner.connections.target(&d.id, None);
                engine.publish(&d).await;
                persistence
            });
            active.insert(id, Active { stop, task });
        }
        Ok(())
    }
    pub async fn resume(&self, id: &str) -> Result<()> {
        self.start(id).await
    }
    pub async fn retry(&self, id: &str) -> Result<()> {
        self.start(id).await
    }
    pub async fn set_priority(&self, id: &str, priority: Priority) -> Result<()> {
        let _command = self.inner.commands.lock().await;
        self.ensure_open()?;
        self.guard_recovery(id)?;
        let mut d = self.get(id).await?;
        if matches!(d.status, Status::Running | Status::Finalizing) {
            return Err(Error::Busy("Pause before changing priority".into()));
        }
        self.settle_worker(id).await?;
        d = self.get(id).await?;
        d.priority = priority;
        self.persist(&d).await
    }
    /// Reorder all currently queued records atomically. Priority remains the
    /// primary ordering key until persistent admission aging takes precedence.
    pub async fn reorder_queue(&self, ids: Vec<String>) -> Result<()> {
        let _command = self.inner.commands.lock().await;
        self.ensure_open()?;
        let mut queued: BTreeMap<_, _> = self
            .list()
            .await
            .into_iter()
            .filter(|d| d.status == Status::Queued)
            .map(|d| (d.id.clone(), d))
            .collect();
        if ids.len() != queued.len() {
            return Err(Error::Invalid("Supply every queued ID exactly once".into()));
        }
        let base = self
            .list()
            .await
            .iter()
            .map(|d| d.queue_order)
            .max()
            .unwrap_or(0);
        let mut changed = Vec::with_capacity(ids.len());
        for (index, id) in ids.into_iter().enumerate() {
            let mut d = queued
                .remove(&id)
                .ok_or_else(|| Error::Invalid("Supply every queued ID exactly once".into()))?;
            d.queue_order = base
                .checked_add(index as u64 + 1)
                .ok_or_else(|| Error::Invalid("Queue order exhausted".into()))?;
            changed.push(d);
        }
        self.inner.storage.save_batch(changed.clone()).await?;
        for d in changed {
            self.publish(&d).await;
        }
        Ok(())
    }
    pub async fn pause(&self, id: &str) -> Result<()> {
        self.stop(id, Stop::Pause).await
    }
    pub async fn cancel(&self, id: &str) -> Result<()> {
        self.stop(id, Stop::Cancel).await
    }
    async fn stop(&self, id: &str, reason: Stop) -> Result<()> {
        let _command = self.inner.commands.lock().await;
        self.ensure_open()?;
        self.guard_recovery(id)?;
        self.manual_override_locked(id).await?;
        self.stop_locked(id, reason).await?;
        let mut d = self.get(id).await?;
        if d.status == Status::Paused {
            d.pause_origin = Some(crate::PauseOrigin::User);
            self.persist(&d).await?;
        }
        Ok(())
    }
    pub(crate) async fn stop_locked(&self, id: &str, reason: Stop) -> Result<()> {
        self.guard_recovery(id)?;
        if let Some(active) = self.inner.active.lock().await.remove(id) {
            active.stop.send_replace(reason);
            active
                .task
                .await
                .map_err(|e| Error::Worker(e.to_string()))??;
        }
        let mut d = self.get(id).await?;
        if d.status != Status::Completed
            && (reason == Stop::Cancel || d.status != Status::Cancelled)
        {
            d.status = if reason == Stop::Cancel {
                Status::Cancelled
            } else {
                Status::Paused
            };
            d.speed = 0.0;
            if reason == Stop::Cancel {
                match tokio::fs::remove_file(&d.partial_path).await {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
            if reason == Stop::Cancel {
                d.downloaded = 0;
                d.durable_bytes = 0;
                d.completed_ranges.clear();
                d.telemetry = Default::default();
                d.resume = Default::default();
                d.active_connections = 0;
                d.current_speed = 0.0;
                d.smoothed_speed = 0.0;
                d.eta_seconds = None;
            }
            self.persist(&d).await?;
        }
        Ok(())
    }
    /// Restart a cancelled record from zero, never reusing deleted checkpoints.
    pub async fn restart_cancelled(&self, id: &str) -> Result<()> {
        let _command = self.inner.commands.lock().await;
        self.ensure_open()?;
        self.guard_recovery(id)?;
        let mut d = self.get(id).await?;
        if d.status != Status::Cancelled {
            return Err(Error::Invalid(
                "Only cancelled downloads can restart from zero".into(),
            ));
        }
        match tokio::fs::symlink_metadata(&d.destination).await {
            Ok(_) => return Err(Error::Invalid("Destination already exists".into())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&d.partial_path)
            .await?;
        file.sync_all().await?;
        transfer::sync_parent(Path::new(&d.partial_path)).await?;
        d.downloaded = 0;
        d.durable_bytes = 0;
        d.completed_ranges.clear();
        d.segmented = false;
        d.recovery_pin = None;
        d.automation_strict_resume = false;
        d.last_http_status = None;
        d.total = None;
        d.etag = None;
        d.last_modified = None;
        d.resolved_url = None;
        d.sha256 = None;
        d.error = None;
        d.error_kind = None;
        d.notice = None;
        d.retries = 0;
        d.telemetry = Default::default();
        d.resume = Default::default();
        d.status = Status::Ready;
        // Leave a safely restartable Ready record if later admission fails.
        if let Err(error) = self.persist(&d).await {
            // This invocation created the empty partial exclusively. Roll back only
            // that new artifact; an older checkpoint is never touched here.
            drop(file);
            tokio::fs::remove_file(&d.partial_path).await?;
            transfer::sync_parent(Path::new(&d.partial_path)).await?;
            return Err(error);
        }
        Ok(())
    }

    /// Metadata-only removal. Refuse if any partial artifact is still present.
    /// Never remove a destination, partial, symlink or unrelated file here.
    pub async fn remove_cancelled(&self, id: &str) -> Result<()> {
        let _command = self.inner.commands.lock().await;
        self.ensure_open()?;
        self.guard_recovery(id)?;
        let d = self.get(id).await?;
        if d.status != Status::Cancelled {
            return Err(Error::Invalid(
                "Only cancelled records can be removed".into(),
            ));
        }
        match tokio::fs::symlink_metadata(&d.partial_path).await {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
            Ok(_) => return Err(Error::Invalid(
                "Partial data still exists; cancel again to clean it up before removing the record"
                    .into(),
            )),
        }
        self.inner.storage.delete(id).await?;
        self.inner.downloads.lock().await.remove(id);
        self.inner.bandwidth.set_local(id, None);
        let snapshot = self.list().await;
        self.inner.events.send_replace(snapshot);
        Ok(())
    }

    pub async fn shutdown(&self) -> Result<()> {
        let _command = self.inner.commands.lock().await;
        self.inner.closed.store(true, Ordering::Release);
        if let Some(scheduler) = self.inner.scheduler.lock().await.take() {
            scheduler.abort();
            let _ = scheduler.await;
        }
        let active = std::mem::take(&mut *self.inner.active.lock().await);
        for task in active.values() {
            task.stop.send_replace(Stop::Pause);
        }
        let moves: Vec<_> = self
            .inner
            .file_ops
            .lock()
            .unwrap()
            .values()
            .map(|m| m.done.subscribe())
            .collect();
        for mut done in moves {
            while !*done.borrow() {
                if done.changed().await.is_err() {
                    break;
                }
            }
        }
        let mut first_error = None;
        for mut d in self
            .list()
            .await
            .into_iter()
            .filter(|d| d.status == Status::Queued)
        {
            d.status = Status::Paused;
            d.queue_position = None;
            if let Err(error) = self.persist(&d).await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        for (_, task) in active {
            let result = task
                .task
                .await
                .map_err(|e| Error::Worker(e.to_string()))
                .and_then(|r| r);
            if let Err(error) = result
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        let mut automation = self.inner.automation.lock().await.clone();
        automation.last_seen_ms = Some(self.inner.clock.now_ms());
        if let Err(error) = self.commit_automation(automation, vec![], None).await
            && first_error.is_none()
        {
            first_error = Some(error);
        }
        if let Some(error) = first_error {
            Err(error)
        } else {
            Ok(())
        }
    }
}

fn queue_key(d: &Download) -> (bool, Priority, u64, String) {
    let aged = d.admission_bypasses >= 8;
    (
        !aged,
        if aged { Priority::High } else { d.priority },
        d.queue_order,
        d.id.clone(),
    )
}
pub(crate) fn validate_bandwidth(limit: Option<u64>) -> Result<()> {
    if limit == Some(0) {
        return Err(Error::Invalid(
            "Bandwidth must be positive bytes/second or unlimited".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod metric_tests {
    use super::*;
    #[tokio::test]
    async fn authoritative_metrics_reset_on_stop_and_unknown_totals() {
        let dir = tempfile::TempDir::new().unwrap();
        let engine = Engine::open(dir.path().join("metadata")).await.unwrap();
        let mut d = engine
            .create(
                "http://localhost/test".into(),
                dir.path().into(),
                "metrics".into(),
            )
            .await
            .unwrap();
        d.status = Status::Running;
        d.total = Some(1000);
        d.downloaded = 100;
        d.speed = 100.0;
        d.telemetry.rate_sequence = 1;
        engine.publish(&d).await;
        let current = engine.get(&d.id).await.unwrap();
        assert_eq!(current.current_speed, 100.0);
        assert_eq!(current.smoothed_speed, 100.0);
        assert_eq!(current.eta_seconds, Some(9));
        d.speed = 200.0;
        d.telemetry.rate_sequence = 2;
        engine.publish(&d).await;
        assert_eq!(engine.get(&d.id).await.unwrap().smoothed_speed, 125.0);
        engine.publish(&d).await;
        assert_eq!(
            engine.get(&d.id).await.unwrap().smoothed_speed,
            125.0,
            "Unrelated publications must not reapply the same rate sample"
        );
        d.retry_not_before_ms = Some(crate::connections::deadline_ms(2));
        engine.publish(&d).await;
        let waiting = engine.get(&d.id).await.unwrap();
        assert_eq!(waiting.retry_remaining_seconds, Some(2));
        assert_eq!(waiting.eta_seconds, None);
        d.retry_not_before_ms = None;
        d.total = None;
        engine.publish(&d).await;
        assert_eq!(engine.get(&d.id).await.unwrap().eta_seconds, None);
        d.status = Status::Paused;
        engine.publish(&d).await;
        let stopped = engine.get(&d.id).await.unwrap();
        assert_eq!(stopped.current_speed, 0.0);
        assert_eq!(stopped.smoothed_speed, 0.0);
        assert_eq!(stopped.eta_seconds, None);
        assert_eq!(stopped.retry_remaining_seconds, None);
        engine.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn shutdown_signals_transfers_before_waiting_for_move_verification() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let e = Engine::open(root.join("meta")).await.unwrap();
        let completed = e
            .create(
                "https://example.org/moving".into(),
                root.clone(),
                "moving.bin".into(),
            )
            .await
            .unwrap();
        let target = root.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/stalled", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut sockets = vec![];
            while let Ok((socket, _)) = listener.accept().await {
                sockets.push(socket);
            }
        });
        let d = e
            .create(url, root.clone(), "active.bin".into())
            .await
            .unwrap();
        e.start(&d.id).await.unwrap();
        let mut stop = e
            .inner
            .active
            .lock()
            .await
            .get(&d.id)
            .unwrap()
            .stop
            .subscribe();
        let (done, _) = watch::channel(false);
        e.inner.file_ops.lock().unwrap().insert(
            completed.id.clone(),
            crate::file_operations::MoveLease {
                target: target.join(&completed.filename),
                done: done.clone(),
            },
        );
        let engine = e.clone();
        let shutdown = tokio::spawn(async move { engine.shutdown().await });
        tokio::time::timeout(std::time::Duration::from_secs(2), stop.changed())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(*stop.borrow(), crate::engine::Stop::Pause);
        assert!(!shutdown.is_finished());
        done.send_replace(true);
        shutdown.await.unwrap().unwrap();
        assert_eq!(e.get(&d.id).await.unwrap().status, Status::Paused);
        server.abort();
    }
}
