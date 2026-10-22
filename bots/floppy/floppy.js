// The ECMAScript of bots/floppy/floppy.scx (SCXML 5.8): loaded by its root
// <script src="floppy.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared: event records and send delays.

function emptyData() {
  return {};
}

function delay(ms) {
  return ms + 'ms';
}

function abilityTarget(id) {
  return '#_' + id;
}

// live

// The references of the devices the bot holds (decision D105).
function heldDevices() {
  return Object.keys(owned_devices);
}

// A held device's address (decision D110).
function deviceTarget(reference) {
  return '#_scxml_' + owned_devices[reference];
}

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

// The abilities' offers a turn may select from: every ability's but the
// persona's, whose revision only reflection may select (the original's
// Cognition, which shows the persona every turn and lets reflection alone
// revise it).
function turnOffers() {
  var turn = Object.assign({}, offers);
  delete turn.persona;
  return turn;
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

// Whether bot.focus_device applies: the turn has focus candidates.
function focusDeviceEnabled() {
  return focus_candidates.length > 0;
}

// Whether the routed bot.focus_device names a candidate device.
function isFocusCandidate(_event) {
  return focus_candidates.indexOf(_event.data.device) >= 0 && devices.indexOf(_event.data.device) >= 0;
}

// processing

function nextTurn() {
  return turn + 1;
}

function choicesFit() {
  return !choices_over;
}

function cognitionInput() {
  return {turn: turn, stimulus: stimulus, focus: focus, focus_candidates: focus_candidates, participants: participants, choices: choices, environment: environmentField()};
}

// The participants a handoff's producer stamped (none when it names none).
function handoffParticipants(_event) {
  return _event.data.participants || [];
}

// The turn's environment from the bot's perspective (decision D106), in
// the original's layout: the environment's id, then the bot itself, its
// state and its own attributes (owned_devices), each held device that was
// read this turn under its reference with its id, state and attributes (a
// device that is not running is left out).
function environmentField() {
  var owned = {};
  devices.forEach(function (reference) {
    var snapshot = device_snapshots[reference];
    if (snapshot !== undefined) {
      owned[reference] = {ref: reference, id: snapshot.id, state: snapshot.state, attributes: snapshot.attributes};
    }
  });
  return {id: environment, self: {state: 'processing', owned_devices: owned}};
}

function choicesOverFailure() {
  return {message: 'the turn\'s choices are ' + choices_size + ' bytes, over 32768', turn: turn};
}

// A reply or timer for the current turn.
function isCurrentTurn(_event) {
  return Object(_event.data).turn === turn;
}

function botOutput(_event) {
  return JSON.stringify({turn: turn, output: _event.data.output});
}

function processingCompleted() {
  return {output: selections, focus_candidates: focus_candidates, turn: turn};
}

function cognitionFailure(_event) {
  return {message: typeof _event.data.message === 'string' && _event.data.message.length > 0 ? _event.data.message : 'Bot processing ability failed.', turn: turn};
}

function turnData() {
  return {turn: turn};
}

// cancelling_processing

// processing_timeout_ms clamped to 100..5000 ms (the original's cancel wait).
function cancelDelay() {
  return delay(Math.min(5000, Math.max(100, processing_timeout_ms)));
}

// The original _dispatch_processing_timeout_failure, after an acknowledged cancel
// and after an unacknowledged one alike.
function cancelledFailure() {
  return {message: 'Bot processing timed out after ' + processing_timeout_ms / 1000 + ' seconds.', turn: turn};
}

function cancelUnacknowledgedReason() {
  return 'turn ' + turn + ': the cognition did not acknowledge its cancel';
}

// The turn's output

// A selection that names no target: the trusted seed's, the body's own.
function hasNoTarget(s) {
  return typeof s.target !== 'string';
}

// active (after its states)

function staleReplyLog(_event) {
  return _event.name + ' for turn ' + _event.data.turn + ' (current ' + turn + ')';
}

// The data a selection is sent with, as the original's dispatch_selected_events
// takes it: its data object, else {} (no data, null, or a non-object).
function selectionData(s) {
  return s.data !== null && typeof s.data === 'object' && !Array.isArray(s.data) ? s.data : ({});
}
