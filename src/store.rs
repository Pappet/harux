use crate::config::StoreConfig;
use crate::hl7::types::{Hl7Message, Hl7MessageSummary};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{info, warn};

const BROADCAST_CAPACITY: usize = 4096;

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
    /// Unix-seconds timestamp of the last "all bookmarked" eviction warning (rate-limit to 30 s)
    last_all_bookmarked_warn: Arc<std::sync::atomic::AtomicI64>,
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
}

impl MessageStore {
    /// Create a new MessageStore.
    ///
    /// Must be called from within a Tokio runtime — spawns a background eviction task.
    pub fn new(config: StoreConfig) -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let store = Self {
            inner: Arc::new(RwLock::new(StoreInner {
                order: VecDeque::with_capacity(1024),
                messages: HashMap::with_capacity(1024),
                capacity: config.max_messages,
                max_bytes: config.max_memory_bytes(),
                current_bytes: 0,
            })),
            tx,
            eviction_notify: Arc::new(tokio::sync::Notify::new()),
            eviction_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            last_all_bookmarked_warn: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        };

        // Spawn background eviction task
        let store_clone = store.clone();
        tokio::spawn(async move {
            store_clone.run_eviction_loop().await;
        });

        store
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
        let needs_eviction = inner.current_bytes >= inner.max_bytes || count >= inner.capacity;
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
        let query_lower = query.to_lowercase();
        let inner = self.inner.read().await;
        inner
            .order
            .iter()
            .rev()
            .filter_map(|id| inner.messages.get(id))
            .filter(|m| {
                m.message_type.to_lowercase().contains(&query_lower)
                    || m.sending_facility.to_lowercase().contains(&query_lower)
                    || m.patient_name
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query_lower)
                    || m.patient_id
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query_lower)
                    || m.message_control_id.to_lowercase().contains(&query_lower)
                    || m.source_addr.contains(&query_lower)
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

            let mut inner = self.inner.write().await;

            // Evict oldest 10% when either size or count limit is breached.
            // Bookmarked messages are protected — they are popped off the front
            // alongside eviction candidates and pushed back to the front in their
            // original relative order. Single linear pass.
            if inner.current_bytes >= inner.max_bytes || inner.order.len() >= inner.capacity {
                let target_count = inner.order.len() / 10;
                if target_count == 0 {
                    // Single message over limit — nothing meaningful to evict.
                    continue;
                }

                let mut kept_bookmarks: Vec<String> = Vec::new();
                let mut freed_bytes: usize = 0;
                let mut evicted: usize = 0;

                while evicted < target_count {
                    let id = match inner.order.pop_front() {
                        Some(id) => id,
                        None => break,
                    };
                    let bookmarked = inner
                        .messages
                        .get(&id)
                        .map(|m| m.bookmarked)
                        .unwrap_or(false);
                    if bookmarked {
                        kept_bookmarks.push(id);
                    } else if let Some(removed) = inner.messages.remove(&id) {
                        freed_bytes += removed.estimated_bytes();
                        evicted += 1;
                    }
                }

                // Restore bookmarked IDs at the front in original order.
                for id in kept_bookmarks.into_iter().rev() {
                    inner.order.push_front(id);
                }

                if evicted == 0 {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;
                    let last = self
                        .last_all_bookmarked_warn
                        .load(std::sync::atomic::Ordering::Relaxed);
                    if now - last >= 30 {
                        warn!(
                            "Eviction triggered but all candidate messages are bookmarked \
                             — skipping eviction (suppressed for 30 s)"
                        );
                        self.last_all_bookmarked_warn
                            .store(now, std::sync::atomic::Ordering::Relaxed);
                    }
                } else {
                    inner.current_bytes = inner.current_bytes.saturating_sub(freed_bytes);
                    info!(
                        "Evicted {} messages from store ({} MB freed, store now {} messages / {} MB)",
                        evicted,
                        freed_bytes / 1024 / 1024,
                        inner.order.len(),
                        inner.current_bytes / 1024 / 1024,
                    );
                }
            }
        }
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
        })
    }

    fn make_msg(id: &str) -> Hl7Message {
        let mut msg = Hl7Message::new_empty(format!("raw-{id}"), "127.0.0.1:5000".into());
        msg.id = id.to_string();
        msg
    }

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
        let store = make_store(10);

        // Insert 10 messages, bookmark the first one
        for i in 0..10 {
            let msg = make_msg(&format!("msg-{i}"));
            store.insert(msg).await;
        }
        store.toggle_bookmark("msg-0").await;

        // Insert one more to trigger eviction (10% = 1 message evicted)
        let msg = make_msg("trigger");
        store.insert(msg).await;

        // Give background eviction task time to run
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Bookmarked message should survive
        assert!(store.get_by_id("msg-0").await.is_some());
        // The first non-bookmarked message should be evicted
        assert!(store.get_by_id("msg-1").await.is_none());
    }

    #[tokio::test]
    async fn test_eviction_skips_scattered_bookmarks() {
        let store = make_store(20);

        for i in 0..20 {
            let msg = make_msg(&format!("msg-{i}"));
            store.insert(msg).await;
        }
        // Bookmark a few oldest messages in scattered positions
        store.toggle_bookmark("msg-0").await;
        store.toggle_bookmark("msg-2").await;
        store.toggle_bookmark("msg-4").await;

        // Trigger eviction: 10% of 20 = 2 non-bookmarked messages to evict
        store.insert(make_msg("trigger")).await;

        // Give background eviction task time to run
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // All three bookmarked messages must survive
        assert!(store.get_by_id("msg-0").await.is_some());
        assert!(store.get_by_id("msg-2").await.is_some());
        assert!(store.get_by_id("msg-4").await.is_some());
        // The two oldest non-bookmarked (msg-1, msg-3) should be evicted
        assert!(store.get_by_id("msg-1").await.is_none());
        assert!(store.get_by_id("msg-3").await.is_none());
        // msg-5 (next non-bookmarked candidate) should still be there
        assert!(store.get_by_id("msg-5").await.is_some());
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

        // Store should have fewer than 10 messages after eviction
        assert!(
            store.count().await < 10,
            "Expected byte-based eviction to reduce message count below 10"
        );
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
