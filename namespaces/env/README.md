# `env` attribute vocabulary

`https://github.com/stateforward/scxml.rs/namespaces/env` is the host's
attribute vocabulary for the slots of executable-content extension elements
and invoked services: an extension element, or an invoked service's
`<invoke>`, names, with an `env` attribute, the variable whose value the host
binds to one of the slots its package declares. It is not an extension
package. The namespace has attributes only: no elements, no executable
content, no extension component, no package manifest, and no release asset.
It is never fetched; the host recognises the URI byte for byte
(`scxml_types::ENV_NAMESPACE_URI`). This folder exists so the URI names a real
path in the repository, like every other `namespaces/<name>` URI; see
[docs/executable-content-extensions.md](../../docs/executable-content-extensions.md#values-are-references-env-slots)
for the full contract.

| Path | What |
|---|---|
| `package/env.xsd` | the schema, for readers and tools that resolve a namespace to a schema (it declares no elements and no attributes; see below) |

## The attributes

```xml
<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:example="https://stateforward.org/scxml/example"
       xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env"
       version="1.0" datamodel="ecmascript">
  …
  <example:fetch env:token="API_TOKEN" env:urlexpr="'API_BASE_URL'" path="/items"/>
```

- `env:<slot>="NAME"` binds the slot `<slot>` to the variable `NAME`. A name
  is `[A-Za-z_][A-Za-z0-9_]*`, at most 256 bytes. Names are not secrets;
  values never appear in a document.
- `env:<slot>expr="EXPR"` binds the slot to the variable whose name `EXPR`
  evaluates to, in the document's data model, each time the element executes
  (SCXML's attribute/`expr` convention). A result that is not a string naming
  a variable raises `error.execution`.
- The slots an element has are the ones its package manifest declares
  (`env-slots`: `[a-z][a-z0-9-]*`, at most 32). The example extension
  ([`components/scxml-extension-example`](../../components/scxml-extension-example))
  declares `url` and `token`.
- An invoked service's `<invoke>` binds the slots its service package
  declares the same way (`<invoke type="URI" env:endpoint="ECHO_URL"/>`); an
  `env:<slot>expr` there is evaluated once, when the `<invoke>` runs
  ([docs/invoked-services.md](../../docs/invoked-services.md#values-are-references-env-slots)).

The validator checks every `env` attribute against the element's package:

- `SCXML083`: a slot the package does not declare (with a "did you mean" fix
  for a near miss), one slot bound twice (`env:<slot>` and
  `env:<slot>expr`), or an `env:<slot>` value that is not a variable name.
- `SCXML084`: an element in the `env` namespace, or an `env` attribute on an
  element in no namespace or on the `<invoke>` of an SCXML session (only
  extension elements and invoked services have slots).
- `SCXML068`: an `env` attribute on any other SCXML element (an attribute in
  another namespace on an SCXML element).

`scx explain SCXML083` and `scx explain SCXML084` give each rule
with an example.

## Values, references, and masking

- **The run holds the values.** An embedder supplies them
  (`ExtensionComponents::with_values`), each variable granted to exact package
  URIs; a slot naming a variable not granted to its element's namespace (or
  its `<invoke>`'s service type URI) is absent, exactly as if no value were
  held. The CLI derives the grants from the document: a variable goes to
  the packages of the elements and `<invoke>`s whose `env` attributes name
  it, and to the package of every `env:<slot>expr` slot. It holds the value
  of `--env NAME=VALUE`, else the last `--env-file PATH` entry (Docker's
  format: `NAME=VALUE` taken literally, a bare `NAME` from the shell, `#`
  comments and blank lines ignored), else, for a bare `--env NAME`, the
  calling shell's; a variable no slot can name is dropped. Nothing is held
  by default, and there is no whole-environment form.
- **The component holds references.** For each `execute` call the host binds
  every slot the element names, whose variable the run holds, to a fresh
  opaque `value-reference`. A reference is valid only during that call and
  only for that element; another element, of the same or another namespace,
  never receives it. The host substitutes the value only into that call's
  `outbound-http` requests, admitted only to an origin the package declares
  or one a reference's value supplies entirely. An invoked service's
  references are bound once, when its invocation starts
  (`invoke-request.references`), and stay valid for the whole invocation,
  only inside that invocation's admitted `outbound-http` requests.
- **A missing value leaves the slot absent.** The component decides what
  that means (`<example:fetch>` sends no `authorization` without a token). Before a
  run, the CLI lists each element's bound slots and names the `--env` flag
  that would supply a literal name it does not hold.
- **Values are masked.** As defense in depth (a component never receives a
  value, so one reaches output only if a document or a service echoes it),
  every value the run holds of at least 4 bytes, and its JSON-escaped form,
  is written as `[masked]` in the text the CLI writes: live-session records,
  notes, diagnostics, and error messages. Canonical snapshot documents are
  written exactly, since their `sha256` covers their bytes.

## Statements

The contract above as numbered statements, each tested by the vocabulary's
own implementation report plan ([`irp/`](irp/README.md), below), through
the example extension's `<example:fetch>`
([`components/scxml-extension-example`](../../components/scxml-extension-example),
whose package declares the `url` and `token` slots).

- <a id="env-1"></a>**env-1** (binding by name). `env:<slot>="NAME"` binds
  the element's slot `<slot>` to the variable `NAME`; the host substitutes
  `NAME`'s value.
- <a id="env-2"></a>**env-2** (binding by expression).
  `env:<slot>expr="EXPR"` binds the slot to the variable whose name `EXPR`
  evaluates to in the document's data model when the element executes; a
  value that is not a string naming a variable, or a failed evaluation,
  raises `error.execution`.
- <a id="env-3"></a>**env-3** (declared slots). An element's slots are the
  ones its package declares (`env-slots`): a slot the package does not
  declare, a slot bound twice (`env:<slot>` and `env:<slot>expr`), or an
  `env:<slot>` value that is not a variable name is rejected (`SCXML083`).
- <a id="env-4"></a>**env-4** (placement). The vocabulary has attributes
  only: an element in the `env` namespace, or an `env` attribute on an
  element in no namespace, is rejected (`SCXML084`), and an `env` attribute
  on an SCXML element is rejected (`SCXML068`).
- <a id="env-5"></a>**env-5** (missing values). A slot whose variable the
  run holds no value for is absent: the element's component decides what
  that means (`<example:fetch>` sends no `authorization` without a `token`,
  and without a `url` slot it uses its `url` attribute, which only an origin
  its package declares could admit).
- <a id="env-6"></a>**env-6** (references are per element). Each execution
  binds the element's own slots: another element, of the same or another
  namespace, never receives them.
- <a id="env-7"></a>**env-7** (admitted requests). The host substitutes a
  value only into the element's requests admitted to an origin its package
  declares or one a value supplies entirely; every other request is refused
  before it is sent (the element raises `error.execution`).

Not covered by the suite: masking, which applies to the text the CLI
writes and is never observable by a document (`scx` tests cover it),
and that an expression is evaluated anew at each execution (a portable
test would need an expression that reads data-model state, which the W3C
templates write differently for each data model).

## Conformance suite

[`irp/`](irp/README.md) is this vocabulary's implementation report plan in
the W3C IRP format: one assertion per statement above, 20 mandatory test
documents written with the W3C `conf:` vocabulary, prepared for the
ECMAScript and XPath data models by the W3C templates, and run through the
shipped components exactly as the W3C IRP is, binding the example extension
package ([docs/conformance.md](../../docs/conformance.md#namespace-suites)).
The vocabulary itself does not depend on the data model; the suite runs
both because `env:<slot>expr` evaluates in the document's. Both data models
pass 20 of 20 (`irp/baselines/`).

## Schema

An extension schema admits the vocabulary on each of its elements laxly:

```xml
<xs:anyAttribute namespace="https://github.com/stateforward/scxml.rs/namespaces/env"
                 processContents="lax"/>
```

(or `##other`). `package/env.xsd` has the `env` target namespace and
declares only the `variable-name` and `value-expression` value types. XSD
cannot enumerate an open set of attribute names, and the slots differ per
package, so slot checking stays in the validator, driven by each element's
manifest; the validator does not load this file.

## Why the URI has no version

A repository namespace's URI carries its release tag
(`namespaces/<name>@v0.2.0`) because a document loads that release's component
and schema from it. The `env` vocabulary has nothing to load: the host defines
it, as the SCXML namespace `http://www.w3.org/2005/07/scxml` is defined by the
processor rather than fetched. A version suffix would name a release asset
that does not exist and would make every document that binds a slot change
its namespace on each release. A change to the vocabulary is a change to the
host contract, recorded in [CHANGELOG.md](../../CHANGELOG.md).
