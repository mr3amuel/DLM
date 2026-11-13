
#[tokio::test]
async fn admission_aging_survives_restart_and_resume() {
    let (dir, engine) = setup().await;
    let (url, server) = stalled_url().await;
    let blocker = stalled_item(&engine, &dir, &url, "aging-blocker").await;
    let low = stalled_item(&engine, &dir, &url, "aging-low").await;
    let high = stalled_item(&engine, &dir, &url, "aging-high").await;
    engine.set_priority(&low, Priority::Low).await.unwrap();
    engine.set_priority(&high, Priority::High).await.unwrap();
    engine.start(&blocker).await.unwrap();
    engine.start(&low).await.unwrap();
    engine.start(&high).await.unwrap();
    engine.cancel(&blocker).await.unwrap();
    wait_running(&engine, &high).await;
    assert_eq!(engine.get(&low).await.unwrap().admission_bypasses, 1);
    engine.shutdown().await.unwrap();
    drop(engine);
    let reopened = Engine::open(dir.path().join("metadata")).await.unwrap();
    assert_eq!(reopened.get(&low).await.unwrap().status, Status::Paused);
    assert_eq!(reopened.get(&low).await.unwrap().admission_bypasses, 1);
    reopened.start(&high).await.unwrap();
    reopened.resume(&low).await.unwrap();
    assert_eq!(reopened.get(&low).await.unwrap().admission_bypasses, 1);
    assert_eq!(reopened.get(&low).await.unwrap().status, Status::Queued);
    reopened.shutdown().await.unwrap();
    server.abort();
}
