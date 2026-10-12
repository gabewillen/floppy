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
READY = expect("bot.ability.offers", "_event.data.ability === 'conversation' && _event.data.choices.length === 0")
DB = "'/memory/floppy.sqlite'"
# Memory under a loaded host can outlast the original's 5 s bound: cases
# that reach the sql service wait longer (the bound itself loses every race to an in-process child: no case can make it fire).
MEMORY = [("database", DB), ("memory_timeout_ms", "600000")]

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


def committed(content, sequence=0, count=None, turn_ref="input-1:turn:alice", session_ref=AMBIENT, event=INPUT, extra=""):
    """The text handoff of an inbound commit."""
    count = sequence + 1 if count is None else count
    cond = ("_event.data.stimulus.event === 'bot.ability.conversation.output' && _event.data.stimulus.source === 'conversation'"
            f" && _event.data.stimulus.data.messages.length === {count}"
            f" && {last('sequence')} === {sequence} && {last('direction')} === 'inbound'"
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


def outbound(content, id_, source=None, target=None):
    provenance = {"event": "x.y", "id": id_}
    if source is not None:
        provenance["source"] = source
    if target is not None:
        provenance["target"] = target
    return send(APPEND, js({"message": {"sequence": 0, "direction": "outbound", "source_ids": ["bot"], "target_ids": ["alice"],
                                        "content": content, "content_type": "text/plain", "provenance": provenance}}))


def history(count, cond=""):
    return expect(OUTPUT, f"_event.data.messages.length === {count}" + (" && " + cond if cond else ""))


S.statement(1, "On start it sends `bot.ability.offers` `{ability: \"conversation\", choices: []}` to its parent: it offers nothing.")
S.statement(2, "A direct `bot.ability.conversation.input` (its parent the envelope `{event: \"bot.ability.conversation.input\", data}`) or a `bot.ability.conversation.routed_input` `{parent}` (the communication's envelope, whose data is the input) is committed as an inbound message (sequence the history's length, provenance event the parent's event); an inbound commit of text hands the parent `bot.ability.cognition.input` `{stimulus}` whose stimulus is `{event: \"bot.ability.conversation.output\", data: {parent, messages}, source: \"conversation\"}`.")
S.statement(3, "Each input is one turn per source identity (names sorted, then embeddings), each exchanged with its own turn detector; what is committed is the turns' product: their stripped texts joined by spaces, each distinct text once, as `text/plain`; with no text, null content for audio and the input's content otherwise. Its provenance's turn_ref is the first turn's, `input-<n>:turn:<track>`. A commit that is not text answers `bot.ability.conversation.output` `{parent, messages}` itself.")
S.statement(4, "Each input finds its relationship: targetless input shares the first targetless (ambient) relationship; other input takes the relationship whose source and target groups match, in either direction; with no match it begins its own, whose session_ref is `relationship-` and its context reference `{\"identity_groups\":[...]}`. The committed provenance's session_ref is the relationship's.")
S.statement(5, "`bot.ability.conversation.snapshot.request` `{request_ref}`, in any state, answers `bot.ability.conversation.snapshot.output` `{request_ref, detector_refs}`: `[session_ref, source]` of every participant (a source whose detector was ready), ordered by relationship and then identity.")
S.statement(6, "`bot.ability.conversation.append` `{message}` with an outbound message is committed with the next sequence and answered `bot.ability.conversation.output` `{parent, messages}`; one whose provenance id (non-empty), source and target match a committed message's is not committed again but is answered all the same; an inbound one is not taken (nothing is committed or answered).")
S.statement(7, "Input that arrives while one is in hand is held (`<event:defer/>`) and taken, in order, on the return to `waiting`.")
S.statement(8, "A turn the detector cannot close (`bot.ability.turn_detector.failed`) or cannot take (an `error.*`: a start its schema refuses) answers `bot.ability.conversation.failed` `{stage: \"turn_detector\", message}` and commits nothing.")
S.statement(9, "With a `database` param it recalls the relationship's memories before the turns and remembers the input after them through the memory ability, then commits as without one; a memory failure answers `bot.ability.conversation.failed` `{stage: \"memory\", message}` and commits nothing.")
S.statement(10, "An input its schema refuses is never routed here: the send raises `error.execution` in the sender, and nothing is committed.")

S.case(100, 1, "the conversation offers nothing on start.", [READY])

S.case(110, 2, "a direct text input is committed and handed to the parent as a cognition input.",
       [READY, say("hello"),
        committed("hello", extra=" && _event.data.stimulus.data.parent.data.content === 'hello'"
                  " && JSON.stringify(" + last("source_ids") + ") === '[\"alice\"]'")])
ROUTED_PARENT = {"event": "bot.ability.communication.input", "data": text_input("routed hi")}
S.case(111, 2, "a routed input is committed under its parent envelope's event.",
       [READY, send(ROUTED, js({"parent": ROUTED_PARENT})),
        committed("routed hi", event="bot.ability.communication.input",
                  extra=" && _event.data.stimulus.data.parent.data.content === 'routed hi'")])
S.case(112, 2, "successive inputs take the next sequences, the history in causal order.",
       [READY, say("one"), committed("one"), say("two"),
        committed("two", sequence=1, turn_ref="input-2:turn:alice",
                  extra=" && _event.data.stimulus.data.messages[0].content === 'one'")])

S.case(120, 3, "several sources: one turn each, in identity order, and the same text committed once.",
       [READY, say("  hi  ", sources=("zed", "alice")), committed("hi", session_ref=ALICE_ZED)])
S.case(121, 3, "audio content is committed with null content and answered as the conversation output.",
       [READY, send(INPUT, js({"source_ids": ["alice"], "target_ids": [], "content": "AAAA", "content_type": "Audio/PCM",
                               "sample_rate_hz": 16000, "channels": 1})),
        history(1, "_event.data.messages[0].content === null && _event.data.messages[0].content_type === 'Audio/PCM'"
                   " && _event.data.messages[0].direction === 'inbound' && _event.data.messages[0].provenance.turn_ref === 'input-1:turn:alice'"
                   " && _event.data.parent.event === 'bot.ability.conversation.input'")])
S.case(122, 3, "structured content is kept and answered as the conversation output; the first turn is the first name.",
       [READY, send(INPUT, js({"source_ids": [[2, 1.25], "bob"], "target_ids": ["bot"], "content": {"b": 1, "a": [True, None]},
                               "content_type": "application/json"})),
        history(1, "_event.data.messages[0].content.b === 1 && _event.data.messages[0].content.a[0] === true && _event.data.messages[0].content.a[1] === null"
                   " && _event.data.messages[0].provenance.turn_ref === 'input-1:turn:bob'")])

S.case(130, 4, "targetless input shares the ambient relationship; addressed input begins its own.",
       [READY, say("aside"), committed("aside"),
        say("to bot", targets=("bot",)), committed("to bot", sequence=1, turn_ref="input-2:turn:alice", session_ref=ALICE_BOT),
        send(INPUT, js(text_input("aside too", sources=("bob",)))),
        committed("aside too", sequence=2, turn_ref="input-3:turn:bob", session_ref=AMBIENT)])
S.case(131, 4, "the reply in the other direction belongs to the same relationship.",
       [READY, say("hi bot", targets=("bot",)), committed("hi bot", session_ref=ALICE_BOT),
        send(INPUT, js(text_input("hi alice", sources=("bot",), targets=("alice",)))),
        committed("hi alice", sequence=1, turn_ref="input-2:turn:bot", session_ref=ALICE_BOT)])
S.case(132, 4, "input addressed to an ambient participant joins the ambient relationship.",
       [READY, say("aside"), committed("aside"),
        send(INPUT, js(text_input("to alice", sources=("carol",), targets=("alice",)))),
        committed("to alice", sequence=1, turn_ref="input-2:turn:carol", session_ref=AMBIENT)])

S.case(140, 5, "before any input the snapshot holds no detectors.", [READY] + snapshot([]))
S.case(141, 5, "each ready source is a participant, ordered by relationship and then identity.",
       [READY, say("aside", sources=("zed", "alice")), committed("aside", session_ref=ALICE_ZED),
        say("to bot", targets=("bot",)), committed("to bot", sequence=1, turn_ref="input-2:turn:alice", session_ref=ALICE_BOT)]
       + snapshot([[ALICE_BOT, "alice"], [ALICE_ZED, "alice"], [ALICE_ZED, "zed"]]))
S.case(142, 5, "a snapshot is answered while an input is in hand.",
       [READY, say("hello"), send(SNAPSHOT, js({"request_ref": "busy"})),
        expect(SNAPSHOT_OUT, "_event.data.request_ref === 'busy' && Array.isArray(_event.data.detector_refs)"),
        committed("hello")])

S.case(150, 6, "an outbound append is committed with the next sequence and answered with the history.",
       [READY, say("hello"), committed("hello"), outbound("hi there", "r-1", source="q"),
        history(2, "_event.data.messages[1].direction === 'outbound' && _event.data.messages[1].sequence === 1"
                   " && _event.data.messages[1].content === 'hi there' && _event.data.messages[1].provenance.id === 'r-1'"
                   " && _event.data.messages[1].provenance.source === 'q'"
                   " && _event.data.parent.data.content === 'hello'")])
S.case(151, 6, "an append with a committed id, source and target is answered without being committed again; another source is committed.",
       [READY, outbound("once", "r-1", source="q"), history(1, "_event.data.messages[0].sequence === 0 && _event.data.parent === null"),
        outbound("once again", "r-1", source="q"), history(1),
        outbound("other", "r-1"), history(2, "_event.data.messages[1].content === 'other' && _event.data.messages[1].provenance.source === null")])
S.case(152, 6, "an inbound append is not taken: nothing is committed or answered.",
       [READY, send(APPEND, js({"message": {"sequence": 0, "direction": "inbound", "source_ids": ["alice"], "target_ids": [],
                                            "content": "x", "content_type": "text/plain", "provenance": {"event": "x.y", "id": "i-1"}}})),
        outbound("after", "r-2"), history(1, "_event.data.messages[0].content === 'after'")])

S.case(160, 7, "inputs sent together are committed one at a time, in order.",
       [READY, say("first"), say("second"), say("third"),
        committed("first"), committed("second", sequence=1, turn_ref="input-2:turn:alice"),
        committed("third", sequence=2, turn_ref="input-3:turn:alice")])

S.case(170, 8, "white space only: the detector closes the turn with nothing to show, and nothing is committed.",
       [READY, say("   "), failed(EMPTY_TURN), say("next"), committed("next", turn_ref="input-2:turn:alice")])
S.case(171, 8, "empty text: the detector's schema refuses the start, and nothing is committed.",
       [READY, say(""), failed("The turn detector could not take the turn (error.execution)"),
        say("next"), committed("next", turn_ref="input-2:turn:alice")])
S.case(172, 8, "a failed turn's source is no participant.",
       [READY, say("   "), failed(EMPTY_TURN)] + snapshot([]))

S.case(180, 9, "with a database the input is recalled, remembered and committed.",
       [READY, say("hello"), committed("hello"), say("again"), committed("again", sequence=1, turn_ref="input-2:turn:alice")],
       params=MEMORY, clock="wall")
S.case(181, 9, "a database the memory cannot open fails the input at the memory stage.",
       [READY, say("hello"), expect(FAILED, "_event.data.stage === 'memory' && typeof _event.data.message === 'string' && _event.data.message.length > 0")]
       + snapshot([]),
       params=[("database", "'/nowhere/floppy.sqlite'"), ("memory_timeout_ms", "600000")], clock="wall")
S.case(190, 10, "an input without source identities is refused at the send; the next is committed first.",
       [READY, send(INPUT, js({"source_ids": [], "target_ids": [], "content": "x", "content_type": "text/plain"})),
        expect("error.execution"), say("next"), committed("next")])
