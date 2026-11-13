use crate::config::StoreConfig;
use crate::hl7::types::{
    heap_bytes, Delivery, Direction, Hl7Message, Hl7MessageSummary, MessageMeta, MessageSnapshot,
};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{info, warn};

const BROADCAST_CAPACITY: usize = 4096;

/// A search query as the UI writes it — parsed like `getParsedQuery` in
/// `static/util.js`, and matched like `matchesSearch` in `static/render.js`,
/// so a server search finds what the client filter shows (#197): optional
/// prefixes — `has:warnings` / `has:errors`, `is:sent` / `is:received`
/// (#272), in any order — then a case-insensitive substring of the patient
/// name or ID, message type, facility, control ID, source address (for a
/// sent message: its destination and its target's name) or a tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    text: String,
    has_warnings: bool,
    has_errors: bool,
    direction: Option<Direction>,
}

impl SearchQuery {
    pub fn parse(query: &str) -> Self {
        let lower = query.trim().to_lowercase();
        let mut rest = lower.as_str();
        let (mut has_warnings, mut has_errors, mut direction) = (false, false, None);
        loop {
            rest = rest.trim_start();
            if let Some(r) = rest.strip_prefix("has:warnings") {
                (has_warnings, has_errors) = (true, false);
                rest = r;
            } else if let Some(r) = rest.strip_prefix("has:errors") {
                (has_warnings, has_errors) = (false, true);
                rest = r;
            } else if let Some(r) = rest.strip_prefix("is:sent") {
                direction = Some(Direction::Out);
                rest = r;
            } else if let Some(r) = rest.strip_prefix("is:received") {
                direction = Some(Direction::In);
                rest = r;
            } else {
                break;
            }
        }
        Self {
            text: rest.trim().to_string(),
            has_warnings,
            has_errors,
            direction,
        }
    }

    fn matches(&self, entry: &StoredEntry) -> bool {
        let m = &entry.message;
        if self.direction.is_some_and(|d| d != m.direction()) {
            return false;
        }
        // has:warnings = any warning; a segment error is one, too.
        if self.has_warnings && m.validation_warnings.is_empty() {
            return false;
        }
        if self.has_errors
            && !m
                .validation_warnings
                .iter()
                .any(|w| w.code == "MISSING_SEGMENT")
        {
            return false;
        }
        let q = self.text.as_str();
        if q.is_empty() {
            return true;
        }
        // `text` is lowercased once in `parse`; only the haystack is folded
        // per field (and only for a non-ASCII needle).
        let hit = |v: &str| contains_lowercase(v, q);
        let opt = |v: &Option<String>| v.as_deref().is_some_and(hit);
        opt(&m.patient_name)
            || hit(&m.message_type)
            || hit(&m.sending_facility)
            || opt(&m.patient_id)
            || hit(&m.message_control_id)
            || hit(&m.source_addr)
            || m.outbound.as_ref().is_some_and(|o| hit(&o.target))
            || entry.meta.tags.iter().any(|t| hit(t))
    }
}

/// Case-insensitive `haystack.contains(needle)` for a needle that is already
/// lowercase — [`SearchQuery`] folds it once instead of once per field.
fn contains_lowercase(haystack: &str, needle: &str) -> bool {
    if needle.is_ascii() {
        let needle = needle.as_bytes();
        return needle.is_empty()
            || haystack
                .as_bytes()
                .windows(needle.len())
                .any(|w| w.eq_ignore_ascii_case(needle));
    }
    haystack.to_lowercase().contains(needle)
}

#[derive(Clone)]
#[allow(clippy::large_enum_variant)]
pub enum StoreEvent {
    NewMessage(Box<Hl7MessageSummary>),
    TagsUpdated(Box<Hl7MessageSummary>),
    BookmarkToggled(Box<Hl7MessageSummary>),
    /// A sent message's delivery changed (#272): the summary, which carries
    /// no response text.
    DeliveryUpdated(Box<Hl7MessageSummary>),
    /// IDs removed by an eviction pass (oldest first). Shared via `Arc` so the
    /// broadcast clone per subscriber does not copy the ID list.
    Evicted(Arc<Vec<String>>),
    Cleared,
}

/// What the broadcast channel carries: a [`StoreEvent`] plus its WebSocket
/// frame, rendered **once** for all subscribers (#196). Before, every
/// connected browser's task serialized the same summary itself, so N viewers
/// meant N identical JSON strings per message. The first subscriber to send
/// renders it via [`EventFrame::cached_json`]; the others reuse the text. The
/// wire format stays in `web.rs`, which passes the renderer.
pub struct EventFrame {
    pub event: StoreEvent,
    json: std::sync::OnceLock<String>,
}

impl EventFrame {
    fn new(event: StoreEvent) -> Arc<Self> {
        Arc::new(Self {
            event,
            json: std::sync::OnceLock::new(),
        })
    }

    /// The rendered frame: `render` runs on the first call only, every later
    /// call (from any subscriber) returns the same text.
    pub fn cached_json(&self, render: impl FnOnce(&StoreEvent) -> String) -> &str {
        self.json.get_or_init(|| render(&self.event))
    }
}

/// One broadcast item, shared by all subscribers.
pub type SharedEvent = Arc<EventFrame>;

/// Thread-safe in-memory message store with broadcast notifications
#[derive(Clone)]
pub struct MessageStore {
    inner: Arc<RwLock<StoreInner>>,
    tx: broadcast::Sender<SharedEvent>,
    eviction_notify: Arc<tokio::sync::Notify>,
    eviction_flag: Arc<std::sync::atomic::AtomicBool>,
}

/// One stored message: the immutable payload, its mutable metadata and the
/// bytes it is accounted for.
///
/// The payload is `Arc`-shared so `get_by_id` hands out a cheap snapshot
/// instead of cloning a (potentially multi-MB) message. It is **never mutated
/// after insert** — tags and bookmark live in `meta` (#192), so a mutation
/// can't trigger a copy-on-write deep clone while a reader holds a snapshot.
struct StoredEntry {
    message: Arc<Hl7Message>,
    meta: MessageMeta,
    /// `message.estimated_bytes() + meta.estimated_bytes()`, computed once at
    /// insert and adjusted by the delta on every meta mutation. Eviction and
    /// accounting use this instead of re-walking the parsed message.
    bytes: usize,
}

// Messages are addressed by ID through `messages` (O(1) lookup) and ordered
// for iteration / eviction through `order`.
struct StoreInner {
    order: VecDeque<String>,
    messages: HashMap<String, StoredEntry>,
    capacity: usize,
    max_bytes: usize,
    current_bytes: usize,
    /// Max number of bookmarked messages kept under eviction pressure.
    bookmark_budget_count: usize,
    /// Max estimated bytes of bookmarked messages kept under eviction pressure.
    bookmark_budget_bytes: usize,
    /// Number of bookmarked messages currently stored — maintained
    /// incrementally so an eviction pass never has to scan the whole store.
    bookmark_count: usize,
    /// Sum of entry bytes over bookmarked messages (incremental).
    bookmark_bytes: usize,
}

/// Bytes the store spends on one entry besides what the payload and the
/// metadata hold themselves (#226): the `Arc` allocation that contains the
/// `Hl7Message` struct, the two extra copies of the ID (in `order` and as map
/// key), the `order` slot and the hash-table slot at maximum load.
fn entry_overhead(id: &str) -> usize {
    heap_bytes(2 * std::mem::size_of::<usize>() + std::mem::size_of::<Hl7Message>())
        + 2 * heap_bytes(id.len())
        + std::mem::size_of::<String>()
        + (std::mem::size_of::<(String, StoredEntry)>() + 1) * 8 / 7
}

/// Everything one stored message is accounted for — the value cached in
/// `StoredEntry::bytes` and summed in `current_bytes`.
fn entry_bytes(msg: &Hl7Message, meta: &MessageMeta) -> usize {
    msg.estimated_bytes() + meta.estimated_bytes() + entry_overhead(&msg.id)
}

/// Result of a single eviction pass (see [`StoreInner::evict`]).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct EvictionOutcome {
    /// Non-bookmarked messages evicted to reach the low watermark.
    evicted: usize,
    /// Bookmarked messages evicted because bookmarks exceeded their budget.
    evicted_bookmarks: usize,
    /// Estimated bytes released by this pass.
    freed_bytes: usize,
    /// IDs of every evicted message, in eviction order — broadcast to the UI.
    ids: Vec<String>,
}

/// `limit` minus 10 % (at least 1) — the hysteresis target an eviction pass
/// drives the store down to, so eviction does not re-fire on every insert.
fn low_watermark(limit: usize) -> usize {
    limit.saturating_sub((limit / 10).max(1))
}

impl StoreInner {
    /// Either hard limit reached → eviction required.
    fn over_limit(&self) -> bool {
        self.current_bytes >= self.max_bytes || self.order.len() >= self.capacity
    }

    /// Still above the low watermark of either limit → keep evicting.
    /// `set_aside` counts IDs temporarily popped off `order` that are still
    /// stored (skipped bookmarks) so they are not mistaken for evictions.
    fn above_low_watermark(&self, set_aside: usize) -> bool {
        self.current_bytes > low_watermark(self.max_bytes)
            || self.order.len() + set_aside > low_watermark(self.capacity)
    }

    /// Add (`add = true`) or remove a message's share of the bookmark totals.
    fn account_bookmark(&mut self, bytes: usize, add: bool) {
        if add {
            self.bookmark_count += 1;
            self.bookmark_bytes += bytes;
        } else {
            self.bookmark_count = self.bookmark_count.saturating_sub(1);
            self.bookmark_bytes = self.bookmark_bytes.saturating_sub(bytes);
        }
    }

    fn bookmarks_over_budget(&self) -> bool {
        self.bookmark_count > self.bookmark_budget_count
            || self.bookmark_bytes > self.bookmark_budget_bytes
    }

    /// Remove a message by ID and release its bytes from the accounting.
    /// Does not touch `order` — callers manage the queue themselves.
    fn remove_message(&mut self, id: &str) -> Option<usize> {
        let removed = self.messages.remove(id)?;
        let bytes = removed.bytes;
        self.current_bytes = self.current_bytes.saturating_sub(bytes);
        if removed.meta.bookmarked {
            self.account_bookmark(bytes, false);
        }
        Some(bytes)
    }

    /// Run one eviction pass if a hard limit is reached.
    ///
    /// 1. **Bookmark budget** — if bookmarked messages exceed their budget
    ///    (`bookmark_budget_percent` of either limit), the oldest bookmarks are
    ///    evicted until they fit. Bookmark protection therefore can never lift
    ///    the memory cap (#178).
    /// 2. **Low watermark** — the oldest non-bookmarked messages are evicted
    ///    until both bytes and count are at or below 90 % of their limits.
    ///    Because the bookmark budget is capped below 90 %
    ///    ([`crate::config::MAX_BOOKMARK_BUDGET_PERCENT`]), this always
    ///    succeeds — the store is back under its limits after every pass.
    ///
    /// Phase 1 is O(n) over `order` but only runs when bookmarks actually
    /// exceed their budget (checked in O(1) via the incremental totals);
    /// Phase 2 touches only the messages it evicts plus skipped bookmarks.
    /// Surviving bookmarks keep their original relative order.
    fn evict(&mut self) -> EvictionOutcome {
        let mut outcome = EvictionOutcome::default();
        if !self.over_limit() {
            return outcome;
        }

        // Phase 1: enforce the bookmark budget (oldest bookmarks first).
        //
        // This deliberately runs *before* the watermark phase, even when
        // evicting non-bookmarks alone would have been enough this time: the
        // budget is what bounds Phase 2. With bookmarks capped at <= 80 % of
        // each limit, Phase 2 is guaranteed to reach the 90 % watermark; if
        // Phase 2 ran first, an over-budget bookmark set could leave it unable
        // to reach the target (the pre-#178 starvation bug), and live traffic
        // would be squeezed into the remaining headroom.
        if self.bookmarks_over_budget() {
            let order = std::mem::take(&mut self.order);
            let mut remaining = VecDeque::with_capacity(order.len());
            for id in order {
                let bookmarked = self.messages.get(&id).is_some_and(|e| e.meta.bookmarked);
                if bookmarked && self.bookmarks_over_budget() {
                    if let Some(bytes) = self.remove_message(&id) {
                        outcome.evicted_bookmarks += 1;
                        outcome.freed_bytes += bytes;
                        outcome.ids.push(id);
                    }
                } else {
                    remaining.push_back(id);
                }
            }
            self.order = remaining;
        }

        // Phase 2: evict oldest non-bookmarked messages down to the low watermark.
        let mut kept_bookmarks: Vec<String> = Vec::new();
        while self.above_low_watermark(kept_bookmarks.len()) {
            let Some(id) = self.order.pop_front() else {
                break;
            };
            match self.messages.get(&id).map(|e| e.meta.bookmarked) {
                Some(true) => kept_bookmarks.push(id),
                Some(false) => {
                    if let Some(bytes) = self.remove_message(&id) {
                        if bytes >= self.max_bytes {
                            warn!(
                                "Message {} ({} KB) alone exceeds the store memory limit \
                                 ({} KB, store.max_memory_mb) and was dropped",
                                id,
                                bytes / 1024,
                                self.max_bytes / 1024,
                            );
                        }
                        outcome.evicted += 1;
                        outcome.freed_bytes += bytes;
                        outcome.ids.push(id);
                    }
                }
                // Dangling ID without a message — drop it from the queue.
                None => {}
            }
        }

        // Restore bookmarked IDs at the front in original order.
        for id in kept_bookmarks.into_iter().rev() {
            self.order.push_front(id);
        }

        outcome
    }
}

impl MessageStore {
    /// Create a new MessageStore.
    ///
    /// Must be called from within a Tokio runtime — spawns a background eviction task.
    pub fn new(config: StoreConfig) -> Self {
        let store = Self::without_eviction_task(config);

        // Spawn background eviction task
        let store_clone = store.clone();
        tokio::spawn(async move {
            store_clone.run_eviction_loop().await;
        });

        store
    }

    /// Build the store without spawning the background eviction task.
    /// Tests use this to drive eviction deterministically via `evict_now`.
    fn without_eviction_task(config: StoreConfig) -> Self {
        if config.bookmark_budget_percent != config.bookmark_budget_percent() {
            warn!(
                "store.bookmark_budget_percent = {} exceeds the maximum — clamped to {}%",
                config.bookmark_budget_percent,
                config.bookmark_budget_percent()
            );
        }
        let pct = config.bookmark_budget_percent();
        let max_bytes = config.max_memory_bytes();
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            inner: Arc::new(RwLock::new(StoreInner {
                order: VecDeque::with_capacity(1024),
                messages: HashMap::with_capacity(1024),
                capacity: config.max_messages,
                max_bytes,
                current_bytes: 0,
                bookmark_budget_count: config.max_messages * pct / 100,
                bookmark_budget_bytes: max_bytes.saturating_mul(pct) / 100,
                bookmark_count: 0,
                bookmark_bytes: 0,
            })),
            tx,
            eviction_notify: Arc::new(tokio::sync::Notify::new()),
            eviction_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Insert a message and broadcast summary to all WebSocket subscribers
    pub async fn insert(&self, msg: Hl7Message) {
        self.insert_entry(msg, MessageMeta::default()).await;
    }

    /// Insert a message Harux is about to send (#272), with delivery
    /// `pending`, and return the stored payload: the sender writes its bytes
    /// from this `Arc` instead of holding a copy of its own. Stored **before**
    /// the connection is opened, so what the destination forwards because of
    /// the send can never be listed above it.
    pub async fn insert_sent(&self, msg: Hl7Message) -> Arc<Hl7Message> {
        let meta = MessageMeta {
            delivery: Some(Box::new(Delivery::pending())),
            ..MessageMeta::default()
        };
        self.insert_entry(msg, meta).await
    }

    async fn insert_entry(&self, msg: Hl7Message, meta: MessageMeta) -> Arc<Hl7Message> {
        let summary = Hl7MessageSummary::new(&msg, &meta);
        // Walk the parsed message once, outside the lock.
        let bytes = entry_bytes(&msg, &meta);
        let id = msg.id.clone();
        let message = Arc::new(msg);
        let entry = StoredEntry {
            message: Arc::clone(&message),
            meta,
            bytes,
        };

        let mut inner = self.inner.write().await;
        inner.current_bytes += bytes;
        inner.order.push_back(id.clone());
        inner.messages.insert(id, entry);
        let count = inner.order.len();
        let needs_eviction = inner.over_limit();
        // Broadcast while the write lock is held (#232) — the rule for all
        // four send sites: `insert`, `mutate_meta`, `evict_now`, `clear`.
        // The order on the channel is then the order in which the store
        // changed. Sent after the lock is released, an eviction of
        // this very message could reach subscribers before `NewMessage`.
        // `send` never blocks. No receivers is not an error.
        let _ = self
            .tx
            .send(EventFrame::new(StoreEvent::NewMessage(Box::new(summary))));
        drop(inner);

        if needs_eviction {
            self.wake_eviction();
        }

        if count % 1000 == 0 {
            info!("Store now holds {} messages", count);
        }
        message
    }

    /// Wake the background eviction task (once per pass).
    fn wake_eviction(&self) {
        // Ordering::Relaxed is sufficient because notify_one() handles the necessary memory barriers
        if !self
            .eviction_flag
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            self.eviction_notify.notify_one();
        }
    }

    /// Get a broadcast receiver for real-time updates
    pub fn subscribe(&self) -> broadcast::Receiver<SharedEvent> {
        self.tx.subscribe()
    }

    /// Get all message summaries (lightweight)
    pub async fn list_summaries(&self, offset: usize, limit: usize) -> Vec<Hl7MessageSummary> {
        let inner = self.inner.read().await;
        inner
            .order
            .iter()
            .rev() // newest first
            .skip(offset)
            .take(limit)
            .filter_map(|id| inner.messages.get(id))
            .map(|e| Hl7MessageSummary::new(&e.message, &e.meta))
            .collect()
    }

    /// Like [`Self::list_summaries`], plus every bookmarked message beyond
    /// that page, in the same newest-first order (#231). A reload otherwise
    /// only sees the newest page, and a bookmark older than that can't be
    /// found in the UI although the store protects it. Bookmarks are bounded
    /// by `store.bookmark_budget_percent`, so the extra rows are too.
    pub async fn list_summaries_with_bookmarks(
        &self,
        offset: usize,
        limit: usize,
    ) -> Vec<Hl7MessageSummary> {
        let inner = self.inner.read().await;
        inner
            .order
            .iter()
            .rev() // newest first
            .enumerate()
            .filter_map(|(pos, id)| {
                let e = inner.messages.get(id)?;
                let in_page = pos >= offset && pos - offset < limit;
                (in_page || (pos >= offset && e.meta.bookmarked))
                    .then(|| Hl7MessageSummary::new(&e.message, &e.meta))
            })
            .collect()
    }

    /// Get a full message by ID: an `Arc` snapshot of the payload (no deep
    /// clone) plus a copy of its small metadata.
    pub async fn get_by_id(&self, id: &str) -> Option<MessageSnapshot> {
        self.inner
            .read()
            .await
            .messages
            .get(id)
            .map(|e| MessageSnapshot {
                message: Arc::clone(&e.message),
                meta: e.meta.clone(),
                direction: e.message.direction(),
            })
    }

    /// Snapshot of every stored message payload, in insertion order (oldest
    /// first — so replaying an export preserves the original sequence).
    ///
    /// Only the `Arc`s are cloned (O(n) pointer copies, no payload bytes), so
    /// the read lock is held briefly and a full store costs a few hundred KB
    /// here instead of a second copy of every raw message (#195). The
    /// snapshot keeps evicted messages alive until the caller drops them.
    ///
    /// Safe to hold during tag/bookmark writes only because metadata lives in
    /// `MessageMeta`, not in the payload (#192): the extra `Arc` references
    /// never trigger a copy-on-write. Do not move mutable fields back into
    /// `Hl7Message`.
    ///
    /// `direction` limits it to received or sent messages (#272); `None`
    /// takes both.
    pub async fn payload_snapshot(&self, direction: Option<Direction>) -> Vec<Arc<Hl7Message>> {
        let inner = self.inner.read().await;
        inner
            .order
            .iter()
            .filter_map(|id| inner.messages.get(id))
            .filter(|e| direction.is_none_or(|d| d == e.message.direction()))
            .map(|e| Arc::clone(&e.message))
            .collect()
    }

    /// Up to `limit` messages matching `query` (all of them if `None`),
    /// newest first, starting below `before`: only messages older than the
    /// one with that ID (#197, #198). A cursor instead of an offset, because
    /// new messages arriving between two requests shift every offset. An
    /// unknown `before` (evicted meanwhile — and with it everything older
    /// that isn't a bookmark) gives an empty page rather than restarting
    /// from the newest.
    pub async fn older_than(
        &self,
        before: Option<&str>,
        query: Option<&SearchQuery>,
        limit: usize,
    ) -> Vec<Hl7MessageSummary> {
        let inner = self.inner.read().await;
        let end = match before {
            Some(id) => match inner.order.iter().rposition(|o| o == id) {
                Some(pos) => pos,
                None => return Vec::new(),
            },
            None => inner.order.len(),
        };
        inner
            .order
            .range(..end)
            .rev() // newest first
            .filter_map(|id| inner.messages.get(id))
            .filter(|e| query.is_none_or(|q| q.matches(e)))
            .take(limit)
            .map(|e| Hl7MessageSummary::new(&e.message, &e.meta))
            .collect()
    }

    /// Total message count
    pub async fn count(&self) -> usize {
        self.inner.read().await.order.len()
    }

    /// `(accounted bytes, byte limit)` — what `store.max_memory_mb` is
    /// compared against, exposed in `/api/stats`.
    pub async fn memory_usage(&self) -> (usize, usize) {
        let inner = self.inner.read().await;
        (inner.current_bytes, inner.max_bytes)
    }

    /// Apply a mutation to a message's metadata (tags, bookmark).
    /// Returns the new summary if the message was found and the mutation ran, or None otherwise.
    ///
    /// Only the small `MessageMeta` is touched — the `Arc`-shared payload is
    /// never mutated, so there is no copy-on-write deep clone under the write
    /// lock even while readers hold snapshots (#192). The entry's byte count
    /// is adjusted by the metadata delta so accounting stays exact (#179).
    ///
    /// The resulting event (`event(summary)`) is broadcast while the write
    /// lock is still held (#232), so two concurrent mutations reach
    /// subscribers in the order they were applied and the last event always
    /// matches the store. Returns whether the message existed and `f`
    /// changed it.
    async fn mutate_meta(
        &self,
        id: &str,
        f: impl FnOnce(&mut MessageMeta) -> bool,
        event: fn(Box<Hl7MessageSummary>) -> StoreEvent,
    ) -> bool {
        let mut guard = self.inner.write().await;
        // Reborrow once so `messages` and the counters can be borrowed disjointly.
        let inner = &mut *guard;
        let Some(entry) = inner.messages.get_mut(id) else {
            return false;
        };

        let before = entry.bytes;
        let was_bookmarked = entry.meta.bookmarked;
        let meta_before = entry.meta.estimated_bytes();
        if !f(&mut entry.meta) {
            return false;
        }
        entry.bytes = (before + entry.meta.estimated_bytes()).saturating_sub(meta_before);
        let after = entry.bytes;
        let is_bookmarked = entry.meta.bookmarked;
        let summary = Hl7MessageSummary::new(&entry.message, &entry.meta);

        inner.current_bytes = (inner.current_bytes + after).saturating_sub(before);
        if was_bookmarked {
            inner.account_bookmark(before, false);
        }
        if is_bookmarked {
            inner.account_bookmark(after, true);
        }

        // Moved into the event — nothing else needs it (no clone under the lock).
        let _ = self.tx.send(EventFrame::new(event(Box::new(summary))));
        // Metadata can grow an entry (a response, #272): check the limits as
        // `insert` does instead of waiting for the next insert.
        let needs_eviction = inner.over_limit();
        drop(guard);
        if needs_eviction {
            self.wake_eviction();
        }
        true
    }

    /// Record what became of a sent message (#272) and broadcast
    /// `DeliveryUpdated`. `false` when the message is gone (evicted or
    /// cleared before its outcome arrived): the caller only logs it.
    pub async fn set_delivery(&self, id: &str, delivery: Delivery) -> bool {
        self.mutate_meta(
            id,
            |meta| {
                meta.delivery = Some(Box::new(delivery));
                true
            },
            StoreEvent::DeliveryUpdated,
        )
        .await
    }

    /// Add a tag to a message and broadcast the update
    pub async fn add_tag(&self, id: &str, tag: String) -> bool {
        let tag_clone = tag.clone();
        self.mutate_meta(
            id,
            |meta| {
                if meta.tags.contains(&tag_clone) {
                    return false;
                }
                meta.tags.push(tag_clone);
                true
            },
            StoreEvent::TagsUpdated,
        )
        .await
    }

    /// Remove a tag from a message and broadcast the update
    pub async fn remove_tag(&self, id: &str, tag: &str) -> bool {
        let tag = tag.to_string();
        self.mutate_meta(
            id,
            |meta| {
                if let Some(pos) = meta.tags.iter().position(|t| *t == tag) {
                    meta.tags.remove(pos);
                    true
                } else {
                    false
                }
            },
            StoreEvent::TagsUpdated,
        )
        .await
    }

    /// Toggle bookmark on a message, returns the new bookmark state or None if not found
    pub async fn toggle_bookmark(&self, id: &str) -> Option<bool> {
        let mut new_state = false;
        self.mutate_meta(
            id,
            |meta| {
                meta.bookmarked = !meta.bookmarked;
                new_state = meta.bookmarked;
                true
            },
            StoreEvent::BookmarkToggled,
        )
        .await
        .then_some(new_state)
    }

    /// Clear all messages
    pub async fn clear(&self) {
        let mut inner = self.inner.write().await;
        inner.order.clear();
        inner.messages.clear();
        inner.current_bytes = 0;
        inner.bookmark_count = 0;
        inner.bookmark_bytes = 0;
        info!("Message store cleared");
        let _ = self.tx.send(EventFrame::new(StoreEvent::Cleared));
    }

    async fn run_eviction_loop(self) {
        loop {
            self.eviction_notify.notified().await;
            // Ordering::Relaxed is sufficient because notified() handles the necessary memory barriers
            self.eviction_flag
                .store(false, std::sync::atomic::Ordering::Relaxed);

            self.evict_now().await;
        }
    }

    /// Run one eviction pass under the write lock and log the result.
    async fn evict_now(&self) -> EvictionOutcome {
        let mut inner = self.inner.write().await;
        let outcome = inner.evict();

        if outcome.evicted_bookmarks > 0 {
            warn!(
                "Bookmarks exceeded their budget (store.bookmark_budget_percent of the \
                 count/bytes limit) — evicted {} oldest bookmarked messages",
                outcome.evicted_bookmarks
            );
        }
        if !outcome.ids.is_empty() {
            // Sent under the write lock (same pattern as `clear`) so subscribers
            // see the eviction before any later insert/mutation event.
            let _ = self.tx.send(EventFrame::new(StoreEvent::Evicted(Arc::new(
                outcome.ids.clone(),
            ))));
        }
        if outcome.evicted + outcome.evicted_bookmarks > 0 {
            info!(
                "Evicted {} messages from store ({} MB freed, store now {} messages / {} MB)",
                outcome.evicted + outcome.evicted_bookmarks,
                outcome.freed_bytes / 1024 / 1024,
                inner.order.len(),
                inner.current_bytes / 1024 / 1024,
            );
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StoreConfig;

    fn make_store(max_messages: usize) -> MessageStore {
        MessageStore::new(StoreConfig {
            max_messages,
            max_memory_mb: 512,
            ..StoreConfig::default()
        })
    }

    /// Store without the background eviction task — eviction runs only when
    /// the test calls `evict_now()`, so results are deterministic.
    fn manual_store(max_messages: usize, max_memory_mb: usize) -> MessageStore {
        MessageStore::without_eviction_task(StoreConfig {
            max_messages,
            max_memory_mb,
            ..StoreConfig::default()
        })
    }

    fn make_msg(id: &str) -> Hl7Message {
        let mut msg = Hl7Message::new_empty(format!("raw-{id}"), "127.0.0.1:5000".into());
        msg.id = id.to_string();
        msg
    }

    /// #196: all subscribers share one rendered frame per event — the
    /// renderer runs once, and every subscriber gets the same text.
    #[tokio::test]
    async fn test_event_frame_is_rendered_once_for_all_subscribers() {
        let store = manual_store(100, 512);
        let (mut a, mut b, mut c) = (store.subscribe(), store.subscribe(), store.subscribe());
        store.insert(make_msg("m")).await;
        let renders = std::sync::atomic::AtomicUsize::new(0);
        let render = |e: &StoreEvent| {
            renders.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match e {
                StoreEvent::NewMessage(s) => format!("new {}", s.id),
                _ => "other".into(),
            }
        };
        let (fa, fb, fc) = (
            a.recv().await.unwrap(),
            b.recv().await.unwrap(),
            c.recv().await.unwrap(),
        );
        let ta = fa.cached_json(render).to_string();
        let tb = fb.cached_json(render);
        let tc = fc.cached_json(render);
        assert_eq!(renders.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(ta, "new m");
        assert!(
            std::ptr::eq(tb, tc),
            "one shared string, not a copy per subscriber"
        );
        assert!(Arc::ptr_eq(&fa, &fb), "one frame per event");
    }

    /// Message whose raw payload is `kb` KiB — dominates `estimated_bytes()`.
    fn make_big_msg(id: &str, kb: usize) -> Hl7Message {
        let mut msg = Hl7Message::new_empty("x".repeat(kb * 1024), "127.0.0.1:5000".into());
        msg.id = id.to_string();
        msg
    }

    impl MessageStore {
        async fn current_bytes(&self) -> usize {
            self.inner.read().await.current_bytes
        }

        /// Wait until the store is within its limits again (#191) — used to
        /// wait for the **background** eviction task without sleeping.
        ///
        /// Subscribes to the store's broadcast channel *before* checking the
        /// state; every eviction pass that evicts anything sends
        /// `StoreEvent::Evicted` under the write lock, and any event (or
        /// `Lagged`) just triggers a re-check, so a pass finishing between
        /// check and wait can't be missed. No production code exists only
        /// for this helper.
        ///
        /// Caveats: it returns immediately if the store is already within its
        /// limits (it does not prove that a pass ran), and it needs a running
        /// background task — on a `without_eviction_task` store that is over
        /// its limit it times out after 5 s.
        async fn wait_until_within_limits(&self) {
            let mut rx = self.subscribe();
            let wait = async {
                loop {
                    if !self.inner.read().await.over_limit() {
                        return;
                    }
                    let _ = rx.recv().await;
                }
            };
            tokio::time::timeout(std::time::Duration::from_secs(5), wait)
                .await
                .expect("background eviction did not bring the store within limits");
        }

        /// Ground truth: payload + metadata bytes recomputed from scratch for
        /// every stored message (ignores the cached `StoredEntry::bytes`).
        async fn recomputed_bytes(&self) -> usize {
            let inner = self.inner.read().await;
            inner
                .messages
                .values()
                .map(|e| entry_bytes(&e.message, &e.meta))
                .sum()
        }

        async fn assert_bytes_in_sync(&self) {
            assert_eq!(
                self.current_bytes().await,
                self.recomputed_bytes().await,
                "current_bytes drifted from the sum of estimated_bytes()"
            );
            let inner = self.inner.read().await;
            for (id, e) in &inner.messages {
                assert_eq!(
                    e.bytes,
                    entry_bytes(&e.message, &e.meta),
                    "cached entry bytes drifted for {id}"
                );
            }
            let (count, bytes) = inner
                .messages
                .values()
                .filter(|e| e.meta.bookmarked)
                .fold((0, 0), |(c, b), e| (c + 1, b + e.bytes));
            assert_eq!(inner.bookmark_count, count, "bookmark_count drifted");
            assert_eq!(inner.bookmark_bytes, bytes, "bookmark_bytes drifted");
        }
    }

    const MB: usize = 1024 * 1024;

    #[tokio::test]
    async fn test_toggle_bookmark_on_off() {
        let store = make_store(100);
        let msg = make_msg("a");
        store.insert(msg).await;

        // Toggle on
        let result = store.toggle_bookmark("a").await;
        assert_eq!(result, Some(true));
        let fetched = store.get_by_id("a").await.unwrap();
        assert!(fetched.meta.bookmarked);

        // Toggle off
        let result = store.toggle_bookmark("a").await;
        assert_eq!(result, Some(false));
        let fetched = store.get_by_id("a").await.unwrap();
        assert!(!fetched.meta.bookmarked);
    }

    #[tokio::test]
    async fn test_toggle_bookmark_not_found() {
        let store = make_store(100);
        assert_eq!(store.toggle_bookmark("nonexistent").await, None);
    }

    #[tokio::test]
    async fn test_toggle_bookmark_broadcasts_event() {
        let store = make_store(100);
        let msg = make_msg("b");
        store.insert(msg).await;

        let mut rx = store.subscribe();
        store.toggle_bookmark("b").await;

        let event = rx.recv().await.unwrap();
        match &event.event {
            StoreEvent::BookmarkToggled(summary) => {
                assert_eq!(summary.id, "b");
                assert!(summary.bookmarked);
            }
            _ => panic!("Expected BookmarkToggled event"),
        }
    }

    #[tokio::test]
    async fn test_bookmarked_message_survives_eviction() {
        let store = manual_store(10, 512);

        for i in 0..10 {
            store.insert(make_msg(&format!("msg-{i}"))).await;
        }
        store.toggle_bookmark("msg-0").await;
        store.insert(make_msg("trigger")).await;

        // 11 messages, count limit 10 → evict down to the low watermark (9).
        let outcome = store.evict_now().await;
        assert_eq!(outcome.evicted, 2);
        assert_eq!(outcome.evicted_bookmarks, 0);
        assert_eq!(store.count().await, 9);

        // Bookmarked message survives; the oldest non-bookmarked ones go.
        assert!(store.get_by_id("msg-0").await.is_some());
        assert!(store.get_by_id("msg-1").await.is_none());
        assert!(store.get_by_id("msg-2").await.is_none());
        assert!(store.get_by_id("msg-3").await.is_some());
    }

    #[tokio::test]
    async fn test_eviction_skips_scattered_bookmarks() {
        let store = manual_store(20, 512);

        for i in 0..20 {
            store.insert(make_msg(&format!("msg-{i}"))).await;
        }
        // Bookmark a few oldest messages in scattered positions
        store.toggle_bookmark("msg-0").await;
        store.toggle_bookmark("msg-2").await;
        store.toggle_bookmark("msg-4").await;
        store.insert(make_msg("trigger")).await;

        // 21 messages, count limit 20 → evict down to 18 (3 non-bookmarked).
        let outcome = store.evict_now().await;
        assert_eq!(outcome.evicted, 3);
        assert_eq!(store.count().await, 18);

        // All three bookmarked messages must survive
        assert!(store.get_by_id("msg-0").await.is_some());
        assert!(store.get_by_id("msg-2").await.is_some());
        assert!(store.get_by_id("msg-4").await.is_some());
        // The three oldest non-bookmarked (msg-1, msg-3, msg-5) are evicted
        assert!(store.get_by_id("msg-1").await.is_none());
        assert!(store.get_by_id("msg-3").await.is_none());
        assert!(store.get_by_id("msg-5").await.is_none());
        assert!(store.get_by_id("msg-6").await.is_some());

        // Surviving bookmarks keep their original order at the front.
        let ids: Vec<String> = store
            .list_summaries(0, 100)
            .await
            .into_iter()
            .rev()
            .take(4)
            .map(|m| m.id)
            .collect();
        assert_eq!(ids, ["msg-0", "msg-2", "msg-4", "msg-6"]);
    }

    /// #179: tag mutations must be reflected in `current_bytes`.
    #[tokio::test]
    async fn test_tag_mutations_keep_byte_accounting_in_sync() {
        let store = manual_store(100, 512);
        store.insert(make_msg("a")).await;
        store.assert_bytes_in_sync().await;

        let before = store.current_bytes().await;
        assert!(store.add_tag("a", "Bug #1234".into()).await);
        assert!(store.add_tag("a", "Test scenario A".into()).await);
        store.assert_bytes_in_sync().await;
        // Two tag strings plus the tag list's buffer.
        assert!(
            store.current_bytes().await >= before + "Bug #1234".len() + "Test scenario A".len()
        );

        assert!(store.remove_tag("a", "Bug #1234").await);
        store.assert_bytes_in_sync().await;

        // Bookmark toggles leave the accounting untouched.
        let before = store.current_bytes().await;
        store.toggle_bookmark("a").await;
        store.assert_bytes_in_sync().await;
        assert_eq!(store.current_bytes().await, before);
        // Tagging a bookmarked message must move its bookmark bytes too.
        assert!(store.add_tag("a", "while-bookmarked".into()).await);
        store.assert_bytes_in_sync().await;
        assert!(store.remove_tag("a", "while-bookmarked").await);
        store.assert_bytes_in_sync().await;
        // No-op mutations leave it untouched.
        let before = store.current_bytes().await;
        assert!(!store.add_tag("a", "Test scenario A".into()).await);
        assert!(!store.remove_tag("a", "missing").await);
        assert_eq!(store.current_bytes().await, before);
    }

    /// #179: after tagging and evicting, nothing may be left over or missing
    /// in `current_bytes` — the old code subtracted the post-mutation size of
    /// tagged messages and drifted below the real total.
    #[tokio::test]
    async fn test_byte_accounting_survives_tagging_and_eviction() {
        let store = manual_store(10, 512);
        for i in 0..10 {
            store.insert(make_msg(&format!("msg-{i}"))).await;
            store
                .add_tag(&format!("msg-{i}"), format!("a-rather-long-tag-{i}"))
                .await;
        }
        store.toggle_bookmark("msg-0").await;
        store.insert(make_msg("trigger")).await;

        store.evict_now().await;
        store.assert_bytes_in_sync().await;

        store.clear().await;
        assert_eq!(store.current_bytes().await, 0);
    }

    /// Review of #200: evictions must reach the UI, otherwise evicted rows
    /// (including force-evicted bookmarks) linger and 404 on click.
    #[tokio::test]
    async fn test_eviction_broadcasts_evicted_ids() {
        let store = manual_store(10, 512);
        for i in 0..10 {
            store.insert(make_msg(&format!("msg-{i}"))).await;
        }
        for i in 0..6 {
            store.toggle_bookmark(&format!("msg-{i}")).await;
        }
        store.insert(make_msg("trigger")).await;

        let mut rx = store.subscribe();
        let outcome = store.evict_now().await;
        // Phase 1 drops the oldest bookmark (6 > budget 5) → 10 messages,
        // Phase 2 the oldest non-bookmark → 9 (watermark).
        assert_eq!(outcome.evicted_bookmarks, 1);
        assert_eq!(outcome.evicted, 1);
        assert_eq!(store.count().await, 9);

        match &rx.recv().await.unwrap().event {
            StoreEvent::Evicted(ids) => {
                assert_eq!(**ids, ["msg-0", "msg-6"]);
            }
            _ => panic!("Expected Evicted event"),
        }
        // No event when a pass has nothing to do.
        store.evict_now().await;
        assert!(rx.try_recv().is_err());
    }

    /// A single message larger than the whole byte limit cannot be kept —
    /// it is evicted on the next pass (and logged with a dedicated warning).
    #[tokio::test]
    async fn test_single_oversize_message_is_dropped() {
        let store = manual_store(10_000, 1);
        store.insert(make_big_msg("huge", 2048)).await;

        let outcome = store.evict_now().await;
        assert_eq!(outcome.ids, ["huge"]);
        assert_eq!(store.count().await, 0);
        assert_eq!(store.current_bytes().await, 0);
    }

    #[tokio::test]
    async fn test_no_eviction_below_limits() {
        let store = manual_store(10, 512);
        for i in 0..9 {
            store.insert(make_msg(&format!("msg-{i}"))).await;
        }
        assert_eq!(store.evict_now().await, EvictionOutcome::default());
        assert_eq!(store.count().await, 9);
    }

    /// #178 gap 2: fewer than 10 huge messages used to yield `target_count == 0`
    /// and skip eviction entirely, leaving the store far above its byte limit.
    #[tokio::test]
    async fn test_few_large_messages_over_byte_limit_are_evicted() {
        let store = manual_store(10_000, 1);
        for i in 0..3 {
            store.insert(make_big_msg(&format!("big-{i}"), 600)).await;
        }
        assert!(store.current_bytes().await > MB);

        let outcome = store.evict_now().await;
        assert_eq!(outcome.evicted, 2);
        assert!(store.current_bytes().await <= low_watermark(MB));
        // Newest message stays.
        assert!(store.get_by_id("big-2").await.is_some());
    }

    /// #178 gap 3: eviction must target bytes, not a fixed 10 % of the count.
    #[tokio::test]
    async fn test_byte_eviction_reaches_low_watermark() {
        let store = manual_store(10_000, 1);
        for i in 0..30 {
            store.insert(make_big_msg(&format!("m-{i}"), 100)).await;
        }
        // ~3 MB in a 1 MB store — 10 % of the count (3 messages) is not enough.
        store.evict_now().await;
        let bytes = store.current_bytes().await;
        assert!(
            bytes <= low_watermark(MB),
            "store still holds {bytes} bytes after eviction"
        );
        assert_eq!(store.count().await, 9);
        assert!(store.get_by_id("m-29").await.is_some());
        assert!(store.get_by_id("m-20").await.is_none());
    }

    /// #178 gap 1: bookmarking large messages must not lift the memory cap.
    #[tokio::test]
    async fn test_bookmarks_cannot_lift_memory_cap() {
        let store = manual_store(10_000, 1);
        for i in 0..4 {
            store.insert(make_big_msg(&format!("bm-{i}"), 300)).await;
            store.toggle_bookmark(&format!("bm-{i}")).await;
        }
        // 1.2 MB of bookmarks in a 1 MB store with a 50 % bookmark budget.
        store.assert_bytes_in_sync().await;
        let outcome = store.evict_now().await;
        store.assert_bytes_in_sync().await;
        assert_eq!(outcome.evicted, 0);
        assert_eq!(outcome.evicted_bookmarks, 3);
        assert!(store.current_bytes().await <= MB / 2);
        // The oldest bookmarks go first; the newest one survives.
        assert!(store.get_by_id("bm-0").await.is_none());
        assert!(store.get_by_id("bm-2").await.is_none());
        assert!(store.get_by_id("bm-3").await.is_some());
    }

    #[tokio::test]
    async fn test_bookmarks_within_budget_are_protected() {
        let store = manual_store(10_000, 1);
        store.insert(make_big_msg("bm-a", 100)).await;
        store.insert(make_big_msg("bm-b", 100)).await;
        store.toggle_bookmark("bm-a").await;
        store.toggle_bookmark("bm-b").await;
        for i in 0..12 {
            store.insert(make_big_msg(&format!("m-{i}"), 100)).await;
        }

        let outcome = store.evict_now().await;
        assert_eq!(outcome.evicted_bookmarks, 0);
        assert!(store.current_bytes().await <= low_watermark(MB));
        assert!(store.get_by_id("bm-a").await.is_some());
        assert!(store.get_by_id("bm-b").await.is_some());
    }

    #[tokio::test]
    async fn test_bookmark_count_budget_enforced() {
        // Count limit 10, 50 % budget → at most 5 bookmarks under pressure.
        let store = manual_store(10, 512);
        for i in 0..10 {
            store.insert(make_msg(&format!("msg-{i}"))).await;
        }
        for i in 0..8 {
            store.toggle_bookmark(&format!("msg-{i}")).await;
        }

        let outcome = store.evict_now().await;
        store.assert_bytes_in_sync().await;
        assert_eq!(outcome.evicted_bookmarks, 3);
        assert!(store.count().await <= 9);
        for i in 0..3 {
            assert!(store.get_by_id(&format!("msg-{i}")).await.is_none());
        }
        for i in 3..8 {
            assert!(store.get_by_id(&format!("msg-{i}")).await.is_some());
        }
    }

    /// #226: an entry is accounted for at least its payload, its field
    /// structs and the store's own per-entry structures.
    #[tokio::test]
    async fn test_entry_accounting_covers_structs_and_overhead() {
        let store = manual_store(100, 512);
        let msg = crate::hl7::parser::parse_message(
            "MSH|^~\\&|APP|FAC|APP|FAC|20240101||ADT^A01|MSG001|P|2.5\rPID|||12345||Smith^John||1980|M",
            "127.0.0.1:5000",
        )
        .unwrap();
        let fields: usize = msg.segments.iter().map(|s| s.fields.len()).sum();
        let floor = msg.raw.len()
            + fields * std::mem::size_of::<crate::hl7::types::Hl7Field>()
            + std::mem::size_of::<Hl7Message>();
        store.insert(msg).await;
        assert!(store.current_bytes().await >= floor);
        assert_eq!(store.memory_usage().await.0, store.current_bytes().await);
        store.assert_bytes_in_sync().await;
    }

    /// #231: older bookmarks come with the newest page, in store order.
    #[tokio::test]
    async fn test_list_with_bookmarks_adds_older_bookmarks() {
        let store = manual_store(100, 512);
        for i in 0..10 {
            store.insert(make_msg(&format!("m{i}"))).await;
        }
        store.toggle_bookmark("m1").await;
        store.toggle_bookmark("m8").await; // inside the page
        let ids = |v: Vec<Hl7MessageSummary>| v.into_iter().map(|s| s.id).collect::<Vec<_>>();
        assert_eq!(
            ids(store.list_summaries_with_bookmarks(0, 3).await),
            ["m9", "m8", "m7", "m1"]
        );
        // Without bookmarks beyond the page it is the plain page.
        assert_eq!(
            ids(store.list_summaries_with_bookmarks(0, 3).await)[..3],
            ids(store.list_summaries(0, 3).await)[..]
        );
        // An offset skips newer bookmarks too.
        assert_eq!(
            ids(store.list_summaries_with_bookmarks(5, 2).await),
            ["m4", "m3", "m1"]
        );
    }

    /// #232: two users tag the same message at the same time; the last
    /// `TagsUpdated` a subscriber sees must be the store's final state.
    ///
    /// A smoke test, not a reliable guard: with the send after the unlock,
    /// the window between the two is a few hundred nanoseconds, so whether
    /// the old code fails here depends on the machine (49 of 5,000 rounds on
    /// one, none in 20,000 on another). The ordering itself is guaranteed by
    /// the rule that every `tx.send` happens under the write lock, and
    /// checked more reliably by `test_insert_event_precedes_its_eviction`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_concurrent_tagging_events_match_store() {
        for round in 0..1000 {
            let store = manual_store(100, 512);
            store.insert(make_msg("m")).await;
            let mut rx = store.subscribe();
            let (s1, s2) = (store.clone(), store.clone());
            let a = tokio::spawn(async move { s1.add_tag("m", "alpha".into()).await });
            let b = tokio::spawn(async move { s2.add_tag("m", "beta".into()).await });
            a.await.unwrap();
            b.await.unwrap();
            let mut last = None;
            while let Ok(ev) = rx.try_recv() {
                if let StoreEvent::TagsUpdated(s) = &ev.event {
                    last = Some(s.tags.clone());
                }
            }
            let actual = store.get_by_id("m").await.unwrap().meta.tags;
            assert_eq!(last.as_ref(), Some(&actual), "round {round}");
        }
    }

    /// #232: a message larger than the whole store is inserted and evicted
    /// by the background task. Subscribers must see `NewMessage` before
    /// `Evicted`, otherwise the UI keeps a row for a message that no longer
    /// exists. With the old order (unlock → wake eviction → send) this
    /// failed probabilistically: at round 6 on one machine, round 890 on
    /// another.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_insert_event_precedes_its_eviction() {
        for round in 0..2000 {
            let store = MessageStore::new(StoreConfig {
                max_messages: 10_000,
                max_memory_mb: 1,
                ..StoreConfig::default()
            });
            let mut rx = store.subscribe();
            store
                .insert(make_big_msg(&format!("big-{round}"), 2048))
                .await;
            store.wait_until_within_limits().await;
            let mut order = Vec::new();
            while let Ok(ev) = rx.try_recv() {
                match ev.event {
                    StoreEvent::NewMessage(_) => order.push('N'),
                    StoreEvent::Evicted(_) => order.push('E'),
                    _ => {}
                }
            }
            assert_eq!(order, ['N', 'E'], "round {round}");
        }
    }

    #[test]
    fn test_estimated_bytes_exceeds_raw_len() {
        let msg = crate::hl7::parser::parse_message(
            "MSH|^~\\&|APP|FAC|APP|FAC|20240101||ADT^A01|MSG001|P|2.5\rPID|||12345||Smith^John||1980|M\rPV1||I",
            "127.0.0.1:5000",
        )
        .unwrap();
        assert!(
            msg.estimated_bytes() > msg.raw.len(),
            "estimated_bytes ({}) should be larger than raw.len() ({}) because parsed strings duplicate content",
            msg.estimated_bytes(),
            msg.raw.len()
        );
    }

    // multi_thread: the background eviction task runs truly concurrently with
    // the test, so the wait helper's check/wait window is actually exercised
    // (on current_thread the task can only run while the test is parked).
    #[tokio::test(flavor = "multi_thread")]
    async fn test_byte_based_eviction_triggers() {
        // Store with a very small byte limit so a single large message overflows it
        let store = MessageStore::new(StoreConfig {
            max_messages: 10_000,
            max_memory_mb: 1, // 1 MB limit
            ..StoreConfig::default()
        });

        // Insert enough small messages to exceed 1 MB total
        let payload = "x".repeat(200_000); // 200 KB each
        for i in 0..10 {
            let mut msg = Hl7Message::new_empty(payload.clone(), "127.0.0.1:5000".into());
            msg.id = format!("big-{i}");
            store.insert(msg).await;
        }

        // Wait for the real background task (not a fixed sleep — #191).
        store.wait_until_within_limits().await;

        // The background task must have brought the store back under its limit
        assert!(
            store.count().await < 10,
            "Expected byte-based eviction to reduce message count below 10"
        );
        assert!(store.current_bytes().await < MB);
    }

    #[tokio::test]
    async fn test_get_by_id_returns_shared_snapshot() {
        let store = make_store(10);
        store.insert(make_msg("shared")).await;

        let a = store.get_by_id("shared").await.unwrap();
        let b = store.get_by_id("shared").await.unwrap();

        // Both snapshots point at the same Arc (no deep clone).
        assert!(Arc::ptr_eq(&a.message, &b.message));
        assert_eq!(a.message.id, "shared");
    }

    /// #192: tagging / bookmarking while a reader holds a snapshot must not
    /// clone the payload. Before the payload/metadata split, `Arc::make_mut`
    /// deep-cloned the whole (potentially multi-MB) message under the write
    /// lock whenever a snapshot was alive.
    #[tokio::test]
    async fn test_meta_mutation_does_not_clone_payload() {
        let store = manual_store(10, 512);
        store.insert(make_big_msg("mdm", 512)).await;

        let held = store.get_by_id("mdm").await.unwrap(); // reader snapshot alive
        assert!(store.add_tag("mdm", "Bug #1234".into()).await);
        assert_eq!(store.toggle_bookmark("mdm").await, Some(true));

        let now = store.get_by_id("mdm").await.unwrap();
        // Same payload allocation — no copy-on-write happened.
        assert!(Arc::ptr_eq(&held.message, &now.message));
        // The held snapshot keeps the metadata it was taken with.
        assert!(held.meta.tags.is_empty() && !held.meta.bookmarked);
        assert_eq!(now.meta.tags, ["Bug #1234"]);
        assert!(now.meta.bookmarked);
        store.assert_bytes_in_sync().await;
    }

    /// #195: the export snapshot shares the stored payloads (no copy of the
    /// raw bytes) and lists them oldest first.
    #[tokio::test]
    async fn test_payload_snapshot_shares_payloads_in_order() {
        let store = manual_store(10, 512);
        for i in 0..3 {
            store.insert(make_msg(&format!("p-{i}"))).await;
        }
        let snap = store.payload_snapshot(None).await;
        let ids: Vec<&str> = snap.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["p-0", "p-1", "p-2"]);
        let detail = store.get_by_id("p-1").await.unwrap();
        assert!(Arc::ptr_eq(&snap[1], &detail.message));
    }

    /// The detail JSON keeps its shape: payload fields and `tags` /
    /// `bookmarked` in one flat object, as the frontend expects.
    #[tokio::test]
    async fn test_snapshot_serializes_flat() {
        let store = manual_store(10, 512);
        store.insert(make_msg("j")).await;
        store.add_tag("j", "t1".into()).await;
        store.toggle_bookmark("j").await;

        let v = serde_json::to_value(store.get_by_id("j").await.unwrap()).unwrap();
        assert_eq!(v["id"], "j");
        assert_eq!(v["raw"], "raw-j");
        assert_eq!(v["tags"], serde_json::json!(["t1"]));
        assert_eq!(v["bookmarked"], true);
        assert!(v.get("message").is_none() && v.get("meta").is_none());
    }

    // --- Sent messages (#272) ---

    fn make_sent(id: &str, target: &str) -> Hl7Message {
        use crate::hl7::types::{Origin, Outbound};
        let mut msg = make_msg(id);
        msg.source_addr = "10.9.9.9:6661".into();
        msg.outbound = Some(Box::new(Outbound {
            target: target.into(),
            transport: crate::hl7::types::Transport::Plain,
            origin: Origin::Resend {
                message_id: "orig".into(),
                changes: Vec::new(),
            },
            requested_by: "10.0.0.7".into(),
            run: "run-1".into(),
        }));
        msg
    }

    fn acked(response: &str) -> Delivery {
        Delivery {
            state: crate::hl7::types::DeliveryState::Acked,
            ack_code: Some("AA".into()),
            response: Some(response.into()),
            latency_ms: Some(12),
            control_id_matches: Some(true),
            ..Delivery::pending()
        }
    }

    /// A sent message is inserted pending; its outcome arrives as
    /// `delivery_updated`, in store order, and the summary never carries the
    /// response text — only the detail does.
    #[tokio::test]
    async fn test_sent_message_delivery_flow() {
        let store = manual_store(100, 512);
        let mut rx = store.subscribe();
        let sent = store.insert_sent(make_sent("s1", "orchestra-test")).await;
        assert_eq!(sent.id, "s1");
        let response = "MSH|^~\\&|R|RF|||20260101||ACK|X|P|2.5\rMSA|AA|C1\rSECRET-RESPONSE";
        assert!(store.set_delivery("s1", acked(response)).await);

        let first = rx.recv().await.unwrap();
        let StoreEvent::NewMessage(s) = &first.event else {
            panic!("expected new_message first")
        };
        assert_eq!(s.direction, Direction::Out);
        assert_eq!(s.target.as_deref(), Some("orchestra-test"));
        assert_eq!(s.origin_id.as_deref(), Some("orig"));
        assert_eq!(
            s.delivery.as_ref().unwrap().state,
            crate::hl7::types::DeliveryState::Pending
        );
        let second = rx.recv().await.unwrap();
        let json = second.cached_json(crate::web::ws_event_json).to_string();
        assert!(json.starts_with(r#"{"type":"delivery_updated""#), "{json}");
        assert!(json.contains(r#""ack_code":"AA""#), "{json}");
        assert!(
            !json.contains("SECRET-RESPONSE"),
            "no response text in a summary"
        );
        let listed = serde_json::to_string(&store.list_summaries(0, 10).await).unwrap();
        assert!(!listed.contains("SECRET-RESPONSE"));

        let detail = serde_json::to_value(store.get_by_id("s1").await.unwrap()).unwrap();
        assert_eq!(detail["direction"], "out");
        assert_eq!(detail["delivery"]["response"], response);
        assert_eq!(detail["outbound"]["target"], "orchestra-test");
        assert_eq!(detail["outbound"]["origin"]["kind"], "resend");
        assert!(
            detail["ack_code"].is_null(),
            "ack_code stays what Harux wrote back"
        );
        store.assert_bytes_in_sync().await;
    }

    /// A response grows an entry: the bytes stay exact and the eviction task
    /// is woken without waiting for the next insert.
    #[tokio::test]
    async fn test_delivery_bytes_and_eviction_wakeup() {
        let store = make_store(10_000);
        {
            let mut inner = store.inner.write().await;
            inner.max_bytes = 200 * 1024;
        }
        store.insert(make_big_msg("old", 60)).await;
        store.insert_sent(make_sent("s", "t")).await;
        store.assert_bytes_in_sync().await;
        assert!(!store.inner.read().await.over_limit());
        // A 150 KB response puts the store over its limit.
        assert!(
            store
                .set_delivery("s", acked(&"x".repeat(150 * 1024)))
                .await
        );
        store.wait_until_within_limits().await;
        store.assert_bytes_in_sync().await;
        assert!(store.get_by_id("old").await.is_none(), "oldest evicted");
    }

    /// The outcome of a message evicted or cleared before it arrived is a
    /// no-op.
    #[tokio::test]
    async fn test_delivery_for_a_message_that_is_gone() {
        let store = manual_store(100, 512);
        store.insert_sent(make_sent("s", "t")).await;
        store.clear().await;
        assert!(!store.set_delivery("s", acked("x")).await);
        assert_eq!(store.current_bytes().await, 0);
    }

    #[tokio::test]
    async fn test_search_and_export_by_direction() {
        let store = manual_store(100, 512);
        store.insert(make_msg("in1")).await;
        store
            .insert_sent(make_sent("out1", "Orchestra-Test-ADT"))
            .await;
        let ids = |v: Vec<Hl7MessageSummary>| v.into_iter().map(|s| s.id).collect::<Vec<_>>();
        let search = |q: &str| SearchQuery::parse(q);
        assert_eq!(
            ids(store.older_than(None, Some(&search("is:sent")), 10).await),
            ["out1"]
        );
        assert_eq!(
            ids(store
                .older_than(None, Some(&search("is:received")), 10)
                .await),
            ["in1"]
        );
        // The target's name and the destination are a sent message's source.
        assert_eq!(
            ids(store
                .older_than(None, Some(&search("orchestra-test")), 10)
                .await),
            ["out1"]
        );
        assert_eq!(
            ids(store.older_than(None, Some(&search("10.9.9.9")), 10).await),
            ["out1"]
        );
        // Prefixes in any order.
        assert_eq!(
            search("has:warnings is:sent foo"),
            search("is:sent has:warnings foo")
        );
        assert!(store
            .older_than(None, Some(&search("is:sent has:warnings")), 10)
            .await
            .is_empty());

        for (d, expect) in [
            (Some(Direction::In), vec!["in1"]),
            (Some(Direction::Out), vec!["out1"]),
            (None, vec!["in1", "out1"]),
        ] {
            let got: Vec<String> = store
                .payload_snapshot(d)
                .await
                .iter()
                .map(|m| m.id.clone())
                .collect();
            assert_eq!(got, expect, "{d:?}");
        }
    }
}
