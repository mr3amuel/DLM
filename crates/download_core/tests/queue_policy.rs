use download_core::{Engine, Priority, QueueConfig, RetryPolicy, Status};
use std::time::Duration;
use tempfile::TempDir;
include!("support/queue_aging_restart.rs");

async fn setup() -> (TempDir, Engine) {
    let dir = TempDir::new().unwrap();
    let engine = Engine::open(dir.path().join("metadata")).await.unwrap();
    engine
        .configure_queue(QueueConfig {
            max_downloads: 1,
            global_connections: 64,
            per_host_connections: 32,
        })
        .await
        .unwrap();
    (dir, engine)
}
async fn item(engine: &Engine, dir: &TempDir, name: &str) -> String {
    engine
        .create(
            "http://127.0.0.1:9/file".into(),
            dir.path().into(),
            name.into(),
        )
        .await
        .unwrap()
        .id
}
async fn wait_running(engine: &Engine, id: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if engine.get(id).await.unwrap().status == Status::Running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
// Keep requests waiting on headers so admission assertions do not race completion.
async fn stalled_url() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/file", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut sockets = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            sockets.push(socket);
        }
    });
    (url, task)
}
async fn stalled_item(engine: &Engine, dir: &TempDir, url: &str, name: &str) -> String {
    engine
        .create(url.into(), dir.path().into(), name.into())
        .await
        .unwrap()
        .id
}
#[tokio::test]
async fn policy_settings_survive_restart_and_reject_zero() {
    let (dir, engine) = setup().await;
    let id = item(&engine, &dir, "policy.bin").await;
    engine.set_priority(&id, Priority::Low).await.unwrap();
    engine
        .set_global_bandwidth_limit(Some(100_000))
        .await
        .unwrap();
    engine
        .set_download_bandwidth_limit(&id, Some(50_000))
        .await
        .unwrap();
    let policy = RetryPolicy {
        max_attempts: 2,
        ..RetryPolicy::default()
    };
    engine.configure_retry_policy(policy.clone()).await.unwrap();
    assert!(engine.set_global_bandwidth_limit(Some(0)).await.is_err());
    assert!(
        engine
            .set_download_bandwidth_limit(&id, Some(0))
            .await
            .is_err()
    );
    engine.shutdown().await.unwrap();
    drop(engine);
    let reopened = Engine::open(dir.path().join("metadata")).await.unwrap();
    assert_eq!(reopened.global_bandwidth_limit().await, Some(100_000));
    assert_eq!(reopened.retry_policy().await, policy);
    let d = reopened.get(&id).await.unwrap();
    assert_eq!(d.bandwidth_limit, Some(50_000));
    assert_eq!(d.priority, Priority::Low);
    assert_eq!(d.status, Status::Paused);
    assert_eq!(d.current_speed, 0.0);
    assert_eq!(d.eta_seconds, None);
    reopened.shutdown().await.unwrap();
}
#[tokio::test]
async fn priority_reorder_is_atomic_and_never_preempts_active() {
    let (dir, engine) = setup().await;
    let (url, server) = stalled_url().await;
    let active = stalled_item(&engine, &dir, &url, "active").await;
    engine.start(&active).await.unwrap();
    let a = stalled_item(&engine, &dir, &url, "a").await;
    let b = stalled_item(&engine, &dir, &url, "b").await;
    let low = stalled_item(&engine, &dir, &url, "low").await;
    engine.set_priority(&low, Priority::Low).await.unwrap();
    engine.start(&a).await.unwrap();
    engine.start(&b).await.unwrap();
    engine.start(&low).await.unwrap();
    engine
        .reorder_queue(vec![b.clone(), a.clone(), low.clone()])
        .await
        .unwrap();
    assert_eq!(engine.get(&b).await.unwrap().queue_position, Some(1));
    let order = engine.get(&a).await.unwrap().queue_order;
    assert!(
        engine
            .reorder_queue(vec![a.clone(), a.clone(), low.clone()])
            .await
            .is_err()
    );
    assert_eq!(engine.get(&a).await.unwrap().queue_order, order);
    engine.set_priority(&a, Priority::High).await.unwrap();
    assert_eq!(engine.get(&a).await.unwrap().queue_position, Some(1));
    assert_eq!(engine.get(&active).await.unwrap().status, Status::Running);
    engine.cancel(&active).await.unwrap();
    wait_running(&engine, &a).await;
    assert_eq!(engine.get(&b).await.unwrap().status, Status::Queued);
    engine.shutdown().await.unwrap();
    server.abort();
}
#[tokio::test]
async fn eight_admissions_bound_priority_starvation() {
    let (dir, engine) = setup().await;
    let (url, server) = stalled_url().await;
    let mut active = stalled_item(&engine, &dir, &url, "blocker").await;
    engine.start(&active).await.unwrap();
    let low = stalled_item(&engine, &dir, &url, "waiting-low").await;
    engine.set_priority(&low, Priority::Low).await.unwrap();
    engine.start(&low).await.unwrap();
    for n in 0..8 {
        let high = stalled_item(&engine, &dir, &url, &format!("high-{n}")).await;
        engine.set_priority(&high, Priority::High).await.unwrap();
        engine.start(&high).await.unwrap();
        engine.cancel(&active).await.unwrap();
        wait_running(&engine, &high).await;
        assert_eq!(engine.get(&low).await.unwrap().admission_bypasses, n + 1);
        active = high;
    }
    let newest = stalled_item(&engine, &dir, &url, "newest-high").await;
    engine.set_priority(&newest, Priority::High).await.unwrap();
    engine.start(&newest).await.unwrap();
    engine.cancel(&active).await.unwrap();
    wait_running(&engine, &low).await;
    assert_eq!(engine.get(&newest).await.unwrap().status, Status::Queued);
    engine.shutdown().await.unwrap();
    server.abort();
}
#[tokio::test]
async fn schema_v2_migration_preserves_records_and_settings() {
    let (dir, engine) = setup().await;
    let id = item(&engine, &dir, "migration.bin").await;
    engine.shutdown().await.unwrap();
    drop(engine);
    let db = dir.path().join("metadata/downloads.sqlite3");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(
        "DROP TABLE organization_state; DROP TABLE file_moves; DROP TABLE automation_state; DROP TABLE policy_settings; PRAGMA user_version=2;",
    )
    .unwrap();
    let json: String = conn
        .query_row("SELECT metadata FROM downloads WHERE id=?1", [&id], |r| {
            r.get(0)
        })
        .unwrap();
    let mut json: serde_json::Value = serde_json::from_str(&json).unwrap();
    let object = json.as_object_mut().unwrap();
    // Genuine v2 representation with a durable segmented prefix and custom order.
    std::fs::write(object["partial_path"].as_str().unwrap(), b"data").unwrap();
    object.insert("durable_bytes".into(), serde_json::json!(4));
    object.insert("downloaded".into(), serde_json::json!(8));
    object.insert("total".into(), serde_json::json!(8));
    object.insert("segmented".into(), serde_json::json!(true));
    object.insert(
        "completed_ranges".into(),
        serde_json::json!([{"start":0,"end":4}]),
    );
    object.insert("etag".into(), serde_json::json!("\"migration-v2\""));
    object.insert("queue_order".into(), serde_json::json!(42));
    object.insert(
        "config".into(),
        serde_json::json!({"max_connections":8,"optimize":false}),
    );
    for key in [
        "priority",
        "bandwidth_limit",
        "admission_bypasses",
        "current_speed",
        "smoothed_speed",
        "eta_seconds",
        "retry_remaining_seconds",
        "error_kind",
        "bandwidth_limited",
    ] {
        object.remove(key);
    }
    conn.execute(
        "UPDATE downloads SET metadata=?1 WHERE id=?2",
        rusqlite::params![json.to_string(), id],
    )
    .unwrap();
    drop(conn);
    let reopened = Engine::open(dir.path().join("metadata")).await.unwrap();
    let d = reopened.get(&id).await.unwrap();
    assert_eq!(d.priority, Priority::Normal);
    assert_eq!(d.filename, "migration.bin");
    assert_eq!(d.durable_bytes, 4);
    assert_eq!(d.downloaded, 4);
    assert_eq!(
        d.completed_ranges,
        vec![download_core::ByteRange { start: 0, end: 4 }]
    );
    assert_eq!(d.etag.as_deref(), Some("\"migration-v2\""));
    assert_eq!(d.queue_order, 42);
    assert_eq!(d.config.max_connections, 8);
    assert!(!d.config.optimize);
    assert_eq!(reopened.queue_config().await.max_downloads, 1);
    reopened.shutdown().await.unwrap();
    drop(reopened);
    let conn = rusqlite::Connection::open(db).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        5
    );
}

#[tokio::test]
async fn batch_resume_admits_high_first_for_every_input_order() {
    use download_core::BatchAction;
    // Fresh paused records have no starvation promotion. Include two- and
    // three-record batches and every input permutation, independent of UUID order.
    for order in [
        vec![1, 0],
        vec![0, 1],
        vec![0, 1, 2],
        vec![0, 2, 1],
        vec![1, 0, 2],
        vec![1, 2, 0],
        vec![2, 0, 1],
        vec![2, 1, 0],
    ] {
        let (dir, engine) = setup().await;
        let (url, server) = stalled_url().await;
        let mut ids = Vec::new();
        for (i, priority) in [Priority::High, Priority::Normal, Priority::Low]
            .into_iter()
            .enumerate()
        {
            let id = stalled_item(&engine, &dir, &url, &format!("batch-{i}")).await;
            engine.set_priority(&id, priority).await.unwrap();
            engine.pause(&id).await.unwrap();
            assert_eq!(engine.get(&id).await.unwrap().status, Status::Paused);
            ids.push(id);
        }
        let results = engine
            .batch(
                order.iter().map(|i| ids[*i].clone()).collect(),
                BatchAction::Resume,
                None,
                false,
            )
            .await
            .unwrap();
        assert_eq!(results.len(), order.len());
        assert!(results.iter().all(|r| r.error.is_none()));
        assert_eq!(engine.get(&ids[0]).await.unwrap().status, Status::Running);
        assert_eq!(engine.get(&ids[1]).await.unwrap().status, Status::Queued);
        if order.len() == 3 {
            assert_eq!(engine.get(&ids[2]).await.unwrap().status, Status::Queued);
            engine.cancel(&ids[0]).await.unwrap();
            wait_running(&engine, &ids[1]).await;
            assert_eq!(engine.get(&ids[2]).await.unwrap().status, Status::Queued);
        }
        engine.shutdown().await.unwrap();
        server.abort();
    }
}

#[tokio::test]
async fn batch_resume_preserves_running_work_and_reports_each_failure() {
    use download_core::BatchAction;
    let (dir, engine) = setup().await;
    let (url, server) = stalled_url().await;
    let running = stalled_item(&engine, &dir, &url, "already-running").await;
    engine.start(&running).await.unwrap();
    let high = stalled_item(&engine, &dir, &url, "batch-high").await;
    engine.set_priority(&high, Priority::High).await.unwrap();
    engine.pause(&high).await.unwrap();
    let results = engine
        .batch(
            vec![
                "missing-id".into(),
                running.clone(),
                high.clone(),
                high.clone(),
            ],
            BatchAction::Resume,
            None,
            false,
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 3); // Existing de-duplication contract.
    assert!(results[0].error.is_some() && results[1].error.is_some());
    assert!(results[2].error.is_none());
    assert_eq!(engine.get(&running).await.unwrap().status, Status::Running);
    assert_eq!(engine.get(&high).await.unwrap().status, Status::Queued);
    engine.cancel(&running).await.unwrap();
    wait_running(&engine, &high).await;
    engine.shutdown().await.unwrap();
    server.abort();
}
