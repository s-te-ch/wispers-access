//! Per-share on-disk storage.
//!
//! `~/.config/waserver/shares/<share>/` holds two files with two owners:
//! `share.toml` (the user's, see `config`) and `state.db` (the daemon's).

use crate::config::{self, ShareConfig};
use chrono::{DateTime, Utc};
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
    #[error("invalid share name '{0}' (use letters, digits, '-' or '_')")]
    InvalidName(String),
    #[error("share {0} is not initialised")]
    NotInitialised(String),
    #[error("share {0} already exists")]
    AlreadyExists(String),
    #[error("no guest node number {0}")]
    NoSuchGuest(i64),
    #[error("stored iroh key has the wrong length")]
    CorruptKey,
    #[error("could not determine the OS config directory")]
    NoConfigDir,
}

/// Names of the initialised shares, unsorted.
pub fn list_shares() -> Result<Vec<String>, Error> {
    let dir = shares_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    Ok(fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join(config::FILENAME).is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect())
}

//-- Share directory ----------------------------------------------------------

#[derive(Clone)]
pub struct ShareDir {
    name: String,
    dir: PathBuf,
}

impl ShareDir {
    /// Validates the name and computes the path, but touches nothing on disk.
    pub fn new(name: &str) -> Result<Self, Error> {
        if !config::is_valid_id(name) {
            return Err(Error::InvalidName(name.to_owned()));
        }
        Ok(Self {
            name: name.to_owned(),
            dir: shares_dir()?.join(name),
        })
    }

    pub fn config_path(&self) -> PathBuf {
        self.dir.join(config::FILENAME)
    }

    pub fn exists(&self) -> bool {
        self.config_path().is_file()
    }

    pub fn load_config(&self) -> Result<ShareConfig, Error> {
        if !self.exists() {
            return Err(Error::NotInitialised(self.name.clone()));
        }
        Ok(ShareConfig::load(&self.config_path())?)
    }

    /// Creates the directory, writes `share.toml` and creates an empty `state.db`.
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

/// Connection to the SQLite file for a share. Cheap to clone.
#[derive(Clone)]
pub struct StateDb {
    conn: Arc<Mutex<rusqlite::Connection>>,
}

/// State specific to the Wispers Connect transport.
#[derive(Clone)]
pub struct WispersConnectState {
    pub api_key: String,
    pub connectivity_group_id: String,
}

const KEY_API_KEY: &str = "api_key";
const KEY_CONNECTIVITY_GROUP_ID: &str = "connectivity_group_id";
const KEY_ROOT_KEY: &str = "root_key";
const KEY_REGISTRATION: &str = "registration";
const KEY_IROH_SECRET: &str = "iroh_secret";

impl StateDb {
    /// For testing. `ShareDir` opens the real file.
    pub(crate) fn open(path: PathBuf) -> Result<Self, Error> {
        let conn = rusqlite::Connection::open(&path).map_err(Error::Db)?;
        // If two processes open the file at the same time (e.g. the daemon and
        // the cli), wait briefly instead of failing.
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(Error::Db)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(Error::Db)?;
        Self::prepare(conn)
    }

    /// A database that lives and dies with the test.
    #[cfg(test)]
    pub(crate) fn open_in_memory() -> Result<Self, Error> {
        Self::prepare(rusqlite::Connection::open_in_memory().map_err(Error::Db)?)
    }

    fn prepare(mut conn: rusqlite::Connection) -> Result<Self, Error> {
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

    pub fn iroh_secret(&self) -> Result<Option<[u8; 32]>, Error> {
        match self.get(KEY_IROH_SECRET)? {
            Some(bytes) => Ok(Some(bytes.try_into().map_err(|_| Error::CorruptKey)?)),
            None => Ok(None),
        }
    }

    pub fn set_iroh_secret(&self, secret: &[u8; 32]) -> Result<(), Error> {
        self.set(KEY_IROH_SECRET, secret)
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
pub struct NewInvite {
    /// The invite secret sent to the invitee. We only store its SHA-256.
    pub secret: wire::InviteSecret,
    /// The user ID that will later be sent to proxied web apps in the identity
    /// header.
    pub user_id: String,
    /// Display name of the guest node, used only on the host side.
    pub node_name: String,
    /// When the invite stops being redeemable.
    pub expires_at: DateTime<Utc>,
}

/// An invite as recorded, for `status`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InviteRow {
    pub id: i64,
    pub user_id: String,
    pub node_name: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
}

/// How long a consumed or expired invite stays in `recent_invites`.
const INVITE_HISTORY: chrono::Duration = chrono::Duration::days(7);

/// The outcome of redeeming an invite.
#[derive(Debug, PartialEq, Eq)]
pub enum Redemption {
    Activated(GuestNode),
    Refused(wire::ActivationError),
}

/// A guest node - a transport identity bound to this share by redeeming an
/// invite. A revoked node keeps its row so its key is never bound again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestNode {
    /// How the CLI addresses it (`waserver revoke <share> <number>`).
    pub number: i64,
    /// The transport's stable identifier for the node.
    pub peer_id: String,
    /// The user identity associated with the node.
    pub user_id: String,
    /// The node's name, shown in waserver status output (e.g. "Alice's phone").
    pub display_name: String,
    /// Time the invite was redeemed.
    pub activated_at: DateTime<Utc>,
    /// Time the node last connected.
    pub last_seen_at: Option<DateTime<Utc>>,
    /// Time the host revoked this node.
    pub revoked_at: Option<DateTime<Utc>>,
}

impl StateDb {
    /// Records an invite, returns its ID.
    pub fn create_invite(&self, invite: NewInvite, now: DateTime<Utc>) -> Result<i64, Error> {
        let conn = self.conn.lock().expect("unpoisoned db lock");
        conn.execute(
            "INSERT INTO invites (secret_hash, user_id, node_name, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                hash_secret(&invite.secret),
                invite.user_id,
                invite.node_name,
                to_secs(now),
                to_secs(invite.expires_at)
            ],
        )
        .map_err(Error::Db)?;
        Ok(conn.last_insert_rowid())
    }

    /// The invites worth showing, newest first: those still redeemable at
    /// `now`, plus those created within [`INVITE_HISTORY`]. Older consumed
    /// and expired invites stay in the table (guests reference the invite
    /// they redeemed) but are not listed, mirroring the hub's token listing.
    pub fn recent_invites(&self, now: DateTime<Utc>) -> Result<Vec<InviteRow>, Error> {
        let conn = self.conn.lock().expect("unpoisoned db lock");
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, node_name, created_at, expires_at, consumed_at
                 FROM invites
                 WHERE (consumed_at IS NULL AND expires_at > ?1) OR created_at > ?2
                 ORDER BY id DESC",
            )
            .map_err(Error::Db)?;
        let rows = stmt
            .query_map([to_secs(now), to_secs(now - INVITE_HISTORY)], |r| {
                Ok(InviteRow {
                    id: r.get(0)?,
                    user_id: r.get(1)?,
                    node_name: r.get(2)?,
                    created_at: from_secs(r.get(3)?),
                    expires_at: from_secs(r.get(4)?),
                    consumed_at: r.get::<_, Option<i64>>(5)?.map(from_secs),
                })
            })
            .map_err(Error::Db)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Error::Db)
    }

    /// Redeem the invite code, binding `peer_id` to the metadata (user ID, node
    /// name, etc) the operator added when generating the invite.
    pub fn redeem_invite(
        &self,
        secret: &wire::InviteSecret,
        peer_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Redemption, Error> {
        use rusqlite::OptionalExtension;
        use wire::ActivationError::*;
        let now_secs = to_secs(now);
        let secret_hash = hash_secret(secret);
        let mut conn = self.conn.lock().expect("unpoisoned db lock");
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(Error::Db)?;

        // Check whether the peer is allowed to redeem an invite (i.e isn't
        // already a guest node, or revoked). One special case: if the peer is
        // is trying to redeem the same invite again, the last response probably
        // got lost, so we return the same response again.
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
                Redemption::Refused(AlreadyGuest)
            });
        }

        // Check invite validity.
        let invite: Option<(i64, String, String, i64, Option<i64>)> = tx
            .query_row(
                "SELECT id, user_id, node_name, expires_at, consumed_at
                 FROM invites WHERE secret_hash = ?1",
                [&secret_hash[..]],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()
            .map_err(Error::Db)?;
        let Some((invite_id, user_id, node_name, expires_at, consumed_at)) = invite else {
            return Ok(Redemption::Refused(InviteUnknown));
        };
        if consumed_at.is_some() {
            return Ok(Redemption::Refused(InviteConsumed));
        }
        if expires_at <= now_secs {
            return Ok(Redemption::Refused(InviteExpired));
        }

        // Finally, redeem the invite.
        tx.execute(
            "INSERT INTO guests (peer_id, user_id, display_name, invite_id, activated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![peer_id, user_id, node_name, invite_id, now_secs],
        )
        .map_err(Error::Db)?;
        let number = tx.last_insert_rowid();
        tx.execute(
            "UPDATE invites SET consumed_at = ?1 WHERE id = ?2",
            rusqlite::params![now_secs, invite_id],
        )
        .map_err(Error::Db)?;
        tx.commit().map_err(Error::Db)?;

        // Return the new guest node.
        Ok(Redemption::Activated(GuestNode {
            number,
            peer_id: peer_id.to_owned(),
            user_id,
            display_name: node_name,
            activated_at: from_secs(now_secs),
            last_seen_at: None,
            revoked_at: None,
        }))
    }

    /// Look up a GuestNode by its peer_id.
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

    /// List of guest node, revoked ones included, by number.
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

    /// Set last_seen for the given peer ID to now.
    pub fn touch_guest(&self, peer_id: &str, now: DateTime<Utc>) -> Result<(), Error> {
        let conn = self.conn.lock().expect("unpoisoned db lock");
        conn.execute(
            "UPDATE guests SET last_seen_at = ?1 WHERE peer_id = ?2",
            rusqlite::params![to_secs(now), peer_id],
        )
        .map_err(Error::Db)?;
        Ok(())
    }

    /// Removes the DB entry for given guest, used when the guests asks to be
    /// removed. Unlike a revoke, this leaves no mark.
    pub fn remove_guest(&self, number: i64) -> Result<(), Error> {
        let conn = self.conn.lock().expect("unpoisoned db lock");
        let removed = conn
            .execute("DELETE FROM guests WHERE id = ?1", [number])
            .map_err(Error::Db)?;
        if removed == 0 {
            return Err(Error::NoSuchGuest(number));
        }
        Ok(())
    }

    /// Marks a guest node revoked. A second revocation is a no-op.
    pub fn revoke_guest(&self, number: i64, now: DateTime<Utc>) -> Result<GuestNode, Error> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().expect("unpoisoned db lock");
        conn.execute(
            "UPDATE guests SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL",
            rusqlite::params![to_secs(now), number],
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
        activated_at: from_secs(r.get(4)?),
        last_seen_at: r.get::<_, Option<i64>>(5)?.map(from_secs),
        revoked_at: r.get::<_, Option<i64>>(6)?.map(from_secs),
    })
}

/// The `_at` columns hold Unix seconds.
fn to_secs(t: DateTime<Utc>) -> i64 {
    t.timestamp()
}

fn from_secs(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(secs, 0).expect("a stored timestamp is in range")
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
                 node_name TEXT NOT NULL,
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

fn shares_dir() -> Result<PathBuf, Error> {
    Ok(base_dir()?.join("shares"))
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

    /// A test instant, `secs` after the epoch.
    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    fn invite(secret: &wire::InviteSecret, user: &str) -> NewInvite {
        NewInvite {
            secret: *secret,
            user_id: user.to_owned(),
            node_name: "phone".to_owned(),
            expires_at: at(1_000 + 24 * 3600),
        }
    }

    #[test]
    fn recent_invites_keeps_open_ones_and_a_week_of_history() {
        let db = StateDb::open_in_memory().unwrap();
        let day = 24 * 3600;
        // Consumed long ago: out. Expired long ago, never used: out.
        let old_used = wire::InviteSecret([1; 16]);
        db.create_invite(invite(&old_used, "old-used"), at(0))
            .unwrap();
        db.redeem_invite(&old_used, "peer-old", at(1)).unwrap();
        db.create_invite(invite(&wire::InviteSecret([2; 16]), "old-expired"), at(0))
            .unwrap();
        // Consumed recently: in, as history.
        let recent_used = wire::InviteSecret([3; 16]);
        db.create_invite(invite(&recent_used, "recent-used"), at(29 * day))
            .unwrap();
        db.redeem_invite(&recent_used, "peer-recent", at(29 * day + 1))
            .unwrap();
        // Created long ago but still open (a long expiry): in.
        db.create_invite(
            NewInvite {
                secret: wire::InviteSecret([4; 16]),
                user_id: "long-open".to_owned(),
                node_name: "phone".to_owned(),
                expires_at: at(100 * day),
            },
            at(0),
        )
        .unwrap();
        let now = at(30 * day);
        let listed: Vec<String> = db
            .recent_invites(now)
            .unwrap()
            .into_iter()
            .map(|i| i.user_id)
            .collect();
        assert_eq!(listed, ["long-open", "recent-used"]);
        // The table keeps everything.
        let total: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM invites", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 4);
    }

    #[test]
    fn invites_are_redeemed_exactly_once_and_idempotently() {
        use wire::ActivationError::*;
        let tmp = tempfile::tempdir().unwrap();
        let db = StateDb::open(tmp.path().join("state.db")).unwrap();
        let alice = wire::InviteSecret([1; 16]);
        db.create_invite(invite(&alice, "alice"), at(1_000))
            .unwrap();

        // Unknown secret, nothing bound.
        assert_eq!(
            db.redeem_invite(&wire::InviteSecret([9; 16]), "peer-a", at(1_001))
                .unwrap(),
            Redemption::Refused(InviteUnknown)
        );
        assert!(db.guests().unwrap().is_empty());

        // First contact binds the peer.
        let Redemption::Activated(m) = db.redeem_invite(&alice, "peer-a", at(1_001)).unwrap()
        else {
            panic!("expected activation");
        };
        assert_eq!(
            (m.number, m.user_id.as_str(), m.activated_at),
            (1, "alice", at(1_001))
        );
        assert_eq!(db.guest_by_peer("peer-a").unwrap().as_ref(), Some(&m));

        // A lost response: the same peer retries and gets the same node.
        assert_eq!(
            db.redeem_invite(&alice, "peer-a", at(1_002)).unwrap(),
            Redemption::Activated(m.clone())
        );
        // Another key with the same secret is refused.
        assert_eq!(
            db.redeem_invite(&alice, "peer-b", at(1_002)).unwrap(),
            Redemption::Refused(InviteConsumed)
        );
        // A guest presenting a fresh invite stays as it is.
        let bob = wire::InviteSecret([2; 16]);
        db.create_invite(invite(&bob, "bob"), at(1_000)).unwrap();
        assert_eq!(
            db.redeem_invite(&bob, "peer-a", at(1_003)).unwrap(),
            Redemption::Refused(AlreadyGuest)
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
        db.create_invite(invite(&secret, "alice"), at(1_000))
            .unwrap();
        assert_eq!(
            db.redeem_invite(&secret, "peer-a", at(1_000 + 24 * 3600))
                .unwrap(),
            Redemption::Refused(InviteExpired)
        );
        let Redemption::Activated(m) = db.redeem_invite(&secret, "peer-a", at(1_001)).unwrap()
        else {
            panic!("expected activation");
        };

        db.touch_guest("peer-a", at(1_500)).unwrap();
        assert_eq!(
            db.guest_by_peer("peer-a").unwrap().unwrap().last_seen_at,
            Some(at(1_500))
        );

        assert!(matches!(
            db.revoke_guest(99, at(2_000)),
            Err(Error::NoSuchGuest(99))
        ));
        let revoked = db.revoke_guest(m.number, at(2_000)).unwrap();
        assert_eq!(revoked.revoked_at, Some(at(2_000)));
        // Revoking again keeps the first time; the row stays listed.
        assert_eq!(
            db.revoke_guest(m.number, at(3_000)).unwrap().revoked_at,
            Some(at(2_000))
        );
        assert_eq!(db.guests().unwrap().len(), 1);

        // A guest that leaves is forgotten outright, invite left consumed.
        let leaver = wire::InviteSecret([3; 16]);
        db.create_invite(invite(&leaver, "carol"), at(2_000))
            .unwrap();
        let Redemption::Activated(carol) = db.redeem_invite(&leaver, "peer-c", at(2_001)).unwrap()
        else {
            panic!("expected activation");
        };
        db.remove_guest(carol.number).unwrap();
        assert!(db.guest_by_peer("peer-c").unwrap().is_none());
        assert!(matches!(
            db.remove_guest(carol.number),
            Err(Error::NoSuchGuest(_))
        ));
        assert_eq!(
            db.redeem_invite(&leaver, "peer-c2", at(2_002)).unwrap(),
            Redemption::Refused(InviteConsumed)
        );

        // A revoked key is never re-bound, even with a fresh invite.
        let fresh = wire::InviteSecret([2; 16]);
        db.create_invite(invite(&fresh, "alice-again"), at(2_000))
            .unwrap();
        assert_eq!(
            db.redeem_invite(&fresh, "peer-a", at(2_001)).unwrap(),
            Redemption::Refused(Revoked)
        );
        // A fresh key redeems it, as a new guest number.
        let Redemption::Activated(again) = db.redeem_invite(&fresh, "peer-a2", at(2_001)).unwrap()
        else {
            panic!("expected activation");
        };
        assert_eq!(again.number, 2);
        assert_eq!(db.guests().unwrap().len(), 2);
    }
}
