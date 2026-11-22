'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const Channel = require('../src/transport/channel.js');
const Logging = require('../src/utils/logging.js');
const { createFakeTimers } = require('./helpers/fake-timers.js');
const { createFakeSocketClass } = require('./helpers/fake-socket.js');

function setup(over) {
  const timers = createFakeTimers();
  const Sock = createFakeSocketClass();
  const logger = Logging.create({ level: 'trace', sink: null });
  const ch = Channel.create(Object.assign({
    name: 'test', url: 'ws://x', WebSocketImpl: Sock, timers, logger,
    backoff: { initial_ms: 2000, max_ms: 30000, factor: 1.0 }, queueMax: 3,
  }, over || {}));
  const statuses = [];
  ch.onStatus((s) => statuses.push(s));
  return { ch, timers, Sock, statuses, logger };
}

test('starts connecting, goes live on open, forwards parsed frames', () => {
  const { ch, Sock, statuses } = setup();
  assert.equal(ch.status(), 'connecting');
  ch.start();
  const got = [];
  ch.onMessage((m) => got.push(m));
  Sock.instances[0].open();
  Sock.instances[0].message({ type: 'knob', delta: 1 });
  assert.deepEqual(statuses, ['live']);
  assert.deepEqual(got, [{ type: 'knob', delta: 1 }]);
});

test('bad frames are counted and logged, not delivered and not silent', () => {
  const { ch, Sock, logger } = setup();
  ch.start();
  const got = [];
  ch.onMessage((m) => got.push(m));
  Sock.instances[0].open();
  Sock.instances[0].message('{nope');
  Sock.instances[0].message('[1]');
  Sock.instances[0].message({ no: 'type' });
  assert.deepEqual(got, []);
  assert.equal(ch.stats().badFrames, 3);
  assert.ok(logger.dump().filter((e) => e.level === 'warn').length >= 3);
});

test('send returns sent / dropped explicitly and reports drops to listeners', () => {
  const { ch, Sock, logger } = setup();
  ch.start();
  const drops = [];
  ch.onDrop((m) => drops.push(m.type));
  assert.equal(ch.send({ type: 'wifi_connect', ssid: 's', password: 'pw-secret' }), 'dropped');
  Sock.instances[0].open();
  assert.equal(ch.send({ type: 'a' }), 'sent');
  assert.deepEqual(Sock.instances[0].sent, [{ type: 'a' }]);
  Sock.instances[0].serverClose();
  assert.equal(ch.send({ type: 'b' }), 'dropped');
  assert.deepEqual(drops, ['wifi_connect', 'b']);
  assert.equal(ch.stats().dropped, 2);
  assert.ok(logger.dump().every((e) => e.text.indexOf('pw-secret') === -1), 'password never logged');
});

test('quiet drops are counted but not reported', () => {
  const { ch } = setup();
  ch.start();
  const drops = [];
  ch.onDrop((m) => drops.push(m));
  assert.equal(ch.send({ type: 'resize' }, { quiet: true }), 'dropped');
  assert.deepEqual(drops, []);
  assert.equal(ch.stats().dropped, 1);
});

test('queueable getters are queued while down, deduped, bounded, and flushed on open', () => {
  const { ch, Sock } = setup();
  ch.start();
  assert.equal(ch.send({ type: 'g1' }, { queue: true }), 'queued');
  assert.equal(ch.send({ type: 'g1' }, { queue: true }), 'queued');
  assert.equal(ch.queued(), 1);
  ch.send({ type: 'g2' }, { queue: true });
  ch.send({ type: 'g3' }, { queue: true });
  ch.send({ type: 'g4' }, { queue: true }); // overflows queueMax=3, drops g1
  assert.equal(ch.queued(), 3);
  assert.equal(ch.stats().overflow, 1);
  Sock.instances[0].open();
  assert.deepEqual(Sock.instances[0].sent, [{ type: 'g2' }, { type: 'g3' }, { type: 'g4' }]);
  assert.equal(ch.queued(), 0);
});

test('a getter sent by an on-open listener is not sent twice from the queue', () => {
  const { ch, Sock } = setup();
  ch.start();
  ch.send({ type: 'get' }, { queue: true });
  ch.onStatus((s) => { if (s === 'live') ch.send({ type: 'get' }, { queue: true }); });
  Sock.instances[0].open();
  assert.deepEqual(Sock.instances[0].sent, [{ type: 'get' }]);
});

test('fixed reconnect delay with factor 1.0 (today\'s 2000 ms)', () => {
  const { ch, timers, Sock, statuses } = setup();
  ch.start();
  Sock.instances[0].open();
  Sock.instances[0].serverClose();
  assert.deepEqual(statuses, ['live', 'disconnected']);
  timers.tick(1999);
  assert.equal(Sock.instances.length, 1);
  timers.tick(1);
  assert.equal(Sock.instances.length, 2);
  Sock.instances[1].serverClose(); // fails without ever opening
  timers.tick(2000);
  assert.equal(Sock.instances.length, 3);
});

test('exponential backoff grows to the cap and resets after an open', () => {
  const { ch, timers, Sock } = setup({ backoff: { initial_ms: 1000, max_ms: 5000, factor: 2 } });
  ch.start();
  const delays = [];
  for (let i = 0; i < 5; i++) {
    const before = Sock.instances.length;
    Sock.instances[before - 1].serverClose();
    let waited = 0;
    while (Sock.instances.length === before) { timers.tick(100); waited += 100; }
    delays.push(waited);
  }
  assert.deepEqual(delays, [1000, 2000, 4000, 5000, 5000]);
  Sock.instances[Sock.instances.length - 1].open();
  Sock.instances[Sock.instances.length - 1].serverClose();
  const before = Sock.instances.length;
  timers.tick(1000);
  assert.equal(Sock.instances.length, before + 1, 'attempt counter reset by the successful open');
});

test('error then close shows ERROR then DISCONNECTED; stale sockets are ignored', () => {
  const { ch, Sock, statuses } = setup();
  ch.start();
  const first = Sock.instances[0];
  first.error();
  first.serverClose();
  assert.deepEqual(statuses, ['error', 'disconnected']);
  first.open(); // a late event from the dead socket must not revive the channel
  assert.equal(ch.status(), 'disconnected');
});

test('constructor failure is retried, close() stops everything', () => {
  const timers = createFakeTimers();
  let attempts = 0;
  const Bad = function () { attempts++; throw new Error('bad url'); };
  const ch = Channel.create({ name: 't', url: 'x', WebSocketImpl: Bad, timers, logger: Logging.create({ level: 'trace', sink: null }), backoff: { initial_ms: 100, max_ms: 100, factor: 1 } });
  ch.start();
  timers.tick(250);
  assert.equal(attempts, 3);
  ch.close();
  timers.tick(1000);
  assert.equal(attempts, 3);
  assert.equal(timers.pending(), 0);
});
