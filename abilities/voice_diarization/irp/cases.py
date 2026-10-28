# The voice_diarization's statements and cases (tests/irp/harness.py writes them).

import json

S = suite
# utterance.wav and utterance.raw: 1 s of 8 kHz mono 16-bit PCM, constant
# non-zero samples (WAV container, raw).
S.media({"utterance.wav": wav(1000, 1, 8000), "utterance.raw": pcm(1000, 1, 8000)})
INPUT = "bot.ability.hearing.voice.diarization.input"
OUTPUT = "bot.ability.hearing.voice.diarization.output"
FAILED = "bot.ability.failed"
CONFIG = ('model',)


def asked(params, event, cond):
    """A case that asks the long-lived child once: its params that are not
    the child's configuration (CONFIG) are the request's fields, and the
    child answers `event` of which `cond` holds."""
    return dict(steps=[request(INPUT, [param for param in params if param[0] not in CONFIG]), expect(event, answered(cond))],
                params=[param for param in params if param[0] in CONFIG])


DIARIZE = "/v1/audio/diarization"
WAV = [("audio", "'media/utterance.wav'"), ("media_type", "'audio/wav'"), ("output", "'media/segment'")]
RAW = [("audio", "'media/utterance.raw'"), ("sample_rate_hz", "8000"), ("channels", "1"), ("output", "'media/segment'")]


def canonical(value):
    return json.dumps(value, separators=(",", ":"), sort_keys=True)


def segment(index, start, end, confidence=None):
    return {"audio": f"media/segment-{index}", "media_type": "audio/pcm", "sample_rate_hz": 8000, "channels": 1,
            "start_seconds": start, "end_seconds": end, "confidence": confidence}


def diarized(value):
    return f"JSON.stringify(_event.data) === {js(canonical({'segments': value}))}"


def message(value):
    return f"_event.data.message === {js(value)} && Object.keys(_event.data).length === 1"


S.statement(1, "It diarizes the utterance through the speech service's `diarize` (`POST {base}/audio/diarization`, model `pyannote-diarization` by default); its `bot.ability.hearing.voice.diarization.output` is answered to its parent as `bot.ability.hearing.voice.diarization.output` exactly `{id, segments}`: one clipped PCM segment per answered span, ascending by start, each `{audio: \"<output>-<index>\", media_type: \"audio/pcm\", sample_rate_hz, channels, start_seconds, end_seconds, confidence}` (`confidence` `null` when the answer has none); raw speaker labels never enter. It is long-lived: each utterance is a request `bot.ability.hearing.voice.diarization.input` `{id, audio, media_type?, sample_rate_hz?, channels?, output}`.")
S.statement(2, "The utterance is a WAV container (its own format) or raw 16-bit PCM in the request's `sample_rate_hz` and `channels`; raw PCM with no format is a failure.")
S.statement(3, "When the service answers `bot.ability.failed` `{message}` (an error status, an answer with no spans, a span outside the utterance, a format it cannot read), it answers `bot.ability.failed` exactly `{id, message}`.")
S.statement(4, "When the diarizer cannot run (a malformed `model` rejects its start with `error.execution`), it answers `bot.ability.failed` `{id, message: \"VoiceDiarization could not run (<error event>)\"}`.")

# 1: segments.
S.case(100, 1, "spans come back as clipped PCM segments ascending by start, without speaker labels.",
       **asked(WAV, OUTPUT, diarized([segment(0, 0.1, 0.4), segment(1, 0.5, 0.9, 0.8)])), backend=[{"path": DIARIZE, "body": {"segments": [
           {"start": 0.5, "end": 0.9, "speaker": "SPEAKER_01", "confidence": 0.8},
           {"start": 0.1, "end": 0.4, "speaker": "SPEAKER_00"}]}}])
S.case(101, 1, "spans with start_seconds/end_seconds and a probability are read too.",
       **asked(WAV, OUTPUT, diarized([segment(0, 0, 1, 0.25)])), backend=[{"path": DIARIZE, "body": {"segments": [{"start_seconds": 0, "end_seconds": 1, "probability": 0.25}]}}])

# 2: the utterance's format.
S.case(110, 2, "raw PCM is read in the given format.",
       **asked(RAW, OUTPUT, diarized([segment(0, 0.2, 0.6)])), backend=[{"path": DIARIZE, "body": {"segments": [{"start": 0.2, "end": 0.6}]}}])
S.case(111, 2, "raw PCM with no format cannot be diarized.",
       **asked([("audio", "'media/utterance.raw'"), ("output", "'media/segment'")], FAILED, message("Voice diarization failed: the audio is not signed 16-bit PCM with a known format.")), backend=[{"path": DIARIZE, "body": {"segments": [{"start": 0.2, "end": 0.6}]}}])

# 3: the service's failures.
S.case(120, 3, "an error status is the service's failure message.",
       **asked(WAV, FAILED, message("Voice diarization failed: the backend answered an error status.")), backend=[{"path": DIARIZE, "status": 503, "body": {"error": "busy"}}])
S.case(121, 3, "an answer with no spans is malformed.",
       **asked(WAV, FAILED, message("Voice diarization failed: the backend's answer is malformed.")), backend=[{"path": DIARIZE, "body": {"segments": []}}])
S.case(122, 3, "a span past the utterance's end is malformed.",
       **asked(WAV, FAILED, message("Voice diarization failed: the backend's answer is malformed.")), backend=[{"path": DIARIZE, "body": {"segments": [{"start": 0.5, "end": 1.5}]}}])
S.case(123, 3, "an utterance that is not there cannot be read.",
       **asked([("audio", "'media/missing.wav'"), ("output", "'media/segment'")], FAILED, message("Voice diarization failed: the audio cannot be read.")), backend=[{"path": DIARIZE, "body": {"segments": [{"start": 0.2, "end": 0.6}]}}])

# 4: the diarizer cannot run.
S.case(131, 4, "an empty model rejects the diarizer's start.",
       **asked(WAV + [("model", "''")], FAILED, message("VoiceDiarization could not run (error.execution)")))
