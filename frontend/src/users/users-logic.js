/*
 * Pure helpers for the first-boot password screen (docs/research/
 * user-management-plan-delta-password-auth.md sections 5 and 6) and for the
 * Phase 2a/2b account menu (validation that mirrors the controller, confirm
 * values, key line sanity check, error text). No DOM; the same file loads as a
 * classic <script> in the kiosk (window.TR.usersLogic) and under node for the
 * tests. The controller stays the authority: these checks only save a round
 * trip and give a message before sending.
 *
 * Nothing here logs or stores a password.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.usersLogic).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});

  var GROUP_LEN = 4;

  // user_error.code -> text for the screen. Unknown codes fall back to the
  // controller's own message, then to a generic line.
  var ERROR_TEXT = {
    generate_failed: 'Could not generate the password. Press M to try again',
    ack_failed: 'Could not save the confirmation. Press M again to retry',
    unavailable: 'Account setup is not available on this controller yet',
    storage_unavailable: 'Account storage is not available. Reboot the device',
    internal: 'Internal controller error. Try again, or reboot the device',
    not_unlocked: 'Admin session ended. Unlock again',
    bad_password: 'Wrong password',
    rate_limited: 'Too many attempts. Wait a moment and try again',
    invalid_name: 'Invalid name',
    invalid_password: 'Password does not meet the rules',
    invalid_key: 'Not a valid SSH public key',
    exists: 'That name is already in use',
    leftover_state: 'Leftover files from an earlier account block this name',
    not_found: 'Not found',
    system_account: 'System accounts cannot be changed here',
    protected_account: 'This account is protected',
    last_admin: 'This would remove the last admin',
    group_not_assignable: 'That group cannot be assigned here',
    too_many: 'Limit reached',
    confirm_required: 'Confirmation needed',
    no_password: 'That account has no password yet',
    sessions_remain: 'The user is still logged in',
    user_busy: 'The user has running processes',
  };

  // Mirrors controller domain/accounts/{names,password,keys,groups}.rs.
  var NAME_RE = /^[a-z_][a-z0-9_-]{0,30}$/;
  var RESERVED_NAMES = ['root', 'nobody', 'kiosk', 'sudo', 'adm', 'wheel', 'shadow', 'disk', 'kmem', 'tty', 'utmp', 'staff', 'sshd', 'messagebus'];
  var MIN_PASSWORD_LEN = 12;
  var MAX_PASSWORD_LEN = 128;
  var MAX_KEY_BYTES = 8 * 1024;
  var MAX_COMMENT_LEN = 128;
  var KEY_TYPES = ['ssh-ed25519', 'ssh-rsa', 'ecdsa-sha2-nistp256', 'ecdsa-sha2-nistp384', 'ecdsa-sha2-nistp521', 'sk-ssh-ed25519@openssh.com', 'sk-ecdsa-sha2-nistp256@openssh.com'];
  var ADMIN_GROUP = 'sudo';
  var KIOSK_USER = 'kiosk';
  // Idle time after which the menu sends user_lock_session (plan section 10).
  var IDLE_LOCK_MS = 5 * 60 * 1000;

  function errorText(code, message) {
    if (typeof code === 'string' && Object.prototype.hasOwnProperty.call(ERROR_TEXT, code)) return ERROR_TEXT[code];
    if (typeof message === 'string' && message.length) return message;
    return 'Could not complete the request';
  }

  // 'Xe4mH7pQ9tLwZc3K' -> ['Xe4m', 'H7pQ', '9tLw', 'Zc3K'].
  function groups(pw) {
    var out = [];
    for (var i = 0; i < pw.length; i += GROUP_LEN) out.push(pw.slice(i, i + GROUP_LEN));
    return out;
  }

  // The controller's phase for a domain.users.bootstrap value, 'unknown'
  // before the first user_bootstrap event.
  function phase(bootstrap) {
    return bootstrap && typeof bootstrap.phase === 'string' ? bootstrap.phase : 'unknown';
  }

  function has(list, v) { return list.indexOf(v) !== -1; }

  // null when `name` may be created (user or group), else a message.
  // `taken` is an optional list of existing user and group names.
  function validateName(name, taken) {
    if (typeof name !== 'string' || !NAME_RE.test(name)) return 'Use 1 to 31 of a-z, 0-9, _ and -, not starting with a digit or -';
    if (has(RESERVED_NAMES, name) || name.indexOf('systemd-') === 0) return 'This name is reserved';
    if (taken && has(taken, name)) return 'That name is already in use';
    return null;
  }

  function classOf(c) {
    if (c >= 'a' && c <= 'z') return 0;
    if (c >= 'A' && c <= 'Z') return 1;
    if (c >= '0' && c <= '9') return 2;
    return 3;
  }

  // null when acceptable for `name`, else a message. An empty password means
  // "generate" on the wire, so callers check isGenerate() first.
  function validatePassword(name, pw) {
    if (typeof pw !== 'string') return 'Enter a password';
    var seen = [false, false, false, false];
    for (var i = 0; i < pw.length; i++) {
      var code = pw.charCodeAt(i);
      if (code < 0x20 || code > 0x7e) return 'Printable ASCII characters only';
      seen[classOf(pw.charAt(i))] = true;
    }
    if (pw.length < MIN_PASSWORD_LEN) return 'At least ' + MIN_PASSWORD_LEN + ' characters';
    if (pw.length > MAX_PASSWORD_LEN) return 'At most ' + MAX_PASSWORD_LEN + ' characters';
    if (pw === name) return 'Must differ from the account name';
    if (seen.filter(function (x) { return x; }).length < 2) return 'Mix at least two kinds of characters';
    return null;
  }

  function isGenerate(pw) { return pw === ''; }

  // Sanity check of one pasted authorized_keys line: type, base64 blob, short
  // comment, no options. The controller parses it for real.
  function validateKeyLine(line) {
    if (typeof line !== 'string') return 'Paste one public key';
    var text = line.replace(/\r?\n$/, '');
    if (text.trim() === '') return 'Paste one public key';
    if (text.length > MAX_KEY_BYTES) return 'Key is longer than 8 KiB';
    if (/[\u0000-\u001f\u007f]/.test(text)) return 'Paste exactly one key on one line';
    var parts = text.trim().split(' ');
    if (!has(KEY_TYPES, parts[0])) return 'Unknown key type (options are not accepted)';
    if (!parts[1] || !/^[A-Za-z0-9+\/]+={0,2}$/.test(parts[1])) return 'The key data is not valid';
    var comment = parts.slice(2).join(' ');
    if (comment.length > MAX_COMMENT_LEN) return 'The comment is too long';
    return null;
  }

  // The `confirm` string a destructive request needs: the target itself.
  function confirmFor(target) { return target; }

  // user_group_add needs confirm for sudo and for any system group beyond adm.
  // `group` is a group_list entry ({name, system}) or a bare name.
  function groupNeedsConfirm(group) {
    var name = typeof group === 'string' ? group : group.name;
    var system = typeof group === 'string' ? false : group.system === true;
    return name === ADMIN_GROUP || (system && name !== 'adm');
  }

  // Why a user row may not be removed, locked or have its password reset from
  // the menu, or null. Mirrors the controller guards; it enforces them again.
  function userBlock(user, op) {
    if (!user) return 'Unknown user';
    if (user.system) return 'System accounts cannot be changed here';
    if (user.protected && (op === 'remove' || op === 'lock')) return 'The ' + user.name + ' account is protected';
    return null;
  }

  // 'SHA256:abc...' -> 'SHA256:abc123de...v' fitted to `max` characters, for
  // a list row; the full value stays the confirm string.
  function shortFingerprint(fp, max) {
    max = max || 24;
    if (typeof fp !== 'string' || fp.length <= max) return fp;
    var tail = 4;
    return fp.slice(0, max - tail - 3) + '...' + fp.slice(fp.length - tail);
  }

  // Row text for a key from ssh_key_list.keys.
  function keyLabel(k) {
    return k.key_type + (k.comment ? ' ' + k.comment : '');
  }

  // Seconds left of the admin session -> '4:07'; null/negative -> '0:00'.
  function countdown(secs) {
    var n = typeof secs === 'number' && secs > 0 ? Math.floor(secs) : 0;
    var s = n % 60;
    return Math.floor(n / 60) + ':' + (s < 10 ? '0' : '') + s;
  }

  // user_result.op is the request type; each maps to a sentence about the name.
  var OP_TEXT = {
    user_add: function (n) { return 'User ' + n + ' added'; },
    user_remove: function (n) { return 'User ' + n + ' removed'; },
    user_set_locked: function (n) { return 'User ' + n + ' lock changed'; },
    user_set_password: function (n) { return 'Password changed for ' + n; },
    user_group_add: function (n) { return 'Group added for ' + n; },
    user_group_remove: function (n) { return 'Group removed for ' + n; },
    group_add: function (n) { return 'Group ' + n + ' added'; },
    group_remove: function (n) { return 'Group ' + n + ' removed'; },
    ssh_key_add: function (n) { return 'SSH key added for ' + n; },
    ssh_key_remove: function (n) { return 'SSH key removed for ' + n; }
  };

  // A user_result as one line for a toast.
  function resultText(r) {
    if (!r) return '';
    if (r.success) return OP_TEXT[r.op] ? OP_TEXT[r.op](r.name) : r.name + ': done';
    return errorText(r.error, r.message);
  }

  var api = {
    NAME_RE: NAME_RE,
    MIN_PASSWORD_LEN: MIN_PASSWORD_LEN,
    MAX_PASSWORD_LEN: MAX_PASSWORD_LEN,
    KEY_TYPES: KEY_TYPES,
    IDLE_LOCK_MS: IDLE_LOCK_MS,
    KIOSK_USER: KIOSK_USER,
    validateName: validateName,
    validatePassword: validatePassword,
    isGenerate: isGenerate,
    validateKeyLine: validateKeyLine,
    confirmFor: confirmFor,
    groupNeedsConfirm: groupNeedsConfirm,
    userBlock: userBlock,
    shortFingerprint: shortFingerprint,
    keyLabel: keyLabel,
    countdown: countdown,
    resultText: resultText,
    GROUP_LEN: GROUP_LEN,
    errorText: errorText,
    groups: groups,
    phase: phase,
  };

  if (node) module.exports = api;
  else TR.usersLogic = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
