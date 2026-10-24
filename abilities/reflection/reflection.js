// The ECMAScript of abilities/reflection/reflection.scx (SCXML 5.8): loaded by its root
// <script src="reflection.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// idle

// An input deferred while the previous reflection was in hand.
function hasDeferredInput() {
  return deferred.length > 0;
}

// How the turn ended (decision D139): completed unless the input says.
function inputOutcome(input) {
  return typeof input.outcome === 'string' ? input.outcome : 'completed';
}

function inputFailure(input) {
  return typeof input.failure === 'string' ? input.failure : null;
}

// What the bot knows of itself (decision D145), null when not told.
function inputSelf(input) {
  return input.self === undefined ? null : input.self;
}

// recalling

function hasNoDatabase() {
  return database === null;
}

// The episodes' select: the turn's focus or none.
function episodeSelectStatements() {
  return [{sql: select_episodes, parameters: [cognition_input.focus, cognition_input.focus]}];
}

// The inventory's select appended: every behavior.
function behaviorSelectStatements() {
  return statements.concat([{sql: select_behaviors, parameters: [null, null]}, {sql: select_triggers, parameters: [null, null]}]);
}

function recalledEpisodes(_event) {
  return _event.data.results[0].rows.map(function (r) { return JSON.parse(r.values[0]); });
}

// The recalled inventory's names, which a change or a break may name.
function behaviorNames() {
  return behaviors.map(function (b) { return b.name; });
}

function recalledLog() {
  return 'turn ' + turn + ': ' + prior_episodes.length + ' episodes, ' + behaviors.length + ' behaviors';
}

// A failed or cancelled turn, which reflection only remembers (decision D139).
function isUnreflected() {
  return outcome !== 'completed';
}

function unreflectedLog() {
  return 'turn ' + turn + ': ' + outcome;
}

function recallFailedMessage(detail) {
  return 'Reflection memory recall failed: ' + detail;
}

// processing

function selectTimeoutDelay() {
  return select_timeout_ms + 'ms';
}

function selectCeilingDelay() {
  return select_ceiling_ms + 'ms';
}

// The select input, with the repair feedback (selection_feedback, decision
// D130) last when this attempt repairs a rejection.
function selectPrompt() {
  return selectInputJson() + selectFeedback();
}

function selectInputJson() {
  var input = {cognition_input: cognition_input, cognition_output: cognition_output, prior_episodes: prior_episodes, behaviors: behaviors};
  if (hasPersona()) {
    input.self = self;
  }
  return JSON.stringify(input);
}

// A persona to revise: the bot was told its owner's traits (decision D145).
function hasPersona() {
  return self !== null && Array.isArray(self.traits) && self.traits.length > 0;
}

// The selections of this hop's reply, every dispatch call's.
function replySelections(_event) {
  var calls = Array.isArray(_event.data.tool_calls) ? _event.data.tool_calls : [];
  return calls.reduce(function (all, call) { return all.concat(Array.isArray(Object(call.args).events) ? call.args.events : []); }, []);
}

function isRevision(s) {
  return s.event === 'bot.ability.persona.revise';
}

// This hop's answer with more than one inventory event or more than one
// revision (decision D145).
function isOverSelection(_event) {
  if (!isGenerate(_event)) {
    return false;
  }
  var selections = replySelections(_event);
  var revisions = selections.filter(isRevision).length;
  return revisions > 1 || selections.length - revisions > 1;
}

function overSelectionRejection() {
  return ({key: 'over-selection', message: 'select at most one of bot.behavior.create, bot.behavior.change or bot.behavior.break, and at most one bot.ability.persona.revise'});
}

// A call's selections with a revision first (none when it holds no list,
// as replySelections reads it).
function revisionsFirst(events) {
  if (!Array.isArray(events)) {
    return [];
  }
  return events.filter(isRevision).concat(events.filter(function (s) { return !isRevision(s); }));
}

function selectFeedback() {
  return select_rejection === null ? '' : '\n' + selection_feedback.replace('MESSAGE', function () { return rejectionMessage(select_rejection); });
}

// The code of a generate.failed, else gen_failure unchanged.
function generateFailure(_event) {
  return typeof Object(_event.data).code === 'string' ? _event.data.code : gen_failure;
}

// An event of this hop's call. processing is entered once per select
// attempt (a repair enters it again), and each entry invokes the service
// under a fresh gen_id, so a late event of an earlier entry's cancelled
// call does not match: scxml.rs keeps the events a cancelled invocation
// queued before its cancel.
function isGenerate(_event) {
  return _event.invokeid === gen_id;
}

function hasSelection(_event) {
  return isGenerate(_event) && Array.isArray(selected);
}

// The rejection of this hop's call (generate.failed, decision D126): present
// exactly for not-json and schema-mismatch, the calls the model can repair.
function selectRejection(_event) {
  return _event.data.rejection === undefined ? null : _event.data.rejection;
}

function isSelectRejection(_event) {
  return isGenerate(_event) && gen_rejection !== null;
}

// The failure message for a rejected selection (decision D130).
function rejectionMessage(r) {
  return 'Selection rejected: ' + r.message;
}

function selectRejectedMessage() {
  return rejectionMessage(gen_rejection);
}

function rejectedLog() {
  return 'attempt ' + select_attempt + ': ' + gen_rejection.key;
}

function selectFailedMessage(detail) {
  return 'Reflection select failed (' + detail + ')';
}

function generateFailedMessage() {
  return selectFailedMessage(gen_failure === null ? 'no selection' : gen_failure);
}

function isSelectHopTimeout(_event) {
  return _event.sendid === hop_timer || _event.sendid === ceiling_timer;
}

// The hop timeout's failure: the select child is cancelled on the hop
// timeout (synchronously, as leaving the invoking state cancels it).
function selectTimedOutMessage() {
  return 'Reflection child cancellation timed out.';
}

// routing_rejection

// The same-rejection check (decision D130): this hop's rejection is the one
// it repaired. The whole diagnostic is compared: a
// D126 key names only where the arguments first fail, so its message is
// compared too.
function isSameSelectRejection() {
  return select_rejection !== null && select_rejection.key === gen_rejection.key && select_rejection.message === gen_rejection.message;
}

// Another repair is allowed: at most max_selection_repairs (decision D130).
function canRepairSelect() {
  return select_attempt < max_selection_repairs;
}

// breaking

// A break: the trimmed reason, by name.
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

// Shared by preparing, authoring and accepting.
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
  return (create_intent === null ? 'change ' : 'create ') + intent.name;
}

// The change input: the turn.
function changeWriteInput() {
  return ({cognition_input: cognition_input, cognition_output: cognition_output, prior_episodes: prior_episodes});
}

// write with the intent and the existing row.
function changeWriteInputWithIntent() {
  return Object.assign(write, {intent: intent, existing_behavior: existing_behavior});
}

// The change instructions with the change input, for the builder, which
// checks the document it builds against the lifetime bounds (min_every,
// turn_after_bound).
function builderRequest() {
  return ({spec: change_instructions + '\n\n' + JSON.stringify(write), name: intent.name, model: model, min_every_ms: min_every_ms, turn_after_bound_ms: turn_after_bound_ms, apply: liveTurn()});
}

// The live turn the builder runs a turn behavior on (the live-turn
// check, decision D131): this turn, its behavior_input as autonomy builds it, and the
// sessions its output names (a learned behavior names its targets, D104).
function liveTurn() {
  return ({turn: turn, input: behavior_input, targets: outputTargets()});
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
// builds it).
function turnContract() {
  return turn_contract.replace(/TRIGGERS|TARGETS|INPUT/g, function (m) { return JSON.stringify(m === 'INPUT' ? behavior_input : m === 'TARGETS' ? outputTargets() : triggers); });
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

// storing

// The turn's cognitive episode, with the behavior the turn applied (null: none).
function cognitiveEpisode(applied) {
  var input = cognition_input;
  var episode = {focus: input.focus, focus_candidates: input.focus_candidates, stimulus_name: input.stimulus.event, output: cognition_output, behavior: applied};
  if (isUnreflected()) {
    episode.outcome = outcome;
    episode.failure = turn_failure;
  }
  return episode;
}

// Whether a failing turn has not tried to store its episode yet.
function isEpisodeUnstored() {
  return episode === null;
}

// The episode's insert.
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

// The revision's inactivity bound and its backstop: the bot's (bounds),
// else this document's defaults (decision D101).
function revisingTimeoutDelay() {
  return (bounds.revising_timeout_ms === undefined ? revising_timeout_ms : bounds.revising_timeout_ms) + 'ms';
}

function revisingCeilingDelay() {
  return (bounds.revising_ceiling_ms === undefined ? revising_ceiling_ms : bounds.revising_ceiling_ms) + 'ms';
}

// The data a selection is sent with: its data object, else {} (no data,
// null, or a non-object).
function selectionData(s) {
  return s.data !== null && typeof s.data === 'object' && !Array.isArray(s.data) ? s.data : ({});
}
