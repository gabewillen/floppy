// The ECMAScript of abilities/reflection/reflection.scx (SCXML 5.8): loaded by its root
// <script src="reflection.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// idle

// An input deferred while the previous reflection was in hand.
function hasDeferredInput() {
  return deferred.length > 0;
}

// The behaviors of select_behaviors' rows (named as the inventory's
// fields) with their triggers from select_triggers' rows: the SQL both
// engines run returns plain rows, assembled here.
function assembledBehaviors(behaviorRows, triggerRows) {
  return behaviorRows.map(function (r) {
    var b = Object.fromEntries(r.columns.map(function (c, i) { return [c, r.values[i]]; }));
    return Object.assign(b, {
      examples: JSON.parse(b.examples),
      used_count: Number(b.used_count),
      failed_count: Number(b.failed_count),
      triggers: triggerRows.filter(function (t) { return t.values[0] === b.name; }).map(function (t) { return t.values[1]; })
    });
  });
}

// The intent's triggers, each once.
function distinctIntentTriggers(list) {
  return list.filter(function (t, i, a) { return a.indexOf(t) === i; });
}

function triggerInserts(list) {
  return distinctIntentTriggers(list).map(function (t) { return {sql: insert_trigger, parameters: [intent.name, t]}; });
}

// recalling

function hasNoDatabase() {
  return database === null;
}

// The original episodes.episode_select_input: the turn's focus or none.
function episodeSelectStatements() {
  return [{sql: select_episodes, parameters: [cognition_input.focus, cognition_input.focus]}];
}

// The original storage.select_all_behaviors_clauses appended: every behavior.
function behaviorSelectStatements() {
  return statements.concat([{sql: select_behaviors, parameters: [null, null]}, {sql: select_triggers, parameters: [null, null]}]);
}

function recalledEpisodes(_event) {
  return _event.data.results[0].rows.map(function (r) { return JSON.parse(r.values[0]); });
}

function recalledBehaviors(_event) {
  return assembledBehaviors(_event.data.results[1].rows, _event.data.results[2].rows);
}

function recalledLog() {
  return 'turn ' + turn + ': ' + prior_episodes.length + ' episodes, ' + behaviors.length + ' behaviors';
}

function recallFailedMessage(detail) {
  return 'Reflection memory recall failed: ' + detail;
}

// processing

function selectTimeoutDelay() {
  return select_timeout_ms + 'ms';
}

// The reference SelectInput.
function selectPrompt() {
  return JSON.stringify({cognition_input: cognition_input, cognition_output: cognition_output, prior_episodes: prior_episodes, behaviors: behaviors});
}

// The events of a generate.result object reply, else selected unchanged
// (D41: the service's events are unchecked).
function generatedSelection(_event) {
  return Object(_event.data).json instanceof Object ? _event.data.json.events : selected;
}

// The code of a generate.failed, else gen_failure unchanged.
function generateFailure(_event) {
  return typeof Object(_event.data).code === 'string' ? _event.data.code : gen_failure;
}

// An event of this hop's call. processing is entered once per reflection,
// and each entry invokes the service under a fresh gen_id, so a late
// event of an earlier entry's cancelled call does not match: scxml.rs
// keeps the events a cancelled invocation queued before its cancel.
function isGenerate(_event) {
  return _event.invokeid === gen_id;
}

function hasSelection(_event) {
  return isGenerate(_event) && Array.isArray(selected);
}

function hasEmptySelection(_event) {
  return hasSelection(_event) && selected.length === 0;
}

function hasSingleSelection(_event) {
  return hasSelection(_event) && selected.length === 1;
}

function selectedEvent() {
  return selected[0].event;
}

function selectedData() {
  return selected[0].data;
}

function selectFailedMessage(detail) {
  return 'Reflection select failed (' + detail + ')';
}

function generateFailedMessage() {
  return selectFailedMessage(gen_failure === null ? 'no selection' : gen_failure);
}

function isSelectHopTimeout(_event) {
  return _event.sendid === hop_timer;
}

// The original _fail_cancel_timeout: the select child is cancelled on the hop
// timeout (synchronously here, as leaving the invoking state cancels it).
function selectTimedOutMessage() {
  return 'Reflection child cancellation timed out.';
}

// breaking

// The original _apply_break: the trimmed reason, by name.
function breakBehaviorStatements() {
  return [{sql: break_behavior, parameters: [typeof behavior.reason === 'string' ? behavior.reason.trim() : '', behavior.name]}];
}

function hasBrokenBehavior(_event) {
  return _event.data.results[0].rowcount === 1;
}

function brokenLog() {
  return behavior.name + ' BROKEN (break)';
}

function unknownBreakMessage() {
  return breakFailedMessage('Reflection break selected unknown behavior: ' + behavior.name + '.');
}

function breakFailedMessage(detail) {
  return 'Reflection break failed: ' + detail;
}

// preparing

// A create whose name the recalled inventory does not hold.
function isCreate() {
  return create_intent !== null && !behaviors.some(function (b) { return b.name === intent.name; });
}

// The original revision._draft: the create stub.
function insertDraftStatements() {
  return [{sql: insert_draft, parameters: [intent.name, intent.description || '']}];
}

function insertDraftTriggersStatements() {
  return statements.concat(triggerInserts(intent.triggers || []));
}

// The intent's behavior row appended.
function intentBehaviorSelectStatements() {
  return statements.concat([{sql: select_behaviors, parameters: [intent.name, intent.name]}, {sql: select_triggers, parameters: [intent.name, intent.name]}]);
}

// The intent's row, from the last two statements (undefined: unknown).
function preparedBehavior(_event) {
  return assembledBehaviors(_event.data.results.at(-2).rows, _event.data.results.at(-1).rows)[0];
}

// Shared by preparing, authoring and accepting.
function revisionFailedMessage(detail) {
  return 'Reflection revision failed: ' + detail;
}

// authoring

function isUnknownBehavior() {
  return existing_behavior === undefined;
}

function unknownBehaviorMessage() {
  return 'Revision selected unknown behavior: ' + intent.name + '.';
}

function revisionLog() {
  return (create_intent === null ? 'change ' : 'create ') + intent.name;
}

// The reference ChangeWriteInput: the turn.
function changeWriteInput() {
  return ({cognition_input: cognition_input, cognition_output: cognition_output, prior_episodes: prior_episodes});
}

// write with the intent and the existing row.
function changeWriteInputWithIntent() {
  return Object.assign(write, {intent: intent, existing_behavior: existing_behavior});
}

// The original CHANGE_INSTRUCTIONS with the ChangeWriteInput, for the builder,
// which checks the document it builds against the original check()'s lifetime
// bounds (min_every, turn_after_bound).
function builderRequest() {
  return ({spec: change_instructions + '\n\n' + JSON.stringify(write), model: model, min_every_ms: min_every_ms, turn_after_bound_ms: turn_after_bound_ms});
}

function builderConstraints() {
  return [turn_contract, routineContract(), triggerContract(), no_schema_files];
}

// This turn's stimulus as Autonomy would give it to a turn behavior (the
// reference behavior_input_payload, autonomy.js behaviorInputPayload).
function turnBehaviorInput() {
  var data = cognition_input.stimulus.data;
  var given = data !== null && typeof data === 'object' ? Object.assign({}, data) : {event: data};
  given.focus = cognition_input.focus;
  given.focus_candidates = cognition_input.focus_candidates;
  return given;
}

// What a turn behavior's input is when Autonomy runs it (the reference
// behavior_input_payload): the data of a stimulus whose event is one of the
// triggers the accepted row will carry (the grounded triggers, computed as
// accepting does).
function triggerContract() {
  if (triggers.length === 0) {
    return 'No stimulus triggers are grounded for this behavior, so Autonomy will not run it as a turn behavior.';
  }
  return 'Autonomy runs a turn behavior only for a stimulus whose event is one of its triggers, ' + JSON.stringify(triggers) + ', so the stimulus event is already matched: input holds that stimulus\'s data fields with focus and focus_candidates, and has no stimulus or event field to test. Guard on input\'s data fields only. For this turn input is ' + JSON.stringify(turnBehaviorInput()) + '.';
}

// The original schedule.At.next_after for an at schedule {time: 'HH:MM', days,
// tz, rule}: the first occurrence later than now (ms since the epoch) by
// more than OCCURRENCE_TOLERANCE (1 s) within 8 days, as {ms, iso} (iso
// the wall time with its offset), or null when days name no weekday. The
// data model has no time zone database (no Intl), so the zone is its
// POSIX TZ rule, the rule its zoneinfo file ends with; a wall time in a
// spring-forward gap resolves past the gap and one in a fall-back overlap
// is the first of its two instants (zoneinfo fold=0). A routine carries it
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
  return typeof Object(_event.data).reason !== 'string' && typeof Object(_event.data).scxml === 'string' && _event.data.scxml.length > 0;
}

// The builder's failed with the last draft it rejected (the original: a failed
// check, which drafts the rejected source).
function hasRejectedDraft(_event) {
  return typeof Object(_event.data).reason === 'string' && typeof _event.data.scxml === 'string' && _event.data.scxml.length > 0;
}

// The original diagnostic.Report.render of the builder's diagnostics.
function renderedReport(diagnostics) {
  return 'behavior validation failed:' + (Array.isArray(diagnostics) ? diagnostics : []).map(function (d) { return '\n  error[' + d.code + '] ' + d.kind + ': ' + d.message; }).join('');
}

// The original Revision._abandon: the same diagnostics after a fix.
function abandonedMessage(_event) {
  return _event.data.reason === 'no-progress' ? 'Reflection change abandoned: same diagnostic message after fix:\n' + renderedReport(_event.data.diagnostics) : 'Reflection change abandoned: ' + _event.data.reason + '.';
}

// The original _fail_child: the change call failed, with no draft.
function builderFailedMessage(_event) {
  var d = Array.isArray(Object(_event.data).diagnostics) ? _event.data.diagnostics[0] : undefined;
  return revisionFailedMessage(String(Object(_event.data).reason) + (d === undefined ? '' : ': ' + d.message));
}

// accepting

// The original _evidence_trigger_names: the turn's stimulus, then the prior episodes'.
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

// The original _grounded_triggers.
function groundedTriggers() {
  return triggers.some(function (t) { return evidence.indexOf(t) >= 0; }) ? triggers.filter(function (t) { return evidence.indexOf(t) >= 0; }) : triggers.length === 0 ? [] : evidence.slice(0, 1);
}

// The grounded triggers, else the existing row's.
function acceptedTriggers() {
  return triggers.length > 0 ? triggers : existing_behavior.triggers;
}

// persistent when the program sends bot.behavior.tick.
function writtenLifetime() {
  return written.indexOf('bot.behavior.tick') >= 0 ? 'persistent' : 'turn';
}

// The original _AppliedEventData.behavior.
function appliedBehavior() {
  return ({event: intent.event, name: intent.name, triggers: triggers, description: intent.description || existing_behavior.description || null});
}

function appliedBehaviorWithSource() {
  return Object.assign(behavior, {reason: intent.reason || null, source: written});
}

// insert_behavior's parameters, the usage counters kept.
function behaviorRow() {
  return [intent.name, written, behavior.description || '', JSON.stringify(existing_behavior.examples), lifetime];
}

function behaviorRowWithUsage() {
  return row.concat([String(existing_behavior.used_count), existing_behavior.last_used_at]);
}

function behaviorRowWithFailures() {
  return row.concat([String(existing_behavior.failed_count), existing_behavior.last_failed_at]);
}

// The original storage.replace_behavior_clauses: delete triggers, delete row.
function replaceBehaviorStatements() {
  return [{sql: delete_triggers, parameters: [intent.name]}, {sql: delete_behavior, parameters: [intent.name]}];
}

function replaceBehaviorWithRow() {
  return statements.concat([{sql: insert_behavior, parameters: row}]);
}

function replaceBehaviorWithTriggers() {
  return statements.concat(triggerInserts(triggers));
}

function acceptedLog() {
  return intent.name + ' ACTIVE ' + lifetime + ' ' + JSON.stringify(triggers) + ' (accept)';
}

// abandoning

// The original Revision._persist_draft: the rejected source, DRAFT validation.
function draftBehaviorStatements() {
  return [{sql: draft_behavior, parameters: [written, intent.name]}];
}

function draftLog() {
  return intent.name + ' DRAFT validation (draft)';
}

// storing

// The reference CognitiveEpisode: the turn's focus.
function cognitiveEpisode() {
  return ({focus: cognition_input.focus, focus_candidates: cognition_input.focus_candidates});
}

// The original episodes.episode_insert_input.
function insertEpisodeStatements() {
  return [{sql: insert_episode, parameters: [cognition_input.focus, JSON.stringify(episode)]}];
}

function storedLog() {
  return 'turn ' + turn;
}

function episodeStoreFailedMessage(detail) {
  return 'Reflection episode store failed: ' + detail;
}

// revising

function revisingTimeoutDelay() {
  return revising_timeout_ms + 'ms';
}
