# The persona's statements and cases (tests/irp/harness.py writes them).

import json

S = suite
OUTPUT = "bot.ability.persona.output"
REVISE = "bot.ability.persona.revise"
FAILED = "bot.ability.failed"
GAVE_UP = "Persona could not recall a description: memory did not answer within the bring-up budget."
DB = memory("'/memory/floppy.sqlite'")
NOWHERE = memory("'/nowhere/floppy.sqlite'")
GIVEN = ["curious", "patient with children"]
TRAITS = ("traits", js(["curious", "  patient with children "]))

READY = expect("bot.ability.ready")
# The offer: the revise event alone, with its effort (2), its schema and the
# schema's description, read from its live session (decision D242).
OFFERED = offers("offers.length === 1 && offers[0].event === 'bot.ability.persona.revise' && offers[0].effort === 2"
                 " && offers[0].target === undefined && offers[0].schema.required.join() === 'text,reason'"
                 " && offers[0].schema.properties.text.maxLength === 2000"
                 " && offers[0].schema.properties.reason.maxLength === 500"
                 " && typeof offers[0].description === 'string'")


def compact(value):
    """JSON.stringify of value as it arrives: an event's data has its keys sorted."""
    return json.dumps(value, separators=(",", ":"), sort_keys=True)


def traits(given=GIVEN):
    return f"JSON.stringify(_event.data.traits) === {js(compact(given))}"


def told(text=None, reason=None, given=GIVEN):
    if text is None:
        return expect(OUTPUT, f"{traits(given)} && _event.data.description === null")
    description = {"text": text, "reason": reason, "traits": given, "kind": "fact"}
    return expect(OUTPUT, f"{traits(given)} && JSON.stringify(_event.data.description) === {js(compact(description))}")


def revise(text, reason):
    return send(REVISE, js({"text": text, "reason": reason}))


def failed(message):
    return expect(FAILED, f"_event.data.message === {js(message)}")


S.statement(1, "Without a database it wakes with no description: on entering knowing it sends `bot.ability.persona.output` `{traits, description: null}` to the parent, traits the `<param>` `traits` in the owner's order with the white space around each stripped, then `bot.ability.ready`, once and with no data (decision D242). What it offers, read from its live session (`<decision:offers>`), is the one model-callable `bot.ability.persona.revise` with its effort (`revise_effort`, 2), its schema and the schema's description, no target.")
S.statement(2, "With a database it recalls the most recent description it wrote (query_tags `self_persona`, subject_ref `self`), else none, and tells and is ready the same way; a recall attempt that fails or does not answer within `recall_attempt_timeout_ms` is asked again after `recall_retry_delay_ms`, and when the bring-up budget (`bring_up_timeout_ms`) runs out first it answers `bot.ability.failed` `{message: \"Persona could not recall a description: memory did not answer within the bring-up budget.\"}` and then tells `bot.ability.persona.output` with no description.")
S.statement(3, "`bot.ability.persona.revise` `{text, reason}` writes the description down (with a database; without one nothing is written) before it becomes the bot's, then tells `bot.ability.persona.output` `{traits, description: {text, reason, traits, kind: \"fact\"}}`, the owner's traits it was written under included, and offers it still (it is not ready again); each revision replaces the description before it.")
S.statement(4, "A revise its schema refuses (a blank or empty text or reason, a text over 2000 characters, a reason over 500, any other field) is never routed: the sender gets `error.execution`, and the persona is unchanged.")
S.statement(5, "A revise that cannot be written down is not taken: it answers `bot.ability.failed` `{message: \"Persona could not remember its revision: ...\"}`, tells nothing, keeps the description it had, and offers it still.")
S.statement(6, "A revise that arrives while it is recalling or revising is deferred and taken, in order, once it is back in knowing.")
S.statement(7, "Traits that are not 1 to 24 traits of at most 200 characters (none given included) are refused where the output is created: it is never ready and ends, its invoker getting `done.invoke.persona` with `{message}`.")

S.case(100, 1, "a persona without a database wakes with its owner's traits and no description, then is ready and offers revise.",
       [told(), READY, OFFERED], params=[TRAITS])

S.case(110, 2, "with a fresh database it recalls no description.",
       [told(), READY], params=[TRAITS, DB], clock="wall")
S.case(111, 2, "a database it cannot open fails every recall attempt until the bring-up budget runs out, then it tells no description.",
       [failed(GAVE_UP), told(), READY],
       params=[TRAITS, NOWHERE, ("bring_up_timeout_ms", "1500")], clock="wall")

# A select that runs for seconds: no attempt answers in time.
SLOW = ("description_select", js("SELECT content FROM (WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 20000000) SELECT count(*) AS n, 'x' AS content FROM c) AS slow WHERE ?1 = 'self_persona' AND ?2 = 'self'"))
S.case(112, 2, "attempts that do not answer in time are asked again until the bring-up budget runs out.",
       [failed(GAVE_UP), told(), READY],
       params=[TRAITS, DB, SLOW, ("recall_attempt_timeout_ms", "200"), ("bring_up_timeout_ms", "1500")], clock="wall")

S.case(120, 3, "a revision without a database is told with the owner's traits, and offered again.",
       [told(), READY, revise("I like questions.", "I keep asking them."),
        told("I like questions.", "I keep asking them."), OFFERED],
       params=[TRAITS])
S.case(121, 3, "with a database each revision is written down and told; the second replaces the first.",
       [told(), READY, revise("I like questions.", "I keep asking them."),
        told("I like questions.", "I keep asking them."), OFFERED,
        revise("I am patient.", "I waited today."), told("I am patient.", "I waited today."), OFFERED],
       params=[TRAITS, DB], clock="wall")

S.case(130, 4, "a blank text is refused where it is sent.",
       [told(), READY, revise("   ", "why"), expect("error.execution"), wait("300ms")],
       params=[TRAITS], clock="wall")
S.case(131, 4, "a text over 2000 characters is refused where it is sent.",
       [told(), READY, send(REVISE, "({text: 'a'.repeat(2001), reason: 'long'})"), expect("error.execution"), wait("300ms")],
       params=[TRAITS], clock="wall")
S.case(132, 4, "a reason over 500 characters, or a field the schema does not name, is refused where it is sent.",
       [told(), READY, send(REVISE, "({text: 'me', reason: 'r'.repeat(501)})"), expect("error.execution"),
        send(REVISE, js({"text": "me", "reason": "why", "traits": ["bold"]})), expect("error.execution"),
        revise("me", "why"), told("me", "why"), OFFERED],
       params=[TRAITS], clock="wall")

S.case(140, 5, "a revision its memory cannot write answers bot.ability.failed and keeps no description.",
       [failed(GAVE_UP), told(), READY, revise("I am new.", "I changed."),
        expect(FAILED, "_event.data.message.indexOf('Persona could not remember its revision: ') === 0"), OFFERED],
       params=[TRAITS, NOWHERE, ("bring_up_timeout_ms", "1000")], clock="wall")

S.case(150, 6, "a revise that arrives while recalling is taken once it knows.",
       [revise("I like questions.", "I keep asking them."), told(), READY,
        told("I like questions.", "I keep asking them."), OFFERED],
       params=[TRAITS, DB], clock="wall")
S.case(151, 6, "a revise that arrives while revising waits for the first.",
       [told(), READY, revise("first", "one"), revise("second", "two"),
        told("first", "one"), OFFERED, told("second", "two"), OFFERED],
       params=[TRAITS, DB], clock="wall")

S.case(160, 7, "a persona given no traits is never ready and ends.",
       [expect("done.invoke.persona", "_event.data.message.indexOf('Persona could not tell what it knows of itself') === 0")])
S.case(161, 7, "a trait that is blank once stripped is refused.",
       [expect("done.invoke.persona", "typeof _event.data.message === 'string'")],
       params=[("traits", js(["curious", "   "]))])
S.case(162, 7, "more than 24 traits are refused.",
       [expect("done.invoke.persona", "typeof _event.data.message === 'string'")],
       params=[("traits", "Array.from({length: 25}, function (_, i) { return 'trait ' + i; })")])
S.case(163, 7, "a trait over 200 characters is refused; 200 is not.",
       [expect("done.invoke.persona", "typeof _event.data.message === 'string'")],
       params=[("traits", "['x'.repeat(201)]")])
S.case(164, 7, "24 traits of 200 characters are its owner's to give.",
       [expect(OUTPUT, "_event.data.traits.length === 24 && _event.data.traits[23].length === 200"), READY],
       params=[("traits", "Array.from({length: 24}, function (_, i) { return String(i).padEnd(200, 'x'); })")])
