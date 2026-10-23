# The turn_detector's statements and cases (tests/irp/harness.py writes them).

import json

S = suite
READY = "bot.ability.turn_detector.ready"
START = "bot.ability.turn_detector.turn.start"
UPDATE = "bot.ability.turn_detector.turn.update"
PAUSE = "bot.ability.turn_detector.turn.pause"
END = "bot.ability.turn_detector.turn.end"
SPEECH = "bot.ability.listening.speech.output"
COMPLETE = "bot.ability.turn_detector.turn.complete"
FAILED = "bot.ability.turn_detector.failed"
LONG = [("end_of_turn_silence_seconds", "60")]
NOTHING = "Turn closed with neither text, audio, nor modality-neutral content."
BOTH = "Turn product cannot contain both text and audio."


def ready(participant="bot", conversation="conversation"):
    return expect(READY, fields(participant_ref=participant, conversation_ref=conversation) + " && Object.keys(_event.data).length === 2")


def fields(**values):
    return " && ".join(f"JSON.stringify(_event.data.{k}) === {js(compact(v))}" for k, v in values.items())


def compact(value):
    """`value` as JSON.stringify writes it."""
    return json.dumps(value, separators=(",", ":"))


def text(content, source="caller"):
    return {"kind": "text", "source_participant_ref": source, "content": content}


def audio(content, rate=None, channels=None, source="caller"):
    stimulus = {"kind": "audio", "source_participant_ref": source, "content": content}
    if rate is not None:
        stimulus["sample_rate_hz"] = rate
    if channels is not None:
        stimulus["channels"] = channels
    return stimulus


def turn(event, **data):
    return send(event, js(data))


def speech(content, voiced, content_type="audio/pcm"):
    segments = [{"start_seconds": 0.0, "end_seconds": 0.2, "confidence": 0.9}] if voiced else []
    return send(SPEECH, js({"content": content, "content_type": content_type, "voice_detection": {"segments": segments},
                            "sample_rate_hz": 16000, "media_type": "audio/pcm", "channels": 1, "source_ids": []}))


def completed(**values):
    return expect(COMPLETE, fields(**values))


def failed(message, **values):
    return expect(FAILED, fields(stage="turn", message=message, **values))


def idle():
    """On the logical clock a delayed send to itself fires once the run is
    idle (before the 60 s silence): a step after which nothing else arrives."""
    return wait("1s")


S.statement(1, "On start it sends `bot.ability.turn_detector.ready` `{participant_ref, conversation_ref}` to its parent: its `<param>`s `participant_ref` (default `\"bot\"`) and `conversation_ref` (default `\"conversation\"`). It is long-lived: it never reaches a final state.")
S.statement(2, "In `idle`, `bot.ability.turn_detector.turn.start` whose `self_participant_ref` (default `\"bot\"`) is the detector's `participant_ref` opens the turn (`open`), taking its `turn_ref`, `conversation_ref` (default `\"conversation\"`) and `source_participant_ref` (default `\"caller\"`); a start for another participant opens nothing.")
S.statement(3, "`bot.ability.turn_detector.turn.end` for the open turn adds its content and closes it: `normalizing` answers the parent with `bot.ability.turn_detector.turn.complete` `{conversation_ref, turn_ref, participant_ref, self_participant_ref, source_participant_ref, text, audio, content, content_type, sample_rate_hz, channels}`, and the detector is `idle` again. Text stimuli are stripped and kept when not empty, joined by spaces; a turn with text has content the text and content_type `text/plain`.")
S.statement(4, "`bot.ability.turn_detector.turn.update` and a repeated `start` for the open turn add their content; `bot.ability.turn_detector.turn.pause` moves it to `paused`, from which update and start reopen it and end closes it. An update, pause or end whose `turn_ref`, `conversation_ref` or `source_participant_ref` is not the open turn's is ignored.")
S.statement(5, "Audio stimuli are kept in order by reference: the turn's audio and content are the references, content_type `audio/pcm`, and its sample_rate_hz and channels the first chunk's that carries them. Content and event stimuli replace the modality-neutral content: a content stimulus's content and content_type, an event as `{event, payload}` (payload default `{}`) with `application/event`; with no text or audio the turn hands that content on. An image stimulus changes nothing (a completed turn carries no images).")
S.statement(6, "A turn closed with neither text, audio nor modality-neutral content, or with both text and audio, answers `bot.ability.turn_detector.failed` `{stage: \"turn\", message, participant_ref, conversation_ref, turn_ref, source_participant_ref}` instead; the detector is `idle` again and takes the next turn.")
S.statement(7, "`bot.ability.listening.speech.output` with voice spans opens a turn in `idle` (its turn_ref the next `turn-<n>`, the detector's conversation_ref, source `caller`) and, in `open` or `paused`, adds the observation's content as an audio chunk while its content_type is `audio/pcm` (decoded words carry no frames), with its sample_rate_hz and channels (default 1); without voice spans it opens nothing in `idle` and moves `open` to `paused`.")
S.statement(8, "`open` and `paused` each wait `end_of_turn_silence_seconds` (at least 10 ms) from their entry, re-armed on every re-entry; when it passes, `closing` raises `bot.ability.turn_detector.turn.silence.expired` to itself and the turn is closed and answered as by an end.")
S.statement(9, "A started turn's `conversation_ref` becomes the detector's own: a later turn opened by voice takes it.")
S.statement(10, "A turn event whose data its schema rejects (e.g. a start with an empty text stimulus, an update with no content) is `error.execution` in its sender and never reaches the detector.")

# 1 readiness
S.case(100, 1, "with no params it announces its defaults.", [ready()])
S.case(101, 1, "its params are the identities it announces, and it is not done.",
       [ready("agent", "support-call")] + idle(),
       params=[("participant_ref", "'agent'"), ("conversation_ref", "'support-call'")] + LONG)

# 2 start
S.case(110, 2, "a start with every field opens the turn with them; its end answers the complete turn.",
       [ready(), turn(START, conversation_ref="call", turn_ref="t1", self_participant_ref="bot",
                      source_participant_ref="alice", content=text("hi", "alice")),
        turn(END, conversation_ref="call", turn_ref="t1", source_participant_ref="alice"),
        completed(conversation_ref="call", turn_ref="t1", participant_ref="bot", self_participant_ref="bot",
                  source_participant_ref="alice", text="hi", audio=[], content="hi", content_type="text/plain",
                  sample_rate_hz=None, channels=None)], params=LONG)
S.case(111, 2, "a start taking the defaults opens the turn conversation/caller.",
       [ready(), turn(START, turn_ref="t1", content=text("hello")), turn(END, turn_ref="t1"),
        completed(conversation_ref="conversation", turn_ref="t1", source_participant_ref="caller", text="hello")],
       params=LONG)
S.case(112, 2, "a start for another participant opens nothing: its end is ignored and the next turn is answered.",
       [ready(), turn(START, turn_ref="t1", self_participant_ref="other", content=text("not mine")),
        turn(END, turn_ref="t1"),
        turn(START, turn_ref="t2", content=text("mine")), turn(END, turn_ref="t2"),
        completed(turn_ref="t2", text="mine")], params=LONG)
S.case(113, 2, "a detector with another participant_ref takes starts addressed to it.",
       [ready("agent"), turn(START, turn_ref="t1", self_participant_ref="agent", content=text("yes")),
        turn(END, turn_ref="t1"),
        completed(turn_ref="t1", participant_ref="agent", self_participant_ref="agent", text="yes")],
       params=[("participant_ref", "'agent'")] + LONG)

# 3 end and text
S.case(120, 3, "texts are stripped, empty ones dropped and the rest joined by spaces, the end's included.",
       [ready(), turn(START, turn_ref="t1", content=text("  hello ")),
        turn(UPDATE, turn_ref="t1", content=text("   ")),
        turn(UPDATE, turn_ref="t1", content=text("world")),
        turn(END, turn_ref="t1", content=text("again ")),
        completed(text="hello world again", content="hello world again", content_type="text/plain", audio=[])],
       params=LONG)
S.case(121, 3, "after a turn closes the detector is idle: the next start opens a fresh turn.",
       [ready(), turn(START, turn_ref="t1", content=text("one")), turn(END, turn_ref="t1"),
        completed(turn_ref="t1", text="one"),
        turn(START, turn_ref="t2", content=text("two")), turn(END, turn_ref="t2"),
        completed(turn_ref="t2", text="two")], params=LONG)

# 4 update, start again, pause, other turns
S.case(130, 4, "a repeated start for the open turn and an update add content; a start for another turn is ignored while one is open.",
       [ready(), turn(START, turn_ref="t1", content=text("a")),
        turn(START, turn_ref="t1", content=text("b")),
        turn(START, turn_ref="t9", content=text("other")),
        turn(UPDATE, turn_ref="t1", content=text("c")),
        turn(END, turn_ref="t1"),
        completed(turn_ref="t1", text="a b c")], params=LONG)
S.case(131, 4, "a paused turn takes an update and closes on its end.",
       [ready(), turn(START, turn_ref="t1", content=text("a")),
        turn(PAUSE, turn_ref="t1", duration_seconds=0.2),
        turn(UPDATE, turn_ref="t1", content=text("b")),
        turn(PAUSE, turn_ref="t1"),
        turn(END, turn_ref="t1", content=text("c")),
        completed(turn_ref="t1", text="a b c")], params=LONG)
S.case(132, 4, "update, pause and end for another turn_ref, conversation_ref or source are ignored.",
       [ready(), turn(START, conversation_ref="call", turn_ref="t1", source_participant_ref="alice", content=text("a", "alice")),
        turn(UPDATE, conversation_ref="call", turn_ref="t2", source_participant_ref="alice", content=text("x", "alice")),
        turn(UPDATE, conversation_ref="other", turn_ref="t1", source_participant_ref="alice", content=text("x", "alice")),
        turn(UPDATE, conversation_ref="call", turn_ref="t1", content=text("x")),
        turn(END, conversation_ref="call", turn_ref="t2", source_participant_ref="alice"),
        turn(END, turn_ref="t1", source_participant_ref="alice"),
        turn(END, conversation_ref="call", turn_ref="t1", source_participant_ref="bob"),
        turn(UPDATE, conversation_ref="call", turn_ref="t1", source_participant_ref="alice", content=text("b", "alice")),
        turn(END, conversation_ref="call", turn_ref="t1", source_participant_ref="alice"),
        completed(conversation_ref="call", turn_ref="t1", source_participant_ref="alice", text="a b")], params=LONG)
S.case(133, 4, "an array source_participant_ref correlates by value.",
       [ready(), turn(START, turn_ref="t1", source_participant_ref=[0.5, -1], content=text("v", [0.5, -1])),
        turn(END, turn_ref="t1", source_participant_ref=[0.5, -1]),
        completed(turn_ref="t1", source_participant_ref=[0.5, -1], text="v")], params=LONG)

# 5 audio and modality-neutral content
S.case(140, 5, "audio chunks are kept in order by reference with the first chunk's format.",
       [ready(), turn(START, turn_ref="t6", content=audio("media/a.pcm", 16000, 1)),
        turn(UPDATE, turn_ref="t6", content=audio("media/b.pcm", 8000, 2)),
        turn(END, turn_ref="t6", content=audio("media/c.pcm")),
        completed(turn_ref="t6", text="", audio=["media/a.pcm", "media/b.pcm", "media/c.pcm"],
                  content=["media/a.pcm", "media/b.pcm", "media/c.pcm"], content_type="audio/pcm",
                  sample_rate_hz=16000, channels=1)], params=LONG)
S.case(141, 5, "the format is the first chunk's that carries one.",
       [ready(), turn(START, turn_ref="t1", content=audio("media/a.pcm")),
        turn(END, turn_ref="t1", content=audio("media/b.pcm", 8000, 2)),
        completed(audio=["media/a.pcm", "media/b.pcm"], sample_rate_hz=8000, channels=2)], params=LONG)
S.case(142, 5, "a content stimulus is handed on with its content_type.",
       [ready(), turn(START, turn_ref="t4", content={"kind": "content", "source_participant_ref": "caller",
                                                    "content": {"a": [1, None]}, "content_type": "application/x-sign-language"}),
        turn(END, turn_ref="t4"),
        completed(turn_ref="t4", text="", audio=[], content={"a": [1, None]},
                  content_type="application/x-sign-language", sample_rate_hz=None, channels=None)], params=LONG)
S.case(143, 5, "an image stimulus is no content, so a turn of one image fails as empty; with an event stimulus the turn is the event as {event, payload}.",
       [ready(), turn(START, turn_ref="t1", content={"kind": "image", "source_participant_ref": "caller", "content": "media/p.png"}),
        turn(END, turn_ref="t1"),
        failed(NOTHING, turn_ref="t1"),
        turn(START, turn_ref="t2", content={"kind": "image", "source_participant_ref": "caller", "content": "media/p.png"}),
        turn(END, turn_ref="t2", content={"kind": "event", "source_participant_ref": "caller", "event": "phone.ring"}),
        completed(turn_ref="t2", content={"event": "phone.ring", "payload": {}}, content_type="application/event")],
       params=LONG)
S.case(144, 5, "text wins over modality-neutral content.",
       [ready(), turn(START, turn_ref="t1", content={"kind": "event", "source_participant_ref": "caller",
                                                    "event": "phone.ring", "payload": {"n": 2}}),
        turn(END, turn_ref="t1", content=text("ringing")),
        completed(text="ringing", content="ringing", content_type="text/plain")], params=LONG)

# 6 failure
S.case(150, 6, "a turn with nothing to show fails, and the detector takes the next turn.",
       [ready(), turn(START, conversation_ref="call", turn_ref="t1", content=text("   ")),
        turn(END, conversation_ref="call", turn_ref="t1"),
        failed(NOTHING, participant_ref="bot", conversation_ref="call", turn_ref="t1", source_participant_ref="caller"),
        turn(START, turn_ref="t2", content=text("ok")), turn(END, turn_ref="t2"),
        completed(turn_ref="t2", text="ok")], params=LONG)
S.case(151, 6, "a turn with both text and audio fails.",
       [ready(), turn(START, turn_ref="t2", source_participant_ref="alice", content=text("hi", "alice")),
        turn(END, turn_ref="t2", source_participant_ref="alice", content=audio("media/a.pcm", source="alice")),
        failed(BOTH, participant_ref="bot", conversation_ref="conversation", turn_ref="t2", source_participant_ref="alice")],
       params=LONG)

# 7 heard speech
S.case(160, 7, "voiced frames open turn-1 and feed it; an unvoiced one pauses it; the silence closes it.",
       [ready(), speech("media/a.pcm", True), speech("media/b.pcm", True), speech("media/c.pcm", False),
        completed(conversation_ref="conversation", turn_ref="turn-1", participant_ref="bot", self_participant_ref="bot",
                  source_participant_ref="caller", text="", audio=["media/a.pcm", "media/b.pcm"],
                  content=["media/a.pcm", "media/b.pcm"], content_type="audio/pcm", sample_rate_hz=16000, channels=1)],
       params=[("end_of_turn_silence_seconds", "1")], clock="wall")
S.case(161, 7, "unvoiced speech opens nothing; decoded words open turn-1 with no frames, which fails with nothing to show.",
       [ready(), speech("media/d.pcm", False), speech("hello there", True, "text/plain"),
        failed(NOTHING, turn_ref="turn-1", conversation_ref="conversation", source_participant_ref="caller")],
       params=[("end_of_turn_silence_seconds", "0.5")], clock="wall")
S.case(162, 7, "voiced speech feeds a started turn's audio; the end closes it.",
       [ready(), turn(START, turn_ref="t1", content=audio("media/a.pcm")), speech("media/b.pcm", True),
        turn(END, turn_ref="t1"),
        completed(turn_ref="t1", audio=["media/a.pcm", "media/b.pcm"], sample_rate_hz=16000, channels=1)],
       params=LONG)
S.case(163, 7, "each turn opened by voice takes the next turn-<n>.",
       [ready(), speech("media/a.pcm", True), completed(turn_ref="turn-1", audio=["media/a.pcm"]),
        speech("media/b.pcm", True), completed(turn_ref="turn-2", audio=["media/b.pcm"])],
       params=[("end_of_turn_silence_seconds", "0.3")], clock="wall")

# 8 silence
S.case(170, 8, "a started turn left open closes on the silence and is answered as by an end.",
       [ready(), turn(START, turn_ref="t1", content=text("hi")),
        completed(turn_ref="t1", text="hi", content="hi")],
       params=[("end_of_turn_silence_seconds", "0.3")], clock="wall")
S.case(171, 8, "a paused turn closes on the silence; an empty one fails.",
       [ready(), turn(START, turn_ref="t1"), turn(PAUSE, turn_ref="t1"),
        failed(NOTHING, turn_ref="t1")],
       params=[("end_of_turn_silence_seconds", "0.3")], clock="wall")
S.case(172, 8, "every re-entry re-arms the silence: updates within it keep the turn open past it.",
       [ready(), turn(START, turn_ref="t1", content=text("a"))] + wait("1s") +
       [turn(UPDATE, turn_ref="t1", content=text("b"))] + wait("1s") +
       [turn(PAUSE, turn_ref="t1")] + wait("1s") +
       [turn(UPDATE, turn_ref="t1", content=text("c")),
        completed(turn_ref="t1", text="a b c")],
       params=[("end_of_turn_silence_seconds", "2")], clock="wall")
S.case(173, 8, "the silence waits at least 10 ms: a zero silence still closes the turn.",
       [ready(), turn(START, turn_ref="t1", content=text("hi")), completed(turn_ref="t1", text="hi")],
       params=[("end_of_turn_silence_seconds", "0")], clock="wall")

# 9 conversation_ref carried over
S.case(180, 9, "after a turn on conversation call, a voice turn takes call.",
       [ready(), turn(START, conversation_ref="call", turn_ref="t1", content=text("hi")),
        turn(END, conversation_ref="call", turn_ref="t1"), completed(conversation_ref="call"),
        speech("media/a.pcm", True), completed(turn_ref="turn-1", conversation_ref="call")],
       params=[("end_of_turn_silence_seconds", "0.3")], clock="wall")

# 10 schema
S.case(190, 10, "a start with an empty text stimulus is error.execution in the sender and opens nothing.",
       [ready(), turn(START, turn_ref="t4", content=text("")), expect("error.execution"),
        turn(END, turn_ref="t4")] + idle(), params=LONG)
S.case(191, 10, "an update with no content and an end with no turn_ref are error.execution in the sender.",
       [ready(), turn(START, turn_ref="t1", content=text("a")),
        turn(UPDATE, turn_ref="t1"), expect("error.execution"),
        turn(END), expect("error.execution"),
        turn(END, turn_ref="t1"), completed(turn_ref="t1", text="a")], params=LONG)
