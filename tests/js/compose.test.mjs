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
        document: { getElementById: byId, createElement: make, querySelectorAll: () => [], activeElement: null },
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
