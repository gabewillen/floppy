// Public contract of the SCXML browser host. WIT records use the jco
// JavaScript shapes (camelCase fields, `{tag, val}` variants, kebab-case
// enum strings) of stateforward:scxml@0.2.0.
//
// This file is the canonical contract for page code; README.md summarizes
// it. `test/contract.test.mjs` fails when the runtime exports of
// src/index.js and the `export` declarations here differ, when a closed
// vocabulary (error codes, statuses, schema policies) drifts from its WIT
// enum, or when a WIT record type here drifts from jco's generated shape.
//
// Telemetry: the host emits through the OpenTelemetry JS API
// (`@opentelemetry/api`, its one runtime dependency) and installs no SDK;
// see TelemetryOption.
//
// Capabilities: the host links one SCXML Event I/O Processor and its own
// clock per session, forwards every data-model function to the session's
// data model, and runs invoked sessions (at any depth, within InvokeLimits)
// through each session's external event queue in the Wasmtime executor's
// order, as the Wasmtime session host does, compiling invoked
// documents itself (InvokeOptions, `compile: 'auto'`) unless the page
// supplies `capabilities.compileInvocation`. It links the BasicHTTP Event
// I/O Processor component, whose transport is the page's `http` capability
// (HttpCapability; deny-all without it).
// Deliberately unsupported (typed, never a silent no-op): `<invoke>` under
// `invoke: { compile: 'provided' }` without `compileInvocation`
// (error.execution in the machine, detail UNSUPPORTED.invoke), and URLs the page neither
// allowlists through `resources` nor supplies through
// `capabilities.loadDocument` (error.execution; a runtime-computed URL has
// detail UNSUPPORTED.dynamicUrl). The host never fetches a document by
// itself: without `resources` no URL is resolved (deny-all).
//
// Failure families (all messages and details are static; caller values are
// never reflected):
// * ScxmlHostError: the host rejected an argument before entering a
//   component, or a component call could not complete (HostErrorCode);
// * ScxmlSessionError: the machine returned a closed WIT component-error;
// * ScxmlCompileError: the compiler rejected a request;
// * CapabilityError: thrown by providers to return one WIT error-code.
//
// Bounds: every host call (a session command, query, timer or `advanceTime`
// delivery, `receiveHttp`, pre-warm compile, or a standalone data-model or
// compiler call) runs on one deterministic fuel budget (`fuel` option,
// default DEFAULT_FUEL, counted in charges: one per function entry and per
// loop iteration of a metered core module) shared by every component the
// call reaches. The data-model, I/O processor, clock, BasicHTTP, compiler,
// and child-machine calls a machine command makes are charged to that
// command's budget, each also capped by its own per-call fuel (the data
// model's `fuel`, `invoke.fuel`), as under the Wasmtime host. Exhaustion
// throws ScxmlHostError('fuel-exhausted') instead of hanging the page (a
// data model that exhausts only its own cap throws CapabilityError
// ('platform'), which fails the machine session); that instance is never
// entered again. The count depends only on the executed path, so exhaustion
// is deterministic for identical inputs and builds. The budget bounds work,
// not wall-clock time.

import type { MeterProvider, TracerProvider } from '@opentelemetry/api';

export type ErrorCode =
  | 'invalid-input'
  | 'duplicate-id'
  | 'unknown-id'
  | 'capacity-exhausted'
  | 'payload-too-large'
  | 'unsupported-version'
  | 'protocol'
  | 'execution'
  | 'unavailable'
  | 'platform';

export type HostErrorCode =
  /** `ir` exceeds MAX_MACHINE_IR_BYTES; nothing was instantiated. */
  | 'machine-ir-too-large'
  /** An argument or option has the wrong type or shape; nothing changed. */
  | 'invalid-argument'
  /** A component could not be instantiated or trapped; never entered again. */
  | 'instance-lost'
  /** A call exhausted its fuel budget; the instance is never entered again. */
  | 'fuel-exhausted'
  /** A compiler result failed its report integrity check. */
  | 'report-mismatch'
  /**
   * The session's own SCXML I/O processor or clock broke its contract; the
   * session is never entered again.
   */
  | 'host-invariant'
  /**
   * A session method was called while another operation of that session
   * was in progress (a capability provider or data model calling back into
   * the session); nothing was entered and the session stays usable.
   */
  | 'reentrant-call';

export type CompilerErrorCode =
  | 'invalid-request'
  | 'invalid-package'
  | 'unsupported-artifact'
  | 'canonical-report-failed'
  | 'invalid-template'
  | 'packaging-failed';

export type DataValue =
  | { tag: 'absent' }
  | { tag: 'undefined' }
  | { tag: 'null' }
  | { tag: 'boolean'; val: boolean }
  | { tag: 'number'; val: number }
  | { tag: 'text'; val: string }
  | { tag: 'json-array'; val: string }
  | { tag: 'json-object'; val: string }
  | { tag: 'xml'; val: string };

/**
 * WIT `assign-placement` (SCXML 1.0 section 5.4, and the XPath data model's
 * `type` and `attr` in SCXML 1.0 CR 2014-03-13 §C.3.6); `replace-children` is the default and the only placement the
 * ECMAScript and null data models define.
 */
export type AssignPlacement =
  | { tag: 'replace-children' }
  | { tag: 'first-child' }
  | { tag: 'last-child' }
  | { tag: 'previous-sibling' }
  | { tag: 'next-sibling' }
  | { tag: 'replace' }
  | { tag: 'delete' }
  | { tag: 'add-attribute'; val: string };

/**
 * WIT `assign-source` (SCXML 1.0 sections 5.4 and 4.6): an `expr` the data
 * model evaluates and assigns in one call, a value the machine materialized
 * for an existing location (inline `<assign>` children, or `absent` for
 * `delete`), or a `<foreach>` item or index whose variable the data model
 * declares when it is not declared yet.
 */
export type AssignSource =
  | { tag: 'expression'; val: string }
  | { tag: 'value'; val: DataValue }
  | { tag: 'foreach-value'; val: DataValue };

export type EventType = 'internal' | 'external' | 'platform';

/** WIT `trace-context`: a W3C carrier (validated by its receiver). */
export interface TraceContext {
  /** Canonical version-00 W3C `traceparent` (55 bytes). */
  traceparent: string;
  /** W3C `tracestate`: at most 512 bytes and 32 list members. */
  tracestate?: string;
  /** W3C baggage: at most 8192 bytes and 64 list members. */
  baggage?: string;
  baggagePolicy: 'drop' | 'propagate';
}

/** WIT `event` as delivered to capability providers. */
export interface WitEvent {
  name: string;
  eventType: EventType;
  sendId?: string;
  origin?: string;
  originType?: string;
  invokeId?: string;
  data: DataValue;
  raw?: string;
  traceContext?: TraceContext;
}

/**
 * An external event passed to `MachineSession.send`. External events arrive
 * through an Event I/O Processor, so the type is always `external` and
 * there is no invoke id: only the machine raises `internal` events, only
 * the platform raises `platform` ones (SCXML 5.10.1; the host's late
 * `error.communication`), and only an invoked child's own messages carry an
 * invoke id (SCXML 6.4). Another type is ScxmlHostError('invalid-argument');
 * the machine rejects an `invokeId` as ScxmlSessionError('invalid-input');
 * neither is processed.
 */
export type EventInput =
  | string
  | {
      name: string;
      type?: 'external';
      data?: DataValue;
      sendId?: string;
      origin?: string;
      originType?: string;
      /**
       * The page's W3C carrier. The machine's batch span that processes the
       * event becomes its child, and its baggage follows `baggagePolicy`.
       * Validated by the machine (a malformed one is `invalid-input`);
       * never changes semantics.
       */
      traceContext?: TraceContext;
    };

/** WIT `named-value`: one `<param>` or `namelist` entry. */
export interface NamedValue {
  name: string;
  value: DataValue;
}

/** WIT `data-initializer`: how one `<data>` declaration is valued (SCXML 5.2-5.3). */
export type DataInitializer =
  | { tag: 'expression'; val: string }
  | { tag: 'value'; val: DataValue };

/** WIT `inline-body`: inline `<content>` or `<data>` children (SCXML 5.6, 6.2.6). */
export type InlineBody = { tag: 'text'; val: string } | { tag: 'markup'; val: string };

/** WIT `foreach-result`: a `<foreach array>` snapshot (SCXML 4.6). */
export type ForeachResult =
  | { tag: 'items'; val: DataValue[] }
  | { tag: 'illegal' };

/** WIT `data-model-api.system-values` (SCXML 5.10). */
export interface SystemValues {
  sessionId: string;
  /** `_name`: the document's `name` attribute; undefined when it has none (SCXML 5.10). */
  machineName?: string;
  ioProcessors: DataValue;
}

/** WIT `qualified-name`: an exact expanded XML name. */
export interface QualifiedName {
  namespaceUri?: string;
  localName: string;
}

/** WIT `extension-attribute`. */
export interface ExtensionAttribute {
  name: QualifiedName;
  value: string;
}

/** WIT `extension-content`: flattened element content in document order. */
export type ExtensionContent =
  | { tag: 'text'; val: string }
  | { tag: 'start-element'; val: { name: QualifiedName; attributes: ExtensionAttribute[] } }
  | { tag: 'end-element'; val: QualifiedName };

/** WIT `executable-content-extension`: one element of additional executable content (SCXML 4.10). */
export interface ExecutableContentExtension {
  name: QualifiedName;
  attributes: ExtensionAttribute[];
  content: ExtensionContent[];
}

/** WIT `pinned-script`: the compile-time identity of one external script. */
export interface PinnedScript {
  mediaType: string;
  /** SHA-256 of the exact script bytes, 64 lowercase hex characters. */
  sha256: string;
  witWorld?: string;
}

/** WIT `script-reference`: one `<script src>` reference. */
export interface ScriptReference {
  /** The `src` URI exactly as authored. */
  uri: string;
  /** Compile-time pin; absent for runtime resolution. */
  pinned?: PinnedScript;
}

export type Status = 'ready' | 'running' | 'finished' | 'failed' | 'shut-down';

export interface CallResult {
  status: Status;
  activeConfiguration: string[];
  /**
   * Effects this session ran for the command, counted across every root
   * machine call the command made: a command that starts or cancels an invoked
   * session runs in pieces (the session suspends at the `<invoke>` and
   * continues once the host carried it out), and the command also runs
   * every queued root event it reaches, whether its own work queued it (an
   * event an invoked session sent this session while starting, for one)
   * or an earlier command left it behind at the delivery bound. The result
   * is that of the command's last call, and reports the whole command.
   */
  effectCount: number;
  /**
   * False only when the machine stopped at its per-call command bound (call
   * `resume`; delayed sends wait until the session is stable again).
   */
  stable: boolean;
  /**
   * `<donedata>` of a session started as an invoked session; always absent
   * for sessions this host creates.
   */
  returnData?: DataValue;
  /**
   * Present when this call finished the session and its last effect batch
   * has a W3C carrier: that batch's carrier (WIT `call-result.done-trace-context`),
   * which the host passes on a child's `done.invoke`.
   */
  doneTraceContext?: TraceContext;
}

export interface ComponentErrorRecord {
  version: 'v1';
  code: ErrorCode;
  detail?: string;
}

/**
 * Data-model capability: the WIT `data-model-api` functions the host
 * forwards one-to-one from `machine-capabilities`, with identical semantics
 * and errors. `event` is `undefined` where SCXML defines no current event.
 * Throw `CapabilityError` to return a closed code; any other exception
 * becomes `platform`, and a malformed return value is `platform`. A missing
 * fallible method is `unavailable`. The pure methods (`namelistKey`,
 * `eventPayload`, `inlineContent`, `externalData`, `eventDataField`) have
 * no error channel: a missing method, an exception, or a malformed value
 * answers `undefined` (WIT `none`), which the machine treats as the WIT
 * defines (usually error.execution). `acceptsEventData` returns a WIT
 * `bool`: a missing method, an exception, or a value that is not a boolean
 * answers `true` (the data is accepted).
 * `releaseForeach` answers nothing: it is forwarded untraced, and a missing
 * method or an exception is ignored. A provider serves exactly one session.
 */
export interface DataModelProvider {
  /**
   * Stable data-model name, read once by createSession. It selects the
   * machine's data-model semantics and the snapshot's `datamodel_name` and
   * `data-model-<name>` role; any other value is `invalid-argument`.
   */
  name?: 'ecmascript' | 'null' | 'xpath';
  /**
   * Declares `declarations` with the placeholder value and binds the system
   * variables. The machine calls it once from `start`, through the host,
   * with the host's `_sessionid` and `_ioprocessors` and the document's
   * `_name`.
   */
  initialize?(declarations: string[], systemValues: SystemValues): void;
  /** Declares one late-bound `<data>` variable (SCXML 5.3). */
  declare?(id: string): void;
  /** Values one declared `<data>` variable (SCXML 5.2-5.3). */
  initializeData?(id: string, initializer: DataInitializer): void;
  /** Sets one invocation parameter; false when `name` is not declared. */
  setParameter?(name: string, value: DataValue): boolean;
  /**
   * Updates the configuration `In()` observes: removes every `exited` state
   * (the others keep their order), then appends every `entered` state in
   * order (WIT `update-active-configuration`). It starts empty.
   */
  updateActiveConfiguration?(exited: string[], entered: string[]): void;
  evaluateCondition?(expression: string, event: WitEvent | undefined): boolean;
  evaluateValue?(expression: string, event: WitEvent | undefined): DataValue;
  /** Reads one existing location (`<param location>`, `namelist`). */
  evaluateLocation?(location: string, event: WitEvent | undefined): DataValue;
  /** A string-typed attribute expression (`eventexpr`, `delayexpr`, ...). */
  evaluateText?(expression: string, event: WitEvent | undefined): string | undefined;
  /** Renders one `<log expr>` to its log text (SCXML 4.7). */
  evaluateLog?(expression: string, event: WitEvent | undefined): string;
  /** One BasicHTTP form value (SCXML C.2). */
  evaluateFormValue?(source: string, location: boolean, event: WitEvent | undefined): DataValue;
  /** A `<foreach array>` item snapshot (SCXML 4.6). */
  foreachItems?(expression: string, event: WitEvent | undefined): ForeachResult;
  /**
   * Forgets every `<foreach>` loop this data model still records (WIT
   * `release-foreach`): the machine calls it after every one of its calls
   * that returns without suspending, so a loop whose block a failed
   * effect aborted (SCXML 4.6) stays recorded no longer. Untraced (no span
   * or counter); a missing method or an exception is ignored.
   */
  releaseForeach?(): void;
  /** The payload key of one `namelist` entry; `undefined` when illegal. */
  namelistKey?(location: string): string | undefined;
  /** `_event.data` from `<param>`/`namelist` entries; `undefined` when not carriable. */
  eventPayload?(entries: NamedValue[]): DataValue | undefined;
  /** The value of inline `<content>`/`<data>` children. */
  inlineContent?(content: InlineBody): DataValue | undefined;
  /** The value of an external `<data src>` document this data model decides. */
  externalData?(text: string): DataValue | undefined;
  /** The value event data carries under `name` (an empty `<finalize>`'s update, SCXML 6.5.2). */
  eventDataField?(data: DataValue, name: string): DataValue | undefined;
  /**
   * Whether this data model can represent `data` as `_event.data` (SCXML
   * C.1). A missing method, an exception, or a non-boolean answer is `true`.
   */
  acceptsEventData?(data: DataValue): boolean;
  assign?(
    location: string,
    source: AssignSource,
    placement: AssignPlacement,
    event: WitEvent | undefined,
  ): void;
  execute?(script: string, event: WitEvent | undefined): void;
  /**
   * Read-only dump of the system values and user variables (the WIT
   * `data-model-api.snapshot` record; docs/session-snapshot.md, "Session
   * object").
   * A provider without `snapshot()` is rejected only when the session
   * snapshot requests `system` or `datamodel`.
   */
  snapshot?(): {
    system: { sessionId: string; machineName?: string; ioProcessors: DataValue };
    variables: {
      name: string;
      value:
        | { tag: 'value'; val: DataValue }
        /** A value with no portable form: its type and string form. */
        | { tag: 'text'; val: { kind: string; text: string } }
        /** A variable over the per-value bound: its byte size only. */
        | { tag: 'oversized'; val: bigint };
    }[];
  };
}

/**
 * A bundled data-model component linked as a complete provider. Its pure
 * methods answer `undefined` (WIT `none`) for a malformed argument, after a
 * trap, and on a lost instance, except `acceptsEventData`, which answers
 * `true` there (the data is accepted), as the Wasmtime host does.
 */
export type BundledDataModel = Required<DataModelProvider> & {
  readonly name: 'ecmascript' | 'xpath' | 'null';
};

/**
 * Page-supplied machine capabilities; missing methods are `unavailable`
 * (except `log`, which is optional). Throw `CapabilityError` to return a
 * closed code; any other exception becomes `platform`. Calls are
 * synchronous, ordered, and never re-entrant. Providers (and a page-supplied
 * data model) run inside a session operation and must not call back into
 * the session: every MachineSession method they call throws
 * ScxmlHostError('reentrant-call') without entering any instance, and the
 * operation in progress continues unaffected (run to completion; the
 * Wasmtime SessionEmbedder contract likewise forbids calling back into the
 * host). Any other function-valued
 * property (for example `send` or `cancel`, which belong to the session's
 * linked SCXML I/O processor) makes createSession throw 'invalid-argument',
 * so a provider is never silently ignored.
 *
 * `log` receives every `<log>` record (the data model's rendered text, of
 * any length the data model renders). Without it, `<log>` is still accepted and recorded
 * in the session trace.
 *
 * `loadDocument` resolves one external document exactly as authored:
 * `kind` 'data' for `<data src>`, 'invocation' for `<invoke src>`. It
 * returns the text, or `{text, mediaType}` with the media type the source
 * indicated (an HTTP `Content-Type` the page read, for example): SCXML
 * B.2.1 lets the ECMAScript data model interpret `<data src>` content by
 * such an indication, and it reads a JSON or XML one (see
 * LoadedDocumentAnswer). The host
 * never fetches a URI: without this provider, or when it throws
 * `unavailable`, the machine raises error.execution (SCXML 5.3). A document
 * of any size is accepted. A URL the
 * `resources` option prefetched is served from that cache and never
 * reaches this provider; with `resources`, it answers only runtime-computed
 * URLs.
 *
 * `compileInvocation` is optional: by default the host compiles `<invoke>`
 * children itself (see InvokeOptions). When present it overrides that
 * compiler: it returns the lowered Machine IR
 * of one child document (`sourceName` is the `src` file name, or the
 * machine's name for inline `<content>`; `source` is the exact document
 * text), which the page lowered beforehand (the bundled compiler with
 * `schemaPolicy: 'off'`, or the CLI). It must be synchronous: throw
 * `invalid-input` for a document it does not provide, and the invocation
 * fails with error.execution (as for a child the compiler rejects); any
 * other thrown code fails the session. With either compiler, createSession
 * compiles every child component's core modules first, so each child is
 * then instantiated synchronously inside the parent's `<invoke>`. Children
 * share `loadDocument`, `loadScript`, and `runScript`; only the root's
 * `<log>` records reach `log` (children's are in the trace). An invoked
 * session may invoke in turn; an invocation beyond the limits the page set in
 * `invokeLimits` fails with error.execution, as under Wasmtime.
 *
 * `loadScript` and `runScript` are the WIT `machine-capabilities`
 * `load-script` and `run-script` functions. An unpinned `<script src>` the
 * `resources` option prefetched is loaded and run by the host (see
 * ResourceProviders) and never reaches them. Otherwise the browser host
 * forwards them unchanged: it does not fetch, authenticate, or bound script bytes, so a
 * page that provides `loadScript` owns every obligation that contract puts
 * on the host:
 * - resolve `reference.uri` exactly as authored, from sources the page
 *   trusts; throw `unavailable` when it cannot be resolved or the page
 *   refuses it, and `platform` for a fetch failure;
 * - with `reference.pinned`, accept only bytes whose SHA-256 and media type
 *   equal the pin; without a pin, accept only digests on the page's own
 *   allowlist; throw `invalid-input` otherwise, and for an unsupported
 *   media type or text in a language other than the active data model's;
 * - throw `payload-too-large` for an empty script, and for one beyond any
 *   bound the page sets (the Wasmtime host bounds a Wasm component at
 *   4194304 bytes and leaves data-model text to the data model, whose own
 *   bound raises error.execution when the script runs). No bound applies
 *   to the distinct URIs one session loads.
 * The machine calls `loadScript` while `createSession` builds the session,
 * once per distinct `<script src>` in document order. Any error it throws
 * (including `unavailable` when `loadScript` is missing) rejects the
 * document (SCXML 1.0 section 5.8), so `createSession` rejects with
 * ScxmlSessionError (`sessionFailed: true`, `code` the thrown code).
 * `runScript` runs a script `loadScript` accepted, with `_event` bound to
 * `event`; throw `protocol` for a reference that was never loaded. Its
 * `invalid-input` and `execution` errors become SCXML `error.execution`.
 */
/**
 * The page's URL policy and resolver (createSession `resources`; see
 * README "URL resources"). The default, no `resources`, is deny-all: no URL
 * is ever resolved and every URL load keeps its typed error.
 *
 * At createSession the host collects every statically known URL of the IR
 * (`<data src>`, unpinned `<script src>`, `<invoke src>`; any number of
 * them, each at most MAX_RESOURCE_URL_BYTES, 16384 bytes), asks `allow(url)` for each
 * (synchronously, once), and calls `resolve(url)` only for URLs `allow`
 * answered exactly `true`, all before the machine runs. The machine's
 * synchronous loads are then served from that cache:
 * - `<data src>`, `<invoke src>`: the text (and the media type of a
 *   LoadedDocumentAnswer), of any size, as under Wasmtime. A denied, unresolved (a rejected
 *   promise, or neither a string nor a LoadedDocumentAnswer), or over-long
 *   URL is `unavailable` (detail
 *   RESOURCE_DETAILS.*), which the machine raises as error.execution (for
 *   `<invoke>`, the invocation fails). An `<invoke src>` document is passed
 *   to `capabilities.compileInvocation(sourceName, source)` unchanged.
 * - `<script src>` (loaded while createSession builds the session): text
 *   in the ECMAScript data model's language, of any size the data model
 *   accepts (the host sets no bound, as under Wasmtime), any number of
 *   distinct URIs per session, run through the data model's `execute`.
 *   A denied URL, or `resolve` rejecting with `CapabilityError('unavailable')`,
 *   is `unavailable`; any other rejection `platform`; empty text
 *   `payload-too-large`; a session without the ECMAScript data model
 *   `invalid-input`: each rejects the document (ScxmlSessionError).
 * The resolved text one session holds is not bounded in total, as under
 * Wasmtime. A URL computed at run
 * time (`srcexpr`) is not prefetched: `capabilities.loadDocument` answers
 * it when present, otherwise it is `unavailable` with detail
 * UNSUPPORTED.dynamicUrl. `allow` must be synchronous and pure; a throw
 * denies. URLs are never telemetry attributes.
 *
 * `allow` and `resolve` get an `http:`/`https:` URL in its canonical form
 * (`new URL(url).href`, the form `fetch` reaches): dot segments (`..`,
 * `%2e%2e`, backslash-separated) are already resolved, so compare the
 * parsed origin and pathname rather than a raw prefix. One that carries
 * userinfo is denied without reaching `allow`. Any other URL (a relative
 * name, another scheme) reaches `allow` exactly as written: the host never
 * fetches it, and `new URL(url)` in a policy rejects a relative name. A `fetch` in `resolve` should pass
 * `{credentials: 'omit', redirect: 'error'}`.
 */
export interface ResourceProviders {
  allow(url: string): boolean;
  resolve(url: string): Promise<string | LoadedDocumentAnswer>;
}

/**
 * A resolved document with the media type its source indicated (for
 * example `response.headers.get('content-type')`). The machine reads a JSON
 * (`application/json`, `+json`) or XML (`application/xml`, `text/xml`,
 * `+xml`) indication for `<data src>`, trying that interpretation first
 * (SCXML B.2.1); any other type, `text/plain` included, is read by content.
 * A `mediaType` that is not visible ASCII is dropped; one of any length is
 * passed on, and the machine component reads one of at most 256 bytes, a
 * longer one indicating nothing.
 * A script resource's media type is ignored.
 */
export interface LoadedDocumentAnswer {
  text: string;
  mediaType?: string;
}


export interface CapabilityProviders {
  log?(record: { label: string | undefined; text: string }): void;
  loadDocument?(kind: 'data' | 'invocation', uri: string): string | LoadedDocumentAnswer;
  compileInvocation?(sourceName: string, source: string): Uint8Array;
  raise?(event: WitEvent): void;
  executeExtension?(element: ExecutableContentExtension): { tag: 'handled' } | { tag: 'not-handled' };
  loadScript?(reference: ScriptReference): void;
  runScript?(reference: ScriptReference, event: WitEvent | undefined): void;
}

/**
 * OpenTelemetry providers for one adapter (OpenTelemetry JS API objects).
 * Omitted providers are the global ones (`trace.getTracerProvider()`,
 * `metrics.getMeterProvider()`), which stay no-op until the page registers
 * an SDK; the host installs no SDK, propagator, or exporter. Only closed
 * labels and numbers are recorded (see README "Telemetry cardinality").
 * Provider failures are contained and never change host behavior.
 */
export interface TelemetryOption {
  tracerProvider?: TracerProvider;
  meterProvider?: MeterProvider;
}

export type Component =
  | 'machine'
  | 'data-model-ecmascript'
  | 'data-model-xpath'
  | 'data-model-null'
  | 'ioprocessor-scxml'
  | 'ioprocessor-basichttp'
  | 'clock'
  | 'compiler'
  | 'snapshot-codec';

export type CoreModuleLoader = (component: Component, file: string) => Promise<WebAssembly.Module>;

/** How delayed sends are delivered (`timers` option of createSession). */
export type TimerMode = 'event-loop' | 'manual';

/** One `snapshot({exclude})` section (a member of SNAPSHOT_SECTIONS). */
export type SnapshotSection =
  | 'system'
  | 'data-model'
  | 'queues'
  | 'pending-sends'
  | 'transitions'
  | 'previews'
  | 'invocations';

/** How `createSession` loads the snapshot codec (the `snapshot` option). */
export type SnapshotMode = 'lazy' | 'none';

/** Trace-carrier policy of one snapshot document. */
export type TraceContextPolicy = 'strip' | 'preserve';

/** Options of `MachineSession.snapshot`. */
export interface SnapshotOptions {
  /** Sections to omit (default: none). */
  exclude?: SnapshotSection[];
  /** Trace-carrier policy (default 'strip'). */
  traceContext?: TraceContextPolicy;
}

/**
 * Closed session snapshot failure code: the `scxml-snapshot` crate's
 * `SnapshotErrorCode` wire values, in SNAPSHOT_ERROR_CODES order
 * (docs/session-snapshot.md, "Error codes").
 */
export type SnapshotErrorCode =
  | 'too-large'
  | 'malformed-json'
  | 'unsupported-version'
  | 'schema-violation'
  | 'non-canonical'
  | 'integrity-mismatch'
  | 'bounds-exceeded'
  | 'invalid-name'
  | 'inconsistent'
  | 'depth-exceeded'
  | 'clock-invariant'
  | 'not-stable'
  | 'status-invalid'
  | 'codec-unavailable';

/**
 * One recorded observation (`trace: true`), attributed to the session that
 * produced it: `session` is its `_sessionid`, `parent` the invoking
 * session's (absent for the root).
 */
export type TraceEntry = { session: string; parent?: string } & (
  | { observation: 'command' }
  | { observation: 'raised'; name: string }
  | { observation: 'processed'; name: string }
  | { observation: 'log'; label: string | undefined; text: string }
);

/** The result of delivering due delayed sends. */
export interface DueDelivery {
  /** The call result of every delivered event, in delivery order. */
  results: CallResult[];
  /**
   * False when delivery stopped early: a call stopped at its command bound
   * (call `resume` first) or MAX_DUE_DELIVERIES were delivered; the rest
   * stay pending.
   */
  complete: boolean;
}

export type SchemaPolicy = 'strict' | 'lax' | 'off';
export type CompileStatus = 'emitted' | 'rejected';

/** Largest Machine IR `createSession` accepts (the machine's own bound). */
export const MAX_MACHINE_IR_BYTES: number;
/** Largest canonical compiler report decoded for the integrity check. */
export const MAX_REPORT_BYTES: number;
/** Concurrently open guest spans per telemetry adapter; beyond it `start` returns id 0. */
export const MAX_OPEN_SPANS: number;
/** `_sessionid` used when neither the caller nor the data model names one. */
export const DEFAULT_SESSION_ID: string;
/**
 * Unix epoch ms of logical time 0 of a `manual`-timer session's clock
 * (2015-09-01T00:00:00Z); an `event-loop` session's clock starts at the
 * wall-clock time it was created. ECMAScript `Date` reads the start plus
 * the session's logical time.
 */
export const BATCH_START_EPOCH_MS: number;
/**
 * Default fuel of one host call (75 000 000 charges), shared by every
 * component it reaches; also the default per-call cap of the bundled data
 * models. Charges are the browser's own unit (function entries and loop
 * iterations), not Wasmtime instructions.
 */
export const DEFAULT_FUEL: number;
/** Default per-compile cap (inside a session, also bounded by the host call's budget). */
export const DEFAULT_COMPILE_FUEL: number;
/** Largest accepted `fuel` option. */
export const MAX_FUEL: number;
/** Export name of the fuel global every metered core module carries. */
export const FUEL_EXPORT: string;
/**
 * Largest URL `resources` is asked about, in UTF-8 bytes (16384, the
 * script URI bound); a longer one is `unavailable` (RESOURCE_DETAILS.limit).
 */
export const MAX_RESOURCE_URL_BYTES: number;
/** `component-error.detail` values of `resources` failures. */
export const RESOURCE_DETAILS: Readonly<{
  denied: 'resource-denied';
  unresolved: 'resource-unresolved';
  limit: 'resource-limit';
}>;
/**
 * Most deliveries (delayed sends and cross-session messages) one command,
 * `advanceTime` call, or timer turn performs (10000). A scheduling slice,
 * not a resource limit: the rest stay pending for the next call and are
 * never dropped or failed.
 */
export const MAX_DUE_DELIVERIES: number;
/**
 * Invoke limits the page chooses for one root session: the deepest nested
 * invoke depth (a session the root invokes is at depth 1) and the most
 * invoked sessions under the root session at once. An invocation beyond them
 * fails with error.execution in its invoking session. An absent field sets
 * no limit, and the host imposes none of its own, as the Wasmtime host's
 * `InvokeLimits`. The value must be a plain object with no other key and
 * data properties only (a misspelled key, a getter, an array, or another
 * kind of object is `invalid-argument`), so a typo never removes a limit.
 * Each property is read once, when the session is created; the session
 * keeps its own copy.
 */
export interface InvokeLimits {
  /** Deepest nested invoke depth (a session the root invokes is at depth 1), a non-negative integer; 0 lets no session invoke. */
  maxInvokeDepth?: number;
  /** Most invoked sessions alive under the root session at once (the root not counted), a non-negative integer; 0 lets no session invoke. */
  maxInvokedSessions?: number;
}
export const TIMER_MODES: readonly TimerMode[];
/** Closed `snapshot({exclude})` sections, in document order. */
export const SNAPSHOT_SECTIONS: readonly SnapshotSection[];
/** Closed `createSession({snapshot})` values. */
export const SNAPSHOT_MODES: readonly SnapshotMode[];
/** Closed `snapshot({traceContext})` values. */
export const TRACE_CONTEXT_POLICIES: readonly TraceContextPolicy[];
/** Largest snapshot or diff document, in bytes (32 MiB; the crate's bound). */
export const MAX_SNAPSHOT_BYTES: number;
/** Per-call fuel budget of the codec component. */
export const DEFAULT_SNAPSHOT_FUEL: number;
/** Closed session snapshot failure codes (the crate's wire values). */
export const SNAPSHOT_ERROR_CODES: readonly SnapshotErrorCode[];
/**
 * `component-error.detail` of the deliberately unsupported capabilities:
 * `invoke` ('unsupported-invoke', under `invoke: { compile: 'provided' }`
 * without `compileInvocation`: the invocation fails with error.execution)
 * and `dynamicUrl`
 * ('dynamic-url-unavailable': with `resources`, a runtime-computed URL no
 * provider answers; error.execution).
 */
export const UNSUPPORTED: Readonly<{
  invoke: 'unsupported-invoke';
  dynamicUrl: 'dynamic-url-unavailable';
}>;
export const COMPONENTS: readonly Component[];
export const ERROR_CODES: readonly ErrorCode[];
export const HOST_ERROR_CODES: readonly HostErrorCode[];
export const COMPILER_ERROR_CODES: readonly CompilerErrorCode[];
export const STATUSES: readonly Status[];
export const SCHEMA_POLICIES: readonly SchemaPolicy[];
export const COMPILE_STATUSES: readonly CompileStatus[];

/**
 * One machine session bound to one machine instance, with its own SCXML
 * Event I/O Processor and clock. Every command runs on one fuel budget,
 * shared by every component the command reaches.
 * Command failures throw ScxmlSessionError; a trap throws
 * ScxmlHostError('instance-lost' | 'fuel-exhausted') and a broken I/O
 * processor contract 'host-invariant', after which every call throws
 * 'instance-lost'.
 *
 * Delayed sends: the processor schedules each `<send delay>` on the
 * session's logical clock (milliseconds). When its deadline is reached the
 * host routes it through the processor again (`route-due`) and delivers it
 * as an external event, in clock order. In `event-loop` mode a browser
 * timer does this (logical time follows the `now` clock, never passing an
 * undelivered deadline) and reports each delivery to `onTimer`; in `manual`
 * mode time moves only through `advanceTime`. Nothing is delivered while
 * the session is not `running` or its last call did not end stable.
 *
 * Invoked children: each child has its own
 * machine, data model, I/O processor, and clock (advanced with the root's).
 * After every command, and in every delivery pass, the host moves events
 * between the sessions in the Wasmtime scheduler's order: root events to
 * children, children's events and then their `done.invoke` to the root, one
 * delivery at a time. A command returns the root's last call result after that
 * work settled. A child failure, trap, or command-bound stop fails the whole
 * session (the child's ScxmlSessionError or ScxmlHostError), as it fails the
 * Wasmtime run.
 *
 * Run to completion: one operation at a time. While a method runs
 * (including a timer-driven delivery), any MachineSession method a
 * capability provider or data model calls throws
 * ScxmlHostError('reentrant-call'), except the BasicHTTP loopback (see
 * `receiveHttp`); `onTimer` runs after its delivery and may call the
 * session. A method never returns success once the session is
 * lost: it throws the fault that lost it (or 'instance-lost').
 */
export class MachineSession {
  private constructor();
  /** Enters the initial configuration and runs to a stable configuration (status `ready`). */
  start(): CallResult;
  /**
   * Processes one external event and runs to a stable configuration (status `running`,
   * previous call ended stable). A malformed `event` throws
   * ScxmlHostError('invalid-argument') without entering the machine.
   */
  send(event: EventInput): CallResult;
  /**
   * Injects one inbound BasicHTTP request addressed, by its location in
   * canonical form, to this (root) session or one of its live, unfinished
   * invoked sessions at any depth (SCXML C.2); requires the `http` option. The BasicHTTP component
   * validates and decodes it as under Wasmtime (event name: the first
   * `_scxmleventname` form parameter, else header, else `HTTP.POST`; data:
   * the other form parameters, or the body text; `_event.raw`: the message
   * as received; `_event.origintype`: the BasicHTTP type URI), the session
   * processes the event as an external event, and the component's response
   * is returned: 200 once enqueued; 400, 405 (`Allow: POST`), or 413 for a
   * rejected message; 404 when `url` is neither the root's location nor
   * that of a live, unfinished invoked session at any depth (as under
   * Wasmtime), or the session is not running; 503 while the last call
   * stopped at its command bound. `url` defaults to the root's location.
   *
   * Called from the page's `http.send` while a session's BasicHTTP send
   * runs (the in-process loopback), the first such request is delivered as
   * under Wasmtime, by location in its canonical form (src/url.js): to the
   * sending session (enqueued before the `<send>` returns), its parent, or
   * one of its live children;
   * any other call during a session operation throws 'reentrant-call'.
   *
   * Throws ScxmlHostError 'invalid-argument' (a malformed request, or no
   * `http` option), 'reentrant-call', 'instance-lost', or 'fuel-exhausted';
   * ScxmlSessionError when the machine rejects or fails the event.
   */
  receiveHttp(request: InboundHttpRequest): Required<HttpResponse>;
  /** Continues a machine whose previous call stopped at its command bound. */
  resume(): CallResult;
  /** Active leaf states in document order. */
  configuration(): string[];
  status(): Status;
  /** The closed cause of a failed session, if any. */
  failure(): ComponentErrorRecord | undefined;
  /** Current logical time of the session's clock, in milliseconds. */
  now(): number;
  /** Deadline of the earliest pending delayed send, if any (logical ms). */
  nextDeadline(): number | undefined;
  /**
   * `manual` timers only: advances logical time by `ms` (a non-negative
   * safe integer) and delivers every delayed send that falls due, in clock
   * order. Otherwise throws ScxmlHostError('invalid-argument').
   */
  advanceTime(ms: number): DueDelivery;
  /** Engine commands this session and its children completed (charged against `maxCommands`). */
  commands(): number;
  /** The recorded observations of the session and its children (`trace: true`), in order. */
  trace(): { entries: TraceEntry[] };
  /** Releases the engine and moves to `shut-down`; idempotent. */
  shutdown(): void;
  /** Shuts down (when usable) and drops the instance; later calls: 'instance-lost'. */
  dispose(): void;
  [Symbol.dispose](): void;
  /**
   * Read-only canonical snapshot (`scxml.session-snapshot/v2`) of the root
   * session and every session it invoked, at a stable checkpoint
   * (docs/session-snapshot.md).
   * Gathers the components' read-only `snapshot` exports and every
   * session's external event queue, then encodes through the lazily loaded codec component;
   * nothing is executed, evaluated, or changed, and nothing is ever
   * restored.
   *
   * Throws ScxmlSnapshotError (see SNAPSHOT_ERROR_CODES): 'status-invalid'
   * for a failed or shut-down session; 'not-stable' during another
   * operation, after a call that did not end stable, or while deliveries are pending;
   * 'codec-unavailable' when the codec component is missing, the session
   * was created with `snapshot: 'none'`, or the codec answers `unavailable`;
   * 'clock-invariant' when a child's clock differs from the root's;
   * 'bounds-exceeded' with children requested and more than 255 invoked
   * sessions, else 'depth-exceeded' with one deeper than 32 (checked
   * before any session is read, at any depth); 'too-large' when the
   * document is over 32 MiB; or another encoder code.
   * Throws ScxmlSessionError when a component's read-only `snapshot` export
   * rejects (for example a page data model without `snapshot()` while
   * `system` or `datamodel` is requested); the session is unaffected.
   * Throws ScxmlHostError: 'invalid-argument' for malformed options;
   * 'instance-lost' when the session's instance is already lost;
   * 'fuel-exhausted' or 'instance-lost' when the codec traps; and
   * 'host-invariant' when the codec answers with an unknown error detail or
   * a result that is not bytes. A codec failure leaves the session
   * unaffected.
   */
  snapshot(options?: SnapshotOptions): Promise<Uint8Array>;
}

/**
 * Encodes the canonical `scxml.session-snapshot-diff/v2` document of two
 * valid snapshot documents, byte-identical to `scxml snapshot diff --format
 * json`.
 *
 * Throws ScxmlSnapshotError: 'too-large' when either input (checked
 * before any codec call) or the diff is over 32 MiB; 'codec-unavailable'
 * when the codec component is missing or answers `unavailable`; or the
 * decoder's code for an invalid input document (see SNAPSHOT_ERROR_CODES).
 * Throws ScxmlHostError:
 * 'invalid-argument' for malformed arguments; 'fuel-exhausted' or
 * 'instance-lost' when the codec traps; and 'host-invariant' when the
 * codec answers with an unknown error detail or a result that is not
 * bytes.
 */
export function diffSnapshots(
  before: Uint8Array,
  after: Uint8Array,
  options?: {
    /** Compare trace carriers too (default false). */
    includeTraceContext?: boolean;
    telemetry?: TelemetryOption;
    loadCoreModule?: CoreModuleLoader;
  },
): Promise<Uint8Array>;

// --- Async sessions (src/async-bridge.js) ------------------------------------

/** Closed values of createSession's `async` option. */
export type AsyncMode = 'off' | 'auto';
/** How an `async: 'auto'` session waits on the page (see asyncMechanism). */
export type AsyncMechanism = 'worker' | 'none';
export const ASYNC_MODES: readonly AsyncMode[];
export const ASYNC_MECHANISMS: readonly AsyncMechanism[];
/**
 * The mechanism an `async: 'auto'` session gets in `scope` (default this
 * realm): 'worker' when module Workers, growable SharedArrayBuffers, and
 * `Atomics.wait` exist and the page is cross-origin isolated (COOP
 * `same-origin` and COEP `require-corp`); otherwise 'none'.
 */
export function asyncMechanism(scope?: object): AsyncMechanism;

/**
 * Page providers of an async session: the CapabilityProviders contract.
 * Under the 'worker' mechanism each may also answer with a Promise, which
 * the blocked worker waits on; under 'none' providers run synchronously in
 * the page, so a Promise answer is malformed (`platform`). A page that must
 * run under both answers synchronously.
 */
export type AsyncCapabilityProviders = {
  [K in keyof CapabilityProviders]?: CapabilityProviders[K] extends ((...args: infer A) => infer R) | undefined
    ? (...args: A) => R | Promise<R>
    : never;
};

/**
 * A session created with `async: 'auto'`: the MachineSession methods and
 * errors, each answered through a Promise.
 *
 * `mechanism` 'worker': the unchanged synchronous session runs in a
 * dedicated module Worker. A capability call that needs the page blocks
 * the worker (`Atomics.wait`) until the page's provider or
 * `resources.resolve` settles, so a runtime-computed URL (`<invoke
 * srcexpr>`, a `<data src>` the page does not supply through
 * `loadDocument`) loads through `resources`: `allow(url)` must return true,
 * then `resolve(url)`'s answer, text or `{text, mediaType}`, is the
 * document (both get the URL in the form static URLs use).
 * A denied URL, a rejected `resolve`, or an answer of any other shape is
 * `unavailable` (error.execution), as for a prefetched URL.
 * Runtime URLs have the prefetch bound: a URL over 16384 bytes is
 * `unavailable`; neither the runtime URLs one session resolves nor their
 * total text are bounded. Core modules come from
 * the page's `loadCoreModule`. The worker's own spans go to the worker's
 * global OpenTelemetry API; the page records `create-session` and one
 * `capability` span per call it serves.
 *
 * `mechanism` 'none': the synchronous session runs in the page, and a URL
 * the page does not supply fails with `unavailable` and detail
 * UNSUPPORTED.dynamicUrl (the machine raises error.execution).
 *
 * One operation at a time: a call before the previous one settled, or
 * from a provider the session is waiting on, rejects with
 * ScxmlHostError('reentrant-call').
 */
export class AsyncMachineSession {
  private constructor();
  readonly mechanism: AsyncMechanism;
  start(): Promise<CallResult>;
  send(event: EventInput): Promise<CallResult>;
  resume(): Promise<CallResult>;
  configuration(): Promise<string[]>;
  status(): Promise<Status>;
  failure(): Promise<ComponentErrorRecord | undefined>;
  now(): Promise<number>;
  nextDeadline(): Promise<number | undefined>;
  advanceTime(ms: number): Promise<DueDelivery>;
  commands(): Promise<number>;
  trace(): Promise<{ entries: TraceEntry[] }>;
  shutdown(): Promise<void>;
  snapshot(options?: SnapshotOptions): Promise<Uint8Array>;
  /**
   * Shuts down and releases the instance (and terminates the worker);
   * idempotent. It never waits on a page operation the worker is blocked
   * on, nor longer than 1000 ms for the worker to release its engine.
   */
  dispose(): Promise<void>;
  [Symbol.asyncDispose](): Promise<void>;
}

/**
 * Creates an async session (`async: 'auto'`). Options are those of the
 * synchronous createSession, except: `dataModel` names a bundled data
 * model; `now` and `http` are not accepted (a worker cannot call the page's
 * BasicHTTP providers); `capabilities` may answer with Promises; and
 * `resources` also resolves runtime-computed URLs. Static URLs are
 * prefetched in the page exactly as for a synchronous session, under both
 * mechanisms, and a worker session is handed those answers. The
 * validation is the same for every mechanism. Throws as createSession, and
 * ScxmlHostError('invalid-argument') for an option that is not accepted.
 */
export function createSession(options: {
  ir: Uint8Array;
  async: 'auto';
  dataModel?: 'ecmascript' | 'xpath' | 'null';
  capabilities?: AsyncCapabilityProviders;
  resources?: ResourceProviders;
  invoke?: InvokeOptions;
  /** Invoke limits the page chooses (default: no limit). */
  invokeLimits?: InvokeLimits;
  telemetry?: TelemetryOption;
  sessionId?: string;
  timers?: TimerMode;
  onTimer?(outcome: DueDelivery | { error: unknown }): void;
  maxCommands?: number;
  trace?: boolean;
  loadCoreModule?: CoreModuleLoader;
  snapshot?: SnapshotMode;
  fuel?: number;
  /**
   * The module Worker entry (default this package's `src/async-worker.js`,
   * which bundlers resolve). Import maps do not apply inside workers, so an
   * unbundled page passes an entry whose `@opentelemetry/api` import is
   * resolved (for example the bundled `src/async-worker.js`).
   */
  workerUrl?: string | URL;
}): Promise<AsyncMachineSession>;

/**
 * Creates one runtime-IR session on a fresh machine instance, linked to a
 * fresh SCXML Event I/O Processor and clock.
 *
 * `ir` is untrusted lowered Machine IR (validated by the machine; the host
 * reads it only for the `<invoke>` pre-warm hint, see InvokeOptions).
 * `dataModel` must be the data model the document
 * names (the host does not choose one); a document that needs one and has
 * none fails `start` with `unavailable`.
 *
 * Throws ScxmlHostError 'invalid-argument', 'machine-ir-too-large',
 * 'fuel-exhausted', or 'instance-lost'; ScxmlSessionError (sessionFailed)
 * when the machine rejects the IR or session id, or when loading an
 * external `<script src>` fails (see ResourceProviders and
 * CapabilityProviders `loadScript`). With `resources`, it resolves every
 * allowed static URL first, so it settles only after `resolve` does.
 */
export function createSession(options: {
  ir: Uint8Array;
  dataModel?: DataModelProvider;
  capabilities?: CapabilityProviders;
  /**
   * The page's URL allowlist and resolver (default none: deny-all). Any
   * other own key, or a missing `allow` or `resolve`, is 'invalid-argument'.
   */
  resources?: ResourceProviders;
  telemetry?: TelemetryOption;
  /** `_sessionid` (default DEFAULT_SESSION_ID). */
  sessionId?: string;
  /** How delayed sends are delivered (default 'event-loop'). */
  timers?: TimerMode;
  /**
   * Called after each asynchronous delivery with its result or the error
   * that stopped it: an 'event-loop' timer turn, or (in either mode) the
   * later `error.communication` of a fire-and-forget BasicHTTP send that
   * failed. Its own exceptions are contained. It runs outside the delivery,
   * so it may call the session.
   */
  onTimer?(outcome: DueDelivery | { error: unknown }): void;
  /**
   * The BasicHTTP capability (see HttpCapability). Without it every
   * BasicHTTP send raises `error.communication`, `_ioprocessors` does not
   * list the processor, and `receiveHttp` throws 'invalid-argument'.
   */
  http?: HttpCapability;
  /** 'event-loop' mode: a monotonic millisecond clock (default `performance.now`). */
  now?(): number;
  /**
   * Session-wide engine command budget (a positive safe integer; default
   * none). A command beyond it fails the session with `capacity-exhausted`.
   */
  maxCommands?: number;
  /** Record the observation trace (default false). */
  trace?: boolean;
  loadCoreModule?: CoreModuleLoader;
  /**
   * Whether the session may load the snapshot codec component on first use
   * (default 'lazy'); 'none' answers every `snapshot()` with
   * `codec-unavailable` and never loads it.
   */
  snapshot?: SnapshotMode;
  /**
   * Fuel of one host call, shared by every component the call reaches (the
   * machine, the data model, the I/O processor and clock, BasicHTTP, the
   * `<invoke>` compiler, and each invoked child's instances), and the
   * per-call cap of every instance the session creates; a positive safe
   * integer (default DEFAULT_FUEL).
   */
  fuel?: number;
  /** How `<invoke>` children are compiled (default `{ compile: 'auto' }`). */
  invoke?: InvokeOptions;
  /** Invoke limits the page chooses (default: no limit). */
  invokeLimits?: InvokeLimits;
  /** 'off' (default): this synchronous session. See the `async: 'auto'` overload. */
  async?: 'off';
}): Promise<MachineSession>;

// BasicHTTP (SCXML C.2). The bundled BasicHTTP Event I/O Processor
// component encodes, interprets, and decodes every message; these are the
// shapes the page exchanges with it.

/**
 * One outgoing BasicHTTP request, as the component encoded it: `method`
 * `POST`, `headers` `{'Content-Type': 'application/x-www-form-urlencoded'}`,
 * and `body` the form: `_scxmleventname=<event>` first, then each
 * `namelist` entry and `<param>` in document order (or the `<content>` as one
 * bare segment), every byte outside RFC 3986 unreserved percent-encoded in
 * uppercase hex.
 */
export interface HttpRequest {
  method: string;
  url: string;
  headers: Record<string, string>;
  body: string;
}

/**
 * An HTTP response. From `http.send`, only `status` is required (2xx
 * accepts the send); from `receiveHttp`, every field is present.
 */
export interface HttpResponse {
  status: number;
  headers?: Record<string, string>;
  body?: string;
}

/** One inbound request for `receiveHttp`. */
export interface InboundHttpRequest {
  method: string;
  headers?: Record<string, string> | Iterable<[string, string]>;
  body?: string | Uint8Array;
  /** The addressed location (default: the root's location). */
  url?: string;
}

/**
 * The BasicHTTP capability of a session and its invoked children.
 *
 * Security: a document reaches the network only through `allow`. A request
 * reaches `send` only when `allow(url)` returns exactly `true`; any other
 * result, or a throw, denies it (`error.communication`, nothing sent).
 * `allow`, `send`, and the default fetch get the target's canonical form
 * (`new URL(target).href`): dot segments are already resolved, so compare
 * the parsed origin and pathname rather than a raw prefix. A target that
 * carries userinfo is denied without reaching `allow`.
 */
export interface HttpCapability {
  allow(url: string): boolean;
  /**
   * Performs one allowed request (default: `fetch` with `credentials:
   * 'omit'` and `redirect: 'error'`). A promise is fire-and-forget: the send
   * completes at once, and a rejection or a non-2xx or malformed response
   * delivers `error.communication` (`_event.type` `platform`,
   * `_event.sendid` the failed `<send>`'s id) on the sender's internal event
   * queue once the failure is known (SCXML 6.2.4: the Processor "MUST place
   * the error error.communication on the internal event queue"), reported
   * through `onTimer`. A response returned directly is interpreted inside
   * the send, as under Wasmtime: a non-2xx or malformed one, or a throw,
   * raises the platform `error.communication`.
   */
  send?(request: HttpRequest): Promise<HttpResponse> | HttpResponse;
  /**
   * The BasicHTTP location `_ioprocessors` lists: a string is the root's; a
   * function answers for every session (root and invoked children) by
   * `_sessionid`. A session for which neither gives one (no `location`,
   * an invoked child under a string, or a function returning `undefined`)
   * is listed at `http://basichttp.loopback.invalid/sessions/<id>/basichttp`
   * (SCXML C.2.3: every entry has a `location`). A send to a URL under
   * `http://basichttp.loopback.invalid` is delivered in-process to the live
   * session at that location, as under Wasmtime, and never reaches `allow`
   * or `send`; one no live session answers raises `error.communication`.
   *
   * A function that throws fails the call that filled the session's
   * `_ioprocessors` (`ScxmlSessionError` `platform`) and makes
   * `receiveHttp` throw `invalid-argument` for that request; when a
   * loopback send is routed, the session answers no location (so a send
   * addressed to it raises `error.communication`), a failed `capability`
   * span is recorded once per session, and its location is read at most
   * once a turn.
   */
  location?: string | ((sessionId: string) => string | undefined);
}

/**
 * `<invoke>` child compilation (createSession `invoke`).
 *
 * `'auto'` (the default): when the root IR declares `<invoke>`, createSession
 * opens a dedicated bundled compiler instance for the session and pre-warms
 * it with every statically known child (an SCXML-typed `<invoke>` with
 * inline `<content>` and no `src`, `srcexpr`, `typeexpr`, or content
 * `expr`). The machine's `compile-invocation(sourceName, source)` is then
 * answered synchronously, with the Wasmtime host's request (lowered IR,
 * schema policy off, runtime script resolution) and outcome mapping:
 * - emitted IR starts the child;
 * - a document the compiler rejects (diagnostics, or `invalid-request`), or
 *   a source over 4194304 UTF-8 bytes, fails the invocation with
 *   error.execution in the invoking session (SCXML 6.4);
 * - any other compiler failure fails the session: the command throws
 *   ScxmlCompileError(code), or ScxmlHostError('fuel-exhausted' |
 *   'instance-lost' | 'report-mismatch') when the compiler traps, runs out
 *   of fuel, or breaks its report integrity.
 * Outcomes are cached per session by the exact `(sourceName, source)` pair
 * (at most 64 entries and 32 MiB, oldest evicted first), so a child invoked
 * again, or pre-warmed, is not compiled again. A page
 * `capabilities.compileInvocation` overrides this compiler.
 *
 * `'provided'`: only `capabilities.compileInvocation` compiles children;
 * without it `<invoke>` is the typed `unsupported-invoke` error.execution.
 *
 * Depth and breadth are the Wasmtime host's: invoked sessions invoke in
 * turn, within the limits the page set in `invokeLimits`, if any (an
 * invocation beyond them fails with error.execution).
 */
export interface InvokeOptions {
  compile?: 'auto' | 'provided';
  /**
   * Per-compile fuel cap, within the session's host-call budget; a positive
   * safe integer (default DEFAULT_COMPILE_FUEL).
   */
  fuel?: number;
}

/** Options of the bundled data-model factories. */
export interface DataModelOptions {
  telemetry?: TelemetryOption;
  loadCoreModule?: CoreModuleLoader;
  /**
   * Per-call fuel cap; a positive safe integer (default DEFAULT_FUEL). Used
   * without a session each call is a host call of its own; linked into a
   * session, each call is also charged to the session's host-call budget.
   */
  fuel?: number;
}

/**
 * Instantiates the bundled ECMAScript data-model component, uninitialized,
 * as a complete provider for one session (the machine initializes it from
 * `start`). Each call is capped by `fuel` and, inside a session, charged to
 * the budget of the session's host call that made it; a trap (fuel exhaustion
 * included) discards the instance: that call and every later fallible one
 * throw CapabilityError('platform'), and every later pure one answers
 * `undefined`. Malformed arguments throw CapabilityError('invalid-input')
 * (pure calls answer `undefined`). `acceptsEventData` answers `true` in both
 * cases (see {@link BundledDataModel}). The component's `raise`/`send`/`cancel`
 * imports fail closed with `unavailable` (effects are machine capabilities).
 *
 * Throws ScxmlHostError 'invalid-argument', 'fuel-exhausted', or
 * 'instance-lost'.
 */
export function createEcmaScriptDataModel(options?: DataModelOptions): Promise<BundledDataModel>;
/** The bundled XPath data-model component; otherwise as createEcmaScriptDataModel. */
export function createXPathDataModel(options?: DataModelOptions): Promise<BundledDataModel>;
/** The bundled null data-model component; otherwise as createEcmaScriptDataModel. */
export function createNullDataModel(options?: DataModelOptions): Promise<BundledDataModel>;

/**
 * Instantiates one compiler. Throws ScxmlHostError 'invalid-argument',
 * 'fuel-exhausted', or 'instance-lost'.
 */
export function createCompiler(options?: {
  telemetry?: TelemetryOption;
  loadCoreModule?: CoreModuleLoader;
  /** Per-compile fuel budget; a positive safe integer (default DEFAULT_COMPILE_FUEL). */
  fuel?: number;
}): Promise<{
  /**
   * Compiles one SCXML document to lowered Machine IR (`schemaPolicy`
   * defaults to 'off', `sourceName` to 'document.scxml'). `report` is the
   * canonical `scxml.report/v3` decision; `ir` is present exactly when
   * `status` is 'emitted' and matches the report's length and SHA-256.
   *
   * Throws ScxmlHostError 'invalid-argument' (the instance stays usable),
   * 'report-mismatch', 'fuel-exhausted', or 'instance-lost' (this and every
   * later compile); ScxmlCompileError for a rejected request.
   */
  compile(
    source: string,
    options?: { sourceName?: string; schemaPolicy?: SchemaPolicy },
  ): Promise<{ status: CompileStatus; report: Uint8Array; ir: Uint8Array | undefined }>;
}>;

/**
 * Fetch loader over `baseUrl` (default: this package's `dist/`). Throws
 * TypeError for a malformed `options.telemetry`.
 */
export function fetchCoreModules(
  baseUrl?: URL | string,
  options?: { telemetry?: TelemetryOption },
): CoreModuleLoader;
/**
 * Caching, name-validating loader over `source`; each request is one
 * `load-core-module` host operation. Throws TypeError for a malformed
 * `options.telemetry`.
 */
export function cachedCoreModules(
  source: (component: string, file: string) => WebAssembly.Module | BufferSource | Promise<WebAssembly.Module | BufferSource>,
  options?: { telemetry?: TelemetryOption },
): CoreModuleLoader;

export class ScxmlHostError extends Error {
  readonly code: HostErrorCode;
}
export class ScxmlSessionError extends Error {
  readonly code: ErrorCode;
  readonly detail?: string;
  readonly committedEffects: number;
  readonly sessionFailed: boolean;
}
/** A session snapshot or snapshot diff failed with a closed code. */
export class ScxmlSnapshotError extends Error {
  readonly code: SnapshotErrorCode;
}
export class ScxmlCompileError extends Error {
  readonly code: CompilerErrorCode;
}
export class CapabilityError extends Error {
  constructor(code: ErrorCode);
  readonly code: ErrorCode;
}
