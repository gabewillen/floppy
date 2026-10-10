---
name: floppy-phase7-pglite-defaults
description: Executes floppy port phase 7 (memory on Postgres through PGlite, refreshed defaults, and release preparation without publishing) one provable slice at a time, split so separate threads can build them in parallel.
project: floppy
repository: github.com/gabewillen/floppy
branch: one branch per slice thread, merged into claude/project-thread-brkerr and main when green
mosfet_source: github.com/stateforward/mosfet.py src/providers/postgres_memory, src/mosfet/abilities/memory, examples/
design: the Memory ability's contract unchanged (one transaction of mosfet's Statements); a second engine behind it, chosen by the run; nothing is published or released
created: 2026-10-10
---

<!-- mdscript: use the mdscript-exec skill or read [spec.md](https://raw.githubusercontent.com/gabewillen/mdscript/main/spec.md) -->

## Rules For Every Slice

* the phase 3 to 6 plans' rules hold
* `abilities/memory.scx` keeps mosfet's Memory surface: callers' statements and results do not change with the engine
* mosfet's `postgres_memory` is the reference for the Postgres engine: `?` placeholders rewritten to `$n` (outside string literals, unlike mosfet's naive split), `created_at TIMESTAMPTZ DEFAULT NOW()`, results as `Row{columns, values}`; floppy migrates the full head schema (mosfet's `postgres_memory` creates only `bot_memory`, which would leave behaviors and short-term memory unstorable)
* no release, tag, publish or `SHA256SUMS` upload until the user says the repositories are public; preparing them is in scope
* no package default endpoint (D31): every run still names `FLOPPY_BASE_URL` and `FLOPPY_API_KEY`

## Done So Far

* Spike PGlite In A Component (verdict: **socket**): the Postgres engine is a service that speaks the Postgres wire protocol over `wasi:sockets/tcp` (granted with `--wasi URI=inherit-network`, which `scx` already links), to PGlite served beside `scx` (`pglite-server` from `@electric-sql/pglite-socket`, or pglite-oxide's `pglite-proxy`) or to any Postgres, hosted included; no scxml.rs change. Proof: `tests/spikes/pglite` (a `wasm32-wasip2` service on `wasip2`'s `wasi:sockets/tcp` directly, since the standard library's sockets go through wasi-libc and also import the filesystem and UDP; composed with `services/sql/wasi-cli-deny.wat`), `tests/fixtures/phase7/spikes/pglite/root.scx`, and `tests/tests/phase7_spikes.rs`: `SELECT 1` returns its row from PGlite 0.5.8, `SELECT 1/0` comes back as the server's message and the next invocation still answers, a duplicate key is reported as such, and without the grant the run is refused before it starts
* Spike, measured and rejected for now: PGlite compiled into the component. PGlite's WASI build (`pglite.wasi`, PostgreSQL 17.5, a 23 MB wasip1 core module importing only `wasi_snapshot_preview1`, from electric-sql/pglite-build `portable`) runs `SELECT 1` in about 1 ms after a 4.2 s first `initdb`, but it is built without setjmp/longjmp (`pgl_sjlj.c`: "sjlj exception handler off" under `__wasi__`), so every SQL error calls `abort()` and traps the instance: no `sql.failed`, no rollback, the Memory contract cannot hold. setjmp/longjmp on WASI needs the Exception-handling proposal: wasi-sdk 25 (LLVM 19) emits only legacy `try`, which binaryen's `wasm-opt --translate-to-exnref` turns into `try_table` (a setjmp/longjmp test recovers under wasmtime with exceptions on), but `scx`'s wasmtime 49 is built without its `gc` feature, so `wasm_exceptions` is off and such a module is refused ("exceptions proposal not enabled"); with `gc` and `gc-null` it validates
* Proposed scxml.rs change for an embedded engine later (not made): build `scxml-host-wasmtime`'s wasmtime with the `gc` and `gc-null` features so wasmtime's `wasm_exceptions` (on by default with `gc`) admits exception-handling components; no new scxml.rs name. PGlite would then also need a WASI build with `-mllvm -wasm-enable-sjlj` (translated to exnref), linked into the service with a canonical-ABI shim, and its `PGDATA`/`PREFIX` environment and share files supplied without `wasi:cli`

## Slices

These run in parallel; the first is a spike whose answer the second builds on, and the rest are independent.

### Spike PGlite In A Component

* goal: find how PGlite (Postgres compiled to WebAssembly with Emscripten) can serve an invoked service: embedded in a `wasm32-wasip2` component, or run by the host and reached over a socket (`pglite-socket`, the Postgres wire protocol over WASI sockets), or as a hosted Postgres with the same wire client
* output: a recorded answer in this plan's Done So Far with the measured option, and the smallest working proof (a component that runs `SELECT 1` on PGlite)
* if the only working option needs a scxml.rs host change, write it as a proposed scxml.rs name and stop there

### Add Postgres Engine To Memory

* goal: the Memory ability runs on Postgres when the run names it: `services/pgsql` (or the spike's choice) answers `sql.result`/`sql.failed` exactly as `services/sql` does, and `abilities/memory.scx` takes an `engine` param (`sqlite` default, `postgres`), migrating with the Postgres dialect of revisions 0001 and 0002 (`TIMESTAMPTZ`, `$n`)
* files: the service, `abilities/memory.scx`, `tests/tests/memory.rs` (the same cases on both engines)
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

* D53: Postgres is a second engine behind the unchanged Memory ability, selected by the run; SQLite stays the default
* D54: the Postgres engine migrates mosfet's full head schema, not only `bot_memory` as mosfet's `postgres_memory` does
