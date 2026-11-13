// Watch (#282, Milestone 4 stage 5): "tell me when a message arrives that
// matches this filter". Per browser, no server code. The bell in the list's
// controls takes a snapshot of the current filter — search, direction,
// validation — and from then on every new message that matches it while
// the tab is in the background
//   - raises a counter in the tab's title: `(3) Harux`,
//   - plays a short sound, if wanted,
//   - shows a desktop notification, if wanted and the browser allows it:
//     only on HTTPS or localhost (a secure context), never on plain
//     http://server:8080, the usual deployment.
// The menu says which of the three are active. Coming back to the tab
// clears the counter.
//
// Loaded after compose.js and before app.js; uses `state`, util.js
// (getParsedQuery, esc, escAttr, showToast) and render.js (matchesSearch).

const WATCH_KEY = 'harux_watch';           // the watch itself, this tab only
const WATCH_PREFS_KEY = 'harux_watch_prefs'; // sound / desktop, this browser
const WATCH_SOUND_GAP_MS = 2000;            // at most one beep per 2 s
const WATCH_NOTIFY_GAP_MS = 5000;           // at most one notification per 5 s
const VALIDATION_WATCH_LABELS = ['', 'with warnings', 'with errors'];
const DIRECTION_WATCH_LABELS = ['', 'received only', 'sent only'];

// --- The watch: a snapshot of a filter ---

// The current filter as a watch. The bookmark filter is left out: a new
// message is never bookmarked yet.
function watchFromFilter() {
    return {
        query: String(state.searchQuery || '').trim(),
        direction: state.directionFilter || 0,
        validation: state.validationFilter || 0,
        since: Date.now(),
    };
}

// Whether `msg` (a summary) matches `watch` — the list's own filter logic.
function watchMatches(watch, msg) {
    if (!watch || !msg) return false;
    if (watch.direction === 1 && msg.direction === 'out') return false;
    if (watch.direction === 2 && msg.direction !== 'out') return false;
    if (watch.validation === 1 && !((msg.validation_warning_count || 0) > 0 || msg.has_segment_errors)) return false;
    if (watch.validation === 2 && !msg.has_segment_errors) return false;
    return !watch.query || matchesSearch(msg, getParsedQuery(watch.query));
}

// "ADT · received only · with errors", or "every new message".
function describeWatch(watch) {
    if (!watch) return '';
    const parts = [];
    if (watch.query) parts.push(`“${watch.query}”`);
    if (DIRECTION_WATCH_LABELS[watch.direction]) parts.push(DIRECTION_WATCH_LABELS[watch.direction]);
    if (VALIDATION_WATCH_LABELS[watch.validation]) parts.push(VALIDATION_WATCH_LABELS[watch.validation]);
    return parts.length ? parts.join(' · ') : 'every new message';
}

// --- Storage (never fatal) ---

function loadWatch() {
    try {
        const raw = sessionStorage.getItem(WATCH_KEY);
        const w = raw ? JSON.parse(raw) : null;
        state.watch = w && typeof w === 'object' && typeof w.query === 'string'
            ? { query: w.query, direction: Number(w.direction) || 0, validation: Number(w.validation) || 0, since: Number(w.since) || Date.now() }
            : null;
    } catch (e) {
        state.watch = null;
    }
    try {
        const p = JSON.parse(localStorage.getItem(WATCH_PREFS_KEY) || '{}') || {};
        state.watchPrefs = { sound: p.sound === true, desktop: p.desktop === true };
    } catch (e) {
        state.watchPrefs = { sound: false, desktop: false };
    }
}

function saveWatch() {
    try {
        if (state.watch) sessionStorage.setItem(WATCH_KEY, JSON.stringify(state.watch));
        else sessionStorage.removeItem(WATCH_KEY);
    } catch (e) { /* lasts for the page then */ }
    try {
        localStorage.setItem(WATCH_PREFS_KEY, JSON.stringify(state.watchPrefs));
    } catch (e) { /* lasts for the page then */ }
}

// --- What the browser allows ---

// `available`: the API exists and the page is a secure context; `granted`:
// the user allowed it; `reason`: why not, in a sentence.
function desktopNotificationStatus() {
    const N = typeof Notification === 'function' ? Notification : null;
    if (!N) return { available: false, granted: false, reason: 'This browser has no desktop notifications' };
    if (typeof isSecureContext !== 'undefined' && !isSecureContext) {
        return { available: false, granted: false, reason: 'Browsers allow desktop notifications only on HTTPS or localhost' };
    }
    if (N.permission === 'denied') return { available: true, granted: false, reason: 'Blocked in this browser\'s site settings' };
    return { available: true, granted: N.permission === 'granted', reason: '' };
}

// The ways that will actually tell the user: the title counter always.
function activeWatchChannels() {
    const out = ['title counter'];
    if (state.watchPrefs.sound) out.push('sound');
    const d = desktopNotificationStatus();
    if (state.watchPrefs.desktop && d.available && d.granted) out.push('desktop notification');
    return out;
}

// --- Counting ---

function pageHidden() {
    return typeof document !== 'undefined' && document.hidden === true;
}

// Called for every new message (ws.js, handleServerEvent). Messages that
// arrive while the WebSocket is away (`lagged`, a reconnect) come in with the
// reloaded list, not as `new_message`, and are not counted: the counter is a
// nudge, the list is what counts (#295 review).
function watchNewMessage(msg) {
    if (!state.watch || !pageHidden() || !watchMatches(state.watch, msg)) return;
    state.watchCount += 1;
    updateWatchTitle();
    const now = Date.now();
    if (state.watchPrefs.sound && now - state.watchLastSound >= WATCH_SOUND_GAP_MS) {
        state.watchLastSound = now;
        playWatchSound();
    }
    if (state.watchPrefs.desktop && now - state.watchLastNotify >= WATCH_NOTIFY_GAP_MS) {
        state.watchLastNotify = now;
        showWatchNotification(msg);
    }
}

// `(3) Harux`; the title without a counter when there is none.
function watchTitle(base, count) {
    return count > 0 ? `(${count > 99 ? '99+' : count}) ${base}` : base;
}

function updateWatchTitle() {
    if (typeof document === 'undefined') return;
    if (!state.watchBaseTitle) state.watchBaseTitle = document.title.replace(/^\(\d+\+?\) /, '') || 'Harux';
    document.title = watchTitle(state.watchBaseTitle, state.watchCount);
}

// Back in the tab: the messages are in front of the user.
function onWatchVisibility() {
    if (pageHidden() || state.watchCount === 0) return;
    state.watchCount = 0;
    updateWatchTitle();
}

let watchAudio = null;
// A short beep, made by the browser: no sound file in the binary.
function playWatchSound() {
    try {
        const Ctx = window.AudioContext || window.webkitAudioContext;
        if (!Ctx) return;
        watchAudio = watchAudio || new Ctx();
        // Made outside a click (the tab is in the background), a context can
        // start suspended (#295 review).
        if (watchAudio.state === 'suspended' && watchAudio.resume) watchAudio.resume();
        const osc = watchAudio.createOscillator();
        const gain = watchAudio.createGain();
        osc.frequency.value = 880;
        gain.gain.setValueAtTime(0.08, watchAudio.currentTime);
        gain.gain.exponentialRampToValueAtTime(0.0001, watchAudio.currentTime + 0.18);
        osc.connect(gain).connect(watchAudio.destination);
        osc.start();
        osc.stop(watchAudio.currentTime + 0.2);
    } catch (e) {
        console.debug('Watch: no sound', e);
    }
}

// Type, source and control ID only — no patient data on the desktop.
function watchNotificationText(msg, count) {
    const what = msg.message_type || 'A message';
    const from = msg.direction === 'out' ? `to ${msg.target || msg.source_addr || '?'}` : `from ${msg.source_addr || '?'}`;
    const id = msg.message_control_id ? ` (${msg.message_control_id})` : '';
    return {
        title: count > 1 ? `Harux: ${count} matching messages` : 'Harux: a matching message',
        body: `${what} ${from}${id}`,
    };
}

function showWatchNotification(msg) {
    const d = desktopNotificationStatus();
    if (!d.available || !d.granted) return;
    try {
        const t = watchNotificationText(msg, state.watchCount);
        const n = new Notification(t.title, { body: t.body, tag: 'harux-watch' });
        n.onclick = () => { window.focus(); n.close(); };
    } catch (e) {
        console.debug('Watch: no notification', e);
    }
}

// --- The bell and its menu ---

function renderWatchButton() {
    const btn = document.getElementById('btn-watch');
    if (!btn) return;
    const on = !!state.watch;
    btn.classList.toggle('on', on);
    btn.setAttribute('aria-pressed', on ? 'true' : 'false');
    btn.setAttribute('aria-expanded', state.watchMenuOpen ? 'true' : 'false');
    btn.title = on ? `Watching ${describeWatch(state.watch)}` : 'Tell me when a message arrives that matches the current filter';
    const label = btn.querySelector('.btn-label');
    if (label) label.textContent = on ? 'Watching' : 'Watch';
}

function watchMenuHtml() {
    const w = state.watch;
    const current = watchFromFilter();
    const d = desktopNotificationStatus();
    const sw = (key, on, label, disabled, hint) => `<div class="row">`
        + `<button class="switch${on ? ' on' : ''}" role="switch" aria-checked="${on}" data-action="watch-pref" data-pref="${key}" aria-label="${escAttr(label)}"${disabled ? ' disabled' : ''}></button>`
        + `<span>${esc(label)}</span>${hint ? `<span class="hint">${esc(hint)}</span>` : ''}</div>`;
    const desktopHint = !d.available ? d.reason : (d.reason || (state.watchPrefs.desktop && !d.granted ? 'Allow it when the browser asks' : ''));
    const head = w
        ? `<p class="what">Watching <b>${esc(describeWatch(w))}</b></p>`
        : `<p class="what">Watch <b>${esc(describeWatch(current))}</b> — the current filter</p>`;
    const change = w && JSON.stringify({ ...current, since: 0 }) !== JSON.stringify({ ...w, since: 0 })
        ? `<button class="ctl-btn" data-action="watch-start">Watch the current filter instead</button>` : '';
    return `${head}
        <p class="hint">While this tab is in the background, matching messages count in its title${state.showBookmarkedOnly ? ' (the bookmark filter is left out: new messages are never bookmarked)' : ''}.</p>
        ${sw('sound', state.watchPrefs.sound, 'Play a sound', false, '')}
        ${sw('desktop', state.watchPrefs.desktop && d.available && d.granted, 'Desktop notification', !d.available, desktopHint)}
        <p class="active">Active: ${esc(activeWatchChannels().join(', '))}</p>
        <div class="foot">${change}<span class="spacer"></span>
        ${w ? '<button class="ctl-btn" data-action="watch-stop">Stop watching</button>' : '<button class="ctl-btn on" data-action="watch-start">Watch</button>'}</div>`;
}

function renderWatchMenu() {
    const host = document.getElementById('run-menu-host');
    if (!host) return;
    let menu = document.getElementById('watch-menu');
    if (!state.watchMenuOpen) {
        if (menu) menu.remove();
        return;
    }
    if (!menu) {
        menu = document.createElement('div');
        menu.id = 'watch-menu';
        menu.className = 'send-menu watch-menu';
        menu.setAttribute('role', 'dialog');
        menu.setAttribute('aria-label', 'Watch');
        host.appendChild(menu);
    }
    menu.innerHTML = watchMenuHtml();
}

function toggleWatchMenu(open = !state.watchMenuOpen) {
    if (open && state.sendMenu && typeof closeSendMenu === 'function') closeSendMenu();
    state.watchMenuOpen = open;
    renderWatchMenu();
    renderWatchButton();
}

function startWatch() {
    state.watch = watchFromFilter();
    state.watchCount = 0;
    saveWatch();
    updateWatchTitle();
    renderWatchMenu();
    renderWatchButton();
    showToast(`Watching ${describeWatch(state.watch)}`, 'info');
}

function stopWatch() {
    state.watch = null;
    state.watchCount = 0;
    saveWatch();
    updateWatchTitle();
    toggleWatchMenu(false);
}

async function setWatchPref(key) {
    if (key !== 'sound' && key !== 'desktop') return;
    const on = !state.watchPrefs[key];
    if (key === 'desktop' && on) {
        const d = desktopNotificationStatus();
        if (!d.available) return;
        // The browser asks only from a click; a "no" is remembered by it.
        if (!d.granted && typeof Notification.requestPermission === 'function') {
            try {
                await Notification.requestPermission();
            } catch (e) {
                console.debug('Watch: permission request failed', e);
            }
        }
    }
    state.watchPrefs[key] = on;
    if (key === 'sound' && on) playWatchSound(); // what it sounds like
    saveWatch();
    renderWatchMenu();
}

// Handles the watch `data-action`s; returns true if it took one.
function handleWatchAction(action, el) {
    switch (action) {
        case 'watch-menu': toggleWatchMenu(); return true;
        case 'watch-start': startWatch(); return true;
        case 'watch-stop': stopWatch(); return true;
        case 'watch-pref': setWatchPref(el.dataset.pref); return true;
        default: return false;
    }
}

function initWatch() {
    loadWatch();
    renderWatchButton();
    if (typeof document !== 'undefined' && document.addEventListener) {
        document.addEventListener('visibilitychange', onWatchVisibility);
    }
}
