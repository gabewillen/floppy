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

* Match Turn Behaviors In Autonomy: `abilities/autonomy/autonomy.scx` loads the ACTIVE turn behaviors whose trigger is the stimulus's event name (one Memory ability transaction, by name) and runs each source as the child `candidate` by `<content expr>` with a 1 s silence bound; handled persists `used_count`/`last_used_at`, a source that cannot run persists `failed_count`/`last_failed_at`; the cognition passes `stimulus` and `memory` (`<data id="memory">`, null until the body passes it); tests `autonomy_runs_the_learned_behaviors_the_stimulus_triggers`, `when_no_learned_behavior_handles_the_turn_intuition_runs`. A source that does not compile fails the invocation with an `error.execution` that carries no invokeid (SCXML 3.12.2), so the candidate state takes any `error.*`

## Slices

Autonomy, routines, and reflection with revision are independent and can run in parallel threads; each takes the database path as `<param name="memory">` from the cognition, which the phase 3 thread is wiring for reasoning.


### Reconcile Routines

* goal: routines selects the ACTIVE `persistent` rows on each reconcile, starts each one as a child, restarts one whose source revision changed, stops removed ones, and forwards a running routine's `bot.behavior.tick` (mosfet `_reconcile_activity`)
* SCXML holds a fixed set of invokes per state, so the routines run as a chain: a runner document invokes the first routine by `<content expr>` and itself for the rest, and passes ticks up; a reconcile whose revisions changed restarts the chain

### Reflect On Turns

* goal: reflection recalls prior episodes and every behavior, asks the generate service to select at most one of `bot.behavior.create`, `bot.behavior.change`, `bot.behavior.break` (mosfet `SELECT_INSTRUCTIONS`), applies it, and stores the turn's `CognitiveEpisode`
* break marks the row `BROKEN` with its reason; create and change go to the next slice

### Revise Behaviors With The Builder

* goal: create and change invoke the builder with a spec from the `ChangeData`, the existing source and the turn; `done` stores the document `ACTIVE` with its grounded triggers (mosfet `_grounded_triggers`); `failed` stores it `DRAFT` with reason `validation` (create first stores the DRAFT stub, as mosfet's preparing)

### Turn The Seams On

* goal: the cognition's `reflection_enabled` and `routines_enabled` follow the body's `memory`; the traces change once

### Document Phase 5

* goal: decisions, port plan, handoff and READMEs say what was built

## Decisions Made Under Delegation

* D48: a behavior's program is an SCXML document the builder authors, stored in `bot_behavior.source` and invoked from it by `<content expr>`; mosfet's Starlark globals other than the program (`triggers`, `description`, `lifetime`) are the row's columns
* D49: the turn behavior contract keeps the phase 4 builder seam's `<donedata>` `{turn, handled, selected}`: a final state is SCXML's one way to answer an invoker exactly once, and it carries mosfet's handled/unhandled distinction
