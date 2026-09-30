//! Storage for shares's key material and the client's own secrets.

use crate::ShareId;
use std::fs;
use std::io;
use std::path::PathBuf;

/// A secret's storage key.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SecretScope {
    /// Key material of one share.
    Share { id: ShareId },
    /// The client's own secrets, like the browser pairing secret.
    Client,
}

/// Generic secret storage. Apps implement this on the platform's secure store.
#[uniffi::export(with_foreign)]
pub trait SecretStore: Send + Sync {
    fn load(&self, scope: SecretScope, key: String) -> Result<Option<Vec<u8>>, SecretStoreError>;
    fn save(&self, scope: SecretScope, key: String, value: Vec<u8>)
    -> Result<(), SecretStoreError>;
    /// Deleting what is not there is not an error.
    fn delete(&self, scope: SecretScope, key: String) -> Result<(), SecretStoreError>;
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum SecretStoreError {
    #[error("{0}")]
    Failed(String),
}

impl From<io::Error> for SecretStoreError {
    fn from(e: io::Error) -> Self {
        SecretStoreError::Failed(e.to_string())
    }
}

impl From<uniffi::UnexpectedUniFFICallbackError> for SecretStoreError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        SecretStoreError::Failed(e.to_string())
    }
}

/// The default store with one file per secret - `<dir>/<share id>/<key>` for a
/// share's and `<dir>/client/<key>` for the client's own, readable by the owner
/// only. For desktop, and for tests.
pub struct FileSecretStore {
    dir: PathBuf,
}

impl FileSecretStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path(&self, scope: &SecretScope, key: &str) -> PathBuf {
        debug_assert!(!key.contains(['/', '\\']), "secret keys are plain names");
        self.scope_dir(scope).join(key)
    }

    fn scope_dir(&self, scope: &SecretScope) -> PathBuf {
        match scope {
            SecretScope::Share { id } => self.dir.join(id.as_str()),
            SecretScope::Client => self.dir.join("client"),
        }
    }
}

impl SecretStore for FileSecretStore {
    fn load(&self, scope: SecretScope, key: String) -> Result<Option<Vec<u8>>, SecretStoreError> {
        match fs::read(self.path(&scope, &key)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn save(
        &self,
        scope: SecretScope,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), SecretStoreError> {
        let path = self.path(&scope, &key);
        let dir = path
            .parent()
            .expect("a secret's path has a scope directory");
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
            file.write_all(&value)?;
            file.sync_all()?;
        }
        fs::rename(&staging, &path)?;
        Ok(())
    }

    fn delete(&self, scope: SecretScope, key: String) -> Result<(), SecretStoreError> {
        let path = self.path(&scope, &key);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        // The scope's directory goes with its last secret. Not being empty
        // yet is fine.
        let _ = fs::remove_dir(self.scope_dir(&scope));
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
        let share_id = ShareId::mint();
        let share = || SecretScope::Share {
            id: share_id.clone(),
        };
        let key = || "root_key".to_owned();
        assert_eq!(store.load(share(), key()).unwrap(), None);
        store.save(share(), key(), b"one".to_vec()).unwrap();
        store.save(share(), key(), b"two".to_vec()).unwrap();
        assert_eq!(
            store.load(share(), key()).unwrap().as_deref(),
            Some(&b"two"[..])
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join(share_id.as_str()).join("root_key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        store.delete(share(), key()).unwrap();
        store.delete(share(), key()).unwrap();
        assert!(!dir.join(share_id.as_str()).exists());

        // The client's own secrets live beside the shares', and are not a share's.
        let pairing = || "browser_pairing_secret".to_owned();
        store
            .save(SecretScope::Client, pairing(), b"s3cret".to_vec())
            .unwrap();
        assert_eq!(
            store
                .load(SecretScope::Client, pairing())
                .unwrap()
                .as_deref(),
            Some(&b"s3cret"[..])
        );
        assert_eq!(store.load(share(), pairing()).unwrap(), None);
        assert!(dir.join("client").join("browser_pairing_secret").exists());
        store.delete(SecretScope::Client, pairing()).unwrap();
        assert!(!dir.join("client").exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
