# The memory's statements and cases (tests/irp/harness.py writes them).

S = suite
OUTPUT = "bot.ability.memory.output"
FAILED = "bot.ability.failed"
DONE = expect("done.invoke.memory")
DB = "'/memory/floppy.sqlite'"
INSERT = "INSERT INTO bot_memory (memory_id, content) VALUES (?, ?)"


def run(statements, database=DB):
    return [("database", database), ("statements", js(statements))]


def failed(cond, kind=None):
    kinded = f" && _event.data.kind === '{kind}'" if kind else ""
    return expect(FAILED, f"typeof _event.data.message === 'string' && {cond}{kinded}")


S.statement(1, "Before the statements, it brings the database to the head of floppy's revisions (0001, 0002, then 0003, decision D165; in the engine's dialect) through the migration ability, so the statements see Alembic's `alembic_version` at `0003` and the tables, `bot_behavior` with its `lifetime` column and `bot_stm_turn`.")
S.statement(2, "It runs the `statements` <param> (`[{sql, parameters?}]`) in order in one transaction and answers its parent `bot.ability.memory.output` `{results}`: one result per statement, in order, each `{rowcount, rows}` with every row `{columns, values}`; a statement's `parameters` bind its placeholders.")
S.statement(3, "A statement that fails (the engine refuses it, or its `rowcount` is not the one it expects) answers `bot.ability.failed` `{message, kind}`, the engine's message and kind `store_failed`, and no `bot.ability.memory.output`.")
S.statement(4, "A database the migration cannot bring to head answers `bot.ability.failed` `{message, kind}` with the migration's message and kind `unavailable`, and the statements never run.")
S.statement(5, "The database names the engine: a `postgresql://` (or `postgres://`) URI is a Postgres database reached through the pgsql service, anything else a SQLite file through the sql service.")
S.statement(6, "After its one answer (`bot.ability.memory.output` or `bot.ability.failed`) the session ends.")

S.case(100, 1, "a fresh database is at 0003 with the tables before the statements run.",
       [expect(OUTPUT, "_event.data.results.length === 3"
                       " && _event.data.results[0].rows.length === 1 && _event.data.results[0].rows[0].values[0] === '0003'"
                       " && _event.data.results[1].rows[0].values[0] === 5"
                       " && _event.data.results[2].rows[0].values[0] === 1"),
        DONE],
       params=run([{"sql": "SELECT version_num FROM alembic_version"},
                   {"sql": "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name LIKE 'bot_%'"},
                   {"sql": "SELECT count(*) FROM pragma_table_info('bot_behavior') WHERE name = 'lifetime'"}]))

S.case(110, 2, "an insert and a select answer one result each, in order, the parameters bound and the schema's defaults applied.",
       [expect(OUTPUT, "_event.data.results.length === 2"
                       " && _event.data.results[0].rowcount === 1 && _event.data.results[0].rows.length === 0"
                       " && _event.data.results[1].rows.length === 1"
                       " && JSON.stringify(_event.data.results[1].rows[0].columns) === '[\"content\",\"scope\"]'"
                       " && _event.data.results[1].rows[0].values[0] === 'Floppy'"
                       " && _event.data.results[1].rows[0].values[1] === 'memory'"),
        DONE],
       params=run([{"sql": INSERT, "parameters": ["m1", "Floppy"]},
                   {"sql": "SELECT content, scope FROM bot_memory WHERE memory_id = ?", "parameters": ["m1"]}]))
S.case(111, 2, "the statements share one transaction: a later statement sees an earlier one's rows.",
       [expect(OUTPUT, "_event.data.results.length === 4"
                       " && _event.data.results[2].rowcount === 1"
                       " && _event.data.results[3].rows.length === 1 && _event.data.results[3].rows[0].values[0] === 'B'"),
        DONE],
       params=run([{"sql": INSERT, "parameters": ["a", "A"]},
                   {"sql": INSERT, "parameters": ["b", "B"]},
                   {"sql": "DELETE FROM bot_memory WHERE memory_id = ?", "parameters": ["a"]},
                   {"sql": "SELECT content FROM bot_memory ORDER BY memory_id"}]))

S.case(120, 3, "a statement the engine refuses answers bot.ability.failed with its message, and no output.",
       [failed("_event.data.message.indexOf('no such table: nowhere') >= 0", "store_failed"), DONE],
       params=run([{"sql": INSERT, "parameters": ["m1", "Floppy"]}, {"sql": "SELECT * FROM nowhere"}]))
S.case(121, 3, "a constraint violation answers bot.ability.failed with the engine's message.",
       [failed("_event.data.message === 'UNIQUE constraint failed: bot_memory.memory_id'", "store_failed"), DONE],
       params=run([{"sql": INSERT, "parameters": ["m1", "A"]}, {"sql": INSERT, "parameters": ["m1", "B"]}]))
S.case(122, 3, "a statement whose rowcount is not the one it expects fails the transaction.",
       [failed("_event.data.message === \"a statement's rowcount is not the one it expects\"", "store_failed"), DONE],
       params=run([{"sql": "UPDATE bot_memory SET content = 'x' WHERE memory_id = 'nope'", "rowcount": 1}]))

S.case(130, 4, "a database outside the granted directory fails in the migration, with its message.",
       [failed("_event.data.message.indexOf('unable to open database file') >= 0", "unavailable"), DONE],
       params=run([{"sql": "SELECT 1"}], "'/elsewhere/floppy.sqlite'"))

S.case(140, 5, "a postgresql:// URI goes to the pgsql service: an unreachable server fails with its message.",
       [failed("_event.data.message.indexOf('pgsql could not connect') >= 0"), DONE],
       params=run([{"sql": "SELECT 1"}], "'postgresql://floppy@127.0.0.1:9/floppy'"),
       clock="wall")

S.case(150, 6, "after its output the session ends, and nothing else is sent.",
       [expect(OUTPUT, "_event.data.results.length === 1 && _event.data.results[0].rows[0].values[0] === 1"),
        expect("done.invoke.memory"), wait("300ms")],
       params=run([{"sql": "SELECT 1"}]), clock="wall")
S.case(151, 6, "after its failure the session ends.",
       [failed("true"), expect("done.invoke.memory"), wait("300ms")],
       params=run([{"sql": "SELECT"}]), clock="wall")
