# The identity's statements and cases (tests/irp/harness.py writes them).

S = suite
# hey.wav: 0.1 s of 16 kHz mono 16-bit WAV, constant non-zero samples.
S.media({"hey.wav": wav(1000)})
OFFERS = "bot.ability.offers"
ADOPT = "bot.ability.identity.adopt"
INPUT = "bot.ability.communication.input"
SOUND = "environment.sound"
HANDOFF = "bot.ability.cognition.input"
FAILED = "bot.ability.failed"
ASK = "bot.ability.identity.ask"
NAMED = "bot.ability.identity.named"
GAVE_UP = "Identity could not recall a name: memory did not answer within the bring-up budget."
DB = ("database", "'/memory/floppy.sqlite'")
INTERACTIONS = "/interactions"

# The offer: the adopt event alone, its schema compacted ($schema, title
# and examples dropped), with the schema's description.
READY = expect(OFFERS, "_event.data.ability === 'identity' && _event.data.choices.length === 1"
                       " && _event.data.choices[0].event === 'bot.ability.identity.adopt'"
                       " && _event.data.choices[0].target === undefined"
                       " && _event.data.choices[0].schema.required[0] === 'name'"
                       " && _event.data.choices[0].schema.title === undefined"
                       " && _event.data.choices[0].schema['$schema'] === undefined"
                       " && _event.data.choices[0].schema.properties.reason.examples === undefined"
                       " && typeof _event.data.choices[0].description === 'string'")


def named(name):
    return ("name", js(name))


def said(text, content_type="text/plain"):
    return js({"source_ids": ["gabe"], "target_ids": ["floppy"], "content": text, "content_type": content_type})


def addressed(name):
    return (f"_event.data.stimulus.event === 'bot.ability.identity.addressed'"
            f" && _event.data.stimulus.data.name === {js(name)} && _event.data.stimulus.source === 'identity'")


def sound(audio="media/hey.wav"):
    return js({"audio": audio, "media_type": "audio/wav", "sample_rate_hz": 16000, "channels": 1})


def transcript(text):
    return {"path": INTERACTIONS, "status": 200,
            "body": {"steps": [{"type": "model_output", "content": [{"type": "text", "text": text}]}]}}


S.statement(1, "Without a database it recalls the name it was started with (the `<param>` `name`, else none) and is ready: on entering knowing it sends `bot.ability.offers` `{ability: \"identity\", choices}`, choices the one model-callable `bot.ability.identity.adopt` with its schema compacted (no `$schema`, title or examples) and the schema's description, no target.")
S.statement(2, "With a database it recalls the last adopted name from memory (a row that does not decode as a name is skipped), else the name it was started with, and is ready with the same offer; a recall attempt that fails or does not answer within `recall_attempt_timeout_ms` is asked again after `recall_retry_delay_ms`, and when the bring-up budget (`bring_up_timeout_ms`) runs out first it answers `bot.ability.failed` `{message: \"Identity could not recall a name: memory did not answer within the bring-up budget.\"}` and then recalls the name it was started with.")
S.statement(3, "A named identity that hears its name in `bot.ability.communication.input` text (string content of a `text/*` content_type; the name's words, split at white space, case-folded and keeping only their letters and digits, appear as a whole run of the text's) hands the body `bot.ability.cognition.input` `{stimulus}`, the stimulus `{event: \"bot.ability.identity.addressed\", data: {name}, source: \"identity\"}`.")
S.statement(4, "Nothing else it hears produces anything: text that does not name it (the name only as part of a word included), content that is not `text/*`, and anything heard while it has no name.")
S.statement(5, "`bot.ability.identity.adopt` `{name, reason?}` takes the name and re-enters knowing, offering again; without a database nothing is written, with one the name is written to memory before it is taken. From then on it answers to that name. A write that fails answers `bot.ability.failed` `{message: \"Identity could not remember the name it took: ...\"}` and re-enters knowing with the old name.")
S.statement(6, "A named identity decodes each `environment.sound` through the speech decoding (the speech service) and looks for its name in the transcript: addressed, it hands the body the same `bot.ability.cognition.input` as for text; a nameless one decodes nothing.")
S.statement(7, "A decoding that fails answers `bot.ability.failed` `{message}` (the speech decoding's failure message) and the identity goes back to listening.")
S.statement(8, "Communication input, sounds and adopt events that arrive while it is recalling or adopting, and input and sounds that arrive while it is recognizing, are deferred and taken, in order, once it is back in knowing (or listening).")
S.statement(9, "`bot.ability.identity.ask` `{}` while knowing is answered with `bot.ability.identity.named` `{name}` to the parent, the name it answers to or null; an ask that arrives while recalling or adopting is deferred and answered once it knows; and every adoption sends `bot.ability.identity.named` with the adopted name once it is taken.")

S.case(100, 1, "a nameless identity without a database is ready with its adopt offer.", [READY])
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
# Stored rows stand in for memory's: the newest is not JSON, the one
# before it holds no name, the oldest is a good one.
DAMAGED = ("name_select", js('SELECT content FROM (SELECT 1 AS n, \'{"text": "Pip", "kind": "fact"}\' AS content UNION ALL SELECT 2, \'{"text": "\' UNION ALL SELECT 3, \'{"text": "", "kind": "fact"}\') AS stored WHERE ?1 = \'self_name\' AND ?2 = \'self\' ORDER BY n'))
S.case(113, 2, "rows that do not decode as a name are skipped, so the name adopted before them is recalled.",
       [READY, send(INPUT, said("hey pip")), expect(HANDOFF, addressed("Pip"))],
       params=[DB, DAMAGED], clock="wall")
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
       [READY, send(ADOPT, js({"name": "Floppy", "reason": "asked"})), expect(NAMED, "_event.data.name === 'Floppy'"), READY,
        send(INPUT, said("hi floppy")), expect(HANDOFF, addressed("Floppy"))])
S.case(141, 5, "with a database the adopted name is written down, then answered to; a second adoption replaces it.",
       [READY, send(ADOPT, js({"name": "Floppy"})), expect(NAMED, "_event.data.name === 'Floppy'"), READY,
        send(ADOPT, js({"name": "Pip"})), expect(NAMED, "_event.data.name === 'Pip'"), READY,
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
        READY, send(ASK, js({})), expect(NAMED, "_event.data.name === null")],
       params=[("database", "'/nowhere/floppy.sqlite'"), ("bring_up_timeout_ms", "1500")], clock="wall")

S.case(150, 6, "its name heard in a sound addresses it.",
       [READY, send(SOUND, sound()), expect(HANDOFF, addressed("Floppy"))],
       params=[named("Floppy")], backend=[transcript("hey floppy")], clock="wall")
S.case(151, 6, "a sound whose transcript does not name it produces nothing.",
       [READY, send(SOUND, sound()), wait("1s")],
       params=[named("Floppy")], backend=[transcript("hello there")], clock="wall")
S.case(152, 6, "a nameless identity decodes no sound (the backend would fail it).",
       [READY, send(SOUND, sound()), wait("1s")],
       backend=[{"path": INTERACTIONS, "status": 500, "body": {"error": {"message": "boom"}}}], clock="wall")

S.case(160, 7, "a decoding the backend fails answers bot.ability.failed and the identity listens again.",
       [READY, send(SOUND, sound()),
        expect(FAILED, "typeof _event.data.message === 'string' && _event.data.message.length > 0"),
        send(INPUT, said("Floppy")), expect(HANDOFF, addressed("Floppy"))],
       params=[named("Floppy")],
       backend=[{"path": INTERACTIONS, "status": 500, "body": {"error": {"message": "boom"}}}], clock="wall")
S.case(161, 7, "a sound with no file in the media directory fails the decoding.",
       [READY, send(SOUND, sound("media/missing.wav")),
        expect(FAILED, "typeof _event.data.message === 'string' && _event.data.message.length > 0")],
       params=[named("Floppy")], backend=[transcript("hey floppy")], clock="wall")

S.case(170, 8, "input that arrives while recalling from memory is taken once it knows its name.",
       [send(INPUT, said("Floppy?")), READY, expect(HANDOFF, addressed("Floppy"))],
       params=[DB, named("Floppy")], clock="wall")
S.case(171, 8, "input that arrives while adopting is heard with the adopted name.",
       [READY, send(ADOPT, js({"name": "Pip"})), send(INPUT, said("Pip?")),
        expect(NAMED, "_event.data.name === 'Pip'"), READY, expect(HANDOFF, addressed("Pip"))],
       params=[DB], clock="wall")
S.case(172, 8, "text that arrives while a sound is being recognized waits for it: the sound's answer comes first.",
       [READY, send(SOUND, sound()), send(INPUT, said("Floppy")),
        expect(FAILED, "typeof _event.data.message === 'string'"), expect(HANDOFF, addressed("Floppy"))],
       params=[named("Floppy")],
       backend=[{"path": INTERACTIONS, "status": 500, "body": {"error": {"message": "boom"}}}], clock="wall")

S.case(180, 9, "a nameless identity is asked and answers that it has no name.",
       [READY, send(ASK, js({})), expect(NAMED, "_event.data.name === null")])
S.case(181, 9, "a named identity is asked and answers its name.",
       [READY, send(ASK, js({})), expect(NAMED, "_event.data.name === 'Floppy'")],
       params=[named("Floppy")])
S.case(182, 9, "an ask that arrives while recalling is answered with the recalled name once it knows.",
       [send(ASK, js({})), READY, expect(NAMED, "_event.data.name === 'Floppy'")],
       params=[DB, named("Floppy")], clock="wall")
S.case(183, 9, "an adoption tells the name; an ask that arrived while adopting is answered with it.",
       [READY, send(ADOPT, js({"name": "Pip"})), send(ASK, js({})),
        expect(NAMED, "_event.data.name === 'Pip'"), READY, expect(NAMED, "_event.data.name === 'Pip'")],
       params=[DB], clock="wall")
