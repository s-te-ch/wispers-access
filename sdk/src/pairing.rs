//! Browser pairing.
//!
//! Our proxy listens on a localhost port, which is open to every process on the
//! device. To prevent unauthorised processes from using the proxy, pairing sets
//! a cookie in the user's browser, which we can then look for in subsequent
//! requests.
//!
//! Every URL the app hands to the (system) browser points to the `/pair`
//! endpoint, carrying a one-time token and the destination app., and the proxy
//! answers with the cookie and a redirect to the app. A browser that has the
//! cookie already is redirected straight away.
//!
//! The cookie's value is a secret the SDK mints once and keeps in the secret
//! store next to the shares' key material, so paired browsers survive a
//! restart. The tokens live in memory and die with the proxy. The `/pair`
//! request itself is answered in [`crate::http`]; this module is the secret,
//! the cookie and the tokens.

use crate::http::{PROXY_DOMAIN, RequiredCookie, constant_time_eq};
use crate::secrets::{SecretScope, SecretStore, SecretStoreError};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Issues the proxy's cookie to the user's browsers. The cookie they must
/// end up with, and the one-time tokens handed out and not yet spent.
pub struct CookieIssuer {
    cookie: RequiredCookie,
    /// How long a token stays usable once minted.
    token_lifetime: Duration,
    /// Each unused token with its expiry, or `None` for one that never
    /// expires (a lifetime past the end of time).
    tokens: Mutex<HashMap<String, Option<Instant>>>,
}

/// The cookie the proxy sets.
const COOKIE_NAME: &str = "__wispers_access_pairing";

/// Where the secret lives in the secret store, under [`SecretScope::Client`].
const SECRET_KEY: &str = "browser_pairing_secret";

impl CookieIssuer {
    /// Restore the issuer by reading the secret from storage, or mint a new one.
    pub fn restore_or_mint(
        secrets: &dyn SecretStore,
        token_lifetime: Duration,
    ) -> Result<Self, SecretStoreError> {
        let secret = match secrets.load(SecretScope::Client, SECRET_KEY.to_owned())? {
            Some(bytes) => String::from_utf8(bytes)
                .map_err(|_| SecretStoreError::Failed("the pairing secret is not text".into()))?,
            None => {
                let secret = random_hex(32);
                secrets.save(
                    SecretScope::Client,
                    SECRET_KEY.to_owned(),
                    secret.clone().into_bytes(),
                )?;
                secret
            }
        };
        Ok(Self {
            cookie: RequiredCookie {
                name: COOKIE_NAME.to_owned(),
                value: secret,
            },
            token_lifetime,
            tokens: Mutex::new(HashMap::new()),
        })
    }

    /// The cookie a paired browser sends and the proxy demands.
    pub fn required_cookie(&self) -> &RequiredCookie {
        &self.cookie
    }

    /// A fresh one-time token, good for `token_lifetime`.
    pub fn mint_token(&self) -> String {
        let token = random_hex(32);
        let now = Instant::now();
        let mut tokens = self.tokens.lock().expect("unpoisoned");
        // Tokens nobody used go when the next one is minted.
        tokens.retain(|_, expiry| expiry.is_none_or(|e| e > now));
        tokens.insert(token.clone(), now.checked_add(self.token_lifetime));
        token
    }

    /// Whether `token` is one we minted, unused and unexpired. Either way it
    /// is spent now.
    pub fn consume_token(&self, token: &str) -> bool {
        let mut tokens = self.tokens.lock().expect("unpoisoned");
        let Some(minted) = tokens.keys().find(|t| constant_time_eq(t, token)).cloned() else {
            return false;
        };
        let expiry = tokens.remove(&minted).flatten();
        expiry.is_none_or(|e| e > Instant::now())
    }

    /// The `Set-Cookie` value that pairs a browser: the cookie for every
    /// `*.wa.localhost` host, out of reach of the apps' scripts, and not sent
    /// along from other sites.
    pub fn set_cookie_header(&self) -> String {
        format!(
            "{}={}; Domain={PROXY_DOMAIN}; Path=/; Max-Age={COOKIE_MAX_AGE}; HttpOnly; SameSite=Lax",
            self.cookie.name, self.cookie.value
        )
    }
}

/// A year: a pairing is a device-local capability, not a session.
const COOKIE_MAX_AGE: u64 = 365 * 24 * 60 * 60;

/// `n` random bytes from the OS, as lowercase hex.
fn random_hex(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    getrandom::fill(&mut bytes).expect("the OS random source is available");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::FileSecretStore;

    fn scratch_store() -> (std::path::PathBuf, FileSecretStore) {
        let dir =
            std::env::temp_dir().join(format!("wispers-access-pairing-{}", uuid::Uuid::new_v4()));
        (dir.clone(), FileSecretStore::new(dir))
    }

    #[test]
    fn the_secret_survives_the_proxy() {
        let (dir, store) = scratch_store();
        let first = CookieIssuer::restore_or_mint(&store, Duration::from_secs(60)).unwrap();
        let second = CookieIssuer::restore_or_mint(&store, Duration::from_secs(60)).unwrap();
        assert_eq!(first.required_cookie(), second.required_cookie());
        assert_eq!(first.required_cookie().value.len(), 64);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_token_is_spent_on_first_use() {
        let (dir, store) = scratch_store();
        let issuer = CookieIssuer::restore_or_mint(&store, Duration::from_secs(60)).unwrap();
        let token = issuer.mint_token();
        assert_eq!(token.len(), 64);
        assert!(!issuer.consume_token("nope"));
        assert!(issuer.consume_token(&token));
        assert!(!issuer.consume_token(&token));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_token_expires() {
        let (dir, store) = scratch_store();
        let issuer = CookieIssuer::restore_or_mint(&store, Duration::ZERO).unwrap();
        let token = issuer.mint_token();
        assert!(!issuer.consume_token(&token));
        let forever = CookieIssuer::restore_or_mint(&store, Duration::MAX).unwrap();
        let token = forever.mint_token();
        assert!(forever.consume_token(&token));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
