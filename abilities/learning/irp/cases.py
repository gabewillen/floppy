# The learning's statements and cases (tests/irp/harness.py writes them).

import json

S = suite
LESSON = "bot.ability.learning.lesson"
OUTPUT = "bot.ability.learning.output"
DB = ("database", "'/memory/floppy.sqlite'")
# Generous bounds for the wall-clock cases, so a loaded machine does not time them out.
SLOW_SELECT = ("select_timeout_ms", "300000")
SLOW_REVISING = ("revising_timeout_ms", "300000")
CHAT = "/v1/chat/completions"
TEACHER = {"id": "gabe", "kind": "person"}


def lesson(turn, content="When you hear a knock, say hello.", media_type=None):
    return js({
        "turn": turn,
        "content": content,
        "media_type": media_type,
        "cognition_input": {
            "stimulus": {"event": "environment.sound", "data": {"label": "knock"}},
            "focus": "door",
            "focus_candidates": ["door"],
        },
        "cognition_output": [
            {"event": "bot.ability.communication.respond", "data": {"parts": [{"words": "Hello!"}]}, "reason": "a knock", "target": "communication"},
            {"event": "bot.ability.learning.input", "data": {"content": content}, "reason": "I was taught", "target": "cognition"},
        ],
        "participants": [TEACHER],
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


# The learning's own session id: its choices are its own transitions, so
# each selection names it as its target.
OWN = "session.learning"
CREATE = {"event": "bot.behavior.create", "target": OWN,
          "data": {"event": "bot.behavior.create", "name": "GreetOnKnock",
                   "triggers": ["environment.sound"],
                   "description": "Say hello when a knock sound arrives.",
                   "reason": "I was taught to greet a knock.", "source": None},
          "reason": "the lesson"}

BREAK = {"event": "bot.behavior.break", "target": OWN,
         "data": {"event": "bot.behavior.break", "name": "GreetOnKnock", "reason": "I was asked to stop greeting a knock."},
         "reason": "the lesson"}

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


def not_learned(content="When you hear a knock, say hello.", turn=1):
    return expect(OUTPUT, f"_event.data.learned === false && _event.data.behavior === null && _event.data.lesson === {js(content)}"
                          f" && _event.data.lesson_turn === {turn}"
                          f" && _event.data.participants[0].id === 'gabe'")


LEARNED = expect(OUTPUT, "_event.data.learned === true && _event.data.lesson_turn === 1 && _event.data.behavior.name === 'GreetOnKnock'"
                         " && _event.data.behavior.status === 'ACTIVE'"
                         " && _event.data.behavior.description === 'Say hello when a knock sound arrives.'"
                         " && _event.data.participants[0].id === 'gabe'")

S.statement(1, "A `bot.ability.learning.lesson` `{turn, content, media_type, cognition_input, cognition_output, participants}` recalls the prior episodes and the inventory, asks the generate service (schema `dispatch`, its `events` at most one) to implement the lesson as a `bot.behavior.create`, `bot.behavior.change` or `bot.behavior.break` this session offers, and invokes the revision ability with the teaching turn and the lesson as its instruction; a built behavior answers `bot.ability.learning.output` `{lesson, lesson_turn, learned: true, behavior: {name, description, status: \"ACTIVE\"}, participants}`, lesson_turn the teaching turn's, and logs `learning.learned` with `lesson of turn <turn>: ` before the behavior's name and source (decisions D149 and D219).")
S.statement(2, "A lesson that does not take answers `bot.ability.learning.output` with `learned` false and `behavior` null, its failure only logged (`learning.failed`, `lesson of turn <turn>: ` before why): without a database, for a lesson that is not text, for a reply that selects nothing, and for a build the revision fails.")
S.statement(3, "A lesson that arrives while another is in hand waits and runs after it, in order: each lesson gets its own answer.")
S.statement(4, "A lesson implemented as `bot.behavior.break` sets the named behavior's status BROKEN with the trimmed reason, building nothing, and answers `bot.ability.learning.output` with `learned` true and the behavior's `status` `BROKEN`; a break of a name the inventory does not hold is not learned (decision D220).")

S.case(100, 1, "a lesson is built and stored, and the output says it was learned.",
       [send(LESSON, lesson(1)), LEARNED],
       params=[DB, SLOW_SELECT, SLOW_REVISING], backend=[select(CREATE), PLAN, draft(BEHAVIOR)], clock="wall",
       logs={"reflection.revision": ["create GreetOnKnock"], "learning.selected": ["bot.behavior.create"]})

S.case(110, 2, "without a database the lesson is not learned.",
       [send(LESSON, lesson(1)), not_learned()],
       logs={"learning.failed": ["lesson of turn 1: Learning memory recall failed: no database."]})
S.case(111, 2, "a lesson that is not text is not learned, asking no model.",
       [send(LESSON, lesson(1, media_type="audio/wav")), not_learned()],
       params=[DB], logs={"learning.failed": ["lesson of turn 1: Learning cannot read audio/wav content: a lesson is text."]})
S.case(112, 2, "a reply that selects nothing is not learned.",
       [send(LESSON, lesson(1)), not_learned()],
       params=[DB, SLOW_SELECT, SLOW_REVISING], backend=[select()], clock="wall",
       logs={"learning.failed": ["lesson of turn 1: Learning select returned an empty selection."]})
S.case(113, 2, "a build that never validates is not learned, and why is only logged.",
       [send(LESSON, lesson(1)), not_learned()],
       params=[DB, SLOW_SELECT, SLOW_REVISING], backend=[select(CREATE), PLAN] + [draft(REJECTED)] * 8, clock="wall")

S.case(120, 3, "a second lesson sent while the first is in hand runs after it.",
       [send(LESSON, lesson(1)), send(LESSON, lesson(2, content="When you hear a knock, say hi.")),
        not_learned(), not_learned("When you hear a knock, say hi.", turn=2)],
       params=[DB, SLOW_SELECT, SLOW_REVISING], backend=[select(), select()], clock="wall")

S.case(130, 4, "a lesson to stop a learned behavior breaks it, and the output says it stopped.",
       [send(LESSON, lesson(1)), LEARNED,
        send(LESSON, lesson(2, content="Stop greeting a knock.")),
        expect(OUTPUT, "_event.data.learned === true && _event.data.lesson_turn === 2 && _event.data.behavior.name === 'GreetOnKnock'"
                       " && _event.data.behavior.status === 'BROKEN'")],
       params=[DB, SLOW_SELECT, SLOW_REVISING], backend=[select(CREATE), PLAN, draft(BEHAVIOR), select(BREAK)], clock="wall",
       logs={"learning.selected": ["bot.behavior.create", "bot.behavior.break"]})
