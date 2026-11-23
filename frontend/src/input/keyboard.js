/*
 * Keyboard to PTY bytes. A paired BT keyboard surfaces as a normal evdev node
 * that Chromium turns into ordinary DOM keydown events, so this is a plain
 * keydown handler. toBytes is pure; bind() is the thin DOM half.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.keyboard).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;

  var KEY_SEQUENCES = {
    Enter: '\r',
    Backspace: '\x7f',
    Tab: '\t',
    Escape: '\x1b',
    ArrowUp: '\x1b[A',
    ArrowDown: '\x1b[B',
    ArrowRight: '\x1b[C',
    ArrowLeft: '\x1b[D',
  };

  // evt: {key, ctrlKey}. Returns the bytes to send, or null when the key has
  // no terminal meaning.
  function toBytes(evt) {
    if (evt.ctrlKey && evt.key.length === 1) {
      // Ctrl+A..Ctrl+Z -> control bytes 0x01..0x1a, the standard mapping.
      var code = evt.key.toUpperCase().charCodeAt(0);
      return code >= 65 && code <= 90 ? String.fromCharCode(code - 64) : null;
    }
    if (Object.prototype.hasOwnProperty.call(KEY_SEQUENCES, evt.key)) return KEY_SEQUENCES[evt.key];
    if (evt.key.length === 1) return evt.key;
    return null;
  }

  // el: the focusable #screen element. isMenuOpen(): true while the menu or the
  // first-boot overlay owns input; keystrokes are then swallowed entirely so a uinput-replayed Enter
  // (or a BT keyboard) cannot type into the PTY under the overlay.
  // onBytes(data): sends the bytes to the active session.
  function bind(el, isMenuOpen, onBytes) {
    // Capture phase: must see the keydown before xterm's own textarea listener
    // can stopPropagation() it.
    el.addEventListener('keydown', function (evt) {
      if (isMenuOpen()) {
        evt.preventDefault();
        if (evt.stopPropagation) evt.stopPropagation();
        if (evt.stopImmediatePropagation) evt.stopImmediatePropagation();
        return;
      }
      var data = toBytes(evt);
      if (data !== null) {
        evt.preventDefault();
        onBytes(data);
      }
    }, { capture: true });
  }

  var api = { KEY_SEQUENCES: KEY_SEQUENCES, toBytes: toBytes, bind: bind };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.keyboard = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
