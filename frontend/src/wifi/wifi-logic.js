/*
 * Pure helpers for the Wi-Fi menu screens (docs/wifi.md). No DOM, no
 * WebSocket, no globals from app.js, so the same file loads as a classic
 * <script> in the kiosk (window.TR.wifiLogic) and under node for the tests in
 * frontend/tests/. Plain ES6, same syntax limits as app.js.
 */
(function (root) {
  'use strict';

  var PASSWORD_MIN = 8;
  var PASSWORD_MAX = 63;

  // Knob character picker ring. The two control tokens sit at the start so
  // that, with wrap-around, they are one detent left of 'a' (OK, then DEL).
  var TOKEN_DONE = '[OK]';
  var TOKEN_DEL = '[DEL]';
  var PICKER_RING = [TOKEN_DONE, TOKEN_DEL]
    .concat('abcdefghijklmnopqrstuvwxyz'.split(''))
    .concat('ABCDEFGHIJKLMNOPQRSTUVWXYZ'.split(''))
    .concat('0123456789'.split(''))
    .concat(' !"#$%&\'()*+,-./:;<=>?@[\\]^_`{|}~'.split(''));
  var PICKER_START = 2; // 'a'

  // Mirrors controller/src/wifi.rs validate_password so a typo is caught
  // before a round trip; the controller re-validates and stays the authority.
  function validatePassword(pw, security) {
    if (security === 'open') {
      return pw.length === 0 ? null : 'Open network takes no password';
    }
    if (security === 'wep' || security === 'eap') {
      return 'This security type is not supported';
    }
    if (pw.length < PASSWORD_MIN || pw.length > PASSWORD_MAX) {
      return 'Password must be 8 to 63 characters';
    }
    for (var i = 0; i < pw.length; i++) {
      var c = pw.charCodeAt(i);
      if (c < 32 || c > 126) return 'Only printable ASCII characters are allowed';
    }
    return null;
  }

  function maskPassword(pw, show) {
    if (show) return pw;
    return new Array(pw.length + 1).join('\u2022');
  }

  // Wraps around the ring; only the sign of the knob delta matters, same
  // one-detent-per-message rule as list navigation in app.js.
  function pickerMove(idx, delta) {
    var dir = delta > 0 ? 1 : (delta < 0 ? -1 : 0);
    var n = PICKER_RING.length;
    return (idx + dir + n) % n;
  }

  // Applies the ring item at idx to the password. Returns the new password
  // and whether the picker should close.
  function pickerApply(pw, idx) {
    var item = PICKER_RING[idx];
    if (item === TOKEN_DONE) return { password: pw, done: true };
    if (item === TOKEN_DEL) return { password: pw.slice(0, -1), done: false };
    if (pw.length >= PASSWORD_MAX) return { password: pw, done: false };
    return { password: pw + item, done: false };
  }

  // The visible window of the ring around idx, for the picker strip.
  function pickerWindow(idx, radius) {
    var out = [];
    for (var d = -radius; d <= radius; d++) {
      var n = PICKER_RING.length;
      var i = (idx + d + n * 4) % n;
      out.push({ offset: d, index: i, item: PICKER_RING[i] });
    }
    return out;
  }

  function pickerLabel(item) {
    if (item === ' ') return 'space';
    return item;
  }

  // 0..4 lit bars from a 0..100 signal percentage.
  function signalBars(pct) {
    if (typeof pct !== 'number') return 0;
    if (pct >= 75) return 4;
    if (pct >= 50) return 3;
    if (pct >= 25) return 2;
    if (pct > 0) return 1;
    return 0;
  }

  function securityLabel(sec) {
    switch (sec) {
      case 'open': return 'Open';
      case 'wpa2': return 'WPA2';
      case 'wpa3': return 'WPA3';
      case 'wpa2/wpa3': return 'WPA2/3';
      case 'wep': return 'WEP';
      case 'eap': return '802.1X';
      default: return String(sec || '?');
    }
  }

  function stateLabel(status) {
    if (!status) return 'Checking...';
    switch (status.state) {
      case 'connected': return 'Connected';
      case 'obtaining_ip': return 'Getting IP address...';
      case 'connecting': return 'Connecting...';
      default: return 'Not connected';
    }
  }

  // Rows for the Wi-Fi home screen's read-only status block.
  function statusRows(status) {
    var rows = [{ label: 'Status', value: stateLabel(status) }];
    if (!status) return rows;
    if (status.state !== 'disconnected') {
      if (status.ssid) rows.push({ label: 'Network', value: status.ssid });
      if (status.ip) rows.push({ label: 'IP', value: status.ip });
      if (typeof status.signal_pct === 'number') {
        rows.push({ label: 'Signal', value: status.signal_pct + '%', bars: signalBars(status.signal_pct) });
      }
    }
    rows.push({ label: 'Interface', value: status.interface || '?' });
    rows.push({ label: 'Ethernet', value: status.ethernet_up ? 'also up' : 'down' });
    return rows;
  }

  // Index of the saved network with this ssid, or -1.
  function findSaved(saved, ssid) {
    if (!saved) return -1;
    for (var i = 0; i < saved.length; i++) {
      if (saved[i].ssid === ssid) return i;
    }
    return -1;
  }

  var api = {
    PASSWORD_MIN: PASSWORD_MIN,
    PASSWORD_MAX: PASSWORD_MAX,
    TOKEN_DONE: TOKEN_DONE,
    TOKEN_DEL: TOKEN_DEL,
    PICKER_RING: PICKER_RING,
    PICKER_START: PICKER_START,
    validatePassword: validatePassword,
    maskPassword: maskPassword,
    pickerMove: pickerMove,
    pickerApply: pickerApply,
    pickerWindow: pickerWindow,
    pickerLabel: pickerLabel,
    signalBars: signalBars,
    securityLabel: securityLabel,
    stateLabel: stateLabel,
    statusRows: statusRows,
    findSaved: findSaved,
  };

  if (typeof module !== 'undefined' && module.exports) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.wifiLogic = api;
  }
})(typeof window !== 'undefined' ? window : this);
