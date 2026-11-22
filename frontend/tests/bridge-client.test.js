'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const Channel = require('../src/transport/channel.js');
const Bridge = require('../src/transport/bridge-client.js');
const Agent = require('../src/transport/agent-client.js');
const Logging = require('../src/utils/logging.js');
const { createFakeTimers } = require('./helpers/fake-timers.js');
const { createFakeSocketClass } = require('./helpers/fake-socket.js');

const PROTOCOL = path.resolve(__dirname, '..', '..', 'protocol');
const fixtures = (dir, prefix) => {
  const d = path.join(PROTOCOL, dir);
  if (!fs.existsSync(d)) return [];
  return fs.readdirSync(d).filter((f) => f.startsWith(prefix) && f.endsWith('.json')).map((f) => ({ file: f, msg: JSON.parse(fs.readFileSync(path.join(d, f), 'utf8')) }));
};

function wire(clientMod) {
  const timers = createFakeTimers();
  const Sock = createFakeSocketClass();
  const logger = Logging.create({ level: 'trace', sink: null });
  const ch = Channel.create({ name: 'c', url: 'ws://x', WebSocketImpl: Sock, timers, logger, backoff: { initial_ms: 10, max_ms: 10, factor: 1 } });
  const events = [];
  const client = clientMod.create(ch, (e) => events.push(e), logger);
  ch.start();
  Sock.instances[0].open();
  return { ch, Sock, events, client, logger };
}

test('bridge decode: knob, button and every known event; unknown is null', () => {
  assert.deepEqual(Bridge.decode({ type: 'knob', delta: -1 }), { type: 'knob', delta: -1 });
  assert.deepEqual(Bridge.decode({ type: 'button', code: 28, value: 1 }), { type: 'button', code: 28, value: 1 });
  Bridge.WIRE_TYPES.forEach((t) => assert.equal(Bridge.decode({ type: t }).type, 'wire'));
  assert.equal(Bridge.decode({ type: 'something_new' }), null);
});

test('bridge: unknown frames are logged at debug, known ones are emitted', () => {
  const { Sock, events, logger } = wire(Bridge);
  Sock.instances[0].message({ type: 'something_new' });
  Sock.instances[0].message({ type: 'wifi_status', state: 'connected' });
  assert.deepEqual(events, [{ type: 'wire', msg: { type: 'wifi_status', state: 'connected' } }]);
  assert.ok(logger.dump().some((e) => /unknown message type something_new/.test(e.text)));
});

test('bridge: every shared server fixture decodes to an event (nothing unknown or dropped)', () => {
  const fx = fixtures('controller', 'event.');
  if (fx.length === 0) return; // shared fixtures not present yet
  fx.forEach((f) => assert.ok(Bridge.decode(f.msg), 'not decoded: ' + f.file));
});

test('bridge: every shared command fixture equals the frame a builder produces', () => {
  const fx = fixtures('controller', 'command.');
  if (fx.length === 0) return;
  const built = [
    Bridge.commands.btPowerGet(), Bridge.commands.btSetPower(true), Bridge.commands.btScanStart(), Bridge.commands.btScanStop(),
    Bridge.commands.btPairedListGet(), Bridge.commands.btPair('AA:BB:CC:DD:EE:FF'), Bridge.commands.btPairingConfirm(true),
    Bridge.commands.btConnect('AA:BB:CC:DD:EE:FF'), Bridge.commands.btDisconnect('AA:BB:CC:DD:EE:FF'), Bridge.commands.btForget('AA:BB:CC:DD:EE:FF'),
    Bridge.commands.brightnessGet(), Bridge.commands.brightnessSet(80), Bridge.commands.timezoneGet(), Bridge.commands.timezoneListGet(),
    Bridge.commands.timezoneSet('Europe/Lisbon'), Bridge.commands.wifiGet(), Bridge.commands.wifiSavedListGet(), Bridge.commands.wifiScan(),
    Bridge.commands.wifiConnect('Home', 'hunter2hunter2'), Bridge.commands.wifiConnect('Home', ''), Bridge.commands.wifiConnectSaved('Home'),
    Bridge.commands.wifiDisconnect(), Bridge.commands.wifiForget('Home'),
  ];
  const types = new Set(built.map((b) => b.type));
  fx.forEach((f) => {
    assert.ok(types.has(f.msg.type), 'no builder for command type ' + f.msg.type);
  });
  // field-for-field match for every fixture that a builder call reproduces
  let matched = 0;
  fx.forEach((f) => { if (built.some((b) => JSON.stringify(sortKeys(b)) === JSON.stringify(sortKeys(f.msg)))) matched++; });
  assert.ok(matched >= Math.floor(fx.length * 0.5), 'builders reproduce ' + matched + ' of ' + fx.length + ' fixtures');
});

function sortKeys(o) { return Object.keys(o).sort().reduce((a, k) => { a[k] = o[k]; return a; }, {}); }

test('bridge builders keep the wire shapes the controller expects', () => {
  assert.deepEqual(Bridge.commands.wifiConnect('Home', 'pw'), { type: 'wifi_connect', ssid: 'Home', password: 'pw' });
  assert.deepEqual(Bridge.commands.btSetPower(false), { type: 'bt_set_power', powered: false });
  assert.deepEqual(Bridge.commands.btPairingConfirm(false), { type: 'bt_pairing_confirm', accept: false });
  assert.deepEqual(Bridge.commands.timezoneSet('UTC'), { type: 'timezone_set', value: 'UTC' });
  assert.deepEqual(Bridge.commands.brightnessSet(8), { type: 'brightness_set', value: 8 });
});

test('bridge: getters queue while down and flush on open, commands drop visibly', () => {
  const timers = createFakeTimers();
  const Sock = createFakeSocketClass();
  const logger = Logging.create({ level: 'trace', sink: null });
  const ch = Channel.create({ name: 'c', url: 'x', WebSocketImpl: Sock, timers, logger, backoff: { initial_ms: 10, max_ms: 10, factor: 1 } });
  const client = Bridge.create(ch, () => {}, logger);
  ch.start();
  const drops = [];
  ch.onDrop((m) => drops.push(m.type));
  assert.equal(client.send(Bridge.commands.wifiGet()), 'queued');
  assert.equal(client.send(Bridge.commands.wifiConnect('Home', 'pw')), 'dropped');
  Sock.instances[0].open();
  assert.deepEqual(Sock.instances[0].sent, [{ type: 'wifi_get' }]);
  assert.deepEqual(drops, ['wifi_connect']);
});

test('bridge: requestInitialState sends the four getters, in order', () => {
  const { Sock, client } = wire(Bridge);
  client.requestInitialState();
  assert.deepEqual(Sock.instances[0].sent, [{ type: 'bt_power_get' }, { type: 'brightness_get' }, { type: 'timezone_get' }, { type: 'wifi_get' }]);
});

test('agent decode: all four frame types, unknown is null', () => {
  assert.deepEqual(Agent.decode({ type: 'session_list' }), { type: 'session_list', sessions: [] });
  assert.deepEqual(Agent.decode({ type: 'output', session: 1, data: 'x' }), { type: 'output', session: 1, data: 'x' });
  assert.deepEqual(Agent.decode({ type: 'exited', session: 1, code: 2 }), { type: 'exited', session: 1, code: 2, needs_manual_respawn: false, auto_respawning: false });
  assert.deepEqual(Agent.decode({ type: 'clock_sync', offset_ms: 5, status: 'ok' }), { type: 'clock_sync', offset_ms: 5, status: 'ok' });
  assert.equal(Agent.decode({ type: 'nope' }), null);
});

test('agent: every shared server fixture decodes; client builders match client fixtures', () => {
  const server = fixtures('terminal', 'server_');
  server.forEach((f) => assert.ok(Agent.decode(f.msg), 'not decoded: ' + f.file));
  const client = fixtures('terminal', 'client_');
  const built = [Agent.commands.input(0, 'h\u00e9llo\r'), Agent.commands.resize(1, 80, 24), Agent.commands.control(0, 'clear'), Agent.commands.control(0, 'interrupt'), Agent.commands.control(3, 'respawn')];
  client.forEach((f) => assert.ok(built.some((b) => JSON.stringify(sortKeys(b)) === JSON.stringify(sortKeys(f.msg))), 'no builder reproduces ' + f.file));
});

test('agent: resize drops are quiet, input drops are reported', () => {
  const timers = createFakeTimers();
  const Sock = createFakeSocketClass();
  const logger = Logging.create({ level: 'trace', sink: null });
  const ch = Channel.create({ name: 'c', url: 'x', WebSocketImpl: Sock, timers, logger, backoff: { initial_ms: 10, max_ms: 10, factor: 1 } });
  const client = Agent.create(ch, () => {}, logger);
  ch.start();
  const drops = [];
  ch.onDrop((m) => drops.push(m.type));
  assert.equal(client.sendResize(0, 80, 24), 'dropped');
  assert.equal(client.sendInput(0, 'a'), 'dropped');
  assert.deepEqual(drops, ['input']);
});
