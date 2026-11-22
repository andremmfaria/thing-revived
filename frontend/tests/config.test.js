'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const Config = require('../src/utils/config.js');
const toml = require('./helpers/toml-subset.js');

const TOML_PATH = path.resolve(__dirname, '..', '..', 'config', 'frontend.toml');
const quiet = () => {};

test('config/frontend.toml equals the compiled-in DEFAULTS exactly', () => {
  const parsed = toml.parse(fs.readFileSync(TOML_PATH, 'utf8'));
  assert.deepEqual(parsed, JSON.parse(JSON.stringify(Config.DEFAULTS)));
});

test('every key in config/frontend.toml has a comment line directly above it', () => {
  const keys = toml.documentedKeys(fs.readFileSync(TOML_PATH, 'utf8'));
  assert.ok(keys.length > 20);
  keys.forEach((k) => {
    assert.ok(k.comment.length > 3, k.section + '.' + k.key + ' has no comment');
  });
});

test('TOML test reader rejects unsupported syntax instead of guessing', () => {
  assert.throws(() => toml.parse('[a]\nx = [1, 2]'), /strings/);
  assert.throws(() => toml.parse('x = 1'), /cannot parse/);
  assert.throws(() => toml.parse('[a]\nx = 1\nx = 2'), /duplicate key/);
  assert.deepEqual(toml.parse('[a]\ns = "q\\"x" # c\nn = -3\nf = 1.5\nb = true\nl = ["a", "b"]'), { a: { s: 'q"x', n: -3, f: 1.5, b: true, l: ['a', 'b'] } });
});

test('with no generated config the default instance serves DEFAULTS', () => {
  assert.equal(Config.get('bridge.url'), 'ws://127.0.0.1:8765');
  assert.equal(Config.get('agent.url'), 'ws://127.0.0.1:8766');
  assert.equal(Config.get('reconnect.factor'), 1.0);
  assert.equal(Config.section('input').esc_hold_ms, 450);
});

test('deep merge: provided keys win, the rest keep defaults', () => {
  const c = Config.create({ ui: { toast_ms: 1234 }, logging: { level: 'debug' } }, quiet);
  assert.equal(c.get('ui.toast_ms'), 1234);
  assert.equal(c.get('ui.font_size'), 13);
  assert.equal(c.get('logging.level'), 'debug');
  assert.equal(c.get('bridge.url'), 'ws://127.0.0.1:8765');
});

test('bad values warn and fall back to the default, never throw', () => {
  const warnings = [];
  const c = Config.create({
    bridge: { url: 'http://nope' },
    input: { esc_hold_ms: 'fast', enter_guard_ms: -5 },
    reconnect: { factor: 0.5 },
    clock: { format: 'hh:mm' },
    terminal: { replay_buffer_bytes: 10 },
    ui: { font_families: [], theme_cursor: 'green' },
    logging: { level: 'loud' },
    bogus: { x: 1 },
  }, (m) => warnings.push(m));
  assert.equal(c.get('bridge.url'), 'ws://127.0.0.1:8765');
  assert.equal(c.get('input.esc_hold_ms'), 450);
  assert.equal(c.get('input.enter_guard_ms'), 400);
  assert.equal(c.get('reconnect.factor'), 1.0);
  assert.equal(c.get('clock.format'), 'HH:mm:ss');
  assert.equal(c.get('terminal.replay_buffer_bytes'), 262144);
  assert.deepEqual(c.get('ui.font_families'), ['JetBrains Mono', 'Consolas', 'Courier New', 'monospace']);
  assert.equal(c.get('ui.theme_cursor'), '#35e38a');
  assert.equal(c.get('logging.level'), 'info');
  assert.ok(warnings.length >= 9, 'warnings: ' + warnings.length);
  assert.ok(warnings.some((w) => /unknown section \[bogus\]/.test(w)));
});

test('non-object input is treated as empty', () => {
  [undefined, null, 5, 'x', []].forEach((bad) => {
    const c = Config.create(bad, quiet);
    assert.equal(c.get('ui.toast_ms'), 4000);
  });
});

test('cross-key constraints fall back to defaults as a group', () => {
  const w = [];
  let c = Config.create({ reconnect: { initial_ms: 5000, max_ms: 1000 } }, (m) => w.push(m));
  assert.equal(c.get('reconnect.initial_ms'), 2000);
  assert.equal(c.get('reconnect.max_ms'), 30000);
  c = Config.create({ ui: { font_size: 40 } }, (m) => w.push(m));
  assert.equal(c.get('ui.font_size'), 13);
  assert.equal(w.length, 2);
});

test('sections and arrays are frozen; get() rejects unknown keys', () => {
  const c = Config.create({}, quiet);
  const ui = c.section('ui');
  assert.ok(Object.isFrozen(ui));
  assert.ok(Object.isFrozen(ui.font_families));
  assert.throws(() => { 'use strict'; ui.toast_ms = 1; }, TypeError);
  assert.throws(() => c.get('ui.nope'), /unknown key/);
  assert.throws(() => c.get('nope'), /unknown key/);
  assert.throws(() => c.section('nope'), /unknown section/);
  assert.ok(Object.isFrozen(Config.DEFAULTS.ui));
});

test('accepts the documented backoff growth settings', () => {
  const c = Config.create({ reconnect: { initial_ms: 500, max_ms: 8000, factor: 2 } }, quiet);
  assert.deepEqual(Object.assign({}, c.section('reconnect')), { initial_ms: 500, max_ms: 8000, factor: 2 });
});
