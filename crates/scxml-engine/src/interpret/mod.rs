//! SCXML effect semantics over an explicit session capability boundary.
//!
//! [`Engine::run`] owns transition selection, the effect-transaction
//! protocol, and block-scoped recovery; this module owns what every planned
//! [`Effect`] *means*: `<data>` binding (SCXML 5.2-5.3), `<send>` evaluation
//! order, target classification, send-id generation, and error precedence
//! (SCXML 6.2, C.1, C.2), `<cancel>` (6.3), `<log>` (4.7), `<foreach>`
//! (4.6), `<donedata>` (5.5, 6.5), and the invocation lifecycle of an
//! invoking session (6.4). The machine component executes it over its WIT
//! `machine-capabilities` imports ([`SessionCapabilities`]); no host ever
//! re-implements any of it.
//!
//! One session is driven by:
//!
//! 1. [`load_extensions`] (from `start` or `start-invoked`), which asks the
//!    host to load the extension components the document uses, then
//!    [`bind_initial_data`], which initializes the data model, values every early-bound declaration (a
//!    top-level one an invoking parent passed a parameter for takes that
//!    value instead, SCXML 6.4.3), and queues the startup `error.execution`
//!    events;
//! 2. [`Engine::run`] with an [`Interpreter`] as its
//!    [`EffectExecutor`](crate::EffectExecutor) for every command.
//!
//! Every recoverable SCXML failure surfaces as the engine's block-scoped
//! `error.execution`/`error.communication`; every other failure is fatal
//! and reaches the session owner either as the capability's own error or as
//! an [`InterpretFailure`] mapped through [`SessionCapabilities::failure`].
//! No function here emits telemetry: the enclosing session boundary span
//! covers the work.

mod capabilities;
mod data;
mod invoke;
mod send;

pub use capabilities::{
    BasicHttpSend, CapabilityErrorKind, DocumentKind, ExtensionExecution, ExtensionSnapshot,
    ForeachItems, InlineBody, Invocation, InvocationProgress, InvokeRequest, InvokedSession,
    LoadedDocument, Observation, SendDisposition, SendTarget, SessionCapabilities,
};
pub use data::{bind_initial_data, load_extensions, requires_data_model};

use scxml_compiler_core::XmlNodeIr;
use scxml_types::{AssignPlacement, AssignSource, DataValue, Event, EventName, TraceContext};

use crate::{
    BatchEnd, Effect, EffectFailure, Engine, EngineError, EvaluationError, GuardEvaluator,
    PlannedEffect, PlatformError, RunStage, StateId,
};

/// What a failed `<donedata>` payload does besides yielding no data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DonedataErrors {
    /// Queue `error.execution` in this session.
    Raise,
    /// Queue nothing: the session has finished and could never process it.
    Drop,
}

/// Disposition class of one interpreter-originated fatal failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureCategory {
    /// A contract between the interpreter, the engine, and the host broke.
    Harness,
    /// The document uses semantics outside the supported slice.
    Unsupported,
    /// SCXML execution failed in a way the session cannot recover from.
    Execution,
    /// A run-wide budget was exhausted.
    Budget,
}

/// One fatal failure the interpreter or the engine originated.
///
/// `code` is a stable, static, payload-free reason code (at most 64 bytes)
/// the session owner may report verbatim; `engine` carries the engine's own
/// failure when the engine originated it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterpretFailure {
    /// Disposition class.
    pub category: FailureCategory,
    /// Stable reason code.
    pub code: &'static str,
    /// The engine failure that caused this one, if any.
    pub engine: Option<EngineError>,
}

impl InterpretFailure {
    /// An interpreter-originated failure.
    #[must_use]
    pub const fn new(category: FailureCategory, code: &'static str) -> Self {
        Self {
            category,
            code,
            engine: None,
        }
    }

    /// An engine-originated failure at `code`.
    #[must_use]
    pub const fn engine(error: EngineError, code: &'static str) -> Self {
        Self {
            category: FailureCategory::Harness,
            code,
            engine: Some(error),
        }
    }
}

/// Context of a session started by an invoking SCXML session (SCXML 6.4;
/// WIT `invocation-context` without its parameters, which bind at start).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvocationContext {
    /// `_sessionid` of the invoking parent session.
    pub parent_session_id: String,
    /// Invoke id the parent assigned to this session.
    pub invoke_id: String,
}

/// Interpreter state one session owns across engine runs.
///
/// Invariant: `session_id` is the session's `_sessionid`, fixed at
/// construction. Counters only grow; they back generated send ids
/// (`sendid_<n>`, SCXML 6.2.4) and generated invoke ids
/// (`<state>.<session>.<n>`, SCXML 6.4.1).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionState {
    session_id: String,
    /// Next generated send-id ordinal.
    pub send_counter: u64,
    /// Next generated invoke-id ordinal.
    pub invoke_counter: u64,
    invoked: Option<InvocationContext>,
    return_data: Option<DataValue>,
    /// The invocation request a suspended run waits on, with the carriers
    /// its open batch had.
    deferred: Option<DeferredEffect>,
}

/// The `<invoke>` start or cancel a suspended run waits on (SCXML 6.4),
/// with the interpreter's carriers of the batch the run suspended in.
#[derive(Clone, Debug, Eq, PartialEq)]
struct DeferredEffect {
    request: DeferredRequest,
    outbound: Option<TraceContext>,
    last_batch: Option<TraceContext>,
}

/// One deferred invocation request.
#[derive(Clone, Debug, Eq, PartialEq)]
enum DeferredRequest {
    /// The start of the invocation `invoke_id` of `state`'s `<invoke>` at
    /// `document_order`.
    Start {
        state: StateId,
        document_order: u32,
        invoke_id: scxml_types::InvokeId,
        /// The started invocation's type URI (the canonical SCXML one for a
        /// session).
        type_uri: String,
    },
    /// The cancel of an invocation.
    Cancel,
}

impl SessionState {
    /// Fresh state of the session `session_id`.
    #[must_use]
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            send_counter: 0,
            invoke_counter: 0,
            invoked: None,
            return_data: None,
            deferred: None,
        }
    }

    /// Whether a run of this session suspended at an invocation request
    /// the host carries out later.
    #[must_use]
    pub const fn is_deferred(&self) -> bool {
        self.deferred.is_some()
    }

    /// The session's `_sessionid`.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Marks this session as started by an invoking parent.
    pub fn set_invoked(&mut self, context: InvocationContext) {
        self.invoked = Some(context);
    }

    /// The invoking parent's context, for a session started by one.
    #[must_use]
    pub const fn invoked(&self) -> Option<&InvocationContext> {
        self.invoked.as_ref()
    }

    /// Takes the evaluated top-level `<donedata>` of an invoked session
    /// that finished during the last run (W3C `returnDoneEvent`; `undefined`
    /// when evaluation failed). `None` for every other run.
    pub fn take_return_data(&mut self) -> Option<DataValue> {
        self.return_data.take()
    }

    /// Allocates the next generated send id (`sendid_<n>`).
    fn next_send_id(&mut self) -> String {
        let id = format!("sendid_{}", self.send_counter);
        self.send_counter = self.send_counter.saturating_add(1);
        id
    }

    /// Allocates the next generated invoke id (`<state>.<session>.<n>`)
    /// for an `<invoke>` in `state`.
    fn next_invoke_id(&mut self, state: &StateId) -> String {
        let id = format!(
            "{}.{}.{}",
            state.as_str(),
            self.session_id,
            self.invoke_counter
        );
        self.invoke_counter = self.invoke_counter.saturating_add(1);
        id
    }
}

/// [`EffectExecutor`](crate::EffectExecutor) of one session run: guards and
/// effects go through `caps`, SCXML semantics are decided here.
pub struct Interpreter<'a, C: SessionCapabilities> {
    caps: &'a mut C,
    state: &'a mut SessionState,
    /// Trace carrier the open batch stamps on raised, sent, forwarded, and
    /// failure events.
    outbound: Option<TraceContext>,
    /// Trace carrier of the last batch this run opened; failed conditions
    /// evaluated after it carry it (see [`GuardEvaluator::last_batch_trace_context`]).
    last_batch: Option<TraceContext>,
    /// The provider's own error for the last fatal guard failure, so the
    /// session reports the exact error the provider returned.
    guard_failure: Option<C::Error>,
    /// The effect just applied asked to suspend the run.
    suspend: bool,
    /// The deferred invocation request's result, for a continued run.
    deferred_result: Option<Result<(), C::Error>>,
}

impl<'a, C: SessionCapabilities> Interpreter<'a, C> {
    /// An interpreter for one [`Engine::run`] call.
    pub fn new(caps: &'a mut C, state: &'a mut SessionState) -> Self {
        Self {
            caps,
            state,
            outbound: None,
            last_batch: None,
            guard_failure: None,
            suspend: false,
            deferred_result: None,
        }
    }

    /// An interpreter for the [`crate::Command::Continue`] run that
    /// resumes a run suspended at a deferred invocation request: `result`
    /// is the request's result, settled as the request's own call result
    /// would have been, and the batch's carriers are restored.
    pub fn continuing(
        caps: &'a mut C,
        state: &'a mut SessionState,
        result: Result<(), C::Error>,
    ) -> Self {
        let (outbound, last_batch) = state.deferred.as_ref().map_or((None, None), |deferred| {
            (deferred.outbound.clone(), deferred.last_batch.clone())
        });
        Self {
            caps,
            state,
            outbound,
            last_batch,
            guard_failure: None,
            suspend: false,
            deferred_result: Some(result),
        }
    }

    /// Suspends the run right after the effect being applied, waiting on
    /// `request`.
    fn defer(&mut self, request: DeferredRequest) {
        self.state.deferred = Some(DeferredEffect {
            request,
            outbound: None,
            last_batch: None,
        });
        self.suspend = true;
    }

    /// A fatal interpreter-originated failure.
    fn fatal(&mut self, category: FailureCategory, code: &'static str) -> EffectFailure<C::Error> {
        EffectFailure::Fatal(self.caps.failure(InterpretFailure::new(category, code)))
    }

    /// A fatal failure of one engine callback made while applying an effect.
    fn engine_fatal(&mut self, error: EngineError, code: &'static str) -> EffectFailure<C::Error> {
        EffectFailure::Fatal(self.caps.failure(InterpretFailure::engine(error, code)))
    }

    /// Queues one platform error without aborting the current block (an
    /// invocation or `<donedata>` failure, SCXML 6.4/6.5).
    fn queue_error(
        &mut self,
        engine: &mut Engine,
        error: PlatformError,
    ) -> Result<(), EffectFailure<C::Error>> {
        engine
            .queue_platform_error(error, self.outbound.clone())
            .map_err(|cause| self.engine_fatal(cause, "engine-queue-platform-error"))
    }

    /// Raises one internal event through the planned-raise protocol, with
    /// the batch's trace carrier.
    fn raise(
        &mut self,
        engine: &mut Engine,
        name: &str,
        data: DataValue,
    ) -> Result<(), EffectFailure<C::Error>> {
        let Ok(name) = EventName::new(name.to_owned()) else {
            return Err(self.fatal(FailureCategory::Harness, "raise-event-invalid"));
        };
        let mut event = Event::internal(name);
        event.data = data;
        event.trace_context.clone_from(&self.outbound);
        engine
            .raise(event)
            .map_err(|cause| self.engine_fatal(cause, "engine-raise"))
    }

    /// Applies one planned effect.
    #[allow(
        clippy::too_many_lines,
        reason = "one exhaustive effect match keeps every capability mapping auditable together"
    )]
    fn apply(
        &mut self,
        engine: &mut Engine,
        effect: &Effect,
        event: Option<&Event>,
    ) -> Result<(), EffectFailure<C::Error>> {
        let context = event.and_then(|event| event.trace_context.as_ref());
        match effect {
            // SCXML 4.7: the data model renders the log text; no `expr`
            // logs empty text.
            Effect::Log { label, expression } => {
                let text = match expression {
                    Some(expression) => self
                        .caps
                        .evaluate_log(expression, event)
                        .map_err(recover::<C>)?,
                    None => String::new(),
                };
                self.caps
                    .log(label.as_deref(), &text, context)
                    .map_err(recover::<C>)?;
            }
            Effect::Raise(name, authored, own_schema) => {
                // A `<raise>` creates its event with no data; one its
                // contracts reject is not raised, and error.execution ends
                // the block instead (SCXML 3.12.2, 4.2; namespaces/event).
                if *authored
                    && !engine.admits_event_data(
                        name,
                        &DataValue::Absent,
                        own_schema.as_ref(),
                        true,
                    )
                {
                    return Err(EffectFailure::Block(PlatformError::Execution));
                }
                self.raise(engine, name, DataValue::Absent)?;
                self.caps.observe(Observation::Raised(name));
            }
            // SCXML 6.5: `<donedata>` is the done event's data; a failure
            // to evaluate it queues error.execution and the done event is
            // still raised with undefined data (official 488/528).
            Effect::DoneEvent {
                event: name,
                donedata,
            } => {
                let payload = self
                    .donedata_payload(engine, donedata, event, DonedataErrors::Raise)?
                    .unwrap_or(DataValue::Undefined);
                self.raise(engine, name, payload)?;
            }
            // W3C exitInterpreter/returnDoneEvent (SCXML 6.4): an invoked
            // session's top-level donedata becomes the data of the
            // done.invoke its parent receives (undefined on failure). SCXML
            // 5.5: otherwise "the SCXML Processor SHOULD return the data to
            // the environment in an implementation-dependent manner": the
            // machine returns it on the call that finishes the session either way.
            Effect::ReturnDoneEvent(value) => {
                let payload = self.donedata_payload(engine, value, event, DonedataErrors::Drop)?;
                self.state.return_data = Some(payload.unwrap_or(DataValue::Undefined));
            }
            Effect::Assign {
                location,
                expression,
                content,
                assign_type,
                attr,
            } => {
                // SCXML 5.4 (and §C.3.6 of the 2014-03-13 CR for XPath
                // placements): the data model evaluates `expr` and assigns
                // in one call.
                let placement = match crate::assign_placement(*assign_type, attr.as_deref()) {
                    Ok(placement) => placement,
                    Err(cause) => return Err(self.engine_fatal(cause, "assign-placement-invalid")),
                };
                let source = crate::assign_source(expression.as_deref(), content.as_ref());
                self.caps
                    .assign(location, &source, &placement, event, context)
                    .map_err(recover::<C>)?;
            }
            // SCXML 4.6: a `<foreach>` item or index names a variable the
            // data model declares when it is not declared yet; an illegal
            // one raises error.execution (official case 152).
            Effect::AssignValue { location, value } => {
                self.caps
                    .assign(
                        location,
                        &AssignSource::ForeachValue(value.clone()),
                        &AssignPlacement::ReplaceChildren,
                        event,
                        context,
                    )
                    .map_err(recover::<C>)?;
            }
            // SCXML 6.5.2: an empty <finalize> updates each namelist item or
            // <param location> "as if by <assign> with any return value that
            // has a name that matches". A namelist item with no legal key is
            // error.execution (as for <send>, 6.2.4); no matching value
            // leaves the location unchanged.
            Effect::UpdateFromReturnedValue { name, location } => {
                let name = match name {
                    Some(name) => name.clone(),
                    None => self
                        .caps
                        .namelist_key(location)
                        .ok_or(EffectFailure::Block(PlatformError::Execution))?,
                };
                let returned =
                    event.and_then(|event| self.caps.event_data_field(&event.data, &name));
                if let Some(value) = returned {
                    self.caps
                        .assign(
                            location,
                            &AssignSource::Value(value),
                            &AssignPlacement::ReplaceChildren,
                            event,
                            context,
                        )
                        .map_err(recover::<C>)?;
                }
            }
            // SCXML 4.6: the array is evaluated once; the shallow snapshot
            // fixes the iteration even when the body mutates the array.
            // Legal collections are ECMAScript arrays and XPath node-sets;
            // anything else raises error.execution (case 152). The index
            // counts from 0, or from 1 under XPath (CR 2014-03-13 §C.3.10:
            // "from an index of 1 by increments of one").
            Effect::Iterate {
                array_expression, ..
            } => {
                match self
                    .caps
                    .foreach_items(array_expression, event)
                    .map_err(recover::<C>)?
                {
                    ForeachItems::Items(items) => {
                        let first_index = u32::from(
                            self.caps.data_model_name().as_deref() == Some(data::XPATH_DATA_MODEL),
                        );
                        engine
                            .begin_iteration(&items, first_index)
                            .map_err(|cause| self.engine_fatal(cause, "engine-begin-iteration"))?;
                    }
                    ForeachItems::Illegal => {
                        return Err(EffectFailure::Block(PlatformError::Execution));
                    }
                }
            }
            // SCXML 5.8: a script runtime or syntax error aborts the block.
            Effect::ExecuteScript(script) => {
                self.caps.execute(script, event).map_err(recover::<C>)?;
            }
            Effect::RunScript(source) => {
                self.caps.run_script(source, event).map_err(recover::<C>)?;
            }
            Effect::ExecuteExtension(element) => self.execute_extension(engine, element, event)?,
            Effect::Send(send) => self.send(engine, send, event)?,
            Effect::Cancel {
                send_id,
                send_id_expression,
            } => self.cancel(send_id.as_deref(), send_id_expression.as_deref(), event)?,
            Effect::BindStateData { declarations, .. } => {
                data::bind_state_data(self, engine, declarations)?;
            }
            Effect::Invoke { state, invoke } => self.start_invocation(engine, state, invoke)?,
            // SCXML 6.4: the cancel acts as the final `<onexit>` of the
            // invoking state; a host that carries it out later suspends the
            // run here until it did.
            Effect::CancelInvoke { invoke_id } => {
                match self.caps.cancel_invocation(invoke_id.as_str())? {
                    InvocationProgress::Completed => {}
                    InvocationProgress::Deferred => self.defer(DeferredRequest::Cancel),
                }
            }
            // SCXML 6.4.1: the engine decided to forward this external
            // event; the host only queues it on the child it owns.
            Effect::Forward { invoke_id, event } => {
                self.caps.forward_to_invocation(
                    invoke_id.as_str(),
                    event,
                    self.outbound.as_ref(),
                )?;
            }
        }
        Ok(())
    }

    /// Executes one executable-content extension (SCXML 4.10): the events the
    /// extension component queued are applied first, whatever its outcome (like
    /// the data-model assignments it already made); then an unhandled element
    /// raises `error.execution` and is never ignored. Queued events without a
    /// carrier take the batch's.
    fn execute_extension(
        &mut self,
        engine: &mut Engine,
        element: &XmlNodeIr,
        event: Option<&Event>,
    ) -> Result<(), EffectFailure<C::Error>> {
        let context = event.and_then(|event| event.trace_context.as_ref());
        // The executable-content image is computed only for an extension
        // component that reads it.
        let snapshot = if self.caps.extension_snapshot_request(element) {
            Some(ExtensionSnapshot {
                image: engine
                    .effect_snapshot()
                    .map_err(|cause| self.engine_fatal(cause, "engine-extension-snapshot"))?,
                session: self.state,
            })
        } else {
            None
        };
        let executed = self
            .caps
            .execute_extension(element, event, context, snapshot)
            .map_err(recover::<C>)?;
        for mut queued in executed.internal {
            if queued.trace_context.is_none() {
                queued.trace_context.clone_from(&self.outbound);
            }
            engine
                .enqueue_internal(queued)
                .map_err(|cause| self.engine_fatal(cause, "engine-enqueue-internal"))?;
        }
        for mut queued in executed.external {
            if queued.trace_context.is_none() {
                queued.trace_context.clone_from(&self.outbound);
            }
            engine
                .enqueue_external(queued)
                .map_err(|cause| self.engine_fatal(cause, "engine-enqueue-external"))?;
        }
        match executed.outcome {
            Ok(true) => Ok(()),
            Ok(false) => Err(EffectFailure::Block(PlatformError::Execution)),
            Err(error) => Err(recover::<C>(error)),
        }
    }
}

/// Maps one capability failure: recoverable ones abort the block with
/// `error.execution` (SCXML 5.10), every other one is fatal.
fn recover<C: SessionCapabilities>(error: C::Error) -> EffectFailure<C::Error> {
    if C::error_kind(&error).is_recoverable() {
        EffectFailure::Block(PlatformError::Execution)
    } else {
        EffectFailure::Fatal(error)
    }
}

impl<C: SessionCapabilities> GuardEvaluator for Interpreter<'_, C> {
    fn evaluate_condition(
        &mut self,
        expression: &str,
        event: Option<&Event>,
    ) -> Result<bool, EvaluationError> {
        self.caps
            .evaluate_condition(expression, event)
            .map_err(|error| {
                // SCXML 5.9: invalid-input and execution failures make the
                // condition false with error.execution; everything else is
                // a fatal provider failure reported with its own error.
                let mapped = match C::error_kind(&error) {
                    CapabilityErrorKind::InvalidInput => EvaluationError::InvalidInput,
                    CapabilityErrorKind::Execution => EvaluationError::Execution,
                    CapabilityErrorKind::Unavailable => EvaluationError::Unavailable,
                    CapabilityErrorKind::PayloadTooLarge
                    | CapabilityErrorKind::CapacityExhausted
                    | CapabilityErrorKind::Other => EvaluationError::Platform,
                };
                if matches!(
                    mapped,
                    EvaluationError::Unavailable | EvaluationError::Platform
                ) {
                    self.guard_failure = Some(error);
                }
                mapped
            })
    }

    fn last_batch_trace_context(&self) -> Option<&TraceContext> {
        self.last_batch.as_ref()
    }
}

impl<C: SessionCapabilities> crate::EffectExecutor for Interpreter<'_, C> {
    type Error = C::Error;

    fn admit_command(&mut self) -> Result<(), C::Error> {
        self.guard_failure = None;
        self.caps.admit_command()
    }

    fn command_completed(&mut self) {
        self.caps.observe(Observation::CommandCompleted);
    }

    fn event_processed(&mut self, event: &Event) {
        self.caps
            .observe(Observation::Processed(event.name.as_str()));
    }

    fn engine_error(&mut self, error: EngineError, stage: RunStage) -> C::Error {
        // A fatal guard failure reports the provider's own error.
        if matches!(
            error,
            EngineError::Evaluation(EvaluationError::Unavailable | EvaluationError::Platform)
        ) && let Some(original) = self.guard_failure.take()
        {
            return original;
        }
        let code = match stage {
            RunStage::Command => "engine-command",
            RunStage::Resume => "engine-resume",
            RunStage::BeginEffects => "engine-begin-effects",
            RunStage::ContinueEffects => "engine-continue-effects",
            RunStage::CompleteEffects => "engine-complete-effects",
            RunStage::AbortBlock => "engine-execution-failed",
        };
        self.caps.failure(InterpretFailure::engine(error, code))
    }

    fn update_configuration(
        &mut self,
        exited: &[StateId],
        entered: &[StateId],
    ) -> Result<(), C::Error> {
        self.caps.update_active_configuration(exited, entered)
    }

    fn begin_batch(&mut self, trigger: Option<&Event>) -> Result<(), C::Error> {
        self.guard_failure = None;
        self.outbound = self.caps.begin_batch(trigger)?;
        self.last_batch.clone_from(&self.outbound);
        Ok(())
    }

    fn batch_trace_context(&self) -> Option<&TraceContext> {
        self.outbound.as_ref()
    }

    fn apply_effect(
        &mut self,
        engine: &mut Engine,
        planned: &PlannedEffect,
        trigger: Option<&Event>,
    ) -> Result<(), EffectFailure<C::Error>> {
        self.apply(engine, &planned.effect, trigger)
    }

    fn end_batch(&mut self, end: BatchEnd) {
        self.outbound = None;
        self.caps.end_batch(end);
    }

    fn suspend_requested(&mut self) -> bool {
        if !std::mem::take(&mut self.suspend) {
            return false;
        }
        if let Some(deferred) = self.state.deferred.as_mut() {
            deferred.outbound.clone_from(&self.outbound);
            deferred.last_batch.clone_from(&self.last_batch);
        }
        true
    }

    fn resume_effect(
        &mut self,
        engine: &mut Engine,
        _planned: &PlannedEffect,
        _trigger: Option<&Event>,
    ) -> Result<(), EffectFailure<C::Error>> {
        let (Some(deferred), Some(result)) =
            (self.state.deferred.take(), self.deferred_result.take())
        else {
            return Err(self.fatal(FailureCategory::Harness, "deferred-invocation-missing"));
        };
        match deferred.request {
            DeferredRequest::Start {
                state,
                document_order,
                invoke_id,
                type_uri,
            } => self.invocation_start_result(
                engine,
                &state,
                document_order,
                invoke_id,
                type_uri,
                result,
            ),
            DeferredRequest::Cancel => result.map_err(EffectFailure::Fatal),
        }
    }
}

/// SCXML's "space-normalized string literal" (5.3, 6.2.6; B.2.1 and
/// B.2.8.1), as `XPath` 1.0 `normalize-space()` defines it and the `XPath` data
/// model applies it: leading and trailing white space stripped and every
/// internal run of it replaced by one space, where white space is "the same
/// as those allowed by the S production in XML" (space, tab, carriage
/// return, line feed); every other character is content.
fn normalize_whitespace(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut pending_space = false;
    for character in text.chars() {
        if matches!(character, ' ' | '\t' | '\r' | '\n') {
            pending_space = !normalized.is_empty();
        } else {
            if pending_space {
                normalized.push(' ');
                pending_space = false;
            }
            normalized.push(character);
        }
    }
    normalized
}

/// Inline children as a data-model body: standalone markup when an element
/// is present, else the concatenated text. `None` when the markup exceeds
/// its serialization bound.
fn inline_body(nodes: &[XmlNodeIr]) -> Option<InlineBody> {
    if nodes
        .iter()
        .any(|node| matches!(node, XmlNodeIr::Element { .. }))
    {
        return scxml_compiler_core::xml_markup(nodes)
            .ok()
            .map(InlineBody::Markup);
    }
    Some(InlineBody::Text(inline_text(nodes)))
}

/// Concatenated text children, in document order.
fn inline_text(nodes: &[XmlNodeIr]) -> String {
    let mut text = String::new();
    for node in nodes {
        if let XmlNodeIr::Text { value } = node {
            text.push_str(value);
        }
    }
    text
}
