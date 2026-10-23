---
name: floppy-phase7-pglite-defaults
description: Executes floppy port phase 7 (memory on Postgres through PGlite, refreshed defaults, and release preparation without publishing) one provable slice at a time, split so separate threads can build them in parallel.
project: floppy
repository: github.com/gabewillen/floppy
branch: one branch per slice thread, merged into claude/project-thread-brkerr and main when green
design: the Memory ability's contract unchanged (one transaction of `Statement`s); a second engine behind it, chosen by the run; nothing is published or released
created: 2026-10-10
---

<!-- mdscript: use the mdscript-exec skill or read [spec.md](https://raw.githubusercontent.com/gabewillen/mdscript/main/spec.md) -->

## Rules For Every Slice

* the phase 3 to 6 plans' rules hold
* `abilities/memory/memory.scx` keeps its Memory surface: callers' statements and results do not change with the engine
* the Postgres engine: `?` placeholders rewritten to `$n` outside string literals, `created_at TIMESTAMPTZ DEFAULT NOW()`, results as `Row{columns, values}`; floppy migrates the full head schema, not only `bot_memory`, so behaviors and short-term memory are storable
* no release, tag, publish or `SHA256SUMS` upload until the user says the repositories are public; preparing them is in scope
* no package default endpoint (D31): every run still names `FLOPPY_BASE_URL` and `FLOPPY_API_KEY`

## Done So Far

* Spike PGlite In A Component (verdict: **socket**): the Postgres engine is a service that speaks the Postgres wire protocol over `wasi:sockets/tcp` (granted with `--wasi URI=inherit-network`, which `scx` already links), to PGlite served beside `scx` (`pglite-server` from `@electric-sql/pglite-socket`, or pglite-oxide's `pglite-proxy`) or to any Postgres, hosted included; no scxml.rs change. Proof: `tests/spikes/pglite` (a `wasm32-wasip2` service on `wasip2`'s `wasi:sockets/tcp` directly, since the standard library's sockets go through wasi-libc and also import the filesystem and UDP; composed with `services/sql/wasi-cli-deny.wat`), `tests/fixtures/phase7/spikes/pglite/root.scx`, and `tests/tests/phase7_spikes.rs`: `SELECT 1` returns its row from PGlite 0.5.8, `SELECT 1/0` comes back as the server's message and the next invocation still answers, a duplicate key is reported as such, and without the grant the run is refused before it starts
* Spike, measured and rejected for now: PGlite compiled into the component. PGlite's WASI build (`pglite.wasi`, PostgreSQL 17.5, a 23 MB wasip1 core module importing only `wasi_snapshot_preview1`, from electric-sql/pglite-build `portable`) runs `SELECT 1` in about 1 ms after a 4.2 s first `initdb`, but it is built without setjmp/longjmp (`pgl_sjlj.c`: "sjlj exception handler off" under `__wasi__`), so every SQL error calls `abort()` and traps the instance: no `sql.failed`, no rollback, the Memory contract cannot hold. setjmp/longjmp on WASI needs the Exception-handling proposal: wasi-sdk 25 (LLVM 19) emits only legacy `try`, which binaryen's `wasm-opt --translate-to-exnref` turns into `try_table` (a setjmp/longjmp test recovers under wasmtime with exceptions on), but `scx`'s wasmtime 49 is built without its `gc` feature, so `wasm_exceptions` is off and such a module is refused ("exceptions proposal not enabled"); with `gc` and `gc-null` it validates
* Proposed scxml.rs change for an embedded engine later (not made): build `scxml-host-wasmtime`'s wasmtime with the `gc` and `gc-null` features so wasmtime's `wasm_exceptions` (on by default with `gc`) admits exception-handling components; no new scxml.rs name. PGlite would then also need a WASI build with `-mllvm -wasm-enable-sjlj` (translated to exnref), linked into the service with a canonical-ABI shim, and its `PGDATA`/`PREFIX` environment and share files supplied without `wasi:cli`
* Add Postgres Engine To Memory (done): `services/pgsql` (`floppy_service_pgsql`) runs one transaction of `Statement`s over the Postgres wire protocol on `wasi:sockets/tcp` (`BEGIN`, each statement through the extended query protocol with text parameters, `COMMIT`, or `ROLLBACK` on any failure) and answers `sql.result`/`sql.failed` exactly as `services/sql` does; `?` and SQLite's `?NNN` placeholders become `$n` outside literals, quoted identifiers, dollar quotes and comments; transaction control is refused before it is sent; cells are typed by their type OID like the SQLite engine's (integers and finite reals as numbers, `bool` as a boolean, `bytea` as hex). Its `database` is a connection URI, `postgresql://USER@IP:PORT/DBNAME`, with no password (a document carries no secret; a run that needs one waits for a host mechanism for socket credentials). `abilities/memory.scx` picks the engine from `database` (a file path is SQLite, a `postgresql://` URI is Postgres) rather than an `engine` param, so no invoker changes; on Postgres it reads and records the schema version in Alembic's `alembic_version` table (`'0001'`, `'0002'`) and migrates revisions 0001 and 0002 in the Postgres dialect, the defaulted `created_at`/`updated_at` as `TIMESTAMPTZ DEFAULT NOW()`. Because `scx` binds every service a document names literally, every run that binds Memory grants both engines: `--dir .../services/sql=HOST::/memory` and `--wasi .../services/pgsql=inherit-network`. Proof: the unit tests of `services/pgsql` (one live against PGlite), `tests/tests/memory.rs` (the three cases on SQLite and as `postgres_*` on a fresh PGlite server each, `support::Pglite`, ignored unless `FLOPPY_PGLITE_SERVER` names pglite-socket's `dist/scripts/server.js`), contracts, and the cognition tests (autonomy remembers through the ability) unchanged. The abilities' own statements followed in the next entry.
* Make The Abilities' SQL Portable (done; every ability's SQL runs on both engines): their statements are SQL both engines run, with no dialect switch. A memory id is the next row count as 32 zero-padded digits (`substr` of zeros `||` `CAST(COUNT(*) + 1 AS TEXT)`) instead of `printf`; JSON rows are told by `content_format = 'application/json'` instead of `json_valid`; recall orders by `created_at, memory_id` instead of `rowid`; identity's insert takes the column default for `created_at` instead of `strftime` with milliseconds (two adoptions in one second order by `memory_id`); a timestamp column written as `YYYY-MM-DDTHH:MM:SSZ` (autonomy's `last_used_at`/`last_failed_at`, reflection's `status_updated_at`) is `replace(substr(CAST(CURRENT_TIMESTAMP AS TEXT), 1, 19), ' ', 'T') || 'Z'`, UTC on both engines since `services/pgsql` starts its session with `TimeZone` `UTC` (PGlite is GMT regardless); and reasoning's optional context is `CAST(?2 AS TEXT) IS NULL`, since Postgres cannot type a bare `$2 IS NULL`. Proof: each ability's memory test on SQLite unchanged (traces included) and as a `postgres_*` variant on a fresh PGlite server (`support::use_database` points the copied body at it): `identity.rs` (an adopted name is recalled by the next run), `cognition.rs` (a reasoned turn is recalled by the next, which also runs identity and conversation on Postgres; autonomy runs the learned behaviors and counts their uses and failures), and `communication.rs` (the conversation recalls and remembers each input). Reflection builds no JSON in SQL any more: it selects the episodes, the behavior rows (named as the inventory's fields) and their triggers plainly, and its datamodel assembles them (`Object.fromEntries` over each row's columns, `examples` parsed, the counters as numbers, the triggers by name); a create's DRAFT stub and its triggers are written only for a name the recalled inventory does not hold, and each distinct trigger is its own `INSERT`. Its six tests run on both engines (`tests/tests/reflection.rs`, `postgres_*`; the harness `tests/fixtures/phase5/reflection_harness.scx` inspects with portable SQL too). Routines issue no SQL.
* Migrations And Transactions On Both Engines (done, decisions D48 to D51): the sql service keeps only transactions (one per `<invoke>`, or an open transaction across `sql.execute`, `sql.commit` and `sql.rollback`, optimistic by replay), and migrations are Alembic's: the pure element `<migration:plan>` (`namespaces/migration`) and the runner `abilities/migration/migration.scx`, which reads `alembic_version`, plans, and applies every step in one transaction. The revisions are one file per engine with the same ids (`abilities/memory/sqlite/`, `abilities/memory/postgresql/`), and `services/pgsql` takes the sql service's whole contract (`rowcount`, `sql.failed {code, message}` with the server's `SQLSTATE`, open transactions). Proof: `tests/tests/migration.rs`, `memory.rs` and `sql_service.rs`, with their `postgres_*` cases on a fresh server per test (`support::Pglite`: PGlite through `FLOPPY_PGLITE_SERVER`, or a Postgres cluster made from `FLOPPY_POSTGRES_BIN`); the Postgres cases were run on Postgres 16, and the maintainer chose SQLite for now (2026-10-10), so running them on PGlite through pglite-socket is future work.
* Refresh Defaults (done): README.md's "Defaults" section states every variable, the model (`Qwen/Qwen3.8-Flash-Next`), the speech and voice models, and every timeout and bound with the document and `<data>` that holds it; `.env.example` names every variable a bot reads, without values; `tests/tests/defaults.rs` keeps the README, `.env.example` and the documents in step.
* Prepare Releases (done, nothing published): `scripts/build-canonical-components.sh` (the pinned `linux/amd64` `rust:1.98.0` image, checking the manifests' `validator-sha256` reproduces), `scripts/check-package-versions.sh`, `scripts/prepare-release.sh` (the assets and `SHA256SUMS`) and `scripts/check-release-layout.sh` (the dry run: a loopback GitHub mirror that `scx` resolves every bot's namespaces and services from); the flow and what waits for going public are in `docs/release.md`.
* Document Phase 7 (done): decisions D69 to D72, the port plan's providers table and phase 7 row, the handoff, README.md's layout and Postgres section, `services/sql/README.md` and `services/pgsql/README.md`, and AGENTS.md's namespace list.

## Slices

These run in parallel; the first is a spike whose answer the second builds on, and the rest are independent.

### Spike PGlite In A Component

* goal: find how PGlite (Postgres compiled to WebAssembly with Emscripten) can serve an invoked service: embedded in a `wasm32-wasip2` component, or run by the host and reached over a socket (`pglite-socket`, the Postgres wire protocol over WASI sockets), or as a hosted Postgres with the same wire client
* output: a recorded answer in this plan's Done So Far with the measured option, and the smallest working proof (a component that runs `SELECT 1` on PGlite)
* if the only working option needs a scxml.rs host change, write it as a proposed scxml.rs name and stop there

### Add Postgres Engine To Memory

* goal: the Memory ability runs on Postgres when the run names it: `services/pgsql` (or the spike's choice) answers `sql.result`/`sql.failed` exactly as `services/sql` does, and `abilities/memory/memory.scx` takes an `engine` param (`sqlite` default, `postgres`), migrating with the Postgres dialect of revisions 0001 and 0002 (`TIMESTAMPTZ`, `$n`)
* files: the service, `abilities/memory/memory.scx`, `tests/tests/memory.rs` (the same cases on both engines)
* done when: every memory, autonomy, reasoning, identity and conversation test passes on both engines

### Refresh Defaults

* goal: the defaults floppy ships are current and stated in one place: the builder's and stages' model names (the Sparks' current models), timeouts, `.env.example` with every env var the bots and services read (names only, no values), and the README quick start
* files: `bots/builder/builder.scx`, `abilities/*.scx` model `<data>`, `.env.example`, `README.md`, `bots/*/README.md`

### Prepare Releases

* goal: everything a release needs, built and checked locally but not published: canonical component builds that reproduce pinned digests (Docker Linux, D27), a script that writes `SHA256SUMS` for the repository namespaces and services, and a dry-run check that the release layout resolves the documents' service URIs from a local directory
* files: `scripts/`, `docs/`; no tags, no uploads

### Document Phase 7

* goal: decisions, port plan (phase 7 row, providers table), handoff and READMEs say what was built and what waits for going public

## Decisions Made Under Delegation

These were first numbered D53 to D56 here, which collided with phase 5's D55; [decisions.md](../decisions.md) records them as D69 to D72.

* D69 (first D53 and D54): Postgres is a second engine behind the unchanged Memory ability, selected by the run, and SQLite stays the default; the Postgres engine migrates the full head schema, not only `bot_memory`
* D70 (first D55 and D56, from the spike): the Postgres engine is a service on the Postgres wire protocol over `wasi:sockets/tcp`, reaching PGlite served beside `scx` or any Postgres; the Memory ability's `database` names the engine (a file path is SQLite, a `postgresql://` connection URI is Postgres), and a run binding Memory grants both engines' resources
* D71 (from the spike): PGlite is not compiled into a component until `scx` admits exception handling
* D72: Postgres support is tested on Postgres 16 (`FLOPPY_POSTGRES_BIN`); the maintainer chose SQLite for now, so the suite on PGlite through pglite-socket is future work
