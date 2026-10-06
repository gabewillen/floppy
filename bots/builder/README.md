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
| `builder.request` | to the builder | `{spec, constraints?, max_attempts?, model?, generate_timeout_ms?}`: the specification, extra constraints (strings), the failure budget (1 to 8, default 3), the generate service's model (default `gpt-6-luna`), and the time each generation may take before `builder.timeout` cancels it (1 ms to 1 h, default 300000) |
| `builder.done` | to `#_parent` | `{scxml, schemas, tests, report}`: the built document, its input event schemas (`{event, schema}`), its test scripts, and the report (attempts, failures, fixes applied, warnings, the plan, the triage decisions, the failure history, the simulation results) |
| `builder.failed` | to `#_parent` | `{reason, diagnostics}`: `invalid-request`, `attempts-exhausted`, or `abandoned`, with the last diagnostics (`{kind, code, message}`) |

The builder answers exactly once and then finishes. `intake` has no timeout:
the invoker owns the builder's lifetime (cancel the invoke to stop it).

`builder.scxml` carries the contracts as inline `event:schema` attributes:
an invoked child is compiled at run time and receives no schema files (a
schema reference would reject it), so `schemas/*.json` are the canonical
files and the `builder_bot` test (`tests/tests/builder_bot.rs`) keeps the
inline copies equal to them. The engine treats them as metadata only; `intake` checks the
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

* `design`: invokes the generate service
  (`https://github.com/stateforward/nemobot/services/generate@v0.1.0`, a
  fresh invoke id in `gen_id` each time) with the `topology_plan`
  structured-output schema; its `<finalize>` takes the plan from
  `generate.result` (or the code from `generate.failed`), and
  `done.invoke` moves on to `author`, or to `retry_gate` without a plan. A
  redesign sends the plan it replaces (`previous_plan`) with the
  diagnostics; a redesign whose generation failed keeps that plan for the
  next one.
* `author`: invokes the generate service the same way with the
  `scxml_draft` schema: the document (plain
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
* `triage`: `decision:menu` (synchronous executable content) writes the
  state's enabled `event:schema` transitions as the canonical menu:
  `triage.repair` (guarded by `plan !== null`, so it is on the menu only
  when there is a plan to repair from), `triage.redesign`, and
  `triage.abandon`. The decision service
  (`https://github.com/stateforward/nemobot/services/decision@v0.1.0`),
  invoked with that menu and the failure state, picks one, validates its
  data against the menu's schema, and emits it.
* `fallback`: the cond-based triage when the decision is unavailable (no
  `TYPESAFE_API_KEY` granted or a backend failure: `decision.failed`,
  `error.execution`, or `error.communication`), times out
  (`fallback:timeout`), or abstains (`decision.abstain`): redesign without
  a plan, else repair.

Model calls are invoked services, never executable content: each runs
while the session keeps processing events, and every bound on time lives in
the topology. `design` and `author` each arm `builder.timeout` after
`generate_timeout_ms` (default 300 s; the request may lower it) and
`triage` after `decision_timeout_ms` (60 s), tagged with the stage run
(`stage_run`) and cancelled on exit. A timeout that fires while the call is
in flight leaves the state, which cancels the invocation: the service is
cancelled, the host closes the request's streaming connection at once (so
the backend stops sending tokens), and no answer reaches anyone. The host's
own request timeout (600 s) is only a backstop. The simulation is bounded the same way:
`simulate-timeout` is a delayed send cancelled on exit that fires after
`max_tests * sim_test_cap_ms` plus 60 s, longer than the longest simulation
`validate` admits (8 tests of at most 120 s each).

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

A session that runs the builder binds the `scxml` and `decision` namespace
packages and the `generate` and `decision` service packages (each to its
verified component: `components.args`, which `scripts/build-components.sh`
writes), and [`builder.args`](builder.args) adds the rest:

| Argument | Why |
| --- | --- |
| `--resource builder.scxml=bots/builder/builder.scxml` | the document `parent.scxml`'s `<invoke src="builder.scxml">` loads |
| `--extension-call-fuel 1000000000` | `sx:validate` runs the whole validator in one extension call (see `namespaces/scxml`) |
| `--env OPENAI_API_KEY@<generate URI>`, `--env OPENAI_BASE_URL@<generate URI>` | the generate service's `env:apikey` and `env:baseurl` slots |
| `--env TYPESAFE_API_KEY@<decision URI>`, `--env TYPESAFE_BASE_URL@<decision URI>` | the decision service's slots |

No backend URL is compiled into a component, so each base URL comes from
the env file or, when the file does not set it, from the calling shell
(`scxml-cli` takes a granted variable's value from its `--env-file`, else
from the shell). A variable neither sets holds no value, so its slot is
absent: without `TYPESAFE_API_KEY` the builder triages with its fallback.
Drafts are validated without packages, so a built machine uses no extension
namespace or service.

## Running

The builder answers its invoker (`#_parent`), so it runs as the child of
[`parent.scxml`](parent.scxml), which invokes it with `autoforward="true"`
(every external event reaches the builder) and logs its one answer
(`builder.done` or `builder.failed`, data as JSON) before it finishes. From
the repository root, after `sh scripts/build-components.sh` and with
`scxml-cli` built with `--features wasmtime` (see the
[README](../../README.md#running-a-bot)):

```console
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button"}}' |
  OPENAI_BASE_URL=https://api.openai.com/v1 TYPESAFE_BASE_URL=https://api.typesafe.ai \
  scxml-cli run @target/components/components.args @bots/builder/builder.args \
    --env-file .env bots/builder/parent.scxml
```

Standard output carries one JSON record per line; the answer is the `log`
record labelled `builder.done` or `builder.failed`, and the run exits 0 once
the parent finishes. The two base URLs on the command line are defaults the
env file's values override.

## Proof

`tests/tests/builder_bot.rs` runs the command above (on the logical clock,
except where noted) with the spec "a traffic light with a pedestrian button" against a fake
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
exhausting `max_attempts`, an invalid request, and, on the wall clock, a
`builder.timeout` that fires while the design generation is in flight (the
fake backend holds its answer for 4 s): it preempts and cancels the call,
the builder redesigns, and the late plan never reaches the report. The
ignored `live_builder_through_an_env_file` test runs the same spec with the
real backends; [`examples/traffic_light`](examples/traffic_light) is one such
live build (its `report.json` records one attempt, so no triage ran), kept
because it validates and passed its own simulation. It was built by the
first builder (commit `89ec4d05`), before the round-1 review fixes (the
draft contract, per-test invoke ids, the simulation timeouts), so its
`report.json` has no per-test `invokeid`. The traffic_light example is
model output, kept as built: for example its `onexit` handlers do not
cancel the phase timers, which its tests do not detect. The first live
`builder.done` of the current builder is
[`examples/traffic_light_luna`](examples/traffic_light_luna) (2026-10-06,
`gpt-6-luna`; three attempts, two `triage.repair` decisions).
[docs/live-proof.md](../../docs/live-proof.md) records that run and every
other live result.
