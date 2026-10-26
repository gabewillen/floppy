# The listening's statements and cases (tests/irp/harness.py writes them).
#
# The listening runs its sensitivity and interpretation (and the
# interpretation's hearing leaves) for real. Each case's audio is in its
# sources/<id>/media/ (16 kHz mono 16-bit WAV: voiced*.wav 0.1 s of constant
# non-zero samples, silence.wav and ring.wav 0.1 s of zeros, said.pcm 2 s
# of the voiced samples, raw); the fake
# backend answers voice detection, diarization, speech decoding and voice
# identification in order (the interpretation's defaults).

S = suite
S.media({"voiced.wav": wav(1000), "voiced2.wav": wav(1000), "silence.wav": wav(), "ring.wav": wav(),
         "said.pcm": pcm(1000, seconds=2.0)})
SOUND = "environment.sound"
EFFERENCE = "bot.ability.speaking.efference"
COGNITION = "bot.ability.cognition.input"
FAILED = "bot.ability.listening.failed"
READY = expect("bot.ability.ready")
PATIENT = [("sensitivity_timeout_ms", "600000")]

VOICED = {"path": "/vad", "status": 200, "body": {"segments": [{"start": 0.5, "end": 0.6}]}}
SILENT = {"path": "/vad", "status": 200, "body": {"segments": []}}


DIARIZED = {"path": "/audio/diarization", "status": 200,
            "body": {"segments": [{"speaker": "SPEAKER_00", "start": 0.0, "end": 0.1}]}}
EMBEDDED = {"path": "/voice/embed", "status": 200, "body": {"embedding": [0.5, -0.25]}}


def decoded(text):
    """An utterance's diarization, words and voice."""
    return [DIARIZED, {"path": "/audio/transcriptions", "status": 200, "body": {"text": text}}, EMBEDDED]


def sound(name, **extra):
    data = {"audio": f"media/{name}", "media_type": "audio/wav", "sample_rate_hz": 16000, "channels": 1}
    data.update(extra)
    return data


def hear(name, **extra):
    return send(SOUND, js(sound(name, **extra)))


def utterance(voiced="voiced.wav", **extra):
    return [hear(voiced, **extra), hear("silence.wav")]


def speech(text, parent="voiced.wav", extra=""):
    product = "_event.data.stimulus.data"
    return expect(COGNITION, "_event.data.stimulus.event === 'bot.ability.listening.speech.output' && _event.data.stimulus.source === 'listening'"
                  f" && {product}.content === {js(text)} && {product}.content_type === 'text/plain' && {product}.media_type === 'audio/pcm'"
                  f" && {product}.sample_rate_hz === 16000 && {product}.channels === 1 && JSON.stringify({product}.source_ids) === '[[0.5,-0.25]]'"
                  f" && {product}.parent.event === 'environment.sound' && {product}.parent.data.audio === 'media/{parent}'" + extra)


S.statement(1, "It waits for both children, the sensitivity and the interpretation (each ready when it sends `bot.ability.ready`), then sends `bot.ability.ready` to its parent (decision D242), with no data: it offers nothing.")
S.statement(2, "Each `environment.sound` is scored by the sensitivity (`bot.ability.listening.sensitivity.score` `{stimulus: {event: \"environment.sound\", data: <the sound>}}`) and the score goes on to the interpretation; the interpretation's speech product is handed to the parent as `bot.ability.cognition.input` `{stimulus: {event: \"bot.ability.listening.speech.output\", data: <the product>, source: \"listening\"}}`, the product's parent the sound's envelope.")
S.statement(3, "A sound is scored at its `received_level_db`: a product the interpretation finds below its threshold is dropped, a louder one handed on.")
S.statement(4, "The interpretation's own `bot.ability.cognition.input` (labelled non-speech) and its `bot.ability.listening.failed` `{stage, message}` go on to the parent unchanged.")
S.statement(5, "`bot.ability.speaking.efference` goes to the sensitivity from wherever perception is, without interrupting it; the score request's stimulus carries the sound's `source` (none when the sound names none), so while the copy's window is open a sound off the copy's `mouth` (speaking stamps its own sound `source: \"speaking\"`) is scored against the prediction, fully predicted at 0 dB, and not interpreted, while a sound from any other source is scored at its received level; a silent copy (`duration` 0) closes the window, so the mouth's sound after it is scored at its received level again. A copy that carries `audio` (a completed utterance's) goes to the interpretation too.")
S.statement(6, "Sounds that arrive before the children are ready, or while one is being scored, are held (`<event:defer/>`) and taken in order.")
S.statement(7, "Scoring is bounded by `sensitivity_timeout_ms` (30 s): its timer answers `bot.ability.listening.failed` `{stage: \"sensitivity\", message: \"Listening sensitivity timed out.\"}` and returns to `Listening`.")
S.statement(8, "A sound its schema refuses (no audio reference) is never routed here: the send raises `error.execution` in the sender.")

S.case(100, 1, "the listening is ready once its children are ready.", [READY])

S.case(110, 2, "a heard utterance is decoded and handed on as a cognition input.",
       [READY] + utterance() + [speech("what is the weather",
                                       extra=" && _event.data.stimulus.data.voice_detection.segments.length === 0"
                                             " && _event.data.stimulus.data.parent.data.media_type === 'audio/wav'")],
       params=PATIENT, backend=[VOICED, SILENT, *decoded("what is the weather")])

S.case(120, 3, "a quiet utterance is dropped; one at the threshold is handed on.",
       [READY] + utterance(received_level_db=2.5) + utterance("voiced2.wav", received_level_db=3)
       + [speech("loud", parent="voiced2.wav")],
       params=PATIENT, backend=[VOICED, SILENT, *decoded("quiet"), VOICED, SILENT, *decoded("loud")])

S.case(130, 4, "labelled non-speech passes through as the normalized environment.sound.",
       [READY, hear("ring.wav", kind="phone.ringing"),
        expect(COGNITION, "_event.data.stimulus.event === 'environment.sound' && _event.data.stimulus.source === 'listening'"
                          " && _event.data.stimulus.data.audio === 'media/floppy-sound-1.pcm' && _event.data.stimulus.data.media_type === 'audio/pcm'"
                          " && _event.data.stimulus.data.kind === 'phone.ringing'")],
       params=PATIENT, backend=[SILENT])
S.case(131, 4, "the interpretation's failure is the listening's.",
       [READY, hear("missing.wav"),
        expect(FAILED, "_event.data.stage === 'voice_detection' && _event.data.message.endsWith('failed: the audio cannot be read.')")],
       params=PATIENT)

S.case(140, 5, "while the efference copy's window is open a sound is scored at its received level, not as the mouth's.",
       [READY, send(EFFERENCE, js({"mouth": "speaking", "duration": 60})),
        hear("voiced.wav", amplitude_db=60, received_level_db=54), hear("silence.wav", received_level_db=54),
        speech("cut in")],
       params=PATIENT, backend=[VOICED, SILENT, *decoded("cut in")])
S.case(141, 5, "while the efference copy's window is open the bot's own sound (source the copy's mouth) is not interpreted; a sound with no source is handed on.",
       [READY, send(EFFERENCE, js({"mouth": "speaking", "duration": 60})),
        hear("voiced.wav", source="speaking"), hear("silence.wav", source="speaking"),
        hear("voiced2.wav"), hear("silence.wav"),
        speech("cut in", parent="voiced2.wav")],
       params=PATIENT, backend=[VOICED, SILENT, *decoded("cut in")])
S.case(142, 5, "a silent copy closes the efference copy's window: the bot's own sound after it is handed on.",
       [READY, send(EFFERENCE, js({"mouth": "speaking", "duration": 60})), send(EFFERENCE, js({"mouth": "speaking", "duration": 0})),
        hear("voiced.wav", source="speaking"), hear("silence.wav", source="speaking"),
        speech("own voice")],
       params=PATIENT, backend=[VOICED, SILENT, *decoded("own voice")])

S.case(143, 5, "a completed utterance's copy, with its audio, teaches the interpretation the bot's own voice: that voice is no source id.",
       [READY, send(EFFERENCE, js({"mouth": "speaking", "duration": 0, "audio": "media/said.pcm", "media_type": "audio/pcm",
                                   "sample_rate_hz": 16000, "channels": 1})),
        hear("voiced.wav"), hear("silence.wav"),
        expect(COGNITION, "_event.data.stimulus.data.content === 'own voice' && _event.data.stimulus.data.source_ids.length === 0")],
       params=PATIENT, backend=[{"path": "/vad", "status": 200, "body": {"segments": [{"start": 0.5, "end": 2.0}]}}, EMBEDDED,
                                VOICED, SILENT, *decoded("own voice")])

S.case(150, 6, "sounds sent before the listening is ready, and back to back, are interpreted in order.",
       utterance() + utterance("voiced2.wav") + [READY, speech("first"), speech("second", parent="voiced2.wav")],
       params=PATIENT, backend=[VOICED, SILENT, *decoded("first"), VOICED, SILENT, *decoded("second")])

S.case(160, 7, "a score that outlasts sensitivity_timeout_ms fails the listening's sensitivity stage.",
       [READY, hear("ring.wav", kind="knock"),
        expect(FAILED, "_event.data.stage === 'sensitivity' && _event.data.message === 'Listening sensitivity timed out.'")],
       params=[("sensitivity_timeout_ms", "0")], ignore=[COGNITION], backend=[SILENT], clock="wall")

S.case(170, 8, "a sound without an audio reference is refused at the send; the next is heard.",
       [READY, send(SOUND, js({"kind": "knock"})), expect("error.execution"),
        hear("ring.wav", kind="knock"),
        expect(COGNITION, "_event.data.stimulus.event === 'environment.sound' && _event.data.stimulus.data.kind === 'knock'")],
       params=PATIENT, backend=[SILENT])
