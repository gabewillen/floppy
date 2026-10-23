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

// The original _is_audible_product: no level, or at least product_threshold_db.
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

// A speech product of a scored sound (the original _dispatch_product_cognition_input):
// voice_embedding only when one is given.
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

// The original's constructor refuses identification with decoding.
function hasNegativeThreshold() {
  return product_threshold_db < 0;
}

function hasNoRecognizeTimeout() {
  return !(recognize_timeout_ms > 0);
}

function refusesIdentificationWithDecoding() {
  return voice_classifier && speech_decoder;
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

// Streamed when Listening owns no decoder (the original _emit_speech_product).
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

function isDiarizedToIdentify(_event) {
  return hasSegments(_event) && voice_classifier;
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
  return ({kind: 'decoding', sensed: open_sensed || detection.sensed, voice_detection: detection.voice_detection, text: _event.data.text});
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

// Decoding rewrites the observation: it becomes words.
function decodedProduct() {
  return speechProduct(completion.text, 'text/plain', completion.sensed, completion.voice_detection, []);
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

// Participants (the original's f6c4e08: producers stamp, transformers copy)

// The participants stamped on a scored sound.
function soundParticipants(scored) {
  return Object(scored.sound).participants || [];
}

// Not yet in the ordered union, which holds at most 32.
function isNewParticipant(p) {
  return active_participants.length < 32 && !active_participants.some(function (q) { return q.ref === p.ref && q.role === p.role; });
}
