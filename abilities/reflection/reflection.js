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

// A create whose name the recalled inventory does not hold.
function isCreate() {
  return create_intent !== null && !behaviors.some(function (b) { return b.name === intent.name; });
}

// The inventory event the intent came as.
function intentEvent() {
  return create_intent === null ? 'bot.behavior.change' : 'bot.behavior.create';
}

// Where the builder sends its progress (decision D101): this session, past
// the revision that invokes it.
function progressTarget() {
  return '#_scxml_' + _sessionid;
}

function revisionFailedMessage(detail) {
  return 'Reflection revision failed: ' + detail;
}

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
