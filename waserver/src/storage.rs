//! Per-circle on-disk storage.
//!
//! `~/.config/waserver/circles/<circle>/` holds two files with two owners:
//! `circle.toml` (the user's; see `config`) and `state.db` (the daemon's:
//! node key material, registration, backend credentials).

use crate::config::{self, CircleConfig};
use rusqlite_migration::{M, Migrations};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tempfile::NamedTempFile;
use wispers_connect as wc;

const STATE_DB_FILENAME: &str = "state.db";

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Config(#[from] config::Error),
    #[error("state database: {0}")]
    Db(rusqlite::Error),
    #[error("state database migration: {0}")]
    Migration(rusqlite_migration::Error),
    #[error("invalid circle name '{0}' (use letters, digits, '-' or '_')")]
    InvalidName(String),
    #[error("circle {0} is not initialised")]
    NotInitialised(String),
    #[error("circle {0} already exists")]
    AlreadyExists(String),
    #[error("could not determine the OS config directory")]
    NoConfigDir,
}

/// Names of the initialised circles, unsorted.
pub fn list_circles() -> Result<Vec<String>, Error> {
    let dir = circles_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    Ok(fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join(config::FILENAME).is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect())
}

//-- Circle directory ----------------------------------------------------------

#[derive(Clone)]
pub struct CircleDir {
    name: String,
    dir: PathBuf,
}

impl CircleDir {
    /// Validates the name and computes the path, but touches nothing on disk.
    pub fn new(name: &str) -> Result<Self, Error> {
        if !config::is_valid_id(name) {
            return Err(Error::InvalidName(name.to_owned()));
        }
        Ok(Self {
            name: name.to_owned(),
            dir: circles_dir()?.join(name),
        })
    }

    pub fn config_path(&self) -> PathBuf {
        self.dir.join(config::FILENAME)
    }

    pub fn exists(&self) -> bool {
        self.config_path().is_file()
    }

    pub fn load_config(&self) -> Result<CircleConfig, Error> {
        if !self.exists() {
            return Err(Error::NotInitialised(self.name.clone()));
        }
        Ok(CircleConfig::load(&self.config_path())?)
    }

    /// Creates the directory, writes `circle.toml` and creates an empty `state.db`.
    pub fn create(&self, config_text: &str) -> Result<StateDb, Error> {
        if self.dir.exists() {
            return Err(Error::AlreadyExists(self.name.clone()));
        }
        ensure_dir_exists(&self.dir)?;
        write_atomically(&self.dir, config::FILENAME, config_text.as_bytes())?;
        StateDb::open(self.dir.join(STATE_DB_FILENAME))
    }

    /// Opens the existing `state.db`.
    pub fn open_state(&self) -> Result<StateDb, Error> {
        let path = self.dir.join(STATE_DB_FILENAME);
        if !self.exists() || !path.is_file() {
            return Err(Error::NotInitialised(self.name.clone()));
        }
        StateDb::open(path)
    }

    pub fn delete(&self) -> Result<(), Error> {
        match fs::remove_dir_all(&self.dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::Io(e)),
        }
    }
}

//-- State database ------------------------------------------------------------

/// The daemon-owned SQLite file. Cheap to clone. Clones share one connection.
#[derive(Clone)]
pub struct StateDb {
    conn: Arc<Mutex<rusqlite::Connection>>,
}

/// What the Wispers Connect transport needs to talk to its backend.
#[derive(Clone)]
pub struct WispersConnectState {
    pub api_key: String,
    pub connectivity_group_id: String,
}

const KEY_API_KEY: &str = "api_key";
const KEY_CONNECTIVITY_GROUP_ID: &str = "connectivity_group_id";
const KEY_ROOT_KEY: &str = "root_key";
const KEY_REGISTRATION: &str = "registration";

impl StateDb {
    fn open(path: PathBuf) -> Result<Self, Error> {
        let mut conn = rusqlite::Connection::open(&path).map_err(Error::Db)?;
        // The daemon and the CLI (or two CLI tasks) may open the file at the
        // same time; wait for a short lock instead of failing.
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(Error::Db)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(Error::Db)?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(Error::Db)?;
        migrations()
            .to_latest(&mut conn)
            .map_err(Error::Migration)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn wispers_connect_state(&self) -> Result<Option<WispersConnectState>, Error> {
        let (Some(api_key), Some(cg_id)) = (
            self.get_string(KEY_API_KEY)?,
            self.get_string(KEY_CONNECTIVITY_GROUP_ID)?,
        ) else {
            return Ok(None);
        };
        Ok(Some(WispersConnectState {
            api_key,
            connectivity_group_id: cg_id,
        }))
    }

    pub fn set_wispers_connect_state(&self, state: &WispersConnectState) -> Result<(), Error> {
        self.set(KEY_API_KEY, state.api_key.as_bytes())?;
        self.set(
            KEY_CONNECTIVITY_GROUP_ID,
            state.connectivity_group_id.as_bytes(),
        )
    }

    fn get_string(&self, key: &str) -> Result<Option<String>, Error> {
        Ok(self
            .get(key)?
            .map(|v| String::from_utf8_lossy(&v).into_owned()))
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, Error> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().expect("unpoisoned db lock");
        conn.query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .map_err(Error::Db)
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), Error> {
        let conn = self.conn.lock().expect("unpoisoned db lock");
        conn.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )
        .map_err(Error::Db)?;
        Ok(())
    }

    fn remove(&self, key: &str) -> Result<(), Error> {
        let conn = self.conn.lock().expect("unpoisoned db lock");
        conn.execute("DELETE FROM kv WHERE key = ?1", [key])
            .map_err(Error::Db)?;
        Ok(())
    }
}

/// wispers-connect keeps the node's root key and registration here.
impl wc::NodeStateStore for StateDb {
    fn load(&self) -> Result<Option<wc::PersistedNodeState>, wc::StorageError> {
        let Some(root_key) = self.get(KEY_ROOT_KEY).map_err(store_error)? else {
            return Ok(None);
        };
        let key_array: [u8; wc::ROOT_KEY_LEN] = root_key
            .try_into()
            .map_err(|_| wc::StorageError::InvalidRootKey)?;
        let registration = match self.get(KEY_REGISTRATION).map_err(store_error)? {
            Some(bytes) => Some(wc::deserialize_registration(&bytes)?),
            None => None,
        };
        Ok(Some(wc::PersistedNodeState::from_stored(
            key_array,
            registration,
        )))
    }

    fn save(&self, state: &wc::PersistedNodeState) -> Result<(), wc::StorageError> {
        self.set(KEY_ROOT_KEY, state.root_key_bytes())
            .map_err(store_error)?;
        match state.registration() {
            Some(reg) => self
                .set(KEY_REGISTRATION, &wc::serialize_registration(reg))
                .map_err(store_error),
            None => self.remove(KEY_REGISTRATION).map_err(store_error),
        }
    }

    fn delete(&self) -> Result<(), wc::StorageError> {
        self.remove(KEY_ROOT_KEY).map_err(store_error)?;
        self.remove(KEY_REGISTRATION).map_err(store_error)
    }
}

fn store_error(e: Error) -> wc::StorageError {
    wc::StorageError::Io(io::Error::other(e))
}

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        // v1 — daemon-owned blobs: node key material, registration, backend
        // credentials.
        M::up("CREATE TABLE kv (key TEXT PRIMARY KEY, value BLOB NOT NULL) STRICT;"),
    ])
}

//-- Paths and file helpers ----------------------------------------------------

fn circles_dir() -> Result<PathBuf, Error> {
    Ok(base_dir()?.join("circles"))
}

fn base_dir() -> Result<PathBuf, Error> {
    let dir = dirs::config_dir()
        .ok_or(Error::NoConfigDir)?
        .join("waserver");
    Ok(dir)
}

fn ensure_dir_exists(dir: &PathBuf) -> Result<(), io::Error> {
    if !dir.exists() {
        fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

fn write_atomically(dir: &PathBuf, name: &str, data: &[u8]) -> Result<(), io::Error> {
    let path = dir.join(name);
    let mut tmp = NamedTempFile::new_in(dir)?;
    tmp.write_all(data)?;
    tmp.as_file().sync_all()?;
    tmp.persist(&path).map_err(|e| e.error)?;
    #[cfg(unix)]
    {
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wc::NodeStateStore;

    #[test]
    fn migrations_are_valid() {
        assert!(migrations().validate().is_ok());
    }

    #[test]
    fn state_db_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let db = StateDb::open(tmp.path().join("state.db")).unwrap();
        assert!(db.wispers_connect_state().unwrap().is_none());
        assert!(db.load().unwrap().is_none());

        db.set_wispers_connect_state(&WispersConnectState {
            api_key: "wc_test_k.secret".to_owned(),
            connectivity_group_id: "cg-1".to_owned(),
        })
        .unwrap();
        let wcs = db.wispers_connect_state().unwrap().unwrap();
        assert_eq!(wcs.api_key, "wc_test_k.secret");
        assert_eq!(wcs.connectivity_group_id, "cg-1");

        let state = wc::PersistedNodeState::from_stored([7u8; wc::ROOT_KEY_LEN], None);
        db.save(&state).unwrap();
        let loaded = db.load().unwrap().unwrap();
        assert_eq!(loaded.root_key_bytes(), &[7u8; wc::ROOT_KEY_LEN]);
        assert!(loaded.registration().is_none());

        db.delete().unwrap();
        assert!(db.load().unwrap().is_none());
        // Backend credentials survive a node reset.
        assert!(db.wispers_connect_state().unwrap().is_some());

        // Reopening runs no migration twice and sees the data.
        drop(db);
        let db = StateDb::open(tmp.path().join("state.db")).unwrap();
        assert!(db.wispers_connect_state().unwrap().is_some());
    }
}
