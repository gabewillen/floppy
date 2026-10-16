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
// or every participant when it names none (the original: ambient input).

function addressees(targetIds) {
  if (targetIds.length === 0) {
    return participants.slice();
  }
  return participants.filter(function (participant) { return targetIds.indexOf(participant) >= 0; });
}
