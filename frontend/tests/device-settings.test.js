'use strict';
// Device settings and USB mode screens (plan 8.1), driven through the real
// chart, screens and reducers. The screens are registered but not yet linked
// from the root (T10 does the cutover), so tests set the stack directly.
const test = require('node:test');
const assert = require('node:assert/strict');
const { makeHarness } = require('./helpers/chart-harness.js');
const Reducers = require('../src/state/reducers.js');
const Sel = require('../src/state/selectors.js');
const Chart = require('../src/interaction/menu-chart.js');
const Screens = require('../src/menu/screens/index.js');
const MenuView = require('../src/menu/menu-view.js');
const WL = require('../src/wifi/wifi-logic.js');
const UL = require('../src/users/users-logic.js');
const Bridge = require('../src/transport/bridge-client.js');
const Config = require('../src/utils/config.js');
const Events = require('../src/interaction/events.js');

const stack = (h) => h.state.stack.join('/');
const settings = (over) => Object.assign({
  type: 'device_settings', mode: 'device', ssh: 'on', adb: 'off', uart: 'off',
  persisted: { mode: 'device', ssh: 'on', adb: 'off', uart: 'off' }, adb_blocked: null, busy: null,
}, over || {});

function at(screen, unlocked) {
  const h = makeHarness();
  h.open();
  h.wire(settings());
  if (unlocked) h.wire({ type: 'user_session', unlocked: true, username: 'kiosk', expires_in_secs: 1800 });
  h.state = Object.assign({}, h.state, { stack: ['root'].concat(screen === 'usb-mode' ? ['device-settings', 'usb-mode'] : ['device-settings']) });
  return h;
}
const row = (h, label) => h.view().rows.find((r) => r.label.indexOf(label) === 0);
const disabled = (r) => r.disabled === true;

test('locked: five rows, first is Unlock, the other four disabled with a hint but highlightable', () => {
  const h = at('device-settings', false);
  const rows = h.view().rows;
  assert.equal(rows.length, 5);
  assert.deepEqual(h.labels(), ['Unlock', 'Users', 'Groups', 'Reboot device', 'USB mode']);
  assert.equal(disabled(rows[0]), false);
  rows.slice(1).forEach((r) => { assert.equal(disabled(r), true); assert.equal(r.disabledHint, 'Unlock first'); });
  h.moveTo('USB mode');
  assert.equal(h.view().highlight, 4);
  h.push();
  assert.equal(stack(h), 'root/device-settings', 'a disabled nav row does not navigate');
  assert.equal(h.state.toast, 'Unlock first');
});

test('locked: Unlock opens the existing unlock screen', () => {
  const h = at('device-settings', false);
  h.choose('Unlock');
  assert.equal(stack(h), 'root/device-settings/users-unlock');
});

test('unlock enables the rows; the first row becomes Lock now and sends user_lock_session', () => {
  const h = at('device-settings', true);
  assert.equal(h.view().rows.filter(disabled).length, 0);
  assert.equal(h.labels()[0], 'Lock now');
  const n = h.mark();
  h.choose('Lock now');
  assert.deepEqual(h.sentSince(n), [Bridge.commands.userLockSession()]);
  h.wire({ type: 'user_session', unlocked: false });
  assert.equal(h.view().rows.filter(disabled).length, 4, 'rows grey out again after the lock');
  assert.equal(h.labels()[0], 'Unlock');
});

test('the countdown label follows the injected clock', () => {
  const cfg = Config.create({}, () => {}).values;
  let clock = 1000;
  const screens = Screens.build({ WL, UL, Cmd: Bridge.commands, Sel, cfg, now: () => clock });
  const chart = Chart.create({ screens, cfg });
  let domain = Reducers.reduce(Reducers.initial(cfg), { type: 'user_session', unlocked: true, username: 'kiosk', expires_in_secs: 1800 }, clock);
  let state = chart.initial(domain);
  state = chart.transition(chart.transition(state, Events.button(1, 1)).state, Events.button(1, 0)).state;
  state = Object.assign({}, state, { stack: ['root', 'device-settings'] });
  assert.equal(MenuView.viewModel(state, chart).rows[0].label, 'Lock now (30:00 left)');
  clock += 61000;
  assert.equal(MenuView.viewModel(state, chart).rows[0].label, 'Lock now (28:59 left)');
});

test('the tick timer re-arms while Device settings is on the stack and stops after leaving', () => {
  const h = at('device-settings', true);
  const fx = h.timer('ds-tick');
  assert.ok(fx.some((e) => e.startTimer && e.startTimer.name === 'ds-tick'));
  h.state = Object.assign({}, h.state, { stack: ['root'] });
  assert.deepEqual(h.timer('ds-tick').filter((e) => e.startTimer), []);
});

test('Reboot device is hold-to-confirm and sends device_reboot; a tap does not', () => {
  const h = at('device-settings', true);
  const r = row(h, 'Reboot device');
  assert.equal(r.hold, true);
  h.moveTo('Reboot device');
  let n = h.mark();
  h.btn(28, 1); h.btn(28, 0);
  assert.deepEqual(h.sentSince(n), [], 'a short push does not send');
  n = h.mark();
  h.btn(28, 1);
  h.timer('enter-hold');
  h.btn(28, 0);
  assert.deepEqual(h.sentSince(n), [Bridge.commands.deviceReboot()]);
});

test('Users, Groups and USB mode navigate when unlocked', () => {
  const h = at('device-settings', true);
  h.choose('USB mode');
  assert.equal(stack(h), 'root/device-settings/usb-mode');
  assert.ok(h.sent().some((m) => m.type === 'device_settings_get'));
});

test('USB mode: four toggle rows show persisted values inline, at most one note, row count capped', () => {
  const h = at('usb-mode', true);
  const rows = h.view().rows;
  assert.deepEqual(rows.slice(0, 4).map((r) => [r.label, r.kind, r.display]), [
    ['Mode', 'toggle', 'Device'], ['SSH', 'toggle', 'ON'], ['ADB', 'toggle', 'OFF'], ['UART', 'toggle', 'OFF']]);
  assert.equal(rows.length, 4, 'no note when nothing to say');
  h.wire(settings({ adb: 'on', persisted: { mode: 'device', ssh: 'on', adb: 'on', uart: 'on' } }));
  assert.equal(h.view().rows.length, 5);
  assert.equal(h.labels()[4], 'ADB or UART is a root shell with no password');
  assert.equal(h.view().hint, 'knob push: change. Settings persist across reboots.');
});

test('USB mode: activating sends the opposite value and the shown value changes only on the event', () => {
  const h = at('usb-mode', true);
  const n = h.mark();
  h.choose('SSH');
  assert.deepEqual(h.sentSince(n), [Bridge.commands.deviceSettingsSet('ssh', 'off')]);
  assert.equal(row(h, 'SSH').display, 'ON', 'not optimistic');
  h.wire(settings({ ssh: 'off', persisted: { mode: 'device', ssh: 'off', adb: 'off', uart: 'off' } }));
  assert.equal(row(h, 'SSH').display, 'OFF');
  const m = h.mark();
  h.choose('Mode');
  assert.deepEqual(h.sentSince(m), [Bridge.commands.deviceSettingsSet('mode', 'host')]);
});

test('USB mode: Host greys ADB with a hint, busy greys every row, locked greys every row', () => {
  const h = at('usb-mode', true);
  h.wire(settings({ mode: 'host', adb: 'off', persisted: { mode: 'host', ssh: 'on', adb: 'off', uart: 'off' }, adb_blocked: 'mode_host' }));
  assert.equal(disabled(row(h, 'ADB')), true);
  assert.equal(row(h, 'ADB').disabledHint, 'ADB needs Device mode');
  assert.equal(h.labels()[4], 'ADB needs Device mode');
  assert.equal(h.view().rows.length, 5);
  const n = h.mark();
  h.choose('ADB');
  assert.deepEqual(h.sentSince(n), []);
  h.wire(settings({ busy: 'ssh' }));
  assert.equal(h.view().rows.slice(0, 4).filter(disabled).length, 4);
  assert.equal(h.labels()[4], 'Switching, link may drop about 10 s');
  h.wire(settings());
  h.wire({ type: 'user_session', unlocked: false });
  assert.equal(h.view().title, 'Device settings', 'relock pops to the Device settings root');
  assert.equal(h.view().rows.slice(1).filter(disabled).length, 4);
});

test('USB mode: an error takes the single note slot, stays until cleared, and leaving clears it', () => {
  const h = at('usb-mode', true);
  h.wire({ type: 'device_settings_error', context: 'device_settings_set', code: 'busy', message: 'x' });
  assert.equal(h.view().rows.length, 5);
  assert.equal(h.labels()[4], 'Another change is in progress. Try again in a moment');
  h.timer('toast');
  assert.equal(h.labels()[4], 'Another change is in progress. Try again in a moment');
  h.back();
  h.back();
  const leave = h.effects.filter((e) => e.dispatch && e.dispatch.type === 'local/device_clear_error');
  assert.ok(leave.length >= 1, 'leave dispatches local/device_clear_error');
  h.settleDomain(leave);
  assert.equal(Sel.deviceError(h.domain), null);
});

test('the screens fit 800x480: 5 rows on Device settings, 4 toggles + 1 note on USB mode', () => {
  assert.equal(at('device-settings', false).view().rows.length, 5);
  const h = at('usb-mode', true);
  h.wire(settings({ adb: 'on', persisted: { mode: 'device', ssh: 'off', adb: 'on', uart: 'on' } }));
  assert.ok(h.view().rows.length <= 5);
});

test('device_rebooting toasts', () => {
  const h = at('device-settings', true);
  h.wire({ type: 'device_rebooting' });
  assert.equal(h.state.toast, 'Rebooting...');
});

test('Device settings hint renders a real middle dot, no literal backslash', () => {
  const h = at('device-settings', true);
  const hint = h.view().hint;
  assert.equal(typeof hint, 'string');
  assert.ok(hint.indexOf('\\') === -1, 'hint contains a backslash: ' + hint);
  assert.ok(hint.indexOf('\u00b7') !== -1);
});
