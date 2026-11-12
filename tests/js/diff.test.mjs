// Tests for the diff view (static/diff.js, #228). No dependencies: the
// browser script is loaded into a `node:vm` context with the few globals it
// uses stubbed, and its functions are called directly.
//
//   node --test tests/js/
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

function loadDiff(previewText = (t) => ({ text: t, truncated: false, size: '' })) {
    const ctx = {
        state: { diffPinnedMessage: null, diffIgnoreDynamic: false },
        DYNAMIC_DIFF_FIELDS: new Set(['MSH-7', 'MSH-10']),
        esc: (s) => String(s ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;'),
        previewText,
        escAttr: (s) => String(s ?? '').replace(/"/g, '&quot;'),
    };
    vm.createContext(ctx);
    vm.runInContext(readFileSync(new URL('../../static/diff.js', import.meta.url), 'utf8'), ctx);
    return ctx;
}

// Segment from its field values: fields are numbered 1..n.
const seg = (name, ...values) => ({
    name,
    fields: values.map((value, i) => ({ index: i + 1, value, description: null })),
});
const msh = (ctrl) => seg('MSH', '|', '^~\\&', 'LAB', 'KLINIK', 'RCV', 'RZ', `2024010112000${ctrl.length}`, '', 'ORU^R01', ctrl, 'P', '2.5');
const obx = (n, code, value) => seg('OBX', String(n), 'NM', code, '', value, 'mmol/L');
const oru = (ctrl, ...obxs) => ({
    message_type: 'ORU^R01',
    message_control_id: ctrl,
    segments: [msh(ctrl), seg('PID', '', '', '4711'), seg('OBR', '1', '', 'F123'), ...obxs],
});

function render(ctx, reference, current, ignoreDynamic = true) {
    ctx.state.diffPinnedMessage = reference;
    ctx.state.diffIgnoreDynamic = ignoreDynamic;
    const container = { innerHTML: '' };
    ctx.renderDiffTab(container, current);
    const html = container.innerHTML;
    const summary = html.match(/class="diff-summary[^"]*">([^<]*)</)[1];
    const changed = [...html.matchAll(/<tr class="diff-row changed">\s*<td class="diff-field-name"[^>]*>([^<]*)/g)]
        .map((m) => m[1]);
    return { html, summary, changed };
}

test('a difference in the second OBX is found (#228)', () => {
    const ctx = loadDiff();
    const a = oru('A', obx(1, 'GLU', '5.4'), obx(2, 'K', '4.1'));
    const b = oru('B', obx(1, 'GLU', '5.4'), obx(2, 'K', '7.9'));
    const r = render(ctx, a, b);
    assert.deepEqual(r.changed, ['OBX[2]-5']);
    assert.match(r.summary, /1 field difference found/);
    assert.match(r.summary, /1 dynamic hidden/); // MSH-10 differs and is hidden
});

test('identical messages stay identical', () => {
    const ctx = loadDiff();
    const r = render(ctx, oru('A', obx(1, 'GLU', '5.4')), oru('B', obx(1, 'GLU', '5.4')));
    assert.match(r.summary, /Messages are identical/);
    assert.deepEqual(r.changed, []);
});

test('an extra or missing occurrence is a difference', () => {
    const ctx = loadDiff();
    const two = oru('A', obx(1, 'GLU', '5.4'), obx(2, 'K', '4.1'));
    const three = oru('B', obx(1, 'GLU', '5.4'), obx(2, 'K', '4.1'), obx(3, 'NA', '140'));

    let r = render(ctx, two, three);
    assert.match(r.html, /OBX\[3\]\s*<span class="diff-missing-badge">only in current/);
    assert.match(r.summary, /1 segment only in one message/);
    assert.doesNotMatch(r.summary, /identical/);

    r = render(ctx, three, two);
    assert.match(r.html, /only in reference/);
    assert.match(r.summary, /1 segment only in one message/);
});

test('a one-sided segment with only empty fields still counts', () => {
    const ctx = loadDiff();
    const a = oru('A', obx(1, 'GLU', '5.4'));
    const b = { ...oru('B', obx(1, 'GLU', '5.4')) };
    b.segments = [...b.segments, seg('NTE', '')];
    const r = render(ctx, a, b);
    assert.equal(r.summary.trim(), '&#9651; 1 segment only in one message (1 dynamic hidden)');
});

test('single segments keep plain labels; repeats are numbered', () => {
    const ctx = loadDiff();
    const pairs = ctx.pairSegments(
        [seg('MSH'), seg('PID'), seg('OBX'), seg('NTE')],
        [seg('MSH'), seg('PID'), seg('OBX'), seg('OBX'), seg('NTE')],
    );
    // The extra OBX goes right after the last OBX pair, not to the end.
    assert.deepEqual(pairs.map((p) => p.label), ['MSH', 'PID', 'OBX[1]', 'OBX[2]', 'NTE']);
    assert.equal(pairs[3].segA, null);
});

test('hiding dynamic fields still matches MSH-7/MSH-10 by segment name', () => {
    const ctx = loadDiff();
    const a = oru('A', obx(1, 'GLU', '5.4'));
    const b = oru('BB', obx(1, 'GLU', '5.4')); // different MSH-7 and MSH-10
    assert.match(render(ctx, a, b, true).summary, /identical \(2 dynamic hidden\)/);
    assert.deepEqual(render(ctx, a, b, false).changed, ['MSH-7', 'MSH-10']);
});

// #215 / #267 review: a long value is shown as a preview with a note, but the
// comparison uses the full values — two values that agree in their first
// characters and differ later are still a difference.
test('long values are previewed but compared in full', () => {
    const preview = (t) => (typeof t === 'string' && t.length > 20
        ? { text: t.slice(0, 10), truncated: true, size: '5.0 MB' }
        : { text: t, truncated: false, size: '' });
    const ctx = loadDiff(preview);
    const head = 'A'.repeat(30);
    const stats = { totalDiffs: 0, hiddenDynamic: 0, segHasDiff: false };
    const html = ctx.buildDiffFieldRow('OBX', 'OBX', 5, { value: head + 'X' }, { value: head + 'Y' }, false, stats);
    assert.equal(stats.totalDiffs, 1, 'differs beyond the preview');
    assert.match(html, /class="diff-row changed"/);
    assert.equal((html.match(/value-truncated/g) || []).length, 2);
    assert.match(html, /5\.0 MB in total, shortened for display/);
    assert.ok(!html.includes(head + 'X'), 'the full value is not rendered');

    const same = { totalDiffs: 0, hiddenDynamic: 0, segHasDiff: false };
    ctx.buildDiffFieldRow('OBX', 'OBX', 5, { value: head + 'X' }, { value: head + 'X' }, false, same);
    assert.equal(same.totalDiffs, 0);
});
