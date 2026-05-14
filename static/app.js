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

function toggleValidationFilter() {
    state.validationFilter = (state.validationFilter + 1) % 3;
    syncValidationFilterUI();
    renderMessageList();
    saveSession();
}

// --- Bulk actions ---
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
        state.messages = [];
        state.pendingMessages = [];
        state.sourceCounts.clear();
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
function copySegment(segIdx, el) {
    if (!state.selectedMessage) return;
    const seg = state.selectedMessage.segments[segIdx];
    if (seg) copyToClipboard(seg.raw, el);
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

    const segEl = e.target.closest('.segment-name');
    if (segEl && !copyBtn) toggleSegment(segEl.dataset.segKey);

    const cell = e.target.closest('.field-val');
    if (cell && !cell.querySelector('.field-empty')) {
        copyToClipboard(cell.textContent.trim(), cell);
    }
}

// --- Init ---

// Restore session state BEFORE first render so restored values take effect
loadSession();

// Search is purely client-side (filters the local `messages` array via matchesSearch).
// The debounce is a forward-looking safeguard: if a future /api/search call is added,
// rapid keystrokes would otherwise hammer the server and cause RwLock contention on the
// Rust side. Local renderMessageList() remains immediately reactive inside the handler.
document.getElementById('search-input').addEventListener('input', (e) => {
    clearTimeout(state.searchDebounceTimer);
    state.searchDebounceTimer = setTimeout(() => {
        state.searchQuery = e.target.value;
        renderMessageList();
        saveSession();
    }, 300);
});

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
})();

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
