'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const Store = require('../src/state/store.js');
const Reducers = require('../src/state/reducers.js');
const Sel = require('../src/state/selectors.js');
const Config = require('../src/utils/config.js');

const cfg = Config.create({}, () => {}).values;
const fresh = () => Store.create(Reducers.reduce, Reducers.initial(cfg));

test('store notifies only when the state reference changes; unsubscribe works', () => {
  const s = fresh();
  const seen = [];
  const off = s.subscribe((st, a) => seen.push(a.type));
  s.dispatch({ type: 'unknown' });
  s.dispatch({ type: 'bt_power', powered: true });
  s.dispatch({ type: 'bt_device_found', address: 'A', name: 'x' });
  s.dispatch({ type: 'bt_device_found', address: 'A', name: 'x' }); // duplicate: same state
  off();
  s.dispatch({ type: 'bt_power', powered: false });
  assert.deepEqual(seen, ['bt_power', 'bt_device_found']);
  assert.equal(s.getState().bt.powered, false);
});

test('reducers never mutate the previous state', () => {
  const s = fresh();
  const before = JSON.stringify(s.getState());
  const snapshot = s.getState();
  [
    { type: 'session_list', sessions: [{ index: 0, name: 'a' }] },
    { type: 'exited', session: 0, needs_manual_respawn: true },
    { type: 'wifi_scan_results', networks: [{ ssid: 'x' }] },
    { type: 'bt_paired_list', devices: [] },
    { type: 'local/terminal', patch: { fontSize: 20 } },
  ].forEach((a) => s.dispatch(a));
  assert.equal(JSON.stringify(snapshot), before);
});

test('sessions: list, active, dead flags', () => {
  const s = fresh();
  s.dispatch({ type: 'session_list', sessions: [{ index: 0, name: 'a' }, { index: 2, name: 'c' }] });
  s.dispatch({ type: 'exited', session: 2, needs_manual_respawn: true });
  s.dispatch({ type: 'exited', session: 0, needs_manual_respawn: false, auto_respawning: true });
  assert.deepEqual(s.getState().sessions.dead, { 2: true });
  s.dispatch({ type: 'local/active', index: 2 });
  assert.equal(Sel.sessionLabel(s.getState()), 'session: c');
  assert.deepEqual(Sel.tabs(s.getState()), [{ name: 'a', active: false }, { name: 'c', active: true }]);
  s.dispatch({ type: 'local/clearDead', index: 2 });
  assert.deepEqual(s.getState().sessions.dead, {});
  assert.equal(Sel.sessionExists(s.getState(), 1), false);
  s.dispatch({ type: 'local/active', index: 9 });
  assert.equal(Sel.sessionLabel(s.getState()), 'session: 9');
});

test('connection labels follow the status values', () => {
  const s = fresh();
  assert.deepEqual(Sel.connection(s.getState(), 'agent'), { ok: false, label: 'AGENT: CONNECTING' });
  s.dispatch({ type: 'connection', which: 'bridge', status: 'live' });
  s.dispatch({ type: 'connection', which: 'agent', status: 'error' });
  assert.deepEqual(Sel.connection(s.getState(), 'bridge'), { ok: true, label: 'BRIDGE: LIVE' });
  assert.equal(Sel.connection(s.getState(), 'agent').label, 'AGENT: ERROR');
});

test('wifi slice: adapter loss clears live data; scan error stops scanning', () => {
  const s = fresh();
  s.dispatch({ type: 'wifi_available', available: true });
  s.dispatch({ type: 'wifi_status', state: 'connected', ssid: 'x' });
  s.dispatch({ type: 'wifi_scan_started' });
  s.dispatch({ type: 'wifi_error', context: 'wifi_scan', message: 'm' });
  assert.equal(s.getState().wifi.scanning, false);
  s.dispatch({ type: 'wifi_scan_started' });
  s.dispatch({ type: 'wifi_error', context: 'other', message: 'm' });
  assert.equal(s.getState().wifi.scanning, true);
  s.dispatch({ type: 'wifi_saved_list', networks: [{ ssid: 'x' }] });
  s.dispatch({ type: 'wifi_available', available: false });
  assert.deepEqual(s.getState().wifi, { available: false, status: null, scanning: false, networks: null, saved: null });
});

test('bt, brightness and timezone slices', () => {
  const s = fresh();
  s.dispatch({ type: 'bt_device_found', address: 'A', name: 'n' });
  s.dispatch({ type: 'bt_scan_started' });
  assert.deepEqual(s.getState().bt.devices, []);
  s.dispatch({ type: 'bt_paired_list' });
  assert.deepEqual(s.getState().bt.paired, []);
  s.dispatch({ type: 'brightness', value: 5, max: 10 });
  s.dispatch({ type: 'local/brightness', value: 7 });
  assert.deepEqual(s.getState().brightness, { value: 7, max: 10 });
  s.dispatch({ type: 'timezone', value: 'UTC' });
  s.dispatch({ type: 'timezone_list', zones: ['b/x', 'a/y', 'UTC', 'b/z'] });
  assert.equal(s.getState().timezone.loaded, true);
  assert.deepEqual(Sel.timezoneGroups(s.getState().timezone.list), ['UTC', 'a', 'b']);
  assert.deepEqual(Sel.zonesInGroup(s.getState().timezone.list, 'b'), ['b/x', 'b/z']);
  assert.equal(s.getState().terminal.fontSize, 13);
});

// ---- device settings slice (device-settings-plan sections 3.2, 8.1) ----

const fx = (name) => JSON.parse(require('fs').readFileSync(require('path').join(__dirname, '..', '..', 'config', 'protocol', 'controller', name), 'utf8'));
const clocked = (t) => {
  const timers = { t: t, now() { return this.t; } };
  return { timers, store: Store.create(Reducers.withClock(timers), Reducers.initial(cfg)) };
};

test('device: starts unknown with no deadline', () => {
  const d = fresh().getState().device;
  assert.equal(d.loaded, false);
  assert.deepEqual(d.live, { mode: null, ssh: null, adb: null, uart: null });
  assert.equal(d.lastError, null);
  assert.equal(d.rebooting, false);
  assert.equal(d.unlockDeadlineMs, null);
  assert.equal(Sel.deviceMode(fresh().getState()), null);
  assert.equal(Sel.deviceBool(fresh().getState(), 'ssh'), null);
  assert.equal(Sel.deviceDebugActive(fresh().getState()), false);
  assert.equal(Sel.deviceUnlockRemainingSecs(fresh().getState(), 0), null);
});

test('device: device_settings fills live and persisted; selectors map on/off to booleans', () => {
  const s = fresh();
  s.dispatch(fx('event.device_settings.drift.json'));
  const st = s.getState();
  assert.equal(Sel.deviceMode(st), 'device');
  assert.equal(Sel.deviceBool(st, 'adb'), true, 'rows show the persisted value');
  assert.equal(Sel.deviceLiveBool(st, 'adb'), false);
  assert.equal(Sel.deviceBool(st, 'ssh'), true);
  assert.equal(Sel.deviceBool(st, 'uart'), false);
  assert.equal(Sel.deviceDrift(st), true);
  assert.equal(Sel.deviceDebugActive(st), false, 'badge follows live, not persisted');
  assert.equal(Sel.deviceBusy(st), null);
  s.dispatch(fx('event.device_settings.adb_on.json'));
  assert.equal(Sel.deviceDebugActive(s.getState()), true);
  assert.equal(Sel.deviceDrift(s.getState()), false);
  s.dispatch(Object.assign({}, fx('event.device_settings.json'), { live_junk: 1, uart: 'on' }));
  assert.equal(Sel.deviceDebugActive(s.getState()), true, 'uart alone also lights the badge');
});

test('device: host mode blocks adb and busy names the setting being applied', () => {
  const s = fresh();
  s.dispatch(fx('event.device_settings.host.json'));
  assert.equal(Sel.deviceMode(s.getState()), 'host');
  assert.equal(Sel.deviceAdbBlocked(s.getState()), true);
  s.dispatch(fx('event.device_settings.busy.json'));
  assert.equal(Sel.deviceAdbBlocked(s.getState()), false);
  assert.equal(Sel.deviceBusy(s.getState()), 'adb');
  assert.equal(Sel.deviceDrift(s.getState()), false, 'no drift while busy');
});

test('device: errors persist across device_settings until the local clear action', () => {
  const s = fresh();
  s.dispatch(fx('event.device_settings_error.apply_failed.json'));
  assert.equal(Sel.deviceError(s.getState()).code, 'apply_failed');
  assert.equal(Sel.deviceError(s.getState()).seq, 1);
  s.dispatch(fx('event.device_settings.json'));
  assert.equal(Sel.deviceError(s.getState()).code, 'apply_failed', 'a new snapshot does not clear it');
  s.dispatch(fx('event.device_settings_error.busy.json'));
  assert.equal(Sel.deviceError(s.getState()).seq, 2);
  s.dispatch({ type: 'local/device_clear_error' });
  assert.equal(Sel.deviceError(s.getState()), null);
  const same = s.getState();
  s.dispatch({ type: 'local/device_clear_error' });
  assert.equal(s.getState(), same, 'clearing nothing keeps the reference');
});

test('device: device_rebooting sets the rebooting flag', () => {
  const s = fresh();
  s.dispatch(fx('event.device_rebooting.json'));
  assert.equal(Sel.deviceRebooting(s.getState()), true);
});

test('device: user_session expires_in_secs sets a deadline on the injected clock, ticking down to zero', () => {
  const { timers, store } = clocked(10000);
  store.dispatch(fx('event.user_session.ttl.json'));
  assert.equal(store.getState().device.unlockDeadlineMs, 10000 + 1800 * 1000);
  assert.equal(Sel.deviceUnlockRemainingSecs(store.getState(), timers.now()), 1800);
  assert.equal(Sel.deviceUnlockRemainingSecs(store.getState(), 10000 + 1000), 1799);
  assert.equal(Sel.deviceUnlockRemainingSecs(store.getState(), 10000 + 1500), 1799, 'rounds up');
  assert.equal(Sel.deviceUnlockRemainingSecs(store.getState(), 10000 + 1800 * 1000), 0);
  assert.equal(Sel.deviceUnlockRemainingSecs(store.getState(), 10000 + 2000 * 1000), 0, 'never negative');
});

test('device: a second unlock restarts the deadline; lock, not_unlocked and a bridge drop clear it', () => {
  const { timers, store } = clocked(0);
  store.dispatch(fx('event.user_session.ttl.json'));
  timers.t = 60000;
  store.dispatch(fx('event.user_session.ttl.json'));
  assert.equal(store.getState().device.unlockDeadlineMs, 60000 + 1800 * 1000);
  store.dispatch({ type: 'user_session', unlocked: false });
  assert.equal(store.getState().device.unlockDeadlineMs, null);

  store.dispatch(fx('event.user_session.ttl.json'));
  store.dispatch({ type: 'user_error', context: 'user_list_get', code: 'not_unlocked', message: 'x' });
  assert.equal(store.getState().device.unlockDeadlineMs, null);

  store.dispatch(fx('event.user_session.ttl.json'));
  store.dispatch({ type: 'connection', which: 'bridge', status: 'disconnected' });
  assert.equal(store.getState().device.unlockDeadlineMs, null);
});

test('device: without an injected clock, or without expires_in_secs, the deadline stays unknown', () => {
  const s = fresh();
  s.dispatch(fx('event.user_session.ttl.json'));
  assert.equal(s.getState().device.unlockDeadlineMs, null);
  const { store } = clocked(5);
  store.dispatch({ type: 'user_session', unlocked: true, username: 'ana' });
  assert.equal(store.getState().device.unlockDeadlineMs, null);
});
