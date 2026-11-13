//! TLS for outgoing MLLP connections (#273, Milestone 4 decision D4).
//!
//! When Harux sends, it is the side that connects, like a browser: the
//! target shows a certificate and Harux checks it against the operating
//! system's certificate store and `send.tls.ca_file`. Harux needs a
//! certificate of its own only for a target that asks the sender for one
//! (`send.tls.client_cert_file` / `client_key_file`, one for the server).
//!
//! The library is rustls with the `ring` provider, so the binary needs no
//! OpenSSL at run time and builds on the three release targets without CMake
//! or NASM. TLS for the *listener* (encrypted receiving) is Milestone 6.
//!
//! Every failure is turned into a reason a person can act on, naming the
//! setting that answers it (see `mllp_client::TlsFailure`): an untrusted
//! certificate names its issuer, which needs this module to read the issuer
//! out of the certificate ([`cert_names`]).

use crate::config::SendTlsConfig;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio_rustls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use tokio_rustls::rustls::client::{ResolvesClientCert, WebPkiServerVerifier};
use tokio_rustls::rustls::crypto::{self, CryptoProvider};
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use tokio_rustls::rustls::sign::CertifiedKey;
use tokio_rustls::rustls::{
    self, CertificateError, ClientConfig, DigitallySignedStruct, OtherError, RootCertStore,
    SignatureScheme,
};
use tokio_rustls::TlsConnector;
use tracing::warn;

/// Everything outgoing TLS needs, built once at start from `[send.tls]`.
pub struct TlsContext {
    provider: Arc<CryptoProvider>,
    /// Checks the target's certificate. `None` when nothing is trusted at
    /// all (an empty OS store and no `ca_file`).
    verifier: Option<Arc<NamingVerifier>>,
    /// Accepts any certificate (still checks the handshake signatures).
    /// Only when `send.tls.allow_unverified` is set.
    unverified: Option<Arc<AcceptAnyCertificate>>,
    /// Harux's own certificate and key, if configured.
    client_key: Option<Arc<CertifiedKey>>,
    /// What the context trusts, for the startup log.
    summary: String,
}

/// No key material in debug output.
impl std::fmt::Debug for TlsContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsContext")
            .field("summary", &self.summary)
            .finish_non_exhaustive()
    }
}

impl TlsContext {
    /// Read `[send.tls]`: the OS store (if `native`), `ca_file`, and the
    /// client certificate with its key. Any file problem is an error naming
    /// the key, so the start stops instead of failing every send later.
    pub fn load(config: &SendTlsConfig, native: bool) -> Result<Self, String> {
        let provider = Arc::new(crypto::ring::default_provider());
        let mut roots = RootCertStore::empty();
        let mut parts = Vec::new();

        if native {
            let found = rustls_native_certs::load_native_certs();
            for e in &found.errors {
                warn!("Reading the operating system's certificate store: {e}");
            }
            let (added, ignored) = roots.add_parsable_certificates(found.certs);
            if ignored > 0 {
                warn!("{ignored} certificate(s) of the operating system's store could not be used");
            }
            parts.push(format!(
                "{added} certificates from the operating system's store"
            ));
        }
        if let Some(file) = non_empty(&config.ca_file) {
            let certs = read_certs(file, "send.tls.ca_file")?;
            let (added, _) = roots.add_parsable_certificates(certs);
            if added == 0 {
                return Err(format!(
                    "send.tls.ca_file {file:?} holds no usable CA certificate"
                ));
            }
            parts.push(format!("{added} from {file}"));
        }

        let verifier = if roots.is_empty() {
            warn!(
                "No trusted certificates for outgoing TLS (empty operating system store, no \
                 send.tls.ca_file): sends with a verified certificate will fail"
            );
            None
        } else {
            let inner =
                WebPkiServerVerifier::builder_with_provider(Arc::new(roots), Arc::clone(&provider))
                    .build()
                    .map_err(|e| format!("send.tls: cannot build the certificate check: {e}"))?;
            Some(Arc::new(NamingVerifier { inner }))
        };

        let client_key = match (
            non_empty(&config.client_cert_file),
            non_empty(&config.client_key_file),
        ) {
            (Some(cert), Some(key)) => {
                let chain = read_certs(cert, "send.tls.client_cert_file")?;
                let key_der = PrivateKeyDer::from_pem_file(key).map_err(|e| {
                    // The path is named, never the content.
                    format!("send.tls.client_key_file {key:?} holds no usable private key: {e}")
                })?;
                let certified = CertifiedKey::from_der(chain, key_der, &provider).map_err(|e| {
                    format!(
                        "send.tls.client_key_file {key:?} does not belong to \
                         send.tls.client_cert_file {cert:?}: {e}"
                    )
                })?;
                parts.push(format!("client certificate from {cert}"));
                Some(Arc::new(certified))
            }
            (None, None) => None,
            _ => {
                return Err(
                    "send.tls.client_cert_file and send.tls.client_key_file must be set together"
                        .into(),
                )
            }
        };

        let unverified = config.allow_unverified.then(|| {
            parts.push("unverified connections allowed".into());
            Arc::new(AcceptAnyCertificate {
                provider: Arc::clone(&provider),
            })
        });
        Ok(Self {
            provider,
            verifier,
            unverified,
            client_key,
            summary: if parts.is_empty() {
                "no trusted certificates".into()
            } else {
                parts.join(", ")
            },
        })
    }

    /// What the context trusts, for the startup log.
    pub fn summary(&self) -> &str {
        &self.summary
    }

    pub fn allow_unverified(&self) -> bool {
        self.unverified.is_some()
    }

    pub fn has_client_certificate(&self) -> bool {
        self.client_key.is_some()
    }

    /// A connector for one connection, and the record of whether the target
    /// asked for a client certificate during it (for the failure reason).
    /// `verify: false` needs `allow_unverified` (checked by the caller).
    pub fn connector(&self, verify: bool) -> Result<(TlsConnector, Arc<ClientAuth>), String> {
        let auth = Arc::new(ClientAuth {
            key: self.client_key.clone(),
            asked: AtomicBool::new(false),
        });
        let builder = ClientConfig::builder_with_provider(Arc::clone(&self.provider))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .dangerous();
        let builder = if verify {
            let verifier = self.verifier.clone().ok_or_else(|| {
                "Nothing is trusted: the operating system's certificate store is empty and \
                 send.tls.ca_file is not set"
                    .to_string()
            })?;
            builder.with_custom_certificate_verifier(verifier)
        } else {
            let any = self.unverified.clone().ok_or_else(|| {
                "An unverified connection needs send.tls.allow_unverified".to_string()
            })?;
            builder.with_custom_certificate_verifier(any)
        };
        let config = builder.with_client_cert_resolver(Arc::clone(&auth) as Arc<_>);
        Ok((TlsConnector::from(Arc::new(config)), auth))
    }
}

fn non_empty(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|f| !f.is_empty())
}

fn read_certs(file: &str, key: &str) -> Result<Vec<CertificateDer<'static>>, String> {
    let certs = CertificateDer::pem_file_iter(file)
        .map_err(|e| format!("{key} {file:?} cannot be read: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("{key} {file:?} is not a PEM certificate file: {e}"))?;
    if certs.is_empty() {
        return Err(format!("{key} {file:?} holds no certificate"));
    }
    Ok(certs)
}

/// Harux's client certificate for one connection, recording whether the
/// target asked for one: rustls calls `resolve` exactly when the server
/// sends a certificate request.
#[derive(Debug)]
pub struct ClientAuth {
    key: Option<Arc<CertifiedKey>>,
    asked: AtomicBool,
}

impl ClientAuth {
    /// The target asked Harux for a certificate.
    pub fn asked(&self) -> bool {
        self.asked.load(Ordering::Relaxed)
    }

    /// Harux had one to show.
    pub fn had_certificate(&self) -> bool {
        self.key.is_some()
    }
}

impl ResolvesClientCert for ClientAuth {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        _sigschemes: &[SignatureScheme],
    ) -> Option<Arc<CertifiedKey>> {
        self.asked.store(true, Ordering::Relaxed);
        self.key.clone()
    }

    fn has_certs(&self) -> bool {
        self.key.is_some()
    }
}

/// An untrusted certificate, with its issuer for the failure reason.
#[derive(Debug)]
pub struct UntrustedCertificate {
    /// `CN=…, O=…` of the issuer.
    pub issuer: String,
    /// The certificate signed itself.
    pub self_signed: bool,
}

impl std::fmt::Display for UntrustedCertificate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.self_signed {
            write!(f, "self-signed certificate ({})", self.issuer)
        } else {
            write!(f, "certificate issued by {}", self.issuer)
        }
    }
}

impl std::error::Error for UntrustedCertificate {}

/// The webpki check, with the issuer added to an "unknown issuer" error.
#[derive(Debug)]
struct NamingVerifier {
    inner: Arc<WebPkiServerVerifier>,
}

impl ServerCertVerifier for NamingVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
            .map_err(|e| match e {
                rustls::Error::InvalidCertificate(CertificateError::UnknownIssuer) => {
                    let (issuer, subject) = cert_names(end_entity).unwrap_or_default();
                    rustls::Error::InvalidCertificate(CertificateError::Other(OtherError(
                        Arc::new(UntrustedCertificate {
                            self_signed: !issuer.is_empty() && issuer == subject,
                            issuer: if issuer.is_empty() {
                                "an unknown issuer".into()
                            } else {
                                issuer
                            },
                        }),
                    )))
                }
                e => e,
            })
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// `send.tls.allow_unverified`: any certificate is accepted — encrypted, not
/// verified. The handshake signatures are still checked, so the connection
/// is with whoever holds the certificate's key.
#[derive(Debug)]
struct AcceptAnyCertificate {
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for AcceptAnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

// --- Reading names out of a certificate ---
//
// Only for the failure reason "issued by CN=…": a small DER walk to the
// issuer and subject of an X.509 certificate. Anything unexpected gives
// `None`; nothing here decides whether a certificate is trusted.

/// One DER element: `(tag, content, rest)`.
fn der(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = input.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = if first < 0x80 {
        (first as usize, rest)
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 4 || rest.len() < n {
            return None;
        }
        let len = rest[..n]
            .iter()
            .fold(0usize, |acc, &b| (acc << 8) | b as usize);
        (len, &rest[n..])
    };
    if rest.len() < len {
        return None;
    }
    Some((tag, &rest[..len], &rest[len..]))
}

/// `(issuer, subject)` of a certificate as `CN=…, O=…`.
pub fn cert_names(cert: &[u8]) -> Option<(String, String)> {
    const SEQUENCE: u8 = 0x30;
    let (SEQUENCE, certificate, _) = der(cert)? else {
        return None;
    };
    let (SEQUENCE, mut tbs, _) = der(certificate)? else {
        return None;
    };
    // [0] version (optional), serial, signature algorithm, then the names.
    let (tag, _, rest) = der(tbs)?;
    if tag == 0xA0 {
        tbs = rest;
        let (_, _, rest) = der(tbs)?; // serial
        tbs = rest;
    } else {
        tbs = rest; // that was the serial
    }
    let (_, _, rest) = der(tbs)?; // signature algorithm
    let (SEQUENCE, issuer, rest) = der(rest)? else {
        return None;
    };
    let (_, _, rest) = der(rest)?; // validity
    let (SEQUENCE, subject, _) = der(rest)? else {
        return None;
    };
    Some((name(issuer), name(subject)))
}

/// A distinguished name's CN, O and OU, in that order of preference.
fn name(mut rdns: &[u8]) -> String {
    let mut parts: Vec<(usize, String)> = Vec::new();
    while let Some((_, set, rest)) = der(rdns) {
        rdns = rest;
        let mut atvs = set;
        while let Some((_, atv, rest)) = der(atvs) {
            atvs = rest;
            let Some((0x06, oid, value)) = der(atv) else {
                continue;
            };
            let Some((_, text, _)) = der(value) else {
                continue;
            };
            let label = match oid {
                [0x55, 0x04, 0x03] => (0, "CN"),
                [0x55, 0x04, 0x0A] => (1, "O"),
                [0x55, 0x04, 0x0B] => (2, "OU"),
                _ => continue,
            };
            parts.push((
                label.0,
                format!("{}={}", label.1, String::from_utf8_lossy(text)),
            ));
        }
    }
    parts.sort_by_key(|p| p.0);
    parts
        .into_iter()
        .map(|p| p.1)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pem(file: &str) -> CertificateDer<'static> {
        CertificateDer::from_pem_file(format!("tests/tls/{file}")).unwrap()
    }

    #[test]
    fn names_are_read_from_certificates() {
        assert_eq!(
            cert_names(&pem("untrusted.pem")).unwrap(),
            (
                "CN=Unknown Test CA, O=Elsewhere".to_string(),
                "CN=localhost".to_string()
            )
        );
        let (issuer, subject) = cert_names(&pem("ca.pem")).unwrap();
        assert_eq!(issuer, subject, "a CA certificate signs itself");
        assert_eq!(issuer, "CN=Harux Test CA, O=Harux Tests");
        assert!(cert_names(b"\x30\x03\x02\x01").is_none());
        assert!(cert_names(&[]).is_none());
    }

    #[test]
    fn files_are_checked_at_load() {
        let cfg = |ca: Option<&str>, cert: Option<&str>, key: Option<&str>| SendTlsConfig {
            ca_file: ca.map(|f| format!("tests/tls/{f}")),
            client_cert_file: cert.map(|f| format!("tests/tls/{f}")),
            client_key_file: key.map(|f| format!("tests/tls/{f}")),
            allow_unverified: false,
        };
        let ok = TlsContext::load(
            &cfg(Some("ca.pem"), Some("client.pem"), Some("client.key")),
            false,
        )
        .unwrap();
        assert!(ok.has_client_certificate() && !ok.allow_unverified());
        assert!(
            ok.summary().contains("1 from tests/tls/ca.pem"),
            "{}",
            ok.summary()
        );

        for (c, why) in [
            (cfg(Some("missing.pem"), None, None), "cannot be read"),
            (cfg(Some("server.key"), None, None), "holds no certificate"),
            (
                cfg(None, Some("client.pem"), Some("other.key")),
                "does not belong to",
            ),
            (
                cfg(None, Some("client.pem"), Some("client.pem")),
                "no usable private key",
            ),
            (cfg(None, Some("client.pem"), None), "set together"),
        ] {
            let err = TlsContext::load(&c, false).unwrap_err();
            assert!(err.contains(why), "{why}: {err}");
            assert!(
                !err.contains("PRIVATE KEY"),
                "never the key's content: {err}"
            );
        }

        // Nothing trusted: a verified connection can't be made, and says why.
        let empty = TlsContext::load(&SendTlsConfig::default(), false).unwrap();
        let err = |verify| empty.connector(verify).err().unwrap();
        assert!(err(true).contains("Nothing is trusted"));
        assert!(err(false).contains("allow_unverified"));
    }
}
