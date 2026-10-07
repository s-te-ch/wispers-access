//! Secret storage, using the platform's credential store: the macOS Keychain,
//! the Windows Credential Manager, and the Secret Service (GNOME Keyring,
//! KWallet) on Linux. Where there is none, as on a Linux desktop without a
//! Secret Service, the SDK keeps secrets in files under its data directory.
//! Every secret is one credential under the app's identifier as the service,
//! named `<share id>/<key>` or `client/<key>` as the iOS app names its Keychain
//! items.

use anyhow::{Context, bail};
use keyring_core::{CredentialStore, Entry, Error};
use std::path::Path;
use std::sync::Arc;
use wispers_access_sdk::{SecretScope, SecretStore, SecretStoreError};

pub struct PlatformSecretStore {
    service: String,
}

/// The file in the data directory that records where the first run put the
/// secrets: `keyring` or `files`.
const CHOICE_FILE: &str = "secret-store";

impl PlatformSecretStore {
    /// The store for this platform, or `None` for files.
    ///
    /// The first run picks, by whether the credential store opens, and later
    /// runs stick with that pick. Otherwise a keyring that comes or goes
    /// between runs would hide every secret saved before. A keyring picked
    /// but gone fails the start rather than quietly losing the shares.
    pub fn open(service: &str, data_dir: &Path) -> anyhow::Result<Option<Self>> {
        let choice_file = data_dir.join(CHOICE_FILE);
        let store = match std::fs::read_to_string(&choice_file) {
            Ok(choice) if choice.trim() == "files" => None,
            Ok(choice) if choice.trim() == "keyring" => Some(
                credential_store().context("could not open the keyring holding this app's keys")?,
            ),
            Ok(choice) => bail!("{} says {choice:?}", choice_file.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let store = match credential_store() {
                    Ok(store) => Some(store),
                    Err(e) => {
                        tracing::warn!(error = %e, "no credential store, keeping secrets in files");
                        None
                    }
                };
                std::fs::create_dir_all(data_dir)?;
                let choice = if store.is_some() { "keyring" } else { "files" };
                std::fs::write(&choice_file, choice)
                    .with_context(|| format!("could not write {}", choice_file.display()))?;
                store
            }
            Err(e) => {
                return Err(e).with_context(|| format!("could not read {}", choice_file.display()));
            }
        };
        Ok(store.map(|store| {
            keyring_core::set_default_store(store);
            Self {
                service: service.to_owned(),
            }
        }))
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
fn credential_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

/// The Windows Credential Manager, as generic credentials of the user.
#[cfg(target_os = "windows")]
fn credential_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(windows_native_keyring_store::Store::new()?)
}

/// The Secret Service on the session bus. Opening it opens a session with
/// the service, so a desktop without one fails here rather than at the first
/// secret.
#[cfg(target_os = "linux")]
fn credential_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(zbus_secret_service_keyring_store::Store::new()?)
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn credential_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Err(Error::NotSupportedByStore("no credential store on this platform".into()))
}
