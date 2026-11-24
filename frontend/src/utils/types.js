/*
 * Central catalogue of the frontend's types: JSDoc typedefs for the shapes
 * that cross module boundaries, plus frozen runtime enums for every string
 * literal more than one module has to agree on. Module-private helper types
 * stay in the module that owns them.
 *
 * Loaded right after config.js and logging.js and before everything else.
 * logging.js loads first and keeps its own private level list (it cannot
 * depend on this file); types.test.js asserts the two lists are identical.
 *
 * Wire protocol: bridge (controller) and agent (terminal) message type names
 * match the fixtures under config/protocol/controller and config/protocol/terminal one to
 * one; types.test.js enforces it both ways.
 *
 * Plain ES2018 subset, classic script, dual-load (browser: TR.utils.types is
 * also reachable as TR.types, node: module.exports).
 */
(function (root) {
  'use strict';

  var node = typeof module !== 'undefined' && module.exports;

  // Frozen identity map: enumOf(['a','b']) -> {a:'a', b:'b'}. Values are
  // unique by construction; keys are the wire/state strings themselves.
  function enumOf(list) {
    var o = {};
    list.forEach(function (v) { o[v] = v; });
    return Object.freeze(o);
  }
  function frozen(o) { return Object.freeze(o); }
  function values(e) { return Object.keys(e).map(function (k) { return e[k]; }); }
  function has(e, v) { return typeof v === 'string' && Object.prototype.hasOwnProperty.call(e, v); }

  // ---- interaction ------------------------------------------------------------------

  /**
   * Interaction chart state (state.mode); dotted paths are parent.child.
   * @enum {string}
   */
  var Mode = frozen({
    TERMINAL: 'terminal',
    LIST: 'open.list',
    ADJUSTING: 'open.adjusting',
    EDITING_TEXT: 'open.editingText',
    SWALLOW_ENTER: 'open.swallowEnter',
    PICKER: 'open.picker',
    PAIRING_CONFIRM: 'open.pairing.confirm',
    PAIRING_PASSKEY: 'open.pairing.passkey',
  });

  /** ESC region of the chart. @enum {string} */
  var EscState = enumOf(['up', 'armed']);

  /**
   * Event kinds the interaction machine reacts to (interaction/events.js).
   * Machine tables additionally use the internal kinds back, close, select.
   * @enum {string}
   */
  var EventKind = enumOf([
    'knob', 'button', 'timerFired', 'wire', 'domain', 'rowTap', 'customTap',
    'textInput', 'textEnter', 'textBlur', 'closeClick', 'sendFailed',
  ]);

  /**
   * Effect kinds the interpreter executes (interaction/effects.js). An effect
   * is an object with exactly one of these keys.
   * @enum {string}
   */
  var EffectKind = enumOf([
    'send', 'dispatch', 'startTimer', 'cancelTimer', 'scroll', 'switchSession',
    'term', 'textBegin', 'textEnd', 'textReset',
  ]);

  /** Row kinds a screen may declare (interaction/menu-chart.js ROW_KINDS). @enum {string} */
  var RowKind = enumOf(['label', 'toggle', 'range', 'nav', 'nav-dynamic', 'field', 'action']);

  /** What an action row does after its descriptors ran. @enum {string} */
  var AfterSelect = enumOf(['root', 'pop', 'none']);

  // ---- input ------------------------------------------------------------------------

  /** Linux KEY_* codes delivered by the controller `button` event. @enum {number} */
  var ButtonCode = frozen({ ESC: 1, KEY_1: 2, KEY_2: 3, KEY_3: 4, KEY_4: 5, ENTER: 28 });

  /** evdev value of a button event. @enum {number} */
  var ButtonValue = frozen({ UP: 0, DOWN: 1, REPEAT: 2 });

  /** Phase derived from ButtonValue. @enum {string} */
  var ButtonPhase = enumOf(['down', 'up', 'repeat']);

  // ---- transport --------------------------------------------------------------------

  /** Channel connection status. @enum {string} */
  var ChannelStatus = enumOf(['connecting', 'live', 'error', 'disconnected']);

  /** Result of Channel.send. @enum {string} */
  var SendResult = enumOf(['sent', 'queued', 'dropped']);

  /** Which transport a store `connection` action refers to. @enum {string} */
  var ChannelName = enumOf(['agent', 'bridge']);

  /** Clock sync status reported by the terminal agent. @enum {string} */
  var ClockStatus = enumOf(['ok', 'stale', 'unsynced']);

  /** Terminal control actions (client -> agent `control`). @enum {string} */
  var ControlAction = enumOf(['clear', 'interrupt', 'respawn']);

  // ---- logging ----------------------------------------------------------------------

  /** Log levels, most to least severe. @enum {string} */
  var LogLevel = enumOf(['error', 'warn', 'info', 'debug', 'trace']);

  // ---- wire message type names ------------------------------------------------------

  /** Controller -> client (settings events plus raw knob/button). @enum {string} */
  var BridgeEvent = enumOf([
    'knob', 'button',
    'bt_power', 'bt_error', 'bt_scan_started', 'bt_scan_stopped', 'bt_device_found', 'bt_paired_list',
    'bt_pairing_confirm_request', 'bt_pairing_passkey_display', 'bt_pair_result',
    'brightness', 'timezone', 'timezone_list', 'settings_error',
    'usb_role', 'usb_role_error', 'usb_role_rebooting',
    'debug_state', 'debug_error',
    'device_settings', 'device_settings_error', 'device_rebooting',
    'wifi_available', 'wifi_status', 'wifi_scan_started', 'wifi_scan_results', 'wifi_connecting',
    'wifi_connect_result', 'wifi_saved_list', 'wifi_error',
    'user_bootstrap', 'user_bootstrap_secret', 'user_error',
    'user_session', 'user_list', 'user_result', 'user_secret', 'group_list', 'ssh_key_list',
  ]);

  /** Client -> controller commands. @enum {string} */
  var BridgeCommand = enumOf([
    'bt_power_get', 'bt_set_power', 'bt_scan_start', 'bt_scan_stop', 'bt_paired_list_get', 'bt_pair',
    'bt_pairing_confirm', 'bt_connect', 'bt_disconnect', 'bt_forget',
    'brightness_get', 'brightness_set', 'timezone_get', 'timezone_list_get', 'timezone_set',
    'usb_role_get', 'usb_role_set', 'usb_role_reboot',
    'debug_get', 'debug_set',
    'device_settings_get', 'device_settings_set', 'device_reboot',
    'wifi_get', 'wifi_saved_list_get', 'wifi_scan', 'wifi_connect', 'wifi_connect_saved',
    'wifi_disconnect', 'wifi_forget',
    'user_bootstrap_get',
    'user_unlock', 'user_lock_session', 'user_list_get', 'user_add', 'user_remove', 'user_set_locked',
    'user_set_password', 'user_group_add', 'user_group_remove',
    'group_list_get', 'group_add', 'group_remove',
    'ssh_key_list_get', 'ssh_key_add', 'ssh_key_remove',
  ]);

  /**
   * Error codes the controller may put in `user_error.code` and
   * `user_result.error` (config/protocol/controller/README.md). The first-boot
   * screen knows the first three; any other code falls back to the
   * controller's message.
   * @enum {string}
   */
  var UserErrorCode = enumOf([
    'generate_failed', 'ack_failed', 'storage_unavailable',
    'not_unlocked', 'bad_password', 'rate_limited', 'invalid_name', 'invalid_password', 'invalid_key',
    'exists', 'leftover_state', 'not_found', 'system_account', 'protected_account', 'last_admin',
    'group_not_assignable', 'too_many', 'confirm_required', 'no_password', 'sessions_remain', 'user_busy',
    'internal',
  ]);

  /**
   * Phases of `user_bootstrap.phase` (delta plan section 5.6). The first-boot
   * overlay is up (and blocks all input) in prompt, generating and showing.
   * @enum {string}
   */
  var FirstBootPhase = enumOf(['none', 'prompt', 'generating', 'showing', 'done']);

  /**
   * Codes of `device_settings_error.code` (device-settings-plan section 3.2).
   * Unknown codes fall back to the controller's message.
   * @enum {string}
   */
  var DeviceErrorCode = enumOf([
    'not_unlocked', 'invalid_setting', 'invalid_value', 'adb_blocked_host', 'busy',
    'rate_limited', 'persist_failed', 'apply_failed', 'unavailable',
  ]);

  /** Terminal agent -> client. @enum {string} */
  var AgentEvent = enumOf(['session_list', 'output', 'exited', 'clock_sync']);

  /** Client -> terminal agent. @enum {string} */
  var AgentCommand = enumOf(['input', 'resize', 'control']);

  /**
   * Wire type names that exist only on the client side and have no protocol
   * fixture. Empty: every constant above has a fixture.
   * @type {string[]}
   */
  var CLIENT_ONLY_WIRE_TYPES = frozen([]);

  /**
   * Store action types that are not wire messages: connection status and
   * optimistic local updates (state/reducers.js).
   * @enum {string}
   */
  var LocalAction = enumOf([
    'connection', 'local/active', 'local/clearDead', 'local/brightness', 'local/terminal',
    'local/users_clear_secret', 'local/users_leave', 'local/debug_clear_error', 'local/device_clear_error',
  ]);

  // ---- frame builders (pure) --------------------------------------------------------

  /** Client -> controller frame builders, one per BridgeCommand. */
  var bridgeCommands = frozen({
    btPowerGet: function () { return { type: BridgeCommand.bt_power_get }; },
    btSetPower: function (powered) { return { type: BridgeCommand.bt_set_power, powered: powered }; },
    btScanStart: function () { return { type: BridgeCommand.bt_scan_start }; },
    btScanStop: function () { return { type: BridgeCommand.bt_scan_stop }; },
    btPairedListGet: function () { return { type: BridgeCommand.bt_paired_list_get }; },
    btPair: function (address) { return { type: BridgeCommand.bt_pair, address: address }; },
    btPairingConfirm: function (accept) { return { type: BridgeCommand.bt_pairing_confirm, accept: accept }; },
    btConnect: function (address) { return { type: BridgeCommand.bt_connect, address: address }; },
    btDisconnect: function (address) { return { type: BridgeCommand.bt_disconnect, address: address }; },
    btForget: function (address) { return { type: BridgeCommand.bt_forget, address: address }; },
    brightnessGet: function () { return { type: BridgeCommand.brightness_get }; },
    brightnessSet: function (value) { return { type: BridgeCommand.brightness_set, value: value }; },
    timezoneGet: function () { return { type: BridgeCommand.timezone_get }; },
    timezoneListGet: function () { return { type: BridgeCommand.timezone_list_get }; },
    timezoneSet: function (value) { return { type: BridgeCommand.timezone_set, value: value }; },
    usbRoleGet: function () { return { type: BridgeCommand.usb_role_get }; },
    // role is 'host' or 'device'; persisted for the next boot, never a live switch.
    usbRoleSet: function (role) { return { type: BridgeCommand.usb_role_set, role: role }; },
    usbRoleReboot: function () { return { type: BridgeCommand.usb_role_reboot }; },
    debugGet: function () { return { type: BridgeCommand.debug_get }; },
    // surface is 'adb' or 'uart'; confirm is the fixed string the hold gesture sends, never typed.
    debugSet: function (surface, enabled, confirm) {
      return { type: BridgeCommand.debug_set, surface: surface, enabled: enabled === true, confirm: confirm || '' };
    },
    deviceSettingsGet: function () { return { type: BridgeCommand.device_settings_get }; },
    // setting is 'mode' | 'ssh' | 'adb' | 'uart'; value is 'host' | 'device' for mode, 'on' | 'off' otherwise.
    // No confirm field: the hold gesture is a frontend-only concern.
    deviceSettingsSet: function (setting, value) {
      return { type: BridgeCommand.device_settings_set, setting: setting, value: value };
    },
    deviceReboot: function () { return { type: BridgeCommand.device_reboot }; },
    wifiGet: function () { return { type: BridgeCommand.wifi_get }; },
    wifiSavedListGet: function () { return { type: BridgeCommand.wifi_saved_list_get }; },
    wifiScan: function () { return { type: BridgeCommand.wifi_scan }; },
    wifiConnect: function (ssid, password) { return { type: BridgeCommand.wifi_connect, ssid: ssid, password: password }; },
    wifiConnectSaved: function (ssid) { return { type: BridgeCommand.wifi_connect_saved, ssid: ssid }; },
    wifiDisconnect: function () { return { type: BridgeCommand.wifi_disconnect }; },
    wifiForget: function (ssid) { return { type: BridgeCommand.wifi_forget, ssid: ssid }; },
    userBootstrapGet: function () { return { type: BridgeCommand.user_bootstrap_get }; },
    // Phase 2a. An empty password in userAdd / userSetPassword means "generate".
    // confirm echoes the target (name or group) for destructive requests.
    userUnlock: function (username, password) { return { type: BridgeCommand.user_unlock, username: username, password: password }; },
    userLockSession: function () { return { type: BridgeCommand.user_lock_session }; },
    userListGet: function () { return { type: BridgeCommand.user_list_get }; },
    userAdd: function (name, password, admin) {
      return { type: BridgeCommand.user_add, name: name, password: password || '', admin: admin === true };
    },
    userRemove: function (name, keepHome, confirm) {
      return { type: BridgeCommand.user_remove, name: name, keep_home: keepHome === true, confirm: confirm || '' };
    },
    userSetLocked: function (name, locked, confirm) {
      return { type: BridgeCommand.user_set_locked, name: name, locked: locked === true, confirm: confirm || '' };
    },
    userSetPassword: function (name, password, currentPassword, confirm) {
      return { type: BridgeCommand.user_set_password, name: name, password: password || '', current_password: currentPassword || '', confirm: confirm || '' };
    },
    // Phase 2b.
    userGroupAdd: function (name, group, confirm) {
      return { type: BridgeCommand.user_group_add, name: name, group: group, confirm: confirm || '' };
    },
    userGroupRemove: function (name, group) { return { type: BridgeCommand.user_group_remove, name: name, group: group }; },
    groupListGet: function () { return { type: BridgeCommand.group_list_get }; },
    groupAdd: function (name) { return { type: BridgeCommand.group_add, name: name }; },
    groupRemove: function (name, confirm) { return { type: BridgeCommand.group_remove, name: name, confirm: confirm || '' }; },
    sshKeyListGet: function (name) { return { type: BridgeCommand.ssh_key_list_get, name: name }; },
    sshKeyAdd: function (name, key) { return { type: BridgeCommand.ssh_key_add, name: name, key: key }; },
    sshKeyRemove: function (name, fingerprint, confirm) {
      return { type: BridgeCommand.ssh_key_remove, name: name, fingerprint: fingerprint, confirm: confirm || '' };
    },
  });

  /** Client -> agent frame builders, one per AgentCommand. */
  var agentCommands = frozen({
    input: function (session, data) { return { type: AgentCommand.input, session: session, data: data }; },
    resize: function (session, cols, rows) { return { type: AgentCommand.resize, session: session, cols: cols, rows: rows }; },
    control: function (session, action) { return { type: AgentCommand.control, session: session, action: action }; },
  });

  // ---- shapes (documentation; guards below check the cheap parts) -------------------

  /**
   * A controller or agent frame: a JSON object with a string `type`.
   * @typedef {{type: string}} WireMessage
   */

  /**
   * A command descriptor or effect: one key from EffectKind with its payload.
   * @typedef {Object} Effect
   */

  /**
   * One menu row. `activate` is a list of descriptors (effects or navigation
   * commands, see menu-chart.js); `change(v)` returns descriptors for a range.
   * @typedef {Object} Row
   * @property {string} id
   * @property {string} label
   * @property {string} kind one of RowKind
   * @property {string=} target screen id for nav rows
   * @property {Array<Object>=} activate
   * @property {string=} afterSelect one of AfterSelect
   * @property {number=} value range rows
   * @property {number=} min
   * @property {number=} max
   * @property {number=} step
   * @property {string=} display
   * @property {function(number): Array<Object>=} change
   */

  /**
   * A screen definition (menu/screens/*.js): data, no DOM.
   * @typedef {Object} Screen
   * @property {string} title
   * @property {function(Object): Row[]} rows ctx -> rows
   * @property {string=} mode a Mode value, default Mode.LIST
   * @property {string=} hint per-screen footer hint
   * @property {Object=} textField text entry description
   */

  /**
   * Interaction chart state.
   * @typedef {Object} InteractionState
   * @property {string} mode one of Mode
   * @property {string} esc one of EscState
   * @property {string[]} stack screen ids, root first
   * @property {Object<string, number>} highlight per screen
   * @property {?Object} toast
   * @property {Object} ui per-screen UI state
   * @property {Object} domain domain store snapshot
   */

  /**
   * One terminal session in the agent's session_list.
   * @typedef {Object} Session
   * @property {number} id
   * @property {string} name
   * @property {boolean=} alive
   */

  /**
   * Channel instance (transport/channel.js).
   * @typedef {Object} Channel
   * @property {function(): void} start
   * @property {function(Object, {queue?: boolean, quiet?: boolean}=): string} send returns a SendResult
   * @property {function(): void} close
   * @property {function(function(WireMessage): void): void} onMessage
   * @property {function(function(string): void): void} onStatus
   * @property {function(function(WireMessage): void): void} onDrop
   * @property {function(): string} status a ChannelStatus
   */

  /**
   * Typed client over a Channel (agent-client.js, bridge-client.js).
   * @typedef {Object} Client
   * @property {function(Object): string=} send bridge: returns a SendResult
   * @property {function(): void=} requestInitialState bridge
   * @property {function(number, string): string=} sendInput agent
   * @property {function(number, number, number): string=} sendResize agent
   * @property {function(number, string): string=} sendControl agent
   */

  /**
   * Callback taking a decoded app event.
   * @callback EventSink
   * @param {Object} event
   * @returns {void}
   */

  // ---- guards -----------------------------------------------------------------------

  function isObject(v) { return v !== null && typeof v === 'object' && !Array.isArray(v); }

  function isWireMessage(v) { return isObject(v) && typeof v.type === 'string'; }
  function isBridgeEvent(v) { return isWireMessage(v) && has(BridgeEvent, v.type); }
  function isBridgeCommand(v) { return isWireMessage(v) && has(BridgeCommand, v.type); }
  function isAgentEvent(v) { return isWireMessage(v) && has(AgentEvent, v.type); }
  function isAgentCommand(v) { return isWireMessage(v) && has(AgentCommand, v.type); }

  // An effect has exactly one key and it is a known kind.
  function isEffect(v) {
    if (!isObject(v)) return false;
    var keys = Object.keys(v);
    return keys.length === 1 && has(EffectKind, keys[0]);
  }

  function isRow(v) {
    if (!isObject(v) || typeof v.id !== 'string' || typeof v.label !== 'string' || !has(RowKind, v.kind)) return false;
    if ((v.kind === 'nav' || v.kind === 'nav-dynamic') && typeof v.target !== 'string') return false;
    if (v.kind === 'range' && (typeof v.min !== 'number' || typeof v.max !== 'number' || typeof v.step !== 'number' || typeof v.change !== 'function')) return false;
    if (v.afterSelect !== undefined && !has(AfterSelect, v.afterSelect)) return false;
    return true;
  }

  function isScreen(v) {
    if (!isObject(v) || typeof v.title !== 'string' || typeof v.rows !== 'function') return false;
    return v.mode === undefined || (typeof v.mode === 'string' && values(Mode).indexOf(v.mode) !== -1);
  }

  function isEvent(v) { return isObject(v) && has(EventKind, v.type); }
  function isSession(v) { return isObject(v) && typeof v.id === 'number' && typeof v.name === 'string'; }
  function isChannelStatus(v) { return has(ChannelStatus, v); }
  function isSendResult(v) { return has(SendResult, v); }

  var api = {
    // enums
    Mode: Mode, EscState: EscState, EventKind: EventKind, EffectKind: EffectKind, RowKind: RowKind,
    AfterSelect: AfterSelect, ButtonCode: ButtonCode, ButtonValue: ButtonValue, ButtonPhase: ButtonPhase,
    ChannelStatus: ChannelStatus, SendResult: SendResult, ChannelName: ChannelName, ClockStatus: ClockStatus,
    ControlAction: ControlAction, UserErrorCode: UserErrorCode, DeviceErrorCode: DeviceErrorCode, FirstBootPhase: FirstBootPhase, LogLevel: LogLevel, LocalAction: LocalAction,
    BridgeEvent: BridgeEvent, BridgeCommand: BridgeCommand, AgentEvent: AgentEvent, AgentCommand: AgentCommand,
    CLIENT_ONLY_WIRE_TYPES: CLIENT_ONLY_WIRE_TYPES,
    // builders
    bridgeCommands: bridgeCommands, agentCommands: agentCommands,
    // helpers
    enumOf: enumOf, values: values, has: has,
    // guards
    isWireMessage: isWireMessage, isBridgeEvent: isBridgeEvent, isBridgeCommand: isBridgeCommand,
    isAgentEvent: isAgentEvent, isAgentCommand: isAgentCommand, isEffect: isEffect, isRow: isRow,
    isScreen: isScreen, isEvent: isEvent, isSession: isSession, isChannelStatus: isChannelStatus,
    isSendResult: isSendResult,
  };

  if (node) {
    module.exports = api;
  } else {
    root.TR = root.TR || {};
    root.TR.utils = root.TR.utils || {};
    root.TR.utils.types = api;
    root.TR.types = api;
  }
})(typeof window !== 'undefined' ? window : (typeof global !== 'undefined' ? global : this));
