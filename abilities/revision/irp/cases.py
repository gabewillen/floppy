# The revision's statements and cases (tests/irp/harness.py writes them).

import json

S = suite
OUTPUT = "bot.ability.reflection.revision.output"
FAILED = "bot.ability.failed"
DB = ("database", "'/memory/floppy.sqlite'")
CHAT = "/v1/chat/completions"
# The builder's progress, which a revision sends its invoker by default.
PROGRESS = ["builder.progress", "generate.progress"]

TURN = [
    ("turn", "1"),
    ("cognition_input", json.dumps({
        "stimulus": {"event": "environment.sound", "data": {"label": "knock"}},
        "focus": "door",
        "focus_candidates": ["door"],
    })),
    ("cognition_output", json.dumps([
        {"event": "bot.ability.communication.respond", "data": {"parts": [{"words": "Hello!"}]}, "reason": "a knock", "target": "communication"}
    ])),
]


def intent(event, name):
    return ("intent", "(" + json.dumps({"event": event, "name": name, "triggers": ["environment.sound"],
                                         "description": "Say hello when a knock sound arrives.",
                                         "reason": "The user asked for it.", "source": None}) + ")")


CREATE = [DB, intent("bot.behavior.create", "GreetOnKnock"), ("intent_event", "'bot.behavior.create'"), ("create", "true")] + TURN
CHANGE_UNKNOWN = [DB, intent("bot.behavior.change", "Nobody"), ("intent_event", "'bot.behavior.change'")] + TURN


def completion(content):
    return {"object": "chat.completion",
            "choices": [{"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": content}}]}


def chat(content, status=200):
    if status != 200:
        return {"path": CHAT, "status": status, "body": {"error": {"message": "boom"}}}
    return {"path": CHAT, "status": 200, "body": completion(content if isinstance(content, str) else json.dumps(content))}


# The builder's answers: its plan and a turn behavior draft.
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
PLAN = chat('{"name":"GreetOnKnock","summary":"Answer a knock with a greeting.","states":[{"id":"answering","kind":"atomic","parent":"","purpose":"Decide the answer."},{"id":"answered","kind":"final","parent":"","purpose":"Carry the answer."}],"events":[],"transitions":[{"source":"answering","event":"","target":"answered","description":"Answer at once."}]}')


def draft(scxml):
    return chat({"scxml": scxml, "schemas": {"$defs": {}}, "script": "",
                 "tests": [{"name": "answers its turn", "events": [], "settle_ms": 2000, "expect_events": [], "expect_done": True}]})


def failed(text):
    return expect(FAILED, f"_event.data.message === {js(text)}")


def failed_prefix(text):
    return expect(FAILED, f"_event.data.message.indexOf({js(text)}) === 0")


S.statement(1, "A create (`create` true) stores a DRAFT stub for its name, invokes the builder bot with a `builder.request` built from the intent and the turn, and on the builder's `done` (`accepting`) replaces the row ACTIVE with the built documents' path (decision D121) and answers `bot.ability.reflection.revision.output` `{behavior}`, the applied behavior: the intent's event, name, grounded triggers, description, reason and that path as `source` (decision D148).")
S.statement(2, "A change of a name the inventory does not hold fails with `bot.ability.failed` `{message: \"Revision selected unknown behavior: <name>.\"}`, asking no model.")
S.statement(3, "A builder that fails with the last draft it rejected (`abandoning`) keeps that draft's path in the row as DRAFT and fails with `Reflection change abandoned: attempts exhausted:` and the rendered diagnostics (decision D133); a builder that fails with no draft fails with `Reflection revision failed: <reason>: <first diagnostic>`.")
S.statement(4, "A memory transaction that fails (here the preparing one, on a database the run does not grant) fails with `Reflection revision failed: <detail>`.")

S.case(100, 1, "a create is built and stored, and the applied behavior names the built documents.",
       [expect(OUTPUT, "_event.data.behavior.name === 'GreetOnKnock' && _event.data.behavior.event === 'bot.behavior.create'"
                       " && _event.data.behavior.triggers.length === 1 && _event.data.behavior.triggers[0] === 'environment.sound'"
                       " && _event.data.behavior.description === 'Say hello when a knock sound arrives.'"
                       " && _event.data.behavior.reason === 'The user asked for it.'"
                       " && /\\.scx$/.test(_event.data.behavior.source)")],
       params=CREATE, backend=[PLAN, draft(BEHAVIOR)], clock="wall", ignore=PROGRESS,
       logs={"reflection.revision": ["create GreetOnKnock"], "reflection.inventory": ["GreetOnKnock ACTIVE turn [\"environment.sound\"] (accept)"]})

S.case(110, 2, "a change of a name the inventory does not hold fails.",
       [failed("Revision selected unknown behavior: Nobody.")],
       params=CHANGE_UNKNOWN, ignore=PROGRESS)

S.case(120, 3, "a build that never validates in its 8 attempts abandons the change with the rendered diagnostics.",
       [failed_prefix("Reflection change abandoned: attempts exhausted:\nbehavior validation failed:\n  error[SCXML005] validate: ")],
       params=CREATE, backend=[PLAN] + [draft(REJECTED)] * 8, clock="wall", ignore=PROGRESS,
       logs={"reflection.inventory": ["GreetOnKnock DRAFT validation (draft)"]})
S.case(121, 3, "a build whose 8 draft generations all fail fails with no draft.",
       [failed_prefix("Reflection revision failed: attempts-exhausted: the author generation failed (status)")],
       params=CREATE, backend=[PLAN] + [chat(None, status=500)] * 8, clock="wall", ignore=PROGRESS)

S.case(130, 4, "a memory transaction on a database the run does not grant fails.",
       [failed("Reflection revision failed: unable to open database file: /nowhere/floppy.sqlite")],
       params=[("database", "'/nowhere/floppy.sqlite'")] + CREATE[1:], ignore=PROGRESS)
