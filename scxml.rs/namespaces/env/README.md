# `env` attribute vocabulary

`https://github.com/stateforward/scxml.rs/namespaces/env` is the host's
attribute vocabulary for the slots of invoked services: an invoked service's
`<invoke>` names, with an `env` attribute, the variable whose value the host
binds to one of the slots its service package declares. It is not an
extension package. The namespace has attributes only: no elements, no
executable content, no extension component, no package manifest, and no
release asset. It is never fetched; the host recognises the URI byte for
byte (`scxml_types::ENV_NAMESPACE_URI`). This folder exists so the URI names
a real path in the repository, like every other `namespaces/<name>` URI; see
[docs/invoked-services.md](../../docs/invoked-services.md#values-are-references-env-slots)
for the full contract.

| Path | What |
|---|---|
| `package/env.xsd` | the schema, for readers and tools that resolve a namespace to a schema (it declares no elements and no attributes; see below) |

## The attributes

```xml
<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env"
       version="1.0" datamodel="ecmascript">
  …
  <invoke type="https://stateforward.org/scxml/services/echo" id="echo"
          env:token="API_TOKEN" env:endpointexpr="'API_BASE_URL'"/>
```

- `env:<slot>="NAME"` binds the slot `<slot>` to the variable `NAME`. A name
  is `[A-Za-z_][A-Za-z0-9_]*`, at most 256 bytes. Names are not secrets;
  values never appear in a document.
- `env:<slot>expr="EXPR"` binds the slot to the variable whose name `EXPR`
  evaluates to, in the document's data model, when the `<invoke>` runs
  (SCXML's attribute/`expr` convention). A result that is not a string
  naming a variable raises `error.execution`, and the service never starts.
- The slots an `<invoke>` has are the ones its service package declares
  (`env-slots`: `[a-z][a-z0-9-]*`, at most 32). The echo service
  ([`components/scxml-service-example`](../../components/scxml-service-example))
  declares `endpoint` and `token`.
- Only an invoked service's `<invoke>` has slots. An executable-content
  extension element has none: it runs synchronously inside a microstep and
  does no I/O, so it never needs a value.

The validator checks every `env` attribute:

- `SCXML083`: a slot the service's package does not declare (with a "did you
  mean" fix for a near miss; judged when the validator is given the
  package), one slot bound twice (`env:<slot>` and `env:<slot>expr`), an
  `env:<slot>` value that is not a variable name, or an attribute that names
  no valid slot.
- `SCXML084`: an element in the `env` namespace, or an `env` attribute on an
  element in no namespace, on an executable-content extension element, or on
  the `<invoke>` of an SCXML session.
- `SCXML068`: an `env` attribute on any other SCXML element (an attribute in
  another namespace on an SCXML element).

`scx explain SCXML083` and `scx explain SCXML084` give each rule
with an example.

## Values, references, and masking

- **The run holds the values.** An embedder supplies them
  (`ServiceComponents::with_values`), each variable granted to exact service
  type URIs; a slot naming a variable not granted to its `<invoke>`'s type
  is absent, exactly as if no value were held. The CLI derives the grants
  from the document: a variable goes to the type of each `<invoke>` whose
  `env` attributes name it, and to the type of every `env:<slot>expr` slot.
  It holds the value of `--env NAME=VALUE`, else the last `--env-file PATH`
  entry (Docker's format: `NAME=VALUE` taken literally, a bare `NAME` from
  the shell, `#` comments and blank lines ignored), else, for a bare
  `--env NAME`, the calling shell's; a variable no slot can name is dropped.
  Nothing is held by default, and there is no whole-environment form.
- **The component holds references.** When an invocation starts, the host
  binds every slot its `<invoke>` names, whose variable the run holds, to a
  fresh opaque `value-reference` (`invoke-request.references`), valid for the
  whole invocation and only inside its own `outbound-http` requests, which
  the host admits only to an origin the package declares or one a
  reference's value supplies entirely. No other invocation ever receives
  them.
- **A missing value leaves the slot absent.** The service decides what that
  means (the echo service sends no `authorization` without a `token`, and
  fails its `fetch` without an `endpoint`). Before a run, the CLI lists each
  variable the document names and the `<invoke>`s it goes to, and says when
  the run holds no value for it.
- **Values are masked.** As defense in depth (a component never receives a
  value, so one reaches output only if a document or a service echoes it),
  every value the run holds of at least 4 bytes, and its JSON-escaped form,
  is written as `[masked]` in the text the CLI writes: live-session records,
  notes, diagnostics, and error messages. Canonical snapshot documents are
  written exactly, since their `sha256` covers their bytes.

## Statements

The contract above as numbered statements. env-1 and env-2 are decided by
a document alone and are tested by the vocabulary's implementation report
plan ([`irp/`](irp/README.md), below); the others are about what a run does
with values, which the conformance runner cannot observe (it binds no
service yet), and are proven by the echo service's tests named under each.

- <a id="env-1"></a>**env-1** (declared slots). An `<invoke>`'s slots are
  the ones its service package declares (`env-slots`): a slot bound twice
  (`env:<slot>` and `env:<slot>expr`), an `env:<slot>` value that is not a
  variable name, or an attribute that names no valid slot is rejected
  (`SCXML083`), and so is a slot the package does not declare, when the
  validator is given the package (IRP, and
  `crates/scxml-validator-core/src/env_attributes.rs`); at run time an
  undeclared slot rejects the start (`service_invoke.rs`,
  `an_unbound_out_of_scope_or_undeclared_env_slot_never_reaches_the_network`).
- <a id="env-2"></a>**env-2** (placement). The vocabulary has attributes
  only, and only an invoked service's `<invoke>` has slots: an element in
  the `env` namespace, or an `env` attribute on an element in no namespace,
  on an executable-content extension element, or on the `<invoke>` of an
  SCXML session, is rejected (`SCXML084`), and an `env` attribute on another
  SCXML element is rejected (`SCXML068`) (IRP).
- <a id="env-3"></a>**env-3** (binding by name). `env:<slot>="NAME"` binds
  the `<invoke>`'s slot `<slot>` to the variable `NAME`; the host substitutes
  `NAME`'s value (`hosts/scxml-host-wasmtime/tests/service_invoke.rs`,
  `an_env_slot_reference_lives_for_the_whole_invocation`;
  `crates/scxml-cli/tests/env_values.rs`).
- <a id="env-4"></a>**env-4** (binding by expression).
  `env:<slot>expr="EXPR"` binds the slot to the variable whose name `EXPR`
  evaluates to in the document's data model when the `<invoke>` runs; a
  value that is not a string naming a variable, or a failed evaluation,
  raises `error.execution` (`service_invoke.rs`,
  `an_env_slot_reference_lives_for_the_whole_invocation`).
- <a id="env-5"></a>**env-5** (missing values). A slot whose variable the
  run holds no value for, or holds only for another service, is absent: the
  service decides what that means (`service_invoke.rs`,
  `an_unbound_out_of_scope_or_undeclared_env_slot_never_reaches_the_network`;
  `env_values.rs`, `a_variable_goes_only_to_the_invokes_naming_it`).
- <a id="env-6"></a>**env-6** (references are per invocation). An
  invocation's references are its own, valid for its whole lifetime: no
  other invocation receives them (`service_invoke.rs`,
  `an_env_slot_reference_lives_for_the_whole_invocation`).
- <a id="env-7"></a>**env-7** (admitted requests). The host substitutes a
  value only into the invocation's requests admitted to an origin its
  package declares or one a value supplies entirely; every other request is
  refused before it is sent (`service_invoke.rs`,
  `a_token_and_a_path_reach_only_a_request_whose_origin_a_value_supplies`;
  `env_values.rs`, `values_reach_only_admitted_origins`).

## Conformance suite

[`irp/`](irp/README.md) is this vocabulary's implementation report plan in
the W3C IRP format: env-1 and env-2, tested by 8 mandatory test documents
written with the W3C `conf:` vocabulary, each of which the canonical
compiler must reject with exactly the listed code (the document-rejection
oracle), prepared for the ECMAScript and XPath data models by the W3C
templates and run through the shipped components exactly as the W3C IRP is,
binding the example extension package for env-2's extension element
([docs/conformance.md](../../docs/conformance.md#namespace-suites)). Both
data models pass 8 of 8 (`irp/baselines/`).

## Schema

`package/env.xsd` has the `env` target namespace and declares only the
`variable-name` and `value-expression` value types. XSD cannot enumerate an
open set of attribute names, and the slots differ per package, so slot
checking stays in the validator, driven by each service's manifest; the
validator does not load this file. SCXML's own schema admits attributes in
other namespaces on `<invoke>` laxly.

## Why the URI has no version

A repository namespace's URI carries its release tag
(`namespaces/<name>@v0.2.0`) because a document loads that release's component
and schema from it. The `env` vocabulary has nothing to load: the host defines
it, as the SCXML namespace `http://www.w3.org/2005/07/scxml` is defined by the
processor rather than fetched. A version suffix would name a release asset
that does not exist and would make every document that binds a slot change
its namespace on each release. A change to the vocabulary is a change to the
host contract, recorded in [CHANGELOG.md](../../CHANGELOG.md).
