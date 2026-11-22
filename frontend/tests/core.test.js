'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const Bus = require('../src/core/bus.js');
const Timers = require('../src/core/timers.js');
const Mod = require('../src/core/module.js');
const { createFakeTimers } = require('./helpers/fake-timers.js');

test('bus delivers in order, supports off and once', () => {
  const bus = Bus.create();
  const seen = [];
  const off = bus.on('t', (p) => seen.push('a' + p));
  bus.on('t', (p) => seen.push('b' + p));
  bus.once('t', (p) => seen.push('once' + p));
  bus.emit('t', 1);
  off();
  bus.emit('t', 2);
  assert.deepEqual(seen, ['a1', 'b1', 'once1', 'b2']);
  assert.equal(bus.count('t'), 1);
});

test('bus: a throwing handler does not stop the others; error goes to onError', () => {
  const errs = [];
  const bus = Bus.create({ onError: (topic, e) => errs.push([topic, e.message]) });
  const seen = [];
  bus.on('t', () => { throw new Error('boom'); });
  bus.on('t', () => seen.push('ran'));
  bus.emit('t');
  assert.deepEqual(seen, ['ran']);
  assert.deepEqual(errs, [['t', 'boom']]);
  const plain = Bus.create();
  plain.on('t', () => { throw new Error('x'); });
  plain.on('t', () => seen.push('ran2'));
  assert.throws(() => plain.emit('t'), /x/);
  assert.deepEqual(seen, ['ran', 'ran2']);
});

test('named timers: start replaces, cancel stops, firing reports the name', () => {
  const ft = createFakeTimers();
  const fired = [];
  const nt = Timers.createNamedTimers(ft, (n) => fired.push(n));
  nt.start('esc', 450);
  ft.tick(300);
  nt.start('esc', 450); // restart
  ft.tick(300);
  assert.deepEqual(fired, []);
  ft.tick(200);
  assert.deepEqual(fired, ['esc']);
  assert.equal(nt.has('esc'), false);
  nt.start('a', 10); nt.start('b', 10);
  nt.cancel('a');
  ft.tick(10);
  assert.deepEqual(fired, ['esc', 'b']);
  nt.start('c', 10);
  nt.cancelAll();
  ft.tick(50);
  assert.deepEqual(fired, ['esc', 'b']);
});

test('timers.create wraps the given environment', () => {
  const calls = [];
  const env = { setTimeout: (f, m) => { calls.push(['st', m]); return 7; }, clearTimeout: (i) => calls.push(['ct', i]), setInterval: () => 8, clearInterval: () => {} };
  const t = Timers.create(env);
  assert.equal(t.setTimeout(() => {}, 5), 7);
  t.clearTimeout(7);
  assert.deepEqual(calls, [['st', 5], ['ct', 7]]);
  assert.equal(typeof t.now(), 'number');
});

test('module.assertLoaded names every missing module', () => {
  Mod.assertLoaded(['a'], { a: {} });
  assert.throws(() => Mod.assertLoaded(['a', 'b', 'c'], { a: {} }), /b, c.*<script> order/);
  assert.deepEqual(Mod.loaded({ z: 1, a: 2 }), ['a', 'z']);
});
