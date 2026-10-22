# The interpretation's statements and cases (tests/irp/harness.py writes them).
#
# Each case's audio is in its sources/<id>/media/ (16 kHz mono 16-bit:
# voiced*.wav 0.1 s of constant non-zero samples, silence.wav 0.1 s of
# zeros, *.pcm the same samples raw); the fake backend answers what each
# stage asks, in order.

S = suite
S.media({"voiced.wav": wav(1000), "voiced2.wav": wav(1000), "silence.wav": wav(), "ring.wav": wav(), "hum.wav": wav(),
         "voiced.pcm": pcm(1000), "silence.pcm": pcm()})
SCORED = "bot.ability.listening.sensitivity.output"
SPEECH = "bot.ability.listening.speech.output"
COGNITION = "bot.ability.cognition.input"
FAILED = "bot.ability.listening.failed"
READY = expect("bot.ability.offers", "_event.data.ability === 'interpretation' && _event.data.choices.length === 0")
# Stages under a loaded host can outlast the original's 30 s bound: cases
# that reach the backend wait longer (statement 11 tests the bound itself).
PATIENT = [("stage_timeout_ms", "600000")]
NO_DECODER = PATIENT + [("speech_decoder", "false")]
STREAMING = NO_DECODER + [("sound_classifier", "false")]
DIARIZING = NO_DECODER + [("diarizer", "true")]

VOICED = {"path": "/vad", "status": 200, "body": {"segments": [{"start": 0.0, "end": 0.1}]}}
SILENT = {"path": "/vad", "status": 200, "body": {"segments": []}}
DIARIZED = {"path": "/audio/diarization", "status": 200,
            "body": {"task": "diarize", "duration": 0.2, "num_speakers": 1,
                     "segments": [{"id": 0, "speaker": "SPEAKER_00", "label": "SPEAKER_00", "start": 0.0, "end": 0.1}]}}
EMBEDDED = {"path": "/voice/embed", "status": 200, "body": {"embedding": [0.5, -0.25], "dim": 2, "model": "voice-detect-wespeaker-resnet34"}}


def decoded(text):
    return {"path": "/interactions", "status": 200,
            "body": {"steps": [{"type": "model_output", "content": [{"type": "text", "text": text}]}]}}


DECODE_ERROR = {"path": "/interactions", "status": 500, "body": {"error": {"message": "boom"}}}


def wav(name, level=None, **extra):
    sound = {"audio": f"media/{name}", "media_type": "audio/wav", "sample_rate_hz": 16000, "channels": 1}
    sound.update(extra)
    return {"sound": sound, "parent": {"event": "environment.sound", "data": sound}, "perceived_level_db": level}


def raw(name, level=None, **extra):
    sound = {"audio": f"media/{name}"}
    sound.update(extra)
    return {"sound": sound, "parent": {"event": "environment.sound", "data": sound}, "perceived_level_db": level}


def hear(scored):
    return send(SCORED, js(scored))


def utterance(voiced="voiced.wav", level=None):
    return [hear(wav(voiced, level)), hear(wav("silence.wav"))]


PCM_FORMAT = " && _event.data.sample_rate_hz === 16000 && _event.data.channels === 1 && _event.data.media_type === 'audio/pcm'"


def words(text, parent="voiced.wav", extra=""):
    """A decoded speech product."""
    return expect(SPEECH, f"_event.data.content === {js(text)} && _event.data.content_type === 'text/plain'" + PCM_FORMAT +
                  " && _event.data.voice_detection.segments.length === 0 && _event.data.source_ids.length === 0"
                  " && !('voice_embedding' in _event.data)"
                  f" && _event.data.parent.event === 'environment.sound' && _event.data.parent.data.audio === 'media/{parent}'" + extra)


def frame(serial, spans, parent, extra=""):
    """A streamed (or silence) PCM speech product of a sound's frame."""
    return expect(SPEECH, f"_event.data.content === 'media/floppy-sound-{serial}.pcm' && _event.data.content_type === 'audio/pcm'" + PCM_FORMAT +
                  f" && _event.data.voice_detection.segments.length === {spans}"
                  f" && _event.data.parent.data.audio === 'media/{parent}'" + extra)


def failed(stage, message=None, ends=None):
    cond = f"_event.data.stage === {js(stage)}"
    if message is not None:
        cond += f" && _event.data.message === {js(message)}"
    if ends is not None:
        cond += f" && typeof _event.data.message === 'string' && _event.data.message.endsWith({js(ends)})"
    return expect(FAILED, cond)


def stimulus(cond):
    return expect(COGNITION, "_event.data.stimulus.event === 'environment.sound' && _event.data.stimulus.source === 'listening' && " + cond)


S.statement(1, "On start it sends `bot.ability.offers` `{ability: \"interpretation\", choices: []}` to its parent: it offers nothing.")
S.statement(2, "The original's constructor refusals are kept: voice identification (`voice_classifier`) with speech decoding (`speech_decoder`), a negative `product_threshold_db`, and a `recognize_timeout_ms` that is not positive each end the interpretation in its final state `degraded` without offering.")
S.statement(3, "With the default configuration (voice detection, sound classification, speech decoding), a voiced scored sound opens the open-speech window, each further voiced sound joins it, and the first sound without voice closes it; the utterance is joined into a WAV file, decoded, and answered as `bot.ability.listening.speech.output` `{content: <words>, content_type: \"text/plain\", voice_detection, sample_rate_hz, channels, media_type: \"audio/pcm\", source_ids: [], parent}`, its parent the loudest frame's (the first with a level when none is louder).")
S.statement(4, "A sound without voice is classified: a labelled one (a non-blank kind) is handed to the parent as `bot.ability.cognition.input` `{stimulus}`, the stimulus `{event: \"environment.sound\", data: <the sound as voice detection normalized it, its PCM written beside it as floppy-sound-<n>.pcm>, source: \"listening\"}`; an unlabelled one is dropped while speech decoding is configured.")
S.statement(5, "A product is handed off only when audible: its `perceived_level_db` null or at least `product_threshold_db` (3 dB); a quieter one is dropped.")
S.statement(6, "With neither speech decoding nor sound classification, a sound without voice is a silence observation: a speech product of the sound's PCM (`content` its floppy-sound-<n>.pcm, `content_type` `audio/pcm`, no voice spans); a PCM sound with no sample rate is handed off as the `environment.sound` stimulus instead. While the window is open and neither decoding nor diarization owns the product, each frame (the closing one too) is streamed as such a speech product with its voice spans.")
S.statement(7, "With diarization the closed utterance is diarized: one speech product per clipped segment (`content` floppy-diarized-<n>-<index>, its span, `confidence`, a `voice_detection` of its own length, `voice_embedding` null and no source ids), or, with voice identification too, each identified by its embedding (`source_ids` `[embedding]`, `voice_embedding` the embedding); with speech decoding too, the utterance is decoded instead.")
S.statement(8, "Voice identification without diarization identifies the opening voice span directly: its one embedding is the window's source identity, carried by every streamed frame; a sound that is not PCM with a sample rate cannot be identified and fails the `voice_identification` stage. Identification waits for its voices at most `stage_timeout_ms` + `recognize_timeout_ms` (the original's identification deadline, 2 s past the stage bound by default); past that, or with no opening embedding, it completes with nobody identified (the original's identifying nobody): the window opens with no source identity, or each diarized segment is handed off with no embedding.")
S.statement(9, "A stage failure (its child's failure, an error, or its timer) answers `bot.ability.listening.failed` `{stage, message}` and returns to `Idle`, where the next sound is interpreted; decoded speech of a sound with no sample rate fails the `speech_decoding` stage.")
S.statement(10, "Scored sounds that arrive while one is being interpreted are held (`<event:defer/>`) and taken in order.")
S.statement(11, "Each stage but identification (statement 8) is bounded by `stage_timeout_ms` (the original's 30 s): its timer fails it with `{stage, message: \"Listening <stage> timed out.\"}`.")

S.case(100, 1, "the interpretation offers nothing on start.", [READY])

S.case(110, 2, "voice identification with speech decoding degrades the interpretation.",
       [expect("done.invoke.interpretation")], params=[("voice_classifier", "true")])
S.case(111, 2, "a negative product threshold degrades the interpretation.",
       [expect("done.invoke.interpretation")], params=[("product_threshold_db", "-1")])
S.case(112, 2, "a recognize timeout that is not positive degrades the interpretation.",
       [expect("done.invoke.interpretation")], params=[("recognize_timeout_ms", "0")])
S.case(113, 2, "a zero product threshold is no refusal: the interpretation offers.",
       [READY], params=[("product_threshold_db", "0")])

S.case(120, 3, "a voiced sound and a silent one are one utterance, decoded into words.",
       [READY] + utterance() + [words("hello")],
       params=PATIENT, backend=[VOICED, SILENT, decoded("hello")])
S.case(121, 3, "the open window joins voiced sounds; the loudest frame is the product's parent.",
       [READY, hear(wav("voiced.wav", 40)), hear(wav("voiced2.wav", 50)), hear(wav("silence.wav")),
        words("hello world", parent="voiced2.wav", extra=" && _event.data.parent.data.audio === 'media/voiced2.wav'")],
       params=PATIENT, backend=[VOICED, VOICED, SILENT, decoded("hello world")])

S.case(130, 4, "a labelled sound without voice is handed off as the normalized environment.sound.",
       [READY, hear(wav("ring.wav", kind=" phone.ringing ")),
        stimulus("_event.data.stimulus.data.audio === 'media/floppy-sound-1.pcm' && _event.data.stimulus.data.media_type === 'audio/pcm'"
                  " && _event.data.stimulus.data.sample_rate_hz === 16000 && _event.data.stimulus.data.channels === 1"
                  " && _event.data.stimulus.data.kind === ' phone.ringing '")],
       params=PATIENT, backend=[SILENT])
S.case(131, 4, "an unlabelled sound without voice is dropped; the next utterance is decoded.",
       [READY, hear(wav("hum.wav", kind="  "))] + utterance() + [words("hello")],
       params=PATIENT, backend=[SILENT, VOICED, SILENT, decoded("hello")])

S.case(140, 5, "a quiet utterance is decoded and dropped; one at the threshold is handed off.",
       [READY] + utterance(level=2.5) + utterance("voiced2.wav", level=3) + [words("loud", parent="voiced2.wav")],
       params=PATIENT, backend=[VOICED, SILENT, decoded("quiet"), VOICED, SILENT, decoded("loud")])

S.case(150, 6, "with neither decoder nor classifier, a sound without voice is a silence observation of its PCM.",
       [READY, hear(wav("hum.wav")), frame(1, 0, "hum.wav", " && _event.data.source_ids.length === 0 && !('voice_embedding' in _event.data)")],
       params=STREAMING, backend=[SILENT])
S.case(151, 6, "a PCM sound with no sample rate is handed off as the environment.sound stimulus.",
       [READY, hear(raw("silence.pcm", media_type="audio/pcm")),
        stimulus("_event.data.stimulus.data.audio === 'media/silence.pcm' && _event.data.stimulus.data.media_type === 'audio/pcm'"
                  " && _event.data.stimulus.data.sample_rate_hz == null")],
       params=STREAMING, backend=[SILENT])
S.case(152, 6, "without a decoder, each frame of the open window is streamed, the closing one too.",
       [READY] + utterance() + [frame(1, 1, "voiced.wav", " && _event.data.voice_detection.segments[0].end_seconds === 0.1"),
                                frame(2, 0, "silence.wav")],
       params=STREAMING, backend=[VOICED, SILENT])

SEGMENT = (" && _event.data.content === 'media/floppy-diarized-2-0' && _event.data.content_type === 'audio/pcm'" + PCM_FORMAT +
           " && _event.data.start_seconds === 0 && _event.data.end_seconds === 0.1 && _event.data.confidence === null"
           " && _event.data.voice_detection.segments.length === 1 && _event.data.voice_detection.segments[0].end_seconds === 0.1"
           " && _event.data.parent.data.audio === 'media/voiced.wav'")
S.case(160, 7, "a diarized utterance is one speech product per segment.",
       [READY] + utterance() + [expect(SPEECH, "_event.data.voice_embedding === null && _event.data.source_ids.length === 0" + SEGMENT)],
       params=DIARIZING, backend=[VOICED, SILENT, DIARIZED])
S.case(161, 7, "diarized segments are identified by their voice embedding.",
       [READY] + utterance() + [expect(SPEECH, "JSON.stringify(_event.data.source_ids) === '[[0.5,-0.25]]'"
                                               " && JSON.stringify(_event.data.voice_embedding.embedding) === '[0.5,-0.25]'" + SEGMENT)],
       params=DIARIZING + [("voice_classifier", "true")], backend=[VOICED, SILENT, DIARIZED, EMBEDDED])
S.case(162, 7, "with speech decoding too, the diarized utterance is decoded.",
       [READY] + utterance() + [words("hello")],
       params=PATIENT + [("diarizer", "true")], backend=[VOICED, SILENT, DIARIZED, decoded("hello")])

IDENTIFIED = (" && JSON.stringify(_event.data.source_ids) === '[[0.5,-0.25]]'"
              " && JSON.stringify(_event.data.voice_embedding.embedding) === '[0.5,-0.25]'")
S.case(170, 8, "the opening voice is identified directly and every streamed frame carries its identity.",
       [READY] + utterance() + [frame(1, 1, "voiced.wav", IDENTIFIED), frame(2, 0, "silence.wav", IDENTIFIED)],
       params=NO_DECODER + [("voice_classifier", "true")], backend=[VOICED, EMBEDDED, SILENT])
S.case(171, 8, "a voiced sound with no sample rate cannot be identified.",
       [READY, hear(raw("voiced.pcm", media_type="audio/pcm")),
        failed("voice_identification", "Voice identification opening segment conversion failed: voice identification requires audio/pcm with sample rate and channels.")],
       params=NO_DECODER + [("voice_classifier", "true")], backend=[VOICED])

S.case(180, 9, "audio that cannot be read fails voice detection; the next sound is interpreted.",
       [READY, hear(wav("missing.wav")), failed("voice_detection", ends="failed: the audio cannot be read."),
        hear(wav("ring.wav", kind="knock")), stimulus("_event.data.stimulus.data.kind === 'knock'")],
       params=PATIENT, backend=[SILENT])
S.case(181, 9, "a decoder error fails speech decoding; the next utterance is decoded.",
       [READY] + utterance() + [failed("speech_decoding", "Speech decoding failed: the backend answered an error status.")]
       + utterance("voiced2.wav") + [words("again", parent="voiced2.wav")],
       params=PATIENT, backend=[VOICED, SILENT, DECODE_ERROR, VOICED, SILENT, decoded("again")])
S.case(182, 9, "speech with no sample rate is decoded and then cannot be placed on a timeline.",
       [READY, hear(raw("voiced.pcm")), hear(raw("silence.pcm")),
        failed("speech_decoding", "Speech decoding completed on sound with no sample rate to place it on a timeline.")],
       params=PATIENT, backend=[VOICED, SILENT, decoded("hello")])

S.case(190, 10, "utterances sent together are interpreted in order.",
       [READY] + utterance() + utterance("voiced2.wav") + [words("first"), words("second", parent="voiced2.wav")],
       params=PATIENT, backend=[VOICED, SILENT, decoded("first"), VOICED, SILENT, decoded("second")])

S.case(200, 11, "a stage that outlasts stage_timeout_ms times out.",
       [READY, hear(wav("voiced.wav")), failed("voice_detection", "Listening voice_detection timed out.")],
       params=[("stage_timeout_ms", "1")], backend=[VOICED], clock="wall")
