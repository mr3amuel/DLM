mod adaptive;
mod bandwidth;
mod connections;
mod engine;
mod model;
mod ranges;
mod retry;
mod segmented;
mod storage;
mod transfer;
pub use engine::Engine;
pub use model::{
    ByteRange, CreationSettings, Download, Error, ErrorKind, Priority, QueueConfig, Result, Status,
    TransferConfig,
};
pub use retry::RetryPolicy;

mod resume;
mod telemetry;
pub use resume::{ResumeCapability, ResumeObservation, ResumeReason};
pub use telemetry::{TransferTelemetry, WorkerPhase, WorkerProgress};

mod recovery;
pub use recovery::{RecoveryOutcome, RecoveryPin, VerificationReport};

mod automation;
pub use automation::{
    Clock, LocalScheduleDate, ScheduleKind, ScheduleRule, ScheduleView, SchedulerView, SystemClock,
};
pub use model::PauseOrigin;

mod organization;
pub use organization::{
    BatchAction, BatchResult, Category, DestinationPlan, DuplicateCandidate, DuplicateQuery,
    OrganizationConfig, SourceSuggestion, classify,
};
mod file_operations;
pub use file_operations::FileMoveResult;
