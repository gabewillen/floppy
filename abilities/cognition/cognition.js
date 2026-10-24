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

// awake and resting (decisions D195 and D205)

// The stretch without input after which idle rests: the bounds'
// rest_after_ms, else this document's default.
function restDelay() {
  return (typeof bounds.rest_after_ms === 'number' ? bounds.rest_after_ms : rest_after_ms) + 'ms';
}

// What the consolidation did: {folded, merged, steps, failed, reason}.
function consolidatedLog(_event) {
  var data = Object(_event.data);
  return ({folded: data.folded, merged: data.merged, steps: data.steps, failed: data.failed, reason: data.reason});
}

function consolidationFailedLog(_event) {
  return ({folded: 0, merged: 0, steps: 0, failed: true, reason: _event.name});
}

// processing

function processingLog() {
  return 'turn ' + turn + ': ' + stimulus.event;
}

function turnLog() {
  return 'turn ' + turn;
}

// A hop timer of the stage in hand, for the turn in hand.
function isChildHopTimeout(_event) {
  return (_event.sendid === hop_timer || _event.sendid === ceiling_timer) && _event.data.turn === turn;
}

function hopTimeoutLog(_event) {
  return _event.sendid === ceiling_timer
    ? 'turn ' + turn + ': ' + stage + ' after ' + ceilingMs() + ' ms in all'
    : 'turn ' + turn + ': ' + stage + ' after ' + inactivityMs() + ' ms without progress';
}

// running_autonomy, running_decision, running_intuition, running_reasoning

// The tier of the stage in hand: reflex (autonomy and the decision
// layer), intuition or reasoning.
function tier() {
  return stage === 'reasoning' || stage === 'intuition' ? stage : 'reflex';
}

// The tier's own step bound (bounds.<tier>_timeout_ms, decision D140),
// else null.
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

// The handed-back stimulus of a step that timed out: the bound that
// fired, in seconds.
function timedOutStep(_event) {
  var seconds = (_event.sendid === ceiling_timer ? ceilingMs() : inactivityMs()) / 1000;
  var message = "Cognition's " + tier() + ' step took longer than ' + seconds + ' seconds, so the turn failed.';
  var data = {message: message, stage: tier(), timeout_seconds: seconds, turn: turn, stimulus: stimulus.event};
  return ({event: 'bot.ability.cognition.step.timed_out', data: data, source: 'cognition'});
}

// The handed-back stimulus of a step that failed with message.
function failedStep(message) {
  var text = "Cognition's " + tier() + ' step failed, so the turn failed: ' + message;
  var data = {message: text, stage: tier(), turn: turn, stimulus: stimulus.event};
  return ({event: 'bot.ability.cognition.step.failed', data: data, source: 'cognition'});
}

// Whether the turn perceives a handed-back failure of one of stages.
function perceivesFailureOf(stages) {
  return perceivesFailure() && stages.indexOf(Object(stimulus.data).stage) >= 0;
}

// Whether the turn perceives a handed-back failure at all: then it hands
// none back.
function perceivesFailure() {
  return ['bot.ability.cognition.step.failed', 'bot.ability.cognition.step.timed_out'].indexOf(stimulus.event) >= 0;
}

// after_autonomy

// The decision layer takes the turn unless it perceives a handed-back
// failure of reflex or intuition, which only a later stage can perceive
// (decision D140).
function takesDecision() {
  return !perceivesFailureOf(['reflex', 'intuition']);
}

// The stage's input param, and bot.ability.failed's input.
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
// A stage's donedata acted is the selections it dispatched (an array), or
// {percepts, acts} (autonomy's, for the percepts its behaviors handled).
function actsOf(_event) {
  var given = _event.data.acted;
  return Array.isArray(given) ? given : Array.isArray(Object(given).acts) ? given.acts : [];
}

function actedAfter(_event) {
  return acted.concat(actsOf(_event));
}

// The percepts an earlier stage acted on, when it named them; else these.
function perceptsAfter(_event) {
  var given = Object(_event.data.acted).percepts;
  return Array.isArray(given) && given.length > 0 ? given : acted_percepts;
}

// What reasoning is told was already done: the acts, or {percepts, acts}
// when a stage named the percepts they were for.
function actedFact() {
  return acted_percepts === null ? acted : ({percepts: acted_percepts, acts: acted});
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

// The failure of a stage that failed: its reason, else
// 'Cognition ability failed.'.
function stageFailure(_event) {
  return (typeof _event.data.reason === 'string' && _event.data.reason.length > 0 ? _event.data.reason : 'Cognition ability failed.');
}

// The failure of a stage that failed after it started (its
// error.communication.<invokeid>).
function stageRunFailure(name, _event) {
  return 'the ' + name + ' could not run (' + _event.name + ')';
}

// running_decision

// The decision layer's confidence tuner from its donedata, else the one in hand.
function decisionTuner(_event) {
  var state = _event.data.confidence_tuner;
  return state instanceof Object && !Array.isArray(state) ? state : confidence_tuner;
}

function tunerLog() {
  return JSON.stringify(confidence_tuner);
}

// The cognition.cascaded log of a decision layer or an intuition that
// failed (decision D140).
function cascadedLog(reason) {
  return ({stage: tier(), reason: reason});
}

// The later tiers the decision layer's pass may hand the turn to: the
// chat tier when composed, then reasoning.
function handOns() {
  return intuition_model === null ? ['reasoning'] : ['intuition', 'reasoning'];
}

// The decision model's bytes a token from the decision layer's donedata,
// else the one in hand.
function decisionBytesPerTokenAfter(_event) {
  var learned = _event.data.bytes_per_token;
  return typeof learned === 'number' && learned > 0 ? learned : decision_bytes_per_token;
}

// The tier the decision layer's pass chose (its donedata's hand_to), else null.
function handTo(_event) {
  return typeof _event.data.hand_to === 'string' ? _event.data.hand_to : null;
}

// after_decision

// The intuition tier takes the turn: it is composed, the decision layer's
// pass did not choose reasoning, and the turn perceives no handed-back
// failure of intuition.
function takesIntuition() {
  return intuition_model !== null && hand_to !== 'reasoning' && !perceivesFailureOf(['intuition']);
}

// running_intuition

// The turn's choices and the deliberative handoff, offered by this ability
// (its call surface, offered to intuition only).
function intuitionChoices() {
  return choices.concat(handoff.map(function (choice) {
    return Object.assign({}, choice, {target: _sessionid});
  }));
}

// What the intuition tier learned of its model's request bytes a token.
function intuitionBytesPerTokenAfter(_event) {
  var learned = _event.data.bytes_per_token;
  return typeof learned === 'number' && learned > 0 ? learned : intuition_bytes_per_token;
}

// completing

// The reflection's cognition_input: the turn's stimulus, focus and focus_candidates.
function cognitionInput() {
  return ({stimulus: stimulus, focus: focus, focus_candidates: focus_candidates});
}

// Context fit (decision D143): the bounds' model context and the tokens
// reserved for the reply, [floppy fields]; null fits nothing.
function contextTokens() {
  return typeof bounds.max_context_tokens === 'number' ? bounds.max_context_tokens : null;
}

function reserveTokens() {
  return typeof bounds.completion_reserve_tokens === 'number' ? bounds.completion_reserve_tokens : 0;
}

// What reasoning learned of the request bytes a token, else what was known.
function bytesPerTokenAfter(_event) {
  var learned = _event.data.bytes_per_token;
  return typeof learned === 'number' && learned > 0 ? learned : bytes_per_token;
}

// alive

// What the bot knows of itself, as its prompts' self block renders it:
// its name only once the identity told it (null: told
// it has none yet), then its owner's traits and its own description.
function selfFact() {
  return name_told ? self : ({traits: self.traits, description: self.description});
}

// The persona's description (absent: none written yet).
function personaDescription(_event) {
  return _event.data.description === undefined ? null : _event.data.description;
}
