// Tests for Watch (#282, static/watch.js): the matcher (the list's own filter
// logic on a snapshot of the filter), the title counter that only counts
// while the tab is in the background, the sound and desktop notification
// and their limits, what the menu says is active, and that a notification
// carries no patient data. state.js, util.js, render.js and watch.js run in
// a `node:vm` context.
//
//   node --test 'tests/js/*.test.mjs'
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { El } from './fake-dom.mjs';

const read = (f) => readFileSync(new URL(`../../static/${f}`, import.meta.url), 'utf8');

function memoryStorage() {
    const data = new Map();
    return {
        getItem: (k) => (data.has(k) ? data.get(k) : null),
        setItem: (k, v) => data.set(k, String(v)),
        removeItem: (k) => data.delete(k),
        data,
    };
}

function setup({ secure = false, permission = 'default', session = memoryStorage(), local = memoryStorage() } = {}) {
    const noop = () => {};
    const toasts = [];
    const sounds = [];
    const notes = [];
    const listeners = {};
    const doc = {
        title: 'Harux',
        hidden: false,
        getElementById: () => null,
        createElement: () => new El(),
        addEventListener: (type, fn) => { listeners[type] = fn; },
    };
    class FakeNotification {
        constructor(title, opts) { notes.push({ title, ...opts }); }
        close() {}
    }
    FakeNotification.permission = permission;
    FakeNotification.requestPermission = async () => { FakeNotification.permission = 'granted'; return 'granted'; };
    const ctx = {
        window: {},
        console: { ...console, debug: noop, info: noop },
        document: doc,
        sessionStorage: session,
        localStorage: local,
        isSecureContext: secure,
        Notification: FakeNotification,
        setTimeout: noop,
        Date, JSON, Map, Set, WeakMap, Array, Math, Number, String,
    };
    vm.createContext(ctx);
    for (const f of ['state.js', 'util.js', 'render.js', 'watch.js']) vm.runInContext(read(f), ctx);
    Object.assign(ctx, {
        showToast: (m, t) => toasts.push([m, t]),
        playWatchSound: () => sounds.push(Date.now()),
        esc: (s) => String(s ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'),
        escAttr: (s) => String(s ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'),
    });
    ctx.window.AudioContext = undefined;
    ctx.initWatch();
    return { ctx, state: ctx.window.state, doc, toasts, sounds, notes, listeners, session, local, FakeNotification };
}

const adt = { id: 'a', message_type: 'ADT^A01', patient_name: 'Muster, Max', source_addr: '10.0.0.1:5000', message_control_id: 'C1', direction: 'in' };
const oru = { id: 'o', message_type: 'ORU^R01', patient_name: 'Other', source_addr: '10.0.0.2:5000', direction: 'in', has_segment_errors: true, validation_warning_count: 1 };
const sent = { id: 's', message_type: 'ADT^A01', source_addr: '10.9.9.9:6661', target: 'orchestra', direction: 'out' };

test('the matcher is the list\'s filter on a snapshot', () => {
    const { ctx, state } = setup();
    state.searchQuery = 'adt';
    state.directionFilter = 1; // received only
    const w = ctx.watchFromFilter();
    // A later change of the list's filter does not change the watch.
    state.searchQuery = '';
    state.directionFilter = 0;
    assert.equal(ctx.watchMatches(w, adt), true);
    assert.equal(ctx.watchMatches(w, oru), false, 'not ADT');
    assert.equal(ctx.watchMatches(w, sent), false, 'received only');

    const errors = { query: '', direction: 0, validation: 2 };
    assert.equal(ctx.watchMatches(errors, oru), true);
    assert.equal(ctx.watchMatches(errors, adt), false);
    const sentOnly = { query: 'is:sent', direction: 0, validation: 0 };
    assert.equal(ctx.watchMatches(sentOnly, sent), true);
    assert.equal(ctx.watchMatches(sentOnly, adt), false);
    // Patient, facility, control ID, source, tags: as the search finds them.
    assert.equal(ctx.watchMatches({ query: 'muster', direction: 0, validation: 0 }, adt), true);
    assert.equal(ctx.watchMatches({ query: '', direction: 0, validation: 0 }, adt), true, 'every new message');

    assert.equal(ctx.describeWatch(w), '“adt” · received only');
    assert.equal(ctx.describeWatch(errors), 'with errors');
    assert.equal(ctx.describeWatch({ query: '', direction: 0, validation: 0 }), 'every new message');
});

test('only a tab in the background counts, in its title; coming back clears it', () => {
    const { ctx, state, doc, listeners } = setup();
    state.searchQuery = 'adt';
    ctx.startWatch();
    ctx.watchNewMessage(adt);
    assert.equal(state.watchCount, 0, 'the tab is in front: nothing to tell');
    doc.hidden = true;
    ctx.watchNewMessage(adt);
    ctx.watchNewMessage(oru); // no match
    ctx.watchNewMessage(adt);
    assert.equal(state.watchCount, 2);
    assert.equal(doc.title, '(2) Harux');
    doc.hidden = false;
    listeners.visibilitychange();
    assert.equal(state.watchCount, 0);
    assert.equal(doc.title, 'Harux');
    assert.equal(ctx.watchTitle('Harux', 150), '(99+) Harux');

    // Stopped: nothing counts.
    ctx.stopWatch();
    doc.hidden = true;
    ctx.watchNewMessage(adt);
    assert.equal(state.watchCount, 0);
});

test('sound and desktop notification only when wanted, and not for every message', async () => {
    const { ctx, state, doc, sounds, notes } = setup({ secure: true, permission: 'granted' });
    ctx.startWatch();
    doc.hidden = true;
    ctx.watchNewMessage(adt);
    assert.equal(sounds.length, 0, 'sound is off by default');
    assert.equal(notes.length, 0, 'desktop is off by default');

    await ctx.setWatchPref('sound');
    sounds.length = 0; // the sample played on switching it on
    await ctx.setWatchPref('desktop');
    ctx.watchNewMessage(adt);
    ctx.watchNewMessage(adt);
    assert.equal(sounds.length, 1, 'one beep for a burst');
    assert.equal(notes.length, 1, 'one notification for a burst');
    assert.equal(notes[0].title, 'Harux: 2 matching messages', 'the count so far');
    assert.equal(notes[0].body, 'ADT^A01 from 10.0.0.1:5000 (C1)');
    assert.ok(!JSON.stringify(notes).includes('Muster'), 'no patient data on the desktop');
    assert.deepEqual(JSON.parse(JSON.stringify(ctx.activeWatchChannels())), ['title counter', 'sound', 'desktop notification']);
    assert.equal(state.watchCount, 3);
});

test('on plain HTTP the desktop notification is unavailable, and the menu says why', async () => {
    const { ctx, state } = setup({ secure: false });
    const d = ctx.desktopNotificationStatus();
    assert.equal(d.available, false);
    assert.match(d.reason, /only on HTTPS or localhost/);
    await ctx.setWatchPref('desktop');
    assert.equal(state.watchPrefs.desktop, false, 'cannot be switched on');
    state.watchPrefs.desktop = true; // kept from an HTTPS visit
    assert.deepEqual(JSON.parse(JSON.stringify(ctx.activeWatchChannels())), ['title counter']);
    const html = ctx.watchMenuHtml();
    assert.match(html, /data-pref="desktop"[^>]* disabled/);
    assert.match(html, /only on HTTPS or localhost/);
    assert.match(html, /Active: title counter/);
});

test('the watch lasts for the tab, the preferences for the browser', async () => {
    const session = memoryStorage();
    const local = memoryStorage();
    const first = setup({ session, local });
    first.state.validationFilter = 2;
    first.ctx.startWatch();
    await first.ctx.setWatchPref('sound');
    const again = setup({ session, local });
    assert.equal(again.state.watch.validation, 2);
    assert.equal(again.state.watchPrefs.sound, true);
    again.ctx.stopWatch();
    assert.equal(session.data.has('harux_watch'), false);
    // Broken storage is not fatal.
    session.data.set('harux_watch', '{not json');
    assert.equal(setup({ session, local }).state.watch, null);
});

test('the menu offers to watch the current filter, and to switch to a changed one', () => {
    const { ctx, state } = setup();
    state.searchQuery = '<b>x</b>';
    assert.match(ctx.watchMenuHtml(), /Watch <b>“&lt;b&gt;x&lt;\/b&gt;”<\/b> — the current filter/);
    assert.match(ctx.watchMenuHtml(), /data-action="watch-start">Watch</);
    ctx.startWatch();
    assert.doesNotMatch(ctx.watchMenuHtml(), /instead/);
    state.searchQuery = 'oru';
    assert.match(ctx.watchMenuHtml(), /Watch the current filter instead/);
    state.showBookmarkedOnly = true;
    assert.match(ctx.watchMenuHtml(), /bookmark filter is left out/);
});

test('a blocked permission shows the desktop switch off, as the Active line says (#295 review)', () => {
    const { ctx, state } = setup({ secure: true, permission: 'denied' });
    state.watchPrefs.desktop = true; // kept from before the block
    const html = ctx.watchMenuHtml();
    assert.match(html, /class="switch" role="switch" aria-checked="false" data-action="watch-pref" data-pref="desktop"/);
    assert.match(html, /Blocked in this browser/);
    assert.match(html, /Active: title counter</);
});

