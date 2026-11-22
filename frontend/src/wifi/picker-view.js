/*
 * DOM body of the Wi-Fi knob character picker screen: the masked password so
 * far, a counter and the ring strip around the current character. Registered
 * as the custom view 'wifi-picker' (see screens/wifi.js `custom`).
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.wifiPickerView).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var WL = node ? require('./wifi-logic.js') : TR.wifiLogic;
  var Events = node ? require('../interaction/events.js') : TR.events;

  // doc: the document, used to create elements.
  function create(doc) {
    return function render(listEl, vm, emit) {
      var w = vm.ctx.ui.wifi;
      var pw = doc.createElement('div');
      pw.className = 'wifi-picker-pw';
      pw.textContent = (w.password.length ? WL.maskPassword(w.password, w.showPassword) : '(empty)') + '_';
      listEl.appendChild(pw);
      var count = doc.createElement('div');
      count.className = 'wifi-picker-count';
      count.textContent = w.password.length + ' / ' + WL.PASSWORD_MAX + ' characters';
      listEl.appendChild(count);
      var strip = doc.createElement('div');
      strip.className = 'wifi-picker-strip';
      WL.pickerWindow(w.pickerIdx, 3).forEach(function (cell) {
        var el = doc.createElement('span');
        el.className = 'wifi-picker-cell' + (cell.offset === 0 ? ' cur' : '') +
          (cell.item.length > 1 && cell.item !== ' ' ? ' ctl' : '');
        el.textContent = WL.pickerLabel(cell.item);
        el.addEventListener('click', function () { emit(Events.customTap(cell.offset, cell.index)); });
        strip.appendChild(el);
      });
      listEl.appendChild(strip);
      var help = doc.createElement('div');
      help.className = 'wifi-picker-help';
      help.textContent = '[DEL] deletes the last character, [OK] returns to the password screen';
      listEl.appendChild(help);
    };
  }

  var api = { create: create };

  if (node) module.exports = api;
  else TR.wifiPickerView = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
