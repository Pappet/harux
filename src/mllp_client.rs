//! The sending side of MLLP (#272): open a connection and do one exchange on
//! it — write `VT payload FS CR`, read until `FS CR`. Knows nothing about the
//! store; `send.rs` decides what is sent and records the outcome.
//!
//! Framing uses the constants and the frame finder of `mllp.rs`, so there is
//! one definition of the envelope for receiving, exporting and sending.

use crate::mllp::{extract_mllp_frame, MLLP_END_1, MLLP_END_2, MLLP_START};
use crate::tls::{ClientAuth, TlsContext, UntrustedCertificate};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{self, AlertDescription, CertificateError};

/// Largest response read to find its end (#272). A destination writes it,
/// so its size is not Harux's to choose; what is stored is cut further.
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// A payload below this size is framed into one buffer and written at once;
/// a larger one is written in three parts, borrowed from the stored message
/// — the same threshold the export stream coalesces at.
const SINGLE_WRITE_MAX: usize = 64 * 1024;

/// Time limits of one exchange.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub connect: Duration,
    pub write: Duration,
    pub response: Duration,
}

/// Why an exchange failed. [`ExchangeError::reason`] is the short form a
/// list row shows, [`ExchangeError::detail`] the sentence.
#[derive(Debug)]
pub enum ExchangeError {
    /// Nothing listens on the port (or a firewall rejects it).
    Refused,
    ConnectTimeout,
    /// Any other connect failure: name not found, network unreachable, …
    Unreachable(std::io::Error),
    WriteTimeout,
    Write(std::io::Error),
    /// No (complete) response within the response timeout.
    ResponseTimeout,
    /// The destination closed the connection before a complete response.
    Closed,
    /// The destination answered with something that is not an MLLP frame.
    NotFramed,
    /// The response is larger than [`MAX_RESPONSE_BYTES`].
    TooLarge,
    Read(std::io::Error),
    /// A plain connection answered with TLS: the target expects TLS.
    ExpectsTls,
    /// The TLS handshake (or a TLS-specific failure after it) — #273.
    Tls(TlsFailure),
}

/// Why a TLS connection failed, each naming the setting that answers it
/// (#273). How the targets' certificates are issued is not known in
/// advance (plan, Q2), so the first real attempt has to say which case
/// applies.
#[derive(Debug)]
pub enum TlsFailure {
    /// Issued by a CA nobody trusts here (or self-signed): `send.tls.ca_file`.
    Untrusted(String),
    /// Valid, but not for the host name or address connected to.
    WrongName(String),
    Expired,
    /// The target asks Harux for a certificate and none is configured.
    CertificateRequired,
    /// The target refused the certificate Harux showed.
    CertificateRefused,
    /// The target did not answer the handshake: a plain MLLP port?
    NoTlsAnswer,
    Other(String),
}

impl ExchangeError {
    /// Short reason, shown in the list row (`✕ refused`).
    pub fn reason(&self) -> &'static str {
        match self {
            ExchangeError::Refused => "refused",
            ExchangeError::ConnectTimeout | ExchangeError::ResponseTimeout => "timeout",
            ExchangeError::WriteTimeout => "write timeout",
            ExchangeError::Unreachable(_) => "unreachable",
            ExchangeError::Write(_) => "write failed",
            ExchangeError::Closed => "closed",
            ExchangeError::NotFramed => "not MLLP",
            ExchangeError::TooLarge => "too large",
            ExchangeError::Read(_) => "read failed",
            ExchangeError::ExpectsTls => "expects TLS",
            ExchangeError::Tls(f) => match f {
                TlsFailure::Untrusted(_) => "untrusted certificate",
                TlsFailure::WrongName(_) => "wrong name",
                TlsFailure::Expired => "expired certificate",
                TlsFailure::CertificateRequired => "certificate required",
                TlsFailure::CertificateRefused => "certificate refused",
                TlsFailure::NoTlsAnswer => "no TLS answer",
                TlsFailure::Other(_) => "TLS failed",
            },
        }
    }

    /// The failure in a sentence a person can act on.
    pub fn detail(&self) -> String {
        match self {
            ExchangeError::Refused => {
                "The destination refused the connection — nothing listens on that port".into()
            }
            ExchangeError::ConnectTimeout => {
                "No connection within send.connect_timeout_secs".into()
            }
            ExchangeError::Unreachable(e) => format!("Could not connect: {e}"),
            ExchangeError::WriteTimeout => {
                "The destination did not take the message within mllp.write_timeout_secs".into()
            }
            ExchangeError::Write(e) => format!("Writing the message failed: {e}"),
            ExchangeError::ResponseTimeout => {
                "No response within send.response_timeout_secs".into()
            }
            ExchangeError::Closed => {
                "The destination closed the connection before it answered".into()
            }
            ExchangeError::NotFramed => {
                "The destination answered with something that is not an MLLP frame".into()
            }
            ExchangeError::TooLarge => format!(
                "The response is larger than {} KB",
                MAX_RESPONSE_BYTES / 1024
            ),
            ExchangeError::Read(e) => format!("Reading the response failed: {e}"),
            ExchangeError::ExpectsTls => {
                "The target expects TLS: switch TLS on for this target".into()
            }
            ExchangeError::Tls(f) => match f {
                TlsFailure::Untrusted(what) => format!(
                    "Certificate not trusted: {what}. Add its CA to send.tls.ca_file, or \
                     switch the check off for this target (needs send.tls.allow_unverified)"
                ),
                TlsFailure::WrongName(detail) => {
                    format!("The certificate is for another name{detail}")
                }
                TlsFailure::Expired => "The target's certificate has expired".into(),
                TlsFailure::CertificateRequired => "The target asks for a certificate from \
                     Harux: set send.tls.client_cert_file and client_key_file"
                    .into(),
                TlsFailure::CertificateRefused => {
                    "The target refused Harux's certificate (send.tls.client_cert_file)".into()
                }
                TlsFailure::NoTlsAnswer => {
                    "No TLS answer — is this a plain MLLP port? Switch TLS off for this target"
                        .into()
                }
                TlsFailure::Other(e) => format!("TLS failed: {e}"),
            },
        }
    }
}

/// Connect to `host:port` within `limit`. `host` is an IP address or a name
/// (resolved here). The socket gets `TCP_NODELAY`: a frame written in parts
/// would otherwise wait for the peer's delayed ACK, adding tens of
/// milliseconds to every send and to the round-trip time Harux shows.
pub async fn connect(host: &str, port: u16, limit: Duration) -> Result<TcpStream, ExchangeError> {
    let stream = match timeout(limit, TcpStream::connect((host, port))).await {
        Err(_) => return Err(ExchangeError::ConnectTimeout),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            return Err(ExchangeError::Refused)
        }
        Ok(Err(e)) => return Err(ExchangeError::Unreachable(e)),
        Ok(Ok(s)) => s,
    };
    // Only a latency matter; the exchange works without it.
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

/// Connect to `host:port` and complete the TLS handshake, both within
/// `limit` (#273): the handshake counts into the connect timeout. Returns
/// the stream and the record of whether the target asked for a client
/// certificate, which [`after_handshake`] needs.
pub async fn connect_tls(
    host: &str,
    port: u16,
    limit: Duration,
    tls: &TlsContext,
    verify: bool,
) -> Result<(TlsStream<TcpStream>, Arc<ClientAuth>), ExchangeError> {
    let started = Instant::now();
    let tcp = connect(host, port, limit).await?;
    let (connector, auth) = tls
        .connector(verify)
        .map_err(|e| ExchangeError::Tls(TlsFailure::Other(e)))?;
    let name = ServerName::try_from(host.to_string())
        .map_err(|e| ExchangeError::Tls(TlsFailure::Other(e.to_string())))?;
    match timeout(
        limit.saturating_sub(started.elapsed()),
        connector.connect(name, tcp),
    )
    .await
    {
        Err(_) => Err(ExchangeError::Tls(TlsFailure::NoTlsAnswer)),
        Ok(Ok(stream)) => Ok((stream, auth)),
        Ok(Err(e)) => Err(ExchangeError::Tls(tls_failure(&e, &auth))),
    }
}

/// An exchange error on a TLS connection, re-read in the light of the
/// handshake (#273): with TLS 1.3 a target checks Harux's certificate only
/// after the client finished the handshake, so a missing or refused client
/// certificate shows up as an alert or a closed connection while reading.
pub fn after_handshake(e: ExchangeError, auth: &ClientAuth) -> ExchangeError {
    match e {
        ExchangeError::Read(io) => match rustls_error(&io) {
            Some(_) => ExchangeError::Tls(tls_failure(&io, auth)),
            None if auth.asked() => ExchangeError::Tls(client_cert_failure(auth)),
            None => ExchangeError::Read(io),
        },
        ExchangeError::Closed | ExchangeError::NotFramed if auth.asked() => {
            ExchangeError::Tls(client_cert_failure(auth))
        }
        e => e,
    }
}

fn client_cert_failure(auth: &ClientAuth) -> TlsFailure {
    if auth.had_certificate() {
        TlsFailure::CertificateRefused
    } else {
        TlsFailure::CertificateRequired
    }
}

fn rustls_error(e: &std::io::Error) -> Option<&rustls::Error> {
    e.get_ref()?.downcast_ref::<rustls::Error>()
}

/// The reason a person can act on, from the error rustls reported.
fn tls_failure(e: &std::io::Error, auth: &ClientAuth) -> TlsFailure {
    let Some(err) = rustls_error(e) else {
        // The connection ended without an alert: a peer that doesn't speak
        // TLS, or one that drops a client without the certificate it wants.
        return match e.kind() {
            _ if auth.asked() => client_cert_failure(auth),
            std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::InvalidData => TlsFailure::NoTlsAnswer,
            _ => TlsFailure::Other(e.to_string()),
        };
    };
    match err {
        rustls::Error::InvalidCertificate(CertificateError::Other(other)) => {
            match other.0.downcast_ref::<UntrustedCertificate>() {
                Some(u) => TlsFailure::Untrusted(u.to_string()),
                None => TlsFailure::Other(err.to_string()),
            }
        }
        rustls::Error::InvalidCertificate(CertificateError::UnknownIssuer) => {
            TlsFailure::Untrusted("issued by an unknown CA".into())
        }
        rustls::Error::InvalidCertificate(CertificateError::NotValidForNameContext {
            expected,
            presented,
        }) => TlsFailure::WrongName(format!(
            ": {expected:?} was asked for, the certificate is for {}",
            presented.join(", ")
        )),
        rustls::Error::InvalidCertificate(CertificateError::NotValidForName) => {
            TlsFailure::WrongName(String::new())
        }
        rustls::Error::InvalidCertificate(
            CertificateError::Expired | CertificateError::ExpiredContext { .. },
        ) => TlsFailure::Expired,
        rustls::Error::AlertReceived(
            AlertDescription::CertificateRequired
            | AlertDescription::BadCertificate
            | AlertDescription::UnknownCA
            | AlertDescription::HandshakeFailure,
        ) if auth.asked() => client_cert_failure(auth),
        rustls::Error::AlertReceived(AlertDescription::CertificateRequired) => {
            client_cert_failure(auth)
        }
        rustls::Error::InvalidMessage(_)
        | rustls::Error::InappropriateMessage { .. }
        | rustls::Error::InappropriateHandshakeMessage { .. } => TlsFailure::NoTlsAnswer,
        other => TlsFailure::Other(other.to_string()),
    }
}

/// One exchange on an open connection: write `payload` as one MLLP frame,
/// then — if `await_response` — read one frame back and return its payload.
/// Bytes after that frame are not read.
pub async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    payload: &[u8],
    await_response: bool,
    timeouts: Timeouts,
) -> Result<Option<Vec<u8>>, ExchangeError> {
    match timeout(timeouts.write, write_frame(stream, payload)).await {
        Err(_) => return Err(ExchangeError::WriteTimeout),
        Ok(Err(e)) => return Err(ExchangeError::Write(e)),
        Ok(Ok(())) => {}
    }
    if !await_response {
        return Ok(None);
    }
    match timeout(timeouts.response, read_frame(stream)).await {
        Err(_) => Err(ExchangeError::ResponseTimeout),
        Ok(result) => result.map(Some),
    }
}

/// Write `VT payload FS CR`: one write for a small payload, three for a
/// large one so the payload is written from where it is stored.
async fn write_frame<S: AsyncWrite + Unpin>(stream: &mut S, payload: &[u8]) -> std::io::Result<()> {
    if payload.len() < SINGLE_WRITE_MAX {
        let mut frame = Vec::with_capacity(payload.len() + 3);
        frame.push(MLLP_START);
        frame.extend_from_slice(payload);
        frame.extend_from_slice(&[MLLP_END_1, MLLP_END_2]);
        stream.write_all(&frame).await?;
    } else {
        stream.write_all(&[MLLP_START]).await?;
        stream.write_all(payload).await?;
        stream.write_all(&[MLLP_END_1, MLLP_END_2]).await?;
    }
    stream.flush().await
}

/// Read one MLLP frame and return its payload. Anything but whitespace
/// before the start byte means the peer does not speak MLLP — answered at
/// once instead of after the response timeout.
async fn read_frame<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Vec<u8>, ExchangeError> {
    let mut buf = vec![0u8; 16 * 1024];
    let mut acc: Vec<u8> = Vec::new();
    let mut scan_from = 0;
    loop {
        let n = stream.read(&mut buf).await.map_err(ExchangeError::Read)?;
        if n == 0 {
            return Err(if framed_so_far(&acc) {
                ExchangeError::Closed
            } else {
                ExchangeError::NotFramed
            });
        }
        acc.extend_from_slice(&buf[..n]);
        if !framed_so_far(&acc) {
            return Err(if speaks_tls(&acc) {
                ExchangeError::ExpectsTls
            } else {
                ExchangeError::NotFramed
            });
        }
        if let Some((payload, _)) = extract_mllp_frame(&acc, scan_from) {
            return Ok(payload.to_vec());
        }
        // +3: the envelope around a payload of the maximum size.
        if acc.len() > MAX_RESPONSE_BYTES + 3 {
            return Err(ExchangeError::TooLarge);
        }
        scan_from = acc.len().saturating_sub(1);
    }
}

/// A TLS record where an MLLP frame was expected: an alert (`0x15`, what a
/// TLS server answers a plain frame with) or a handshake (`0x16`), followed
/// by a TLS version (`0x03`).
fn speaks_tls(acc: &[u8]) -> bool {
    matches!(acc, [0x15 | 0x16, 0x03, ..])
}

/// Whether `acc` is (the start of) an MLLP frame: only whitespace before the
/// start byte. Nothing but whitespace yet counts as framed.
fn framed_so_far(acc: &[u8]) -> bool {
    acc.iter()
        .find(|b| !b.is_ascii_whitespace())
        .is_none_or(|&b| b == MLLP_START)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_check() {
        assert!(framed_so_far(b""));
        assert!(framed_so_far(b"\r\n"));
        assert!(framed_so_far(b"\r\n\x0bMSH"));
        assert!(!framed_so_far(b"HTTP/1.1 400"));
        assert!(speaks_tls(b"\x15\x03\x03\x00\x02\x02\x32"));
        assert!(!speaks_tls(b"\x15x"));
    }

    #[tokio::test]
    async fn small_and_large_payloads_are_framed_alike() {
        for size in [10, SINGLE_WRITE_MAX + 10] {
            let payload = vec![b'x'; size];
            let mut out = Vec::new();
            write_frame(&mut out, &payload).await.unwrap();
            assert_eq!(out.len(), size + 3);
            assert_eq!(out[0], MLLP_START);
            assert_eq!(&out[out.len() - 2..], &[MLLP_END_1, MLLP_END_2]);
        }
    }
}
