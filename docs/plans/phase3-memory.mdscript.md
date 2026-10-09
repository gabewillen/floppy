---
name: floppy-phase3-memory
description: Executes floppy port phase 3 (mosfet's Memory ability on SQLite) as an invoked Wasm service, services/memory, and replaces the phase 4 memory seams with invocations of it, one provable slice at a time.
project: floppy
repository: github.com/gabewillen/floppy
branch: claude/project-thread-r7141w (from claude/project-thread-brkerr f56de3f)
mosfet_source: github.com/stateforward/mosfet.py src/mosfet/abilities/memory, src/providers/sqlite_memory
scxml_rs_subtree: febf633
design: mosfet's transaction contract unchanged (one apply = one transaction of parameterized statements), SQLite compiled into the service component
created: 2026-10-09
---

<!-- mdscript: use the mdscript-exec skill or read [spec.md](https://raw.githubusercontent.com/gabewillen/mdscript/main/spec.md) -->

## Rules For Every Slice

* read `docs/handoff.md`, `docs/decisions.md` and `AGENTS.md` before the first slice of a session; trust them over `docs/port-plan.md`
* everything the phase 4 plan's [Rules For Every Slice](phase4-bot-core.mdscript.md#rules-for-every-slice) says still holds: `.scx` documents only for bots and abilities, no `<script>` and no `function`, payloads checked by `event:schema` only (decision D42), conds only correlate, every cross-document `<send>` visible, mosfet event names byte for byte
* memory is I/O, so it is an invoked service (`<invoke type="https://github.com/stateforward/floppy/services/memory">`), never executable content and never a namespace element (AGENTS.md, decision D13)
* the service's contract is mosfet's Memory ability (`mosfet/abilities/memory/store.py`): one invocation is one transaction of ordered, parameterized statements that all commit or all roll back; the documents own their SQL exactly as mosfet's domain code owns its SQLAlchemy clauses
* every SQL text a document sends lives in a top-level `<data>` (where the 160-character `expr` bound does not apply); bind values go in `parameters`, never into the SQL text
* the only allowed non-.scx artifacts are those of the phase 4 plan plus `services/memory/` and the build script's pinned C toolchain (wasi-sdk) for SQLite
* each slice keeps the phase 4 traces unchanged unless the slice says it changes them; rewrite them with `FLOPPY_TRACE_BLESS=1` only for that intended change and say so in the commit
* never edit `scxml.rs/` inside floppy; a needed scxml.rs change goes upstream first
* never run `find`; use `rg` and `ls`
* never print or commit `.env` values
* commit atomically after each slice's proof is green, and push the branch (pushing is authorized while the repositories are private; no pull requests)
* the full proof is `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`, plus `sh scripts/build-components.sh` when a component changed

## Done So Far

* 2026-10-09: mosfet's memory read. The `Memory` ability (`memory.py`, `store.py`) takes `bot.ability.memory.input {statements: [{sql, parameters}]}` (at least one), runs them in one transaction on its private SQLite engine (`sqlite+pysqlite`, migrated to the head schema by Alembic), and answers `bot.ability.memory.output {results: [{rowcount, rows: [{columns, values}]}]}` (one result per statement, in order) or, on any failure, rolls back and answers `bot.ability.failed {message}`. Domain code compiles SQLAlchemy Core clauses against `bot_memory` (`schema.py`) to `Statement`s: reasoning recalls prior episodes (`query_tags = 'cognitive_episode'`) and standing directives (`'standing_directive'`) in one transaction and retains one episode per handled turn (`episodes.py`, `directives.py`); identity recalls and inserts its adopted name (`query_tags = 'self_name'`, `subject_ref = 'self'`, `identity/name.py`); conversation recalls and remembers a relationship's payloads (`'conversation_memory'`, `conversation/memory.py`); reflection recalls episodes and stores behavior work (phase 5). The schema is migrations `0001` (`bot_memory`, `bot_behavior`, `bot_behavior_trigger`, `bot_stm_memory` and their indexes) and `0002` (`bot_behavior.lifetime`)
* 2026-10-09: SQLite builds for `wasm32-wasip2`: `rusqlite` 0.37 with `bundled` (SQLite 3.50 amalgamation, `libsqlite3-sys` 0.35) compiles with wasi-sdk 25 (`WASI_SDK_PATH`), and SQLite's own `__wasi__` port (`SQLITE_WASI`: threadsafe 0, no extension loading, the `unix-dotfile` VFS, no WAL) imports only `wasi:filesystem`, `wasi:io`, `wasi:clocks` and `wasi:cli` 0.2; scxml.rs's service host links `wasi:filesystem` from `wasmtime-wasi`'s 0.2 bindings under `--dir URI=HOST::GUEST` (scxml.rs `hosts/scxml-host-wasmtime/src/wasi.rs`)

## Document Structure Map

| mosfet unit | floppy artifact | kind |
| --- | --- | --- |
| `memory/memory.py` `Memory`, `store.py` `MemoryStore` (transaction apply) | `services/memory` (`floppy_service_memory`) | invoked service |
| `providers/sqlite_memory` (SQLite engine) | SQLite compiled into `services/memory` (`rusqlite`, `bundled`) | in the service |
| `memory/schema.py`, `migrations/versions/0001`, `0002` | the service's migrations, run on open to `PRAGMA user_version` 2 | in the service |
| `memory_model` topology (`idle` → `applying` with `hsm.defer`) | the invoking state: `<invoke>` on entry, `done.invoke.<id>` on completion, a `<send delay>` timer to preempt it | topology of each caller |
| `Statement`, `InputData`, `Row`, `StatementResult`, `OutputData`, `FailureData` | `<invoke>` `<param name="statements">`; `bot.ability.memory.output` and `bot.ability.failed`, described by `event:schema` in `abilities/schemas/` | event contracts |
| `episodes.py` `episode_select_input`/`episode_insert_input`, `directives.py` `directive_select_input` | SQL `<data>` and statements in `abilities/reasoning.scx` (`recalling`, `retaining`) | ability `.scx` |
| `identity/name.py` `name_select_input`/`name_insert_input`/`adopted_name` | SQL `<data>` in `abilities/identity.scx` (`recalling`, `adopting`) | ability `.scx` |
| `conversation/memory.py` recall and insert | SQL `<data>` in `abilities/conversation.scx` (`recalling`) and `abilities/communication.scx` | ability `.scx` |
| `MemoryGeneration`, `MemoryClassification`, `MemoryConsolidation`, `AssociativeMemory`, `StmMemory`, `ShortTermMemory`, `LongTermMemory` | not in phase 3: they are model-driven or stimulus-driven abilities over the same store; each becomes a `.scx` ability invoking `services/memory` when a later phase needs it | later |

## Next Steps

### Add Memory Service

* goal: mosfet's Memory transaction contract as a WASI 0.3 invoked service backed by SQLite in a granted directory
* files: `services/memory/` (`Cargo.toml`, `src/lib.rs`, `src/tests.rs`, `package/memory.service.json`), `Cargo.toml` (member, `rusqlite` pin), `Cargo.lock`, `scripts/build-components.sh` (component list, wasi-sdk pin, WASI pass-through and import allowlist), `tests/tests/memory_service.rs`, `tests/fixtures/memory/*.scx`, `tests/tests/service_packages.rs`
* events: `bot.ability.memory.output`, `bot.ability.failed` (mosfet's names, returned by the service)
* `<param>`s: `database` (required: an absolute guest path ending in a file name, inside a directory the run grants with `--dir https://github.com/stateforward/floppy/services/memory=HOST::GUEST`) and `statements` (required: a non-empty JSON array of `{sql, parameters?}`, `sql` a non-empty string, `parameters` an array of strings, finite numbers, booleans and nulls); anything else rejects the start with `error.execution`
* on open the service runs the migrations to `user_version` 2 inside their own transaction (mosfet 0001 and 0002, the same DDL), then runs the statements in one `BEGIN IMMEDIATE` transaction; a statement that returns columns yields its rows (`{columns, values}`), any other its changed row count; any failure rolls back and answers `bot.ability.failed {message}` with SQLite's message; a `BLOB` cell is returned as its lowercase hex text
* bounds (wasm only): statements per transaction, SQL bytes, parameters per statement, rows returned, and the result's encoded bytes under the 96 KiB event bound (a result over it fails the transaction rather than truncating it)
* build: `rusqlite` with `bundled`, compiled for `wasm32-wasip2` with the pinned wasi-sdk 25 (`WASI_SDK_PATH`, its version file checked); wasi-virt passes `wasi:filesystem` (and the `wasi:io` and wall-clock interfaces it uses) through to the host and virtualizes the rest; the import allowlist names exactly what remains
* tests: unit tests on the native build (parameters, statements, migrations, rollback, result shapes, bounds); `memory_service.rs` through `scx run`: an insert then a select in a second invocation sees the row (the file persists), a failing statement rolls back the whole transaction, a run binding the service without `--dir` is refused before the session starts, and a malformed `<param>` raises `error.execution`
* done when: the full proof is green and the component's imports are exactly the allowlist

### Grant Memory To The Floppy Bot

* goal: every floppy run has a memory directory, and the documents can describe the service's events
* files: `abilities/schemas/bot.ability.memory.output.json` (generated from mosfet's `OutputData`), `bots/floppy/README.md` (the `--dir` grant), `tests/tests/support/mod.rs` (a per-run memory directory granted to floppy runs), `tests/tests/contracts.rs` (the new schema)
* the body holds the guest database path `/memory/floppy.sqlite` as a `<data>` and passes it by `<param name="memory">` to the abilities that remember; nothing else in the body changes
* done when: the full proof is green and the phase 4 traces are unchanged

### Recall And Retain Reasoning Episodes

* goal: reasoning's `recalling` and `retaining` seams become invocations of the memory service, as mosfet `Reasoning._recall_activity` and `_retain_activity`
* files: `abilities/reasoning.scx`, `abilities/cognition.scx` (passes `memory` through), `tests/tests/cognition.rs`, the cognition traces
* `recalling` invokes the service with mosfet's two selects in one transaction (prior episodes with `query_tags = 'cognitive_episode'`, standing directives with `'standing_directive'`, each for the turn's context or none, oldest first, at most 50) and answers `bot.ability.reasoning.recalled` with `prior_episodes` and `memory_consulted: true`; the model prompt carries the recalled episodes and directives
* `retaining` inserts one episode (`{focus, focus_candidates, stimulus_name, output}` as JSON, `scope 'short_term'`, `kind 'task'`, the memory id from SQLite's `randomblob`) and answers `bot.ability.reasoning.retained` with `memory_written: true`
* a memory failure or a timeout fails the stage as mosfet does (`Reasoning memory recall failed: ...`)
* traces: the cognition traces change (the recall and retain replace the `memory absent` logs); blessed once, then stable
* done when: the full proof is green and a second turn's prompt carries the first turn's episode

### Recall And Adopt The Identity Name

* goal: identity's `recalling` and `adopting` seams become invocations of the memory service, as mosfet `Identity._recall_name` and `_adopt_name`
* files: `abilities/identity.scx`, `bots/floppy/floppy.scx` (passes `memory`), `tests/tests/identity.rs`, the identity traces
* `recalling` selects the adopted names (`query_tags = 'self_name'`, `subject_ref = 'self'`, oldest first) and takes the last as the name, else the `<param>` name; `adopting` inserts the name (`scope 'long_term'`, `kind` the name's kind, `created_at` with microseconds so two adoptions in one second order) and answers `adopted {name, persisted: true}`, or `bot.ability.identity.adopt.failed` when the transaction fails
* done when: the full proof is green and a second run of the bot on the same directory answers to the name the first adopted

### Recall And Remember Conversations

* goal: the conversation's `recalling` seam recalls the relationship's memories, and each committed message is remembered, as mosfet `conversation/memory.py`
* files: `abilities/conversation.scx`, `abilities/communication.scx` (passes `memory`), `tests/tests/communication.rs`, the communication traces
* recall selects `content` where `query_tags = 'conversation_memory'` and `context_ref` is the relationship's (the `<conversation:commit>` transcript key), oldest first, at most 50, and the output carries them as `memories`; each committed message is inserted with `scope 'short_term'`, `kind 'conversation'`
* done when: the full proof is green and a second run on the same directory recalls the first run's messages

### Close Phase 3 Docs

* goal: the decisions, port plan, handoff and READMEs say what was built
* files: `docs/decisions.md` (D44 onward), `docs/port-plan.md` (phase 3 row, providers table, memory seam convention), `docs/handoff.md`, `services/memory/README.md` if the service needs more than its crate docs, this plan
* done when: the docs name every new artifact and no doc still calls a replaced seam a seam

## Handle Blocked Slice

* record what failed, the command, and the root cause found in [Done So Far](#done-so-far)
* if the cause is in scxml.rs (a host or CLI gap), write the smallest upstream change and its names into [Proposed scxml.rs Names](#proposed-scxml-rs-names), report it, and continue with the slices that do not depend on it
* if the cause is SQLite on WASI (locking, files, clocks), prefer an SQLite compile-time option or pragma over code

## Report Phase 3

* report the commits to merge, the proof run for each slice, and the decisions taken under delegation

## Proposed scxml.rs Names

* none so far

## Decisions Made Under Delegation

* D44: the service keeps mosfet's Memory surface: `<param name="statements">` is `InputData.statements`, and it answers mosfet's `bot.ability.memory.output` (`OutputData`) or `bot.ability.failed` (`FailureData {message}`) rather than new `memory.*` names, because the ability it ports already names its events
* D45: SQLite is the C library compiled into the component (`rusqlite` `bundled`, wasi-sdk 25), not a Rust rewrite: it is the engine mosfet's `sqlite_memory` uses, so the same SQL runs unchanged, and phase 7's PGlite replaces the engine behind the same contract
* D46: the database is a file in a directory the run grants (`--dir URI=HOST::GUEST`), named by the invoking document's `database` `<param>`; the service holds no connection between invocations (each `<invoke>` opens, migrates, runs one transaction, and closes), so a cancelled invocation leaves at most an uncommitted journal SQLite rolls back on the next open
* D47: migrations are the service's, tracked by `PRAGMA user_version` (mosfet's Alembic revisions `0001` and `0002`, same tables, columns, defaults and indexes); a database whose `user_version` is above the service's is refused
