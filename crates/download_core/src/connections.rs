use crate::{Download, Engine, Error, QueueConfig, Result, engine::Stop};
use reqwest::{Response, header};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};
use tokio::sync::{Notify, watch};

struct Counts {
    config: QueueConfig,
    total: u32,
    hosts: HashMap<String, u32>,
    downloads: HashMap<String, u32>,
    cooldowns: HashMap<String, Instant>,
    targets: HashMap<String, u32>,
}
pub(crate) struct Connections {
    counts: Mutex<Counts>,
    notify: Notify,
}
pub(crate) struct Permit {
    worker: Option<Arc<crate::telemetry::WorkerStats>>,
    limiter: Arc<Connections>,
    host: String,
    id: String,
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut counts = self.limiter.counts.lock().expect("connection counter lock");
        counts.total -= 1;
        if let Some(worker) = &self.worker {
            worker.permit(false);
        }
        if let Some(n) = counts.hosts.get_mut(&self.host) {
            *n -= 1;
            if *n == 0 {
                counts.hosts.remove(&self.host);
            }
        }
        if let Some(n) = counts.downloads.get_mut(&self.id) {
            *n -= 1;
            if *n == 0 {
                counts.downloads.remove(&self.id);
            }
        }
        drop(counts);
        self.limiter.notify.notify_waiters();
    }
}
impl Connections {
    pub fn new(config: QueueConfig) -> Arc<Self> {
        Arc::new(Self {
            counts: Mutex::new(Counts {
                config,
                total: 0,
                hosts: HashMap::new(),
                downloads: HashMap::new(),
                cooldowns: HashMap::new(),
                targets: HashMap::new(),
            }),
            notify: Notify::new(),
        })
    }
    pub fn configure(&self, config: QueueConfig) -> Result<()> {
        let mut counts = self.counts.lock().expect("connection counter lock");
        if counts.total > config.global_connections
            || counts
                .hosts
                .values()
                .any(|n| *n > config.per_host_connections)
        {
            return Err(Error::Busy(
                "Pause transfers before lowering limits below active connections".into(),
            ));
        }
        counts.config = config;
        drop(counts);
        self.notify.notify_waiters();
        Ok(())
    }
    pub fn target(&self, id: &str, target: Option<u32>) {
        let mut counts = self.counts.lock().expect("connection counter lock");
        let changed = match target {
            Some(target) => counts.targets.insert(id.into(), target) != Some(target),
            None => counts.targets.remove(id).is_some(),
        };
        drop(counts);
        if changed {
            self.notify.notify_waiters();
        }
    }
    pub fn active(&self, id: &str) -> u32 {
        *self
            .counts
            .lock()
            .expect("connection counter lock")
            .downloads
            .get(id)
            .unwrap_or(&0)
    }
    pub fn sample_workers<'a>(
        &self,
        id: &str,
        generation: &str,
        sequence: u64,
        workers: impl Iterator<Item = &'a Arc<crate::telemetry::WorkerStats>>,
    ) -> (Vec<crate::WorkerProgress>, u32) {
        let counts = self.counts.lock().expect("connection counter lock");
        let mut snapshots: Vec<_> = workers.map(|w| w.snapshot(generation, sequence)).collect();
        snapshots.sort_by_key(|w| w.worker_id);
        (snapshots, *counts.downloads.get(id).unwrap_or(&0))
    }
    pub fn throttle(&self, host: &str, delay: Duration) {
        let mut counts = self.counts.lock().expect("connection counter lock");
        let until = Instant::now()
            .checked_add(delay)
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(86400 * 365));
        let entry = counts.cooldowns.entry(host.into()).or_insert(until);
        *entry = (*entry).max(until);
        drop(counts);
        self.notify.notify_waiters();
    }
    pub(crate) async fn acquire(
        self: &Arc<Self>,
        host: String,
        id: String,
        stop: &mut watch::Receiver<Stop>,
    ) -> Result<Permit> {
        self.acquire_tracked(host, id, stop, None).await
    }
    async fn acquire_tracked(
        self: &Arc<Self>,
        host: String,
        id: String,
        stop: &mut watch::Receiver<Stop>,
        worker: Option<Arc<crate::telemetry::WorkerStats>>,
    ) -> Result<Permit> {
        loop {
            if *stop.borrow() != Stop::Running {
                return Err(Error::Stopped);
            }
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let delay = {
                let mut c = self.counts.lock().expect("connection counter lock");
                c.cooldowns.retain(|_, until| *until > Instant::now());
                let delay = c
                    .cooldowns
                    .get(&host)
                    .map(|until| until.saturating_duration_since(Instant::now()));
                if delay.is_none()
                    && c.total < c.config.global_connections
                    && *c.hosts.get(&host).unwrap_or(&0) < c.config.per_host_connections
                    && *c.downloads.get(&id).unwrap_or(&0) < *c.targets.get(&id).unwrap_or(&32)
                {
                    if let Some(worker) = &worker {
                        worker.permit(true);
                        worker.phase(crate::WorkerPhase::Requesting);
                    }
                    c.total += 1;
                    *c.hosts.entry(host.clone()).or_default() += 1;
                    *c.downloads.entry(id.clone()).or_default() += 1;
                    return Ok(Permit {
                        worker,
                        limiter: self.clone(),
                        host,
                        id,
                    });
                }
                delay.unwrap_or(Duration::from_secs(30))
            };
            tokio::select! { _=notified=>{}, _=tokio::time::sleep(delay)=>{}, _=stop.changed()=>return Err(Error::Stopped) }
            if *stop.borrow() != Stop::Running {
                return Err(Error::Stopped);
            }
        }
    }
}
pub(crate) struct HttpResponse {
    pub response: Response,
    _permit: Permit,
}
pub(crate) fn retryable(error: &Error) -> bool {
    crate::retry::is_retryable(error)
}
pub(crate) fn deadline_ms(seconds: u64) -> u64 {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64;
    now.saturating_add(seconds.saturating_mul(1000))
}
fn retry_after(response: &Response) -> u64 {
    parse_retry_after(
        response
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok()),
        SystemTime::now(),
    )
}
fn parse_retry_after(value: Option<&str>, now: SystemTime) -> u64 {
    value
        .and_then(|value| {
            let value = value.trim();
            if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Some(value.parse::<u64>().unwrap_or(u64::MAX));
            }
            httpdate::parse_http_date(value).ok().map(|date| {
                let remaining = date.duration_since(now).unwrap_or_default();
                remaining
                    .as_secs()
                    .saturating_add(u64::from(remaining.subsec_nanos() > 0))
            })
        })
        .unwrap_or(0)
}
/// Redirects are explicit so every actual destination obeys the host limit.
pub(crate) async fn send(
    engine: &Engine,
    d: &Download,
    range: Option<String>,
    validator: Option<&str>,
    stop: &mut watch::Receiver<Stop>,
) -> Result<HttpResponse> {
    send_tracked(engine, d, range, validator, stop, None).await
}
pub(crate) async fn send_tracked(
    engine: &Engine,
    d: &Download,
    range: Option<String>,
    validator: Option<&str>,
    stop: &mut watch::Receiver<Stop>,
    worker: Option<Arc<crate::telemetry::WorkerStats>>,
) -> Result<HttpResponse> {
    send_impl(engine, d, range, validator, stop, worker, true).await
}
/// Same limits/redirects/cooldowns, without publishing a candidate URL as live state.
pub(crate) async fn send_verification(
    engine: &Engine,
    d: &Download,
    range: String,
    validator: Option<&str>,
    stop: &mut watch::Receiver<Stop>,
) -> Result<HttpResponse> {
    send_impl(engine, d, Some(range), validator, stop, None, false).await
}
async fn send_impl(
    engine: &Engine,
    d: &Download,
    range: Option<String>,
    validator: Option<&str>,
    stop: &mut watch::Receiver<Stop>,
    worker: Option<Arc<crate::telemetry::WorkerStats>>,
    publish: bool,
) -> Result<HttpResponse> {
    let mut url = reqwest::Url::parse(&d.url).map_err(|e| Error::Invalid(e.to_string()))?;
    for _ in 0..=10 {
        if *stop.borrow() != Stop::Running {
            return Err(Error::Stopped);
        }
        let host = url
            .host_str()
            .ok_or_else(|| Error::Invalid("Missing host".into()))?
            .to_owned();
        if let Some(worker) = &worker {
            worker.phase(crate::WorkerPhase::WaitingForPermit);
        }
        let permit = engine
            .inner
            .connections
            .acquire_tracked(host.clone(), d.id.clone(), stop, worker.clone())
            .await?;
        if worker.is_none() && publish {
            // Probes/single streams have no range coordinator to publish permit
            // ownership while waiting for response headers.
            engine.publish(d).await;
        }
        let mut request = engine
            .inner
            .client
            .get(url.clone())
            .header(header::ACCEPT_ENCODING, "identity");
        if let Some(range) = &range {
            request = request.header(header::RANGE, range);
        }
        if let Some(validator) = validator {
            request = request.header(header::IF_RANGE, validator);
        }
        let response = tokio::select! {result=request.send()=>result?, _=stop.changed()=>return Err(Error::Stopped)};
        let retry_after_secs = retry_after(&response);
        if response.headers().contains_key(header::RETRY_AFTER)
            && retry_after_secs > crate::retry::MAX_RETRY_AFTER_SECS
        {
            return Err(Error::Protocol(
                "Retry-After exceeds the supported one-year wait horizon; retry manually later"
                    .into(),
            ));
        }
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| Error::Protocol("Redirect missing Location".into()))?;
            let next = url
                .join(location)
                .map_err(|e| Error::Protocol(e.to_string()))?;
            if !matches!(next.scheme(), "http" | "https")
                || !next.username().is_empty()
                || next.password().is_some()
            {
                return Err(Error::Protocol("Unsafe redirect URL".into()));
            }
            if response.headers().contains_key(header::RETRY_AFTER) {
                engine.inner.connections.throttle(
                    next.host_str().unwrap_or_default(),
                    Duration::from_secs(retry_after_secs),
                );
            }
            url = next;
            drop(response);
            drop(permit);
            continue;
        }
        if response.status().is_client_error() || response.status().is_server_error() {
            let status = response.status().as_u16();
            if retry_after_secs > crate::retry::MAX_RETRY_AFTER_SECS {
                return Err(Error::Protocol(
                    "Retry-After exceeds the supported one-year wait horizon; retry manually later"
                        .into(),
                ));
            }
            if retryable(&Error::Http {
                status,
                retry_after_secs,
            }) {
                engine
                    .inner
                    .connections
                    .throttle(&host, Duration::from_secs(retry_after_secs));
            }
            return Err(Error::Http {
                status,
                retry_after_secs,
            });
        }
        return Ok(HttpResponse {
            response,
            _permit: permit,
        });
    }
    Err(Error::Protocol("Too many redirects".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_seconds_dates_and_invalid_values() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        assert_eq!(parse_retry_after(Some("60"), now), 60);
        assert_eq!(parse_retry_after(Some(" 60 "), now), 60);
        assert_eq!(
            parse_retry_after(Some("999999999999999999999999999999"), now),
            u64::MAX
        );
        assert_eq!(parse_retry_after(None, now), 0);
        assert_eq!(parse_retry_after(Some("invalid"), now), 0);
        let date = httpdate::fmt_http_date(now + Duration::from_secs(61));
        assert_eq!(parse_retry_after(Some(&date), now), 61);
        assert_eq!(
            parse_retry_after(Some(&date), now + Duration::from_millis(500)),
            61
        );
        assert_eq!(
            parse_retry_after(Some(&date), now + Duration::from_secs(100)),
            0
        );
    }

    #[tokio::test]
    async fn cancellation_interrupts_cooldown_and_never_acquires_permit() {
        let limiter = Connections::new(QueueConfig::default());
        limiter.throttle("example.test", Duration::from_secs(60));
        let (sender, mut stop) = watch::channel(Stop::Running);
        let waiting = limiter.acquire("example.test".into(), "id".into(), &mut stop);
        let cancel = async {
            tokio::task::yield_now().await;
            sender.send(Stop::Pause).unwrap();
        };
        let (result, ()) = tokio::join!(waiting, cancel);
        assert!(matches!(result, Err(Error::Stopped)));
        assert_eq!(limiter.active("id"), 0);
        assert!(matches!(
            limiter
                .acquire("other.test".into(), "id".into(), &mut stop)
                .await,
            Err(Error::Stopped)
        ));
    }

    #[tokio::test]
    async fn transient_network_failures_are_classified() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let error = reqwest::Client::new()
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap_err();
        assert!(retryable(&Error::Network(error)));
        let builder = reqwest::Client::new().get("not a URL").build().unwrap_err();
        assert!(!retryable(&Error::Network(builder)));
    }

    #[tokio::test]
    async fn truncated_identity_body_remains_retryable() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nx")
                .await
                .unwrap();
        });
        let error = reqwest::get(format!("http://{address}"))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap_err();
        assert!(retryable(&Error::Network(error)));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn body_timeout_wrapped_as_decode_remains_retryable() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nx")
                .await
                .unwrap();
            std::future::pending::<()>().await;
        });
        let response = reqwest::Client::builder()
            .read_timeout(Duration::from_millis(50))
            .build()
            .unwrap()
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap();
        let error = response.bytes().await.unwrap_err();
        assert!(error.is_timeout());
        assert!(retryable(&Error::Network(error)));
        server.abort();
        let _ = server.await;
    }
}

#[cfg(test)]
mod telemetry_tests {
    use super::*;
    use crate::{ByteRange, WorkerPhase, telemetry::WorkerStats};
    #[tokio::test]
    async fn permit_phase_and_count_are_sampled_together_through_completion_and_cancellation() {
        let limiter = Connections::new(QueueConfig {
            global_connections: 1,
            ..Default::default()
        });
        let a = Arc::new(WorkerStats::new(1, ByteRange { start: 0, end: 100 }));
        let b = Arc::new(WorkerStats::new(
            2,
            ByteRange {
                start: 100,
                end: 200,
            },
        ));
        let (_tx, mut stop) = watch::channel(Stop::Running);
        let permit = limiter
            .acquire_tracked("host".into(), "id".into(), &mut stop, Some(a.clone()))
            .await
            .unwrap();
        let sample = |seq| limiter.sample_workers("id", "generation", seq, [&a, &b].into_iter());
        let (workers, count) = sample(1);
        assert_eq!(count, 1);
        assert_eq!(workers[0].phase, WorkerPhase::Requesting);
        assert!(workers[0].request_permit_held);
        assert_eq!(workers[1].phase, WorkerPhase::WaitingForPermit);
        assert!(!workers[1].request_permit_held);
        a.phase(WorkerPhase::Transferring);
        a.progress.store(100, std::sync::atomic::Ordering::Relaxed);
        a.phase(WorkerPhase::Synchronizing);
        assert_eq!(sample(2).0[0].phase, WorkerPhase::Synchronizing);
        let (tx, mut waiting_stop) = watch::channel(Stop::Running);
        tx.send_replace(Stop::Cancel);
        assert!(matches!(
            limiter
                .acquire_tracked(
                    "host".into(),
                    "id".into(),
                    &mut waiting_stop,
                    Some(b.clone())
                )
                .await,
            Err(Error::Stopped)
        ));
        a.phase(WorkerPhase::Completed);
        drop(permit);
        let (workers, count) = sample(3);
        assert_eq!(count, 0);
        assert_eq!(workers[0].phase, WorkerPhase::Completed);
        assert_eq!(workers[0].written_bytes, 100);
        assert!(workers.iter().all(|w| !w.request_permit_held));
    }
}
