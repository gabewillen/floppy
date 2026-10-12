# The speech_decoding's statements and cases (tests/irp/harness.py writes them).

S = suite
DONE = "done.invoke.speech_decoding"
AUDIO = ("audio", "'media/speech.wav'")
WHISPER = [("provider", "null"), ("model", "'whisper-1'")]


def text(value):
    return f"_event.data.text === {js(value)} && Object.keys(_event.data).length === 1"


def message(value):
    return f"_event.data.message === {js(value)} && Object.keys(_event.data).length === 1"


S.statement(1, "By default it decodes through the original's gemini SpeechDecoder: the speech service's `decode` with provider `gemini` and model `gemini-3.5-flash`, `POST {base}/interactions`. The service's `bot.ability.hearing.speech.decoding.output` `{text}` ends it in `applied`, its done data (`done.invoke.speech_decoding`) exactly `{text}`: the answer's `output_text`, or its `model_output` steps' text, trimmed.")
S.statement(2, "Speech the backend heard none of decodes to empty words: done data `{text: \"\"}`, not a failure.")
S.statement(3, "The `provider` and `model` `<param>`s choose the backend: provider `null` is the OpenAI-compatible `POST {base}/audio/transcriptions`, whose answer's `text` becomes the done data's `text`.")
S.statement(4, "When the service answers `bot.ability.failed` `{message}` (an unreadable or empty audio file, an error status, a malformed answer), the done data is the original's FailureData, exactly `{message}` with the service's message.")
S.statement(5, "When the decoder cannot run (a missing `audio`, or a malformed `media_type`, rejects its start with `error.execution`), the done data is `{message: \"SpeechDecoding could not run (<error event>)\"}`.")

# 1: the gemini default.
S.case(100, 1, "the default gemini decoder's output_text comes back trimmed as the done data.",
       [expect(DONE, text("hello there"))],
       params=[AUDIO], backend=[{"path": "/v1/interactions", "body": {"output_text": "  hello there \n"}}])
S.case(101, 1, "the gemini answer's model_output steps' text is joined as the words.",
       [expect(DONE, text("hello\nthere"))],
       params=[AUDIO, ("media_type", "'audio/wav'")],
       backend=[{"path": "/v1/interactions", "body": {"steps": [
           {"type": "user_input", "content": [{"type": "text", "text": "ignored"}]},
           {"type": "model_output", "content": [{"type": "text", "text": " hello "}, {"type": "text", "text": "there"}]}]}}])

# 2: no speech heard.
S.case(110, 2, "a gemini model output with no text decodes to empty words.",
       [expect(DONE, text(""))],
       params=[AUDIO], backend=[{"path": "/v1/interactions", "body": {"steps": [{"type": "model_output", "content": []}]}}])
S.case(111, 2, "an OpenAI-compatible transcription of silence decodes to empty words.",
       [expect(DONE, text(""))],
       params=[AUDIO] + WHISPER, backend=[{"path": "/v1/audio/transcriptions", "body": {"text": "   "}}])

# 3: provider and model.
S.case(120, 3, "provider null decodes through /audio/transcriptions.",
       [expect(DONE, text("good morning"))],
       params=[AUDIO] + WHISPER, backend=[{"path": "/v1/audio/transcriptions", "body": {"text": " good morning "}}])
S.case(121, 3, "provider null never calls the gemini API (its answer there would be other words).",
       [expect(DONE, text("transcribed"))],
       params=[AUDIO] + WHISPER,
       backend=[{"path": "/v1/interactions", "body": {"output_text": "wrong backend"}},
                {"path": "/v1/audio/transcriptions", "body": {"text": "transcribed"}}])

# 4: the service's failures.
S.case(130, 4, "an error status is the service's failure message.",
       [expect(DONE, message("Speech decoding failed: the backend answered an error status."))],
       params=[AUDIO], backend=[{"path": "/v1/interactions", "status": 500, "body": {"error": {"message": "boom"}}}])
S.case(131, 4, "a malformed answer is the service's failure message.",
       [expect(DONE, message("Speech decoding failed: the backend's answer is malformed."))],
       params=[AUDIO] + WHISPER, backend=[{"path": "/v1/audio/transcriptions", "body": {"words": "no text field"}}])
S.case(132, 4, "an audio file that is not there is the service's failure message.",
       [expect(DONE, message("Speech decoding failed: the audio cannot be read."))],
       params=[("audio", "'media/missing.wav'")], backend=[])
S.case(133, 4, "an empty audio file is the service's failure message.",
       [expect(DONE, message("Speech decoding failed: the audio is empty."))],
       params=[("audio", "'media/empty.wav'")], backend=[])

# 5: the decoder cannot run.
S.case(140, 5, "with no audio the decoder cannot start.",
       [expect(DONE, message("SpeechDecoding could not run (error.execution)"))])
S.case(141, 5, "a malformed media_type rejects the decoder's start.",
       [expect(DONE, message("SpeechDecoding could not run (error.execution)"))],
       params=[AUDIO, ("media_type", "'not a media type'")])
