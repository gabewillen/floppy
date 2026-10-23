// The ECMAScript of abilities/speech_decoding/speech_decoding.scx (SCXML 5.8): loaded by its root
// <script src="speech_decoding.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// applying

// A decoding output with words (D41: the service's events are unchecked).
function hasDecodedText(_event) {
  return typeof Object(_event.data).text === 'string';
}

// The decoded speech, as words.
function decodedOutput(_event) {
  return ({text: _event.data.text});
}

// The failure: the service's message, or a fixed one.
function failedOutput(_event) {
  return ({message: typeof Object(_event.data).message === 'string' ? _event.data.message : 'SpeechDecoding failed.'});
}

function mismatchedOutput() {
  return ({message: 'SpeechDecoding produced output that does not match its output schema.'});
}

function undecodedOutput() {
  return ({message: 'SpeechDecoding ended without decoding.'});
}

function errorOutput(_event) {
  return ({message: 'SpeechDecoding could not run (' + _event.name + ')'});
}
