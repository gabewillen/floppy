# The memory's statements and cases (tests/irp/harness.py writes them).

S = suite
OUTPUT = "bot.ability.memory.output"
DB = "'/memory/floppy.sqlite'"

S.statement(1, "Given a SQLite database path and statements, it migrates the database to head and runs the statements in one transaction, answering `bot.ability.memory.output` `{results}` (one result per statement, in order), then its session ends.")

S.case(100, 1, "a select runs on a migrated database and answers its rows.",
       [expect(OUTPUT, "_event.data.results.length === 1"), expect("done.invoke.memory")],
       params=[("database", DB), ("statements", js([{"sql": "SELECT COUNT(*) AS n FROM alembic_version"}]))])
