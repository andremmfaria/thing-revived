'use strict';
// Debug access settings screen (T-D4.1/T-D4.2): reducer/selector slice, the
// pure confirm-string and error-text helpers, the hold-to-confirm toggle
// rows, the Users unlock gate, the host-role block on ADB, the persistent
// error line and the badge visibility selector. Wire messages come from the
// shared golden fixtures, like every other screen's tests.
//
// Naming note (config/protocol/controller/README.md): the implementing
// controller task fixed the wire family as `debug_get`/`debug_set` and
// `debug_state`/`debug_error` with a `surface` field, not the plan text's
// `debug_access_*` names and `kind` field. This file (and the screen it
// tests) follows the shipped names.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { makeHarness } = require('./helpers/chart-harness.js');
const Reducers = require('../src/state/reducers.js');
const Config = require('../src/utils/config.js');
const Sel = require('../src/state/selectors.js');
const Bridge = require('../src/transport/bridge-client.js');
const DL = require('../src/debug-access/debug-access-logic.js');

const PROTOCOL = path.resolve(__dirname, '..', '..', 'config', 'protocol', 'controller');
const fx = (name, over) => Object.assign(JSON.parse(fs.readFileSync(path.join(PROTOCOL, name + '.json'), 'utf8')), over || {});
const debugEvent = (over) => fx('event.debug_state', over);
const debugError = (over) => fx('event.debug_error', over);

const toast = (h) => h.state.toast;
const stack = (h) => h.state.stack.join('/');

function unlock(h) {
  h.wire({ type: 'user_session', unlocked: true, username: 'kiosk' });
}

function onDebugScreen(over) {
  const h = makeHarness();
  h.open();
  unlock(h);
  if (over) h.wire(debugEvent(over));
  h.choose('Debug access');
  return h;
}

test('reducer: debug_state fills the flat slice, debug_error sets a persistent error, the local action clears it', () => {
  const cfg = Config.create({}, () => {}).values;
  let s = Reducers.initial(cfg);
  assert.deepEqual(s.debugAccess, { adb: null, uart: null, adbBuildDefault: null, uartBuildDefault: null, blocked: null, error: null });
  s = Reducers.reduce(s, debugEvent({ adb: true, uart: false, adb_build_default: false, uart_build_default: true, blocked: null }));
  assert.deepEqual(s.debugAccess, { adb: true, uart: false, adbBuildDefault: false, uartBuildDefault: true, blocked: null, error: null });
  s = Reducers.reduce(s, debugError({ code: 'usb_role_host', message: 'x' }));
  assert.deepEqual(s.debugAccess.error, { context: 'debug_set', code: 'usb_role_host', message: 'x' });
  // A fresh debug_state does not clear a standing error by itself.
  s = Reducers.reduce(s, debugEvent());
  assert.ok(s.debugAccess.error);
  s = Reducers.reduce(s, { type: 'local/debug_clear_error' });
  assert.equal(s.debugAccess.error, null);
});

test('selectors: badge activity, the host-role block (debug_state.blocked), and the error passthrough', () => {
  const cfg = Config.create({}, () => {}).values;
  let s = Reducers.initial(cfg);
  assert.equal(Sel.debugAccessActive(s), false);
  s = Reducers.reduce(s, debugEvent({ adb: true }));
  assert.equal(Sel.debugAccessActive(s), true);
  assert.equal(Sel.debugAccessAdbBlocked(s), false);
  s = Reducers.reduce(s, fx('event.debug_state.blocked_host'));
  assert.equal(Sel.debugAccessAdbBlocked(s), true);
  s = Reducers.reduce(s, debugError({ code: 'not_unlocked' }));
  const fixture = fx('event.debug_error', { code: 'not_unlocked' });
  assert.deepEqual(Sel.debugAccessError(s), { context: fixture.context, code: fixture.code, message: fixture.message });
});

test('pure logic: confirm strings equal the surface name, error text covers every shipped code and falls back', () => {
  assert.deepEqual(DL.CONFIRM, { adb: 'adb', uart: 'uart' });
  assert.equal(DL.errorText('usb_role_host'), 'USB role is host: ADB needs device mode');
  assert.equal(DL.errorText('not_unlocked'), 'Admin session ended. Unlock again');
  assert.equal(DL.errorText('confirm_required'), 'Confirmation needed');
  assert.equal(DL.errorText('unknown_code', 'controller said so'), 'controller said so');
  assert.equal(DL.errorText('unknown_code'), 'Could not complete the request');
});

test('pure logic: every shipped wire error code maps to its own non generic text', () => {
  const GENERIC = 'Could not complete the request';
  const SHIPPED_CODES = [
    'not_unlocked', 'confirm_required', 'invalid_surface', 'usb_role_host',
    'toggle_failed', 'busy', 'unavailable', 'rate_limited',
  ];
  const seen = new Set();
  SHIPPED_CODES.forEach((code) => {
    const text = DL.errorText(code);
    assert.notEqual(text, GENERIC, `code ${code} must not fall back to the generic message`);
    assert.ok(text && text.length, `code ${code} must map to non empty text`);
    seen.add(text);
  });
  assert.equal(seen.size, SHIPPED_CODES.length, 'each shipped code must map to distinct text');
});

test('frames: builders match the shared fixtures, the getter is queueable, the setter never replays', () => {
  assert.deepEqual(Bridge.commands.debugGet(), fx('command.debug_get'));
  assert.deepEqual(Bridge.commands.debugSet('adb', true, 'adb'), fx('command.debug_set'));
  assert.deepEqual(Bridge.commands.debugSet('uart', false, 'uart'), fx('command.debug_set.disable'));
  assert.equal(Bridge.QUEUEABLE.debug_get, 1);
  assert.equal(Bridge.QUEUEABLE.debug_set, undefined, 'a mutating command must never replay after a reconnect');
  assert.ok(Bridge.decode(debugEvent()));
  assert.ok(Bridge.decode(debugError()));
});

test('root: Debug access is always listed, independent of USB role or lock state', () => {
  const h = makeHarness();
  h.open();
  assert.ok(h.labels().includes('Debug access'));
});

test('locked: entering shows the Users locked notice, not the toggle rows', () => {
  const h = makeHarness();
  h.open();
  h.choose('Debug access');
  assert.deepEqual(h.labels(), ['Admin session is locked', 'Unlock...']);
});

test('unlocked: requests debug_get on entry and shows both surfaces off', () => {
  const h = makeHarness();
  h.open();
  unlock(h);
  const mark = h.mark();
  h.choose('Debug access');
  assert.deepEqual(h.sentSince(mark), [{ type: 'debug_get' }]);
  assert.deepEqual(h.labels().slice(0, 4), [
    'ADB: ... (build default: OFF)', 'Enable ADB',
    'UART console: ... (build default: OFF)', 'Enable UART console',
  ]);
});

test('state is never optimistic: only a fresh debug_state flips the shown ON/OFF', () => {
  const h = onDebugScreen({ adb: false });
  h.moveTo('Enable ADB');
  h.btn(28, 1);
  h.timer('enter-hold');
  // The toggle row was held and sent, but the label does not flip until the event arrives.
  assert.ok(h.labels().includes('ADB: OFF (build default: OFF)'));
  h.btn(28, 0);
  h.wire(debugEvent({ adb: true }));
  assert.ok(h.labels().includes('ADB: ON (build default: OFF)'));
});

test('hold-to-confirm: a tap or ESC tap sends nothing and hints; the held push sends debug_set with the fixed confirm', () => {
  const h = onDebugScreen({ adb: false });
  h.moveTo('Enable ADB');
  const mark = h.mark();
  h.btn(28, 1);
  h.btn(28, 0);
  assert.deepEqual(h.sentSince(mark), []);
  assert.equal(toast(h), 'Hold the knob push to confirm');
  h.escDown();
  h.escUp();
  assert.deepEqual(h.sentSince(mark), []);

  const m2 = h.mark();
  h.btn(28, 1);
  assert.deepEqual(h.sentSince(m2), [], 'not at the press');
  h.timer('enter-hold');
  assert.deepEqual(h.sentSince(m2), [{ type: 'debug_set', surface: 'adb', enabled: true, confirm: 'adb' }]);
  h.btn(28, 0);
  assert.equal(h.sentSince(m2).length, 1, 'the release adds nothing');
  assert.equal(stack(h), 'root/debug-access');
});

test('toggling off sends enabled:false with the uart confirm', () => {
  const h = onDebugScreen({ uart: true });
  h.moveTo('Disable UART console');
  const mark = h.mark();
  h.btn(28, 1);
  h.timer('enter-hold');
  assert.deepEqual(h.sentSince(mark), [{ type: 'debug_set', surface: 'uart', enabled: false, confirm: 'uart' }]);
  h.btn(28, 0);
});

test('a stale hold timer after moving to another row sends nothing', () => {
  const h = onDebugScreen({ adb: false });
  h.moveTo('Enable ADB');
  h.btn(28, 1);
  h.knob(1);
  const mark = h.mark();
  h.timer('enter-hold');
  assert.deepEqual(h.sentSince(mark), []);
});

test('ADB blocked (debug_state.blocked): a disabled note replaces the toggle, UART is unaffected', () => {
  const h = onDebugScreen({ adb: false });
  h.wire(fx('event.debug_state.blocked_host'));
  const labels = h.labels();
  assert.ok(labels.includes('ADB: switch USB mode to Device to use it'));
  assert.ok(!labels.some((l) => l.indexOf('ADB') === 0 && l.indexOf('Enable') !== -1));
  assert.ok(labels.includes('Enable UART console'));
});

test('an error shows as a persistent note, named in words, not a toast; cleared on re-entry', () => {
  const h = onDebugScreen({ adb: false });
  h.wire(debugError({ code: 'usb_role_host' }));
  assert.ok(h.labels().includes('USB role is host: ADB needs device mode'));
  assert.equal(toast(h), null, 'not a toast');
  h.back();
  h.choose('Debug access');
  assert.ok(!h.labels().includes('USB role is host: ADB needs device mode'), 'cleared on re-entry');
});

test('the warning lines are always on the screen', () => {
  const h = onDebugScreen();
  const labels = h.labels();
  assert.ok(labels.some((l) => /unauthenticated root shell/.test(l)));
  assert.ok(labels.some((l) => /drop the USB link for about 10 seconds/.test(l)));
});
