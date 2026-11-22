'use strict';
// index.html is the module graph: every listed script must exist, every
// src/*.js must be listed, and loading them in listed order must provide
// everything boot.js asserts on (the same check it does at runtime).
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const Boot = require('../src/boot.js');

const FRONTEND = path.resolve(__dirname, '..');
const html = fs.readFileSync(path.join(FRONTEND, 'index.html'), 'utf8');
const scripts = Array.from(html.matchAll(/<script src="([^"]+)"><\/script>/g)).map((m) => m[1]);

function walk(dir, out) {
  fs.readdirSync(dir, { withFileTypes: true }).forEach((e) => {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) walk(p, out); else if (e.name.endsWith('.js')) out.push(p);
  });
  return out;
}

test('every script in index.html exists and every src module is listed exactly once', () => {
  scripts.forEach((s) => assert.ok(fs.existsSync(path.join(FRONTEND, s)), 'missing ' + s));
  assert.equal(new Set(scripts).size, scripts.length, 'duplicate script tag');
  const listed = new Set(scripts.map((s) => path.join(FRONTEND, s)));
  walk(path.join(FRONTEND, 'src'), []).forEach((f) => assert.ok(listed.has(f), 'not in index.html: ' + path.relative(FRONTEND, f)));
  assert.equal(scripts[scripts.length - 1], 'src/boot.js');
  assert.ok(scripts.indexOf('config.generated.js') < scripts.indexOf('src/utils/config.js'));
});

test('loading the scripts in index.html order as classic scripts provides every module boot needs', () => {
  const ctx = { console: { warn() {}, info() {}, debug() {}, error() {} } };
  ctx.window = ctx;
  vm.createContext(ctx);
  scripts.filter((s) => s.startsWith('src/') || s === 'config.generated.js').filter((s) => s !== 'src/boot.js').forEach((s) => {
    vm.runInContext(fs.readFileSync(path.join(FRONTEND, s), 'utf8'), ctx, { filename: s });
  });
  Boot.NEEDED.forEach((n) => assert.ok(ctx.TR[n], 'TR.' + n + ' not provided'));
  ctx.TR.module.assertLoaded(Boot.NEEDED, ctx.TR);
  assert.equal(ctx.TR.config.get('bridge.url'), 'ws://127.0.0.1:8765');
});

test('a wrong order is reported by name at boot, not as a later undefined crash', () => {
  const ctx = { console };
  ctx.window = ctx;
  vm.createContext(ctx);
  vm.runInContext(fs.readFileSync(path.join(FRONTEND, 'src/core/module.js'), 'utf8'), ctx);
  assert.throws(() => ctx.TR.module.assertLoaded(Boot.NEEDED, ctx.TR), /TR modules missing: .*channel.*<script> order/);
});
