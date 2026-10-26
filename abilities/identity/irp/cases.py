# The identity's statements and cases (tests/irp/harness.py writes them).

S = suite
# hey.wav: 0.1 s of 16 kHz mono 16-bit WAV, constant non-zero samples.
S.media({"hey.wav": wav(1000)})
ADOPT = "bot.ability.identity.adopt"
INPUT = "bot.ability.communication.input"
SOUND = "environment.sound"
HANDOFF = "bot.ability.cognition.input"
FAILED = "bot.ability.failed"
ASK = "bot.ability.identity.ask"
NAMED = "bot.ability.identity.named"
GAVE_UP = "Identity could not recall a name: memory did not answer within the bring-up budget."
DB = ("database", "'/memory/floppy.sqlite'")
TRANSCRIPTIONS = "/audio/transcriptions"

READY = expect("bot.ability.ready")
# The offer: the adopt event alone, with its effort (1), its schema and the
# schema's description, read from its live session (decision D242).
OFFERED = offers("offers.length === 1 && offers[0].event === 'bot.ability.identity.adopt' && offers[0].effort === 1"
                 " && offers[0].target === undefined && offers[0].schema.required[0] === 'name'"
                 " && typeof offers[0].description === 'string'")


def named(name):
    return ("name", js(name))


def said(text, content_type="text/plain"):
    return js({"source_ids": ["gabe"], "target_ids": ["floppy"], "content": text, "content_type": content_type})


def addressed(name):
    return (f"_event.data.stimulus.event === 'bot.ability.identity.addressed'"
            f" && _event.data.stimulus.data.name === {js(name)} && _event.data.stimulus.source === 'identity'")


def sound(audio="media/hey.wav"):
    return js({"audio": audio, "media_type": "audio/wav", "sample_rate_hz": 16000, "channels": 1})


S.statement(1, "Without a database it recalls the name it was started with (the `<param>` `name`, else none) and is ready: on entering knowing it sends `bot.ability.ready` to its parent, once and with no data (decision D242). What it offers, read from its live session (`<decision:offers>`), is the one model-callable `bot.ability.identity.adopt` with its effort (`adopt_effort`, 1), its schema and the schema's description, no target.")
S.statement(2, "With a database it recalls the last adopted name from memory (a row that does not decode as a name is skipped), else the name it was started with, and is ready with the same offer; a recall attempt that fails or does not answer within `recall_attempt_timeout_ms` is asked again after `recall_retry_delay_ms`, and when the bring-up budget (`bring_up_timeout_ms`) runs out first it answers `bot.ability.failed` `{message: \"Identity could not recall a name: memory did not answer within the bring-up budget.\"}` and then recalls the name it was started with.")
S.statement(3, "A named identity that hears its name in `bot.ability.communication.input` text (string content of a `text/*` content_type; the name's words, split at white space, case-folded and keeping only their letters and digits, appear as a whole run of the text's) hands the body `bot.ability.cognition.input` `{stimulus}`, the stimulus `{event: \"bot.ability.identity.addressed\", data: {name}, source: \"identity\"}`.")
S.statement(4, "Nothing else it hears produces anything: text that does not name it (the name only as part of a word included), content that is not `text/*`, and anything heard while it has no name.")
S.statement(5, "`bot.ability.identity.adopt` `{name, reason?}` takes the name and re-enters knowing, offering it still (it is not ready again); without a database nothing is written, with one the name is written to memory before it is taken. From then on it answers to that name. A write that fails answers `bot.ability.failed` `{message: \"Identity could not remember the name it took: ...\"}` and re-enters knowing with the old name.")
S.statement(6, "It takes no sound (#89): words heard out loud reach it the way typed words do, as `bot.ability.communication.input` once the listening has decoded them and the communication has taken them, so an `environment.sound` sent to it produces nothing and is never decoded.")
S.statement(7, "Communication input and adopt events that arrive while it is recalling or adopting are deferred and taken, in order, once it is back in knowing.")
S.statement(8, "`bot.ability.identity.ask` `{}` while knowing is answered with `bot.ability.identity.named` `{name}` to the parent, the name it answers to or null; an ask that arrives while recalling or adopting is deferred and answered once it knows; and every adoption sends `bot.ability.identity.named` with the adopted name once it is taken.")

S.case(100, 1, "a nameless identity without a database is ready with its adopt offer.", [READY, OFFERED])
S.case(101, 1, "an identity started with a name is ready with the same offer and answers to that name.",
       [READY, send(INPUT, said("hey Floppy, hi")), expect(HANDOFF, addressed("Floppy"))],
       params=[named("Floppy")])

S.case(110, 2, "with a fresh database it recalls nothing and is ready; the started name stands.",
       [READY, send(INPUT, said("Floppy?")), expect(HANDOFF, addressed("Floppy"))],
       params=[DB, named("Floppy")], clock="wall")
S.case(111, 2, "a database it cannot open fails every recall attempt until the bring-up budget runs out, then the started name is recalled.",
       [expect(FAILED, f"_event.data.message === {js(GAVE_UP)}"),
        READY, send(INPUT, said("hey floppy")), expect(HANDOFF, addressed("Floppy"))],
       params=[("database", "'/nowhere/floppy.sqlite'"), named("Floppy"), ("bring_up_timeout_ms", "1500")], clock="wall")
# Stored rows stand in for memory's, oldest first: a good name, then rows
# that are not JSON, hold an empty name, or carry a key a name has not.
DAMAGED = ("name_select", js('SELECT content FROM (SELECT 1 AS n, \'{"text": "Pip", "kind": "fact"}\' AS content UNION ALL SELECT 2, \'{"text": "\' UNION ALL SELECT 3, \'{"text": "", "kind": "fact"}\' UNION ALL SELECT 4, \'{"text": "Bo", "kind": "fact", "reason": "x"}\') AS stored WHERE ?1 = \'self_name\' AND ?2 = \'self\' ORDER BY n'))
# A name stored without its kind, the newest, is a fact.
KINDLESS = ("name_select", js('SELECT content FROM (SELECT 1 AS n, \'{"text": "Pip", "kind": "fact"}\' AS content UNION ALL SELECT 2, \'{"text": "Bo"}\') AS stored WHERE ?1 = \'self_name\' AND ?2 = \'self\' ORDER BY n'))
# A select that runs for seconds: no attempt answers in time.
SLOW = ("name_select", js("SELECT content FROM (WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 20000000) SELECT count(*) AS n, 'x' AS content FROM c) AS slow WHERE ?1 = 'self_name' AND ?2 = 'self'"))
S.case(112, 2, "attempts that do not answer in time are asked again until the bring-up budget runs out, then the started name is recalled.",
       [expect(FAILED, f"_event.data.message === {js(GAVE_UP)}"),
        READY, send(INPUT, said("hey floppy")), expect(HANDOFF, addressed("Floppy"))],
       params=[DB, SLOW, named("Floppy"), ("recall_attempt_timeout_ms", "200"), ("bring_up_timeout_ms", "1500")], clock="wall")
S.case(113, 2, "rows that do not decode as a name are skipped, so the name adopted before them is recalled.",
       [READY, send(INPUT, said("hey pip")), expect(HANDOFF, addressed("Pip"))],
       params=[DB, DAMAGED], clock="wall")
S.case(114, 2, "a row without a kind is a name.",
       [READY, send(INPUT, said("hey bo")), expect(HANDOFF, addressed("Bo"))],
       params=[DB, KINDLESS], clock="wall")

S.case(120, 3, "its name in text addresses it.",
       [READY, send(INPUT, said("hey floppy, what's your name?")), expect(HANDOFF, addressed("Floppy"))],
       params=[named("Floppy")])
S.case(121, 3, "a multi-word name is heard as a whole run of words, case-folded, punctuation dropped.",
       [READY, send(INPUT, said("Is that you, MR. Floppy 2?")), expect(HANDOFF, addressed("Mr Floppy 2"))],
       params=[named("Mr Floppy 2")])
S.case(122, 3, "any text/* content type is heard.",
       [READY, send(INPUT, said("# Floppy", "text/markdown")), expect(HANDOFF, addressed("Floppy"))],
       params=[named("Floppy")])

S.case(130, 4, "a name that is only part of a word does not address it.",
       [READY, send(INPUT, said("always")), wait("500ms")],
       params=[named("Al")], clock="wall")
S.case(131, 4, "content that is not text/* is not heard, even when it carries the name.",
       [READY, send(INPUT, said("Floppy", "application/json")), wait("500ms"),
        send(INPUT, said("Floppy")), expect(HANDOFF, addressed("Floppy"))],
       params=[named("Floppy")], clock="wall")
S.case(132, 4, "a nameless identity only hears words.",
       [READY, send(INPUT, said("Floppy, hello")), wait("500ms")], clock="wall")

S.case(133, 4, "a name heard inside a hyphenated word does not address it (the word is one token).",
       [READY, send(INPUT, said("hi Floppy-2")), wait("500ms")],
       params=[named("Floppy")], clock="wall")

S.case(140, 5, "an adopted name is taken without a database, offered again, and answered to.",
       [READY, send(ADOPT, js({"name": "Floppy", "reason": "asked"})), expect(NAMED, "_event.data.name === 'Floppy'"), OFFERED,
        send(INPUT, said("hi floppy")), expect(HANDOFF, addressed("Floppy"))])
S.case(141, 5, "with a database the adopted name is written down, then answered to; a second adoption replaces it.",
       [READY, send(ADOPT, js({"name": "Floppy"})), expect(NAMED, "_event.data.name === 'Floppy'"), OFFERED,
        send(ADOPT, js({"name": "Pip"})), expect(NAMED, "_event.data.name === 'Pip'"), OFFERED,
        send(INPUT, said("Floppy?")), wait("500ms"),
        send(INPUT, said("Pip?")), expect(HANDOFF, addressed("Pip"))],
       params=[DB], clock="wall")
S.case(142, 5, "an adopt whose data its schema refuses is never routed: the sender gets error.execution.",
       [READY, send(ADOPT, js({"name": ""})), expect("error.execution"),
        send(INPUT, said("hello")), wait("300ms")],
       clock="wall")

S.case(143, 5, "a name it cannot write down is not taken: the failure is answered and it offers again.",
       [expect(FAILED, f"_event.data.message === {js(GAVE_UP)}"), READY,
        send(ADOPT, js({"name": "Pip"})),
        expect(FAILED, "_event.data.message.indexOf('Identity could not remember the name it took: ') === 0"),
        OFFERED, send(ASK, js({})), expect(NAMED, "_event.data.name === null")],
       params=[("database", "'/nowhere/floppy.sqlite'"), ("bring_up_timeout_ms", "1500")], clock="wall")

S.case(150, 6, "a sound produces nothing and decodes nothing; its name in text still addresses it.",
       [READY, send(SOUND, sound()), wait("1s"), send(INPUT, said("Floppy")), expect(HANDOFF, addressed("Floppy"))],
       params=[named("Floppy")],
       backend=[{"path": TRANSCRIPTIONS, "status": 500, "body": {"error": {"message": "boom"}}}], clock="wall")

S.case(170, 7, "input that arrives while recalling from memory is taken once it knows its name.",
       [send(INPUT, said("Floppy?")), READY, expect(HANDOFF, addressed("Floppy"))],
       params=[DB, named("Floppy")], clock="wall")
S.case(171, 7, "input that arrives while adopting is heard with the adopted name.",
       [READY, send(ADOPT, js({"name": "Pip"})), send(INPUT, said("Pip?")),
        expect(NAMED, "_event.data.name === 'Pip'"), OFFERED, expect(HANDOFF, addressed("Pip"))],
       params=[DB], clock="wall")
S.case(180, 8, "a nameless identity is asked and answers that it has no name.",
       [READY, send(ASK, js({})), expect(NAMED, "_event.data.name === null")])
S.case(181, 8, "a named identity is asked and answers its name.",
       [READY, send(ASK, js({})), expect(NAMED, "_event.data.name === 'Floppy'")],
       params=[named("Floppy")])
S.case(182, 8, "an ask that arrives while recalling is answered with the recalled name once it knows.",
       [send(ASK, js({})), READY, expect(NAMED, "_event.data.name === 'Floppy'")],
       params=[DB, named("Floppy")], clock="wall")
S.case(183, 8, "an adoption tells the name; an ask that arrived while adopting is answered with it.",
       [READY, send(ADOPT, js({"name": "Pip"})), send(ASK, js({})),
        expect(NAMED, "_event.data.name === 'Pip'"), OFFERED, expect(NAMED, "_event.data.name === 'Pip'")],
       params=[DB], clock="wall")
