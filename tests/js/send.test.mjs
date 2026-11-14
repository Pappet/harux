// Tests for resending from the UI (#274): the user's own target list in
// static/send.js (add, change, remove, a reload, storage that fails, a file
// that is not a list), *ask before sending*, the request it makes, one-click
// resend, and the direction filter in static/render.js. state.js, util.js,
// render.js and send.js run in a `node:vm` context.
//
//   node --test 'tests/js/*.test.mjs'
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { webcrypto } from 'node:crypto';
import vm from 'node:vm';
import { El } from './fake-dom.mjs';

const read = (f) => readFileSync(new URL(`../../static/${f}`, import.meta.url), 'utf8');

// A localStorage that can be made to fail.
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

function setup({ storage = memoryStorage(), confirmAnswer = true, crypto = webcrypto } = {}) {
    const noop = () => {};
    const toasts = [];
    const requests = [];
    const confirms = [];
    const ctx = {
        window: {},
        console: { ...console, debug: noop, info: noop },
        document: { getElementById: () => null, createElement: () => new El() },
        sessionStorage: { getItem: () => null, setItem: noop },
        localStorage: storage,
        crypto,
        setTimeout: noop,
        WeakMap, Set, Map, JSON, Uint8Array, Array,
        confirm: (q) => { confirms.push(q); return confirmAnswer; },
        fetch: async (url, opts) => {
            requests.push({ url, opts });
            return {
                ok: true,
                json: async () => ({ id: 'sent-1', delivery: { state: 'acked', ack_code: 'AA', latency_ms: 42 } }),
                text: async () => '',
            };
        },
    };
    vm.createContext(ctx);
    for (const f of ['state.js', 'util.js', 'render.js', 'send.js']) vm.runInContext(read(f), ctx);
    Object.assign(ctx, {
        showToast: (m, type) => toasts.push([m, type]),
        renderDetail: noop,
        renderMessageList: noop,
    });
    const state = ctx.window.state;
    state.sendConfig = { enabled: true, loopback: { name: 'This Harux', host: '127.0.0.1', port: 2575 }, tls: { allow_unverified: false } };
    return { ctx, state, storage, toasts, requests, confirms };
}

const plain = (v) => JSON.parse(JSON.stringify(v));
const msg = { id: 'm1', message_type: 'ADT^A01' };

test('newId works without a secure context (no crypto.randomUUID)', () => {
    const { ctx } = setup({ crypto: { getRandomValues: (a) => webcrypto.getRandomValues(a) } });
    const id = ctx.newId();
    assert.match(id, /^[0-9a-f]{32}$/);
    assert.notEqual(id, ctx.newId());
});

test('a target is normalized: TLS on by default, a name from the address, bad ports refused', () => {
    const { ctx } = setup();
    const t = ctx.normalizeTarget({ host: ' orchestra.local ', port: '6661' });
    assert.deepEqual({ ...t, id: undefined }, { id: undefined, name: 'orchestra.local:6661', host: 'orchestra.local', port: 6661, tls: true, verify: true, ask: false });
    assert.equal(ctx.normalizeTarget({ host: 'h', port: 6661, tls: false }).tls, false);
    for (const bad of [{ host: '', port: 1 }, { host: 'a b', port: 1 }, { host: 'h', port: 0 }, { host: 'h', port: 70000 }, { host: 'h', port: 1.5 }, null, 'x']) {
        assert.equal(ctx.normalizeTarget(bad), null, JSON.stringify(bad));
    }
});

test('targets: add, change, remove, and a reload sees the list', () => {
    const { ctx, state, storage } = setup();
    const a = ctx.upsertTarget(ctx.normalizeTarget({ name: 'test', host: 'h1', port: 1 }));
    ctx.upsertTarget(ctx.normalizeTarget({ name: 'prod', host: 'h2', port: 2, ask: true }));
    ctx.upsertTarget({ ...a, port: 11 });
    assert.deepEqual(plain(state.targets.map((t) => [t.name, t.port])), [['test', 11], ['prod', 2]]);

    // A new page with the same storage: the list is back.
    const again = setup({ storage });
    again.ctx.loadTargets();
    assert.deepEqual(plain(again.state.targets.map((t) => t.name)), ['test', 'prod']);

    again.state.lastTargetId = a.id;
    again.ctx.removeTarget(a.id);
    assert.deepEqual(plain(again.state.targets.map((t) => t.name)), ['prod']);
    assert.equal(again.state.lastTargetId, null, 'a removed target is no longer the one-click target');
    // "This Harux" is always offered and never stored.
    assert.deepEqual(plain(again.ctx.allTargets().map((t) => t.name)), ['prod', 'This Harux']);
    assert.ok(!storage.data.get('harux_targets').includes('This Harux'));
});

test('storage that fails leaves the list usable for the session and says so once', () => {
    const storage = memoryStorage();
    storage.broken = true;
    const { ctx, state, toasts } = setup({ storage });
    ctx.loadTargets(); // reading fails too: an empty list, no exception
    assert.equal(state.targets.length, 0);
    ctx.upsertTarget(ctx.normalizeTarget({ name: 'a', host: 'h', port: 1 }));
    ctx.upsertTarget(ctx.normalizeTarget({ name: 'b', host: 'h', port: 2 }));
    assert.equal(state.targets.length, 2);
    assert.equal(toasts.length, 1, 'one warning, not one per write');
    assert.match(toasts[0][0], /Save to file/);
});

test('a file that is not a target list is refused with a reason', () => {
    const { ctx } = setup();
    assert.throws(() => ctx.parseTargetFile('not json'), /not JSON/);
    assert.throws(() => ctx.parseTargetFile('{"foo": 1}'), /no target list/);
    assert.throws(() => ctx.parseTargetFile('[{"host": "h", "port": 1}, {"host": "", "port": 2}]'), /Entry 2/);
    const saved = JSON.stringify({ harux_targets: 1, targets: [{ id: 'x', name: 'n', host: 'h', port: 5, tls: false }] });
    assert.deepEqual(plain(ctx.parseTargetFile(saved)), [{ id: 'x', name: 'n', host: 'h', port: 5, tls: false, verify: true, ask: false }]);
    assert.equal(ctx.parseTargetFile('[{"host": "h", "port": 1}]').length, 1, 'a bare array works too');
});

test('the request names the target; loopback by name', async () => {
    const { ctx, state, requests, toasts } = setup();
    const tls = ctx.upsertTarget(ctx.normalizeTarget({ name: 'orch', host: 'o.local', port: 6661 }));
    assert.equal(await ctx.resendMessage(msg, tls), true);
    const body = JSON.parse(requests[0].opts.body);
    assert.deepEqual(body, {
        target: { name: 'orch', host: 'o.local', port: 6661, tls: true, verify: true },
        items: [{ message_id: 'm1' }],
        allow_invalid: false,
    });
    assert.equal(requests[0].opts.method, 'POST');
    assert.equal(state.lastTargetId, tls.id, 'remembered for one-click resend');
    assert.equal(toasts.at(-1)[0], 'Sent to orch · AA · 42 ms');
    assert.equal(state.targetResults[tls.id].code, 'AA');

    await ctx.resendMessage(msg, ctx.loopbackTarget());
    assert.equal(JSON.parse(requests[1].opts.body).target, 'loopback');
    const plainTarget = ctx.targetRequest(ctx.normalizeTarget({ host: 'h', port: 1, tls: false }));
    assert.equal(plainTarget.verify, undefined, 'verify only with TLS');
});

test('ask before sending: asks every time, and a "no" sends nothing', async () => {
    const no = setup({ confirmAnswer: false });
    const prod = no.ctx.upsertTarget(no.ctx.normalizeTarget({ name: 'prod', host: 'p', port: 1, ask: true }));
    assert.equal(await no.ctx.resendMessage(msg, prod), false);
    assert.equal(no.requests.length, 0);
    assert.match(no.confirms[0], /^Send ADT\^A01 to prod \(p:1\)\?$/);

    const yes = setup();
    const p2 = yes.ctx.upsertTarget(yes.ctx.normalizeTarget({ name: 'prod', host: 'p', port: 1, ask: true }));
    await yes.ctx.resendMessage(msg, p2);
    await yes.ctx.resendMessage(msg, p2);
    assert.equal(yes.confirms.length, 2, 'every time, not once');
    assert.equal(yes.requests.length, 2);
});

test('a message that did not parse is sent only after a second confirmation', async () => {
    const { ctx, requests, confirms } = setup();
    const t = ctx.upsertTarget(ctx.normalizeTarget({ host: 'h', port: 1 }));
    await ctx.resendMessage({ id: 'bad', parse_error: 'Message does not start with MSH' }, t);
    assert.match(confirms[0], /did not parse/);
    assert.equal(JSON.parse(requests[0].opts.body).allow_invalid, true);
});

test('one click: the target used last; the first use opens the menu', async () => {
    const { ctx, state, requests } = setup();
    let opened = 0;
    ctx.openSendMenu = () => { opened++; };
    state.selectedMessage = msg;
    ctx.resendOneClick();
    assert.equal(opened, 1);
    assert.equal(requests.length, 0);

    state.lastTargetId = 'loopback';
    ctx.resendOneClick();
    await new Promise((r) => setImmediate(r));
    assert.equal(requests.length, 1);
    assert.equal(JSON.parse(requests[0].opts.body).target, 'loopback');
});

test('nothing to resend while sending is off, or for a cut-off entry', () => {
    const { ctx, state } = setup();
    assert.equal(ctx.canResend(msg), true);
    assert.equal(ctx.canResend({ ...msg, truncated: true }), false);
    assert.equal(ctx.sendActionsHtml({ ...msg, truncated: true }), '');
    state.sendConfig = null;
    assert.equal(ctx.canResend(msg), false);
    assert.equal(ctx.sendActionsHtml(msg), '', 'no button while sending is off (D11)');
});

test('the menu shows each target\'s last result', () => {
    const { ctx } = setup();
    const now = Date.parse('2026-11-13T12:00:00Z');
    assert.deepEqual(plain(ctx.describeTargetResult(null, now)), { text: 'not used yet', when: '', cls: '' });
    assert.deepEqual(plain(ctx.describeTargetResult({ state: 'acked', code: 'AA', ms: 42, at: now - 120_000 }, now)),
        { text: 'AA · 42 ms', when: '2m ago', cls: 'ok' });
    assert.equal(ctx.describeTargetResult({ state: 'acked', code: 'AE', ms: 5, at: now }, now).cls, 'bad');
    assert.deepEqual(plain(ctx.describeTargetResult({ state: 'failed', error: 'refused', at: now - 26 * 3600_000 }, now)),
        { text: 'refused', when: 'yesterday', cls: 'bad' });
});

test('direction filter: In + Out, Received, Sent; only In + Out prepends', () => {
    const { ctx, state } = setup();
    const inbound = { direction: 'in' };
    const outbound = { direction: 'out' };
    const legacy = {}; // a summary from before #272 has no direction
    state.directionFilter = 0;
    assert.ok(ctx.canPrependOnly());
    assert.ok([inbound, outbound, legacy].every(ctx.matchesDirection));
    state.directionFilter = 1;
    assert.ok(!ctx.canPrependOnly());
    assert.deepEqual([inbound, outbound, legacy].map(ctx.matchesDirection), [true, false, true]);
    state.directionFilter = 2;
    assert.deepEqual([inbound, outbound, legacy].map(ctx.matchesDirection), [false, true, false]);
});

// --- #286 review ---

test('newId works without any Web Crypto at all', () => {
    const { ctx } = setup({ crypto: null });
    assert.match(ctx.newId(), /^[0-9a-f]{32}$/);
});

test('target names and addresses are escaped in the menu (lists come from files)', () => {
    const { ctx } = setup();
    // esc() needs a real DOM; this escapes the same five characters.
    const escape = (v) => String(v ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#x27;');
    ctx.esc = escape;
    ctx.escAttr = escape;
    const evil = ctx.normalizeTarget({ id: 'x"><img src=y>', name: '<img src=x onerror=alert(1)>', host: 'h"><b>', port: 1 });
    assert.ok(evil, 'odd but harmless names are allowed — they are only shown');
    const html = ctx.targetOptionHtml(evil, false, Date.now());
    assert.ok(!/<img|<b>/.test(html), html);
    assert.match(html, /&lt;img src=x onerror=alert\(1\)&gt;/);
    assert.match(html, /data-target-id="x&quot;&gt;&lt;img src=y&gt;"/);
});

test('the menu starts with the form only when there is no target at all', () => {
    const { ctx, state } = setup();
    ctx.renderSendMenu = () => {};
    ctx.updateSendButtons = () => {};
    state.selectedMessage = msg;
    ctx.openSendMenu();
    assert.equal(state.sendMenu.form, null, 'This Harux is a target: no empty form');
    assert.equal(state.sendMenu.pickedId, 'loopback');
    state.sendConfig = { enabled: true, loopback: null, tls: {} };
    ctx.openSendMenu();
    assert.ok(state.sendMenu.form, 'nothing to pick: the form opens');
});

test('a port typed into the host field is refused with a hint', () => {
    const { ctx } = setup();
    for (const bad of ['orchestra.local:6661', '10.0.0.1:2575', '[::1]:2575', 'h:']) {
        assert.equal(ctx.normalizeTarget({ host: bad, port: 1 }), null, bad);
        assert.ok(ctx.hostHasPort(bad), bad);
    }
    assert.ok(ctx.normalizeTarget({ host: '::1', port: 2575 }), 'IPv6 addresses stay allowed');
    assert.ok(!ctx.hostHasPort('fe80::1'));
});

test('results of removed targets are forgotten', () => {
    const { ctx, state, storage } = setup();
    const a = ctx.upsertTarget(ctx.normalizeTarget({ host: 'a', port: 1 }));
    ctx.recordTargetResult(a.id, { state: 'acked', ack_code: 'AA' });
    ctx.recordTargetResult('loopback', { state: 'acked', ack_code: 'AA' });
    ctx.removeTarget(a.id);
    assert.deepEqual(Object.keys(state.targetResults), ['loopback']);
    assert.ok(!storage.data.get('harux_target_results').includes(a.id));
});

// --- Send as (#275) ---

test('send as: the way is part of the request and remembered', async () => {
    const { ctx, state, requests, storage } = setup();
    state.selectedMessage = msg;
    ctx.updateSendButtons = () => {};
    ctx.chooseSendAs('new_id_time');
    await ctx.resendMessage(msg, ctx.loopbackTarget());
    assert.deepEqual(JSON.parse(requests[0].opts.body).set, { 'MSH-10': '{{id}}', 'MSH-7': '{{now}}' });

    const again = setup({ storage });
    again.ctx.loadTargets();
    assert.equal(again.state.sendAs, 'new_id_time', 'remembered across a reload');

    ctx.chooseSendAs('unchanged');
    await ctx.resendMessage(msg, ctx.loopbackTarget());
    assert.equal(JSON.parse(requests[1].opts.body).set, undefined, 'as received: no `set` at all');
    ctx.chooseSendAs('nonsense');
    assert.equal(state.sendAs, 'unchanged');
});

test('send as: a message that did not parse goes out only as received', async () => {
    const { ctx, state, requests, toasts } = setup();
    state.sendAs = 'new_id';
    assert.equal(await ctx.resendMessage({ id: 'bad', parse_error: 'x' }, ctx.loopbackTarget()), false);
    assert.equal(requests.length, 0);
    assert.match(toasts[0][0], /only be resent as received/);
});

test('two tabs share the target list: a save keeps the other tab\'s targets, a removal stays removed (#300)', () => {
    const storage = memoryStorage();
    const a = setup({ storage });
    const b = setup({ storage });
    a.ctx.loadTargets();
    b.ctx.loadTargets();
    const ta = a.ctx.upsertTarget(a.ctx.normalizeTarget({ name: 'from A', host: 'h1', port: 1 }));
    // Tab B never heard of it and saves its own.
    b.ctx.upsertTarget(b.ctx.normalizeTarget({ name: 'from B', host: 'h2', port: 2 }));
    const names = () => plain(JSON.parse(storage.data.get('harux_targets')).map((t) => t.name));
    assert.deepEqual(names(), ['from A', 'from B'], 'B\'s save kept A\'s target');
    assert.deepEqual(plain(b.state.targets.map((t) => t.name)), ['from A', 'from B'], 'and B shows it');

    // A removes its target; B saves again without having heard of that.
    a.ctx.syncTargets(storage.data.get('harux_targets'));
    a.ctx.removeTarget(ta.id);
    b.ctx.upsertTarget(b.ctx.normalizeTarget({ name: 'B again', host: 'h3', port: 3 }));
    assert.deepEqual(names(), ['from B', 'B again'], 'the removed target does not come back');
    assert.deepEqual(plain(b.state.targets.map((t) => t.name)), ['from B', 'B again']);

    // The storage event: A takes the list as it is now.
    a.ctx.syncTargets(storage.data.get('harux_targets'));
    assert.deepEqual(plain(a.state.targets.map((t) => t.name)), ['from B', 'B again']);
});

test('a target file this browser saved loads back without losing the list (#312 review)', async () => {
    const { ctx, state, storage, toasts } = setup();
    ctx.loadTargets();
    ctx.upsertTarget(ctx.normalizeTarget({ name: 'one', host: 'h1', port: 1 }));
    ctx.upsertTarget(ctx.normalizeTarget({ name: 'two', host: 'h2', port: 2 }));
    const saved = JSON.stringify({ targets: plain(state.targets) }); // what "Save to file" writes, IDs included
    // The real loadTargetsFromFile with a file input that hands over `saved`.
    const load = async (text) => {
        let onChange = null;
        ctx.document.createElement = () => {
            const el = new El();
            el.addEventListener = (type, f) => { if (type === 'change') onChange = f; };
            el.click = () => {};
            el.files = [{ name: 'harux-targets.json', text: async () => text }];
            return el;
        };
        ctx.updateSendButtons = () => {};
        ctx.loadTargetsFromFile();
        await onChange();
    };
    const names = () => plain(JSON.parse(storage.data.get('harux_targets')).map((t) => t.name));
    await load(saved);
    assert.deepEqual(plain(state.targets.map((t) => t.name)), ['one', 'two']);
    assert.deepEqual(names(), ['one', 'two'], 'the stored list survives');
    await load(saved); // the same file twice
    assert.deepEqual(names(), ['one', 'two']);
    assert.match(toasts.at(-1)[0], /^Loaded 2 target/);
    // A file without one of them removes it, and it stays removed.
    await load(JSON.stringify({ targets: [plain(state.targets[1])] }));
    assert.deepEqual(names(), ['two']);
});

test('a merge over the target limit keeps this tab\'s targets and says so (#312 review)', () => {
    const storage = memoryStorage();
    const a = setup({ storage });
    const b = setup({ storage });
    a.ctx.loadTargets();
    b.ctx.loadTargets();
    for (let i = 0; i < 199; i++) b.ctx.upsertTarget(b.ctx.normalizeTarget({ name: `b${i}`, host: 'h', port: i + 1 }));
    a.ctx.syncTargets(storage.data.get('harux_targets')); // A sees B's 199
    // At the same moment both tabs add the 200th: B first, A without having heard of it.
    b.ctx.upsertTarget(b.ctx.normalizeTarget({ name: 'b-last', host: 'h', port: 999 }));
    a.ctx.upsertTarget(a.ctx.normalizeTarget({ name: 'a-new', host: 'a', port: 1 }));
    const stored = plain(JSON.parse(storage.data.get('harux_targets')).map((t) => t.name));
    assert.equal(stored.length, 200);
    assert.ok(stored.includes('a-new'), 'the target added here stays');
    assert.ok(!stored.includes('b-last'), 'the other tab\'s newest is left out');
    assert.match(a.toasts.at(-1)[0], /At most 200 targets/);
});
