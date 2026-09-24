//! The SDK's store: one SQLite file per client.

use crate::{Share, ShareState};
use anyhow::Result;
use rusqlite_migration::Migrations;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use wire::{App, AppKind, ConfigHash, ShareInfo};
use wispers_access_wire as wire;

pub struct DB {
    conn: Mutex<rusqlite::Connection>,
}

/// Device-local ID of a share.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ShareId(String);

impl ShareId {
    pub(crate) fn mint() -> Self {
        ShareId(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

uniffi::custom_type!(ShareId, String, {
    lower: |id| id.0,
    try_lift: |s| Ok(ShareId(s)),
});

impl std::fmt::Display for ShareId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl DB {
    /// Opens `state.db` under `dir`, creating both as needed.
    pub fn open(dir: &Path) -> Result<Arc<Self>> {
        let conn = open_db(dir)?;
        let db = DB {
            conn: Mutex::new(conn),
        };
        Ok(Arc::new(db))
    }

    pub fn new_row(self: &Arc<Self>) -> Result<Row> {
        let t = now_millis();
        let id: i64;
        {
            let conn = self.conn.lock().expect("unpoisoned db lock");
            conn.execute(
                "INSERT INTO shares (share_id, created_at) VALUES (?1, ?2)",
                rusqlite::params![ShareId::mint().as_str(), t],
            )?;
            id = conn.last_insert_rowid();
        }
        Ok(Row {
            db: self.clone(),
            id,
        })
    }

    /// Looks a share up by its hostname label (or share id).
    pub fn find_row(self: &Arc<Self>, key: &str) -> Result<Option<Row>> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().expect("unpoisoned db lock");
        let id = conn
            .query_row(
                "SELECT id FROM shares
                 WHERE complete = TRUE AND (hostname = ?1 OR share_id = ?1)",
                [key],
                |r| r.get::<_, i64>(0),
            )
            .optional()?;
        Ok(id.map(|id| Row {
            db: self.clone(),
            id,
        }))
    }

    pub fn get_all_rows(self: &Arc<Self>) -> Result<Vec<Row>> {
        let conn = self.conn.lock().expect("unpoisoned db lock");
        let mut stmt = conn.prepare("SELECT id FROM shares WHERE complete = TRUE")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Row {
                    db: self.clone(),
                    id: r.get(0)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

/// One joined share.
#[derive(Clone)]
pub struct Row {
    db: Arc<DB>,
    id: i64,
}

impl Row {
    pub fn share_id(&self) -> Result<ShareId> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        let id = conn.query_row(
            "SELECT share_id FROM shares WHERE id = ?1",
            [self.id],
            |r| r.get::<_, String>(0),
        )?;
        Ok(ShareId(id))
    }

    pub fn write_display_name(&self, name: &str) -> Result<()> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        conn.execute(
            "UPDATE shares SET display_name = ?1 WHERE id = ?2",
            rusqlite::params![name, self.id],
        )?;
        Ok(())
    }

    /// Claims `hostname` as this share's label, or `hostname-2`, `-3`, … if
    /// taken; falls back to the share id.
    pub fn write_deduped_hostname(&self, hostname: &str) -> Result<String> {
        let share_id = self.share_id()?;
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        for n in 1.. {
            let candidate: String = if n == 1 {
                hostname.to_owned()
            } else {
                format!("{}-{}", hostname, n)
            };
            if candidate.len() > 63 {
                // Max DNS label exceeded.
                break;
            }
            match conn.execute(
                "UPDATE shares SET hostname = ?1 WHERE id = ?2",
                rusqlite::params![candidate, self.id],
            ) {
                Ok(_) => return Ok(candidate),
                Err(e) if is_unique_violation(&e) => continue,
                Err(e) => return Err(e.into()),
            }
        }
        // Normal deduping has failed. Just use the share id.
        conn.execute(
            "UPDATE shares SET hostname = ?1 WHERE id = ?2",
            rusqlite::params![share_id.as_str(), self.id],
        )?;
        Ok(share_id.to_string())
    }

    /// (share id, display name, hostname label)
    pub fn read_names(&self) -> Result<(ShareId, String, String)> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        let row = conn.query_row(
            "SELECT share_id, display_name, hostname
                 FROM shares
                 WHERE id = ?1",
            [self.id],
            |row| {
                Ok((
                    ShareId(row.get::<_, String>(0)?),
                    row.get::<_, Option<String>>(1)?.unwrap_or("".to_owned()),
                    row.get::<_, Option<String>>(2)?.unwrap_or("".to_owned()),
                ))
            },
        )?;
        Ok(row)
    }

    /// Persists the self-hosted hub a Wispers Connect share uses, if any.
    pub fn write_wispers_connect_backend(&self, backend: Option<&str>) -> Result<()> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        conn.execute(
            "UPDATE shares SET wispers_connect_backend = ?1 WHERE id = ?2",
            rusqlite::params![backend, self.id],
        )?;
        Ok(())
    }

    /// Records that this share rides iroh, and which endpoint its host node
    /// is. The device's key for it lives in the secret store.
    pub fn write_iroh_endpoint_id(&self, host_endpoint_id: &str) -> Result<()> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        conn.execute(
            "UPDATE shares SET transport = 'iroh', iroh_endpoint_id = ?1 WHERE id = ?2",
            rusqlite::params![host_endpoint_id, self.id],
        )?;
        Ok(())
    }

    /// The host node's endpoint ID, in hex, for an iroh share.
    pub fn read_iroh_endpoint_id(&self) -> Result<Option<String>> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        let host = conn.query_row(
            "SELECT iroh_endpoint_id FROM shares WHERE id = ?1",
            [self.id],
            |r| r.get::<_, Option<String>>(0),
        )?;
        Ok(host)
    }

    /// The transport this share rides.
    pub fn read_transport_kind(&self) -> Result<wire::Transport> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        let name: String = conn.query_row(
            "SELECT transport FROM shares WHERE id = ?1",
            [self.id],
            |r| r.get(0),
        )?;
        Ok(name.parse()?)
    }

    pub fn read_wispers_connect_backend(&self) -> Result<Option<String>> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        let backend = conn.query_row(
            "SELECT wispers_connect_backend FROM shares WHERE id = ?1",
            [self.id],
            |r| r.get::<_, Option<String>>(0),
        )?;
        Ok(backend)
    }

    pub fn write_share_info(&self, info: &ShareInfo) -> Result<()> {
        let mut conn = self.db.conn.lock().expect("unpoisoned db lock");
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE shares SET display_name = ?1, config_hash = ?2 WHERE id = ?3",
            rusqlite::params![info.name, info.config_hash.to_string(), self.id],
        )?;
        tx.execute("DELETE FROM apps WHERE share_id = ?1", [self.id])?;
        for (position, app) in info.apps.iter().enumerate() {
            tx.execute(
                "INSERT INTO apps (share_id, position, app_id, name, kind)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    self.id,
                    position as i64,
                    app.id,
                    app.name,
                    app.kind.as_str()
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The config hash the stored app list came with, for a conditional
    /// refetch. `None` before the first fetch.
    pub fn read_share_config_hash(&self) -> Result<Option<ConfigHash>> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        let hash = conn.query_row(
            "SELECT config_hash FROM shares WHERE id = ?1",
            [self.id],
            |r| r.get::<_, Option<String>>(0),
        )?;
        Ok(hash.and_then(|h| h.parse().ok()))
    }

    /// The apps as last fetched, in the host node's order.
    pub fn read_apps(&self) -> Result<Vec<App>> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        let mut stmt = conn
            .prepare("SELECT app_id, name, kind FROM apps WHERE share_id = ?1 ORDER BY position")?;
        let apps = stmt
            .query_map([self.id], |r| {
                Ok(App {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    kind: AppKind::parse_or_web(&r.get::<_, String>(2)?),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(apps)
    }

    pub fn mark_complete(&self) -> Result<()> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        conn.execute("UPDATE shares SET complete = TRUE WHERE id = ?1", [self.id])?;
        Ok(())
    }

    /// Records that the hub definitively rejected this share's node. One-way:
    /// a terminal share is never dialed again, only removed.
    pub fn write_terminal_state(&self, state: &str) -> Result<()> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        conn.execute(
            "UPDATE shares SET terminal_state = ?1 WHERE id = ?2",
            rusqlite::params![state, self.id],
        )?;
        Ok(())
    }

    /// When this device joined the share.
    pub fn read_joined_at(&self) -> Result<SystemTime> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        let millis: i64 = conn.query_row(
            "SELECT created_at FROM shares WHERE id = ?1",
            [self.id],
            |r| r.get(0),
        )?;
        Ok(UNIX_EPOCH + Duration::from_millis(millis.max(0) as u64))
    }

    /// The share as the SDK's API presents it.
    pub fn read_share(&self) -> Result<Share> {
        let (id, name, label) = self.read_names()?;
        let state = match self
            .read_terminal_state()?
            .as_deref()
            .and_then(crate::transports::TerminalState::parse)
        {
            None => ShareState::Live,
            Some(crate::transports::TerminalState::Removed) => ShareState::Removed,
            Some(crate::transports::TerminalState::Revoked) => ShareState::Revoked,
        };
        Ok(Share {
            id,
            name,
            label,
            transport: self.read_transport_kind()?,
            apps: self.read_apps()?,
            state,
            joined_at: self.read_joined_at()?,
        })
    }

    pub fn read_terminal_state(&self) -> Result<Option<String>> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        let state = conn.query_row(
            "SELECT terminal_state FROM shares WHERE id = ?1",
            [self.id],
            |r| r.get::<_, Option<String>>(0),
        )?;
        Ok(state)
    }

    /// The port a `PerApp` proxy last served this app on.
    pub fn read_app_port(&self, app_id: &str) -> Result<Option<u16>> {
        use rusqlite::OptionalExtension;
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        let port = conn
            .query_row(
                "SELECT port FROM app_ports WHERE share_id = ?1 AND app_id = ?2",
                rusqlite::params![self.id, app_id],
                |r| r.get::<_, u16>(0),
            )
            .optional()?;
        Ok(port)
    }

    pub fn write_app_port(&self, app_id: &str, port: u16) -> Result<()> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        conn.execute(
            "INSERT INTO app_ports (share_id, app_id, port) VALUES (?1, ?2, ?3)
             ON CONFLICT (share_id, app_id) DO UPDATE SET port = excluded.port",
            rusqlite::params![self.id, app_id, port],
        )?;
        Ok(())
    }

    /// Deletes the share and its apps outright. Its secrets are the secret
    /// store's to delete.
    pub fn delete_row(&self) -> Result<()> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        conn.execute("DELETE FROM shares WHERE id = ?1", [self.id])?;
        Ok(())
    }
}

fn is_unique_violation(e: &rusqlite::Error) -> bool {
    use rusqlite::{Error, ErrorCode};
    matches!(
        e,
        Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: ErrorCode::ConstraintViolation,
                ..
            },
            _
        )
    )
}

fn open_db(dir: &Path) -> Result<rusqlite::Connection> {
    let db_path = dir.join("state.db");
    fs::create_dir_all(dir)?;
    let conn = rusqlite::Connection::open(db_path)?;
    prepare(conn)
}

/// Everything a freshly opened connection needs before use: the pragmas,
/// the schema at its latest version, and the sweep of rows a failed `join`
/// left behind. Shared with the in-memory database the tests use.
fn prepare(mut conn: rusqlite::Connection) -> Result<rusqlite::Connection> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrations().to_latest(&mut conn)?;
    clean_up_incomplete_rows(&mut conn)?;
    Ok(conn)
}

fn migrations() -> Migrations<'static> {
    use rusqlite_migration::M;

    Migrations::new(vec![
        // v1 — one row per joined share (which transport, how to reach the
        // host node, and what it said the share is), one row per app in it.
        // Key material is not here but in the secret store. The schema of
        // the unreleased waclient builds that kept keys in this file was
        // dropped without a migration.
        M::up(
            "CREATE TABLE shares (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 share_id TEXT NOT NULL UNIQUE,
                 display_name TEXT,
                 hostname TEXT UNIQUE,
                 transport TEXT NOT NULL DEFAULT 'wispers-connect',
                 wispers_connect_backend TEXT,
                 iroh_endpoint_id TEXT,
                 config_hash TEXT,
                 created_at INTEGER NOT NULL,
                 complete INTEGER,
                 terminal_state TEXT
             ) STRICT;
             CREATE TABLE apps (
                 share_id INTEGER NOT NULL REFERENCES shares(id) ON DELETE CASCADE,
                 position INTEGER NOT NULL,
                 app_id TEXT NOT NULL,
                 name TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 PRIMARY KEY (share_id, app_id)
             ) STRICT;
             CREATE TABLE app_ports (
                 share_id INTEGER NOT NULL REFERENCES shares(id) ON DELETE CASCADE,
                 app_id TEXT NOT NULL,
                 port INTEGER NOT NULL,
                 PRIMARY KEY (share_id, app_id)
             ) STRICT;",
        ),
    ])
}

fn clean_up_incomplete_rows(conn: &mut rusqlite::Connection) -> Result<()> {
    conn.execute("DELETE FROM shares WHERE complete = FALSE", [])?;
    Ok(())
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_set_is_valid() {
        migrations().validate().unwrap();
    }

    fn in_memory() -> Arc<DB> {
        let conn = prepare(rusqlite::Connection::open_in_memory().unwrap()).unwrap();
        Arc::new(DB {
            conn: Mutex::new(conn),
        })
    }

    #[test]
    fn share_info_round_trips_and_replaces() {
        let db = in_memory();
        let row = db.new_row().unwrap();
        assert!(row.read_share_config_hash().unwrap().is_none());
        assert!(row.read_apps().unwrap().is_empty());

        let info = ShareInfo {
            config_hash: ConfigHash(7),
            name: "Family".into(),
            transport: "wispers-connect".into(),
            apps: vec![
                App {
                    id: "jf".into(),
                    name: "Jellyfin".into(),
                    kind: AppKind::Jellyfin,
                },
                App {
                    id: "photos".into(),
                    name: "Photos".into(),
                    kind: AppKind::Web,
                },
            ],
        };
        row.write_share_info(&info).unwrap();
        assert_eq!(row.read_share_config_hash().unwrap(), Some(ConfigHash(7)));
        assert_eq!(row.read_apps().unwrap(), info.apps);
        assert_eq!(row.read_names().unwrap().1, "Family");

        // A later fetch replaces the list wholesale, order included.
        let later = ShareInfo {
            config_hash: ConfigHash(8),
            apps: vec![info.apps[1].clone()],
            ..info
        };
        row.write_share_info(&later).unwrap();
        assert_eq!(row.read_share_config_hash().unwrap(), Some(ConfigHash(8)));
        assert_eq!(row.read_apps().unwrap(), later.apps);

        // Deleting the share takes its apps with it.
        row.write_deduped_hostname("family").unwrap();
        row.mark_complete().unwrap();
        assert!(db.find_row("family").unwrap().is_some());
        row.delete_row().unwrap();
        assert!(db.find_row("family").unwrap().is_none());
        let orphans: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM apps", [], |r| r.get(0))
            .unwrap();
        assert_eq!(orphans, 0);
    }

    #[test]
    fn hostnames_dedupe() {
        let db = in_memory();
        let a = db.new_row().unwrap();
        let b = db.new_row().unwrap();
        assert_ne!(a.share_id().unwrap(), b.share_id().unwrap());
        assert_eq!(a.write_deduped_hostname("family").unwrap(), "family");
        assert_eq!(b.write_deduped_hostname("family").unwrap(), "family-2");
        // Lookup by share id, once the row is complete.
        let id = a.share_id().unwrap();
        assert!(db.find_row(id.as_str()).unwrap().is_none());
        a.mark_complete().unwrap();
        assert!(db.find_row(id.as_str()).unwrap().is_some());
    }
}
