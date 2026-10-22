# Handoff: resuming the floppy / scxml.rs work

Read this first when picking the work up in a new session or account. It
records where things stand, how to work, and what is open. The detailed
history lives in [decisions.md](decisions.md) (numbered decisions),
[port-plan.md](port-plan.md) (the original mapping and phases), and
[live-proof.md](live-proof.md) (every live run). Where they disagree with
this file, the newer of decisions.md and this file wins.

Last updated: 2026-10-22.

## Current state (2026-10-09) — resume here

- **Learned behaviors are repaired, run on their live turn and built
  from a template (2026-10-22, D130 to D132):** reflection repairs a
  select call the generate service refuses (D126's `rejection`) at most
  twice with the original's feedback, failing at once on the same
  rejection and then with `Selection rejected: <message>` (D130). The
  builder runs a turn behavior once on the live turn it is learned from
  (`builder.request`'s `apply`, which reflection always sends) and
  checks the run with `<behavior:verify_apply>` (D131); a routine keeps
  its forced tick. A turn behavior is built from the fixed template
  `namespaces/behavior/templates/turn.scx` (`template` `'turn'`,
  `<behavior:turn_program>`), so the model writes only its script,
  `applies(input)` and `selections(input, turn)`, with at most seven test
  inputs; a script whose `lifetime` is persistent falls back once to a
  whole document, and a whole document's author call is bounded by
  `document_max_completion_tokens` (16384) (D132). The build-time half
  of "nothing tests learned behaviors on real input" is done by D131;
  the runtime half is pending (gap L5).
- **Seeing (2026-10-22, D185 to D189):** `abilities/seeing/seeing.scx`
  is the original's eye: it holds the newest `environment.visual` of each
  `source` (a new [floppy field]; the schema moved to `abilities/schemas/`),
  measures change through the new `vision` service (`byte_identity`,
  `frame_diff`), looks when the model selects `bot.ability.seeing.look`, and
  hands each sight to the body as `bot.ability.cognition.input`; object
  detection and description are its per-view stage children over generate.
  The body invokes it always, and its `seeing_enabled` switch (off by
  default) decides whether it is expected, fed and offered; every bot run
  now grants the vision service a media directory. Faces wait on decision 6.
  `frame_diff` decodes as the original's OpenCV 5.0.0 build does (its own
  JPEG DC decoder, libpng's gray and gamma, `image-webp`, Exif
  orientation), checked against OpenCV fixtures in
  `services/vision/src/testdata/`; Cargo.lock carries `miniz_oxide` 0.8.9
  for `png` beside scxml.rs's 0.9 (D185). A document IRP case may now
  require its run's logs (`logs=` in cases.py, `logs.json`).
- **Body and room parity (2026-10-22, D155 to D160):** `bot.clear_focus`
  is gone (focus stays until the bot focuses another device; the reflex
  floor keeps it). A busy body holds what arrives (`floppy.held`), and the
  next turn takes at most 8 percepts, 64 KiB and 32 participants: one
  percept as itself, more as one `bot.perceived {percepts}`; what is still
  held when the bot leaves active is released (`floppy.released`), never
  replayed. `bot.input` carries `participants`, and every schema inlines
  the original's one `Participant` (`contracts.rs` checks they are equal;
  scxml.rs refuses a cross-file `$ref`). The room supervises each bot in
  its own `supervising.<bot>` region with the original's restart budget
  and backoff, and a device may sit on an object (`on`, rendered on its
  `<device>`). The body starts a firmware after its abilities
  (`firmware_src`, default `bots/floppy/firmware/firmware.scx`, which offers
  nothing); an owner's firmware is offered, read each turn and rendered as
  `<firmware ref id state>`. A test that lays the body out must copy
  `firmware/` beside it, as `body.rs` does.
- **One schema file per event (2026-10-21, D125):** scxml.rs
  `5bc75e72` resolves an `event:schema` reference against its
  document's base URI like any relative URI, `..` included, so the twin
  directories are gone: abilities reference `../schemas/<event>.json`, the
  body and the room `../../abilities/schemas/<event>.json` (and the room
  `../../bots/floppy/schemas/<event>.json`); 240 schema files became 109.
  `contracts.rs` checks every reference resolves to its event's one file
  and no schema file name occurs twice. The builder references its
  `schemas/*.json` instead of inlining them. A test that lays documents
  out in a scratch directory must copy `abilities/schemas/` (and, for the
  body's events, `bots/floppy/schemas/`) beside them, as
  `support::copy_ability` and `support::environment_with_devices` do.
- **The same failure more than once ends the build (2026-10-20, D124):**
  the maintainer's rule, "if it gets the same failure more than once it
  should end". `retry_gate` ends with `no-progress` as soon as any
  diagnostic of a failure is one an earlier failure of the same build had,
  consecutive or not: `fingerprints` (every identity seen in the build)
  replaces `previous_fingerprint`. A validate diagnostic's identity is
  kind|code plus its label (no line: the same mistake moves around), any
  other's kind|code plus its message; a `test-failed` message now names its
  error without the candidate's generated invoke id, which made every
  `error.communication` failure look new. `max_attempts` stays. This
  catches the live run 2's `E0009`, repeated four times among other
  diagnostics. Live (2026-10-21, docs/live-proof.md), on the maintainer's
  lab (mosfet's stack: `gemma-4-26b-a4b` on the vLLM at
  `192.168.2.2:8100`, the systemone decision server at
  `192.168.2.2:8102`): `live_cascade` passes; `live_conversation` failed
  both runs with no run ended early and no behavior learned, every
  revision ended `no-progress` by D124 on a repeated diagnostic (most
  non-consecutive; repeats of `XML001`, `<assign>` misuse and the
  8192-token `truncated`), and the decision server chose the ignore on
  turn 3's "ping". The lab's model and decision servers match floppy's
  wire formats; its speech and voice servers do not (speech-to-text and
  text-to-speech on two origins, voice not LocalAI's API), so speech stayed
  on Gemini and voice was unconfigured.
- **A cond that always throws ends only its session (2026-10-20,
  scxml.rs `cf8f293a`):** a transition condition that fails on every
  evaluation now costs one command per failure and stops at the per-call
  command bound instead of running out of fuel; the root then fails with
  `command-bound`, and an invoked session is ended as D122 ends a runaway
  (`error.execution` for a start, `error.communication.<invokeid>` later).
  This resolves D123's open eventless-cond failure (`live_conversation`
  run 1's `machine session failed: protocol`): a simulated draft whose
  eventless cond throws fails its test with `error error.execution` and is
  repaired, which `tests/tests/builder_bot.rs`'s `/throwing` route proves
  with no builder change. Live (2026-10-20, docs/live-proof.md):
  `live_cascade` passes; `live_conversation` failed both runs with no run
  ended early (run 1: reflection selected nothing on any turn, no drafts;
  run 2: `PingPongResponder` was abandoned on turns 2 and 3, on `E0009`
  lifetime and `no-document`, and accepted only on turn 4, after the
  turn-2 deadline; reasoning chose the ignore on turn 3's "ping").
- **Builder diagnostics a repair can act on; `live_conversation`
  passes (2026-10-20, D123):** each validate diagnostic carries `line`
  and `rendered` (`scx validate`'s form: `--> NAME.scx:line:column`, the
  marked line, notes, help, fixes) and was fingerprinted by label and
  line (D124 drops the line);
  the author rules say the constraints win (a turn behavior's tests expect
  no outputs), keep every non-trivial `cond`/`expr` a call of `script`
  (no `&` in XML), need a top-level `final`, and ask for one call; a draft
  with no document (`no-document`, a model's stub call), a script not
  loaded (`script-not-loaded`) or a call no script declares
  (`undeclared-function`) is rejected before its simulation. Live:
  `live_conversation` passes (`GabePingPongRoutine` learned on turn 2,
  answered turn 3). An eventless transition whose cond
  throws ended the whole run (`host-failure` or `machine session failed:
  protocol`), an scxml.rs bug resolved upstream by `cf8f293a` (above).
  Open (D123): the model still sends stub calls and writes tests expecting
  outputs now and then; `parallel_tool_calls` and `minLength` were not
  used (D123 says why).
- **A runaway invoked session ends only itself (2026-10-20, D122):**
  scxml.rs `f920cb47` (now under `cf8f293a`): an invoked session whose call stops at the
  per-call command bound is ended as a cancelled invocation is (a start
  that stopped there fails its `<invoke>` with `error.execution`, a later
  call queues `error.communication.<invokeid>`); only the root's own stop
  still fails the run with `command-bound`. A builder draft whose
  simulation runs away (the 2026-10-19 `live_conversation` run 2) now
  fails its test (`test-failed`, `error error.execution` or
  `error error.communication.<invokeid>`) and is repaired; the builder
  needed no change (`sim_run`'s `error.*` transition already took both),
  and `tests/tests/builder_bot.rs` proves both cases. Live (2026-10-20,
  docs/live-proof.md): `live_cascade` passes; `live_conversation` still
  learns no behavior (its drafts failed `XML001` in the chart and other
  validation codes; none ran away, so D122 was not reached live).
- **Multi-document learned behaviors (2026-10-18, D121):** the builder's
  author tool call returns `scxml`, `schemas` (`$defs` by input event) and
  `script`, and the builder writes them as `<name>.scx`, `<name>.json` and
  `<name>.js` through the new `services/files` (scxml.rs's files example's
  `name`/`text`/`seed`) into the run's `behaviors/` grant
  (`--dir https://github.com/stateforward/floppy/services/files=HOST::/behaviors`;
  every bot and builder run needs it now, and the test harness grants the
  layout's `behaviors/`). The chart references `<name>.json#/$defs/<event>`
  and one root `<script src="<name>.js"/>`; it is validated with
  `EVENT004` waived and simulated by `srcexpr` on
  `../../behaviors/<name>.scx`. A behavior row's `source` is that path;
  reflection reads a stored program back with `seed` for a change; the
  candidates and routines runners invoke by `srcexpr`. scxml.rs
  `c8bb69a8` (a `srcexpr` child's schema files and scripts load against
  its own base URI, fetched each time it is invoked) came with it. Open: D121's "To
  review" list under "Open decisions" below.
- **Parity gap 13's small part (2026-10-18,
  [parity-gaps](plans/parity-gaps.md)):** `<conversation:ingest_content>`
  no longer takes an image stimulus as `image/*` content, as the original's
  `0d0f5e2` deletes that branch; until the turn attachments are ported (the
  rest of gap 13), an image adds nothing to a turn, and a turn of images
  alone fails as empty. The conversation namespace IRP was re-prepared and
  its baseline re-measured on macOS arm64.
- **Q8: the decision service's kinds are OpenAI Decisions API's question
  types, with J16 and J17 (2026-10-18, D120,
  [plan](plans/original-domains.md#to-review-q8)):** `choice` (the default;
  was `choose`, and `classify` when given `labels`), `predicate` (was
  `select`, its behavior unchanged: each event's confidence is still its
  share of the one `selection` question) and `score` (was `rank`); the
  events keep their names and TypeSafe's `/v1/systemone` stays the backend.
  The service now never offers `bot.ability.reasoning.input` and builds the
  original's `{"turn": {"envelope", "data"}}` state from a `stimulus` param,
  so intuition passes its `choices` and `stimulus` as they are. Open: the
  plan's "To review (Q8)" list.
- **Parity gaps 3, 5 and 4 ported; gap 6 stopped (2026-10-18, D117 to
  D119, [parity-gaps](plans/parity-gaps.md)):** recalled context trims to
  the prompt budget instead of failing the turn (D117; open: on wasm the
  data model's 64 KiB value bound refuses a prompt before it is big enough
  to trim, so the maintainer lowers the budget or the bound is raised
  upstream); a `bot.deactivate` while activating answers
  `bot.deactivated`, and an unacknowledged cancel announces `bot.degraded
  {stage: "processing_cancel"}` (D118); stimuli and cognition inputs carry
  producer-stamped `participants`, the conversation stamping with
  `<conversation:turn_participants>` (D119). Gap 6
  (`bot.ability.speaking.stop`) waits on decision 6 and gap 11: the
  original's stop is built on `c88b6cd`'s streamed utterance and its
  `OutputData` (`voice_id`, `spoken`, `unspoken`).
- **The behavior and autonomy namespaces, and no bot namespace (2026-10-18,
  D116, [plan](plans/original-domains.md) step 4, the last):**
  `namespaces/behavior` (`<behavior:instances_from_behavior_results>`,
  `<behavior:replace_behavior_clauses>`, `<behavior:lifetime_errors>`,
  `<behavior:verify_tick>`) and `namespaces/autonomy`
  (`<autonomy:behavior_input_payload>`) replace the reflection's,
  the builder's and autonomy's behavior JavaScript; a create's draft is
  stored as an accepted change is, by `replace_behavior_clauses`.
  `<bot:stimulus>` is plain `<assign>`s and `namespaces/bot` is gone, so
  `decision` keeps only `choice` and `pin`. Open: J7 (`next_after` stays
  JavaScript: `<scx:validate>` binds no extension package, so a routine
  draft cannot use a namespace element; an scxml.rs change), the plan's
  "To review (step 4)" list and its [Map status](plans/original-domains.md#map-status);
  the behavior and autonomy namespace IRP baselines were measured on macOS
  arm64. On macOS, `scripts/namespace-irp.sh prepare` fails at
  `cache-create` (its `mktemp -d` is under the `/var` symlink); prepare
  with `scx conformance prepare` and a `--cache` in the scratchpad.
- **The identity, conversation and memory namespaces (2026-10-18, D115,
  [plan](plans/original-domains.md) step 3):** `namespaces/identity`
  (`<identity:hears_name>`, was `<bot:recognize>`;
  `<identity:sorted_identities>`), `namespaces/memory` (`<memory:to_json>`,
  `<memory:contents>`) and the `conversation` namespace's ten elements
  (`<conversation:message_for_turn>`, was `<conversation:commit>`, and
  the original's `_relationship_for_input`, `_participant_for_source`,
  `_fail_turn`, `detector_refs`, `_new_track_ref`, `_content_for_detector`,
  `_complete_turn` and the turn detector's `_ingest_content` and `_on_end`)
  replace the conversation's, the turn detector's and reasoning's row
  JavaScript, writing what it wrote. `isCommittedAppend` stays JavaScript
  (it searches the history). Next in Q10's order: `behavior`. Open: the
  plan's "To review (step 3)" list; the bot, conversation, identity and
  memory namespace IRP baselines were measured on macOS arm64.
- **The intuition and cognition namespaces (2026-10-18, D114,
  [plan](plans/original-domains.md) step 2):** `namespaces/intuition`
  (`<intuition:observe>`, `<intuition:escalate>`, the original's
  `ConfidenceTuner`, its state the `confidence_tuner` datum) replaces
  `<decision:tuner>`; `namespaces/cognition`
  (`<cognition:attention_bias_for_turn>`,
  `<cognition:model_facing_content failurelocation?>`) replaces
  `<bot:attention>` and reasoning's prompt JavaScript, rendering the same
  bytes. `<decision:pin>` stays (Q11); `autonomy` is a later step. Next in
  Q10's order: `identity`, `conversation` and `memory`. Open: the plan's
  "To review (step 2)" list; the four namespace IRP baselines touched were
  measured on macOS arm64.
- **The processing namespace and every stage's sends (2026-10-18, D113,
  [plan](plans/original-domains.md) step 1):** `namespaces/processing`
  (`<processing:model_facing_schema>`, `<processing:collect_offered_events>`,
  `<processing:send max?>`, `<processing:selection_confidence>`) replaces
  `<decision:compact>`, `<decision:assemble>`, `<decision:selection>` and
  intuition's confidence JavaScript; `<decision:route>`, `<decision:names>`,
  `<bot:verdict>`, the body's `routing` state, the reasoning and reflection
  `.js` merges and D87's dedupe are gone. Every choice is tagged with its
  offerer's session id, and each stage sends its own selections with
  `<foreach>` + `<send targetexpr="'#_scxml_' + s.target">`; the decision
  service names each selection's only offerer. Next in Q10's order:
  `intuition` and `cognition`. Open: the plan's "To review (step 1)" list;
  the decision, bot and processing namespace IRP baselines were measured
  on macOS arm64.
- **Tools, not structured output (2026-10-16, D112, amended
  2026-10-17 and 2026-10-18):** the generate service asks for schema-shaped output as the
  original does on Chat Completions: one function tool (the `schemaname`,
  the schema's root `description` as the function's, the rest of the
  schema as its `parameters`, no `strict`), `"tool_choice": "required"`,
  and `reasoning_effort` `none`. `generate.result` is always
  `{content, tool_calls: [{id, name, args}]}`, each call of the function
  checked against the schema before any is returned (`schema-mismatch`),
  after its top-level fields the schema's root `properties` do not
  declare are dropped (the original reads only `args.events`; nested
  objects stay strict); `strict` is gone. Reasoning and reflection send
  the `dispatch` calls' events with nested `<foreach>` (D113), the builder
  takes its call by name, and `<processing:send>` builds the original's
  dispatch tool exactly. The maintainer's mapping of floppy's domains onto
  the original's modules is [plans/original-domains.md](plans/original-domains.md);
  its step 1 is done (above).

- **The environment (2026-10-16, D102 to D111,
  [plan](plans/environment.mdscript.md)):** the bot runs as a participant of
  `environments/room/room.scx`, the root session, and activates on joining
  (no `bot.activate`; `inactive` is its final). The room routes host input,
  broadcasts sound and visuals with the original's placement and loudness,
  and logs the bot's events; the communication puts speaking's sound into
  the room after the efference copy (D103, amended). Devices are documents
  the room invokes, held through `holdings` and `owned_devices`, read with
  the new `namespaces/device` (`<device:snapshot>`) and acted on at their
  address; the cognition input's `environment` field is rendered by
  reasoning as the original's environment block. Every environment run
  passes `--fuel 9223372036854775807` until average usage is measured
  (D111). scxml.rs is at `5bc75e72` (D125's `..` in `event:schema`), on top of `cf8f293a` (D123's eventless-cond fix), on top of `f920cb47` (D122), on top of `c8bb69a8` (D121) and `b59f8111` (D104, D109, D110). Speech and voice
  backends exist now, each through an env file in the session scratchpad
  (outside the repository) named in `FLOPPY_LIVE_ENV_FILE` with `.env`:
  `env.speech` (Gemini's OpenAI-compatible API, `FLOPPY_SPEECH_*`) and
  `env.voice` (a local LocalAI on `127.0.0.1:8089`, `FLOPPY_VOICE_*`,
  started by the scratchpad's `localai/start.sh`; its
  `pyannote-diarization` model serves NVIDIA Streaming Sortformer v2).
  `environment_live`'s `live_two_participants` passed on them with the
  vLLM on 2026-10-17 (docs/live-proof.md). Open: the bot and device
  namespace IRP baselines were measured on macOS arm64 (regenerate on
  Linux x86_64).

- **Behavior learning proven live (2026-10-12, D100, D101, D96):** reflection
  offers change and break only for inventory names (neither on an empty
  inventory); the cascade stage, reflection's select and revising, and the
  builder's generations are bounded by inactivity while their reply streams
  and by a backstop always (decision D101). Streaming is per backend:
  `FLOPPY_STREAM=true` (unset by default; the CPA gateway cannot stream, the
  vLLM can); scxml.rs's `outbound-http.send` delivers a streamed body as it
  arrives, the generate service returns `generate.progress` (a reply that
  is not streamed returns none, so only its backstop bounds it), and the
  builder passes it on to reflection with `builder.progress` for each stage;
  the builder caps its replies at 8192 tokens. Live (docs/live-proof.md):
  CPA with streaming off passed; the vLLM with streaming on completed every
  turn but failed the learned-by-turn-2 assertion twice (the model's
  choices; one run's author replies all hit the token bound).
  The stage and revision bounds are configurable through `bot.activate`; the
  turn behavior contract names `parent.data` as a
  conversation turn's message. Two `live_conversation` runs passed: the
  behavior was learned on turn 2, answered "pong" on turn 3 through
  autonomy, and left turn 4 to reasoning, which recalled "green".
- **Conversation history in reasoning's prompt (2026-10-12, D99, D97):**
  reasoning's prompt renders object items of JSON arrays, so the
  conversation's `messages` (the history, replies included) reach the
  model; before, only `<value count="N" />` did, and the fourth
  `live_conversation` turn could not recall the first turn's fact on main
  or this branch. Reflection now stores a failed turn's episode (behavior
  null), so a turn whose reflection fails is not lost to the next turn's
  recall. `live_conversation` now asserts a behavior learned by turn 2 and
  "green" in turn 4's answer (model-dependent).
- **Speaking bound and self-hearing (2026-10-12, D98):** speaking's
  `encoding_timeout_ms` is 30 s (the communication passes 30 s and bounds
  a response by 36 s), since Gemini's unstreamed synthesis of a sentence
  outlasted the original's 5 s; `environment.sound` carries an optional
  `source` [floppy field], which speaking stamps with its mouth, so the
  listening's sensitivity scores the bot's own sound against the efference
  copy and the interpretation withholds its product. Voice detection still
  runs on it and fails without `FLOPPY_VOICE_BASE_URL`, as designed.
- **Selections the body can't place (audit item 4, 2026-10-10; superseded by D113, 2026-10-18):** a turn
  whose selections cannot all be placed fails with `bot.processing.failed`
  and the original's `SelectionRejectionError` message, never
  `bot.processing.completed`; `floppy.route.dropped` is gone. `<bot:verdict>`
  (ignore selections removed, the reference `attention_selection_error`) and
  `<decision:route rejectionlocation choices>` (the reference `_resolve_target`)
  run where the original dispatches: in autonomy before a learned behavior's use
  is counted (a rejected one is counted failed and fails the turn),
  intuition ("Selection rejected: ...", without the original's one retry),
  reasoning (before returning its selections), and in the body's `routing` before it logs
  the outcome (which catches the trusted seed's selection).
  `<decision:assemble>` lost `routeslocation`.
- **Phase 7 (2026-10-10):** done except publishing
  ([plan](plans/phase7-pglite-defaults.mdscript.md), decisions D48 to D51
  and D69 to D72). Memory runs on two engines behind the unchanged Memory
  ability, picked by its `database`: `services/sql` (SQLite, a file in the
  directory the run grants) and `services/pgsql` (the Postgres wire
  protocol over `wasi:sockets/tcp`, a `postgresql://USER@IP:PORT/DBNAME`
  URI, granted `inherit-network`), with the same `sql.result`/`sql.failed`
  contract, `rowcount` guards and open transactions. Migrations are
  Alembic's: `<migration:plan>` (`namespaces/migration`) and
  `abilities/migration/migration.scx`, with the original's revisions per engine
  in `abilities/memory/{sqlite,postgresql}/` and the version in
  `alembic_version`. Every ability's SQL is portable to both. Postgres
  support is tested on Postgres 16 (`FLOPPY_POSTGRES_BIN=<bin dir>` runs the
  ignored `postgres_*` tests on a fresh cluster each); the user chose SQLite
  for now (2026-10-10), so running them on PGlite through pglite-socket
  (`FLOPPY_PGLITE_SERVER`) is future work. PGlite inside a component waits on `scx` admitting
  exception handling (D71). Defaults are in README.md's "Defaults" and
  `.env.example` (`tests/tests/defaults.rs`). Releases are prepared, not
  published: [release.md](release.md); its open decisions (where the
  release lives, a release job, the scxml.rs release it follows) wait for
  going public.
- **Building and testing (2026-10-10):** `WASI_SDK_PATH` must name wasi-sdk
  25 (the sql service compiles SQLite). Live calls (CPA, TypeSafe) from a
  cloud session go through the session's proxy by a local relay, since
  `scx` ignores proxies.
- **Layout (2026-10-10, D65):** every bot and ability is
  `<name>/<name>.scx` with its ECMAScript in `<name>/<name>.js` beside it
  (one root `<script src="<name>.js"/>`; conds and exprs call its
  functions). Abilities are `abilities/<ability>/<ability>.scx`, each referencing
  `abilities/schemas/` as `../schemas/<event>.json` (D125; the
  `abilities/<ability>/schemas/` twins are gone). Tests lay abilities out with
  `support::copy_ability`. `scx validate` reads each document's `.js`
  (scxml.rs `98fd120`, `Validator::with_script_resources`), so a cond or
  `<log expr>` calling its functions is judged by their bodies and every
  document validates with 0 errors, 0 warnings, and 0 notes.
- **floppy:** work continues on branch `claude/project-thread-brkerr`
  (pushed to the private `gabewillen/floppy`; `main` is behind it).
  Phase 4 is done through "Replace Script Validators With Event Schemas":
  no phase 4 document has a `<script>`, payload checks are `event:schema`
  only, conds only correlate, and `tests/tests/contracts.rs` enforces the
  structure rule (no `function`, cond/expr at most 160 characters, every
  schema reference resolving to its event's one file (D125), literal SCXML
  invoke ids and srcs). The first identity attempt is on branch `wip/identity-slice`
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

## Intuition contract and reasoning prompt (2026-10-10)

- **Intuition** uses the decision service's kind `predicate` (`select`
  until D120; the reference decision processor, D150): `decision.selected
  {selections: [{event, data, confidence, target?}]}`, only the chosen event
  with its probability. Intuition observes that confidence (0-100) with
  `<intuition:observe>` first, then `<intuition:escalate>` compares it with
  the updated threshold (the original's `_tune_confidence`, D114). `low_confidence` and `deliberative_input` send
  only the environment actions before the turn escalates (never the ignore
  or the handoff), and every donedata carries `acted`, what was sent. The
  service's pass criteria are one per tier of `hand_ons` (`__intuition__`,
  `__reasoning__`; `["reasoning"]` by default), a pass returning `hand_to`;
  its state is `{turn: T}`, T the stimulus's `<cognition:model_facing_content>`,
  and data grounds from the `stimulus` param's non-null value then the enum
  (no `const`). With `max_context_tokens` the request is fitted to the
  model's context, and intuition learns and returns `bytes_per_token` from
  the service's done data. `choice` (`choose` until D120, the builder's
  triage) keeps its pass criterion `__unhandled__`; it lost its `threshold`
  (and `below-threshold`).
- **Reasoning** sends the original's `model_facing_xml(ProcessorInput)` under
  the original's budgets (`<cognition:model_facing_content>`, D114; it was
  `promptProjection()` in reasoning.js, checked byte-for-byte against the
  original's renderer on sample inputs, and the element renders what that
  JavaScript rendered). The reasoning
  and intuition system prompts still describe floppy's old inputs (gap 9,
  the maintainer's).
- **Tests in worktrees:** a `target/` copied from another checkout keeps
  `floppy-tests` binaries whose `CARGO_MANIFEST_DIR` is the other
  checkout, so `support::workspace()` builds and runs there. Run
  `cargo clean -p floppy-tests` (and remove `target/test-components`)
  first.

## Phase 3 (2026-10-10): memory service

- **Branch:** `claude/project-thread-r7141w` (from `claude/project-thread-brkerr`),
  pushed to `gabewillen/floppy`; the phase 4 branch merges its commits.
- **Done:** `services/sql` (one SQLite transaction per `<invoke>`, any
  schema) and `abilities/memory/memory.scx` (the original's Memory: migrates the original's
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

## Phase 5 (2026-10-10): behaviors and routines

- **Done on `main`:** autonomy matches the ACTIVE turn behaviors the
  stimulus triggers and runs each source by `<content expr>` (after the
  SpeechHeard seed); reflection selects through its enabled transitions,
  revises behaviors with the builder bot and stores episodes; routines run
  as a chain of runners passing `bot.behavior.tick` up. Decisions D60 to
  D64, D73 to D75, D80 (the plan's D48, D49 and D55, renumbered past clashes).
  Plan: [plans/phase5-behaviors.mdscript.md](plans/phase5-behaviors.mdscript.md).
- **Routines and revision, the original's behavior (2026-10-10):** a routine's
  `at` schedule computes its next occurrence in its own data model with
  `next_after` (reflection.js, the reference `At.next_after` over the zone's POSIX
  TZ rule: the ECMAScript data model has no `Intl` or time zone database);
  the builder, given `min_every_ms`/`turn_after_bound_ms`, rejects drafts
  outside the reference `check()`'s lifetime bounds (E0009) and forces a
  routine's first tick (E0008); the tick schema refuses an empty `output`;
  reflection drafts the builder's last rejected source and drafts nothing
  when the change call itself failed.
- **Seams on (2026-10-10, D80):** the cognition always runs reflection and
  routines (no gates, as the original composes them); with no database each logs
  `absent`. Every bot turn with memory now ends with reflection's select
  call and the turn's episode, which reflection alone stores (D97: reasoning
  retains none, so each turn records one episode, a failed reflection's
  included).

## Conversation memory (2026-10-10)

- **Done** on branch `wt-conversation`: the conversation keeps the original's
  relationships (matched in either direction, targetless input in the
  ambient one) and keys its memory by the original's `relationship_context_ref`
  (`{"identity_groups":[...]}`, written by `<conversation:commit
  keylocation>`), recalls before the turns and remembers after them (each
  its own 5 s operation), and remembers the original's `Memory.to_json`. It
  answers `bot.ability.conversation.snapshot.request` and a repeated
  append; the communication forwards a non-text product whose latest
  message is inbound. Memories kept by earlier floppy runs, under the old
  `relationship-<ids>` key, are not recalled.

## Turn detectors (2026-10-10)

- **Done** on branch `claude/project-thread-fubdvs`: the original's TurnDetector
  as `abilities/turn_detector/turn_detector.scx` (its seven events'
  schemas in `abilities/schemas/`), and the conversation's
  `turn_exchange`, which runs one turn per source of each input through it
  before memory and commits the turns' product. Decisions D76 to D79;
  conventions: [port-plan.md](port-plan.md#turn-detector-conventions).
- **Decided (D79):** The original commits a multi-source text input once per
  source (`hello hello hello`); the maintainer chose to say it once, so
  floppy joins each distinct turn text once. The detector has no decoder
  (D77).

## Phase 6 (2026-10-10): speech and vision

- **Done on `main`** (`d569318`): `services/speech` (decode, encode, and the
  voice operations `detect`, `diarize`, `identify` with the media steps
  `normalize`, `append`, `clip`); the original's hearing leaves, sensitivity,
  interpretation (open-speech window included) and listening, which the
  body invokes as its input ability; the SpeechHeard seed in autonomy;
  vocal and speaking in audio mode; image parts in the generate service;
  vision and reading (invoked by nothing yet, as in the original). Decisions
  D52, D57 to D59, D66, D67; D38 is superseded.
  Plan: [plans/phase6-speech-vision.mdscript.md](plans/phase6-speech-vision.mdscript.md);
  conventions: [port-plan.md](port-plan.md#phase-6-conventions).
- **Running:** the floppy bot's run also grants the speech service the
  media directory (`--dir https://github.com/stateforward/floppy/services/speech=HOST::/media`)
  and needs `FLOPPY_SPEECH_BASE_URL`/`FLOPPY_SPEECH_API_KEY` (speaking,
  speech decoding) and `FLOPPY_VOICE_BASE_URL`/`FLOPPY_VOICE_API_KEY`
  (LocalAI: every sound goes through voice detection). Without them a
  response fails and every sound is `bot.ability.listening.failed`.
- **Not ported:** devices (phone, LiveKit, audio hardware; out of scope)
  and the Conversation's matching of embedding sources to earlier
  participants' voice centroids. The turn detectors are ported (see
  below).
  ElevenLabs encoding exists in the service (`provider` `elevenlabs`), but
  no document selects it.
- **Live:** `speech_service`'s `live_speech_round_trip_through_an_env_file`
  is the one live speech test (ignored; it needs a speech backend in
  `FLOPPY_LIVE_ENV_FILE`). Reading passed live on the CPA gateway
  (2026-10-10), and on the Gemini API (D94) the speech round trip
  (speaking's encoder and hearing's decoder) and `floppy_live`'s
  `live_chat` with real spoken audio passed (2026-10-11,
  [live-proof.md](live-proof.md)); the listening ran live end to end on
  2026-10-17 (`environment_live`'s `live_two_participants`, voice
  detection on a local LocalAI at `FLOPPY_VOICE_BASE_URL`).

## Repositories

| Repository | Path | Branch | Notes |
| --- | --- | --- | --- |
| floppy (this repo, formerly nemobot) | `~/VSCode/stateforward/floppy`, remote `github.com/gabewillen/floppy` (private) | `main` | bots, floppy's namespaces and services, docs; no runtime of its own (D14) |
| scxml.rs | `~/VSCode/stateforward/scxml.rs` | `main` | engine, `scx` CLI, hosts, validator, resolver |
| scxml.rs work branch | `~/VSCode/stateforward/scxml.rs-services` (git worktree) | `feat/invoke-services` | all scxml.rs changes are made here first |
| Reference implementation (source of the port) | not in this repository | — | Python runtime being ported; reference only |

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
  `scx run --offline --cache-dir target/components/package-cache --env-file .env --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" --dir "https://github.com/stateforward/floppy/services/files=$PWD/behaviors::/behaviors" bots/builder/builder.scx`
  with a `builder.request` event on stdin (see `bots/builder/README.md`).
- Live tests: `FLOPPY_LIVE_ENV_FILE` (colon-separated env files) and
  `FLOPPY_LIVE_MODEL`.
- Model endpoint (decision D31): the `generate` service has no default
  backend; every run needs `FLOPPY_BASE_URL`, and without it the builder
  ends at once in `failed` (`unconfigured`); `FLOPPY_API_KEY` is needed
  only by a keyed backend (a run without it sends no key header, D127).
  floppy's gitignored `.env` sets them to the user's two DGX Sparks:
  - **direct vLLM:** `FLOPPY_BASE_URL=http://192.168.2.89:8000/v1`, model
    `Qwen/Qwen3.8-Flash-Next` (the builder's default), `FLOPPY_API_KEY` (a
    copy of the vLLM key, also kept as `VLLM_API_KEY`). Streams reasoning
    immediately.
  - the gateway in front of the Sparks is no longer used: it holds
    reasoning back, so long generations time out (D24, D25, live-proof.md);
    a report for its maintainer was drafted in the 2026-10-07 session.
  - decision service: TypeSafe (`TYPESAFE_API_KEY` in
    `.env.local`).
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
  `abilities/schemas/`, the original's domain names in
  `abilities/schemas/floppy-event-names.json`; documents reference them by
  `event:schema="schemas/<event>.json"`, never inline. A schema reference
  cannot contain `..`, so ability documents live flat as
  `abilities/<ability>.scx`. `tests/tests/contracts.rs` checks the schema
  set and every `.scx` under `abilities/` and `bots/floppy/`.

- Phase 4 body and thin cognition: `bots/floppy/floppy.scx` (the original's Bot
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
  the original's ConfidenceTuner passed by `<param>` and returned in
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
  through the body to the conversation (as the original's
  `bot.ability.conversation.routed_input`), whose text product (its whole
  history, as in the original) comes back as a `bot.ability.cognition.input`
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
  `008cd04a`): the shared offer compaction (the original
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
  thresholdlocation observe>`: reasoning's strict dispatch schema (the original
  `_selection_item_branch`/`dispatch_tool`) and intuition's
  `ConfidenceTuner` (the original defaults as Rust constants; the threshold is
  written as a probability) are namespace elements; the stage results are
  plain `<assign>`s. Reasoning's `onChoices` cond is gone: the generate
  service's strict-schema check already fails an off-choices reply as
  `schema-mismatch`. Intuition and reasoning now script only `isObject`
  (validators go with the event-schema slice), traces unchanged.

- `namespaces/bot` (floppy `242fb57a`, `d5fffb8d`), a new floppy
  namespace (`https://github.com/stateforward/floppy/namespaces/bot`,
  component `floppy_bot`, imports `types`, `telemetry`,
  `data-model-host`): `<bot:attention location focuslocation stimulus
  devices focus sources>` (the reference `attention_bias_for_turn`) and
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
  direction event message sequence provenance?>` (the original
  `_message_for_turn`, `_model_safe_content`, `_stable_identity_basis`,
  `_stable_context_ref`, `MessageProvenance`) builds one committed message
  and its relationship key; it never holds the history (the extension
  boundary's values are bounded at 64 KiB on wasm), so the conversation
  appends with `history.concat([message])` and
  `relationships[participants]` (since 2026-10-10 the key is the original's
  relationship context reference). The communication builds its outbound record
  with it (the append's provenance now carries explicit `null` source,
  target, session_ref and turn_ref; the committed history is unchanged).
  `the_conversation_commits_each_message_into_its_history` pins the
  committed messages field by field (blessed on the old document). Both
  documents now script validators only (`isObject`, `identities`,
  `validX`, `isProduct`, ...).

Next: phase 4 (bot core and abilities), per
`~/.agents/projects/floppy/plans/20261008-phase4-bot-core.mdscript.md`.

## Port run decisions (user, 2026-10-08)

The remaining the original port runs as one ultracode run with these grants:

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
- **Behaviors and routines (phase 5) use the builder:** The original has the model
  author behaviors and routines as Starlark HSM programs and compiles them at
  runtime (`behavior/compiler.py`). floppy mirrors that with
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
2. D121, to review (choices made closest to the design where it left a
   question open):
   - The path is `../../behaviors/<name>.scx`, written in `builder.js`: it
     resolves the same from the builder and from the candidates and
     routines runners only because each sits two levels below the layout
     root. A path given to the builder (a request field or `<param>`)
     would free that, at the cost of a new name.
   - The files service's guest directory is fixed at `/behaviors`, as the
     example's is at `/workspace`.
   - `<scx:validate>` waives `EVENT004`: `<scx:eventschema uri>` is a
     literal, and the draft's file name is not known until run time, so
     the validator never sees the schema file (the simulation's compile
     does). A `uriexpr` upstream would close it; it was not added (no
     ceremony in scxml.rs).
   - scxml.rs fetches a `srcexpr` document (and its schema files and
     scripts) each time its `<invoke>` runs, never from the run's memo,
     so a repaired draft written under the same name loads as written.
   - A row's `source` column holds the path (no migration of the
     original's schema); routines tell a revision by `updated_at` (second
     resolution), since a revised program keeps its path.
   - The builder writes every draft before validating it, so a change
     being built overwrites the stored program's files at once, and a
     failed change leaves its rejected draft there, as its DRAFT row says;
     it always writes `<name>.js` (empty for no script) so a read back
     finds all three.
   - The builder makes a name one plain file name (each run of characters
     other than letters, digits, `_` and `-` becomes `_`), so two names
     can share a file; a read back that fails fails the revision.
   - `SessionEmbedder::scripts` takes the session's base URI (a host API
     change in scxml.rs), and the CLI keys scripts by URL.
   - Live (2026-10-19, docs/live-proof.md): no `XML001` came from a schema
     or a script any more, but no behavior was learned either (the drafts
     fail their simulation on the turn behavior contract). (Resolved by
     D122: the draft whose simulation looped no longer ends the run; it
     fails its test and is repaired.)

## Backlog (agreed, not started)

- scxml.rs: check a service's WASI grant only when an invocation of it
  starts (or let a run declare a grant optional). `scx run` resolves every
  service the bound document tree names before the session starts, even one
  in a state never entered, so the floppy bot's every run grants the vision
  service its media directory though seeing is off by default (D185, PR #8).
  With the check moved, the default run would need no vision grant.
- Browser parity slice 2 (services, resolution, WASI shims, element trace).
- `scx compile --target native`: one self-contained executable per platform;
  top-level `<data>` ids are the arguments, `<donedata>` JSON on stdout,
  `<log>` on stderr, exit code from the final state.
- Exporter components for session outputs (`<log>`, `<donedata>`, trace):
  stdout default, file/WebSocket/OTLP packages. Low priority.
- PGlite compiled into a component (D71: needs `scx`'s wasmtime with the
  `gc` and `gc-null` features, a proposed scxml.rs change not made).
- Website (after the port): the original's animated robot (`web/src/face`),
  web components + hsm.ts, marketplace of packages, a Mindstorms-style
  workshop whose pieces are inferred from element XSDs, live highlighting
  of running bots.
- Live listening: voice detection (every sound) runs on a local LocalAI
  at `FLOPPY_VOICE_BASE_URL` (the session scratchpad's `env.voice`), and
  speech decoding and speaking on Gemini (`env.speech`); the listening
  ran live end to end on 2026-10-17 (live-proof.md). Diarization and
  voice identification have not run live.
- Going public: CI, publishing the prepared releases with `SHA256SUMS`
  ([release.md](release.md)), per-platform `scx` binaries, IRP identity
  regeneration on Linux CI.

## Known stale docs

- The "Where things stand (2026-10-08)" section above is history; the
  current state is at the top of this file.
