/*
 * Domain reducer. Actions are the controller/agent wire messages themselves
 * (keyed by their `type`) plus a few `local/*` actions for optimistic UI
 * changes (brightness knob, font settings, session switch). Pure: returns
 * the same state object for actions it does not handle.
 *
 * Navigation consequences of a message (popping screens, toasts) are NOT
 * here; they belong to the interaction chart, which sees the same message.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.reducers).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;
  var Types = node ? require('../utils/types.js') : root.TR.types;

  // cfg: the config values object (utils/config.js `values`).
  function initial(cfg) {
    return {
      connection: { agent: Types.ChannelStatus.connecting, bridge: Types.ChannelStatus.connecting },
      sessions: { list: [], active: 0, dead: {} },
      clock: { offsetMs: 0, status: Types.ClockStatus.unsynced },
      bt: { powered: null, devices: [], paired: null },
      brightness: { value: null, max: null },
      timezone: { current: null, list: [], loaded: false },
      device: deviceFresh(),
      wifi: { available: false, status: null, scanning: false, networks: null, saved: null },
      terminal: { fontSize: cfg.ui.font_size, fontFamilyIndex: 0 },
      // bootstrap never holds the first-boot password: it lives only in
      // firstboot/first-boot.js. The rest is the Phase 2a/2b account menu
      // (see usersFresh); `secret` holds a generated password from
      // user_secret until the menu clears it.
      users: usersFresh(null),
    };
  }

  // Device settings slice (device-settings-plan sections 3.2 and 8.1). live and
  // persisted hold 'host'|'device' for mode and 'on'|'off' for the rest, null
  // until the first device_settings. adbBlocked is 'mode_host' or null, busy is
  // the setting being applied or null. lastError ({seq, context, code, message})
  // stays until the local action local/device_clear_error. unlockDeadlineMs is
  // the display deadline of the admin unlock on the injected clock, null while
  // locked or unknown (display only, the server is authoritative).
  function deviceFresh() {
    return {
      live: { mode: null, ssh: null, adb: null, uart: null },
      persisted: { mode: null, ssh: null, adb: null, uart: null },
      adbBlocked: null, busy: null, loaded: false, rebooting: false,
      lastError: null, unlockDeadlineMs: null,
    };
  }

  function deviceValues(src) {
    var o = src && typeof src === 'object' ? src : {};
    function pick(k) { return typeof o[k] === 'string' ? o[k] : null; }
    return { mode: pick('mode'), ssh: pick('ssh'), adb: pick('adb'), uart: pick('uart') };
  }

  // Account menu state. Everything but `bootstrap` is dropped when the admin
  // session ends, the bridge drops, or the menu section is left.
  function usersFresh(bootstrap) {
    return {
      bootstrap: bootstrap,
      session: { unlocked: false, username: null, expiresInSecs: null },
      list: null, // user_list.users, null until loaded
      groups: null, // group_list.groups
      keys: null, // {name, list} from the last ssh_key_list
      lastResult: null, // {seq, op, name, success, error, message}
      lastError: null, // {seq, context, code, message}, non-bootstrap user_error
      secret: null, // {name, password} from user_secret, shown once
    };
  }

  // True when users holds anything the menu must forget on a bridge drop.
  function sessionState(u) {
    return u.session.unlocked || u.list !== null || u.groups !== null || u.keys !== null || u.secret !== null ||
      u.lastResult !== null || u.lastError !== null;
  }

  function nextSeq(prev) { return prev && typeof prev.seq === 'number' ? prev.seq + 1 : 1; }

  // New state with state[key] replaced by a shallow merge of patch.
  function set(state, key, patch) {
    var next = Object.assign({}, state);
    next[key] = Object.assign({}, state[key], patch);
    return next;
  }

  function without(map, key) {
    var out = Object.assign({}, map);
    delete out[key];
    return out;
  }

  // nowMs: the injected clock reading for this dispatch (see withClock), or
  // undefined when the store has no clock; deadlines are then left null.
  function reduce(state, a, nowMs) {
    switch (a.type) {
      case 'connection': {
        var conn = {};
        conn[a.which] = a.status;
        var withConn = set(state, 'connection', conn);
        // The controller drops the admin session with the connection.
        if (a.which === 'bridge' && a.status !== Types.ChannelStatus.live && sessionState(state.users)) {
          withConn.users = usersFresh(state.users.bootstrap);
        }
        if (a.which === 'bridge' && a.status !== Types.ChannelStatus.live && state.device.unlockDeadlineMs !== null) {
          withConn.device = Object.assign({}, state.device, { unlockDeadlineMs: null });
        }
        return withConn;
      }
      case 'session_list':
        return set(state, 'sessions', { list: a.sessions || [] });
      case 'local/active':
        return set(state, 'sessions', { active: a.index });
      case 'exited':
        if (!a.needs_manual_respawn) return state;
        var dead = Object.assign({}, state.sessions.dead);
        dead[a.session] = true;
        return set(state, 'sessions', { dead: dead });
      case 'local/clearDead':
        return set(state, 'sessions', { dead: without(state.sessions.dead, a.index) });
      case 'clock_sync':
        return set(state, 'clock', { offsetMs: a.offset_ms, status: a.status });

      case 'bt_power':
        return set(state, 'bt', { powered: a.powered });
      case 'bt_scan_started':
      case 'local/bt_devices_reset':
        return set(state, 'bt', { devices: [] });
      case 'bt_device_found': {
        var exists = state.bt.devices.some(function (d) { return d.address === a.address; });
        if (exists) return state;
        return set(state, 'bt', { devices: state.bt.devices.concat([{ address: a.address, name: a.name }]) });
      }
      case 'bt_paired_list':
        return set(state, 'bt', { paired: a.devices || [] });

      case 'brightness':
        return set(state, 'brightness', { value: a.value, max: a.max });
      case 'local/brightness':
        return set(state, 'brightness', { value: a.value });
      case 'timezone':
        return set(state, 'timezone', { current: a.value });
      case 'timezone_list':
        return set(state, 'timezone', { list: a.zones || [], loaded: true });

      case 'device_settings':
        return set(state, 'device', {
          live: deviceValues(a),
          persisted: deviceValues(a.persisted),
          adbBlocked: typeof a.adb_blocked === 'string' ? a.adb_blocked : null,
          busy: typeof a.busy === 'string' ? a.busy : null,
          loaded: true,
        });
      case 'device_settings_error':
        return set(state, 'device', {
          lastError: { seq: nextSeq(state.device.lastError), context: a.context, code: a.code, message: a.message || null },
        });
      case 'device_rebooting':
        return set(state, 'device', { rebooting: true });
      case 'local/device_clear_error':
        return state.device.lastError === null ? state : set(state, 'device', { lastError: null });

      case 'wifi_available':
        if (a.available) return set(state, 'wifi', { available: true });
        // Adapter unplugged: drop live state.
        return set(state, 'wifi', { available: false, status: null, networks: null, saved: null, scanning: false });
      case 'wifi_status':
        return set(state, 'wifi', { status: a });
      case 'wifi_scan_started':
        return set(state, 'wifi', { scanning: true });
      case 'wifi_scan_results':
        return set(state, 'wifi', { scanning: false, networks: a.networks || [] });
      case 'local/wifi_networks_reset':
        // Entering the list sends wifi_scan in the same step, so the list is
        // already "scanning" before wifi_scan_started arrives.
        return set(state, 'wifi', { networks: null, scanning: true });
      case 'wifi_saved_list':
        return set(state, 'wifi', { saved: a.networks || [] });
      case 'wifi_error':
        return a.context === 'wifi_scan' ? set(state, 'wifi', { scanning: false }) : state;

      case 'user_bootstrap':
        // Phase only; the password is not a store value (see first-boot.js).
        return set(state, 'users', {
          bootstrap: {
            phase: typeof a.phase === 'string' ? a.phase : 'none',
            username: a.username,
            storage_ok: a.storage_ok !== false,
          },
        });

      case 'user_session':
        if (a.unlocked === true) {
          var unlocked = set(state, 'users', {
            session: { unlocked: true, username: a.username === undefined ? null : a.username, expiresInSecs: typeof a.expires_in_secs === 'number' ? a.expires_in_secs : null },
          });
          // A fresh unlock restarts the fixed TTL; events without
          // expires_in_secs (or a store without a clock) leave the deadline unknown.
          unlocked.device = Object.assign({}, state.device, {
            unlockDeadlineMs: typeof a.expires_in_secs === 'number' && typeof nowMs === 'number' ? nowMs + a.expires_in_secs * 1000 : null,
          });
          return unlocked;
        }
        return Object.assign({}, state, { users: usersFresh(state.users.bootstrap), device: state.device.unlockDeadlineMs === null ? state.device : Object.assign({}, state.device, { unlockDeadlineMs: null }) });
      case 'user_list':
        return set(state, 'users', { list: a.users || [] });
      case 'group_list':
        return set(state, 'users', { groups: a.groups || [] });
      case 'ssh_key_list':
        return set(state, 'users', { keys: { name: a.name, list: a.keys || [] } });
      case 'user_result':
        return set(state, 'users', {
          lastResult: { seq: nextSeq(state.users.lastResult), op: a.op, name: a.name, success: a.success === true, error: a.error || null, message: a.message || null },
        });
      case 'user_secret':
        return set(state, 'users', { secret: { name: a.name, password: a.password } });
      case 'user_error': {
        // First-boot errors belong to firstboot/first-boot.js.
        if (typeof a.context === 'string' && a.context.indexOf('user_bootstrap') === 0) return state;
        var err = { seq: nextSeq(state.users.lastError), context: a.context, code: a.code, message: a.message || null };
        // The controller dropped (or never had) our admin session.
        if (a.code === 'not_unlocked') {
          return Object.assign({}, state, {
            users: Object.assign(usersFresh(state.users.bootstrap), { lastError: err }),
            device: state.device.unlockDeadlineMs === null ? state.device : Object.assign({}, state.device, { unlockDeadlineMs: null }),
          });
        }
        return set(state, 'users', { lastError: err });
      }
      case 'local/users_clear_secret':
        return state.users.secret === null ? state : set(state, 'users', { secret: null });
      case 'local/users_leave':
        // Leaving the section: forget what must not outlive the screens.
        return set(state, 'users', { secret: null, lastResult: null, lastError: null, keys: null });

      case 'local/terminal':
        return set(state, 'terminal', a.patch);
      default:
        return state;
    }
  }

  // A reducer bound to an injected clock (anything with now() -> ms, such as
  // the timers object from core/timers.js): Store.create(Reducers.withClock(timers), ...).
  function withClock(clock) {
    return function (state, a) { return reduce(state, a, clock.now()); };
  }

  var api = { initial: initial, reduce: reduce, withClock: withClock };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.reducers = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
