# Tests (`tests/**`, IRP suites under `**/irp/`)

The `tests/` crate builds the components and `scx`, and runs every bot,
namespace and service through `scx run` and `scx validate`. Review against
these rules and cite them.

- Tests drive scxml.rs's `scx` through `tests/tests/support`; they never embed
  a runtime or simulate engine semantics (D14).
- Each file starts with a `//!` doc of what it checks. Test names are
  descriptive snake_case sentences. `expect("<invariant>")` is allowed; avoid
  bare `unwrap()`.
- A test matches the exact claim of the change: a behavior change needs a test
  that fails without it. A decision that names its proving test must have that
  test assert the claim (AGENTS.md Engineering).
- Directory walking uses `std::fs::read_dir`, never the `find` command; scratch
  directories come from `support::scratch` / `unique_scratch`.
- Environment runs pass `--fuel 9223372036854775807` (`support::FUEL`, D111);
  grants come from the support helpers (`media_grant`, `memory_grant`,
  `behaviors_grant`, `pgsql_grant`, …). A layout in a scratch directory copies
  `abilities/schemas/` (and `bots/floppy/schemas/`, and `firmware/` for a body)
  beside the documents (D125, D160).
- Variables reach a run only through an env file or an explicit `--env` in a
  test; never commit real keys. Live tests are `#[ignore = "…"]` and read
  `FLOPPY_LIVE_ENV_FILE` and `FLOPPY_LIVE_MODEL`. Fake backends listen on
  `127.0.0.1`; no implicit network.
- Trace fixtures (`tests/fixtures/phase4/traces/*.ndjson`) are compared byte
  for byte; a change to them must follow from an intended behavior change in
  the same diff.
- IRP test documents (`irp/sources/<id>/test<id>.scxml`) are tests, not
  shipped documents: the `<name>/<name>.scx` layout and root-script rules do
  not apply to them, but each must test exactly the statement its manifest
  entry names and end in `pass` or `fail`. `cases.py` is Python tooling.
- Document IRP suites (`irp/README.md` Statements with `<a id>` anchors,
  `irp/manifest.xml`, `irp/sources/<id>/test<id>.scxml` ending in `pass` or
  `fail`) and namespace IRP suites keep statements, manifest and sources in
  step; a namespace change re-baselines with `scripts/namespace-irp.sh`.
- Changes to `tests/tests/contracts.rs` (`BODY_EVENTS`, `ABILITY_EVENTS`,
  `CORRELATED`, `RUNTIME_ONLY`, `MAX_EXPRESSION_CHARS`, the floppy-name set)
  must match a schema or decision change in the same diff; loosening a check
  needs a decision. Never skip, disable or weaken a test to get green.
- `tests/tests/defaults.rs` couples documents to README.md Defaults,
  `.env.example` and `support::GRANTED` / `DEFAULT_MODEL`; a change to one
  side updates the others.
- Test text is new text: names and comments do not cite another
  implementation ("the original's", "the reference's"); describe the behavior.

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
