//! Per-circle on-disk storage.
//!
//! `~/.config/waserver/circles/<circle>/` holds two files with two owners:
//! `circle.toml` (the user's, see `config`) and `state.db` (the daemon's).

use crate::config::{self, CircleConfig};
use rusqlite_migration::{M, Migrations};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tempfile::NamedTempFile;
use wispers_access_wire as wire;
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
    #[error("no guest node number {0}")]
    NoSuchGuest(i64),
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

//-- Invites and guest nodes ---------------------------------------------------

/// An invite to be recorded.
#[allow(dead_code)] // the iroh transport is the first caller
pub struct NewInvite<'a> {
    /// Only its SHA-256 is stored, so a copied database grants no access;
    /// the secret is high-entropy random, so no slow hash is needed.
    pub secret: &'a wire::InviteSecret,
    /// The label the app sees in the identity header.
    pub user_id: &'a str,
    /// What the host calls this guest's device.
    pub display_name: &'a str,
    pub expires_at: i64,
}

/// A guest's node: a transport identity bound to this circle by redeeming
/// an invite. A revoked node keeps its row so its key is never bound again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestNode {
    /// How the CLI addresses it (`waserver revoke <circle> <number>`).
    pub number: i64,
    /// The transport's stable identifier for the node, if used (Wispers Connect
    /// doesn't).
    pub peer_id: String,
    /// The label the app sees in the identity header.
    pub user_id: String,
    /// What the host calls this guest's device.
    pub display_name: String,
    pub activated_at: i64,
    pub last_seen_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

/// The outcome of redeeming an invite.
#[derive(Debug, PartialEq, Eq)]
pub enum Redemption {
    Activated(GuestNode),
    Refused(wire::ActivationError),
}

/// Times are Unix seconds; callers pass `now` so the rules are testable.
#[allow(dead_code)] // the iroh transport is the first caller
impl StateDb {
    /// Records an invite, returns its id.
    pub fn create_invite(&self, invite: NewInvite<'_>, now: i64) -> Result<i64, Error> {
        let conn = self.conn.lock().expect("unpoisoned db lock");
        conn.execute(
            "INSERT INTO invites (secret_hash, user_id, display_name, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                hash_secret(invite.secret),
                invite.user_id,
                invite.display_name,
                now,
                invite.expires_at
            ],
        )
        .map_err(Error::Db)?;
        Ok(conn.last_insert_rowid())
    }

    /// Redeem the invite code, binding `peer_id` to the metadata (user ID, node
    /// name, etc) the operator added when generating the invite. The binding
    /// happens at most once. Two guests racing for one invite serialise and the
    /// second is refused. The same peer redeeming the same invite again
    /// indicates a lost response being retried, and we return the same data
    /// again.
    pub fn redeem_invite(
        &self,
        secret: &wire::InviteSecret,
        peer_id: &str,
        now: i64,
    ) -> Result<Redemption, Error> {
        use rusqlite::OptionalExtension;
        use wire::ActivationError::*;
        let secret_hash = hash_secret(secret);
        let mut conn = self.conn.lock().expect("unpoisoned db lock");
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(Error::Db)?;

        let existing: Option<(GuestNode, Vec<u8>)> = tx
            .query_row(
                &format!(
                    "SELECT {GUEST_COLUMNS}, i.secret_hash FROM guests g
                     JOIN invites i ON i.id = g.invite_id WHERE g.peer_id = ?1"
                ),
                [peer_id],
                |r| Ok((guest_from_row(r)?, r.get(7)?)),
            )
            .optional()
            .map_err(Error::Db)?;
        if let Some((guest, bound_hash)) = existing {
            return Ok(if guest.revoked_at.is_some() {
                Redemption::Refused(Revoked)
            } else if bound_hash == secret_hash {
                Redemption::Activated(guest)
            } else {
                Redemption::Refused(AlreadyMember)
            });
        }

        let invite: Option<(i64, String, String, i64, Option<i64>)> = tx
            .query_row(
                "SELECT id, user_id, display_name, expires_at, consumed_at
                 FROM invites WHERE secret_hash = ?1",
                [&secret_hash[..]],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()
            .map_err(Error::Db)?;
        let Some((invite_id, user_id, display_name, expires_at, consumed_at)) = invite else {
            return Ok(Redemption::Refused(InviteUnknown));
        };
        if consumed_at.is_some() {
            return Ok(Redemption::Refused(InviteConsumed));
        }
        if expires_at <= now {
            return Ok(Redemption::Refused(InviteExpired));
        }

        tx.execute(
            "INSERT INTO guests (peer_id, user_id, display_name, invite_id, activated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![peer_id, user_id, display_name, invite_id, now],
        )
        .map_err(Error::Db)?;
        let number = tx.last_insert_rowid();
        tx.execute(
            "UPDATE invites SET consumed_at = ?1 WHERE id = ?2",
            rusqlite::params![now, invite_id],
        )
        .map_err(Error::Db)?;
        tx.commit().map_err(Error::Db)?;
        Ok(Redemption::Activated(GuestNode {
            number,
            peer_id: peer_id.to_owned(),
            user_id,
            display_name,
            activated_at: now,
            last_seen_at: None,
            revoked_at: None,
        }))
    }

    /// The identity resolver's lookup. A revoked node is returned as such.
    pub fn guest_by_peer(&self, peer_id: &str) -> Result<Option<GuestNode>, Error> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().expect("unpoisoned db lock");
        conn.query_row(
            &format!("SELECT {GUEST_COLUMNS} FROM guests g WHERE g.peer_id = ?1"),
            [peer_id],
            guest_from_row,
        )
        .optional()
        .map_err(Error::Db)
    }

    /// Every guest node, revoked ones included, by number.
    pub fn guests(&self) -> Result<Vec<GuestNode>, Error> {
        let conn = self.conn.lock().expect("unpoisoned db lock");
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {GUEST_COLUMNS} FROM guests g ORDER BY g.id"
            ))
            .map_err(Error::Db)?;
        let rows = stmt.query_map([], guest_from_row).map_err(Error::Db)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Error::Db)
    }

    pub fn touch_guest(&self, peer_id: &str, now: i64) -> Result<(), Error> {
        let conn = self.conn.lock().expect("unpoisoned db lock");
        conn.execute(
            "UPDATE guests SET last_seen_at = ?1 WHERE peer_id = ?2",
            rusqlite::params![now, peer_id],
        )
        .map_err(Error::Db)?;
        Ok(())
    }

    /// Marks a guest node revoked. Revoking twice keeps the first time.
    pub fn revoke_guest(&self, number: i64, now: i64) -> Result<GuestNode, Error> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().expect("unpoisoned db lock");
        conn.execute(
            "UPDATE guests SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL",
            rusqlite::params![now, number],
        )
        .map_err(Error::Db)?;
        conn.query_row(
            &format!("SELECT {GUEST_COLUMNS} FROM guests g WHERE g.id = ?1"),
            [number],
            guest_from_row,
        )
        .optional()
        .map_err(Error::Db)?
        .ok_or(Error::NoSuchGuest(number))
    }
}

pub fn hash_secret(secret: &wire::InviteSecret) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(secret.0).into()
}

const GUEST_COLUMNS: &str =
    "g.id, g.peer_id, g.user_id, g.display_name, g.activated_at, g.last_seen_at, g.revoked_at";

fn guest_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<GuestNode> {
    Ok(GuestNode {
        number: r.get(0)?,
        peer_id: r.get(1)?,
        user_id: r.get(2)?,
        display_name: r.get(3)?,
        activated_at: r.get(4)?,
        last_seen_at: r.get(5)?,
        revoked_at: r.get(6)?,
    })
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
        // v2 — invites and the guest nodes they were redeemed into: the
        // identity resolver's backing store on transports where waserver
        // is the authority (`docs/access/wire-contract.md`, Activation).
        M::up(
            "CREATE TABLE invites (
                 id INTEGER PRIMARY KEY,
                 secret_hash BLOB NOT NULL UNIQUE,
                 user_id TEXT NOT NULL,
                 display_name TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 expires_at INTEGER NOT NULL,
                 consumed_at INTEGER
             ) STRICT;
             CREATE TABLE guests (
                 id INTEGER PRIMARY KEY,
                 peer_id TEXT NOT NULL UNIQUE,
                 user_id TEXT NOT NULL,
                 display_name TEXT NOT NULL,
                 invite_id INTEGER NOT NULL REFERENCES invites(id),
                 activated_at INTEGER NOT NULL,
                 last_seen_at INTEGER,
                 revoked_at INTEGER
             ) STRICT;",
        ),
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

    fn invite<'a>(secret: &'a wire::InviteSecret, user: &'a str) -> NewInvite<'a> {
        NewInvite {
            secret,
            user_id: user,
            display_name: "phone",
            expires_at: 1_000 + 24 * 3600,
        }
    }

    #[test]
    fn invites_are_redeemed_exactly_once_and_idempotently() {
        use wire::ActivationError::*;
        let tmp = tempfile::tempdir().unwrap();
        let db = StateDb::open(tmp.path().join("state.db")).unwrap();
        let alice = wire::InviteSecret([1; 16]);
        db.create_invite(invite(&alice, "alice"), 1_000).unwrap();

        // Unknown secret, nothing bound.
        assert_eq!(
            db.redeem_invite(&wire::InviteSecret([9; 16]), "peer-a", 1_001)
                .unwrap(),
            Redemption::Refused(InviteUnknown)
        );
        assert!(db.guests().unwrap().is_empty());

        // First contact binds the peer.
        let Redemption::Activated(m) = db.redeem_invite(&alice, "peer-a", 1_001).unwrap() else {
            panic!("expected activation");
        };
        assert_eq!(
            (m.number, m.user_id.as_str(), m.activated_at),
            (1, "alice", 1_001)
        );
        assert_eq!(db.guest_by_peer("peer-a").unwrap().as_ref(), Some(&m));

        // A lost response: the same peer retries and gets the same node.
        assert_eq!(
            db.redeem_invite(&alice, "peer-a", 1_002).unwrap(),
            Redemption::Activated(m.clone())
        );
        // Another key with the same secret is refused.
        assert_eq!(
            db.redeem_invite(&alice, "peer-b", 1_002).unwrap(),
            Redemption::Refused(InviteConsumed)
        );
        // A guest presenting a fresh invite stays as it is.
        let bob = wire::InviteSecret([2; 16]);
        db.create_invite(invite(&bob, "bob"), 1_000).unwrap();
        assert_eq!(
            db.redeem_invite(&bob, "peer-a", 1_003).unwrap(),
            Redemption::Refused(AlreadyMember)
        );
        assert_eq!(db.guests().unwrap().len(), 1);
        assert_eq!(
            db.guest_by_peer("peer-a").unwrap().unwrap().user_id,
            "alice"
        );
    }

    #[test]
    fn expired_invites_and_revoked_guests_are_refused() {
        use wire::ActivationError::*;
        let tmp = tempfile::tempdir().unwrap();
        let db = StateDb::open(tmp.path().join("state.db")).unwrap();
        let secret = wire::InviteSecret([1; 16]);
        db.create_invite(invite(&secret, "alice"), 1_000).unwrap();
        assert_eq!(
            db.redeem_invite(&secret, "peer-a", 1_000 + 24 * 3600)
                .unwrap(),
            Redemption::Refused(InviteExpired)
        );
        let Redemption::Activated(m) = db.redeem_invite(&secret, "peer-a", 1_001).unwrap() else {
            panic!("expected activation");
        };

        db.touch_guest("peer-a", 1_500).unwrap();
        assert_eq!(
            db.guest_by_peer("peer-a").unwrap().unwrap().last_seen_at,
            Some(1_500)
        );

        assert!(matches!(
            db.revoke_guest(99, 2_000),
            Err(Error::NoSuchGuest(99))
        ));
        let revoked = db.revoke_guest(m.number, 2_000).unwrap();
        assert_eq!(revoked.revoked_at, Some(2_000));
        // Revoking again keeps the first time; the row stays listed.
        assert_eq!(
            db.revoke_guest(m.number, 3_000).unwrap().revoked_at,
            Some(2_000)
        );
        assert_eq!(db.guests().unwrap().len(), 1);

        // A revoked key is never re-bound, even with a fresh invite.
        let fresh = wire::InviteSecret([2; 16]);
        db.create_invite(invite(&fresh, "alice-again"), 2_000)
            .unwrap();
        assert_eq!(
            db.redeem_invite(&fresh, "peer-a", 2_001).unwrap(),
            Redemption::Refused(Revoked)
        );
        // A fresh key redeems it, as a new guest number.
        let Redemption::Activated(again) = db.redeem_invite(&fresh, "peer-a2", 2_001).unwrap()
        else {
            panic!("expected activation");
        };
        assert_eq!(again.number, 2);
        assert_eq!(db.guests().unwrap().len(), 2);
    }
}
