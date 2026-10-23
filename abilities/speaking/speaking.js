// The ECMAScript of abilities/speaking/speaking.scx (SCXML 5.8): loaded by its root
// <script src="speaking.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared: the timer in hand and send delays.

// The timer in hand (a timer is a bot.ability.speaking.apply.failed send).
function isTimer(_event) {
  return _event.sendid === timer;
}

function delay(ms) {
  return ms + 'ms';
}

// idle

// The text of record of a reply: its parts' words joined verbatim
// (decision D178).
function said(parts) {
  return parts.map(function (part) { return typeof part.words === 'string' ? part.words : ''; }).join('');
}

// An input for an utterance already said (decision D175): only one whose
// sender named its id can be one.
function isFinishedUtterance(_event) {
  return !!_event.sendid && finished_utterances.indexOf(_event.sendid) >= 0;
}

// The utterance's id: its input's sendid, or null when none was named.
function utteranceId(_event) {
  return _event.sendid ? _event.sendid : null;
}

// speaking

// An input for a finished utterance, or another copy of the one in hand
// or of one held.
function isLateUtterance(_event) {
  return isFinishedUtterance(_event) || isCurrentUtterance(_event) || isHeldUtterance(_event);
}

// Another copy of the utterance in hand.
function isCurrentUtterance(_event) {
  return !!_event.sendid && _event.sendid === utterance_id;
}

// Another copy of a held utterance: the first copy is kept, as
// <event:defer/> cannot replace an event it holds (decision D175).
function isHeldUtterance(_event) {
  return !!_event.sendid && held_ids.indexOf(_event.sendid) >= 0;
}

// The held ids with this input's, when its sender named one.
function heldIds(_event) {
  return _event.sendid ? held_ids.concat([_event.sendid]) : held_ids;
}

function canHold() {
  return held < held_utterances_bound;
}

function heldOverMessage() {
  return 'holding ' + held + ' utterances, the most it holds';
}

// The finished ids with the one in hand, the oldest dropped past the
// bound (decision D175).
function finishedUtterances() {
  var ids = utterance_id === null ? finished_utterances : finished_utterances.concat([utterance_id]);
  return ids.slice(Math.max(0, ids.length - finished_utterances_bound));
}

// A reply that says something (decision D178): every part has words (not blank), a sound or a pause, every
// expression directs a cue (sound_at only with a sound), and the words,
// joined exactly as written, are not blank.
function isUnspokenInput(_event) {
  return !isSpokenReply(_event.data.parts);
}

function isSpokenReply(parts) {
  return parts.every(isSpokenPart) && /\S/.test(said(parts));
}

function isSpokenPart(part) {
  var expression = isSet(part.expression) ? part.expression : null;
  if (isSet(part.words) && !/\S/.test(part.words)) {
    return false;
  }
  if (expression === null) {
    return typeof part.words === 'string';
  }
  var cues = ['instructions', 'emotion', 'style', 'intensity', 'pace', 'pitch', 'volume', 'pause', 'sound'];
  return (cues.some(function (cue) { return isSet(expression[cue]); }) || expression.emphasis === true)
    && (expression.sound_at === undefined || isSet(expression.sound))
    && (typeof part.words === 'string' || isSet(expression.pause) || isSet(expression.sound));
}

function isSet(value) {
  return value !== undefined && value !== null;
}

// encoding

function nextOperation() {
  return operation + 1;
}

// Where the speech service writes the audio: output_prefix + operation +
// "." + the media subtype.
function outputPath() {
  return output_prefix + operation + '.' + media_type.split('/')[1];
}

// vocal's encoded speech, copied.
function encodedSpeech(_event) {
  return Object.assign({}, _event.data);
}

// The encoded speech's sample rate, else speaking's own.
function encodedSampleRate() {
  return encoded.sample_rate_hz == null ? sample_rate_hz : encoded.sample_rate_hz;
}

// The encoded speech's channels, else speaking's own.
function encodedChannels() {
  return encoded.channels == null ? channels : encoded.channels;
}

// The encoded speech's media type, else speaking's own.
function encodedMediaType() {
  return encoded.media_type == null ? media_type : encoded.media_type;
}

function encodingTimedOutLog() {
  return 'encoding ' + operation + ' after ' + encoding_timeout_ms + ' ms';
}

function encodingRunFailure(_event) {
  return 'Speech encoding could not run (' + _event.name + ').';
}

// playing

// The playout's length in ms: the encoded speech's measured duration, 0
// when it has none.
function durationMs() {
  return typeof encoded.duration === 'number' && encoded.duration > 0 ? Math.round(encoded.duration * 1000) : 0;
}

// The encoded speech has a measured duration: without one no efference
// copy is sent and no pieces are commanded.
function hasDuration() {
  return duration_ms > 0;
}

// The encoded speech's format, speaking's own before anything was encoded.
function audioFormat() {
  return encoded === null ? {media_type: media_type, sample_rate_hz: sample_rate_hz, channels: channels} : encoded;
}

// The efference copy of `seconds` of playout still to come.
function efferenceCopy(seconds) {
  var format = audioFormat();
  return ({mouth: mouth, duration: seconds, media_type: format.media_type,
    sample_rate_hz: format.sample_rate_hz, channels: format.channels});
}

function remainingSeconds() {
  return (duration_ms - played_ms) / 1000;
}

// The next piece commanded to the mouth: at most half the lookahead, the
// rest of the playout at the end (decision D179).
function nextPieceMs() {
  return Math.min(lookahead_ms / 2, duration_ms - played_ms);
}

// The mark of a commanded piece: u<utterance>.<piece> (decision D179).
function markName() {
  return 'u' + operation + '.' + commands;
}

// A piece is pending (none when the audio had no duration).
function hasMark() {
  return mark !== null;
}

// The mouth's report of the pending piece.
function isMark(_event) {
  return _event.sendid === mark;
}

// The report of the last piece.
function isLastMark(_event) {
  return isMark(_event) && played_ms + nextPieceMs() >= duration_ms;
}

// The words the played share of the audio covers, rounded down to whole
// words (decision D179): all of them once all of it played.
function spokenWords() {
  if (played_ms >= duration_ms) {
    return text;
  }
  var ends = [];
  text.replace(/\S+/g, function (word, at) { ends.push(at + word.length); return word; });
  var count = Math.floor(ends.length * played_ms / duration_ms);
  return count > 0 ? text.slice(0, ends[count - 1]) : '';
}

function playoutFailure(_event) {
  return 'Speech playout failed (' + _event.name + ').';
}

// recording

// Only an utterance that spoke something is recorded (decision D179).
function hasSpoken() {
  return spoken.length > 0;
}

// What was spoken, as the outbound message the conversation commits: no
// participants until speaking has a voice of its own (decision D176).
function outboundMessage() {
  return ({source_ids: [], target_ids: [], content: spoken, content_type: 'text/plain', participants: []});
}

// The answer to the parent: bot.ability.speaking.output (decision D179;
// voice_id null until speaking is voiced).
function speechProduct() {
  var format = audioFormat();
  return ({text: text, spoken: spoken, unspoken: unspoken, outcome: outcome, voice_id: null,
    media_type: format.media_type, sample_rate_hz: format.sample_rate_hz, channels: format.channels});
}

function recordingTimedOutLog() {
  return 'recording ' + operation + ' after ' + recording_timeout_ms + ' ms';
}

function recordingFailure(_event) {
  return 'Conversation history recording failed (' + _event.name + ').';
}
