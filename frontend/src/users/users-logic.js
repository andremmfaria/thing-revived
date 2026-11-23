/*
 * Pure helpers for the first-boot password screen (docs/research/
 * user-management-plan-delta-password-auth.md sections 5 and 6). No DOM; the
 * same file loads as a classic <script> in the kiosk (window.TR.usersLogic) and
 * under node for the tests.
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
  };

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

  var api = {
    GROUP_LEN: GROUP_LEN,
    errorText: errorText,
    groups: groups,
    phase: phase,
  };

  if (node) module.exports = api;
  else TR.usersLogic = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
