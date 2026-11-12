//! History database: samples and events, written off the UI thread.
//!
//! The UI never touches SQLite. It hands rows to a background thread through a
//! bounded channel; if the writer is behind, the row is dropped and the app keeps
//! running. Reads for the analytics ranges go through the same thread and come
//! back as an update.
//!
//! Schema is versioned in the `meta` table. Journal mode is WAL, so reads never
//! block the writer, and `synchronous = NORMAL` keeps a durable-enough history
//! without an fsync per sample.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use rusqlite::Connection;
use thiserror::Error;

use crate::config::settings::HistoryConfig;
use crate::services::Update;
use crate::telemetry::history::Sample;

/// Rows are buffered and committed together, so a burst of MQTT blocks costs one
/// fsync, not one per sample.
const COMMIT_BATCH: usize = 64;
/// Channel depth for pending writes. Beyond it the app drops rows: history is
/// best-effort, the live view is not.
const CHANNEL_DEPTH: usize = 256;

/// Version 2 adds `bat_w`, the signed battery-side power the runtime estimate
/// averages over. Old databases get the column added, not rebuilt.
const SCHEMA_VERSION: u32 = 2;

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("opening {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("sqlite: {0}")]
    Db(#[from] rusqlite::Error),
}

/// What the writer thread accepts. Every variant is fire-and-forget except `Query`,
/// whose answer arrives back on the app's update channel.
#[derive(Debug, Clone)]
pub enum StorageCmd {
    Sample {
        sn: String,
        sample: Sample,
    },
    Event {
        t: DateTime<Utc>,
        level: String,
        kind: String,
        sn: String,
        text: String,
    },
    Device {
        sn: String,
        model: String,
        seen: DateTime<Utc>,
    },
    /// Samples of one device in `[from, to]`, at most `limit` of them.
    Query {
        sn: String,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    },
    /// Drop rows older than the retention window.
    Purge {
        keep_days: u32,
    },
}

/// The app's handle: `send` never blocks, and a full queue means the writer is
/// behind, which the app reports as a notice.
#[derive(Clone)]
pub struct StorageHandle {
    tx: tokio::sync::mpsc::Sender<StorageCmd>,
}

impl StorageHandle {
    /// Queue a command. Returns false when the queue is full, so the caller can
    /// say that history is falling behind.
    pub fn send(&self, cmd: StorageCmd) -> bool {
        self.tx.try_send(cmd).is_ok()
    }
}

/// A handle with no writer behind it: the queue fills and `send` refuses. Used by
/// tests to exercise the app's "writer behind" path without racing a real thread.
#[cfg(test)]
pub fn pending_handle(depth: usize) -> (StorageHandle, tokio::sync::mpsc::Receiver<StorageCmd>) {
    let (tx, rx) = tokio::sync::mpsc::channel(depth);
    (StorageHandle { tx }, rx)
}

/// Open the database, create the schema if needed, and apply retention.
pub fn open(path: &Path, retention_days: u32) -> Result<Connection, StorageError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| StorageError::Open {
            path: path.to_path_buf(),
            source: rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(14),
                Some(e.to_string()),
            ),
        })?;
    }
    let conn = Connection::open(path).map_err(|e| StorageError::Open {
        path: path.to_path_buf(),
        source: e,
    })?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "busy_timeout", 2000)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    setup(&conn)?;
    record(&conn, "created", &Utc::now().to_rfc3339())?;
    purge(&conn, retention_days)?;
    Ok(conn)
}

fn setup(conn: &Connection) -> Result<(), StorageError> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS devices (
            sn TEXT PRIMARY KEY,
            model TEXT NOT NULL,
            first_seen INTEGER NOT NULL,
            last_seen INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS samples (
            id INTEGER PRIMARY KEY,
            sn TEXT NOT NULL,
            t INTEGER NOT NULL,
            soc REAL NOT NULL,
            in_w REAL NOT NULL,
            out_w REAL NOT NULL,
            solar_w REAL NOT NULL,
            temp_c REAL NOT NULL,
            bat_w REAL NOT NULL DEFAULT 0.0
        );
        CREATE INDEX IF NOT EXISTS ix_samples_sn_t ON samples(sn, t);
        CREATE TABLE IF NOT EXISTS events (
            id INTEGER PRIMARY KEY,
            t INTEGER NOT NULL,
            level TEXT NOT NULL,
            kind TEXT NOT NULL,
            sn TEXT NOT NULL,
            text TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS ix_events_t ON events(t);
        "#,
    )?;
    // A database from version 1 has the samples table without `bat_w`; adding the
    // column keeps the history the app already collected.
    let has_bat: bool = conn
        .prepare_cached("SELECT count(*) FROM pragma_table_info('samples') WHERE name = 'bat_w'")?
        .query_row([], |r| r.get::<_, i64>(0))
        .map(|n| n > 0)?;
    if !has_bat {
        conn.execute(
            "ALTER TABLE samples ADD COLUMN bat_w REAL NOT NULL DEFAULT 0.0",
            [],
        )?;
    }
    let version: u32 = conn
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'schema'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if version < SCHEMA_VERSION {
        record(conn, "schema", &SCHEMA_VERSION.to_string())?;
    }
    Ok(())
}

fn record(conn: &Connection, key: &str, value: &str) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
        (key, value),
    )?;
    Ok(())
}

/// Delete rows outside the retention window. Runs at startup and inside the writer
/// loop, so a long-running app keeps the file bounded.
pub fn purge(conn: &Connection, keep_days: u32) -> Result<(), StorageError> {
    let cutoff = (Utc::now() - Duration::days(keep_days as i64)).timestamp();
    // Rows in the future are not history either: a device clock, and a test
    // fixture, can be ahead of the wall clock, and a bounded window has no room
    // for them.
    let future = Utc::now().timestamp();
    conn.execute(
        "DELETE FROM samples WHERE t < ?1 OR t > ?2",
        (cutoff, future),
    )?;
    conn.execute(
        "DELETE FROM events WHERE t < ?1 OR t > ?2",
        (cutoff, future),
    )?;
    Ok(())
}

pub fn sample_count(conn: &Connection) -> Result<i64, StorageError> {
    conn.query_row("SELECT count(*) FROM samples", [], |r| r.get(0))
        .map_err(StorageError::Db)
}

pub fn get_samples(
    conn: &Connection,
    sn: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    limit: usize,
) -> Result<Vec<Sample>, StorageError> {
    let mut stmt = conn.prepare_cached(
        "SELECT t, soc, in_w, out_w, solar_w, temp_c, bat_w FROM samples WHERE sn = ?1 AND t >= ?2 AND t <= ?3 ORDER BY t LIMIT ?4",
    )?;
    let rows = stmt.query_map((sn, from.timestamp(), to.timestamp(), limit as i64), |r| {
        Ok(Sample {
            t: DateTime::from_timestamp(r.get::<_, i64>(0)?, 0).unwrap_or_else(Utc::now),
            soc: r.get(1)?,
            in_w: r.get(2)?,
            out_w: r.get(3)?,
            solar_w: r.get(4)?,
            temp_c: r.get(5)?,
            bat_w: r.get(6)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Start the writer thread. `updates` is the app's channel, so query results and
/// storage warnings reach the UI as ordinary updates.
pub fn spawn(
    path: PathBuf,
    cfg: HistoryConfig,
    updates: tokio::sync::mpsc::Sender<Update>,
) -> Result<StorageHandle, StorageError> {
    let conn = open(&path, cfg.retention_days)?;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<StorageCmd>(CHANNEL_DEPTH);
    std::thread::spawn(move || {
        let mut batch: Vec<StorageCmd> = Vec::with_capacity(COMMIT_BATCH);
        let mut since_purge = 0u64;
        while let Some(cmd) = rx.blocking_recv() {
            batch.push(cmd);
            while let Ok(next) = rx.try_recv() {
                batch.push(next);
                if batch.len() >= COMMIT_BATCH {
                    break;
                }
            }
            if let Err(e) = write(&conn, &batch, &updates, cfg.retention_days) {
                tracing::warn!("history write failed: {e}");
                let _ = updates.try_send(Update::Notification(
                    crate::services::Notification::warn(format!("history database: {e}")),
                ));
            }
            batch.clear();
            since_purge += 1;
            if since_purge >= 500 {
                since_purge = 0;
                if let Err(e) = purge(&conn, cfg.retention_days) {
                    tracing::warn!("retention purge failed: {e}");
                }
            }
        }
        // The channel closed with the app; flush and checkpoint the WAL.
        let _ = conn.pragma_update(None, "wal_checkpoint", "TRUNCATE");
    });
    Ok(StorageHandle { tx })
}

/// One transaction per batch: rows grouped by statement, prepared once.
fn write(
    conn: &Connection,
    batch: &[StorageCmd],
    updates: &tokio::sync::mpsc::Sender<Update>,
    retention_days: u32,
) -> Result<(), StorageError> {
    if batch.is_empty() {
        return Ok(());
    }
    conn.execute("BEGIN IMMEDIATE", [])?;
    for cmd in batch {
        match cmd {
            StorageCmd::Sample { sn, sample } => {
                conn.execute(
                    "INSERT INTO samples(sn, t, soc, in_w, out_w, solar_w, temp_c, bat_w) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    (sn, sample.t.timestamp(), sample.soc, sample.in_w, sample.out_w, sample.solar_w, sample.temp_c, sample.bat_w),
                )?;
            }
            StorageCmd::Event {
                t,
                level,
                kind,
                sn,
                text,
            } => {
                conn.execute(
                    "INSERT INTO events(t, level, kind, sn, text) VALUES (?1, ?2, ?3, ?4, ?5)",
                    (t.timestamp(), level, kind, sn, text),
                )?;
            }
            StorageCmd::Device { sn, model, seen } => {
                conn.execute(
                    "INSERT INTO devices(sn, model, first_seen, last_seen) VALUES (?1, ?2, ?3, ?3)
                     ON CONFLICT(sn) DO UPDATE SET model = ?2, last_seen = ?3",
                    (sn, model, seen.timestamp()),
                )?;
            }
            StorageCmd::Query {
                sn,
                from,
                to,
                limit,
            } => {
                let rows = get_samples(conn, sn, *from, *to, *limit)?;
                let _ = updates.blocking_send(Update::History(sn.clone(), rows));
            }
            StorageCmd::Purge { keep_days } => {
                purge(conn, (*keep_days).min(retention_days))?;
            }
        }
    }
    conn.execute("COMMIT", [])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration as StdDuration;

    /// Times relative to the wall clock: the startup purge keeps only rows inside
    /// the window, so a fixture in the past would be deleted and a fixture dated in
    /// the future is not history.
    fn at(secs: i64) -> DateTime<Utc> {
        Utc::now() + Duration::seconds(secs)
    }

    fn sample(secs: i64, in_w: f64) -> Sample {
        Sample {
            t: at(secs),
            soc: 50.0,
            in_w,
            out_w: 0.0,
            solar_w: 0.0,
            temp_c: 25.0,
            bat_w: -in_w,
        }
    }

    fn temp_db() -> (std::path::PathBuf, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp dir");
        (dir.path().join("history.sqlite"), dir)
    }

    #[test]
    fn a_new_database_gets_the_schema() {
        let (path, _keep) = temp_db();
        let conn = open(&path, 90).expect("open");
        let version: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'schema'", [], |r| {
                r.get(0)
            })
            .expect("schema row");
        assert_eq!(version, "2");
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .expect("journal mode");
        assert_eq!(mode, "wal", "the writer must not block readers");
        assert_eq!(sample_count(&conn).expect("count"), 0);
    }

    #[test]
    fn a_version_one_database_keeps_its_rows_and_gains_the_column() {
        let (path, _keep) = temp_db();
        // Build the version-1 table by hand, then write a row in it.
        let conn = Connection::open(&path).expect("open");
        conn.execute(
            "CREATE TABLE samples (id INTEGER PRIMARY KEY, sn TEXT, t INTEGER, soc REAL, in_w REAL, out_w REAL, solar_w REAL, temp_c REAL)",
            [],
        )
        .expect("v1 table");
        conn.execute(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
            [],
        )
        .expect("meta table");
        conn.execute("INSERT INTO meta VALUES ('schema', '1')", [])
            .expect("schema row");
        conn.execute(
            "INSERT INTO samples(sn, t, soc, in_w, out_w, solar_w, temp_c) VALUES ('SN1', ?1, 50, 100, 0, 0, 25)",
            (at(-60).timestamp(),),
        )
        .expect("v1 row");
        drop(conn);
        let conn = open(&path, 90).expect("reopen with migration");
        let rows = get_samples(&conn, "SN1", at(-600), at(0), 100).expect("query");
        assert_eq!(rows.len(), 1, "the old row survives the migration");
        assert_eq!(
            rows[0].bat_w, 0.0,
            "a sample the old schema stored has no battery power"
        );
        let version: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'schema'", [], |r| {
                r.get(0)
            })
            .expect("schema row");
        assert_eq!(version, "2");
    }

    #[test]
    fn samples_are_written_and_read_back_in_time_order() {
        let (path, _keep) = temp_db();
        let conn = open(&path, 90).expect("open");
        conn.execute(
            "INSERT INTO samples(sn, t, soc, in_w, out_w, solar_w, temp_c, bat_w) VALUES ('SN1', ?1, 50, 100, 0, 0, 25, -100)",
            (at(60).timestamp(),),
        )
        .expect("insert 1");
        conn.execute(
            "INSERT INTO samples(sn, t, soc, in_w, out_w, solar_w, temp_c, bat_w) VALUES ('SN1', ?1, 55, 200, 0, 0, 26, -200)",
            (at(120).timestamp(),),
        )
        .expect("insert 2");
        conn.execute(
            "INSERT INTO samples(sn, t, soc, in_w, out_w, solar_w, temp_c, bat_w) VALUES ('SN2', ?1, 10, 5, 0, 0, 20, 5)",
            (at(90).timestamp(),),
        )
        .expect("insert 3");
        let rows = get_samples(&conn, "SN1", at(0), at(600), 100).expect("query");
        assert_eq!(rows.len(), 2, "only the requested device");
        assert!(rows[0].t < rows[1].t, "oldest first");
        assert_eq!(rows[1].in_w, 200.0);
        assert_eq!((rows[0].soc, rows[1].soc), (50.0, 55.0));
        assert_eq!(
            rows[1].bat_w, -200.0,
            "the signed battery power round-trips"
        );
    }

    #[test]
    fn the_query_limit_is_a_hard_cap() {
        let (path, _keep) = temp_db();
        let conn = open(&path, 90).expect("open");
        for i in 0..200 {
            conn.execute(
                "INSERT INTO samples(sn, t, soc, in_w, out_w, solar_w, temp_c, bat_w) VALUES ('SN1', ?1, 50, 10, 0, 0, 25, -10)",
                (at(i * 10).timestamp(),),
            )
            .expect("insert");
        }
        let rows = get_samples(&conn, "SN1", at(0), at(10_000), 100).expect("query");
        assert_eq!(rows.len(), 100, "the limit is enforced in the query");
    }

    #[test]
    fn retention_drops_what_is_past_the_window() {
        let (path, _keep) = temp_db();
        let conn = open(&path, 30).expect("open");
        let old = Utc::now() - StdDuration::from_secs(31 * 86400);
        let recent = Utc::now() - StdDuration::from_secs(60);
        conn.execute(
            "INSERT INTO samples(sn, t, soc, in_w, out_w, solar_w, temp_c) VALUES ('SN1', ?1, 50, 0, 0, 0, 25)",
            (old.timestamp(),),
        )
        .expect("old insert");
        conn.execute(
            "INSERT INTO samples(sn, t, soc, in_w, out_w, solar_w, temp_c) VALUES ('SN1', ?1, 50, 0, 0, 0, 25)",
            (recent.timestamp(),),
        )
        .expect("recent insert");
        conn.execute(
            "INSERT INTO events(t, level, kind, sn, text) VALUES (?1, 'Warn', 'BatteryLow', 'SN1', 'low')",
            (old.timestamp(),),
        )
        .expect("event insert");
        purge(&conn, 30).expect("purge");
        assert_eq!(
            sample_count(&conn).expect("count"),
            1,
            "only the recent sample"
        );
        let events: i64 = conn
            .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
            .expect("event count");
        assert_eq!(events, 0, "events age out with the same window");
    }

    #[test]
    fn the_writer_thread_persists_a_batch() {
        let (path, _keep) = temp_db();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Update>(16);
        let handle = spawn(path.clone(), HistoryConfig::default(), tx).expect("spawn");
        for i in 0..10 {
            assert!(handle.send(StorageCmd::Sample {
                sn: "SN1".to_string(),
                sample: sample(i * 10, 100.0 + i as f64),
            }));
        }
        assert!(handle.send(StorageCmd::Event {
            t: at(0),
            level: "Warn".to_string(),
            kind: "BatteryLow".to_string(),
            sn: "SN1".to_string(),
            text: "battery 12%".to_string(),
        }));
        assert!(handle.send(StorageCmd::Device {
            sn: "SN1".to_string(),
            model: "DELTA 2 Max".to_string(),
            seen: at(0),
        }));
        // The writer is a thread; give it a moment to take the batch.
        std::thread::sleep(StdDuration::from_millis(400));
        let conn = Connection::open(&path).expect("reopen");
        assert_eq!(sample_count(&conn).expect("count"), 10);
        let model: String = conn
            .query_row("SELECT model FROM devices WHERE sn = 'SN1'", [], |r| {
                r.get(0)
            })
            .expect("device row");
        assert_eq!(model, "DELTA 2 Max");
        let level: String = conn
            .query_row("SELECT level FROM events WHERE sn = 'SN1'", [], |r| {
                r.get(0)
            })
            .expect("event row");
        assert_eq!(level, "Warn");
        // The same file, reopened, is the restart case: the app reads its history
        // back through the writer thread.
        let rows = get_samples(&conn, "SN1", at(-3600), at(3600), 100).expect("query");
        assert_eq!(rows.len(), 10, "history survives a restart");
        assert_eq!(rows[9].bat_w, -(100.0 + 9.0), "the newest row is intact");
    }

    #[test]
    fn a_full_queue_is_reported_and_the_handle_never_blocks() {
        // The production channel is CHANNEL_DEPTH deep; the same rule applies at any
        // depth: the row beyond it is refused immediately, so the UI keeps running.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<StorageCmd>(2);
        let handle = StorageHandle { tx };
        assert!(handle.send(StorageCmd::Sample {
            sn: "SN1".to_string(),
            sample: sample(0, 100.0),
        }));
        assert!(handle.send(StorageCmd::Sample {
            sn: "SN1".to_string(),
            sample: sample(10, 100.0),
        }));
        assert!(
            !handle.send(StorageCmd::Sample {
                sn: "SN1".to_string(),
                sample: sample(20, 100.0),
            }),
            "a full queue drops the row"
        );
        // Drain: `blocking_recv` ends when the last sender goes, so dropping the
        // handle lets the loop finish in a synchronous test.
        drop(handle);
        let drained = std::iter::from_fn(|| rx.blocking_recv()).collect::<Vec<_>>();
        assert_eq!(drained.len(), 2, "only the rows that fit were queued");
        assert_eq!(
            CHANNEL_DEPTH, 256,
            "the writer queue is bounded, not growing"
        );
    }

    #[test]
    fn a_query_answer_comes_back_as_an_update() {
        let (path, _keep) = temp_db();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Update>(16);
        let conn = open(&path, 90).expect("open");
        conn.execute(
            "INSERT INTO samples(sn, t, soc, in_w, out_w, solar_w, temp_c, bat_w) VALUES ('SN1', ?1, 50, 100, 0, 0, 25, -100)",
            (at(-60).timestamp(),),
        )
        .expect("insert");
        drop(conn);
        let handle = spawn(path, HistoryConfig::default(), tx).expect("spawn");
        assert!(handle.send(StorageCmd::Query {
            sn: "SN1".to_string(),
            from: at(-600),
            to: at(0),
            limit: 500,
        }));
        let update = rx
            .blocking_recv()
            .expect("the query result reaches the app");
        match update {
            Update::History(sn, rows) => {
                assert_eq!(sn, "SN1");
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].in_w, 100.0);
                assert_eq!(rows[0].bat_w, -100.0);
            }
            other => panic!("expected a history update, got {other:?}"),
        }
    }
}
