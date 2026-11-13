mod support;
use download_core::{ByteRange, Engine, QueueConfig, Status, TransferConfig};
use std::{sync::atomic::Ordering, time::Duration};
use support::*;
use tempfile::TempDir;
async fn setup(f: &Fixture, t: &TempDir, connections: u32) -> (Engine, String) {
    let engine = Engine::open(t.path().join("metadata")).await.unwrap();
    let d = engine
        .create(f.url.clone(), t.path().into(), "output.bin".into())
        .await
        .unwrap();
    engine
        .configure_download(
            &d.id,
            TransferConfig {
                max_connections: connections,
                optimize: false,
            },
        )
        .await
        .unwrap();
    (engine, d.id)
}
#[tokio::test]
async fn dynamic_ranges_out_of_order_and_correct_bytes() {
    let f = fixture().await;
    f.state.mode.store(8, Ordering::SeqCst);
    let t = TempDir::new().unwrap();
    let (e, id) = setup(&f, &t, 4).await;
    e.start(&id).await.unwrap();
    let d = done(&e, &id, &f.state.data).await;
    assert!(d.segmented);
    assert_eq!(
        d.completed_ranges,
        vec![ByteRange {
            start: 0,
            end: f.state.data.len() as u64
        }]
    );
    assert!(f.state.peak.load(Ordering::SeqCst) >= 2);
    // A later chunk is completed while the deliberately slow first 2 MiB worker is still busy.
    let order = f.state.finished.lock().unwrap().clone();
    let slow = order.iter().rposition(|n| *n == 0).unwrap();
    assert!(
        order[..slow].iter().any(|n| *n >= 4 * 1024 * 1024),
        "{order:?}"
    );
    e.shutdown().await.unwrap();
}
#[tokio::test]
async fn segmented_pause_recovery_and_missing_ranges_only() {
    let f = fixture().await;
    let t = TempDir::new().unwrap();
    let (e, id) = setup(&f, &t, 4).await;
    e.start(&id).await.unwrap();
    wait(&e, &id, |d| {
        d.durable_bytes >= 4 * 1024 * 1024 && d.status == Status::Running
    })
    .await;
    e.pause(&id).await.unwrap();
    let saved = e.get(&id).await.unwrap();
    assert!(saved.durable_bytes > 0);
    e.shutdown().await.unwrap();
    drop(e);
    f.state.requested.lock().unwrap().clear();
    let e = Engine::open(t.path().join("metadata")).await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(f.state.requested.lock().unwrap().is_empty());
    e.resume(&id).await.unwrap();
    done(&e, &id, &f.state.data).await;
    for &(start, end) in f
        .state
        .requested
        .lock()
        .unwrap()
        .iter()
        .filter(|&&(a, b)| !(a == 0 && b == 1))
    {
        assert!(
            saved
                .completed_ranges
                .iter()
                .all(|r| end <= r.start || start >= r.end)
        );
    }
    e.shutdown().await.unwrap();
}
#[tokio::test]
async fn fallback_unknown_sizes_missing_validators_and_redirects() {
    let f = fixture().await;
    for mode in [1, 5, 11, 13, 18] {
        f.state.mode.store(mode, Ordering::SeqCst);
        let t = TempDir::new().unwrap();
        let (e, id) = setup(&f, &t, 4).await;
        e.start(&id).await.unwrap();
        let d = done(&e, &id, &f.state.data).await;
        assert_eq!(d.segmented, mode == 13);
        if mode != 13 {
            assert!(d.notice.is_some());
        }
        e.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn integrity_errors_never_fall_back_silently() {
    let f = fixture().await;
    for mode in [2, 9, 10, 15] {
        f.state.mode.store(mode, Ordering::SeqCst);
        let t = TempDir::new().unwrap();
        let (e, id) = setup(&f, &t, 4).await;
        e.start(&id).await.unwrap();
        let d = wait(&e, &id, |d| d.status == Status::Failed).await;
        assert!(d.error.is_some());
        assert!(!t.path().join("output.bin").exists());
        e.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn changed_resource_validators_reject_segmented_recovery() {
    let f = fixture().await;
    for mode in [3, 4, 5] {
        f.state.mode.store(0, Ordering::SeqCst);
        let t = TempDir::new().unwrap();
        let (e, id) = setup(&f, &t, 2).await;
        e.start(&id).await.unwrap();
        wait(&e, &id, |d| d.durable_bytes > 0).await;
        e.pause(&id).await.unwrap();
        let durable = e.get(&id).await.unwrap().durable_bytes;
        f.state.mode.store(mode, Ordering::SeqCst);
        e.resume(&id).await.unwrap();
        let d = wait(&e, &id, |d| d.status == Status::Failed).await;
        assert_eq!(d.durable_bytes, durable);
        assert!(!t.path().join("output.bin").exists());
        e.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn retry_after_429_and_503_are_respected() {
    let f = fixture().await;
    for mode in [6, 14] {
        f.state.mode.store(mode, Ordering::SeqCst);
        f.state.throttle_budget.store(1, Ordering::SeqCst);
        f.state.timestamps.lock().unwrap().clear();
        let t = TempDir::new().unwrap();
        let (e, id) = setup(&f, &t, 4).await;
        e.start(&id).await.unwrap();
        done(&e, &id, &f.state.data).await;
        let times = f.state.timestamps.lock().unwrap().clone();
        assert!(times[1].duration_since(times[0]) >= Duration::from_millis(990));
        e.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn queue_is_fifo_bounded_and_persistent() {
    let f = fixture().await;
    let t = TempDir::new().unwrap();
    let (e, a) = setup(&f, &t, 1).await;
    e.configure_queue(QueueConfig {
        max_downloads: 1,
        global_connections: 2,
        per_host_connections: 1,
    })
    .await
    .unwrap();
    let b = e
        .create(f.url.clone(), t.path().into(), "b.bin".into())
        .await
        .unwrap()
        .id;
    let c = e
        .create(f.url.clone(), t.path().into(), "c.bin".into())
        .await
        .unwrap()
        .id;
    for id in [&a, &b, &c] {
        e.start(id).await.unwrap();
    }
    assert_eq!(e.get(&b).await.unwrap().queue_position, Some(1));
    assert_eq!(e.get(&c).await.unwrap().queue_position, Some(2));
    assert!(e.start(&b).await.is_err());
    e.cancel(&a).await.unwrap();
    wait(&e, &b, |d| d.status == Status::Running).await;
    assert_eq!(e.get(&c).await.unwrap().status, Status::Queued);
    e.shutdown().await.unwrap();
    let order = e.list().await.into_iter().map(|d| d.id).collect::<Vec<_>>();
    drop(e);
    let e = Engine::open(t.path().join("metadata")).await.unwrap();
    assert_eq!(
        e.list().await.into_iter().map(|d| d.id).collect::<Vec<_>>(),
        order
    );
    assert_eq!(e.get(&b).await.unwrap().status, Status::Paused);
    assert_eq!(e.get(&c).await.unwrap().status, Status::Paused);
    assert_eq!(e.queue_config().await.max_downloads, 1);
    e.shutdown().await.unwrap();
}
#[tokio::test]
async fn global_and_per_host_limits_and_cancel_all_writers() {
    let f = fixture().await;
    let t = TempDir::new().unwrap();
    let (e, id) = setup(&f, &t, 32).await;
    e.configure_queue(QueueConfig {
        max_downloads: 2,
        global_connections: 3,
        per_host_connections: 2,
    })
    .await
    .unwrap();
    let b = e
        .create(f.url.clone(), t.path().into(), "second.bin".into())
        .await
        .unwrap()
        .id;
    e.configure_download(
        &b,
        TransferConfig {
            max_connections: 16,
            optimize: false,
        },
    )
    .await
    .unwrap();
    e.start(&id).await.unwrap();
    e.start(&b).await.unwrap();
    wait(&e, &id, |d| d.downloaded > 100000).await;
    e.cancel(&id).await.unwrap();
    assert!(!std::path::Path::new(&e.get(&id).await.unwrap().partial_path).exists());
    done(&e, &b, &f.state.data).await;
    assert!(f.state.peak.load(Ordering::SeqCst) <= 2);
    e.shutdown().await.unwrap();
}
#[tokio::test]
async fn milestone_one_schema_migrates_without_losing_prefix() {
    let f = fixture().await;
    let t = TempDir::new().unwrap();
    let (e, id) = setup(&f, &t, 4).await;
    let mut d = e.get(&id).await.unwrap();
    e.shutdown().await.unwrap();
    drop(e);
    let prefix = 65536;
    tokio::fs::write(&d.partial_path, &f.state.data[..prefix])
        .await
        .unwrap();
    d.durable_bytes = prefix as u64;
    d.downloaded = d.durable_bytes;
    d.etag = Some("\"version-1\"".into());
    d.total = Some(f.state.data.len() as u64);
    d.status = Status::Running;
    let mut json = serde_json::to_value(d).unwrap();
    for key in [
        "completed_ranges",
        "config",
        "queue_order",
        "segmented",
        "resolved_url",
        "active_connections",
        "target_connections",
        "queue_position",
        "notice",
        "retries",
    ] {
        json.as_object_mut().unwrap().remove(key);
    }
    let db = rusqlite::Connection::open(t.path().join("metadata/downloads.sqlite3")).unwrap();
    db.execute_batch(
        "DROP TABLE organization_state; DROP TABLE file_moves; DROP TABLE automation_state; DROP TABLE engine_settings; PRAGMA user_version=1;",
    )
    .unwrap();
    db.execute(
        "UPDATE downloads SET metadata=?1 WHERE id=?2",
        rusqlite::params![json.to_string(), id],
    )
    .unwrap();
    drop(db);
    let e = Engine::open(t.path().join("metadata")).await.unwrap();
    let d = e.get(&id).await.unwrap();
    assert_eq!(d.status, Status::Paused);
    assert_eq!(
        d.completed_ranges,
        vec![ByteRange {
            start: 0,
            end: prefix as u64
        }]
    );
    e.resume(&id).await.unwrap();
    done(&e, &id, &f.state.data).await;
    e.shutdown().await.unwrap();
}

#[tokio::test]
async fn automatic_scaling_uses_stable_throughput_and_respects_maximum() {
    let f = fixture_with_size(32).await;
    f.state.mode.store(16, Ordering::SeqCst);
    let t = TempDir::new().unwrap();
    let (e, id) = setup(&f, &t, 4).await;
    e.configure_download(
        &id,
        TransferConfig {
            max_connections: 4,
            optimize: true,
        },
    )
    .await
    .unwrap();
    e.start(&id).await.unwrap();
    let first = wait(&e, &id, |d| d.downloaded > 100000).await;
    assert_eq!(first.target_connections, 1);
    let scaled = wait(&e, &id, |d| d.target_connections >= 2).await;
    assert!(scaled.target_connections <= 4);
    assert!(scaled.downloaded > 0);
    e.cancel(&id).await.unwrap();
    e.shutdown().await.unwrap();
}
#[tokio::test]
async fn repeated_connection_resets_fail_safely_then_retry_succeeds() {
    let f = fixture().await;
    f.state.mode.store(7, Ordering::SeqCst);
    let t = TempDir::new().unwrap();
    let (e, id) = setup(&f, &t, 4).await;
    e.start(&id).await.unwrap();
    let d = wait(&e, &id, |d| d.status == Status::Failed).await;
    assert!(!t.path().join("output.bin").exists());
    assert!(d.retries >= 3, "{d:?}");
    f.state.mode.store(0, Ordering::SeqCst);
    e.resume(&id).await.unwrap();
    done(&e, &id, &f.state.data).await;
    e.shutdown().await.unwrap();
}
#[tokio::test]
async fn stalled_body_times_out_and_retries_its_missing_range() {
    let f = fixture().await;
    f.state.mode.store(17, Ordering::SeqCst);
    f.state.throttle_budget.store(1, Ordering::SeqCst);
    let t = TempDir::new().unwrap();
    let (e, id) = setup(&f, &t, 2).await;
    e.start(&id).await.unwrap();
    let d = done(&e, &id, &f.state.data).await;
    assert!(d.retries >= 1);
    e.shutdown().await.unwrap();
}
#[tokio::test]
#[ignore = "subprocess helper invoked by segmented_process_termination"]
async fn segmented_child() {
    let directory = std::path::PathBuf::from(std::env::var("DLM_SEGMENT_DIRECTORY").unwrap());
    let e = Engine::open(directory.join("metadata")).await.unwrap();
    let d = e
        .create(
            std::env::var("DLM_SEGMENT_URL").unwrap(),
            directory.clone(),
            "output.bin".into(),
        )
        .await
        .unwrap();
    e.configure_download(
        &d.id,
        TransferConfig {
            max_connections: 2,
            optimize: false,
        },
    )
    .await
    .unwrap();
    e.set_global_bandwidth_limit(Some(8 * 1024 * 1024))
        .await
        .unwrap();
    e.set_download_bandwidth_limit(&d.id, Some(4 * 1024 * 1024))
        .await
        .unwrap();
    e.set_priority(&d.id, download_core::Priority::Low)
        .await
        .unwrap();
    e.configure_retry_policy(download_core::RetryPolicy {
        max_attempts: 5,
        ..download_core::RetryPolicy::default()
    })
    .await
    .unwrap();
    e.start(&d.id).await.unwrap();
    wait(&e, &d.id, |d| d.durable_bytes >= 2 * 1024 * 1024).await;
    tokio::fs::write(directory.join("checkpoint"), &d.id)
        .await
        .unwrap();
    std::future::pending::<()>().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn segmented_process_termination() {
    let f = fixture().await;
    let t = TempDir::new().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "segmented_child"])
        .env("DLM_SEGMENT_DIRECTORY", t.path())
        .env("DLM_SEGMENT_URL", &f.url)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        while !t.path().join("checkpoint").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    let id = tokio::fs::read_to_string(t.path().join("checkpoint"))
        .await
        .unwrap();
    let e = Engine::open(t.path().join("metadata")).await.unwrap();
    let d = e.get(&id).await.unwrap();
    assert_eq!(d.status, Status::Paused);
    assert!(d.telemetry.workers.is_empty());
    assert!(d.telemetry.generation.is_none());
    assert_eq!(
        d.resume.capability,
        download_core::ResumeCapability::Unknown
    );
    assert!(d.segmented);
    assert!(d.durable_bytes >= 2 * 1024 * 1024);
    assert_eq!(e.global_bandwidth_limit().await, Some(8 * 1024 * 1024));
    assert_eq!(d.bandwidth_limit, Some(4 * 1024 * 1024));
    assert_eq!(d.priority, download_core::Priority::Low);
    assert_eq!(e.retry_policy().await.max_attempts, 5);
    e.resume(&id).await.unwrap();
    done(&e, &id, &f.state.data).await;
    e.shutdown().await.unwrap();
}

#[tokio::test]
async fn global_limit_applies_across_distinct_hosts() {
    let f = fixture().await;
    let t = TempDir::new().unwrap();
    let (e, a) = setup(&f, &t, 8).await;
    e.configure_queue(QueueConfig {
        max_downloads: 2,
        global_connections: 3,
        per_host_connections: 2,
    })
    .await
    .unwrap();
    let b = e
        .create(
            f.url.replace("127.0.0.1", "localhost"),
            t.path().into(),
            "other-host.bin".into(),
        )
        .await
        .unwrap()
        .id;
    e.configure_download(
        &b,
        TransferConfig {
            max_connections: 8,
            optimize: false,
        },
    )
    .await
    .unwrap();
    e.start(&a).await.unwrap();
    e.start(&b).await.unwrap();
    done(&e, &a, &f.state.data).await;
    done(&e, &b, &f.state.data).await;
    assert!(f.state.peak.load(Ordering::SeqCst) <= 3);
    e.shutdown().await.unwrap();
}
#[tokio::test]
async fn automatic_queue_advances_on_completion_and_failure() {
    let f = fixture().await;
    let t = TempDir::new().unwrap();
    let (e, a) = setup(&f, &t, 4).await;
    e.configure_queue(QueueConfig {
        max_downloads: 1,
        ..QueueConfig::default()
    })
    .await
    .unwrap();
    let b = e
        .create(f.url.clone(), t.path().into(), "next.bin".into())
        .await
        .unwrap()
        .id;
    e.start(&a).await.unwrap();
    e.start(&b).await.unwrap();
    done(&e, &a, &f.state.data).await;
    done(&e, &b, &f.state.data).await;
    let c = e
        .create(
            "http://127.0.0.1:1/unavailable".into(),
            t.path().into(),
            "fail.bin".into(),
        )
        .await
        .unwrap()
        .id;
    let d = e
        .create(f.url.clone(), t.path().into(), "after-failure.bin".into())
        .await
        .unwrap()
        .id;
    e.start(&c).await.unwrap();
    e.start(&d).await.unwrap();
    wait(&e, &c, |d| d.status == Status::Failed).await;
    done(&e, &d, &f.state.data).await;
    e.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "optional mounted-filesystem test: set DLM_TEST_DESTINATION"]
async fn mounted_filesystem_finalization() {
    let destination = std::path::PathBuf::from(std::env::var("DLM_TEST_DESTINATION").unwrap());
    let target = tempfile::tempdir_in(&destination).unwrap();
    let metadata = TempDir::new().unwrap();
    let f = fixture().await;
    let e = Engine::open(metadata.path()).await.unwrap();
    let d = e
        .create(f.url.clone(), target.path().into(), "external.bin".into())
        .await
        .unwrap();
    e.configure_download(
        &d.id,
        TransferConfig {
            max_connections: 4,
            optimize: false,
        },
    )
    .await
    .unwrap();
    e.start(&d.id).await.unwrap();
    done(&e, &d.id, &f.state.data).await;
    assert!(!std::path::Path::new(&d.partial_path).exists());
    assert!(
        e.create(f.url.clone(), target.path().into(), "external.bin".into())
            .await
            .is_err()
    );
    e.shutdown().await.unwrap();
}

#[tokio::test]
async fn retry_after_deadline_survives_pause_and_restart() {
    let f = fixture().await;
    f.state.mode.store(6, Ordering::SeqCst);
    f.state.throttle_budget.store(1, Ordering::SeqCst);
    let t = TempDir::new().unwrap();
    let (e, id) = setup(&f, &t, 4).await;
    e.start(&id).await.unwrap();
    wait(&e, &id, |d| d.retry_not_before_ms.is_some()).await;
    e.pause(&id).await.unwrap();
    e.shutdown().await.unwrap();
    drop(e);
    let e = Engine::open(t.path().join("metadata")).await.unwrap();
    e.resume(&id).await.unwrap();
    done(&e, &id, &f.state.data).await;
    let times = f.state.timestamps.lock().unwrap().clone();
    assert!(times[1].duration_since(times[0]) >= Duration::from_millis(990));
    e.shutdown().await.unwrap();
}

#[tokio::test]
async fn server_connection_cap_reduces_live_concurrency() {
    let f = fixture().await;
    f.state.mode.store(19, Ordering::SeqCst);
    let t = TempDir::new().unwrap();
    let (e, id) = setup(&f, &t, 8).await;
    e.start(&id).await.unwrap();
    let d = done(&e, &id, &f.state.data).await;
    assert!(d.retries > 0);
    assert!(d.target_connections < 8);
    e.shutdown().await.unwrap();
}

#[tokio::test]
async fn immediate_failure_notification_can_be_reconfigured_and_retried() {
    let f = fixture().await;
    f.state.mode.store(10, Ordering::SeqCst);
    let t = TempDir::new().unwrap();
    let (e, id) = setup(&f, &t, 4).await;
    let mut events = e.subscribe();
    e.start(&id).await.unwrap();
    loop {
        if e.get(&id).await.unwrap().status == Status::Failed {
            break;
        }
        events.changed().await.unwrap();
    }
    f.state.mode.store(0, Ordering::SeqCst);
    e.configure_download(
        &id,
        TransferConfig {
            max_connections: 2,
            optimize: false,
        },
    )
    .await
    .unwrap();
    e.resume(&id).await.unwrap();
    let d = done(&e, &id, &f.state.data).await;
    assert_eq!(d.config.max_connections, 2);
    e.shutdown().await.unwrap();
}

#[tokio::test]
async fn complete_durable_ranges_finalize_even_after_url_expiry() {
    let f = fixture().await;
    let t = TempDir::new().unwrap();
    let (e, id) = setup(&f, &t, 4).await;
    let mut d = e.get(&id).await.unwrap();
    e.shutdown().await.unwrap();
    drop(e);
    tokio::fs::write(&d.partial_path, f.state.data.as_slice())
        .await
        .unwrap();
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&d.partial_path)
        .await
        .unwrap();
    file.sync_all().await.unwrap();
    drop(file);
    d.total = Some(f.state.data.len() as u64);
    d.durable_bytes = d.total.unwrap();
    d.downloaded = d.durable_bytes;
    d.completed_ranges = vec![ByteRange {
        start: 0,
        end: d.durable_bytes,
    }];
    d.segmented = true;
    d.status = Status::Running;
    d.etag = Some("\"version-1\"".into());
    let db = rusqlite::Connection::open(t.path().join("metadata/downloads.sqlite3")).unwrap();
    db.execute(
        "UPDATE downloads SET metadata=?1 WHERE id=?2",
        rusqlite::params![serde_json::to_string(&d).unwrap(), id],
    )
    .unwrap();
    drop(db);
    f.state.mode.store(10, Ordering::SeqCst);
    let e = Engine::open(t.path().join("metadata")).await.unwrap();
    e.resume(&id).await.unwrap();
    done(&e, &id, &f.state.data).await;
    assert!(f.state.timestamps.lock().unwrap().is_empty());
    e.shutdown().await.unwrap();
}
