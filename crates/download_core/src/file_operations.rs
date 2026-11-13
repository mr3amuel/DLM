//! Journaled moves of completed files. No active partial path is ever relocated.
use crate::{Download, Engine, Error, Result, Status, storage::Storage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::watch,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum MovePhase {
    Prepared,
    Published,
    Committed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MoveJournal {
    pub id: String,
    pub source: String,
    pub target: String,
    pub staging: String,
    pub nonce: String,
    pub length: u64,
    pub hash: String,
    pub copied: bool,
    pub phase: MovePhase,
}
pub(crate) struct MoveLease {
    pub target: PathBuf,
    pub done: watch::Sender<bool>,
}
struct MoveGuard {
    inner: Arc<crate::engine::Inner>,
    id: String,
    journaled: bool,
    finished: bool,
    done: watch::Sender<bool>,
}
impl Drop for MoveGuard {
    fn drop(&mut self) {
        if !self.journaled || self.finished {
            self.inner.file_ops.lock().unwrap().remove(&self.id);
        }
        self.done.send_replace(true);
    }
}
#[derive(Debug, Clone)]
pub struct FileMoveResult {
    pub download: Download,
    pub source_kept: bool,
}
fn interrupted(engine: Option<&Engine>) -> Result<()> {
    if engine.is_some_and(Engine::is_shutdown) {
        Err(Error::Stopped)
    } else {
        Ok(())
    }
}
async fn digest(path: &Path, engine: Option<&Engine>) -> Result<(u64, String)> {
    if !tokio::fs::symlink_metadata(path).await?.is_file() {
        return Err(Error::Invalid(
            "File operations require a regular file, not a symlink".into(),
        ));
    }
    let mut f = tokio::fs::File::open(path).await?;
    let before = f.metadata().await?;
    let mut bytes = 0;
    let mut hash = Sha256::new();
    let mut buf = vec![0; 64 * 1024];
    loop {
        interrupted(engine)?;
        let n = f.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        bytes += n as u64;
    }
    let after = f.metadata().await?;
    if before.len() != bytes
        || after.len() != bytes
        || before.modified().ok() != after.modified().ok()
    {
        return Err(Error::Invalid(
            "File changed while it was being verified".into(),
        ));
    }
    Ok((bytes, format!("{:x}", hash.finalize())))
}
async fn matches(path: &str, j: &MoveJournal) -> bool {
    digest(Path::new(path), None)
        .await
        .is_ok_and(|(n, h)| n == j.length && h == j.hash)
}
impl MoveJournal {
    fn validate(&self, d: &Download) -> Result<()> {
        let source = Path::new(&self.source);
        let target = Path::new(&self.target);
        if uuid::Uuid::parse_str(&self.nonce).is_err()
            || self.id != d.id
            || self.hash.len() != 64
            || !self.hash.bytes().all(|b| b.is_ascii_hexdigit())
            || !source.is_absolute()
            || !target.is_absolute()
            || source == target
            || (d.destination != self.source && d.destination != self.target)
            || target.file_name().and_then(|s| s.to_str()) != Some(&d.filename)
            || target
                .parent()
                .map(|p| p.join(format!(".dlm-move-{}.tmp", self.nonce)))
                != Some(PathBuf::from(&self.staging))
        {
            return Err(Error::Storage(
                "Invalid completed-file move journal; no files were changed".into(),
            ));
        }
        Ok(())
    }
}
async fn remove_staging(j: &MoveJournal) -> Result<()> {
    match tokio::fs::symlink_metadata(&j.staging).await {
        Ok(m) if m.is_file() => {
            tokio::fs::remove_file(&j.staging).await?;
            crate::transfer::sync_parent(Path::new(&j.staging)).await?;
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(Error::Storage(
            "Move staging path was replaced; no files were removed".into(),
        )),
        Err(e) => Err(e.into()),
    }
}
async fn complete(
    storage: &Storage,
    mut j: MoveJournal,
    mut d: Download,
) -> Result<FileMoveResult> {
    j.validate(&d)?;
    if !matches(&j.target, &j).await {
        return Err(Error::Storage(format!(
            "Cannot verify interrupted move destination: {}. Source and journal were preserved",
            j.target
        )));
    }
    d.destination = j.target.clone();
    j.phase = MovePhase::Committed;
    storage
        .move_record(j.clone(), Some(d.clone()), false)
        .await?;
    let mut source_kept = false;
    if j.copied {
        match tokio::fs::symlink_metadata(&j.source).await {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Ok(m) if m.is_file() && matches(&j.source, &j).await => {
                if tokio::fs::remove_file(&j.source).await.is_err() {
                    source_kept = true;
                } else {
                    crate::transfer::sync_parent(Path::new(&j.source)).await?;
                }
            }
            _ => source_kept = true,
        }
    }
    if source_kept {
        d.notice=Some("Move completed; the original file was kept because it changed or could not be removed safely".into());
    }
    remove_staging(&j).await?;
    storage.move_record(j, Some(d.clone()), true).await?;
    Ok(FileMoveResult {
        download: d,
        source_kept,
    })
}
pub(crate) async fn recover_moves(storage: &Storage) -> Result<()> {
    for j in storage.moves().await? {
        let d = storage
            .load()
            .await?
            .into_iter()
            .find(|d| d.id == j.id)
            .ok_or_else(|| Error::Storage("Move journal references a missing download".into()))?;
        j.validate(&d)?;
        if j.phase == MovePhase::Prepared {
            if tokio::fs::symlink_metadata(&j.source).await.is_ok() {
                // Publication may have raced a crash before its journal update. Keep
                // both files rather than infer ownership of an existing destination.
                remove_staging(&j).await?;
                let mut d = d;
                if tokio::fs::symlink_metadata(&j.target).await.is_ok() {
                    d.notice=Some("An interrupted move left a destination copy. The original location and both files were preserved; review the destination before trying again".into());
                }
                storage.move_record(j, Some(d), true).await?;
            } else if matches(&j.target, &j).await {
                complete(storage, j, d).await?;
            } else {
                return Err(Error::Storage(format!(
                    "Cannot recover completed-file move for {}: neither source nor verified target is available. No files were deleted",
                    j.id
                )));
            }
        } else {
            complete(storage, j, d).await?;
        }
    }
    Ok(())
}
impl Engine {
    pub(crate) fn guard_move_target(&self, path: &Path) -> Result<()> {
        if self.inner.file_ops.lock().unwrap().values().any(|l| {
            l.target.to_string_lossy().to_lowercase() == path.to_string_lossy().to_lowercase()
        }) {
            return Err(Error::Busy(
                "A completed-file move has reserved this destination".into(),
            ));
        }
        Ok(())
    }

    pub async fn move_completed(&self, id: &str, directory: PathBuf) -> Result<FileMoveResult> {
        self.move_completed_impl(id, directory, false).await
    }
    async fn move_completed_impl(
        &self,
        id: &str,
        directory: PathBuf,
        copy_only: bool,
    ) -> Result<FileMoveResult> {
        let (d, target, mut guard) = {
            let _command = self.inner.commands.lock().await;
            self.ensure_open()?;
            self.guard_recovery(id)?;
            let d = self.get(id).await?;
            if d.status != Status::Completed {
                return Err(Error::Invalid(
                    "Only completed files can be moved; active partial files stay in place".into(),
                ));
            }
            let directory = tokio::fs::canonicalize(directory).await?;
            if !tokio::fs::metadata(&directory).await?.is_dir() {
                return Err(Error::Invalid("Choose a destination folder".into()));
            }
            let target = directory.join(&d.filename);
            self.guard_move_target(&target)?;
            if target == Path::new(&d.destination) {
                return Ok(FileMoveResult {
                    download: d,
                    source_kept: false,
                });
            }
            if self.list().await.iter().any(|x| {
                x.id != id
                    && x.status != Status::Cancelled
                    && x.destination.to_lowercase() == target.to_string_lossy().to_lowercase()
            }) {
                return Err(Error::Invalid(
                    "Another download uses this destination".into(),
                ));
            }
            match tokio::fs::symlink_metadata(&target).await {
                Ok(_) => {
                    return Err(Error::Invalid(
                        "Destination already exists; choose another folder".into(),
                    ));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            let (done, _) = watch::channel(false);
            self.inner.file_ops.lock().unwrap().insert(
                id.into(),
                MoveLease {
                    done: done.clone(),
                    target: target.clone(),
                },
            );
            (
                d,
                target,
                MoveGuard {
                    inner: self.inner.clone(),
                    id: id.into(),
                    journaled: false,
                    finished: false,
                    done,
                },
            )
        };
        let (length, hash) = digest(Path::new(&d.destination), Some(self)).await?;
        if d.total.is_some_and(|n| n != length) || d.sha256.as_ref().is_some_and(|h| h != &hash) {
            return Err(Error::Invalid(
                "Completed file differs from its recorded size or checksum; no file was moved"
                    .into(),
            ));
        }
        let nonce = uuid::Uuid::new_v4().to_string();
        let mut j = MoveJournal {
            id: id.into(),
            source: d.destination.clone(),
            target: target.to_string_lossy().into(),
            staging: target
                .parent()
                .unwrap()
                .join(format!(".dlm-move-{nonce}.tmp"))
                .to_string_lossy()
                .into(),
            nonce,
            length,
            hash,
            copied: copy_only,
            phase: MovePhase::Prepared,
        };
        guard.journaled = true;
        self.inner
            .storage
            .move_record(j.clone(), None, false)
            .await?;
        let result: Result<FileMoveResult> = async {
            interrupted(Some(self))?;
            let copy = if copy_only {
                true
            } else {
                match renamore::rename_exclusive(&j.source, &j.target) {
                    Ok(()) => false,
                    Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => true,
                    Err(e) => return Err(e.into()),
                }
            };
            if copy {
                j.copied = true;
                self.inner
                    .storage
                    .move_record(j.clone(), None, false)
                    .await?;
                let mut source = tokio::fs::File::open(&j.source).await?;
                let mut staging = tokio::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&j.staging)
                    .await?;
                let mut hash = Sha256::new();
                let mut length = 0;
                let mut buf = vec![0; 64 * 1024];
                loop {
                    interrupted(Some(self))?;
                    let n = source.read(&mut buf).await?;
                    if n == 0 {
                        break;
                    }
                    staging.write_all(&buf[..n]).await?;
                    hash.update(&buf[..n]);
                    length += n as u64;
                }
                staging.sync_all().await?;
                drop(staging);
                if length != j.length || format!("{:x}", hash.finalize()) != j.hash {
                    return Err(Error::Invalid(
                        "Source changed during copy; original file was preserved".into(),
                    ));
                }
                interrupted(Some(self))?;
                renamore::rename_exclusive(&j.staging, &j.target)?;
            }
            crate::transfer::sync_parent(Path::new(&j.target)).await?;
            if !copy {
                crate::transfer::sync_parent(Path::new(&j.source)).await?;
            }
            j.phase = MovePhase::Published;
            self.inner
                .storage
                .move_record(j.clone(), None, false)
                .await?;
            complete(&self.inner.storage, j.clone(), d.clone()).await
        }
        .await;
        match result {
            Ok(result) => {
                guard.finished = true;
                self.publish(&result.download).await;
                Ok(result)
            }
            Err(error) => {
                if j.phase == MovePhase::Prepared
                    && tokio::fs::symlink_metadata(&j.source).await.is_ok()
                    && remove_staging(&j).await.is_ok()
                    && self.inner.storage.move_record(j, None, true).await.is_ok()
                {
                    guard.finished = true;
                }
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn completed(root: &Path) -> (Engine, Download, PathBuf) {
        let e = Engine::open(root.join("meta")).await.unwrap();
        let mut d = e
            .create(
                "https://example.org/file".into(),
                root.into(),
                "complete.bin".into(),
            )
            .await
            .unwrap();
        tokio::fs::write(&d.destination, b"verified completed data")
            .await
            .unwrap();
        tokio::fs::remove_file(&d.partial_path).await.unwrap();
        let (length, hash) = digest(Path::new(&d.destination), None).await.unwrap();
        d.status = Status::Completed;
        d.ever_started = true;
        d.downloaded = length;
        d.total = Some(length);
        d.sha256 = Some(hash);
        e.inner.storage.save(&d).await.unwrap();
        e.publish(&d).await;
        let target = root.join("target");
        tokio::fs::create_dir(&target).await.unwrap();
        (e, d, target)
    }
    fn journal(d: &Download, target: &Path, copied: bool, phase: MovePhase) -> MoveJournal {
        let nonce = uuid::Uuid::new_v4().to_string();
        MoveJournal {
            id: d.id.clone(),
            source: d.destination.clone(),
            target: target.join(&d.filename).to_string_lossy().into(),
            staging: target
                .join(format!(".dlm-move-{nonce}.tmp"))
                .to_string_lossy()
                .into(),
            nonce,
            length: d.total.unwrap(),
            hash: d.sha256.clone().unwrap(),
            copied,
            phase,
        }
    }
    #[tokio::test]
    async fn cross_volume_copy_path_verifies_and_cleans_source_after_metadata_commit() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let (e, d, target) = completed(&root).await;
        let moved = e
            .move_completed_impl(&d.id, target.clone(), true)
            .await
            .unwrap();
        assert!(!moved.source_kept);
        assert!(!Path::new(&d.destination).exists());
        assert_eq!(
            tokio::fs::read(target.join(&d.filename)).await.unwrap(),
            b"verified completed data"
        );
        assert!(e.inner.storage.moves().await.unwrap().is_empty());
        e.shutdown().await.unwrap();
        drop(e);
        let e = Engine::open(root.join("meta")).await.unwrap();
        assert_eq!(
            e.get(&d.id).await.unwrap().destination,
            moved.download.destination
        );
        e.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn interrupted_move_phases_recover_without_guessing_or_overwriting() {
        for phase in [
            MovePhase::Prepared,
            MovePhase::Published,
            MovePhase::Committed,
        ] {
            for copied in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let root = std::fs::canonicalize(temp.path()).unwrap();
                let (e, mut d, target) = completed(&root).await;
                let j = journal(&d, &target, copied, phase);
                if copied {
                    tokio::fs::copy(&j.source, &j.target).await.unwrap();
                } else {
                    tokio::fs::rename(&j.source, &j.target).await.unwrap();
                }
                if phase == MovePhase::Committed {
                    d.destination = j.target.clone();
                }
                e.inner
                    .storage
                    .move_record(j.clone(), Some(d.clone()), false)
                    .await
                    .unwrap();
                e.shutdown().await.unwrap();
                drop(e);
                let e = Engine::open(root.join("meta")).await.unwrap();
                let restored = e.get(&d.id).await.unwrap();
                if copied && phase == MovePhase::Prepared {
                    assert_eq!(restored.destination, j.source);
                    assert!(Path::new(&j.source).exists());
                } else {
                    assert_eq!(restored.destination, j.target);
                    assert!(!Path::new(&j.source).exists());
                }
                assert_eq!(
                    tokio::fs::read(&j.target).await.unwrap(),
                    b"verified completed data"
                );
                assert!(e.inner.storage.moves().await.unwrap().is_empty());
                e.shutdown().await.unwrap();
            }
        }
    }
    #[tokio::test]
    async fn interrupted_staging_and_changed_source_never_destroy_user_data() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let (e, d, target) = completed(&root).await;
        let mut j = journal(&d, &target, true, MovePhase::Prepared);
        tokio::fs::write(&j.staging, b"unfinished copy")
            .await
            .unwrap();
        e.inner
            .storage
            .move_record(j.clone(), None, false)
            .await
            .unwrap();
        recover_moves(&e.inner.storage).await.unwrap();
        assert!(!Path::new(&j.staging).exists());
        assert!(Path::new(&j.source).exists());
        tokio::fs::copy(&j.source, &j.target).await.unwrap();
        tokio::fs::write(&j.source, b"changed externally")
            .await
            .unwrap();
        j.phase = MovePhase::Published;
        e.inner
            .storage
            .move_record(j.clone(), None, false)
            .await
            .unwrap();
        recover_moves(&e.inner.storage).await.unwrap();
        assert_eq!(
            tokio::fs::read(&j.source).await.unwrap(),
            b"changed externally"
        );
        let saved = e.inner.storage.load().await.unwrap().pop().unwrap();
        assert_eq!(saved.destination, j.target);
        assert!(saved.notice.is_some());
        e.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn leases_reject_concurrent_mutations_and_corrupt_recovery_preserves_both_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let (e, d, target) = completed(&root).await;
        let (done, _) = watch::channel(false);
        e.inner.file_ops.lock().unwrap().insert(
            d.id.clone(),
            MoveLease {
                done: done.clone(),
                target: target.join(&d.filename),
            },
        );
        assert!(e.move_completed(&d.id, target.clone()).await.is_err());
        assert!(e.assign_category(&d.id, "music").await.is_err());
        assert!(e.remove_record(&d.id).await.is_err());
        assert!(
            e.configure_download(&d.id, Default::default())
                .await
                .is_err()
        );
        assert!(
            e.create(
                "https://example.org/other".into(),
                target.clone(),
                d.filename.clone()
            )
            .await
            .is_err()
        );
        done.send_replace(true);
        e.inner.file_ops.lock().unwrap().clear();
        let j = journal(&d, &target, true, MovePhase::Published);
        tokio::fs::write(&j.target, b"unrelated data")
            .await
            .unwrap();
        e.inner
            .storage
            .move_record(j.clone(), None, false)
            .await
            .unwrap();
        assert!(recover_moves(&e.inner.storage).await.is_err());
        assert_eq!(tokio::fs::read(&j.target).await.unwrap(), b"unrelated data");
        assert!(Path::new(&j.source).exists());
        assert_eq!(e.inner.storage.moves().await.unwrap().len(), 1);
        e.shutdown().await.unwrap();
    }
}
