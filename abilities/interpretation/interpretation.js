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

// A stage child's (or the speech service's) failure {message}. The
// decoder's names its own stage: it decodes while the voices are heard.
function childFailure(_event) {
  return ({stage: stage, message: Object(_event.data).message});
}

// A stage's request id: the stage, then the serial of the sound in hand,
// so a stage's answer about an earlier sound is never taken for this one.
function requestId(child) {
  return child + '.' + serial;
}

function isReply(_event, child) {
  return _event.data.id === requestId(child);
}

// A failure of a stage that fails the interpretation's work.
function isStageReply(_event) {
  return ['voice_detection', 'voice_diarization', 'voice_identification', 'speech_decoding']
    .some(function (child) { return isReply(_event, child); });
}

function stageReplyFailure(_event) {
  return ({stage: _event.data.id.slice(0, _event.data.id.lastIndexOf('.')), message: _event.data.message});
}

function hasAudio(_event) {
  return typeof Object(_event.data).audio === 'string';
}

// A speech product of a scored sound.
function speechProduct(content, contentType, scored, voiceDetection, sourceIds) {
  var product = ({content: content, content_type: contentType, voice_detection: voiceDetection});
  product.sample_rate_hz = scored.sound.sample_rate_hz;
  product.channels = soundChannels(scored.sound);
  product.media_type = 'audio/pcm';
  product.source_ids = sourceIds;
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

// The current stage's failure, or the decoder's while it decodes.
function isCurrentStageFailure(_event) {
  return _event.data.stage === stage || (_event.data.stage === 'speech_decoding' && In('DecodingSpeech'));
}

function hasStage() {
  return stage !== null;
}

function stageErrorFailure(_event) {
  return ({stage: stage, message: 'Listening ' + stage + ' could not run (' + _event.name + ')'});
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

// The request for the sound in hand to the voice detection.
function detectionInput(sound) {
  return ({id: requestId('voice_detection'), audio: sound.audio, media_type: orNull(sound.media_type),
    sample_rate_hz: orNull(sound.sample_rate_hz), channels: orNull(sound.channels), output: soundOutput()});
}

// The sound's voice spans, as the products carry them.
function detectedVoice(_event) {
  return ({segments: _event.data.segments});
}

function detectionRecord() {
  return ({sensed: sensed, voice_detection: {segments: result.segments}});
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
  return speechProduct(detection.sensed.sound.audio, 'audio/pcm', detection.sensed, detection.voice_detection, active_source_ids);
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

// Diarized while it is decoded when speech decoding is configured
// (DecodingSpeech.Diarized), else on its own (DiarizingVoice).
function isUtteranceToDiarize(_event) {
  return diarizer && hasAudio(_event);
}

// ClassifyingSound

function classificationInput() {
  return ({id: requestId('sound_classification'), audio: detection.sensed.sound.audio,
    media_type: orNull(detection.sensed.sound.media_type), kind: orNull(detection.sensed.sound.kind)});
}

function hasAnyLabel(_event) {
  return _event.data.labels.length > 0;
}

// DiarizingVoice

function diarizedOutput() {
  return media_dir + 'floppy-diarized-' + serial;
}

// Identified before anything else only when nothing decodes: decoded
// speech is identified while its words are decoded (Identifying).
function diarizationInput() {
  return ({id: requestId('voice_diarization'), audio: utterance.audio, media_type: utterance.media_type,
    sample_rate_hz: utterance.sample_rate_hz, channels: utterance.channels, output: diarizedOutput()});
}

function isDiarizedToIdentify() {
  return voice_classifier && !speech_decoder;
}

// Diarized segments whose voices are identified while the words are
// decoded.
function identifiesSegments(_event) {
  return voice_classifier && _event.data.segments.length > 0;
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

function openingIdentificationInput() {
  return ({id: requestId('voice_identification'), audio: detection.sensed.sound.audio,
    sample_rate_hz: detection.sensed.sound.sample_rate_hz, channels: soundChannels(detection.sensed.sound),
    start_seconds: openingStart(), end_seconds: openingEnd(), output: voiceOutput()});
}

function identificationInput(segments) {
  return ({id: requestId('voice_identification'), segments: segments});
}

function hasNoEmbedding(_event) {
  return _event.data.embeddings.length === 0;
}

function firstEmbedding(_event) {
  return _event.data.embeddings[0];
}

// The opening voice's ref, unless it is the bot's own.
function activeSourceIds(_event) {
  return sourceIds(identify([firstEmbedding(_event)]));
}

// The identification deadline (decision D177): a stage, then the
// registry's answer.
function identifyDelay() {
  return (stage_timeout_ms + recognize_timeout_ms) + 'ms';
}

function isIdentifyTimer(_event) {
  return _event.sendid === 'direct-identification-timer' || _event.sendid === 'voice-identification-timer'
    || _event.sendid === 'speech-voice-identification-timer';
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
function unidentifiedVoices() {
  return completion.segments.map(function () { return null; });
}

// The segments embedded: the voiced ones while decoding, else all.
function identifiedSegments() {
  return In('Identifying') ? voicedSegments() : completion.segments;
}

function isEmbeddingCountMismatch(_event) {
  return _event.data.embeddings.length !== identifiedSegments().length;
}

function embeddingCountMismatch(_event) {
  return 'received ' + _event.data.embeddings.length + ' for ' + identifiedSegments().length + ' segments.';
}

function embeddingCountFailure() {
  return stageFailure('Voice identification must return exactly one embedding per voice segment; ' + mismatch);
}

// DecodingSpeech

function decodingCompletion() {
  return ({kind: 'decoding', sensed: open_sensed || detection.sensed, voice_detection: detection.voice_detection, text: result.text,
    segments: isDiarization() ? completion.segments : []});
}

// Decoded words after diarization (In() no longer holds once the
// transition's content runs: its source state has been exited).
function isDiarization() {
  return completion !== null && completion.kind === 'diarization';
}

// The words decoded and the tone heard (or not asked for).
function isDecoded() {
  return result !== null && !classifying;
}

// The voiced segments' embeddings are in (null: nobody was identified in
// time).
function hasVoices() {
  return completion !== null && completion.embeddings !== undefined;
}

// Decoded words with who spoke: each voiced segment's voice is identified
// once the words are in, so a voice is minted or refined only for speech
// that was decoded.
function identifiedDecodingCompletion() {
  var segments = voicedSegments();
  var embeddings = completion.embeddings;
  var decoded = decodingCompletion();
  decoded.segments = segments;
  decoded.recognitions = embeddings === null ? segments.map(function () { return null; }) : identify(embeddings);
  return decoded;
}

// The voices whose tone is heard (mosfet.py's tone): the diarized
// segments, else the whole utterance; none without an affect classifier.
function affectSegments() {
  if (!affect_classifier) {
    return [];
  }
  if (isDiarization() && Array.isArray(completion.segments)) {
    return completion.segments;
  }
  return [{audio: utterance.audio, media_type: utterance.media_type, sample_rate_hz: utterance.sample_rate_hz, channels: utterance.channels}];
}

// The requests of DecodingSpeech: the utterance to decode, and its voices
// whose tone is heard.
function decodingInput() {
  return ({id: requestId('speech_decoding'), audio: utterance.audio, media_type: utterance.media_type});
}

function affectInput() {
  return ({id: requestId('voice_affect'), segments: affectSegments()});
}

// The tone voice_affect heard (mosfet.py's voice.Affect), without the
// request's id.
function heardAffect(_event) {
  var data = _event.data;
  return ({label: data.label, scores: data.scores, valence: data.valence, confidence: data.confidence});
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
  var product = speechProduct(completion.text, 'text/plain', completion.sensed, completion.voice_detection, sourceIds(completion.recognitions));
  if (affect !== null) {
    product.affect = affect;
  }
  return product;
}

// identification (mosfet.py's identification): the interpretation owns
// the voice registry, so a print never leaves it; a product names each
// voice by its ref, voice:<id>.

// A print's vector, L2-normalized (mosfet.py's Print); null for one that
// is not a non-empty array of finite numbers with a norm.
function printVector(voice) {
  var vector = voice === null || typeof voice !== 'object' ? null : voice.embedding;
  if (!Array.isArray(vector) || vector.length === 0 || !vector.every(function (value) { return typeof value === 'number' && isFinite(value); })) {
    return null;
  }
  var norm = Math.sqrt(vector.reduce(function (sum, value) { return sum + value * value; }, 0));
  return norm === 0 ? null : vector.map(function (value) { return value / norm; });
}

// The cosine of two normalized vectors; null across vector spaces.
function similarity(a, b) {
  if (a.length !== b.length) {
    return null;
  }
  var dot = 0;
  for (var i = 0; i < a.length; i++) {
    dot += a[i] * b[i];
  }
  return dot;
}

// mosfet.py's identification.decide over the registry's ids (own ones
// only, when own): the best must reach match_policy.threshold and beat the
// runner-up by its margin, and one at refine_threshold also refines;
// anything else mints. {kind: 'match' | 'refine' | 'mint', entry}.
function decide(vector, own) {
  var ranked = registry.filter(function (entry) { return !own || entry.own; }).map(function (entry) {
    return ({entry: entry, similarity: similarity(entry.vector, vector)});
  }).filter(function (neighbor) { return neighbor.similarity !== null; }).sort(function (a, b) { return b.similarity - a.similarity; });
  if (ranked.length === 0 || ranked[0].similarity < match_policy.threshold
      || (ranked.length > 1 && ranked[0].similarity - ranked[1].similarity < match_policy.margin)) {
    return ({kind: 'mint', entry: null});
  }
  return ({kind: ranked[0].similarity >= match_policy.refine_threshold ? 'refine' : 'match', entry: ranked[0].entry});
}

// A minted id, as mosfet.py mints uuid4().hex: 32 hex digits, the clock's
// first, so ids minted by another run never repeat these.
function mintedId() {
  minted = minted + 1;
  var id = Date.now().toString(16) + minted.toString(16);
  while (id.length < 32) {
    id = id + Math.floor(Math.random() * 16).toString(16);
  }
  return id.slice(0, 32);
}

// An id seen now: last in the registry, which is in the order ids were
// seen (mosfet.py's seen_at).
function seen(entry) {
  registry = registry.filter(function (other) { return other !== entry; }).concat([entry]);
  return entry;
}

// A new id for a print, its ref voice:<id>: at max_ids the least
// recently seen id that is not own makes room (mosfet.py's
// InProcessStore). With a memory, an id that is not own is remembered
// as a person, the print as heard their first voice and its compact text
// their representative.
function mint(vector, own, voice) {
  if (registry.length >= max_ids) {
    var evicted = registry.filter(function (entry) { return !entry.own; })[0];
    registry = registry.filter(function (entry) { return entry !== evicted; });
  }
  var entry = seen(({id: 'voice:' + mintedId(), vector: vector, own: own, seen_count: 1}));
  if (memory !== null && !own) {
    statements = statements.concat([
      ({sql: person_insert, parameters: [entry.id]}),
      ({sql: voice_insert, parameters: [entry.id, 'voice', JSON.stringify(voice.embedding)]}),
      ({sql: representative_insert, parameters: [entry.id, 'voice', encodeVoice(vector)]})
    ]);
  }
  return entry;
}

// A refined id's running mean, weighted by at most refine_weight_cap
// earlier prints; with a memory, a person's representative is the
// refined mean, its weight the prints in it.
function refine(entry, vector) {
  var weight = Math.min(entry.seen_count, match_policy.refine_weight_cap);
  var sum = entry.vector.map(function (value, i) { return value * weight + vector[i]; });
  var refined = printVector(({embedding: sum}));
  entry.vector = refined === null ? entry.vector : refined;
  entry.seen_count = Math.min(entry.seen_count + 1, match_policy.refine_weight_cap);
  if (memory !== null && !entry.own) {
    statements = statements.concat([({sql: representative_update, parameters: [encodeVoice(entry.vector), entry.seen_count, entry.id, 'voice']})]);
  }
  return entry;
}

// The standard base64 alphabet of a voice's compact text, and each
// character's place in it.
var BASE64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
var BASE64_INDEX = {};
for (var place = 0; place < BASE64.length; place++) {
  BASE64_INDEX[BASE64.charAt(place)] = place;
}

// A voice's compact text (the conversation namespace's encode_voice,
// decision D205): i8: and the standard base64 of the unit vector's
// components, each round(component * 127), half away from zero, as a
// signed byte.
function encodeVoice(vector) {
  var bytes = vector.map(function (value) {
    var quantized = Math.max(-127, Math.min(127, (value < 0 ? -1 : 1) * Math.round(Math.abs(value) * 127)));
    return quantized < 0 ? quantized + 256 : quantized;
  });
  var text = 'i8:';
  for (var i = 0; i < bytes.length; i += 3) {
    var a = bytes[i];
    var b = i + 1 < bytes.length ? bytes[i + 1] : 0;
    var c = i + 2 < bytes.length ? bytes[i + 2] : 0;
    var sextets = [a >> 2, ((a & 3) << 4) | (b >> 4), ((b & 15) << 2) | (c >> 6), c & 63];
    for (var k = 0; k < 4; k++) {
      text = text + (k <= bytes.length - i ? BASE64.charAt(sextets[k]) : '=');
    }
  }
  return text;
}

// A representative's unit vector, from its compact text (the conversation
// namespace's decode_voice); null for text of another form.
function decodeVoice(text) {
  if (typeof text !== 'string' || text.slice(0, 3) !== 'i8:' || text.length === 3 || (text.length - 3) % 4 !== 0) {
    return null;
  }
  var components = [];
  for (var i = 3; i < text.length; i += 4) {
    var last = i + 4 === text.length;
    var padding = last && text.charAt(i + 3) === '=' ? (text.charAt(i + 2) === '=' ? 2 : 1) : 0;
    var word = 0;
    for (var k = 0; k < 4; k++) {
      var sextet = k < 4 - padding ? BASE64_INDEX[text.charAt(i + k)] : 0;
      if (sextet === undefined) {
        return null;
      }
      word = word * 64 + sextet;
    }
    var bytes = [Math.floor(word / 65536) % 256, Math.floor(word / 256) % 256, word % 256];
    for (var b = 0; b < 3 - padding; b++) {
      components.push((bytes[b] > 127 ? bytes[b] - 256 : bytes[b]) / 127);
    }
  }
  return printVector(({embedding: components}));
}

// Recalling: one person memory holds (a row [person_id, centroid,
// weight], the most recently seen first), placed before those already
// recalled as an id that is not own, weighted by the prints in its
// representative; a damaged representative is left out. One row a call,
// so recalling many people stays within a data-model call's fuel.
function withRecalled(row) {
  var vector = decodeVoice(row.values[1]);
  if (vector === null) {
    return registry;
  }
  var weight = Math.max(1, Math.min(Number(row.values[2]) || 1, match_policy.refine_weight_cap));
  return [({id: row.values[0], vector: vector, own: false, seen_count: weight})].concat(registry);
}

function recalledRows(_event) {
  var rows = Object(Object(Object(_event.data).results)[0]).rows;
  return Array.isArray(rows) ? rows : [];
}

function recallStatements() {
  return [({sql: voice_select, parameters: ['voice']})];
}

function hasStatements() {
  return statements.length > 0;
}

function memoryFailure(_event) {
  var message = Object(_event.data).message;
  return 'the voices heard were not remembered' + (typeof message === 'string' ? ': ' + message : '');
}

// Who each voice is (mosfet.py's Store.identify): {id, recognized, own}
// for each print, matched to an id or minted one, or null for no print.
function identify(voices) {
  return voices.map(function (voice) {
    var vector = printVector(voice);
    if (vector === null) {
      return null;
    }
    var decision = decide(vector, false);
    if (decision.kind === 'mint') {
      return ({id: mint(vector, false, voice).id, recognized: false, own: false});
    }
    var entry = seen(decision.kind === 'refine' ? refine(decision.entry, vector) : decision.entry);
    return ({id: entry.id, recognized: true, own: entry.own});
  });
}

// The registry with the bot's own print enrolled (mosfet.py's
// Store.enroll, own and undeclared): it refines the nearest own id or
// mints an own id.
function enrolledOwn(voice) {
  var vector = printVector(voice);
  if (vector !== null) {
    var decision = decide(vector, true);
    if (decision.kind === 'mint') {
      mint(vector, true, voice);
    } else {
      seen(refine(decision.entry, vector));
    }
  }
  return registry;
}

function hasOwnVoice() {
  return registry.some(function (entry) { return entry.own; });
}

// The refs of the voices recognized (none when nobody was identified) that
// are not the bot's own, each once, at most 4 (mosfet.py's
// MAX_IDENTITY_SET_SIZE).
function sourceIds(recognitions) {
  var refs = [];
  (Array.isArray(recognitions) ? recognitions : []).forEach(function (recognition) {
    var ref = recognition === null || recognition.own ? null : recognition.id;
    if (ref !== null && refs.indexOf(ref) < 0 && refs.length < 4) {
      refs.push(ref);
    }
  });
  return refs;
}

// DetectingOwnVoice, IdentifyingOwnVoice

// Speaking's copy of a completed utterance, with the PCM it commanded,
// while the bot's own voice is unknown and voices are identified.
function learnsOwnVoice(_event) {
  return voice_classifier && !hasOwnVoice() && typeof _event.data.audio === 'string'
    && _event.data.media_type === 'audio/pcm' && typeof _event.data.sample_rate_hz === 'number';
}

// The copy's directory, where the clip of its voice is written.
function ownMediaDir() {
  return utterance.audio.slice(0, utterance.audio.lastIndexOf('/') + 1);
}

function ownVoiceUndetected(_event) {
  var message = Object(_event.data).message;
  return 'the bot\'s own voice could not be detected' + (typeof message === 'string' ? ': ' + message : '');
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
  return _event.data.segments.length > 0
    && spanEnd(_event.data.segments) - _event.data.segments[0].start_seconds >= min_voice_seconds;
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

function ownIdentificationInput() {
  return ({id: requestId('voice_identification'), audio: utterance.audio, sample_rate_hz: utterance.sample_rate_hz,
    channels: utterance.channels, start_seconds: ownVoiceStart(), end_seconds: ownVoiceEnd(), output: voiceOutput()});
}

function hasOneEmbedding(_event) {
  return _event.data.embeddings.length === 1 && _event.data.embeddings[0] !== null
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

// The segment's recognition, after identification.
function segmentRecognition() {
  return completion.kind === 'identification' ? completion.recognitions[index] : null;
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
  product.source_ids = sourceIds([result]);
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

// memory

// Whether the event answers the request the state `name` last sent the
// memory (decision D265): its id is that request's.
function isMemoryReply(name) {
  return memory_requests[name] !== undefined && _event.data.id === memory_requests[name];
}

// Whether the interpretation has a memory to recall from and remember in.
function hasMemory() {
  return memory !== null;
}
