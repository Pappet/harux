use crate::config::StoreConfig;
use crate::hl7::types::{Hl7Message, Hl7MessageSummary, MessageMeta, MessageSnapshot};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{info, warn};

const BROADCAST_CAPACITY: usize = 4096;

/// Case-insensitive substring search.
///
/// Fast path: ASCII-only needle — byte-level windowed scan, zero allocation.
/// ASCII code points are all single bytes, so windows of byte-length(needle)
/// always align on character boundaries and `eq_ignore_ascii_case` is correct.
///
/// Slow path: non-ASCII needle (unusual for HL7 searches) — allocate and use
/// Unicode-aware `to_lowercase` so multi-byte code points fold correctly.
fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.is_ascii() {
        let needle = needle.as_bytes();
        return haystack
            .as_bytes()
            .windows(needle.len())
            .any(|w| w.eq_ignore_ascii_case(needle));
    }
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

#[derive(Clone)]
#[allow(clippy::large_enum_variant)]
pub enum StoreEvent {
    NewMessage(Box<Hl7MessageSummary>),
    TagsUpdated(Box<Hl7MessageSummary>),
    BookmarkToggled(Box<Hl7MessageSummary>),
    /// IDs removed by an eviction pass (oldest first). Shared via `Arc` so the
    /// broadcast clone per subscriber does not copy the ID list.
    Evicted(Arc<Vec<String>>),
    Cleared,
}

/// Thread-safe in-memory message store with broadcast notifications
#[derive(Clone)]
pub struct MessageStore {
    inner: Arc<RwLock<StoreInner>>,
    tx: broadcast::Sender<StoreEvent>,
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
        let meta = MessageMeta::default();
        let summary = Hl7MessageSummary::new(&msg, &meta);
        // Walk the parsed message once, outside the lock.
        let bytes = msg.estimated_bytes() + meta.estimated_bytes();
        let id = msg.id.clone();
        let entry = StoredEntry {
            message: Arc::new(msg),
            meta,
            bytes,
        };

        let mut inner = self.inner.write().await;
        inner.current_bytes += bytes;
        inner.order.push_back(id.clone());
        inner.messages.insert(id, entry);
        let count = inner.order.len();
        let needs_eviction = inner.over_limit();
        drop(inner);

        if needs_eviction
            // Ordering::Relaxed is sufficient because notify_one() handles the necessary memory barriers
            && !self
                .eviction_flag
                .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            self.eviction_notify.notify_one();
        }

        // Broadcast to WebSocket subscribers (ignore if no receivers)
        let _ = self.tx.send(StoreEvent::NewMessage(Box::new(summary)));

        if count % 1000 == 0 {
            info!("Store now holds {} messages", count);
        }
    }

    /// Get a broadcast receiver for real-time updates
    pub fn subscribe(&self) -> broadcast::Receiver<StoreEvent> {
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
    pub async fn payload_snapshot(&self) -> Vec<Arc<Hl7Message>> {
        let inner = self.inner.read().await;
        inner
            .order
            .iter()
            .filter_map(|id| inner.messages.get(id))
            .map(|e| Arc::clone(&e.message))
            .collect()
    }

    /// Search messages by filter text (matches message type, patient name, facility, etc.)
    pub async fn search(&self, query: &str, limit: usize) -> Vec<Hl7MessageSummary> {
        let inner = self.inner.read().await;
        inner
            .order
            .iter()
            .rev()
            .filter_map(|id| inner.messages.get(id))
            .filter(|e| {
                let m = &e.message;
                contains_ignore_ascii_case(&m.message_type, query)
                    || contains_ignore_ascii_case(&m.sending_facility, query)
                    || m.patient_name
                        .as_deref()
                        .is_some_and(|n| contains_ignore_ascii_case(n, query))
                    || m.patient_id
                        .as_deref()
                        .is_some_and(|n| contains_ignore_ascii_case(n, query))
                    || contains_ignore_ascii_case(&m.message_control_id, query)
                    || contains_ignore_ascii_case(&m.source_addr, query)
            })
            .take(limit)
            .map(|e| Hl7MessageSummary::new(&e.message, &e.meta))
            .collect()
    }

    /// Total message count
    pub async fn count(&self) -> usize {
        self.inner.read().await.order.len()
    }

    /// Apply a mutation to a message's metadata (tags, bookmark).
    /// Returns the new summary if the message was found and the mutation ran, or None otherwise.
    ///
    /// Only the small `MessageMeta` is touched — the `Arc`-shared payload is
    /// never mutated, so there is no copy-on-write deep clone under the write
    /// lock even while readers hold snapshots (#192). The entry's byte count
    /// is adjusted by the metadata delta so accounting stays exact (#179).
    async fn mutate_meta(
        &self,
        id: &str,
        f: impl FnOnce(&mut MessageMeta) -> bool,
    ) -> Option<Hl7MessageSummary> {
        let mut guard = self.inner.write().await;
        // Reborrow once so `messages` and the counters can be borrowed disjointly.
        let inner = &mut *guard;
        let entry = inner.messages.get_mut(id)?;

        let before = entry.bytes;
        let was_bookmarked = entry.meta.bookmarked;
        let meta_before = entry.meta.estimated_bytes();
        if !f(&mut entry.meta) {
            return None;
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

        Some(summary)
    }

    /// Add a tag to a message and broadcast the update
    pub async fn add_tag(&self, id: &str, tag: String) -> bool {
        let tag_clone = tag.clone();
        if let Some(summary) = self
            .mutate_meta(id, |meta| {
                if meta.tags.contains(&tag_clone) {
                    return false;
                }
                meta.tags.push(tag_clone);
                true
            })
            .await
        {
            let _ = self.tx.send(StoreEvent::TagsUpdated(Box::new(summary)));
            true
        } else {
            false
        }
    }

    /// Remove a tag from a message and broadcast the update
    pub async fn remove_tag(&self, id: &str, tag: &str) -> bool {
        let tag = tag.to_string();
        if let Some(summary) = self
            .mutate_meta(id, |meta| {
                if let Some(pos) = meta.tags.iter().position(|t| *t == tag) {
                    meta.tags.remove(pos);
                    true
                } else {
                    false
                }
            })
            .await
        {
            let _ = self.tx.send(StoreEvent::TagsUpdated(Box::new(summary)));
            true
        } else {
            false
        }
    }

    /// Toggle bookmark on a message, returns the new bookmark state or None if not found
    pub async fn toggle_bookmark(&self, id: &str) -> Option<bool> {
        let mut new_state = false;
        let summary = self
            .mutate_meta(id, |meta| {
                meta.bookmarked = !meta.bookmarked;
                new_state = meta.bookmarked;
                true
            })
            .await?;
        let _ = self.tx.send(StoreEvent::BookmarkToggled(Box::new(summary)));
        Some(new_state)
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
        let _ = self.tx.send(StoreEvent::Cleared);
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
            let _ = self
                .tx
                .send(StoreEvent::Evicted(Arc::new(outcome.ids.clone())));
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
                .map(|e| e.message.estimated_bytes() + e.meta.estimated_bytes())
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
                    e.message.estimated_bytes() + e.meta.estimated_bytes(),
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
        match event {
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
        assert_eq!(
            store.current_bytes().await,
            before + "Bug #1234".len() + "Test scenario A".len()
        );

        assert!(store.remove_tag("a", "Bug #1234").await);
        store.assert_bytes_in_sync().await;

        // Bookmark toggles and no-op mutations leave the accounting untouched.
        let before = store.current_bytes().await;
        store.toggle_bookmark("a").await;
        store.assert_bytes_in_sync().await;
        // Tagging a bookmarked message must move its bookmark bytes too.
        assert!(store.add_tag("a", "while-bookmarked".into()).await);
        store.assert_bytes_in_sync().await;
        assert!(store.remove_tag("a", "while-bookmarked").await);
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

        match rx.recv().await.unwrap() {
            StoreEvent::Evicted(ids) => {
                assert_eq!(*ids, ["msg-0", "msg-6"]);
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
        let snap = store.payload_snapshot().await;
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
}
