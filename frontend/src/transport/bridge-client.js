/*
 * Typed layer over the controller-agent channel (hardware buttons, knob and
 * settings commands). Wire shapes are unchanged: `commands` builds exactly
 * the frames the controller expects, `decode` maps frames to app events:
 *   knob   -> {type:'knob', delta}
 *   button -> {type:'button', code, value}
 *   every other known event -> {type:'wire', msg}
 * Unknown types are logged at debug level, never silently swallowed.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.bridgeClient).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var Logging = node ? require('../utils/logging.js') : TR.logging;

  // Server -> client events that carry settings state.
  var WIRE_TYPES = [
    'bt_power', 'bt_error', 'bt_scan_started', 'bt_scan_stopped', 'bt_device_found', 'bt_paired_list',
    'bt_pairing_confirm_request', 'bt_pairing_passkey_display', 'bt_pair_result',
    'brightness', 'timezone', 'timezone_list', 'settings_error',
    'wifi_available', 'wifi_status', 'wifi_scan_started', 'wifi_scan_results', 'wifi_connecting',
    'wifi_connect_result', 'wifi_saved_list', 'wifi_error',
  ];

  // Pure frame builders (client -> controller). One per ClientCommand.
  var commands = {
    btPowerGet: function () { return { type: 'bt_power_get' }; },
    btSetPower: function (powered) { return { type: 'bt_set_power', powered: powered }; },
    btScanStart: function () { return { type: 'bt_scan_start' }; },
    btScanStop: function () { return { type: 'bt_scan_stop' }; },
    btPairedListGet: function () { return { type: 'bt_paired_list_get' }; },
    btPair: function (address) { return { type: 'bt_pair', address: address }; },
    btPairingConfirm: function (accept) { return { type: 'bt_pairing_confirm', accept: accept }; },
    btConnect: function (address) { return { type: 'bt_connect', address: address }; },
    btDisconnect: function (address) { return { type: 'bt_disconnect', address: address }; },
    btForget: function (address) { return { type: 'bt_forget', address: address }; },
    brightnessGet: function () { return { type: 'brightness_get' }; },
    brightnessSet: function (value) { return { type: 'brightness_set', value: value }; },
    timezoneGet: function () { return { type: 'timezone_get' }; },
    timezoneListGet: function () { return { type: 'timezone_list_get' }; },
    timezoneSet: function (value) { return { type: 'timezone_set', value: value }; },
    wifiGet: function () { return { type: 'wifi_get' }; },
    wifiSavedListGet: function () { return { type: 'wifi_saved_list_get' }; },
    wifiScan: function () { return { type: 'wifi_scan' }; },
    wifiConnect: function (ssid, password) { return { type: 'wifi_connect', ssid: ssid, password: password }; },
    wifiConnectSaved: function (ssid) { return { type: 'wifi_connect_saved', ssid: ssid }; },
    wifiDisconnect: function () { return { type: 'wifi_disconnect' }; },
    wifiForget: function (ssid) { return { type: 'wifi_forget', ssid: ssid }; },
  };

  // Idempotent getters: safe to queue while disconnected and replay later.
  var QUEUEABLE = {
    bt_power_get: 1, brightness_get: 1, timezone_get: 1, wifi_get: 1,
    wifi_saved_list_get: 1, bt_paired_list_get: 1, timezone_list_get: 1,
  };

  function decode(msg) {
    if (msg.type === 'knob') return { type: 'knob', delta: msg.delta };
    if (msg.type === 'button') return { type: 'button', code: msg.code, value: msg.value };
    if (WIRE_TYPES.indexOf(msg.type) !== -1) return { type: 'wire', msg: msg };
    return null;
  }

  function create(channel, emit, logger) {
    var log = (logger || Logging).child('bridge');
    channel.onMessage(function (msg) {
      var ev = decode(msg);
      if (!ev) {
        log.debug('ignoring unknown message type', msg.type);
        return;
      }
      emit(ev);
    });
    return {
      // msg is a frame built with `commands`.
      send: function (msg) {
        return channel.send(msg, { queue: Object.prototype.hasOwnProperty.call(QUEUEABLE, msg.type) });
      },
      // Settings pulled on every (re)connect so the menu has values to show.
      requestInitialState: function () {
        [commands.btPowerGet(), commands.brightnessGet(), commands.timezoneGet(), commands.wifiGet()].forEach(function (m) {
          channel.send(m, { queue: true });
        });
      },
    };
  }

  var api = { create: create, decode: decode, commands: commands, WIRE_TYPES: WIRE_TYPES, QUEUEABLE: QUEUEABLE };

  if (node) module.exports = api;
  else TR.bridgeClient = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
