// The ECMAScript of abilities/vocal/vocal.scx (SCXML 5.8): loaded by its root
// <script src="vocal.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// applying

function isFromSpeech(_event) {
  return _event.invokeid === 'speech';
}

// The speech service's answer, when it is an encoding output with an audio
// reference; else the answer kept so far (D41: its events are unchecked).
function encodedAnswer(_event) {
  return _event.name === 'bot.ability.vocal.speech.encoding.output' && typeof Object(_event.data).audio === 'string' ? _event.data : encoded;
}

// The speech service's failure message, when it is a failure with one;
// else the failure kept so far.
function failureAnswer(_event) {
  return _event.name === 'bot.ability.failed' && typeof Object(_event.data).message === 'string' ? _event.data.message : failure;
}

// The service ended with an encoding.
function hasEncoded(_event) {
  return isFromSpeech(_event) && encoded !== null;
}

function failureMessage() {
  return failure === null ? 'The speech service ended without encoding.' : failure;
}

function speechRunFailure(_event) {
  return 'The speech service could not run (' + _event.name + ').';
}
