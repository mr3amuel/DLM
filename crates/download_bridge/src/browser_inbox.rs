//! Authenticated, bounded application-shell inbox. No engine commands live here.
use crate::browser_protocol::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
    task::{JoinHandle, JoinSet},
};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    enabled: bool,
    token: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    pub id: String,
    pub url: String,
    pub filename: Option<String>,
}
struct State {
    config: Config,
    instance: Option<String>,
    drafts: VecDeque<Draft>,
    seen: BTreeMap<String, Instant>,
    closed: bool,
    retained: Option<PathBuf>,
}
pub struct Inbox {
    dir: PathBuf,
    state: Arc<Mutex<State>>,
    command: Mutex<()>,
    task: StdMutex<Option<JoinHandle<()>>>,
    lease: StdMutex<Option<fs::File>>,
}
impl Inbox {
    #[allow(dead_code)] // Shared portable review APIs.
    pub async fn open(dir: PathBuf) -> Result<Self, &'static str> {
        Self::open_mode(dir, false).await
    }
    // Explicit native opt-in with durable inbox state.
    #[allow(dead_code)] // Shared portable review APIs.
    pub async fn open_retained(dir: PathBuf) -> Result<Self, &'static str> {
        Self::open_mode(dir, true).await
    }
    async fn open_mode(dir: PathBuf, retain: bool) -> Result<Self, &'static str> {
        private_dir(&dir)?;
        let mut opts = fs::OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let lock = opts
            .open(dir.join("inbox.lock"))
            .map_err(|_| "storage_unavailable")?;
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| "busy")?;
        let path = dir.join("pairing.json");
        let config: Config = if path.exists() {
            read_private(&path)?
        } else {
            let c = Config {
                enabled: false,
                token: random_hex(32)?,
            };
            write_private(&path, &c)?;
            c
        };
        if config.token.len() != 64
            || !config
                .token
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err("unsafe_storage");
        }
        let enabled = config.enabled;
        let retained = retain.then(|| dir.join("pending-reviews"));
        if let Some(path) = &retained {
            private_dir(path)?;
        }

        let saved = if enabled {
            retained
                .as_ref()
                .map(|path| load_retained(path))
                .transpose()?
                .unwrap_or_default()
        } else {
            VecDeque::new()
        };
        let inbox = Self {
            dir,
            state: Arc::new(Mutex::new(State {
                config,
                instance: None,
                drafts: saved,
                seen: BTreeMap::new(),
                closed: false,
                retained: retained.clone(),
            })),
            command: Mutex::new(()),
            task: StdMutex::new(None),
            lease: StdMutex::new(Some(lock)),
        };
        // Initial listener setup must not discard the retained queue.
        inbox.configure(enabled).await?;
        Ok(inbox)
    }
    pub fn directory(&self) -> String {
        self.dir.to_string_lossy().into()
    }
    pub async fn enabled(&self) -> bool {
        self.state.lock().await.config.enabled
    }
    pub async fn pairing_code(&self) -> Result<String, &'static str> {
        let s = self.state.lock().await;
        if s.closed {
            return Err("unavailable");
        }
        Ok(s.config.token.clone())
    }
    pub async fn configure(&self, enabled: bool) -> Result<(), &'static str> {
        let _command = self.command.lock().await;
        let mut state = self.state.lock().await;
        if state.closed {
            return Err("unavailable");
        }
        let listener = if enabled {
            Some(
                TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                    .await
                    .map_err(|_| "unavailable")?,
            )
        } else {
            None
        };
        let endpoint = if let Some(listener) = &listener {
            Endpoint {
                version: VERSION,
                enabled: true,
                port: Some(listener.local_addr().map_err(|_| "unavailable")?.port()),
                instance: Some(random_hex(16)?),
            }
        } else {
            Endpoint {
                version: VERSION,
                enabled: false,
                port: None,
                instance: None,
            }
        };
        if !enabled {
            clear_retained(&state)?;
        }
        let config = Config {
            enabled,
            token: state.config.token.clone(),
        };
        write_private(&self.dir.join("pairing.json"), &config)?;
        if let Err(e) = write_private(&endpoint_path(&self.dir), &endpoint) {
            let _ = write_private(&self.dir.join("pairing.json"), &state.config);
            return Err(e);
        }
        if let Some(task) = self.task.lock().unwrap().take() {
            task.abort();
        }
        state.config = config;
        state.instance = endpoint.instance;
        if !enabled || state.retained.is_none() {
            state.drafts.clear();
        }
        state.seen.clear();
        if let Some(listener) = listener {
            let state = self.state.clone();
            *self.task.lock().unwrap() = Some(tokio::spawn(async move {
                let mut tasks = JoinSet::new();
                loop {
                    tokio::select! {accepted=listener.accept(),if tasks.len()<16=>{let Ok((socket,peer))=accepted else{break};if peer.ip().is_loopback(){let state=state.clone();tasks.spawn(async move{let _=tokio::time::timeout(Duration::from_secs(2),serve(socket,state)).await;});}},_=tasks.join_next(),if !tasks.is_empty()=>{}}
                }
            }));
        }
        Ok(())
    }
    pub async fn reset_pairing(&self) -> Result<(), &'static str> {
        let _command = self.command.lock().await;
        let mut state = self.state.lock().await;
        let config = Config {
            enabled: state.config.enabled,
            token: random_hex(32)?,
        };
        if state.closed {
            return Err("unavailable");
        }
        clear_retained(&state)?;
        write_private(&self.dir.join("pairing.json"), &config)?;
        state.config = config;
        state.drafts.clear();
        state.seen.clear();
        Ok(())
    }
    #[allow(dead_code)] // Shared portable review APIs.
    pub async fn pending(&self) -> Option<Draft> {
        self.state.lock().await.drafts.front().cloned()
    }
    #[allow(dead_code)] // Shared portable review APIs.
    pub async fn acknowledge(&self, id: &str) -> Result<(), &'static str> {
        let mut state = self.state.lock().await;
        if state.closed {
            return Err("unavailable");
        }
        if let Some(draft) = state.drafts.iter().find(|draft| draft.id == id) {
            if let Some(path) = &state.retained {
                match fs::remove_file(path.join(retained_name(&draft.id))) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err("storage_unavailable"),
                }
                sync_retained(path)?;
            }
            state.drafts.retain(|draft| draft.id != id);
        }
        Ok(())
    }
    #[allow(dead_code)] // Shared portable review APIs.
    pub async fn take_next(&self) -> Option<Draft> {
        self.state.lock().await.drafts.pop_front()
    }
    pub async fn shutdown(&self) {
        let _command = self.command.lock().await;
        let mut s = self.state.lock().await;
        s.closed = true;
        s.drafts.clear();
        let task = self.task.lock().unwrap().take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
        if s.config.enabled {
            remove_endpoint(&self.dir, s.instance.as_deref());
        }
        self.lease.lock().unwrap().take();
    }
}
fn retained_name(id: &str) -> String {
    // Preserve distinct protocol IDs on case-insensitive macOS filesystems.
    let encoded: String = id.bytes().map(|byte| format!("{byte:02x}")).collect();
    format!("{encoded}.json")
}
fn load_retained(path: &Path) -> Result<VecDeque<Draft>, &'static str> {
    let mut drafts = Vec::new();
    for entry in fs::read_dir(path).map_err(|_| "storage_unavailable")? {
        let path = entry.map_err(|_| "storage_unavailable")?.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let draft: Draft = read_private(&path)?;
        let mut request = Request {
            version: VERSION,
            request_id: draft.id.clone(),
            operation: "review".into(),
            pairing_token: "0".repeat(64),
            url: Some(draft.url.clone()),
            filename: draft.filename.clone(),
        };
        request.validate()?;
        if path.file_name().and_then(|x| x.to_str()) != Some(retained_name(&draft.id).as_str())
            || drafts.len() >= 8
        {
            return Err("unsafe_storage");
        }
        drafts.push(draft);
    }
    drafts.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(drafts.into())
}
fn sync_retained(path: &Path) -> Result<(), &'static str> {
    #[cfg(unix)]
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| "storage_unavailable")?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn clear_retained(state: &State) -> Result<(), &'static str> {
    if let Some(path) = &state.retained {
        for file in fs::read_dir(path).map_err(|_| "storage_unavailable")? {
            let path = file.map_err(|_| "storage_unavailable")?.path();
            if path.extension().and_then(|x| x.to_str()) == Some("json") {
                fs::remove_file(path).map_err(|_| "storage_unavailable")?;
            }
        }
        sync_retained(path)?;
    }
    Ok(())
}
fn remove_endpoint(dir: &Path, instance: Option<&str>) {
    if let Ok(endpoint) = read_private::<Endpoint>(&endpoint_path(dir))
        && endpoint.instance.as_deref() == instance
    {
        let _ = fs::remove_file(endpoint_path(dir));
    }
}
impl Drop for Inbox {
    fn drop(&mut self) {
        if let Some(task) = self.task.get_mut().unwrap().take() {
            task.abort();
        }
        if let Ok(s) = self.state.try_lock()
            && s.config.enabled
        {
            remove_endpoint(&self.dir, s.instance.as_deref());
        }
    }
}
async fn receive(socket: &mut TcpStream) -> Result<Vec<u8>, ()> {
    let mut len = [0; 4];
    socket.read_exact(&mut len).await.map_err(|_| ())?;
    let n = u32::from_ne_bytes(len) as usize;
    if n == 0 || n > MAX_FRAME {
        return Err(());
    }
    let mut data = vec![0; n];
    socket.read_exact(&mut data).await.map_err(|_| ())?;
    Ok(data)
}
async fn send(socket: &mut TcpStream, value: &(impl Serialize + Sync)) -> Result<(), ()> {
    let bytes = serde_json::to_vec(value).map_err(|_| ())?;
    socket
        .write_all(&(bytes.len() as u32).to_ne_bytes())
        .await
        .map_err(|_| ())?;
    socket.write_all(&bytes).await.map_err(|_| ())?;
    Ok(())
}
async fn serve(mut socket: TcpStream, state: Arc<Mutex<State>>) -> Result<(), ()> {
    let challenge: Challenge =
        serde_json::from_slice(&receive(&mut socket).await?).map_err(|_| ())?;
    let hello = {
        let s = state.lock().await;
        let status = if challenge.version != VERSION {
            "unsupported_version"
        } else if s.closed {
            "unavailable"
        } else if !s.config.enabled {
            "disabled"
        } else if s.instance.as_ref() != Some(&challenge.instance)
            || challenge.nonce.len() != 64
            || !challenge.nonce.bytes().all(|b| b.is_ascii_hexdigit())
        {
            "unpaired"
        } else {
            "ready"
        };
        ServerProof {
            version: VERSION,
            status: status.into(),
            proof: if status == "ready" {
                Some(proof(
                    &s.config.token,
                    "server",
                    &challenge.instance,
                    &challenge.nonce,
                    None,
                ))
            } else {
                None
            },
        }
    };
    send(&mut socket, &hello).await?;
    if hello.status != "ready" {
        return Ok(());
    }
    let data = receive(&mut socket).await?;
    let response = match serde_json::from_slice::<Envelope>(&data) {
        Err(_) => Response::new(None, "invalid_request"),
        Ok(mut envelope) => {
            let validation = envelope.request.validate();
            let id = Some(envelope.request.request_id.clone());
            if let Err(e) = validation {
                Response::new(None, e)
            } else {
                let mut s = state.lock().await;
                let status = if s.closed {
                    "unavailable"
                } else if !s.config.enabled {
                    "disabled"
                } else if s.instance.as_ref() != Some(&envelope.instance)
                    || envelope.instance != challenge.instance
                    || !matches_proof(
                        &envelope.request.pairing_token,
                        &proof(
                            &s.config.token,
                            "client",
                            &challenge.instance,
                            &challenge.nonce,
                            Some(&envelope.request),
                        ),
                    )
                {
                    "unpaired"
                } else if envelope.request.operation == "ping" {
                    "ready"
                } else {
                    s.seen
                        .retain(|_, at| at.elapsed() < Duration::from_secs(300));
                    if s.seen.contains_key(&envelope.request.request_id)
                        || s.drafts.iter().any(|d| d.id == envelope.request.request_id)
                    {
                        "already_queued"
                    } else if s.drafts.len() >= 8 || s.seen.len() >= 256 {
                        "busy"
                    } else {
                        let draft = Draft {
                            id: envelope.request.request_id.clone(),
                            url: envelope.request.url.unwrap(),
                            filename: envelope.request.filename,
                        };
                        if s.retained.as_ref().is_some_and(|path| {
                            write_private(&path.join(retained_name(&draft.id)), &draft).is_err()
                        }) {
                            "storage_unavailable"
                        } else {
                            s.drafts.push_back(draft);
                            s.seen.insert(envelope.request.request_id, Instant::now());
                            "queued"
                        }
                    }
                };
                Response::new(id, status)
            }
        }
    };
    send(&mut socket, &response).await
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn exchange(endpoint: &Endpoint, request: Request) -> Response {
        let mut socket =
            TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, endpoint.port.unwrap()))
                .await
                .unwrap();
        let nonce = "c".repeat(64);
        let instance = endpoint.instance.clone().unwrap();
        send(
            &mut socket,
            &Challenge {
                version: 1,
                instance: instance.clone(),
                nonce: nonce.clone(),
            },
        )
        .await
        .unwrap();
        let hello: ServerProof =
            serde_json::from_slice(&receive(&mut socket).await.unwrap()).unwrap();
        if hello.status != "ready" {
            return Response::new(None, &hello.status);
        }
        if !matches_proof(
            hello.proof.as_deref().unwrap_or(""),
            &proof(&request.pairing_token, "server", &instance, &nonce, None),
        ) {
            return Response::new(None, "unpaired");
        }
        let mut request = request;
        let _ = request.validate();
        request.pairing_token = proof(
            &request.pairing_token,
            "client",
            &instance,
            &nonce,
            Some(&request),
        );
        let data = serde_json::to_vec(&Envelope {
            instance: endpoint.instance.clone().unwrap(),
            request,
        })
        .unwrap();
        socket
            .write_all(&(data.len() as u32).to_ne_bytes())
            .await
            .unwrap();
        socket.write_all(&data).await.unwrap();
        let mut len = [0; 4];
        socket.read_exact(&mut len).await.unwrap();
        let mut bytes = vec![0; u32::from_ne_bytes(len) as usize];
        socket.read_exact(&mut bytes).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
    fn request(token: &str, id: u32) -> Request {
        Request {
            version: 1,
            request_id: format!("aaaaaaaa-aaaa-4aaa-8aaa-{id:012x}"),
            operation: "review".into(),
            pairing_token: token.into(),
            url: Some("https://example.org/file?secret=local#fragment".into()),
            filename: Some("review.zip".into()),
        }
    }
    #[tokio::test]
    async fn native_retention_storage_failure_does_not_acknowledge_or_drop_a_review() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("browser-integration");
        let inbox = Inbox::open_retained(dir.clone()).await.unwrap();
        inbox.configure(true).await.unwrap();
        let token = inbox.pairing_code().await.unwrap();
        let endpoint: Endpoint = read_private(&endpoint_path(&dir)).unwrap();
        let folder = dir.join("pending-reviews");
        let moved = dir.join("interrupted-folder");
        fs::rename(&folder, &moved).unwrap();
        fs::write(&folder, "blocked").unwrap();
        let request = request(&token, 3);
        assert_eq!(
            exchange(&endpoint, request.clone()).await.status,
            "storage_unavailable"
        );
        assert!(inbox.pending().await.is_none());
        fs::remove_file(&folder).unwrap();
        fs::rename(&moved, &folder).unwrap();
        assert_eq!(exchange(&endpoint, request.clone()).await.status, "queued");
        fs::rename(&folder, &moved).unwrap();
        assert!(inbox.acknowledge(&request.request_id).await.is_err());
        assert!(inbox.pending().await.is_some());
        fs::rename(moved, folder).unwrap();
        inbox.acknowledge(&request.request_id).await.unwrap();
        assert!(inbox.pending().await.is_none());
        inbox.shutdown().await;
    }

    #[tokio::test]
    async fn native_retained_reviews_survive_restart_ack_and_revocation() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("browser-integration");
        let inbox = Inbox::open_retained(dir.clone()).await.unwrap();
        inbox.configure(true).await.unwrap();
        let token = inbox.pairing_code().await.unwrap();
        let endpoint: Endpoint = read_private(&endpoint_path(&dir)).unwrap();
        let first = request(&token, 1);
        let second = request(&token, 2);
        assert_eq!(exchange(&endpoint, first.clone()).await.status, "queued");
        assert_eq!(
            exchange(&endpoint, first.clone()).await.status,
            "already_queued"
        );
        assert_eq!(exchange(&endpoint, second).await.status, "queued");
        let mut uppercase = first.clone();
        uppercase.request_id = uppercase.request_id.to_ascii_uppercase();
        assert_eq!(exchange(&endpoint, uppercase).await.status, "queued");
        assert_eq!(
            fs::read_dir(dir.join("pending-reviews")).unwrap().count(),
            3
        );
        assert_eq!(inbox.pending().await.unwrap().id, first.request_id);
        assert_eq!(inbox.pending().await.unwrap().id, first.request_id); // Peek is not consume.
        inbox.shutdown().await;
        drop(inbox);
        let inbox = Inbox::open_retained(dir.clone()).await.unwrap();
        let endpoint: Endpoint = read_private(&endpoint_path(&dir)).unwrap();
        assert_eq!(
            exchange(&endpoint, first.clone()).await.status,
            "already_queued"
        );
        inbox.acknowledge(&first.request_id).await.unwrap();
        inbox.acknowledge(&first.request_id).await.unwrap(); // Retry is idempotent.
        assert_ne!(inbox.pending().await.unwrap().id, first.request_id);
        inbox.reset_pairing().await.unwrap();
        assert!(inbox.pending().await.is_none());
        assert_eq!(exchange(&endpoint, first).await.status, "unpaired");
        assert!(
            fs::read_dir(dir.join("pending-reviews"))
                .unwrap()
                .next()
                .is_none()
        );
        inbox.shutdown().await;
    }

    #[tokio::test]
    async fn pairing_queue_replay_rotation_and_restart_are_bounded_and_never_create_downloads() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("browser-integration");
        let inbox = Inbox::open(dir.clone()).await.unwrap();
        assert!(!inbox.enabled().await);
        assert!(inbox.take_next().await.is_none());
        assert!(
            !read_private::<Endpoint>(&endpoint_path(&dir))
                .unwrap()
                .enabled
        );
        assert_eq!(Inbox::open(dir.clone()).await.err(), Some("busy"));
        inbox.configure(true).await.unwrap();
        let endpoint: Endpoint = read_private(&endpoint_path(&dir)).unwrap();
        let token = inbox.pairing_code().await.unwrap();
        assert_eq!(
            exchange(&endpoint, request(&"b".repeat(64), 0))
                .await
                .status,
            "unpaired"
        );
        assert!(inbox.take_next().await.is_none());
        let mut ping = request(&token, 1);
        ping.operation = "ping".into();
        ping.url = None;
        ping.filename = None;
        assert_eq!(exchange(&endpoint, ping).await.status, "ready");
        assert!(inbox.take_next().await.is_none());
        for id in 0..8 {
            assert_eq!(
                exchange(&endpoint, request(&token, id)).await.status,
                "queued"
            );
        }
        assert_eq!(exchange(&endpoint, request(&token, 9)).await.status, "busy");
        let draft = inbox.take_next().await.unwrap();
        assert_eq!(draft.url, "https://example.org/file?secret=local");
        assert_eq!(draft.filename.as_deref(), Some("review.zip"));
        assert_eq!(
            exchange(&endpoint, request(&token, 0)).await.status,
            "already_queued"
        );
        inbox.reset_pairing().await.unwrap();
        assert!(inbox.take_next().await.is_none());
        assert_eq!(
            exchange(&endpoint, request(&token, 20)).await.status,
            "unpaired"
        );
        let token = inbox.pairing_code().await.unwrap();
        inbox.shutdown().await;
        assert!(!endpoint_path(&dir).exists());
        let restarted = Inbox::open(dir.clone()).await.unwrap();
        assert!(restarted.enabled().await);
        assert_eq!(restarted.pairing_code().await.unwrap(), token);
        let new: Endpoint = read_private(&endpoint_path(&dir)).unwrap();
        assert_ne!(endpoint.instance, new.instance);
        let stale = Endpoint {
            port: new.port,
            ..endpoint
        };
        assert_eq!(
            exchange(&stale, request(&token, 30)).await.status,
            "unpaired"
        );
        assert_eq!(exchange(&new, request(&token, 31)).await.status, "queued");
        restarted.configure(false).await.unwrap();
        assert!(restarted.take_next().await.is_none());
        assert!(
            !read_private::<Endpoint>(&endpoint_path(&dir))
                .unwrap()
                .enabled
        );
        restarted.shutdown().await;
        assert!(!temp.path().join("downloads.sqlite3").exists());
    }
    #[tokio::test]
    async fn malformed_frames_and_unknown_payload_fields_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("browser-integration");
        let inbox = Inbox::open(dir.clone()).await.unwrap();
        inbox.configure(true).await.unwrap();
        let endpoint: Endpoint = read_private(&endpoint_path(&dir)).unwrap();
        let mut socket =
            TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, endpoint.port.unwrap()))
                .await
                .unwrap();
        socket
            .write_all(&(MAX_FRAME as u32 + 1).to_ne_bytes())
            .await
            .unwrap();
        let mut one = [0];
        assert_eq!(socket.read(&mut one).await.unwrap_or(0), 0);
        let mut r = request(&inbox.pairing_code().await.unwrap(), 0);
        r.version = 99;
        assert_eq!(exchange(&endpoint, r).await.status, "unsupported_version");
        assert!(inbox.take_next().await.is_none());
        inbox.shutdown().await;
    }
}
