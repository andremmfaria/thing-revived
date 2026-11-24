'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const L = require('../src/device-settings/device-settings-logic.js');
const Types = require('../src/utils/types.js');

test('every device_settings_error code has plain words; unknown falls back to the message, then a generic line', () => {
  Object.keys(Types.DeviceErrorCode).forEach((c) => {
    assert.ok(L.ERROR_TEXT[c] && L.errorText(c, 'raw') === L.ERROR_TEXT[c], c);
  });
  assert.equal(Object.keys(L.ERROR_TEXT).length, Object.keys(Types.DeviceErrorCode).length);
  assert.equal(L.errorText('future_code', 'From the controller'), 'From the controller');
  assert.equal(L.errorText('future_code'), 'Could not complete the request');
  assert.equal(L.errorText(undefined, ''), 'Could not complete the request');
});

test('formatCountdown is mm:ss, clamps at zero, and tolerates unknown input', () => {
  assert.equal(L.formatCountdown(1800), '30:00');
  assert.equal(L.formatCountdown(1721), '28:41');
  assert.equal(L.formatCountdown(59), '00:59');
  assert.equal(L.formatCountdown(0), '00:00');
  assert.equal(L.formatCountdown(-5), '00:00');
  assert.equal(L.formatCountdown(null), '--:--');
  assert.equal(L.formatCountdown(NaN), '--:--');
});

test('labels, next values and the unlock row label', () => {
  assert.equal(L.valueLabel('mode', 'host'), 'Host');
  assert.equal(L.valueLabel('mode', 'device'), 'Device');
  assert.equal(L.valueLabel('adb', 'on'), 'ON');
  assert.equal(L.valueLabel('ssh', 'off'), 'OFF');
  assert.equal(L.valueLabel('ssh', null), '...');
  assert.equal(L.settingName('uart'), 'UART');
  assert.equal(L.nextValue('mode', 'host'), 'device');
  assert.equal(L.nextValue('mode', 'device'), 'host');
  assert.equal(L.nextValue('ssh', 'on'), 'off');
  assert.equal(L.nextValue('uart', 'off'), 'on');
  assert.equal(L.nextValue('adb', null), null);
  assert.equal(L.unlockLabel(false, 100), 'Unlock');
  assert.equal(L.unlockLabel(true, 1721), 'Lock now (28:41 left)');
  assert.equal(L.unlockLabel(true, null), 'Lock now');
});

const dev = (over) => Object.assign({
  live: { mode: 'device', ssh: 'on', adb: 'off', uart: 'off' },
  persisted: { mode: 'device', ssh: 'on', adb: 'off', uart: 'off' },
  adbBlocked: null, busy: null,
}, over);

test('usbNote shows one note in priority order: busy, adb blocked, root surface, ssh off', () => {
  assert.equal(L.usbNote(dev({})), null);
  assert.equal(L.usbNote(dev({ busy: 'adb', adbBlocked: 'mode_host' })), 'Switching, link may drop about 10 s');
  assert.equal(L.usbNote(dev({ adbBlocked: 'mode_host', live: { mode: 'host', ssh: 'on', adb: 'off', uart: 'on' } })), 'ADB needs Device mode');
  assert.equal(L.usbNote(dev({ live: { mode: 'device', ssh: 'off', adb: 'on', uart: 'off' }, persisted: { mode: 'device', ssh: 'off', adb: 'on', uart: 'off' } })), 'ADB or UART is a root shell with no password');
  assert.equal(L.usbNote(dev({ persisted: { mode: 'device', ssh: 'off', adb: 'off', uart: 'off' }, live: { mode: 'device', ssh: 'off', adb: 'off', uart: 'off' } })), 'SSH off: no remote login');
});

test('driftNote names the first setting whose live value differs, never while busy', () => {
  assert.equal(L.driftNote(dev({})), null);
  const drift = dev({ live: { mode: 'device', ssh: 'on', adb: 'off', uart: 'off' }, persisted: { mode: 'device', ssh: 'on', adb: 'on', uart: 'off' } });
  assert.equal(L.driftNote(drift), 'ADB is OFF now, saved as ON');
  assert.equal(L.driftNote(Object.assign({}, drift, { busy: 'adb' })), null);
});

test('no em or en dashes in the logic module', () => {
  const src = require('fs').readFileSync(require.resolve('../src/device-settings/device-settings-logic.js'), 'utf8');
  assert.ok(!/[\u2013\u2014]/.test(src));
});
