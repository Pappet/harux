use crate::config::StoreConfig;
use crate::hl7::types::{Hl7Message, Hl7MessageSummary};
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

// Messages are addressed by ID through `messages` (O(1) lookup) and ordered
// for iteration / eviction through `order`. Arc-wrapping lets `get_by_id`
// hand out a cheap reference-counted snapshot instead of cloning the whole
// message (raw + parsed segments + validation warnings can be multi-MB for
// MDM payloads). Mutations (tags, bookmark) use `Arc::make_mut` — copy-on-
// write that only allocates when a reader holds a concurrent snapshot.
struct StoreInner {
    order: VecDeque<String>,
    messages: HashMap<String, Arc<Hl7Message>>,
    capacity: usize,
    max_bytes: usize,
    current_bytes: usize,
    /// Max number of bookmarked messages kept under eviction pressure.
    bookmark_budget_count: usize,
    /// Max estimated bytes of bookmarked messages kept under eviction pressure.
    bookmark_budget_bytes: usize,
}

/// Result of a single eviction pass (see [`StoreInner::evict`]).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct EvictionOutcome {
    /// Non-bookmarked messages evicted to reach the low watermark.
    evicted: usize,
    /// Bookmarked messages evicted because bookmarks exceeded their budget.
    evicted_bookmarks: usize,
    /// Estimated bytes released by this pass.
    freed_bytes: usize,
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

    /// Remove a message by ID and release its bytes from the accounting.
    /// Does not touch `order` — callers manage the queue themselves.
    fn remove_message(&mut self, id: &str) -> Option<usize> {
        let removed = self.messages.remove(id)?;
        let bytes = removed.estimated_bytes();
        self.current_bytes = self.current_bytes.saturating_sub(bytes);
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
    /// Both phases are a single linear walk over `order` with O(1) removals;
    /// surviving bookmarks keep their original relative order.
    fn evict(&mut self) -> EvictionOutcome {
        let mut outcome = EvictionOutcome::default();
        if !self.over_limit() {
            return outcome;
        }

        // Phase 1: enforce the bookmark budget (oldest bookmarks first).
        let (mut bm_count, mut bm_bytes) = self
            .messages
            .values()
            .filter(|m| m.bookmarked)
            .fold((0usize, 0usize), |(c, b), m| {
                (c + 1, b + m.estimated_bytes())
            });

        if bm_count > self.bookmark_budget_count || bm_bytes > self.bookmark_budget_bytes {
            let order = std::mem::take(&mut self.order);
            let mut remaining = VecDeque::with_capacity(order.len());
            for id in order {
                let over_budget =
                    bm_count > self.bookmark_budget_count || bm_bytes > self.bookmark_budget_bytes;
                let bookmarked = self.messages.get(&id).is_some_and(|m| m.bookmarked);
                if over_budget && bookmarked {
                    if let Some(bytes) = self.remove_message(&id) {
                        bm_count -= 1;
                        bm_bytes = bm_bytes.saturating_sub(bytes);
                        outcome.evicted_bookmarks += 1;
                        outcome.freed_bytes += bytes;
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
            match self.messages.get(&id).map(|m| m.bookmarked) {
                Some(true) => kept_bookmarks.push(id),
                Some(false) => {
                    if let Some(bytes) = self.remove_message(&id) {
                        outcome.evicted += 1;
                        outcome.freed_bytes += bytes;
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
                bookmark_budget_bytes: max_bytes / 100 * pct,
            })),
            tx,
            eviction_notify: Arc::new(tokio::sync::Notify::new()),
            eviction_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Insert a message and broadcast summary to all WebSocket subscribers
    pub async fn insert(&self, msg: Hl7Message) {
        let summary = Hl7MessageSummary::from(&msg);

        let mut inner = self.inner.write().await;

        let raw_len = msg.estimated_bytes();
        let id = msg.id.clone();
        inner.current_bytes += raw_len;
        inner.order.push_back(id.clone());
        inner.messages.insert(id, Arc::new(msg));
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
            .map(|arc| Hl7MessageSummary::from(arc.as_ref()))
            .collect()
    }

    /// Get a full message by ID. Returns an `Arc` snapshot — no deep clone.
    pub async fn get_by_id(&self, id: &str) -> Option<Arc<Hl7Message>> {
        self.inner.read().await.messages.get(id).cloned()
    }

    /// Snapshot of every stored message's raw HL7 payload, in insertion order
    /// (oldest first — so replaying the result preserves the original sequence).
    pub async fn list_all_raw(&self) -> Vec<String> {
        let inner = self.inner.read().await;
        inner
            .order
            .iter()
            .filter_map(|id| inner.messages.get(id))
            .map(|arc| arc.raw.clone())
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
            .filter(|m| {
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
            .map(|arc| Hl7MessageSummary::from(arc.as_ref()))
            .collect()
    }

    /// Total message count
    pub async fn count(&self) -> usize {
        self.inner.read().await.order.len()
    }

    /// Apply a mutation to a message by ID and broadcast the appropriate event.
    /// Returns the new summary if the message was found and the mutation ran, or None otherwise.
    async fn mutate_message(
        &self,
        id: &str,
        f: impl FnOnce(&mut Hl7Message) -> bool,
    ) -> Option<Hl7MessageSummary> {
        let mut inner = self.inner.write().await;
        let arc = inner.messages.get_mut(id)?;
        if f(Arc::make_mut(arc)) {
            let summary = Hl7MessageSummary::from(arc.as_ref());
            Some(summary)
        } else {
            None
        }
    }

    /// Add a tag to a message and broadcast the update
    pub async fn add_tag(&self, id: &str, tag: String) -> bool {
        let tag_clone = tag.clone();
        if let Some(summary) = self
            .mutate_message(id, |msg| {
                if msg.tags.contains(&tag_clone) {
                    return false;
                }
                msg.tags.push(tag_clone);
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
            .mutate_message(id, |msg| {
                if let Some(pos) = msg.tags.iter().position(|t| *t == tag) {
                    msg.tags.remove(pos);
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
            .mutate_message(id, |msg| {
                msg.bookmarked = !msg.bookmarked;
                new_state = msg.bookmarked;
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
                "Bookmarks exceeded their memory budget — evicted {} oldest bookmarked messages",
                outcome.evicted_bookmarks
            );
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
        assert!(fetched.bookmarked);

        // Toggle off
        let result = store.toggle_bookmark("a").await;
        assert_eq!(result, Some(false));
        let fetched = store.get_by_id("a").await.unwrap();
        assert!(!fetched.bookmarked);
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
        let outcome = store.evict_now().await;
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

    #[tokio::test]
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

        // Give eviction task time to run
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

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
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(a.id, "shared");
    }
}
