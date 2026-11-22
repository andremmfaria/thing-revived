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

  // cfg: the config values object (utils/config.js `values`).
  function initial(cfg) {
    return {
      connection: { agent: 'connecting', bridge: 'connecting' },
      sessions: { list: [], active: 0, dead: {} },
      clock: { offsetMs: 0, status: 'unsynced' },
      bt: { powered: null, devices: [], paired: null },
      brightness: { value: null, max: null },
      timezone: { current: null, list: [], loaded: false },
      wifi: { available: false, status: null, scanning: false, networks: null, saved: null },
      terminal: { fontSize: cfg.ui.font_size, fontFamilyIndex: 0 },
    };
  }

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

  function reduce(state, a) {
    switch (a.type) {
      case 'connection': {
        var conn = {};
        conn[a.which] = a.status;
        return set(state, 'connection', conn);
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
        return set(state, 'wifi', { networks: null });
      case 'wifi_saved_list':
        return set(state, 'wifi', { saved: a.networks || [] });
      case 'wifi_error':
        return a.context === 'wifi_scan' ? set(state, 'wifi', { scanning: false }) : state;

      case 'local/terminal':
        return set(state, 'terminal', a.patch);
      default:
        return state;
    }
  }

  var api = { initial: initial, reduce: reduce };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.reducers = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
