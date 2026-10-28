//! The session capability boundary of [`super::Interpreter`].
//!
//! Every type here is the native mirror of one WIT
//! `machine-capability-types` item (`stateforward:scxml@0.2.0`); the machine
//! component implements [`SessionCapabilities`] one-to-one over its
//! `machine-capabilities` imports, and engine tests implement it with an
//! in-memory mock. The trait carries facts and data-model operations only:
//! every SCXML decision (evaluation order, error precedence, send-id
//! generation, invocation lifecycle) stays in the interpreter.

use scxml_compiler_core::{ScriptSourceIr, XmlNodeIr};
use scxml_types::{
    AssignPlacement, AssignSource, DataInitializer, DataValue, Event, ExtensionAttribute,
    FormValueSource, NamedValue, SendId, SendRequest, StateId, TraceContext,
};

use super::InterpretFailure;
use crate::BatchEnd;

/// Closed class of one capability failure, as the interpreter must treat it.
///
/// Only [`Self::InvalidInput`] and [`Self::Execution`] are recoverable SCXML
/// failures (`error.execution`, SCXML 5.10); every other class is fatal to
/// the session unless a capability documents otherwise.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityErrorKind {
    /// Malformed or rejected input (WIT `invalid-input`).
    InvalidInput,
    /// SCXML execution failed (WIT `execution`).
    Execution,
    /// A byte bound was exceeded (WIT `payload-too-large`).
    PayloadTooLarge,
    /// A capability or provider is unavailable (WIT `unavailable`).
    Unavailable,
    /// A bounded resource is full (WIT `capacity-exhausted`), for example
    /// a clock that holds as many delayed sends as it can.
    CapacityExhausted,
    /// Every other closed failure (WIT `duplicate-id`, `unknown-id`,
    /// `unsupported-version`, `protocol`, `platform`).
    Other,
}

impl CapabilityErrorKind {
    /// Whether SCXML error handling turns this failure into a recoverable
    /// `error.execution` (SCXML 5.10).
    #[must_use]
    pub const fn is_recoverable(self) -> bool {
        matches!(self, Self::InvalidInput | Self::Execution)
    }
}

/// How the host carries out one `start_invocation` or `cancel_invocation`
/// request (WIT `invocation-progress`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvocationProgress {
    /// The host carried the request out inside the call.
    Completed,
    /// The host carries the request out after the run suspends right after
    /// this effect, and hands its result back to continue the run.
    Deferred,
}

/// One trace observation reported to the host (WIT `observation`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Observation<'a> {
    /// One admitted engine command produced its result; reported before that
    /// command's effects run.
    CommandCompleted,
    /// One `<raise>` was applied; carries the raised event name.
    Raised(&'a str),
    /// One queued event was dequeued and processed by transition selection
    /// (selected or ignored, W3C Appendix D `mainEventLoop`); carries its
    /// name. Reported after the processing command's `CommandCompleted`,
    /// before that command's effects run.
    Processed(&'a str),
}

/// What the host did with one accepted `<send>` (WIT `send-disposition`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SendDisposition {
    /// The event is for the sending session's own external queue; the
    /// interpreter enqueues it (SCXML C.1 loopback). Boxed: the other
    /// dispositions carry nothing.
    EnqueueExternal(Box<Event>),
    /// The host routed the event to another session.
    Routed,
    /// The processor scheduled the event on the session's clock.
    Scheduled,
    /// No live session answers the target: the interpreter raises
    /// `error.communication` carrying the send id (SCXML 6.2, C.1).
    Undeliverable,
}

/// What executing one executable-content extension produced (WIT
/// `extension-execution`).
#[derive(Debug)]
pub struct ExtensionExecution<E> {
    /// Whether the extension component handled the element (`false`: declined,
    /// which is `error.execution`), or its failure (recoverable ones are
    /// `error.execution`, every other one is fatal).
    pub outcome: Result<bool, E>,
    /// Internal events the extension component queued, in order; the
    /// interpreter appends them to the internal queue whatever the outcome.
    pub internal: Vec<Event>,
    /// Self-addressed external events the extension component sent, in order;
    /// the interpreter appends them to the external queue whatever the outcome.
    pub external: Vec<Event>,
}

/// The machine's executable-content image handed to the extension component's
/// host with one executable-content extension (WIT `execute-extension`'s
/// `snapshot`): the engine's [`crate::Engine::effect_snapshot`] and the session
/// state that holds the send and invoke counters and the invoking parent.
#[derive(Debug)]
pub struct ExtensionSnapshot<'a> {
    /// The engine image at the executing effect.
    pub image: crate::EngineSnapshot,
    /// The executing session's state.
    pub session: &'a super::SessionState,
}

/// Delivery class of one SCXML Event I/O Processor target (WIT
/// `send-target`, SCXML 6.2 and C.1). Decided only by the processor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SendTarget {
    /// `#_internal`: the sender's internal queue.
    Internal,
    /// Absent, or the sender's own `#_scxml_sessionid`.
    Local,
    /// `#_parent` or the parent's session address.
    Parent,
    /// A live invocation of the sender, by invoke id.
    Invocation(String),
    /// Any other session of the sender's run, by `_sessionid`.
    Session(String),
    /// A well-formed session address no live session answers
    /// (`error.communication`).
    Unreachable,
    /// Any other target form (`error.execution`).
    Invalid,
}

/// One evaluated `<send>` for an Event I/O Processor other than the SCXML
/// one, such as `BasicHTTP` (SCXML C.2) or a processor a package binds (WIT
/// `processor-send`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessorSend {
    /// Evaluated `type`/`typeexpr`, exactly as written; the host routes the
    /// send by it.
    pub processor_type: String,
    /// The send's id: the authored `id`, else the platform-generated one
    /// (SCXML 3.14, 6.2.4). It never enters the message; the host
    /// keeps it so a failure it learns of after the `<send>` returned
    /// carries it as `_event.sendid` (SCXML 5.10.1).
    pub send_id: SendId,
    /// Evaluated `event`/`eventexpr`; optional (a `BasicHTTP` receiver
    /// then uses `HTTP.POST`).
    pub event_name: Option<String>,
    /// Evaluated `target`/`targetexpr`.
    pub target: Option<String>,
    /// `namelist` entries then `<param>` children, in document order, as
    /// data-model form values.
    pub params: Vec<NamedValue>,
    /// Evaluated `<content>`; mutually exclusive with `params`.
    pub content: Option<DataValue>,
    /// Delay in whole milliseconds.
    pub delay_ms: u64,
    /// Trace carrier of the triggering batch; never changes semantics.
    pub trace_context: Option<TraceContext>,
}

/// Kind of one document the interpreter asks the host to load (WIT
/// `document-kind`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentKind {
    /// An external `<data src>` document (SCXML 5.3).
    Data,
    /// An `<invoke src>` child document (SCXML 6.4).
    Invocation,
}

/// One document the host loaded (WIT `loaded-document`): its text and the
/// media type its source indicated, if any (SCXML B.2.1).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedDocument {
    /// The document text exactly as the resource holds it.
    pub text: String,
    /// The indicated media type (an HTTP `Content-Type` value, or one an
    /// embedder derives), untrusted: data binding reads only a well-formed
    /// one.
    pub media_type: Option<String>,
}

/// Inline `<content>` or `<data>` children (WIT `inline-body`, SCXML 5.6,
/// 6.2.6).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InlineBody {
    /// Text-only children, concatenated in document order.
    Text(String),
    /// Children containing an element, as standalone namespace-correct
    /// markup.
    Markup(String),
}

/// Result of evaluating a `<foreach array>` expression (WIT
/// `foreach-result`, SCXML 4.6).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ForeachItems {
    /// Shallow snapshot of the collection's items in iteration order.
    Items(Vec<DataValue>),
    /// The value is not a legal collection (`error.execution`).
    Illegal,
}

/// One invoked SCXML session the interpreter asks the host to start (WIT
/// `invoked-session`, SCXML 6.4).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvokedSession {
    /// Interpreter-owned invoke id. The host names the invoked session
    /// itself (its `_sessionid`, unique across the run).
    pub invoke_id: String,
    /// Child Machine IR bytes produced by
    /// [`SessionCapabilities::compile_invocation`].
    pub machine_ir: Vec<u8>,
    /// The child document's `datamodel`.
    pub data_model: Option<String>,
    /// `<param>` and `namelist` entries evaluated in the parent (SCXML
    /// 6.4.2), in document order.
    pub parameters: Vec<NamedValue>,
    /// Carrier of the invoking effect batch: the child's start batch is its
    /// child. Never changes semantics.
    pub trace_context: Option<TraceContext>,
}

/// One evaluated `<invoke>` of an invoked service the interpreter asks the
/// host to start (WIT `invoke-request`, SCXML 6.4).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvokeRequest {
    /// Interpreter-owned invoke id.
    pub invoke_id: String,
    /// The `<invoke>`'s type (or evaluated `typeexpr`) exactly as authored
    /// or evaluated; the host routes by it byte for byte.
    pub type_uri: String,
    /// Evaluated `src`/`srcexpr` text: data for the service, never loaded.
    pub source: Option<String>,
    /// Evaluated `<content>` value.
    pub content: Option<DataValue>,
    /// `<param>` and `namelist` entries evaluated in the invoking session
    /// (SCXML 6.4.2), in document order.
    pub parameters: Vec<NamedValue>,
    /// The `<invoke>`'s `env` attributes, sorted by slot: `env:<slot>` with
    /// the name of the variable whose value the host binds to the slot as
    /// an opaque reference for the whole invocation (`env:<slot>expr`
    /// evaluated when the `<invoke>` ran). Never a value.
    pub attributes: Vec<ExtensionAttribute>,
    /// Carrier of the invoking effect batch. Never changes semantics.
    pub trace_context: Option<TraceContext>,
}

/// What one `<invoke>` asks the host to start (WIT `invocation`, SCXML
/// 6.4).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Invocation {
    /// An invoked SCXML session.
    Session(InvokedSession),
    /// An invoked service.
    Service(InvokeRequest),
}

/// Capabilities one interpreted session executes through.
///
/// Calls are synchronous, ordered, and never re-entrant. Every fallible
/// call reports a host error the interpreter classifies with
/// [`Self::error_kind`]; the interpreter never inspects it otherwise, so a
/// fatal capability error reaches the session owner unchanged. Data-model
/// operations have the semantics and errors of the WIT `data-model-api`
/// function of the same name; a session with no linked data model answers
/// each fallible one with an [`CapabilityErrorKind::Unavailable`] error and
/// each pure one with `None`.
pub trait SessionCapabilities {
    /// Host failure type.
    type Error;

    /// Classifies one host failure.
    fn error_kind(error: &Self::Error) -> CapabilityErrorKind;

    /// Maps one interpreter- or engine-originated failure to the host
    /// failure type.
    fn failure(&mut self, failure: InterpretFailure) -> Self::Error;

    /// Admits one engine command against the run's shared budget.
    ///
    /// # Errors
    ///
    /// Any error fails the session before the command.
    fn admit_command(&mut self) -> Result<(), Self::Error>;

    /// Reports one trace observation; infallible, never alters semantics.
    fn observe(&mut self, observation: Observation<'_>);

    /// Updates the configuration observed by data-model `In()` queries:
    /// removes every `exited` state (the others keep their order), then
    /// appends every `entered` state in order (see
    /// [`crate::EffectExecutor::update_configuration`]).
    ///
    /// # Errors
    ///
    /// Any error fails the session.
    fn update_active_configuration(
        &mut self,
        exited: &[StateId],
        entered: &[StateId],
    ) -> Result<(), Self::Error>;

    /// Opens one ordered effect batch whose `_event` is `trigger` and
    /// returns the W3C trace carrier of the batch's own operation span,
    /// stamped on every event the batch raises, sends, forwards, or fails
    /// with (`None` when telemetry produced no carrier and the trigger
    /// carries none).
    ///
    /// # Errors
    ///
    /// Any error fails the session.
    fn begin_batch(&mut self, trigger: Option<&Event>)
    -> Result<Option<TraceContext>, Self::Error>;

    /// Closes the batch opened by [`Self::begin_batch`]; `end` is
    /// [`BatchEnd::Aborted`] when a fatal failure stopped it.
    fn end_batch(&mut self, end: BatchEnd);

    /// The linked data model's stable name; `None` when none is linked.
    fn data_model_name(&mut self) -> Option<String>;

    /// `initialize` with the host's own system values (`_sessionid`,
    /// `_ioprocessors`) and the interpreter-supplied `_name` (SCXML 5.10).
    ///
    /// # Errors
    ///
    /// Any error fails the session.
    fn initialize_data_model(
        &mut self,
        declarations: &[String],
        machine_name: Option<&str>,
    ) -> Result<(), Self::Error>;

    /// Declares one late-bound `<data>` variable (SCXML 5.3).
    ///
    /// # Errors
    ///
    /// Any error fails the session.
    fn declare(&mut self, id: &str) -> Result<(), Self::Error>;

    /// Values one declared `<data>` variable (SCXML 5.2-5.3).
    ///
    /// # Errors
    ///
    /// Recoverable errors are initializer failures (`error.execution`).
    fn initialize_data(
        &mut self,
        id: &str,
        initializer: &DataInitializer,
    ) -> Result<(), Self::Error>;

    /// Sets one `<invoke>` parameter in an invoked session (SCXML 6.4.2);
    /// `false` when the session does not declare `name`.
    ///
    /// # Errors
    ///
    /// As [`Self::initialize_data`].
    fn set_parameter(&mut self, name: &str, value: &DataValue) -> Result<bool, Self::Error>;

    /// Evaluates a transition or `<if>` condition (SCXML 5.9).
    ///
    /// # Errors
    ///
    /// Recoverable errors make the condition false with `error.execution`.
    fn evaluate_condition(
        &mut self,
        expression: &str,
        event: Option<&Event>,
    ) -> Result<bool, Self::Error>;

    /// Evaluates a value expression.
    ///
    /// # Errors
    ///
    /// Recoverable errors are `error.execution`.
    fn evaluate_value(
        &mut self,
        expression: &str,
        event: Option<&Event>,
    ) -> Result<DataValue, Self::Error>;

    /// Reads one existing location (`<param location>`, a `namelist`
    /// entry).
    ///
    /// # Errors
    ///
    /// Recoverable errors are `error.execution`.
    fn evaluate_location(
        &mut self,
        location: &str,
        event: Option<&Event>,
    ) -> Result<DataValue, Self::Error>;

    /// Evaluates a string-typed attribute (`eventexpr`, `targetexpr`,
    /// `typeexpr`, `delayexpr`, `sendidexpr`, `srcexpr`); `None` for a
    /// non-string value.
    ///
    /// # Errors
    ///
    /// Recoverable errors are `error.execution`.
    fn evaluate_text(
        &mut self,
        expression: &str,
        event: Option<&Event>,
    ) -> Result<Option<String>, Self::Error>;

    /// Renders a `<log expr>` (SCXML 4.7) to its log text.
    ///
    /// # Errors
    ///
    /// Recoverable errors are `error.execution`.
    fn evaluate_log(
        &mut self,
        expression: &str,
        event: Option<&Event>,
    ) -> Result<String, Self::Error>;

    /// Evaluates one `BasicHTTP` form value (SCXML C.2).
    ///
    /// # Errors
    ///
    /// Recoverable errors are `error.execution`.
    fn evaluate_form_value(
        &mut self,
        source: &str,
        kind: FormValueSource,
        event: Option<&Event>,
    ) -> Result<DataValue, Self::Error>;

    /// Evaluates a `<foreach array>` expression to its item snapshot.
    ///
    /// # Errors
    ///
    /// Recoverable errors are `error.execution`.
    fn foreach_items(
        &mut self,
        expression: &str,
        event: Option<&Event>,
    ) -> Result<ForeachItems, Self::Error>;

    /// The payload key of one `namelist` entry; `None` for an illegal entry
    /// (SCXML 6.2.4). Pure.
    fn namelist_key(&mut self, location: &str) -> Option<String>;

    /// Builds `_event.data` from `<param>`/`namelist` entries (SCXML
    /// 6.2.4-6.2.5); `None` when a value cannot be carried. Pure.
    fn event_payload(&mut self, entries: &[NamedValue]) -> Option<DataValue>;

    /// Value of inline `<content>` or `<data>` children; `None` when the
    /// data model has no value for it. Pure.
    fn inline_content(&mut self, content: &InlineBody) -> Option<DataValue>;

    /// Value of an external `<data src>` document the data model decides
    /// itself; `None` leaves the interpreter's JSON-or-text rule. Pure.
    fn external_data(&mut self, text: &str) -> Option<DataValue>;

    /// The value event data carries under `name` (the entry a sender built
    /// from a `<param>` or `namelist` item of that name); `None` when there
    /// is none (SCXML 6.5.2). Pure.
    fn event_data_field(&mut self, data: &DataValue, name: &str) -> Option<DataValue>;

    /// Executes `<assign>`, an `idlocation` store, or a `<foreach>`
    /// `item`/`index` assignment (SCXML 5.4, 4.6).
    ///
    /// # Errors
    ///
    /// Recoverable errors are `error.execution`; a failed assignment
    /// changes no data.
    fn assign(
        &mut self,
        location: &str,
        source: &AssignSource,
        placement: &AssignPlacement,
        event: Option<&Event>,
        context: Option<&TraceContext>,
    ) -> Result<(), Self::Error>;

    /// Executes one inline `<script>` body (SCXML 5.8).
    ///
    /// # Errors
    ///
    /// Recoverable errors are `error.execution`.
    fn execute(&mut self, script: &str, event: Option<&Event>) -> Result<(), Self::Error>;

    /// Runs one external script loaded at document load (SCXML 5.8).
    ///
    /// # Errors
    ///
    /// Recoverable errors are `error.execution`.
    fn run_script(
        &mut self,
        source: &ScriptSourceIr,
        event: Option<&Event>,
    ) -> Result<(), Self::Error>;

    /// Loads one external document exactly as authored by `uri`, with the
    /// media type its source indicated.
    ///
    /// # Errors
    ///
    /// [`CapabilityErrorKind::Unavailable`] when the URI is not resolvable
    /// (`error.execution`); anything else fails the session.
    fn load_document(
        &mut self,
        kind: DocumentKind,
        uri: &str,
    ) -> Result<LoadedDocument, Self::Error>;

    /// Classifies one `<send>` target through the SCXML Event I/O
    /// Processor with this session's sender facts.
    fn classify_target(&mut self, target: Option<&str>) -> SendTarget;

    /// Routes one SCXML Event I/O Processor `<send>`; the processor stamps
    /// `origin`, `origin-type`, and `raw`.
    ///
    /// # Errors
    ///
    /// [`CapabilityErrorKind::InvalidInput`] and
    /// [`CapabilityErrorKind::PayloadTooLarge`] are `error.execution`;
    /// everything else fails the session.
    fn send(&mut self, request: SendRequest) -> Result<SendDisposition, Self::Error>;

    /// Sends one `<send>` through the Event I/O Processor its type names
    /// (WIT `send-processor`): at once when its delay is 0; otherwise the
    /// processor checks it and the host schedules it
    /// ([`SendDisposition::Scheduled`]) and dispatches it when it falls
    /// due, and a due send that cannot be dispatched raises
    /// `error.communication` with its send id (SCXML 6.2, 6.2.4, C.2).
    ///
    /// # Errors
    ///
    /// [`CapabilityErrorKind::InvalidInput`] (a type no processor answers
    /// included),
    /// [`CapabilityErrorKind::PayloadTooLarge`], and
    /// [`CapabilityErrorKind::CapacityExhausted`] are `error.execution`;
    /// everything else fails the session.
    fn send_processor(&mut self, request: ProcessorSend) -> Result<SendDisposition, Self::Error>;

    /// Cancels one pending delayed send (SCXML 6.3); an unknown id succeeds.
    ///
    /// # Errors
    ///
    /// Recoverable errors are `error.execution`.
    fn cancel(&mut self, send_id: &str, context: Option<&TraceContext>) -> Result<(), Self::Error>;

    /// Records one `<log>` with its rendered text (SCXML 4.7).
    ///
    /// # Errors
    ///
    /// Recoverable errors are `error.execution`.
    fn log(
        &mut self,
        label: Option<&str>,
        text: &str,
        context: Option<&TraceContext>,
    ) -> Result<(), Self::Error>;

    /// Executes one executable-content extension (SCXML 4.10), with
    /// `_event` bound to `event`, through the extension component bound to its
    /// namespace.
    ///
    /// # Errors
    ///
    /// The host's failure: no extension component is bound (`unavailable`) or a
    /// fatal host failure; both are fatal to the session unless recoverable.
    /// The extension component's own answer is [`ExtensionExecution::outcome`].
    /// `snapshot` is present exactly when [`Self::extension_snapshot_request`]
    /// answered true for this element.
    fn execute_extension(
        &mut self,
        element: &XmlNodeIr,
        event: Option<&Event>,
        context: Option<&TraceContext>,
        snapshot: Option<ExtensionSnapshot<'_>>,
    ) -> Result<ExtensionExecution<Self::Error>, Self::Error>;

    /// Whether the extension component bound to `element`'s namespace reads the
    /// session snapshot, so the interpreter computes its executable-content
    /// image for [`Self::execute_extension`]. Infallible and side-effect free.
    fn extension_snapshot_request(&mut self, element: &XmlNodeIr) -> bool;

    /// Loads the extension component of every executable-content extension
    /// namespace in `namespace_uris` (SCXML 4.10): the Machine IR's exact,
    /// sorted, unique, non-empty list. Called once per session by
    /// [`load_extensions`](super::load_extensions), before data binding.
    ///
    /// # Errors
    ///
    /// Any error is fatal: the session must not start.
    fn load_extensions(&mut self, namespace_uris: &[String]) -> Result<(), Self::Error>;

    /// Compiles one child document to lowered Machine IR bytes.
    ///
    /// # Errors
    ///
    /// [`CapabilityErrorKind::InvalidInput`] when the document is rejected
    /// (the invocation fails with `error.execution`).
    fn compile_invocation(
        &mut self,
        source_name: &str,
        source: &str,
    ) -> Result<Vec<u8>, Self::Error>;

    /// Starts one invocation: inside this call
    /// ([`InvocationProgress::Completed`]), or by the host once the run
    /// suspends right after this effect ([`InvocationProgress::Deferred`],
    /// what both shipped hosts do; the result comes back through
    /// [`super::Interpreter::continuing`]). An invoked session is
    /// constructed and started; an invoked service runs in the service
    /// component the host binds to its exact type.
    ///
    /// # Errors
    ///
    /// [`CapabilityErrorKind::InvalidInput`] when the child is rejected at
    /// load or the service rejected its start, and
    /// [`CapabilityErrorKind::Unavailable`] for a service type no component
    /// is bound to (each fails the invocation with `error.execution`);
    /// everything else fails the session.
    fn start_invocation(&mut self, request: Invocation) -> Result<InvocationProgress, Self::Error>;

    /// Cancels and drops one invocation (an invoked session or service), now or deferred as
    /// [`Self::start_invocation`]; an absent id succeeds.
    ///
    /// # Errors
    ///
    /// Any error fails the session.
    fn cancel_invocation(&mut self, invoke_id: &str) -> Result<InvocationProgress, Self::Error>;

    /// Queues one event on a live, unfinished invoked session's external event queue; otherwise a
    /// no-op. The forwarded copy carries `trace_context`, the forwarding
    /// batch's carrier, in place of the event's own (SCXML 6.4.1 copies
    /// every `_event` field; the carrier is not one).
    ///
    /// # Errors
    ///
    /// Any error fails the session.
    fn forward_to_invocation(
        &mut self,
        invoke_id: &str,
        event: &Event,
        trace_context: Option<&TraceContext>,
    ) -> Result<(), Self::Error>;
}
