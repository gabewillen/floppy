# The migration's statements and cases (tests/irp/harness.py writes them).

S = suite
OUTPUT = "bot.ability.migration.output"
FAILED = "bot.ability.failed"
DONE = expect("done.invoke.migration")
DB = "'/memory/floppy.sqlite'"

# A two-revision history: b's upgrade needs the table a's creates, so b
# applying proves a applied before it, in the same transaction.
A = {"revision": "a", "down_revision": None,
     "upgrade": [{"sql": "CREATE TABLE t (x TEXT)"}], "downgrade": [{"sql": "DROP TABLE t"}]}
B = {"revision": "b", "down_revision": "a",
     "upgrade": [{"sql": "INSERT INTO t (x) VALUES ('b')"}], "downgrade": [{"sql": "DELETE FROM t"}]}
# A revision whose upgrade the engine refuses.
BROKEN = {"revision": "a", "down_revision": None,
          "upgrade": [{"sql": "SELECT * FROM nowhere"}], "downgrade": []}


def migrate(revisions, target=None, database=DB):
    params = [("database", database), ("revisions", js(revisions))]
    if target is not None:
        params.append(("target", js(target)))
    return params


def output(start, end):
    return expect(OUTPUT, f"_event.data.from === {js(start)} && _event.data.to === {js(end)}")


def failed(cond):
    return expect(FAILED, f"typeof _event.data.message === 'string' && {cond}")


S.statement(1, "It reads the database's revision from Alembic's `alembic_version` (creating the table when the database lacks it; no row is base) and, with no `target` <param>, upgrades to head: it answers its parent `bot.ability.migration.output` `{from, to}`, the revisions before and after (`null` for base).")
S.statement(2, "The `target` <param> is Alembic's: `head`, `base`, a revision, or a relative `+N` / `-N` from the revision read.")
S.statement(3, "The planned steps' statements run oldest first, after the version row's guard, in one transaction on the database's engine; a statement the engine refuses answers `bot.ability.failed` `{message}` with the engine's message.")
S.statement(4, "At the target already, nothing is applied: it answers `{from, to}` with both the revision read.")
S.statement(5, "A target the history does not hold, a relative step past base or head, or revisions that are not one linear history answer `bot.ability.failed` `{message: \"Migration could not plan from <current> to <target>\"}`.")
S.statement(6, "A transaction that fails with a lost race's code (`rowcount`, `SQLITE_BUSY`, `23505`, `40001`, `40P01`) is read, planned and applied again once; failing again, it answers `bot.ability.failed` with the engine's message.")
S.statement(7, "A database the engine cannot open answers `bot.ability.failed` `{message}`; the database names the engine (a `postgresql://` URI the pgsql service, else the sql service's SQLite file).")
S.statement(8, "After its one answer (`bot.ability.migration.output` or `bot.ability.failed`) the session ends.")

S.case(100, 1, "a fresh database is upgraded from base to head by default.",
       [output(None, "b"), DONE], params=migrate([A, B]))
S.case(101, 1, "an explicit head target upgrades the same way.",
       [output(None, "b"), DONE], params=migrate([A, B], "head"))

S.case(110, 2, "a revision target stops at that revision.",
       [output(None, "a"), DONE], params=migrate([A, B], "a"))
S.case(111, 2, "a relative +N target moves N revisions from the one read.",
       [output(None, "a"), DONE], params=migrate([A, B], "+1"))
S.case(112, 2, "+2 from base reaches head.",
       [output(None, "b"), DONE], params=migrate([A, B], "+2"))

S.case(120, 3, "the later revision's upgrade sees the earlier one's table: the steps run oldest first in one transaction.",
       [output(None, "c"), DONE],
       params=migrate([A, B, {"revision": "c", "down_revision": "b",
                              "upgrade": [{"sql": "UPDATE t SET x = 'c' WHERE x = 'b'", "rowcount": 1}],
                              "downgrade": []}]))
S.case(121, 3, "a step the engine refuses fails the migration with the engine's message.",
       [failed("_event.data.message.indexOf('no such table: nowhere') >= 0"), DONE],
       params=migrate([BROKEN]))
S.case(122, 3, "revisions given out of order are still applied oldest first.",
       [output(None, "b"), DONE], params=migrate([B, A]))

S.case(130, 4, "at base with target base, nothing is applied: the broken revision never runs.",
       [output(None, None), DONE], params=migrate([BROKEN], "base"))
S.case(131, 4, "an empty history is at head already.",
       [output(None, None), DONE], params=migrate([]))

S.case(140, 5, "a target the history does not hold fails to plan.",
       [failed("_event.data.message === 'Migration could not plan from null to 0009'"), DONE],
       params=migrate([A, B], "0009"))
S.case(141, 5, "a relative step past head fails to plan.",
       [failed("_event.data.message === 'Migration could not plan from null to +3'"), DONE],
       params=migrate([A, B], "+3"))
S.case(142, 5, "a relative step past base fails to plan.",
       [failed("_event.data.message === 'Migration could not plan from null to -1'"), DONE],
       params=migrate([A, B], "-1"))
S.case(143, 5, "two revisions following the same one are no linear history.",
       [failed("_event.data.message === 'Migration could not plan from null to head'"), DONE],
       params=migrate([A, B, dict(B, revision="b2")]))

S.case(150, 6, "a transaction failing with code rowcount is tried again once, then fails with the engine's message.",
       [failed("_event.data.message === \"a statement's rowcount is not the one it expects\""), DONE],
       params=migrate([{"revision": "a", "down_revision": None,
                        "upgrade": [{"sql": "CREATE TABLE t (x TEXT)"},
                                    {"sql": "UPDATE t SET x = 'y'", "rowcount": 1}],
                        "downgrade": []}]))

S.case(160, 7, "a SQLite file outside the granted directory fails in the read.",
       [failed("_event.data.message.indexOf('unable to open database file') >= 0"), DONE],
       params=migrate([A, B], database="'/elsewhere/floppy.sqlite'"))
S.case(161, 7, "a postgresql:// URI goes to the pgsql service: an unreachable server fails.",
       [failed("_event.data.message.indexOf('pgsql could not connect') >= 0"), DONE],
       params=migrate([A, B], database="'postgresql://floppy@127.0.0.1:9/floppy'"), clock="wall")

S.case(170, 8, "after its output the session ends, and nothing else is sent.",
       [output(None, "b"), expect("done.invoke.migration"), wait("300ms")],
       params=migrate([A, B]), clock="wall")
S.case(171, 8, "after its failure the session ends, and nothing else is sent.",
       [failed("true"), expect("done.invoke.migration"), wait("300ms")],
       params=migrate([BROKEN]), clock="wall")
