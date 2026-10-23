// The ECMAScript of abilities/voice_diarization/voice_diarization.scx (SCXML 5.8): loaded by its root
// <script src="voice_diarization.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// applying

// A diarization output with its segments (D41: the service's events are
// unchecked).
function hasSegments(_event) {
  return Array.isArray(Object(_event.data).segments);
}

// The clipped segments, as the service answered them.
function diarizedResult(_event) {
  return ({segments: _event.data.segments});
}

// The failure: the service's message, or a fixed one.
function failedResult(_event) {
  return ({message: typeof Object(_event.data).message === 'string' ? _event.data.message : 'VoiceDiarization failed.'});
}

function mismatchedResult() {
  return ({message: 'VoiceDiarization produced output that does not match its output schema.'});
}

function undiarizedResult() {
  return ({message: 'VoiceDiarization ended without diarizing.'});
}

function errorResult(_event) {
  return ({message: 'VoiceDiarization could not run (' + _event.name + ')'});
}
