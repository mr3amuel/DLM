use crate::model::PolicySettings;
use crate::{ByteRange, Download, Error, QueueConfig, Result};
use rusqlite::{Connection, params};
use std::{path::PathBuf, sync::mpsc};
use tokio::sync::oneshot;

type AutomationChange = (
    crate::automation::AutomationState,
    Vec<Download>,
    Option<PolicySettings>,
);
type OrganizationChange = (crate::organization::OrganizationConfig, Vec<Download>);
enum Command {
    Organization(
        Option<Box<OrganizationChange>>,
        oneshot::Sender<Result<crate::organization::OrganizationConfig>>,
    ),
    Moves(oneshot::Sender<Result<Vec<crate::file_operations::MoveJournal>>>),
    Move(
        Box<crate::file_operations::MoveJournal>,
        Option<Box<Download>>,
        bool,
        oneshot::Sender<Result<()>>,
    ),
    Automation(
        Option<Box<AutomationChange>>,
        oneshot::Sender<Result<crate::automation::AutomationState>>,
    ),
    Delete(String, oneshot::Sender<Result<()>>),
    Policy(
        Option<PolicySettings>,
        oneshot::Sender<Result<PolicySettings>>,
    ),
    Batch(Vec<Download>, oneshot::Sender<Result<()>>),
    Config(Option<QueueConfig>, oneshot::Sender<Result<QueueConfig>>),
    Load(oneshot::Sender<Result<Vec<Download>>>),
    Save(Box<Download>, oneshot::Sender<Result<()>>),
}

pub(crate) struct Storage {
    tx: mpsc::SyncSender<Command>,
}

impl Storage {
    pub async fn open(path: PathBuf) -> Result<Self> {
        let (tx, rx) = mpsc::sync_channel(32);
        let (ready_tx, ready_rx) = oneshot::channel();
        std::thread::Builder::new()
            .name("dlm-metadata".into())
            .spawn(move || {
                let connection = match open_connection(path) {
                    Ok(connection) => connection,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                for command in rx {
                    match command {
                        Command::Organization(value, reply) => {
                            let _ = reply.send(organization_settings(&connection, value.map(|v|*v)));
                        }
                        Command::Moves(reply) => {
                            let _ = reply.send(load_moves(&connection));
                        }
                        Command::Move(journal, record, remove, reply) => {
                            let _ = reply.send(save_move(&connection, *journal, record.map(|d|*d), remove));
                        }
                        Command::Automation(value, reply) => {
                            let result = (|| {
                                if let Some(value) = value {
                                    let (state, records, policy) = *value;
                                    let tx = connection.unchecked_transaction().map_err(storage_error)?;
                                    for d in records { save_download(&tx, &d)?; }
                                    if let Some(policy) = policy { policy_settings(&tx, Some(policy))?; }
                                    tx.execute("INSERT INTO automation_state(id,config) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET config=excluded.config", [serde_json::to_string(&state).map_err(storage_error)?]).map_err(storage_error)?;
                                    tx.commit().map_err(storage_error)?;
                                    Ok(state)
                                } else {
                                    use rusqlite::OptionalExtension;
                                    let value: Option<String> = connection.query_row("SELECT config FROM automation_state WHERE id=1", [], |r| r.get(0)).optional().map_err(storage_error)?;
                                    value.map(|v| serde_json::from_str(&v).map_err(storage_error)).unwrap_or_else(|| Ok(Default::default()))
                                }
                            })();
                            let _ = reply.send(result);
                        }
                        Command::Delete(id, reply) => {
                            let result = connection
                                .execute("DELETE FROM downloads WHERE id=?1", [id])
                                .map(|_| ())
                                .map_err(storage_error);
                            let _ = reply.send(result);
                        }
                        Command::Policy(value, reply) => {
                            let _ = reply.send(policy_settings(&connection, value));
                        }
                        Command::Batch(records, reply) => {
                            let result = (|| {
                                let transaction =
                                    connection.unchecked_transaction().map_err(storage_error)?;
                                for record in records {
                                    save_download(&transaction, &record)?;
                                }
                                transaction.commit().map_err(storage_error)
                            })();
                            let _ = reply.send(result);
                        }
                        Command::Config(config, reply) => {
                            let _ = reply.send(queue_config(&connection, config));
                        }
                        Command::Load(reply) => {
                            let _ = reply.send(load_downloads(&connection));
                        }
                        Command::Save(download, reply) => {
                            let _ = reply.send(save_download(&connection, &download));
                        }
                    }
                }
                let _ = connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
            })?;
        ready_rx.await.map_err(storage_error)??;
        Ok(Self { tx })
    }

    pub async fn organization(
        &self,
        value: Option<OrganizationChange>,
    ) -> Result<crate::organization::OrganizationConfig> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .try_send(Command::Organization(value.map(Box::new), tx))
            .map_err(storage_error)?;
        rx.await.map_err(storage_error)?
    }
    pub async fn moves(&self) -> Result<Vec<crate::file_operations::MoveJournal>> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .try_send(Command::Moves(tx))
            .map_err(storage_error)?;
        rx.await.map_err(storage_error)?
    }
    pub async fn move_record(
        &self,
        journal: crate::file_operations::MoveJournal,
        record: Option<Download>,
        remove: bool,
    ) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .try_send(Command::Move(
                Box::new(journal),
                record.map(Box::new),
                remove,
                tx,
            ))
            .map_err(storage_error)?;
        rx.await.map_err(storage_error)?
    }
    pub async fn automation(
        &self,
        value: Option<AutomationChange>,
    ) -> Result<crate::automation::AutomationState> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .try_send(Command::Automation(value.map(Box::new), tx))
            .map_err(storage_error)?;
        rx.await.map_err(storage_error)?
    }
    pub async fn delete(&self, id: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .try_send(Command::Delete(id.into(), tx))
            .map_err(storage_error)?;
        rx.await.map_err(storage_error)?
    }
    pub async fn config(&self, config: Option<QueueConfig>) -> Result<QueueConfig> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .try_send(Command::Config(config, tx))
            .map_err(storage_error)?;
        rx.await.map_err(storage_error)?
    }
    pub async fn policy(&self, value: Option<PolicySettings>) -> Result<PolicySettings> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .try_send(Command::Policy(value, tx))
            .map_err(storage_error)?;
        rx.await.map_err(storage_error)?
    }
    pub async fn save_batch(&self, records: Vec<Download>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .try_send(Command::Batch(records, tx))
            .map_err(storage_error)?;
        rx.await.map_err(storage_error)?
    }
    // try_send keeps bounded backpressure from blocking a Tokio worker.
    pub async fn load(&self) -> Result<Vec<Download>> {
        let (tx, rx) = oneshot::channel();
        self.tx.try_send(Command::Load(tx)).map_err(storage_error)?;
        rx.await.map_err(storage_error)?
    }

    pub async fn save(&self, download: &Download) -> Result<()> {
        let mut durable = download.clone();
        durable.downloaded = durable.durable_bytes;
        durable.speed = 0.0;
        let (tx, rx) = oneshot::channel();
        self.tx
            .try_send(Command::Save(Box::new(durable), tx))
            .map_err(storage_error)?;
        rx.await.map_err(storage_error)?
    }
}

fn open_connection(path: PathBuf) -> Result<Connection> {
    let connection = Connection::open(path).map_err(storage_error)?;
    connection
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
        .map_err(storage_error)?;
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(storage_error)?;
    match version {
        0 => connection
            .execute_batch(
                "BEGIN IMMEDIATE;
             CREATE TABLE downloads (id TEXT PRIMARY KEY, metadata TEXT NOT NULL);
             PRAGMA user_version=1;
             COMMIT;",
            )
            .map_err(storage_error)?,
        1 => {}
        2 => {}
        3 => {}
        4 => {}
        5 => {}
        other => {
            return Err(Error::Storage(format!(
                "Unsupported database schema version {other}"
            )));
        }
    }
    if version < 2 {
        let transaction = connection.unchecked_transaction().map_err(storage_error)?;
        transaction.execute_batch("CREATE TABLE engine_settings (id INTEGER PRIMARY KEY CHECK(id=1), config TEXT NOT NULL);").map_err(storage_error)?;
        let mut records = load_downloads(&transaction)?;
        for (index, d) in records.iter_mut().enumerate() {
            if d.durable_bytes > 0 {
                d.completed_ranges = vec![ByteRange {
                    start: 0,
                    end: d.durable_bytes,
                }];
            }
            d.queue_order = index as u64 + 1;
            save_download(&transaction, d)?;
        }
        transaction
            .execute_batch("PRAGMA user_version=2;")
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
    }
    if version < 3 {
        let transaction = connection.unchecked_transaction().map_err(storage_error)?;
        transaction.execute_batch("CREATE TABLE IF NOT EXISTS policy_settings (id INTEGER PRIMARY KEY CHECK(id=1), config TEXT NOT NULL);").map_err(storage_error)?;
        // Deserialize defaults and rewrite every record in the same migration transaction.
        // Existing validators, range ledgers, queue order and metadata are preserved.
        for record in load_downloads(&transaction)? {
            save_download(&transaction, &record)?;
        }
        transaction
            .execute_batch("PRAGMA user_version=3;")
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
    }
    if version < 4 {
        let tx = connection.unchecked_transaction().map_err(storage_error)?;
        tx.execute_batch("CREATE TABLE automation_state (id INTEGER PRIMARY KEY CHECK(id=1), config TEXT NOT NULL); PRAGMA user_version=4;").map_err(storage_error)?;
        // Validate old records before committing an irreversible schema advance.
        load_downloads(&tx)?;
        tx.commit().map_err(storage_error)?;
    }
    if version < 5 {
        let tx = connection.unchecked_transaction().map_err(storage_error)?;
        tx.execute_batch("CREATE TABLE organization_state(id INTEGER PRIMARY KEY CHECK(id=1),config TEXT NOT NULL); CREATE TABLE file_moves(id TEXT PRIMARY KEY,metadata TEXT NOT NULL); PRAGMA user_version=5;").map_err(storage_error)?;
        for mut d in load_downloads(&tx)? {
            d.ever_started = d.status != crate::Status::Ready
                || d.durable_bytes > 0
                || d.total.is_some()
                || d.etag.is_some();
            d.category_id =
                Some(crate::organization::classify(&d.filename, d.content_type.as_deref()).into());
            save_download(&tx, &d)?;
        }
        tx.commit().map_err(storage_error)?;
    }
    Ok(connection)
}

fn organization_settings(
    connection: &Connection,
    value: Option<OrganizationChange>,
) -> Result<crate::organization::OrganizationConfig> {
    use rusqlite::OptionalExtension;
    if let Some((config, records)) = value {
        let tx = connection.unchecked_transaction().map_err(storage_error)?;
        for d in records {
            save_download(&tx, &d)?;
        }
        tx.execute("INSERT INTO organization_state(id,config) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET config=excluded.config",[serde_json::to_string(&config).map_err(storage_error)?]).map_err(storage_error)?;
        tx.commit().map_err(storage_error)?;
        Ok(config)
    } else {
        let json: Option<String> = connection
            .query_row(
                "SELECT config FROM organization_state WHERE id=1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        json.map(|s| serde_json::from_str(&s).map_err(storage_error))
            .unwrap_or_else(|| Ok(Default::default()))
    }
}
fn load_moves(connection: &Connection) -> Result<Vec<crate::file_operations::MoveJournal>> {
    let mut statement = connection
        .prepare("SELECT metadata FROM file_moves ORDER BY id")
        .map_err(storage_error)?;
    let rows = statement
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(storage_error)?;
    rows.map(|r| serde_json::from_str(&r.map_err(storage_error)?).map_err(storage_error))
        .collect()
}
fn save_move(
    connection: &Connection,
    journal: crate::file_operations::MoveJournal,
    record: Option<Download>,
    remove: bool,
) -> Result<()> {
    let tx = connection.unchecked_transaction().map_err(storage_error)?;
    if let Some(d) = record {
        save_download(&tx, &d)?;
    }
    if remove {
        tx.execute("DELETE FROM file_moves WHERE id=?1", [&journal.id])
            .map_err(storage_error)?;
    } else {
        tx.execute("INSERT INTO file_moves(id,metadata) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET metadata=excluded.metadata",params![journal.id,serde_json::to_string(&journal).map_err(storage_error)?]).map_err(storage_error)?;
    }
    tx.commit().map_err(storage_error)
}

fn policy_settings(
    connection: &Connection,
    value: Option<PolicySettings>,
) -> Result<PolicySettings> {
    use rusqlite::OptionalExtension;
    if let Some(value) = value {
        connection.execute("INSERT INTO policy_settings(id,config) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET config=excluded.config", [serde_json::to_string(&value).map_err(storage_error)?]).map_err(storage_error)?;
        return Ok(value);
    }
    let json: Option<String> = connection
        .query_row("SELECT config FROM policy_settings WHERE id=1", [], |r| {
            r.get(0)
        })
        .optional()
        .map_err(storage_error)?;
    json.map(|s| serde_json::from_str(&s).map_err(storage_error))
        .unwrap_or_else(|| Ok(PolicySettings::default()))
}

fn load_downloads(connection: &Connection) -> Result<Vec<Download>> {
    let mut statement = connection
        .prepare("SELECT metadata FROM downloads ORDER BY rowid")
        .map_err(storage_error)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(storage_error)?;
    rows.map(|row| serde_json::from_str(&row.map_err(storage_error)?).map_err(storage_error))
        .collect()
}

fn save_download(connection: &Connection, download: &Download) -> Result<()> {
    let mut durable = download.clone();
    durable.downloaded = durable.durable_bytes;
    durable.speed = 0.0;
    durable.current_speed = 0.0;
    durable.smoothed_speed = 0.0;
    durable.eta_seconds = None;
    durable.retry_remaining_seconds = None;
    let json = serde_json::to_string(&durable).map_err(storage_error)?;
    connection
        .execute(
            "INSERT INTO downloads(id, metadata) VALUES (?1, ?2)
         ON CONFLICT(id) DO UPDATE SET metadata=excluded.metadata",
            params![download.id, json],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn storage_error(error: impl std::fmt::Display) -> Error {
    Error::Storage(error.to_string())
}

fn queue_config(connection: &Connection, value: Option<QueueConfig>) -> Result<QueueConfig> {
    if let Some(config) = value {
        config.validate()?;
        connection.execute("INSERT INTO engine_settings(id,config) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET config=excluded.config", [serde_json::to_string(&config).map_err(storage_error)?]).map_err(storage_error)?;
        return Ok(config);
    }
    use rusqlite::OptionalExtension;
    let json: Option<String> = connection
        .query_row("SELECT config FROM engine_settings WHERE id=1", [], |r| {
            r.get(0)
        })
        .optional()
        .map_err(storage_error)?;
    let config = match json {
        Some(json) => serde_json::from_str::<QueueConfig>(&json).map_err(storage_error)?,
        None => QueueConfig::default(),
    };
    config.validate()?;
    Ok(config)
}
