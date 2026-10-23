# Docs and READMEs (`**/*.md`, `.env.example`)

Review documentation against these rules and cite them. Only lines the diff
adds or changes are in scope; dated history is not rewritten.

## READMEs

- A bot's README gives the exact run command: `scx run --env-file <file> …`
  on `environments/<name>/<name>.scx` (or `bots/<name>/<name>.scx` for a root
  bot such as the builder) with `--fuel 9223372036854775807` for environment
  runs, the dev form `--offline --cache-dir target/components/package-cache`,
  and every grant its services' imports need (`--dir …/services/generate=HOST::/media`,
  speech and vision the same, `…/services/sql=HOST::/memory`,
  `--wasi …/services/pgsql=inherit-network`, `…/services/files=HOST::/behaviors`).
  A change that adds a service, import or grant updates every command.
- Commands never carry secret values (`--env NAME=VALUE`).
- A namespace README leads with its URI, prefix and component name, has an
  element table, lists the component's imports, and has Statements anchors
  matching `irp/manifest.xml`. A service README documents grants, events and
  backends.
- Root README Defaults: every `*_timeout_ms` `<data>` has a row, Variables
  matches `.env.example` (names only, one comment each), Model names
  `Qwen/Qwen3.8-Flash-Next`.

## Decision record and handoff

- `docs/decisions.md` rows are `| D<n> | YYYY-MM-DD | text |`. A change that
  departs from a decision adds a row or marks the old one "(Amended by D<m>…)"
  / "(Superseded by D<m>.)"; old rows are not silently rewritten. Rows cite
  the maintainer's words, the proving tests and upstream scxml.rs commits.
- After a subtree pull, the scxml.rs subtree table records the new source
  commit and squash commit.
- `docs/handoff.md` updates "Last updated" and its current-state bullets with
  significant work; `docs/port-plan.md` maps stay in sync with layout or
  convention changes. Docs, tests, schemas and artifacts change together.
- Dated records (`docs/overnight-*.md`, dated `docs/live-proof.md` entries) are
  not rewritten.

## AGENTS.md

- AGENTS.md is normative (RFC 2119). Its namespace list matches `namespaces/`.
  Rule changes come from the maintainer's direction and keep MUST/SHOULD/MAY
  wording precise.

## Wording

- New text describes floppy's behavior in its own terms and cites `D<n>` for
  why. Flag added lines that name the upstream Python project or describe
  behavior as "the original's", "the reference's", "as the original does" or
  "the reference implementation".
- Use the repository's terms (bot, body, ability, environment, device,
  namespace, service, package, event schema, turn, stimulus, percept,
  participant, grant, slot, origin) and mark floppy-only names and fields
  `[floppy name]` / `[floppy field]`.
- Claims are backed by evidence: name the test or live-proof entry; record
  macOS-only baselines as open; no performance claims without measurement.
- Never suggest the `find` command in instructions; use `rg` and `ls`.

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
