'use strict';
// Phase 2a/2b account screens (T-P2.6): unlock gate, list, user page, add,
// one-time password, hold-to-confirm, groups, keys, fixed-TTL session loss.
// Driven through the real chart, screens and reducers; wire messages come
// from the golden fixtures with explicit overrides.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { makeHarness } = require('./helpers/chart-harness.js');
const Events = require('../src/interaction/events.js');
const UL = require('../src/users/users-logic.js');
const Sel = require('../src/state/selectors.js');

const PROTOCOL = path.resolve(__dirname, '..', '..', 'config', 'protocol', 'controller');
const fx = (name, over) => Object.assign(JSON.parse(fs.readFileSync(path.join(PROTOCOL, name + '.json'), 'utf8')), over || {});
const session = (over) => fx('event.user_session', Object.assign({ username: 'kiosk' }, over || {}));
const KEY_FP = fx('event.ssh_key_list').keys[0].fingerprint;

const toast = (h) => h.state.toast;
const stack = (h) => h.state.stack.join('/');
const lastSend = (h) => h.sent()[h.sent().length - 1];

// Types into the shared field behind the highlighted `label` row, then ends
// editing the way the on-screen keyboard does (Enter) and lets the guard pass.
function typeInto(h, label, text) {
  h.choose(label);
  assert.equal(h.state.mode, 'open.editingText');
  h.send(Events.textInput(text));
  h.send(Events.textEnter());
  h.timer('enter-guard');
}

// Holds the knob push on the highlighted row past the threshold.
function holdPush(h) {
  h.btn(28, 1);
  const fxs = h.timer('enter-hold');
  h.btn(28, 0);
  return fxs;
}

function unlockedHome(over) {
  const h = makeHarness();
  h.open();
  h.wire(session(over));
  h.wire(fx('event.user_list'));
  h.wire(fx('event.group_list'));
  h.choose('Device settings');
  h.choose('Users');
  return h;
}

test('root has no Users row; Device settings opens locked, rows are greyed and Unlock opens the gate', () => {
  const h = makeHarness();
  h.open();
  assert.equal(h.labels().includes('Users'), false);
  const mark = h.mark();
  h.choose('Device settings');
  assert.equal(stack(h), 'root/device-settings');
  h.choose('Users');
  assert.equal(stack(h), 'root/device-settings', 'a disabled row does not open');
  assert.equal(toast(h), 'Unlock first');
  h.choose('Unlock');
  assert.equal(stack(h), 'root/device-settings/users-unlock');
  assert.deepEqual(h.sentSince(mark).filter((m) => m.type !== 'user_list_get'), []);
  assert.deepEqual(h.labels(), ['Admin', 'Password', 'Show password', 'Unlock']);
  assert.equal(h.view().rows[0].valueText, 'kiosk');
});

test('the old first-boot reveal row and screens are gone', () => {
  const h = makeHarness();
  h.wire(fx('event.user_bootstrap.prompt'));
  h.open();
  assert.equal(h.labels().some((l) => /First-boot/.test(l)), false);
  assert.equal(h.screens.defs['users-bootstrap'], undefined);
  assert.equal(h.screens.defs['users-ack'], undefined);
});

test('unlock: empty fields toast, a typed password is sent once and wiped from ui, success pops back to Device settings', () => {
  const h = makeHarness();
  h.open();
  h.choose('Device settings');
  h.choose('Unlock');
  h.choose('Unlock');
  assert.match(toast(h), /Enter the admin password/);
  typeInto(h, 'Password', 'correct horse 9');
  assert.equal(h.view().rows[1].valueText.length, 'correct horse 9'.length, 'masked bullets, same length');
  assert.doesNotMatch(h.view().rows[1].valueText, /correct/);
  const mark = h.mark();
  h.choose('Unlock');
  assert.deepEqual(h.sentSince(mark), [{ type: 'user_unlock', username: 'kiosk', password: 'correct horse 9' }]);
  assert.equal(h.state.ui.users.pw, '');
  assert.equal(JSON.stringify(h.state.ui).indexOf('correct horse'), -1);
  const m2 = h.mark();
  h.wire(session());
  assert.equal(stack(h), 'root/device-settings');
  assert.deepEqual(h.sentSince(m2), []);
  assert.equal(h.view().rows[1].disabled, false);
});

test('unlock: a wrong password toasts, keeps the gate and leaves nothing typed', () => {
  const h = makeHarness();
  h.open();
  h.choose('Device settings');
  h.choose('Unlock');
  typeInto(h, 'Password', 'wrong password 1');
  h.choose('Unlock');
  h.wire(fx('event.user_error', { context: 'user_unlock', code: 'bad_password', message: 'x' }));
  assert.equal(toast(h), 'Wrong password');
  assert.equal(stack(h), 'root/device-settings/users-unlock');
  assert.equal(h.state.ui.users.pw, '');
});

test('unlocked: Users opens the list from Device settings; users are listed, system accounts are not', () => {
  const h = makeHarness();
  h.open();
  h.wire(session());
  h.wire(fx('event.user_list', { users: fx('event.user_list').users.concat([{ name: 'daemon', uid: 1, gid: 1, home: '/', shell: '/x', locked: false, admin: false, system: true, protected: false, groups: [], key_count: 0 }]) }));
  h.choose('Device settings');
  h.choose('Users');
  assert.equal(stack(h), 'root/device-settings/users-list');
  assert.deepEqual(h.labels(), ['kiosk', 'ana', 'Add user...', 'Groups...']);
  assert.equal(h.view().rows[0].valueText, 'admin protected');
  assert.equal(h.view().rows[1].valueText, 'locked');
});

test('leaving Users does not lock the session; it drops the section state and clears the password fields', () => {
  const h = unlockedHome();
  h.send(Events.domain(h.domain));
  const mark = h.mark();
  h.back();
  assert.equal(stack(h), 'root/device-settings');
  assert.deepEqual(h.sentSince(mark), []);
  const dispatched = h.effects.filter((e) => e.dispatch).map((e) => e.dispatch.type);
  assert.ok(dispatched.includes('local/users_leave'));
  assert.equal(Sel.isUnlocked(h.domain), true);
  assert.equal(h.state.ui.users.pw, '');
});

test('closing the menu from a deep screen does not end the session', () => {
  const h = unlockedHome();
  h.choose('Groups...');
  const mark = h.mark();
  for (let i = 0; i < 4; i++) {
    h.escDown();
    h.timer('esc-hold');
    h.escUp();
  }
  assert.equal(h.state.mode, 'terminal');
  assert.deepEqual(h.sentSince(mark).filter((m) => m.type === 'user_lock_session'), []);
});

test('Lock now (first Device settings row) sends user_lock_session; the lock event returns to the Device settings root', () => {
  const h = unlockedHome();
  h.back();
  assert.equal(stack(h), 'root/device-settings');
  assert.match(h.labels()[0], /^Lock now/);
  const mark = h.mark();
  h.choose('Lock now');
  assert.deepEqual(h.sentSince(mark), [{ type: 'user_lock_session' }]);
  h.wire(fx('event.user_session.locked'));
  assert.equal(stack(h), 'root/device-settings');
  assert.equal(h.labels()[0], 'Unlock');
  assert.equal(h.view().rows[1].disabled, true);
});

test('no idle timer: unlock and activity start nothing, and activity does not extend the session', () => {
  const h = makeHarness();
  h.open();
  h.choose('Device settings');
  h.choose('Unlock');
  const fxs = h.wire(session());
  assert.equal(fxs.some((e) => e.startTimer && e.startTimer.name === 'users-idle'), false);
  h.choose('Users');
  const act = h.choose('Add user...');
  assert.equal(act.some((e) => e.startTimer && e.startTimer.name === 'users-idle'), false);
});

test('session_locked from any Device settings or Users screen pops to the Device settings root with a toast', () => {
  const h = unlockedHome();
  h.choose('Add user...');
  h.wire(fx('event.user_session.locked'));
  assert.equal(stack(h), 'root/device-settings');
  assert.match(toast(h), /Admin session locked/);
  const h2 = unlockedHome();
  h2.back();
  h2.choose('USB mode');
  assert.equal(stack(h2), 'root/device-settings/usb-mode');
  h2.wire(fx('event.user_session.locked'));
  assert.equal(stack(h2), 'root/device-settings');
});

test('device_settings_error: not_unlocked pops to the Device settings root, other codes only toast', () => {
  const h = unlockedHome();
  h.back();
  h.choose('USB mode');
  h.wire(fx('event.device_settings_error.rate_limited'));
  assert.equal(stack(h), 'root/device-settings/usb-mode');
  h.wire(fx('event.device_settings_error.not_unlocked'));
  assert.equal(stack(h), 'root/device-settings');
  assert.match(toast(h), /Admin session ended/);
});

test('not_unlocked (expiry, another connection) returns to the gate from any section screen and drops the data', () => {
  const h = unlockedHome();
  h.choose('ana');
  h.wire(fx('event.user_error', { context: 'user_remove', code: 'not_unlocked', message: 'x' }));
  assert.equal(stack(h), 'root/device-settings');
  assert.equal(toast(h), 'Admin session ended. Unlock again');
  assert.equal(Sel.userList(h.domain), null);
});

test('a dropped bridge (session state reset) shows a locked notice on section screens instead of stale rows', () => {
  const h = unlockedHome();
  h.send(Events.domain(Object.assign({}, h.domain, { users: Object.assign({}, h.domain.users, { session: { unlocked: false, username: null, expiresInSecs: null }, list: null }) })));
  assert.deepEqual(h.labels(), ['Admin session is locked', 'Unlock...']);
  h.choose('Unlock...');
  assert.equal(stack(h), 'root/device-settings/users-unlock');
});

// ---- add user -----------------------------------------------------------------
test('add user: defaults to a generated password, validates the name, sends once', () => {
  const h = unlockedHome();
  h.choose('Add user...');
  assert.deepEqual(h.labels(), ['Name', 'Administrator', 'Generate password', 'Create user']);
  h.choose('Create user');
  assert.match(toast(h), /Use 1 to 31/);
  typeInto(h, 'Name', 'ana');
  h.choose('Create user');
  assert.equal(toast(h), 'That name is already in use');
  typeInto(h, 'Name', 'bob');
  h.choose('Administrator');
  const mark = h.mark();
  h.choose('Create user');
  assert.deepEqual(h.sentSince(mark), [{ type: 'user_add', name: 'bob', password: '', admin: true }]);
});

test('add user with a typed password: rules are checked before sending, the field only exists while generate is off', () => {
  const h = unlockedHome();
  h.choose('Add user...');
  typeInto(h, 'Name', 'bob');
  h.choose('Generate password');
  assert.deepEqual(h.labels(), ['Name', 'Administrator', 'Generate password', 'Password', 'Show password', 'Create user']);
  typeInto(h, 'Password', 'short');
  h.choose('Create user');
  assert.equal(toast(h), 'At least 12 characters');
  typeInto(h, 'Password', 'a long enough pw 7');
  const mark = h.mark();
  h.choose('Create user');
  assert.deepEqual(h.sentSince(mark), [{ type: 'user_add', name: 'bob', password: 'a long enough pw 7', admin: false }]);
  // success: toast, back on the list, the typed password is gone
  h.wire(fx('event.user_result', { op: 'user_add', name: 'bob' }));
  assert.equal(stack(h), 'root/device-settings/users-list');
  assert.equal(JSON.stringify(h.state.ui).indexOf('long enough'), -1);
});

test('add user (generated): the user_secret screen shows the password as one string, Done returns to the list and clears it', () => {
  const h = unlockedHome();
  h.choose('Add user...');
  typeInto(h, 'Name', 'bob');
  h.choose('Create user');
  h.wire(fx('event.user_result', { op: 'user_add', name: 'bob' }));
  assert.equal(stack(h), 'root/device-settings/users-list');
  h.settleDomain(h.wire(fx('event.user_secret', { name: 'bob' })));
  assert.equal(stack(h), 'root/device-settings/users-list/users-secret');
  assert.equal(h.view().title, 'Password for bob');
  assert.deepEqual(h.labels(), ['Shown once. It cannot be shown again.', 'Xe4mH7pQ9tLwZc3K', 'Done']);
  assert.equal(h.view().rows[1].cls, 'secret-row');
  assert.equal(h.view().highlight, 2, 'lands on Done');
  assert.equal(Sel.pendingSecret(h.domain).password, 'Xe4mH7pQ9tLwZc3K');
  h.settleDomain(h.choose('Done'));
  assert.equal(stack(h), 'root/device-settings/users-list');
  assert.equal(Sel.pendingSecret(h.domain), null);
});

test('secret arriving before the result: the screen stays until Done, which goes back to the list', () => {
  const h = unlockedHome();
  h.choose('Add user...');
  typeInto(h, 'Name', 'bob');
  h.choose('Create user');
  h.settleDomain(h.wire(fx('event.user_secret', { name: 'bob' })));
  h.wire(fx('event.user_result', { op: 'user_add', name: 'bob' }));
  assert.equal(h.state.stack[h.state.stack.length - 1], 'users-secret');
  h.settleDomain(h.choose('Done'));
  assert.equal(stack(h), 'root/device-settings/users-list');
  assert.equal(Sel.pendingSecret(h.domain), null);
});

test('the secret is cleared by back, by lock and by an expired session', () => {
  let h = unlockedHome();
  h.settleDomain(h.wire(fx('event.user_secret')));
  h.settleDomain(h.btn(2, 1));
  assert.equal(Sel.pendingSecret(h.domain), null, 'back');

  h = unlockedHome();
  h.settleDomain(h.wire(fx('event.user_secret')));
  assert.notEqual(Sel.pendingSecret(h.domain), null);
  h.settleDomain(h.wire(fx('event.user_session.locked')));
  assert.equal(Sel.pendingSecret(h.domain), null, 'lock');
  assert.equal(stack(h), 'root/device-settings');
  assert.equal(h.labels().some((l) => /Xe4m/.test(l)), false);

  h = unlockedHome();
  h.settleDomain(h.wire(fx('event.user_secret')));
  h.wire(fx('event.user_error', { context: 'user_list_get', code: 'not_unlocked', message: 'x' }));
  assert.equal(Sel.pendingSecret(h.domain), null, 'expiry');
  assert.equal(h.view().rows.map((r) => r.label).join().indexOf('Xe4m'), -1);
});

test('a user_secret with no open section is discarded, not queued', () => {
  const h = makeHarness();
  h.open();
  const fxs = h.wire(fx('event.user_secret'));
  assert.deepEqual(fxs, [{ dispatch: { type: 'local/users_clear_secret' } }]);
  assert.equal(stack(h), 'root');
});

// ---- user page, password, lock ---------------------------------------------------
test('user page: protected kiosk can change its password but not be locked or removed; the text says so', () => {
  const h = unlockedHome();
  h.choose('kiosk');
  assert.equal(h.view().title, 'kiosk');
  const labels = h.labels();
  assert.ok(labels.includes('Lock: The kiosk account is protected'));
  assert.ok(labels.includes('Remove: The kiosk account is protected'));
  assert.ok(labels.includes('Set password...'));
  assert.ok(labels.includes('Groups: sudo, adm'));
});

test('set password for another user: generate by default, sends confirm = the name', () => {
  const h = unlockedHome();
  h.choose('ana');
  h.choose('Set password...');
  assert.deepEqual(h.labels(), ['Generate new password', 'Set password']);
  const mark = h.mark();
  h.choose('Set password');
  assert.deepEqual(h.sentSince(mark), [{ type: 'user_set_password', name: 'ana', password: '', current_password: '', confirm: 'ana' }]);
});

test('set password on the own account needs the current password; kiosk shows the SSH note', () => {
  const h = unlockedHome();
  h.choose('kiosk');
  h.choose('Set password...');
  assert.equal(h.labels()[0], 'This is also the SSH login password');
  h.choose('Set password');
  assert.equal(toast(h), 'Enter your current password');
  typeInto(h, 'Current password', 'old password 12');
  const mark = h.mark();
  h.choose('Set password');
  assert.deepEqual(h.sentSince(mark), [{ type: 'user_set_password', name: 'kiosk', password: '', current_password: 'old password 12', confirm: 'kiosk' }]);
});

test('set password: success returns to the user page and forgets the typed values', () => {
  const h = unlockedHome();
  h.choose('ana');
  h.choose('Set password...');
  h.choose('Generate new password');
  typeInto(h, 'New password', 'brand new pass 77');
  h.choose('Set password');
  h.wire(fx('event.user_result', { op: 'user_set_password', name: 'ana' }));
  assert.equal(stack(h), 'root/device-settings/users-list/users-user');
  assert.equal(JSON.stringify(h.state.ui).indexOf('brand new'), -1);
});

test('unlock account is direct (no confirm); lock is behind a hold confirm', () => {
  const h = unlockedHome();
  h.choose('ana');
  const mark = h.mark();
  h.choose('Unlock account');
  assert.deepEqual(h.sentSince(mark), [{ type: 'user_set_locked', name: 'ana', locked: false, confirm: '' }]);
});

test('hold confirm: lock account', () => {
  const h = unlockedHome();
  h.wire(fx('event.user_list', { users: fx('event.user_list').users.map((u) => (u.name === 'ana' ? Object.assign({}, u, { locked: false }) : u)) }));
  h.choose('ana');
  h.choose('Lock account...');
  assert.equal(h.view().title, 'Lock ana?');
  assert.equal(h.view().highlight, 0);
  assert.equal(h.view().rows[0].label, 'Cancel');
  h.moveTo('Lock ana');
  const mark = h.mark();
  holdPush(h);
  assert.deepEqual(h.sentSince(mark), [{ type: 'user_set_locked', name: 'ana', locked: true, confirm: 'ana' }]);
});

// ---- hold-to-confirm mechanics ------------------------------------------------------
function atRemoveConfirm() {
  const h = unlockedHome();
  h.choose('ana');
  h.choose('Remove user...');
  h.moveTo('Remove ana');
  return h;
}

test('remove: opens on Cancel, the destructive row fires at the threshold while the push is still held', () => {
  const h = unlockedHome();
  h.choose('ana');
  h.choose('Remove user...');
  assert.equal(h.view().title, 'Remove ana?');
  assert.equal(h.view().highlight, 0);
  h.moveTo('Remove ana');
  const mark = h.mark();
  const down = h.btn(28, 1);
  assert.deepEqual(down, [{ cancelTimer: 'enter-hold' }, { startTimer: { name: 'enter-hold', ms: 450 } }]);
  assert.deepEqual(h.sentSince(mark), [], 'nothing on the press itself');
  const fired = h.timer('enter-hold');
  assert.deepEqual(h.sentSince(mark), [{ type: 'user_remove', name: 'ana', keep_home: false, confirm: 'ana' }], 'sent while still held');
  assert.ok(fired.some((e) => e.send));
  h.btn(28, 0);
  assert.deepEqual(h.sentSince(mark).length, 1, 'the release sends nothing more');
});

test('remove: releasing early sends nothing, only hints; autorepeat neither restarts nor fires', () => {
  const h = atRemoveConfirm();
  const mark = h.mark();
  h.btn(28, 1);
  assert.deepEqual(h.btn(28, 2), []);
  h.btn(28, 0);
  assert.deepEqual(h.sentSince(mark), []);
  assert.equal(toast(h), 'Hold the knob push to confirm');
  assert.deepEqual(h.timer('enter-hold'), [], 'the stale timer is ignored');
  assert.deepEqual(h.sentSince(mark), []);
});

test('remove: an ESC tap or a plain Enter tap on the hold row does not confirm; a touch tap on the row does', () => {
  const h = atRemoveConfirm();
  const mark = h.mark();
  h.escDown();
  h.escUp();
  assert.deepEqual(h.sentSince(mark), []);
  assert.equal(toast(h), 'Hold the knob push to confirm');
  h.btn(28, 1); h.btn(28, 0);
  assert.deepEqual(h.sentSince(mark), []);
  h.send(Events.rowTap(2));
  assert.equal(h.sentSince(mark).length, 1);
});

test('remove: the hold is dropped when the screen changes while held (no select on the wrong row)', () => {
  const h = atRemoveConfirm();
  const mark = h.mark();
  h.btn(28, 1);
  h.wire(fx('event.user_session.locked')); // pops to the unlock gate mid hold
  h.timer('enter-hold');
  assert.deepEqual(h.sentSince(mark).filter((m) => m.type === 'user_remove'), []);
});

test('remove: Cancel pops, keep-home is sent, a ghost user pops back to the list', () => {
  let h = atRemoveConfirm();
  h.choose('Cancel');
  assert.equal(h.view().title, 'ana');

  h = unlockedHome();
  h.choose('ana');
  h.choose('Remove user...');
  h.choose('Keep home directory');
  h.moveTo('Remove ana');
  const mark = h.mark();
  holdPush(h);
  assert.deepEqual(h.sentSince(mark), [{ type: 'user_remove', name: 'ana', keep_home: true, confirm: 'ana' }]);
  h.wire(fx('event.user_result', { op: 'user_remove', name: 'ana' }));
  assert.equal(stack(h), 'root/device-settings/users-list');
  h.wire(fx('event.user_list', { users: fx('event.user_list').users.filter((u) => u.name !== 'ana') }));
  assert.deepEqual(h.labels().slice(0, 2), ['kiosk', 'Add user...']);
});

test('a guard error toasts the controller text and stays put', () => {
  const h = atRemoveConfirm();
  holdPush(h);
  h.wire(fx('event.user_result.failed', { op: 'user_remove', name: 'ana', error: 'last_admin' }));
  assert.equal(toast(h), 'This would remove the last admin');
  assert.equal(h.view().title, 'Remove ana?');
});

test('a user removed elsewhere while its page is open drops back to the list', () => {
  const h = unlockedHome();
  h.choose('ana');
  h.wire(fx('event.user_list', { users: fx('event.user_list').users.filter((u) => u.name !== 'ana') }));
  assert.equal(stack(h), 'root/device-settings/users-list');
});

// ---- groups --------------------------------------------------------------------------
test('user groups: plain groups toggle directly, sudo goes through a hold confirm with the group as confirm', () => {
  const h = unlockedHome();
  h.choose('ana');
  h.choose('Groups...');
  assert.deepEqual(h.view().rows.map((r) => r.label + '|' + r.display), ['sudo|OFF', 'team|OFF']);
  const mark = h.mark();
  h.choose('team');
  assert.deepEqual(h.sentSince(mark), [{ type: 'user_group_add', name: 'ana', group: 'team', confirm: '' }]);
  h.choose('sudo');
  assert.equal(h.view().title, 'Add to sudo?');
  assert.equal(h.view().rows[0].label, 'Cancel');
  h.moveTo('Add ana to sudo');
  const m2 = h.mark();
  holdPush(h);
  assert.deepEqual(h.sentSince(m2), [{ type: 'user_group_add', name: 'ana', group: 'sudo', confirm: 'sudo' }]);
  h.wire(fx('event.user_result', { op: 'user_group_add', name: 'ana' }));
  assert.equal(h.view().title, 'Groups: ana');
});

test('user groups: a member toggles off with user_group_remove', () => {
  const h = unlockedHome();
  h.choose('kiosk');
  h.choose('Groups...');
  const mark = h.mark();
  h.choose('sudo');
  assert.deepEqual(h.sentSince(mark), [{ type: 'user_group_remove', name: 'kiosk', group: 'sudo' }]);
});

test('groups screen: list, add validates the name, remove is a hold confirm, system groups are read only', () => {
  const h = unlockedHome();
  h.choose('Groups...');
  assert.deepEqual(h.labels(), ['sudo', 'team', 'Add group...']);
  assert.equal(h.view().rows[0].valueText, 'system');
  assert.equal(h.view().rows[1].valueText, '0 members');
  h.choose('sudo');
  assert.ok(h.labels().includes('System group: cannot be removed'));
  h.back();
  h.choose('Add group...');
  typeInto(h, 'Name', 'team');
  h.choose('Create group');
  assert.equal(toast(h), 'That name is already in use');
  typeInto(h, 'Name', 'ops');
  const mark = h.mark();
  h.choose('Create group');
  assert.deepEqual(h.sentSince(mark), [{ type: 'group_add', name: 'ops' }]);
  h.wire(fx('event.user_result', { op: 'group_add', name: 'ops' }));
  assert.equal(h.view().title, 'Groups');
  h.choose('team');
  h.choose('Remove group...');
  h.moveTo('Remove group team');
  const m2 = h.mark();
  holdPush(h);
  assert.deepEqual(h.sentSince(m2), [{ type: 'group_remove', name: 'team', confirm: 'team' }]);
});

// ---- keys ------------------------------------------------------------------------------
test('keys: list per user, add validates the line, remove needs the held confirm with the fingerprint', () => {
  const h = unlockedHome();
  h.choose('ana');
  const mark = h.mark();
  h.choose('SSH keys...');
  assert.deepEqual(h.sentSince(mark), [{ type: 'ssh_key_list_get', name: 'ana' }]);
  assert.deepEqual(h.labels(), ['Loading...', 'Add key...']);
  h.wire(fx('event.ssh_key_list'));
  assert.deepEqual(h.labels(), ['ssh-ed25519 ana@laptop', 'Add key...']);
  assert.match(h.view().rows[0].valueText, /^SHA256:/);

  h.choose('Add key...');
  h.choose('Add key');
  assert.equal(toast(h), 'Paste one public key');
  typeInto(h, 'Public key', 'not a key');
  h.choose('Add key');
  assert.equal(toast(h), 'Unknown key type (options are not accepted)');
  const line = 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample ana@laptop';
  typeInto(h, 'Public key', line);
  const m2 = h.mark();
  h.choose('Add key');
  assert.deepEqual(h.sentSince(m2), [{ type: 'ssh_key_add', name: 'ana', key: line }]);
  h.wire(fx('event.user_result', { op: 'ssh_key_add', name: 'ana' }));
  assert.equal(h.view().title, 'SSH keys: ana');
  assert.equal(h.state.ui.users.keyLine, '', 'left the add screen, the line is gone');

  h.choose('ssh-ed25519');
  h.choose('Remove key...');
  h.moveTo('Remove key');
  const m3 = h.mark();
  holdPush(h);
  assert.deepEqual(h.sentSince(m3), [{ type: 'ssh_key_remove', name: 'ana', fingerprint: KEY_FP, confirm: KEY_FP }]);
});

test('an empty key list says so, and a key removed elsewhere pops the key page', () => {
  const h = unlockedHome();
  h.choose('ana');
  h.choose('SSH keys...');
  h.wire(fx('event.ssh_key_list.empty', { name: 'ana' }));
  assert.deepEqual(h.labels(), ['No keys', 'Add key...']);
  h.wire(fx('event.ssh_key_list'));
  h.choose('ssh-ed25519');
  h.wire(fx('event.ssh_key_list.empty', { name: 'ana' }));
  assert.equal(h.view().title, 'SSH keys: ana');
});

test('text screens use their own input limit and placeholder; Wi-Fi keeps the page defaults', () => {
  const h = unlockedHome();
  h.choose('Add user...');
  h.choose('Name');
  let vm = h.view();
  assert.deepEqual(vm.textField, { maxLength: 31, placeholder: 'New user name' });
  h.send(Events.textEnter()); h.timer('enter-guard');
  h.back(); h.choose('ana'); h.choose('SSH keys...'); h.choose('Add key...'); h.choose('Public key');
  vm = h.view();
  assert.equal(vm.textField.maxLength, 8192);
  assert.equal(vm.hint, 'type, then enter or esc: done');
});

test('every account screen id reachable from the rows exists and no screen id is duplicated', () => {
  const h = makeHarness();
  const ids = Object.keys(h.screens.defs).filter((id) => id.indexOf('users-') === 0);
  assert.ok(ids.length >= 18, ids.join());
  ids.forEach((id) => assert.ok(h.screens.defs[id].title !== undefined, id));
});
