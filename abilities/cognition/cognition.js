// The ECMAScript of abilities/cognition/cognition.scx (SCXML 5.8): loaded by its root
// <script src="cognition.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// idle

function inputFocus(_event) {
  return _event.data.focus === undefined ? null : _event.data.focus;
}

function inputFocusCandidates(_event) {
  return _event.data.focus_candidates === undefined ? [] : _event.data.focus_candidates;
}

function inputChoices(_event) {
  return _event.data.choices === undefined ? [] : _event.data.choices;
}

// processing

function processingLog() {
  return 'turn ' + turn + ': ' + stimulus.event;
}

function turnLog() {
  return 'turn ' + turn;
}

// The hop timer of the stage in hand, for the turn in hand (the original _is_child_hop_timeout).
function isChildHopTimeout(_event) {
  return _event.sendid === hop_timer && _event.data.turn === turn;
}

function hopTimeoutLog() {
  return 'turn ' + turn + ': ' + stage + ' after ' + child_operation_timeout_ms + ' ms';
}

// running_autonomy, running_intuition, running_reasoning

function hopDelay() {
  return child_operation_timeout_ms + 'ms';
}

// The stage's input param, and bot.ability.failed's input (the original _build_processing_input).
function processingInput() {
  return ({focus: focus, focus_candidates: focus_candidates});
}

function isCurrentTurn(_event) {
  return _event.data.turn === turn;
}

// A done.invoke.<stage> that handled the turn in hand with its selections.
function isHandled(_event) {
  return _event.data.turn === turn && _event.data.handled === true && _event.data.failed !== true && Array.isArray(_event.data.selected);
}

// A done.invoke.<stage> that did not handle the turn in hand, and did not fail.
function isUnhandled(_event) {
  return _event.data.turn === turn && typeof _event.data.handled === 'boolean' && _event.data.failed !== true;
}

// Any well-formed done.invoke.<stage> for the turn in hand (after isHandled and isUnhandled: a failure).
function matchesStageOutput(_event) {
  return _event.data.turn === turn && typeof _event.data.handled === 'boolean';
}

// The cognition.stage log of an outcome with the stage's reason.
function stageLog(outcome, _event) {
  return outcome + ' (' + _event.data.reason + ')';
}

// The failure of a stage that failed: its reason, else the original's
// 'Cognition ability failed.' (_fail_child).
function stageFailure(_event) {
  return (typeof _event.data.reason === 'string' && _event.data.reason.length > 0 ? _event.data.reason : 'Cognition ability failed.');
}

// The failure of a stage that failed after it started (its
// error.communication.<invokeid>).
function stageRunFailure(name, _event) {
  return 'the ' + name + ' could not run (' + _event.name + ')';
}

// running_intuition

// The intuition's tuner from its donedata, else the one in hand.
function intuitionTuner(_event) {
  return _event.data.tuner instanceof Object && !Array.isArray(_event.data.tuner) ? _event.data.tuner : tuner;
}

function tunerLog() {
  return JSON.stringify(tuner);
}

function intuitionFailure(_event) {
  return stageFailure(_event);
}

// An escalated intuition's environment actions (the original _environment_actions), else none.
function environmentActions(_event) {
  return Array.isArray(_event.data.selected) ? _event.data.selected : [];
}

// completing

// The turn's output: an escalated intuition's environment actions first,
// then the handling stage's selections that do not repeat one of them
// (the same event and data: intuition names no target, and an event the
// body routes without one has exactly one offerer; decision D98).
function turnOutput() {
  var routed = environment_actions.map(selectionKey);
  return environment_actions.concat(selected.filter(function (s) { return routed.indexOf(selectionKey(s)) < 0; }));
}

// A selection's event and data as one string, its objects' keys sorted.
function selectionKey(s) {
  return canonical([s.event, s.data === undefined ? null : s.data]);
}

function canonical(v) {
  if (Array.isArray(v)) {
    return '[' + v.map(canonical).join(',') + ']';
  }
  if (v !== null && typeof v === 'object') {
    return '{' + Object.keys(v).sort().map(function (k) { return JSON.stringify(k) + ':' + canonical(v[k]); }).join(',') + '}';
  }
  return JSON.stringify(v);
}

// The reflection's cognition_input: the turn's stimulus, focus and focus_candidates.
function cognitionInput() {
  return ({stimulus: stimulus, focus: focus, focus_candidates: focus_candidates});
}
