// The ECMAScript of abilities/voice_identification/voice_identification.scx (SCXML 5.8): loaded by its root
// <script src="voice_identification.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// identifying: any failure ends the identification with {message}.

function failedResult(_event) {
  return ({id: request.id, message: typeof Object(_event.data).message === 'string' ? _event.data.message : 'VoiceIdentification failed.'});
}

function errorResult(_event) {
  return ({id: request.id, message: 'VoiceIdentification could not run (' + _event.name + ')'});
}

// clipping

// The invoker passed segments: no direct identification, nothing to clip.
function hasSegments() {
  return Array.isArray(segments);
}

function clipFailedResult(_event) {
  return ({id: request.id, message: 'Voice identification opening segment conversion failed: ' + Object(_event.data).message});
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
  return ({id: request.id, message: 'Voice identification requires a first voiced VAD segment.'});
}

// applying

// An identification output with its embeddings (D41: the service's events
// are unchecked).
function hasEmbeddings(_event) {
  return Array.isArray(Object(_event.data).embeddings);
}

// The segments and their embeddings.
function identifiedResult(_event) {
  return ({id: request.id, segments: segments, embeddings: _event.data.embeddings});
}

function mismatchedResult() {
  return ({id: request.id, message: 'VoiceIdentification produced output that does not match its output schema.'});
}

function unidentifiedResult() {
  return ({id: request.id, message: 'VoiceIdentification ended without identifying.'});
}

// identifying

// The segments the request names, or null for direct identification.
function requestedSegments() {
  return Array.isArray(request.segments) ? request.segments : null;
}

// A request's optional field, or null.
function requested(name) {
  return request[name] === undefined ? null : request[name];
}
