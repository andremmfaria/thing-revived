// Run: node --test frontend/tests/*.test.js   (the bare directory form `node --test frontend/tests/` fails on Node 24)
'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const WL = require('../src/wifi/wifi-logic.js');

test('validatePassword enforces WPA length and printable ASCII', () => {
  assert.equal(WL.validatePassword('12345678', 'wpa2'), null);
  assert.equal(WL.validatePassword('a'.repeat(63), 'wpa3'), null);
  assert.ok(WL.validatePassword('1234567', 'wpa2'));
  assert.ok(WL.validatePassword('a'.repeat(64), 'wpa2'));
  assert.ok(WL.validatePassword('caf\u00e9-password', 'wpa2'));
  assert.ok(WL.validatePassword('tab\there123', 'wpa2/wpa3'));
  assert.equal(WL.validatePassword('', 'open'), null);
  assert.ok(WL.validatePassword('x', 'open'));
  assert.ok(WL.validatePassword('12345678', 'wep'));
  assert.ok(WL.validatePassword('12345678', 'eap'));
});

test('maskPassword hides unless shown', () => {
  assert.equal(WL.maskPassword('abc', false), '\u2022\u2022\u2022');
  assert.equal(WL.maskPassword('abc', true), 'abc');
  assert.equal(WL.maskPassword('', false), '');
});

test('picker ring wraps in both directions and starts on a', () => {
  assert.equal(WL.PICKER_RING[WL.PICKER_START], 'a');
  const n = WL.PICKER_RING.length;
  assert.equal(WL.pickerMove(0, -1), n - 1);
  assert.equal(WL.pickerMove(n - 1, 1), 0);
  assert.equal(WL.pickerMove(5, 7), 6); // only the sign counts
  assert.equal(WL.pickerMove(5, 0), 5);
});

test('control tokens are one detent left of a (wrap-around reachable)', () => {
  assert.equal(WL.PICKER_RING[WL.PICKER_START - 1], WL.TOKEN_DEL);
  assert.equal(WL.PICKER_RING[WL.PICKER_START - 2], WL.TOKEN_DONE);
});

test('pickerApply appends, deletes, finishes and respects the 63 limit', () => {
  const idx = (ch) => WL.PICKER_RING.indexOf(ch);
  assert.deepEqual(WL.pickerApply('ab', idx('c')), { password: 'abc', done: false });
  assert.deepEqual(WL.pickerApply('ab', idx(' ')), { password: 'ab ', done: false });
  assert.deepEqual(WL.pickerApply('ab', idx(WL.TOKEN_DEL)), { password: 'a', done: false });
  assert.deepEqual(WL.pickerApply('', idx(WL.TOKEN_DEL)), { password: '', done: false });
  assert.deepEqual(WL.pickerApply('ab', idx(WL.TOKEN_DONE)), { password: 'ab', done: true });
  const full = 'x'.repeat(63);
  assert.deepEqual(WL.pickerApply(full, idx('c')), { password: full, done: false });
});

test('ring covers every printable ASCII character exactly once', () => {
  const chars = WL.PICKER_RING.filter((t) => t.length === 1);
  assert.equal(new Set(chars).size, chars.length);
  for (let c = 32; c <= 126; c++) assert.ok(chars.includes(String.fromCharCode(c)), 'missing ' + c);
  assert.equal(chars.length, 95);
});

test('pickerWindow centres the current index', () => {
  const w = WL.pickerWindow(WL.PICKER_START, 3);
  assert.equal(w.length, 7);
  assert.equal(w[3].item, 'a');
  assert.equal(w[3].offset, 0);
  assert.equal(w[2].item, WL.TOKEN_DEL);
  const wrapped = WL.pickerWindow(0, 2);
  assert.equal(wrapped[0].index, WL.PICKER_RING.length - 2);
});

test('signalBars thresholds', () => {
  assert.deepEqual([100, 75, 74, 50, 49, 25, 24, 1, 0].map(WL.signalBars), [4, 4, 3, 3, 2, 2, 1, 1, 0]);
  assert.equal(WL.signalBars(undefined), 0);
});

test('statusRows for connected, connecting and disconnected', () => {
  const connected = WL.statusRows({
    state: 'connected', ssid: 'Home', ip: '10.0.0.5', interface: 'wlan0', signal_pct: 80, ethernet_up: true,
  });
  const labels = connected.map((r) => r.label);
  assert.deepEqual(labels, ['Status', 'Network', 'IP', 'Signal', 'Interface', 'Ethernet']);
  assert.equal(connected[3].bars, 4);
  assert.equal(connected[5].value, 'also up');

  const off = WL.statusRows({ state: 'disconnected', ssid: null, ip: null, interface: 'wlan0', signal_pct: null, ethernet_up: false });
  assert.deepEqual(off.map((r) => r.label), ['Status', 'Interface', 'Ethernet']);
  assert.equal(off[0].value, 'Not connected');
  assert.equal(WL.statusRows(null)[0].value, 'Checking...');
  assert.equal(WL.stateLabel({ state: 'obtaining_ip' }), 'Getting IP address...');
});

test('securityLabel and findSaved', () => {
  assert.equal(WL.securityLabel('wpa2/wpa3'), 'WPA2/3');
  assert.equal(WL.securityLabel('eap'), '802.1X');
  assert.equal(WL.findSaved([{ ssid: 'A' }, { ssid: 'B' }], 'B'), 1);
  assert.equal(WL.findSaved(null, 'B'), -1);
});
