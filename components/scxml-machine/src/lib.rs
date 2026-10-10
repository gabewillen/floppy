//! Generic machine guest.
//!
//! The generic deterministic `scxml-engine` is linked into this guest. It
//! embeds no machine: every session obtains its immutable lowered Machine IR
//! at construction through the imported `stateforward:scxml/ir-source`
//! interface, decodes it under explicit bounds, checks its version, validates
//! it structurally and semantically, and only then builds the engine. The
//! same component therefore serves two deployment modes:
//!
//! * runtime mode: a host (Wasmtime today, a browser later) implements
//!   `ir-source` with compiler output supplied at run time;
//! * packaged mode: the compiler's packager composes this component with a
//!   data-only provider that exports `ir-source` (non-browser AOT hosts).
//!
//! In both modes the host never interprets SCXML or owns machine semantics.
//! Every effect's SCXML meaning (data binding, `<send>`, `<cancel>`,
//! `<log>`, `<foreach>`, `<donedata>`, and invocations) comes from
//! [`scxml_engine::interpret`]; this guest only implements its
//! `SessionCapabilities` one-to-one over the imported
//! `machine-capabilities` resource.
//!
//! ## Unsafe-code containment
//!
//! Authored code in this crate carries `#![forbid(unsafe_code)]` on every
//! native target. On `wasm32` the crate-root `allow(unsafe_code)` attribute
//! exists solely because the generated `wit_bindgen::generate!` canonical-ABI
//! shims expand unsafe code inside this crate's span; no authored unsafe is
//! permitted. Containment is enforced mechanically by
//! `scripts/verify-component-authored-unsafe.py` in every proof run, which
//! fails closed if authored unsafe code appears outside the generated
//! bindings. Removal plan: the crate-wide allow is retired when the
//! generated bindings move to their own crate or wit-bindgen stops emitting
//! unsafe spans inside the guest crate.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; scripts/verify-component-authored-unsafe.py rejects authored unsafe"
    )
)]

use std::fmt;

/// Maximum UTF-8 bytes accepted for one event metadata value (origin,
/// origin-type); the portable event contract's bound.
pub const MAX_METADATA_BYTES: usize = scxml_types::MAX_EVENT_METADATA_BYTES;
/// Maximum UTF-8 bytes accepted for one text, JSON, or XML data value and
/// for `_event.raw`; the portable event contract's bound.
pub const MAX_DATA_VALUE_BYTES: usize = scxml_types::MAX_EVENT_DATA_BYTES;
/// Maximum aggregate UTF-8 bytes accepted for one event; the portable event
/// contract's bound.
pub const MAX_EVENT_BYTES: usize = scxml_types::MAX_EVENT_BYTES;
/// Maximum `<param>`/`namelist` entries accepted by `start-invoked`; the
/// WIT `named-value` list bound.
pub const MAX_INVOCATION_PARAMETERS: usize = scxml_types::MAX_SEND_MESSAGE_PARAMS;

/// Typed failure of loading untrusted Machine IR at session construction.
///
/// Each variant maps to one closed WIT `error-code` and one static detail, so
/// a failed session reports why without echoing the IR.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IrLoadError {
    /// More than [`scxml_compiler_core::MAX_IR_BYTES`] bytes (`payload-too-large`).
    TooLarge,
    /// A Machine IR version other than
    /// [`scxml_compiler_core::MACHINE_IR_VERSION`] (`unsupported-version`).
    UnsupportedVersion,
    /// Malformed JSON, unknown fields or variants, or a structurally or
    /// semantically invalid machine (`invalid-input`).
    Invalid,
    /// Valid IR that uses semantics outside the engine's supported slice
    /// (`unavailable`).
    Unsupported,
}

impl IrLoadError {
    /// Static, payload-free description (at most 160 UTF-8 bytes).
    #[must_use]
    pub const fn detail(&self) -> &'static str {
        match self {
            Self::TooLarge => "machine IR exceeds its byte bound",
            Self::UnsupportedVersion => "machine IR version is unsupported",
            Self::Invalid => "machine IR is malformed or invalid",
            Self::Unsupported => "machine IR uses unsupported engine semantics",
        }
    }
}

impl fmt::Display for IrLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.detail())
    }
}

impl std::error::Error for IrLoadError {}

/// A freshly loaded engine plus the external scripts its host must load.
#[cfg(any(target_arch = "wasm32", test))]
struct LoadedMachine {
    engine: scxml_engine::Engine,
    /// Distinct `<script src>` references the host resolves at document
    /// load (SCXML 1.0 section 5.8), in document order.
    scripts: Vec<scxml_compiler_core::ScriptSourceIr>,
}

/// Decodes, bounds, version-checks, and validates untrusted Machine IR bytes
/// and builds a fresh engine from them.
///
/// The bytes are borrowed and not retained; on failure nothing is built.
/// Checks run in this order: byte bound, version probe, closed-schema decode
/// plus structural and semantic validation, engine support.
///
/// # Errors
///
/// Returns the first failing check as a closed [`IrLoadError`].
#[cfg(any(target_arch = "wasm32", test))]
fn load_engine_from_ir(bytes: &[u8]) -> Result<LoadedMachine, IrLoadError> {
    if scxml_types::exceeds_wasm_bound(bytes.len(), scxml_compiler_core::MAX_IR_BYTES) {
        return Err(IrLoadError::TooLarge);
    }
    let machine =
        scxml_compiler_core::MachineIr::from_json(bytes).map_err(|error| match error {
            scxml_compiler_core::LowerError::UnsupportedIrVersion(_) => {
                IrLoadError::UnsupportedVersion
            }
            scxml_compiler_core::LowerError::LimitExceeded {
                field: "IR bytes", ..
            } => IrLoadError::TooLarge,
            _ => IrLoadError::Invalid,
        })?;
    let scripts = machine.host_loaded_scripts().into_iter().cloned().collect();
    let engine = scxml_engine::Engine::new(machine).map_err(|error| match error {
        scxml_engine::EngineError::UnsupportedModel => IrLoadError::Unsupported,
        _ => IrLoadError::Invalid,
    })?;
    Ok(LoadedMachine { engine, scripts })
}

/// Closed input classes used by the machine boundary's typed limit errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MachineInputKind {
    /// A session, event, invocation, or state name.
    Name,
    /// Event origin or origin-type metadata.
    Metadata,
    /// A text data-model value.
    Text,
    /// A JSON data-model value.
    Json,
    /// An XML data-model value.
    Xml,
    /// Retained I/O-processor message text (`_event.raw`).
    Raw,
    /// One complete event.
    Event,
}

/// Fail-closed validation failure for untrusted machine WIT inputs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MachineBoundaryError {
    /// A value failed its constructor-owned syntax contract.
    InvalidValue(MachineInputKind),
    /// One value exceeded its class-specific UTF-8 byte limit.
    ValueTooLarge(MachineInputKind),
    /// Checked aggregate byte accounting overflowed or exceeded its limit.
    AggregateTooLarge(MachineInputKind),
}

impl MachineBoundaryError {
    /// Static, payload-free description of this failure.
    ///
    /// The text is at most 160 UTF-8 bytes, names only the closed input
    /// class, and never reflects caller data, so it is safe to carry as the
    /// WIT `component-error.detail`.
    #[must_use]
    pub const fn detail(&self) -> &'static str {
        match self {
            Self::InvalidValue(MachineInputKind::Name) => "machine name input is invalid",
            Self::InvalidValue(MachineInputKind::Metadata) => "machine metadata input is invalid",
            Self::InvalidValue(MachineInputKind::Text) => "machine text input is invalid",
            Self::InvalidValue(MachineInputKind::Json) => "machine JSON input is invalid",
            Self::InvalidValue(MachineInputKind::Xml) => "machine XML input is invalid",
            Self::InvalidValue(MachineInputKind::Raw) => "machine raw event input is invalid",
            Self::InvalidValue(MachineInputKind::Event) => "machine event input is invalid",
            Self::ValueTooLarge(MachineInputKind::Name) => {
                "machine name input exceeds its UTF-8 byte limit"
            }
            Self::ValueTooLarge(MachineInputKind::Metadata) => {
                "machine metadata input exceeds its UTF-8 byte limit"
            }
            Self::ValueTooLarge(MachineInputKind::Text) => {
                "machine text input exceeds its UTF-8 byte limit"
            }
            Self::ValueTooLarge(MachineInputKind::Json) => {
                "machine JSON input exceeds its UTF-8 byte limit"
            }
            Self::ValueTooLarge(MachineInputKind::Xml) => {
                "machine XML input exceeds its UTF-8 byte limit"
            }
            Self::ValueTooLarge(MachineInputKind::Raw) => {
                "machine raw event input exceeds its UTF-8 byte limit"
            }
            Self::ValueTooLarge(MachineInputKind::Event) => {
                "machine event input exceeds its UTF-8 byte limit"
            }
            Self::AggregateTooLarge(MachineInputKind::Event) => {
                "machine event aggregate exceeds its byte limit"
            }
            Self::AggregateTooLarge(
                MachineInputKind::Name
                | MachineInputKind::Metadata
                | MachineInputKind::Text
                | MachineInputKind::Json
                | MachineInputKind::Xml
                | MachineInputKind::Raw,
            ) => "machine input byte accounting overflowed",
        }
    }
}

impl fmt::Display for MachineBoundaryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.detail())
    }
}

impl std::error::Error for MachineBoundaryError {}

#[derive(Clone, Copy)]
#[cfg(any(target_arch = "wasm32", test))]
enum BoundaryData<'a> {
    Empty,
    Text(&'a str),
    Json(&'a str),
    Xml(&'a str),
}

#[derive(Clone, Copy)]
#[cfg(any(target_arch = "wasm32", test))]
struct BoundaryEvent<'a> {
    name: &'a str,
    send_id: Option<&'a str>,
    origin: Option<&'a str>,
    origin_type: Option<&'a str>,
    invoke_id: Option<&'a str>,
    data: BoundaryData<'a>,
    raw: Option<&'a str>,
    trace_bytes: usize,
}

#[cfg(any(target_arch = "wasm32", test))]
fn checked_value_bytes(
    value: &str,
    maximum: usize,
    kind: MachineInputKind,
) -> Result<usize, MachineBoundaryError> {
    if value.len() > maximum {
        Err(MachineBoundaryError::ValueTooLarge(kind))
    } else {
        Ok(value.len())
    }
}

#[cfg(any(target_arch = "wasm32", test))]
fn checked_add(
    total: usize,
    bytes: usize,
    maximum: usize,
    kind: MachineInputKind,
) -> Result<usize, MachineBoundaryError> {
    total
        .checked_add(bytes)
        .filter(|value| *value <= maximum)
        .ok_or(MachineBoundaryError::AggregateTooLarge(kind))
}

#[cfg(any(target_arch = "wasm32", test))]
fn validate_event_boundary(event: BoundaryEvent<'_>) -> Result<usize, MachineBoundaryError> {
    let mut total = checked_value_bytes(
        event.name,
        scxml_types::MAX_NAME_BYTES,
        MachineInputKind::Name,
    )?;
    for value in [event.origin, event.origin_type].into_iter().flatten() {
        let bytes = checked_value_bytes(value, MAX_METADATA_BYTES, MachineInputKind::Metadata)?;
        total = checked_add(total, bytes, MAX_EVENT_BYTES, MachineInputKind::Event)?;
    }
    if let Some(value) = event.send_id {
        let bytes =
            checked_value_bytes(value, scxml_types::MAX_NAME_BYTES, MachineInputKind::Name)?;
        total = checked_add(total, bytes, MAX_EVENT_BYTES, MachineInputKind::Event)?;
    }
    if let Some(value) = event.invoke_id {
        let bytes =
            checked_value_bytes(value, scxml_types::MAX_NAME_BYTES, MachineInputKind::Name)?;
        total = checked_add(total, bytes, MAX_EVENT_BYTES, MachineInputKind::Event)?;
    }
    if let Some(value) = event.raw {
        // SCXML 6.2 `_event.raw` is retained processor message text. It
        // shares the portable event contract's data bound
        // (`scxml_types::Event::validate`) and still counts against the
        // aggregate event budget.
        let bytes = checked_value_bytes(value, MAX_DATA_VALUE_BYTES, MachineInputKind::Raw)?;
        total = checked_add(total, bytes, MAX_EVENT_BYTES, MachineInputKind::Event)?;
    }
    let (value, kind) = match event.data {
        BoundaryData::Empty => (None, MachineInputKind::Text),
        BoundaryData::Text(value) => (Some(value), MachineInputKind::Text),
        BoundaryData::Json(value) => (Some(value), MachineInputKind::Json),
        BoundaryData::Xml(value) => (Some(value), MachineInputKind::Xml),
    };
    if let Some(value) = value {
        let bytes = checked_value_bytes(value, MAX_DATA_VALUE_BYTES, kind)?;
        total = checked_add(total, bytes, MAX_EVENT_BYTES, MachineInputKind::Event)?;
    }
    checked_add(
        total,
        event.trace_bytes,
        MAX_EVENT_BYTES,
        MachineInputKind::Event,
    )
}

#[cfg(target_arch = "wasm32")]
use scxml_engine::Engine;

/// Lifecycle of one machine session resource.
///
/// `Active` sessions own an engine. `Failed` and `ShutDown` are terminal and
/// own none; only `shutdown` moves a `Failed` session to `ShutDown`.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionLifecycle {
    Active,
    Failed,
    ShutDown,
}

/// Session command gated by [`gate_command`].
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionCommand {
    Start,
    Send,
    Resume,
    /// `cancel`: W3C `exitInterpreter` of a running session.
    Cancel,
    /// `invocation-completed`: continues a suspended call.
    Continue,
}

/// Closed reason a command was rejected before touching session state.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LifecycleRejection {
    /// The session has failed; only `status`, `failure`, `configuration`,
    /// and `shutdown` remain meaningful.
    Failed,
    /// The session was shut down.
    ShutDown,
    /// `start` was called on a session that is not `ready`.
    AlreadyStarted,
    /// `send`, `resume`, or `cancel` was called before `start`.
    NotStarted,
    /// `send`, `resume`, or `cancel` was called on a `finished` session.
    Finished,
    /// `send` was called while the previous call stopped at the engine's
    /// per-call command bound with work still pending (`call-result.stable`
    /// false); `resume` must run first.
    NotStable,
    /// A command other than `invocation-completed` was called while the
    /// previous call is suspended (`call-result.suspended`).
    Suspended,
    /// `invocation-completed` was called while no call is suspended.
    NotSuspended,
}

#[cfg(any(target_arch = "wasm32", test))]
impl LifecycleRejection {
    /// Static, payload-free detail text for the WIT `component-error`.
    const fn detail(self) -> &'static str {
        match self {
            Self::Failed => "session has failed",
            Self::ShutDown => "session is shut down",
            Self::AlreadyStarted => "session is already started",
            Self::NotStarted => "session is not started",
            Self::Finished => "session is finished",
            Self::NotStable => "session has pending work; resume it before send",
            Self::Suspended => "session is suspended; complete its invocation request first",
            Self::NotSuspended => "session has no suspended invocation request",
        }
    }
}

/// Why `session.send` refused an event whose provenance only the machine
/// or the invocation path may set.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ForgedProvenance {
    /// The event type is `internal` or `platform`: only the machine's own
    /// `<raise>` and `<send target="_internal">` raise internal events, and
    /// platform errors go on the internal queue (SCXML 5.10.1, 3.12.2),
    /// through `communication-error` for those the host learns of late.
    NotExternal,
    /// The event carries an invoke id, but the session was not started by
    /// `start-invoked`: only `deliver-from-invocation` stamps an invoke id
    /// on an event a session receives from its child (SCXML 5.10.1, 6.4),
    /// and that id selects the `<finalize>` that runs.
    InvokeId,
}

#[cfg(any(target_arch = "wasm32", test))]
impl ForgedProvenance {
    /// Static, payload-free detail text for the WIT `component-error`.
    const fn detail(self) -> &'static str {
        match self {
            Self::NotExternal => "a sent event must have event type external",
            Self::InvokeId => "a sent event carries an invoke id only into an invoked session",
        }
    }
}

/// How a session was started: as the root, or by `start-invoked` for an
/// `<invoke>` of its parent.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionStart {
    /// The root session.
    Root,
    /// A session started by `start-invoked`.
    Invoked,
}

/// Checks the provenance of one event `session.send` delivers onto the
/// external queue. It arrives through an Event I/O Processor (`external`),
/// the only kind a stable session reads from its external queue (SCXML
/// Appendix D `mainEventLoop` blocks on `externalQueue.dequeue()`); an
/// `internal` event is never sent, and an error the platform raises after
/// the macrostep that caused it, such as the `error.communication` of a
/// `BasicHTTP` request that fails once its `<send>` returned, goes through
/// `communication-error` onto the internal queue instead. The event carries
/// no invoke id: a session receives its children's events only through
/// `deliver-from-invocation`. The one exception is a session started by
/// `start-invoked` ([`SessionStart::Invoked`]): its parent's `autoforward`
/// sends it an exact copy of each external-queue event the parent processes
/// (SCXML 6.4.1), invoke id included.
#[cfg(any(target_arch = "wasm32", test))]
const fn check_sent_provenance(
    event_type: scxml_types::EventType,
    invoke_id: Option<&scxml_types::InvokeId>,
    start: SessionStart,
) -> Result<(), ForgedProvenance> {
    if !matches!(event_type, scxml_types::EventType::External) {
        Err(ForgedProvenance::NotExternal)
    } else if invoke_id.is_some() && matches!(start, SessionStart::Root) {
        Err(ForgedProvenance::InvokeId)
    } else {
        Ok(())
    }
}

/// Whether the engine's last call ended at a checkpoint (WIT
/// `call-result.stable`).
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Stability {
    /// Every queue it could drain is drained.
    Stable,
    /// It stopped at its command bound with work left.
    NotStable,
}

/// The engine's `is_stable` answer as a [`Stability`].
#[cfg(target_arch = "wasm32")]
impl From<bool> for Stability {
    fn from(stable: bool) -> Self {
        if stable {
            Self::Stable
        } else {
            Self::NotStable
        }
    }
}

/// Decides whether one command may run against the current lifecycle and
/// engine status. Rejections leave the session unchanged.
#[cfg(any(target_arch = "wasm32", test))]
///
/// `engine` is the engine's status and stability, `None` when the session
/// owns no engine.
fn gate_command(
    lifecycle: SessionLifecycle,
    engine: Option<(&scxml_engine::EngineStatus, Stability)>,
    command: SessionCommand,
) -> Result<(), LifecycleRejection> {
    match (lifecycle, engine) {
        (SessionLifecycle::ShutDown, _) => Err(LifecycleRejection::ShutDown),
        (SessionLifecycle::Failed, _) | (SessionLifecycle::Active, None) => {
            Err(LifecycleRejection::Failed)
        }
        (SessionLifecycle::Active, Some((status, stable))) => match (command, status) {
            (SessionCommand::Send, scxml_engine::EngineStatus::Running)
                if stable == Stability::NotStable =>
            {
                Err(LifecycleRejection::NotStable)
            }
            (SessionCommand::Start, scxml_engine::EngineStatus::Ready)
            | (
                SessionCommand::Send | SessionCommand::Resume | SessionCommand::Cancel,
                scxml_engine::EngineStatus::Running,
            ) => Ok(()),
            // Only `MachineSession::gate` admits a continuation, and only
            // of a suspended call.
            (SessionCommand::Continue, _) => Err(LifecycleRejection::NotSuspended),
            (SessionCommand::Start, _) => Err(LifecycleRejection::AlreadyStarted),
            (_, scxml_engine::EngineStatus::Ready) => Err(LifecycleRejection::NotStarted),
            (_, scxml_engine::EngineStatus::Finished) => Err(LifecycleRejection::Finished),
        },
    }
}

/// Whether the session's last engine call suspended at a deferred
/// invocation request (`call-result.suspended`).
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LastCall {
    /// It returned suspended; only `invocation-completed` continues it.
    Suspended,
    /// It completed or was rejected.
    Settled,
}

/// [`gate_command`] behind the suspension gate: a suspended call admits
/// only its continuation, and a continuation needs a suspended call.
#[cfg(any(target_arch = "wasm32", test))]
fn gate_session(
    lifecycle: SessionLifecycle,
    engine: Option<(&scxml_engine::EngineStatus, Stability)>,
    last_call: LastCall,
    command: SessionCommand,
) -> Result<(), LifecycleRejection> {
    match (command, last_call) {
        (SessionCommand::Continue, LastCall::Suspended) => Ok(()),
        (SessionCommand::Continue, LastCall::Settled) => Err(LifecycleRejection::NotSuspended),
        (_, LastCall::Suspended) => Err(LifecycleRejection::Suspended),
        (_, LastCall::Settled) => gate_command(lifecycle, engine, command),
    }
}

/// Static detail of a `snapshot` refused because the last call did not
/// report `stable` (`docs/session-snapshot.md`, "When a snapshot can be
/// taken").
#[cfg(any(target_arch = "wasm32", test))]
const NOT_STABLE_DETAIL: &str = "not-stable";

/// Decides whether `session.snapshot` may read the session: never for a
/// failed or shut-down session, and only at a checkpoint (the last call
/// reported `stable`; a `ready` or `finished` session always is). The
/// error is the static `protocol` detail.
#[cfg(any(target_arch = "wasm32", test))]
fn gate_snapshot(
    lifecycle: SessionLifecycle,
    engine: Option<(&scxml_engine::EngineStatus, Stability)>,
) -> Result<(), &'static str> {
    match (lifecycle, engine) {
        (SessionLifecycle::ShutDown, _) => Err(LifecycleRejection::ShutDown.detail()),
        (SessionLifecycle::Failed, _) | (SessionLifecycle::Active, None) => {
            Err(LifecycleRejection::Failed.detail())
        }
        (
            SessionLifecycle::Active,
            Some((scxml_engine::EngineStatus::Running, Stability::NotStable)),
        ) => Err(NOT_STABLE_DETAIL),
        (SessionLifecycle::Active, Some(_)) => Ok(()),
    }
}

/// Closed WIT `error-code` class of one engine failure: each engine error
/// reports the class the WIT `error-code` definitions assign to its cause,
/// never a blanket `execution`.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EngineFailureClass {
    /// Malformed IR input (`invalid-input`).
    InvalidInput,
    /// A queue or per-command effect bound is full (`capacity-exhausted`).
    CapacityExhausted,
    /// A lifecycle or effect-protocol contract was broken (`protocol`).
    Protocol,
    /// SCXML execution failed (`execution`).
    Execution,
    /// Semantics or a capability are unavailable (`unavailable`).
    Unavailable,
    /// The guard-evaluation provider failed (`platform`).
    Platform,
}

/// Classifies one engine failure under its documented WIT `error-code`.
#[cfg(any(target_arch = "wasm32", test))]
fn classify_engine_error(error: scxml_engine::EngineError) -> EngineFailureClass {
    use scxml_engine::{EngineError, EvaluationError};
    match error {
        EngineError::InvalidModel => EngineFailureClass::InvalidInput,
        EngineError::QueueLimit | EngineError::EffectLimit => EngineFailureClass::CapacityExhausted,
        EngineError::AlreadyStarted
        | EngineError::NotStarted
        | EngineError::Finished
        | EngineError::NotStable
        | EngineError::BrokenInvariant
        | EngineError::EffectProtocol
        | EngineError::UnknownBlock => EngineFailureClass::Protocol,
        EngineError::Evaluation(EvaluationError::InvalidInput | EvaluationError::Execution) => {
            EngineFailureClass::Execution
        }
        EngineError::UnsupportedModel | EngineError::Evaluation(EvaluationError::Unavailable) => {
            EngineFailureClass::Unavailable
        }
        EngineError::Evaluation(EvaluationError::Platform) => EngineFailureClass::Platform,
    }
}

/// Byte range of the trace id in a canonical version-00 `traceparent`.
#[cfg(any(target_arch = "wasm32", test))]
const TRACE_ID: std::ops::Range<usize> = 3..35;
/// Byte range of the trace id and parent id (the span identity).
#[cfg(any(target_arch = "wasm32", test))]
const SPAN_IDENTITY: std::ops::Range<usize> = 3..52;
/// Byte range of the trace flags.
#[cfg(any(target_arch = "wasm32", test))]
const TRACE_FLAGS: std::ops::Range<usize> = 53..55;
/// W3C Trace Context Level 2 `random-trace-id` flag.
#[cfg(any(target_arch = "wasm32", test))]
const RANDOM_TRACE_ID_FLAG: u8 = 0x02;

/// Trace flags of a canonical `traceparent`, when present.
#[cfg(any(target_arch = "wasm32", test))]
fn trace_flags(traceparent: &str) -> Option<u8> {
    traceparent
        .get(TRACE_FLAGS)
        .and_then(|flags| u8::from_str_radix(flags, 16).ok())
}

/// The `traceparent` to hand the telemetry facade, which models only the
/// `sampled` flag: every other Level 2 flag is masked so a Level 2 carrier
/// still parents the batch span. [`child_event_trace_context`] restores
/// the `random-trace-id` flag on the outbound carrier.
#[cfg(any(target_arch = "wasm32", test))]
fn telemetry_traceparent(traceparent: &str) -> std::borrow::Cow<'_, str> {
    match (
        trace_flags(traceparent),
        traceparent.get(..TRACE_FLAGS.start),
    ) {
        (Some(flags), Some(prefix)) if flags > 1 => {
            std::borrow::Cow::Owned(format!("{prefix}{:02x}", flags & 1))
        }
        _ => std::borrow::Cow::Borrowed(traceparent),
    }
}

/// The carrier one effect batch stamps on every event it raises, sends,
/// autoforwards, or fails with.
///
/// `child` is the batch span's own carrier from the telemetry facade;
/// `parent` is the triggering event's carrier. When telemetry produced a
/// span of its own, the result names it (so the batch processing the event
/// is its child) with the trigger's baggage under the trigger's baggage
/// policy (no baggage and `drop` without a trigger carrier), and keeps the
/// trigger's Level 2 `random-trace-id` flag within the same trace. When it
/// did not (telemetry unavailable, a malformed answer, or a non-recording
/// provider echoing the trigger's own span), the trigger's carrier passes
/// through unchanged: telemetry never changes behavior.
///
/// # Errors
///
/// A noncanonical `parent` (the machine validates every carrier at its
/// boundary, so this is an invariant check).
#[cfg(any(target_arch = "wasm32", test))]
fn child_event_trace_context(
    child: Option<scxml_telemetry::TraceContext>,
    parent: Option<&scxml_types::TraceContext>,
) -> Result<Option<scxml_types::TraceContext>, scxml_types::TraceContextError> {
    if let Some(parent) = parent {
        parent.validate()?;
    }
    let passthrough = || Ok(parent.cloned());
    let Some((mut traceparent, tracestate)) =
        child.and_then(|child| child.traceparent.map(|value| (value, child.tracestate)))
    else {
        return passthrough();
    };
    if let Some(parent) = parent {
        if traceparent.get(SPAN_IDENTITY) == parent.traceparent.get(SPAN_IDENTITY) {
            return passthrough();
        }
        let random =
            trace_flags(&parent.traceparent).is_some_and(|flags| flags & RANDOM_TRACE_ID_FLAG != 0);
        if random
            && traceparent.get(TRACE_ID) == parent.traceparent.get(TRACE_ID)
            && let (Some(flags), Some(prefix)) = (
                trace_flags(&traceparent),
                traceparent.get(..TRACE_FLAGS.start),
            )
        {
            traceparent = format!("{prefix}{:02x}", flags | RANDOM_TRACE_ID_FLAG);
        }
    }
    let (baggage, policy) = parent.map_or((None, scxml_types::BaggagePolicy::Drop), |parent| {
        (parent.baggage.clone(), parent.baggage_policy)
    });
    scxml_types::TraceContext::new(traceparent, tracestate, baggage, policy)
        .map(Some)
        .or_else(|_| passthrough())
}

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    use std::cell::{Cell, RefCell};

    use super::{
        BoundaryData, BoundaryEvent, Engine, IrLoadError, LastCall, LifecycleRejection,
        MachineBoundaryError, MachineInputKind, SessionCommand, SessionLifecycle, SessionStart,
        Stability, check_sent_provenance, checked_add, checked_value_bytes,
        child_event_trace_context, gate_session, gate_snapshot, telemetry_traceparent,
        validate_event_boundary,
    };
    use scxml_compiler_core::XmlNodeIr;
    use scxml_engine::interpret::{
        self, BasicHttpSend, CapabilityErrorKind, DocumentKind, ExtensionExecution,
        ExtensionSnapshot, FailureCategory, ForeachItems, InlineBody, InterpretFailure,
        Interpreter, Invocation, InvocationContext as Invoked, InvocationProgress,
        SendTarget as NativeSendTarget, SessionCapabilities, SessionState,
    };
    use scxml_engine::{BatchEnd, Command, EngineStatus};
    use scxml_types::{
        BaggagePolicy, Event, EventName, EventType, SessionId, StateId,
        TraceContext as EventTraceContext,
    };

    wit_bindgen::generate!({
        world: "machine",
        path: "../../crates/scxml-wit/wit/scxml",
    });

    use exports::stateforward::scxml::machine_api::{
        CallResult, Guest, GuestSession, InvocationContext, InvocationMessage, MachineSnapshot,
        SessionFailure, SnapshotOptions, Status,
    };
    use stateforward::scxml::ir_source;
    use stateforward::scxml::machine_capability_types::Invocation as WitInvocation;
    use stateforward::scxml::machine_capability_types::{
        DocumentKind as WitDocumentKind, EventSchema as WitEventSchema,
        EventSchemaText as WitEventSchemaText, HistoryValue as WitHistoryValue,
        InvocationProgress as WitInvocationProgress, InvocationSnapshot as WitInvocationSnapshot,
        InvokedBy, InvokedSession as WitInvokedSession, MachineCapabilities,
        Observation as WitObservation, SendDisposition as WitSendDisposition,
        TransitionSnapshot as WitTransitionSnapshot, TransitionType as WitTransitionType,
    };
    use stateforward::scxml::telemetry::{self, Operation, Outcome};
    use stateforward::scxml::types::InvokeRequest as WitInvokeRequest;
    use stateforward::scxml::types::{
        AssignPlacement as WitAssignPlacement, AssignSource as WitAssignSource,
        BaggagePolicy as WitBaggagePolicy, BasichttpSend as WitBasichttpSend, ComponentError,
        DataInitializer as WitDataInitializer, DataValue as WitDataValue, ErrorCode, ErrorVersion,
        Event as WitEvent, EventType as WitEventType, ExtensionOutcome, ForeachResult,
        InlineBody as WitInlineBody, NamedValue as WitNamedValue, PinnedScript, ScriptReference,
        SendMessage as WitSendMessage, SendRequest as WitSendRequest, SendTarget as WitSendTarget,
    };

    struct Component;

    /// One machine session.
    ///
    /// Invariant: `lifecycle` is `Active` exactly when `engine` is `Some`;
    /// `failure` is `Some` exactly when `lifecycle` is `Failed`.
    ///
    /// Mutation and sharing contract: the `RefCell`/`Cell` fields exist only
    /// because `GuestSession` methods take `&self`. The session resource has
    /// one owner, the Component Model instance that created it; a guest runs
    /// on one thread, so the resource is never shared across threads or
    /// instances (it is neither `Send` nor `Sync`). Every export method
    /// borrows these fields for that call only. `run` holds the
    /// `engine.borrow_mut()` across `Engine::run`, which calls the imported
    /// `machine-capabilities` resource methods and `telemetry` through
    /// `MachineExecutor`; that is safe because the Component Model forbids
    /// an import from re-entering this instance while an export is running,
    /// so no other method of this session can run and borrow `engine` until
    /// `run` drops the borrow. `fail` and `shutdown` replace `engine` and
    /// `failure` only after that borrow is dropped.
    struct MachineSession {
        engine: RefCell<Option<Engine>>,
        capabilities: MachineCapabilities,
        lifecycle: Cell<SessionLifecycle>,
        failure: RefCell<Option<ComponentError>>,
        /// Interpreter state: `_sessionid`, the generated send- and
        /// invoke-id counters,
        /// the invoking parent's context, and pending return data. Borrowed
        /// only for one export call, like `engine`.
        state: RefCell<SessionState>,
        /// The open effect batch's telemetry state of a suspended call,
        /// restored by `invocation-completed`.
        suspended: RefCell<Option<SuspendedBatch>>,
    }

    /// What one [`MachineSession::run_prepared`] call runs.
    enum RunCommand<'a> {
        /// One engine command.
        Engine(Command<'a>),
        /// The continuation of a suspended call with its deferred
        /// invocation request's result.
        Continue(Result<(), ComponentError>),
    }

    /// [`WitCapabilities`] state of a call suspended inside an effect batch.
    struct SuspendedBatch {
        batch: Option<(u64, Option<stateforward::scxml::types::TraceContext>)>,
        last_outbound: Option<EventTraceContext>,
        start_parent: Option<EventTraceContext>,
    }

    impl Guest for Component {
        type Session = MachineSession;
    }

    impl GuestSession for MachineSession {
        fn new(session_id: String, capabilities: MachineCapabilities) -> Self {
            let input_bytes = metric(session_id.len());
            let state = SessionState::new(session_id.clone());
            let session = checked_value_bytes(
                &session_id,
                scxml_types::MAX_NAME_BYTES,
                MachineInputKind::Name,
            )
            .and_then(|_| {
                SessionId::new(session_id)
                    .map_err(|_| MachineBoundaryError::InvalidValue(MachineInputKind::Name))
            });
            let (engine, failure, outcome) = match session {
                Ok(_) => match load_engine(&capabilities) {
                    Ok(engine) => (Some(engine), None, Outcome::Ok),
                    Err(cause) => {
                        let outcome = classify_error(&cause);
                        (None, Some(cause), outcome)
                    }
                },
                Err(cause) => (None, Some(boundary_error(cause)), Outcome::Rejected),
            };
            let lifecycle = if engine.is_some() {
                SessionLifecycle::Active
            } else {
                SessionLifecycle::Failed
            };
            telemetry::observe(
                Operation::Initialize,
                outcome,
                Some(measurement(
                    telemetry::MeasurementKind::InputBytes,
                    input_bytes,
                )),
                None,
            );
            telemetry::event(
                Operation::Initialize,
                if outcome == Outcome::Ok {
                    telemetry::SignalEvent::InputAccepted
                } else {
                    telemetry::SignalEvent::InputRejected
                },
                None,
            );
            Self {
                engine: RefCell::new(engine),
                capabilities,
                lifecycle: Cell::new(lifecycle),
                failure: RefCell::new(failure),
                state: RefCell::new(state),
                suspended: RefCell::new(None),
            }
        }

        fn start(&self) -> Result<CallResult, SessionFailure> {
            // W3C Appendix D `interpret`: data binding precedes the
            // initial configuration (SCXML 5.2-5.3).
            self.run_prepared(
                Operation::Start,
                SessionCommand::Start,
                None,
                |engine, caps, _| {
                    interpret::load_extensions(engine, caps)?;
                    interpret::bind_initial_data(engine, caps, &[])
                },
                RunCommand::Engine(Command::Start),
            )
        }

        fn send(&self, event: WitEvent) -> Result<CallResult, SessionFailure> {
            let input_bytes = metric(wit_event_bytes(&event));
            let reject = |error: ComponentError| {
                telemetry::observe(
                    Operation::Send,
                    Outcome::Rejected,
                    Some(measurement(
                        telemetry::MeasurementKind::InputBytes,
                        input_bytes,
                    )),
                    None,
                );
                telemetry::event(Operation::Send, telemetry::SignalEvent::InputRejected, None);
                self.rejection(error)
            };
            if let Err(rejection) = self.gate(SessionCommand::Send) {
                return Err(reject(lifecycle_error(rejection)));
            }
            let start = if self.state.borrow().invoked().is_some() {
                SessionStart::Invoked
            } else {
                SessionStart::Root
            };
            let event = to_sent_event(event, start).map_err(reject)?;
            let context = event.trace_context.as_ref().map(telemetry_context);
            // SCXML C.1 applies to messages of the SCXML Event I/O
            // Processor, which stamps its type as the event's origintype.
            let scxml_message = event.origin_type.as_deref().is_some_and(|kind| {
                scxml_types::classify_processor_type(kind) == scxml_types::ProcessorKind::Scxml
            });
            if scxml_message && !self.handles_event_data(&event) {
                return self.ignore_message(Operation::Send, context.as_ref(), &event);
            }
            self.run(
                Operation::Send,
                SessionCommand::Send,
                context.as_ref(),
                RunCommand::Engine(Command::Send(&event)),
            )
        }

        fn accepts_event_data(&self, event: WitEvent) -> bool {
            let Ok(event) = to_event(event) else {
                return false;
            };
            // A pure read of the document's contracts; the engine is never
            // borrowed across a host call, and a session without one (its
            // document never loaded) judges nothing.
            match self.engine.try_borrow_mut() {
                Ok(mut engine) => engine.as_mut().is_none_or(|engine| {
                    engine.accepts_event_data(event.name.as_str(), &event.data)
                }),
                Err(_) => true,
            }
        }

        fn start_invoked(&self, context: InvocationContext) -> Result<CallResult, SessionFailure> {
            let (invoked, parameters, parent) = to_invocation(context).map_err(|error| {
                telemetry::observe(Operation::StartInvoked, Outcome::Rejected, None, None);
                telemetry::event(
                    Operation::StartInvoked,
                    telemetry::SignalEvent::InputRejected,
                    None,
                );
                self.rejection(error)
            })?;
            // SCXML 6.4: data binding, where a parameter the parent passed
            // is the initial value of the top-level <data> it names
            // (6.4.3), then the initial configuration. The session
            // remembers the parent for return data. Its start batches are
            // the invoking batch's children.
            let context = parent.as_ref().map(telemetry_context);
            self.run_prepared(
                Operation::StartInvoked,
                SessionCommand::Start,
                context.as_ref(),
                move |engine, caps, state| {
                    caps.start_parent = parent;
                    state.set_invoked(invoked);
                    interpret::load_extensions(engine, caps)?;
                    interpret::bind_initial_data(engine, caps, &parameters)
                },
                RunCommand::Engine(Command::Start),
            )
        }

        fn deliver_from_invocation(
            &self,
            invoke_id: String,
            message: InvocationMessage,
        ) -> Result<CallResult, SessionFailure> {
            if matches!(message, InvocationMessage::CommunicationError) {
                return self.invocation_communication_error(invoke_id);
            }
            let event = to_invocation_event(invoke_id, message).map_err(|error| {
                telemetry::observe(Operation::Deliver, Outcome::Rejected, None, None);
                telemetry::event(
                    Operation::Deliver,
                    telemetry::SignalEvent::InputRejected,
                    None,
                );
                self.rejection(error)
            })?;
            let context = event.trace_context.as_ref().map(telemetry_context);
            // A message from an invoked SCXML session travels through the
            // SCXML Event I/O Processor (SCXML C.1).
            if !self.handles_event_data(&event) {
                return self.ignore_message(Operation::Deliver, context.as_ref(), &event);
            }
            // SCXML 6.4: the engine runs the invocation's `<finalize>`
            // before selecting on the stamped event. No liveness check: a
            // message from a cancelled invocation is still delivered
            // (official case 236).
            self.run(
                Operation::Deliver,
                SessionCommand::Send,
                context.as_ref(),
                RunCommand::Engine(Command::Send(&event)),
            )
        }

        fn cancel(&self) -> Result<CallResult, SessionFailure> {
            // W3C `exitInterpreter`: `<onexit>` in exit order and every
            // invocation cancelled, without `done.invoke` (SCXML 6.4).
            self.run(
                Operation::Exit,
                SessionCommand::Cancel,
                None,
                RunCommand::Engine(Command::Cancel),
            )
        }

        fn communication_error(
            &self,
            send_id: Option<String>,
        ) -> Result<CallResult, SessionFailure> {
            let Ok(send_id) = send_id.map(scxml_types::SendId::new).transpose() else {
                telemetry::observe(Operation::CommunicationError, Outcome::Rejected, None, None);
                telemetry::event(
                    Operation::CommunicationError,
                    telemetry::SignalEvent::InputRejected,
                    None,
                );
                return Err(self.rejection(error(
                    ErrorCode::InvalidInput,
                    "communication-error send id is invalid",
                )));
            };
            // SCXML 3.12.2, 6.2.4: the error joins the internal queue; the
            // resumed run is the macrostep that processes it (the engine's
            // `queue_communication_error` documents the Appendix D mapping).
            self.run_prepared(
                Operation::CommunicationError,
                SessionCommand::Resume,
                None,
                move |engine, _, _| {
                    engine
                        .queue_communication_error(send_id, None, None)
                        .map_err(engine_error)
                },
                RunCommand::Engine(Command::Resume),
            )
        }

        fn resume(&self) -> Result<CallResult, SessionFailure> {
            let event_context = self
                .engine
                .borrow()
                .as_ref()
                .and_then(|engine| {
                    engine
                        .next_internal()
                        .and_then(|event| event.trace_context.clone())
                })
                .map(|context| {
                    context.validate()?;
                    Ok::<_, scxml_types::TraceContextError>(context)
                })
                .transpose()
                .map_err(|_| {
                    telemetry::observe(Operation::Resume, Outcome::Rejected, None, None);
                    telemetry::event(
                        Operation::Resume,
                        telemetry::SignalEvent::InputRejected,
                        None,
                    );
                    self.rejection(error(
                        ErrorCode::InvalidInput,
                        "resume trace context is invalid",
                    ))
                })?;
            let context = event_context.as_ref().map(telemetry_context);
            self.run(
                Operation::Resume,
                SessionCommand::Resume,
                context.as_ref(),
                RunCommand::Engine(Command::Resume),
            )
        }

        fn invocation_completed(
            &self,
            result: Result<(), ComponentError>,
        ) -> Result<CallResult, SessionFailure> {
            // SCXML 6.4: the deferred `<invoke>` start or cancel settles as
            // the suspended call's own request result would have.
            self.run(
                Operation::Resume,
                SessionCommand::Continue,
                None,
                RunCommand::Continue(result),
            )
        }

        fn configuration(&self) -> Vec<String> {
            self.engine
                .borrow()
                .as_ref()
                .map(|engine| {
                    engine
                        .leaves()
                        .iter()
                        .map(|state| state.as_str().to_owned())
                        .collect()
                })
                .unwrap_or_default()
        }

        fn status(&self) -> Status {
            match self.lifecycle.get() {
                SessionLifecycle::Active => self
                    .engine
                    .borrow()
                    .as_ref()
                    .map_or(Status::Failed, |engine| to_status(engine.status())),
                SessionLifecycle::Failed => Status::Failed,
                SessionLifecycle::ShutDown => Status::ShutDown,
            }
        }

        fn failure(&self) -> Option<ComponentError> {
            match self.lifecycle.get() {
                SessionLifecycle::Failed => self.failure.borrow().clone(),
                SessionLifecycle::Active | SessionLifecycle::ShutDown => None,
            }
        }

        // Read-only: borrows the engine and session state for this call,
        // evaluates nothing, calls no capability, and records no telemetry.
        fn snapshot(&self, options: SnapshotOptions) -> Result<MachineSnapshot, ComponentError> {
            let engine = self.engine.borrow();
            gate_snapshot(
                self.lifecycle.get(),
                engine.as_ref().map(|engine| {
                    (
                        engine.status(),
                        Stability::from(engine.is_stable() && !engine.is_suspended()),
                    )
                }),
            )
            .map_err(|detail| error(ErrorCode::Protocol, detail))?;
            let Some(engine) = engine.as_ref() else {
                return Err(lifecycle_error(LifecycleRejection::Failed));
            };
            let image = engine
                .snapshot(scxml_engine::SnapshotOptions {
                    transitions: if options.transitions {
                        scxml_engine::TransitionListing::Include
                    } else {
                        scxml_engine::TransitionListing::Omit
                    },
                    previews: if options.previews {
                        scxml_engine::Previews::Include
                    } else {
                        scxml_engine::Previews::Omit
                    },
                })
                .map_err(engine_error)?;
            Ok(to_machine_snapshot(image, &self.state.borrow()))
        }

        fn shutdown(&self) -> Result<(), ComponentError> {
            self.lifecycle.set(SessionLifecycle::ShutDown);
            *self.engine.borrow_mut() = None;
            *self.failure.borrow_mut() = None;
            telemetry::observe(Operation::Shutdown, Outcome::Ok, None, None);
            telemetry::event(
                Operation::Shutdown,
                telemetry::SignalEvent::TransformationCompleted,
                None,
            );
            Ok(())
        }
    }

    impl MachineSession {
        /// Gates every command on lifecycle and engine status: a suspended
        /// call admits only its continuation.
        fn gate(&self, command: SessionCommand) -> Result<(), LifecycleRejection> {
            let engine = self.engine.borrow();
            let last_call = if engine.as_ref().is_some_and(Engine::is_suspended) {
                LastCall::Suspended
            } else {
                LastCall::Settled
            };
            gate_session(
                self.lifecycle.get(),
                engine
                    .as_ref()
                    .map(|engine| (engine.status(), Stability::from(engine.is_stable()))),
                last_call,
                command,
            )
        }

        /// The capabilities of one call: a continued call resumes inside
        /// the batch its suspended call left open.
        fn capabilities_for(&self, command: &RunCommand<'_>) -> WitCapabilities<'_> {
            let resumed = match command {
                RunCommand::Continue(_) => self.suspended.borrow_mut().take(),
                RunCommand::Engine(_) => None,
            };
            let (batch, last_outbound, start_parent) = resumed
                .map_or((None, None, None), |resumed| {
                    (resumed.batch, resumed.last_outbound, resumed.start_parent)
                });
            WitCapabilities {
                capabilities: &self.capabilities,
                batch,
                last_outbound,
                start_parent,
            }
        }

        /// Keeps the open batch of a call that suspended for its
        /// continuation.
        fn keep_suspended_batch(&self, caps: &mut WitCapabilities<'_>) {
            *self.suspended.borrow_mut() = Some(SuspendedBatch {
                batch: caps.batch.take(),
                last_outbound: caps.last_outbound.take(),
                start_parent: caps.start_parent.take(),
            });
        }

        /// A rejection that leaves the session unchanged.
        fn rejection(&self, error: ComponentError) -> SessionFailure {
            SessionFailure {
                error,
                committed_effects: 0,
                session_failed: self.lifecycle.get() == SessionLifecycle::Failed,
            }
        }

        /// Moves the session to `Failed`, releasing the engine and retaining
        /// the cause for `failure`.
        fn fail(&self, cause: ComponentError, committed_effects: u32) -> SessionFailure {
            self.lifecycle.set(SessionLifecycle::Failed);
            *self.engine.borrow_mut() = None;
            *self.failure.borrow_mut() = Some(cause.clone());
            SessionFailure {
                error: cause,
                committed_effects,
                session_failed: true,
            }
        }

        /// Runs one command through [`Engine::run`] with an
        /// [`Interpreter`] over this session's capabilities: the engine owns
        /// the effect-transaction protocol, block-scoped recovery, and
        /// run-to-stable-configuration under [`scxml_engine::MAX_COMMANDS_PER_RUN`];
        /// the interpreter owns every effect's SCXML meaning; this session
        /// only gates the lifecycle and fails terminally on any run failure.
        fn run(
            &self,
            operation: Operation,
            command_kind: SessionCommand,
            context: Option<&stateforward::scxml::types::TraceContext>,
            command: RunCommand<'_>,
        ) -> Result<CallResult, SessionFailure> {
            self.run_prepared(operation, command_kind, context, |_, _, _| Ok(()), command)
        }

        /// Whether this session's data model can represent the data of an
        /// SCXML Event I/O Processor message as `_event.data`.
        fn handles_event_data(&self, event: &Event) -> bool {
            self.capabilities
                .accepts_event_data(&from_native_value(event.data.clone()))
        }

        /// SCXML C.1: "If the receiving Processor cannot handle the data
        /// format contained in the message, the receiving Processor MUST
        /// place the error error.communication in internal queue of the
        /// session for which the message was intended and MUST ignore the
        /// message." The error carries the message's send id (5.10.1, the
        /// failed attempt's), and the resumed run is the macrostep that
        /// processes it; the message itself is never dequeued.
        /// `deliver-from-invocation` of a `communication-error` message: the
        /// invoked service `invoke_id` failed after it started, so its
        /// `error.communication.<invoke_id>` (carrying the invoke id) joins
        /// the internal queue and the resumed run is the macrostep that
        /// processes it, as for `communication-error` (SCXML 3.12.2).
        fn invocation_communication_error(
            &self,
            invoke_id: String,
        ) -> Result<CallResult, SessionFailure> {
            let Ok(invoke_id) = scxml_types::InvokeId::new(invoke_id) else {
                telemetry::observe(Operation::Deliver, Outcome::Rejected, None, None);
                telemetry::event(
                    Operation::Deliver,
                    telemetry::SignalEvent::InputRejected,
                    None,
                );
                return Err(self.rejection(error(ErrorCode::InvalidInput, "invoke id is invalid")));
            };
            self.run_prepared(
                Operation::Deliver,
                SessionCommand::Resume,
                None,
                move |engine, _, _| {
                    engine
                        .queue_communication_error(None, Some(invoke_id), None)
                        .map_err(engine_error)
                },
                RunCommand::Engine(Command::Resume),
            )
        }

        fn ignore_message(
            &self,
            operation: Operation,
            context: Option<&stateforward::scxml::types::TraceContext>,
            event: &Event,
        ) -> Result<CallResult, SessionFailure> {
            let send_id = event.send_id.clone();
            let trace_context = event.trace_context.clone();
            self.run_prepared(
                operation,
                SessionCommand::Send,
                context,
                move |engine, _, _| {
                    engine
                        .queue_communication_error(send_id, None, trace_context)
                        .map_err(engine_error)
                },
                RunCommand::Engine(Command::Resume),
            )
        }

        /// [`Self::run`] after `prepare` (data binding, invocation
        /// parameters) ran against the gated engine; a `prepare` failure
        /// fails the session before the command.
        fn run_prepared(
            &self,
            operation: Operation,
            command_kind: SessionCommand,
            context: Option<&stateforward::scxml::types::TraceContext>,
            prepare: impl FnOnce(
                &mut Engine,
                &mut WitCapabilities<'_>,
                &mut SessionState,
            ) -> Result<(), ComponentError>,
            command: RunCommand<'_>,
        ) -> Result<CallResult, SessionFailure> {
            if let Err(rejection) = self.gate(command_kind) {
                telemetry::observe(operation, Outcome::Rejected, None, context);
                telemetry::event(operation, telemetry::SignalEvent::InputRejected, context);
                return Err(self.rejection(lifecycle_error(rejection)));
            }
            let mut engine_slot = self.engine.borrow_mut();
            let Some(engine) = engine_slot.as_mut() else {
                drop(engine_slot);
                telemetry::observe(operation, Outcome::Rejected, None, context);
                return Err(self.rejection(lifecycle_error(LifecycleRejection::Failed)));
            };
            let mut state = self.state.borrow_mut();
            let mut caps = self.capabilities_for(&command);
            let result = prepare(engine, &mut caps, &mut state)
                .map_err(|error| scxml_engine::RunFailure {
                    error,
                    kind: scxml_engine::RunFailureKind::Effect,
                    committed_effects: 0,
                })
                .and_then(|()| match command {
                    RunCommand::Engine(command) => {
                        engine.run(command, &mut Interpreter::new(&mut caps, &mut state))
                    }
                    RunCommand::Continue(result) => engine.run(
                        Command::Continue,
                        &mut Interpreter::continuing(&mut caps, &mut state, result),
                    ),
                });
            if matches!(&result, Ok(report) if report.suspended) {
                self.keep_suspended_batch(&mut caps);
            } else {
                // Every `<foreach>` this call ran is over (SCXML 4.6).
                self.capabilities.release_foreach();
            }
            let return_data = state.take_return_data();
            let last_outbound = caps.last_outbound.take();
            drop(state);
            drop(engine_slot);
            match result {
                Ok(report) => {
                    telemetry::observe(
                        operation,
                        Outcome::Ok,
                        Some(measurement(
                            telemetry::MeasurementKind::Effects,
                            report.committed_effects,
                        )),
                        context,
                    );
                    telemetry::event(
                        operation,
                        telemetry::SignalEvent::TransformationCompleted,
                        context,
                    );
                    Ok(CallResult {
                        status: to_status(&report.result.status),
                        active_configuration: report
                            .result
                            .leaves
                            .iter()
                            .map(|state| state.as_str().to_owned())
                            .collect(),
                        effect_count: report.committed_effects,
                        stable: report.stable,
                        // Only a session started by `start-invoked` whose
                        // top-level final has `<donedata>` returns data.
                        return_data: return_data.map(from_native_value),
                        // The batch that finished the session is the
                        // operation `done.invoke` names.
                        done_trace_context: (report.result.status == EngineStatus::Finished
                            && !report.suspended)
                            .then(|| last_outbound.as_ref().map(to_wit_trace_context))
                            .flatten(),
                        suspended: report.suspended,
                    })
                }
                Err(failure) => {
                    let outcome = classify_error(&failure.error);
                    telemetry::observe(
                        operation,
                        outcome,
                        Some(measurement(
                            telemetry::MeasurementKind::Effects,
                            failure.committed_effects,
                        )),
                        context,
                    );
                    if outcome == Outcome::Rejected {
                        telemetry::event(operation, telemetry::SignalEvent::InputRejected, context);
                    }
                    Err(self.fail(failure.error, failure.committed_effects))
                }
            }
        }
    }

    /// [`SessionCapabilities`] over the host-owned `machine-capabilities`
    /// resource: every call is one WIT import call with bounded value
    /// conversion; every SCXML decision stays in
    /// [`scxml_engine::interpret`].
    struct WitCapabilities<'a> {
        capabilities: &'a MachineCapabilities,
        /// Guest telemetry span of the open effect batch and the trigger's
        /// trace carrier.
        batch: Option<(u64, Option<stateforward::scxml::types::TraceContext>)>,
        /// The carrier of the last batch this call opened.
        last_outbound: Option<EventTraceContext>,
        /// During `start-invoked`: the invoking batch's carrier, the parent
        /// of every batch whose trigger has no carrier.
        start_parent: Option<EventTraceContext>,
    }

    impl SessionCapabilities for WitCapabilities<'_> {
        type Error = ComponentError;

        fn error_kind(error: &ComponentError) -> CapabilityErrorKind {
            match error.code {
                ErrorCode::InvalidInput => CapabilityErrorKind::InvalidInput,
                ErrorCode::Execution => CapabilityErrorKind::Execution,
                ErrorCode::PayloadTooLarge => CapabilityErrorKind::PayloadTooLarge,
                ErrorCode::Unavailable => CapabilityErrorKind::Unavailable,
                ErrorCode::CapacityExhausted => CapabilityErrorKind::CapacityExhausted,
                ErrorCode::DuplicateId
                | ErrorCode::UnknownId
                | ErrorCode::UnsupportedVersion
                | ErrorCode::Protocol
                | ErrorCode::Platform => CapabilityErrorKind::Other,
            }
        }

        /// Engine failures keep their documented class; interpreter
        /// failures carry their stable reason code as the static detail.
        fn failure(&mut self, failure: InterpretFailure) -> ComponentError {
            if let Some(cause) = failure.engine {
                return engine_error(cause);
            }
            let code = match failure.category {
                FailureCategory::Harness => ErrorCode::Protocol,
                FailureCategory::Unsupported => ErrorCode::Unavailable,
                FailureCategory::Execution => ErrorCode::Execution,
                FailureCategory::Budget => ErrorCode::CapacityExhausted,
            };
            error(code, failure.code)
        }

        fn admit_command(&mut self) -> Result<(), ComponentError> {
            self.capabilities.admit_command()
        }

        fn observe(&mut self, observation: interpret::Observation<'_>) {
            let observation = match observation {
                interpret::Observation::CommandCompleted => WitObservation::CommandCompleted,
                interpret::Observation::Raised(name) => WitObservation::Raised(name.to_owned()),
                interpret::Observation::Processed(name) => {
                    WitObservation::Processed(name.to_owned())
                }
            };
            self.capabilities.observe(&observation);
        }

        fn update_active_configuration(
            &mut self,
            exited: &[StateId],
            entered: &[StateId],
        ) -> Result<(), ComponentError> {
            let names = |states: &[StateId]| {
                states
                    .iter()
                    .map(|state| state.as_str().to_owned())
                    .collect::<Vec<_>>()
            };
            self.capabilities
                .update_active_configuration(&names(exited), &names(entered))
        }

        fn begin_batch(
            &mut self,
            trigger: Option<&Event>,
        ) -> Result<Option<EventTraceContext>, ComponentError> {
            let trigger_context = trigger
                .and_then(|event| event.trace_context.as_ref())
                .or(self.start_parent.as_ref());
            let context = trigger_context.map(telemetry_context);
            let (span_id, child_context) = telemetry::start(Operation::Execute, context.as_ref());
            if let Ok(outbound) = child_event_trace_context(
                child_context.map(|context| scxml_telemetry::TraceContext {
                    traceparent: Some(context.traceparent),
                    tracestate: context.tracestate,
                }),
                trigger_context,
            ) {
                self.batch = Some((span_id, context));
                self.last_outbound.clone_from(&outbound);
                Ok(outbound)
            } else {
                telemetry::end(span_id, Outcome::Rejected);
                Err(error(
                    ErrorCode::InvalidInput,
                    "effect trace context is invalid",
                ))
            }
        }

        fn end_batch(&mut self, end: BatchEnd) {
            let outcome = match end {
                BatchEnd::Completed => Outcome::Ok,
                BatchEnd::Aborted => Outcome::Failed,
            };
            if let Some((span_id, context)) = self.batch.take() {
                telemetry::end(span_id, outcome);
                telemetry::observe(Operation::Execute, outcome, None, context.as_ref());
                if end == BatchEnd::Completed {
                    telemetry::event(
                        Operation::Execute,
                        telemetry::SignalEvent::TransformationCompleted,
                        context.as_ref(),
                    );
                }
            }
        }

        fn data_model_name(&mut self) -> Option<String> {
            self.capabilities.data_model_name()
        }

        fn initialize_data_model(
            &mut self,
            declarations: &[String],
            machine_name: Option<&str>,
        ) -> Result<(), ComponentError> {
            self.capabilities
                .initialize_data_model(declarations, machine_name)
        }

        fn declare(&mut self, id: &str) -> Result<(), ComponentError> {
            self.capabilities.declare(id)
        }

        fn initialize_data(
            &mut self,
            id: &str,
            initializer: &scxml_types::DataInitializer,
        ) -> Result<(), ComponentError> {
            let initializer = match initializer {
                scxml_types::DataInitializer::Expression(expression) => {
                    WitDataInitializer::Expression(expression.clone())
                }
                scxml_types::DataInitializer::Value(value) => {
                    WitDataInitializer::Value(wit_value(value)?)
                }
            };
            self.capabilities.initialize_data(id, &initializer)
        }

        fn set_parameter(
            &mut self,
            name: &str,
            value: &scxml_types::DataValue,
        ) -> Result<bool, ComponentError> {
            self.capabilities.set_parameter(name, &wit_value(value)?)
        }

        fn evaluate_condition(
            &mut self,
            expression: &str,
            event: Option<&Event>,
        ) -> Result<bool, ComponentError> {
            let event = event.cloned().map(from_event);
            let result = self
                .capabilities
                .evaluate_condition(expression, event.as_ref());
            if result.is_err() {
                telemetry::event(
                    Operation::EvaluateCondition,
                    telemetry::SignalEvent::InputRejected,
                    None,
                );
            }
            result
        }

        fn evaluate_value(
            &mut self,
            expression: &str,
            event: Option<&Event>,
        ) -> Result<scxml_types::DataValue, ComponentError> {
            let event = event.cloned().map(from_event);
            native_value(
                self.capabilities
                    .evaluate_value(expression, event.as_ref())?,
            )
        }

        fn evaluate_location(
            &mut self,
            location: &str,
            event: Option<&Event>,
        ) -> Result<scxml_types::DataValue, ComponentError> {
            let event = event.cloned().map(from_event);
            native_value(
                self.capabilities
                    .evaluate_location(location, event.as_ref())?,
            )
        }

        fn evaluate_text(
            &mut self,
            expression: &str,
            event: Option<&Event>,
        ) -> Result<Option<String>, ComponentError> {
            let event = event.cloned().map(from_event);
            self.capabilities.evaluate_text(expression, event.as_ref())
        }

        fn evaluate_log(
            &mut self,
            expression: &str,
            event: Option<&Event>,
        ) -> Result<String, ComponentError> {
            let event = event.cloned().map(from_event);
            self.capabilities.evaluate_log(expression, event.as_ref())
        }

        fn evaluate_form_value(
            &mut self,
            source: &str,
            kind: scxml_types::FormValueSource,
            event: Option<&Event>,
        ) -> Result<scxml_types::DataValue, ComponentError> {
            let event = event.cloned().map(from_event);
            // The WIT `location` flag is the wire form of the source kind.
            let location = kind == scxml_types::FormValueSource::Location;
            native_value(
                self.capabilities
                    .evaluate_form_value(source, location, event.as_ref())?,
            )
        }

        fn foreach_items(
            &mut self,
            expression: &str,
            event: Option<&Event>,
        ) -> Result<ForeachItems, ComponentError> {
            let event = event.cloned().map(from_event);
            Ok(
                match self
                    .capabilities
                    .foreach_items(expression, event.as_ref())?
                {
                    ForeachResult::Items(items) => ForeachItems::Items(
                        items
                            .into_iter()
                            .map(native_value)
                            .collect::<Result<_, _>>()?,
                    ),
                    ForeachResult::Illegal => ForeachItems::Illegal,
                },
            )
        }

        fn namelist_key(&mut self, location: &str) -> Option<String> {
            self.capabilities.namelist_key(location)
        }

        fn event_payload(
            &mut self,
            entries: &[scxml_types::NamedValue],
        ) -> Option<scxml_types::DataValue> {
            let entries = wit_named_values(entries).ok()?;
            self.capabilities
                .event_payload(&entries)
                .and_then(|value| native_value(value).ok())
        }

        fn inline_content(&mut self, content: &InlineBody) -> Option<scxml_types::DataValue> {
            let content = match content {
                InlineBody::Text(text) => WitInlineBody::Text(text.clone()),
                InlineBody::Markup(markup) => WitInlineBody::Markup(markup.clone()),
            };
            self.capabilities
                .inline_content(&content)
                .and_then(|value| native_value(value).ok())
        }

        fn external_data(&mut self, text: &str) -> Option<scxml_types::DataValue> {
            self.capabilities
                .external_data(text)
                .and_then(|value| native_value(value).ok())
        }

        fn event_data_field(
            &mut self,
            data: &scxml_types::DataValue,
            name: &str,
        ) -> Option<scxml_types::DataValue> {
            let data = wit_value(data).ok()?;
            self.capabilities
                .event_data_field(&data, name)
                .and_then(|value| native_value(value).ok())
        }

        fn assign(
            &mut self,
            location: &str,
            source: &scxml_types::AssignSource,
            placement: &scxml_types::AssignPlacement,
            event: Option<&Event>,
            context: Option<&EventTraceContext>,
        ) -> Result<(), ComponentError> {
            let source = match source {
                scxml_types::AssignSource::Expression(expression) => {
                    WitAssignSource::Expression(expression.clone())
                }
                scxml_types::AssignSource::Value(value) => {
                    WitAssignSource::Value(wit_value(value)?)
                }
                scxml_types::AssignSource::ForeachValue(value) => {
                    WitAssignSource::ForeachValue(wit_value(value)?)
                }
            };
            let event = event.cloned().map(from_event);
            let context = context.map(to_wit_trace_context);
            self.capabilities.assign(
                location,
                &source,
                &to_wit_placement(placement.clone()),
                event.as_ref(),
                context.as_ref(),
            )
        }

        fn execute(&mut self, script: &str, event: Option<&Event>) -> Result<(), ComponentError> {
            let event = event.cloned().map(from_event);
            self.capabilities.execute(script, event.as_ref())
        }

        fn run_script(
            &mut self,
            source: &scxml_compiler_core::ScriptSourceIr,
            event: Option<&Event>,
        ) -> Result<(), ComponentError> {
            let event = event.cloned().map(from_event);
            self.capabilities
                .run_script(&to_script_reference(source), event.as_ref())
        }

        fn load_document(
            &mut self,
            kind: DocumentKind,
            uri: &str,
        ) -> Result<interpret::LoadedDocument, ComponentError> {
            let kind = match kind {
                DocumentKind::Data => WitDocumentKind::Data,
                DocumentKind::Invocation => WitDocumentKind::Invocation,
            };
            self.capabilities
                .load_document(kind, uri)
                .map(|document| interpret::LoadedDocument {
                    text: document.text,
                    media_type: document.media_type,
                })
        }

        fn classify_target(&mut self, target: Option<&str>) -> NativeSendTarget {
            match self.capabilities.classify_target(target) {
                WitSendTarget::Internal => NativeSendTarget::Internal,
                WitSendTarget::Local => NativeSendTarget::Local,
                WitSendTarget::Parent => NativeSendTarget::Parent,
                WitSendTarget::Invocation(invoke_id) => NativeSendTarget::Invocation(invoke_id),
                WitSendTarget::Unreachable => NativeSendTarget::Unreachable,
                WitSendTarget::Invalid => NativeSendTarget::Invalid,
            }
        }

        fn send(
            &mut self,
            request: scxml_types::SendRequest,
        ) -> Result<interpret::SendDisposition, ComponentError> {
            let message = match request.message {
                scxml_types::SendMessage::Empty => WitSendMessage::Empty,
                scxml_types::SendMessage::Params(fields) => {
                    WitSendMessage::Params(wit_named_values(&fields)?)
                }
                scxml_types::SendMessage::Content => WitSendMessage::Content,
            };
            let request = WitSendRequest {
                send_id: request.send_id.map(|id| id.as_str().to_owned()),
                event: from_event(request.event),
                target: request.target,
                processor_type: request.processor_type,
                delay_ms: request.delay_ms,
                message,
            };
            native_disposition(self.capabilities.send(&request)?)
        }

        fn send_basichttp(
            &mut self,
            request: BasicHttpSend,
        ) -> Result<interpret::SendDisposition, ComponentError> {
            let request = WitBasichttpSend {
                send_id: request.send_id.as_str().to_owned(),
                event_name: request.event_name,
                target: request.target,
                params: wit_named_values(&request.params)?,
                content: request.content.as_ref().map(wit_value).transpose()?,
                delay_ms: request.delay_ms,
                trace_context: request.trace_context.as_ref().map(to_wit_trace_context),
            };
            native_disposition(self.capabilities.send_basichttp(&request)?)
        }

        fn cancel(
            &mut self,
            send_id: &str,
            context: Option<&EventTraceContext>,
        ) -> Result<(), ComponentError> {
            let context = context.map(to_wit_trace_context);
            self.capabilities.cancel(send_id, context.as_ref())
        }

        fn log(
            &mut self,
            label: Option<&str>,
            text: &str,
            context: Option<&EventTraceContext>,
        ) -> Result<(), ComponentError> {
            let context = context.map(to_wit_trace_context);
            self.capabilities.log(label, text, context.as_ref())
        }

        fn execute_extension(
            &mut self,
            element: &XmlNodeIr,
            event: Option<&Event>,
            context: Option<&EventTraceContext>,
            snapshot: Option<ExtensionSnapshot<'_>>,
        ) -> Result<ExtensionExecution<ComponentError>, ComponentError> {
            let element = to_executable_content_extension(element)?;
            let event = event.cloned().map(from_event);
            let context = context.map(to_wit_trace_context);
            let snapshot =
                snapshot.map(|snapshot| to_machine_snapshot(snapshot.image, snapshot.session));
            let executed = self.capabilities.execute_extension(
                &element,
                event.as_ref(),
                context.as_ref(),
                snapshot.as_ref(),
            )?;
            Ok(ExtensionExecution {
                outcome: executed
                    .outcome
                    .map(|result| matches!(result, ExtensionOutcome::Handled)),
                internal: queued_events(executed.internal, EventType::Internal)?,
                external: queued_events(executed.external, EventType::External)?,
            })
        }

        fn extension_snapshot_request(&mut self, element: &XmlNodeIr) -> bool {
            match element {
                XmlNodeIr::Element { name, .. } => name
                    .namespace_uri
                    .as_deref()
                    .is_some_and(|uri| self.capabilities.extension_snapshot_request(uri)),
                XmlNodeIr::Text { .. } => false,
            }
        }

        fn load_extensions(&mut self, namespace_uris: &[String]) -> Result<(), ComponentError> {
            self.capabilities.load_extensions(namespace_uris)
        }

        fn compile_invocation(
            &mut self,
            source_name: &str,
            source: &str,
        ) -> Result<Vec<u8>, ComponentError> {
            self.capabilities.compile_invocation(source_name, source)
        }

        fn start_invocation(
            &mut self,
            request: Invocation,
        ) -> Result<InvocationProgress, ComponentError> {
            let request = match request {
                Invocation::Session(request) => WitInvocation::Session(WitInvokedSession {
                    invoke_id: request.invoke_id,
                    machine_ir: request.machine_ir,
                    data_model: request.data_model,
                    parameters: wit_named_values(&request.parameters)?,
                    trace_context: request.trace_context.as_ref().map(to_wit_trace_context),
                }),
                Invocation::Service(request) => WitInvocation::Service(WitInvokeRequest {
                    invoke_id: request.invoke_id,
                    type_uri: request.type_uri,
                    source: request.source,
                    content: request.content.as_ref().map(wit_value).transpose()?,
                    parameters: wit_named_values(&request.parameters)?,
                    attributes: request
                        .attributes
                        .into_iter()
                        .map(|attribute| stateforward::scxml::types::ExtensionAttribute {
                            name: stateforward::scxml::types::QualifiedName {
                                namespace_uri: attribute
                                    .name
                                    .namespace_uri
                                    .map(|uri| uri.as_str().to_owned()),
                                local_name: attribute.name.local_name,
                            },
                            value: attribute.value,
                        })
                        .collect(),
                    // The host binds the references; the machine never
                    // holds one.
                    references: Vec::new(),
                    trace_context: request.trace_context.as_ref().map(to_wit_trace_context),
                }),
            };
            self.capabilities
                .start_invocation(&request)
                .map(invocation_progress)
        }

        fn cancel_invocation(
            &mut self,
            invoke_id: &str,
        ) -> Result<InvocationProgress, ComponentError> {
            self.capabilities
                .cancel_invocation(invoke_id)
                .map(invocation_progress)
        }

        fn forward_to_invocation(
            &mut self,
            invoke_id: &str,
            event: &Event,
            trace_context: Option<&EventTraceContext>,
        ) -> Result<(), ComponentError> {
            let mut forwarded = from_event(event.clone());
            forwarded.trace_context = trace_context.map(to_wit_trace_context);
            self.capabilities
                .forward_to_invocation(invoke_id, &forwarded)
        }
    }

    /// A native value as a bounded WIT value.
    fn wit_value(value: &scxml_types::DataValue) -> Result<WitDataValue, ComponentError> {
        canonical_wit_value(from_native_value(value.clone()))
    }

    /// A host-supplied WIT value, bound-checked and canonicalized.
    fn native_value(value: WitDataValue) -> Result<scxml_types::DataValue, ComponentError> {
        canonical_wit_value(value).and_then(to_native_value)
    }

    fn wit_named_values(
        entries: &[scxml_types::NamedValue],
    ) -> Result<Vec<WitNamedValue>, ComponentError> {
        entries
            .iter()
            .map(|entry| {
                Ok(WitNamedValue {
                    name: entry.name.clone(),
                    value: wit_value(&entry.value)?,
                })
            })
            .collect()
    }

    /// The host's send disposition; a self-addressed event must itself be
    /// valid (a host that returns anything else broke its contract).
    fn native_disposition(
        disposition: WitSendDisposition,
    ) -> Result<interpret::SendDisposition, ComponentError> {
        Ok(match disposition {
            WitSendDisposition::EnqueueExternal(event) => {
                let event = to_event(event).map_err(|_| {
                    error(
                        ErrorCode::Protocol,
                        "host returned an invalid self-addressed event",
                    )
                })?;
                interpret::SendDisposition::EnqueueExternal(Box::new(event))
            }
            WitSendDisposition::Routed => interpret::SendDisposition::Routed,
            WitSendDisposition::Scheduled => interpret::SendDisposition::Scheduled,
            WitSendDisposition::Undeliverable => interpret::SendDisposition::Undeliverable,
        })
    }

    /// How the host carries out one invocation request (WIT
    /// `invocation-progress`).
    const fn invocation_progress(progress: WitInvocationProgress) -> InvocationProgress {
        match progress {
            WitInvocationProgress::Completed => InvocationProgress::Completed,
            WitInvocationProgress::Deferred => InvocationProgress::Deferred,
        }
    }

    /// Validates a `start-invoked` context: the parent's session id, the
    /// invoke id, and at most [`super::MAX_INVOCATION_PARAMETERS`] bounded
    /// parameters.
    fn to_invocation(
        context: InvocationContext,
    ) -> Result<
        (
            Invoked,
            Vec<scxml_types::NamedValue>,
            Option<EventTraceContext>,
        ),
        ComponentError,
    > {
        SessionId::new(context.parent_session_id.clone())
            .map_err(|_| error(ErrorCode::InvalidInput, "parent session id is invalid"))?;
        scxml_types::InvokeId::new(context.invoke_id.clone())
            .map_err(|_| error(ErrorCode::InvalidInput, "invoke id is invalid"))?;
        if context.parameters.len() > super::MAX_INVOCATION_PARAMETERS {
            return Err(error(
                ErrorCode::PayloadTooLarge,
                "invocation parameters exceed their count bound",
            ));
        }
        let parameters = context
            .parameters
            .into_iter()
            .map(|parameter| {
                if parameter.name.is_empty() || parameter.name.len() > scxml_types::MAX_NAME_BYTES {
                    return Err(error(
                        ErrorCode::InvalidInput,
                        "invocation parameter name is invalid",
                    ));
                }
                Ok(scxml_types::NamedValue {
                    name: parameter.name,
                    value: native_value(parameter.value)?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let trace_context = context
            .trace_context
            .map(to_native_trace_context)
            .transpose()?;
        Ok((
            Invoked {
                parent_session_id: context.parent_session_id,
                invoke_id: context.invoke_id,
            },
            parameters,
            trace_context,
        ))
    }

    /// The external event one child message becomes (SCXML 6.4): a child
    /// event stamped with its invoke id, or `done.invoke.<id>` carrying the
    /// child's `<donedata>` value.
    fn to_invocation_event(
        invoke_id: String,
        message: InvocationMessage,
    ) -> Result<Event, ComponentError> {
        let typed = scxml_types::InvokeId::new(invoke_id)
            .map_err(|_| error(ErrorCode::InvalidInput, "invoke id is invalid"))?;
        let mut event = match message {
            InvocationMessage::Event(event) => to_event(event)?,
            InvocationMessage::Done(done) => {
                let name = EventName::new(format!("done.invoke.{typed}")).map_err(|_| {
                    error(ErrorCode::InvalidInput, "done.invoke event name is invalid")
                })?;
                let mut event = Event::external(name);
                if let Some(data) = done.data {
                    event.data = native_value(data)?;
                }
                event.trace_context = done
                    .trace_context
                    .map(to_native_trace_context)
                    .transpose()?;
                event
            }
            // `deliver-from-invocation` handles it before this conversion.
            InvocationMessage::CommunicationError => {
                return Err(error(
                    ErrorCode::Protocol,
                    "an invocation's communication error is no event",
                ));
            }
        };
        event.invoke_id = Some(typed);
        Ok(event)
    }

    const fn classify_error(error: &ComponentError) -> Outcome {
        match error.code {
            ErrorCode::InvalidInput
            | ErrorCode::DuplicateId
            | ErrorCode::UnknownId
            | ErrorCode::CapacityExhausted
            | ErrorCode::PayloadTooLarge
            | ErrorCode::UnsupportedVersion
            | ErrorCode::Protocol => Outcome::Rejected,
            ErrorCode::Execution | ErrorCode::Unavailable | ErrorCode::Platform => Outcome::Failed,
        }
    }

    /// Fetches this session's Machine IR through the imported `ir-source`,
    /// builds the engine, and has the host load every external script
    /// (SCXML 1.0 section 5.8: a `src` that cannot be fetched or parsed at
    /// document load rejects the document); every failure is a typed
    /// construction failure.
    fn load_engine(capabilities: &MachineCapabilities) -> Result<Engine, ComponentError> {
        let bytes = ir_source::machine_ir();
        super::load_engine_from_ir(&bytes)
            .map_err(ir_load_error)
            .and_then(|loaded| {
                for script in &loaded.scripts {
                    capabilities.load_script(&to_script_reference(script))?;
                }
                Ok(loaded.engine)
            })
    }

    fn to_script_reference(source: &scxml_compiler_core::ScriptSourceIr) -> ScriptReference {
        ScriptReference {
            uri: source.uri.clone(),
            pinned: source.pinned.as_ref().map(|pinned| PinnedScript {
                media_type: pinned.media_type.clone(),
                sha256: pinned.sha256.clone(),
                wit_world: pinned.wit_world.clone(),
            }),
        }
    }

    fn ir_load_error(cause: IrLoadError) -> ComponentError {
        let code = match cause {
            IrLoadError::TooLarge => ErrorCode::PayloadTooLarge,
            IrLoadError::UnsupportedVersion => ErrorCode::UnsupportedVersion,
            IrLoadError::Invalid => ErrorCode::InvalidInput,
            IrLoadError::Unsupported => ErrorCode::Unavailable,
        };
        error(code, cause.detail())
    }

    fn to_executable_content_extension(
        element: &XmlNodeIr,
    ) -> Result<stateforward::scxml::types::ExecutableContentExtension, ComponentError> {
        let XmlNodeIr::Element {
            name,
            attributes,
            children,
        } = element
        else {
            return Err(error(
                ErrorCode::InvalidInput,
                "executable content extension root must be an element",
            ));
        };
        let mut content = Vec::new();
        append_extension_content(&mut content, children)?;
        let element = stateforward::scxml::types::ExecutableContentExtension {
            name: to_qualified_name(name),
            attributes: attributes
                .iter()
                .map(|attribute| stateforward::scxml::types::ExtensionAttribute {
                    name: to_qualified_name(&attribute.name),
                    value: attribute.value.clone(),
                })
                .collect(),
            content,
        };
        Ok(element)
    }

    fn append_extension_content(
        content: &mut Vec<stateforward::scxml::types::ExtensionContent>,
        children: &[XmlNodeIr],
    ) -> Result<(), ComponentError> {
        for child in children {
            match child {
                XmlNodeIr::Text { value } => content.push(
                    stateforward::scxml::types::ExtensionContent::Text(value.clone()),
                ),
                XmlNodeIr::Element {
                    name,
                    attributes,
                    children,
                } => {
                    content.push(stateforward::scxml::types::ExtensionContent::StartElement(
                        stateforward::scxml::types::StartElement {
                            name: to_qualified_name(name),
                            attributes: attributes
                                .iter()
                                .map(|attribute| stateforward::scxml::types::ExtensionAttribute {
                                    name: to_qualified_name(&attribute.name),
                                    value: attribute.value.clone(),
                                })
                                .collect(),
                        },
                    ));
                    append_extension_content(content, children)?;
                    content.push(stateforward::scxml::types::ExtensionContent::EndElement(
                        to_qualified_name(name),
                    ));
                }
            }
        }
        Ok(())
    }

    fn to_qualified_name(
        name: &scxml_compiler_core::ExpandedNameIr,
    ) -> stateforward::scxml::types::QualifiedName {
        stateforward::scxml::types::QualifiedName {
            namespace_uri: name.namespace_uri.clone(),
            local_name: name.local_name.clone(),
        }
    }

    /// The event `session.send` delivers: a valid event whose provenance an
    /// Event I/O Processor may carry (see [`super::check_sent_provenance`]).
    fn to_sent_event(event: WitEvent, start: SessionStart) -> Result<Event, ComponentError> {
        let event = to_event(event)?;
        check_sent_provenance(event.event_type, event.invoke_id.as_ref(), start)
            .map_err(|forged| error(ErrorCode::InvalidInput, forged.detail()))?;
        Ok(event)
    }

    /// The events an extension component queued through the host: each must be
    /// a valid event of the queue's own type, at most the engine's queue
    /// bound of them; anything else is a broken host contract (`protocol`).
    fn queued_events(
        events: Vec<WitEvent>,
        event_type: EventType,
    ) -> Result<Vec<Event>, ComponentError> {
        if events.len() > scxml_compiler_core::MAX_NESTED_ITEMS {
            return Err(error(
                ErrorCode::Protocol,
                "host queued more extension component events than the queue bound",
            ));
        }
        events
            .into_iter()
            .map(|event| {
                to_event(event)
                    .ok()
                    .filter(|event| event.event_type == event_type)
                    .ok_or_else(|| {
                        error(
                            ErrorCode::Protocol,
                            "host returned an invalid extension-queued event",
                        )
                    })
            })
            .collect()
    }

    fn to_event(event: WitEvent) -> Result<Event, ComponentError> {
        validate_wit_event(&event).map_err(boundary_error)?;
        Ok(Event {
            name: EventName::new(event.name)
                .map_err(|_| error(ErrorCode::InvalidInput, "event name is invalid"))?,
            event_type: match event.event_type {
                WitEventType::Internal => EventType::Internal,
                WitEventType::External => EventType::External,
                WitEventType::Platform => EventType::Platform,
            },
            send_id: event
                .send_id
                .map(scxml_types::SendId::new)
                .transpose()
                .map_err(|_| error(ErrorCode::InvalidInput, "event send id is invalid"))?,
            origin: event.origin,
            origin_type: event.origin_type,
            invoke_id: event
                .invoke_id
                .map(scxml_types::InvokeId::new)
                .transpose()
                .map_err(|_| error(ErrorCode::InvalidInput, "event invoke id is invalid"))?,
            data: to_native_value(event.data)?,
            raw: event.raw.clone(),
            trace_context: event
                .trace_context
                .map(to_native_trace_context)
                .transpose()?,
        })
    }

    /// A host-supplied carrier, validated and canonicalized.
    fn to_native_trace_context(
        context: stateforward::scxml::types::TraceContext,
    ) -> Result<EventTraceContext, ComponentError> {
        EventTraceContext::new(
            context.traceparent,
            context.tracestate,
            context.baggage,
            match context.baggage_policy {
                WitBaggagePolicy::Drop => BaggagePolicy::Drop,
                WitBaggagePolicy::Propagate => BaggagePolicy::Propagate,
            },
        )
        .map_err(|_| error(ErrorCode::InvalidInput, "trace context is invalid"))
    }

    /// A trigger carrier as the telemetry facade's parent: Level 2 flags
    /// the facade does not model are masked (see [`telemetry_traceparent`]).
    fn telemetry_context(context: &EventTraceContext) -> stateforward::scxml::types::TraceContext {
        let mut carrier = to_wit_trace_context(context);
        if let std::borrow::Cow::Owned(masked) = telemetry_traceparent(&context.traceparent) {
            carrier.traceparent = masked;
        }
        carrier
    }

    fn ids(states: &[StateId]) -> Vec<String> {
        states
            .iter()
            .map(|state| state.as_str().to_owned())
            .collect()
    }

    /// The WIT image of one engine snapshot plus the session's counters
    /// (both the next generated ordinal) and invoking parent.
    fn to_machine_snapshot(
        image: scxml_engine::EngineSnapshot,
        state: &SessionState,
    ) -> MachineSnapshot {
        MachineSnapshot {
            status: to_status(&image.status),
            name: image.name,
            atomic: ids(&image.atomic),
            configuration: ids(&image.configuration),
            internal: image.internal.into_iter().map(from_event).collect(),
            history: image
                .history
                .iter()
                .map(|history| WitHistoryValue {
                    state: history.state.as_str().to_owned(),
                    value: ids(&history.value),
                })
                .collect(),
            bound_states: ids(&image.bound_states),
            invocations: image
                .invocations
                .iter()
                .map(|invocation| WitInvocationSnapshot {
                    invoke_id: invocation.invoke_id.as_str().to_owned(),
                    state: invocation.state.as_str().to_owned(),
                    index: invocation.index,
                    type_uri: invocation.type_uri.clone(),
                    autoforward: invocation.autoforward,
                })
                .collect(),
            invoked_by: state.invoked().map(|invoked| InvokedBy {
                parent_session_id: invoked.parent_session_id.clone(),
                invoke_id: invoked.invoke_id.clone(),
            }),
            send_counter: state.send_counter,
            invoke_counter: state.invoke_counter,
            transitions: image
                .transitions
                .into_iter()
                .map(|transition| WitTransitionSnapshot {
                    source: transition.source.as_str().to_owned(),
                    index: transition.index,
                    targets: ids(&transition.targets),
                    type_: match transition.transition_type {
                        scxml_compiler_core::TransitionType::External => {
                            WitTransitionType::External
                        }
                        scxml_compiler_core::TransitionType::Internal => {
                            WitTransitionType::Internal
                        }
                    },
                    domain: transition
                        .domain
                        .as_ref()
                        .map(|domain| domain.as_str().to_owned()),
                    exit: ids(&transition.exit),
                    entry: ids(&transition.entry),
                    events: transition.events,
                    cond: transition.cond,
                    event_schema: transition.event_schema.map(|schema| match schema {
                        scxml_compiler_core::EventSchemaIr::Schema { schema, sha256 } => {
                            WitEventSchema::Schema(WitEventSchemaText { schema, sha256 })
                        }
                        scxml_compiler_core::EventSchemaIr::Schemaexpr { expr } => {
                            WitEventSchema::Schemaexpr(expr)
                        }
                    }),
                })
                .collect(),
        }
    }

    fn to_status(status: &EngineStatus) -> Status {
        match status {
            EngineStatus::Ready => Status::Ready,
            EngineStatus::Running => Status::Running,
            EngineStatus::Finished => Status::Finished,
        }
    }

    fn from_event(event: Event) -> WitEvent {
        WitEvent {
            name: event.name.as_str().to_owned(),
            event_type: match event.event_type {
                EventType::Internal => WitEventType::Internal,
                EventType::External => WitEventType::External,
                EventType::Platform => WitEventType::Platform,
            },
            send_id: event.send_id.map(|value| value.as_str().to_owned()),
            origin: event.origin,
            origin_type: event.origin_type,
            invoke_id: event.invoke_id.map(|value| value.as_str().to_owned()),
            data: from_native_value(event.data),
            raw: event.raw.clone(),
            trace_context: event.trace_context.as_ref().map(to_wit_trace_context),
        }
    }

    fn to_wit_trace_context(
        context: &scxml_types::TraceContext,
    ) -> stateforward::scxml::types::TraceContext {
        stateforward::scxml::types::TraceContext {
            traceparent: context.traceparent.clone(),
            tracestate: context.tracestate.clone(),
            baggage: context.baggage.clone(),
            baggage_policy: match context.baggage_policy {
                BaggagePolicy::Drop => WitBaggagePolicy::Drop,
                BaggagePolicy::Propagate => WitBaggagePolicy::Propagate,
            },
        }
    }

    fn to_wit_placement(placement: scxml_types::AssignPlacement) -> WitAssignPlacement {
        match placement {
            scxml_types::AssignPlacement::ReplaceChildren => WitAssignPlacement::ReplaceChildren,
            scxml_types::AssignPlacement::FirstChild => WitAssignPlacement::FirstChild,
            scxml_types::AssignPlacement::LastChild => WitAssignPlacement::LastChild,
            scxml_types::AssignPlacement::PreviousSibling => WitAssignPlacement::PreviousSibling,
            scxml_types::AssignPlacement::NextSibling => WitAssignPlacement::NextSibling,
            scxml_types::AssignPlacement::Replace => WitAssignPlacement::Replace,
            scxml_types::AssignPlacement::Delete => WitAssignPlacement::Delete,
            scxml_types::AssignPlacement::AddAttribute(name) => {
                WitAssignPlacement::AddAttribute(name)
            }
        }
    }

    fn to_native_value(value: WitDataValue) -> Result<scxml_types::DataValue, ComponentError> {
        match value {
            WitDataValue::Absent => Ok(scxml_types::DataValue::Absent),
            WitDataValue::Undefined => Ok(scxml_types::DataValue::Undefined),
            WitDataValue::Null => Ok(scxml_types::DataValue::Null),
            WitDataValue::Boolean(value) => Ok(scxml_types::DataValue::Bool(value)),
            WitDataValue::Number(value) => scxml_types::FiniteNumber::new(value)
                .map(scxml_types::DataValue::Number)
                .map_err(|_| error(ErrorCode::InvalidInput, "event number must be finite")),
            WitDataValue::Text(value) => Ok(scxml_types::DataValue::String(value)),
            WitDataValue::JsonArray(value) => scxml_types::DataValue::canonical_json_array(&value)
                .map_err(|_| error(ErrorCode::InvalidInput, "event JSON array is invalid")),
            WitDataValue::JsonObject(value) => {
                scxml_types::DataValue::canonical_json_object(&value)
                    .map_err(|_| error(ErrorCode::InvalidInput, "event JSON object is invalid"))
            }
            WitDataValue::Xml(value) => Ok(scxml_types::DataValue::Xml(value)),
        }
    }

    fn canonical_wit_value(value: WitDataValue) -> Result<WitDataValue, ComponentError> {
        let boundary = match &value {
            WitDataValue::Absent
            | WitDataValue::Undefined
            | WitDataValue::Null
            | WitDataValue::Boolean(_)
            | WitDataValue::Number(_) => BoundaryData::Empty,
            WitDataValue::Text(value) => BoundaryData::Text(value),
            WitDataValue::JsonArray(value) | WitDataValue::JsonObject(value) => {
                BoundaryData::Json(value)
            }
            WitDataValue::Xml(value) => BoundaryData::Xml(value),
        };
        if let BoundaryData::Text(value) | BoundaryData::Json(value) | BoundaryData::Xml(value) =
            boundary
        {
            let kind = match boundary {
                BoundaryData::Text(_) | BoundaryData::Empty => MachineInputKind::Text,
                BoundaryData::Json(_) => MachineInputKind::Json,
                BoundaryData::Xml(_) => MachineInputKind::Xml,
            };
            checked_value_bytes(value, super::MAX_DATA_VALUE_BYTES, kind)
                .map_err(boundary_error)?;
        }
        to_native_value(value).map(from_native_value)
    }

    fn from_native_value(value: scxml_types::DataValue) -> WitDataValue {
        match value {
            scxml_types::DataValue::Absent => WitDataValue::Absent,
            scxml_types::DataValue::Undefined => WitDataValue::Undefined,
            scxml_types::DataValue::Null => WitDataValue::Null,
            scxml_types::DataValue::Bool(value) => WitDataValue::Boolean(value),
            scxml_types::DataValue::Number(value) => WitDataValue::Number(value.get()),
            scxml_types::DataValue::String(value) => WitDataValue::Text(value),
            scxml_types::DataValue::JsonArray(value) => WitDataValue::JsonArray(value),
            scxml_types::DataValue::JsonObject(value) => WitDataValue::JsonObject(value),
            scxml_types::DataValue::Xml(value) => WitDataValue::Xml(value),
        }
    }

    fn measurement(kind: telemetry::MeasurementKind, value: u32) -> telemetry::Measurement {
        telemetry::Measurement {
            kind,
            value: u64::from(value),
        }
    }

    /// Maps a typed boundary failure to `invalid-input` with its static,
    /// payload-free detail, so the closed input class and limit kind survive.
    fn boundary_error(cause: MachineBoundaryError) -> ComponentError {
        error(ErrorCode::InvalidInput, cause.detail())
    }

    /// Maps a session lifecycle rejection to `protocol` with static detail.
    fn lifecycle_error(rejection: LifecycleRejection) -> ComponentError {
        error(ErrorCode::Protocol, rejection.detail())
    }

    /// Maps an engine failure to its documented closed code (see
    /// [`super::classify_engine_error`]), preserving the engine's static
    /// description when it fits the 160-byte detail bound.
    fn engine_error(cause: scxml_engine::EngineError) -> ComponentError {
        let detail = cause.to_string();
        let code = match super::classify_engine_error(cause) {
            super::EngineFailureClass::InvalidInput => ErrorCode::InvalidInput,
            super::EngineFailureClass::CapacityExhausted => ErrorCode::CapacityExhausted,
            super::EngineFailureClass::Protocol => ErrorCode::Protocol,
            super::EngineFailureClass::Execution => ErrorCode::Execution,
            super::EngineFailureClass::Unavailable => ErrorCode::Unavailable,
            super::EngineFailureClass::Platform => ErrorCode::Platform,
        };
        ComponentError {
            version: ErrorVersion::V1,
            code,
            detail: Some(if detail.len() <= 160 {
                detail
            } else {
                "engine execution failed".to_owned()
            }),
        }
    }

    fn error(code: ErrorCode, detail: &'static str) -> ComponentError {
        ComponentError {
            version: ErrorVersion::V1,
            code,
            detail: (detail.len() <= 160).then(|| detail.into()),
        }
    }

    fn metric(value: usize) -> u32 {
        u32::try_from(value).unwrap_or(u32::MAX)
    }

    fn wit_event_bytes(event: &WitEvent) -> usize {
        let mut total = event.name.len();
        for value in [
            event.send_id.as_deref(),
            event.origin.as_deref(),
            event.origin_type.as_deref(),
            event.invoke_id.as_deref(),
        ] {
            total = total.saturating_add(value.map_or(0, str::len));
        }
        total = total.saturating_add(match &event.data {
            WitDataValue::Absent | WitDataValue::Undefined | WitDataValue::Null => 0,
            WitDataValue::Boolean(_) => 1,
            WitDataValue::Number(_) => 8,
            WitDataValue::Text(value)
            | WitDataValue::JsonArray(value)
            | WitDataValue::JsonObject(value)
            | WitDataValue::Xml(value) => value.len(),
        });
        if let Some(context) = event.trace_context.as_ref() {
            total = total
                .saturating_add(context.traceparent.len())
                .saturating_add(context.tracestate.as_ref().map_or(0, String::len))
                .saturating_add(context.baggage.as_ref().map_or(0, String::len));
        }
        total
    }

    fn trace_bytes(event: &WitEvent) -> Result<usize, MachineBoundaryError> {
        let Some(context) = event.trace_context.as_ref() else {
            return Ok(0);
        };
        let mut total = checked_value_bytes(
            &context.traceparent,
            scxml_types::TRACEPARENT_BYTES,
            MachineInputKind::Metadata,
        )?;
        if let Some(value) = context.tracestate.as_deref() {
            let bytes = checked_value_bytes(
                value,
                scxml_types::MAX_TRACESTATE_BYTES,
                MachineInputKind::Metadata,
            )?;
            total = checked_add(
                total,
                bytes,
                super::MAX_EVENT_BYTES,
                MachineInputKind::Event,
            )?;
        }
        if let Some(value) = context.baggage.as_deref() {
            let bytes = checked_value_bytes(
                value,
                scxml_types::MAX_BAGGAGE_BYTES,
                MachineInputKind::Metadata,
            )?;
            total = checked_add(
                total,
                bytes,
                super::MAX_EVENT_BYTES,
                MachineInputKind::Event,
            )?;
        }
        Ok(total)
    }

    fn validate_wit_event(event: &WitEvent) -> Result<usize, MachineBoundaryError> {
        wit_boundary_event(event).and_then(validate_event_boundary)
    }

    fn wit_boundary_event(event: &WitEvent) -> Result<BoundaryEvent<'_>, MachineBoundaryError> {
        let data = match &event.data {
            WitDataValue::Absent
            | WitDataValue::Undefined
            | WitDataValue::Null
            | WitDataValue::Boolean(_)
            | WitDataValue::Number(_) => BoundaryData::Empty,
            WitDataValue::Text(value) => BoundaryData::Text(value),
            WitDataValue::JsonArray(value) | WitDataValue::JsonObject(value) => {
                BoundaryData::Json(value)
            }
            WitDataValue::Xml(value) => BoundaryData::Xml(value),
        };
        Ok(BoundaryEvent {
            name: &event.name,
            send_id: event.send_id.as_deref(),
            origin: event.origin.as_deref(),
            origin_type: event.origin_type.as_deref(),
            invoke_id: event.invoke_id.as_deref(),
            data,
            raw: event.raw.as_deref(),
            trace_bytes: trace_bytes(event)?,
        })
    }

    export!(Component);
}

/// Marker used by native tests to prove this package contains the engine.
#[must_use]
pub const fn embeds_engine() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use scxml_compiler_core::{Binding, InitialIr, MachineIr, StateIr, StateKind};

    #[test]
    fn machine_guest_declares_embedded_engine() {
        assert!(embeds_engine());
    }

    #[test]
    fn engine_failures_report_their_documented_wit_error_code_class() {
        use scxml_engine::{EngineError, EvaluationError};
        // WIT `error-code`: capacity bounds, lifecycle/protocol breaks,
        // unavailable semantics, and provider failures keep their own class
        // instead of collapsing into `execution`.
        let cases = [
            (EngineError::InvalidModel, EngineFailureClass::InvalidInput),
            (
                EngineError::UnsupportedModel,
                EngineFailureClass::Unavailable,
            ),
            (EngineError::AlreadyStarted, EngineFailureClass::Protocol),
            (EngineError::NotStarted, EngineFailureClass::Protocol),
            (EngineError::Finished, EngineFailureClass::Protocol),
            (EngineError::NotStable, EngineFailureClass::Protocol),
            (EngineError::BrokenInvariant, EngineFailureClass::Protocol),
            (EngineError::EffectProtocol, EngineFailureClass::Protocol),
            (EngineError::UnknownBlock, EngineFailureClass::Protocol),
            (
                EngineError::QueueLimit,
                EngineFailureClass::CapacityExhausted,
            ),
            (
                EngineError::EffectLimit,
                EngineFailureClass::CapacityExhausted,
            ),
            (
                EngineError::Evaluation(EvaluationError::InvalidInput),
                EngineFailureClass::Execution,
            ),
            (
                EngineError::Evaluation(EvaluationError::Execution),
                EngineFailureClass::Execution,
            ),
            (
                EngineError::Evaluation(EvaluationError::Unavailable),
                EngineFailureClass::Unavailable,
            ),
            (
                EngineError::Evaluation(EvaluationError::Platform),
                EngineFailureClass::Platform,
            ),
        ];
        for (error, class) in cases {
            assert_eq!(classify_engine_error(error), class, "{error:?}");
        }
    }

    fn two_state_ir() -> MachineIr {
        MachineIr {
            version: scxml_compiler_core::MACHINE_IR_VERSION,
            name: Some("loader".into()),
            data_model: None,
            binding: Binding::Early,
            initial: InitialIr {
                document_order: 0,
                targets: vec![0],
                actions: Vec::new(),
            },
            states: vec![
                StateIr {
                    document_order: 1,
                    id: Some("ready".into()),
                    kind: StateKind::Atomic,
                    parent: None,
                    children: Vec::new(),
                    initial: None,
                    onentry: Vec::new(),
                    onexit: Vec::new(),
                    transitions: vec![0],
                    data: Vec::new(),
                    invokes: Vec::new(),
                    donedata: None,
                },
                StateIr {
                    document_order: 3,
                    id: Some("done".into()),
                    kind: StateKind::Final,
                    parent: None,
                    children: Vec::new(),
                    initial: None,
                    onentry: Vec::new(),
                    onexit: Vec::new(),
                    transitions: Vec::new(),
                    data: Vec::new(),
                    invokes: Vec::new(),
                    donedata: None,
                },
            ],
            transitions: vec![scxml_compiler_core::TransitionIr {
                document_order: 2,
                source: 0,
                targets: vec![1],
                events: vec!["finish".into()],
                cond: None,
                transition_type: scxml_compiler_core::TransitionType::External,
                actions: Vec::new(),
                event_schema: None,
            }],
            data: Vec::new(),
            scripts: Vec::new(),
            extension_namespaces: Vec::new(),
        }
    }

    #[test]
    fn ir_loader_accepts_canonical_ir_and_builds_a_ready_engine() {
        let bytes = two_state_ir()
            .to_json()
            .expect("the loader fixture is valid canonical IR");
        let loaded = load_engine_from_ir(&bytes).expect("canonical IR loads");
        assert!(
            loaded.scripts.is_empty(),
            "the fixture has no external scripts"
        );
        assert_eq!(
            *loaded.engine.status(),
            scxml_engine::EngineStatus::Ready,
            "a loaded session starts ready"
        );
    }

    #[test]
    fn ir_loader_fails_closed_with_typed_causes_on_hostile_ir() {
        // The Machine IR byte bound applies to wasm builds only; the host's
        // component tests check it through the shipped machine
        // (`machine_ir_modes.rs`, `payload-too-large`).

        let mut future = two_state_ir();
        future.version += 1;
        let future = serde_json::to_vec(&future).expect("future-version fixture encodes");
        assert_eq!(
            load_engine_from_ir(&future).err(),
            Some(IrLoadError::UnsupportedVersion)
        );

        for malformed in [
            &b""[..],
            b"not json",
            b"[]",
            b"{\"version\": 3",
            b"{\"version\": \"3\"}",
        ] {
            assert_eq!(
                load_engine_from_ir(malformed).err(),
                Some(IrLoadError::Invalid),
                "malformed IR must be invalid input"
            );
        }

        let mut dangling = two_state_ir();
        dangling.transitions[0].targets = vec![u32::MAX];
        let dangling = serde_json::to_vec(&dangling).expect("dangling fixture encodes");
        assert_eq!(
            load_engine_from_ir(&dangling).err(),
            Some(IrLoadError::Invalid),
            "semantically invalid IR must be rejected before engine use"
        );

        let mut unknown_field =
            serde_json::to_value(two_state_ir()).expect("fixture converts to JSON");
        unknown_field["unexpected"] = serde_json::Value::Bool(true);
        let unknown_field = serde_json::to_vec(&unknown_field).expect("fixture encodes");
        assert_eq!(
            load_engine_from_ir(&unknown_field).err(),
            Some(IrLoadError::Invalid),
            "unknown IR fields fail closed"
        );

        // An id-less history state is valid Machine IR outside the engine
        // slice (lowering always generates an id, SCXML 3.14).
        let mut outside_slice = two_state_ir();
        let template = outside_slice.states[0].clone();
        outside_slice.states[0].kind = StateKind::Compound;
        outside_slice.states[0].children = vec![2, 3];
        outside_slice.states[0].initial = Some(InitialIr {
            document_order: 4,
            targets: vec![3],
            actions: Vec::new(),
        });
        outside_slice.states.push(StateIr {
            document_order: 5,
            id: None,
            kind: StateKind::ShallowHistory,
            parent: Some(0),
            transitions: Vec::new(),
            ..template.clone()
        });
        outside_slice.states.push(StateIr {
            document_order: 6,
            id: Some("inner".into()),
            parent: Some(0),
            transitions: Vec::new(),
            ..template
        });
        assert_eq!(
            outside_slice.validate(),
            Ok(()),
            "the fixture is valid Machine IR"
        );
        let outside_slice =
            serde_json::to_vec(&outside_slice).expect("outside-slice fixture encodes");
        assert_eq!(
            load_engine_from_ir(&outside_slice).err(),
            Some(IrLoadError::Unsupported),
            "valid IR outside the engine slice is unavailable, not invalid"
        );
    }

    #[test]
    fn ir_load_error_details_are_static_bounded_and_distinct() {
        let details = [
            IrLoadError::TooLarge,
            IrLoadError::UnsupportedVersion,
            IrLoadError::Invalid,
            IrLoadError::Unsupported,
        ]
        .map(|error| error.detail());
        for detail in details {
            assert!(!detail.is_empty() && detail.len() <= 160);
        }
        let mut unique = details.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), details.len());
    }

    #[test]
    fn send_accepts_only_external_events_and_autoforwarded_invoke_ids() {
        use scxml_types::EventType::{External, Internal, Platform};
        let invoke_id = scxml_types::InvokeId::new("k").expect("a valid invoke id");
        for event_type in [External] {
            for start in [SessionStart::Root, SessionStart::Invoked] {
                assert_eq!(check_sent_provenance(event_type, None, start), Ok(()));
            }
            assert_eq!(
                check_sent_provenance(event_type, Some(&invoke_id), SessionStart::Root),
                Err(ForgedProvenance::InvokeId),
                "a root session never receives an invoke id through send"
            );
            assert_eq!(
                check_sent_provenance(event_type, Some(&invoke_id), SessionStart::Invoked),
                Ok(()),
                "an invoked session receives its parent's autoforwarded copies"
            );
        }
        for id in [None, Some(&invoke_id)] {
            for start in [SessionStart::Root, SessionStart::Invoked] {
                for event_type in [Internal, Platform] {
                    assert_eq!(
                        check_sent_provenance(event_type, id, start),
                        Err(ForgedProvenance::NotExternal),
                        "only the machine raises internal events, and platform errors go through communication-error"
                    );
                }
            }
        }
        for forged in [ForgedProvenance::NotExternal, ForgedProvenance::InvokeId] {
            assert!(!forged.detail().is_empty() && forged.detail().len() <= 160);
        }
    }

    #[test]
    fn lifecycle_gate_rejects_terminal_sessions_and_misordered_commands() {
        use scxml_engine::EngineStatus;

        let ready = Some((&EngineStatus::Ready, Stability::Stable));
        let running = Some((&EngineStatus::Running, Stability::Stable));
        let pending = Some((&EngineStatus::Running, Stability::NotStable));
        let finished = Some((&EngineStatus::Finished, Stability::Stable));
        assert_eq!(
            gate_command(SessionLifecycle::Active, pending, SessionCommand::Send),
            Err(LifecycleRejection::NotStable),
            "send after a bound-stopped call must resume first"
        );
        assert_eq!(
            gate_command(SessionLifecycle::Active, pending, SessionCommand::Resume),
            Ok(())
        );
        assert_eq!(
            gate_command(SessionLifecycle::Active, ready, SessionCommand::Start),
            Ok(())
        );
        assert_eq!(
            gate_command(SessionLifecycle::Active, pending, SessionCommand::Cancel),
            Ok(()),
            "cancel exits a running session even mid-bound"
        );
        for command in [
            SessionCommand::Send,
            SessionCommand::Resume,
            SessionCommand::Cancel,
        ] {
            assert_eq!(
                gate_command(SessionLifecycle::Active, running, command),
                Ok(())
            );
            assert_eq!(
                gate_command(SessionLifecycle::Active, ready, command),
                Err(LifecycleRejection::NotStarted)
            );
            assert_eq!(
                gate_command(SessionLifecycle::Active, finished, command),
                Err(LifecycleRejection::Finished)
            );
        }
        for status in [running, finished] {
            assert_eq!(
                gate_command(SessionLifecycle::Active, status, SessionCommand::Start),
                Err(LifecycleRejection::AlreadyStarted)
            );
        }
        for command in [
            SessionCommand::Start,
            SessionCommand::Send,
            SessionCommand::Resume,
            SessionCommand::Cancel,
        ] {
            for status in [None, ready, running, finished] {
                assert_eq!(
                    gate_command(SessionLifecycle::Failed, status, command),
                    Err(LifecycleRejection::Failed),
                    "failed sessions reject every command"
                );
                assert_eq!(
                    gate_command(SessionLifecycle::ShutDown, status, command),
                    Err(LifecycleRejection::ShutDown),
                    "shut-down sessions reject every command"
                );
            }
            assert_eq!(
                gate_command(SessionLifecycle::Active, None, command),
                Err(LifecycleRejection::Failed),
                "an active session without an engine is treated as failed"
            );
        }
        for rejection in [
            LifecycleRejection::Failed,
            LifecycleRejection::ShutDown,
            LifecycleRejection::AlreadyStarted,
            LifecycleRejection::NotStarted,
            LifecycleRejection::Finished,
            LifecycleRejection::NotStable,
        ] {
            assert!(!rejection.detail().is_empty() && rejection.detail().len() <= 160);
        }
    }

    /// WIT `call-result.suspended`: a suspended call admits only
    /// `invocation-completed`, which needs a suspended call; both
    /// rejections are `protocol` rejections that change nothing.
    #[test]
    fn a_suspended_call_admits_only_its_continuation() {
        use scxml_engine::EngineStatus;

        let running = Some((&EngineStatus::Running, Stability::NotStable));
        let finished = Some((&EngineStatus::Finished, Stability::Stable));
        for engine in [running, finished] {
            assert_eq!(
                gate_session(
                    SessionLifecycle::Active,
                    engine,
                    LastCall::Suspended,
                    SessionCommand::Continue
                ),
                Ok(()),
                "a suspended exit continues whatever the status"
            );
        }
        for command in [
            SessionCommand::Start,
            SessionCommand::Send,
            SessionCommand::Resume,
            SessionCommand::Cancel,
        ] {
            assert_eq!(
                gate_session(
                    SessionLifecycle::Active,
                    running,
                    LastCall::Suspended,
                    command
                ),
                Err(LifecycleRejection::Suspended),
                "{command:?}"
            );
        }
        assert_eq!(
            gate_session(
                SessionLifecycle::Active,
                Some((&EngineStatus::Running, Stability::Stable)),
                LastCall::Settled,
                SessionCommand::Continue
            ),
            Err(LifecycleRejection::NotSuspended)
        );
        for rejection in [
            LifecycleRejection::Suspended,
            LifecycleRejection::NotSuspended,
        ] {
            assert!(!rejection.detail().is_empty() && rejection.detail().len() <= 160);
        }
    }

    #[test]
    fn snapshot_gate_reads_only_checkpoints_of_live_sessions() {
        use scxml_engine::EngineStatus;

        for status in [
            (&EngineStatus::Ready, Stability::Stable),
            (&EngineStatus::Running, Stability::Stable),
            (&EngineStatus::Finished, Stability::Stable),
            (&EngineStatus::Finished, Stability::NotStable),
        ] {
            assert_eq!(
                gate_snapshot(SessionLifecycle::Active, Some(status)),
                Ok(())
            );
        }
        assert_eq!(
            gate_snapshot(
                SessionLifecycle::Active,
                Some((&EngineStatus::Running, Stability::NotStable))
            ),
            Err("not-stable"),
            "a call stopped at the command bound is not a checkpoint"
        );
        for status in [None, Some((&EngineStatus::Running, Stability::Stable))] {
            assert_eq!(
                gate_snapshot(SessionLifecycle::Failed, status),
                Err(LifecycleRejection::Failed.detail())
            );
            assert_eq!(
                gate_snapshot(SessionLifecycle::ShutDown, status),
                Err(LifecycleRejection::ShutDown.detail())
            );
        }
        assert_eq!(
            gate_snapshot(SessionLifecycle::Active, None),
            Err(LifecycleRejection::Failed.detail())
        );
    }

    #[test]
    fn boundary_error_details_are_static_bounded_and_distinct() {
        let kinds = [
            MachineInputKind::Name,
            MachineInputKind::Metadata,
            MachineInputKind::Text,
            MachineInputKind::Json,
            MachineInputKind::Xml,
            MachineInputKind::Raw,
            MachineInputKind::Event,
        ];
        let mut details = std::collections::BTreeSet::new();
        for kind in kinds {
            for error in [
                MachineBoundaryError::InvalidValue(kind),
                MachineBoundaryError::ValueTooLarge(kind),
            ] {
                let detail = error.detail();
                assert!(!detail.is_empty() && detail.len() <= 160, "{error:?}");
                assert_eq!(error.to_string(), detail);
                assert!(details.insert(detail), "detail must identify {error:?}");
            }
        }
    }

    /// Joins WIT `///` doc lines into single-spaced prose for phrase checks.
    fn wit_doc_text(wit: &str) -> String {
        wit.lines()
            .map(str::trim)
            .filter_map(|line| line.strip_prefix("///"))
            .flat_map(str::split_whitespace)
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn documented_wit_bounds_match_enforced_constants() {
        // `scxml.wit` documents the wasm machine's bound; natively it is unbounded.
        const DOCUMENTED_MAX_IR_BYTES: usize = 16 * 1024 * 1024;
        assert_eq!(
            scxml_compiler_core::MAX_IR_BYTES,
            scxml_types::wasm_bound(DOCUMENTED_MAX_IR_BYTES)
        );
        let docs = wit_doc_text(include_str!(
            "../../../crates/scxml-wit/wit/scxml/scxml.wit"
        ));
        for phrase in [
            format!(
                "`invoke-id` at most {} bytes each",
                scxml_types::MAX_NAME_BYTES
            ),
            format!("`origin-type` at most {MAX_METADATA_BYTES} bytes each"),
            format!("`raw` at most {MAX_DATA_VALUE_BYTES} bytes each"),
            format!("context, at most {MAX_EVENT_BYTES} bytes"),
            format!(
                "or over {} bytes) starts in `failed` with `invalid-input`",
                scxml_types::MAX_NAME_BYTES
            ),
            format!("rejects more than {DOCUMENTED_MAX_IR_BYTES} bytes with `payload-too-large`"),
        ] {
            assert!(
                docs.contains(&phrase),
                "scxml.wit must document the enforced bound: {phrase:?}"
            );
        }
    }

    #[test]
    fn event_boundary_enforces_value_and_aggregate_byte_limits() {
        let oversized_send_id = "s".repeat(scxml_types::MAX_NAME_BYTES + 1);
        let invalid_send_id = BoundaryEvent {
            name: "event",
            send_id: Some(&oversized_send_id),
            origin: None,
            origin_type: None,
            invoke_id: None,
            data: BoundaryData::Empty,
            raw: None,
            trace_bytes: 0,
        };
        assert_eq!(
            validate_event_boundary(invalid_send_id),
            Err(MachineBoundaryError::ValueTooLarge(MachineInputKind::Name))
        );

        let oversized_text = "x".repeat(MAX_DATA_VALUE_BYTES + 1);
        let oversized = BoundaryEvent {
            name: "event",
            send_id: None,
            origin: None,
            origin_type: None,
            invoke_id: None,
            data: BoundaryData::Text(&oversized_text),
            raw: None,
            trace_bytes: 0,
        };
        assert_eq!(
            validate_event_boundary(oversized),
            Err(MachineBoundaryError::ValueTooLarge(MachineInputKind::Text))
        );

        let metadata = "m".repeat(MAX_METADATA_BYTES);
        let data = "d".repeat(MAX_DATA_VALUE_BYTES);
        let aggregate = BoundaryEvent {
            name: "event",
            send_id: None,
            origin: Some(&metadata),
            origin_type: Some(&metadata),
            invoke_id: None,
            data: BoundaryData::Xml(&data),
            raw: None,
            trace_bytes: 0,
        };
        assert_eq!(
            validate_event_boundary(aggregate),
            Err(MachineBoundaryError::AggregateTooLarge(
                MachineInputKind::Event
            ))
        );
    }

    #[test]
    fn event_boundary_bounds_raw_field() {
        let oversized_raw = "r".repeat(MAX_DATA_VALUE_BYTES + 1);
        let oversized = BoundaryEvent {
            name: "event",
            send_id: None,
            origin: None,
            origin_type: None,
            invoke_id: None,
            data: BoundaryData::Empty,
            raw: Some(&oversized_raw),
            trace_bytes: 0,
        };
        assert_eq!(
            validate_event_boundary(oversized),
            Err(MachineBoundaryError::ValueTooLarge(MachineInputKind::Raw))
        );
        let at_limit = "r".repeat(MAX_DATA_VALUE_BYTES);
        assert!(
            validate_event_boundary(BoundaryEvent {
                raw: Some(&at_limit),
                ..oversized
            })
            .is_ok(),
            "raw at the shared portable data bound must be accepted"
        );

        // A within-limit raw still counts against the aggregate event budget.
        let metadata = "m".repeat(MAX_METADATA_BYTES);
        let data = "d".repeat(MAX_DATA_VALUE_BYTES);
        let aggregate = BoundaryEvent {
            name: "event",
            send_id: None,
            origin: Some(&metadata),
            origin_type: None,
            invoke_id: None,
            data: BoundaryData::Json(&data),
            raw: Some(&metadata),
            trace_bytes: 0,
        };
        assert_eq!(
            validate_event_boundary(aggregate),
            Err(MachineBoundaryError::AggregateTooLarge(
                MachineInputKind::Event
            ))
        );
    }

    #[test]
    fn child_event_context_replaces_parent_identity_and_preserves_baggage_policy() {
        let parent = scxml_types::TraceContext::new(
            "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01",
            Some("vendor=value".into()),
            Some("tenant=example".into()),
            scxml_types::BaggagePolicy::Propagate,
        )
        .expect("parent trace fixture is canonical");
        let child = scxml_telemetry::TraceContext {
            traceparent: Some("00-0123456789abcdef0123456789abcdef-fedcba9876543210-01".into()),
            tracestate: Some("vendor=value".into()),
        };

        let propagated = child_event_trace_context(Some(child), Some(&parent))
            .expect("child carrier is accepted")
            .expect("parent context produces an outbound carrier");
        assert_ne!(propagated.traceparent, parent.traceparent);
        assert_eq!(propagated.baggage, parent.baggage);
        assert_eq!(propagated.baggage_policy, parent.baggage_policy);
        assert_eq!(propagated.validate(), Ok(()));
    }

    /// A non-recording provider (the OpenTelemetry JS API without an SDK)
    /// answers the parent's own span: that is no span of the batch's own,
    /// so the trigger's carrier passes through instead of failing the
    /// batch.
    #[test]
    fn child_event_context_passes_the_parent_through_an_echoed_identity() {
        let parent = scxml_types::TraceContext::new(
            "00-0123456789abcdef0123456789abcdef-0123456789abcdef-03",
            None,
            Some("tenant=example".into()),
            scxml_types::BaggagePolicy::Propagate,
        )
        .expect("parent trace fixture is canonical");
        // The facade saw the masked flags (`01`) and echoed them.
        let echoed = scxml_telemetry::TraceContext {
            traceparent: Some("00-0123456789abcdef0123456789abcdef-0123456789abcdef-01".into()),
            tracestate: None,
        };
        assert_eq!(
            child_event_trace_context(Some(echoed), Some(&parent)),
            Ok(Some(parent.clone()))
        );
        let malformed = scxml_telemetry::TraceContext {
            traceparent: Some("not-a-traceparent".into()),
            tracestate: None,
        };
        assert_eq!(
            child_event_trace_context(Some(malformed), Some(&parent)),
            Ok(Some(parent))
        );
    }

    /// Without a trigger carrier the batch span still parents every event
    /// the batch emits; there is no baggage to carry.
    #[test]
    fn child_event_context_without_a_trigger_names_the_batch_span() {
        let child = scxml_telemetry::TraceContext {
            traceparent: Some("00-0123456789abcdef0123456789abcdef-fedcba9876543210-01".into()),
            tracestate: Some("vendor=value".into()),
        };
        let outbound = child_event_trace_context(Some(child), None)
            .expect("a batch carrier is accepted")
            .expect("the batch span produces an outbound carrier");
        assert_eq!(
            outbound.traceparent,
            "00-0123456789abcdef0123456789abcdef-fedcba9876543210-01"
        );
        assert_eq!(outbound.tracestate.as_deref(), Some("vendor=value"));
        assert_eq!(outbound.baggage, None);
        assert_eq!(outbound.baggage_policy, scxml_types::BaggagePolicy::Drop);
        assert_eq!(child_event_trace_context(None, None), Ok(None));
    }

    /// Level 2: the facade models only `sampled`, so the random-trace-id
    /// flag is masked for it and restored on the outbound carrier of the
    /// same trace.
    #[test]
    fn level_two_random_trace_id_flag_survives_the_batch_span() {
        assert_eq!(
            telemetry_traceparent("00-0123456789abcdef0123456789abcdef-0123456789abcdef-03"),
            "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01"
        );
        assert_eq!(
            telemetry_traceparent("00-0123456789abcdef0123456789abcdef-0123456789abcdef-02"),
            "00-0123456789abcdef0123456789abcdef-0123456789abcdef-00"
        );
        let canonical = "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01";
        assert!(matches!(
            telemetry_traceparent(canonical),
            std::borrow::Cow::Borrowed(value) if value == canonical
        ));
        let parent = scxml_types::TraceContext::new(
            "00-0123456789abcdef0123456789abcdef-0123456789abcdef-02",
            None,
            None,
            scxml_types::BaggagePolicy::Drop,
        )
        .expect("Level 2 parent fixture is canonical");
        let child = scxml_telemetry::TraceContext {
            traceparent: Some("00-0123456789abcdef0123456789abcdef-fedcba9876543210-00".into()),
            tracestate: None,
        };
        assert_eq!(
            child_event_trace_context(Some(child), Some(&parent))
                .expect("child carrier is accepted")
                .map(|context| context.traceparent),
            Some("00-0123456789abcdef0123456789abcdef-fedcba9876543210-02".to_owned())
        );
        // A span in another trace keeps its own flags.
        let other = scxml_telemetry::TraceContext {
            traceparent: Some("00-ffffffffffffffffffffffffffffffff-fedcba9876543210-01".into()),
            tracestate: None,
        };
        assert_eq!(
            child_event_trace_context(Some(other), Some(&parent))
                .expect("child carrier is accepted")
                .map(|context| context.traceparent),
            Some("00-ffffffffffffffffffffffffffffffff-fedcba9876543210-01".to_owned())
        );
    }

    #[test]
    fn child_event_context_preserves_parent_when_telemetry_is_unavailable() {
        let parent = scxml_types::TraceContext::new(
            "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01",
            Some("vendor=value".into()),
            Some("tenant=example".into()),
            scxml_types::BaggagePolicy::Propagate,
        )
        .expect("parent trace fixture is canonical");

        assert_eq!(
            child_event_trace_context(None, Some(&parent)),
            Ok(Some(parent))
        );
    }
}
