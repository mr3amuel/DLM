#[allow(dead_code)]
mod support;
use download_core::{BatchAction, CreationSettings, DuplicateQuery, Engine, Status, classify};

struct OrganizationClock;
impl download_core::Clock for OrganizationClock {
    fn now_ms(&self) -> i64 {
        1_800_000_000_000
    }
}

#[test]
fn classification_uses_known_extensions_then_specific_mime() {
    for (name, mime, expected) in [
        ("film.MKV", None, "videos"),
        ("song.flac", None, "music"),
        ("image.heic", None, "images"),
        ("book.pdf", None, "documents"),
        ("file.tar.gz", None, "archives"),
        ("setup.DMG", None, "applications"),
        ("unknown", Some("video/mp4; charset=binary"), "videos"),
        ("opaque", Some("application/octet-stream"), "other"),
        ("report.pdf", Some("text/html"), "documents"),
        ("unknown", None, "other"),
    ] {
        assert_eq!(classify(name, mime), expected);
    }
}
#[tokio::test]
async fn destination_rules_manual_overrides_and_custom_categories_survive_restart() {
    let root = tempfile::tempdir().unwrap();
    let videos = root.path().join("videos");
    std::fs::create_dir(&videos).unwrap();
    let meta = root.path().join("meta");
    let e = Engine::open(&meta).await.unwrap();
    e.save_category(
        Some("videos".into()),
        "ignored builtin rename".into(),
        Some(videos.clone()),
    )
    .await
    .unwrap();
    let p = e
        .suggest_destination("movie.mp4", root.path().to_str().unwrap(), None, None)
        .await
        .unwrap();
    assert_eq!(p.directory, root.path().to_str().unwrap());
    e.set_organization_enabled(true).await.unwrap();
    let settings = CreationSettings {
        use_category_directory: true,
        bandwidth_limit: Some(123_000),
        allow_duplicates: false,
        ..Default::default()
    };
    let d = e
        .create_with_settings(
            "https://example.org/movie".into(),
            root.path().into(),
            "movie.mp4".into(),
            settings.clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        d.destination,
        std::fs::canonicalize(&videos)
            .unwrap()
            .join("movie.mp4")
            .to_str()
            .unwrap()
    );
    assert_eq!(d.category_id.as_deref(), Some("videos"));
    assert_eq!(d.status, Status::Ready);
    let id = e
        .save_category(None, "Courses".into(), Some(videos))
        .await
        .unwrap();
    e.assign_category(&d.id, &id).await.unwrap();
    assert_eq!(e.get(&d.id).await.unwrap().destination, d.destination);
    let manual = e
        .create_with_settings(
            "https://example.org/second".into(),
            root.path().into(),
            "second.mp4".into(),
            CreationSettings {
                use_category_directory: false,
                ..settings
            },
        )
        .await
        .unwrap();
    assert_eq!(
        manual.destination,
        std::fs::canonicalize(root.path())
            .unwrap()
            .join("second.mp4")
            .to_str()
            .unwrap()
    );
    assert!(e.delete_category("videos").await.is_err());
    e.shutdown().await.unwrap();
    drop(e);
    let e = Engine::open(&meta).await.unwrap();
    assert!(e.organization().await.enabled);
    assert_eq!(
        e.get(&d.id).await.unwrap().category_id.as_deref(),
        Some(id.as_str())
    );
    assert_eq!(e.get(&d.id).await.unwrap().bandwidth_limit, Some(123_000));
    e.delete_category(&id).await.unwrap();
    assert_eq!(
        e.get(&d.id).await.unwrap().category_id.as_deref(),
        Some("other")
    );
    assert!(e.save_category(None, "Videos".into(), None).await.is_err());
    e.shutdown().await.unwrap();
}
#[tokio::test]
async fn duplicates_are_advisory_and_separate_copies_are_reserved_under_command_lock() {
    let root = tempfile::tempdir().unwrap();
    let e = Engine::open(root.path().join("meta")).await.unwrap();
    let original = e
        .create(
            "https://EXAMPLE.org:443/file?token=a#part".into(),
            root.path().into(),
            "one.zip".into(),
        )
        .await
        .unwrap();
    let found = e
        .duplicate_candidates(DuplicateQuery {
            url: "https://example.org/file?token=a#other".into(),
            directory: root.path().to_str().unwrap().into(),
            filename: "different.zip".into(),
            total: None,
            etag: None,
        })
        .await
        .unwrap();
    assert_eq!(found[0].id, original.id);
    let strict = CreationSettings {
        allow_duplicates: false,
        ..Default::default()
    };
    assert!(
        e.create_with_settings(
            original.url.clone(),
            root.path().into(),
            "other.zip".into(),
            strict.clone()
        )
        .await
        .is_err()
    );
    std::fs::write(root.path().join("one (2).zip"), b"never overwrite").unwrap();
    let settings = CreationSettings {
        allow_duplicates: true,
        separate_copy: true,
        ..strict
    };
    let (a, b) = tokio::join!(
        e.create_with_settings(
            original.url.clone(),
            root.path().into(),
            "one.zip".into(),
            settings.clone()
        ),
        e.create_with_settings(
            original.url.clone(),
            root.path().into(),
            "one.zip".into(),
            settings
        )
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_ne!(a.destination, b.destination);
    assert!([a.filename, b.filename].contains(&"one (3).zip".into()));
    assert_eq!(
        std::fs::read(root.path().join("one (2).zip")).unwrap(),
        b"never overwrite"
    );
    e.shutdown().await.unwrap();
}
#[tokio::test]
async fn rename_batch_lifecycle_and_category_changes_preserve_data() {
    let root = tempfile::tempdir().unwrap();
    let e = Engine::open(root.path().join("meta")).await.unwrap();
    let d = e
        .create(
            "https://example.org/a".into(),
            root.path().into(),
            "initial.bin".into(),
        )
        .await
        .unwrap();
    e.rename_download(&d.id, "final.pdf").await.unwrap();
    assert_eq!(
        e.get(&d.id).await.unwrap().category_id.as_deref(),
        Some("documents")
    );
    for name in [
        "../bad", "CON.txt", "bad:", "bad.", "COM1", "bad?.bin", "bad*.bin",
    ] {
        assert!(e.rename_download(&d.id, name).await.is_err());
    }
    std::fs::write(root.path().join("existing.pdf"), b"keep").unwrap();
    assert!(e.rename_download(&d.id, "existing.pdf").await.is_err());
    let results = e
        .batch(
            vec![d.id.clone(), "missing".into(), d.id.clone()],
            BatchAction::Category,
            Some("music".into()),
            false,
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
    assert!(results[0].error.is_none());
    assert!(results[1].error.is_some());
    assert!(
        e.batch(vec![d.id.clone()], BatchAction::Remove, None, false)
            .await
            .unwrap()[0]
            .error
            .is_some()
    );
    assert!(
        e.batch(vec![d.id.clone()], BatchAction::Pause, None, false)
            .await
            .unwrap()[0]
            .error
            .is_none()
    );
    assert_eq!(e.get(&d.id).await.unwrap().status, Status::Paused);
    e.batch(vec![d.id.clone()], BatchAction::Cancel, None, false)
        .await
        .unwrap();
    e.batch(vec![d.id.clone()], BatchAction::Remove, None, false)
        .await
        .unwrap();
    assert!(e.list().await.is_empty());
    e.shutdown().await.unwrap();
}
#[tokio::test]
async fn active_category_override_cannot_roll_back_worker_progress_and_completed_move_is_verified()
{
    let f = support::fixture().await;
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    std::fs::create_dir(&target).unwrap();
    let e = Engine::open(root.path().join("meta")).await.unwrap();
    let d = e
        .create(f.url.clone(), root.path().into(), "file.bin".into())
        .await
        .unwrap();
    e.start(&d.id).await.unwrap();
    support::wait(&e, &d.id, |d| d.downloaded > 0).await;
    e.assign_category(&d.id, "archives").await.unwrap();
    let done = support::wait(&e, &d.id, |d| d.status == Status::Completed).await;
    assert!(done.ever_started);
    assert_eq!(done.category_id.as_deref(), Some("archives"));
    assert!(e.rename_download(&d.id, "no.bin").await.is_err());
    std::fs::write(target.join("file.bin"), b"collision").unwrap();
    assert!(e.move_completed(&d.id, target.clone()).await.is_err());
    assert_eq!(
        std::fs::read(target.join("file.bin")).unwrap(),
        b"collision"
    );
    std::fs::remove_file(target.join("file.bin")).unwrap();
    let moved = e.move_completed(&d.id, target.clone()).await.unwrap();
    assert!(!std::path::Path::new(&d.destination).exists());
    assert_eq!(
        std::fs::read(&moved.download.destination).unwrap(),
        *f.state.data
    );
    e.remove_record(&d.id).await.unwrap();
    assert!(target.join("file.bin").exists());
    e.shutdown().await.unwrap();
}

#[tokio::test]
async fn http_metadata_suggestions_are_advisory_and_actual_mime_updates_automatic_category() {
    use axum::{Router, http::header, routing::get};
    let router = Router::new().route(
        "/file",
        get(|| async {
            (
                [
                    (header::CONTENT_TYPE, "image/png"),
                    (header::ETAG, "\"image-v1\""),
                ],
                b"actual bytes".as_slice(),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/file", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let e = Engine::open(root.path().join("meta")).await.unwrap();
    let hint = e.inspect_source_category(&url, "opaque").await.unwrap();
    assert_eq!(hint.category_id, "images");
    assert_eq!(hint.total, Some(12));
    assert!(e.list().await.is_empty());
    let d = e
        .create(url, root.path().into(), "opaque".into())
        .await
        .unwrap();
    e.configure_download(
        &d.id,
        download_core::TransferConfig {
            max_connections: 1,
            optimize: false,
        },
    )
    .await
    .unwrap();
    e.start(&d.id).await.unwrap();
    let done = support::wait(&e, &d.id, |d| d.status == Status::Completed).await;
    assert_eq!(done.category_id.as_deref(), Some("images"));
    assert_eq!(done.destination, d.destination);
    e.shutdown().await.unwrap();
    task.abort();
}
#[tokio::test]
async fn v4_migration_preserves_records_and_rolls_back_on_conflicting_tables() {
    let root = tempfile::tempdir().unwrap();
    let meta = root.path().join("meta");
    let e = Engine::open(&meta).await.unwrap();
    let d = e
        .create(
            "https://example.org/a".into(),
            root.path().into(),
            "legacy.pdf".into(),
        )
        .await
        .unwrap();
    e.shutdown().await.unwrap();
    drop(e);
    let db = meta.join("downloads.sqlite3");
    let conn = rusqlite::Connection::open(&db).unwrap();
    let raw: String = conn
        .query_row("SELECT metadata FROM downloads", [], |r| r.get(0))
        .unwrap();
    let mut old: serde_json::Value = serde_json::from_str(&raw).unwrap();
    for key in [
        "category_id",
        "category_manual",
        "content_type",
        "ever_started",
    ] {
        old.as_object_mut().unwrap().remove(key);
    }
    conn.execute("UPDATE downloads SET metadata=?1", [old.to_string()])
        .unwrap();
    conn.execute_batch("DROP TABLE organization_state;DROP TABLE file_moves;PRAGMA user_version=4;CREATE TABLE file_moves(valuable TEXT);INSERT INTO file_moves VALUES('keep');").unwrap();
    drop(conn);
    assert!(Engine::open(&meta).await.is_err());
    let conn = rusqlite::Connection::open(&db).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        4
    );
    assert_eq!(
        conn.query_row("SELECT valuable FROM file_moves", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "keep"
    );
    conn.execute_batch("DROP TABLE file_moves").unwrap();
    drop(conn);
    let e = Engine::open(&meta).await.unwrap();
    let restored = e.get(&d.id).await.unwrap();
    assert_eq!(restored.category_id.as_deref(), Some("documents"));
    assert_eq!(restored.destination, d.destination);
    assert!(!restored.ever_started);
    assert!(std::path::Path::new(&d.partial_path).exists());
    e.shutdown().await.unwrap();
}

#[tokio::test]
async fn automatic_subfolders_are_previewed_without_io_and_used_once_for_scheduled_creation() {
    use download_core::{ScheduleKind, ScheduleRule};
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let e = Engine::open_with_clock(root.join("meta"), std::sync::Arc::new(OrganizationClock))
        .await
        .unwrap();
    let old = e
        .create(
            "https://example.org/old".into(),
            root.clone(),
            "old.mp4".into(),
        )
        .await
        .unwrap();
    assert!(!e.organization().await.enabled);
    e.set_organization_enabled(true).await.unwrap();
    let plan = e
        .suggest_destination("movie.mp4", root.to_str().unwrap(), None, None)
        .await
        .unwrap();
    assert_eq!(plan.directory, root.join("Videos").to_str().unwrap());
    assert!(!root.join("Videos").exists());
    assert!(
        e.duplicate_candidates(DuplicateQuery {
            url: "https://example.org/movie".into(),
            directory: plan.directory.clone(),
            filename: "movie.mp4".into(),
            total: None,
            etag: None
        })
        .await
        .unwrap()
        .is_empty()
    );
    assert!(!root.join("Videos").exists());
    let rule = ScheduleRule {
        id: String::new(),
        name: String::new(),
        enabled: true,
        kind: ScheduleKind::Download,
        download_id: None,
        at_ms: Some(1_800_000_000_000 + 86_400_000),
        weekdays: 127,
        start_minute: 540,
        end_minute: 600,
        timezone: "UTC".into(),
        bandwidth_limit: None,
        last_occurrence: None,
        last_local_day: None,
        revision: 0,
        note: None,
    };
    let settings = CreationSettings {
        use_category_directory: true,
        allow_duplicates: false,
        bandwidth_limit: Some(2_000_000),
        ..Default::default()
    };
    let scheduled = e
        .create_scheduled_with_settings(
            "https://example.org/movie".into(),
            root.clone(),
            "movie.mp4".into(),
            settings.clone(),
            rule,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        scheduled.destination,
        root.join("Videos/movie.mp4").to_str().unwrap()
    );
    assert!(root.join("Videos").is_dir());
    assert!(!root.join("Videos/Videos").exists());
    assert_eq!(scheduled.status, Status::Ready);
    assert_eq!(e.get(&old.id).await.unwrap().destination, old.destination);
    e.assign_category(&old.id, "music").await.unwrap();
    assert_eq!(e.get(&old.id).await.unwrap().destination, old.destination);
    let manual = root.join("Chosen");
    std::fs::create_dir(&manual).unwrap();
    let d = e
        .create_with_settings(
            "https://example.org/manual".into(),
            manual.clone(),
            "manual.mp4".into(),
            CreationSettings {
                use_category_directory: false,
                ..settings
            },
        )
        .await
        .unwrap();
    assert_eq!(d.destination, manual.join("manual.mp4").to_str().unwrap());
    assert!(!manual.join("Videos").exists());
    e.shutdown().await.unwrap();
    drop(e);
    let e = Engine::open_with_clock(root.join("meta"), std::sync::Arc::new(OrganizationClock))
        .await
        .unwrap();
    assert_eq!(
        e.get(&scheduled.id).await.unwrap().destination,
        scheduled.destination
    );
    assert_eq!(
        e.get(&scheduled.id).await.unwrap().bandwidth_limit,
        Some(2_000_000)
    );
    assert_eq!(e.scheduler_view().await.rules.len(), 1);
    e.shutdown().await.unwrap();
}
#[tokio::test]
async fn automatic_category_folder_collisions_never_overwrite_files_or_follow_symlinks() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let e = Engine::open(root.join("meta")).await.unwrap();
    e.set_organization_enabled(true).await.unwrap();
    std::fs::write(root.join("Videos"), b"keep existing file").unwrap();
    let settings = CreationSettings {
        use_category_directory: true,
        ..Default::default()
    };
    assert!(
        e.create_with_settings(
            "https://example.org/movie".into(),
            root.clone(),
            "movie.mp4".into(),
            settings.clone()
        )
        .await
        .is_err()
    );
    assert_eq!(
        std::fs::read(root.join("Videos")).unwrap(),
        b"keep existing file"
    );
    assert!(e.list().await.is_empty());
    #[cfg(unix)]
    {
        std::fs::remove_file(root.join("Videos")).unwrap();
        let outside = root.join("elsewhere");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("Videos")).unwrap();
        assert!(
            e.create_with_settings(
                "https://example.org/movie".into(),
                root.clone(),
                "movie.mp4".into(),
                settings
            )
            .await
            .is_err()
        );
        assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
    }
    e.shutdown().await.unwrap();
}
#[tokio::test]
async fn custom_category_subfolders_are_portable_and_explicit_rules_take_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let e = Engine::open(root.join("meta")).await.unwrap();
    let id = e
        .save_category(None, "../Outside".into(), None)
        .await
        .unwrap();
    e.set_organization_enabled(true).await.unwrap();
    let p = e
        .suggest_destination("file", root.to_str().unwrap(), Some(&id), None)
        .await
        .unwrap();
    assert_eq!(
        std::path::Path::new(&p.directory).parent(),
        Some(root.as_path())
    );
    assert!(
        std::path::Path::new(&p.directory)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("Category-")
    );
    let custom = root.join("Explicit");
    std::fs::create_dir(&custom).unwrap();
    e.save_category(Some(id.clone()), "Projects".into(), Some(custom.clone()))
        .await
        .unwrap();
    let settings = CreationSettings {
        category_id: Some(id),
        use_category_directory: true,
        ..Default::default()
    };
    let d = e
        .create_with_settings(
            "https://example.org/file".into(),
            root.clone(),
            "file.bin".into(),
            settings,
        )
        .await
        .unwrap();
    assert_eq!(d.destination, custom.join("file.bin").to_str().unwrap());
    assert!(!root.join("Projects").exists());
    e.shutdown().await.unwrap();
}
