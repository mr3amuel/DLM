use axum::{
    Router,
    body::{Body, Bytes},
    extract::{OriginalUri, State},
    http::{HeaderMap, Response},
    routing::get,
};
use download_core::{Engine, RecoveryOutcome, Status};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;

#[derive(Clone)]
struct Source {
    data: Arc<Vec<u8>>,
    mode: Arc<AtomicUsize>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}
struct Fixture {
    url: String,
    source: Source,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(
    State(s): State<Source>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response<Body> {
    if uri.path() == "/redirect" {
        return Response::builder()
            .status(302)
            .header("Location", "/file?secret=redirect")
            .body(Body::empty())
            .unwrap();
    }
    let mode = s.mode.load(Ordering::SeqCst);
    if mode == 11 {
        s.entered.notify_one();
        s.release.notified().await;
    }
    if mode == 4 || mode == 7 {
        return Response::builder()
            .status(if mode == 4 { 403 } else { 503 })
            .body(Body::empty())
            .unwrap();
    }
    let range = headers
        .get("range")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .map(|r| r.split_once('-').unwrap())
        .map(|(a, b)| {
            (
                a.parse::<usize>().unwrap(),
                if b.is_empty() {
                    s.data.len() - 1
                } else {
                    b.parse().unwrap()
                },
            )
        });
    let (a, b) = range.unwrap_or((0, s.data.len() - 1));
    let ranged = range.is_some() && mode != 3;
    let (start, end) = if ranged {
        (a, b)
    } else {
        (0, s.data.len() - 1)
    };
    let mut bytes = s.data[start..=end].to_vec();
    // Deliberately mismatch beyond the usual first/last sample bytes.
    if mode == 1 && start <= 12345 && end >= 12345 {
        bytes[12345 - start] ^= 1;
    }
    let mut builder = Response::builder().status(if ranged { 206 } else { 200 });
    if mode != 2 {
        builder = builder.header(
            "ETag",
            if mode == 8 && range != Some((0, 0)) {
                "\"changed\""
            } else {
                "\"candidate\""
            },
        );
    }
    if ranged {
        builder = builder.header(
            "Content-Range",
            format!(
                "bytes {}-{}/{}",
                if mode == 6 { start + 1 } else { start },
                end,
                s.data.len() + usize::from(mode == 9)
            ),
        );
    }
    if mode == 10 {
        builder = builder.header("Content-Encoding", "gzip");
    }
    builder.body(Body::from(Bytes::from(bytes))).unwrap()
}
async fn fixture() -> Fixture {
    let source = Source {
        data: Arc::new((0..512 * 1024).map(|i| (i % 251) as u8).collect()),
        mode: Arc::new(AtomicUsize::new(0)),
        entered: Arc::new(Notify::new()),
        release: Arc::new(Notify::new()),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/file", get(serve))
        .route("/redirect", get(serve))
        .with_state(source.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Fixture { url, source, task }
}
// Disposable pre-existing v3 database/checkpoint fixture. No production metadata.
async fn seeded(root: &std::path::Path, fixture: &Fixture, segmented: bool) -> (Engine, String) {
    let engine = Engine::open(root.join("metadata")).await.unwrap();
    let d = engine
        .create(
            format!("{}/expired?token=old-secret", fixture.url),
            root.into(),
            "result.bin".into(),
        )
        .await
        .unwrap();
    engine.shutdown().await.unwrap();
    drop(engine);
    std::fs::write(&d.partial_path, &fixture.source.data[..128 * 1024]).unwrap();
    let db = rusqlite::Connection::open(root.join("metadata/downloads.sqlite3")).unwrap();
    let mut value = serde_json::to_value(&d).unwrap();
    value["status"] = "Paused".into();
    value["total"] = (fixture.source.data.len() as u64).into();
    value["durable_bytes"] = (64 * 1024).into();
    value["downloaded"] = (64 * 1024).into();
    value["etag"] = "\"original\"".into();
    value["segmented"] = segmented.into();
    value["config"] = serde_json::json!({"max_connections":1,"optimize":false});
    value["completed_ranges"] = if segmented {
        serde_json::json!([{"start":0,"end":32768},{"start":65536,"end":98304}])
    } else {
        serde_json::json!([{"start":0,"end":65536}])
    };
    db.execute(
        "UPDATE downloads SET metadata=?1 WHERE id=?2",
        rusqlite::params![value.to_string(), d.id],
    )
    .unwrap();
    drop(db);
    (Engine::open(root.join("metadata")).await.unwrap(), d.id)
}
async fn wait(engine: &Engine, id: &str, status: Status) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let d = engine.get(id).await.unwrap();
            if d.status == status {
                break;
            }
            if d.status == Status::Failed && status != Status::Failed {
                panic!("{:?}", d.error);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn complete_checkpoint_comparison_preserves_identity_settings_and_bytes() {
    for segmented in [false, true] {
        let f = fixture().await;
        let root = tempfile::tempdir().unwrap();
        let (engine, id) = seeded(root.path(), &f, segmented).await;
        let old = engine.get(&id).await.unwrap();
        let bytes = std::fs::read(&old.partial_path).unwrap();
        let result = engine
            .verify_download_link(&id, &format!("{}/redirect?token=new-secret", f.url))
            .await
            .unwrap();
        assert_eq!(result.outcome, RecoveryOutcome::SafeToResume);
        assert_eq!(result.compared_bytes, old.durable_bytes);
        assert_eq!(std::fs::read(&old.partial_path).unwrap(), bytes);
        assert_eq!(engine.get(&id).await.unwrap().url, old.url); // verification is read-only
        engine
            .apply_download_link(&id, result.token.as_ref().unwrap(), false)
            .await
            .unwrap();
        let new = engine.get(&id).await.unwrap();
        assert_eq!(
            (
                new.id,
                new.destination,
                new.created_at_ms,
                new.config.max_connections,
                new.durable_bytes
            ),
            (
                old.id.clone(),
                old.destination.clone(),
                old.created_at_ms,
                old.config.max_connections,
                old.durable_bytes
            )
        );
        engine.start(&id).await.unwrap();
        wait(&engine, &id, Status::Completed).await;
        assert_eq!(std::fs::read(&old.destination).unwrap(), *f.source.data);
        engine.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn incompatible_unverifiable_and_expired_replacements_never_change_data_or_url() {
    let f = fixture().await;
    let root = tempfile::tempdir().unwrap();
    let (engine, id) = seeded(root.path(), &f, false).await;
    let old = engine.get(&id).await.unwrap();
    let original = std::fs::read(&old.partial_path).unwrap();
    for (mode, outcome) in [
        (1, RecoveryOutcome::Incompatible),
        (2, RecoveryOutcome::CannotVerify),
        (3, RecoveryOutcome::RangeUnsupported),
        (4, RecoveryOutcome::CannotVerify),
        (6, RecoveryOutcome::CannotVerify),
        (7, RecoveryOutcome::TemporaryFailure),
        (8, RecoveryOutcome::CannotVerify),
        (9, RecoveryOutcome::Incompatible),
        (10, RecoveryOutcome::CannotVerify),
    ] {
        f.source.mode.store(mode, Ordering::SeqCst);
        let result = engine
            .verify_download_link(&id, &format!("{}/file?token=do-not-log", f.url))
            .await
            .unwrap();
        assert_eq!(result.outcome, outcome, "mode {mode}");
        assert!(!result.reason.contains("do-not-log"));
        assert!(
            engine
                .apply_download_link(&id, result.token.as_ref().unwrap(), false)
                .await
                .is_err()
        );
        assert_eq!(engine.get(&id).await.unwrap().url, old.url);
        assert_eq!(std::fs::read(&old.partial_path).unwrap(), original);
    }
    engine.shutdown().await.unwrap();
}
#[tokio::test]
async fn corruption_and_stale_tickets_are_detected_before_commit_and_transfer() {
    let f = fixture().await;
    let root = tempfile::tempdir().unwrap();
    let (engine, id) = seeded(root.path(), &f, false).await;
    let old = engine.get(&id).await.unwrap();
    let first = engine
        .verify_download_link(&id, &format!("{}/file", f.url))
        .await
        .unwrap();
    let second = engine
        .verify_download_link(&id, &format!("{}/file", f.url))
        .await
        .unwrap();
    assert!(
        engine
            .apply_download_link(&id, first.token.as_ref().unwrap(), false)
            .await
            .is_err()
    );
    let mut corrupt = std::fs::read(&old.partial_path).unwrap();
    corrupt[12345] ^= 1;
    std::fs::write(&old.partial_path, &corrupt).unwrap();
    assert!(
        engine
            .apply_download_link(&id, second.token.as_ref().unwrap(), false)
            .await
            .is_err()
    );
    assert_eq!(engine.get(&id).await.unwrap().url, old.url);
    let bad = engine
        .verify_download_link(&id, &format!("{}/file", f.url))
        .await
        .unwrap();
    assert_eq!(bad.outcome, RecoveryOutcome::Incompatible);
    corrupt[12345] ^= 1;
    std::fs::write(&old.partial_path, &corrupt).unwrap();
    let good = engine
        .verify_download_link(&id, &format!("{}/file", f.url))
        .await
        .unwrap();
    engine
        .apply_download_link(&id, good.token.as_ref().unwrap(), false)
        .await
        .unwrap();
    // Resource changes between Verify and Start must not invoke legacy 200 reset.
    f.source.mode.store(3, Ordering::SeqCst);
    engine.start(&id).await.unwrap();
    wait(&engine, &id, Status::Failed).await;
    assert_eq!(std::fs::read(&old.partial_path).unwrap(), corrupt);
    assert_eq!(
        engine.get(&id).await.unwrap().durable_bytes,
        old.durable_bytes
    );
    engine.shutdown().await.unwrap();
}
#[tokio::test]
async fn concurrent_lifecycle_commands_are_busy_and_shutdown_does_not_wait_for_network() {
    let f = fixture().await;
    let root = tempfile::tempdir().unwrap();
    let (engine, id) = seeded(root.path(), &f, false).await;
    f.source.mode.store(11, Ordering::SeqCst);
    let (other, key, replacement) = (engine.clone(), id.clone(), format!("{}/file", f.url));
    let task = tokio::spawn(async move { other.verify_download_link(&key, &replacement).await });
    f.source.entered.notified().await;
    assert!(engine.start(&id).await.is_err());
    assert!(engine.cancel(&id).await.is_err());
    assert!(
        engine
            .verify_download_link(&id, &format!("{}/file", f.url))
            .await
            .is_err()
    );
    tokio::time::timeout(Duration::from_secs(1), engine.shutdown())
        .await
        .unwrap()
        .unwrap();
    f.source.release.notify_one();
    assert!(task.await.unwrap().is_err());
}
#[tokio::test]
async fn confirmed_restart_uses_a_new_partial_and_cancelled_records_cannot_resume() {
    let f = fixture().await;
    let root = tempfile::tempdir().unwrap();
    let (engine, id) = seeded(root.path(), &f, false).await;
    let old = engine.get(&id).await.unwrap();
    f.source.mode.store(1, Ordering::SeqCst);
    let bad = engine
        .verify_download_link(&id, &format!("{}/file", f.url))
        .await
        .unwrap();
    engine
        .apply_download_link(&id, bad.token.as_ref().unwrap(), true)
        .await
        .unwrap();
    let fresh = engine.get(&id).await.unwrap();
    assert_eq!(fresh.id, id);
    assert_ne!(fresh.partial_path, old.partial_path);
    assert_eq!(fresh.durable_bytes, 0);
    assert!(!std::path::Path::new(&old.partial_path).exists());
    engine.cancel(&id).await.unwrap();
    f.source.mode.store(0, Ordering::SeqCst);
    let cancelled = engine
        .verify_download_link(&id, &format!("{}/file", f.url))
        .await
        .unwrap();
    assert_eq!(cancelled.outcome, RecoveryOutcome::CannotVerify);
    assert!(
        engine
            .apply_download_link(&id, cancelled.token.as_ref().unwrap(), false)
            .await
            .is_err()
    );
    engine
        .apply_download_link(&id, cancelled.token.as_ref().unwrap(), true)
        .await
        .unwrap();
    engine.start(&id).await.unwrap();
    wait(&engine, &id, Status::Completed).await;
    assert_eq!(std::fs::read(&old.destination).unwrap(), *f.source.data);
    engine.shutdown().await.unwrap();
}
#[tokio::test]
async fn persisted_pin_survives_reopen_and_stops_changed_resource() {
    let f = fixture().await;
    let root = tempfile::tempdir().unwrap();
    let (engine, id) = seeded(root.path(), &f, false).await;
    let old = engine.get(&id).await.unwrap();
    let verified = engine
        .verify_download_link(&id, &format!("{}/file", f.url))
        .await
        .unwrap();
    engine
        .apply_download_link(&id, verified.token.as_ref().unwrap(), false)
        .await
        .unwrap();
    engine.shutdown().await.unwrap();
    drop(engine);
    let engine = Engine::open(root.path().join("metadata")).await.unwrap();
    assert!(engine.get(&id).await.unwrap().recovery_pin.is_some());
    f.source.mode.store(8, Ordering::SeqCst); // probe passes; actual remainder changes ETag
    engine.start(&id).await.unwrap();
    wait(&engine, &id, Status::Failed).await;
    assert_eq!(
        engine.get(&id).await.unwrap().durable_bytes,
        old.durable_bytes
    );
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn missing_and_post_commit_corrupt_partials_cannot_resume() {
    let f = fixture().await;
    let root = tempfile::tempdir().unwrap();
    let (engine, id) = seeded(root.path(), &f, false).await;
    let old = engine.get(&id).await.unwrap();
    let good = engine
        .verify_download_link(&id, &format!("{}/file", f.url))
        .await
        .unwrap();
    engine
        .apply_download_link(&id, good.token.as_ref().unwrap(), false)
        .await
        .unwrap();
    let mut bytes = std::fs::read(&old.partial_path).unwrap();
    bytes[2] ^= 1;
    std::fs::write(&old.partial_path, &bytes).unwrap();
    engine.start(&id).await.unwrap();
    wait(&engine, &id, Status::Failed).await;
    assert_eq!(std::fs::read(&old.partial_path).unwrap(), bytes);
    std::fs::remove_file(&old.partial_path).unwrap();
    let current = engine.get(&id).await.unwrap();
    let missing = engine
        .verify_download_link(&id, &format!("{}/file", f.url))
        .await
        .unwrap();
    assert_eq!(missing.outcome, RecoveryOutcome::CannotVerify);
    assert_eq!(engine.get(&id).await.unwrap().url, current.url);
    engine.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "subprocess helper invoked by replacement_commit_survives_process_termination"]
async fn recovery_commit_child() {
    let root = std::path::PathBuf::from(std::env::var("DLM_RECOVERY_ROOT").unwrap());
    let id = std::env::var("DLM_RECOVERY_ID").unwrap();
    let url = std::env::var("DLM_RECOVERY_URL").unwrap();
    let engine = Engine::open(root.join("metadata")).await.unwrap();
    let verified = engine.verify_download_link(&id, &url).await.unwrap();
    assert_eq!(verified.outcome, RecoveryOutcome::SafeToResume);
    engine
        .apply_download_link(&id, verified.token.as_ref().unwrap(), false)
        .await
        .unwrap();
    std::fs::write(root.join("committed"), b"committed").unwrap();
    std::future::pending::<()>().await;
}
#[tokio::test]
async fn replacement_commit_survives_process_termination() {
    let f = fixture().await;
    let root = tempfile::tempdir().unwrap();
    let (engine, id) = seeded(root.path(), &f, false).await;
    let old = engine.get(&id).await.unwrap();
    engine.shutdown().await.unwrap();
    drop(engine);
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "recovery_commit_child", "--ignored"])
        .env("DLM_RECOVERY_ROOT", root.path())
        .env("DLM_RECOVERY_ID", &id)
        .env(
            "DLM_RECOVERY_URL",
            format!("{}/file?token=replacement-secret", f.url),
        )
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let reached = tokio::time::timeout(Duration::from_secs(15), async {
        while !root.path().join("committed").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    child.kill().unwrap();
    child.wait().unwrap();
    reached.unwrap();
    let engine = Engine::open(root.path().join("metadata")).await.unwrap();
    let recovered = engine.get(&id).await.unwrap();
    assert_eq!(recovered.durable_bytes, old.durable_bytes);
    assert!(recovered.recovery_pin.is_some());
    assert_eq!(recovered.status, Status::Paused);
    assert_eq!(
        recovered.url,
        format!("{}/file?token=replacement-secret", f.url)
    );
    f.source.mode.store(3, Ordering::SeqCst);
    engine.start(&id).await.unwrap();
    wait(&engine, &id, Status::Failed).await;
    assert_eq!(
        engine.get(&id).await.unwrap().durable_bytes,
        old.durable_bytes
    );
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn expired_original_link_suggests_recovery_without_claiming_expiration() {
    let f = fixture().await;
    let root = tempfile::tempdir().unwrap();
    let (engine, id) = seeded(root.path(), &f, false).await;
    let before = engine.get(&id).await.unwrap();
    engine.start(&id).await.unwrap();
    wait(&engine, &id, Status::Failed).await;
    let failed = engine.get(&id).await.unwrap();
    assert_eq!(failed.last_http_status, Some(404));
    assert_eq!(failed.durable_bytes, before.durable_bytes);
    let verified = engine
        .verify_download_link(&id, &format!("{}/file", f.url))
        .await
        .unwrap();
    assert_eq!(verified.outcome, RecoveryOutcome::SafeToResume);
    engine
        .apply_download_link(&id, verified.token.as_ref().unwrap(), false)
        .await
        .unwrap();
    engine.start(&id).await.unwrap();
    wait(&engine, &id, Status::Completed).await;
    assert_eq!(std::fs::read(&before.destination).unwrap(), *f.source.data);
    engine.shutdown().await.unwrap();
}
