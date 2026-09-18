//! Persistent storage for waclient.

use anyhow::{Context, Result};
use rusqlite_migration::Migrations;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use wire::{CircleInfo, ConfigHash, Share, ShareKind};
use wispers_access_wire as wire;
use wispers_connect as wc;

pub struct DB {
    conn: Mutex<rusqlite::Connection>,
}

impl DB {
    pub fn new() -> Result<Arc<Self>> {
        let conn = open_db()?;
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
            conn.execute("INSERT INTO circles (created_at) VALUES (?1)", [t])?;
            id = conn.last_insert_rowid();
        }
        Ok(Row {
            db: self.clone(),
            id,
        })
    }

    /// Looks a circle up by its hostname label (or connectivity group id).
    pub fn find_row(self: &Arc<Self>, key: &str) -> Result<Option<Row>> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().expect("unpoisoned db lock");
        let id = conn
            .query_row(
                "SELECT id FROM circles
                 WHERE complete = TRUE AND (hostname = ?1 OR connectivity_group_id = ?1)",
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
        let mut stmt = conn.prepare("SELECT id FROM circles WHERE complete = TRUE")?;
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

/// One joined circle.
#[derive(Clone)]
pub struct Row {
    db: Arc<DB>,
    id: i64,
}

impl Row {
    pub fn write_connectivity_group_id(&self, cg_id: &str) -> Result<()> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        conn.execute(
            "UPDATE circles SET connectivity_group_id = ?1 WHERE id = ?2",
            rusqlite::params![cg_id, self.id],
        )?;
        Ok(())
    }

    pub fn write_display_name(&self, name: &str) -> Result<()> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        conn.execute(
            "UPDATE circles SET display_name = ?1 WHERE id = ?2",
            rusqlite::params![name, self.id],
        )?;
        Ok(())
    }

    /// Claims `hostname` as this circle's label, or `hostname-2`, `-3`, … if
    /// taken; falls back to the connectivity group id.
    pub fn write_deduped_hostname(&self, hostname: &str, cg_id: &str) -> Result<String> {
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
                "UPDATE circles SET hostname = ?1 WHERE id = ?2",
                rusqlite::params![candidate, self.id],
            ) {
                Ok(_) => return Ok(candidate),
                Err(e) if is_unique_violation(&e) => continue,
                Err(e) => return Err(e.into()),
            }
        }
        // Normal deduping has failed. Just use the connectivity group ID.
        conn.execute(
            "UPDATE circles SET hostname = ?1 WHERE id = ?2",
            rusqlite::params![cg_id, self.id],
        )?;
        Ok(cg_id.to_owned())
    }

    /// (connectivity group id, display name, hostname label)
    pub fn read_names(&self) -> Result<(String, String, String)> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        let row = conn.query_row(
            "SELECT connectivity_group_id, display_name, hostname
                 FROM circles
                 WHERE id = ?1",
            [self.id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?.unwrap_or("".to_owned()),
                    row.get::<_, Option<String>>(1)?.unwrap_or("".to_owned()),
                    row.get::<_, Option<String>>(2)?.unwrap_or("".to_owned()),
                ))
            },
        )?;
        Ok(row)
    }

    /// Persist the circle's custom backend base URL, if any.
    pub fn write_backend(&self, backend: Option<&str>) -> Result<()> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        conn.execute(
            "UPDATE circles SET backend = ?1 WHERE id = ?2",
            rusqlite::params![backend, self.id],
        )?;
        Ok(())
    }

    pub fn read_backend(&self) -> Result<Option<String>> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        let backend = conn.query_row(
            "SELECT backend FROM circles WHERE id = ?1",
            [self.id],
            |r| r.get::<_, Option<String>>(0),
        )?;
        Ok(backend)
    }

    pub fn write_circle_info(&self, info: &CircleInfo) -> Result<()> {
        let mut conn = self.db.conn.lock().expect("unpoisoned db lock");
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE circles SET display_name = ?1, config_hash = ?2 WHERE id = ?3",
            rusqlite::params![info.name, info.config_hash.to_string(), self.id],
        )?;
        tx.execute("DELETE FROM shares WHERE circle_id = ?1", [self.id])?;
        for (position, share) in info.shares.iter().enumerate() {
            tx.execute(
                "INSERT INTO shares (circle_id, position, share_id, name, kind)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    self.id,
                    position as i64,
                    share.id,
                    share.name,
                    share.kind.as_str()
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The config hash the stored share list came with, for a conditional
    /// refetch. `None` before the first fetch.
    pub fn read_circle_config_hash(&self) -> Result<Option<ConfigHash>> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        let hash = conn.query_row(
            "SELECT config_hash FROM circles WHERE id = ?1",
            [self.id],
            |r| r.get::<_, Option<String>>(0),
        )?;
        Ok(hash.and_then(|h| h.parse().ok()))
    }

    /// The shares as last fetched, in the server's order.
    pub fn read_shares(&self) -> Result<Vec<Share>> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        let mut stmt = conn.prepare(
            "SELECT share_id, name, kind FROM shares WHERE circle_id = ?1 ORDER BY position",
        )?;
        let shares = stmt
            .query_map([self.id], |r| {
                Ok(Share {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    kind: parse_share_kind(&r.get::<_, String>(2)?),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(shares)
    }

    pub fn mark_complete(&self) -> Result<()> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        conn.execute(
            "UPDATE circles SET complete = TRUE WHERE id = ?1",
            [self.id],
        )?;
        Ok(())
    }

    /// Records that the hub definitively rejected this circle's node. One-way:
    /// a terminal circle is never dialed again, only removed.
    pub fn write_terminal_state(&self, state: &str) -> Result<()> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        conn.execute(
            "UPDATE circles SET terminal_state = ?1 WHERE id = ?2",
            rusqlite::params![state, self.id],
        )?;
        Ok(())
    }

    pub fn read_terminal_state(&self) -> Result<Option<String>> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        let state = conn.query_row(
            "SELECT terminal_state FROM circles WHERE id = ?1",
            [self.id],
            |r| r.get::<_, Option<String>>(0),
        )?;
        Ok(state)
    }

    /// Deletes the circle and its shares outright (unlike
    /// [wc::NodeStateStore::delete], which only clears the node state). For
    /// `waclient remove`.
    pub fn delete_row(&self) -> Result<()> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        conn.execute("DELETE FROM circles WHERE id = ?1", [self.id])?;
        Ok(())
    }
}

/// A kind this build does not know reads as `web`: the server may be newer.
fn parse_share_kind(s: &str) -> ShareKind {
    serde_json::from_value(serde_json::Value::String(s.to_owned())).unwrap_or_default()
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

impl wc::NodeStateStore for Row {
    fn load(&self) -> Result<Option<wc::PersistedNodeState>, wc::StorageError> {
        let db = self
            .db
            .conn
            .lock()
            .map_err(|_| wc::StorageError::Poisoned)?;
        let row = db
            .query_row(
                "SELECT root_key, registration FROM circles WHERE id = ?1",
                [self.id],
                |r| {
                    Ok((
                        r.get::<_, Option<Vec<u8>>>(0)?,
                        r.get::<_, Option<Vec<u8>>>(1)?,
                    ))
                },
            )
            .map_err(to_wc_error)?;
        let (Some(rk), reg) = row else {
            // There's no root key, conclude nothing has been saved yet.
            return Ok(None);
        };
        let key: [u8; wc::ROOT_KEY_LEN] = rk
            .try_into()
            .map_err(|_| wc::StorageError::InvalidRootKey)?;
        let reg = reg.and_then(|b| wc::deserialize_registration(&b).ok());
        Ok(Some(wc::PersistedNodeState::from_stored(key, reg)))
    }

    fn save(&self, state: &wc::PersistedNodeState) -> Result<(), wc::StorageError> {
        let conn = self
            .db
            .conn
            .lock()
            .map_err(|_| wc::StorageError::Poisoned)?;
        let registration: Option<Vec<u8>> = state.registration().map(wc::serialize_registration);
        let n = conn
            .execute(
                "UPDATE circles SET root_key = ?1, registration = ?2 WHERE id = ?3",
                rusqlite::params![
                    // Convert &[u8; 32] -> &[u8] (BLOB).
                    state.root_key_bytes().as_slice(),
                    registration,
                    self.id,
                ],
            )
            .map_err(to_wc_error)?;
        // The row is pre-INSERTed in new_row(). 0 means the row vanished
        // underneath us => a logic error worth surfacing.
        if n == 0 {
            return Err(wc::StorageError::Io(std::io::Error::other(format!(
                "no circles row with id {}",
                self.id
            ))));
        }
        Ok(())
    }

    fn delete(&self) -> Result<(), wc::StorageError> {
        let conn = self
            .db
            .conn
            .lock()
            .map_err(|_| wc::StorageError::Poisoned)?;
        conn.execute(
            "UPDATE circles
             SET root_key = NULL, registration = NULL
             WHERE id = ?1",
            [self.id],
        )
        .map_err(to_wc_error)?;
        Ok(())
    }
}

fn to_wc_error(e: rusqlite::Error) -> wc::StorageError {
    wc::StorageError::Io(std::io::Error::other(e))
}

fn open_db() -> Result<rusqlite::Connection> {
    let dir = base_dir()?;
    let db_path = dir.join("circles.db");
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

fn base_dir() -> Result<PathBuf> {
    let config_dir = dirs::config_dir().context("could not determine config directory")?;
    Ok(config_dir.join("waclient"))
}

fn migrations() -> Migrations<'static> {
    use rusqlite_migration::M;

    Migrations::new(vec![
        // v1 — one row per joined circle (the node's identity and what the
        // server said the circle is), one row per share in it.
        M::up(
            "CREATE TABLE circles (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 connectivity_group_id TEXT,
                 display_name TEXT,
                 hostname TEXT UNIQUE,
                 backend TEXT,
                 config_hash TEXT,
                 created_at INTEGER NOT NULL,
                 root_key BLOB,
                 registration BLOB,
                 complete INTEGER,
                 terminal_state TEXT
             ) STRICT;
             CREATE TABLE shares (
                 circle_id INTEGER NOT NULL REFERENCES circles(id) ON DELETE CASCADE,
                 position INTEGER NOT NULL,
                 share_id TEXT NOT NULL,
                 name TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 PRIMARY KEY (circle_id, share_id)
             ) STRICT;",
        ),
    ])
}

fn clean_up_incomplete_rows(conn: &mut rusqlite::Connection) -> Result<()> {
    conn.execute("DELETE FROM circles WHERE complete = FALSE", [])?;
    Ok(())
}

fn now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
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
    fn circle_info_round_trips_and_replaces() {
        let db = in_memory();
        let row = db.new_row().unwrap();
        assert!(row.read_circle_config_hash().unwrap().is_none());
        assert!(row.read_shares().unwrap().is_empty());

        let info = CircleInfo {
            config_hash: ConfigHash(7),
            name: "Family".into(),
            transport: "wispers-connect".into(),
            shares: vec![
                Share {
                    id: "jf".into(),
                    name: "Jellyfin".into(),
                    kind: ShareKind::Jellyfin,
                },
                Share {
                    id: "photos".into(),
                    name: "Photos".into(),
                    kind: ShareKind::Web,
                },
            ],
        };
        row.write_circle_info(&info).unwrap();
        assert_eq!(row.read_circle_config_hash().unwrap(), Some(ConfigHash(7)));
        assert_eq!(row.read_shares().unwrap(), info.shares);
        assert_eq!(row.read_names().unwrap().1, "Family");

        // A later fetch replaces the list wholesale, order included.
        let later = CircleInfo {
            config_hash: ConfigHash(8),
            shares: vec![info.shares[1].clone()],
            ..info
        };
        row.write_circle_info(&later).unwrap();
        assert_eq!(row.read_circle_config_hash().unwrap(), Some(ConfigHash(8)));
        assert_eq!(row.read_shares().unwrap(), later.shares);

        // Deleting the circle takes its shares with it.
        row.write_deduped_hostname("family", "cg").unwrap();
        row.mark_complete().unwrap();
        assert!(db.find_row("family").unwrap().is_some());
        row.delete_row().unwrap();
        assert!(db.find_row("family").unwrap().is_none());
        let orphans: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM shares", [], |r| r.get(0))
            .unwrap();
        assert_eq!(orphans, 0);
    }

    #[test]
    fn hostnames_dedupe() {
        let db = in_memory();
        let a = db.new_row().unwrap();
        let b = db.new_row().unwrap();
        assert_eq!(
            a.write_deduped_hostname("family", "cg-a").unwrap(),
            "family"
        );
        assert_eq!(
            b.write_deduped_hostname("family", "cg-b").unwrap(),
            "family-2"
        );
    }
}
