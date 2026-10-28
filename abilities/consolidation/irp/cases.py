# The consolidation's statements and cases (tests/irp/harness.py writes them).
#
# A case's /memory is a fresh, empty directory and the harness invokes the
# consolidation at once, so no case can seed people before it reads them:
# the cases cover no database, a fresh database (nothing to consolidate)
# and a database the Memory ability refuses. Folding voices into a
# representative, merging people and the cancel of a step in progress are
# covered by tests/tests/consolidation.rs (with seeded people) and
# tests/tests/cognition.rs (rest).

S = suite
DONE = "done.invoke.consolidation"
DB = memory("'/memory/floppy.sqlite'")


def done(cond):
    return expect(DONE, cond)


S.statement(1, "With no `memory` `<param>` (null) it logs `memory` `absent` and ends at once, its donedata `{folded: 0, merged: 0, steps: 0, failed: false, reason: \"absent\"}`.")
S.statement(2, "With a database it reads the people memory holds through the Memory ability (which brings the database to the head revision), folds the voices heard for each person since into their representative and merges two people whose representatives are at the conversation's threshold, one guarded transaction per step; with nothing to do it ends with `{folded: 0, merged: 0, steps: 0, failed: false, reason: null}`.")
S.statement(3, "A memory step that fails ends it at once with `failed: true` and the failure as its `reason`.")

S.case(100, 1, "no database: nothing to consolidate, memory absent.",
       [done("_event.data.folded === 0 && _event.data.merged === 0 && _event.data.steps === 0"
             " && _event.data.failed === false && _event.data.reason === 'absent'")],
       logs={"memory": ["absent"]})
S.case(110, 2, "a fresh database holds no people: nothing to fold or merge.",
       [done("_event.data.folded === 0 && _event.data.merged === 0 && _event.data.steps === 0"
             " && _event.data.failed === false && _event.data.reason === null")],
       params=[DB], clock="wall")
S.case(120, 3, "a relative database path the Memory ability refuses fails the consolidation.",
       [done("_event.data.failed === true && typeof _event.data.reason === 'string' && _event.data.reason.length > 0"
             " && _event.data.steps === 0")],
       params=[memory("'relative.sqlite'")], clock="wall")
