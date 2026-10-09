# `event` attribute vocabulary

`https://github.com/stateforward/scxml.rs/namespaces/event` is the host's
attribute vocabulary for describing event data with JSON Schema: which
`_event.data` a transition handles, which data a `<send>` or `<raise>`
creates its event with, and which data the `done.state.<id>` event of a
`<donedata>` carries. It is not an extension package. The namespace has
attributes only: no elements, no executable content, no extension component,
no package manifest, and no release asset. It is never fetched; the toolchain
recognises the URI byte for byte (`scxml_types::EVENT_NAMESPACE_URI`). This
folder exists so the URI names a real path in the repository, like every
other `namespaces/<name>` URI.

| Path | What |
|---|---|
| `package/event.xsd` | the schema, for readers and tools that resolve a namespace to a schema (it declares the two attributes as strings; the validator does not load it) |
| `examples/order.scx` | a machine with a file schema and JSON Pointer, and inline schemas on a `<send>`, a transition, and a `<donedata>` |
| `examples/schemas/order.json` | the schema file the example references |

```sh
scx validate namespaces/event/examples/order.scx
scx compile namespaces/event/examples/order.scx --output order.scxml-ir.json
```

## The attributes

```xml
<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event"
       version="1.0" datamodel="ecmascript">
  …
  <transition event="order.placed" target="checking"
              event:schema="schemas/order.json#/$defs/Order"/>
  <transition event="order.checked" target="shipped"
              event:schema='{"type": "object", "required": ["count"]}'/>
  <transition event="order.refunded" target="refunded"
              event:schemaexpr="schemas.refund"/>
```

- `event:schema` is a JSON Schema. Its value is either
  - **inline JSON**: the first non-white-space character is `{`, or the value
    is exactly `true` or `false` (the boolean schemas). In a double-quoted
    XML attribute write `"` as `&quot;`, or quote the attribute with `'`; or
  - **a reference**: a path relative to the document, at or below its
    directory (no scheme, absolute path, query, backslash, or empty, `.`, or
    `..` segment; percent-encoded like any URI path), optionally with a JSON
    Pointer fragment (RFC 6901, percent-encoded as a URI fragment):
    `schemas/order.json#/$defs/Order` selects member `Order` of member
    `$defs` of the file. A plain-name fragment (`#Order`) is not supported.
- `event:schemaexpr` is an expression in the document's data model whose
  value is the schema. It is syntax-checked like every other `*expr`
  attribute (`SCXML038` when it does not parse) and carried unevaluated.
- An element has at most one of the two (SCXML's attribute/`expr`
  convention).
- They are allowed on `<transition>`, `<send>`, `<raise>`, and `<donedata>`
  only. (`<raise>` creates its event with no data, which a schema sees as
  `null`; see [At run time](#at-run-time).)

## At run time

A literal `event:schema` is a contract on event data, which the session
creating an event keeps (`crates/scxml-engine/src/contracts.rs`):

- An executed `<send>` through the SCXML Event I/O Processor checks the
  data it created (`_event.data` as the receiver would read it) against its
  own literal schema, whatever its target, and, when the session delivers
  the event to itself (no `target`, `#_internal`, or its own
  `#_scxml_<sessionid>`), against the literal schema of every transition of
  the document whose event descriptor matches the event's name (SCXML
  3.12.1), in any state. Data one of them rejects is not dispatched:
  `error.execution` (with the send's `_event.sendid`) ends the block, as
  for any other `<send>` argument error (SCXML 6.2: "If the evaluation of
  `<send>`'s arguments produces an error, the Processor MUST discard the
  message without attempting to deliver it").
- An executed `<raise>` checks its event's data, which is none (`null`),
  against its own literal schema and those of the transitions handling its
  event the same way; a rejected event is not raised, and
  `error.execution` ends the block.
- The data a schema sees is the event's data as JSON: no data, `undefined`,
  and `null` are `null`; booleans, numbers (an integral one is an
  integer), strings, arrays, and objects are themselves. XML data (the
  XPath data model's) has no JSON instance, so only the schema `true`
  admits it.

Nothing else changes: transitions match events by name exactly as SCXML
specifies (a transition's schema is the contract the document declares for
the events it handles, checked where they are created, never during
selection); `<donedata>`, the `done.state.<id>` and `done.invoke.<id>`
events, a `BasicHTTP` `<send>`, and `event:schemaexpr` are never checked;
and no event is added. A schema the engine cannot compile (only Machine IR
no canonical compiler produced) admits nothing. Each schema is compiled
once per session, on first use. The W3C IRP results are unchanged, and
`hosts/scxml-host-wasmtime/tests/session_snapshot.rs`
(`event_schemas_never_change_execution`) proves that a machine whose
events satisfy their schemas runs exactly as the same machine without
them. The schemas of the events an invoked service sends live here too,
on the transitions that handle them; a service package declares no
events; see
[docs/invoked-services.md](../../docs/invoked-services.md#event-schemas).

Not yet checked (see [What is not supported yet](#what-is-not-supported-yet)):
an event a session sends another session (`#_parent`, `#_<invokeid>`)
against the receiving document's transitions, and an event the host
delivers (`scx run` standard input) against the root document's.

## Compile time

`validate` and `compile` (and `snapshot run` and `run`, which compile first)
read every referenced file relative to the document before validating; the
validator and compiler themselves perform no I/O, so the validator and
compiler components take the files' bytes in their requests
(`event-schema-resource`). For each `event:schema` the compiler:

1. parses the UTF-8 JSON (a leading byte-order mark is ignored);
2. applies the JSON Pointer, when there is one;
3. takes the dialect from the selected schema's `$schema`, else the file's,
   else JSON Schema 2020-12. Supported: 2020-12
   (`https://json-schema.org/draft/2020-12/schema`), 2019-09
   (`https://json-schema.org/draft/2019-09/schema`), and draft-07
   (`http://json-schema.org/draft-07/schema#`), each with or without its
   trailing `#`; any other `$schema` is an error;
4. handles references (below);
5. checks the result against its dialect's metaschema and compiles it once,
   so every keyword has a valid value, every reference it keeps resolves,
   and every `pattern` is a valid regular expression;
6. embeds it in the Machine IR on the owning transition, `<send>`,
   `<raise>`, or `<donedata>` as canonical JSON (no white space, object
   members sorted by the UTF-8 bytes of their keys, numbers and strings as
   `serde_json` writes them) with the SHA-256 of that text.

`event:schemaexpr` is embedded as its expression. An element without either
attribute keeps exactly the Machine IR bytes it had.

### References

Nothing is fetched. A `$ref`, `$dynamicRef`, or `$recursiveRef` whose value
is not a fragment (`#…`) names another resource and is rejected (`EVENT008`).
Without a pointer the file's schema is embedded whole, so every fragment
reference keeps its meaning. With a pointer, a JSON Pointer `$ref` into the
selected schema is rebased onto it (`#/$defs/Order/properties/x` becomes
`#/properties/x`), and one to anywhere else in the file is bundled: the
referenced value is copied into the result's `$defs` (`definitions` for
draft-07) under its original pointer as the key, and the reference is
rewritten to it (`#/$defs/Item` becomes `#/$defs/~1$defs~1Item`), so the
embedded schema stands alone. A selected schema that relies on `$id`,
`$anchor`, `$dynamicAnchor`, `$recursiveAnchor`, plain-name fragments, or
dynamic references cannot be moved and is rejected; reference the whole file
instead. A pointer extraction's bounds are listed under [Bounds](#bounds).

### Bounds

On wasm builds (the shipped components) one schema, as read and as embedded,
is at most 1 MiB (the Machine IR string bound, `LIMIT005`), a pointer
extraction bundles at most 256 referenced values (`EVENT008`), and a
component request carries at most 64 schema files of at most 4 MiB each and
16 MiB together (a request past one is answered with the WIT
`invalid-request` code, with no diagnostic). The native CLI reads files of
any size and bundles any number of values. On every build a pointer
extraction's result is at most 8 times its file plus 4096 bytes and must
nest no deeper than 128 levels, so the Machine IR decoder can read it back
(`EVENT008`).

Numbers are read as JSON numbers in the range of a 64-bit integer or a
64-bit float: an integer outside the 64-bit range (for example a `maximum`
of `18446744073709551616`) is read, compared, and embedded as the nearest
64-bit float, without a diagnostic.

## Where it shows

- **Machine IR** (`event_schema` on a transition, a `raise` or `send`
  action, and a `donedata`): `{"kind":"schema","schema":"<canonical JSON>","sha256":"<64 hex>"}`
  or `{"kind":"schemaexpr","expr":"<expression>"}`; decoding rejects a
  digest that does not match its schema or a schema that is not canonical.
- **Session snapshot** (`scxml.session-snapshot/v2`): each listed transition
  carries the same `event_schema` object
  ([docs/session-snapshot.md](../../docs/session-snapshot.md)); `scx
  snapshot show` prints `schema sha256:<12 hex>` or `schemaexpr \`…\``. A
  snapshot evaluates no expression, so an `event:schemaexpr` appears
  unevaluated. The snapshot lists only transitions; the schemas of
  `<send>`, `<raise>`, and `<donedata>` are in the Machine IR, whose
  SHA-256 the snapshot names.

## Validator codes

- `EVENT001`: an element in the `event` namespace, an `event` attribute on
  any other element (with a removal fix), or an attribute other than
  `schema` and `schemaexpr` (with a "did you mean" fix).
- `EVENT002`: both `event:schema` and `event:schemaexpr` on one element.
- `EVENT003`: the schema is not UTF-8 JSON (an inline schema at the exact
  byte, a file with its line and column).
- `EVENT004`: a referenced file is not available (not a relative path at
  or below the document's directory, or it could not be read).
- `EVENT005`: the fragment is not a JSON Pointer, or names no value.
- `EVENT006`: the schema violates its dialect's metaschema or does not
  compile (an inline schema at the offending value; a misspelled `type`
  gets a "did you mean" fix).
- `EVENT007`: an unsupported `$schema` dialect.
- `EVENT008`: a reference the compiler cannot keep.
- `EVENT009`: the literal data of a `<send>` or `<donedata>` does not
  satisfy its own literal schema.
- `EVENT010` (warning): a `<send>` or `<donedata>` creates data a
  transition of the same document handling its event does not accept.
- `EVENT011` (warning): a `<send>` between a document and a literal
  `<invoke src>` document it invokes (`#_<id>` one way, `#_parent` the
  other) creates data a transition of the receiving document handling its
  event does not accept, judged when the caller supplies the invoked
  documents (`scx validate`, `compile`, and `run` read the `file:` ones).

An `event` attribute is never `LINT022`. `scx --explain EVENT001`
through `--explain EVENT011` give each rule with an example.

## Statements

The contract above as numbered statements, each tested by the vocabulary's
own implementation report plan ([`irp/`](irp/README.md), below).

- <a id="event-1"></a>**event-1** (placement). `event:schema` and
  `event:schemaexpr` are allowed on `<transition>`, `<send>`, `<raise>`, and
  `<donedata>` only: an `event` attribute on any other element, an
  attribute other than `schema` and `schemaexpr`, or an element in the
  `event` namespace is rejected (`EVENT001`).
- <a id="event-2"></a>**event-2** (one attribute). An element has at most
  one of `event:schema` and `event:schemaexpr` (`EVENT002`).
- <a id="event-3"></a>**event-3** (inline schemas). A value whose first
  non-white-space character is `{`, or that is exactly `true` or `false`,
  is an inline JSON schema; one that is not UTF-8 JSON is rejected
  (`EVENT003`).
- <a id="event-4"></a>**event-4** (schema files). Any other value references
  a file relative to the document, at or below its directory, optionally
  with a JSON Pointer fragment selecting a value of the file: an unavailable
  file, or a path outside the document's directory, is rejected
  (`EVENT004`); a file that is not UTF-8 JSON (`EVENT003`); a fragment that
  is not a JSON Pointer or names no value (`EVENT005`).
- <a id="event-5"></a>**event-5** (dialects). The dialect is the selected
  schema's `$schema`, else the file's, else JSON Schema 2020-12; 2020-12,
  2019-09, and draft-07 are supported, each with or without its trailing
  `#`, and any other `$schema` is rejected (`EVENT007`).
- <a id="event-6"></a>**event-6** (valid schemas). A schema must satisfy
  its dialect's metaschema and compile, so every keyword has a valid value
  and every `pattern` is a valid regular expression (`EVENT006`).
- <a id="event-7"></a>**event-7** (references). Nothing is fetched: a
  `$ref`, `$dynamicRef`, or `$recursiveRef` that is not a fragment names
  another resource and is rejected (`EVENT008`); fragment references are
  kept; with a pointer, references to elsewhere in the file are bundled so
  the selected schema stands alone, and a selected schema that relies on
  `$id`, `$anchor`, `$dynamicAnchor`, `$recursiveAnchor`, plain-name
  fragments, or dynamic references cannot be moved and is rejected
  (`EVENT008`).
- <a id="event-8"></a>**event-8** (unevaluated expressions).
  `event:schemaexpr` is carried unevaluated: the expression is never
  evaluated, so it never raises `error.execution`.
- <a id="event-9"></a>**event-9** (send and raise contracts). A `<send>`
  through the SCXML Event I/O Processor whose data its own literal
  `event:schema` rejects, or, for an event the session delivers to itself
  (no target, `#_internal`, or its own session address), a literal
  `event:schema` of a transition of the document whose descriptor matches
  the event, is not dispatched, and `error.execution` ends its block; a
  `<raise>` such a schema rejects (its event has no data) is not raised,
  and `error.execution` ends its block. Transition selection and
  `<donedata>` are unchanged: no schema is checked when an event is
  selected or a done event is generated.

Not covered by the suite: the Machine IR and session snapshot forms
([Where it shows](#where-it-shows)), which no document observes
(`scxml-compiler-core` and `scxml-host-wasmtime` tests cover them);
`SCXML038` for an `event:schemaexpr` that does not parse, which needs the
CLI's expression syntax checkers (the conformance compiler links none);
the wasm-only bounds; the `_event.sendid` of a rejected `<send>`'s error
and data other than `null` (both data models must run every document,
and the XPath data model's data is XML, which only `true` admits), which
`crates/scxml-engine/tests/interpret.rs` covers.

## Conformance suite

[`irp/`](irp/README.md) is this vocabulary's implementation report plan in
the W3C IRP format: one assertion per statement above, 30 mandatory test
documents written with the W3C `conf:` vocabulary (17 that must be rejected
at compile time, decided by the suite's document-rejection oracle, and 13
that must run to `conf:pass`), prepared for the ECMAScript and XPath data
models by the W3C templates, and run through the shipped components
exactly as the W3C IRP is
([docs/conformance.md](../../docs/conformance.md#namespace-suites)). The
vocabulary does not depend on the data model; the suite runs both to show
that it behaves the same under each. Both data models pass 30 of 30
(`irp/baselines/`).

## What is not supported yet

- Checking an event one session sends another (`#_parent`,
  `#_<invokeid>`) against the receiving document's transitions, and an
  event the host delivers (`scx run` standard input, an embedder's `send`)
  against the root document's: the sending engine does not have the
  receiving document, and a host must not judge events itself. Such
  events are delivered whatever their data; `EVENT011` reports the ones
  whose literal data provably fails, for literal `<invoke src>` documents.

- A reference to a repository-namespace resource or any absolute URI: only
  files relative to the document.
- Schema files for documents compiled without the CLI's file access. A
  literal `<invoke src>` child loaded from a `file:` URL gets its own files:
  `snapshot run` and `run` read them relative to the child's directory
  before the session and the Wasmtime host hands them to its compile
  (`EmbeddedDocument::event_schemas`). Every other child document a host
  compiles when it is invoked (inline `<content>`, a remote or `srcexpr`
  `<invoke src>`, and every child the browser host compiles) gets no schema
  files, so it may use inline schemas and `event:schemaexpr` only (a
  reference rejects the document). A browser page supplies a document's
  schema files to the validator and compiler (`eventSchemas`).
- Evaluating `event:schemaexpr`: a session snapshot evaluates no expression
  (docs/session-snapshot.md), so readers get the expression.

## Why the URI has no version

As for [`namespaces/env`](../env/README.md#why-the-uri-has-no-version): the
host defines the vocabulary, nothing is loaded from it, and a change to it is
a change to the host contract, recorded in [CHANGELOG.md](../../CHANGELOG.md).
