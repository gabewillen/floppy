# The vocal's statements and cases (tests/irp/harness.py writes them).

import base64
import json

S = suite
INPUT = "bot.ability.vocal.speech.encoding.input"
OUTPUT = "bot.ability.vocal.speech.encoding.output"
CHUNK = "bot.ability.vocal.speech.encoding.chunk"
FAILED = "bot.ability.failed"
READY = expect("bot.ability.ready")
SPEECH = "/v1/audio/speech"
PARAMS = [("model", "'tts-1'"), ("voice", "'alloy'"), ("output_prefix", "'media/reply-'")]
PCM = "A" * 4800  # a tenth of a second of 24 kHz mono 16-bit PCM
# An eighth of a second of 12336 Hz mono 16-bit PCM in a WAV container,
# every byte ASCII so the fake backend answers it as text.
WAV = wav(0x4141, 0.125, 12336).decode("latin-1")


def canonical(value):
    return json.dumps(value, separators=(",", ":"), sort_keys=True)


def say(text="Hello there."):
    """An utterance of one part, its words `text`."""
    return send(INPUT, js({"parts": [{"words": text}]}))


def reply(*parts):
    return send(INPUT, js({"parts": list(parts)}))


def encoded(audio="media/reply-1.pcm", media_type="audio/pcm", rate=24000, channels=1, duration=0.1):
    value = {"audio": audio, "media_type": media_type, "sample_rate_hz": rate, "channels": channels, "duration": duration}
    return expect(OUTPUT, f"JSON.stringify(_event.data) === {js(canonical(value))}")


def chunk(sequence, duration=0.1, audio="media/reply-1.{}.pcm", media_type="audio/pcm", rate=24000, channels=1):
    """The service's chunk `sequence`, sent on as it is."""
    value = {"audio": audio.format(sequence), "channels": channels, "duration": duration,
             "media_type": media_type, "sample_rate_hz": rate, "sequence": sequence}
    return expect(CHUNK, f"JSON.stringify(_event.data) === {js(canonical(value))}")


def failed(text):
    return expect(FAILED, f"_event.data.message === {js(text)} && Object.keys(_event.data).length === 1")


S.statement(1, "On start, and only then, it sends `bot.ability.ready` to its parent (decision D242), with no data: it offers nothing, and a later entry into `idle` sends nothing more.")
S.statement(2, "`bot.ability.vocal.speech.encoding.input` `{parts}` (1 to 64 parts `{expression?, words?}`) in `idle` is encoded by the speech service's `encode` with the `<param>`s `parts`, `model`, `voice`, `output` (the `<param>` `output_prefix`, the utterance's number counted from 1, and `.` and the media subtype) and `media_type` (`audio/pcm` by default); with provider `null` (the default) that is the OpenAI-compatible `POST {base}/audio/speech`. The service's answer is sent to the parent as `bot.ability.vocal.speech.encoding.output` `{audio, media_type, sample_rate_hz, channels, duration}` (the audio as the `output` reference; raw PCM is 24 kHz mono, its duration measured from its bytes; a format the service cannot measure has `null`s), and it returns to `idle`.")
S.statement(3, "The `provider` `<param>` names another provider instead: `elevenlabs` is `POST {base}/text-to-speech/{voice}/stream?output_format=…`, `gemini` is `POST {base}/interactions` (its answer's base64 audio part), each answered as above.")
S.statement(4, "When the service answers `bot.ability.failed` `{message}` (an error status, an empty answer, an `output` that cannot be written), it sends the parent `bot.ability.failed` with the service's message and returns to `idle`.")
S.statement(5, "When the service cannot run (a missing `voice`, rejecting its start with `error.execution`), it sends the parent `bot.ability.failed` `{message: \"The speech service could not run (<error event>).\"}` and returns to `idle`.")
S.statement(6, "An input that arrives while `applying` is dropped: only the input in hand is encoded and answered.")
S.statement(7, "Each input is a new encoding: after an answer or a failure, the next input in `idle` is encoded again.")
S.statement(8, "The service says the parts (decision D178): walking them in order, a part with a pause opens a new stretch, each stretch whose words are not blank is one backend request saying its words joined as written, one at a time in order, and no other cue (a sound, an emotion) is applied. With a pause the answers are joined as 16-bit PCM with each pause's silence at its place (0.4 s short, 1 s long), written raw for `audio/pcm` or in a WAV container for `audio/wav`; a pause with another `media_type` rejects the service's start, answered as in statement 5.")
S.statement(10, "It is long-lived: `bot.ability.processing.cancel` `{}` while `applying` cancels the utterance in hand (the service call with it) and answers nothing; in `idle` it is dropped. The cancelled call's late events are not the next utterance's, which is encoded and answered as any.")
S.statement(9, "The service streams its encoding (decision D203): each `bot.ability.vocal.speech.encoding.chunk` `{audio, sequence, media_type, sample_rate_hz, channels, duration}` it returns while `applying` is sent to the parent at once, as it arrives, before the output: its audio is chunk `sequence`'s own file (the `output` reference with `.<sequence>` before its extension), a streamed answer is cut into 4800 bytes (a tenth of a second of 24 kHz mono PCM) first and each chunk after it half again as long, in whole twentieths of a second, up to 24000 bytes (half a second; decision D280), Gemini's answer is one chunk per stretch, and a pause is a chunk of its silence.")

# 1: readiness.
S.case(100, 1, "the vocal is ready on start.", [READY], params=PARAMS)

# 2: the OpenAI-compatible encoding.
S.case(110, 2, "raw PCM comes back as its reference with its format and measured duration, and the vocal is not ready again.",
       [READY, say(), chunk(0), encoded(), wait("1s")],
       params=PARAMS, backend=[{"path": SPEECH, "body": PCM}])
S.case(111, 2, "an mp3 encoding has no measured format.",
       [READY, say(), chunk(0, None, "media/reply-1.{}.mpeg", "audio/mpeg", None, None),
        encoded(audio="media/reply-1.mpeg", media_type="audio/mpeg", rate=None, channels=None, duration=None)],
       params=[("model", "'tts-1'"), ("voice", "'alloy'"), ("output_prefix", "'media/reply-'"), ("media_type", "'audio/mpeg'")],
       backend=[{"path": SPEECH, "body": "ID3 not really mp3"}])

# 3: the named providers.
S.case(120, 3, "provider elevenlabs encodes through its text-to-speech API in the named voice.",
       [READY, say(), chunk(0), encoded()],
       params=PARAMS + [("provider", "'elevenlabs'")],
       backend=[{"path": "/v1/text-to-speech/alloy/stream?output_format=pcm_24000", "body": PCM}])
S.case(121, 3, "provider gemini encodes through the interactions API.",
       [READY, say(), chunk(0), encoded()],
       params=[("provider", "'gemini'"), ("model", "'gemini-3.1-flash-tts-preview'"), ("voice", "'Kore'"), ("output_prefix", "'media/reply-'")],
       backend=[{"path": "/v1/interactions", "body": {"steps": [{"type": "model_output", "content": [
           {"type": "audio", "data": base64.b64encode(PCM.encode()).decode(), "sample_rate": 24000, "channels": 1}]}]}}])

# 4: the service's failures.
S.case(130, 4, "an error status is passed on as the vocal's failure, and it is idle again.",
       [READY, say(), failed("Speech encoding failed: the backend answered an error status.")],
       params=PARAMS, backend=[{"path": SPEECH, "status": 500, "body": {"error": {"message": "boom"}}}])
S.case(131, 4, "an empty answer is malformed.",
       [READY, say(), failed("Speech encoding failed: the backend's answer is malformed.")],
       params=PARAMS, backend=[{"path": SPEECH, "body": ""}])
S.case(132, 4, "an output outside the granted directory cannot be written.",
       [READY, say(), failed("Speech encoding failed: the output cannot be written.")],
       params=[("model", "'tts-1'"), ("voice", "'alloy'"), ("output_prefix", "'elsewhere/reply-'")],
       backend=[{"path": SPEECH, "body": PCM}])

# 5: the service cannot run.
S.case(140, 5, "with no voice the service cannot start.",
       [READY, say(), failed("The speech service could not run (error.execution).")],
       params=[("model", "'tts-1'"), ("output_prefix", "'media/reply-'")],
       backend=[{"path": SPEECH, "body": PCM}])

# 6: inputs while applying.
S.case(150, 6, "a second input sent while the first is encoding is dropped: one answer, and nothing after it.",
       [READY, say("first"), say("second"), chunk(0), encoded(duration=0.1), wait("1s")],
       params=PARAMS, backend=[{"path": SPEECH, "body": PCM}, {"path": SPEECH, "body": PCM * 2}],
       clock="wall")

# 7: one encoding per input.
S.case(160, 7, "after an answer the next input is encoded again.",
       [READY, say("first"), chunk(0), encoded(duration=0.1), say("second"), chunk(0, 0.1, "media/reply-2.{}.pcm"), chunk(1, 0.1, "media/reply-2.{}.pcm"),
        encoded("media/reply-2.pcm", duration=0.2)],
       params=PARAMS, backend=[{"path": SPEECH, "body": PCM}, {"path": SPEECH, "body": PCM * 2}])
S.case(161, 7, "after a failure the next input is encoded again.",
       [READY, say("first"), failed("Speech encoding failed: the backend answered an error status."),
        say("second"), chunk(0, audio="media/reply-2.{}.pcm"), encoded("media/reply-2.pcm")],
       params=PARAMS, backend=[{"path": SPEECH, "status": 500, "body": {"error": "boom"}}, {"path": SPEECH, "body": PCM}])

# 8: parts, stretches and pauses.
S.case(170, 8, "a short pause splits the reply into two requests, joined with 0.4 s of silence between them (0.1 + 0.4 + 0.2 s).",
       [READY, reply({"words": "One,"}, {"expression": {"pause": "short"}, "words": " two."}),
        chunk(0), chunk(1, 0.4), chunk(2, 0.2), encoded(duration=0.7)],
       params=PARAMS, backend=[{"path": SPEECH, "body": PCM}, {"path": SPEECH, "body": PCM * 2}])
S.case(171, 8, "a long pause that opens the reply is 1 s of silence before its one request's speech.",
       [READY, reply({"expression": {"pause": "long"}}, {"words": "Hello."}), chunk(0, 1), chunk(1), encoded(duration=1.1)],
       params=PARAMS, backend=[{"path": SPEECH, "body": PCM}, {"path": SPEECH, "body": PCM * 2}])
S.case(172, 8, "a closing pause with no words after it sends no request of its own: one request, then 0.4 s of silence.",
       [READY, reply({"words": "Hello."}, {"expression": {"pause": "short"}}), chunk(0), chunk(1, 0.4), encoded(duration=0.5)],
       params=PARAMS, backend=[{"path": SPEECH, "body": PCM}, {"path": SPEECH, "body": PCM * 2}])
S.case(173, 8, "parts without a pause are one request, a sound and other cues applying nothing.",
       [READY, reply({"expression": {"sound": "laugh"}}, {"expression": {"emotion": "happy"}, "words": "Ha,"}, {"words": " yes."}),
        chunk(0), encoded(duration=0.1)],
       params=PARAMS, backend=[{"path": SPEECH, "body": PCM}, {"path": SPEECH, "body": PCM * 2}])
S.case(174, 8, "a pause with audio/mpeg cannot be encoded: the service rejects its start.",
       [READY, reply({"words": "One,"}, {"expression": {"pause": "short"}, "words": " two."}),
        failed("The speech service could not run (error.execution).")],
       params=[("model", "'tts-1'"), ("voice", "'alloy'"), ("output_prefix", "'media/reply-'"), ("media_type", "'audio/mpeg'")],
       backend=[{"path": SPEECH, "body": "ID3 not really mp3"}])

# 9: the streamed encoding's chunks.
S.case(180, 9, "a streamed answer of 1.25 s is sent on as six chunks, 0.1 s and each half again as long (in whole twentieths of a second), and the rest, before its output.",
       [READY, say(), chunk(0, 0.1), chunk(1, 0.15), chunk(2, 0.2), chunk(3, 0.3), chunk(4, 0.45), chunk(5, 0.05), encoded(duration=1.25)],
       params=PARAMS, backend=[{"path": SPEECH, "body": "A" * 60000}])
S.case(181, 9, "a WAV answer's chunks are WAV files of their own, named after the output.",
       [READY, say(), chunk(0, 0.125, "media/reply-1.{}.wav", "audio/wav", 12336, 1),
        encoded(audio="media/reply-1.wav", media_type="audio/wav", rate=12336, channels=1, duration=0.125)],
       params=[("model", "'tts-1'"), ("voice", "'alloy'"), ("output_prefix", "'media/reply-'"), ("media_type", "'audio/wav'")],
       backend=[{"path": SPEECH, "body": WAV}])

# 10: a cancelled utterance.
CANCEL = "bot.ability.processing.cancel"
S.case(190, 10, "a cancelled utterance is not answered, and the next one is encoded and answered.",
       [READY, say("first"), send(CANCEL, js({})), wait("1s"),
        say("second"), chunk(0, audio="media/reply-2.{}.pcm"), encoded("media/reply-2.pcm")],
       params=PARAMS, backend=[{"path": SPEECH, "body": PCM}, {"path": SPEECH, "body": PCM}],
       clock="wall")
S.case(191, 10, "a cancel in idle is dropped.",
       [READY, send(CANCEL, js({})), say("first"), chunk(0), encoded()],
       params=PARAMS, backend=[{"path": SPEECH, "body": PCM}])
