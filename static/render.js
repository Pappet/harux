// DOM rendering for the message list, source legend, health pills, and the
// detail panel (parsed / raw / ack / json tabs). Pure DOM updates — no
// network, no state mutation beyond presentational toggles like
// `collapsedSegments`.

// --- Source legend ---
function registerSource(addr) {
    if (addr) state.seenSources.add(addr);
}

function recomputeSourceCounts() {
    state.sourceCounts.clear();
    for (const m of state.messages) {
        const label = srcLabelFor(m.source_addr);
        if (!label) continue;
        state.sourceCounts.set(label, (state.sourceCounts.get(label) || 0) + 1);
    }
}

function renderSourceLegend() {
    const container = document.getElementById('source-rail');
    if (!container) return;

    if (state.seenSources.size === 0) {
        container.innerHTML = `
            <span class="label">Sources</span>
            <div class="source-rail-chips">
                <span class="source-rail-empty">none yet</span>
            </div>
            <details class="source-rail-overflow">
                <summary title="Source options">⋯</summary>
                <div class="popover">
                    <label>
                        <input type="checkbox" data-action="color-by-port" ${state.colorByPort ? 'checked' : ''}>
                        Color by Port
                    </label>
                </div>
            </details>
        `;
        return;
    }

    const uniqueLabels = new Set();
    state.seenSources.forEach(addr => uniqueLabels.add(srcLabelFor(addr)));
    const sortedLabels = Array.from(uniqueLabels).sort();

    const chipsHtml = sortedLabels.map(label => {
        const isActive = state.highlightedSource === label;
        const isDimmed = state.highlightedSource && state.highlightedSource !== label;
        const classes = `source-chip${isActive ? ' active' : ''}${isDimmed ? ' dimmed' : ''}`;
        const num = state.sourceCounts.get(label) || 0;
        return `<span class="${classes}" tabindex="0" role="button" aria-label="Filter by source ${escAttr(label)}" data-action="toggle-source" data-source="${escAttr(label)}">
            <span class="dot"></span>
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
                    <input type="checkbox" data-action="color-by-port" ${state.colorByPort ? 'checked' : ''}>
                    Color by Port
                </label>
            </div>
        </details>
    `;

    // Set dot colors via JS after innerHTML (avoids inline style= attributes)
    sortedLabels.forEach(label => {
        const chip = container.querySelector(`[data-source="${CSS.escape(label)}"]`);
        if (!chip) return;
        const color = SOURCE_PALETTE[hashString(label) % SOURCE_PALETTE.length];
        const dot = chip.querySelector('.dot');
        if (dot) { dot.style.background = color; dot.style.color = color; }
    });
}

// --- Health pills ---
function pruneRateWindow() {
    const cutoff = Date.now() - 60000;
    while (state.rateWindow.length > 0 && state.rateWindow[0] < cutoff) {
        state.rateWindow.shift();
    }
}

function renderRateSpark() {
    const buckets = new Array(10).fill(0);
    const now = Date.now();
    for (const t of state.rateWindow) {
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

function renderHealthPills() {
    pruneRateWindow();

    // rate pill — show only after first message
    const ratePill = document.getElementById('pill-rate');
    const rateValue = document.getElementById('pill-rate-value');
    const rateSpark = document.getElementById('pill-rate-spark');
    if (state.rateWindow.length > 0) {
        ratePill.style.display = '';
        rateValue.textContent = `${state.rateWindow.length}/min`;
        rateSpark.innerHTML = renderRateSpark();
    } else {
        ratePill.style.display = 'none';
    }

    // last pill — show only after first message
    const lastPill = document.getElementById('pill-last');
    const lastValue = document.getElementById('pill-last-value');
    if (state.lastMessageReceivedAt !== null) {
        lastPill.style.display = '';
        lastValue.textContent = formatRelativeTime(Date.now() - state.lastMessageReceivedAt);
    } else {
        lastPill.style.display = 'none';
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

// --- Header counters (total / validation / parse errors) ---
// `parse errors` is updated by pollStats from the server stat. The other two
// counters are derived from the in-memory message buffer.
function updateHeaderCounters() {
    const totalEl = document.getElementById('pill-total-value');
    if (totalEl) {
        totalEl.textContent = state.messages.length + state.pendingMessages.length;
    }

    let validationCount = 0;
    for (const m of state.messages) {
        validationCount += (m.validation_warning_count || 0);
        if (m.has_segment_errors) validationCount++;
    }
    const validationPill = document.getElementById('pill-validation');
    const validationValue = document.getElementById('pill-validation-value');
    if (validationValue) validationValue.textContent = validationCount;
    if (validationPill) {
        validationPill.classList.toggle('warn-pill', validationCount > 0);
    }

    const bookmarkCount = state.messages.reduce((n, m) => n + (m.bookmarked ? 1 : 0), 0);
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

// --- Message list ---
function buildMessageRow(msg) {
    const row = document.createElement('div');
    let rowClass = 'message-row';
    if (msg.id === state.selectedId) rowClass += ' selected';
    if (msg.bookmarked) rowClass += ' bookmarked';

    const srcKey = state.colorByPort ? msg.source_addr : (msg.source_addr ? msg.source_addr.split(':')[0] : '');
    if (state.highlightedSource && srcKey !== state.highlightedSource) {
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
        tagsHtml = `<span class="msg-tags-list">${visible}${overflow}</span>`;
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
        <div class="msg-source-bar" title="${escAttr(msg.source_addr || '')}"></div>
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
            <button class="${bookmarkClass}" aria-label="${bookmarkLabel}" data-action="bookmark" data-id="${msg.id}" title="Bookmark">${bookmarkSvg}</button>
        </div>
    `;
    // Set source bar color via JS after innerHTML (avoids inline style= attribute)
    const bar = row.querySelector('.msg-source-bar');
    if (bar) bar.style.background = srcColor;
    return row;
}

// True when no client-side filters are active, so new messages can be
// prepended to the DOM directly instead of rebuilding the whole list.
function canPrependOnly() {
    return !state.searchQuery && !state.showBookmarkedOnly && state.validationFilter === 0;
}

// Insert freshly-arrived messages at the top of the list without touching
// existing rows. Group-header continuity is preserved via a `data-bucket`
// attribute on each header.
function prependMessagesToList(newSummaries) {
    const list = document.getElementById('message-list');
    const empty = document.getElementById('empty-state');
    if (!list || !newSummaries.length) return;

    empty.style.display = 'none';

    const firstChild = list.firstElementChild;
    const firstExistingBucket = firstChild && firstChild.classList && firstChild.classList.contains('group-header')
        ? firstChild.dataset.bucket || null
        : null;

    const fragment = document.createDocumentFragment();
    const now = Date.now();
    let lastBucket = null;
    for (const msg of newSummaries) {
        const bucket = bucketKey(msg, now);
        if (bucket !== lastBucket) {
            lastBucket = bucket;
            if (bucket !== firstExistingBucket) {
                const header = document.createElement('div');
                header.className = 'group-header';
                header.dataset.bucket = bucket;
                header.textContent = bucketLabel(bucket);
                fragment.appendChild(header);
            }
        }
        fragment.appendChild(buildMessageRow(msg));
    }
    list.insertBefore(fragment, list.firstChild);

    if (state.autoscroll) {
        list.scrollTop = 0;
    }
}

// Patch a single existing row's mutable fields (tags / bookmark) without
// re-rendering the whole list. Safe no-op if the row is not currently
// in the DOM (e.g. filtered out).
function patchRow(id, mutations) {
    const list = document.getElementById('message-list');
    if (!list) return;
    const row = list.querySelector(`.message-row[data-id="${CSS.escape(id)}"]`);
    if (!row) return;

    if ('bookmarked' in mutations) {
        const on = !!mutations.bookmarked;
        row.classList.toggle('bookmarked', on);
        const btn = row.querySelector('.msg-bookmark');
        if (btn) {
            btn.classList.toggle('active', on);
            btn.innerHTML = on ? ICONS.starFilled : ICONS.starOutline;
            btn.setAttribute('aria-label', on ? 'Remove bookmark' : 'Add bookmark');
        }
    }

    if ('tags' in mutations) {
        const row1 = row.querySelector('.msg-row1');
        if (!row1) return;
        const oldTags = row1.querySelector('.msg-tags-list');
        if (oldTags) oldTags.remove();
        const tagsArr = mutations.tags || [];
        if (tagsArr.length === 0) return;
        const tagsEl = document.createElement('span');
        tagsEl.className = 'msg-tags-list';
        tagsEl.style.marginTop = '0';
        const visible = tagsArr.slice(0, 2);
        for (const t of visible) {
            const span = document.createElement('span');
            span.className = 'msg-tag-small';
            span.textContent = t;
            tagsEl.appendChild(span);
        }
        if (tagsArr.length > 2) {
            const over = document.createElement('span');
            over.className = 'msg-tag-small';
            over.textContent = '+' + (tagsArr.length - 2);
            tagsEl.appendChild(over);
        }
        // Insert before the ACK chip to match buildMessageRow's ordering.
        const ackEl = row1.querySelector('.msg-ack');
        if (ackEl) row1.insertBefore(tagsEl, ackEl);
        else row1.appendChild(tagsEl);
    }
}

// Move the .selected class from the previously-selected row to the new one.
function updateRowSelection(prevId, newId) {
    const list = document.getElementById('message-list');
    if (!list) return;
    if (prevId) {
        const prev = list.querySelector(`.message-row[data-id="${CSS.escape(prevId)}"]`);
        if (prev) prev.classList.remove('selected');
    }
    if (newId) {
        const next = list.querySelector(`.message-row[data-id="${CSS.escape(newId)}"]`);
        if (next) next.classList.add('selected');
    }
}

function renderMessageList() {
    const list = document.getElementById('message-list');
    const empty = document.getElementById('empty-state');

    // Lifted search query parsing out of the filter loop.
    // Reduces query parse operations from O(N) to O(1), significantly
    // speeding up list rendering when searching large message buffers.
    let parsedSearchQuery = null;
    if (state.searchQuery) {
        parsedSearchQuery = getParsedQuery(state.searchQuery);
    }

    let filtered = state.searchQuery
        ? state.messages.filter(m => matchesSearch(m, parsedSearchQuery))
        : state.messages;
    if (state.showBookmarkedOnly) {
        filtered = filtered.filter(m => m.bookmarked);
    }
    if (state.validationFilter === 1) { // Any warnings
        filtered = filtered.filter(m => (m.validation_warning_count || 0) > 0 || m.has_segment_errors);
    } else if (state.validationFilter === 2) { // Errors only
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
            header.dataset.bucket = bucket;
            header.textContent = bucketLabel(bucket);
            fragment.appendChild(header);
        }
        fragment.appendChild(buildMessageRow(msg));
    }

    list.querySelectorAll('.message-row, .group-header').forEach(r => r.remove());
    list.appendChild(fragment);

    if (state.autoscroll) {
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
    const prevId = state.selectedId;
    state.selectedId = id;
    updateRowSelection(prevId, id);
    saveSession();

    try {
        const resp = await fetch(`/api/messages/${id}`);
        if (!resp.ok) return;
        state.selectedMessage = await resp.json();
        renderDetail();
    } catch (e) {
        console.error('Failed to load message:', e);
    }
}

// --- Detail panel header / meta ---
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
    if (!state.selectedMessage) return;
    const msg = state.selectedMessage;

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

    document.getElementById('detail-meta').innerHTML = buildDetailMeta(msg);

    // Actions row: tag chips + add-tag input + Bookmark + Pin diff (all on the right).
    const isPinned = state.diffPinnedMessage && state.diffPinnedMessage.id === msg.id;
    const bookmarkClass = msg.bookmarked ? 'action-btn bookmark active' : 'action-btn bookmark';
    const bookmarkSvg = msg.bookmarked ? ICONS.starFilled : ICONS.starOutline;
    const bookmarkLabel = msg.bookmarked ? 'Bookmarked' : 'Bookmark';
    const pinClass = isPinned ? 'action-btn pin active' : 'action-btn pin';
    const pinLabel = isPinned ? 'Pinned' : 'Pin diff';

    const tagChipsHtml = (msg.tags || []).map(t =>
        `<span class="msg-tag">${esc(t)}<span class="msg-tag-remove" role="button" tabindex="0" data-action="remove-tag" data-id="${msg.id}" data-tag="${escAttr(t)}" title="Remove tag" aria-label="Remove tag">${ICONS.xMark}</span></span>`
    ).join('');
    const tagAddHtml = `
        <div class="msg-tag-add">
            <input type="text" id="add-tag-input" placeholder="Add tag" aria-label="Add tag" data-action="add-tag-input" data-id="${msg.id}">
            <button data-action="add-tag" data-id="${msg.id}" aria-label="Submit tag">+</button>
        </div>
    `;

    document.getElementById('detail-actions').innerHTML = `
        <div id="detail-tags" class="detail-tags-inline">${tagChipsHtml}${tagAddHtml}</div>
        <button class="${bookmarkClass}" aria-label="${bookmarkLabel}" data-action="bookmark" data-id="${msg.id}" title="Toggle bookmark">${bookmarkSvg}<span class="btn-label">${bookmarkLabel}</span></button>
        <button class="${pinClass}" aria-label="${pinLabel}" data-action="diff-pin" data-id="${msg.id}" title="Pin this message as the diff reference">${ICONS.pin}<span class="btn-label">${pinLabel}</span></button>
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
        const showDiff = state.diffPinnedMessage && state.diffPinnedMessage.id !== msg.id;
        diffTabBtn.style.display = showDiff ? '' : 'none';
        if (!showDiff && state.activeTab === 'diff') {
            state.activeTab = 'parsed';
        }
    }

    renderTab();
}

function switchTab(tab) {
    state.activeTab = tab;
    document.querySelectorAll('.detail-tab').forEach(t => {
        t.classList.toggle('active', t.dataset.tab === tab);
    });
    renderTab();
    saveSession();
}

// --- Detail tab DOM builders ---
// Helpers build real DOM nodes (no innerHTML on the hot path) so that opening a
// message with many segments/fields stays cheap and predictable. Segment
// collapse is a CSS class on .segment-block — the field table is always built
// once and hidden via styles, so toggling never rebuilds the DOM.

function buildParseErrorEl(parseError) {
    const wrap = document.createElement('div');
    wrap.style.cssText = 'color:var(--error);font-family:var(--font-mono);padding:16px;line-height:1.6;';
    wrap.appendChild(document.createTextNode('⚠ Parse Error'));
    wrap.appendChild(document.createElement('br'));
    wrap.appendChild(document.createElement('br'));
    const inner = document.createElement('span');
    inner.style.color = 'var(--text-secondary)';
    inner.textContent = parseError;
    wrap.appendChild(inner);
    wrap.appendChild(document.createElement('br'));
    wrap.appendChild(document.createElement('br'));
    const hint = document.createElement('span');
    hint.style.color = 'var(--text-muted)';
    hint.textContent = 'Raw message is available in the Raw tab.';
    wrap.appendChild(hint);
    return wrap;
}

function buildTypicalChecklist(msg, missingSegWarnings, fieldWarningSegs) {
    const div = document.createElement('div');
    div.className = 'seg-checklist';
    const label = document.createElement('span');
    label.className = 'seg-checklist-label';
    label.textContent = 'Typical segments';
    div.appendChild(label);
    const presentNames = new Set(msg.segments.map(s => s.name));
    const descs = msg.typical_segment_descriptions || {};
    for (const s of msg.typical_segments) {
        const desc = descs[s];
        let cls, symbol, titleText;
        if (missingSegWarnings[s]) {
            cls = 'missing'; symbol = '✕'; titleText = missingSegWarnings[s];
        } else if (fieldWarningSegs[s]) {
            cls = 'warn'; symbol = '⚠'; titleText = (desc ? desc + ' — ' : '') + 'has required fields missing';
        } else if (presentNames.has(s)) {
            cls = 'present'; symbol = '✓'; titleText = desc || null;
        } else {
            cls = 'absent'; symbol = ''; titleText = desc || null;
        }
        const pill = document.createElement('span');
        pill.className = 'seg-pill ' + cls;
        if (titleText) pill.title = titleText;
        pill.textContent = symbol ? s + ' ' + symbol : s;
        div.appendChild(pill);
    }
    return div;
}

function buildValidationBanner(warnings) {
    const hasSegErrors = warnings.some(w => w.code === 'MISSING_SEGMENT');
    const details = document.createElement('details');
    details.className = hasSegErrors ? 'validation-summary error' : 'validation-summary';

    const summary = document.createElement('summary');
    const iconSpan = document.createElement('span');
    iconSpan.className = 'summary-icon';
    iconSpan.innerHTML = ICONS.warning;
    summary.appendChild(iconSpan);

    const textSpan = document.createElement('span');
    textSpan.className = 'summary-text';
    const strongEl = document.createElement('strong');
    strongEl.textContent = `${warnings.length} validation ${warnings.length === 1 ? 'warning' : 'warnings'}`;
    textSpan.appendChild(strongEl);

    const segMissing = warnings.filter(w => w.code === 'MISSING_SEGMENT').map(w => w.segment);
    const fieldMissing = warnings.filter(w => w.code === 'MISSING_FIELD').map(w => `${w.segment}-${w.field}`);
    const datatype = warnings.filter(w => w.code === 'INVALID_DATATYPE').map(w => `${w.segment}-${w.field}`);

    function appendListPart(prefix, items, listCls) {
        textSpan.appendChild(document.createTextNode(' · ' + prefix));
        const listSpan = document.createElement('span');
        listSpan.className = listCls;
        const head = items.slice(0, 3).join(', ');
        const rest = items.length > 3 ? ` +${items.length - 3} more` : '';
        listSpan.textContent = head + rest;
        textSpan.appendChild(listSpan);
    }
    if (fieldMissing.length) appendListPart('required field missing in ', fieldMissing, 'seg-list');
    if (segMissing.length) appendListPart('expected segment not sent: ', segMissing, 'seg-list');
    if (datatype.length) appendListPart('invalid datatype in ', datatype, 'seg-list-type');

    summary.appendChild(textSpan);

    const chevSpan = document.createElement('span');
    chevSpan.className = 'summary-chevron';
    chevSpan.innerHTML = ICONS.chevronRight;
    summary.appendChild(chevSpan);

    details.appendChild(summary);

    const ul = document.createElement('ul');
    ul.className = 'validation-warnings-list';
    for (const w of warnings) {
        const li = document.createElement('li');
        const badge = document.createElement('span');
        badge.className = w.code === 'MISSING_SEGMENT' ? 'validation-seg error'
            : w.code === 'INVALID_DATATYPE' ? 'validation-seg type'
            : 'validation-seg';
        badge.textContent = w.segment + (w.field != null ? '-' + w.field : '');
        li.appendChild(badge);
        li.appendChild(document.createTextNode(' ' + w.message));
        ul.appendChild(li);
    }
    details.appendChild(ul);
    return details;
}

function buildSegmentBlock(seg, segIdx, key, collapsed, warnFields) {
    const block = document.createElement('div');
    block.className = collapsed ? 'segment-block collapsed' : 'segment-block';

    const name = document.createElement('div');
    name.className = seg.description ? 'segment-name has-seg-tooltip' : 'segment-name';
    name.dataset.segKey = key;
    if (seg.description) name.dataset.desc = seg.name + ': ' + seg.description;
    name.setAttribute('role', 'button');
    name.tabIndex = 0;
    name.setAttribute('aria-expanded', String(!collapsed));

    const icon = document.createElement('span');
    icon.className = 'collapse-icon';
    icon.innerHTML = collapsed ? ICONS.chevronRight : ICONS.chevronDown;
    name.appendChild(icon);

    name.appendChild(document.createTextNode(' ' + seg.name + ' '));

    const count = document.createElement('span');
    count.className = 'field-count';
    count.textContent = `(${seg.fields.length})`;
    name.appendChild(count);

    const copyBtn = document.createElement('span');
    copyBtn.className = 'copy-btn';
    copyBtn.setAttribute('role', 'button');
    copyBtn.tabIndex = 0;
    copyBtn.setAttribute('aria-label', 'Copy segment');
    copyBtn.title = 'Copy segment';
    copyBtn.innerHTML = ICONS.copy;
    copyBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        copySegment(segIdx, copyBtn);
    });
    name.appendChild(copyBtn);

    block.appendChild(name);

    const table = document.createElement('table');
    table.className = 'field-table';
    const tbody = document.createElement('tbody');
    for (const f of seg.fields) {
        tbody.appendChild(buildFieldRow(seg, f, warnFields && warnFields.has(f.index)));
    }
    table.appendChild(tbody);
    block.appendChild(table);

    return block;
}

function buildFieldRow(seg, f, isWarn) {
    const tr = document.createElement('tr');
    if (isWarn) tr.className = 'warn';

    const tdIdx = document.createElement('td');
    tdIdx.className = 'field-idx';
    tdIdx.appendChild(document.createTextNode(`${seg.name}-${f.index}`));
    if (f.description) {
        const desc = document.createElement('span');
        desc.className = 'desc-text';
        desc.textContent = f.description;
        tdIdx.appendChild(desc);
    }
    tr.appendChild(tdIdx);

    const tdVal = document.createElement('td');
    tdVal.className = 'field-val';
    if (f.value) {
        tdVal.setAttribute('role', 'button');
        tdVal.tabIndex = 0;
        tdVal.setAttribute('aria-label', 'Copy field value');
        tdVal.textContent = f.value;
    } else {
        const emptySpan = document.createElement('span');
        emptySpan.className = 'field-empty';
        emptySpan.textContent = 'empty';
        tdVal.appendChild(emptySpan);
    }
    tr.appendChild(tdVal);

    const tdComp = document.createElement('td');
    tdComp.className = 'field-components';
    if (f.components.length > 1) {
        for (let i = 0; i < f.components.length; i++) {
            if (i > 0) {
                const sep = document.createElement('span');
                sep.style.color = 'var(--text-muted)';
                sep.textContent = ' ^ ';
                tdComp.appendChild(sep);
            }
            const cs = document.createElement('span');
            cs.title = `${seg.name}-${f.index}.${i + 1}`;
            cs.textContent = f.components[i];
            tdComp.appendChild(cs);
        }
    }
    tr.appendChild(tdComp);

    return tr;
}

function renderParsedTab(content, msg) {
    if (msg.parse_error) {
        content.replaceChildren(buildParseErrorEl(msg.parse_error));
        return;
    }

    const warnings = msg.validation_warnings || [];
    const missingSegWarnings = {};
    const fieldWarningSegs = {};
    const missingFieldByseg = new Map();
    for (const w of warnings) {
        if (w.code === 'MISSING_SEGMENT') {
            missingSegWarnings[w.segment] = w.message;
        } else if (w.code === 'MISSING_FIELD') {
            fieldWarningSegs[w.segment] = true;
            if (w.segment != null && w.field != null) {
                if (!missingFieldByseg.has(w.segment)) missingFieldByseg.set(w.segment, new Set());
                missingFieldByseg.get(w.segment).add(w.field);
            }
        }
    }

    const children = [];
    if (msg.typical_segments && msg.typical_segments.length) {
        children.push(buildTypicalChecklist(msg, missingSegWarnings, fieldWarningSegs));
    }
    if (warnings.length) {
        children.push(buildValidationBanner(warnings));
    }
    for (let segIdx = 0; segIdx < msg.segments.length; segIdx++) {
        const seg = msg.segments[segIdx];
        const key = `${msg.id}-${segIdx}`;
        children.push(buildSegmentBlock(seg, segIdx, key, state.collapsedSegments.has(key), missingFieldByseg.get(seg.name)));
    }
    content.replaceChildren(...children);
}

// Pull the 5 HL7 delimiter chars from the first MSH segment. Returns null
// when MSH is absent (e.g. a malformed payload starting with PID): we have
// no way to know the separators, so colourising guessed defaults would be
// misleading.
function detectDelimiters(raw) {
    const m = raw && raw.match(/MSH(.)(.{0,4})/);
    if (!m) return null;
    const field = m[1];
    const enc = m[2] || '';
    const chars = [field];
    if (enc.charAt(0)) chars.push(enc.charAt(0));
    if (enc.charAt(1)) chars.push(enc.charAt(1));
    if (enc.charAt(2)) chars.push(enc.charAt(2));
    if (enc.charAt(3)) chars.push(enc.charAt(3));
    return new Set(chars);
}

function appendColorizedSegmentBody(row, text, delimSet) {
    if (!delimSet || delimSet.size === 0) {
        row.appendChild(document.createTextNode(text));
        return;
    }
    let buf = '';
    for (let i = 0; i < text.length; i++) {
        const ch = text.charAt(i);
        if (delimSet.has(ch)) {
            if (buf) {
                row.appendChild(document.createTextNode(buf));
                buf = '';
            }
            const s = document.createElement('span');
            s.className = 'hl-delim';
            s.textContent = ch;
            row.appendChild(s);
        } else {
            buf += ch;
        }
    }
    if (buf) row.appendChild(document.createTextNode(buf));
}

function renderRawLinesView(container, raw, withCopyButton) {
    const lines = raw.split(/\r?\n|\r/).filter(l => l.trim());
    const delimSet = detectDelimiters(raw);
    const children = [];
    if (withCopyButton) {
        const top = document.createElement('div');
        top.style.cssText = 'display:flex;justify-content:flex-end;margin-bottom:8px;';
        const btn = document.createElement('button');
        btn.className = 'copy-raw-btn';
        btn.title = 'Copy entire message';
        btn.innerHTML = ICONS.copy + ' Copy All';
        btn.addEventListener('click', () => copyRawMessage(btn));
        top.appendChild(btn);
        children.push(top);
    }
    const view = document.createElement('div');
    view.className = 'raw-view';
    for (const line of lines) {
        const row = document.createElement('div');
        row.className = 'segment-line';
        const span = document.createElement('span');
        span.className = 'hl-seg-name';
        span.textContent = line.substring(0, 3);
        row.appendChild(span);
        appendColorizedSegmentBody(row, line.substring(3), delimSet);
        view.appendChild(row);
    }
    children.push(view);
    container.replaceChildren(...children);
}

function renderAckTab(content, ack) {
    if (!ack) {
        const empty = document.createElement('div');
        empty.className = 'empty-state';
        const p = document.createElement('p');
        p.textContent = 'No ACK was generated for this message';
        empty.appendChild(p);
        content.replaceChildren(empty);
        return;
    }
    renderRawLinesView(content, ack, false);
}

// Highlights a pretty-printed JSON string. Walks via regex so whitespace and
// indentation are preserved verbatim; non-token text is HTML-escaped to keep
// arbitrary string values safe.
function highlightJson(json) {
    const re = /("(?:\\.|[^"\\])*")(\s*:)?|\b(true|false|null)\b|(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)/g;
    let out = '';
    let last = 0;
    let m;
    while ((m = re.exec(json)) !== null) {
        if (m.index > last) out += esc(json.slice(last, m.index));
        if (m[1] !== undefined) {
            const isKey = m[2] !== undefined;
            const cls = isKey ? 'json-key' : 'json-str';
            out += `<span class="${cls}">${esc(m[1])}</span>`;
            if (isKey) out += esc(m[2]);
        } else if (m[3] !== undefined) {
            const cls = m[3] === 'null' ? 'json-null' : 'json-bool';
            out += `<span class="${cls}">${m[3]}</span>`;
        } else if (m[4] !== undefined) {
            out += `<span class="json-num">${m[4]}</span>`;
        }
        last = re.lastIndex;
    }
    if (last < json.length) out += esc(json.slice(last));
    return out;
}

function renderJsonTab(content, msg) {
    let json = detailJsonCache.get(msg);
    if (json === undefined) {
        json = JSON.stringify(msg, null, 2);
        detailJsonCache.set(msg, json);
    }
    const pre = document.createElement('pre');
    pre.className = 'raw-view json-view';
    pre.innerHTML = highlightJson(json);
    content.replaceChildren(pre);
}

function renderTab() {
    const content = document.getElementById('detail-content');
    if (!state.selectedMessage) return;
    const msg = state.selectedMessage;

    if (state.activeTab === 'parsed') {
        renderParsedTab(content, msg);
    } else if (state.activeTab === 'raw') {
        renderRawLinesView(content, msg.raw, true);
    } else if (state.activeTab === 'ack') {
        renderAckTab(content, msg.ack_response);
    } else if (state.activeTab === 'json') {
        renderJsonTab(content, msg);
    } else if (state.activeTab === 'diff') {
        renderDiffTab(content, msg);
    }
}

function toggleSegment(key) {
    const isCollapsed = !state.collapsedSegments.has(key);
    if (isCollapsed) {
        state.collapsedSegments.add(key);
    } else {
        state.collapsedSegments.delete(key);
    }
    // Only update the affected segment-block — no DOM rebuild. The field
    // table is always rendered; .collapsed hides it via CSS.
    const content = document.getElementById('detail-content');
    if (content) {
        const nameEl = content.querySelector(`.segment-name[data-seg-key="${CSS.escape(key)}"]`);
        if (nameEl) {
            const block = nameEl.closest('.segment-block');
            if (block) block.classList.toggle('collapsed', isCollapsed);
            nameEl.setAttribute('aria-expanded', String(!isCollapsed));
            const iconEl = nameEl.querySelector('.collapse-icon');
            if (iconEl) iconEl.innerHTML = isCollapsed ? ICONS.chevronRight : ICONS.chevronDown;
        }
    }
    saveSession();
}
