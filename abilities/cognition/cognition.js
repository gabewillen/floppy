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

// The turn's environment (decision D106), null when the input has none.
function inputEnvironment(_event) {
  return _event.data.environment === undefined ? null : _event.data.environment;
}

function inputParticipants(_event) {
  return _event.data.participants === undefined ? [] : _event.data.participants;
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

// A hop timer of the stage in hand, for the turn in hand (the original _is_child_hop_timeout).
function isChildHopTimeout(_event) {
  return (_event.sendid === hop_timer || _event.sendid === ceiling_timer) && _event.data.turn === turn;
}

function hopTimeoutLog(_event) {
  return _event.sendid === ceiling_timer
    ? 'turn ' + turn + ': ' + stage + ' after ' + ceilingMs() + ' ms in all'
    : 'turn ' + turn + ': ' + stage + ' after ' + inactivityMs() + ' ms without progress';
}

// running_autonomy, running_decision, running_reasoning

// The reference's tier of the stage in hand: reflex (autonomy and the
// decision layer) or reasoning.
function tier() {
  return stage === 'reasoning' ? 'reasoning' : 'reflex';
}

// The tier's own step bound (bounds.<tier>_timeout_ms, the reference
// _tier_timeouts), else null.
function tierMs() {
  var ms = bounds[tier() + '_timeout_ms'];
  return ms === undefined ? null : ms;
}

// A stage's inactivity bound and its backstop: its tier's own bound, else
// the bot's bounds, else this document's defaults (decision D101).
function inactivityMs() {
  if (tierMs() !== null) return tierMs();
  return bounds.child_operation_timeout_ms === undefined ? child_operation_timeout_ms : bounds.child_operation_timeout_ms;
}

function ceilingMs() {
  if (tierMs() !== null) return tierMs();
  return bounds.child_operation_ceiling_ms === undefined ? child_operation_ceiling_ms : bounds.child_operation_ceiling_ms;
}

function hopDelay() {
  return inactivityMs() + 'ms';
}

function ceilingDelay() {
  return ceilingMs() + 'ms';
}

// The handed-back stimulus of a step that timed out (the reference
// StepTimedOutData): the bound that fired, in seconds.
function timedOutStep(_event) {
  var seconds = (_event.sendid === ceiling_timer ? ceilingMs() : inactivityMs()) / 1000;
  var message = "Cognition's " + tier() + ' step took longer than ' + seconds + ' seconds, so the turn failed.';
  var data = {message: message, stage: tier(), timeout_seconds: seconds, turn: turn, stimulus: stimulus.event};
  return ({event: 'bot.ability.cognition.step.timed_out', data: data, source: 'cognition'});
}

// The handed-back stimulus of a step that failed with message (the
// reference StepFailedData).
function failedStep(message) {
  var text = "Cognition's " + tier() + ' step failed, so the turn failed: ' + message;
  var data = {message: text, stage: tier(), turn: turn, stimulus: stimulus.event};
  return ({event: 'bot.ability.cognition.step.failed', data: data, source: 'cognition'});
}

// Whether the turn perceives a handed-back failure of one of stages (the
// reference _perceives_failure).
function perceivesFailureOf(stages) {
  return perceivesFailure() && stages.indexOf(Object(stimulus.data).stage) >= 0;
}

// Whether the turn perceives a handed-back failure at all: then it hands
// none back (the reference _hand_back_failure's bound).
function perceivesFailure() {
  return ['bot.ability.cognition.step.failed', 'bot.ability.cognition.step.timed_out'].indexOf(stimulus.event) >= 0;
}

// after_autonomy

// The decision layer takes the turn unless it perceives a handed-back
// failure of reflex or intuition, which only a later stage can perceive
// (the reference _takes_decision).
function takesDecision() {
  return !perceivesFailureOf(['reflex', 'intuition']);
}

// The stage's input param, and bot.ability.failed's input (the original _build_processing_input).
function processingInput() {
  return ({focus: focus, focus_candidates: focus_candidates});
}

function isCurrentTurn(_event) {
  return _event.data.turn === turn;
}

// The turn's output: what earlier stages acted, then the handling stage's selections.
function handledOutput(_event) {
  return acted.concat(_event.data.selected);
}

// What earlier stages acted, then what this stage dispatched before it
// passed the turn on (its donedata's acted, when it has one).
function actedAfter(_event) {
  return Array.isArray(_event.data.acted) ? acted.concat(_event.data.acted) : acted;
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

// running_decision

// The decision layer's own bound: the reflex tier's when the bot gives one,
// else its default.
function decisionTimeoutMs() {
  return bounds.reflex_timeout_ms === undefined ? decision_timeout_ms : bounds.reflex_timeout_ms;
}

// The decision layer's confidence tuner from its donedata, else the one in hand.
function decisionTuner(_event) {
  var state = _event.data.confidence_tuner;
  return state instanceof Object && !Array.isArray(state) ? state : confidence_tuner;
}

function tunerLog() {
  return JSON.stringify(confidence_tuner);
}

// The cognition.cascaded log of a decision layer that failed (the reference
// _cascade_decision_failure).
function cascadedLog(reason) {
  return ({stage: 'reflex', reason: reason});
}

// completing

// The reflection's cognition_input: the turn's stimulus, focus and focus_candidates.
function cognitionInput() {
  return ({stimulus: stimulus, focus: focus, focus_candidates: focus_candidates});
}
