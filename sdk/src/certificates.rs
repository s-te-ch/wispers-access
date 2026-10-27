//! Certificates for a share's `https` origins: what each certificate the share
//! names can vouch for, and, for the desktop app, whether this device trusts
//! the origins.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::X509Certificate;

/// What one of a share's certificates is, for displaying to the user.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CertificateDescription {
    /// The subject's common name, or the whole subject if it has none.
    pub subject: String,
    pub kind: CertificateKind,
    /// A server certificate's subject alternative names, the names it was
    /// issued for. Empty for a CA.
    pub names: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum CertificateKind {
    /// A CA, which can vouch for any website. Name constraints aren't
    /// described, as an organisation's CA rarely has them, and Android
    /// WebView ignores them. A self-signed server certificate marked CA is a
    /// CA too.
    Ca,
    /// A server's own certificate, signed by itself.
    SelfSignedServer,
    /// A server's certificate from a CA. The device has to trust that CA, so
    /// in a share's list it was most likely pasted instead of its CA's.
    CaSignedServer,
}

/// `None` if `pem` doesn't hold an X.509 certificate.
pub(crate) fn describe(pem: &str) -> Option<CertificateDescription> {
    let (_, pem) = x509_parser::pem::parse_x509_pem(pem.as_bytes()).ok()?;
    let certificate = pem.parse_x509().ok()?;
    let subject = certificate
        .subject()
        .iter_common_name()
        .next()
        .and_then(|cn| cn.as_str().ok())
        .map(str::to_owned)
        .unwrap_or_else(|| certificate.subject().to_string());
    let (kind, names) = if certificate.is_ca() {
        (CertificateKind::Ca, Vec::new())
    } else if certificate.subject().as_raw() == certificate.issuer().as_raw() {
        (CertificateKind::SelfSignedServer, own_names(&certificate))
    } else {
        (CertificateKind::CaSignedServer, own_names(&certificate))
    };
    Some(CertificateDescription {
        subject,
        kind,
        names,
    })
}

fn own_names(certificate: &X509Certificate) -> Vec<String> {
    let Ok(Some(names)) = certificate.subject_alternative_name() else {
        return Vec::new();
    };
    names
        .value
        .general_names
        .iter()
        .filter_map(|name| match name {
            GeneralName::DNSName(name) => Some((*name).to_owned()),
            GeneralName::IPAddress(bytes) => Some(ip_address(bytes)?.to_string()),
            _ => None,
        })
        .collect()
}

fn ip_address(bytes: &[u8]) -> Option<IpAddr> {
    Some(match bytes.len() {
        4 => IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(bytes).ok()?)),
        16 => IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(bytes).ok()?)),
        _ => return None,
    })
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) use trust::check_origins;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use trust::{OriginTrust, TrustVerdict};

/// The desktop app's check. The mobile apps learn the same from their
/// webviews, and `rustls-platform-verifier` would need a JNI setup on Android
/// that the SDK, bound through JNA, doesn't have.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod trust {
    use crate::forward_proxy::{TunnelRefusal, open_tunnel};
    use crate::guest_node::GuestNode;
    use crate::{Result, SdkError};
    use rustls::pki_types::ServerName;
    use rustls_platform_verifier::ConfigVerifierExt;
    use std::sync::Arc;
    use std::time::Duration;
    use wispers_access_wire::Origin;

    /// Whether this device trusts an `https` origin, as the platform's
    /// verifier decides.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum TrustVerdict {
        Trusted,
        /// The verifier refused the origin's certificate.
        Untrusted {
            reason: String,
        },
        /// No verdict, as there was no handshake. The host node couldn't
        /// reach the origin, the tunnel failed, what answered doesn't speak
        /// TLS, or it took too long.
        Unreachable {
            reason: String,
        },
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct OriginTrust {
        pub origin: String,
        pub verdict: TrustVerdict,
    }

    /// How long opening one origin's tunnel may take, incl. handshake.
    const TUNNEL_TIMEOUT: Duration = Duration::from_secs(10);
    const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

    /// Checks one origin at a time, each by a TLS handshake through a tunnel to
    /// it. Gives up if a tunnel fails because we can't reach the host node.
    pub(crate) async fn check_origins(
        node: &GuestNode,
        origins: Vec<Origin>,
    ) -> Result<Vec<OriginTrust>> {
        let config = Arc::new(
            rustls::ClientConfig::with_platform_verifier()
                .map_err(|e| SdkError::Internal(format!("no certificate verifier: {e}")))?,
        );
        let mut checked = Vec::with_capacity(origins.len());
        let mut host_down: Option<String> = None;
        for origin in origins {
            let verdict = match &host_down {
                Some(reason) => TrustVerdict::Unreachable {
                    reason: reason.clone(),
                },
                None => match check_origin(node, &origin, config.clone()).await {
                    Ok(verdict) => verdict,
                    Err(HostNodeDown { reason }) => {
                        host_down = Some(reason.clone());
                        TrustVerdict::Unreachable { reason }
                    }
                },
            };
            checked.push(OriginTrust {
                origin: origin.to_string(),
                verdict,
            });
        }
        Ok(checked)
    }

    /// The origin's verdict, or `HostNodeDown` if the tunnel failed because of
    /// the host node rather than the origin.
    async fn check_origin(
        node: &GuestNode,
        origin: &Origin,
        config: Arc<rustls::ClientConfig>,
    ) -> std::result::Result<TrustVerdict, HostNodeDown> {
        let opened = tokio::time::timeout(
            TUNNEL_TIMEOUT,
            open_tunnel(node, origin.host(), origin.port()),
        )
        .await;
        let stream = match opened {
            Ok(Ok(stream)) => stream,
            Ok(Err(refusal @ (TunnelRefusal::NotAllowed | TunnelRefusal::Unreachable))) => {
                return Ok(TrustVerdict::Unreachable {
                    reason: refusal.to_string(),
                });
            }
            Ok(Err(refusal)) => {
                return Err(HostNodeDown {
                    reason: refusal.to_string(),
                });
            }
            Err(_) => {
                return Err(HostNodeDown {
                    reason: "the share's host didn't answer in time".to_owned(),
                });
            }
        };
        // IPv6 literals come in brackets.
        let host = origin.host().trim_start_matches('[').trim_end_matches(']');
        let Ok(name) = ServerName::try_from(host.to_owned()) else {
            return Ok(TrustVerdict::Unreachable {
                reason: format!("{host} is not a server name"),
            });
        };
        let handshake = tokio::time::timeout(
            HANDSHAKE_TIMEOUT,
            tokio_rustls::TlsConnector::from(config).connect(name, stream),
        );
        Ok(match handshake.await {
            Ok(Ok(_)) => TrustVerdict::Trusted,
            Ok(Err(e)) => match e.get_ref().and_then(|e| e.downcast_ref::<rustls::Error>()) {
                Some(e @ rustls::Error::InvalidCertificate(_)) => TrustVerdict::Untrusted {
                    reason: e.to_string(),
                },
                _ => TrustVerdict::Unreachable {
                    reason: e.to_string(),
                },
            },
            Err(_) => TrustVerdict::Unreachable {
                reason: "the handshake timed out".to_owned(),
            },
        })
    }

    /// Why the host node couldn't open a tunnel.
    struct HostNodeDown {
        reason: String,
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::test_support::Setup;
        use rustls::pki_types::pem::PemObject;
        use rustls::pki_types::{CertificateDer, PrivateKeyDer};
        use tokio::net::TcpListener;

        #[tokio::test]
        async fn origins_are_checked_by_a_handshake_through_the_share() {
            let tls_port = self_signed_server().await;
            let setup =
                Setup::with_origins_redirected_to(vec!["https://app.internal".into()], tls_port)
                    .await;
            let checked = setup
                .client
                .check_origin_trust(setup.share.clone())
                .await
                .unwrap();
            assert_eq!(checked.len(), 1);
            assert_eq!(checked[0].origin, "https://app.internal");
            let TrustVerdict::Untrusted { reason } = &checked[0].verdict else {
                panic!("{checked:?}");
            };
            assert!(reason.contains("certificate"), "{reason}");
            assert_eq!(setup.targets(), [("app.internal".to_owned(), 443)]);
        }

        #[tokio::test]
        async fn origins_the_host_cant_reach_or_that_dont_speak_tls_are_unreachable() {
            // The host node connects every tunnel to the plain-HTTP echo app.
            let setup = Setup::with_origins_redirected(vec![
                "https://app.internal".into(),
                "http://localhost:4200".into(),
            ])
            .await;
            let checked = setup
                .client
                .check_origin_trust(setup.share.clone())
                .await
                .unwrap();
            assert_eq!(checked.len(), 1, "{checked:?}");
            assert!(
                matches!(checked[0].verdict, TrustVerdict::Unreachable { .. }),
                "{checked:?}"
            );

            let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = closed.local_addr().unwrap().port();
            drop(closed);
            let setup = Setup::with_origins(vec![format!("https://127.0.0.1:{port}")]).await;
            let checked = setup
                .client
                .check_origin_trust(setup.share.clone())
                .await
                .unwrap();
            assert_eq!(
                checked[0].verdict,
                TrustVerdict::Unreachable {
                    reason: "the share's host could not reach it".to_owned()
                }
            );
        }

        /// A TLS server with the self-signed `app.internal` certificate,
        /// which no platform trusts.
        async fn self_signed_server() -> u16 {
            let certificate = CertificateDer::from_pem_slice(include_bytes!(
                "../testdata/certificates/self-signed.pem"
            ))
            .unwrap();
            let key = PrivateKeyDer::from_pem_slice(include_bytes!(
                "../testdata/certificates/self-signed.key"
            ))
            .unwrap();
            let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
            let config = rustls::ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![certificate], key)
                .unwrap();
            let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move {
                loop {
                    let (tcp, _) = listener.accept().await.unwrap();
                    let acceptor = acceptor.clone();
                    tokio::spawn(async move { acceptor.accept(tcp).await.ok() });
                }
            });
            port
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ca_is_described_by_its_subject() {
        assert_eq!(
            describe(include_str!("../testdata/certificates/ca.pem")),
            Some(CertificateDescription {
                subject: "Example Root CA".into(),
                kind: CertificateKind::Ca,
                names: vec![],
            })
        );
    }

    #[test]
    fn a_servers_certificate_vouches_for_its_own_names() {
        assert_eq!(
            describe(include_str!("../testdata/certificates/self-signed.pem")),
            Some(CertificateDescription {
                subject: "app.internal".into(),
                kind: CertificateKind::SelfSignedServer,
                names: vec!["app.internal".into(), "10.1.2.3".into()],
            })
        );
        assert_eq!(
            describe(include_str!("../testdata/certificates/server.pem")),
            Some(CertificateDescription {
                subject: "app.internal.example".into(),
                kind: CertificateKind::CaSignedServer,
                names: vec!["app.internal.example".into()],
            })
        );
        assert_eq!(describe("not a certificate"), None);
    }
}
