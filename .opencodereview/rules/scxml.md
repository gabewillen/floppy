# SCXML documents (`*.scx`, document IRP `*.scxml`)

A bot (`bots/<name>/`), ability (`abilities/<ability>/`), environment
(`environments/<name>/`) or device (`devices/<name>/`) is an SCXML 1.0 document
with `datamodel="ecmascript"`, run by scxml.rs. Review it against these rules
and cite the rule (AGENTS.md or `D<n>` in docs/decisions.md). Rules marked
*(contracts.rs)* are also enforced by `tests/tests/contracts.rs` for
`abilities/`, `bots/floppy/` and `environments/`; still flag them, and apply
them by hand to `bots/builder/`, `devices/` and fixtures that stand in for a
shipped document.

## Layout and loading

- The document is `<name>/<name>.scx` with its ECMAScript in `<name>/<name>.js`
  beside it (D65). Repository documents use `.scx`; `.scxml` is only for W3C
  and document IRP test documents under `irp/sources/` (D16).
- Exactly one `<script src="<name>.js"/>`, an empty direct child of `<scxml>`;
  no other `<script>` and no `function` token anywhere in the file, comments
  included (D39, D65) *(contracts.rs)*.
- Builder output (`behaviors/`, `bots/builder/examples/`) is `<name>.scx`
  beside `<name>.json` and `<name>.js`; every `event:schema` references
  `<name>.json#/$defs/<event>`; at most its one root script (D121).
- No foreign or unbound markup, no `datamodel:src` (D30). The document must
  validate clean under `scx validate` (0 errors, warnings, notes).

## Namespaces and invokes

- A floppy package is bound by its exact tagless URI,
  `https://github.com/stateforward/floppy/namespaces/<name>` with prefix
  `<name>`; a literal `<invoke type>` is exactly
  `https://github.com/stateforward/floppy/services/<name>`. An `@<tag>` pin is
  allowed only after those URIs, never on the built-ins (`event`, `env`, `scx`,
  `datamodel`), and one document never pins a package to two releases (D21).
- Built-ins use their prefixes: `event`, `env`, and `scx`
  (`https://github.com/stateforward/scxml.rs/namespaces/scx`, `<scx:validate>`).
  floppy never ships its own copy of `scx`.
- Every `<invoke type="scxml">` has a literal `id` and a literal relative `src`
  (no scheme, no leading `/`), or a `srcexpr` for a stored behavior (D121).
  No `<content>` child documents *(contracts.rs)*.
- `env:` attributes appear only on a service's `<invoke>`, never on an SCXML
  invoke or an extension element, and name only slots the service's manifest
  declares. Variable names match `.env.example` and `support::GRANTED`; a new
  variable updates both. Model backends go through `env:baseurl` /
  `env:apikey`; model calls bind `env:stream="FLOPPY_STREAM"`.
- Every `model` `<data>` is `Qwen/Qwen3.8-Flash-Next` and the model name
  appears nowhere else in a document (defaults.rs).
- A new service binding or WASI import means every README run command and the
  test grants change too; flag the document change if they do not.

## Composition (D39)

- The bot root (body) only composes: states, `<invoke>`s, `<send>`s. One `.scx`
  per ported state machine. Reusable synchronous logic is a namespace element
  that writes to a `location`; I/O is an invoked service.
- Anything asynchronous, I/O-bound or long-running (model calls, network,
  storage, files, decoding) is an `<invoke>` of a service, never executable
  content or an extension element (AGENTS.md Components).
- Abilities are invoked children or services; devices are invoked by the
  environment. No attachment protocol (`attachment.*`, `ready.request`): invoke
  lifecycle is attach and detach.
- The body sends what it emits to `#_parent`; there is no bot-as-root path
  (D102). Every literal `#_parent` / `#_<invokeid>` send has a receiving
  transition *(contracts.rs, literal sends only)*: check `eventexpr` and
  `targetexpr` sends by hand.

## Event contracts

- `event:schema` is always a file reference, never inline (D37, D41)
  *(contracts.rs)*. Paths (D125): an ability uses `../schemas/<event>.json`;
  the body and environments `../../abilities/schemas/<event>.json` (the room
  uses `../../bots/floppy/schemas/<event>.json` for body events); the body
  uses `schemas/<event>.json` for its own events; the builder uses
  `schemas/<event>.json`. No schema file is copied.
- A transition whose event is one contract event carries its `event:schema`;
  check multi-token and wildcard descriptors by hand.
- A new event a document takes needs its one schema file, the name list
  (`abilities/schemas/floppy-event-names.json`) and the event arrays in
  contracts.rs in the same change.
- Domain event names (`bot.*`, `bot.ability.*`, `phone.*`) are kept byte for
  byte; flag a renamed or re-cased one.
- Do not put a schema on transitions taking `generate.*` or `decision.*` in
  abilities: they are service results.

## Conditions and routing (D42, D81, D82)

- Payloads are checked by `event:schema` only. A `cond` correlates (which
  turn, run, timer, child) and must not re-check a payload's shape, except
  for events scxml.rs does not check: `done.invoke.<id>` donedata and an
  invoked service's events. Flag fallbacks only a schema failure could reach.
- A transition is routed by its event descriptor and its state. A `cond` must
  not test `_event.name`, and must not test `_event.invokeid` except for a
  re-entered state's `idlocation` id or the builder's simulated child. Use
  `done.invoke.<id>`, and take a child's failure as
  `error.communication.<invokeid>`. One transition per event rather than an
  `expr` or `<finalize>` that branches on the name.

## Expressions (D65)

- Every `cond`, and every `expr` outside `<data>`, is at most 160 characters
  *(contracts.rs measures only `cond` and `expr`)*; hold `delayexpr`,
  `targetexpr`, `eventexpr`, `srcexpr` and `location` to the same limit.
- Every `cond` and `expr` outside `<data>` that is more than a literal, an
  identifier or a dotted path calls a function of `<name>.js`. Accepted:
  `({})`, `delay(x_ms)`, `'#_scxml_' + s.target`, simple concatenation in
  `<log expr>`. Flag inline logic, ternaries, `&&`/`<`/`>` in attributes.
- `<data>` initial values stay inline (SQL text, constants, `*_ms`, `null`,
  `[]`). State changes are `<assign>`, never a side effect of a script call.

## Topology (AGENTS.md Bots, D9)

- Retries, timeouts and bounded loops are states, `<send delay>` and data-model
  counters, never host or component code. A timeout on an invoked service is
  armed in the invoking state, and leaving the state cancels the invocation.
- Timer idiom: `<send id="<state>-after" event="<state>.after"
  delayexpr="delay(<x>_ms)"/>` on entry, `<cancel sendid="<state>-after"/>` on
  exit (a state entered once per session needs no cancel, D93). Durations are
  `<data>` named `*_ms`; each `*_timeout_ms` needs a row in README.md Defaults.
- Deferral is `<event:defer/>` in a targetless transition, never a data-model
  queue (D95).
- Branching is a transient state with eventless `cond` transitions, the last
  unconditional. A stage end that cancels a child drains its queued answers
  with a self-send correlated by `sendid` (D186).
- Results are reported through the `<donedata>` of top-level finals.
- The builder builds SCXML only; drafts pass `scx:validate` and a simulation
  as an invoked child before `done`, and the builder never restricts what a
  draft may use (D28, D121). A repeated diagnostic ends a build with
  `no-progress` (D124).

## Removed names (hard cutovers)

Flag these when they reappear: `<decision:menu>` and "menu" (now
`<decision:choice>`), `sx:`/`scxml` authoring namespaces (now `scx`),
`namespaces/bot` and `<bot:*>`, `<decision:compact|assemble|selection|route|names|tuner>`,
`<conversation:commit>`, `keylocation`, `tuner` (now `confidence_tuner`),
decision kinds `choose`/`classify`/`select`/`rank` (now
`choice`/`predicate`/`score`), `<content expr>` behavior invocation (now
`<invoke srcexpr>`), `bot.activate`, `bot.clear_focus`, the body's `routing`
state, `floppy.route*`, `bot.ability.reasoning.retained`,
`previous_fingerprint`, schema twin directories such as
`abilities/<ability>/schemas/`.
