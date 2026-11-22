/*
 * Effects interpreter: executes the effect descriptors the interaction chart
 * emits. Every collaborator is injected, so tests pass fakes and nothing here
 * touches the DOM, sockets or real timers directly.
 *
 *   {send: msg}                          controller command via bridge.send
 *   {dispatch: action}                   domain store action (local/* updates)
 *   {startTimer: {name, ms}}             one pending timer per name; firing
 *   {cancelTimer: name}                  re-enters as event timerFired(name)
 *   {scroll: lines}                      xterm scrollLines
 *   {switchSession: index}               sessions.switchTo
 *   {term: {fontSize | fontFamily}}      terminal view option, then refit
 *   {textBegin: {masked, value}}         focus the text field with this value
 *   {textEnd: true}                      blur the field, refocus the terminal
 *   {textReset: true}                    clear and blur the field
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.effects).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var Logging = node ? require('../utils/logging.js') : TR.logging;

  // deps: { bridge:{send}, store:{dispatch}, named:{start,cancel}, term:{scrollLines,setFontSize,setFontFamily},
  //         sessions:{switchTo}, text:{begin,end,reset}, logger }
  function create(deps) {
    var log = (deps.logger || Logging).child('effects');

    var handlers = {
      send: function (msg) { deps.bridge.send(msg); },
      dispatch: function (action) { deps.store.dispatch(action); },
      startTimer: function (t) { deps.named.start(t.name, t.ms); },
      cancelTimer: function (name) { deps.named.cancel(name); },
      scroll: function (lines) { deps.term.scrollLines(lines); },
      switchSession: function (index) { deps.sessions.switchTo(index); },
      term: function (opt) {
        if (opt.fontSize !== undefined) deps.term.setFontSize(opt.fontSize);
        if (opt.fontFamily !== undefined) deps.term.setFontFamily(opt.fontFamily);
      },
      textBegin: function (t) { deps.text.begin(t); },
      textEnd: function () { deps.text.end(); },
      textReset: function () { deps.text.reset(); },
    };

    function runOne(effect) {
      var key = Object.keys(effect)[0];
      if (!Object.prototype.hasOwnProperty.call(handlers, key)) {
        log.error('unknown effect', key);
        return;
      }
      handlers[key](effect[key]);
    }

    return {
      run: function (effects) {
        effects.forEach(function (effect) {
          try {
            runOne(effect);
          } catch (err) {
            // One failing effect must not strand the rest (a timer cancel after a failed send).
            log.error('effect failed:', Object.keys(effect)[0], err);
          }
        });
      },
    };
  }

  var api = { create: create };

  if (node) module.exports = api;
  else TR.effects = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
