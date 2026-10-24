// The ECMAScript of abilities/autonomy/autonomy.scx (SCXML 5.8): loaded by its root
// <script src="autonomy.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// loading

function memoryTimeoutDelay() {
  return memory_timeout_ms + 'ms';
}

// percept

// The turn's percepts: a bot.perceived batch's stimuli in arrival order,
// else the stimulus itself.
function perceptsOf(stimulus) {
  if (stimulus.event === 'bot.perceived') {
    return stimulus.data.percepts.map(function (percept) { return percept.stimulus; });
  }
  return [stimulus];
}

function currentPercept() {
  return percepts[position];
}

// matching

// The percept triggers the SpeechHeard seed.
function triggersSpeechHeard() {
  return view.event === 'bot.ability.listening.speech.output';
}

// seeding

function triedWithSeed() {
  return tried.concat(['SpeechHeard']);
}

function silenceDelay() {
  return silence_ms + 'ms';
}

// seeding: the seed answered the turn with at least
// one selection (an empty one is unhandled).
function isBehaviorHandled(event) {
  return event.data.turn === turn && event.data.handled === true && Array.isArray(event.data.selected) && event.data.selected.length > 0;
}

// Why the seed finished without handling the turn, as the candidates
// runner logs a learned candidate's (decision D224).
function seedAdvanceLog(event) {
  var d = event.data;
  var why = d === null || typeof d !== 'object' ? 'no donedata object'
    : d.turn !== turn ? 'donedata turn ' + JSON.stringify(d.turn) + ' is not the turn ' + turn
    : d.handled !== true ? 'handled ' + JSON.stringify(d.handled)
    : 'no selections';
  return 'SpeechHeard (' + why + ')';
}

function seedSilentLog() {
  return 'SpeechHeard after ' + silence_ms + ' ms';
}

// Why the seed failed: the error event's name.
function seedFailedReason(event) {
  return 'Autonomy native seed \'SpeechHeard\' failed: ' + event.name;
}

// inventory

// database null means no learned inventory.
function hasNoMemory() {
  return database === null;
}

function noMemoryLog() {
  return 'turn ' + turn + ': no memory';
}

// loading

// One statement: the ACTIVE turn behaviors the percept's event triggers.
function behaviorSelectStatements() {
  return [{sql: select_sql, parameters: [view.event]}];
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
  return 'turn ' + turn + ': ' + rows.length + ' matched for ' + view.event;
}

function hasNoMatch() {
  return rows.length === 0;
}

// running

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

// percept_handled

function handledWithPercept() {
  return handled.concat([position]);
}

function selectedWithAnswer() {
  return selected.concat(answer);
}

// next_percept

function hasNextPercept() {
  return position + 1 < percepts.length;
}

function nextPosition() {
  return position + 1;
}

function everyPerceptHandled() {
  return handled.length === percepts.length;
}

function hasNoCandidates() {
  return tried.length === 0;
}

// done, unhandled, answered, failed

function noBehaviorsOutput() {
  return {turn: turn, handled: false, candidates: [], reason: 'no-behaviors'};
}

// Unhandled, with what the behaviors that handled some percepts did
// (acted) when any did.
function unhandledOutput() {
  var output = {turn: turn, handled: false, candidates: tried, reason: 'unhandled'};
  if (handled.length > 0) {
    output.acted = {percepts: handled, acts: selected};
  }
  return output;
}

function answeredOutput() {
  return {turn: turn, handled: true, selected: selected, candidates: tried, reason: 'behavior'};
}

function failedOutput() {
  return {turn: turn, handled: false, failed: true, candidates: tried, reason: answer};
}
