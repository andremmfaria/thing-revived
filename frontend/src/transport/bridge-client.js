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
  var Types = node ? require('../utils/types.js') : TR.types;
  var Logging = node ? require('../utils/logging.js') : TR.logging;

  // Server -> client events that carry settings state (everything but knob/button).
  var WIRE_TYPES = Types.values(Types.BridgeEvent).filter(function (t) { return t !== Types.BridgeEvent.knob && t !== Types.BridgeEvent.button; });

  // Pure frame builders (client -> controller), catalogued in utils/types.js.
  var commands = Types.bridgeCommands;

  // Idempotent getters: safe to queue while disconnected and replay later.
  var QUEUEABLE = {
    bt_power_get: 1, brightness_get: 1, timezone_get: 1, wifi_get: 1,
    wifi_saved_list_get: 1, bt_paired_list_get: 1, timezone_list_get: 1,
  };

  function decode(msg) {
    if (msg.type === Types.BridgeEvent.knob) return { type: Types.EventKind.knob, delta: msg.delta };
    if (msg.type === Types.BridgeEvent.button) return { type: Types.EventKind.button, code: msg.code, value: msg.value };
    if (WIRE_TYPES.indexOf(msg.type) !== -1) return { type: Types.EventKind.wire, msg: msg };
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
