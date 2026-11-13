// Tests for drafts in the UI (#277, static/compose.js): draft storage (a
// reload, *keep*, storage that fails), the one selection (an open draft
// survives `cleared`, `evicted` and a reload), the keyboard (a space typed
// into a draft, Ctrl+Enter), sending (no second row, *Send anyway*, *ask
// before sending*, the request), *Edit*, a file, the preview's last-wins
// rule and the cursor's field. state.js … compose.js run in a `node:vm`
// context with a fake DOM.
//
//   node --test 'tests/js/*.test.mjs'
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { webcrypto } from 'node:crypto';
import vm from 'node:vm';
import { El } from './fake-dom.mjs';

const read = (f) => readFileSync(new URL(`../../static/${f}`, import.meta.url), 'utf8');

function memoryStorage() {
    const data = new Map();
    const s = {
        broken: false,
        getItem: (k) => { if (s.broken) throw new Error('denied'); return data.has(k) ? data.get(k) : null; },
        setItem: (k, v) => { if (s.broken) throw new Error('quota'); data.set(k, String(v)); },
        data,
    };
    return s;
}

// `responses` maps a URL prefix to a function (url, opts) → { ok, json/text }.
function setup({ local = memoryStorage(), session = memoryStorage(), confirmAnswer = true, responses = {} } = {}) {
    const noop = () => {};
    const toasts = [];
    const requests = [];
    const confirms = [];
    const elements = new Map();
    const byId = (id) => {
        if (!elements.has(id)) {
            const el = new El('', id);
            el.addEventListener = noop;
            elements.set(id, el);
        }
        return elements.get(id);
    };
    const make = (tag) => {
        const el = new El();
        el.tagName = String(tag).toUpperCase();
        el.addEventListener = noop;
        return el;
    };
    const ctx = {
        window: {},
        console: { ...console, debug: noop, info: noop },
        document: {
            getElementById: byId,
            createElement: make,
            createTextNode: (text) => { const el = new El(); el.textContent = String(text); return el; },
            querySelectorAll: () => [],
            activeElement: null,
        },
        CSS: { escape: (v) => String(v) },
        navigator: {},
        sessionStorage: session,
        localStorage: local,
        crypto: webcrypto,
        setTimeout: noop,
        clearTimeout: noop,
        WeakMap, Set, Map, JSON, Uint8Array, Array, Date, Promise,
        confirm: (q) => { confirms.push(q); return confirmAnswer; },
        fetch: async (url, opts) => {
            requests.push({ url, opts });
            const key = Object.keys(responses).find((p) => url.startsWith(p));
            if (key) return responses[key](url, opts);
            return { ok: true, json: async () => ({}), text: async () => '' };
        },
    };
    vm.createContext(ctx);
    for (const f of ['state.js', 'util.js', 'ws.js', 'render.js', 'diff.js', 'send.js', 'compose.js']) {
        vm.runInContext(read(f), ctx);
    }
    // esc() needs a real DOM to escape; the fake one only keeps text.
    const escape = (v) => String(v ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
    Object.assign(ctx, {
        esc: escape,
        escAttr: escape,
        showToast: (m, type) => toasts.push([m, type]),
        renderMessageList: noop,
        renderSourceLegend: noop,
        renderHealthPills: noop,
        updateHeaderCounters: noop,
        removeRowsFromList: noop,
    });
    const state = ctx.window.state;
    state.sendConfig = { enabled: true, loopback: { name: 'This Harux', host: '127.0.0.1', port: 2575 }, tls: { allow_unverified: false } };
    return { ctx, state, toasts, requests, confirms, local, session, byId };
}

const plain = (v) => JSON.parse(JSON.stringify(v));
const okJson = (body) => async () => ({ ok: true, json: async () => body, text: async () => JSON.stringify(body) });

test('drafts stay in the tab; kept ones on this computer; a reload brings both back', () => {
    const { ctx, state, local, session } = setup();
    const a = ctx.addDraft({ raw: 'MSH|^~\\&|A\r\nPID|1', origin: { kind: 'new' } });
    const b = ctx.addDraft({ raw: 'MSH|^~\\&|B', origin: { kind: 'new' } });
    assert.equal(a.raw, 'MSH|^~\\&|A\nPID|1', 'line ends as a textarea keeps them');
    ctx.toggleKeepDraft(b.id);
    assert.deepEqual(plain(JSON.parse(session.data.get('harux_drafts')).map((d) => d.id)), [a.id]);
    assert.deepEqual(plain(JSON.parse(local.data.get('harux_drafts_kept')).map((d) => d.id)), [b.id]);

    // A reload: a new page with the same storage.
    const again = setup({ local, session });
    again.ctx.loadDrafts();
    assert.deepEqual(plain(again.state.drafts.map((d) => [d.id, d.keep])), [[a.id, false], [b.id, true]]);
});

test('storage that fails is never fatal: the drafts last for the page, one toast says so', () => {
    const session = memoryStorage();
    session.broken = true;
    const { ctx, state, toasts } = setup({ session });
    ctx.addDraft({ raw: 'MSH|1' });
    ctx.addDraft({ raw: 'MSH|2' });
    assert.equal(state.drafts.length, 2);
    assert.equal(toasts.length, 1);
    assert.match(toasts[0][0], /do(es)? not keep your drafts/);
    // Unreadable storage at load: no drafts, no exception.
    ctx.loadDrafts();
    assert.equal(state.drafts.length, 0);
});

test('one selection: clearing a message selection never closes the draft', () => {
    const { ctx, state } = setup();
    state.selectedId = 'm1';
    assert.deepEqual(plain(state.selection), { kind: 'message', id: 'm1' });
    const d = ctx.addDraft({ raw: 'MSH|1' });
    state.selection = { kind: 'draft', id: d.id };
    assert.equal(state.selectedId, null);
    assert.equal(state.selectedDraftId, d.id);
    state.selectedId = null; // what `cleared` / `evicted` do
    assert.equal(state.selectedDraftId, d.id, 'the draft stays open');
});

test('an open draft survives `cleared`, `evicted` and a reload of the list', async () => {
    const { ctx, state, byId, requests } = setup();
    const d = ctx.addDraft({ raw: 'MSH|1' });
    state.selection = { kind: 'draft', id: d.id };
    const rendered = [];
    ctx.renderDraftDetail = () => rendered.push(state.selectedDraftId);
    byId('detail-content').innerHTML = 'the draft';

    ctx.handleServerEvent({ type: 'cleared' });
    assert.equal(state.selectedDraftId, d.id);
    assert.equal(byId('detail-content').innerHTML, 'the draft', 'not emptied');
    assert.deepEqual(rendered, [d.id]);

    state.messages = [{ id: 'm1', tags: [] }];
    state.diffPinnedMessage = { id: 'm1' };
    ctx.handleServerEvent({ type: 'evicted', data: ['m1'] });
    assert.equal(state.selectedDraftId, d.id);
    assert.equal(state.diffPinnedMessage, null);
    assert.deepEqual(rendered, [d.id, d.id], 'redrawn without its pinned reference');

    // The list reloads (init / lagged): no message is fetched into the panel.
    await ctx.restoreAfterLoad();
    assert.ok(!requests.some((r) => r.url.startsWith('/api/messages/')), 'nothing selected');

    // A selected message, by contrast, is cleared with the store.
    state.selectedId = 'm2';
    ctx.handleServerEvent({ type: 'cleared' });
    assert.equal(state.selection, null);
    assert.notEqual(byId('detail-content').innerHTML, 'the draft');
});

test('a space typed into a draft is the textarea\'s; Ctrl+Enter sends', () => {
    const { ctx, state } = setup();
    const textarea = { tagName: 'TEXTAREA', dataset: {} };
    assert.equal(ctx.isTextEntry(textarea), true);
    assert.equal(ctx.isTextEntry({ tagName: 'SELECT' }), true);
    assert.equal(ctx.isTextEntry({ tagName: 'INPUT', type: 'text' }), true);
    assert.equal(ctx.isTextEntry({ tagName: 'BUTTON', dataset: { action: 'x' } }), false);
    assert.equal(ctx.isTextEntry({ tagName: 'INPUT', type: 'checkbox' }), false);

    const d = ctx.addDraft({ raw: 'MSH|1' });
    state.selection = { kind: 'draft', id: d.id };
    let prevented = 0;
    const key = (k, extra = {}) => ({ key: k, preventDefault: () => prevented++, ...extra });
    assert.equal(ctx.handleDraftKeydown(key(' ')), false);
    assert.equal(ctx.handleDraftKeydown(key('Enter')), false);
    assert.equal(prevented, 0, 'typing is left alone');
    let sent = 0;
    ctx.sendOpenDraft = () => { sent++; return Promise.resolve(true); };
    assert.equal(ctx.handleDraftKeydown(key('Enter', { ctrlKey: true })), true);
    assert.equal(ctx.handleDraftKeydown(key('Enter', { metaKey: true })), true);
    assert.equal(prevented, 2);
    assert.equal(sent, 2);
    state.selection = null;
    assert.equal(ctx.handleDraftKeydown(key('Enter', { ctrlKey: true })), false, 'no draft open');
});

test('sending a draft: the request, the history, no second row', async () => {
    const { ctx, state, requests, toasts } = setup({
        responses: { '/api/send': okJson({ id: 'sent-1', delivery: { state: 'acked', ack_code: 'AA', latency_ms: 7 } }) },
    });
    ctx.renderDraftHeader = () => {};
    const d = ctx.addDraft({ raw: 'MSH|^~\\&|A', charset: 'windows-1252', basedOn: 'm1' });
    state.selection = { kind: 'draft', id: d.id };
    const target = ctx.loopbackTarget();
    assert.ok(ctx.draftHasUnsentChanges(d), 'never sent');

    assert.equal(await ctx.sendDraft(d, target), true);
    const req = requests.find((r) => r.url === '/api/send');
    assert.deepEqual(JSON.parse(req.opts.body), {
        target: 'loopback',
        items: [{ raw: 'MSH|^~\\&|A', charset: 'windows-1252', based_on: 'm1' }],
        allow_invalid: false,
    });
    // The sent row comes over the WebSocket only.
    assert.equal(state.messages.length + state.pendingMessages.length, 0);
    assert.deepEqual(plain(d.sends.map((s) => [s.id, s.code, s.ms, s.target])), [['sent-1', 'AA', 7, 'This Harux']]);
    assert.equal(d.targetId, 'loopback');
    assert.equal(ctx.draftHasUnsentChanges(d), false);
    assert.match(toasts.at(-1)[0], /^Sent to This Harux · AA · 7 ms/);
    d.raw += 'X';
    assert.equal(ctx.draftHasUnsentChanges(d), true, 'changed since the last send');
});

test('a draft that does not parse: the first click arms "Send anyway", the second sends', async () => {
    const { ctx, state, requests } = setup({
        responses: { '/api/send': okJson({ id: 's', delivery: { state: 'acked', ack_code: 'AE' } }) },
    });
    ctx.renderDraftHeader = () => {};
    const d = ctx.addDraft({ raw: 'not hl7' });
    state.selection = { kind: 'draft', id: d.id };
    state.draftPreview = { draftId: d.id, raw: d.raw, filled: false, data: { message: { parse_error: 'Message does not start with MSH' } }, error: null };
    const target = ctx.loopbackTarget();
    assert.equal(await ctx.sendDraft(d, target), false);
    assert.equal(state.draftSendAnyway, d.id);
    assert.equal(requests.filter((r) => r.url === '/api/send').length, 0, 'nothing sent yet');
    assert.match(ctx.draftActionsHtml(d), /Send anyway/);
    assert.equal(await ctx.sendDraft(d, target), true);
    assert.equal(JSON.parse(requests.find((r) => r.url === '/api/send').opts.body).allow_invalid, true);
    assert.equal(state.draftSendAnyway, null);
});

test('a target that asks before sending asks; "no" sends nothing', async () => {
    const { ctx, state, requests, confirms } = setup({ confirmAnswer: false });
    ctx.renderDraftHeader = () => {};
    const d = ctx.addDraft({ raw: 'MSH|1' });
    state.selection = { kind: 'draft', id: d.id };
    const prod = ctx.normalizeTarget({ name: 'prod', host: 'prod.local', port: 6661, ask: true });
    assert.equal(await ctx.sendDraft(d, prod), false);
    assert.equal(confirms.length, 1);
    assert.match(confirms[0], /prod \(prod\.local:6661\)/);
    assert.equal(requests.length, 0);
});

test('Edit makes a draft from the server\'s draft text, with its references and encoding', async () => {
    const { ctx, state, requests } = setup({
        responses: {
            '/api/messages/m1/draft': okJson({
                based_on: 'm1', raw: 'MSH|^~\\&|A\nOBX|1|ED|X||{{from:OBX-5}}', charset: '8859/1', encoding: 'windows-1252',
                references: [{ path: 'OBX-5', placeholder: '{{from:OBX-5}}', chars: 5000, bytes: 5000 }],
            }),
        },
    });
    ctx.openDraft = (id) => { state.selection = { kind: 'draft', id }; };
    state.selectedMessage = { id: 'm1', message_type: 'MDM^T02', patient_name: 'Muster, Max', message_control_id: 'C1' };
    const d = await ctx.editMessage('m1');
    assert.equal(requests[0].url, '/api/messages/m1/draft');
    assert.equal(d.basedOn, 'm1');
    assert.equal(d.charset, 'windows-1252');
    assert.equal(d.references[0].path, 'OBX-5');
    assert.deepEqual(plain(d.origin), { kind: 'edit', id: 'm1', control: 'C1' });
    assert.equal(d.title, 'Muster, Max');
    assert.equal(state.selectedDraftId, d.id);
    assert.match(ctx.draftOriginHtml(d), /^Edit of <button class="origin-link" data-action="select-message" data-id="m1"/);
});

test('a file goes to the server as bytes; the draft takes its encoding', async () => {
    const { ctx, state, requests, toasts } = setup({
        responses: { '/api/parse': okJson({ raw: 'MSH|^~\\&|A\nPID|||1||Müller', encoding: 'windows-1252', warning: null, framed: true }) },
    });
    ctx.openDraft = (id) => { state.selection = { kind: 'draft', id }; };
    const bytes = new Uint8Array([0x0b, 0x4d, 0xfc]);
    const d = await ctx.draftFromFileBytes('adt.hl7', bytes);
    assert.equal(requests[0].opts.headers['Content-Type'], 'application/octet-stream');
    assert.equal(requests[0].opts.body, bytes, 'the bytes, not text');
    assert.equal(d.charset, 'windows-1252');
    assert.equal(d.raw, 'MSH|^~\\&|A\nPID|||1||Müller');
    assert.match(ctx.draftOriginHtml(d), /From the file adt\.hl7/);
    assert.equal(toasts.length, 0);
});

test('the preview: the last request wins, and it names the draft in the rail', async () => {
    const answers = [];
    const { ctx, state } = setup({
        responses: { '/api/parse': () => new Promise((resolve) => answers.push(resolve)) },
    });
    ctx.renderDraftHeader = () => {};
    ctx.renderDraftRail = () => {};
    ctx.updateDraftStatus = () => {};
    state.activeTab = 'raw';
    const d = ctx.addDraft({ raw: 'MSH|1' });
    state.selection = { kind: 'draft', id: d.id };
    const first = ctx.requestDraftPreview(d, true);
    const second = ctx.requestDraftPreview(d, true);
    const reply = (type) => ({ ok: true, json: async () => ({ raw: d.raw, positions: [], message: { message_type: type, patient_name: 'P' } }) });
    answers[1](reply('NEW^ONE'));
    await second;
    answers[0](reply('OLD^ONE'));
    await first;
    assert.equal(d.type, 'NEW^ONE', 'the older answer is dropped');
    assert.equal(d.title, 'P');
    assert.equal(ctx.currentPreview(d).data.message.message_type, 'NEW^ONE');
    d.raw = 'MSH|2';
    assert.equal(ctx.currentPreview(d), null, 'a preview of other text is not current');
});

test('the cursor\'s field: UTF-16 positions, MSH-1, the segment name, the end of a field', () => {
    const { ctx } = setup();
    // "MSH|^~\&|A\nPID|||😀||Müller": positions as /api/parse gives them.
    const preview = {
        data: {
            positions: [
                { name: 'MSH', start: 0, end: 10, fields: [[1, 3, 4], [2, 4, 8], [3, 9, 10]] },
                { name: 'PID', start: 11, end: 27, fields: [[1, 15, 15], [2, 16, 16], [3, 17, 19], [4, 20, 20], [5, 21, 27]] },
            ],
            message: {
                segments: [
                    { name: 'MSH', fields: [{ index: 3, description: 'Sending Application', datatype: 'HD' }] },
                    { name: 'PID', fields: [{ index: 5, description: 'Patient Name', datatype: 'XPN' }, { index: 3, description: 'Patient Identifier List', datatype: 'CX' }] },
                ],
            },
        },
    };
    assert.equal(ctx.draftStatusText(preview, 22), 'PID-5 · Patient Name · XPN');
    assert.equal(ctx.draftStatusText(preview, 27), 'PID-5 · Patient Name · XPN', 'the end of the line');
    assert.equal(ctx.draftStatusText(preview, 18), 'PID-3 · Patient Identifier List · CX', 'inside the emoji');
    assert.equal(ctx.draftStatusText(preview, 12), 'PID', 'on the segment name');
    assert.equal(ctx.draftStatusText(preview, 3), 'MSH-1', 'the separator itself');
    assert.equal(ctx.draftStatusText(preview, 9), 'MSH-3 · Sending Application · HD');
    assert.equal(ctx.draftStatusText(preview, 40), '');
    assert.equal(ctx.draftStatusText(null, 3), '');
});

test('the rail: one chip per draft with a dot for unsent changes, only while sending is on', () => {
    const { ctx, state, byId } = setup();
    const a = ctx.addDraft({ raw: 'MSH|1', type: 'ADT^A01', title: 'Müller' });
    const b = ctx.addDraft({ raw: 'MSH|2', type: 'ORU^R01', title: '<b>x</b>' });
    b.sentRaw = b.raw;
    state.selection = { kind: 'draft', id: a.id };
    ctx.renderDraftRail();
    const rail = byId('draft-rail');
    assert.ok(!rail.classList.contains('hidden'));
    assert.equal((rail.innerHTML.match(/class="draft-chip/g) || []).length, 2);
    assert.match(rail.innerHTML, /class="draft-chip active"[^>]*data-draft-id="[^"]+"/);
    assert.equal((rail.innerHTML.match(/class="unsent"/g) || []).length, 1, 'b was sent as it is');
    assert.ok(!rail.innerHTML.includes('<b>x</b>'), 'escaped');
    state.sendConfig = null;
    ctx.renderDraftRail();
    assert.ok(rail.classList.contains('hidden'), 'nothing new while sending is off');
});

// --- #290 review ---

test('the status line never reads a preview with the references filled in', () => {
    const { ctx, state, byId } = setup();
    const d = ctx.addDraft({ raw: 'MSH|^~\\&|A\nOBX|1|ED|X||{{from:OBX-5}}', basedOn: 'm1' });
    state.selection = { kind: 'draft', id: d.id };
    const ta = byId('draft-text');
    ta.selectionStart = 0;
    const data = { positions: [{ name: 'MSH', start: 0, end: 10, fields: [] }], message: { segments: [] } };
    state.draftPreview = { draftId: d.id, raw: d.raw, filled: true, data, error: null };
    ctx.updateDraftStatus();
    assert.equal(byId('draft-status').textContent, 'Reading the draft…');
    state.draftPreview = { ...state.draftPreview, filled: false };
    ctx.updateDraftStatus();
    assert.equal(byId('draft-status').textContent, 'MSH');
});

test('a reload never drops a kept draft, however many drafts the tab holds', () => {
    const { ctx, state, local, session } = setup();
    for (let i = 0; i < 50; i++) ctx.addDraft({ raw: `MSH|${i}` });
    state.drafts[0].keep = true;
    ctx.saveDrafts();
    // The tab holds 49, the computer 1; the tab then gets one more.
    const tab = JSON.parse(session.data.get('harux_drafts'));
    tab.push({ id: 'extra', raw: 'MSH|x' });
    session.data.set('harux_drafts', JSON.stringify(tab));
    const again = setup({ local, session });
    again.ctx.loadDrafts();
    assert.equal(again.state.drafts.length, 51);
    assert.ok(again.state.drafts.some((d) => d.keep), 'the kept draft is there');
    again.ctx.saveDrafts();
    assert.equal(JSON.parse(local.data.get('harux_drafts_kept')).length, 1, 'and stays stored');
});

test('a sent draft makes its target the one the next draft starts with', async () => {
    const { ctx, state, local } = setup({
        responses: { '/api/send': okJson({ id: 's', delivery: { state: 'acked', ack_code: 'AA' } }) },
    });
    ctx.renderDraftHeader = () => {};
    const d = ctx.addDraft({ raw: 'MSH|1' });
    state.selection = { kind: 'draft', id: d.id };
    await ctx.sendDraft(d, ctx.loopbackTarget());
    assert.equal(state.lastTargetId, 'loopback');
    assert.equal(local.data.get('harux_last_target'), '"loopback"');
    const next = ctx.addDraft({ raw: 'MSH|2' });
    assert.equal(ctx.draftTarget(next).id, 'loopback');
});

test('a draft whose original is evicted or cleared says so at once', () => {
    const { ctx, state } = setup();
    ctx.renderDraftHeader = () => {};
    const withRefs = ctx.addDraft({ raw: 'OBX|1|ED|X||{{from:OBX-5}}', basedOn: 'm1', origin: { kind: 'edit', id: 'm1', control: 'C1' }, references: [{ path: 'OBX-5' }] });
    const other = ctx.addDraft({ raw: 'MSH|1', basedOn: 'm2', origin: { kind: 'edit', id: 'm2' } });
    state.messages = [{ id: 'm1', tags: [] }, { id: 'm2', tags: [] }];
    ctx.handleServerEvent({ type: 'evicted', data: ['m1'] });
    assert.equal(withRefs.originGone, true);
    assert.equal(other.originGone, false);
    assert.match(ctx.draftOriginHtml(withRefs), /no longer stored — its references cannot be filled in/);
    assert.ok(!ctx.draftOriginHtml(withRefs).includes('data-action="select-message"'), 'no dead link');
    ctx.handleServerEvent({ type: 'cleared' });
    assert.equal(other.originGone, true);
    assert.match(ctx.draftOriginHtml(other), /^Edit of a message <span class="origin-gone">\(no longer stored\)<\/span>$/);
});

// --- Editing field by field (#278) ---

test('field paths: the occurrence only when it is not the first; odd names none', () => {
    const { ctx } = setup();
    assert.equal(ctx.draftFieldPath('PID', 1, 5), 'PID-5');
    assert.equal(ctx.draftFieldPath('OBX', 3, 5), 'OBX[3]-5');
    assert.equal(ctx.draftFieldPath('obx', 1, 5), null);
    assert.equal(ctx.draftFieldPath('~OB', 1, 5), null);
    assert.equal(ctx.placeholderBadge('MSH-10', '{{id}}'), 'new for every send');
    assert.equal(ctx.placeholderBadge('MSH-7', '{{now}}'), 'set when sent');
    assert.equal(ctx.placeholderBadge('PID-3', 'X{{rand:6}}'), 'filled in when sent');
    assert.equal(ctx.placeholderBadge('PID-3', '{{open}}x'), '', 'an escaped brace is no placeholder');
    assert.equal(ctx.placeholderBadge('PID-3', 'plain'), '');
});

function previewOf(raw, message, positions = []) {
    return { raw, changes: [], message: { segments: [], validation_warnings: [], ...message }, positions };
}

test('a field is written back by the server; the answer is the new text and its preview', async () => {
    const replies = [];
    const { ctx, state, requests } = setup({
        responses: { '/api/parse': (url, opts) => replies.shift()(JSON.parse(opts.body)) },
    });
    for (const f of ['renderDraftRail', 'renderDraftHeader', 'refreshDraftSegmentsInPlace']) ctx[f] = () => {};
    const d = ctx.addDraft({ raw: 'MSH|^~\\&|A\nPID|||1||Muster', basedOn: 'm1' });
    state.selection = { kind: 'draft', id: d.id };
    replies.push((body) => ({ ok: true, json: async () => previewOf('MSH|^~\\&|A\nPID|||1||Doe^Jane', { message_type: 'ADT^A01', patient_name: 'Doe, Jane' }), text: async () => '' }));
    assert.equal(await ctx.writeDraftFieldNow('PID-5', 'Doe^Jane'), true);
    assert.deepEqual(JSON.parse(requests[0].opts.body), { raw: 'MSH|^~\\&|A\nPID|||1||Muster', set: { 'PID-5': 'Doe^Jane' }, based_on: 'm1' });
    assert.equal(d.raw, 'MSH|^~\\&|A\nPID|||1||Doe^Jane', 'both tabs show the server\'s text');
    assert.equal(ctx.currentPreview(d).data.message.patient_name, 'Doe, Jane');
    assert.equal(d.title, 'Doe, Jane');

    // A refused value changes nothing and says why.
    let reverted = null;
    ctx.revertDraftInput = (path) => { reverted = path; };
    replies.push(() => ({ ok: false, status: 422, text: async () => 'PID-5: the value contains the field separator' }));
    assert.equal(await ctx.writeDraftFieldNow('PID-5', 'a|b'), false);
    assert.equal(d.raw, 'MSH|^~\\&|A\nPID|||1||Doe^Jane');
    assert.equal(reverted, 'PID-5');
});

test('writes apply in order, and a send waits for them', async () => {
    const pending = [];
    const { ctx, state, requests } = setup({
        responses: {
            '/api/parse': (url, opts) => new Promise((resolve) => pending.push({ body: JSON.parse(opts.body), resolve })),
            '/api/send': okJson({ id: 's', delivery: { state: 'acked', ack_code: 'AA' } }),
        },
    });
    for (const f of ['renderDraftRail', 'renderDraftHeader', 'refreshDraftSegmentsInPlace']) ctx[f] = () => {};
    const d = ctx.addDraft({ raw: 'MSH|0' });
    state.selection = { kind: 'draft', id: d.id };
    const answer = (raw) => ({ ok: true, json: async () => previewOf(raw, {}), text: async () => '' });
    ctx.writeDraftField('PID-5', 'one');
    ctx.writeDraftField('PID-7', 'two');
    const sending = ctx.sendDraft(d, ctx.loopbackTarget());
    await new Promise((r) => setImmediate(r));
    assert.equal(pending.length, 1, 'the second write waits for the first');
    pending[0].resolve(answer('MSH|1'));
    await new Promise((r) => setImmediate(r));
    await new Promise((r) => setImmediate(r));
    assert.equal(pending.length, 2);
    assert.equal(pending[1].body.raw, 'MSH|1', 'written into the text the first write gave');
    assert.ok(!requests.some((r) => r.url === '/api/send'), 'no send before the writes');
    pending[1].resolve(answer('MSH|2'));
    await sending;
    const send = requests.find((r) => r.url === '/api/send');
    assert.equal(JSON.parse(send.opts.body).items[0].raw, 'MSH|2');
});

test('Ctrl+Enter in an input writes its value before the send', async () => {
    const { ctx, state, requests } = setup({
        responses: {
            '/api/parse': okJson(previewOf('MSH|^~\\&|A\nPID|||1||Written', {})),
            '/api/send': okJson({ id: 's', delivery: { state: 'acked', ack_code: 'AA' } }),
        },
    });
    for (const f of ['renderDraftRail', 'renderDraftHeader', 'refreshDraftSegmentsInPlace']) ctx[f] = () => {};
    const d = ctx.addDraft({ raw: 'MSH|^~\\&|A\nPID|||1||Old' });
    state.selection = { kind: 'draft', id: d.id };
    state.lastTargetId = 'loopback';
    const input = { value: 'Written', dataset: { fieldPath: 'PID-5', written: 'Old' }, classList: { contains: (c) => c === 'field-input' } };
    ctx.document.activeElement = input;
    await ctx.sendOpenDraft();
    assert.deepEqual(requests.map((r) => r.url), ['/api/parse', '/api/send']);
    assert.equal(JSON.parse(requests[1].opts.body).items[0].raw, 'MSH|^~\\&|A\nPID|||1||Written');
});

test('segments are added, duplicated and removed at the server\'s offsets', () => {
    const { ctx, state } = setup();
    ctx.requestDraftPreview = () => {};
    const raw = 'MSH|^~\\&|A\nEVN|A01\nPID|||1\nPV1||I';
    const d = ctx.addDraft({ raw });
    state.selection = { kind: 'draft', id: d.id };
    const lines = raw.split('\n');
    let at = 0;
    const positions = lines.map((l) => {
        const p = { name: l.slice(0, 3), start: at, end: at + l.length, fields: [] };
        at += l.length + 1;
        return p;
    });
    const setPreview = () => {
        state.draftPreview = { draftId: d.id, raw: d.raw, filled: false, error: null, data: previewOf(d.raw, { delimiters: '|^~\\&', typical_segments: ['MSH', 'EVN', 'PID', 'PD1', 'NK1', 'PV1', 'PV2'] }, positions) };
    };
    setPreview();
    ctx.addDraftSegment('NK1');
    assert.equal(d.raw, 'MSH|^~\\&|A\nEVN|A01\nPID|||1\nNK1|\nPV1||I', 'after PID, before PV1');
    d.raw = raw; setPreview();
    ctx.addDraftSegment('PV2');
    assert.equal(d.raw, raw + '\nPV2|', 'at the end');
    d.raw = raw; setPreview();
    ctx.duplicateDraftSegment(2);
    assert.equal(d.raw, 'MSH|^~\\&|A\nEVN|A01\nPID|||1\nPID|||1\nPV1||I');
    d.raw = raw; setPreview();
    ctx.removeDraftSegment(1);
    assert.equal(d.raw, 'MSH|^~\\&|A\nPID|||1\nPV1||I');
    d.raw = raw; setPreview();
    ctx.removeDraftSegment(3);
    assert.equal(d.raw, 'MSH|^~\\&|A\nEVN|A01\nPID|||1', 'the last line takes the line end before it');
    d.raw = raw; setPreview();
    ctx.removeDraftSegment(0);
    assert.equal(d.raw, raw, 'MSH stays');
    // Without a preview of the current text nothing is cut.
    d.raw = raw + 'X';
    ctx.removeDraftSegment(1);
    assert.equal(d.raw, raw + 'X');
});

// The Segments tab of a draft, rendered into a fake detail panel.
function segmentsTab(ctx, state, byId, d, message) {
    state.activeTab = 'parsed';
    const content = byId('detail-content');
    ctx.renderDraftSegmentsTab(content, d, { raw: d.raw, positions: [], message });
    return content;
}

const adtMessage = (pid5, warnings = []) => ({
    message_type: 'ADT^A01',
    delimiters: '|^~\\&',
    typical_segments: ['MSH', 'PID', 'NK1'],
    typical_segment_descriptions: {},
    validation_warnings: warnings,
    segments: [
        { name: 'MSH', fields: [{ index: 1, value: '|' }, { index: 2, value: '^~\\&' }, { index: 3, value: 'A' }, { index: 10, value: 'C1' }] },
        { name: 'PID', fields: [{ index: 1, value: '' }, { index: 5, value: pid5 }, { index: 7, value: '19800515' }] },
    ],
});

test('after a write the tab is updated in place: the focused input keeps what was typed (#291 review)', () => {
    const { ctx, state, byId } = setup();
    const d = ctx.addDraft({ raw: 'MSH|^~\\&|A\nPID|||1||' });
    state.selection = { kind: 'draft', id: d.id };
    const missing = [{ code: 'MISSING_FIELD', segment: 'PID', field: 5, message: 'PID-5 is required' }];
    const content = segmentsTab(ctx, state, byId, d, adtMessage('', missing));
    const input = (path) => content.querySelector(`.field-input[data-field-path="${path}"]`);
    assert.ok(input('PID-5'), 'a required empty field is listed');
    assert.ok(content.querySelector('tr[data-label="PID-5"]').classList.contains('warn'));
    assert.equal(content.querySelectorAll('.field-fixed').length, 2, 'MSH-1/MSH-2 read-only');
    assert.ok(content.querySelector('.seg-pill[data-seg="NK1"]').classList.contains('addable'));

    // PID-5 was written; meanwhile the user moved to PID-7 and typed.
    const pid7 = input('PID-7');
    pid7.value = '1980051X';
    ctx.document.activeElement = pid7;
    const pid5 = input('PID-5');
    ctx.refreshDraftSegmentsInPlace('PID-5', 'Doe^Jane', { raw: d.raw, positions: [], message: adtMessage('Doe^Jane') });
    assert.equal(input('PID-7'), pid7, 'the same element: not re-rendered');
    assert.equal(pid7.value, '1980051X', 'what was typed survives');
    assert.equal(input('PID-5'), pid5);
    assert.equal(pid5.value, 'Doe^Jane');
    assert.equal(pid5.dataset.written, 'Doe^Jane');
    assert.ok(!content.querySelector('tr[data-label="PID-5"]').classList.contains('warn'), 'the mark is gone');

    // The focused input itself is not overwritten while the user types on.
    pid7.value = '19800516';
    ctx.refreshDraftSegmentsInPlace('PID-7', '1980051X', { raw: d.raw, positions: [], message: adtMessage('Doe^Jane') });
    assert.equal(pid7.value, '19800516');
    assert.equal(pid7.dataset.written, '1980051X');

    // Another list of segments re-renders.
    const more = adtMessage('Doe^Jane');
    more.segments.push({ name: 'NK1', fields: [{ index: 1, value: '1' }] });
    ctx.refreshDraftSegmentsInPlace('PID-5', 'Doe^Jane', { raw: d.raw, positions: [], message: more });
    assert.equal(content.querySelectorAll('.segment-block').length, 3);
    assert.notEqual(input('PID-7'), pid7);
});

test('a field write never overwrites a change made meanwhile (#291 review)', async () => {
    let answer;
    const { ctx, state, toasts } = setup({
        responses: { '/api/parse': () => new Promise((resolve) => { answer = resolve; }) },
    });
    for (const f of ['renderDraftRail', 'renderDraftHeader', 'refreshDraftSegmentsInPlace']) ctx[f] = () => {};
    let reverted = null;
    ctx.revertDraftInput = (path) => { reverted = path; };
    const d = ctx.addDraft({ raw: 'MSH|^~\\&|A\nPID|||1||Old' });
    state.selection = { kind: 'draft', id: d.id };
    const writing = ctx.writeDraftFieldNow('PID-5', 'New');
    d.raw = 'MSH|^~\\&|A\nPID|||1||Old\nPV1||I'; // typed in the Raw tab meanwhile
    answer({ ok: true, json: async () => previewOf('MSH|^~\\&|A\nPID|||1||New', {}), text: async () => '' });
    assert.equal(await writing, false);
    assert.equal(d.raw, 'MSH|^~\\&|A\nPID|||1||Old\nPV1||I', 'the change made meanwhile stays');
    assert.equal(reverted, 'PID-5');
    assert.match(toasts.at(-1)[0], /changed meanwhile/);
});

test('a missing segment\'s pill keeps its mark when it becomes addable (#291 review)', () => {
    const { ctx, state, byId } = setup();
    const d = ctx.addDraft({ raw: 'MSH|1' });
    state.selection = { kind: 'draft', id: d.id };
    const msg = adtMessage('x', [{ code: 'MISSING_SEGMENT', segment: 'NK1', message: 'NK1 is expected' }]);
    const content = segmentsTab(ctx, state, byId, d, msg);
    assert.equal(content.querySelector('.seg-pill[data-seg="NK1"]').textContent, '+ NK1 \u2715');
});

// --- Templates (#279) ---

test('the New message menu lists the templates by group', async () => {
    const { ctx, state, byId } = setup({
        responses: {
            '/api/templates': okJson([
                { id: 'builtin:adt_a01', name: 'ADT^A01 Admit', group: 'Built in', message_type: 'ADT^A01' },
                { id: 'site:lab.hl7', name: 'lab <b>', group: 'Site', message_type: 'ORU^R01' },
            ]),
        },
    });
    ctx.renderNewMessageButton = () => {};
    ctx.toggleNewMenu(true);
    await new Promise((r) => setImmediate(r));
    const html = byId('new-menu-templates').innerHTML;
    assert.match(html, /Templates<\/div><button role="menuitem" data-action="new-template" data-template-id="builtin:adt_a01">ADT\^A01 Admit<\/button>/);
    assert.match(html, /Site templates<\/div>.*data-template-id="site:lab.hl7">lab &lt;b&gt; <span class="hint">ORU\^R01<\/span>/);
    assert.equal(state.templates.length, 2);
});

test('a template becomes a draft on its Segments tab; the send names the template', async () => {
    const { ctx, state, requests } = setup({
        responses: {
            '/api/templates/': okJson({ id: 'site:a b.hl7', name: 'Site ADT', group: 'Site', message_type: 'ADT^A08', raw: 'MSH|^~\\&|S|X|||{{now}}||ADT^A08|{{id}}|T|2.5', encoding: 'windows-1252', charset: '8859/1', warning: null }),
            '/api/send': okJson({ id: 's', delivery: { state: 'acked', ack_code: 'AA' } }),
        },
    });
    let opened = null;
    ctx.openDraft = (id, tab) => { opened = [id, tab]; state.selection = { kind: 'draft', id }; };
    ctx.renderDraftHeader = () => {};
    const d = await ctx.newDraftFromTemplate('site:a b.hl7');
    assert.equal(requests[0].url, '/api/templates/site%3Aa%20b.hl7', 'the ID is encoded, never a path');
    assert.deepEqual(opened, [d.id, 'parsed'], 'fill it out field by field');
    assert.equal(d.charset, 'windows-1252');
    assert.equal(d.type, 'ADT^A08');
    assert.match(ctx.draftOriginHtml(d), /^From the template Site ADT$/);
    await ctx.sendDraft(d, ctx.loopbackTarget());
    const item = JSON.parse(requests.find((r) => r.url === '/api/send').opts.body).items[0];
    assert.deepEqual(item, { raw: d.raw, charset: 'windows-1252', template: 'Site ADT' });
    const html = ctx.sentDescriptionHtml({ outbound: { target: 't', origin: { kind: 'template', name: 'Site <ADT>' } } });
    assert.equal(html, 'Sent to t · from the template Site &lt;ADT&gt;');
});

test('Repeat (#280): the draft n times in one run, asked first; noted when the run ends', async () => {
    const run = { id: 'run-7', target: 'This Harux', total: 3, done: 0, state: 'running', codes: {}, written: 0, failed: 0, avg_latency_ms: null, reason: null };
    const { ctx, state, requests, confirms, toasts } = setup({
        responses: { '/api/send': async () => ({ ok: true, status: 202, json: async () => run, text: async () => '' }) },
    });
    ctx.renderDraftHeader = () => {};
    state.sendConfig.max_run_messages = 1000;
    const d = ctx.addDraft({ raw: 'MSH|^~\\&|A|B|||{{now}}||ADT^A01|X{{n}}|P|2.5' });
    state.selection = { kind: 'draft', id: d.id };
    assert.equal(d.repeat, 1);
    ctx.setDraftRepeat('3');
    assert.equal(d.repeat, 3);
    assert.match(ctx.draftActionsHtml(d), /Send 3×/);
    assert.match(ctx.draftToolsHtml(d), /id="draft-repeat"[^>]*value="3"/);

    const target = ctx.loopbackTarget();
    assert.equal(await ctx.sendDraft(d, target), true);
    assert.match(confirms[0], /Send this draft 3 times to This Harux \(127\.0\.0\.1:2575\)/);
    const body = JSON.parse(requests.find((r) => r.url === '/api/send').opts.body);
    assert.equal(body.repeat, 3);
    assert.deepEqual(plain(body.items), [{ raw: d.raw, charset: 'auto' }]);
    assert.equal(state.run.id, 'run-7');
    assert.equal(d.sends.length, 0, 'noted when it has ended');

    // The run ends: one entry in the draft's history, no link (its rows are in the list).
    state.run.status = { ...run, done: 3, state: 'done', codes: { AA: 3 }, avg_latency_ms: 4 };
    ctx.finishRun(state.run);
    assert.deepEqual(plain(d.sends.map((s) => [s.run, s.done, s.total, s.ok, s.summary])), [['run-7', 3, 3, true, '3 AA · avg 4 ms']]);
    assert.equal(ctx.sendChip(d.sends[0]).text, '3 / 3');
    assert.match(ctx.sendFacts(d.sends[0]), /run to This Harux · 3 AA/);
    assert.match(toasts.at(-1)[0], /: Done · 3 \/ 3/);

    // Out of range: kept within 1 … the run limit.
    ctx.setDraftRepeat('0');
    assert.equal(d.repeat, 1);
    ctx.setDraftRepeat('5000');
    assert.equal(d.repeat, 1000);
    ctx.setDraftRepeat('abc');
    assert.equal(d.repeat, 1);
});

test('Repeat: a target that asks before sending takes no runs', async () => {
    const { ctx, state, requests, confirms, toasts } = setup();
    ctx.renderDraftHeader = () => {};
    const d = ctx.addDraft({ raw: 'MSH|1' });
    state.selection = { kind: 'draft', id: d.id };
    d.repeat = 4;
    const prod = ctx.normalizeTarget({ name: 'prod', host: 'prod.local', port: 6661, ask: true });
    assert.equal(await ctx.sendDraft(d, prod), false);
    assert.equal(confirms.length, 0);
    assert.equal(requests.length, 0);
    assert.match(toasts.at(-1)[0], /takes no runs/);
});

test('From a file (#281): an export is loaded into the list, one message opens as a draft', async () => {
    const summary = { source: 'import:x.hl7', imported: 3, parse_errors: 1, truncated: 0, bytes: 99, skipped_bytes: 0 };
    const { ctx, state, requests, confirms, toasts } = setup({
        responses: {
            '/api/import': async () => ({ ok: true, status: 200, json: async () => summary, text: async () => '' }),
            '/api/parse': okJson({ raw: 'MSH|^~\\&|A', encoding: 'UTF-8', warning: null, framed: true }),
        },
    });
    ctx.openDraft = () => {};
    state.sendConfig.max_message_bytes = 1024;
    const enc = (s) => new TextEncoder().encode(s);
    const file = (name, bytes) => ({
        name,
        size: bytes.length,
        slice: (a, b) => ({ arrayBuffer: async () => bytes.slice(a, b).buffer }),
        arrayBuffer: async () => bytes.slice().buffer,
    });
    const frame = (t) => `\x0bMSH|^~\\&|A|B|||1||ADT^A01|${t}|P|2.5\r\x1c\r`;

    assert.equal(ctx.isFramedHead(enc('\r\n\x0bMSH')), true);
    assert.equal(ctx.isFramedHead(enc('MSH|')), false);
    assert.equal(ctx.countFrames(enc(frame('A') + frame('B'))), 2);

    // Three frames: asked, then streamed to /api/import with the file's name.
    const three = file('x y.hl7', enc(frame('A') + frame('B') + frame('C')));
    assert.deepEqual(plain(await ctx.openFile(three)), summary);
    assert.match(confirms[0], /Load the 3 messages in x y\.hl7 into the list, as received from import:x y\.hl7/);
    const imp = requests.find((r) => r.url.startsWith('/api/import'));
    assert.equal(imp.url, '/api/import?name=x%20y.hl7');
    assert.equal(imp.opts.body, three, 'the file itself: the browser streams it');
    assert.match(toasts.at(-1)[0], /^Loaded 3 messages from x y\.hl7 · 1 did not parse$/);
    assert.equal(state.drafts.length, 0);

    // One frame, or none: a draft, as before.
    await ctx.openFile(file('one.hl7', enc(frame('A'))));
    await ctx.openFile(file('plain.hl7', enc('MSH|^~\\&|A')));
    assert.equal(requests.filter((r) => r.url.startsWith('/api/import')).length, 1);
    assert.equal(state.drafts.length, 2);

    // Larger than one message can be: an import without counting.
    const big = file('big.hl7', enc(frame('A') + 'x'.repeat(2000)));
    await ctx.openFile(big);
    assert.match(confirms.at(-1), /Load the messages in big\.hl7/);
    assert.equal(ctx.importSummaryText('f', { imported: 1, truncated: 1, skipped_bytes: 7 }), 'Loaded 1 message from f · 1 above the size limit, kept cut off · 7 bytes outside frames not loaded');
});
