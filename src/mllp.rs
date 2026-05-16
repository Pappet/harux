use crate::config::MllpConfig;
use crate::hl7::parser::{build_ack, build_nack, parse_message};
use crate::hl7::types::Hl7Message;
use crate::store::MessageStore;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tracing::{debug, info, warn};

/// MLLP framing constants
const MLLP_START: u8 = 0x0B; // Vertical Tab (VT)
const MLLP_END_1: u8 = 0x1C; // File Separator (FS)
const MLLP_END_2: u8 = 0x0D; // Carriage Return (CR)

/// Stats for the MLLP server
#[derive(Clone)]
pub struct MllpStats {
    pub received: Arc<AtomicU64>,
    pub parsed_ok: Arc<AtomicU64>,
    pub parse_errors: Arc<AtomicU64>,
    pub active_connections: Arc<AtomicU64>,
    pub rejected_connections: Arc<AtomicU64>,
    /// Notified whenever `active_connections` drops to zero, enabling a
    /// non-polling graceful-shutdown wait in `main`.
    pub drain_notify: Arc<tokio::sync::Notify>,
}

impl MllpStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset message-derived counters when the store is cleared.
    /// `active_connections` and `rejected_connections` are live network
    /// state and are intentionally left untouched.
    pub fn reset_message_counters(&self) {
        self.received.store(0, Ordering::Relaxed);
        self.parsed_ok.store(0, Ordering::Relaxed);
        self.parse_errors.store(0, Ordering::Relaxed);
    }
}

impl Default for MllpStats {
    fn default() -> Self {
        Self {
            received: Arc::new(AtomicU64::new(0)),
            parsed_ok: Arc::new(AtomicU64::new(0)),
            parse_errors: Arc::new(AtomicU64::new(0)),
            active_connections: Arc::new(AtomicU64::new(0)),
            rejected_connections: Arc::new(AtomicU64::new(0)),
            drain_notify: Arc::new(tokio::sync::Notify::new()),
        }
    }
}

/// Start the MLLP TCP server
pub async fn start_mllp_server(
    bind_addr: &str,
    store: MessageStore,
    stats: MllpStats,
    mut shutdown: watch::Receiver<bool>,
    config: MllpConfig,
) -> anyhow::Result<()> {
    let max_connections = config.max_connections;
    let semaphore = Arc::new(Semaphore::new(max_connections));
    let config = Arc::new(config);
    let listener = TcpListener::bind(bind_addr).await?;
    info!(
        "MLLP server listening on {} (max {} connections)",
        bind_addr, max_connections
    );

    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => {
                info!("MLLP server shutting down gracefully");
                break;
            }
            result = listener.accept() => {
                let (socket, peer_addr) = result?;
                let store = store.clone();
                let stats = stats.clone();
                let config = Arc::clone(&config);
                let peer = peer_addr.to_string();
                let shutdown_clone = shutdown.clone();

                // Try to acquire a connection permit
                match Arc::clone(&semaphore).try_acquire_owned() {
                    Ok(permit) => {
                        stats.active_connections.fetch_add(1, Ordering::Relaxed);
                        info!("New MLLP connection from {}", peer);

                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(socket, &peer, &store, &stats, &config, shutdown_clone).await {
                                warn!("Connection error from {}: {}", peer, e);
                            }
                            let remaining = stats.active_connections.fetch_sub(1, Ordering::Relaxed);
                            info!("MLLP connection closed: {}", peer);
                            drop(permit); // Release the semaphore permit
                            if remaining == 1 {
                                // We were the last connection; wake the drain waiter in main.
                                stats.drain_notify.notify_one();
                            }
                        });
                    }
                    Err(_) => {
                        stats.rejected_connections.fetch_add(1, Ordering::Relaxed);
                        warn!(
                            "Connection from {} rejected: max connections ({}) reached",
                            peer, max_connections
                        );
                        // Socket is dropped here, closing the TCP connection
                        drop(socket);
                    }
                }
            }
        }
    }

    Ok(())
}

async fn handle_connection(
    mut socket: tokio::net::TcpStream,
    peer: &str,
    store: &MessageStore,
    stats: &MllpStats,
    config: &MllpConfig,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let mut buf = vec![0u8; 64 * 1024]; // 64 KB read buffer
    let mut accumulated = Vec::with_capacity(8 * 1024);
    let max_size = config.max_message_size();
    let read_timeout = config.read_timeout();
    let write_timeout = config.write_timeout();

    // Tracks the farthest byte already scanned for the MLLP end sequence.
    // Avoids O(n²) re-scanning of large messages arriving in many small chunks.
    let mut scan_offset: usize = 0;

    let mut shutdown_requested = false;

    loop {
        let n = tokio::select! {
            result = timeout(read_timeout, socket.read(&mut buf)) => {
                match result {
                    Ok(Ok(0)) => {
                        debug!("Client {} disconnected cleanly", peer);
                        break;
                    }
                    Err(_) => {
                        debug!("Read timeout — closing idle connection from {}", peer);
                        break;
                    }
                    Ok(Ok(n)) => n,
                    Ok(Err(e)) => {
                        warn!("Socket error from {}: {}", peer, e);
                        break;
                    }
                }
            }
            _ = shutdown.changed(), if !shutdown_requested => {
                shutdown_requested = true;
                if accumulated.is_empty() {
                    break;
                }
                // Continue reading to finish the current frame
                continue;
            }
        };

        accumulated.extend_from_slice(&buf[..n]);

        if accumulated.len() > max_size {
            warn!(
                "Buffer exceeded {} MB from {}, closing connection",
                config.max_message_size_mb, peer
            );
            return Ok(());
        }

        // Process all complete MLLP frames in the buffer
        while let Some((message, consumed, charset)) = extract_mllp_frame(&accumulated, scan_offset)
        {
            stats.received.fetch_add(1, Ordering::Relaxed);

            match parse_message(&message, peer) {
                Ok(mut msg) => {
                    msg.charset = charset.clone();
                    stats.parsed_ok.fetch_add(1, Ordering::Relaxed);

                    // Never ACK an ACK — doing so would create an ACK storm
                    if msg.message_type.starts_with("ACK") {
                        debug!(
                            "Received ACK message from {}, suppressing response to avoid ACK storm",
                            peer
                        );
                    } else {
                        // Build and send ACK
                        let ack = build_ack(&msg, "AA");
                        msg.ack_response = Some(ack.clone());
                        msg.ack_code = Some("AA".to_string());
                        let ack_frame = wrap_mllp(&ack);
                        match timeout(write_timeout, socket.write_all(&ack_frame)).await {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => warn!("Failed to send ACK to {}: {}", peer, e),
                            Err(_) => warn!("Write timeout sending ACK to {}", peer),
                        }
                    }

                    // Store the message (async, non-blocking for the connection)
                    store.insert(msg).await;
                }
                Err(e) => {
                    stats.parse_errors.fetch_add(1, Ordering::Relaxed);
                    warn!("Parse error from {}: {}", peer, e);

                    // Send NACK (AE = Application Error)
                    let nack = build_nack("Message parse error");

                    // Store the failed message so it is visible in the UI
                    let mut failed = Hl7Message::new_empty(message.clone(), peer.to_string());
                    failed.charset = charset.clone();
                    failed.message_type = "UNKNOWN".to_string();
                    failed.parse_error = Some(e.clone());
                    failed.ack_response = Some(nack.clone());
                    failed.ack_code = Some("AE".to_string());
                    store.insert(failed).await;

                    let nack_frame = wrap_mllp(&nack);
                    let _ = timeout(write_timeout, socket.write_all(&nack_frame)).await;
                }
            }

            // Remove processed bytes; reset scan position for the next frame.
            accumulated.drain(..consumed);
            scan_offset = 0;
        }
        // No complete frame yet — advance past already-scanned bytes on the next read.
        // Subtract 1 so we re-check the last byte in case FS+CR straddles a chunk boundary.
        scan_offset = accumulated.len().saturating_sub(1);

        if shutdown_requested && accumulated.is_empty() {
            info!("Gracefully closing connection from {} after draining", peer);
            break;
        }
    }

    Ok(())
}

/// Extract one complete MLLP frame from the buffer.
///
/// `scan_from` is a hint: the caller tracks how far it has already scanned for
/// the end sequence across partial reads, so only newly arrived bytes are searched.
/// Pass `0` for the first call on a fresh buffer; reset to `0` after consuming a frame.
///
/// Returns (message_content, bytes_consumed, detected_charset) or None if incomplete.
fn extract_mllp_frame(buf: &[u8], scan_from: usize) -> Option<(String, usize, Option<String>)> {
    // Find start byte
    let start_pos = buf.iter().position(|&b| b == MLLP_START)?;

    // Resume end-sequence search from where the previous call stopped, never before start+1.
    let search_from = (start_pos + 1).max(scan_from);

    // Find end sequence (FS + CR)
    for i in search_from..buf.len().saturating_sub(1) {
        if buf[i] == MLLP_END_1 && buf[i + 1] == MLLP_END_2 {
            let message_bytes = &buf[start_pos + 1..i];

            let charset = extract_msh18(message_bytes);
            let message = if let Some(cs) = &charset {
                let normalized = cs.replace("/", "-");
                let label = if normalized.starts_with("8859-") {
                    format!("iso-{}", normalized)
                } else {
                    normalized
                };
                if let Some(encoding) = encoding_rs::Encoding::for_label(label.as_bytes()) {
                    let (cow, _, _) = encoding.decode(message_bytes);
                    cow.into_owned()
                } else {
                    String::from_utf8_lossy(message_bytes).to_string()
                }
            } else {
                String::from_utf8_lossy(message_bytes).to_string()
            };

            return Some((message, i + 2, charset));
        }
    }

    None // Incomplete frame
}

fn extract_msh18(bytes: &[u8]) -> Option<String> {
    // Find the first \r to isolate the MSH segment
    let end_of_msh = bytes
        .iter()
        .position(|&b| b == b'\r')
        .unwrap_or(bytes.len());
    let msh_bytes = &bytes[..end_of_msh];

    if msh_bytes.len() < 5 || &msh_bytes[0..3] != b"MSH" {
        return None;
    }

    let separator = msh_bytes[3];
    let mut parts = msh_bytes.split(|&b| b == separator);
    // MSH-1 is the separator itself. MSH-2 is index 1.
    // ...
    // MSH-18 is index 17.
    let msh18_part = parts.nth(17)?;

    String::from_utf8(msh18_part.to_vec())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Wrap a message in MLLP framing
fn wrap_mllp(message: &str) -> Vec<u8> {
    let mut frame = Vec::with_capacity(message.len() + 3);
    frame.push(MLLP_START);
    frame.extend_from_slice(message.as_bytes());
    frame.push(MLLP_END_1);
    frame.push(MLLP_END_2);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_mllp_frame() {
        let msg = "MSH|^~\\&|TEST\rPID|||123";
        let mut frame = vec![MLLP_START];
        frame.extend_from_slice(msg.as_bytes());
        frame.push(MLLP_END_1);
        frame.push(MLLP_END_2);

        let (extracted, consumed, charset) = extract_mllp_frame(&frame, 0).unwrap();
        assert_eq!(extracted, msg);
        assert_eq!(consumed, frame.len());
        assert_eq!(charset, None);
    }

    #[test]
    fn test_extract_mllp_frame_latin1() {
        // MSH|^~\&|...|8859/1
        // We put Latin-1 byte \xE4 which is 'ä'
        let msg_bytes = b"MSH|^~\\&||||||||||||||||8859/1\rPID|||\xE4\r".to_vec();
        let mut frame = vec![MLLP_START];
        frame.extend_from_slice(&msg_bytes);
        frame.push(MLLP_END_1);
        frame.push(MLLP_END_2);

        let (extracted, consumed, charset) = extract_mllp_frame(&frame, 0).unwrap();
        assert_eq!(charset.as_deref(), Some("8859/1"));
        assert_eq!(consumed, frame.len());
        // \xE4 should be decoded as 'ä'
        assert!(extracted.contains('ä'));
    }

    #[test]
    fn test_incomplete_frame() {
        let frame = vec![MLLP_START, b'M', b'S', b'H'];
        assert!(extract_mllp_frame(&frame, 0).is_none());
    }

    #[test]
    fn test_wrap_mllp() {
        let wrapped = wrap_mllp("TEST");
        assert_eq!(wrapped[0], MLLP_START);
        assert_eq!(wrapped[wrapped.len() - 2], MLLP_END_1);
        assert_eq!(wrapped[wrapped.len() - 1], MLLP_END_2);
    }

    #[test]
    fn test_mllp_stats_new() {
        let stats = MllpStats::new();
        assert_eq!(stats.received.load(Ordering::Relaxed), 0);
        assert_eq!(stats.parsed_ok.load(Ordering::Relaxed), 0);
        assert_eq!(stats.parse_errors.load(Ordering::Relaxed), 0);
        assert_eq!(stats.active_connections.load(Ordering::Relaxed), 0);
        assert_eq!(stats.rejected_connections.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_scan_offset_skips_already_scanned_bytes() {
        // Simulate a large frame arriving in two chunks: the end sequence is
        // only present in the second chunk. Verify that passing the correct
        // scan_from skips re-scanning the first chunk entirely.
        let payload = b"MSH|^~\\&|A|B|||20200101||ADT^A01|1|P|2.5";
        let mut frame: Vec<u8> = vec![MLLP_START];
        frame.extend_from_slice(payload);

        // First chunk: no end sequence yet — scan_from=0 returns None.
        assert!(extract_mllp_frame(&frame, 0).is_none());
        // Advance scan_from past already-checked bytes.
        let scan_from = frame.len().saturating_sub(1);

        // Second chunk: append end sequence.
        frame.push(MLLP_END_1);
        frame.push(MLLP_END_2);

        // With correct scan_from the frame is found; with scan_from=0 it would also
        // be found but would re-scan unnecessarily. Both must return the same result.
        let result_incremental = extract_mllp_frame(&frame, scan_from);
        let result_full = extract_mllp_frame(&frame, 0);
        assert!(result_incremental.is_some());
        assert_eq!(
            result_incremental.map(|(m, c, _)| (m, c)),
            result_full.map(|(m, c, _)| (m, c))
        );
    }
}
