'use strict';
// Phase 2a/2b account menu: builders, reducer slice (ui.users), selectors and
// pure logic. Wire shapes come only from the golden fixtures.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const Types = require('../src/utils/types.js');
const Logging = require('../src/utils/logging.js');
const Bridge = require('../src/transport/bridge-client.js');
const Reducers = require('../src/state/reducers.js');
const Selectors = require('../src/state/selectors.js');
const Store = require('../src/state/store.js');
const Config = require('../src/utils/config.js');
const UL = require('../src/users/users-logic.js');

const PROTOCOL = path.resolve(__dirname, '..', '..', 'config', 'protocol', 'controller');
const fx = (name) => JSON.parse(fs.readFileSync(path.join(PROTOCOL, name + '.json'), 'utf8'));
const cfg = Config.create({}, () => {}).values;
const fresh = () => Reducers.initial(cfg);
const run = (s, ...msgs) => msgs.reduce((acc, m) => Reducers.reduce(acc, m), s);
const C = Bridge.commands;

test('users builders: defaults and the commands that must never be queued', () => {
  assert.deepEqual(C.userAdd('ana'), { type: 'user_add', name: 'ana', password: '', admin: false });
  assert.deepEqual(C.userRemove('ana'), { type: 'user_remove', name: 'ana', keep_home: false, confirm: '' });
  assert.deepEqual(C.userGroupAdd('ana', 'adm'), { type: 'user_group_add', name: 'ana', group: 'adm', confirm: '' });
  assert.deepEqual(C.userSetPassword('ana'), fx('command.user_set_password.generate') && { type: 'user_set_password', name: 'ana', password: '', current_password: '', confirm: '' });
  Types.values(Types.BridgeCommand).filter((t) => /^(user|group|ssh_key)_/.test(t) && t !== 'user_bootstrap_get')
    .forEach((t) => assert.equal(Bridge.QUEUEABLE[t], undefined, t + ' must not be queued (passwords, stale session)'));
});

test('users wire: every users event fixture decodes and passwords are redacted in logs', () => {
  ['user_session', 'user_session.locked', 'user_list', 'user_result', 'user_result.failed', 'user_secret', 'group_list', 'ssh_key_list', 'ssh_key_list.empty']
    .forEach((n) => { const m = fx('event.' + n); assert.deepEqual(Bridge.decode(m), { type: 'wire', msg: m }, n); });
  const pw = fx('event.user_secret').password;
  assert.equal(Logging.describe(fx('event.user_secret')).indexOf(pw), -1);
  assert.equal(Logging.describe(fx('command.user_unlock')).indexOf(fx('command.user_unlock').password), -1);
  assert.equal(Logging.describe(fx('command.user_set_password')).indexOf('oldpassword1234'), -1);
});

test('users reducer: session, lists, result, secret from the fixtures', () => {
  let s = fresh();
  assert.deepEqual(s.users.session, { unlocked: false, username: null, expiresInSecs: null });
  s = run(s, fx('event.user_session'), fx('event.user_list'), fx('event.group_list'), fx('event.ssh_key_list'), fx('event.user_result'), fx('event.user_secret'));
  assert.deepEqual(s.users.session, { unlocked: true, username: 'ana', expiresInSecs: 300 });
  assert.equal(s.users.list.length, 2);
  assert.equal(s.users.groups[0].name, 'sudo');
  assert.equal(s.users.keys.name, 'ana');
  assert.equal(s.users.keys.list.length, 1);
  assert.deepEqual(s.users.lastResult, { seq: 1, op: 'user_add', name: 'ana', success: true, error: null, message: null });
  assert.deepEqual(s.users.secret, { name: 'ana', password: 'Xe4mH7pQ9tLwZc3K' });
  s = run(s, fx('event.user_result.failed'));
  assert.equal(s.users.lastResult.seq, 2);
  assert.equal(s.users.lastResult.success, false);
  assert.equal(s.users.lastResult.error, 'protected_account');
});

test('users reducer: lock, not_unlocked and a bridge drop clear everything but bootstrap', () => {
  const loaded = () => run(fresh(), fx('event.user_bootstrap.showing'), fx('event.user_session'), fx('event.user_list'), fx('event.group_list'), fx('event.ssh_key_list'), fx('event.user_secret'));
  const bootstrap = loaded().users.bootstrap;
  assert.equal(bootstrap.phase, 'showing');
  const wiped = (s) => {
    assert.deepEqual(s.users, Object.assign({}, fresh().users, { bootstrap: bootstrap }, s.users.lastError ? { lastError: s.users.lastError } : {}));
    assert.equal(JSON.stringify(s.users).indexOf('Xe4mH7pQ9tLwZc3K'), -1, 'secret gone');
  };
  wiped(Reducers.reduce(loaded(), fx('event.user_session.locked')));
  const ne = Reducers.reduce(loaded(), { type: 'user_error', context: 'user_list_get', code: 'not_unlocked', message: 'x' });
  wiped(ne);
  assert.equal(ne.users.lastError.code, 'not_unlocked');
  wiped(Reducers.reduce(loaded(), { type: 'connection', which: 'bridge', status: 'disconnected' }));
  // agent drops and a live bridge keep the session
  assert.equal(Reducers.reduce(loaded(), { type: 'connection', which: 'agent', status: 'disconnected' }).users.session.unlocked, true);
  assert.equal(Reducers.reduce(loaded(), { type: 'connection', which: 'bridge', status: 'live' }).users.secret.name, 'ana');
});

test('users reducer: leave and clear_secret; first-boot errors are ignored; no mutation', () => {
  const s = run(fresh(), fx('event.user_session'), fx('event.user_list'), fx('event.user_secret'), fx('event.user_result'), fx('event.ssh_key_list'));
  const snap = JSON.stringify(s);
  const left = Reducers.reduce(s, { type: 'local/users_leave' });
  assert.equal(left.users.secret, null);
  assert.equal(left.users.lastResult, null);
  assert.equal(left.users.keys, null);
  assert.equal(left.users.list.length, 2, 'list stays, session still open');
  assert.equal(left.users.session.unlocked, true);
  const cleared = Reducers.reduce(s, { type: 'local/users_clear_secret' });
  assert.equal(cleared.users.secret, null);
  assert.equal(cleared.users.lastResult.op, 'user_add');
  assert.equal(Reducers.reduce(cleared, { type: 'local/users_clear_secret' }), cleared, 'no-op keeps the reference');
  assert.equal(Reducers.reduce(s, fx('event.user_error')), s, 'bootstrap context error is first-boot only');
  const e = Reducers.reduce(s, { type: 'user_error', context: 'user_add', code: 'rate_limited', message: 'slow' });
  assert.deepEqual(e.users.lastError, { seq: 1, context: 'user_add', code: 'rate_limited', message: 'slow' });
  assert.equal(JSON.stringify(s), snap, 'previous state not mutated');
});

test('users store: notifies on user events only when state changes', () => {
  const st = Store.create(Reducers.reduce, fresh());
  let n = 0;
  st.subscribe(() => { n++; });
  st.dispatch(fx('event.user_list'));
  st.dispatch({ type: 'local/users_clear_secret' });
  assert.equal(n, 1);
});

test('users selectors', () => {
  const s = run(fresh(), fx('event.user_session'), fx('event.user_list'), fx('event.group_list'), fx('event.ssh_key_list'), fx('event.user_secret'), fx('event.user_result.failed'));
  assert.equal(Selectors.isUnlocked(s), true);
  assert.equal(Selectors.sessionUser(s), 'ana');
  assert.equal(Selectors.userList(s).length, 2);
  assert.equal(Selectors.userByName(s, 'ana').key_count, 2);
  assert.equal(Selectors.userByName(s, 'zed'), null);
  assert.equal(Selectors.adminCount(s), 1);
  assert.equal(Selectors.groupList(s).length, 2);
  assert.deepEqual(Selectors.membership(s, 'kiosk'), [{ name: 'sudo', system: true, member: true }, { name: 'team', system: false, member: false }]);
  assert.equal(Selectors.keysFor(s, 'ana').length, 1);
  assert.equal(Selectors.keysFor(s, 'kiosk'), null);
  assert.equal(Selectors.pendingSecret(s).name, 'ana');
  assert.equal(Selectors.lastResult(s).error, 'protected_account');
  assert.equal(Selectors.lastError(s), null);
  const e = fresh();
  assert.equal(Selectors.isUnlocked(e), false);
  assert.deepEqual(Selectors.membership(e, 'x'), []);
  assert.equal(Selectors.adminCount(e), 0);
});

test('users logic: names', () => {
  assert.equal(UL.validateName('ana'), null);
  assert.equal(UL.validateName('a-b_c9'), null);
  assert.equal(UL.validateName('a'.repeat(31)), null);
  ['', 'A', '1a', '-a', 'a b', 'a.b', 'a:b', 'a'.repeat(32), null].forEach((n) => assert.notEqual(UL.validateName(n), null, String(n)));
  ['root', 'kiosk', 'sudo', 'systemd-x'].forEach((n) => assert.match(UL.validateName(n), /reserved/));
  assert.match(UL.validateName('ana', ['ana']), /in use/);
});

test('users logic: passwords', () => {
  assert.equal(UL.validatePassword('ana', 'hunter2hunter2'), null);
  assert.equal(UL.validatePassword('ana', fx('event.user_secret').password), null);
  assert.match(UL.validatePassword('ana', 'short1A'), /12/);
  assert.match(UL.validatePassword('ana', 'a'.repeat(129) + '1'), /At most/);
  assert.match(UL.validatePassword('ana', 'abcdefghijkl'), /two kinds/);
  assert.match(UL.validatePassword('ana', 'caf\u00e9 caf\u00e9 caf\u00e9 1'), /ASCII/);
  assert.match(UL.validatePassword('abcdefghij1k', 'abcdefghij1k'), /differ/);
  assert.notEqual(UL.validatePassword('ana', undefined), null);
  assert.equal(UL.isGenerate(''), true);
  assert.equal(UL.isGenerate('x'), false);
});

test('users logic: key line, fingerprint, labels', () => {
  const good = fx('command.ssh_key_add').key;
  assert.equal(UL.validateKeyLine(good), null);
  assert.equal(UL.validateKeyLine(good + '\n'), null);
  assert.equal(UL.validateKeyLine('ssh-ed25519 AAAAC3Nza'), null);
  assert.notEqual(UL.validateKeyLine(''), null);
  assert.notEqual(UL.validateKeyLine('   '), null);
  assert.match(UL.validateKeyLine('command="x" ' + good), /Unknown key type/);
  assert.match(UL.validateKeyLine(good + '\n' + good), /one line/);
  assert.match(UL.validateKeyLine('ssh-rsa not*base64'), /not valid/);
  assert.match(UL.validateKeyLine('ssh-rsa AAAA ' + 'c'.repeat(129)), /comment/);
  assert.match(UL.validateKeyLine('ssh-rsa ' + 'A'.repeat(9000)), /8 KiB/);
  const fp = fx('event.ssh_key_list').keys[0].fingerprint;
  assert.equal(UL.shortFingerprint(fp, 24).length, 24);
  assert.ok(UL.shortFingerprint(fp, 24).endsWith(fp.slice(-4)));
  assert.equal(UL.shortFingerprint('SHA256:x', 24), 'SHA256:x');
  assert.equal(UL.keyLabel(fx('event.ssh_key_list').keys[0]), 'ssh-ed25519 ana@laptop');
  assert.equal(UL.keyLabel({ key_type: 'ssh-rsa', comment: '' }), 'ssh-rsa');
});

test('users logic: confirm rules, blocks, countdown, texts', () => {
  assert.equal(UL.groupNeedsConfirm('sudo'), true);
  assert.equal(UL.groupNeedsConfirm({ name: 'adm', system: true }), false);
  assert.equal(UL.groupNeedsConfirm({ name: 'dialout', system: true }), true);
  assert.equal(UL.groupNeedsConfirm({ name: 'team', system: false }), false);
  assert.equal(UL.confirmFor('ana'), 'ana');
  const users = fx('event.user_list').users;
  assert.match(UL.userBlock(users[0], 'remove'), /protected/);
  assert.match(UL.userBlock(users[0], 'lock'), /protected/);
  assert.equal(UL.userBlock(users[0], 'password'), null);
  assert.equal(UL.userBlock(users[1], 'remove'), null);
  assert.match(UL.userBlock({ name: 'daemon', system: true }, 'password'), /System/);
  assert.equal(UL.countdown(247), '4:07');
  assert.equal(UL.countdown(null), '0:00');
  assert.equal(UL.countdown(-3), '0:00');
  assert.equal(UL.resultText(fx('event.user_result')), 'User ana added');
  assert.equal(UL.resultText(fx('event.user_result.failed')), UL.errorText('protected_account'));
  assert.equal(UL.resultText(null), '');
  Types.values(Types.UserErrorCode).forEach((c) => assert.notEqual(UL.errorText(c, 'fallback'), 'fallback', c));
});
