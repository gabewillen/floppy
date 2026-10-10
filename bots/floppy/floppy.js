// The ECMAScript of bots/floppy/floppy.scx (SCXML 5.8): loaded by its root
// <script src="floppy.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared: event records and send delays.

function eventDataJson(_event) {
  return JSON.stringify(_event.data);
}

function emptyDataJson() {
  return JSON.stringify({});
}

function delay(ms) {
  return ms + 'ms';
}

function abilityTarget(id) {
  return '#_' + id;
}

// inactive

function activationModel(_event) {
  return _event.data.model === undefined ? model : _event.data.model;
}

// live

function nextLiveRun() {
  return live_run + 1;
}

function expectedAbilities() {
  return expected.slice();
}

// Whether the ability whose offers _event carries is still waited for.
function isWaiting(_event) {
  return waiting.indexOf(_event.data.ability) >= 0;
}

// waiting without the ability whose offers _event carries.
function waitingWithout(_event) {
  return waiting.filter(function (ability) { return ability !== _event.data.ability; });
}

// activating

function everyAbilityReady() {
  return waiting.length === 0;
}

// The activation timer of the current run.
function isCurrentRun(_event) {
  return Object(_event.data).run === live_run;
}

function activationTimeoutDiagnostic() {
  return 'the abilities ' + JSON.stringify(waiting) + ' were not ready within ' + activation_timeout_ms + ' ms';
}

function activatingErrorDiagnostic(_event) {
  return _event.name + ' while activating' + (_event.invokeid ? ' (from ' + _event.invokeid + ')' : '');
}

function activatingDoneDiagnostic(_event) {
  return 'the ability ' + _event.invokeid + ' ended while activating';
}

// active

function abilityEndedReason(_event) {
  return 'the ability ' + _event.invokeid + ' ended while active';
}

function hasFocusedDevice() {
  return focused_device !== null;
}

// While offering, whether bot.focus_device is among the enabled transitions;
// while routing, whether it is a routed selection.
function focusDeviceEnabled() {
  return offering ? focus_candidates.length > 0 : routing_selections;
}

// Whether the routed bot.focus_device names a candidate device.
function isFocusCandidate(_event) {
  return focus_candidates.indexOf(_event.data.device) >= 0 && devices.indexOf(_event.data.device) >= 0;
}

// While offering, whether bot.clear_focus is among the enabled transitions;
// while routing, whether it is a routed selection.
function clearFocusEnabled() {
  return offering ? hasFocusedDevice() : routing_selections;
}

// unfocused, focused

function hasQueuedStimulus() {
  return queue.length > 0;
}

function queuedStimulus() {
  return queue[0];
}

// queue without its first stimulus.
function queueRest() {
  return queue.slice(1);
}

// processing

function nextTurn() {
  return turn + 1;
}

function choicesFit() {
  return !choices_over;
}

function cognitionInput() {
  return {turn: turn, stimulus: stimulus, focus: focus, focus_candidates: focus_candidates, choices: choices};
}

function choicesOverFailure() {
  return JSON.stringify({message: 'the turn\'s choices are ' + choices_size + ' bytes, over 32768', turn: turn});
}

// A reply or timer for the current turn.
function isCurrentTurn(_event) {
  return Object(_event.data).turn === turn;
}

function botOutput(_event) {
  return JSON.stringify({turn: turn, output: _event.data.output});
}

function processingCompleted() {
  return JSON.stringify({output: selections, focus_candidates: focus_candidates, turn: turn});
}

function cognitionFailure(_event) {
  return JSON.stringify({message: typeof _event.data.message === 'string' && _event.data.message.length > 0 ? _event.data.message : 'Bot processing ability failed.', turn: turn});
}

function turnJson() {
  return JSON.stringify({turn: turn});
}

// queue with the pending stimulus appended.
function queueWithPending() {
  return queue.concat([pending]);
}

function deferredCount() {
  return String(queue.length);
}

// cancelling_processing

// processing_timeout_ms clamped to 100..5000 ms (mosfet's cancel wait).
function cancelDelay() {
  return delay(Math.min(5000, Math.max(100, processing_timeout_ms)));
}

// mosfet _dispatch_processing_timeout_failure, after an acknowledged cancel
// and after an unacknowledged one alike.
function cancelledFailure() {
  return JSON.stringify({message: 'Bot processing timed out after ' + processing_timeout_ms / 1000 + ' seconds.', turn: turn});
}

function cancelUnacknowledgedReason() {
  return 'turn ' + turn + ': the cognition did not acknowledge its cancel';
}

// routing

// No selection was rejected (by <bot:verdict> or <decision:route>).
function isAccepted() {
  return rejection === null;
}

// The turn's choices, and bot.ability.communication.input placed on the
// body itself (the trusted SpeechHeard seed's selection, mosfet's declared
// event of the bot), which takes it as outside communication input.
function routeChoices() {
  return choices.concat([{event: 'bot.ability.communication.input', schema: {}}]);
}

// mosfet: a rejected selection fails the turn (bot.processing.failed).
function rejectionFailure() {
  return JSON.stringify({message: rejection, turn: turn});
}

function isBodyRoute(route) {
  return route.target === null;
}

function bodyRouteLog(route) {
  return route.event + ' -> bot';
}

function abilityRouteLog(route) {
  return route.event + ' -> ' + route.target;
}

// active (after its states)

function staleReplyLog(_event) {
  return _event.name + ' for turn ' + _event.data.turn + ' (current ' + turn + ')';
}
