# Document ECMAScript (`<name>/<name>.js` beside a `.scx`)

This file is the ECMAScript of the SCXML document `<name>.scx` beside it,
loaded only by that document's root `<script src="<name>.js"/>` (SCXML 5.8,
D65); its functions are globals of the document's data model, which runs on
Boa. Review it against these rules and cite the rule.

- **Functions read and return.** Each function reads the data model (or the
  `_event` passed to it as a parameter) and returns a value. It never assigns
  data-model variables or other globals and has no side effects; state changes
  stay `<assign>` in the `.scx` (D65). Flag mutation of arguments or globals.
- **Every non-trivial `cond`/`expr` of the document calls a function here**,
  and each call site stays at most 160 characters. A function the document
  calls must exist here; a function must not throw on every evaluation (D123).
- **No payload validators.** Do not add `isObject`/`validX` shape checks of
  contract events: `event:schema` does that (D42). Shape checks of
  `done.invoke` donedata, service results, or stored rows read back from a
  database are allowed.
- **Numbers are plain JS numbers.** Do not imitate Python or pydantic float
  formatting (`1.0` vs `1`), `repr`-style number-to-string shims, or Python
  rounding helpers; emit numbers as JSON numbers and let JSON do the rest. The
  data model holds IEEE doubles and canonical JSON sorts object keys, so do not
  rely on key insertion order or big-integer precision (D88, D141).
- **Time.** Boa has no `Intl` or `Temporal`: time-zone logic uses POSIX TZ
  rules as `next_after` in `abilities/revision/revision.js` does (D83). Use
  `Date.now()` and UTC (`toISOString`, `Date.UTC`). Database timestamps come
  from SQL in UTC (D63, D69); do not hand-format a timestamp to look like a
  Python serializer's output when the schema only needs a valid date-time.
  Delays are `*_ms` integers, turned into a delay with the shared
  `delay(ms)` helper.
- **Bounds.** On wasm every value crossing the data-model boundary is limited
  to 64 KiB (D117), and a send over the event bound raises `error.execution`
  in the sender. Code that builds prompts or payloads must not assume more.
- **Reuse namespace elements.** Logic that exists as a namespace element is
  called through the element, not reimplemented here (D113 to D116).
- **Idiomatic code.** Write the behavior in ordinary ECMAScript; flag helper
  names, structure or data shapes carried over from a Python implementation
  rather than required by the behavior or the schema.
- **Style** (flag only gross drift): a header comment naming the document,
  functions grouped by state with `// <state>` comments, a one-line comment on
  what each returns.
- Builder output's `<name>.js` may be empty but always exists (D121).

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
