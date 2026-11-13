use download_core::{Engine, Status};

#[tokio::test]
async fn cancelled_restart_and_metadata_removal_never_delete_destinations() {
    let dir = tempfile::tempdir().unwrap();
    let metadata = dir.path().join("metadata");
    let engine = Engine::open(&metadata).await.unwrap();
    let d = engine
        .create(
            "http://127.0.0.1:9/file".into(),
            dir.path().into(),
            "result.bin".into(),
        )
        .await
        .unwrap();
    assert!(engine.remove_cancelled(&d.id).await.is_err());
    tokio::fs::write(&d.partial_path, b"partial").await.unwrap();
    engine.pause(&d.id).await.unwrap();
    assert!(tokio::fs::try_exists(&d.partial_path).await.unwrap());
    engine.cancel(&d.id).await.unwrap();
    let cancelled = engine.get(&d.id).await.unwrap();
    assert_eq!(cancelled.status, Status::Cancelled);
    assert_eq!(cancelled.downloaded, 0);
    assert_eq!(cancelled.durable_bytes, 0);
    assert!(!tokio::fs::try_exists(&d.partial_path).await.unwrap());
    engine.restart_cancelled(&d.id).await.unwrap();
    assert_eq!(engine.get(&d.id).await.unwrap().status, Status::Ready);
    assert_eq!(tokio::fs::metadata(&d.partial_path).await.unwrap().len(), 0);
    assert!(engine.restart_cancelled(&d.id).await.is_err());
    engine.cancel(&d.id).await.unwrap();
    // Never delete even a newly appearing artifact as a side effect of removal.
    tokio::fs::write(&d.partial_path, b"unexpected")
        .await
        .unwrap();
    assert!(engine.remove_cancelled(&d.id).await.is_err());
    assert_eq!(
        tokio::fs::read(&d.partial_path).await.unwrap(),
        b"unexpected"
    );
    engine.cancel(&d.id).await.unwrap();
    tokio::fs::write(&d.destination, b"unrelated user file")
        .await
        .unwrap();
    assert!(engine.restart_cancelled(&d.id).await.is_err());
    engine.remove_cancelled(&d.id).await.unwrap();
    assert_eq!(
        tokio::fs::read(&d.destination).await.unwrap(),
        b"unrelated user file"
    );
    assert!(engine.get(&d.id).await.is_err());
    engine.shutdown().await.unwrap();
    drop(engine);
    let engine = Engine::open(&metadata).await.unwrap();
    assert!(engine.list().await.is_empty());
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn unavailable_destination_does_not_erase_cancelled_record_or_block_recovery() {
    let root = tempfile::tempdir().unwrap();
    let metadata = root.path().join("metadata");
    let destination = root.path().join("destination");
    tokio::fs::create_dir(&destination).await.unwrap();
    let engine = Engine::open(&metadata).await.unwrap();
    let d = engine
        .create(
            "http://127.0.0.1:9/file".into(),
            destination.clone(),
            "result.bin".into(),
        )
        .await
        .unwrap();
    engine.cancel(&d.id).await.unwrap();
    tokio::fs::remove_dir(&destination).await.unwrap();
    assert!(engine.restart_cancelled(&d.id).await.is_err());
    assert_eq!(engine.get(&d.id).await.unwrap().status, Status::Cancelled);
    engine.shutdown().await.unwrap();
    drop(engine);
    let engine = Engine::open(&metadata).await.unwrap();
    assert_eq!(engine.get(&d.id).await.unwrap().status, Status::Cancelled);
    assert_eq!(engine.get(&d.id).await.unwrap().downloaded, 0);
    engine.shutdown().await.unwrap();
}
