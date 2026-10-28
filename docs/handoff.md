# Handoff: resuming the floppy / scxml.rs work

Read this first when picking the work up in a new session or account. It
records where things stand, how to work, and what is open. The detailed
history lives in [decisions.md](decisions.md) (numbered decisions),
[port-plan.md](port-plan.md) (the concept map, phases and conventions), and
[live-proof.md](live-proof.md) (every live run). Where they disagree with
this file, the newer of decisions.md and this file wins.

Last updated: 2026-10-26.

## Current state (2026-10-09) — resume here

- **Latency hill climb: long-lived hearing (2026-10-27, D264):** the
  interpretation invokes its six hearing stages once and sends each a
  request per sound (`<name>.input {id, ...}`, answered by `<name>.output`
  or `bot.ability.failed` with the id), as mosfet.py's Interpretation holds
  them. Speaking's vocal is long-lived too (D265, cancelled by
  `bot.ability.processing.cancel`). First speech on the release probe:
  13.9 / 9.1 / 9.1 s before, 12.9 / 8.9 / 8.4 s after D264. Still
  restarted per turn (`/mnt/project-files/floppy-perf/long-lived-invokes.md`):
  cognition's autonomy, intuition and reasoning (the decision layer is
  long-lived, D268), which build on
  the long-lived memory's addressing (PR #130); then the turn detector and
  the candidate loop folded into states. Embedding is long-lived beside
  the memory (D267, addressed like it).
- **Latency hill climb: speech out (2026-10-27, D262):** a reply is
  said while the model is still writing it (mosfet.py's drafts).
  Communication's respond offer declares a draft marker; reasoning passes
  the choices' markers to the generate service (`drafts`), which returns
  `bot.ability.communication.draft` as the streamed dispatch call's words
  arrive (`services/generate/src/drafts.rs`, mosfet.py's DraftParser);
  communication sends `bot.ability.speaking.draft` on to speaking, vocal
  invokes the speech service's `encode` without `parts`, and the service
  says each finished sentence as soon as it is written
  (`services/speech/src/drafting.rs`). Reasoning streams by default (its
  `stream` `<data>`; no env flag). Hearing streaming in (api.willen.dev's
  WebSocket `/v1/audio/stream`) waits on Gabe: scxml.rs's outbound-http
  has no WebSocket call yet.
- **Latency hill climb: hearing in parallel (2026-10-26, D261):** with
  diarization, the interpretation decodes the utterance while it diarizes
  it (`DecodingSpeech.Diarized { Diarizing -> Identifying }`), and hears
  the tone and embeds the voices together; voices are identified once the
  words are in. Same products; about 0.3 s less hearing per utterance on
  api.willen.dev (2.04 to 1.71 to 1.86 s, 2.58 to 2.18 to 2.38 s). What
  is left between the calls is engine time (about 0.1 s each to start a
  child) and the two voice detections, which wait for the sounds.
- **Latency hill climb: routing (2026-10-26, D260):** everything runs on
  api.willen.dev (`gemma-4-26b-a4b` on `FLOPPY_BASE_URL`) except
  reflection, learning, revision and the builder, which use
  `reflection_model` (`sparks/qwen3.8-flash-next` in floppy) on
  `FLOPPY_REFLECTION_BASE_URL` / `FLOPPY_REFLECTION_API_KEY` (CPA in front
  of the Sparks); `reflection_model` null keeps them on `model` and
  `FLOPPY_BASE_URL`. A live spoken turn was about 10.7 s to first speech:
  about 5 s of engine CPU (the ECMAScript data model re-parsing values and
  events, a fresh JavaScript realm per session), 3.3 s of serial hearing
  calls, 2 s of reasoning. Next: scxml.rs builds JSON values directly
  (upstream first), a pre-initialized data-model snapshot, HTTPS
  connection reuse, then the hearing calls in parallel.
- **Recall by association (2026-10-26, D256, #86):** the Embedding
  ability (`abilities/embedding/`) embeds each episode's heard words
  (reflection, stored in `bot_memory_embedding`) and each turn's cue
  (reasoning), on `FLOPPY_EMBEDDING_BASE_URL` (`embeddinggemma-2`, 256
  dimensions, api.willen.dev). Reasoning ranks the 64 newest episodes by
  mosfet.py's associative score and recalls the best within 24576
  characters; without an embedder recall is by recency.
- **Decisions from the snapshot (2026-10-26, D257, #84 #95 #96):** a
  failed turn hands back its whole stimulus, so the turn that perceives the
  failure answers what went unanswered; the environment stamps the bot as
  an `own` target when `target_ids` names it, and the ignore's description
  says whom words are for in those terms; being named is the identity's
  state (`environment.self.identity {name, addressed}`), read each turn
  once the identity answers the turn's ask, never a turn of its own.
  Eval (12 scenarios, the stratified split, seed 7): held-out 0.985
  (baseline 0.550), suite 0.959 (baseline 0.452); chat_ignore is noisy (0.83 over four runs: two answered "Ana, could you pass me the salt?"). Open: overheard,
  ignored turns are not recalled (people_overheard, memory's), and
  `a_bot_hears_its_own_speech_through_the_environment_and_withholds_it`
  fails on main too.
- **The workshop compiles and runs bots in the browser (2026-10-26, #104):**
  the website's workshop validates, compiles and runs a document in the
  page with scxml.rs's browser host and the same components `scx run`
  uses (`web/src/runtime.js`); its JavaScript preview interpreter is gone.
  scxml.rs's browser host build takes `--extensions <dir>` so it ships
  floppy's namespace components (scxml.rs `1e16968e`),
  `web/build-host.sh` builds it into `target/web`, and the Pages workflow
  runs it. The browser host resolves an `<invoke src>` child's own scripts,
  data, schema files and children against the child's base URI (the page
  answers each document with `baseUri` and `eventSchemas`, as the Wasmtime
  embedder's `EmbeddedDocument`), so 38 of the 40 documents run with their
  children in the page. Invoked services run in the page too (scxml.rs
  `e4896082`, browser parity slice 2): the host build ships every service
  but `pgsql` (sockets), the TestBench's Service keys dialog holds the
  `env` values in the page's memory only, requests go through the page's
  `fetch`, and `sql`, `files`, `generate`, `speech` and `vision` get
  in-memory directories at their `--dir` paths (JSPI: Chrome 137+). Not
  yet: the floppy bot and the environment exceed Chromium's limit of about
  125 live Wasm memories per page (each session holds about 20: machine,
  data model, I/O processor, clock and namespaces), so a tree of more
  than a handful of sessions fails with `instance-lost`, which the
  TestBench explains. Host-side sharing cannot fix it: the data model,
  I/O processor, clock and extension WIT interfaces hold one session per
  instance, so it needs a WIT contract change (stateforward/scxml.rs#33,
  not scheduled: the maintainer's call). The register's
  facts come from `web/src/register.py`.
- **Voices are remembered as people (2026-10-26, D258):** with a
  database, the interpretation recalls the people long-term memory holds
  before it is ready, and remembers each new voice as a person whose ref
  is the voice's `voice:<id>` (the representative its running mean), so a
  voice is recognized across restarts (`listening.rs`
  `a_voice_heard_before_a_restart_is_recognized_after_it`). The body
  passes its database through the listening. The bot's own voice is still
  learned again each run.
- **The prompt fits a generate call (2026-10-26, D255):** the cognition's
  prompt budget (`MAX_PROJECTION_BYTES`) is 49152, down from 262144, so
  the system channel, the schema and the prompt fit the 65536-byte event
  value a generate call's `messages` is; recalled turns, episodes and
  directives are trimmed on wasm instead of failing the turn with
  `error.execution` (reasoning case 141 fails at the old budget). The
  cognition namespace IRP's manifest names the new budget, but its prepared
  corpus still carries the old statement text: re-prepare it
  (`scripts/namespace-irp.sh prepare cognition`, which needs SaxonJ-HE)
  where Saxon is installed.
- **A voice is named by its ref (2026-10-26, D253):** the interpretation
  owns the voice registry (mosfet.py's identification), so an embedding
  never leaves it and products carry `voice:<id>` refs. This keeps a long
  spoken conversation's episodes and prompts small (the embeddings used to
  push a generate invoke past 64 KiB by the third spoken turn). The
  registry is in the session only: a restart forgets voices until it gets
  mosfet.py's `IdentityStore` (`bot_identity`), the next step.
- **What is heard and said is remembered (2026-10-25, D251, #85, #87):**
  the conversation writes each message it commits with words to the
  short-term register (heard for inbound text, typed or decoded from
  speech; said for appends), so the recent turns reasoning reads are no
  longer empty, and each episode keeps the words its stimulus carried
  (`heard`). Memory's held-out eval score went from 0.23 to 0.81. Episodes
  keep no images yet (the seeing thread's), and recall is still by
  recency, not by meaning (embeddings next).
- **Speech on api.willen.dev (2026-10-25, D244):** both speech variables
  name `https://api.willen.dev/v1` (its openapi.json is the contract), with
  the documents' models unchanged; the GPT-Live harness ran on it with
  VAD, STT, TTS and diarization ([live-proof.md](live-proof.md)). A
  diarized end up to 0.1 s past the utterance is now cut to it. Its
  emotion, voiceprint and streaming STT (`/v1/audio/stream`, with
  turn-taking) have no floppy ability yet.
- **The decision element trimmed (2026-10-24, D243):** `<decision:pin>`
  is gone (the body's `isFocusCandidate` guard and reflection's
  `hasBehaviors()` cond do its work, as mosfet.py's guards do), and the
  decision service asks only TypeSafe `system_one` `choice` questions:
  `kind` is no longer a param, `predicate` is folded into `choice`
  (`decision.selected`, one pass criterion per tier of `hand_ons`), and
  `score` and `choice` over labels are removed. The builder's triage sends
  itself the selection. The learning stack dropped its pins when it took
  the trim: learning's change and break take `hasBehaviors()`, and
  reflection's change takes `hasChangeable()` and refuses a change of a
  behavior that declined the turn as `declined-change` (D230).
- **Offers through snapshots (2026-10-24, D242, #73):** abilities no
  longer send `bot.ability.offers`. Each sends a bare `bot.ability.ready`
  once, and each turn the body reads what each ability offers from its
  live session with `<decision:offers location target undeclared?>`: the
  enabled `event:schema` transitions without a cond that its
  `<data id="offerable">` (`{event: {effort?}}`) declares. A cond'd
  transition is never offered but still shadows later ones, so a guarded
  offer moves its guard into a transient state or an `<if>`
  (communication's `admitting_response`). Effort is declared in
  `offerable`; `<decision:choice>` takes no `effort`. Each ability's
  offers are bounded alone (32 KiB, `floppy.offers.dropped`), and the
  firmware is started once its session answers a snapshot
  (`firmware_check_ms`).
- **Effort ceilings (2026-10-24, D206, #50):** each tier of a turn is
  offered only the actions within its effort ceiling (D196). An offering
  document declares an action's effort in its `offerable` data (D242; an
  expression, 0 when absent, so the ignore and every other offer stay
  light); identity's adopt weighs 1 and persona's revise 2. The cognition
  frames each stage's choices with the new `<cognition:within_ceiling>`
  against `decision_effort_ceiling` (0) and `intuition_effort_ceiling` (1),
  which the bounds override; reasoning's ceiling is the whole frame. What a
  tier is not offered is its `held` param, rendered as the `<held>` block
  of its model-facing content (the decision layer's state, intuition's
  prompt), so it can only hand it on. The efforts and ceilings are the
  maintainer's to tune; no live run has measured them.
- **The bot learns what it is taught (2026-10-23, D148, D149, D151):**
  the cognition always offers `bot.ability.learning.input` beside its
  ignore, so any tier can select a lesson, and after the turn's output
  its long-lived child `learning` (`abilities/learning/learning.scx`)
  selects a create or change and invokes the Revision document
  (`abilities/revision/revision.scx`, reflection's revision moved into
  its own ability) with the lesson as the builder's instruction.
  Reflection only remembers a taught turn and a turn the bot's own
  routine or lesson caused.
- **Rest and consolidation (2026-10-24, D205, #49):** after
  `rest_after_ms` (5 minutes) without input the cognition rests and
  invokes `abilities/consolidation/consolidation.scx`, which folds the
  voices heard into each person's compact representative
  (`bot_person_centroid`) and merges two people at D90's threshold,
  re-pointing everything keyed on the newer one. Each step is one guarded
  transaction after a five-second pause, and input leaves rest at once,
  cancelling the consolidation between two steps. Identification reads
  one representative a person (about 100 people at 256 dimensions), and
  new people are numbered from `bot_sequence`. Episodes from short-term
  turns are a seam (reflection already records each turn's, D97).
- **Voices are people in long-term memory (2026-10-23, D204, #49):** the
  conversation identifies each voice embedding through the memory ability
  before it finds the input's relationship: a voice memory holds is its
  `person-<n>`, any other a new person, and every voice heard is stored
  for its person (`bot_person`, `bot_person_embedding` in `0001`). The
  person is the participant the rest of the bot sees, across restarts.
  Faces and `voice_id` are what is left of #49 (rest is D205).
- **The docs describe floppy on its own terms (2026-10-23, #19):** the
  plans that tracked floppy against another system are retired. What they
  left open is under [Open work](#open-work),
  [Design decisions for the maintainer](#design-decisions-for-the-maintainer)
  and [Choices to review](#choices-to-review).
- **Learned behaviors are repaired and run on their live turn
  (2026-10-22, D130 and D131):** reflection repairs a
  select call the generate service refuses (D126's `rejection`) at most
  twice with repair feedback carrying the rejection, failing at once on the same
  rejection and then with `Selection rejected: <message>` (D130). The
  builder runs a turn behavior once on the live turn it is learned from
  (`builder.request`'s `apply`, which reflection always sends) and
  checks the run with `<behavior:verify_apply>` (D131); a routine keeps
  its forced tick; the author call (a whole document) is bounded by
  `document_max_completion_tokens` (16384). D132 (a fixed turn template
  whose script the model writes) is withdrawn: the maintainer wants no
  templates in the builder, and the bar is whether the model builds whole
  documents itself (to be tried on sparks). The build-time half
  of "nothing tests learned behaviors on real input" is done by D131;
  the runtime half, a learned behavior tested on a later real turn, is
  under Learning in [Open work](#open-work) (#14).
- **Seeing (2026-10-22, D185 to D189):** `abilities/seeing/seeing.scx`
  is the bot's eye: it holds the newest `environment.visual` of each
  `source` (a new [floppy field]; the schema moved to `abilities/schemas/`),
  measures change through the new `vision` service (`byte_identity`,
  `frame_diff`), looks when the model selects `bot.ability.seeing.look`, and
  hands each sight to the body as `bot.ability.cognition.input`; object
  detection and description are its per-view stage children over generate.
  The body invokes it always, and its `seeing_enabled` switch (off by
  default) decides whether it is expected, fed and offered; every bot run
  now grants the vision service a media directory. Faces follow D194 (#49).
  `frame_diff` decodes as OpenCV 5.0.0 does (its own
  JPEG DC decoder, libpng's gray and gamma, `image-webp`, Exif
  orientation), checked against OpenCV fixtures in
  `services/vision/src/testdata/`; Cargo.lock carries `miniz_oxide` 0.8.9
  for `png` beside scxml.rs's 0.9 (D185). A document IRP case may now
  require its run's logs (`logs=` in cases.py, `logs.json`).
- **Body and environment (2026-10-22, D155 to D160):** `bot.clear_focus`
  is gone (focus stays until the bot focuses another device; the reflex
  floor keeps it). A busy body holds what arrives (`floppy.held`), and the
  next turn takes at most 8 percepts, 64 KiB and 32 participants: one
  percept as itself, more as one `bot.perceived {percepts}`; what is still
  held when the bot leaves active is released (`floppy.released`), never
  replayed. `bot.input` carries `participants`, and every schema inlines
  the same `Participant` definition (`contracts.rs` checks they are equal;
  scxml.rs refuses a cross-file `$ref`). The environment supervises each bot in
  its own `supervising.<bot>` region with a restart budget
  (`max_restarts`) and a doubling backoff (`backoff_initial_ms` to
  `backoff_max_ms`), and a device may sit on an object (`on`, rendered on its
  `<device>`). The body starts a firmware after its abilities
  (`firmware_src`, default `bots/floppy/firmware/firmware.scx`, which offers
  nothing); an owner's firmware is offered, read each turn and rendered as
  `<firmware ref id state>`. A test that lays the body out must copy
  `firmware/` beside it, as `body.rs` does.
- **One schema file per event (2026-10-21, D125):** scxml.rs
  `5bc75e72` resolves an `event:schema` reference against its
  document's base URI like any relative URI, `..` included, so the twin
  directories are gone: abilities reference `../schemas/<event>.json`, the
  body and the environment `../../abilities/schemas/<event>.json` (and the environment
  `../../bots/floppy/schemas/<event>.json`); 240 schema files became 109.
  `contracts.rs` checks every reference resolves to its event's one file
  and no schema file name occurs twice. The builder references its
  `schemas/*.json` instead of inlining them. A test that lays documents
  out in a scratch directory must copy `abilities/schemas/` (and, for the
  body's events, `bots/floppy/schemas/`) beside them, as
  `support::copy_ability` and `support::environment_with_devices` do.
- **A build retries to its cap (2026-10-23, D133):** the maintainer chose
  "Retry to a cap" over D124's end at the first repeat. `max_attempts`
  defaults to 8 and is the only end of a failing build
  (`attempts-exhausted`); a diagnostic an earlier failure had goes to the
  next author call as `repeated`. The D124 entry below keeps its
  fingerprint rules.
- **The same failure more than once ends the build (2026-10-20, D124,
  amended by D133):**
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
  lab (`gemma-4-26b-a4b` on the vLLM at
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
- **An image is no turn content (2026-10-18):**
  `<conversation:ingest_content>` no longer takes an image stimulus as
  `image/*` content; until a turn carries attachments
  ([open work](#open-work)), an image adds nothing to a turn, and a turn
  of images alone fails as empty. The conversation namespace IRP was re-prepared and
  its baseline re-measured on macOS arm64.
- **Q8: the decision service's kinds are OpenAI Decisions API's question
  types (2026-10-18, D120,
  [to review](#choices-to-review)):** `choice` (the default;
  was `choose`, and `classify` when given `labels`), `predicate` (was
  `select`, its behavior unchanged: each event's confidence is still its
  share of the one `selection` question) and `score` (was `rank`); the
  events keep their names and TypeSafe's `/v1/systemone` stays the backend.
  The service now never offers `bot.ability.reasoning.input` and builds the
  `{"turn": {"envelope", "data"}}` state from a `stimulus` param, so
  intuition passes its `choices` and `stimulus` as they are (amended by
  D150). Open: the [choices to review](#choices-to-review).
- **Recall trimmed, lifecycle answers, stamped participants (2026-10-18,
  D117 to D119):** recalled context trims to the prompt budget instead of
  failing the turn (D117; open: on wasm the data model's 64 KiB value
  bound refuses a prompt before it is big enough to trim,
  [open work](#open-work)); a `bot.deactivate` while activating answers
  `bot.deactivated`, and an unacknowledged cancel announces `bot.degraded
  {stage: "processing_cancel"}` (D118); stimuli and cognition inputs carry
  producer-stamped `participants`, the conversation stamping with
  `<conversation:turn_participants>` (D119).
  `bot.ability.speaking.stop` came later (D179, 2026-10-22): the stop,
  progress and `spoken`/`unspoken`/`outcome` on unstreamed audio,
  `voice_id` null until D194 is built (#49).
- **The behavior and autonomy namespaces, and no bot namespace (2026-10-18,
  D116, the namespace plan's step 4, the last):**
  `namespaces/behavior` (`<behavior:instances_from_behavior_results>`,
  `<behavior:replace_behavior_clauses>`, `<behavior:lifetime_errors>`,
  `<behavior:verify_tick>`) and `namespaces/autonomy`
  (`<autonomy:behavior_input_payload>`) replace the reflection's,
  the builder's and autonomy's behavior JavaScript; a create's draft is
  stored as an accepted change is, by `replace_behavior_clauses`.
  `<bot:stimulus>` is plain `<assign>`s and `namespaces/bot` is gone, so
  `decision` keeps only `choice` and `pin`. Open: `next_after` stays
  JavaScript (`<scx:validate>` binds no extension package, so a routine
  draft cannot use a namespace element; an scxml.rs change), the
  [choices to review](#choices-to-review) and the [open work](#open-work);
  the behavior and autonomy namespace IRP baselines were measured on macOS
  arm64. On macOS, `scripts/namespace-irp.sh prepare` fails at
  `cache-create` (its `mktemp -d` is under the `/var` symlink); prepare
  with `scx conformance prepare` and a `--cache` in the scratchpad.
- **The identity, conversation and memory namespaces (2026-10-18, D115,
  the namespace plan's step 3):** `namespaces/identity`
  (`<identity:hears_name>`, was `<bot:recognize>`;
  `<identity:sorted_identities>`), `namespaces/memory` (`<memory:to_json>`,
  `<memory:contents>`) and the `conversation` namespace's ten elements
  (`<conversation:message_for_turn>`, was `<conversation:commit>`,
  `relationship_for_input`, `participant_for_source`, `fail_turn`,
  `detector_refs`, `new_track_ref`, `content_for_detector`,
  `complete_turn` and the turn detector's `ingest_content` and `on_end`)
  replace the conversation's, the turn detector's and reasoning's row
  JavaScript, writing what it wrote. `isCommittedAppend` stays JavaScript
  (it searches the history). Next in Q10's order: `behavior`. Open: the
  [choices to review](#choices-to-review); the bot, conversation, identity and
  memory namespace IRP baselines were measured on macOS arm64.
- **The intuition and cognition namespaces (2026-10-18, D114,
  the namespace plan's step 2):** `namespaces/intuition`
  (`<intuition:observe>`, `<intuition:escalate>`, a confidence tuner
  whose state is the `confidence_tuner` datum) replaces
  `<decision:tuner>`; `namespaces/cognition`
  (`<cognition:attention_bias_for_turn>`,
  `<cognition:model_facing_content failurelocation?>`) replaces
  `<bot:attention>` and reasoning's prompt JavaScript, rendering the same
  bytes. `<decision:pin>` stayed (Q11) until D243 removed it; `autonomy` is a later step. Next in
  Q10's order: `identity`, `conversation` and `memory`. Open: the
  [choices to review](#choices-to-review); the four namespace IRP baselines touched were
  measured on macOS arm64.
- **The processing namespace and every stage's sends (2026-10-18, D113,
  the namespace plan's step 1):** `namespaces/processing`
  (`<processing:model_facing_schema>`, `<processing:collect_offered_events>`,
  `<processing:send max?>`, `<processing:selection_confidence>`) replaces
  `<decision:compact>`, `<decision:assemble>`, `<decision:selection>` and
  intuition's confidence JavaScript; `<decision:route>`, `<decision:names>`,
  `<bot:verdict>`, the body's `routing` state, the reasoning and reflection
  `.js` merges and D87's dedupe are gone. Every choice is tagged with its
  offerer's session id, and each stage sends its own selections with
  `<foreach>` + `<send targetexpr="'#_scxml_' + s.target">`; the decision
  service names each selection's only offerer. Next in Q10's order:
  `intuition` and `cognition`. Open: the
  [choices to review](#choices-to-review);
  the decision, bot and processing namespace IRP baselines were measured
  on macOS arm64.
- **Tools, not structured output (2026-10-16, D112, amended
  2026-10-17 and 2026-10-18):** the generate service asks Chat
  Completions for schema-shaped output as one function tool (the `schemaname`,
  the schema's root `description` as the function's, the rest of the
  schema as its `parameters`, no `strict`), `"tool_choice": "required"`,
  and `reasoning_effort` `none`. `generate.result` is always
  `{content, tool_calls: [{id, name, args}]}`, each call of the function
  checked against the schema before any is returned (`schema-mismatch`),
  after its top-level fields the schema's root `properties` do not
  declare are dropped (nested objects stay strict); `strict` is gone. Reasoning and reflection send
  the `dispatch` calls' events with nested `<foreach>` (D113), the builder
  takes its call by name, and `<processing:send>` builds the dispatch
  tool.

- **The environment (2026-10-16, D102 to D111,
  [plan](plans/environment.mdscript.md)):** the bot runs as a participant of
  `environments/environment/environment.scx`, the root session, and activates on joining
  (no `bot.activate`; `inactive` is its final). The environment routes host input,
  broadcasts sound and visuals by placement and free-field loudness,
  and logs the bot's events; the communication puts speaking's sound into
  the environment after the efference copy (D103, amended). Devices are documents
  the environment invokes, held through `holdings` and `owned_devices`, read with
  the new `namespaces/device` (`<device:snapshot>`) and acted on at their
  address; the cognition input's `environment` field is rendered by
  reasoning as its `<environment>` block. Every environment run
  passes `--fuel 9223372036854775807` until average usage is measured
  (D111). scxml.rs is at `cf27c2e7` (D217's env masking only inside a record's JSON strings), on top of `5bc75e72` (D125's `..` in `event:schema`), on top of `cf8f293a` (D123's eventless-cond fix), on top of `f920cb47` (D122), on top of `c8bb69a8` (D121) and `b59f8111` (D104, D109, D110). Speech and voice
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
  "green" in turn 4's answer (model-dependent). **Amended by D176
  (2026-10-22):** the conversation hands reasoning only the current
  exchange, so turn 4 recalls "green" from
  reflection's episode of turn 1 (`prior_episodes`), not from the
  messages; `live_conversation` has not been rerun since.
- **Speaking bound and self-hearing (2026-10-12, D98):** speaking's
  `encoding_timeout_ms` is 30 s (the communication passes 30 s and bounds
  a response by 36 s), since Gemini's unstreamed synthesis of a sentence
  outlasted 5 s; `environment.sound` carries an optional
  `source` [floppy field], which speaking stamps with its mouth, so the
  listening's sensitivity scores the bot's own sound against the efference
  copy and the interpretation withholds its product. Voice detection still
  runs on it and fails without `FLOPPY_VOICE_BASE_URL`, as designed.
- **Selections the body can't place (audit item 4, 2026-10-10; superseded by D113, 2026-10-18):** a turn
  whose selections cannot all be placed fails with `bot.processing.failed`
  and a selection rejection message, never
  `bot.processing.completed`; `floppy.route.dropped` is gone. `<bot:verdict>`
  (ignore selections removed, a focus selection's device checked against
  the turn's focus candidates) and
  `<decision:route rejectionlocation choices>` (each selection placed on its
  target) run where a stage dispatches: in autonomy before a learned behavior's use
  is counted (a rejected one is counted failed and fails the turn),
  intuition ("Selection rejected: ...", with no retry),
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
  `abilities/migration/migration.scx`, with the memory schema's revisions per engine
  in `abilities/memory/{sqlite,postgresql}/` and the version in
  `alembic_version`. Every ability's SQL is portable to both. Postgres
  support is tested on Postgres 16 (`FLOPPY_POSTGRES_BIN=<bin dir>` runs the
  ignored `postgres_*` tests on a fresh cluster each); the user chose SQLite
  for now (2026-10-10), so running them on PGlite through pglite-socket
  (`FLOPPY_PGLITE_SERVER`) is future work. PGlite inside a component waits on `scx` admitting
  exception handling (D71). Defaults are in [defaults.md](defaults.md) and
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

- **Intuition** uses the decision service's `choice` (`select` until
  D120, `predicate` until D243; D150): `decision.selected
  {selections: [{event, data, confidence, target?}]}`, only the chosen event
  with its probability. Intuition observes that confidence (0-100) with
  `<intuition:observe>` first, then `<intuition:escalate>` compares it with
  the updated threshold (D114). `low_confidence` and `deliberative_input` send
  only the environment actions before the turn escalates (never the ignore
  or the handoff), and every donedata carries `acted`, what was sent. The
  service's pass criteria are one per tier of `hand_ons` (`__intuition__`,
  `__reasoning__`; `["reasoning"]` by default), a pass returning `hand_to`;
  its state is `{turn: T}`, T the stimulus's `<cognition:model_facing_content>`,
  and data grounds from the `stimulus` param's non-null value then the enum
  (no `const`). With `max_context_tokens` the request is fitted to the
  model's context, and intuition learns and returns `bytes_per_token` from
  the service's done data. The builder's triage asks the same `choice`
  and sends itself the selection (D243); the service lost its `threshold`
  (and `below-threshold`).
- **Reasoning** sends its input as budgeted XML
  (`<cognition:model_facing_content>`, D114; it was `promptProjection()`
  in reasoning.js, and the element renders what that JavaScript
  rendered). The reasoning
  and intuition system prompts still describe floppy's old inputs.
- **Tests in worktrees:** a `target/` copied from another checkout keeps
  `floppy-tests` binaries whose `CARGO_MANIFEST_DIR` is the other
  checkout, so `support::workspace()` builds and runs there. Run
  `cargo clean -p floppy-tests` (and remove `target/test-components`)
  first.

## Phase 3 (2026-10-10): memory service

- **Branch:** `claude/project-thread-r7141w` (from `claude/project-thread-brkerr`),
  pushed to `gabewillen/floppy`; the phase 4 branch merges its commits.
- **Done:** `services/sql` (one SQLite transaction per `<invoke>`, any
  schema) and `abilities/memory/memory.scx` (the Memory ability: migrates the memory
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
- **Routines and revision (2026-10-10):** a routine's
  `at` schedule computes its next occurrence in its own data model with
  `next_after` (reflection.js, over the zone's POSIX
  TZ rule: the ECMAScript data model has no `Intl` or time zone database);
  the builder, given `min_every_ms`/`turn_after_bound_ms`, rejects drafts
  outside the lifetime bounds (E0009) and forces a
  routine's first tick (E0008); the tick schema refuses an empty `output`;
  reflection drafts the builder's last rejected source and drafts nothing
  when the change call itself failed.
- **Seams on (2026-10-10, D80):** the cognition always runs reflection and
  routines (no gates); with no database each logs
  `absent`. Every bot turn with memory now ends with reflection's select
  call and the turn's episode, which reflection alone stores (D97: reasoning
  retains none, so each turn records one episode, a failed reflection's
  included).

## Conversation memory (2026-10-10)

- **Done** on branch `wt-conversation`: the conversation keeps
  relationships (matched in either direction, targetless input in the
  ambient one) and keys its memory by the relationship's `relationship_context_ref`
  (`{"identity_groups":[...]}`, written by `<conversation:commit
  keylocation>`), recalls before the turns and remembers after them (each
  its own 5 s operation), and remembers each input as its memory row's JSON. It
  answers `bot.ability.conversation.snapshot.request` and a repeated
  append; the communication forwards a non-text product whose latest
  message is inbound. Memories kept by earlier floppy runs, under the old
  `relationship-<ids>` key, are not recalled.

## Turn detectors (2026-10-10)

- **Done** on branch `claude/project-thread-fubdvs`: the turn detector
  `abilities/turn_detector/turn_detector.scx` (its seven events'
  schemas in `abilities/schemas/`), and the conversation's
  `turn_exchange`, which runs one turn per source of each input through it
  before memory and commits the turns' product. Decisions D76 to D79;
  conventions: [port-plan.md](port-plan.md#turn-detector-conventions).
- **Decided (D79):** the maintainer chose to say a multi-source text
  input once, so floppy joins each distinct turn text once (`hello`, not
  `hello hello hello`). The detector has no decoder
  (D77).

## Phase 6 (2026-10-10): speech and vision

- **Done on `main`** (`d569318`): `services/speech` (decode, encode, and the
  voice operations `detect`, `diarize`, `identify` with the media steps
  `normalize`, `append`, `clip`); the hearing leaves, sensitivity,
  interpretation (open-speech window included) and listening, which the
  body invokes as its input ability; the SpeechHeard seed in autonomy;
  vocal and speaking in audio mode; image parts in the generate service;
  vision and reading (invoked by nothing yet). Decisions
  D52, D57 to D59, D66, D67; D38 is superseded.
  Plan: [plans/phase6-speech-vision.mdscript.md](plans/phase6-speech-vision.mdscript.md);
  conventions: [port-plan.md](port-plan.md#phase-6-conventions).
- **Running:** the floppy bot's run also grants the speech service the
  media directory (`--dir https://github.com/stateforward/floppy/services/speech=HOST::/media`)
  and needs `FLOPPY_SPEECH_BASE_URL`/`FLOPPY_SPEECH_API_KEY` (speaking,
  speech decoding) and `FLOPPY_VOICE_BASE_URL`/`FLOPPY_VOICE_API_KEY`
  (LocalAI: every sound goes through voice detection). Without them a
  response fails and every sound is `bot.ability.listening.failed`.
- **Not built:** devices (phone, LiveKit, audio hardware; out of scope)
  and the conversation's matching of embedding sources to earlier
  participants' voice centroids. The turn detectors are built (see
  above).
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
  Since D260 the default model is `gemma-4-26b-a4b` on api.willen.dev
  (`FLOPPY_BASE_URL=https://api.willen.dev/v1`), and reflection, revision
  and the builder use `sparks/qwen3.8-flash-next` on
  `FLOPPY_REFLECTION_BASE_URL` (CPA, `https://cpa.willen.dev/v1`).
  Before D260, floppy's gitignored `.env` set them to the user's two DGX Sparks:
  - **direct vLLM:** `FLOPPY_BASE_URL=http://192.168.2.89:8000/v1`, model
    `Qwen/Qwen3.8-Flash-Next` (then the builder's default), `FLOPPY_API_KEY` (a
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
  `abilities/schemas/`, the domain event names in
  `abilities/schemas/floppy-event-names.json`; documents reference them by
  `event:schema="schemas/<event>.json"`, never inline. A schema reference
  cannot contain `..`, so ability documents live flat as
  `abilities/<ability>.scx`. `tests/tests/contracts.rs` checks the schema
  set and every `.scx` under `abilities/` and `bots/floppy/`.

- Phase 4 body and thin cognition: `bots/floppy/floppy.scx` (the bot's
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
  the confidence tuner passed by `<param>` and returned in
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
  through the body to the conversation, whose text product (its whole
  history) comes back as a `bot.ability.cognition.input`
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
  `008cd04a`): the shared offer compaction (the model-facing schema
  and the 32 KiB bound) in
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
  thresholdlocation observe>`: reasoning's strict dispatch schema and
  intuition's confidence tuner (its defaults as Rust constants; the threshold is
  written as a probability) are namespace elements; the stage results are
  plain `<assign>`s. Reasoning's `onChoices` cond is gone: the generate
  service's strict-schema check already fails an off-choices reply as
  `schema-mismatch`. Intuition and reasoning now script only `isObject`
  (validators go with the event-schema slice), traces unchanged.

- `namespaces/bot` (floppy `242fb57a`, `d5fffb8d`), a new floppy
  namespace (`https://github.com/stateforward/floppy/namespaces/bot`,
  component `floppy_bot`, imports `types`, `telemetry`,
  `data-model-host`): `<bot:attention location focuslocation stimulus
  devices focus sources>` (the turn's attention bias) and
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
  direction event message sequence provenance?>` builds one committed message
  and its relationship key; it never holds the history (the extension
  boundary's values are bounded at 64 KiB on wasm), so the conversation
  appends with `history.concat([message])` and
  `relationships[participants]` (since 2026-10-10 the key is the
  relationship context reference). The communication builds its outbound record
  with it (the append's provenance now carries explicit `null` source,
  target, session_ref and turn_ref; the committed history is unchanged).
  `the_conversation_commits_each_message_into_its_history` pins the
  committed messages field by field (blessed on the old document). Both
  documents now script validators only (`isObject`, `identities`,
  `validX`, `isProduct`, ...).

Next: phase 4 (bot core and abilities), per
`~/.agents/projects/floppy/plans/20261008-phase4-bot-core.mdscript.md`.

## Build run decisions (user, 2026-10-08)

The remaining phases run as one ultracode run with these grants:

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
- **Browser slice 2:** after phase 7.
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
- **Live model:** the DGX Sparks only (direct vLLM); never hosted models
  (user, 2026-10-24).
- **Behaviors and routines (phase 5) use the builder:** a behavior or
  routine is an SCXML document `bots/builder` authors, validates, and
  hands back, and the bot then invokes it.
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
   - A row's `source` column holds the path (no schema
     migration); routines tell a revision by `updated_at` (second
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

## Open work

What is still to build, each item with its issue where one exists.

- **The recall budget on wasm (D117).** `<cognition:model_facing_content>`
  trims `prior_episodes` and `standing_directives` to the prompt budget,
  newest first, but on wasm the data model's 64 KiB value bound refuses a
  prompt before it is big enough to trim, so the trimming is proven only
  natively (`namespaces/cognition/src/tests.rs`). Lower the budget, or
  raise the bound in scxml.rs.
- **Learning** (#14, #16): drafts staged before they are installed, an
  end-to-end test of a learned behavior on a later real turn, examples
  and byte budgets for reflection's select and builder prompts, and a
  builder that works with small models (#14); reflection selecting
  `bot.ability.persona.revise` (#16). The Revision document (D148) and
  the Learning ability (D149) are in review. A change does not yet keep
  the behavior's own triggers as evidence: `evidenceStimulusNames()` in
  `abilities/reflection/reflection.js` reads the turn's stimulus and the
  prior episodes, not `existing_behavior.triggers` (#14, L6).
- **Communication's rings** (#29, D175): the response and utterance
  senders name their send ids, so the rings keyed on `_event.sendid`
  catch a repeat.
- **Interpretation reads `producing`** (#30, D177).
- **Streaming speech** (S5 step B; step A, #18, is D179): streaming
  encoding and per-chunk playout are built (D202, D203, #55): the speech
  service returns `bot.ability.vocal.speech.encoding.chunk`s as its
  answers arrive and speaking plays each as it comes. Still open: drafts
  spoken while the reply is generated with an `abandoned` outcome (they
  wait until something produces them, D202; how a draft is correlated
  with its final respond, since an SCXML `<send>` takes no id from an
  expression, and the draft timeout; drafts are D262), a live run against
  a streaming backend (none has been measured; the cloud proxy does not
  carry WebSocket upgrades, so measure on a workstation with
  `scripts/talk.py`),
  and a `spoken` that knows the reply's total length while it still
  streams (D203 counts the share of the audio received).
- **Streamed hearing** (D270): the transcription streams scored sounds
  to the speech service's `stream` session and hands each turn with words
  to the interpretation through the listening. Still open: the overlap
  fact (the session's `overlap` is dropped), one session per sound
  source, and speaking over the backend's `/v1/audio/speech` WebSocket.
- **Voicing**: voice catalogs, voice design and the bot's own voice;
  `bot.ability.speaking.output`'s `voice_id` is null until then (D179).
- **Graph memory and embeddings** ([plan](plans/graph-memory.md)):
  episodes as nodes with vectors, associative recall,
  `bot.ability.memory.recall`, an embedding ability for text and images
  over a remote embedder, and consolidation while the cognition rests.
  People are D194, their voices built by D204, and consolidation D195,
  built by D205 (#49); the schema is one revision (D193). Matching reads
  one compact representative per person (D205), which holds about 100
  people at 256 dimensions (the memory ability's output event):
  an embedding index is where that bound goes. Consolidation into
  episodes and links waits on graph memory.
- **Hearing identity**: overlap while speaking, tone, and enrolling the
  bot's own voice. People in long-term memory (D194) are built for voices
  by D204 (#49): the conversation matches each voice embedding to a
  `person-<n>` memory holds (cosine 0.85, margin 0.05) or makes a new one,
  and stores every voice heard for its person; at rest (D205) each
  person's voices fold into their representative and duplicate people
  merge. Still open on #49: faces (below) and `voice_id` (D179).
- **Multimodal turns**: `<conversation:ingest_content>` takes no
  `image/*` content, so an image adds nothing to a turn and a turn of
  images alone fails as empty (`Turn closed with neither text, audio, nor
  modality-neutral content.`). Open: images attached to a completed turn
  (a bound on their count, a failure past it, the conversation's
  attachments and how long it keeps them), a typed image value, image
  placeholders and content parts in a turn's content, and a typed
  `environment.visual` image.
- **Faces** (D188): face detection and face identity in seeing follow
  D194's voices, built by D204 (#49; `bot_person_embedding`'s `kind`
  `face` is the seam); an embedding change measure and a cascade of measures wait
  on embeddings.
- **Sandbox, devices and transport** (D200, #53): a sandbox ability over
  a Docker or Podman host; computer, HID keyboard and pointer, audio
  microphone and speaker, and phone devices, with owner-command routing;
  transform codecs, transport peers, yamux over WebSocket and a LiveKit
  room. The tests use the lamp fixture.
- **Example bots**: a building example (the name freed by D198), and minimal,
  listen-and-speak, phone and SMS chat bots, each after its abilities.
- **Telemetry metrics**: no metrics are emitted. A voice embedding is
  never written to the records (`<biometric:VoiceEmbedding>`,
  `environment.rs` `a_voice_embedding_is_never_written_to_the_records`).
- **Logic still in JavaScript that a namespace or a schema could hold:**
  - A routine's `at` schedule: `next_after` is JavaScript text in
    `reflection.js` that each routine carries as data. A
    `<behavior:next_after>` element waits on scxml.rs: `<scx:validate>`
    binds no extension package, so a draft that uses one fails with
    `XSD001` (D116).
  - Reflection's evidence and grounded triggers (`evidenceStimulusNames`,
    `evidenceTriggerNames`, `groundedTriggers`, `acceptedTriggers`) as a
    `<reflection:grounded_triggers>` element.
  - Interpretation's `speechProduct` as a `<send>` with `<param>`s, its
    `voice_embedding` behind an `<if>`.
  - Interpretation's segment checks (`isSegmentSourceMismatch`,
    `isSegmentRateMismatch`, `isSegmentFormatMismatch`,
    `segmentSourceMismatch`, `segmentConversionFailure`), which compare
    one field with another, in a `listening` namespace (its name open);
    sensitivity's `receivedLevel`, `observedGain` and `perceivedLevel`
    may join it.
  - The body's `environmentField()` (`bots/floppy/floppy.js`) as an
    `<environment:model_snapshot>` element.
  - `<migration:plan>` writing the version guard and the ordered
    statements (`versionGuard`, `appliedStatements` in `migration.js`;
    `canRetry` stays).
  - `builder.request`'s optional bounds (`isOptionalBound`,
    `requestBound`, `requestConstraints`, `requestMaxAttempts`,
    `requestModel`, `requestGenerateTimeoutMs`,
    `requestGenerateCeilingMs`, `isOptionalGenerateBound` in
    `builder.js`) in `bots/builder/schemas/builder.request.json` (D42).
- **Session-id targets and the local model.** The dispatch tool's
  `target` enum lists session ids (D113). Live on 2026-10-18,
  `Qwen/Qwen3.8-Flash-Next` put `bot.focus_device`'s `device` beside
  `event` instead of in `data`; with readable keys, or no `target` on
  that branch, it filled the selection correctly (docs/live-proof.md).
  Offering readable keys and sending to their sessions is open.

## Design decisions for the maintainer

All answered on 2026-10-23: decision 2 by D199, 3 by D193, 4 by D198, 5 by D200, 6 by D194, 7 by D195, 8 by D196 (built by D206), 9 by D197. D192, D201 and D202 were decided with them.

## Choices to review

Choices the namespace steps (D113 to D116) and the decision kinds (D120)
made where the plan left a question open, each for the maintainer to
review. Q9 (a document with no functions drops its `.js`, amending D65)
was taken on the recommendation and is not exercised: every document
still has functions. Two names are open: `<processing:model_facing_schema>`
(or `<processing:schema>`) and `<conversation:message_for_turn>`;
Q11 is closed: D243 removed `<decision:pin>`.

**D206, effort ceilings:**

1. An action's effort is declared by its offerer (`<decision:choice
   effort>`), not by its event schema or the bot's bounds; undeclared is 0.
2. The weights: `bot.ability.identity.adopt` 1, `bot.ability.persona.revise`
   2, everything else 0; the ceilings: the decision layer 0, intuition 1.
3. The decision layer's `<held>` block goes into the state its decision
   reads (before the stimulus), so the decision service's request grows by
   the held descriptions.
4. An event offered by two sessions, one within the ceiling and one over,
   is offered (from the one within) and not held.

**D113, the processing namespace and every stage's sends:**

1. The ignore is tagged with the cognition's session id, so every stage
   sends it to the cognition, whose transitions log it.
2. The body sends itself (`#_internal`) each selection whose `target` is
   not a string (`hasNoTarget` in `floppy.js`): the trusted seed's
   selection names none.
3. `<processing:collect_offered_events>` takes the focused device's
   events from `devices` and `focus`, tagged with its snapshot's `id` and
   sorted by its reference among the children's invoke ids.
4. Reflection's trailing self-send is `bot.ability.reflection.selected`;
   the candidates runner raises
   `bot.ability.autonomy.candidate.dispatch.succeeded` after its sends,
   and an `error.*` before it fails the candidate (`Autonomy behavior
   dispatch failed: <error>`).
5. Intuition sends a selection no single session offers nowhere: the
   decision service names a target only for a single offerer (D150).
6. `<processing:selection_confidence>` reads a number in (0, 1] as a
   probability; the data model cannot tell 1 from 1.0, so 1 is certainty
   (100).
7. Autonomy and the candidates runner take no `choices`; the turn
   behavior contract asks a behavior to name each selection's `target`.
8. The body's `bot.focus_device` guard holds whenever the turn has focus
   candidates.
9. The turn's output is the handling stage's selections; intuition sends
   its environment actions itself, and they are not in the output.
10. `<log expr="choices"/>` logs whole choices; tests read their names
    with `support::choice_names`.
11. A turn may focus the lamp twice (intuition's environment action, then
    reasoning's selection), and the body sets the focus each time;
    `live_choices` expects the lamp focused at least once.

**D114, the intuition and cognition namespaces:**

1. A budget's message goes to `failurelocation` (reasoning's
   `prompt_failure`) and `null` to `location`; without
   `failurelocation` it is `error.execution`.
2. The element's inputs are `stimulus`, `prior_episodes`,
   `standing_directives`, `environment`, `instructions` and `schema` (the
   parameters `<processing:send>` built).
3. Object members render in sorted order, the data model's canonical
   JSON.
4. Each input is bounded by the 64 KiB value bound; one over it fails
   reasoning as `the reasoning could not run (error.execution)`.
5. A null confidence never escalates.
6. Intuition logs `turn N: <events> (confidence C, escalate B)`.
7. The tuner datum, `<param>` and donedata field are `confidence_tuner`;
   the log label `cognition.tuner` is kept.
8. A tuner state that is not one reads as the initial state.
9. The environment block is part of `<cognition:model_facing_content>`;
   the body's `environmentField()` stays in `floppy.js` (open work).
10. Malformed input (a `self` or a device's `attributes` that is a
    string) renders nothing.

**D115, the identity, conversation and memory namespaces:**

1. `<conversation:message_for_turn>` has no `keylocation`;
   `<conversation:relationship_for_input>` works out the relationship's
   context reference.
2. `identities_json` is part of `<memory:to_json>`, its only caller.
3. `<memory:contents location result>` takes one statement's result,
   parses each content as a JSON object and skips the rest.
4. `<conversation:participant_for_source>` adds a participant on first
   sight and `<conversation:fail_turn>` drops the one whose detector
   failed; a failed remember restores the participants from the
   `participants` datum, and an inbound commit appends its sequence with
   `<assign>`.
5. `isCommittedAppend` stays in `conversation.js`: it searches the
   history, which never crosses the extension boundary.
6. The turn detector's elements are `<conversation:ingest_content>` and
   `<conversation:on_end>`, over one `turn_content` datum.
7. `<conversation:new_track_ref>` runs in `turn_exchange`'s entry and
   writes the unit vector as `JSON.stringify` does.
8. The similarity threshold and margin are Rust constants
   (`SIMILARITY_THRESHOLD` 0.85, `SIMILARITY_MARGIN` 0.05).
9. An identity may be an embedding (an array of numbers).
10. The relationships cross the extension boundary (64 KiB per value on
    wasm), so a long conversation with many embedding participants can
    fail an input as `error.execution`.
11. `<conversation:complete_turn>` takes the relationship's
    `session_ref` and commits the product itself.
12. `<memory:to_json>` writes `null` for an input without content, and
    `<memory:contents>` skips a row with a lone surrogate escape or a
    number out of range.

**D116, the behavior and autonomy namespaces:**

1. The stimulus envelope is assigned member by member and sent with
   `<param name="stimulus">`; a `bot.input`'s source is `null`.
2. `<behavior:instances_from_behavior_results>` takes two statements'
   results (`behaviors`, `triggers`); the first instance is a `prepared`
   datum (`preparedBehavior()`).
3. `<behavior:replace_behavior_clauses>` owns its SQL as Rust constants;
   a create's draft is stored by it too (delete, delete, insert `DRAFT`,
   then its triggers).
4. A document is persistent when it has a `<send
   event="bot.behavior.tick">`, read as XML (`lifetimelocation`).
5. `<behavior:lifetime_errors>` takes `min_every` and `turn_after_bound`
   in milliseconds and `scheduleslocation`; a draft that does not parse
   has no diagnostics, and the validator's report decides it.
6. `<behavior:verify_tick>` checks the forced tick; comparing a test's
   outputs with its expectations stays in `builder.js`, gated by
   `isTickTest()`.
7. `<autonomy:behavior_input_payload>` always sets `focus` and
   `focus_candidates` (`null` and `[]` included); array data is
   `{event: data}`.
8. Element outputs are canonical JSON, so behaviors in reflection's
   select prompt and the turn contract's `INPUT` render with sorted keys.

**D120, the decision service's kinds** (superseded by D243: one kind,
`choice`):

1. `predicate` is one kind, not one predicate per choice, so each
   event's confidence is still its share of one `selection` question and
   intuition can abstain as `declined`.
2. `choice` takes `choices` (the chosen event) or `labels`
   (`decision.classified`), exactly one, each with its own default
   instructions.
3. The service's events keep their names.
4. The batch's question names follow the kinds for `score` and for
   `choice` over labels.
5. The service skips the handoff by name, `bot.ability.reasoning.input`,
   for `choice` over events and for `predicate`; `isDeliberativeHandoff`
   stays in `intuition.js` for `hasDeliberativeInput`.
6. `stimulus` reads only `event` and `data`; its other members are
   ignored.
7. `services/decision` has no README: its contract is the crate
   documentation in `src/lib.rs`.

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
- Website (after phase 7): an animated robot face, web components + hsm.ts, marketplace of packages, a Mindstorms-style
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
