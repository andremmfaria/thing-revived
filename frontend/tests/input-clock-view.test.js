'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const Keyboard = require('../src/input/keyboard.js');
const Buttons = require('../src/input/buttons.js');
const Clock = require('../src/clock/clock.js');
const TerminalView = require('../src/terminal/terminal-view.js');
const Effects = require('../src/interaction/effects.js');
const Logging = require('../src/utils/logging.js');
const Config = require('../src/utils/config.js');
const Events = require('../src/interaction/events.js');
const { createFakeTimers } = require('./helpers/fake-timers.js');
const { makeHarness } = require('./helpers/chart-harness.js');
const MenuView = require('../src/menu/menu-view.js');

test('keyboard.toBytes maps keys like a terminal', () => {
  const k = (key, ctrlKey) => Keyboard.toBytes({ key, ctrlKey: !!ctrlKey });
  assert.equal(k('a'), 'a');
  assert.equal(k('Enter'), '\r');
  assert.equal(k('Backspace'), '\x7f');
  assert.equal(k('ArrowLeft'), '\x1b[D');
  assert.equal(k('c', true), '\x03');
  assert.equal(k('Z', true), '\x1a');
  assert.equal(k('1', true), null);
  assert.equal(k('Shift'), null);
  assert.equal(k('F5'), null);
});

test('keyboard.bind swallows everything while the menu is open', () => {
  let handler;
  const el = { addEventListener: (type, fn, opts) => { handler = { type, fn, opts }; } };
  let open = true;
  const sent = [];
  Keyboard.bind(el, () => open, (d) => sent.push(d));
  assert.equal(handler.type, 'keydown');
  assert.deepEqual(handler.opts, { capture: true });
  const evt = (key) => ({ key, ctrlKey: false, prevented: false, preventDefault() { this.prevented = true; } });
  let e = evt('a');
  handler.fn(e);
  assert.equal(e.prevented, true);
  assert.deepEqual(sent, []);
  open = false;
  e = evt('a'); handler.fn(e);
  e = evt('Shift'); handler.fn(e);
  assert.deepEqual(sent, ['a']);
  assert.equal(e.prevented, false);
});

test('buttons: session mapping and phases', () => {
  assert.deepEqual([2, 3, 4, 5].map(Buttons.sessionForButton), [0, 1, 2, 3]);
  assert.equal(Buttons.sessionForButton(28), null);
  assert.equal(Buttons.sessionForButton(1), null);
  assert.deepEqual([1, 0, 2].map(Buttons.phaseOf), ['down', 'up', 'repeat']);
  assert.equal(Events.button(1, 1).phase, 'down');
});

test('clock: format and dot classes', () => {
  const t = new Date(2026, 0, 1, 9, 5, 7).getTime();
  assert.equal(Clock.formatClock(t, 0, 'HH:mm:ss'), '09:05:07');
  assert.equal(Clock.formatClock(t, 3600000, 'HH:mm:ss'), '10:05:07');
  assert.equal(Clock.formatClock(t, -7000, 'HH:mm'), '09:05');
  assert.deepEqual(Clock.dotClasses('ok'), { ok: true, stale: false });
  assert.deepEqual(Clock.dotClasses('stale'), { ok: false, stale: true });
  assert.deepEqual(Clock.dotClasses('unsynced'), { ok: false, stale: true });
});

test('clock.create redraws on the configured interval', () => {
  const ft = createFakeTimers();
  const els = { 'clock-dot': { classList: { toggle(n, f) { this[n] = f; } } }, 'clock-text': { textContent: '' } };
  const doc = { getElementById: (id) => els[id] };
  const c = Clock.create({ doc, timers: ft, getClock: () => ({ offsetMs: 0, status: 'ok' }), format: 'HH:mm', intervalMs: 500 });
  c.start();
  assert.equal(els['clock-text'].textContent, '');
  ft.tick(500);
  assert.match(els['clock-text'].textContent, /^\d\d:\d\d$/);
  assert.equal(els['clock-dot'].classList.ok, true);
});

test('terminal view: constructs xterm from config, fits and reports size on every refit', () => {
  const made = [];
  class FakeTerminal {
    constructor(o) { this.opts = o; this.options = {}; this.cols = 80; this.rows = 24; this.log = []; made.push(this); }
    loadAddon() {}
    open() { this.log.push('open'); }
    write(d) { this.log.push('w:' + d); }
    reset() { this.log.push('reset'); }
    scrollLines(n) { this.log.push('scroll:' + n); }
    focus() {}
  }
  const fits = [];
  class FakeFit { fit() { fits.push(1); } }
  const sizes = [];
  const cfg = Config.create({}, () => {}).values;
  const v = TerminalView.create({ Terminal: FakeTerminal, FitAddon: FakeFit, ui: cfg.ui, terminal: cfg.terminal, onResize: (c, r) => sizes.push([c, r]) });
  const t = made[0];
  assert.equal(t.opts.fontSize, 13);
  assert.equal(t.opts.scrollback, 1000);
  assert.equal(t.opts.disableStdin, true);
  assert.deepEqual(t.opts.theme, { background: '#05070a', foreground: '#c9dcea', cursor: '#35e38a' });
  v.open({});
  v.setFontSize(15);
  assert.equal(t.options.fontSize, 15);
  v.setFontFamily("'Consolas', monospace");
  assert.deepEqual(sizes, [[80, 24], [80, 24]]);
  assert.equal(fits.length, 3);
  v.replay(['a', 'b']);
  v.scrollLines(-4);
  assert.deepEqual(t.log, ['open', 'reset', 'w:a', 'w:b', 'scroll:-4']);
});

test('effects interpreter runs each descriptor, isolates failures and logs unknown ones', () => {
  const calls = [];
  const logger = Logging.create({ level: 'trace', sink: null });
  const fx = Effects.create({
    bridge: { send: (m) => { calls.push(['send', m]); if (m.type === 'boom') throw new Error('x'); } },
    store: { dispatch: (a) => calls.push(['dispatch', a]) },
    named: { start: (n, ms) => calls.push(['start', n, ms]), cancel: (n) => calls.push(['cancel', n]) },
    term: { scrollLines: (n) => calls.push(['scroll', n]), setFontSize: (v) => calls.push(['size', v]), setFontFamily: (f) => calls.push(['family', f]) },
    sessions: { switchTo: (i) => calls.push(['switch', i]) },
    text: { begin: (t) => calls.push(['begin', t]), end: () => calls.push(['end']), reset: () => calls.push(['reset']) },
    logger,
  });
  fx.run([
    { send: { type: 'boom' } }, { send: { type: 'ok' } }, { dispatch: { type: 'a' } }, { startTimer: { name: 'n', ms: 5 } },
    { cancelTimer: 'n' }, { scroll: 2 }, { switchSession: 3 }, { term: { fontSize: 9 } }, { term: { fontFamily: 'f' } },
    { textBegin: { masked: true, value: 'v' } }, { textEnd: true }, { textReset: true }, { nonsense: 1 },
  ]);
  assert.deepEqual(calls.map((c) => c[0]), ['send', 'send', 'dispatch', 'start', 'cancel', 'scroll', 'switch', 'size', 'family', 'begin', 'end', 'reset']);
  const errs = logger.dump().filter((e) => e.level === 'error').map((e) => e.text);
  assert.equal(errs.length, 2);
  assert.ok(errs.some((t) => /unknown effect nonsense/.test(t)));
});

test('menu view model: hints come from state and screen data with the documented precedence', () => {
  const h = makeHarness();
  h.open();
  assert.equal(h.view().hint, MenuView.HINTS.top);
  h.moveTo('Terminal'); h.push();
  assert.equal(h.view().hint, MenuView.HINTS.sub);
  h.push(); // arm font size
  assert.equal(h.view().hint, MenuView.HINTS.adjusting);
  const v = h.view();
  assert.equal(v.adjusting, true);
  assert.equal(v.open, true);
  h.btn(1, 1); h.btn(1, 0);
  const closed = makeHarness().view();
  assert.equal(closed.open, false);
  assert.deepEqual(closed.rows, []);
});
