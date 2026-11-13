use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Ready,
    Queued,
    Running,
    Paused,
    Cancelled,
    Finalizing,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Download {
    #[serde(default)]
    pub category_id: Option<String>,
    #[serde(default)]
    pub category_manual: bool,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub ever_started: bool,
    #[serde(default)]
    pub pause_origin: Option<PauseOrigin>,
    #[serde(default)]
    pub automation_strict_resume: bool,
    #[serde(skip)]
    pub schedule_state: Option<String>,
    #[serde(default)]
    pub recovery_pin: Option<crate::recovery::RecoveryPin>,
    #[serde(default)]
    pub last_http_status: Option<u16>,
    #[serde(default)]
    pub created_at_ms: Option<u64>,
    #[serde(skip)]
    pub telemetry: crate::TransferTelemetry,
    #[serde(skip)]
    pub resume: crate::ResumeObservation,
    #[serde(default)]
    pub priority: Priority,
    #[serde(default)]
    pub admission_bypasses: u32,
    #[serde(default)]
    pub bandwidth_limit: Option<u64>,
    #[serde(default)]
    pub current_speed: f64,
    #[serde(default)]
    pub smoothed_speed: f64,
    #[serde(default)]
    pub eta_seconds: Option<u64>,
    #[serde(default)]
    pub retry_remaining_seconds: Option<u64>,
    #[serde(default)]
    pub error_kind: Option<ErrorKind>,
    #[serde(default)]
    pub bandwidth_limited: bool,
    #[serde(default)]
    pub config: TransferConfig,
    #[serde(default)]
    pub completed_ranges: Vec<ByteRange>,
    #[serde(default)]
    pub segmented: bool,
    #[serde(default)]
    pub resolved_url: Option<String>,
    #[serde(default)]
    pub queue_order: u64,
    #[serde(default)]
    pub queue_position: Option<u32>,
    #[serde(default)]
    pub active_connections: u32,
    #[serde(default)]
    pub target_connections: u32,
    #[serde(default)]
    pub retries: u32,
    #[serde(default)]
    pub notice: Option<String>,
    #[serde(default)]
    pub retry_not_before_ms: Option<u64>,
    pub id: String,
    pub url: String,
    pub filename: String,
    pub destination: String,
    pub partial_path: String,
    pub status: Status,
    pub downloaded: u64,
    pub durable_bytes: u64,
    pub total: Option<u64>,
    pub speed: f64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub sha256: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    High,
    #[default]
    Normal,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorKind {
    Network,
    Http,
    RateLimited,
    Authentication,
    Disk,
    Storage,
    Integrity,
    Invalid,
    Internal,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct PolicySettings {
    pub global_bandwidth_limit: Option<u64>,
    #[serde(default)]
    pub retry_policy: crate::RetryPolicy,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid request: {0}")]
    Invalid(String),
    #[error("Download not found: {0}")]
    NotFound(String),
    #[error("Download is busy: {0}")]
    Busy(String),
    #[error("Engine is shutting down")]
    Closed,
    #[error("HTTP error: {0}")]
    Network(#[source] reqwest::Error),
    #[error("File error: {0}")]
    Disk(#[from] std::io::Error),
    #[error("Metadata error: {0}")]
    Storage(String),
    #[error("Unsafe HTTP response: {0}")]
    Protocol(String),
    #[error("HTTP {status}")]
    Http { status: u16, retry_after_secs: u64 },
    #[error("Transfer stopped")]
    Stopped,
    #[error("Worker error: {0}")]
    Worker(String),
}
pub type Result<T> = std::result::Result<T, Error>;

/// Half-open byte interval. File length is never a completion ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}
impl ByteRange {
    pub fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }
    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferConfig {
    pub max_connections: u32,
    pub optimize: bool,
}
impl Default for TransferConfig {
    fn default() -> Self {
        Self {
            max_connections: 4,
            optimize: true,
        }
    }
}
impl TransferConfig {
    pub fn validate(&self) -> Result<()> {
        if ![1, 2, 4, 8, 16, 32].contains(&self.max_connections) {
            return Err(Error::Invalid(
                "Connections must be 1, 2, 4, 8, 16 or 32".into(),
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueConfig {
    pub max_downloads: u32,
    pub global_connections: u32,
    pub per_host_connections: u32,
}
impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            max_downloads: 3,
            global_connections: 64,
            per_host_connections: 32,
        }
    }
}
impl QueueConfig {
    pub fn validate(&self) -> Result<()> {
        if !(1..=8).contains(&self.max_downloads)
            || !(1..=128).contains(&self.global_connections)
            || !(1..=32).contains(&self.per_host_connections)
        {
            return Err(Error::Invalid("Download slots must be 1–8, global connections 1–128 and per-host connections 1–32".into()));
        }
        Ok(())
    }
}

// Request URLs can contain bearer tokens. Strip them before any error is stored,
// formatted, forwarded or included in a source chain.
impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Self::Network(error.without_url())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PauseOrigin {
    User,
    Scheduler,
    Recovery,
}

/// Settings installed before a newly created download becomes visible/admissible.
#[derive(Debug, Clone)]
pub struct CreationSettings {
    pub transfer: TransferConfig,
    pub bandwidth_limit: Option<u64>,
    pub category_id: Option<String>,
    pub use_category_directory: bool,
    pub content_type_hint: Option<String>,
    pub expected_size: Option<u64>,
    pub expected_etag: Option<String>,
    pub allow_duplicates: bool,
    pub separate_copy: bool,
}
impl Default for CreationSettings {
    fn default() -> Self {
        Self {
            transfer: TransferConfig::default(),
            bandwidth_limit: None,
            category_id: None,
            use_category_directory: false,
            content_type_hint: None,
            expected_size: None,
            expected_etag: None,
            allow_duplicates: true,
            separate_copy: false,
        }
    }
}
