//! Shared hierarchical token buckets. A byte is charged to both levels atomically.
//! FIFO eligible turns are per bounded grant, not per response or connection lifetime.
use crate::{Error, Result, engine::Stop};
use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
    time::Duration,
};
use tokio::{
    sync::{Notify, watch},
    time::Instant,
};

const SCALE: u128 = 1_000_000_000;
const QUANTUM: usize = 64 * 1024;
const MAX_BURST: usize = 1024 * 1024;

struct Bucket {
    rate: u64,
    credit: u128,
    updated: Instant,
}
impl Bucket {
    fn new(rate: u64, now: Instant) -> Self {
        Self {
            rate: rate.max(1),
            credit: 0,
            updated: now,
        }
    }
    fn capacity_bytes(&self) -> usize {
        // At most 100ms of burst, with a one-byte floor and 1MiB ceiling.
        // Burst capacity exceeds the grant quantum at high rates so timer
        // granularity does not impose an artificial throughput ceiling.
        (self.rate / 10).clamp(1, MAX_BURST as u64) as usize
    }
    fn refill(&mut self, now: Instant) {
        self.credit = self
            .credit
            .saturating_add(
                now.duration_since(self.updated)
                    .as_nanos()
                    .saturating_mul(self.rate as u128),
            )
            .min(self.capacity_bytes() as u128 * SCALE);
        self.updated = now;
    }
    fn wait(&self, bytes: usize) -> Duration {
        Duration::from_nanos(
            ((bytes as u128 * SCALE)
                .saturating_sub(self.credit)
                .div_ceil(self.rate as u128)) as u64,
        )
    }
}
#[derive(Default)]
struct State {
    global: Option<Bucket>,
    local: HashMap<String, Bucket>,
    queue: VecDeque<(u64, String, usize)>,
    next: u64,
}
impl State {
    fn local_ready(&mut self, id: &str, bytes: usize, now: Instant) -> bool {
        let mut grant = bytes.min(QUANTUM);
        if let Some(global) = &self.global {
            grant = grant.min(global.capacity_bytes());
        }
        if let Some(local) = self.local.get_mut(id) {
            local.refill(now);
            grant = grant.min(local.capacity_bytes());
            local.wait(grant).is_zero()
        } else {
            true
        }
    }
    fn allowance(&mut self, id: &str, bytes: usize, now: Instant) -> (usize, Duration) {
        let mut grant = bytes.min(QUANTUM);
        if let Some(b) = &mut self.global {
            b.refill(now);
            grant = grant.min(b.capacity_bytes());
        }
        if let Some(b) = self.local.get_mut(id) {
            b.refill(now);
            grant = grant.min(b.capacity_bytes());
        }
        if self.global.is_none() && !self.local.contains_key(id) {
            return (bytes, Duration::ZERO);
        }
        let mut wait = Duration::ZERO;
        if let Some(b) = &self.global {
            wait = wait.max(b.wait(grant));
        }
        if let Some(b) = self.local.get(id) {
            wait = wait.max(b.wait(grant));
        }
        (grant, wait)
    }
}
#[derive(Default)]
pub(crate) struct Bandwidth {
    state: Mutex<State>,
    changed: Notify,
}
impl Bandwidth {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set_global(&self, rate: Option<u64>) {
        let mut state = self.state.lock().unwrap();
        Self::replace(&mut state.global, rate);
        drop(state);
        self.changed.notify_waiters();
    }
    fn replace(bucket: &mut Option<Bucket>, rate: Option<u64>) {
        // Updating a cap cannot mint a fresh burst. Newly enabled caps start empty.
        let now = Instant::now();
        let old = bucket
            .as_mut()
            .map(|b| {
                b.refill(now);
                b.credit
            })
            .unwrap_or(0);
        *bucket = rate.filter(|r| *r > 0).map(|r| {
            let mut b = Bucket::new(r, now);
            b.credit = old.min(b.capacity_bytes() as u128 * SCALE);
            b
        });
    }
    pub fn set_local(&self, id: &str, rate: Option<u64>) {
        let mut state = self.state.lock().unwrap();
        let mut bucket = state.local.remove(id);
        Self::replace(&mut bucket, rate);
        if let Some(bucket) = bucket {
            state.local.insert(id.to_owned(), bucket);
        }
        drop(state);
        self.changed.notify_waiters();
    }
    pub fn is_capped(&self, id: &str) -> bool {
        let state = self.state.lock().unwrap();
        state.global.is_some() || state.local.contains_key(id)
    }
    /// Returns a bounded byte grant, or the full slice on the unlimited fast path.
    /// Dropping the future, pause, cancellation and closed stop channels release its turn.
    pub async fn acquire(
        &self,
        id: &str,
        bytes: usize,
        stop: &mut watch::Receiver<Stop>,
    ) -> Result<usize> {
        if bytes == 0 {
            return Ok(0);
        }
        let ticket = {
            let mut state = self.state.lock().unwrap();
            if *stop.borrow() != Stop::Running {
                return Err(Error::Stopped);
            }
            if state.global.is_none() && !state.local.contains_key(id) {
                return Ok(bytes);
            }
            let ticket = state.next;
            state.next = state.next.wrapping_add(1);
            state.queue.push_back((ticket, id.to_owned(), bytes));
            ticket
        };
        let _turn = Turn {
            bandwidth: self,
            ticket,
        };
        loop {
            // Register before inspecting state to avoid losing a runtime update.
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if *stop.borrow() != Stop::Running {
                return Err(Error::Stopped);
            }
            let wait = {
                let mut state = self.state.lock().unwrap();
                let now = Instant::now();
                // Skip locally blocked turns so slow local caps cannot block others.
                let preceding: Vec<_> = state
                    .queue
                    .iter()
                    .take_while(|(t, _, _)| *t != ticket)
                    .map(|(_, id, bytes)| (id.clone(), *bytes))
                    .collect();
                let earlier_ready = preceding
                    .iter()
                    .any(|(id, bytes)| state.local_ready(id, *bytes, now));
                let (grant, wait) = state.allowance(id, bytes, now);
                if !earlier_ready && wait.is_zero() {
                    let debit = grant as u128 * SCALE;
                    if let Some(b) = &mut state.global {
                        b.credit -= debit;
                    }
                    if let Some(b) = state.local.get_mut(id) {
                        b.credit -= debit;
                    }
                    return Ok(grant);
                }
                if earlier_ready && wait.is_zero() {
                    Duration::from_secs(3600)
                } else {
                    wait
                }
            };
            tokio::select! {
                _ = &mut notified => {},
                _ = tokio::time::sleep(wait) => {},
                _ = stop.changed() => return Err(Error::Stopped),
            }
        }
    }
}
struct Turn<'a> {
    bandwidth: &'a Bandwidth,
    ticket: u64,
}
impl Drop for Turn<'_> {
    fn drop(&mut self) {
        self.bandwidth
            .state
            .lock()
            .unwrap()
            .queue
            .retain(|(t, _, _)| *t != self.ticket);
        self.bandwidth.changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };

    fn fill(bandwidth: &Bandwidth) {
        let mut state = bandwidth.state.lock().unwrap();
        if let Some(b) = &mut state.global {
            b.credit = b.capacity_bytes() as u128 * SCALE;
        }
        for b in state.local.values_mut() {
            b.credit = b.capacity_bytes() as u128 * SCALE;
        }
        drop(state);
        bandwidth.changed.notify_waiters();
    }

    #[test]
    fn monotonic_refill_fractional_credit_and_bounded_burst() {
        let now = Instant::now();
        let mut b = Bucket::new(123, now);
        b.refill(now + Duration::from_millis(1));
        assert_eq!(b.credit, 123_000_000);
        assert_eq!(b.wait(1), Duration::from_nanos(7_130_082));
        b.refill(now + Duration::from_secs(1));
        assert_eq!(b.credit, 12 * SCALE);
        b.credit -= 10 * SCALE;
        b.refill(now + Duration::from_secs(1000));
        assert_eq!(b.credit, 12 * SCALE);
        let mut huge = Bucket::new(u64::MAX, now);
        huge.refill(now + Duration::from_secs(1_000_000));
        assert_eq!(huge.credit, MAX_BURST as u128 * SCALE);
        let mut tiny = Bucket::new(1, now);
        assert_eq!(tiny.wait(1), Duration::from_secs(1));
        tiny.refill(now + Duration::from_secs(100));
        assert_eq!(tiny.credit, SCALE);
    }

    #[test]
    fn hierarchical_accounting_aggregates_workers_and_downloads() {
        let now = Instant::now();
        let mut state = State {
            global: Some(Bucket::new(1000, now)),
            ..State::default()
        };
        state.local.insert("a".into(), Bucket::new(400, now));
        let at = now + Duration::from_millis(100);
        let (grant, wait) = state.allowance("a", 1000, at);
        assert_eq!((grant, wait), (40, Duration::ZERO));
        state.global.as_mut().unwrap().credit -= grant as u128 * SCALE;
        state.local.get_mut("a").unwrap().credit -= grant as u128 * SCALE;
        // A second worker of the same download cannot acquire another local burst.
        assert_eq!(state.allowance("a", 1000, at).1, Duration::from_millis(100));
        // Another download sees the same global debit, not a separate global bucket.
        assert_eq!(
            state.allowance("b", 1000, at),
            (100, Duration::from_millis(40))
        );
        assert_eq!(
            state.allowance("b", 1000, at + Duration::from_millis(40)).1,
            Duration::ZERO
        );
    }

    #[test]
    fn high_rate_uses_batches_and_reconfiguration_clamps_old_credit() {
        let bandwidth = Bandwidth::new();
        bandwidth.set_global(Some(100_000_000));
        fill(&bandwidth);
        {
            let mut state = bandwidth.state.lock().unwrap();
            let at = Instant::now();
            assert_eq!(
                state.allowance("a", 1_000_000, at),
                (QUANTUM, Duration::ZERO)
            );
            state.global.as_mut().unwrap().credit = 0;
            let (grant, wait) = state.allowance("a", 1_000_000, at);
            assert_eq!(grant, QUANTUM);
            // A wake waits for a real batch, never for one byte at a time.
            assert_eq!(wait, Duration::from_nanos(655_360));
        }
        fill(&bandwidth);
        bandwidth.set_global(Some(100));
        assert_eq!(
            bandwidth
                .state
                .lock()
                .unwrap()
                .global
                .as_ref()
                .unwrap()
                .credit,
            10 * SCALE
        );
        bandwidth.set_global(None);
        bandwidth.set_global(Some(100));
        assert_eq!(
            bandwidth
                .state
                .lock()
                .unwrap()
                .global
                .as_ref()
                .unwrap()
                .credit,
            0
        );
    }

    #[tokio::test]
    async fn fifo_concurrency_cancellation_and_dropped_future_release_turns() {
        let bandwidth = Bandwidth::new();
        bandwidth.set_global(Some(10));
        let (_tx, mut first_stop) = watch::channel(Stop::Running);
        let (second_tx, mut second_stop) = watch::channel(Stop::Running);
        let (_third_tx, mut third_stop) = watch::channel(Stop::Running);
        let mut first = Box::pin(bandwidth.acquire("a", 100, &mut first_stop));
        let mut second = Box::pin(bandwidth.acquire("b", 100, &mut second_stop));
        let mut third = Box::pin(bandwidth.acquire("c", 100, &mut third_stop));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(first.as_mut().poll(&mut cx).is_pending());
        assert!(second.as_mut().poll(&mut cx).is_pending());
        assert!(third.as_mut().poll(&mut cx).is_pending());
        fill(&bandwidth);
        // Polling the newest waiter first cannot steal the oldest eligible turn.
        assert!(third.as_mut().poll(&mut cx).is_pending());
        assert!(matches!(first.as_mut().poll(&mut cx), Poll::Ready(Ok(1))));
        fill(&bandwidth);
        second_tx.send_replace(Stop::Cancel);
        assert!(matches!(
            second.as_mut().poll(&mut cx),
            Poll::Ready(Err(Error::Stopped))
        ));
        assert!(matches!(third.as_mut().poll(&mut cx), Poll::Ready(Ok(1))));
        assert!(bandwidth.state.lock().unwrap().queue.is_empty());
        drop((first, second, third));
        let mut pending = Box::pin(bandwidth.acquire("a", 100, &mut first_stop));
        assert!(pending.as_mut().poll(&mut cx).is_pending());
        assert_eq!(bandwidth.state.lock().unwrap().queue.len(), 1);
        drop(pending);
        assert!(bandwidth.state.lock().unwrap().queue.is_empty());
    }

    #[tokio::test]
    async fn slow_local_does_not_block_unlimited_or_global_peer() {
        let bandwidth = Bandwidth::new();
        bandwidth.set_local("slow", Some(1));
        let (_tx, mut stop) = watch::channel(Stop::Running);
        let (_peer_tx, mut peer_stop) = watch::channel(Stop::Running);
        let mut slow = Box::pin(bandwidth.acquire("slow", 100, &mut stop));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(slow.as_mut().poll(&mut cx).is_pending());
        assert_eq!(
            bandwidth
                .acquire("fast", 1_000_000, &mut peer_stop)
                .await
                .unwrap(),
            1_000_000
        );
        bandwidth.set_global(Some(1000));
        // Only refill global credits, keeping slow local ineligible.
        bandwidth
            .state
            .lock()
            .unwrap()
            .global
            .as_mut()
            .unwrap()
            .credit = 100 * SCALE;
        assert_eq!(
            bandwidth
                .acquire("fast", 1000, &mut peer_stop)
                .await
                .unwrap(),
            100
        );
        assert!(slow.as_mut().poll(&mut cx).is_pending());
        drop(slow);
    }

    #[tokio::test]
    async fn runtime_updates_notify_waiters_without_minting_credits() {
        let bandwidth = std::sync::Arc::new(Bandwidth::new());
        bandwidth.set_global(Some(1));
        bandwidth.set_local("a", Some(1));
        let (tx, mut stop) = watch::channel(Stop::Running);
        let cloned = bandwidth.clone();
        let task = tokio::spawn(async move { cloned.acquire("a", 100_000, &mut stop).await });
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
        bandwidth.set_global(Some(1_000_000));
        assert!(
            bandwidth
                .state
                .lock()
                .unwrap()
                .global
                .as_ref()
                .unwrap()
                .credit
                <= SCALE
        );
        bandwidth.set_global(None);
        assert!(!task.is_finished());
        bandwidth.set_local("a", None);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            100_000
        );
        drop(tx);
        assert!(!bandwidth.is_capped("a"));
    }
}
