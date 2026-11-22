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
