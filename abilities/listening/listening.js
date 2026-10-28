// The ECMAScript of abilities/listening/listening.scx (SCXML 5.8): loaded by its root
// <script src="listening.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared: send delays.

function delay(ms) {
  return ms + 'ms';
}

// live

function childCouldNotRunLog(_event) {
  return 'the ' + _event.invokeid + ' child could not run (' + _event.name + ')';
}

function childEndedLog(_event) {
  return 'the ' + _event.invokeid + ' child ended';
}

// initializing

function everyChildReady(ready) {
  return ready.sensitivity === true && ready.interpretation === true;
}

// A scored sound for the transcription: while it streams, unless the
// sound is labelled with a kind (the interpretation classifies it).
function streamsSound(scored) {
  return transcribing && scored.sound.kind == null;
}

// The transcription's streaming session is ready.
function isTranscription(_event) {
  return _event.invokeid === 'transcription';
}

function childCouldNotStartLog(_event) {
  return 'a child could not start (' + _event.name + ')';
}

// Sensing

// The sound's envelope. Its source is the transducer the sound came off
// (the mouth the sensitivity compares it with); a sound that names none
// gets an envelope without one.
function scoreRequest(sound) {
  return ({stimulus: Object.assign({event: 'environment.sound', data: sound}, soundSource(sound))});
}

function soundSource(sound) {
  return typeof sound.source === 'string' ? {source: sound.source} : {};
}

function sensitivityTimedOut() {
  return ({stage: 'sensitivity', message: 'Listening sensitivity timed out.'});
}

// The participants a speech product carries.
function productParticipants(_event) {
  return _event.data.participants || [];
}
