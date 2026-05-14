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
        } else if (data.type === 'lagged') {
            console.warn(`Missed ${data.missed} messages, reloading...`);
            loadMessages();
        } else if (data.type === 'cleared') {
            console.info("Server cleared messages via Web UI or API");
            state.messages = [];
            state.pendingMessages = [];
            state.sourceCounts.clear();
            state.totalMessagesCount = 0;
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
    if (listMsg) listMsg.bookmarked = summary.bookmarked;

    const pendingMsg = state.pendingMessages.find(m => m.id === summary.id);
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

    // Hot path: no client-side filters → DOM diff via prepend instead of
    // tearing down and rebuilding every row.
    if (canPrependOnly()) {
        prependMessagesToList(newOnes);
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
        state.pendingMessages = [];
        for (const m of state.messages) {
            registerSource(m.source_addr);
        }
        recomputeSourceCounts();
        renderMessageList();
        renderSourceLegend();
        updateHeaderCounters();
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
                rejectedPill.style.display = '';
                rejectedValue.textContent = stats.rejected_connections;
                rejectedPill.classList.add('warn-pill');
            } else {
                rejectedPill.style.display = 'none';
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
