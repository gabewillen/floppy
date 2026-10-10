# sql

The `sql` invoked service: one SQLite transaction of mosfet's `Statement`s
per `<invoke>`, as a Wasm component
(`https://github.com/stateforward/floppy/services/sql`, component
`floppy_service_sql`). It is the SQLite engine under mosfet's Memory
ability, which floppy runs as
[`abilities/memory/memory.scx`](../../abilities/memory/memory.scx), and it
knows no schema of its own. Its crate documentation ([src/lib.rs](src/lib.rs))
is the full contract: parameters, events, open transactions, failure codes,
bounds, and telemetry.

- **Engine.** SQLite is the C library compiled into the component
  (`rusqlite` `bundled`, built with wasi-sdk 25 at `WASI_SDK_PATH`;
  decision D45). The component's `wasi:cli` imports are satisfied by the
  composed deny-all provider [`wasi-cli-deny.wat`](wasi-cli-deny.wat).
- **Grant.** The database is a file in a directory the run grants
  (`--dir https://github.com/stateforward/floppy/services/sql=HOST::/memory`),
  named by the invoking document's `database` `<param>`; a run that binds
  the service without the grant is refused before it starts (decision D46).
- **Transactions.** One invocation is one `IMMEDIATE` transaction, all or
  nothing; `<param name="transaction" expr="'open'">` keeps one open across
  `sql.execute`, `sql.commit` and `sql.rollback`, optimistic by replay
  (decision D50). A statement may carry `rowcount`, the count it must affect
  (decision D49).
- **Events.** `sql.result {results}` or `sql.failed {code, message}`,
  `code` being SQLite's primary result-code name or one of the service's own
  (decision D49).
- **Migrations** are not the service's: the migration ability
  ([`abilities/migration/migration.scx`](../../abilities/migration/migration.scx))
  runs Alembic's revisions through it (decision D48).

[`services/pgsql`](../pgsql/README.md) is the Postgres engine with the same
contract (decision D51).
