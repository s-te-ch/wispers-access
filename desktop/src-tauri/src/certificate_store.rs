//! The user's certificate store, where a share's certificates go so that the
//! browsers running its profiles trust its `https` origins: the login keychain
//! with SSL trust on macOS, the user's root store on Windows. Linux has no
//! store every browser reads, so there the app only saves the certificates.
//! Each change asks the user: macOS for their password, Windows for a yes.

pub use platform::{CAN_INSTALL, install, remove};

#[cfg(target_os = "macos")]
mod platform {
    use anyhow::{Context, bail};
    use core_foundation::array::CFArray;
    use core_foundation::base::{OSStatus, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use security_framework::certificate::SecCertificate;
    use security_framework::item::{ItemClass, ItemSearchOptions, Limit, Reference, SearchResult};
    use security_framework::policy::SecPolicy;
    use security_framework::secure_transport::SslProtocolSide;
    use security_framework_sys::base::SecCertificateRef;
    use security_framework_sys::trust_settings::{
        SecTrustSettingsDomain, SecTrustSettingsSetTrustSettings, kSecTrustSettingsDomainUser,
        kSecTrustSettingsResultTrustRoot,
    };

    // Not in security-framework-sys. The Security framework is linked by it.
    unsafe extern "C" {
        fn SecTrustSettingsRemoveTrustSettings(
            certificate: SecCertificateRef,
            domain: SecTrustSettingsDomain,
        ) -> OSStatus;
    }

    const ERR_SEC_DUPLICATE_ITEM: OSStatus = -25299;
    const ERR_SEC_ITEM_NOT_FOUND: OSStatus = -25300;
    const ERR_AUTHORIZATION_CANCELED: OSStatus = -60006;

    pub const CAN_INSTALL: bool = true;

    /// Adds the certificate to the login keychain and trusts it as a root, for
    /// SSL only (`security add-trusted-cert -r trustRoot -p ssl`): limiting
    /// it to the share's host names would make Chrome and Firefox not trust
    /// it at all.
    pub fn install(der: &[u8]) -> anyhow::Result<()> {
        let certificate = SecCertificate::from_der(der).context("not a certificate")?;
        match certificate.add_to_keychain(None) {
            Ok(()) => {}
            Err(e) if e.code() == ERR_SEC_DUPLICATE_ITEM => {}
            Err(e) => return Err(e).context("could not add the certificate to the login keychain"),
        }
        // The keys are `CFSTR` macros in SecTrustSettings.h, not symbols.
        let policy_key = CFString::from_static_string("kSecTrustSettingsPolicy");
        let result_key = CFString::from_static_string("kSecTrustSettingsResult");
        let setting = CFDictionary::from_CFType_pairs(&[
            (
                policy_key.as_CFType(),
                SecPolicy::create_ssl(SslProtocolSide::SERVER, None).as_CFType(),
            ),
            (
                result_key.as_CFType(),
                CFNumber::from(kSecTrustSettingsResultTrustRoot as i32).as_CFType(),
            ),
        ]);
        let settings = CFArray::from_CFTypes(&[setting]);
        let status = unsafe {
            SecTrustSettingsSetTrustSettings(
                certificate.as_concrete_TypeRef(),
                kSecTrustSettingsDomainUser,
                settings.as_CFTypeRef(),
            )
        };
        match status {
            0 => Ok(()),
            ERR_AUTHORIZATION_CANCELED => bail!("cancelled"),
            status => bail!("could not trust the certificate (error {status})"),
        }
    }

    /// Takes the trust away and the certificate out of the keychain. A
    /// certificate that is gone already is fine.
    pub fn remove(der: &[u8]) -> anyhow::Result<()> {
        let certificate = SecCertificate::from_der(der).context("not a certificate")?;
        let status = unsafe {
            SecTrustSettingsRemoveTrustSettings(
                certificate.as_concrete_TypeRef(),
                kSecTrustSettingsDomainUser,
            )
        };
        match status {
            0 | ERR_SEC_ITEM_NOT_FOUND => {}
            ERR_AUTHORIZATION_CANCELED => bail!("cancelled"),
            status => bail!("could not remove the certificate's trust (error {status})"),
        }
        for item in keychain_items(&certificate, der)? {
            item.delete()
                .context("could not remove the certificate from the keychain")?;
        }
        Ok(())
    }

    /// The keychain's own items for the certificate. Deleting needs these: a
    /// certificate made from its DER isn't one.
    fn keychain_items(
        certificate: &SecCertificate,
        der: &[u8],
    ) -> anyhow::Result<Vec<SecCertificate>> {
        let found = ItemSearchOptions::new()
            .class(ItemClass::certificate())
            .label(&certificate.subject_summary())
            .load_refs(true)
            .limit(Limit::All)
            .search();
        let results = match found {
            Ok(results) => results,
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => return Ok(Vec::new()),
            Err(e) => return Err(e).context("could not look the certificate up in the keychain"),
        };
        Ok(results
            .into_iter()
            .filter_map(|result| match result {
                SearchResult::Ref(Reference::Certificate(item)) if item.to_der() == der => {
                    Some(item)
                }
                _ => None,
            })
            .collect())
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use anyhow::bail;
    use std::ptr;
    use windows_sys::Win32::Foundation::{ERROR_CANCELLED, GetLastError};
    use windows_sys::Win32::Security::Cryptography::{
        CERT_FIND_EXISTING, CERT_STORE_ADD_REPLACE_EXISTING, CERT_STORE_PROV_SYSTEM_W,
        CERT_SYSTEM_STORE_CURRENT_USER, CertAddEncodedCertificateToStore, CertCloseStore,
        CertCreateCertificateContext, CertDeleteCertificateFromStore, CertFindCertificateInStore,
        CertFreeCertificateContext, CertOpenStore, HCERTSTORE, PKCS_7_ASN_ENCODING,
        X509_ASN_ENCODING,
    };

    pub const CAN_INSTALL: bool = true;

    const ENCODING: u32 = X509_ASN_ENCODING | PKCS_7_ASN_ENCODING;

    /// Adds the certificate to the user's root store (`CurrentUser\Root`),
    /// which needs no admin rights. Windows asks the user to confirm.
    pub fn install(der: &[u8]) -> anyhow::Result<()> {
        let store = RootStore::open()?;
        let added = unsafe {
            CertAddEncodedCertificateToStore(
                store.0,
                ENCODING,
                der.as_ptr(),
                der.len() as u32,
                CERT_STORE_ADD_REPLACE_EXISTING,
                ptr::null_mut(),
            )
        };
        if added == 0 {
            failure("could not add the certificate to the root store")?;
        }
        Ok(())
    }

    /// Takes the certificate out of the user's root store. Windows asks the
    /// user to confirm. A certificate that is gone already is fine.
    pub fn remove(der: &[u8]) -> anyhow::Result<()> {
        let store = RootStore::open()?;
        unsafe {
            let wanted = CertCreateCertificateContext(ENCODING, der.as_ptr(), der.len() as u32);
            if wanted.is_null() {
                bail!("not a certificate");
            }
            let found = CertFindCertificateInStore(
                store.0,
                ENCODING,
                0,
                CERT_FIND_EXISTING,
                wanted as *const _,
                ptr::null(),
            );
            CertFreeCertificateContext(wanted);
            // Deleting frees the found context, whatever the outcome.
            if !found.is_null() && CertDeleteCertificateFromStore(found) == 0 {
                failure("could not remove the certificate from the root store")?;
            }
        }
        Ok(())
    }

    /// The last error, as an error saying what failed, or as cancelled.
    fn failure(what: &str) -> anyhow::Result<()> {
        match unsafe { GetLastError() } {
            ERROR_CANCELLED => bail!("cancelled"),
            code => bail!("{what} (error {code})"),
        }
    }

    /// The user's root store, closed when dropped.
    struct RootStore(HCERTSTORE);

    impl RootStore {
        fn open() -> anyhow::Result<Self> {
            let name: Vec<u16> = "ROOT\0".encode_utf16().collect();
            let store = unsafe {
                CertOpenStore(
                    CERT_STORE_PROV_SYSTEM_W,
                    0,
                    0,
                    CERT_SYSTEM_STORE_CURRENT_USER,
                    name.as_ptr() as *const _,
                )
            };
            if store.is_null() {
                bail!("could not open the user's root store");
            }
            Ok(Self(store))
        }
    }

    impl Drop for RootStore {
        fn drop(&mut self) {
            unsafe { CertCloseStore(self.0, 0) };
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    pub const CAN_INSTALL: bool = false;

    pub fn install(_der: &[u8]) -> anyhow::Result<()> {
        anyhow::bail!("this app can't install certificates on Linux")
    }

    pub fn remove(_der: &[u8]) -> anyhow::Result<()> {
        Ok(())
    }
}
