/*
 * Leveled logger for the kiosk UI. Writes through console.* (the Chromium
 * remote-debugging port exposes it) and keeps the last RING_SIZE entries in
 * memory, retrievable with TR.logging.dump() from the debugging console.
 *
 * Secrets: wire messages must be logged through describe()/redact(), which
 * replace passwords, PSKs, passkeys and typed or terminal text with
 * "[redacted]". Free-text log messages must never contain those values;
 * callers pass only message types, ids and counts.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.logging,
 * node: module.exports).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var Config = node ? require('./config.js') : TR.config;

  var LEVELS = ['error', 'warn', 'info', 'debug', 'trace'];
  var RING_SIZE = 200;
  var PREFIX = '[frontend]';

  // Field names whose values never reach a log line. `data` covers terminal
  // input and output frames, `text` typed text. `value` is only secret on
  // typed-text events, so settings messages such as
  // {type:'brightness_set', value:80} stay readable.
  var SECRET_KEYS = {
    password: 1, current_password: 1, psk: 1, passkey: 1, passphrase: 1, secret: 1, token: 1,
    last4: 1, data: 1, text: 1,
  };
  var REDACTED = '[redacted]';

  function isTypedText(o) {
    return o.type === 'text' || o.type === 'input' || o.type === 'textInput';
  }

  function redact(v, depth) {
    depth = depth || 0;
    if (v === null || typeof v !== 'object') return v;
    if (depth > 6) return '[deep]';
    if (Array.isArray(v)) return v.map(function (x) { return redact(x, depth + 1); });
    var out = {};
    var typed = isTypedText(v);
    Object.keys(v).forEach(function (k) {
      var lk = k.toLowerCase();
      var secret = SECRET_KEYS[lk] === 1 || (lk === 'value' && typed);
      out[k] = secret ? REDACTED : redact(v[k], depth + 1);
    });
    return out;
  }

  // Safe one-line description of a wire message: `wifi_connect {"ssid":"Home","password":"[redacted]"}`.
  function describe(msg) {
    if (msg === null || typeof msg !== 'object') return String(msg);
    var type = typeof msg.type === 'string' ? msg.type : '?';
    var rest = {};
    Object.keys(msg).forEach(function (k) { if (k !== 'type') rest[k] = msg[k]; });
    var body = redact(rest);
    return Object.keys(body).length ? type + ' ' + JSON.stringify(body) : type;
  }

  function stringify(arg) {
    if (arg instanceof Error) return arg.message;
    if (arg !== null && typeof arg === 'object') {
      try { return JSON.stringify(redact(arg)); } catch (e) { return '[unserializable]'; }
    }
    return String(arg);
  }

  // opts: { level, sink (console-like), now, ringSize }
  function create(opts) {
    opts = opts || {};
    var sink = opts.sink || (typeof console !== 'undefined' ? console : null);
    var now = opts.now || function () { return Date.now(); };
    var ringSize = opts.ringSize || RING_SIZE;
    var threshold = LEVELS.indexOf(opts.level || 'info');
    if (threshold < 0) threshold = LEVELS.indexOf('info');
    var ring = [];

    function emit(level, component, args) {
      if (LEVELS.indexOf(level) > threshold) return;
      var text = Array.prototype.map.call(args, stringify).join(' ');
      var entry = { t: now(), level: level, component: component || '', text: text };
      ring.push(entry);
      if (ring.length > ringSize) ring.shift();
      if (!sink) return;
      var line = PREFIX + ' ' + level.toUpperCase() + (component ? ' ' + component : '') + ': ' + text;
      var fn = level === 'error' ? 'error' : level === 'warn' ? 'warn' : level === 'info' ? 'info' : 'debug';
      if (typeof sink[fn] === 'function') sink[fn](line);
      else if (typeof sink.log === 'function') sink.log(line);
    }

    function logger(component) {
      var l = {};
      LEVELS.forEach(function (lv) {
        l[lv] = function () { emit(lv, component, arguments); };
      });
      l.child = function (name) { return logger(component ? component + '.' + name : name); };
      return l;
    }

    var log = logger('');
    log.setLevel = function (lv) {
      var i = LEVELS.indexOf(lv);
      if (i >= 0) threshold = i;
    };
    log.getLevel = function () { return LEVELS[threshold]; };
    log.dump = function () { return ring.map(function (e) { return { t: e.t, level: e.level, component: e.component, text: e.text }; }); };
    log.describe = describe;
    log.redact = redact;
    return log;
  }

  var api = create({ level: Config.get('logging.level') });
  api.create = create;
  api.describe = describe;
  api.redact = redact;
  api.LEVELS = LEVELS;
  api.RING_SIZE = RING_SIZE;

  if (node) module.exports = api;
  else TR.logging = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
