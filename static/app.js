// --- Icons (lucide-style outlines, currentColor stroke) ---
const ICONS = {
    pause: '<svg class="i" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="6" y="4" width="4" height="16"/><rect x="14" y="4" width="4" height="16"/></svg>',
    play: '<svg class="i" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="5 3 19 12 5 21 5 3"/></svg>',
    download: '<svg class="i" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>',
    arrowDown: '<svg class="i" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="5" x2="12" y2="19"/><polyline points="19 12 12 19 5 12"/></svg>',
    starOutline: '<svg class="i" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2"/></svg>',
    starFilled: '<svg class="i" viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2"/></svg>',
    pin: '<svg class="i" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="17" x2="12" y2="22"/><path d="M5 17h14V8a3 3 0 0 0-3-3H8a3 3 0 0 0-3 3v9z"/></svg>',
    warning: '<svg class="i" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/><line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>',
    trash: '<svg class="i" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/><line x1="10" y1="11" x2="10" y2="17"/><line x1="14" y1="11" x2="14" y2="17"/><path d="M9 6V4a2 2 0 0 1 2-2h2a2 2 0 0 1 2 2v2"/></svg>',
    copy: '<svg class="i-sm" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>',
    chevronRight: '<svg class="i-sm" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>',
    chevronDown: '<svg class="i-sm" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"/></svg>',
    xMark: '<svg class="i-sm" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>',
};

// Empty-state markup for the detail panel when no message is selected.
const DETAIL_EMPTY_HTML = `
<div class="empty-state">
    <div class="empty-card">
        <div class="glyph">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/>
                <polyline points="14 2 14 8 20 8"/>
                <line x1="16" y1="13" x2="8" y2="13"/>
                <line x1="16" y1="17" x2="8" y2="17"/>
                <polyline points="10 9 9 9 8 9"/>
            </svg>
        </div>
        <h3>No message selected</h3>
        <p>Pick a message from the list to inspect its segments, raw payload, ACK, or JSON.</p>
    </div>
</div>`;

// --- State ---
let messages = [];
let selectedId = null;
let selectedMessage = null;
let activeTab = 'parsed';
let autoscroll = true;
let searchQuery = '';
let ws = null;
let collapsedSegments = new Set();

// WebSocket reconnection with exponential backoff
const WS_RECONNECT_INITIAL = 1000;   // 1 second
const WS_RECONNECT_MAX = 60000;  // 60 seconds
const WS_RECONNECT_MULT = 2;      // double each time
let wsReconnectDelay = WS_RECONNECT_INITIAL;

// Task 2: batching state
let paused = false;
let pendingMessages = [];
let renderScheduled = false;
let showBookmarkedOnly = false;
let validationFilter = 0; // 0: All, 1: Warnings, 2: Errors Only

// Server state for accurate total message count
let totalMessagesCount = 0;

// Health-pill state: rolling 60-second window of message timestamps.
const rateWindow = [];               // Date.now() timestamps within last 60 s
let lastMessageReceivedAt = null;    // Date.now() of most recent addMessage

// Segment diff state
let diffPinnedMessage = null; // the reference message pinned for comparison
let diffIgnoreDynamic = false;
const DYNAMIC_DIFF_FIELDS = new Set(['MSH-7', 'MSH-10']);

// --- Source Color Mapping ---
// 12 visually distinct colors optimised for dark backgrounds (HSL, high sat, medium lightness)
const SOURCE_PALETTE = [
    'hsl(210, 90%, 65%)',   // blue
    'hsl(150, 70%, 55%)',   // green
    'hsl(30,  85%, 60%)',   // orange
    'hsl(280, 75%, 65%)',   // purple
    'hsl(0,   80%, 62%)',   // red
    'hsl(180, 70%, 50%)',   // teal
    'hsl(50,  85%, 55%)',   // gold
    'hsl(330, 75%, 62%)',   // pink
    'hsl(200, 80%, 55%)',   // sky
    'hsl(100, 60%, 50%)',   // lime
    'hsl(260, 65%, 60%)',   // indigo
    'hsl(15,  90%, 58%)',   // coral
];
let seenSources = new Set();
let colorByPort = false;
let highlightedSource = null;

function hashString(str) {
    let hash = 0;
    for (let i = 0; i < str.length; i++) {
        hash = ((hash << 5) - hash + str.charCodeAt(i)) | 0;
    }
    return Math.abs(hash);
}

function getSourceColor(addr) {
    if (!addr) return 'var(--text-muted)';
    const colorKey = colorByPort ? addr : addr.split(':')[0];
    return SOURCE_PALETTE[hashString(colorKey) % SOURCE_PALETTE.length];
}

function registerSource(addr) {
    if (addr) seenSources.add(addr);
}

function toggleColorByPort(e) {
    colorByPort = e.target.checked;
    highlightedSource = null; // reset highlight on toggle
    renderMessageList();
    renderSourceLegend();
    saveSession();
}

function toggleHighlightSource(label) {
    highlightedSource = (highlightedSource === label) ? null : label;
    renderMessageList();
    renderSourceLegend();
    saveSession();
}

function srcLabelFor(addr) {
    if (!addr) return '';
    return colorByPort ? addr : addr.split(':')[0];
}

function renderSourceLegend() {
    const container = document.getElementById('source-rail');
    if (!container) return;

    if (seenSources.size === 0) {
        container.innerHTML = `
            <span class="label">Sources</span>
            <div class="source-rail-chips">
                <span class="source-rail-empty">none yet</span>
            </div>
            <details class="source-rail-overflow">
                <summary title="Source options">⋯</summary>
                <div class="popover">
                    <label>
                        <input type="checkbox" onchange="toggleColorByPort(event)" ${colorByPort ? 'checked' : ''}>
                        Color by Port
                    </label>
                </div>
            </details>
        `;
        return;
    }

    // Build per-source counts from the messages array.
    const counts = new Map();
    for (const m of messages) {
        const label = srcLabelFor(m.source_addr);
        if (!label) continue;
        counts.set(label, (counts.get(label) || 0) + 1);
    }

    const uniqueLabels = new Set();
    seenSources.forEach(addr => uniqueLabels.add(srcLabelFor(addr)));
    const sortedLabels = Array.from(uniqueLabels).sort();

    const chipsHtml = sortedLabels.map(label => {
        const color = SOURCE_PALETTE[hashString(label) % SOURCE_PALETTE.length];
        const isActive = highlightedSource === label;
        const isDimmed = highlightedSource && highlightedSource !== label;
        const classes = `source-chip${isActive ? ' active' : ''}${isDimmed ? ' dimmed' : ''}`;
        const num = counts.get(label) || 0;
        return `<span class="${classes}" onclick="toggleHighlightSource('${escAttr(escJS(label))}')">
            <span class="dot" style="background:${color};color:${color}"></span>
            ${esc(label)}
            <span class="num">${num}</span>
        </span>`;
    }).join('');

    container.innerHTML = `
        <span class="label">Sources</span>
        <div class="source-rail-chips">${chipsHtml}</div>
        <details class="source-rail-overflow">
            <summary title="Source options">⋯</summary>
            <div class="popover">
                <label>
                    <input type="checkbox" onchange="toggleColorByPort(event)" ${colorByPort ? 'checked' : ''}>
                    Color by Port
                </label>
            </div>
        </details>
    `;
}

// --- Session Persistence ---
const SESSION_KEY = 'harux_session';
const SESSION_STATE_KEY = SESSION_KEY + '_state';
const sessionId = sessionStorage.getItem(SESSION_KEY) || crypto.randomUUID();
sessionStorage.setItem(SESSION_KEY, sessionId);

function saveSession() {
    try {
        sessionStorage.setItem(SESSION_STATE_KEY, JSON.stringify({
            selectedId,
            activeTab,
            autoscroll,
            searchQuery,
            paused,
            collapsedSegments: [...collapsedSegments],
            colorByPort,
            highlightedSource,
            showBookmarkedOnly,
            validationFilter,
            diffIgnoreDynamic
        }));
    } catch (_) { /* sessionStorage full or unavailable */ }
}

function loadSession() {
    try {
        const raw = sessionStorage.getItem(SESSION_STATE_KEY);
        if (!raw) return;
        const saved = JSON.parse(raw);
        if (saved.selectedId != null) selectedId = saved.selectedId;
        if (saved.activeTab) activeTab = saved.activeTab;
        if (typeof saved.autoscroll === 'boolean') autoscroll = saved.autoscroll;
        if (typeof saved.searchQuery === 'string') searchQuery = saved.searchQuery;
        if (typeof saved.paused === 'boolean') paused = saved.paused;
        if (typeof saved.colorByPort === 'boolean') colorByPort = saved.colorByPort;
        if (saved.highlightedSource !== undefined) highlightedSource = saved.highlightedSource;
        if (typeof saved.showBookmarkedOnly === 'boolean') showBookmarkedOnly = saved.showBookmarkedOnly;
        if (typeof saved.validationFilter === 'number') validationFilter = saved.validationFilter;
        if (typeof saved.diffIgnoreDynamic === 'boolean') diffIgnoreDynamic = saved.diffIgnoreDynamic;
        if (Array.isArray(saved.collapsedSegments)) {
            collapsedSegments = new Set(saved.collapsedSegments);
        }
    } catch (_) { /* corrupted or unavailable */ }
}

// --- WebSocket ---
function connectWs() {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    ws = new WebSocket(`${proto}//${location.host}/ws`);

    ws.onopen = () => {
        wsReconnectDelay = WS_RECONNECT_INITIAL; // reset on success
        setListeningPillState('live');
    };

    ws.onclose = () => {
        const jitter = wsReconnectDelay * (0.75 + Math.random() * 0.5);
        const delaySec = Math.round(jitter / 1000);
        setListeningPillState('disconnected', `Reconnecting in ${delaySec}s\u2026`);
        setTimeout(connectWs, jitter);
        wsReconnectDelay = Math.min(wsReconnectDelay * WS_RECONNECT_MULT, WS_RECONNECT_MAX);
    };

    ws.onerror = (event) => {
        console.error('WebSocket error:', event);
        setListeningPillState('disconnected', 'WebSocket error');
    };

    ws.onmessage = (event) => {
        const data = JSON.parse(event.data);
        if (data.type === 'init') {
            totalMessagesCount = data.total;
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
            messages = [];
            pendingMessages = [];
            totalMessagesCount = 0;
            rateWindow.length = 0;
            lastMessageReceivedAt = null;
            selectedId = null;
            selectedMessage = null;
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
    const listMsg = messages.find(m => m.id === summary.id);
    if (listMsg) listMsg.tags = summary.tags;

    const pendingMsg = pendingMessages.find(m => m.id === summary.id);
    if (pendingMsg) pendingMsg.tags = summary.tags;

    if (selectedMessage && selectedMessage.id === summary.id) {
        selectedMessage.tags = summary.tags;
        renderDetail();
    }

    renderMessageList();
}

function updateMessageBookmark(summary) {
    const listMsg = messages.find(m => m.id === summary.id);
    if (listMsg) listMsg.bookmarked = summary.bookmarked;

    const pendingMsg = pendingMessages.find(m => m.id === summary.id);
    if (pendingMsg) pendingMsg.bookmarked = summary.bookmarked;

    if (selectedMessage && selectedMessage.id === summary.id) {
        selectedMessage.bookmarked = summary.bookmarked;
        renderDetail();
    }

    renderMessageList();
}

// Task 2: buffer incoming messages, flush at most every 250 ms
function addMessage(summary) {
    pendingMessages.unshift(summary);
    // Register source for color mapping
    registerSource(summary.source_addr);
    totalMessagesCount++;
    const now = Date.now();
    rateWindow.push(now);
    lastMessageReceivedAt = now;
    if (!paused) {
        scheduleRender();
    }
}

function scheduleRender() {
    if (renderScheduled) return;
    renderScheduled = true;
    setTimeout(() => {
        renderScheduled = false;
        flushAndRender();
    }, 250);
}

function flushAndRender() {
    if (pendingMessages.length > 0) {
        messages = [...pendingMessages, ...messages];
        pendingMessages = [];
    }
    renderMessageList();
    renderSourceLegend();
    updateHeaderCounters();
}

async function loadMessages() {
    try {
        const resp = await fetch('/api/messages?limit=1000');
        if (!resp.ok) return;
        messages = await resp.json();
        pendingMessages = [];
        // Register all source addresses for color mapping
        for (const m of messages) {
            registerSource(m.source_addr);
        }
        renderMessageList();
        renderSourceLegend();
        updateHeaderCounters();
    } catch (e) {
        console.error('Failed to load messages:', e);
    }
}

// --- Stats polling ---
async function pollStats() {
    try {
        const resp = await fetch('/api/stats');
        if (!resp.ok) return;
        const stats = await resp.json();
        totalMessagesCount = stats.total_messages;

        // conns pill
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
    } catch (e) { }
}

// --- Health pills ---
function setListeningPillState(state, tooltip) {
    const pill = document.getElementById('pill-listening');
    if (!pill) return;
    pill.classList.remove('live', 'disconnected');
    pill.classList.add(state);
    pill.title = tooltip || (state === 'live' ? 'MLLP listener health' : 'WebSocket disconnected — reconnecting');
}

function formatRelativeTime(ms) {
    if (ms < 1000) return 'just now';
    if (ms < 60000) return `${Math.floor(ms / 1000)}s ago`;
    if (ms < 3600000) return `${Math.floor(ms / 60000)}m ago`;
    return `${Math.floor(ms / 3600000)}h ago`;
}

function pruneRateWindow() {
    const cutoff = Date.now() - 60000;
    while (rateWindow.length > 0 && rateWindow[0] < cutoff) {
        rateWindow.shift();
    }
}

function renderRateSpark() {
    const buckets = new Array(10).fill(0);
    const now = Date.now();
    for (const t of rateWindow) {
        const age = now - t;
        if (age < 0 || age >= 60000) continue;
        // idx 0 = oldest (60s ago), idx 9 = newest (most recent 6s)
        const idx = Math.floor((60000 - age) / 6000);
        const clamped = Math.max(0, Math.min(9, idx));
        buckets[clamped]++;
    }
    const max = Math.max(...buckets, 1);
    const w = 60;
    const h = 18;
    const pad = 1;
    const step = w / (buckets.length - 1);
    const points = buckets.map((v, i) => {
        const x = i * step;
        const y = h - (v / max) * (h - 2 * pad) - pad;
        return `${x.toFixed(1)},${y.toFixed(1)}`;
    }).join(' ');
    return `<polyline points="${points}" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round" stroke-linecap="round" />`;
}

// --- Header counters (total / validation / parse errors) ---
// `parse errors` is updated by pollStats from the server stat. The other two
// counters are derived from the in-memory message buffer.
function updateHeaderCounters() {
    const totalEl = document.getElementById('pill-total-value');
    if (totalEl) {
        totalEl.textContent = messages.length + pendingMessages.length;
    }

    let validationCount = 0;
    for (const m of messages) {
        validationCount += (m.validation_warning_count || 0);
        if (m.has_segment_errors) validationCount++;
    }
    const validationPill = document.getElementById('pill-validation');
    const validationValue = document.getElementById('pill-validation-value');
    if (validationValue) validationValue.textContent = validationCount;
    if (validationPill) {
        validationPill.classList.toggle('warn-pill', validationCount > 0);
    }

    // Bookmarks button count — keep in sync with current message buffer.
    const bookmarkCount = messages.reduce((n, m) => n + (m.bookmarked ? 1 : 0), 0);
    const bookmarkBtn = document.getElementById('btn-bookmarks');
    if (bookmarkBtn) {
        const existing = bookmarkBtn.querySelector('.count');
        if (bookmarkCount > 0) {
            if (existing) {
                existing.textContent = bookmarkCount;
            } else {
                const span = document.createElement('span');
                span.className = 'count';
                span.textContent = bookmarkCount;
                bookmarkBtn.appendChild(span);
            }
        } else if (existing) {
            existing.remove();
        }
    }
}

function tickRowRelativeTimes() {
    const now = Date.now();
    document.querySelectorAll('.message-row').forEach(row => {
        const ts = row.dataset.received;
        if (!ts) return;
        const timeEl = row.querySelector('.msg-row2 .time');
        if (timeEl) timeEl.textContent = rowTimeLabel({ received_at: ts }, now);
    });
}

function renderHealthPills() {
    pruneRateWindow();

    // rate pill — show only after first message
    const ratePill = document.getElementById('pill-rate');
    const rateValue = document.getElementById('pill-rate-value');
    const rateSpark = document.getElementById('pill-rate-spark');
    if (rateWindow.length > 0) {
        ratePill.style.display = '';
        rateValue.textContent = `${rateWindow.length}/min`;
        rateSpark.innerHTML = renderRateSpark();
    } else {
        ratePill.style.display = 'none';
    }

    // last pill — show only after first message
    const lastPill = document.getElementById('pill-last');
    const lastValue = document.getElementById('pill-last-value');
    if (lastMessageReceivedAt !== null) {
        lastPill.style.display = '';
        lastValue.textContent = formatRelativeTime(Date.now() - lastMessageReceivedAt);
    } else {
        lastPill.style.display = 'none';
    }
}

// --- Rendering ---
const MONTH_SHORT = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

function bucketKey(msg, now) {
    const t = new Date(msg.received_at).getTime();
    const age = now - t;
    if (age < 30_000) return 'live';
    if (age < 300_000) return 'recent';

    const msgDate = new Date(t);
    const today = new Date(now);
    if (msgDate.toDateString() === today.toDateString()) return 'today';

    const yesterday = new Date(now - 86_400_000);
    if (msgDate.toDateString() === yesterday.toDateString()) return 'yesterday';

    const y = msgDate.getFullYear();
    const m = String(msgDate.getMonth() + 1).padStart(2, '0');
    const d = String(msgDate.getDate()).padStart(2, '0');
    return `${y}-${m}-${d}`;
}

function bucketLabel(key) {
    if (key === 'live') return 'Live';
    if (key === 'recent') return 'Last few minutes';
    if (key === 'today') return 'Earlier today';
    if (key === 'yesterday') return 'Yesterday';
    const [y, m, d] = key.split('-').map(Number);
    return `${MONTH_SHORT[m - 1]} ${d}`;
}

function rowTimeLabel(msg, now) {
    const t = new Date(msg.received_at).getTime();
    const age = now - t;
    if (age < 60_000) return `${Math.max(0, Math.floor(age / 1000))}s ago`;
    if (age < 300_000) return `${Math.floor(age / 60_000)}m ago`;

    const msgDate = new Date(t);
    const hh = String(msgDate.getHours()).padStart(2, '0');
    const mn = String(msgDate.getMinutes()).padStart(2, '0');
    const ss = String(msgDate.getSeconds()).padStart(2, '0');

    const today = new Date(now);
    if (msgDate.toDateString() === today.toDateString()) {
        return `${hh}:${mn}:${ss}`;
    }
    const yesterday = new Date(now - 86_400_000);
    if (msgDate.toDateString() === yesterday.toDateString()) {
        return `Yest ${hh}:${mn}`;
    }
    return `${MONTH_SHORT[msgDate.getMonth()]} ${msgDate.getDate()} ${hh}:${mn}`;
}

function ackChipClass(code) {
    if (code === 'AA') return 'msg-ack aa';
    if (code === 'AE') return 'msg-ack ae';
    if (code === 'AR') return 'msg-ack ar';
    return 'msg-ack none';
}

function buildMessageRow(msg) {
    const row = document.createElement('div');
    let rowClass = 'message-row';
    if (msg.id === selectedId) rowClass += ' selected';
    if (msg.bookmarked) rowClass += ' bookmarked';

    const srcKey = colorByPort ? msg.source_addr : (msg.source_addr ? msg.source_addr.split(':')[0] : '');
    if (highlightedSource && srcKey !== highlightedSource) {
        rowClass += ' dimmed';
    }

    row.className = rowClass;
    row.dataset.id = msg.id;
    row.tabIndex = 0;
    row.setAttribute('role', 'button');
    row.setAttribute('aria-label', `Message from ${msg.sending_facility || 'unknown'}, type ${msg.message_type || 'unknown'}, received ${msg.received_at}`);
    row.onclick = () => selectMessage(msg.id);
    row.onkeydown = (e) => {
        if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            selectMessage(msg.id);
        }
    };

    const srcColor = getSourceColor(msg.source_addr);

    const typeHtml = msg.parse_error
        ? `<span class="msg-type parse-error" title="${escAttr(msg.parse_error)}">⚠ PARSE ERROR</span>`
        : `<span class="msg-type">${esc(msg.message_type || '—')}</span>`;

    const warnCount = msg.validation_warning_count || 0;
    const warnHtml = warnCount > 0
        ? `<span class="msg-warning ${msg.has_segment_errors ? 'err' : 'warn'}" title="${warnCount} validation warning${warnCount > 1 ? 's' : ''}">⚠ ${warnCount}</span>`
        : '';

    const tagsArr = msg.tags || [];
    let tagsHtml = '';
    if (tagsArr.length > 0) {
        const visible = tagsArr.slice(0, 2).map(t => `<span class="msg-tag-small">${esc(t)}</span>`).join('');
        const overflow = tagsArr.length > 2
            ? `<span class="msg-tag-small">+${tagsArr.length - 2}</span>`
            : '';
        tagsHtml = `<span class="msg-tags-list" style="margin-top:0">${visible}${overflow}</span>`;
    }

    const ackCode = (msg.ack_code || '').toUpperCase();
    const ackHtml = ackCode
        ? `<span class="${ackChipClass(ackCode)}">${esc(ackCode)}</span>`
        : `<span class="msg-ack none">—</span>`;

    const now = Date.now();
    const timeLabel = rowTimeLabel(msg, now);
    const segCount = msg.segment_count != null ? `${msg.segment_count} segs` : '—';
    const facility = esc(msg.sending_facility || 'unknown');
    const sourceAddr = esc(msg.source_addr || '');
    const patient = esc(msg.patient_name || msg.patient_id || '—');

    const bookmarkClass = msg.bookmarked ? 'msg-bookmark active' : 'msg-bookmark';
    const bookmarkSvg = msg.bookmarked ? ICONS.starFilled : ICONS.starOutline;
    const bookmarkLabel = msg.bookmarked ? 'Remove bookmark' : 'Add bookmark';

    row.dataset.received = msg.received_at || '';

    row.innerHTML = `
        <div class="msg-source-bar" style="background:${srcColor}" title="${escAttr(msg.source_addr || '')}"></div>
        <div class="msg-body">
            <div class="msg-row1">
                ${typeHtml}
                <span class="msg-patient">${patient}</span>
                ${warnHtml}
                ${tagsHtml}
                ${ackHtml}
            </div>
            <div class="msg-row2">
                <span class="facility">${facility}</span>
                <span class="sep">·</span>
                <span class="src">${sourceAddr}</span>
                <span class="sep">·</span>
                <span class="segs">${segCount}</span>
                <span class="time">${esc(timeLabel)}</span>
            </div>
        </div>
        <div class="msg-actions">
            <button class="${bookmarkClass}" aria-label="${bookmarkLabel}" onclick="toggleBookmark('${msg.id}', event)" title="Bookmark">${bookmarkSvg}</button>
        </div>
    `;
    return row;
}

// Memoize parsed search queries so the `has:warnings` / `has:errors` prefix
// stripping and the lowercasing of the residual query happen once per query
// instead of once per message during each filter pass. Bounded at 100 entries
// so an attacker (or a developer typing fast) cannot grow the cache without
// limit.
const parsedQueryCache = new Map();
const PARSED_QUERY_CACHE_MAX = 100;

function getParsedQuery(query) {
    const cached = parsedQueryCache.get(query);
    if (cached) return cached;

    let q = query.toLowerCase().trim();
    let hasWarnings = false;
    let hasErrors = false;
    if (q.startsWith('has:warnings')) {
        hasWarnings = true;
        q = q.slice('has:warnings'.length).trim();
    } else if (q.startsWith('has:errors')) {
        hasErrors = true;
        q = q.slice('has:errors'.length).trim();
    }

    const result = { q, hasWarnings, hasErrors };
    if (parsedQueryCache.size >= PARSED_QUERY_CACHE_MAX) parsedQueryCache.clear();
    parsedQueryCache.set(query, result);
    return result;
}

function renderMessageList() {
    const list = document.getElementById('message-list');
    const empty = document.getElementById('empty-state');
    let filtered = searchQuery
        ? messages.filter(m => matchesSearch(m, getParsedQuery(searchQuery)))
        : messages;
    if (showBookmarkedOnly) {
        filtered = filtered.filter(m => m.bookmarked);
    }
    if (validationFilter === 1) { // Any warnings
        filtered = filtered.filter(m => (m.validation_warning_count || 0) > 0 || m.has_segment_errors);
    } else if (validationFilter === 2) { // Errors only
        filtered = filtered.filter(m => m.has_segment_errors);
    }

    if (filtered.length === 0) {
        empty.style.display = 'flex';
        list.querySelectorAll('.message-row, .group-header').forEach(r => r.remove());
        return;
    }

    empty.style.display = 'none';

    const fragment = document.createDocumentFragment();
    const now = Date.now();
    let currentBucket = null;

    for (const msg of filtered) {
        const bucket = bucketKey(msg, now);
        if (bucket !== currentBucket) {
            currentBucket = bucket;
            const header = document.createElement('div');
            header.className = 'group-header';
            header.textContent = bucketLabel(bucket);
            fragment.appendChild(header);
        }
        fragment.appendChild(buildMessageRow(msg));
    }

    list.querySelectorAll('.message-row, .group-header').forEach(r => r.remove());
    list.appendChild(fragment);

    if (autoscroll) {
        list.scrollTop = 0;
    }
}

// Filter check using a pre-parsed query (see getParsedQuery). Each `if` is a
// short-circuit early return so no further `.toLowerCase()` runs once a field
// matches. Empty-string fields are skipped before the lowercasing — saves a
// no-op call per absent property.
function matchesSearch(msg, parsedQuery) {
    if (parsedQuery.hasWarnings) {
        if ((msg.validation_warning_count || 0) === 0 && !msg.has_segment_errors) return false;
    } else if (parsedQuery.hasErrors) {
        if (!msg.has_segment_errors) return false;
    }

    const q = parsedQuery.q;
    if (!q) return true;

    if (msg.patient_name && msg.patient_name.toLowerCase().includes(q)) return true;
    if (msg.message_type && msg.message_type.toLowerCase().includes(q)) return true;
    if (msg.sending_facility && msg.sending_facility.toLowerCase().includes(q)) return true;
    if (msg.patient_id && msg.patient_id.toLowerCase().includes(q)) return true;
    if (msg.message_control_id && msg.message_control_id.toLowerCase().includes(q)) return true;
    if (msg.source_addr && msg.source_addr.toLowerCase().includes(q)) return true;
    if (msg.tags && msg.tags.some(t => t.toLowerCase().includes(q))) return true;

    return false;
}

async function selectMessage(id) {
    selectedId = id;
    renderMessageList();
    saveSession();

    try {
        const resp = await fetch(`/api/messages/${id}`);
        if (!resp.ok) return;
        selectedMessage = await resp.json();
        renderDetail();
    } catch (e) {
        console.error('Failed to load message:', e);
    }
}

function resetDetailHeader() {
    const typeEl = document.getElementById('detail-type');
    if (typeEl) {
        typeEl.style.display = 'none';
        typeEl.textContent = '';
    }
    const titleEl = document.getElementById('detail-title');
    if (titleEl) titleEl.textContent = 'Select a message';
    const descEl = document.getElementById('detail-desc');
    if (descEl) {
        descEl.style.display = 'none';
        descEl.textContent = '';
    }
    const metaEl = document.getElementById('detail-meta');
    if (metaEl) metaEl.innerHTML = '';
    const actionsEl = document.getElementById('detail-actions');
    if (actionsEl) actionsEl.innerHTML = '';
    const segBadge = document.getElementById('tab-segments-badge');
    if (segBadge) segBadge.style.display = 'none';
}

function formatDetailReceived(received) {
    if (!received) return '';
    const t = new Date(received);
    if (isNaN(t.getTime())) return received;
    const yyyy = t.getFullYear();
    const mm = String(t.getMonth() + 1).padStart(2, '0');
    const dd = String(t.getDate()).padStart(2, '0');
    const hh = String(t.getHours()).padStart(2, '0');
    const min = String(t.getMinutes()).padStart(2, '0');
    const ss = String(t.getSeconds()).padStart(2, '0');
    return `${yyyy}-${mm}-${dd} ${hh}:${min}:${ss}`;
}

function buildDetailMeta(msg) {
    const items = [];
    const patient = msg.patient_name || msg.patient_id;
    if (patient) {
        items.push(`<span class="item"><span class="k">patient</span><span class="v">${esc(patient)}</span></span>`);
    }
    if (msg.patient_id && msg.patient_name) {
        // Only show MRN separately if both name and ID exist (otherwise patient covers it).
        items.push(`<span class="item"><span class="k">MRN</span><span class="v">${esc(msg.patient_id)}</span></span>`);
    }
    if (msg.message_control_id) {
        items.push(`<span class="item"><span class="k">control</span><span class="v">${esc(msg.message_control_id)}</span></span>`);
    }
    if (msg.version) {
        items.push(`<span class="item"><span class="k">v</span><span class="v">${esc(msg.version)}</span></span>`);
    }
    if (msg.source_addr) {
        items.push(`<span class="item"><span class="k">from</span><span class="v">${esc(msg.source_addr)}</span></span>`);
    }
    if (msg.charset) {
        items.push(`<span class="item"><span class="k">charset</span><span class="v">${esc(msg.charset)}</span></span>`);
    }
    const received = formatDetailReceived(msg.received_at);
    if (received) {
        items.push(`<span class="item"><span class="k">received</span><span class="v">${esc(received)}</span></span>`);
    }
    return items.join('<span class="sep">·</span>');
}

function renderDetail() {
    if (!selectedMessage) return;
    const msg = selectedMessage;

    // Title row: type chip + human title (patient when available, else message type).
    const typeEl = document.getElementById('detail-type');
    if (msg.message_type) {
        typeEl.textContent = msg.message_type;
        typeEl.style.display = '';
    } else {
        typeEl.style.display = 'none';
    }
    document.getElementById('detail-title').textContent =
        msg.patient_name || msg.patient_id || msg.message_type || 'Message';

    // Description row — always populate when message_type_description is available.
    const descEl = document.getElementById('detail-desc');
    if (msg.message_type_description) {
        descEl.textContent = msg.message_type_description;
        descEl.style.display = '';
    } else {
        descEl.style.display = 'none';
    }

    // Metadata row.
    document.getElementById('detail-meta').innerHTML = buildDetailMeta(msg);

    // Actions row: tag chips + add-tag input + Bookmark + Pin diff (all on the right).
    const isPinned = diffPinnedMessage && diffPinnedMessage.id === msg.id;
    const bookmarkClass = msg.bookmarked ? 'action-btn bookmark active' : 'action-btn bookmark';
    const bookmarkSvg = msg.bookmarked ? ICONS.starFilled : ICONS.starOutline;
    const bookmarkLabel = msg.bookmarked ? 'Bookmarked' : 'Bookmark';
    const pinClass = isPinned ? 'action-btn pin active' : 'action-btn pin';
    const pinLabel = isPinned ? 'Pinned' : 'Pin diff';

    const tagChipsHtml = (msg.tags || []).map(t =>
        `<span class="msg-tag">${esc(t)}<span class="msg-tag-remove" role="button" tabindex="0" onclick="removeTag('${msg.id}', '${escAttr(escJS(t))}')" onkeydown="if(event.key === 'Enter' || event.key === ' ') { event.preventDefault(); removeTag('${msg.id}', '${escAttr(escJS(t))}'); }" title="Remove tag" aria-label="Remove tag">${ICONS.xMark}</span></span>`
    ).join('');
    const tagAddHtml = `
        <div class="msg-tag-add">
            <input type="text" id="add-tag-input" placeholder="Add tag" aria-label="Add tag" onkeypress="if(event.key === 'Enter') addTag('${msg.id}', this.value)">
            <button onclick="addTag('${msg.id}', document.getElementById('add-tag-input').value)" aria-label="Submit tag">+</button>
        </div>
    `;

    document.getElementById('detail-actions').innerHTML = `
        <div id="detail-tags" class="detail-tags-inline">${tagChipsHtml}${tagAddHtml}</div>
        <button class="${bookmarkClass}" aria-label="${bookmarkLabel}" onclick="toggleBookmark('${msg.id}', event)" title="Toggle bookmark">${bookmarkSvg}<span class="btn-label">${bookmarkLabel}</span></button>
        <button class="${pinClass}" aria-label="${pinLabel}" onclick="toggleDiffPin('${msg.id}', event)" title="Pin this message as the diff reference">${ICONS.pin}<span class="btn-label">${pinLabel}</span></button>
    `;

    // Segments tab badge — segment count.
    const segBadge = document.getElementById('tab-segments-badge');
    if (segBadge) {
        const count = (msg.segments && msg.segments.length) || 0;
        if (count > 0) {
            segBadge.textContent = count;
            segBadge.style.display = '';
        } else {
            segBadge.style.display = 'none';
        }
    }

    // Show/hide Diff tab based on whether a pinned message exists and it's a different message
    const diffTabBtn = document.getElementById('tab-btn-diff');
    if (diffTabBtn) {
        const showDiff = diffPinnedMessage && diffPinnedMessage.id !== msg.id;
        diffTabBtn.style.display = showDiff ? '' : 'none';
        if (!showDiff && activeTab === 'diff') {
            activeTab = 'parsed';
        }
    }

    renderTab();
}

async function toggleDiffPin(id, event) {
    if (event) event.stopPropagation();
    if (diffPinnedMessage && diffPinnedMessage.id === id) {
        diffPinnedMessage = null;
        renderMessageList();
        renderDetail();
        return;
    }
    try {
        const resp = await fetch(`/api/messages/${id}`);
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
        diffPinnedMessage = await resp.json();
    } catch (e) {
        console.error('Failed to fetch pinned message:', e);
        return;
    }
    renderMessageList();
    renderDetail();
}

function switchTab(tab) {
    activeTab = tab;
    document.querySelectorAll('.detail-tab').forEach(t => {
        t.classList.toggle('active', t.dataset.tab === tab);
    });
    renderTab();
    saveSession();
}

function renderTab() {
    const content = document.getElementById('detail-content');
    if (!selectedMessage) return;
    const msg = selectedMessage;

    if (activeTab === 'parsed') {
        // Task 1: show parse error banner instead of empty segment table
        if (msg.parse_error) {
            content.innerHTML = `<div style="color:var(--error);font-family:var(--font-mono);padding:16px;line-height:1.6;">
                ⚠ Parse Error<br><br>
                <span style="color:var(--text-secondary)">${esc(msg.parse_error)}</span><br><br>
                <span style="color:var(--text-muted)">Raw message is available in the Raw tab.</span>
            </div>`;
            return;
        }
        // Build warning maps so typical-segment badges can reflect validation state.
        // missingSegWarnings: segName → warning message (MISSING_SEGMENT)
        // fieldWarningSegs:   segName → true (has at least one MISSING_FIELD warning)
        const warnings = msg.validation_warnings || [];
        const missingSegWarnings = {};
        const fieldWarningSegs = {};
        for (const w of warnings) {
            if (w.code === 'MISSING_SEGMENT') missingSegWarnings[w.segment] = w.message;
            else if (w.code === 'MISSING_FIELD') fieldWarningSegs[w.segment] = true;
        }

        const typicalChecklist = (msg.typical_segments && msg.typical_segments.length)
            ? `<div class="seg-checklist">
                <span class="seg-checklist-label">Typical segments</span>
                ${msg.typical_segments.map(s => {
                const present = msg.segments.some(seg => seg.name === s);
                const desc = (msg.typical_segment_descriptions || {})[s];
                let cls, symbol, titleText;
                if (missingSegWarnings[s]) {
                    cls = 'missing';
                    symbol = '✕';
                    titleText = missingSegWarnings[s];
                } else if (fieldWarningSegs[s]) {
                    cls = 'warn';
                    symbol = '⚠';
                    titleText = (desc ? desc + ' — ' : '') + 'has required fields missing';
                } else if (present) {
                    cls = 'present';
                    symbol = '✓';
                    titleText = desc || null;
                } else {
                    cls = 'absent';
                    symbol = '';
                    titleText = desc || null;
                }
                const titleAttr = titleText ? ` title="${escAttr(titleText)}"` : '';
                const symbolHtml = symbol ? ` ${symbol}` : '';
                return `<span class="seg-pill ${cls}"${titleAttr}>${esc(s)}${symbolHtml}</span>`;
            }).join('')}
               </div>`
            : '';

        // Validation summary banner — one-line aggregate + collapsible full list.
        let validationBanner = '';
        if (warnings.length) {
            const hasSegErrors = warnings.some(w => w.code === 'MISSING_SEGMENT');
            const summaryClass = hasSegErrors ? 'validation-summary error' : 'validation-summary';

            const segMissing = warnings.filter(w => w.code === 'MISSING_SEGMENT').map(w => w.segment);
            const fieldMissing = warnings.filter(w => w.code === 'MISSING_FIELD').map(w => `${w.segment}-${w.field}`);
            const datatype = warnings.filter(w => w.code === 'INVALID_DATATYPE').map(w => `${w.segment}-${w.field}`);

            const fmtList = (items, max) => {
                const head = items.slice(0, max).map(x => esc(x)).join(', ');
                const rest = items.length > max ? ` +${items.length - max} more` : '';
                return head + rest;
            };

            const parts = [];
            if (fieldMissing.length) {
                parts.push(`required field missing in <span class="seg-list">${fmtList(fieldMissing, 3)}</span>`);
            }
            if (segMissing.length) {
                parts.push(`expected segment not sent: <span class="seg-list">${fmtList(segMissing, 3)}</span>`);
            }
            if (datatype.length) {
                parts.push(`invalid datatype in <span class="seg-list-type">${fmtList(datatype, 3)}</span>`);
            }
            const summaryLine = parts.join(' · ');
            const headline = `${warnings.length} validation ${warnings.length === 1 ? 'warning' : 'warnings'}`;

            validationBanner = `<details class="${summaryClass}">
                <summary>
                    <span class="summary-icon">${ICONS.warning}</span>
                    <span class="summary-text"><strong>${headline}</strong> · ${summaryLine}</span>
                    <span class="summary-chevron">${ICONS.chevronRight}</span>
                </summary>
                <ul class="validation-warnings-list">
                    ${warnings.map(w => {
                const badgeCls = w.code === 'MISSING_SEGMENT' ? 'validation-seg error'
                    : w.code === 'INVALID_DATATYPE' ? 'validation-seg type'
                    : 'validation-seg';
                const label = w.segment + (w.field != null ? '-' + w.field : '');
                return `<li><span class="${badgeCls}">${esc(label)}</span> ${esc(w.message)}</li>`;
            }).join('')}
                </ul>
            </details>`;
        }

        // Field-level warning lookup: segName → Set of field indices flagged as MISSING_FIELD.
        const missingFieldByseg = new Map();
        for (const w of warnings) {
            if (w.code === 'MISSING_FIELD' && w.segment != null && w.field != null) {
                if (!missingFieldByseg.has(w.segment)) missingFieldByseg.set(w.segment, new Set());
                missingFieldByseg.get(w.segment).add(w.field);
            }
        }

        content.innerHTML = typicalChecklist + validationBanner + msg.segments.map((seg, segIdx) => {
            const key = `${msg.id}-${segIdx}`;
            const collapsed = collapsedSegments.has(key);
            const icon = collapsed ? ICONS.chevronRight : ICONS.chevronDown;
            const warnFields = missingFieldByseg.get(seg.name);
            return `
            <div class="segment-block">
                <div class="segment-name ${seg.description ? 'has-seg-tooltip' : ''}" data-seg-key="${key}"${seg.description ? ` data-desc="${escAttr(seg.name + ': ' + seg.description)}"` : ''} role="button" tabindex="0" aria-expanded="${!collapsed}">
                    <span class="collapse-icon">${icon}</span>
                    ${esc(seg.name)}
                    <span class="field-count">(${seg.fields.length})</span>
                    <span class="copy-btn" onclick="event.stopPropagation(); copySegment(${segIdx}, this)" title="Copy segment" role="button" tabindex="0" aria-label="Copy segment">${ICONS.copy}</span>
                </div>
                ${collapsed ? '' : `<table class="field-table">
                    <tbody>
                    ${seg.fields.map(f => {
                const trCls = warnFields && warnFields.has(f.index) ? ' class="warn"' : '';
                const descLine = f.description ? `<span class="desc-text">${esc(f.description)}</span>` : '';
                return `
                        <tr${trCls}>
                            <td class="field-idx">${esc(seg.name)}-${f.index}${descLine}</td>
                            <td class="field-val" ${f.value ? 'role="button" tabindex="0" aria-label="Copy field value"' : ''}>${esc(f.value) || '<span class="field-empty">empty</span>'}</td>
                            <td class="field-components">${f.components.length > 1
                        ? f.components.map((c, i) => `<span title="${escAttr(seg.name + '-' + f.index + '.' + (i + 1))}">${esc(c)}</span>`).join(' <span style="color:var(--text-muted)">^</span> ')
                        : ''
                    }</td>
                        </tr>`;
            }).join('')}
                    </tbody>
                </table>`}
            </div>`;
        }).join('');
    } else if (activeTab === 'raw') {
        const lines = msg.raw.split(/\r?\n|\r/).filter(l => l.trim());
        content.innerHTML = `
            <div style="display:flex;justify-content:flex-end;margin-bottom:8px;">
                <button class="copy-raw-btn" onclick="copyRawMessage(this)" title="Copy entire message">${ICONS.copy} Copy All</button>
            </div>
            <div class="raw-view">${lines.map(line => {
            const segName = line.substring(0, 3);
            return `<div class="segment-line"><span style="color:var(--accent);font-weight:600">${esc(segName)}</span>${esc(line.substring(3))}</div>`;
        }).join('')
            }</div>`;
    } else if (activeTab === 'ack') {
        const ack = msg.ack_response;
        if (!ack) {
            content.innerHTML = `<div class="empty-state"><p>No ACK was generated for this message</p></div>`;
        } else {
            const lines = ack.split(/\r?\n|\r/).filter(l => l.trim());
            content.innerHTML = `<div class="raw-view">${lines.map(line => {
                const segName = line.substring(0, 3);
                return `<div class="segment-line"><span style="color:var(--accent);font-weight:600">${esc(segName)}</span>${esc(line.substring(3))}</div>`;
            }).join('')
                }</div>`;
        }
    } else if (activeTab === 'json') {
        content.innerHTML = `<pre class="raw-view">${esc(JSON.stringify(msg, null, 2))}</pre>`;
    } else if (activeTab === 'diff') {
        renderDiffTab(content, msg);
    }
}

function renderDiffTab(container, msgB) {
    const msgA = diffPinnedMessage;
    if (!msgA) {
        container.innerHTML = '<div class="empty-state"><p>No reference message pinned.</p></div>';
        return;
    }

    // Build lookup: segName → segment for each message
    // If a segment appears multiple times, index by name+occurrence
    function segKey(seg, idx) { return `${seg.name}#${idx}`; }

    // Collect all segment names (union, preserving order: A first, then B-only)
    const segsA = msgA.segments || [];
    const segsB = msgB.segments || [];
    const allSegNames = [];
    const seen = new Set();
    [...segsA, ...segsB].forEach(s => { if (!seen.has(s.name)) { seen.add(s.name); allSegNames.push(s.name); } });

    // For each segment name, pair the first occurrence in A and B
    function firstSeg(segs, name) { return segs.find(s => s.name === name); }

    let html = `
        <div class="diff-header">
            <div class="diff-col-label diff-label-a">
                &#128204; Reference: <strong>${esc(msgA.message_type)}</strong>
                <span class="diff-meta">${esc(msgA.message_control_id)}</span>
            </div>
            <div class="diff-col-label diff-label-b">
                &#10145; Current: <strong>${esc(msgB.message_type)}</strong>
                <span class="diff-meta">${esc(msgB.message_control_id)}</span>
            </div>
        </div>
    `;

    let totalDiffs = 0;
    let hiddenDynamic = 0;

    for (const segName of allSegNames) {
        const segA = firstSeg(segsA, segName);
        const segB = firstSeg(segsB, segName);

        if (!segA && !segB) continue;

        // Collect all field indices (union)
        const allIdxs = new Set();
        (segA ? segA.fields : []).forEach(f => allIdxs.add(f.index));
        (segB ? segB.fields : []).forEach(f => allIdxs.add(f.index));
        const sortedIdxs = Array.from(allIdxs).sort((a, b) => a - b);

        const missingA = !segA;
        const missingB = !segB;
        const segClass = (missingA || missingB) ? 'diff-segment-missing' : 'diff-segment';

        let rows = '';
        let segHasDiff = missingA || missingB;

        for (const idx of sortedIdxs) {
            const fieldKey = `${segName}-${idx}`;
            const fA = segA ? segA.fields.find(f => f.index === idx) : null;
            const fB = segB ? segB.fields.find(f => f.index === idx) : null;
            const vA = fA ? fA.value : '';
            const vB = fB ? fB.value : '';
            const changed = vA !== vB;

            if (diffIgnoreDynamic && DYNAMIC_DIFF_FIELDS.has(fieldKey)) {
                if (changed) hiddenDynamic++;
                continue;
            }

            const desc = (fA && fA.description) || (fB && fB.description) || '';
            if (changed) { totalDiffs++; segHasDiff = true; }
            const rowClass = changed ? 'diff-row changed' : 'diff-row same';
            rows += `
                <tr class="${rowClass}">
                    <td class="diff-field-name" title="${escAttr(desc)}">${esc(segName)}-${idx}${desc ? ' <span class="diff-desc">' + esc(desc) + '</span>' : ''}</td>
                    <td class="diff-val diff-val-a ${changed ? 'diff-changed' : ''}">${esc(vA) || '<span class="field-empty">empty</span>'}</td>
                    <td class="diff-val diff-val-b ${changed ? 'diff-changed' : ''}">${esc(vB) || '<span class="field-empty">empty</span>'}</td>
                </tr>`;
        }

        html += `
            <div class="diff-segment-block${segHasDiff ? ' has-diff' : ''}">
                <div class="diff-segment-name ${segClass}">
                    ${esc(segName)}
                    ${missingA ? '<span class="diff-missing-badge">only in current</span>' : ''}
                    ${missingB ? '<span class="diff-missing-badge">only in reference</span>' : ''}
                </div>
                <table class="diff-table">
                    <thead><tr>
                        <th class="diff-field-col">Field</th>
                        <th>Reference value</th>
                        <th>Current value</th>
                    </tr></thead>
                    <tbody>${rows}</tbody>
                </table>
            </div>`;
    }

    const hiddenNote = hiddenDynamic > 0 ? ` (${hiddenDynamic} dynamic hidden)` : '';
    const summary = totalDiffs === 0
        ? `<div class="diff-summary same">&#10003; Messages are identical${hiddenNote}</div>`
        : `<div class="diff-summary changed">&#9651; ${totalDiffs} field difference${totalDiffs > 1 ? 's' : ''} found${hiddenNote}</div>`;

    const optionsBar = `
        <div class="diff-options-bar">
            <label class="theme-toggle" style="cursor:pointer; display:flex; align-items:center; gap:8px;">
                <input type="checkbox" onchange="toggleDiffIgnoreDynamic(event)" style="display:none" ${diffIgnoreDynamic ? 'checked' : ''}>
                <span class="toggle-slider"></span>
                Hide dynamic fields (MSH-7, MSH-10)
            </label>
        </div>`;

    container.innerHTML = summary + optionsBar + html;
}

function toggleDiffIgnoreDynamic(e) {
    diffIgnoreDynamic = e.target.checked;
    renderTab();
    saveSession();
}

function toggleSegment(key) {
    if (collapsedSegments.has(key)) {
        collapsedSegments.delete(key);
    } else {
        collapsedSegments.add(key);
    }
    renderTab();
    saveSession();
}

// --- Actions ---
function toggleAutoscroll() {
    autoscroll = !autoscroll;
    const btn = document.getElementById('btn-autoscroll');
    if (!btn) return;
    btn.classList.toggle('on', autoscroll);
    btn.classList.toggle('ok-on', autoscroll);
    saveSession();
}

// Task 2: pause/resume live updates
function togglePause() {
    paused = !paused;
    const btn = document.getElementById('btn-pause');
    if (!btn) return;
    btn.classList.toggle('on', paused);
    btn.classList.toggle('warn-on', paused);
    if (paused) {
        btn.innerHTML = `${ICONS.play}<span class="btn-label">Live</span>`;
    } else {
        btn.innerHTML = `${ICONS.pause}<span class="btn-label">Pause</span>`;
        flushAndRender();
    }
    saveSession();
}

async function exportMessages() {
    try {
        const resp = await fetch('/api/messages?limit=100000');
        if (!resp.ok) throw new Error(`Server error: ${resp.status}`);
        const data = await resp.json();
        const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' });
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = `harux-export-${new Date().toISOString().slice(0, 19)}.json`;
        a.click();
        URL.revokeObjectURL(url);
    } catch (e) {
        showToast('Export failed: ' + (e.message || e));
    }
}

async function clearMessages() {
    if (!confirm('Delete all messages?')) return;
    try {
        const resp = await fetch('/api/clear', { method: 'POST' });
        if (!resp.ok) throw new Error(`Server error: ${resp.status}`);
        messages = [];
        pendingMessages = [];
        rateWindow.length = 0;
        lastMessageReceivedAt = null;
        selectedId = null;
        selectedMessage = null;
        renderMessageList();
        renderSourceLegend();
        renderHealthPills();
        updateHeaderCounters();
        resetDetailHeader();
        document.getElementById('detail-content').innerHTML = DETAIL_EMPTY_HTML;
    } catch (e) {
        console.error('Failed to clear messages:', e);
    }
}

async function addTag(id, tag) {
    if (!tag || !tag.trim()) return;
    try {
        const resp = await fetch(`/api/messages/${id}/tags`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ tag: tag.trim() })
        });
        if (!resp.ok) throw new Error('Failed to add tag');
    } catch (e) {
        showToast(e.message);
    }
}

async function removeTag(id, tag) {
    try {
        const resp = await fetch(`/api/messages/${id}/tags/${encodeURIComponent(tag)}`, {
            method: 'DELETE'
        });
        if (!resp.ok) throw new Error('Failed to remove tag');
    } catch (e) {
        showToast(e.message);
    }
}

async function toggleBookmark(id, event) {
    event.stopPropagation();
    try {
        const resp = await fetch(`/api/messages/${id}/bookmark`, { method: 'POST' });
        if (!resp.ok) throw new Error('Failed to toggle bookmark');
    } catch (e) {
        showToast(e.message);
    }
}

function toggleBookmarkFilter() {
    showBookmarkedOnly = !showBookmarkedOnly;
    const btn = document.getElementById('btn-bookmarks');
    if (!btn) return;
    btn.classList.toggle('on', showBookmarkedOnly);
    btn.classList.toggle('warn-on', showBookmarkedOnly);
    const labelHtml = `<span class="btn-label">Bookmarks</span>`;
    const icon = showBookmarkedOnly ? ICONS.starFilled : ICONS.starOutline;
    const existingCount = btn.querySelector('.count');
    btn.innerHTML = `${icon}${labelHtml}` + (existingCount ? existingCount.outerHTML : '');
    renderMessageList();
    saveSession();
}

function syncValidationFilterUI() {
    const btn = document.getElementById('btn-validation');
    if (!btn) return;
    btn.classList.remove('on', 'warn-on', 'err-on');
    const baseHtml = `${ICONS.warning}<span class="btn-label">`;
    if (validationFilter === 0) {
        btn.innerHTML = `${baseHtml}All</span>`;
    } else if (validationFilter === 1) {
        btn.innerHTML = `${baseHtml}Warn</span>`;
        btn.classList.add('on', 'warn-on');
    } else if (validationFilter === 2) {
        btn.innerHTML = `${baseHtml}Error</span>`;
        btn.classList.add('on', 'err-on');
    }
}

function toggleValidationFilter() {
    validationFilter = (validationFilter + 1) % 3;
    syncValidationFilterUI();
    renderMessageList();
    saveSession();
}

// --- Utility ---
function showToast(message, type = 'error') {
    const toast = document.createElement('div');
    toast.className = `toast ${type}`;
    toast.textContent = message;
    document.body.appendChild(toast);
    setTimeout(() => toast.remove(), 4000);
}

function esc(str) {
    if (!str) return '';
    const div = document.createElement('div');
    div.textContent = str;
    return div.innerHTML;
}

function escAttr(str) {
    if (!str) return '';
    return str.replace(/&/g, '&amp;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#x27;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;');
}

function escJS(str) {
    if (!str) return '';
    return str.replace(/\\/g, '\\\\')
        .replace(/'/g, '\\\'')
        .replace(/"/g, '\\"')
        .replace(/\n/g, '\\n')
        .replace(/\r/g, '\\r');
}

// --- Copy to Clipboard ---
async function copyToClipboard(text, feedbackEl) {
    try {
        await navigator.clipboard.writeText(text);
        if (feedbackEl) {
            feedbackEl.classList.add('copy-success');
            setTimeout(() => feedbackEl.classList.remove('copy-success'), 1500);
        }
    } catch (e) {
        showToast('Copy failed: ' + e.message);
    }
}

function copySegment(segIdx, el) {
    if (!selectedMessage) return;
    const seg = selectedMessage.segments[segIdx];
    if (seg) copyToClipboard(seg.raw, el);
}

function copyRawMessage(el) {
    if (!selectedMessage) return;
    copyToClipboard(selectedMessage.raw, el);
}

function copyCliSnippet(btn) {
    const snippetEl = document.getElementById('cli-snippet');
    if (!snippetEl) return;
    const text = snippetEl.textContent.replace(/\s+/g, ' ').trim();
    copyToClipboard(text, btn);
}

// --- Panel Splitter ---
const SPLITTER_STORAGE_KEY = 'harux_splitter_width';
const SPLITTER_DEFAULT_RATIO = 0.55;
const SPLITTER_MIN_PX = 300;
const SPLITTER_MAX_RATIO = 0.80;

function initSplitter() {
    const splitter = document.getElementById('panel-splitter');
    const listPanel = document.querySelector('.list-panel');
    const container = document.querySelector('.main-container');

    if (!splitter || !listPanel || !container) return;

    // Restore saved width
    const saved = localStorage.getItem(SPLITTER_STORAGE_KEY);
    if (saved) {
        const px = parseInt(saved, 10);
        if (!isNaN(px) && px >= SPLITTER_MIN_PX) {
            listPanel.style.flexBasis = px + 'px';
        }
    }

    let dragging = false;

    function onDragStart(e) {
        e.preventDefault();
        dragging = true;
        document.body.classList.add('resizing');
        document.addEventListener('mousemove', onDragMove);
        document.addEventListener('mouseup', onDragEnd);
        document.addEventListener('touchmove', onDragMove, { passive: false });
        document.addEventListener('touchend', onDragEnd);
    }

    function onDragMove(e) {
        if (!dragging) return;
        if (e.type === 'touchmove') e.preventDefault();

        const clientX = e.touches ? e.touches[0].clientX : e.clientX;
        const rect = container.getBoundingClientRect();
        const maxPx = rect.width * SPLITTER_MAX_RATIO;

        let newWidth = clientX - rect.left;
        newWidth = Math.max(SPLITTER_MIN_PX, Math.min(newWidth, maxPx));

        listPanel.style.flexBasis = newWidth + 'px';
    }

    function onDragEnd() {
        if (!dragging) return;
        dragging = false;
        document.body.classList.remove('resizing');
        document.removeEventListener('mousemove', onDragMove);
        document.removeEventListener('mouseup', onDragEnd);
        document.removeEventListener('touchmove', onDragMove);
        document.removeEventListener('touchend', onDragEnd);

        // Persist width
        const currentWidth = listPanel.getBoundingClientRect().width;
        localStorage.setItem(SPLITTER_STORAGE_KEY, Math.round(currentWidth));
    }

    // Double-click resets to default
    splitter.addEventListener('dblclick', () => {
        listPanel.style.flexBasis = (SPLITTER_DEFAULT_RATIO * 100) + '%';
        localStorage.removeItem(SPLITTER_STORAGE_KEY);
    });

    splitter.addEventListener('mousedown', onDragStart);
    splitter.addEventListener('touchstart', onDragStart, { passive: false });
}

// --- Init ---

// Restore session state BEFORE first render so restored values take effect
loadSession();

// Search is purely client-side (filters the local `messages` array via matchesSearch).
// The debounce is a forward-looking safeguard: if a future /api/search call is added,
// rapid keystrokes would otherwise hammer the server and cause RwLock contention on the
// Rust side. Local renderMessageList() remains immediately reactive inside the handler.
let _searchDebounceTimer = null;
document.getElementById('search-input').addEventListener('input', (e) => {
    clearTimeout(_searchDebounceTimer);
    _searchDebounceTimer = setTimeout(() => {
        searchQuery = e.target.value;
        renderMessageList();
        saveSession();
    }, 300);
});

function handleInteraction(e) {
    const copyBtn = e.target.closest('.copy-btn');
    if (e.type === 'keydown' && copyBtn) {
        e.preventDefault();
        copyBtn.click();
        return;
    }

    const segEl = e.target.closest('.segment-name');
    if (segEl && !copyBtn) toggleSegment(segEl.dataset.segKey);

    const cell = e.target.closest('.field-val');
    if (cell && !cell.querySelector('.field-empty')) {
        copyToClipboard(cell.textContent.trim(), cell);
    }
}

document.addEventListener('click', handleInteraction);
document.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === ' ') {
        const activeEl = document.activeElement;
        if (activeEl && (activeEl.classList.contains('segment-name') || activeEl.classList.contains('field-val') || activeEl.classList.contains('copy-btn'))) {
            e.preventDefault();
            handleInteraction({
                target: activeEl,
                type: 'keydown',
                preventDefault: () => e.preventDefault()
            });
        }
    }
});

// Apply restored session state to UI elements
(function applyRestoredSession() {
    if (searchQuery) document.getElementById('search-input').value = searchQuery;

    if (activeTab !== 'parsed') {
        document.querySelectorAll('.detail-tab').forEach(t => {
            t.classList.toggle('active', t.dataset.tab === activeTab);
        });
    }

    if (paused) {
        const btn = document.getElementById('btn-pause');
        btn.classList.add('on', 'warn-on');
        btn.innerHTML = `${ICONS.play}<span class="btn-label">Live</span>`;
    }

    if (showBookmarkedOnly) {
        const btn = document.getElementById('btn-bookmarks');
        btn.classList.add('on', 'warn-on');
        btn.innerHTML = `${ICONS.starFilled}<span class="btn-label">Bookmarks</span>`;
    }

    syncValidationFilterUI();
})();

initSplitter();
// Set autoscroll visual state — toggleAutoscroll flips the value, so pre-flip it
autoscroll = !autoscroll;
toggleAutoscroll();
connectWs();
setInterval(pollStats, 3000);
setInterval(() => {
    renderHealthPills();
    tickRowRelativeTimes();
}, 1000);
renderHealthPills();
renderSourceLegend();
updateHeaderCounters();
pollStats();

// Search shortcut: Cmd/Ctrl+K focuses the filter input. The hint chip
// inside the input adapts to the platform (⌘K on Mac, ^K elsewhere).
(function setupSearchShortcut() {
    const isMac = /Mac|iPod|iPhone|iPad/.test(navigator.platform || '');
    const tip = document.getElementById('search-tip');
    if (tip) tip.textContent = isMac ? '⌘K' : '^K';

    document.addEventListener('keydown', (e) => {
        if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'k') {
            const input = document.getElementById('search-input');
            if (!input) return;
            e.preventDefault();
            input.focus();
            input.select();
        }
    });
})();
