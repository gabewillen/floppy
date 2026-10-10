# pgsql

The `pgsql` invoked service: one Postgres transaction of mosfet's
`Statement`s per `<invoke>`, as a Wasm component
(`https://github.com/stateforward/floppy/services/pgsql`, component
`floppy_service_pgsql`). It is the Postgres engine under mosfet's Memory
ability (mosfet's `postgres_memory`), beside the SQLite engine
[`services/sql`](../sql/README.md), and answers exactly as that service does:
the same `statements`, `rowcount` guards, open transactions, and
`sql.result`/`sql.failed` events (decision D51). Its crate documentation
([src/lib.rs](src/lib.rs)) is the full contract.

- **Connection.** The Postgres wire protocol (3.0) over
  `wasi:sockets/tcp`, granted with
  `--wasi https://github.com/stateforward/floppy/services/pgsql=inherit-network`.
  The `database` `<param>` is a connection URI,
  `postgresql://USER@IP:PORT/DBNAME`: the host is an IP address (the
  component imports no name lookup) and there is no password (a document
  carries no secret), so the server must accept the user without one
  (decision D70).
- **Servers.** PGlite served beside `scx` (`pglite-server` from
  `@electric-sql/pglite-socket`), a local Postgres, or a hosted one. Postgres
  support is tested on Postgres 16; PGlite is not compiled into the
  component (decisions D71, D72).
- **SQL.** `?` and `?NNN` placeholders become `$n` outside string literals,
  quoted identifiers, dollar quotes and comments; a statement that would
  start or end the transaction is refused before it is sent; each session
  runs in `TimeZone` `UTC`. Cells are typed by their type OID as the SQLite
  engine's are (integers and finite reals as numbers, `bool` as a boolean,
  `bytea` as hex).
- **Failures.** `sql.failed {code, message}`, `code` being the server's
  `SQLSTATE` (`23505`, `40001`, ...) or one of `rowcount`, `bound`,
  `statement`, `connection`, `changed`, `event`.

The memory and migration abilities pick this service for a `postgresql://`
`database`, with Alembic's revisions in the Postgres dialect from
[`abilities/memory/postgresql/`](../../abilities/memory/postgresql/). Because
`scx` binds every service a document names literally, a run that binds the
Memory ability grants both engines' resources.

## Testing

The unit tests in [src/tests.rs](src/tests.rs) include two ignored live
cases against a running server (`FLOPPY_PGLITE_ADDRESS`, its `IP:PORT`).
The `postgres_*` integration tests (`tests/tests/sql_service.rs`,
`migration.rs`, `memory.rs`, `identity.rs`, `cognition.rs`,
`communication.rs`, `reflection.rs`) each start a fresh server through
`support::Pglite` and are ignored unless one is configured:

```console
# A Postgres cluster made per test from Postgres 16's binaries.
FLOPPY_POSTGRES_BIN=/usr/lib/postgresql/16/bin \
  cargo test -p floppy-tests --test memory -- --include-ignored
```

`FLOPPY_PGLITE_SERVER` (pglite-socket's `dist/scripts/server.js`) runs them
on PGlite instead.
