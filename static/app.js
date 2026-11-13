// Top-level init, UI handlers wired up by `onclick` attributes in
// index.html, panel splitter, and the click/keydown delegation handler.
// State lives in state.js, rendering in render.js / diff.js, network in
// ws.js, pure helpers in util.js. Loaded last so all dependencies exist.

// --- Source legend UI handlers ---
function toggleColorByPort(e) {
    state.colorByPort = e.target.checked;
    state.highlightedSource = null; // reset highlight on toggle
    // Labels change between "host" and "host:port" — rebuild the count cache.
    recomputeSourceCounts();
    renderMessageList();
    renderSourceLegend();
    saveSession();
}

function toggleHighlightSource(label) {
    state.highlightedSource = (state.highlightedSource === label) ? null : label;
    renderMessageList();
    renderSourceLegend();
    saveSession();
}

// --- List toggles ---
function toggleAutoscroll() {
    state.autoscroll = !state.autoscroll;
    const btn = document.getElementById('btn-autoscroll');
    if (!btn) return;
    btn.classList.toggle('on', state.autoscroll);
    btn.classList.toggle('ok-on', state.autoscroll);
    saveSession();
}

function togglePause() {
    state.paused = !state.paused;
    const btn = document.getElementById('btn-pause');
    if (!btn) return;
    btn.classList.toggle('on', state.paused);
    btn.classList.toggle('warn-on', state.paused);
    if (state.paused) {
        btn.innerHTML = `${ICONS.play}<span class="btn-label">Live</span>`;
    } else {
        btn.innerHTML = `${ICONS.pause}<span class="btn-label">Pause</span>`;
        flushAndRender();
    }
    saveSession();
}

function toggleBookmarkFilter() {
    state.showBookmarkedOnly = !state.showBookmarkedOnly;
    const btn = document.getElementById('btn-bookmarks');
    if (!btn) return;
    btn.classList.toggle('on', state.showBookmarkedOnly);
    btn.classList.toggle('warn-on', state.showBookmarkedOnly);
    const labelHtml = `<span class="btn-label">Bookmarks</span>`;
    const icon = state.showBookmarkedOnly ? ICONS.starFilled : ICONS.starOutline;
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
    if (state.validationFilter === 0) {
        btn.innerHTML = `${baseHtml}All</span>`;
    } else if (state.validationFilter === 1) {
        btn.innerHTML = `${baseHtml}Warn</span>`;
        btn.classList.add('on', 'warn-on');
    } else if (state.validationFilter === 2) {
        btn.innerHTML = `${baseHtml}Error</span>`;
        btn.classList.add('on', 'err-on');
    }
}

// In + Out → Received → Sent (#274). The button exists while sending is on
// (or a filter other than In + Out is still active).
const DIRECTION_LABELS = ['In + Out', 'Received', 'Sent'];

function syncDirectionFilterUI() {
    const btn = document.getElementById('btn-direction');
    if (!btn) return;
    btn.innerHTML = `${ICONS.updown}<span class="btn-label">${DIRECTION_LABELS[state.directionFilter]}</span>`;
    btn.classList.toggle('on', state.directionFilter !== 0);
    btn.classList.toggle('hidden', !state.sendConfig && state.directionFilter === 0);
}

function toggleDirectionFilter() {
    state.directionFilter = (state.directionFilter + 1) % 3;
    syncDirectionFilterUI();
    renderMessageList();
    saveSession();
}

// Trace (#274): filter the list by one control ID.
function traceControlId(value) {
    if (!value) return;
    clearTimeout(state.searchDebounceTimer);
    state.searchQuery = value;
    const input = document.getElementById('search-input');
    if (input) input.value = value;
    renderMessageList();
    saveSession();
}

function toggleValidationFilter() {
    state.validationFilter = (state.validationFilter + 1) % 3;
    syncValidationFilterUI();
    renderMessageList();
    saveSession();
}

// Reset search, bookmark filter and validation filter in one go — the
// action behind the "No matching messages" empty state (#203).
function clearFilters() {
    clearTimeout(state.searchDebounceTimer);
    state.searchQuery = '';
    const input = document.getElementById('search-input');
    if (input) input.value = '';
    if (state.showBookmarkedOnly) toggleBookmarkFilter(); // updates its button
    state.validationFilter = 0;
    syncValidationFilterUI();
    state.directionFilter = 0;
    syncDirectionFilterUI();
    renderMessageList();
    saveSession();
}

// --- Bulk actions ---
async function exportMessages() {
    try {
        const resp = await fetch('/api/export');
        if (!resp.ok) throw new Error(`Server error: ${resp.status}`);
        const blob = await resp.blob();
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = `harux-export-${new Date().toISOString().slice(0, 19)}.hl7`;
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
        state.messages = [];
        state.pendingMessages = [];
        state.sourceCounts.clear();
        state.seenSources.clear();
        state.diffPinnedMessage = null;
        state.totalValidationCount = 0;
        state.totalBookmarkCount = 0;
        state.rateWindow.length = 0;
        state.lastMessageReceivedAt = null;
        state.selectedId = null; // an open draft stays open (#277)
        state.selectedMessage = null;
        markDraftOriginsGone(null);
        renderMessageList();
        renderSourceLegend();
        renderHealthPills();
        updateHeaderCounters();
        showDetailAfterStoreChange();
    } catch (e) {
        console.error('Failed to clear messages:', e);
    }
}

// --- Per-message actions ---
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

// --- Copy helpers (DOM-bound; pure copyToClipboard lives in util.js) ---
// The server no longer ships a per-segment copy of the text (#180); derive
// it from the message's raw payload exactly as the parser splits segments:
// on \r or \n, skipping blank lines.
// Cached per message object, so repeated copies on a 10 MB message split it
// once rather than on every click.
const segmentLinesCache = new WeakMap();

function segmentLines(msg) {
    let lines = segmentLinesCache.get(msg);
    if (!lines) {
        lines = (msg.raw || '').split(/[\r\n]/).filter(s => s.trim() !== '');
        segmentLinesCache.set(msg, lines);
    }
    return lines;
}

function copySegment(segIdx, el) {
    if (!state.selectedMessage) return;
    const line = segmentLines(state.selectedMessage)[segIdx];
    if (line !== undefined) copyToClipboard(line, el);
}

function copyRawMessage(el) {
    if (!state.selectedMessage) return;
    copyToClipboard(state.selectedMessage.raw, el);
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

        const currentWidth = listPanel.getBoundingClientRect().width;
        localStorage.setItem(SPLITTER_STORAGE_KEY, Math.round(currentWidth));
    }

    splitter.addEventListener('dblclick', () => {
        listPanel.style.flexBasis = (SPLITTER_DEFAULT_RATIO * 100) + '%';
        localStorage.removeItem(SPLITTER_STORAGE_KEY);
    });

    splitter.addEventListener('mousedown', onDragStart);
    splitter.addEventListener('touchstart', onDragStart, { passive: false });
}

// --- Document-level interaction delegation ---
function handleInteraction(e) {
    const copyBtn = e.target.closest('.copy-btn');
    if (e.type === 'keydown' && copyBtn) {
        e.preventDefault();
        copyBtn.click();
        return;
    }

    // A click outside the Resend menu closes it (#274).
    if (state.sendMenu && e.type === 'click' && e.target.closest
        && !e.target.closest('#send-menu') && !e.target.closest('[data-action="resend-menu"]')) {
        closeSendMenu();
    }

    // A click outside the New message menu closes it (#277).
    if (state.newMenuOpen && e.type === 'click' && e.target.closest
        && !e.target.closest('#new-message')) {
        toggleNewMenu(false);
    }

    // data-action delegation (replaces inline onclick= handlers)
    const actionEl = e.target.closest('[data-action]');
    if (actionEl) {
        const action = actionEl.dataset.action;
        const id = actionEl.dataset.id;
        if (action === 'select-message') { selectMessage(id); return; }
        if (action === 'load-older') { loadOlderMessages(); return; }
        if (action === 'search-older') { searchOlderOnServer(); return; }
        if (action === 'bookmark') { toggleBookmark(id, e); return; }
        if (action === 'toggle-source') { toggleHighlightSource(actionEl.dataset.source); return; }
        if (action === 'remove-tag') { removeTag(id, actionEl.dataset.tag); return; }
        if (action === 'add-tag') {
            const input = document.getElementById('add-tag-input');
            if (input) addTag(id, input.value);
            return;
        }
        if (action === 'diff-pin') { toggleDiffPin(id, e); return; }
        if (action === 'trace-control') { traceControlId(actionEl.dataset.value); return; }
        if (handleSendAction(action, actionEl)) return;
        if (handleComposeAction(action, actionEl)) return;
    }

    const segEl = e.target.closest('.segment-name');
    if (segEl && !copyBtn) toggleSegment(segEl.dataset.segKey);

    const cell = e.target.closest('.field-val');
    if (cell && !cell.querySelector('.field-empty')) {
        // A shortened cell (#215) shows a preview: copy the full value.
        const full = fullFieldValues.get(cell);
        copyToClipboard(full !== undefined ? full : cell.textContent.trim(), cell);
    }
}

// --- Init ---

// Restore session state BEFORE first render so restored values take effect
loadSession();

// Wire up static buttons that previously used inline onclick= in index.html.
document.getElementById('btn-export').addEventListener('click', exportMessages);
document.getElementById('btn-clear').addEventListener('click', clearMessages);
document.getElementById('btn-pause').addEventListener('click', togglePause);
document.getElementById('btn-autoscroll').addEventListener('click', toggleAutoscroll);
document.getElementById('btn-bookmarks').addEventListener('click', toggleBookmarkFilter);
document.getElementById('btn-validation').addEventListener('click', toggleValidationFilter);
document.getElementById('btn-direction').addEventListener('click', toggleDirectionFilter);
document.getElementById('btn-copy-cli').addEventListener('click', function () { copyCliSnippet(this); });
document.getElementById('btn-clear-filters').addEventListener('click', clearFilters);
document.getElementById('btn-new-message').addEventListener('click', () => toggleNewMenu());

// Tab buttons — delegate on the container via data-tab attribute.
document.querySelector('.detail-tabs').addEventListener('click', (e) => {
    const btn = e.target.closest('.detail-tab');
    if (btn && btn.dataset.tab) switchTab(btn.dataset.tab);
});

// Checkboxes in innerHTML-generated markup (source legend, diff options bar).
document.addEventListener('change', (e) => {
    const action = e.target.dataset.action;
    if (action === 'color-by-port') toggleColorByPort(e);
    if (action === 'diff-ignore-dynamic') toggleDiffIgnoreDynamic(e);
    if (e.target.id === 'draft-charset') setDraftCharset(e.target.value); // #277
});

// Search is purely client-side (filters the local `messages` array via matchesSearch).
// Debounced: the list rebuild runs once typing pauses. The server search
// (/api/search, #197) is not run per keystroke — only from the list's
// "Search older on the server" button.
document.getElementById('search-input').addEventListener('input', (e) => {
    clearTimeout(state.searchDebounceTimer);
    state.searchDebounceTimer = setTimeout(() => {
        state.searchQuery = e.target.value;
        renderMessageList(); // also refreshes the note's "Search older" button
        saveSession();
    }, 300);
});

document.addEventListener('click', handleInteraction);
document.addEventListener('keydown', (e) => {
    const activeEl = document.activeElement;
    if (!activeEl) return;

    // Ctrl/Cmd+Enter sends the open draft (#277).
    if (handleDraftKeydown(e)) return;

    // Escape closes the Resend menu; Enter in its form saves the target.
    // Typing in the form's inputs is left alone (they carry no data-action).
    if (state.sendMenu && e.key === 'Escape') {
        e.preventDefault();
        closeSendMenu();
        return;
    }
    if (state.newMenuOpen && e.key === 'Escape') {
        e.preventDefault();
        toggleNewMenu(false);
        return;
    }
    if (e.key === 'Enter' && activeEl.closest && activeEl.closest('.target-form') && activeEl.tagName === 'INPUT') {
        e.preventDefault();
        saveTargetForm();
        return;
    }

    // Enter in the add-tag input submits the tag.
    if (e.key === 'Enter' && activeEl.dataset.action === 'add-tag-input') {
        e.preventDefault();
        addTag(activeEl.dataset.id, activeEl.value);
        return;
    }

    // Text entry (a draft's text, its encoding, any input) keeps its keys:
    // a space typed into a draft must not activate anything (#277).
    if (isTextEntry(activeEl)) return;

    if (e.key === 'Enter' || e.key === ' ') {
        // Activate data-action elements with keyboard (toggle-source, remove-tag, etc.)
        const action = activeEl.dataset.action;
        if (action && action !== 'add-tag-input') {
            e.preventDefault();
            handleInteraction({ target: activeEl, type: 'click', stopPropagation: () => e.stopPropagation() });
            return;
        }

        if (activeEl.classList.contains('segment-name') || activeEl.classList.contains('field-val') || activeEl.classList.contains('copy-btn')) {
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
    if (state.searchQuery) document.getElementById('search-input').value = state.searchQuery;

    if (state.activeTab !== 'parsed') {
        document.querySelectorAll('.detail-tab').forEach(t => {
            t.classList.toggle('active', t.dataset.tab === state.activeTab);
        });
    }

    if (state.paused) {
        const btn = document.getElementById('btn-pause');
        btn.classList.add('on', 'warn-on');
        btn.innerHTML = `${ICONS.play}<span class="btn-label">Live</span>`;
    }

    if (state.showBookmarkedOnly) {
        const btn = document.getElementById('btn-bookmarks');
        btn.classList.add('on', 'warn-on');
        btn.innerHTML = `${ICONS.starFilled}<span class="btn-label">Bookmarks</span>`;
    }

    syncValidationFilterUI();
    syncDirectionFilterUI();
})();

// Sending (#274): the user's targets from this browser, and whether the
// server sends at all (nothing new is shown while it doesn't).
loadTargets();
// Drafts (#277) of this tab and the ones kept on this computer; shown once
// /api/send/config says sending is on.
loadDrafts();
loadSendConfig();

initSplitter();
// Set autoscroll visual state — toggleAutoscroll flips the value, so pre-flip it
state.autoscroll = !state.autoscroll;
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
