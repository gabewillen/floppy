// The ECMAScript of abilities/autonomy/autonomy.scx (SCXML 5.8): loaded by its root
// <script src="autonomy.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared by seeding, loading, running, using and recording_failure.

function isFromInvoke(event, invokeid) {
  return event.invokeid === invokeid;
}

function isTimerEvent(event) {
  return event.sendid === timer;
}

function memoryTimeoutDelay() {
  return memory_timeout_ms + 'ms';
}

// matching

// The stimulus triggers the SpeechHeard seed.
function triggersSpeechHeard() {
  return stimulus.event === 'bot.ability.listening.speech.output';
}

// seeding

function triedWithSeed() {
  return tried.concat(['SpeechHeard']);
}

function silenceDelay() {
  return silence_ms + 'ms';
}

// Shared by seeding and running: the child answered the turn with at least
// one selection (mosfet _coerce_behavior_output: an empty one is unhandled).
function isBehaviorHandled(event) {
  return event.data.turn === turn && event.data.handled === true && Array.isArray(event.data.selected) && event.data.selected.length > 0;
}

function seedSilentLog() {
  return 'SpeechHeard after ' + silence_ms + ' ms';
}

// mosfet _emit_native_start_failure.
function seedFailedReason(event) {
  return 'Autonomy native seed \'SpeechHeard\' failed: ' + event.name;
}

// inventory

// database null means no learned inventory.
function hasNoMemory() {
  return database === null;
}

function hasNoMemoryAndNoCandidates() {
  return database === null && tried.length === 0;
}

function noMemoryLog() {
  return 'turn ' + turn + ': no memory';
}

// loading

// mosfet _behavior_select_input.
function behaviorSelectStatements() {
  return [{sql: select_sql, parameters: [stimulus.event]}];
}

function behaviorsFromMemoryOutput(event) {
  return event.data.results[0].rows;
}

function loadFailedReason(detail) {
  return 'Autonomy behavior load failed: ' + detail;
}

function loadTimedOutReason() {
  return 'Autonomy behavior load timed out after ' + memory_timeout_ms + ' ms';
}

// matched

function matchedLog() {
  return 'turn ' + turn + ': ' + rows.length + ' matched for ' + stimulus.event;
}

function hasNoMatch() {
  return rows.length === 0;
}

function hasNoMatchAndNoCandidates() {
  return rows.length === 0 && tried.length === 0;
}

// running

// The name of the behavior at index.
function candidateName() {
  return rows[index].values[0];
}

// The SCXML source of the behavior at index.
function candidateSource() {
  return rows[index].values[1];
}

function triedWithCandidate() {
  return tried.concat([candidateName()]);
}

// mosfet behavior_input_payload without focus and focus_candidates, which
// running assigns after it.
function behaviorInputPayload() {
  return (stimulus.data !== null && typeof stimulus.data === 'object' ? Object.assign({}, stimulus.data) : {event: stimulus.data});
}

function candidateSilentLog() {
  return candidateName() + ' after ' + silence_ms + ' ms';
}

function candidateFailedLog(event) {
  return candidateName() + ' (' + event.name + ')';
}

// advancing

function nextIndex() {
  return index + 1;
}

function hasNextCandidate() {
  return index < rows.length;
}

// using and recording_failure

// mosfet _prepare_used_persistence.
function usedPersistenceStatements() {
  return [{sql: used_sql, parameters: [candidateName()]}];
}

// mosfet _prepare_failed_persistence.
function failedPersistenceStatements() {
  return [{sql: failed_sql, parameters: [candidateName()]}];
}

function usageFailedReason(detail) {
  return 'Autonomy behavior usage failed: ' + detail;
}

function usageTimedOutReason() {
  return 'Autonomy behavior usage timed out after ' + memory_timeout_ms + ' ms';
}

// done, unhandled, answered, failed

function noBehaviorsOutput() {
  return {turn: turn, handled: false, candidates: [], reason: 'no-behaviors'};
}

function unhandledOutput() {
  return {turn: turn, handled: false, candidates: tried, reason: 'unhandled'};
}

function answeredOutput() {
  return {turn: turn, handled: true, selected: answer, candidates: tried, reason: 'behavior'};
}

function failedOutput() {
  return {turn: turn, handled: false, failed: true, candidates: tried, reason: answer};
}
