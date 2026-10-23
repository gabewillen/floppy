// The ECMAScript of abilities/intuition/intuition.scx (SCXML 5.8): loaded by its root
// <script src="intuition.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// deciding

function decisionTimeoutDelay() {
  return decision_timeout_ms + 'ms';
}

// The deliberate handoff event.
function isDeliberativeHandoff(name) {
  return name === 'bot.ability.reasoning.input';
}

// The cognition's ignore: cognition only, never an
// environment action.
function isIgnore(name) {
  return name === 'bot.ability.cognition.ignore';
}

// The decision's state: the stimulus as the model reads it (decision
// D150).
function decisionState() {
  return ({turn: stimulus_content});
}

// A selected event with at least one selection (the service's events are
// unchecked).
function hasSelections(_event) {
  return Array.isArray(Object(_event.data).selections) && _event.data.selections.length > 0;
}

function selectionNames(_event) {
  return _event.data.selections.map(function (selection) { return selection.event; }).join(' ');
}

function tunedLog(_event) {
  return 'turn ' + turn + ': ' + selectionNames(_event) + ' (confidence ' + confidence + ', escalate ' + escalate + ')';
}

function hasDeliberativeInput(_event) {
  return _event.data.selections.some(function (selection) { return isDeliberativeHandoff(selection.event); });
}

// The selections as the turn's product {event, data, target?} (no target
// when no single session offers the event).
function product(_event) {
  return _event.data.selections.map(function (selection) {
    var item = {event: selection.event, data: Object(selection.data)};
    return typeof selection.target === 'string' ? Object.assign(item, {target: selection.target}) : item;
  });
}

// What the stage sends: a handled product's selections, and of an
// escalating one only its environment actions (never the ignore or the
// handoff); each to its
// one target, so a selection without one is not sent.
function actedSelections(_event) {
  return product(_event).filter(function (item) {
    return typeof item.target === 'string' && (result.handled || !(isIgnore(item.event) || isDeliberativeHandoff(item.event)));
  });
}

// What every answer carries: the turn, what was sent, and the state the
// cognition keeps across turns.
function stageResult(fields) {
  return Object.assign({turn: turn, acted: acted, confidence_tuner: confidence_tuner, bytes_per_token: bytes_per_token}, fields);
}

// The turn's selections (escalation reason none).
function selectionResult(_event) {
  return stageResult({handled: true, selected: product(_event), reason: 'none'});
}

// An escalating product: unhandled, its environment actions sent before
// the next tier runs.
function escalatedResult(reason) {
  return stageResult({handled: false, reason: reason});
}

// An abstain with one of its reasons (the service's events are unchecked).
function isAbstainReason(_event) {
  return ['no-candidates', 'declined'].indexOf(Object(_event.data).reason) >= 0;
}

function abstainedLog(_event) {
  return 'turn ' + turn + ': abstained (' + _event.data.reason + ')';
}

// An empty product: no selection, no observation; a pass
// names the tier it hands the turn to.
function abstainedResult(_event) {
  var empty = stageResult({handled: false, reason: 'empty_product'});
  return typeof _event.data.hand_to === 'string' ? Object.assign(empty, {hand_to: _event.data.hand_to}) : empty;
}

function failedCode(_event) {
  return typeof Object(_event.data).code === 'string' ? _event.data.code : 'unknown';
}

function decisionFailedResult() {
  return stageResult({handled: false, failed: true, reason: 'the decision failed (' + failed_code + ')'});
}

function timedOutResult() {
  return stageResult({handled: false, failed: true, reason: 'the decision timed out after ' + decision_timeout_ms + ' ms'});
}

function couldNotRunResult(_event) {
  return stageResult({handled: false, failed: true, reason: 'the decision could not run (' + _event.name + ')'});
}

// Whether the decision has answered.
function hasResult() {
  return result !== null;
}

// The fewest bytes a token any answer showed: the request's bytes over the tokens the backend
// counted, from the service's done data (unchecked); a count that is not
// positive teaches nothing.
function learnedBytesPerToken(_event) {
  var done = Object(_event.data);
  var bytes = done.request_bytes;
  var tokens = Object(done.usage).input_tokens;
  return typeof bytes === 'number' && typeof tokens === 'number' && bytes > 0 && tokens > 0 ? Math.min(bytes_per_token, bytes / tokens) : bytes_per_token;
}

// The data a selection is sent with (decision D113): its data object, else {} (no data, null, or a non-object).
function selectionData(s) {
  return s.data !== null && typeof s.data === 'object' && !Array.isArray(s.data) ? s.data : ({});
}
