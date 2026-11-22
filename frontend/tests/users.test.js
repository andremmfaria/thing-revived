'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const Types = require('../src/utils/types.js');
const Logging = require('../src/utils/logging.js');
const Bridge = require('../src/transport/bridge-client.js');
const Reducers = require('../src/state/reducers.js');
const Config = require('../src/utils/config.js');
const UL = require('../src/users/users-logic.js');
const SecretView = require('../src/users/secret-view.js');
const { makeHarness } = require('./helpers/chart-harness.js');

const PROTOCOL = path.resolve(__dirname, '..', '..', 'config', 'protocol', 'controller');
const fx = (name) => JSON.parse(fs.readFileSync(path.join(PROTOCOL, name + '.json'), 'utf8'));
const SECRET = fx('event.user_bootstrap_secret').password;
const PENDING = fx('event.user_bootstrap');
const DONE = fx('event.user_bootstrap.none');
const BAD_ACK = fx('event.user_error');
const ENTER = 28;

// ---- builders, decoders, logging ----------------------------------------------------
test('users: command builders match the golden fixtures', () => {
  assert.deepEqual(Bridge.commands.userBootstrapGet(), fx('command.user_bootstrap_get'));
  assert.deepEqual(Bridge.commands.userBootstrapReveal(), fx('command.user_bootstrap_reveal'));
  const ack = fx('command.user_bootstrap_ack');
  assert.deepEqual(Bridge.commands.userBootstrapAck(ack.last4), ack);
});

test('users: every user event fixture decodes to a wire event', () => {
  ['event.user_bootstrap', 'event.user_bootstrap.none', 'event.user_bootstrap_secret', 'event.user_error'].forEach((n) => {
    const m = fx(n);
    assert.deepEqual(Bridge.decode(m), { type: 'wire', msg: m }, n);
  });
  assert.equal(Bridge.QUEUEABLE.user_bootstrap_get, 1);
  assert.equal(Bridge.QUEUEABLE.user_bootstrap_reveal, undefined, 'a reveal needs a fresh knob push, never replayed');
  assert.equal(Bridge.QUEUEABLE.user_bootstrap_ack, undefined);
});

test('users: logging redacts password, last4 and current_password everywhere', () => {
  const l = Logging.create({ level: 'trace', sink: null });
  l.info('got', fx('event.user_bootstrap_secret'));
  l.info('sent', fx('command.user_bootstrap_ack'));
  l.info('nested', { x: [{ current_password: 'hunter2hunter2', last4: 'abcd' }] });
  const text = l.dump().map((e) => e.text).join('\n');
  [SECRET, 'k7Qz', 'hunter2hunter2', 'abcd'].forEach((s) => assert.equal(text.indexOf(s), -1, 'leaked ' + s));
  assert.match(Logging.describe(fx('command.user_bootstrap_ack')), /"last4":"\[redacted\]"/);
  assert.match(Logging.describe(fx('event.user_bootstrap_secret')), /"expires_in_secs":885/);
  assert.equal(Logging.describe(fx('event.user_bootstrap_secret')).indexOf(SECRET), -1);
});

// ---- reducer ---------------------------------------------------------------------------
test('users reducer: bootstrap status is kept, the password never reaches the store', () => {
  const cfg = Config.create({}, () => {}).values;
  let s = Reducers.initial(cfg);
  assert.equal(s.users.bootstrap, null);
  const s1 = Reducers.reduce(s, PENDING);
  assert.deepEqual(s1.users.bootstrap, { pending: true, username: 'kiosk', acknowledged: false, expires_in_secs: 900, storage_ok: true });
  assert.equal(Reducers.reduce(s1, DONE).users.bootstrap.expires_in_secs, null);
  assert.equal(Reducers.reduce(s1, fx('event.user_bootstrap_secret')), s1, 'secret event leaves the state untouched');
  assert.equal(Reducers.reduce(s1, BAD_ACK), s1);
  assert.equal(s.users.bootstrap, null, 'previous state not mutated');
  assert.equal(JSON.stringify(s1).indexOf(SECRET), -1);
});

// ---- logic ---------------------------------------------------------------------------------
test('users logic: groups, countdown, phase, error text', () => {
  assert.deepEqual(UL.groups(SECRET), ['Xe4m', 'H7pQ', '9tLw', 'Zc3K']);
  assert.equal(UL.countdown(885), '14:45');
  assert.equal(UL.countdown(5), '0:05');
  assert.equal(UL.countdown(null), '--:--');
  assert.equal(UL.phase(null), 'unknown');
  assert.equal(UL.phase({ pending: true, acknowledged: false }), 'pending');
  assert.equal(UL.phase({ pending: false, acknowledged: true }), 'done');
  assert.equal(UL.phase({ pending: false, acknowledged: false }), 'expired');
  Types.values(Types.UserErrorCode).forEach((c) => assert.ok(UL.errorText(c, 'ignored').indexOf('ignored') === -1, c));
  assert.equal(UL.errorText('weird', 'Controller says no'), 'Controller says no');
  assert.equal(UL.errorText('weird'), 'Could not complete the request');
  assert.equal(UL.ackDisplay('k7'), 'k7__');
});

test('users logic: ackApply caps at four characters, DEL removes, OK finishes', () => {
  const WL = require('../src/wifi/wifi-logic.js');
  const idx = (c) => WL.PICKER_RING.indexOf(c);
  let e = '';
  'k7Qz'.split('').forEach((c) => { e = UL.ackApply(e, idx(c)).entry; });
  assert.equal(e, 'k7Qz');
  assert.equal(UL.ackApply(e, idx('a')).entry, 'k7Qz', 'fifth character ignored');
  assert.equal(UL.ackApply(e, idx(WL.TOKEN_DEL)).entry, 'k7Q');
  assert.deepEqual(UL.ackApply(e, idx(WL.TOKEN_DONE)), { entry: 'k7Qz', done: true });
});

// ---- screens ----------------------------------------------------------------------------------
function pickChars(h, text) {
  const WL = require('../src/wifi/wifi-logic.js');
  text.split('').forEach((c) => {
    const target = WL.PICKER_RING.indexOf(c);
    for (let i = 0; i < 200 && h.state.ui.users.ackIdx !== target; i++) {
      const d = (target - h.state.ui.users.ackIdx + WL.PICKER_RING.length) % WL.PICKER_RING.length;
      h.knob(d <= WL.PICKER_RING.length / 2 ? 1 : -1);
    }
    h.btn(ENTER, 1);
    h.btn(ENTER, 0);
  });
}

function revealed() {
  const h = makeHarness();
  h.wire(PENDING);
  h.choose('Press the knob');
  h.wire(fx('event.user_bootstrap_secret'));
  return h;
}

test('first boot: a pending bootstrap opens the blocking screen from the closed menu', () => {
  const h = makeHarness();
  assert.equal(h.state.mode, Types.Mode.TERMINAL);
  h.wire(PENDING);
  assert.equal(h.state.mode, Types.Mode.LIST);
  assert.deepEqual(h.state.stack, ['root', 'users-bootstrap']);
  assert.match(h.view().title, /First boot/);
  assert.deepEqual(h.labels(), ['Press the knob to reveal the password']);
  assert.equal(h.view().custom, 'users-bootstrap');
});

test('first boot: already confirmed or no event means no screen', () => {
  const h = makeHarness();
  h.wire(DONE);
  assert.equal(h.state.mode, Types.Mode.TERMINAL);
  assert.equal(h.labels().indexOf('First-boot admin password'), -1);
});

test('first boot: opening over another screen leaves it first', () => {
  const h = makeHarness();
  h.open();
  h.choose('Terminal');
  h.wire(PENDING);
  assert.deepEqual(h.state.stack, ['root', 'users-bootstrap']);
});

test('first boot: pressing the knob row sends one reveal, the secret arrives into ui only', () => {
  const h = makeHarness();
  h.wire(PENDING);
  const mark = h.mark();
  h.choose('Press the knob');
  assert.deepEqual(h.sentSince(mark), [{ type: 'user_bootstrap_reveal' }]);
  assert.equal(h.state.ui.users.secret, null);
  h.wire(fx('event.user_bootstrap_secret'));
  assert.equal(h.state.ui.users.secret, SECRET);
  assert.equal(h.state.ui.users.secretLeft, 885);
  assert.equal(JSON.stringify(h.domain).indexOf(SECRET), -1, 'domain store never holds it');
  assert.ok(h.effects.some((e) => e.startTimer && e.startTimer.name === 'users-tick'));
  assert.deepEqual(h.labels(), ['Shown once. Expires in 14:45', 'I have saved it', 'Hide password']);
  assert.equal(h.view().highlight, 1);
  const rendered = JSON.stringify(h.view().rows);
  assert.equal(rendered.indexOf(SECRET), -1, 'rows never carry the password (only the custom view does)');
});

test('first boot: a secret that arrives after the user left is ignored', () => {
  const h = makeHarness();
  h.wire(PENDING);
  h.choose('Press the knob');
  h.escDown();
  h.timer('esc-hold');
  assert.deepEqual(h.state.stack, ['root']);
  h.wire(fx('event.user_bootstrap_secret'));
  assert.equal(h.state.ui.users.secret, null);
});

test('first boot: countdown ticks, then the secret is hidden and the status refreshed', () => {
  const h = revealed();
  h.timer('users-tick');
  assert.equal(h.state.ui.users.secretLeft, 884);
  assert.ok(h.effects.slice(-1)[0].startTimer.name === 'users-tick');
  h.state = Object.assign({}, h.state, { ui: Object.assign({}, h.state.ui, { users: Object.assign({}, h.state.ui.users, { secretLeft: 1 }) }) });
  const mark = h.mark();
  h.timer('users-tick');
  assert.equal(h.state.ui.users.secret, null);
  assert.deepEqual(h.sentSince(mark), [{ type: 'user_bootstrap_get' }]);
  assert.match(h.state.toast, /Time ran out/);
  h.timer('users-tick');
  assert.equal(h.state.ui.users.secret, null, 'a stray tick after hiding does nothing');
});

test('first boot: leaving the screen wipes the secret, also by closing the menu', () => {
  const h = revealed();
  h.escDown();
  h.timer('esc-hold');
  assert.equal(h.state.ui.users.secret, null);
  const h2 = revealed();
  h2.send(h2.Events.closeClick());
  assert.equal(h2.state.mode, Types.Mode.TERMINAL);
  assert.equal(h2.state.ui.users.secret, null);
  assert.ok(h2.effects.some((e) => e.cancelTimer === 'users-tick'));
});

test('first boot: Hide password wipes it but keeps the screen', () => {
  const h = revealed();
  h.choose('Hide password');
  assert.equal(h.state.ui.users.secret, null);
  assert.deepEqual(h.state.stack, ['root', 'users-bootstrap']);
  assert.deepEqual(h.labels(), ['Press the knob to reveal the password']);
});

test('first boot: a bridge disconnect wipes the secret, also from the confirm screen', () => {
  const h = revealed();
  h.choose('I have saved it');
  assert.equal(h.state.stack[h.state.stack.length - 1], 'users-ack');
  h.wire({ type: 'connection', which: 'bridge', status: 'disconnected' });
  assert.equal(h.state.ui.users.secret, null);
  assert.equal(h.state.stack[h.state.stack.length - 1], 'users-bootstrap');
  h.wire({ type: 'connection', which: 'agent', status: 'disconnected' });
});

test('first boot: a reconnect with the secret pending shows the reveal row again', () => {
  const h = revealed();
  h.wire({ type: 'connection', which: 'bridge', status: 'disconnected' });
  h.wire(PENDING);
  assert.deepEqual(h.labels(), ['Press the knob to reveal the password']);
});

test('confirm: four characters then OK send the ack with last4; the secret stays until the controller agrees', () => {
  const h = revealed();
  h.choose('I have saved it');
  assert.equal(h.view().custom, 'users-ack');
  assert.equal(h.state.mode, Types.Mode.PICKER);
  const mark = h.mark();
  pickChars(h, 'k7Qz');
  assert.equal(h.state.ui.users.ack, 'k7Qz');
  assert.deepEqual(h.sentSince(mark), []);
  while (h.state.ui.users.ackIdx !== 0) h.knob(-1);
  h.btn(ENTER, 1);
  assert.deepEqual(h.sentSince(mark), [fx('command.user_bootstrap_ack')]);
  assert.equal(h.state.ui.users.secret, SECRET);
});

test('confirm: OK with fewer than four characters only toasts', () => {
  const h = revealed();
  h.choose('I have saved it');
  pickChars(h, 'k7');
  while (h.state.ui.users.ackIdx !== 0) h.knob(-1);
  const mark = h.mark();
  h.btn(ENTER, 1);
  assert.deepEqual(h.sentSince(mark), []);
  assert.match(h.state.toast, /all 4/);
});

test('confirm: bad_ack shows the message, clears the entry and keeps the secret', () => {
  const h = revealed();
  h.choose('I have saved it');
  pickChars(h, 'k7Qz');
  h.wire(BAD_ACK);
  assert.equal(h.state.ui.users.secret, SECRET);
  assert.equal(h.state.ui.users.ack, '');
  assert.match(h.state.toast, /do not match/);
  assert.equal(h.state.stack[h.state.stack.length - 1], 'users-ack');
});

test('confirm: leaving the confirm screen clears what was typed, the secret stays', () => {
  const h = revealed();
  h.choose('I have saved it');
  pickChars(h, 'k7');
  h.escDown();
  h.timer('esc-hold');
  assert.equal(h.state.ui.users.ack, '');
  assert.equal(h.state.ui.users.secret, SECRET);
});

test('confirm: the acknowledged status wipes everything and returns to the root menu', () => {
  const h = revealed();
  h.choose('I have saved it');
  pickChars(h, 'k7Qz');
  h.wire(DONE);
  assert.equal(h.state.ui.users.secret, null);
  assert.equal(h.state.ui.users.ack, '');
  assert.deepEqual(h.state.stack, ['root']);
  assert.match(h.state.toast, /confirmed/);
  assert.equal(JSON.stringify(h.state).indexOf(SECRET), -1);
});

test('errors: press_required, unavailable and unknown codes show clear messages', () => {
  const h = makeHarness();
  h.wire(PENDING);
  h.choose('Press the knob');
  h.wire({ type: 'user_error', context: 'user_bootstrap_reveal', code: 'press_required', message: 'x' });
  assert.deepEqual(h.labels(), [UL.errorText('press_required'), 'Press the knob to reveal the password']);
  assert.match(h.state.toast, /Press the knob/);
  h.wire({ type: 'user_error', context: 'user_bootstrap_reveal', code: 'unavailable', message: 'x' });
  assert.equal(h.labels()[0], UL.errorText('unavailable'));
  h.wire({ type: 'user_error', context: 'user_bootstrap_get', code: 'brand_new', message: 'Controller says no' });
  assert.equal(h.labels()[0], 'Controller says no');
  const t = makeHarness();
  t.wire({ type: 'user_error', context: 'user_unlock', code: 'bad_password', message: 'nope' });
  assert.equal(t.state.toast, 'nope');
  assert.equal(t.state.mode, Types.Mode.TERMINAL, 'a non first-boot error does not open the menu');
});

test('expiry: not pending and not acknowledged shows the reboot message and a root row', () => {
  const h = revealed();
  h.wire({ type: 'user_bootstrap', pending: false, username: 'kiosk', acknowledged: false, expires_in_secs: null, storage_ok: true });
  assert.equal(h.state.ui.users.secret, null);
  assert.deepEqual(h.labels(), ['Admin password was not collected.', 'Reboot to generate a new one.']);
  h.escDown();
  h.timer('esc-hold');
  assert.equal(h.labels()[0], 'First-boot admin password');
});

test('storage_ok false is shown on the screen', () => {
  const h = makeHarness();
  h.wire(Object.assign({}, PENDING, { storage_ok: false }));
  assert.equal(h.labels()[0], 'Account storage is not available');
});

test('dismissed screen is not reopened by later bootstrap events; the root row returns to it', () => {
  const h = makeHarness();
  h.wire(PENDING);
  h.escDown();
  h.timer('esc-hold');
  h.wire(PENDING);
  assert.deepEqual(h.state.stack, ['root']);
  h.moveTo('First-boot admin password');
  const mark = h.mark();
  h.push();
  assert.deepEqual(h.state.stack, ['root', 'users-bootstrap']);
  assert.deepEqual(h.sentSince(mark), [{ type: 'user_bootstrap_get' }]);
});

test('after a page reload the state starts empty and a fresh status asks for a new reveal', () => {
  const h = makeHarness();
  assert.equal(h.state.ui.users.secret, null);
  h.wire(PENDING);
  assert.deepEqual(h.labels(), ['Press the knob to reveal the password']);
});

// ---- DOM half --------------------------------------------------------------------------------------
function fakeDoc() {
  const mk = (tag) => ({
    tag, className: '', textContent: '', children: [], listeners: {},
    appendChild(c) { this.children.push(c); return c; },
    addEventListener(n, fn) { this.listeners[n] = fn; },
  });
  return { createElement: mk };
}
const flat = (el) => [el.textContent].concat(...el.children.map(flat)).join('|');

test('secret view draws four groups as text spans, never an input', () => {
  const doc = fakeDoc();
  const views = SecretView.create(doc);
  const list = doc.createElement('div');
  const ctx = { ui: { users: { secret: SECRET, ack: '', ackIdx: 2 } }, domain: { users: { bootstrap: PENDING } } };
  views.bootstrap(list, { ctx });
  const text = flat(list);
  ['Xe4m', 'H7pQ', '9tLw', 'Zc3K', 'kiosk'].forEach((g) => assert.ok(text.indexOf(g) !== -1, g));
  const tags = [];
  (function walk(e) { tags.push(e.tag); e.children.forEach(walk); })(list);
  assert.equal(tags.indexOf('input'), -1);
  const empty = doc.createElement('div');
  views.bootstrap(empty, { ctx: { ui: { users: { secret: null } }, domain: { users: { bootstrap: PENDING } } } });
  assert.equal(flat(empty).indexOf('Xe4m'), -1, 'no secret in state, none in the DOM');
});

test('secret view: the confirm body shows typed characters and a tappable strip', () => {
  const doc = fakeDoc();
  const views = SecretView.create(doc);
  const list = doc.createElement('div');
  const emitted = [];
  views.ack(list, { ctx: { ui: { users: { ack: 'k7', ackIdx: 2 } } } }, (e) => emitted.push(e));
  assert.ok(flat(list).indexOf('k7__') !== -1);
  const strip = list.children.find((c) => c.className === 'wifi-picker-strip');
  assert.equal(strip.children.length, 7);
  strip.children[4].listeners.click();
  assert.equal(emitted[0].type, 'customTap');
});
