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

## 3. Migrations: a parameter of the service, applied atomically

```xml
<data id="migrations" expr="[revision_0001, revision_0002]"/>
<data id="revision_0001" src="memory/0001.sql"/>   <!-- mosfet Alembic 0001 -->
<data id="revision_0002" src="memory/0002.sql"/>
...
<param name="migrations" expr="migrations"/>
```

* `migrations` is an ordered array of revision scripts. Revision *n* brings
  `PRAGMA user_version` from *n−1* to *n*, a linear history like
  Alembic's.
* Each script may hold several DDL statements: it is the document's own
  trusted schema, run as a batch.
* On every invocation that passes `migrations`, the service runs the
  following in the same transaction as the statements, under the write lock:
  1. It reads `user_version`.
  2. It runs the missing revisions in order.
  3. It sets `user_version` to the newest revision.
* Version check and migration are therefore atomic: two concurrent first
  runs cannot both migrate, and memory.scx's read-then-migrate step and its
  retry go away.
* A database newer than the newest revision is refused untouched:
  `sql.failed {code: "schema-version"}`. A revision that fails rolls
  everything back.
* `sql.result` carries only the statements' results. `user_version` is
  readable as a statement when a document needs it.
* Revisions are append-only. A shipped revision never changes, and
  downgrades are not supported (Alembic's `downgrade` is not ported).
* The memory ability passes mosfet's 0001 and 0002 this way, and phase 5's
  behaviors reuse them. A bot that needs its own tables uses its own
  database file and its own revision list.

## 4. Errors as events

| What happened | Event in the invoking session |
| --- | --- |
| malformed `<param>` (missing database, bad statements or migrations) | `error.execution` (the invocation never starts) |
| no grant for the directory | the run is refused before it starts (exit 2) |
| SQL error, constraint, busy, bound exceeded, newer schema | `sql.failed {message, code}` |

* `code` is SQLite's extended result-code name (`SQLITE_CONSTRAINT_PRIMARYKEY`,
  `SQLITE_BUSY`), or `schema-version`, `bound` or `cancelled`, so a
  document can `cond` on it without parsing text.
* `message` never carries a bound value or a path.
* After `sql.failed` the invocation ends: nothing commits, and
  `done.invoke.<id>` follows.

## 5. Decisions for the user

* **Interactive transactions.** Recommended: yes, as specified in section 2.
  Every phase 3 caller works in batch mode, but phase 5's behavior
  authoring (validate, simulate, then store or discard) fits a transaction
  held open across events.
* **Migrations as `.sql` files via `<data src>`** (recommended, if scxml.rs
  loads them as text) or as strings in `<data expr>`, as now.
