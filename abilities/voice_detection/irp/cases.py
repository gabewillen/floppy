# The voice_detection's statements and cases (tests/irp/harness.py writes them).

S = suite
DONE = "done.invoke.voice_detection"
VAD = "/v1/vad"


def sound(audio, media_type, rate, channels):
    return (f"JSON.stringify(_event.data.sound) === {js(json_text({'audio': audio, 'media_type': media_type, 'sample_rate_hz': rate, 'channels': channels}))}")


def json_text(value):
    import json
    return json.dumps(value, separators=(",", ":"), sort_keys=True)


def segments(value):
    return f"JSON.stringify(_event.data.voice_detection) === {js(json_text({'segments': value}))}"


def only(*keys):
    return f"Object.keys(_event.data).sort().join() === {js(','.join(sorted(keys)))}"


def message(value):
    return f"_event.data.message === {js(value)} && {only('message')}"


def detected(sound_cond, spans):
    return f"{only('sound', 'voice_detection')} && {sound_cond} && {segments(spans)}"


WAV = [("audio", "'media/sound.wav'"), ("media_type", "'audio/wav'"), ("output", "'media/sound.pcm'")]
RAW = [("audio", "'media/sound.raw'"), ("sample_rate_hz", "8000"), ("channels", "1"), ("output", "'media/sound.pcm'")]
NONE = {"path": VAD, "body": {"segments": []}}

S.statement(1, "It normalizes the sound, then detects its voice: a WAV container's signed 16-bit PCM is written to `output` and the sound becomes `{audio: output, media_type: \"audio/pcm\", sample_rate_hz, channels}` (the container's own format); the speech service's `detect` (`POST {base}/vad`, model `silero-vad` by default) answers `bot.ability.hearing.voice.detection.output` `{segments}`, and the done data (`done.invoke.voice_detection`) is exactly `{sound, voice_detection: {segments}}`, each segment `{start_seconds, end_seconds, confidence}` (the answer's `confidence`, `probability` or `score`, else `null`) in ascending start order, clamped to the clip.")
S.statement(2, "Raw, `audio/l16` or unnamed audio with a rate and channels normalizes to `audio/pcm` in place (the same `audio`); any other media type, or a WAV container that cannot be read, leaves the sound unchanged, and the detection still runs on it.")
S.statement(3, "A clip with no voice (no spans answered, or only spans outside the clip) is not a failure: its `voice_detection` is `{segments: []}`.")
S.statement(4, "When the detection answers `bot.ability.failed` `{message}` (an error status, a malformed answer, audio that cannot be read), the done data is exactly the original's FailureData `{message}`, with no sound.")
S.statement(5, "When the normalization fails (`bot.ability.failed`, such as an `output` that cannot be written) it ends with that `{message}` and nothing is detected; when a step cannot run (a missing `output`, or a malformed `model`, rejects its start with `error.execution`), the done data is `{message: \"VoiceDetection could not run (<error event>)\"}`.")

# 1: WAV normalized, spans folded.
S.case(100, 1, "a WAV sound is normalized to PCM and its spans come back sorted, clamped and with their confidence.",
       [expect(DONE, detected(sound("media/sound.pcm", "audio/pcm", 8000, 1), [
           {"start_seconds": 0.1, "end_seconds": 0.3, "confidence": 0.9},
           {"start_seconds": 0.5, "end_seconds": 1, "confidence": None}]))],
       params=WAV,
       backend=[{"path": VAD, "body": {"segments": [{"start": 0.5, "end": 2.0}, {"start": 0.1, "end": 0.3, "probability": 0.9}]}}])
S.case(101, 1, "the original's start_seconds/end_seconds spans and a score are read too.",
       [expect(DONE, detected(sound("media/sound.pcm", "audio/pcm", 8000, 1), [
           {"start_seconds": 0.25, "end_seconds": 0.75, "confidence": 0.5}]))],
       params=WAV,
       backend=[{"path": VAD, "body": {"segments": [{"start_seconds": 0.25, "end_seconds": 0.75, "score": 0.5}]}}])

# 2: other sounds.
S.case(110, 2, "unnamed raw audio with a rate and channels is audio/pcm in place.",
       [expect(DONE, detected(sound("media/sound.raw", "audio/pcm", 8000, 1), [
           {"start_seconds": 0, "end_seconds": 0.5, "confidence": None}]))],
       params=RAW, backend=[{"path": VAD, "body": {"segments": [{"start": 0, "end": 0.5}]}}])
S.case(111, 2, "audio/l16 with a rate and channels is audio/pcm in place.",
       [expect(DONE, detected(sound("media/sound.raw", "audio/pcm", 8000, 1), []))],
       params=RAW + [("media_type", "'audio/l16'")], backend=[NONE])
S.case(112, 2, "another media type is left unchanged and still detected.",
       [expect(DONE, detected(sound("media/sound.raw", "audio/mpeg", None, None), []))],
       params=[("audio", "'media/sound.raw'"), ("media_type", "'audio/mpeg'"), ("output", "'media/sound.pcm'")],
       backend=[NONE])
S.case(113, 2, "a WAV container that cannot be read is left unchanged.",
       [expect(DONE, detected(sound("media/bad.wav", "audio/wav", None, None), []))],
       params=[("audio", "'media/bad.wav'"), ("media_type", "'audio/wav'"), ("output", "'media/sound.pcm'")],
       backend=[NONE])

# 3: no voice.
S.case(120, 3, "no spans answered is no voice.",
       [expect(DONE, detected(sound("media/sound.pcm", "audio/pcm", 8000, 1), []))],
       params=WAV, backend=[NONE])
S.case(121, 3, "spans outside the clip are dropped.",
       [expect(DONE, detected(sound("media/sound.pcm", "audio/pcm", 8000, 1), []))],
       params=WAV, backend=[{"path": VAD, "body": {"segments": [{"start": 1.5, "end": 2.0}]}}])

# 4: the detection's failures.
S.case(130, 4, "an error status is the detection's failure message, with no sound.",
       [expect(DONE, message("Voice detection failed: the backend answered an error status."))],
       params=WAV, backend=[{"path": VAD, "status": 500, "body": {"error": "boom"}}])
S.case(131, 4, "an answer with no segments is malformed.",
       [expect(DONE, message("Voice detection failed: the backend's answer is malformed."))],
       params=WAV, backend=[{"path": VAD, "body": {"spans": []}}])
S.case(132, 4, "a sound that is not there is unchanged by the normalization, and the detection cannot read it.",
       [expect(DONE, message("Voice detection failed: the audio cannot be read."))],
       params=[("audio", "'media/missing.wav'"), ("media_type", "'audio/wav'"), ("output", "'media/sound.pcm'")],
       backend=[NONE])

# 5: the normalization fails, or a step cannot run.
S.case(140, 5, "an output outside the granted directory fails the normalization before any detection.",
       [expect(DONE, message("Sound normalization failed: the output cannot be written."))],
       params=[("audio", "'media/sound.wav'"), ("media_type", "'audio/wav'"), ("output", "'elsewhere/sound.pcm'")],
       backend=[{"path": VAD, "status": 500, "body": {"error": "never asked"}}])
S.case(141, 5, "with no output the normalizer cannot start.",
       [expect(DONE, message("VoiceDetection could not run (error.execution)"))],
       params=[("audio", "'media/sound.wav'"), ("media_type", "'audio/wav'")])
S.case(142, 5, "an empty model rejects the detector's start after the normalization.",
       [expect(DONE, message("VoiceDetection could not run (error.execution)"))],
       params=WAV + [("model", "''")], backend=[{"path": VAD, "status": 500, "body": {"error": "never asked"}}])
