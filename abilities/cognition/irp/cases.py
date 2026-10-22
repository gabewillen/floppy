# The cognition's statements and cases (tests/irp/harness.py writes them).

import json
import os

S = suite
INPUT = "bot.ability.cognition.input"
OUTPUT = "bot.ability.cognition.output"
FAILED = "bot.ability.failed"
CANCEL = "bot.ability.cognition.cancel"
CANCELLED = "bot.ability.cognition.cancelled"
OFFERS = "bot.ability.offers"
IGNORE = "bot.ability.cognition.ignore"
FAILED_STEP = "bot.ability.cognition.step.failed"
TIMED_OUT_STEP = "bot.ability.cognition.step.timed_out"
RESPOND = "bot.ability.communication.respond"
CHAT = "/v1/chat/completions"
DECIDE = "/v1/systemone"
SCHEMAS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "schemas")
# The service-backed cases run on the logical clock, where no bound fires while a service call is in
# flight (the decision layer's own 20 s decision bound is the cognition's decision_timeout_ms); SLOW keeps
# the stage bound generous too.
SLOW = ("child_operation_timeout_ms", "300000")
DB = ("database", "'/memory/floppy.sqlite'")

READY = expect(OFFERS, f"_event.data.ability === 'cognition' && _event.data.choices.length === 1 && _event.data.choices[0].event === {js(IGNORE)}"
                       " && _event.data.choices[0].schema.title === undefined")


def offered(event):
    """An ability schema as the body offers it ($schema, title and examples compacted away)."""
    schema = json.load(open(os.path.join(SCHEMAS, f"{event}.json")))
    for key in ("$schema", "title", "examples"):
        schema.pop(key, None)
    return schema


def turn(n, text="hello", choices=(IGNORE,)):
    # Tagged with their offering sessions' ids: the cognition's own ignore,
    # and respond offered by the harness (the run's root, "session").
    targets = {IGNORE: "session.cognition", RESPOND: "session"}
    return js({"turn": n,
               "stimulus": {"event": "bot.input", "data": {"text": text}, "source": "conversation"},
               "focus": None, "focus_candidates": [],
               "choices": [{"event": e, "schema": offered(e), "target": targets[e]} for e in choices]})


def completion(content):
    return {"object": "chat.completion",
            "choices": [{"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": content}}]}


def chat(events=None, status=200):
    if status != 200:
        return {"path": CHAT, "status": status, "body": {"error": {"message": "boom"}}}
    return {"path": CHAT, "status": 200, "body": completion(json.dumps({"events": events}))}


def decided(probabilities, choice, extra=None):
    answers = {"selection": {"type": "choice", "choice": choice, "confidence": probabilities[choice], "probabilities": probabilities}}
    answers.update(extra or {})
    return {"path": DECIDE, "status": 200, "body": {"model": "fake", "answers": answers, "usage": {"input_tokens": 1, "output_tokens": 1}}}


DECIDE_IGNORE = decided({IGNORE: 0.8, "__unhandled__": 0.2}, IGNORE)
DECIDE_ABSTAIN = decided({IGNORE: 0.2, "__unhandled__": 0.8}, "__unhandled__")
# Ignore 0.34 and respond 0.33 (its text grounded in the stimulus): the least, 33, is under the warmup floor 35.
DECIDE_LOW = decided({IGNORE: 0.34, RESPOND: 0.33, "__unhandled__": 0.33}, IGNORE,
                     {f"{RESPOND}::text": {"type": "choice", "choice": "hi", "confidence": 1.0, "probabilities": {"hi": 1.0}}})
REASONED_IGNORE = chat([{"event": IGNORE, "data": {}, "reason": "nothing to say", "target": "session.cognition"}])
REASONED_REPEAT = chat([{"event": RESPOND, "data": {"text": "hi"}, "reason": "answer", "target": "session"}])
SAID_HI = expect(RESPOND, "_event.data.text === 'hi'")
OFF_CHOICE = chat([{"event": "bot.focus_device", "data": {"device": "lamp"}, "reason": "look"}])


def output(n, cond):
    return expect(OUTPUT, f"_event.data.turn === {n} && {cond}")


def failed(n, message):
    return expect(FAILED, f"_event.data.turn === {n} && _event.data.message === {js(message)}"
                          " && _event.data.input.focus === null && _event.data.input.focus_candidates.length === 0")


def handed_back(n, step, stage):
    """The failure of turn `n` handed back to the parent as the stimulus of a new turn."""
    return expect(INPUT, f"_event.data.stimulus.event === {js(step)} && _event.data.stimulus.data.turn === {n}"
                         f" && _event.data.stimulus.data.stage === {js(stage)}")


IGNORED = f"_event.data.output.length === 1 && _event.data.output[0].event === {js(IGNORE)}"
REASONED = (f"_event.data.output.length === 1 && _event.data.output[0].event === {js(IGNORE)}"
            " && _event.data.output[0].reason === 'nothing to say' && _event.data.output[0].target === 'session.cognition'")


S.statement(1, "On start, and on every return to `idle`, it sends `bot.ability.offers` `{ability: \"cognition\", choices}` to its parent: its one choice, `bot.ability.cognition.ignore`, its schema made model-facing (`$schema`, `title` and `examples` dropped).")
S.statement(2, "A `bot.ability.cognition.input` `{turn, stimulus, focus?, focus_candidates?, choices?}` runs one turn of the cascade: autonomy (without a database it leaves the turn unhandled), then the decision layer, whose confident decision handles the turn: `bot.ability.cognition.output` `{turn, output}`, the output the decision layer's selections, which it has sent to their targets.")
S.statement(3, "When the decision layer leaves the turn unhandled (its decision abstains), reasoning (the generate service, schema `dispatch`) handles it, and the output is reasoning's selections; with a database (the `<param>` `database`) autonomy and reasoning use the memory and the turn is answered the same way.")
S.statement(4, "A decision layer that escalates for low confidence sends its environment actions (its selections) before the turn goes on to reasoning; the output is reasoning's selections, which reasoning sends after them, a repeat of an environment action included.")
S.statement(5, "A decision layer that fails (`the decision failed (status)`, or no decision within `decision_timeout_ms`) does not fail the turn: the cognition logs `cognition.cascaded` and reasoning answers it (the reference's D-R-36). A reasoning stage that fails answers the turn with `bot.ability.failed` `{message, turn, input: {focus, focus_candidates}}`, the message the stage's reason (`the reasoning failed (status)`, or `Selection rejected: schema-mismatch` once a refused selection is not repaired), then hands the failure back to its parent as a `bot.ability.cognition.input` whose stimulus is `bot.ability.cognition.step.failed` `{message: \"Cognition's reasoning step failed, so the turn failed: <reason>\", stage: \"reasoning\", turn}`.")
S.statement(6, "A step that makes no progress for `child_operation_timeout_ms` (from its start for autonomy and the decision layer; for reasoning from its streamed reply's first `generate.progress`, each later one starting it over), or runs `child_operation_ceiling_ms` in all (the `<param>` `bounds` overriding either, decision D101, and its `reflex_timeout_ms`, `intuition_timeout_ms` or `reasoning_timeout_ms` bounding one tier), fails only its turn: `bot.ability.failed` `{message: \"Cognition's <stage> step took longer than <seconds> seconds, so the turn failed.\", turn, input}`, then the failure handed back to its parent as a `bot.ability.cognition.input` whose stimulus is `bot.ability.cognition.step.timed_out` `{message, stage, timeout_seconds, turn}`; the cognition is idle again (it offers). A turn whose stimulus is a handed-back failure hands nothing back when it fails too.")
S.statement(7, "`bot.ability.cognition.cancel` `{turn}` for the turn in hand cancels the stage and is acknowledged with `bot.ability.cognition.cancelled` `{turn}`, and the cognition is idle again (it offers); in `idle` a cancel is acknowledged at once; a cancel for another turn is not taken and the turn goes on.")
S.statement(8, "`bot.ability.cognition.ignore` (its offer) is taken in `idle` and while processing, and only logged: it sends nothing and does not end a turn.")
S.statement(11, "Until it is told the bot's name (`bot.ability.identity.named`), every return to `idle` sends `bot.ability.identity.ask` `{}` to its parent after its offers (the reference's `_ask_name`); once told, it asks no more.")
S.statement(10, "With an `intuition_model` (the `<param>`; none by default composes no intuition tier, decision D144), a turn the decision layer leaves unhandled goes to the intuition tier first, offered the deliberative handoff `bot.ability.reasoning.input` as the cognition's own: its selections handle the turn, or, when they hold the handoff, the others are sent first and the turn goes on to reasoning, whose output follows them. An intuition that fails hands the same turn on to reasoning (`cognition.cascaded`).")
S.statement(9, "The events it takes are checked against `abilities/schemas/` where they are created: a `bot.ability.cognition.input` without a stimulus is refused at its sender (`error.execution` there) and starts no turn.")

ASK = "bot.ability.identity.ask"
NAMED = "bot.ability.identity.named"

# Every other case drops the identity ask (statement 11).
_case = S.case


def _case_dropping_ask(*args, ignore=(), **kwargs):
    return _case(*args, ignore=tuple(ignore) + (ASK,), **kwargs)


S.case = _case_dropping_ask

S.case(100, 1, "the cognition offers ignore on start.", [READY])
S.case(101, 1, "it offers again on the return to idle after a turn.",
       [READY, send(INPUT, turn(1)), output(1, IGNORED), READY],
       params=[SLOW], backend=[DECIDE_IGNORE], clock="logical")

S.case(110, 2, "a confident intuition handles the turn: the output is its ignore selection.",
       [READY, send(INPUT, turn(1)), output(1, IGNORED)],
       params=[SLOW], backend=[DECIDE_IGNORE], clock="logical", ignore=[OFFERS])
S.case(111, 2, "turns are answered one after another, each with its own turn.",
       [READY, send(INPUT, turn(1)), output(1, IGNORED), send(INPUT, turn(2)), output(2, IGNORED)],
       params=[SLOW], backend=[DECIDE_IGNORE], clock="logical", ignore=[OFFERS])

S.case(120, 3, "an abstaining intuition escalates to reasoning, whose selection is the output.",
       [READY, send(INPUT, turn(1)), output(1, REASONED)],
       params=[SLOW], backend=[DECIDE_ABSTAIN, REASONED_IGNORE], clock="logical", ignore=[OFFERS])
S.case(121, 3, "with a database, the turn goes through autonomy's and reasoning's memory and is answered by reasoning.",
       [READY, send(INPUT, turn(1)), output(1, REASONED)],
       params=[SLOW, DB], backend=[DECIDE_ABSTAIN, REASONED_IGNORE, chat([])], clock="logical", ignore=[OFFERS])

S.case(130, 4, "a low-confidence intuition's respond is sent first; the output is reasoning's ignore.",
       [READY, send(INPUT, turn(1, text="hi", choices=(IGNORE, RESPOND))), SAID_HI,
        output(1, f"_event.data.output.length === 1"
                  f" && _event.data.output[0].event === {js(IGNORE)} && _event.data.output[0].reason === 'nothing to say'")],
       params=[SLOW], backend=[DECIDE_LOW, REASONED_IGNORE], clock="logical", ignore=[OFFERS])
S.case(131, 4, "reasoning's repeat of the low-confidence intuition's respond is sent again, after it.",
       [READY, send(INPUT, turn(1, text="hi", choices=(IGNORE, RESPOND))), SAID_HI, SAID_HI,
        output(1, f"_event.data.output.length === 1 && _event.data.output[0].event === {js(RESPOND)}"
                  " && _event.data.output[0].data.text === 'hi' && _event.data.output[0].target === 'session'")],
       params=[SLOW], backend=[DECIDE_LOW, REASONED_REPEAT], clock="logical", ignore=[OFFERS])

S.case(140, 5, "a failed decision cascades the turn to reasoning, which answers it.",
       [READY, send(INPUT, turn(1)), output(1, REASONED)],
       params=[SLOW], backend=[{"path": DECIDE, "status": 500, "body": {"error": {"message": "boom"}}}, REASONED_IGNORE],
       clock="logical", ignore=[OFFERS])
S.case(141, 5, "a failed reasoning call fails the turn, and the failure is handed back.",
       [READY, send(INPUT, turn(1)), failed(1, "the reasoning failed (status)"), handed_back(1, FAILED_STEP, "reasoning")],
       params=[SLOW], backend=[DECIDE_ABSTAIN, chat(status=500)], clock="logical", ignore=[OFFERS])
S.case(142, 5, "a reasoning selection outside the choices, refused again after its repair, fails the turn, and the failure is handed back.",
       [READY, send(INPUT, turn(1)), failed(1, "Selection rejected: schema-mismatch"), handed_back(1, FAILED_STEP, "reasoning")],
       params=[SLOW], backend=[DECIDE_ABSTAIN, OFF_CHOICE, OFF_CHOICE], clock="logical", ignore=[OFFERS])
S.case(143, 5, "a handed-back failure that fails again is not handed back: the cognition is idle again.",
       [READY, send(INPUT, js({"turn": 2, "focus": None, "focus_candidates": [], "choices": [],
                                "stimulus": {"event": FAILED_STEP, "source": "cognition",
                                             "data": {"message": "earlier", "stage": "reasoning", "turn": 1}}})),
        expect(FAILED, "_event.data.turn === 2"), READY],
       params=[SLOW], backend=[DECIDE_ABSTAIN], clock="logical")

S.case(150, 6, "a step that does not answer in time fails only its turn, which is handed back; the cognition is idle again.",
       [READY, send(INPUT, turn(1)),
        expect(FAILED, "_event.data.turn === 1 && _event.data.message === \"Cognition's reflex step took longer than 0.001 seconds, so the turn failed.\""),
        handed_back(1, TIMED_OUT_STEP, "reflex"), READY],
       params=[("child_operation_timeout_ms", "1")], backend=[DECIDE_IGNORE], clock="wall")

S.case(160, 7, "a cancel for the turn in hand is acknowledged and the cognition is idle again.",
       [READY, send(INPUT, turn(1)), send(CANCEL, js({"turn": 1})), expect(CANCELLED, "_event.data.turn === 1"), READY],
       params=[SLOW], backend=[DECIDE_IGNORE], clock="logical")
S.case(161, 7, "a cancel in idle is acknowledged at once with its turn.",
       [READY, send(CANCEL, js({"turn": 7})), expect(CANCELLED, "_event.data.turn === 7")])
S.case(162, 7, "a cancel for another turn is not taken: the turn is answered.",
       [READY, send(INPUT, turn(1)), send(CANCEL, js({"turn": 2})), output(1, IGNORED)],
       params=[SLOW], backend=[DECIDE_IGNORE], clock="logical", ignore=[OFFERS])

S.case(170, 8, "an ignore in idle sends nothing: the next event answered is the cancel's.",
       [READY, send(IGNORE, js({})), send(CANCEL, js({"turn": 3})), expect(CANCELLED, "_event.data.turn === 3")])
S.case(171, 8, "an ignore during a turn sends nothing and the turn is answered.",
       [READY, send(INPUT, turn(1)), send(IGNORE, js({})), output(1, IGNORED)],
       params=[SLOW], backend=[DECIDE_IGNORE], clock="logical", ignore=[OFFERS])

S.case(180, 9, "an input without a stimulus is refused at the sender; the cognition stays idle and acknowledges a cancel at once.",
       [READY, send(INPUT, js({"turn": 1})), expect("error.execution"),
        send(CANCEL, js({"turn": 1})), expect(CANCELLED, "_event.data.turn === 1")])

INTUITION = ("intuition_model", "'chat-model'")
HANDOFF = "bot.ability.reasoning.input"
INTUITED_HANDOFF = chat([{"event": RESPOND, "data": {"text": "hi"}, "reason": "hold on", "target": "session"},
                         {"event": HANDOFF, "data": {}, "reason": "this needs thought", "target": "session.cognition"}])
S.case(190, 10, "an intuition tier handles the turn the decision layer left unhandled, without reasoning.",
       [READY, send(INPUT, turn(1)), output(1, REASONED)],
       params=[SLOW, INTUITION], backend=[DECIDE_ABSTAIN, REASONED_IGNORE], clock="logical", ignore=[OFFERS])
S.case(191, 10, "an intuition that selects the handoff sends its respond first; the output is it, then reasoning's ignore.",
       [READY, send(INPUT, turn(1, choices=(IGNORE, RESPOND))), SAID_HI,
        output(1, f"_event.data.output.length === 2 && _event.data.output[0].event === {js(RESPOND)}"
                  f" && _event.data.output[1].event === {js(IGNORE)}")],
       params=[SLOW, INTUITION], backend=[DECIDE_ABSTAIN, INTUITED_HANDOFF, REASONED_IGNORE], clock="logical",
       ignore=[OFFERS])
S.case(192, 10, "a failed intuition cascades the turn to reasoning, which answers it.",
       [READY, send(INPUT, turn(1)), output(1, REASONED)],
       params=[SLOW, INTUITION], backend=[DECIDE_ABSTAIN, chat(status=500), REASONED_IGNORE], clock="logical",
       ignore=[OFFERS])

_case(200, 11, "the cognition asks the identity for the bot's name after its offers, again after a turn, and no more once told.",
      [READY, expect(ASK), send(INPUT, turn(1)), output(1, IGNORED), READY, expect(ASK),
       send(NAMED, js({"name": "Pip"})), send(INPUT, turn(2)), output(2, IGNORED), READY,
       send(CANCEL, js({"turn": 3})), expect(CANCELLED, "_event.data.turn === 3")],
      params=[SLOW], backend=[DECIDE_IGNORE, DECIDE_IGNORE], clock="logical")
