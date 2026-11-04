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
        const data = JSON.parse(event.data);
        if (data.type === 'init') {
            state.totalMessagesCount = data.total;
            loadMessages();
        } else if (data.type === 'new_message') {
            addMessage(data.data);
        } else if (data.type === 'tags_updated') {
            updateMessageTags(data.data);
        } else if (data.type === 'bookmark_toggled') {
            updateMessageBookmark(data.data);
        } else if (data.type === 'evicted') {
            removeEvictedMessages(data.data);
        } else if (data.type === 'lagged') {
            console.warn(`Missed ${data.missed} messages, reloading...`);
            loadMessages();
        } else if (data.type === 'cleared') {
            console.info("Server cleared messages via Web UI or API");
            state.messages = [];
            state.pendingMessages = [];
            state.sourceCounts.clear();
            state.seenSources.clear();
            state.diffPinnedMessage = null;
            state.totalMessagesCount = 0;
            state.totalValidationCount = 0;
            state.totalBookmarkCount = 0;
            state.rateWindow.length = 0;
            state.lastMessageReceivedAt = null;
            state.selectedId = null;
            state.selectedMessage = null;
            renderMessageList();
            renderSourceLegend();
            renderHealthPills();
            updateHeaderCounters();
            resetDetailHeader();
            document.getElementById('detail-content').innerHTML = DETAIL_EMPTY_HTML;
        }
    };
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
    const listMsg = state.messages.find(m => m.id === summary.id);
    if (listMsg) {
        if (listMsg.bookmarked !== summary.bookmarked) {
            state.totalBookmarkCount += summary.bookmarked ? 1 : -1;
        }
        listMsg.bookmarked = summary.bookmarked;
    }

    const pendingMsg = state.pendingMessages.find(m => m.id === summary.id);
    if (pendingMsg) {
        // Edge case: message arrived but not yet flushed from pendingMessages.
        if (pendingMsg.bookmarked !== summary.bookmarked) {
            state.totalBookmarkCount += summary.bookmarked ? 1 : -1;
        }
        pendingMsg.bookmarked = summary.bookmarked;
    }

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

// Undo a message's contribution to the incremental client counters — the
// inverse of what addMessage adds. Used when a message leaves the client
// buffer (server eviction or client-side cap). `totalMessagesCount` is the
// server total and is deliberately not touched here.
function forgetMessageCounters(m) {
    state.totalValidationCount = Math.max(0,
        state.totalValidationCount - (m.validation_warning_count || 0) - (m.has_segment_errors ? 1 : 0));
    if (m.bookmarked) state.totalBookmarkCount = Math.max(0, state.totalBookmarkCount - 1);
    const label = srcLabelFor(m.source_addr);
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
    let excess = state.messages.length - CLIENT_MESSAGE_CAP;
    if (excess <= 0) return dropped;
    // O(1) early exit: if exempt entries alone account for the excess there
    // is nothing droppable, so don't scan the whole buffer on every flush.
    // (pendingMessages is empty here, so totalBookmarkCount covers messages[].)
    const selectedExempt = state.selectedId && !(state.selectedMessage && state.selectedMessage.bookmarked) ? 1 : 0;
    if (state.messages.length - state.totalBookmarkCount - selectedExempt <= CLIENT_MESSAGE_CAP) return dropped;

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
    registerSource(summary.source_addr);
    // Keep the source-counts cache current — avoids re-iterating messages[]
    // on every renderSourceLegend call.
    const label = srcLabelFor(summary.source_addr);
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
    const now = Date.now();
    state.rateWindow.push(now);
    state.lastMessageReceivedAt = now;
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

async function loadMessages() {
    try {
        const resp = await fetch('/api/messages?limit=1000');
        if (!resp.ok) return;
        state.messages = await resp.json();
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
            registerSource(m.source_addr);
            state.totalValidationCount += (m.validation_warning_count || 0) + (m.has_segment_errors ? 1 : 0);
            if (m.bookmarked) state.totalBookmarkCount++;
        }
        recomputeSourceCounts();
        renderMessageList();
        renderSourceLegend();
        updateHeaderCounters();
        if (state._pendingDiffPinnedId) {
            const pinId = state._pendingDiffPinnedId;
            delete state._pendingDiffPinnedId;
            // Same sequencing as toggleDiffPin (#183): a pin/unpin the user
            // makes while this restore is in flight wins.
            const seq = ++state.diffPinRequestSeq;
            try {
                const r = await fetch(`/api/messages/${pinId}`);
                if (r.ok) {
                    const msg = await r.json();
                    if (seq === state.diffPinRequestSeq) {
                        state.diffPinnedMessage = msg;
                        renderMessageList();
                    }
                }
            } catch (_) { /* pinned message gone — silently drop */ }
        }
        if (state.selectedId) {
            selectMessage(state.selectedId);
        }
    } catch (e) {
        console.error('Failed to load messages:', e);
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
