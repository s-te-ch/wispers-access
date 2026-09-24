//! Where a share's key material lives: a store the host app provides, or
//! the file store under the data directory by default.

use crate::ShareId;
use std::fs;
use std::io;
use std::path::PathBuf;

/// Key material per share, under transport-defined keys (`root_key`,
/// `iroh_secret`, …). Apps implement this on the platform's secure store;
/// the SDK never writes a secret anywhere else.
pub trait SecretStore: Send + Sync {
    fn load(&self, share: &ShareId, key: &str) -> Result<Option<Vec<u8>>, SecretStoreError>;
    fn save(&self, share: &ShareId, key: &str, value: &[u8]) -> Result<(), SecretStoreError>;
    /// Deleting what is not there is not an error.
    fn delete(&self, share: &ShareId, key: &str) -> Result<(), SecretStoreError>;
}

#[derive(Debug)]
pub struct SecretStoreError(pub String);

impl std::fmt::Display for SecretStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SecretStoreError {}

impl From<io::Error> for SecretStoreError {
    fn from(e: io::Error) -> Self {
        SecretStoreError(e.to_string())
    }
}

/// The default store: one file per secret, `<dir>/<share id>/<key>`,
/// readable by the owner only. For desktop, and for tests.
pub struct FileSecretStore {
    dir: PathBuf,
}

impl FileSecretStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path(&self, share: &ShareId, key: &str) -> PathBuf {
        debug_assert!(!key.contains(['/', '\\']), "secret keys are plain names");
        self.dir.join(share.as_str()).join(key)
    }
}

impl SecretStore for FileSecretStore {
    fn load(&self, share: &ShareId, key: &str) -> Result<Option<Vec<u8>>, SecretStoreError> {
        match fs::read(self.path(share, key)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn save(&self, share: &ShareId, key: &str, value: &[u8]) -> Result<(), SecretStoreError> {
        let path = self.path(share, key);
        let dir = path
            .parent()
            .expect("a secret's path has a share directory");
        fs::create_dir_all(dir)?;
        // Write beside the target and rename, so a crash leaves the old
        // secret or the new one, never a torn file.
        let staging = path.with_extension("staging");
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        {
            use io::Write;
            let mut file = options.open(&staging)?;
            file.write_all(value)?;
            file.sync_all()?;
        }
        fs::rename(&staging, &path)?;
        Ok(())
    }

    fn delete(&self, share: &ShareId, key: &str) -> Result<(), SecretStoreError> {
        let path = self.path(share, key);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        // The share directory goes with its last secret. Not being empty
        // yet is fine.
        let _ = fs::remove_dir(self.dir.join(share.as_str()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_round_trip_and_leave_nothing_behind() {
        let dir =
            std::env::temp_dir().join(format!("wispers-access-secrets-{}", uuid::Uuid::new_v4()));
        let store = FileSecretStore::new(dir.clone());
        let share = ShareId::mint();
        assert_eq!(store.load(&share, "root_key").unwrap(), None);
        store.save(&share, "root_key", b"one").unwrap();
        store.save(&share, "root_key", b"two").unwrap();
        assert_eq!(
            store.load(&share, "root_key").unwrap().as_deref(),
            Some(&b"two"[..])
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join(share.as_str()).join("root_key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        store.delete(&share, "root_key").unwrap();
        store.delete(&share, "root_key").unwrap();
        assert!(!dir.join(share.as_str()).exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
