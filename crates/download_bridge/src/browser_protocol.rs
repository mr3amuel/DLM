//! Browser/app-shell protocol. Deliberately independent of download_core and FRB.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
pub const VERSION: u32 = 1;
pub const MAX_FRAME: usize = 16 * 1024;
pub const HOST_NAME: &str = "com.dlm.browser";
pub const FIREFOX_ID: &str = "browser@dlm.local";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Request {
    pub version: u32,
    pub request_id: String,
    pub operation: String,
    pub pairing_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}
impl Request {
    pub fn validate(&mut self) -> Result<(), &'static str> {
        if self.version != VERSION {
            return Err("unsupported_version");
        }
        if self.request_id.len() != 36
            || !self.request_id.bytes().enumerate().all(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_hexdigit()
                }
            })
        {
            return Err("invalid_request");
        }
        if self.pairing_token.len() != 64
            || !self
                .pairing_token
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err("unpaired");
        }
        match self.operation.as_str() {
            "ping" if self.url.is_none() && self.filename.is_none() => Ok(()),
            "review" => {
                let input = self.url.as_ref().ok_or("invalid_url")?;
                if input.len() > 8192 || input.chars().any(|c| c.is_whitespace() || c.is_control())
                {
                    return Err("invalid_url");
                }
                let mut url = url::Url::parse(input).map_err(|_| "invalid_url")?;
                if !matches!(url.scheme(), "http" | "https")
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                {
                    return Err("invalid_url");
                }
                url.set_fragment(None);
                let normalized = url.to_string();
                if normalized.len() > 8192 {
                    return Err("invalid_url");
                }
                self.url = Some(normalized);
                if let Some(name) = &self.filename {
                    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
                    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                        || (stem.len() == 4
                            && (stem.starts_with("COM") || stem.starts_with("LPT"))
                            && matches!(stem.as_bytes()[3], b'1'..=b'9'));
                    if reserved
                        || name.is_empty()
                        || name.len() > 255
                        || name.trim() != name
                        || name.contains(['/', '\\', ':', '\0', '<', '>', '"', '|', '?', '*'])
                        || name.chars().any(char::is_control)
                        || name.ends_with(['.', ' '])
                        || matches!(name.as_str(), "." | "..")
                    {
                        return Err("invalid_filename");
                    }
                }
                Ok(())
            }
            _ => Err("invalid_request"),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Response {
    pub version: u32,
    pub request_id: Option<String>,
    pub status: String,
}
impl Response {
    pub fn new(id: Option<String>, status: &str) -> Self {
        Self {
            version: VERSION,
            request_id: id,
            status: status.into(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub version: u32,
    pub enabled: bool,
    pub port: Option<u16>,
    pub instance: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub instance: String,
    pub request: Request,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Challenge {
    pub version: u32,
    pub instance: String,
    pub nonce: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerProof {
    pub version: u32,
    pub status: String,
    pub proof: Option<String>,
}
pub fn proof(
    token: &str,
    kind: &str,
    instance: &str,
    nonce: &str,
    request: Option<&Request>,
) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(token.as_bytes()).expect("HMAC accepts any key length");
    for part in ["DLM-BROWSER-V1", kind, instance, nonce] {
        mac.update(part.as_bytes());
        mac.update(&[0]);
    }
    if let Some(request) = request {
        let mut public = request.clone();
        public.pairing_token.clear();
        mac.update(&serde_json::to_vec(&public).expect("fixed string fields serialize"));
    }
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn matches_proof(actual: &str, expected: &str) -> bool {
    use subtle::ConstantTimeEq;
    actual.len() == 64 && bool::from(actual.as_bytes().ct_eq(expected.as_bytes()))
}

pub fn random_hex(bytes: usize) -> Result<String, &'static str> {
    let mut raw = vec![0; bytes];
    getrandom::getrandom(&mut raw).map_err(|_| "random_unavailable")?;
    Ok(raw.iter().map(|b| format!("{b:02x}")).collect())
}
pub fn read_frame(mut reader: impl Read) -> Result<Vec<u8>, &'static str> {
    let mut len = [0; 4];
    reader.read_exact(&mut len).map_err(|_| "invalid_frame")?;
    let len = u32::from_ne_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err("invalid_frame");
    }
    let mut bytes = vec![0; len];
    reader.read_exact(&mut bytes).map_err(|_| "invalid_frame")?;
    Ok(bytes)
}
pub fn write_frame(mut writer: impl Write, value: &impl Serialize) -> Result<(), &'static str> {
    let bytes = serde_json::to_vec(value).map_err(|_| "invalid_frame")?;
    if bytes.len() > MAX_FRAME {
        return Err("invalid_frame");
    }
    writer
        .write_all(&(bytes.len() as u32).to_ne_bytes())
        .and_then(|_| writer.write_all(&bytes))
        .and_then(|_| writer.flush())
        .map_err(|_| "unavailable")
}
pub fn private_dir(path: &Path) -> Result<(), &'static str> {
    if !path.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(path).map_err(|_| "storage_unavailable")?;
    }
    let m = fs::symlink_metadata(path).map_err(|_| "storage_unavailable")?;
    if !m.is_dir() || m.file_type().is_symlink() {
        return Err("unsafe_storage");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
            return Err("unsafe_storage");
        }
    }
    Ok(())
}
pub fn read_private<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, &'static str> {
    let m = fs::symlink_metadata(path).map_err(|_| "unavailable")?;
    if !m.is_file() || m.file_type().is_symlink() || m.len() > MAX_FRAME as u64 {
        return Err("unsafe_storage");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
            return Err("unsafe_storage");
        }
    }
    serde_json::from_slice(&fs::read(path).map_err(|_| "storage_unavailable")?)
        .map_err(|_| "outdated_app")
}
pub fn write_private(path: &Path, value: &impl Serialize) -> Result<(), &'static str> {
    let parent = path.parent().ok_or("unsafe_storage")?;
    private_dir(parent)?;
    let tmp = parent.join(format!(".dlm-{}.tmp", random_hex(12)?));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut f = options.open(&tmp).map_err(|_| "storage_unavailable")?;
        serde_json::to_writer(&mut f, value).map_err(|_| "storage_unavailable")?;
        f.sync_all().map_err(|_| "storage_unavailable")?;
        fs::rename(&tmp, path).map_err(|_| "storage_unavailable")?;
        #[cfg(unix)]
        fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|_| "storage_unavailable")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}
pub fn endpoint_path(dir: &Path) -> PathBuf {
    dir.join("endpoint.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Request {
        Request {
            version: 1,
            request_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into(),
            operation: "review".into(),
            pairing_token: "a".repeat(64),
            url: Some("https://example.org/file?token=private#fragment".into()),
            filename: None,
        }
    }
    #[test]
    fn strict_url_and_filename_validation_never_accepts_privileged_fields() {
        let mut r = request();
        r.validate().unwrap();
        assert_eq!(
            r.url.as_deref(),
            Some("https://example.org/file?token=private")
        );
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/plain,secret",
            "https://user:password@example.org/file",
            "https://example.org/a b",
            "https://example.org/\nfile",
        ] {
            let mut r = request();
            r.url = Some(url.into());
            assert!(r.validate().is_err());
        }
        for name in ["../file", "CON.txt", "a\\b", "a:b", "file.", "a\nfile"] {
            let mut r = request();
            r.filename = Some(name.into());
            assert_eq!(r.validate(), Err("invalid_filename"));
        }
        for key in [
            "cookies",
            "headers",
            "authorization",
            "destination",
            "autoStart",
            "command",
        ] {
            let mut v = serde_json::to_value(request()).unwrap();
            v[key] = serde_json::json!(true);
            assert!(serde_json::from_value::<Request>(v).is_err());
        }
        let mut r = request();
        r.url = Some("a".repeat(8193));
        assert_eq!(r.validate(), Err("invalid_url"));
    }
    #[test]
    fn version_auth_and_operations_are_explicit() {
        let mut r = request();
        r.version = 2;
        assert_eq!(r.validate(), Err("unsupported_version"));
        r = request();
        r.operation = "install".into();
        assert_eq!(r.validate(), Err("invalid_request"));
        r = request();
        r.pairing_token = "A".repeat(64);
        assert_eq!(r.validate(), Err("unpaired"));
        r = request();
        r.operation = "ping".into();
        assert_eq!(r.validate(), Err("invalid_request"));
        r.url = None;
        assert!(r.validate().is_ok());
        let token = "a".repeat(64);
        let original = proof(&token, "client", "instance", "nonce", Some(&r));
        assert!(matches_proof(&original, &original));
        assert!(!matches_proof("", &original));
        for changed in [
            proof(&token, "server", "instance", "nonce", Some(&r)),
            proof(&token, "client", "other", "nonce", Some(&r)),
            proof(&token, "client", "instance", "other", Some(&r)),
            proof(&"b".repeat(64), "client", "instance", "nonce", Some(&r)),
        ] {
            assert!(!matches_proof(&changed, &original));
        }
        r.request_id = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".into();
        assert!(!matches_proof(
            &proof(&token, "client", "instance", "nonce", Some(&r)),
            &original,
        ));
    }
    #[test]
    fn framing_bounds_truncation_and_native_byte_order() {
        for n in [0u32, MAX_FRAME as u32 + 1, u32::MAX] {
            assert_eq!(read_frame(n.to_ne_bytes().as_slice()), Err("invalid_frame"));
        }
        assert!(read_frame([0, 0].as_slice()).is_err());
        let mut truncated = 10u32.to_ne_bytes().to_vec();
        truncated.push(1);
        assert!(read_frame(truncated.as_slice()).is_err());
        let mut bytes = vec![];
        write_frame(&mut bytes, &request()).unwrap();
        assert_eq!(
            u32::from_ne_bytes(bytes[..4].try_into().unwrap()) as usize,
            bytes.len() - 4
        );
        let decoded: Request =
            serde_json::from_slice(&read_frame(bytes.as_slice()).unwrap()).unwrap();
        assert_eq!(decoded.request_id, request().request_id);
    }
    #[test]
    fn private_storage_rejects_symlinks_and_world_readable_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("secure/private.json");
        write_private(&path, &serde_json::json!({"v":1})).unwrap();
        assert_eq!(read_private::<serde_json::Value>(&path).unwrap()["v"], 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(
                read_private::<serde_json::Value>(&path).unwrap_err(),
                "unsafe_storage"
            );
            fs::remove_file(&path).unwrap();
            let outside = root.path().join("outside");
            fs::write(&outside, b"keep").unwrap();
            std::os::unix::fs::symlink(&outside, &path).unwrap();
            assert_eq!(
                read_private::<serde_json::Value>(&path).unwrap_err(),
                "unsafe_storage"
            );
            assert_eq!(fs::read(outside).unwrap(), b"keep");
        }
    }
}
