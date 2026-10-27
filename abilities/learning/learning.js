// The ECMAScript of abilities/learning/learning.scx (SCXML 5.8): loaded by its root
// <script src="learning.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// idle

function hasDeferredLesson() {
  return deferred.length > 0;
}

// recalling

function hasNoMemory() {
  return memory === null;
}

// A lesson that is not text: the bot learns from text only.
function isNotText() {
  return typeof lesson.media_type === 'string' && !/^text\//.test(lesson.media_type);
}

// Whether recalling asks the memory: with a memory and a text lesson.
function isRecallable() {
  return !hasNoMemory() && !isNotText();
}

function notTextMessage() {
  return 'Learning cannot read ' + lesson.media_type + ' content: a lesson is text.';
}

// The prior episodes, then every behavior row and its triggers.
function recallStatements() {
  return [{sql: select_episodes, parameters: []}, {sql: select_behaviors, parameters: []}, {sql: select_triggers, parameters: []}];
}

function recalledEpisodes(_event) {
  return _event.data.results[0].rows.map(function (r) { return JSON.parse(r.values[0]); });
}

function recalledLog() {
  return lessonOf() + prior_episodes.length + ' episodes, ' + behaviors.length + ' behaviors';
}

function recallFailedMessage(detail) {
  return 'Learning memory recall failed: ' + detail;
}

// selecting

// Whether the recalled inventory holds a behavior a change or a break may name.
function hasBehaviors() {
  return behaviors.length > 0;
}

// The lesson, the turn it was taught on, the prior episodes and the
// inventory (the select input, as model-facing XML: decision D201).
function selectInput() {
  return {content: lesson.content, cognition_input: lesson.cognition_input, cognition_output: lesson.cognition_output, prior_episodes: prior_episodes, behaviors: behaviors};
}

function selectTimeoutDelay() {
  return select_timeout_ms + 'ms';
}

function selectCeilingDelay() {
  return select_ceiling_ms + 'ms';
}

// An event of this state's call (a late event of an earlier lesson's
// cancelled call carries another gen_id).
function isGenerate(_event) {
  return _event.invokeid === gen_id;
}

function hasAnswered(_event) {
  return isGenerate(_event) && answered;
}

// The code of a generate.failed, else gen_failure unchanged.
function generateFailure(_event) {
  return typeof Object(_event.data).code === 'string' ? _event.data.code : gen_failure;
}

function selectFailedMessage(detail) {
  return 'Learning select failed (' + detail + ')';
}

// Whether the select call may be made again: once (#93).
function canRetrySelect() {
  return select_attempt < 1;
}

function generateFailedMessage() {
  return selectFailedMessage(gen_failure === null ? 'no selection' : gen_failure);
}

function isSelectHopTimeout(_event) {
  return _event.sendid === hop_timer || _event.sendid === ceiling_timer;
}

// The data a selection is sent with: its data object, else {}.
function selectionData(s) {
  return s.data !== null && typeof s.data === 'object' && !Array.isArray(s.data) ? s.data : ({});
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

// The lesson as the revision's instruction (decision D149).
// The teaching turn's output and choices without the lesson itself: a
// behavior never teaches the bot (decisions D221 and D222).
function lessonOutput() {
  return lesson.cognition_output.filter(function (s) { return Object(s).event !== 'bot.ability.learning.input'; });
}

function lessonChoices() {
  return (Array.isArray(lesson.choices) ? lesson.choices : []).filter(function (c) { return Object(c).event !== 'bot.ability.learning.input'; });
}

function lessonInstruction() {
  return ({text: lesson.content, kind: 'instruction'});
}

// The revision's inactivity bound and its backstop: the bot's (bounds),
// else this document's defaults (decision D101).
function revisingTimeoutDelay() {
  return (bounds.revising_timeout_ms === undefined ? revising_timeout_ms : bounds.revising_timeout_ms) + 'ms';
}

function revisingCeilingDelay() {
  return (bounds.revising_ceiling_ms === undefined ? revising_ceiling_ms : bounds.revising_ceiling_ms) + 'ms';
}

function revisionFailedMessage(detail) {
  return 'Learning revision failed: ' + detail;
}

// reporting and failing

// The lesson's log lines name its turn, so a reader ties each to its lesson.
function learnedLog() {
  return lessonOf() + behavior.name + ' ' + behavior.source + (behavior.status === 'BROKEN' ? ' BROKEN' : '');
}

function failedLog() {
  return lessonOf() + failure;
}

function lessonOf() {
  return 'lesson of turn ' + lesson.turn + ': ';
}

// breaking (decision D220)

function breakStatements() {
  return [{sql: break_behavior, parameters: [typeof intent.reason === 'string' ? intent.reason.trim() : '', intent.name]}];
}

function hasBroken(_event) {
  return _event.data.results[0].rowcount === 1;
}

// The broken behavior as the lesson's result names it: its source is the
// inventory's (a break builds nothing).
function brokenBehavior() {
  var row = behaviors.filter(function (b) { return b.name === intent.name; })[0];
  return ({name: intent.name, source: row === undefined ? '' : row.source, description: row === undefined ? null : row.description, status: 'BROKEN'});
}

function breakFailedMessage(detail) {
  return 'Learning break failed: ' + detail;
}

// The lesson's result: whether it took and the behavior it became, never
// why it failed, with the teaching turn's participants.
function learningOutput(learned) {
  return ({lesson: lesson.content, lesson_turn: lesson.turn, learned: learned, behavior: learned ? {name: behavior.name, description: behavior.description === undefined ? null : behavior.description, status: behavior.status === 'BROKEN' ? 'BROKEN' : 'ACTIVE'} : null, participants: lesson.participants});
}

// memory

// Whether the event answers the request the state `name` last sent the
// memory (decision D262): its id is that request's.
function isMemoryReply(name) {
  return memory_requests[name] !== undefined && _event.data.id === memory_requests[name];
}
