/*
 * First-boot admin password screens (docs/research/user-management-plan.md
 * sections 5 and 10): the blocking reveal screen and the knob picker that asks
 * for the last four characters. All first-boot knowledge lives here and in
 * users/users-logic.js; the generic menu code only knows `custom`, `onKnob`,
 * `onSelect` and the timer table.
 *
 * The one-time password is held only in ui.users.secret. It is cleared on
 * acknowledgement, on leaving the screen, when the bridge connection drops,
 * on expiry and on a hide request. The domain store, the log and
 * localStorage never see it, and a page reload starts empty.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.screensUsers).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var TR = node ? null : (root.TR = root.TR || {});
  var Types = node ? require('../../utils/types.js') : TR.types;

  var SCREEN = 'users-bootstrap';
  var ACK_SCREEN = 'users-ack';
  var TICK = 'users-tick';

  function build(deps) {
    var WL = deps.WL;
    var UL = deps.UL;
    var Cmd = deps.Cmd;

    function inFlow(ctx) {
      return ctx.stack.indexOf(SCREEN) !== -1;
    }

    // Forget the secret and everything typed to confirm it.
    function wipe() {
      return [
        { ui: { users: { secret: null, secretLeft: 0, ack: '', ackIdx: WL.PICKER_START } } },
        { cancelTimer: TICK },
      ];
    }

    function sendAck(ctx) {
      var u = ctx.ui.users;
      if (u.ack.length !== UL.ACK_LEN) return [{ toast: 'Pick all ' + UL.ACK_LEN + ' last characters first' }];
      return [{ ui: { users: { error: null } } }, { send: Cmd.userBootstrapAck(u.ack) }];
    }

    function ackSelect(ctx) {
      var u = ctx.ui.users;
      var r = UL.ackApply(u.ack, u.ackIdx);
      var cmds = [{ ui: { users: { ack: r.entry } } }];
      if (r.done) cmds = cmds.concat(sendAck({ ui: { users: { ack: r.entry } } }));
      return cmds;
    }

    var defs = {};

    defs[SCREEN] = {
      title: 'First boot: admin password',
      custom: 'users-bootstrap',
      hint: 'knob push: select \u00b7 esc (hold): back (hides the password)',
      onEnter: function () {
        return [{ ui: { users: { error: null, dismissed: false } } }, { send: Cmd.userBootstrapGet() }];
      },
      // Leaving, for any reason, wipes the secret from state; the next render
      // of the overlay replaces the DOM that showed it.
      onLeave: function () {
        return wipe().concat([{ ui: { users: { dismissed: true } } }]);
      },
      rows: function (ctx) {
        var u = ctx.ui.users;
        var b = ctx.domain.users.bootstrap;
        var rows = [];
        if (u.error) rows.push({ id: 'users-error', label: u.error, kind: 'label' });
        if (u.secret) {
          rows.push({ id: 'users-expires', label: 'Shown once. Expires in ' + UL.countdown(u.secretLeft), kind: 'label' });
          rows.push({ id: 'users-saved', label: 'I have saved it', kind: 'nav', target: ACK_SCREEN, enter: [{ ui: { users: { ack: '', ackIdx: WL.PICKER_START, error: null } } }] });
          rows.push({ id: 'users-hide', label: 'Hide password', kind: 'action', afterSelect: 'none', activate: wipe() });
          return rows;
        }
        var phase = UL.phase(b);
        if (b && !b.storage_ok) rows.push({ id: 'users-storage', label: 'Account storage is not available', kind: 'label' });
        if (phase === 'pending') {
          rows.push({
            id: 'users-reveal', label: 'Press the knob to reveal the password', kind: 'action', afterSelect: 'none',
            activate: [{ ui: { users: { error: null } } }, { send: Cmd.userBootstrapReveal() }],
          });
        } else if (phase === 'expired') {
          rows.push({ id: 'users-expired-1', label: 'Admin password was not collected.', kind: 'label' });
          rows.push({ id: 'users-expired-2', label: 'Reboot to generate a new one.', kind: 'label' });
        } else if (phase === 'done') {
          rows.push({ id: 'users-done', label: 'Admin password already confirmed', kind: 'label' });
        } else {
          rows.push({ id: 'users-wait', label: 'Waiting for the controller...', kind: 'label' });
        }
        return rows;
      },
    };

    defs[ACK_SCREEN] = {
      title: 'Confirm: last 4 characters',
      mode: Types.Mode.PICKER,
      hint: 'knob: choose \u00b7 esc: add / apply \u00b7 esc (hold): back to the password',
      custom: 'users-ack',
      onEnter: function () { return [{ ui: { users: { ackIdx: WL.PICKER_START } } }]; },
      onLeave: function () { return [{ ui: { users: { ack: '', ackIdx: WL.PICKER_START } } }]; },
      onKnob: function (ctx, delta) { return [{ ui: { users: { ackIdx: WL.pickerMove(ctx.ui.users.ackIdx, delta) } } }]; },
      onSelect: ackSelect,
      onCustomTap: function (ctx, tap) {
        return tap.offset === 0 ? ackSelect(ctx) : [{ ui: { users: { ackIdx: tap.index } } }];
      },
    };

    // A pending secret opens the blocking screen on its own, unless the user
    // already left it in this page life (the root menu keeps a row to return).
    function onBootstrap(ctx, msg) {
      if (msg.acknowledged) {
        var cmds = wipe();
        if (inFlow(ctx)) cmds.push({ popToRoot: true }, { toast: 'Admin password confirmed' });
        return cmds;
      }
      if (msg.pending) {
        if (inFlow(ctx) || ctx.ui.users.dismissed) return [];
        return [{ text: 'reset' }, { popToRoot: true }, { push: SCREEN }, { open: true }];
      }
      // Expired or never collected: nothing to show any more.
      return wipe();
    }

    var reactions = {
      user_bootstrap: onBootstrap,
      user_bootstrap_secret: function (ctx, msg) {
        // Targeted reply to our reveal. Ignore one that arrives after the user left.
        if (!inFlow(ctx)) return [];
        return [
          { ui: { users: { secret: msg.password, secretLeft: msg.expires_in_secs, error: null, ack: '', ackIdx: WL.PICKER_START } } },
          { highlight: { 'users-bootstrap': 1 } },
          { cancelTimer: TICK },
          { startTimer: { name: TICK, ms: UL.TICK_MS } },
        ];
      },
      user_error: function (ctx, msg) {
        var text = UL.errorText(msg.code, msg.message);
        if (typeof msg.context !== 'string' || msg.context.indexOf('user_bootstrap') !== 0) return [{ toast: text }];
        var cmds = [{ ui: { users: { error: text } } }, { toast: text }];
        // Wrong characters: forget the entry, keep the secret and the screen.
        if (msg.code === 'bad_ack') cmds.push({ ui: { users: { ack: '' } } });
        return cmds;
      },
      // The secret is only valid for the connection it was revealed on.
      connection: function (ctx, msg) {
        if (msg.which !== 'bridge' || msg.status === Types.ChannelStatus.live) return [];
        var cmds = wipe();
        if (ctx.top === ACK_SCREEN) cmds.push({ popTo: SCREEN });
        return cmds;
      },
    };

    var timers = {};
    timers[TICK] = function (ctx) {
      var u = ctx.ui.users;
      if (!u.secret) return [];
      var left = u.secretLeft - 1;
      if (left > 0) return [{ ui: { users: { secretLeft: left } } }, { startTimer: { name: TICK, ms: UL.TICK_MS } }];
      // Out of time: hide the secret and ask the controller where it stands.
      var cmds = wipe().concat([{ send: Cmd.userBootstrapGet() }, { toast: 'Time ran out, password hidden' }]);
      if (ctx.top === ACK_SCREEN) cmds.push({ popTo: SCREEN });
      return cmds;
    };

    return {
      defs: defs,
      reactions: reactions,
      timers: timers,
      initialUi: function () {
        return { users: { secret: null, secretLeft: 0, ack: '', ackIdx: WL.PICKER_START, error: null, dismissed: false } };
      },
    };
  }

  var api = { build: build, SCREEN: SCREEN, ACK_SCREEN: ACK_SCREEN, TICK: TICK };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.screensUsers = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
