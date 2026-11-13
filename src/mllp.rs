use crate::config::MllpConfig;
use crate::hl7::charset::{decode_payload, DecodedPayload};
use crate::hl7::parser::{
    build_ack, build_nack, build_reject, parse_message, reply_charset, AckError,
};
use crate::hl7::types::{Hl7Message, WireForm};
use crate::store::MessageStore;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

/// MLLP framing constants — the single definition, also used by the
/// `/api/export` stream in `web.rs`.
pub(crate) const MLLP_START: u8 = 0x0B; // Vertical Tab (VT)
pub(crate) const MLLP_END_1: u8 = 0x1C; // File Separator (FS)
pub(crate) const MLLP_END_2: u8 = 0x0D; // Carriage Return (CR)

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

/// Pause before the next `accept()` after an error that is not about a
/// single connection (out of file descriptors, buffers or memory), so the
/// loop does not spin while the condition lasts (#224).
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(100);

/// Bytes of an over-limit frame kept for the store entry — enough for the MSH
/// header and the start of the message, never the payload (#223).
const OVERSIZE_KEEP_BYTES: usize = 4096;

/// Start the MLLP TCP server: bind `bind_addr`, then [`serve`]. Only a failed
/// bind is an error.
pub async fn start_mllp_server(
    bind_addr: &str,
    store: MessageStore,
    stats: MllpStats,
    shutdown: watch::Receiver<bool>,
    config: MllpConfig,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(bind_addr).await.map_err(|e| {
        anyhow::anyhow!("cannot listen on {bind_addr} (server.mllp_bind / mllp_port): {e}")
    })?;
    serve(listener, store, stats, shutdown, config).await;
    Ok(())
}

/// Accept MLLP connections on an already bound listener until `shutdown`
/// fires. Tests bind `127.0.0.1:0` and pass the listener in.
///
/// An `accept()` error is never fatal (#224): it ends the process for every
/// user, and the errors are usually transient (descriptor exhaustion) or
/// concern one connection only (`ECONNABORTED`). The first error of a streak
/// is logged, the recovery too; resource errors back off briefly.
pub async fn serve(
    listener: TcpListener,
    store: MessageStore,
    stats: MllpStats,
    mut shutdown: watch::Receiver<bool>,
    config: MllpConfig,
) {
    let max_connections = config.max_connections;
    let semaphore = Arc::new(Semaphore::new(max_connections));
    let config = Arc::new(config);
    let local = listener
        .local_addr()
        .map_or_else(|_| "?".to_string(), |a| a.to_string());
    info!(
        "MLLP server listening on {} (max {} connections)",
        local, max_connections
    );
    let mut accept_failures: u64 = 0;

    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => {
                info!("MLLP server shutting down gracefully");
                break;
            }
            result = listener.accept() => {
                let (socket, peer_addr) = match result {
                    Ok(accepted) => accepted,
                    Err(e) => {
                        if accept_failures == 0 {
                            warn!("Failed to accept an MLLP connection: {e} — still listening");
                        }
                        accept_failures += 1;
                        if !is_per_connection_error(&e) {
                            tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                        }
                        continue;
                    }
                };
                if accept_failures > 0 {
                    info!("Accepting MLLP connections again after {accept_failures} failed accept(s)");
                    accept_failures = 0;
                }
                let store = store.clone();
                let stats = stats.clone();
                let config = Arc::clone(&config);
                let peer = peer_addr.to_string();
                let shutdown_clone = shutdown.clone();

                // Try to acquire a connection permit
                match Arc::clone(&semaphore).try_acquire_owned() {
                    Ok(permit) => {
                        // Counted from here; the guard un-counts on every exit
                        // path of the task, including a panic (#235).
                        let guard = ConnectionGuard::new(&stats);
                        info!("New MLLP connection from {}", peer);

                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(socket, &peer, &store, &stats, &config, shutdown_clone).await {
                                warn!("Connection error from {}: {}", peer, e);
                            }
                            info!("MLLP connection closed: {}", peer);
                            drop(permit);
                            drop(guard);
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
}

/// `accept()` errors that only concern the connection being accepted; the
/// next `accept()` can follow immediately.
fn is_per_connection_error(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::Interrupted
    )
}

/// Counts one open connection in `active_connections` for as long as it
/// lives. Un-counting in `Drop` keeps the counter right when the connection
/// task ends by error or panic (#235), and wakes [`wait_for_drain`] when the
/// last connection goes.
struct ConnectionGuard {
    stats: MllpStats,
}

impl ConnectionGuard {
    fn new(stats: &MllpStats) -> Self {
        stats.active_connections.fetch_add(1, Ordering::SeqCst);
        Self {
            stats: stats.clone(),
        }
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        if self.stats.active_connections.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.stats.drain_notify.notify_one();
        }
    }
}

/// Wait until no MLLP connection is open, at most `limit`. Returns `false` on
/// timeout.
///
/// Waits for the *count*, not for one notification (#241): `notify_one`
/// stores a permit whenever the count drops to zero during normal operation,
/// so a single `notified().await` would return at once on a stale permit
/// while connections are still open. A stale permit now costs one extra loop
/// iteration, and a connection closing between the check and the await
/// leaves a permit, so no wake-up is lost.
pub async fn wait_for_drain(stats: &MllpStats, limit: Duration) -> bool {
    timeout(limit, async {
        while stats.active_connections.load(Ordering::SeqCst) > 0 {
            stats.drain_notify.notified().await;
        }
    })
    .await
    .is_ok()
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

    // Set while the rest of a rejected over-limit frame is skipped (#223):
    // the connection stays usable for the frames after it.
    let mut discarding = false;
    let mut last_was_fs = false;

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

        let mut data = &buf[..n];
        if discarding {
            match skip_to_frame_end(data, &mut last_was_fs) {
                Some(end) => {
                    discarding = false;
                    data = &data[end..];
                }
                None => data = &[],
            }
        }
        accumulated.extend_from_slice(data);

        // Process all complete MLLP frames in the buffer
        while let Some((payload, consumed)) = extract_mllp_frame(&accumulated, scan_offset) {
            stats.received.fetch_add(1, Ordering::Relaxed);
            let reply = if payload.len() > max_size {
                reject_oversize(payload, peer, store, stats, config).await
            } else {
                process_frame(payload, peer, store, stats).await
            };
            if let Some(reply) = reply {
                send_reply(&mut socket, &reply, peer, write_timeout).await;
            }

            // Remove processed bytes; reset scan position for the next frame.
            accumulated.drain(..consumed);
            scan_offset = 0;
        }

        // An incomplete frame already beyond the limit: answer now, then skip
        // the rest of it instead of buffering it (#223).
        if accumulated.len() > max_size {
            stats.received.fetch_add(1, Ordering::Relaxed);
            let body = match accumulated.iter().position(|&b| b == MLLP_START) {
                Some(p) => &accumulated[p + 1..],
                None => &accumulated[..],
            };
            if let Some(reply) = reject_oversize(body, peer, store, stats, config).await {
                send_reply(&mut socket, &reply, peer, write_timeout).await;
            }
            discarding = true;
            last_was_fs = accumulated.last() == Some(&MLLP_END_1);
            accumulated.clear();
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

/// Decode, parse, store one frame and return the reply to send, if any.
///
/// The message is stored before the reply goes out, so an ACK means the
/// message is in the store. The reply comes back as bytes in the sender's
/// own charset ([`reply_bytes`]).
async fn process_frame(
    payload: &[u8],
    peer: &str,
    store: &MessageStore,
    stats: &MllpStats,
) -> Option<Vec<u8>> {
    let mut msg = ingest(payload, peer);
    let reply = reply_for(&mut msg, peer, stats);
    let reply = reply.map(|text| reply_bytes(&text, &msg));
    store.insert(msg).await;
    reply
}

/// The reply to a stored message, recorded on it (`ack_response`,
/// `ack_code`): a NACK with MSA-3 + ERR for a parse error, no reply for an
/// incoming ACK (ACK storms), an ACK otherwise.
fn reply_for(msg: &mut Hl7Message, peer: &str, stats: &MllpStats) -> Option<String> {
    if let Some(e) = &msg.parse_error {
        stats.parse_errors.fetch_add(1, Ordering::Relaxed);
        warn!("Parse error from {}: {}", peer, e);
        // NACK (AE = Application Error) with the actual reason in MSA-3 and
        // an ERR segment, so the sender's log says what was wrong without
        // opening Harux (#218). A parser panic is Harux's fault (207), any
        // other parse error a structure the message doesn't have (100).
        let error = if e.starts_with(PARSER_PANIC_PREFIX) {
            AckError::InternalError
        } else {
            AckError::SegmentSequence
        };
        let nack = build_nack(
            error,
            &format!("Message parse error: {e}"),
            reply_charset(msg),
        );
        msg.ack_response = Some(nack.clone());
        msg.ack_code = Some("AE".to_string());
        Some(nack)
    } else {
        stats.parsed_ok.fetch_add(1, Ordering::Relaxed);
        // Never ACK an ACK — doing so would create an ACK storm
        if msg.message_type.starts_with("ACK") {
            debug!(
                "Received ACK message from {}, suppressing response to avoid ACK storm",
                peer
            );
            None
        } else {
            let ack = build_ack(msg, "AA");
            msg.ack_response = Some(ack.clone());
            msg.ack_code = Some("AA".to_string());
            Some(ack)
        }
    }
}

/// The reply as bytes in the charset the sender's payload was written in
/// (#266 review): a Latin-1 sender gets its echoed MSH-3/4 and the quoted
/// start of a broken frame back in Latin-1, not as UTF-8 mojibake. Harux's
/// own texts in the reply are ASCII, so they fit any charset. Payloads that
/// were UTF-8, or whose bytes no encoding reproduces, are answered in UTF-8.
fn reply_bytes(reply: &str, msg: &Hl7Message) -> Vec<u8> {
    match msg.wire {
        WireForm::Encoded(encoding) => encoding.encode(reply).0.into_owned(),
        WireForm::Utf8 | WireForm::Original(_) => reply.as_bytes().to_vec(),
    }
}

/// Turn a frame's bytes into the message to store: parsed, or — when parsing
/// fails *or panics* — a failed message carrying `parse_error`. Never panics
/// (#235): a parser bug becomes a visible parse error and a NACK instead of a
/// silently dropped connection. Public only for `tests/fuzz_ingest.rs`
/// (#243); not part of the supported API — `serve` is.
#[doc(hidden)]
pub fn ingest(payload: &[u8], peer: &str) -> Hl7Message {
    ingest_with(payload, peer, parse_message)
}

/// Start of the `parse_error` of a frame whose parser panicked. `reply_for`
/// answers those with ERR-3 `207` instead of `100` — one constant, so the
/// two can't drift apart (#266 review).
const PARSER_PANIC_PREFIX: &str = "Internal error while parsing the message";

fn ingest_with(
    payload: &[u8],
    peer: &str,
    parse: impl FnOnce(&str, &str) -> Result<Hl7Message, String>,
) -> Hl7Message {
    // Bytes → text per MSH-18, with a fallback (and a warning on the
    // message) when the declared charset doesn't fit the bytes (#188).
    let decoded = catch_panic(|| decode_payload(payload)).unwrap_or_else(|p| {
        error!("Internal error while decoding a frame from {peer}: {p}");
        DecodedPayload {
            text: String::from_utf8_lossy(payload).into_owned(),
            declared: None,
            warning: None,
            encoding: encoding_rs::UTF_8,
        }
    });
    if let Some(w) = &decoded.warning {
        warn!("Charset fallback for message from {}: {}", peer, w.message);
    }

    let parsed = catch_panic(|| parse(&decoded.text, peer)).unwrap_or_else(|p| {
        error!("Internal error while parsing a frame from {peer}: {p}");
        Err(format!("{PARSER_PANIC_PREFIX}: {p}"))
    });
    let mut msg = match parsed {
        Ok(msg) => msg,
        Err(e) => {
            let mut failed = Hl7Message::new_empty(decoded.text, peer.to_string());
            failed.message_type = "UNKNOWN".to_string();
            failed.parse_error = Some(e);
            failed
        }
    };
    msg.wire = wire_form(payload, &msg.raw, decoded.encoding);
    msg.charset = decoded.declared;
    msg.validation_warnings.extend(decoded.warning);
    msg
}

/// How to give back the received `payload` from the stored `raw` text
/// (#230). Only when re-encoding can't reproduce the bytes exactly (the
/// decode replaced something) is a second copy kept.
fn wire_form(payload: &[u8], raw: &str, encoding: &'static encoding_rs::Encoding) -> WireForm {
    if raw.as_bytes() == payload {
        return WireForm::Utf8;
    }
    if encoding != encoding_rs::UTF_8 {
        let (bytes, _, unmappable) = encoding.encode(raw);
        if !unmappable && bytes.as_ref() == payload {
            return WireForm::Encoded(encoding);
        }
    }
    WireForm::Original(payload.into())
}

/// Run `f`, turning a panic into its message.
fn catch_panic<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|p| {
        p.downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string())
    })
}

/// Store a visible entry for a frame above `mllp.max_message_size_mb` and
/// return the reject (`AR`) to send (#223). `frame` is the frame content
/// received so far, without the start byte; only its first
/// [`OVERSIZE_KEEP_BYTES`] are kept.
async fn reject_oversize(
    frame: &[u8],
    peer: &str,
    store: &MessageStore,
    stats: &MllpStats,
    config: &MllpConfig,
) -> Option<Vec<u8>> {
    stats.parse_errors.fetch_add(1, Ordering::Relaxed);
    warn!(
        "Message from {} exceeds {} MB (mllp.max_message_size_mb) — rejected with AR",
        peer, config.max_message_size_mb
    );
    let mut msg = oversize_message(frame, config.max_message_size_mb, peer);
    // In the sender's delimiters when its MSH header was readable
    // (`oversize_message` copies them), so the sender can parse the reject.
    let reject = build_reject(
        msg.delimiters,
        &msg.message_control_id,
        &format!(
            "Message exceeds the maximum size of {} MB",
            config.max_message_size_mb
        ),
    );
    msg.ack_response = Some(reject.clone());
    msg.ack_code = Some("AR".to_string());
    store.insert(msg).await;
    // The frame was not decoded per MSH-18 (only its head is kept), so the
    // reject is UTF-8: Harux's ASCII text and the echoed MSH-10.
    Some(reject.into_bytes())
}

/// The store entry for an over-limit frame: its first bytes as `raw`, header
/// fields from the MSH line when it can be read, and a `parse_error` naming
/// the limit.
fn oversize_message(frame: &[u8], limit_mb: usize, peer: &str) -> Hl7Message {
    let head = String::from_utf8_lossy(&frame[..frame.len().min(OVERSIZE_KEEP_BYTES)]).into_owned();

    let mut msg = Hl7Message::new_empty(head, peer.to_string());
    msg.message_type = "UNKNOWN".to_string();
    // Only the beginning is kept: it can't be resent (#272).
    msg.truncated = true;
    let header_line = msg
        .raw
        .trim_start()
        .split(['\r', '\n'])
        .next()
        .unwrap_or("");
    if let Ok(Ok(h)) = catch_panic(|| parse_message(header_line, peer)) {
        msg.message_type = h.message_type;
        msg.trigger_event = h.trigger_event;
        msg.message_control_id = h.message_control_id;
        msg.sending_application = h.sending_application;
        msg.sending_facility = h.sending_facility;
        msg.receiving_application = h.receiving_application;
        msg.receiving_facility = h.receiving_facility;
        msg.version = h.version;
        msg.delimiters = h.delimiters;
    }
    msg.parse_error = Some(format!(
        "Message exceeds mllp.max_message_size_mb ({limit_mb} MB) and was rejected with AR; \
         only its first {} bytes are kept here",
        msg.raw.len()
    ));
    msg
}

/// Find the end (`FS CR`) of a frame being skipped. `last_was_fs` carries a
/// trailing `FS` across reads. Returns the index just past the `CR`.
fn skip_to_frame_end(data: &[u8], last_was_fs: &mut bool) -> Option<usize> {
    for (i, &b) in data.iter().enumerate() {
        if *last_was_fs && b == MLLP_END_2 {
            *last_was_fs = false;
            return Some(i + 1);
        }
        *last_was_fs = b == MLLP_END_1;
    }
    None
}

async fn send_reply(
    socket: &mut tokio::net::TcpStream,
    reply: &[u8],
    peer: &str,
    write_timeout: Duration,
) {
    match timeout(write_timeout, socket.write_all(&wrap_mllp(reply))).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => warn!("Failed to send ACK/NACK to {}: {}", peer, e),
        Err(_) => warn!("Write timeout sending ACK/NACK to {}", peer),
    }
}

/// Extract one complete MLLP frame from the buffer.
///
/// `scan_from` is a hint: the caller tracks how far it has already scanned for
/// the end sequence across partial reads, so only newly arrived bytes are searched.
/// Pass `0` for the first call on a fresh buffer; reset to `0` after consuming a frame.
///
/// Returns `(payload, bytes_consumed)`, or `None` if the frame is incomplete.
/// The payload is still raw bytes: decoding by MSH-18 is
/// `hl7::charset::decode_payload`'s job (#188). Also reads the responses to
/// what Harux sends (`mllp_client`, #272).
pub(crate) fn extract_mllp_frame(buf: &[u8], scan_from: usize) -> Option<(&[u8], usize)> {
    // Find start byte
    let start_pos = buf.iter().position(|&b| b == MLLP_START)?;

    // Resume end-sequence search from where the previous call stopped, never before start+1.
    let search_from = (start_pos + 1).max(scan_from);

    // Find end sequence (FS + CR)
    for i in search_from..buf.len().saturating_sub(1) {
        if buf[i] == MLLP_END_1 && buf[i + 1] == MLLP_END_2 {
            return Some((&buf[start_pos + 1..i], i + 2));
        }
    }

    None // Incomplete frame
}

/// Wrap a message in MLLP framing
fn wrap_mllp(message: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(message.len() + 3);
    frame.push(MLLP_START);
    frame.extend_from_slice(message);
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

        let (extracted, consumed) = extract_mllp_frame(&frame, 0).unwrap();
        assert_eq!(extracted, msg.as_bytes());
        assert_eq!(consumed, frame.len());
    }

    /// Framing hands over the raw bytes untouched (a Latin-1 `ä` stays one
    /// byte); `decode_payload` then honours MSH-18.
    #[test]
    fn test_extract_mllp_frame_latin1() {
        let msg_bytes = b"MSH|^~\\&||||||||||||||||8859/1\rPID|||\xE4\r".to_vec();
        let mut frame = vec![MLLP_START];
        frame.extend_from_slice(&msg_bytes);
        frame.push(MLLP_END_1);
        frame.push(MLLP_END_2);

        let (extracted, consumed) = extract_mllp_frame(&frame, 0).unwrap();
        assert_eq!(extracted, &msg_bytes[..]);
        assert_eq!(consumed, frame.len());

        let decoded = decode_payload(extracted);
        assert_eq!(decoded.declared.as_deref(), Some("8859/1"));
        assert!(decoded.text.contains('ä'));
        assert!(decoded.warning.is_none());
    }

    #[test]
    fn test_incomplete_frame() {
        let frame = vec![MLLP_START, b'M', b'S', b'H'];
        assert!(extract_mllp_frame(&frame, 0).is_none());
    }

    #[test]
    fn test_wrap_mllp() {
        let wrapped = wrap_mllp(b"TEST");
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

    /// #235: a panicking parser yields a stored parse error, not a lost frame.
    #[test]
    fn test_ingest_turns_parser_panic_into_parse_error() {
        let msg = ingest_with(b"MSH|^~\\&|A|B", "t", |_, _| panic!("boom"));
        assert_eq!(msg.message_type, "UNKNOWN");
        assert_eq!(msg.raw, "MSH|^~\\&|A|B");
        let err = msg.parse_error.clone().unwrap();
        assert!(
            err.starts_with(PARSER_PANIC_PREFIX) && err.contains("boom"),
            "{err}"
        );
    }

    /// #218 / #266 review: a parser panic is answered with ERR-3 `207`
    /// (Harux's fault), any other parse error with `100`; an ACK has no ERR.
    #[test]
    fn test_reply_error_codes() {
        let stats = MllpStats::new();
        let mut panicked = ingest_with(b"MSH|^~\\&|A|B", "t", |_, _| panic!("boom"));
        let nack = reply_for(&mut panicked, "t", &stats).unwrap();
        assert!(
            nack.contains("\rERR|||207^Application internal error^HL70357|E"),
            "{nack}"
        );
        assert_eq!(panicked.ack_code.as_deref(), Some("AE"));

        let mut broken = ingest(b"NOT HL7", "t");
        let nack = reply_for(&mut broken, "t", &stats).unwrap();
        assert!(
            nack.contains("\rERR|||100^Segment sequence error^HL70357|E"),
            "{nack}"
        );

        let mut ok = ingest(b"MSH|^~\\&|A|B|C|D|20240101||ADT^A01|C1|P|2.5\r", "t");
        let ack = reply_for(&mut ok, "t", &stats).unwrap();
        assert!(ack.contains("MSA|AA|C1") && !ack.contains("ERR"), "{ack}");
        assert_eq!(stats.parse_errors.load(Ordering::Relaxed), 2);
    }

    /// #222 inputs go through the real pipeline without panicking.
    #[test]
    fn test_ingest_multibyte_inputs() {
        let m = ingest("Sehr geehrte Damen ä und Herren".as_bytes(), "t");
        assert!(m
            .parse_error
            .unwrap()
            .starts_with("Message does not start with MSH"));
        let m = ingest(
            "MSH|^~\\&|A|F|R|RF|20240101120000||ADT^A01|P1|P|2.5\rEVN||20240101\rPID|||1^^^H||M^J||1980-05ä|M\rPV1||I"
                .as_bytes(),
            "t",
        );
        assert!(m.parse_error.is_none());
        assert!(m
            .validation_warnings
            .iter()
            .any(|w| w.code == "INVALID_DATATYPE" && w.segment == "PID" && w.field == Some(7)));
    }

    #[test]
    fn test_oversize_message_keeps_header_only() {
        let mut frame =
            b"MSH|^~\\&|DMS|KLINIK|R|RZ|20240101||MDM^T02|BIG1|P|2.5\rOBX|1|ED|X||".to_vec();
        frame.extend(std::iter::repeat_n(b'Q', 100_000));
        let m = oversize_message(&frame, 1, "t");
        assert_eq!(m.raw.len(), OVERSIZE_KEEP_BYTES);
        assert!(m.truncated, "cut entries can't be resent (#272)");
        assert_eq!(m.message_type, "MDM^T02");
        assert_eq!(m.message_control_id, "BIG1");
        assert!(m
            .parse_error
            .unwrap()
            .contains("max_message_size_mb (1 MB)"));

        let m = oversize_message(&[b'x'; 10], 1, "t");
        assert_eq!(m.message_type, "UNKNOWN");
        assert_eq!(m.message_control_id, "");
    }

    #[test]
    fn test_skip_to_frame_end() {
        let mut fs = false;
        assert_eq!(skip_to_frame_end(b"abc", &mut fs), None);
        assert_eq!(skip_to_frame_end(b"ab\x1c\x0dNEXT", &mut fs), Some(4));
        // FS and CR in different reads
        assert_eq!(skip_to_frame_end(b"ab\x1c", &mut fs), None);
        assert!(fs);
        assert_eq!(skip_to_frame_end(b"\x0dNEXT", &mut fs), Some(1));
        // CR without FS does not end the frame
        assert_eq!(skip_to_frame_end(b"\x0d\x0d", &mut fs), None);
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
        assert_eq!(result_incremental, result_full);
    }
}
