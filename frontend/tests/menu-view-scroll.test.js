'use strict';
// scrollTopFor is the pure math behind keeping the highlighted menu row
// inside the .menu-list viewport (see render() in src/menu/menu-view.js for
// the DOM half: it preserves scrollTop across a rebuild, then passes real
// offsetTop/offsetHeight/scrollTop/clientHeight numbers through this).
const test = require('node:test');
const assert = require('node:assert/strict');
const MenuView = require('../src/menu/menu-view.js');
const scrollTopFor = MenuView.scrollTopFor;

test('row above the visible area: scrolls up to the row\'s own top', () => {
  // list shows [100, 220); row 3 sits at 40..70, fully above the fold.
  assert.equal(scrollTopFor(3, 40, 30, 100, 120), 40);
});

test('row below the visible area: scrolls down just enough to show its bottom', () => {
  // list shows [0, 120); row 10 sits at 300..330, below the fold.
  assert.equal(scrollTopFor(10, 300, 30, 0, 120), 210);
});

test('row already fully visible: scrollTop is left untouched', () => {
  // list shows [100, 220); row at 150..180 is entirely inside that range.
  assert.equal(scrollTopFor(5, 150, 30, 100, 120), 100);
});

test('highlight 0 always pins to the very top, even mid-scroll', () => {
  assert.equal(scrollTopFor(0, 500, 30, 240, 120), 0);
});

test('last row below the fold scrolls down by the same minimal rule', () => {
  // 20 rows of height 30 (600px of content), list clientHeight 120; the
  // last row (index 19) sits at 570..600, well past a scrollTop of 0.
  assert.equal(scrollTopFor(19, 570, 30, 0, 120), 480);
});

test('last row already visible from a prior scroll stays put', () => {
  assert.equal(scrollTopFor(19, 570, 30, 480, 120), 480);
});
