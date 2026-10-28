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

// seeding and candidate: the seed or candidate answered the turn with at
// least one selection (an empty one is unhandled).
function isBehaviorHandled(event) {
  return unhandledReason(event.data) === null;
}

// Why the seed finished without handling the turn, the reason
// candidateAdvanceLog gives for a learned candidate.
function seedAdvanceLog(event) {
  return 'SpeechHeard (' + unhandledReason(event.data) + ')';
}

// Why a donedata does not handle this turn (not an object, another turn,
// not handled, or no selections), or null when it handles it.
function unhandledReason(d) {
  if (d === null || typeof d !== 'object') {
    return 'no donedata object';
  }
  if (d.turn !== turn) {
    return 'donedata turn ' + JSON.stringify(d.turn) + ' is not the turn ' + turn;
  }
  if (d.handled !== true) {
    return 'handled ' + JSON.stringify(d.handled);
  }
  if (!Array.isArray(d.selected) || d.selected.length === 0) {
    return 'no selections';
  }
  return null;
}

function seedSilentLog() {
  return 'SpeechHeard after ' + silence_ms + ' ms';
}

// Why the seed failed: the error event's name.
function seedFailedReason(event) {
  return 'Autonomy native seed \'SpeechHeard\' failed: ' + event.name;
}

// inventory

// memory null means no learned inventory.
function hasNoMemory() {
  return memory === null;
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

// The matched rows as the candidates, [{name, source}] in inventory order.
function candidateRows() {
  return rows.map(function (row) { return {name: row.values[0], source: row.values[1]}; });
}

// candidate: the first of the candidates left.

function candidateName() {
  return candidates[0].name;
}

// The path of the candidate's program (its source).
function candidateSource() {
  return candidates[0].source;
}

function triedWithCandidate() {
  return tried.concat([candidateName()]);
}

// Why a candidate that finished did not handle the turn.
function candidateAdvanceLog(event) {
  return candidateName() + ' (' + unhandledReason(event.data) + ')';
}

function candidateSilentLog() {
  return candidateName() + ' after ' + silence_ms + ' ms';
}

function candidateFailedLog(event) {
  return candidateName() + ' (' + event.name + ')';
}

// dispatching and recorded_failure

// The data a selection is sent with: its data object, else {} (no data,
// null, or a non-object).
function selectionData(s) {
  return s.data !== null && typeof s.data === 'object' && !Array.isArray(s.data) ? s.data : ({});
}

// A dispatch that failed, with the error and what the candidate selected
// (an error.execution is most often data its target's schema refuses,
// decision D221).
function dispatchFailure(_event) {
  return 'Autonomy behavior dispatch failed: ' + _event.name + ' (selected ' + (Array.isArray(answer) && answer.length > 0 ? answer.map(selectionText).join(', ') : 'nothing') + ')';
}

function selectionText(s) {
  var selection = Object(s);
  return String(selection.event) + (typeof selection.target === 'string' ? ' to ' + selection.target : ' with no target');
}

function isRejected() {
  return rejection !== null;
}

function candidateRejectedLog() {
  return candidateName() + ' (' + rejection + ')';
}

// using and recording_failure

// The handled candidate's use.
function usedPersistenceStatements() {
  return [{sql: used_sql, parameters: [candidateName()]}];
}

// The candidate's failure.
function failedPersistenceStatements() {
  return [{sql: failed_sql, parameters: [candidateName()]}];
}

function usageFailedReason(detail) {
  return 'Autonomy behavior usage failed: ' + detail;
}

function usageTimedOutReason() {
  return 'Autonomy behavior usage timed out after ' + memory_timeout_ms + ' ms';
}

// advancing

function hasRest() {
  return candidates.length > 1;
}

function restCandidates() {
  return candidates.slice(1);
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

// memory

// Whether the event answers the request the state `name` last sent the
// memory (decision D266): its id is that request's.
function isMemoryReply(name) {
  return memory_requests[name] !== undefined && _event.data.id === memory_requests[name];
}
