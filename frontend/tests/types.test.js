'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const Types = require('../src/utils/types.js');
const Logging = require('../src/utils/logging.js');
const Effects = require('../src/interaction/effects.js');
const Bridge = require('../src/transport/bridge-client.js');
const Agent = require('../src/transport/agent-client.js');
const Events = require('../src/interaction/events.js');
const Buttons = require('../src/input/buttons.js');

const PROTOCOL = path.resolve(__dirname, '..', '..', 'protocol');
const ENUMS = ['Mode', 'EscState', 'EventKind', 'EffectKind', 'RowKind', 'AfterSelect', 'ButtonCode', 'ButtonValue',
  'ButtonPhase', 'ChannelStatus', 'SendResult', 'ChannelName', 'ClockStatus', 'ControlAction', 'LogLevel',
  'LocalAction', 'BridgeEvent', 'BridgeCommand', 'AgentEvent', 'AgentCommand'];

test('every enum object is frozen and its values are unique', () => {
  ENUMS.forEach((name) => {
    const e = Types[name];
    assert.ok(e, name + ' exported');
    assert.ok(Object.isFrozen(e), name + ' frozen');
    const vals = Object.keys(e).map((k) => e[k]);
    assert.equal(new Set(vals).size, vals.length, name + ' values unique');
    assert.ok(vals.length > 0, name + ' not empty');
  });
  assert.ok(Object.isFrozen(Types.bridgeCommands) && Object.isFrozen(Types.agentCommands) && Object.isFrozen(Types.CLIENT_ONLY_WIRE_TYPES));
});

test('wire enums do not overlap in a way that would make a type ambiguous within one direction', () => {
  const all = Object.keys(Types.BridgeEvent).concat(Object.keys(Types.BridgeCommand));
  assert.equal(new Set(all).size, all.length);
});

function typesIn(dir) {
  const d = path.join(PROTOCOL, dir);
  return fs.readdirSync(d).filter((f) => f.endsWith('.json')).map((f) => ({ file: f, msg: JSON.parse(fs.readFileSync(path.join(d, f), 'utf8')) }));
}

test('every fixture type is a catalogued bridge/agent type, and every catalogued type has a fixture', () => {
  const ctl = typesIn('controller');
  const term = typesIn('terminal');
  const bridgeAll = new Set(Object.keys(Types.BridgeEvent).concat(Object.keys(Types.BridgeCommand)));
  const agentAll = new Set(Object.keys(Types.AgentEvent).concat(Object.keys(Types.AgentCommand)));
  ctl.forEach((f) => assert.ok(bridgeAll.has(f.msg.type), f.file + ': ' + f.msg.type + ' not in BridgeEvent/BridgeCommand'));
  term.forEach((f) => assert.ok(agentAll.has(f.msg.type), f.file + ': ' + f.msg.type + ' not in AgentEvent/AgentCommand'));
  const seenCtl = new Set(ctl.map((f) => f.msg.type));
  const seenTerm = new Set(term.map((f) => f.msg.type));
  bridgeAll.forEach((t) => assert.ok(seenCtl.has(t) || Types.CLIENT_ONLY_WIRE_TYPES.indexOf(t) !== -1, 'no controller fixture for ' + t));
  agentAll.forEach((t) => assert.ok(seenTerm.has(t) || Types.CLIENT_ONLY_WIRE_TYPES.indexOf(t) !== -1, 'no terminal fixture for ' + t));
});

test('fixture directions: command.* fixtures are BridgeCommand, event.* are BridgeEvent; client_* / server_* likewise', () => {
  typesIn('controller').forEach((f) => {
    if (f.file.startsWith('command.')) assert.ok(Types.isBridgeCommand(f.msg), f.file);
    if (f.file.startsWith('event.')) assert.ok(Types.isBridgeEvent(f.msg), f.file);
  });
  typesIn('terminal').forEach((f) => {
    if (f.file.startsWith('client_')) assert.ok(Types.isAgentCommand(f.msg), f.file);
    if (f.file.startsWith('server_')) assert.ok(Types.isAgentEvent(f.msg), f.file);
  });
});

test('control fixtures use catalogued actions', () => {
  typesIn('terminal').filter((f) => f.msg.type === 'control').forEach((f) => assert.ok(Types.has(Types.ControlAction, f.msg.action), f.file));
});

test('consumers agree with the catalogue', () => {
  assert.deepEqual(Logging.LEVELS, Types.values(Types.LogLevel));
  const probe = Effects.create({ bridge: {}, store: {}, named: {}, term: {}, sessions: {}, text: {}, logger: Logging });
  assert.ok(probe, 'effects interpreter builds only when handlers match EffectKind');
  Types.values(Types.BridgeEvent).forEach((t) => {
    if (t === 'knob' || t === 'button') return;
    assert.ok(Bridge.WIRE_TYPES.indexOf(t) !== -1, t);
  });
  assert.equal(Bridge.commands, Types.bridgeCommands);
  assert.equal(Agent.commands, Types.agentCommands);
  Object.keys(Events).forEach((k) => assert.ok(Types.has(Types.EventKind, Events[k].length === 0 ? Events[k]().type : Events[k](1, 1).type), k));
  assert.equal(Buttons.KEY_ESC, Types.ButtonCode.ESC);
  assert.equal(Buttons.KEY_ENTER, Types.ButtonCode.ENTER);
  assert.equal(Buttons.sessionForButton(Types.ButtonCode.KEY_3), 2);
  assert.equal(Buttons.phaseOf(Types.ButtonValue.REPEAT), 'repeat');
});

test('guards accept the right shapes and reject the wrong ones', () => {
  assert.ok(Types.isEffect({ send: { type: 'wifi_get' } }));
  assert.ok(!Types.isEffect({ send: 1, dispatch: 2 }));
  assert.ok(!Types.isEffect({ bogus: 1 }));
  assert.ok(!Types.isEffect(null));
  assert.ok(Types.isRow({ id: 'a', label: 'A', kind: 'label' }));
  assert.ok(!Types.isRow({ id: 'a', label: 'A', kind: 'nav' }));
  assert.ok(!Types.isRow({ id: 'a', label: 'A', kind: 'weird' }));
  assert.ok(Types.isRow({ id: 'r', label: 'R', kind: 'range', min: 0, max: 1, step: 1, change() { return []; } }));
  assert.ok(Types.isScreen({ title: 'T', rows() { return []; } }));
  assert.ok(!Types.isScreen({ title: 'T', rows() { return []; }, mode: 'nope' }));
  assert.ok(Types.isWireMessage({ type: 'x' }) && !Types.isWireMessage([]) && !Types.isWireMessage({ type: 1 }));
  assert.ok(Types.isSession({ id: 0, name: 'sh' }));
  assert.ok(Types.isChannelStatus('live') && !Types.isChannelStatus('up'));
  assert.ok(Types.isSendResult('queued') && !Types.isSendResult('ok'));
  assert.ok(Types.isEvent({ type: 'knob' }) && !Types.isEvent({ type: 'bogus' }));
});
