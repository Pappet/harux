// Tests for the list (re)load in static/ws.js (#231, #239). state.js and
// ws.js run in a `node:vm` context; rendering is stubbed, and `fetch` is
// controlled by the test so a WebSocket event can arrive while the list
// request is still in flight.
//
//   node --test 'tests/js/*.test.mjs'
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = (f) => readFileSync(new URL(`../../static/${f}`, import.meta.url), 'utf8');

function setup() {
    const requests = [];
    const noop = () => {};
    const ctx = {
        window: {},
        console: { ...console, info: noop, warn: noop },
        document: { getElementById: () => ({ innerHTML: '' }) },
        setTimeout: noop, // no render flush: inspect state.pendingMessages
        WeakMap,
        Set,
        Map,
        CLIENT_MESSAGE_CAP: 2000,
        DETAIL_EMPTY_HTML: '',
        detailJsonCache: new WeakMap(),
        srcLabelFor: (addr) => addr || '',
        registerSource: (addr) => { if (addr) ctx.window.state.seenSources.add(addr); },
        recomputeSourceCounts: noop,
        renderMessageList: noop,
        renderSourceLegend: noop,
        renderHealthPills: noop,
        updateHeaderCounters: noop,
        resetDetailHeader: noop,
        patchRow: noop,
        renderDetail: noop,
        removeRowsFromList: noop,
        showToast: noop,
        saveSession: noop,
        selectMessage: noop,
        prependMessagesToList: noop,
        canPrependOnly: () => true,
        // Each fetch returns a promise the test resolves via `respond`.
        fetch: (url) => new Promise((resolve) => requests.push({ url, resolve })),
    };
    vm.createContext(ctx);
    vm.runInContext(read('state.js'), ctx);
    vm.runInContext(read('ws.js'), ctx);
    const state = ctx.window.state;
    const respond = (i, list) => requests[i].resolve({ ok: true, json: async () => list });
    const held = () => [...state.pendingMessages, ...state.messages].map((m) => m.id);
    return { ctx, state, requests, respond, held };
}

const summary = (id, extra = {}) => ({ id, source_addr: '10.0.0.1:5000', tags: [], bookmarked: false, ...extra });
const event = (type, data) => ({ type, data });
const tick = () => new Promise((r) => setImmediate(r));

test('a message arriving while the list loads is kept (#239)', async () => {
    const { ctx, respond, held } = setup();
    const load = ctx.loadMessages();
    ctx.handleServerEvent(event('new_message', summary('X')));
    respond(0, [summary('B'), summary('A')]); // snapshot taken before X arrived
    await load;
    assert.deepEqual(held().sort(), ['A', 'B', 'X']);
});

test('a message both in the snapshot and in the queue is not doubled', async () => {
    const { ctx, respond, held } = setup();
    const load = ctx.loadMessages();
    ctx.handleServerEvent(event('new_message', summary('B')));
    respond(0, [summary('B'), summary('A')]); // snapshot taken after B arrived
    await load;
    assert.deepEqual(held().sort(), ['A', 'B']);
});

test('tag and bookmark updates during the load are applied after it', async () => {
    const { ctx, state, respond } = setup();
    const load = ctx.loadMessages();
    ctx.handleServerEvent(event('tags_updated', summary('A', { tags: ['Bug 1'] })));
    ctx.handleServerEvent(event('bookmark_toggled', summary('A', { bookmarked: true })));
    respond(0, [summary('A')]); // older snapshot without the tag/bookmark
    await load;
    assert.deepEqual([...state.messages[0].tags], ['Bug 1']);
    assert.equal(state.messages[0].bookmarked, true);
    assert.equal(state.totalBookmarkCount, 1);
});

test('a clear during the load wins over the older snapshot', async () => {
    const { ctx, respond, held } = setup();
    const load = ctx.loadMessages();
    ctx.handleServerEvent(event('cleared'));
    ctx.handleServerEvent(event('new_message', summary('C')));
    respond(0, [summary('A')]);
    await load;
    assert.deepEqual(held(), ['C']);
});

test('the load asks for bookmarks beyond the page (#231)', async () => {
    const { ctx, state, requests, respond } = setup();
    const load = ctx.loadMessages();
    assert.match(requests[0].url, /include_bookmarks=true/);
    respond(0, [summary('NEW'), summary('OLD', { bookmarked: true })]);
    await load;
    assert.equal(state.totalBookmarkCount, 1);
});

test('a superseded load does not overwrite the newer one', async () => {
    const { ctx, state, respond, held } = setup();
    const first = ctx.loadMessages();
    const second = ctx.loadMessages(); // e.g. `lagged` during the initial load
    ctx.handleServerEvent(event('new_message', summary('Q')));
    respond(1, [summary('N2'), summary('N1')]);
    await second;
    respond(0, [summary('STALE')]);
    await first;
    await tick();
    assert.deepEqual(held().sort(), ['N1', 'N2', 'Q']);
    assert.equal(state.loadInProgress, false);
    assert.equal(state.loadQueue.length, 0);
});

test('outside a load, events apply immediately', () => {
    const { ctx, state, held } = setup();
    ctx.handleServerEvent(event('new_message', summary('Z')));
    assert.deepEqual(held(), ['Z']);
    assert.equal(state.loadQueue.length, 0);
});
