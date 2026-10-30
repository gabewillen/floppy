# SCXML specification errata

The W3C SCXML 1.0 Recommendation (2015-09-01, "REC") is the normative text
this project implements; the XPath data model is defined only by the
Candidate Recommendation of 2014-03-13 ("CR", §C.3), because the REC dropped
it. The W3C SCXML Implementation Report Plan corpus ("IRP", manifest
2015-03-10, bundled under `tests/conformance/w3c-irp`) is the conformance
oracle (see [conformance.md](conformance.md)).

In the few places below, the IRP requires behavior the specification text
does not state, or contradicts. The maintainers' rule for these cases is
**follow the IRP and record the erratum here**: the IRP is the only
executable statement of the W3C's intent, and a processor that followed
the text instead would fail official cases. Each entry quotes both texts exactly, names the files and lines, and
states what this implementation does. The traceability status of the
requirements concerned is "documented spec-text inconsistency (follows the
IRP)", not a pass of the text.

This file is the complete list. Any other difference between this
implementation and the specification text is a defect; the known ones are
listed at the end, under "Known limitations (pending)".

## 1. `origintype` of an SCXML Event I/O Processor event

- **REC C.1** (SCXML Event I/O Processor), item 4: "The 'origintype' field
  of the event raised in the receiving session MUST have the value
  "scxml"."
- **IRP 352** (mandatory, `upstream/sources/352/test352.txml:3`): "test the
  origintype is 'http://www.w3.org/TR/scxml/#SCXMLEventProcessor'", checked
  at `test352.txml:24` with
  `conf:idQuoteVal="1=http://www.w3.org/TR/scxml/#SCXMLEventProcessor"`. Its
  manifest assertion (`upstream/manifest.xml:1337-1338`) is the CR wording:
  "'sourcetype'. The sending SCXML Processor MUST assign this attribute the
  value "scxml". ... The receiving Processor MUST use this value as the value
  of the 'origintype' field of the event that it generates."
- **REC 5.10.1** also says origintype "is equivalent to the 'type' field on
  the <send> element" and SHOULD allow a reply "via the Event I/O Processor
  specified in 'origintype'", and REC 6.2.5 makes
  `http://www.w3.org/TR/scxml/#SCXMLEventProcessor` the processor's type
  value ("scxml" is only an optional short form, "Processors MAY define
  short form notations as an authoring convenience").

**This implementation** sets `origintype` to the type URI
`http://www.w3.org/TR/scxml/#SCXMLEventProcessor` (or the exact type string
the author wrote on `<send>`), as IRP 352 requires
(`components/scxml-ioprocessor-scxml/src/lib.rs`, `dispatch`). A processor that
set "scxml" would fail mandatory case 352.

## 2. The event name of a `BasicHTTP` message without `_scxmleventname`

- **REC C.2.1**: "If the parameter '_scxmleventname' is not present, the
  SCXML Processor MUST use the name of the HTTP method that was used to
  deliver the message as the name of the event that it raises." The method
  is `POST` (C.2.1: messages arrive "as HTTP POST requests").
- **IRP 532** (optional, `upstream/sources/532/test532.txml:16`):
  `<transition event="HTTP.POST" conf:targetpass=""/>`, while its manifest
  assertion (`upstream/manifest.xml:1438-1439`) restates the text ("the name
  of the HTTP method"). IRP 520 (`test520.txml:16-17`) also expects
  `HTTP.POST`. A transition on `POST` does not match `HTTP.POST` (event
  descriptors match token prefixes, 3.12.1), so the two cannot both be
  satisfied.

**This implementation** names such an event `HTTP.POST`
(`components/scxml-ioprocessor-basichttp/src/lib.rs`, `DEFAULT_EVENT_NAME`),
as IRP 520 and 532 require.

## 3. XPath system variables are named elements, not `<data id>` elements

- **CR C.3.7**: "For each system variable defined in 5.10 System Variables,
  the SCXML Processor MUST create a data model <data> element with an 'id'
  attribute whose value is the name of the system variable. The Processor
  also MUST bind an XPath variable whose name is the name of the system
  variable to this element."
- **IRP 354** (mandatory), XPath specialization: state `s3` has
  `<transition event="event2" cond="_event/data = 123" target="pass"/>`
  (`templates/xpath.xsl:527-529`, `conf:eventdataSomeVal`). That relative
  path selects a child element *named* `_event` of the context node; under
  the CR representation (`<data id="_event">`) it selects nothing and the
  case fails.

**This implementation** represents `_sessionid`, `_name`, `_ioprocessors`,
and `_event` as elements with those names (`<_sessionid>`, `<_name>`,
`<_ioprocessors>`, `<_event>`) bound to `$_sessionid`, `$_name`,
`$_ioprocessors`, and `$_event` (`components/scxml-data-model-xpath`).
Access through the variables (`$_event/name`) is identical under either
representation; only the relative path IRP 354 uses differs.

## 4. The `raw` field of `_event`

- **REC 5.10.1** lists the fields "present in all events": `name`, `type`,
  `sendid`, `origin`, `origintype`, `invokeid`, and `data`. Neither the REC
  nor the CR defines a `raw` field.
- **CR C.3.7**: "for each of the fields defined in 5.10.1 The Internal
  Structure of Events, the Processor also MUST create an child element whose
  name is the name of the field".
- **REC 5.10**: "Platforms MUST place all platform-specific system variables
  under the '_x' root."
- **IRP** 178 (`conf:eventRaw`, `test178.txml:21`), 509
  (`conf:methodIsPost`, `test509.txml:14`), 518
  (`conf:eventIdParamHasValue`, `test518.txml:17`), 519 and 534
  (`conf:eventNamedParamHasValue`, `test519.txml:15`, `test534.txml:14`),
  and 520 (`conf:messageBodyEquals`, `test520.txml:16-17`) read the raw
  message as `_event.raw` in the ECMAScript template
  (`templates/ecmascript.xsl:203-206`, `:673-674`, `:692-706`) and as
  `$_event/raw` in the XPath template (`templates/xpath.xsl:296-299`,
  `:336-338`, `:357-374`).

**This implementation** exposes the raw message as `_event.raw`
(ECMAScript) and as a `<raw>` child of `<_event>` after the 5.10.1 fields
(XPath), empty or undefined when the event has no raw form, as those cases
require. A `BasicHTTP` event's raw form is the message as received. The SCXML
Event I/O Processor has no wire format (C.1 defines none), so its events
carry the message the processor would post as a `BasicHTTP` form body
(C.2.1, `application/x-www-form-urlencoded`): `_scxmleventname=<name>`,
then each `<param>` and `namelist` entry in document order with duplicates
kept (IRP 178), or `_scxmlcontent=<content>` for `<content>`; for example
`_scxmleventname=ext&p=1&q=a+b`. Internal events (`<raise>`, platform
events) have none.

## 5. Side effects in ECMAScript conditions

- **REC Appendix B**, on what "the definition of a data model MUST" do:
  "Specify the boolean expression language used as the value of the 'cond'
  attribute in <transition>, <if> and <elseif> This language MUST not have
  side effects and MUST include the predicate 'In'".
- **REC 5.9.1**: "Conditional expressions in conformant SCXML documents
  SHOULD NOT have side effects."
- **REC B.2.3** makes ECMAScript conditions ECMAScript expressions, which can
  assign.
- **IRP** 444 (`test444.txml:12`, assertion 444, specnum B.2, "optional")
  writes `<transition cond="++var1==2" conf:targetpass=""/>` and passes only
  when the condition increments `var1` and compares the result.

**This implementation** follows the IRP: an ECMAScript condition runs as
ECMAScript, side effects included, and a condition whose syntax certainly
has a side effect is the validator warning `LINT020` (the document SHOULD
NOT, per 5.9.1), not an error, so test 444 compiles and passes. A `<log
expr>` with a side effect stays the error `SCXML074`: REC 4.7.3 ("The SCXML
processor MUST ensure that <log> has no side-effects on document
interpretation.") is contradicted by no official case.

## 6. The value of a `<content>` whose `expr` fails

- **REC 5.6.2**: "If the evaluation of 'expr' produces an error, the
  Processor MUST place error.execution in the internal event queue and use
  the empty string as the value of the <content> element." The manifest
  assertion of IRP 528 (`upstream/manifest.xml:468-469`) restates it ("use
  the empty string as the output of the content element").
- **IRP 528** (mandatory, `upstream/sources/528/test528.txml:18`, `:28`):
  `<donedata><content conf:illegalExpr=""/></donedata>`, then
  `<transition event="done.state.s0" conf:emptyEventData="" conf:targetpass=""/>`.
  The ECMAScript template specializes `conf:emptyEventData` to
  `typeof _event.data === 'undefined'` (`templates/ecmascript.xsl:446-448`),
  which an empty string fails (`typeof '' === 'string'`). The XPath template
  specializes it to `not($_event/data/*)` (`templates/xpath.xsl:531-533`),
  which both values satisfy.

**This implementation** raises `error.execution` and then the
`done.state.<id>` event with no data (ECMAScript `undefined`, an empty XPath
`<data/>`), as IRP 528 requires (`crates/scxml-engine/src/interpret/mod.rs`,
the `Effect::DoneEvent` arm). The question arises only for `<donedata>`:
for `<send>`, REC 6.2 governs ("If the evaluation of <send>'s arguments
produces an error, the Processor MUST discard the message without attempting
to deliver it"), and an `<invoke>` whose content fails is not started.

## 7. `<send>` with both `event` and `<content>`

- **REC 6.2.3**: "A conformant SCXML document MUST specify exactly one of
  'event', 'eventexpr' and <content>."
- **IRP** 179 and 354 (mandatory) and 561, 562 and 578 (optional) write
  `<send event="...">` with a `<content>` child
  (`upstream/sources/179/test179.txml:9-11`,
  `354/test354.txml:43-45`, `561/test561.txml:11-12`,
  `562/test562.txml:11-12`, `578/test578.txml:11-12`) and pass only when
  the event arrives under that name with the content as `_event.data`.

**This implementation** accepts the combination (the validator reports
nothing; `docs/validator-rules.md`, "Event identity may combine with any
legal payload form") and sends the named event with the content as its
data, as those cases require.

## 8. An XPath `<foreach>` item variable denotes the collection's node

- **CR C.3.1**: "For each <data> element in the document, the SCXML
  Processor MUST create a child of <datamodel> called <data> ... The
  Processor MUST also bind an XPath variable of the same name to that
  datamodel <data> element."
- **CR C.3.10**: "In the XPath data model, the legal iterable collections
  are Node-Sets." **REC 4.6**: "The SCXML processor MUST act as if it has
  made a shallow copy of the collection produced by the evaluation of
  'array'. ... For each item in turn, the processor MUST assign it to the
  item variable." Neither text says what assigning a node to a variable
  bound to a `<data>` element does.
- **IRP 153** (mandatory, `upstream/sources/153/test153.txml:10`, `:18-20`)
  declares the item variable as `<data conf:id="2"/>`, iterates
  `conf:arrayVar="3"` (`$Var3/*`, three `<node>` elements,
  `templates/xpath.xsl:156-163`), and compares `conf:compareIDVal="1<2"`,
  which reads `$Var2/text()` (`templates/xpath.xsl:433-437`); IRP 155
  (`test155.txml`) sums the same reads. Only a `$Var2` that is the `<node>`
  element itself has that text: a `<data id="Var2">` holding a copy of the
  node, as a C.3.6 `replacechildren` assignment would leave it, has no text
  child, and both cases fail.

**This implementation** makes the item variable denote the collection's own
node (a shallow copy of a node-set holds the same nodes), so an assignment
through `$item` changes the collection, as one through an ECMAScript item
changes the array element. The declared `<data id>` element stays in
`<datamodel>` with its value; a later binding outside a loop (a
`<foreach index>`, or an item no loop expects) replaces its children and
makes `$name` denote it again (`components/scxml-data-model-xpath/src/model.rs`,
`bind_variable`; `src/tests.rs`,
`foreach_items_are_the_collection_nodes_and_keep_declared_data`).

## 9. The XPath `_ioprocessors` entry named `#SCXMLProcessor`

- **REC C.1**: "SCXML Processors MUST maintain a
  'http://www.w3.org/TR/scxml/#SCXMLEventProcessor' entry in the
  _ioprocessors system variable." REC 6.2.5 names no processor
  `http://www.w3.org/TR/scxml/#SCXMLProcessor`.
- **IRP 500 and 501** (mandatory, `upstream/sources/500/test500.txml:7`,
  `501/test501.txml:7`) read the SCXML processor's location through
  `conf:scxmlEventIOLocation`. The ECMAScript template reads the entry the
  REC names (`templates/ecmascript.xsl:224-225`,
  `_ioprocessors['http://www.w3.org/TR/scxml/#SCXMLEventProcessor']`), but
  the XPath template reads
  `$_ioprocessors/processor[@name="http://www.w3.org/TR/scxml/#SCXMLProcessor"]/location/text()`
  (`templates/xpath.xsl:316-319`), a name no specification defines. Under
  the REC entries alone that expression selects no node, so `Var1` holds no
  location and both cases fail.

**This implementation** follows the IRP: the XPath data model also lists a
`<processor name="http://www.w3.org/TR/scxml/#SCXMLProcessor">` whose
location is the `#SCXMLEventProcessor` entry's
(`components/scxml-data-model-xpath/src/lib.rs`,
`XPATH_TEMPLATE_PROCESSOR_NAME`). It is the only `_ioprocessors` name no
specification text gives, and only XPath documents see it; the ECMAScript
and null data models list exactly the processors the REC names and their
aliases.

## Where the schema declares what the prose does not

### S.1 `<scxml exmode>`

- **Schema**: `scxml-module-core.xsd:63` declares
  `<xsd:attribute name="exmode" type="Exmode.datatype"/>` on `<scxml>`,
  and `scxml-datatypes.xsd:18-24` documents `Exmode.datatype` as "the
  processor execution mode for this document, being either "lax" or
  "strict"".
- **REC**: the attribute table of §3.2.1 (`<scxml>` Attribute Details)
  lists `initial`, `name`, `xmlns`, `version`, `datamodel`, and `binding`;
  the Recommendation never mentions `exmode` or an execution mode.
- **This implementation** accepts `exmode` with either value (the
  validator checks the value against the schema) and gives it no effect:
  `compile` lowers a document with it to the same Machine IR as without it
  (`exmode_validates_and_lowers_with_no_effect` in
  `crates/scxml-compiler-core/tests/validate_covers_lowering.rs`).

## Where the normative prose and the informative Appendix D differ

The REC says of its Appendix D: "[This section is informative.] This
section contains an illustrative algorithm for the interpretation of an
SCXML document. It is intended as a guide for implementers only.
Implementations are free to implement SCXML interpreters in any way they
choose." Conformance is to the normative sections (REC A.2: "A Conforming
SCXML 1.0 Processor MUST support the syntax and semantics of all mandatory
SCXML elements described in this document"). Where the two differ, this
implementation follows the normative prose and records the difference here.
No official IRP case distinguishes them.

### D.1 Order of transition content in one microstep

- **REC 3.13**: "To execute a set of transitions, the SCXML Processor MUST
  first exit all the states in the transitions' exit set in exit order. It
  MUST then execute the executable content contained in the transitions in
  document order. It MUST then enter the states in the transitions' entry
  set in entry order."
- **Appendix D** `selectTransitions` adds each transition to an ordered set
  while visiting "atomicStates = configuration.toList().filter(isAtomicState)
  .sort(documentOrder)", and `executeTransitionContent` runs "for t in
  enabledTransitions: executeContent(t)", so the content runs in the order
  of the atomic states that selected the transitions. The two orders differ
  when a transition of an ancestor precedes, in the document, a transition
  that an earlier atomic state selected (a `<parallel>` whose own
  transition is written before its regions).

**This implementation** runs transition content in the document order of
the `<transition>` elements (`crates/scxml-engine/src/lib.rs`,
`apply_microstep`; `crates/scxml-engine/tests/appendix_d.rs`,
`transition_content_runs_in_the_document_order_of_the_transitions`).

### D.2 Default history content when the history's parent stays active

- **REC 3.10.2**: "If it has not, the SCXML processor MUST behave as if the
  transition had taken the default stored state configuration for that
  history state as its target. The Process MUST execute any executable
  content in the transition after the parent state's onentry handlers, and,
  in the case where the history pseudo-state is the target of an <initial>
  transition, the executable content inside the <initial> transition."
- **Appendix D** `addDescendantStatesToEnter` records
  "defaultHistoryContent[state.parent.id] = state.transition.content", and
  `enterStates` runs it only for an entered state: "if
  defaultHistoryContent[s.id]: executeContent(defaultHistoryContent[s.id])".
  When a transition targets a history pseudo-state whose parent is the
  transition's domain (for example `s1` targeting its parent `s`'s history,
  or an internal transition of `s` targeting it), `s` is not entered and
  Appendix D drops the content.

**This implementation** follows 3.10.2 and runs the content. The REC
places it only "after the parent state's onentry handlers", which ran when
the parent was last entered; as an implementation-defined choice the
content runs where the parent's entry would end, before the first state
entered below the parent, the same position it has when the parent is
entered (`crates/scxml-engine/src/lib.rs`, `flatten_entry_effects`;
`crates/scxml-engine/tests/appendix_d.rs`,
`default_history_content_runs_when_the_parent_stays_active`).

## Known limitations (pending)

Unlike the entries above, these are departures from the specification
text that no IRP case requires, recorded here until a change closes them:
L.1 and L.2 need a change to the bundled ECMAScript engine; L.3 is an
address only an embedder can provide; L.4 and L.5 are this
implementation's resource bounds (SCXML 1.0 Appendix A.2: "There is no
conformance requirement with respect to performance characteristics of the
SCXML 1.0 Processor"), which apply to wasm builds only; L.6 lists every
document and `<invoke>` a bound refuses. The traceability
matrix marks the requirements concerned as partial, not covered.

### L.1 Run-time side effects of `<log expr>`

- **REC 4.7.3**: "The SCXML processor MUST ensure that <log> has no
  side-effects on document interpretation."
- **This implementation** enforces it only statically: the validator
  rejects a `<log expr>` whose syntax certainly has a side effect
  (`SCXML074`, an error) and warns on one that may have one (`LINT019`).
  At run time the expression is evaluated as written, so a side effect the
  analysis cannot see (one reached through a call the analysis cannot
  resolve) still changes the data model. Enforcing it at run time needs a
  write barrier in the ECMAScript engine.

### L.2 Deleting a system variable by computed or reflective means

- **REC 5.10**: "The Processor MUST cause any attempt to change the value of
  a system variable to fail and MUST place the error 'error.execution' on
  the internal event queue when such an attempt is made."
- **This implementation** fails every assignment to a system variable with
  `error.execution`, and the validator reports every static write it can see
  (`SCXML077`, a warning). A deletion it cannot see statically, a non-strict
  `delete` reached through `eval` or `Function` (`eval('delete _event')`) or
  a reflective one with a computed key
  (`Reflect.deleteProperty(globalThis, name)`), fails as ECMAScript defines
  for a non-configurable property: it returns `false`, and the variable
  keeps its value, but no `error.execution` is raised. Raising it needs a
  hook in the ECMAScript engine.

### L.3 A `BasicHTTP` location no external entity can use

- **REC C.2.3**: "The Processor MUST maintain a 'location' field inside this
  entry whose value holds an address that external entities can use to
  communicate with this SCXML session using the Basic HTTP Event I/O
  Processor."
- **This implementation** lists a location for every session. Unless the
  embedder provides an address (`scx run --http-listen` with
  `--http-public-url`, or the browser host's `http.location`), it is
  `http://basichttp.loopback.invalid/sessions/<id>/basichttp`, an address
  under the reserved `.invalid` domain (RFC 6761) that only the run's
  in-process loopback answers: batch runs, IRP runs, and browser sessions
  without `http.location` have no address an external entity can reach.

### L.4 Resource bounds that end a session instead of raising an error event

- **REC 3.12.2**: "Once the SCXML processor has begun executing a
  well-formed SCXML document, it MUST signal any errors that occur by
  raising SCXML events whose names begin with 'error.'."
- **This implementation** (wasm builds, which every session runs in) ends
  the session, with no error event, when its internal event queue reaches
  10,000 events (the error event itself would have to join that queue),
  when a host's fuel budget
  is exhausted (a macrostep that never ends; Appendix D: "A macrostep that
  does not terminate ... This is currently allowed"), when one host call's
  linear memory growth copies exceed its copy allowance (under Wasmtime,
  on every build, its fuel's worth of bytes, 3,000,000,000 by default; a
  memory growing from empty in 64 KiB steps reaches about 19 MiB on it;
  the browser host has no such copy, its engine grows memory in place), or
  when a command budget the caller set is spent. One host call (a command, with every
  child start, cancel, and continuation it carries out) spends one fuel
  budget (by default 3,000,000,000 instructions under Wasmtime and
  75,000,000 metered charges in the browser), so a finite command that
  starts a deep or heavy chain of invocations can exhaust it too. [limits.md](limits.md#resource-limits)
  lists these bounds.

### L.5 ECMAScript programs that reach a resource bound

- **REC B.2.10**: "The SCXML Processor MUST accept any ECMAScript program as
  defined in Section 14 of [ECMASCRIPT-262] as the content of a <script>
  element."
- **This implementation** runs every program in the session's one realm,
  but a program that reaches a bound runs differently from ECMA-262: the
  guarded built-ins throw
  a catchable `RangeError` before passing a bound, and `eval` and
  `Function` refuse source beyond the parse bounds with a `TypeError`; on
  every build, the native stack guards (128 parse nesting units, recursion
  depth 128, built-in value nesting 32) do the same. Uncaught, each is
  `error.execution`. ECMA-262 3rd edition, the edition SCXML 1.0 B.2
  requires, grants no such latitude (its §16 lists the only permitted
  departures).

### L.6 Documents and invocations a bound refuses

- **REC A.2**: "A SCXML 1.0 processor is a user agent that can parse and
  process Conforming SCXML 1.0 documents." **REC 6.4.2**: "Otherwise the
  Processor MUST start a new logical instance of the external service
  specified in 'type' or 'typeexpr', passing it the URL specified by 'src'
  or the data specified by <content>, \"namelist\" or <param>."
- **This implementation** refuses a conformant document, or fails one
  `<invoke>` with `error.execution`, only for these bounds:
  - native stack guards, on every build: an XML element nesting deeper
    than 256 (`XML002`); an ECMAScript fragment over 128 structural nesting
    units, where each open bracket costs 8 (16 brackets, 15 inside a value
    expression), or recursion past 128 (`LIMIT003`, and at run time
    `error.execution`); an XPath expression nested deeper than 32
    parentheses, predicates, or arguments, or a tree deeper than 128
    (`LIMIT003`). The native validator applies exactly the bounds of the
    wasm data models every session runs in, so `compile` refuses what
    every session would decline;
  - the XML parser's amplification guards, on every build (`XML002`): an
    internal DTD subset of more than 4,096 markup declarations (counting
    each one a parameter-entity inclusion repeats), an entity whose
    replacement text expands past 64 KiB, entity references that expand
    past 1 MiB in one document or nest deeper than 16, and attribute-default
    normalization that expands more than 1,048,576 entity references;
  - the semantic graph analysis's work budget, on every build
    (`LIMIT002`): a machine whose reachability, target-legality, and
    liveness checks need more than 8,000,000 work units (or 1,024 per
    element and target reference, when that is more);
  - the bundled data models' variable bounds, on every build: a document
    with more than 256 distinct `<data>` ids, or whose ids' placeholder
    encoding exceeds 64 KiB, compiles, but its session fails at `start`
    with `invalid-input` (the data model's `initialize` declines it)
    before any state is entered;
  - in wasm builds only, the validator's and compiler's size bounds
    (`LIMIT001`, `LIMIT004`, `LIMIT005`, `COMP001`, `LIMIT003` for size;
    [validator-rules.md](validator-rules.md) lists them);
  - `SCXML074`: a `<log expr>` whose syntax certainly has a side effect,
    which REC 4.7.3 forbids the processor to let happen (see L.1);
  - an `<invoke>` beyond the `InvokeLimits` the embedder chose (none by
    default), which raises `error.execution`. The host imposes no invoke
    depth or count of its own: an invoked session never starts inside the
    call of the session that invokes it, and a chain of any depth runs,
    given fuel for every level (the command that starts the chain spends
    one fuel budget on all of them, see L.4);
  - a `<send eventexpr>` value that is not a name, which fails its `<send>` with
    `error.execution` (REC 3.12.2: "it MUST signal any errors that occur
    by raising SCXML events whose names begin with 'error.'"); none of
    these ends the session;
  - in wasm builds only, inline `<invoke><content>` that serializes into a
    child document of more than 1 MiB (the machine's string bound), which
    fails that `<invoke>` with `error.execution`.
