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
RESPOND = "bot.ability.communication.respond"
CHAT = "/v1/chat/completions"
DECIDE = "/v1/systemone"
SCHEMAS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "schemas")
# The service-backed cases run on the logical clock, where no bound fires while a service call is in
# flight (intuition's own 20 s decision bound is not a <param> of the cognition); SLOW keeps the
# stage bound generous too.
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
    targets = {IGNORE: "cognition", RESPOND: "communication"}
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
REASONED_IGNORE = chat([{"event": IGNORE, "data": {}, "reason": "nothing to say", "target": "cognition"}])
REASONED_REPEAT = chat([{"event": RESPOND, "data": {"text": "hi"}, "reason": "answer", "target": "communication"}])
OFF_CHOICE = chat([{"event": "bot.focus_device", "data": {"device": "lamp"}, "reason": "look"}])


def output(n, cond):
    return expect(OUTPUT, f"_event.data.turn === {n} && {cond}")


def failed(n, message):
    return expect(FAILED, f"_event.data.turn === {n} && _event.data.message === {js(message)}"
                          " && _event.data.input.focus === null && _event.data.input.focus_candidates.length === 0")


IGNORED = f"_event.data.output.length === 1 && _event.data.output[0].event === {js(IGNORE)}"
REASONED = (f"_event.data.output.length === 1 && _event.data.output[0].event === {js(IGNORE)}"
            " && _event.data.output[0].reason === 'nothing to say' && _event.data.output[0].target === 'cognition'")


S.statement(1, "On start, and on every return to `idle`, it sends `bot.ability.offers` `{ability: \"cognition\", choices}` to its parent: its one choice, `bot.ability.cognition.ignore`, its schema compacted (`$schema`, `title` and `examples` dropped).")
S.statement(2, "A `bot.ability.cognition.input` `{turn, stimulus, focus?, focus_candidates?, choices?}` runs one turn of the cascade: autonomy (without a database it leaves the turn unhandled), then intuition, whose confident decision handles the turn: `bot.ability.cognition.output` `{turn, output}`, the output intuition's selections.")
S.statement(3, "When intuition leaves the turn unhandled (its decision abstains), reasoning (the generate service, schema `dispatch`) handles it, and the output is reasoning's selections; with a database (the `<param>` `database`) autonomy and reasoning use the memory and the turn is answered the same way.")
S.statement(4, "An intuition that escalates for low confidence hands out its environment actions (its selections less ignore), which the output carries first, before reasoning's selections; a reasoning selection that repeats one of them (the same event and data) is not carried again, so the turn acts once.")
S.statement(5, "A stage that fails answers the turn with `bot.ability.failed` `{message, turn, input: {focus, focus_candidates}}`, the message the stage's reason (`the decision failed (status)`, `the reasoning failed (status)`, `the reasoning failed (schema-mismatch)`).")
S.statement(6, "A stage that makes no progress for `child_operation_timeout_ms` (from its start for autonomy and the intuition; for reasoning from its streamed reply's first `generate.progress`, each later one starting it over), or runs `child_operation_ceiling_ms` in all (the `<param>` `bounds` overriding either, decision D101), ends the turn without an answer: the cognition sends `bot.reboot` `{reason: \"cognition_child_teardown_failed\"}` to its parent and waits in `rebooting`, where it acknowledges any `bot.ability.cognition.cancel` `{turn}` at once with `bot.ability.cognition.cancelled` `{turn}`.")
S.statement(7, "`bot.ability.cognition.cancel` `{turn}` for the turn in hand cancels the stage and is acknowledged with `bot.ability.cognition.cancelled` `{turn}`, and the cognition is idle again (it offers); in `idle` a cancel is acknowledged at once; a cancel for another turn is not taken and the turn goes on.")
S.statement(8, "`bot.ability.cognition.ignore` (its offer) is taken in `idle` and while processing, and only logged: it sends nothing and does not end a turn.")
S.statement(9, "The events it takes are checked against `abilities/schemas/` where they are created: a `bot.ability.cognition.input` without a stimulus is refused at its sender (`error.execution` there) and starts no turn.")

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

S.case(130, 4, "a low-confidence intuition's respond goes first, then reasoning's ignore.",
       [READY, send(INPUT, turn(1, text="hi", choices=(IGNORE, RESPOND))),
        output(1, f"_event.data.output.length === 2 && _event.data.output[0].event === {js(RESPOND)}"
                  " && _event.data.output[0].data.text === 'hi'"
                  f" && _event.data.output[1].event === {js(IGNORE)} && _event.data.output[1].reason === 'nothing to say'")],
       params=[SLOW], backend=[DECIDE_LOW, REASONED_IGNORE], clock="logical", ignore=[OFFERS])
S.case(131, 4, "reasoning's repeat of the low-confidence intuition's respond is not carried again: the output holds it once.",
       [READY, send(INPUT, turn(1, text="hi", choices=(IGNORE, RESPOND))),
        output(1, f"_event.data.output.length === 1 && _event.data.output[0].event === {js(RESPOND)}"
                  " && _event.data.output[0].data.text === 'hi' && _event.data.output[0].target === undefined")],
       params=[SLOW], backend=[DECIDE_LOW, REASONED_REPEAT], clock="logical", ignore=[OFFERS])

S.case(140, 5, "a failed decision fails the turn.",
       [READY, send(INPUT, turn(1)), failed(1, "the decision failed (status)")],
       params=[SLOW], backend=[{"path": DECIDE, "status": 500, "body": {"error": {"message": "boom"}}}], clock="logical", ignore=[OFFERS])
S.case(141, 5, "a failed reasoning call fails the turn.",
       [READY, send(INPUT, turn(1)), failed(1, "the reasoning failed (status)")],
       params=[SLOW], backend=[DECIDE_ABSTAIN, chat(status=500)], clock="logical", ignore=[OFFERS])
S.case(142, 5, "a reasoning selection outside the choices fails the turn.",
       [READY, send(INPUT, turn(1)), failed(1, "the reasoning failed (schema-mismatch)")],
       params=[SLOW], backend=[DECIDE_ABSTAIN, OFF_CHOICE], clock="logical", ignore=[OFFERS])

S.case(150, 6, "a stage that does not answer in time asks the parent for a reboot; a cancel is then acknowledged at once.",
       [READY, send(INPUT, turn(1)), expect("bot.reboot", "_event.data.reason === 'cognition_child_teardown_failed'"),
        send(CANCEL, js({"turn": 1})), expect(CANCELLED, "_event.data.turn === 1")],
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
