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

// The hop timer of the stage in hand, for the turn in hand (mosfet _is_child_hop_timeout).
function isChildHopTimeout(_event) {
  return _event.sendid === hop_timer && _event.data.turn === turn;
}

function hopTimeoutLog() {
  return 'turn ' + turn + ': ' + stage + ' after ' + hop_timeout_ms + ' ms';
}

// running_autonomy, running_intuition, running_reasoning

function hopDelay() {
  return hop_timeout_ms + 'ms';
}

// The stage's input param, and bot.ability.failed's input (mosfet _build_processing_input).
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

// The failure of a stage that failed: its reason, else 'the stage failed'.
function stageFailure(_event) {
  return (typeof _event.data.reason === 'string' && _event.data.reason.length > 0 ? _event.data.reason : 'the stage failed');
}

function isFromChild(_event, invokeid) {
  return _event.invokeid === invokeid;
}

// The failure of a stage that could not run (an error.* from it).
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
  return 'the intuition failed: ' + stageFailure(_event);
}
