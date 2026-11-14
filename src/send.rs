//! Sending stored messages to other MLLP receivers (#272, Milestone 4).
//!
//! A **run** is one request to send: stored messages (as received, or with
//! fields set, #275) and drafts (#276), each item `repeat` times, in order,
//! over one connection, to the destination the request names (#280). A run
//! of one message answers with its outcome; a longer one goes on in a task
//! of its own, with its status in a small registry (`GET
//! /api/send/runs/{id}`) and a switch to cancel it. The rules (`AGENTS.md`,
//! *Sending*):
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

/// How many requests with a large body (`/api/send`, `/api/parse`,
/// `/api/build`) may be read and handled at once (#276). A larger one is
/// answered `429`.
pub const LARGE_BODY_PERMITS: usize = 4;

/// From this size on a request body counts as large (or when its size is
/// not announced).
pub const LARGE_BODY_BYTES: u64 = 256 * 1024;

/// Longest pause between the messages of a run (`interval_ms`, #280).
pub const MAX_INTERVAL_MS: u64 = 60_000;

/// Transport failures in a row after which a run gives up (#280).
const MAX_FAILURES_IN_ROW: u32 = 3;

/// Finished runs kept for `GET /api/send/runs/{id}` (#280).
const KEPT_RUNS: usize = 50;

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
    /// `send.templates_dir` (#279), a directory (checked at start).
    pub templates_dir: Option<std::path::PathBuf>,
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
            templates_dir: match (&s.templates_dir, s.enabled) {
                (Some(dir), true) => {
                    let path = std::path::PathBuf::from(dir);
                    if !path.is_dir() {
                        return Err(format!(
                            "send.templates_dir {dir:?} is not a directory (relative paths: the working directory)"
                        ));
                    }
                    Some(path)
                }
                _ => None,
            },
        })
    }
}

// --- Request ---

/// `POST /api/send` (#272). Unknown fields are refused, so a request that
/// asks for something this version does not do fails instead of sending the
/// message unchanged.
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
    /// Send the list of items this many times (#280): `A B C A B C`. The
    /// placeholders are filled in anew for every copy; `{{n}}` is the
    /// position in the run.
    #[serde(default = "one")]
    pub repeat: usize,
    /// Pause between two messages of a run, at most [`MAX_INTERVAL_MS`].
    #[serde(default)]
    pub interval_ms: u64,
    /// Stop the run at the first answer that is not an accept (`AE`, `AR`,
    /// `CE`, `CR`, or a response that is no acknowledgement).
    #[serde(default)]
    pub stop_on_nack: bool,
}

fn yes() -> bool {
    true
}

fn one() -> usize {
    1
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

/// One message to send: a stored one (`message_id`) or a draft (`raw`,
/// #276) — exactly one of the two.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemRequest {
    /// A stored message, sent as received or with `set`.
    #[serde(default)]
    pub message_id: Option<String>,
    /// A draft's text: line ends become `CR`, placeholders and references
    /// are filled in, encoded with `charset` (#276).
    #[serde(default)]
    pub raw: Option<String>,
    /// The draft's encoding: `"auto"` (default: MSH-18, else UTF-8) or a
    /// name (`UTF-8`, `8859/1`, `windows-1252`, …).
    #[serde(default)]
    pub charset: Option<String>,
    /// The stored message the draft was made from; its references
    /// (`{{from:OBX[1]-5}}`) are filled in from it.
    #[serde(default)]
    pub based_on: Option<String>,
    /// The template the draft was made from (#279): recorded on the sent
    /// message as its origin. A label only.
    #[serde(default)]
    pub template: Option<String>,
}

// For callers of the library (the integration tests); the binary builds
// items from JSON only.
#[allow(dead_code)]
impl ItemRequest {
    /// A stored message, by ID.
    pub fn stored(id: impl Into<String>) -> Self {
        Self {
            message_id: Some(id.into()),
            ..Self::default()
        }
    }

    /// A draft's text.
    pub fn draft(raw: impl Into<String>) -> Self {
        Self {
            raw: Some(raw.into()),
            ..Self::default()
        }
    }
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

impl From<crate::hl7::compose::ComposeError> for SendError {
    /// A bad path is a malformed request; a missing segment or a value that
    /// would leave its field is a message that cannot be built.
    fn from(e: crate::hl7::compose::ComposeError) -> Self {
        use crate::hl7::compose::ComposeError;
        match e {
            ComposeError::BadPath(m) => SendError::BadRequest(m),
            ComposeError::MissingSegment(m) | ComposeError::BadValue(m) => {
                SendError::Unprocessable(m)
            }
        }
    }
}

impl SendError {
    /// The HTTP status of the refusal.
    pub fn status(&self) -> u16 {
        match self {
            SendError::Disabled | SendError::Forbidden(_) => 403,
            SendError::BadRequest(_) => 400,
            SendError::NotFound(_) => 404,
            SendError::TooLarge(_) => 413,
            SendError::Unprocessable(_) => 422,
            SendError::Busy => 429,
        }
    }

    /// The refusal of item `n` of several: its reason names the item.
    fn for_item(self, n: Option<usize>) -> Self {
        let Some(n) = n else { return self };
        let named = |m: String| format!("Item {n}: {m}");
        match self {
            SendError::BadRequest(m) => SendError::BadRequest(named(m)),
            SendError::Forbidden(m) => SendError::Forbidden(named(m)),
            SendError::NotFound(m) => SendError::NotFound(named(m)),
            SendError::TooLarge(m) => SendError::TooLarge(named(m)),
            SendError::Unprocessable(m) => SendError::Unprocessable(named(m)),
            e @ (SendError::Disabled | SendError::Busy) => e,
        }
    }

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
    /// Requests with a large body being read or handled (#276).
    large_bodies: Arc<Semaphore>,
    stats: SendStats,
    shutdown: watch::Sender<bool>,
    /// Runs under way and the newest finished ones (#280).
    registry: std::sync::Mutex<Registry>,
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
                large_bodies: Arc::new(Semaphore::new(LARGE_BODY_PERMITS)),
                stats: SendStats::default(),
                shutdown,
                registry: Default::default(),
            }),
        }
    }

    pub fn enabled(&self) -> bool {
        self.inner.settings.enabled
    }

    /// `mllp.max_message_size_mb` in bytes: the largest message that can
    /// be sent, and the base of the body limit of the draft routes.
    pub fn max_message_bytes(&self) -> usize {
        self.inner.settings.max_message_bytes
    }

    /// A permit to read and handle a large request body (#276), or `None`
    /// when [`LARGE_BODY_PERMITS`] such requests are under way: axum reads
    /// a body into memory before the handler runs, so their number is
    /// limited too, not only their size.
    pub fn large_body_permit(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        Arc::clone(&self.inner.large_bodies)
            .try_acquire_owned()
            .ok()
    }

    /// `send.templates_dir`, if set (#279).
    pub fn templates_dir(&self) -> Option<&std::path::Path> {
        self.inner.settings.templates_dir.as_deref()
    }

    /// The stored message `id`, if it is still stored.
    pub async fn stored(&self, id: &str) -> Option<Arc<Hl7Message>> {
        self.inner.store.get_by_id(id).await.map(|s| s.message)
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

    /// Run `request` for the client at `requested_by` (#272, runs #280).
    ///
    /// Everything that can be refused is refused here, before anything is
    /// sent: the target, every item (an unknown message refuses the whole
    /// request), the run limits. One message: the request waits for its
    /// outcome ([`SendOutcome::One`]); a failed delivery (refused, timeout,
    /// NACK) is `Ok` — the request worked, the send did not. Several: the run
    /// goes on in a task of its own and the answer is its status
    /// ([`SendOutcome::Run`]).
    pub async fn start(
        &self,
        request: SendRequest,
        requested_by: IpAddr,
    ) -> Result<SendOutcome, SendError> {
        let settings = &self.inner.settings;
        if !settings.enabled {
            return Err(SendError::Disabled);
        }
        if request.items.is_empty() {
            return Err(SendError::BadRequest("`items` is empty".into()));
        }
        if request.repeat == 0 {
            return Err(SendError::BadRequest("`repeat` is at least 1".into()));
        }
        if request.interval_ms > MAX_INTERVAL_MS {
            return Err(SendError::BadRequest(format!(
                "`interval_ms` is at most {MAX_INTERVAL_MS} (one minute)"
            )));
        }
        let total = request.items.len().saturating_mul(request.repeat);
        if total > settings.max_run_messages {
            return Err(SendError::TooLarge(format!(
                "A run of {total} messages is more than send.max_run_messages ({})",
                settings.max_run_messages
            )));
        }
        let destination = self.check_target(&request.target)?;
        let peer = destination.display();

        // Look every item up (async), then build the first copy of each off
        // the async workers: a run may hold many large messages.
        let several = request.items.len() > 1;
        let mut resolved = Vec::with_capacity(request.items.len());
        for (i, item) in request.items.iter().enumerate() {
            let r = self
                .resolve(item, &request)
                .await
                .map_err(|e| e.for_item(several.then_some(i + 1)))?;
            resolved.push(r);
        }
        let set = Arc::new(request.set);

        // Every sent message is stored again (rule 1): a run may not take
        // more than its share of the store.
        // `at_least`: the lower bound before the build, not the size itself
        // (#309 review).
        let too_large = |run_bytes: usize, at_least: bool| {
            SendError::TooLarge(format!(
                "Storing {} again would take {}{} MB, more than send.max_run_store_percent \
                 ({} %) of the store",
                if total == 1 {
                    "this message".to_string()
                } else {
                    format!("these {total} messages")
                },
                if at_least { "at least " } else { "" },
                run_bytes.div_ceil(1024 * 1024),
                settings.max_run_store_percent
            ))
        };
        // First what the run takes at least, known without building a
        // message: a run that is too large is refused before anything is
        // built (#298).
        let least = resolved
            .iter()
            .map(|r| least_bytes(r, &set))
            .fold(0usize, usize::saturating_add)
            .saturating_mul(request.repeat);
        if least > settings.max_run_bytes {
            return Err(too_large(least, true));
        }
        // Building takes the run's permit: at most `max_concurrent_runs`
        // requests build at once (#298).
        let permit = Arc::clone(&self.inner.runs)
            .try_acquire_owned()
            .map_err(|_| SendError::Busy)?;

        let max_bytes = settings.max_message_bytes;
        let allow_invalid = request.allow_invalid;
        // A run of one message keeps the copy it builds as the check: it is
        // sent right after. A longer run builds every copy when it is sent
        // (`{{now}}` is the time of the send, #305), so it never holds more
        // than one built message at a time (#298).
        let keep_first = total == 1;
        let (items, first) = tokio::task::spawn_blocking({
            let set = Arc::clone(&set);
            let peer = peer.clone();
            move || {
                prepare(
                    resolved,
                    &set,
                    &peer,
                    max_bytes,
                    allow_invalid,
                    several,
                    keep_first,
                )
            }
        })
        .await
        .map_err(|e| SendError::Unprocessable(format!("Internal error while building: {e}")))??;

        // Then exactly, from the sizes of the copies built.
        let run_bytes = items
            .iter()
            .map(|i| i.estimated)
            .fold(0usize, usize::saturating_add)
            .saturating_mul(request.repeat);
        if run_bytes > settings.max_run_bytes {
            return Err(too_large(run_bytes, false));
        }

        let handle = self.register(RunStatus::new(
            &destination.label,
            total,
            requested_by.to_canonical().to_string(),
        ));
        let plan = RunPlan {
            items: Arc::new(items),
            repeat: request.repeat,
            set,
            destination,
            peer,
            requested_by: requested_by.to_canonical().to_string(),
            allow_invalid,
            await_response: request.await_response,
            interval: Duration::from_millis(request.interval_ms),
            stop_on_nack: request.stop_on_nack,
        };
        let sender = self.clone();
        let run = tokio::spawn({
            let handle = Arc::clone(&handle);
            async move {
                let result = sender.run(&handle, plan, first).await;
                drop(permit);
                result
            }
        });
        if total > 1 {
            return Ok(SendOutcome::Run(handle.status()));
        }
        match run.await {
            Ok(Some(result)) => Ok(SendOutcome::One(result)),
            Ok(None) => Err(SendError::Unprocessable(
                handle
                    .status()
                    .reason
                    .unwrap_or_else(|| "The message was not sent".into()),
            )),
            // `run` catches the exchange's panics itself; this is the store
            // calls around it.
            Err(e) => Err(SendError::Unprocessable(format!(
                "Internal error while sending: {e}"
            ))),
        }
    }

    /// Send one message and wait for its outcome: [`Sender::start`] for a
    /// request of exactly one message. For callers of the library (the
    /// integration tests); the web handler calls `start`.
    #[allow(dead_code)]
    pub async fn send(
        &self,
        request: SendRequest,
        requested_by: IpAddr,
    ) -> Result<SendResult, SendError> {
        if request.items.len().saturating_mul(request.repeat) > 1 {
            return Err(SendError::BadRequest(
                "Several messages are a run: Sender::start".into(),
            ));
        }
        match self.start(request, requested_by).await? {
            SendOutcome::One(result) => Ok(result),
            SendOutcome::Run(_) => unreachable!("a request of one message is no run"),
        }
    }

    /// The checks of one item that need the store: the stored message it
    /// names (kept as an `Arc`, so eviction cannot take it away before the
    /// run sends it), or the message a draft refers to.
    async fn resolve(
        &self,
        item: &ItemRequest,
        request: &SendRequest,
    ) -> Result<Resolved, SendError> {
        let settings = &self.inner.settings;
        match (&item.message_id, &item.raw) {
            (Some(id), None) => {
                if item.charset.is_some() || item.based_on.is_some() || item.template.is_some() {
                    return Err(SendError::BadRequest(
                        "charset, based_on and template belong to a draft (raw), not to a stored \
                         message"
                            .into(),
                    ));
                }
                let original = self
                    .stored(id)
                    .await
                    .ok_or_else(|| SendError::NotFound(format!("No message {id}")))?;
                if original.truncated {
                    return Err(SendError::Unprocessable(
                        "This entry holds only the beginning of a frame above the size limit and \
                         cannot be resent"
                            .into(),
                    ));
                }
                if original.parse_error.is_some() && !request.allow_invalid {
                    return Err(SendError::Unprocessable(
                        "This message did not parse. Set allow_invalid to send it as it was \
                         received"
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
                Ok(Resolved::Stored(original))
            }
            (None, Some(raw)) => {
                // `set` is written into the draft's text once; its
                // placeholders are filled in for every copy (`{{n}}`). Off
                // the async workers: the text can be megabytes (#301).
                let (raw, set) = (raw.clone(), request.set.clone());
                let (text, _) =
                    tokio::task::spawn_blocking(move || crate::draft::apply_set(&raw, &set))
                        .await
                        .map_err(|e| {
                            SendError::Unprocessable(format!(
                                "Internal error while writing the changes: {e}"
                            ))
                        })??;
                let based_on = self.based_on(item.based_on.as_deref(), &text).await?;
                // A label only: no control characters, cut (it reaches the log).
                let template = item.template.as_deref().map(|t| {
                    t.trim()
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(MAX_TARGET_NAME_CHARS)
                        .collect::<String>()
                });
                let origin = match (template, &item.based_on) {
                    (Some(name), None) if !name.is_empty() => Origin::Template { name },
                    _ => Origin::Draft {
                        based_on: item.based_on.clone(),
                    },
                };
                Ok(Resolved::Draft {
                    text,
                    charset: item.charset.clone(),
                    based_on,
                    origin,
                })
            }
            _ => Err(SendError::BadRequest(
                "An item names a stored message (message_id) or carries a draft (raw), \
                 one of the two"
                    .into(),
            )),
        }
    }

    /// The message a draft was made from, when the draft needs it: `None`
    /// without references. A draft whose references point into a message
    /// that is no longer stored is refused (a bookmark keeps it).
    pub async fn based_on(
        &self,
        id: Option<&str>,
        text: &str,
    ) -> Result<Option<Arc<Hl7Message>>, SendError> {
        if !crate::draft::has_references(text) {
            return Ok(None);
        }
        match id {
            None => Ok(None), // `fill_references` says what is missing
            Some(id) => self.stored(id).await.map(Some).ok_or_else(|| {
                SendError::Unprocessable(format!(
                    "The draft refers to values of the message it was made from ({id}), \
                     which is no longer stored; a bookmark keeps a message"
                ))
            }),
        }
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

    // --- Runs (#280) ---

    /// Put a new run into the registry, under way.
    fn register(&self, status: RunStatus) -> Arc<RunHandle> {
        let (cancel, _) = watch::channel(false);
        let handle = Arc::new(RunHandle {
            id: status.id.clone(),
            status: std::sync::Mutex::new(status),
            cancel,
        });
        lock(&self.inner.registry)
            .running
            .insert(handle.id.clone(), Arc::clone(&handle));
        handle
    }

    /// Move run `id` to the finished ones, of which the newest
    /// [`KEPT_RUNS`] are kept (#293 review: O(1), no status taken).
    fn retire(&self, id: &str) {
        let mut registry = lock(&self.inner.registry);
        if let Some(run) = registry.running.remove(id) {
            registry.finished.push_back(run);
            if registry.finished.len() > KEPT_RUNS {
                registry.finished.pop_front();
            }
        }
    }

    fn find_run(&self, id: &str) -> Option<Arc<RunHandle>> {
        let registry = lock(&self.inner.registry);
        registry
            .running
            .get(id)
            .or_else(|| registry.finished.iter().find(|r| r.id == id))
            .cloned()
    }

    /// The status of run `id`, while it is kept.
    pub fn run_status(&self, id: &str) -> Option<RunStatus> {
        self.find_run(id).map(|r| r.status())
    }

    /// Ask run `id` to stop. The message in flight ends as it would (its
    /// response is awaited); the run stops before the next one. Messages it
    /// never tried are not stored. A finished run stays as it was.
    pub fn cancel_run(&self, id: &str) -> Option<RunStatus> {
        let run = self.find_run(id)?;
        let _ = run.cancel.send(true);
        Some(run.status())
    }

    /// Send the messages of `plan` in order over one connection. Returns the
    /// outcome of the last message stored (for a run of one: its outcome).
    async fn run(
        &self,
        handle: &RunHandle,
        plan: RunPlan,
        mut first: Option<Hl7Message>,
    ) -> Option<SendResult> {
        let inner = &self.inner;
        let run_id = handle.status().id;
        let mut shutdown = inner.shutdown.subscribe();
        let mut cancel = handle.cancel.subscribe();
        let len = plan.items.len();
        let mut connection: Option<mllp_client::Connection> = None;
        let mut failures_in_row = 0u32;
        let mut last = None;

        let end = 'run: {
            for n in 1..=len * plan.repeat {
                let i = (n - 1) % len;
                if n > 1 && !plan.interval.is_zero() {
                    tokio::select! {
                        () = tokio::time::sleep(plan.interval) => {}
                        () = shutdown_signal(&mut cancel) => break 'run (RunState::Cancelled, None),
                        () = shutdown_signal(&mut shutdown) => break 'run (RunState::Shutdown, None),
                    }
                }
                if *cancel.borrow() {
                    break 'run (RunState::Cancelled, None);
                }
                if *shutdown.borrow() {
                    break 'run (RunState::Shutdown, None);
                }

                // Every copy is built when it is sent, its placeholders
                // filled in then (#305); only a run of one message brings
                // the copy it was checked with. Either way `{{now}}` is taken
                // before the copy is stored and connected (rule 1), so a slow
                // connect delays every copy alike — don't "fix" the single
                // message by building it again here.
                let built = match if n == 1 { first.take() } else { None } {
                    Some(msg) => Ok(msg),
                    None => {
                        let items = Arc::clone(&plan.items);
                        let set = Arc::clone(&plan.set);
                        let peer = plan.peer.clone();
                        let (max, allow_invalid) =
                            (inner.settings.max_message_bytes, plan.allow_invalid);
                        tokio::task::spawn_blocking(move || {
                            items[i].build(&set, n, &peer, max, allow_invalid)
                        })
                        .await
                        .unwrap_or_else(|e| {
                            Err(SendError::Unprocessable(format!(
                                "Internal error while building: {e}"
                            )))
                        })
                    }
                };
                let mut msg = match built {
                    Ok(msg) => msg,
                    Err(e) => {
                        // Never tried, so not stored: counted and named.
                        warn!(
                            "Run {run_id}: message {n} could not be built: {}",
                            crate::mllp::loggable(&e.message())
                        );
                        handle.update(|s| {
                            s.done += 1;
                            s.failed += 1;
                            s.reason = Some(format!("Message {n}: {}", e.message()));
                        });
                        continue;
                    }
                };
                msg.id = uuid::Uuid::new_v4().to_string();
                msg.received_at = chrono::Utc::now();
                msg.source_addr = plan.peer.clone();
                msg.ack_response = None;
                msg.ack_code = None;
                msg.outbound = Some(Box::new(Outbound {
                    target: plan.destination.label.clone(),
                    transport: plan.destination.transport,
                    origin: plan.items[i].origin.clone(),
                    requested_by: plan.requested_by.clone(),
                    run: run_id.clone(),
                }));
                // A receiver does not answer an ACK, just as Harux never does.
                let await_response = plan.await_response && !msg.message_type.starts_with("ACK");
                let sent = inner.store.insert_sent(msg).await;

                // A target that closed the connection after its last answer
                // gets a new one before the message is written.
                if let Some(c) = &connection {
                    if !c.is_reusable().await {
                        connection = None;
                    }
                }
                let started = Instant::now();
                // The exchange runs in a task of its own: a panic there ends
                // that task only and is recorded as a failure (as `ingest`
                // does, #235).
                let mut exchange = tokio::spawn(exchange_on(
                    connection.take(),
                    Arc::clone(&sent),
                    plan.destination.clone(),
                    inner.settings.tls.clone(),
                    await_response,
                    inner.settings.timeouts,
                ));
                let (kept, delivery, transport_failed, stop) = tokio::select! {
                    joined = &mut exchange => match joined {
                        Ok((kept, delivery, transport_failed)) => (kept, delivery, transport_failed, None),
                        Err(e) => (None, Delivery::failed(
                            "internal error",
                            format!("Internal error while sending: {e}"),
                            Some(elapsed_ms(started)),
                        ), true, None),
                    },
                    () = shutdown_signal(&mut shutdown) => {
                        exchange.abort();
                        (None, Delivery::failed(
                            "shutdown",
                            "Harux shut down before the exchange ended",
                            Some(elapsed_ms(started)),
                        ), false, Some(RunState::Shutdown))
                    }
                };
                connection = kept;
                self.record(&sent, &plan, &delivery).await;
                handle.update(|s| s.count(&delivery));
                last = Some(SendResult {
                    id: sent.id.clone(),
                    delivery: delivery.clone(),
                });
                if let Some(state) = stop {
                    break 'run (state, None);
                }
                if transport_failed {
                    failures_in_row += 1;
                    if failures_in_row >= MAX_FAILURES_IN_ROW {
                        break 'run (
                            RunState::GaveUp,
                            Some(format!(
                                "Gave up after {MAX_FAILURES_IN_ROW} transport failures in a \
                                 row; the last: {}",
                                describe(&delivery)
                            )),
                        );
                    }
                } else {
                    failures_in_row = 0;
                }
                if plan.stop_on_nack && !accepted(&delivery) && !transport_failed {
                    break 'run (
                        RunState::StoppedOnNack,
                        Some(format!(
                            "Stopped at message {n}: the target answered {}",
                            describe(&delivery)
                        )),
                    );
                }
            }
            (RunState::Done, None)
        };
        drop(connection);

        let (state, reason) = end;
        handle.update(|s| {
            s.state = state;
            if reason.is_some() {
                s.reason = reason;
            }
            s.finished_at = Some(chrono::Utc::now());
        });
        self.retire(&run_id);
        let s = handle.status();
        if s.total > 1 {
            info!(
                "Run {} to {} for {} ended ({:?}): {} of {} messages, {} failed",
                s.id, s.target, s.requested_by, s.state, s.done, s.total, s.failed
            );
        }
        last
    }

    /// Count, log and store the outcome of one send (rule 3).
    async fn record(&self, sent: &Hl7Message, plan: &RunPlan, delivery: &Delivery) {
        let inner = &self.inner;
        let failed = delivery.state == DeliveryState::Failed;
        // A NACK is an answer, not a delivery: counted on its own (#284 review).
        let counter = if failed {
            &inner.stats.failed
        } else if accepted(delivery) {
            &inner.stats.sent
        } else {
            &inner.stats.rejected
        };
        counter.fetch_add(1, Ordering::Relaxed);
        // Metadata only — never content (#307).
        let outcome = describe_for_log(delivery);
        let d = &plan.destination;
        if failed {
            warn!(
                "Send of {} {} ({} bytes) to {} ({}) for {} failed: {}",
                sent.message_type,
                sent.message_control_id,
                sent.wire_len(),
                d.label,
                d.display(),
                plan.requested_by,
                outcome
            );
        } else {
            info!(
                "Sent {} {} ({} bytes) to {} ({}) for {}: {}",
                sent.message_type,
                sent.message_control_id,
                sent.wire_len(),
                d.label,
                d.display(),
                plan.requested_by,
                outcome
            );
        }
        if !inner.store.set_delivery(&sent.id, delivery.clone()).await {
            info!(
                "Sent message {} left the store before its outcome arrived ({outcome})",
                sent.id
            );
        }
    }
}

/// What `POST /api/send` started.
#[derive(Debug)]
pub enum SendOutcome {
    /// One message: its stored ID and outcome (`200`).
    One(SendResult),
    /// Several: the run's status at the start (`202`); progress from
    /// `GET /api/send/runs/{id}`, the rows over the WebSocket.
    Run(RunStatus),
}

/// An item after the store lookups.
enum Resolved {
    Stored(Arc<Hl7Message>),
    Draft {
        text: String,
        charset: Option<String>,
        based_on: Option<Arc<Hl7Message>>,
        origin: Origin,
    },
}

/// One item of a run, ready to be built for any position.
struct Item {
    source: Source,
    origin: Origin,
    /// What one copy takes in the store (its first copy, built).
    estimated: usize,
}

enum Source {
    /// Sent byte for byte as received (the stored payload, cloned).
    Unchanged(Arc<Hl7Message>),
    /// With the request's `set` (#275).
    Changed(Arc<Hl7Message>),
    /// A draft's text, `set` already written in (#276).
    Draft {
        text: String,
        charset: Option<String>,
        based_on: Option<Arc<Hl7Message>>,
    },
}

impl Item {
    /// The message at position `n` of the run (`{{n}}`), decoded and parsed
    /// like a received frame.
    fn build(
        &self,
        set: &std::collections::BTreeMap<String, String>,
        n: usize,
        peer: &str,
        max_bytes: usize,
        allow_invalid: bool,
    ) -> Result<Hl7Message, SendError> {
        let (msg, what) = match &self.source {
            // Unchanged: the stored message itself — its bytes are the same
            // (`wire` is copied), so the parse is too.
            Source::Unchanged(original) => return Ok((**original).clone()),
            Source::Changed(original) => {
                let (bytes, _) = changed_payload(original, set, n, max_bytes)?;
                (
                    crate::mllp::ingest(&bytes, peer),
                    "After the changes the message",
                )
            }
            Source::Draft {
                text,
                charset,
                based_on,
            } => {
                let bytes = crate::draft::build_payload(
                    text,
                    charset.as_deref(),
                    based_on.as_deref(),
                    n,
                    max_bytes,
                )?;
                (crate::mllp::ingest(&bytes, peer), "The draft")
            }
        };
        if let Some(e) = &msg.parse_error {
            if !allow_invalid {
                return Err(SendError::Unprocessable(format!(
                    "{what} does not parse: {e}. Set allow_invalid to send it anyway"
                )));
            }
        }
        Ok(msg)
    }
}

/// What one copy of `r` takes in the store **at least**, known without
/// building it (#298) — so a run that is too large is refused before a
/// message is built. A lower bound, never more than [`prepare`] measures:
///
/// - unchanged: the stored message's own size (exact);
/// - with `set`: the text that stays — the stored text without the fields
///   `set` replaces (a new value can be empty, a missing field only adds);
/// - a draft, or a stored message that did not parse: nothing (its
///   references and placeholders can shrink or grow it; the draft's text
///   is in the request already).
fn least_bytes(r: &Resolved, set: &std::collections::BTreeMap<String, String>) -> usize {
    let original = match r {
        Resolved::Stored(original) if set.is_empty() => return original.estimated_bytes(),
        Resolved::Stored(original) if original.parse_error.is_none() => original,
        _ => return 0,
    };
    let mut replaced = 0usize;
    for path in set.keys() {
        let Ok(path) = crate::hl7::compose::FieldPath::parse(path) else {
            return 0; // refused when it is built
        };
        let Some(segment) = original
            .segments
            .iter()
            .filter(|s| s.name == path.segment)
            .nth(path.occurrence - 1)
        else {
            return 0; // refused when it is built
        };
        // `index` is the HL7 field number, MSH included (the parser's shift).
        if let Some(field) = segment.fields.iter().find(|f| f.index == path.field) {
            replaced = replaced.saturating_add(field.value.len());
        }
    }
    original.raw.len().saturating_sub(replaced)
}

/// Build the first copy of every item (its position in the first pass) and
/// measure it: it checks what can be checked before anything is sent. Each
/// copy is dropped once measured, so a run never holds more than one built
/// message here (#298); the run builds every copy again when it sends it,
/// with the time of the send (#305). `keep_first` (a run of one message)
/// keeps the one copy instead — it is sent right after. Unchanged messages
/// are not built: they are the stored ones.
fn prepare(
    resolved: Vec<Resolved>,
    set: &std::collections::BTreeMap<String, String>,
    peer: &str,
    max_bytes: usize,
    allow_invalid: bool,
    several: bool,
    keep_first: bool,
) -> Result<(Vec<Item>, Option<Hl7Message>), SendError> {
    let mut items = Vec::with_capacity(resolved.len());
    let mut first = None;
    for (i, r) in resolved.into_iter().enumerate() {
        let n = i + 1;
        let (source, origin) = match r {
            Resolved::Stored(original) if set.is_empty() => (
                Source::Unchanged(Arc::clone(&original)),
                Origin::Resend {
                    message_id: original.id.clone(),
                    changes: Vec::new(),
                },
            ),
            Resolved::Stored(original) => {
                // As `changed_payload` names them (`msh-10` → `MSH-10`).
                let changes = set
                    .keys()
                    .map(|p| {
                        crate::hl7::compose::FieldPath::parse(p)
                            .map_or_else(|_| p.clone(), |p| p.to_string())
                    })
                    .collect();
                (
                    Source::Changed(Arc::clone(&original)),
                    Origin::Resend {
                        message_id: original.id.clone(),
                        changes,
                    },
                )
            }
            Resolved::Draft {
                text,
                charset,
                based_on,
                origin,
            } => (
                Source::Draft {
                    text,
                    charset,
                    based_on,
                },
                origin,
            ),
        };
        let mut item = Item {
            source,
            origin,
            estimated: 0,
        };
        match &item.source {
            Source::Unchanged(original) => item.estimated = original.estimated_bytes(),
            _ => {
                let msg = item
                    .build(set, n, peer, max_bytes, allow_invalid)
                    .map_err(|e| e.for_item(several.then_some(n)))?;
                item.estimated = msg.estimated_bytes();
                if keep_first && i == 0 {
                    first = Some(msg);
                }
            }
        }
        items.push(item);
    }
    Ok((items, first))
}

/// Everything a run needs once it has started.
struct RunPlan {
    items: Arc<Vec<Item>>,
    repeat: usize,
    set: Arc<std::collections::BTreeMap<String, String>>,
    destination: Destination,
    /// `destination.display()`: the sent messages' `source_addr`.
    peer: String,
    requested_by: String,
    allow_invalid: bool,
    await_response: bool,
    interval: Duration,
    stop_on_nack: bool,
}

/// How a run stands or how it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    /// Every message was tried.
    Done,
    /// Stopped by `POST /api/send/runs/{id}/cancel`.
    Cancelled,
    /// Three transport failures in a row.
    GaveUp,
    /// `stop_on_nack` and an answer that was not an accept.
    StoppedOnNack,
    /// Harux shut down.
    Shutdown,
}

/// Progress and result of a run (`GET /api/send/runs/{id}`).
#[derive(Debug, Clone, Serialize)]
pub struct RunStatus {
    pub id: String,
    /// The target's label.
    pub target: String,
    pub requested_by: String,
    pub total: usize,
    /// Messages tried so far (stored, or counted as failed when one could
    /// not be built).
    pub done: usize,
    pub state: RunState,
    /// Responses by acknowledgement code (`AA`, `AE`, …).
    pub codes: std::collections::BTreeMap<String, usize>,
    /// Written without awaiting a response.
    pub written: usize,
    /// No usable answer: transport failures, unreadable responses, messages
    /// that could not be built.
    pub failed: usize,
    /// Average time of the exchanges that ended with an answer.
    pub avg_latency_ms: Option<u64>,
    /// Why the run stopped early, or the last problem.
    pub reason: Option<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip)]
    latency_sum: u64,
    #[serde(skip)]
    latency_count: u64,
}

impl RunStatus {
    fn new(target: &str, total: usize, requested_by: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            target: target.to_string(),
            requested_by,
            total,
            done: 0,
            state: RunState::Running,
            codes: Default::default(),
            written: 0,
            failed: 0,
            avg_latency_ms: None,
            reason: None,
            started_at: chrono::Utc::now(),
            finished_at: None,
            latency_sum: 0,
            latency_count: 0,
        }
    }

    /// For callers of the library (the integration tests).
    #[allow(dead_code)]
    pub fn running(&self) -> bool {
        self.state == RunState::Running
    }

    fn count(&mut self, d: &Delivery) {
        self.done += 1;
        match d.state {
            DeliveryState::Acked => {
                let code = d.ack_code.clone().unwrap_or_else(|| "?".into());
                *self.codes.entry(code).or_default() += 1;
            }
            DeliveryState::Sent => self.written += 1,
            DeliveryState::Failed | DeliveryState::Pending => {
                self.failed += 1;
                self.reason = Some(describe(d));
            }
        }
        if d.state == DeliveryState::Acked {
            if let Some(ms) = d.latency_ms {
                self.latency_sum = self.latency_sum.saturating_add(ms);
                self.latency_count += 1;
                self.avg_latency_ms = Some(self.latency_sum / self.latency_count);
            }
        }
    }
}

/// A run in the registry: its status, and the switch that cancels it.
/// The runs the server keeps: every run under way, by ID, and the newest
/// [`KEPT_RUNS`] finished ones, oldest first.
#[derive(Default)]
struct Registry {
    running: std::collections::HashMap<String, Arc<RunHandle>>,
    finished: std::collections::VecDeque<Arc<RunHandle>>,
}

struct RunHandle {
    /// `status.id`, readable without the lock.
    id: String,
    status: std::sync::Mutex<RunStatus>,
    cancel: watch::Sender<bool>,
}

impl RunHandle {
    fn status(&self) -> RunStatus {
        lock(&self.status).clone()
    }

    fn update(&self, f: impl FnOnce(&mut RunStatus)) {
        f(&mut lock(&self.status));
    }
}

/// A std mutex that a panic elsewhere did not leave unusable: the data are
/// counters, valid after any partial update.
fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether the target took the message: `AA`/`CA`, or written without
/// awaiting a response.
fn accepted(d: &Delivery) -> bool {
    d.state == DeliveryState::Sent || matches!(d.ack_code.as_deref(), Some("AA" | "CA"))
}

/// Resolves when the switch is on (at once if it already is).
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

/// `describe` as a log line may hold it (#307, rule 7: metadata only). A
/// failure the target answered — a response that is not HL7 or has no
/// usable MSA — is named by its short reason and the response's size: its
/// detail can quote the response (a parse error quotes the first 20
/// characters). The detail stays in the stored delivery, where the UI shows
/// it. A failure without a response (refused, timed out, TLS) is Harux's own
/// text and is logged in full.
fn describe_for_log(d: &Delivery) -> String {
    match (d.state, d.response_size) {
        (DeliveryState::Failed, Some(size)) => {
            let ms = d
                .latency_ms
                .map_or(String::new(), |ms| format!(" in {ms} ms"));
            format!(
                "{} ({size} bytes of response){ms}",
                d.error.as_deref().unwrap_or("failed")
            )
        }
        _ => describe(d),
    }
}

/// One exchange of a run: on `connection` if it is open, else on a new one
/// (with TLS if the target uses it); write the stored bytes, read the
/// response. If the kept connection turns out to be closed by the target
/// before it read the message ([`mllp_client::ExchangeError::peer_hung_up`]),
/// the message is written once more on a new connection. Returns the connection if it can be used again, the outcome,
/// and whether it was a transport failure (connect, write, read, timeout —
/// the connection is then dropped).
async fn exchange_on(
    connection: Option<mllp_client::Connection>,
    sent: Arc<Hl7Message>,
    destination: Destination,
    tls: Option<Arc<TlsContext>>,
    await_response: bool,
    timeouts: Timeouts,
) -> (Option<mllp_client::Connection>, Delivery, bool) {
    let started = Instant::now();
    let failed = |e: mllp_client::ExchangeError| {
        Delivery::failed(e.reason(), e.detail(), Some(elapsed_ms(started)))
    };
    // Unchanged means byte for byte (#230): what was received.
    let payload = sent.wire_bytes();
    // A connection kept from the last message that the target has closed
    // since (it found no byte of an answer, or the write found it closed):
    // the target never read the message, so it is written once more on a
    // new connection.
    let mut connection = match connection {
        Some(mut c) => match c.exchange(&payload, await_response, timeouts).await {
            Ok(response) => return answered(c, response, &sent, started),
            Err(e) if e.peer_hung_up() => {
                drop(c);
                match open(&destination, tls.as_deref(), timeouts).await {
                    Ok(c) => c,
                    Err(d) => return (None, *d, true),
                }
            }
            Err(e) => return (None, failed(e), true),
        },
        None => match open(&destination, tls.as_deref(), timeouts).await {
            Ok(c) => c,
            Err(d) => return (None, *d, true),
        },
    };
    match connection
        .exchange(&payload, await_response, timeouts)
        .await
    {
        Ok(response) => answered(connection, response, &sent, started),
        Err(e) => (None, failed(e), true),
    }
}

/// The outcome of an exchange that ended with an answer, or without one
/// awaited. Only after an answer can the connection take the next message:
/// after a message written without awaiting one (`await_response: false`, an
/// ACK) the target may still answer it, and that late answer would be read
/// as the next message's (#293 review) — so that connection is closed.
fn answered(
    connection: mllp_client::Connection,
    response: Option<Vec<u8>>,
    sent: &Hl7Message,
    started: Instant,
) -> (Option<mllp_client::Connection>, Delivery, bool) {
    match response {
        None => {
            let d = Delivery {
                state: DeliveryState::Sent,
                latency_ms: Some(elapsed_ms(started)),
                ..Delivery::pending()
            };
            (None, d, false)
        }
        Some(response) => {
            let d = read_response(&response, &sent.message_control_id, elapsed_ms(started));
            (Some(connection), d, false)
        }
    }
}

/// Open a connection to `destination`, with TLS if it uses it.
async fn open(
    destination: &Destination,
    tls: Option<&TlsContext>,
    timeouts: Timeouts,
) -> Result<mllp_client::Connection, Box<Delivery>> {
    let started = Instant::now();
    let tls = match destination.transport {
        Transport::Plain => None,
        Transport::Tls | Transport::TlsUnverified => {
            let Some(ctx) = tls else {
                return Err(Box::new(Delivery::failed(
                    "TLS failed",
                    "TLS is not set up on this server (send.enabled)",
                    None,
                )));
            };
            Some((ctx, destination.transport == Transport::Tls))
        }
    };
    mllp_client::Connection::open(&destination.host, destination.port, timeouts.connect, tls)
        .await
        .map_err(|e| {
            Box::new(Delivery::failed(
                e.reason(),
                e.detail(),
                Some(elapsed_ms(started)),
            ))
        })
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
    use crate::hl7::charset;
    use crate::hl7::compose::{self, FieldPath, PlaceholderContext};
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
        let path = FieldPath::parse(path)?;
        let value = compose::fill_placeholders(value, &ctx);
        text = compose::set_field(&text, d, &path, &value)?;
        changes.push(path.to_string());
    }
    // Compared with the parser's own list, so "the same segments" means "as
    // the parser reads them" (#287 review).
    if !compose::segment_names(&text, d)
        .into_iter()
        .eq(original.segments.iter().map(|s| s.name.as_str()))
    {
        return Err(SendError::Unprocessable(
            "The changes would alter the message's list of segments".into(),
        ));
    }
    let declared_now = charset::declared_charset(text.as_bytes());
    let bytes = charset::encode_for_send(
        &text,
        &original.wire,
        original.raw.is_ascii(),
        original.charset.as_deref(),
        declared_now.as_deref(),
    )
    .map_err(SendError::Unprocessable)?;
    // The receiver must read what the sent row shows: decoded by its own
    // MSH-18, the bytes give back the text, without a charset warning the
    // original did not already carry.
    let check = charset::decode_payload(&bytes);
    let fell_back =
        |w: &[crate::validation::ValidationWarning]| w.iter().any(|w| w.code == "CHARSET_FALLBACK");
    if check.text != text || (check.warning.is_some() && !fell_back(&original.validation_warnings))
    {
        return Err(SendError::Unprocessable(format!(
            "The changed message would not read back as written: {}",
            check
                .warning
                .map(|w| w.message)
                .unwrap_or_else(|| "its bytes decode to other text".into())
        )));
    }
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

    /// #307: the log line of a send whose response does not parse holds no
    /// text of the response; the stored delivery keeps the detail.
    #[test]
    fn the_log_line_of_a_response_that_is_not_hl7_quotes_none_of_it() {
        let d = read_response(b"PID|1||4711^^^HOSP^M\r", "C1", 3);
        assert!(
            d.error_detail.as_deref().unwrap().contains("4711"),
            "kept for the UI"
        );
        let line = describe_for_log(&d);
        assert_eq!(line, "invalid response (21 bytes of response) in 3 ms");
        assert!(!line.contains("PID") && !line.contains("4711"), "{line}");
        // A failure without a response keeps its detail: it is Harux's text.
        let refused = Delivery::failed("refused", "Connection refused by 10.0.0.1:2575", Some(1));
        assert!(describe_for_log(&refused).contains("Connection refused by 10.0.0.1:2575"));
        // An answer is logged as before.
        let acked = read_response(
            b"MSH|^~\\&|R|RF|||20260101||ACK|X|P|2.5\rMSA|AA|C1\r",
            "C1",
            2,
        );
        assert_eq!(describe_for_log(&acked), "AA in 2 ms");
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
