// Sending from the UI (#274, Milestone 4 stage 1): the user's own target
// list, the Resend split button and its menu. The server keeps no targets
// (decision D2): this browser keeps them in localStorage, the request names
// the target, and the sent message comes back as a row over the WebSocket —
// never added here from the answer (`addMessage` does not de-duplicate).
//
// Loaded after diff.js and before app.js; uses `state`, util.js helpers
// (esc, escAttr, showToast, newId, deliveryChip, formatLatency) and
// render.js (renderDetail).

const TARGETS_KEY = 'harux_targets';
const TARGET_RESULTS_KEY = 'harux_target_results';
const LAST_TARGET_KEY = 'harux_last_target';
const SEND_AS_KEY = 'harux_send_as';

// How a resend goes out (#275): byte for byte, or with fields set through
// the server's overrides. Remembered per browser for the one-click resend.
const SEND_AS = {
    unchanged: { label: 'As received', code: 'byte for byte', set: {} },
    new_id: { label: 'With a new control ID', code: 'MSH-10', set: { 'MSH-10': '{{id}}' } },
    new_id_time: {
        label: 'With a new control ID and the current time',
        code: 'MSH-10, MSH-7',
        set: { 'MSH-10': '{{id}}', 'MSH-7': '{{now}}' },
    },
};
const LOOPBACK_TARGET_ID = 'loopback';
const MAX_TARGETS = 200;
const MAX_TARGET_NAME = 64;

// --- Storage (never fatal: a browser that refuses storage keeps the list
// for the session, and the file export still works) ---

function readStored(key, fallback) {
    try {
        const raw = localStorage.getItem(key);
        return raw ? JSON.parse(raw) : fallback;
    } catch (e) {
        console.debug(`Could not read ${key}:`, e);
        return fallback;
    }
}

function writeStored(key, value) {
    try {
        localStorage.setItem(key, JSON.stringify(value));
        return true;
    } catch (e) {
        console.debug(`Could not save ${key}:`, e);
        if (!state.targetStorageWarned) {
            state.targetStorageWarned = true;
            showToast('This browser does not keep your targets — they last until the page is closed. "Save to file" keeps them.');
        }
        return false;
    }
}

// --- The target model ---

// A target as stored and sent: `{ id, name, host, port, tls, verify, ask }`,
// or null if `raw` has no usable host and port. TLS is on unless switched
// off (a new target uses it, decision D4); `verify: false` is only offered
// when the server allows unverified connections.
function normalizeTarget(raw) {
    if (!raw || typeof raw !== 'object') return null;
    const host = String(raw.host ?? '').trim();
    if (hostHasPort(host)) return null;
    const port = Number(raw.port);
    if (!host || host.length > 253 || /[\s/]/.test(host)) return null;
    if (!Number.isInteger(port) || port < 1 || port > 65535) return null;
    const name = String(raw.name ?? '').trim().slice(0, MAX_TARGET_NAME) || `${host}:${port}`;
    return {
        id: typeof raw.id === 'string' && raw.id ? raw.id : newId(),
        name,
        host,
        port,
        tls: raw.tls !== false,
        verify: raw.verify !== false,
        ask: raw.ask === true,
    };
}

// `name:port` or `1.2.3.4:port` typed into the Host field. An IPv6 address
// has several colons and is left alone (the server checks hosts anyway).
function hostHasPort(host) {
    return /^[^:]+:\d*$/.test(host) || /^\[.*\]:\d*$/.test(host);
}

// A target list from a file: `{ targets: [...] }` as "Save to file" writes
// it, or a bare array. Throws with a sentence for the user.
function parseTargetFile(text) {
    let data;
    try {
        data = JSON.parse(text);
    } catch (e) {
        throw new Error('The file is not a target list (not JSON)');
    }
    const list = Array.isArray(data) ? data : (data && Array.isArray(data.targets) ? data.targets : null);
    if (!list) throw new Error('The file holds no target list');
    const targets = list.map(normalizeTarget);
    const bad = targets.findIndex((t) => !t);
    if (bad >= 0) throw new Error(`Entry ${bad + 1} of the file has no valid host and port`);
    return targets.slice(0, MAX_TARGETS);
}

function loadTargets() {
    const stored = readStored(TARGETS_KEY, []);
    state.targets = (Array.isArray(stored) ? stored : []).map(normalizeTarget).filter(Boolean);
    state.targetsSeen = new Set(state.targets.map((t) => t.id));
    const results = readStored(TARGET_RESULTS_KEY, {});
    state.targetResults = results && typeof results === 'object' ? results : {};
    const last = readStored(LAST_TARGET_KEY, null);
    state.lastTargetId = typeof last === 'string' ? last : null;
    const sendAs = readStored(SEND_AS_KEY, 'unchanged');
    state.sendAs = Object.hasOwn(SEND_AS, sendAs) ? sendAs : 'unchanged';
}

// The list is shared by every tab of the browser (#300): merged with what
// is stored now, so a target another tab added stays (and comes into this
// tab), one removed here stays out, and one another tab removed is not
// written back.
function saveTargets() {
    const stored = readStored(TARGETS_KEY, []);
    const current = (Array.isArray(stored) ? stored : []).map(normalizeTarget).filter(Boolean);
    const { list } = mergeStoredList(current, state.targets, state.targetsRemoved, state.targetsSeen);
    state.targets = list.slice(0, MAX_TARGETS);
    if (writeStored(TARGETS_KEY, state.targets)) state.targetsSeen = new Set(state.targets.map((t) => t.id));
}

// Another tab wrote the list (a `storage` event, #300): every write is
// already a merge, so it is the list as it is now.
function syncTargets(text) {
    let list;
    try {
        list = text ? JSON.parse(text) : [];
    } catch (e) {
        return;
    }
    if (!Array.isArray(list)) return;
    state.targets = list.map(normalizeTarget).filter((t) => t && !state.targetsRemoved.has(t.id)).slice(0, MAX_TARGETS);
    state.targetsSeen = new Set(state.targets.map((t) => t.id));
    if (state.sendMenu) {
        if (!findTarget(state.sendMenu.pickedId)) state.sendMenu.pickedId = (allTargets()[0] || {}).id || null;
        renderSendMenu();
    }
    updateSendButtons();
}

// "This Harux": the server's own MLLP listener, always in the list while
// the server offers it (decision D6). Never stored — it comes from
// /api/send/config.
function loopbackTarget() {
    const lb = state.sendConfig && state.sendConfig.loopback;
    if (!lb) return null;
    return { id: LOOPBACK_TARGET_ID, name: lb.name, host: lb.host, port: lb.port, tls: false, verify: true, ask: false, loopback: true };
}

function allTargets() {
    const lb = loopbackTarget();
    return lb ? [...state.targets, lb] : [...state.targets];
}

function findTarget(id) {
    return allTargets().find((t) => t.id === id) || null;
}

// Add a new target or replace the one with the same id; returns it.
function upsertTarget(target) {
    const i = state.targets.findIndex((t) => t.id === target.id);
    if (i >= 0) state.targets[i] = target;
    else state.targets.push(target);
    saveTargets();
    return target;
}

// Forget the last results of targets that no longer exist (#286 review).
function pruneTargetResults() {
    const keep = new Set([LOOPBACK_TARGET_ID, ...state.targets.map((t) => t.id)]);
    let changed = false;
    for (const id of Object.keys(state.targetResults)) {
        if (!keep.has(id)) {
            delete state.targetResults[id];
            changed = true;
        }
    }
    if (changed) writeStored(TARGET_RESULTS_KEY, state.targetResults);
}

function removeTarget(id) {
    state.targets = state.targets.filter((t) => t.id !== id);
    state.targetsRemoved.add(id); // never merged back from the stored list (#300)
    saveTargets();
    pruneTargetResults();
    if (state.lastTargetId === id) {
        state.lastTargetId = null;
        writeStored(LAST_TARGET_KEY, null);
    }
}

// The `target` of POST /api/send.
function targetRequest(t) {
    if (t.loopback) return 'loopback';
    const r = { name: t.name, host: t.host, port: t.port, tls: t.tls };
    if (t.tls) r.verify = t.verify;
    return r;
}

// --- Last result per target (this browser only) ---

function recordTargetResult(id, delivery) {
    state.targetResults[id] = {
        state: delivery.state,
        code: delivery.ack_code || null,
        error: delivery.error || null,
        ms: typeof delivery.latency_ms === 'number' ? delivery.latency_ms : null,
        at: Date.now(),
    };
    writeStored(TARGET_RESULTS_KEY, state.targetResults);
}

function sinceLabel(ms) {
    if (ms < 86_400_000) return formatRelativeTime(ms);
    const days = Math.floor(ms / 86_400_000);
    return days === 1 ? 'yesterday' : `${days} days ago`;
}

// `{ text, when, cls }` for the menu: "AA · 42 ms" / "2m ago", "refused".
function describeTargetResult(r, now) {
    if (!r) return { text: 'not used yet', when: '', cls: '' };
    const when = sinceLabel(Math.max(0, now - r.at));
    if (r.state === 'failed') return { text: r.error || 'failed', when, cls: 'bad' };
    if (r.state === 'sent') return { text: 'sent', when, cls: 'ok' };
    const code = r.code || '?';
    const ms = r.ms != null ? ` · ${formatLatency(r.ms)}` : '';
    return { text: `${code}${ms}`, when, cls: code === 'AA' || code === 'CA' ? 'ok' : 'bad' };
}

// --- Sending ---

// Whether the server sends at all, and this message can be resent: an
// over-limit entry (#223) holds only the beginning of its frame.
function canResend(msg) {
    return !!(state.sendConfig && state.sendConfig.enabled && msg && msg.id && !msg.truncated);
}

// Load /api/send/config: `null` while sending is off (403), so the UI
// shows nothing new (decision D11).
async function loadSendConfig() {
    let config = null;
    try {
        const resp = await fetch('/api/send/config');
        if (resp.ok) config = await resp.json();
    } catch (e) {
        console.debug('Could not read the send configuration:', e);
    }
    const changed = JSON.stringify(config) !== JSON.stringify(state.sendConfig);
    state.sendConfig = config && config.enabled ? config : null;
    if (changed) applySendingUi();
}

// Show or hide what sending adds, and redraw an open detail panel.
function applySendingUi() {
    const dirBtn = document.getElementById('btn-direction');
    if (dirBtn) dirBtn.classList.toggle('hidden', !state.sendConfig && state.directionFilter === 0);
    const runBtn = document.getElementById('btn-resend-filtered');
    if (runBtn) runBtn.classList.toggle('hidden', !state.sendConfig);
    if (!state.sendConfig) closeSendMenu();
    if (state.selectedMessage) renderDetail();
    if (typeof applyDraftsUi === 'function') applyDraftsUi(); // #277
}

// Send `msg` to `target`: asks first for a target marked *ask before
// sending* (decision D3) and for a message that did not parse. The sent
// message appears over the WebSocket; this only reports the outcome.
async function resendMessage(msg, target, sendAs = state.sendAs) {
    if (!msg || !target || state.sending) return false;
    const way = SEND_AS[sendAs] || SEND_AS.unchanged;
    const changes = Object.keys(way.set).length > 0;
    if (changes && msg.parse_error) {
        showToast('A message that did not parse can only be resent as received');
        return false;
    }
    if (target.ask && !confirm(`Send ${msg.message_type || 'this message'} to ${target.name} (${target.host}:${target.port})?`)) {
        return false;
    }
    let allowInvalid = false;
    if (msg.parse_error) {
        if (!confirm('This message did not parse. Send it exactly as it was received anyway?')) return false;
        allowInvalid = true;
    }
    state.sending = true;
    updateSendButtons();
    try {
        const resp = await fetch('/api/send', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                target: targetRequest(target),
                items: [{ message_id: msg.id }],
                allow_invalid: allowInvalid,
                ...(changes ? { set: way.set } : {}),
            }),
        });
        if (!resp.ok) {
            showToast(`Not sent: ${(await resp.text()) || `HTTP ${resp.status}`}`);
            return false;
        }
        const result = await resp.json();
        state.lastTargetId = target.id;
        writeStored(LAST_TARGET_KEY, target.id);
        recordTargetResult(target.id, result.delivery);
        const chip = deliveryChip(result.delivery);
        const latency = formatLatency(result.delivery.latency_ms);
        showToast(`Sent to ${target.name} · ${chip.text}${latency ? ' · ' + latency : ''}`,
            result.delivery.state === 'failed' ? 'error' : 'info');
        return true;
    } catch (e) {
        showToast('Not sent: ' + (e.message || e));
        return false;
    } finally {
        state.sending = false;
        updateSendButtons();
    }
}

// The left part of the split button: the target used last; the first use
// opens the menu instead.
function resendOneClick() {
    const target = findTarget(state.lastTargetId);
    if (!target) {
        openSendMenu();
        return;
    }
    resendMessage(state.selectedMessage, target);
}

// --- Detail header: the split button ---

function sendActionsHtml(msg) {
    if (!canResend(msg)) return '';
    const target = findTarget(state.lastTargetId);
    const to = target ? `<span class="to">→ ${esc(target.name)}</span>` : '';
    const way = SEND_AS[state.sendAs] || SEND_AS.unchanged;
    const how = state.sendAs === 'unchanged' ? 'byte for byte' : way.label.toLowerCase();
    // aria-disabled, not disabled: a disabled button drops keyboard focus,
    // and resendMessage ignores clicks while a send is under way anyway.
    const busy = state.sending ? ' aria-disabled="true"' : '';
    const open = state.sendMenu && !state.sendMenu.forRun ? 'true' : 'false';
    // *Edit* (#277) opens the message as a draft.
    return `<span class="split-btn" id="send-split">
        <button class="ctl-btn" data-action="resend"${busy} title="${target ? `Send this message again to ${escAttr(target.name)}, ${escAttr(how)}` : 'Choose a target'}">${ICONS.resend}<span class="btn-label">Resend</span>${to}</button><button class="ctl-btn caret" data-action="resend-menu" aria-haspopup="dialog" aria-expanded="${open}" aria-label="Choose target" title="Choose target">${ICONS.chevronDown}</button>
    </span><button class="ctl-btn" data-action="draft-edit" data-id="${escAttr(msg.id)}" title="Open this message as a draft to change and send it">${ICONS.edit}<span class="btn-label">Edit</span></button>`;
}

function updateSendButtons() {
    if (typeof isDraftOpen === 'function' && isDraftOpen()) {
        updateDraftActions(); // #277: the draft's target button
        return;
    }
    const split = document.getElementById('send-split');
    if (!split || !state.selectedMessage) return;
    // Keep keyboard focus on the button that had it (#286 review).
    const active = document.activeElement;
    const focused = active && split.contains && split.contains(active) ? active.dataset.action : null;
    const tmp = document.createElement('div');
    tmp.innerHTML = sendActionsHtml(state.selectedMessage);
    const fresh = tmp.firstElementChild;
    if (!fresh) return;
    split.replaceWith(fresh);
    if (focused) {
        const again = fresh.querySelector(`[data-action="${focused}"]`);
        if (again) again.focus();
    }
}

// --- The menu ---

// The menu also picks the target of an open draft (#277); then it has no
// "Send as" (a draft is sent as written) and its Send sends the draft.
function openSendMenu() {
    const draft = typeof openDraftObject === 'function' ? openDraftObject() : null;
    if (draft ? !state.sendConfig : !canResend(state.selectedMessage)) return;
    const picked = (draft && draftTarget(draft)) || findTarget(state.lastTargetId) || allTargets()[0] || null;
    state.sendMenu = {
        forRun: false,
        forDraft: draft ? draft.id : null,
        pickedId: picked ? picked.id : null,
        // No target at all (not even This Harux): start with the form.
        form: allTargets().length === 0 ? emptyTargetForm() : null,
    };
    renderSendMenu();
    updateSendButtons();
}

// The same menu for *Resend filtered* (#280), under the list's controls:
// the target, *Send as*, and the number of messages the run sends.
function openRunMenu() {
    if (!state.sendConfig) return;
    if (state.sendMenu && typeof closeSendMenu === 'function') closeSendMenu();
    const last = findTarget(state.lastTargetId);
    const picked = (last && !last.ask ? last : null) || allTargets().find((t) => !t.ask) || allTargets()[0] || null;
    state.sendMenu = {
        forRun: true,
        forDraft: null,
        pickedId: picked ? picked.id : null,
        form: allTargets().length === 0 ? emptyTargetForm() : null,
    };
    renderSendMenu();
    updateRunButton();
}

function closeSendMenu() {
    if (!state.sendMenu) return;
    const wasRun = state.sendMenu.forRun;
    state.sendMenu = null;
    const menu = document.getElementById('send-menu');
    if (menu) menu.remove();
    if (wasRun) updateRunButton();
    else updateSendButtons();
}

function toggleSendMenu() {
    if (state.sendMenu && !state.sendMenu.forRun) closeSendMenu();
    else openSendMenu();
}

function toggleRunMenu() {
    if (state.sendMenu && state.sendMenu.forRun) closeSendMenu();
    else if (runUnderWay()) showToast('A run is under way — wait for it or cancel it first');
    else openRunMenu();
}

function emptyTargetForm() {
    return { editingId: null, name: '', host: '', port: '', tls: true, verify: true, ask: false, error: '' };
}

function switchHtml(flag, on, label) {
    return `<button class="switch${on ? ' on' : ''}" role="switch" aria-checked="${on}" data-action="target-flag" data-flag="${flag}" aria-label="${escAttr(label)}"></button><span>${esc(label)}</span>`;
}

function targetOptionHtml(t, picked, now, forRun = false) {
    const r = describeTargetResult(state.targetResults[t.id], now);
    const tls = t.tls ? `<span class="tls${t.verify ? '' : ' unverified'}">${t.verify ? 'TLS' : 'TLS unverified'}</span>` : '';
    // Decision D3: a target that asks before every message takes no runs.
    const guard = t.ask ? `<span class="guard">${forRun ? '⚠ asks before sending — no runs' : '⚠ asks before sending'}</span>` : '';
    const addr = `${t.host}:${t.port}${t.loopback ? ' · loopback' : ''}`;
    const edit = t.loopback ? '' : `<button class="target-edit" data-action="target-edit" data-target-id="${escAttr(t.id)}" title="Edit ${escAttr(t.name)}" aria-label="Edit ${escAttr(t.name)}">Edit</button>`;
    return `<div class="target-option${picked ? ' selected' : ''}" role="radio" aria-checked="${picked}" tabindex="0" data-action="target-pick" data-target-id="${escAttr(t.id)}">
        <span class="radio"></span>
        <span class="name">${esc(t.name)} ${tls} ${guard}</span>
        <span class="last ${r.cls}">${esc(r.text)}${r.when ? `<br>${esc(r.when)}` : ''}</span>
        <span class="addr">${esc(addr)}</span>
        ${edit}
    </div>`;
}

function targetFormHtml(f) {
    const allowUnverified = !!(state.sendConfig && state.sendConfig.tls && state.sendConfig.tls.allow_unverified);
    const verify = f.tls && allowUnverified ? switchHtml('verify', f.verify, 'Check certificate') : '';
    return `<div class="target-form">
        <h4>${f.editingId ? 'Edit target' : 'Add target'}</h4>
        <div class="row">
            <input type="text" id="tf-name" placeholder="Name" aria-label="Name" maxlength="${MAX_TARGET_NAME}" value="${escAttr(f.name)}">
            <input type="text" id="tf-host" class="host" placeholder="Host" aria-label="Host" value="${escAttr(f.host)}">
            <input type="text" id="tf-port" class="port" placeholder="Port" aria-label="Port" inputmode="numeric" value="${escAttr(String(f.port))}">
        </div>
        <div class="row">
            ${switchHtml('tls', f.tls, 'TLS')}
            ${verify}
            ${switchHtml('ask', f.ask, 'Ask before sending')}
        </div>
        <div class="row">
            <span class="form-error" role="alert">${esc(f.error || '')}</span>
            <span class="spacer"></span>
            ${f.editingId ? '<button class="ctl-btn" data-action="target-delete">Delete</button>' : ''}
            <button class="ctl-btn" data-action="target-cancel">Cancel</button>
            <button class="ctl-btn on" data-action="target-save">Save</button>
        </div>
    </div>`;
}

function renderSendMenu() {
    const m = state.sendMenu;
    const host = m && document.getElementById(m.forRun ? 'run-menu-host' : 'detail-header');
    if (!host || !m) return;
    const now = Date.now();
    const targets = allTargets();
    const options = targets.map((t) => targetOptionHtml(t, t.id === m.pickedId, now, m.forRun)).join('');
    const picked = findTarget(m.pickedId);
    let menu = document.getElementById('send-menu');
    if (menu && menu.parentNode !== host) {
        menu.remove();
        menu = null;
    }
    if (!menu) {
        menu = document.createElement('div');
        menu.id = 'send-menu';
        menu.className = 'send-menu';
        menu.setAttribute('role', 'dialog');
        host.appendChild(menu);
    }
    menu.classList.toggle('run-menu', !!m.forRun);
    const forDraft = !!m.forDraft;
    menu.setAttribute('aria-label', m.forRun ? 'Resend the filtered messages' : (forDraft ? 'Target of this draft' : 'Resend this message'));
    const run = m.forRun ? runCandidates(filteredForRun()) : null;
    const canSend = picked && !state.sending && (!run || (run.send.length > 0 && !picked.ask));
    const hint = run ? runMenuHint(run, picked)
        : (forDraft ? 'The draft is sent as written, placeholders filled in.' : 'Target and way are remembered for the next one-click resend.');
    const label = run ? `Resend ${run.send.length}` : 'Send';
    menu.innerHTML = `
        <h4>My targets <span class="h4-actions">
            <button class="link-btn" data-action="target-add">Add</button> ·
            <button class="link-btn" data-action="targets-save-file">Save to file</button> ·
            <button class="link-btn" data-action="targets-load-file">Load</button>
        </span></h4>
        <div class="target-list" role="radiogroup" aria-label="Targets">${options || '<div class="target-empty">No targets yet</div>'}</div>
        ${m.form ? targetFormHtml(m.form) : ''}
        <hr>
        ${forDraft ? '' : `<h4>Send as</h4>
        <div class="send-as" role="radiogroup" aria-label="Send as">${sendAsHtml()}</div>`}
        <div class="foot">
            <span class="hint">${esc(hint)}</span>
            <button class="ctl-btn on send-btn" data-action="send-picked"${canSend ? '' : ' disabled'}>${ICONS.resend}<span class="btn-label">${esc(label)}</span></button>
        </div>`;
    if (m.form) {
        const first = document.getElementById(m.form.host ? 'tf-name' : (m.form.name ? 'tf-host' : 'tf-name'));
        if (first && document.activeElement && !menu.contains(document.activeElement)) first.focus();
    }
}

function sendAsHtml() {
    return Object.entries(SEND_AS).map(([key, way]) => {
        const on = key === state.sendAs;
        return `<div class="opt-row${on ? ' selected' : ''}" role="radio" aria-checked="${on}" tabindex="0" data-action="send-as" data-value="${key}">
            <span class="radio"></span>${esc(way.label)} <code>${esc(way.code)}</code>
        </div>`;
    }).join('');
}

function chooseSendAs(key) {
    if (!Object.hasOwn(SEND_AS, key)) return;
    state.sendAs = key;
    writeStored(SEND_AS_KEY, key);
    if (state.sendMenu) renderSendMenu();
    updateSendButtons();
}

// Copy what was typed into the form state before a re-render.
function readTargetForm() {
    const f = state.sendMenu && state.sendMenu.form;
    if (!f) return null;
    for (const [key, id] of [['name', 'tf-name'], ['host', 'tf-host'], ['port', 'tf-port']]) {
        const el = document.getElementById(id);
        if (el) f[key] = el.value;
    }
    return f;
}

function saveTargetForm() {
    const f = readTargetForm();
    if (!f) return;
    const target = normalizeTarget({
        id: f.editingId || undefined,
        name: f.name,
        host: f.host,
        port: Number(String(f.port).trim()),
        tls: f.tls,
        verify: f.verify,
        ask: f.ask,
    });
    if (!target) {
        f.error = hostHasPort(String(f.host).trim())
            ? 'Put the port in the Port field, not after the host'
            : 'Host and a port from 1 to 65535 are needed';
        renderSendMenu();
        return;
    }
    if (!f.editingId && state.targets.length >= MAX_TARGETS) {
        f.error = `At most ${MAX_TARGETS} targets`;
        renderSendMenu();
        return;
    }
    upsertTarget(target);
    state.sendMenu.form = null;
    state.sendMenu.pickedId = target.id;
    renderSendMenu();
}

function editTarget(id) {
    const t = state.targets.find((x) => x.id === id);
    if (!t || !state.sendMenu) return;
    state.sendMenu.form = { editingId: t.id, name: t.name, host: t.host, port: String(t.port), tls: t.tls, verify: t.verify, ask: t.ask, error: '' };
    renderSendMenu();
}

function deleteEditedTarget() {
    const f = state.sendMenu && state.sendMenu.form;
    if (!f || !f.editingId) return;
    const t = state.targets.find((x) => x.id === f.editingId);
    if (t && !confirm(`Remove the target ${t.name}?`)) return;
    removeTarget(f.editingId);
    if (state.sendMenu.pickedId === f.editingId) state.sendMenu.pickedId = (allTargets()[0] || {}).id || null;
    state.sendMenu.form = null;
    renderSendMenu();
    updateSendButtons();
}

function toggleTargetFlag(flag) {
    const f = readTargetForm();
    if (!f || !['tls', 'verify', 'ask'].includes(flag)) return;
    f[flag] = !f[flag];
    renderSendMenu();
}

function pickTarget(id) {
    if (!state.sendMenu || !findTarget(id)) return;
    state.sendMenu.pickedId = id;
    // A draft remembers its target (#277).
    const draft = state.sendMenu.forDraft && findDraft(state.sendMenu.forDraft);
    if (draft) {
        draft.targetId = id;
        saveDrafts();
        renderDraftHeader();
    }
    renderSendMenu();
}

async function sendPicked() {
    const target = state.sendMenu && findTarget(state.sendMenu.pickedId);
    if (state.sendMenu && state.sendMenu.forRun) {
        if (await resendFiltered(target)) closeSendMenu();
        else if (state.sendMenu) renderSendMenu();
        return;
    }
    const draft = state.sendMenu && state.sendMenu.forDraft && findDraft(state.sendMenu.forDraft);
    if (draft) {
        if (await sendDraft(draft, target)) closeSendMenu();
        else if (state.sendMenu) renderSendMenu();
        return;
    }
    const msg = state.selectedMessage;
    if (!target || !msg) return;
    if (await resendMessage(msg, target)) closeSendMenu();
    else if (state.sendMenu) renderSendMenu();
}

// --- Runs (#280): several messages in order over one connection ---

const RUN_POLL_MS = 500;
const RUN_END_TEXT = {
    done: 'Done',
    cancelled: 'Cancelled',
    gave_up: 'Gave up',
    stopped_on_nack: 'Stopped at an error',
    shutdown: 'Harux shut down',
    lost: 'No longer known to the server',
};

// The loaded messages the filters show, oldest first: the order a run
// sends them in.
function filteredForRun() {
    return typeof filteredMessages === 'function' ? filteredMessages().slice().reverse() : [];
}

// What a run of `list` sends: an over-limit entry (#223) holds only the
// beginning of its frame and is left out; a message that did not parse
// goes out as received, after a second confirmation.
function runCandidates(list) {
    const send = list.filter((m) => m.id && !m.truncated);
    return {
        send,
        truncated: list.length - send.length,
        unparsed: send.filter((m) => m.parse_error).length,
    };
}

function runMenuHint(run, target) {
    if (run.send.length === 0) return 'The filters show no message to resend.';
    if (target && target.ask) return `${target.name} asks before every message — it takes no runs. Pick another target.`;
    const max = state.sendConfig ? state.sendConfig.max_run_messages : 0;
    const parts = [`The ${run.send.length} loaded message${run.send.length === 1 ? '' : 's'} the filters show, oldest first, over one connection.`];
    if (run.truncated) parts.push(`${run.truncated} cut-off ${run.truncated === 1 ? 'entry is' : 'entries are'} left out.`);
    if (max && run.send.length > max) parts.push(`At most ${max} per run (send.max_run_messages) — narrow the filter.`);
    return parts.join(' ');
}

// *Resend filtered*: asks first, naming target and number (always).
async function resendFiltered(target) {
    if (!target) return false;
    const run = runCandidates(filteredForRun());
    const n = run.send.length;
    if (n === 0) {
        showToast('The filters show no message to resend');
        return false;
    }
    if (target.ask) {
        showToast(`${target.name} asks before every message — it takes no runs`);
        return false;
    }
    const max = state.sendConfig ? state.sendConfig.max_run_messages : n;
    if (n > max) {
        showToast(`${n} messages are more than one run takes (${max}, send.max_run_messages) — narrow the filter`);
        return false;
    }
    const way = SEND_AS[state.sendAs] || SEND_AS.unchanged;
    const changes = Object.keys(way.set).length > 0;
    if (changes && run.unparsed) {
        showToast(`${run.unparsed} of them did not parse and can only be resent as received — choose "As received" or filter them out`);
        return false;
    }
    const how = state.sendAs === 'unchanged' ? 'byte for byte' : way.label.toLowerCase();
    const left = run.truncated ? ` ${run.truncated} cut-off ${run.truncated === 1 ? 'entry is' : 'entries are'} left out.` : '';
    if (!confirm(`Resend ${n} message${n === 1 ? '' : 's'} to ${target.name} (${target.host}:${target.port}), oldest first, ${how}?${left}`)) return false;
    let allowInvalid = false;
    if (run.unparsed) {
        if (!confirm(`${run.unparsed} of them did not parse. Send them exactly as they were received anyway?`)) return false;
        allowInvalid = true;
    }
    return startRun({
        target,
        items: run.send.map((m) => ({ message_id: m.id })),
        set: changes ? way.set : null,
        allowInvalid,
        what: `Resending ${n} message${n === 1 ? '' : 's'} → ${target.name}`,
    });
}

function runUnderWay() {
    return !!(state.run && state.run.status && state.run.status.state === 'running');
}

// Start a run of `items` (each `repeat` times) to `target`. The rows come
// over the WebSocket; the bar under the list follows the run's status.
// `onEnd(status)` is called once it has ended. A request that turns out to
// be one message is answered like a resend.
async function startRun({ target, items, repeat = 1, set = null, allowInvalid = false, what, onEnd = null }) {
    if (!target || !items.length || state.sending) return false;
    if (runUnderWay()) {
        showToast('A run is under way — wait for it or cancel it first');
        return false;
    }
    if (target.ask) {
        showToast(`${target.name} asks before every message — it takes no runs`);
        return false;
    }
    state.sending = true;
    updateSendButtons();
    updateRunButton();
    try {
        const body = { target: targetRequest(target), items, allow_invalid: allowInvalid };
        if (repeat > 1) body.repeat = repeat;
        if (set) body.set = set;
        const resp = await fetch('/api/send', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(body),
        });
        if (!resp.ok) {
            showToast(`Not sent: ${(await resp.text()) || `HTTP ${resp.status}`}`);
            return false;
        }
        const result = await resp.json();
        state.lastTargetId = target.id;
        writeStored(LAST_TARGET_KEY, target.id);
        if (resp.status !== 202) {
            // One message after all: its outcome, as a resend reports it.
            recordTargetResult(target.id, result.delivery);
            const chip = deliveryChip(result.delivery);
            showToast(`Sent to ${target.name} · ${chip.text}`, result.delivery.state === 'failed' ? 'error' : 'info');
            if (onEnd) onEnd(null, result);
            return true;
        }
        if (state.run && state.run.timer) clearTimeout(state.run.timer);
        state.run = { id: result.id, what, targetId: target.id, status: result, onEnd, cancelling: false, timer: null };
        renderRunBar();
        state.run.timer = setTimeout(pollRun, RUN_POLL_MS);
        return true;
    } catch (e) {
        showToast('Not sent: ' + (e.message || e));
        return false;
    } finally {
        state.sending = false;
        updateSendButtons();
        updateRunButton();
    }
}

async function pollRun() {
    const run = state.run;
    if (!run) return;
    run.timer = null;
    try {
        const resp = await fetch(`/api/send/runs/${encodeURIComponent(run.id)}`);
        if (resp.ok) run.status = await resp.json();
        else if (resp.status === 404 || resp.status === 403) {
            // A restarted server (or sending switched off) knows no run.
            run.status = { ...run.status, state: 'lost', reason: 'The server no longer knows this run' };
        }
    } catch (e) {
        console.debug('Could not read the run:', e); // asked again below
    }
    if (state.run !== run) return;
    renderRunBar();
    if (run.status.state === 'running') run.timer = setTimeout(pollRun, RUN_POLL_MS);
    else finishRun(run);
}

function finishRun(run) {
    const s = run.status;
    const end = RUN_END_TEXT[s.state] || s.state;
    const summary = runSummaryParts(s).map((p) => p.text).join(' · ');
    const ok = s.state === 'done' && !s.failed && runAllAccepted(s);
    const why = s.reason && !(s.state === 'done' && !s.failed) ? ` — ${s.reason}` : '';
    showToast(`${run.what}: ${end} · ${s.done} / ${s.total}${summary ? ' · ' + summary : ''}${why}`, ok ? 'info' : 'error');
    updateRunButton();
    if (run.onEnd) {
        const onEnd = run.onEnd;
        run.onEnd = null;
        onEnd(s, null);
    }
}

function runAllAccepted(s) {
    return Object.keys(s.codes || {}).every((c) => c === 'AA' || c === 'CA');
}

async function cancelRun() {
    const run = state.run;
    if (!runUnderWay() || run.cancelling) return;
    run.cancelling = true;
    renderRunBar();
    try {
        const resp = await fetch(`/api/send/runs/${encodeURIComponent(run.id)}/cancel`, { method: 'POST' });
        if (resp.ok && state.run === run) run.status = await resp.json();
    } catch (e) {
        console.debug('Could not cancel the run:', e);
        run.cancelling = false;
    }
    if (state.run === run) renderRunBar();
}

function closeRunBar() {
    if (runUnderWay()) return;
    if (state.run && state.run.timer) clearTimeout(state.run.timer);
    state.run = null;
    renderRunBar();
}

// `24 AA`, `2 AE`, `1 failed`, `avg 38 ms` — the bar's counts.
function runSummaryParts(s) {
    const parts = [];
    for (const [code, n] of Object.entries(s.codes || {})) {
        parts.push({ text: `${n} ${code}`, cls: code === 'AA' || code === 'CA' ? 'ok' : 'bad' });
    }
    if (s.written) parts.push({ text: `${s.written} written`, cls: 'ok' });
    if (s.failed) parts.push({ text: `${s.failed} failed`, cls: 'bad' });
    if (s.avg_latency_ms != null) parts.push({ text: `avg ${formatLatency(s.avg_latency_ms)}`, cls: '' });
    return parts;
}

function runBarHtml(run) {
    const s = run.status;
    const running = s.state === 'running';
    const parts = runSummaryParts(s)
        .map((p) => `<span class="mono${p.cls ? ' ' + p.cls : ''}">${esc(p.text)}</span>`).join('');
    const clean = s.state === 'done' && !s.failed;
    const end = running ? ''
        : `<span class="mono end ${clean ? 'ok' : 'bad'}">${esc(RUN_END_TEXT[s.state] || s.state)}</span>`;
    // Why it stopped early — or, for a run that tried every message, why
    // some failed (the last failure; #293 review).
    const reason = !running && s.reason && !clean
        ? `<span class="reason" title="${escAttr(s.reason)}">${esc(s.state === 'done' ? `last failure: ${s.reason}` : s.reason)}</span>` : '';
    const button = running
        ? `<button class="ctl-btn" data-action="run-cancel"${run.cancelling ? ' aria-disabled="true"' : ''} title="Stop before the next message; the one in flight ends as it would">${run.cancelling ? 'Cancelling…' : 'Cancel'}</button>`
        : '<button class="ctl-btn" data-action="run-close" title="Close" aria-label="Close the run bar">✕</button>';
    return `<span class="mono what" title="${escAttr(run.what)}">${esc(run.what)}</span>`
        + `<span class="track" role="progressbar" aria-label="Progress" aria-valuemin="0" aria-valuemax="${s.total}" aria-valuenow="${s.done}"><i></i></span>`
        + `<span class="mono">${s.done} / ${s.total}</span>${parts}${end}${reason}<span class="spacer"></span>${button}`;
}

function renderRunBar() {
    const bar = document.getElementById('run-bar');
    if (!bar) return;
    const run = state.run;
    if (!run) {
        bar.classList.add('hidden');
        bar.replaceChildren();
        return;
    }
    // Keep focus on the button while it changes from Cancel to ✕.
    const hadFocus = bar.contains && document.activeElement && bar.contains(document.activeElement);
    bar.innerHTML = runBarHtml(run);
    bar.classList.remove('hidden');
    const s = run.status;
    const track = bar.querySelector('.track');
    const fill = track && track.firstElementChild;
    // Set through the CSSOM: the CSP forbids style attributes.
    if (fill && fill.style) fill.style.width = `${s.total ? Math.round((100 * s.done) / s.total) : 0}%`;
    if (hadFocus) {
        const btn = bar.querySelector('button');
        if (btn && btn.focus) btn.focus();
    }
}

// The list's *Resend filtered* button: off while a run is under way.
function updateRunButton() {
    const btn = document.getElementById('btn-resend-filtered');
    if (!btn) return;
    btn.classList.toggle('hidden', !state.sendConfig);
    btn.setAttribute('aria-expanded', state.sendMenu && state.sendMenu.forRun ? 'true' : 'false');
    if (runUnderWay() || state.sending) btn.setAttribute('aria-disabled', 'true');
    else btn.removeAttribute('aria-disabled');
}

// --- Target list as a file (to keep it, or to pass it to a colleague) ---

function saveTargetsToFile() {
    const blob = new Blob([JSON.stringify({ harux_targets: 1, targets: state.targets }, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = 'harux-targets.json';
    a.click();
    URL.revokeObjectURL(url);
}

function loadTargetsFromFile() {
    const input = document.createElement('input');
    input.type = 'file';
    input.accept = '.json,application/json';
    input.addEventListener('change', async () => {
        const file = input.files && input.files[0];
        if (!file) return;
        try {
            const targets = parseTargetFile(await file.text());
            if (state.targets.length > 0 && !confirm(`Replace your ${state.targets.length} target(s) with the ${targets.length} in ${file.name}?`)) return;
            // The file replaces the list: the targets it had are removed.
            for (const t of state.targets) state.targetsRemoved.add(t.id);
            state.targets = targets;
            saveTargets();
            pruneTargetResults();
            if (state.sendMenu) {
                state.sendMenu.pickedId = (allTargets()[0] || {}).id || null;
                renderSendMenu();
            }
            updateSendButtons();
            showToast(`Loaded ${targets.length} target(s) from ${file.name}`, 'info');
        } catch (e) {
            showToast(e.message || String(e));
        }
    });
    input.click();
}

// --- Delegation (called from app.js) ---

// Handles the send-related `data-action`s; returns true if it took one.
function handleSendAction(action, el) {
    switch (action) {
        case 'resend': resendOneClick(); return true;
        case 'resend-menu': toggleSendMenu(); return true;
        case 'resend-filtered': toggleRunMenu(); return true;
        case 'run-cancel': cancelRun(); return true;
        case 'run-close': closeRunBar(); return true;
        case 'target-pick': pickTarget(el.dataset.targetId); return true;
        case 'target-edit': editTarget(el.dataset.targetId); return true;
        case 'target-add':
            if (state.sendMenu) { state.sendMenu.form = emptyTargetForm(); renderSendMenu(); }
            return true;
        case 'target-flag': toggleTargetFlag(el.dataset.flag); return true;
        case 'target-save': saveTargetForm(); return true;
        case 'target-cancel':
            if (state.sendMenu) { state.sendMenu.form = null; renderSendMenu(); }
            return true;
        case 'target-delete': deleteEditedTarget(); return true;
        case 'send-picked': sendPicked(); return true;
        case 'send-as': chooseSendAs(el.dataset.value); return true;
        case 'targets-save-file': saveTargetsToFile(); return true;
        case 'targets-load-file': loadTargetsFromFile(); return true;
        default: return false;
    }
}
