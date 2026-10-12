# The candidates runner's statements and cases (tests/irp/harness.py writes them).

S = suite
DONE = "done.invoke.candidates_runner"
DB = "'/memory/floppy.sqlite'"
IGNORE = "bot.ability.cognition.ignore"
SAY_CHOICES = [{"event": "bot.say", "schema": {}, "target": "speaking"}]


def behavior(donedata):
    """A turn behavior's source: it takes the <param>s turn and input and
    answers by its donedata."""
    return ('<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="done">'
            '<datamodel><data id="turn" expr="0"/><data id="input" expr="null"/></datamodel>'
            f'<final id="done"><donedata><content expr="{donedata}"/></donedata></final></scxml>')


SILENT = '<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript"><state id="waiting"/></scxml>'
BROKEN = "<scxml"
DECLINES = behavior("({turn: turn, handled: false})")
HANDLES = behavior(f"({{turn: turn, handled: true, selected: [{{event: '{IGNORE}', data: {{}}, reason: 'handled'}}]}})")
INPUT = {"text": "hello", "focus": None, "focus_candidates": []}


def run(candidates, choices=(), input=INPUT, turn=3, database=DB, silence_ms=None, memory_timeout_ms=None):
    params = [("turn", js(turn)), ("input", js(input)),
              ("candidates", js([{"name": name, "source": source} for name, source in candidates])),
              ("choices", js(list(choices))), ("database", database)]
    if silence_ms is not None:
        params.append(("silence_ms", js(silence_ms)))
    if memory_timeout_ms is not None:
        params.append(("memory_timeout_ms", js(memory_timeout_ms)))
    return params


def answer(tried, handled, rejection=None, extra=None):
    cond = (f"_event.data.tried.join(' ') === {js(' '.join(tried))} && _event.data.tried.length === {len(tried)}"
            f" && _event.data.handled === {js(handled)} && _event.data.rejection === {js(rejection)}")
    if not handled:
        cond += " && _event.data.selected === null"
    if extra:
        cond += " && " + extra
    return expect(DONE, cond)


HANDLED_SELECTED = f"_event.data.selected.length === 1 && _event.data.selected[0].event === '{IGNORE}' && _event.data.selected[0].reason === 'handled'"

S.statement(1, "It runs the first of its `candidates` <param> (`[{name, source}]`) as the child `candidate` from its source, with the <param>s `turn` and `input`. A candidate that answers by its donedata `{turn, handled: true, selected}` for this turn with at least one selection, all accepted, has its use persisted, and the runner ends with donedata `{tried: [name], handled: true, selected, rejection: null}`, `selected` the candidate's own selections; it sends its parent nothing else.")
S.statement(2, "The handled candidate's selections are checked before its use is persisted: `<bot:verdict>` drops `bot.ability.cognition.ignore` selections and applies the attention policy with `input.focus` and `input.focus_candidates`, then `<decision:route>` places each remaining one on the target offering it in the `choices` <param>.")
S.statement(3, "An answer that does not handle this turn (`handled` not `true`, no selections, or another turn) is not handled: with no candidates left, the runner ends with `{tried: [name], handled: false, selected: null, rejection: null}`.")
S.statement(4, "When its candidate does not handle the turn and candidates are left, the runner invokes itself for the rest (with the same turn, input, choices, database and bounds) and ends with the rest's donedata, its own candidate first in `tried`.")
S.statement(5, "A candidate that does not answer within `silence_ms` is passed over, and the rest run.")
S.statement(6, "A candidate whose source cannot run has its failure persisted, and the rest run.")
S.statement(7, "A handled candidate whose selection the verdict or the route rejects has its failure persisted, and the runner ends with `{tried: [name], handled: false, selected: null, rejection}`, the rejection's message; the rest do not run.")
S.statement(8, "Each persistence is one transaction of the Memory ability on the `database` <param>; one that fails or outlasts `memory_timeout_ms` takes the same effect as one persisted.")

S.case(100, 1, "a handled candidate answers the turn with its own selections.",
       [answer(["A"], True, extra=HANDLED_SELECTED)], params=run([("A", HANDLES)]))
S.case(101, 1, "the candidate gets the turn and input <param>s.",
       [answer(["Echo"], True, extra="_event.data.selected[0].data.turn === 7 && _event.data.selected[0].data.text === 'hello'")],
       params=run([("Echo", behavior(f"({{turn: turn, handled: true, selected: [{{event: '{IGNORE}', data: {{turn: turn, text: input.text}}}}]}})"))], turn=7))

S.case(110, 2, "a selection the turn offers is placed, and the candidate answers.",
       [answer(["Say"], True, extra="_event.data.selected.length === 1 && _event.data.selected[0].event === 'bot.say' && _event.data.selected[0].data.text === 'hi'")],
       params=run([("Say", behavior("({turn: turn, handled: true, selected: [{event: 'bot.say', data: {text: 'hi'}}]})"))], SAY_CHOICES))
S.case(111, 2, "a focus_device among input.focus_candidates passes the verdict.",
       [answer(["Focus"], True, extra="_event.data.selected[0].event === 'bot.focus_device' && _event.data.selected[0].data.device === 'kitchen'")],
       params=run([("Focus", behavior("({turn: turn, handled: true, selected: [{event: 'bot.focus_device', data: {device: 'kitchen'}}]})"))],
                  [{"event": "bot.focus_device", "schema": {}}],
                  input={"focus": None, "focus_candidates": ["kitchen"]}))

S.case(120, 3, "handled false is not handled.",
       [answer(["Declines"], False)], params=run([("Declines", DECLINES)]))
S.case(121, 3, "handled with no selections is not handled.",
       [answer(["Empty"], False)], params=run([("Empty", behavior("({turn: turn, handled: true, selected: []})"))]))
S.case(122, 3, "an answer for another turn is not handled.",
       [answer(["Stale"], False)],
       params=run([("Stale", behavior(f"({{turn: turn + 1, handled: true, selected: [{{event: '{IGNORE}', data: {{}}}}]}})"))]))

S.case(130, 4, "the rest run after a declining candidate, and the first that handles answers.",
       [answer(["A", "B"], True, extra=HANDLED_SELECTED)], params=run([("A", DECLINES), ("B", HANDLES)]))
S.case(131, 4, "every candidate declining leaves the turn unhandled, each named in order.",
       [answer(["A", "B", "C"], False)], params=run([("A", DECLINES), ("B", DECLINES), ("C", DECLINES)]))
S.case(132, 4, "a candidate after the one that handles never runs.",
       [answer(["A"], True, extra=HANDLED_SELECTED)], params=run([("A", HANDLES), ("B", BROKEN)]))

S.case(140, 5, "a silent candidate is passed over after silence_ms.",
       [answer(["Silent", "B"], True, extra=HANDLED_SELECTED)],
       params=run([("Silent", SILENT), ("B", HANDLES)], silence_ms=200), clock="wall")
S.case(141, 5, "a silent last candidate leaves the turn unhandled.",
       [answer(["Silent"], False)], params=run([("Silent", SILENT)], silence_ms=200), clock="wall")

S.case(150, 6, "a source that cannot run is passed over, and the rest run.",
       [answer(["Broken", "B"], True, extra=HANDLED_SELECTED)], params=run([("Broken", BROKEN), ("B", HANDLES)]))
S.case(151, 6, "a last source that cannot run leaves the turn unhandled.",
       [answer(["Broken"], False)], params=run([("Broken", BROKEN)]))

S.case(160, 7, "a selection the turn does not offer is rejected, and the rest do not run.",
       [answer(["Unoffered"], False, "Processing selected unavailable event: bot.ability.reasoning.input.")],
       params=run([("Unoffered", behavior("({turn: turn, handled: true, selected: [{event: 'bot.ability.reasoning.input', data: {}}]})")),
                   ("B", HANDLES)], SAY_CHOICES))
S.case(161, 7, "a focus_device outside input.focus_candidates is rejected by the verdict.",
       [answer(["Focus"], False, "Processing selected focus_device outside available device candidates.")],
       params=run([("Focus", behavior("({turn: turn, handled: true, selected: [{event: 'bot.focus_device', data: {device: 'garage'}}]})"))],
                  [{"event": "bot.focus_device", "schema": {}}],
                  input={"focus": None, "focus_candidates": ["kitchen"]}))

S.case(170, 8, "a use whose persistence fails still answers the turn.",
       [answer(["A"], True, extra=HANDLED_SELECTED)], params=run([("A", HANDLES)], database="'/elsewhere/floppy.sqlite'"))
S.case(171, 8, "a use whose persistence outlasts memory_timeout_ms still answers the turn.",
       [answer(["A"], True, extra=HANDLED_SELECTED)], params=run([("A", HANDLES)], memory_timeout_ms=0))
S.case(172, 8, "a failure whose persistence fails still lets the rest run.",
       [answer(["Broken", "B"], True, extra=HANDLED_SELECTED)],
       params=run([("Broken", BROKEN), ("B", HANDLES)], database="'/elsewhere/floppy.sqlite'"))
S.case(173, 8, "with no database, a rejection is still answered.",
       [answer(["Unoffered"], False, "Processing selected unavailable event: bot.ability.reasoning.input.")],
       params=run([("Unoffered", behavior("({turn: turn, handled: true, selected: [{event: 'bot.ability.reasoning.input', data: {}}]})"))],
                  SAY_CHOICES, database="null"))
