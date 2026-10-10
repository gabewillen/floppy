# Handoff: resuming the floppy / scxml.rs work

Read this first when picking the work up in a new session or account. It
records where things stand, how to work, and what is open. The detailed
history lives in [decisions.md](decisions.md) (numbered decisions),
[port-plan.md](port-plan.md) (mosfet mapping and phases), and
[live-proof.md](live-proof.md) (every live run). Where they disagree with
this file, the newer of decisions.md and this file wins.

Last updated: 2026-10-09.

## Current state (2026-10-09) — resume here

- **Layout (2026-10-10, D65):** every bot and ability is
  `<name>/<name>.scx` with its ECMAScript in `<name>/<name>.js` beside it
  (one root `<script src="<name>.js"/>`; conds and exprs call its
  functions). Abilities are `abilities/<ability>/<ability>.scx`, each with
  byte-identical twins of the schemas it references in
  `abilities/<ability>/schemas/` (canonical files stay in
  `abilities/schemas/`). Tests lay abilities out with
  `support::copy_ability`. `scx validate` reads each document's `.js`
  (scxml.rs `98fd120`, `Validator::with_script_resources`), so a cond or
  `<log expr>` calling its functions is judged by their bodies and every
  document validates with 0 errors, 0 warnings, and 0 notes.
- **floppy:** work continues on branch `claude/project-thread-brkerr`
  (pushed to the private `gabewillen/floppy`; `main` is behind it).
  Phase 4 is done through "Replace Script Validators With Event Schemas":
  no phase 4 document has a `<script>`, payload checks are `event:schema`
  only, conds only correlate, and `tests/tests/contracts.rs` enforces the
  structure rule (no `function`, cond/expr at most 160 characters, body
  twins byte-identical to their ability schemas, literal SCXML invoke ids
  and srcs). The first identity attempt is on branch `wip/identity-slice`
  (reference only). Phase 4's docs are closed: decisions D33 to D43,
  port-plan's [Phase 4 conventions](port-plan.md#phase-4-conventions), and
  the AGENTS.md layout and no-`<script>` rules.
- **scxml.rs:** `main` and `feat/invoke-services` at `c5da80c`
  (`session.accepts-event-data`: a send between sessions, and host input,
  is checked against the receiving document's `event:schema`s; refused
  host input is the `scx run` error record `machine-invalid-input`; a
  relative `srcexpr` resolves against the invoked document that evaluates
  it; a snapshot read in a compound state's entry content is valid).
  floppy's subtree is at `c5da80c`.
- **Plan:** [plans/phase4-bot-core.mdscript.md](plans/phase4-bot-core.mdscript.md).
  Next slice: "Add Identity Text Ability", then the phase 5 abilities, live
  examples, docs, and the phase-end catch-up (doc tests once, deferred live
  runs via CPA; `.env.cpa` is not present in cloud sessions).
- **Open for the user:** harden
  `scxml_conformance::RunReport::read_json` (`--baseline`) like the scx
  readers; the gateway report.

## Phase 3 (2026-10-10): memory service

- **Branch:** `claude/project-thread-r7141w` (from `claude/project-thread-brkerr`),
  pushed to `gabewillen/floppy`; the phase 4 branch merges its commits.
- **Done:** `services/sql` (one SQLite transaction per `<invoke>`, any
  schema) and `abilities/memory/memory.scx` (mosfet's Memory: migrates mosfet's
  schema and applies statements through the sql service), decisions D44 to
  D47. Reasoning, identity and conversation recall and remember through the
  memory ability, and the body passes its database `/memory/floppy.sqlite`
  (README: the run grants
  `--dir https://github.com/stateforward/floppy/services/sql=HOST::/memory`).
  Plan: [plans/phase3-memory.mdscript.md](plans/phase3-memory.mdscript.md).
- **Building:** the sql component is C (SQLite), so
  `scripts/build-components.sh` (and every test, which runs it) needs
  wasi-sdk 25 at `WASI_SDK_PATH` (it checks `$WASI_SDK_PATH/VERSION`).
- **Tests:** a run whose documents bind the sql service gets a fresh
  granted directory from `support::memory_grant()` unless it passes its
  own. Each `scx run` takes 15 to 25 s of CPU to start (component
  compilation, no cache), so the behavior tests' per-record bound is 300 s.
- **Next:** phase 5 (behaviors and routines, stored in memory), 6, 7.

## Repositories

| Repository | Path | Branch | Notes |
| --- | --- | --- | --- |
| floppy (this repo, formerly nemobot) | `~/VSCode/stateforward/floppy`, remote `github.com/gabewillen/floppy` (private) | `main` | bots, floppy's namespaces and services, docs; no runtime of its own (D14) |
| scxml.rs | `~/VSCode/stateforward/scxml.rs` | `main` | engine, `scx` CLI, hosts, validator, resolver |
| scxml.rs work branch | `~/VSCode/stateforward/scxml.rs-services` (git worktree) | `feat/invoke-services` | all scxml.rs changes are made here first |
| mosfet (source of the port) | `~/VSCode/stateforward/mosfet/mosfet.py` | — | Python runtime being ported; reference only |

`scxml.rs/` inside floppy is a **squashed git subtree** of scxml.rs.

## How to work

- **Merge policy.** Make scxml.rs changes on `feat/invoke-services` in the
  worktree and commit there. After each pass, fast-forward scxml.rs `main`:
  `git -C ~/VSCode/stateforward/scxml.rs merge --ff-only feat/invoke-services`.
  Then pull into floppy:
  `git subtree pull --prefix scxml.rs ~/VSCode/stateforward/scxml.rs feat/invoke-services --squash`.
  Never edit `scxml.rs/` inside floppy directly.
- **Private remotes only.** floppy pushes to the private
  `github.com/gabewillen/floppy` and scxml.rs to the private
  `github.com/stateforward/scxml.rs` (over HTTPS via `gh`; SSH signing is
  unavailable here). No public repos, PRs, or releases until the user
  decides to go public.
- **No invented names.** Every public name comes from the W3C SCXML 1.0
  spec, a standard (WASI 0.3, Component Model, wasmtime CLI, OpenAI API,
  rustc/clippy, OpenTelemetry), or an existing scxml.rs name. Ask the user
  before adding a genuinely new name.
- **SCXML first.** Follow the spec's defaults and mechanisms (invocation
  cancel, `<donedata>`, `<data>` + `<param>`, `<send delay>` timeouts)
  instead of adding host mechanisms. Async or I/O work is an invoked
  service; executable content is synchronous (D13).
- **Keep scxml.rs and floppy separate.** Generic SCXML tooling lives in
  scxml.rs; bot policy lives in floppy.
- **Hard cutovers.** 0.2.0 is unreleased: no aliases or fallbacks.
- Pinned `wasm-tools` 1.244.0 is installed globally in `~/.cargo/bin`.

## Running things

- Build floppy's components: `scripts/build-components.sh` (publishes a dev
  package cache under `target/components/package-cache`).
- Run the builder bot (dev, until floppy has a release):
  `scx run --offline --cache-dir target/components/package-cache --env-file .env --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" bots/builder/builder.scx`
  with a `builder.request` event on stdin (see `bots/builder/README.md`).
- Live tests: `FLOPPY_LIVE_ENV_FILE` (colon-separated env files) and
  `FLOPPY_LIVE_MODEL`.
- Model endpoint (decision D31): the `generate` service has no default
  backend; every run needs `FLOPPY_BASE_URL` and `FLOPPY_API_KEY`, and
  without either the builder ends at once in `failed` (`unconfigured`).
  floppy's gitignored `.env` sets them to the user's two DGX Sparks:
  - **direct vLLM:** `FLOPPY_BASE_URL=http://192.168.2.89:8000/v1`, model
    `Qwen/Qwen3.8-Flash-Next` (the builder's default), `FLOPPY_API_KEY` (a
    copy of the vLLM key, also kept as `VLLM_API_KEY`). Streams reasoning
    immediately.
  - the gateway in front of the Sparks is no longer used: it holds
    reasoning back, so long generations time out (D24, D25, live-proof.md);
    a report for its maintainer was drafted in the 2026-10-07 session.
  - decision service: TypeSafe (`TYPESAFE_API_KEY` in
    `~/VSCode/stateforward/mosfet/mosfet.py/.env`).
- `.env` files are gitignored; never print or commit their values.

## Where things stand (2026-10-08)

Done (scxml.rs `main` and `feat/invoke-services` at `c28799aa`, which
inlines the browser `validate` options type so no unapproved
`ValidateOptions` name is exported; floppy's subtree matches, squash
`c7d5ba5b`, pull `0b45e45b`):

- Invoked services on WASI 0.3 / Component Model async (one `invoke`
  export); SCXML §6.4 cancel closes connections; no host time limits on
  service I/O.
- `scx` CLI with embedded engine components, `.scx` documents, document
  env scopes, SCXML-default URI loading, optional `@tag` pins split at
  parse time and resolved once per run, GitHub resolver as a Wasm
  component.
- Validator: rustc-style fixes (closest-match, cross-element such as
  `sendid` → `id`), `package-validator` components (codes render as
  `<prefix>::<code>`), literal-payload `event:schema` checks, code families
  (SCXML = spec only, XML, XSD, COMP, PKG, ENV, EVENT, DATAMODEL, LINT,
  LIMIT).
- Browser parity slice 1 (validator and extension components in the browser).
- floppy: `generate` and `decision` services, `decision:choice`, the builder
  bot (unrestricted, D28; `no-progress` exit, D26; failure causes logged),
  generate's package validator.
- scxml.rs datamodel pass landed (`66f102b0` to `92742d65`, D30): data
  models as packages (`datamodel:src`, `datamodels/` repository URIs,
  example `datamodels/example`), foreign attributes and elements accepted
  (`xml:base` honored, unbound namespaces are `LINT022`), `DATAMODEL001`/
  `002` and `PKG005`, LIMIT size bounds only (`LIMIT007`-`009` renumbered
  to `LIMIT005`-`007`), and `deny.toml` allowing ISC and
  CDLA-Permissive-2.0. The flaky `repository_namespaces` test is fixed
  (`f4f14cc8`: each test writes its document in its own scratch directory).
- scxml.rs `EmbeddedDocument::event_schemas` (`73df0554`) and `EVENT011`
  (`674e2f6e`): event data judged across the invoke boundary; canonical
  components recorded (`20fe0722`, manifest `03bc0069`, IRP baselines
  unchanged). floppy is green on it with `<decision:choice>` (D32,
  `e7c0a870`); the `builder_bot` live test passed on the subtree at
  `c28799aa` through the CPA gateway (`gabe-codex-1/gpt-6-luna`, 86 s).
- scxml.rs runtime event contracts (`01b71ac5`, 2026-10-09; subtree pull
  `43489ead`): a `<send>` whose data its own literal `event:schema`
  rejects, or, for an event the session delivers to itself, a schema of a
  transition handling it rejects, is not dispatched, and such a `<raise>` is
  not raised; `error.execution` ends the block. Transitions still match by
  name, `<donedata>` is unchecked, and events between sessions
  (`#_parent`, `#_<invokeid>`) and host input are not checked yet (needs a
  receiving-session machine interface, a new scxml.rs name). The SCXML
  processor rejects a send whose stamped event is over the 96 KiB event
  bound in the sender (`bb917ad5`). Canonical components recorded
  (`d4db774c`, manifest `5902fddb`). floppy's phase 4 traces are unchanged.
- floppy is green on the datamodel pass (2026-10-08): `scripts/build-components.sh`, fmt,
  clippy `-D warnings`, all workspace tests, and the `builder_bot` live
  test on direct vLLM; no floppy code or test named a moved code.

- Model endpoint cutover (D31, floppy `39c3b5df`): `FLOPPY_BASE_URL` and
  `FLOPPY_API_KEY` with no default backend; full proof green, the
  `builder_bot` live test passed on direct vLLM, and the README run is saved
  as `bots/builder/examples/traffic_light_vllm/` (docs/live-proof.md).

- Phase 4 event contracts (floppy `cb4e47e0`): one JSON Schema file per
  event, body events in `bots/floppy/schemas/`, ability events in
  `abilities/schemas/`, mosfet's domain names in
  `abilities/schemas/mosfet-event-names.json`; documents reference them by
  `event:schema="schemas/<event>.json"`, never inline. A schema reference
  cannot contain `..`, so ability documents live flat as
  `abilities/<ability>.scx`. `tests/tests/contracts.rs` checks the schema
  set and every `.scx` under `abilities/` and `bots/floppy/`.

- Phase 4 body and thin cognition: `bots/floppy/floppy.scx` (mosfet's Bot
  lifecycle and turns as the root session: `inactive`, `live{activating,
  active{unfocused, focused, processing, cancelling_processing,
  returning}}`, `deactivating`, final sink `degraded`) invokes
  `abilities/cognition/cognition.scx` (one `generate` call per turn, answering
  `bot.ability.communication.respond`). Readiness is each ability's first
  `bot.ability.offers`; the body emits its events as `log` records labelled
  with the event name (it has no parent). Deferred stimuli wait in a data
  model queue and are taken by eventless transitions on return (no
  `#_internal` replay, so no `from` field). `bot.ability.cognition.input`
  carries a required `stimulus` envelope and `bot.activate` an optional
  `model` (`[floppy field]`s). Tests: `tests/tests/body.rs` (against
  `tests/fixtures/phase4/cognition_double.scx`) and the ignored
  `tests/tests/floppy_live.rs` `live_text_turn` (since replaced by
  `live_cascade`).

- Phase 4 cognition cascade (floppy `b8964ae9`): `abilities/cognition/cognition.scx`
  invokes per turn `abilities/autonomy/autonomy.scx` (always unhandled without
  memory), `abilities/intuition/intuition.scx` (`<decision:choice>` of ignore or
  the reasoning handoff, decided by the decision service on TypeSafe, with
  mosfet's ConfidenceTuner passed by `<param>` and returned in
  `<donedata>`), and `abilities/reasoning/reasoning.scx` (memory steps around one
  strict structured `generate` call whose schema is built from
  `<decision:choice>`: respond and ignore until the body assembles
  choices). Stages answer by `done.invoke.<stage>`; hop timers (30 s) ask
  the body for `bot.reboot`; a cancel acknowledges at once. A run needs
  `TYPESAFE_API_KEY` as well as `FLOPPY_BASE_URL`/`FLOPPY_API_KEY`.
  Tests: `tests/tests/cognition.rs` (fake decision + model backend) and
  the ignored `floppy_live.rs` `live_cascade` (passed on the CPA gateway,
  `gabe-codex-1/gpt-6-luna`, 17 s).

- Phase 4 choices and routing (floppy `a59eb060`, `8e108a78`): on each
  `processing` entry the body assembles the turn's choices (its own
  `bot.focus_device`, pinned to the turn's focus candidates, and
  `bot.clear_focus` while focused, read by a filtered `<decision:choice>`;
  plus each ability's latest offers tagged `target`), compacts them and
  holds them to 32 KiB (else the turn fails before the cognition), and
  sends them as `bot.ability.cognition.input.choices` (`[floppy field]`).
  Reasoning selects only from them (a `target` enum per event). A
  completed turn passes through `routing`, which sends each selection to
  `#_internal` or `#_<target>` and logs `floppy.route`/`floppy.route.dropped`;
  `floppy.routed` ends it. Focus is taken only as a routed selection. No
  ability offers `bot.ability.communication.respond` until communication
  lands, so the bot does not speak yet. Live: `live_choices` and
  `live_cascade` passed on the CPA gateway (`gabe-codex-1/gpt-6-luna`,
  22 s for both).

- Phase 4 communication, conversation and speaking (floppy `80f19f29`):
  the body invokes `abilities/communication/communication.scx` beside the cognition
  (both must offer before `bot.activated`). Communication holds
  `abilities/conversation/conversation.scx` and `abilities/speaking/speaking.scx` as invoked
  children, offers `bot.ability.communication.respond` and `.activate`,
  says a routed response through speaking (text mode, `<log
  label="floppy.say">`; the body logs `bot.ability.speaking.output`) within a
  6 s hop timer, records it as an outbound
  `bot.ability.conversation.append`, and defers input and responses while
  one is in flight; a waiting one passes the same checks when replayed
  (a blank waiting response fails and is not said). External `bot.ability.communication.input` goes
  through the body to the conversation (as mosfet's
  `bot.ability.conversation.routed_input`), whose text product (its whole
  history, as in mosfet) comes back as a `bot.ability.cognition.input`
  handoff the body grants a turn.
  Tests: `tests/tests/communication.rs` (8), the fake-backend end-to-end
  `a_reasoned_response_is_said_through_the_communication` in
  `cognition.rs`; live `live_chat` passed on the CPA gateway
  (`gabe-codex-1/gpt-6-luna`, 18 s, one turn). The 42 KiB `machine-platform`
  failure no longer occurs since choice assembly moved into namespace
  elements: it was ECMAScript data-model fuel exhaustion, which still ends a
  session (scxml.rs `docs/limitations.md`); offers over the 96 KiB event
  bound now raise `error.execution` in the sending ability (scxml.rs
  `bb917ad5`).

- Phase 4 event traces (floppy `b93dbfa3`): each of the 37 deterministic
  tests in `body.rs`, `cognition.rs` and `communication.rs` compares its
  whole run (every session's `log`, `done` and `error` records in order,
  then the fake backend's requests) with
  `tests/fixtures/phase4/traces/<test>.ndjson` byte for byte
  (`tests/tests/support/trace.rs`). The restructure slices must leave them
  unchanged; rewrite them with `FLOPPY_TRACE_BLESS=1` only for an intended
  change, and say so in the commit.

- `<decision:compact location bound overlocation>` (floppy `8f2b2616`,
  `008cd04a`): the shared offer compaction (mosfet
  `model_facing_event_json_schema` and the 32 KiB bound) in
  `namespaces/decision`. The cognition and communication offers use it;
  their `compactSchema`/`compactOffers` copies are gone, traces unchanged.
  Still scripted: the body's `compactSchema`/`compactChoices` (its
  over-bound turn failure keeps the list and logs its size, so it moves
  with `<decision:assemble>`) and reasoning's `dataSchema` (part of the
  strict selection schema, so it moves with `<decision:selection>`).

- `<decision:pin>`, `<decision:assemble>`, `<decision:route>`,
  `<decision:names>` (floppy `6093fef1`, `851a6399`): the body's choice
  assembly, focus grounding and selection routing are namespace elements;
  the body keeps only the visible `<foreach>`/`<send>` that delivers each
  placed selection. `<decision:route>` writes one ordered list of
  `{event, data, target}` or `{dropped}` entries (not separate routed and
  dropped lists) so the `floppy.route`/`floppy.route.dropped` log order stays
  byte-identical. The body's choice/route script functions are gone,
  traces unchanged. Still scripted in the body: validators,
  `attentionBias`, `envelope` (later slices).

- `<decision:selection location choices>`, `<decision:tuner location
  thresholdlocation observe>`: reasoning's strict dispatch schema (mosfet
  `_selection_item_branch`/`dispatch_tool`) and intuition's
  `ConfidenceTuner` (mosfet defaults as Rust constants; the threshold is
  written as a probability) are namespace elements; the stage results are
  plain `<assign>`s. Reasoning's `onChoices` cond is gone: the generate
  service's strict-schema check already fails an off-choices reply as
  `schema-mismatch`. Intuition and reasoning now script only `isObject`
  (validators go with the event-schema slice), traces unchanged.

- `namespaces/bot` (floppy `242fb57a`, `d5fffb8d`), a new floppy
  namespace (`https://github.com/stateforward/floppy/namespaces/bot`,
  component `floppy_bot`, imports `types`, `telemetry`,
  `data-model-host`): `<bot:attention location focuslocation stimulus
  devices focus sources>` (mosfet `attention_bias_for_turn`) and
  `<bot:stimulus location event data source target? id?>` (the cognition
  input's stimulus envelope, canonical JSON). The body's `attentionBias`
  and `envelope` functions and its processing-entry and deferral scripts
  are gone (deferral is `<bot:stimulus location="pending" ...>` plus
  `queue.concat([pending])`), traces unchanged. Still scripted in the
  body: validators only (`isObject`, `validX`, `forTurn`, `allReady`,
  `routed`, ...).

- `namespaces/conversation` (floppy `d8a0b49b`, `0f2db251`, `b81a5ab5`),
  a new floppy namespace
  (`https://github.com/stateforward/floppy/namespaces/conversation`,
  component `floppy_conversation`, imports `types`, `telemetry`,
  `data-model-host`): `<conversation:commit location keylocation?
  direction event message sequence provenance?>` (mosfet
  `_message_for_turn`, `_model_safe_content`, `_stable_identity_basis`,
  `_stable_context_ref`, `MessageProvenance`) builds one committed message
  and its transcript key; it never holds the history (the extension
  boundary's values are bounded at 64 KiB on wasm), so the conversation
  appends with `history.concat([message])` and
  `transcripts[participants]`. The communication builds its outbound record
  with it (the append's provenance now carries explicit `null` source,
  target, session_ref and turn_ref; the committed history is unchanged).
  `the_conversation_commits_each_message_into_its_history` pins the
  committed messages field by field (blessed on the old document). Both
  documents now script validators only (`isObject`, `identities`,
  `validX`, `isProduct`, ...).

Next: phase 4 (bot core and abilities), per
`~/.agents/projects/floppy/plans/20261008-phase4-bot-core.mdscript.md`.

## Port run decisions (user, 2026-10-08)

The remaining mosfet port runs as one ultracode run with these grants:

- **Commits:** commit locally; when the full proof is green, fast-forward
  scxml.rs `main` and subtree-pull into floppy. Never push, publish, or
  release.
- **Live proof:** may run unattended against the DGX Sparks (direct vLLM)
  and the TypeSafe decision service.
- **Names:** the no-invented-names rule applies to scxml.rs only. floppy
  names freely; new scxml.rs names are batched for the user's approval.
- **Order:** backlog order — phase 4 bot core and abilities, phase 3 memory
  on SQLite, phase 5 behaviors and routines, phase 6 speech and vision,
  phase 7 PGlite memory and refreshed defaults.
- **Speech and vision:** OpenAI-compatible audio and vision APIs (served
  from the Sparks); ElevenLabs is an optional backend.
- **Devices (phone, audio):** out of scope for now.
- **Phase 7:** PGlite yes; releases and `SHA256SUMS` wait for going public.
- **Endpoint:** no package default (`FLOPPY_BASE_URL` required, fail fast
  when unset; D31); the gitignored `.env` sets `FLOPPY_BASE_URL` to direct
  vLLM with provider-neutral `FLOPPY_API_KEY`.
- **Builder:** save the vLLM traffic-light run as
  `bots/builder/examples/traffic_light_vllm/`; drafts reference schema
  files instead of inline JSON.
- **Browser slice 2:** after the port.
- **Almost everything is `.scx`:** bots, abilities, behaviors, routines, and
  policy are SCXML documents in `.scx` files; Rust/Wasm components only for
  invoked services doing work SCXML cannot express (async I/O).
- **Phase 4 answers:** bot `bots/floppy/`, abilities in top-level
  `abilities/`, no default model URL (`FLOPPY_BASE_URL` required), intuition
  on the decision service, `turn` field and media by reference in payloads,
  `<decision:choice events>` filter (renamed from `<decision:menu>`, D32),
  telemetry one-trace proof deferred,
  scxml.rs `EmbeddedDocument::event_schemas` and `EVENT011` approved. Full
  list in `~/.agents/projects/floppy/plans/20261008-phase4-bot-core.mdscript.md`.
- **scxml.rs names approved:** the datamodel pass's names, with the example
  package renamed to `datamodels/example`; `deny.toml` allows ISC and
  CDLA-Permissive-2.0 (TLS stack).
- **Live model while the Sparks are saturated:** CPA gateway via the
  gitignored `.env.cpa`, model `gabe-codex-1/gpt-6-luna` (fallback
  `gabe-xai/grok-4.6`).
- **Behaviors and routines (phase 5) use the builder:** mosfet has the model
  author behaviors and routines as Starlark HSM programs and compiles them at
  runtime (`mosfet/behavior/compiler.py`). floppy mirrors that with
  `bots/builder`: a behavior or routine is an SCXML document the builder
  authors, validates, and hands back, and the bot then invokes it. No
  Starlark port.
- **Delegated decisions (2026-10-09):** the user delegated all remaining
  decisions overnight. Agents choose by SCXML 1.0 and existing patterns and
  record each choice (and any new scxml.rs name) under "Decisions Made Under
  Delegation" in `~/.agents/projects/floppy/plans/20261008-phase4-bot-core.mdscript.md`
  for review. Still no push, publish, or release.
- **Pacing:** run through all phases, report after each; stop only for a
  real blocker or scxml.rs names needing approval.

## Open decisions (waiting on the user)

1. Send the gateway report (reasoning buffering, keep-alives, abort on disconnect).

## Backlog (agreed, not started)

- Browser parity slice 2 (services, resolution, WASI shims, element trace).
- `scx compile --target native`: one self-contained executable per platform;
  top-level `<data>` ids are the arguments, `<donedata>` JSON on stdout,
  `<log>` on stderr, exit code from the final state.
- Exporter components for session outputs (`<log>`, `<donedata>`, trace):
  stdout default, file/WebSocket/OTLP packages. Low priority.
- Canonical (Docker Linux) builds of floppy's components so pinned digests
  reproduce (D27).
- Rest of the mosfet port: bot core and abilities (phase 4), behaviors and
  routines (5), memory service SQLite then PGlite (3), speech/vision/devices
  as services (6).
- Website (after the port): mosfet's animated robot (`mosfet/web/src/face`),
  web components + hsm.ts, marketplace of packages, a Mindstorms-style
  workshop whose pieces are inferred from element XSDs, live highlighting
  of running bots.
- Going public: push, CI, releases with `SHA256SUMS`, per-platform `scx`
  binaries, IRP identity regeneration on Linux CI.

## Known stale docs

- The "Where things stand (2026-10-08)" section above is history; the
  current state is at the top of this file.
