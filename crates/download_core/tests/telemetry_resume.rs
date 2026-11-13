mod support;
use download_core::{
    Engine, ResumeCapability as Capability, ResumeReason, Status, TransferConfig, WorkerPhase,
};
use std::{sync::atomic::Ordering, time::Duration};
use support::{done, fixture, fixture_with_size, wait};

async fn item(engine: &Engine, url: &str, dir: &std::path::Path, max: u32) -> String {
    let d = engine
        .create(url.into(), dir.into(), "output.bin".into())
        .await
        .unwrap();
    engine
        .configure_download(
            &d.id,
            TransferConfig {
                max_connections: max,
                optimize: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(d.resume.capability, Capability::Unknown);
    assert!(d.resume.observed_at_ms.is_none());
    d.id
}

#[tokio::test]
async fn real_workers_wait_for_permits_and_clear_after_pause_retry_shutdown_recovery() {
    let f = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let metadata = dir.path().join("metadata");
    let engine = Engine::open(&metadata).await.unwrap();
    engine
        .configure_queue(download_core::QueueConfig {
            global_connections: 1,
            per_host_connections: 1,
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .set_global_bandwidth_limit(Some(2 * 1024 * 1024))
        .await
        .unwrap();
    let id = item(&engine, &f.url, dir.path(), 4).await;
    engine.start(&id).await.unwrap();
    let first = wait(&engine, &id, |d| {
        d.telemetry.workers.len() == 4 && d.telemetry.workers.iter().any(|w| w.written_bytes > 0)
    })
    .await;
    assert_eq!(first.active_connections, 1);
    assert_eq!(
        first
            .telemetry
            .workers
            .iter()
            .filter(|w| w.request_permit_held)
            .count(),
        1
    );
    assert!(
        first
            .telemetry
            .workers
            .iter()
            .any(|w| w.phase == WorkerPhase::WaitingForPermit)
    );
    assert_eq!(
        first.downloaded,
        first.durable_bytes
            + first
                .telemetry
                .workers
                .iter()
                .map(|w| w.written_bytes)
                .sum::<u64>()
    );
    assert!(
        first
            .telemetry
            .workers
            .iter()
            .all(
                |w| Some(&w.transfer_generation) == first.telemetry.generation.as_ref()
                    && w.sample_sequence == first.telemetry.sequence
                    && w.written_bytes <= w.range_end - w.range_start
            )
    );
    let generation = first.telemetry.generation;
    engine.pause(&id).await.unwrap();
    let paused = engine.get(&id).await.unwrap();
    assert!(paused.telemetry.workers.is_empty());
    assert_eq!(paused.active_connections, 0);
    assert_eq!(paused.resume.capability, Capability::Supported);
    engine.resume(&id).await.unwrap();
    let restarted = wait(&engine, &id, |d| !d.telemetry.workers.is_empty()).await;
    assert_ne!(restarted.telemetry.generation, generation);
    engine.shutdown().await.unwrap();
    assert!(engine.get(&id).await.unwrap().telemetry.workers.is_empty());
    drop(engine);
    let conn = rusqlite::Connection::open(metadata.join("downloads.sqlite3")).unwrap();
    let json: String = conn
        .query_row("SELECT metadata FROM downloads WHERE id=?1", [&id], |r| {
            r.get(0)
        })
        .unwrap();
    let saved: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(saved.get("telemetry").is_none() && saved.get("resume").is_none());
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        5
    );
    drop(conn);
    let engine = Engine::open(&metadata).await.unwrap();
    let restored = engine.get(&id).await.unwrap();
    assert_eq!(restored.status, Status::Paused);
    assert_eq!(restored.resume.capability, Capability::Unknown);
    assert!(restored.telemetry.generation.is_none() && restored.telemetry.workers.is_empty());
    engine.resume(&id).await.unwrap();
    wait(&engine, &id, |d| !d.telemetry.workers.is_empty()).await;
    engine.cancel(&id).await.unwrap();
    let cancelled = engine.get(&id).await.unwrap();
    assert_eq!(cancelled.active_connections, 0);
    assert!(cancelled.telemetry.workers.is_empty());
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn small_files_can_support_resume_without_segmentation_and_single_connection_is_unknown() {
    let f = fixture_with_size(1).await;
    for (max, expected) in [(4, Capability::Supported), (1, Capability::Unknown)] {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(dir.path().join("metadata")).await.unwrap();
        let id = item(&engine, &f.url, dir.path(), max).await;
        engine.start(&id).await.unwrap();
        let d = done(&engine, &id, &f.state.data).await;
        assert!(!d.segmented);
        assert_eq!(d.resume.capability, expected);
        assert!(d.resume.observed_at_ms.is_some());
        engine.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn resume_evidence_is_conservative_for_ignored_ranges_validators_auth_and_redirects() {
    for (mode, expected, reason) in [
        (1, Capability::Unsupported, ResumeReason::RangeIgnored),
        (
            5,
            Capability::Unsupported,
            ResumeReason::MissingStrongValidator,
        ),
        (
            10,
            Capability::Unknown,
            ResumeReason::AuthenticationRequired,
        ),
        (13, Capability::Supported, ResumeReason::ValidatedRange),
        (18, Capability::Unknown, ResumeReason::NotObserved),
    ] {
        let f = fixture_with_size(1).await;
        f.state.mode.store(mode, Ordering::SeqCst);
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(dir.path().join("metadata")).await.unwrap();
        let url = f.url.clone();
        let id = item(&engine, &url, dir.path(), 4).await;
        engine.start(&id).await.unwrap();
        let d = wait(&engine, &id, |d| {
            matches!(d.status, Status::Completed | Status::Failed)
        })
        .await;
        assert_eq!(d.resume.capability, expected, "mode {mode}: {d:?}");
        assert_eq!(d.resume.reason, reason, "mode {mode}");
        engine.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn single_stream_stall_publishes_zero_speed_without_waiting_for_timeout() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        assert!(socket.read(&mut request).await.unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1048576\r\nETag: \"stall-v1\"\r\n\r\n")
            .await
            .unwrap();
        socket.write_all(&vec![42; 128 * 1024]).await.unwrap();
        std::future::pending::<()>().await;
    });
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path().join("metadata")).await.unwrap();
    let id = item(&engine, &format!("http://{address}/file"), dir.path(), 1).await;
    engine.start(&id).await.unwrap();
    let flowing = tokio::time::timeout(
        Duration::from_secs(3),
        wait(&engine, &id, |d| {
            d.current_speed > 0.0 && d.downloaded == 128 * 1024
        }),
    )
    .await
    .unwrap();
    let stalled = tokio::time::timeout(
        Duration::from_secs(2),
        wait(&engine, &id, |d| {
            d.telemetry.sequence > flowing.telemetry.sequence && d.current_speed == 0.0
        }),
    )
    .await
    .unwrap();
    assert_eq!(stalled.downloaded, flowing.downloaded);
    assert_eq!(stalled.eta_seconds, None);
    assert_eq!(stalled.status, Status::Running);
    assert_eq!(stalled.resume.capability, Capability::Unknown);
    engine.cancel(&id).await.unwrap();
    engine.shutdown().await.unwrap();
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn single_stream_validates_real_resume_and_conditional_full_response_is_unknown() {
    for mode in [0, 1, 3] {
        let f = fixture().await;
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(dir.path().join("metadata")).await.unwrap();
        let id = item(&engine, &f.url, dir.path(), 1).await;
        engine.start(&id).await.unwrap();
        wait(&engine, &id, |d| d.downloaded >= 100_000).await;
        engine.pause(&id).await.unwrap();
        f.state.mode.store(mode, Ordering::SeqCst);
        engine.resume(&id).await.unwrap();
        let d = wait(&engine, &id, |d| {
            matches!(d.status, Status::Completed | Status::Failed)
        })
        .await;
        match mode {
            0 => {
                assert_eq!(d.status, Status::Completed);
                assert_eq!(d.resume.capability, Capability::Supported);
                assert_eq!(d.resume.reason, ResumeReason::ValidatedRange);
            }
            1 => {
                assert_eq!(d.status, Status::Completed);
                assert_eq!(d.resume.capability, Capability::Unknown);
                assert_eq!(d.resume.reason, ResumeReason::ConditionalFullResponse);
            }
            _ => {
                assert_eq!(d.status, Status::Failed);
                assert_eq!(d.resume.capability, Capability::Unknown);
                assert_eq!(d.resume.reason, ResumeReason::IdentityChanged);
            }
        }
        // One initial GET and one actual resume, no badge-only probe.
        assert_eq!(f.state.timestamps.lock().unwrap().len(), 2);
        engine.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn transient_retry_uses_a_new_generation_and_discards_previous_worker_snapshots() {
    let f = fixture().await;
    f.state.mode.store(7, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path().join("metadata")).await.unwrap();
    let id = item(&engine, &f.url, dir.path(), 4).await;
    engine.start(&id).await.unwrap();
    let waiting = wait(&engine, &id, |d| {
        d.retries > 0 && d.retry_remaining_seconds.is_some()
    })
    .await;
    assert!(waiting.telemetry.workers.is_empty());
    assert_eq!(waiting.active_connections, 0);
    f.state.mode.store(0, Ordering::SeqCst);
    let next = wait(&engine, &id, |d| !d.telemetry.workers.is_empty()).await;
    assert_ne!(next.telemetry.generation, waiting.telemetry.generation);
    assert!(
        next.telemetry
            .workers
            .iter()
            .all(|w| Some(&w.transfer_generation) == next.telemetry.generation.as_ref())
    );
    done(&engine, &id, &f.state.data).await;
    assert!(engine.get(&id).await.unwrap().telemetry.workers.is_empty());
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn local_publication_failure_does_not_claim_remote_resume_is_unsupported() {
    let f = fixture_with_size(1).await;
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path().join("metadata")).await.unwrap();
    let id = item(&engine, &f.url, dir.path(), 4).await;
    std::fs::write(dir.path().join("output.bin"), b"existing file").unwrap();
    engine.start(&id).await.unwrap();
    let failed = wait(&engine, &id, |d| d.status == Status::Failed).await;
    assert_eq!(failed.resume.capability, Capability::Supported);
    assert_eq!(failed.error_kind, Some(download_core::ErrorKind::Disk));
    assert_eq!(
        std::fs::read(dir.path().join("output.bin")).unwrap(),
        b"existing file"
    );
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn faster_worker_gets_a_new_reservation_while_slow_worker_retains_identity() {
    let f = fixture().await;
    f.state.mode.store(8, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path().join("metadata")).await.unwrap();
    let id = item(&engine, &f.url, dir.path(), 2).await;
    engine.start(&id).await.unwrap();
    let sample = wait(&engine, &id, |d| {
        d.telemetry
            .workers
            .iter()
            .any(|w| w.range_start == 0 && w.written_bytes > 0)
            && d.telemetry
                .workers
                .iter()
                .any(|w| w.worker_id > 2 && w.written_bytes > 0)
    })
    .await;
    let slow = sample
        .telemetry
        .workers
        .iter()
        .find(|w| w.range_start == 0)
        .unwrap();
    assert_eq!(slow.worker_id, 1);
    assert!(sample.durable_bytes >= 2 * 1024 * 1024);
    assert!(
        sample
            .telemetry
            .workers
            .iter()
            .any(|w| w.range_start >= 4 * 1024 * 1024)
    );
    assert_eq!(
        sample.downloaded,
        sample.durable_bytes
            + sample
                .telemetry
                .workers
                .iter()
                .map(|w| w.written_bytes)
                .sum::<u64>()
    );
    done(&engine, &id, &f.state.data).await;
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn segmented_identity_and_conditional_range_changes_invalidate_resume_evidence() {
    for (mode, reason) in [
        (3, ResumeReason::IdentityChanged),
        (1, ResumeReason::ConditionalFullResponse),
    ] {
        let f = fixture().await;
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(dir.path().join("metadata")).await.unwrap();
        let id = item(&engine, &f.url, dir.path(), 4).await;
        engine.start(&id).await.unwrap();
        wait(&engine, &id, |d| d.downloaded > 100_000).await;
        engine.pause(&id).await.unwrap();
        assert_eq!(
            engine.get(&id).await.unwrap().resume.capability,
            Capability::Supported
        );
        f.state.mode.store(mode, Ordering::SeqCst);
        engine.resume(&id).await.unwrap();
        let failed = wait(&engine, &id, |d| d.status == Status::Failed).await;
        assert_eq!(failed.resume.capability, Capability::Unknown);
        assert_eq!(failed.resume.reason, reason);
        assert!(failed.telemetry.workers.is_empty());
        assert!(!dir.path().join("output.bin").exists());
        engine.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn probes_and_single_streams_report_request_startup_without_inventing_workers() {
    use tokio::io::AsyncReadExt;
    for max in [1, 4] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            std::future::pending::<()>().await;
        });
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(dir.path().join("metadata")).await.unwrap();
        let id = item(&engine, &format!("http://{address}/file"), dir.path(), max).await;
        engine.start(&id).await.unwrap();
        let requesting = tokio::time::timeout(
            Duration::from_secs(2),
            wait(&engine, &id, |d| d.active_connections == 1),
        )
        .await
        .unwrap();
        assert!(requesting.telemetry.workers.is_empty());
        assert_eq!(requesting.resume.capability, Capability::Unknown);
        engine.cancel(&id).await.unwrap();
        assert_eq!(engine.get(&id).await.unwrap().active_connections, 0);
        engine.shutdown().await.unwrap();
        server.abort();
        let _ = server.await;
    }
}
