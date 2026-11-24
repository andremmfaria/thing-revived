'use strict';
// USB mode menu option: row, screen, reducer slice, command frames and the
// hold-to-confirm reboot. Driven through the real chart, screens and reducers;
// wire messages come from the golden fixtures.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { makeHarness } = require('./helpers/chart-harness.js');
const Reducers = require('../src/state/reducers.js');
const Config = require('../src/utils/config.js');
const Sel = require('../src/state/selectors.js');
const Bridge = require('../src/transport/bridge-client.js');

const PROTOCOL = path.resolve(__dirname, '..', '..', 'config', 'protocol', 'controller');
const fx = (name, over) => Object.assign(JSON.parse(fs.readFileSync(path.join(PROTOCOL, name + '.json'), 'utf8')), over || {});
const usbEvent = (over) => fx('event.usb_role', over);

const toast = (h) => h.state.toast;
const stack = (h) => h.state.stack.join('/');

function onUsbScreen(over) {
  const h = makeHarness();
  h.open();
  if (over) h.wire(usbEvent(over));
  h.choose('USB mode');
  return h;
}

test('the row is on the root with no Wi-Fi adapter, and shows the running role', () => {
  const h = makeHarness();
  h.open();
  assert.equal(h.view().rows.some((r) => r.label === 'USB mode: ...'), true, 'listed before the first usb_role');
  h.wire(usbEvent({ role: 'device', boot_role: null }));
  assert.ok(h.labels().includes('USB mode: Device'));
  h.wire(usbEvent({ role: 'host', boot_role: 'host' }));
  assert.ok(h.labels().includes('USB mode: Host'));
});

test('the row mentions a different persisted role, but not on an ADB build', () => {
  const h = makeHarness();
  h.open();
  h.wire(usbEvent({ role: 'device', boot_role: 'host' }));
  assert.ok(h.labels().includes('USB mode: Device (Host next boot)'));
  h.wire(usbEvent({ role: 'device', boot_role: 'host', adb_forced: true }));
  assert.ok(h.labels().includes('USB mode: Device'));
});

test('reducer: usb_role fills the slice, nulls stay null, an error leaves it alone', () => {
  const cfg = Config.create({}, () => {}).values;
  let s = Reducers.initial(cfg);
  assert.deepEqual(s.usb, { role: null, bootRole: null, adbForced: false, loaded: false });
  s = Reducers.reduce(s, usbEvent({ role: 'device', boot_role: 'host', adb_forced: true }));
  assert.deepEqual(s.usb, { role: 'device', bootRole: 'host', adbForced: true, loaded: true });
  s = Reducers.reduce(s, { type: 'usb_role', role: null, boot_role: null, adb_forced: false });
  assert.deepEqual(s.usb, { role: null, bootRole: null, adbForced: false, loaded: true });
  assert.equal(Reducers.reduce(s, fx('event.usb_role_error')), s);
});

test('selectors: names and fallbacks', () => {
  assert.equal(Sel.usbRoleName('host'), 'Host');
  assert.equal(Sel.usbRoleName('device'), 'Device');
  assert.equal(Sel.usbRoleName(null), '...');
  assert.equal(Sel.usbRoleName('otg', 'x'), 'x');
  assert.equal(Sel.usbBootRole({ adbForced: true, bootRole: 'host' }), null);
});

test('frames: builders match the shared fixtures and only the getter is queueable', () => {
  assert.deepEqual(Bridge.commands.usbRoleGet(), fx('command.usb_role_get'));
  assert.deepEqual(Bridge.commands.usbRoleSet('host'), fx('command.usb_role_set'));
  assert.deepEqual(Bridge.commands.usbRoleReboot(), fx('command.usb_role_reboot'));
  assert.equal(Bridge.QUEUEABLE.usb_role_get, 1);
  assert.equal(Bridge.QUEUEABLE.usb_role_set, undefined, 'a set must never replay after a reconnect');
  assert.equal(Bridge.QUEUEABLE.usb_role_reboot, undefined, 'a device restart must never replay after a reconnect');
  ['usb_role', 'usb_role.adb_forced', 'usb_role_error', 'usb_role_rebooting'].forEach((n) => assert.ok(Bridge.decode(fx('event.' + n)), n));
});

test('opening the screen asks for the state and lands on the first choice', () => {
  const h = makeHarness();
  h.open();
  h.wire(usbEvent({ role: 'device', boot_role: null }));
  const mark = h.mark();
  h.choose('USB mode');
  assert.equal(stack(h), 'root/usb-role');
  assert.deepEqual(h.sentSince(mark), [{ type: 'usb_role_get' }]);
  const v = h.view();
  assert.equal(v.title, 'USB mode');
  assert.deepEqual(v.rows.map((r) => r.label), [
    'Running now: Device',
    'Next boot: Device (default)',
    'Boot as Device (USB link to a PC)',
    'Boot as Host (dock: Wi-Fi, Ethernet, storage)',
    'Reboot now',
    'Host mode drops the USB link to a PC (ssh, adb) after the next boot.',
    'Recovery: run "superbird-usb-role persist device" on the device terminal, or reflash.',
  ]);
  assert.equal(v.highlight, 2);
});

test('the screen shows the running role, the persisted role and which choice is saved', () => {
  const h = onUsbScreen({ role: 'device', boot_role: 'host' });
  assert.deepEqual(h.labels().slice(0, 2), ['Running now: Device', 'Next boot: Host']);
  const rows = h.view().rows;
  assert.equal(rows[3].valueText, 'selected');
  assert.equal(rows[2].valueText, undefined);
  h.wire(usbEvent({ role: 'host', boot_role: 'device' }));
  assert.deepEqual(h.labels().slice(0, 2), ['Running now: Host', 'Next boot: Device']);
});

test('unknown roles read as "..." and an unset boot role as the device default', () => {
  const h = onUsbScreen({ role: null, boot_role: null });
  assert.deepEqual(h.labels().slice(0, 2), ['Running now: ...', 'Next boot: Device (default)']);
});

test('ADB build: the note appears and the persisted value is not shown as the next boot', () => {
  const h = onUsbScreen({ role: 'device', boot_role: 'host', adb_forced: true });
  assert.deepEqual(h.labels().slice(0, 3), ['Running now: Device', 'Next boot: Device (forced)', 'ADB build: device mode is forced at boot, the saved choice is ignored']);
  assert.equal(h.view().rows.some((r) => r.valueText === 'selected'), false);
  assert.equal(h.view().highlight, 3, 'still lands on the first choice');
});

test('picking a role sends usb_role_set only (never a live switch), stays on the screen and the event updates it', () => {
  const h = onUsbScreen({ role: 'device', boot_role: null });
  const mark = h.mark();
  h.choose('Boot as Host');
  assert.deepEqual(h.sentSince(mark), [{ type: 'usb_role_set', role: 'host' }]);
  assert.equal(stack(h), 'root/usb-role');
  h.wire(usbEvent({ role: 'device', boot_role: 'host' }));
  assert.equal(h.view().rows[3].valueText, 'selected');
  const m2 = h.mark();
  h.choose('Boot as Device');
  assert.deepEqual(h.sentSince(m2), [{ type: 'usb_role_set', role: 'device' }]);
  h.sent().forEach((m) => assert.ok(['usb_role_get', 'usb_role_set'].includes(m.type), m.type));
});

test('the restart row is hold-to-confirm: a tap or ESC tap only hints, the held push sends usb_role_reboot', () => {
  const h = onUsbScreen({ role: 'device', boot_role: 'host' });
  h.moveTo('Reboot now');
  const mark = h.mark();
  h.btn(28, 1);
  const early = h.btn(28, 0);
  assert.deepEqual(h.sentSince(mark), []);
  assert.deepEqual(early.filter((e) => e.send), []);
  assert.equal(toast(h), 'Hold the knob push to confirm');
  h.escDown();
  h.escUp();
  assert.deepEqual(h.sentSince(mark), []);
  assert.equal(toast(h), 'Hold the knob push to confirm');

  const m2 = h.mark();
  h.btn(28, 1);
  assert.deepEqual(h.sentSince(m2), [], 'not at the press');
  h.timer('enter-hold');
  assert.deepEqual(h.sentSince(m2), [{ type: 'usb_role_reboot' }]);
  h.btn(28, 0);
  assert.equal(h.sentSince(m2).length, 1, 'the release adds nothing');
  assert.equal(stack(h), 'root/usb-role');
});

test('a stale hold timer after moving to another row sends nothing', () => {
  const h = onUsbScreen({ role: 'device', boot_role: null });
  h.moveTo('Reboot now');
  h.btn(28, 1);
  h.knob(-1);
  const mark = h.mark();
  h.timer('enter-hold');
  assert.deepEqual(h.sentSince(mark), []);
});

test('the hint line says the choice applies from the next boot', () => {
  const h = onUsbScreen({ role: 'device', boot_role: null });
  assert.match(h.view().hint, /applies from the next boot/);
  assert.match(h.view().hint, /knob push \(hold\): reboot/);
});

test('errors and the restart announcement show as toasts', () => {
  const h = onUsbScreen({ role: 'device', boot_role: null });
  h.wire(fx('event.usb_role_error', { message: 'superbird-usb-role persist host failed: read-only' }));
  assert.equal(toast(h), 'USB mode: superbird-usb-role persist host failed: read-only');
  h.wire(fx('event.usb_role_rebooting'));
  assert.equal(toast(h), 'Rebooting...');
});

test('the screen fits 800x480: short single-line rows, at most nine of them', () => {
  const h = onUsbScreen({ role: 'device', boot_role: null, adb_forced: true });
  h.labels().forEach((l) => assert.ok(l.length <= 90, l.length + ': ' + l));
  assert.ok(h.view().rows.length <= 9);
});
