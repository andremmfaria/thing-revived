'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const Logging = require('../src/utils/logging.js');

function sinkRecorder() {
  const lines = [];
  const sink = {};
  ['error', 'warn', 'info', 'debug'].forEach((l) => { sink[l] = (s) => lines.push([l, s]); });
  return { lines, sink };
}

test('describe() redacts the password of a wifi_connect message', () => {
  const msg = { type: 'wifi_connect', ssid: 'Home', password: 'hunter2hunter2' };
  const s = Logging.describe(msg);
  assert.match(s, /^wifi_connect /);
  assert.ok(s.indexOf('Home') !== -1);
  assert.ok(s.indexOf('hunter2hunter2') === -1);
  assert.ok(s.indexOf('[redacted]') !== -1);
  assert.equal(msg.password, 'hunter2hunter2', 'the message itself is not mutated');
});

test('passkeys, typed text and terminal frames never appear in log text', () => {
  const l = Logging.create({ level: 'trace', sink: sinkRecorder().sink });
  l.info('got', { type: 'bt_pairing_confirm_request', address: 'AA', passkey: 123456 });
  l.info('got', { type: 'input', session: 0, data: 'secret-typing' });
  l.info('got', { type: 'output', session: 0, data: 'screen text' });
  l.info('got', { type: 'text', value: 'typed-pw' });
  l.info('got', { nested: { deeper: [{ psk: 'abc' }] } });
  const joined = l.dump().map((e) => e.text).join('\n');
  ['123456', 'secret-typing', 'screen text', 'typed-pw', 'abc'].forEach((s) => {
    assert.ok(joined.indexOf(s) === -1, 'leaked ' + s);
  });
  assert.ok(joined.indexOf('AA') !== -1);
});

test('settings messages keep their non-secret value field', () => {
  assert.match(Logging.describe({ type: 'brightness_set', value: 80 }), /80/);
  assert.match(Logging.describe({ type: 'timezone_set', value: 'UTC' }), /UTC/);
});

test('levels filter output and the prefix names the frontend and component', () => {
  const r = sinkRecorder();
  const l = Logging.create({ level: 'warn', sink: r.sink });
  l.child('bridge').error('boom');
  l.warn('careful');
  l.info('hidden');
  l.debug('hidden');
  l.trace('hidden');
  assert.deepEqual(r.lines, [
    ['error', '[frontend] ERROR bridge: boom'],
    ['warn', '[frontend] WARN: careful'],
  ]);
  l.setLevel('trace');
  l.trace('now visible');
  assert.equal(r.lines[2][0], 'debug');
  assert.match(r.lines[2][1], /TRACE: now visible/);
  assert.equal(l.getLevel(), 'trace');
});

test('ring buffer keeps the last 200 entries', () => {
  const l = Logging.create({ level: 'info', sink: null });
  for (let i = 0; i < 250; i++) l.info('n' + i);
  const d = l.dump();
  assert.equal(d.length, 200);
  assert.equal(d[0].text, 'n50');
  assert.equal(d[199].text, 'n249');
  d.pop();
  assert.equal(l.dump().length, 200, 'dump returns a copy');
});

test('Error objects log their message only; circular objects do not throw', () => {
  const l = Logging.create({ level: 'info', sink: null });
  l.error('failed:', new Error('bad thing'));
  const o = {}; o.self = o;
  l.info('circ', o);
  const d = l.dump();
  assert.equal(d[0].text, 'failed: bad thing');
  assert.ok(d[1].text.length > 0);
});

test('the default instance reads its level from the config (info)', () => {
  assert.equal(Logging.getLevel(), 'info');
  assert.equal(Logging.RING_SIZE, 200);
});
