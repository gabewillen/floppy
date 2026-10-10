# SQL service: transactions and migrations in the SCXML pattern

Status: proposal (2026-10-10), for the user's review before building.
Context: `services/sql` today runs one transaction per `<invoke>` and knows
no schema. `abilities/memory.scx` reads `PRAGMA user_version` in one
invocation and migrates in a second, so a race between two fresh migrations
needs a retry (decision D47).

## Principle

An SCXML state is the scope. Entering the invoking state opens the database
work. Its `done.invoke` ends it, and leaving the state early cancels it
(SCXML 6.4). A cancelled invocation never commits. SQL stays data in the
document: `<data>` or `<data src="...sql">`, with bind values in
`parameters`.

## 1. Batch transaction (today's shape, kept as the default)

```xml
<invoke id="recall" type="https://github.com/stateforward/floppy/services/sql">
  <param name="database" expr="database"/>
  <param name="statements" expr="[{sql: select_names, parameters: ['self_name']}]"/>
</invoke>
```

* One invocation is one `BEGIN IMMEDIATE ... COMMIT` of the statements, all
  or nothing (mosfet's `InputData`).
* The service answers `sql.result {results}` or `sql.failed`, then
  `done.invoke.<id>`.
* Most callers need only this: recall, insert, or a read-modify-write that
  fits in SQL.

## 2. Interactive transaction (new): one invocation spans several exchanges

For work where the next statement depends on a decision the document makes
in between (behavior authoring in phase 5, for example), the invocation
stays open and holds one transaction:

```xml
<state id="editing">
  <invoke id="tx" type="https://github.com/stateforward/floppy/services/sql">
    <param name="database" expr="database"/>
    <param name="transaction" expr="'immediate'"/>   <!-- deferred | immediate | exclusive -->
  </invoke>
  <onentry><send event="sql.execute" target="#_tx"><param name="statements" expr="..."/></send></onentry>
  <transition event="sql.result" cond="_event.invokeid === 'tx'">  ...decide, send more sql.execute... </transition>
  <transition event="..." ><send event="sql.commit" target="#_tx"/></transition>
  <transition event="sql.committed" target="next"/>
  <transition event="sql.failed" target="recovering"/>
  <transition event="tx.timeout" target="recovering"/>   <!-- <send delay> bounds it -->
</state>
```

* Inputs to the service: `sql.execute {statements}`, which answers
  `sql.result {results}`. Then `sql.commit`, which answers `sql.committed`
  and ends the invocation, or `sql.rollback`, which answers
  `sql.rolledback` and ends it.
* Leaving the state cancels the invocation, and a cancelled invocation
  rolls back.
* A statement that fails rolls back the whole transaction, answers
  `sql.failed`, and ends the invocation. No partial state survives.
* The SQL keywords name the events (`BEGIN` modes, `COMMIT`, `ROLLBACK`),
  so nothing new is invented.
* The rule against a statement ending or widening the transaction
  (`BEGIN`, `COMMIT`, `ROLLBACK`, `ATTACH`, `DETACH`) stays: only the events
  end it. `SAVEPOINT` stays allowed for nested undo.
* Locking: SQLite holds the write lock until commit. Another invocation
  that writes meanwhile gets `sql.failed` with `code: "SQLITE_BUSY"`
  immediately (no busy wait inside a component), and its document decides
  whether to retry. The invoking state's `<send delay>` bounds how long a
  transaction may stay open.

## 3. Migrations: an Alembic-style migration ability, agnostic of the database

The user's direction: "make it like alembic that's agnostic of the database
so we have a migration service, and then sql has the transaction". The sql
service keeps only transactions. Migrations become their own layer that
knows revisions and never a database engine. A Wasm service cannot invoke
another service, so the layer is two pieces:

* **`<migration:plan>`**: a pure Wasm namespace element in a new
  `namespaces/migration`, with no I/O, synchronous logic per D13. It takes
  the revisions, the current revision and a target, and writes the ordered
  steps.
  * It walks the revision graph as Alembic does, by `revision` and
    `down_revision`.
  * Targets are `head`, `base`, a revision id, or relative `+N`/`-N`.
  * A step is `{revision, direction: "upgrade" | "downgrade", statements}`.
  * Unknown revisions, multiple heads and cycles fail the element
    (`error.execution`).
* **`abilities/migration.scx`**: the runner, which plays Alembic's
  `command.upgrade` and `command.downgrade`. It is invoked with
  `<param name="database">`, `<param name="revisions">` and
  `<param name="target">` (default `head`), and runs these states:
  1. **Reading:** it invokes the sql service for the current version.
  2. **Planning:** `<migration:plan>` computes the steps.
  3. **Applying:** one sql transaction runs every step's statements and
     updates the version row.
  4. **Done:** `<donedata>` returns `{previous, current}`.

  A failure answers `migration.failed {message, code}` and commits nothing.

Revisions are data, like Alembic's script directory. One file per revision
(`<data src="memory/migrations/0001.json">`) holds
`{revision, down_revision, upgrade: [statements], downgrade: [statements]}`.

* The statements are plain SQL for whatever engine the database is, so the
  layer itself is agnostic. It never parses SQL, and phase 7's PGlite engine
  reuses the same runner.
* The version lives in Alembic's own table, `alembic_version (version_num)`,
  not SQLite's `PRAGMA user_version`. That is portable, it is what mosfet's
  databases already carry, and an existing mosfet SQLite file migrates in
  place.

Transactions:

* The read and the apply are separate invocations. The apply's first
  statement re-checks the version row inside the transaction:
  `UPDATE alembic_version SET version_num = ?new WHERE version_num IS ?old`,
  and the runner checks its rowcount.
* So a concurrent migration that got there first makes the apply fail
  cleanly with `code: "version-changed"`. The runner then re-reads once.
* DDL is transactional on SQLite and Postgres, so a whole upgrade is one
  transaction (Alembic's default for these engines).

Usage:

* The memory ability invokes the migration ability once per database open,
  then its statements. A bot can also run it at activation.
* `scx run` of a one-line document is the `alembic upgrade head` /
  `downgrade -1` command line.

Every name comes from Alembic: revision, down_revision, head, base,
upgrade, downgrade, current, alembic_version.

## 4. Errors as events

| What happened | Event in the invoking session |
| --- | --- |
| malformed `<param>` (missing database, bad statements or migrations) | `error.execution` (the invocation never starts) |
| no grant for the directory | the run is refused before it starts (exit 2) |
| SQL error, constraint, busy, bound exceeded | `sql.failed {message, code}` |
| unknown target or revision, multiple heads, a lost version race | `migration.failed {message, code}` from the migration ability |

* `code` is SQLite's extended result-code name (`SQLITE_CONSTRAINT_PRIMARYKEY`,
  `SQLITE_BUSY`), or `bound` or `cancelled`, so a
  document can `cond` on it without parsing text.
* `message` never carries a bound value or a path.
* After `sql.failed` the invocation ends: nothing commits, and
  `done.invoke.<id>` follows.

## 5. Decisions for the user

* **Interactive transactions.** Recommended: yes, as specified in section 2.
  Every phase 3 caller works in batch mode, but phase 5's behavior
  authoring (validate, simulate, then store or discard) fits a transaction
  held open across events.
* **Revision files.** Recommended: JSON files via `<data src>` (scxml.rs
  parses JSON `<data src>` as data).
