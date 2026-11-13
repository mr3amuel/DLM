mod support;
use download_core::{
    Clock, Engine, PauseOrigin, ResumeCapability, ScheduleKind, ScheduleRule, Status,
    TransferConfig,
};
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};
struct TestClock(AtomicI64);
impl Clock for TestClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
fn time(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s)
        .unwrap()
        .timestamp_millis()
}
fn clock(s: &str) -> Arc<TestClock> {
    Arc::new(TestClock(AtomicI64::new(time(s))))
}
fn recurring(kind: ScheduleKind, id: Option<String>, start: u16, end: u16) -> ScheduleRule {
    ScheduleRule {
        id: String::new(),
        name: "Fixture rule".into(),
        enabled: true,
        kind,
        download_id: id,
        at_ms: None,
        weekdays: 127,
        start_minute: start,
        end_minute: end,
        timezone: "UTC".into(),
        bandwidth_limit: None,
        last_occurrence: None,
        last_local_day: None,
        revision: 0,
        note: None,
    }
}
async fn stalled() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/source", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut sockets = vec![];
        while let Ok((s, _)) = listener.accept().await {
            sockets.push(s);
        }
    });
    (url, task)
}
async fn create(e: &Engine, root: &std::path::Path, url: &str) -> String {
    e.create(url.into(), root.into(), "scheduled.bin".into())
        .await
        .unwrap()
        .id
}
#[test]
fn overnight_weekdays_and_dst_are_explicit_and_deterministic() {
    let mut r = recurring(ScheduleKind::Bandwidth, None, 23 * 60, 2 * 60);
    r.weekdays = 1; // Monday
    assert!(r.window(time("2026-11-02T23:00:00Z")).is_some());
    assert!(r.window(time("2026-11-03T01:59:59Z")).is_some());
    assert!(r.window(time("2026-11-03T02:00:00Z")).is_none());
    r.timezone = "America/New_York".into();
    r.weekdays = 64;
    r.start_minute = 150;
    r.end_minute = 210;
    assert!(r.window(time("2026-03-08T07:10:00Z")).is_none()); // nonexistent 02:30 skips occurrence
    r.start_minute = 90;
    r.end_minute = 150;
    let window = r.window(time("2026-11-01T06:45:00Z")).unwrap();
    assert_eq!(
        window,
        (time("2026-11-01T05:30:00Z"), time("2026-11-01T07:30:00Z"))
    );
    assert_eq!(r.window(time("2026-11-01T05:45:00Z")), Some(window));
    r.last_occurrence = Some(window.0);
    assert!(r.next(time("2026-11-01T06:30:00Z")).unwrap() > window.1);
    r.timezone = "Asia/Tehran".into();
    r.weekdays = 127;
    r.start_minute = 9 * 60;
    r.end_minute = 10 * 60;
    assert!(r.window(time("2026-11-02T05:30:00Z")).is_some()); // pinned timezone, OS independent
}
#[tokio::test]
async fn overlapping_bandwidth_restores_manual_and_manual_override_survives_restart() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T08:00:00Z");
    let e = Engine::open_with_clock(root.path(), c.clone())
        .await
        .unwrap();
    e.set_global_bandwidth_limit(Some(5_000_000)).await.unwrap();
    let mut day = recurring(ScheduleKind::Bandwidth, None, 9 * 60, 17 * 60);
    day.bandwidth_limit = Some(2_000_000);
    let id = e.save_schedule(day).await.unwrap();
    let unlimited = e
        .save_schedule(recurring(ScheduleKind::Bandwidth, None, 0, 23 * 60 + 59))
        .await
        .unwrap();
    assert_eq!(e.scheduler_view().await.effective_limit, None);
    c.0.store(time("2026-11-02T09:00:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    assert_eq!(e.scheduler_view().await.effective_limit, Some(2_000_000));
    e.set_global_bandwidth_limit(Some(3_000_000)).await.unwrap();
    assert_eq!(e.scheduler_view().await.effective_limit, Some(3_000_000));
    e.shutdown().await.unwrap();
    drop(e);
    let e = Engine::open_with_clock(root.path(), c.clone())
        .await
        .unwrap();
    assert_eq!(e.scheduler_view().await.effective_limit, Some(3_000_000));
    c.0.store(time("2026-11-02T17:00:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    assert_eq!(e.scheduler_view().await.effective_limit, None);
    c.0.store(time("2026-11-02T09:00:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    assert_eq!(e.scheduler_view().await.effective_limit, Some(2_000_000)); // expired manual override cannot resurrect on rewind
    c.0.store(time("2026-11-02T17:00:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    e.delete_schedule(&unlimited).await.unwrap();
    assert_eq!(e.scheduler_view().await.effective_limit, Some(3_000_000));
    assert_eq!(e.global_bandwidth_limit().await, Some(3_000_000));
    e.delete_schedule(&id).await.unwrap();
    e.shutdown().await.unwrap();
}
#[tokio::test]
async fn manual_pause_survives_restart_and_enable_but_explicit_edit_schedules_again() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T08:00:00Z");
    let (url, server) = stalled().await;
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    let id = create(&e, root.path(), &url).await;
    let rule = e
        .save_schedule(recurring(
            ScheduleKind::Download,
            Some(id.clone()),
            9 * 60,
            10 * 60,
        ))
        .await
        .unwrap();
    e.pause(&id).await.unwrap();
    assert_eq!(
        e.get(&id).await.unwrap().pause_origin,
        Some(PauseOrigin::User)
    );
    e.shutdown().await.unwrap();
    drop(e);
    c.0.store(time("2026-11-02T09:00:00Z"), Ordering::SeqCst);
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    e.set_schedule_enabled(&rule, false).await.unwrap();
    e.set_schedule_enabled(&rule, true).await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    let mut spec = e.scheduler_view().await.rules[0].rule.clone();
    spec.name = "Edited".into();
    e.save_schedule(spec).await.unwrap();
    support::wait(&e, &id, |d| d.status == Status::Running).await;
    c.0.store(time("2026-11-02T11:00:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Running); // Unknown: do not pause
    assert!(
        e.scheduler_view().await.rules[0]
            .status
            .contains("cannot safely pause")
    );
    e.shutdown().await.unwrap();
    server.abort();
}
#[tokio::test]
async fn batch_resume_preserves_manual_start_and_later_pause_precedence() {
    use download_core::BatchAction;
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T08:00:00Z");
    let (url, server) = stalled().await;
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    let weekly = create(&e, root.path(), &url).await;
    let once = e
        .create(url, root.path().into(), "once.bin".into())
        .await
        .unwrap()
        .id;
    e.save_schedule(recurring(
        ScheduleKind::Download,
        Some(weekly.clone()),
        9 * 60,
        10 * 60,
    ))
    .await
    .unwrap();
    let mut rule = recurring(ScheduleKind::Download, Some(once.clone()), 0, 1);
    rule.at_ms = Some(time("2026-11-02T09:00:00Z"));
    e.save_schedule(rule).await.unwrap();
    e.pause(&weekly).await.unwrap();
    e.pause(&once).await.unwrap();
    // Resume All is an explicit manual start, using the same schedule policy
    // as individual Resume: consume pending once, hold recurring automation.
    let result = e
        .batch(
            vec![weekly.clone(), once.clone()],
            BatchAction::Resume,
            None,
            false,
        )
        .await
        .unwrap();
    assert!(result.iter().all(|r| r.error.is_none()));
    assert_eq!(e.get(&weekly).await.unwrap().status, Status::Running);
    assert_eq!(e.get(&once).await.unwrap().status, Status::Running);
    let view = e.scheduler_view().await;
    assert_eq!(view.rules.len(), 1);
    assert_eq!(view.rules[0].rule.download_id.as_ref(), Some(&weekly));
    assert!(view.rules[0].held);
    for id in [&weekly, &once] {
        e.pause(id).await.unwrap();
    }
    c.0.store(time("2026-11-02T09:00:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    for id in [&weekly, &once] {
        let d = e.get(id).await.unwrap();
        assert_eq!(d.status, Status::Paused);
        assert_eq!(d.pause_origin, Some(PauseOrigin::User));
    }
    e.shutdown().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn one_time_missed_policy_and_clock_rewinds_do_not_duplicate_claims() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T09:05:00Z");
    let (url, server) = stalled().await;
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    let id = create(&e, root.path(), &url).await;
    let mut once = recurring(ScheduleKind::Download, Some(id.clone()), 0, 1);
    once.at_ms = Some(time("2026-11-02T09:00:00Z"));
    let schedule = e.save_schedule(once).await.unwrap();
    let claimed = e.scheduler_view().await.rules[0].rule.last_occurrence;
    assert_eq!(claimed, Some(time("2026-11-02T09:00:00Z")));
    let order = e.get(&id).await.unwrap().queue_order;
    for t in [
        "2026-11-02T08:59:00Z",
        "2026-11-02T09:05:00Z",
        "2026-11-02T09:05:00Z",
    ] {
        c.0.store(time(t), Ordering::SeqCst);
        e.tick_automation().await.unwrap();
    }
    assert_eq!(e.get(&id).await.unwrap().queue_order, order);
    assert_eq!(
        e.scheduler_view().await.rules[0].rule.last_occurrence,
        claimed
    );
    e.delete_schedule(&schedule).await.unwrap();
    e.pause(&id).await.unwrap();
    let other = e
        .create(url.clone(), root.path().into(), "missed.bin".into())
        .await
        .unwrap();
    let mut missed = recurring(ScheduleKind::Download, Some(other.id.clone()), 0, 1);
    missed.at_ms = Some(time("2026-11-02T08:00:00Z"));
    e.save_schedule(missed).await.unwrap();
    assert_eq!(e.get(&other.id).await.unwrap().status, Status::Ready);
    assert!(
        e.scheduler_view()
            .await
            .rules
            .iter()
            .any(|v| v.status.contains("Missed"))
    );
    e.shutdown().await.unwrap();
    server.abort();
}
#[tokio::test]
async fn supported_window_pause_and_strict_single_resume_preserve_checkpoint() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T09:00:00Z");
    let f = support::fixture_with_size(8).await;
    f.state.mode.store(16, Ordering::SeqCst);
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    let id = create(&e, root.path(), &f.url).await;
    e.configure_download(
        &id,
        TransferConfig {
            max_connections: 1,
            optimize: false,
        },
    )
    .await
    .unwrap();
    e.start(&id).await.unwrap();
    support::wait(&e, &id, |d| d.durable_bytes > 0).await;
    e.pause(&id).await.unwrap();
    let _schedule = e
        .save_schedule(recurring(
            ScheduleKind::Download,
            Some(id.clone()),
            9 * 60,
            9 * 60 + 1,
        ))
        .await
        .unwrap();
    support::wait(&e, &id, |d| {
        d.resume.capability == ResumeCapability::Supported && d.status == Status::Running
    })
    .await;
    c.0.store(time("2026-11-02T09:02:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    let paused = e.get(&id).await.unwrap();
    assert_eq!(paused.status, Status::Paused);
    assert_eq!(paused.pause_origin, Some(PauseOrigin::Scheduler));
    let bytes = std::fs::read(&paused.partial_path).unwrap();
    let durable = paused.durable_bytes;
    assert!(durable > 0);
    f.state.mode.store(1, Ordering::SeqCst);
    c.0.store(time("2026-11-03T09:00:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    support::wait(&e, &id, |d| d.status == Status::Failed).await;
    assert_eq!(e.get(&id).await.unwrap().durable_bytes, durable);
    assert_eq!(std::fs::read(&paused.partial_path).unwrap(), bytes);
    e.shutdown().await.unwrap();
}
#[tokio::test]
async fn owned_restart_continues_same_occurrence_without_losing_manual_precedence() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T09:00:00Z");
    let (url, server) = stalled().await;
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    let id = create(&e, root.path(), &url).await;
    let rule = e
        .save_schedule(recurring(
            ScheduleKind::Download,
            Some(id.clone()),
            9 * 60,
            10 * 60,
        ))
        .await
        .unwrap();
    e.shutdown().await.unwrap();
    drop(e);
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    e.tick_automation().await.unwrap();
    support::wait(&e, &id, |d| d.status == Status::Running).await;
    assert_eq!(
        e.scheduler_view().await.rules[0].rule.last_occurrence,
        Some(time("2026-11-02T09:00:00Z"))
    );
    let (a, b) = tokio::join!(e.pause(&id), e.tick_automation());
    a.unwrap();
    b.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    e.delete_schedule(&rule).await.unwrap();
    e.shutdown().await.unwrap();
    server.abort();
}
#[tokio::test]
async fn schema_v3_migrates_forward_and_schedule_json_survives() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("downloads.sqlite3");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("CREATE TABLE downloads(id TEXT PRIMARY KEY, metadata TEXT NOT NULL);CREATE TABLE engine_settings(id INTEGER PRIMARY KEY,config TEXT NOT NULL);CREATE TABLE policy_settings(id INTEGER PRIMARY KEY,config TEXT NOT NULL);PRAGMA user_version=3;").unwrap();
    drop(conn);
    let c = clock("2026-11-02T08:00:00Z");
    let e = Engine::open_with_clock(root.path(), c.clone())
        .await
        .unwrap();
    let mut r = recurring(ScheduleKind::Bandwidth, None, 9 * 60, 17 * 60);
    r.bandwidth_limit = Some(2_000_000);
    let id = e.save_schedule(r).await.unwrap();
    e.shutdown().await.unwrap();
    drop(e);
    let e = Engine::open_with_clock(root.path(), c).await.unwrap();
    assert_eq!(e.scheduler_view().await.rules[0].rule.id, id);
    e.shutdown().await.unwrap();
    let conn = rusqlite::Connection::open(db).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        5
    );
}

#[tokio::test]
async fn scheduled_completion_keeps_exact_bytes_and_never_redownloads_completed_files() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T09:00:00Z");
    let f = support::fixture().await;
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    let id = create(&e, root.path(), &f.url).await;
    e.save_schedule(recurring(
        ScheduleKind::Download,
        Some(id.clone()),
        9 * 60,
        10 * 60,
    ))
    .await
    .unwrap();
    support::done(&e, &id, &f.state.data).await;
    let requests = f.state.requested.lock().unwrap().len();
    c.0.store(time("2026-11-03T09:00:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Completed);
    assert_eq!(f.state.requested.lock().unwrap().len(), requests);
    assert_eq!(e.list().await.len(), 1);
    e.shutdown().await.unwrap();
}
#[tokio::test]
async fn scheduled_admissions_use_existing_priority_slots_and_window_gate() {
    use download_core::{Priority, QueueConfig};
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T08:00:00Z");
    let (url, server) = stalled().await;
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    e.configure_queue(QueueConfig {
        max_downloads: 1,
        global_connections: 64,
        per_host_connections: 32,
    })
    .await
    .unwrap();
    let blocker = create(&e, root.path(), &url).await;
    e.start(&blocker).await.unwrap();
    let low = e
        .create(url.clone(), root.path().into(), "low.bin".into())
        .await
        .unwrap()
        .id;
    let high = e
        .create(url.clone(), root.path().into(), "high.bin".into())
        .await
        .unwrap()
        .id;
    e.set_priority(&low, Priority::Low).await.unwrap();
    e.set_priority(&high, Priority::High).await.unwrap();
    for id in [&low, &high] {
        e.save_schedule(recurring(
            ScheduleKind::Download,
            Some(id.clone()),
            9 * 60,
            10 * 60,
        ))
        .await
        .unwrap();
    }
    c.0.store(time("2026-11-02T09:00:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&low).await.unwrap().status, Status::Queued);
    assert_eq!(e.get(&high).await.unwrap().status, Status::Queued);
    e.pause(&blocker).await.unwrap();
    support::wait(&e, &high, |d| d.status == Status::Running).await;
    assert_eq!(e.get(&low).await.unwrap().status, Status::Queued);
    c.0.store(time("2026-11-02T11:00:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&low).await.unwrap().status, Status::Paused);
    assert_eq!(
        e.get(&low).await.unwrap().pause_origin,
        Some(PauseOrigin::Scheduler)
    );
    assert_eq!(e.get(&high).await.unwrap().status, Status::Running);
    e.shutdown().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn claim_transaction_failure_never_consumes_occurrence_or_duplicates_schedule() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T09:00:00Z");
    let (url, server) = stalled().await;
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    let id = create(&e, root.path(), &url).await;
    let conn = rusqlite::Connection::open(root.path().join("meta/downloads.sqlite3")).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_claim BEFORE UPDATE ON downloads WHEN json_extract(NEW.metadata,'$.status')='Queued' BEGIN SELECT RAISE(ABORT,'fixture queue claim'); END;").unwrap();
    let schedule = e
        .save_schedule(recurring(
            ScheduleKind::Download,
            Some(id.clone()),
            9 * 60,
            10 * 60,
        ))
        .await
        .unwrap();
    let view = e.scheduler_view().await;
    assert_eq!(view.rules.len(), 1);
    assert!(view.error.is_some());
    assert_eq!(view.rules[0].rule.last_occurrence, None);
    assert_eq!(e.get(&id).await.unwrap().status, Status::Ready);
    conn.execute_batch("DROP TRIGGER fail_claim;").unwrap();
    drop(conn);
    e.tick_automation().await.unwrap();
    assert_eq!(e.scheduler_view().await.rules[0].rule.id, schedule);
    assert_eq!(
        e.scheduler_view().await.rules[0].rule.last_occurrence,
        Some(time("2026-11-02T09:00:00Z"))
    );
    assert!(e.scheduler_view().await.error.is_none());
    e.shutdown().await.unwrap();
    server.abort();
}
#[tokio::test]
async fn migration_collision_rolls_back_without_overwriting_existing_tables() {
    let root = tempfile::tempdir().unwrap();
    let conn = rusqlite::Connection::open(root.path().join("downloads.sqlite3")).unwrap();
    conn.execute_batch("CREATE TABLE downloads(id TEXT PRIMARY KEY,metadata TEXT NOT NULL); CREATE TABLE engine_settings(id INTEGER PRIMARY KEY,config TEXT NOT NULL); CREATE TABLE policy_settings(id INTEGER PRIMARY KEY,config TEXT NOT NULL); CREATE TABLE automation_state(valuable TEXT); INSERT INTO automation_state VALUES('preserve'); PRAGMA user_version=3;").unwrap();
    assert!(Engine::open(root.path()).await.is_err());
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        conn.query_row("SELECT valuable FROM automation_state", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "preserve"
    );
}

#[tokio::test]
async fn manual_start_can_override_a_closed_window_until_explicit_rearm() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T08:00:00Z");
    let (url, server) = stalled().await;
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    let id = create(&e, root.path(), &url).await;
    e.save_schedule(recurring(
        ScheduleKind::Download,
        Some(id.clone()),
        9 * 60,
        10 * 60,
    ))
    .await
    .unwrap();
    e.start(&id).await.unwrap();
    support::wait(&e, &id, |d| d.status == Status::Running).await;
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Running);
    assert!(e.scheduler_view().await.rules[0].held);
    e.shutdown().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn unsupported_resume_is_not_automatically_paused_at_window_end() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T09:00:00Z");
    let f = support::fixture_with_size(16).await;
    f.state.mode.store(20, Ordering::SeqCst);
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    let id = create(&e, root.path(), &f.url).await;
    e.save_schedule(recurring(
        ScheduleKind::Download,
        Some(id.clone()),
        9 * 60,
        10 * 60,
    ))
    .await
    .unwrap();
    support::wait(&e, &id, |d| {
        d.status == Status::Running && d.resume.capability == ResumeCapability::Unsupported
    })
    .await;
    c.0.store(time("2026-11-02T11:00:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Running);
    assert!(
        e.scheduler_view().await.rules[0]
            .status
            .contains("cannot safely pause")
    );
    e.shutdown().await.unwrap();
}

#[tokio::test]
async fn restart_after_a_closed_recurring_window_waits_for_the_next_window() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T09:00:00Z");
    let (url, server) = stalled().await;
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    let id = create(&e, root.path(), &url).await;
    e.save_schedule(recurring(
        ScheduleKind::Download,
        Some(id.clone()),
        9 * 60,
        10 * 60,
    ))
    .await
    .unwrap();
    e.shutdown().await.unwrap();
    drop(e);
    c.0.store(time("2026-11-02T11:00:00Z"), Ordering::SeqCst);
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    assert!(
        e.scheduler_view().await.rules[0]
            .status
            .contains("Window closed")
    );
    c.0.store(time("2026-11-03T09:00:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Running);
    assert_eq!(e.list().await.len(), 1);
    e.shutdown().await.unwrap();
    server.abort();
}

#[tokio::test]
#[ignore = "subprocess helper invoked by schedule_claim_survives_process_kill"]
async fn schedule_commit_child() {
    let root = std::path::PathBuf::from(std::env::var("DLM_AUTOMATION_ROOT").unwrap());
    let id = std::env::var("DLM_AUTOMATION_ID").unwrap();
    let e = Engine::open_with_clock(root.join("meta"), clock("2026-11-02T09:00:00Z"))
        .await
        .unwrap();
    let rule = e
        .save_schedule(recurring(ScheduleKind::Download, Some(id), 9 * 60, 10 * 60))
        .await
        .unwrap();
    std::fs::write(root.join("schedule-committed"), rule).unwrap();
    std::future::pending::<()>().await;
}
#[tokio::test]
async fn schedule_claim_survives_process_kill() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T09:00:00Z");
    let (url, server) = stalled().await;
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    let id = create(&e, root.path(), &url).await;
    e.shutdown().await.unwrap();
    drop(e);
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "schedule_commit_child"])
        .env("DLM_AUTOMATION_ROOT", root.path())
        .env("DLM_AUTOMATION_ID", &id)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let committed = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while !root.path().join("schedule-committed").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    child.kill().unwrap();
    child.wait().unwrap();
    committed.unwrap();
    let e = Engine::open_with_clock(root.path().join("meta"), c)
        .await
        .unwrap();
    let claimed = e.scheduler_view().await.rules[0].rule.last_occurrence;
    e.tick_automation().await.unwrap();
    assert_eq!(e.list().await.len(), 1);
    assert_eq!(claimed, Some(time("2026-11-02T09:00:00Z")));
    assert_eq!(
        e.scheduler_view().await.rules[0].rule.last_occurrence,
        claimed
    );
    assert_eq!(e.get(&id).await.unwrap().status, Status::Running);
    e.pause(&id).await.unwrap();
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    e.shutdown().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn editing_a_consumed_one_time_schedule_has_a_new_upcoming_time_and_missed_policy() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T09:00:00Z");
    let (url, server) = stalled().await;
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    let id = create(&e, root.path(), &url).await;
    let mut rule = recurring(ScheduleKind::Download, Some(id.clone()), 0, 1);
    rule.at_ms = Some(time("2026-11-02T08:00:00Z"));
    e.save_schedule(rule).await.unwrap();
    let mut edited = e.scheduler_view().await.rules[0].rule.clone();
    edited.at_ms = Some(time("2026-11-02T10:00:00Z"));
    e.save_schedule(edited).await.unwrap();
    assert_eq!(
        e.scheduler_view().await.rules[0].next_run_ms,
        Some(time("2026-11-02T10:00:00Z"))
    );
    c.0.store(time("2026-11-02T11:00:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Ready);
    assert!(e.scheduler_view().await.rules[0].status.contains("Missed"));
    e.shutdown().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn local_one_time_resolution_rejects_gaps_chooses_earliest_fold_and_stays_absolute() {
    use download_core::LocalScheduleDate;
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-01-01T00:00:00Z");
    let e = Engine::open_with_clock_in_timezone(
        root.path().join("meta"),
        c.clone(),
        "America/New_York",
    )
    .await
    .unwrap();
    let id = create(&e, root.path(), "http://127.0.0.1:1/file").await;
    let mut r = recurring(ScheduleKind::Download, Some(id), 150, 210);
    r.name.clear();
    r.enabled = false;
    let result = e
        .save_schedule_local(
            r.clone(),
            Some(LocalScheduleDate {
                year: 2026,
                month: 3,
                day: 8,
            }),
        )
        .await;
    assert!(result.unwrap_err().to_string().contains("does not exist"));
    assert!(e.scheduler_view().await.rules.is_empty());
    r.start_minute = 90;
    let rule = e
        .save_schedule_local(
            r,
            Some(LocalScheduleDate {
                year: 2026,
                month: 11,
                day: 1,
            }),
        )
        .await
        .unwrap();
    let v = e.scheduler_view().await;
    assert_eq!(v.rules[0].rule.at_ms, Some(time("2026-11-01T05:30:00Z")));
    assert_eq!(v.rules[0].rule.name, "scheduled.bin");
    e.set_system_timezone("Asia/Tehran").await.unwrap();
    assert_eq!(
        e.scheduler_view().await.rules[0].rule.at_ms,
        v.rules[0].rule.at_ms
    );
    e.shutdown().await.unwrap();
    drop(e);
    let e = Engine::open_with_clock_in_timezone(root.path().join("meta"), c, "Pacific/Auckland")
        .await
        .unwrap();
    assert_eq!(e.scheduler_view().await.rules[0].rule.id, rule);
    assert_eq!(
        e.scheduler_view().await.rules[0].rule.at_ms,
        v.rules[0].rule.at_ms
    );
    e.shutdown().await.unwrap();
}

#[tokio::test]
async fn system_timezone_updates_active_bandwidth_and_restart_uses_new_zone_before_admission() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T08:30:00Z");
    let e = Engine::open_with_clock(root.path(), c.clone())
        .await
        .unwrap();
    e.set_global_bandwidth_limit(Some(5_000_000)).await.unwrap();
    let mut r = recurring(ScheduleKind::Bandwidth, None, 540, 600);
    r.timezone = "Asia/Tokyo".into(); // API callers cannot pin the rule anymore.
    r.bandwidth_limit = Some(2_000_000);
    e.save_schedule(r).await.unwrap();
    assert_eq!(e.scheduler_view().await.effective_limit, Some(5_000_000));
    e.set_system_timezone("Europe/Paris").await.unwrap(); // local 09:30
    assert_eq!(e.scheduler_view().await.effective_limit, Some(2_000_000));
    e.set_global_bandwidth_limit(Some(3_000_000)).await.unwrap();
    e.set_system_timezone("UTC").await.unwrap();
    assert_eq!(e.scheduler_view().await.effective_limit, Some(3_000_000));
    e.shutdown().await.unwrap();
    drop(e);
    let e = Engine::open_with_clock_in_timezone(root.path(), c.clone(), "Europe/Paris")
        .await
        .unwrap();
    assert_eq!(e.scheduler_view().await.effective_limit, Some(2_000_000));
    assert_eq!(
        e.scheduler_view().await.rules[0].rule.timezone,
        "Europe/Paris"
    );
    assert!(e.set_system_timezone("invalid zone").await.is_err());
    assert_eq!(e.scheduler_view().await.effective_limit, Some(2_000_000));
    // Same zone, new DST offset: spring gap occurrence is skipped without changing config.
    let mut r = e.scheduler_view().await.rules[0].rule.clone();
    r.start_minute = 150;
    r.end_minute = 210;
    e.save_schedule(r).await.unwrap();
    c.0.store(time("2026-03-29T01:15:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    assert_eq!(e.scheduler_view().await.effective_limit, Some(3_000_000));
    e.shutdown().await.unwrap();
}

#[tokio::test]
async fn timezone_changes_rebind_open_ownership_but_never_replay_a_closed_local_day() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T09:30:00Z");
    let (url, server) = stalled().await;
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    e.configure_queue(download_core::QueueConfig {
        max_downloads: 1,
        global_connections: 64,
        per_host_connections: 32,
    })
    .await
    .unwrap();
    let blocker = create(&e, root.path(), &url).await;
    e.start(&blocker).await.unwrap();
    let id = e
        .create(url, root.path().into(), "waiting.bin".into())
        .await
        .unwrap()
        .id;
    e.save_schedule(recurring(
        ScheduleKind::Download,
        Some(id.clone()),
        540,
        720,
    ))
    .await
    .unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Queued);
    let day = e.scheduler_view().await.rules[0].rule.last_local_day;
    e.set_system_timezone("Europe/Paris").await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Queued);
    assert_eq!(
        e.scheduler_view().await.rules[0].rule.last_occurrence,
        Some(time("2026-11-02T08:00:00Z"))
    );
    e.set_system_timezone("Asia/Tehran").await.unwrap(); // local 13:00, closed
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    e.set_system_timezone("UTC").await.unwrap(); // open wall-clock window again, same day
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    assert_eq!(e.scheduler_view().await.rules[0].rule.last_local_day, day);
    e.shutdown().await.unwrap();
    drop(e);
    let db = rusqlite::Connection::open(root.path().join("meta/downloads.sqlite3")).unwrap();
    let raw: String = db
        .query_row("SELECT config FROM automation_state", [], |r| r.get(0))
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    value.as_object_mut().unwrap().remove("system_timezone");
    for rule in value["rules"].as_object_mut().unwrap().values_mut() {
        rule.as_object_mut().unwrap().remove("last_local_day");
    }
    db.execute("UPDATE automation_state SET config=?1", [value.to_string()])
        .unwrap();
    drop(db);
    let e =
        Engine::open_with_clock_in_timezone(root.path().join("meta"), c.clone(), "Europe/Paris")
            .await
            .unwrap();
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    c.0.store(time("2026-11-03T09:30:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    support::wait(&e, &id, |d| d.status == Status::Running).await;
    let (a, b) = tokio::join!(e.pause(&id), e.set_system_timezone("UTC"));
    a.unwrap();
    b.unwrap();
    e.set_system_timezone("Europe/Paris").await.unwrap();
    e.tick_automation().await.unwrap();
    assert_eq!(
        e.get(&id).await.unwrap().pause_origin,
        Some(PauseOrigin::User)
    );
    e.shutdown().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn legacy_calendar_json_converts_without_replay_and_zone_write_failure_is_atomic() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T09:30:00Z");
    let e = Engine::open_with_clock(root.path(), c.clone())
        .await
        .unwrap();
    let mut r = recurring(ScheduleKind::Bandwidth, None, 540, 600);
    r.bandwidth_limit = Some(2_000_000);
    e.save_schedule(r).await.unwrap();
    e.shutdown().await.unwrap();
    drop(e);
    let db = rusqlite::Connection::open(root.path().join("downloads.sqlite3")).unwrap();
    let raw: String = db
        .query_row("SELECT config FROM automation_state", [], |r| r.get(0))
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    value.as_object_mut().unwrap().remove("system_timezone");
    for rule in value["rules"].as_object_mut().unwrap().values_mut() {
        rule.as_object_mut().unwrap().remove("last_local_day");
    }
    db.execute("UPDATE automation_state SET config=?1", [value.to_string()])
        .unwrap();
    drop(db);
    let e = Engine::open_with_clock_in_timezone(root.path(), c, "Asia/Tehran")
        .await
        .unwrap();
    assert_eq!(
        e.scheduler_view().await.rules[0].rule.timezone,
        "Asia/Tehran"
    );
    assert_eq!(e.scheduler_view().await.effective_limit, None);
    let db = rusqlite::Connection::open(root.path().join("downloads.sqlite3")).unwrap();
    db.execute_batch("CREATE TRIGGER zone_failure BEFORE UPDATE ON automation_state BEGIN SELECT RAISE(ABORT,'zone fixture'); END;").unwrap();
    assert!(e.set_system_timezone("UTC").await.is_err());
    assert_eq!(
        e.scheduler_view().await.rules[0].rule.timezone,
        "Asia/Tehran"
    );
    assert_eq!(e.scheduler_view().await.effective_limit, None);
    db.execute_batch("DROP TRIGGER zone_failure;").unwrap();
    e.shutdown().await.unwrap();
}

#[tokio::test]
async fn scheduled_creation_is_atomic_and_missed_restart_waits_for_start_now() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T13:16:00Z");
    let f = support::fixture().await;
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    let mut r = recurring(ScheduleKind::Download, None, 0, 1);
    r.at_ms = Some(time("2026-11-02T13:17:00Z"));
    let (d, tick) = tokio::join!(
        e.create_scheduled_download(
            f.url.clone(),
            root.path().into(),
            "future.bin".into(),
            TransferConfig {
                max_connections: 2,
                optimize: false
            },
            r,
            None
        ),
        e.tick_automation()
    );
    let d = d.unwrap();
    tick.unwrap();
    assert_eq!(e.get(&d.id).await.unwrap().status, Status::Ready);
    assert_eq!(e.get(&d.id).await.unwrap().config.max_connections, 2);
    assert_eq!(
        e.scheduler_view().await.rules[0]
            .rule
            .download_id
            .as_deref(),
        Some(d.id.as_str())
    );
    assert!(f.state.timestamps.lock().unwrap().is_empty());
    let db = rusqlite::Connection::open(root.path().join("meta/downloads.sqlite3")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM downloads", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        1
    );
    let raw: String = db
        .query_row("SELECT config FROM automation_state", [], |r| r.get(0))
        .unwrap();
    assert!(raw.contains(&d.id));
    drop(db);
    e.shutdown().await.unwrap();
    drop(e);
    c.0.store(time("2026-11-02T13:18:00Z"), Ordering::SeqCst);
    let e = Engine::open_with_clock(root.path().join("meta"), c)
        .await
        .unwrap();
    assert!(!e.scheduler_view().await.rules[0].held);
    e.tick_automation().await.unwrap();
    assert!(e.scheduler_view().await.rules[0].notify_missed);
    assert!(f.state.timestamps.lock().unwrap().is_empty());
    e.start(&d.id).await.unwrap();
    assert!(e.scheduler_view().await.rules.is_empty());
    support::done(&e, &d.id, &f.state.data).await;
    assert_eq!(e.list().await.len(), 1);
    e.shutdown().await.unwrap();
}

#[tokio::test]
async fn scheduled_creation_failure_rolls_back_both_records_and_cleans_only_its_empty_candidate() {
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("output");
    std::fs::create_dir(&output).unwrap();
    let c = clock("2026-11-02T13:16:00Z");
    let f = support::fixture().await;
    let e = Engine::open_with_clock(root.path().join("meta"), c)
        .await
        .unwrap();
    let db = rusqlite::Connection::open(root.path().join("meta/downloads.sqlite3")).unwrap();
    db.execute_batch("CREATE TRIGGER refuse_schedule BEFORE UPDATE ON automation_state BEGIN SELECT RAISE(ABORT,'atomic fixture'); END;").unwrap();
    let mut r = recurring(ScheduleKind::Download, None, 0, 1);
    r.at_ms = Some(time("2026-11-02T13:17:00Z"));
    assert!(
        e.create_scheduled_download(
            f.url.clone(),
            output.clone(),
            "future.bin".into(),
            TransferConfig::default(),
            r,
            None
        )
        .await
        .is_err()
    );
    assert!(e.list().await.is_empty());
    assert!(e.scheduler_view().await.rules.is_empty());
    assert_eq!(
        db.query_row("SELECT count(*) FROM downloads", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        0
    );
    assert_eq!(std::fs::read_dir(output).unwrap().count(), 0);
    assert!(f.state.timestamps.lock().unwrap().is_empty());
    db.execute_batch("DROP TRIGGER refuse_schedule;").unwrap();
    e.shutdown().await.unwrap();
}

#[tokio::test]
async fn paused_1317_schedule_starts_without_checkbox_and_later_pause_wins() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T13:16:00Z");
    let (url, server) = stalled().await;
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    let id = create(&e, root.path(), &url).await;
    e.pause(&id).await.unwrap();
    let mut r = recurring(ScheduleKind::Download, Some(id.clone()), 0, 1);
    r.at_ms = Some(time("2026-11-02T13:17:00Z"));
    let rule = e.save_schedule(r).await.unwrap();
    c.0.store(time("2026-11-02T13:18:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    support::wait(&e, &id, |d| d.status == Status::Running).await;
    assert!(!e.scheduler_view().await.rules[0].held);
    e.pause(&id).await.unwrap();
    e.tick_automation().await.unwrap();
    assert!(e.scheduler_view().await.rules[0].held);
    e.shutdown().await.unwrap();
    drop(e);
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    assert_eq!(e.scheduler_view().await.rules[0].rule.id, rule);
    c.0.store(time("2026-11-02T13:40:00Z"), Ordering::SeqCst);
    e.save_schedule_with_consent(e.scheduler_view().await.rules[0].rule.clone(), None, true)
        .await
        .unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    assert!(e.scheduler_view().await.rules[0].status.contains("Missed"));
    e.shutdown().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn queued_scheduling_defers_atomically_and_active_save_never_interrupts_a_transfer() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T13:16:00Z");
    let (url, server) = stalled().await;
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    e.configure_queue(download_core::QueueConfig {
        max_downloads: 1,
        global_connections: 64,
        per_host_connections: 32,
    })
    .await
    .unwrap();
    let blocker = create(&e, root.path(), &url).await;
    e.start(&blocker).await.unwrap();
    let id = e
        .create(url, root.path().into(), "queued.bin".into())
        .await
        .unwrap()
        .id;
    e.start(&id).await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Queued);
    let mut r = recurring(ScheduleKind::Download, Some(id.clone()), 0, 1);
    r.at_ms = Some(time("2026-11-02T13:17:00Z"));
    e.save_schedule(r).await.unwrap();
    e.pause(&blocker).await.unwrap();
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    c.0.store(time("2026-11-02T13:17:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    support::wait(&e, &id, |d| d.status == Status::Running).await;
    let r = e.scheduler_view().await.rules[0].rule.clone();
    assert!(e.save_schedule_with_consent(r, None, true).await.is_err());
    assert_eq!(e.get(&id).await.unwrap().status, Status::Running);
    assert_eq!(e.scheduler_view().await.rules[0].next_run_ms, None);
    e.shutdown().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn pending_once_is_consumed_on_admission_and_manual_resume_cancels_its_future_instruction() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T13:16:00Z");
    let (url, server) = stalled().await;
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    let id = create(&e, root.path(), &url).await;
    let mut r = recurring(ScheduleKind::Download, Some(id.clone()), 0, 1);
    r.at_ms = Some(time("2026-11-02T13:17:00Z"));
    e.pause(&id).await.unwrap();
    e.save_schedule(r).await.unwrap();
    assert!(e.scheduler_view().await.rules[0].pending);
    assert!(!e.scheduler_view().await.rules[0].held);
    c.0.store(time("2026-11-02T13:17:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Running);
    assert!(!e.scheduler_view().await.rules[0].pending);
    e.pause(&id).await.unwrap();
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    let mut r = e.scheduler_view().await.rules[0].rule.clone();
    r.at_ms = Some(time("2026-11-02T14:00:00Z"));
    e.save_schedule(r).await.unwrap();
    assert!(e.scheduler_view().await.rules[0].pending);
    e.start(&id).await.unwrap();
    assert!(e.scheduler_view().await.rules.is_empty());
    e.pause(&id).await.unwrap();
    e.shutdown().await.unwrap();
    drop(e);
    let e = Engine::open_with_clock(root.path().join("meta"), c)
        .await
        .unwrap();
    assert!(e.scheduler_view().await.rules.is_empty());
    assert_eq!(e.list().await.len(), 1);
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    e.shutdown().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn missed_once_is_persistent_dismissible_and_never_replayed_by_clock_rewind() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T13:16:00Z");
    let (url, server) = stalled().await;
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    let id = create(&e, root.path(), &url).await;
    let mut r = recurring(ScheduleKind::Download, Some(id.clone()), 0, 1);
    r.at_ms = Some(time("2026-11-02T13:17:00Z"));
    let rid = e.save_schedule(r).await.unwrap();
    e.shutdown().await.unwrap();
    drop(e);
    c.0.store(time("2026-11-02T13:18:00Z"), Ordering::SeqCst);
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    e.tick_automation().await.unwrap();
    assert!(e.scheduler_view().await.rules[0].notify_missed);
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    // A stale dismiss cannot acknowledge another occurrence.
    e.dismiss_missed_schedules(&[(rid.clone(), time("2026-11-02T12:17:00Z"))])
        .await
        .unwrap();
    assert!(e.scheduler_view().await.rules[0].notify_missed);
    e.dismiss_missed_schedules(&[(rid, time("2026-11-02T13:17:00Z"))])
        .await
        .unwrap();
    assert!(!e.scheduler_view().await.rules[0].notify_missed);
    e.shutdown().await.unwrap();
    drop(e);
    c.0.store(time("2026-11-02T13:16:00Z"), Ordering::SeqCst);
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    c.0.store(time("2026-11-02T13:18:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    assert!(!e.scheduler_view().await.rules[0].notify_missed);
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    let mut r = e.scheduler_view().await.rules[0].rule.clone();
    r.at_ms = Some(time("2026-11-02T13:19:00Z"));
    e.save_schedule(r).await.unwrap();
    assert_eq!(e.scheduler_view().await.rules[0].missed_at_ms, None);
    c.0.store(time("2026-11-02T13:19:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Running);
    e.shutdown().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn missed_weekly_window_waits_for_user_and_next_missed_occurrence_notifies_once_again() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T08:00:00Z");
    let (url, server) = stalled().await;
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    let id = create(&e, root.path(), &url).await;
    let rid = e
        .save_schedule(recurring(
            ScheduleKind::Download,
            Some(id.clone()),
            540,
            600,
        ))
        .await
        .unwrap();
    e.shutdown().await.unwrap();
    drop(e);
    c.0.store(time("2026-11-02T09:30:00Z"), Ordering::SeqCst);
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    e.tick_automation().await.unwrap();
    let v = e.scheduler_view().await;
    assert!(v.rules[0].notify_missed);
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    e.dismiss_missed_schedules(&[(rid.clone(), v.rules[0].missed_at_ms.unwrap())])
        .await
        .unwrap();
    e.shutdown().await.unwrap();
    drop(e);
    c.0.store(time("2026-11-03T09:30:00Z"), Ordering::SeqCst);
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    e.tick_automation().await.unwrap();
    assert!(e.scheduler_view().await.rules[0].notify_missed);
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    // If DLM stays open, a later explicitly scheduled weekly window still runs normally.
    c.0.store(time("2026-11-04T09:00:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Running);
    assert!(!e.scheduler_view().await.rules[0].notify_missed);
    e.pause(&id).await.unwrap();
    c.0.store(time("2026-11-05T09:00:00Z"), Ordering::SeqCst);
    e.tick_automation().await.unwrap();
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    e.delete_schedule(&rid).await.unwrap();
    assert_eq!(e.list().await.len(), 1);
    assert!(std::path::Path::new(&e.get(&id).await.unwrap().partial_path).exists());
    e.shutdown().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn creation_caps_and_settings_are_durable_before_visibility_and_failed_creation_is_atomic() {
    use download_core::CreationSettings;
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T08:00:00Z");
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    let d = e
        .create_with_settings(
            "http://127.0.0.1:1/file".into(),
            root.path().into(),
            "limited.bin".into(),
            CreationSettings {
                transfer: TransferConfig {
                    max_connections: 2,
                    optimize: false,
                },
                bandwidth_limit: Some(1_250_000),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(e.get(&d.id).await.unwrap().bandwidth_limit, Some(1_250_000));
    assert_eq!(d.config.max_connections, 2);
    let r = recurring(ScheduleKind::Download, None, 540, 600);
    let sd = e
        .create_scheduled_with_settings(
            "http://127.0.0.1:1/file".into(),
            root.path().into(),
            "scheduled-limit.bin".into(),
            CreationSettings {
                transfer: TransferConfig::default(),
                bandwidth_limit: Some(750_000),
                ..Default::default()
            },
            r,
            None,
        )
        .await
        .unwrap();
    assert_eq!(sd.bandwidth_limit, Some(750_000));
    assert_eq!(e.list().await.len(), 2);
    assert!(
        e.create_with_settings(
            "http://127.0.0.1:1/file".into(),
            root.path().into(),
            "bad.bin".into(),
            CreationSettings {
                transfer: TransferConfig::default(),
                bandwidth_limit: Some(0),
                ..Default::default()
            }
        )
        .await
        .is_err()
    );
    assert_eq!(e.list().await.len(), 2);
    e.shutdown().await.unwrap();
    drop(e);
    let e = Engine::open_with_clock(root.path().join("meta"), c)
        .await
        .unwrap();
    assert_eq!(e.get(&d.id).await.unwrap().bandwidth_limit, Some(1_250_000));
    assert_eq!(e.get(&sd.id).await.unwrap().bandwidth_limit, Some(750_000));
    e.shutdown().await.unwrap();
}

#[tokio::test]
async fn startup_in_a_new_timezone_reviews_an_unclaimed_open_window_instead_of_starting() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T08:30:00Z");
    let (url, server) = stalled().await;
    let e = Engine::open_with_clock_in_timezone(root.path().join("meta"), c.clone(), "UTC")
        .await
        .unwrap();
    let id = create(&e, root.path(), &url).await;
    e.save_schedule(recurring(
        ScheduleKind::Download,
        Some(id.clone()),
        540,
        600,
    ))
    .await
    .unwrap();
    e.shutdown().await.unwrap();
    drop(e);
    // Current Paris window began at 08:00 UTC, before the previous shutdown timestamp.
    let e = Engine::open_with_clock_in_timezone(root.path().join("meta"), c, "Europe/Paris")
        .await
        .unwrap();
    e.tick_automation().await.unwrap();
    assert!(e.scheduler_view().await.rules[0].notify_missed);
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    e.shutdown().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn consumed_hidden_history_is_reclaimed_on_new_schedule_without_touching_download_data() {
    let root = tempfile::tempdir().unwrap();
    let c = clock("2026-11-02T13:17:00Z");
    let (url, server) = stalled().await;
    let e = Engine::open_with_clock(root.path().join("meta"), c.clone())
        .await
        .unwrap();
    let id = create(&e, root.path(), &url).await;
    let mut r = recurring(ScheduleKind::Download, Some(id.clone()), 0, 1);
    r.at_ms = Some(c.now_ms());
    e.save_schedule(r).await.unwrap();
    support::wait(&e, &id, |d| d.status == Status::Running).await;
    e.pause(&id).await.unwrap();
    assert!(!e.scheduler_view().await.rules[0].pending);
    let next = e
        .create(url, root.path().into(), "another.bin".into())
        .await
        .unwrap();
    let mut r = recurring(ScheduleKind::Download, Some(next.id.clone()), 0, 1);
    r.at_ms = Some(time("2026-11-02T14:00:00Z"));
    e.save_schedule(r).await.unwrap();
    let v = e.scheduler_view().await;
    assert_eq!(v.rules.len(), 1);
    assert_eq!(
        v.rules[0].rule.download_id.as_deref(),
        Some(next.id.as_str())
    );
    assert_eq!(e.list().await.len(), 2);
    assert_eq!(e.get(&id).await.unwrap().status, Status::Paused);
    assert!(std::path::Path::new(&e.get(&id).await.unwrap().partial_path).exists());
    e.shutdown().await.unwrap();
    server.abort();
}
