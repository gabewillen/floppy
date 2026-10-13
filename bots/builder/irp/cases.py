# The builder's statements and cases (tests/irp/harness.py writes them).

import json

S = suite
REQUEST = "builder.request"
DONE = "done.invoke.builder"
CHAT = "/v1/chat/completions"
DECIDE = "/v1/systemone"


def completion(content):
    return {"object": "chat.completion",
            "choices": [{"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": content}}]}


def chat(content=None, status=200):
    if status != 200:
        return {"path": CHAT, "status": status, "body": {"error": {"message": "boom"}}}
    return {"path": CHAT, "status": 200, "body": completion(content if isinstance(content, str) else json.dumps(content))}


def decided(event):
    """The decision service's system_one answer choosing `event` in its selection."""
    return {"path": DECIDE, "status": 200,
            "body": {"model": "fake", "answers": {"selection": {"type": "choice", "choice": event, "confidence": 0.9,
                                                                 "probabilities": {event: 0.9}}},
                     "usage": {"input_tokens": 1, "output_tokens": 1}}}


PLAN = chat({"name": "pinger", "summary": "Answer a ping with a pong, then finish.",
             "states": [{"id": "idle", "kind": "atomic", "parent": "", "purpose": "Wait for a ping."},
                        {"id": "done", "kind": "final", "parent": "", "purpose": "Finished."}],
             "events": [{"name": "ping", "direction": "input", "description": "A ping."},
                        {"name": "pong", "direction": "output", "description": "The answer."}],
             "transitions": [{"source": "idle", "event": "ping", "target": "done", "description": "Answer and finish."}]})

PINGER = """<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event" version="1.0" datamodel="ecmascript" name="pinger" initial="idle">
  <state id="idle">
    <transition event="ping" event:schema='{"type":"object"}' target="done">
      <send target="#_parent" event="pong"/>
    </transition>
  </state>
  <final id="done"/>
</scxml>"""
# SCXML005: a transition target that does not exist.
INVALID = PINGER.replace('target="done"', 'target="nowhere"')
# SCXML013 (a machine-applicable fix): a comma-separated descriptor list.
FIXABLE = PINGER.replace("""event="ping" event:schema='{"type":"object"}'""", 'event="ping,poke"')
PING_TEST = {"name": "a ping is answered", "events": [{"name": "ping", "data": "{}", "delay_ms": 0}],
             "settle_ms": 1000, "expect_events": ["pong"], "expect_done": True}


def draft(scxml, tests=None, schemas=None):
    return chat({"scxml": scxml,
                 "schemas": [{"event": "ping", "schema": '{"type":"object"}'}] if schemas is None else schemas,
                 "tests": [PING_TEST] if tests is None else tests})


# A persistent routine (it sends bot.behavior.tick) whose schedule is
# hourly.every, armed with `delay`, ticking with `output` (the builder_bot test's).
def routine(delay, output):
    return f"""<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" name="Hourly" initial="waiting">
  <state id="waiting">
    <onentry><send id="every" event="hourly.every" delay="{delay}"/></onentry>
    <onexit><cancel sendid="every"/></onexit>
    <transition event="hourly.every" target="waiting">
      <send event="bot.behavior.tick" target="#_parent">
        <content expr="({{name: 'Hourly', trigger: 'every', scheduled_at: new Date().toISOString(), fired_at: new Date().toISOString(), output: {output}}})"/>
      </send>
    </transition>
  </state>
</scxml>"""


CHIME = "[{event: 'bot.speaking.say', data: {text: 'Hourly chime.'}}]"
WAITS = [{"name": "waits", "events": [], "settle_ms": 500, "expect_events": [], "expect_done": False}]
BOUNDS = {"min_every_ms": 60000, "turn_after_bound_ms": 1000}


def request(**fields):
    data = {"spec": "a pinger: answer a ping with pong and finish"}
    data.update(fields)
    return js(data)


def failed(reason, extra=""):
    return expect(DONE, f"_event.data.reason === {js(reason)} && Array.isArray(_event.data.history)" + (f" && {extra}" if extra else ""))


def diagnostic(kind, code):
    return f"_event.data.diagnostics.some(function (d) {{ return d.kind === {js(kind)} && d.code === {js(code)}; }})"


def built(extra=""):
    return expect(DONE, "typeof _event.data.scxml === 'string' && _event.data.scxml.indexOf('pinger') >= 0"
                        " && _event.data.report.simulation.every(function (r) { return r.passed; })" + (f" && {extra}" if extra else ""))


S.statement(1, "It waits in `intake` for `builder.request` `{spec, constraints?, max_attempts?, model?, generate_timeout_ms?, generate_ceiling_ms?, min_every_ms?, turn_after_bound_ms?}`; data its schema refuses is refused at the sender (its send raises `error.execution` there) and the builder keeps waiting; a request whose spec is only whitespace ends in the final state `failed` with `{reason: \"invalid-request\", diagnostics: [{kind: \"request\", code: \"invalid-request\"}]}`, which the invoker receives as `done.invoke.<id>`.")
S.statement(2, "A request is planned (`design`, schema `topology_plan`), authored (`author`, schema `scxml_draft`), validated (`validate`) and simulated (`simulate`, each test script driving the draft as an invoked child); a draft that validates and passes every test ends in the final state `done` with `{scxml, schemas, tests, report: {attempts, failures, fixes_applied, warnings, plan, decisions, history, simulation}}`, its schemas parsed from their JSON text.")
S.statement(3, "A draft whose validator errors all have machine-applicable fixes is fixed (`apply_fixes`, at most 2 passes per draft) and validated again, without a new attempt: `report.fixes_applied` counts it.")
S.statement(4, "Every failure goes through `retry_gate`, then `triage`, which asks the decision service to choose among its enabled transitions: `triage.repair` authors the draft again from the same plan (`report.decisions` `[\"triage.repair\"]`), `triage.redesign` plans again, `triage.abandon` ends in `failed` `{reason: \"abandoned\"}`. When the decision is unavailable it falls back (`fallback`): repair when there is a plan, else redesign.")
S.statement(5, "A failure whose diagnostics equal the previous failure's ends in `failed` `{reason: \"no-progress\"}` with those diagnostics and, when a check rejected a draft, its `scxml`; at the request's `max_attempts` failures it ends in `failed` `{reason: \"attempts-exhausted\"}`.")
S.statement(6, "Each check has its own diagnostics: a validator error (`kind` `validate`, its code), a schemas entry that is not JSON Schema text (`draft` `schema-not-json`), test scripts out of bounds (`draft` `tests-out-of-bounds`, one to eight), and a test whose outputs or done differ from its expectations (`simulate` `test-failed`).")
S.statement(7, "A generation that fails (`generate.failed`) is a `generate` diagnostic `<stage>-failed`; one that runs `generate_ceiling_ms` in all, or, once its reply streams, makes no progress for `generate_timeout_ms` (the first `generate.progress` arms it, each later one re-arms it), is cancelled, a `timeout` diagnostic `<stage>-timeout` (decision D101).")
S.statement(8, "With the request's lifetime bounds (`min_every_ms`, `turn_after_bound_ms`), a draft that sends `bot.behavior.tick` (a persistent routine) whose schedule is shorter than `min_every_ms` is a `lifetime` diagnostic `E0009`; a routine within the bounds also gets a forced first tick, whose output must select at least one event (else an `apply` diagnostic `E0008`).")

S.case(100, 1, "a request refused by its schema raises error.execution at the sender; the builder keeps waiting and answers a whitespace spec invalid-request.",
       [send(REQUEST, js({"spec": "x", "colour": "blue"})), expect("error.execution"),
        send(REQUEST, js({"spec": "   "})),
        failed("invalid-request", diagnostic("request", "invalid-request") + " && _event.data.history.length === 0 && _event.data.scxml === undefined")])

S.case(110, 2, "a plan and a valid draft are built in one attempt.",
       [send(REQUEST, request()),
        built("_event.data.report.attempts === 1 && _event.data.report.failures === 0 && _event.data.report.fixes_applied === 0"
              " && _event.data.report.plan.name === 'pinger' && _event.data.report.decisions.length === 0"
              " && _event.data.schemas[0].event === 'ping' && _event.data.schemas[0].schema.type === 'object'"
              " && _event.data.tests[0].name === 'a ping is answered' && _event.data.report.simulation.length === 1")],
       backend=[PLAN, draft(PINGER)], clock="wall")

S.case(120, 3, "a draft with a machine-applicable fix is fixed and built in one attempt.",
       [send(REQUEST, request()),
        built("_event.data.report.attempts === 1 && _event.data.report.failures === 0 && _event.data.report.fixes_applied === 1"
              " && _event.data.scxml.indexOf('ping poke') >= 0")],
       backend=[PLAN, draft(FIXABLE, schemas=[])], clock="wall")

S.case(130, 4, "the decision chooses triage.repair: the draft is authored again and built in the second attempt.",
       [send(REQUEST, request()),
        built("_event.data.report.attempts === 2 && _event.data.report.failures === 1"
              " && JSON.stringify(_event.data.report.decisions) === '[\"triage.repair\"]'")],
       backend=[PLAN, draft(INVALID), draft(PINGER), decided("triage.repair")], clock="wall")
S.case(131, 4, "the decision chooses triage.redesign: the topology is planned again, then built.",
       [send(REQUEST, request()),
        built("_event.data.report.attempts === 2 && _event.data.report.failures === 1"
              " && JSON.stringify(_event.data.report.decisions) === '[\"triage.redesign\"]'")],
       backend=[PLAN, draft(INVALID), PLAN, draft(PINGER), decided("triage.redesign")], clock="wall")
S.case(132, 4, "the decision chooses triage.abandon: the builder ends in failed abandoned.",
       [send(REQUEST, request()),
        failed("abandoned", diagnostic("validate", "SCXML005") + " && _event.data.scxml.indexOf('nowhere') >= 0")],
       backend=[PLAN, draft(INVALID), decided("triage.abandon")], clock="wall")
S.case(133, 4, "the decision unavailable (the backend answers 404), the fallback repairs from the plan.",
       [send(REQUEST, request()),
        built("_event.data.report.attempts === 2 && JSON.stringify(_event.data.report.decisions) === '[\"fallback:unavailable\"]'")],
       backend=[PLAN, draft(INVALID), draft(PINGER)], clock="wall")
S.case(134, 4, "the decision unavailable and no plan yet (the plan generation failed), the fallback redesigns.",
       [send(REQUEST, request()),
        built("_event.data.report.attempts === 1 && _event.data.report.failures === 1"
              " && JSON.stringify(_event.data.report.decisions) === '[\"fallback:unavailable\"]' && _event.data.report.plan.name === 'pinger'")],
       backend=[chat(status=500), PLAN, draft(PINGER)], clock="wall")

S.case(140, 5, "the same validator error twice is no progress: failed with the rejected draft.",
       [send(REQUEST, request()),
        failed("no-progress", diagnostic("validate", "SCXML005") + " && _event.data.scxml.indexOf('nowhere') >= 0 && _event.data.history.length === 2")],
       backend=[PLAN, draft(INVALID)], clock="wall")
S.case(141, 5, "max_attempts 1 ends at the first failure: attempts-exhausted.",
       [send(REQUEST, request(max_attempts=1)),
        failed("attempts-exhausted", diagnostic("validate", "SCXML005") + " && _event.data.history.length === 1")],
       backend=[PLAN, draft(INVALID)], clock="wall")

S.case(150, 6, "a schemas entry that is not JSON Schema text is a draft schema-not-json diagnostic.",
       [send(REQUEST, request(max_attempts=1)),
        failed("attempts-exhausted", diagnostic("draft", "schema-not-json"))],
       backend=[PLAN, draft(PINGER, schemas=[{"event": "ping", "schema": "not json"}])], clock="wall")
S.case(151, 6, "a draft with no test script is a draft tests-out-of-bounds diagnostic.",
       [send(REQUEST, request(max_attempts=1)),
        failed("attempts-exhausted", diagnostic("draft", "tests-out-of-bounds"))],
       backend=[PLAN, draft(PINGER, tests=[])], clock="wall")
S.case(152, 6, "a test whose expected outputs never come is a simulate test-failed diagnostic, with the draft.",
       [send(REQUEST, request(max_attempts=1)),
        failed("attempts-exhausted", diagnostic("simulate", "test-failed") + " && _event.data.scxml.indexOf('pinger') >= 0")],
       backend=[PLAN, draft(PINGER, tests=[dict(PING_TEST, expect_events=["pang"])])], clock="wall")

S.case(160, 7, "a design generation that fails is a generate design-failed diagnostic; twice is no progress, with no draft.",
       [send(REQUEST, request()),
        failed("no-progress", diagnostic("generate", "design-failed") + " && _event.data.scxml === undefined")],
       backend=[chat(status=500)], clock="wall")
S.case(161, 7, "an author generation that fails is a generate author-failed diagnostic.",
       [send(REQUEST, request(max_attempts=1)),
        failed("attempts-exhausted", diagnostic("generate", "author-failed"))],
       backend=[PLAN, chat(status=500)], clock="wall")
S.case(162, 7, "a design generation that outlasts generate_ceiling_ms is a timeout design-timeout diagnostic.",
       [send(REQUEST, request(generate_ceiling_ms=1, max_attempts=1)),
        failed("attempts-exhausted", diagnostic("timeout", "design-timeout"))],
       backend=[PLAN, draft(PINGER)], clock="wall")

S.case(170, 8, "with the lifetime bounds, a routine scheduled every 30 s is a lifetime E0009 diagnostic.",
       [send(REQUEST, request(max_attempts=1, **BOUNDS)),
        failed("attempts-exhausted", diagnostic("lifetime", "E0009"))],
       backend=[PLAN, draft(routine("30s", CHIME), tests=WAITS, schemas=[])], clock="wall")
S.case(171, 8, "with the lifetime bounds, an hourly routine passes its forced first tick and is built.",
       [send(REQUEST, request(**BOUNDS)),
        expect(DONE, "_event.data.scxml.indexOf('bot.behavior.tick') >= 0 && _event.data.report.simulation.length === 2"
                     " && _event.data.report.simulation[1].name === 'first tick' && _event.data.report.simulation[1].passed === true")],
       backend=[PLAN, draft(routine("3600s", CHIME), tests=WAITS, schemas=[])], clock="wall")
S.case(172, 8, "with the lifetime bounds, a routine whose forced tick selects nothing is an apply E0008 diagnostic.",
       [send(REQUEST, request(max_attempts=1, **BOUNDS)),
        failed("attempts-exhausted", diagnostic("apply", "E0008"))],
       backend=[PLAN, draft(routine("3600s", "[]"), tests=WAITS, schemas=[])], clock="wall")
S.case(135, 4, "each failure is triaged again: two different failures, two decisions, then built in the third attempt.",
       [send(REQUEST, request()),
        built("_event.data.report.attempts === 3 && _event.data.report.failures === 2"
              " && JSON.stringify(_event.data.report.decisions) === '[\"triage.repair\",\"triage.repair\"]'")],
       backend=[PLAN, draft(INVALID), draft(PINGER, schemas=[{"event": "ping", "schema": "not json"}]), draft(PINGER),
                decided("triage.repair")], clock="wall")
