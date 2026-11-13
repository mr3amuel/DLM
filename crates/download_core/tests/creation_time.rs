use download_core::{Engine, Priority};

#[tokio::test]
async fn creation_time_survives_queue_changes_and_restart_without_inventing_legacy_dates() {
    let dir = tempfile::tempdir().unwrap();
    let metadata = dir.path().join("metadata");
    let engine = Engine::open(&metadata).await.unwrap();
    let d = engine
        .create(
            "http://127.0.0.1:9/file".into(),
            dir.path().into(),
            "created.bin".into(),
        )
        .await
        .unwrap();
    let created = d.created_at_ms.unwrap();
    assert!(created > 0);
    engine.set_priority(&d.id, Priority::High).await.unwrap();
    engine.start(&d.id).await.unwrap();
    engine.pause(&d.id).await.unwrap();
    assert_eq!(
        engine.get(&d.id).await.unwrap().created_at_ms,
        Some(created)
    );
    engine.shutdown().await.unwrap();
    drop(engine);
    let engine = Engine::open(&metadata).await.unwrap();
    assert_eq!(
        engine.get(&d.id).await.unwrap().created_at_ms,
        Some(created)
    );
    engine.shutdown().await.unwrap();
    drop(engine);
    // Only this disposable test row is made legacy-shaped.
    let connection = rusqlite::Connection::open(metadata.join("downloads.sqlite3")).unwrap();
    let json: String = connection
        .query_row("SELECT metadata FROM downloads WHERE id=?1", [&d.id], |r| {
            r.get(0)
        })
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value.as_object_mut().unwrap().remove("created_at_ms");
    connection
        .execute(
            "UPDATE downloads SET metadata=?1 WHERE id=?2",
            rusqlite::params![value.to_string(), d.id],
        )
        .unwrap();
    drop(connection);
    let engine = Engine::open(&metadata).await.unwrap();
    assert_eq!(engine.get(&d.id).await.unwrap().created_at_ms, None);
    engine.shutdown().await.unwrap();
}
