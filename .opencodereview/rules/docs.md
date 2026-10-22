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
