/*
 * First-boot overlay (delta plan sections 5.3 and 6): a full-screen message
 * that is not part of the menu stack, so no menu route can reach or dismiss
 * it. The controller owns the phases (prompt -> generating -> showing -> done)
 * and counts the physical M presses; this module only reflects state and
 * blocks every input path while the overlay is up.
 *
 * The password lives in a module-private variable and in the overlay DOM
 * only: never in the store, the log, localStorage or the interaction state.
 * It is wiped on any phase that is not `showing`, and when the bridge closes.
 *
 * WIRE is the single place that knows the controller's field and type names
 * for this flow (plan section 5.6). Adjust here if D-C1 names differ.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.firstBoot).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var UL = node ? require('../users/users-logic.js') : TR.usersLogic;

  var WIRE = {
    stateEvent: 'user_bootstrap', // {phase, username, storage_ok}
    secretEvent: 'user_bootstrap_secret', // {username, password}
    errorEvent: 'user_error', // {code, message, context}
    errorContextPrefix: 'user_bootstrap',
    errorCodes: ['generate_failed', 'ack_failed', 'storage_unavailable'],
    phaseField: 'phase',
    usernameField: 'username',
    storageOkField: 'storage_ok',
    passwordField: 'password',
  };
  // 'retry' is local, not a wire phase: the controller could not read its account
  // state, so whether the kiosk password is set is unknown. Input stays blocked
  // (fail closed) and the overlay asks again until a real phase arrives.
  var BLOCKING = { prompt: 1, generating: 1, showing: 1, retry: 1 };
  var RETRY_MS = 3000;
  var RETRY_CODE = 'internal';
  var SWALLOWED_KEYS = ['keydown', 'keypress', 'keyup'];
  var SWALLOWED_POINTER = ['touchstart', 'touchmove', 'touchend', 'mousedown', 'mouseup', 'click', 'pointerdown', 'pointerup', 'wheel'];

  // A user_error belongs to this flow by its context, or by a first-boot code
  // while the overlay is up. Any other user_error goes to the normal toast.
  function isOurs(msg, active) {
    if (msg.type === WIRE.stateEvent || msg.type === WIRE.secretEvent) return true;
    if (msg.type !== WIRE.errorEvent) return false;
    if (typeof msg.context === 'string' && msg.context.indexOf(WIRE.errorContextPrefix) === 0) return true;
    return active === true && WIRE.errorCodes.indexOf(msg.code) !== -1;
  }

  // The state machine, DOM free. onChange() runs after every transition.
  // opts (optional): { timers, retry: function () {}, retryMs } - the retry
  // state asks again through opts.retry on a timer; without them it just waits.
  function createModel(onChange, opts) {
    opts = opts || {};
    var retryTimer = null;
    var phase = 'none';
    var username = 'kiosk';
    var storageOk = true;
    var password = null; // module-private on purpose, see header
    var error = null;

    function wipe() { password = null; }
    function cancelRetry() {
      if (retryTimer !== null) { opts.timers.clearTimeout(retryTimer); retryTimer = null; }
    }
    function scheduleRetry() {
      cancelRetry();
      if (!opts.timers || !opts.retry) return;
      retryTimer = opts.timers.setTimeout(function () { retryTimer = null; opts.retry(); }, opts.retryMs || RETRY_MS);
    }
    function changed() { if (onChange) onChange(); }

    return {
      // true while every input path must be dead
      active: function () { return storageOk && BLOCKING[phase] === 1; },
      phase: function () { return phase; },
      // storage failure: informational only, nothing is blocked
      storageNote: function () { return !storageOk && phase !== 'done'; },
      view: function () {
        return {
          phase: phase, username: username, storageOk: storageOk, error: error,
          password: password,
        };
      },
      // Returns true when the message was consumed (callers must not forward it).
      handle: function (msg) {
        if (msg.type === WIRE.stateEvent) {
          var next = msg[WIRE.phaseField];
          cancelRetry();
          phase = typeof next === 'string' ? next : 'none';
          if (typeof msg[WIRE.usernameField] === 'string') username = msg[WIRE.usernameField];
          storageOk = msg[WIRE.storageOkField] !== false;
          if (phase !== 'showing') wipe();
          if (phase === 'done' || phase === 'none') error = null;
          changed();
          return true;
        }
        if (msg.type === WIRE.secretEvent) {
          var pw = msg[WIRE.passwordField];
          if (typeof pw === 'string' && pw.length) {
            password = pw;
            if (typeof msg[WIRE.usernameField] === 'string') username = msg[WIRE.usernameField];
            phase = 'showing';
            error = null;
            changed();
          }
          return true;
        }
        // The controller could not read its state at user_bootstrap_get: it
        // sent no phase, so the screen would stay blank and unblocked.
        if (msg.type === WIRE.errorEvent && msg.code === RETRY_CODE && isOurs(msg, false) && (phase === 'none' || phase === 'retry')) {
          phase = 'retry';
          wipe();
          error = UL.errorText(msg.code, msg.message);
          scheduleRetry();
          changed();
          return true;
        }
        if (msg.type === WIRE.errorEvent && isOurs(msg, storageOk && BLOCKING[phase] === 1)) {
          error = UL.errorText(msg.code, msg.message);
          changed();
          return true;
        }
        return false;
      },
      // Bridge link dropped: forget the password and the screen; the next live
      // connection asks again (user_bootstrap_get).
      connectionClosed: function () {
        cancelRetry();
        wipe();
        phase = 'none';
        error = null;
        changed();
      },
    };
  }

  function el(doc, tag, cls, text) {
    var e = doc.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  // Overlay DOM. `root` is the #first-boot element from index.html.
  // textFieldId: the Wi-Fi text field, disabled while the overlay is up.
  function createView(doc, rootEl, textFieldId) {
    var body = el(doc, 'div', 'fb-panel');
    rootEl.appendChild(body);

    function stop(evt) {
      evt.stopPropagation();
      if (evt.cancelable) evt.preventDefault();
    }
    SWALLOWED_POINTER.forEach(function (t) { rootEl.addEventListener(t, stop, { capture: false, passive: false }); });

    function render(v, blocking, note) {
      rootEl.className = 'first-boot' + (blocking ? ' blocking' : (note ? ' note' : ' hidden'));
      rootEl.setAttribute('data-phase', v.phase);
      body.innerHTML = '';
      var field = doc.getElementById(textFieldId);
      if (field) field.disabled = blocking;
      if (!blocking && !note) return;
      if (!blocking) {
        body.appendChild(el(doc, 'div', 'fb-text', 'Account storage is not available'));
        return;
      }
      if (v.phase === 'prompt') {
        body.appendChild(el(doc, 'div', 'fb-title', 'Kiosk password not set'));
        body.appendChild(el(doc, 'div', 'fb-text', 'The kiosk user password is unset. Press the M button to generate one.'));
        body.appendChild(el(doc, 'div', 'fb-arrow', '\u2192'));
        body.appendChild(el(doc, 'div', 'fb-arrow-label', 'M'));
      } else if (v.phase === 'retry') {
        body.appendChild(el(doc, 'div', 'fb-title', 'Checking the account state...'));
        body.appendChild(el(doc, 'div', 'fb-text', 'The controller could not read the account state. Trying again.'));
      } else if (v.phase === 'generating') {
        body.appendChild(el(doc, 'div', 'fb-title', 'Generating...'));
      } else if (v.phase === 'showing') {
        body.appendChild(el(doc, 'div', 'fb-title', 'Password for ' + v.username));
        body.appendChild(el(doc, 'div', 'fb-secret', v.password || ''));
        body.appendChild(el(doc, 'div', 'fb-text', 'Save this password now. It will not be shown again.'));
        body.appendChild(el(doc, 'div', 'fb-text', 'Press M again to confirm you saved it'));
      }
      if (v.error) body.appendChild(el(doc, 'div', 'fb-error', v.error));
    }

    return { render: render };
  }

  // Wires model, view and the document-level key swallow. Returns the handle
  // boot.js uses: active(), handle(msg), connectionClosed().
  // opts: see createModel.
  function create(doc, rootEl, textFieldId, opts) {
    var model;
    var view = createView(doc, rootEl, textFieldId);
    model = createModel(function () { view.render(model.view(), model.active(), model.storageNote()); }, opts);

    // Capture on the document so nothing, not even xterm's textarea, sees a
    // key while the overlay is up (raw keys leak in when the evdev grab fails,
    // and a BT keyboard is a separate node).
    SWALLOWED_KEYS.forEach(function (t) {
      doc.addEventListener(t, function (evt) {
        if (!model.active()) return;
        evt.preventDefault();
        evt.stopPropagation();
        evt.stopImmediatePropagation();
      }, true);
    });

    view.render(model.view(), false, false);
    return model;
  }

  var api = { create: create, createModel: createModel, createView: createView, WIRE: WIRE, isOurs: isOurs };

  if (node) module.exports = api;
  else TR.firstBoot = api;
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
