use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, Response},
    routing::get,
};
use download_core::{Engine, Status};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tempfile::TempDir;

#[derive(Clone)]
struct ServerState {
    data: Arc<Vec<u8>>,
    mode: Arc<AtomicUsize>,
    ranges: Arc<AtomicUsize>,
}
struct Server {
    url: String,
    state: ServerState,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn handler(State(state): State<ServerState>, headers: HeaderMap) -> Response<Body> {
    let mode = state.mode.load(Ordering::SeqCst);
    let requested = headers
        .get("range")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|v| v.strip_suffix('-'))
        .and_then(|v| v.parse::<usize>().ok());
    if requested.is_some() {
        state.ranges.fetch_add(1, Ordering::SeqCst);
    }
    if mode == 6 {
        return Response::builder().status(503).body(Body::empty()).unwrap();
    }
    if mode == 8 && requested.is_some() {
        return Response::builder().status(416).body(Body::empty()).unwrap();
    }
    let start = if mode == 1 || mode == 7 {
        0
    } else {
        requested.unwrap_or(0)
    };
    let changed = mode == 2 || mode == 7;
    let data = if mode == 7 {
        Arc::new(
            state
                .data
                .iter()
                .map(|b| b.wrapping_add(1))
                .collect::<Vec<_>>(),
        )
    } else {
        state.data.clone()
    };
    let end = if mode == 5 {
        (start + 96 * 1024).min(data.len())
    } else {
        data.len()
    };
    let chunks =
        futures_util::stream::unfold((start, data.clone()), move |(position, data)| async move {
            if position >= end {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(3)).await;
            let next = (position + 8192).min(end);
            Some((
                Ok::<_, std::io::Error>(Bytes::copy_from_slice(&data[position..next])),
                (next, data),
            ))
        });
    let mut builder = Response::builder();
    if mode != 9 {
        builder = builder.header("ETag", if changed { "\"v2\"" } else { "\"v1\"" });
    }
    if start > 0 {
        builder = builder.status(206).header(
            "Content-Range",
            format!(
                "bytes {}-{}/{}",
                if mode == 3 { start + 1 } else { start },
                data.len() - 1,
                data.len()
            ),
        );
    }
    if mode != 4 {
        builder = builder.header("Content-Length", (data.len() - start).to_string());
    }
    builder.body(Body::from_stream(chunks)).unwrap()
}
async fn server() -> Server {
    let state = ServerState {
        data: Arc::new((0..2 * 1024 * 1024).map(|n| (n % 251) as u8).collect()),
        mode: Arc::new(AtomicUsize::new(0)),
        ranges: Arc::new(AtomicUsize::new(0)),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/file", listener.local_addr().unwrap());
    let router = Router::new()
        .route("/file", get(handler))
        .with_state(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server { url, state, task }
}
async fn wait_for(
    engine: &Engine,
    id: &str,
    predicate: impl Fn(&download_core::Download) -> bool,
) -> download_core::Download {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let d = engine.get(id).await.unwrap();
            if predicate(&d) {
                return d;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("download timed out")
}
async fn setup(server: &Server, temp: &TempDir) -> (Engine, String) {
    let engine = Engine::open(temp.path().join("metadata")).await.unwrap();
    let d = engine
        .create(server.url.clone(), temp.path().into(), "output.bin".into())
        .await
        .unwrap();
    (engine, d.id)
}
async fn finish(engine: &Engine, id: &str, temp: &TempDir, expected: &[u8]) {
    let d = wait_for(engine, id, |d| {
        matches!(d.status, Status::Completed | Status::Failed)
    })
    .await;
    assert_eq!(d.status, Status::Completed, "{:?}", d.error);
    assert_eq!(
        tokio::fs::read(temp.path().join("output.bin"))
            .await
            .unwrap(),
        expected
    );
    assert_eq!(d.durable_bytes, expected.len() as u64);
    assert!(d.sha256.is_some());
    engine.shutdown().await.unwrap();
}
#[tokio::test]
async fn download_correctness_and_unknown_length() {
    let server = server().await;
    for mode in [0, 4] {
        server.state.mode.store(mode, Ordering::SeqCst);
        let temp = TempDir::new().unwrap();
        let (engine, id) = setup(&server, &temp).await;
        engine.start(&id).await.unwrap();
        finish(&engine, &id, &temp, &server.state.data).await;
    }
}
#[tokio::test]
async fn pause_resume_uses_validated_range() {
    let server = server().await;
    let temp = TempDir::new().unwrap();
    let (engine, id) = setup(&server, &temp).await;
    engine.start(&id).await.unwrap();
    wait_for(&engine, &id, |d| d.downloaded > 100_000).await;
    engine.pause(&id).await.unwrap();
    let paused = engine.get(&id).await.unwrap();
    assert_eq!(paused.status, Status::Paused);
    assert!(paused.durable_bytes > 0);
    assert_eq!(paused.downloaded, paused.durable_bytes);
    engine.resume(&id).await.unwrap();
    finish(&engine, &id, &temp, &server.state.data).await;
    assert!(server.state.ranges.load(Ordering::SeqCst) > 0);
}
#[tokio::test]
async fn ignored_range_and_changed_full_response_restart_safely() {
    let server = server().await;
    for mode in [1, 7] {
        server.state.mode.store(0, Ordering::SeqCst);
        let temp = TempDir::new().unwrap();
        let (engine, id) = setup(&server, &temp).await;
        engine.start(&id).await.unwrap();
        wait_for(&engine, &id, |d| d.downloaded > 100_000).await;
        engine.pause(&id).await.unwrap();
        server.state.mode.store(mode, Ordering::SeqCst);
        engine.resume(&id).await.unwrap();
        let expected = if mode == 7 {
            server
                .state
                .data
                .iter()
                .map(|b| b.wrapping_add(1))
                .collect()
        } else {
            server.state.data.as_ref().clone()
        };
        finish(&engine, &id, &temp, &expected).await;
    }
}
#[tokio::test]
async fn invalid_range_changed_identity_and_416_are_rejected() {
    let server = server().await;
    for mode in [2, 3, 8] {
        server.state.mode.store(0, Ordering::SeqCst);
        let temp = TempDir::new().unwrap();
        let (engine, id) = setup(&server, &temp).await;
        engine.start(&id).await.unwrap();
        wait_for(&engine, &id, |d| d.downloaded > 100_000).await;
        engine.pause(&id).await.unwrap();
        let before = engine.get(&id).await.unwrap().durable_bytes;
        server.state.mode.store(mode, Ordering::SeqCst);
        engine.resume(&id).await.unwrap();
        let d = wait_for(&engine, &id, |d| d.status == Status::Failed).await;
        assert_eq!(d.durable_bytes, before);
        assert!(d.error.is_some());
        assert!(!temp.path().join("output.bin").exists());
        engine.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn interrupted_network_does_not_publish_incomplete_file() {
    let server = server().await;
    server.state.mode.store(5, Ordering::SeqCst);
    let temp = TempDir::new().unwrap();
    let (engine, id) = setup(&server, &temp).await;
    engine.start(&id).await.unwrap();
    let d = wait_for(&engine, &id, |d| d.status == Status::Failed).await;
    assert!(d.error.is_some());
    assert!(!temp.path().join("output.bin").exists());
    server.state.mode.store(0, Ordering::SeqCst);
    engine.resume(&id).await.unwrap();
    finish(&engine, &id, &temp, &server.state.data).await;
}
#[tokio::test]
async fn shutdown_recovery_and_excess_bytes_are_conservative() {
    let server = server().await;
    let temp = TempDir::new().unwrap();
    let (engine, id) = setup(&server, &temp).await;
    engine.start(&id).await.unwrap();
    wait_for(&engine, &id, |d| d.downloaded > 100_000).await;
    engine.shutdown().await.unwrap();
    let saved = engine.get(&id).await.unwrap();
    assert_eq!(saved.status, Status::Paused);
    assert!(matches!(
        engine.resume(&id).await,
        Err(download_core::Error::Closed)
    ));
    drop(engine);
    use tokio::io::AsyncWriteExt;
    tokio::fs::OpenOptions::new()
        .append(true)
        .open(&saved.partial_path)
        .await
        .unwrap()
        .write_all(b"uncheckpointed garbage")
        .await
        .unwrap();
    let engine = Engine::open(temp.path().join("metadata")).await.unwrap();
    assert_eq!(engine.get(&id).await.unwrap().status, Status::Paused);
    engine.resume(&id).await.unwrap();
    finish(&engine, &id, &temp, &server.state.data).await;
}
#[tokio::test]
async fn destination_safety_cancellation_and_exclusive_ownership() {
    let server = server().await;
    let temp = TempDir::new().unwrap();
    let (engine, id) = setup(&server, &temp).await;
    assert!(Engine::open(temp.path().join("metadata")).await.is_err());
    assert!(
        engine
            .create(server.url.clone(), temp.path().into(), "../escape".into())
            .await
            .is_err()
    );
    assert!(
        engine
            .create(server.url.clone(), temp.path().into(), "output.bin".into())
            .await
            .is_err()
    );
    let partial = engine.get(&id).await.unwrap().partial_path;
    engine.start(&id).await.unwrap();
    wait_for(&engine, &id, |d| d.downloaded > 100_000).await;
    engine.cancel(&id).await.unwrap();
    assert_eq!(engine.get(&id).await.unwrap().status, Status::Cancelled);
    assert!(!std::path::Path::new(&partial).exists());
    assert!(engine.resume(&id).await.is_err());
    engine.shutdown().await.unwrap();
}
#[tokio::test]
async fn final_destination_collision_is_a_disk_error_and_never_overwrites() {
    let server = server().await;
    let temp = TempDir::new().unwrap();
    let (engine, id) = setup(&server, &temp).await;
    tokio::fs::write(temp.path().join("output.bin"), b"keep me")
        .await
        .unwrap();
    engine.start(&id).await.unwrap();
    let d = wait_for(&engine, &id, |d| d.status == Status::Failed).await;
    assert!(d.error.unwrap().contains("File error"));
    assert_eq!(
        tokio::fs::read(temp.path().join("output.bin"))
            .await
            .unwrap(),
        b"keep me"
    );
    engine.shutdown().await.unwrap();
}
#[tokio::test]
async fn missing_or_truncated_partial_is_reported() {
    let server = server().await;
    let temp = TempDir::new().unwrap();
    let (engine, id) = setup(&server, &temp).await;
    let path = engine.get(&id).await.unwrap().partial_path;
    tokio::fs::remove_file(path).await.unwrap();
    engine.start(&id).await.unwrap();
    let d = wait_for(&engine, &id, |d| d.status == Status::Failed).await;
    assert!(d.error.is_some());
    engine.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "subprocess helper, launched by abrupt_process_termination"]
async fn checkpoint_child() {
    let directory = std::env::var("DLM_TEST_DIRECTORY").unwrap();
    let engine = Engine::open(std::path::Path::new(&directory).join("metadata"))
        .await
        .unwrap();
    let d = engine
        .create(
            std::env::var("DLM_TEST_URL").unwrap(),
            directory.clone().into(),
            "output.bin".into(),
        )
        .await
        .unwrap();
    engine.start(&d.id).await.unwrap();
    wait_for(&engine, &d.id, |d| d.durable_bytes >= 1024 * 1024).await;
    tokio::fs::write(
        std::path::Path::new(&directory).join("checkpoint-ready"),
        &d.id,
    )
    .await
    .unwrap();
    std::future::pending::<()>().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abrupt_process_termination() {
    let server = server().await;
    let temp = TempDir::new().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "checkpoint_child"])
        .env("DLM_TEST_DIRECTORY", temp.path())
        .env("DLM_TEST_URL", &server.url)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        while !temp.path().join("checkpoint-ready").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    let id = tokio::fs::read_to_string(temp.path().join("checkpoint-ready"))
        .await
        .unwrap();
    let engine = Engine::open(temp.path().join("metadata")).await.unwrap();
    let d = engine.get(&id).await.unwrap();
    assert_eq!(d.status, Status::Paused);
    assert!(d.durable_bytes >= 1024 * 1024);
    engine.resume(&id).await.unwrap();
    finish(&engine, &id, &temp, &server.state.data).await;
}

#[tokio::test]
async fn missing_strong_validator_restarts_instead_of_appending() {
    let server = server().await;
    server.state.mode.store(9, Ordering::SeqCst);
    let temp = TempDir::new().unwrap();
    let (engine, id) = setup(&server, &temp).await;
    engine.start(&id).await.unwrap();
    wait_for(&engine, &id, |d| d.downloaded > 100_000).await;
    engine.pause(&id).await.unwrap();
    assert!(engine.get(&id).await.unwrap().etag.is_none());
    engine.resume(&id).await.unwrap();
    finish(&engine, &id, &temp, &server.state.data).await;
    assert_eq!(server.state.ranges.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn concurrency_is_bounded_and_pause_releases_a_slot() {
    let server = server().await;
    let temp = TempDir::new().unwrap();
    let engine = Engine::open(temp.path().join("metadata")).await.unwrap();
    let mut ids = vec![];
    for n in 0..4 {
        ids.push(
            engine
                .create(server.url.clone(), temp.path().into(), format!("file-{n}"))
                .await
                .unwrap()
                .id,
        );
    }
    for id in &ids[..3] {
        engine.start(id).await.unwrap();
    }
    engine.start(&ids[3]).await.unwrap();
    assert_eq!(engine.get(&ids[3]).await.unwrap().status, Status::Queued);
    assert_eq!(engine.get(&ids[3]).await.unwrap().queue_position, Some(1));
    engine.pause(&ids[0]).await.unwrap();
    wait_for(&engine, &ids[3], |d| d.status == Status::Running).await;
    engine.shutdown().await.unwrap();
    assert!(
        engine
            .list()
            .await
            .iter()
            .all(|d| d.status == Status::Paused)
    );
}

#[tokio::test]
async fn complete_checkpoint_finishes_locally_and_finalization_recovers() {
    use tokio::io::AsyncWriteExt;
    let server = server().await;
    let temp = TempDir::new().unwrap();
    let (engine, id) = setup(&server, &temp).await;
    let mut d = engine.get(&id).await.unwrap();
    engine.shutdown().await.unwrap();
    drop(engine);
    let mut partial = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&d.partial_path)
        .await
        .unwrap();
    partial.write_all(&server.state.data).await.unwrap();
    partial.write_all(b"uncheckpointed tail").await.unwrap();
    partial.sync_all().await.unwrap();
    drop(partial);
    d.status = Status::Running;
    d.total = Some(server.state.data.len() as u64);
    d.downloaded = server.state.data.len() as u64;
    d.durable_bytes = d.downloaded;
    let db = rusqlite::Connection::open(temp.path().join("metadata/downloads.sqlite3")).unwrap();
    db.execute(
        "UPDATE downloads SET metadata=?1 WHERE id=?2",
        rusqlite::params![serde_json::to_string(&d).unwrap(), id],
    )
    .unwrap();
    drop(db);
    let engine = Engine::open(temp.path().join("metadata")).await.unwrap();
    assert_eq!(engine.get(&id).await.unwrap().status, Status::Paused);
    engine.resume(&id).await.unwrap();
    finish(&engine, &id, &temp, &server.state.data).await;
    d = engine.get(&id).await.unwrap();
    drop(engine);
    d.status = Status::Finalizing;
    let db = rusqlite::Connection::open(temp.path().join("metadata/downloads.sqlite3")).unwrap();
    db.execute(
        "UPDATE downloads SET metadata=?1 WHERE id=?2",
        rusqlite::params![serde_json::to_string(&d).unwrap(), id],
    )
    .unwrap();
    drop(db);
    let engine = Engine::open(temp.path().join("metadata")).await.unwrap();
    assert_eq!(engine.get(&id).await.unwrap().status, Status::Completed);
    engine.shutdown().await.unwrap();
}
