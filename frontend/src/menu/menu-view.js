/*
 * Menu rendering. viewModel(state, screens) is pure (node-testable): it turns
 * the interaction state into what is on screen (title, rows with the
 * highlight, hint, toast). render() is the thin DOM half: it rebuilds the row
 * list from a view model and reports touches as events. The hint comes from
 * the screen and the machine state, not from an if-chain on screen ids.
 *
 * A row may carry `cls` (an extra CSS class) and `hold` (see menu-chart.js).
 *
 * Hint precedence: adjusting, a screen's pinned hint, the text-entry hint of
 * the current screen while typing, the screen's own hint, then the generic
 * depth based hint.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.menuView).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var Types = node ? require('../utils/types.js') : TR.types;
  var Events = node ? require('../interaction/events.js') : TR.events;

  var HINTS = {
    adjusting: 'knob: adjust \u00b7 esc: confirm \u00b7 esc (hold): cancel',
    sub: 'knob: move \u00b7 esc: confirm \u00b7 esc (hold): back',
    top: 'knob: move \u00b7 esc: confirm \u00b7 esc (hold): close',
  };

  function resolve(value, ctx) {
    return typeof value === 'function' ? value(ctx) : value;
  }

  function hintFor(state, screen) {
    if (state.mode === Types.Mode.ADJUSTING) return HINTS.adjusting;
    var h = screen && screen.hint;
    if (h && typeof h === 'object' && h.pinned) return h.text;
    if (state.mode === Types.Mode.EDITING_TEXT && screen && screen.textField) return screen.textField.hint;
    if (h) return typeof h === 'object' ? h.text : h;
    return state.stack.length > 1 ? HINTS.sub : HINTS.top;
  }

  // chart: the object from menu-chart.create(); returns null when not open.
  function viewModel(state, chart) {
    var screen = chart.screenOf(state);
    var ctx = chart.ctxOf(state);
    var vm = { open: state.mode !== Types.Mode.TERMINAL, toast: state.toast, title: '', rows: [], highlight: 0, adjusting: false, hint: '', custom: null, textFieldVisible: false, textField: null, ctx: ctx };
    if (!vm.open || !screen) return vm;
    vm.title = resolve(screen.title, ctx);
    vm.custom = screen.custom || null;
    // A custom body may sit above ordinary rows (first-boot screen); a custom
    // screen without rows (the pickers) has none.
    vm.rows = screen.rows ? screen.rows(ctx) : [];
    vm.highlight = screen.rows ? chart.clampHighlight(state) : 0;
    vm.adjusting = state.mode === Types.Mode.ADJUSTING;
    vm.textFieldVisible = state.mode === Types.Mode.EDITING_TEXT && !!screen.textField;
    if (vm.textFieldVisible) {
      vm.textField = { maxLength: resolve(screen.textField.maxLength, ctx), placeholder: resolve(screen.textField.placeholder, ctx) };
    }
    vm.hint = hintFor(state, screen);
    return vm;
  }

  // ---- DOM half ------------------------------------------------------------------
  // customViews: {name: function (listEl, vm, emit)} for screens with their own body.
  function createRenderer(doc, ids, customViews, emit) {
    var el = function (id) { return doc.getElementById(id); };

    function rowEl(entry, i, vm) {
      var row = doc.createElement('div');
      row.className = 'menu-row' + (i === vm.highlight ? ' sel' : '') + (i === vm.highlight && vm.adjusting ? ' adjusting' : '') + (entry.cls ? ' ' + entry.cls : '');
      var label = doc.createElement('span');
      label.className = 'menu-row-label';
      label.textContent = entry.label;
      row.appendChild(label);
      if (entry.kind === 'toggle' || entry.kind === 'range') {
        var value = doc.createElement('span');
        value.className = 'menu-row-value';
        value.textContent = entry.display;
        row.appendChild(value);
      }
      if (entry.valueText) {
        var secondary = doc.createElement('span');
        secondary.className = 'menu-row-value menu-row-secondary';
        secondary.textContent = entry.valueText;
        row.appendChild(secondary);
      }
      if (typeof entry.bars === 'number') {
        // Four-step signal indicator; lit steps use the row's own text color.
        var bars = doc.createElement('span');
        bars.className = 'wifi-bars';
        for (var b = 1; b <= 4; b++) {
          var bar = doc.createElement('i');
          bar.className = b <= entry.bars ? 'on' : '';
          bar.style.height = (b * 3 + 2) + 'px';
          bars.appendChild(bar);
        }
        row.appendChild(bars);
      }
      if (typeof entry.statusDot === 'boolean') {
        // Paired/saved connected indicator, reusing the .status-dot convention.
        var state = doc.createElement('span');
        state.className = 'menu-row-value';
        state.textContent = entry.statusDot ? 'Connected' : 'Disconnected';
        row.appendChild(state);
        var dot = doc.createElement('span');
        dot.className = 'menu-row-dot status-dot' + (entry.statusDot ? ' ok' : '');
        row.appendChild(dot);
      }
      // Touch is a secondary input: tapping a row selects it like a knob push.
      row.addEventListener('click', function () { emit(Events.rowTap(i)); });
      return row;
    }

    // The one <input> serves every text screen: each may set its own limit and
    // placeholder; the page's own values are the defaults (Wi-Fi password).
    function applyFieldLimits(input, tf) {
      if (!input) return;
      if (input.getAttribute('data-ph') === null) {
        input.setAttribute('data-ph', input.getAttribute('placeholder') || '');
        input.setAttribute('data-max', input.getAttribute('maxlength') || '');
      }
      var ph = tf && tf.placeholder ? tf.placeholder : input.getAttribute('data-ph');
      var max = tf && tf.maxLength ? String(tf.maxLength) : input.getAttribute('data-max');
      input.setAttribute('placeholder', ph);
      if (max) input.setAttribute('maxlength', max); else input.removeAttribute('maxlength');
    }

    return function render(vm) {
      var toastEl = el(ids.toast);
      if (toastEl) {
        if (vm.toast) {
          toastEl.textContent = vm.toast;
          toastEl.classList.remove('hidden');
        } else {
          toastEl.classList.add('hidden');
        }
      }
      el(ids.overlay).classList.toggle('hidden', !vm.open);
      var titleEl = el(ids.title);
      var listEl = el(ids.list);
      // A closed menu must not keep the last screen's DOM (a one-time password).
      if (!vm.open) {
        listEl.innerHTML = '';
        return;
      }
      titleEl.textContent = vm.title;
      var inputWrap = el(ids.inputWrap);
      if (inputWrap) {
        inputWrap.classList.toggle('hidden', !vm.textFieldVisible);
        applyFieldLimits(inputWrap.querySelector('input'), vm.textField);
      }
      listEl.innerHTML = '';
      if (vm.custom && customViews[vm.custom]) customViews[vm.custom](listEl, vm, emit);
      vm.rows.forEach(function (entry, i) { listEl.appendChild(rowEl(entry, i, vm)); });
      var hintEl = el(ids.hint);
      if (hintEl) hintEl.textContent = vm.hint;
    };
  }

  var api = { viewModel: viewModel, createRenderer: createRenderer, hintFor: hintFor, HINTS: HINTS };

  if (node) module.exports = api;
  else TR.menuView = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
