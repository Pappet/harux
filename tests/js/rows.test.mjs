// Tests for the message-row builder in static/render.js (#202, #267 review):
// the skeleton's shape that patchRow, the delegation in app.js and the CSS
// rely on, and one filled row. render.js runs in a `node:vm` context with the
// fake DOM from fake-dom.mjs.
//
//   node --test 'tests/js/*.test.mjs'
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { El } from './fake-dom.mjs';

function setup() {
    const ctx = {
        window: {},
        console,
        document: { createElement: (tag) => { const el = new El(); el.tagName = tag.toUpperCase(); return el; } },
        WeakMap, Set, Map,
        getSourceColor: () => '#4fb98a',
        TARGET_PREFIX: '→ ',
        sourceKeyOf: (m) => (m.direction === 'out' ? `→ ${m.target}` : m.source_addr || ''),
        srcLabelFor: (key) => key || '',
        deliveryChip: (d) => (d && d.state === 'acked'
            ? { cls: `msg-ack ${d.ack_code.toLowerCase()}`, text: d.ack_code, title: '' }
            : { cls: 'msg-ack pending', text: '…', title: '' }),
        formatLatency: (ms) => (typeof ms === 'number' ? `${ms} ms` : ''),
        rowTimeLabel: () => '3s ago',
        ackChipClass: (code) => `msg-ack ${code.toLowerCase()}`,
    };
    vm.createContext(ctx);
    vm.runInContext(readFileSync(new URL('../../static/state.js', import.meta.url), 'utf8'), ctx);
    vm.runInContext(readFileSync(new URL('../../static/render.js', import.meta.url), 'utf8'), ctx);
    // The SVG icons are parsed from markup in the browser; a marker is enough here.
    ctx.iconNode = (name) => { const el = new El('icon'); el.dataset.icon = name; return el; };
    return { ctx, state: ctx.window.state };
}

const cls = (el) => el.className.split(' ').filter(Boolean);
const shape = (el) => ({ tag: el.tagName, cls: cls(el)[0], children: el.children.map(shape) });

test('the row skeleton has the structure patchRow and the delegation rely on', () => {
    const { ctx } = setup();
    const row = ctx.rowSkeleton();
    assert.deepEqual(JSON.parse(JSON.stringify(shape(row))), {
        tag: 'DIV', cls: 'message-row', children: [
            { tag: 'DIV', cls: 'msg-source-bar', children: [] },
            { tag: 'DIV', cls: 'msg-body', children: [
                { tag: 'DIV', cls: 'msg-row1', children: [
                    { tag: 'SPAN', cls: 'msg-type', children: [] },
                    { tag: 'SPAN', cls: 'msg-patient', children: [] }] },
                { tag: 'DIV', cls: 'msg-row2', children: ['facility', 'sep', 'src', 'sep', 'segs', 'time']
                    .map((c) => ({ tag: 'SPAN', cls: c, children: [] })) }] },
            { tag: 'DIV', cls: 'msg-actions', children: [{ tag: 'BUTTON', cls: 'msg-bookmark', children: [] }] },
        ],
    });
    assert.equal(row.dataset.action, 'select-message');
    assert.equal(row.tabIndex, 0);
    assert.equal(row.getAttribute('role'), 'button');
    assert.equal(row.children[2].firstElementChild.dataset.action, 'bookmark');
    // Every row is a fresh clone: filling one doesn't touch the next.
    row.children[1].children[0].children[0].textContent = 'changed';
    assert.equal(ctx.rowSkeleton().children[1].children[0].children[0].textContent, '');
});

test('a filled row: texts, chips in order with the ACK last, bookmark button', () => {
    const { ctx, state } = setup();
    state.selectedId = 'm1';
    const row = ctx.buildMessageRow({
        id: 'm1', message_type: 'ADT^A01', patient_name: 'Muster, Max', sending_facility: 'KLINIK',
        source_addr: '10.0.0.1:5000', segment_count: 4, received_at: '2026-11-12T18:00:00Z',
        validation_warning_count: 2, has_segment_errors: false, tags: ['a', 'b', 'c'],
        ack_code: 'aa', bookmarked: true,
    });
    assert.deepEqual(cls(row).sort(), ['bookmarked', 'message-row', 'selected']);
    assert.equal(row.dataset.id, 'm1');
    const [bar, body, actions] = row.children;
    assert.equal(bar.style.background, '#4fb98a');
    const [row1, row2] = body.children;
    assert.deepEqual(row1.children.map((c) => cls(c)[0]), ['msg-type', 'msg-patient', 'msg-warning', 'msg-tags-list', 'msg-ack']);
    assert.equal(row1.children[0].textContent, 'ADT^A01');
    assert.equal(row1.children[1].textContent, 'Muster, Max');
    assert.equal(row1.children[2].textContent, '⚠ 2');
    assert.deepEqual(row1.children[3].children.map((c) => c.textContent), ['a', 'b', '+1']);
    assert.equal(row1.lastElementChild.textContent, 'AA', 'patchRow inserts tags before the ACK chip');
    assert.deepEqual(row2.children.map((c) => c.textContent), ['KLINIK', '·', '10.0.0.1:5000', '·', '4 segs', '3s ago']);
    const btn = actions.firstElementChild;
    assert.equal(btn.dataset.id, 'm1');
    assert.equal(btn.getAttribute('aria-label'), 'Remove bookmark');
    assert.ok(cls(btn).includes('active'));
    assert.equal(btn.firstElementChild.dataset.icon, 'starFilled');
});

test('a parse error row shows the marker and keeps the reason as title', () => {
    const { ctx } = setup();
    const row = ctx.buildMessageRow({ id: 'e', message_type: 'UNKNOWN', parse_error: 'Message does not start with MSH', tags: [] });
    const type = row.children[1].children[0].children[0];
    assert.ok(cls(type).includes('parse-error'));
    assert.equal(type.textContent, '⚠ PARSE ERROR');
    assert.equal(type.title, 'Message does not start with MSH');
    assert.equal(row.children[1].children[0].lastElementChild.textContent, '—', 'no ACK: placeholder chip');
});

test('a sent row: arrow, target instead of source, chip from the delivery, no sender colour (#272)', () => {
    const { ctx } = setup();
    const sent = {
        id: 's1', direction: 'out', target: 'orchestra-test', message_type: 'ADT^A01', patient_name: 'Muster, Max',
        sending_facility: 'KLINIK', source_addr: '10.4.1.9:6661', segment_count: 4, tags: [],
        delivery: { state: 'pending' },
    };
    const row = ctx.buildMessageRow(sent);
    assert.ok(cls(row).includes('outbound'));
    const [bar, body] = row.children;
    assert.equal(bar.style.background, undefined, 'the dashed bar comes from style.css');
    const [row1, row2] = body.children;
    assert.equal(row1.children[0].textContent, '↑ ADT^A01');
    assert.equal(row1.lastElementChild.textContent, '…');
    assert.equal(row2.children[2].textContent, '→ orchestra-test');
    assert.equal(row2.children[2].title, 'Sent to 10.4.1.9:6661');

    // The outcome replaces the chip and adds the round-trip time.
    const list = { querySelector: () => row };
    ctx.document.getElementById = () => list;
    ctx.CSS = { escape: (v) => v };
    row.querySelector = (sel) => {
        const c = sel.replace(/^\./, '');
        const walk = (el) => el.classList.contains(c) ? el : el.children.map(walk).find(Boolean) || null;
        return walk(row);
    };
    ctx.patchRow('s1', { delivery: { state: 'acked', ack_code: 'AA', latency_ms: 42 } });
    assert.deepEqual(row1.children.map((c) => c.textContent).slice(-2), ['AA', '42 ms']);
    ctx.patchRow('s1', { delivery: { state: 'acked', ack_code: 'AE', latency_ms: 7 } });
    assert.deepEqual(row1.children.map((c) => cls(c)[0]), ['msg-type', 'msg-patient', 'msg-ack', 'msg-latency']);
    assert.equal(row1.children[2].textContent, 'AE');
});
