/*
 * Focus policy and the text field adapter (DOM).
 *
 * A touch inside the terminal shell refocuses the terminal so a physical BT
 * keyboard keeps a keydown target, except taps on the text field itself (they
 * must keep focus there so the on-screen keyboard stays up).
 *
 * The text field adapter is what the effects interpreter drives (begin / end /
 * reset), and bindTextField forwards the input element's own events to the
 * interaction machine.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.focus).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var Events = node ? require('../interaction/events.js') : TR.events;

  // deps: { doc, termFocus() }
  function create(deps) {
    var doc = deps.doc;

    function focusScreen() {
      deps.termFocus();
      doc.getElementById('screen').focus();
    }

    function bindRefocus(shellEl, textFieldId) {
      var refocus = function (evt) {
        if (evt.target && evt.target.id === textFieldId) return;
        focusScreen();
      };
      shellEl.addEventListener('click', refocus);
      shellEl.addEventListener('touchstart', refocus, { passive: true });
    }

    function textAdapter(inputId) {
      var input = function () { return doc.getElementById(inputId); };
      return {
        begin: function (t) {
          var el = input();
          if (!el) return;
          el.type = t.masked ? 'password' : 'text';
          el.value = t.value;
          el.focus();
        },
        end: function () {
          var el = input();
          if (el) el.blur();
          focusScreen();
        },
        reset: function () {
          var el = input();
          if (el) { el.value = ''; el.blur(); }
        },
      };
    }

    // emit(event): the interaction event dispatcher.
    function bindTextField(inputId, emit) {
      var el = doc.getElementById(inputId);
      if (!el) return;
      el.addEventListener('input', function () { emit(Events.textInput(el.value)); });
      el.addEventListener('keydown', function (evt) {
        if (evt.key === 'Enter') {
          evt.preventDefault();
          emit(Events.textEnter());
        }
      });
      // Dismissing the on-screen keyboard blurs the field; treat as done.
      el.addEventListener('blur', function () { emit(Events.textBlur()); });
    }

    return { focusScreen: focusScreen, bindRefocus: bindRefocus, textAdapter: textAdapter, bindTextField: bindTextField };
  }

  var api = { create: create };

  if (node) module.exports = api;
  else TR.focus = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
