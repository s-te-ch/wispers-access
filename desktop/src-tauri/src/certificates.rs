//! Tauri commands for a share's certificates: whether this device trusts the
//! share's `https` origins, and installing the share's certificates in the
//! user's certificate store (see `certificate_store`). The app records what
//! it installed, to take it out again when the last share listing it is left.

use crate::Desktop;
use crate::certificate_store;
use crate::shares::find;
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::State;
use wispers_access_sdk as sdk;

/// What the share's pane says about its certificates.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificateStatus {
    /// The share's `https` origins whose certificate this device doesn't
    /// trust. Origins that couldn't be reached aren't among them.
    untrusted_origins: Vec<String>,
    /// Whether the check couldn't tell for any origin whether this device
    /// trusts it, as the share's host node or every origin was out of reach.
    unchecked: bool,
    certificates: Vec<CertificateView>,
    /// Whether this app can install certificates here, or only save them.
    can_install: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CertificateView {
    subject: String,
    /// `ca`, `selfSignedServer` or `caSignedServer`.
    kind: &'static str,
    /// A server certificate's names.
    names: Vec<String>,
    /// Whether installing it can help (see `is_installable`).
    installable: bool,
    /// Whether this app installed it. One installed some other way doesn't
    /// count, and installing it again does no harm.
    installed: bool,
}

/// Checks each of the share's `https` origins by a TLS handshake through the
/// share, as the browsers can't tell this app about a certificate error.
#[tauri::command]
pub async fn check_certificates(
    desktop: State<'_, Desktop>,
    share_id: String,
) -> Result<CertificateStatus, String> {
    let share = find(&desktop, &share_id)?;
    let checked = desktop
        .client
        .check_origin_trust(share.id.clone())
        .await
        .map_err(|e| e.to_string())?;
    let unchecked = !checked.is_empty()
        && checked
            .iter()
            .all(|checked| matches!(checked.verdict, sdk::TrustVerdict::Unreachable { .. }));
    let untrusted_origins: Vec<String> = checked
        .into_iter()
        .filter(|checked| matches!(checked.verdict, sdk::TrustVerdict::Untrusted { .. }))
        .map(|checked| checked.origin)
        .collect();
    tracing::info!(share = %share.label, untrusted = ?untrusted_origins, unchecked, "checked the share's origins");
    Ok(CertificateStatus {
        untrusted_origins,
        unchecked,
        certificates: share
            .ca_certificates
            .iter()
            .filter_map(|pem| {
                let description = sdk::describe_certificate(pem.clone())?;
                Some(CertificateView::of(
                    description,
                    desktop.installed_certificates.contains(pem),
                ))
            })
            .collect(),
        can_install: certificate_store::CAN_INSTALL,
    })
}

impl CertificateView {
    fn of(description: sdk::CertificateDescription, installed: bool) -> Self {
        Self {
            installable: is_installable(&description.kind),
            kind: match description.kind {
                sdk::CertificateKind::Ca => "ca",
                sdk::CertificateKind::SelfSignedServer => "selfSignedServer",
                sdk::CertificateKind::CaSignedServer => "caSignedServer",
            },
            subject: description.subject,
            names: description.names,
            installed,
        }
    }
}

/// A CA's certificate, or a self-signed server's, which every desktop browser
/// takes as a root of its own.
fn is_installable(kind: &sdk::CertificateKind) -> bool {
    !matches!(kind, sdk::CertificateKind::CaSignedServer)
}

/// Installs the share's installable certificates in the user's store. The
/// system asks the user for each one; a refusal stops there.
#[tauri::command]
pub async fn install_certificates(
    desktop: State<'_, Desktop>,
    share_id: String,
) -> Result<(), String> {
    let share = find(&desktop, &share_id)?;
    let installed = desktop.installed_certificates.clone();
    // The system's prompts block until the user answers.
    tauri::async_runtime::spawn_blocking(move || {
        for pem in installable(&share) {
            certificate_store::install(&der(&pem)?).map_err(|e| format!("{e:#}"))?;
            installed.add(pem);
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Saves the share's installable certificates into a folder the user picks,
/// for where the app can't install them. The folder, or `None` if the user
/// cancelled.
#[tauri::command]
pub async fn save_certificates(
    desktop: State<'_, Desktop>,
    share_id: String,
) -> Result<Option<String>, String> {
    let share = find(&desktop, &share_id)?;
    let Some(folder) = rfd::AsyncFileDialog::new().pick_folder().await else {
        return Ok(None);
    };
    let folder = folder.path();
    for pem in installable(&share) {
        let name = sdk::describe_certificate(pem.clone())
            .map(|description| description.subject)
            .unwrap_or_else(|| share.label.clone());
        let file = folder.join(format!("{}.crt", file_name(&name)));
        std::fs::write(&file, &pem)
            .map_err(|e| format!("could not save {}: {e}", file.display()))?;
    }
    Ok(Some(folder.display().to_string()))
}

/// Takes out what this app installed for a share just left, unless another
/// share still lists it. Runs the system's prompts, so off the async runtime.
pub fn remove_for_left_share(
    client: &sdk::Client,
    installed: &InstalledCertificates,
    left: &sdk::Share,
) {
    let shares = match client.shares() {
        Ok(shares) => shares,
        Err(e) => {
            tracing::warn!(error = %e, "could not read the shares, keeping their certificates");
            return;
        }
    };
    for pem in removed_on_leave(installed, left, &shares) {
        match der(pem).and_then(|der| certificate_store::remove(&der).map_err(|e| format!("{e:#}")))
        {
            Ok(()) => installed.remove(pem),
            Err(e) => {
                tracing::warn!(share = %left.label, error = %e, "could not remove a certificate")
            }
        }
    }
}

/// The certificates leaving `share` takes out, which are those this app
/// installed for it that none of the other `shares` lists. The remove dialog
/// announces them, as removing has the system ask the user.
pub fn removed_on_leave<'a>(
    installed: &InstalledCertificates,
    share: &'a sdk::Share,
    shares: &[sdk::Share],
) -> Vec<&'a String> {
    share
        .ca_certificates
        .iter()
        .filter(|pem| installed.contains(pem))
        .filter(|pem| {
            !shares
                .iter()
                .any(|other| other.id != share.id && other.ca_certificates.contains(pem))
        })
        .collect()
}

/// The share's certificates that installing can help with.
fn installable(share: &sdk::Share) -> Vec<String> {
    share
        .ca_certificates
        .iter()
        .filter(|pem| {
            sdk::describe_certificate((*pem).clone())
                .is_some_and(|description| is_installable(&description.kind))
        })
        .cloned()
        .collect()
}

fn der(pem: &str) -> Result<Vec<u8>, String> {
    CertificateDer::from_pem_slice(pem.as_bytes())
        .map(|der| der.to_vec())
        .map_err(|e| format!("not a certificate: {e}"))
}

/// A certificate's subject as a file name, on every platform.
fn file_name(subject: &str) -> String {
    subject
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " ._()-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

/// The certificates this app installed, by their PEM as the shares list them.
pub struct InstalledCertificates {
    path: PathBuf,
    pems: Mutex<BTreeSet<String>>,
}

impl InstalledCertificates {
    /// Reads the file at `path`, or starts empty: an unreadable file costs
    /// removing the certificates on leave, not the app.
    pub fn open(path: PathBuf) -> Arc<Self> {
        let pems = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Arc::new(Self {
            path,
            pems: Mutex::new(pems),
        })
    }

    /// True if this app installed the certificate and it's still in the store.
    pub fn contains(&self, pem: &str) -> bool {
        if !self.lock().contains(pem) {
            return false;
        }
        match der(pem)
            .map_err(anyhow::Error::msg)
            .and_then(|der| certificate_store::is_installed(&der))
        {
            Ok(true) => true,
            Ok(false) => {
                self.remove(pem);
                false
            }
            // Unknown, so as recorded.
            Err(e) => {
                tracing::warn!(
                    error = format!("{e:#}"),
                    "could not look a certificate up in the store"
                );
                true
            }
        }
    }

    fn add(&self, pem: String) {
        let mut pems = self.lock();
        pems.insert(pem);
        self.persist(&pems);
    }

    fn remove(&self, pem: &str) {
        let mut pems = self.lock();
        pems.remove(pem);
        self.persist(&pems);
    }

    fn persist(&self, pems: &BTreeSet<String>) {
        let written = serde_json::to_vec(pems)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&self.path, bytes));
        if let Err(e) = written {
            tracing::warn!(error = %e, path = %self.path.display(), "could not save the installed certificates");
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeSet<String>> {
        self.pems.lock().expect("unpoisoned")
    }
}
