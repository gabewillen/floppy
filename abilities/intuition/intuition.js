// The ECMAScript of abilities/intuition/intuition.scx (SCXML 5.8): loaded by its root
// <script src="intuition.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// deciding

// mosfet's stimulus data mapping: the stimulus's object data, else {}.
function stimulusData() {
  return stimulus !== null && stimulus.data instanceof Object && !Array.isArray(stimulus.data) ? stimulus.data : ({});
}

function decisionTimeoutDelay() {
  return decision_timeout_ms + 'ms';
}

// mosfet's deliberate handoff event (types.DELIBERATIVE_HANDOFF_EVENT_NAME).
function isDeliberativeHandoff(name) {
  return name === 'bot.ability.reasoning.input';
}

// The choices the processor offers: the turn's, less the deliberate handoff
// (the TypeSafe processor skips deliberative handoff schemas).
function processorChoices() {
  return choices.filter(function (choice) { return !isDeliberativeHandoff(choice.event); });
}

// mosfet _stimulus_document as the TypeSafe state {"turn": ...}: the
// stimulus envelope's name and its data.
function decisionState() {
  if (stimulus === null) {
    return ({turn: {data: null}});
  }
  return ({turn: {envelope: {name: stimulus.event}, data: stimulus.data === undefined ? null : stimulus.data}});
}

// A selected event with at least one selection (the service's events are
// unchecked).
function hasSelections(_event) {
  return Array.isArray(Object(_event.data).selections) && _event.data.selections.length > 0;
}

// mosfet normalize_confidence: a probability in (0, 1] as a whole number
// on the 0-100 scale, rounded half to even as Python's round does.
function normalizedConfidence(probability) {
  var scaled = probability * 100;
  var rounded = Math.round(scaled);
  if (rounded - scaled === 0.5 && rounded % 2 !== 0) {
    rounded -= 1;
  }
  return Math.max(0, Math.min(100, rounded));
}

// mosfet selection_confidence: the minimum of the selections' confidences.
function selectionConfidence(_event) {
  return _event.data.selections.reduce(function (least, selection) {
    return Math.min(least, normalizedConfidence(Number(selection.confidence)));
  }, 100);
}

function selectionNames(_event) {
  return _event.data.selections.map(function (selection) { return selection.event; }).join(' ');
}

function tunedLog(_event) {
  return 'turn ' + turn + ': ' + selectionNames(_event) + ' (confidence ' + confidence + ', threshold ' + threshold + ')';
}

// mosfet should_escalate: the confidence is below the updated threshold
// (written as a probability).
function isLowConfidence() {
  return confidence / 100 < threshold;
}

function hasDeliberativeInput(_event) {
  return _event.data.selections.some(function (selection) { return isDeliberativeHandoff(selection.event); });
}

// The selections as the turn's product {event, data}.
function product(_event) {
  return _event.data.selections.map(function (selection) { return ({event: selection.event, data: Object(selection.data)}); });
}

// The turn's selections (mosfet's escalation reason none).
function selectionResult(_event) {
  return ({handled: true, selected: product(_event), reason: 'none', turn: turn, tuner: tuner});
}

// mosfet _environment_actions: the selections safe to hand out before the
// turn escalates, less the deliberate handoff and the cognition ignore
// (types.is_ignore_event).
function escalatedResult(reason, _event) {
  var actions = product(_event).filter(function (item) { return !isDeliberativeHandoff(item.event) && item.event !== 'bot.ability.cognition.ignore'; });
  return ({handled: false, selected: actions, reason: reason, turn: turn, tuner: tuner});
}

// An abstain with one of its reasons (the service's events are unchecked).
function isAbstainReason(_event) {
  return ['no-candidates', 'declined'].indexOf(Object(_event.data).reason) >= 0;
}

function abstainedLog(_event) {
  return 'turn ' + turn + ': abstained (' + _event.data.reason + ')';
}

// mosfet's empty product: no selection, no observation.
function abstainedResult() {
  return ({handled: false, reason: 'empty_product', turn: turn, tuner: tuner});
}

function failedCode(_event) {
  return typeof Object(_event.data).code === 'string' ? _event.data.code : 'unknown';
}

function decisionFailedResult() {
  return ({handled: false, failed: true, reason: 'the decision failed (' + failed_code + ')', turn: turn, tuner: tuner});
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

function timedOutResult() {
  return ({handled: false, failed: true, reason: 'the decision timed out after ' + decision_timeout_ms + ' ms', turn: turn, tuner: tuner});
}

function couldNotRunResult(_event) {
  return ({handled: false, failed: true, reason: 'the decision could not run (' + _event.name + ')', turn: turn, tuner: tuner});
}
