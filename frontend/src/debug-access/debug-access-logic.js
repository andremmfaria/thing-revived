/*
 * Pure helpers for the Debug access settings screen (docs/research/
 * debug-toggles-plan.md sections 5.3 and 6): the fixed confirm strings the
 * hold gesture sends (never typed), and debug_access_error.code -> words for
 * the persistent error line. No DOM; dual-load (browser: TR.debugAccessLogic).
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.debugAccessLogic).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});

  // The kind's canonical confirm string (plan section 5.2 item 2): the hold
  // gesture sends this, it is never typed by the user.
  var CONFIRM = { adb: 'adb', uart: 'uart' };

  // debug_access_error.code -> text for the persistent error line. Unknown
  // codes fall back to the controller's own message, then a generic line.
  var ERROR_TEXT = {
    not_unlocked: 'Admin session ended. Unlock again',
    confirm_required: 'Confirmation needed',
    invalid_surface: 'Invalid debug surface',
    usb_role_host: 'USB role is host: ADB needs device mode',
    toggle_failed: 'Could not change the setting',
    busy: 'Another toggle is already in progress. Try again in a moment',
    unavailable: 'Debug access is not available on this device right now',
    rate_limited: 'Too many attempts. Wait a moment and try again',
  };

  function errorText(code, message) {
    if (typeof code === 'string' && Object.prototype.hasOwnProperty.call(ERROR_TEXT, code)) return ERROR_TEXT[code];
    if (typeof message === 'string' && message.length) return message;
    return 'Could not complete the request';
  }

  var api = { CONFIRM: CONFIRM, ERROR_TEXT: ERROR_TEXT, errorText: errorText };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.debugAccessLogic = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
