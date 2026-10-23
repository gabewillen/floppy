# builder

The builder bot builds SCXML documents (never Rust). It is itself an SCXML
machine, [`builder.scx`](builder.scx) (decision D9 in
[../../docs/decisions.md](../../docs/decisions.md)). Run it directly, with the
request on standard input:

```console
# The locally built packages (the dev cache under target/).
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button"}}' |
  scx run --offline --cache-dir target/components/package-cache --env-file .env --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" --dir "https://github.com/stateforward/floppy/services/files=$PWD/behaviors::/behaviors" bots/builder/builder.scx
# floppy's latest release, once one is published (phase 7).
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button"}}' |
  scx run --env-file .env --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" --dir "https://github.com/stateforward/floppy/services/files=$PWD/behaviors::/behaviors" bots/builder/builder.scx
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
| `builder.request` | an event to the builder (standard input when run directly) | `{spec, name?, constraints?, context?, max_attempts?, model?, generate_timeout_ms?, generate_ceiling_ms?, min_every_ms?, turn_after_bound_ms?, apply?, document_max_completion_tokens?}`: the specification, the name the draft's documents are written under (default the plan's; each run of characters other than letters, digits, `_` and `-` becomes `_`; reflection sends the behavior's), extra constraints (strings), the data the specification refers to (an object, which the design and author prompts carry beside it; reflection sends the change input, decision D201), the failure cap (1 to 8, default 8, decision D133), the generate service's model (default `Qwen/Qwen3.8-Flash-Next`, a reasoning model), the time each streamed generation may go without progress, and each generation may take in all, before `builder.timeout` cancels it (1 ms to 1 h each, defaults 30000 and 1200000), and the lifetime bounds (1 ms to 1 day each, no default: without them neither the lifetime check nor the forced tick runs; reflection sends 60000 and 1000, `ROUTINE_MIN_EVERY` and `TURN_SILENCE_BOUND`, decision D84), and the live turn a turn behavior is run on before it is built (`{turn, input, targets, choices?, handled?}`: the turn, the behavior input as autonomy builds it, the sessions that turn's output names, at most 32, and the turn's choices `[{event, schema, target?}]`, which each selection must be one of with data its schema admits, as the dispatch is checked, decision D206, and `handled`, false for a turn that taught the behavior, whose run may leave it unhandled, decision D207; no default: without it a turn behavior runs only its own tests; reflection sends its turn's, decision D131), and the author call's first token bound (1024 to 65536, default 16384; a truncated author generation doubles it for the next author call, up to 65536, decision D135) |
| final state `done` | its `<donedata>` ([`schemas/done.json`](schemas/done.json)) | `{scxml, schemas, script, path, tests, report}`: the built document's three parts (its SCXML, its input event schemas as one object with `$defs` keyed by event, and its ECMAScript, `""` for none), the path of the `.scx` they were written as (`../../behaviors/<name>.scx`), its test scripts, and the report (attempts, failures, fixes applied, warnings, the plan, the triage decisions, the failure history, the simulation results) |
| final state `failed` | its `<donedata>` ([`schemas/failed.json`](schemas/failed.json)) | `{reason, diagnostics, history, scxml?, path?}`: `invalid-request`, `unconfigured` (the run holds no `FLOPPY_BASE_URL`; the first generation ends the build at once), `attempts-exhausted` (the failure cap, decision D133), or `abandoned`, with the last diagnostics (`{kind, code, message}`), the failure history (every failure's attempt and diagnostics, as in `done`'s report), and `scxml`, the last draft a check (`validate`, the lifetime check, `simulate`) rejected, with `path`, where it was written, both absent when no draft was rejected (the reflection stores the rejected source as a draft) |

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

`builder.scx` itself carries the contracts as inline `event:schema` attributes
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

## Three documents

A draft is three documents beside each other, so the model never escapes
one inside another (decision D121): the `author` tool call returns
`scxml` (structure only), `schemas` (a JSON object, `$defs` keyed by input
event) and `script` (plain ECMAScript, or empty), and `writing` writes
them as `<name>.scx`, `<name>.json` and `<name>.js` through the files
service ([`services/files`](../../services/files/README.md)) into the
directory the run grants it, `behaviors/` beside `bots/` and `abilities/`
(`--dir "https://github.com/stateforward/floppy/services/files=$PWD/behaviors::/behaviors"`;
create `behaviors/` first; it is ignored by git). The chart references
them by relative URI: `event:schema="<name>.json#/$defs/<event>"` on the
transitions that take an input event and, when there is a script, one
root `<script src="<name>.js"/>`. The simulation, and every later invoker
of a learned behavior, loads it by `<invoke srcexpr>` on its path,
`../../behaviors/<name>.scx`, which resolves the same from this document
and from every document that invokes a learned behavior (each two levels
below the layout root); scxml.rs loads its schema file and script against
its own base URI, as they are when it is invoked.

## Topology

```
intake -> design -> author -> writing -> validate -> simulate -> done
                       ^          ^          |           |
                       |          +-- apply_fixes        |
                       |                     v           v
                       +--------- triage <- retry_gate -> failed
                                  fallback
```

Each generation's prompt is its request as model-facing XML
(`<cognition:model_facing_data>`, decision D201): a `<request task>`
whose fields are the spec, the context, the constraints and what the stage
carries, a document or script as the text it is written in, never JSON
text.

* `design`: invokes the generate service
  (`https://github.com/stateforward/floppy/services/generate`, a
  fresh invoke id in `gen_id` each time) with the `topology_plan`
  structured-output schema; its `<finalize>` takes the plan from
  `generate.result` (or the code from `generate.failed`), and
  `done.invoke` moves on to `author`, or to `retry_gate` without a plan,
  except that `unconfigured` (no `FLOPPY_BASE_URL` held) goes straight to
  `failed`, since no retry can supply a backend. A
  redesign sends the plan it replaces (`previous_plan`) with the
  diagnostics; a redesign whose generation failed keeps that plan for the
  next one.
* `author`: invokes the generate service the same way with the
  `scxml_draft` schema: the three documents (above), told the `name` they
  reference, and one to eight test scripts (input events with
  delays, a settle time, the outputs expected in order, and whether the
  machine finishes). Its system prompt is the author rules followed by
  the W3C SCXML 1.0 XML Schema (`scxml_xsd`, loaded by `<data src>` from
  [`xsd/scxml-1.0.json`](xsd/scxml-1.0.json): the scxml.rs subtree's
  schema modules without comments or annotations, decision D134), so the
  model writes against the schema the validator holds it to, with no
  template or example document. A repair sees the failed draft, its
  schemas, its script, its tests, the diagnostics, and which of them an
  earlier draft already failed with (`repeated`, decision D133);
  `unconfigured` ends the build in
  `failed` as in `design`. A draft whose `scxml` holds no `<scxml`
  element (empty, or a placeholder a model's stub call holds) is a `draft`
  diagnostic, `no-document`, quoting what it held, before anything is
  written (decision D123). The author's rules keep every `cond` and `expr`
  beyond a literal, an identifier or a dotted path a call of a function of
  `script` (as decision D65 keeps floppy's own documents), so no attribute
  holds `&`, `<` or `>`; they say the constraints win where they differ
  from the builder's rules (a turn behavior sends nothing to its invoker,
  so its tests expect no outputs), and ask for exactly one call of
  `scxml_draft`.
* `writing`: three invocations of the files service write the three
  documents; one that fails (no grant, a name the service refuses) is a
  `draft` diagnostic, `write-failed`.
* `validate`: `scx:validate` with
  `applyfixes="machine-applicable"`, waiving `EVENT004` (it is given no
  schema file: `<scx:eventschema uri>` names a literal file, and the name
  is the draft's; the simulation's compile reads the file); when the fixes
  change the text, `apply_fixes` takes them and writes and validates again
  (at most 2 passes per draft);
  a clean draft whose `script` is not empty but whose document has no
  `<script src="NAME.js"/>` (`script-not-loaded`), or whose `cond` and
  `*expr` attributes call a bare function name `script` does not declare
  and that is neither `In` nor an ECMAScript global (`undeclared-function`,
  naming them), is a `draft` diagnostic before it is simulated (decision
  D123): such a cond throws, and on an eventless transition scxml.rs
  retries it every microstep until the whole run fails, not only the
  simulated child;
  otherwise the error diagnostics feed back to `author`, each with
  `line` (the trimmed source line its primary span starts on) and
  `rendered`, the form `scx validate` prints it in: `error[CODE]: message`,
  `--> NAME.scx:line:column`, that line (at most 160 characters around
  the span) with the span marked by `^` and its label, then its notes,
  help and fixes (decision D123). A simulate diagnostic whose `done`
  differs from the test's says what that means (the machine reached no
  top-level final state, invoked with no param and sent only the test's
  events, or reached one the test did not expect); only the `live turn`
  run below is invoked with params. With the
  request's `min_every_ms` and `turn_after_bound_ms`, a clean draft is then
  checked against the lifetime bounds (`<behavior:lifetime_errors>`,
  diagnostics of kind `lifetime`, code `E0009`): a
  draft that sends `bot.behavior.tick` is a persistent routine (decision
  D61), and a delayed send of an event that a transition sending the tick
  takes is one of its schedules (`every`, `at`). A schedule's literal
  `delay` shorter than `min_every_ms` is rejected, a schedule with a
  literal `target` is rejected (it is sent elsewhere and never fires,
  decision D147), a persistent routine without a schedule is rejected, and in a turn behavior (any other draft)
  a literal `delay` longer than `turn_after_bound_ms` is rejected, since
  autonomy silences a turn behavior then. A `delayexpr` is not judged.
* `simulate`: each test script runs the draft as a fresh invoked child
  (`<invoke idlocation="candidate_id" type="scxml" srcexpr="draftPath()"/>`,
  loaded from its path with its schema file and script,
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
  failed to start) is dropped. With the lifetime bounds, a persistent
  routine also runs one more test, `first tick` (the forced tick, which
  stands in for waiting on the schedule's clock): a fresh child runs the
  draft's forced tick copy, `<name>.tick.scx` beside it (each schedule's
  `delay` or `delayexpr` replaced by a `delay` of 1500 ms, from
  `<behavior:lifetime_errors ticklocation>`), and is sent nothing, so its
  own schedule send must arm and fire (decision D153: a schedule that
  never arms in the room fails here, not only live), and within 2 s (the
  verify timeout) the child must send `bot.behavior.tick` whose data carries
  `name`, `trigger` (`every` or `at`), `scheduled_at` and `fired_at`, and
  whose `output` is a non-empty list of selections (objects with an
  `event`) whose data reads the tick's own values; otherwise the
  diagnostics are `<behavior:verify_tick>`'s (kind `apply`, code `E0008`). So a routine whose
  schedule is minutes or days away is verified in the builder's own time,
  within the 2 s verify timeout (decision D131), while revising is bounded only
  by inactivity (`revising_timeout_ms`) and its ceiling
  (`revising_ceiling_ms`, decision D101). With the request's `apply`, a
  turn behavior (any draft that does not send `bot.behavior.tick`) also
  runs one more test, `live turn` (the live-turn check of a turn
  behavior, decision D131), in its own state
  `sim_apply`: a fresh child invoked as the candidates runner invokes it,
  with the params `turn` and `input` of that live turn, is sent nothing and
  must reach a top-level final state within min(`turn_after_bound_ms`,
  2000) ms, armed once it has started; `<behavior:verify_apply>` then checks
  its donedata: `{turn, handled, selected}` with the live turn, `handled`
  true and at least one selection, each with an `event`, a `target` among
  `apply.targets` and data whose members agree with the string, number and
  boolean members of `input` of the same names; otherwise the diagnostics
  are kind `apply`, code `E0008`, as for the forced tick.
* `done`: a final state whose `<donedata>` carries the result.
* `retry_gate`: every failure (generation, draft, validation, simulation,
  timeout) passes through it. It counts the failure and takes its
  fingerprint, the sorted, distinct identities of its diagnostics, which
  the datamodel keeps (`fingerprint`, and those of every earlier failure
  of the build in `fingerprints`), and the failure's diagnostics an earlier failure already had,
  consecutive or not, go to the next author call as `repeated`, which
  the author rules say to fix another way. The build ends in `failed`
  with `attempts-exhausted` only at `max_attempts` failures, 8 unless the
  request sets it (decision D133, which replaces D124's end at the first
  repeat: the maintainer chose to retry to a cap). A
  diagnostic's identity is its kind and code and, taken from the canonical
  report rather than prose, for a validator diagnostic its primary label
  (not the source line it marks: the same mistake moves around between
  drafts, so the same code and label on another line is the same failure),
  and for any other its message: a simulation diagnostic's names the
  failing test, its expected and observed outputs, its expected and
  observed `done`, and its error without the candidate's generated invoke
  id (which is new on every run), so the same test failing with the same
  mismatch is the same failure and two different test failures are not;
  any other diagnostic (a generation failure, a timeout, a write failure,
  a draft check, `E0009`, `E0008`) is identified by the message the
  builder or its namespace writes.
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
`generate_ceiling_ms` in all (default 1200 s) and, once their reply streams
(the run's `FLOPPY_STREAM` is `true`), after `generate_timeout_ms` without
progress (default 30 s; the first `generate.progress` arms it, each later
one re-arms it), the request may set either; each reply is bounded by
`max_completion_tokens` (8192, reasoning included; a truncated design
generation doubles it for the next design call, up to 65536, decision D208), but the author reply (a
whole document) by `document_max_completion_tokens` (16384: the 2026-10-21
runs truncated whole documents at 8192; decision D131); and `triage` arms it
after `decision_timeout_ms` (60 s), tagged with the stage run
(`stage_run`) and cancelled on exit (decision D101). Invoked with the
`<param>` `progress_target` `'#_parent'` (reflection does), the builder
sends its invoker `builder.progress {stage}` as it enters `design`,
`author`, `validate`, each simulation run and `triage`, and passes on each
`generate.progress {received}` of a streamed generation, so the invoker can
bound a streamed generation by inactivity. A timeout that fires while the call is
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

* Its `cognition` and `decision` namespaces and its `<invoke type>`s (`generate`,
  `decision`, `files`) are tagless repository URIs
  (`https://github.com/stateforward/floppy/namespaces/<name>`,
  `.../services/<name>`), which name floppy's latest release: `scx`
  resolves them from that release, or, with `--offline --cache-dir
  target/components/package-cache`, from the dev cache
  `scripts/build-components.sh` publishes the locally built packages into
  (README, "Quick start"). The consent listing shows the release tag and
  commit each one resolved to.
* Its `env` attributes scope the variables: `FLOPPY_API_KEY` and
  `FLOPPY_BASE_URL` go only to the generate service's `<invoke>`s
  (`env:apikey`, `env:baseurl`), and `TYPESAFE_API_KEY` and
  `TYPESAFE_BASE_URL` only to the decision service's. `scx run` lists each
  variable and the `<invoke>`s it goes to before running (names only).
* The `generate` service imports `wasi:filesystem` (it reads the images a
  message names, `services/generate/README.md`), so the run grants it a
  media directory, `--dir
  "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media"`
  (create `media/` first; it is ignored by git): `scx run` refuses a run
  that binds the service without it. The builder sends no images.
* The `files` service imports `wasi:filesystem` too, so the run grants it
  the behaviors directory,
  `--dir "https://github.com/stateforward/floppy/services/files=$PWD/behaviors::/behaviors"`
  (above).
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
`http://192.168.2.89:8000/v1`) is required, and without it the generate
service sends nothing and answers `generate.failed` (`unconfigured`), so the
builder ends at once in `failed` with the reason `unconfigured` and a
diagnostic naming it. `FLOPPY_API_KEY` is needed only by a keyed backend: a
run that holds no key reaches a keyless backend with no key header (decision
D127). The
`decision` service without a `baseurl` value uses its package's first
declared `http-origins` entry, `https://api.typesafe.ai`. `--env-file`
repeats, so the keys may come from two files (`--env-file .env --env-file
<typesafe env file>`). Without `TYPESAFE_API_KEY` the decision service
answers `decision.failed` (`unconfigured`) and the builder triages with its
fallback. [`.env.example`](../../.env.example) names the variables, and
[docs/defaults.md](../../docs/defaults.md) lists the model, the timeouts
and the bounds the builder ships with. Drafts are
validated without packages, so a built machine uses no extension namespace
or service.

## Running

From the repository root, after `sh scripts/build-components.sh` and with
`scx` installed from the subtree (see the
[docs/running.md](../../docs/running.md#build)):

```console
# The locally built packages (the dev cache under target/).
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button"}}' |
  scx run --offline --cache-dir target/components/package-cache --env-file .env --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" --dir "https://github.com/stateforward/floppy/services/files=$PWD/behaviors::/behaviors" bots/builder/builder.scx
# floppy's latest release, once one is published (phase 7).
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button"}}' |
  scx run --env-file .env --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" --dir "https://github.com/stateforward/floppy/services/files=$PWD/behaviors::/behaviors" bots/builder/builder.scx
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
ids), loaded from the path it was written to, before the builder ends in
`done`; the repair request carries the failed draft, its schemas, its
script and its tests. Each case runs the builder in a scratch layout of
its own (`bots/builder/` and `behaviors/`), so the documents it writes are
never shared; one case checks the three files written under the
request's name, a draft with a script. Other cases cover a redesign that sends the plan it replaces,
a failed redesign keeping that plan, a design generation that fails at
once keeping its cause (`status`) in its `retry_gate` log and in the
`failed` history after a later failure ends the build, `triage.repair` not being offered
without a plan, a draft that invokes the run's `generate` service being
simulated through it, a candidate's
`sim.settle` and `sim.drive` being recorded as outputs, a cancelled
candidate's `builder.timeout` not ending the next simulation, a failed
invocation's `sim.drive` not driving the next test, the fallback triage
repairing on to the cap when a repair fails with the same diagnostic again,
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
