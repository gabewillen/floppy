// The ECMAScript of abilities/turn_detector/turn_detector.scx (SCXML 5.8): loaded by its
// root <script src="turn_detector.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared: identities, the original's field defaults, and the timer in hand.

// Two identities (a string, or an array of numbers) are the same identity.
function sameIdentity(left, right) {
  return JSON.stringify(left) === JSON.stringify(right);
}

// A turn event's conversation_ref (the original's default "conversation").
function conversationRef(_event) {
  return _event.data.conversation_ref === undefined ? 'conversation' : _event.data.conversation_ref;
}

// A turn event's source_participant_ref (the original's default "caller").
function sourceRef(_event) {
  return _event.data.source_participant_ref === undefined ? 'caller' : _event.data.source_participant_ref;
}

// A turn start's self_participant_ref (the original's default "bot").
function selfRef(_event) {
  return _event.data.self_participant_ref === undefined ? 'bot' : _event.data.self_participant_ref;
}

// The original's hsm.after delay: the end-of-turn silence, at least 10 ms.
function silenceDelay() {
  return Math.max(Math.round(end_of_turn_silence_seconds * 1000), 10) + 'ms';
}

function isTimer(_event) {
  return _event.sendid === timer;
}

// Guards (the original _has_start, _matches_active_turn, _has_voice).

// The event is for the open turn: its turn, conversation and source.
function isActiveTurn(_event) {
  return turn_ref !== null && _event.data.turn_ref === turn_ref && conversationRef(_event) === active_conversation_ref && sameIdentity(sourceRef(_event), source_participant_ref);
}

// A start addressed to this detector that opens a turn, or is for the open one.
function hasStart(_event) {
  return sameIdentity(selfRef(_event), participant_ref) && (turn_ref === null || isActiveTurn(_event));
}

// Heard speech with voice spans.
function hasVoice(_event) {
  return (_event.data.voice_detection.segments || []).length > 0;
}

// The turn a voiced observation opens (the original: a fresh uuid).
function voiceTurnRef() {
  return 'turn-' + voice_turns;
}

// Content: <conversation:ingest_content> takes each stimulus in (the original
// _ingest_content); a stimulus is the event's optional content, {kind,
// source_participant_ref, content, ...}, or heard speech as one.

// Heard speech as an audio stimulus: its audio while acoustic (decoded
// words carry no frames), with its rate and channels.
function voiceStimulus(_event) {
  var acoustic = _event.data.content_type === undefined || _event.data.content_type === 'audio/pcm';
  return {kind: 'audio', content: acoustic ? _event.data.content : '', sample_rate_hz: _event.data.sample_rate_hz, channels: _event.data.channels === undefined ? 1 : _event.data.channels};
}

// closing

// The original's _TurnSilenceExpiredData: the provenance of the expiry.
function silenceExpired() {
  return {participant_ref: participant_ref, conversation_ref: active_conversation_ref, turn_ref: turn_ref, source_participant_ref: source_participant_ref};
}

function isTurnSilenceExpired(_event) {
  return sameIdentity(_event.data.participant_ref, participant_ref) && _event.data.turn_ref === turn_ref && _event.data.conversation_ref === active_conversation_ref && sameIdentity(_event.data.source_participant_ref, source_participant_ref);
}

function isFailedTurn() {
  return closed.failure !== undefined;
}
