//! The sending side of MLLP (#272): open a connection and do one exchange on
//! it — write `VT payload FS CR`, read until `FS CR`. Knows nothing about the
//! store; `send.rs` decides what is sent and records the outcome.
//!
//! Framing uses the constants and the frame finder of `mllp.rs`, so there is
//! one definition of the envelope for receiving, exporting and sending.

use crate::mllp::{extract_mllp_frame, MLLP_END_1, MLLP_END_2, MLLP_START};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

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
            return Err(ExchangeError::NotFramed);
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
