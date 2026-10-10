// The ECMAScript of abilities/voice_detection/voice_detection.scx (SCXML 5.8): loaded by its root
// <script src="voice_detection.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// detecting: any failure ends the detection with mosfet's FailureData.

function failedResult(_event) {
  return ({message: typeof Object(_event.data).message === 'string' ? _event.data.message : 'VoiceDetection failed.'});
}

function errorResult(_event) {
  return ({message: 'VoiceDetection could not run (' + _event.name + ')'});
}

// normalizing

// The normalizer's done data is a sound when it carries an audio reference.
function isNormalizedSound(_event) {
  return typeof Object(_event.data).audio === 'string';
}

function unnormalizedResult() {
  return ({message: 'VoiceDetection could not normalize the sound.'});
}

// applying

// A detection output with its segments (D41: the service's events are
// unchecked).
function hasSegments(_event) {
  return Array.isArray(Object(_event.data).segments);
}

// The normalized sound and mosfet's ApplyData.
function detectedResult(_event) {
  return ({sound: sound, voice_detection: {segments: _event.data.segments}});
}

function mismatchedResult() {
  return ({message: 'Voice detection produced output that does not match its apply schema.'});
}

function undetectedResult() {
  return ({message: 'VoiceDetection ended without detecting.'});
}
