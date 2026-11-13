//! macOS Chrome wizard. Uses the legacy host's validation and authenticated relay.
use crate::{
    browser_host::{
        Installation, caller_allowed, chrome_id, deliver, validate_installation, write_manifest,
    },
    browser_protocol::{
        Request, Response, private_dir, random_hex, read_frame, read_private, write_private,
    },
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub(crate) const HOST: &str = if cfg!(feature = "native-production") {
    "com.dlm.native.chrome"
} else if cfg!(feature = "native-chrome") {
    "com.dlm.native_development.chrome"
} else {
    "com.dlm.chrome"
};
pub(crate) const HELPER: &str = if cfg!(feature = "native-chrome") {
    "dlm-native-chrome-host"
} else {
    "dlm-chrome-host"
};
pub(crate) const APP_EXECUTABLE: &str = if cfg!(feature = "native-chrome") {
    "DLMSwiftExperiment"
} else {
    "DLM"
};
pub(crate) const NATIVE_BUNDLE: &str = if cfg!(feature = "native-production") {
    "com.dlm.native"
} else {
    "com.dlm.native-development"
};
pub(crate) const NATIVE_FIREFOX: &str = if cfg!(feature = "native-production") {
    "browser-production@dlm.local"
} else {
    "browser-native@dlm.local"
};
const CONFIG: &str = "installation.json";
const RECEIPT: &str = "connection.json";
const EXTENSION: &str = "DLM Chrome Extension";
const EXTENSION_FILES: &[&str] = &[
    "protocol.js",
    "background.js",
    "options.js",
    "options.css",
    "options.html",
    "_locales/en/messages.json",
    "_locales/fa/messages.json",
    "manifest.json",
];
type Result<T> = std::result::Result<T, &'static str>;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registration {
    installation: Installation,
    attempt: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    previous_apps: Vec<PathBuf>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    attempt: String,
    status: String,
}

fn root(home: &Path) -> PathBuf {
    home.join("Library/Application Support")
        .join(if cfg!(feature = "native-production") {
            "DLM Native Chrome"
        } else if cfg!(feature = "native-chrome") {
            "DLM Native Development Chrome"
        } else {
            "DLM Chrome"
        })
}

pub(crate) fn manifest_for(home: &Path, browser: &str) -> Result<PathBuf> {
    let directory = match browser {
        "chrome" => "Google/Chrome/NativeMessagingHosts",
        "edge" if cfg!(feature = "native-chrome") => "Microsoft Edge/NativeMessagingHosts",
        "firefox" if cfg!(feature = "native-chrome") => "Mozilla/NativeMessagingHosts",
        _ => return Err("invalid_browser"),
    };
    Ok(home
        .join("Library/Application Support")
        .join(directory)
        .join(format!("{HOST}.json")))
}
#[cfg(test)]
fn manifest(home: &Path) -> PathBuf {
    manifest_for(home, "chrome").expect("fixed browser")
}

fn packaged_helper(app: &Path) -> PathBuf {
    app.join("Contents/Helpers").join(HELPER)
}

fn check_manifest_for(home: &Path, candidate: Option<&Path>, browser: &str) -> Result<()> {
    let path = manifest_for(home, browser)?;
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(m) if m.is_file() && m.len() <= 16384 => {
            let bytes = fs::read(path).map_err(|_| "storage_unavailable")?;
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| "unsafe_storage")?;
            let installed = value["path"].as_str().ok_or("unsafe_storage")?;
            let same = |path: PathBuf| path.to_str() == Some(installed);
            let legacy = same(root(home).join(HELPER));
            let packaged = cfg!(feature = "native-chrome")
                && (candidate.is_some_and(|app| same(packaged_helper(app)))
                    || read_private::<Registration>(&root(home).join(CONFIG)).is_ok_and(
                        |current| {
                            same(packaged_helper(&current.installation.app))
                                || current
                                    .previous_apps
                                    .iter()
                                    .any(|app| same(packaged_helper(app)))
                        },
                    ));
            if value["name"] == HOST && (legacy || packaged) {
                Ok(())
            } else {
                Err("unsafe_storage")
            }
        }
        _ => Err("unsafe_storage"),
    }
}

fn same_file_contents(source: &Path, destination: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(destination) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    let Ok(original) = fs::metadata(source) else {
        return false;
    };
    if metadata.len() != original.len() || metadata.permissions() != original.permissions() {
        return false;
    }
    fs::read(source).is_ok_and(|bytes| fs::read(destination).is_ok_and(|saved| saved == bytes))
}

fn copy_file(source: &Path, destination: &Path) -> Result<()> {
    let parent = destination.parent().ok_or("unsafe_storage")?;
    private_dir(parent)?;
    if same_file_contents(source, destination) {
        return Ok(());
    }
    let staging = parent.join(format!(".dlm-{}.tmp", random_hex(12)?));
    let result = (|| {
        fs::copy(source, &staging).map_err(|_| "storage_unavailable")?;
        fs::rename(&staging, destination).map_err(|_| "storage_unavailable")
    })();
    if result.is_err() {
        let _ = fs::remove_file(staging);
    }
    result
}

fn lock(root: &Path) -> Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(root.join("setup.lock"))
        .map_err(|_| "storage_unavailable")?;
    FileExt::lock_exclusive(&file).map_err(|_| "storage_unavailable")?;
    Ok(file)
}

fn prepare(home: &Path, app: &Path) -> Result<PathBuf> {
    let source = app.join("Contents/Resources/DLMChromeExtension");
    let destination = root(home).join(EXTENSION);
    private_dir(&root(home))?;
    let _lock = lock(&root(home))?;
    private_dir(&destination)?;
    private_dir(&destination.join("_locales"))?;
    // Manifest last, so a partially staged first install can't be loaded.
    for name in EXTENSION_FILES {
        copy_file(&source.join(name), &destination.join(name))?;
    }
    Ok(destination)
}

fn register(home: &Path, app: &Path, inbox: &Path, id: &str) -> Result<String> {
    register_browser(home, app, inbox, id, "chrome")
}
fn register_browser(
    home: &Path,
    app: &Path,
    inbox: &Path,
    id: &str,
    browser: &str,
) -> Result<String> {
    let path = manifest_for(home, browser)?;
    let valid = if browser == "firefox" {
        id == NATIVE_FIREFOX
    } else {
        chrome_id(id)
    };
    if !valid || !app.join("Contents/MacOS").join(APP_EXECUTABLE).is_file() {
        return Err("invalid_installation");
    }
    let mut installation = Installation {
        app: fs::canonicalize(app).map_err(|_| "invalid_installation")?,
        inbox: inbox.into(),
        callers: vec![id.into()],
    };
    validate_installation(&installation)?;
    private_dir(inbox)?;
    let root = root(home);
    private_dir(&root)?;
    let _lock = lock(&root)?;
    check_manifest_for(home, Some(&installation.app), browser)?;
    // Native manifests share only their configuration-specific inbox and identity.
    let mut previous_apps = Vec::new();
    if cfg!(feature = "native-chrome")
        && let Ok(old) = read_private::<Registration>(&root.join(CONFIG))
    {
        previous_apps = old.previous_apps;
        if old.installation.app != installation.app
            && !previous_apps.contains(&old.installation.app)
        {
            previous_apps.push(old.installation.app);
        }
        if previous_apps.len() > 32 {
            return Err("invalid_installation");
        }
        for caller in old.installation.callers {
            if !installation.callers.contains(&caller) {
                installation.callers.push(caller);
            }
        }
    }
    validate_installation(&installation)?;
    let executable = if cfg!(feature = "native-chrome") {
        // App Sandbox quarantines executables copied out of the signed bundle.
        // Chrome launches the signed, bundled helper directly in this profile.
        let helper = packaged_helper(&installation.app);
        if !fs::symlink_metadata(&helper).is_ok_and(|metadata| metadata.is_file()) {
            return Err("invalid_installation");
        }
        helper
    } else {
        let helper = root.join(HELPER);
        copy_file(&packaged_helper(app), &helper)?;
        helper
    };
    let attempt = random_hex(16)?;
    write_private(
        &root.join(CONFIG),
        &Registration {
            installation,
            attempt: attempt.clone(),
            previous_apps,
        },
    )?;
    let desired = if browser == "firefox" {
        serde_json::json!({"name": HOST, "description": "DLM native review host", "type": "stdio",
            "path": executable, "allowed_extensions": [NATIVE_FIREFOX]})
    } else {
        serde_json::json!({"name": HOST, "description": "DLM Chrome review host", "type": "stdio",
            "path": executable, "allowed_origins": [format!("chrome-extension://{id}/")]})
    };
    let registered = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    if registered.as_ref() != Some(&desired) {
        write_manifest(&path, &desired)?;
    }
    Ok(attempt)
}

fn status(home: &Path, attempt: &str) -> String {
    let root = root(home);
    if private_dir_if_present(&root).is_ok()
        && let Ok(config) = read_private::<Registration>(&root.join(CONFIG))
        && config.attempt == attempt
        && let Ok(receipt) = read_private::<Receipt>(&root.join(RECEIPT))
        && receipt.attempt == attempt
    {
        return receipt.status;
    }
    "pending".into()
}

fn private_dir_if_present(path: &Path) -> Result<()> {
    if !path.exists() {
        return Err("unavailable");
    }
    private_dir(path)
}

fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("storage_unavailable"),
    }
}

fn remove(home: &Path) -> Result<()> {
    let root = root(home);
    let _lock = if root.exists() {
        private_dir(&root)?;
        Some(lock(&root)?)
    } else {
        None
    };
    let browsers = if cfg!(feature = "native-chrome") {
        &["chrome", "edge", "firefox"][..]
    } else {
        &["chrome"][..]
    };
    // Validate every target before removing any registration.
    for browser in browsers {
        check_manifest_for(home, None, browser)?;
    }
    for browser in browsers {
        remove_file(&manifest_for(home, browser)?)?;
    }
    for name in [CONFIG, RECEIPT, HELPER] {
        remove_file(&root.join(name))?;
    }
    Ok(())
}

pub fn command(home: &Path, args: &[String]) -> Result<String> {
    match args {
        [action, app] if action == "prepare" => prepare(home, Path::new(app))?
            .into_os_string()
            .into_string()
            .map_err(|_| "invalid_installation"),
        [action, app, inbox, id] if action == "register" => {
            register(home, Path::new(app), Path::new(inbox), id)
        }
        [action, browser, app, inbox, id]
            if action == "register-browser" && cfg!(feature = "native-chrome") =>
        {
            register_browser(home, Path::new(app), Path::new(inbox), id, browser)
        }
        [action, browser, app, inbox, id]
            if action == "inspect" && cfg!(feature = "native-chrome") =>
        {
            let path = manifest_for(home, browser)?;
            let state = if check_manifest_for(home, Some(Path::new(app)), browser).is_err() {
                "unsafe"
            } else if !path.exists() {
                "missing"
            } else if let Ok(config) = read_private::<Registration>(&root(home).join(CONFIG)) {
                let registered = fs::read(&path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
                if config.installation.app
                    == fs::canonicalize(app).map_err(|_| "invalid_installation")?
                    && config.installation.inbox == Path::new(inbox)
                    && config.installation.callers.iter().any(|v| v == id)
                    && registered.as_ref().and_then(|v| v["path"].as_str())
                        == packaged_helper(
                            &fs::canonicalize(app).map_err(|_| "invalid_installation")?,
                        )
                        .to_str()
                    && registered.as_ref().is_some_and(|v| {
                        if browser == "firefox" {
                            v["allowed_extensions"] == serde_json::json!([id])
                        } else {
                            v["allowed_origins"]
                                == serde_json::json!([format!("chrome-extension://{id}/")])
                        }
                    })
                {
                    "current"
                } else {
                    "stale"
                }
            } else {
                "stale"
            };
            serde_json::to_string(&serde_json::json!({"registration":state, "folder":root(home).join(EXTENSION).is_dir(),
                "directory":root(home).join(EXTENSION).is_dir().then(|| root(home).join(EXTENSION))})).map_err(|_| "unavailable")
        }
        [action, attempt] if action == "status" => Ok(status(home, attempt)),
        [action] if action == "remove" => remove(home).map(|()| String::new()),
        _ => Err("invalid_request"),
    }
}

pub fn process(home: &Path, args: &[String], input: impl io::Read) -> Response {
    let result = (|| {
        let root = root(home);
        private_dir_if_present(&root)?;
        let registration: Registration = read_private(&root.join(CONFIG))?;
        let installation = &registration.installation;
        validate_installation(installation)?;
        if installation.callers.is_empty()
            || installation.callers.len() > 8
            || installation.callers.iter().any(|id| {
                !(chrome_id(id) || (cfg!(feature = "native-chrome") && id == NATIVE_FIREFOX))
            })
            || !caller_allowed(args, installation, home)
        {
            return Err("unauthorized_extension");
        }
        let mut request: Request =
            serde_json::from_slice(&read_frame(input)?).map_err(|_| "invalid_request")?;
        request.validate()?;
        let ping = request.operation == "ping";
        let response = deliver(installation, request, !ping);
        if ping {
            let _lock = lock(&root)?;
            let config: Registration = read_private(&root.join(CONFIG))?;
            if config.attempt != registration.attempt {
                return response;
            }
            if !cfg!(feature = "native-chrome")
                && read_private::<Receipt>(&root.join(RECEIPT)).is_ok_and(|receipt| {
                    receipt.attempt == config.attempt && receipt.status == "ready"
                })
            {
                return response;
            }
            let status = match &response {
                Ok(response) if response.status == "ready" => "ready",
                _ => "failed",
            };
            // Only this registered caller's authenticated ping can create a ready receipt.
            write_private(
                &root.join(RECEIPT),
                &Receipt {
                    attempt: registration.attempt,
                    status: status.into(),
                },
            )?;
        }
        response
    })();
    result.unwrap_or_else(|error| Response::new(None, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_protocol::{
        Challenge, Endpoint, Envelope, ServerProof, endpoint_path, matches_proof, proof,
        write_frame,
    };
    use std::{net::TcpListener, thread, time::Duration};

    const ID: &str = "abcdefghijklmnopabcdefghijklmnop";

    #[cfg(feature = "native-chrome")]
    #[test]
    fn identity_storage_and_manifest_paths_are_configuration_scoped() {
        let home = Path::new("/disposable/home");
        let production = cfg!(feature = "native-production");
        assert_eq!(
            NATIVE_BUNDLE,
            if production {
                "com.dlm.native"
            } else {
                "com.dlm.native-development"
            }
        );
        assert_eq!(
            HOST,
            if production {
                "com.dlm.native.chrome"
            } else {
                "com.dlm.native_development.chrome"
            }
        );
        assert_eq!(
            root(home),
            home.join("Library/Application Support")
                .join(if production {
                    "DLM Native Chrome"
                } else {
                    "DLM Native Development Chrome"
                })
        );
        for browser in ["chrome", "edge", "firefox"] {
            let path = manifest_for(home, browser).unwrap();
            assert_eq!(path.file_name().unwrap(), format!("{HOST}.json").as_str());
        }
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("First Location/DLM.app");
        let executable = app.join("Contents/MacOS").join(APP_EXECUTABLE);
        let helper = app.join("Contents/Helpers").join(HELPER);
        for file in [executable, helper] {
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, "fixture").unwrap();
        }
        let inbox = dir.path().join("browser-integration");
        private_dir(&inbox).unwrap();
        (dir, app, inbox)
    }

    #[cfg(feature = "native-chrome")]
    #[test]
    fn native_profiles_repair_relocation_without_touching_production_registrations() {
        let (dir, app, inbox) = fixture();
        let home = dir.path();
        let production = home.join(
            "Library/Application Support/Google/Chrome/NativeMessagingHosts/com.dlm.chrome.json",
        );
        fs::create_dir_all(production.parent().unwrap()).unwrap();
        fs::write(&production, "production sentinel").unwrap();
        let args = |browser: &str, app: &Path, id: &str| {
            vec![
                "inspect".into(),
                browser.into(),
                app.to_string_lossy().into_owned(),
                inbox.to_string_lossy().into_owned(),
                id.into(),
            ]
        };
        assert!(
            command(home, &args("chrome", &app, ID))
                .unwrap()
                .contains("missing")
        );
        register_browser(home, &app, &inbox, ID, "chrome").unwrap();
        register_browser(home, &app, &inbox, ID, "edge").unwrap();
        register_browser(home, &app, &inbox, NATIVE_FIREFOX, "firefox").unwrap();
        for (browser, id) in [("chrome", ID), ("edge", ID), ("firefox", NATIVE_FIREFOX)] {
            let inspected = command(home, &args(browser, &app, id)).unwrap();
            assert!(inspected.contains("current"), "{browser}: {inspected}");
        }
        let edge_id = "b".repeat(32);
        register_browser(home, &app, &inbox, &edge_id, "edge").unwrap();
        let config: Registration = read_private(&root(home).join(CONFIG)).unwrap();
        assert!(config.installation.callers.iter().any(|id| id == ID));
        assert!(config.installation.callers.contains(&edge_id));
        assert!(caller_allowed(
            &[
                manifest_for(home, "firefox")
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                NATIVE_FIREFOX.into()
            ],
            &config.installation,
            home
        ));
        assert!(!caller_allowed(
            &["/wrong".into(), NATIVE_FIREFOX.into()],
            &config.installation,
            home
        ));
        assert_eq!(
            process(
                home,
                &[
                    manifest_for(home, "firefox")
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    NATIVE_FIREFOX.into()
                ],
                &[][..]
            )
            .status,
            "invalid_frame"
        );
        assert_eq!(
            process(
                home,
                &["/production/manifest".into(), "browser@dlm.local".into()],
                &[][..]
            )
            .status,
            "unauthorized_extension"
        );
        let moved = home.join("Moved/DLM.app");
        fs::create_dir_all(moved.parent().unwrap()).unwrap();
        fs::rename(&app, &moved).unwrap();
        assert!(
            command(home, &args("chrome", &moved, ID))
                .unwrap()
                .contains("stale")
        );
        register_browser(home, &moved, &inbox, ID, "chrome").unwrap();
        // Other native manifests can still be removed safely via trusted prior app paths.
        remove(home).unwrap();
        assert_eq!(
            fs::read_to_string(production).unwrap(),
            "production sentinel"
        );
        for browser in ["chrome", "edge", "firefox"] {
            assert!(!manifest_for(home, browser).unwrap().exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn preparation_keeps_a_stable_folder_and_repairs_changed_resources() {
        use std::os::unix::fs::MetadataExt;
        let (dir, app, _) = fixture();
        let source = app.join("Contents/Resources/DLMChromeExtension");
        for name in EXTENSION_FILES {
            let file = source.join(name);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, name).unwrap();
        }
        let prepared = prepare(dir.path(), &app).unwrap();
        let protocol = prepared.join("protocol.js");
        let before = fs::metadata(&protocol).unwrap().ino();
        assert_eq!(prepare(dir.path(), &app).unwrap(), prepared);
        assert_eq!(fs::metadata(&protocol).unwrap().ino(), before);
        fs::write(source.join("protocol.js"), "updated").unwrap();
        assert_eq!(prepare(dir.path(), &app).unwrap(), prepared);
        assert_eq!(fs::read_to_string(protocol).unwrap(), "updated");
    }

    #[cfg(unix)]
    #[test]
    fn current_registration_reuses_the_helper_and_manifest_but_requires_a_fresh_ping() {
        use std::os::unix::fs::MetadataExt;
        let (dir, app, inbox) = fixture();
        let home = dir.path();
        let first = register(home, &app, &inbox, ID).unwrap();
        let helper = if cfg!(feature = "native-chrome") {
            packaged_helper(&app)
        } else {
            root(home).join(HELPER)
        };
        let manifest = manifest(home);
        let helper_inode = fs::metadata(&helper).unwrap().ino();
        let manifest_inode = fs::metadata(&manifest).unwrap().ino();
        let second = register(home, &app, &inbox, ID).unwrap();
        assert_ne!(first, second);
        assert_eq!(status(home, &second), "pending");
        assert_eq!(fs::metadata(helper).unwrap().ino(), helper_inode);
        assert_eq!(fs::metadata(manifest).unwrap().ino(), manifest_inode);
    }

    #[test]
    fn repairs_moved_app_and_removes_only_its_own_registration() {
        let (dir, app, inbox) = fixture();
        let home = dir.path();
        let original = register(home, &app, &inbox, ID).unwrap();
        let path = manifest(home);
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            value["allowed_origins"],
            serde_json::json!([format!("chrome-extension://{ID}/")])
        );
        let legacy = path.with_file_name("com.dlm.browser.json");
        fs::write(&legacy, "legacy").unwrap();

        let moved = home.join("New Location/DLM.app");
        fs::create_dir_all(moved.parent().unwrap()).unwrap();
        fs::rename(app, &moved).unwrap();
        let current = register(home, &moved, &inbox, ID).unwrap();
        assert_ne!(original, current);
        let registration: Registration = read_private(&root(home).join(CONFIG)).unwrap();
        assert_eq!(
            registration.installation.app,
            fs::canonicalize(moved).unwrap()
        );
        assert_eq!(registration.installation.inbox, inbox);
        if cfg!(feature = "native-chrome") {
            let value: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            assert_eq!(
                value["path"],
                packaged_helper(&registration.installation.app)
                    .to_str()
                    .unwrap()
            );
        }

        remove(home).unwrap();
        assert!(!path.exists());
        assert!(!root(home).join(HELPER).exists());
        assert_eq!(fs::read_to_string(legacy).unwrap(), "legacy");
        assert!(inbox.is_dir());
        remove(home).unwrap();
    }

    #[test]
    fn rejects_invalid_ids_and_foreign_registration() {
        let (dir, app, inbox) = fixture();
        let home = dir.path();
        assert!(register(home, &app, &inbox, "not-an-id").is_err());
        let path = manifest(home);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let foreign = r#"{"name":"com.dlm.chrome","path":"/other/helper"}"#;
        fs::write(&path, foreign).unwrap();
        assert!(register(home, &app, &inbox, ID).is_err());
        assert!(remove(home).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), foreign);
    }

    #[test]
    fn a_ready_receipt_requires_an_authorized_ping_and_is_scoped_to_the_attempt() {
        let (dir, app, inbox) = fixture();
        let home = dir.path();
        let attempt = register(home, &app, &inbox, ID).unwrap();
        assert_eq!(status(home, &attempt), "pending");
        assert_eq!(
            process(home, &["chrome-extension://wrong/".into()], &[][..]).status,
            "unauthorized_extension"
        );
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        write_private(
            &endpoint_path(&inbox),
            &Endpoint {
                version: 1,
                enabled: true,
                port: Some(listener.local_addr().unwrap().port()),
                instance: Some("b".repeat(32)),
            },
        )
        .unwrap();
        let token = "a".repeat(64);
        let server_token = token.clone();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let challenge: Challenge =
                serde_json::from_slice(&read_frame(&mut socket).unwrap()).unwrap();
            write_frame(
                &mut socket,
                &ServerProof {
                    version: 1,
                    status: "ready".into(),
                    proof: Some(proof(
                        &server_token,
                        "server",
                        &challenge.instance,
                        &challenge.nonce,
                        None,
                    )),
                },
            )
            .unwrap();
            let request = serde_json::from_slice::<Envelope>(&read_frame(&mut socket).unwrap())
                .unwrap()
                .request;
            assert!(matches_proof(
                &request.pairing_token,
                &proof(
                    &server_token,
                    "client",
                    &challenge.instance,
                    &challenge.nonce,
                    Some(&request)
                )
            ));
            assert_eq!(request.operation, "ping");
            assert!(request.url.is_none());
            write_frame(
                &mut socket,
                &Response::new(Some(request.request_id), "ready"),
            )
            .unwrap();
        });
        let mut frame = Vec::new();
        write_frame(
            &mut frame,
            &Request {
                version: 1,
                request_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into(),
                operation: "ping".into(),
                pairing_token: token,
                url: None,
                filename: None,
            },
        )
        .unwrap();
        assert_eq!(
            process(
                home,
                &[format!("chrome-extension://{ID}/")],
                frame.as_slice()
            )
            .status,
            "ready"
        );
        server.join().unwrap();
        assert_eq!(status(home, &attempt), "ready");
        if cfg!(feature = "native-chrome") {
            // A failed fresh test must replace a historical success in the candidate.
            assert_eq!(
                process(
                    home,
                    &[format!("chrome-extension://{ID}/")],
                    frame.as_slice()
                )
                .status,
                "app_unavailable"
            );
            assert_eq!(status(home, &attempt), "failed");
        }
        let retry = register(home, &app, &inbox, ID).unwrap();
        assert_eq!(status(home, &retry), "pending");
        assert_eq!(status(home, &attempt), "pending");
    }
}
