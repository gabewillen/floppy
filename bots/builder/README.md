# builder

The builder bot builds SCXML documents (never Rust). It is itself an SCXML
machine, [`builder.scx`](builder.scx) (decision D9 in
[../../docs/decisions.md](../../docs/decisions.md)). Run it directly, with the
request on standard input:

```console
# The locally built packages (the dev cache under target/).
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button"}}' |
  scx run --offline --cache-dir target/components/package-cache --env-file .env bots/builder/builder.scx
# floppy's latest release, once one is published (phase 7).
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button"}}' |
  scx run --env-file .env bots/builder/builder.scx
```

or invoke it as a child and send it the request; it ends with one of two
final states and its parent receives that state's done data as
`done.invoke.<id>` (SCXML 6.4):

```xml
<invoke id="builder" type="scxml" src="builder.scx"/>
<send event="builder.request" target="#_builder">
  <content expr="({spec: 'a traffic light with a pedestrian button'})"/>
</send>
<transition event="done.invoke.builder" .../>
```

## Contract

| What | How | Data ([`schemas/`](schemas)) |
| --- | --- | --- |
| `builder.request` | an event to the builder (standard input when run directly) | `{spec, constraints?, max_attempts?, model?, generate_timeout_ms?}`: the specification, extra constraints (strings), an optional failure cap (1 to 8, no default: without it only the no-progress rule ends the loop), the generate service's model (default `Qwen/Qwen3.8-Flash-Next`, a reasoning model), and the time each generation may take before `builder.timeout` cancels it (1 ms to 1 h, default 300000) |
| final state `done` | its `<donedata>` ([`schemas/done.json`](schemas/done.json)) | `{scxml, schemas, tests, report}`: the built document, its input event schemas (`{event, schema}`), its test scripts, and the report (attempts, failures, fixes applied, warnings, the plan, the triage decisions, the failure history, the simulation results) |
| final state `failed` | its `<donedata>` ([`schemas/failed.json`](schemas/failed.json)) | `{reason, diagnostics, history}`: `invalid-request`, `unconfigured` (the run holds no `FLOPPY_BASE_URL` or no `FLOPPY_API_KEY`; the first generation ends the build at once), `no-progress` (the same failure twice in a row), `attempts-exhausted` (the request's cap), or `abandoned`, with the last diagnostics (`{kind, code, message}`) and the failure history (every failure's attempt and diagnostics, as in `done`'s report) |

The builder answers exactly once, by finishing. Run directly, `scx run`
prints the done data in its `done` record and exits 0; invoked, the parent
gets it as the data of `done.invoke.<id>`. Progress goes to `<log>`, each
record labelled with the stage (`design`, `author`, `validate`,
`simulate`, `retry_gate`, `triage`, then `done` or `failed`); the
`retry_gate` record of each failure and the `failed` record name its first
diagnostic's kind, code, and message, which carries the cause (for a
generation, the generate service's failure code, a timeout, or which
error event ended it). `intake` has
no timeout: the invoker owns the builder's lifetime (cancel the invoke to
stop it).

`builder.scx` carries the contracts as inline `event:schema` attributes
(on the transitions that take an event, and on each final state's
`<donedata>`): an invoked child is compiled at run time and receives no
schema files (a schema reference would reject it), so `schemas/*.json` are
the canonical files and the `builder_bot` test
(`tests/tests/builder_bot.rs`) keeps the inline copies equal to them.
scxml.rs checks them where a request is created: `scx run` refuses a
`builder.request` on standard input that the schema rejects (an `error`
record, `machine-invalid-input`; the builder stays in `intake`), and an
invoker's send of one raises `error.execution` in the invoker. A request
that satisfies the schema but whose `cond` fails (a blank `spec`) ends in
`failed` (`invalid-request`).

## Topology

```
intake -> design -> author -> validate -> simulate -> done
                       ^          |  ^        |
                       |     apply_fixes      |
                       |          v           v
                       +--- triage <- retry_gate -> failed
                            fallback
```

* `design`: invokes the generate service
  (`https://github.com/stateforward/floppy/services/generate`, a
  fresh invoke id in `gen_id` each time) with the `topology_plan`
  structured-output schema; its `<finalize>` takes the plan from
  `generate.result` (or the code from `generate.failed`), and
  `done.invoke` moves on to `author`, or to `retry_gate` without a plan,
  except that `unconfigured` (no `FLOPPY_BASE_URL` or no `FLOPPY_API_KEY`
  held) goes straight to `failed`, since no retry can supply a backend. A
  redesign sends the plan it replaces (`previous_plan`) with the
  diagnostics; a redesign whose generation failed keeps that plan for the
  next one.
* `author`: invokes the generate service the same way with the
  `scxml_draft` schema: the document (plain
  SCXML with inline `event:schema` on its input transitions), the input event
  schemas as JSON text, and one to eight test scripts (input events with
  delays, a settle time, the outputs expected in order, and whether the
  machine finishes). A repair sees the failed draft, its tests, and the
  diagnostics; `unconfigured` ends the build in `failed` as in `design`.
* `validate`: its schemas must parse; `scx:validate` with
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
* `done`: a final state whose `<donedata>` carries the result.
* `retry_gate`: every failure (generation, draft, validation, simulation,
  timeout) passes through it. It counts the failure and takes its
  fingerprint, which the datamodel keeps (`fingerprint`, and the previous
  failure's in `previous_fingerprint`), and a guarded transition
  (`fingerprint === previous_fingerprint`) ends the build in `failed` with
  the reason `no-progress` and the repeated diagnostics: the same failure
  twice in a row means the repair loop is making no progress. A request's
  optional `max_attempts` also ends it (`attempts-exhausted`). A fingerprint
  is the sorted, distinct list of each diagnostic's kind, code, and
  identity, taken from the canonical report rather than prose: a validator
  diagnostic's identity is its primary label (the report gives no element
  path, and byte spans shift between drafts), so two drafts fail the same
  way when they have the same set of codes with the same primary labels; a
  simulation diagnostic's identity is its message, which names the failing
  test, its expected and observed outputs, its expected and observed
  `done`, and its error, so the same test failing with the same mismatch is
  the same failure; any other diagnostic (a generation failure, a timeout, a
  draft schema that is not JSON) is identified by its message, which the
  builder writes. A primary label often says only what kind of thing is
  wrong (`SCXML005`'s is "unknown target identifier"), so a draft that
  replaces one unknown target with another counts as the same failure.
* `triage`: `decision:choice` (synchronous executable content) writes the
  state's enabled `event:schema` transitions as the canonical choices:
  `triage.repair` (guarded by `plan !== null`, so it is among the choices only
  when there is a plan to repair from), `triage.redesign`, and
  `triage.abandon`. The decision service
  (`https://github.com/stateforward/floppy/services/decision`),
  invoked with those choices and the failure state, picks one, validates its
  data against the choices' schema, and returns it.
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
the backend stops sending tokens), and no answer reaches anyone. `scx`
puts no time limit of its own on a service request: the invoking state's
`builder.timeout` is the bound. The simulation is bounded the same way:
`simulate-timeout` is a delayed send cancelled on exit that fires after
`max_tests * sim_test_cap_ms` plus 60 s, longer than the longest simulation
`validate` admits (8 tests of at most 120 s each).

## Drafts are unrestricted

The builder does not limit what it builds (decision D28): a draft may use
any SCXML, extension namespaces, `<invoke>` (SCXML children and services),
`<script>`, and `src`, `srcexpr`, and `typeexpr`. A draft runs as an
invoked child of the builder's session, so when it is simulated it can do
exactly what any document of the run can, bounded only by the run's own
consent and grants: the `env` scopes the builder's document names, the
origins the bound packages declare, and the `--dir` and `--wasi` grants
the run gives. A draft that invokes the `generate` service the run binds
reaches it during its simulation like the builder does.

## Bindings

Everything a run needs is in the document:

* Its `decision` namespace and its `<invoke type>`s (`generate`,
  `decision`) are tagless repository URIs
  (`https://github.com/stateforward/floppy/namespaces/<name>`,
  `.../services/<name>`), which name floppy's latest release: `scx`
  resolves them from that release, or, with `--offline --cache-dir
  target/components/package-cache`, from the dev cache
  `scripts/build-components.sh` publishes the locally built packages into
  (README, "Running a bot"). The consent listing shows the release tag and
  commit each one resolved to.
* Its `env` attributes scope the variables: `FLOPPY_API_KEY` and
  `FLOPPY_BASE_URL` go only to the generate service's `<invoke>`s
  (`env:apikey`, `env:baseurl`), and `TYPESAFE_API_KEY` and
  `TYPESAFE_BASE_URL` only to the decision service's. `scx run` lists each
  variable and the `<invoke>`s it goes to before running (names only).
* `scx:validate` is scxml.rs's `scx` namespace
  (`https://github.com/stateforward/scxml.rs/namespaces/scx`), which `scx`
  embeds, so it needs no package and no fetch, and it runs within the
  default extension call fuel.

Each base URL comes from the env file, or, with a bare `--env NAME`, from
the calling shell (`scx` takes a variable's value from `--env NAME=VALUE`,
else its `--env-file`, else, for a bare `--env NAME`, the shell). The
`generate` service has no default backend (its package declares no
`http-origins`): `FLOPPY_BASE_URL` (any `OpenAI`-compatible Chat Completions
base URL, for example the maintainer's direct vLLM on the DGX Sparks,
`http://192.168.2.89:8000/v1`) and `FLOPPY_API_KEY` are both required, and
without either the generate service sends nothing and answers
`generate.failed` (`unconfigured`), so the builder ends at once in `failed`
with the reason `unconfigured` and a diagnostic naming both variables. The
`decision` service without a `baseurl` value uses its package's first
declared `http-origins` entry, `https://api.typesafe.ai`. `--env-file`
repeats, so the keys may come from two files (`--env-file .env --env-file
<typesafe env file>`). Without `TYPESAFE_API_KEY` the decision service
answers `decision.failed` (`unconfigured`) and the builder triages with its
fallback. Drafts are
validated without packages, so a built machine uses no extension namespace
or service.

## Running

From the repository root, after `sh scripts/build-components.sh` and with
`scx` installed from the subtree (see the
[README](../../README.md#running-a-bot)):

```console
# The locally built packages (the dev cache under target/).
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button"}}' |
  scx run --offline --cache-dir target/components/package-cache --env-file .env bots/builder/builder.scx
# floppy's latest release, once one is published (phase 7).
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button"}}' |
  scx run --env-file .env bots/builder/builder.scx
```

Standard output carries one JSON record per line: `log` records with the
progress, then the `done` record whose `data` is the result, and the run
exits 0.

## Proof

`tests/tests/builder_bot.rs` runs the command above (on the logical clock,
except where noted) with the spec "a traffic light with a pedestrian button" against a fake
Chat Completions and `system_one` backend on 127.0.0.1: an invalid first
draft (`SCXML005`) goes back to `author` through `triage.repair`, the second
draft's only error is fixed by its machine-applicable fix, and the fixed
draft runs its two test scripts as two invoked children (distinct invoke
ids) before the builder ends in `done`; the repair request carries the failed draft and
its tests. Other cases cover a redesign that sends the plan it replaces,
a failed redesign keeping that plan, a design generation that fails at
once keeping its cause (`status`) in its `retry_gate` log and in the
`failed` history after a later failure ends the build, `triage.repair` not being offered
without a plan, a draft that invokes the run's `generate` service being
simulated through it, a candidate's
`sim.settle` and `sim.drive` being recorded as outputs, a cancelled
candidate's `builder.timeout` not ending the next simulation, a failed
invocation's `sim.drive` not driving the next test, the fallback triage
ending in `no-progress` when a repair fails with the same diagnostic again,
a different diagnostic each time repairing on until `done`, a request's
`max_attempts` ending the build (`attempts-exhausted`), an invalid request,
and, on the wall clock, a
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
successful build of the current builder is
[`examples/traffic_light_luna`](examples/traffic_light_luna) (2026-10-06,
`gpt-6-luna`; three attempts, two `triage.repair` decisions). [`examples/traffic_light_vllm`](examples/traffic_light_vllm)
is the README command above run on direct vLLM after the endpoint cutover
(2026-10-08, `Qwen/Qwen3.8-Flash-Next`, `.env` only, so triage used its
fallback; five attempts, one machine-applicable fix), kept as built; it
validates with 0 errors and 0 warnings.
[docs/live-proof.md](../../docs/live-proof.md) records that run and every
other live result.
