// The ECMAScript of abilities/autonomy/autonomy.scx (SCXML 5.8): loaded by its root
// <script src="autonomy.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// loading

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

// seeding: the seed answered the turn with at least
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

// mosfet behavior_input_payload without focus and focus_candidates, which
// running assigns after it.
function behaviorInputPayload() {
  return (stimulus.data !== null && typeof stimulus.data === 'object' ? Object.assign({}, stimulus.data) : {event: stimulus.data});
}

// The matched rows as the runner's candidates, [{name, source}].
function candidateRows() {
  return rows.map(function (row) { return {name: row.values[0], source: row.values[1]}; });
}

function isRunHandled(event) {
  return event.data.handled === true;
}

function isRunRejected(event) {
  return event.data.rejection !== null;
}

function triedWithRun(event) {
  return tried.concat(event.data.tried);
}

function runFailedLog(event) {
  return rows[0].values[0] + ' (' + event.name + ')';
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
