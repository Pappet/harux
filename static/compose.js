// Drafts in the UI (#277, Milestone 4 stage 2): the drafts rail, a draft in
// the detail panel (header with target and Send, Segments preview, Raw
// editor, Response history, Diff vs pinned), *Edit* on a stored message and
// *New message* (blank, clipboard, file).
//
// A draft is text kept in this browser tab; everything that needs to
// understand HL7 is done by the server (src/draft.rs, #276): the preview
// (`POST /api/parse`), the bytes (`POST /api/build`), a file's encoding, and
// the send (`POST /api/send` with a draft item). A long value of the message
// a draft was made from stays on the server as a reference `{{from:…}}`.
// The sent message comes back as a row over the WebSocket only — never added
// here from the answer.
//
// Loaded after send.js and before app.js; uses `state`, util.js, render.js
// (renderParsedTab, renderRawLinesView, resetDetailHeader, updateRowSelection,
// textSpan), diff.js (renderDiffTab) and send.js (findTarget, targetRequest,
// recordTargetResult, openSendMenu).

const DRAFTS_SESSION_KEY = 'harux_drafts';
const DRAFTS_KEPT_KEY = 'harux_drafts_kept';
const DRAFT_PREVIEW_DELAY_MS = 300; // the search's debounce
const MAX_DRAFTS = 50;
const MAX_DRAFT_HISTORY = 20;

// What *New message → Blank* starts with: a header whose control ID and time
// are filled in for every send.
const BLANK_DRAFT = 'MSH|^~\\&|HARUX|HARUX|||{{now}}||ADT^A01|{{id}}|P|2.5\nEVN|A01|{{now}}\nPID|1||\nPV1|1|';

// Encodings a user can pick in the meta row; `auto` follows MSH-18.
const DRAFT_ENCODINGS = [
    ['auto', 'MSH-18 (else UTF-8)'],
    ['UTF-8', 'UTF-8'],
    ['windows-1252', 'Latin-1 (Windows-1252)'],
    ['ISO-8859-15', 'Latin-9 (ISO-8859-15)'],
];

// --- Storage (never fatal: a tab that refuses storage keeps its drafts in
// memory until it is closed, and says so once) ---

function readDraftStore(storage, key) {
    try {
        const raw = storage.getItem(key);
        const list = raw ? JSON.parse(raw) : [];
        return Array.isArray(list) ? list : [];
    } catch (e) {
        console.debug(`Could not read ${key}:`, e);
        return [];
    }
}

function writeDraftStore(storage, key, list) {
    try {
        storage.setItem(key, JSON.stringify(list));
        return true;
    } catch (e) {
        console.debug(`Could not save ${key}:`, e);
        if (!state.draftStorageWarned) {
            state.draftStorageWarned = true;
            showToast('This browser does not keep your drafts — they last until the page is closed. "Save as file" keeps one.');
        }
        return false;
    }
}

// *Repeat* (#280): how many copies a send makes, 1 … the run limit.
function draftRepeatValue(v) {
    const max = state.sendConfig && state.sendConfig.max_run_messages ? state.sendConfig.max_run_messages : 1000;
    const n = Math.floor(Number(v));
    return Number.isFinite(n) ? Math.min(Math.max(n, 1), max) : 1;
}

function setDraftRepeat(value) {
    const d = openDraftObject();
    if (!d) return;
    d.repeat = draftRepeatValue(value);
    saveDrafts();
    renderDraftHeader();
}

// A draft as stored, or null if `raw` is not one.
function normalizeDraft(raw) {
    if (!raw || typeof raw !== 'object' || typeof raw.raw !== 'string') return null;
    return {
        id: typeof raw.id === 'string' && raw.id ? raw.id : newId(),
        raw: raw.raw,
        charset: typeof raw.charset === 'string' && raw.charset ? raw.charset : 'auto',
        basedOn: typeof raw.basedOn === 'string' && raw.basedOn ? raw.basedOn : null,
        references: Array.isArray(raw.references) ? raw.references : [],
        origin: raw.origin && typeof raw.origin === 'object' ? raw.origin : { kind: 'new' },
        type: typeof raw.type === 'string' ? raw.type : '',
        title: typeof raw.title === 'string' ? raw.title : '',
        targetId: typeof raw.targetId === 'string' ? raw.targetId : null,
        sends: Array.isArray(raw.sends) ? raw.sends.slice(-MAX_DRAFT_HISTORY) : [],
        sentRaw: typeof raw.sentRaw === 'string' ? raw.sentRaw : null,
        keep: raw.keep === true,
        repeat: draftRepeatValue(raw.repeat),
        originGone: raw.originGone === true,
        created: typeof raw.created === 'number' ? raw.created : Date.now(),
    };
}

function loadDrafts() {
    const kept = readDraftStore(localStorage, DRAFTS_KEPT_KEY).map((d) => ({ ...d, keep: true }));
    const tab = readDraftStore(sessionStorage, DRAFTS_SESSION_KEY);
    const seen = new Set();
    // No cut here (#290 review): a draft the user kept must never fall off
    // the list and be overwritten by the next save. The limit applies when
    // a draft is added.
    // In the order they were made, whichever store holds them.
    state.drafts = [...kept, ...tab].map(normalizeDraft).filter((d) => {
        if (!d || seen.has(d.id)) return false;
        seen.add(d.id);
        return true;
    }).sort((x, y) => x.created - y.created);
    if (state.selectedDraftId && !findDraft(state.selectedDraftId)) state.selection = null;
}

// Drafts made from real messages hold patient data: by default they stay in
// this tab (sessionStorage) and reach the disk only when the user keeps one
// (plan section 6).
function saveDrafts() {
    const okTab = writeDraftStore(sessionStorage, DRAFTS_SESSION_KEY, state.drafts.filter((d) => !d.keep));
    const okKept = writeDraftStore(localStorage, DRAFTS_KEPT_KEY, state.drafts.filter((d) => d.keep));
    return okTab && okKept;
}

function findDraft(id) {
    return state.drafts.find((d) => d.id === id) || null;
}

function openDraftObject() {
    return state.selectedDraftId ? findDraft(state.selectedDraftId) : null;
}

function isDraftOpen() {
    return !!openDraftObject();
}

// Changed since the last send (or never sent): the rail's dot.
function draftHasUnsentChanges(d) {
    return d.sentRaw === null || d.sentRaw !== d.raw;
}

// --- Making drafts ---

// Line ends as a <textarea> keeps them (LF), so offsets match the preview.
function textareaText(text) {
    return String(text).replace(/\r\n?/g, '\n');
}

function addDraft(fields) {
    if (state.drafts.length >= MAX_DRAFTS) {
        showToast(`At most ${MAX_DRAFTS} drafts — delete one first`);
        return null;
    }
    // Strictly increasing, so the order survives a reload (loadDrafts sorts).
    const created = Math.max(Date.now(), ...state.drafts.map((d) => d.created + 1));
    const draft = normalizeDraft({ ...fields, raw: textareaText(fields.raw || ''), id: newId(), created });
    state.drafts.push(draft);
    saveDrafts();
    return draft;
}

// *Edit* on a stored message: the server gives its text with long values as
// references (#276) and the encoding it arrived in.
async function editMessage(id) {
    const msg = state.selectedMessage && state.selectedMessage.id === id ? state.selectedMessage : null;
    let data;
    try {
        const resp = await fetch(`/api/messages/${encodeURIComponent(id)}/draft`);
        if (!resp.ok) {
            showToast(`Cannot edit: ${(await resp.text()) || `HTTP ${resp.status}`}`);
            return null;
        }
        data = await resp.json();
    } catch (e) {
        showToast('Cannot edit: ' + (e.message || e));
        return null;
    }
    const draft = addDraft({
        raw: data.raw,
        charset: data.encoding || 'auto',
        basedOn: data.based_on || id,
        references: data.references || [],
        origin: {
            kind: 'edit',
            id: data.based_on || id,
            control: msg ? msg.message_control_id || '' : '',
        },
        type: msg ? msg.message_type || '' : '',
        title: msg ? msg.patient_name || msg.patient_id || '' : '',
    });
    if (draft) openDraft(draft.id, 'raw');
    return draft;
}

// The stored messages a draft was made from are gone (#290 review): evicted
// (`ids`) or cleared (`null`, all of them). The draft says so at once —
// its references can no longer be filled, so a send would be refused.
function markDraftOriginsGone(ids) {
    const gone = ids ? new Set(ids) : null;
    let changed = false;
    for (const d of state.drafts) {
        if (d.basedOn && !d.originGone && (!gone || gone.has(d.basedOn))) {
            d.originGone = true;
            changed = true;
        }
    }
    if (!changed) return;
    saveDrafts();
    renderDraftRail();
    if (isDraftOpen()) renderDraftHeader();
}

function newBlankDraft() {
    const draft = addDraft({ raw: BLANK_DRAFT, origin: { kind: 'new' }, type: 'ADT^A01' });
    if (draft) openDraft(draft.id, 'raw');
    return draft;
}

// From the clipboard: needs a secure context (HTTPS or localhost). On plain
// HTTP, the usual deployment, an empty draft opens for pasting instead.
async function newDraftFromClipboard() {
    let text = null;
    try {
        if (navigator.clipboard && navigator.clipboard.readText) text = await navigator.clipboard.readText();
    } catch (e) {
        console.debug('Could not read the clipboard:', e);
    }
    if (text === null) {
        showToast('The browser does not let this page read the clipboard here — paste with Ctrl+V into the Raw tab', 'info');
        text = '';
    }
    const draft = addDraft({ raw: text, origin: { kind: 'clipboard' } });
    if (draft) openDraft(draft.id, 'raw');
    return draft;
}

// From a file: its bytes go to the server, which decodes them like a
// received frame (a browser reads text as UTF-8 only and would damage a
// Latin-1 file).
async function draftFromFileBytes(name, bytes) {
    let data;
    try {
        const resp = await fetch('/api/parse', {
            method: 'POST',
            headers: { 'Content-Type': 'application/octet-stream' },
            body: bytes,
        });
        if (!resp.ok) {
            showToast(`Cannot read ${name}: ${(await resp.text()) || `HTTP ${resp.status}`}`);
            return null;
        }
        data = await resp.json();
    } catch (e) {
        showToast(`Cannot read ${name}: ` + (e.message || e));
        return null;
    }
    if (data.warning) showToast(`${name}: ${data.warning}`, 'info');
    const draft = addDraft({ raw: data.raw, charset: data.encoding || 'auto', origin: { kind: 'file', name: String(name).slice(0, 120) } });
    if (draft) openDraft(draft.id, 'raw');
    return draft;
}

function newDraftFromFile() {
    const input = document.createElement('input');
    input.type = 'file';
    input.accept = '.hl7,.txt,.msg,text/plain,application/octet-stream';
    input.addEventListener('change', async () => {
        const file = input.files && input.files[0];
        if (file) await draftFromFileBytes(file.name, await file.arrayBuffer());
    });
    input.click();
}

function deleteDraft(id) {
    const d = findDraft(id);
    if (!d) return false;
    if (d.raw.trim() && draftHasUnsentChanges(d) && !confirm('Delete this draft? Its changes were not sent.')) return false;
    state.drafts = state.drafts.filter((x) => x.id !== id);
    saveDrafts();
    if (state.selectedDraftId === id) closeDraftView();
    renderDraftRail();
    return true;
}

function toggleKeepDraft(id) {
    const d = findDraft(id);
    if (!d) return;
    d.keep = !d.keep;
    saveDrafts();
    renderDraftHeader();
    showToast(d.keep ? 'Kept on this computer (localStorage) until you delete it' : 'Kept in this tab only', 'info');
}

// --- Opening and closing ---

function openDraft(id, tab) {
    const d = findDraft(id);
    if (!d) return;
    const prevMessageId = state.selectedId;
    if (typeof closeSendMenu === 'function') closeSendMenu();
    state.selection = { kind: 'draft', id };
    state.selectedMessage = null;
    state.selectRequestSeq++; // a message fetch in flight must not take the panel back
    if (prevMessageId) updateRowSelection(prevMessageId, null);
    if (tab) state.activeTab = tab;
    if (!['parsed', 'raw', 'ack', 'diff'].includes(state.activeTab)) state.activeTab = 'raw';
    syncTabButtons();
    if (state.draftSendAnyway !== id) state.draftSendAnyway = null;
    renderDraftRail();
    renderDraftDetail();
    requestDraftPreview(d, true);
    saveSession();
}

// Leave the draft view (the draft stays in the rail).
function closeDraftView() {
    if (!state.selectedDraftId) return;
    state.selection = null;
    state.draftPreview = null;
    clearTimeout(state.draftPreviewTimer);
    resetDetailHeader();
    document.getElementById('detail-content').innerHTML = DETAIL_EMPTY_HTML;
    renderDraftRail();
    saveSession();
}

function syncTabButtons() {
    document.querySelectorAll('.detail-tab').forEach((t) => {
        t.classList.toggle('active', t.dataset.tab === state.activeTab);
    });
}

// --- Preview (POST /api/parse) ---

function draftNeedsFilledPreview(d) {
    return state.activeTab === 'diff' && !!d.basedOn && d.references.length > 0;
}

// The preview of the open draft, if it describes its current text.
function currentPreview(d) {
    const p = state.draftPreview;
    if (!p || !d || p.draftId !== d.id || p.raw !== d.raw || p.error) return null;
    return p;
}

function scheduleDraftPreview(d) {
    clearTimeout(state.draftPreviewTimer);
    state.draftPreviewTimer = setTimeout(() => requestDraftPreview(d), DRAFT_PREVIEW_DELAY_MS);
}

async function requestDraftPreview(d, now = false) {
    if (!d) return;
    if (!now) clearTimeout(state.draftPreviewTimer);
    const fill = draftNeedsFilledPreview(d);
    const seq = ++state.draftPreviewSeq;
    const raw = d.raw;
    let entry;
    try {
        const resp = await fetch('/api/parse', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ raw, based_on: d.basedOn, fill_references: fill }),
        });
        entry = resp.ok
            ? { draftId: d.id, raw, filled: fill, data: await resp.json(), error: null }
            : { draftId: d.id, raw, filled: fill, data: null, error: (await resp.text()) || `HTTP ${resp.status}` };
    } catch (e) {
        entry = { draftId: d.id, raw, filled: fill, data: null, error: e.message || String(e) };
    }
    // Last request wins, and only for the draft still open.
    if (seq !== state.draftPreviewSeq || state.selectedDraftId !== d.id) return;
    state.draftPreview = entry;
    if (entry.data && entry.data.message && !fill) {
        const m = entry.data.message;
        const type = m.parse_error ? '' : m.message_type || '';
        const title = m.patient_name || m.patient_id || '';
        if (type !== d.type || title !== d.title) {
            d.type = type;
            d.title = title;
            saveDrafts();
            renderDraftRail();
        }
    }
    renderDraftHeader();
    if (state.activeTab === 'raw') updateDraftStatus();
    else renderDraftTab(document.getElementById('detail-content'));
}

// --- Where the cursor is (the Raw tab's status line) ---

// The segment line and field at `offset` (UTF-16 units, as a <textarea>
// counts) in the preview's positions: `{ seg, name, field }`, field null on
// the segment name, or null outside every segment. A cursor right after a
// field's last character belongs to that field.
function fieldAt(positions, offset) {
    if (!Array.isArray(positions)) return null;
    for (let seg = 0; seg < positions.length; seg++) {
        const p = positions[seg];
        if (offset < p.start || offset > p.end) continue;
        for (const [field, a, b] of p.fields) {
            if (offset >= a && offset <= b) return { seg, name: p.name, field };
        }
        return { seg, name: p.name, field: null };
    }
    return null;
}

// "PID-5 · Patient Name · XPN" for the cursor at `offset`.
function draftStatusText(preview, offset) {
    if (!preview || !preview.data) return '';
    const at = fieldAt(preview.data.positions, offset);
    if (!at) return '';
    if (at.field === null) return at.name;
    const label = `${at.name}-${at.field}`;
    const seg = preview.data.message && preview.data.message.segments && preview.data.message.segments[at.seg];
    const f = seg && seg.fields ? seg.fields.find((x) => x.index === at.field) : null;
    return [label, f && f.description, f && f.datatype].filter(Boolean).join(' · ');
}

function updateDraftStatus() {
    const el = document.getElementById('draft-status');
    const ta = document.getElementById('draft-text');
    const d = openDraftObject();
    if (!el || !ta || !d) return;
    // A preview with the references filled in (for the diff) has offsets
    // into another text (#290 review): it never names the cursor's field.
    const p = currentPreview(d);
    el.textContent = p && !p.filled ? draftStatusText(p, ta.selectionStart) || ' ' : 'Reading the draft…';
}

// --- Typing ---

function onDraftInput(ta) {
    const d = openDraftObject();
    if (!d) return;
    d.raw = ta.value;
    if (state.draftSendAnyway === d.id) state.draftSendAnyway = null;
    scheduleDraftPreview(d);
    saveDraftsSoon();
    renderDraftRail();
    updateDraftStatus();
}

let draftSaveTimer = null;
function saveDraftsSoon() {
    clearTimeout(draftSaveTimer);
    draftSaveTimer = setTimeout(saveDrafts, DRAFT_PREVIEW_DELAY_MS);
}

function setDraftCharset(value) {
    const d = openDraftObject();
    if (!d || (!DRAFT_ENCODINGS.some(([v]) => v === value) && value !== d.charset)) return;
    d.charset = value;
    saveDrafts();
    renderDraftHeader();
}

// --- Sending ---

function draftTarget(d) {
    return findTarget(d.targetId) || findTarget(state.lastTargetId) || null;
}

// Send `d` to `target`. A draft that does not parse is armed by the first
// click ("Send anyway") and sent by the second, with `allow_invalid`.
async function sendDraft(d, target) {
    if (!d || state.sending) return false;
    await draftWriteChain; // field writes first (#278)
    if (!target) {
        openSendMenu();
        return false;
    }
    const p = currentPreview(d);
    const broken = !!(p && p.data && p.data.message && p.data.message.parse_error);
    if (broken && state.draftSendAnyway !== d.id) {
        state.draftSendAnyway = d.id;
        renderDraftHeader();
        showToast('This draft does not parse. "Send anyway" sends it as it is — it is stored with its parse error.', 'info');
        return false;
    }
    const raw = d.raw;
    const item = { raw, charset: d.charset };
    if (d.basedOn) item.based_on = d.basedOn;
    // The sent message names the template it came from (#279).
    else if (d.origin && d.origin.kind === 'template' && d.origin.name) item.template = d.origin.name;
    if (d.repeat > 1) return repeatDraft(d, target, item, broken);
    if (target.ask && !confirm(`Send this draft to ${target.name} (${target.host}:${target.port})?`)) return false;
    state.sending = true;
    renderDraftHeader();
    try {
        const resp = await fetch('/api/send', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ target: targetRequest(target), items: [item], allow_invalid: broken }),
        });
        if (!resp.ok) {
            showToast(`Not sent: ${(await resp.text()) || `HTTP ${resp.status}`}`);
            return false;
        }
        const result = await resp.json();
        const dl = result.delivery || {};
        d.sends.push({
            id: result.id,
            at: Date.now(),
            target: target.name,
            state: dl.state || '',
            code: dl.ack_code || null,
            error: dl.error || null,
            ms: typeof dl.latency_ms === 'number' ? dl.latency_ms : null,
        });
        d.sends = d.sends.slice(-MAX_DRAFT_HISTORY);
        d.sentRaw = raw;
        d.targetId = target.id;
        // The next draft starts with this target too, as a resend does
        // (#290 review).
        state.lastTargetId = target.id;
        writeStored(LAST_TARGET_KEY, target.id);
        state.draftSendAnyway = null;
        saveDrafts();
        recordTargetResult(target.id, dl);
        const chip = deliveryChip(dl);
        const latency = formatLatency(dl.latency_ms);
        showToast(`Sent to ${target.name} · ${chip.text}${latency ? ' · ' + latency : ''}`, dl.state === 'failed' ? 'error' : 'info');
        return true;
    } catch (e) {
        showToast('Not sent: ' + (e.message || e));
        return false;
    } finally {
        state.sending = false;
        if (state.selectedDraftId === d.id) {
            renderDraftHeader();
            if (state.activeTab === 'ack') renderDraftTab(document.getElementById('detail-content'));
        }
        renderDraftRail();
    }
}

// *Repeat* (#280): the draft `d.repeat` times in one run, after a
// confirmation naming target and number. A target that asks before every
// message takes no runs (decision D3). The run is noted in the draft's
// history when it has ended.
async function repeatDraft(d, target, item, broken) {
    const n = d.repeat;
    if (target.ask) {
        showToast(`${target.name} asks before every message — it takes no runs. Set Repeat to 1 or pick another target.`);
        return false;
    }
    if (!confirm(`Send this draft ${n} times to ${target.name} (${target.host}:${target.port}), over one connection?`)) return false;
    const raw = d.raw;
    const started = await startRun({
        target,
        items: [item],
        repeat: n,
        allowInvalid: broken,
        what: `Sending ${d.type || 'the draft'} ${n}× → ${target.name}`,
        onEnd: (status) => {
            if (!status) return;
            d.sends.push({
                id: null,
                run: status.id,
                at: Date.now(),
                target: target.name,
                state: status.state,
                done: status.done,
                total: status.total,
                ok: status.state === 'done' && !status.failed && runAllAccepted(status),
                summary: runSummaryParts(status).map((p) => p.text).join(' · '),
            });
            d.sends = d.sends.slice(-MAX_DRAFT_HISTORY);
            saveDrafts();
            if (state.selectedDraftId === d.id) {
                renderDraftHeader();
                if (state.activeTab === 'ack') renderDraftTab(document.getElementById('detail-content'));
            }
        },
    });
    if (!started) return false;
    d.sentRaw = raw;
    d.targetId = target.id;
    state.draftSendAnyway = null;
    saveDrafts();
    if (state.selectedDraftId === d.id) renderDraftHeader();
    renderDraftRail();
    return true;
}

async function sendOpenDraft() {
    const d = openDraftObject();
    if (!d) return false;
    await commitFocusedField();
    return sendDraft(d, draftTarget(d));
}

// A field input whose value was typed but not yet written (no `change`
// yet: Ctrl+Enter while still in it) is written first (#278).
function commitFocusedField() {
    const a = document.activeElement;
    if (a && a.classList && a.classList.contains('field-input') && a.value !== a.dataset.written) {
        return writeDraftField(a.dataset.fieldPath, a.value);
    }
    return draftWriteChain;
}

// *Save as file*: the bytes a send would write, in the draft's encoding,
// built by the server.
async function saveDraftAsFile() {
    const d = openDraftObject();
    if (!d) return;
    await commitFocusedField();
    try {
        const body = { raw: d.raw, charset: d.charset };
        if (d.basedOn) body.based_on = d.basedOn;
        const resp = await fetch('/api/build', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(body),
        });
        if (!resp.ok) {
            showToast(`Cannot save: ${(await resp.text()) || `HTTP ${resp.status}`}`);
            return;
        }
        const url = URL.createObjectURL(await resp.blob());
        const a = document.createElement('a');
        a.href = url;
        a.download = `${(d.type || 'draft').replace(/[^A-Za-z0-9_-]+/g, '_')}-draft.hl7`;
        a.click();
        URL.revokeObjectURL(url);
    } catch (e) {
        showToast('Cannot save: ' + (e.message || e));
    }
}

// --- Rendering ---

function renderDraftRail() {
    const rail = document.getElementById('draft-rail');
    if (!rail) return;
    const show = !!state.sendConfig && state.drafts.length > 0;
    rail.classList.toggle('hidden', !show);
    if (!show) {
        rail.innerHTML = '';
        return;
    }
    const chips = state.drafts.map((d) => {
        const active = d.id === state.selectedDraftId ? ' active' : '';
        const title = d.title || (d.origin.kind === 'file' ? d.origin.name : '') || 'Draft';
        const dot = draftHasUnsentChanges(d) ? '<span class="unsent" title="Changed since the last send"></span>' : '';
        const gone = d.originGone && d.references.length ? '<span class="gone" title="The message it was made from is no longer stored">\u26a0</span>' : '';
        return `<span class="draft-chip${active}" role="button" tabindex="0" data-action="draft-open" data-draft-id="${escAttr(d.id)}" title="Open the draft">`
            + `${d.type ? `<span class="t">${esc(d.type)}</span>` : ''}<span class="n">${esc(title)}</span>${gone}${dot}`
            + `<button class="x" data-action="draft-delete" data-draft-id="${escAttr(d.id)}" title="Delete the draft" aria-label="Delete the draft">✕</button></span>`;
    }).join('');
    rail.innerHTML = `<span class="label">Drafts</span><div class="draft-rail-chips">${chips}</div>`;
}

function draftOriginHtml(d) {
    const o = d.origin || {};
    if (o.kind === 'edit' && o.id) {
        if (d.originGone) {
            const refs = d.references.length > 0
                ? ' — its references cannot be filled in, so a send is refused; delete the draft or replace them'
                : '';
            return `Edit of ${esc(o.control || 'a message')} <span class="origin-gone">(no longer stored${esc(refs)})</span>`;
        }
        return `Edit of <button class="origin-link" data-action="select-message" data-id="${escAttr(o.id)}" title="Open the message this draft was made from">${esc(o.control || 'the original')}</button>`;
    }
    if (o.kind === 'file') return `From the file ${esc(o.name || '')}`;
    if (o.kind === 'template') return `From the template ${esc(o.name || '')}`;
    if (o.kind === 'clipboard') return 'From the clipboard';
    return 'New message';
}

function draftSendStateText(d) {
    if (d.sends.length === 0) return 'not sent yet';
    const n = d.sends.length;
    return draftHasUnsentChanges(d) ? `changed since the last send · sent ${n}×` : `sent ${n}×`;
}

function draftMetaHtml(d) {
    const items = [];
    const target = draftTarget(d);
    const item = (k, v) => `<span class="item"><span class="k">${esc(k)}</span><span class="v">${v}</span></span>`;
    if (target) {
        const how = target.tls ? (target.verify ? ' (TLS)' : ' (TLS, not verified)') : '';
        items.push(item('to', esc(`${target.host}:${target.port}${how}`)));
    }
    const p = currentPreview(d);
    const declared = p && p.data && p.data.message ? p.data.message.charset : null;
    const options = DRAFT_ENCODINGS.map(([v, label]) => {
        const text = v === 'auto' && declared ? `MSH-18 (${declared})` : label;
        return `<option value="${escAttr(v)}"${v === d.charset ? ' selected' : ''}>${esc(text)}</option>`;
    });
    if (!DRAFT_ENCODINGS.some(([v]) => v === d.charset)) {
        options.unshift(`<option value="${escAttr(d.charset)}" selected>${esc(d.charset)}</option>`);
    }
    items.push(item('encoding', `<select class="draft-charset" id="draft-charset" aria-label="Encoding">${options.join('')}</select>`));
    if (p && p.data && p.data.message && !p.data.message.parse_error) {
        items.push(item('segments', esc(String((p.data.message.segments || []).length))));
    }
    const refBytes = d.references.reduce((n, r) => n + (r.bytes || 0), 0);
    items.push(item('size', esc(formatSize(utf8Length(d.raw) + refBytes))));
    if (d.references.length) {
        items.push(item('references', esc(`${d.references.length} (${formatSize(refBytes)} from the original)`)));
    }
    return items.join('<span class="sep">·</span>');
}

function draftActionsHtml(d) {
    const target = draftTarget(d);
    const busy = state.sending ? ' aria-disabled="true"' : '';
    const open = state.sendMenu && !state.sendMenu.forRun ? 'true' : 'false';
    const anyway = state.draftSendAnyway === d.id;
    const to = target ? esc(target.name) : 'choose a target';
    return `<button class="ctl-btn target-btn" data-action="resend-menu" aria-haspopup="dialog" aria-expanded="${open}" title="Choose the target">`
        + `<span class="k">to</span> <span class="to">${to}</span>${ICONS.chevronDown}</button>`
        + `<button class="ctl-btn on send-draft${anyway ? ' anyway' : ''}" data-action="draft-send"${busy} title="Send the draft (Ctrl+Enter)">`
        + `${ICONS.resend}<span class="btn-label">${anyway ? 'Send anyway' : (d.repeat > 1 ? `Send ${d.repeat}×` : 'Send')}</span><kbd class="key">Ctrl ⏎</kbd></button>`;
}

function draftToolsHtml(d) {
    const max = state.sendConfig && state.sendConfig.max_run_messages ? state.sendConfig.max_run_messages : 1000;
    // *Repeat* (#280): the draft goes out n times over one connection, its
    // placeholders filled in for every copy ({{n}} counts).
    const repeat = `<label class="repeat-ctl" title="Send the draft this many times in one run; placeholders are filled in for every copy, {{n}} counts">`
        + `<span>Repeat</span><input type="number" id="draft-repeat" class="repeat-input" min="1" max="${max}" step="1" value="${d.repeat || 1}" aria-label="Repeat the draft this many times"><span>×</span></label>`;
    return repeat + `<button class="action-btn${d.keep ? ' active' : ''}" data-action="draft-keep" title="${d.keep ? 'Kept on this computer — click to keep it in this tab only' : 'Keep this draft on this computer (it holds patient data if made from a real message)'}">`
        + `${d.keep ? ICONS.starFilled : ICONS.starOutline}<span class="btn-label">${d.keep ? 'Kept' : 'Keep on this computer'}</span></button>`
        + `<button class="action-btn" data-action="draft-save-file" title="Download the bytes a send would write">${ICONS.download}<span class="btn-label">Save as file</span></button>`
        + `<button class="action-btn" data-action="draft-delete" data-draft-id="${escAttr(d.id)}" title="Delete the draft">${ICONS.trash}<span class="btn-label">Delete</span></button>`;
}

// The header of the open draft: badge, title, target and Send, origin,
// meta row, tools. Tabs: Segments, Raw, Response, Diff vs pinned.
function renderDraftHeader() {
    const d = openDraftObject();
    if (!d) return;
    const dirEl = document.getElementById('detail-direction');
    if (dirEl) {
        dirEl.textContent = 'DRAFT';
        dirEl.classList.add('draft');
        dirEl.classList.remove('hidden');
        dirEl.title = 'A draft: text kept in this browser';
    }
    const typeEl = document.getElementById('detail-type');
    if (typeEl) {
        typeEl.textContent = d.type || '';
        typeEl.classList.toggle('hidden', !d.type);
    }
    const titleEl = document.getElementById('detail-title');
    if (titleEl) titleEl.textContent = d.title || 'Draft';
    const descEl = document.getElementById('detail-desc');
    if (descEl) {
        descEl.innerHTML = `${draftOriginHtml(d)} · ${esc(draftSendStateText(d))}`;
        descEl.classList.remove('hidden');
    }
    const metaEl = document.getElementById('detail-meta');
    if (metaEl) metaEl.innerHTML = draftMetaHtml(d);
    updateDraftActions();
    const toolsEl = document.getElementById('detail-tools');
    if (toolsEl) toolsEl.innerHTML = draftToolsHtml(d);
    const ackTab = document.getElementById('tab-btn-ack');
    if (ackTab) ackTab.textContent = 'Response';
    const jsonTab = document.getElementById('tab-btn-json');
    if (jsonTab) jsonTab.classList.add('hidden');
    const diffTab = document.getElementById('tab-btn-diff');
    if (diffTab) diffTab.classList.toggle('hidden', !state.diffPinnedMessage);
    const segBadge = document.getElementById('tab-segments-badge');
    const p = currentPreview(d);
    const segCount = p && p.data && p.data.message && !p.data.message.parse_error ? (p.data.message.segments || []).length : 0;
    if (segBadge) {
        segBadge.textContent = segCount;
        segBadge.classList.toggle('hidden', segCount === 0);
    }
    if (state.sendMenu && typeof renderSendMenu === 'function') renderSendMenu();
}

// Re-render only the title row's buttons, keeping keyboard focus.
function updateDraftActions() {
    const d = openDraftObject();
    const el = document.getElementById('detail-actions');
    if (!d || !el) return;
    const active = document.activeElement;
    const focused = active && el.contains && el.contains(active) ? active.dataset.action : null;
    el.innerHTML = draftActionsHtml(d);
    if (focused) {
        const again = el.querySelector(`[data-action="${focused}"]`);
        if (again && again.focus) again.focus();
    }
}

function renderDraftDetail() {
    renderDraftHeader();
    renderDraftTab(document.getElementById('detail-content'));
}

function emptyNote(text) {
    const div = document.createElement('div');
    div.className = 'empty-state';
    const p = document.createElement('p');
    p.textContent = text;
    div.appendChild(p);
    return div;
}

function renderDraftTab(content) {
    const d = openDraftObject();
    if (!d || !content) return;
    // Leaving or entering the diff changes which preview is needed (filled
    // or as written): ask for the right one.
    const shown = state.draftPreview;
    if (state.activeTab !== 'ack' && shown && shown.draftId === d.id && shown.filled !== draftNeedsFilledPreview(d)) {
        requestDraftPreview(d, true);
    }
    if (state.activeTab === 'raw') return renderDraftRawTab(content, d);
    if (state.activeTab === 'ack') return renderDraftResponseTab(content, d);
    const p = state.draftPreview && state.draftPreview.draftId === d.id && state.draftPreview.raw === d.raw ? state.draftPreview : null;
    if (state.activeTab === 'diff') {
        if (!state.diffPinnedMessage) {
            content.replaceChildren(emptyNote('No reference message pinned.'));
            return;
        }
        if (!p || p.filled !== draftNeedsFilledPreview(d)) {
            content.replaceChildren(emptyNote('Reading the draft…'));
            requestDraftPreview(d, true);
            return;
        }
        if (p.error) {
            content.replaceChildren(emptyNote(p.error));
            return;
        }
        renderDiffTab(content, p.data.message);
        return;
    }
    // Segments: field by field (#278), with the inspector's validation.
    if (!p) {
        content.replaceChildren(emptyNote('Reading the draft…'));
        return;
    }
    if (p.error) {
        content.replaceChildren(emptyNote(p.error));
        return;
    }
    renderDraftSegmentsTab(content, d, p.data);
}

// --- Editing field by field (#278) ---
//
// Every field is an input. A changed value is written back by the server
// (`POST /api/parse` with `set`, the code that writes overrides — the
// browser parses nothing), and the answer is the new text and its preview.
// The tab is then updated in place — validation, checklist, warning marks —
// so the input the user moved on to keeps its focus and what was typed.
// Adding, duplicating and removing a segment cut and insert whole lines at
// the offsets the server gave (`positions`), then re-render.

const DRAFT_QUICK = {
    'MSH-7': { value: '{{now}}', label: 'now', badge: 'set when sent' },
    'MSH-10': { value: '{{id}}', label: 'new ID', badge: 'new for every send' },
};

// `SEG-n` / `SEG[k]-n` for the field `index` of the segment `seg` that is
// the `occurrence`-th of its name; null for a name the server's field paths
// do not take (`~OBX`, `obx`, nine letters).
function draftFieldPath(name, occurrence, index) {
    if (!/^[A-Z0-9]{3}$/.test(name)) return null;
    return occurrence === 1 ? `${name}-${index}` : `${name}[${occurrence}]-${index}`;
}

// Warnings by field (`PID-5`), for the warning marks. A validation warning
// names the segment, not which occurrence of it (#291 review), so a missing
// required field in one OBX marks that field in every OBX — the banner lists
// the warnings themselves.
function draftWarningIndex(message) {
    const fields = new Set();
    for (const w of message.validation_warnings || []) {
        if (w.field != null && w.segment) fields.add(`${w.segment}-${w.field}`);
    }
    return fields;
}

function placeholderBadge(path, value) {
    const quick = DRAFT_QUICK[path];
    if (quick && value === quick.value) return quick.badge;
    return /\{\{(?!open\}\})[^}]+\}\}/.test(value || '') ? 'filled in when sent' : '';
}

function renderDraftSegmentsTab(content, d, data) {
    const msg = data.message;
    if (msg.parse_error) {
        const children = [buildParseErrorEl(msg.parse_error)];
        children.push(emptyNote('Fields can be edited once the text parses — fix it in the Raw tab.'));
        content.replaceChildren(...children);
        return;
    }
    const children = [];
    const top = document.createElement('div');
    top.className = 'draft-validation';
    fillDraftValidation(top, msg);
    children.push(top);
    const warned = draftWarningIndex(msg);
    const seen = new Map();
    msg.segments.forEach((seg, segIdx) => {
        const occurrence = (seen.get(seg.name) || 0) + 1;
        seen.set(seg.name, occurrence);
        children.push(buildDraftSegmentBlock(d, seg, segIdx, occurrence, warned, msg.delimiters));
    });
    content.replaceChildren(...children);
}

// The checklist (missing segments addable with one click) and the banner.
function fillDraftValidation(container, msg) {
    const warnings = msg.validation_warnings || [];
    const missing = {};
    const fieldWarn = {};
    for (const w of warnings) {
        if (w.code === 'MISSING_SEGMENT') missing[w.segment] = w.message;
        else if (w.code === 'MISSING_FIELD') fieldWarn[w.segment] = true;
    }
    const nodes = [];
    if (msg.typical_segments && msg.typical_segments.length) {
        const list = buildTypicalChecklist(msg, missing, fieldWarn);
        for (const pill of list.children) {
            const name = pill.dataset && pill.dataset.seg;
            if (!name || !(pill.classList.contains('absent') || pill.classList.contains('missing'))) continue;
            pill.classList.add('addable');
            pill.dataset.action = 'draft-add-segment';
            pill.setAttribute('role', 'button');
            pill.tabIndex = 0;
            pill.title = `Add a ${name} segment` + (pill.title ? ` — ${pill.title}` : '');
            pill.textContent = pill.classList.contains('missing') ? `+ ${name} \u2715` : `+ ${name}`;
        }
        nodes.push(list);
    }
    if (warnings.length) nodes.push(buildValidationBanner(warnings));
    container.replaceChildren(...nodes);
}

function segToolButton(action, label, segIdx, extra = '') {
    const b = document.createElement('button');
    b.className = 'seg-tool';
    b.dataset.action = action;
    b.dataset.segIdx = String(segIdx);
    b.textContent = label;
    if (extra) b.title = extra;
    return b;
}

function buildDraftSegmentBlock(d, seg, segIdx, occurrence, warned, delims) {
    const key = `draft-${d.id}-${segIdx}`;
    const collapsed = state.collapsedSegments.has(key);
    const block = document.createElement('div');
    block.className = collapsed ? 'segment-block collapsed' : 'segment-block';
    block.dataset.segIdx = String(segIdx);

    const head = document.createElement('div');
    head.className = 'segment-name';
    head.dataset.segKey = key;
    head.setAttribute('role', 'button');
    head.tabIndex = 0;
    head.setAttribute('aria-expanded', String(!collapsed));
    const icon = document.createElement('span');
    icon.className = 'collapse-icon';
    icon.innerHTML = collapsed ? ICONS.chevronRight : ICONS.chevronDown;
    head.appendChild(icon);
    head.appendChild(document.createTextNode(` ${seg.name} `));
    head.appendChild(textSpan('field-count', `(${seg.fields.length})`));
    const tools = document.createElement('span');
    tools.className = 'seg-tools';
    const emptyKey = `${seg.name}[${occurrence}]`;
    const showingEmpty = state.draftShowEmpty.has(emptyKey);
    tools.appendChild(segToolButton('draft-show-empty', showingEmpty ? 'hide empty fields' : 'show empty fields', segIdx));
    if (seg.name !== 'MSH') {
        tools.appendChild(segToolButton('draft-dup-segment', 'duplicate', segIdx, 'Insert a copy of this segment below it'));
        tools.appendChild(segToolButton('draft-remove-segment', 'remove', segIdx, 'Remove this segment from the draft'));
    }
    head.appendChild(tools);
    block.appendChild(head);

    const table = document.createElement('table');
    table.className = 'field-table editable';
    const tbody = document.createElement('tbody');
    const present = new Map(seg.fields.map((f) => [f.index, f]));
    const dict = showingEmpty ? state.dictionaryCache.get(seg.name) : null;
    const highest = Math.max(
        seg.fields.length ? seg.fields[seg.fields.length - 1].index : 0,
        dict && dict.fields.length ? dict.fields[dict.fields.length - 1].seq : 0,
    );
    const dictBySeq = new Map(dict ? dict.fields.map((f) => [f.seq, f]) : []);
    for (let index = 1; index <= highest; index++) {
        const f = present.get(index);
        const path = draftFieldPath(seg.name, occurrence, index);
        const label = `${seg.name}-${index}`;
        const isWarn = warned.has(label);
        const value = f ? f.value : '';
        // Empty fields are listed only on request, or when they are missing
        // and required.
        if (!value && !showingEmpty && !isWarn) continue;
        const def = dictBySeq.get(index);
        const description = (f && f.description) || (def && def.name) || '';
        const datatype = (f && f.datatype) || (def && def.datatype) || '';
        tbody.appendChild(buildDraftFieldRow(seg.name, index, path, value, description, datatype, isWarn));
    }
    table.appendChild(tbody);
    block.appendChild(table);
    return block;
}

function buildDraftFieldRow(segName, index, path, value, description, datatype, isWarn) {
    const tr = document.createElement('tr');
    tr.dataset.label = `${segName}-${index}`;
    if (isWarn) tr.className = 'warn';
    const tdIdx = document.createElement('td');
    tdIdx.className = 'field-idx';
    tdIdx.appendChild(document.createTextNode(`${segName}-${index}`));
    if (description || datatype) {
        const desc = [description, datatype].filter(Boolean).join(' \u00b7 ');
        tdIdx.appendChild(textSpan('desc-text', desc, desc));
    }
    tr.appendChild(tdIdx);

    const tdVal = document.createElement('td');
    tdVal.className = 'field-edit';
    const delimiterField = segName === 'MSH' && index <= 2;
    const tooLong = value.length > DISPLAY_VALUE_MAX;
    if (delimiterField || !path || tooLong) {
        // MSH-1/MSH-2 are the delimiters; a very long value (a file's
        // attachment) would freeze an input (#215).
        const shown = previewText(value);
        tdVal.appendChild(textSpan('field-fixed', shown.text));
        const why = delimiterField ? 'the delimiters — change them in the Raw tab'
            : !path ? 'this segment name cannot be addressed — edit it in the Raw tab'
            : `${formatSize(utf8Length(value))} — edit it in the Raw tab`;
        tdVal.appendChild(textSpan('field-note', why));
    } else {
        const input = document.createElement('input');
        input.type = 'text';
        input.className = 'field-input';
        input.value = value;
        input.spellcheck = false;
        input.dataset.fieldPath = path;
        input.dataset.written = value;
        input.setAttribute('aria-label', `${segName}-${index}${description ? ' ' + description : ''}`);
        input.addEventListener('change', () => writeDraftField(path, input.value));
        tdVal.appendChild(input);
        syncPlaceholderMark(input, path);
    }
    tr.appendChild(tdVal);

    const tdQuick = document.createElement('td');
    tdQuick.className = 'field-quick-cell';
    const quick = DRAFT_QUICK[`${segName}-${index}`];
    if (quick && path) {
        const b = document.createElement('button');
        b.className = 'field-quick';
        b.dataset.action = 'draft-quick';
        b.dataset.path = path;
        b.dataset.value = quick.value;
        b.textContent = quick.label;
        b.title = `Set ${segName}-${index} to ${quick.value}: ${quick.badge}`;
        tdQuick.appendChild(b);
    }
    tr.appendChild(tdQuick);
    return tr;
}

// A value with a placeholder is marked, with what it becomes.
function syncPlaceholderMark(input, path) {
    const badge = placeholderBadge(path, input.value);
    input.classList.toggle('auto', !!badge);
    input.title = badge ? `${input.value}: ${badge}` : '';
}

// Write `value` into the field `path` of the open draft. Writes are queued,
// so two quick changes apply in order to the right text.
let draftWriteChain = Promise.resolve();
function writeDraftField(path, value) {
    draftWriteChain = draftWriteChain.then(() => writeDraftFieldNow(path, value)).catch(() => {});
    return draftWriteChain;
}

async function writeDraftFieldNow(path, value) {
    const d = openDraftObject();
    if (!d) return false;
    // The text the field is written into: if the draft changed meanwhile
    // (the Raw tab), the answer would overwrite that change (#291 review).
    const base = d.raw;
    let resp;
    try {
        resp = await fetch('/api/parse', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ raw: base, set: { [path]: value }, based_on: d.basedOn }),
        });
    } catch (e) {
        showToast(`${path} not written: ` + (e.message || e));
        revertDraftInput(path);
        return false;
    }
    if (!resp.ok) {
        showToast(`${path} not written: ${(await resp.text()) || `HTTP ${resp.status}`}`);
        revertDraftInput(path);
        return false;
    }
    const data = await resp.json();
    if (state.selectedDraftId !== d.id) return false;
    if (d.raw !== base) {
        showToast(`${path} not written: the draft changed meanwhile — write it again`, 'info');
        revertDraftInput(path);
        return false;
    }
    d.raw = data.raw;
    if (state.draftSendAnyway === d.id) state.draftSendAnyway = null;
    saveDrafts();
    state.draftPreviewSeq++; // an older preview in flight must not win
    state.draftPreview = { draftId: d.id, raw: d.raw, filled: false, data, error: null };
    const m = data.message;
    d.type = m.parse_error ? '' : m.message_type || '';
    d.title = m.patient_name || m.patient_id || '';
    renderDraftRail();
    renderDraftHeader();
    if (state.activeTab === 'parsed') refreshDraftSegmentsInPlace(path, value, data);
    return true;
}

function draftInputFor(path) {
    const content = document.getElementById('detail-content');
    if (!content || !content.querySelector) return null;
    return content.querySelector(`.field-input[data-field-path="${CSS.escape(path)}"]`);
}

function revertDraftInput(path) {
    const input = draftInputFor(path);
    if (input) {
        input.value = input.dataset.written || '';
        syncPlaceholderMark(input, path);
    }
}

// After a write: the same segments, so only the marks, the checklist and
// the banner change, and the written input shows what was written — unless
// it has the focus and the user typed on. Another segment list re-renders.
function refreshDraftSegmentsInPlace(path, value, data) {
    const content = document.getElementById('detail-content');
    const d = openDraftObject();
    if (!content || !d) return;
    const blocks = content.querySelectorAll ? content.querySelectorAll('.segment-block') : [];
    if (blocks.length !== data.message.segments.length || data.message.parse_error) {
        renderDraftSegmentsTab(content, d, data);
        return;
    }
    const top = content.querySelector('.draft-validation');
    if (top) fillDraftValidation(top, data.message);
    const warned = draftWarningIndex(data.message);
    for (const tr of content.querySelectorAll('tr[data-label]')) {
        tr.classList.toggle('warn', warned.has(tr.dataset.label));
    }
    const input = draftInputFor(path);
    if (input) {
        input.dataset.written = value;
        if (document.activeElement !== input || input.value === value) input.value = value;
        syncPlaceholderMark(input, path);
    }
}

// --- Whole segments, at the server's offsets ---

function draftLines(d) {
    const p = currentPreview(d);
    if (!p || p.filled || !p.data || p.data.message.parse_error) {
        showToast('Wait for the preview of the draft, then try again', 'info');
        return null;
    }
    return p.data;
}

function replaceDraftText(d, text) {
    d.raw = text;
    if (state.draftSendAnyway === d.id) state.draftSendAnyway = null;
    saveDrafts();
    renderDraftRail();
    requestDraftPreview(d, true);
}

function removeDraftSegment(segIdx) {
    const d = openDraftObject();
    const data = d && draftLines(d);
    const pos = data && data.positions[segIdx];
    if (!pos || pos.name === 'MSH') return;
    let { start, end } = pos;
    // The line and one line end with it.
    if (d.raw[end] === '\n') end += 1;
    else if (start > 0 && d.raw[start - 1] === '\n') start -= 1;
    replaceDraftText(d, d.raw.slice(0, start) + d.raw.slice(end));
}

function duplicateDraftSegment(segIdx) {
    const d = openDraftObject();
    const data = d && draftLines(d);
    const pos = data && data.positions[segIdx];
    if (!pos || pos.name === 'MSH') return;
    const line = d.raw.slice(pos.start, pos.end);
    replaceDraftText(d, d.raw.slice(0, pos.end) + '\n' + line + d.raw.slice(pos.end));
}

// A missing segment goes after the last present one that comes before it
// in the message type's typical order (else after MSH).
function addDraftSegment(name) {
    const d = openDraftObject();
    const data = d && draftLines(d);
    if (!data || !/^[A-Z0-9]{3}$/.test(name)) return;
    const order = data.message.typical_segments || [];
    const rank = order.indexOf(name);
    let after = data.positions[0];
    for (const pos of data.positions) {
        const r = order.indexOf(pos.name);
        if (r !== -1 && r < rank) after = pos;
    }
    if (!after) return;
    const sep = (data.message.delimiters || '|')[0];
    replaceDraftText(d, d.raw.slice(0, after.end) + `\n${name}${sep}` + d.raw.slice(after.end));
}

// *Show empty fields*: the segment's fields from the dictionary.
async function toggleDraftEmptyFields(segIdx) {
    const d = openDraftObject();
    const p = d && currentPreview(d);
    const seg = p && p.data && p.data.message.segments[segIdx];
    if (!seg) return;
    const occurrence = p.data.message.segments.slice(0, segIdx + 1).filter((s) => s.name === seg.name).length;
    const key = `${seg.name}[${occurrence}]`;
    if (state.draftShowEmpty.has(key)) {
        state.draftShowEmpty.delete(key);
    } else {
        state.draftShowEmpty.add(key);
        if (!state.dictionaryCache.has(seg.name)) {
            try {
                const resp = await fetch(`/api/dictionary/${encodeURIComponent(seg.name)}`);
                state.dictionaryCache.set(seg.name, resp.ok ? await resp.json() : { fields: [] });
            } catch (e) {
                state.dictionaryCache.set(seg.name, { fields: [] });
            }
        }
    }
    const content = document.getElementById('detail-content');
    if (state.activeTab === 'parsed' && state.selectedDraftId === d.id) renderDraftTab(content);
}

function renderDraftRawTab(content, d) {
    const wrap = document.createElement('div');
    wrap.className = 'draft-raw';
    if (d.references.length) {
        const refs = document.createElement('div');
        refs.className = 'draft-refs';
        refs.appendChild(textSpan('k', 'From the original'));
        for (const r of d.references) {
            const chip = textSpan('ref-chip', `${r.placeholder || r.path} · ${formatSize(r.bytes || 0)}`,
                `${r.path} of the message this draft was made from (${(r.chars || 0).toLocaleString()} characters). Filled in by the server when the draft is sent.`);
            refs.appendChild(chip);
        }
        wrap.appendChild(refs);
    }
    const ta = document.createElement('textarea');
    ta.id = 'draft-text';
    ta.className = 'draft-text';
    ta.spellcheck = false;
    ta.setAttribute('aria-label', 'Draft text, one segment per line');
    ta.setAttribute('autocomplete', 'off');
    ta.value = d.raw;
    ta.addEventListener('input', () => onDraftInput(ta));
    for (const ev of ['keyup', 'click', 'select', 'focus']) ta.addEventListener(ev, updateDraftStatus);
    wrap.appendChild(ta);
    const status = document.createElement('div');
    status.id = 'draft-status';
    status.className = 'draft-status';
    status.setAttribute('aria-live', 'polite');
    wrap.appendChild(status);
    content.replaceChildren(wrap);
    updateDraftStatus();
}

// The chip of one entry of a draft's history: a send's response, or a
// run's end (#280).
function sendChip(s) {
    if (s.run) {
        return {
            cls: s.ok ? 'msg-ack aa' : 'msg-ack ae',
            text: `${s.done} / ${s.total}`,
            title: `A run: ${(typeof RUN_END_TEXT === 'object' && RUN_END_TEXT[s.state]) || s.state}`,
        };
    }
    return deliveryChip({ state: s.state, ack_code: s.code, error: s.error });
}

function sendFacts(s) {
    if (s.run) return [`run to ${s.target}`, s.summary].filter(Boolean).join(' · ');
    return [`to ${s.target}`, formatLatency(s.ms)].filter(Boolean).join(' · ');
}

// The draft's last result and its earlier sends, each a link to the sent
// message (the rows themselves come over the WebSocket).
function renderDraftResponseTab(content, d) {
    if (d.sends.length === 0) {
        content.replaceChildren(emptyNote('Not sent yet. Ctrl+Enter sends the draft to its target.'));
        return;
    }
    const children = [];
    const last = d.sends[d.sends.length - 1];
    const chip = sendChip(last);
    const result = document.createElement('div');
    result.className = 'response-result';
    result.appendChild(textSpan(chip.cls, chip.text, chip.title));
    result.appendChild(textSpan('response-facts', sendFacts(last)));
    children.push(result);
    const history = document.createElement('div');
    history.className = 'send-history';
    const h = document.createElement('h5');
    h.textContent = 'Sends of this draft';
    history.appendChild(h);
    for (const s of [...d.sends].reverse()) {
        const row = document.createElement('div');
        row.className = 'h-row';
        row.appendChild(textSpan('when', new Date(s.at).toLocaleTimeString()));
        const c = sendChip(s);
        row.appendChild(textSpan(c.cls, c.text, c.title));
        row.appendChild(textSpan('what', sendFacts(s)));
        if (!s.id) { // a run (#280): its rows are in the list
            history.appendChild(row);
            continue;
        }
        const link = document.createElement('button');
        link.className = 'origin-link';
        link.dataset.action = 'select-message';
        link.dataset.id = s.id;
        link.textContent = 'show';
        link.title = 'Open the sent message';
        row.appendChild(link);
        history.appendChild(row);
    }
    children.push(history);
    content.replaceChildren(...children);
}

// --- New message menu (header) ---

function renderNewMessageButton() {
    const wrap = document.getElementById('new-message');
    if (!wrap) return;
    wrap.classList.toggle('hidden', !state.sendConfig);
    const menu = document.getElementById('new-menu');
    if (menu) menu.classList.toggle('hidden', !state.newMenuOpen || !state.sendConfig);
    const btn = document.getElementById('btn-new-message');
    if (btn) btn.setAttribute('aria-expanded', String(!!state.newMenuOpen));
}

function toggleNewMenu(open = !state.newMenuOpen) {
    state.newMenuOpen = !!open;
    renderNewMessageButton();
    if (state.newMenuOpen) loadTemplateMenu();
}

// --- Templates (#279) ---
//
// The built-in set and the site's directory (`send.templates_dir`), listed
// by the server each time the menu opens — the directory is read then, so a
// template the team adds shows up without a restart. Choosing one makes a
// draft and opens its Segments tab: "fill out a template" is changing its
// fields and sending it.

async function loadTemplateMenu() {
    let list = null;
    try {
        const resp = await fetch('/api/templates');
        if (resp.ok) list = await resp.json();
    } catch (e) {
        console.debug('Could not list the templates:', e);
    }
    state.templates = Array.isArray(list) ? list : [];
    renderTemplateMenu();
}

function renderTemplateMenu() {
    const box = document.getElementById('new-menu-templates');
    if (!box) return;
    const groups = new Map();
    for (const t of state.templates) {
        if (!groups.has(t.group)) groups.set(t.group, []);
        groups.get(t.group).push(t);
    }
    let html = '';
    for (const [group, items] of groups) {
        html += `<div class="new-menu-group">${esc(group === 'Site' ? 'Site templates' : 'Templates')}</div>`;
        html += items.map((t) => `<button role="menuitem" data-action="new-template" data-template-id="${escAttr(t.id)}">`
            + `${esc(t.name)}${t.message_type && !t.name.startsWith(t.message_type) ? ` <span class="hint">${esc(t.message_type)}</span>` : ''}</button>`).join('');
    }
    box.innerHTML = html;
}

async function newDraftFromTemplate(id) {
    let t;
    try {
        const resp = await fetch(`/api/templates/${encodeURIComponent(id)}`);
        if (!resp.ok) {
            showToast(`Cannot open the template: ${(await resp.text()) || `HTTP ${resp.status}`}`);
            return null;
        }
        t = await resp.json();
    } catch (e) {
        showToast('Cannot open the template: ' + (e.message || e));
        return null;
    }
    if (t.warning) showToast(`${t.name}: ${t.warning}`, 'info');
    const draft = addDraft({
        raw: t.raw,
        charset: t.encoding || 'auto',
        origin: { kind: 'template', id: t.id, name: t.name },
        type: t.message_type || '',
    });
    if (draft) openDraft(draft.id, 'parsed');
    return draft;
}

// Sending switched on or off (send.js): the drafts UI follows it.
function applyDraftsUi() {
    renderNewMessageButton();
    renderDraftRail();
    const d = openDraftObject();
    if (d && !state.sendConfig) closeDraftView();
    else if (d && !document.getElementById('draft-text') && !state.draftPreview) {
        syncTabButtons();
        renderDraftDetail();
        requestDraftPreview(d, true);
    }
}

// --- Keyboard and delegation (called from app.js) ---

// Ctrl/Cmd+Enter sends the open draft. Returns true if it took the key.
function handleDraftKeydown(e) {
    if ((e.ctrlKey || e.metaKey) && e.key === 'Enter' && isDraftOpen()) {
        e.preventDefault();
        sendOpenDraft();
        return true;
    }
    return false;
}

function handleComposeAction(action, el) {
    switch (action) {
        case 'draft-open': openDraft(el.dataset.draftId); return true;
        case 'draft-delete': deleteDraft(el.dataset.draftId || state.selectedDraftId); return true;
        case 'draft-send': sendOpenDraft(); return true;
        case 'draft-keep': if (state.selectedDraftId) toggleKeepDraft(state.selectedDraftId); return true;
        case 'draft-save-file': saveDraftAsFile(); return true;
        case 'draft-edit': editMessage(el.dataset.id); return true;
        case 'draft-quick': {
            const input = draftInputFor(el.dataset.path);
            if (input) { input.value = el.dataset.value; syncPlaceholderMark(input, el.dataset.path); }
            writeDraftField(el.dataset.path, el.dataset.value);
            return true;
        }
        case 'draft-add-segment': addDraftSegment(el.dataset.seg); return true;
        case 'draft-dup-segment': duplicateDraftSegment(Number(el.dataset.segIdx)); return true;
        case 'draft-remove-segment': removeDraftSegment(Number(el.dataset.segIdx)); return true;
        case 'draft-show-empty': toggleDraftEmptyFields(Number(el.dataset.segIdx)); return true;
        case 'new-message': toggleNewMenu(); return true;
        case 'new-blank': toggleNewMenu(false); newBlankDraft(); return true;
        case 'new-clipboard': toggleNewMenu(false); newDraftFromClipboard(); return true;
        case 'new-file': toggleNewMenu(false); newDraftFromFile(); return true;
        case 'new-template': toggleNewMenu(false); newDraftFromTemplate(el.dataset.templateId); return true;
        default: return false;
    }
}
