//! Secret storage, using the platform's credential store: the macOS Keychain
//! today, with the other platforms' stores to be added in [`credential_store`].
//! Every secret is one credential under the app's identifier as the service,
//! named `<share id>/<key>` or `client/<key>` as the iOS app names its Keychain
//! items.

use keyring_core::{CredentialStore, Entry, Error};
use std::sync::Arc;
use wispers_access_sdk::{SecretScope, SecretStore, SecretStoreError};

pub struct PlatformSecretStore {
    service: String,
}

impl PlatformSecretStore {
    /// The store for this platform, or `None` where there is none yet.
    pub fn open(service: &str) -> Option<Self> {
        keyring_core::set_default_store(credential_store()?);
        Some(Self {
            service: service.to_owned(),
        })
    }

    fn entry(&self, scope: &SecretScope, key: &str) -> Result<Entry, SecretStoreError> {
        let account = match scope {
            SecretScope::Share { id } => format!("{id}/{key}"),
            SecretScope::Client => format!("client/{key}"),
        };
        Entry::new(&self.service, &account).map_err(failed)
    }
}

impl SecretStore for PlatformSecretStore {
    fn load(&self, scope: SecretScope, key: String) -> Result<Option<Vec<u8>>, SecretStoreError> {
        match self.entry(&scope, &key)?.get_secret() {
            Ok(bytes) => Ok(Some(bytes)),
            Err(Error::NoEntry) => Ok(None),
            Err(e) => Err(failed(e)),
        }
    }

    fn save(
        &self,
        scope: SecretScope,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), SecretStoreError> {
        self.entry(&scope, &key)?.set_secret(&value).map_err(failed)
    }

    fn delete(&self, scope: SecretScope, key: String) -> Result<(), SecretStoreError> {
        match self.entry(&scope, &key)?.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(e) => Err(failed(e)),
        }
    }
}

fn failed(e: Error) -> SecretStoreError {
    SecretStoreError::Failed(e.to_string())
}

/// The macOS Keychain.
#[cfg(target_os = "macos")]
fn credential_store() -> Option<Arc<CredentialStore>> {
    match apple_native_keyring_store::keychain::Store::new() {
        Ok(store) => Some(store),
        Err(e) => {
            tracing::warn!(error = %e, "could not open the Keychain");
            None
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn credential_store() -> Option<Arc<CredentialStore>> {
    None
}
