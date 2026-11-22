/*
 * Typed accessor over the frontend configuration.
 *
 * Source of truth is config/frontend.toml. The kiosk cannot read TOML (no
 * build step, file:// page, Chromium 69), so the image build converts that
 * file into frontend/config.generated.js, a classic script that defines
 * `window.THING_CONFIG = {...}`. This module deep-merges that object over the
 * compiled-in DEFAULTS below, so the app works unchanged when the generated
 * file is empty or missing (for example when index.html is opened straight
 * from a checkout). A node test asserts that config/frontend.toml equals
 * DEFAULTS, so the two cannot drift.
 *
 * Bad values never throw at load: the offending key falls back to its default
 * and a console warning is printed (this module cannot use utils/logging.js,
 * which itself reads the log level from here).
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.config,
 * node: module.exports).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;

  // type: 'string' | 'int' | 'number' | 'bool' | 'enum' | 'strings'
  // Every key of config/frontend.toml appears here exactly once.
  var SCHEMA = {
    bridge: {
      url: { type: 'string', def: 'ws://127.0.0.1:8765', pattern: /^wss?:\/\/[^\s]+$/, doc: 'controller agent WebSocket (hardware buttons, knob, settings)' },
    },
    agent: {
      url: { type: 'string', def: 'ws://127.0.0.1:8766', pattern: /^wss?:\/\/[^\s]+$/, doc: 'terminal agent WebSocket (PTY I/O, clock)' },
    },
    reconnect: {
      initial_ms: { type: 'int', def: 2000, min: 50, max: 600000, doc: 'delay before the first reconnect attempt' },
      max_ms: { type: 'int', def: 30000, min: 50, max: 3600000, doc: 'ceiling for the reconnect delay' },
      factor: { type: 'number', def: 1.0, min: 1.0, max: 10.0, doc: 'delay multiplier per failed attempt (1.0 = fixed delay)' },
    },
    transport: {
      queue_max: { type: 'int', def: 16, min: 0, max: 256, doc: 'bounded queue size for idempotent getter messages sent while disconnected' },
    },
    input: {
      esc_hold_ms: { type: 'int', def: 450, min: 50, max: 5000, doc: 'ESC held at least this long counts as a hold (back) instead of a tap (select)' },
      enter_guard_ms: { type: 'int', def: 400, min: 0, max: 5000, doc: 'window after text entry ends in which the echoed knob-push Enter is swallowed' },
    },
    ui: {
      toast_ms: { type: 'int', def: 4000, min: 500, max: 60000, doc: 'how long a toast stays on screen' },
      font_size: { type: 'int', def: 13, min: 6, max: 64, doc: 'initial terminal font size in px' },
      font_size_min: { type: 'int', def: 8, min: 6, max: 64, doc: 'smallest font size the menu allows' },
      font_size_max: { type: 'int', def: 28, min: 6, max: 64, doc: 'largest font size the menu allows' },
      font_size_step: { type: 'int', def: 1, min: 1, max: 16, doc: 'font size change per knob detent' },
      font_family: { type: 'string', def: "'JetBrains Mono', Consolas, 'Courier New', monospace", doc: 'initial terminal font stack' },
      font_families: { type: 'strings', def: ['JetBrains Mono', 'Consolas', 'Courier New', 'monospace'], doc: 'choices in the Terminal > Font family menu' },
      line_height: { type: 'number', def: 1.3, min: 1.0, max: 3.0, doc: 'terminal line height multiplier' },
      theme_background: { type: 'string', def: '#05070a', pattern: /^#[0-9a-fA-F]{6}$/, doc: 'terminal background color' },
      theme_foreground: { type: 'string', def: '#c9dcea', pattern: /^#[0-9a-fA-F]{6}$/, doc: 'terminal text color' },
      theme_cursor: { type: 'string', def: '#35e38a', pattern: /^#[0-9a-fA-F]{6}$/, doc: 'terminal cursor color' },
    },
    brightness: {
      steps: { type: 'int', def: 20, min: 1, max: 100, doc: 'knob detents across the full brightness range (step = max / steps)' },
      fallback_max: { type: 'int', def: 160, min: 1, max: 100000, doc: 'assumed maximum until the controller reports one' },
    },
    clock: {
      format: { type: 'enum', def: 'HH:mm:ss', values: ['HH:mm:ss', 'HH:mm'], doc: 'status bar clock format (24 hour)' },
      interval_ms: { type: 'int', def: 1000, min: 100, max: 60000, doc: 'how often the status bar clock redraws' },
    },
    terminal: {
      scrollback_lines: { type: 'int', def: 1000, min: 0, max: 100000, doc: 'xterm scrollback length in lines' },
      replay_buffer_bytes: { type: 'int', def: 262144, min: 4096, max: 16777216, doc: 'byte cap of the per-session replay buffer used when switching tabs' },
      knob_scroll_lines: { type: 'int', def: 2, min: 1, max: 100, doc: 'lines scrolled per knob detent in terminal mode' },
      cursor_blink: { type: 'bool', def: true, doc: 'blinking terminal cursor' },
    },
    logging: {
      level: { type: 'enum', def: 'info', values: ['error', 'warn', 'info', 'debug', 'trace'], doc: 'minimum level written to the console and the ring buffer' },
    },
  };

  function deepFreeze(o) {
    if (o && typeof o === 'object' && !Object.isFrozen(o)) {
      Object.freeze(o);
      Object.keys(o).forEach(function (k) { deepFreeze(o[k]); });
    }
    return o;
  }

  function cloneValue(v) {
    return Array.isArray(v) ? v.slice() : v;
  }

  var DEFAULTS = {};
  Object.keys(SCHEMA).forEach(function (sec) {
    DEFAULTS[sec] = {};
    Object.keys(SCHEMA[sec]).forEach(function (key) {
      DEFAULTS[sec][key] = cloneValue(SCHEMA[sec][key].def);
    });
  });
  deepFreeze(DEFAULTS);

  // Returns null when v is acceptable for rule, else a short reason.
  function check(rule, v) {
    switch (rule.type) {
      case 'string':
        if (typeof v !== 'string' || v.length === 0) return 'expected a non-empty string';
        if (rule.pattern && !rule.pattern.test(v)) return 'does not match ' + rule.pattern;
        return null;
      case 'int':
        if (typeof v !== 'number' || !isFinite(v) || Math.floor(v) !== v) return 'expected an integer';
        break;
      case 'number':
        if (typeof v !== 'number' || !isFinite(v)) return 'expected a number';
        break;
      case 'bool':
        return typeof v === 'boolean' ? null : 'expected true or false';
      case 'enum':
        return rule.values.indexOf(v) === -1 ? 'expected one of ' + rule.values.join(', ') : null;
      case 'strings':
        if (!Array.isArray(v) || v.length === 0) return 'expected a non-empty array of strings';
        for (var i = 0; i < v.length; i++) {
          if (typeof v[i] !== 'string' || v[i].length === 0) return 'expected only non-empty strings';
        }
        return null;
      default:
        return 'unknown rule type';
    }
    if (typeof rule.min === 'number' && v < rule.min) return 'below minimum ' + rule.min;
    if (typeof rule.max === 'number' && v > rule.max) return 'above maximum ' + rule.max;
    return null;
  }

  function defaultWarn(msg) {
    if (typeof console !== 'undefined' && console.warn) console.warn('[frontend] config: ' + msg);
  }

  // Deep-merges `user` over DEFAULTS with validation. Never throws.
  function build(user, warn) {
    warn = warn || defaultWarn;
    var out = {};
    var src = user && typeof user === 'object' ? user : {};
    Object.keys(src).forEach(function (sec) {
      if (!SCHEMA[sec]) warn('unknown section [' + sec + '] ignored');
    });
    Object.keys(SCHEMA).forEach(function (sec) {
      out[sec] = {};
      var given = src[sec] && typeof src[sec] === 'object' ? src[sec] : {};
      Object.keys(given).forEach(function (key) {
        if (!SCHEMA[sec][key]) warn('unknown key ' + sec + '.' + key + ' ignored');
      });
      Object.keys(SCHEMA[sec]).forEach(function (key) {
        var rule = SCHEMA[sec][key];
        var v = given[key];
        if (v === undefined) {
          out[sec][key] = cloneValue(rule.def);
          return;
        }
        var why = check(rule, v);
        if (why) {
          warn(sec + '.' + key + ' ' + why + '; using default ' + JSON.stringify(rule.def));
          out[sec][key] = cloneValue(rule.def);
        } else {
          out[sec][key] = cloneValue(v);
        }
      });
    });
    // Cross-key constraints: fall back to defaults for the whole group.
    if (out.reconnect.max_ms < out.reconnect.initial_ms) {
      warn('reconnect.max_ms is below reconnect.initial_ms; using defaults for both');
      out.reconnect.initial_ms = DEFAULTS.reconnect.initial_ms;
      out.reconnect.max_ms = DEFAULTS.reconnect.max_ms;
    }
    var u = out.ui;
    if (u.font_size_min > u.font_size_max || u.font_size < u.font_size_min || u.font_size > u.font_size_max) {
      warn('ui.font_size must lie within ui.font_size_min..ui.font_size_max; using defaults for the font size group');
      u.font_size = DEFAULTS.ui.font_size;
      u.font_size_min = DEFAULTS.ui.font_size_min;
      u.font_size_max = DEFAULTS.ui.font_size_max;
    }
    return deepFreeze(out);
  }

  // Wraps a validated values object with get('section.key') and frozen
  // section getters.
  function wrap(values) {
    return {
      values: values,
      get: function (path) {
        var parts = String(path).split('.');
        if (parts.length !== 2 || !SCHEMA[parts[0]] || !SCHEMA[parts[0]][parts[1]]) {
          throw new Error('config: unknown key ' + path);
        }
        return values[parts[0]][parts[1]];
      },
      section: function (name) {
        if (!SCHEMA[name]) throw new Error('config: unknown section ' + name);
        return values[name];
      },
    };
  }

  function create(user, warn) {
    return wrap(build(user, warn));
  }

  var api = {
    SCHEMA: SCHEMA,
    DEFAULTS: DEFAULTS,
    build: build,
    create: create,
  };

  // Default instance: window.THING_CONFIG (written by config.generated.js)
  // over DEFAULTS. Absent or malformed generated config means defaults.
  var injected = root.THING_CONFIG;
  var instance = wrap(build(injected, defaultWarn));
  api.get = instance.get;
  api.section = instance.section;
  api.values = instance.values;

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.config = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
