//! One Native Messaging request per process. CLI installation is never a message operation.
use crate::browser_protocol::*;
use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::{
    fs, io,
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Installation {
    pub app: PathBuf,
    pub inbox: PathBuf,
    pub callers: Vec<String>,
}
fn home() -> Result<PathBuf, &'static str> {
    #[cfg(unix)]
    {
        let mut account = unsafe { std::mem::zeroed::<libc::passwd>() };
        let mut result = std::ptr::null_mut();
        let mut bytes = vec![0u8; 65536];
        let status = unsafe {
            libc::getpwuid_r(
                libc::geteuid(),
                &mut account,
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                &mut result,
            )
        };
        if status != 0 || result.is_null() || account.pw_dir.is_null() {
            return Err("unavailable");
        }
        Ok(PathBuf::from(
            unsafe { std::ffi::CStr::from_ptr(account.pw_dir) }
                .to_string_lossy()
                .into_owned(),
        ))
    }
    #[cfg(not(unix))]
    {
        std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .ok_or("unsupported_platform")
    }
}
fn install_root(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/DLM/NativeMessaging")
    } else {
        home.join(".local/share/dlm/native-messaging")
    }
}
fn manifest_path(home: &Path, browser: &str) -> Result<PathBuf, &'static str> {
    let path = match browser {
        "chrome" => "Google/Chrome/NativeMessagingHosts",
        "edge" => "Microsoft Edge/NativeMessagingHosts",
        "firefox" => "Mozilla/NativeMessagingHosts",
        _ => return Err("invalid_browser"),
    };
    Ok(home
        .join("Library/Application Support")
        .join(path)
        .join(format!("{HOST_NAME}.json")))
}
pub(crate) fn chrome_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| (b'a'..=b'p').contains(&b))
}
pub(crate) fn validate_installation(c: &Installation) -> Result<(), &'static str> {
    if !c.app.is_absolute()
        || c.app.extension().and_then(|s| s.to_str()) != Some("app")
        || !c.inbox.is_absolute()
        || c.inbox.file_name().and_then(|s| s.to_str()) != Some("browser-integration")
        || c.callers.is_empty()
        || c.callers.len() > 8
        || c.callers.iter().any(|id| {
            id != FIREFOX_ID
                && !(cfg!(feature = "native-chrome") && id == crate::chrome_setup::NATIVE_FIREFOX)
                && !chrome_id(id)
        })
    {
        return Err("invalid_installation");
    }
    Ok(())
}
pub(crate) fn caller_allowed(args: &[String], config: &Installation, home: &Path) -> bool {
    if args.len() == 1
        && config.callers.iter().any(|id| {
            chrome_id(id)
                && (args[0] == format!("chrome-extension://{id}/")
                    || args[0] == format!("chrome-extension://{id}"))
        })
    {
        return true;
    }
    if cfg!(feature = "native-chrome")
        && args.len() == 2
        && args[1] == crate::chrome_setup::NATIVE_FIREFOX
    {
        return config
            .callers
            .iter()
            .any(|id| id == crate::chrome_setup::NATIVE_FIREFOX)
            && crate::chrome_setup::manifest_for(home, "firefox")
                .is_ok_and(|p| args[0] == p.to_string_lossy());
    }
    args.len() == 2
        && args[1] == FIREFOX_ID
        && config.callers.iter().any(|id| id == FIREFOX_ID)
        && manifest_path(home, "firefox").is_ok_and(|p| args[0] == p.to_string_lossy())
}
fn launch(app: &Path) -> Result<(), &'static str> {
    #[cfg(target_os = "macos")]
    {
        if !app
            .join("Contents/MacOS")
            .join(crate::chrome_setup::APP_EXECUTABLE)
            .is_file()
        {
            return Err("app_unavailable");
        }
        if cfg!(feature = "native-chrome") {
            let identifier = Command::new("/usr/bin/plutil")
                .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
                .arg(app.join("Contents/Info.plist"))
                .output()
                .map_err(|_| "app_unavailable")?;
            if !identifier.status.success()
                || identifier.stdout
                    != format!("{}\n", crate::chrome_setup::NATIVE_BUNDLE).as_bytes()
            {
                return Err("invalid_installation");
            }
        }
        let protocol = Command::new("/usr/bin/plutil")
            .args(["-extract", "DLMBrowserProtocolVersion", "raw", "-o", "-"])
            .arg(app.join("Contents/Info.plist"))
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|_| "app_unavailable")?;
        if !protocol.status.success() || protocol.stdout != b"1\n" {
            return Err("outdated_app");
        }
        let status = Command::new("/usr/bin/open")
            .arg("-a")
            .arg(app)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|_| "app_unavailable")?;
        if !status.success() {
            return Err("app_unavailable");
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err("unsupported_platform")
    }
}
fn connect(endpoint: &Endpoint) -> Result<TcpStream, &'static str> {
    if endpoint.version != VERSION {
        return Err("outdated_app");
    }
    if !endpoint.enabled {
        return Err("disabled");
    }
    let port = endpoint.port.filter(|p| *p > 0).ok_or("outdated_app")?;
    let instance = endpoint.instance.as_ref().ok_or("outdated_app")?;
    if instance.len() != 32 || !instance.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("outdated_app");
    }
    let socket = TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(200),
    )
    .map_err(|_| "app_unavailable")?;
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|_| "app_unavailable")?;
    socket
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|_| "app_unavailable")?;
    Ok(socket)
}
pub(crate) fn deliver(
    config: &Installation,
    request: Request,
    can_launch: bool,
) -> Result<Response, &'static str> {
    deliver_with_launch(config, request, can_launch, launch)
}
fn deliver_with_launch(
    config: &Installation,
    request: Request,
    can_launch: bool,
    mut launcher: impl FnMut(&Path) -> Result<(), &'static str>,
) -> Result<Response, &'static str> {
    let path = endpoint_path(&config.inbox);
    let mut launched = false;
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        match read_private::<Endpoint>(&path) {
            Ok(endpoint) => match connect(&endpoint) {
                Ok(mut socket) => {
                    let instance = endpoint.instance.ok_or("outdated_app")?;
                    let nonce = random_hex(32)?;
                    write_frame(
                        &mut socket,
                        &Challenge {
                            version: VERSION,
                            instance: instance.clone(),
                            nonce: nonce.clone(),
                        },
                    )?;
                    let hello: ServerProof = serde_json::from_slice(&read_frame(&mut socket)?)
                        .map_err(|_| "outdated_app")?;
                    if hello.version != VERSION {
                        return Err("outdated_app");
                    }
                    if hello.status != "ready" {
                        return Err(
                            if ["disabled", "unpaired", "unavailable", "unsupported_version"]
                                .contains(&hello.status.as_str())
                            {
                                match hello.status.as_str() {
                                    "disabled" => "disabled",
                                    "unpaired" => "unpaired",
                                    "unsupported_version" => "unsupported_version",
                                    _ => "unavailable",
                                }
                            } else {
                                "outdated_app"
                            },
                        );
                    }
                    if !matches_proof(
                        hello.proof.as_deref().unwrap_or(""),
                        &proof(&request.pairing_token, "server", &instance, &nonce, None),
                    ) {
                        return Err("unpaired");
                    }
                    let mut authenticated = request.clone();
                    authenticated.pairing_token = proof(
                        &request.pairing_token,
                        "client",
                        &instance,
                        &nonce,
                        Some(&request),
                    );
                    write_frame(
                        &mut socket,
                        &Envelope {
                            instance,
                            request: authenticated,
                        },
                    )?;
                    let response: Response = serde_json::from_slice(&read_frame(&mut socket)?)
                        .map_err(|_| "outdated_app")?;
                    if response.version != VERSION
                        || response.request_id.as_ref() != Some(&request.request_id)
                        || ![
                            "ready",
                            "queued",
                            "already_queued",
                            "busy",
                            "disabled",
                            "unpaired",
                            "unavailable",
                            "invalid_request",
                            "invalid_url",
                            "invalid_filename",
                            "unsupported_version",
                        ]
                        .contains(&response.status.as_str())
                    {
                        return Err("outdated_app");
                    }
                    if can_launch
                        && request.operation == "review"
                        && matches!(response.status.as_str(), "queued" | "already_queued")
                    {
                        let _ = launcher(&config.app);
                    }
                    return Ok(response);
                }
                Err("app_unavailable") => {}
                Err(e) => return Err(e),
            },
            Err("unavailable") => {}
            Err(e) => return Err(e),
        }
        if !can_launch {
            return Err("app_unavailable");
        }
        if !launched {
            launcher(&config.app)?;
            launched = true;
        }
        if Instant::now() >= deadline {
            return Err("app_unavailable");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn process(args: &[String], home: &Path, input: impl io::Read, can_launch: bool) -> Response {
    let result = (|| {
        let root = install_root(home);
        private_dir(&root)?;
        let config: Installation = read_private(&root.join("installation.json"))?;
        validate_installation(&config)?;
        if !caller_allowed(args, &config, home) {
            return Err("unauthorized_extension");
        }
        let mut request: Request =
            serde_json::from_slice(&read_frame(input)?).map_err(|_| "invalid_request")?;
        request.validate()?;
        deliver(&config, request, can_launch)
    })();
    result.unwrap_or_else(|e| Response::new(None, e))
}
pub(crate) fn write_manifest(path: &Path, value: &serde_json::Value) -> Result<(), &'static str> {
    let parent = path.parent().ok_or("invalid_installation")?;
    fs::create_dir_all(parent).map_err(|_| "storage_unavailable")?;
    let tmp = parent.join(format!(".dlm-{}.tmp", random_hex(12)?));
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| "invalid_installation")?;
    let result = (|| {
        use io::Write;
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).map_err(|_| "storage_unavailable")?;
        f.write_all(&bytes)
            .and_then(|_| f.sync_all())
            .map_err(|_| "storage_unavailable")?;
        fs::rename(&tmp, path).map_err(|_| "storage_unavailable")
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}
fn install(home: &Path, args: &[String], executable: &Path) -> Result<(), &'static str> {
    if !args.len().is_multiple_of(2) {
        return Err("usage");
    }
    let mut flags = std::collections::BTreeMap::new();
    for pair in args.chunks(2) {
        if !["--browser", "--extension-id", "--app", "--inbox"].contains(&pair[0].as_str())
            || flags.insert(pair[0].as_str(), pair[1].as_str()).is_some()
        {
            return Err("usage");
        }
    }
    let browser = *flags.get("--browser").ok_or("usage")?;
    let id = *flags.get("--extension-id").ok_or("usage")?;
    let manifest = manifest_path(home, browser)?;
    if (browser == "firefox" && id != FIREFOX_ID) || (browser != "firefox" && !chrome_id(id)) {
        return Err("invalid_extension_id");
    }
    let app =
        fs::canonicalize(flags.get("--app").ok_or("usage")?).map_err(|_| "app_unavailable")?;
    if !app.join("Contents/MacOS/DLM").is_file() {
        return Err("app_unavailable");
    }
    let inbox=flags.get("--inbox").map(PathBuf::from).unwrap_or_else(||home.join("Library/Application Support").join(crate::chrome_setup::NATIVE_BUNDLE).join("browser-integration"));
    let root = install_root(home);
    private_dir(&root)?;
    let config_path = root.join("installation.json");
    let mut callers = if config_path.exists() {
        read_private::<Installation>(&config_path)?.callers
    } else {
        vec![]
    };
    if !callers.iter().any(|v| v == id) {
        callers.push(id.into());
    }
    let config = Installation {
        app,
        inbox,
        callers,
    };
    validate_installation(&config)?;
    let target = root.join("dlm-browser-host");
    if fs::canonicalize(executable).ok().as_ref() != fs::canonicalize(&target).ok().as_ref()
        || !target.exists()
    {
        let tmp = root.join(format!(".host-{}", random_hex(12)?));
        fs::copy(executable, &tmp).map_err(|_| "storage_unavailable")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o700))
                .map_err(|_| "storage_unavailable")?;
        }
        fs::rename(tmp, &target).map_err(|_| "storage_unavailable")?;
    }
    write_private(&config_path, &config)?;
    let value = if browser == "firefox" {
        serde_json::json!({"name":HOST_NAME,"description":"DLM browser review host","path":target,"type":"stdio","allowed_extensions":[FIREFOX_ID]})
    } else {
        serde_json::json!({"name":HOST_NAME,"description":"DLM browser review host","path":target,"type":"stdio","allowed_origins":config.callers.iter().filter(|id|chrome_id(id)).map(|id|format!("chrome-extension://{id}/")).collect::<Vec<_>>()})
    };
    write_manifest(&manifest, &value)
}
fn uninstall(home: &Path) -> Result<(), &'static str> {
    let root = install_root(home);
    let target = root.join("dlm-browser-host");
    for browser in ["chrome", "edge", "firefox"] {
        let path = manifest_path(home, browser)?;
        if path.exists() {
            let v: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).map_err(|_| "storage_unavailable")?)
                    .map_err(|_| "unsafe_storage")?;
            if v["name"] != HOST_NAME || v["path"].as_str() != target.to_str() {
                return Err("unsafe_storage");
            }
            fs::remove_file(path).map_err(|_| "storage_unavailable")?;
        }
    }
    for path in [root.join("installation.json"), target] {
        if path.exists() {
            fs::remove_file(path).map_err(|_| "storage_unavailable")?;
        }
    }
    Ok(())
}
pub fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.as_slice() == ["--help"] {
        eprintln!(
            "DLM Native Messaging host (protocol 1)\nInstall: dlm-browser-host install --browser chrome|edge|firefox --extension-id ID --app /Applications/DLM.app [--inbox PATH]\nRemove: dlm-browser-host uninstall\nSetup: docs/browser-integration.md"
        );
        return;
    }
    let Ok(home) = home() else {
        let _ = write_frame(io::stdout().lock(), &Response::new(None, "unavailable"));
        return;
    };
    if args.first().is_some_and(|s| s == "chrome-setup") {
        if !cfg!(target_os = "macos") {
            std::process::exit(1);
        }
        match crate::chrome_setup::command(&home, &args[1..]) {
            Ok(value) => println!("{value}"),
            Err(_) => std::process::exit(1),
        }
        return;
    }
    if args
        .first()
        .is_some_and(|s| s == "install" || s == "uninstall")
    {
        if !cfg!(target_os = "macos") || cfg!(feature = "native-chrome") {
            eprintln!("Legacy installation is unavailable in this build.");
            std::process::exit(1);
        }
        let result = if args[0] == "install" {
            std::env::current_exe()
                .map_err(|_| "unavailable")
                .and_then(|exe| install(&home, &args[1..], &exe))
        } else if args.len() == 1 {
            uninstall(&home)
        } else {
            Err("usage")
        };
        match result {
            Ok(()) => eprintln!("DLM native host registration updated."),
            Err(code) => {
                eprintln!("DLM native host: {code}. See docs/browser-integration.md.");
                std::process::exit(1);
            }
        }
        return;
    }
    let managed = std::env::current_exe().ok().is_some_and(|path| {
        path.file_name()
            .is_some_and(|name| name == crate::chrome_setup::HELPER)
    });
    let response = if managed || cfg!(feature = "native-chrome") {
        crate::chrome_setup::process(&home, &args, io::stdin().lock())
    } else {
        process(&args, &home, io::stdin().lock(), true)
    };
    let _ = write_frame(io::stdout().lock(), &response);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup(home: &Path) -> Installation {
        let root = install_root(home);
        private_dir(&root).unwrap();
        let config = Installation {
            app: home.join("DLM.app"),
            inbox: home.join("browser-integration"),
            callers: vec!["a".repeat(32), FIREFOX_ID.into()],
        };
        write_private(&root.join("installation.json"), &config).unwrap();
        private_dir(&config.inbox).unwrap();
        config
    }
    fn frame() -> Vec<u8> {
        let mut bytes = vec![];
        write_frame(
            &mut bytes,
            &Request {
                version: 1,
                request_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into(),
                operation: "review".into(),
                pairing_token: "a".repeat(64),
                url: Some("https://example.org/file?secret=private".into()),
                filename: None,
            },
        )
        .unwrap();
        bytes
    }
    #[test]
    fn closed_native_delivery_uses_only_the_supplied_launcher_and_authenticates_after_launch() {
        let temp = tempfile::tempdir().unwrap();
        let config = setup(temp.path());
        let request: Request =
            serde_json::from_slice(&read_frame(frame().as_slice()).unwrap()).unwrap();
        let mut launches = 0;
        let mut server = None;
        let result = deliver_with_launch(&config, request.clone(), true, |app| {
            assert_eq!(app, config.app);
            launches += 1;
            if launches == 1 {
                let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
                write_private(
                    &endpoint_path(&config.inbox),
                    &Endpoint {
                        version: 1,
                        enabled: true,
                        port: Some(listener.local_addr().unwrap().port()),
                        instance: Some("b".repeat(32)),
                    },
                )
                .unwrap();
                server = Some(std::thread::spawn(move || {
                    let (mut socket, _) = listener.accept().unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let challenge: Challenge =
                        serde_json::from_slice(&read_frame(&mut socket).unwrap()).unwrap();
                    write_frame(
                        &mut socket,
                        &ServerProof {
                            version: 1,
                            status: "ready".into(),
                            proof: Some(proof(
                                &"a".repeat(64),
                                "server",
                                &challenge.instance,
                                &challenge.nonce,
                                None,
                            )),
                        },
                    )
                    .unwrap();
                    let envelope: Envelope =
                        serde_json::from_slice(&read_frame(&mut socket).unwrap()).unwrap();
                    assert!(matches_proof(
                        &envelope.request.pairing_token,
                        &proof(
                            &"a".repeat(64),
                            "client",
                            &challenge.instance,
                            &challenge.nonce,
                            Some(&envelope.request)
                        )
                    ));
                    write_frame(
                        &mut socket,
                        &Response::new(Some(envelope.request.request_id), "queued"),
                    )
                    .unwrap();
                }));
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(result.status, "queued");
        assert_eq!(launches, 2); // Cold launch then foreground request, no shell execution.
        server.unwrap().join().unwrap();
        fs::remove_file(endpoint_path(&config.inbox)).unwrap();
        let mut calls = 0;
        assert_eq!(
            deliver_with_launch(&config, request.clone(), false, |_| {
                calls += 1;
                Ok(())
            })
            .unwrap_err(),
            "app_unavailable"
        );
        assert_eq!(calls, 0);
        assert_eq!(
            deliver_with_launch(&config, request, true, |_| Err("outdated_app")).unwrap_err(),
            "outdated_app"
        );
    }

    #[test]
    fn caller_identity_malformed_input_and_unavailable_app_never_launch_commands() {
        let temp = tempfile::tempdir().unwrap();
        let config = setup(temp.path());
        assert_eq!(
            process(
                &["chrome-extension://bad/".into()],
                temp.path(),
                frame().as_slice(),
                false
            )
            .status,
            "unauthorized_extension"
        );
        let caller = vec![format!("chrome-extension://{}/", "a".repeat(32))];
        assert_eq!(
            process(&caller, temp.path(), [1, 0].as_slice(), false).status,
            "invalid_frame"
        );
        assert_eq!(
            process(&caller, temp.path(), frame().as_slice(), false).status,
            "app_unavailable"
        );
        assert!(caller_allowed(
            &[
                manifest_path(temp.path(), "firefox")
                    .unwrap()
                    .to_string_lossy()
                    .into(),
                FIREFOX_ID.into()
            ],
            &config,
            temp.path()
        ));
        assert!(!caller_allowed(
            &["/wrong/manifest".into(), FIREFOX_ID.into()],
            &config,
            temp.path()
        ));
    }
    #[test]
    fn disabled_and_incompatible_endpoints_are_explicit() {
        let temp = tempfile::tempdir().unwrap();
        let config = setup(temp.path());
        let caller = vec![format!("chrome-extension://{}/", "a".repeat(32))];
        write_private(
            &endpoint_path(&config.inbox),
            &Endpoint {
                version: 1,
                enabled: false,
                port: None,
                instance: None,
            },
        )
        .unwrap();
        assert_eq!(
            process(&caller, temp.path(), frame().as_slice(), false).status,
            "disabled"
        );
        write_private(
            &endpoint_path(&config.inbox),
            &Endpoint {
                version: 2,
                enabled: false,
                port: None,
                instance: None,
            },
        )
        .unwrap();
        assert_eq!(
            process(&caller, temp.path(), frame().as_slice(), false).status,
            "outdated_app"
        );
    }
    #[test]
    fn native_stdio_request_is_relayed_only_to_authenticated_local_inbox() {
        let temp = tempfile::tempdir().unwrap();
        let config = setup(temp.path());
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        write_private(
            &endpoint_path(&config.inbox),
            &Endpoint {
                version: 1,
                enabled: true,
                port: Some(port),
                instance: Some("b".repeat(32)),
            },
        )
        .unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let challenge: Challenge =
                serde_json::from_slice(&read_frame(&mut socket).unwrap()).unwrap();
            write_frame(
                &mut socket,
                &ServerProof {
                    version: 1,
                    status: "ready".into(),
                    proof: Some(proof(
                        &"a".repeat(64),
                        "server",
                        &challenge.instance,
                        &challenge.nonce,
                        None,
                    )),
                },
            )
            .unwrap();
            let envelope: Envelope =
                serde_json::from_slice(&read_frame(&mut socket).unwrap()).unwrap();
            assert_eq!(envelope.instance, "b".repeat(32));
            assert_ne!(envelope.request.pairing_token, "a".repeat(64));
            assert_eq!(
                envelope.request.url.as_deref(),
                Some("https://example.org/file?secret=private")
            );
            write_frame(
                &mut socket,
                &Response::new(Some(envelope.request.request_id), "queued"),
            )
            .unwrap();
        });
        let result = process(
            &[format!("chrome-extension://{}/", "a".repeat(32))],
            temp.path(),
            frame().as_slice(),
            false,
        );
        assert_eq!(result.status, "queued");
        server.join().unwrap();
    }
    #[test]
    fn stale_endpoint_cannot_steal_the_pairing_secret_or_url() {
        let temp = tempfile::tempdir().unwrap();
        let config = setup(temp.path());
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        write_private(
            &endpoint_path(&config.inbox),
            &Endpoint {
                version: 1,
                enabled: true,
                port: Some(listener.local_addr().unwrap().port()),
                instance: Some("b".repeat(32)),
            },
        )
        .unwrap();
        let server = std::thread::spawn(move || {
            use std::io::Read;
            let (mut socket, _) = listener.accept().unwrap();
            let first = read_frame(&mut socket).unwrap();
            let text = String::from_utf8(first).unwrap();
            assert!(!text.contains("private"));
            assert!(!text.contains(&"a".repeat(64)));
            write_frame(
                &mut socket,
                &ServerProof {
                    version: 1,
                    status: "ready".into(),
                    proof: Some("0".repeat(64)),
                },
            )
            .unwrap();
            let mut rest = [0];
            assert_eq!(socket.read(&mut rest).unwrap(), 0);
        });
        assert_eq!(
            process(
                &[format!("chrome-extension://{}/", "a".repeat(32))],
                temp.path(),
                frame().as_slice(),
                false
            )
            .status,
            "unpaired"
        );
        server.join().unwrap();
    }
    #[test]
    fn per_browser_registration_is_isolated_and_uninstall_preserves_other_files() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("DLM.app");
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::write(app.join("Contents/MacOS/DLM"), b"test fixture").unwrap();
        let executable = temp.path().join("source-host");
        fs::write(&executable, b"host fixture").unwrap();
        for (browser, id) in [
            ("chrome", "a".repeat(32)),
            ("edge", "b".repeat(32)),
            ("firefox", FIREFOX_ID.into()),
        ] {
            install(
                temp.path(),
                &[
                    "--browser".into(),
                    browser.into(),
                    "--extension-id".into(),
                    id,
                    "--app".into(),
                    app.to_string_lossy().into(),
                ],
                &executable,
            )
            .unwrap();
            let v: serde_json::Value = serde_json::from_slice(
                &fs::read(manifest_path(temp.path(), browser).unwrap()).unwrap(),
            )
            .unwrap();
            assert_eq!(v["name"], HOST_NAME);
            assert!(if browser == "firefox" {
                v["allowed_extensions"].is_array()
            } else {
                v["allowed_origins"].is_array()
            });
        }
        let other = manifest_path(temp.path(), "chrome")
            .unwrap()
            .parent()
            .unwrap()
            .join("other-host.json");
        fs::write(&other, b"keep").unwrap();
        uninstall(temp.path()).unwrap();
        assert_eq!(fs::read(other).unwrap(), b"keep");
        assert!(app.exists());
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn outdated_app_is_detected_before_launch() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("Old.app");
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::write(
            app.join("Contents/MacOS")
                .join(crate::chrome_setup::APP_EXECUTABLE),
            b"unused",
        )
        .unwrap();
        fs::write(
            app.join("Contents/Info.plist"),
            format!("<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>{}</string></dict></plist>", crate::chrome_setup::NATIVE_BUNDLE),
        )
        .unwrap();
        assert_eq!(launch(&app), Err("outdated_app"));
    }
}
