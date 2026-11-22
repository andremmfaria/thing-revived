'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const Sessions = require('../src/terminal/sessions.js');
const Store = require('../src/state/store.js');
const Reducers = require('../src/state/reducers.js');
const Config = require('../src/utils/config.js');

test('utf8Length counts bytes, not characters', () => {
  assert.equal(Sessions.utf8Length('abc'), 3);
  assert.equal(Sessions.utf8Length('h\u00e9'), 3);
  assert.equal(Sessions.utf8Length('\u2022'), 3);
  assert.equal(Sessions.utf8Length('\ud83d\ude00'), 4);
});

test('replay buffer is capped by bytes, dropping the oldest chunks first', () => {
  const r = Sessions.createReplayBuffers(10);
  r.append(0, 'aaaa'); r.append(0, 'bbbb'); r.append(0, 'cccc');
  assert.deepEqual(r.get(0), ['bbbb', 'cccc']);
  assert.equal(r.bytes(0), 8);
  r.append(1, 'x');
  assert.deepEqual(r.get(1), ['x']);
  assert.deepEqual(r.get(7), []);
});

test('chunk count does not matter, only bytes: many tiny chunks stay under the cap', () => {
  const r = Sessions.createReplayBuffers(100);
  for (let i = 0; i < 5000; i++) r.append(0, 'z');
  assert.equal(r.bytes(0), 100);
  assert.equal(r.get(0).length, 100);
});

test('a single oversize chunk is cut to its tail, multibyte-safe', () => {
  const r = Sessions.createReplayBuffers(8);
  r.append(0, 'abcdefghijklmnop');
  assert.deepEqual(r.get(0), ['ijklmnop']);
  r.append(1, '\u2022\u2022\u2022\u2022'); // 12 bytes
  assert.equal(r.bytes(1) <= 8, true);
  assert.deepEqual(r.get(1), ['\u2022\u2022']);
  r.append(2, 'x\ud83d\ude00\ud83d\ude00'); // 9 bytes, cut must not split a pair
  const kept = r.get(2).join('');
  assert.equal(kept, '\ud83d\ude00\ud83d\ude00');
});

test('default config cap (256 KiB) holds the most recent output of a flood', () => {
  const cfg = Config.create({}, () => {}).values;
  const r = Sessions.createReplayBuffers(cfg.terminal.replay_buffer_bytes);
  for (let i = 0; i < 20000; i++) r.append(0, 'line ' + i + ' ' + 'x'.repeat(80) + '\r\n');
  assert.ok(r.bytes(0) <= 262144 && r.bytes(0) > 262144 - 200);
  assert.match(r.get(0)[r.get(0).length - 1], /line 19999 /);
});

function controller() {
  const cfg = Config.create({}, () => {}).values;
  const store = Store.create(Reducers.reduce, Reducers.initial(cfg));
  store.dispatch({ type: 'session_list', sessions: [{ index: 0, name: 'a' }, { index: 1, name: 'b' }] });
  const calls = [];
  const replay = Sessions.createReplayBuffers(1000);
  const termView = { write: (d) => calls.push(['write', d]), replay: (c) => calls.push(['replay', c]), fitAndResize: () => calls.push(['fit']) };
  const agent = { sendControl: (i, a) => calls.push(['control', i, a]) };
  const ctl = Sessions.createSessions({ store, replay, termView, agent });
  return { store, calls, ctl, replay };
}

test('output: only the active session is written live, all are buffered', () => {
  const { ctl, calls, replay } = controller();
  ctl.onOutput(0, 'A');
  ctl.onOutput(1, 'B');
  assert.deepEqual(calls, [['write', 'A']]);
  assert.deepEqual(replay.get(1), ['B']);
});

test('switchTo: replays the buffer and forces a resize; no-ops for the active or a missing session', () => {
  const { ctl, calls, store } = controller();
  ctl.onOutput(1, 'B');
  calls.length = 0;
  ctl.switchTo(1);
  assert.deepEqual(calls, [['replay', ['B']], ['fit']]);
  assert.equal(store.getState().sessions.active, 1);
  calls.length = 0;
  ctl.switchTo(1);
  ctl.switchTo(5);
  assert.deepEqual(calls, []);
});

test('dead session: switching to it sends respawn once and clears the flag', () => {
  const { ctl, calls, store } = controller();
  ctl.onExited({ session: 1, code: null, needs_manual_respawn: true, auto_respawning: false });
  assert.deepEqual(store.getState().sessions.dead, { 1: true });
  calls.length = 0;
  ctl.switchTo(1);
  assert.deepEqual(calls.slice(0, 1), [['control', 1, 'respawn']]);
  assert.deepEqual(store.getState().sessions.dead, {});
  ctl.switchTo(0);
  calls.length = 0;
  ctl.switchTo(1);
  assert.deepEqual(calls.filter((c) => c[0] === 'control'), []);
});

test('exited notices differ for manual respawn, auto respawn and plain exit', () => {
  const { ctl, replay } = controller();
  ctl.onExited({ session: 0, code: 3, needs_manual_respawn: false, auto_respawning: true });
  ctl.onExited({ session: 0, code: 0, needs_manual_respawn: false, auto_respawning: false });
  ctl.onExited({ session: 0, code: null, needs_manual_respawn: true, auto_respawning: false });
  assert.deepEqual(replay.get(0), [
    '\n[session exited, code=3 -- restarting...]\n',
    '\n[session exited, code=0]\n',
    '\n[session exited -- select this tab again to restart it]\n',
  ]);
});

test('session_list repaints the active session from its buffer', () => {
  const { ctl, calls } = controller();
  ctl.onOutput(0, 'keep');
  calls.length = 0;
  ctl.onSessionList([{ index: 0, name: 'a' }]);
  assert.deepEqual(calls, [['replay', ['keep']]]);
});
