# Overnight report, 2026-10-06

This covers the overnight port run: scxml.rs invoked services (slice 2), the
nemobot scaffold, the three namespaces, and the builder bot. Every commit is
local. Nothing was pushed, no remote exists, and no PR was opened. The proof
section says which results I re-ran this morning and which come only from
the workers' own reports.

## Bottom line

- **scxml.rs `feat/invoke-services`** (worktree `scxml.rs-services`) is at
  `2bf0dd55`, 13 commits past `main`. Slice 2 is done for invoked services:
  WASI permissions and grants, event contracts, env slots on `<invoke>`,
  and repository services. **Extension components still cannot get WASI.**
  The host refuses an extension package that declares a WASI permission, so
  decision D4 is still open for extensions.
- **nemobot `main`** has 39 commits. It contains the subtree, the workspace,
  `nemobot-host`, the namespaces `scxml` (`sx:validate`), `generate`
  (`gen:generate`) and `decision` (`rank`/`classify`/`choose`), and the
  builder bot (`bots/builder/builder.scxml`). The subtree at `scxml.rs/` is
  byte-identical to `2bf0dd55`: both trees hash to `1f4bf477`.
- **Deterministic proof is green in both repositories** (re-run this
  morning, see below).
- **Live:** one live `gen:generate` call and one live `decision:choose` call
  passed, both before review round 1. **No live build of the current
  builder has passed.** The only passing live build is
  `bots/builder/examples/traffic_light`, made by the first builder
  (`89ec4d05`), before the round-1 fixes.

## What was built, by phase

### Phase 0a: scxml.rs slice 2 (`feat/invoke-services`)

Slice 1 (`c0cbd9b5`, `6470c5b5`) was already done. Slice 2 is split below by
topic.

**WASI permissions (slice 2a).**
- Manifests gain an optional `wasi-permissions` field from a closed
  vocabulary: `wasi:clocks`, `wasi:filesystem` (mount, read-only or
  read-write), `wasi:random`, `wasi:sockets/ip-name-lookup`, and
  `wasi:sockets/tcp` (outbound connect allowlist only).
- The CLI grants them with `--grant URI=PERMISSION[=VALUE]` and lists them
  (`= WASI:`) before running.
- Each service gets its own linker holding only what it declares and the run
  grants. Undeclared imports fail at session start.
- A run with any declared WASI permission refuses `--record`/`--replay`.
- The example service is `components/scxml-service-files`.
- AGENTS.md now reads "no WASI interface beyond what the manifest declares
  and the run grants".

**Event contracts (slice 2b).**
- Packages can declare `events.emits` and `events.accepts` with JSON
  Schemas, pinned by SHA-256.
- The host enforces them for services:
  - A refused emit retires the service, and the parent gets
    `error.communication`.
  - A refused `<send>` to `#_id` raises `error.execution`.
- New validator codes: SCXML095 (schema differs) and SCXML096 (schema
  missing). Document `event:schema` stays metadata-only.

**Env slots on `<invoke>`.** A service gets opaque references that stay valid
for the whole invocation (WIT `service-invocation.env`, `InvokeIr.env`). WIT
stays `@0.2.0` and Machine IR stays 5.

**Repository services.** `https://github.com/<owner>/<repo>/services/<name>@<tag>`
URIs resolve like repository namespaces: a pinned cache
(`stateforward.scxml.service-cache/v1`), `--offline`, authentication through
`SHA256SUMS`, and fail closed.

**Review fixes.**
- One service export call may emit at most 1024 events and 4 MiB.
- The run log binds the service components, so a replay with a different
  service is refused.
- `ExtensionEnvironment`'s Debug output prints variable names only.

### Phase 0b: nemobot scaffold

- **Subtree.** A squashed subtree of `feat/invoke-services`, pulled three
  times: `9971c05a`, then `fe7d555d`, `dd18cd11` and `2bf0dd55`.
- **Workspace.** A Cargo workspace (Rust 1.98, edition 2024) that excludes
  the subtree and uses its crates by path.
- **`nemobot-host`.**
  - Reads a strict JSON config (`stateforward.nemobot.config/v1`).
  - Reads values only from a granted env file. `env-defaults` gives
    non-secret defaults.
  - Binds and authenticates extension and service packages and checks
    their WASI grants.
  - Builds the consent listing.
  - Compiles documents and runs live sessions on threads.
- **Scripts.** `scripts/build-components.sh` builds and verifies components
  reproducibly.
- **Docs.** `AGENTS.md`, `docs/port-plan.md`, and `docs/decisions.md`
  (D1-D12).

### Phase 1: namespaces

All three are extension components with exact import allowlists and no WASI
left after wasi-virt.

**`scxml` (`sx:validate`)**
- Wraps the scxml.rs validator.
- Writes the canonical scxml.report/v3 report to `location` and can apply
  machine-applicable fixes into `fixedlocation`.
- Its package must be bound with `extension-call-fuel` (1B is documented),
  because one call takes about 200M instructions.

**`generate` (`gen:generate`)**
- Calls Chat Completions (`POST {base}/chat/completions`) with a canonical
  JSON body.
- Assigns structured output only after it matches the schema.
- Takes `env:apikey` and `env:baseurl`. Since round 1, the base URL is
  required and no URL is compiled in.

**`decision` (`rank`, `classify`, `choose`)**
- The TypeSafe `system_one` backend sits behind a `Backend` trait.
- `choose` builds its menu from the session snapshot: the `event:schema`
  transitions whose guards hold.
- It fills required top-level members from grounded options and validates
  the data before raising the event. Otherwise it raises `decision.abstain`.

### Phase 2: builder bot

`bots/builder/builder.scxml` runs these steps:

1. Intake.
2. Design and author through `gen:generate`.
3. `sx:validate`, with at most 2 machine-fix passes.
4. Simulate the draft as a child via `#_candidate`, one invoke id per test.
5. Package and send `builder.done`, or send `builder.failed`.

Retries are bounded by a retry gate and `max_attempts`. Triage uses
`decision:choose` (`triage.repair`, `triage.redesign`, `triage.abandon`),
with a cond-based fallback.

**Hardening from the review rounds.**
- A draft contract refuses drafts that use foreign namespaces, `invoke`,
  `script`, `src`, `srcexpr` or `typeexpr`, or non-SCXML send types. It
  also handles non-ASCII prefixes and entities.
- Simulation control events carry their run's tag, so a stale `sim.drive`,
  `sim.settle` or `builder.timeout` never counts.
- A redesign sends the previous plan.

**Harness.** `host/tests/builder_bot.rs` runs against fake OpenAI and
TypeSafe servers on loopback.

**Not built yet:** the CLI around `nemobot-host`. Phases 3 to 7 are not
started.

## Commits

### scxml.rs-services: `git log --oneline main..HEAD`

```
2bf0dd55 Print only variable names in ExtensionEnvironment's Debug and state the extension WASI refusal in AGENTS.md
dd18cd11 Bind invoked service components in the run log identity so a replay with a different service is refused
fe7d555d Bound the events one invoked-service export call may emit by count and bytes
9971c05a Resolve repository services from services/<name>@<tag> like repository namespaces
5025352c Enforce service event contracts in the host and cross-check them in the validator
84e34829 Bind env slots on invoked services' <invoke> for the whole invocation
c8eef819 Declare event contracts with JSON Schemas in extension and service packages
ddbe3faa Allow WASI interfaces a package declares and the run grants
440a25d1 Grant WASI permissions from the CLI and list them before running
bbfe3e91 Link only declared and granted WASI interfaces into service components
ec0cbcea Declare WASI permissions in extension and service package manifests
6470c5b5 Document invoked services, their diagnostics, and the service package
c0cbd9b5 Run custom <invoke type> services as Wasm components
```

### nemobot: `git log --oneline`

This report's own commit comes on top of these.

```
6ad78cc3 Record the scxml.rs subtree at 2bf0dd55
6dedcbe2 Pull scxml.rs feat/invoke-services at 2bf0dd55 (ExtensionEnvironment Debug names only; extension WASI refusal in AGENTS.md)
0a20ad2b Squashed 'scxml.rs/' changes from dd18cd11..2bf0dd55
afa81374 State that an extension raising document-defined events declares only its own event contracts (decision:choose)
72f40cec Tag the builder's simulation control events with their run so a stale sim.drive, sim.settle, or builder.timeout never counts; mark the unrecorded live runs unproven
65c581a4 Say that the saved live example predates the round-1 builder and that no live build of the current builder has passed
fe9a0315 Record the scxml.rs subtree at dd18cd11 and keep D4's extension WASI item open
5438f010 Pull scxml.rs feat/invoke-services at dd18cd11 (run log binds service components)
070aa258 Squashed 'scxml.rs/' changes from fe7d555d..dd18cd11
387af206 Run invoked services through nemobot-host sessions: a contracted echo's emits and a granted files service
34975fb1 Move the builder mock's routing into a function to satisfy clippy
bd569660 Harden the builder's draft contract against non-ASCII prefixes and entities, guard its control events by invoke id, and keep the replaced plan across a failed redesign
af90b0be Build the consent listing from formatted lines instead of discarding writeln! results
fe50ef47 Satisfy clippy in the decision event-name bound test
45acb3cc Declare the decision event-name bound through wasm_bound and preempt menu transitions per atomic state's chain
1753822f Bring AGENTS.md and the docs in line with the code after review round 1
e43c208b Remove the namespace package .gitkeep placeholders now that each package directory tracks real files
718c57fe Test service package binding, WASI grants, and the consent listing in nemobot-host
8f538d51 Split the builder repair test's request checks into a helper to satisfy clippy
52f1be06 Fix the builder bot's round-1 review findings
a02352d1 Offer only transitions whose guards hold in decision:choose's snapshot menu
c7b873a2 Compile no backend URL into generate or decision; require env:baseurl
5be41b24 Squashed 'scxml.rs/' changes from 9971c05a..fe7d555d
5f845788 Pull scxml.rs feat/invoke-services: per-call bounds on invoked-service emits
89ec4d05 Implement the builder bot as bots/builder/builder.scxml with its event contracts and harness
49de8a7d Implement the decision namespace's rank, classify, and choose as an extension component
d2a02e61 Implement the generate namespace's <gen:generate> as an extension component
df49b6d2 Implement the scxml namespace's <sx:validate> as an extension component
4b0bd468 Let a nemobot config set the instruction fuel of one extension component call
da7c41cf Document the mosfet to nemobot port plan, the decisions, and the builder bot's contract
f6892e47 Adopt scxml.rs's component rules and the port decisions as nemobot's AGENTS.md
a5c59895 Build and verify components with scripts/build-components.sh and smoke-test nemobot-host on real sessions
8b9e4045 Add nemobot-host: config, env-file values, package binding, compilation, and the live session thread
a8894608 Add the Cargo workspace and the scxml, generate, and decision namespace skeletons
353ce1a5 Squashed 'scxml.rs/' content from commit 9971c05
df96ba46 Merge commit '353ce1a582fbca4418a21d8c2d6ad7982ccde00a' as 'scxml.rs'
b7c6aef8 Start the nemobot repository
```

## Proof

### Re-run this morning on HEAD, all passing

nemobot (`6ad78cc3`):

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass |
| `cargo test --workspace --locked` | pass. Unit tests: decision 25, generate 19, host 12, scxml 17. Host integration tests: `builder_bot` 14 (+1 ignored live), `decision_namespace` 5 (+1 ignored), `generate_namespace` 5 (+1 ignored), `scxml_namespace` 2, `service_packages` 7, `smoke` 3. The test harness runs `scripts/build-components.sh`, and every component verified |
| `scxml-cli validate bots/builder/builder.scxml --offline` with the three nemobot extension packages | 0 errors, 0 warnings, 0 notes |
| Subtree identity: `git rev-parse HEAD:scxml.rs` against `2bf0dd55^{tree}` | equal (`1f4bf477`) |

scxml.rs-services (`2bf0dd55`, clean tree):

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | pass |
| `cargo clippy -p scxml-host-wasmtime -p scxml-cli --all-features --all-targets --locked -- -D warnings` | pass |
| `cargo test -p scxml-host-wasmtime --features wasmtime,http --locked --test service_invoke` | 16/16 |
| `cargo test -p scxml-host-wasmtime --features wasmtime --lib service` | 2/2 (emit bounds) |
| `cargo test -p scxml-cli --features wasmtime --locked --test repository_services --test service_packages` | 4/4 and 5/5 |

### Reported by the workers, not re-run this morning

These are the slower checks.
- **Full scxml.rs workspace.** `cargo test --workspace --all-targets`
  passed: 1203 passed, 0 failed, at slice 2b. The wasm32-wasip2 clippy run
  over all guest crates, `cargo doc -D warnings`,
  `scripts/verify-wasm-capabilities.sh`,
  `scripts/verify-component-authored-unsafe.py`, and `scripts/test-wasm32.sh`
  all passed.
- **cargo deny.** Passed only with `--offline`, because the advisory fetch
  had no GitHub auth.
- **W3C IRP.** `--canonical-baseline`: ecmascript 201/201 and xpath 178/178,
  plus the env/event/openai namespace suites. Every disposition matched the
  baseline. The identity bytes were not regenerated, because they reproduce
  only on Linux x86_64 CI and slice-1/2 source changes move them.
- **Not re-run after the round-1 to round-3 scxml.rs fixes**
  (`fe7d555d`..`2bf0dd55`): the full scxml.rs matrix and the IRP runs.

## Live proof (real model calls, pass/fail only)

All live tests read keys only through an env file (`NEMOBOT_LIVE_ENV_FILE`).
No values were printed or committed. I did not re-run any live test this
morning.

| Test | Result | Note |
| --- | --- | --- |
| `generate_namespace live_generate_through_an_env_file` (gpt-4o-mini, strict structured output) | pass | Run before `c7b873a2` made `env:baseurl` required. Not re-run since |
| `decision_namespace live_choose_through_an_env_file` (TypeSafe `system_one`) | pass | Run before the guard and preemption changes (`a02352d1`, `45acb3cc`). Not re-run since |
| `builder_bot live_builder_through_an_env_file`, first builder `89ec4d05`, gpt-4.1 | pass (2 runs) | The second run is saved as `bots/builder/examples/traffic_light`. It has a known flaw: its phase timers are never cancelled |
| Same, first builder, gpt-4o-mini and gpt-4.1-mini | fail | Ran out of attempts |
| `live_builder_through_an_env_file`, current builder | fail / unproven | The session log reports two runs ending in `builder.failed`. The repository has no record of them, so their causes are unknown |

## Review rounds

| Round | Confirmed findings | Minor findings |
| --- | --- | --- |
| 1 | 12 | 11 |
| 2 | 4 | 6 |
| 3 | 2 | 5 |

- **Round 1.** The worker reports every finding fixed. The fixes are
  scxml.rs `fe7d555d` and nemobot `c7b873a2` through `1753822f`.
- **Rounds 2 and 3.** These were fixed in nemobot `45acb3cc` through
  `6ad78cc3`, and in scxml.rs `dd18cd11` and `2bf0dd55`.
- **No fourth round was run.** That means the round-3 fixes have not been
  reviewed independently, and I have no record that any finding was left
  open on purpose. Treat "zero unresolved" as unverified until a fresh
  review round runs.

## Deviations from the brief

**scxml.rs**
- The `wasi-permissions` manifest field is optional rather than required.
  Making it required would change the openai manifest digest that the IRP
  baseline binds.
- `wasi:http` is not in the WASI vocabulary. The existing outbound-http
  capability covers that.
- Record/replay is refused for any declared WASI permission, including
  clocks and random, not only effectful ones.
- `--grant` applies to services only.
- Two diagnostic codes instead of one: SCXML095 and SCXML096.
- AGENTS.md was amended for WASI, repository services, invocation-lifetime
  service references, and the extension WASI refusal.
- The echo example now uses outbound-http (an `endpoint` env slot), to prove
  invocation-lifetime references.
- scxml.rs AGENTS.md says AI agents commit only on the maintainer's explicit
  request. Local commits were made because the task asked for them.

**nemobot**
- The config is JSON rather than TOML.
- `generate` uses Chat Completions rather than the Responses API, so OpenAI,
  NVIDIA and local servers all work.
- `env:baseurl` is required, with `env-defaults` instead of a compiled-in
  URL.
- Config gained `extension-call-fuel`.
- The builder:
  - carries inline copies of its event schemas, which tests keep equal to
    `bots/builder/schemas/*.json`;
  - names the draft's invoke id `candidate` rather than `draft`;
  - accepts an extra optional `model` field in `builder.request`;
  - names its triage events `triage.*`.

## Gaps

**Extension components**
- They cannot use WASI (D4 is still open). The extension store data is not
  `Send`, and wasmtime-wasi requires it.
- Their declared event contracts are verified but not enforced at run time.

**Event contracts**
- Autoforwarded events are not checked against a service's `accepts`
  schema.
- The validator's contract check compares canonical schemas for exact
  identity, not subsumption.
- The validator does not check `<send>` data against `accepts`.

**Services**
- A service named only by `typeexpr` is checked at instantiation, and a
  repository service named that way is never resolved.
- No shipped component answers a repository service URI, so the end-to-end
  proof stops at the type-uri check.

**`sx:validate`**
- It binds no packages, so drafts that use `generate:` or `decision:`, or
  literal service types, are reported as errors.
- It does not syntax-check expressions.

**`decision:choose`**
- It fills only top-level required members.
- It skips transitions that use `event:schemaexpr`.
- It evaluates guards against the current event.

**`nemobot-host`**
- No CLI, so the consent listing is never shown to a user.
- No repository resolution and no record/replay.
- The env grant is one flat list for every bound package.
- It duplicates embedding code that is private to `scxml-cli`.

**Builder**
- A simulation passes when the expected outputs appear in order, so extra
  events are allowed.
- Only validator errors gate a draft; warnings are not fed back.
- Generation timeouts cannot fire mid-call. HTTP timeouts bound it instead.

**Releases**
- `v0.1.0` namespace tags are placeholders. No releases or `SHA256SUMS`
  exist yet.

## Decisions needed from you

1. **D4 for extensions.** Choose one:
   - approve a session-host refactor in scxml.rs that makes the extension
     store `Send`, so extensions can link WASI; or
   - narrow D4 to services for 0.2.0.
2. **Builder live budget.** Should the next live runs of the current builder
   record their outcome in the repository? Which model should be the target
   (gpt-4.1 worked, the mini models did not)? Or should we wait for NVIDIA?
3. **Slice 2a deviations.** Accept or reject each of these:
   - `wasi-permissions` optional rather than required;
   - no `wasi:http`;
   - record/replay refused for every WASI permission.
4. **Scoping env grants.** Should each granted variable be limited to the
   package URIs it is for, in the nemobot config?
5. **IRP identity bytes.** These must be regenerated on Linux x86_64 CI
   before `feat/invoke-services` can merge. When and how do you want that
   done (it needs a push or CI access, which I did not use)?
6. **Releasing the namespaces.** When should they get real tags and
   `SHA256SUMS`, so documents can bind them as repository namespaces?
7. **Upstream embedding API.** Should scxml.rs expose a public embedding API
   so `nemobot-host` stops duplicating `scxml-cli` internals?
