// Tests for sent messages in the list (#272): the delivery chip, the
// `is:sent` / `is:received` search, the peer key (a target is never a
// sender), and `delivery_updated` — before the flush and during a list load.
// state.js, util.js, ws.js and render.js run in a `node:vm` context; util.js
// is loaded for real, with stubs for the browser globals it touches.
//
//   node --test 'tests/js/*.test.mjs'
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = (f) => readFileSync(new URL(`../../static/${f}`, import.meta.url), 'utf8');

function setup() {
    const noop = () => {};
    const requests = [];
    const patched = [];
    const ctx = {
        window: {},
        console: { ...console, info: noop, warn: noop, debug: noop },
        document: { getElementById: () => null, createElement: () => ({}) },
        sessionStorage: { getItem: () => null, setItem: noop },
        crypto: { randomUUID: () => 'session' },
        setTimeout: noop, // no render flush: inspect state.pendingMessages
        WeakMap, Set, Map,
        fetch: (url) => new Promise((resolve) => requests.push({ url, resolve })),
    };
    vm.createContext(ctx);
    for (const f of ['state.js', 'util.js', 'ws.js', 'render.js']) vm.runInContext(read(f), ctx);
    // Rendering is not under test here.
    Object.assign(ctx, {
        renderMessageList: noop, renderSourceLegend: noop, updateHeaderCounters: noop,
        prependMessagesToList: noop, removeRowsFromList: noop, renderDetail: noop,
        patchRow: (id, m) => patched.push([id, m]),
        selectMessage: (id) => patched.push(['selected', id]),
    });
    const state = ctx.window.state;
    const respond = (i, list) => requests[i].resolve({ ok: true, json: async () => list });
    return { ctx, state, patched, respond };
}

const received = (id) => ({ id, direction: 'in', source_addr: '10.4.1.22:5000', tags: [], message_type: 'ADT^A01' });
const sent = (id, extra = {}) => ({
    id, direction: 'out', target: 'orchestra-test', source_addr: '10.9.9.9:6661', tags: [],
    message_type: 'ADT^A01', delivery: { state: 'pending' }, ...extra,
});
const event = (type, data) => ({ type, data });

test('the delivery chip: pending, acked, commit codes, failed, sent', () => {
    const { ctx } = setup();
    assert.deepEqual(ctx.deliveryChip({ state: 'pending' }).text, '…');
    assert.deepEqual(ctx.deliveryChip(undefined).text, '…');
    const aa = ctx.deliveryChip({ state: 'acked', ack_code: 'aa' });
    assert.equal(aa.text, 'AA');
    assert.equal(aa.cls, 'msg-ack aa');
    assert.equal(ctx.deliveryChip({ state: 'acked', ack_code: 'CE' }).cls, 'msg-ack ae');
    const failed = ctx.deliveryChip({ state: 'failed', error: 'refused' });
    assert.equal(failed.text, '✕ refused');
    assert.equal(failed.cls, 'msg-ack failed');
    assert.equal(ctx.deliveryChip({ state: 'sent' }).text, 'sent');
    assert.equal(ctx.formatLatency(42), '42 ms');
    assert.equal(ctx.formatLatency(1500), '1.5 s');
    assert.equal(ctx.formatLatency(undefined), '');
});

test('a sent message is keyed by its target, never by its destination', () => {
    const { ctx } = setup();
    const key = ctx.sourceKeyOf(sent('s'));
    assert.equal(key, '→ orchestra-test');
    assert.equal(ctx.srcLabelFor(key), '→ orchestra-test', 'no port split on a target');
    assert.equal(ctx.getSourceColor(key), 'var(--accent)');
    assert.equal(ctx.srcLabelFor(ctx.sourceKeyOf(received('r'))), '10.4.1.22');
});

test('is:sent / is:received filter like SearchQuery, prefixes in any order', () => {
    const { ctx } = setup();
    const match = (q, m) => ctx.matchesSearch(m, ctx.getParsedQuery(q));
    assert.ok(match('is:sent', sent('s')));
    assert.ok(!match('is:sent', received('r')));
    assert.ok(match('is:received', received('r')));
    assert.ok(!match('is:received', sent('s')));
    // The target's name and the destination are a sent message's source.
    assert.ok(match('orchestra', sent('s')));
    assert.ok(match('10.9.9.9', sent('s')));
    assert.ok(match('is:sent orchestra', sent('s')));
    assert.deepEqual(
        { ...ctx.getParsedQuery('has:warnings is:sent foo') },
        { q: 'foo', hasWarnings: true, hasErrors: false, direction: 'out' },
    );
    assert.equal(ctx.getParsedQuery('is:sent has:errors').hasErrors, true);
});

test('sent messages count in the rail by target and stay out of the rate', () => {
    const { ctx, state } = setup();
    ctx.addMessage(received('r1'));
    ctx.addMessage(sent('s1'));
    ctx.addMessage(sent('s2'));
    assert.equal(state.sourceCounts.get('→ orchestra-test'), 2);
    assert.equal(state.sourceCounts.get('10.4.1.22'), 1);
    assert.ok(!state.seenSources.has('10.9.9.9:6661'), 'the destination is no source');
    assert.equal(state.rateWindow.length, 1, 'rate counts received messages only');
    ctx.forgetMessageCounters(sent('s1'));
    assert.equal(state.sourceCounts.get('→ orchestra-test'), 1);
});

test('delivery_updated before the flush updates the pending summary', () => {
    const { ctx, state, patched } = setup();
    ctx.handleServerEvent(event('new_message', sent('s1')));
    const outcome = { state: 'acked', ack_code: 'AA', latency_ms: 9 };
    ctx.handleServerEvent(event('delivery_updated', sent('s1', { delivery: outcome })));
    assert.deepEqual(state.pendingMessages[0].delivery, outcome);
    assert.deepEqual(JSON.parse(JSON.stringify(patched)), [['s1', { delivery: outcome }]]);
    // The open detail panel is fetched again (the event has no response).
    state.selectedId = 's1';
    ctx.handleServerEvent(event('delivery_updated', sent('s1', { delivery: outcome })));
    assert.deepEqual([...patched.at(-1)], ['selected', 's1']);
});

test('delivery_updated during a list load is applied after the snapshot', async () => {
    const { ctx, state, respond } = setup();
    const load = ctx.loadMessages();
    const outcome = { state: 'failed', error: 'timeout' };
    ctx.handleServerEvent(event('delivery_updated', sent('s1', { delivery: outcome })));
    assert.equal(state.loadQueue.length, 1, 'queued while the list loads');
    respond(0, [sent('s1')]); // snapshot taken while the send was pending
    await load;
    assert.deepEqual(state.messages[0].delivery, outcome);
});
