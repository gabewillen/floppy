# Observability and cardinality

Every runtime boundary emits OpenTelemetry signals through a facade whose
labels are closed sets, so no document, event, path, or digest can grow
metric or span cardinality. The per-operation tables for contributors are
in [CONTRIBUTING.md](../CONTRIBUTING.md#opentelemetry-and-cardinality).

## The telemetry facade

Native crates use the lightweight `scxml-telemetry` OTEL API facade with static
bounded module/operation/error/event enums and parent context. Spans open only
at boundaries (component and WIT entry points, the facade crates' public
validate/compile/package/run operations, capability and I/O
calls, and host lifecycle); getters, pure helpers, and hot inner loops run
inside the enclosing span and emit nothing of their own, and every operation
label names exactly the operation it covers. `cargo run --release --locked -p
scxml-bench` measures the engine hot path so telemetry changes carry
benchmark evidence. Machine, I/O,
and clock guests import versioned WIT telemetry capabilities and bind no
SDK/exporter; host adapters own export. Clock and I/O operation, outcome, and
event values are closed WIT enums. Effects, input bytes, snapshot bytes, queue
depth, and fuel use fixed kind-specific instruments and units without metric
labels; guest events never duplicate observation samples. Guests do not use
the WASI OpenTelemetry interfaces; telemetry crosses only the versioned WIT
facade.

## Trace propagation through events

Spans propagate through events. Each event carries the W3C carrier of the
machine batch span that emitted it, and the batch that processes it is that
span's child on every route both hosts provide (`<raise>`, immediate and
delayed self sends, `#_<invokeid>`, `#_parent`, autoforward, `done.invoke`,
send errors), plus the Wasmtime host's `BasicHTTP` loopback and external
client POST (`traceparent`/`tracestate` headers) and carriers the embedder
injects; baggage follows each carrier's `baggage-policy`. Carriers never
change semantics or `_event.raw`. The per-route guarantees and their proof
are in [CONTRIBUTING.md](../CONTRIBUTING.md#trace-propagation-through-events).

## Packaging and host spans

The packager uses the static `packager` module label, the closed operations
`scxml.packager.package` and `validate-template` (IR validation, provider and
bundle encoding, and composition run inside the `package` span), and the
closed `packaging` failure category; the Wasmtime host adds the closed
`scxml.host-wasmtime.ir-source` operation. The Wasmtime host's own
spans use the tracer `scxml-host-wasmtime` and the closed span names
`component-execute`, `machine-graph-link`, `machine-session`,
`capability-graph-execute`, `session-run`, and `component-compile` (one per
session-host compiled-component cache miss, on the global tracer provider
because the cache is process-wide). A live session adds one boundary span per
call: `scxml.host-wasmtime.session-open`, `session-send`, `session-advance`
(only when a deadline fires), `session-receive-http`,
`session-communication-error`, and `session-close`, plus `scxml.cli.run` for
the command, and the `http` feature adds one `basichttp-network-post` span
per outbound request (a child of the send that issued it) and one
`basichttp-listener-request` span per inbound connection (with
`basichttp-listener-accept` for a failed accept and
`basichttp-listener-connection` for an accepted connection it dropped or
could not keep), with no URL, host,
header, status, or body attribute; `component-execute`,
`machine-graph-link`, `capability-graph-execute`, and `component-compile`
record `scxml.outcome` `ok` or `failed`, while
`machine-session` and `session-run` record a failure only as the error status
`failed`. A refused guest telemetry import (malformed carrier, exhausted span
table or lease, unknown handle) adds the one closed event
`guest-telemetry-refused` to the host execution span. Its guest spans use the
tracers `scxml-component-guest` (`scxml.module` `machine-component`,
`data-model-component`, `script-component`, `compiler-component`, whose one
span name is `compile`, or `validator-component`, whose one span name is
`validate`), `scxml-capability-guest` (`capability-component`), and
`scxml-guest`; component measurements use the meter `scxml-host-wasmtime`.
The compiler and validator guest spans record `scxml.outcome` `ok` or
`failed`, their closed WIT span events (`input-rejected`,
`artifact-produced`), and a failure's closed WIT error category as
`error.type` (validator `invalid-input`, `validation`, `report`; compiler
also `lowering` and `packaging`). The build-time `scxml-wasm-meter` tool uses
module `wasm-meter` and one span per boundary: `scxml.wasm-meter.instrument`
per `meter()` call and `scxml.wasm-meter.cli` per command-line run, never one
per function or loop head.

The browser host emits through the OpenTelemetry JS API. Its host spans use
the tracer `scxml-host-browser`, `scxml.module` `host-browser`, the nine
closed host operations (`create-session`, `ir-source`, `command`,
`capability`, `data-model`, `compile`, `load-core-module`, `delayed-send`,
`invocation`), and outcomes `ok`/`rejected`/`failed`, counted by
`scxml.host.operations`. Its guest spans reuse the Wasmtime host's tracers,
`scxml.module` values, and measurement instruments
([table](../hosts/scxml-host-browser/README.md#telemetry-cardinality)).
None carries a digest,
byte count, template identity, or IR content as an attribute; IR size is only
the guest's numeric `input-bytes` measurement.

## Wasm script components

External scripts add the static `script` module label with the closed
operations `scxml.script.verify` and `validate-component`, plus the Wasmtime
host's `scxml.host-wasmtime.script-load` and `script-run`; script pinning runs
inside `scxml.compiler.compile` and bundle encoding inside
`scxml.packager.package`. Script component guest
signals carry the fixed `scxml.module` value `script-component`. No URI,
media type, digest, byte count, or script payload is ever an attribute.

## Executable content extensions

Extension namespaces add the Wasmtime host's boundary spans
`scxml.host-wasmtime.extension-instantiate` (per extension component per
session), `extension-execute` (per executable-content extension),
`extension-shutdown`, and `extension-http-fetch` (per `outbound-http` request an
extension component makes), the existing `session-snapshot` (per
`session-snapshot-host.snapshot` an extension component calls), and the CLI's
`scxml.cli.read-extension-package`, the session-start
`scxml.host-wasmtime.extension-load` (per `load-extensions` call), and the CLI's
repository namespace spans `scxml.cli.resolve-extension-package` and
`scxml.cli.resolve-extension-component`; each records only its closed outcome and
signal event (and, on failure, its closed error category). Extension component
guest signals carry the fixed `scxml.module` value
`executable-content-extension`, and an extension component's
`execution-host.log` adds one `extension-log` event whose only attribute is
`scxml.log-level` (five values). Namespace URIs, element and attribute names,
environment variable names or values, request URLs, headers, bodies, statuses,
prompts, model names, and log messages are never attributes or metric labels.

## Invoked services

Invoked services add the Wasmtime host's boundary spans
`scxml.host-wasmtime.service-load` (per `load-services` call),
`service-start` (per `start-service`, including the instance's `type-uri`
check and `start`), `service-send` (per event delivered to a service),
`service-wake` (per due wakeup), `service-cancel` (per cancelled service),
and `service-http-fetch` (per `outbound-http` request a service makes), and
the CLI's `scxml.cli.read-service-package` and, for repository services,
`scxml.cli.resolve-service-package` and `scxml.cli.resolve-service-component`;
each records only its closed
outcome and signal event (and, on failure, its closed error category).
Service guest signals carry the fixed `scxml.module` value
`invoked-service` and the guest's closed WIT operation enum, which gains
`wake`. Service type URIs, invoke ids, events, payloads, URLs, and slot
names are never attributes or metric labels; the set of labels stays closed.

## Validation

The validation path records one span per boundary, never per rule, element,
or expression: `scxml.syntax.parse-xml` (module `syntax`) per parse;
`scxml.validator.validate-source` or `validate-document`,
`scxml.validator.xsd-validate`, and `scxml.validator.schema-file-read`
(module `validator`); the `scxml-reporting` public operations
`artifact-validate`, `encode-canonical`, `decode-canonical`, `rustc-render`,
and `render-sarif`; the validator and reporter components' WIT entry spans;
and the CLI's `main`, command (`validate`, `compile`, `explain`,
`conformance`, `conformance-prepare`), file-read (`read-source`,
`read-template`, `read-script`, `read-event-schema`), and `emit` spans. Failures carry only a
closed `ErrorCategory` value (for example `validation`, `input-read`, or
`command-budget`); no diagnostic code, count, path, or namespace URI is an
attribute. `cargo run --release --locked -p scxml-bench --bin validator`
measures validator and reporter throughput for telemetry changes.

## Execution

Execution records one span per boundary, never per transition, state,
configuration query, effect-planning step, or conversion. The engine
(module `engine`) opens `scxml.engine.new`, `run` (one host command to
a stable configuration), one `start`, `send`, `resume`, or `cancel` per engine command,
and, on its failure paths only,
`queue-platform-error` and `block-failed`. The ECMAScript and
`XPath` data models record their public operations (initialize, evaluate,
assign, execute, check-syntax), and every guest records
only its WIT entry points; the machine guest adds one span per effect batch
and one `input-rejected` event per failed guard or effect capability call.
The conformance runner records `run-corpus`, `run-suite`, one `run-case` and
one `run-case-wasm` per case, and its file, process, and
network I/O (suite load/write, file digests, errata load, corpus
preparation, manifest and corpus-file acquisition, download, the XSLT
transformer's identity and transform runs, report JSON write/read). The
Wasmtime `BasicHTTP` linker adds the closed
`scxml.host-wasmtime.basichttp-instantiate`, `basichttp-send`,
`basichttp-receive`, and `basichttp-transport-post` operations. Every one of
these labels is a closed `Operation` variant; none carries a state, event,
expression, URL, or path.

## Data-model components and the validator adapter

A data-model component linked by the Wasmtime host adds the closed
`scxml.host-wasmtime.data-model-instantiate` operation (per-call data-model
work is recorded by the data-model guest's own spans), and its guest signals carry the
fixed `scxml.module` value `data-model-component` plus the guest's closed WIT
operation/outcome/event enums. The validator adapter (`ComponentValidator`)
adds the closed `scxml.host-wasmtime.validator-new` (one compile and link)
and `validator-validate` (one request) operations; the data model's
`check-syntax` calls run inside `validator-validate`. Every `scxml.module`
value is a static component label, so the set stays closed. Operation spans a guest opens through
`telemetry.start` go through one bounded span table per Wasmtime store (at
most 1024 open spans; a refused span answers handle `0` and the guest
continues without it). The machine, clock, and I/O processor guests open
spans today; the data-model and script stores accept and export
them the same way, although the bundled data-model and script guests only emit
point signals. Expressions, values, state ids, and
events are never attributes.

Live sessions add per-call spans listed in
[live-session.md](live-session.md#telemetry).

## Forbidden attributes

Session/test/spec IDs, event/state names, expressions, payloads, paths, raw
URLs, namespace URIs, component identities, corpus/artifact digests, messages,
and timestamps are forbidden as indexed telemetry attributes. Detailed context
stays in deterministic reports and diagnostics.
