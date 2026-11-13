//! Portable classification, destination planning and metadata-only organization.
use crate::{CreationSettings, Download, Engine, Error, Result, ResumeCapability, Status};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    pub id: String,
    pub name: String,
    pub directory: Option<String>,
    pub builtin: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganizationConfig {
    pub enabled: bool,
    pub categories: Vec<Category>,
}
impl Default for OrganizationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            categories: [
                ("videos", "Videos"),
                ("music", "Music"),
                ("images", "Images"),
                ("documents", "Documents"),
                ("archives", "Archives"),
                ("applications", "Applications"),
                ("other", "Other"),
            ]
            .into_iter()
            .map(|(id, name)| Category {
                id: id.into(),
                name: name.into(),
                directory: None,
                builtin: true,
            })
            .collect(),
        }
    }
}
impl OrganizationConfig {
    pub fn validate(&self) -> Result<()> {
        if self.categories.len() > 71 {
            return Err(Error::Invalid(
                "Category limit reached (64 custom categories)".into(),
            ));
        }
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        for c in &self.categories {
            if c.id.is_empty()
                || c.id.len() > 80
                || !ids.insert(&c.id)
                || !names.insert(c.name.to_lowercase())
                || c.name.trim().is_empty()
                || c.name.chars().count() > 64
                || c.name.chars().any(char::is_control)
            {
                return Err(Error::Invalid(
                    "Use a unique category name of 1–64 characters".into(),
                ));
            }
        }
        for builtin in Self::default().categories {
            if !self
                .categories
                .iter()
                .any(|c| c.id == builtin.id && c.builtin && c.name == builtin.name)
            {
                return Err(Error::Storage(
                    "Built-in categories are missing or changed".into(),
                ));
            }
        }
        if self
            .categories
            .iter()
            .any(|c| c.builtin && !Self::default().categories.iter().any(|b| b.id == c.id))
        {
            return Err(Error::Storage("Unknown built-in category".into()));
        }
        Ok(())
    }
    pub fn contains(&self, id: &str) -> bool {
        self.categories.iter().any(|c| c.id == id)
    }
}

// Display names remain arbitrary user text; generated folders must be portable
// single components and must never interpret a category name as a path.
fn category_folder(category: &Category) -> String {
    if validate_filename(&category.name).is_ok() {
        category.name.clone()
    } else {
        use sha2::{Digest, Sha256};
        format!("Category-{:x}", Sha256::digest(category.id.as_bytes()))[..25].into()
    }
}

async fn planned_directory(path: &Path) -> Result<PathBuf> {
    match tokio::fs::canonicalize(path).await {
        Ok(p) => Ok(p),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .ok_or_else(|| Error::Invalid("Destination has no parent folder".into()))?;
            let name = path
                .file_name()
                .ok_or_else(|| Error::Invalid("Choose a destination folder".into()))?;
            Ok(tokio::fs::canonicalize(parent).await?.join(name))
        }
        Err(e) => Err(e.into()),
    }
}

pub fn classify(filename: &str, content_type: Option<&str>) -> &'static str {
    let ext = Path::new(filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let known = match ext.as_str() {
        "mp4" | "mkv" | "avi" | "mov" | "webm" | "m4v" | "wmv" | "flv" | "mpeg" | "mpg" | "3gp" => {
            "videos"
        }
        "mp3" | "m4a" | "aac" | "flac" | "wav" | "ogg" | "opus" | "wma" | "alac" => "music",
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "svg" | "heic" | "heif" | "tiff"
        | "tif" | "avif" | "ico" => "images",
        "pdf" | "txt" | "doc" | "docx" | "odt" | "rtf" | "xls" | "xlsx" | "csv" | "ppt"
        | "pptx" | "epub" | "md" | "json" | "xml" => "documents",
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "tgz" | "zst" | "xip" => "archives",
        "exe" | "msi" | "msix" | "dmg" | "pkg" | "appimage" | "deb" | "rpm" | "apk" => {
            "applications"
        }
        _ => "other",
    };
    if known != "other" {
        return known;
    }
    let mime = content_type
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if mime.starts_with("video/") {
        "videos"
    } else if mime.starts_with("audio/") {
        "music"
    } else if mime.starts_with("image/") {
        "images"
    } else if mime.starts_with("text/")
        || matches!(
            mime.as_str(),
            "application/pdf"
                | "application/msword"
                | "application/epub+zip"
                | "application/rtf"
                | "application/json"
                | "application/xml"
        )
        || mime.contains("officedocument")
    {
        "documents"
    } else if matches!(
        mime.as_str(),
        "application/zip"
            | "application/gzip"
            | "application/x-tar"
            | "application/x-7z-compressed"
            | "application/vnd.rar"
    ) {
        "archives"
    } else if matches!(
        mime.as_str(),
        "application/vnd.microsoft.portable-executable"
            | "application/x-apple-diskimage"
            | "application/vnd.android.package-archive"
            | "application/x-debian-package"
    ) {
        "applications"
    } else {
        "other"
    }
}

pub(crate) fn validate_filename(name: &str) -> Result<()> {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if name.trim().is_empty()
        || [".", ".."].contains(&name)
        || name.len() > 255
        || name.ends_with(['.', ' '])
        || name.contains(['/', '\\', '\0', ':', '<', '>', '"', '|', '?', '*'])
        || name.chars().any(char::is_control)
        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        return Err(Error::Invalid(
            "Choose a portable filename without reserved names or path separators".into(),
        ));
    }
    Ok(())
}
pub fn normalized_url(value: &str) -> Result<String> {
    crate::recovery::url(value)
}
#[derive(Debug, Clone)]
pub struct DestinationPlan {
    pub category_id: String,
    pub directory: String,
}
#[derive(Debug, Clone)]
pub struct DuplicateCandidate {
    pub id: String,
    pub filename: String,
    pub destination: String,
    pub reasons: Vec<String>,
}
#[derive(Debug, Clone)]
pub struct DuplicateQuery {
    pub url: String,
    pub directory: String,
    pub filename: String,
    pub total: Option<u64>,
    pub etag: Option<String>,
}
fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}
fn candidates(
    items: &[Download],
    query: &DuplicateQuery,
    directory: &Path,
) -> Result<Vec<DuplicateCandidate>> {
    let url = normalized_url(&query.url)?;
    let target = path_key(&directory.join(&query.filename));
    let origin = reqwest::Url::parse(&url).ok().map(|u| u.origin());
    Ok(items
        .iter()
        .filter(|d| d.status != Status::Cancelled)
        .filter_map(|d| {
            let mut reasons = vec![];
            if normalized_url(&d.url).ok().as_deref() == Some(&url) {
                reasons.push("Same source URL".into());
            }
            if path_key(Path::new(&d.destination)) == target {
                reasons.push("Same destination".into());
            }
            if query.total.is_some()
                && query.total == d.total
                && query.filename.eq_ignore_ascii_case(&d.filename)
            {
                reasons.push("Matching filename and size".into());
            }
            if query.total.is_some()
                && query.total == d.total
                && query
                    .etag
                    .as_ref()
                    .is_some_and(|e| !e.starts_with("W/") && Some(e) == d.etag.as_ref())
                && reqwest::Url::parse(&d.url).ok().map(|u| u.origin()) == origin
            {
                reasons.push("Matching server metadata".into());
            }
            (!reasons.is_empty()).then(|| DuplicateCandidate {
                id: d.id.clone(),
                filename: d.filename.clone(),
                destination: d.destination.clone(),
                reasons,
            })
        })
        .collect())
}
#[derive(Debug, Clone)]
pub struct SourceSuggestion {
    pub category_id: String,
    pub content_type: Option<String>,
    pub total: Option<u64>,
    pub etag: Option<String>,
}
#[derive(Debug, Clone, Copy)]
pub enum BatchAction {
    Pause,
    Resume,
    Cancel,
    Remove,
    Category,
}
#[derive(Debug, Clone)]
pub struct BatchResult {
    pub id: String,
    pub error: Option<String>,
}
impl Engine {
    pub async fn organization(&self) -> OrganizationConfig {
        self.inner.organization.lock().await.clone()
    }
    pub async fn set_organization_enabled(&self, enabled: bool) -> Result<()> {
        let _command = self.inner.commands.lock().await;
        self.ensure_open()?;
        let mut config = self.organization().await;
        config.enabled = enabled;
        self.inner
            .storage
            .organization(Some((config.clone(), vec![])))
            .await?;
        *self.inner.organization.lock().await = config;
        Ok(())
    }
    pub async fn save_category(
        &self,
        id: Option<String>,
        name: String,
        directory: Option<PathBuf>,
    ) -> Result<String> {
        let _command = self.inner.commands.lock().await;
        self.ensure_open()?;
        let mut config = self.organization().await;
        let id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let folder = if let Some(p) = directory {
            let p = tokio::fs::canonicalize(p).await?;
            if !tokio::fs::metadata(&p).await?.is_dir() {
                return Err(Error::Invalid("Choose a destination folder".into()));
            }
            Some(p.to_string_lossy().into_owned())
        } else {
            None
        };
        let builtin = config
            .categories
            .iter()
            .find(|c| c.id == id)
            .is_some_and(|c| c.builtin);
        if let Some(c) = config.categories.iter_mut().find(|c| c.id == id) {
            if !builtin {
                c.name = name.trim().into();
            }
            c.directory = folder;
        } else {
            config.categories.push(Category {
                id: id.clone(),
                name: name.trim().into(),
                directory: folder,
                builtin: false,
            });
        }
        config.validate()?;
        self.inner
            .storage
            .organization(Some((config.clone(), vec![])))
            .await?;
        *self.inner.organization.lock().await = config;
        Ok(id)
    }
    pub async fn delete_category(&self, id: &str) -> Result<()> {
        let _command = self.inner.commands.lock().await;
        self.ensure_open()?;
        let mut config = self.organization().await;
        let c = config
            .categories
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| Error::NotFound("Category".into()))?;
        if c.builtin {
            return Err(Error::Invalid(
                "Built-in categories cannot be deleted".into(),
            ));
        }
        let mut map = self.inner.downloads.lock().await;
        let mut records = vec![];
        for mut d in map
            .values()
            .filter(|d| d.category_id.as_deref() == Some(id))
            .cloned()
        {
            self.guard_recovery(&d.id)?;
            d.category_id = Some("other".into());
            d.category_manual = true;
            records.push(d);
        }
        config.categories.retain(|c| c.id != id);
        self.inner
            .storage
            .organization(Some((config.clone(), records.clone())))
            .await?;
        *self.inner.organization.lock().await = config;
        for d in records {
            map.insert(d.id.clone(), d);
        }
        self.inner
            .events
            .send_replace(map.values().cloned().collect());
        Ok(())
    }
    pub async fn suggest_destination(
        &self,
        filename: &str,
        default_directory: &str,
        category: Option<&str>,
        content_type: Option<&str>,
    ) -> Result<DestinationPlan> {
        let config = self.organization().await;
        let id = category.unwrap_or_else(|| classify(filename, content_type));
        let c = config
            .categories
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| Error::Invalid("Category no longer exists".into()))?;
        Ok(DestinationPlan {
            category_id: id.into(),
            directory: if config.enabled {
                c.directory.clone().unwrap_or_else(|| {
                    Path::new(default_directory)
                        .join(category_folder(c))
                        .to_string_lossy()
                        .into()
                })
            } else {
                default_directory.into()
            },
        })
    }
    pub async fn duplicate_candidates(
        &self,
        query: DuplicateQuery,
    ) -> Result<Vec<DuplicateCandidate>> {
        let directory = planned_directory(Path::new(&query.directory)).await?;
        let mut found = candidates(&self.list().await, &query, &directory)?;
        if tokio::fs::symlink_metadata(directory.join(&query.filename))
            .await
            .is_ok()
            && !found.iter().any(|c| {
                path_key(Path::new(&c.destination)) == path_key(&directory.join(&query.filename))
            })
        {
            found.push(DuplicateCandidate {
                id: String::new(),
                filename: query.filename.clone(),
                destination: directory.join(&query.filename).to_string_lossy().into(),
                reasons: vec!["Destination file already exists".into()],
            });
        }
        Ok(found)
    }
    pub(crate) async fn prepare_creation(
        &self,
        url: &str,
        directory: PathBuf,
        mut filename: String,
        settings: &CreationSettings,
    ) -> Result<(PathBuf, String, String)> {
        validate_filename(&filename)?;
        let directory = tokio::fs::canonicalize(directory).await?;
        if !tokio::fs::metadata(&directory).await?.is_dir() {
            return Err(Error::Invalid("Choose a destination folder".into()));
        }
        let default = directory.to_string_lossy();
        let plan = self
            .suggest_destination(
                &filename,
                &default,
                settings.category_id.as_deref(),
                settings.content_type_hint.as_deref(),
            )
            .await?;
        let directory = if settings.use_category_directory {
            PathBuf::from(plan.directory)
        } else {
            directory
        };
        if settings.use_category_directory {
            let config = self.organization().await;
            if config.enabled
                && config
                    .categories
                    .iter()
                    .any(|c| c.id == plan.category_id && c.directory.is_none())
            {
                // Only create the one generated child of an existing configured base.
                // Suggestions and category assignment never create folders or move files.
                match tokio::fs::create_dir(&directory).await {
                    Ok(()) => crate::transfer::sync_parent(&directory).await?,
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e.into()),
                }
                let metadata = tokio::fs::symlink_metadata(&directory).await?;
                if !metadata.is_dir() || metadata.is_symlink() {
                    return Err(Error::Invalid(
                        "The category destination must be a folder, not a file or symlink".into(),
                    ));
                }
            }
        }
        let directory = tokio::fs::canonicalize(directory).await?;
        let items = self.list().await;
        if !settings.allow_duplicates
            && !candidates(
                &items,
                &DuplicateQuery {
                    url: url.into(),
                    directory: directory.to_string_lossy().into(),
                    filename: filename.clone(),
                    total: settings.expected_size,
                    etag: settings.expected_etag.clone(),
                },
                &directory,
            )?
            .is_empty()
        {
            return Err(Error::Invalid(
                "A similar download exists. Review it or choose a separate copy".into(),
            ));
        }
        if settings.separate_copy {
            let original = filename.clone();
            let p = Path::new(&original);
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(&original);
            let ext = p.extension().and_then(|s| s.to_str());
            for n in 1..=10000 {
                let candidate = if n == 1 {
                    original.clone()
                } else {
                    format!(
                        "{stem} ({n}){}",
                        ext.map(|e| format!(".{e}")).unwrap_or_default()
                    )
                };
                let path = directory.join(&candidate);
                let occupied = items.iter().any(|d| {
                    d.status != Status::Cancelled
                        && path_key(Path::new(&d.destination)) == path_key(&path)
                });
                match tokio::fs::symlink_metadata(&path).await {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound && !occupied => {
                        filename = candidate;
                        break;
                    }
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                    _ => {}
                }
                if n == 10000 {
                    return Err(Error::Busy("No available separate-copy filename".into()));
                }
            }
        }
        validate_filename(&filename)?;
        self.guard_move_target(&directory.join(&filename))?;
        Ok((directory, filename, plan.category_id))
    }
    pub async fn assign_category(&self, id: &str, category: &str) -> Result<()> {
        let _command = self.inner.commands.lock().await;
        self.ensure_open()?;
        self.guard_recovery(id)?;
        if !self.organization().await.contains(category) {
            return Err(Error::Invalid("Category no longer exists".into()));
        }
        let mut map = self.inner.downloads.lock().await;
        let mut d = map
            .get(id)
            .cloned()
            .ok_or_else(|| Error::NotFound(id.into()))?;
        d.category_id = Some(category.into());
        d.category_manual = true;
        self.inner.storage.save(&d).await?;
        map.insert(d.id.clone(), d);
        self.inner
            .events
            .send_replace(map.values().cloned().collect());
        Ok(())
    }
    pub async fn rename_download(&self, id: &str, filename: &str) -> Result<()> {
        let _command = self.inner.commands.lock().await;
        self.ensure_open()?;
        self.guard_recovery(id)?;
        validate_filename(filename)?;
        let mut d = self.get(id).await?;
        if d.ever_started
            || !matches!(d.status, Status::Ready | Status::Paused)
            || d.durable_bytes > 0
        {
            return Err(Error::Invalid(
                "Only downloads that have never started can be renamed".into(),
            ));
        }
        let parent = Path::new(&d.destination)
            .parent()
            .ok_or_else(|| Error::Invalid("Destination has no folder".into()))?;
        let target = parent.join(filename);
        self.guard_move_target(&target)?;
        if self.list().await.iter().any(|x| {
            x.id != id
                && x.status != Status::Cancelled
                && path_key(Path::new(&x.destination)) == path_key(&target)
        }) {
            return Err(Error::Invalid(
                "Another download uses this destination".into(),
            ));
        }
        match tokio::fs::symlink_metadata(&target).await {
            Ok(_) => return Err(Error::Invalid("Destination already exists".into())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        d.filename = filename.into();
        d.destination = target.to_string_lossy().into();
        if !d.category_manual {
            d.category_id = Some(classify(filename, d.content_type.as_deref()).into());
        }
        self.persist(&d).await
    }
    pub async fn remove_record(&self, id: &str) -> Result<()> {
        let d = self.get(id).await?;
        if d.status == Status::Cancelled {
            return self.remove_cancelled(id).await;
        }
        let _command = self.inner.commands.lock().await;
        self.ensure_open()?;
        self.guard_recovery(id)?;
        let d = self.get(id).await?;
        if d.status != Status::Completed {
            return Err(Error::Invalid(
                "Cancel unfinished downloads before removing their records".into(),
            ));
        }
        self.inner.storage.delete(id).await?;
        self.inner.downloads.lock().await.remove(id);
        self.inner.bandwidth.set_local(id, None);
        self.inner.events.send_replace(self.list().await);
        Ok(())
    }
    // Caller holds commands across the entire batch. Only submit here: admitting
    // an early item would hide later, higher-priority items from the scheduler.
    async fn resume_preserving_data_locked(&self, id: &str) -> Result<()> {
        let mut d = self.prepare_enqueue_locked(id).await?;
        if d.durable_bytes > 0 {
            if d.etag.is_none() {
                return Err(Error::Invalid(
                    "Cannot safely resume; update the link or restart explicitly".into(),
                ));
            }
            d.automation_strict_resume = true;
        }
        d.pause_origin = None;
        self.commit_manual_start(d).await
    }
    pub async fn batch(
        &self,
        ids: Vec<String>,
        action: BatchAction,
        category: Option<String>,
        allow_unsafe_pause: bool,
    ) -> Result<Vec<BatchResult>> {
        if ids.len() > 1000 {
            return Err(Error::Invalid("Select at most 1000 downloads".into()));
        }
        // The supervisor and manual/automation commands use this same lock.
        // Keep it through validation, all submissions and the single admission pass.
        let _resume_batch = if matches!(action, BatchAction::Resume) {
            Some(self.inner.commands.lock().await)
        } else {
            None
        };
        let mut seen = BTreeSet::new();
        let mut out = vec![];
        for id in ids.into_iter().filter(|id| seen.insert(id.clone())) {
            let result=match self.get(&id).await {
                Err(e)=>Err(e),Ok(d)=>match action {
                    BatchAction::Pause if d.status==Status::Running && d.resume.capability!=ResumeCapability::Supported && !allow_unsafe_pause=>Err(Error::Invalid("Pause requires confirmation because resume safety is unknown or unsupported".into())),
                    BatchAction::Pause if matches!(d.status,Status::Completed|Status::Cancelled|Status::Finalizing)=>Err(Error::Invalid("This download cannot be paused".into())),
                    BatchAction::Pause=>self.pause(&id).await,
                    BatchAction::Resume=>self.resume_preserving_data_locked(&id).await,
                    BatchAction::Cancel if matches!(d.status,Status::Completed|Status::Finalizing)=>Err(Error::Invalid("Completed files are not cancelled or deleted".into())),
                    BatchAction::Cancel=>self.cancel(&id).await,
                    BatchAction::Remove=>self.remove_record(&id).await,
                    BatchAction::Category=>match &category {Some(c)=>self.assign_category(&id,c).await,None=>Err(Error::Invalid("Choose a category".into()))},
                }};
            out.push(BatchResult {
                id,
                error: result.err().map(|e| e.to_string()),
            });
        }
        if matches!(action, BatchAction::Resume)
            && out.iter().any(|result| result.error.is_none())
            && let Err(error) = self.schedule_locked().await
        {
            // Queue commits already succeeded. Preserve individual validation
            // errors and report admission trouble against still-queued submissions.
            for result in out.iter_mut().filter(|result| result.error.is_none()) {
                if self
                    .get(&result.id)
                    .await
                    .is_ok_and(|d| d.status == Status::Queued)
                {
                    result.error = Some(format!(
                        "Submitted to queue; admission needs attention: {error}"
                    ));
                }
            }
        }
        Ok(out)
    }
    pub async fn inspect_source_category(
        &self,
        url: &str,
        filename: &str,
    ) -> Result<SourceSuggestion> {
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            self.inspect_source_category_inner(url, filename),
        )
        .await
        .map_err(|_| Error::Protocol("Link inspection timed out".into()))?
    }
    async fn inspect_source_category_inner(
        &self,
        url: &str,
        filename: &str,
    ) -> Result<SourceSuggestion> {
        self.ensure_open()?;
        let mut url = reqwest::Url::parse(&normalized_url(url)?)
            .map_err(|_| Error::Invalid("Invalid URL".into()))?;
        let id = format!("preview-{}", uuid::Uuid::new_v4());
        let (_tx, mut stop) = tokio::sync::watch::channel(crate::engine::Stop::Running);
        for _ in 0..=10 {
            let permit = self
                .inner
                .connections
                .acquire(url.host_str().unwrap_or("").into(), id.clone(), &mut stop)
                .await?;
            let response = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                self.inner
                    .client
                    .head(url.clone())
                    .header(reqwest::header::ACCEPT_ENCODING, "identity")
                    .send(),
            )
            .await
            .map_err(|_| Error::Protocol("Link inspection timed out".into()))??;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| Error::Protocol("Redirect missing Location".into()))?;
                let next = url
                    .join(location)
                    .map_err(|_| Error::Protocol("Invalid redirect".into()))?;
                url = reqwest::Url::parse(&normalized_url(next.as_str())?)
                    .map_err(|_| Error::Protocol("Invalid redirect".into()))?;
                drop(permit);
                continue;
            }
            if !response.status().is_success() {
                return Err(Error::Http {
                    status: response.status().as_u16(),
                    retry_after_secs: 0,
                });
            }
            let content_type = crate::transfer::text(&response, reqwest::header::CONTENT_TYPE)
                .filter(|s| s.len() < 256);
            let identity = crate::transfer::text(&response, reqwest::header::CONTENT_ENCODING)
                .is_none_or(|v| v.eq_ignore_ascii_case("identity"));
            return Ok(SourceSuggestion {
                category_id: classify(filename, content_type.as_deref()).into(),
                content_type,
                total: identity
                    .then(|| {
                        crate::transfer::text(&response, reqwest::header::CONTENT_LENGTH)
                            .and_then(|n| n.parse::<u64>().ok())
                    })
                    .flatten(),
                etag: if identity {
                    crate::transfer::text(&response, reqwest::header::ETAG)
                        .filter(|s| s.len() < 1024 && !s.starts_with("W/"))
                } else {
                    None
                },
            });
        }
        Err(Error::Protocol("Too many redirects".into()))
    }
}

#[cfg(test)]
mod batch_resume_tests {
    use super::*;

    #[tokio::test]
    async fn unsafe_partial_resume_does_not_block_other_batch_submissions() {
        let root = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/file", listener.local_addr().unwrap());
        let engine = Engine::open(root.path().join("metadata")).await.unwrap();
        let unsafe_item = engine
            .create(url.clone(), root.path().into(), "unsafe.bin".into())
            .await
            .unwrap();
        let safe_item = engine
            .create(url, root.path().into(), "safe.bin".into())
            .await
            .unwrap();
        engine.pause(&unsafe_item.id).await.unwrap();
        engine.pause(&safe_item.id).await.unwrap();
        // A retained checkpoint with no validator must not be silently restarted.
        let mut retained = engine.get(&unsafe_item.id).await.unwrap();
        retained.durable_bytes = 1;
        retained.downloaded = 1;
        retained.etag = None;
        engine.persist(&retained).await.unwrap();
        let result = engine
            .batch(
                vec![unsafe_item.id.clone(), safe_item.id.clone()],
                BatchAction::Resume,
                None,
                false,
            )
            .await
            .unwrap();
        assert!(
            result[0]
                .error
                .as_deref()
                .unwrap()
                .contains("Cannot safely resume")
        );
        assert!(result[1].error.is_none());
        let retained = engine.get(&unsafe_item.id).await.unwrap();
        assert_eq!(retained.status, Status::Paused);
        assert_eq!(retained.durable_bytes, 1);
        assert!(engine.get(&safe_item.id).await.unwrap().ever_started);
        engine.shutdown().await.unwrap();
    }
}
