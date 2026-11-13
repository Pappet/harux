// WebSocket connection, server event dispatch, message ingestion, and
// stats polling. Owns the network boundary and pushes data into `state`,
// then delegates DOM updates to render.js.

const WS_RECONNECT_INITIAL = 1000;   // 1 second
const WS_RECONNECT_MAX = 60000;  // 60 seconds
const WS_RECONNECT_MULT = 2;     // double each time

function setListeningPillState(stateName, tooltip) {
    const pill = document.getElementById('pill-listening');
    if (!pill) return;
    pill.classList.remove('live', 'disconnected');
    pill.classList.add(stateName);
    pill.title = tooltip || (stateName === 'live' ? 'MLLP listener health' : 'WebSocket disconnected — reconnecting');
}

function connectWs() {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    state.ws = new WebSocket(`${proto}//${location.host}/ws`);

    state.ws.onopen = () => {
        state.wsReconnectDelay = WS_RECONNECT_INITIAL; // reset on success
        setListeningPillState('live');
    };

    state.ws.onclose = () => {
        const jitter = state.wsReconnectDelay * (0.75 + Math.random() * 0.5);
        const delaySec = Math.round(jitter / 1000);
        setListeningPillState('disconnected', `Reconnecting in ${delaySec}s…`);
        setTimeout(connectWs, jitter);
        state.wsReconnectDelay = Math.min(state.wsReconnectDelay * WS_RECONNECT_MULT, WS_RECONNECT_MAX);
    };

    state.ws.onerror = (event) => {
        console.error('WebSocket error:', event);
        setListeningPillState('disconnected', 'WebSocket error');
    };

    state.ws.onmessage = (event) => {
        handleServerEvent(JSON.parse(event.data));
    };
}

// Apply one server event. While a list (re)load is in flight, everything but
// `init` / `lagged` (which start a load themselves) is queued and replayed
// after the snapshot lands (#239) — applied now, it would be overwritten by
// the older snapshot. `known` (replay only) holds the IDs already present,
// so a message that is both in the snapshot and in the queue isn't doubled.
function handleServerEvent(data, known = null) {
    if (data.type === 'init') {
        state.totalMessagesCount = data.total;
        loadMessages();
        return;
    }
    if (data.type === 'lagged') {
        console.warn(`Missed ${data.missed} messages, reloading...`);
        loadMessages();
        return;
    }
    if (state.loadInProgress && !known) {
        state.loadQueue.push(data);
        return;
    }
    if (data.type === 'new_message') {
        if (known) {
            if (known.has(data.data.id)) return;
            known.add(data.data.id);
        }
        addMessage(data.data);
    } else if (data.type === 'tags_updated') {
        updateMessageTags(data.data);
    } else if (data.type === 'bookmark_toggled') {
        updateMessageBookmark(data.data);
    } else if (data.type === 'delivery_updated') {
        updateMessageDelivery(data.data);
    } else if (data.type === 'evicted') {
        removeEvictedMessages(data.data);
    } else if (data.type === 'cleared') {
        if (known) known.clear();
        applyCleared();
    } else {
        console.debug('Ignoring unknown server event type', data.type);
    }
}

function applyCleared() {
    console.info("Server cleared messages via Web UI or API");
    state.messages = [];
    resetOlderHistory();
    state.pendingMessages = [];
    state.sourceCounts.clear();
    state.seenSources.clear();
    state.diffPinnedMessage = null;
    state.totalMessagesCount = 0;
    state.totalValidationCount = 0;
    state.totalBookmarkCount = 0;
    state.rateWindow.length = 0;
    state.lastMessageReceivedAt = null;
    state.selectedId = null; // never closes an open draft (#277)
    state.selectedMessage = null;
    if (typeof markDraftOriginsGone === 'function') markDraftOriginsGone(null);
    renderMessageList();
    renderSourceLegend();
    renderHealthPills();
    updateHeaderCounters();
    showDetailAfterStoreChange();
}

// After `cleared` / *Clear*: an open draft stays (#277) — it is the user's
// work, not a stored message; only its pinned reference may be gone.
// Otherwise the panel is emptied.
function showDetailAfterStoreChange() {
    if (state.selectedDraftId && typeof renderDraftDetail === 'function') {
        renderDraftDetail();
        return;
    }
    resetDetailHeader();
    document.getElementById('detail-content').innerHTML = DETAIL_EMPTY_HTML;
}

function updateMessageTags(summary) {
    const listMsg = state.messages.find(m => m.id === summary.id);
    if (listMsg) listMsg.tags = summary.tags;

    const pendingMsg = state.pendingMessages.find(m => m.id === summary.id);
    if (pendingMsg) pendingMsg.tags = summary.tags;

    if (state.selectedMessage && state.selectedMessage.id === summary.id) {
        state.selectedMessage.tags = summary.tags;
        detailJsonCache.delete(state.selectedMessage);
        renderDetail();
    }

    patchRow(summary.id, { tags: summary.tags });
}

function updateMessageBookmark(summary) {
    // A message is in the flushed list or still in the pending buffer —
    // since #239 never in both, but the counter must not depend on that:
    // it changes at most once per event, by the state the client held (#184).
    const listMsg = state.messages.find(m => m.id === summary.id);
    const pendingMsg = state.pendingMessages.find(m => m.id === summary.id);
    const held = listMsg || pendingMsg;
    if (held && held.bookmarked !== summary.bookmarked) {
        state.totalBookmarkCount += summary.bookmarked ? 1 : -1;
    }
    if (listMsg) listMsg.bookmarked = summary.bookmarked;
    if (pendingMsg) pendingMsg.bookmarked = summary.bookmarked;

    if (state.selectedMessage && state.selectedMessage.id === summary.id) {
        state.selectedMessage.bookmarked = summary.bookmarked;
        detailJsonCache.delete(state.selectedMessage);
        renderDetail();
    }

    // If the bookmarks-only filter is active and a row toggles off, it
    // needs to vanish from the list — fall back to a full rebuild in that
    // case. Otherwise just patch the visible row in place.
    if (state.showBookmarkedOnly && !summary.bookmarked) {
        renderMessageList();
    } else {
        patchRow(summary.id, { bookmarked: summary.bookmarked });
    }
    updateHeaderCounters();
}

// A sent message's outcome arrived (#272). It can come within the 250 ms
// before its row is flushed, so the summary is updated wherever the client
// holds it, like tags. The event carries no response text: an open detail
// panel fetches the message again.
function updateMessageDelivery(summary) {
    for (const m of [...state.messages, ...state.pendingMessages]) {
        if (m.id === summary.id) m.delivery = summary.delivery;
    }
    patchRow(summary.id, { delivery: summary.delivery });
    if (state.selectedId === summary.id) selectMessage(summary.id);
}

// Undo a message's contribution to the incremental client counters — the
// inverse of what addMessage adds. Used when a message leaves the client
// buffer (server eviction or client-side cap). `totalMessagesCount` is the
// server total and is deliberately not touched here.
function forgetMessageCounters(m) {
    state.totalValidationCount = Math.max(0,
        state.totalValidationCount - (m.validation_warning_count || 0) - (m.has_segment_errors ? 1 : 0));
    if (m.bookmarked) state.totalBookmarkCount = Math.max(0, state.totalBookmarkCount - 1);
    const label = srcLabelFor(sourceKeyOf(m));
    if (label) {
        const n = (state.sourceCounts.get(label) || 0) - 1;
        if (n > 0) state.sourceCounts.set(label, n);
        else state.sourceCounts.delete(label);
    }
}

// Keep `state.messages` at or below CLIENT_MESSAGE_CAP (#177) by dropping the
// oldest entries. Bookmarked messages and the selected message are kept, so
// the bookmark filter and the open detail panel never lose their row.
// Returns the Set of dropped IDs (empty when nothing was trimmed).
function trimClientBuffer() {
    const dropped = new Set();
    // The cap counts the droppable messages only: bookmarked and selected
    // ones are exempt and don't take places in it. Computing the excess from
    // the total instead dropped as many extra messages as there were
    // bookmarks on one flush, and none on the next (#204).
    // O(1): pendingMessages is empty here, so totalBookmarkCount covers
    // messages[] and no scan is needed when nothing is droppable.
    const selectedExempt = state.selectedId && !(state.selectedMessage && state.selectedMessage.bookmarked) ? 1 : 0;
    // Older pages the user loaded (#198) raise the cap by their size.
    let excess = state.messages.length - state.totalBookmarkCount - selectedExempt
        - (CLIENT_MESSAGE_CAP + state.olderLoaded);
    if (excess <= 0) return dropped;

    for (let i = state.messages.length - 1; i >= 0 && excess > 0; i--) {
        const m = state.messages[i];
        if (m.bookmarked || m.id === state.selectedId) continue;
        dropped.add(m.id);
        forgetMessageCounters(m);
        excess--;
    }
    if (dropped.size > 0) {
        state.messages = state.messages.filter(m => !dropped.has(m.id));
    }
    return dropped;
}

// Server evicted messages (memory/count limit). Drop them from the client
// buffers so stale rows don't linger and 404 on click, undo their share of
// the incremental counters, and tell the user when bookmarks were among them.
function removeEvictedMessages(ids) {
    if (!Array.isArray(ids) || ids.length === 0) return;
    const gone = new Set(ids);
    let lostBookmarks = 0;

    const keep = (m) => {
        if (!gone.has(m.id)) return true;
        if (m.bookmarked) lostBookmarks++;
        forgetMessageCounters(m);
        return false;
    };

    const listBefore = state.messages.length;
    state.messages = state.messages.filter(keep);
    const removedFromList = listBefore - state.messages.length;
    state.pendingMessages = state.pendingMessages.filter(keep);
    state.totalMessagesCount = Math.max(0, state.totalMessagesCount - ids.length);

    if (state.selectedId && gone.has(state.selectedId)) {
        state.selectedId = null;
        state.selectedMessage = null;
        resetDetailHeader();
        document.getElementById('detail-content').innerHTML = DETAIL_EMPTY_HTML;
    }
    // A pinned diff reference that the server evicted can't be shown or
    // unpinned from the list anymore — drop it instead of diffing against a
    // message that no longer exists.
    if (state.diffPinnedMessage && gone.has(state.diffPinnedMessage.id)) {
        state.diffPinnedMessage = null;
        state.diffPinRequestSeq++; // also void an in-flight pin restore
        saveSession();
        if (state.selectedMessage) renderDetail();
        else if (state.selectedDraftId && typeof renderDraftDetail === 'function') renderDraftDetail();
    }

    // Drafts made from an evicted message can't fill their references any
    // more (#290 review).
    if (typeof markDraftOriginsGone === 'function') markDraftOriginsGone(ids);

    if (removedFromList > 0) removeRowsFromList(gone);
    renderSourceLegend();
    updateHeaderCounters();

    if (lostBookmarks > 0) {
        showToast(`${lostBookmarks} bookmarked message${lostBookmarks === 1 ? ' was' : 's were'} evicted — the store reached its bookmark budget`);
    }
}

// Buffer incoming messages, flush at most every 250 ms (prevents DOM freeze at high message rates)
function addMessage(summary) {
    state.pendingMessages.unshift(summary);
    registerSource(sourceKeyOf(summary));
    // Keep the source-counts cache current — avoids re-iterating messages[]
    // on every renderSourceLegend call.
    const label = srcLabelFor(sourceKeyOf(summary));
    if (label) state.sourceCounts.set(label, (state.sourceCounts.get(label) || 0) + 1);
    state.totalMessagesCount++;
    state.totalValidationCount += (summary.validation_warning_count || 0) + (summary.has_segment_errors ? 1 : 0);
    if (summary.bookmarked) state.totalBookmarkCount++; // defensive: normally false, but safe on reconnect
    // While paused nothing is flushed, so cap the pending buffer too (#177):
    // keep the newest CLIENT_MESSAGE_CAP, drop the oldest pending one. On
    // resume the flush then shows a contiguous run of the newest messages.
    if (state.pendingMessages.length > CLIENT_MESSAGE_CAP) {
        forgetMessageCounters(state.pendingMessages.pop());
    }
    // `rate` and `last` count received messages only (#272).
    if (summary.direction !== 'out') {
        const now = Date.now();
        state.rateWindow.push(now);
        state.lastMessageReceivedAt = now;
    }
    if (!state.paused) {
        scheduleRender();
    }
}

function scheduleRender() {
    if (state.renderScheduled) return;
    state.renderScheduled = true;
    setTimeout(() => {
        state.renderScheduled = false;
        flushAndRender();
    }, 250);
}

function flushAndRender() {
    if (state.pendingMessages.length === 0) {
        updateHeaderCounters();
        return;
    }
    const newOnes = state.pendingMessages;
    state.messages = [...newOnes, ...state.messages];
    state.pendingMessages = [];
    const dropped = trimClientBuffer();

    // Hot path: no client-side filters → DOM diff via prepend instead of
    // tearing down and rebuilding every row. Rows trimmed from the buffer are
    // removed from the bottom of the list in the same flush.
    if (canPrependOnly()) {
        prependMessagesToList(newOnes.filter(m => !dropped.has(m.id)));
        removeRowsFromList(dropped);
    } else {
        renderMessageList();
    }
    renderSourceLegend();
    updateHeaderCounters();
}

// Load the newest page plus every bookmarked message (#231: a bookmark older
// than the page would otherwise be unreachable after a reload), then replay
// the events that arrived while the request was in flight (#239).
async function loadMessages() {
    const seq = ++state.loadSeq;
    state.loadInProgress = true;
    let snapshot = null;
    try {
        const resp = await fetch('/api/messages?limit=1000&include_bookmarks=true');
        if (resp.ok) snapshot = await resp.json();
    } catch (e) {
        console.error('Failed to load messages:', e);
    }
    // A newer load started meanwhile: it owns the queue and will replay it.
    if (seq !== state.loadSeq) return;

    state.loadInProgress = false;
    const queued = state.loadQueue;
    state.loadQueue = [];
    if (snapshot) applySnapshot(snapshot);

    const known = new Set(state.messages.map(m => m.id));
    for (const m of state.pendingMessages) known.add(m.id);
    for (const ev of queued) handleServerEvent(ev, known);
    // A replayed `evicted` older than the snapshot lowers the total by IDs
    // the snapshot no longer had; resync it now instead of on the next
    // 3-second poll.
    if (queued.length > 0) pollStats();

    if (snapshot) await restoreAfterLoad();
}

function resetOlderHistory() {
    state.olderLoaded = 0;
    state.serverSearch = { query: '', cursor: null, done: false };
}

function applySnapshot(messages) {
    state.messages = messages;
    resetOlderHistory();
    // The total normally comes from the WS `init` event / pollStats; make
    // sure it is never below what is actually loaded (e.g. socket not up).
    state.totalMessagesCount = Math.max(state.totalMessagesCount, state.messages.length);
    state.pendingMessages = [];
    // Recompute all incremental counters from the freshly-loaded batch.
    // This is O(n) but only runs on init/lagged reload, never per-message.
    state.totalValidationCount = 0;
    state.totalBookmarkCount = 0;
    state.seenSources.clear();
    for (const m of state.messages) {
        registerSource(sourceKeyOf(m));
        state.totalValidationCount += (m.validation_warning_count || 0) + (m.has_segment_errors ? 1 : 0);
        if (m.bookmarked) state.totalBookmarkCount++;
    }
    recomputeSourceCounts();
    renderMessageList();
    renderSourceLegend();
    updateHeaderCounters();
}

async function restoreAfterLoad() {
    if (state._pendingDiffPinnedId) {
        const pinId = state._pendingDiffPinnedId;
        delete state._pendingDiffPinnedId;
        // Same sequencing as toggleDiffPin (#183): a pin/unpin the user
        // makes while this restore is in flight wins. Checked after the
        // fetch and after the body read, like the other two paths.
        const seq = ++state.diffPinRequestSeq;
        try {
            const r = await fetch(`/api/messages/${pinId}`);
            if (!r.ok) {
                // Evicted or cleared since the session was saved (#190).
                console.debug(`Pinned diff message ${pinId} is no longer stored (HTTP ${r.status})`);
            } else if (seq === state.diffPinRequestSeq) {
                const msg = await r.json();
                if (seq === state.diffPinRequestSeq) {
                    state.diffPinnedMessage = msg;
                    renderMessageList();
                }
            }
        } catch (e) {
            // Network error while restoring the pin: drop it (#190).
            console.debug('Could not restore the pinned diff message:', e);
        }
    }
    if (state.selectedId) {
        selectMessage(state.selectedId);
    }
}

async function pollStats() {
    try {
        const resp = await fetch('/api/stats');
        if (!resp.ok) return;
        const stats = await resp.json();
        state.totalMessagesCount = stats.total_messages;
        updateHeaderCounters(); // keep the total live while paused (O(1))

        document.getElementById('pill-conns-value').textContent =
            `${stats.active_connections} / ${stats.max_connections}`;

        // parse errors pill — paint pill red when nonzero (server-side MLLP parse failures)
        const parseErrorsPill = document.getElementById('pill-parse-errors');
        const errorsValue = document.getElementById('pill-errors-value');
        errorsValue.textContent = stats.parse_errors;
        if (parseErrorsPill) {
            parseErrorsPill.classList.toggle('err-pill', stats.parse_errors > 0);
        }

        // `out` pill (#274) — every send attempt, as the list shows them;
        // hidden at 0, amber when a send failed or was rejected. Labelled
        // "out", not "sent": /api/stats' `sent` counts accepted ones only.
        const sentPill = document.getElementById('pill-sent');
        const sentValue = document.getElementById('pill-sent-value');
        if (sentPill && sentValue) {
            const sent = stats.sent || 0;
            const rejected = stats.send_rejected || 0;
            const failed = stats.send_failed || 0;
            const total = sent + rejected + failed;
            sentPill.classList.toggle('hidden', total === 0);
            sentPill.classList.toggle('warn-pill', rejected + failed > 0);
            sentValue.textContent = total;
            sentPill.title = `Messages Harux sent or tried to send: ${sent} accepted, ${rejected} rejected, ${failed} failed`;
        }
        // The server's sending switch changed (a restart with another
        // harux.toml): read its configuration again.
        if (typeof loadSendConfig === 'function' && !!stats.sending_enabled !== !!state.sendConfig) {
            loadSendConfig();
        }

        // rejected pill — hidden when zero
        const rejectedPill = document.getElementById('pill-rejected');
        const rejectedValue = document.getElementById('pill-rejected-value');
        if (rejectedPill && rejectedValue) {
            if (stats.rejected_connections > 0) {
                rejectedPill.classList.remove('hidden');
                rejectedValue.textContent = stats.rejected_connections;
                rejectedPill.classList.add('warn-pill');
            } else {
                rejectedPill.classList.add('hidden');
                rejectedPill.classList.remove('warn-pill');
            }
        }

        // listening pill port + empty-state hint + CLI snippet port
        if (stats.mllp_port) {
            document.getElementById('pill-port').textContent = stats.mllp_port;
            const emptyPort = document.getElementById('mllp-port');
            if (emptyPort) emptyPort.textContent = stats.mllp_port;
            const cliPort = document.getElementById('cli-port');
            if (cliPort) cliPort.textContent = stats.mllp_port;
        }
    } catch (e) {
        console.debug('[pollStats]', e);
    }
}

// --- Older history (#198) and server search (#197) ---

// The ID below which "Load older" continues: the oldest loaded message that
// is neither bookmarked, selected nor a server-search hit. Trimming drops the
// oldest of those first, so every message newer than it is loaded — the
// exempt ones may sit further back and are deduplicated on merge.
function historyCursor() {
    for (let i = state.messages.length - 1; i >= 0; i--) {
        const m = state.messages[i];
        if (!m.bookmarked && m.id !== state.selectedId && !m._searchHit) return m.id;
    }
    return null;
}

// Merge `found` (newest first, from the server) into the buffer: skip what
// is already loaded, keep the list ordered by arrival, count the additions
// like addMessage does, and raise the cap so they stay. Returns the number
// of messages added.
function mergeOlderMessages(found, searchHit) {
    const known = new Set(state.messages.map(m => m.id));
    for (const m of state.pendingMessages) known.add(m.id);
    const added = found.filter(m => !known.has(m.id));
    if (added.length === 0) return 0;
    for (const m of added) {
        if (searchHit) m._searchHit = true;
        registerSource(sourceKeyOf(m));
        const label = srcLabelFor(sourceKeyOf(m));
        if (label) state.sourceCounts.set(label, (state.sourceCounts.get(label) || 0) + 1);
        state.totalValidationCount += (m.validation_warning_count || 0) + (m.has_segment_errors ? 1 : 0);
        if (m.bookmarked) state.totalBookmarkCount++;
    }
    // Newest first by arrival. received_at is RFC 3339; Date.parse keeps
    // millisecond order, and the sort is stable for ties.
    state.messages = [...state.messages, ...added]
        .map((m, i) => [Date.parse(m.received_at) || 0, i, m])
        .sort((a, b) => (b[0] - a[0]) || (a[1] - b[1]))
        .map(x => x[2]);
    state.olderLoaded += added.length;
    renderMessageList();
    renderSourceLegend();
    updateHeaderCounters();
    return added.length;
}

// GET `url`, unless a list (re)load is running or starts meanwhile — its
// snapshot replaces the buffer, so an older page fetched against the old
// one would be merged into the wrong list.
async function fetchOlder(url) {
    if (state.loadInProgress || state.olderLoading) return null;
    const seq = state.loadSeq;
    state.olderLoading = true;
    updateTruncationNote();
    try {
        const resp = await fetch(url);
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
        const list = await resp.json();
        return seq === state.loadSeq && !state.loadInProgress ? list : null;
    } catch (e) {
        console.warn('Could not load older messages:', e);
        showToast('Could not load older messages');
        return null;
    } finally {
        state.olderLoading = false;
        updateTruncationNote();
    }
}

// "Load older" (#198): the next page below the loaded history.
async function loadOlderMessages() {
    const cursor = historyCursor();
    const url = cursor
        ? `/api/messages?before=${encodeURIComponent(cursor)}&limit=${OLDER_PAGE_SIZE}`
        : `/api/messages?limit=${OLDER_PAGE_SIZE}`;
    const found = await fetchOlder(url);
    if (!found) return;
    if (mergeOlderMessages(found, false) === 0) {
        // Nothing older (or the cursor was evicted meanwhile): say so, like
        // the server search does (#268 review).
        showToast('No older messages on the server', 'info');
    }
}

// "Search older on the server" (#197): the current query over the messages
// the client hasn't loaded, newest first, continuing below the last hit.
async function searchOlderOnServer() {
    const query = state.searchQuery;
    if (!query) return;
    if (state.serverSearch.query !== query) state.serverSearch = { query, cursor: null, done: false };
    if (state.serverSearch.done) return;
    const cursor = state.serverSearch.cursor || historyCursor();
    let url = `/api/search?q=${encodeURIComponent(query)}&limit=${OLDER_PAGE_SIZE}`;
    if (cursor) url += `&before=${encodeURIComponent(cursor)}`;
    const found = await fetchOlder(url);
    if (!found || state.searchQuery !== query) return;
    if (found.length > 0) state.serverSearch.cursor = found[found.length - 1].id;
    state.serverSearch.done = found.length < OLDER_PAGE_SIZE;
    const added = mergeOlderMessages(found, true);
    if (added === 0) updateTruncationNote();
    if (found.length === 0) showToast('No older matches on the server', 'info');
}
