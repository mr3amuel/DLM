//! Versioned native desktop C ABI. All scheduling, HTTP and durable state live in download_core.
use download_core::{
    BatchAction, CreationSettings, Download, DuplicateQuery, Engine, LocalScheduleDate, Priority,
    QueueConfig, ResumeCapability, ScheduleKind, ScheduleRule, Status, TransferConfig,
};
use serde::{Deserialize, Serialize};
use std::{
    ffi::{CString, c_char},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    ptr,
};
use tokio::{runtime::Runtime, sync::watch};

#[path = "../../../../crates/download_bridge/src/browser_inbox.rs"]
mod browser_inbox;
#[path = "../../../../crates/download_bridge/src/browser_protocol.rs"]
#[allow(dead_code)] // Also used by the standalone browser host.
mod browser_protocol;

pub struct DlmExperiment {
    inbox: Option<browser_inbox::Inbox>,
    inbox_path: PathBuf,
    engine: Engine,
    directory: PathBuf,
    runtime: Runtime,
    snapshots: watch::Receiver<Vec<Download>>,
    first_snapshot: bool,
    faulted: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Open {
    root: PathBuf,
    timezone: String,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Snapshot {},
    BrowserState {},
    BrowserEnable {
        enabled: bool,
    },
    BrowserPairingCode {},
    BrowserReset {},
    BrowserTake {},
    BrowserAcknowledge {
        id: String,
    },
    VerifyLink {
        id: String,
        url: String,
    },
    ApplyLink {
        id: String,
        token: String,
        restart: bool,
    },
    RestartCancelled {
        id: String,
    },
    InspectSource {
        url: String,
        filename: String,
    },
    PartialInfo {
        id: String,
    },
    InspectDraft {
        draft: Draft,
    },
    Add {
        draft: Draft,
        separate_copy: bool,
        schedule: Option<ScheduleInput>,
    },
    Retry {
        id: String,
    },
    Rename {
        id: String,
        filename: String,
    },
    Move {
        id: String,
        directory: PathBuf,
    },
    Priority {
        id: String,
        priority: Priority,
    },
    Reorder {
        ids: Vec<String>,
    },
    Configure {
        id: String,
        connections: u32,
        optimize: bool,
    },
    SpeedLimit {
        id: String,
        limit: Option<u64>,
    },
    QueueSettings {},
    SetRetry {
        policy: download_core::RetryPolicy,
    },
    Schedules {},
    SaveSchedule {
        schedule: ScheduleInput,
    },
    EnableSchedule {
        id: String,
        enabled: bool,
    },
    DeleteSchedule {
        id: String,
    },
    RearmSchedule {
        id: String,
    },
    DismissMissed {
        occurrences: Vec<(String, i64)>,
    },
    SetQueue {
        queue: QueueConfig,
    },
    GlobalLimit {
        limit: Option<u64>,
    },
    Organization {},
    SetOrganization {
        enabled: bool,
    },
    SaveCategory {
        id: Option<String>,
        name: String,
        directory: Option<PathBuf>,
    },
    DeleteCategory {
        id: String,
    },
    Batch {
        ids: Vec<String>,
        action: Batch,
        category: Option<String>,
        allow_unsafe_pause: bool,
    },
    Start {
        id: String,
    },
    Pause {
        id: String,
    },
    Resume {
        id: String,
    },
    Cancel {
        id: String,
    },
    Timezone {
        timezone: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Draft {
    url: String,
    filename: String,
    directory: Option<PathBuf>,
    category: Option<String>,
    route: bool,
    connections: u32,
    optimize: bool,
    limit: Option<u64>,
    source: Option<SourceMetadata>,
}

// Optional source observations are hints, never proof of resource identity.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceMetadata {
    url: String,
    #[serde(rename = "category_id")]
    _category_id: Option<String>,
    content_type: Option<String>,
    total: Option<u64>,
    etag: Option<String>,
}
impl Draft {
    fn source(&self) -> Option<&SourceMetadata> {
        self.source.as_ref().filter(|s| s.url == self.url)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Batch {
    Pause,
    Resume,
    Cancel,
    Remove,
    Category,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScheduleInput {
    id: String,
    name: String,
    enabled: bool,
    kind: ScheduleKind,
    download_id: Option<String>,
    date: Option<ScheduleDate>,
    at_ms: Option<i64>,
    expected_revision: Option<u64>,
    weekdays: u8,
    start_minute: u16,
    end_minute: u16,
    limit: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScheduleDate {
    year: i32,
    month: u32,
    day: u32,
}

impl ScheduleInput {
    fn into_rule(self) -> (ScheduleRule, Option<LocalScheduleDate>) {
        (
            ScheduleRule {
                id: self.id,
                name: self.name,
                enabled: self.enabled,
                kind: self.kind,
                download_id: self.download_id,
                at_ms: self.at_ms,
                weekdays: self.weekdays,
                start_minute: self.start_minute,
                end_minute: self.end_minute,
                timezone: "UTC".into(),
                bandwidth_limit: self.limit,
                last_occurrence: None,
                last_local_day: None,
                revision: 0,
                note: None,
            },
            self.date.map(|d| LocalScheduleDate {
                year: d.year,
                month: d.month,
                day: d.day,
            }),
        )
    }
}

#[derive(Serialize)]
struct Row {
    id: String,
    created_at_ms: Option<u64>,
    content_type: Option<String>,
    etag: Option<String>,
    last_modified: Option<String>,
    sha256: Option<String>,
    resolved_url: Option<String>,
    partial_path: String,
    error_kind: Option<download_core::ErrorKind>,
    notice: Option<String>,
    resume_reason: &'static str,
    filename: String,
    destination: String,
    status: Status,
    downloaded: u64,
    durable_bytes: u64,
    total: Option<u64>,
    speed: f64,
    url: String,
    eta_seconds: Option<u64>,
    retry_remaining_seconds: Option<u64>,
    resume_capability: &'static str,
    workers: Vec<download_core::WorkerProgress>,
    active_connections: u32,
    target_connections: u32,
    segmented: bool,
    error: Option<String>,
    schedule_state: Option<String>,
    priority: Priority,
    category_id: Option<String>,
    queue_position: Option<u32>,
    connections: u32,
    optimize: bool,
    bandwidth_limit: Option<u64>,
    safe_pause: bool,
}

impl From<&Download> for Row {
    fn from(d: &Download) -> Self {
        Self {
            id: d.id.clone(),
            created_at_ms: d.created_at_ms,
            content_type: d.content_type.clone(),
            etag: d.etag.clone(),
            last_modified: d.last_modified.clone(),
            sha256: d.sha256.clone(),
            resolved_url: d.resolved_url.clone(),
            partial_path: d.partial_path.clone(),
            error_kind: d.error_kind,
            notice: d.notice.clone(),
            resume_reason: match d.resume.reason {
                download_core::ResumeReason::NotObserved => "NotObserved",
                download_core::ResumeReason::RevalidationRequired => "RevalidationRequired",
                download_core::ResumeReason::ValidatedRange => "ValidatedRange",
                download_core::ResumeReason::RangeIgnored => "RangeIgnored",
                download_core::ResumeReason::MissingStrongValidator => "MissingStrongValidator",
                download_core::ResumeReason::ConditionalFullResponse => "ConditionalFullResponse",
                download_core::ResumeReason::IdentityChanged => "IdentityChanged",
                download_core::ResumeReason::InvalidRangeResponse => "InvalidRangeResponse",
                download_core::ResumeReason::AuthenticationRequired => "AuthenticationRequired",
                download_core::ResumeReason::UnknownTotal => "UnknownTotal",
            },
            filename: d.filename.clone(),
            destination: d.destination.clone(),
            status: d.status,
            downloaded: d.downloaded,
            durable_bytes: d.durable_bytes,
            total: d.total,
            speed: d.current_speed,
            url: d.url.clone(),
            eta_seconds: d.eta_seconds,
            retry_remaining_seconds: d.retry_remaining_seconds,
            resume_capability: match d.resume.capability {
                ResumeCapability::Unknown => "unknown",
                ResumeCapability::Supported => "supported",
                ResumeCapability::Unsupported => "unsupported",
            },
            workers: d.telemetry.workers.clone(),
            active_connections: d.active_connections,
            target_connections: d.target_connections,
            segmented: d.segmented,
            error: d.error.clone(),
            schedule_state: d.schedule_state.clone(),
            priority: d.priority,
            category_id: d.category_id.clone(),
            queue_position: d.queue_position,
            connections: d.config.max_connections,
            optimize: d.config.optimize,
            bandwidth_limit: d.bandwidth_limit,
            safe_pause: d.resume.capability == ResumeCapability::Supported,
        }
    }
}

type Outcome = Result<serde_json::Value, Failure>;

#[derive(Serialize)]
struct Failure {
    code: &'static str,
    message: String,
}

impl Failure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<download_core::Error> for Failure {
    fn from(error: download_core::Error) -> Self {
        use download_core::Error::*;
        let code = match &error {
            Invalid(_) => "invalid",
            NotFound(_) => "not_found",
            Busy(_) => "busy",
            Closed => "closed",
            Network(_) | Http { .. } => "http",
            Disk(_) => "disk",
            Storage(_) => "storage",
            Protocol(_) => "protocol",
            Stopped => "stopped",
            Worker(_) => "worker",
        };
        Self::new(code, error.to_string())
    }
}

fn internal() -> Failure {
    Failure::new(
        "internal",
        "The native bridge failed. Quit and reopen DLM Native.",
    )
}

fn encode(result: Outcome) -> *mut c_char {
    let response = match result {
        Ok(data) => serde_json::json!({"data": data}),
        Err(error) => serde_json::json!({"error": error}),
    };
    // JSON escapes NULs. The returned allocation belongs to this library.
    CString::new(response.to_string()).map_or(ptr::null_mut(), CString::into_raw)
}

unsafe fn decode<T: serde::de::DeserializeOwned>(
    input: *const u8,
    length: usize,
) -> Result<T, Failure> {
    if input.is_null() || length == 0 || length > 64 * 1024 {
        return Err(Failure::new("invalid", "Invalid request size."));
    }
    serde_json::from_slice(unsafe { std::slice::from_raw_parts(input, length) })
        .map_err(|_| Failure::new("invalid", "Invalid native request."))
}

#[unsafe(no_mangle)]
pub extern "C" fn dlm_abi_version() -> u32 {
    1
}

fn value(value: impl Serialize) -> Outcome {
    serde_json::to_value(value).map_err(|_| internal())
}

fn done(result: download_core::Result<()>) -> Outcome {
    result.map(|()| serde_json::Value::Null).map_err(Into::into)
}

/// # Safety
/// The input points to length readable bytes and out is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dlm_open(
    input: *const u8,
    length: usize,
    out: *mut *mut DlmExperiment,
) -> *mut c_char {
    if out.is_null() {
        return encode(Err(Failure::new("invalid", "Missing output handle.")));
    }
    unsafe { *out = ptr::null_mut() };
    encode(
        catch_unwind(AssertUnwindSafe(|| {
            let request: Open = unsafe { decode(input, length) }?;
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|error| Failure::new("runtime", error.to_string()))?;
            let directory = request.root.join("Downloads");
            std::fs::create_dir_all(&directory).map_err(download_core::Error::from)?;
            let engine = runtime.block_on(Engine::open_in_timezone(
                request.root.join("Engine"),
                &request.timezone,
            ))?;
            let snapshots = engine.subscribe();
            let session = DlmExperiment {
                engine,
                inbox: None,
                inbox_path: request.root.join("browser-integration"),
                directory,
                runtime,
                snapshots,
                first_snapshot: true,
                faulted: false,
            };
            unsafe { *out = Box::into_raw(Box::new(session)) };
            Ok(serde_json::Value::Null)
        }))
        .unwrap_or_else(|_| Err(internal())),
    )
}

impl DlmExperiment {
    fn browser_command(&mut self, command: Command) -> Outcome {
        let failure = |code| Failure::new(code, "Browser integration is unavailable.");
        self.runtime.block_on(async {
            if self.inbox.is_none() {
                self.inbox = Some(browser_inbox::Inbox::open_retained(self.inbox_path.clone()).await.map_err(failure)?);
            }
            let inbox = self.inbox.as_ref().ok_or_else(internal)?;
            match command {
                Command::BrowserState {} => Ok(serde_json::json!({
                    "enabled": inbox.enabled().await, "directory": inbox.directory(),
                })),
                Command::BrowserEnable { enabled } => {
                    if inbox.enabled().await != enabled {
                        inbox.configure(enabled).await.map_err(failure)?;
                    }
                    value(true)
                }
                Command::BrowserPairingCode {} => value(inbox.pairing_code().await.map_err(failure)?),
                Command::BrowserReset {} => {
                    inbox.reset_pairing().await.map_err(failure)?;
                    value(true)
                }
                Command::BrowserAcknowledge { id } => { inbox.acknowledge(&id).await.map_err(failure)?; value(true) },
                Command::BrowserTake {} => Ok(inbox.pending().await.map_or(serde_json::Value::Null, |draft| {
                    serde_json::json!({ "url": draft.url, "filename": draft.filename, "id": draft.id })
                })),
                _ => Err(internal()),
            }
        })
    }

    fn call(&mut self, command: Command) -> Outcome {
        if let Command::Snapshot {} = command {
            if !self.first_snapshot && !self.snapshots.has_changed().unwrap_or(false) {
                return Ok(serde_json::Value::Null);
            }
            self.first_snapshot = false;
            let rows: Vec<_> = {
                let snapshot = self.snapshots.borrow_and_update();
                snapshot.iter().map(Row::from).collect()
            };
            return value(rows);
        }
        if matches!(
            command,
            Command::BrowserState {}
                | Command::BrowserEnable { .. }
                | Command::BrowserPairingCode {}
                | Command::BrowserReset {}
                | Command::BrowserTake {}
                | Command::BrowserAcknowledge { .. }
        ) {
            return self.browser_command(command);
        }
        self.runtime.block_on(async {
            match command {
                Command::BrowserState {}
                | Command::BrowserEnable { .. }
                | Command::BrowserPairingCode {}
                | Command::BrowserReset {}
                | Command::BrowserTake {}
                | Command::BrowserAcknowledge { .. } => Err(internal()),
                Command::VerifyLink { id, url } => {
                    let r = self.engine.verify_download_link(&id, &url).await?;
                    Ok(serde_json::json!({
                        "safe_to_resume": r.outcome == download_core::RecoveryOutcome::SafeToResume,
                        "outcome": match r.outcome {
                            download_core::RecoveryOutcome::SafeToResume => "safe_to_resume",
                            download_core::RecoveryOutcome::Incompatible => "incompatible",
                            download_core::RecoveryOutcome::CannotVerify => "cannot_verify",
                            download_core::RecoveryOutcome::RangeUnsupported => "range_unsupported",
                            download_core::RecoveryOutcome::TemporaryFailure => "temporary_failure",
                        },
                        "reason": r.reason, "token": r.token, "compared_bytes": r.compared_bytes,
                    }))
                }
                Command::ApplyLink { id, token, restart } => {
                    done(self.engine.apply_download_link(&id, &token, restart).await)
                }
                Command::RestartCancelled { id } => {
                    self.engine.restart_cancelled(&id).await?;
                    // Same record/ID. If admission fails, core leaves a safe Ready record.
                    done(self.engine.start(&id).await)
                }
                Command::InspectSource { url, filename } => {
                    // Optional preview metadata must not occupy the serial desktop bridge indefinitely.
                    let source = tokio::time::timeout(std::time::Duration::from_secs(5),
                        self.engine.inspect_source_category(&url, &filename)).await
                        .map_err(|_| Failure::new("http", "Source inspection timed out; downloading is still available."))??;
                    Ok(serde_json::json!({"url": url, "category_id": source.category_id,
                        "content_type": source.content_type, "total": source.total, "etag": source.etag}))
                }
                Command::PartialInfo { id } => {
                    let d = self.engine.get(&id).await?;
                    let length = match tokio::fs::symlink_metadata(&d.partial_path).await {
                        Ok(m) if m.is_file() => Some(m.len()),
                        Ok(_) => return Err(download_core::Error::Invalid("Partial path is not a regular file".into()).into()),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                        Err(e) => return Err(download_core::Error::Disk(e).into()),
                    };
                    let retained = length.map_or(0, |len| if d.segmented {
                        d.completed_ranges.iter().map(|r| r.end.min(len).saturating_sub(r.start)).sum()
                    } else { d.durable_bytes.min(len) });
                    Ok(serde_json::json!({"exists": length.is_some(), "file_length": length,
                        "retained_verified_bytes": retained}))
                }
                Command::InspectDraft { draft } => {
                    let base = tokio::fs::canonicalize(draft.directory.as_deref().unwrap_or(&self.directory))
                        .await.map_err(download_core::Error::Disk)?;
                    let base = base
                        .to_str()
                        .ok_or_else(|| Failure::new("invalid", "Invalid destination path."))?;
                    let plan = self
                        .engine
                        .suggest_destination(&draft.filename, base, draft.category.as_deref(), draft.source().and_then(|s| s.content_type.as_deref()))
                        .await?;
                    let directory = if draft.route {
                        plan.directory
                    } else {
                        base.into()
                    };
                    let total = draft.source().and_then(|s| s.total);
                    let etag = draft.source().and_then(|s| s.etag.clone());
                    let matches = self
                        .engine
                        .duplicate_candidates(DuplicateQuery {
                            url: draft.url,
                            directory: directory.clone(),
                            filename: draft.filename,
                            total,
                            etag,
                        })
                        .await?;
                    Ok(serde_json::json!({
                        "directory": directory, "category_id": plan.category_id,
                        "duplicates": matches.into_iter().map(|m| serde_json::json!({
                            "id": m.id, "filename": m.filename, "destination": m.destination,
                            "reasons": m.reasons,
                        })).collect::<Vec<_>>()
                    }))
                }
                Command::Add {
                    draft,
                    separate_copy,
                    schedule,
                } => {
                    let content_type_hint = draft.source().and_then(|s| s.content_type.clone());
                    let expected_size = draft.source().and_then(|s| s.total);
                    let expected_etag = draft.source().and_then(|s| s.etag.clone());
                    let settings = CreationSettings {
                        transfer: TransferConfig {
                            max_connections: draft.connections,
                            optimize: draft.optimize,
                        },
                        allow_duplicates: separate_copy,
                        separate_copy,
                        bandwidth_limit: draft.limit,
                        category_id: draft.category,
                        use_category_directory: draft.route,
                        content_type_hint,
                        expected_size,
                        expected_etag,
                    };
                    let directory = draft.directory.unwrap_or_else(|| self.directory.clone());
                    let record = if let Some(input) = schedule {
                        let (rule, date) = input.into_rule();
                        self.engine
                            .create_scheduled_with_settings(
                                draft.url,
                                directory,
                                draft.filename,
                                settings,
                                rule,
                                date,
                            )
                            .await?
                    } else {
                        let record = self
                            .engine
                            .create_with_settings(draft.url, directory, draft.filename, settings)
                            .await?;
                        self.engine.start(&record.id).await?;
                        record
                    };
                    value(record.id)
                }
                Command::Rename { id, filename } => {
                    done(self.engine.rename_download(&id, &filename).await)
                }
                Command::Move { id, directory } => value(
                    self.engine
                        .move_completed(&id, directory)
                        .await?
                        .source_kept,
                ),
                Command::Priority { id, priority } => {
                    done(self.engine.set_priority(&id, priority).await)
                }
                Command::Reorder { ids } => done(self.engine.reorder_queue(ids).await),
                Command::Configure {
                    id,
                    connections,
                    optimize,
                } => done(
                    self.engine
                        .configure_download(
                            &id,
                            TransferConfig {
                                max_connections: connections,
                                optimize,
                            },
                        )
                        .await,
                ),
                Command::SpeedLimit { id, limit } => {
                    done(self.engine.set_download_bandwidth_limit(&id, limit).await)
                }
                Command::Schedules {} => {
                    let view = self.engine.scheduler_view().await;
                    Ok(serde_json::json!({
                        "manual_limit": view.manual_limit,
                        "effective_limit": view.effective_limit,
                        "manual_override": view.manual_override,
                        "error": view.error,
                        "rules": view.rules.into_iter().map(|v| serde_json::json!({
                            "rule": v.rule, "next_run_ms": v.next_run_ms,
                            "active": v.active, "held": v.held, "status": v.status,
                            "pending": v.pending, "missed_at_ms": v.missed_at_ms,
                            "notify_missed": v.notify_missed,
                        })).collect::<Vec<_>>(),
                    }))
                }
                Command::SaveSchedule { schedule } => {
                    let current = self.engine.scheduler_view().await;
                    if !schedule.id.is_empty() {
                        let old = current.rules.iter().find(|entry| entry.rule.id == schedule.id)
                            .ok_or_else(|| Failure::new("not_found", "This schedule was deleted. Close this editor and choose a download again."))?;
                        if schedule.expected_revision != Some(old.rule.revision)
                            || schedule.kind != old.rule.kind || schedule.download_id != old.rule.download_id {
                            return Err(Failure::new("changed", "This schedule changed. Close and reopen it to load the current values."));
                        }
                    } else if schedule.kind == ScheduleKind::Download && current.rules.iter().any(|entry|
                        entry.rule.kind == ScheduleKind::Download && entry.rule.download_id == schedule.download_id) {
                        return Err(Failure::new("changed", "This download already has a schedule. Close and reopen its schedule editor."));
                    }
                    if schedule.date.is_some() && schedule.at_ms.is_some() {
                        return Err(Failure::new("invalid", "Choose a local date or an existing instant, not both."));
                    }
                    let (rule, date) = schedule.into_rule();
                    value(self.engine.save_schedule_local(rule, date).await?)
                }
                Command::EnableSchedule { id, enabled } => {
                    done(self.engine.set_schedule_enabled(&id, enabled).await)
                }
                Command::DeleteSchedule { id } => done(self.engine.delete_schedule(&id).await),
                Command::RearmSchedule { id } => done(self.engine.allow_schedule(&id).await),
                Command::DismissMissed { occurrences } => {
                    done(self.engine.dismiss_missed_schedules(&occurrences).await)
                }
                Command::QueueSettings {} => Ok(serde_json::json!({
                    "queue": self.engine.queue_config().await,
                    "limit": self.engine.global_bandwidth_limit().await,
                    "retry": self.engine.retry_policy().await,
                })),
                Command::SetRetry { policy } => {
                    done(self.engine.configure_retry_policy(policy).await)
                }
                Command::SetQueue { queue } => done(self.engine.configure_queue(queue).await),
                Command::GlobalLimit { limit } => {
                    done(self.engine.set_global_bandwidth_limit(limit).await)
                }
                Command::Organization {} => value(self.engine.organization().await),
                Command::SetOrganization { enabled } => {
                    done(self.engine.set_organization_enabled(enabled).await)
                }
                Command::SaveCategory {
                    id,
                    name,
                    directory,
                } => value(self.engine.save_category(id, name, directory).await?),
                Command::DeleteCategory { id } => done(self.engine.delete_category(&id).await),
                Command::Batch {
                    ids,
                    action,
                    category,
                    allow_unsafe_pause,
                } => {
                    let action = match action {
                        Batch::Pause => BatchAction::Pause,
                        Batch::Resume => BatchAction::Resume,
                        Batch::Cancel => BatchAction::Cancel,
                        Batch::Remove => BatchAction::Remove,
                        Batch::Category => BatchAction::Category,
                    };
                    let results = self
                        .engine
                        .batch(ids, action, category, allow_unsafe_pause)
                        .await?;
                    Ok(serde_json::json!(
                        results
                            .into_iter()
                            .map(|r| serde_json::json!({
                                "id": r.id, "error": r.error,
                            }))
                            .collect::<Vec<_>>()
                    ))
                }
                Command::Start { id } => self
                    .engine
                    .start(&id)
                    .await
                    .map(|()| serde_json::Value::Null)
                    .map_err(Into::into),
                Command::Pause { id } => self
                    .engine
                    .pause(&id)
                    .await
                    .map(|()| serde_json::Value::Null)
                    .map_err(Into::into),
                Command::Resume { id } | Command::Retry { id } => {
                    let result = self
                        .engine
                        .batch(vec![id], BatchAction::Resume, None, false)
                        .await?;
                    match result.into_iter().next().and_then(|r| r.error) {
                        Some(error) => Err(Failure::new("resume", error)),
                        None => Ok(serde_json::Value::Null),
                    }
                }
                Command::Cancel { id } => self
                    .engine
                    .cancel(&id)
                    .await
                    .map(|()| serde_json::Value::Null)
                    .map_err(Into::into),
                Command::Timezone { timezone } => self
                    .engine
                    .set_system_timezone(&timezone)
                    .await
                    .map(|()| serde_json::Value::Null)
                    .map_err(Into::into),
                Command::Snapshot {} => unreachable!(),
            }
        })
    }
}

/// # Safety
/// The handle is live and exclusively owned by the calling queue. Input is readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dlm_call(
    engine: *mut DlmExperiment,
    input: *const u8,
    length: usize,
) -> *mut c_char {
    let Some(session) = (unsafe { engine.as_mut() }) else {
        return encode(Err(Failure::new("closed", "The engine is closed.")));
    };
    if session.faulted {
        return encode(Err(internal()));
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let command = unsafe { decode(input, length) }?;
        session.call(command)
    }));
    encode(result.unwrap_or_else(|_| {
        session.faulted = true;
        Err(internal())
    }))
}

/// # Safety
/// A non-null handle must be live and uniquely owned, with no pending calls.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dlm_close(engine: *mut DlmExperiment) -> *mut c_char {
    if engine.is_null() {
        return encode(Ok(serde_json::Value::Null));
    }
    encode(
        catch_unwind(AssertUnwindSafe(|| {
            let session = unsafe { Box::from_raw(engine) };
            session.runtime.block_on(async {
                if let Some(inbox) = &session.inbox {
                    inbox.shutdown().await;
                }
                session.engine.shutdown().await
            })?;
            Ok(serde_json::Value::Null)
        }))
        .unwrap_or_else(|_| Err(internal())),
    )
}

/// # Safety
/// The pointer is null or a previously returned, unfreed result from this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dlm_free_json(result: *mut c_char) {
    if !result.is_null() {
        drop(unsafe { CString::from_raw(result) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::ffi::CStr;

    struct Session(*mut DlmExperiment);
    impl Session {
        fn open(root: &std::path::Path) -> Self {
            let input = json!({"root": root, "timezone": "UTC"}).to_string();
            let mut handle = ptr::null_mut();
            let result = read(unsafe { dlm_open(input.as_ptr(), input.len(), &mut handle) });
            assert!(result.get("error").is_none(), "{result}");
            assert!(!handle.is_null());
            Self(handle)
        }

        fn call(&mut self, command: Value) -> Value {
            let input = command.to_string();
            read(unsafe { dlm_call(self.0, input.as_ptr(), input.len()) })
        }

        fn data(&mut self, command: Value) -> Value {
            let result = self.call(command);
            assert!(result.get("error").is_none(), "{result}");
            result["data"].clone()
        }
    }
    impl Drop for Session {
        fn drop(&mut self) {
            let result = read(unsafe { dlm_close(self.0) });
            assert!(result.get("error").is_none(), "{result}");
        }
    }

    fn read(output: *mut c_char) -> Value {
        assert!(!output.is_null());
        let bytes = unsafe { CStr::from_ptr(output) }.to_bytes().to_vec();
        unsafe { dlm_free_json(output) };
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn abi_rejects_invalid_commands_and_coalesces_idle_snapshots() {
        assert_eq!(dlm_abi_version(), 1);
        let root = tempfile::tempdir().unwrap();
        let mut session = Session::open(root.path());
        assert_eq!(session.data(json!({"operation":"snapshot"})), json!([]));
        assert!(session.data(json!({"operation":"snapshot"})).is_null());
        assert_eq!(
            session.call(json!({"operation":"snapshot","unexpected":1}))["error"]["code"],
            "invalid"
        );
        let input = vec![b' '; 65_537];
        let response = read(unsafe { dlm_call(session.0, input.as_ptr(), input.len()) });
        assert_eq!(response["error"]["code"], "invalid");
        drop(session);
        assert_eq!(
            Session::open(root.path()).data(json!({"operation":"snapshot"})),
            json!([])
        );
    }

    #[test]
    fn browser_opt_in_survives_restart_without_creating_downloads() {
        let root = tempfile::tempdir().unwrap();
        let token = {
            let mut session = Session::open(root.path());
            assert!(!root.path().join("browser-integration").exists());
            assert_eq!(
                session.data(json!({"operation":"browser_state"}))["enabled"],
                false
            );
            session.data(json!({"operation":"browser_enable", "enabled":true}));
            assert!(session.data(json!({"operation":"browser_take"})).is_null());
            session.data(json!({"operation":"browser_pairing_code"}))
        };
        let mut reopened = Session::open(root.path());
        assert_eq!(
            reopened.data(json!({"operation":"browser_state"}))["enabled"],
            true
        );
        assert!(reopened.data(json!({"operation":"browser_pairing_code"})) == token);
        reopened.data(json!({"operation":"browser_reset"}));
        assert!(reopened.data(json!({"operation":"browser_pairing_code"})) != token);
        reopened.data(json!({"operation":"browser_enable", "enabled":false}));
        assert_eq!(reopened.data(json!({"operation":"snapshot"})), json!([]));
    }

    #[test]
    fn scheduled_category_creation_and_duplicate_review_survive_reopen() {
        let root = tempfile::tempdir().unwrap();
        let mut session = Session::open(root.path());
        session.data(json!({"operation":"set_organization","enabled":true}));
        let category =
            session.data(json!({"operation":"save_category","name":"Fixture documents"}));
        let draft = json!({
            "url":"http://127.0.0.1:1/fixture.pdf","filename":"fixture.pdf","category":category,
            "route":true,"connections":1,"optimize":false,"limit":1234,
        });
        let review = session.data(json!({"operation":"inspect_draft","draft":draft}));
        assert!(
            review["directory"]
                .as_str()
                .unwrap()
                .ends_with("Fixture documents")
        );
        assert_eq!(review["duplicates"], json!([]));
        let command = json!({
            "operation":"add","draft":draft,"separate_copy":false,
            "schedule":{
                "id":"","name":"Future start","enabled":true,"kind":"Download",
                "date":{"year":2090,"month":3,"day":4},
                "weekdays":0,"start_minute":600,"end_minute":0,
            }
        });
        let id = session.data(command.clone());
        let schedules = session.data(json!({"operation":"schedules"}));
        assert_eq!(schedules["rules"][0]["rule"]["download_id"], id);
        assert!(session.call(command)["error"].is_object());
        let review = session.data(json!({"operation":"inspect_draft","draft":draft}));
        assert_eq!(review["duplicates"][0]["id"], id);
        drop(session);

        let mut reopened = Session::open(root.path());
        let rows = reopened.data(json!({"operation":"snapshot"}));
        assert_eq!(rows[0]["id"], id);
        assert_eq!(rows[0]["bandwidth_limit"], 1234);
        assert_eq!(rows[0]["category_id"], category);
        assert_ne!(rows[0]["status"], "Running");
        assert_eq!(
            reopened.data(json!({"operation":"schedules"}))["rules"][0]["rule"]["name"],
            "Future start"
        );
    }
    // Tiny loopback HEAD fixture: no external network, no application data.
    fn phase2a_head(headers: &str) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/resource", listener.local_addr().unwrap());
        let response = format!("HTTP/1.1 200 OK\r\nConnection: close\r\n{headers}\r\n");
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                assert_eq!(stream.read(&mut byte).unwrap(), 1);
                request.push(byte[0]);
                assert!(request.len() < 8192);
            }
            assert!(request.starts_with(b"HEAD "));
            stream.write_all(response.as_bytes()).unwrap();
        });
        (url, thread)
    }

    #[test]
    fn phase2a_source_metadata_and_missing_or_encoded_headers() {
        let root = tempfile::tempdir().unwrap();
        let mut session = Session::open(root.path());
        for (headers, total, etag) in [
            (
                "Content-Type: video/mp4\r\nContent-Length: 4096\r\nETag: \"strong\"\r\n",
                json!(4096),
                json!("\"strong\""),
            ),
            (
                "Content-Type: video/mp4\r\nETag: W/\"weak\"\r\n",
                Value::Null,
                Value::Null,
            ),
            (
                "Content-Type: video/mp4\r\nContent-Length: 100\r\nContent-Encoding: gzip\r\nETag: \"encoded\"\r\n",
                Value::Null,
                Value::Null,
            ),
        ] {
            let (url, thread) = phase2a_head(headers);
            let metadata = session
                .data(json!({"operation":"inspect_source", "url":url, "filename":"resource"}));
            thread.join().unwrap();
            assert_eq!(metadata["url"], url);
            assert_eq!(metadata["content_type"], "video/mp4");
            assert_eq!(metadata["category_id"], "videos");
            assert_eq!(metadata["total"], total);
            assert_eq!(metadata["etag"], etag);
        }
        let (url, thread) = phase2a_head("");
        let metadata =
            session.data(json!({"operation":"inspect_source", "url":url, "filename":"resource"}));
        thread.join().unwrap();
        assert!(
            metadata["content_type"].is_null()
                && metadata["total"].is_null()
                && metadata["etag"].is_null()
        );
    }

    #[test]
    fn phase2a_resolved_destinations_and_duplicate_ids() {
        let root = tempfile::tempdir().unwrap();
        let mut session = Session::open(root.path());
        session.data(json!({"operation":"set_organization", "enabled":true}));
        let videos = root.path().join("custom-videos");
        std::fs::create_dir(&videos).unwrap();
        let videos = std::fs::canonicalize(videos).unwrap();
        session.data(json!({"operation":"save_category", "id":"videos", "name":"Videos", "directory":videos}));
        let mut draft = json!({"url":"http://127.0.0.1:1/resource", "filename":"resource",
            "route":true,"connections":1,"optimize":false,
            "source":{"url":"http://127.0.0.1:1/resource", "content_type":"video/mp4", "total":4096, "etag":"\"strong\""}});
        let plan = session.data(json!({"operation":"inspect_draft", "draft":draft}));
        assert_eq!(plan["directory"], videos.to_str().unwrap());
        assert_eq!(plan["category_id"], "videos");
        // Reuse the existing future-start API to exercise atomic creation without a transfer.
        let create = |draft: Value, separate_copy| {
            json!({"operation":"add","draft":draft,"separate_copy":separate_copy,
            "schedule":{"id":"","name":"Future","enabled":true,"kind":"Download",
            "date":{"year":2090,"month":3,"day":4},"weekdays":0,"start_minute":600,"end_minute":0}})
        };
        let id = session.data(create(draft.clone(), false));
        let row = session.data(json!({"operation":"snapshot"}))[0].clone();
        assert_eq!(row["category_id"], "videos");
        assert!(
            row["destination"]
                .as_str()
                .unwrap()
                .starts_with(videos.to_str().unwrap())
        );
        assert!(session.call(create(draft.clone(), false))["error"].is_object());
        let matches = session.data(json!({"operation":"inspect_draft", "draft":draft}));
        assert_eq!(matches["duplicates"][0]["id"], id);
        let second = session.data(create(draft.clone(), true));
        assert_ne!(id, second);
        let rows = session.data(json!({"operation":"snapshot"}));
        assert_ne!(rows[0]["destination"], rows[1]["destination"]);
        // An explicit base override wins; stale source hints must not classify a different URL.
        draft["route"] = false.into();
        draft["directory"] = root.path().to_str().unwrap().into();
        assert_eq!(
            session.data(json!({"operation":"inspect_draft", "draft":draft}))["directory"],
            root.path().canonicalize().unwrap().to_str().unwrap()
        );
        draft["url"] = "http://127.0.0.1:1/other".into();
        assert_eq!(
            session.data(json!({"operation":"inspect_draft", "draft":draft}))["category_id"],
            "other"
        );
    }

    #[test]
    fn phase2a_cancelled_restart_and_replacement_protection() {
        let root = tempfile::tempdir().unwrap();
        let mut session = Session::open(root.path());
        let engine = unsafe { &*session.0 };
        let record = engine
            .runtime
            .block_on(engine.engine.create(
                "http://127.0.0.1:1/original".into(),
                root.path().into(),
                "payload.bin".into(),
            ))
            .unwrap();
        let id = &record.id;
        assert_eq!(
            session.data(json!({"operation":"partial_info", "id":id}))["exists"],
            true
        );
        session.data(json!({"operation":"cancel", "id":id}));
        assert_eq!(
            session.data(json!({"operation":"partial_info", "id":id}))["exists"],
            false
        );
        let result = session.data(json!({"operation":"verify_link", "id":id, "url":"https://example.invalid/replacement"}));
        assert_eq!(result["outcome"], "cannot_verify");
        assert_eq!(result["compared_bytes"], 0);
        assert!(
            session.call(
                json!({"operation":"apply_link", "id":id,"token":result["token"],"restart":false})
            )["error"]
                .is_object()
        );
        assert!(!std::path::Path::new(&record.partial_path).exists());
        // Existing destination prevents restart without touching its bytes or creating a new record.
        std::fs::write(&record.destination, b"keep me").unwrap();
        assert!(
            session.call(json!({"operation":"restart_cancelled", "id":id}))["error"].is_object()
        );
        assert_eq!(std::fs::read(&record.destination).unwrap(), b"keep me");
        std::fs::remove_file(&record.destination).unwrap();
        session.data(json!({"operation":"restart_cancelled", "id":id}));
        let rows = session.data(json!({"operation":"snapshot"}));
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["id"], *id);
        assert_ne!(rows[0]["status"], "Cancelled");
        assert_eq!(rows[0]["durable_bytes"], 0);
        assert!(
            session.call(json!({"operation":"restart_cancelled", "id":id}))["error"].is_object()
        );
    }
    #[test]
    fn phase2b_row_metadata_uses_existing_core_fields() {
        let root = tempfile::tempdir().unwrap();
        let session = Session::open(root.path());
        let engine = unsafe { &*session.0 };
        let mut record = engine
            .runtime
            .block_on(engine.engine.create(
                "https://example.invalid/file".into(),
                root.path().into(),
                "metadata.bin".into(),
            ))
            .unwrap();
        assert!(record.created_at_ms.is_some());
        record.created_at_ms = Some(123456789);
        record.content_type = Some("application/octet-stream".into());
        record.etag = Some("\"strong\"".into());
        record.last_modified = Some("Wed, 21 Oct 2015 07:28:00 GMT".into());
        record.sha256 = Some("fixture-sha".into());
        record.error_kind = Some(download_core::ErrorKind::Disk);
        record.notice = Some("Fixture notice".into());
        record.resume.reason = download_core::ResumeReason::RevalidationRequired;
        let row = serde_json::to_value(Row::from(&record)).unwrap();
        assert_eq!(row["created_at_ms"], 123456789);
        assert_eq!(row["content_type"], "application/octet-stream");
        assert_eq!(row["etag"], "\"strong\"");
        assert_eq!(row["sha256"], "fixture-sha");
        assert_eq!(
            row["last_modified"],
            record.last_modified.as_ref().unwrap().as_str()
        );
        assert_eq!(row["partial_path"], record.partial_path.as_str());
        assert_eq!(row["notice"], "Fixture notice");
        assert_eq!(row["error_kind"], "Disk");
        assert_eq!(row["resume_reason"], "RevalidationRequired");
        record.created_at_ms = None;
        assert!(serde_json::to_value(Row::from(&record)).unwrap()["created_at_ms"].is_null());
    }
    #[test]
    fn phase3a_edit_existing_rejects_stale_deleted_and_duplicate_targets() {
        let root = tempfile::tempdir().unwrap();
        let mut session = Session::open(root.path());
        let id = session.data(json!({"operation":"add", "draft":{
            "url":"https://example.invalid/once", "filename":"once.bin", "route":false,
            "connections":1,"optimize":false}, "separate_copy":false,
            "schedule":{"id":"","name":"Once","enabled":true,"kind":"Download",
                "date":{"year":2090,"month":3,"day":4},"weekdays":0,"start_minute":600,"end_minute":0}}));
        let original = session.data(json!({"operation":"schedules"}))["rules"][0].clone();
        assert_eq!(original["pending"], true);
        let mut edit = json!({"id":original["rule"]["id"], "name":"Edited", "enabled":true,
            "kind":"Download", "download_id":id,"at_ms":original["rule"]["at_ms"],
            "expected_revision":original["rule"]["revision"],"weekdays":0,"start_minute":600,"end_minute":0});
        assert_eq!(
            session.data(json!({"operation":"save_schedule", "schedule":edit})),
            original["rule"]["id"]
        );
        let updated = session.data(json!({"operation":"schedules"}));
        assert_eq!(updated["rules"].as_array().unwrap().len(), 1);
        assert_eq!(updated["rules"][0]["rule"]["name"], "Edited");
        assert_eq!(
            updated["rules"][0]["rule"]["at_ms"],
            original["rule"]["at_ms"]
        );
        assert_eq!(
            updated["rules"][0]["rule"]["revision"].as_u64().unwrap(),
            original["rule"]["revision"].as_u64().unwrap() + 1
        );
        assert_eq!(
            session.call(json!({"operation":"save_schedule", "schedule":edit}))["error"]["code"],
            "changed"
        );
        let mut duplicate = edit.clone();
        duplicate["id"] = "".into();
        duplicate["expected_revision"] = Value::Null;
        assert_eq!(
            session.call(json!({"operation":"save_schedule", "schedule":duplicate}))["error"]["code"],
            "changed"
        );
        session.data(json!({"operation":"delete_schedule", "id":original["rule"]["id"]}));
        edit["expected_revision"] = updated["rules"][0]["rule"]["revision"].clone();
        assert_eq!(
            session.call(json!({"operation":"save_schedule", "schedule":edit}))["error"]["code"],
            "not_found"
        );
        assert!(
            session.data(json!({"operation":"schedules"}))["rules"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let rows = session.data(json!({"operation":"snapshot"}));
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["id"], id);
        assert_ne!(rows[0]["status"], "Running");
    }

    #[test]
    fn phase3a_weekly_creation_is_gated_and_persistent() {
        let root = tempfile::tempdir().unwrap();
        let mut session = Session::open(root.path());
        // UTC is the fixture session zone. Start two hours ahead; currently closed,
        // including around midnight. The normal supervisor must never start it now.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            / 60
            % 1440;
        let start = (now + 120) % 1440;
        let end = (now + 180) % 1440;
        let id = session.data(json!({"operation":"add", "draft":{
            "url":"https://example.invalid/weekly", "filename":"weekly.bin", "route":false,
            "connections":4,"optimize":true,"limit":1234}, "separate_copy":false,
            "schedule":{"id":"","name":"Weekly","enabled":true,"kind":"Download",
                "weekdays":127,"start_minute":start,"end_minute":end}}));
        let view = session.data(json!({"operation":"schedules"}));
        assert_eq!(view["rules"][0]["pending"], true);
        assert!(view["rules"][0]["rule"]["at_ms"].is_null());
        assert_eq!(view["rules"][0]["rule"]["weekdays"], 127);
        assert_eq!(view["rules"][0]["rule"]["download_id"], id);
        drop(session);
        let mut reopened = Session::open(root.path());
        let rows = reopened.data(json!({"operation":"snapshot"}));
        assert_eq!(rows[0]["id"], id);
        assert_eq!(rows[0]["bandwidth_limit"], 1234);
        assert!(!matches!(
            rows[0]["status"].as_str(),
            Some("Running" | "Queued" | "Finalizing")
        ));
        assert_eq!(rows[0]["durable_bytes"], 0);
        let view = reopened.data(json!({"operation":"schedules"}));
        assert_eq!(view["rules"][0]["pending"], true);
        assert_eq!(view["rules"][0]["rule"]["start_minute"], start);
        assert_eq!(view["rules"][0]["rule"]["end_minute"], end);
    }
}
