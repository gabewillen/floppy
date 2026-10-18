// The ECMAScript of abilities/candidates_runner/candidates_runner.scx (SCXML 5.8): loaded by its root
// <script src="candidates_runner.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// running

// The name of the first candidate.
function candidateName() {
  return candidates[0].name;
}

// The SCXML source of the first candidate.
function candidateSource() {
  return candidates[0].source;
}

function silenceDelay() {
  return silence_ms + 'ms';
}

// The candidate answered the turn with at least one selection (the original
// _coerce_behavior_output: an empty one is unhandled).
function isBehaviorHandled(event) {
  return event.data.turn === turn && event.data.handled === true && Array.isArray(event.data.selected) && event.data.selected.length > 0;
}

function candidateSilentLog() {
  return candidateName() + ' after ' + silence_ms + ' ms';
}

function candidateFailedLog(event) {
  return candidateName() + ' (' + event.name + ')';
}

// dispatching and recorded_failure

// A dispatch that failed: the original's dispatch failure, with the error.
function dispatchFailure(_event) {
  return 'Autonomy behavior dispatch failed: ' + _event.name;
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

// the reference _prepare_used_persistence.
function usedPersistenceStatements() {
  return [{sql: used_sql, parameters: [candidateName()]}];
}

// the reference _prepare_failed_persistence.
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

// The data a selection is sent with, as the original's dispatch_selected_events
// takes it: its data object, else {} (no data, null, or a non-object).
function selectionData(s) {
  return s.data !== null && typeof s.data === 'object' && !Array.isArray(s.data) ? s.data : ({});
}
