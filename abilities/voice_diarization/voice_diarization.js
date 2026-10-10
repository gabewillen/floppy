// The ECMAScript of abilities/voice_diarization/voice_diarization.scx (SCXML 5.8): loaded by its root
// <script src="voice_diarization.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// applying

function isFromDiarizer(_event) {
  return _event.invokeid === 'diarizer';
}

function isDiarizerOutput(_event) {
  return _event.invokeid === 'diarizer' && Array.isArray(Object(_event.data).segments);
}

// mosfet's OutputData.
function diarizedResult(_event) {
  return ({segments: _event.data.segments});
}

// mosfet's FailureData.
function failedResult(_event) {
  return ({message: typeof Object(_event.data).message === 'string' ? _event.data.message : 'VoiceDiarization failed.'});
}

// Anything else the diarizer sends but its done event.
function isOtherDiarizerEvent(_event) {
  return _event.invokeid === 'diarizer' && _event.name.indexOf('done.') !== 0;
}

function mismatchedResult() {
  return ({message: 'Voice diarization produced output that does not match its apply schema.'});
}

function undiarizedResult() {
  return ({message: 'VoiceDiarization ended without diarizing.'});
}

function errorResult(_event) {
  return ({message: 'VoiceDiarization could not run (' + _event.name + ')'});
}
