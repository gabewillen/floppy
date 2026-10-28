// The ECMAScript of abilities/voice_affect/voice_affect.scx (SCXML 5.8): loaded by its root
// <script src="voice_affect.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// applying

// A segment's length in seconds, null when it has no span.
function segmentSeconds(segment) {
  var start = Object(segment).start_seconds;
  var end = Object(segment).end_seconds;
  return typeof start === 'number' && typeof end === 'number' ? end - start : null;
}

// The longest segment with audio (the first of equals; one with no span is
// long enough), or null when none is at least min_segment_seconds.
function longestSegment() {
  var longest = null;
  var items = request.segments;
  for (var index = 0; index < items.length; index++) {
    var item = items[index];
    var seconds = segmentSeconds(item);
    if (typeof Object(item).audio !== 'string' || (seconds !== null && seconds < min_segment_seconds)) {
      continue;
    }
    if (longest === null || (seconds !== null && segmentSeconds(longest) !== null && seconds > segmentSeconds(longest))) {
      longest = item;
    }
  }
  return longest;
}

function timeoutDelay() {
  return timeout_ms + 'ms';
}

function segmentMediaType() {
  return typeof segment.media_type === 'string' ? segment.media_type : null;
}

function segmentRate() {
  return typeof segment.sample_rate_hz === 'number' ? segment.sample_rate_hz : null;
}

function segmentChannels() {
  return typeof segment.channels === 'number' ? segment.channels : null;
}

function isHeard() {
  return result !== null;
}

function unheardFailure() {
  return ({id: request.id, message: reason});
}

// An affect as the service answers it (D41: the service's events are
// unchecked).
function isAffect(_event) {
  return typeof Object(_event.data).label === 'string';
}

function heardResult(_event) {
  var data = _event.data;
  return ({id: request.id, label: data.label, scores: Object(data.scores), valence: data.valence === undefined ? null : data.valence,
    confidence: data.confidence === undefined ? null : data.confidence});
}

function heardLog() {
  return 'heard ' + result.label + (typeof result.confidence === 'number' ? ' ' + result.confidence : '');
}

function hasNoSegment() {
  return segment === null;
}

// The logs (listening.affect): what was heard, or why nothing was.
function skippedLog() {
  return 'skipped: no segment of at least ' + min_segment_seconds + ' s';
}

function failedLog(_event) {
  return 'failed: ' + _event.data.message;
}

function timedOutLog() {
  return 'failed: not heard within ' + timeout_ms + ' ms';
}

function endedLog() {
  return 'failed: the classifier ended without an affect';
}

function errorLog(_event) {
  return 'failed: the classifier could not run (' + _event.name + ')';
}

function mismatchedLog() {
  return 'failed: the classifier answered something that is not an affect';
}
