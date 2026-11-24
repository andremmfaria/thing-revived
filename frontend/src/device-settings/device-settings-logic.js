/*
 * Pure helpers for the Device settings screens (docs/research/
 * device-settings-plan.md sections 3.2 and 8.1): device_settings_error.code to
 * plain words, the mm:ss unlock countdown, and row/note label helpers. No DOM.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.deviceSettingsLogic).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;

  var SETTINGS = ['mode', 'ssh', 'adb', 'uart'];

  var SETTING_NAMES = { mode: 'Mode', ssh: 'SSH', adb: 'ADB', uart: 'UART' };

  // Plain words per device_settings_error.code. Unknown codes fall back to the
  // controller's own message, then a generic line.
  var ERROR_TEXT = {
    not_unlocked: 'Admin session ended. Unlock again',
    invalid_setting: 'That setting does not exist',
    invalid_value: 'That value is not allowed',
    adb_blocked_host: 'ADB needs Device mode. Switch the USB port to Device first',
    busy: 'Another change is in progress. Try again in a moment',
    rate_limited: 'Too many changes. Wait a minute and try again',
    persist_failed: 'Could not save the setting',
    apply_failed: 'Could not apply the setting',
    unavailable: 'Device settings are not available right now',
  };

  function has(map, k) { return typeof k === 'string' && Object.prototype.hasOwnProperty.call(map, k); }

  function errorText(code, message) {
    if (has(ERROR_TEXT, code)) return ERROR_TEXT[code];
    if (typeof message === 'string' && message.length) return message;
    return 'Could not complete the request';
  }

  function pad(n) { return (n < 10 ? '0' : '') + n; }

  // Whole seconds to 'mm:ss' (minutes may exceed 59); null or invalid -> '--:--'.
  function formatCountdown(secs) {
    if (typeof secs !== 'number' || !isFinite(secs)) return '--:--';
    var s = Math.max(0, Math.floor(secs));
    return pad(Math.floor(s / 60)) + ':' + pad(s % 60);
  }

  function settingName(setting) { return has(SETTING_NAMES, setting) ? SETTING_NAMES[setting] : String(setting); }

  // Display of a wire value: mode 'device'/'host' -> 'Device'/'Host', 'on'/'off' -> 'ON'/'OFF', else '...'.
  function valueLabel(setting, value) {
    if (setting === 'mode') return value === 'host' ? 'Host' : (value === 'device' ? 'Device' : '...');
    return value === 'on' ? 'ON' : (value === 'off' ? 'OFF' : '...');
  }

  // The wire value a toggle row sends: the opposite of the persisted one. Null
  // while the persisted value is unknown (the row must not send a guess).
  function nextValue(setting, persisted) {
    if (setting === 'mode') return persisted === 'host' ? 'device' : (persisted === 'device' ? 'host' : null);
    return persisted === 'on' ? 'off' : (persisted === 'off' ? 'on' : null);
  }

  // First row of Device settings: 'Unlock' when locked, else 'Lock now (28:41 left)'
  // ('Lock now' when the remaining time is unknown).
  function unlockLabel(unlocked, remainingSecs) {
    if (!unlocked) return 'Unlock';
    if (typeof remainingSecs !== 'number') return 'Lock now';
    return 'Lock now (' + formatCountdown(remainingSecs) + ' left)';
  }

  // The single note row of the USB mode screen, most relevant first (plan
  // section 8.1). `device` is domain.device; returns text or null.
  function usbNote(device) {
    if (device.busy !== null) return 'Switching, link may drop about 10 s';
    if (device.adbBlocked !== null) return 'ADB needs Device mode';
    if (device.live.adb === 'on' || device.live.uart === 'on') return 'ADB or UART is a root shell with no password';
    if (device.persisted.ssh === 'off') return 'SSH off: no remote login';
    return null;
  }

  // Live differs from persisted after an apply: a short line for the failed setting, else null.
  function driftNote(device) {
    if (device.busy !== null) return null;
    for (var i = 0; i < SETTINGS.length; i++) {
      var k = SETTINGS[i];
      if (device.live[k] !== null && device.persisted[k] !== null && device.live[k] !== device.persisted[k]) {
        return settingName(k) + ' is ' + valueLabel(k, device.live[k]) + ' now, saved as ' + valueLabel(k, device.persisted[k]);
      }
    }
    return null;
  }

  var api = {
    SETTINGS: SETTINGS, ERROR_TEXT: ERROR_TEXT, errorText: errorText, formatCountdown: formatCountdown,
    settingName: settingName, valueLabel: valueLabel, nextValue: nextValue, unlockLabel: unlockLabel,
    usbNote: usbNote, driftNote: driftNote,
  };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.deviceSettingsLogic = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
