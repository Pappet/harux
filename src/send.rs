//! Sending stored messages to other MLLP receivers (#272, Milestone 4).
//!
//! A **run** is one request to send. This package sends one stored message,
//! unchanged — the bytes it arrived as (`Hl7Message::wire_bytes`, #230) —
//! to the destination the request names. The rules (`AGENTS.md`, *Sending*):
//!
//! 1. Everything Harux sends, or tries to send, is stored, **before** it
//!    connects: a refused or timed-out send is a row too, and whatever the
//!    destination forwards because of the send is listed after it.
//! 2. Sending is off until `send.enabled`; `send.allowed_destinations` can
//!    narrow it. The server keeps no list of targets: each request names one.
//! 3. Every send ends with an outcome (`set_delivery`): a response, a
//!    transport failure, shutdown or a panic — no row stays pending.
//! 4. No store lock is held across network I/O; the exchange runs in a task
//!    of its own, so a browser that gives up does not stop a send half-way.

use crate::config::{normalize_host, Config, DestinationRule};
use crate::hl7::types::{
    Delivery, DeliveryState, Hl7Message, Origin, Outbound, Transport, MAX_STORED_RESPONSE_BYTES,
};
use crate::mllp_client::{self, Timeouts};
use crate::store::MessageStore;
use crate::tls::TlsContext;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{watch, Semaphore};
use tracing::{info, warn};

/// The label of the loopback target (Harux's own MLLP listener).
pub const LOOPBACK_NAME: &str = "This Harux";

/// Longest target name kept; it is a label, shown in the row.
const MAX_TARGET_NAME_CHARS: usize = 64;

/// What the sender needs from the configuration (`[send]`, `[mllp]`,
/// `[store]`). Built once at start; tests build it by hand.
#[derive(Debug, Clone)]
pub struct SendSettings {
    pub enabled: bool,
    /// Harux's own listener, offered as the target "This Harux" and always
    /// allowed. `None` when `send.loopback_target` is off.
    pub loopback: Option<SocketAddr>,
    /// `send.allowed_destinations`; empty = any destination.
    pub rules: Vec<DestinationRule>,
    pub timeouts: Timeouts,
    pub max_run_messages: usize,
    pub max_run_store_percent: usize,
    /// `max_run_store_percent` of `store.max_memory_mb`, in bytes.
    pub max_run_bytes: usize,
    pub max_concurrent_runs: usize,
    /// `mllp.max_message_size_mb`: the largest message that can be sent is
    /// the largest that can be received.
    pub max_message_bytes: usize,
    /// `[send.tls]`, read at start when sending is on (#273).
    pub tls: Option<Arc<TlsContext>>,
}

impl SendSettings {
    pub fn from_config(config: &Config) -> Result<Self, String> {
        let s = &config.send;
        Ok(Self {
            enabled: s.enabled,
            loopback: config.loopback_destination(),
            rules: s.destination_rules()?,
            timeouts: Timeouts {
                connect: Duration::from_secs(s.connect_timeout_secs),
                write: config.mllp.write_timeout(),
                response: Duration::from_secs(s.response_timeout_secs),
            },
            max_run_messages: s.max_run_messages,
            max_run_store_percent: s.max_run_store_percent,
            max_run_bytes: config
                .store
                .max_memory_bytes()
                .saturating_mul(s.max_run_store_percent)
                / 100,
            max_concurrent_runs: s.max_concurrent_runs.max(1),
            max_message_bytes: config.mllp.max_message_size(),
            // The OS store and the files are read only when sending is on;
            // a file problem then stops the start (#273).
            tls: if s.enabled {
                Some(Arc::new(TlsContext::load(&s.tls, true)?))
            } else {
                None
            },
        })
    }
}

// --- Request ---

/// `POST /api/send` (#272). Unknown fields are refused, so a request that
/// asks for something this version does not do (`set`, `repeat`, …) fails
/// instead of sending the message unchanged.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendRequest {
    pub target: TargetRequest,
    pub items: Vec<ItemRequest>,
    /// `false`: write the message and do not wait for an answer.
    #[serde(default = "yes")]
    pub await_response: bool,
    /// Send a stored message that did not parse, as it was received.
    #[serde(default)]
    pub allow_invalid: bool,
    /// Field overrides (#275): `{"MSH-10": "{{id}}", "MSH-7": "{{now}}"}`.
    /// Values are a field's raw text and may use placeholders (`{{now}}`,
    /// `{{today}}`, `{{id}}`, `{{n}}`, `{{rand:N}}`). Empty or missing: the
    /// message goes out byte for byte as received.
    #[serde(default)]
    pub set: std::collections::BTreeMap<String, String>,
}

fn yes() -> bool {
    true
}

/// `"loopback"` or a target of the user's own list.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum TargetRequest {
    Named(String),
    Explicit(ExplicitTarget),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplicitTarget {
    /// The user's label, shown in the list. Defaults to `host:port`.
    #[serde(default)]
    pub name: Option<String>,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub tls: bool,
    /// With TLS: check the target's certificate. `false` only with
    /// `send.tls.allow_unverified`. Ignored without TLS.
    #[serde(default = "yes")]
    pub verify: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemRequest {
    pub message_id: String,
}

/// The answer to a run of one message: the sent message's ID and outcome.
#[derive(Debug, Serialize)]
pub struct SendResult {
    pub id: String,
    pub delivery: Delivery,
}

/// Why a request was refused, by HTTP status.
#[derive(Debug, PartialEq, Eq)]
pub enum SendError {
    /// 403: `send.enabled` is off.
    Disabled,
    /// 400: malformed request, host or target.
    BadRequest(String),
    /// 403: destination outside `send.allowed_destinations`, or loopback off.
    Forbidden(String),
    /// 404: unknown message.
    NotFound(String),
    /// 413: the message is too large to send.
    TooLarge(String),
    /// 422: the message cannot be sent (cut off, does not parse).
    Unprocessable(String),
    /// 429: too many runs at once.
    Busy,
}

impl SendError {
    pub fn message(&self) -> String {
        match self {
            SendError::Disabled => {
                "Sending is off. An administrator switches it on with send.enabled = true \
                 in harux.toml."
                    .into()
            }
            SendError::Busy => {
                "Too many sends at once (send.max_concurrent_runs); try again shortly".into()
            }
            SendError::BadRequest(m)
            | SendError::Forbidden(m)
            | SendError::NotFound(m)
            | SendError::TooLarge(m)
            | SendError::Unprocessable(m) => m.clone(),
        }
    }
}

/// A destination after the checks: what to connect to and how to label it.
#[derive(Debug, Clone)]
struct Destination {
    /// The user's label (or "This Harux").
    label: String,
    /// What is connected to: an IP address or a name.
    host: String,
    port: u16,
    transport: Transport,
}

impl Destination {
    /// `host:port`, IPv6 in brackets — stored as the sent message's
    /// `source_addr`.
    fn display(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

// --- Sender ---

/// Counters of sending, shown in `/api/stats` and reset by *Clear*.
/// `MllpStats` stays about received frames.
#[derive(Debug, Default)]
struct SendStats {
    sent: AtomicU64,
    rejected: AtomicU64,
    failed: AtomicU64,
}

/// What became of the sends since start or the last *Clear* (#272).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SendCounters {
    /// Accepted (`AA`/`CA`), or written without awaiting a response.
    pub sent: u64,
    /// Answered with any other code (`AE`, `AR`, `CE`, `CR`, …).
    pub rejected: u64,
    /// No usable answer: a transport failure or a response without MSA.
    pub failed: u64,
}

/// Runs sends. Cheap to clone; shared by the web handlers.
#[derive(Clone)]
pub struct Sender {
    inner: Arc<Inner>,
}

struct Inner {
    settings: SendSettings,
    store: MessageStore,
    runs: Arc<Semaphore>,
    stats: SendStats,
    shutdown: watch::Sender<bool>,
}

impl Sender {
    pub fn new(settings: SendSettings, store: MessageStore) -> Self {
        let runs = Arc::new(Semaphore::new(settings.max_concurrent_runs));
        let (shutdown, _) = watch::channel(false);
        Self {
            inner: Arc::new(Inner {
                settings,
                store,
                runs,
                stats: SendStats::default(),
                shutdown,
            }),
        }
    }

    pub fn enabled(&self) -> bool {
        self.inner.settings.enabled
    }

    /// The counters since start or the last *Clear*.
    pub fn counters(&self) -> SendCounters {
        let s = &self.inner.stats;
        SendCounters {
            sent: s.sent.load(Ordering::Relaxed),
            rejected: s.rejected.load(Ordering::Relaxed),
            failed: s.failed.load(Ordering::Relaxed),
        }
    }

    pub fn reset_counters(&self) {
        let s = &self.inner.stats;
        for c in [&s.sent, &s.rejected, &s.failed] {
            c.store(0, Ordering::Relaxed);
        }
    }

    /// Runs under way.
    pub fn active_runs(&self) -> usize {
        self.inner.settings.max_concurrent_runs - self.inner.runs.available_permits()
    }

    /// Stop every run: each records `shutdown` as its outcome.
    pub fn shutdown(&self) {
        let _ = self.inner.shutdown.send(true);
    }

    /// Wait until no run is under way, at most `limit`. `false` on timeout.
    pub async fn wait_for_runs(&self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        while self.active_runs() > 0 {
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        true
    }

    /// Run `request` for the client at `requested_by` and wait for its
    /// outcome. A failed delivery (refused, timeout, NACK) is `Ok`: the
    /// request worked, the send did not.
    pub async fn send(
        &self,
        request: SendRequest,
        requested_by: IpAddr,
    ) -> Result<SendResult, SendError> {
        let settings = &self.inner.settings;
        if !settings.enabled {
            return Err(SendError::Disabled);
        }
        let item = match request.items.as_slice() {
            [item] => item,
            [] => return Err(SendError::BadRequest("`items` is empty".into())),
            _ => {
                return Err(SendError::BadRequest(
                    "One message per request: sending several in one run is not \
                     available yet"
                        .into(),
                ))
            }
        };
        let destination = self.check_target(&request.target)?;

        let original = self
            .inner
            .store
            .get_by_id(&item.message_id)
            .await
            .ok_or_else(|| SendError::NotFound(format!("No message {}", item.message_id)))?
            .message;
        if original.truncated {
            return Err(SendError::Unprocessable(
                "This entry holds only the beginning of a frame above the size limit and \
                 cannot be resent"
                    .into(),
            ));
        }
        if original.parse_error.is_some() && !request.allow_invalid {
            return Err(SendError::Unprocessable(
                "This message did not parse. Set allow_invalid to send it as it was received"
                    .into(),
            ));
        }
        let size = original.wire_len();
        if size > settings.max_message_bytes {
            return Err(SendError::TooLarge(format!(
                "The message ({} KB) is larger than mllp.max_message_size_mb",
                size / 1024
            )));
        }
        // Every sent message is stored again (rule 1): one run may not take
        // more than its share of the store.
        if original.estimated_bytes() > settings.max_run_bytes {
            return Err(SendError::TooLarge(format!(
                "Storing this message again would take more than \
                 send.max_run_store_percent ({} %) of the store",
                settings.max_run_store_percent
            )));
        }

        // Unchanged: the stored message itself — its bytes are the same
        // (`wire` is copied), so the parse is too. Changed: the overrides
        // written into its text, encoded like the original, then decoded
        // and parsed like a received frame (#275).
        let (mut msg, changes) = if request.set.is_empty() {
            ((*original).clone(), Vec::new())
        } else {
            self.changed_message(
                &original,
                &request.set,
                &destination.display(),
                request.allow_invalid,
            )?
        };

        let permit = Arc::clone(&self.inner.runs)
            .try_acquire_owned()
            .map_err(|_| SendError::Busy)?;

        msg.id = uuid::Uuid::new_v4().to_string();
        msg.received_at = chrono::Utc::now();
        msg.source_addr = destination.display();
        msg.ack_response = None;
        msg.ack_code = None;
        msg.outbound = Some(Box::new(Outbound {
            target: destination.label.clone(),
            transport: destination.transport,
            origin: Origin::Resend {
                message_id: original.id.clone(),
                changes,
            },
            requested_by: requested_by.to_canonical().to_string(),
            run: uuid::Uuid::new_v4().to_string(),
        }));
        drop(original);
        // A receiver does not answer an ACK, just as Harux never does.
        let await_response = request.await_response && !msg.message_type.starts_with("ACK");

        let sender = self.clone();
        let run = tokio::spawn(async move {
            let result = sender.run_one(msg, destination, await_response).await;
            drop(permit);
            result
        });
        run.await.map_err(|e| {
            // `run_one` catches the exchange's panics itself; this is the
            // store calls around it.
            SendError::Unprocessable(format!("Internal error while sending: {e}"))
        })
    }

    /// `original` with the fields of `set` overridden (#275), decoded and
    /// parsed like a received frame. See [`changed_payload`].
    fn changed_message(
        &self,
        original: &Hl7Message,
        set: &std::collections::BTreeMap<String, String>,
        peer: &str,
        allow_invalid: bool,
    ) -> Result<(Hl7Message, Vec<String>), SendError> {
        let (bytes, changes) =
            changed_payload(original, set, 1, self.inner.settings.max_message_bytes)?;
        let msg = crate::mllp::ingest(&bytes, peer);
        if let Some(e) = &msg.parse_error {
            if !allow_invalid {
                return Err(SendError::Unprocessable(format!(
                    "After the changes the message does not parse: {e}"
                )));
            }
        }
        Ok((msg, changes))
    }

    /// Check the request's target against the configuration.
    fn check_target(&self, target: &TargetRequest) -> Result<Destination, SendError> {
        let settings = &self.inner.settings;
        match target {
            TargetRequest::Named(name) if name == "loopback" => match settings.loopback {
                // The listener speaks plain MLLP (TLS for it is Milestone 6).
                Some(addr) => Ok(Destination {
                    label: LOOPBACK_NAME.into(),
                    host: addr.ip().to_string(),
                    port: addr.port(),
                    transport: Transport::Plain,
                }),
                None => Err(SendError::Forbidden(
                    "The loopback target is off (send.loopback_target)".into(),
                )),
            },
            TargetRequest::Named(other) => Err(SendError::BadRequest(format!(
                "Unknown target {other:?}: name \"loopback\" or give host and port"
            ))),
            TargetRequest::Explicit(t) => {
                let transport = match (t.tls, t.verify) {
                    (false, _) => Transport::Plain,
                    (true, true) => Transport::Tls,
                    (true, false) => {
                        if !settings.tls.as_ref().is_some_and(|c| c.allow_unverified()) {
                            return Err(SendError::Forbidden(
                                "An unverified TLS connection (verify: false) needs \
                                 send.tls.allow_unverified = true"
                                    .into(),
                            ));
                        }
                        Transport::TlsUnverified
                    }
                };
                let host = normalize_host(&t.host).ok_or_else(|| {
                    SendError::BadRequest(format!("{:?} is not a host name or IP address", t.host))
                })?;
                if t.port == 0 {
                    return Err(SendError::BadRequest("port must be 1-65535".into()));
                }
                let is_loopback = settings.loopback.is_some_and(|a| {
                    a.port() == t.port && a.ip().to_canonical().to_string() == host
                });
                let allowed = settings.rules.is_empty()
                    || is_loopback
                    || settings.rules.iter().any(|r| r.allows(&host, t.port));
                let destination = Destination {
                    label: String::new(),
                    host,
                    port: t.port,
                    transport,
                };
                if !allowed {
                    return Err(SendError::Forbidden(format!(
                        "{} is not in send.allowed_destinations",
                        destination.display()
                    )));
                }
                // A label only: no control characters (it reaches the log).
                let name: String = t
                    .name
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(MAX_TARGET_NAME_CHARS)
                    .collect();
                Ok(Destination {
                    label: if name.is_empty() {
                        destination.display()
                    } else {
                        name
                    },
                    ..destination
                })
            }
        }
    }

    /// Store `msg` as pending, do the exchange, record the outcome. Every
    /// path ends with `set_delivery` (rule 3).
    async fn run_one(
        &self,
        msg: Hl7Message,
        destination: Destination,
        await_response: bool,
    ) -> SendResult {
        let inner = &self.inner;
        let requested_by = msg
            .outbound
            .as_ref()
            .map(|o| o.requested_by.clone())
            .unwrap_or_default();
        let sent = inner.store.insert_sent(msg).await;
        let id = sent.id.clone();

        let mut shutdown = inner.shutdown.subscribe();
        let timeouts = inner.settings.timeouts;
        let tls = inner.settings.tls.clone();
        let started = Instant::now();
        // The exchange runs in a task of its own: a panic there ends that
        // task only and is recorded as a failure (as `ingest` does, #235).
        let mut exchange = tokio::spawn({
            let sent = Arc::clone(&sent);
            let destination = destination.clone();
            async move {
                exchange_one(
                    &sent,
                    &destination,
                    tls.as_deref(),
                    await_response,
                    timeouts,
                )
                .await
            }
        });
        let delivery = tokio::select! {
            joined = &mut exchange => joined.unwrap_or_else(|e| Delivery::failed(
                "internal error",
                format!("Internal error while sending: {e}"),
                Some(elapsed_ms(started)),
            )),
            _ = shutdown_signal(&mut shutdown) => {
                exchange.abort();
                Delivery::failed(
                    "shutdown",
                    "Harux shut down before the exchange ended",
                    Some(elapsed_ms(started)),
                )
            }
        };

        let failed = delivery.state == DeliveryState::Failed;
        // A NACK is an answer, not a delivery: counted on its own (#284 review).
        let accepted = delivery.state == DeliveryState::Sent
            || matches!(delivery.ack_code.as_deref(), Some("AA" | "CA"));
        let counter = if failed {
            &inner.stats.failed
        } else if accepted {
            &inner.stats.sent
        } else {
            &inner.stats.rejected
        };
        counter.fetch_add(1, Ordering::Relaxed);
        // Metadata only — never content.
        let outcome = describe(&delivery);
        if failed {
            warn!(
                "Send of {} {} ({} bytes) to {} ({}) for {} failed: {}",
                sent.message_type,
                sent.message_control_id,
                sent.wire_len(),
                destination.label,
                destination.display(),
                requested_by,
                outcome
            );
        } else {
            info!(
                "Sent {} {} ({} bytes) to {} ({}) for {}: {}",
                sent.message_type,
                sent.message_control_id,
                sent.wire_len(),
                destination.label,
                destination.display(),
                requested_by,
                outcome
            );
        }
        if !inner.store.set_delivery(&id, delivery.clone()).await {
            info!("Sent message {id} left the store before its outcome arrived ({outcome})");
        }
        SendResult { id, delivery }
    }
}

/// Resolves when shutdown is signalled (at once if it already was).
async fn shutdown_signal(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow_and_update() {
            return;
        }
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

fn elapsed_ms(since: Instant) -> u64 {
    since.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

/// The outcome for the log line.
fn describe(d: &Delivery) -> String {
    let ms = d
        .latency_ms
        .map_or(String::new(), |ms| format!(" in {ms} ms"));
    match d.state {
        DeliveryState::Acked => format!("{}{ms}", d.ack_code.as_deref().unwrap_or("?")),
        DeliveryState::Sent => format!("written, no response awaited{ms}"),
        DeliveryState::Pending => "pending".into(),
        DeliveryState::Failed => format!(
            "{}{ms}",
            d.error_detail
                .as_deref()
                .or(d.error.as_deref())
                .unwrap_or("failed")
        ),
    }
}

/// Connect (with TLS if the target uses it), write the stored bytes, read
/// the response.
async fn exchange_one(
    sent: &Hl7Message,
    destination: &Destination,
    tls: Option<&TlsContext>,
    await_response: bool,
    timeouts: Timeouts,
) -> Delivery {
    let started = Instant::now();
    let failed = |e: mllp_client::ExchangeError| {
        Delivery::failed(e.reason(), e.detail(), Some(elapsed_ms(started)))
    };
    // Unchanged means byte for byte (#230): what was received.
    let payload = sent.wire_bytes();
    let (host, port) = (destination.host.as_str(), destination.port);
    let result = match destination.transport {
        Transport::Plain => match mllp_client::connect(host, port, timeouts.connect).await {
            Ok(mut stream) => {
                mllp_client::exchange(&mut stream, &payload, await_response, timeouts).await
            }
            Err(e) => return failed(e),
        },
        Transport::Tls | Transport::TlsUnverified => {
            let Some(tls) = tls else {
                return Delivery::failed(
                    "TLS failed",
                    "TLS is not set up on this server (send.enabled)",
                    None,
                );
            };
            let verify = destination.transport == Transport::Tls;
            match mllp_client::connect_tls(host, port, timeouts.connect, tls, verify).await {
                Ok((mut stream, auth)) => {
                    mllp_client::exchange(&mut stream, &payload, await_response, timeouts)
                        .await
                        .map_err(|e| mllp_client::after_handshake(e, &auth))
                }
                Err(e) => return failed(e),
            }
        }
    };
    match result {
        Ok(None) => Delivery {
            state: DeliveryState::Sent,
            latency_ms: Some(elapsed_ms(started)),
            ..Delivery::pending()
        },
        Ok(Some(response)) => {
            read_response(&response, &sent.message_control_id, elapsed_ms(started))
        }
        Err(e) => failed(e),
    }
}

/// The bytes of `original` with the fields of `set` overridden (#275):
/// placeholders filled in (`n` is the message's position in its run), each
/// value written into its field (`hl7::compose`), the list of segments
/// unchanged, the text encoded as the original arrived
/// (`charset::encode_for_send`), no MLLP frame byte in the result, at most
/// `max_bytes`. Returns the payload and the fields that were set. Public for
/// `tests/fuzz_ingest.rs`.
pub fn changed_payload(
    original: &Hl7Message,
    set: &std::collections::BTreeMap<String, String>,
    n: usize,
    max_bytes: usize,
) -> Result<(Vec<u8>, Vec<String>), SendError> {
    use crate::hl7::compose::{self, ComposeError, FieldPath, PlaceholderContext};
    if original.parse_error.is_some() {
        return Err(SendError::Unprocessable(
            "A message that did not parse can only be resent unchanged".into(),
        ));
    }
    let ctx = PlaceholderContext {
        n,
        now: chrono::Local::now(),
    };
    let d = original.delimiters;
    let mut text = original.raw.clone();
    let mut changes = Vec::with_capacity(set.len());
    for (path, value) in set {
        let path = FieldPath::parse(path).map_err(|e| SendError::BadRequest(e.to_string()))?;
        let value = compose::fill_placeholders(value, &ctx);
        text = compose::set_field(&text, d, &path, &value).map_err(|e| match e {
            ComposeError::BadPath(m) => SendError::BadRequest(m),
            ComposeError::MissingSegment(m) | ComposeError::BadValue(m) => {
                SendError::Unprocessable(m)
            }
        })?;
        changes.push(path.to_string());
    }
    if compose::segment_names(&text, d) != compose::segment_names(&original.raw, d) {
        return Err(SendError::Unprocessable(
            "The changes would alter the message's list of segments".into(),
        ));
    }
    let bytes = crate::hl7::charset::encode_for_send(
        &text,
        &original.wire,
        original.raw.is_ascii(),
        original.charset.as_deref(),
    )
    .map_err(SendError::Unprocessable)?;
    // One frame on the wire is one stored message (rule 1): a payload Harux
    // builds never carries a frame byte.
    if let Some(b) = bytes.iter().find(|&&b| b == 0x0B || b == 0x1C) {
        return Err(SendError::Unprocessable(format!(
            "The message contains the MLLP {} byte (0x{b:02X}); a changed message cannot carry it",
            if *b == 0x0B { "start" } else { "end" }
        )));
    }
    if bytes.len() > max_bytes {
        return Err(SendError::TooLarge(format!(
            "The changed message ({} KB) is larger than mllp.max_message_size_mb",
            bytes.len() / 1024
        )));
    }
    Ok((bytes, changes))
}

/// What a response says: decoded per MSH-18 and parsed like a received
/// frame (under `catch_unwind`, `mllp::ingest`); MSA-1 is the ACK code,
/// MSA-2 is compared with the control ID that was sent. A response that is
/// not an HL7 acknowledgement is kept as text and marked as such. What is
/// stored of it is cut at [`MAX_STORED_RESPONSE_BYTES`].
pub fn read_response(payload: &[u8], sent_control_id: &str, latency_ms: u64) -> Delivery {
    let msg = crate::mllp::ingest(payload, "response");
    let mut text = msg.raw.clone();
    let truncated = text.len() > MAX_STORED_RESPONSE_BYTES;
    if truncated {
        let mut end = MAX_STORED_RESPONSE_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.shrink_to_fit();
    }
    let mut delivery = Delivery {
        latency_ms: Some(latency_ms),
        response: Some(text),
        response_size: Some(payload.len()),
        response_truncated: truncated,
        ..Delivery::pending()
    };

    let msa = msg.segments.iter().find(|s| s.name == "MSA");
    let leaf = |index: usize| {
        msa.and_then(|s| s.fields.iter().find(|f| f.index == index))
            .and_then(|f| f.leaf(msg.delimiters, 0, 0, 0))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let problem = match (&msg.parse_error, msa, leaf(1)) {
        (Some(e), _, _) => Some(format!("The response is not HL7: {e}")),
        (None, None, _) => Some("The response has no MSA segment".to_string()),
        (None, Some(_), None) => Some("The response's MSA-1 (acknowledgment code) is empty".into()),
        (None, Some(_), Some(_)) => None,
    };
    match problem {
        Some(p) => {
            delivery.state = DeliveryState::Failed;
            delivery.error = Some("invalid response".into());
            delivery.error_detail = Some(p.clone());
            delivery.response_error = Some(p);
        }
        None => {
            delivery.state = DeliveryState::Acked;
            delivery.ack_code = leaf(1).map(|c| c.to_ascii_uppercase());
            delivery.control_id_matches =
                Some(leaf(2).as_deref().unwrap_or_default() == sent_control_id);
        }
    }
    delivery
}

/// `GET /api/send/config` (#272): what the UI needs to offer sending.
#[derive(Debug, Serialize)]
pub struct SendConfigResponse {
    pub enabled: bool,
    /// Harux's own listener, if offered as a target.
    pub loopback: Option<LoopbackTarget>,
    /// `send.allowed_destinations`, normalized; empty = any destination.
    pub allowed_destinations: Vec<String>,
    pub connect_timeout_secs: u64,
    pub response_timeout_secs: u64,
    pub max_run_messages: usize,
    pub max_run_store_percent: usize,
    pub max_concurrent_runs: usize,
    pub max_message_bytes: usize,
    /// What outgoing TLS offers (#273): never paths or keys.
    pub tls: TlsInfo,
}

#[derive(Debug, Serialize)]
pub struct TlsInfo {
    /// A user may switch the certificate check off for a target.
    pub allow_unverified: bool,
    /// Harux shows a certificate of its own to targets that ask.
    pub client_certificate: bool,
}

#[derive(Debug, Serialize)]
pub struct LoopbackTarget {
    pub name: &'static str,
    pub host: String,
    pub port: u16,
}

impl Sender {
    pub fn config_response(&self) -> SendConfigResponse {
        let s = &self.inner.settings;
        SendConfigResponse {
            enabled: s.enabled,
            loopback: s.loopback.map(|a| LoopbackTarget {
                name: LOOPBACK_NAME,
                host: a.ip().to_string(),
                port: a.port(),
            }),
            allowed_destinations: s.rules.iter().map(ToString::to_string).collect(),
            connect_timeout_secs: s.timeouts.connect.as_secs(),
            response_timeout_secs: s.timeouts.response.as_secs(),
            max_run_messages: s.max_run_messages,
            max_run_store_percent: s.max_run_store_percent,
            max_concurrent_runs: s.max_concurrent_runs,
            max_message_bytes: s.max_message_bytes,
            tls: TlsInfo {
                allow_unverified: s.tls.as_ref().is_some_and(|t| t.allow_unverified()),
                client_certificate: s.tls.as_ref().is_some_and(|t| t.has_client_certificate()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #285 review: a hand-written request with `"tls": true` and no
    /// `verify` checks the certificate.
    #[test]
    fn verify_defaults_to_on() {
        let req: SendRequest = serde_json::from_str(
            r#"{"target":{"host":"h","port":1,"tls":true},"items":[{"message_id":"m"}]}"#,
        )
        .unwrap();
        let TargetRequest::Explicit(t) = req.target else {
            panic!("explicit target expected")
        };
        assert!(t.tls && t.verify);
        assert!(req.await_response && !req.allow_invalid);
    }

    #[test]
    fn response_is_read_from_msa() {
        let d = read_response(
            b"MSH|^~\\&|R|RF|Harux|H|20260101||ACK^A01|X|P|2.5\rMSA|aa|C1\r",
            "C1",
            7,
        );
        assert_eq!(d.state, DeliveryState::Acked);
        assert_eq!(d.ack_code.as_deref(), Some("AA"));
        assert_eq!(d.control_id_matches, Some(true));
        assert_eq!(d.latency_ms, Some(7));
        assert!(d.response.unwrap().starts_with("MSH|"));

        let d = read_response(
            b"MSH|^~\\&|R|RF|||20260101||ACK|X|P|2.5\rMSA|AE|OTHER|bad\r",
            "C1",
            1,
        );
        assert_eq!(d.ack_code.as_deref(), Some("AE"));
        assert_eq!(d.control_id_matches, Some(false));
    }

    #[test]
    fn a_response_that_is_no_ack_is_kept_and_marked() {
        let d = read_response(b"hello", "C1", 1);
        assert_eq!(d.state, DeliveryState::Failed);
        assert_eq!(d.error.as_deref(), Some("invalid response"));
        assert_eq!(d.response.as_deref(), Some("hello"));
        assert!(d.response_error.unwrap().contains("not HL7"));

        let d = read_response(b"MSH|^~\\&|R|RF|||20260101||ADT^A01|X|P|2.5\r", "C1", 1);
        assert_eq!(d.state, DeliveryState::Failed);
        assert!(d.response_error.unwrap().contains("no MSA"));
    }

    #[test]
    fn a_large_response_is_cut() {
        let mut big = b"MSH|^~\\&|R|RF|||20260101||ACK|X|P|2.5\rMSA|AA|C1|".to_vec();
        // Multi-byte characters across the cut must not split.
        big.extend("ä".repeat(MAX_STORED_RESPONSE_BYTES).as_bytes());
        let d = read_response(&big, "C1", 1);
        assert_eq!(d.ack_code.as_deref(), Some("AA"));
        assert!(d.response_truncated);
        assert_eq!(d.response_size, Some(big.len()));
        assert!(d.response.unwrap().len() <= MAX_STORED_RESPONSE_BYTES);
    }
}
