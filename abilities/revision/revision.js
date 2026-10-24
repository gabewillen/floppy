// The ECMAScript of abilities/revision/revision.scx (SCXML 5.8): loaded by its root
// <script src="revision.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// preparing

// A create's stub, DRAFT.
function draftInstance() {
  return ({name: intent.name, source: '', description: intent.description || '', status: 'DRAFT', triggers: intent.triggers || []});
}

// The intent's behavior row appended.
function intentBehaviorSelectStatements() {
  return statements.concat([{sql: select_behaviors, parameters: [intent.name, intent.name]}, {sql: select_triggers, parameters: [intent.name, intent.name]}]);
}

// The intent's row, from the last two statements (undefined: unknown).
function preparedBehavior() {
  return prepared[0];
}

// Shared by every state that fails.
function revisionFailedMessage(detail) {
  return 'Reflection revision failed: ' + detail;
}

// reading

// No program to read: an unknown row, or one with no source yet.
function hasNoDocuments() {
  return existing_behavior === undefined || existing_behavior.source === '';
}

// The file of the row's program named by its path's last segment.
function documentFile(extension) {
  return existing_behavior.source.split('/').pop().replace(/\.scx$/, '') + extension;
}

function seededSchemas(_event) {
  return JSON.parse(_event.data.seed);
}

// authoring

function isUnknownBehavior() {
  return existing_behavior === undefined;
}

function unknownBehaviorMessage() {
  return 'Revision selected unknown behavior: ' + intent.name + '.';
}

function revisionLog() {
  return (intent_event === 'bot.behavior.create' ? 'create ' : 'change ') + intent.name;
}

// The change input: the turn, and the lesson taught when there is one
// (decision D149).
function changeWriteInput() {
  var input = {cognition_input: cognition_input, cognition_output: cognition_output, prior_episodes: prior_episodes, turn_choices: turn_choices};
  if (instruction !== null) {
    input.instruction = instruction;
  }
  return input;
}

// write with the intent and the existing row.
function changeWriteInputWithIntent() {
  return Object.assign(write, {intent: intent, existing_behavior: existing_behavior});
}

// The change instructions with the change input as their context, for the
// builder, which checks the document it builds against the lifetime bounds
// (min_every, turn_after_bound).
function builderRequest() {
  return ({spec: change_instructions, context: write, name: intent.name, model: model, min_every_ms: min_every_ms, turn_after_bound_ms: turn_after_bound_ms, apply: liveTurn()});
}

// Whether the lesson's intent carries an example and this turn's input
// holds the message it replaces (a conversation's parent.data.content,
// decision D213).
function isProbe() {
  var example = intent === null ? undefined : intent.example;
  var parent = behavior_input === null ? undefined : behavior_input.parent;
  return instruction !== null && typeof example === 'string' && example.length > 0 && parent !== null && typeof parent === 'object' && parent.data !== null && typeof parent.data === 'object' && typeof parent.data.content === 'string';
}

// This turn's input with the lesson's message replaced by its example,
// where parent.data names it and in the conversation's message that
// holds it.
function probedInput() {
  if (!probing) {
    return behavior_input;
  }
  var probe = JSON.parse(JSON.stringify(behavior_input));
  var lesson = probe.parent.data.content;
  probe.parent.data.content = intent.example;
  if (Array.isArray(probe.messages)) {
    for (var i = probe.messages.length - 1; i >= 0; i--) {
      var m = probe.messages[i];
      if (m !== null && typeof m === 'object' && m.content === lesson) {
        m.content = intent.example;
        break;
      }
    }
  }
  return probe;
}

// The live turn the builder runs a turn behavior on (the live-turn
// check, decision D131): this turn, its behavior_input as autonomy builds it, and the
// sessions its output names (a learned behavior names its targets, D104).
function liveTurn() {
  var live = {turn: turn, input: behavior_input, targets: outputTargets(), choices: turn_choices, handled: instruction === null || probing};
  if (probing) {
    live.decline = lesson_input;
  }
  return live;
}

// The distinct targets of this turn's cognition_output, at most 32.
function outputTargets() {
  return cognition_output.map(function (s) { return s.target; }).filter(function (t, i, a) { return typeof t === 'string' && t.length > 0 && a.indexOf(t) === i; }).slice(0, 32);
}

function builderConstraints() {
  return [turnContract(), routineContract()];
}

// The turn behavior contract with the grounded triggers it is stored under
// (autonomy matches them against the stimulus event), the targets this
// turn's output names, and this turn's input (behavior_input, as autonomy
// builds it, in model-facing XML: decision D201).
function turnContract() {
  var contract = turn_contract.replace(/TRIGGERS|TARGETS|INPUT/g, function (m) { return m === 'INPUT' ? turn_input : namesText(m === 'TARGETS' ? outputTargets() : triggers); });
  return instruction === null ? contract : contract + ' ' + (probing ? probe_contract : lesson_contract);
}

// Names as a parenthesized, comma-separated list.
function namesText(names) {
  return '(' + names.join(', ') + ')';
}

// The next occurrence of an at schedule {time: 'HH:MM', days, tz, rule}:
// the first occurrence later than now (ms since the epoch) by more than
// 1 s within 8 days, as {ms, iso} (iso
// the wall time with its offset), or null when days name no weekday. The
// data model has no time zone database (no Intl), so the zone is its
// POSIX TZ rule, the rule its zoneinfo file ends with; a wall time in a
// spring-forward gap resolves past the gap and one in a fall-back overlap
// is the earlier of its two instants. A routine carries it
// as data (routineContract), so its text has no <, &, or quotes.
var next_after = (at, now) => {
  const hms = s => {
    const m = /^([+-]?)(\d+)(?::(\d+))?(?::(\d+))?$/.exec(s);
    return (m[1] == `-` ? -1 : 1) * (m[2] * 3600 + (m[3] || 0) * 60 + (m[4] || 0) * 1) * 1000;
  };
  const r = /^(\x3c[^\x3e]+\x3e|[A-Za-z]+)([+-]?[\d:]+)(?:(\x3c[^\x3e]+\x3e|[A-Za-z]+)([+-]?[\d:]+)?,M(\d+)\.(\d)\.(\d)(?:\/([+-]?[\d:]+))?,M(\d+)\.(\d)\.(\d)(?:\/([+-]?[\d:]+))?)?$/.exec(at.rule);
  const std = -hms(r[2]);
  const dst = r[3] ? (r[4] ? -hms(r[4]) : std + 3600000) : std;
  const change = (y, m, w, d, t, o) => {
    let day = 1 + (d - new Date(Date.UTC(y, m - 1, 1)).getUTCDay() + 7) % 7 + (w - 1) * 7;
    while (day > new Date(Date.UTC(y, m, 0)).getUTCDate()) {
      day -= 7;
    }
    return Date.UTC(y, m - 1, day) + (t ? hms(t) : 7200000) - o;
  };
  const offset = u => {
    if (!r[3]) {
      return std;
    }
    const y = new Date(u).getUTCFullYear();
    const s = change(y, +r[5], +r[6], +r[7], r[8], std);
    const e = change(y, +r[9], +r[10], +r[11], r[12], dst);
    return (s > e ? (u >= s || e > u) : !(s > u || u >= e)) ? dst : std;
  };
  const hi = Math.max(std, dst);
  const lo = Math.min(std, dst);
  const hm = at.time.split(`:`);
  const today = new Date(now + offset(now));
  for (let k = 0; 8 > k; k += 1) {
    const local = Date.UTC(today.getUTCFullYear(), today.getUTCMonth(), today.getUTCDate() + k, +hm[0], +hm[1]);
    const u = offset(local - hi) == hi ? local - hi : local - lo;
    const day = [`sun`, `mon`, `tue`, `wed`, `thu`, `fri`, `sat`][new Date(local).getUTCDay()];
    if (at.days.indexOf(day) >= 0 ? u > now + 1000 : false) {
      const o = (local - u) / 60000;
      const a = Math.abs(o);
      return {ms: u, iso: new Date(local).toISOString().slice(0, 19) + (o >= 0 ? `+` : `-`) + String(Math.floor(a / 60)).padStart(2, `0`) + `:` + String(a % 60).padStart(2, `0`)};
    }
  }
  return null;
};

// The persistent routine contract with next_after's text on one line.
function routineContract() {
  return routine_contract.replace('NEXT_AFTER', function () { return String(next_after).replace(/\s+/g, ' '); });
}

// The builder's done: a document that validated and passed its simulation.
function isBuilt(_event) {
  return typeof Object(_event.data).reason !== 'string' && typeof Object(_event.data).scxml === 'string' && _event.data.scxml.length > 0 && typeof _event.data.path === 'string';
}

// The builder's failed with the last draft it rejected (a failed check,
// which drafts the rejected source).
function hasRejectedDraft(_event) {
  return typeof Object(_event.data).reason === 'string' && typeof _event.data.scxml === 'string' && _event.data.scxml.length > 0 && typeof _event.data.path === 'string';
}

// The builder's diagnostics rendered as a validation report.
function renderedReport(diagnostics) {
  return 'behavior validation failed:' + (Array.isArray(diagnostics) ? diagnostics : []).map(function (d) { return '\n  error[' + d.code + '] ' + d.kind + ': ' + d.message; }).join('');
}

// A build that used its attempts ends with its last diagnostics (decision D133).
// Whether the revised row is ACTIVE: a failed change keeps it (D209).
function isActive() {
  return existing_behavior !== null && existing_behavior !== undefined && existing_behavior.status === 'ACTIVE';
}

function keptLog() {
  return intent.name + ' ACTIVE kept (change abandoned)';
}

function abandonedMessage(_event) {
  return _event.data.reason === 'attempts-exhausted' ? 'Reflection change abandoned: attempts exhausted:\n' + renderedReport(_event.data.diagnostics) : 'Reflection change abandoned: ' + _event.data.reason + '.';
}

// The change call failed, with no draft.
function builderFailedMessage(_event) {
  var d = Array.isArray(Object(_event.data).diagnostics) ? _event.data.diagnostics[0] : undefined;
  return revisionFailedMessage(String(Object(_event.data).reason) + (d === undefined ? '' : ': ' + d.message));
}

// accepting

// The evidence's trigger names: the turn's stimulus, then the prior episodes'.
function evidenceStimulusNames() {
  return [cognition_input.stimulus.event].concat(prior_episodes.map(function (e) { return e.stimulus_name; }));
}

// evidence without empty or repeated names.
function evidenceTriggerNames() {
  return evidence.filter(function (n, i, a) { return typeof n === 'string' && n.length > 0 && a.indexOf(n) === i; });
}

function intentTriggers() {
  return Array.isArray(intent.triggers) ? intent.triggers : [];
}

// The triggers grounded in the evidence: the intent's that the evidence
// names, else the evidence's first name when the intent named any, else
// none.
function groundedTriggers() {
  return triggers.some(function (t) { return evidence.indexOf(t) >= 0; }) ? triggers.filter(function (t) { return evidence.indexOf(t) >= 0; }) : triggers.length === 0 ? [] : evidence.slice(0, 1);
}

// The grounded triggers, else the existing row's.
function acceptedTriggers() {
  return triggers.length > 0 ? triggers : existing_behavior.triggers;
}

// The applied inventory event's behavior.
function appliedBehavior() {
  return ({event: intent.event, name: intent.name, triggers: triggers, description: intent.description || existing_behavior.description || null});
}

function appliedBehaviorWithSource() {
  return Object.assign(behavior, {reason: intent.reason || null, source: path});
}

// The accepted instance, ACTIVE with the written documents' path as its
// source and their lifetime, the
// grounded triggers and the existing row's examples and usage counters.
function acceptedInstance() {
  var e = existing_behavior;
  return ({name: intent.name, source: path, description: behavior.description || '', examples: e.examples, lifetime: lifetime, status: 'ACTIVE', triggers: triggers,
    used_count: e.used_count, last_used_at: e.last_used_at, failed_count: e.failed_count, last_failed_at: e.last_failed_at});
}

function acceptedLog() {
  return intent.name + ' ACTIVE ' + lifetime + ' ' + JSON.stringify(triggers) + ' (accept)';
}

// abandoning

// An abandoned revision's draft: the rejected draft's path, DRAFT validation.
function draftBehaviorStatements() {
  return [{sql: draft_behavior, parameters: [path, intent.name]}];
}

function draftLog() {
  return intent.name + ' DRAFT validation (draft)';
}
