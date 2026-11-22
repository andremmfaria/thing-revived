/*
 * Hardware button vocabulary. Real Linux KEY_* codes on /dev/input/event0
 * (docs/research/button-knob-clock-design.md): KEY_ESC=1, KEY_1..KEY_4=2..5,
 * KEY_ENTER=28. evdev values: 1 press, 0 release, 2 autorepeat.
 *
 * Knob push double delivery (documented here, handled by the interaction
 * chart's swallowEnter state): the controller reports a knob push as a
 * bridge `button` message (code 28) AND replays an Enter keystroke through
 * uinput that reaches the page as a DOM keydown. When text entry ends on one
 * of the two, the other must not re-trigger a select; the chart swallows
 * selects for enter_guard_ms after text entry ends.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.buttons).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;

  var KEY_ESC = 1;
  var KEY_1 = 2;
  var KEY_ENTER = 28;

  // KEY_1..KEY_4 select a session index directly (not next/prev). KEY_M and
  // KEY_ENTER outside the menu arrive through the normal keydown path.
  var SESSION_FOR_BUTTON = { 2: 0, 3: 1, 4: 2, 5: 3 };

  function sessionForButton(code) {
    return Object.prototype.hasOwnProperty.call(SESSION_FOR_BUTTON, code) ? SESSION_FOR_BUTTON[code] : null;
  }

  function phaseOf(value) {
    return value === 1 ? 'down' : value === 0 ? 'up' : 'repeat';
  }

  var api = {
    KEY_ESC: KEY_ESC,
    KEY_1: KEY_1,
    KEY_ENTER: KEY_ENTER,
    sessionForButton: sessionForButton,
    phaseOf: phaseOf,
  };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.buttons = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
