// The ECMAScript of abilities/speaking/speaking.scx (SCXML 5.8): loaded by its root
// <script src="speaking.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared: the sender of an event, the timer in hand, and send delays.

function isFromVocal(_event) {
  return _event.invokeid === 'vocal';
}

// The timer in hand (a timer is a bot.ability.speaking.apply.failed send).
function isTimer(_event) {
  return _event.sendid === timer;
}

function delay(ms) {
  return ms + 'ms';
}

// idle

function hasQueued() {
  return queue.length > 0;
}

// The text that waited longest.
function queuedText() {
  return queue[0];
}

function remainingQueue() {
  return queue.slice(1);
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

// vocal's error or end, or a vocal that could not start (no invokeid,
// SCXML 6.4.2).
function isFromVocalOrStart(_event) {
  return _event.invokeid === 'vocal' || !_event.invokeid;
}

function encodingRunFailure(_event) {
  return 'Speech encoding could not run (' + _event.name + ').';
}

// The queue with the input's text appended.
function deferredQueue(_event) {
  return queue.concat([_event.data.text]);
}

// playing

// mosfet sends no efference copy without a duration.
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

function recordingTimedOutLog() {
  return 'recording ' + operation + ' after ' + recording_timeout_ms + ' ms';
}

function recordingFailure(_event) {
  return 'Conversation history recording failed (' + _event.name + ').';
}
