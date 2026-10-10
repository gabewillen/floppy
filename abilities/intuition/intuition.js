// The ECMAScript of abilities/intuition/intuition.scx (SCXML 5.8): loaded by its root
// <script src="intuition.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// deciding

// mosfet's stimulus data mapping: the stimulus's object data, else {}.
function stimulusData() {
  return stimulus !== null && stimulus.data instanceof Object && !Array.isArray(stimulus.data) ? stimulus.data : ({});
}

function thresholdLog() {
  return 'turn ' + turn + ': threshold ' + threshold;
}

function decisionTimeoutDelay() {
  return decision_timeout_ms + 'ms';
}

function decisionState() {
  return ({stimulus: stimulus, focus: focus, focus_candidates: focus_candidates});
}

// An event from the decision service invoked for this turn.
function isDecisionEvent(_event) {
  return decision_id !== null && _event.invokeid === decision_id;
}

function isDecisionAbstain(_event) {
  return isDecisionEvent(_event) && ['no-candidates', 'declined', 'below-threshold'].indexOf(Object(_event.data).reason) >= 0;
}

// mosfet's low_confidence: a below-threshold abstain with its probability.
function isLowConfidence(_event) {
  return _event.data.reason === 'below-threshold' && typeof _event.data.confidence === 'number';
}

function lowConfidenceLog(_event) {
  return 'turn ' + turn + ': low confidence ' + Math.round(_event.data.confidence * 100);
}

function lowConfidenceResult() {
  return ({handled: false, reason: 'low_confidence', turn: turn, tuner: tuner});
}

function abstainedLog(_event) {
  return 'turn ' + turn + ': abstained (' + _event.data.reason + ')';
}

function abstainedResult(_event) {
  return ({handled: false, reason: 'empty_product', turn: turn, tuner: tuner});
}

function failedCode(_event) {
  return typeof Object(_event.data).code === 'string' ? _event.data.code : 'unknown';
}

function decisionFailedResult() {
  return ({handled: false, failed: true, reason: 'the decision failed (' + failed_code + ')', turn: turn, tuner: tuner});
}

// The event the decision service chose: any of its events but done.*.
function isChosenEvent(_event) {
  return isDecisionEvent(_event) && _event.name.indexOf('done.') !== 0;
}

function chosenLog(_event) {
  return 'turn ' + turn + ': ' + _event.name;
}

// mosfet's deliberate handoff event: the turn goes on to reasoning.
function isDeliberativeInput(_event) {
  return _event.name === 'bot.ability.reasoning.input';
}

function deliberativeInputResult() {
  return ({handled: false, reason: 'deliberative_input', turn: turn, tuner: tuner});
}

// The turn's one selection {event, data} (mosfet's escalation reason none).
function selectionResult(_event) {
  return ({handled: true, selected: [{event: _event.name, data: Object(_event.data)}], reason: 'none', turn: turn, tuner: tuner});
}

// No selection was rejected.
function isAccepted() {
  return rejection === null;
}

function isRejected() {
  return rejection !== null;
}

// mosfet _selection_rejection_message: the rejection fails the stage.
function rejectedResult() {
  return ({handled: false, failed: true, reason: 'Selection rejected: ' + rejection, turn: turn, tuner: tuner});
}

function isDecisionTimeout(_event) {
  return _event.sendid === decision_timer;
}

function timedOutResult() {
  return ({handled: false, failed: true, reason: 'the decision timed out after ' + decision_timeout_ms + ' ms', turn: turn, tuner: tuner});
}

function couldNotRunResult(_event) {
  return ({handled: false, failed: true, reason: 'the decision could not run (' + _event.name + ')', turn: turn, tuner: tuner});
}
