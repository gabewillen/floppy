# The speaking's statements and cases (tests/irp/harness.py writes them).

import re

S = suite
# An input the harness sends with an id (its sendid, which the speaking
# keeps as the utterance's id): the harness writes no id attribute itself,
# so the target carries it and the written document moves it into place.
_document = S.document
SENT_AS = re.compile(r'<send event="([^"]+)" target="(#_[A-Za-z_]+)\|([^"]+)">(<content expr=[^>]*/>)</send>')


def _with_send_ids(case):
    """Each send whose target names an id goes through the harness's one
    `<send id>` for that id (an id is declared once), raised by a self-send
    harness.send.<id>; every copy carries the first copy's data."""
    text, sends = _document(case), {}

    def through(match):
        event, target, sendid, content = match.groups()
        sends.setdefault(sendid, f'<send event="{event}" id="{sendid}" target="{target}">{content}</send>')
        return f'<send event="harness.send.{sendid}"/>'

    text = SENT_AS.sub(through, text)
    resends = "".join(f'  <transition event="harness.send.{sendid}">{send}</transition>\n' for sendid, send in sends.items())
    return text.replace('  <transition event="*" target="fail">', resends + '  <transition event="*" target="fail">', 1)


S.document = _with_send_ids
INPUT = "bot.ability.speaking.input"
OUTPUT = "bot.ability.speaking.output"
FAILED = "bot.ability.failed"
EFFERENCE = "bot.ability.speaking.efference"
PROGRESS = "bot.ability.speaking.progress"
STOP = "bot.ability.speaking.stop"
SOUND = "environment.sound"
APPEND = "bot.ability.conversation.append"
READY = expect("bot.ability.offers", "_event.data.ability === 'speaking' && _event.data.choices.length === 0")

# The Gemini Interactions API's answer: 4800 bytes of 24 kHz mono 16-bit
# PCM (0.1 s).
PCM = {"path": "/interactions", "status": 200,
       "body": {"steps": [{"type": "model_output", "content": [{"type": "audio", "data": "A" * 6400}]}]}}
# Synthesis under a loaded host can take longer than 5 s: cases
# that reach the backend wait longer (statement 7 tests the bound itself).
PATIENT = [("encoding_timeout_ms", "600000")]
# Two seconds of the same (96000 bytes): four pieces of half the 1 s
# lookahead.
LONG_PCM = {"path": "/interactions", "status": 200,
            "body": {"steps": [{"type": "model_output", "content": [{"type": "audio", "data": "A" * 128000}]}]}}
ERROR = {"path": "/interactions", "status": 500, "body": {"error": {"message": "boom"}}}
# The OpenAI-compatible speech API's answer: MP3 bytes, no rate or duration.
MP3 = {"path": "/audio/speech", "status": 200, "body": "ID3-mp3"}
OPENAI_MP3 = [("encoding_timeout_ms", "600000"), ("provider", "null"), ("model", "'tts-1'"), ("voice", "'alloy'"), ("media_type", "'audio/mpeg'")]


BLANK = "A reply must say something: every part needs words, a sound or a pause, and the words must not be blank."
HOLDING = "Speaking is holding too many utterances; this one is refused."


def say(text):
    """An utterance of one part, its words `text`."""
    return send(INPUT, js({"parts": [{"words": text}]}))


def reply(*parts):
    return send(INPUT, js({"parts": list(parts)}))


def say_as(sendid, text):
    """An utterance whose sender names its id."""
    return send(INPUT, js({"parts": [{"words": text}]}), target=f"#_speaking|{sendid}")


def efference(duration="0.1"):
    return expect(EFFERENCE, "_event.data.mouth === 'speaking' && _event.data.duration === " + duration +
                  " && _event.data.media_type === 'audio/pcm' && _event.data.sample_rate_hz === 24000 && _event.data.channels === 1")


def sound(n, media="pcm", media_type="audio/pcm"):
    return expect(SOUND, f"_event.data.audio === 'media/speaking-{n}.{media}' && _event.data.media_type === '{media_type}'"
                  " && _event.data.sample_rate_hz === 24000 && _event.data.channels === 1 && _event.data.source === 'speaking'")


def appended(text, n):
    return expect(APPEND, "_event.data.message.direction === 'outbound' && _event.data.message.content === " + js(text) +
                  " && _event.data.message.content_type === 'text/plain' && !('sequence' in _event.data.message)"
                  " && Array.isArray(_event.data.message.participants) && _event.data.message.participants.length === 0"
                  " && _event.data.message.source_ids.length === 0 && _event.data.message.target_ids.length === 0"
                  f" && _event.data.message.provenance.id === 'speaking-{n}'"
                  " && _event.data.message.provenance.event === 'bot.ability.conversation.append'")


def output(text, media_type="audio/pcm", spoken=None, outcome="completed"):
    said = text if spoken is None else spoken
    return expect(OUTPUT, "_event.data.text === " + js(text) + f" && _event.data.media_type === '{media_type}'"
                  " && _event.data.sample_rate_hz === 24000 && _event.data.channels === 1"
                  f" && _event.data.spoken === {js(said)} && _event.data.unspoken === {js(text[len(said):])}"
                  f" && _event.data.outcome === '{outcome}' && _event.data.voice_id === null")


def progress(seconds):
    return expect(PROGRESS, "_event.data.spoken_seconds === " + seconds)


def stop():
    return send(STOP, js({}))


def seconds(ms):
    """A JavaScript number literal of `ms` / 1000."""
    return js(ms / 1000).removesuffix(".0")


def playout(duration_ms):
    """The playout after the sound: the first progress, then each piece's
    report (at most half the 1 s lookahead): the copy of what is still to
    come (0 after the last) and the progress."""
    steps, played = [progress("0")], 0
    while played < duration_ms:
        played += min(500, duration_ms - played)
        steps += [efference(seconds(duration_ms - played)), progress(seconds(played))]
    return steps


def spoken(text, n, duration_ms=100):
    """An utterance played out whole: its copy, its sound, its playout, the
    record and the answer."""
    return [efference(seconds(duration_ms)), sound(n)] + playout(duration_ms) + [
        appended(text, n), output(text), READY]


def refused():
    return [expect(FAILED, f"_event.data.message === {js(BLANK)}"), READY]


S.statement(1, "Every entry into `idle` (its start, and the end of each utterance, spoken or failed) sends `bot.ability.offers` `{ability: \"speaking\", choices: []}` to its parent: it offers nothing.")
S.statement(2, "`bot.ability.speaking.input` `{parts}` (1 to 64 parts `{expression?, words?}`, decision D178) is encoded by the vocal child, which gets the parts, with the speaking's `provider`, `model`, `voice` and `media_type` (by default the gemini encoder, `audio/pcm`), written to `output_prefix` + the utterance's number + `.` + the media subtype (`media/speaking-<n>.pcm`); the utterance's text of record is the parts' words joined exactly as written (nothing stripped).")
S.statement(3, "`playing` sends the efference copy `bot.ability.speaking.efference` `{mouth: \"speaking\", duration, media_type, sample_rate_hz, channels}` before the sound, `environment.sound` `{audio, media_type, sample_rate_hz, channels, source: \"speaking\"}` (its source the copy's mouth, so the listening can tell its own voice apart), and `bot.ability.speaking.progress` `{spoken_seconds: 0}`, to its parent; with no measured duration no copy is sent, and a rate or channel count the encoder leaves null is the speaking's own (24000 Hz mono).")
S.statement(4, "`recording` sends `bot.ability.conversation.append` `{message}` to its parent, the outbound message `<conversation:message_for_turn>` builds (content what was spoken, content_type `text/plain`, participants `[]` until the speaking has a voice of its own, provenance id `speaking-<n>` and event `bot.ability.conversation.append`), unless nothing was spoken, then answers `bot.ability.speaking.output` `{text, spoken, unspoken, outcome, voice_id: null, media_type, sample_rate_hz, channels}` (decision D179) and returns to `idle`.")
S.statement(5, "An utterance that says nothing (its words, joined, blank; a part whose words are blank; a part with neither words, a pause nor a sound; an expression that directs no cue) is refused before any encoding: `bot.ability.failed` `{message: \"A reply must say something: every part needs words, a sound or a pause, and the words must not be blank.\"}`, and back to `idle`; one its schema refuses (no parts, empty words) never arrives: the send raises `error.execution` in the sender.")
S.statement(6, "A vocal failure (its `bot.ability.failed`) answers the parent with `bot.ability.failed` `{message}` carrying the vocal's message, and back to `idle`, where the next input is spoken.")
S.statement(7, "Encoding is bounded by `encoding_timeout_ms` (30 s by default, decision D98): its timer answers `bot.ability.failed` `{message: \"Speech encoding timed out.\"}` and returns to `idle`.")
S.statement(8, "Input for another utterance that arrives outside `idle` is held (`<event:defer/>`), up to `held_utterances_bound` (16, a `<param>`), and taken, in order, on the return to `idle`; past that bound it is refused with `bot.ability.failed` `{message: \"Speaking is holding too many utterances; this one is refused.\"}` (decision D175).")
S.statement(9, "An utterance's id is its input's sendid (none when the sender names none). A finished utterance's id is kept among the last `finished_utterances_bound` (16) finished ids, and an input with a finished id, or another copy of the utterance in hand or of a held one, is dropped (logged `speaking.late`): nothing is said or answered for it, and a re-sent held id takes no second slot toward `held_utterances_bound` (the first copy is kept, decision D175).")

S.statement(10, "Playout lasts as long as the measured audio: the mouth's report of each piece (at most half of `lookahead_ms`, 1000; `devices.audio.played` `{mark}`, which the speaking times with `<send delay>` until the mouth is ported) is followed by the efference copy of the playout still to come (`duration` 0 after the last piece) and `bot.ability.speaking.progress` `{spoken_seconds}` (the seconds reported played); after the last piece the utterance is recorded, `outcome` `completed`, everything spoken. Audio with no measured duration counts as played at once.")
S.statement(11, "`bot.ability.speaking.stop` `{}` stops the utterance in hand: in encoding nothing is said; in playing what was said is the words the reported pieces cover, in proportion to the share of the audio played, rounded down to whole words. Either way the efference copy `{duration: 0}` leaves at once, the utterance is recorded with `outcome` `stopped` (appended to the conversation only when something was spoken), and the speaking is idle again. A stop with nothing in hand does nothing (logged `speaking.stale`).")

S.case(100, 1, "the speaking offers nothing on start.", [READY])
S.case(101, 1, "the speaking offers again after an utterance.",
       [READY, say("hi")] + spoken("hi", 1), params=PATIENT, backend=[PCM])
S.case(110, 2, "the text of record is the words exactly as written, nothing stripped, encoded by the gemini encoder and written to media/speaking-1.pcm.",
       [READY, say("  hello there  ")] + spoken("  hello there  ", 1),
       params=PATIENT, backend=[PCM])
S.case(111, 2, "the speaking's provider, model, voice and media_type params reach the encoder: the OpenAI-compatible API writes media/speaking-1.mpeg.",
       [READY, say("hi"), sound(1, "mpeg", "audio/mpeg"), progress("0"), appended("hi", 1), output("hi", "audio/mpeg"), READY],
       params=OPENAI_MP3, backend=[MP3])
S.case(112, 2, "a reply of several parts is one utterance, its text the parts' words joined as written; its pause reaches the encoder as silence (0.1 s, 0.4 s, 0.1 s).",
       [READY, reply({"words": "Well,"}, {"expression": {"pause": "short", "emotion": "warm"}, "words": " hello there."})]
       + spoken("Well, hello there.", 1, duration_ms=600),
       params=PATIENT, backend=[PCM])
S.case(120, 3, "the efference copy, with the measured duration, leaves before the sound.",
       [READY, say("hi"), efference("0.1"), sound(1)],
       ignore=[APPEND, OUTPUT, PROGRESS, "bot.ability.offers"], params=PATIENT, backend=[PCM])
S.case(121, 3, "with no measured duration no copy is sent, and the null rate and channels are the speaking's own.",
       [READY, say("hi"), sound(1, "mpeg", "audio/mpeg"), progress("0"), appended("hi", 1), output("hi", "audio/mpeg")],
       ignore=["bot.ability.offers"], params=OPENAI_MP3, backend=[MP3])
S.case(130, 4, "the words are appended to the conversation as an outbound message with no participants, then answered.",
       [READY, say("good morning")] + spoken("good morning", 1),
       params=PATIENT, backend=[PCM])
S.case(131, 4, "each utterance's provenance id is its number.",
       [READY, say("one")] + spoken("one", 1) + [say("two")] + spoken("two", 2), params=PATIENT, backend=[PCM])
S.case(140, 5, "blank words are refused, and the speaking is idle again.",
       [READY, say("   ")] + refused())
S.case(141, 5, "empty words are refused by the input's schema at the send; the next utterance is the first.",
       [READY, say(""), expect("error.execution"), say("hi")] + spoken("hi", 1),
       params=PATIENT, backend=[PCM])
S.case(142, 5, "a part whose words are blank is refused, though another part says something.",
       [READY, reply({"words": "Hello"}, {"words": " "})] + refused())
S.case(143, 5, "a part with neither words, a pause nor a sound is refused.",
       [READY, reply({"words": "Hello"}, {"expression": {"style": "whisper"}})] + refused())
S.case(144, 5, "an expression that directs no cue is refused.",
       [READY, reply({"expression": {}, "words": "Hello"})] + refused())
S.case(145, 5, "a reply of only a pause and a sound says no words and is refused.",
       [READY, reply({"expression": {"pause": "long"}}, {"expression": {"sound": "sigh"}})] + refused())
S.case(146, 5, "no parts are refused by the input's schema at the send.",
       [READY, reply(), expect("error.execution")])
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
S.case(171, 8, "past held_utterances_bound an input is refused; the held one is still spoken.",
       [READY, say("first"), say("second"), say("third"),
        expect(FAILED, f"_event.data.message === {js(HOLDING)}")] + spoken("first", 1) + spoken("second", 2),
       params=PATIENT + [("held_utterances_bound", "1")], backend=[PCM])
S.case(180, 9, "an input with the id of a finished utterance is dropped; the next utterance is the second spoken.",
       [READY, say_as("u-1", "first")] + spoken("first", 1)
       + [say_as("u-1", "first"), say_as("u-2", "second")] + spoken("second", 2),
       params=PATIENT, backend=[PCM])
S.case(181, 9, "another copy of the utterance in hand is dropped, not held.",
       [READY, say_as("u-1", "first"), say_as("u-1", "first"), say_as("u-2", "second")]
       + spoken("first", 1) + spoken("second", 2),
       params=PATIENT, backend=[PCM])
S.case(182, 9, "another copy of a held utterance is dropped and takes no slot: at held_utterances_bound 1 nothing is refused.",
       [READY, say_as("u-1", "first"), say_as("u-2", "second"), say_as("u-2", "second")]
       + spoken("first", 1) + spoken("second", 2),
       params=PATIENT + [("held_utterances_bound", "1")], backend=[PCM])

S.case(190, 10, "a 2 s playout is four pieces: a copy and a progress after each, then the record.",
       [READY, say("one two three four")] + spoken("one two three four", 1, duration_ms=2000),
       params=PATIENT, backend=[LONG_PCM])
S.case(191, 10, "with no measured duration the utterance is spoken at once, with no copy.",
       [READY, say("hi"), sound(1, "mpeg", "audio/mpeg"), progress("0"), appended("hi", 1),
        output("hi", "audio/mpeg"), READY],
       params=OPENAI_MP3, backend=[MP3])
S.case(195, 11, "a stop in encoding says nothing: a copy of nothing left, no record, the answer stopped.",
       [READY, say("hi"), stop(), efference("0"), output("hi", spoken="", outcome="stopped"), READY,
        say("again")] + spoken("again", 2),
       params=PATIENT, backend=[PCM])
S.case(196, 11, "a stop after the first of four pieces says the first quarter of the words.",
       [READY, say("one two three four"), efference("2"), sound(1), progress("0"), efference("1.5"), progress("0.5"),
        stop(), efference("0"), appended("one", 1), output("one two three four", spoken="one", outcome="stopped"), READY],
       params=PATIENT, backend=[LONG_PCM])
S.case(197, 11, "a stop with nothing in hand does nothing; the next utterance is spoken.",
       [READY, stop(), say("hi")] + spoken("hi", 1),
       params=PATIENT, backend=[PCM])
