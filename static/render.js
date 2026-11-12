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

    // Fast path: if the set of source labels is unchanged, only patch count
    // spans and active/dimmed classes — no innerHTML rebuild, no DOM teardown.
    const existingChips = Array.from(container.querySelectorAll('[data-source]'));
    const labelsUnchanged = existingChips.length === sortedLabels.length &&
        sortedLabels.every((l, i) => existingChips[i].dataset.source === l);

    if (labelsUnchanged) {
        for (const chip of existingChips) {
            const label = chip.dataset.source;
            chip.classList.toggle('active', state.highlightedSource === label);
            chip.classList.toggle('dimmed', !!(state.highlightedSource && state.highlightedSource !== label));
            const numEl = chip.querySelector('.num');
            if (numEl) numEl.textContent = state.sourceCounts.get(label) || 0;
        }
        const checkbox = container.querySelector('[data-action="color-by-port"]');
        if (checkbox) checkbox.checked = state.colorByPort;
        return;
    }

    // Full rebuild — new source label(s) added or labelling changed (e.g. toggleColorByPort).
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
        ratePill.classList.remove('hidden');
        rateValue.textContent = `${state.rateWindow.length}/min`;
        rateSpark.innerHTML = renderRateSpark();
    } else {
        ratePill.classList.add('hidden');
    }

    // last pill — show only after first message
    const lastPill = document.getElementById('pill-last');
    const lastValue = document.getElementById('pill-last-value');
    if (state.lastMessageReceivedAt !== null) {
        lastPill.classList.remove('hidden');
        lastValue.textContent = formatRelativeTime(Date.now() - state.lastMessageReceivedAt);
    } else {
        lastPill.classList.add('hidden');
    }
}

function tickRowRelativeTimes() {
    if (document.hidden) return;
    const now = Date.now();
    // rowTimeLabel shows a static clock time for messages older than 5 min — skip those.
    const cutoff = now - 300_000;
    document.querySelectorAll('.message-row').forEach(row => {
        const ts = row.dataset.received;
        if (!ts) return;
        if (new Date(ts).getTime() < cutoff) return;
        const timeEl = row.querySelector('.msg-row2 .time');
        if (!timeEl) return;
        // Assigning an unchanged label still replaces the text node and
        // invalidates layout; skip it (#216).
        const label = rowTimeLabel({ received_at: ts }, now);
        if (timeEl.textContent !== label) timeEl.textContent = label;
    });
}

// --- Header counters (total / validation / parse errors) ---
// `parse errors` is updated by pollStats from the server stat. Validation and
// bookmark counters are maintained incrementally in state (see ws.js) so this
// function is O(1) regardless of store size.
function updateHeaderCounters() {
    const totalEl = document.getElementById('pill-total-value');
    if (totalEl) {
        // Server-side total (init / addMessage / evicted / pollStats), not the
        // client buffer length — the buffer is capped at CLIENT_MESSAGE_CAP.
        totalEl.textContent = state.totalMessagesCount;
    }

    const validationCount = state.totalValidationCount;
    const validationPill = document.getElementById('pill-validation');
    const validationValue = document.getElementById('pill-validation-value');
    if (validationValue) validationValue.textContent = validationCount;
    if (validationPill) {
        validationPill.classList.toggle('warn-pill', validationCount > 0);
    }

    const bookmarkCount = state.totalBookmarkCount;
    const bookmarkBtn = document.getElementById('btn-bookmarks');
    updateTruncationNote();

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
// Row skeleton, parsed once and cloned per row (#202). Parsing a template
// string (with two inline SVGs) per row and attaching two closures to each
// cost ~0.8 ms/row, so resuming after a long pause blocked ~1.6 s for 2000
// rows. Clicks and Enter/Space reach the row through the document-level
// `data-action` delegation in app.js (`select-message`).
let messageRowTemplate = null;
const iconNodeCache = new Map();

function iconNode(name) {
    let node = iconNodeCache.get(name);
    if (!node) {
        const t = document.createElement('template');
        t.innerHTML = ICONS[name];
        node = t.content.firstElementChild;
        iconNodeCache.set(name, node);
    }
    return node.cloneNode(true);
}

function rowSkeleton() {
    if (!messageRowTemplate) {
        const t = document.createElement('template');
        t.innerHTML = '<div class="message-row" tabindex="0" role="button" data-action="select-message">' +
            '<div class="msg-source-bar"></div>' +
            '<div class="msg-body">' +
            '<div class="msg-row1"><span class="msg-type"></span><span class="msg-patient"></span></div>' +
            '<div class="msg-row2"><span class="facility"></span><span class="sep">·</span>' +
            '<span class="src"></span><span class="sep">·</span>' +
            '<span class="segs"></span><span class="time"></span></div>' +
            '</div>' +
            '<div class="msg-actions"><button class="msg-bookmark" data-action="bookmark" title="Bookmark"></button></div>' +
            '</div>';
        messageRowTemplate = t.content.firstElementChild;
    }
    return messageRowTemplate.cloneNode(true);
}

function textSpan(className, text, title) {
    const el = document.createElement('span');
    el.className = className;
    el.textContent = text;
    if (title) el.title = title;
    return el;
}

function buildMessageRow(msg) {
    const row = rowSkeleton();
    if (msg.id === state.selectedId) row.classList.add('selected');
    if (msg.bookmarked) row.classList.add('bookmarked');
    const srcKey = state.colorByPort ? msg.source_addr : (msg.source_addr ? msg.source_addr.split(':')[0] : '');
    if (state.highlightedSource && srcKey !== state.highlightedSource) row.classList.add('dimmed');

    row.dataset.id = msg.id;
    row.dataset.received = msg.received_at || '';
    row.setAttribute('aria-label', `Message from ${msg.sending_facility || 'unknown'}, type ${msg.message_type || 'unknown'}, received ${msg.received_at}`);

    const [bar, body, actions] = row.children;
    bar.title = msg.source_addr || '';
    bar.style.background = getSourceColor(msg.source_addr);

    const [row1, row2] = body.children;
    const [type, patient] = row1.children;
    if (msg.parse_error) {
        type.classList.add('parse-error');
        type.title = msg.parse_error;
        type.textContent = '⚠ PARSE ERROR';
    } else {
        type.textContent = msg.message_type || '—';
    }
    patient.textContent = msg.patient_name || msg.patient_id || '—';

    const warnCount = msg.validation_warning_count || 0;
    if (warnCount > 0) {
        row1.appendChild(textSpan(`msg-warning ${msg.has_segment_errors ? 'err' : 'warn'}`, `⚠ ${warnCount}`,
            `${warnCount} validation warning${warnCount > 1 ? 's' : ''}`));
    }
    const tagsArr = msg.tags || [];
    if (tagsArr.length > 0) {
        const tags = document.createElement('span');
        tags.className = 'msg-tags-list';
        for (const t of tagsArr.slice(0, 2)) tags.appendChild(textSpan('msg-tag-small', t));
        if (tagsArr.length > 2) tags.appendChild(textSpan('msg-tag-small', `+${tagsArr.length - 2}`));
        row1.appendChild(tags);
    }
    const ackCode = (msg.ack_code || '').toUpperCase();
    row1.appendChild(ackCode ? textSpan(ackChipClass(ackCode), ackCode) : textSpan('msg-ack none', '—'));

    const [facility, , src, , segs, time] = row2.children;
    facility.textContent = msg.sending_facility || 'unknown';
    src.textContent = msg.source_addr || '';
    segs.textContent = msg.segment_count != null ? `${msg.segment_count} segs` : '—';
    time.textContent = rowTimeLabel(msg, Date.now());

    const btn = actions.firstElementChild;
    btn.dataset.id = msg.id;
    btn.setAttribute('aria-label', msg.bookmarked ? 'Remove bookmark' : 'Add bookmark');
    if (msg.bookmarked) btn.classList.add('active');
    btn.appendChild(iconNode(msg.bookmarked ? 'starFilled' : 'starOutline'));
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

// Remove the rows for `ids` from the list without rebuilding it. Dropped
// rows are normally the oldest ones, so the walk starts at the bottom and
// stops as soon as every ID has been removed. IDs without a row (filtered
// out, or dropped before they were ever rendered) just let the walk run to
// the top — a plain sibling walk with a Set lookup, no selector matching
// (a per-ID querySelector prepass cost ~70x the removal itself). Group
// headers left without any row beneath them are removed afterwards.
function removeRowsFromList(ids) {
    const list = document.getElementById('message-list');
    if (!list || ids.size === 0) return;

    let remaining = ids.size;
    let el = list.lastElementChild;
    while (el && remaining > 0) {
        const prev = el.previousElementSibling;
        if (el.classList.contains('message-row') && ids.has(el.dataset.id)) {
            el.remove();
            remaining--;
        }
        el = prev;
    }

    // Drop orphaned group headers (header not directly followed by a row —
    // i.e. by another header, the truncation note, or nothing). Headers are
    // few (one per time bucket), so a pass over all of them is cheap.
    for (const header of list.querySelectorAll('.group-header')) {
        const next = header.nextElementSibling;
        if (!next || !next.classList.contains('message-row')) header.remove();
    }

    if (!list.querySelector('.message-row')) {
        showEmptyState();
        updateTruncationNote();
    }
}

// Show the list's empty state with the right card (#203): the onboarding
// card ("Listening on …", CLI snippet) only when there really are no
// messages; when messages are loaded but a search/filter hides all of them,
// a "No matching messages" card instead.
function showEmptyState() {
    const empty = document.getElementById('empty-state');
    if (!empty) return;
    const noMatch = !canPrependOnly() && state.messages.length > 0;
    const onboarding = document.getElementById('empty-onboarding');
    const noMatchCard = document.getElementById('empty-no-match');
    if (onboarding) onboarding.classList.toggle('hidden', noMatch);
    if (noMatchCard) noMatchCard.classList.toggle('hidden', !noMatch);
    empty.style.display = 'flex';
}

// Bottom-of-list note shown when the server holds more messages than this
// view has loaded (client cap, #177; initial 1000-row load, #198). Filters
// and search run client-side, so without this a search for a trimmed
// message would silently show nothing. Kept as the last child of the list;
// when no row is shown (e.g. a search with no loaded match) it goes first,
// above the full-height empty state, so it stays visible.
function updateTruncationNote() {
    const list = document.getElementById('message-list');
    if (!list) return;
    const held = state.messages.length + state.pendingMessages.length;
    const total = state.totalMessagesCount;
    let note = document.getElementById('list-truncated-note');
    if (total <= held) {
        if (note) note.classList.add('hidden');
        return;
    }
    if (!note) {
        note = document.createElement('div');
        note.id = 'list-truncated-note';
        note.className = 'list-truncated-note';
        note.setAttribute('role', 'note');
    }
    note.textContent = `This view holds the newest ${held.toLocaleString()} of ` +
        `${total.toLocaleString()} messages. Older messages are kept on the server ` +
        `but not loaded here — filters and search only cover the loaded ones.`;
    note.classList.remove('hidden');
    if (list.querySelector('.message-row')) {
        if (list.lastElementChild !== note) list.appendChild(note);
    } else if (list.firstElementChild !== note) {
        list.insertBefore(note, list.firstElementChild);
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
        showEmptyState();
        list.querySelectorAll('.message-row, .group-header').forEach(r => r.remove());
        updateTruncationNote();
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
    updateTruncationNote(); // rows were appended after it — move it back to the end

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

    // Last click wins (#183): clicking A then B quickly must not let A's
    // slower response overwrite B's detail panel. Checked after every await,
    // since both the fetch and the body read can resolve out of order. Also
    // covers the selection being cleared meanwhile (e.g. server eviction).
    const seq = ++state.selectRequestSeq;
    const isCurrent = () => seq === state.selectRequestSeq && state.selectedId === id;

    try {
        const resp = await fetch(`/api/messages/${id}`);
        if (!isCurrent()) return;
        if (!resp.ok) {
            // The message is gone (typically evicted between list render and
            // click). Don't leave the previous message in the panel next to
            // the newly highlighted row — clear both and say why.
            updateRowSelection(id, null);
            state.selectedId = null;
            state.selectedMessage = null;
            resetDetailHeader();
            document.getElementById('detail-content').innerHTML = DETAIL_EMPTY_HTML;
            saveSession();
            showToast('This message is no longer available on the server');
            return;
        }
        const msg = await resp.json();
        if (!isCurrent()) return;
        state.selectedMessage = msg;
        renderDetail();
    } catch (e) {
        console.error('Failed to load message:', e);
    }
}

// --- Detail panel header / meta ---
function resetDetailHeader() {
    const typeEl = document.getElementById('detail-type');
    if (typeEl) {
        typeEl.classList.add('hidden');
        typeEl.textContent = '';
    }
    const titleEl = document.getElementById('detail-title');
    if (titleEl) titleEl.textContent = 'Select a message';
    const descEl = document.getElementById('detail-desc');
    if (descEl) {
        descEl.classList.add('hidden');
        descEl.textContent = '';
    }
    const metaEl = document.getElementById('detail-meta');
    if (metaEl) metaEl.innerHTML = '';
    const actionsEl = document.getElementById('detail-actions');
    if (actionsEl) actionsEl.innerHTML = '';
    const segBadge = document.getElementById('tab-segments-badge');
    if (segBadge) segBadge.classList.add('hidden');
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
        typeEl.classList.remove('hidden');
    } else {
        typeEl.classList.add('hidden');
    }
    document.getElementById('detail-title').textContent =
        msg.patient_name || msg.patient_id || msg.message_type || 'Message';

    // Description row — always populate when message_type_description is available.
    const descEl = document.getElementById('detail-desc');
    if (msg.message_type_description) {
        descEl.textContent = msg.message_type_description;
        descEl.classList.remove('hidden');
    } else {
        descEl.classList.add('hidden');
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
            segBadge.classList.remove('hidden');
        } else {
            segBadge.classList.add('hidden');
        }
    }

    // Show/hide Diff tab based on whether a pinned message exists and it's a different message
    const diffTabBtn = document.getElementById('tab-btn-diff');
    if (diffTabBtn) {
        const showDiff = state.diffPinnedMessage && state.diffPinnedMessage.id !== msg.id;
        diffTabBtn.classList.toggle('hidden', !showDiff);
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

function buildSegmentBlock(seg, segIdx, key, collapsed, warnFields, delims) {
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
        tbody.appendChild(buildFieldRow(seg, f, warnFields && warnFields.has(f.index), delims));
    }
    table.appendChild(tbody);
    block.appendChild(table);

    return block;
}

// Renders a field's decoded structure (#185). `f.repetitions` is only present
// for structured fields: [rep][comp][sub] leaves, escape-decoded server-side
// (`value` stays raw). Separators are the sender's own (`msg.delimiters`,
// "|^~\\&" form). Leaf tooltips use HL7 positions: PID-3[2].4.1 (the
// repetition index only when the field repeats, the sub-component index only
// when the component has several).
function buildFieldStructure(td, seg, f, delims) {
    const reps = f.repetitions;
    if (!reps) return;
    // A lone leaf identical to the raw value adds nothing (e.g. an unknown
    // escape kept verbatim).
    if (reps.length === 1 && reps[0].length === 1 && reps[0][0].length === 1 && reps[0][0][0] === f.value) return;

    const d = (typeof delims === 'string' && delims.length === 5) ? delims : '|^~\\&';
    const sep = (ch, cls) => {
        const el = document.createElement('span');
        el.className = 'field-sep ' + cls;
        el.textContent = ` ${ch} `;
        return el;
    };
    reps.forEach((comps, r) => {
        if (r > 0) td.appendChild(sep(d[2], 'rep'));
        comps.forEach((subs, c) => {
            if (c > 0) td.appendChild(sep(d[1], 'comp'));
            subs.forEach((leaf, sIdx) => {
                if (sIdx > 0) td.appendChild(sep(d[4], 'sub'));
                const el = document.createElement('span');
                el.className = 'field-leaf';
                el.title = `${seg.name}-${f.index}`
                    + (reps.length > 1 ? `[${r + 1}]` : '')
                    + `.${c + 1}`
                    + (subs.length > 1 ? `.${sIdx + 1}` : '');
                const shown = previewText(leaf);
                el.textContent = shown.text;
                if (shown.truncated) el.appendChild(truncationNote(shown.size));
                td.appendChild(el);
            });
        });
    });
}

function buildFieldRow(seg, f, isWarn, delims) {
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
        const shown = previewText(f.value);
        tdVal.textContent = shown.text;
        if (shown.truncated) {
            // A multi-MB value would freeze layout for seconds (#215); show a
            // preview, and let the copy handler take the full value.
            tdVal.appendChild(truncationNote(shown.size, 'click to copy the full value'));
            fullFieldValues.set(tdVal, f.value);
        }
    } else {
        const emptySpan = document.createElement('span');
        emptySpan.className = 'field-empty';
        emptySpan.textContent = 'empty';
        tdVal.appendChild(emptySpan);
    }
    tr.appendChild(tdVal);

    const tdComp = document.createElement('td');
    tdComp.className = 'field-components';
    buildFieldStructure(tdComp, seg, f, delims);
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
        children.push(buildSegmentBlock(seg, segIdx, key, state.collapsedSegments.has(key), missingFieldByseg.get(seg.name), msg.delimiters));
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
        // A multi-MB line (an MDM attachment) is shown as a preview (#215);
        // "Copy All" copies the complete message from state, not the DOM.
        const shown = previewText(line);
        appendColorizedSegmentBody(row, shown.text.substring(3), delimSet);
        if (shown.truncated) {
            row.appendChild(truncationNote(shown.size, withCopyButton ? '"Copy All" copies the complete message' : ''));
        }
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
    let cached = detailJsonCache.get(msg);
    if (cached === undefined) {
        // Long strings are shortened for display (#215): stringifying and
        // highlighting a multi-MB attachment, twice (value + repetitions),
        // froze the tab. The JSON API (/api/messages/{id}) has everything.
        let shortened = false;
        const json = JSON.stringify(msg, (key, value) => {
            const shown = previewText(value);
            if (!shown.truncated) return value;
            shortened = true;
            return `${shown.text}… [${shown.size}, shortened for display]`;
        }, 2);
        cached = { json, shortened };
        detailJsonCache.set(msg, cached);
    }
    const children = [];
    if (cached.shortened) {
        const note = document.createElement('div');
        note.className = 'json-shortened-note';
        note.textContent = `Strings longer than ${DISPLAY_VALUE_MAX.toLocaleString()} characters are shortened here. ` +
            `The complete message is at /api/messages/${msg.id}.`;
        children.push(note);
    }
    const pre = document.createElement('pre');
    pre.className = 'raw-view json-view';
    pre.innerHTML = highlightJson(cached.json);
    children.push(pre);
    content.replaceChildren(...children);
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
