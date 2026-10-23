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

// No efference copy is sent without a duration.
function hasDuration() {
  return typeof encoded.duration === 'number' && encoded.duration > 0;
}

// The answer to the parent: bot.ability.speaking.output.
function speechProduct() {
  return ({text: text, media_type: encoded.media_type, sample_rate_hz: encoded.sample_rate_hz, channels: encoded.channels});
}

function playoutFailure(_event) {
  return 'Speech playout failed (' + _event.name + ').';
}

// recording

// What was said, as the outbound message the conversation commits: no
// participants until speaking has a voice of its own, when it stamps its
// voice's ref, own (decision D176).
function outboundMessage() {
  return ({source_ids: [], target_ids: [], content: text, content_type: 'text/plain', participants: []});
}

function recordingTimedOutLog() {
  return 'recording ' + operation + ' after ' + recording_timeout_ms + ' ms';
}

function recordingFailure(_event) {
  return 'Conversation history recording failed (' + _event.name + ').';
}
