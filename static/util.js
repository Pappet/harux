// Constants and pure utility helpers shared across the modules. No DOM
// rendering, no network calls — anything stateful that runs once at startup
// (sessionId) lives here because it's used by saveSession/loadSession.

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

const MONTH_SHORT = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

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

// Fields that change every transmission — used by the diff view to suppress
// noise when the user enables "Hide dynamic fields".
const DYNAMIC_DIFF_FIELDS = new Set(['MSH-7', 'MSH-10']);

// --- Session Persistence ---
const SESSION_KEY = 'harux_session';
const SESSION_STATE_KEY = SESSION_KEY + '_state';
const sessionId = sessionStorage.getItem(SESSION_KEY) || crypto.randomUUID();
sessionStorage.setItem(SESSION_KEY, sessionId);

function saveSession() {
    try {
        sessionStorage.setItem(SESSION_STATE_KEY, JSON.stringify({
            selectedId: state.selectedId,
            activeTab: state.activeTab,
            autoscroll: state.autoscroll,
            searchQuery: state.searchQuery,
            paused: state.paused,
            collapsedSegments: [...state.collapsedSegments],
            colorByPort: state.colorByPort,
            highlightedSource: state.highlightedSource,
            showBookmarkedOnly: state.showBookmarkedOnly,
            validationFilter: state.validationFilter,
            diffIgnoreDynamic: state.diffIgnoreDynamic,
            diffPinnedId: state.diffPinnedMessage ? state.diffPinnedMessage.id : null
        }));
    } catch (_) { /* sessionStorage full or unavailable */ }
}

function loadSession() {
    try {
        const raw = sessionStorage.getItem(SESSION_STATE_KEY);
        if (!raw) return;
        const saved = JSON.parse(raw);
        if (saved.selectedId != null) state.selectedId = saved.selectedId;
        if (saved.activeTab) state.activeTab = saved.activeTab;
        if (typeof saved.autoscroll === 'boolean') state.autoscroll = saved.autoscroll;
        if (typeof saved.searchQuery === 'string') state.searchQuery = saved.searchQuery;
        if (typeof saved.paused === 'boolean') state.paused = saved.paused;
        if (typeof saved.colorByPort === 'boolean') state.colorByPort = saved.colorByPort;
        if (saved.highlightedSource !== undefined) state.highlightedSource = saved.highlightedSource;
        if (typeof saved.showBookmarkedOnly === 'boolean') state.showBookmarkedOnly = saved.showBookmarkedOnly;
        if (typeof saved.validationFilter === 'number') state.validationFilter = saved.validationFilter;
        if (typeof saved.diffIgnoreDynamic === 'boolean') state.diffIgnoreDynamic = saved.diffIgnoreDynamic;
        // The full pinned message can be multi-MB (raw + segments), so we
        // persist just the ID and re-fetch from /api/messages/:id during
        // loadMessages(). Stashed on state for the loader to pick up.
        if (typeof saved.diffPinnedId === 'string') state._pendingDiffPinnedId = saved.diffPinnedId;
        if (Array.isArray(saved.collapsedSegments)) {
            state.collapsedSegments = new Set(saved.collapsedSegments);
        }
    } catch (_) { /* corrupted or unavailable */ }
}

// --- Escaping helpers ---
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

// --- Toast + clipboard ---
function showToast(message, type = 'error') {
    const toast = document.createElement('div');
    toast.className = `toast ${type}`;
    toast.textContent = message;
    document.body.appendChild(toast);
    setTimeout(() => toast.remove(), 4000);
}

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

// --- Source color mapping ---
function hashString(str) {
    let hash = 0;
    for (let i = 0; i < str.length; i++) {
        hash = ((hash << 5) - hash + str.charCodeAt(i)) | 0;
    }
    return Math.abs(hash);
}

function getSourceColor(addr) {
    if (!addr) return 'var(--text-muted)';
    const colorKey = state.colorByPort ? addr : addr.split(':')[0];
    return SOURCE_PALETTE[hashString(colorKey) % SOURCE_PALETTE.length];
}

function srcLabelFor(addr) {
    if (!addr) return '';
    return state.colorByPort ? addr : addr.split(':')[0];
}

// --- Time formatting ---
function formatRelativeTime(ms) {
    if (ms < 1000) return 'just now';
    if (ms < 60000) return `${Math.floor(ms / 1000)}s ago`;
    if (ms < 3600000) return `${Math.floor(ms / 60000)}m ago`;
    return `${Math.floor(ms / 3600000)}h ago`;
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
    const [, m, d] = key.split('-').map(Number);
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

// Cache for the JSON tab's stringified payload. WeakMap keys the cache on
// the message object itself so we don't mutate the data model with a
// presentation-layer property, and entries are reclaimed automatically when
// a message is no longer referenced (e.g. after switching selection).
const detailJsonCache = new WeakMap();
