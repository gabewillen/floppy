# The voice_detection's statements and cases (tests/irp/harness.py writes them).

S = suite
# sound.wav and sound.raw: 1 s of 8 kHz mono 16-bit PCM, constant non-zero
# samples (WAV container, raw); bad.wav: no WAV container.
S.media({"sound.wav": wav(1000, 1, 8000), "sound.raw": pcm(1000, 1, 8000), "bad.wav": b"not a WAV container\n"})
INPUT = "bot.ability.hearing.voice.detection.input"
OUTPUT = "bot.ability.hearing.voice.detection.output"
FAILED = "bot.ability.failed"
VAD = "/v1/vad"


def sound(audio, media_type, rate, channels):
    return (f"JSON.stringify(_event.data.sound) === {js(json_text({'audio': audio, 'media_type': media_type, 'sample_rate_hz': rate, 'channels': channels}))}")


def json_text(value):
    import json
    return json.dumps(value, separators=(",", ":"), sort_keys=True)


def segments(value):
    return f"JSON.stringify(_event.data.segments) === {js(json_text(value))}"


def only(*keys):
    return f"Object.keys(_event.data).sort().join() === {js(','.join(sorted(keys)))}"


def message(value):
    return f"_event.data.message === {js(value)} && _event.data.id === 'r1' && {only('id', 'message')}"


def detected(sound_cond, spans):
    return f"_event.data.id === 'r1' && {only('id', 'segments', 'sound')} && {sound_cond} && {segments(spans)}"


def detect(fields, request_id="r1"):
    """The steps of one request: its fields (name, expr) sent as the input."""
    return [request(INPUT, fields, request_id)]


def answered(request_id):
    return f"_event.data.id === {js(request_id)} && {only('id', 'segments', 'sound')}"


WAV = [("audio", "'media/sound.wav'"), ("media_type", "'audio/wav'"), ("output", "'media/sound.pcm'")]
RAW = [("audio", "'media/sound.raw'"), ("sample_rate_hz", "8000"), ("channels", "1"), ("output", "'media/sound.pcm'")]
NONE = {"path": VAD, "body": {"segments": []}}

S.statement(1, "It serves each `bot.ability.hearing.voice.detection.input` `{id, audio, media_type?, sample_rate_hz?, channels?, output?}` by normalizing the sound, then detecting its voice: a WAV container's signed 16-bit PCM is written to `output` and the sound becomes `{audio: output, media_type: \"audio/pcm\", sample_rate_hz, channels}` (the container's own format); the speech service's `detect` (`POST {base}/vad`, model `silero-vad` by default) answers `bot.ability.hearing.voice.detection.output` `{segments}`, and the detection answers its parent with `bot.ability.hearing.voice.detection.output` exactly `{id, sound, segments}`, each segment `{start_seconds, end_seconds, confidence}` (the answer's `confidence`, `probability` or `score`, else `null`) in ascending start order, clamped to the clip; the clip is sent between half a second of silence (a cold VAD misses voice that starts at the first sample), and the answered spans are shifted back onto it.")
S.statement(2, "Raw, `audio/l16` or unnamed audio with a rate and channels normalizes to `audio/pcm` in place (the same `audio`); any other media type, or a WAV container that cannot be read, leaves the sound unchanged, and the detection still runs on it.")
S.statement(3, "A clip with no voice (no spans answered, or only spans outside the clip) is not a failure: its `segments` are `[]`.")
S.statement(4, "When the detection answers `bot.ability.failed` `{message}` (an error status, a malformed answer, audio that cannot be read), the detection answers `bot.ability.failed` exactly `{id, message}`, with no sound.")
S.statement(5, "When the normalization fails (`bot.ability.failed`, such as an `output` that cannot be written) it answers that `{id, message}` and nothing is detected; when a step cannot run (a missing `output`, or a malformed `model`, rejects its start with `error.execution`), it answers `{id, message: \"VoiceDetection could not run (<error event>)\"}`.")
S.statement(6, "It is long-lived (mosfet.py's Interpretation holds its VoiceDetection): it answers request after request, one at a time, and a request that arrives while one is in flight replaces it, so only the newer one is answered.")

# 1: WAV normalized, spans folded.
S.case(100, 1, "a WAV sound is normalized to PCM and its spans come back sorted, clamped and with their confidence.",
       [detect(WAV), expect(OUTPUT, detected(sound("media/sound.pcm", "audio/pcm", 8000, 1), [
           {"start_seconds": 0.1, "end_seconds": 0.3, "confidence": 0.9},
           {"start_seconds": 0.5, "end_seconds": 1, "confidence": None}]))],
       backend=[{"path": VAD, "body": {"segments": [{"start": 1.0, "end": 2.5}, {"start": 0.6, "end": 0.8, "probability": 0.9}]}}])
S.case(101, 1, "spans with start_seconds/end_seconds and a score are read too.",
       [detect(WAV), expect(OUTPUT, detected(sound("media/sound.pcm", "audio/pcm", 8000, 1), [
           {"start_seconds": 0.25, "end_seconds": 0.75, "confidence": 0.5}]))],
       backend=[{"path": VAD, "body": {"segments": [{"start_seconds": 0.75, "end_seconds": 1.25, "score": 0.5}]}}])

# 2: other sounds.
S.case(110, 2, "unnamed raw audio with a rate and channels is audio/pcm in place.",
       [detect(RAW), expect(OUTPUT, detected(sound("media/sound.raw", "audio/pcm", 8000, 1), [
           {"start_seconds": 0, "end_seconds": 0.5, "confidence": None}]))], backend=[{"path": VAD, "body": {"segments": [{"start": 0.5, "end": 1.0}]}}])
S.case(111, 2, "audio/l16 with a rate and channels is audio/pcm in place.",
       [detect(RAW + [("media_type", "'audio/l16'")]), expect(OUTPUT, detected(sound("media/sound.raw", "audio/pcm", 8000, 1), []))], backend=[NONE])
S.case(112, 2, "another media type is left unchanged and still detected.",
       [detect([("audio", "'media/sound.raw'"), ("media_type", "'audio/mpeg'"), ("output", "'media/sound.pcm'")]), expect(OUTPUT, detected(sound("media/sound.raw", "audio/mpeg", None, None), []))],
       backend=[NONE])
S.case(113, 2, "a WAV container that cannot be read is left unchanged.",
       [detect([("audio", "'media/bad.wav'"), ("media_type", "'audio/wav'"), ("output", "'media/sound.pcm'")]), expect(OUTPUT, detected(sound("media/bad.wav", "audio/wav", None, None), []))],
       backend=[NONE])

# 3: no voice.
S.case(120, 3, "no spans answered is no voice.",
       [detect(WAV), expect(OUTPUT, detected(sound("media/sound.pcm", "audio/pcm", 8000, 1), []))], backend=[NONE])
S.case(121, 3, "spans outside the clip are dropped.",
       [detect(WAV), expect(OUTPUT, detected(sound("media/sound.pcm", "audio/pcm", 8000, 1), []))], backend=[{"path": VAD, "body": {"segments": [{"start": 2.0, "end": 2.5}]}}])

# 4: the detection's failures.
S.case(130, 4, "an error status is the detection's failure message, with no sound.",
       [detect(WAV), expect(FAILED, message("Voice detection failed: the backend answered an error status."))], backend=[{"path": VAD, "status": 500, "body": {"error": "boom"}}])
S.case(131, 4, "an answer with no segments is malformed.",
       [detect(WAV), expect(FAILED, message("Voice detection failed: the backend's answer is malformed."))], backend=[{"path": VAD, "body": {"spans": []}}])
S.case(132, 4, "a sound that is not there is unchanged by the normalization, and the detection cannot read it.",
       [detect([("audio", "'media/missing.wav'"), ("media_type", "'audio/wav'"), ("output", "'media/sound.pcm'")]), expect(FAILED, message("Voice detection failed: the audio cannot be read."))],
       backend=[NONE])

# 5: the normalization fails, or a step cannot run.
S.case(140, 5, "an output outside the granted directory fails the normalization before any detection.",
       [detect([("audio", "'media/sound.wav'"), ("media_type", "'audio/wav'"), ("output", "'elsewhere/sound.pcm'")]), expect(FAILED, message("Sound normalization failed: the output cannot be written."))],
       backend=[{"path": VAD, "status": 500, "body": {"error": "never asked"}}])
S.case(141, 5, "with no output the normalizer cannot start.",
       [detect([("audio", "'media/sound.wav'"), ("media_type", "'audio/wav'")]), expect(FAILED, message("VoiceDetection could not run (error.execution)"))])
S.case(142, 5, "an empty model rejects the detector's start after the normalization.",
       [detect(WAV), expect(FAILED, message("VoiceDetection could not run (error.execution)"))],
       params=[("model", "''")], backend=[{"path": VAD, "status": 500, "body": {"error": "never asked"}}])

# 6: long-lived.
S.case(150, 6, "two requests in turn are each answered.",
       [detect(WAV, "r1"), expect(OUTPUT, answered("r1")), detect(RAW, "r2"), expect(OUTPUT, answered("r2"))],
       backend=[NONE])
S.case(151, 6, "a request that arrives while one is in flight replaces it.",
       [detect(WAV, "r1") + detect(RAW, "r2"), expect(OUTPUT, answered("r2"))],
       backend=[NONE])
