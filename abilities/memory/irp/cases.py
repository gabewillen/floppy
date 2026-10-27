# The memory's statements and cases (tests/irp/harness.py writes them).

S = suite
INPUT = "bot.ability.memory.input"
OUTPUT = "bot.ability.memory.output"
FAILED = "bot.ability.failed"
DB = "'/memory/floppy.sqlite'"
INSERT = "INSERT INTO bot_memory (memory_id, content) VALUES (?, ?)"


def opened(database=DB):
    return [("database", database)]


def request(statements, id="r1"):
    return send(INPUT, js({"id": id, "statements": statements}))


def output(cond, id="r1"):
    return expect(OUTPUT, f"_event.data.id === '{id}' && {cond}")


def failed(cond, kind=None, id="r1"):
    kinded = f" && _event.data.kind === '{kind}'" if kind else ""
    return expect(FAILED, f"_event.data.id === '{id}' && typeof _event.data.message === 'string' && {cond}{kinded}")


S.statement(1, "Before it serves a request, it brings the database to the head of floppy's revisions (two revisions, 0001, decision D193, and 0002, decision D256; in the engine's dialect) through the migration ability, so the statements see Alembic's `alembic_version` at `0002` and the tables, `bot_behavior` with its `lifetime` column, `bot_stm_turn`, the people `bot_person` and `bot_person_embedding` (decision D194), each person's representative `bot_person_centroid` and the named sequences `bot_sequence` (decision D205), and `bot_memory_embedding`.")
S.statement(2, "A `bot.ability.memory.input` `{id, statements}` runs its `statements` (`[{sql, parameters?}]`) in order in one transaction and answers its sender `bot.ability.memory.output` `{id, results}`: one result per statement, in order, each `{rowcount, rows}` with every row `{columns, values}`; a statement's `parameters` bind its placeholders.")
S.statement(3, "A statement that fails (the engine refuses it, or its `rowcount` is not the one it expects) answers its sender `bot.ability.failed` `{id, message, kind}`, the engine's message and kind `store_failed`, and no `bot.ability.memory.output`.")
S.statement(4, "When the migration cannot bring the database to head, every request is answered `bot.ability.failed` `{id, message, kind}` with the migration's message and kind `unavailable`, and its statements never run.")
S.statement(5, "The database names the engine: a `postgresql://` (or `postgres://`) URI is a Postgres database reached through the pgsql service, anything else a SQLite file through the sql service.")
S.statement(6, "It is long-lived (decision D262): it answers each request once, one at a time and in the order they arrive, and goes on serving after an answer, `bot.ability.memory.output` or `bot.ability.failed`, until its invoker ends it.")

S.case(100, 1, "a fresh database is at the head, 0002, with the tables before the statements run.",
       [request([{"sql": "SELECT version_num FROM alembic_version"},
                 {"sql": "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name LIKE 'bot_%'"},
                 {"sql": "SELECT count(*) FROM pragma_table_info('bot_behavior') WHERE name = 'lifetime'"}]),
        output("_event.data.results.length === 3"
               " && _event.data.results[0].rows.length === 1 && _event.data.results[0].rows[0].values[0] === '0002'"
               " && _event.data.results[1].rows[0].values[0] === 10"
               " && _event.data.results[2].rows[0].values[0] === 1")],
       params=opened())

S.case(110, 2, "an insert and a select answer one result each, in order, the parameters bound and the schema's defaults applied.",
       [request([{"sql": INSERT, "parameters": ["m1", "Floppy"]},
                 {"sql": "SELECT content, scope FROM bot_memory WHERE memory_id = ?", "parameters": ["m1"]}]),
        output("_event.data.results.length === 2"
               " && _event.data.results[0].rowcount === 1 && _event.data.results[0].rows.length === 0"
               " && _event.data.results[1].rows.length === 1"
               " && JSON.stringify(_event.data.results[1].rows[0].columns) === '[\"content\",\"scope\"]'"
               " && _event.data.results[1].rows[0].values[0] === 'Floppy'"
               " && _event.data.results[1].rows[0].values[1] === 'memory'")],
       params=opened())
S.case(111, 2, "the statements share one transaction: a later statement sees an earlier one's rows.",
       [request([{"sql": INSERT, "parameters": ["a", "A"]},
                 {"sql": INSERT, "parameters": ["b", "B"]},
                 {"sql": "DELETE FROM bot_memory WHERE memory_id = ?", "parameters": ["a"]},
                 {"sql": "SELECT content FROM bot_memory ORDER BY memory_id"}]),
        output("_event.data.results.length === 4"
               " && _event.data.results[2].rowcount === 1"
               " && _event.data.results[3].rows.length === 1 && _event.data.results[3].rows[0].values[0] === 'B'")],
       params=opened())

S.case(120, 3, "a statement the engine refuses answers bot.ability.failed with its message, and no output.",
       [request([{"sql": INSERT, "parameters": ["m1", "Floppy"]}, {"sql": "SELECT * FROM nowhere"}]),
        failed("_event.data.message.indexOf('no such table: nowhere') >= 0", "store_failed")],
       params=opened())
S.case(121, 3, "a constraint violation answers bot.ability.failed with the engine's message.",
       [request([{"sql": INSERT, "parameters": ["m1", "A"]}, {"sql": INSERT, "parameters": ["m1", "B"]}]),
        failed("_event.data.message === 'UNIQUE constraint failed: bot_memory.memory_id'", "store_failed")],
       params=opened())
S.case(122, 3, "a statement whose rowcount is not the one it expects fails the transaction.",
       [request([{"sql": "UPDATE bot_memory SET content = 'x' WHERE memory_id = 'nope'", "rowcount": 1}]),
        failed("_event.data.message === \"a statement's rowcount is not the one it expects\"", "store_failed")],
       params=opened())

S.case(130, 4, "a database outside the granted directory fails in the migration: each request is answered with its message.",
       [request([{"sql": "SELECT 1"}]),
        failed("_event.data.message.indexOf('unable to open database file') >= 0", "unavailable"),
        request([{"sql": "SELECT 1"}], "r2"),
        failed("_event.data.message.indexOf('unable to open database file') >= 0", "unavailable", "r2")],
       params=opened("'/elsewhere/floppy.sqlite'"))

S.case(140, 5, "a postgresql:// URI goes to the pgsql service: an unreachable server fails with its message.",
       [request([{"sql": "SELECT 1"}]),
        failed("_event.data.message.indexOf('pgsql could not connect') >= 0")],
       params=opened("'postgresql://floppy@127.0.0.1:9/floppy'"),
       clock="wall")

S.case(150, 6, "two requests sent together are answered in order, and the session goes on.",
       [request([{"sql": INSERT, "parameters": ["m1", "A"]}]),
        request([{"sql": "SELECT content FROM bot_memory"}], "r2"),
        output("_event.data.results[0].rowcount === 1"),
        output("_event.data.results[0].rows.length === 1 && _event.data.results[0].rows[0].values[0] === 'A'", "r2"),
        wait("300ms")],
       params=opened(), clock="wall")
S.case(151, 6, "after a failure it goes on serving: a next request is answered.",
       [request([{"sql": "SELECT"}]), failed("true"),
        request([{"sql": "SELECT 1"}], "r2"),
        output("_event.data.results[0].rows[0].values[0] === 1", "r2"),
        wait("300ms")],
       params=opened(), clock="wall")
