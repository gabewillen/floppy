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

// The sound fields of vocal's chunk or output: a rate, channel count or
// media type it leaves null is speaking's own.
function soundFormat(data) {
  return ({media_type: data.media_type == null ? media_type : data.media_type,
    sample_rate_hz: data.sample_rate_hz == null ? sample_rate_hz : data.sample_rate_hz,
    channels: data.channels == null ? channels : data.channels});
}

// The encoded speech's format, from its first chunk.
function chunkFormat(_event) {
  return soundFormat(_event.data);
}

// The encoded speech's format, from its output.
function encodedFormat(_event) {
  return soundFormat(_event.data);
}

// A chunk's measured length in ms, 0 when it has none (decision D203).
function chunkMs(_event) {
  var duration = _event.data.duration;
  return typeof duration === 'number' && duration > 0 ? Math.round(duration * 1000) : 0;
}

// The chunks waiting to sound, with this one behind them.
function queued(_event) {
  var format = soundFormat(_event.data);
  return chunks.concat([{audio: _event.data.audio, media_type: format.media_type,
    sample_rate_hz: format.sample_rate_hz, channels: format.channels, duration_ms: chunkMs(_event)}]);
}

function encodingTimedOutLog() {
  return 'encoding ' + operation + ' after ' + encoding_timeout_ms + ' ms';
}

function encodingRunFailure(_event) {
  return 'Speech encoding could not run (' + _event.name + ').';
}

// playing

// Some measured audio was received: without any no efference copy is
// sent and no pieces are commanded.
function hasDuration() {
  return received_ms > 0;
}

// Measured audio was received and has not played yet.
function hasRemaining() {
  return received_ms > played_ms;
}

// Chunks wait to sound.
function hasQueued() {
  return chunks.length > 0;
}

// The chunk in hand is the utterance's first sound.
function isFirstSound() {
  return sounds === 0;
}

// The chunk in hand has a measured duration: it is played piece by piece.
function chunkHasDuration() {
  return chunk.duration_ms > 0;
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

// The seconds of measured audio received and not yet played.
function remainingSeconds() {
  return (received_ms - played_ms) / 1000;
}

// The next piece commanded to the mouth: at most half the lookahead, the
// rest of the chunk in hand at its end (decision D179).
function nextPieceMs() {
  return Math.min(lookahead_ms / 2, chunk.duration_ms - chunk_played_ms);
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

// The report of the chunk's last piece.
function isChunkEnd(_event) {
  return isMark(_event) && chunk_played_ms + nextPieceMs() >= chunk.duration_ms;
}

// The chunk's last piece played, and another chunk waits.
function endsChunkWithMore(_event) {
  return isChunkEnd(_event) && hasQueued();
}

// The chunk's last piece played, and it was the utterance's last.
function endsUtterance(_event) {
  return isChunkEnd(_event) && encoded_all;
}

// The chunk's last piece played, and the next has not arrived.
function endsChunk(_event) {
  return isChunkEnd(_event);
}

// The words the played share of the audio covers, rounded down to whole
// words (decision D179): all of them once all of it played. While the
// encoding still streams, the audio is what has been received (decision
// D203); nothing is spoken when none of it was measured.
function spokenWords() {
  if (encoded_all && played_ms >= received_ms) {
    return text;
  }
  if (received_ms <= 0) {
    return '';
  }
  var ends = [];
  text.replace(/\S+/g, function (word, at) { ends.push(at + word.length); return word; });
  var count = Math.floor(ends.length * played_ms / received_ms);
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
// Stamped with the bot's own voice once hearing has learned it.
function outboundMessage() {
  return ({source_ids: [], target_ids: [], content: spoken, content_type: 'text/plain', participants: ownVoice()});
}

function ownVoice() {
  return voice_id === null ? [] : [{ref: voice_id, role: 'source', own: true}];
}

// A copy of the bot's own voice is handed to hearing until it has learned
// it: an utterance played whole, its encoded audio PCM of at least 1.5 s
// (mosfet.py's shortest enrollment).
function enrolls() {
  return voice_id === null && outcome === 'completed' && said_audio !== null && said_audio.media_type === 'audio/pcm'
    && typeof said_audio.duration === 'number' && said_audio.duration >= 1.5;
}

// bot.ability.listening.enroll: the commanded PCM of the utterance.
function enrollCopy() {
  var format = soundFormat(said_audio);
  return ({voice_id: voice_id, segment: {audio: said_audio.audio, media_type: 'audio/pcm',
    sample_rate_hz: format.sample_rate_hz, channels: format.channels, start_seconds: 0, end_seconds: said_audio.duration}});
}

// The answer to the parent: bot.ability.speaking.output (decision D179;
// voice_id null until hearing has learned the bot's own voice).
function speechProduct() {
  var format = audioFormat();
  return ({text: text, spoken: spoken, unspoken: unspoken, outcome: outcome, voice_id: voice_id,
    media_type: format.media_type, sample_rate_hz: format.sample_rate_hz, channels: format.channels});
}

function recordingTimedOutLog() {
  return 'recording ' + operation + ' after ' + recording_timeout_ms + ' ms';
}

function recordingFailure(_event) {
  return 'Conversation history recording failed (' + _event.name + ').';
}
