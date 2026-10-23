# Event schemas (`abilities/schemas/`, `bots/*/schemas/`, builder `<name>.json`)

Each file is the one JSON Schema of one event, referenced from documents with
`event:schema`. scxml.rs checks it where the event is created, on sends between
sessions and on host input (D41). Review against these rules and cite them.
Rules marked *(contracts.rs)* are enforced by `tests/tests/contracts.rs`.

- **One file per event** (D125): ability events in `abilities/schemas/`, body
  events in `bots/floppy/schemas/`, builder contracts in
  `bots/builder/schemas/`. No copies or twins; no file name twice under
  `abilities/`, `bots/`, `environments/`, `devices/`, `services/`
  *(contracts.rs)*.
- `"$schema": "https://json-schema.org/draft/2020-12/schema"`; `title` equals
  the file name without `.json`; every `examples` entry satisfies the schema
  *(contracts.rs)*.
- Adding or removing a schema file updates `BODY_EVENTS` / `ABILITY_EVENTS`
  (and `CORRELATED` when relevant) in contracts.rs and the name list
  `abilities/schemas/floppy-event-names.json` (sorted, unique) in the same
  change. A name floppy adds is either in that list or marked `[floppy name]`
  at the start of `description`, never both *(contracts.rs)*.
- Correlated, non-tool events of a turn require `turn` (`integer`,
  `minimum: 1`); tool events, whose data a model fills, must not (D36).
- Never carry runtime-only fields: `operation_id`, `generation`,
  `parent_operation_id`, `parent_generation`, `token`, `request_id`,
  `actor_id`, `capability` (D36) *(contracts.rs)*.
- Media is a reference (`format: uri-reference`); no `format: base64url`
  (D36) *(contracts.rs)*.
- No cross-file `$ref` (scxml.rs refuses it): shared shapes are inlined, and
  every participant item is the one `{ref, role, own, resembles}` Participant,
  identical everywhere (D157) *(contracts.rs)*. A recursive "any JSON" is `{}`.
- Fields floppy adds are marked `[floppy field]` in their `description`;
  floppy-only enum values and checks `[floppy value]` / `[floppy check]`.
- Tool events keep model-facing `description` prose, titles and examples.
  Prefer `additionalProperties: false` on objects; generate drops only
  undeclared top-level fields, nested objects stay strict (D112).
- **Describe the contract, not a serializer.** Constrain values the way the
  behavior needs (`type: number`, `format: date-time`); do not encode one
  language's formatting quirks (forcing floats to carry `.0`, Python datetime
  string patterns) into a schema.
- Schema descriptions are new text like any other: they do not cite another
  implementation ("as the original", "the reference's model"); state what the
  field means.
- Builder output `<name>.json` is an object whose `$defs` is keyed by input
  event and referenced as `<name>.json#/$defs/<event>` (D121).
- The body's choices and offers are held to 32 KiB (`bound="32768"`, D35);
  flag changes that inflate tool schemas noticeably.

## Rules for every file

floppy is a bot written entirely as SCXML documents, plus the Wasm packages
(extension namespaces under `namespaces/`, invoked services under
`services/`) those documents use. It has no runtime of its own: every bot runs
under scxml.rs's `scx run`, and the scxml.rs engine selects transitions and
runs macrosteps. `AGENTS.md` is normative (RFC 2119 MUST/SHOULD/MAY);
`docs/decisions.md` records each decision as `D<n>` and is cited by number.
Cite the AGENTS.md rule or decision a finding rests on.

- **No edits under `scxml.rs/`.** It is a squashed subtree; a change there goes
  upstream first and arrives only through `git subtree pull --squash`. Any
  hunk under `scxml.rs/` in an ordinary pull request is a defect.
- **No host code.** floppy adds no embedder, session loop, config format, or
  other runtime code; a capability a bot needs from the runtime is added to
  scxml.rs upstream. Code that selects transitions or mimics engine semantics
  is a defect.
- **Hard cutovers.** Pre-1.0, a replacement removes the old path, its tests,
  configuration and documentation in the same change. Flag compatibility
  shims, legacy fallbacks, "deprecated" aliases, or both paths left in place.
- **Idiomatic behavior, not ported artifacts.** floppy matches the behavior
  it ports, written idiomatically for SCXML, ECMAScript and Rust. Flag code
  that reproduces another language's implementation details instead of the
  behavior: mirroring Python or pydantic float formatting (`1.0` vs `1`),
  number-to-string shims that imitate `repr`, datetime formatting copied
  from a Python serializer when the schema only needs a valid timestamp, or
  helper names and structure carried over from Python. In ECMAScript, numbers
  are plain JS numbers. Python *tooling* (scripts under `scripts/`, test
  helpers, generators) is fine and is not a finding.
- **Wording in new text.** New code, comments, docs, schema descriptions,
  test names and commit-visible text MUST NOT name the upstream Python
  project the bot was ported from (the word `mosfet`, its module paths such as
  `src/mosfet/`) and MUST NOT describe behavior as "the original's" or "the
  reference's" ("as the original does", "the reference implementation",
  "matches the original"). State the behavior itself, and cite a decision
  (`D<n>`) for why. Only flag lines the diff adds or changes; existing text
  elsewhere is out of scope.
- **Terminology.** Use the repository's terms: *bot* (an SCXML document under
  `bots/<name>/`), *body* (the bot root, which only composes), *ability* (an
  invoked child document under `abilities/`), *environment* (the root session
  a bot runs in, `environments/<name>/`), *device* (a document an environment
  invokes, `devices/<name>/`), *namespace* (an executable-content extension
  package), *service* (an invoked-service package), *package* (a namespace or
  service with its manifest), *event contract* / *event schema* (an
  `event:schema` JSON Schema file). Domain event names (`bot.*`,
  `bot.ability.*`, `phone.*`) are kept byte for byte; flag a renamed or
  re-cased domain event.
- **Naming.** Concise domain names; no redundant `-component` suffix on
  crates, worlds or artifacts. Package identities are the exact tagless URIs
  `https://github.com/stateforward/floppy/namespaces/<name>` and
  `https://github.com/stateforward/floppy/services/<name>`.
- **Secrets.** Secret values are never printed, logged, formatted into
  errors, committed, or written into commands or documents
  (`--env NAME=VALUE`). Keys reach a run only through `scx --env-file`, and
  only `env`-slotted elements and `<invoke>`s receive them.
- **Tooling.** Never the `find` command in scripts or agent workflows; use
  `rg` and `ls`.
- **Scope.** A change is narrowly scoped and backed by a test matching its
  exact claim. Flag behavior changes with no test, and unrelated edits mixed
  into a change.
