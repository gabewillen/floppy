# builder

The builder bot builds SCXML documents (never Rust). It is itself an SCXML
machine, [`builder.scxml`](builder.scxml), used by invoking it and talking to
it with events (decision D9 in [../../docs/decisions.md](../../docs/decisions.md)):

```xml
<invoke id="builder" type="scxml" src="builder.scxml"/>
<send event="builder.request" target="#_builder">
  <content expr="({spec: 'a traffic light with a pedestrian button'})"/>
</send>
<transition event="builder.done" .../>
<transition event="builder.failed" .../>
```

## Contract

| Event | Direction | Data ([`schemas/`](schemas)) |
| --- | --- | --- |
| `builder.request` | to the builder | `{spec, constraints?, max_attempts?, model?}`: the specification, extra constraints (strings), the failure budget (1 to 8, default 3), the `generate` model (default `gpt-6-luna`) |
| `builder.done` | to `#_parent` | `{scxml, schemas, tests, report}`: the built document, its input event schemas (`{event, schema}`), its test scripts, and the report (attempts, failures, fixes applied, warnings, the plan, the triage decisions, the failure history, the simulation results) |
| `builder.failed` | to `#_parent` | `{reason, diagnostics}`: `invalid-request`, `attempts-exhausted`, or `abandoned`, with the last diagnostics (`{kind, code, message}`) |

The builder answers exactly once and then finishes. `intake` has no timeout:
the invoker owns the builder's lifetime (cancel the invoke to stop it).

`builder.scxml` carries the contracts as inline `event:schema` attributes:
an invoked child is compiled at run time and receives no schema files (a
schema reference would reject it), so `schemas/*.json` are the canonical
files and the `builder_bot` test of `nemobot-host` keeps the inline copies
equal to them. The engine treats them as metadata only; `intake` checks the
request with its `cond` and answers `builder.failed` (`invalid-request`)
otherwise.

## Topology

```
intake -> design -> author -> validate -> simulate -> package -> done
                       ^          |  ^        |
                       |     apply_fixes      |
                       |          v           v
                       +--- triage <- retry_gate -> failed
                            fallback
```

* `design`: `gen:generate` with the `topology_plan` structured-output schema.
  A redesign sends the plan it replaces (`previous_plan`) with the
  diagnostics; a redesign whose generation failed keeps that plan for the
  next one.
* `author`: `gen:generate` with the `scxml_draft` schema: the document (plain
  SCXML with inline `event:schema` on its input transitions), the input event
  schemas as JSON text, and one to eight test scripts (input events with
  delays, a settle time, the outputs expected in order, and whether the
  machine finishes). A repair sees the failed draft, its tests, and the
  diagnostics.
* `validate`: the draft must keep the draft contract (below); its schemas
  must parse; `sx:validate` with
  `applyfixes="machine-applicable"`; when the fixes change the text,
  `apply_fixes` takes them and validates again (at most 2 passes per draft);
  otherwise the error diagnostics feed back to `author`.
* `simulate`: each test script runs the draft as a fresh invoked child
  (`<invoke idlocation="candidate_id" type="scxml"><content expr="scxml_text"/></invoke>`,
  so each test has its own generated invoke id and nothing a cancelled
  earlier child left on the queue counts), sends the script's events through
  `#_<candidate_id>` with their delays, collects every event that child
  sends to `#_parent` until the settle time (at most `sim_test_cap_ms`,
  120 s) or its `done.invoke`, and compares: the expected outputs must
  appear in order (others may come between) and the child must finish
  exactly when the test says so. The builder's own control events
  (`sim.drive`, `sim.settle`, and the simulation's `builder.timeout`) are
  taken only without an invoke id and only with the `run` tag of the test
  run (`sim_run_seq`) or simulation (`simulation`) that sent them: a child
  that sends events of those names has them recorded as outputs (or, once
  its invocation is cancelled, dropped), and a control event an earlier run
  left on the queue (for example the `sim.drive` of a run whose invocation
  failed to start) is dropped.
* `package`: sends `builder.done`.
* `retry_gate`: every failure (generation, draft, validation, simulation,
  timeout) passes through it; it counts the failure and fails the build at
  `max_attempts`, so every loop is bounded.
* `triage`: `decision:choose` picks among the state's enabled `event:schema`
  transitions: `triage.repair` (guarded by `plan !== null`, so it is offered
  only when there is a plan to repair from), `triage.redesign`, and
  `triage.abandon`.
* `fallback`: the cond-based triage when the decision is unavailable (no
  `TYPESAFE_API_KEY` granted or a backend failure: `error.execution`) or
  abstains (`decision.abstain`): redesign without a plan, else repair.

Time is bounded in two ways. `gen:generate` is a synchronous extension
call inside `onentry`, so no delayed send can preempt it: each generation
is bounded by the host's outbound HTTP timeouts (10 s to connect, 120 s
per request) and a timed-out request is `error.execution`, a failure like
any other. The simulation is bounded in the topology: `simulate-timeout`
is a delayed send cancelled on exit that fires after
`max_tests * sim_test_cap_ms` plus 60 s, longer than the longest
simulation `validate` admits (8 tests of at most 120 s each).

## The draft contract

Drafts are model output, and a draft runs as an invoked child of the
builder's session, which would inherit every namespace package and `env`
value the run binds. So `validate` refuses, before `sx:validate`'s verdict
counts, a draft that has a document type declaration (so no entity can
hide markup), declares a namespace other than SCXML and `event`, mentions
`xmlns` anywhere but such a declaration, or holds an `invoke` or `script`
element (with any prefix), a `src` or `srcexpr` attribute, a `typeexpr`
attribute, or a `type` other than `internal`, `external`, `deep`,
`shallow`, or the SCXML Event I/O Processor (so no BasicHTTP send). The
diagnostics (`draft-contract`) feed back to `author`. The check is textual
and fails closed: a prefix is any run of characters other than XML white
space and markup delimiters, so a non-ASCII prefix (`xmlns:é`, `<é:invoke>`)
is read like any other, and a forbidden pattern inside a comment or a CDATA
section is refused too.

## Bindings

A session that runs the builder binds the `scxml`, `generate`, and `decision`
namespace packages, an `extension-call-fuel` large enough for `sx:validate`
(1 billion; see `namespaces/scxml`), and grants `OPENAI_API_KEY` and
`OPENAI_BASE_URL` for `generate` and `TYPESAFE_API_KEY` and
`TYPESAFE_BASE_URL` for `decision`. No backend URL is compiled into a
component, so each base URL comes from the env file or, when the file does
not set it, from the config's non-secret `env-defaults` (for example
`{"OPENAI_BASE_URL": "https://api.openai.com/v1"}`). Drafts are validated without
packages, so a built machine uses no extension namespace or service.

## Proof

`host/tests/builder_bot.rs` invokes the builder from a parent machine with
the spec "a traffic light with a pedestrian button" against a fake
Chat Completions and `system_one` backend on 127.0.0.1: an invalid first
draft (`SCXML005`) goes back to `author` through `triage.repair`, the second
draft's only error is fixed by its machine-applicable fix, and the fixed
draft runs its two test scripts as two invoked children (distinct invoke
ids) before `builder.done`; the repair request carries the failed draft and
its tests. Other cases cover a redesign that sends the plan it replaces,
a failed redesign keeping that plan, `triage.repair` not being offered
without a plan, a draft that breaks the draft contract never running (also
through a non-ASCII namespace prefix or an entity), a candidate's
`sim.settle` and `sim.drive` being recorded as outputs, a cancelled
candidate's `builder.timeout` not ending the next simulation, a failed
invocation's `sim.drive` not driving the next test, the fallback triage
exhausting `max_attempts`, and an invalid request. The
ignored `live_builder_through_an_env_file` test runs the same spec with the
real backends; [`examples/traffic_light`](examples/traffic_light) is one such
live build (its `report.json` records one attempt, so no triage ran), kept
because it validates and passed its own simulation. It was built by the
first builder (commit `89ec4d05`), before the round-1 review fixes (the
draft contract, per-test invoke ids, the simulation timeouts), so its
`report.json` has no per-test `invokeid`. The traffic_light example is
model output, kept as built: for example its `onexit` handlers do not
cancel the phase timers, which its tests do not detect. No live
`builder.done` of the current builder exists yet. The development session
log reports two live runs of the current builder (2026-10-06,
`gpt-4o-mini` and `gpt-4.1`) that ended in `builder.failed`; the
repository holds no record of them (the live test overwrites the
untracked `target/builder-live/failed.json`), so their outcomes and causes
are unproven.
