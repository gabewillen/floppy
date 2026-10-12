# The intuition's statements and cases (tests/irp/harness.py writes them).

import json

S = suite
DONE = "done.invoke.intuition"
SYSTEM_ONE = "/v1/systemone"
ABSTAIN = "__unhandled__"
IGNORE = "bot.ability.cognition.ignore"
HANDOFF = "bot.ability.reasoning.input"
SAY = {"event": "bot.say", "schema": {}, "target": "speaking"}
IGNORE_CHOICE = {"event": IGNORE, "schema": {}, "target": "cognition"}
HANDOFF_CHOICE = {"event": HANDOFF, "schema": {}, "target": "reasoning"}
# The decision bound for every case but the timeout's: the decision service
# starts slowly on a loaded machine, and its own default (20 s) can pass first.
SLOW = 150000
STIMULUS = {"event": "bot.input", "data": {"observation": {"event": "text", "data": "hello"}}, "source": "body"}


def decided(probabilities, members=None):
    """The system_one answer: the selection question's distribution, and
    each member question <event>::<member> answering its one label."""
    chosen = max(probabilities, key=probabilities.get)
    answers = {"selection": {"type": "choice", "choice": chosen, "confidence": probabilities[chosen],
                             "probabilities": probabilities}}
    for name, label in (members or {}).items():
        answers[name] = {"type": "choice", "choice": label, "confidence": 1.0,
                                             "probabilities": {label: 1.0}}
    return [{"path": SYSTEM_ONE, "status": 200,
             "body": {"model": "fake", "answers": answers, "usage": {"input_tokens": 1, "output_tokens": 1}}}]


def run(choices, turn=1, stimulus=STIMULUS, tuner=None, focus_candidates=None, timeout_ms=SLOW):
    params = [("turn", js(turn)), ("stimulus", js(stimulus)), ("focus", "null"), ("choices", js(choices))]
    if focus_candidates is not None:
        params.append(("focus_candidates", js(focus_candidates)))
    if tuner is not None:
        params.append(("tuner", js(tuner)))
    params.append(("decision_timeout_ms", js(timeout_ms)))
    return params


def done(turn, handled, reason, extra=None):
    cond = (f"_event.data.turn === {turn} && _event.data.handled === {js(handled)}"
            f" && _event.data.reason === {js(reason)} && typeof _event.data.tuner === 'object'")
    if extra:
        cond += " && " + extra
    return expect(DONE, cond)


def selected(*items):
    return f"JSON.stringify(_event.data.selected) === {js(json.dumps([{'event': e, 'data': d} for e, d in items], sort_keys=True, separators=(',', ':')))}"


def failed(turn, reason, extra="true"):
    return done(turn, False, reason, f"_event.data.failed === true && {extra}")


def tuner(n):
    return f"_event.data.tuner.n === {n}"


S.statement(1, "It asks the decision service (kind `select`) over the `choices` <param> less `bot.ability.reasoning.input`, with the stimulus's object data as evidence: each event's required data is filled from it. A confident product is handled: it ends with donedata `{turn, handled: true, selected: [{event, data}], reason: \"none\", tuner}`, every selection the service returned; its donedata is its only answer.")
S.statement(2, "The turn's confidence is the minimum of its selections' probabilities as whole numbers on the 0-100 scale. The `tuner` <param> `{mean, var, n}` observes it and comes back updated in every donedata; the turn is low-confidence when the confidence is below the updated threshold (the floor 35 during the warmup of 8 observations, then `max(17.5, mean - std)`).")
S.statement(3, "A low-confidence product is unhandled and escalates: `{turn, handled: false, selected, reason: \"low_confidence\", tuner}`, `selected` its environment actions (its selections less `bot.ability.reasoning.input` and `bot.ability.cognition.ignore`).")
S.statement(4, "When the service abstains (the pass criterion wins, or no event could be offered) the product is empty: `{turn, handled: false, reason: \"empty_product\", tuner}` with no `selected`, and the tuner observes nothing.")
S.statement(5, "The selections handed out are checked before it answers: `<bot:verdict>` drops `bot.ability.cognition.ignore` and applies the attention policy with the `focus` and `focus_candidates` <param>s, then `<decision:route>` places each on the target offering it. A rejection fails the stage: `{turn, handled: false, failed: true, reason: \"Selection rejected: <message>\", tuner}`.")
S.statement(6, "A failed decision (`decision.failed {code}`) fails the stage with reason `the decision failed (<code>)`; one that cannot start (`error.*`) with `the decision could not run (<event>)`; and no answer within `decision_timeout_ms` with `the decision timed out after <n> ms`; each `{turn, handled: false, failed: true, reason, tuner}`.")

S.case(100, 1, "a confident selection is handled, its data the service's.",
       [done(1, True, "none", f"{selected(('bot.say', {}))} && {tuner(1)}")],
       params=run([SAY]), backend=decided({"bot.say": 0.9, ABSTAIN: 0.1}))
S.case(101, 1, "a required member is filled from the stimulus data.",
       [done(2, True, "none", selected(("bot.say", {"text": "hi"})))],
       params=run([{"event": "bot.say", "schema": {"type": "object", "required": ["text"],
                                                    "properties": {"text": {"type": "string"}}}, "target": "speaking"}],
                  turn=2, stimulus={"event": "bot.input", "data": {"text": "hi"}, "source": "body"}),
       backend=decided({"bot.say": 0.9, ABSTAIN: 0.1}, {"bot.say::text": "hi"}))
S.case(102, 1, "the deliberate handoff is never offered to the service.",
       [done(3, True, "none", selected(("bot.say", {})))],
       params=run([HANDOFF_CHOICE, SAY], turn=3), backend=decided({"bot.say": 0.9, ABSTAIN: 0.1}))

S.case(110, 2, "the confidence is the selections' minimum, observed by the tuner: 38 clears the warmup floor.",
       [done(1, True, "none", f"{selected(('bot.say', {}), (IGNORE, {}))} && {tuner(1)}"
                              " && _event.data.tuner.mean === 38")],
       params=run([SAY, IGNORE_CHOICE]), backend=decided({"bot.say": 0.6, IGNORE: 0.38, ABSTAIN: 0.02}))
S.case(111, 2, "past the warmup, the threshold follows the tuner's mean: 70 is low against a mean of 90.",
       [done(1, False, "low_confidence", f"{selected(('bot.say', {}))} && {tuner(21)}")],
       params=run([SAY], tuner={"mean": 90, "var": 100, "n": 20}), backend=decided({"bot.say": 0.7, ABSTAIN: 0.3}))
S.case(112, 2, "past the warmup, 80 clears a mean of 90.",
       [done(1, True, "none", f"{selected(('bot.say', {}))} && {tuner(21)}")],
       params=run([SAY], tuner={"mean": 90, "var": 100, "n": 20}), backend=decided({"bot.say": 0.8, ABSTAIN: 0.2}))

S.case(120, 3, "a low-confidence product escalates with its environment actions, the ignore dropped.",
       [done(4, False, "low_confidence", f"{selected(('bot.say', {}))} && {tuner(1)}")],
       params=run([SAY, IGNORE_CHOICE], turn=4), backend=decided({"bot.say": 0.9, IGNORE: 0.05, ABSTAIN: 0.05}))

S.case(130, 4, "the pass criterion winning is an empty product, unobserved.",
       [done(5, False, "empty_product", f"_event.data.selected === undefined && _event.data.failed === undefined && {tuner(0)}")],
       params=run([SAY], turn=5), backend=decided({"bot.say": 0.2, ABSTAIN: 0.8}))
S.case(131, 4, "with only the deliberate handoff offered, nothing can be offered: an empty product.",
       [done(6, False, "empty_product", f"_event.data.selected === undefined && {tuner(0)}")],
       params=run([HANDOFF_CHOICE], turn=6), backend=[])
S.case(132, 4, "an event whose required data cannot be filled is not offered.",
       [done(7, False, "empty_product", tuner(0))],
       params=run([{"event": "bot.say", "schema": {"type": "object", "required": ["text"],
                                                    "properties": {"text": {"type": "string"}}}}], turn=7),
       backend=[])

S.case(140, 5, "an event several targets offer, selected without a target, is rejected by the route.",
       [failed(8, "Selection rejected: Processing selected unavailable event: tool.go.", tuner(1))],
       params=run([{"event": "tool.go", "schema": {}, "target": "a"}, {"event": "tool.go", "schema": {}, "target": "b"}], turn=8),
       backend=decided({"tool.go": 0.9, ABSTAIN: 0.1}))
S.case(141, 5, "a focus_device outside focus_candidates is rejected by the verdict.",
       [failed(9, "Selection rejected: Processing selected focus_device outside available device candidates.")],
       params=run([{"event": "bot.focus_device", "schema": {"type": "object", "required": ["device"],
                                                            "properties": {"device": {"type": "string", "enum": ["garage"]}}}}],
                  turn=9, focus_candidates=["kitchen"]),
       backend=decided({"bot.focus_device": 0.9, ABSTAIN: 0.1}, {"bot.focus_device::device": "garage"}))
S.case(142, 5, "a focus_device among focus_candidates passes.",
       [done(10, True, "none", selected(("bot.focus_device", {"device": "kitchen"})))],
       params=run([{"event": "bot.focus_device", "schema": {"type": "object", "required": ["device"],
                                                            "properties": {"device": {"type": "string", "enum": ["kitchen"]}}}}],
                  turn=10, focus_candidates=["kitchen"]),
       backend=decided({"bot.focus_device": 0.9, ABSTAIN: 0.1}, {"bot.focus_device::device": "kitchen"}))

S.case(150, 6, "a backend error status fails the stage with the service's code.",
       [failed(11, "the decision failed (status)", tuner(0))],
       params=run([SAY], turn=11), backend=[{"path": SYSTEM_ONE, "status": 500, "body": {"error": {"message": "boom"}}}])
S.case(151, 6, "an answer that is not a system_one answer fails the stage as malformed.",
       [failed(12, "the decision failed (malformed)")],
       params=run([SAY], turn=12), backend=[{"path": SYSTEM_ONE, "status": 200, "body": {"answers": {}}}])
S.case(152, 6, "a decision that cannot start fails the stage.",
       [failed(13, "the decision could not run (error.execution)")],
       params=run([{"event": "bot.say"}], turn=13), backend=[])
S.case(153, 6, "no answer within decision_timeout_ms fails the stage.",
       [failed(14, "the decision timed out after 1 ms")],
       params=run([SAY], turn=14, timeout_ms=1), backend=decided({"bot.say": 0.9, ABSTAIN: 0.1}))
