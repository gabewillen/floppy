# Session snapshots (`scxml.session-snapshot/v2`)

A session snapshot is a canonical JSON document that shows a running root
session and the sessions it invoked at a stable checkpoint. Tools and
LLMs read it to inspect them. It is read-only: nothing in this workspace saves, restores, resumes, or
rolls back session state, because SCXML defines no persistence. An agent that
wants to act on what it sees uses the existing send path.

- Model, codec, diff, and renderers: the pure crate `crates/scxml-snapshot`
  (serde, `serde_json`, `serde_path_to_error`, `sha2`, `scxml-types`, and
  the terminal escaping of `scxml-reporting`; no Wasmtime, no engine, no
  I/O). The Wasmtime host and the CLI link it natively.
- Browser: the optional capability-free component
  `components/scxml-snapshot-codec` (WIT world `session-snapshot`). It
  exports only `encode` and `diff` of the same crate, so both hosts encode
  the same capture to the same bytes. At every checkpoint both hosts reach,
  the shared fixtures pin documents that are byte-identical except for two
  documented host differences: the BasicHTTP entries of
  `system.ioprocessors` when the browser page configures no BasicHTTP
  (`http`), and event trace carriers under `preserve` (see
  [Parity fixtures](#parity-fixtures)).
- JSON Schemas (Draft 2020-12):
  `crates/scxml-snapshot/schema/scxml.session-snapshot.v2.schema.json` and
  `…/scxml.session-snapshot-diff.v2.schema.json`. The decoder is
  authoritative. The schemas cannot express key order, canonical form, or
  the digest.

## When a snapshot can be taken

Snapshots are taken only at a checkpoint:

- every machine call has returned `stable` (SCXML Appendix D: the internal
  queue is empty and no eventless transition is enabled);
- no host scheduling operation is in progress;
- no session is `failed` or `shut-down`.

A request for a `failed` or `shut-down` session fails with `status-invalid`
on every host. A request at any other point outside a checkpoint (during a
machine call or a host drain, re-entrantly from a component or telemetry
callback, or while a delivered event is still being processed) fails with
`not-stable`. Either failure changes nothing. Pending delayed sends do
not prevent a checkpoint: they are part of the snapshot.

On the Wasmtime host the checkpoints are numbered in this order:

- `0` after `start`;
- one after each scripted input or external-client post;
- one at the top of each iteration of the executor's drain: each iteration
  then delivers exactly one entry of one session's external event queue, or
  one batch of due delayed sends;
- one final checkpoint at the end of the run.

The executor runs the root session and every session it invoked, at any
nested invoke depth, cooperatively, in invocation order: the root first, then
each invoked session followed by the sessions it invoked, siblings in
ascending invoke-id order. Each session owns an internal event queue (inside
the machine) and an external event queue. The SCXML Event I/O Processor
delivers across sessions by pushing onto the target session's external
queue when the event is sent (`#_parent`, `#_<invokeid>`, autoforward);
`done.invoke.<id>` joins the invoking session's queue after every event the
finished session sent. A session whose cancellation has begun sends nothing
more to its invoking session; events it sent earlier stay queued.

The per-component `snapshot` exports and document assembly have no side
effects. They apply no effect, evaluate no condition or expression, call no
capability, charge no engine command, record no trace entry, and advance no
clock. As a result every conformance report field, fuel included, is the same
whether or not snapshots are taken.

### Non-perturbation proof over the IRP

`scxml conformance run --snapshot-every-checkpoint` runs the selected
cases three times through the shipped components under Wasmtime: once
plain, and twice with one read-only document per reached checkpoint
(`SnapshotPoints::Every`, nothing excluded, `preserve`). It fails closed
unless the three canonical reports are byte-identical (collection never
perturbs dispositions, configurations, commands, fuel, traces, or logs), the
two collecting runs produce byte-identical documents, and `check` reports
no issue for any document (its passes include the canonical re-encode).
The host lets a session go on when one checkpoint's snapshot fails (for
example over a document bound), so a collecting run whose host reports any
failed checkpoint snapshot is that case's harness failure
(`session-snapshot`) rather than a collection with a gap, and the reports
then differ.

A per-model summary with each case's checkpoint ordinals and document
digests is written to `target/snapshot-proof/<datamodel>.json`, relative to
the current directory. The plain report is then handled exactly like a
plain run's: the per-case lines and summary are printed, `--json` and
`--report-copy` are written, and `--baseline`, `--fail-on-regression`,
`--canonical-baseline`, and the mandatory-complete gate all apply.

Collecting runs keep telemetry on, with deterministic sequential telemetry
ID generators instead of random ones: `preserve` needs a real carrier on
every event, and the OTEL SDK tracer provider cannot be bypassed. The
pinned IDs keep the documents byte-stable across runs.

## Document

```json
{"version":"scxml.session-snapshot/v2","sha256":"<64 hex>","options":{"exclude":[],"trace_context":"strip"},
 "snapshot":{"components":[{"role":"clock","sha256":"…"},…],"clock":{"now_ms":250},"root":{…}}}
```

`sha256` is the SHA-256 of the canonical bytes of `snapshot`. It detects
corruption. It is not authentication. A document is at most 32 MiB. A larger
one fails with `too-large` and is never truncated.

### Canonical encoding

- Compact `serde_json`, UTF-8, no whitespace, no trailing newline.
- Keys appear in contract order, which is struct order, never sorted.
- No JSON object is used as a map: collections are arrays with a stated
  order.
- An absent optional value is omitted, and `null` is never written. An empty
  array is written as `[]`.
- Integers are unsigned, at most 2^53 − 1. The document contains no JSON
  floats.
- Data-model numbers are strings holding Rust `f64` `Display` output: the
  shortest round trip, no exponent, `-0` kept.
- JSON data values are strings holding their canonical JSON text. XML values
  are strings holding XML text.
- Digests are 64 lowercase hex characters.

The decoder checks, in order: size, JSON, `version`, shape (unknown keys are
rejected), re-encode byte equality (`non-canonical`), `sha256`, bounds,
names, and consistency. `scxml snapshot check` lists every issue it finds
after the structural checks.

### `options`

- `exclude` is sorted and unique, drawn from `system`, `data_model`, `queues`,
  `pending_sends`, `transitions`, `previews`, and `invocations`. An excluded section
  is omitted from every session, and the host never collects its data.
  `previews` omits only a transition's `exit` and `entry`. `invocations` omits
  `invocations` and the invoked sessions.
- `point` is `executable-content` for a snapshot an extension component took
  while one executable-content extension executed
  ([Snapshots during executable content](#snapshots-during-executable-content))
  and omitted for a checkpoint snapshot.
- `trace_context` is `strip` (the default) or `preserve`. Under `preserve`,
  events keep their held W3C carriers (`traceparent`, `tracestate`,
  `baggage`, `baggage_policy`). Carriers are specific to one run, and baggage
  can carry personal data. Documents therefore get the protections of
  canonical validator artifacts: they are never logged or indexed.

### `snapshot`

- `components` lists the release-artifact digest of each role: `clock`,
  `data-model-<name>` for every data model the root session and its invoked sessions link,
  `io-processor-scxml`, and `machine`. It is sorted by role. On the
  browser host a page-supplied data model is named like a bundled one
  (`ecmascript`, `xpath`, or `null`), and its `data-model-<name>` entry
  carries that bundled component's release digest, not a digest of the
  page's own implementation.
- `clock.now_ms` is the shared virtual clock. Every session runs on it:
  a session's own clock is moved to it before the session runs, and before
  it is captured, so every captured session reads this time.
- `root` is the root session object.

### Session object (keys in this order)

| Key | Content |
| --- | --- |
| `session_id` | `_sessionid` |
| `name` | `_name`: the document `name`; absent when the document has none (SCXML 5.10) |
| `datamodel_name` | `ecmascript`, `xpath`, or `null`; omitted when no model is linked |
| `ir` | `{"version":5,"sha256":…}`: the digest of the exact Machine IR bytes loaded |
| `status` | `ready`, `running`, or `finished` |
| `invoked_by` | `{parent_session_id, invoke_id}`, invoked sessions only |
| `configuration` | `{"atomic":[…],"full":[…]}`: active atomic/final states and every active state, both in document order |
| `history` | `[{state, value:[…]}]`: recorded history values |
| `bound_states` | late-binding states already bound (SCXML 5.3) |
| `counters` | `{send, invoke}`: the next generated send-id and invoke-id ordinals. Both start at 0: send ids at `sendid_0`, invoke ids at `<state>.0`. Only a generated id takes an ordinal; a `<send>` or `<invoke>` with a literal `id` leaves its counter unchanged, so a fresh session reports `send: 0, invoke: 0` |
| `system` | section: `{"sessionid","name","ioprocessors"}` |
| `data_model` | section: `[{name, value}]`, each declared `<data>` id in document order (ECMAScript: late-bound ids declared afterwards follow) with its current value (≤ 256) |
| `queues` | section: `{"internal","external"}` |
| `pending_sends` | section: delayed sends by deadline, then in schedule order (≤ 1024): `{sendid, event?, basichttp?, target?, type?, delay_ms, deadline_ms}` with exactly one of `event` (an SCXML Event I/O Processor send) and `basichttp` (a `BasicHTTP` send, SCXML C.2: `{event?, params:[{name, value}], content?, trace_context?}`, as evaluated at the `<send>`) |
| `transitions` | section: `[{source, index, events, targets, type, cond?, has_cond, domain?, exit?, entry?, event_schema?}]`, every transition an event could trigger (≤ 10 000) |
| `invocations` | section: `[{invoke_id,type,state,index,autoforward,finished,session}]`; every session object has it, at any depth |

- **Values** have the form `{"type":T,"value":V?}`. `T` is `absent`,
  `undefined`, `null`, `boolean`, `number`, `text`, `json-array`,
  `json-object`, or `xml`. The data model reads each declared variable
  through its normal value conversion (the same one `<send>` data and
  `evaluate-value` use), without running user code. An ECMAScript variable
  whose value has no portable form (a function, a `Date`, a cyclic object,
  `NaN`, ...) is shown as `{"type":"text","typeof":K,"text":S}`: `K` is its
  ECMAScript `typeof` and `S` its string form (a function's source text, a
  `Date`'s `Date.prototype.toString`, `[object Tag]` with ECMA-262's
  built-in tag for any other object, a primitive's `ToString`). Script
  globals that are not `<data>` ids are not listed.
- **Oversized variables.** A variable whose text (its value text, XPath
  element markup, or string form) is longer than 65 536
  bytes is shown as `{"type":"oversized","bytes":N}`: its exact length in
  UTF-8 bytes, with no text. Nothing is truncated, and a variable's size
  never makes a snapshot fail. `N` is always greater than 65 536; a value that fits
  is written inline, and the decoder rejects an `oversized` entry that
  would have fit as `inconsistent`. `show` renders it as
  `<oversized: N bytes>`.
- **Variable count.** Each data model declares at most 256 `<data>` ids
  and rejects a 257th when it would be declared, so the `data_model` section
  never exceeds its bound and the variable count never makes a snapshot
  fail.
- **Document bounds.** One bound is enforced before a snapshot: 1024
  pending sends per session (the clock's timer bound). The others are not,
  because the native host imposes no limit of its own on invoked sessions
  or queues, so a valid set of sessions can exceed them: 255 invoked
  sessions under the root session, 32 nested invoke depth, 10 000 listed
  transitions and 10 000 states in one configuration list per session,
  10 000 entries in one queue, and 32 MiB for the whole encoded document
  (JSON escaping counts, so a control character in a value takes 6 bytes).
  A snapshot over one of them fails and nothing is truncated: with
  `bounds-exceeded` and a pointer that names the bounded list (for example
  `/snapshot/root/transitions`), with `depth-exceeded` for the
  invoke depth, or with `too-large` for the document size. Only that
  snapshot fails; the session goes on. Excluding the section (for example
  `transitions`) avoids its bound.
- **Data-model image.** `system` and `data_model` are omitted while the
  session is `ready`, because the data model is not initialized yet. They are
  also omitted when no data model is linked, and when a linked data model has
  never been initialized: the host records whether `initialize` ever ran and
  reports no image until it has.
- **Queues** are the SCXML Appendix D queues, FIFOs of at most 10 000
  entries each in a document:
  - `internal` is `[]` at every checkpoint.
  - `external` is the session's external event queue, oldest first: events
    sent to it (by its invoking session, autoforward, or an external
    client's post) and events from the sessions it invoked, shown as the
    session will process them: stamped with their `invokeid`, and a finished
    session's completion as `done.invoke.<id>` carrying its `<donedata>`.
- **Pending sends** have the form
  `{"sendid","event","target"?,"type"?,"delay_ms","deadline_ms"}`.
  `deadline_ms` is absolute on the virtual clock. `show` renders
  `due +Nms`.
- **Transitions** are a static view: conditions are never evaluated and events
  are never matched.
  - The walk takes each active atomic state in document order, then its
    ancestors up to `<scxml>`. Each state's transitions that carry at least
    one event descriptor are taken in document order, with duplicates
    dropped.
  - `has_cond` is true exactly when `cond` (the authored text) is present.
  - `domain` is the SCXML 3.13 transition domain. It is omitted when the
    transition has no target or when the domain is `<scxml>`.
  - `exit` and `entry` are the engine's own exit and entry sets for this one
    transition taken alone.
  - `event_schema` is the transition's `event` attribute vocabulary
    ([namespaces/event](../namespaces/event/README.md)) exactly as the
    Machine IR carries it, and is omitted when the transition has none:
    `{"kind":"schema","schema":S,"sha256":H}` for `event:schema` (`S` the
    schema's canonical JSON text, at most 1 MiB; `H` its SHA-256, which the
    decoder checks, `inconsistent` otherwise) or
    `{"kind":"schemaexpr","expr":E}` for `event:schemaexpr`. A snapshot
    evaluates no expression, so `E` is the authored expression, never its
    value. The schemas of `<send>`, `<raise>`, and `<donedata>` have no
    snapshot entry; they are in the Machine IR that `ir.sha256` names.
    `show` renders `schema sha256:<first 12 hex>` or
    ``schemaexpr `<expression>` ``.
- **Invocations** are listed in ascending invoke-id order, each with the
  session it invoked, which lists its own `invocations` in turn: at most
  255 invoked sessions under the root session, at most 32 nested invoke
  depth in a document.

## Snapshots during executable content

An extension component may read the executing session's snapshot while it
executes one executable-content extension
(`stateforward:scxml/session-snapshot-host`,
[executable-content-extensions.md](executable-content-extensions.md), "Reading
the session snapshot"). Both hosts offer it: the Wasmtime host encodes it
with the `scxml-snapshot` crate, and the browser host with the codec
component (`snapshot-options.point` `executable-content`), byte for byte the
same document. The document has the
same format, with `options.point` set to `executable-content` (omitted, and so
`checkpoint`, everywhere else). It is read-only like every other snapshot.

Executable content is not a checkpoint: the session is inside a
macrostep, at the executable-content extension element being executed.
The document says exactly what holds at that element:

- **One session.** `root` is the executing session, which may itself be
  an invoked session (then it carries `invoked_by`). `invocations` and
  `previews` are always in `options.exclude`; the decoder rejects an
  executable-content document without them (`inconsistent` at `/options/point`).
- **Configuration.** `configuration.full` is the configuration the element
  observes, the one `In()` answers (SCXML 5.9.1; official case 409): exit
  content no longer sees the states exited before it, and entry content
  sees every state entered before it, so during the entry content of a
  compound state its children are not yet listed. `configuration.atomic`
  lists the atomic and final states among them, so it is empty while none
  is entered yet (the decoder accepts that only for an executable-content
  document).
- **Transitions.** The walk starts from each innermost state of that
  configuration (a state none of whose children is in it; at a checkpoint
  these are exactly the atomic states), so the entry content of a compound
  state sees that state's own transitions. Conditions are never evaluated.
- **Queues.** `internal` is the machine's internal queue at the element
  (events raised earlier in the macrostep and not yet processed) followed
  by the events this extension has queued so far; it may be non-empty,
  and the decoder accepts that only for an executable-content document.
  `external` is the session's external queue followed by the extension's
  self-addressed sends so far.
- **History, bound states, counters, status.** As the machine holds them
  at the element: history the macrostep's exits recorded is present (SCXML
  3.10 records it before exit content runs).
- **Data model, pending sends, clock.** Read when the extension component asks,
  so they include the assignments and delayed sends made earlier in the element's
  block and by the extension component itself.
- **Trace carriers** are always stripped.

The machine cannot be called while it executes the extension, so it hands this
image to the host with the element (`machine-capabilities.execute-extension`),
and computes it only for an extension component whose component imports
`session-snapshot-host`. Taking it changes nothing, so extension outcomes,
configurations, traces, and fuel are the same whether or not an extension
component reads it, apart from the image's own computation in the machine call.

## Snapshots of another session

During the same `execute` call, `session-snapshot-host.snapshot` with a
`target` reads another session of the executing session's run, resolved as
a `<send>` target is (`#_parent`, `#_<invokeid>` of a live invoked session,
`#_scxml_<sessionid>` of any running session of the run; a target naming the
executing session is the executable-content document above). The executor
runs one session at a time, so every other session is between machine calls
and stable ("When a snapshot can be taken"): the document is that session's
at its last checkpoint, `options.point` `checkpoint` (omitted), with one
session object (an invoked one carries its `invoked_by`, which a document
listing invocations never has at its root),
`invocations` and `previews` always in `options.exclude`, carriers stripped,
its external queue as it stands, and its own clock. Both hosts offer it.
Nothing is recorded: the read is a pure function of the run's state at the
call, which replay reproduces, and it changes nothing in the target session
(its later documents and trace are those of a run without the read). A
target that is malformed or names no running session of the run (an
invoked service, a finished, `failed`, or `shut-down` session) is
`unavailable`; the 64-call and size bounds apply unchanged.

## Diff (`scxml.session-snapshot-diff/v2`)

`scxml snapshot diff A B` compares two valid documents. The JSON output
looks like this:

```json
{"version":"scxml.session-snapshot-diff/v2","before":{"sha256":"…"},"after":{"sha256":"…"},
 "identical":false,"compatible":true,"clock":{"from_ms":100,"to_ms":350},
 "sessions":[{"path":["root"],"change":"changed","status":{"from":"running","to":"finished"},
   "configuration":{"exited":["s0"],"entered":["s1"]},
   "data_model":[{"name":"x","change":"changed","from":{…},"to":{…}}],
   "queues":{"external":{"consumed":1,"appended":[…]}},
   "pending_sends":{"added":[…],"removed":["sendid_0"]},
   "invocations":{"added":["i2"],"removed":[]},
   "transitions":{"added":[{"source":"s1","index":0}],"removed":[…]}},
  {"path":["root","i2"],"change":"added"}]}
```

- **Sessions** are matched by path (`["root"]`, `["root", invoke_id]`). They
  are listed in `after` invocation order, then removed sessions in `before`
  invocation order. Unchanged sessions and keys are omitted.
- **`configuration`** holds the set differences of `full`, in document
  order. `atomic` (`{exited, entered}`) appears only when the atomic states
  changed in a way `exited`/`entered` do not show: a state that stayed active
  but stopped or started being listed as atomic.
- **`system`, variables, history, and counters** are compared by key. Changed
  and added entries appear in `after` order, then removed entries in
  `before` order.
- **`session_id`, `name`, and `ir`** are reported as `{from, to}`, and
  **`datamodel_name` and `invoked_by`** as `{from?, to?}` (an absent side is
  omitted). **`bound_states`** reports `{added, removed}`. **`reordered`**
  lists session lists (`configuration`, `history`, `bound_states`,
  `data_model`, `pending_sends`, `invocations`, `transitions`) whose entries are
  unchanged but whose order differs.
- **Queues** are compared as FIFOs. `consumed` is the smallest `k` such that
  `before[k..]` is a prefix of `after`, and `appended` holds the rest. Such
  a `k` always exists (`k = len(before)` relates any two queues), so every
  queue change has this one form.
- **Pending sends** are keyed by `sendid`. A changed send is listed both as
  removed and as added. **Transitions** are keyed by `(source, index)`. A
  transition listed on both sides with a different listing (events, targets,
  type, `cond`, domain, or `exit`/`entry` previews, which are ignored when
  either side excluded `previews`) appears in `changed` with its full `from`
  and `to` listing. **Invocations** report added and removed invoke ids,
  plus `changed` entries whose `type`, `state`, `index`, `autoforward`, or
  `finished` differ.
- **Carriers** are ignored unless `--include-trace-context` is given.
- **`skipped`** lists every section excluded on either side. It is always
  written, as `[]` when no section was excluded (canonical documents never
  omit an empty array).
- **`compatible`** is false when any matched session loaded different IR.
  The diff still runs.
- **`identical`** is true when no difference is found, and false exactly when
  at least one change is reported. `exclude` (`{from, to}`) appears when
  `options.exclude` differs, and `components` (keyed by role, like `system`)
  when a component identity differs. `clock` is omitted when the clock did
  not move.

## CLI

```text
scx snapshot run DOC.scx [--components DIR --components-manifest-sha256 HEX]
    [--event NAME]... [--at N]... | [--every] [--exclude SECTION]...
    [--trace-context strip|preserve] [--output FILE | --output-dir DIR]
scx snapshot show SNAP.json [--format human|json] [--color always|never]
scx snapshot check SNAP.json [--format human|json]
scx snapshot diff A.json B.json [--format human|json] [--include-trace-context] [--color always|never]
```

- **`run`** compiles DOC and runs it through the shipped components under
  the Wasmtime session host: the scripted events, then drain. It snapshots
  the final checkpoint by default, or the ones named by `--at` or
  `--every`. One document goes to stdout, or to `--output FILE`. Several
  checkpoints need `--output-dir DIR`, which receives
  `DIR/NNNN.snapshot.json`. A `--at` ordinal the run never reaches fails
  the run with the `unreached` failure. A snapshot of a named checkpoint
  that fails (for example over a document bound) fails the command after
  the run. A build without the `wasmtime`
  feature fails with `[wasmtime-disabled]`. `<invoke src>`,
  `<script src>`, and `<data src>` load by SCXML's defaults: an `https:`
  URL, a `file:` URL, or a relative URI resolved against the base URI of
  the document that names it. Every literal one, of DOC and of every
  `<invoke src>` document it loads, is fetched once before the first
  macrostep and listed (`document `URL``, `= for: <invoke src="…">`, and
  its size and SHA-256); a `<script src>` of DOC that cannot be fetched
  rejects it (SCXML 5.8), and any other failure raises `error.execution`
  when its element runs. An `<invoke srcexpr>` URI is fetched each time the
  invoke runs, as the document is then. Remote documents are fetched over HTTPS only, within 16 MiB,
  at most 5 redirects (each to `https:`), and the connect and total
  timeouts, with no retry and no credentials; `--offline` forbids them. A
  document's HTTP `Content-Type`, or else a URL ending in `.json` or `.xml`
  (`application/json`, RFC 8259 §11; `application/xml`, RFC 7303 §9.1), is
  the media type the ECMAScript data model reads as the indication SCXML
  B.2.1 lets it interpret `<data src>` content by; any other file, `.txt`
  included, carries none and is read by content. `run` loads them the same
  way.
- **`show`** renders one view per session: identity, status,
  configuration, history, variables, queues, pending sends as `due +Nms`,
  transitions with their previews, and children. `--format json` re-emits the
  validated bytes.
- **`check`** prints rustc-style `error[SNAP-<code>]` diagnostics with
  `= pointer:` and `= help:` lines. A `malformed-json` issue is located as
  `--> PATH:LINE:COLUMN`. `--format json` prints one compact object with
  sorted keys: `{"issues":[…],"valid":true|false}`, where each issue is
  `{"code":"SNAP-<code>"}` plus `"pointer"` when the issue has one, and
  `"column"` and `"line"` for `malformed-json`. For example:
  `{"issues":[{"code":"SNAP-schema-violation","pointer":"/snapshot/clock/now_ms"}],"valid":false}`.
- **`diff`** prints `snapshot diff: A → B`, then for each session a
  `--> status:` line and `= configuration`, `= datamodel`, `= queues`,
  `= pending sends`, `= children`, and `= transitions` blocks with `+`, `-`, and
  `~` markers. In text output only, values over 120 bytes are elided.
  `--format json` writes the canonical diff document.

### Terminal output

Every string that `show` and `diff` take from a document, and the file
names `diff` prints, is untrusted. Human output escapes C0 and C1 control
characters, DEL, the U+2028 and U+2029 separators, and the Unicode
bidirectional formatting characters (U+061C, U+200E, U+200F,
U+202A–U+202E, U+2066–U+2069) as visible `\u{XXXX}` escapes, and newlines,
carriage returns, and tabs as `\n`, `\r`, and `\t`. It uses the same
escaping as the validator's diagnostics. A multi-line value therefore stays
on its own `+`, `-`, or `~` line, and only the renderer's own color codes
reach the terminal. Elision happens after escaping. `--format json` output
is the document or diff bytes, unchanged.
- **Exit codes:**
  - `run` and `show` exit 0, or 2 on error.
  - `check` exits 0 when the document is valid, 1 when it has issues, and 2
    on error.
  - `diff` exits 0 when the documents are identical, 1 when they differ,
    and 2 on error.

## Error codes

Every failure carries a closed code and an optional RFC 6901 pointer. The
pointer is built only from fixed keys and indices, is at most 256 bytes, and
never contains a name or payload. `scx --explain SNAP-<code>` prints
the long form of each code.

| Code | Meaning |
| --- | --- |
| `too-large` | the document is over 32 MiB |
| `malformed-json` | the input is not JSON |
| `unsupported-version` | the `version` is not `scxml.session-snapshot/v2` |
| `schema-violation` | wrong shape, an unknown key, a float, or an unknown enumeration value |
| `non-canonical` | valid, but not the canonical bytes (formatting, key order, `null`, number or JSON text) |
| `integrity-mismatch` | `sha256` does not match `snapshot` |
| `bounds-exceeded` | a list, string, or integer is over its bound |
| `invalid-name` | an identifier is not a valid SCXML name |
| `inconsistent` | a derived field disagrees, an id repeats, a parent link is broken, or section presence does not match `options` |
| `depth-exceeded` | an invoked session is deeper than 32 nested invoke depth |
| `clock-invariant` | a session clock differs from the shared clock |
| `not-stable` | requested outside a checkpoint: during a machine call or drain, re-entrantly, or while a delivered event is still being processed |
| `status-invalid` | requested for a `failed` or `shut-down` session (every host) |
| `codec-unavailable` | the browser host has no codec component |

The codec component reports these codes as `invalid-input`, or as
`payload-too-large` for `too-large`, with the static detail
`snapshot-<code>`.

A `schema-violation` points at the value that has the wrong shape: a float,
a negative or out-of-range integer, or an unknown enumeration value is
located at that value, and an unknown key at the object that holds it (the
key itself is document text). A document that is not a JSON object has
the root pointer `""`, which human output shows as `"" (the document
root)`.

Pointers name two different trees, depending on when the failure is found.
Errors raised while assembling or encoding a capture point into the
*would-be document* (for example `/snapshot/root/datamodel`). Invocation-shape
errors — a broken parent link, an invoked session deeper than the bound, or an
invocation without its child capture — point into the *capture* (for
example `/sessions/1/parent`), because the document could not be built. Both
spellings are accepted; a pointer never names user data.

On the Wasmtime host a snapshot failure never fails or ends a session: a
batch run reports each failed checkpoint snapshot beside the documents it
took (`SessionOutcome::snapshot_failures`), and a live session's
`snapshot` answers the failure and the session goes on. The Wasmtime host
fails a run only when a requested checkpoint is never reached
(`unreached`). `scx snapshot run` sorts and deduplicates
its `--at` ordinals, so their order on the command line does not matter;
strictly ascending ordinals are a precondition only of the Wasmtime host's Rust
`SnapshotPoints::At` API, which fails a run whose list is not.

## Telemetry

Telemetry is recorded only at boundaries:

- the codec component's `encode` and `diff`, each one observation with the
  output size as the `snapshot-bytes` measurement;
- the Wasmtime host's `session-snapshot` span;
- the CLI spans `scxml.cli.snapshot-{run,show,check,diff}`.

The components' read-only `snapshot` exports (machine, data model, I/O
processor, and clock) record no telemetry. The `scxml-snapshot` crate itself
is pure and records nothing. No session
id, state id, pointer, or code ever appears as a telemetry attribute.

## Parity fixtures

The shared fixtures in `tests/fixtures/session-snapshot` (`<name>.scx`
plus `<name>.ops.json`, with goldens in `golden/<name>/` and diffs in
`diff/<name>/`) run on both hosts. Their scripted-input vocabulary is
deliberately narrow: `start` first and exactly once, `send`, `advance`, and
`snapshot` ops with strictly ascending checkpoints, and no `send` after an
`advance`. Wasmtime scripts every send before one drain, so `advance` is a
position marker there; the browser settles each send fully as it is posted.

A `snapshot` op applies to both hosts by default. One marked
`"hosts":"wasmtime"` names a checkpoint only the Wasmtime executor reaches,
such as one in the middle of a drain while a session's external queue is
non-empty;
its golden is compared on Wasmtime only.

Cross-host output is byte-identical at every checkpoint both hosts reach,
except for two host differences:

- **`system.ioprocessors`, only without browser BasicHTTP.** The Wasmtime
  host always links the BasicHTTP Event I/O Processor. The browser host
  advertises it only when the page configures `http`. Without it, the
  Wasmtime `_ioprocessors` value of each session also holds the `basichttp`
  and `http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor` entries, and
  every other entry is identical. With `http` configured at the same
  locations (`http.location` answering each session's Wasmtime loopback
  location), the value is byte-identical and this exception does not
  apply.
- **Event trace carriers under `preserve`.** Each embedder's telemetry
  provider mints the carriers, so the `trace_context` of an event in
  `queues` or `pending_sends` is specific to the host and its provider. The
  Wasmtime goldens pin deterministic IDs, while the browser runs on the
  page's provider. `options.trace_context` itself is identical. Under the
  default `strip` there is no carrier and no such difference.

The goldens do not bind component identity. `snapshot.components[*].sha256`
names the release artifacts that produced a document, and those bytes
depend on the build host: the canonical release is the Linux x86_64 CI
build (docs/conformance.md), and a build on another host reproduces every
snapshot's content but not the component digests. Both the Wasmtime
harness and the browser parity test therefore replace every component
digest with the fixed placeholder
`0000000000000000000000000000000000000000000000000000000000000000`,
re-derive the envelope `sha256`, and assert that nothing else changed,
before comparing bytes. The goldens are stored in that normalised form
(the harness blesses them normalised, and the diff goldens are computed
from the normalised documents), so they are identical on every host. A
normalised golden is still a valid document: its `sha256` is re-derived,
so `scxml snapshot check` accepts it.

`sha256` covers the component digests and both host differences, so it
differs whenever any of them does. The browser
parity test runs every fixture twice: with BasicHTTP configured at the
Wasmtime locations, where it removes no `_ioprocessors` entry, and without
`http`, where it removes the BasicHTTP entries. In both runs it normalises
the component digests and removes the preserve-mode carriers in both
documents, re-derives `sha256`, and asserts that nothing else changed. It then compares the bytes
of every unmarked checkpoint exactly. A document with neither difference
(for example `system` excluded, or no data model, under `strip`) is compared
unchanged. The test skips only marked checkpoints, and it fails unless each
fixture compares at least one shared checkpoint.
