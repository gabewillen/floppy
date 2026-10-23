# The reflection's statements and cases (tests/irp/harness.py writes them).

import json

S = suite
INPUT = "bot.ability.reflection.input"
OUTPUT = "bot.ability.reflection.output"
FAILED = "bot.ability.failed"
DB = ("database", "'/memory/floppy.sqlite'")
# Generous bounds for the wall-clock cases, so a loaded machine does not time them out.
SLOW_SELECT = ("select_timeout_ms", "300000")
SLOW_REVISING = ("revising_timeout_ms", "300000")
CHAT = "/v1/chat/completions"


def turn(n):
    return js({
        "turn": n,
        "cognition_input": {
            "stimulus": {"event": "environment.sound", "data": {"label": "knock"}},
            "focus": "door",
            "focus_candidates": ["door"],
        },
        "cognition_output": [
            {"event": "bot.ability.communication.respond", "data": {"parts": [{"words": "Hello!"}]}, "reason": "a knock", "target": "communication"}
        ],
    })


def completion(content):
    return {"object": "chat.completion",
            "choices": [{"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": content}}]}


def chat(content, status=200):
    if status != 200:
        return {"path": CHAT, "status": status, "body": {"error": {"message": "boom"}}}
    return {"path": CHAT, "status": 200, "body": completion(content if isinstance(content, str) else json.dumps(content))}


def select(*events):
    return chat({"events": list(events)})


# The reflection's own session id: its choices are its own transitions, so
# each selection names it as its target.
OWN = "session.reflection"
CREATE = {"event": "bot.behavior.create", "target": OWN,
          "data": {"event": "bot.behavior.create", "name": "GreetOnKnock",
                   "triggers": ["phone.incoming_call", "environment.sound"],
                   "description": "Say hello when a knock sound arrives.",
                   "reason": "Same knock and greeting pattern across recent episodes.", "source": None},
          "reason": "a repeated pattern"}


def change(name):
    return {"event": "bot.behavior.change", "target": OWN,
            "data": {"event": "bot.behavior.change", "name": name, "triggers": ["environment.sound"],
                     "description": "Say hello twice.", "reason": "It should greet warmly.", "source": None},
            "reason": "revise it"}


def brk(name):
    return {"event": "bot.behavior.break", "target": OWN,
            "data": {"event": "bot.behavior.break", "name": name, "reason": "  It greets strangers.  "},
            "reason": "harm outweighs practice"}


# The builder's answers (the reflection test's): its plan and a turn behavior draft.
BEHAVIOR = """<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" name="GreetOnKnock" initial="answering">
  <datamodel>
    <data id="turn" expr="0"/>
    <data id="input" expr="null"/>
  </datamodel>
  <state id="answering">
    <transition target="answered"/>
  </state>
  <final id="answered">
    <donedata>
      <content expr="({turn: turn, handled: true, selected: [{event: 'bot.ability.communication.respond', target: 'communication', data: {parts: [{words: 'Hello!'}]}, reason: 'a knock'}]})"/>
    </donedata>
  </final>
</scxml>"""
REJECTED = BEHAVIOR.replace('target="answered"', 'target="nowhere"')
# A turn behavior whose test waits 10 s for it (it never finishes).
WAITING = BEHAVIOR.replace('<transition target="answered"/>', "")
PLAN = chat('{"name":"GreetOnKnock","summary":"Answer a knock with a greeting.","states":[{"id":"answering","kind":"atomic","parent":"","purpose":"Decide the answer."},{"id":"answered","kind":"final","parent":"","purpose":"Carry the answer."}],"events":[],"transitions":[{"source":"answering","event":"","target":"answered","description":"Answer at once."}]}')


def draft(scxml, settle=2000, done=True):
    return chat({"scxml": scxml, "schemas": {"$defs": {}}, "script": "",
                 "tests": [{"name": "answers its turn", "events": [], "settle_ms": settle, "expect_events": [], "expect_done": done}]})


def failed(text):
    return expect(FAILED, f"_event.data.message === {js(text)}")


def failed_prefix(text):
    return expect(FAILED, f"_event.data.message.indexOf({js(text)}) === 0")


S.statement(1, "Without a database (the `<param>` `database` null, its default) it offers nothing on start and answers each `bot.ability.reflection.input` `{turn, cognition_input, cognition_output}` at once with `bot.ability.reflection.output` (no data), asking no model.")
S.statement(2, "With a database, `recalling` reads the prior episodes and the behavior inventory in one memory transaction and `processing` asks the generate service (schema `dispatch`, its `events` at most one) to select at most one of `bot.behavior.create`, `bot.behavior.change` or `bot.behavior.break`, each offered by the reflection's own session (a change's and a break's `name` pinned to the inventory's names, and neither offered when the inventory is empty; decision D100); the selection is sent to the reflection's own session, and an empty selection goes to `storing`, which stores the turn's episode and answers `bot.ability.reflection.output`.")
S.statement(3, "A select call the generate service refuses (`not-json`, or `schema-mismatch` such as more than one selected event, which the dispatch tool does not admit) is asked again with the repair feedback (`selection_feedback`) carrying its rejection, at most twice (decision D130); the same rejection as the one being repaired, or a third rejection, fails the select with `bot.ability.failed` `{message: \"Selection rejected: <message>\"}`; any other failed select call fails it with `Reflection select failed (<code>)`; a select call whose streamed reply makes no progress for `select_timeout_ms` (its first `generate.progress` arms it, each later one re-arms it) or that runs `select_ceiling_ms` in all fails it with `Reflection child cancellation timed out.` (decision D101).")
S.statement(4, "A selected `bot.behavior.break` (`breaking`) marks the named behavior BROKEN with its trimmed reason and keeps the row, then stores the episode and answers `bot.ability.reflection.output`; a break of a name the inventory does not hold is not offered, so a model that selects one is refused twice and fails the select with `Selection rejected: <message>`.")
S.statement(5, "A selected `bot.behavior.create` (`revising`) stores a DRAFT stub for a new name, invokes the builder bot with a `builder.request`, and on the builder's `done` (`accepting`) replaces the row ACTIVE with the built document's path (its three documents the builder wrote, decision D121), then stores the episode and answers `bot.ability.reflection.output`; a `bot.behavior.change` of a stored name reads its documents back (`reading`) and revises it the same way, and a change of a name the inventory does not hold is not offered, so a model that selects one is refused twice and fails the select with `Selection rejected: <message>`.")
S.statement(6, "A builder that fails with the last draft it rejected (`abandoning`) keeps that draft's path in the row as DRAFT and fails the turn with `Reflection change abandoned: same diagnostic message after fix:` and the rendered diagnostics (no-progress); a builder that fails with no draft fails the turn with `Reflection revision failed: <reason>: <first diagnostic>`, the create's DRAFT stub kept.")
S.statement(7, "`revising` is bounded by `revising_timeout_ms` without progress while a builder generation streams (the builder, invoked with `progress_target` `'#_parent'`, passes on each `generate.progress` of its streamed generations, which arms it, and sends `builder.progress` as it enters a stage, which disarms it) and by `revising_ceiling_ms` in all (the `<param>` `bounds` overriding either, decision D101): when one passes, the revision is cancelled and the turn fails with `Reflection child cancellation timed out.`.")
S.statement(8, "A memory transaction that fails (here the recall, on a database the run does not grant) fails the turn with `Reflection memory recall failed: <detail>`; after any failure the turn's episode is stored with no behavior (unless storing it is what failed; decision D97) and the reflection is idle again and takes the next input.")
S.statement(9, "An input that arrives while a reflection is in hand waits and runs after it, in order: each turn gets its own answer.")

S.case(100, 1, "without a database each input is answered at once with an output.",
       [send(INPUT, turn(1)), expect(OUTPUT, "_event.data === null || _event.data === undefined"),
        send(INPUT, turn(2)), expect(OUTPUT)])

S.case(110, 2, "an empty selection stores the episode and answers the output; the next turn runs the same way.",
       [send(INPUT, turn(1)), expect(OUTPUT), send(INPUT, turn(2)), expect(OUTPUT)],
       params=[DB, SLOW_SELECT, SLOW_REVISING], backend=[select(), select()], clock="wall")

S.case(120, 3, "two selected events, refused twice running, fail the turn.",
       [send(INPUT, turn(1)), failed_prefix("Selection rejected: ")],
       params=[DB, SLOW_SELECT, SLOW_REVISING], backend=[select(CREATE, CREATE)], clock="wall")
S.case(121, 3, "a select call the backend fails fails the turn with its code.",
       [send(INPUT, turn(1)), failed("Reflection select failed (status)")],
       params=[DB, SLOW_SELECT, SLOW_REVISING], backend=[chat(None, status=500)], clock="wall")
S.case(122, 3, "a select call that outlasts its backstop fails the turn.",
       [send(INPUT, turn(1)), failed("Reflection child cancellation timed out.")],
       params=[DB, ("select_ceiling_ms", "1")], backend=[select()], clock="wall")
S.case(123, 3, "a select reply that is not JSON is asked again, and the repaired empty selection answers the output.",
       [send(INPUT, turn(1)), expect(OUTPUT)],
       params=[DB, SLOW_SELECT, SLOW_REVISING], backend=[chat("not json"), select()], clock="wall")
S.case(124, 3, "three different rejections running fail the turn after two repairs.",
       [send(INPUT, turn(1)), failed_prefix("Selection rejected: ")],
       params=[DB, SLOW_SELECT, SLOW_REVISING], backend=[chat("not json"), select(CREATE, CREATE), chat("not json"), select()], clock="wall")
S.case(125, 3, "two different rejections running are both repaired, and the repaired empty selection answers the output.",
       [send(INPUT, turn(1)), expect(OUTPUT)],
       params=[DB, SLOW_SELECT, SLOW_REVISING], backend=[chat("not json"), select(CREATE, CREATE), select()], clock="wall")

S.case(130, 4, "a break of a behavior the inventory does not hold is not offered: selecting one fails the select.",
       [send(INPUT, turn(1)), failed_prefix("Selection rejected: ")],
       params=[DB, SLOW_SELECT, SLOW_REVISING], backend=[select(brk("Nobody"))], clock="wall")
S.case(131, 4, "a break of a stored behavior answers the output and keeps the row: a second break of it answers too.",
       [send(INPUT, turn(1)), expect(OUTPUT), send(INPUT, turn(2)), expect(OUTPUT), send(INPUT, turn(3)), expect(OUTPUT)],
       params=[DB, SLOW_SELECT, SLOW_REVISING], backend=[select(CREATE), PLAN, draft(BEHAVIOR), select(brk("GreetOnKnock")), select(brk("GreetOnKnock"))],
       clock="wall")

S.case(140, 5, "a create is built and stored, and the turn answers the output.",
       [send(INPUT, turn(1)), expect(OUTPUT)],
       params=[DB, SLOW_SELECT, SLOW_REVISING], backend=[select(CREATE), PLAN, draft(BEHAVIOR)], clock="wall")
S.case(141, 5, "a change of a created behavior is built and stored.",
       [send(INPUT, turn(1)), expect(OUTPUT), send(INPUT, turn(2)), expect(OUTPUT)],
       params=[DB, SLOW_SELECT, SLOW_REVISING], backend=[select(CREATE), PLAN, draft(BEHAVIOR), select(change("GreetOnKnock")), PLAN, draft(BEHAVIOR)],
       clock="wall")
S.case(142, 5, "a change of a behavior the inventory does not hold is not offered: selecting one fails the select.",
       [send(INPUT, turn(1)), failed_prefix("Selection rejected: ")],
       params=[DB, SLOW_SELECT, SLOW_REVISING], backend=[select(change("Nobody"))], clock="wall")

S.case(150, 6, "a build that never validates abandons the change with the rendered diagnostics, and its draft row stays (a break of it answers).",
       [send(INPUT, turn(1)),
        failed_prefix("Reflection change abandoned: same diagnostic message after fix:\nbehavior validation failed:\n  error[SCXML005] validate: "),
        send(INPUT, turn(2)), expect(OUTPUT)],
       params=[DB, SLOW_SELECT, SLOW_REVISING], backend=[select(CREATE), PLAN, draft(REJECTED), draft(REJECTED), select(brk("GreetOnKnock"))],
       clock="wall")
S.case(151, 6, "a build whose every draft generation fails fails the turn with no draft, and the create's stub stays (a break of it answers).",
       [send(INPUT, turn(1)),
        failed_prefix("Reflection revision failed: no-progress: the author generation failed (status)"),
        send(INPUT, turn(2)), expect(OUTPUT)],
       params=[DB, SLOW_SELECT, SLOW_REVISING], backend=[select(CREATE), PLAN, chat(None, status=500), chat(None, status=500), select(brk("GreetOnKnock"))],
       clock="wall")

S.case(160, 7, "a revision that outlasts revising_ceiling_ms fails the turn.",
       [send(INPUT, turn(1)), failed("Reflection child cancellation timed out.")],
       params=[DB, SLOW_SELECT, ("revising_ceiling_ms", "2000")],
       backend=[select(CREATE), PLAN, draft(WAITING, settle=20000, done=False)], clock="wall")

S.case(170, 8, "a recall on a database the run does not grant fails the turn, and the next input is taken.",
       [send(INPUT, turn(1)), failed_prefix("Reflection memory recall failed: "),
        send(INPUT, turn(2)), expect(FAILED)],
       params=[("database", "'/nowhere/floppy.sqlite'")])

S.case(180, 9, "a second input sent while the first is in hand runs after it: the first's failure, then the second's output.",
       [send(INPUT, turn(1)), send(INPUT, turn(2)),
        failed_prefix("Selection rejected: "), expect(OUTPUT)],
       params=[DB, SLOW_SELECT, SLOW_REVISING], backend=[select(brk("Nobody")), select(brk("Nobody")), select()], clock="wall")
