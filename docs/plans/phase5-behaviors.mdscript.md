---
name: floppy-phase5-behaviors
description: Executes floppy port phase 5 (mosfet's learned behaviors and routines) by replacing the phase 4 autonomy, reflection and routines seams with mosfet's inventory, authored by the builder bot as SCXML documents and kept by the Memory ability, one provable slice at a time.
project: floppy
repository: github.com/gabewillen/floppy
branch: claude/project-thread-brkerr (merged into main when green)
mosfet_source: github.com/stateforward/mosfet.py src/mosfet/behavior, src/mosfet/abilities/cognition/{autonomy,reflection,routines,inventory}.py
design: mosfet's inventory, lifecycle and events unchanged; a behavior's program is an SCXML document the builder authors instead of Starlark the model writes
created: 2026-10-10
---

<!-- mdscript: use the mdscript-exec skill or read [spec.md](https://raw.githubusercontent.com/gabewillen/mdscript/main/spec.md) -->

## Rules For Every Slice

* everything the phase 4 and phase 3 plans' rules say still holds: `.scx` only for bots, abilities, behaviors and routines; no `<script>`; payloads checked by `event:schema` only (D42); conds only correlate; SQL lives in top-level `<data>`; memory is the Memory ability (`abilities/memory/memory.scx`, over the sql service)
* names are mosfet's (events, payload fields, statuses, lifetimes, table columns), with mosfet = floppy; floppy names only what SCXML forces (a state that holds a gated invoke)
* a behavior or routine is one SCXML document stored as `bot_behavior.source` and run as a child by `<invoke type="scxml"><content expr="source"/></invoke>` (no file, no srcexpr); like every invoked child it gets no schema files, so it carries no `event:schema` references
* the builder bot (`bots/builder/builder.scx`) is mosfet's Revision authoring and check loop (the model writes the program, it is validated, diagnostics drive the retry); no Starlark port
* each slice keeps every trace unchanged unless it says it changes them
* commit atomically after each slice's proof is green and push the branch; merge into `main` when the full proof is green

## Map

| mosfet | floppy |
| --- | --- |
| `behavior.Source` (Starlark: `input_event`, `output_event`, `triggers`, `description`, `lifetime`, `hsm.define`) | an SCXML document; `name`, `triggers`, `description`, `lifetime` are its `bot_behavior` row's columns |
| `Instance` (`status` `ACTIVE`/`DRAFT`/`BROKEN`, `lifetime` `turn`/`persistent`, usage counters) | the `bot_behavior` and `bot_behavior_trigger` rows (mosfet's 0001/0002 DDL, run by `abilities/memory/memory.scx`) |
| `inventory.store_behavior` (delete then insert, `cause`) | one memory transaction: delete triggers, delete row, insert row, insert triggers |
| `Behavior.apply` (input event, output selections, 1 s silence bound) | the turn behavior contract below |
| `hsm.every` / `hsm.at` routines, `bot.behavior.tick` `TickData` | a persistent routine document: `<send delay>` loops that send `bot.behavior.tick` to `#_parent` |
| `CreateData`, `ChangeData`, `BreakData`, `TickData` | `abilities/schemas/bot.behavior.{create,change,break,tick}.json` |
| Revision (`ChangeWriteInput`, `_check_change`, retry while diagnostics change) | reflection invokes the builder with a spec built from the `ChangeData` and the turn |

### Turn Behavior Contract

* the invoker passes `<param name="turn">` and `<param name="input">` (the turn's stimulus data plus `focus` and `focus_candidates`, mosfet `behavior_input_payload`)
* the behavior ends in a final state whose `<donedata>` is `{turn, handled, selected}`: `handled` true with `selected` mosfet's output selections (`[{event, target?, data?, reason?}]`), or `handled` false (mosfet `unhandled`)
* no answer within 1 s (`TURN_SILENCE_BOUND`) is mosfet's `silent`; an `error.*` for the child is `failed`

### Persistent Routine Contract

* no params; each scheduled transition is a `<send delay>` loop (at least 60 s, `ROUTINE_MIN_EVERY`) that sends `bot.behavior.tick` `{name, trigger, scheduled_at, fired_at, output}` to `#_parent`, `output` a non-empty selection list

## Done So Far

* mosfet's inventory events are contracts already (phase 4): `abilities/schemas/bot.behavior.{create,change,break,tick}.json`, listed in `tests/tests/contracts.rs`

* Match Turn Behaviors In Autonomy: `abilities/autonomy/autonomy.scx` loads the ACTIVE turn behaviors whose trigger is the stimulus's event name (one Memory ability transaction, by name) and runs each source as the child `candidate` by `<content expr>` with a 1 s silence bound; handled persists `used_count`/`last_used_at`, a source that cannot run persists `failed_count`/`last_failed_at`; the cognition passes `stimulus` and `database` (`<data id="database">`, null until the body passes it); tests `autonomy_runs_the_learned_behaviors_the_stimulus_triggers`, `when_no_learned_behavior_handles_the_turn_intuition_runs`. A source that does not compile fails the invocation with an `error.execution` that carries no invokeid (SCXML 3.12.2), so the candidate state takes any `error.*`

* Reflect On Turns: `abilities/reflection/reflection.scx` takes `<param name="database">` and `model` from the cognition and `bot.ability.reflection.input {turn, cognition_input, cognition_output}` (D62); `recalling` reads the prior episodes and the whole inventory in one Memory transaction, `processing` asks the generate service to select at most one of the inventory events through its enabled transitions (D60, mosfet `SELECT_INSTRUCTIONS`, a 30 s `bot.ability.reflection.select.hop.timed_out`), `breaking` marks the row BROKEN with its trimmed reason, and `storing` inserts the turn's `CognitiveEpisode`; failures answer `bot.ability.failed {message}` (D64); memory null keeps the phase 4 seam and its traces; tests `tests/tests/reflection.rs` (harness `tests/fixtures/phase5/reflection_harness.scx`): an empty selection stores the episode and the next turn recalls it, a break keeps the row BROKEN, an unknown break and two selections fail. SQLite's `randomblob` repeats across the sql service's invocations, so the episode's memory id is the next rowid in hex (as the phase 3 callers do)
* Revise Behaviors With The Builder: `revising` stores a create's DRAFT stub, loads the row, invokes `bots/builder/builder.scx` and sends it `builder.request` (mosfet `CHANGE_INSTRUCTIONS` and `ChangeWriteInput` as the spec, the turn behavior and persistent routine contracts as constraints); `done` replaces the row ACTIVE with grounded triggers and usage kept (lifetime by D61), `failed` marks it DRAFT `validation` and fails the turn; tests: a create is built and stored ACTIVE with its grounded trigger, a failed build leaves a validation draft
* Reconcile Routines: `abilities/routines/routines.scx` takes `<param name="database">` from the cognition (null keeps the `routines absent` log) and on each reconcile selects the ACTIVE `persistent` rows in name order through one Memory ability transaction; when their names or sources differ from the running set it raises `bot.ability.routines.reconciled` and restarts the chain, `abilities/routines_runner/routines_runner.scx` (each runner invokes the first routine by `<content expr>` and itself for the rest, and passes ticks whose name is the routine's up); a failed load logs `bot.ability.routines.reconcile.failed` and keeps the running set; a reconcile during a load re-enters it; a routine that ends or cannot start stays stopped (`routines.failed`). Tests `tests/tests/routines.rs` under `tests/fixtures/phase5/routines_harness.scx`; the cognition traces are unchanged

## Slices

Autonomy, routines, and reflection with revision are independent and can run in parallel threads; each takes the database path as `<param name="database">` from the cognition, which the phase 3 thread is wiring for reasoning.


### Reconcile Routines

* goal: routines selects the ACTIVE `persistent` rows on each reconcile, starts each one as a child, restarts one whose source revision changed, stops removed ones, and forwards a running routine's `bot.behavior.tick` (mosfet `_reconcile_activity`)
* SCXML holds a fixed set of invokes per state, so the routines run as a chain: a runner document invokes the first routine by `<content expr>` and itself for the rest, and passes ticks up; a reconcile whose revisions changed restarts the chain

### Turn The Seams On

* goal: the cognition's `reflection_enabled` and `routines_enabled` follow the body's `memory`; the traces change once

### Document Phase 5

* goal: decisions, port plan, handoff and READMEs say what was built

## Decisions Made Under Delegation

D48, D49 and D55 were first numbered here, which collided with the migration layer (D48), the sql row-count guard (D49) and phase 7's plan (D55); [decisions.md](../decisions.md) records them as D73, D74 and D75, and D60 to D64 unchanged.

* D73 (first D48): a behavior's program is an SCXML document the builder authors, stored in `bot_behavior.source` and invoked from it by `<content expr>`; mosfet's Starlark globals other than the program (`triggers`, `description`, `lifetime`) are the row's columns
* D74 (first D49): the turn behavior contract keeps the phase 4 builder seam's `<donedata>` `{turn, handled, selected}`: a final state is SCXML's one way to answer an invoker exactly once, and it carries mosfet's handled/unhandled distinction
* D60: reflection selects through the enabled transitions: its `processing` state holds one transition per inventory event (`bot.behavior.create`, `change`, `break`) with its `event:schema`, `<decision:choice>` reads them and `<decision:selection>` builds the strict schema, and the one selection is sent to `#_internal`, so scxml.rs checks its data against the event's schema before the transition taking it applies it (mosfet `_SelectedEvent`)
* D61: a behavior's `lifetime` is `persistent` when its built document sends `bot.behavior.tick` (the persistent routine contract) and `turn` otherwise: mosfet reads it from the program's `lifetime` global, and the tick send is the SCXML program's equivalent
* D62: `bot.ability.reflection.input` carries mosfet's `cognition_input` (`{stimulus, focus, focus_candidates}`, a `[floppy field]` description, required): mosfet passes the live `InputData` (`SkipJsonSchema`), and reflection needs it for the episode, the select prompt, the builder's spec and the grounded triggers
* D63: the inventory and episode reads return one JSON value per statement (SQLite `json_group_array`/`json_object`), so the document parses one cell instead of mapping rows; the write SQL is mosfet's clauses (delete then insert for `store_behavior`, an `UPDATE` for `mark_broken`/`mark_draft`, which leave the same row), and timestamps are SQLite's (`strftime('%Y-%m-%dT%H:%M:%SZ', 'now')`, mosfet `utc_now_iso`)
* D64: a reflection failure answers the cognition `bot.ability.failed {message}` (mosfet `_fail_from_stage`) with mosfet's messages, and the cognition logs it as `cognition.reflection.failed`; a revision whose build fails marks the row DRAFT (`validation`) and fails the turn without storing its episode, as mosfet's `_abandon`
* D75 (first D55): the routines run as one chain of runners that restarts as a whole when any routine's name or source changes (mosfet restarts only the changed routine): SCXML holds a fixed set of invokes per state, so the running set is a parameter of one invocation, and a routine has no state worth keeping across a restart (mosfet replays nothing after one either)
