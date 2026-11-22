/*
 * The single shared xterm.js instance (all session tabs reuse it: a tab
 * switch resets it and replays that session's buffer). disableStdin is true
 * because keyboard input goes over the agent WebSocket from input/keyboard.js,
 * not through xterm's onData; xterm is purely an output renderer here.
 *
 * Constructors and the resize callback are injected so the sizing logic can be
 * driven with fakes in node.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.terminalView).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});

  // opts: { Terminal, FitAddon, ui (config.ui), terminal (config.terminal), onResize(cols, rows) }
  function create(opts) {
    var ui = opts.ui;
    var term = new opts.Terminal({
      cursorBlink: opts.terminal.cursor_blink,
      disableStdin: true,
      convertEol: true,
      fontFamily: ui.font_family,
      fontSize: ui.font_size,
      lineHeight: ui.line_height,
      scrollback: opts.terminal.scrollback_lines,
      theme: { background: ui.theme_background, foreground: ui.theme_foreground, cursor: ui.theme_cursor },
    });
    var fit = new opts.FitAddon();
    term.loadAddon(fit);

    // Re-measures the container and tells the active session's PTY the real
    // size (the callback is a no-op while the agent is down).
    function fitAndResize() {
      fit.fit();
      opts.onResize(term.cols, term.rows);
    }

    return {
      open: function (el) { term.open(el); fit.fit(); },
      fit: function () { fit.fit(); },
      fitAndResize: fitAndResize,
      write: function (data) { term.write(data); },
      replay: function (chunks) {
        term.reset();
        for (var i = 0; i < chunks.length; i++) term.write(chunks[i]);
      },
      // The knob scrolls xterm locally; the agent never sees it.
      scrollLines: function (n) { term.scrollLines(n); },
      setFontSize: function (v) { term.options.fontSize = v; fitAndResize(); },
      setFontFamily: function (css) { term.options.fontFamily = css; fitAndResize(); },
      focus: function () { term.focus(); },
      raw: term,
    };
  }

  var api = { create: create };

  if (node) module.exports = api;
  else TR.terminalView = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
