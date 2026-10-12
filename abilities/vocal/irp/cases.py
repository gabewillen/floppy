# The vocal's statements and cases (tests/irp/harness.py writes them).

import base64
import json

S = suite
INPUT = "bot.ability.vocal.speech.encoding.input"
OUTPUT = "bot.ability.vocal.speech.encoding.output"
FAILED = "bot.ability.failed"
READY = expect("bot.ability.offers", "_event.data.ability === 'vocal' && Array.isArray(_event.data.choices) && _event.data.choices.length === 0 && Object.keys(_event.data).length === 2")
SPEECH = "/v1/audio/speech"
PARAMS = [("model", "'tts-1'"), ("voice", "'alloy'"), ("output", "'media/reply.pcm'")]
PCM = "A" * 4800  # a tenth of a second of 24 kHz mono 16-bit PCM


def canonical(value):
    return json.dumps(value, separators=(",", ":"), sort_keys=True)


def say(text="Hello there."):
    return send(INPUT, js({"text": text}))


def encoded(audio="media/reply.pcm", media_type="audio/pcm", rate=24000, channels=1, duration=0.1):
    value = {"audio": audio, "media_type": media_type, "sample_rate_hz": rate, "channels": channels, "duration": duration}
    return expect(OUTPUT, f"JSON.stringify(_event.data) === {js(canonical(value))}")


def failed(text):
    return expect(FAILED, f"_event.data.message === {js(text)} && Object.keys(_event.data).length === 1")


S.statement(1, "On start, and on every later entry into `idle`, it offers nothing: it sends `bot.ability.offers` `{ability: \"vocal\", choices: []}` to its parent.")
S.statement(2, "`bot.ability.vocal.speech.encoding.input` `{text}` in `idle` is encoded by the speech service's `encode` with the `<param>`s `model`, `voice`, `output` and `media_type` (`audio/pcm` by default); with provider `null` (the default) that is the OpenAI-compatible `POST {base}/audio/speech`. The service's answer is sent to the parent as `bot.ability.vocal.speech.encoding.output` `{audio, media_type, sample_rate_hz, channels, duration}` (the audio as the `output` reference; raw PCM is 24 kHz mono, its duration measured from its bytes; a format the service cannot measure has `null`s), and it returns to `idle`.")
S.statement(3, "The `provider` `<param>` names the original's provider instead: `elevenlabs` is `POST {base}/text-to-speech/{voice}?output_format=…`, `gemini` is `POST {base}/interactions` (its answer's base64 audio part), each answered as above.")
S.statement(4, "When the service answers `bot.ability.failed` `{message}` (an error status, an empty answer, an `output` that cannot be written), it sends the parent `bot.ability.failed` with the service's message and returns to `idle`.")
S.statement(5, "When the service cannot run (a missing `voice`, rejecting its start with `error.execution`), it sends the parent `bot.ability.failed` `{message: \"The speech service could not run (<error event>).\"}` and returns to `idle`.")
S.statement(6, "An input that arrives while `applying` is dropped: only the input in hand is encoded and answered.")
S.statement(7, "Each input is a new encoding: after an answer or a failure, the next input in `idle` is encoded again.")

# 1: readiness.
S.case(100, 1, "the vocal offers nothing on start.", [READY], params=PARAMS)

# 2: the OpenAI-compatible encoding.
S.case(110, 2, "raw PCM comes back as its reference with its format and measured duration, then the vocal offers again.",
       [READY, say(), encoded(), READY],
       params=PARAMS, backend=[{"path": SPEECH, "body": PCM}])
S.case(111, 2, "an mp3 encoding has no measured format.",
       [READY, say(), encoded(audio="media/reply.mp3", media_type="audio/mpeg", rate=None, channels=None, duration=None), READY],
       params=[("model", "'tts-1'"), ("voice", "'alloy'"), ("output", "'media/reply.mp3'"), ("media_type", "'audio/mpeg'")],
       backend=[{"path": SPEECH, "body": "ID3 not really mp3"}])

# 3: the original's providers.
S.case(120, 3, "provider elevenlabs encodes through its text-to-speech API in the named voice.",
       [READY, say(), encoded(), READY],
       params=PARAMS + [("provider", "'elevenlabs'")],
       backend=[{"path": "/v1/text-to-speech/alloy?output_format=pcm_24000", "body": PCM}])
S.case(121, 3, "provider gemini encodes through the interactions API.",
       [READY, say(), encoded(), READY],
       params=[("provider", "'gemini'"), ("model", "'gemini-3.1-flash-tts-preview'"), ("voice", "'Kore'"), ("output", "'media/reply.pcm'")],
       backend=[{"path": "/v1/interactions", "body": {"steps": [{"type": "model_output", "content": [
           {"type": "audio", "data": base64.b64encode(PCM.encode()).decode(), "sample_rate": 24000, "channels": 1}]}]}}])

# 4: the service's failures.
S.case(130, 4, "an error status is passed on as the vocal's failure, then it offers again.",
       [READY, say(), failed("Speech encoding failed: the backend answered an error status."), READY],
       params=PARAMS, backend=[{"path": SPEECH, "status": 500, "body": {"error": {"message": "boom"}}}])
S.case(131, 4, "an empty answer is malformed.",
       [READY, say(), failed("Speech encoding failed: the backend's answer is malformed."), READY],
       params=PARAMS, backend=[{"path": SPEECH, "body": ""}])
S.case(132, 4, "an output outside the granted directory cannot be written.",
       [READY, say(), failed("Speech encoding failed: the output cannot be written."), READY],
       params=[("model", "'tts-1'"), ("voice", "'alloy'"), ("output", "'elsewhere/reply.pcm'")],
       backend=[{"path": SPEECH, "body": PCM}])

# 5: the service cannot run.
S.case(140, 5, "with no voice the service cannot start.",
       [READY, say(), failed("The speech service could not run (error.execution)."), READY],
       params=[("model", "'tts-1'"), ("output", "'media/reply.pcm'")],
       backend=[{"path": SPEECH, "body": PCM}])

# 6: inputs while applying.
S.case(150, 6, "a second input sent while the first is encoding is dropped: one answer, and nothing after it.",
       [READY, say("first"), say("second"), encoded(duration=0.1), READY, wait("1s")],
       params=PARAMS, backend=[{"path": SPEECH, "body": PCM}, {"path": SPEECH, "body": PCM * 2}],
       clock="wall")

# 7: one encoding per input.
S.case(160, 7, "after an answer the next input is encoded again.",
       [READY, say("first"), encoded(duration=0.1), READY, say("second"), encoded(duration=0.2), READY],
       params=PARAMS, backend=[{"path": SPEECH, "body": PCM}, {"path": SPEECH, "body": PCM * 2}])
S.case(161, 7, "after a failure the next input is encoded again.",
       [READY, say("first"), failed("Speech encoding failed: the backend answered an error status."), READY,
        say("second"), encoded(), READY],
       params=PARAMS, backend=[{"path": SPEECH, "status": 500, "body": {"error": "boom"}}, {"path": SPEECH, "body": PCM}])
