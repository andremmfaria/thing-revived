/*
 * DOM bodies of the first-boot screens, registered as custom views
 * 'users-bootstrap' (banner plus the one-time password in groups of four) and
 * 'users-ack' (the entered confirmation characters and the knob picker strip).
 *
 * The password is drawn as text in plain spans, never in an input, with
 * user-select off in the stylesheet. It exists in the DOM only while this
 * screen renders; menu-view clears the list whenever another screen renders or
 * the menu closes.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.usersSecretView).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var WL = node ? require('../wifi/wifi-logic.js') : TR.wifiLogic;
  var UL = node ? require('./users-logic.js') : TR.usersLogic;
  var Events = node ? require('../interaction/events.js') : TR.events;

  function div(doc, cls, text) {
    var el = doc.createElement('div');
    el.className = cls;
    if (text !== undefined) el.textContent = text;
    return el;
  }

  // doc: the document, used to create elements.
  function create(doc) {
    function bootstrap(listEl, vm) {
      var u = vm.ctx.ui.users;
      var b = vm.ctx.domain.users.bootstrap;
      var name = b && b.username ? b.username : 'kiosk';
      var box = div(doc, 'users-banner');
      box.appendChild(div(doc, 'users-banner-title', 'FIRST BOOT'));
      box.appendChild(div(doc, 'users-banner-text', 'Admin account "' + name + '" created. Write down its one-time password.'));
      if (u.secret) {
        var pw = div(doc, 'users-secret');
        UL.groups(u.secret).forEach(function (g) {
          var span = doc.createElement('span');
          span.className = 'users-secret-group';
          span.textContent = g;
          pw.appendChild(span);
        });
        box.appendChild(pw);
      }
      listEl.appendChild(box);
    }

    function ack(listEl, vm, emit) {
      var u = vm.ctx.ui.users;
      listEl.appendChild(div(doc, 'users-ack-help', 'Choose the last ' + UL.ACK_LEN + ' characters of the password, then [OK]'));
      listEl.appendChild(div(doc, 'users-ack-entry', UL.ackDisplay(u.ack)));
      var strip = div(doc, 'wifi-picker-strip');
      WL.pickerWindow(u.ackIdx, 3).forEach(function (cell) {
        var el = doc.createElement('span');
        el.className = 'wifi-picker-cell' + (cell.offset === 0 ? ' cur' : '') +
          (cell.item.length > 1 && cell.item !== ' ' ? ' ctl' : '');
        el.textContent = WL.pickerLabel(cell.item);
        el.addEventListener('click', function () { emit(Events.customTap(cell.offset, cell.index)); });
        strip.appendChild(el);
      });
      listEl.appendChild(strip);
      listEl.appendChild(div(doc, 'wifi-picker-help', '[DEL] deletes the last character, [OK] confirms'));
    }

    return { bootstrap: bootstrap, ack: ack };
  }

  var api = { create: create };

  if (node) module.exports = api;
  else TR.usersSecretView = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
