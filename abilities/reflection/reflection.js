// The ECMAScript of abilities/reflection/reflection.scx (SCXML 5.8): loaded by its root
// <script src="reflection.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared by every memory step.

function isFromInvoke(_event, invokeid) {
  return _event.invokeid === invokeid;
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

// mosfet episodes.episode_select_input: the turn's focus or none.
function episodeSelectStatements() {
  return [{sql: select_episodes, parameters: [cognition_input.focus, cognition_input.focus]}];
}

// mosfet storage.select_all_behaviors_clauses appended: every behavior.
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

// mosfet SelectInput.
function selectPrompt() {
  return JSON.stringify({cognition_input: cognition_input, cognition_output: cognition_output, prior_episodes: prior_episodes, behaviors: behaviors});
}

// The events of a generate.result object reply, else selected unchanged.
function generatedSelection(_event) {
  return _event.name === 'generate.result' && Object(_event.data).json instanceof Object ? _event.data.json.events : selected;
}

// The code of a generate.failed, else gen_failure unchanged.
function generateFailure(_event) {
  return _event.name === 'generate.failed' && typeof Object(_event.data).code === 'string' ? _event.data.code : gen_failure;
}

function isGenerateDone(_event) {
  return _event.invokeid === gen_id;
}

function hasSelection(_event) {
  return isGenerateDone(_event) && Array.isArray(selected);
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

function selectTimedOutMessage() {
  return 'Reflection select timed out after ' + select_timeout_ms + ' ms';
}

// breaking

// mosfet _apply_break: the trimmed reason, by name.
function breakBehaviorStatements() {
  return [{sql: break_behavior, parameters: [typeof behavior.reason === 'string' ? behavior.reason.trim() : '', behavior.name]}];
}

function hasBrokenBehavior(_event) {
  return isFromInvoke(_event, 'breaking_memory') && _event.data.results[0].rowcount === 1;
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

// mosfet revision._draft: the create stub.
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

// mosfet ChangeWriteInput: the turn.
function changeWriteInput() {
  return ({cognition_input: cognition_input, cognition_output: cognition_output, prior_episodes: prior_episodes});
}

// write with the intent and the existing row.
function changeWriteInputWithIntent() {
  return Object.assign(write, {intent: intent, existing_behavior: existing_behavior});
}

// mosfet CHANGE_INSTRUCTIONS with the ChangeWriteInput, for the builder.
function builderRequest() {
  return ({spec: change_instructions + '\n\n' + JSON.stringify(write), model: model});
}

function builderConstraints() {
  return [turn_contract, routine_contract, no_schema_files];
}

function hasWrittenScxml(_event) {
  return typeof _event.data.scxml === 'string' && _event.data.scxml.length > 0;
}

function abandonedMessage(_event) {
  return 'Reflection change abandoned: ' + _event.data.reason + '.';
}

// accepting

// mosfet _evidence_trigger_names: the turn's stimulus, then the prior episodes'.
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

// mosfet _grounded_triggers.
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

// mosfet _AppliedEventData.behavior.
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

// mosfet storage.replace_behavior_clauses: delete triggers, delete row.
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

// mosfet storage.mark_draft(reason=validation).
function draftBehaviorStatements() {
  return [{sql: draft_behavior, parameters: [intent.name]}];
}

function draftLog() {
  return intent.name + ' DRAFT validation (draft)';
}

// storing

// mosfet CognitiveEpisode: the turn's focus.
function cognitiveEpisode() {
  return ({focus: cognition_input.focus, focus_candidates: cognition_input.focus_candidates});
}

// mosfet episodes.episode_insert_input.
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
