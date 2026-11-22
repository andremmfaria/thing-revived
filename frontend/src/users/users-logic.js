/*
 * Pure helpers for the first-boot password screens (docs/research/user-management-plan.md
 * sections 5, 8 and 10). No DOM and no globals from other modules except the
 * wifi picker ring, which the last-four entry reuses, so the same file loads as a
 * classic <script> in the kiosk (window.TR.usersLogic) and under node for the tests.
 *
 * Nothing here logs or stores a password; callers keep the secret only in the
 * interaction state of the screen that shows it (ui.users.secret).
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.usersLogic).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var WL = node ? require('../wifi/wifi-logic.js') : TR.wifiLogic;

  var ACK_LEN = 4;
  var GROUP_LEN = 4;
  var TICK_MS = 1000;

  // user_error.code -> text for the screen. Unknown codes fall back to the
  // controller's own message, then to a generic line.
  var ERROR_TEXT = {
    press_required: 'Press the knob right before asking again, then try once more',
    bad_ack: 'Those four characters do not match. Check the password and try again',
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

  // 885 -> '14:45'.
  function countdown(secs) {
    if (typeof secs !== 'number' || secs < 0) return '--:--';
    var m = Math.floor(secs / 60);
    var s = secs % 60;
    return m + ':' + (s < 10 ? '0' : '') + s;
  }

  // What the first-boot flow looks like for a domain.users.bootstrap value:
  //   unknown   no user_bootstrap event yet
  //   pending   a secret is waiting to be collected
  //   expired   not acknowledged and no longer pending (not collected in time)
  //   done      acknowledged
  function phase(bootstrap) {
    if (!bootstrap) return 'unknown';
    if (bootstrap.pending) return 'pending';
    return bootstrap.acknowledged ? 'done' : 'expired';
  }

  // Knob picker for the confirmation characters: the wifi ring, capped at 4.
  function ackApply(entry, idx) {
    var item = WL.PICKER_RING[idx];
    if (item === WL.TOKEN_DONE) return { entry: entry, done: true };
    if (item === WL.TOKEN_DEL) return { entry: entry.slice(0, -1), done: false };
    if (entry.length >= ACK_LEN) return { entry: entry, done: false };
    return { entry: entry + item, done: false };
  }

  // Entered characters plus placeholders: 'k7' -> 'k7__'.
  function ackDisplay(entry) {
    var out = entry;
    while (out.length < ACK_LEN) out += '_';
    return out;
  }

  var api = {
    ACK_LEN: ACK_LEN,
    GROUP_LEN: GROUP_LEN,
    TICK_MS: TICK_MS,
    errorText: errorText,
    groups: groups,
    countdown: countdown,
    phase: phase,
    ackApply: ackApply,
    ackDisplay: ackDisplay,
  };

  if (node) module.exports = api;
  else TR.usersLogic = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
