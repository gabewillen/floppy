# The speech_decoding's statements and cases (tests/irp/harness.py writes them).

S = suite
# speech.wav: 0.1 s of 16 kHz mono 16-bit WAV, constant non-zero samples;
# empty.wav: no bytes.
S.media({"speech.wav": wav(1000), "empty.wav": b""})
INPUT = "bot.ability.hearing.speech.decoding.input"
OUTPUT = "bot.ability.hearing.speech.decoding.output"
FAILED = "bot.ability.failed"
CONFIG = ('provider', 'model')


def asked(params, event, cond):
    """A case that asks the long-lived child once: its params that are not
    the child's configuration (CONFIG) are the request's fields, and the
    child answers `event` of which `cond` holds."""
    return dict(steps=[request(INPUT, [param for param in params if param[0] not in CONFIG]), expect(event, answered(cond))],
                params=[param for param in params if param[0] in CONFIG])


AUDIO = ("audio", "'media/speech.wav'")
WHISPER = [("provider", "null"), ("model", "'whisper-1'")]
GEMINI = [("provider", "'gemini'"), ("model", "'gemini-3.5-flash'")]


def text(value):
    return f"_event.data.text === {js(value)} && Object.keys(_event.data).length === 1"


def message(value):
    return f"_event.data.message === {js(value)} && Object.keys(_event.data).length === 1"


S.statement(1, "By default it decodes through the OpenAI-compatible API: the speech service's `decode` with provider `null` and model `nemotron-speech-streaming-en-0.6b` (the lab's Nemotron), `POST {base}/audio/transcriptions`. The service's `bot.ability.hearing.speech.decoding.output` `{text}` is answered to its parent as `bot.ability.hearing.speech.decoding.output` exactly `{id, text}` (the request's `id`, the answer's `text`, trimmed). It is long-lived: each utterance is a request `bot.ability.hearing.speech.decoding.input` `{id, audio, media_type?}`.")
S.statement(2, "Speech the backend heard none of decodes to empty words: `{id, text: \"\"}`, not a failure.")
S.statement(3, "The `provider` and `model` `<param>`s choose the backend: provider `gemini` is Gemini's `POST {base}/interactions`, whose answer's `output_text`, or its `model_output` steps' text, trimmed, becomes the answer's `text`; provider `null` is the OpenAI-compatible `POST {base}/audio/transcriptions` and never calls Gemini's.")
S.statement(4, "When the service answers `bot.ability.failed` `{message}` (an unreadable or empty audio file, an error status, a malformed answer), it answers `bot.ability.failed` exactly `{id, message}` with the service's message.")
S.statement(5, "When the decoder cannot run (a malformed `media_type` rejects its start with `error.execution`), it answers `bot.ability.failed` `{id, message: \"SpeechDecoding could not run (<error event>)\"}`.")

# 1: the OpenAI-compatible default.
S.case(100, 1, "the default decoder's transcription text comes back trimmed as the done data.",
       **asked([AUDIO], OUTPUT, text("hello there")), backend=[{"path": "/v1/audio/transcriptions", "body": {"text": "  hello there \n"}}])

# 2: no speech heard.
S.case(110, 2, "a gemini model output with no text decodes to empty words.",
       **asked([AUDIO] + GEMINI, OUTPUT, text("")), backend=[{"path": "/v1/interactions", "body": {"steps": [{"type": "model_output", "content": []}]}}])
S.case(111, 2, "an OpenAI-compatible transcription of silence decodes to empty words.",
       **asked([AUDIO] + WHISPER, OUTPUT, text("")), backend=[{"path": "/v1/audio/transcriptions", "body": {"text": "   "}}])

# 3: provider and model.
S.case(101, 3, "the gemini answer's model_output steps' text is joined as the words.",
       **asked([AUDIO, ("media_type", "'audio/wav'")] + GEMINI, OUTPUT, text("hello\nthere")), backend=[{"path": "/v1/interactions", "body": {"steps": [
           {"type": "user_input", "content": [{"type": "text", "text": "ignored"}]},
           {"type": "model_output", "content": [{"type": "text", "text": " hello "}, {"type": "text", "text": "there"}]}]}}])
S.case(102, 3, "the gemini answer's output_text comes back trimmed as the words.",
       **asked([AUDIO] + GEMINI, OUTPUT, text("hello there")), backend=[{"path": "/v1/interactions", "body": {"output_text": "  hello there \n"}}])
S.case(120, 3, "provider null decodes through /audio/transcriptions.",
       **asked([AUDIO] + WHISPER, OUTPUT, text("good morning")), backend=[{"path": "/v1/audio/transcriptions", "body": {"text": " good morning "}}])
S.case(121, 3, "provider null never calls the gemini API (its answer there would be other words).",
       **asked([AUDIO] + WHISPER, OUTPUT, text("transcribed")), backend=[{"path": "/v1/interactions", "body": {"output_text": "wrong backend"}},
                {"path": "/v1/audio/transcriptions", "body": {"text": "transcribed"}}])

# 4: the service's failures.
S.case(130, 4, "an error status is the service's failure message.",
       **asked([AUDIO], FAILED, message("Speech decoding failed: the backend answered an error status.")), backend=[{"path": "/v1/audio/transcriptions", "status": 500, "body": {"error": {"message": "boom"}}}])
S.case(131, 4, "a malformed answer is the service's failure message.",
       **asked([AUDIO] + WHISPER, FAILED, message("Speech decoding failed: the backend's answer is malformed.")), backend=[{"path": "/v1/audio/transcriptions", "body": {"words": "no text field"}}])
S.case(132, 4, "an audio file that is not there is the service's failure message.",
       **asked([("audio", "'media/missing.wav'")], FAILED, message("Speech decoding failed: the audio cannot be read.")), backend=[])
S.case(133, 4, "an empty audio file is the service's failure message.",
       **asked([("audio", "'media/empty.wav'")], FAILED, message("Speech decoding failed: the audio is empty.")), backend=[])

# 5: the decoder cannot run.
S.case(141, 5, "a malformed media_type rejects the decoder's start.",
       **asked([AUDIO, ("media_type", "'not a media type'")], FAILED, message("SpeechDecoding could not run (error.execution)")))
