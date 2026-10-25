// The ECMAScript of abilities/interpretation/interpretation.scx (SCXML 5.8):
// loaded by its root <script src="interpretation.js">, so every function here
// is a global of the document's data model, callable from its cond and expr
// attributes.

// Shared: the scored sound's fields, the stage timer and stage failures,
// the done.invoke shape checks, and the speech products.

// A field of a sound or a scored sound, null when it is absent.
function orNull(value) {
  return value === undefined ? null : value;
}

function hasSampleRate(sound) {
  return typeof sound.sample_rate_hz === 'number';
}

// A sound's channels, mono when it does not say.
function soundChannels(sound) {
  return typeof sound.channels === 'number' ? sound.channels : 1;
}

// Audible: no level, or at least product_threshold_db.
function isAudible(scored) {
  return scored.perceived_level_db == null || scored.perceived_level_db >= product_threshold_db;
}

function stageDelay() {
  return stage_timeout_ms + 'ms';
}

function stageTimeout() {
  return ({stage: stage, message: 'Listening ' + stage + ' timed out.'});
}

function stageFailure(message) {
  return ({stage: stage, message: message});
}

// A stage child's (or the speech service's) failure {message}.
function childFailure(_event) {
  return ({stage: stage, message: Object(_event.data).message});
}

// A done.invoke that is not a stage child's failure {message}.
function isNotChildFailure(_event) {
  return typeof Object(_event.data).message !== 'string';
}

function hasLabels(_event) {
  return Array.isArray(Object(_event.data).labels);
}

function hasSegments(_event) {
  return Array.isArray(Object(_event.data).segments);
}

function hasEmbeddings(_event) {
  return Array.isArray(Object(_event.data).embeddings);
}

function hasAudio(_event) {
  return typeof Object(_event.data).audio === 'string';
}

function hasText(_event) {
  return typeof Object(_event.data).text === 'string';
}

// A speech product of a scored sound: voice_embedding only when one is
// given.
function speechProduct(content, contentType, scored, voiceDetection, sourceIds, voiceEmbedding) {
  var product = ({content: content, content_type: contentType, voice_detection: voiceDetection});
  product.sample_rate_hz = scored.sound.sample_rate_hz;
  product.channels = soundChannels(scored.sound);
  product.media_type = 'audio/pcm';
  product.source_ids = sourceIds;
  if (arguments.length > 5) {
    product.voice_embedding = voiceEmbedding;
  }
  product.parent = orNull(scored.parent);
  return product;
}

// initializing

// The configuration initializing refuses: a negative threshold, a
// recognize_timeout_ms that is not positive.
function hasNegativeThreshold() {
  return product_threshold_db < 0;
}

function hasNoRecognizeTimeout() {
  return !(recognize_timeout_ms > 0);
}

// Working

// The current stage's failure.
function isCurrentStageFailure(_event) {
  return _event.data.stage === stage;
}

function hasStage() {
  return stage !== null;
}

function stageErrorFailure(_event) {
  return ({stage: stage, message: 'Listening ' + stage + ' could not run (' + _event.name + ')'});
}

function isStageChildFailure(_event) {
  return stage !== null && typeof Object(_event.data).message === 'string';
}

// Idle, HearingSpeech

// The bot's own voice arriving as predicted: heard while producing, below
// the product threshold. A sound with no level is never predicted.
function isPredictedOwnVoice(scored) {
  return scored.producing === true && scored.perceived_level_db != null && scored.perceived_level_db < product_threshold_db;
}

// DetectingVoice, FeedingSpeech (voice detection)

function nextSerial() {
  return serial + 1;
}

// The directory of the sound's audio reference.
function soundMediaDir() {
  return sensed.sound.audio.slice(0, sensed.sound.audio.lastIndexOf('/') + 1);
}

function soundOutput() {
  return media_dir + 'floppy-sound-' + serial + '.pcm';
}

// The voice detection child's ApplyData {sound, voice_detection}.
function isApplyData(_event) {
  return Array.isArray(Object(Object(_event.data).voice_detection).segments) && Object(_event.data).sound;
}

function detectionRecord() {
  return ({sensed: sensed, voice_detection: result.voice_detection});
}

// The child normalized the sound to other audio.
function isRenormalized() {
  return result.sound.audio !== sensed.sound.audio || result.sound.media_type !== (sensed.sound.media_type || null);
}

function renormalizedSensed() {
  return ({sound: Object.assign({}, sensed.sound, result.sound), parent: sensed.parent, perceived_level_db: sensed.perceived_level_db});
}

function isDetected() {
  return detection !== null;
}

// DetectingVoice

function isSilent() {
  return detection !== null && detection.voice_detection.segments.length === 0;
}

function dropsSilence() {
  return isSilent() && !sound_classifier && speech_decoder;
}

function handsOffSilence() {
  return isSilent() && !speech_decoder && !sound_classifier;
}

function classifiesSilence() {
  return isSilent() && sound_classifier;
}

function identifiesDirectly() {
  return detection !== null && voice_classifier && !diarizer;
}

// What HandingOff hands off for the sound in hand.
function soundCompletion(kind) {
  return ({kind: kind, sensed: detection.sensed, voice_detection: detection.voice_detection});
}

// The open speech, opened with the frame just detected.
function openingFrames() {
  return [detection.sensed.sound];
}

// HearingSpeech, SpeechEnded (the frame just heard)

// Streamed when Listening owns no decoder.
function streamsFrame() {
  return audible && !speech_decoder && !diarizer && hasSampleRate(detection.sensed.sound);
}

function streamsPcmFrame() {
  return audible && detection.sensed.sound.media_type === 'audio/pcm' && detection.sensed.sound.audio;
}

function frameProduct() {
  return speechProduct(detection.sensed.sound.audio, 'audio/pcm', detection.sensed, detection.voice_detection, active_source_ids, active_voice_embedding);
}

// FeedingSpeech

function openFramesWithFrame() {
  return open_frames.concat([detection.sensed.sound]);
}

function isStillVoiced() {
  return detection !== null && detection.voice_detection.segments.length > 0;
}

// The first frame, or the first with a level.
function isFirstLevelledFrame() {
  return open_sensed === null || (open_sensed.perceived_level_db == null && detection.sensed.perceived_level_db != null);
}

function isLouderFrame() {
  return open_sensed.perceived_level_db != null && detection.sensed.perceived_level_db > open_sensed.perceived_level_db;
}

// SpeechEnded

function openSpeechStage() {
  return diarizer ? 'voice_diarization' : 'speech_decoding';
}

function opensWav() {
  return hasSampleRate(open_sensed.sound);
}

function readsNoUtterance() {
  return !diarizer && !speech_decoder;
}

function utteranceOutput() {
  return media_dir + 'floppy-utterance-' + serial + (open_wav ? '.wav' : '.pcm');
}

function isUtteranceToDiarize(_event) {
  return diarizer && hasAudio(_event);
}

// ClassifyingSound

function hasAnyLabel(_event) {
  return hasLabels(_event) && _event.data.labels.length > 0;
}

function isUnlabelledForDecoder(_event) {
  return hasLabels(_event) && speech_decoder;
}

// DiarizingVoice

function diarizedOutput() {
  return media_dir + 'floppy-diarized-' + serial;
}

// Identified before anything else only when nothing decodes: decoded
// speech is identified after its words.
function isDiarizedToIdentify(_event) {
  return hasSegments(_event) && voice_classifier && !speech_decoder;
}

function isDiarizedToDecode(_event) {
  return hasSegments(_event) && speech_decoder;
}

function diarizationCompletion() {
  return ({kind: 'diarization', sensed: open_sensed || detection.sensed, voice_detection: detection.voice_detection});
}

// IdentifyingVoiceDirect

function cannotIdentifyOpening() {
  return detection.sensed.sound.media_type !== 'audio/pcm' || !hasSampleRate(detection.sensed.sound);
}

function openingConversionFailure() {
  return stageFailure('Voice identification opening segment conversion failed: ' + mismatch);
}

function openingStart() {
  return detection.voice_detection.segments[0].start_seconds;
}

function openingEnd() {
  return detection.voice_detection.segments[0].end_seconds;
}

function voiceOutput() {
  return media_dir + 'floppy-voice-' + serial + '.pcm';
}

function hasNoEmbedding(_event) {
  return hasEmbeddings(_event) && _event.data.embeddings.length === 0;
}

function firstEmbedding(_event) {
  return _event.data.embeddings[0];
}

function activeSourceIds() {
  return [active_voice_embedding.embedding];
}

// The identification deadline (decision D177): a stage, then the
// registry's answer.
function identifyDelay() {
  return (stage_timeout_ms + recognize_timeout_ms) + 'ms';
}

function isIdentifyTimer(_event) {
  return _event.sendid === 'direct-identification-timer' || _event.sendid === 'voice-identification-timer';
}

function identifyTimedOut(count) {
  return count + ' segment(s) unidentified after ' + (stage_timeout_ms + recognize_timeout_ms) + ' ms';
}

// IdentifyingVoice

// Decoded speech stays decoded speech once its voices are identified.
function identifiedKind() {
  return completion.kind === 'decoding' ? 'decoding' : 'identification';
}

// No voice for any segment.
function unidentifiedEmbeddings() {
  return completion.segments.map(function () { return null; });
}

function isEmbeddingCountMismatch(_event) {
  return hasEmbeddings(_event) && _event.data.embeddings.length !== completion.segments.length;
}

function embeddingCountMismatch(_event) {
  return 'received ' + _event.data.embeddings.length + ' for ' + completion.segments.length + ' segments.';
}

function embeddingCountFailure() {
  return stageFailure('Voice identification must return exactly one embedding per voice segment; ' + mismatch);
}

// DecodingSpeech

function decodingCompletion(_event) {
  return ({kind: 'decoding', sensed: open_sensed || detection.sensed, voice_detection: detection.voice_detection, text: _event.data.text,
    segments: isDiarization() ? completion.segments : []});
}

// Decoded words after diarization (In() no longer holds once the
// transition's content runs: its source state has been exited).
function isDiarization() {
  return completion !== null && completion.kind === 'diarization';
}

// Decoded words whose diarized voices are identified next.
function identifiesDecoded(_event) {
  return hasText(_event) && voice_classifier && In('Diarized') && completion.segments.length > 0;
}

// The diarized segments embedded for decoded speech: those of at least
// min_voice_seconds, else the longest (a print from less is too little to
// tell a voice by).
function voicedSegments() {
  var long = completion.segments.filter(function (segment) { return segment.end_seconds - segment.start_seconds >= min_voice_seconds; });
  if (long.length > 0) {
    return long;
  }
  return [completion.segments.reduce(function (longest, segment) {
    return segment.end_seconds - segment.start_seconds > longest.end_seconds - longest.start_seconds ? segment : longest;
  })];
}

// HandingOff

function isInaudible() {
  return !audible;
}

function isDecodingWithoutRate() {
  return completion.kind === 'decoding' && !hasSampleRate(completion.sensed.sound);
}

function decodingRateFailure() {
  return ({stage: 'speech_decoding', message: 'Speech decoding completed on sound with no sample rate to place it on a timeline.'});
}

function isDecoding() {
  return completion.kind === 'decoding';
}

// Decoding rewrites the observation: it becomes words, with the voices
// heard that are not the bot's own as its source_ids.
function decodedProduct() {
  return speechProduct(completion.text, 'text/plain', completion.sensed, completion.voice_detection, otherVoices());
}

// The identified voices of decoded speech.
function heardVoices() {
  return Array.isArray(completion.embeddings) ? completion.embeddings.filter(function (voice) { return voice !== null && Array.isArray(voice.embedding); }) : [];
}

function cosine(a, b) {
  if (a.length !== b.length) {
    return null;
  }
  var dot = 0, aa = 0, bb = 0;
  for (var i = 0; i < a.length; i++) {
    dot += a[i] * b[i];
    aa += a[i] * a[i];
    bb += b[i] * b[i];
  }
  return aa === 0 || bb === 0 ? null : dot / Math.sqrt(aa * bb);
}

// A voice at least voice_similarity_threshold alike to the bot's own.
function isOwnVoice(voice) {
  if (own_voice === null) {
    return false;
  }
  var score = cosine(voice.embedding, own_voice.embedding);
  return score !== null && score >= voice_similarity_threshold;
}

// The embeddings of the voices that are not the bot's own, the first of
// alike ones (at the threshold) once, at most 4.
function otherVoices() {
  var others = [];
  heardVoices().forEach(function (voice) {
    if (isOwnVoice(voice) || others.length >= 4) {
      return;
    }
    var known = others.some(function (other) {
      var score = cosine(other, voice.embedding);
      return score !== null && score >= voice_similarity_threshold;
    });
    if (!known) {
      others.push(voice.embedding);
    }
  });
  return others;
}

// DetectingOwnVoice, IdentifyingOwnVoice

// Speaking's copy of a completed utterance, with the PCM it commanded,
// while the bot's own voice is unknown and voices are identified.
function learnsOwnVoice(_event) {
  return voice_classifier && own_voice === null && typeof _event.data.audio === 'string'
    && _event.data.media_type === 'audio/pcm' && typeof _event.data.sample_rate_hz === 'number';
}

// The copy's directory, where the clip of its voice is written.
function ownMediaDir() {
  return utterance.audio.slice(0, utterance.audio.lastIndexOf('/') + 1);
}

function ownVoiceUnidentified(_event) {
  var message = Object(_event.data).message;
  return 'no embedding for the bot\'s own voice' + (typeof message === 'string' ? ': ' + message : '');
}

function ownUtterance(_event) {
  return ({audio: _event.data.audio, media_type: 'audio/pcm', sample_rate_hz: _event.data.sample_rate_hz,
    channels: typeof _event.data.channels === 'number' ? _event.data.channels : 1});
}

// At least min_voice_seconds from the copy's first voice to its last.
function isOwnVoiceLongEnough(_event) {
  return isApplyData(_event) && _event.data.voice_detection.segments.length > 0
    && spanEnd(_event.data.voice_detection.segments) - _event.data.voice_detection.segments[0].start_seconds >= min_voice_seconds;
}

function spanEnd(segments) {
  return segments.reduce(function (end, segment) { return Math.max(end, segment.end_seconds); }, 0);
}

function ownVoiceStart() {
  return utterance.voice_detection.segments[0].start_seconds;
}

function ownVoiceEnd() {
  return spanEnd(utterance.voice_detection.segments);
}

function hasOneEmbedding(_event) {
  return hasEmbeddings(_event) && _event.data.embeddings.length === 1 && _event.data.embeddings[0] !== null
    && Array.isArray(_event.data.embeddings[0].embedding);
}

function isSegmented() {
  return completion.kind === 'diarization' || completion.kind === 'identification';
}

function isSegmentSourceMismatch() {
  return completion.sensed.sound.media_type !== 'audio/pcm' || !hasSampleRate(completion.sensed.sound);
}

function segmentSourceMismatch() {
  return mismatch ? 'voice segment speech requires source audio/pcm with a sample rate.' : null;
}

// The segment in product (the <foreach> item) against its source.
function isSegmentRateMismatch() {
  return mismatch === null && product.sample_rate_hz !== completion.sensed.sound.sample_rate_hz;
}

function isSegmentFormatMismatch() {
  return mismatch === null && (product.channels !== channels || product.media_type !== 'audio/pcm');
}

function hasMismatch() {
  return mismatch !== null;
}

function segmentConversionFailure() {
  return ({stage: 'voice_' + completion.kind, message: 'Voice ' + completion.kind + ' product conversion failed: ' + mismatch});
}

// The segment's embedding, after identification.
function segmentEmbedding() {
  return completion.kind === 'identification' ? completion.embeddings[index] : null;
}

// One speech product per clipped segment.
function segmentProduct() {
  var product = ({content: segment.audio, content_type: 'audio/pcm', media_type: 'audio/pcm'});
  product.sample_rate_hz = segment.sample_rate_hz;
  product.channels = segment.channels;
  product.start_seconds = segment.start_seconds;
  product.end_seconds = segment.end_seconds;
  product.confidence = orNull(segment.confidence);
  product.voice_detection = ({segments: [{start_seconds: 0.0, end_seconds: segment.end_seconds - segment.start_seconds}]});
  product.voice_detection.segments[0].confidence = product.confidence;
  product.source_ids = result === null ? [] : [result.embedding];
  product.voice_embedding = result;
  product.parent = orNull(completion.sensed.parent);
  return product;
}

function isPcmSilence() {
  return completion.kind === 'detection' && completion.sensed.sound.media_type === 'audio/pcm' && completion.sensed.sound.audio;
}

// A silence observation: the sound's PCM with its spans.
function silenceProduct() {
  return speechProduct(completion.sensed.sound.audio, 'audio/pcm', completion.sensed, completion.voice_detection, []);
}

// Participants (producers stamp, transformers copy)

// The participants stamped on a scored sound.
function soundParticipants(scored) {
  return Object(scored.sound).participants || [];
}

// Not yet in the ordered union, which holds at most 32.
function isNewParticipant(p) {
  return active_participants.length < 32 && !active_participants.some(function (q) { return q.ref === p.ref && q.role === p.role; });
}
