# The communication's statements and cases (tests/irp/harness.py writes them).

import re

S = suite
# The harness is the environment the playout goes into: every case passes
# its own _sessionid as environment (decision D103), so the communication
# puts speaking's environment.sound into it at #_scxml_ + that id.
ENVIRONMENT = ("environment", "_sessionid")
_case = S.case


def _in_the_harness(*args, params=(), **options):
    return _case(*args, params=[ENVIRONMENT] + list(params), **options)


S.case = _in_the_harness
# A respond the harness sends with an id (its sendid, which the communication
# keeps as the response's id): the harness writes no id attribute itself, so
# the target carries it and the written document moves it into place.
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
OFFERS = "bot.ability.offers"
RESPOND = "bot.ability.communication.respond"
INPUT = "bot.ability.communication.input"
ACTIVATE = "bot.ability.communication.activate"
FAILED = "bot.ability.communication.failed"
HANDOFF = "bot.ability.cognition.input"
PRODUCT = "bot.ability.conversation.output"
REJECTED = "bot.ability.conversation.failed"
EFFERENCE = "bot.ability.speaking.efference"
SOUND = "environment.sound"
SPOKEN = "bot.ability.speaking.output"
STOP = "bot.ability.communication.stop"
INTERACTIONS = "/interactions"

# The offer: respond alone, its schema compacted ($schema, title and
# examples dropped), no target.
READY = expect(OFFERS, "_event.data.ability === 'communication' && _event.data.choices.length === 1"
                       " && _event.data.choices[0].event === 'bot.ability.communication.respond'"
                       " && _event.data.choices[0].target === undefined"
                       " && _event.data.choices[0].schema.title === undefined"
                       " && _event.data.choices[0].schema['$schema'] === undefined"
                       " && _event.data.choices[0].schema.required.indexOf('parts') >= 0")

# The offer while a response is said: respond (held) and stop, compacted.
RESPONDING = expect(OFFERS, "_event.data.ability === 'communication' && _event.data.choices.length === 2"
                            " && _event.data.choices.map(function (c) { return c.event; }).sort().join(' ') ==="
                            " 'bot.ability.communication.respond bot.ability.communication.stop'"
                            " && _event.data.choices.every(function (c) { return c.schema.title === undefined; })")

# A tenth of a second of 24 kHz mono 16-bit PCM (4800 zero bytes), as the
# Gemini speech backend answers it.
AUDIO = {"path": INTERACTIONS, "status": 200, "body": {"steps": [{"type": "model_output", "content": [{
    "type": "audio", "data": "A" * 6400, "sample_rate": 24000, "channels": 1,
    "mime_type": "audio/l16; rate=24000; channels=1"}]}]}}
# Two seconds of the same (96000 bytes): four pieces of playout.
LONG_AUDIO = {"path": INTERACTIONS, "status": 200, "body": {"steps": [{"type": "model_output", "content": [{
    "type": "audio", "data": "A" * 128000, "sample_rate": 24000, "channels": 1,
    "mime_type": "audio/l16; rate=24000; channels=1"}]}]}}
BOOM = {"path": INTERACTIONS, "status": 500, "body": {"error": {"message": "boom"}}}


def said(content, content_type="text/plain"):
    return js({"source_ids": ["gabe"], "target_ids": ["floppy"], "content": content, "content_type": content_type})


def respond(text):
    """A reply of one part, its words `text`."""
    return js({"parts": [{"words": text}]})


def reply(*parts):
    return js({"parts": list(parts)})


def respond_as(sendid, text):
    """A respond whose sender names its id."""
    return send(RESPOND, respond(text), target=f"#_communication|{sendid}")


def efference(duration):
    return expect(EFFERENCE, f"_event.data.mouth === 'speaking' && _event.data.duration === {duration}"
                             " && _event.data.media_type === 'audio/pcm' && _event.data.sample_rate_hz === 24000"
                             " && _event.data.channels === 1")


def spoken(operation, text):
    """Response `operation` said: the offer while it is said, its playout
    (the copy, the sound, the copy of nothing left once the mouth reported
    its one piece) and its answer, in order."""
    audio = f"media/speaking-{operation}.0.pcm"
    return [
        RESPONDING,
        efference("0.1"),
        expect(SOUND, f"_event.data.audio === {js(audio)} && _event.data.media_type === 'audio/pcm'"
                      " && _event.data.sample_rate_hz === 24000 && _event.data.channels === 1 && _event.data.source === 'speaking'"),
        efference("0"),
        expect(SPOKEN, f"_event.data.text === {js(text)} && _event.data.media_type === 'audio/pcm'"
                       " && _event.data.sample_rate_hz === 24000 && _event.data.channels === 1"
                       f" && _event.data.spoken === {js(text)} && _event.data.outcome === 'completed'"),
    ]


BLANK = "A reply must say something: every part needs words, a sound or a pause, and the words must not be blank."
HOLDING = "Communication is holding too many responses; this one is refused."
GABE = js(js([{"ref": "gabe", "role": "source"}, {"ref": "floppy", "role": "target"}]).replace(" ", ""))


def failed(message):
    return expect(FAILED, f"_event.data.message === {js(message)}")


S.statement(1, "It is ready once both its children, the conversation and speaking, are ready (their first `bot.ability.offers`): on every entry into active it sends `bot.ability.offers` `{ability: \"communication\", choices}`, choices the one model-callable `bot.ability.communication.respond` with its schema compacted (no `$schema`, title or examples), no target.")
S.statement(2, "`bot.ability.communication.input` in active, or in responding while a response is said (hearing goes on, decision D175), goes to the conversation; a text product comes back to the body as `bot.ability.cognition.input` `{stimulus}` unchanged, its stimulus carrying the conversation's current exchange, which the input opens as its one, inbound, message, stamped with the turn's participants (its source and target ids).")
S.statement(3, "A product that is not text reaches the body as the conversation's `bot.ability.conversation.output` itself (its latest message inbound); an exchange whose latest message is outbound (the record of a response) is not forwarded.")
S.statement(4, "A conversation that cannot take an input fails it: its `bot.ability.conversation.failed` `{stage, message}` goes on to the body unchanged.")
S.statement(5, "`bot.ability.communication.respond` `{parts}` (1 to 64 parts `{expression?, words?}`, decision D178) that says something, in active, is said through speaking: in order, the body gets speaking's efference copies `bot.ability.speaking.efference` `{mouth: \"speaking\", duration, media_type, sample_rate_hz, channels}` (the first before the sound, then one per piece played), the communication puts speaking's `environment.sound` `{audio, media_type, sample_rate_hz, channels, source: \"speaking\"}` into the environment whose id its `environment` names (`#_scxml_` + that id), and the body gets `bot.ability.speaking.output` `{text, spoken, unspoken, outcome, voice_id, media_type, sample_rate_hz, channels}`, its text the parts' words joined exactly as written; the communication is active again and offers again, and the outbound record joins the conversation's exchange only.")
S.statement(6, "A reply that says nothing (its words, joined, blank; a part whose words are blank; a part with neither words, a pause nor a sound; an expression that directs no cue) answers `bot.ability.communication.failed` `{message: \"A reply must say something: every part needs words, a sound or a pause, and the words must not be blank.\"}` without speaking; the communication stays active and says the next response. A respond its schema refuses (no parts, empty words) never arrives: the send raises `error.execution` in the sender.")
S.statement(7, "A speaking failure answers `bot.ability.communication.failed` `{message}` (speaking's failure message) and the communication is active again.")
S.statement(8, "A response is bounded by `response_timeout_ms` (36000 by default, a `<param>`), which bounds speaking's silence on a response: each `bot.ability.speaking.progress` re-arms it (D179). When it passes before speaking answers, the body gets `bot.ability.communication.failed` `{message: \"Speaking response timed out.\"}` and the communication is active again; speaking's late answer is not forwarded. `encoding_timeout_ms` (30000 by default, a `<param>`; decision D98) is passed to speaking as its encoding bound.")
S.statement(9, "Responses that arrive while a response is in hand, and input that arrives before it is active, are deferred and taken, in order, once it is active.")
S.statement(10, "`bot.ability.communication.activate` `{}` is taken only in inactive: in active it does nothing.")
S.statement(11, "A response's id is its respond's sendid (none when the sender names none). A finished response's id is kept among the last `closed_responses_bound` (16) finished ids, and a respond with a finished id, or another copy of the response in hand or of a held one, is dropped (logged `communication.late`): nothing is said or answered for it, and a re-sent held id takes no second slot toward `held_responses_bound` (the first copy is kept, decision D175).")
S.statement(12, "At most `held_responses_bound` (16, a `<param>`) responses are held while one is in hand; past that a respond is refused with `bot.ability.communication.failed` `{message: \"Communication is holding too many responses; this one is refused.\"}` (decision D175), and the held ones are still said, in order.")
S.statement(13, "Every entry into responding sends `bot.ability.offers` `{ability: \"communication\", choices}`, the events enabled there: respond (held) and the model-callable `bot.ability.communication.stop` `{}` (decision D179). A stop while a response is said sends speaking `bot.ability.speaking.stop` `{}`, which stops it: the body gets the efference copy `{duration: 0}` and `bot.ability.speaking.output` with `outcome` `stopped`, and the communication is active again.")
S.statement(14, "Speaking's `bot.ability.speaking.progress` re-arms the response's hop timer (decision D179), and the hop timeout also sends speaking `bot.ability.speaking.stop` `{}`.")

# The cases that say a response wait on the real speech service; its first
# encoding outlasts speaking's 5 s bound on a loaded test host, so they lift
# both bounds.
SPEECH_BOUNDS = [("encoding_timeout_ms", "600000"), ("response_timeout_ms", "600000")]

S.case(100, 1, "the communication is ready with its respond offer.", [READY], backend=[], clock="wall")

S.case(110, 2, "text input is a turn whose stimulus's exchange is the input, inbound, with the turn's participants.",
       [READY, send(INPUT, said("hello there")),
        expect(HANDOFF, "_event.data.stimulus.data.messages.length === 1"
                        " && _event.data.stimulus.data.messages[0].direction === 'inbound'"
                        " && _event.data.stimulus.data.messages[0].content === 'hello there'"
                        f" && JSON.stringify(_event.data.stimulus.data.messages[0].participants) === {GABE}"
                        " && !('sequence' in _event.data.stimulus.data.messages[0])")],
       backend=[], clock="wall")
S.case(111, 2, "a second input opens a new exchange: its stimulus carries only that input.",
       [READY, send(INPUT, said("one")), expect(HANDOFF, "_event.data.stimulus.data.messages.length === 1"),
        send(INPUT, said("two")),
        expect(HANDOFF, "_event.data.stimulus.data.messages.length === 1"
                        " && _event.data.stimulus.data.messages[0].content === 'two'")],
       backend=[], clock="wall")
S.case(112, 2, "input that arrives while a response is said is heard at once, before speaking answers.",
       [READY, send(RESPOND, respond("hold on")), RESPONDING, send(INPUT, said("are you there")),
        expect(HANDOFF, "_event.data.stimulus.data.messages.length === 1"
                        " && _event.data.stimulus.data.messages[0].content === 'are you there'"),
        expect(SPOKEN, "_event.data.text === 'hold on'"), READY],
       params=SPEECH_BOUNDS, backend=[AUDIO], ignore=[EFFERENCE, SOUND], clock="wall")

S.case(120, 3, "a product that is not text reaches the body as the conversation output.",
       [READY, send(INPUT, said({"a": 1}, "application/json")),
        expect(PRODUCT, "_event.data.messages.length === 1 && _event.data.messages[0].direction === 'inbound'"
                        " && _event.data.messages[0].content.a === 1")],
       backend=[], clock="wall")
S.case(121, 3, "the outbound record of a response joins the exchange only: nothing is forwarded, and the next text turn opens a new exchange.",
       [READY, send(INPUT, said("one")), expect(HANDOFF, "_event.data.stimulus.data.messages.length === 1"),
        send(RESPOND, respond("reply 1")), *spoken(1, "reply 1"), READY,
        send(INPUT, said("two")),
        expect(HANDOFF, "_event.data.stimulus.data.messages.length === 1"
                        " && _event.data.stimulus.data.messages[0].direction === 'inbound'"
                        " && _event.data.stimulus.data.messages[0].content === 'two'")],
       params=SPEECH_BOUNDS, backend=[AUDIO], clock="wall")

S.case(130, 4, "input whose turn closes with nothing to show is failed by the conversation, forwarded unchanged.",
       [READY, send(INPUT, said("   ")),
        expect(REJECTED, "_event.data.stage === 'turn_detector'"
                         " && _event.data.message === 'Turn closed with neither text, audio, nor modality-neutral content.'")],
       backend=[], clock="wall")

S.case(140, 5, "a response is spoken through the speech service, played out, answered, and offered again.",
       [READY, send(RESPOND, respond("Hi! I am floppy.")), *spoken(1, "Hi! I am floppy."), READY],
       params=SPEECH_BOUNDS, backend=[AUDIO], clock="wall")
S.case(141, 5, "each response is numbered: the second plays out as speaking-2 (its one chunk, speaking-2.0).",
       [READY, send(RESPOND, respond("one")), *spoken(1, "one"), READY,
        send(RESPOND, respond("two")), *spoken(2, "two"), READY],
       params=SPEECH_BOUNDS, backend=[AUDIO], clock="wall")
S.case(142, 5, "a reply of several parts is said as one response, its text the parts' words joined exactly as written.",
       [READY, send(RESPOND, reply({"words": " Hi"}, {"expression": {"emotion": "warm", "emphasis": True}, "words": ", I am floppy. "})),
        *spoken(1, " Hi, I am floppy. "), READY],
       params=SPEECH_BOUNDS, backend=[AUDIO], clock="wall")

S.case(150, 6, "a reply whose words are blank fails without speaking; the next is said.",
       [READY, send(RESPOND, respond(" \n\t")), failed(BLANK),
        send(RESPOND, respond("next")), *spoken(1, "next"), READY],
       params=SPEECH_BOUNDS, backend=[AUDIO], clock="wall")
S.case(151, 6, "a part whose words are blank fails the reply, though the others say something.",
       [READY, send(RESPOND, reply({"words": "Hello"}, {"words": "  "})), failed(BLANK)],
       backend=[], clock="wall")
S.case(152, 6, "a part with neither words, a pause nor a sound fails the reply.",
       [READY, send(RESPOND, reply({"words": "Hello"}, {"expression": {"emotion": "happy"}})), failed(BLANK)],
       backend=[], clock="wall")
S.case(153, 6, "an expression that directs no cue fails the reply.",
       [READY, send(RESPOND, reply({"expression": {"sound_at": "after"}, "words": "Hello"})), failed(BLANK)],
       backend=[], clock="wall")
S.case(154, 6, "a reply of only a pause and a sound says no words and fails.",
       [READY, send(RESPOND, reply({"expression": {"pause": "short"}}, {"expression": {"sound": "laugh"}})), failed(BLANK)],
       backend=[], clock="wall")
S.case(155, 6, "a respond without parts, or with empty words, is refused by its schema at the send.",
       [READY, send(RESPOND, reply()), expect("error.execution"),
        send(RESPOND, respond("")), expect("error.execution"),
        send(INPUT, said("still here")),
        expect(HANDOFF, "_event.data.stimulus.data.messages[0].content === 'still here'")],
       backend=[], clock="wall")

S.case(160, 7, "a speech service failure fails the response; the communication is active again.",
       [READY, send(RESPOND, respond("hello")), RESPONDING,
        expect(FAILED, "typeof _event.data.message === 'string' && _event.data.message.length > 0"
                       " && _event.data.message !== 'Speaking response timed out.'"),
        READY],
       backend=[BOOM], clock="wall")

S.case(170, 8, "a response speaking does not answer in time fails with the hop timeout.",
       [READY, send(RESPOND, respond("hello")), RESPONDING, failed("Speaking response timed out."), READY],
       params=[("response_timeout_ms", "100")], backend=[AUDIO],
       ignore=[EFFERENCE, SOUND])

S.case(180, 9, "a response that arrives while one is in hand is said after it.",
       [READY, send(RESPOND, respond("first")), send(RESPOND, respond("second")),
        *spoken(1, "first"), READY, *spoken(2, "second"), READY],
       params=SPEECH_BOUNDS, backend=[AUDIO], clock="wall")
S.case(202, 11, "another copy of a held response is dropped and takes no slot: at held_responses_bound 1 nothing is refused.",
       [READY, respond_as("r-1", "first"), respond_as("r-2", "second"), respond_as("r-2", "second"),
        *spoken(1, "first"), READY, *spoken(2, "second"), READY],
       params=SPEECH_BOUNDS + [("held_responses_bound", "1")], backend=[AUDIO], clock="wall")
S.case(181, 9, "input sent before the communication is active is taken once it is.",
       [send(INPUT, said("early")), READY,
        expect(HANDOFF, "_event.data.stimulus.data.messages[0].content === 'early'")],
       backend=[], clock="wall")

S.case(190, 10, "activate in active does nothing.",
       [READY, send(ACTIVATE, js({})), wait("500ms"),
        send(INPUT, said("still here")),
        expect(HANDOFF, "_event.data.stimulus.data.messages[0].content === 'still here'")],
       backend=[], clock="wall")

S.case(200, 11, "a respond with the id of a finished response is dropped; the next response is the second said.",
       [READY, respond_as("r-1", "first"), *spoken(1, "first"), READY,
        respond_as("r-1", "first"), respond_as("r-2", "second"), *spoken(2, "second"), READY],
       params=SPEECH_BOUNDS, backend=[AUDIO], clock="wall")
S.case(201, 11, "another copy of the response in hand is dropped, not held.",
       [READY, respond_as("r-1", "first"), respond_as("r-1", "first"), respond_as("r-2", "second"),
        *spoken(1, "first"), READY, *spoken(2, "second"), READY],
       params=SPEECH_BOUNDS, backend=[AUDIO], clock="wall")

S.case(210, 12, "past held_responses_bound a respond is refused; the held one is still said.",
       [READY, send(RESPOND, respond("first")), send(RESPOND, respond("second")), send(RESPOND, respond("third")),
        RESPONDING, failed(HOLDING), *spoken(1, "first")[1:], READY, *spoken(2, "second"), READY],
       params=SPEECH_BOUNDS + [("held_responses_bound", "1")], backend=[AUDIO], clock="wall")

S.case(220, 13, "a stop while a response is said stops speaking: nothing was played, so nothing is said or recorded.",
       [READY, send(RESPOND, respond("one two three four")), RESPONDING, efference("2"), send(STOP, js({})),
        efference("0"),
        expect(SPOKEN, "_event.data.text === 'one two three four' && _event.data.spoken === ''"
                       " && _event.data.unspoken === 'one two three four' && _event.data.outcome === 'stopped'"),
        READY],
       params=SPEECH_BOUNDS, backend=[LONG_AUDIO], ignore=[SOUND], clock="wall")
S.case(221, 14, "a response whose hop times out stops speaking: its copy of nothing left follows the failure.",
       [READY, send(RESPOND, respond("hello")), RESPONDING, failed("Speaking response timed out."), READY,
        efference("0")],
       params=[("response_timeout_ms", "100")], backend=[AUDIO], ignore=[SOUND])
