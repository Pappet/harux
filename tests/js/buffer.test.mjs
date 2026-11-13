// Tests for the client buffer and its incremental counters (#204, #184):
// trimClientBuffer / forgetMessageCounters / removeEvictedMessages /
// updateMessageBookmark in static/ws.js, and removeRowsFromList /
// updateTruncationNote in static/render.js against a minimal fake DOM.
// No jsdom, no npm: state.js, ws.js and render.js run in a `node:vm` context.
//
//   node --test 'tests/js/*.test.mjs'
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = (f) => readFileSync(new URL(`../../static/${f}`, import.meta.url), 'utf8');

import { El } from './fake-dom.mjs';

function setup({ cap = 5 } = {}) {
    const noop = () => {};
    const list = new El('message-list', 'message-list');
    const byId = { 'message-list': list, 'empty-state': new El('', 'empty-state') };
    const ctx = {
        window: {},
        console: { ...console, info: noop, warn: noop, debug: noop },
        document: {
            getElementById: (id) => {
                if (id === 'list-truncated-note') {
                    return list.children.find((c) => c.id === id) || null;
                }
                return byId[id] || { innerHTML: '', classList: { add: noop, remove: noop, toggle: noop } };
            },
            createElement: () => new El(),
        },
        setTimeout: noop,
        WeakMap, Set, Map,
        CLIENT_MESSAGE_CAP: cap,
        OLDER_PAGE_SIZE: 3,
        DETAIL_EMPTY_HTML: '',
        detailJsonCache: new WeakMap(),
        srcLabelFor: (key) => key || '',
        sourceKeyOf: (m) => (m.direction === 'out' ? `→ ${m.target}` : m.source_addr || ''),
        registerSource: noop,
        recomputeSourceCounts: noop,
        renderMessageList: noop,
        renderSourceLegend: noop,
        renderHealthPills: noop,
        updateHeaderCounters: noop,
        resetDetailHeader: noop,
        patchRow: noop,
        renderDetail: noop,
        showToast: noop,
        saveSession: noop,
        selectMessage: noop,
        prependMessagesToList: noop,
        fetch: () => new Promise(noop),
    };
    vm.createContext(ctx);
    vm.runInContext(read('state.js'), ctx);
    vm.runInContext(read('ws.js'), ctx);
    vm.runInContext(read('render.js'), ctx);
    // render.js redefines the render functions; keep the two under test and
    // put the stubs back for the rest, so the buffer tests don't render.
    ctx.removeRowsFromListReal = ctx.removeRowsFromList;
    Object.assign(ctx, {
        removeRowsFromList: noop,
        prependMessagesToList: noop,
        renderMessageList: noop,
        renderSourceLegend: noop,
        renderHealthPills: noop,
        updateHeaderCounters: noop,
        patchRow: noop,
        renderDetail: noop,
        canPrependOnly: () => true,
        showEmptyState: noop,
    });
    return { ctx, state: ctx.window.state, list };
}

const msg = (id, extra = {}) => ({
    id, source_addr: '10.0.0.1:5000', tags: [], bookmarked: false,
    validation_warning_count: 0, has_segment_errors: false, ...extra,
});

/// The counters, recomputed from scratch over everything the client holds.
function recount(state) {
    const held = [...state.messages, ...state.pendingMessages];
    return {
        bookmarks: held.filter((m) => m.bookmarked).length,
        validation: held.reduce((n, m) => n + (m.validation_warning_count || 0) + (m.has_segment_errors ? 1 : 0), 0),
    };
}
// Arrays built inside the vm context have that realm's prototype; copy
// into this realm before deepEqual.
const ids = (list) => Array.from(list, (m) => m.id);
const counters = (state) => ({ bookmarks: state.totalBookmarkCount, validation: state.totalValidationCount });

/// Receive messages the way the WebSocket does, then flush.
function receive(ctx, list) {
    for (const m of list) ctx.addMessage(m);
    ctx.flushAndRender();
}

test('trimming keeps CLIENT_MESSAGE_CAP droppable messages plus the bookmarked and selected ones', () => {
    const { ctx, state } = setup({ cap: 5 });
    // Oldest first, like the stream: m0 … m9; m1 bookmarked, m2 selected.
    const all = Array.from({ length: 10 }, (_, i) => msg(`m${i}`, i === 1 ? { bookmarked: true } : {}));
    receive(ctx, all.slice(0, 3));
    state.selectedId = 'm2';
    receive(ctx, all.slice(3));
    // 5 newest + the bookmarked m1 + the selected m2: exempt messages don't
    // take places in the cap (#204: the loop used to drop m5 and m6 too).
    assert.deepEqual(ids(state.messages), ['m9', 'm8', 'm7', 'm6', 'm5', 'm2', 'm1']);
    assert.deepEqual(counters(state), recount(state));
});

test('only exempt messages over the cap: nothing is dropped', () => {
    const { ctx, state } = setup({ cap: 2 });
    assert.equal(ctx.trimClientBuffer().size, 0, 'empty buffer');
    // One flush per message, so the pending-buffer cap doesn't apply.
    for (const id of ['a', 'b', 'c']) receive(ctx, [msg(id, { bookmarked: true })]);
    receive(ctx, [msg('d')]);
    // Only one droppable message: within the cap, nothing goes.
    assert.deepEqual(ids(state.messages), ['d', 'c', 'b', 'a']);
    receive(ctx, [msg('e')]);
    receive(ctx, [msg('f')]);
    // Three droppable messages, cap 2: exactly the oldest droppable one goes.
    assert.deepEqual(ids(state.messages), ['f', 'e', 'c', 'b', 'a']);
    assert.equal(ctx.trimClientBuffer().size, 0);
});

test('counters equal a full recount after trim, server eviction and pause/resume', () => {
    const { ctx, state } = setup({ cap: 4 });
    const w = (id, n, extra) => msg(id, { validation_warning_count: n, ...extra });
    receive(ctx, [w('a', 1), w('b', 2, { bookmarked: true }), w('c', 0, { has_segment_errors: true }), w('d', 3), w('e', 1), w('f', 2)]);
    assert.deepEqual(counters(state), recount(state), 'after trim');

    ctx.removeEvictedMessages(['b', 'e']);
    assert.deepEqual(counters(state), recount(state), 'after server eviction');

    // Pause: messages pile up in pendingMessages, capped as well.
    state.paused = true;
    for (const m of [w('g', 1), w('h', 0, { bookmarked: true }), w('i', 4), w('j', 1), w('k', 2), w('l', 1)]) ctx.addMessage(m);
    assert.ok(state.pendingMessages.length <= 4, 'pending buffer is capped');
    assert.deepEqual(counters(state), recount(state), 'while paused');
    ctx.removeEvictedMessages(['i']); // eviction while paused hits the pending buffer
    assert.deepEqual(counters(state), recount(state), 'eviction while paused');

    state.paused = false;
    ctx.flushAndRender();
    assert.deepEqual(counters(state), recount(state), 'after resume');
});

test('a bookmark toggle counts once even if a message is in both buffers (#184)', () => {
    const { ctx, state } = setup();
    receive(ctx, [msg('a')]);
    // Force the state the issue describes: the same id in both lists.
    state.pendingMessages.unshift(msg('a'));
    ctx.updateMessageBookmark(msg('a', { bookmarked: true }));
    assert.equal(state.totalBookmarkCount, 1);
    assert.ok(state.messages[0].bookmarked && state.pendingMessages[0].bookmarked);
    ctx.updateMessageBookmark(msg('a', { bookmarked: false }));
    assert.equal(state.totalBookmarkCount, 0);
    // A repeated event with an unchanged state changes nothing.
    ctx.updateMessageBookmark(msg('a', { bookmarked: false }));
    assert.equal(state.totalBookmarkCount, 0);
});

// --- DOM: rows, group headers and the truncation note.
function row(id) { const el = new El('message-row'); el.dataset.id = id; return el; }
const header = (label) => { const el = new El('group-header'); el.textContent = label; return el; };
const shape = (list) => list.children.map((c) =>
    c.classList.contains('message-row') ? c.dataset.id : c.classList.contains('group-header') ? `[${c.textContent}]` : `#${c.id}`);

test('removing rows leaves no orphaned group header', () => {
    const { ctx, list } = setup();
    for (const el of [header('Live'), row('a'), row('b'), header('Yesterday'), row('c'), row('d')]) list.appendChild(el);
    ctx.removeRowsFromListReal(new Set(['c', 'd']));
    assert.deepEqual(shape(list), ['[Live]', 'a', 'b']);
    ctx.removeRowsFromListReal(new Set(['a']));
    assert.deepEqual(shape(list), ['[Live]', 'b']);
});

test('the truncation note stays at the bottom and disappears when everything is loaded', () => {
    const { ctx, state, list } = setup();
    for (const el of [header('Live'), row('a'), row('b')]) list.appendChild(el);
    state.messages = [msg('a'), msg('b')];
    state.totalMessagesCount = 10; // the server holds more than the view
    ctx.updateTruncationNote();
    assert.deepEqual(shape(list), ['[Live]', 'a', 'b', '#list-truncated-note']);
    const note = list.lastElementChild;
    assert.match(note.children[0].textContent, /holds 2 of 10 messages/);
    // #198: the note offers to load older messages; #197: with a search
    // query, also to search them on the server.
    assert.deepEqual(note.children.slice(1).map((b) => b.dataset.action), ['load-older']);
    state.searchQuery = 'adt';
    ctx.updateTruncationNote();
    assert.deepEqual(note.children.slice(1).map((b) => b.dataset.action), ['load-older', 'search-older']);
    state.searchQuery = '';

    // Rows removed from the bottom: the note is still last, headers clean.
    list.appendChild(header('Older'));
    list.appendChild(row('z'));
    list.appendChild(list.children.find((c) => c.id === 'list-truncated-note'));
    ctx.removeRowsFromListReal(new Set(['z']));
    assert.deepEqual(shape(list), ['[Live]', 'a', 'b', '#list-truncated-note']);

    state.totalMessagesCount = 2;
    ctx.updateTruncationNote();
    assert.ok(list.lastElementChild.classList.contains('hidden'));
});

// --- Older history (#198) and server search (#197) ---
const at = (sec) => new Date(Date.UTC(2026, 10, 12, 12, 0, sec)).toISOString();
const timed = (id, sec, extra = {}) => msg(id, { received_at: at(sec), ...extra });

test('the history cursor skips bookmarked, selected and search-hit messages', () => {
    const { ctx, state } = setup({ cap: 10 });
    state.messages = [timed('n3', 30), timed('n2', 20), timed('sel', 10), timed('bm', 5, { bookmarked: true })];
    state.selectedId = 'sel';
    assert.equal(ctx.historyCursor(), 'n2');
    state.messages.push(timed('hit', 1, { _searchHit: true }));
    assert.equal(ctx.historyCursor(), 'n2');
    state.messages = [];
    assert.equal(ctx.historyCursor(), null);
});

test('older pages merge in arrival order, skip loaded ones, count, and raise the cap', () => {
    const { ctx, state } = setup({ cap: 2 });
    state.messages = [timed('n4', 40), timed('n3', 30), timed('bm', 5, { bookmarked: true })];
    state.totalBookmarkCount = 1;
    const added = ctx.mergeOlderMessages([
        timed('n2', 20, { validation_warning_count: 2 }), timed('n1', 10), timed('bm', 5, { bookmarked: true }),
    ], false);
    assert.equal(added, 2, 'the already loaded bookmark is not added twice');
    assert.deepEqual(ids(state.messages), ['n4', 'n3', 'n2', 'n1', 'bm']);
    assert.equal(state.olderLoaded, 2);
    assert.equal(state.totalValidationCount, 2);
    // Live messages trim against CLIENT_MESSAGE_CAP + olderLoaded: the
    // loaded page survives the next flush.
    ctx.addMessage(timed('n5', 50));
    ctx.flushAndRender();
    assert.deepEqual(ids(state.messages), ['n5', 'n4', 'n3', 'n2', 'bm'], 'only one over the raised cap goes');
    assert.deepEqual(counters(state), recount(state));
    // A reload forgets the loaded history.
    ctx.applySnapshot([timed('n5', 50)]);
    assert.equal(state.olderLoaded, 0);
});

test('a server search continues below its last hit and stops when exhausted', async () => {
    const { ctx, state } = setup({ cap: 10 });
    const calls = [];
    const pages = [[timed('h3', 3), timed('h2', 2), timed('h1', 1)], [timed('h0', 0)]];
    ctx.fetch = async (url) => { calls.push(url); return { ok: true, json: async () => pages.shift() || [] }; };
    state.messages = [timed('n9', 90), timed('n8', 80)];
    state.searchQuery = 'adt';
    await ctx.searchOlderOnServer();
    assert.match(calls[0], /\/api\/search\?q=adt&limit=3&before=n8$/);
    assert.deepEqual(ids(state.messages), ['n9', 'n8', 'h3', 'h2', 'h1']);
    assert.ok(state.messages[2]._searchHit);
    await ctx.searchOlderOnServer();
    assert.match(calls[1], /before=h1$/, 'continues below the last hit');
    assert.equal(state.serverSearch.done, true, 'fewer than a page: no more matches');
    await ctx.searchOlderOnServer();
    assert.equal(calls.length, 2, 'no request once exhausted');
    // A new query starts over; "Load older" continues below the loaded run,
    // not below the search hits.
    state.searchQuery = 'oru';
    await ctx.searchOlderOnServer();
    assert.match(calls[2], /q=oru&limit=3&before=n8$/);
    await ctx.loadOlderMessages();
    assert.match(calls[3], /\/api\/messages\?before=n8&limit=3$/);
});

test('an empty older page says so', async () => {
    const { ctx, state } = setup({ cap: 10 });
    const toasts = [];
    ctx.showToast = (m, type) => toasts.push([m, type]);
    ctx.fetch = async () => ({ ok: true, json: async () => [] });
    state.messages = [timed('n1', 10)];
    await ctx.loadOlderMessages();
    assert.deepEqual(toasts, [['No older messages on the server', 'info']]);
});

test('an older page fetched across a reload is dropped', async () => {
    const { ctx, state } = setup({ cap: 10 });
    let release;
    ctx.fetch = () => new Promise((r) => { release = () => r({ ok: true, json: async () => [timed('old', 1)] }); });
    state.messages = [timed('n1', 10)];
    const pending = ctx.loadOlderMessages();
    state.loadSeq++; // a list reload started meanwhile
    release();
    await pending;
    assert.deepEqual(ids(state.messages), ['n1']);
    assert.equal(state.olderLoading, false);
});
