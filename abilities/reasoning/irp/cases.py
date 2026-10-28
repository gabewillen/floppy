# The reasoning's statements and cases (tests/irp/harness.py writes them).

S = suite
INPUT = "bot.ability.processing.input"
DONE = "bot.ability.processing.output"
CHAT = "/chat/completions"
IGNORE = "bot.ability.cognition.ignore"
RESPOND = "bot.ability.communication.respond"
FOCUS = "bot.focus_device"
LAMP = "lamp.on"
DB = memory("'/memory/floppy.sqlite'")

OBJECT = {"type": "object", "properties": {}, "additionalProperties": False}
# Respond's data is its parts, each with optional words (the reply schema's shape).
PARTS = {"type": "object", "properties": {"parts": {
             "type": "array", "minItems": 1, "maxItems": 64,
             "items": {"type": "object", "properties": {"words": {"type": "string", "minLength": 1}},
                       "additionalProperties": False}}},
         "required": ["parts"], "additionalProperties": False}
DEVICE = {"type": "object", "properties": {"device": {"type": "string", "minLength": 1}},
          "required": ["device"], "additionalProperties": False}
# The harness (the run's root session, "session") offers respond and the
# focus events, so each one the stage sends comes to it; the ignore is
# tagged with the stage's own session id, standing in for the cognition (it
# takes no ignore, so the ignore is dropped).
HARNESS = "session"
OWN = "session.reasoning"
CHOICES = [
    {"event": IGNORE, "schema": OBJECT, "description": "Ignore the stimulus.", "target": OWN},
    {"event": RESPOND, "schema": PARTS, "description": "Say something.", "target": HARNESS},
    {"event": FOCUS, "schema": DEVICE, "description": "Look at a device.", "target": HARNESS},
    {"event": LAMP, "schema": OBJECT, "description": "Turn the lamp on.", "target": HARNESS},
]
STIMULUS = {"event": "bot.input", "data": {"observation": {"event": "text", "data": "say hi"}}, "source": "gabe"}


def turn(choices=CHOICES, stimulus=STIMULUS, number=1, focus=None, candidates=(), extra=()):
    return [("turn", js(number)), ("stimulus", js(stimulus)), ("focus", js(focus)),
            ("focus_candidates", js(list(candidates))), ("choices", js(choices)), ("model", "'mock-model'"),
            *extra]


def completion(content, finish="stop"):
    return {"object": "chat.completion",
            "choices": [{"index": 0, "finish_reason": finish, "message": {"role": "assistant", "content": content}}]}


def selection(*events):
    return [{"path": CHAT, "status": 200, "body": completion(js({"events": list(events)}))}]


def failed(reason, number=1):
    return expect(DONE, f"_event.data.turn === {number} && _event.data.handled === false"
                        f" && _event.data.failed === true && _event.data.reason === {js(reason)}")


def failed_starting(prefix):
    return expect(DONE, "_event.data.turn === 1 && _event.data.handled === false && _event.data.failed === true"
                        f" && _event.data.reason.indexOf({js(prefix)}) === 0 && _event.data.reason.length > {len(prefix)}")


SAY = {"event": RESPOND, "data": {"parts": [{"words": "Hi! I am floppy."}]}, "reason": "greet", "target": HARNESS}
SKIP = {"event": IGNORE, "data": {}, "reason": "nothing to say", "target": OWN}
# The harness takes the respond the stage sends it.
SAID = expect(RESPOND, "_event.data.parts.length === 1 && _event.data.parts[0].words === 'Hi! I am floppy.'")


S.statement(1, "Each `bot.ability.processing.input` (turn, stimulus, focus, focus_candidates, choices, held, environment, self, acted, recent, max_context_tokens, completion_reserve_tokens and bytes_per_token; its `<param>`s are model, tier, memory and embedding) is one turn, answered by one `bot.ability.processing.output` to the parent: `{turn, handled: true, selected}` or `{turn, handled: false, failed: true, reason}`, turn the given turn.")
S.statement(2, "With no choices it fails the turn without a model call: reason `the reasoning has no choices to select from`.")
S.statement(3, "With choices it makes one generate call offering the dispatch tool, which allows only the offered events (with their targets); the events of its dispatch calls, in order, are returned as the model gave them with `handled: true`, each `{event, data, reason, target}`, `bot.ability.cognition.ignore` selections included; an empty selection is handled with nothing selected.")
S.statement(4, "A generate call that fails fails the turn with reason `the reasoning failed (<code>)`, the generate service's failure code: `status` for an error status, `truncated` for a reply cut short, `malformed` for an answer that is no chat completion. A selection the dispatch tool refuses (`schema-mismatch`, or `not-json`) is repaired first (decision D142): the call is made again with the rejection as the prompt's feedback, at most twice, and the same rejection twice running, or one once the repairs are spent, fails the turn with reason `Selection rejected: <message>` (the rejection's message, else its code).")
S.statement(5, "A prompt over the projection budgets (49152 encoded bytes, 4096 nodes, 4096 collection items, 65536 characters per scalar) fails the turn without a model call, its reason the budget's message (for example `prompt projection collection item budget exceeded`); recalled turns over the bytes the stimulus leaves are trimmed to the newest that fit, and the turn is reasoned.")
S.statement(6, "Each selection is sent, in order, to the session its target names (`#_scxml_` + target) before the turn ends; a selection without its data is refused by the dispatch tool, whose every branch requires it (decision D231, where D113 sent it with `{}`); a send that fails fails the turn with reason `the reasoning could not run (<error>)`.")
S.statement(7, "With a database it recalls prior episodes, standing directives and the bot's active learned behaviors (named in the system channel's self block, decision D154) before the model call and writes nothing (reflection stores the turn's episode, decision D97), and the turn is handled as without one; a memory that fails fails the turn with reason `Reasoning memory recall failed: ...` without a model call.")

S.statement(8, "Its model call streams its reply when the run's `FLOPPY_STREAM` is `true`, and each `generate.progress` the generate service returns while the reply arrives is sent on to the parent unchanged, so the cognition can bound the stage by inactivity (decision D101).")

S.statement(9, "As the intuition tier (the `<param>` tier `intuition`, decision D144) it may select the deliberative handoff, `bot.ability.reasoning.input`, among its choices: the handoff is never sent, the other selections are sent in order, and it ends `{turn, handled: false, reason: \"handoff\", acted}`, acted what was sent; a selection without the handoff is handled as reasoning's is, and its failures name the tier (`the intuition failed (<code>)`).")

# The <param>s the cognition invokes the stage with; every other name is
# the turn's, sent as its bot.ability.processing.input (the cognition's
# defaults where a case names none).
STATIC = ("model", "tier", "memory")
TURN_DEFAULTS = [("held", "[]"), ("environment", "null"), ("self", "null"), ("acted", "[]"), ("recent", "[]"),
                 ("max_context_tokens", "null"), ("completion_reserve_tokens", "0"), ("bytes_per_token", "3")]


def as_input(params):
    """The stage's <param>s of `params`, and the input step of the rest."""
    given = dict((name, expr) for name, expr in params if name not in STATIC)
    for name, expr in TURN_DEFAULTS:
        given.setdefault(name, expr)
    return ([p for p in params if p[0] in STATIC],
            send(INPUT, "({" + ", ".join(f"{name}: {expr}" for name, expr in given.items()) + "})"))


_original_case = S.case


def _turn_case(test_id, number, comment, steps, params=(), **kwargs):
    static, step = as_input(params)
    return _original_case(test_id, number, comment, [step] + list(steps), params=static, **kwargs)


# Every other case drops the streamed reply's progress (statement 8).
_case = _turn_case


def _case_dropping_progress(*args, ignore=(), **kwargs):
    return _case(*args, ignore=tuple(ignore) + ("generate.progress",), **kwargs)


S.case = _case_dropping_progress

S.case(100, 1, "a reasoned turn ends with its turn number, handled.",
       [SAID, expect(DONE, "_event.data.turn === 7 && _event.data.handled === true && _event.data.selected.length === 1")],
       params=turn(number=7), backend=selection(SAY))
S.case(101, 1, "a failed turn ends with its turn number and reason.",
       [failed("the reasoning has no choices to select from", 3)],
       params=turn(choices=[], number=3))

S.case(110, 2, "no choices fail the turn without a model call (the backend would answer).",
       [failed("the reasoning has no choices to select from")],
       params=turn(choices=[]), backend=selection(SAY))

S.case(120, 3, "a selection of an offered event is returned with its data, reason and target.",
       [SAID, expect(DONE, "_event.data.handled === true && _event.data.selected.length === 1"
                     " && _event.data.selected[0].event === 'bot.ability.communication.respond'"
                     " && _event.data.selected[0].data.parts[0].words === 'Hi! I am floppy.'"
                     " && _event.data.selected[0].reason === 'greet'"
                     " && _event.data.selected[0].target === 'session'")],
       params=turn(), backend=selection(SAY))
S.case(121, 3, "an ignore selection is returned with the others, in the model's order.",
       [SAID, expect(DONE, "_event.data.handled === true && _event.data.selected.length === 2"
                     " && _event.data.selected[0].event === 'bot.ability.cognition.ignore'"
                     " && _event.data.selected[0].target === 'session.reasoning'"
                     " && _event.data.selected[1].event === 'bot.ability.communication.respond'")],
       params=turn(), backend=selection(SKIP, SAY))
S.case(122, 3, "an empty selection is handled with nothing selected.",
       [expect(DONE, "_event.data.handled === true && Array.isArray(_event.data.selected) && _event.data.selected.length === 0")],
       params=turn(), backend=selection())

S.case(130, 4, "a model that answers 500 fails the turn as status.",
       [failed("the reasoning failed (status)")],
       params=turn(), backend=[{"path": CHAT, "status": 500, "body": {"error": {"message": "boom"}}}])
OFF_EVENT = {"event": "bot.ability.identity.adopt", "data": {"name": "Pip"}, "reason": "x", "target": HARNESS}
S.case(131, 4, "a selection of an event outside the choices is repaired once, and the same rejection again fails the turn.",
       [failed("Selection rejected: /events/0/event: value must be one of \"bot.ability.cognition.ignore\", \"bot.ability.communication.respond\", \"bot.focus_device\", \"lamp.on\"")],
       params=turn(), backend=selection(OFF_EVENT) + selection(OFF_EVENT))
S.case(132, 4, "a selection of an offered event on a target that does not offer it is repaired once, then fails the same way.",
       [failed("Selection rejected: /events/0: anyOf failed")],
       params=turn(), backend=selection({**SAY, "target": OWN}) + selection({**SAY, "target": OWN}))
S.case(135, 4, "a rejected selection repaired by the next call is handled: the repair's selection is sent and returned.",
       [SAID, expect(DONE, "_event.data.handled === true && _event.data.selected.length === 1"
                     " && _event.data.selected[0].event === 'bot.ability.communication.respond'")],
       params=turn(), backend=selection(OFF_EVENT) + selection(SAY))
S.case(133, 4, "a reply cut short fails the turn as truncated.",
       [failed("the reasoning failed (truncated)")],
       params=turn(), backend=[{"path": CHAT, "status": 200, "body": completion("{\"events\": [", "length")}])
S.case(134, 4, "an answer that is no chat completion fails the turn as malformed.",
       [failed("the reasoning failed (malformed)")],
       params=turn(), backend=[{"path": CHAT, "status": 200, "body": {"nothing": True}}])

S.case(140, 5, "a stimulus with over 4096 collection items fails the turn without a model call.",
       [failed("prompt projection collection item budget exceeded")],
       params=[p for p in turn() if p[0] != "stimulus"]
       + [("stimulus", "({event: 'bot.input', source: 'gabe', data: {items: Array.from({length: 4097}, (_, i) => i)}})")],
       backend=selection(SAY))
# A stimulus whose items render to about 37 KiB (from about 6 KiB of
# input) and turns of 30 KiB: the prompt would pass a generate call's
# 64 KiB, though the input is under the event bound.
LONG_STIMULUS = ("stimulus", "({event: 'bot.input', source: 'gabe', data: {items: Array.from({length: 1600}, () => 'a')}})")
LONG_TURNS = ("recent", "Array.from({length: 4}, (_, i) => ({at: '2026-07-07T12:00:00Z', direction: 'heard',"
                        " text: 'turn-' + i + '-' + 'w'.repeat(7500)}))")
S.case(141, 5, "recalled turns that would carry the prompt past a generate call's bound are trimmed, and the turn is reasoned.",
       [SAID, expect(DONE, "_event.data.handled === true && _event.data.selected.length === 1")],
       params=[p for p in turn() if p[0] != "stimulus"] + [LONG_STIMULUS, LONG_TURNS], backend=selection(SAY))
S.case(150, 6, "a focus_device and a device action are sent to their target, in order, before the turn ends.",
       [expect(FOCUS, "_event.data.device === 'lamp'"), expect(LAMP),
        expect(DONE, "_event.data.handled === true && _event.data.selected.length === 2"
                     " && _event.data.selected[0].event === 'bot.focus_device'"
                     " && _event.data.selected[1].event === 'lamp.on'")],
       params=turn(), backend=selection({"event": FOCUS, "data": {"device": "lamp"}, "reason": "look", "target": HARNESS},
                                        {"event": LAMP, "data": {}, "reason": "light", "target": HARNESS}))
S.case(152, 6, "a selection without data is refused, every branch requiring its data.",
       [failed("Selection rejected: /events/0: missing properties 'data'")],
       params=turn(), backend=selection({"event": LAMP, "reason": "light", "target": HARNESS}))
S.case(151, 6, "a selection whose target no session of the run answers fails the turn.",
       [failed("the reasoning could not run (error.communication)")],
       params=turn(choices=[{**CHOICES[1], "target": "session.nowhere"}]),
       backend=selection({**SAY, "target": "session.nowhere"}))

S.case(160, 7, "with a fresh database the turn recalls and reasons, handled.",
       [SAID, expect(DONE, "_event.data.turn === 1 && _event.data.handled === true && _event.data.selected.length === 1"
                     " && _event.data.selected[0].data.parts[0].words === 'Hi! I am floppy.'")],
       params=turn(extra=[DB]), backend=selection(SAY))
S.case(161, 7, "with a database and no choices, the recall still runs and the turn fails for its choices.",
       [failed("the reasoning has no choices to select from")],
       params=turn(choices=[], extra=[DB]))
S.case(162, 7, "a database that cannot be opened fails the recall without a model call.",
       [failed_starting("Reasoning memory recall failed: ")],
       params=turn(extra=[memory("'/nowhere/floppy.sqlite'")]), backend=selection(SAY))

_case(170, 8, "a streamed reply's progress reaches the parent before the turn ends, handled.",
      [expect("generate.progress", "_event.data.received > 0"), SAID,
       expect(DONE, "_event.data.turn === 1 && _event.data.handled === true && _event.data.selected.length === 1")],
      params=turn(), backend=selection(SAY), stream=True, ignore=("generate.progress",))

HANDOFF = "bot.ability.reasoning.input"
# The cognition offers the handoff to intuition as its own (here, the
# stage's own session stands in for it, as for the ignore).
HAND_ON = {"event": HANDOFF, "data": {}, "reason": "this needs thought", "target": OWN}
INTUITION_CHOICES = CHOICES + [{"event": HANDOFF, "schema": OBJECT, "description": "Hand this turn to reasoning.",
                                "target": OWN}]
INTUITION = ("tier", "'intuition'")
S.case(180, 9, "intuition selecting the handoff sends its other selections and hands the turn on with them as acted.",
       [SAID, expect(DONE, "_event.data.turn === 1 && _event.data.handled === false && _event.data.reason === 'handoff'"
                     " && _event.data.failed === undefined && _event.data.acted.length === 1"
                     " && _event.data.acted[0].event === 'bot.ability.communication.respond'")],
       params=turn(choices=INTUITION_CHOICES, extra=[INTUITION]), backend=selection(SAY, HAND_ON))
S.case(181, 9, "intuition selecting no handoff handles the turn with its selections.",
       [SAID, expect(DONE, "_event.data.handled === true && _event.data.selected.length === 1")],
       params=turn(choices=INTUITION_CHOICES, extra=[INTUITION]), backend=selection(SAY))
S.case(182, 9, "an intuition whose model answers 500 fails as the intuition.",
       [failed("the intuition failed (status)")],
       params=turn(choices=INTUITION_CHOICES, extra=[INTUITION]),
       backend=[{"path": CHAT, "status": 500, "body": {"error": {"message": "boom"}}}])

S.statement(10, "It is long-lived (mosfet.py's Cognition holds its Reasoning and Intuition): it answers each input in turn with nothing of the turn before, and a `bot.ability.processing.cancel` ends the turn in hand unanswered, so the next input is the next turn.")
S.case(190, 10, "two turns are answered in order by one session.",
       [SAID, expect(DONE, "_event.data.turn === 1 && _event.data.handled === true && _event.data.selected.length === 1"),
        as_input(turn(number=2))[1], SAID,
        expect(DONE, "_event.data.turn === 2 && _event.data.handled === true && _event.data.selected.length === 1")],
       params=turn(), backend=selection(SAY) + selection(SAY))
S.case(191, 10, "a cancelled turn is never answered; the next turn is.",
       [send("bot.ability.processing.cancel", js({})), as_input(turn(number=2))[1], SAID,
        expect(DONE, "_event.data.turn === 2 && _event.data.handled === true && _event.data.selected.length === 1")],
       params=turn(), backend=selection(SAY) + selection(SAY))
