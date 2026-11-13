use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, Response},
    routing::get,
};
use download_core::{Download, Engine, Status};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Clone)]
pub struct FixtureState {
    pub data: Arc<Vec<u8>>,
    pub mode: Arc<AtomicUsize>,
    pub active: Arc<AtomicUsize>,
    pub peak: Arc<AtomicUsize>,
    pub finished: Arc<Mutex<Vec<u64>>>,
    pub requested: Arc<Mutex<Vec<(u64, u64)>>>,
    pub throttle_budget: Arc<AtomicUsize>,
    pub timestamps: Arc<Mutex<Vec<Instant>>>,
}
pub struct Fixture {
    pub url: String,
    pub state: FixtureState,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
struct Guard(Arc<AtomicUsize>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
async fn handler(State(s): State<FixtureState>, headers: HeaderMap) -> Response<Body> {
    let mode = s.mode.load(Ordering::SeqCst);
    if mode == 13 {
        return Response::builder()
            .status(302)
            .header("Location", "/file")
            .body(Body::empty())
            .unwrap();
    }
    respond(s, headers).await
}
async fn respond(s: FixtureState, headers: HeaderMap) -> Response<Body> {
    let mode = s.mode.load(Ordering::SeqCst);
    s.timestamps.lock().unwrap().push(Instant::now());
    let range = headers
        .get("Range")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|v| v.split_once('-'))
        .map(|(a, b)| {
            (
                a.parse::<u64>().unwrap(),
                if b.is_empty() {
                    s.data.len() as u64 - 1
                } else {
                    b.parse::<u64>().unwrap()
                },
            )
        });
    if mode == 10 {
        return Response::builder().status(403).body(Body::empty()).unwrap();
    }
    if mode == 19 && s.active.load(Ordering::SeqCst) >= 2 && range != Some((0, 0)) {
        return Response::builder()
            .status(429)
            .header("Retry-After", "1")
            .body(Body::empty())
            .unwrap();
    }
    let throttled = matches!(mode, 6 | 14)
        && s.throttle_budget
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
    if throttled {
        return Response::builder()
            .status(if mode == 6 { 429 } else { 503 })
            .header("Retry-After", "1")
            .body(Body::empty())
            .unwrap();
    }
    let ignore = mode == 1 || mode == 11 || mode == 20 || (mode == 9 && range != Some((0, 0)));
    let (start, end) = if ignore {
        (0, s.data.len() as u64 - 1)
    } else {
        range.unwrap_or((0, s.data.len() as u64 - 1))
    };
    if end >= s.data.len() as u64 || mode == 15 {
        return Response::builder().status(416).body(Body::empty()).unwrap();
    }
    s.requested.lock().unwrap().push((start, end + 1));
    let count = s.active.fetch_add(1, Ordering::SeqCst) + 1;
    s.peak.fetch_max(count, Ordering::SeqCst);
    let guard = Guard(s.active.clone());
    let data = s.data.clone();
    let finished = s.finished.clone();
    let stall = mode == 17
        && end > start
        && s.throttle_budget
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
    let stream = futures_util::stream::unfold((start, guard), move |(position, guard)| {
        let data = data.clone();
        let finished = finished.clone();
        async move {
            if position > end {
                finished.lock().unwrap().push(start);
                return None;
            }
            if mode == 7 && position >= start + 65536 && end > start {
                return Some((
                    Err::<Bytes, _>(std::io::Error::other("injected reset")),
                    (end + 1, guard),
                ));
            }
            if stall && position == start {
                tokio::time::sleep(Duration::from_secs(31)).await;
            }
            let delay = if mode == 16 || mode == 20 {
                12
            } else if mode == 8 && start == 0 && end > 0 {
                8
            } else {
                1
            };
            tokio::time::sleep(Duration::from_millis(delay)).await;
            let next = (position + 32768).min(end + 1);
            if next > end {
                finished.lock().unwrap().push(start);
            }
            Some((
                Ok::<_, std::io::Error>(Bytes::copy_from_slice(
                    &data[position as usize..next as usize],
                )),
                (next, guard),
            ))
        }
    });
    let mut response = Response::builder().header("Accept-Ranges", "bytes");
    if mode != 5 {
        response = response.header(
            "ETag",
            if mode == 3 {
                "\"version-2\""
            } else {
                "\"version-1\""
            },
        );
    }
    response = response.header(
        "Last-Modified",
        if mode == 4 {
            "Wed, 21 Oct 2015 07:28:01 GMT"
        } else {
            "Wed, 21 Oct 2015 07:28:00 GMT"
        },
    );
    if range.is_some() && !ignore {
        response = response.status(206).header(
            "Content-Range",
            format!(
                "bytes {}-{}/{}",
                if mode == 2 { start + 1 } else { start },
                end,
                if mode == 18 {
                    "*".into()
                } else {
                    s.data.len().to_string()
                }
            ),
        );
    }
    if mode != 11 {
        response = response.header("Content-Length", end - start + 1);
    }
    response.body(Body::from_stream(stream)).unwrap()
}
async fn redirected(State(s): State<FixtureState>, headers: HeaderMap) -> Response<Body> {
    respond(s, headers).await
}
pub async fn fixture() -> Fixture {
    fixture_with_size(16).await
}
pub async fn fixture_with_size(mib: usize) -> Fixture {
    let state = FixtureState {
        data: Arc::new((0..mib * 1024 * 1024).map(|i| (i % 251) as u8).collect()),
        mode: Arc::new(AtomicUsize::new(0)),
        active: Arc::new(AtomicUsize::new(0)),
        peak: Arc::new(AtomicUsize::new(0)),
        finished: Arc::new(Mutex::new(vec![])),
        requested: Arc::new(Mutex::new(vec![])),
        throttle_budget: Arc::new(AtomicUsize::new(0)),
        timestamps: Arc::new(Mutex::new(vec![])),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/source", listener.local_addr().unwrap());
    let router = Router::new()
        .route("/source", get(handler))
        .route("/file", get(redirected))
        .with_state(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Fixture { url, state, task }
}
pub async fn wait(engine: &Engine, id: &str, predicate: impl Fn(&Download) -> bool) -> Download {
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let d = engine.get(id).await.unwrap();
            if predicate(&d) {
                return d;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("state timeout")
}
pub async fn done(engine: &Engine, id: &str, expected: &[u8]) -> Download {
    let d = wait(engine, id, |d| {
        matches!(d.status, Status::Completed | Status::Failed)
    })
    .await;
    assert_eq!(d.status, Status::Completed, "{:?}", d.error);
    assert_eq!(tokio::fs::read(&d.destination).await.unwrap(), expected);
    d
}
