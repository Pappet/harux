// Tests for the long-value preview in static/util.js (#215). util.js runs in
// a `node:vm` context with stubbed sessionStorage/crypto (its top level reads
// the session id).
//
//   node --test 'tests/js/*.test.mjs'
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

function setup() {
    const store = new Map();
    const ctx = {
        sessionStorage: { getItem: (k) => store.get(k) ?? null, setItem: (k, v) => store.set(k, v) },
        crypto: { randomUUID: () => 'test-session' },
        console,
        document: { createElement: () => ({ className: '', textContent: '' }) },
        window: {},
    };
    vm.createContext(ctx);
    vm.runInContext(readFileSync(new URL('../../static/util.js', import.meta.url), 'utf8'), ctx);
    // const declarations are not context properties; expose what the tests need.
    vm.runInContext('globalThis.LIMITS = { max: DISPLAY_VALUE_MAX, preview: DISPLAY_VALUE_PREVIEW };', ctx);
    return ctx;
}

test('short values are shown unchanged', () => {
    const ctx = setup();
    const v = 'x'.repeat(ctx.LIMITS.max);
    const shown = ctx.previewText(v);
    assert.equal(shown.truncated, false);
    assert.equal(shown.text, v);
    assert.equal(ctx.previewText(undefined).truncated, false);
});

test('long values show a preview and their total size', () => {
    const ctx = setup();
    const v = 'QUJD'.repeat(1310726); // 5.0 MB of Base64
    const shown = ctx.previewText(v);
    assert.equal(shown.truncated, true);
    assert.equal(shown.text, v.slice(0, ctx.LIMITS.preview));
    assert.equal(shown.size, '5.0 MB');
});

test('the preview never splits a surrogate pair', () => {
    const ctx = setup();
    const v = 'a'.repeat(ctx.LIMITS.preview - 1) + '😀'.repeat(ctx.LIMITS.max);
    const shown = ctx.previewText(v);
    assert.equal(shown.text.length, ctx.LIMITS.preview - 1);
    assert.ok(!/[\uD800-\uDBFF]$/.test(shown.text));
});

test('formatSize', () => {
    const ctx = setup();
    assert.equal(ctx.formatSize(512), '512 B');
    assert.equal(ctx.formatSize(2048), '2.0 KB');
    assert.equal(ctx.formatSize(3 * 1024 * 1024), '3.0 MB');
});
