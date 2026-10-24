// The ECMAScript of abilities/candidates_runner/candidates_runner.scx (SCXML 5.8): loaded by its root
// <script src="candidates_runner.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// running

// The name of the first candidate.
function candidateName() {
  return candidates[0].name;
}

// The path of the first candidate's program (its source).
function candidateSource() {
  return candidates[0].source;
}

function silenceDelay() {
  return silence_ms + 'ms';
}

// The candidate answered the turn with at least one selection (an empty one
// is unhandled).
function isBehaviorHandled(event) {
  return event.data.turn === turn && event.data.handled === true && Array.isArray(event.data.selected) && event.data.selected.length > 0;
}

// Why a candidate that finished did not handle the turn: its donedata is
// not an object, names another turn, is not handled, or selects nothing.
function candidateAdvanceLog(event) {
  var d = event.data;
  var why = d === null || typeof d !== 'object' ? 'no donedata object'
    : d.turn !== turn ? 'donedata turn ' + JSON.stringify(d.turn) + ' is not the turn ' + turn
    : d.handled !== true ? 'handled ' + JSON.stringify(d.handled)
    : 'no selections';
  return candidateName() + ' (' + why + ')';
}

function candidateSilentLog() {
  return candidateName() + ' after ' + silence_ms + ' ms';
}

function candidateFailedLog(event) {
  return candidateName() + ' (' + event.name + ')';
}

// dispatching and recorded_failure

// A dispatch that failed, with the error and what the candidate selected
// (an error.execution is most often data its target's schema refuses,
// decision D220).
function dispatchFailure(_event) {
  return 'Autonomy behavior dispatch failed: ' + _event.name + ' (selected ' + answer.map(selectionText).join(', ') + ')';
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

function memoryTimeoutDelay() {
  return memory_timeout_ms + 'ms';
}

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

// advancing and running_rest

function hasRest() {
  return candidates.length > 1;
}

function restCandidates() {
  return candidates.slice(1);
}

function restFailedLog(event) {
  return candidates[1].name + ' (' + event.name + ')';
}

// answered, rejected, unhandled, passed

function answeredOutput() {
  return {tried: [candidateName()], handled: true, selected: answer, rejection: null};
}

function rejectedOutput() {
  return {tried: [candidateName()], handled: false, selected: null, rejection: rejection};
}

function unhandledOutput() {
  return {tried: [candidateName()], handled: false, selected: null, rejection: null};
}

// The rest's output, with this runner's candidate first in tried.
function passedOutput() {
  return Object.assign({}, rest_output, {tried: [candidateName()].concat(rest_output.tried)});
}

// The data a selection is sent with: its data object, else {} (no data,
// null, or a non-object).
function selectionData(s) {
  return s.data !== null && typeof s.data === 'object' && !Array.isArray(s.data) ? s.data : ({});
}
