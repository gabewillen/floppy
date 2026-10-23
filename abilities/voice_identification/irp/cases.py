# The voice_identification's statements and cases (tests/irp/harness.py writes them).

import json

S = suite
# sound.raw: 1 s of 8 kHz mono 16-bit PCM, constant non-zero samples;
# first.raw and second.raw: 0.5 s each.
S.media({"sound.raw": pcm(1000, 1, 8000), "first.raw": pcm(1000, 0.5, 8000), "second.raw": pcm(1000, 0.5, 8000)})
DONE = "done.invoke.voice_identification"
EMBED = "/v1/voice/embed"
DEFAULT_MODEL = "voice-detect-wespeaker-resnet34"


def canonical(value):
    return json.dumps(value, separators=(",", ":"), sort_keys=True)


def seg(audio, start, end):
    return {"audio": audio, "media_type": "audio/pcm", "sample_rate_hz": 8000, "channels": 1,
            "start_seconds": start, "end_seconds": end}


TWO = [seg("media/first.raw", 0, 0.5), seg("media/second.raw", 0.5, 1)]
SEGMENTS = ("segments", canonical(TWO))
DIRECT = [("audio", "'media/sound.raw'"), ("sample_rate_hz", "8000"), ("channels", "1"),
          ("start_seconds", "0.25"), ("end_seconds", "2"), ("output", "'media/clip'")]


def embedding(values, model=DEFAULT_MODEL):
    return {"embedding": values, "model": model, "confidence": None}


def same(location, value):
    return f"JSON.stringify({location}) === {js(canonical(value))}"


def identified(segments, embeddings):
    return (f"Object.keys(_event.data).sort().join() === 'embeddings,segments' && "
            f"{same('_event.data.embeddings', embeddings)} && _event.data.segments.length === {len(segments)} && "
            + " && ".join(f"_event.data.segments[{i}].{key} === {js(value)}"
                          for i, item in enumerate(segments) for key, value in sorted(item.items())))


def message(value):
    return f"_event.data.message === {js(value)} && Object.keys(_event.data).length === 1"


def answers(*vectors):
    return [{"path": EMBED, "body": {"embedding": vector, "dim": len(vector)}} for vector in vectors]


S.statement(1, "Given the `segments` `<param>`, it clips nothing: the speech service's `identify` (one `POST {base}/voice/embed` per segment) answers `bot.ability.hearing.voice.identification.output` `{embeddings}`, and the done data (`done.invoke.voice_identification`) is exactly `{segments, embeddings}`: the segments as given and one embedding per segment, in segment order, each `{embedding, model, confidence: null}`.")
S.statement(2, "The `model` `<param>` names the embedding model (`voice-detect-wespeaker-resnet34` by default), and each embedding reports it.")
S.statement(3, "Without `segments` it identifies directly: `clipping` invokes the speech service's `clip` on `audio` (`sample_rate_hz`, `channels`) over `start_seconds`..`end_seconds` (the end clamped to the sound) into `output`, and the one clipped segment `{audio: output, media_type: \"audio/pcm\", sample_rate_hz, channels, start_seconds, end_seconds}` is what it identifies and returns as `segments`.")
S.statement(4, "When the clip answers `bot.ability.failed` `{message}` (a span that holds no audio, a sound that cannot be read), the done data is `{message: \"Voice identification opening segment conversion failed: <message>\"}` and nothing is identified.")
S.statement(5, "When the identification answers `bot.ability.failed` `{message}` (an error status, a malformed embedding), the done data is exactly `{message}`.")
S.statement(6, "When a step cannot run (segments that are not ascending `audio/pcm` voice segments, or no segments and no sound to clip, reject its start with `error.execution`), the done data is `{message: \"VoiceIdentification could not run (<error event>)\"}`.")

# 1: given segments.
S.case(100, 1, "two segments come back with one embedding each, in segment order.",
       [expect(DONE, identified(TWO, [embedding([0.1, 0.2]), embedding([0.3, 0.4])]))],
       params=[SEGMENTS], backend=answers([0.1, 0.2], [0.3, 0.4]))
S.case(101, 1, "one segment, one embedding.",
       [expect(DONE, identified(TWO[:1], [embedding([1, -1, 0.5])]))],
       params=[("segments", canonical(TWO[:1]))], backend=answers([1, -1, 0.5]))

# 2: the model.
S.case(110, 2, "a named model is reported in each embedding.",
       [expect(DONE, identified(TWO, [embedding([0.1], "custom-embedder"), embedding([0.2], "custom-embedder")]))],
       params=[SEGMENTS, ("model", "'custom-embedder'")], backend=answers([0.1], [0.2]))

# 3: direct identification.
S.case(120, 3, "the opening voice span is clipped (its end clamped to the sound) and identified.",
       [expect(DONE, identified([seg("media/clip", 0.25, 1)], [embedding([0.5, 0.6])]))],
       params=DIRECT, backend=answers([0.5, 0.6]))

# 4: the clip fails.
S.case(130, 4, "a span past the sound holds no audio.",
       [expect(DONE, message("Voice identification opening segment conversion failed: Voice segment clipping failed: the span holds no audio."))],
       params=[("audio", "'media/sound.raw'"), ("sample_rate_hz", "8000"), ("channels", "1"),
               ("start_seconds", "2"), ("end_seconds", "3"), ("output", "'media/clip'")],
       backend=[{"path": EMBED, "status": 500, "body": {"error": "never asked"}}])
S.case(131, 4, "a sound that is not there cannot be clipped.",
       [expect(DONE, message("Voice identification opening segment conversion failed: Voice segment clipping failed: the audio cannot be read."))],
       params=[("audio", "'media/missing.raw'"), ("sample_rate_hz", "8000"), ("channels", "1"),
               ("start_seconds", "0"), ("end_seconds", "1"), ("output", "'media/clip'")],
       backend=[{"path": EMBED, "status": 500, "body": {"error": "never asked"}}])

# 5: the identification fails.
S.case(140, 5, "an error status is the identification's failure message.",
       [expect(DONE, message("Voice identification failed: the backend answered an error status."))],
       params=[SEGMENTS], backend=[{"path": EMBED, "status": 500, "body": {"error": "boom"}}])
S.case(141, 5, "an empty embedding is malformed.",
       [expect(DONE, message("Voice identification failed: the backend's answer is malformed."))],
       params=[SEGMENTS], backend=answers([0.1], []))
S.case(142, 5, "a segment file that is not there fails the identification.",
       [expect(DONE, message("Voice identification failed: the audio cannot be read."))],
       params=[("segments", canonical([seg("media/missing.raw", 0, 0.5)]))], backend=answers([0.1]))

# 6: a step cannot run.
S.case(150, 6, "segments out of order reject the identifier's start.",
       [expect(DONE, message("VoiceIdentification could not run (error.execution)"))],
       params=[("segments", canonical(list(reversed(TWO))))], backend=answers([0.1]))
S.case(151, 6, "no segments and no sound reject the clipper's start.",
       [expect(DONE, message("VoiceIdentification could not run (error.execution)"))])
