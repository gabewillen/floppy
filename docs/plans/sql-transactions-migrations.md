# SQL service: transactions and migrations in the SCXML pattern

Status: built on both engines (2026-10-10, decisions D48 to D51): SQLite
through `services/sql` and Postgres through `services/pgsql`.
As built, the migration ability answers `bot.ability.migration.output
{from, to}` and `bot.ability.failed {message}` (the original's ability events)
rather than `<donedata>` and `migration.failed`, and the version guard's
failure code is the sql service's `rowcount`.
Context: `services/sql` today runs one transaction per `<invoke>` and knows
no schema. `abilities/memory/memory.scx` reads `PRAGMA user_version` in one
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
  or nothing (the original's `InputData`).
* The service answers `sql.result {results}` or `sql.failed`, then
  `done.invoke.<id>`.
* Most callers need only this: recall, insert, or a read-modify-write that
  fits in SQL.

## 2. Open transaction: one invocation spans several exchanges (built, D50)

For work where the next statement depends on a decision the document makes
in between (behavior authoring in phase 5, for example), the invocation
stays open and holds one transaction:

```xml
<state id="editing">
  <invoke id="tx" type="https://github.com/stateforward/floppy/services/sql">
    <param name="database" expr="database"/>
    <param name="transaction" expr="'open'"/>
    <param name="statements" expr="[...]"/>                <!-- optional, run at once -->
  </invoke>
  <transition event="sql.result" cond="_event.invokeid === 'tx'">
    ...decide, then <send event="sql.execute" target="#_tx"><param name="statements" expr="..."/></send>
    or <send event="sql.commit" target="#_tx"/> or <send event="sql.rollback" target="#_tx"/>
  </transition>
  <transition event="sql.committed" target="next"/>
  <transition event="sql.failed" target="recovering"/>
</state>
```

* Inputs: `sql.execute {statements}` answers `sql.result {results}` and keeps
  the transaction open; `sql.commit` answers `sql.committed {}`;
  `sql.rollback` answers `sql.rolledback {}`. The last two end the
  invocation.
* A failure answers `sql.failed {code, message}` and ends it with nothing
  committed; so does any other event (code `event`). Leaving the state
  cancels it, and nothing commits.
* As built, the service holds no lock between events. The host drops a
  cancelled invocation's instance without running it again, and SQLite's
  WASI build locks with a dot-file, so a lock held across events would
  outlive a cancel and lock the database for good. Instead the transaction
  is optimistic: the service keeps each statement it ran with the result it
  answered, each `sql.execute` replays them and runs its own inside one
  transaction it rolls back, and `sql.commit` replays them all in one it
  commits. A replayed result that differs (another writer committed
  meanwhile) fails with code `changed`, and the document decides whether to
  start over (Postgres's serialization failure, in the same shape). A
  statement that reads the clock or randomness changes on replay, so an
  open transaction reads neither.
* The rule against a statement ending or widening the transaction stays.

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
* **`abilities/migration/migration.scx`**: the runner, which plays Alembic's
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
  not SQLite's `PRAGMA user_version`. That is portable, it is what the original's
  databases already carry, and an existing the original SQLite file migrates in
  place.

Postgres (built, D51):

* The runner and `<migration:plan>` never parse SQL. The runner picks the
  engine from `database` as the memory ability does (a `postgresql://` URI
  is the pgsql service, a file path the sql service), with one literal
  `<invoke>` per engine, since scx binds a repository service only from a
  literal `<invoke type>`.
* Its own statements are standard SQL both engines accept (`CREATE TABLE IF
  NOT EXISTS alembic_version`, `INSERT ... SELECT ?1 WHERE NOT EXISTS`,
  `UPDATE`, `DELETE`); the pgsql service rewrites `?N` to `$N`.
* Revision files hold each engine's SQL, with the same revision ids:
  `abilities/memory/sqlite/` and `abilities/memory/postgresql/` (Postgres's
  defaulted timestamps are `TIMESTAMPTZ DEFAULT NOW()`).
* The pgsql service takes the whole contract: `rowcount`, `sql.failed
  {code, message}` with the server's `SQLSTATE`, and open transactions by
  replay. A lost race on Postgres is `23505` (the version row's key),
  `40001` or `40P01`, which the runner retries once like `rowcount`.

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

* **Open transactions.** Built on the recommended yes (section 2); every
  phase 3 caller still works in batch mode.
* **Revision files.** Recommended: JSON files via `<data src>` (scxml.rs
  parses JSON `<data src>` as data).
