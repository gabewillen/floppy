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
