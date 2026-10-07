# Implementation-defined choices and limits

Where the specification leaves a choice to the platform, this project's
choice and the governing text are listed here; where the implementation
departs from or bounds what the text requires, it says so. The departures
that follow official IRP cases are in [spec-errata.md](spec-errata.md),
with the known open defects under its "Known limitations (pending)".

## Choices the specification leaves to the platform

- **Default data model: ECMAScript.** SCXML 1.0 §3.2.1 gives `datamodel` the
  default "platform-specific", and §3.2.2 says "Platforms SHOULD document
  their default data model"; see
  [architecture.md](architecture.md#components-and-declarative-resources).
- **`_name` without a `name` attribute: no value.** See
  [architecture.md](architecture.md#_name-without-a-name-attribute) (§5.10).
- **`<scxml exmode>`: accepted, no effect.** The W3C schema declares it
  (`lax` or `strict`) and the Recommendation gives it no semantics, so it
  validates against the schema and changes nothing
  ([spec-errata.md](spec-errata.md), S.1).
- **Sessions `#_scxml_<sessionid>` reaches.** SCXML 1.0 C.1: "The set of
  SCXML sessions that are accessible to a given SCXML Processor is
  platform-dependent." A session reaches itself, the session that invoked
  it, and its own live invoked sessions; any other id (a sibling, a
  grandchild, a finished child, another run's session) raises
  `error.communication`.
- **`BasicHTTP` location.** SCXML 1.0 C.2.3: the processor "MUST maintain a
  'location' field inside this entry whose value holds an address that
  external entities can use to communicate with this SCXML session". In
  batch runs, IRP runs, and browser sessions the page gives no location, the
  location is `http://basichttp.loopback.invalid/sessions/<_sessionid>/basichttp`,
  which only the host's in-process loopback answers (`.invalid` never
  resolves), so no external entity can use it there; a live session with
  `scx run --http-listen` (or a browser `http.location`) advertises an
  address external clients reach (`<base>/sessions/<_sessionid>/basichttp`).
  The SCXML processor's own location, `#_scxml_<sessionid>`, likewise reaches
  only the sessions listed above.
- **Duplicate keys in `_event.data`.** SCXML 1.0 §5.10.1: "In the case of
  duplicate keys, the behavior is platform-specific." ECMAScript keeps the
  last value of a repeated key; XPath keeps every one, each as its own
  `<data id>` child of `_event/data`.
- **`_sessionid` of a session.** SCXML 1.0 §5.10 has the processor bind
  "the system-generated id for the current SCXML session". The host chooses
  the root's id (`scx run --session-id`, default `session`; the browser
  host's default `machine-session`), and names each invoked session
  `<root id>.<n>`, `n` counting the sessions the run invoked in the order
  they start; ids are unique within one run, the only scope `#_scxml_`
  addresses, and do not grow with the nesting depth, so an invoke chain of
  any depth keeps every id within the 1024-byte name bound (only a root id
  within a few bytes of that bound leaves too little room for its invoked
  sessions' ids). A generated invoke id is `<state id>.<invoking session's
  _sessionid>.<n>` (SCXML 6.4.1 `stateid.platformid`). The host-side
  records that name a session also name its parent: trace entries and
  `log` outputs (`parent`), snapshots (each session's `parent` index and
  `invoked_by`), and the replay log's `http` and `communication-error`
  records (the invoke-id `path` from the root); an `http-response` record
  names an invoked service's invocation by the same `path`.
- **ECMAScript XML values.** SCXML 1.0 B.2.1: "the Processor MUST create the
  corresponding DOM structure and assign it as the value of the variable."
  The DOM is a frozen, read-only DOM Core subset (node, element, attribute,
  text, comment, and processing-instruction read accessors and
  `getElementsByTagName[NS]`; `components/scxml-data-model-ecmascript/src/dom.rs`):
  no mutation method exists, so an XML value stays its exact XML text.
- **`<log expr>` with a side effect is rejected.** SCXML 1.0 §4.7.3: "The
  SCXML processor MUST ensure that <log> has no side-effects on document
  interpretation." The validator meets that duty by rejecting a `<log expr>`
  whose syntax certainly has a side effect (`SCXML074`, an error) and warning
  on one it cannot prove free of them (`SCXML075`); the run-time half is the
  known limitation L.1 of [spec-errata.md](spec-errata.md#l1-run-time-side-effects-of-log-expr).

### ECMAScript data model choices

Implementation-defined choices of the ECMAScript data model (each one a
latitude the cited text leaves to the implementation):

- Current time. ECMA-262 3rd edition 15.9.3.3: the `new Date()` object's
  value "is set to the current time (UTC)". As a host choice, the current
  time of a session is its clock's time, so a replay reads the times it
  recorded: it is the session clock's
  start epoch plus its logical time, the clock the session's delayed sends
  are scheduled on: a live session on a timed clock starts at the wall-clock
  time it opened and records that start as the first entry of its input log,
  a replay starts at the recorded time, and every other session (batch runs,
  the IRP runner, logical-clock sessions, `manual`-timer browser sessions)
  starts at 2015-09-01T00:00:00Z (`BATCH_START_EPOCH_MS`, the date of the
  SCXML 1.0 Recommendation), so they read the same times on every run.
- Local time zone. ECMA-262 3rd edition 15.9.1.9 leaves the local time zone
  adjustment to the implementation; it is always UTC (offset 0).
- `Math.random`. ECMA-262 3rd edition 15.8.2.14: "using an
  implementation-dependent algorithm or strategy". The n-th number of a
  session is the top 53 bits of SplitMix64(seed + (n + 1) *
  0x9E3779B97F4A7C15), where the seed is the 64-bit FNV-1a hash of
  `_sessionid`, drawn in the order the session's program calls it.
- `In()` argument. B.2.3 defines `In()` taking "a stateID"; the argument is
  converted with `ToString`, as ECMAScript built-ins that take a string do.
- Dynamic code. `eval` and `Function` refuse text beyond the data model's
  parse bounds with a `TypeError` (Boa implements a later edition, whose
  `HostEnsureCanCompileStrings` lets the host refuse a compilation; the 3rd
  edition has no such hook, so this is a resource limit, listed under
  [Resource limits](#resource-limits)). A function the
  `Function` constructor creates has the source text
  `function anonymous(<parameters>\n) {\n<body>\n}` (ECMA-262 3rd edition
  15.3.4.2 leaves `Function.prototype.toString` implementation-dependent).
- Snapshot display. A declared `<data>` id whose value has no portable form
  is shown by its ECMAScript `typeof` and a string form read without running
  user code: a function's source text, a `Date`'s `Date.prototype.toString`,
  `[object Tag]` with ECMA-262's built-in tag for any other object, and a
  primitive's `ToString` ([session-snapshot.md](session-snapshot.md)).

## Fuel

The Wasmtime session host bounds every host call with one fuel budget
(`DEFAULT_FUEL`, 3 000 000 000 instructions) shared by every component the call
reaches: the machine and each data-model, I/O processor, clock, BasicHTTP,
compiler, script, and child-machine call it makes are charged to the same budget
(Wasmtime call hooks move the remaining fuel between stores), so one call never
executes more instructions than that, deterministically. A session command is
one host call even when it defers `<invoke>` starts or cancels to the host's
loop: the command, every child start, cancel, and continuation carried out for
it, and the command's own continuation spend one budget, as the same work done
inside one uninterrupted call would (so a chain of invoked sessions started or
cancelled by one command needs fuel for every level; the 10,000-level
null-model test chain starts and cancels on 10 million per level). On an Apple
M3 Max this is roughly 0.22 to 0.25 s of Wasm, about 70 to 85 ms per billion
fuel: the heavy nested `<foreach>`/`<script>` document traps inside its
`start` after 0.23 to 0.25 s, and a 128-region parallel machine whose regions
toggle eventlessly traps after 505 commands and 0.22 to 0.25 s
(`default_fuel_runaway_call_time` prints them). Logical runaways (an eventless
transition loop, a `<raise>` loop, a 0 ms self-send chain) of narrower
machines, a 64-region one among them, stop earlier, at the machine's
1 024-command per-call bound, and a command budget the caller sets
(`--max-commands`; none by default) bounds the calls. The fuel default is
sized from the largest single call of each workload, the smallest fuel it runs
under, on documents `compile` accepts (`default_fuel_sizing_workloads` in
hosts/scxml-host-wasmtime/tests/fuel_sizing.rs bisects and prints them and
requires 1.5 times headroom of each; run it with
`--release -- --ignored --nocapture --test-threads 1`). The ECMAScript data model creates
its guarded interpreter realm once, at `initialize`, and keeps it for the whole
session, so an ECMAScript session's start costs about 0.11 billion and its later
calls far less: a `<foreach>` with an `<assign>` over 60, 90, and 1 000 items in
the start takes 0.11, 0.12, and 0.21 billion; eight non-assignment `<script>`s
in one transition and the representative machine's start run within the
start's 0.11 billion; and a 64-region parallel state whose regions all
transition takes 10 million (its start, which also covers one event). The
heaviest is the start of an ECMAScript root around a 4 000-state inline
`<invoke>` child, 1.73 billion. 3 000 000 000 admits the ECMAScript workloads
with at least 14 times headroom and the 4 000-state child with 1.72 times. The browser host charges every host
call to one budget shared by every component it reaches in the same way, in its
own coarser unit (function entries and loop iterations of the metered modules,
about 31 to 49 Wasmtime fuel per charge on these workloads); its `DEFAULT_FUEL`
of 75 000 000 charges, the Wasmtime default divided by 40, admits the same
workloads (the 1 000-item `<foreach>` takes 4.3 million, 17 times headroom,
and the 4 000-state child 22.4 million, 3.35 times) and stops the same heavy
document in about 0.3 s
([browser host README](../hosts/scxml-host-browser/README.md#fuel)).

## The Boa bounding gap

Boa 0.22 exposes no allocation hook, interrupt, or
native-recursion limit, so the ECMAScript data model bounds hostile
expressions structurally. Parse recursion is bounded before Boa sees any
text: a lexer-based scanner (`components/scxml-data-model-ecmascript/src/nesting.rs`,
exact for strings, template literals, comments, and escaped keywords; it
settles a `/` as a regular expression or division only where the grammar
leaves one reading, otherwise scans both readings and takes the deeper, and
declines a source needing more than 16 such forks) caps total structural nesting, including bracket depth,
statement nesting, conditional/assignment/arrow/unary chains, and left-nested
binary, member, call, and sequence chains, at 128 units on every parse path
(validator syntax check, compile, every runtime evaluate/execute/assign, and
runtime-compiled strings); portable JSON values and `JSON.parse` input are
capped at 32 nesting levels. Native builds also run every Boa parse on a
dedicated 64 MiB-stack thread. Inputs, `ArrayBuffer` allocations, recursion,
and the value stack are capped, and loop iterations are capped at 100 000
*per call frame* (Boa restarts the count in every frame, so nested calls
multiply it); a guard script
(`components/scxml-data-model-ecmascript/src/builtin_guard.js`) replaces the
single-call amplifying built-ins (`String.prototype.repeat`, `padStart`,
`padEnd`, `concat`, `replace`, `replaceAll`, `RegExp.prototype[@@replace]`,
`String.raw`, array `join`/`toString`/`toLocaleString`/`concat`/`flat`/
`flatMap`, length-driven `Array.prototype` methods, array iterators,
`Array.from`, `Function.prototype.apply`, `Reflect.apply`/`construct`, and
`JSON.stringify`) with versions that throw a catchable `RangeError` before Boa
allocates, iterates, or recurses past the value or nesting bound; `Proxy`,
`Promise`, `WebAssembly`, `Atomics`, and `SharedArrayBuffer` (none of them
part of the ECMAScript 3rd edition that SCXML B.2 requires) are removed from
the realm (using one raises `error.execution`; program text that merely
spells those names evaluates normally); `eval` and the `Function`
constructors compile runtime strings only within the same bounds (the host
hook ECMA-262 consults before compiling a string, `HostEnsureCanCompileStrings`,
applies the nesting scan and the script byte bound), so no source reaches
Boa without the nesting scan; and
every value that leaves the data model passes a portable-value bound. Operator-level
string concatenation and native iteration of user-defined iterables still
allocate inside Boa before that post-evaluation bound. CPU time is bounded
only by fuel: under Wasmtime by the store's per-call fuel (with its memory
limit), in the browser by build-time metering (see [Fuel](#fuel)); native (non-Wasm)
builds have no CPU or allocation bound beyond process limits, so nested-call
loops and catastrophic regular-expression backtracking can run long there.

## Resource limits

SCXML 1.0 Appendix A.2: "There is no conformance
requirement with respect to performance characteristics of the SCXML 1.0
Processor." The bounds below are this implementation's, not the
specification's, and **they apply only to wasm builds**: the shipped
components (validator, compiler, machine, clock, data models, I/O
processors) are wasm builds and keep them. Every native build, including
`scx`, the Wasmtime session host, `scxml-scheduler`, and any native
build of the workspace crates, has none of its own: a resource bound is
declared through `scxml_types::wasm_bound`, which is the bound on `wasm32`
and `usize::MAX` natively, and the native-only host and CLI caps are
removed. So a native build never rejects a document or ends or fails a
session because of a bound it imposes. A session always executes inside
the wasm components, so a native host still runs it under their bounds;
the browser host likewise adds none of its own beyond its network and
resource-fetch policy. In a wasm build, a conformant document that exceeds
one of them is treated as follows:

- **Raised as `error.execution`** (SCXML 1.0 §3.12.2: "it MUST signal any
  errors that occur by raising SCXML events whose names begin with
  'error.'"): a delayed send the session clock has no room for (1024
  pending sends, 4 MiB of retained payload); an expression or location over
  16 KiB, or a script over 64 KiB, which every bundled data model answers
  `invalid-input` (a condition is then false, SCXML 1.0 §5.9); an inline
  `<invoke><content>` whose document serializes past the 1 MiB string
  bound, which fails that invocation; and, on every build, an `<invoke>`
  beyond `InvokeLimits` the embedder chose (none by default).
- **Ending the session** (no error event can be raised for them): an
  internal event queue at its bound (10,000 events; the error event itself
  would have to join that queue), one engine command producing more than 10,000
  effects, an exhausted fuel budget (Wasmtime and browser metering; a
  macrostep that never ends, Appendix D: "A macrostep that does not
  terminate ... This is currently allowed"), and a command budget only when
  the caller sets one (`--max-commands`; none by default). One host call (a
  command, with every child start, cancel, and continuation it carries out)
  spends one fuel budget (by default 3,000,000,000 instructions under
  Wasmtime, 75,000,000 metered charges in the browser), so a finite command
  that starts a deep or heavy chain of invocations can exhaust it too.
- **Rejected by the validator and compiler components**: a document over a
  Machine IR, graph, or expression-checking bound (`SCXML027`, `SCXML028`,
  `SCXML040` for size, `SCXML041`, `SCXML069`, `COMP001`; the bounds are
  listed in [validator-rules.md](validator-rules.md)), including an
  `event:schema` over the 1 MiB string bound (`SCXML069`), and a JSON
  Pointer extraction that would bundle more than 256 referenced values
  (`SCXML092`). The native
  `scx` validates and compiles a document of any size and reports
  these codes for no size bound; it reports `SCXML028` only when graph
  analysis work grows faster than the machine (below).
- **ECMAScript bounds inside the language**: loop iterations (100,000 per
  call frame), the value stack (16,384), and the distinct state IDs one
  operation asks `In()` about (128) are bounded on wasm builds only, and
  the guarded built-ins throw a catchable `RangeError` before passing a
  bound; `eval` and `Function` refuse source beyond the parse bounds with a
  `TypeError`. ECMA-262 3rd edition, the edition SCXML 1.0 B.2 requires,
  grants no such latitude (its §16 lists the only permitted departures), so
  a program that reaches one of these bounds behaves differently from
  ECMA-262: it sees that exception, or, uncaught, `error.execution`.

### Bounds applied on every build

These apply on every build, because they are not resource bounds:

- **Native stack guards**: the XML element depth (256), the ECMAScript parse
  nesting (128 units, so `SCXML040` for nesting stays), recursion (128) and
  built-in value nesting (32), and the XPath parse nesting (32) and
  expression-tree depth (128). These parsers, walkers, and drops recurse
  once per level, so without a guard a deep input would overflow the stack
  and abort the process. Invocation needs no such guard: an invoked
  session never starts inside the call of the session that invokes it.
  That call suspends right after the `<invoke>` effect (WIT
  `invocation-progress.deferred`), the host starts (or cancels) the child
  from its own loop, and the invoking session then continues exactly as
  if the start had run inside its call (the engine's `Command::Continue`;
  SCXML 6.4). Starting, cancelling, dropping, and snapshotting a chain of
  invocations are iterative in both hosts, and ordering cross-session work
  does not grow with the nesting depth: each session takes its place in
  invocation order when it starts (an integer label), so taking the next
  queued event is logarithmic in the sessions with queued work, and
  cancelling or dropping a session costs its subtree (the Wasmtime
  scheduler also reads only the sessions a command ran on, and a session's
  clock moves only when the session is next used; the browser host does
  the same). Measured on an Apple M3 Max: a Wasmtime chain 10,000 sessions
  deep, every level's send to its invoking session queued at once, starts,
  delivers every event, cancels, and is dropped in about 70 s, in exactly
  four commands a level
  (`a_ten_thousand_deep_chain_with_every_level_queued_drains_in_bounded_commands`
  in hosts/scxml-host-wasmtime/tests/nested_invocation.rs, `cargo test`
  dev profile, on its 2 MiB test thread), nearly all of it the sessions'
  own compiles, loads, and starts; ordering and taking that chain's queued
  work costs the scheduler at most 12 n log2 n counted work steps (1,238,945
  for 10,000 levels, each doubling of the depth about 2.1 times the steps;
  `a_deep_chain_with_every_level_queued_is_taken_in_order_in_n_log_n_steps`,
  crates/scxml-scheduler, whose test-only step count compiles out of
  every other build) and about 20 ms in the browser
  host's queues (`ordering queued work costs no more than the queue and
  the tree: 10,000 levels each queued at once`,
  hosts/scxml-host-browser/test/capabilities.test.mjs, Node 24 over the
  release components). A browser session costs about 70 ms of component
  instantiation, so the browser test that runs a 300-level chain and a
  self-invocation until the command budget (`a chain of invocations runs
  from the host loop, not nested calls`, same file) takes about 35 s. A
  document that
  invokes itself without end nests until the caller's fuel or command
  budget ends the run. Each Wasmtime linear memory is exactly its size:
  one read-write mapping with no guard region, no reservation to grow
  into (it moves when it grows), and no copy-on-write image (its data
  segments are copied in), so the memories of live sessions merge into a
  few mappings. Linux allows a process 65,530 mappings by default
  (`vm.max_map_count`), and a guard, growth reservation, or image would
  make each memory three or four mappings that never merge, for every
  memory of every live session. Measured on
  Linux x86_64 (Docker, linux/amd64) by
  `a_live_waiting_session_adds_few_memory_mappings`
  (hosts/scxml-host-wasmtime/tests/linear_memory_mappings.rs, which prints
  them): 350 live waiting sessions add no mapping, and each costs about
  8.6 MiB of address space (its linear memories) and 0.3 MiB resident.
  Compiled code checks memory bounds explicitly. A memory that grows is
  copied to its new place by the host, and one host call may copy at most
  its fuel's worth of bytes (`MEMORY_GROWTH_COPY_BYTES_PER_FUEL`, one byte
  per fuel unit; 3,000,000,000 bytes on the default fuel), an allowance
  kept apart from the Wasm fuel, on every build: a copy the allowance
  cannot hold ends the call as fuel exhaustion does (errata L.4). Each
  64 KiB step of a memory growing from empty copies the whole memory, so
  the default allowance holds 302 steps, about 19 MiB, and a data model's
  own billion 174 steps, about 11 MiB (arithmetic, not a measurement). Measured by
  `memory_growth_copies_are_bounded_by_the_calls_copy_allowance`
  (hosts/scxml-host-wasmtime/tests/data_model_component.rs, release
  profile, Apple M3 Max, which prints them): a billion copied bytes take
  67 to 93 ms and a billion fuel of the test's ECMAScript loop 95 to
  121 ms; an expression that grows its memory 60 KB at a time toward the
  64 MiB store bound stops after 0.99 GB (about 0.07 s) on a billion
  fuel; and one that
  allocates 40,000 small objects copies 628 MB and completes on 577
  million fuel of Wasm, the same on any fuel. A 4,000-state inline
  `<invoke>` child of an ECMAScript root, whose compile and start copy
  their growing memories from the allowance, starts on the default fuel
  (`a_large_inline_child_starts_on_the_default_fuel` in
  hosts/scxml-host-wasmtime/tests/session_host.rs).
- **Amplification guards**: DTD entity expansion (64 KiB per reference,
  1 MiB per document, which also pays for the name and value of every
  attribute a default supplies, 1,048,576 entity references expanded into
  attribute defaults per document, 16 levels, 4096 processed markup
  declarations), XSD
  content-model expansion and schema-compilation work, the ECMAScript
  side-effect analysis (200,000 steps per checked fragment; past it the
  fragment is one undecidable `SCXML075` warning), the graph analysis work
  budget (8,000,000 units per machine, or 1,024 per element and target
  reference when that is more; past it `SCXML028`), the bundling of an
  `event:schema` a JSON Pointer selects (a result at most 8 times its file
  plus 4096 bytes, checked as the copies grow, and nested no deeper than
  the 128 levels the Machine IR decoder reads back; past either
  `SCXML092`), the 32 MiB rendered and SARIF report bounds, and the 64
  documents `snapshot run` and `run` load through `<invoke src>` before the
  first macrostep. Each guards against output that grows faster than its
  input.
- **Network-ingress protections of the CLI**: a document it fetches by URI
  (`<invoke src>`, `<script src>`, `<data src>`, `srcexpr`) is at most
  16 MiB over HTTPS, follows at most 5 redirects (each to `https:`), and
  shares the repository resolver's connect (10 s) and total (60 s)
  timeouts, with no retry.
- **Format and interchange contracts**: the WIT value bounds of events and
  data (for example 64 KiB per event payload or data value, raised as
  `error.execution`, and 1024-byte names: a literal event name, send id,
  or invoke id over it, or a state or invoke id over 1,013 / 1,012 bytes,
  whose `done.state.` or `done.invoke.` event would be over it, is a
  compile error,
  `SCXML081`/`SCXML082`, and a run-time `eventexpr` name or a generated
  invoke id over it raises `error.execution`), the `data-model-api` bounds of
  `initialize` (at most 256 declared `<data>` ids, whose placeholder
  encoding is at most 64 KiB, so every data model snapshot fits one
  `data-model-api.snapshot` image), the `scxml.session-snapshot/v2` bounds (255 invoked
  sessions, 32 nested invoke levels, 10,000 queue entries, 32 MiB per
  document; a snapshot of a session past one fails and the session goes
  on), the validator and compiler requests' `event-schemas` (64 schema
  files, 4 MiB each, 16 MiB together; a request past one is answered with
  the WIT `invalid-request` code, without diagnostics, so an embedder
  that sends schema files checks these bounds first; the Wasmtime and
  browser hosts send none), the 32 MiB canonical report, and the
  `u32` Machine IR indices.
- **Specification**: a duplicate author `<send id>` while that send is
  pending ends the session (SCXML 1.0 §3.14: ids "MUST be unique within the
  session"; a document that repeats one is not conformant and the
  specification gives it no behavior).
- **Network ingress and artifact authentication**: the BasicHTTP listener
  and client bounds, and the component, manifest, and script byte bounds
  that authenticate artifacts.
