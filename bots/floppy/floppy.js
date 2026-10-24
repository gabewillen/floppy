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

// The identity's and the persona's bring-up budget: half the activation
// timeout, so a memory that never answers leaves them awake on their
// fallbacks before the activation gives up on them.
function bringUpTimeout() {
  return Math.floor(activation_timeout_ms / 2);
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

// The abilities expected ready: seeing too while it is on.
function expectedAbilities() {
  return seeing_enabled ? expected.concat(['seeing']) : expected.slice();
}

// Whether the eye is off: its ending is then ignored.
function isSeeingOff() {
  return !seeing_enabled;
}

// Whether the offers _event carries are taken: seeing's only while it is on.
function isTakenOffer(_event) {
  return seeing_enabled || _event.data.ability !== 'seeing';
}

// Whether the ability whose offers _event carries is still waited for.
function isWaiting(_event) {
  return waiting.indexOf(_event.data.ability) >= 0;
}

// waiting without the ability whose offers _event carries.
function waitingWithout(_event) {
  return waiting.filter(function (ability) { return ability !== _event.data.ability; });
}

// waiting without the ability that sent _event, its readiness (decision
// D230): a bare bot.ability.ready names its sender by its invoke id only.
function waitingWithoutSender(_event) {
  return waiting.filter(function (ability) { return ability !== _event.invokeid; });
}

// The abilities an environment.visual goes to: seeing after the input
// abilities while it is on.
function visualInput() {
  return seeing_enabled ? input.concat(['seeing']) : input;
}

// The abilities' offers a turn may select from: every ability's but the
// persona's, whose revision only reflection may select (the cognition
// shows the persona every turn and lets reflection alone revise it).
function turnOffers() {
  var turn = Object.assign({}, offers);
  delete turn.persona;
  return turn;
}

// Whether what an ability offers, read from its live session this turn,
// differs from its latest pushed offers (decision D230, while both are
// read): the reads are canonical, so equal offers stringify alike.
function offersDiffer(ability) {
  return JSON.stringify(read_offers[ability]) !== JSON.stringify(offers[ability] === undefined ? [] : offers[ability]);
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

// started (the firmware)

// The firmware's document: the owner's, else the firmware of a bot that
// has none.
function firmwareSrc() {
  return firmware_src === null ? 'firmware/firmware.scx' : firmware_src;
}

function hasFirmware() {
  return firmware_src !== null;
}

// The firmware's address, <_sessionid>.firmware (decision D110).
function firmwareTarget() {
  return '#_scxml_' + _sessionid + '.firmware';
}

// The firmware has sent its first offers this run.
function isFirmwareStarted() {
  return offers.firmware !== undefined;
}

function firmwareTimeoutDiagnostic() {
  return 'the firmware did not start within ' + firmware_timeout_ms + ' ms';
}

// active

function abilityEndedReason(_event) {
  return 'the ability ' + _event.invokeid + ' ended while active';
}

function hasFocusedDevice() {
  return focused_device !== null;
}

// The held device a bot.input came from (its target_device), else null.
function inputDevice(_event) {
  return devices.indexOf(_event.data.target_device) >= 0 ? _event.data.target_device : null;
}

// Whether bot.focus_device applies: the turn has focus candidates.
function focusDeviceEnabled() {
  return focus_candidates.length > 0;
}

// Whether the routed bot.focus_device names a candidate device.
function isFocusCandidate(_event) {
  return focus_candidates.indexOf(_event.data.device) >= 0 && devices.indexOf(_event.data.device) >= 0;
}

// The short-term register's reads: one more asked, one answered, and
// whether the answer arriving is to the last read asked.
function turnReadAsked() {
  return turn_reads + 1;
}

function turnReadAnswered() {
  return Math.max(0, turn_reads - 1);
}

function isLastTurnRead() {
  return turn_reads === 1;
}

// The register's answer to a read it could not serve (stm.scx's failure
// message for the request), which answers that read.
function isTurnReadFailure(_event) {
  return _event.data.message.indexOf('short-term memory bot.ability.memory.stm.recent_turns failed') === 0;
}

// The short-term register's record of an admitted stimulus: its name, its
// data's bounded projection (stm_payload), and the source a sound names
// (the sound carries it in place of an envelope source); floppy has no
// envelope id or target.
function stmRecord(name, _event) {
  const record = {stimulus_name: name, payload: stm_payload};
  if (typeof _event.data.source === 'string' && _event.data.source !== '') {
    record.source = _event.data.source;
  }
  return record;
}

// Percepts

// held with a bot.input as its percept: the envelope, no source (it reaches
// the body from outside its session), the participants its device stamped.
function heldWithInput(_event) {
  var stimulus = {event: _event.name, data: _event.data, source: null};
  return held.concat([{stimulus: stimulus, source: null, participants: _event.data.participants || []}]);
}

// held with a handoff's stimulus as its percept, as its producer gave it.
function heldWithHandoff(_event) {
  var stimulus = _event.data.stimulus;
  var source = typeof stimulus.source === 'string' ? stimulus.source : null;
  return held.concat([{stimulus: stimulus, source: source, participants: _event.data.participants || []}]);
}

function holdsPercepts() {
  return held.length > 0;
}

function holdsPerceptsWithFocus() {
  return held.length > 0 && focused_device !== null;
}

// The held device the first held percept came from (the reflex floor), else
// null: a bot.input's target_device; a handoff comes from no device.
function firstHeldDevice() {
  var stimulus = held[0].stimulus;
  return stimulus.event === 'bot.input' && devices.indexOf(stimulus.data.target_device) >= 0 ? stimulus.data.target_device : null;
}

// The UTF-8 length of text.
function utf8Length(text) {
  var length = 0;
  for (var i = 0; i < text.length; i++) {
    var code = text.charCodeAt(i);
    if (code < 0x80) {
      length += 1;
    } else if (code < 0x800) {
      length += 2;
    } else if (code >= 0xd800 && code < 0xdc00 && i + 1 < text.length) {
      length += 4;
      i++;
    } else {
      length += 3;
    }
  }
  return length;
}

// participants with each of more that it does not hold yet, in order.
function joinedParticipants(participants, more) {
  var joined = participants.slice();
  var keys = joined.map(function (participant) { return JSON.stringify(participant); });
  more.forEach(function (participant) {
    var key = JSON.stringify(participant);
    if (keys.indexOf(key) < 0) {
      keys.push(key);
      joined.push(participant);
    }
  });
  return joined;
}

// The turn's take of held: its longest arrival-order prefix, at least one
// percept, within 8 percepts, 65536 bytes of serialized stimuli and 32
// participants in their union. One percept is
// the turn's stimulus as itself; more are perceived together as
// bot.perceived, with the union as the turn's participants. Pure: the
// state changes are the <assign>s that read it.
function takePercepts() {
  var taken = [];
  var bytes = 0;
  var union = [];
  for (var i = 0; i < held.length && taken.length < 8; i++) {
    var size = utf8Length(JSON.stringify(held[i].stimulus));
    var joined = joinedParticipants(union, held[i].participants);
    if (taken.length > 0 && (bytes + size > 65536 || joined.length > 32)) {
      break;
    }
    taken.push(held[i]);
    bytes += size;
    union = joined;
  }
  if (taken.length === 1) {
    return {count: 1, stimulus: taken[0].stimulus, participants: taken[0].participants};
  }
  var perceived = {event: 'bot.perceived', data: {percepts: taken}, source: _sessionid};
  return {count: taken.length, stimulus: perceived, participants: union};
}

// held without the turn's take.
function heldAfterTake() {
  return held.slice(take.count);
}

// What a leaving transition releases, with its reason (deactivated,
// rebooted or degraded).
function releasedLog(reason) {
  return JSON.stringify({count: held.length, reason: reason});
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

// The turn's environment from the bot's perspective (decision D106): the
// environment's id, then the bot itself, its
// state and its own attributes (owned_devices, each held device that was
// read this turn under its reference with its id, state and attributes,
// and on, the object it sits on (decision D159), a device that is not
// running left out; held_percepts, how many percepts wait for a later
// turn; firmware, the owner's firmware read this turn, under the
// reference firmware, when it has one).
function environmentField() {
  var owned = {};
  devices.forEach(function (reference) {
    var snapshot = device_snapshots[reference];
    if (snapshot !== undefined) {
      owned[reference] = {ref: reference, id: snapshot.id, state: snapshot.state, attributes: snapshot.attributes};
      if (typeof placed_on[reference] === 'string') {
        owned[reference].on = placed_on[reference];
      }
    }
  });
  var own = {state: 'processing', owned_devices: owned, held_percepts: held.length};
  if (firmware_snapshot !== null) {
    own.firmware = {ref: 'firmware', id: firmware_snapshot.id, state: firmware_snapshot.state, attributes: firmware_snapshot.attributes};
  }
  return {id: environment, self: own};
}

// The turn's input could not be sent to the cognition.
function executionFailure(_event) {
  var reason = Object(_event.data).message;
  return {message: 'the turn\'s input could not be sent: ' + (typeof reason === 'string' ? reason : _event.name), turn: turn};
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

// processing_timeout_ms clamped to 100..5000 ms.
function cancelDelay() {
  return delay(Math.min(5000, Math.max(100, processing_timeout_ms)));
}

// The turn's timeout failure, after an acknowledged cancel and after an
// unacknowledged one alike.
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

// The data a selection is sent with: its data object, else {} (no data,
// null, or a non-object).
function selectionData(s) {
  return s.data !== null && typeof s.data === 'object' && !Array.isArray(s.data) ? s.data : ({});
}
