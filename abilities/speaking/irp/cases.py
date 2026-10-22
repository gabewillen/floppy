# The speaking's statements and cases (tests/irp/harness.py writes them).

S = suite
INPUT = "bot.ability.speaking.input"
OUTPUT = "bot.ability.speaking.output"
FAILED = "bot.ability.failed"
EFFERENCE = "bot.ability.speaking.efference"
SOUND = "environment.sound"
APPEND = "bot.ability.conversation.append"
READY = expect("bot.ability.offers", "_event.data.ability === 'speaking' && _event.data.choices.length === 0")

# The Gemini Interactions API's answer: 4800 bytes of 24 kHz mono 16-bit
# PCM (0.1 s).
PCM = {"path": "/interactions", "status": 200,
       "body": {"steps": [{"type": "model_output", "content": [{"type": "audio", "data": "A" * 6400}]}]}}
# Synthesis under a loaded host can outlast the original's 5 s bound: cases
# that reach the backend wait longer (statement 7 tests the bound itself).
PATIENT = [("encoding_timeout_ms", "600000")]
ERROR = {"path": "/interactions", "status": 500, "body": {"error": {"message": "boom"}}}
# The OpenAI-compatible speech API's answer: MP3 bytes, no rate or duration.
MP3 = {"path": "/audio/speech", "status": 200, "body": "ID3-mp3"}
OPENAI_MP3 = [("encoding_timeout_ms", "600000"), ("provider", "null"), ("model", "'tts-1'"), ("voice", "'alloy'"), ("media_type", "'audio/mpeg'")]


def say(text):
    return send(INPUT, js({"text": text}))


def efference(duration="0.1"):
    return expect(EFFERENCE, "_event.data.mouth === 'speaking' && _event.data.duration === " + duration +
                  " && _event.data.media_type === 'audio/pcm' && _event.data.sample_rate_hz === 24000 && _event.data.channels === 1")


def sound(n, media="pcm", media_type="audio/pcm"):
    return expect(SOUND, f"_event.data.audio === 'media/speaking-{n}.{media}' && _event.data.media_type === '{media_type}'"
                  " && _event.data.sample_rate_hz === 24000 && _event.data.channels === 1 && _event.data.source === 'speaking'")


def appended(text, n):
    return expect(APPEND, "_event.data.message.direction === 'outbound' && _event.data.message.content === " + js(text) +
                  " && _event.data.message.content_type === 'text/plain' && _event.data.message.sequence === 0"
                  " && _event.data.message.source_ids.length === 0 && _event.data.message.target_ids.length === 0"
                  f" && _event.data.message.provenance.id === 'speaking-{n}'"
                  " && _event.data.message.provenance.event === 'bot.ability.conversation.append'")


def output(text, media_type="audio/pcm"):
    return expect(OUTPUT, "_event.data.text === " + js(text) + f" && _event.data.media_type === '{media_type}'"
                  " && _event.data.sample_rate_hz === 24000 && _event.data.channels === 1")


def spoken(text, n):
    return [efference(), sound(n), appended(text, n), output(text), READY]


S.statement(1, "Every entry into `idle` (its start, and the end of each utterance, spoken or failed) sends `bot.ability.offers` `{ability: \"speaking\", choices: []}` to its parent: it offers nothing.")
S.statement(2, "`bot.ability.speaking.input` `{text}` is encoded by the vocal child with the speaking's `provider`, `model`, `voice` and `media_type` (by default the original's gemini encoder, `audio/pcm`), written to `output_prefix` + the utterance's number + `.` + the media subtype (`media/speaking-<n>.pcm`), the text stripped first.")
S.statement(3, "`playing` sends the efference copy `bot.ability.speaking.efference` `{mouth: \"speaking\", duration, media_type, sample_rate_hz, channels}` before the sound, `environment.sound` `{audio, media_type, sample_rate_hz, channels, source: \"speaking\"}` (its source the copy's mouth, so the listening can tell its own voice apart), to its parent; with no measured duration no copy is sent, and a rate or channel count the encoder leaves null is the speaking's own (24000 Hz mono).")
S.statement(4, "`recording` sends `bot.ability.conversation.append` `{message}` to its parent, the outbound message `<conversation:message_for_turn>` builds (content the text, content_type `text/plain`, sequence 0, provenance id `speaking-<n>` and event `bot.ability.conversation.append`), then answers `bot.ability.speaking.output` `{text, media_type, sample_rate_hz, channels}` and returns to `idle`.")
S.statement(5, "A blank text (after stripping) is refused before any encoding: `bot.ability.failed` `{message: \"Speaking requires non-blank text.\"}`, and back to `idle`; an empty text never arrives: the input's schema refuses it at the send, raising `error.execution` in the sender.")
S.statement(6, "A vocal failure (its `bot.ability.failed`) answers the parent with `bot.ability.failed` `{message}` carrying the vocal's message, and back to `idle`, where the next input is spoken.")
S.statement(7, "Encoding is bounded by `encoding_timeout_ms` (30 s by default, decision D98; the original's 5 s is shorter than Gemini's unstreamed synthesis of a sentence): its timer answers `bot.ability.failed` `{message: \"Speech encoding timed out.\"}` and returns to `idle`.")
S.statement(8, "Input that arrives outside `idle` is held (`<event:defer/>`) and taken, in order, on the return to `idle`.")

S.case(100, 1, "the speaking offers nothing on start.", [READY])
S.case(101, 1, "the speaking offers again after an utterance.",
       [READY, say("hi")] + spoken("hi", 1), params=PATIENT, backend=[PCM])
S.case(110, 2, "the stripped text is encoded by the gemini encoder and written to media/speaking-1.pcm.",
       [READY, say("  hello there  "), efference(), sound(1), appended("hello there", 1), output("hello there"), READY],
       params=PATIENT, backend=[PCM])
S.case(111, 2, "the speaking's provider, model, voice and media_type params reach the encoder: the OpenAI-compatible API writes media/speaking-1.mpeg.",
       [READY, say("hi"), sound(1, "mpeg", "audio/mpeg"), appended("hi", 1), output("hi", "audio/mpeg"), READY],
       params=OPENAI_MP3, backend=[MP3])
S.case(120, 3, "the efference copy, with the measured duration, leaves before the sound.",
       [READY, say("hi"), efference("0.1"), sound(1)],
       ignore=[APPEND, OUTPUT, "bot.ability.offers"], params=PATIENT, backend=[PCM])
S.case(121, 3, "with no measured duration no copy is sent, and the null rate and channels are the speaking's own.",
       [READY, say("hi"), sound(1, "mpeg", "audio/mpeg"), appended("hi", 1), output("hi", "audio/mpeg")],
       ignore=["bot.ability.offers"], params=OPENAI_MP3, backend=[MP3])
S.case(130, 4, "the words are appended to the conversation as an outbound message, then answered.",
       [READY, say("good morning"), efference(), sound(1), appended("good morning", 1), output("good morning"), READY],
       params=PATIENT, backend=[PCM])
S.case(131, 4, "each utterance's provenance id is its number.",
       [READY, say("one")] + spoken("one", 1) + [say("two")] + spoken("two", 2), params=PATIENT, backend=[PCM])
S.case(140, 5, "a blank text is refused, and the speaking is idle again.",
       [READY, say("   "), expect(FAILED, "_event.data.message === 'Speaking requires non-blank text.'"), READY])
S.case(141, 5, "an empty text is refused by the input's schema at the send; the next text is the first utterance.",
       [READY, say(""), expect("error.execution"), say("hi")] + spoken("hi", 1),
       params=PATIENT, backend=[PCM])
S.case(150, 6, "an encoder error status is the vocal's failure, answered as the speaking's.",
       [READY, say("hi"), expect(FAILED, "_event.data.message === 'Speech encoding failed: the backend answered an error status.'"), READY],
       params=PATIENT, backend=[ERROR])
S.case(151, 6, "after a vocal failure the next input is spoken.",
       [READY, say("hi"), expect(FAILED, "typeof _event.data.message === 'string'"), READY,
        say("again")] + spoken("again", 2),
       params=PATIENT, backend=[ERROR, PCM])
S.case(160, 7, "an encoding that outlasts encoding_timeout_ms times out.",
       [READY, say("hi"), expect(FAILED, "_event.data.message === 'Speech encoding timed out.'"), READY],
       params=[("encoding_timeout_ms", "1")], backend=[PCM], clock="wall")
S.case(170, 8, "a second input sent during the first utterance is spoken after it, in order.",
       [READY, say("first"), say("second")] + spoken("first", 1) + spoken("second", 2),
       params=PATIENT, backend=[PCM])
