// The ECMAScript of abilities/voice_identification/voice_identification.scx (SCXML 5.8): loaded by its root
// <script src="voice_identification.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// identifying: any failure ends the identification with mosfet's FailureData.

function failedResult(_event) {
  return ({message: typeof Object(_event.data).message === 'string' ? _event.data.message : 'VoiceIdentification failed.'});
}

function errorResult(_event) {
  return ({message: 'VoiceIdentification could not run (' + _event.name + ')'});
}

// clipping

// The invoker passed segments: no direct identification, nothing to clip.
function hasSegments() {
  return Array.isArray(segments);
}

function isFromClipper(_event) {
  return _event.invokeid === 'clipper';
}

function clipFailedResult(_event) {
  return ({message: 'Voice identification opening segment conversion failed: ' + Object(_event.data).message});
}

// The clipper's done data is a segment when it carries an audio reference.
function isClippedSegment(_event) {
  return typeof Object(_event.data).audio === 'string';
}

// The one clipped segment.
function clippedSegments(_event) {
  return [_event.data];
}

function unclippedResult() {
  return ({message: 'Voice identification requires a first voiced VAD segment.'});
}

// applying

function isIdentifierOutput(_event) {
  return _event.invokeid === 'identifier' && Array.isArray(Object(_event.data).embeddings);
}

// The segments and mosfet's OutputData.
function identifiedResult(_event) {
  return ({segments: segments, embeddings: _event.data.embeddings});
}

// Anything else the identifier sends but its done event and bot.ability.failed.
function isOtherIdentifierEvent(_event) {
  return _event.invokeid === 'identifier' && _event.name.indexOf('done.') !== 0 && _event.name !== 'bot.ability.failed';
}

function mismatchedResult() {
  return ({message: 'Voice identification produced output that does not match its apply schema.'});
}

function unidentifiedResult() {
  return ({message: 'VoiceIdentification ended without identifying.'});
}
