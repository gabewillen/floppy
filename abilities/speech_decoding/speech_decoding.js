// The ECMAScript of abilities/speech_decoding/speech_decoding.scx (SCXML 5.8): loaded by its root
// <script src="speech_decoding.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// applying

function isFromDecoder(_event) {
  return _event.invokeid === 'decoder';
}

function isDecoderOutput(_event) {
  return _event.invokeid === 'decoder' && typeof Object(_event.data).text === 'string';
}

// mosfet's decoded speech, as words.
function decodedOutput(_event) {
  return ({text: _event.data.text});
}

// mosfet's FailureData.
function failedOutput(_event) {
  return ({message: typeof Object(_event.data).message === 'string' ? _event.data.message : 'SpeechDecoding failed.'});
}

// Anything else the decoder sends but its done event.
function isOtherDecoderEvent(_event) {
  return _event.invokeid === 'decoder' && _event.name.indexOf('done.') !== 0;
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
