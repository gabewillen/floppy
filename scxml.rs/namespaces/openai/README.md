# `openai` executable-content extension package

`<openai:generate>` (namespace `https://github.com/stateforward/scxml.rs/namespaces/openai@v0.2.0`,
a repository namespace: a document that binds it loads it from this
repository's `v0.2.0` release) as a
`stateforward:scxml/executable-content-extension` Wasm component: it asks an
OpenAI-compatible Responses API (`POST {base}/responses`) for one reply, with
the executing session's read-only snapshot as the system message and the events
the session accepts as function tools, then sends the events the model called
and stores the reply's text at a data-model location. It is a rewrite
modelled on the `openai` namespace of
[agentml-go](https://github.com/agentflare-ai/agentml.go) (MIT;
`openai/namespace.go` at commit 5d305e7); see
[docs/executable-content-extensions.md](../../docs/executable-content-extensions.md)
for the extension component contract.

| Path | What |
|---|---|
| `src/lib.rs` | decision logic behind the `OpenAiHost` trait (natively tested) and the guest glue |
| `package/openai.xsd` | the schema (structure only) |
| `package/openai.extension.json` | the package manifest (`scxml_extension_openai.virtualized.wasm`) |
| `examples/hello.scxml` | content generation: the reply is stored at `location` |
| `examples/decide.scxml` | function calling: the model's chosen event moves the machine |
| `run-example.sh` | runs an example through `scxml-cli snapshot run` |

## The element

```xml
<openai:generate env:apikey="OPENAI_API_KEY" env:baseurl="OPENAI_BASE_URL"
                 model="gpt-4o-mini" location="reply"
                 prompt="Be brief." promptexpr="'Topic: ' + topic"
                 max-output-tokens="64" reasoning="low">
  <openai:prompt>Say hello.</openai:prompt>
</openai:generate>
```

(`xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env"`, the host's
[`env` vocabulary](../env/README.md).)

- `env:apikey="NAME"` (required at run time) and `env:baseurl="NAME"`
  (optional), or `env:apikeyexpr`/`env:baseurlexpr`: the variables whose
  values the host binds to the element's two slots. The component receives
  opaque references, never the values; the host substitutes them only into
  the request (`Authorization: Bearer <apikey>`, `<baseurl>/responses`).
  Without `baseurl` the request goes to `https://api.openai.com/v1/responses`;
  a base URL must not end with `/`.

- `model` or `modelexpr` (required; `modelexpr` wins and must evaluate to a
  non-empty string).
- The prompt is the `prompt` attribute (literal), then `promptexpr`'s string
  value, then each `<openai:prompt>` child's trimmed text in document order;
  non-empty parts are joined with a newline, in agentml-go's order. At least one
  part is required. It is sent as one `user` message.
- `location` (optional): the reply text is assigned there as a string.
  Without it the element is in function-calling mode (below).
- `max-output-tokens` (positive integer) and `reasoning` (sent as
  `reasoning.effort`) are optional.
- The reply is the first `output_text` of the first assistant `message` in
  the result's `output`, as agentml extracts it.

## The snapshot, tools, and function calling

Like agentml-go, every request carries what the session is doing and what
it can do next, through the host's read-only `session-snapshot-host`
([docs/session-snapshot.md](../../docs/session-snapshot.md), "Snapshots during
executable content"):

- **System message**: the executing session's canonical
  `scxml.session-snapshot/v2` document with `data_model` and `transitions`
  excluded (agentml-go's `SnapshotConfig{ExcludeData: true}` and
  `PruneSnapshot`). It shows the configuration the element's effect
  observes, the system variables, the queues, and the pending sends; data
  values never leave the session.
- **Tools**: from a second document that keeps only `transitions`, one
  `send_<event>` function per event descriptor of the transitions the
  active configuration can take, in the order of the transitions walk (agentml-go's
  `BuildSendFunctions` and `sanitizeFunctionName`: `.` becomes `_`, any
  byte outside `[A-Za-z0-9_-]` becomes `_`; the name maps back to the
  original event name). `foo.*` and `foo.` name `foo` (SCXML 3.12.1); `*`
  names no event and gets no function; a name over 64 bytes or one an
  earlier event already took gets none. Each tool has the optional string
  parameters `target` and `delay` (a CSS2 duration such as `250ms` or
  `1.5s`). This package does not read a transition's `event:schema`
  ([the `event` vocabulary](../event/README.md), in the snapshot as
  `event_schema`) yet, so no tool declares `data`.
- **Tool choice**: `required` without `location` (function-calling mode),
  `auto` with it. Without `location`, a session that accepts no event
  raises `error.execution` before any request.
- **Calls**: every function call in the result is checked first (an
  undeclared function, arguments that are not a JSON object, a non-string
  `target` or `delay`, or an unparsable `delay` raise `error.execution`, and
  nothing is sent); then each is one `execution-host.send`, routed exactly
  as `<send>` routes it: the original event name, `target` (none: the
  session's own external queue), `delay` (the n-th delayed send of a
  session, counting from 0, gets the send id `openai.send.<n>`, which
  `<cancel>` takes; as for a `<send>` without `id`, the delivered event's
  `sendid` stays blank), and as data the `data` argument or else every other
  argument as one object. Without `location` at least one call is
  required; with it, the output text (when the result has one) is assigned
  after the sends.

When the host offers no snapshot (`unavailable`, as on a host without
`session-snapshot-host`), the request has no system message and no tools,
as agentml-go's has when its `Snapshot` fails.

The package declares `env-slots` `apikey` and `baseurl` and `http-origins`
`https://api.openai.com`: importing it consents to that origin. The host
admits the request there, or to the origin a `baseurl` value supplies
entirely (a local Ollama, for example); anything else is refused before it
is sent. The component imports no WASI interface.

Every failure raises `error.execution` in the session and the session keeps
running: a missing model or prompt; function-calling mode with no accepted
event, no call, an undeclared function, or invalid arguments; a failed
send; an evaluation failure; a
missing key (no variable named, or none the run holds), a base URL that
supplies no origin; a refused, unreachable, or timed-out
request; a non-2xx status (including a redirect, which is never followed);
a response that is not a Responses API result with assistant text; or a
failed assignment. Error details are static; the prompt, the reply, the
model name, and the key are never echoed or recorded.

## Statements

The behavior above as numbered statements, each tested by the namespace's
own implementation report plan ([`irp/`](irp/README.md), below). They
describe this package version exactly; the prose above is the explanation.

- <a id="openai-1"></a>**openai-1** (the element). An `<openai:generate>`
  element is executable content (SCXML 1.0 section 4.10) that the
  namespace's extension component executes where its block runs: it sends
  one Responses API request (`POST {base}/responses`) and, with `location`,
  assigns the reply text at `location` as a string after any sends.
- <a id="openai-2"></a>**openai-2** (the model). The model is
  `modelexpr`'s value when `modelexpr` is present and not empty, which must
  be a non-empty string, else the `model` attribute; without either the
  element raises `error.execution`. The model is sent as the request's
  `model`.
- <a id="openai-3"></a>**openai-3** (the prompt). The prompt is the
  `prompt` attribute, then `promptexpr`'s string value, then the trimmed
  text of each `<openai:prompt>` child in document order, the non-empty
  parts joined with a line break, sent as the one `user` message. Without a
  part, or with a `promptexpr` that fails or is not a string, the element
  raises `error.execution`.
- <a id="openai-4"></a>**openai-4** (options). `max-output-tokens`, a
  positive integer, is sent as `max_output_tokens`, and `reasoning` as
  `reasoning.effort`; both are omitted when absent, and a
  `max-output-tokens` that is not a positive integer raises
  `error.execution`.
- <a id="openai-5"></a>**openai-5** (key and endpoint). The `apikey` slot's
  value is sent as `Authorization: Bearer <value>`; without an `apikey`
  value the element raises `error.execution` and sends nothing. The request
  goes to `<baseurl>/responses` when the `baseurl` slot has a value, else to
  `https://api.openai.com/v1/responses`.
- <a id="openai-6"></a>**openai-6** (admitted origins). The host admits the
  request only to the origin the package declares (`https://api.openai.com`)
  or the origin a `baseurl` value supplies entirely; a base URL that
  supplies no origin raises `error.execution` and nothing is sent.
- <a id="openai-7"></a>**openai-7** (the reply). The reply is the first
  `output_text` of the first `message` of the result's `output`, which must
  be an assistant message; a body that is not a Responses API result, or a
  result without such a message or text, raises `error.execution`.
- <a id="openai-8"></a>**openai-8** (service failures). A request the
  service does not answer (unreachable, failed, or too large a response)
  and a non-2xx status, including a redirect, which is never followed,
  raise `error.execution`.
- <a id="openai-9"></a>**openai-9** (the system message). The request's
  first input is a `system` message holding the executing session's
  canonical session snapshot, taken at the `executable-content` point with
  `data_model` and `transitions` excluded; it shows the configuration the
  element's block observes.
- <a id="openai-10"></a>**openai-10** (tools). The tools are one
  `send_<event>` function per event descriptor of the transitions the
  active configuration can take, in the order the snapshot lists them: `.`
  becomes `_` and every other character outside `[A-Za-z0-9_-]` becomes
  `_`; `foo.*` and `foo.` name `foo`; `*` names no event; a name over 64
  bytes, or one an earlier event already took, gets no function. Each has
  the optional string parameters `target` and `delay`.
- <a id="openai-11"></a>**openai-11** (tool choice). With `location` the
  tool choice is `auto`; without it (function-calling mode) it is
  `required`, and a session whose active configuration accepts no event
  raises `error.execution` before any request.
- <a id="openai-12"></a>**openai-12** (function calls). Each function call
  the result carries is sent, in order, as the event the function names,
  routed as `<send>` routes it: `target` (none: the session's own external
  queue), `delay` (a CSS2 duration; the n-th delayed send of a session,
  counting from 0, gets the send id `openai.send.<n>`, which `<cancel>`
  takes), and as data the `data` argument, else every other argument as one
  object. Without `location` at least one call is required; with it the
  reply text is assigned after the sends.
- <a id="openai-13"></a>**openai-13** (invalid calls). Every call is
  checked before the first is sent: a function the tools did not declare,
  arguments that are not a JSON object, a `target` or `delay` that is not a
  string, or a `delay` that does not parse raises `error.execution`, and
  nothing is sent.
- <a id="openai-14"></a>**openai-14** (unsupported attributes and
  elements). The `stream` and `retry` attributes, and any element of the
  namespace other than `generate`, raise `error.execution`.
- <a id="openai-15"></a>**openai-15** (failures). Every failure raises
  `error.execution` in the session, which keeps running, and the rest of
  the element's block is not executed (SCXML 1.0 section 4.9).

Not covered by the suite: a host without `session-snapshot-host` (the
suite's host always offers snapshots), the network transport itself (the
suite's host reaches only an in-process stand-in), and the XSD (the suite
compiles with schema validation off, so `stream` and `retry` reach the
component).

## Conformance suite

[`irp/`](irp/README.md) is this namespace's implementation report plan in
the W3C IRP format: one assertion per statement above, 64 mandatory test
documents written with the W3C `conf:` vocabulary, prepared for the
ECMAScript and XPath data models by the W3C templates, and run through the
shipped components exactly as the W3C IRP is (`scxml-cli conformance run`,
[docs/conformance.md](../../docs/conformance.md#namespace-suites)). The
requests go to a deterministic in-process stand-in for the Responses API
with fake keys; nothing reaches the network. Both data models pass 64 of
64 (`irp/baselines/`).

## Differences from agentml-go

| agentml-go | this package |
|---|---|
| namespace `github.com/agentflare-ai/agentml-go/openai` | `https://github.com/stateforward/scxml.rs/namespaces/openai@v0.2.0` (absolute URIs are required here) |
| content-generation mode (`location` given) | implemented |
| function-calling mode (no `location`; `send_*` tools from the session's transitions) | implemented; tools come from every event descriptor of every listed transition (agentml takes each transition's first descriptor only); wildcard descriptors are normalized or skipped |
| tools declared from a transition's non-SCXML `schema` attribute (`data` parameter) | **not yet**: a transition's `event:schema` ([the `event` vocabulary](../event/README.md)) is in the snapshot's `event_schema`, but this package does not read it, so no tool has `data` |
| system message: the session's pruned SCXML snapshot (XML, document structure without executable content) | the canonical `scxml.session-snapshot/v2` JSON document with `data_model` and `transitions` excluded |
| tool calls executed as they stream; a later invalid call after earlier ones were sent | every call is checked before the first is sent; an invalid one raises `error.execution` and sends nothing |
| with `location` and tools, text output is discarded | sends the calls, then assigns the text when there is one |
| unknown function names fall back to `send_a_b` -> `a.b` | raise `error.execution` |
| `stream` (incremental `location` updates) and `retry` (re-asking the model after a call fails its data schema) | **not implemented**; rejected by the XSD and by the extension component (`error.execution`) rather than ignored. `retry` corrects schema failures of calls agentml already began executing; with no schemas and every call checked before any send, there is nothing for it to correct |
| Go `text/template` in `<openai:prompt>` (with an empty data map) and data-model evaluation of a `prompt` containing `${` or `{{` | **dropped**: prompt text is literal; use `promptexpr` for computed text |
| an unevaluable or non-string `modelexpr`/`promptexpr` silently falls back | raises `error.execution` |
| an invalid `max-output-tokens` is ignored | raises `error.execution` (the XSD types it `xs:positiveInteger`) |
| a result with no assistant text assigns `""` | raises `error.execution` |
| `os.Getenv("OPENAI_API_KEY")`, optional (the SDK sends none) | the `apikey` slot, required (any value for servers that ignore it, such as Ollama); the component holds a reference, never the key |
| `os.Getenv("OPENAI_BASE_URL")` | the `baseurl` slot (a reference; a base URL the run supplies is an origin the element may reach) |
| `net/http` client with a 90 s timeout, ambient proxy and environment | the host's admitted transport: the declared origin or a run-supplied one, no redirect, proxy, or ambient credential; 10 s connect, 120 s total |
| OTEL span with the model and location as attributes | no extension-owned span attributes: the guest records the closed `execute` operation and outcome; the host adds one `extension-http-fetch` span per request and one `session-snapshot` span per snapshot, each with only its closed outcome event and error category |

## Running the example

Build the components and the CLI, then run the example against any
OpenAI-compatible endpoint:

```console
sh scripts/verify-wasm-capabilities.sh --artifacts-dir target/openai-release \
  scxml_clock scxml_data_model_ecmascript scxml_data_model_null scxml_data_model_xpath \
  scxml_ioprocessor_scxml scxml_ioprocessor_basichttp scxml_extension_openai \
  scxml_compiler scxml_machine
cargo build -p scxml-cli --features wasmtime
OPENAI_API_KEY=sk-... sh namespaces/openai/run-example.sh target/openai-release \
  https://api.openai.com/v1
# Ollama's OpenAI-compatible endpoint (any key; a model it serves; the base
# URL the run supplies is an origin the element may reach):
OPENAI_API_KEY=ollama sh namespaces/openai/run-example.sh target/openai-release \
  http://127.0.0.1:11434/v1 llama3.2:1b
```

Function calling runs the `decide` example (the fourth argument):

```console
OPENAI_API_KEY=sk-... sh namespaces/openai/run-example.sh target/openai-release \
  https://api.openai.com/v1 gpt-4o-mini decide
```

Released, the element needs no local build: a document that binds its URI
loads it from the `v0.2.0` release (see
[Loading an element from a repository](../../docs/executable-content-extensions.md#loading-an-element-from-a-repository)).

Captured against a local OpenAI-compatible mock on 127.0.0.1:18741 whose
`/v1/responses` answers a `tool_choice: required` request with one call of
`send_answer_yes` (arguments `{}`) and any other request with the message
`Hello from <model>!`:

```console
$ OPENAI_API_KEY=sk-mock sh namespaces/openai/run-example.sh target/openai-release \
    http://127.0.0.1:18741/v1 mock-model
diagnostic summary: 0 error(s), 0 warning(s), 0 note(s), 0 total
compiler status: Emitted
compiler output: lowered-ir (4521 bytes, sha256 d5c105ab251f7d344ce21a47c84e176ba5bcb4338a14b22fb428ab0e57017b24)
snapshot scxml.session-snapshot/v2 sha256 24bb178f4b8ec696260f614608d751d678576c5f9d3e6cb80d2d28142ce9c11d
 = clock: 0ms
 = components: clock ad28e6acd406, data-model-ecmascript 387859f3efc4, io-processor-scxml 1974094e0771, machine 6d53bc5c4a0f
 = options: exclude [], trace_context strip

session root `session` datamodel ecmascript ir v5 d5c105ab251f
  --> status: finished
   = configuration: atomic [answered] full [answered]
   = counters: send 0, invoke 0
   = system: _sessionid `session`, _name undefined, _ioprocessors {"basichttp":{"location":"http://basichttp.loopback.invalid/sessions/session/basichttp"},"http://www.w3.org/TR/scxml/#Ba… (339 bytes)
   = data model:
       model = "mock-model"
       reply = "Hello from mock-model!"
   = queues: internal 0, external 0
   = pending sends: none
   = transitions: none
   = invocations: none
```

The mock received one request: `POST /v1/responses`,
`Authorization: Bearer ...`, `model` `mock-model`, `max_output_tokens` 64,
`tool_choice` `auto`, the tool `send_error_execution` (the only event `ask`
accepts), and `input` = a `system` message holding the session's snapshot
(`options.exclude` `["data_model","transitions","previews","invocations"]`,
`options.point` `executable-content`, `configuration`
`{"atomic":["ask"],"full":["ask"]}`) and the `user` message
`Answer in at most five words.\nSay hello to a state machine.`.

```console
$ OPENAI_API_KEY=sk-mock sh namespaces/openai/run-example.sh target/openai-release \
    http://127.0.0.1:18741/v1 mock-model decide
diagnostic summary: 0 error(s), 0 warning(s), 0 note(s), 0 total
compiler status: Emitted
compiler output: lowered-ir (3751 bytes, sha256 f99fcb32cf72f48f4ac081e304cae95e6044313575ed58da04fc2f5b9fa4fcb8)
snapshot scxml.session-snapshot/v2 sha256 e153bdfa7fb32090c37f03b72ad06f87c921214a133b8143914fa7184fc41a13
 = clock: 0ms
 = components: clock ad28e6acd406, data-model-ecmascript 387859f3efc4, io-processor-scxml 1974094e0771, machine 6d53bc5c4a0f
 = options: exclude [], trace_context strip

session root `session` datamodel ecmascript ir v5 f99fcb32cf72
  --> status: finished
   = configuration: atomic [yes] full [yes]
   = counters: send 0, invoke 0
   = system: _sessionid `session`, _name undefined, _ioprocessors {"basichttp":{"location":"http://basichttp.loopback.invalid/sessions/session/basichttp"},"http://www.w3.org/TR/scxml/#Ba… (339 bytes)
   = data model:
       model = "mock-model"
   = queues: internal 0, external 0
   = pending sends: none
   = transitions: none
   = invocations: none
```

That request carried `tool_choice` `required`, the tools `send_answer_yes`,
`send_answer_no`, and `send_error_execution`, and the same kind of system
message; the mock's call of `send_answer_yes` was sent as `answer.yes` and
moved the machine to `yes`. With a base URL the mock answers 500, a base URL
that supplies no origin, or no `OPENAI_API_KEY`, either run finishes in
`failed`; the last two send nothing. (These captures predate the `env` slots;
the run now also lists the element, its version, its declared origin, and its
slots before it starts, and the component bytes and digests differ.)

## Proof

- `namespaces/openai/irp` ([Conformance suite](#conformance-suite)): every
  statement above, through the shipped components, in both data models
  (`cargo test --release -p scxml-conformance --features wasmtime --test
  irp_components --test checked_in_namespace_irp`).
- `cargo test -p scxml-extension-openai`: request building, prompt
  concatenation order, response extraction, the two snapshot exclusion
  sets (pruning), tool derivation and naming, call-to-send mapping and
  delay parsing, function-calling and `auto` modes, and every error
  mapping, behind `OpenAiHost`.
- `cargo test -p scxml-host-wasmtime --features wasmtime,http --test openai_extension`:
  the shipped component under the session host against a mock on
  127.0.0.1 (success drives the transition; the system message is a
  canonical executable-content snapshot and the tools are the events `ask`
  accepts; a mock function call drives the machine to its target; an
  undeclared function, non-2xx, an origin neither declared nor run-supplied,
  no transport, a missing key, and an unreachable service raise
  `error.execution`).
- `cargo test -p scxml-cli --features wasmtime --test openai_extension`:
  the same through `scxml-cli validate` and `snapshot run` (both
  examples), the element listing, masking, the missing-value note, and the
  removed flags.
- `cargo test -p scxml-cli --features wasmtime --test repository_namespaces`:
  the element loaded from a mock GitHub release, and isolation between it
  and another element.
- `cargo test -p scxml-host-wasmtime --features wasmtime,http --test executable_content_extensions`:
  the snapshot capability itself through the example namespace, including
  `unavailable` outside `execute`.
