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

// Content (the original _ingest_content). A stimulus is the event's optional
// content: {kind, source_participant_ref, content, ...}.

function isKind(stimulus, kind) {
  return stimulus !== undefined && stimulus !== null && stimulus.kind === kind;
}

// The kept texts with a text stimulus's stripped text, when not empty.
function textPartsWith(stimulus) {
  return isKind(stimulus, 'text') && stimulus.content.trim() !== '' ? text_parts.concat([stimulus.content.trim()]) : text_parts;
}

// An audio stimulus that carries audio.
function isAudio(stimulus) {
  return isKind(stimulus, 'audio') && Boolean(stimulus.content);
}

// The audio chunks with an audio stimulus's reference.
function audioWith(stimulus) {
  return isAudio(stimulus) ? audio.concat([stimulus.content]) : audio;
}

// The first audio chunk's sample rate wins.
function sampleRateWith(stimulus) {
  return audio_sample_rate_hz === null && isAudio(stimulus) && isPresent(stimulus.sample_rate_hz) ? stimulus.sample_rate_hz : audio_sample_rate_hz;
}

function channelsWith(stimulus) {
  return audio_channels === null && isAudio(stimulus) && isPresent(stimulus.channels) ? stimulus.channels : audio_channels;
}

function isPresent(value) {
  return value !== undefined && value !== null;
}

// The modality-neutral content a content, image or event stimulus replaces.
function contentWith(stimulus) {
  if (isKind(stimulus, 'content') || isKind(stimulus, 'image')) {
    return stimulus.content;
  }
  if (isKind(stimulus, 'event')) {
    return {event: stimulus.event, payload: stimulus.payload === undefined ? {} : stimulus.payload};
  }
  return content;
}

function contentTypeWith(stimulus) {
  if (isKind(stimulus, 'content')) {
    return stimulus.content_type;
  }
  if (isKind(stimulus, 'image')) {
    return 'image/*';
  }
  return isKind(stimulus, 'event') ? 'application/event' : content_type;
}

// Heard speech as an audio stimulus: its audio while acoustic (decoded
// words carry no frames), with its rate and channels.
function voiceStimulus(_event) {
  var acoustic = _event.data.content_type === undefined || _event.data.content_type === 'audio/pcm';
  return {kind: 'audio', content: acoustic ? _event.data.content : '', sample_rate_hz: _event.data.sample_rate_hz, channels: _event.data.channels === undefined ? 1 : _event.data.channels};
}

function audioWithVoice(_event) {
  return audioWith(voiceStimulus(_event));
}

function voiceSampleRate(_event) {
  return sampleRateWith(voiceStimulus(_event));
}

function voiceChannels(_event) {
  return channelsWith(voiceStimulus(_event));
}

// closing

// The original's _TurnSilenceExpiredData: the provenance of the expiry.
function silenceExpired() {
  return {participant_ref: participant_ref, conversation_ref: active_conversation_ref, turn_ref: turn_ref, source_participant_ref: source_participant_ref};
}

function isTurnSilenceExpired(_event) {
  return sameIdentity(_event.data.participant_ref, participant_ref) && _event.data.turn_ref === turn_ref && _event.data.conversation_ref === active_conversation_ref && sameIdentity(_event.data.source_participant_ref, source_participant_ref);
}

// normalizing (the original _on_end): the closed turn as {turn} (TurnCompleteData)
// or {failure} (FailedEventData) when it has nothing to show.
function closedTurn() {
  var text = text_parts.join(' ').trim();
  if (text === '' && audio.length === 0 && content === null) {
    return {failure: turnFailure('Turn closed with neither text, audio, nor modality-neutral content.')};
  }
  var hasAudio = audio.length > 0;
  if (text !== '' && hasAudio) {
    return {failure: turnFailure('Turn product cannot contain both text and audio.')};
  }
  return {turn: {
    conversation_ref: active_conversation_ref,
    turn_ref: turn_ref,
    participant_ref: participant_ref,
    self_participant_ref: participant_ref,
    source_participant_ref: source_participant_ref,
    text: text,
    audio: audio,
    content: text !== '' ? text : hasAudio ? audio : content,
    content_type: text !== '' ? 'text/plain' : hasAudio ? 'audio/pcm' : content_type,
    sample_rate_hz: hasAudio ? audio_sample_rate_hz : null,
    channels: hasAudio ? audio_channels : null
  }};
}

// The original's FailedEventData for the open turn.
function turnFailure(message) {
  return {stage: 'turn', message: message, participant_ref: participant_ref, conversation_ref: active_conversation_ref, turn_ref: turn_ref, source_participant_ref: source_participant_ref};
}

function isFailedTurn() {
  return closed.failure !== undefined;
}
