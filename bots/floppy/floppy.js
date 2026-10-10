// The ECMAScript of bots/floppy/floppy.scx (SCXML 5.8): loaded by its root
// <script src="floppy.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared: the sender of an event (its invoke id; none from outside or from
// the body itself), event records, and send delays.

function isFromOutside(_event) {
  return !_event.invokeid;
}

function isFromCognition(_event) {
  return _event.invokeid === 'cognition';
}

function isFromCommunication(_event) {
  return _event.invokeid === 'communication';
}

function isFromIdentity(_event) {
  return _event.invokeid === 'identity';
}

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

function isExpectedAbility(_event) {
  return expected.indexOf(_event.invokeid) >= 0;
}

// An expected ability's offers, sent under its own invoke id.
function isAbilityOffers(_event) {
  return isExpectedAbility(_event) && _event.data.ability === _event.invokeid;
}

function isWaiting(_event) {
  return waiting.indexOf(_event.invokeid) >= 0;
}

// waiting without the ability that sent _event.
function waitingWithout(_event) {
  return waiting.slice(0, waiting.indexOf(_event.invokeid)).concat(waiting.slice(waiting.indexOf(_event.invokeid) + 1));
}

// activating

function everyAbilityReady() {
  return waiting.length === 0;
}

// The activation timer of the current run.
function isCurrentRunTimer(_event) {
  return !_event.invokeid && Object(_event.data).run === live_run;
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
// while routing, whether the routed selection names a candidate device.
function focusDeviceEnabled(_event) {
  return offering ? focus_candidates.length > 0 : routing_selections && focus_candidates.indexOf(_event.data.device) >= 0 && devices.indexOf(_event.data.device) >= 0;
}

// While offering, whether bot.clear_focus is among the enabled transitions;
// while routing, whether the routed selection is taken.
function clearFocusEnabled() {
  return offering ? hasFocusedDevice() : routing_selections && hasFocusedDevice();
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

// A conversation output whose last message is inbound.
function isConversationProduct(_event) {
  return _event.invokeid === 'communication' && _event.data.messages.length > 0 && _event.data.messages.slice(-1)[0].direction === 'inbound';
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

// A reply from the cognition for the current turn.
function isCurrentTurnReply(_event) {
  return _event.invokeid === 'cognition' && _event.data.turn === turn;
}

function botOutput(_event) {
  return JSON.stringify({turn: turn, output: _event.data.output});
}

function processingCompleted(_event) {
  return JSON.stringify({output: _event.data.output, focus_candidates: focus_candidates, turn: turn});
}

function cognitionFailure(_event) {
  return JSON.stringify({message: typeof _event.data.message === 'string' && _event.data.message.length > 0 ? _event.data.message : 'the cognition failed', turn: turn});
}

// The body's own timer for the current turn.
function isCurrentTurnTimer(_event) {
  return !_event.invokeid && _event.data.turn === turn;
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

// cancel_timeout_ms clamped to 100..5000 ms.
function cancelDelay() {
  return delay(Math.min(5000, Math.max(100, cancel_timeout_ms)));
}

function cancelledFailure() {
  return JSON.stringify({message: 'the turn timed out after ' + processing_timeout_ms + ' ms and was cancelled', turn: turn});
}

function cancelUnacknowledgedReason() {
  return 'turn ' + turn + ': the cognition did not acknowledge its cancel';
}

// routing

function isDroppedRoute(route) {
  return route.dropped !== undefined;
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

// A routed body event no transition took.
function isUntakenRoutedEvent(_event) {
  return routing_selections && _event.type === 'internal' && _event.name !== 'floppy.routed';
}

function untakenRoutedEventLog(_event) {
  return _event.name + ' was not taken';
}
