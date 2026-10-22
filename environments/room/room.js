// The ECMAScript of environments/room/room.scx (SCXML 5.8): loaded by its root
// <script src="room.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared: event records and send targets.

// An event's data as JSON for its record. A voice embedding is biometric
// and never written (the original's capture writes a Biometric as
// <biometric:ClassName>, its VoiceEmbedding included), at any depth: an
// identity in source_ids or target_ids that is an array of numbers, and
// an embedding's JSON inside a reference (a *_ref the conversation
// builds from its participants' identities), are written as
// <biometric:VoiceEmbedding>.
function eventDataJson(_event) {
  var marker = '<biometric:VoiceEmbedding>';
  return JSON.stringify(_event.data, function (key, value) {
    if ((key === 'source_ids' || key === 'target_ids') && Array.isArray(value)) {
      return value.map(function (id) { return Array.isArray(id) ? marker : id; });
    }
    return typeof value === 'string' && /_ref$/.test(key) ? value.replace(/\[-?\d[\d.eE+-]*(,-?\d[\d.eE+-]*)*\]/g, marker) : value;
  });
}

function participantTarget(id) {
  return '#_' + id;
}

// The record of a leave: the event's invokeid and data.

function leaveJson(_event) {
  return JSON.stringify({invokeid: _event.invokeid, data: _event.data});
}

// Membership: participants without the one that left (unchanged for an
// event that names no invoke, a failed send's error.communication).

function without(id) {
  return participants.filter(function (participant) { return participant !== id; });
}

// Membership: participants with the one that (re)joined, once.

function joined(id) {
  return participants.indexOf(id) >= 0 ? participants.slice() : participants.concat([id]);
}

// Supervision (decision D158, the original's runtime Supervisor).

// The leave of _event's participant, for its supervisor: restart when it
// failed (failed: an error.communication), left degraded (its donedata
// has a reason), or left after its activation failed.
function leftData(_event, failed) {
  var id = _event.invokeid || null;
  var degraded = Object(_event.data).reason !== undefined;
  return {participant: id, restart: failed || degraded || activation_failed[id] === true};
}

// The bot that activated, from _event.invokeid (decision D108).
function activatedData(_event) {
  return {participant: _event.invokeid || null};
}

// The participant a supervision event is for.
function isFor(_event, id) {
  return _event.data.participant === id;
}

function isRestartable(_event, id) {
  return _event.data.participant === id && _event.data.restart === true;
}

function attemptsOf(id) {
  return attempts[id] === undefined ? 0 : attempts[id];
}

// Another attempt fits the budget of consecutive failed attempts.
function isWithinBudget(id) {
  return attemptsOf(id) + 1 < supervision.max_restarts;
}

function nextAttempt(id) {
  return attemptsOf(id) + 1;
}

// The doubling backoff of the current run of failed attempts, capped.
function backoffMs(id) {
  var delay = supervision.backoff_initial_ms;
  for (var i = 1; i < attemptsOf(id); i++) {
    if (delay >= supervision.backoff_max_ms / 2) {
      return supervision.backoff_max_ms;
    }
    delay *= 2;
  }
  return Math.min(delay, supervision.backoff_max_ms);
}

function backoffDelay(id) {
  return backoffMs(id) + 'ms';
}

function stableDelay() {
  return supervision.stable_after_ms + 'ms';
}

function restartedJson(id) {
  return JSON.stringify({participant: id, attempt: attemptsOf(id), delay_ms: backoffMs(id)});
}

function exhaustedJson(id) {
  return JSON.stringify({participant: id, attempts: attemptsOf(id) + 1});
}

// activation_failed with _event's bot marked.
function activationFailedOf(_event) {
  var failed = Object.assign({}, activation_failed);
  failed[_event.invokeid] = true;
  return failed;
}

// Host input: the participants a communication input's target_ids names,
// or every bot when it names none (the original: ambient input).

function addressees(targetIds) {
  if (targetIds.length === 0) {
    return bots();
  }
  return participants.filter(function (participant) { return targetIds.indexOf(participant) >= 0; });
}

// Holdings (decision D105): the bots present (holdings' keys), and the
// devices bot holds as reference (its device's invoke id) to address
// (<_sessionid>.<invoke id>, decision D110).

function bots() {
  return Object.keys(holdings).filter(function (id) { return participants.indexOf(id) >= 0; });
}

function ownedDevices(bot) {
  var owned = {};
  holdings[bot].forEach(function (device) { owned[device] = _sessionid + '.' + device; });
  return owned;
}

// Objects (decision D159): each device bot holds that sits on a known
// object (its placement's on), as reference to the object's id. A
// placement whose on names no object of objects is not carried.

function placedOn(bot) {
  var placed = {};
  holdings[bot].forEach(function (device) {
    var placement = placements[device];
    if (placement !== undefined && objects.indexOf(placement.on) >= 0) {
      placed[device] = placement.on;
    }
  });
  return placed;
}

// A device's bot.input goes to the bots that hold it; the host's (no
// invokeid) to every bot.

function inputRecipients(_event) {
  if (!_event.invokeid) {
    return bots();
  }
  return bots().filter(function (bot) { return holdings[bot].indexOf(_event.invokeid) >= 0; });
}

// A device's bot.input with target_device stamped as its reference.

function inputData(_event) {
  if (!_event.invokeid) {
    return _event.data;
  }
  return Object.assign({}, _event.data, {target_device: _event.invokeid});
}

// Sound: the original's reception (decision D103). The level of _event's
// sound where the participant id is, by the free-field law from the
// sender's placement (1 m reference, 1 cm clamp); the sound's own
// received_level_db when there is nothing to measure (no amplitude, no
// origin, or no placement).

function receivedLevelDb(id, _event) {
  var origin = placements[_event.invokeid];
  var placement = placements[id];
  var amplitude = _event.data.amplitude_db;
  if (typeof amplitude !== 'number' || origin === undefined || placement === undefined) {
    return _event.data.received_level_db;
  }
  var dx = placement.position.x - origin.position.x;
  var dy = placement.position.y - origin.position.y;
  var dz = placement.position.z - origin.position.z;
  var distance = Math.sqrt(dx * dx + dy * dy + dz * dz);
  return amplitude - 20 * Math.log10(Math.max(distance, 0.01) / 1.0);
}

// Whether the participant id hears _event's sound: always when there is
// nothing to measure, else when its level there is at least its threshold.

function hears(id, _event) {
  var origin = placements[_event.invokeid];
  var placement = placements[id];
  if (typeof _event.data.amplitude_db !== 'number' || origin === undefined || placement === undefined) {
    return true;
  }
  return receivedLevelDb(id, _event) >= placement.threshold_db;
}

// The sound as the participant id receives it: received_level_db filled
// when there is something to measure, the emitter's data otherwise.

function delivered(id, _event) {
  var level = receivedLevelDb(id, _event);
  if (level === undefined) {
    return _event.data;
  }
  return Object.assign({}, _event.data, {received_level_db: level});
}
