# The reasoning's statements and cases (tests/irp/harness.py writes them).

S = suite
DONE = "done.invoke.reasoning"
CHAT = "/chat/completions"
IGNORE = "bot.ability.cognition.ignore"
RESPOND = "bot.ability.communication.respond"
FOCUS = "bot.focus_device"
CLEAR = "bot.clear_focus"
DB = ("database", "'/memory/floppy.sqlite'")

OBJECT = {"type": "object", "properties": {}, "additionalProperties": False}
TEXT = {"type": "object", "properties": {"text": {"type": "string", "minLength": 1}},
        "required": ["text"], "additionalProperties": False}
DEVICE = {"type": "object", "properties": {"device": {"type": "string", "minLength": 1}},
          "required": ["device"], "additionalProperties": False}
# The body's assembled choices: the cognition's and the communication's
# offers (tagged with their invoke ids) and the body's own focus events.
CHOICES = [
    {"event": IGNORE, "schema": OBJECT, "description": "Ignore the stimulus.", "target": "cognition"},
    {"event": RESPOND, "schema": TEXT, "description": "Say something.", "target": "communication"},
    {"event": FOCUS, "schema": DEVICE, "description": "Look at a device."},
    {"event": CLEAR, "schema": OBJECT, "description": "Stop looking at the device."},
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


SAY = {"event": RESPOND, "data": {"text": "Hi! I am floppy."}, "reason": "greet", "target": "communication"}
SKIP = {"event": IGNORE, "data": {}, "reason": "nothing to say", "target": "cognition"}


S.statement(1, "It runs one turn (the `<param>`s turn, stimulus, focus, focus_candidates, choices, model and database) and ends: its `<donedata>`, received as `done.invoke.reasoning`, is `{turn, handled: true, selected}` or `{turn, handled: false, failed: true, reason}`, turn the given turn.")
S.statement(2, "With no choices it fails the turn without a model call: reason `the reasoning has no choices to select from`.")
S.statement(3, "With choices it makes one structured generate call whose strict schema allows only the offered events (with their targets); the reply's selections, once checked (see 6), are returned as the model gave them with `handled: true`, each `{event, data, reason, target?}`, `bot.ability.cognition.ignore` selections included (the body routes them; dispatching only checks); an empty selection is handled with nothing selected.")
S.statement(4, "A generate call that fails fails the turn with reason `the reasoning failed (<code>)`, the generate service's failure code: `status` for an error status, `schema-mismatch` for a selection outside the choices, `truncated` for a reply cut short, `malformed` for an answer that is no chat completion.")
S.statement(5, "A prompt over the original's projection budgets (262144 encoded bytes, 4096 nodes, 4096 collection items, 65536 characters per scalar) fails the turn without a model call, its reason the original's message (for example `prompt projection collection item budget exceeded`).")
S.statement(6, "Selections the attention policy refuses fail the turn before anything is retained, with the original's message: a `bot.focus_device` outside the focus candidates, or a `bot.clear_focus` with no focused device; a focus selection within the policy is returned.")
S.statement(7, "With a database it recalls prior episodes and standing directives before the model call and writes nothing (reflection stores the turn's episode, decision D96), and the turn is handled as without one; a memory that fails fails the turn with reason `Reasoning memory recall failed: ...` without a model call.")

S.case(100, 1, "a reasoned turn ends with its turn number, handled.",
       [expect(DONE, "_event.data.turn === 7 && _event.data.handled === true && _event.data.selected.length === 1")],
       params=turn(number=7), backend=selection(SAY))
S.case(101, 1, "a failed turn ends with its turn number and reason.",
       [failed("the reasoning has no choices to select from", 3)],
       params=turn(choices=[], number=3))

S.case(110, 2, "no choices fail the turn without a model call (the backend would answer).",
       [failed("the reasoning has no choices to select from")],
       params=turn(choices=[]), backend=selection(SAY))

S.case(120, 3, "a selection of an offered event is returned with its data, reason and target.",
       [expect(DONE, "_event.data.handled === true && _event.data.selected.length === 1"
                     " && _event.data.selected[0].event === 'bot.ability.communication.respond'"
                     " && _event.data.selected[0].data.text === 'Hi! I am floppy.'"
                     " && _event.data.selected[0].reason === 'greet'"
                     " && _event.data.selected[0].target === 'communication'")],
       params=turn(), backend=selection(SAY))
S.case(121, 3, "an ignore selection is returned with the others, in the model's order.",
       [expect(DONE, "_event.data.handled === true && _event.data.selected.length === 2"
                     " && _event.data.selected[0].event === 'bot.ability.cognition.ignore'"
                     " && _event.data.selected[0].target === 'cognition'"
                     " && _event.data.selected[1].event === 'bot.ability.communication.respond'")],
       params=turn(), backend=selection(SKIP, SAY))
S.case(122, 3, "an empty selection is handled with nothing selected.",
       [expect(DONE, "_event.data.handled === true && Array.isArray(_event.data.selected) && _event.data.selected.length === 0")],
       params=turn(), backend=selection())

S.case(130, 4, "a model that answers 500 fails the turn as status.",
       [failed("the reasoning failed (status)")],
       params=turn(), backend=[{"path": CHAT, "status": 500, "body": {"error": {"message": "boom"}}}])
S.case(131, 4, "a selection of an event outside the choices fails the turn as schema-mismatch.",
       [failed("the reasoning failed (schema-mismatch)")],
       params=turn(), backend=selection({"event": "bot.ability.identity.adopt", "data": {"name": "Pip"}, "reason": "x",
                                         "target": "identity"}))
S.case(132, 4, "a selection of an offered event on a target that does not offer it fails as schema-mismatch.",
       [failed("the reasoning failed (schema-mismatch)")],
       params=turn(), backend=selection({**SAY, "target": "cognition"}))
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
S.case(150, 6, "a focus_device outside the focus candidates fails the turn with the original's message.",
       [failed("Processing selected focus_device outside available device candidates.")],
       params=turn(candidates=["desk"]), backend=selection({"event": FOCUS, "data": {"device": "lamp"}, "reason": "look"}))
S.case(151, 6, "a clear_focus with no focused device fails the turn with the original's message.",
       [failed("Processing selected clear_focus with no focused device.")],
       params=turn(), backend=selection({"event": CLEAR, "data": {}, "reason": "stop"}))
S.case(152, 6, "a focus_device within the candidates is returned.",
       [expect(DONE, "_event.data.handled === true && _event.data.selected.length === 1"
                     " && _event.data.selected[0].event === 'bot.focus_device'"
                     " && _event.data.selected[0].data.device === 'lamp'")],
       params=turn(candidates=["desk", "lamp"]), backend=selection({"event": FOCUS, "data": {"device": "lamp"}, "reason": "look"}))

S.case(160, 7, "with a fresh database the turn recalls and reasons, handled.",
       [expect(DONE, "_event.data.turn === 1 && _event.data.handled === true && _event.data.selected.length === 1"
                     " && _event.data.selected[0].data.text === 'Hi! I am floppy.'")],
       params=turn(extra=[DB]), backend=selection(SAY))
S.case(161, 7, "with a database and no choices, the recall still runs and the turn fails for its choices.",
       [failed("the reasoning has no choices to select from")],
       params=turn(choices=[], extra=[DB]))
S.case(162, 7, "a database that cannot be opened fails the recall without a model call.",
       [failed_starting("Reasoning memory recall failed: ")],
       params=turn(extra=[("database", "'/nowhere/floppy.sqlite'")]), backend=selection(SAY))

