// The ECMAScript of environments/room/room.scx (SCXML 5.8): loaded by its root
// <script src="room.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared: event records and send targets.

function eventDataJson(_event) {
  return JSON.stringify(_event.data);
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
