use download_core::{Engine, Error};

#[tokio::test]
async fn startup_reports_corrupt_database_without_replacing_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("downloads.sqlite3");
    let original = b"This is not a SQLite database";
    std::fs::write(&db, original).unwrap();
    let error = match Engine::open(dir.path()).await {
        Ok(_) => panic!("Corrupt database must fail"),
        Err(error) => error,
    };
    assert!(matches!(&error, Error::Storage(_)));
    assert!(error.to_string().contains("not a database"), "{error}");
    assert_eq!(std::fs::read(&db).unwrap(), original);
}

#[tokio::test]
async fn failed_v3_migration_rolls_back_and_can_be_reopened_after_repair() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("downloads.sqlite3");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(
        "CREATE TABLE downloads(id TEXT PRIMARY KEY, metadata TEXT NOT NULL);
        CREATE TABLE engine_settings(id INTEGER PRIMARY KEY, config TEXT NOT NULL);
        INSERT INTO downloads VALUES('broken','invalid json'); PRAGMA user_version=2;",
    )
    .unwrap();
    let error = match Engine::open(dir.path()).await {
        Ok(_) => panic!("Invalid metadata must fail migration"),
        Err(error) => error,
    };
    assert!(matches!(error, Error::Storage(_)));
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='policy_settings'",
            [],
            |r| r.get::<_, u32>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT metadata FROM downloads", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "invalid json"
    );
    // Repair only this disposable fixture, then prove the failed startup released its lock.
    conn.execute("DELETE FROM downloads", []).unwrap();
    drop(conn);
    let engine = Engine::open(dir.path()).await.unwrap();
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn startup_rejects_newer_schema_and_non_directory_path_with_causes() {
    let dir = tempfile::tempdir().unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("downloads.sqlite3")).unwrap();
    conn.execute_batch("PRAGMA user_version=99;").unwrap();
    drop(conn);
    let error = match Engine::open(dir.path()).await {
        Ok(_) => panic!("Newer schema must fail"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("schema version 99"));
    let file = dir.path().join("regular-file");
    std::fs::write(&file, "preserve me").unwrap();
    assert!(matches!(Engine::open(&file).await, Err(Error::Disk(_))));
    assert_eq!(std::fs::read_to_string(file).unwrap(), "preserve me");
}
