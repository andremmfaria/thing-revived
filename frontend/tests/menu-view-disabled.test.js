'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const Chart = require('../src/interaction/menu-chart.js');
const MenuView = require('../src/menu/menu-view.js');

// Minimal DOM double: just what rowEl and render touch.
function fakeDoc() {
  const mk = (tag) => {
    const e = { tag, className: '', textContent: '', children: [], attrs: {}, style: {}, listeners: {} };
    e.appendChild = (c) => { e.children.push(c); c.parentNode = e; return c; };
    e.setAttribute = (k, v) => { e.attrs[k] = String(v); };
    e.getAttribute = (k) => (k in e.attrs ? e.attrs[k] : null);
    e.removeAttribute = (k) => { delete e.attrs[k]; };
    e.addEventListener = (n, f) => { e.listeners[n] = f; };
    e.querySelector = () => null;
    e.classList = { add() {}, remove() {}, toggle() {} };
    return e;
  };
  const els = {};
  ['toast', 'overlay', 'title', 'list', 'inputWrap', 'input', 'hint'].forEach((id) => { els[id] = mk('div'); });
  return { els, createElement: mk, getElementById: (id) => els[id] };
}

function build(rows) {
  const screens = {
    defs: { root: { title: 'T', rows: () => rows } },
    reactions: {},
    initialUi: () => ({}),
  };
  const cfg = { input: { esc_hold_ms: 450, enter_guard_ms: 0 }, ui: { toast_ms: 2000 }, terminal: { knob_scroll_lines: 2 } };
  const chart = Chart.create({ screens, cfg });
  const state = Object.assign(chart.initial({}), { mode: 'open.list' });
  return { chart, state };
}

function render(rows, ctxState) {
  const { chart, state } = build(rows);
  const doc = fakeDoc();
  const draw = MenuView.createRenderer(doc, { toast: 'toast', overlay: 'overlay', title: 'title', list: 'list', inputWrap: 'inputWrap', input: 'input', hint: 'hint' }, {}, () => {});
  draw(MenuView.viewModel(ctxState || state, chart));
  return doc.els.list.children;
}

test('disabled row renders class disabled and aria-disabled; enabled rows do not', () => {
  const kids = render([
    { id: 'a', label: 'A', kind: 'label' },
    { id: 'b', label: 'B', kind: 'label', disabled: true },
    { id: 'c', label: 'C', kind: 'label', disabled: () => true },
    { id: 'd', label: 'D', kind: 'label', disabled: false },
  ]);
  assert.equal(kids[0].className, 'menu-row sel');
  assert.equal(kids[0].getAttribute('aria-disabled'), null);
  assert.match(kids[1].className, /\bdisabled\b/);
  assert.equal(kids[1].getAttribute('aria-disabled'), 'true');
  assert.match(kids[2].className, /\bdisabled\b/);
  assert.equal(kids[3].className, 'menu-row');
  assert.equal(kids[3].getAttribute('aria-disabled'), null);
});

test('a highlighted disabled row has both sel and disabled', () => {
  const { chart, state } = build([{ id: 'a', label: 'A', kind: 'label', disabled: true }]);
  const doc = fakeDoc();
  const draw = MenuView.createRenderer(doc, { toast: 'toast', overlay: 'overlay', title: 'title', list: 'list', inputWrap: 'inputWrap', input: 'input', hint: 'hint' }, {}, () => {});
  draw(MenuView.viewModel(state, chart));
  assert.match(doc.els.list.children[0].className, /\bsel\b.*\bdisabled\b/);
});
