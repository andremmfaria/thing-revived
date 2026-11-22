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
  var Buttons = node ? require('../input/buttons.js') : TR.buttons;

  var api = {
    knob: function (delta) { return { type: 'knob', delta: delta }; },
    button: function (code, value) { return { type: 'button', code: code, value: value, phase: Buttons.phaseOf(value) }; },
    timerFired: function (name) { return { type: 'timerFired', name: name }; },
    // A controller message, with the domain state as it is AFTER the reducer ran.
    wire: function (msg, domain) { return { type: 'wire', msg: msg, domain: domain }; },
    domain: function (domain) { return { type: 'domain', domain: domain }; },
    rowTap: function (index) { return { type: 'rowTap', index: index }; },
    customTap: function (offset, index) { return { type: 'customTap', offset: offset, index: index }; },
    textInput: function (value) { return { type: 'textInput', value: value }; },
    textEnter: function () { return { type: 'textEnter' }; },
    textBlur: function () { return { type: 'textBlur' }; },
    closeClick: function () { return { type: 'closeClick' }; },
    sendFailed: function (msgType, target) { return { type: 'sendFailed', msgType: msgType, target: target }; },
  };

  if (node) module.exports = api;
  else TR.events = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
