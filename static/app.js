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

function renderSourceLegend() {
    const container = document.getElementById('source-legend');
    if (!container) return;
    if (seenSources.size === 0) {
        container.style.display = 'none';
        return;
    }
    container.style.display = 'flex';

    const uniqueLabels = new Set();
    seenSources.forEach(addr => {
        uniqueLabels.add(colorByPort ? addr : addr.split(':')[0]);
    });

    const sortedLabels = Array.from(uniqueLabels).sort();

    let html = sortedLabels.map(label => {
        const color = SOURCE_PALETTE[hashString(label) % SOURCE_PALETTE.length];
        const isHighlighted = highlightedSource === label;
        const isDimmed = highlightedSource && highlightedSource !== label;
        const classes = `source-legend-item${isHighlighted ? ' highlighted' : ''}${isDimmed ? ' dimmed' : ''}`;

        return `<span class="${classes}" onclick="toggleHighlightSource('${escAttr(escJS(label))}')">
            <span class="source-dot" style="background:${color};box-shadow:0 0 4px ${color}"></span>
            ${esc(label)}
        </span>`;
    }).join('');

    html += `
        <label class="theme-toggle" style="margin-left:auto; cursor:pointer; display:flex; align-items:center; gap:8px; color:var(--text-muted)">
            <input type="checkbox" onchange="toggleColorByPort(event)" style="display:none" ${colorByPort ? 'checked' : ''}>
            <span class="toggle-slider"></span>
            Color by Port
        </label>
    `;
    container.innerHTML = html;
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
        document.getElementById('ws-dot').className = 'stat-dot green';
        document.getElementById('ws-status').textContent = 'Connected';
    };

    ws.onclose = () => {
        document.getElementById('ws-dot').className = 'stat-dot red';
        const jitter = wsReconnectDelay * (0.75 + Math.random() * 0.5);
        const delaySec = Math.round(jitter / 1000);
        document.getElementById('ws-status').textContent = `Reconnecting in ${delaySec}s\u2026`;
        setTimeout(connectWs, jitter);
        wsReconnectDelay = Math.min(wsReconnectDelay * WS_RECONNECT_MULT, WS_RECONNECT_MAX);
    };

    ws.onerror = (event) => {
        console.error('WebSocket error:', event);
        document.getElementById('ws-dot').className = 'stat-dot red';
        document.getElementById('ws-status').textContent = 'Error';
    };

    ws.onmessage = (event) => {
        const data = JSON.parse(event.data);
        if (data.type === 'init') {
            totalMessagesCount = data.total;
            document.getElementById('stat-total').textContent = totalMessagesCount;
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
            selectedId = null;
            selectedMessage = null;
            renderMessageList();
            document.getElementById('detail-content').innerHTML = '<div class="empty-state"><p>No message selected</p></div>';
            document.getElementById('detail-title').textContent = 'Select a message';
            document.getElementById('detail-meta').textContent = '';
            document.getElementById('stat-total').textContent = '0';
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
    document.getElementById('stat-total').textContent = totalMessagesCount;
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
    document.getElementById('stat-total').textContent = totalMessagesCount;
    renderMessageList();
    renderSourceLegend();
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
        document.getElementById('stat-total').textContent = totalMessagesCount;
        renderMessageList();
        renderSourceLegend();
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
        document.getElementById('stat-total').textContent = totalMessagesCount;
        document.getElementById('stat-connections').textContent =
            `${stats.active_connections} / ${stats.max_connections}`;
        document.getElementById('stat-errors').textContent = stats.parse_errors;
        const rejectedEl = document.getElementById('stat-rejected');
        if (rejectedEl) {
            if (stats.rejected_connections > 0) {
                rejectedEl.parentElement.style.display = '';
                rejectedEl.textContent = stats.rejected_connections;
            } else {
                rejectedEl.parentElement.style.display = 'none';
            }
        }
        if (stats.mllp_port) {
            document.getElementById('mllp-port').textContent = stats.mllp_port;
        }
    } catch (e) { }
}

// --- Rendering ---
function renderMessageList() {
    const list = document.getElementById('message-list');
    const empty = document.getElementById('empty-state');
    let filtered = searchQuery
        ? messages.filter(m => matchesSearch(m, searchQuery))
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
        list.querySelectorAll('.message-row').forEach(r => r.remove());
        return;
    }

    empty.style.display = 'none';

    const fragment = document.createDocumentFragment();
    for (const msg of filtered) {
        const row = document.createElement('div');

        let rowClass = 'message-row';
        if (msg.id === selectedId) rowClass += ' selected';

        const srcKey = colorByPort ? msg.source_addr : (msg.source_addr ? msg.source_addr.split(':')[0] : '');
        if (highlightedSource && srcKey !== highlightedSource) {
            rowClass += ' dimmed';
        }

        row.className = rowClass;
        row.dataset.id = msg.id;
        row.tabIndex = 0;
        row.setAttribute('role', 'button');
        row.setAttribute('aria-label', `Message from ${msg.sending_facility}, type ${msg.message_type}, received at ${msg.received_at}`);

        const handleClick = () => selectMessage(msg.id);
        row.onclick = handleClick;
        row.addEventListener('keydown', (e) => {
            if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                handleClick();
            }
        });

        const time = new Date(msg.received_at);
        const yyyy = time.getFullYear();
        const mm = String(time.getMonth() + 1).padStart(2, '0');
        const dd = String(time.getDate()).padStart(2, '0');
        const hh = String(time.getHours()).padStart(2, '0');
        const min = String(time.getMinutes()).padStart(2, '0');
        const ss = String(time.getSeconds()).padStart(2, '0');
        const timeStr = `${yyyy}-${mm}-${dd} ${hh}:${min}:${ss}`;
        // Source color dot
        const srcColor = getSourceColor(msg.source_addr);
        const dotHtml = `<span class="source-dot" style="background:${srcColor};box-shadow:0 0 4px ${srcColor}" title="${escAttr(msg.source_addr)}"></span>`;

        // Validation badge: red if missing segments (errors), yellow for field warnings only
        const warnCount = msg.validation_warning_count || 0;
        const warnCls = msg.has_segment_errors ? 'validation-badge error' : 'validation-badge';
        const warnBadge = warnCount > 0
            ? ` <span class="${warnCls}" title="${warnCount} validation warning${warnCount > 1 ? 's' : ''}">⚠ ${warnCount}</span>`
            : '';
        const typeHtml = msg.parse_error
            ? `<span class="msg-type" style="color:var(--error)" title="${escAttr(msg.parse_error)}">⚠ PARSE ERROR</span>`
            : `<span class="msg-type">${esc(msg.message_type)}${warnBadge}</span>`;

        const tagsHtml = (msg.tags && msg.tags.length > 0)
            ? `<div class="msg-tags-list">` + msg.tags.map(t => `<span class="msg-tag-small">${esc(t)}</span>`).join('') + `</div>`
            : '';

        let ackColor = '';
        if (msg.ack_code === 'AA') ackColor = 'color: var(--success);';
        else if (msg.ack_code === 'AE' || msg.ack_code === 'AR') ackColor = 'color: var(--error);';

        const ackHtml = msg.ack_code
            ? `<span class="msg-ack" style="${ackColor}">${esc(msg.ack_code)}</span>`
            : `<span class="msg-ack">—</span>`;

        const bookmarkClass = msg.bookmarked ? 'msg-bookmark active' : 'msg-bookmark';
        const bookmarkIcon = msg.bookmarked ? '★' : '☆';
        const bookmarkLabel = msg.bookmarked ? 'Remove bookmark' : 'Add bookmark';

        const isPinned = diffPinnedMessage && diffPinnedMessage.id === msg.id;
        const pinClass = isPinned ? 'msg-pin active' : 'msg-pin';
        const pinIcon = isPinned ? '◉' : '◎';
        const pinTitle = isPinned ? 'Unpin (diff reference)' : 'Pin as diff reference';

        row.innerHTML = `
            ${dotHtml}
            <div style="display:flex; flex-direction:column; gap:2px; overflow:hidden;">
                ${typeHtml}
                ${tagsHtml}
            </div>
            <span class="msg-source">${esc(msg.sending_facility)}</span>
            <span class="msg-patient">${esc(msg.patient_name || msg.patient_id || '—')}</span>
            <span class="msg-time">${timeStr}</span>
            <span class="msg-segs">${msg.segment_count}</span>
            ${ackHtml}
            <button class="${bookmarkClass}" aria-label="${bookmarkLabel}" onclick="toggleBookmark('${msg.id}', event)" title="Bookmark">${bookmarkIcon}</button>
            <button class="${pinClass}" aria-label="${pinTitle}" onclick="toggleDiffPin('${msg.id}', event)" title="${pinTitle}">${pinIcon}</button>
        `;
        fragment.appendChild(row);
    }

    list.querySelectorAll('.message-row').forEach(r => r.remove());
    list.appendChild(fragment);

    if (autoscroll) {
        list.scrollTop = 0;
    }
}

function matchesSearch(msg, query) {
    let q = query.toLowerCase().trim();
    if (q.startsWith('has:warnings')) {
        if ((msg.validation_warning_count || 0) === 0 && !msg.has_segment_errors) return false;
        q = q.replace('has:warnings', '').trim();
        if (!q) return true;
    } else if (q.startsWith('has:errors')) {
        if (!msg.has_segment_errors) return false;
        q = q.replace('has:errors', '').trim();
        if (!q) return true;
    }
    return (
        (msg.message_type || '').toLowerCase().includes(q) ||
        (msg.sending_facility || '').toLowerCase().includes(q) ||
        (msg.patient_name || '').toLowerCase().includes(q) ||
        (msg.patient_id || '').toLowerCase().includes(q) ||
        (msg.message_control_id || '').toLowerCase().includes(q) ||
        (msg.source_addr || '').toLowerCase().includes(q) ||
        (msg.tags || []).some(t => t.toLowerCase().includes(q))
    );
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

function renderDetail() {
    if (!selectedMessage) return;
    const msg = selectedMessage;

    document.getElementById('detail-title').textContent =
        `${msg.message_type} — ${msg.patient_name || msg.patient_id || 'Unknown'}`;

    const descEl = document.getElementById('detail-type-desc');
    if (descEl) {
        if (msg.message_type_description) {
            descEl.textContent = msg.message_type_description;
            descEl.style.display = '';
        } else {
            descEl.style.display = 'none';
        }
    }

    document.getElementById('detail-meta').textContent =
        `${msg.source_addr} | ${msg.message_control_id} | v${msg.version}${msg.charset ? ` | ${msg.charset}` : ''}`;

    const tagsContainer = document.getElementById('detail-tags');

    const bookmarkBtnClass = msg.bookmarked ? 'detail-bookmark-btn active' : 'detail-bookmark-btn';
    const bookmarkBtnIcon = msg.bookmarked ? '★' : '☆';

    const isPinned = diffPinnedMessage && diffPinnedMessage.id === msg.id;
    const pinBtnClass = isPinned ? 'detail-pin-btn active' : 'detail-pin-btn';
    const pinBtnLabel = isPinned ? '📌 Pinned' : '📌 Pin for Diff';

    tagsContainer.innerHTML = `
        <button class="${bookmarkBtnClass}" onclick="toggleBookmark('${msg.id}', event)" title="Toggle bookmark">${bookmarkBtnIcon} Bookmark</button>
        <button class="${pinBtnClass}" onclick="toggleDiffPin('${msg.id}')" title="Pin this message as the diff reference">${pinBtnLabel}</button>
    ` + (msg.tags || []).map(t =>
        `<span class="msg-tag">${esc(t)} <span class="msg-tag-remove" onclick="removeTag('${msg.id}', '${escAttr(escJS(t))}')">×</span></span>`
    ).join('') + `
        <div class="msg-tag-add">
            <input type="text" id="add-tag-input" placeholder="Add tag" onkeypress="if(event.key === 'Enter') addTag('${msg.id}', this.value)">
            <button onclick="addTag('${msg.id}', document.getElementById('add-tag-input').value)">+</button>
        </div>
    `;

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

        const typicalBanner = (msg.typical_segments && msg.typical_segments.length)
            ? `<div class="typical-segments-bar">
                <span class="typical-segments-label">Typical segments:</span>
                ${msg.typical_segments.map(s => {
                const present = msg.segments.some(seg => seg.name === s);
                const desc = (msg.typical_segment_descriptions || {})[s];
                let cls, titleText;
                if (missingSegWarnings[s]) {
                    cls = 'missing';
                    titleText = missingSegWarnings[s];
                } else if (fieldWarningSegs[s]) {
                    cls = 'warn';
                    titleText = (desc ? desc + ' — ' : '') + 'has required fields missing';
                } else if (present) {
                    cls = 'present';
                    titleText = desc || null;
                } else {
                    cls = 'absent';
                    titleText = desc || null;
                }
                const titleAttr = titleText ? ` title="${escAttr(titleText)}"` : '';
                return `<span class="typical-seg ${cls}"${titleAttr}>${esc(s)}</span>`;
            }).join('')}
               </div>`
            : '';
        const hasSegErrors = warnings.some(w => w.code === 'MISSING_SEGMENT');
        const panelCls = hasSegErrors ? 'validation-warnings-panel error' : 'validation-warnings-panel';
        const validationBanner = (msg.validation_warnings && msg.validation_warnings.length)
            ? `<div class="${panelCls}">
                <div class="validation-warnings-title">&#9888; Validation ${hasSegErrors ? 'Errors' : 'Warnings'} (${msg.validation_warnings.length})</div>
                <ul class="validation-warnings-list">
                ${msg.validation_warnings.map(w => {
                const badgeCls = w.code === 'MISSING_SEGMENT' ? 'validation-seg error'
                    : w.code === 'INVALID_DATATYPE' ? 'validation-seg type'
                    : 'validation-seg';
                const label = w.segment + (w.field != null ? '-' + w.field : '');
                return `<li><span class="${badgeCls}">${esc(label)}</span> ${esc(w.message)}</li>`;
            }).join('')}
                </ul>
               </div>`
            : '';
        content.innerHTML = typicalBanner + validationBanner + msg.segments.map((seg, segIdx) => {
            const key = `${msg.id}-${segIdx}`;
            const collapsed = collapsedSegments.has(key);
            const icon = collapsed ? '▸' : '▾';
            return `
            <div class="segment-block">
                <div class="segment-name ${seg.description ? 'has-seg-tooltip' : ''}" data-seg-key="${key}"${seg.description ? ` data-desc="${escAttr(seg.name + ': ' + seg.description)}"` : ''}>
                    <span class="collapse-icon">${icon}</span>
                    ${esc(seg.name)}
                    <span class="field-count">(${seg.fields.length})</span>
                    <span class="copy-btn" onclick="event.stopPropagation(); copySegment(${segIdx}, this)" title="Copy segment">📋</span>
                </div>
                ${collapsed ? '' : `<table class="field-table">
                    <thead><tr><th style="width:70px">Field</th><th>Value</th><th>Components</th></tr></thead>
                    <tbody>
                    ${seg.fields.map(f => `
                        <tr>
                            <td class="field-idx ${f.description ? 'has-tooltip' : ''}" ${f.description ? `data-desc="${escAttr(seg.name + '-' + f.index + ': ' + f.description)}"` : ''}>${esc(seg.name)}-${f.index}</td>
                            <td class="field-val">${esc(f.value) || '<span class="field-empty">empty</span>'}</td>
                            <td class="field-components">${f.components.length > 1
                    ? f.components.map((c, i) => `<span title="${escAttr(seg.name + '-' + f.index + '.' + (i + 1))}">${esc(c)}</span>`).join(' <span style="color:var(--text-muted)">^</span> ')
                    : ''
                }</td>
                        </tr>
                    `).join('')}
                    </tbody>
                </table>`}
            </div>`;
        }).join('');
    } else if (activeTab === 'raw') {
        const lines = msg.raw.split(/\r?\n|\r/).filter(l => l.trim());
        content.innerHTML = `
            <div style="display:flex;justify-content:flex-end;margin-bottom:8px;">
                <button class="copy-raw-btn" onclick="copyRawMessage(this)" title="Copy entire message">📋 Copy All</button>
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
    btn.style.borderColor = autoscroll ? 'var(--success)' : 'var(--border)';
    btn.style.color = autoscroll ? 'var(--success)' : 'var(--text-primary)';
    saveSession();
}

// Task 2: pause/resume live updates
function togglePause() {
    paused = !paused;
    const btn = document.getElementById('btn-pause');
    if (paused) {
        btn.textContent = '▶ Live';
        btn.style.borderColor = 'var(--warning)';
        btn.style.color = 'var(--warning)';
    } else {
        btn.textContent = '⏸ Pause';
        btn.style.borderColor = '';
        btn.style.color = '';
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
        selectedId = null;
        selectedMessage = null;
        renderMessageList();
        document.getElementById('detail-content').innerHTML = '<div class="empty-state"><p>No message selected</p></div>';
        document.getElementById('detail-title').textContent = 'Select a message';
        document.getElementById('detail-meta').textContent = '';
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
    if (showBookmarkedOnly) {
        btn.textContent = '★ Bookmarks';
        btn.style.borderColor = 'var(--warning)';
        btn.style.color = 'var(--warning)';
    } else {
        btn.textContent = '☆ Bookmarks';
        btn.style.borderColor = '';
        btn.style.color = '';
    }
    renderMessageList();
    saveSession();
}

function syncValidationFilterUI() {
    const btn = document.getElementById('btn-validation');
    if (!btn) return;
    if (validationFilter === 0) {
        btn.textContent = '⚠ All';
        btn.style.borderColor = '';
        btn.style.color = '';
    } else if (validationFilter === 1) {
        btn.textContent = '⚠ Warn';
        btn.style.borderColor = 'var(--warning)';
        btn.style.color = 'var(--warning)';
    } else if (validationFilter === 2) {
        btn.textContent = '⚠ Error';
        btn.style.borderColor = 'var(--error)';
        btn.style.color = 'var(--error)';
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

document.addEventListener('click', (e) => {
    const segEl = e.target.closest('.segment-name');
    if (segEl) toggleSegment(segEl.dataset.segKey);

    const cell = e.target.closest('.field-val');
    if (cell && !cell.querySelector('.field-empty')) {
        copyToClipboard(cell.textContent.trim(), cell);
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
        btn.textContent = '▶ Live';
        btn.style.borderColor = 'var(--warning)';
        btn.style.color = 'var(--warning)';
    }

    if (showBookmarkedOnly) {
        const btn = document.getElementById('btn-bookmarks');
        btn.textContent = '★ Bookmarks';
        btn.style.borderColor = 'var(--warning)';
        btn.style.color = 'var(--warning)';
    }

    syncValidationFilterUI();
})();

initSplitter();
// Set autoscroll visual state — toggleAutoscroll flips the value, so pre-flip it
autoscroll = !autoscroll;
toggleAutoscroll();
connectWs();
setInterval(pollStats, 3000);
