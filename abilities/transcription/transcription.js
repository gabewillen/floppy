// The ECMAScript of abilities/transcription/transcription.scx (SCXML 5.8): loaded by its
// root <script src="transcription.js">, so every function here is a global of
// the document's data model, callable from its cond and expr attributes.

// The most transcripts and voice spans a turn keeps (mosfet.py's), and
// the most participants (the interpretation's).
var MAX_TRANSCRIPTS = 64;
var MAX_SPANS = 64;
var MAX_PARTICIPANTS = 32;

function delay(ms) {
  return ms + 'ms';
}

// Streaming

function unavailableLog(_event) {
  return 'no streaming session (' + lossOf(_event) + '): the listening hears through the interpretation';
}

function lostLog(_event) {
  return 'the streaming session was lost (' + lossOf(_event) + '); reconnecting in ' + backoff_ms + ' ms';
}

// The session's failure kind, else the event that ended it.
function lossOf(_event) {
  var data = Object(_event.data);
  if (typeof data.kind === 'string') {
    return data.kind;
  }
  if (typeof data.message === 'string') {
    return data.message;
  }
  return _event.name;
}

// Reconnecting: the backoff doubles up to its last step.
function nextBackoff() {
  return Math.min(backoff_ms * 2, last_backoff_ms);
}

// Hearing

// The bot's own voice arriving as predicted: heard while producing, below
// the product threshold. A sound with no level is never predicted.
function isPredictedOwnVoice(scored) {
  return scored.producing === true && scored.perceived_level_db != null && scored.perceived_level_db < product_threshold_db;
}

function isProducingChange(scored) {
  return producing_told !== (scored.producing === true);
}

// The sound as the session reads it: by reference, with its format, and
// where the session writes the 16 kHz PCM it sends (heardFrame()).
function streamedAudio(sound) {
  var audio = ({audio: sound.audio, output: heardFrame(sound).audio});
  ['media_type', 'sample_rate_hz', 'channels'].forEach(function (field) {
    if (sound[field] != null) {
      audio[field] = sound[field];
    }
  });
  return audio;
}

// The PCM the session wrote for the sound heard as number heard_count,
// beside the sound: a ring of twice max_turn_frames files, so a turn's
// sounds are never overwritten while it is handed on.
function heardFrame(sound) {
  var dir = sound.audio.slice(0, sound.audio.lastIndexOf('/') + 1);
  return ({audio: dir + 'floppy-heard-' + (heard_count % (2 * max_turn_frames)) + '.pcm', media_type: 'audio/pcm',
    sample_rate_hz: 16000, channels: 1});
}

// The turn's sounds (the PCM the session wrote), the oldest let go past
// max_turn_frames.
function framesWith(sound) {
  var kept = frames.concat([heardFrame(sound)]);
  return kept.length > max_turn_frames ? kept.slice(kept.length - max_turn_frames) : kept;
}

// The first scored sound, or the first with a level, or a louder one.
function isLouder(scored) {
  if (loudest === null) {
    return true;
  }
  if (loudest.perceived_level_db == null) {
    return scored.perceived_level_db != null;
  }
  return scored.perceived_level_db != null && scored.perceived_level_db > loudest.perceived_level_db;
}

function soundParticipants(scored) {
  return Array.isArray(scored.sound.participants) ? scored.sound.participants : [];
}

function isNewParticipant(participant) {
  return participants.length < MAX_PARTICIPANTS && !participants.some(function (known) {
    return known.ref === participant.ref && known.role === participant.role;
  });
}

function isTranscript(_event) {
  return typeof Object(_event.data).text === 'string' && transcripts.length < MAX_TRANSCRIPTS;
}

function heardTranscript(_event) {
  var confidence = _event.data.confidence;
  return ({text: _event.data.text, confidence: typeof confidence === 'number' ? confidence : null});
}

function isAt(_event) {
  return typeof Object(_event.data).at_seconds === 'number';
}

function spansStarted(at) {
  return spans.length < MAX_SPANS ? spans.concat([{start: at, end: null}]) : spans;
}

function spansEnded(at) {
  var last = spans[spans.length - 1];
  if (last === undefined || last.end !== null) {
    return spans;
  }
  return spans.slice(0, -1).concat([{start: last.start, end: at}]);
}

// A voice still heard is the next turn's too.
function openSpans() {
  var last = spans[spans.length - 1];
  return last !== undefined && last.end === null ? [last] : [];
}

// The turn the session said has ended: its words (turn_ended's text, else
// its transcripts joined), their lowest confidence, its sounds, its
// loudest scored sound, its voice spans in seconds from its start, and
// its participants.
function endedTurn(_event) {
  var ended = _event.data.at_seconds;
  var said = typeof _event.data.text === 'string' ? _event.data.text.trim() : '';
  var text = said !== '' ? said : transcripts.map(function (heard) { return heard.text.trim(); })
    .filter(function (words) { return words !== ''; }).join(' ');
  var confidences = transcripts.map(function (heard) { return heard.confidence; })
    .filter(function (confidence) { return confidence !== null; });
  var segments = [];
  spans.forEach(function (span) {
    var start = Math.max(0, span.start - turn_started_at);
    var end = Math.max(0, (span.end === null ? ended : span.end) - turn_started_at);
    if (end > start) {
      segments.push({start_seconds: start, end_seconds: end});
    }
  });
  return ({
    text: text,
    confidence: confidences.length > 0 ? Math.min.apply(null, confidences) : null,
    frames: frames,
    sensed: loudest,
    voice_detection: {segments: segments},
    participants: participants
  });
}

function hasWords(heard) {
  return heard.text !== '' && heard.frames.length > 0 && heard.sensed !== null;
}
