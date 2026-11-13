mod support;

use download_core::{Engine, Priority, QueueConfig, RetryPolicy, Status, TransferConfig};
use std::time::{Duration, Instant};
use support::{done, fixture, fixture_with_size, wait};

const MIB: u64 = 1024 * 1024;

async fn create(
    engine: &Engine,
    url: &str,
    path: &std::path::Path,
    name: &str,
    connections: u32,
) -> String {
    let d = engine
        .create(url.into(), path.into(), name.into())
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
    d.id
}

#[tokio::test]
async fn global_limit_is_aggregate_across_downloads_and_connections() {
    let fixture = fixture_with_size(4).await;
    let directory = tempfile::tempdir().unwrap();
    let engine = Engine::open(directory.path().join("metadata"))
        .await
        .unwrap();
    engine
        .set_global_bandwidth_limit(Some(4 * MIB))
        .await
        .unwrap();
    let a = create(&engine, &fixture.url, directory.path(), "a.bin", 4).await;
    let b = create(&engine, &fixture.url, directory.path(), "b.bin", 4).await;
    let start = Instant::now();
    engine.start(&a).await.unwrap();
    engine.start(&b).await.unwrap();
    let (a, b) = tokio::join!(
        done(&engine, &a, &fixture.state.data),
        done(&engine, &b, &fixture.state.data)
    );
    assert!(a.segmented && b.segmented);
    // Buckets start empty. 50 ms is observation/clock tolerance, not credit.
    assert!(
        start.elapsed() + Duration::from_millis(50)
            >= Duration::from_secs_f64((8 * MIB) as f64 / (4 * MIB) as f64)
    );
    assert_eq!(a.retries + b.retries, 0);
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn slow_local_cap_does_not_stall_unrelated_unlimited_download() {
    let fixture = fixture_with_size(1).await;
    let directory = tempfile::tempdir().unwrap();
    let engine = Engine::open(directory.path().join("metadata"))
        .await
        .unwrap();
    let slow = create(&engine, &fixture.url, directory.path(), "slow.bin", 1).await;
    let fast = create(&engine, &fixture.url, directory.path(), "fast.bin", 1).await;
    engine
        .set_download_bandwidth_limit(&slow, Some(1))
        .await
        .unwrap();
    engine.start(&slow).await.unwrap();
    engine.start(&fast).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(3),
        done(&engine, &fast, &fixture.state.data),
    )
    .await
    .expect("An independent unlimited download must not wait for a one-byte/s local cap");
    tokio::time::timeout(Duration::from_millis(500), engine.cancel(&slow))
        .await
        .expect("Cancellation must interrupt the limiter promptly")
        .unwrap();
    assert_eq!(engine.get(&slow).await.unwrap().status, Status::Cancelled);
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn runtime_hierarchy_changes_preserve_ranges_settings_and_queue_after_restart() {
    let fixture = fixture().await;
    let directory = tempfile::tempdir().unwrap();
    let metadata = directory.path().join("metadata");
    let engine = Engine::open(&metadata).await.unwrap();
    engine
        .configure_queue(QueueConfig {
            max_downloads: 1,
            ..QueueConfig::default()
        })
        .await
        .unwrap();
    engine
        .set_global_bandwidth_limit(Some(8 * MIB))
        .await
        .unwrap();
    let policy = RetryPolicy {
        max_attempts: 5,
        initial_delay_ms: 100,
        max_delay_ms: 2000,
        jitter_percent: 10,
    };
    engine.configure_retry_policy(policy.clone()).await.unwrap();
    let a = create(&engine, &fixture.url, directory.path(), "recovered.bin", 2).await;
    let b = create(&engine, &fixture.url, directory.path(), "queued.bin", 2).await;
    engine.set_priority(&b, Priority::Low).await.unwrap();
    engine.start(&a).await.unwrap();
    engine.start(&b).await.unwrap();
    assert_eq!(engine.get(&b).await.unwrap().status, Status::Queued);
    wait(&engine, &a, |d| {
        d.durable_bytes >= 2 * MIB && d.status == Status::Running
    })
    .await;
    // Change the cap while worker copies are live, then force final-state publication.
    engine
        .set_download_bandwidth_limit(&a, Some(MIB))
        .await
        .unwrap();
    engine
        .set_global_bandwidth_limit(Some(4 * MIB))
        .await
        .unwrap();
    engine.shutdown().await.unwrap();
    let saved = engine.get(&a).await.unwrap();
    assert_eq!(saved.status, Status::Paused);
    assert!(saved.durable_bytes > 0 && saved.durable_bytes < 16 * MIB);
    assert_eq!(saved.bandwidth_limit, Some(MIB));
    let queued = engine.get(&b).await.unwrap();
    drop(engine);
    fixture.state.requested.lock().unwrap().clear();
    let engine = Engine::open(&metadata).await.unwrap();
    assert_eq!(engine.global_bandwidth_limit().await, Some(4 * MIB));
    assert_eq!(engine.retry_policy().await, policy);
    let restored = engine.get(&a).await.unwrap();
    assert_eq!(restored.status, Status::Paused);
    assert_eq!(restored.bandwidth_limit, Some(MIB));
    assert_eq!(restored.completed_ranges, saved.completed_ranges);
    let restored_queue = engine.get(&b).await.unwrap();
    assert_eq!(restored_queue.status, Status::Paused);
    assert_eq!(restored_queue.priority, Priority::Low);
    assert_eq!(restored_queue.queue_order, queued.queue_order);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(fixture.state.requested.lock().unwrap().is_empty());
    engine.set_global_bandwidth_limit(None).await.unwrap();
    engine.set_download_bandwidth_limit(&a, None).await.unwrap();
    engine.resume(&a).await.unwrap();
    done(&engine, &a, &fixture.state.data).await;
    for &(start, end) in fixture
        .state
        .requested
        .lock()
        .unwrap()
        .iter()
        .filter(|&&(a, b)| (a, b) != (0, 1))
    {
        assert!(
            saved
                .completed_ranges
                .iter()
                .all(|r| end <= r.start || start >= r.end)
        );
    }
    engine.cancel(&b).await.unwrap();
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn per_download_cap_is_shared_by_all_range_workers() {
    let fixture = fixture_with_size(4).await;
    let directory = tempfile::tempdir().unwrap();
    let engine = Engine::open(directory.path().join("metadata"))
        .await
        .unwrap();
    engine
        .set_global_bandwidth_limit(Some(4 * MIB))
        .await
        .unwrap();
    let id = create(&engine, &fixture.url, directory.path(), "local.bin", 4).await;
    engine
        .set_download_bandwidth_limit(&id, Some(2 * MIB))
        .await
        .unwrap();
    let start = Instant::now();
    engine.start(&id).await.unwrap();
    let result = done(&engine, &id, &fixture.state.data).await;
    assert!(result.segmented);
    // Empty buckets and a shared local debit prohibit multiplied per-worker rates.
    assert!(start.elapsed() + Duration::from_millis(50) >= Duration::from_secs(2));
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn exhausted_retry_budget_preserves_server_cooldown_across_restart() {
    use std::sync::atomic::Ordering;
    let fixture = fixture_with_size(1).await;
    fixture.state.mode.store(6, Ordering::SeqCst);
    fixture.state.throttle_budget.store(1, Ordering::SeqCst);
    let directory = tempfile::tempdir().unwrap();
    let metadata = directory.path().join("metadata");
    let engine = Engine::open(&metadata).await.unwrap();
    engine
        .configure_retry_policy(RetryPolicy {
            max_attempts: 1,
            ..RetryPolicy::default()
        })
        .await
        .unwrap();
    let id = create(&engine, &fixture.url, directory.path(), "cooldown.bin", 1).await;
    engine.start(&id).await.unwrap();
    let failed = wait(&engine, &id, |d| d.status == Status::Failed).await;
    assert_eq!(failed.retries, 0);
    assert!(failed.retry_not_before_ms.is_some());
    assert_eq!(fixture.state.timestamps.lock().unwrap().len(), 1);
    engine.shutdown().await.unwrap();
    drop(engine);
    let engine = Engine::open(&metadata).await.unwrap();
    assert_eq!(engine.get(&id).await.unwrap().status, Status::Paused);
    engine.retry(&id).await.unwrap();
    let waiting = wait(&engine, &id, |d| d.retry_remaining_seconds.is_some()).await;
    assert_eq!(waiting.eta_seconds, None);
    assert_eq!(waiting.current_speed, 0.0);
    assert_eq!(fixture.state.timestamps.lock().unwrap().len(), 1);
    done(&engine, &id, &fixture.state.data).await;
    {
        let times = fixture.state.timestamps.lock().unwrap();
        assert!(times[1].duration_since(times[0]) >= Duration::from_secs(1));
    }
    engine.shutdown().await.unwrap();
}
