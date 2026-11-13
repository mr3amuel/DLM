//! Runtime-only observations. None of these fields establish durable completion.
use crate::ByteRange;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[repr(u8)]
pub enum WorkerPhase {
    WaitingForPermit,
    Requesting,
    Transferring,
    Synchronizing,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkerProgress {
    pub worker_id: u64,
    pub transfer_generation: String,
    pub range_start: u64,
    pub range_end: u64,
    pub written_bytes: u64,
    pub phase: WorkerPhase,
    pub request_permit_held: bool,
    pub sample_sequence: u64,
}

#[derive(Debug, Clone, Default)]
pub struct TransferTelemetry {
    pub generation: Option<String>,
    pub sequence: u64,
    pub(crate) rate_sequence: u64,
    pub workers: Vec<WorkerProgress>,
}
impl TransferTelemetry {
    pub(crate) fn begin(&mut self) {
        self.generation = Some(uuid::Uuid::new_v4().to_string());
        self.workers.clear();
        self.rate_sequence = 0;
        self.sequence = self.sequence.saturating_add(1);
    }
    pub(crate) fn clear(&mut self) {
        self.workers.clear();
        self.sequence = self.sequence.saturating_add(1);
    }
}

pub(crate) struct WorkerStats {
    pub id: u64,
    pub range: ByteRange,
    pub progress: AtomicU64,
    pub identity_changed: AtomicBool,
    // One atomic observation for phase + ownership. Permit transitions additionally
    // occur under the connection-counter lock so a sampled vector/count agree.
    state: AtomicU8,
}
impl WorkerStats {
    pub fn new(id: u64, range: ByteRange) -> Self {
        Self {
            id,
            range,
            progress: AtomicU64::new(0),
            identity_changed: AtomicBool::new(false),
            state: AtomicU8::new(0),
        }
    }
    pub fn phase(&self, phase: WorkerPhase) {
        self.state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                Some((state & 128) | phase as u8)
            })
            .unwrap();
    }
    pub fn permit(&self, held: bool) {
        self.state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                Some((state & 127) | if held { 128 } else { 0 })
            })
            .unwrap();
    }
    pub fn snapshot(&self, generation: &str, sequence: u64) -> WorkerProgress {
        let state = self.state.load(Ordering::Acquire);
        let phase = match state & 127 {
            0 => WorkerPhase::WaitingForPermit,
            1 => WorkerPhase::Requesting,
            2 => WorkerPhase::Transferring,
            3 => WorkerPhase::Synchronizing,
            _ => WorkerPhase::Completed,
        };
        WorkerProgress {
            worker_id: self.id,
            transfer_generation: generation.into(),
            range_start: self.range.start,
            range_end: self.range.end,
            written_bytes: self.progress.load(Ordering::Relaxed),
            phase,
            request_permit_held: state & 128 != 0,
            sample_sequence: sequence,
        }
    }
}

/// Monotonic rate window. Completion publications reuse the last speed; only
/// timed samples advance the window, including zero-byte samples during stalls.
pub(crate) struct RateSample {
    at: tokio::time::Instant,
    bytes: u64,
}
impl RateSample {
    pub fn new(bytes: u64) -> Self {
        Self {
            at: tokio::time::Instant::now(),
            bytes,
        }
    }
    pub fn sample(&mut self, bytes: u64) -> f64 {
        let now = tokio::time::Instant::now();
        let rate = bytes.saturating_sub(self.bytes) as f64
            / now.duration_since(self.at).as_secs_f64().max(0.001);
        self.at = now;
        self.bytes = bytes;
        rate
    }
}
