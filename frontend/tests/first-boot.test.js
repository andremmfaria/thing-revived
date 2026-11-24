'use strict';
// First-boot overlay: phase model, view, and every input gate (delta plan 5.3).
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const FB = require('../src/firstboot/first-boot.js');
const Keyboard = require('../src/input/keyboard.js');
const Focus = require('../src/input/focus.js');
const Types = require('../src/utils/types.js');
const { makeHarness } = require('./helpers/chart-harness.js');

const PROTOCOL = path.resolve(__dirname, '..', '..', 'config', 'protocol', 'controller');
const fx = (name) => JSON.parse(fs.readFileSync(path.join(PROTOCOL, name + '.json'), 'utf8'));
const SECRET = fx('event.user_bootstrap_secret').password;
const ph = (p) => fx('event.user_bootstrap.' + p);

// Minimal DOM: elements with children, classes, listeners; a document with
// capture listeners and getElementById.
function makeDoc() {
  const byId = {};
  const docListeners = {};
  const mk = (tag) => {
    const e = {
      tag, children: [], className: '', textContent: '', attrs: {}, listeners: {}, disabled: false,
      appendChild(c) { this.children.push(c); return c; },
      addEventListener(n, fn) { (this.listeners[n] = this.listeners[n] || []).push(fn); },
      setAttribute(k, v) { this.attrs[k] = v; },
    };
    Object.defineProperty(e, 'innerHTML', { get() { return ''; }, set() { this.children = []; } });
    return e;
  };
  const doc = {
    createElement: mk,
    getElementById: (id) => byId[id] || null,
    addEventListener(n, fn, capture) { (docListeners[n] = docListeners[n] || []).push({ fn, capture }); },
    docListeners, byId,
  };
  return doc;
}
const flat = (el) => [el.textContent].concat(...el.children.map(flat)).join('|');
const evt = (key) => ({
  key, ctrlKey: false, calls: [],
  preventDefault() { this.calls.push('p'); },
  stopPropagation() { this.calls.push('s'); },
  stopImmediatePropagation() { this.calls.push('i'); },
});

function setup() {
  const doc = makeDoc();
  const root = doc.createElement('div');
  const field = doc.createElement('input');
  doc.byId['first-boot'] = root;
  doc.byId['wifi-pass-input'] = field;
  const fb = FB.create(doc, root, 'wifi-pass-input');
  return { doc, root, field, fb };
}

test('model: phases drive active(), only prompt, generating and showing block', () => {
  const m = FB.createModel();
  assert.equal(m.active(), false);
  ['prompt', 'generating', 'showing'].forEach((p) => { m.handle(ph(p)); assert.equal(m.active(), true, p); });
  ['done', 'none'].forEach((p) => { m.handle(ph(p)); assert.equal(m.active(), false, p); });
});

test('model: storage failure shows a note and blocks nothing', () => {
  const m = FB.createModel();
  m.handle(fx('event.user_bootstrap.storage_failed'));
  assert.equal(m.active(), false);
  assert.equal(m.storageNote(), true);
  m.handle(Object.assign({}, ph('prompt'), { storage_ok: false }));
  assert.equal(m.active(), false, 'M is not intercepted by the controller, so nothing may trap input');
});

test('model: the secret is held only while showing and wiped on any other phase', () => {
  const m = FB.createModel();
  m.handle(ph('prompt'));
  assert.equal(m.handle(fx('event.user_bootstrap_secret')), true);
  assert.equal(m.phase(), 'showing');
  assert.equal(m.view().password, SECRET);
  m.handle(ph('showing'));
  assert.ok(m.view().password, 'a repeated showing event keeps it');
  m.handle(ph('generating'));
  assert.equal(m.view().password, null);
  m.handle(fx('event.user_bootstrap_secret'));
  m.handle(ph('done'));
  assert.equal(m.view().password, null);
  assert.equal(JSON.stringify(m.view()).indexOf(SECRET), -1);
});

test('model: connection close wipes the secret and the screen; a new get re-shows it', () => {
  const m = FB.createModel();
  m.handle(ph('showing'));
  m.handle(fx('event.user_bootstrap_secret'));
  m.connectionClosed();
  assert.equal(m.active(), false);
  assert.equal(m.view().password, null);
  m.handle(ph('showing'));
  m.handle(fx('event.user_bootstrap_secret'));
  assert.equal(m.active(), true);
  assert.ok(m.view().password);
});

test('model: errors belong to the flow by context, or by code while active; others are not ours', () => {
  const m = FB.createModel();
  const err = fx('event.user_error');
  assert.equal(FB.isOurs(err, false), true, 'context user_bootstrap_get');
  assert.equal(FB.isOurs({ type: 'user_error', context: 'user_add', code: 'storage_unavailable' }, false), false);
  assert.equal(FB.isOurs({ type: 'user_error', context: 'user_add', code: 'ack_failed' }, true), true);
  assert.equal(FB.isOurs({ type: 'user_error', context: 'user_add', code: 'exists' }, true), false);
  assert.equal(FB.isOurs({ type: 'wifi_status' }, true), false);
  m.handle(ph('prompt'));
  m.handle(err);
  assert.match(m.view().error, /generate/i);
  m.handle(ph('generating'));
  assert.ok(m.view().error, 'error stays until done or none');
  m.handle(ph('done'));
  assert.equal(m.view().error, null);
});

test('view: prompt text, arrow, no timer; showing has the password as one unbroken string, never an input', () => {
  const { root, fb } = setup();
  assert.match(root.className, /hidden/);
  fb.handle(ph('prompt'));
  assert.match(root.className, /blocking/);
  const prompt = flat(root.children[0]);
  assert.match(prompt, /Kiosk password not set/);
  assert.match(prompt, /kiosk user password is unset/);
  assert.match(prompt, /M button/);
  assert.ok(root.children[0].children.some((c) => c.className === 'fb-arrow'));
  fb.handle(ph('generating'));
  assert.match(flat(root.children[0]), /Generating/);
  fb.handle(ph('showing'));
  fb.handle(fx('event.user_bootstrap_secret'));
  const text = flat(root.children[0]);
  [SECRET, 'Press M again', 'will not be shown again'].forEach((s) => assert.ok(text.indexOf(s) !== -1, s));
  assert.doesNotMatch(text, /\d+:\d\d|expires|seconds|countdown/i);
  const tags = [];
  (function walk(e) { tags.push(e.tag); e.children.forEach(walk); })(root);
  assert.equal(tags.indexOf('input'), -1);
  fb.handle(ph('done'));
  assert.match(root.className, /hidden/);
  assert.equal(flat(root).indexOf(SECRET), -1, 'no password left in the DOM after done');
});

test('view: the Wi-Fi text field is disabled while the overlay blocks, enabled after', () => {
  const { field, fb } = setup();
  fb.handle(ph('prompt'));
  assert.equal(field.disabled, true);
  fb.handle(ph('done'));
  assert.equal(field.disabled, false);
});

test('view: storage note is a non-blocking strip', () => {
  const { root, fb } = setup();
  fb.handle(fx('event.user_bootstrap.storage_failed'));
  assert.match(root.className, /note/);
  assert.doesNotMatch(root.className, /blocking/);
  assert.match(flat(root), /storage is not available/);
});

test('view: touch, click and pointer events on the overlay are swallowed', () => {
  const { root, fb } = setup();
  fb.handle(ph('prompt'));
  ['touchstart', 'touchend', 'click', 'mousedown'].forEach((t) => {
    assert.ok(root.listeners[t], t);
    const e = { cancelable: true, stopped: false, prevented: false, stopPropagation() { this.stopped = true; }, preventDefault() { this.prevented = true; } };
    root.listeners[t][0](e);
    assert.equal(e.stopped && e.prevented, true, t);
  });
});

test('document capture swallows every key event while active, and none after', () => {
  const { doc, fb } = setup();
  ['keydown', 'keypress', 'keyup'].forEach((t) => {
    const l = doc.docListeners[t];
    assert.equal(l.length, 1);
    assert.equal(l[0].capture, true);
    fb.handle(ph('prompt'));
    ['m', 'Escape', 'Enter', '1', 'a'].forEach((k) => {
      const e = evt(k);
      l[0].fn(e);
      assert.deepEqual(e.calls, ['p', 's', 'i'], t + ' ' + k);
    });
    fb.handle(ph('done'));
    const e = evt('m');
    l[0].fn(e);
    assert.deepEqual(e.calls, []);
  });
});

test('keyboard.bind: the active predicate swallows with stopPropagation and sends nothing', () => {
  let handler;
  const el = { addEventListener: (type, fn) => { handler = fn; } };
  let active = true;
  const sent = [];
  Keyboard.bind(el, () => active, (d) => sent.push(d));
  const e = evt('m');
  handler(e);
  assert.deepEqual(e.calls, ['p', 's', 'i']);
  assert.deepEqual(sent, []);
  active = false;
  handler(evt('m'));
  assert.deepEqual(sent, ['m']);
});

test('chart: ESC, buttons 1-4, Enter, knob, taps and text events are ignored while blocked', () => {
  let blocked = true;
  const h = makeHarness();
  const Chart = require('../src/interaction/menu-chart.js');
  const chart = Chart.create({ screens: h.screens, cfg: h.cfg, isBlocked: () => blocked });
  let state = chart.initial(h.domain);
  const E = h.Events;
  const feed = (ev) => { const r = chart.transition(state, ev); state = r.state; return r; };
  const before = state;
  [E.button(1, 1), E.button(1, 0), E.button(2, 1), E.button(3, 1), E.button(4, 1), E.button(5, 1), E.button(28, 1), E.button(28, 0),
    E.knob(1), E.knob(-1), E.rowTap(0), E.customTap(0, 0), E.textInput('x'), E.textEnter(), E.textBlur(), E.closeClick(),
  ].forEach((ev) => {
    const r = feed(ev);
    assert.equal(r.state, before, ev.type + ' ' + (ev.code || ''));
    assert.deepEqual(r.effects, []);
  });
  assert.equal(state.mode, Types.Mode.TERMINAL);
  blocked = false;
  feed(E.button(1, 1));
  feed(E.button(1, 0));
  assert.notEqual(state.mode, Types.Mode.TERMINAL, 'ESC opens the menu again once unblocked');
});

test('chart: wire and domain events still flow while blocked', () => {
  const h = makeHarness();
  const Chart = require('../src/interaction/menu-chart.js');
  const chart = Chart.create({ screens: h.screens, cfg: h.cfg, isBlocked: () => true });
  const s0 = chart.initial(h.domain);
  const r = chart.transition(s0, h.Events.domain(h.domain));
  assert.notEqual(r.state, undefined);
});

test('focus: no terminal refocus and no text events while blocked', () => {
  const doc = makeDoc();
  const input = doc.createElement('input');
  doc.byId.f = input;
  doc.byId.screen = { focus() {} };
  let blocked = true;
  let refocus = 0;
  const focus = Focus.create({ doc, termFocus: () => { refocus += 1; }, isBlocked: () => blocked });
  const shell = doc.createElement('div');
  focus.bindRefocus(shell, 'f');
  shell.listeners.click[0]({ target: {} });
  assert.equal(refocus, 0);
  const emitted = [];
  focus.bindTextField('f', (e) => emitted.push(e));
  input.listeners.input[0]();
  input.listeners.keydown[0]({ key: 'Enter', preventDefault() {} });
  input.listeners.blur[0]();
  assert.deepEqual(emitted, []);
  blocked = false;
  shell.listeners.click[0]({ target: {} });
  assert.equal(refocus, 1);
  input.listeners.blur[0]();
  assert.equal(emitted.length, 1);
});

test('boot: sendInput sink drops bytes while active, the boot wires isBlocked and the first-boot router', () => {
  const src = fs.readFileSync(path.join(__dirname, '..', 'src', 'boot.js'), 'utf8');
  assert.match(src, /fbActive\(\) \? false : rawSendInput/);
  assert.match(src, /isBlocked: fbActive/);
  assert.match(src, /ix\.mode !== Types\.Mode\.TERMINAL \|\| fbActive\(\)/);
});

test('model: an internal user_bootstrap error blocks, shows a retry state and asks again until a phase arrives', () => {
  const timers = [];
  let asked = 0;
  const fake = { setTimeout: (fn) => { timers.push(fn); return timers.length; }, clearTimeout: (id) => { timers[id - 1] = null; } };
  const m = FB.createModel(null, { timers: fake, retry: () => { asked += 1; }, retryMs: 10 });
  const err = fx('event.user_error');
  const internal = Object.assign({}, err, { context: 'user_bootstrap', code: 'internal', message: 'Could not read the account state' });
  assert.equal(m.handle(internal), true);
  assert.equal(m.phase(), 'retry');
  assert.equal(m.active(), true, 'input stays blocked while the state is unknown');
  assert.equal(timers.filter(Boolean).length, 1);
  const fire = timers.filter(Boolean)[0];
  timers.fill(null);
  fire();
  assert.equal(asked, 1);
  assert.equal(m.active(), true, 'still blocked until the controller answers');
  m.handle(internal); // a second failure schedules another try
  assert.equal(timers.filter(Boolean).length, 1);
  m.handle(ph('none'));
  assert.equal(m.active(), false);
  assert.equal(timers.filter(Boolean).length, 0, 'the answer cancels the retry');
  m.handle(internal);
  m.handle(ph('prompt'));
  assert.equal(m.phase(), 'prompt');
  m.connectionClosed();
  assert.equal(m.phase(), 'none');
});

test('view: the retry state is a blocking overlay with its own text', () => {
  const { root, field, fb } = setup();
  fb.handle(Object.assign({}, fx('event.user_error'), { context: 'user_bootstrap', code: 'internal' }));
  assert.match(root.className, /blocking/);
  assert.equal(root.attrs['data-phase'], 'retry');
  assert.equal(field.disabled, true);
  assert.match(flat(root), /Checking the account state/);
});

test('chart: an enter-hold timer that fires while the overlay is up does nothing', () => {
  let blocked = false;
  const h = makeHarness();
  const Chart = require('../src/interaction/menu-chart.js');
  const chart = Chart.create({ screens: h.screens, cfg: h.cfg, isBlocked: () => blocked });
  const armed = Object.assign({}, chart.initial(h.domain), { enter: 'some-hold-key' });
  blocked = true;
  const r = chart.transition(armed, h.Events.timerFired('enter-hold'));
  assert.deepEqual(r.effects, []);
  assert.equal(r.state.enter, null, 'the held push is forgotten');
});
