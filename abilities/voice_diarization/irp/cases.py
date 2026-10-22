# The voice_diarization's statements and cases (tests/irp/harness.py writes them).

import json

S = suite
# utterance.wav and utterance.raw: 1 s of 8 kHz mono 16-bit PCM, constant
# non-zero samples (WAV container, raw).
S.media({"utterance.wav": wav(1000, 1, 8000), "utterance.raw": pcm(1000, 1, 8000)})
DONE = "done.invoke.voice_diarization"
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


S.statement(1, "It diarizes the utterance through the speech service's `diarize` (`POST {base}/audio/diarization`, model `pyannote-diarization` by default); its `bot.ability.hearing.voice.diarization.output` ends it in `applied` with the done data (`done.invoke.voice_diarization`) exactly `{segments}`: the original's OutputData, one clipped PCM segment per answered span, ascending by start, each `{audio: \"<output>-<index>\", media_type: \"audio/pcm\", sample_rate_hz, channels, start_seconds, end_seconds, confidence}` (`confidence` `null` when the answer has none); raw speaker labels never enter.")
S.statement(2, "The utterance is a WAV container (its own format) or raw 16-bit PCM in the `sample_rate_hz` and `channels` `<param>`s; raw PCM with no format is a failure.")
S.statement(3, "When the service answers `bot.ability.failed` `{message}` (an error status, an answer with no spans, a span outside the utterance, a format it cannot read), the done data is exactly the original's FailureData `{message}`.")
S.statement(4, "When the diarizer cannot run (a missing `output`, or a malformed `model`, rejects its start with `error.execution`), the done data is `{message: \"VoiceDiarization could not run (<error event>)\"}`.")

# 1: segments.
S.case(100, 1, "spans come back as clipped PCM segments ascending by start, without speaker labels.",
       [expect(DONE, diarized([segment(0, 0.1, 0.4), segment(1, 0.5, 0.9, 0.8)]))],
       params=WAV,
       backend=[{"path": DIARIZE, "body": {"segments": [
           {"start": 0.5, "end": 0.9, "speaker": "SPEAKER_01", "confidence": 0.8},
           {"start": 0.1, "end": 0.4, "speaker": "SPEAKER_00"}]}}])
S.case(101, 1, "the original's start_seconds/end_seconds spans and a probability are read too.",
       [expect(DONE, diarized([segment(0, 0, 1, 0.25)]))],
       params=WAV,
       backend=[{"path": DIARIZE, "body": {"segments": [{"start_seconds": 0, "end_seconds": 1, "probability": 0.25}]}}])

# 2: the utterance's format.
S.case(110, 2, "raw PCM is read in the given format.",
       [expect(DONE, diarized([segment(0, 0.2, 0.6)]))],
       params=RAW, backend=[{"path": DIARIZE, "body": {"segments": [{"start": 0.2, "end": 0.6}]}}])
S.case(111, 2, "raw PCM with no format cannot be diarized.",
       [expect(DONE, message("Voice diarization failed: the audio is not signed 16-bit PCM with a known format."))],
       params=[("audio", "'media/utterance.raw'"), ("output", "'media/segment'")],
       backend=[{"path": DIARIZE, "body": {"segments": [{"start": 0.2, "end": 0.6}]}}])

# 3: the service's failures.
S.case(120, 3, "an error status is the service's failure message.",
       [expect(DONE, message("Voice diarization failed: the backend answered an error status."))],
       params=WAV, backend=[{"path": DIARIZE, "status": 503, "body": {"error": "busy"}}])
S.case(121, 3, "an answer with no spans is malformed.",
       [expect(DONE, message("Voice diarization failed: the backend's answer is malformed."))],
       params=WAV, backend=[{"path": DIARIZE, "body": {"segments": []}}])
S.case(122, 3, "a span past the utterance's end is malformed.",
       [expect(DONE, message("Voice diarization failed: the backend's answer is malformed."))],
       params=WAV, backend=[{"path": DIARIZE, "body": {"segments": [{"start": 0.5, "end": 1.5}]}}])
S.case(123, 3, "an utterance that is not there cannot be read.",
       [expect(DONE, message("Voice diarization failed: the audio cannot be read."))],
       params=[("audio", "'media/missing.wav'"), ("output", "'media/segment'")],
       backend=[{"path": DIARIZE, "body": {"segments": [{"start": 0.2, "end": 0.6}]}}])

# 4: the diarizer cannot run.
S.case(130, 4, "with no output the diarizer cannot start.",
       [expect(DONE, message("VoiceDiarization could not run (error.execution)"))],
       params=[("audio", "'media/utterance.wav'")])
S.case(131, 4, "an empty model rejects the diarizer's start.",
       [expect(DONE, message("VoiceDiarization could not run (error.execution)"))],
       params=WAV + [("model", "''")])
