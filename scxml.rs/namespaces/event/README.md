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
| `examples/order.scxml` | a machine with a file schema and JSON Pointer, and inline schemas on a `<send>`, a transition, and a `<donedata>` |
| `examples/schemas/order.json` | the schema file the example references |

```sh
scxml-cli validate namespaces/event/examples/order.scxml
scxml-cli compile namespaces/event/examples/order.scxml --output order.scxml-ir.json
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
  only. (`<raise>` creates its event with no data, so its schema describes
  that absence or what a reader should expect; nothing checks it.)

## Metadata only

The attributes never change how a document executes. (An invoked service's
package may declare its own event contract with the same schemas, which the
session host does enforce at the service's boundaries, and against which the
validator checks a document's `event:schema`: `SCXML095`, `SCXML096`; see
[docs/invoked-services.md](../../docs/invoked-services.md#event-contracts).
That changes nothing here: a document's own attributes stay metadata.) Transition selection,
`<send>`, `<raise>`, and `<donedata>` behave exactly as without them: no
event data is checked against a schema, no event is added, and a schema
never raises `error.execution`. The engine copies them only into session
snapshots. The W3C IRP results are unchanged, and
`hosts/scxml-host-wasmtime/tests/session_snapshot.rs`
(`event_schemas_never_change_execution`) proves that a machine with them
runs exactly as the same machine without them.

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
is not a fragment (`#…`) names another resource and is rejected (`SCXML092`).
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
is at most 1 MiB (the Machine IR string bound, `SCXML069`), a pointer
extraction bundles at most 256 referenced values (`SCXML092`), and a
component request carries at most 64 schema files of at most 4 MiB each and
16 MiB together (a request past one is answered with the WIT
`invalid-request` code, with no diagnostic). The native CLI reads files of
any size and bundles any number of values. On every build a pointer
extraction's result is at most 8 times its file plus 4096 bytes and must
nest no deeper than 128 levels, so the Machine IR decoder can read it back
(`SCXML092`).

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
  ([docs/session-snapshot.md](../../docs/session-snapshot.md)); `scxml-cli
  snapshot show` prints `schema sha256:<12 hex>` or `schemaexpr \`…\``. A
  snapshot evaluates no expression, so an `event:schemaexpr` appears
  unevaluated. The snapshot lists only transitions; the schemas of
  `<send>`, `<raise>`, and `<donedata>` are in the Machine IR, whose
  SHA-256 the snapshot names.

## Validator codes

- `SCXML085`: an element in the `event` namespace, an `event` attribute on
  any other element (with a removal fix), or an attribute other than
  `schema` and `schemaexpr` (with a "did you mean" fix).
- `SCXML086`: both `event:schema` and `event:schemaexpr` on one element.
- `SCXML087`: the schema is not UTF-8 JSON (an inline schema at the exact
  byte, a file with its line and column).
- `SCXML088`: a referenced file is not available (not a relative path at
  or below the document's directory, or it could not be read).
- `SCXML089`: the fragment is not a JSON Pointer, or names no value.
- `SCXML090`: the schema violates its dialect's metaschema or does not
  compile (an inline schema at the offending value; a misspelled `type`
  gets a "did you mean" fix).
- `SCXML091`: an unsupported `$schema` dialect.
- `SCXML092`: a reference the compiler cannot keep.

An `event` attribute is never `SCXML068`. `scxml-cli --explain SCXML085`
through `--explain SCXML092` give each rule with an example.

## Statements

The contract above as numbered statements, each tested by the vocabulary's
own implementation report plan ([`irp/`](irp/README.md), below).

- <a id="event-1"></a>**event-1** (placement). `event:schema` and
  `event:schemaexpr` are allowed on `<transition>`, `<send>`, `<raise>`, and
  `<donedata>` only: an `event` attribute on any other element, an
  attribute other than `schema` and `schemaexpr`, or an element in the
  `event` namespace is rejected (`SCXML085`).
- <a id="event-2"></a>**event-2** (one attribute). An element has at most
  one of `event:schema` and `event:schemaexpr` (`SCXML086`).
- <a id="event-3"></a>**event-3** (inline schemas). A value whose first
  non-white-space character is `{`, or that is exactly `true` or `false`,
  is an inline JSON schema; one that is not UTF-8 JSON is rejected
  (`SCXML087`).
- <a id="event-4"></a>**event-4** (schema files). Any other value references
  a file relative to the document, at or below its directory, optionally
  with a JSON Pointer fragment selecting a value of the file: an unavailable
  file, or a path outside the document's directory, is rejected
  (`SCXML088`); a file that is not UTF-8 JSON (`SCXML087`); a fragment that
  is not a JSON Pointer or names no value (`SCXML089`).
- <a id="event-5"></a>**event-5** (dialects). The dialect is the selected
  schema's `$schema`, else the file's, else JSON Schema 2020-12; 2020-12,
  2019-09, and draft-07 are supported, each with or without its trailing
  `#`, and any other `$schema` is rejected (`SCXML091`).
- <a id="event-6"></a>**event-6** (valid schemas). A schema must satisfy
  its dialect's metaschema and compile, so every keyword has a valid value
  and every `pattern` is a valid regular expression (`SCXML090`).
- <a id="event-7"></a>**event-7** (references). Nothing is fetched: a
  `$ref`, `$dynamicRef`, or `$recursiveRef` that is not a fragment names
  another resource and is rejected (`SCXML092`); fragment references are
  kept; with a pointer, references to elsewhere in the file are bundled so
  the selected schema stands alone, and a selected schema that relies on
  `$id`, `$anchor`, `$dynamicAnchor`, `$recursiveAnchor`, plain-name
  fragments, or dynamic references cannot be moved and is rejected
  (`SCXML092`).
- <a id="event-8"></a>**event-8** (unevaluated expressions).
  `event:schemaexpr` is carried unevaluated: the expression is never
  evaluated, so it never raises `error.execution`.
- <a id="event-9"></a>**event-9** (metadata only). The attributes never
  change how a document executes: transition selection, `<send>`,
  `<raise>`, and `<donedata>` behave exactly as without them; no event data
  is checked against a schema, no event is added, and a schema never raises
  `error.execution`.

Not covered by the suite: the Machine IR and session snapshot forms
([Where it shows](#where-it-shows)), which no document observes
(`scxml-compiler-core` and `scxml-host-wasmtime` tests cover them);
`SCXML038` for an `event:schemaexpr` that does not parse, which needs the
CLI's expression syntax checkers (the conformance compiler links none);
and the wasm-only bounds.

## Conformance suite

[`irp/`](irp/README.md) is this vocabulary's implementation report plan in
the W3C IRP format: one assertion per statement above, 29 mandatory test
documents written with the W3C `conf:` vocabulary (17 that must be rejected
at compile time, decided by the suite's document-rejection oracle, and 12
that must run to `conf:pass`), prepared for the ECMAScript and XPath data
models by the W3C templates, and run through the shipped components
exactly as the W3C IRP is
([docs/conformance.md](../../docs/conformance.md#namespace-suites)). The
vocabulary does not depend on the data model; the suite runs both to show
that execution is unchanged under each. Both data models pass 29 of 29
(`irp/baselines/`).

## What is not supported yet

- A reference to a repository-namespace resource or any absolute URI: only
  files relative to the document.
- Schema files for documents compiled without the CLI's file access: a child
  document the Wasmtime host compiles when it is invoked, and every document
  the browser host compiles, get no schema files, so they may use inline
  schemas and `event:schemaexpr` only (a reference rejects the document).
- Evaluating `event:schemaexpr`: a session snapshot evaluates no expression
  (docs/session-snapshot.md), so readers get the expression.

## Why the URI has no version

As for [`namespaces/env`](../env/README.md#why-the-uri-has-no-version): the
host defines the vocabulary, nothing is loaded from it, and a change to it is
a change to the host contract, recorded in [CHANGELOG.md](../../CHANGELOG.md).
