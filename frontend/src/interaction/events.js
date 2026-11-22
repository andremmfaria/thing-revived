/*
 * Event constructors for the interaction machine. Everything the machine
 * reacts to is one of these plain objects, so tests build them without a
 * browser and timers are just another event (`timerFired`).
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.events).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var Types = node ? require('../utils/types.js') : TR.types;
  var Buttons = node ? require('../input/buttons.js') : TR.buttons;

  var api = {
    knob: function (delta) { return { type: Types.EventKind.knob, delta: delta }; },
    button: function (code, value) { return { type: Types.EventKind.button, code: code, value: value, phase: Buttons.phaseOf(value) }; },
    timerFired: function (name) { return { type: Types.EventKind.timerFired, name: name }; },
    // A controller message, with the domain state as it is AFTER the reducer ran.
    wire: function (msg, domain) { return { type: Types.EventKind.wire, msg: msg, domain: domain }; },
    domain: function (domain) { return { type: Types.EventKind.domain, domain: domain }; },
    rowTap: function (index) { return { type: Types.EventKind.rowTap, index: index }; },
    customTap: function (offset, index) { return { type: Types.EventKind.customTap, offset: offset, index: index }; },
    textInput: function (value) { return { type: Types.EventKind.textInput, value: value }; },
    textEnter: function () { return { type: Types.EventKind.textEnter }; },
    textBlur: function () { return { type: Types.EventKind.textBlur }; },
    closeClick: function () { return { type: Types.EventKind.closeClick }; },
    sendFailed: function (msgType, target) { return { type: Types.EventKind.sendFailed, msgType: msgType, target: target }; },
  };

  if (node) module.exports = api;
  else TR.events = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
