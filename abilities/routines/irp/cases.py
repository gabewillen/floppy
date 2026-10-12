# The routines' statements and cases (tests/irp/harness.py writes them).
#
# A case's /memory is a fresh, empty directory and the harness invokes the
# routines at once, so no case can seed behavior inventory before the
# routines recall it: the cases cover an inventory with no database, an
# empty inventory and a recall that fails. Running, forwarding, deferring
# and restarting routines are covered by the routines_runner suite (the
# chain) and tests/tests/routines.rs (with a seeded inventory).

S = suite
RECONCILE = "bot.ability.routines.reconcile"
DB = ("database", "'/memory/floppy.sqlite'")

# On the logical clock a delayed send to itself fires once the run is idle:
# a step after which nothing else arrives.
QUIET = wait("1s")


S.statement(1, "With no `database` `<param>` (null) it reconciles to nothing: it runs no routine and sends its parent nothing; it is long-lived and never reaches a final state.")
S.statement(2, "With a database it recalls the ACTIVE persistent `bot_behavior` rows through the Memory ability (which brings the database to the head revision); an empty inventory runs no routine and sends its parent nothing (the Memory ability's answers stay with the routines).")
S.statement(3, "`bot.ability.routines.reconcile` `{}` reads the inventory again, any number of times, back to back (a reconcile while reconciling re-enters it); it answers the parent nothing.")
S.statement(4, "A recall that fails (a database the Memory ability refuses) is logged `bot.ability.routines.reconcile.failed` and keeps the running set; nothing reaches the parent and later reconciles still work.")
S.statement(5, "A reconcile whose data its schema rejects (any field: it carries no data) is `error.execution` in its sender and never reaches the routines.")

S.case(100, 1, "no database: nothing runs and nothing is sent.", QUIET)
S.case(101, 1, "no database: a reconcile still sends nothing.",
       QUIET + [send(RECONCILE, "({})")] + QUIET)
S.case(110, 2, "a fresh database: the empty inventory runs nothing.", QUIET, params=[DB])
S.case(120, 3, "a fresh database: reconciles one after another and back to back answer nothing.",
       QUIET + [send(RECONCILE, "({})")] + QUIET + [send(RECONCILE, "({})"), send(RECONCILE, "({})")] + QUIET,
       params=[DB])
S.case(130, 4, "a relative database path the Memory ability refuses: nothing reaches the parent, and a later reconcile fails the same way.",
       QUIET + [send(RECONCILE, "({})")] + QUIET, params=[("database", "'relative.sqlite'")])
S.case(140, 5, "a reconcile carrying a field is error.execution in the sender.",
       [send(RECONCILE, "({force: true})"), expect("error.execution")] + QUIET, params=[DB])
