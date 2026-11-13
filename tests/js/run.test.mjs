// Tests for runs in the UI (#280): *Resend filtered* in static/send.js — what
// it sends (the filtered messages, oldest first, cut-off entries left out),
// the confirmation that names target and number, the targets that take no
// runs, the limits — and the run bar that follows the run's status until it
// ends, with Cancel. state.js, util.js, render.js and send.js run in a
// `node:vm` context; fetch answers like the server.
//
//   node --test 'tests/js/*.test.mjs'
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { webcrypto } from 'node:crypto';
import vm from 'node:vm';
import { El } from './fake-dom.mjs';

const read = (f) => readFileSync(new URL(`../../static/${f}`, import.meta.url), 'utf8');

const running = (over = {}) => ({
    id: 'run-1', target: 'orchestra', requested_by: '10.0.0.7', total: 3, done: 0, state: 'running',
    codes: {}, written: 0, failed: 0, avg_latency_ms: null, reason: null, ...over,
});

function setup({ confirmAnswer = true, statuses = [], sendStatus = 202 } = {}) {
    const noop = () => {};
    const toasts = [];
    const requests = [];
    const confirms = [];
    const timers = [];
    const bar = new El('run-bar hidden', 'run-bar');
    const elements = { 'run-bar': bar };
    const ctx = {
        window: {},
        console: { ...console, debug: noop, info: noop },
        document: { getElementById: (id) => elements[id] || null, createElement: () => new El(), activeElement: null },
        sessionStorage: { getItem: () => null, setItem: noop },
        localStorage: { getItem: () => null, setItem: noop },
        crypto: webcrypto,
        setTimeout: (fn) => { timers.push(fn); return timers.length; },
        clearTimeout: noop,
        encodeURIComponent,
        WeakMap, Set, Map, JSON, Uint8Array, Array, Object, Math,
        confirm: (q) => { confirms.push(q); return confirmAnswer; },
        fetch: async (url, opts = {}) => {
            requests.push({ url, opts });
            if (url === '/api/send') {
                return { ok: true, status: sendStatus, json: async () => running(), text: async () => '' };
            }
            if (url.endsWith('/cancel')) {
                return { ok: true, status: 200, json: async () => running({ done: 1 }), text: async () => '' };
            }
            const next = statuses.shift();
            if (next === 404) return { ok: false, status: 404, json: async () => ({}), text: async () => 'No such run' };
            return { ok: true, status: 200, json: async () => next, text: async () => '' };
        },
    };
    vm.createContext(ctx);
    for (const f of ['state.js', 'util.js', 'render.js', 'send.js']) vm.runInContext(read(f), ctx);
    // esc() needs a real DOM; this escapes the same five characters.
    const escape = (s) => (s ? String(s).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`) : '');
    Object.assign(ctx, {
        esc: escape,
        showToast: (m, type) => toasts.push([m, type]),
        renderDetail: noop,
        renderMessageList: noop,
    });
    const state = ctx.window.state;
    state.sendConfig = {
        enabled: true, max_run_messages: 1000,
        loopback: { name: 'This Harux', host: '127.0.0.1', port: 2575 }, tls: { allow_unverified: false },
    };
    // Newest first, as the list holds them.
    state.messages = [
        { id: 'm3', message_type: 'ADT^A08', direction: 'in' },
        { id: 'm2', message_type: 'ORU^R01', direction: 'in', truncated: true },
        { id: 'm1', message_type: 'ADT^A01', direction: 'in' },
    ];
    const target = ctx.upsertTarget(ctx.normalizeTarget({ name: 'orchestra', host: 'orch.local', port: 6661, tls: false }));
    const runAll = async () => {
        while (timers.length) await timers.shift()();
    };
    return { ctx, state, toasts, requests, confirms, bar, target, runAll };
}

const body = (r) => JSON.parse(r.opts.body);

test('resend filtered: the filtered messages, oldest first, cut-off entries left out', async () => {
    const { ctx, state, requests, confirms, target, bar } = setup();
    assert.equal(await ctx.resendFiltered(target), true);
    const sent = body(requests[0]);
    assert.deepEqual(sent.items.map((i) => i.message_id), ['m1', 'm3']);
    assert.equal(sent.allow_invalid, false);
    assert.equal(sent.set, undefined, 'as received: no overrides');
    assert.equal(sent.repeat, undefined);
    // The confirmation names target, address and number.
    assert.match(confirms[0], /Resend 2 messages to orchestra \(orch\.local:6661\), oldest first, byte for byte\? 1 cut-off entry is left out/);
    assert.equal(state.run.id, 'run-1');
    assert.equal(state.lastTargetId, target.id);
    assert.ok(!bar.classList.contains('hidden'));
    assert.match(bar.innerHTML, /Resending 2 messages → orchestra/);
    assert.match(bar.innerHTML, /0 \/ 3/);
    assert.match(bar.innerHTML, /data-action="run-cancel"/);
});

test('resend filtered follows the filters: direction, bookmarks', async () => {
    const { ctx, state, requests, target } = setup();
    state.messages.push({ id: 'm0', message_type: 'ADT^A01', direction: 'out', bookmarked: true });
    state.directionFilter = 1; // received only
    await ctx.resendFiltered(target);
    assert.deepEqual(body(requests[0]).items.map((i) => i.message_id), ['m1', 'm3']);

    const second = setup();
    second.state.messages.push({ id: 'm0', message_type: 'ADT^A01', bookmarked: true });
    second.state.showBookmarkedOnly = true;
    await second.ctx.resendFiltered(second.target);
    assert.deepEqual(body(second.requests[0]).items.map((i) => i.message_id), ['m0']);
});

test('a "no" sends nothing; a target that asks before sending takes no runs', async () => {
    const no = setup({ confirmAnswer: false });
    assert.equal(await no.ctx.resendFiltered(no.target), false);
    assert.equal(no.requests.length, 0);

    const { ctx, requests, confirms, toasts, target } = setup();
    target.ask = true;
    assert.equal(await ctx.resendFiltered(target), false);
    assert.equal(requests.length, 0);
    assert.equal(confirms.length, 0);
    assert.match(toasts[0][0], /asks before every message — it takes no runs/);
    // The menu says so too.
    assert.match(ctx.runMenuHint(ctx.runCandidates(ctx.filteredForRun()), target), /takes no runs/);
    assert.match(ctx.targetOptionHtml(target, true, Date.now(), true), /no runs/);
});

test('the run limit and send-as with messages that did not parse', async () => {
    const big = setup();
    big.state.sendConfig.max_run_messages = 1;
    assert.equal(await big.ctx.resendFiltered(big.target), false);
    assert.equal(big.requests.length, 0);
    assert.match(big.toasts[0][0], /more than one run takes \(1, send\.max_run_messages\)/);

    const { ctx, state, requests, confirms, toasts, target } = setup();
    state.messages[0].parse_error = 'no MSH';
    state.sendAs = 'new_id';
    assert.equal(await ctx.resendFiltered(target), false);
    assert.match(toasts[0][0], /did not parse and can only be resent as received/);
    assert.equal(requests.length, 0);

    // As received: a second confirmation, then allow_invalid.
    state.sendAs = 'unchanged';
    assert.equal(await ctx.resendFiltered(target), true);
    assert.equal(confirms.length, 2);
    assert.match(confirms[1], /1 of them did not parse/);
    assert.equal(body(requests[0]).allow_invalid, true);
});

test('send as applies to the whole run', async () => {
    const { ctx, state, requests, target } = setup();
    state.sendAs = 'new_id_time';
    await ctx.resendFiltered(target);
    assert.deepEqual(body(requests[0]).set, { 'MSH-10': '{{id}}', 'MSH-7': '{{now}}' });
});

test('the bar follows the run until it ends, then offers to close', async () => {
    const statuses = [
        running({ total: 50, done: 24, codes: { AA: 22, AE: 2 }, avg_latency_ms: 38 }),
        running({ total: 50, done: 50, state: 'done', codes: { AA: 48, AE: 2 }, avg_latency_ms: 40 }),
    ];
    const { ctx, state, bar, toasts, requests, target, runAll } = setup({ statuses });
    let ended = 0;
    await ctx.startRun({ target, items: [{ message_id: 'm1' }], repeat: 50, what: 'Sending ×50', onEnd: () => { ended += 1; } });
    assert.equal(body(requests[0]).repeat, 50);
    await runAll();
    assert.equal(requests[1].url, '/api/send/runs/run-1');
    assert.equal(state.run.status.state, 'done');
    assert.match(bar.innerHTML, /50 \/ 50/);
    assert.match(bar.innerHTML, /<span class="mono ok">48 AA<\/span><span class="mono bad">2 AE<\/span>/);
    assert.match(bar.innerHTML, /avg 40 ms/);
    assert.match(bar.innerHTML, /Done/);
    assert.match(bar.innerHTML, /data-action="run-close"/);
    assert.equal(ended, 1, 'onEnd once');
    assert.match(toasts.at(-1)[0], /Sending ×50: Done · 50 \/ 50 · 48 AA · 2 AE/);
    assert.equal(toasts.at(-1)[1], 'error', 'an AE is no clean run');
    ctx.closeRunBar();
    assert.equal(state.run, null);
    assert.ok(bar.classList.contains('hidden'));
});

test('cancel asks the server to stop; a second run waits for the first', async () => {
    const { ctx, state, requests, toasts, target } = setup({ statuses: [] });
    await ctx.startRun({ target, items: [{ message_id: 'm1' }, { message_id: 'm3' }], what: 'two' });
    assert.equal(await ctx.startRun({ target, items: [{ message_id: 'm1' }], what: 'again' }), false);
    assert.match(toasts.at(-1)[0], /A run is under way/);
    ctx.closeRunBar(); // not while it runs
    assert.ok(state.run);
    await ctx.cancelRun();
    const cancel = requests.find((r) => r.url === '/api/send/runs/run-1/cancel');
    assert.equal(cancel.opts.method, 'POST');
    assert.equal(state.run.cancelling, true);
});

test('a run the server no longer knows ends the bar', async () => {
    const { ctx, state, bar, target, runAll } = setup({ statuses: [404] });
    await ctx.startRun({ target, items: [{ message_id: 'm1' }, { message_id: 'm3' }], what: 'two' });
    await runAll();
    assert.equal(state.run.status.state, 'lost');
    assert.match(bar.innerHTML, /No longer known to the server/);
});

test('a "run" of one message is answered like a resend', async () => {
    const { ctx, state, toasts, target } = setup({ sendStatus: 200 });
    // The fake answers a run status to /api/send; a 200 is read as a delivery.
    state.messages = [{ id: 'm1', message_type: 'ADT^A01' }];
    ctx.fetch = async () => ({
        ok: true, status: 200,
        json: async () => ({ id: 's1', delivery: { state: 'acked', ack_code: 'AA', latency_ms: 5 } }),
        text: async () => '',
    });
    assert.equal(await ctx.resendFiltered(target), true);
    assert.equal(state.run, null);
    assert.match(toasts.at(-1)[0], /Sent to orchestra · AA/);
});

test('the bar escapes what it shows', () => {
    const { ctx } = setup();
    const html = ctx.runBarHtml({
        what: 'Resending 2 → <b>x</b>',
        status: running({ state: 'gave_up', reason: '<img src=x>', failed: 3, done: 3 }),
    });
    assert.ok(!html.includes('<b>') && !html.includes('<img'), html);
    assert.match(html, /Gave up/);
    assert.match(html, /3 failed/);
});

test('a run that tried every message but lost some says why (#293 review)', () => {
    const { ctx } = setup();
    const lossy = running({ state: 'done', done: 5, total: 5, codes: { AA: 3 }, failed: 2, reason: 'No response within send.response_timeout_secs' });
    const html = ctx.runBarHtml({ what: 'five', status: lossy });
    assert.match(html, /<span class="mono end bad">Done<\/span>/);
    assert.match(html, /last failure: No response within send\.response_timeout_secs/);
    // A clean run shows no reason.
    const clean = ctx.runBarHtml({ what: 'five', status: running({ state: 'done', done: 5, total: 5, codes: { AA: 5 }, reason: null }) });
    assert.match(clean, /<span class="mono end ok">Done<\/span>/);
    assert.ok(!clean.includes('class="reason"'));
});


test('an imported file is one source, named whole (#281)', () => {
    const { ctx, state } = setup();
    assert.equal(ctx.srcLabelFor('import:export 2026:11.hl7'), 'import:export 2026:11.hl7');
    assert.equal(ctx.srcLabelFor('10.0.0.1:5000'), '10.0.0.1', 'an address still groups by host');
    state.colorByPort = false;
    assert.notEqual(ctx.getSourceColor('import:a.hl7'), undefined);
    assert.equal(ctx.getSourceColor('import:a.hl7'), ctx.getSourceColor('import:a.hl7'));
});
