# Validator semantic rule inventory

The validator is the primary authoring product. Diagnostics are designed for an
AI repair loop: each rule states what is wrong, labels the responsible bytes,
explains why, gives a concrete repair direction, and cites the governing SCXML
section where available. XSD findings remain in the same canonical artifact.

The comparison baseline is the semantic rule inventory of the Go validator in
the separately maintained AgentML project (`validator/semantic_rules.go` in
[agentml-go](https://github.com/agentflare-ai/agentml.go)). It is an external reference only and is not needed to
build, test, or verify this repository. Its semantic inventory is complete in
the table below; AgentML XSD enhancement and recursive filesystem invocation are
separate boundary features rather than SCXML semantic rules.

| AgentML rule | Rust stable code(s) | Status | Notes |
| --- | --- | --- | --- |
| E301 ID NCName | SCXML003 | Supported with normative XML NCName character ranges | Checks the schema value of `id` (white space collapsed, see [schema values](#schema-values-of-white-space-collapsing-attributes)), labels it, and offers a maybe-incorrect sanitized replacement; `:` is excluded while XML `NameStartChar` ranges such as U+200C/U+200D are accepted. |
| E302 event descriptor | SCXML013 | Supported | Detects empty lists, commas, malformed tokens, and offers comma-to-space repair. Accepts the SCXML wildcard descriptors `*` and `.*`, matching engine behavior. |
| E310 param name and expr/location XOR | SCXML007 | Supported | Separate missing-name, missing-value-source, and conflicting-attribute diagnostics. |
| E311 cancel sendid/sendidexpr XOR | SCXML008 | Supported | Labels both conflicting attributes and offers a maybe-incorrect removal of `sendidexpr`. |
| E312 send source alternatives | SCXML014 | Corrected and extended | Enforces the actual attribute alternatives (`event`/`eventexpr`, `target`/`targetexpr`, `type`/`typeexpr`, `delay`/`delayexpr`, and `id`/`idlocation`), each with a maybe-incorrect removal of the second form. Event identity may combine with any legal payload form. |
| E313 send payload conflicts | SCXML015 | Supported | Accepts compatible `namelist` plus `param`; rejects `content` combined with either payload source and labels both sides. |
| E314 invoke source alternatives | SCXML016 | Supported and extended | Checks `src`/`srcexpr`, `type`/`typeexpr`, `id`/`idlocation`, and external source versus inline `content`. Initialization data from `namelist` and `param` may combine with inline source content. |
| E315 donedata content/param | SCXML018 | Supported | Labels both conflicting children and offers maybe-incorrect removals of either kind; the schema's content-model finding for the same child is superseded. |
| E320 initial exactly one transition | SCXML009 | Supported | Reports the observed transition count; with more than one, the first extra transition is primary and a maybe-incorrect fix keeps only the first. |
| E330 initial target and no event/cond | SCXML010 | Supported with machine-applicable attribute removal | Event and condition are diagnosed independently; an empty or blank `target` counts as missing. A `type` is accepted: SCXML 3.6.2 forbids only "'cond' or 'event' attributes" on this transition. |
| E331 initial target descendant | SCXML022 and SCXML017 | Supported | Covers explicit `<initial>` transitions and `initial` attributes on root/state. |
| E332 shallow history immediate child | SCXML021 | Supported | `type="deep"` accepts deeper descendants. |
| E333 transition event/cond/target/executable content | SCXML004 | Supported | Does not misapply the general rule to initial/history transitions. A transition with neither `event`, `cond`, nor `target` is an error even when it has executable content: SCXML 1.0 §3.5.2 "A conformant SCXML document MUST specify at least one of 'event', 'cond' or 'target'." |
| E334 state initial attribute/child conflict | SCXML011 | Supported and extended to `<scxml>` | Labels both forms. |
| E335 atomic state initial misuse | SCXML012 | Supported | Covers attribute or child initial configuration. |
| W340 conditional-only deadlock | SCXML025 | Narrow data-model-neutral lint analogue | Warns only when every exit is an eventless transition with a `cond`; ordinary reactive states with event-triggered exits and invoke states are valid wait patterns and are not diagnosed. The warning identifies a possible indefinite conditional wait, not invalid SCXML or a proven runtime deadlock. |
| E341 unconditional transition cycle | SCXML026 | Supported with deterministic cycle labels | Detects direct targeted eventless, conditionless state cycles represented reliably by the parsed tree. |

Additional Rust rules not present as AgentML semantic rule objects:

- `SCXML001` validates the exact W3C SCXML root expanded QName. A `<scxml>` root without any namespace gets a machine-applicable `xmlns` declaration; a near-miss namespace (trailing slash, `https`, letter case) labels the declared URI with a machine-applicable exact-URI replacement, and the missing-schema `XSD001` for that URI is not also reported.
- `SCXML002` reports duplicate IDs with the first declaration as a secondary label. Each nested inline `<invoke><content><scxml>` root has an independent ID namespace, so duplicate IDs across roots are legal.
- `SCXML005`/`SCXML006` report unknown target/initial IDs with deterministic typo suggestions and structured replacement edits for single-token values. Suggestions use optimal-string-alignment distance (an adjacent transposition is one edit) with a length-scaled threshold (1 edit up to 3 characters, 2 up to 7, otherwise 3), ties broken by the smallest name. The search is bounded to 128 Unicode scalar values per identifier, 65,536 candidates, 4,000,000 work units per lookup, and 32,000,000 per machine. A suggestion is exact or absent: when any bound cuts a search short, no suggestion is offered, so a closer candidate that was never examined can never lose to a worse one.
- `SCXML005`/`SCXML006` explain whitespace-split references to an invalid ID (`initial="1st state"` against `id="1st state"`): each piece is labeled as part of the whole reference, and the `SCXML003` rename fix edits the declaration and every reference in one maybe-incorrect fix.
- `SCXML005` reports a comma-separated `target` (`target="r1b,r2b"`) whose comma-separated pieces all name states as one finding: target IDs are whitespace-separated, and a machine-applicable fix replaces the commas with spaces, as `SCXML013` does for `event` and `SCXML039` for `namelist`.
- `SCXML005`/`SCXML006` resolve references only inside their containing SCXML document root. Parent transitions cannot target child-machine IDs, and child transitions cannot target parent-machine IDs.
- `SCXML027` fails closed when an inline-machine path exceeds 64 SCXML document roots, including the outer root. Traversal is iterative; XML parsing independently retains its configured depth and node budgets.
- `SCXML014` additionally checks `target`/`targetexpr`, `type`/`typeexpr`, `delay`/`delayexpr`, and `id`/`idlocation` on `send`.
- `SCXML052` reports a `<send>` handled by the SCXML Event I/O Processor (no `type`, or exactly `http://www.w3.org/TR/scxml/#SCXMLEventProcessor`) with neither `event` nor `eventexpr` (SCXML 1.0 §6.2, §C.1), with a has-placeholders `event="…"` insertion. A `typeexpr` or another processor type is not judged.
- `SCXML053` reports `<data>` with more than one of `src`, `expr`, and child content (SCXML 1.0 §5.3), `<content expr>` with child content (§5.6), and `<assign expr>` with child content (§5.4); it labels every source and offers a maybe-incorrect removal of the conflicting attribute(s).
- `SCXML074` (ECMAScript, error), `SCXML076` (ECMAScript, warning), and `SCXML075` (ECMAScript, warning) enforce the side-effect rules. SCXML 1.0 §4.7.3 says "The SCXML processor MUST ensure that <log> has no side-effects on document interpretation.", §5.9.1 says "Conditional expressions in conformant SCXML documents SHOULD NOT have side effects.", and Appendix B requires a data model's condition language to have none ("This language MUST not have side effects"), which official IRP test 444 (`cond="++var1==2"`) contradicts: the project follows the IRP ([spec-errata.md](spec-errata.md) §5). Those are the only side-effect requirements on expressions, so exactly `cond` on `<transition>`, `<if>`, and `<elseif>` and `<log expr>` are checked; `<data expr>`, `<assign expr>`, `<param expr>`, `<send>`/`<invoke>` `*expr`, and `<foreach array>` may have effects as ECMAScript defines. The linked data model's parse-only `check-effects` analysis parses every fragment of the machine with the same parser and wrappers as evaluation and never evaluates anything. Syntax that certainly changes state outside the expression when it runs is `SCXML074` in a `<log expr>` and `SCXML076` in a condition (for a condition that is exactly one plain `=`, labeled at the operator with a maybe-incorrect `===` fix): an assignment, compound assignment, `++`/`--`, or `delete` of a variable or property outside it; a built-in method ECMA-262 defines to change its receiver (`push`, `pop`, `shift`, `unshift`, `splice`, `sort`, `reverse`, `fill`, `copyWithin`, `Map`/`Set` `set`/`add`/`delete`/`clear`, `Date` and `DataView` setters, iterator `next`) on a value reachable from outside the expression, or its argument (`Object.assign`, `Object.defineProperty`/`defineProperties`/`setPrototypeOf`/`freeze`/`seal`/`preventExtensions`, the `Reflect` writers, `Array.prototype.push.call(target, ...)`); `Math.random()`, whose session sequence advances (the data model's strategy, `components/scxml-data-model-ecmascript/tests/random.rs`); and a call or `new` of a document function (a static `<script>` declaration or a `<data expr>` function the document never reassigns) whose body, followed through the calls it makes, does one of those, with a secondary label at the write in the script. Writes to the expression's own locals, to parameters bound to fresh objects, and to objects the expression creates are not effects, so `(function (s) { s = String(s); return s.length; })(x)` and a helper that sorts a `slice()` copy are clean. `SCXML075` reports what the analysis cannot decide: a call of a function it has no source for (an undeclared name, a data value, a method it does not know, a function the document reassigns), `eval` or `Function`, a `with` scope, a `<script src>` (which may define or replace anything), a document getter, setter, conversion method (`valueOf`, `toString`, `toJSON`), or generator that has an effect when the expression reads values, a function the expression itself creates whose body has an effect (a function or arrow literal, such as a `sort` comparator, or an object literal's method, getter, setter, or conversion method, which a built-in, a property read, or a conversion may call; reported at the function, with the effect as its cause, whenever nothing else in the expression is), `RegExp` `exec`/`test` (and `String` methods given a non-literal pattern) on a value that may be a global or sticky expression, a write through a reference it cannot trace, and an expression over its step or call-depth bounds. A document that writes a built-in or a prototype makes every built-in call undecidable. Known limits, stated rather than guessed: block-scoped declarations are treated as local to their whole function, and local variables have one flow-insensitive origin per function. An answer outside the checker's contract (a span outside its fragment, a finding for an unchecked fragment other than `SCXML077`'s, an over-long string, more than 4 findings per fragment) is declined as one `SCXML075`. The official W3C IRP corpora have no `SCXML074` or `SCXML075`, and one `SCXML076`: test 444.
- `SCXML077` (ECMAScript, warning) reports an assignment, compound assignment, `++`/`--`, or `delete` of a system variable itself (`_event`, `_sessionid`, `_name`, `_ioprocessors`, or `_x`, the root 5.10 reserves for platform-specific ones) in any expression or static `<script>` of the machine, and the static forms that write one through the global object (`globalThis._event = 1`, `this._name++` outside any function, `delete globalThis['_event']`, `Reflect.deleteProperty`/`Reflect.defineProperty`/`Reflect.set`/`Object.defineProperty` with `globalThis` or top-level `this` and a string literal), outside a scope that declares the name as its own (a function's variables and parameters, a block's or `switch`'s `let`/`const`/`class`, a `for` head's `let`/`const`, a `catch` parameter). SCXML 1.0 §5.10: "The Processor MUST cause any attempt to change the value of a system variable to fail and MUST place the error 'error.execution' on the internal event queue when such an attempt is made." Because the specification defines the run-time outcome, a document that attempts it is conformant and the finding is a warning, not an error. The data model fails every such assignment at run time with `error.execution`; a non-strict or reflective deletion returns `false` without one, a known limitation pending an engine patch ([spec-errata.md](spec-errata.md), "Known limitations (pending)"). `<assign location="_event">` is not reported: it fails at run time with `error.execution`, which official IRP tests 322, 324, 326, 329, and 346 require. The official corpora have no `SCXML077`.
- `SCXML062` (warning) reports an explicit Unicode bidirectional embedding, override, or isolate control (U+202A-U+202E, U+2066-U+2069, the set rustc's `text_direction_codepoint_in_literal` lint rejects) anywhere in the source, one finding per line with every control labeled by its Unicode name: it reorders how the text around it is displayed, so a review can see a condition, ID, or script other than the one that runs (Trojan Source, CVE-2021-42574). The fix writes each control as a character reference (`&#x202E;`), which XML decodes to the same character in an attribute value or text, so it is machine-applicable there; a control in a comment or CDATA section, which references cannot express, is removed and the fix is maybe-incorrect. Independently, the rustc reporter escapes these characters (and U+061C, U+200E, U+200F) as `\u{XXXX}` in every message, label, and source line it prints, as it escapes C0 and C1 controls.
- `SCXML063` (ECMAScript, warning) reports a condition that is itself an arrow function (`cond="count => 3"`, a typo for `>=`): a function is truthy, so the condition always holds. The `=>` must be at the top level, outside literals and every bracket, so a call that takes an arrow function (`list.some(x => x > 1)`) is not reported; when the parameter is a single name the finding offers a maybe-incorrect `>=` fix, verified to parse.
- `SCXML070` reports a `delay` or `delayexpr` on a `<send>` whose literal `target` is `#_internal`: SCXML 1.0 §6.2 says both "Must not occur ... when the attribute 'target' has the value "_internal"" (the special target §C.1 names `#_internal`). The schema allows it, so only this rule catches it; the delay is primary, the target secondary, and a maybe-incorrect fix removes the delay. A computed `targetexpr` is not judged; at run time such a send raises `error.execution` (§6.2.4).
- `SCXML073` reports a `<data id>` that begins with `_` under every data model other than null (`<data>` is `SCXML037` there): SCXML 1.0 §5.10 "Variable names beginning with '_' are reserved for system use. A conformant SCXML document MUST NOT contain ids beginning with '_' in the <data> element." A maybe-incorrect fix renames the declaration without the leading `_` (references are not rewritten).
- `SCXML072` (warning) reports a state listed twice in one target list (a transition `target`, an `initial` attribute, or an `<initial>`/history default transition): SCXML 1.0 §3.11 defines "a legal state specification to be a set of states such that 1) no state is an ancestor of any other state on the list, and 2) a full legal state configuration results when all ancestors and default initial descendants have been added", so the repeat names the same member of the set, the document is conforming, and the compiler lists the state once in Machine IR. The machine-applicable fix removes the repeat.
- `SCXML026` and `SCXML036` are warnings: SCXML 1.0 Appendix D, "Termination", says "A microstep always terminates. A macrostep may not. A macrostep that does not terminate may be said to consist of an infinitely long sequence of microsteps. This is currently allowed." A document whose eventless transitions loop is therefore conforming and compiles; at run time the host's step and fuel bounds end such a macrostep.
- `SCXML071` reports an `<invoke>` that has both a `namelist` and `<param>` children: SCXML 1.0 §6.4.1 says `namelist` "Must not occur with the <param> element." The schema allows it, so only this rule catches it; the namelist is primary, the first `<param>` secondary, and a maybe-incorrect fix passes each namelist location as `<param name="L" location="L"/>` (§6.4.1: "The name of the location is the attribute and the value stored at the location is the value") and removes the `namelist`.
- `SCXML055` reports a `<raise>` or `<send>` anywhere inside a `<finalize>` (including `<if>`/`<foreach>` bodies): SCXML 1.0 §6.5 says `<finalize>` MUST NOT raise events or invoke external actions and names both elements. The schema allows the content, so only this rule catches it; it labels the `<finalize>` and offers a maybe-incorrect removal.
- `SCXML058` (warning) reports an `In(...)` call in a `cond` whose single literal argument names no `<state>`, `<parallel>`, or `<final>` of the machine (always false; bounded did-you-mean fix), names a `<history>` (never active; the help names its parent state), or, under ECMAScript and XPath, passes a state ID unquoted when no `<data>` variable has that name (a quoting fix). Under the null data model an unquoted `In(id)` is the `SCXML079` warning instead, because the null data-model component accepts only a quoted ID and raises `error.execution` for anything else. Calls with computed arguments are not judged.
- `SCXML019`/`SCXML020` check history transition cardinality, target (an empty or blank one counts as missing), and unconditionality. A `type` is accepted: SCXML 3.10.2 forbids only "'cond' or 'event' attributes" on this transition, and the default history transition is never selected, so its type has no effect.
- `SCXML006` also reports an `initial` attribute that names no state at all (`initial=""` or only white space) on `<scxml>` or a compound `<state>`, with a maybe-incorrect fix naming the first child state (the one entered by document order); the compiler rejects an empty initial set.
- `SCXML023` warns when compound state entry depends on implicit document order, labels the first child state (the one entered today), and, when it has an `id`, offers a machine-applicable `initial="…"` insertion that makes the current behavior explicit.
- `SCXML024` warns when a non-final atomic state can never be left: neither it nor any enclosing `<state>` or `<parallel>` has a transition or `<invoke>`, and no transition inside an enclosing `<parallel>` can leave that `<parallel>` (one whose targets leave its own region, or an external transition from a region itself, exits every sibling region too).
- Cascade suppression: one mistake yields one finding. `SCXML029` is not reported for a machine with an unresolved `target` or `initial` token (the typo is the root cause) or with a misspelled SCXML element; `SCXML024`/`SCXML025` are not reported inside a subtree `SCXML029` already reports as unreachable; a duplicate ID's later declaration is not also reported as unreachable or as a state that can never exit (`SCXML024`/`SCXML025`), because every reference resolves to the first declaration; a transition `SCXML034` reports as shadowed is not an edge of an `SCXML026` cycle (only the first eventless, conditionless transition of a state can be selected) and is not followed by `SCXML029` reachability, so a state only it targets is reported unreachable up front and removing it adds no finding; an unknown element's attributes are not expression sites; an `<invoke>` with `src`, `srcexpr`, and `<content>` together is one `SCXML016`, not three pairwise errors; all null-data-model expression sites of one element are one `SCXML037`/`SCXML043` (the others are secondary labels); an `<initial>`/`<history>` with several transitions is `SCXML009`/`SCXML019` only (the XSD content-model finding is superseded); `<state type="final">` is one XSD finding with a rename-to-`<final>` fix and no `SCXML024`; and a near-miss SCXML namespace on the root is one `SCXML001`, not also `XSD001`.

### Lowering rules: `validate` covers every `compile` rejection

`validate` is the gate `compile` relies on, so every structural rejection in
`scxml-compiler-core` lowering and Machine IR validation has a validator
diagnostic at the responsible bytes. Most restate a schema or semantic rule
above; the bundled W3C schema allows the constructs below, so these rules
report them. `crates/scxml-compiler-core/tests/validate_covers_lowering.rs`
enumerates every rejection site in lowering and Machine IR validation (a
source scan fails when a site is added without a case) and proves that each
reachable one is a validator error with the expected code, or states why it
cannot be reached from a parsed document (an invariant lowering constructs,
a bound implied by an earlier one, or a check of decoded IR only). The one
remaining `COMP001` after a clean validation is the 16 MiB bound on encoded
Machine IR, which exists only after lowering; `compile` then reports it at
the root element with a note, and every other `COMP001` names the element
lowering rejected. Machine IR bounds apply to wasm builds only (the shipped
compiler and machine components): a native `scx compile` lowers a
document of any size, and the same test proves that natively lowering
accepts every bound fixture, `validate` reports no `SCXML069`, and `compile`
reports no `COMP001`.

| Code | Severity | What it proves |
| --- | --- | --- |
| `SCXML064` | error | A `<send>` or `<invoke>` has more than one `<content>`, an `<invoke>` more than one `<finalize>`, or a `<state>` more than one `<initial>` (SCXML 1.0 §6.2, §6.4, §3.3). The second child is primary, the first secondary, and a maybe-incorrect fix removes every extra one. |
| `SCXML065` | error | An `<assign>` has no value (neither `expr` nor child content, and not `type="delete"`; has-placeholders `expr="…"` insertion), uses `attr` without `type="addattribute"` (maybe-incorrect removal), or `type="addattribute"` without `attr` (has-placeholders insertion) (SCXML 1.0 §5.4, SCXML CR 2014 §C.3.6). `expr` together with child content is `SCXML053`. A `type="delete"` with a value is conformant (CR §C.3.6: "('expr' is ignored.)"; §5.4.2 asks for "either "expr" or children"), so it is not reported, and the compiler does not lower the value, so it is never evaluated. |
| `SCXML066` | warning | A required expression attribute is empty: `<if>`/`<elseif>` `cond`, `<foreach>` `array`/`item`, `<assign>` `location`. An empty value is an ill-formed expression, whose run-time outcome SCXML defines (§5.9.4: the processor "MAY wait and place 'error.execution' in the internal event queue at runtime when the expressions are evaluated"), so the document compiles and the construct raises `error.execution` each time it runs (a blank one, such as `cond=" "`, is the `SCXML042` warning instead). A has-placeholders fix fills the value. |
| `SCXML067` | error | An `<scxml>` has no `<state>`, `<parallel>`, or `<final>` (SCXML 1.0 §3.2.2: "A conformant SCXML document MUST have at least one <state>, <parallel> or <final> child"). |
| `SCXML078` | warning | A `<parallel>` has no child state. SCXML 1.0 §3.4.2 allows it ("Occurs 0 or more times"), so it compiles and runs, but it is never atomic (§3.3), so no transition is enabled through it (§3.13) and it raises no `done.state`; a maybe-incorrect fix renames it to `<state>`. |
| `SCXML080` | warning | A multi-target set names a `<history>` and a state below that history's parent (`target="h b1"` with `h` a history of `p` and `b1` in a region of `p`). SCXML 1.0 §3.10.2 enters the history's recorded or default configuration, which covers the parent's subtree, so the set yields a legal configuration only when that configuration contains the other target; otherwise the document is not conformant (§3.11: "A conformant SCXML document MUST place the state machine only in legal state configurations") and the processor's behavior is undefined (§A.2). The recorded configuration is known only at run time, so this is a warning. |
| `SCXML081` | error | A `<state>`, `<parallel>`, `<final>`, or `<history>` ID is longer than 1,024 bytes of UTF-8, the bound of every name the processor reports. Every build names the active states by their IDs (the configuration each call result reports, snapshots, `In()`), so the state could never be reported once entered. Also, with its own message, the ID of a state that can complete (one with a `<final>` child, or a `<parallel>` with a `<state>` region that has a `<final>` child and whose every region can be in a final state at once; a `<parallel>` whose regions are all `<parallel>` never raises `done.state`, SCXML 1.0 Appendix D) is longer than 1,013 bytes, so the `done.state.<ID>` event it raises (SCXML 1.0 §3.7: the processor "MUST generate the event done.state.id") is longer than the 1,024-byte bound of an event name. Every length is of the ID's schema value (white space collapsed), as lowering measures it. SCXML 1.0 §3.14 sets no length; this is the implementation's interface bound on every build ([spec errata L.6](spec-errata.md)). |
| `SCXML082` | error | A literal `<raise event>`, `<send event>`, `<send id>`, or `<invoke id>` is longer than 1,024 bytes of UTF-8, the bound every build sets on an event name, a send ID, and an invoke ID, so the element could never run as written; or, with its own message, an `<invoke id>` is longer than 1,012 bytes, so the `done.invoke.<ID>` event its invocation returns (SCXML 1.0 §6.4: the service "MUST return a special event 'done.invoke.id'") exceeds the event-name bound. Every length is of the attribute's schema value (white space collapsed: `event="  e…  "` with 1,024 bytes between the spaces is within the bound). A name computed at run time (`eventexpr`, a generated invoke ID) over the bound raises `error.execution` instead (SCXML 1.0 §3.12.2). SCXML 1.0 sets no length; this is the implementation's interface bound on every build ([spec errata L.6](spec-errata.md)). |
| `SCXML083` | error | An `env` attribute (`https://github.com/stateforward/scxml.rs/namespaces/env`) on the `<invoke>` of an invoked service names a slot its service package does not declare (`env-slots`; judged for a literal `type` a given service package binds, with a "did you mean" fix for a near miss), binds one slot twice (`env:<slot>` and `env:<slot>expr`), names no valid slot (`[a-z][a-z0-9-]*`), or gives `env:<slot>` a value that is not a variable name (`[A-Za-z_][A-Za-z0-9_]*`, at most 256 bytes). The host binds each slot to an opaque reference to the named variable's value for the whole invocation. |
| `SCXML084` | error | An element in the `env` namespace (an attribute vocabulary only), or an `env` attribute on an element in no namespace, on an executable-content extension element (executable content does no I/O, so it has no slots), or on the `<invoke>` of an SCXML session (an absent `type` or an SCXML invoke type): only an invoked service's `<invoke>` has slots. An `env` attribute on any other SCXML element is `SCXML068`; on an `<invoke typeexpr>` the host judges it when the type is known. |
| `SCXML085` | error | An element in the `event` namespace (`https://github.com/stateforward/scxml.rs/namespaces/event`, an attribute vocabulary only), an `event` attribute on any element other than `<transition>`, `<send>`, `<raise>`, or `<donedata>` (maybe-incorrect removal), or an `event` attribute other than `schema` and `schemaexpr` (with a "did you mean" fix for a near miss). The W3C schema admits attributes in other namespaces laxly, so XSD cannot catch these; [namespaces/event](../namespaces/event/README.md) documents the vocabulary. |
| `SCXML086` | error | One element has both `event:schema` and `event:schemaexpr` (the second is primary, with a maybe-incorrect removal of `event:schemaexpr`). |
| `SCXML087` | error | An `event:schema` is not UTF-8 JSON: an inline schema (first non-white-space character `{`, or exactly `true` or `false`) at the exact byte the JSON parser stopped at, or a referenced file at the attribute value with the file's line and column in the message. |
| `SCXML088` | error | A referenced `event:schema` file is not available: the reference is not a relative path at or below the document's directory (an absolute URI, an absolute path, a repository-namespace resource, a query, a backslash, or an empty, `.`, or `..` segment), or no bytes were supplied for it (the CLI reads each referenced file relative to the document before validating). |
| `SCXML089` | error | The fragment of an `event:schema` reference is not a JSON Pointer (RFC 6901; a plain-name fragment, a bad `~` escape, or bad percent-encoding) or names no value; the primary label is the fragment, and the message names the longest prefix that names no value. |
| `SCXML090` | error | An `event:schema` violates its dialect's metaschema (the message names the location in the schema), or does not compile (for example a `pattern` that is not a regular expression). |
| `SCXML091` | error | An `event:schema` declares a `$schema` other than JSON Schema 2020-12 (the default), 2019-09, or draft-07. |
| `SCXML092` | error | An `event:schema` has a `$ref`, `$dynamicRef`, or `$recursiveRef` that names another resource (nothing is fetched), a fragment that names no value, or, when a pointer selects part of a file, a part that relies on `$id`, `$anchor`, `$dynamicAnchor`, `$recursiveAnchor`, plain-name fragments, or dynamic references and so cannot be bundled, or a bundle over its bounds: more than 256 referenced values (wasm builds only), more than 8 times its file plus 4096 bytes, or nested deeper than the 128 levels the Machine IR decoder reads back. A schema over the 1 MiB Machine IR string bound (wasm builds only) is `SCXML069`. |
| `SCXML093` | warning | A literal `<invoke type>` is a platform-specific value, not a URI: neither an SCXML invoke type (absent, `scxml`, or `http://www.w3.org/TR/scxml/`) nor an absolute service type URI of at most 4096 bytes. SCXML 1.0 §6.4.1 allows platform-specific types, but a service package binds an invoked service by an exact absolute URI, so on a host built from this repository the `<invoke>` raises `error.execution` when it runs; the document still compiles and runs. |
| `SCXML094` | warning | A literal `<invoke type>` names an invoked service no given service package binds (`--service-package`). Invocation is a runtime action (SCXML 6.4): the document compiles and runs, and without a bound package the `<invoke>` raises `error.execution` when it runs. |
| `SCXML102` | error | In an ECMAScript document (`datamodel="ecmascript"`, or none), the literal JSON `<content>` of a `<send>` or `<donedata>` (no `expr`, text only, parsed after the data model's white-space normalization) does not satisfy the element's own literal `event:schema` (inline, or a file whose bytes were supplied). The primary label is the `<content>`, the secondary the schema. `event:schemaexpr` and schemas that do not resolve are skipped silently. |
| `SCXML103` | warning | In an ECMAScript document, a `<send event="x">` delivered to its own session (no `target`, or `#_internal`, through the SCXML Event I/O Processor) or the `<donedata>` of a final child of a compound state (its `done.state.<id>`) creates data a transition of the same document handling that event (SCXML 3.12.1 descriptor matching, nested `<content>` documents excluded) does not accept: its literal JSON `<content>`, or a witness of its own literal schema, fails the transition's literal schema. Equal schemas are compatible; otherwise only a witness is reported, an instance the producer's schema admits and the transition's rejects, drawn from the producer's schema (`const`, `enum`, `examples`, `default`, then a minimal instance of each `type`, its required properties filled in), so the check never reports an incompatibility it cannot exhibit. `event:schemaexpr` is skipped silently. The schemas are metadata, so the transition is still taken; this is a warning. |
| `SCXML068` | error | An attribute in another namespace on an SCXML element that lowering visits (`xml:lang`, `xml:space`, `xml:base`, `xsi:schemaLocation`, or a custom attribute, but not an `event` attribute, which `SCXML085` owns; one finding per element, machine-applicable removal when every such attribute is `xml:*` or `xsi:*`, which carry no SCXML behavior), or an element in another namespace anywhere but executable content (where it is a custom action), including inside `<log>`, `<cancel>`, `<param>`, and `<script>`, which the schema's `##other` wildcards admit but lowering does not. A content model without a wildcard (`<donedata>`, `<raise>`, `<elseif>`) is left to the schema's `XSD-cvc-complex-type.2.4`, attributed to the W3C SCXML schema. The compiler lowers neither rather than drop it silently. This is a limit of this toolchain, not an SCXML rule: SCXML 1.0 and the bundled W3C schema allow both (`##other` wildcards), so the finding has category `internal` (like `COMP001`), cites no SCXML section, and says the construct cannot be lowered by this compiler. The `xsi` namespace, like `xml`, needs no registered schema (no `XSD001`). |
| `SCXML069` | error | The document exceeds a Machine IR bound: 65 nested states (a child of `<scxml>` and 64 levels below it), 64 levels of `<if>`/`<foreach>` nesting, 32 levels of inline XML in `<data>`, `<content>`, or `<assign>` (31 below a custom action, fewer inside deep executable content), 10,000 child nodes of an element, attributes of an inline element, tokens in a `target`, `initial`, `event`, or `namelist`, actions in an `<if>` branch, `<elseif>` branches, or `<data>` per scope, 100,000 targets across a machine's multi-target sets, 10,000 extension namespaces, or 1 MiB per attribute value, text run, script body, name, or namespace URI. At most 16 findings per machine; target-list findings are left to `SCXML028` when the machine is over the graph's target-reference bound. |

Lowering also generates an ID for a `<state>` or `<parallel>` without one,
as SCXML 1.0 §3.3 and §3.4 require (it did so only for `<final>`), so an
anonymous state compiles.

### Draft profile

A profile restricts what one validation admits on top of SCXML 1.0. The
schema policy (`--policy strict|lax|off`) governs only namespaces without a
schema, and the validator had no rule-set concept, so the profile is a
separate selection: `Validator::with_profile(Profile::Draft)` in
`scxml-validator-core`, and `<scx:validate profile="draft">` in the
[`scx` namespace](../namespaces/scx/README.md). The draft profile checks a
document a machine writes and then runs as its own invoked child (a builder
bot's draft): the document must load nothing, start nothing, and talk only
through the SCXML Event I/O Processor. Every check reads the parsed tree
(expanded names, never prefixes); `SCXML101` reads the prolog before the
root element, skipping comments and processing instructions. Without the
profile none of these codes is reported. `crates/scxml-validator-core/src/draft_profile.rs`
holds the rules and their tests, and `namespaces/scx/src/tests.rs` drives
them through the element.

| Code | Severity | What it proves |
| --- | --- | --- |
| `SCXML097` | error | A namespace other than the SCXML namespace, the `event` attribute vocabulary, and the XML namespace is declared (at the declaration's URI) or used by an element or attribute whose namespace is declared on another element. An extension namespace or the host's `env` vocabulary would reach packages and variables the writing machine binds. The namespace is not also reported as a missing schema (`XSD001`). |
| `SCXML098` | error | An `<invoke>` (SCXML 6.4) or `<script>` (5.8) element: a draft starts no session or service and runs no script. |
| `SCXML099` | error | A `src` or `srcexpr` attribute on an SCXML element (`<data>`, `<script>`, `<invoke>`): a draft loads nothing. |
| `SCXML100` | error | A `typeexpr` attribute, or a `<send type>` other than the SCXML Event I/O Processor's `http://www.w3.org/TR/scxml/#SCXMLEventProcessor` (SCXML 6.2, C.1). `<transition type>` and `<history type>` are not judged. |
| `SCXML101` | error | A document type declaration (XML 1.0 §2.8): a draft declares no entity or attribute default. |

### Package validators

An extension or service package may ship a package validator component
(`stateforward:scxml-validator/package-validator`, named by the manifest's
`validator-artifact` and `validator-sha256`, resolved, pinned, and
digest-verified like the package's own component) that checks what its XSD
cannot. After the XSD and built-in checks, the validator builds the
document's read-only context once (`document.context`: every element in
document order with its attributes, spans, children, and parent, the
`<data>` and state ids, every `<transition>` with its source state, `event`,
and resolved literal `event:schema`, and the root's `datamodel`) and calls
each package's validator once with its targets: the elements in its
namespace (an extension package), or the `<invoke>` elements whose literal
`type` is its service URI (a service package). The world imports nothing:
no WASI, no I/O; each call runs on a fresh fuel-bounded store, so its answer
depends only on its input. Its diagnostics are reported with the code
`<prefix>::<code>` and category `package`: the prefix is the document's
prefix of the namespace (an extension package; a namespace declared as the
default namespace uses its last path segment), or the service URI's last
path segment, which is a repository service's `<name>` (a service package:
`https://github.com/stateforward/floppy/services/generate` reports
`generate::schema_mismatch`). Labels keep their exact spans.

| Code | Severity | What it proves |
| --- | --- | --- |
| `PKG002` | error | A package validator failed closed: it trapped (fuel exhaustion included), returned an error, returned more than 256 diagnostics, a code that is not `[a-z][a-z0-9_]*` of at most 64 bytes, a span outside the source or off a character boundary, overlapping edits within one fix, or an edit outside every element of the package. All of that package's diagnostics are discarded for this one error, which names the package and labels its first element. |

`scx validate`, `compile`, `snapshot run`, and `run` link the package
validators of the packages they bind through the Wasmtime host
(`PackageValidators`); the validator component reaches them through its
`package-validator-api` import, which the Wasmtime host's
`ComponentValidator::with_package_validators` and the browser host's
`createValidator({packageValidators})` answer with the same routing (by the
first target's exact namespace URI, or the literal `type` of an `<invoke>`),
so the reports agree. `crates/scxml-validator-core/src/package_validator/tests.rs`
covers the rendering and every fail-closed case;
`hosts/scxml-host-wasmtime/tests/package_validators.rs`,
`crates/scxml-cli/tests/extension_packages.rs`, and the browser host's
`test/validator.test.mjs` run the example package validator
(`components/scxml-package-validator-example`).

### Semantic machine graph rules

Each `<scxml>` root (including inline `<invoke><content>` machines) is lowered
into a bounded semantic graph: states, `<parallel>` regions, `<final>`,
`<history>` and `<initial>` pseudo-states, default entry (`initial` attribute,
`<initial>` transition, or first child), and transitions with resolved targets,
event descriptors, and `cond` presence. All analyses of one machine share a
work budget of 8,000,000 units, or 1,024 units per element and `target` or
`initial` reference when that is more, on every build: each linear analysis
spends a few units per element and nesting level, so only work that grows
faster than the machine (the pairwise checks of thousands of transitions of
one state or of one target set) spends it. On wasm builds (the validator
component) one machine graph also holds at most 65,536 nodes, 65,536
transitions, and 131,072 target references, and the budget is at most
8,000,000 units. Exceeding any bound yields a single fail-closed `SCXML028`
error and no partial graph findings; the native `scx` analyzes a machine
of any size whose analysis is not super-linear.

| Code | Severity | What it proves |
| --- | --- | --- |
| `SCXML028` | error | The machine exceeds a graph bound; reachability, target legality, and liveness were not checked. |
| `SCXML029` | warning (lint) | A `<state>`, `<parallel>`, or `<final>` can never become active. Reachability over-approximates: every event can arrive, every `cond` can be true (a transition `SCXML034` proves can never be selected is not followed), `<parallel>` entry enters every region, a shallow history may restore any reachable child with its default entry, and a malformed default entry enters every child. Only the topmost unreachable state is reported; a transition targeting it from an unreachable state is labeled. |
| `SCXML030` | error | Two targets of one transition, `<initial>`/history default transition, or `initial` attribute lie in different children of a compound state, so they cannot form a legal configuration. Exactly as the compiler's Machine IR check decides it, every pair of distinct targets must lie in different regions of a common `<parallel>`: a target and its own ancestor or descendant (`target="p x1"`) is reported as nested; a repeated target (`target="a a"`) is legal (SCXML 1.0 §3.11 defines a legal state specification as "a set of states"), so it is only the `SCXML072` warning, with a machine-applicable removal of the repeat, and the compiler enters the state once; a `<history>` target counts as itself, a child of its parent. Offers a maybe-incorrect removal of either conflicting token only when the removal strands no state (re-running reachability without that token); a removal that would leave a state unreachable is not offered, and a note names the state it would strand, so no suggested fix introduces an `SCXML029`. |
| `SCXML031` | error | A `<final>` contains an SCXML element other than `<onentry>`, `<onexit>`, or `<donedata>`, or declares `initial`. Offers maybe-incorrect `<final>`→`<state>` rename and removal fixes; the `initial` removal is machine-applicable. |
| `SCXML032` | error | `<donedata>` appears outside `<final>`, or a `<final>` has more than one. |
| `SCXML033` | warning (lint) | A `<parallel>` has exactly one region. Offers a maybe-incorrect rename to `<state>` and notes the completion-semantics difference. |
| `SCXML034` | warning (lint) | A transition can never be selected because an earlier transition in the same state has no `cond` and matches every event it matches (prefix descriptor matching, `*` wildcard), or both are eventless. Offers a maybe-incorrect removal fix, and a maybe-incorrect reorder fix only when moving the transition up resolves the finding: when both transitions match the same events (a duplicate `event`, two eventless transitions), the moved transition would shadow the other one, so no reorder is offered. |
| `SCXML036` | warning | A cycle of eventless transitions that are each the deterministic selection for a non-parallel atomic state, closed through default entry (`initial` attributes/children) or inherited ancestor transitions. Direct state-to-state cycles stay with `SCXML026`. |

### Data-model expression rules

`datamodel="null"` checks and the `datamodel` declaration checks are
data-model-neutral and always run. Other data models are syntax-checked only
when the host passes an explicit, optional, parse-only `ExpressionSyntaxChecker`
(`Validator::with_expression_checker`) whose data model equals the machine's
data model exactly: its `datamodel` attribute, or `ecmascript` when it has none
(SCXML 1.0 §3.2.1 gives `datamodel` the default "platform-specific", and
§3.2.2 says "Platforms SHOULD document their default data model"; the compiler records the
default in Machine IR, so every host links the ECMAScript data model for such a
document, and the validator checks it exactly as `datamodel="ecmascript"`).
Expressions are never skipped silently: an unrecognized `datamodel` is the
`SCXML044` error (with a typo fix such as `ecmascipt` to `ecmascript`), and a
machine whose recognized data model has no linked checker gets an `SCXML045`
note that its expressions were not checked. The component contract is `stateforward:scxml/data-model-api`
`check-syntax(kind, source) -> result<list<syntax-issue>, component-error>`,
implemented by the ECMAScript provider with Boa's parser and by the XPath
provider with the bundled XPath 1.0 parser (both parse only, never evaluated),
and explicitly declined (`unavailable`) by the null provider. The native
`scx` links both checkers. ECMAScript conditions and values are parsed
exactly as the ECMAScript data model evaluates them (parenthesized, one
trailing `;` tolerated); XPath conditions, values, and locations are parsed as
XPath 1.0 expressions with the SCXML `In()` extension, a location must be an
expression that can select nodes, and a non-empty `<script>` is rejected
because the XPath data model defines no script language (SCXML CR 2014 §C.3.9,
§C.3.11). Issue offsets map
through XML character/entity references and CDATA to exact source bytes. At
most 16,384 fragments and 4 MiB of expression text are checked per document
(`SCXML041`; wasm builds only, the native `scx` checks every fragment) and
at most 4 issues are reported per fragment.

| Code | Severity | What it proves |
| --- | --- | --- |
| `SCXML037` | error | Under `datamodel="null"`, an element SCXML 1.0 §B.1.7 does not support ("The <foreach> element and the elements defined in 5 Data Model and Data Manipulation are not supported in the Null Data Model": `<datamodel>`, `<data>`, `<assign>`, `<donedata>`, `<content>`, `<param>`, `<script>`, `<foreach>`) is one finding at its start tag (what it contains is not reported again), with maybe-incorrect removal (not for `<content>`, which may be an `<invoke>`'s only source) and switch-to-`ecmascript` fixes. Every expression under the null data model (a value expression, a location, a `namelist`, a `<foreach item/index>`, or a `cond` other than `In('stateID')`, including an unquoted `In(s)`) is the `SCXML079` warning instead, because §5.9.4 defines its run-time outcome (`error.execution`). No run-time behavior is defined for the unsupported elements, and §A.1 requires a document to "conform to any additional syntactic constraints that are defined for the data model that it has chosen", so they stay an error. |
| `SCXML079` | warning | Under `datamodel="null"`, an expression: a value expression (`expr`, `*expr`, `array`; §B.1.4 "There is no value expression language."), a location, `namelist`, or `<foreach item/index>` (§B.1.3), or a `cond` other than `In('stateID')` (§B.1.2), with the ID single- or double-quoted, non-empty, and free of XML white space (#x20, #x9, #xD, #xA) and backslashes, and optional XML white space around `In`, `(`, and the call, exactly as the null data-model component evaluates it. An unquoted `In(s)` gets its own finding at the argument with a quoting fix (machine-applicable when `s` names a state, maybe-incorrect to the nearest state ID otherwise). One finding per element (other expressions on it are secondary labels), with maybe-incorrect removal and switch-to-`ecmascript` fixes. It is a warning because §5.9.4 defines the run-time outcome of an ill-formed expression (`error.execution`; a condition is then false), so the document compiles. |
| `SCXML038` | warning | With a checker (a warning because SCXML 1.0 §5.9.4 lets the processor "wait and place 'error.execution' in the internal event queue at runtime when the expressions are evaluated", which this toolchain does, so the document compiles): a `cond`, value expression, or `<script>` does not parse. The primary span is the exact failing token. Operator habits from another language are all reported, each at its own token, when repairing every one of them makes the fragment parse: XPath `==`/`===` (to `=`), `!==` (to `!=`), `&&` (to `and`), `||` (to `or`), `%` (to `mod`), and uppercase `AND`/`OR`/`DIV`/`MOD` get machine-applicable fixes, and `!x` and `not x` a maybe-incorrect `not(x)`; ECMAScript `and`/`or` get machine-applicable `&amp;&amp;`/`||`, and `not x` a maybe-incorrect `!x` (`!` binds tighter than a comparison). The lints of a fragment that parses (`SCXML056`, `SCXML057`, `SCXML059`, `SCXML060`) and the side-effect analysis (`SCXML074`-`SCXML077`) run on the repaired fragment too, so applying every repair reveals no unreported finding. When the repairs together do not make the fragment parse, the checker's first issue is worded instead. ECMAScript: unquoted prose in a value (including `:`, `…`, emoji, or `@`), an apostrophe that ends a string early, a stray `=` after a comparison (`x === = 3`), an assignment to a non-assignable left side (`typeof x = 'y'`), `x.0`/`x.1` and `x.0.name` (in a value or a location; the message echoes the source `.0`), a `#` Markdown heading or shell comment (quoted as text in a value, `//` in a `<script>`), a private name such as `x.#y`, and a `let`/`const`/`var` declaration get targeted help and maybe-incorrect fixes (quoting, `\'`, one comparison operator, `===`, `x[0]`, `//`, `x.y`); a comment-only value, a statement keyword (`if`, `for`, `return`) in a value or condition, an unterminated string or template literal (labeled at its opening quote), and a character the lexer rejects get targeted help. XPath: a space inside `>=`, a trailing argument comma, a missing `,` between function arguments (`concat('a' $b)`), an apostrophe that ends a string early, a `fn:` prefix on a core function, and unknown functions (`length()` to `string-length()` or `count()`, typos by bounded nearest match) get maybe-incorrect fixes; unclosed `(`/`[` label their opener, arity errors state the accepted count, and a `<script>` is reported as unsupported with a removal fix. |
| `SCXML039` | warning | With a checker (a warning because SCXML 1.0 defines the run-time outcome: §5.9.2 "If a location expression cannot be evaluated to yield a valid location, the SCXML processor MUST place the error 'error.execution' in the internal event queue", and §4.6 for a `<foreach item>` that "does not specify a legal variable name"): a location is not assignable (ECMAScript: not a variable name or property access, and an ECMAScript reserved word such as `class` is named as one; XPath: not an expression that can select nodes, with `$var`/`$var/child` advice). A reserved word as a `<foreach item/index>` gets a maybe-incorrect rename (`classItem`), and a comma-separated `namelist` token a machine-applicable comma-to-space fix. A `<foreach item/index>` that starts with a digit gets a maybe-incorrect prefix fix: `_1x` under XPath (an NCName; the help never suggests `$`), `item1x` under ECMAScript. Any ECMAScript identifier is a legal loop variable (SCXML 1.0 §B.2.11: "The legal values for the 'item' attribute are legal ECMAScript variable names"), including one that starts with `_` or contains non-ASCII letters. A location that names a `<data id>` no expression can name (`location="retry-count"`) names that declaration as the root cause (a secondary label and the `SCXML061` rename). Under XPath, a `<foreach item/index>` written as `$name` when nothing declares `name` (`item="$it"`, a location assigning to an undeclared variable instead of binding one) is one finding at the `$` with a machine-applicable fix that removes it; the uses `$it` in the loop are not also `SCXML056`. With `<data id="it"/>` declared, `item="$it"` is a legal location and is not reported. |
| `SCXML040` | error | With a checker: the checker declined a fragment over the data model's bound. On every build, the nesting bounds, which guard the native stack (ECMAScript: at most 128 structural nesting units, where each open bracket costs 8, so 16 brackets, 15 inside a value expression; XPath: 32 levels of parentheses, predicates, or function arguments, and a 128-deep expression tree); on wasm builds only, the size bounds (ECMAScript: 16 KiB expressions, 64 KiB scripts; XPath: 16 KiB expressions and 4,096 tree nodes), so the native `scx` reports `SCXML040` for nesting only. Both providers decline a bound instead of reporting a syntax error, so a bound is never `SCXML038`. The data model applies the same bound when it evaluates the fragment, so it would be rejected at run time and `compile` refuses the document. |
| `SCXML041` | error | With a checker: the document exceeds the expression-checking bound; checking stops fail-closed. |
| `SCXML042` | warning (lint) | With a checker: an expression attribute is present but empty. A blank `<log expr>` lowers as the absent attribute (log without a value), so its removal fix is machine-applicable; any other empty expression, including `<transition cond>`, is still evaluated as an invalid expression and raises `error.execution` (a condition is then false), so its removal is maybe-incorrect and offered only when the attribute is optional (never `<if>`/`<elseif> cond`, `<assign>`, `<foreach array/item>`, or the only `<param>` value source); the help is worded for the element. |
| `SCXML044` | error | The `datamodel` value (its schema value, white space collapsed: `datamodel=" xpath "` is `xpath`) is not one this project recognizes (`ecmascript`, `null`, `xpath`); no expression could be evaluated or checked. Offers a maybe-incorrect typo fix. |
| `SCXML045` | note | Note: the machine's data model (its `datamodel`, or the default `ecmascript`) is a recognized data model other than null, and the run links no syntax checker for it (for example the Wasm validator component composed with another data model's checker), so nothing is claimed about its expressions. |
| `SCXML056` | warning | With the XPath checker (a warning because evaluating an unbound variable raises `error.execution`, SCXML 1.0 §5.9.3 "If a value expression does not return a legal data value, the SCXML Processor MUST place the error 'error.execution' in the internal event queue"; official IRP cases 156 and 245 rely on it): a fragment that parses references `$name`, but no `<data id>`, `<foreach item/index>` or `idlocation` name, or system variable (`_event`, `_sessionid`, `_name`, `_ioprocessors`) of the machine declares it, so evaluating it raises `error.execution`. `$user.name` is one variable named `user.name` (`.` is an XPath name character) and gets a `$user/name` fix when `user` is declared; a typo gets a bounded did-you-mean fix. `-` is a name character too, so `$a1-1` is one variable named `a1-1`: when a declared variable is the part before a `-`, the finding explains it and offers `$a1 - 1` instead of a nearest-name fix that would drop the subtraction. |
| `SCXML057` | warning (lint) | With the XPath checker: a top-level relative path is evaluated against `<datamodel>`, whose children are the `<data id="…">` elements and the system variables, so a bare declared variable name (`location="count"`, fix `$count`), `true`/`false` (fix `true()`), or a dotted member access (`_event.data.ok`, fix `$_event/data/ok`) selects nothing. Any other first step that is not such a child is reported too; a `<foreach>` item is not one, because the item variable denotes the collection's own node (SCXML 1.0 §4.6), reached as `$item`. Predicate contents and `In()` arguments are not judged. |
| `SCXML059` | warning (lint) | With the XPath checker: a numeric predicate whose number is not a positive integer (`$list/item[0]`, the ECMAScript habit of indexing from 0) selects no node, because XPath positions start at 1: a location that selects nothing raises `error.execution`, a condition is false, and a value is empty. `[0]` gets a maybe-incorrect `[1]` fix. |
| `SCXML060` | warning (lint) | With the XPath checker: `+` with a string literal that is not numeric (`'Hello, ' + $name`) is numeric addition in XPath and yields `NaN`. The additive chain gets a maybe-incorrect `concat(…)` fix of its operands; a chain that also subtracts gets none. |
| `SCXML061` | warning | Always on for `datamodel="ecmascript"` (no checker needed): a `<data id>` that is an XML NCName but not an ECMAScript identifier: an ASCII character other than a letter, digit, `_`, or `$` (`retry-count`, `max.tries`), a leading ASCII digit, or an ECMA-262 `ReservedWord` of a non-strict script (`class`; not `let`, `yield`, or `await`). SCXML 1.0 §B.2.1 ("the SCXML Processor MUST create an ECMAScript variable object whose name is the value of the 'id' attribute of <data>") makes such an id conformant, and the data model declares it as a property of the global object (`globalThis['retry-count']`), so this is a warning: no expression can name the variable, and a reference such as `retry-count < 3` silently subtracts. A non-ASCII letter (`价格`, `é`) is an identifier character and is not reported; a leading `_` is `SCXML073`; an ID that is not an NCName (`max tries`, `$price`) is `SCXML003`, whose fix is the same rename. Labels the ID, names its first reference, and offers one maybe-incorrect rename (`retryCount`, `maxTries`, `classValue`) of the declaration and every reference to the name in the machine's expressions (identifier boundaries, outside string literals, not after `.`; at most 256 edits, and no fix when the new name is taken, nothing ASCII remains to derive one from, or a reference cannot be mapped exactly onto the source; the help then suggests no name). A `<data id="…">`, the placeholder a `has-placeholders` fix inserts, is not an NCName, so it is `SCXML003`. |

Checker messages are rewritten into project-owned wording with concrete help
(for example `= =` gets a maybe-incorrect `==` fix, and a `<foreach item>` that
starts with a digit is reported as an invalid variable name with a prefix
fix) rather than exposing parser grammar names, and a message echoes the
source text rather than the parser's normalized token. No checker message is
shown verbatim: the ECMAScript lexer's messages (`Abrupt end: Expecting
private identifier`, `unexpected '@'`) and any message without a dedicated
rewording become project wording that backticks the source text at the
position (`crates/scxml-validator-core/tests/ecmascript_wording.rs` guards
this with the real checker). The synthetic `)` the
checker's parenthesized parse expects is never shown: a complete expression
followed by more text is reported as `unexpected `tok` after a complete
expression`. Every repair of an expression fragment is applied to the decoded
fragment and re-checked with the same data-model checker before it is
offered, so no suggested fix leaves an unreported syntax error behind: a
single repair must make the fragment parse, and the repairs of several
operator habits in one fragment must make it parse together (applying one
of them leaves only the other reported habits); XPath findings
cite SCXML CR 2014 §C.3 (the 2015 Recommendation dropped the XPath data
model, so the Candidate Recommendation is its last W3C definition).

Official IRP evidence: over all 206 prepared ECMAScript IRP documents (cases
and `.scxml` dependencies), the validator reports no error. The deliberate
illegal expressions the tests use to exercise `error.execution` are warnings,
pinned per document: `SCXML038` for `expr="return"`-style expressions in tests
156, 277, 309, 312–314, 344, 487, 488, 528; `SCXML039` for the illegal
`<foreach item>` in 152 and 457 and the illegal `namelist` in 553 and 554; and
two `SCXML079` in 436, whose document fixes `datamodel="null"` while the
template logs its outcomes with a value expression, which §B.1.4 rules out
(the case is declared not applicable). The other warnings are pinned
exactly: 16 `SCXML023` (compound entry by document order), 3 `SCXML025`
(conditional-only eventless exits), 20 `SCXML029` (states the tests never
enter), 2 `SCXML034` (duplicate `done.state.s0` transitions in 343 and 488), 2
`SCXML042` (empty `<log expr>` in 307), 3 `SCXML058` (test 580's
deliberate `In('sh1')` of a `<history>`), 1 `SCXML076` (test 444), and no
`SCXML024`.

Over all 206 prepared XPath IRP documents, with both checkers linked, the
validator reports no error either. The deliberate findings are warnings: 11
`SCXML038` (`expr="!1"`, `cond="==!*"`, and `<data expr="(">` in tests 152,
156, 277, 309, 312–314, 344, 487, 488, 528, plus 456, not applicable), 3
`SCXML039` (the `'continue'` item in 457 and the `"foo"` namelist in 553 and
554, which keep ECMAScript bodies), 5 `SCXML056` (156's "non-existent var"
`$Var5`; 245's child reads `$Var2`, which it deliberately does not declare;
302 and 304 read a variable only a script would declare, and the W3C marks
them not applicable to XPath), and 2 `SCXML079` (436's value expressions under
`datamodel="null"`; not applicable). The other warnings are the ECMAScript
ones plus 7 `SCXML057` (the deliberately illegal `location="foo.bar.baz "` in
286, 298, 311, 331, 343, 401, 402); no IRP document gets `SCXML059`,
`SCXML060`, or `SCXML061`. The regression test
`crates/scxml-validator-core/tests/irp_semantic_rules.rs` pins both corpora and
fails on any error or any added, lost, or changed finding.

## Exact remaining gaps

The validator does not claim facts the current parsed tree or data-model-neutral
boundary cannot establish:

- XSD remains responsible for element content models, attribute enumerations,
  required structural children, and complete datatype lexical spaces. The
  official W3C SCXML 1.0 schema set is bundled and applied by default (see
  [Structural validation](#structural-validation-with-the-official-w3c-schema));
  XSD ID/IDREF identity (`cvc-id.*`) inside SCXML elements is superseded by the
  per-root semantic identity rules, so extension-namespace `xs:ID` uniqueness is
  not enforced across SCXML identifiers.
- Expression syntax is checked for `datamodel="ecmascript"` and
  `datamodel="xpath"`, and only when that data model's checker is linked. The
  native `scx` links both data-model crates' parse-only `check_syntax`
  directly for both `validate` and `compile`. The `scxml-validator` Wasm
  component imports the `expression-syntax` interface of its world, which
  names one supported data model per request. The Wasmtime host's
  `ComponentValidator` is the adapter: it links that import to one
  authenticated data-model component's `data-model-api.check-syntax` (the
  ECMAScript or XPath provider), or answers `none` when no data model (or
  the null data model, which has no expression syntax) is linked, in which
  case expressions of a recognized data model get the `SCXML045` note instead
  of a check. `hosts/scxml-host-wasmtime/tests/validator_component.rs`
  validates every single-data-model `tests/ui` case through the component
  with the matching data-model component linked and requires its canonical
  report to equal the CLI's `.json` snapshot byte for byte. A document whose
  machines use both checked data models is checked for only the linked one
  (the other's expressions get the note); the CLI links both. A fragment the
  checker declines (size or nesting bounds, which the data model also
  enforces at evaluation) is the `SCXML040` error. The size bounds apply to
  wasm builds only, so the native CLI and the validator component differ for
  a fragment past one: `the_wasm_validator_keeps_the_bounds_the_native_cli_lifts`
  proves the component still reports it while the CLI's generated `ui` cases
  validate cleanly. The nesting bounds guard the parser's native stack and
  apply to every build.
  Types and runtime values are never checked. ECMAScript variables are not
  resolved (a `<script>` may declare them); XPath variables are resolved
  against the machine's declarations (`SCXML056`), but XPath paths below a
  variable are not checked against its content, and a `<foreach>` item's
  element name is unknown statically, so `SCXML057` then judges only declared
  names, `true`/`false`, and dotted member accesses. `<data>` inline content
  and `<content>` bodies are not parsed as expressions.
- XML well-formedness errors stop the parser, so only the first one is
  reported (`XML001`–`XML004`); fix it and rerun to see the next. The one
  exception is text outside the root element: every such run found (a code
  fence before and after the document, trailing prose) is one `XML001` whose
  single fix removes them all, including chat prose that itself contains a
  tag (`Here is the <scxml> machine:` before a fenced document). Semantic and
  schema rules run only on a well-formed document.
- `SCXML025` cannot prove whether a condition becomes true. It reports the
  bounded structural case where all exits are conditional and eventless; it
  does not diagnose event-triggered reactive waits or claim invalid semantics.
- Reachability (`SCXML029`) over-approximates, so it can miss unreachable
  states (for example a region default that is always overridden by explicit
  multi-targets) but never reports a state that can be entered.
- Transition conflict sets between parallel regions are not diagnosed:
  transitions from one region to another are legal (they exit and re-enter the
  `<parallel>`), and conflicts are resolved by document order at runtime.
- Liveness (`SCXML036`) is proven only for atomic states without a
  `<parallel>` ancestor, single resolved targets, and default entry that lands
  in one atomic state. It does not follow history restoration, `<parallel>`
  entry, completion (`done.state.*` events are queued events, not eventless
  steps), executable content, invokes, or conditions; any `cond` on the
  ancestor chain makes a step undecidable and it is not reported.
- History default targets outside the history's parent are reported by the
  existing `SCXML021` (shallow: immediate child; deep: descendant).
- Inline SCXML documents under `<invoke><content>` are validated as independent
  machine roots with separate ID/target scopes and a 64-root path budget.
  Recursive acquisition of invoked external SCXML is not in the validator Wasm
  boundary because reporters and guests have no filesystem/network imports;
  hosts may validate explicitly supplied external sources as separate artifacts.
- The canonical envelope supports validator, compiler, and conformance payloads.
  Compiler and conformance producers carry their authoritative status,
  dispositions, counts, output metadata, and complete run identity; reporters
  only validate and render those canonical decisions. Conformance report schema
  version 8 (see [conformance.md](conformance.md)) requires manifest/source-set/prepared-corpus digests, revision,
  template and transformer identity (portable argv and optional
  argv-and-artifact digest), schema policy, compiler/engine/data-model/runner/
  manual-equivalent-oracle/IR versions, every filter
  dimension needed to substantiate its counts, and the oracle kind of every
  case decided by an oracle.

## Structural validation with the official W3C schema

Every `validate` run applies the unmodified official W3C SCXML 1.0 XML Schema
set (`scxml.xsd` and its six modules from <https://www.w3.org/2011/04/SCXML/>),
vendored with SHA-256 provenance and license notice in
[`crates/scxml-validator-xsd/schemas/w3c-scxml-1.0/`](../crates/scxml-validator-xsd/schemas/w3c-scxml-1.0/NOTICE.md).
It is the default schema for the exact namespace
`http://www.w3.org/2005/07/scxml` in both the CLI and the validator Wasm
component; an explicitly registered schema for that namespace replaces it.
`xsd-schema` compiles the complete set without modification. Two adapter
extensions are enforced by tests: the work preflight budgets `xs:group ref`
model groups (undefined and circular groups fail closed
as `XSD007`), and the projected schema-compilation budget is 256,000,000 units
against a pinned official-set measurement of 117,570,649.

`xsd-schema` reports instance errors without byte positions, so the adapter
drives validation event by event and attributes each finding to the element
start, attribute, end of attributes, text run, or element end that produced it.
Findings are then labeled at the element name, attribute name, attribute
value, text, or end tag. XSD findings that restate a semantic rule with better
scoping are superseded rather than duplicated:

| XSD finding | Owning rule |
| --- | --- |
| `cvc-id.*` on SCXML elements | `SCXML002`, `SCXML005`, `SCXML006` (per-root ID scopes) |
| lexical errors on SCXML `id`, `<transition target>`, `initial` | `SCXML003`, `SCXML005`, `SCXML006` |
| lexical errors on `<transition event>` | `SCXML013` |
| missing `<param name>` | `SCXML007` |
| `<initial>`/`<history>` missing their `<transition>` or with several | `SCXML009`, `SCXML019` |
| SCXML children or `initial` on `<final>` | `SCXML031` |
| `<donedata>` outside `<final>` or repeated | `SCXML032` |
| `<content>`/`<param>` out of place in a `<donedata>` that has both | `SCXML018` |

Unknown names get rename suggestions, and a rename is machine-applicable
only when it is the one unambiguous repair and keeps the value:

- An undeclared attribute on an SCXML element that the W3C schema declares
  on a related element (`vocabulary::COUNTERPARTS`, each entry citing SCXML
  1.0 and checked against the bundled schema) is renamed to the element's own
  attribute for that role: `sendid` on `<send>` to `id` (§6.2.1, §6.3.1),
  `id` on `<cancel>` to `sendid`, and `expr` on `<transition>`, `<if>`, or
  `<elseif>` to `cond` (§3.5.1, §4.3.1, §4.4.1) are machine-applicable;
  `target` on `<state>` to `initial` (§3.3.1) is maybe-incorrect, because a
  `<transition target>` child may be what was meant. The help says which
  element owns the written attribute, and the owner's section is cited.
- Otherwise the attribute names closest to the unknown one (the bounded
  distance `SCXML005` uses, without its tie-break) among those the element
  declares and does not already carry are suggested: one closest name whose
  declared values admit the value (any value, unless the attribute is an
  enumeration) is machine-applicable (`evnt` to `event`); a tie offers every
  closest name as its own maybe-incorrect fix (`tage` on `<send>`: `target`
  or `type`). On an element outside the SCXML namespace the candidates are
  the unqualified attributes its package schema declares, directly, through
  attribute groups, or from base types, and the rename is maybe-incorrect,
  since the package's value types are not consulted; with no close name the
  help lists them.
- An undeclared SCXML element is renamed the same way to the closest
  children its parent's content model accepts: one closest element that
  declares every unprefixed attribute the misspelled one carries is
  machine-applicable (`<trasition>` to `<transition>`), otherwise each
  closest name is maybe-incorrect. An element in an extension namespace
  under the schema's lax `##other` wildcards is not an XSD finding.
- A rename onto an attribute the element already carries is never offered.

### Schema values of white-space-collapsing attributes

XML Schema 1.1 Part 2 §4.3.6 (`whiteSpace`) fixes the facet to `collapse`
for `xsd:token` and every type derived from it, for `xsd:anyURI`, and for
`xsd:decimal`: "All occurrences of #x9 (tab), #xA (line feed) and #xD
(carriage return) are replaced with #x20 (space)", then "contiguous
sequences of #x20's are collapsed to a single #x20, and initial and/or
final #x20's are deleted". A value of such a type is valid with leading,
trailing, or repeated white space, and means its collapsed form.

In the bundled schema those are `xsd:ID` (every `id`), `xsd:IDREF`
(`<cancel sendid>`), `xsd:IDREFS` (`initial`, `<transition target>`),
`xsd:NMTOKEN` (`<scxml name>`, `<scxml datamodel>`, `<raise event>`,
`<param name>`, `<assign attr>`), `xsd:decimal` (`<scxml version>`), and the
schema's types restricted from `xsd:token`, `NMTOKEN`, `NMTOKENS`, or
`xsd:anyURI` (`EventType`, `EventTypes`, `Binding`, `Exmode`,
`TransitionType`, `AssignType`, `Boolean`, `URI`: `<send event>`,
`<transition event>`, `<scxml binding>`, `<scxml exmode>`,
`<transition type>`, `<assign type>`, `<invoke autoforward>`, and every
`src` and `<send target>`). Every semantic and expression rule reads these
attributes' collapsed values, exactly as lowering and therefore the engine
do: `<state id=" s ">` declares `s`, so `target="s"`, `initial="s"`, and
`In('s')` resolve to it and `<final id="s&#9;">` beside it is a duplicate
(`SCXML002`); `datamodel=" xpath "` selects XPath; and every length bound
(`SCXML069`, `SCXML081`, `SCXML082`) measures the collapsed value, except
inside inline XML (`<data>`, `<content>`, `<assign>`, and custom-action
content): Machine IR keeps it byte for byte, so `SCXML069` measures an
attribute there, even on an element in the SCXML namespace, by its exact
value. A
diagnostic labels the collapsed value's bytes when the value has no
character references. List items (`IDREFS`, the event-descriptor list,
`namelist`) are separated by XML white space only (#x20, #x9, #xA, #xD), not
by other Unicode spaces such as U+00A0. `<history type>`, `<send type>`,
`<invoke type>`, `delay`, `namelist`, `<foreach item/index>`, `<log label>`,
and every expression are `xsd:string` (or restricted from it) and keep their
exact value. `scxml_validator_core::COLLAPSED_ATTRIBUTES` lists the
collapsing attributes; `crates/scxml-validator-core/tests/collapsed_attributes.rs`
derives the same set from the bundled schema's attribute groups and types,
and `validate_covers_lowering.rs` proves padded values validate and lower to
their collapsed values (including exactly at the name bounds) and that a
padded value over a bound is rejected by both.

## Presentation contract

Every diagnostic in the canonical artifact satisfies these invariants, which
the UI snapshot suite checks for every case:

- The primary span is the most specific responsible bytes: an attribute value
  or name, an element name, a single token of a list attribute, an `&…;`
  reference, or a text run. Element-level findings never label a whole
  multi-line start tag; a report-wide precision pass narrows any start-tag span
  to the element name.
- A second location involved in the finding (the first declaration, the
  similarly named state, the conflicting attribute, the parent content model,
  the closing tag) is a secondary label.
- `note:` explains why the constraint exists and `help:` gives the concrete
  repair; when a rule supplies neither, the canonical explain table below
  supplies them.
- A normative citation (`spec:`) names the SCXML, XML, or XML Schema section.
- The `--> file:line:column` header uses the one-based Unicode scalar
  (character) column, as rustc does, so an editor or agent jumps to the exact
  character; display width is used only to align carets under tabs and wide
  characters. JSON keeps exact UTF-8 byte spans and SARIF reports UTF-16 code
  unit columns (`columnKind: utf16CodeUnits`). Source lines wider than 120
  display columns, including suggestion lines, are shown as a window around
  the labels with `...` elisions. A label that spans several lines (such as
  "while parsing this `<script>`" over a CDATA body) is drawn with rustc's
  multi-line markers: a `_` run from a left-margin lane to its first column,
  the lane `|` down the lines it covers, and `|_` to its last column with the
  message.
- Structured fixes carry rustc applicability: `machine-applicable` (safe to
  apply unattended), `maybe-incorrect` (plausible, review it),
  `has-placeholders` (contains `…` to replace), or `unspecified`. The rustc
  reporter renders insertions and replacements inline (`+` and `~`), and a
  fix that deletes anything, alone or with other edits (such as moving a
  transition), as a `-`/`+` diff of the changed lines. Edits on nearby lines
  form one block and blocks are separated by `...`, as rustc separates the
  parts of a multipart suggestion, so every edit of a multi-edit fix is
  visible (a rename's far references, both Markdown fences); a block longer
  than 12 lines, or blocks after the 32nd, are elided with an explicit
  `...` marker, never silently. SARIF carries them as `fixes[].artifactChanges` with
  line/column and byte regions and `properties.applicability`.
- Applying every machine-applicable fix of a UI case removes those diagnostics
  and introduces no new diagnostic of any severity, schema findings included;
  applying any single fix of any applicability removes one finding with the
  code that offers it, and (except a placeholder) introduces none either (the
  rustfix round-trip tests). After an XML parse failure, later phases had not run, so
  only new errors count. The few maybe-incorrect fixes whose application
  legitimately reveals an independent pre-existing finding are listed with
  their reason in `crates/scxml-cli/tests/ui.rs`.
- Spans index the original bytes, including a leading UTF-8 byte order mark
  (XML 1.0 §4.3.3); line/column positions (rustc header, SARIF regions) count
  from the first character after it, as editors and rustc do.

XML trust-boundary failures have one code per `scxml_syntax::XmlErrorKind`:
`XML001` malformed, `XML002` resource limit, `XML003` undefined entity
reference (labels the exact `&name;` bytes; HTML entities get a
machine-applicable numeric-reference fix, anything else a maybe-incorrect
`&amp;` escape), `XML004` invalid character reference, `XML005` invalid
caller-supplied limits, and `XML006` an entity the parser does not read.

The parser processes the internal DTD subset as XML 1.0 §5.1 requires of a
non-validating processor ("MUST use the information in those declarations to
normalize attribute values, include the replacement text of internal
entities, and supply default attribute values"):

- each `<!ENTITY name "value">` declaration (character references replaced
  at declaration, the first declaration binding, §4.2, §4.5) is included
  where it is referenced: in element content (§4.4.2), as character data, or,
  when its replacement text holds markup, parsed as `content` in place of
  the reference (§4.3.2; every node it adds spans the reference), and in
  attribute values (§4.4.5, then normalized, §3.3.3);
- a parameter-entity reference between declarations is included as PE
  (§4.4.8) and its replacement text processed as declarations, conditional
  sections (`INCLUDE`, `IGNORE`) included (§2.8 WFC: PE Between
  Declarations, §3.4); one inside a markup declaration is `XML001` (§2.8
  WFC: PEs in Internal Subset);
- `<!ATTLIST>` declarations (merged, the first definition of an attribute
  binding, §3.3) supply default values, normalized at declaration (§3.3.3;
  a defaulted `xmlns` attribute declares a namespace), and every value of an
  attribute declared with a type other than CDATA is further normalized;
  a supplied default spans its literal in the DTD.

Inclusion is bounded (256 general and 256 parameter entity declarations,
1024 attribute definitions, 4096 processed markup declarations, 64 KiB per
reference, 1 MiB of included text and supplied defaults per document,
1,048,576 entity references expanded into attribute defaults per document,
16 levels of nesting, and the tree limits; over a bound is `XML002`, so
exponential "billion laughs" expansion fails before the text is built). The
declaration counts and the tree's size limits apply to wasm builds only; the
markup-declaration, expansion, and nesting limits, and the 256-element tree
depth, guard against amplification and the native stack and apply to every
build. Recursion is
recursion is `XML001` (§4.1 WFC: No Recursion). An external entity is never
read (§4.4.3 lets a non-validating processor not include one; reading it
could disclose local files or reach the network): a reference to one in
content is `XML006`, and one in an attribute value `XML001` (§3.1 WFC: No
External Entity References). After a reference to a parameter entity that is
not read, later entity and attribute-list declarations are not processed
(§5.1), and a reference to an entity only they would declare is `XML006`.
XML Schema validation assesses the infoset (XML Schema 1.0 Part 1 §2.1), so
a document with an internal DTD subset is validated as the serialization of
the infoset the parser built, with every finding mapped back to its element.

`XML001` classifies the well-formedness violations AI authors and editors
produce (`scxml_syntax::XmlErrorDetail`) and gives each its exact span,
constraint citation, and repair; parser-internal wording and byte positions are
never shown:

| Violation | Primary span | Repair |
| --- | --- | --- |
| Text, a reference, or CDATA outside the root (XML 1.0 §2.1): a Markdown code fence, chat prose, a second BOM | first line of the first run; further runs are secondary labels | one fix removing every run (machine-applicable when the removed text contains no markup) |
| `<` in an attribute value (§3.1 WFC: No < in Attribute Values) | the `<` | machine-applicable `&lt;` for every `<` in the value |
| `<` in text with no element name after it (§2.4), e.g. a comparison in `<script>` | the `<` | machine-applicable `&lt;`; help names CDATA for scripts |
| `--` in a comment or a comment ending `--->` (§2.5) | the `--` or trailing `-` | machine-applicable collapse of repeated hyphens |
| XML declaration not at the very start (§2.8) | the declaration | machine-applicable removal of leading whitespace, otherwise a maybe-incorrect removal of the declaration |
| Duplicate attribute (§3.1 WFC: Unique Att Spec) | the repeated attribute; the first is secondary | maybe-incorrect removal of the repeat |
| Second root element (§2.1) | the second root's name; the first root is secondary | help only |
| Mismatched end tag | the end-tag name; the unclosed start tag is secondary | maybe-incorrect self-close, close, or (for a misspelling such as `</stat>`) rename |
| Chat prose that contains markup around an SCXML-namespaced document (`Here is the <scxml> machine:` then a fenced document) | the first prose line; the real root and trailing text are secondary | one fix keeping only the document (machine-applicable when Markdown fences enclose it, maybe-incorrect otherwise), offered only when that document parses on its own |
| A document that ends with elements still open (a truncated document, a missing `</scxml>`) | the last character of the document; the innermost unclosed start tag is secondary | maybe-incorrect insertion of every missing end tag, one per line, verified by re-parsing (at most 32) |
| An attribute without `=value` (`<parallel id="p" final>`, `<else if cond="…"/>`) | the attribute name (`else if` as a whole) | maybe-incorrect `<elseif>` rename for `else if`, otherwise a maybe-incorrect removal, each offered only when it removes the error |
| An attribute value whose closing quote is missing (`<state id="a>`) | the opening quote | maybe-incorrect closing quote where the value was meant to end (before `>`, `/>`, or the line end), offered only when it removes the error |
| Bytes that are not UTF-8 | the first invalid sequence (shown as U+FFFD in the report's source text, so every format has a line and column) | help only: save as UTF-8 or use a character reference |

`crates/scxml-syntax/tests/well_formedness.rs` is the curated not-well-formed
conformance set for these constructs (and their legal neighbours, such as a
UTF-8 byte order mark, `>` in an attribute, and comments after the root);
every case must be rejected at the responsible bytes, and `compile` rejects
them because it validates first.

## Diagnostic code reference

Errors fall into two kinds. Most reject a document the specification or its
schema makes non-conformant and for which it defines no run-time behavior;
a condition whose specification-defined outcome is a run-time error event
(`error.execution`, `error.communication`) is a warning instead (for example
`SCXML038`, `SCXML039`, `SCXML056`, `SCXML066`, `SCXML077`, `SCXML079`). The
rest are this implementation's limits, not specification rules: `SCXML027`,
`SCXML028`, `SCXML040`, `SCXML041`, `SCXML069`, `COMP001`, and the `XML`,
`XSD`, and package resource codes reject a document over a bound the
toolchain can process (SCXML 1.0 Appendix A.2: "There is no conformance
requirement with respect to performance characteristics of the SCXML 1.0
Processor"). Resource bounds apply to wasm builds only
(`scxml_types::wasm_bound`): the validator and compiler components report
them, and the native `scx` never reports `SCXML027`, `SCXML041`,
`SCXML069`, or `COMP001`, nor `SCXML028`, `SCXML040`, `XML002`, or `XSD006`
for a size bound. What stays on every build guards against amplification or
the native stack rather than bounding a resource: the XML depth (256), the
DTD expansion limits, the XSD content-model expansion and compilation
budgets, the ECMAScript side-effect analysis budget (200,000 steps per
checked fragment; exhausting it is one undecidable `SCXML075`
warning, not a rejection), the graph analysis work budget (`SCXML028`,
above), and the ECMAScript (128 units) and XPath (32 levels) parse nesting.
`SCXML044` and `SCXML068` reject a data model or foreign markup this
processor does not support. `SCXML074` is the processor's own way of
meeting SCXML 1.0 §4.7.3 ("The SCXML processor MUST ensure that <log> has no
side-effects on document interpretation"), not a document constraint.

`scx --explain CODE` prints the long form of any code below (what, why,
how to fix, an erroneous and a corrected example, and the specification link).
Every example that is a complete document is validated as `scx validate`
validates it (bundled schema, both expression checkers) by
`crates/scxml-validator-core/tests/explain_examples.rs`: the erroneous example
must emit its code and the corrected example must produce no diagnostic of any
severity, so a copied correction is clean. The few entries that cannot be
checked by `validate` (compile-only and package codes, schema-authoring codes,
and resource bounds) are listed there with their reason, and their examples
are shown as `text`, never as XML, unless another test checks them: the
document examples of the compile-only `COMP` codes are compiled by
`crates/scxml-compiler-core/tests/explain_examples.rs` (the erroneous example
must emit its code and the corrected one must compile with no diagnostic).
The table is generated from `scxml_reporting::explain::CODES`, the same
source the CLI uses; a unit test fails when this section drifts
(`sh scripts/bless-fixtures.sh --validator-rules` regenerates it). Codes written `XSD-cvc-….*` document every
backend constraint code with that prefix. An `XSD-cvc-*` code is the literal
XML Schema 1.0 validation-rule identifier (for example `cvc-complex-type.2.4`,
defined by the W3C recommendation, not by the backend library), prefixed with
`XSD-`; these identifiers are fixed by the specification, so the codes are as
stable as the SCXML-owned `SCXMLnnn` codes. XSD findings that an SCXML rule
states more precisely are superseded by that rule (table above).

<!-- BEGIN GENERATED: diagnostic codes (scxml_reporting::explain::markdown_table) -->
| Code | Severity | Category | Summary | Specification |
| --- | --- | --- | --- | --- |
| `COMP001` | error | internal | the validated document could not be lowered to Machine IR | [SCXML 1.0 §3.2 <scxml>](https://www.w3.org/TR/scxml/#scxml) |
| `COMP002` | error | package | a used extension namespace has no verified extension package | [SCXML 1.0 §4.10 Extensibility of Executable Content](https://www.w3.org/TR/scxml/#extensibility) |
| `COMP003` | error | package | a `<script src>` cannot be resolved or authenticated at compile time | [SCXML 1.0 §5.8 <script>](https://www.w3.org/TR/scxml/#script) |
| `PKG001` | error | package | an extension package manifest does not match its schema binding | [SCXML 1.0 §4.10 Extensibility of Executable Content](https://www.w3.org/TR/scxml/#extensibility) |
| `PKG002` | error | package | a package validator failed or returned diagnostics outside its contract | [SCXML 1.0 §4.10 Extensibility of Executable Content](https://www.w3.org/TR/scxml/#extensibility) |
| `SCXML001` | error | semantic | the root element is not `<scxml>` in the SCXML namespace | [SCXML 1.0 §3.2 <scxml>](https://www.w3.org/TR/scxml/#scxml) |
| `SCXML002` | error | semantic | an identifier is declared more than once | [SCXML 1.0 §3.14 IDs](https://www.w3.org/TR/scxml/#IDs) |
| `SCXML003` | error | semantic | an identifier is not an XML NCName | [SCXML 1.0 §3.14 IDs](https://www.w3.org/TR/scxml/#IDs) |
| `SCXML004` | error | semantic | a transition has no event, condition, or target | [SCXML 1.0 §3.5 <transition>](https://www.w3.org/TR/scxml/#transition) |
| `SCXML005` | error | semantic | a transition target does not name a state in this document | [SCXML 1.0 §3.5 <transition>](https://www.w3.org/TR/scxml/#transition) |
| `SCXML006` | error | semantic | an `initial` attribute names a state that does not exist | [SCXML 1.0 §3.2 <scxml>](https://www.w3.org/TR/scxml/#scxml) |
| `SCXML007` | error | semantic | a `<param>` lacks `name` or has zero or two value sources | [SCXML 1.0 §5.7 <param>](https://www.w3.org/TR/scxml/#param) |
| `SCXML008` | error | semantic | a `<cancel>` has no send ID or both `sendid` and `sendidexpr` | [SCXML 1.0 §6.3 <cancel>](https://www.w3.org/TR/scxml/#cancel) |
| `SCXML009` | error | semantic | an `<initial>` does not contain exactly one `<transition>` | [SCXML 1.0 §3.6 <initial>](https://www.w3.org/TR/scxml/#initial) |
| `SCXML010` | error | semantic | an `<initial>` transition has an event, a condition, or no target | [SCXML 1.0 §3.6 <initial>](https://www.w3.org/TR/scxml/#initial) |
| `SCXML011` | error | semantic | a state uses both an `initial` attribute and an `<initial>` child | [SCXML 1.0 §3.3 <state>](https://www.w3.org/TR/scxml/#state) |
| `SCXML012` | error | semantic | an atomic state declares an initial configuration | [SCXML 1.0 §3.3 <state>](https://www.w3.org/TR/scxml/#state) |
| `SCXML013` | error | semantic | a transition `event` is not a valid event-descriptor list | [SCXML 1.0 §3.12.1 Event Descriptors](https://www.w3.org/TR/scxml/#EventDescriptors) |
| `SCXML014` | error | semantic | a `<send>` uses both forms of one property (a literal attribute and its `…expr` or `idlocation` alternative) | [SCXML 1.0 §6.2 <send>](https://www.w3.org/TR/scxml/#send) |
| `SCXML015` | error | semantic | a `<send>` combines `<content>` with `namelist` or `<param>` | [SCXML 1.0 §6.2 <send>](https://www.w3.org/TR/scxml/#send) |
| `SCXML016` | error | semantic | an `<invoke>` has conflicting source or type forms | [SCXML 1.0 §6.4 <invoke>](https://www.w3.org/TR/scxml/#invoke) |
| `SCXML017` | error | semantic | an `initial` attribute targets a state outside its element | [SCXML 1.0 §3.3 <state>](https://www.w3.org/TR/scxml/#state) |
| `SCXML018` | error | semantic | a `<donedata>` contains both `<content>` and `<param>` | [SCXML 1.0 §5.5 <donedata>](https://www.w3.org/TR/scxml/#donedata) |
| `SCXML019` | error | semantic | a `<history>` does not contain exactly one `<transition>` | [SCXML 1.0 §3.10 <history>](https://www.w3.org/TR/scxml/#history) |
| `SCXML020` | error | semantic | a `<history>` transition has an event, a condition, or no target | [SCXML 1.0 §3.10 <history>](https://www.w3.org/TR/scxml/#history) |
| `SCXML021` | error | semantic | a history default targets a state outside what the history can restore | [SCXML 1.0 §3.10 <history>](https://www.w3.org/TR/scxml/#history) |
| `SCXML022` | error | semantic | an `<initial>` transition targets a state outside its parent | [SCXML 1.0 §3.6 <initial>](https://www.w3.org/TR/scxml/#initial) |
| `SCXML023` | warning | lint | a compound state relies on document order for its initial child | [SCXML 1.0 §3.3 <state>](https://www.w3.org/TR/scxml/#state) |
| `SCXML024` | warning | lint | a non-final atomic state has no exit | [SCXML 1.0 §3.3 <state>](https://www.w3.org/TR/scxml/#state) |
| `SCXML025` | warning | lint | every exit from a state is a conditional eventless transition | [SCXML 1.0 §3.13 Selecting and Executing Transitions](https://www.w3.org/TR/scxml/#SelectingTransitions) |
| `SCXML026` | warning | lint | unconditional eventless transitions form a cycle | [SCXML 1.0 §3.13 Selecting and Executing Transitions](https://www.w3.org/TR/scxml/#SelectingTransitions) |
| `SCXML027` | error | semantic | inline `<scxml>` documents nest deeper than the validation budget | [SCXML 1.0 §6.4 <invoke>](https://www.w3.org/TR/scxml/#invoke) |
| `SCXML028` | error | semantic | a machine is too large for bounded semantic graph analysis | [SCXML 1.0 §3.11 Legal State Configurations and Specifications](https://www.w3.org/TR/scxml/#LegalStateConfigurations) |
| `SCXML029` | warning | lint | a state can never become active | [SCXML 1.0 §3.11 Legal State Configurations and Specifications](https://www.w3.org/TR/scxml/#LegalStateConfigurations) |
| `SCXML030` | error | semantic | a multi-target set cannot be active at the same time | [SCXML 1.0 §3.11 Legal State Configurations and Specifications](https://www.w3.org/TR/scxml/#LegalStateConfigurations) |
| `SCXML031` | error | semantic | a `<final>` state contains something other than `<onentry>`, `<onexit>`, or `<donedata>` | [SCXML 1.0 §3.7 <final>](https://www.w3.org/TR/scxml/#final) |
| `SCXML032` | error | semantic | `<donedata>` appears outside `<final>` or more than once | [SCXML 1.0 §5.5 <donedata>](https://www.w3.org/TR/scxml/#donedata) |
| `SCXML033` | warning | lint | a `<parallel>` has a single region | [SCXML 1.0 §3.4 <parallel>](https://www.w3.org/TR/scxml/#parallel) |
| `SCXML034` | warning | lint | a transition is shadowed by an earlier unconditional transition in the same state | [SCXML 1.0 §3.13 Selecting and Executing Transitions](https://www.w3.org/TR/scxml/#SelectingTransitions) |
| `SCXML036` | warning | lint | eventless transitions loop forever through default entry | [SCXML 1.0 §3.13 Selecting and Executing Transitions](https://www.w3.org/TR/scxml/#SelectingTransitions) |
| `SCXML037` | error | semantic | the null data model is used with an element it does not support | [SCXML 1.0 §B.1 The Null Data Model](https://www.w3.org/TR/scxml/#minimal-profile) |
| `SCXML038` | warning | semantic | an expression or script does not parse in the selected data model | [SCXML 1.0 §B.2 The ECMAScript Data Model](https://www.w3.org/TR/scxml/#ecma-profile) |
| `SCXML039` | warning | semantic | a location is not assignable in the selected data model | [SCXML 1.0 §5.9.2 Location Expressions](https://www.w3.org/TR/scxml/#LocationExpressions) |
| `SCXML040` | error | semantic | an expression exceeds the data model's size or nesting bound | [SCXML 1.0 §5.9 Expressions](https://www.w3.org/TR/scxml/#expressions) |
| `SCXML041` | error | semantic | a document has too many expressions for bounded syntax checking | [SCXML 1.0 §5.9 Expressions](https://www.w3.org/TR/scxml/#expressions) |
| `SCXML042` | warning | lint | an expression attribute is present but empty | [SCXML 1.0 §5.9 Expressions](https://www.w3.org/TR/scxml/#expressions) |
| `SCXML044` | error | semantic | the `datamodel` attribute names a data model this validator does not recognize | [SCXML 1.0 §5.1 Data Model](https://www.w3.org/TR/scxml/#data-module) |
| `SCXML045` | note | lint | a machine's expressions were not syntax-checked | [SCXML 1.0 §5.1 Data Model](https://www.w3.org/TR/scxml/#data-module) |
| `SCXML050` | error | semantic | a `<script>` has both `src` and inline content | [SCXML 1.0 §5.8 <script>](https://www.w3.org/TR/scxml/#script) |
| `SCXML051` | error | semantic | a `<script src>` does not name a fetchable script | [SCXML 1.0 §5.8 <script>](https://www.w3.org/TR/scxml/#script) |
| `SCXML052` | error | semantic | a `<send>` to the SCXML event processor names no event | [SCXML 1.0 §6.2 <send>](https://www.w3.org/TR/scxml/#send) |
| `SCXML053` | error | semantic | a `<data>`, `<content>`, or `<assign>` has more than one value source | [SCXML 1.0 §5.3 <data>](https://www.w3.org/TR/scxml/#data) |
| `SCXML055` | error | semantic | a `<finalize>` raises or sends an event | [SCXML 1.0 §6.5 <finalize>](https://www.w3.org/TR/scxml/#finalize) |
| `SCXML056` | warning | semantic | an XPath expression references a variable the machine never declares | [SCXML CR 2014 §C.3.1 Data Model](https://www.w3.org/TR/2014/CR-scxml-20140313/#xpath-datamodel) |
| `SCXML057` | warning | lint | an XPath expression starts with a relative path that selects no data | [SCXML CR 2014 §C.3.1 Data Model](https://www.w3.org/TR/2014/CR-scxml-20140313/#xpath-datamodel) |
| `SCXML058` | warning | lint | an `In()` condition names no state of the machine or passes the state ID unquoted | [SCXML 1.0 §5.9.1 Conditional Expressions](https://www.w3.org/TR/scxml/#ConditionalExpressions) |
| `SCXML059` | warning | lint | an XPath predicate selects by a position that no node has | [XPath 1.0 §2.4 Predicates](https://www.w3.org/TR/1999/REC-xpath-19991116/#predicates) |
| `SCXML060` | warning | lint | an XPath expression adds a string with `+` | [XPath 1.0 §3.5 Numbers](https://www.w3.org/TR/1999/REC-xpath-19991116/#numbers) |
| `SCXML061` | warning | semantic | an ECMAScript `<data id>` is not an identifier, so no expression can name it | [SCXML 1.0 §B.2 The ECMAScript Data Model](https://www.w3.org/TR/scxml/#ecma-profile) |
| `SCXML062` | warning | lint | a Unicode bidirectional control character changes how the source is displayed | [Unicode Standard Annex #9 §2 Directional Formatting Characters](https://www.unicode.org/reports/tr9/#Directional_Formatting_Characters) |
| `SCXML063` | warning | lint | an ECMAScript condition is an arrow function, so it is always true | [SCXML 1.0 §5.9.1 Conditional Expressions](https://www.w3.org/TR/scxml/#ConditionalExpressions) |
| `SCXML064` | error | semantic | a `<send>`, `<invoke>`, or `<state>` repeats a child it allows once | [SCXML 1.0 §6.2 <send>](https://www.w3.org/TR/scxml/#send) |
| `SCXML065` | error | semantic | an `<assign>` has no value, or its value does not fit its `type` | [SCXML 1.0 §5.4 <assign>](https://www.w3.org/TR/scxml/#assign) |
| `SCXML066` | warning | semantic | a required expression attribute is empty | [SCXML 1.0 §3.12 <if>](https://www.w3.org/TR/scxml/#if) |
| `SCXML067` | error | semantic | an `<scxml>` has no child states | [SCXML 1.0 §3.2 <scxml>](https://www.w3.org/TR/scxml/#scxml) |
| `SCXML068` | error | internal | this compiler cannot lower an attribute or element from another namespace here | — |
| `SCXML069` | error | semantic | the document exceeds a Machine IR bound | — |
| `SCXML070` | error | semantic | a `<send>` to `#_internal` has a delay | [SCXML 1.0 §6.2 <send>](https://www.w3.org/TR/scxml/#send) |
| `SCXML071` | error | semantic | an `<invoke>` combines `namelist` with `<param>` | [SCXML 1.0 §6.4 <invoke>](https://www.w3.org/TR/scxml/#invoke) |
| `SCXML072` | warning | lint | a target list names the same state twice | [SCXML 1.0 §3.11 Legal State Configurations and Specifications](https://www.w3.org/TR/scxml/#LegalStateConfigurations) |
| `SCXML073` | error | semantic | a `<data id>` begins with `_` | [SCXML 1.0 §5.10 System Variables](https://www.w3.org/TR/scxml/#SystemVariables) |
| `SCXML074` | error | semantic | a `<log expr>` has a side effect | [SCXML 1.0 §4.7.3 <log>](https://www.w3.org/TR/scxml/#log) |
| `SCXML075` | warning | lint | the validator cannot prove a condition or `<log expr>` has no side effect | [SCXML 1.0 Appendix B Data Models](https://www.w3.org/TR/scxml/#profiles) |
| `SCXML076` | warning | lint | a condition has a side effect | [SCXML 1.0 §5.9.1 Conditional Expressions](https://www.w3.org/TR/scxml/#ConditionalExpressions) |
| `SCXML077` | warning | semantic | an expression or `<script>` changes a system variable itself | [SCXML 1.0 §5.10 System Variables](https://www.w3.org/TR/scxml/#SystemVariables) |
| `SCXML078` | warning | semantic | a `<parallel>` has no child states, so no transition is enabled through it | [SCXML 1.0 §3.4 <parallel>](https://www.w3.org/TR/scxml/#parallel) |
| `SCXML079` | warning | semantic | an expression is used under the null data model | [SCXML 1.0 §B.1 The Null Data Model](https://www.w3.org/TR/scxml/#minimal-profile) |
| `SCXML080` | warning | semantic | a target set combines a `<history>` with a state it restores | [SCXML 1.0 §3.11 Legal State Configurations and Specifications](https://www.w3.org/TR/scxml/#LegalStateConfigurations) |
| `SCXML081` | error | semantic | a state ID is longer than a state name may be | [SCXML 1.0 §3.14 IDs](https://www.w3.org/TR/scxml/#IDs) |
| `SCXML082` | error | semantic | an event name or a send or invoke ID is longer than a name may be | [SCXML 1.0 §6.4 <invoke>](https://www.w3.org/TR/scxml/#invoke) |
| `SCXML083` | error | semantic | an `env` attribute names a slot its invoked service does not have | [SCXML 1.0 §6.4 <invoke>](https://www.w3.org/TR/scxml/#invoke) |
| `SCXML084` | error | semantic | the `env` namespace is used outside an invoked service's `<invoke>` attributes | [SCXML 1.0 §6.4 <invoke>](https://www.w3.org/TR/scxml/#invoke) |
| `SCXML085` | error | semantic | the `event` namespace is used outside the attributes of an element with event data | — |
| `SCXML086` | error | semantic | an element gives both `event:schema` and `event:schemaexpr` | — |
| `SCXML087` | error | syntax | an `event:schema` is not JSON | [RFC 8259 The JavaScript Object Notation (JSON) Data Interchange Format](https://www.rfc-editor.org/rfc/rfc8259) |
| `SCXML088` | error | semantic | an `event:schema` file is not available | — |
| `SCXML089` | error | semantic | the fragment of an `event:schema` reference selects no schema | [RFC 6901 JavaScript Object Notation (JSON) Pointer](https://www.rfc-editor.org/rfc/rfc6901) |
| `SCXML090` | error | semantic | an `event:schema` is not a valid JSON Schema | [JSON Schema 2020-12 Validation](https://json-schema.org/draft/2020-12/json-schema-validation) |
| `SCXML091` | error | semantic | an `event:schema` declares an unsupported JSON Schema dialect | [JSON Schema 2020-12 Core §8.1.1 The $schema Keyword](https://json-schema.org/draft/2020-12/json-schema-core#section-8.1.1) |
| `SCXML092` | error | semantic | an `event:schema` has a reference the compiler cannot keep | [JSON Schema 2020-12 Core §8.2.3.1 Direct References with $ref](https://json-schema.org/draft/2020-12/json-schema-core#section-8.2.3.1) |
| `SCXML093` | warning | semantic | an `<invoke type>` is a platform-specific value, not a URI | [SCXML 1.0 §6.4.1 Attribute Details](https://www.w3.org/TR/scxml/#invoke) |
| `SCXML094` | warning | package | no service package binds a literal `<invoke type>` | [SCXML 1.0 §6.4 <invoke>](https://www.w3.org/TR/scxml/#invoke) |
| `SCXML097` | error | semantic | a draft declares or uses a namespace the draft profile does not admit | [SCXML 1.0 §3.2 <scxml>](https://www.w3.org/TR/scxml/#scxml) |
| `SCXML098` | error | semantic | a draft holds an `<invoke>` or `<script>` element | [SCXML 1.0 §6.4 <invoke>](https://www.w3.org/TR/scxml/#invoke) |
| `SCXML099` | error | semantic | a draft has a `src` or `srcexpr` attribute | [SCXML 1.0 §5.3 <data>](https://www.w3.org/TR/scxml/#data) |
| `SCXML100` | error | semantic | a draft names a type by expression or sends through another Event I/O Processor | [SCXML 1.0 §6.2 <send>](https://www.w3.org/TR/scxml/#send) |
| `SCXML101` | error | semantic | a draft has a document type declaration | [XML 1.0 (Fifth Edition) §2.8 Prolog and Document Type Declaration](https://www.w3.org/TR/xml/#sec-prolog-dtd) |
| `SCXML102` | error | semantic | the literal data of a `<send>` or `<donedata>` does not satisfy its own `event:schema` | [JSON Schema 2020-12 Validation](https://json-schema.org/draft/2020-12/json-schema-validation) |
| `SCXML103` | warning | semantic | a `<send>` or `<donedata>` creates event data a transition handling it does not accept | [SCXML 1.0 §3.12.1 Event Descriptors](https://www.w3.org/TR/scxml/#EventDescriptors) |
| `XML001` | error | syntax | the document is not well-formed XML | [XML 1.0 (Fifth Edition) §2.1 Well-Formed XML Documents](https://www.w3.org/TR/xml/#sec-well-formed) |
| `XML002` | error | syntax | the document exceeds a configured XML resource limit | — |
| `XML003` | error | syntax | an entity reference names no predefined or declared entity | [XML 1.0 (Fifth Edition) §4.6 Predefined Entities](https://www.w3.org/TR/xml/#sec-predefined-ent) |
| `XML004` | error | syntax | a character reference names no legal XML character | [XML 1.0 (Fifth Edition) §4.1 Character and Entity References](https://www.w3.org/TR/xml/#sec-references) |
| `XML005` | error | internal | the caller configured an invalid XML limit | — |
| `XML006` | error | syntax | the document relies on an entity the parser does not read | [XML 1.0 (Fifth Edition) §4.4 XML Processor Treatment of Entities and References](https://www.w3.org/TR/xml/#entproc) |
| `XSD001` | error | schema | no XML Schema is registered for a namespace used by the document | [SCXML 1.0 §4.10 Extensibility of Executable Content](https://www.w3.org/TR/scxml/#extensibility) |
| `XSD002` | error | schema | a supplied schema is not parseable XML Schema | [XML Schema 1.0 Part 1 §3.15 Schemas as a Whole](https://www.w3.org/TR/xmlschema-1/#Schemas) |
| `XSD003` | error | schema | a supplied schema set does not compile | [XML Schema 1.0 Part 1 §5 Schemas and Namespaces: Access and Composition](https://www.w3.org/TR/xmlschema-1/#composition) |
| `XSD004` | error | schema | a schema content model cannot be prepared for validation | [XML Schema 1.0 Part 1 §3.8.6 Unique Particle Attribution](https://www.w3.org/TR/xmlschema-1/#cos-nonambig) |
| `XSD005` | error | schema | the XML Schema instance driver could not complete | [XML Schema 1.0 Part 1 §5.2 Assessing Schema-Validity](https://www.w3.org/TR/xmlschema-1/#validation_outcome) |
| `XSD006` | error | schema | schema or instance validation work exceeds a bounded budget | [XML Schema 1.0 Part 1 §3.9 Particles](https://www.w3.org/TR/xmlschema-1/#cParticles) |
| `XSD007` | error | schema | a schema model-group reference cannot be budgeted | [XML Schema 1.0 Part 1 §3.7 Model Group Definitions](https://www.w3.org/TR/xmlschema-1/#cModel_Group_Definitions) |
| `XSD-src-resolve.*` | error | schema | the governing schema references a component it does not define | [XML Schema 1.0 Part 1 §3.15.3 Constraints on XML Representations of Schemas](https://www.w3.org/TR/xmlschema-1/#src-resolve) |
| `XSD-cvc-attribute.*` | error | schema | an attribute value is not valid for its declared type | [XML Schema 1.0 Part 1 §3.2.4 Attribute Locally Valid](https://www.w3.org/TR/xmlschema-1/#cvc-attribute) |
| `XSD-cvc-complex-type.*` | error | schema | an element's content or attributes violate its complex type | [XML Schema 1.0 Part 1 §3.4.4 Element Locally Valid (Complex Type)](https://www.w3.org/TR/xmlschema-1/#cvc-complex-type) |
| `XSD-cvc-complex-type.2.3.*` | error | schema | an element with element-only content contains text | [XML Schema 1.0 Part 1 §3.4.4 Element Locally Valid (Complex Type)](https://www.w3.org/TR/xmlschema-1/#cvc-complex-type) |
| `XSD-cvc-complex-type.2.4.*` | error | schema | a child element is not allowed at this position, or required children are missing | [XML Schema 1.0 Part 1 §3.4.4 Element Locally Valid (Complex Type)](https://www.w3.org/TR/xmlschema-1/#cvc-complex-type) |
| `XSD-cvc-complex-type.3.2.2.*` | error | schema | an attribute is not declared for this element | [XML Schema 1.0 Part 1 §3.4.4 Element Locally Valid (Complex Type)](https://www.w3.org/TR/xmlschema-1/#cvc-complex-type) |
| `XSD-cvc-complex-type.4.*` | error | schema | a required attribute is missing | [XML Schema 1.0 Part 1 §3.4.4 Element Locally Valid (Complex Type)](https://www.w3.org/TR/xmlschema-1/#cvc-complex-type) |
| `XSD-cvc-datatype-valid.*` | error | schema | a value is not in the lexical space of its datatype | [XML Schema 1.0 Part 2 §4.1.4 Datatype Valid](https://www.w3.org/TR/xmlschema-2/#cvc-datatype-valid) |
| `XSD-cvc-elt.*` | error | schema | an element is not valid against its declaration | [XML Schema 1.0 Part 1 §3.3.4 Element Locally Valid (Element)](https://www.w3.org/TR/xmlschema-1/#cvc-elt) |
| `XSD-cvc-elt.1.*` | error | schema | an element is not declared in its namespace | [XML Schema 1.0 Part 1 §3.3.4 Element Locally Valid (Element)](https://www.w3.org/TR/xmlschema-1/#cvc-elt) |
| `XSD-cvc-enumeration-valid.*` | error | schema | a value is not one of the allowed enumerated values | [XML Schema 1.0 Part 2 §4.3.5.4 Enumeration valid](https://www.w3.org/TR/xmlschema-2/#rf-enumeration) |
| `XSD-cvc-pattern-valid.*` | error | schema | a value does not match its datatype's pattern | [XML Schema 1.0 Part 2 §4.3.4.4 Pattern valid](https://www.w3.org/TR/xmlschema-2/#rf-pattern) |
| `XSD-cvc-simple-type.*` | error | schema | a value is not valid for its simple type | [XML Schema 1.0 Part 1 §3.14.4 String Valid](https://www.w3.org/TR/xmlschema-1/#cvc-simple-type) |
| `XSD-cvc-type.*` | error | schema | an element or attribute value does not match its type | [XML Schema 1.0 Part 1 §3.3.4 Element Locally Valid (Type)](https://www.w3.org/TR/xmlschema-1/#cvc-type) |
<!-- END GENERATED: diagnostic codes -->

## Privacy and determinism

`scxml.report/v3` preserves `source_name` and the complete `source_text` so
independent Wasm reporters can produce source snippets and precise SARIF regions
without rerunning validation. The artifact can contain secrets, personal data,
expressions, local paths, and proprietary logic. Store and transmit it only
where the original source is allowed.

Canonical JSON is compact UTF-8 with a fixed struct field order. Producers sort
diagnostics and related collections before encoding. Decoders reject unknown
fields, versions and kinds, inconsistent counts, invalid UTF-8 byte boundaries,
out-of-range spans, and overlapping fix edits.
