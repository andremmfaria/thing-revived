/*
 * The knob/button/menu interaction as a statechart (table + machine.js).
 * Everything here is pure: transition(state, event) returns {state, effects},
 * effects are data (see effects.js), timers are effects and their firing is
 * the ordinary event `timerFired`.
 *
 * States (state.mode):
 *   terminal               menu closed; knob scrolls, buttons 1..4 switch session
 *   open.list              menu open (the default, also covers plain screens)
 *   open.adjusting         a range row is live-edited by the knob
 *   open.editingText       the real <input> has focus; knob ignored
 *   open.swallowEnter      text entry just ended; selects are ignored for
 *                          enter_guard_ms (the knob-push double delivery guard,
 *                          input/buttons.js) while knob and back still work
 *   open.picker            the knob character picker screen
 *   open.pairing.confirm   Bluetooth accept/deny screen
 *   open.pairing.passkey   Bluetooth passkey display screen
 * Orthogonal region state.esc: 'up' | 'armed' (ESC is down inside the menu and
 * the esc-hold timer is running). The pairing states are entered from
 * controller events even while the menu is closed. Screens declare their own
 * mode (default open.list), so no screen ids appear in this file.
 *
 * Screens are data (menu/screens/*.js): rows(ctx) returns rows whose
 * `activate` is a list of descriptors. A descriptor is either a navigation or
 * state command handled here ({push, pop, popToRoot, popTo, ui, highlight,
 * adjust, textEdit, text, toast, open}) or an effect passed through to the
 * interpreter ({send, dispatch, term, switchSession, scroll, startTimer,
 * cancelTimer}).
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.menuChart).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var Types = node ? require('../utils/types.js') : TR.types;
  var Machine = node ? require('./machine.js') : TR.machine;
  var Buttons = node ? require('../input/buttons.js') : TR.buttons;

  var AFTER_SELECT = { root: [{ popToRoot: true }], pop: [{ pop: true }], none: [] };

  // What selecting a row of each kind does, as descriptors.
  var ROW_KINDS = {
    label: function () { return []; },
    toggle: function (row) { return row.activate || []; },
    range: function () { return [{ adjust: 'toggle' }]; },
    nav: function (row) { return [{ push: row.target, enter: row.enter || [] }]; },
    'nav-dynamic': function (row) { return [{ push: row.target, enter: row.enter || [] }]; },
    field: function (row) { return row.activate || []; },
    action: function (row) { return (row.activate || []).concat(AFTER_SELECT[row.afterSelect || 'pop']); },
  };

  var PASSTHROUGH = { send: 1, dispatch: 1, term: 1, switchSession: 1, scroll: 1, startTimer: 1, cancelTimer: 1, textBegin: 1, textEnd: 1, textReset: 1 };

  // opts: { screens: {defs, reactions}, cfg: config values }
  function create(opts) {
    var defs = opts.screens.defs;
    var reactions = opts.screens.reactions;
    var cfg = opts.cfg;
    var escHoldMs = cfg.input.esc_hold_ms;
    var guardMs = cfg.input.enter_guard_ms;
    var toastMs = cfg.ui.toast_ms;
    var scrollLines = cfg.terminal.knob_scroll_lines;

    // ---- working copy + helpers ---------------------------------------------
    function begin(state) {
      var s = Object.assign({}, state);
      s.stack = state.stack.slice();
      s.highlight = Object.assign({}, state.highlight);
      s.ui = {};
      Object.keys(state.ui).forEach(function (k) { s.ui[k] = Object.assign({}, state.ui[k]); });
      return { s: s, fx: [] };
    }
    function out(r) { return { state: r.s, effects: r.fx }; }
    function top(s) { return s.stack[s.stack.length - 1]; }
    function screenOf(s) { return defs[top(s)]; }
    function ctxOf(s) {
      return { domain: s.domain, ui: s.ui, stack: s.stack, top: top(s), highlight: s.highlight, cfg: cfg };
    }
    function rowsOf(s) {
      var sc = screenOf(s);
      return sc && sc.rows ? sc.rows(ctxOf(s)) : [];
    }
    function deriveMode(s) {
      var sc = screenOf(s);
      return sc && sc.mode ? sc.mode : Types.Mode.LIST;
    }
    var STICKY = {};
    STICKY[Types.Mode.TERMINAL] = 1;
    STICKY[Types.Mode.EDITING_TEXT] = 1;
    STICKY[Types.Mode.SWALLOW_ENTER] = 1;
    // After a navigation step: adjusting never survives; text states do.
    function settle(s) {
      if (s.mode === Types.Mode.ADJUSTING) s.mode = Types.Mode.LIST;
      if (!STICKY[s.mode]) s.mode = deriveMode(s);
    }
    function clampHighlight(s, n) {
      var id = top(s);
      if (typeof s.highlight[id] !== 'number') s.highlight[id] = 0;
      if (s.highlight[id] >= n) s.highlight[id] = Math.max(0, n - 1);
      return s.highlight[id];
    }
    function exitText(s) {
      if (s.mode === Types.Mode.EDITING_TEXT) s.mode = deriveMode(s);
    }

    // ---- descriptors ----------------------------------------------------------
    function popOne(r) {
      if (r.s.stack.length <= 1) return;
      var leaving = r.s.stack.pop();
      settle(r.s);
      var def = defs[leaving];
      if (def && def.onLeave) run(r, def.onLeave(ctxOf(r.s)));
    }

    var COMMANDS = {
      push: function (r, d) {
        r.s.stack.push(d.push);
        settle(r.s);
        run(r, d.enter || []);
        var def = defs[d.push];
        if (def && def.onEnter) run(r, def.onEnter(ctxOf(r.s)));
      },
      pop: function (r) { popOne(r); },
      popToRoot: function (r) { while (r.s.stack.length > 1) popOne(r); },
      popTo: function (r, d) { while (r.s.stack.length > 1 && top(r.s) !== d.popTo) popOne(r); },
      ui: function (r, d) {
        Object.keys(d.ui).forEach(function (k) { r.s.ui[k] = Object.assign({}, r.s.ui[k], d.ui[k]); });
      },
      highlight: function (r, d) { Object.assign(r.s.highlight, d.highlight); },
      adjust: function (r) {
        r.s.mode = r.s.mode === Types.Mode.ADJUSTING ? deriveMode(r.s) : Types.Mode.ADJUSTING;
      },
      textEdit: function (r) {
        var tf = screenOf(r.s) && screenOf(r.s).textField;
        if (!tf) return;
        var ctx = ctxOf(r.s);
        r.s.mode = Types.Mode.EDITING_TEXT;
        r.fx.push({ textBegin: { masked: tf.masked(ctx), value: tf.value(ctx) } });
      },
      text: function (r, d) {
        if (d.text === 'reset') r.fx.push({ textReset: true });
        exitText(r.s);
      },
      toast: function (r, d) {
        r.s.toast = d.toast;
        r.fx.push({ cancelTimer: 'toast' }, { startTimer: { name: 'toast', ms: toastMs } });
      },
      open: function (r) {
        if (r.s.mode === Types.Mode.TERMINAL) r.s.mode = deriveMode(r.s);
      },
    };

    function run(r, list) {
      list.forEach(function (d) {
        var key = Object.keys(d).filter(function (k) { return k !== 'enter'; })[0];
        if (Object.prototype.hasOwnProperty.call(COMMANDS, key)) COMMANDS[key](r, d);
        else if (PASSTHROUGH[key]) r.fx.push(d);
        else throw new Error('menu-chart: unknown descriptor ' + key);
      });
    }

    function closeMenu(r) {
      COMMANDS.popToRoot(r);
      r.s.mode = Types.Mode.TERMINAL;
      r.s.esc = 'up';
      r.fx.push({ cancelTimer: 'esc-hold' }, { cancelTimer: 'enter-guard' });
    }

    function stopEdit(r) {
      if (guardMs > 0) {
        r.s.mode = Types.Mode.SWALLOW_ENTER;
        r.fx.push({ startTimer: { name: 'enter-guard', ms: guardMs } });
      } else {
        r.s.mode = deriveMode(r.s);
      }
      r.fx.push({ textEnd: true });
    }

    // Feeds an internal event (select/back/close) through the machine and
    // folds its result into the running transition.
    function chain(r, m, ev) {
      var inner = m.transition(r.s, ev);
      return { state: inner.state, effects: r.fx.concat(inner.effects) };
    }

    // Runs `ev` as if the base (non-swallow) mode were current, then goes back
    // to swallowEnter unless the menu closed.
    function asBase(state, ev, m) {
      var inner = m.transition(Object.assign({}, state, { mode: deriveMode(state) }), ev);
      var s = inner.state;
      if (s.mode.indexOf('open') === 0 && s.mode !== Types.Mode.EDITING_TEXT) s = Object.assign({}, s, { mode: Types.Mode.SWALLOW_ENTER });
      return { state: s, effects: inner.effects };
    }

    // ---- handlers -------------------------------------------------------------
    function listKnob(state, ev) {
      var sc = screenOf(state);
      var r = begin(state);
      if (sc && sc.onKnob) {
        run(r, sc.onKnob(ctxOf(r.s), ev.delta));
        return out(r);
      }
      var rows = rowsOf(r.s);
      if (rows.length === 0) return out(r);
      var idx = clampHighlight(r.s, rows.length);
      // One row per message regardless of magnitude: fast spins would skip rows.
      var dir = ev.delta > 0 ? 1 : (ev.delta < 0 ? -1 : 0);
      var next = Math.min(Math.max(idx + dir, 0), rows.length - 1);
      r.s.highlight[top(r.s)] = next;
      return out(r);
    }

    function adjustKnob(state, ev) {
      var r = begin(state);
      var rows = rowsOf(r.s);
      var row = rows[clampHighlight(r.s, rows.length)];
      if (!row || row.kind !== 'range') return null; // fall back to list movement
      var v = (row.value === null ? 0 : row.value) + ev.delta * row.step;
      if (v < row.min) v = row.min;
      if (v > row.max) v = row.max;
      run(r, row.change(v));
      return out(r);
    }

    function select(state) {
      var r = begin(state);
      var sc = screenOf(r.s);
      if (sc && sc.onSelect) {
        run(r, sc.onSelect(ctxOf(r.s)));
        return out(r);
      }
      var rows = rowsOf(r.s);
      var row = rows[clampHighlight(r.s, rows.length)];
      if (row && ROW_KINDS[row.kind]) run(r, ROW_KINDS[row.kind](row));
      return out(r);
    }

    function openButton(state, ev, m) {
      var r = begin(state);
      if (ev.code === Buttons.KEY_ESC) {
        if (ev.phase === 'down') {
          // A new press supersedes any leaked timer.
          r.s.esc = 'armed';
          r.fx.push({ cancelTimer: 'esc-hold' }, { startTimer: { name: 'esc-hold', ms: escHoldMs } });
          return out(r);
        }
        if (ev.phase === 'up' && state.esc === 'armed') {
          // Released before the timer fired: a tap, which confirms.
          r.s.esc = 'up';
          r.fx.push({ cancelTimer: 'esc-hold' });
          return chain(r, m, { type: 'select' });
        }
        return out(r); // release after the hold already fired, or autorepeat: no-op
      }
      if (ev.phase !== 'down') return out(r);
      if (ev.code === Buttons.KEY_ENTER) return chain(r, m, { type: 'select' });
      if (ev.code === Buttons.KEY_1) return chain(r, m, { type: 'back' });
      return out(r); // KEY_2..KEY_4 are ignored while the menu is open
    }

    function states() {
      return {
        '': {
          domain: function (state, ev) { var r = begin(state); r.s.domain = ev.domain; return out(r); },
          timerFired: function (state, ev) {
            if (ev.name !== 'toast') return null;
            var r = begin(state);
            r.s.toast = null;
            return out(r);
          },
          sendFailed: function (state, ev) {
            var r = begin(state);
            var text = ev.target === 'agent' ? 'Terminal agent not connected' : 'Controller not connected (' + ev.msgType + ' not sent)';
            run(r, [{ toast: text }]);
            return out(r);
          },
          wire: function (state, ev) {
            var r = begin(state);
            r.s.domain = ev.domain || state.domain;
            var fn = Object.prototype.hasOwnProperty.call(reactions, ev.msg.type) ? reactions[ev.msg.type] : null;
            if (fn) run(r, fn(ctxOf(r.s), ev.msg));
            return out(r);
          },
        },
        terminal: {
          button: function (state, ev) {
            var r = begin(state);
            if (ev.phase !== 'down') return out(r);
            if (ev.code === Buttons.KEY_ESC) {
              run(r, [{ open: true }]);
            } else if (Buttons.sessionForButton(ev.code) !== null) {
              r.fx.push({ switchSession: Buttons.sessionForButton(ev.code) });
            }
            return out(r);
          },
          knob: function (state, ev) { return { state: state, effects: [{ scroll: ev.delta * scrollLines }] }; },
        },
        open: {
          button: openButton,
          knob: listKnob,
          select: select,
          back: function (state) { var r = begin(state); popOne(r); return out(r); },
          close: function (state) { var r = begin(state); closeMenu(r); return out(r); },
          closeClick: function (state, ev, m) { return m.transition(state, { type: 'close' }); },
          rowTap: function (state, ev, m) {
            var r = begin(state);
            clampHighlight(r.s, rowsOf(r.s).length);
            r.s.highlight[top(r.s)] = ev.index;
            return chain(r, m, { type: 'select' });
          },
          customTap: function (state, ev) {
            var sc = screenOf(state);
            var r = begin(state);
            if (sc && sc.onCustomTap) run(r, sc.onCustomTap(ctxOf(r.s), ev));
            return out(r);
          },
          timerFired: function (state, ev, m) {
            if (ev.name !== 'esc-hold') return null;
            if (state.esc !== 'armed') return { state: state, effects: [] };
            var r = begin(state);
            r.s.esc = 'up';
            var leaves = state.mode === Types.Mode.ADJUSTING || state.stack.length > 1;
            return chain(r, m, { type: leaves ? 'back' : 'close' });
          },
        },
        [Types.Mode.ADJUSTING]: {
          knob: adjustKnob,
          select: function (state) { var r = begin(state); run(r, [{ adjust: 'toggle' }]); return out(r); },
          back: function (state) { var r = begin(state); r.s.mode = deriveMode(r.s); return out(r); },
        },
        [Types.Mode.EDITING_TEXT]: {
          knob: function (state) { return { state: state, effects: [] }; },
          select: function (state) { var r = begin(state); stopEdit(r); return out(r); },
          back: function (state) { var r = begin(state); stopEdit(r); return out(r); },
          textEnter: function (state) { var r = begin(state); stopEdit(r); return out(r); },
          textBlur: function (state) { var r = begin(state); stopEdit(r); return out(r); },
          textInput: function (state, ev) {
            var sc = screenOf(state);
            var r = begin(state);
            if (sc && sc.textField) run(r, sc.textField.onInput(ctxOf(r.s), ev.value));
            return out(r);
          },
        },
        [Types.Mode.SWALLOW_ENTER]: {
          select: function (state) { return { state: state, effects: [] }; },
          knob: function (state, ev, m) { return asBase(state, ev, m); },
          back: function (state, ev, m) { return asBase(state, ev, m); },
          timerFired: function (state, ev) {
            if (ev.name !== 'enter-guard') return null;
            var r = begin(state);
            r.s.mode = deriveMode(r.s);
            return out(r);
          },
        },
      };
    }

    var machine = Machine.create({ states: states() });

    return {
      machine: machine,
      transition: machine.transition,
      initial: function (domain) {
        return {
          mode: Types.Mode.TERMINAL,
          esc: 'up',
          stack: ['root'],
          highlight: {},
          toast: null,
          ui: opts.screens.initialUi(),
          domain: domain,
        };
      },
      ctxOf: ctxOf,
      rowsOf: rowsOf,
      screenOf: screenOf,
      clampHighlight: function (state) {
        var s = Object.assign({}, state, { highlight: Object.assign({}, state.highlight) });
        return clampHighlight(s, rowsOf(s).length);
      },
    };
  }

  var api = { create: create, ROW_KINDS: ROW_KINDS };

  if (node) module.exports = api;
  else TR.menuChart = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
