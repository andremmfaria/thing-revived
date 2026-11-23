'use strict';
// First-boot wire pieces: builders, decoders, logging, reducer, pure logic.
// The overlay itself is in first-boot.test.js.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const Types = require('../src/utils/types.js');
const Logging = require('../src/utils/logging.js');
const Bridge = require('../src/transport/bridge-client.js');
const Reducers = require('../src/state/reducers.js');
const Config = require('../src/utils/config.js');
const UL = require('../src/users/users-logic.js');

const PROTOCOL = path.resolve(__dirname, '..', '..', 'config', 'protocol', 'controller');
const fx = (name) => JSON.parse(fs.readFileSync(path.join(PROTOCOL, name + '.json'), 'utf8'));
const SECRET = fx('event.user_bootstrap_secret').password;
const PHASES = ['none', 'prompt', 'generating', 'showing', 'done'];

test('users: user_bootstrap_get is the only first-boot command and matches its fixture', () => {
  assert.deepEqual(Bridge.commands.userBootstrapGet(), fx('command.user_bootstrap_get'));
  assert.equal(Bridge.commands.userBootstrapReveal, undefined);
  assert.equal(Bridge.commands.userBootstrapAck, undefined);
  assert.equal(Types.BridgeCommand.user_bootstrap_reveal, undefined);
  assert.equal(Types.BridgeCommand.user_bootstrap_ack, undefined);
});

test('users: every first-boot event fixture decodes to a wire event, one fixture per phase', () => {
  PHASES.forEach((p) => assert.equal(fx('event.user_bootstrap.' + p).phase, p));
  PHASES.forEach((p) => assert.ok(Types.FirstBootPhase[p], p));
  ['event.user_bootstrap.prompt', 'event.user_bootstrap.showing', 'event.user_bootstrap.storage_failed', 'event.user_bootstrap_secret', 'event.user_error'].forEach((n) => {
    const m = fx(n);
    assert.deepEqual(Bridge.decode(m), { type: 'wire', msg: m }, n);
  });
  assert.equal(Bridge.QUEUEABLE.user_bootstrap_get, 1);
});

test('users: logging redacts the password in the secret event', () => {
  const l = Logging.create({ level: 'trace', sink: null });
  l.info('got', fx('event.user_bootstrap_secret'));
  const text = l.dump().map((e) => e.text).join('\n');
  assert.equal(text.indexOf(SECRET), -1);
  assert.equal(Logging.describe(fx('event.user_bootstrap_secret')).indexOf(SECRET), -1);
});

test('users reducer: only the phase is kept, the password never reaches the store', () => {
  const cfg = Config.create({}, () => {}).values;
  const s = Reducers.initial(cfg);
  assert.equal(s.users.bootstrap, null);
  const s1 = Reducers.reduce(s, fx('event.user_bootstrap.showing'));
  assert.deepEqual(s1.users.bootstrap, { phase: 'showing', username: 'kiosk', storage_ok: true });
  assert.equal(Reducers.reduce(s1, fx('event.user_bootstrap.storage_failed')).users.bootstrap.storage_ok, false);
  assert.equal(Reducers.reduce(s1, fx('event.user_bootstrap_secret')), s1, 'secret event leaves the state untouched');
  assert.equal(s.users.bootstrap, null, 'previous state not mutated');
  assert.equal(JSON.stringify(s1).indexOf(SECRET), -1);
});

test('users logic: groups, phase, error text', () => {
  assert.deepEqual(UL.groups('Xe4mH7pQ9tLwZc3K'), ['Xe4m', 'H7pQ', '9tLw', 'Zc3K']);
  assert.equal(UL.phase(null), 'unknown');
  assert.equal(UL.phase({ phase: 'prompt' }), 'prompt');
  assert.match(UL.errorText('generate_failed', 'x'), /try again/i);
  assert.match(UL.errorText('ack_failed'), /again/i);
  assert.match(UL.errorText('storage_unavailable'), /storage/i);
  assert.equal(UL.errorText('weird', 'controller text'), 'controller text');
  assert.equal(UL.errorText('weird'), 'Could not complete the request');
  ['press_required', 'bad_ack', 'expired', 'not_pending'].forEach((c) => assert.equal(Types.UserErrorCode[c], undefined, c));
});
