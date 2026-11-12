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
        DETAIL_EMPTY_HTML: '',
        detailJsonCache: new WeakMap(),
        srcLabelFor: (addr) => addr || '',
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
    assert.match(list.lastElementChild.textContent, /newest 2 of 10/);

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
