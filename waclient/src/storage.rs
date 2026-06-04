//! Persistent storage for waclient.

use anyhow::{Context, Result};
use rusqlite;
use rusqlite_migration::Migrations;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
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
            conn.execute("INSERT INTO shares (created_at) VALUES (?1)", [t])?;
            id = conn.last_insert_rowid();
        }
        Ok(Row {
            db: self.clone(),
            id,
        })
    }

    pub fn get_all_rows(self: &Arc<Self>) -> Result<Vec<Row>> {
        let conn = self.conn.lock().expect("unpoisoned db lock");
        let mut stmt = conn.prepare("SELECT id FROM shares WHERE complete = TRUE")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Row {
                    // [] = no params; closure maps &Row -> T
                    db: self.clone(),
                    id: r.get(0)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[derive(Clone)]
pub struct Row {
    db: Arc<DB>,
    id: i64,
}

impl Row {
    pub fn write_names(&self, cg_id: &str, display_name: &str, hostname: &str) -> Result<()> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        conn.execute(
            "UPDATE shares SET
                 connectivity_group_id = ?1,
                 display_name = ?2,
                 hostname = ?3
             WHERE id = ?4",
            rusqlite::params![cg_id, display_name, hostname, self.id],
        )?;
        Ok(())
    }

    pub fn read_names(&self) -> Result<(String, String, String)> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        let row = conn.query_row(
            "SELECT connectivity_group_id, display_name, hostname
                 FROM shares
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

    pub fn mark_complete(&self) -> Result<()> {
        let conn = self.db.conn.lock().expect("unpoisoned db lock");
        conn.execute("UPDATE shares SET complete = TRUE WHERE id = ?1", [self.id])?;
        Ok(())
    }
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
                "SELECT root_key, registration FROM shares WHERE id = ?1",
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
                "UPDATE shares SET root_key = ?1, registration = ?2 WHERE id = ?3",
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
                "no shares row with id {}",
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
            "UPDATE shares
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
    let db_path = dir.join("shares.db");
    fs::create_dir_all(dir)?;
    let mut conn = rusqlite::Connection::open(db_path)?;
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
        // v1 — the schema we designed becomes migration #1
        M::up(
            "CREATE TABLE shares (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 connectivity_group_id TEXT,
                 display_name TEXT,
                 hostname TEXT UNIQUE,
                 created_at INTEGER NOT NULL,
                 root_key BLOB,
                 registration BLOB,
                 complete INTEGER
             ) STRICT;",
        ),
    ])
}

fn clean_up_incomplete_rows(conn: &mut rusqlite::Connection) -> Result<()> {
    conn.execute("DELETE FROM shares WHERE complete = FALSE", [])?;
    Ok(())
}

fn now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_millis() as i64
}
