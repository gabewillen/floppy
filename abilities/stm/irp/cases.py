# The short-term register's statements and cases (tests/irp/harness.py writes them).

import json

S = suite
OFFERS = "bot.ability.offers"
RECORD = "bot.ability.memory.stm.record"
REINFORCE = "bot.ability.memory.stm.reinforce"
RECENT = "bot.ability.memory.stm.recent"
RECENT_OUTPUT = "bot.ability.memory.stm.recent.output"
TURN = "bot.ability.memory.stm.turn"
RECENT_TURNS = "bot.ability.memory.stm.recent_turns"
RECENT_TURNS_OUTPUT = "bot.ability.memory.stm.recent_turns.output"
FAILED = "bot.ability.failed"
DB = ("database", "'/memory/floppy.sqlite'")

READY = expect(OFFERS, "_event.data.ability === 'stm' && _event.data.choices.length === 0")


def record(name, payload=None, **envelope):
    return send(RECORD, js({"stimulus_name": name, "payload": payload or {}, **envelope}))


def turn(turn_id, direction, text, speakers=(), cut=False, conversation="listening"):
    return send(TURN, js({"id": turn_id, "direction": direction, "speakers": list(speakers), "text": text,
                          "cut": cut, "conversation": conversation}))


def recent(**data):
    return send(RECENT, js(data))


def recent_turns(**data):
    return send(RECENT_TURNS, js(data))


def stringified(values):
    """The JS string literal of JSON.stringify(values)."""
    return js(json.dumps(list(values), separators=(",", ":")))


def names(*expected):
    return f"JSON.stringify(_event.data.entries.map((e) => e.stimulus_name)) === {stringified(expected)}"


def texts(*expected):
    return f"JSON.stringify(_event.data.turns.map((t) => t.text)) === {stringified(expected)}"


S.statement(1, "With a database it opens it (migrating it to the head revision through the memory ability) and is ready: it sends `bot.ability.offers` `{ability: \"stm\", choices: []}` once; without one it is ready the same way, holds nothing (records and turns are dropped) and answers every read empty.")
S.statement(2, "`bot.ability.memory.stm.record` `{stimulus_name, payload, event_id?, source?, target?}` is written with no reply, and `bot.ability.memory.stm.recent` `{stimulus_name?, limit?}` answers `bot.ability.memory.stm.recent.output` `{entries}`: the records inside the recency window, newest first, each `{stm_id, stimulus_name, payload, event_id, source, target, created_at, last_accessed_at}`, optionally only one stimulus's, at most `limit`.")
S.statement(3, "Each record evicts, in its own transaction, every record beyond `capacity` by the effective clock (last access, else admission), so the register holds the newest `capacity`; a read past `recency_window_ms` finds nothing.")
S.statement(4, "`bot.ability.memory.stm.reinforce` `{stm_ids}` stamps those records' `last_accessed_at` (no reply), which moves their effective clock forward for reads and eviction.")
S.statement(5, "`bot.ability.memory.stm.turn` `{id, direction, speakers, text, cut, conversation}` is written with no reply, and `bot.ability.memory.stm.recent_turns` `{limit?, within?}` answers `bot.ability.memory.stm.recent_turns.output` `{turns}`: the newest turns, at most `limit`, oldest first, each `{id, at, direction, speakers, text, cut, conversation}` with `at` UTC (Z); a stored turn that does not decode as one is skipped.")
S.statement(6, "A turn that puts the register over `turn_capacity` sheds the older half at once, in the same transaction: the newest `turn_capacity - floor(turn_capacity / 2)` stay.")
S.statement(7, "A turn's words longer than 2048 UTF-8 bytes are cut at a whole word and its `cut` is true; words that cut to nothing (only white space before the bound) are not written.")
S.statement(8, "The turns' window is `turn_window_ms`, or `within` (milliseconds) when that is shorter; its floor moves in whole steps of half the window, so a turn is read for at least the window and at most one and a half windows.")
S.statement(9, "Requests and notifications are applied one at a time, in arrival order: what arrives while it is opening or applying waits, so a read sees every write sent before it.")
S.statement(10, "A database it cannot open leaves it degraded, without an offer: it sends `bot.ability.failed` `{message: \"unavailable: StmMemory is degraded.\"}`, answers every request the same way, and drops every notification.")

S.case(100, 1, "with a database it is ready with no choices.", [READY], params=[DB], clock="wall")
S.case(101, 1, "without a database it is ready, drops records and turns, and answers reads empty.",
       [READY, record("environment.sound", {"kind": "knock"}), turn("t1", "heard", "hello"),
        recent(), expect(RECENT_OUTPUT, "_event.data.entries.length === 0"),
        recent_turns(), expect(RECENT_TURNS_OUTPUT, "_event.data.turns.length === 0")])

S.case(110, 2, "a record is read back with its projection and envelope; absent envelope fields are null.",
       [READY, record("environment.sound", {"kind": "knock", "count": 2}, event_id="e1", source="device-a"),
        recent(),
        expect(RECENT_OUTPUT, "_event.data.entries.length === 1 && _event.data.entries[0].stimulus_name === 'environment.sound'"
                              " && _event.data.entries[0].payload.kind === 'knock' && _event.data.entries[0].payload.count === 2"
                              " && _event.data.entries[0].event_id === 'e1' && _event.data.entries[0].source === 'device-a'"
                              " && _event.data.entries[0].target === null && _event.data.entries[0].last_accessed_at === null"
                              " && /\\+00:00$/.test(_event.data.entries[0].created_at)")],
       params=[DB], clock="wall")
S.case(111, 2, "records are read newest first, optionally for one stimulus, at most limit.",
       [READY, record("environment.sound"), record("environment.visual"), record("environment.sound", {"n": 3}),
        recent(), expect(RECENT_OUTPUT, names("environment.sound", "environment.visual", "environment.sound")),
        recent(stimulus_name="environment.visual"), expect(RECENT_OUTPUT, names("environment.visual")),
        recent(limit=1), expect(RECENT_OUTPUT, names("environment.sound") + " && _event.data.entries[0].payload.n === 3")],
       params=[DB], clock="wall")

S.case(120, 3, "beyond capacity the oldest records are evicted.",
       [READY, record("a"), record("b"), record("c"), record("d"),
        recent(), expect(RECENT_OUTPUT, names("d", "c"))],
       params=[DB, ("capacity", "2")], clock="wall")
S.case(121, 3, "a record older than the recency window is not read.",
       [READY, record("old"), wait("1200ms"), record("new"),
        recent(), expect(RECENT_OUTPUT, names("new"))],
       params=[DB, ("recency_window_ms", "1000")], clock="wall")

S.case(130, 4, "a reinforced record is read first and outlives a newer one at eviction.",
       [READY, record("a"), recent(), expect(RECENT_OUTPUT, names("a"), assigns=[("stm_id", "_event.data.entries[0].stm_id")]),
        wait("50ms"), record("b"), wait("50ms"),
        send(REINFORCE, "({stm_ids: [stm_id]})"), wait("50ms"), record("c"),
        recent(), expect(RECENT_OUTPUT, names("c", "a") + " && typeof _event.data.entries[1].last_accessed_at === 'string'")],
       data=[("stm_id", "null")], params=[DB, ("capacity", "2")], clock="wall")

S.case(140, 5, "heard and said turns are read back oldest first with their speakers.",
       [READY, turn("t1", "heard", "Hi there, I'm Maya.", [{"ref": "voice:3f2a9c", "role": "source"}]),
        turn("t2", "said", "Hello Maya.", [{"ref": "voice:tts:warm", "role": "source", "own": True}], conversation="conversation"),
        recent_turns(),
        expect(RECENT_TURNS_OUTPUT, texts("Hi there, I'm Maya.", "Hello Maya.")
               + " && _event.data.turns[0].direction === 'heard' && _event.data.turns[1].direction === 'said'"
               " && _event.data.turns[0].speakers[0].ref === 'voice:3f2a9c' && _event.data.turns[1].speakers[0].own === true"
               " && _event.data.turns[1].conversation === 'conversation' && _event.data.turns[0].cut === false"
               " && /Z$/.test(_event.data.turns[0].at)")],
       params=[DB], clock="wall")
S.case(141, 5, "limit keeps the newest turns, still oldest first.",
       [READY, turn("t1", "heard", "one"), turn("t2", "said", "two"), turn("t3", "heard", "three"),
        recent_turns(limit=2), expect(RECENT_TURNS_OUTPUT, texts("two", "three"))],
       params=[DB], clock="wall")

# Stored rows stand in for the register's: the older one's direction is
# neither heard nor said.
DAMAGED = ("recent_turns_select", js("SELECT turn_id, at, direction, speakers_json, text, cut, conversation FROM (SELECT 2 AS n, 't2' AS turn_id, '2026-07-07T12:00:01.000000+00:00' AS at, 'heard' AS direction, '[]' AS speakers_json, 'good' AS text, 0 AS cut, 'listening' AS conversation UNION ALL SELECT 1, 't1', '2026-07-07T12:00:00.000000+00:00', 'shouted', '[]', 'bad', 0, 'listening') AS stored WHERE ?1 = ?1 AND ?2 = ?2 ORDER BY n DESC"))
S.case(142, 5, "a stored turn that does not decode as one is skipped; the turns around it are read.",
       [READY, recent_turns(), expect(RECENT_TURNS_OUTPUT, texts("good"))],
       params=[DB, DAMAGED], clock="wall")

S.case(150, 6, "the fifth turn over a capacity of four sheds the older half: the newest two stay.",
       [READY] + [turn(f"t{i}", "heard", f"turn {i}") for i in range(1, 6)]
       + [recent_turns(), expect(RECENT_TURNS_OUTPUT, texts("turn 4", "turn 5"))],
       params=[DB, ("turn_capacity", "4")], clock="wall")

S.case(160, 7, "words over 2048 bytes are cut at a whole word and marked cut.",
       [READY, send(TURN, "({id: 't1', direction: 'heard', speakers: [], text: 'word '.repeat(409) + 'longword', cut: false, conversation: 'listening'})"),
        recent_turns(),
        expect(RECENT_TURNS_OUTPUT, "_event.data.turns.length === 1 && _event.data.turns[0].cut === true"
                                    " && _event.data.turns[0].text === 'word '.repeat(409).slice(0, -1)")],
       params=[DB], clock="wall")
S.case(161, 7, "a turn its producer already cut stays marked cut.",
       [READY, turn("t1", "said", "the beginning", cut=True), recent_turns(),
        expect(RECENT_TURNS_OUTPUT, "_event.data.turns[0].cut === true && _event.data.turns[0].text === 'the beginning'")],
       params=[DB], clock="wall")

S.case(162, 7, "words that cut to nothing are not written; the turns after them are.",
       [READY, send(TURN, "({id: 't1', direction: 'heard', speakers: [], text: ' '.repeat(2049) + 'word', cut: false, conversation: 'listening'})"),
        turn("t2", "heard", "after"), recent_turns(), expect(RECENT_TURNS_OUTPUT, texts("after"))],
       params=[DB], clock="wall")

S.case(170, 8, "a turn older than one and a half windows is not read.",
       [READY, turn("t1", "heard", "old"), wait("1700ms"), turn("t2", "heard", "new"),
        recent_turns(), expect(RECENT_TURNS_OUTPUT, texts("new"))],
       params=[DB, ("turn_window_ms", "1000")], clock="wall")
S.case(171, 8, "within narrows the window but never widens it.",
       [READY, turn("t1", "heard", "old"), wait("1700ms"), turn("t2", "heard", "new"),
        recent_turns(within=1000), expect(RECENT_TURNS_OUTPUT, texts("new")),
        recent_turns(within=3600000), expect(RECENT_TURNS_OUTPUT, texts("old", "new"))],
       params=[DB, ("turn_window_ms", "60000")], clock="wall")

S.case(180, 9, "what is sent while it is opening waits, and a read sees every write sent before it.",
       [record("environment.sound"), turn("t1", "heard", "early"), recent(), recent_turns(),
        READY, expect(RECENT_OUTPUT, names("environment.sound")), expect(RECENT_TURNS_OUTPUT, texts("early"))],
       params=[DB], clock="wall")

S.case(190, 10, "a database it cannot open degrades it: it says so, refuses requests, and drops notifications.",
       [expect(FAILED, "_event.data.message === 'unavailable: StmMemory is degraded.'"),
        record("environment.sound"), recent(),
        expect(FAILED, "_event.data.message === 'unavailable: StmMemory is degraded.'"),
        recent_turns(), expect(FAILED, "_event.data.message === 'unavailable: StmMemory is degraded.'")],
       params=[("database", "'/nowhere/floppy.sqlite'")], clock="wall")
