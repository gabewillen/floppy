# The intuition's statements and cases (tests/irp/harness.py writes them).

import json

S = suite
DONE = "done.invoke.intuition"
SYSTEM_ONE = "/v1/systemone"
PASS = "__reasoning__"
IGNORE = "bot.ability.cognition.ignore"
HANDOFF = "bot.ability.reasoning.input"
# Every choice is offered by the harness (its session id is the run's root,
# "session"), so each selection the stage sends comes to it.
HARNESS = "session"
SAY = {"event": "bot.say", "schema": {}, "target": HARNESS}
IGNORE_CHOICE = {"event": IGNORE, "schema": {}, "target": HARNESS}
HANDOFF_CHOICE = {"event": HANDOFF, "schema": {}, "target": HARNESS}
# The decision bound for every case but the timeout's: the decision service
# starts slowly on a loaded machine, and its own default (20 s) can pass first.
SLOW = 150000
STIMULUS = {"event": "bot.input", "data": {"observation": {"event": "text", "data": "hello"}}, "source": "body"}


def decided(probabilities, members=None, input_tokens=1):
    """The system_one answer: the selection question's distribution, and
    each member question <event>::<member> answering its one label."""
    chosen = max(probabilities, key=probabilities.get)
    answers = {"selection": {"type": "choice", "choice": chosen, "confidence": probabilities[chosen],
                             "probabilities": probabilities}}
    for name, label in (members or {}).items():
        answers[name] = {"type": "choice", "choice": label, "confidence": 1.0,
                                             "probabilities": {label: 1.0}}
    return [{"path": SYSTEM_ONE, "status": 200,
             "body": {"model": "fake", "answers": answers, "usage": {"input_tokens": input_tokens, "output_tokens": 1}}}]


def run(choices, turn=1, stimulus=STIMULUS, tuner=None, focus_candidates=None, timeout_ms=SLOW,
        hand_ons=None, max_context_tokens=None):
    params = [("turn", js(turn)), ("stimulus", js(stimulus)), ("focus", "null"), ("choices", js(choices))]
    if focus_candidates is not None:
        params.append(("focus_candidates", js(focus_candidates)))
    if tuner is not None:
        params.append(("confidence_tuner", js(tuner)))
    if hand_ons is not None:
        params.append(("hand_ons", js(hand_ons)))
    if max_context_tokens is not None:
        params.append(("max_context_tokens", js(max_context_tokens)))
    params.append(("decision_timeout_ms", js(timeout_ms)))
    return params


def done(turn, handled, reason, extra=None):
    cond = (f"_event.data.turn === {turn} && _event.data.handled === {js(handled)}"
            f" && _event.data.reason === {js(reason)} && typeof _event.data.confidence_tuner === 'object'"
            " && Array.isArray(_event.data.acted) && typeof _event.data.bytes_per_token === 'number'")
    if extra:
        cond += " && " + extra
    return expect(DONE, cond)


def selected(*items):
    return f"JSON.stringify(_event.data.selected) === {js(json.dumps([{'event': e, 'data': d, 'target': HARNESS} for e, d in items], sort_keys=True, separators=(',', ':')))}"


def acted(*items):
    return f"JSON.stringify(_event.data.acted) === {js(json.dumps([{'event': e, 'data': d, 'target': HARNESS} for e, d in items], sort_keys=True, separators=(',', ':')))}"


def sent(event, data=None):
    """The stage's send of `event` (with `data`) to the harness."""
    cond = None if data is None else f"JSON.stringify(_event.data) === {js(json.dumps(data, sort_keys=True, separators=(',', ':')))}"
    return expect(event, cond)


def failed(turn, reason, extra="true"):
    return done(turn, False, reason, f"_event.data.failed === true && {extra}")


def tuner(n):
    return f"_event.data.confidence_tuner.n === {n}"


S.statement(1, "It asks the decision service (kind `predicate`) over the `choices` <param> with the stimulus's model-facing text as its state, so the service leaves out `bot.ability.reasoning.input` and takes the stimulus's object data as evidence: each event's required data is filled from it. The service returns only the chosen event. A confident product is handled: it sends its selection to its target and ends with donedata `{turn, handled: true, selected: [{event, data, target}], acted, reason: \"none\", confidence_tuner, bytes_per_token}`, `acted` what it sent.")
S.statement(2, "The turn's confidence is its selection's probability as a whole number on the 0-100 scale. The `confidence_tuner` <param> `{mean, var, n}` observes it (the original's `ConfidenceTuner.observe`) and comes back updated in every donedata; the turn is low-confidence when the confidence is below the updated threshold (`should_escalate`) (the floor 35 during the warmup of 8 observations, then `max(17.5, mean - std)`).")
S.statement(3, "A low-confidence product is unhandled and escalates: it sends its environment actions (dispatched before the next tier runs), never the ignore or the deliberate handoff, and ends with `{turn, handled: false, reason: \"low_confidence\", acted, confidence_tuner, bytes_per_token}`, `acted` what it sent.")
S.statement(4, "When the service abstains (a pass criterion wins, or no event could be offered) the product is empty: `{turn, handled: false, reason: \"empty_product\", acted: [], confidence_tuner, bytes_per_token}` with no `selected`, and the tuner observes nothing. The service offers one pass per tier of the `hand_ons` <param> (`[\"reasoning\"]` by default), and a pass names its tier as `hand_to`.")
S.statement(5, "Each selection is sent to its target, the one session the choices tag its event with (the decision service's single offerer, the original's `_single_enabler`): the ignore goes to the session that offers it, and an event several sessions offer names no target, so it reaches none and is not acted.")
S.statement(6, "A failed decision (`decision.failed {code}`) fails the stage with reason `the decision failed (<code>)`; one that cannot start (`error.*`) with `the decision could not run (<event>)`; and no answer within `decision_timeout_ms` with `the decision timed out after <n> ms`; each `{turn, handled: false, failed: true, reason, acted: [], confidence_tuner, bytes_per_token}`.")
S.statement(7, "The stage answers once the decision service has ended. Its done data's `request_bytes` over the backend's counted `usage.input_tokens` teaches `bytes_per_token` (the <param>, 3 to start), which keeps the fewest bytes a token any answer showed and comes back in every donedata (the reference `ContextFit.observe`); with a `max_context_tokens` <param> the request is fitted to it and still sent.")

S.case(100, 1, "a confident selection is sent and handled, its data the service's.",
       [sent("bot.say", {}), done(1, True, "none", f"{selected(('bot.say', {}))} && {acted(('bot.say', {}))} && {tuner(1)}")],
       params=run([SAY]), backend=decided({"bot.say": 0.9, PASS: 0.1}))
S.case(101, 1, "a required member is filled from the stimulus data.",
       [sent("bot.say", {"text": "hi"}), done(2, True, "none", selected(("bot.say", {"text": "hi"})))],
       params=run([{"event": "bot.say", "schema": {"type": "object", "required": ["text"],
                                                    "properties": {"text": {"type": "string"}}}, "target": HARNESS}],
                  turn=2, stimulus={"event": "bot.input", "data": {"text": "hi"}, "source": "body"}),
       backend=decided({"bot.say": 0.9, PASS: 0.1}, {"bot.say::text": "hi"}))
S.case(102, 1, "the deliberate handoff is never offered to the service.",
       [sent("bot.say"), done(3, True, "none", selected(("bot.say", {})))],
       params=run([HANDOFF_CHOICE, SAY], turn=3), backend=decided({"bot.say": 0.9, PASS: 0.1}))
S.case(103, 1, "only the chosen event is selected and sent, however probable another.",
       [sent("bot.say"), done(4, True, "none", f"{selected(('bot.say', {}))} && {acted(('bot.say', {}))}")],
       params=run([SAY, IGNORE_CHOICE], turn=4), backend=decided({"bot.say": 0.5, IGNORE: 0.45, PASS: 0.05}))

S.case(110, 2, "the confidence is the selection's probability, observed by the tuner: 38 clears the warmup floor.",
       [sent("bot.say"),
        done(1, True, "none", f"{selected(('bot.say', {}))} && {tuner(1)}"
                              " && _event.data.confidence_tuner.mean === 38")],
       params=run([SAY, IGNORE_CHOICE]), backend=decided({"bot.say": 0.38, IGNORE: 0.31, PASS: 0.31}))
S.case(111, 2, "past the warmup, the threshold follows the tuner's mean: 70 is low against a mean of 90.",
       [sent("bot.say"), done(1, False, "low_confidence", f"_event.data.selected === undefined && {tuner(21)}")],
       params=run([SAY], tuner={"mean": 90, "var": 100, "n": 20}), backend=decided({"bot.say": 0.7, PASS: 0.3}))
S.case(112, 2, "past the warmup, 80 clears a mean of 90.",
       [sent("bot.say"), done(1, True, "none", f"{selected(('bot.say', {}))} && {tuner(21)}")],
       params=run([SAY], tuner={"mean": 90, "var": 100, "n": 20}), backend=decided({"bot.say": 0.8, PASS: 0.2}))

S.case(120, 3, "a low-confidence product sends its environment action, then escalates with it acted.",
       [sent("bot.say"), done(4, False, "low_confidence", f"_event.data.selected === undefined && {acted(('bot.say', {}))} && {tuner(21)}")],
       params=run([SAY, IGNORE_CHOICE], turn=4, tuner={"mean": 90, "var": 100, "n": 20}),
       backend=decided({"bot.say": 0.7, IGNORE: 0.2, PASS: 0.1}))
S.case(121, 3, "a low-confidence ignore is not sent: nothing is acted.",
       [done(5, False, "low_confidence", f"{acted()} && {tuner(21)}")],
       params=run([SAY, IGNORE_CHOICE], turn=5, tuner={"mean": 90, "var": 100, "n": 20}),
       backend=decided({IGNORE: 0.7, "bot.say": 0.2, PASS: 0.1}))

S.case(130, 4, "the reasoning pass winning is an empty product handed to reasoning, unobserved.",
       [done(5, False, "empty_product", f"_event.data.selected === undefined && _event.data.failed === undefined"
                                        f" && _event.data.hand_to === 'reasoning' && {acted()} && {tuner(0)}")],
       params=run([SAY], turn=5), backend=decided({"bot.say": 0.2, PASS: 0.8}))
S.case(131, 4, "with only the deliberate handoff offered, nothing can be offered: an empty product.",
       [done(6, False, "empty_product", f"_event.data.selected === undefined && _event.data.hand_to === undefined && {tuner(0)}")],
       params=run([HANDOFF_CHOICE], turn=6), backend=[])
S.case(132, 4, "an event whose required data cannot be filled is not offered.",
       [done(7, False, "empty_product", tuner(0))],
       params=run([{"event": "bot.say", "schema": {"type": "object", "required": ["text"],
                                                    "properties": {"text": {"type": "string"}}}}], turn=7),
       backend=[])
S.case(133, 4, "with intuition among the hand_ons, its pass hands the turn to intuition.",
       [done(8, False, "empty_product", f"_event.data.hand_to === 'intuition' && {acted()} && {tuner(0)}")],
       params=run([SAY], turn=8, hand_ons=["intuition", "reasoning"]),
       backend=decided({"bot.say": 0.1, "__intuition__": 0.6, PASS: 0.3}))

S.case(140, 5, "an event several sessions offer names no target, so it reaches none and is not acted; the turn is still handled.",
       [done(8, True, "none", f"_event.data.selected.length === 1 && _event.data.selected[0].target === undefined && {acted()}")],
       params=run([{"event": "tool.go", "schema": {}, "target": "a"}, {"event": "tool.go", "schema": {}, "target": "b"}], turn=8),
       backend=decided({"tool.go": 0.9, PASS: 0.1}))
S.case(141, 5, "the ignore goes to the session that offers it.",
       [sent(IGNORE, {}), done(9, True, "none", f"{selected((IGNORE, {}))} && {acted((IGNORE, {}))}")],
       params=run([IGNORE_CHOICE], turn=9),
       backend=decided({IGNORE: 0.9, PASS: 0.1}))
S.case(142, 5, "a focus_device is sent with its grounded device.",
       [sent("bot.focus_device", {"device": "kitchen"}), done(10, True, "none", selected(("bot.focus_device", {"device": "kitchen"})))],
       params=run([{"event": "bot.focus_device", "schema": {"type": "object", "required": ["device"],
                                                            "properties": {"device": {"type": "string", "enum": ["kitchen"]}}}, "target": HARNESS}],
                  turn=10, focus_candidates=["kitchen"]),
       backend=decided({"bot.focus_device": 0.9, PASS: 0.1}, {"bot.focus_device::device": "kitchen"}))

S.case(150, 6, "a backend error status fails the stage with the service's code.",
       [failed(11, "the decision failed (status)", f"{acted()} && {tuner(0)}")],
       params=run([SAY], turn=11), backend=[{"path": SYSTEM_ONE, "status": 500, "body": {"error": {"message": "boom"}}}])
S.case(151, 6, "an answer that is not a system_one answer fails the stage as malformed.",
       [failed(12, "the decision failed (malformed)")],
       params=run([SAY], turn=12), backend=[{"path": SYSTEM_ONE, "status": 200, "body": {"answers": {}}}])
S.case(152, 6, "a decision that cannot start fails the stage.",
       [failed(13, "the decision could not run (error.execution)")],
       params=run([{"event": "bot.say"}], turn=13), backend=[])
S.case(153, 6, "no answer within decision_timeout_ms fails the stage.",
       [failed(14, "the decision timed out after 1 ms")],
       params=run([SAY], turn=14, timeout_ms=1), backend=decided({"bot.say": 0.9, PASS: 0.1}))

S.case(160, 7, "an answer counting many tokens teaches fewer bytes a token.",
       [sent("bot.say"), done(15, True, "none", "_event.data.bytes_per_token > 0 && _event.data.bytes_per_token < 3")],
       params=run([SAY], turn=15), backend=decided({"bot.say": 0.9, PASS: 0.1}, input_tokens=1000000))
S.case(161, 7, "an answer counting few tokens teaches nothing: bytes a token stay 3.",
       [sent("bot.say"), done(16, True, "none", "_event.data.bytes_per_token === 3")],
       params=run([SAY], turn=16), backend=decided({"bot.say": 0.9, PASS: 0.1}))
S.case(162, 7, "a request over max_context_tokens is still sent: it cannot fit, and the turn is handled.",
       [sent("bot.say"), done(17, True, "none", selected(("bot.say", {})))],
       params=run([SAY], turn=17, max_context_tokens=1), backend=decided({"bot.say": 0.9, PASS: 0.1}))
