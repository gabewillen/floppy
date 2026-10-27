# The conversation's statements and cases (tests/irp/harness.py writes them).

S = suite
INPUT = "bot.ability.conversation.input"
ROUTED = "bot.ability.conversation.routed_input"
APPEND = "bot.ability.conversation.append"
OUTPUT = "bot.ability.conversation.output"
FAILED = "bot.ability.conversation.failed"
COGNITION = "bot.ability.cognition.input"
SNAPSHOT = "bot.ability.conversation.snapshot.request"
SNAPSHOT_OUT = "bot.ability.conversation.snapshot.output"
READY = expect("bot.ability.ready")
DB = "'/memory/floppy.sqlite'"
# Memory under a loaded host can outlast the default 5 s bound: cases
# that reach the sql service wait longer (the bound itself loses every race to an in-process child: no case can make it fire).
MEMORY = [memory(DB), ("memory_timeout_ms", "600000")]

AMBIENT = 'relationship-{"identity_groups":[[],["alice"]]}'
ALICE_BOT = 'relationship-{"identity_groups":[["alice"],["bot"]]}'
ALICE_ZED = 'relationship-{"identity_groups":[[],["alice","zed"]]}'
EMPTY_TURN = "Turn closed with neither text, audio, nor modality-neutral content."


def text_input(content, sources=("alice",), targets=()):
    return {"source_ids": list(sources), "target_ids": list(targets), "content": content, "content_type": "text/plain"}


def say(content, sources=("alice",), targets=()):
    return send(INPUT, js(text_input(content, sources, targets)))


def last(field):
    return f"_event.data.stimulus.data.messages[_event.data.stimulus.data.messages.length - 1].{field}"


def committed(content, turn_ref="input-1:turn:alice", session_ref=AMBIENT, event=INPUT, extra=""):
    """The text handoff of an inbound commit: it opens a new exchange, the
    message its one message."""
    cond = ("_event.data.stimulus.event === 'bot.ability.conversation.output' && _event.data.stimulus.source === 'conversation'"
            " && _event.data.stimulus.data.messages.length === 1"
            f" && !('sequence' in {last('direction').rsplit('.', 1)[0]}) && {last('direction')} === 'inbound'"
            f" && {last('content')} === {js(content)} && {last('content_type')} === 'text/plain'"
            f" && {last('provenance.turn_ref')} === {js(turn_ref)} && {last('provenance.session_ref')} === {js(session_ref)}"
            f" && {last('provenance.event')} === {js(event)}"
            f" && _event.data.stimulus.data.parent.event === {js(event)}")
    return expect(COGNITION, cond + extra)


def failed(message, stage="turn_detector"):
    return expect(FAILED, f"_event.data.stage === {js(stage)} && _event.data.message === {js(message)}")


def snapshot(refs, ref="s-1"):
    return [send(SNAPSHOT, js({"request_ref": ref})),
            expect(SNAPSHOT_OUT, f"_event.data.request_ref === {js(ref)} && JSON.stringify(_event.data.detector_refs) === {js(json_text(refs))}")]


def json_text(value):
    import json
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False)


def outbound(content, id_, source=None, target=None, participants=None):
    provenance = {"event": "x.y", "id": id_}
    if source is not None:
        provenance["source"] = source
    if target is not None:
        provenance["target"] = target
    message = {"direction": "outbound", "source_ids": ["bot"], "target_ids": ["alice"],
               "content": content, "content_type": "text/plain", "provenance": provenance}
    if participants is not None:
        message["participants"] = participants
    return send(APPEND, js({"message": message}))


def participants(location, value):
    """The participants at `location` are `value` (their members as the
    committed message writes them, sorted)."""
    import json
    return f"JSON.stringify({location}) === {js(json.dumps(value, separators=(',', ':'), sort_keys=True))}"


def exchange(count, cond=""):
    return expect(OUTPUT, f"_event.data.messages.length === {count}" + (" && " + cond if cond else ""))


ALICE = [{"ref": "alice", "role": "source"}]
ALICE_TO_BOT = [{"ref": "alice", "role": "source"}, {"ref": "bot", "role": "target"}]

S.statement(1, "On start it sends `bot.ability.ready` to its parent (decision D242), with no data: it offers nothing.")
S.statement(2, "A direct `bot.ability.conversation.input` (its parent the envelope `{event: \"bot.ability.conversation.input\", data}`) or a `bot.ability.conversation.routed_input` `{parent}` (the communication's envelope, whose data is the input) is committed as an inbound message (provenance event the parent's event; no sequence); an inbound commit of text hands the parent `bot.ability.cognition.input` `{stimulus, participants}` whose stimulus is `{event: \"bot.ability.conversation.output\", data: {parent, messages}, source: \"conversation\"}`, messages the exchange the commit opens.")
S.statement(3, "Each input is one turn per source identity (names sorted, then embeddings), each exchanged with its own turn detector; what is committed is the turns' product: their stripped texts joined by spaces, each distinct text once, as `text/plain`; with no text, null content for audio and the input's content otherwise. Its provenance's turn_ref is the first turn's, `input-<n>:turn:<track>`. A commit that is not text answers `bot.ability.conversation.output` `{parent, messages}` itself.")
S.statement(4, "Each input finds its relationship: targetless input shares the first targetless (ambient) relationship; other input takes the relationship whose source and target groups match, in either direction; with no match it begins its own, whose session_ref is `relationship-` and its context reference `{\"identity_groups\":[...]}`. The committed provenance's session_ref is the relationship's.")
S.statement(5, "`bot.ability.conversation.snapshot.request` `{request_ref}`, in any state, answers `bot.ability.conversation.snapshot.output` `{request_ref, detector_refs}`: `[session_ref, source]` of every participant (a source whose detector was ready), ordered by relationship and then identity.")
S.statement(6, "`bot.ability.conversation.append` `{message}` with an outbound message joins the current exchange and is answered `bot.ability.conversation.output` `{parent, messages}`; one whose provenance id (non-empty), source and target match a message's in the current exchange is not committed again but is answered all the same; an inbound one is not taken (nothing is committed or answered).")
S.statement(7, "Input that arrives while one is in hand is held (`<event:defer/>`) and taken, in order, on the return to `waiting`.")
S.statement(8, "A turn the detector cannot close (`bot.ability.turn_detector.failed`) or cannot take (an `error.*`: a start its schema refuses) answers `bot.ability.conversation.failed` `{stage: \"turn_detector\", message}` and commits nothing.")
S.statement(9, "With a `memory` param (decision D266) it recalls the relationship's memories before the turns and remembers the input after them through the memory ability, then commits as without one; a memory failure answers `bot.ability.conversation.failed` `{stage: \"memory\", message}` and commits nothing.")
S.statement(10, "An input its schema refuses is never routed here: the send raises `error.execution` in the sender, and nothing is committed.")
S.statement(11, "Every output is the current exchange (decision D176), never the cumulative history: an inbound commit opens a new exchange, `[the inbound message]`, its input the exchange's `parent`; each outbound commit joins it; the next inbound replaces it, and nothing earlier is carried.")
S.statement(12, "Every message carries `participants`, stamped by its producer and never inferred: an inbound message the turn's participants as `<conversation:turn_participants>` stamps them (its string source ids as `{ref, role: \"source\"}`, then its string target ids as `{ref, role: \"target\"}`; an embedding is no ref), which the cognition handoff carries too; an outbound message those its append carries (`[]` when it carries none).")

S.case(100, 1, "the conversation is ready on start.", [READY])

S.case(110, 2, "a direct text input is committed and handed to the parent as a cognition input.",
       [READY, say("hello"),
        committed("hello", extra=" && _event.data.stimulus.data.parent.data.content === 'hello'"
                  " && JSON.stringify(" + last("source_ids") + ") === '[\"alice\"]'")])
ROUTED_PARENT = {"event": "bot.ability.communication.input", "data": text_input("routed hi")}
S.case(111, 2, "a routed input is committed under its parent envelope's event.",
       [READY, send(ROUTED, js({"parent": ROUTED_PARENT})),
        committed("routed hi", event="bot.ability.communication.input",
                  extra=" && _event.data.stimulus.data.parent.data.content === 'routed hi'")])
S.case(112, 2, "successive inputs are committed in causal order, each its own handoff.",
       [READY, say("one"), committed("one"), say("two"),
        committed("two", turn_ref="input-2:turn:alice", extra=" && _event.data.stimulus.data.parent.data.content === 'two'")])

S.case(120, 3, "several sources: one turn each, in identity order, and the same text committed once.",
       [READY, say("  hi  ", sources=("zed", "alice")), committed("hi", session_ref=ALICE_ZED)])
S.case(121, 3, "audio content is committed with null content and answered as the conversation output.",
       [READY, send(INPUT, js({"source_ids": ["alice"], "target_ids": [], "content": "AAAA", "content_type": "Audio/PCM",
                               "sample_rate_hz": 16000, "channels": 1})),
        exchange(1, "_event.data.messages[0].content === null && _event.data.messages[0].content_type === 'Audio/PCM'"
                    " && _event.data.messages[0].direction === 'inbound' && _event.data.messages[0].provenance.turn_ref === 'input-1:turn:alice'"
                    " && _event.data.parent.event === 'bot.ability.conversation.input'")])
S.case(122, 3, "structured content is kept and answered as the conversation output; the first turn is the first name.",
       [READY, send(INPUT, js({"source_ids": [[2, 1.25], "bob"], "target_ids": ["bot"], "content": {"b": 1, "a": [True, None]},
                               "content_type": "application/json"})),
        exchange(1, "_event.data.messages[0].content.b === 1 && _event.data.messages[0].content.a[0] === true && _event.data.messages[0].content.a[1] === null"
                    " && _event.data.messages[0].provenance.turn_ref === 'input-1:turn:bob'")])

S.case(130, 4, "targetless input shares the ambient relationship; addressed input begins its own.",
       [READY, say("aside"), committed("aside"),
        say("to bot", targets=("bot",)), committed("to bot", turn_ref="input-2:turn:alice", session_ref=ALICE_BOT),
        send(INPUT, js(text_input("aside too", sources=("bob",)))),
        committed("aside too", turn_ref="input-3:turn:bob", session_ref=AMBIENT)])
S.case(131, 4, "the reply in the other direction belongs to the same relationship.",
       [READY, say("hi bot", targets=("bot",)), committed("hi bot", session_ref=ALICE_BOT),
        send(INPUT, js(text_input("hi alice", sources=("bot",), targets=("alice",)))),
        committed("hi alice", turn_ref="input-2:turn:bot", session_ref=ALICE_BOT)])
S.case(132, 4, "input addressed to an ambient participant joins the ambient relationship.",
       [READY, say("aside"), committed("aside"),
        send(INPUT, js(text_input("to alice", sources=("carol",), targets=("alice",)))),
        committed("to alice", turn_ref="input-2:turn:carol", session_ref=AMBIENT)])

S.case(140, 5, "before any input the snapshot holds no detectors.", [READY] + snapshot([]))
S.case(141, 5, "each ready source is a participant, ordered by relationship and then identity.",
       [READY, say("aside", sources=("zed", "alice")), committed("aside", session_ref=ALICE_ZED),
        say("to bot", targets=("bot",)), committed("to bot", turn_ref="input-2:turn:alice", session_ref=ALICE_BOT)]
       + snapshot([[ALICE_BOT, "alice"], [ALICE_ZED, "alice"], [ALICE_ZED, "zed"]]))
S.case(142, 5, "a snapshot is answered while an input is in hand.",
       [READY, say("hello"), send(SNAPSHOT, js({"request_ref": "busy"})),
        expect(SNAPSHOT_OUT, "_event.data.request_ref === 'busy' && Array.isArray(_event.data.detector_refs)"),
        committed("hello")])

S.case(150, 6, "an outbound append joins the exchange after its inbound message and is answered with the exchange.",
       [READY, say("hello"), committed("hello"), outbound("hi there", "r-1", source="q"),
        exchange(2, "_event.data.messages[1].direction === 'outbound' && !('sequence' in _event.data.messages[1])"
                    " && _event.data.messages[1].content === 'hi there' && _event.data.messages[1].provenance.id === 'r-1'"
                    " && _event.data.messages[1].provenance.source === 'q'"
                    " && _event.data.parent.data.content === 'hello'")])
S.case(151, 6, "an append with an id, source and target already in the exchange is answered without being committed again; another source is committed.",
       [READY, outbound("once", "r-1", source="q"), exchange(1, "_event.data.messages[0].content === 'once' && _event.data.parent === null"),
        outbound("once again", "r-1", source="q"), exchange(1),
        outbound("other", "r-1"), exchange(2, "_event.data.messages[1].content === 'other' && _event.data.messages[1].provenance.source === null")])
S.case(152, 6, "an inbound append is not taken: nothing is committed or answered.",
       [READY, send(APPEND, js({"message": {"direction": "inbound", "source_ids": ["alice"], "target_ids": [],
                                            "content": "x", "content_type": "text/plain", "provenance": {"event": "x.y", "id": "i-1"}}})),
        outbound("after", "r-2"), exchange(1, "_event.data.messages[0].content === 'after'")])

S.case(160, 7, "inputs sent together are committed one at a time, in order.",
       [READY, say("first"), say("second"), say("third"),
        committed("first"), committed("second", turn_ref="input-2:turn:alice"),
        committed("third", turn_ref="input-3:turn:alice")])

S.case(170, 8, "white space only: the detector closes the turn with nothing to show, and nothing is committed.",
       [READY, say("   "), failed(EMPTY_TURN), say("next"), committed("next", turn_ref="input-2:turn:alice")])
S.case(171, 8, "empty text: the detector's schema refuses the start, and nothing is committed.",
       [READY, say(""), failed("The turn detector could not take the turn (error.execution)"),
        say("next"), committed("next", turn_ref="input-2:turn:alice")])
S.case(172, 8, "a failed turn's source is no participant.",
       [READY, say("   "), failed(EMPTY_TURN)] + snapshot([]))

S.case(180, 9, "with a database the input is recalled, remembered and committed.",
       [READY, say("hello"), committed("hello"), say("again"), committed("again", turn_ref="input-2:turn:alice")],
       params=MEMORY, clock="wall")
S.case(181, 9, "a database the memory cannot open fails the input at the memory stage.",
       [READY, say("hello"), expect(FAILED, "_event.data.stage === 'memory' && typeof _event.data.message === 'string' && _event.data.message.length > 0")]
       + snapshot([]),
       params=[memory("'/nowhere/floppy.sqlite'"), ("memory_timeout_ms", "600000")], clock="wall")
S.case(190, 10, "an input without source identities is refused at the send; the next is committed first.",
       [READY, send(INPUT, js({"source_ids": [], "target_ids": [], "content": "x", "content_type": "text/plain"})),
        expect("error.execution"), say("next"), committed("next")])

S.case(200, 11, "an inbound commit opens a new exchange: the reply and the input before it are not carried.",
       [READY, say("hello"), committed("hello"), outbound("hi there", "r-1"), exchange(2),
        say("again"),
        committed("again", turn_ref="input-2:turn:alice", extra=" && _event.data.stimulus.data.parent.data.content === 'again'")])
S.case(201, 11, "an append matches only the current exchange: the id of a reply in an earlier exchange is committed again.",
       [READY, say("hello"), committed("hello"), outbound("hi", "r-1"), exchange(2),
        say("again"), committed("again", turn_ref="input-2:turn:alice"),
        outbound("hi", "r-1"), exchange(2, "_event.data.messages[0].content === 'again' && _event.data.messages[1].provenance.id === 'r-1'"
                                          " && _event.data.parent.data.content === 'again'")])
S.case(202, 11, "a commit that is not text opens a new exchange too.",
       [READY, say("hello"), committed("hello"), outbound("hi", "r-1"), exchange(2),
        send(INPUT, js({"source_ids": ["alice"], "target_ids": [], "content": {"a": 1}, "content_type": "application/json"})),
        exchange(1, "_event.data.messages[0].content.a === 1 && _event.data.parent.data.content.a === 1")])

S.case(210, 12, "an inbound message and its handoff carry the turn's participants, sources then targets.",
       [READY, say("hi bot", targets=("bot",)),
        committed("hi bot", session_ref=ALICE_BOT,
                  extra=" && " + participants(last("participants"), ALICE_TO_BOT)
                        + " && " + participants("_event.data.participants", ALICE_TO_BOT))])
S.case(211, 12, "an embedding is no participant: only the named source is stamped.",
       [READY, send(INPUT, js({"source_ids": [[2, 1.25], "alice"], "target_ids": [], "content": "hi", "content_type": "text/plain"})),
        expect(COGNITION, participants(last("participants"), ALICE) + " && " + participants("_event.data.participants", ALICE))])
S.case(212, 12, "an outbound message carries the participants its append stamps, and none when it stamps none.",
       [READY, outbound("hi", "r-1", participants=[{"ref": "voice:warm", "role": "source", "own": True}]),
        exchange(1, participants("_event.data.messages[0].participants", [{"ref": "voice:warm", "role": "source", "own": True}])),
        outbound("again", "r-2"), exchange(2, participants("_event.data.messages[1].participants", []))])

PERSON_1 = 'relationship-{"identity_groups":[[],["person-1"]]}'
VOICE_A = [0.28, 0.96]
VOICE_A_AGAIN = [0.3, 0.95]
VOICE_B = [0.96, -0.28]


def heard(content, voice):
    return send(INPUT, js(text_input(content, sources=(voice,))))


def by(person):
    return (" && JSON.stringify(" + last("source_ids") + ") === " + js(json_text([person]))
            + " && " + participants("_event.data.participants", [{"ref": person, "role": "source"}]))


S.statement(13, "With a `memory` param, an input with a voice embedding among its identities is identified through long-term memory first (decision D194): each embedding is the person memory holds whose voice it matches (cosine at 0.85, margin 0.05, as relationships match), else a new person `person-<n>`, and the person's ref takes the embedding's place, so the relationship, the turns, the participants and the committed message key on the person; every voice heard is stored for its person. Without one an embedding is its own participant, its track `track-` and its unit vector. A memory failure there answers `bot.ability.conversation.failed` `{stage: \"memory\", message}` and commits nothing.")

S.case(220, 13, "the same voice twice is one person, across inputs.",
       [READY, heard("hi", VOICE_A), committed("hi", turn_ref="input-1:turn:person-1", session_ref=PERSON_1, extra=by("person-1")),
        heard("again", VOICE_A_AGAIN),
        committed("again", turn_ref="input-2:turn:person-1", session_ref=PERSON_1, extra=by("person-1"))],
       params=MEMORY, clock="wall")
S.case(221, 13, "a different voice is a new person.",
       [READY, heard("hi", VOICE_A), committed("hi", turn_ref="input-1:turn:person-1", session_ref=PERSON_1, extra=by("person-1")),
        heard("hello", VOICE_B),
        committed("hello", turn_ref="input-2:turn:person-2", session_ref=PERSON_1, extra=by("person-2"))],
       params=MEMORY, clock="wall")
S.case(222, 13, "without a database an embedding is its own participant.",
       [READY, heard("hi", VOICE_A),
        committed("hi", turn_ref="input-1:turn:track-[0.28,0.96]",
                  session_ref='relationship-{"identity_groups":[[],[[0.28,0.96]]]}',
                  extra=" && " + participants("_event.data.participants", []))])
S.case(223, 13, "a database the memory cannot open fails a voice at the memory stage, and commits nothing.",
       [READY, heard("hi", VOICE_A),
        expect(FAILED, "_event.data.stage === 'memory' && typeof _event.data.message === 'string' && _event.data.message.length > 0")]
       + snapshot([]),
       params=[memory("'/nowhere/floppy.sqlite'"), ("memory_timeout_ms", "600000")], clock="wall")
