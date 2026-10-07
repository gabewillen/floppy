//! Run-wide state, machine session handles, and the scheduler binding.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use opentelemetry::Context;
use opentelemetry::trace::{TraceContextExt as _, Tracer as _, TracerProvider as _};
use std::collections::VecDeque;

use scxml_scheduler::{
    Checkpoint, CheckpointHook, Delivery as QueuedDelivery, DrainStop,
    DueRoute as InvocationDueRoute, Horizon, Invoked, InvokedSessions, Scheduler, SessionKey,
    Sessions,
};
use wasmtime::component::{Resource, ResourceAny};

use super::links::{ParentLink, SessionLinks, SessionPlace, invalid_input};
use super::loopback::{
    CompilerRuntime, ReceiverRuntime, Route, Routes, inbound_http_trace_context,
};
use super::network::RunHttp;
use super::snapshot::{CheckpointSnapshot, CheckpointSnapshotFailure, Snapshotter};
use super::{
    HostCore, LogEntry, RunFailure, ScriptedEvent, SessionEmbedder, SessionHostError,
    SessionOutcome, SessionOutput, SessionRun, Terminal, TraceEntry, TraceObservation,
};
use crate::basichttp_types::{Delivery, HttpRequest, HttpResponse};
use crate::bindings;
use crate::bindings::exports::stateforward::scxml::machine_api::{
    CallResult, InvocationContext, InvocationDone, InvocationMessage, MachineSnapshot,
    SessionFailure, SnapshotOptions, Status,
};
use crate::bindings::stateforward::scxml::types::{
    ComponentError, DataValue as WitDataValue, ErrorCode, Event as WitEvent,
    TraceContext as WitTraceContext,
};
use crate::component::{MachineStoreData, SessionMachine, instantiate_session_machine};
use crate::io_bindings::exports::stateforward::scxml::io_processor_api::{DelayedSend, DueRoute};
use crate::session::{external_event, wit_trace_context};
use crate::{ComponentDataModel, HostError};

/// The root session, its invoked sessions, and each session's external event queue:
/// sessions are addressed by their invoke-id paths; a finished session's
/// `done.invoke` carries its `<donedata>` and finishing carrier.
pub(crate) type RunInvokedSessions = InvokedSessions<String, WitEvent, InvocationDone>;

/// The run-wide command budget, trace, and root logs.
#[derive(Default)]
struct Ledger {
    commands: u64,
    max_commands: u64,
    /// Commands charged before the current call began (0 for a batch run,
    /// which is one call).
    step_origin: u64,
    trace: Vec<TraceEntry>,
    root_logs: Vec<LogEntry>,
}

/// State every session of one run shares. Single-threaded; each `RefCell`
/// borrow is released before any component call.
pub(crate) struct RunShared {
    ledger: RefCell<Ledger>,
    /// The executor's sessions and queues, lent here for the duration of one session
    /// command so capability calls can push onto external queues.
    pub(crate) invoked_sessions: RefCell<RunInvokedSessions>,
    fault: RefCell<Option<RunFailure>>,
    compiler: RefCell<Option<CompilerRuntime>>,
    pub(crate) receiver: RefCell<Option<ReceiverRuntime>>,
    /// A live session's streamed outputs, taken by the embedder after each
    /// call; `None` for a batch run, which streams nothing.
    outputs: Option<RefCell<Vec<SessionOutput>>>,
    /// The run's `BasicHTTP` address space and outbound transport.
    pub(crate) http: RunHttp,
    /// The Unix epoch ms of logical time 0 of every session clock of the
    /// run: what ECMAScript `Date` reads is this plus the logical time.
    pub(crate) start_epoch_ms: u64,
    /// Every session's `_sessionid` and completion: what an invoking
    /// session's links read about the sessions it invoked.
    pub(crate) sessions: RefCell<SessionTable>,
    /// The `start-invocation` or `cancel-invocation` request the session
    /// whose call just suspended deferred to the host.
    pub(crate) deferred: RefCell<Option<DeferredRequest>>,
}

/// One session's (or invoked service's) identity and completion.
pub(crate) struct SessionRecord {
    /// The session's `_sessionid`; none for an invoked service.
    pub(crate) session_id: Option<String>,
    /// Whether the session reached a top-level final state.
    pub(crate) finished: bool,
    /// The session's `<donedata>` value for its `done.invoke`.
    return_data: Option<WitDataValue>,
    /// The carrier of the session's finishing batch, for its `done.invoke`.
    done_trace_context: Option<WitTraceContext>,
}

/// Every session of a run by key, and the running ones by `_sessionid`.
#[derive(Default)]
pub(crate) struct SessionTable {
    records: BTreeMap<SessionKey, SessionRecord>,
    by_session_id: BTreeMap<String, SessionKey>,
    /// The root's `_sessionid`.
    root_session_id: String,
    /// Invoked sessions named so far in this run.
    invoked_named: u64,
}

impl SessionTable {
    /// The `_sessionid` of the next session the run starts: the root's,
    /// then `.` and the session's ordinal among the invoked sessions the
    /// run started (SCXML 5.10 "the system-generated id for the current
    /// SCXML session"). Unique across the run, so `#_scxml_<sessionid>`
    /// (C.1) names one session, and of a length that does not grow with
    /// the nesting depth.
    pub(crate) fn next_invoked_session_id(&mut self) -> String {
        self.invoked_named = self.invoked_named.saturating_add(1);
        format!("{}.{}", self.root_session_id, self.invoked_named)
    }

    /// Adds the session `key` with `session_id` (none for an invoked
    /// service), not finished.
    pub(crate) fn insert(&mut self, key: SessionKey, session_id: Option<String>) {
        if let Some(session_id) = &session_id {
            self.by_session_id.insert(session_id.clone(), key);
        }
        self.records.insert(
            key,
            SessionRecord {
                session_id,
                finished: false,
                return_data: None,
                done_trace_context: None,
            },
        );
    }

    /// Removes the session `key`.
    pub(crate) fn remove(&mut self, key: SessionKey) {
        if let Some(record) = self.records.remove(&key)
            && let Some(session_id) = &record.session_id
        {
            self.by_session_id.remove(session_id);
        }
    }

    /// The session `key`.
    pub(crate) fn get(&self, key: SessionKey) -> Option<&SessionRecord> {
        self.records.get(&key)
    }

    /// The session whose `_sessionid` is `session_id`.
    pub(crate) fn key_of(&self, session_id: &str) -> Option<SessionKey> {
        self.by_session_id.get(session_id).copied()
    }

    /// Records one completed call of the session `key`: completion and
    /// return data.
    fn observe_step(&mut self, key: SessionKey, step: CallResult) {
        let Some(record) = self.records.get_mut(&key) else {
            return;
        };
        if step.status == Status::Finished {
            record.finished = true;
        }
        if step.return_data.is_some() {
            record.return_data = step.return_data;
        }
        if step.done_trace_context.is_some() {
            record.done_trace_context = step.done_trace_context;
        }
    }

    /// Records that the invoked service `key` completed with `data`.
    fn service_completed(&mut self, key: SessionKey, data: Option<WitDataValue>) {
        if let Some(record) = self.records.get_mut(&key) {
            record.finished = true;
            record.return_data = data;
        }
    }

    /// Records that the invoked service `key` failed: it runs nothing more
    /// and no invocation address answers for it.
    fn service_failed(&mut self, key: SessionKey) {
        if let Some(record) = self.records.get_mut(&key) {
            record.finished = true;
        }
    }

    /// The session's completion for its `done.invoke`: `<donedata>` and
    /// the finishing batch's carrier, each taken once.
    fn take_done(&mut self, key: SessionKey) -> Option<InvocationDone> {
        let record = self.records.get_mut(&key)?;
        Some(InvocationDone {
            data: record.return_data.take(),
            trace_context: record.done_trace_context.take(),
        })
    }
}

/// A `start-invocation` or `cancel-invocation` request a session deferred
/// to the host (WIT `invocation-progress.deferred`): its call suspended
/// right after the `<invoke>` effect, and the host carries the request out
/// before completing it (`session.invocation-completed`).
pub(crate) enum DeferredRequest {
    /// Start the constructed, registered session `child`.
    Start {
        child: SessionKey,
        handle: Box<MachineHandle>,
        context: InvocationContext,
    },
    /// Start the invocation of the registered invoked service `child`.
    StartService {
        child: SessionKey,
        request: Box<crate::bindings::stateforward::scxml::types::InvokeRequest>,
        /// The invoking session's execution context.
        execution_context: Context,
        /// The invoking session's document bindings.
        bindings: Option<Rc<super::DocumentBindings>>,
    },
    /// Cancel the session `child`, whose cancellation began.
    Cancel { child: SessionKey },
}

/// The start epoch of a batch run's session clocks (and of a live session
/// on the logical clock that replays no recording): 2015-09-01T00:00:00Z,
/// the date of the SCXML 1.0 Recommendation, fixed so a batch run is
/// deterministic. ECMA-262 leaves the current time to the implementation.
pub(crate) const BATCH_START_EPOCH_MS: u64 = 1_441_065_600_000;

#[cfg(test)]
mod deferred_tests;

/// Whether a run is a batch run or a live session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RunMode {
    /// A batch run: it streams nothing.
    Batch,
    /// A live session: the embedder takes its outputs after each step.
    Live,
}

impl RunShared {
    /// Run-wide state of one batch run or one live session (`mode`), with a
    /// budget of `max_commands` engine commands per call, the `BasicHTTP`
    /// network `http`, and session clocks that start at `start_epoch_ms`.
    pub(crate) fn new(
        max_commands: u64,
        mode: RunMode,
        http: RunHttp,
        start_epoch_ms: u64,
    ) -> Self {
        Self {
            ledger: RefCell::new(Ledger {
                max_commands,
                ..Ledger::default()
            }),
            invoked_sessions: RefCell::new(RunInvokedSessions::default()),
            fault: RefCell::new(None),
            compiler: RefCell::new(None),
            receiver: RefCell::new(None),
            outputs: (mode == RunMode::Live).then(|| RefCell::new(Vec::new())),
            http,
            start_epoch_ms,
            sessions: RefCell::new(SessionTable::default()),
            deferred: RefCell::new(None),
        }
    }

    /// Records that a command ran on the session `key` inside another
    /// session's call (the executor rereads its status and deadline).
    pub(crate) fn touch(&self, key: SessionKey) {
        self.invoked_sessions.borrow_mut().touch(key);
    }

    /// Starts the next call of a live session: a fresh command budget and
    /// an empty trace (live outputs stream instead). Never called in a
    /// batch run.
    pub(crate) fn begin_step(&self) {
        let mut ledger = self.ledger.borrow_mut();
        ledger.step_origin = ledger.commands;
        ledger.trace.clear();
        ledger.root_logs.clear();
    }

    /// Whether this run streams outputs to a live embedder.
    pub(crate) fn streaming(&self) -> bool {
        self.outputs.is_some()
    }

    /// Appends one streamed output; a batch run drops it (it never calls
    /// this for anything it also records).
    pub(crate) fn emit(&self, output: SessionOutput) {
        if let Some(outputs) = &self.outputs {
            outputs.borrow_mut().push(output);
        }
    }

    /// Takes every streamed output, in order.
    pub(crate) fn take_outputs(&self) -> Vec<SessionOutput> {
        self.outputs
            .as_ref()
            .map(|outputs| std::mem::take(&mut *outputs.borrow_mut()))
            .unwrap_or_default()
    }

    /// Fails closed before a command that would exceed the call's budget.
    pub(crate) fn require_budget(&self) -> Result<(), RunFailure> {
        let ledger = self.ledger.borrow();
        if ledger.commands.saturating_sub(ledger.step_origin) >= ledger.max_commands {
            Err(RunFailure::CommandBudget)
        } else {
            Ok(())
        }
    }

    /// Commands charged in the current call.
    pub(crate) fn step_commands(&self) -> u64 {
        let ledger = self.ledger.borrow();
        ledger.commands.saturating_sub(ledger.step_origin)
    }

    /// Charges one completed command.
    pub(crate) fn charge(&self) {
        let mut ledger = self.ledger.borrow_mut();
        ledger.commands = ledger.commands.saturating_add(1);
    }

    /// Commands charged so far.
    pub(crate) fn commands(&self) -> u64 {
        self.ledger.borrow().commands
    }

    /// Appends one trace observation.
    pub(crate) fn record(
        &self,
        session: &str,
        parent: Option<&str>,
        observation: TraceObservation,
    ) {
        self.ledger.borrow_mut().trace.push(TraceEntry {
            session: session.to_owned(),
            parent: parent.map(str::to_owned),
            observation,
        });
    }

    /// Appends one root `<log>` record.
    pub(crate) fn push_root_log(&self, entry: LogEntry) {
        self.ledger.borrow_mut().root_logs.push(entry);
    }

    /// Records the run's first fault; later ones are consequences.
    pub(crate) fn fault(&self, failure: RunFailure) {
        let mut fault = self.fault.borrow_mut();
        if fault.is_none() {
            *fault = Some(failure);
        }
    }

    /// Whether a fault was recorded.
    pub(crate) fn has_fault(&self) -> bool {
        self.fault.borrow().is_some()
    }

    pub(crate) fn take_fault(&self) -> Option<RunFailure> {
        self.fault.borrow_mut().take()
    }

    /// Runs `body` with the run's compiler, instantiated on first use.
    pub(crate) fn with_compiler<T>(
        &self,
        core: &HostCore,
        body: impl FnOnce(&mut CompilerRuntime) -> Result<T, HostError>,
    ) -> Result<T, HostError> {
        let mut slot = self.compiler.borrow_mut();
        if slot.is_none() {
            *slot = Some(CompilerRuntime::instantiate(core)?);
        }
        match slot.as_mut() {
            Some(compiler) => body(compiler),
            None => Err(HostError::Component("compiler-unavailable".into())),
        }
    }

    /// Runs `body` with the run's `BasicHTTP` receiver, instantiated on
    /// first use.
    pub(crate) fn with_receiver<T>(
        &self,
        core: &HostCore,
        body: impl FnOnce(&mut ReceiverRuntime) -> Result<T, HostError>,
    ) -> Result<T, HostError> {
        let mut slot = self.receiver.borrow_mut();
        if slot.is_none() {
            *slot = Some(ReceiverRuntime::instantiate(core)?);
        }
        match slot.as_mut() {
            Some(receiver) => body(receiver),
            None => Err(HostError::Component(
                "basichttp-receiver-unavailable".into(),
            )),
        }
    }
}

/// One machine session: its store, bindings, and session resource.
pub(crate) struct MachineHandle {
    store: wasmtime::Store<MachineStoreData>,
    machine: bindings::Machine,
    session: ResourceAny,
    fuel: u64,
    /// The fuel a suspended call had left: its continuation runs on it, so
    /// a call and its continuations share one call's fuel.
    suspended_fuel: Option<u64>,
}

/// Everything needed to construct one session.
pub(crate) struct SessionSpec {
    pub(crate) session_id: String,
    /// The session among the run's sessions.
    pub(crate) key: SessionKey,
    pub(crate) parent: Option<ParentLink>,
    pub(crate) data_model: Option<String>,
    pub(crate) machine_ir: Vec<u8>,
    pub(crate) clock_now: u64,
    /// The bindings of the document the session runs; `None` for the
    /// host's.
    pub(crate) bindings: Option<Rc<super::DocumentBindings>>,
}

/// Outcome of constructing one session.
pub(crate) enum Constructed {
    /// The session is `ready`.
    Ready(Box<MachineHandle>),
    /// The session started `failed`: its cause, and the first
    /// `load-script` failure code when a script rejected the document.
    Rejected {
        cause: ComponentError,
        script_failure: Option<ErrorCode>,
    },
}

impl MachineHandle {
    /// Constructs one session: its data model, SCXML I/O processor and
    /// clock (advanced to `spec.clock_now`), script capability, and the
    /// machine session itself.
    pub(crate) fn construct(
        core: &Rc<HostCore>,
        shared: &Rc<RunShared>,
        embedder: &Rc<dyn SessionEmbedder>,
        spec: SessionSpec,
        execution_context: &Context,
    ) -> Result<Constructed, RunFailure> {
        // The session clock's time, advanced by the links and read by the
        // data model (`execution-host.now-ms`).
        let time =
            crate::data_model::SessionTime::new(std::sync::atomic::AtomicU64::new(spec.clock_now));
        let data_model = match spec
            .data_model
            .as_deref()
            .map(|name| core.data_model(name))
            .transpose()?
            .flatten()
        {
            Some(component) => Some(ComponentDataModel::from_compiled(
                &core.engine,
                component,
                &core.data_model_linker,
                &core.config,
                &core.providers,
                execution_context.clone(),
                Some(&core.call_budget),
            )?),
            None => None,
        };
        let data_model = data_model.map(|mut data_model| {
            data_model.share_session_time(shared.start_epoch_ms, std::sync::Arc::clone(&time));
            data_model
        });
        let mut clock = crate::capability_graph::instantiate_clock_compiled(
            &core.engine,
            core.clock(),
            &core.clock_linker,
            &core.config,
            &core.providers,
            Some(&core.call_budget),
        )?;
        if spec.clock_now > 0 {
            clock.advance(spec.clock_now)?;
        }
        let io = crate::capability_graph::instantiate_io_compiled(
            &core.engine,
            core.io(),
            &core.io_linker,
            &core.config,
            &core.providers,
            clock,
            Some(&core.call_budget),
        )?;
        let links = SessionLinks::new(
            Rc::clone(core),
            Rc::clone(shared),
            Rc::clone(embedder),
            SessionPlace {
                session_id: spec.session_id.clone(),
                key: spec.key,
                parent: spec.parent,
                bindings: spec.bindings.clone(),
            },
            io,
            execution_context.clone(),
            std::sync::Arc::clone(&time),
        );
        // Every bound extension component gets its own instance for this
        // session, initialized before the machine runs anything.
        let namespaces = core.session_namespaces(execution_context, spec.bindings.as_deref())?;
        let (mut store, machine) = instantiate_session_machine(
            &core.engine,
            core.machine(),
            &core.machine_linker,
            &core.config,
            &core.providers,
            execution_context.clone(),
            SessionMachine {
                machine_ir: spec.machine_ir,
                data_model,
                scripts: embedder.scripts(),
                session_id: spec.session_id.clone(),
                links: Box::new(links),
                call_budget: Arc::clone(&core.call_budget),
                namespaces,
            },
        )?;
        let fuel = core.config.store.fuel;
        let api = machine.stateforward_scxml_machine_api().session();
        store.set_fuel(fuel).map_err(HostError::from)?;
        let session = api
            .call_constructor(&mut store, &spec.session_id, Resource::new_own(0))
            .map_err(HostError::from)?;
        store.set_fuel(fuel).map_err(HostError::from)?;
        if let Some(cause) = api
            .call_failure(&mut store, session)
            .map_err(HostError::from)?
        {
            return Ok(Constructed::Rejected {
                cause,
                script_failure: store.data().script_load_failure(),
            });
        }
        Ok(Constructed::Ready(Box::new(Self {
            store,
            machine,
            session,
            fuel,
            suspended_fuel: None,
        })))
    }

    fn call<T>(
        &mut self,
        body: impl FnOnce(
            &bindings::exports::stateforward::scxml::machine_api::GuestSession,
            &mut wasmtime::Store<MachineStoreData>,
            ResourceAny,
        ) -> wasmtime::Result<T>,
    ) -> Result<T, HostError> {
        self.store.set_fuel(self.fuel)?;
        let api = self.machine.stateforward_scxml_machine_api().session();
        body(&api, &mut self.store, self.session).map_err(HostError::from)
    }

    /// One call-result-returning call: it runs on the fuel a suspended call left,
    /// else on a fresh call's fuel, and leaves its own remaining fuel when
    /// it suspends.
    fn step_call(
        &mut self,
        body: impl FnOnce(
            &bindings::exports::stateforward::scxml::machine_api::GuestSession,
            &mut wasmtime::Store<MachineStoreData>,
            ResourceAny,
        ) -> wasmtime::Result<Result<CallResult, SessionFailure>>,
    ) -> MachineResult {
        let fuel = self.suspended_fuel.take().unwrap_or(self.fuel);
        self.store.set_fuel(fuel)?;
        let api = self.machine.stateforward_scxml_machine_api().session();
        let result = body(&api, &mut self.store, self.session).map_err(HostError::from)?;
        if matches!(&result, Ok(step) if step.suspended) {
            self.suspended_fuel = Some(self.store.get_fuel()?);
        }
        Ok(result)
    }

    pub(crate) fn start(&mut self) -> MachineResult {
        self.step_call(|api, store, session| api.call_start(store, session))
    }

    pub(crate) fn start_invoked(&mut self, context: &InvocationContext) -> MachineResult {
        self.step_call(|api, store, session| api.call_start_invoked(store, session, context))
    }

    pub(crate) fn send(&mut self, event: &WitEvent) -> MachineResult {
        self.step_call(|api, store, session| api.call_send(store, session, event))
    }

    pub(crate) fn deliver(
        &mut self,
        invoke_id: &str,
        message: &InvocationMessage,
    ) -> MachineResult {
        self.step_call(|api, store, session| {
            api.call_deliver_from_invocation(store, session, invoke_id, message)
        })
    }

    /// The machine's `session.communication-error` (SCXML 3.12.2, 6.2.4).
    pub(crate) fn communication_error(&mut self, send_id: Option<&str>) -> MachineResult {
        self.step_call(|api, store, session| api.call_communication_error(store, session, send_id))
    }

    pub(crate) fn cancel(&mut self) -> MachineResult {
        self.step_call(|api, store, session| api.call_cancel(store, session))
    }

    /// The machine's `session.invocation-completed`: the deferred
    /// invocation request's result continues the suspended call.
    pub(crate) fn invocation_completed(
        &mut self,
        outcome: &Result<(), ComponentError>,
    ) -> MachineResult {
        self.step_call(|api, store, session| {
            api.call_invocation_completed(store, session, outcome.as_ref().copied())
        })
    }

    /// The machine's read-only `session.snapshot`.
    pub(crate) fn snapshot(
        &mut self,
        options: SnapshotOptions,
    ) -> Result<Result<MachineSnapshot, ComponentError>, HostError> {
        self.call(|api, store, session| api.call_snapshot(store, session, options))
    }

    /// The machine's current status (`session.status`; infallible and
    /// read-only).
    pub(crate) fn status(&mut self) -> Result<Status, HostError> {
        self.call(|api, store, session| api.call_status(store, session))
    }

    /// See `MachineStoreData::data_model_snapshot`.
    pub(crate) fn data_model_snapshot(
        &mut self,
        read: crate::data_model::DataModelRead,
    ) -> Option<(
        String,
        Option<Result<crate::data_model::DataModelSnapshot, ComponentError>>,
    )> {
        self.store.data_mut().data_model_snapshot(read)
    }

    /// SHA-256 of the Machine IR bytes this session loaded, as 64
    /// lowercase hex digits (computed at snapshot time only).
    pub(crate) fn machine_ir_sha256(&self) -> String {
        use sha2::Digest as _;
        crate::artifact::lower_hex(&sha2::Sha256::digest(self.store.data().machine_ir()))
    }

    pub(crate) fn configuration(&mut self) -> Result<Vec<String>, HostError> {
        self.call(|api, store, session| api.call_configuration(store, session))
    }

    /// This session's mutable links.
    pub(crate) fn links_mut(&mut self) -> Result<&mut SessionLinks, RunFailure> {
        self.store.data_mut().links_mut().ok_or_else(missing_links)
    }
}

fn missing_links() -> RunFailure {
    RunFailure::Harness {
        code: "session-links-missing",
        message: "a session-host machine store has no session links".into(),
    }
}

/// The result of one call-result-returning machine call.
pub(crate) type MachineResult = Result<Result<CallResult, SessionFailure>, HostError>;

/// Resolves one machine call's result: a recorded fault wins (the machine
/// only echoes it), then traps, then the machine's own failure, then a call
/// that stopped at its command bound.
pub(crate) fn settle(
    shared: &RunShared,
    result: Result<Result<CallResult, SessionFailure>, HostError>,
) -> Result<CallResult, RunFailure> {
    if let Some(fault) = shared.take_fault() {
        return Err(fault);
    }
    let step = match result {
        Err(error) => return Err(RunFailure::Host(error)),
        Ok(Err(failure)) => {
            return Err(RunFailure::Machine {
                code: crate::component::error_code_label(failure.error.code),
                detail: failure.error.detail,
            });
        }
        Ok(Ok(step)) => step,
    };
    if !step.stable && step.status == Status::Running {
        return Err(RunFailure::CommandBound);
    }
    Ok(step)
}

/// One session of the run as the host holds it: an SCXML machine session,
/// or an invoked service (SCXML 6.4).
pub(crate) enum SessionHandle {
    /// A machine session (the root, or an invoked SCXML session).
    Machine(Box<MachineHandle>),
    /// An invoked service.
    Service(Box<crate::service::ServiceHandle>),
}

/// One due item of a session on the shared logical clock: a delayed send
/// of a machine session, or the timers of an invoked service
/// (`wasi:clocks/monotonic-clock.wait-until`).
pub(crate) enum RunDue {
    /// A machine session's due delayed send.
    Send(Box<DelayedSend>),
    /// An invoked service's due timers.
    Timers,
}

/// The run's sessions as the executor sees them: every machine session and
/// invoked service by key, in one flat table (no session owns another, so
/// nothing recurses through a chain of invocations), and the shared logical
/// clock.
pub(crate) struct RunSessions {
    handles: BTreeMap<SessionKey, SessionHandle>,
    root_status: Status,
    /// The root's `<donedata>` payload, from the call that finished it.
    root_return_data: Option<crate::bindings::stateforward::scxml::types::DataValue>,
    /// The shared logical clock, in ms: a session's own clock moves to it
    /// before the session is next used.
    now: u64,
    shared: Rc<RunShared>,
    core: Rc<HostCore>,
    root_id: String,
    /// How the run carries out its invoked services' requests.
    requests: crate::service::RequestMode,
    /// The last run-unique invocation id handed out.
    last_invocation: u64,
}

/// A machine call addressed an invoked service: a host defect.
fn service_addressed() -> RunFailure {
    RunFailure::Harness {
        code: "service-addressed",
        message: "the host addressed an invoked service as a machine session".into(),
    }
}

fn session_missing() -> RunFailure {
    RunFailure::Harness {
        code: "session-missing",
        message: "the executor addressed a session the host does not hold".into(),
    }
}

/// An invocation request a suspended session waits on, in the host's
/// explicit stack of suspended sessions.
enum Pending {
    /// The start of the session `child`.
    Start(SessionKey),
    /// The cancel of the session `child`.
    Cancel(SessionKey),
}

/// One suspended session waiting on an invocation request, with the span
/// of that request (ended when the request completes).
struct Suspended {
    session: SessionKey,
    pending: Pending,
    span: scxml_telemetry::FunctionSpan,
}

/// How the request a suspended session waits on completed.
enum Completion {
    /// The call the request ran returned.
    CallReturned,
    /// The request ran no call.
    NothingRan,
}

impl RunSessions {
    /// Moves the executor's sessions and queues into the shared run state for one session
    /// call (capability calls push onto external queues there), then back.
    fn with_invoked_sessions<T>(
        &mut self,
        invoked_sessions: &mut RunInvokedSessions,
        body: impl FnOnce(&mut Self) -> Result<T, RunFailure>,
    ) -> Result<T, RunFailure> {
        let placeholder = RunInvokedSessions::new(invoked_sessions.limits());
        *self.shared.invoked_sessions.borrow_mut() =
            std::mem::replace(invoked_sessions, placeholder);
        let result = body(self);
        let placeholder = RunInvokedSessions::new(invoked_sessions.limits());
        *invoked_sessions =
            std::mem::replace(&mut *self.shared.invoked_sessions.borrow_mut(), placeholder);
        result
    }

    fn root_step(&mut self, result: MachineResult) -> Result<(), RunFailure> {
        let step = settle(&self.shared, result)?;
        self.root_status = step.status;
        if step.return_data.is_some() {
            self.root_return_data = step.return_data;
        }
        Ok(())
    }

    /// The machine session `key`, its own clock moved to the shared clock
    /// first.
    fn session(&mut self, key: SessionKey) -> Result<&mut MachineHandle, RunFailure> {
        let now = self.now;
        let handle = match self.handles.get_mut(&key) {
            Some(SessionHandle::Machine(handle)) => handle,
            Some(SessionHandle::Service(_)) => return Err(service_addressed()),
            None => return Err(session_missing()),
        };
        handle.links_mut()?.clock_sync(now)?;
        Ok(handle)
    }

    /// The live invoked service whose run-unique invocation id is
    /// `invocation`, if it is still in the run.
    fn service_key(&self, invocation: u64) -> Option<SessionKey> {
        self.handles.iter().find_map(|(key, handle)| match handle {
            SessionHandle::Service(service) if service.invocation() == invocation => Some(*key),
            SessionHandle::Service(_) | SessionHandle::Machine(_) => None,
        })
    }

    /// Whether a live invoked service waits for the response of a request.
    fn services_awaiting(&self) -> bool {
        self.handles.values().any(|handle| match handle {
            SessionHandle::Service(service) => service.awaiting(),
            SessionHandle::Machine(_) => false,
        })
    }

    /// Whether `key` is an invoked service.
    fn is_service(&self, key: SessionKey) -> bool {
        matches!(self.handles.get(&key), Some(SessionHandle::Service(_)))
    }

    /// Applies what one input left of the invoked service `key`, in order:
    /// each returned event onto its invoking session's queue (ignored once
    /// that session began cancelling it), then its completion
    /// (`done.invoke`, queued by the executor after those events) or its
    /// failure (`communication-error`, and no `done.invoke`: its
    /// cancellation begins).
    fn apply_service_step(&mut self, key: SessionKey, step: crate::service::ServiceStep) {
        use crate::service::ReportState;
        let failed = matches!(step.state, ReportState::Failed | ReportState::Rejected);
        {
            let mut invoked_sessions = self.shared.invoked_sessions.borrow_mut();
            for event in step.events {
                invoked_sessions.push_to_parent(key, Invoked::Event(event));
            }
            if failed {
                invoked_sessions.push_to_parent(key, Invoked::CommunicationError);
                invoked_sessions.begin_cancel(key);
            }
            invoked_sessions.touch(key);
        }
        match step.state {
            ReportState::Running => {}
            ReportState::Done(data) => {
                self.shared
                    .sessions
                    .borrow_mut()
                    .service_completed(key, data);
            }
            ReportState::Failed | ReportState::Rejected => {
                self.shared.sessions.borrow_mut().service_failed(key);
            }
        }
    }

    /// Hands one input to the invoked service `key` as one host call inside
    /// a boundary span, and applies what it left.
    fn service_input(
        &mut self,
        key: SessionKey,
        operation: scxml_telemetry::Operation,
        input: impl FnOnce(
            &mut crate::service::ServiceHandle,
            u64,
            &crate::service::RequestMode,
        ) -> Option<crate::service::ServiceStep>,
    ) -> Result<(), RunFailure> {
        let span = scxml_telemetry::observe(scxml_telemetry::Module::HostWasmtime, operation);
        let now = self.now;
        let requests = self.requests.clone();
        let step = match self.handles.get_mut(&key) {
            Some(SessionHandle::Service(service)) => input(service, now, &requests),
            Some(SessionHandle::Machine(_)) | None => {
                span.fail(scxml_telemetry::ErrorCategory::Harness);
                return Err(session_missing());
            }
        };
        let Some(step) = step else {
            span.event(scxml_telemetry::SignalEvent::InputRejected);
            return Ok(());
        };
        if matches!(
            step.state,
            crate::service::ReportState::Failed | crate::service::ReportState::Rejected
        ) {
            span.event(scxml_telemetry::SignalEvent::InputRejected);
            span.fail(scxml_telemetry::ErrorCategory::Execution);
        } else {
            span.event(scxml_telemetry::SignalEvent::TransformationCompleted);
        }
        self.apply_service_step(key, step);
        Ok(())
    }

    /// The start of the invocation of the invoked service `child` for
    /// `request`: its thread starts and calls `invoke`, its handle joins the
    /// run, and the start's report applies. A rejected start fails that
    /// `<invoke>` (`invalid-input`) and the service is dropped.
    fn start_service(
        &mut self,
        child: SessionKey,
        request: crate::bindings::stateforward::scxml::types::InvokeRequest,
        execution_context: &Context,
        bindings: Option<&super::DocumentBindings>,
    ) -> Result<(), ComponentError> {
        self.last_invocation = self.last_invocation.saturating_add(1);
        let http_offered = !matches!(self.requests, crate::service::RequestMode::None);
        let started = self.core.start_service(
            request,
            self.now,
            self.last_invocation,
            http_offered,
            execution_context,
            bindings,
        );
        let (mut handle, report) = match started {
            Ok(Some(Ok(started))) => started,
            Ok(Some(Err(rejected))) => {
                self.drop_sessions(child);
                return Err(rejected);
            }
            Ok(None) => {
                self.drop_sessions(child);
                return Err(crate::component::component_error(
                    ErrorCode::Unavailable,
                    "no service component is bound to this invoke type",
                ));
            }
            Err(error) => {
                self.drop_sessions(child);
                return Err(fatal(&self.shared, RunFailure::Host(error)));
            }
        };
        let step = handle.settle(Some(report), self.now, &self.requests);
        if matches!(step.state, crate::service::ReportState::Rejected) {
            self.drop_sessions(child);
            return Err(invalid_input("the invoked service rejected its start"));
        }
        self.handles
            .insert(child, SessionHandle::Service(Box::new(handle)));
        self.shared.touch(child);
        self.apply_service_step(child, step);
        Ok(())
    }

    /// The links of the session `key`, its clock moved to the shared clock.
    fn session_links(&mut self, key: SessionKey) -> Result<&mut SessionLinks, RunFailure> {
        self.session(key)?.links_mut()
    }

    /// Whether the session `key` finished.
    fn is_finished(&self, key: SessionKey) -> bool {
        if key.is_root() {
            return self.root_status == Status::Finished;
        }
        self.shared
            .sessions
            .borrow()
            .get(key)
            .is_some_and(|record| record.finished)
    }

    /// Runs one command on the session `key` and records its call result. Every
    /// invocation request the command defers (and those of the sessions it
    /// starts or cancels in turn) is carried out first, in the order one
    /// uninterrupted call would have made them. The command is one host
    /// call of the fuel budget: the call, every start, cancel, and
    /// continuation carried out for it, spend one budget.
    fn command(
        &mut self,
        key: SessionKey,
        call: impl FnOnce(&mut MachineHandle) -> MachineResult,
    ) -> Result<(), RunFailure> {
        let core = Rc::clone(&self.core);
        let result = core.call_budget.hold(|| {
            let result = call(self.session(key)?);
            self.carry_out_deferred(key, result)
        })?;
        if key.is_root() {
            return self.root_step(result);
        }
        let step = settle(&self.shared, result)?;
        self.shared.sessions.borrow_mut().observe_step(key, step);
        Ok(())
    }

    /// Carries out the invocation request the call that returned `result`
    /// on `key` deferred, then completes that call, iteratively: a session
    /// started or cancelled for a request runs on this loop, not inside
    /// the invoking session's call, so a chain of any depth takes no native
    /// stack per level. SCXML 6.4: the invoking session is suspended right
    /// after its `<invoke>` start or cancel and continues only once the
    /// request completed, so every session runs exactly what it ran when
    /// the request was carried out inside the call. Returns the final
    /// result of `key`'s call.
    fn carry_out_deferred(
        &mut self,
        key: SessionKey,
        mut result: MachineResult,
    ) -> Result<MachineResult, RunFailure> {
        let mut suspended: Vec<Suspended> = Vec::new();
        let mut current = key;
        loop {
            let completion = if matches!(&result, Ok(Ok(step)) if step.suspended) {
                let Some(request) = self.shared.deferred.borrow_mut().take() else {
                    return Err(RunFailure::Harness {
                        code: "deferred-request-missing",
                        message: "a session suspended without deferring a request".into(),
                    });
                };
                match request {
                    DeferredRequest::StartService {
                        child,
                        request,
                        execution_context,
                        bindings,
                    } => {
                        // A service runs no machine: its start runs here,
                        // and the suspended call continues at once.
                        let span = scxml_telemetry::observe(
                            scxml_telemetry::Module::HostWasmtime,
                            scxml_telemetry::Operation::HostWasmtimeServiceStart,
                        );
                        let outcome = self.start_service(
                            child,
                            *request,
                            &execution_context,
                            bindings.as_deref(),
                        );
                        outcome.span_end(&span);
                        drop(span);
                        self.shared.touch(current);
                        result = self.session(current)?.invocation_completed(&outcome);
                        continue;
                    }
                    DeferredRequest::Start {
                        child,
                        handle,
                        context,
                    } => {
                        suspended.push(Suspended {
                            session: current,
                            pending: Pending::Start(child),
                            span: scxml_telemetry::observe(
                                scxml_telemetry::Module::HostWasmtime,
                                scxml_telemetry::Operation::HostWasmtimeInvokedSessionStart,
                            ),
                        });
                        self.handles.insert(child, SessionHandle::Machine(handle));
                        self.shared.touch(child);
                        current = child;
                        result = self.session(child)?.start_invoked(&context);
                        continue;
                    }
                    DeferredRequest::Cancel { child } => {
                        let service = self.is_service(child);
                        suspended.push(Suspended {
                            session: current,
                            pending: Pending::Cancel(child),
                            span: scxml_telemetry::observe(
                                scxml_telemetry::Module::HostWasmtime,
                                if service {
                                    scxml_telemetry::Operation::HostWasmtimeServiceCancel
                                } else {
                                    scxml_telemetry::Operation::HostWasmtimeInvokedSessionCancel
                                },
                            ),
                        });
                        if service {
                            // SCXML 6.4: dropping its handle cancels the
                            // `invoke` subtask and drops its store (below,
                            // with the session), and no `done.invoke`
                            // follows.
                        } else if !self.is_finished(child) {
                            self.shared.touch(child);
                            current = child;
                            result = self.session(child)?.cancel();
                            continue;
                        }
                        Completion::NothingRan
                    }
                }
            } else {
                self.discard_deferred();
                Completion::CallReturned
            };
            let Some(frame) = suspended.pop() else {
                return Ok(result);
            };
            let outcome = match (frame.pending, completion) {
                (Pending::Start(child), _) => self.complete_start(child, result),
                // A finished session runs nothing: the cancel completes at
                // once.
                (Pending::Cancel(child), Completion::NothingRan) => {
                    self.complete_cancel(child, Ok(()))
                }
                (Pending::Cancel(child), Completion::CallReturned) => {
                    let cancelled = settle(&self.shared, result).map(|_| ());
                    self.complete_cancel(child, cancelled)
                }
            };
            outcome.span_end(&frame.span);
            drop(frame.span);
            current = frame.session;
            self.shared.touch(current);
            result = self.session(current)?.invocation_completed(&outcome);
        }
    }

    /// Drops the request a call deferred without suspending for it (the
    /// call trapped or failed right after the `<invoke>` effect): a child
    /// constructed for it is dropped and deregistered, so nothing it holds
    /// keeps the run's shared state alive.
    fn discard_deferred(&mut self) {
        let request = self.shared.deferred.borrow_mut().take();
        match request {
            Some(DeferredRequest::Start { child, handle, .. }) => {
                drop(handle);
                self.drop_sessions(child);
            }
            Some(DeferredRequest::StartService { child, .. }) => {
                self.drop_sessions(child);
            }
            Some(DeferredRequest::Cancel { .. }) | None => {}
        }
    }

    /// The result of the `start-invocation` request of the session
    /// `child`, whose `start-invoked` returned `started`. A child that
    /// rejected its start fails that `<invoke>` (`error.execution` in the
    /// invoking session) and is dropped; any other failure is the run's.
    fn complete_start(
        &mut self,
        child: SessionKey,
        started: MachineResult,
    ) -> Result<(), ComponentError> {
        let rejected = matches!(
            &started,
            Ok(Err(failure))
                if matches!(failure.error.code, ErrorCode::InvalidInput | ErrorCode::Execution)
        ) && !self.shared.has_fault();
        if rejected {
            self.drop_sessions(child);
            return Err(invalid_input("the invoked session rejected its start"));
        }
        match settle(&self.shared, started) {
            Ok(step) => {
                self.shared.sessions.borrow_mut().observe_step(child, step);
                Ok(())
            }
            Err(failure) => {
                self.drop_sessions(child);
                Err(fatal(&self.shared, failure))
            }
        }
    }

    /// The result of the `cancel-invocation` request of the session
    /// `child`, whose own cancel ended as `cancelled`: it is dropped with
    /// every session below it, deepest first.
    fn complete_cancel(
        &mut self,
        child: SessionKey,
        cancelled: Result<(), RunFailure>,
    ) -> Result<(), ComponentError> {
        self.drop_sessions(child);
        cancelled.map_err(|failure| fatal(&self.shared, failure))
    }

    /// Removes the session `key` and every session below it from the run,
    /// with their external queues, and drops their stores deepest first.
    fn drop_sessions(&mut self, key: SessionKey) {
        let removed = self.shared.invoked_sessions.borrow_mut().remove(key);
        for gone in removed {
            self.shared.sessions.borrow_mut().remove(gone);
            drop(self.handles.remove(&gone));
        }
    }

    /// Drops every session's store, each after every session below it.
    fn teardown(&mut self, order: Vec<SessionKey>) {
        for key in order.into_iter().rev() {
            drop(self.handles.remove(&key));
        }
        self.handles.clear();
    }

    /// The external client's POST to the root's `BasicHTTP` location
    /// through the loopback: only the root session is externally
    /// addressable; arrivals queue in order.
    fn external_post(
        &mut self,
        client: &dyn Fn(&str) -> HttpRequest,
        arrivals: &mut VecDeque<WitEvent>,
    ) -> Result<HttpResponse, RunFailure> {
        let span = scxml_telemetry::observe(
            scxml_telemetry::Module::HostWasmtime,
            scxml_telemetry::Operation::HostWasmtimeLoopbackPost,
        );
        let location = self.shared.http.location(&self.root_id);
        let request = client(&location);
        let routes = Routes {
            base: self.shared.http.base.clone(),
            own: location,
            parent: None,
            children: Vec::new(),
        };
        let Ok(route) = routes.resolve(&request.uri) else {
            span.fail(scxml_telemetry::ErrorCategory::Harness);
            return Err(RunFailure::Harness {
                code: "external-client-unroutable",
                message: "the external client request left the loopback address space".into(),
            });
        };
        let response = self.shared.with_receiver(&self.core, |receiver| {
            let delivery = match route {
                // Only the root is externally addressable.
                None | Some(Route::Parent(_) | Route::Child(_)) => Delivery::UnknownSession,
                Some(Route::Own) => match receiver
                    .receive(&request, inbound_http_trace_context(&request.headers))?
                {
                    Err(rejection) => Delivery::Rejected(rejection),
                    Ok(event) => {
                        arrivals.push_back(event);
                        Delivery::Enqueued
                    }
                },
            };
            receiver.respond(delivery)
        });
        match response {
            Ok(response) => {
                span.event(scxml_telemetry::SignalEvent::TransformationCompleted);
                Ok(response)
            }
            Err(error) => {
                span.fail(scxml_telemetry::ErrorCategory::Platform);
                Err(RunFailure::Host(error))
            }
        }
    }
}

/// Ends one invocation request's span with its outcome.
trait SpanEnd {
    fn span_end(&self, span: &scxml_telemetry::FunctionSpan);
}

impl SpanEnd for Result<(), ComponentError> {
    fn span_end(&self, span: &scxml_telemetry::FunctionSpan) {
        if self.is_ok() {
            span.event(scxml_telemetry::SignalEvent::TransformationCompleted);
        } else {
            span.event(scxml_telemetry::SignalEvent::InputRejected);
            span.fail(scxml_telemetry::ErrorCategory::Execution);
        }
    }
}

impl Sessions for RunSessions {
    type Id = String;
    type Event = WitEvent;
    type Value = InvocationDone;
    type Due = RunDue;
    type Input = ScriptedEvent;
    type SendId = Option<String>;
    type Error = RunFailure;

    fn require_budget(&self) -> Result<(), RunFailure> {
        self.shared.require_budget()
    }

    fn start_root(&mut self, invoked_sessions: &mut RunInvokedSessions) -> Result<(), RunFailure> {
        self.with_invoked_sessions(invoked_sessions, |sessions| {
            sessions.command(SessionKey::ROOT, MachineHandle::start)
        })
    }

    fn send_input(
        &mut self,
        invoked_sessions: &mut RunInvokedSessions,
        input: &ScriptedEvent,
    ) -> Result<(), RunFailure> {
        scxml_types::EventName::new(input.name.clone()).map_err(|error| RunFailure::Harness {
            code: "event-name-invalid",
            message: error.to_string(),
        })?;
        let mut event = external_event(&input.name);
        event.data = crate::session::wit_value(input.data.clone());
        event.trace_context = input.trace_context.as_ref().map(wit_trace_context);
        self.deliver(
            invoked_sessions,
            SessionKey::ROOT,
            QueuedDelivery::External(event),
        )
    }

    fn deliver(
        &mut self,
        invoked_sessions: &mut RunInvokedSessions,
        session: SessionKey,
        delivery: QueuedDelivery<String, WitEvent, InvocationDone>,
    ) -> Result<(), RunFailure> {
        if self.is_service(session) {
            return self.with_invoked_sessions(invoked_sessions, |sessions| match delivery {
                // `#_<invokeid>` or autoforward (SCXML 6.4): the service's
                // `events` stream.
                QueuedDelivery::External(event) => sessions.service_input(
                    session,
                    scxml_telemetry::Operation::HostWasmtimeServiceSend,
                    |service, now, requests| Some(service.send(event, now, requests)),
                ),
                QueuedDelivery::FromInvocation { .. } => Err(RunFailure::Harness {
                    code: "service-invocation-message",
                    message: "an invoked service received a message from an invocation".into(),
                }),
            });
        }
        self.with_invoked_sessions(invoked_sessions, |sessions| match delivery {
            QueuedDelivery::External(event) => {
                sessions.command(session, |handle| handle.send(&event))
            }
            QueuedDelivery::FromInvocation { invoke_id, message } => {
                let message = match message {
                    Invoked::Event(event) => InvocationMessage::Event(event),
                    Invoked::Done(done) => {
                        InvocationMessage::Done(done.unwrap_or(InvocationDone {
                            data: None,
                            trace_context: None,
                        }))
                    }
                    Invoked::CommunicationError => InvocationMessage::CommunicationError,
                };
                sessions.command(session, |handle| handle.deliver(&invoke_id, &message))
            }
        })
    }

    fn communication_error(
        &mut self,
        invoked_sessions: &mut RunInvokedSessions,
        session: SessionKey,
        send_id: Option<String>,
    ) -> Result<(), RunFailure> {
        if self.is_service(session) {
            return Err(service_addressed());
        }
        self.with_invoked_sessions(invoked_sessions, |sessions| {
            sessions.command(session, |handle| {
                handle.communication_error(send_id.as_deref())
            })
        })
    }

    fn finished(&self, session: SessionKey) -> bool {
        self.is_finished(session)
    }

    fn take_return_data(&mut self, session: SessionKey) -> Option<InvocationDone> {
        self.shared.sessions.borrow_mut().take_done(session)
    }

    fn next_deadline(&mut self, session: SessionKey) -> Result<Option<u64>, RunFailure> {
        if let Some(SessionHandle::Service(service)) = self.handles.get(&session) {
            return Ok(service.next_deadline());
        }
        self.session_links(session)?.clock_next_deadline()
    }

    fn now(&mut self) -> Result<u64, RunFailure> {
        Ok(self.now)
    }

    fn advance_clock(&mut self, delta: u64) -> Result<(), RunFailure> {
        self.now = self.now.checked_add(delta).ok_or(RunFailure::Harness {
            code: "send-clock-advance",
            message: "the shared logical clock overflowed".into(),
        })?;
        Ok(())
    }

    fn drain_due(&mut self, session: SessionKey) -> Result<Vec<RunDue>, RunFailure> {
        let now = self.now;
        if let Some(SessionHandle::Service(service)) = self.handles.get_mut(&session) {
            return Ok(if service.take_due(now) {
                vec![RunDue::Timers]
            } else {
                Vec::new()
            });
        }
        Ok(self
            .session_links(session)?
            .drain_due()?
            .into_iter()
            .map(|due| RunDue::Send(Box::new(due)))
            .collect())
    }

    fn route_due(
        &mut self,
        invoked_sessions: &mut RunInvokedSessions,
        session: SessionKey,
        due: RunDue,
    ) -> Result<InvocationDueRoute<String, WitEvent, Option<String>>, RunFailure> {
        let due = match due {
            // The service's timers: every one due fires now; what it returns
            // reaches the invoking session's queue.
            RunDue::Timers => {
                return self.with_invoked_sessions(invoked_sessions, |sessions| {
                    sessions
                        .service_input(
                            session,
                            scxml_telemetry::Operation::HostWasmtimeServiceWaitUntil,
                            |service, now, requests| Some(service.timers(now, requests)),
                        )
                        .map(|()| InvocationDueRoute::Dispatched)
                });
            }
            RunDue::Send(due) => *due,
        };
        let due = match due {
            DelayedSend::Scxml(request) => request,
            // SCXML 6.2: the `BasicHTTP` processor posts it now; a loopback
            // POST may reach any session of the run.
            DelayedSend::Basichttp(send) => {
                return self.with_invoked_sessions(invoked_sessions, |sessions| {
                    sessions
                        .session_links(session)?
                        .dispatch_due_basichttp(send)
                });
            }
        };
        let route = self.with_invoked_sessions(invoked_sessions, |sessions| {
            sessions.session_links(session)?.route_due(&due)
        })?;
        Ok(match route {
            DueRoute::Local(event) => InvocationDueRoute::Local(event),
            DueRoute::Parent(event) => InvocationDueRoute::Parent(event),
            DueRoute::Invocation(routed) => {
                InvocationDueRoute::Invocation(routed.invoke_id, routed.event)
            }
            DueRoute::Undeliverable(send_id) => InvocationDueRoute::Undeliverable(send_id),
        })
    }
}

/// The host-owned `session-run` span of one root session and the telemetry
/// lease that keeps its providers alive: a batch run holds it for the run,
/// a live session from open to close. Dropping it ends the span.
pub(crate) struct RunSpan {
    context: Context,
    _lease: crate::LifecycleLease,
}

impl RunSpan {
    /// Starts `session-run` as the child of the caller's current context.
    pub(crate) fn start(core: &HostCore) -> Result<Self, RunFailure> {
        let lease = core.providers.acquire_span()?;
        let parent = crate::guest_spans::span_parent(&Context::current());
        let tracer = core.providers.tracer.tracer("scxml-host-wasmtime");
        let context = parent.with_span(tracer.start_with_context("session-run", &parent));
        Ok(Self {
            context,
            _lease: lease,
        })
    }

    /// The span's context: the parent of every session's machine batches.
    pub(crate) const fn context(&self) -> &Context {
        &self.context
    }

    /// Marks the run failed (the status only; no attribute).
    pub(crate) fn fail(&self) {
        self.context
            .span()
            .set_status(opentelemetry::trace::Status::error("failed"));
    }
}

impl Drop for RunSpan {
    fn drop(&mut self) {
        self.context.span().end();
    }
}

/// What opening a root session produced.
pub(crate) enum Opened {
    /// The root session is constructed and ready to start.
    Ready(Box<RunEngine>),
    /// The root document was rejected at load; no session ran.
    Rejected(super::LoadRejection),
}

/// The one execution engine of a root session and every child it invokes:
/// the sessions, the scheduler, and the run-wide state. A batch run
/// ([`execute`]) and a live session ([`super::LiveSession`]) drive the same
/// engine through the same stepwise scheduler calls; they differ only in
/// how far each drain may move the logical clock and whether outputs
/// stream.
pub(crate) struct RunEngine {
    core: Rc<HostCore>,
    shared: Rc<RunShared>,
    sessions: RunSessions,
    scheduler: Scheduler<RunSessions>,
    /// Drops a request still deferred when the engine ends.
    deferred_guard: DeferredGuard,
}

/// Empties the run's deferred-request slot when dropped: a constructed
/// child left there holds the run's shared state, which holds the slot.
struct DeferredGuard(Rc<RunShared>);

impl Drop for DeferredGuard {
    fn drop(&mut self) {
        drop(self.0.deferred.borrow_mut().take());
    }
}

/// A checkpoint hook of [`RunEngine`] calls.
pub(crate) trait RunHook: CheckpointHook<RunSessions> {}

impl<H: CheckpointHook<RunSessions>> RunHook for H {}

/// The hook of calls that take no snapshot.
pub(crate) fn no_hook() -> impl RunHook {
    |_: &mut RunSessions, _: &RunInvokedSessions, _: Checkpoint<'_, WitEvent>| Ok(())
}

impl RunSessions {
    /// Every machine session's handle with its clock at the shared clock,
    /// for a read-only snapshot.
    ///
    /// # Errors
    ///
    /// [`super::SnapshotFailure::ServiceInvocation`] when the run holds an
    /// invoked service: the session snapshot document cannot represent one.
    pub(crate) fn synced_handles(
        &mut self,
    ) -> Result<BTreeMap<SessionKey, &mut MachineHandle>, RunFailure> {
        let now = self.now;
        let mut handles = BTreeMap::new();
        for (key, handle) in &mut self.handles {
            match handle {
                SessionHandle::Machine(handle) => {
                    handle.links_mut()?.clock_sync(now)?;
                    handles.insert(*key, &mut **handle);
                }
                SessionHandle::Service(_) => {
                    return Err(super::SnapshotFailure::ServiceInvocation.into());
                }
            }
        }
        Ok(handles)
    }

    /// The run's session records.
    pub(crate) fn shared(&self) -> &RunShared {
        &self.shared
    }
}

impl RunEngine {
    /// Constructs the root session (not yet started).
    ///
    /// # Errors
    ///
    /// A machine, host, or construction failure.
    pub(crate) fn open(
        core: &Rc<HostCore>,
        shared: &Rc<RunShared>,
        root: RootSpec<'_>,
        execution_context: &Context,
    ) -> Result<Opened, RunFailure> {
        let spec = SessionSpec {
            session_id: root.session_id.to_owned(),
            key: SessionKey::ROOT,
            parent: None,
            data_model: root.data_model.map(str::to_owned),
            machine_ir: root.machine_ir.to_vec(),
            clock_now: 0,
            bindings: None,
        };
        let handle =
            match MachineHandle::construct(core, shared, root.embedder, spec, execution_context)? {
                Constructed::Ready(handle) => handle,
                Constructed::Rejected {
                    cause,
                    script_failure: Some(script_code),
                } => {
                    return match script_code {
                        ErrorCode::Unavailable => {
                            Ok(Opened::Rejected(super::LoadRejection::ScriptUnavailable))
                        }
                        ErrorCode::InvalidInput | ErrorCode::PayloadTooLarge => {
                            Ok(Opened::Rejected(super::LoadRejection::ScriptInvalid))
                        }
                        _ => Err(RunFailure::Machine {
                            code: crate::component::error_code_label(cause.code),
                            detail: cause.detail,
                        }),
                    };
                }
                Constructed::Rejected {
                    cause,
                    script_failure: None,
                } => {
                    return Err(RunFailure::Machine {
                        code: crate::component::error_code_label(cause.code),
                        detail: cause.detail,
                    });
                }
            };
        {
            let mut sessions = shared.sessions.borrow_mut();
            root.session_id.clone_into(&mut sessions.root_session_id);
            sessions.insert(SessionKey::ROOT, Some(root.session_id.to_owned()));
        }
        Ok(Opened::Ready(Box::new(Self {
            core: Rc::clone(core),
            shared: Rc::clone(shared),
            sessions: RunSessions {
                handles: BTreeMap::from([(SessionKey::ROOT, SessionHandle::Machine(handle))]),
                root_status: Status::Ready,
                root_return_data: None,
                now: 0,
                shared: Rc::clone(shared),
                core: Rc::clone(core),
                root_id: root.session_id.to_owned(),
                requests: shared.http.service_requests(core.service_http.as_ref()),
                last_invocation: 0,
            },
            scheduler: Scheduler::with_limits(core.invoke_limits),
            deferred_guard: DeferredGuard(Rc::clone(shared)),
        })))
    }

    /// The run-wide state.
    pub(crate) fn shared(&self) -> &Rc<RunShared> {
        &self.shared
    }

    /// Starts the root ([`Scheduler::start`]).
    pub(crate) fn start(&mut self, hook: &mut impl RunHook) -> Result<(), RunFailure> {
        self.scheduler.start(&mut self.sessions, hook)
    }

    /// Delivers one scripted input to the root ([`Scheduler::input`]).
    pub(crate) fn input(
        &mut self,
        input: &ScriptedEvent,
        hook: &mut impl RunHook,
    ) -> Result<(), RunFailure> {
        self.scheduler.input(&mut self.sessions, input, hook)
    }

    /// Drains up to `horizon` ([`Scheduler::drain_until`]).
    pub(crate) fn drain(
        &mut self,
        horizon: Horizon,
        hook: &mut impl RunHook,
    ) -> Result<DrainStop, RunFailure> {
        self.scheduler
            .drain_until(&mut self.sessions, horizon, hook)
    }

    /// Moves every session clock to `to` ms
    /// ([`Scheduler::advance_clock_to`]).
    pub(crate) fn advance_clock_to(&mut self, to: u64) -> Result<(), RunFailure> {
        self.scheduler.advance_clock_to(&mut self.sessions, to)
    }

    /// Runs the external-client phase ([`Scheduler::external_client`]).
    pub(crate) fn external_client(
        &mut self,
        client: &dyn Fn(&str) -> HttpRequest,
        hook: &mut impl RunHook,
    ) -> Result<HttpResponse, RunFailure> {
        self.scheduler.external_client(
            &mut self.sessions,
            |sessions, arrivals| sessions.external_post(client, arrivals),
            hook,
        )
    }

    /// Reaches the final checkpoint ([`Scheduler::end`]).
    pub(crate) fn end(&mut self, hook: &mut impl RunHook) -> Result<(), RunFailure> {
        self.scheduler.end(&mut self.sessions, hook)
    }

    /// The earliest pending deadline across the run, in logical ms.
    pub(crate) fn next_deadline(&mut self) -> Result<Option<u64>, RunFailure> {
        self.scheduler.next_deadline(&mut self.sessions)
    }

    /// The shared logical clock, in ms.
    pub(crate) fn now(&mut self) -> Result<u64, RunFailure> {
        self.sessions.now()
    }

    /// The status of the root's latest completed command.
    pub(crate) const fn root_status(&self) -> Status {
        self.sessions.root_status
    }

    /// Takes the finished root's `<donedata>` payload (SCXML 5.5).
    pub(crate) fn take_root_return_data(
        &mut self,
    ) -> Option<crate::bindings::stateforward::scxml::types::DataValue> {
        self.sessions.root_return_data.take()
    }

    /// The root machine's current status, read from the component.
    pub(crate) fn machine_status(&mut self) -> Result<Status, RunFailure> {
        Ok(self.sessions.session(SessionKey::ROOT)?.status()?)
    }

    /// The root's active leaf states in document order.
    pub(crate) fn configuration(&mut self) -> Result<Vec<String>, RunFailure> {
        Ok(self.sessions.session(SessionKey::ROOT)?.configuration()?)
    }

    /// The root's `_ioprocessors` entries: each linked processor's type URI
    /// or alias and this session's location, sorted by key.
    pub(crate) fn io_processors(&mut self) -> Result<BTreeMap<String, String>, RunFailure> {
        let entries = self
            .sessions
            .session_links(SessionKey::ROOT)?
            .io_processor_locations();
        match entries {
            Ok(entries) => Ok(entries),
            Err(_) => Err(self.shared.take_fault().unwrap_or(RunFailure::Harness {
                code: "ioprocessors-unavailable",
                message: "the linked processors did not report their locations".into(),
            })),
        }
    }

    /// One read-only canonical session snapshot of the stable run.
    pub(crate) fn snapshot(
        &mut self,
        options: &scxml_snapshot::Options,
    ) -> Result<Vec<u8>, RunFailure> {
        super::snapshot::take(
            &self.core.digests,
            &self.core.providers,
            options,
            &mut self.sessions,
            self.scheduler.invoked_sessions(),
            None,
        )
    }

    /// Delayed sends still pending in every unfinished session (the root session and its invoked sessions).
    pub(crate) fn pending_sends(&mut self) -> Result<usize, RunFailure> {
        let mut pending = 0_usize;
        for key in self.scheduler.invoked_sessions().keys() {
            if self.sessions.is_service(key) || (!key.is_root() && self.sessions.is_finished(key)) {
                continue;
            }
            let links = self.sessions.session_links(key)?;
            pending = pending.saturating_add(count_pending(links)?);
        }
        Ok(pending)
    }

    /// Live invoked sessions, at every depth.
    pub(crate) fn child_count(&self) -> usize {
        self.scheduler.invoked_sessions().len().saturating_sub(1)
    }

    /// The running session whose `_sessionid` is `session_id`: the root
    /// while it runs, or a live, unfinished invoked session at any depth.
    /// At most one session carries an id.
    pub(crate) fn session_key(&self, session_id: &str) -> Option<SessionKey> {
        if session_id == self.sessions.root_id {
            return (self.sessions.root_status == Status::Running).then_some(SessionKey::ROOT);
        }
        self.shared
            .sessions
            .borrow()
            .key_of(session_id)
            .filter(|key| !key.is_root() && !self.sessions.is_finished(*key))
    }

    /// The invoke ids from the root to the session `key`.
    pub(crate) fn session_path(&self, key: SessionKey) -> Option<Vec<String>> {
        self.scheduler.invoked_sessions().path(key)
    }

    /// Whether the invoke ids from the root to the session `key` are
    /// exactly `path`, compared without building them
    /// ([`scxml_scheduler::InvokedSessions::has_path`]).
    pub(crate) fn session_has_path(&self, key: SessionKey, path: &[String]) -> bool {
        self.scheduler.invoked_sessions().has_path(key, path)
    }

    /// Raises the `error.communication` of the `<send>` `send_id` in the
    /// session at `path` at once, on its internal queue
    /// ([`Scheduler::communication_error`]).
    ///
    /// # Errors
    ///
    /// Any command failure.
    pub(crate) fn communication_error(
        &mut self,
        key: SessionKey,
        send_id: String,
    ) -> Result<(), RunFailure> {
        self.scheduler
            .communication_error(&mut self.sessions, key, Some(send_id))
    }

    /// Hands each settled request of an invoked service to its invocation,
    /// when it is still in the run and waits for it (one boundary span
    /// each); a response no live invocation waits for is dropped. Returns
    /// what each delivered response was, for the record: the invoke ids
    /// from the root to its invocation, its request id, and its outcome.
    /// The caller drains afterwards: what the invocations returned is
    /// queued on the invoking sessions.
    ///
    /// # Errors
    ///
    /// A host failure of a delivery.
    pub(crate) fn deliver_http_responses(
        &mut self,
        settled: Vec<crate::service::Settled>,
    ) -> Result<Vec<super::RecordedResponse>, RunFailure> {
        let mut delivered = Vec::new();
        for item in settled {
            let Some(key) = self.sessions.service_key(item.invocation) else {
                continue;
            };
            let Some(path) = self.session_path(key) else {
                continue;
            };
            let request = item.request;
            let outcome = item.outcome.clone();
            let mut answered = false;
            self.sessions.with_invoked_sessions(
                self.scheduler.invoked_sessions_mut(),
                |sessions| {
                    sessions.service_input(
                        key,
                        scxml_telemetry::Operation::HostWasmtimeServiceHttpFetch,
                        |service, now, requests| {
                            let step = service.respond(request, item.outcome, now, requests);
                            answered = step.is_some();
                            step
                        },
                    )
                },
            )?;
            if answered {
                delivered.push(super::RecordedResponse {
                    path,
                    request,
                    outcome,
                });
            }
        }
        Ok(delivered)
    }

    /// Delivers one recorded response to the invoked service at `path` (a
    /// replay); `false` when no live invocation of that lineage waits for
    /// that request.
    ///
    /// # Errors
    ///
    /// A host failure of the delivery.
    pub(crate) fn deliver_recorded_response(
        &mut self,
        response: &super::RecordedResponse,
    ) -> Result<bool, RunFailure> {
        let Some(key) = self.sessions.handles.keys().copied().find(|key| {
            self.sessions.is_service(*key)
                && self
                    .scheduler
                    .invoked_sessions()
                    .has_path(*key, &response.path)
        }) else {
            return Ok(false);
        };
        let mut answered = false;
        let outcome = response.outcome.clone();
        let request = response.request;
        self.sessions
            .with_invoked_sessions(self.scheduler.invoked_sessions_mut(), |sessions| {
                sessions.service_input(
                    key,
                    scxml_telemetry::Operation::HostWasmtimeServiceHttpFetch,
                    |service, now, requests| {
                        let step = service.respond(request, outcome, now, requests);
                        answered = step.is_some();
                        step
                    },
                )
            })?;
        Ok(answered)
    }

    /// Whether the run waits for the response of an invoked service's
    /// request: one in flight (or awaited from the record), or one settled
    /// and not yet delivered.
    pub(crate) fn awaiting_http_responses(&self) -> bool {
        self.sessions.services_awaiting()
            || self
                .shared
                .http
                .http_responses
                .as_ref()
                .is_some_and(|responses| responses.has_settled())
    }

    /// Queues `event` on the external queue of the session `key`; `false`
    /// when no such session is present.
    pub(crate) fn push_external(&mut self, key: SessionKey, event: WitEvent) -> bool {
        self.scheduler
            .invoked_sessions_mut()
            .push_external(key, event)
    }

    /// Decodes one inbound `BasicHTTP` message with the run's receiver
    /// component, the event carrying `trace_context`.
    pub(crate) fn http_receive(
        &self,
        request: &HttpRequest,
        trace_context: Option<crate::bindings::stateforward::scxml::types::TraceContext>,
    ) -> Result<Result<WitEvent, crate::basichttp_types::Rejection>, HostError> {
        self.shared.with_receiver(&self.core, |receiver| {
            receiver.receive(request, trace_context)
        })
    }

    /// The response the receiver component chooses for `delivery`.
    pub(crate) fn http_respond(&self, delivery: Delivery) -> Result<HttpResponse, HostError> {
        self.shared
            .with_receiver(&self.core, |receiver| receiver.respond(delivery))
    }

    /// Exits the root interpreter (`session.cancel`: `<onexit>` of the
    /// active configuration, then every invocation cancelled). Only a
    /// running root is cancelled.
    pub(crate) fn cancel_root(&mut self) -> Result<(), RunFailure> {
        if self.sessions.root_status != Status::Running {
            return Ok(());
        }
        self.sessions
            .with_invoked_sessions(self.scheduler.invoked_sessions_mut(), |sessions| {
                sessions.command(SessionKey::ROOT, MachineHandle::cancel)
            })
    }

    /// Ends the run: every store is dropped, each after every session below
    /// it (each session's extension components shut down once), then the
    /// ledger is returned.
    pub(crate) fn finish(self) -> (u64, Vec<LogEntry>, Vec<TraceEntry>) {
        let Self {
            shared,
            mut sessions,
            scheduler,
            core,
            deferred_guard,
        } = self;
        drop(deferred_guard);
        sessions.teardown(scheduler.invoked_sessions().keys());
        drop(sessions);
        drop(scheduler);
        drop(core);
        let ledger = shared.ledger.replace(Ledger::default());
        (ledger.commands, ledger.root_logs, ledger.trace)
    }
}

fn count_pending(links: &mut SessionLinks) -> Result<usize, RunFailure> {
    Ok(links
        .io_snapshot()?
        .map_err(|error| RunFailure::Machine {
            code: crate::component::error_code_label(error.code),
            detail: error.detail,
        })?
        .len())
}

/// The root session of one run.
#[derive(Clone, Copy)]
pub(crate) struct RootSpec<'a> {
    pub(crate) machine_ir: &'a [u8],
    pub(crate) data_model: Option<&'a str>,
    pub(crate) session_id: &'a str,
    pub(crate) embedder: &'a Rc<dyn SessionEmbedder>,
}

/// Executes one batch run (see [`super::SessionHost::run`]): the stepwise
/// engine with the logical clock unbounded.
pub(super) fn execute(
    core: &Rc<HostCore>,
    run: &SessionRun<'_>,
) -> Result<SessionOutcome, SessionHostError> {
    let shared = Rc::new(RunShared::new(
        run.max_commands,
        RunMode::Batch,
        RunHttp::batch(),
        BATCH_START_EPOCH_MS,
    ));
    let fail = |failure: RunFailure, shared: &RunShared| SessionHostError {
        failure,
        commands: shared.commands(),
    };
    if run.max_commands == 0 {
        return Err(fail(
            RunFailure::Harness {
                code: "command-budget-invalid",
                message: "max_commands must be greater than zero".into(),
            },
            &shared,
        ));
    }
    let span = RunSpan::start(core).map_err(|failure| fail(failure, &shared))?;
    let result = execute_in(core, &shared, run, span.context());
    if result.is_err() {
        span.fail();
    }
    drop(span);
    result.map_err(|failure| fail(failure, &shared))
}

fn execute_in(
    core: &Rc<HostCore>,
    shared: &Rc<RunShared>,
    run: &SessionRun<'_>,
    execution_context: &Context,
) -> Result<SessionOutcome, RunFailure> {
    let root = RootSpec {
        machine_ir: run.machine_ir,
        data_model: run.data_model,
        session_id: run.session_id,
        embedder: &run.embedder,
    };
    let mut engine = match RunEngine::open(core, shared, root, execution_context)? {
        Opened::Ready(engine) => engine,
        Opened::Rejected(rejection) => {
            return Ok(SessionOutcome {
                terminal: Terminal::Rejected(rejection),
                configuration: Vec::new(),
                commands: 0,
                root_logs: Vec::new(),
                trace: Vec::new(),
                http_response: None,
                snapshots: Vec::new(),
                snapshot_failures: Vec::new(),
            });
        }
    };
    let (http_response, (snapshots, snapshot_failures)) = schedule(core, &mut engine, run)?;
    let terminal = match engine.root_status() {
        Status::Running => Terminal::Running,
        Status::Finished => Terminal::Finished,
        Status::Ready | Status::Failed | Status::ShutDown => {
            return Err(RunFailure::Harness {
                code: "engine-invariant",
                message: "the root session is neither running nor finished after start".into(),
            });
        }
    };
    let configuration = engine.configuration()?;
    let (commands, root_logs, trace) = engine.finish();
    Ok(SessionOutcome {
        terminal,
        configuration,
        commands,
        root_logs,
        trace,
        http_response,
        snapshots,
        snapshot_failures,
    })
}

/// The snapshots a batch run took and the ones that failed.
type Snapshots = (Vec<CheckpointSnapshot>, Vec<CheckpointSnapshotFailure>);

/// Runs the batch schedule over `engine`: `start`, then the scripted inputs
/// or the external client's arrivals, then an unbounded drain and the final
/// checkpoint, with the snapshot hook only when snapshots are requested.
fn schedule(
    core: &HostCore,
    engine: &mut RunEngine,
    run: &SessionRun<'_>,
) -> Result<(Option<HttpResponse>, Snapshots), RunFailure> {
    let Some(mut snapshotter) = Snapshotter::new(&run.snapshots, &core.digests, &core.providers)?
    else {
        let response = batch(engine, run, &mut no_hook())?;
        return Ok((response, (Vec::new(), Vec::new())));
    };
    let mut hook = |sessions: &mut RunSessions,
                    invoked_sessions: &RunInvokedSessions,
                    checkpoint: Checkpoint<'_, WitEvent>| {
        snapshotter.at(sessions, invoked_sessions, &checkpoint)
    };
    let response = batch(engine, run, &mut hook)?;
    Ok((response, snapshotter.finish()?))
}

/// The batch call sequence of [`schedule`].
fn batch(
    engine: &mut RunEngine,
    run: &SessionRun<'_>,
    hook: &mut impl RunHook,
) -> Result<Option<HttpResponse>, RunFailure> {
    engine.start(hook)?;
    let response = match run.external_client {
        None => {
            for input in run.events {
                engine.input(input, hook)?;
            }
            None
        }
        Some(client) => Some(engine.external_client(client, hook)?),
    };
    engine.drain(Horizon::Unbounded, hook)?;
    engine.end(hook)?;
    Ok(response)
}

/// Converts a host failure in a capability call into the error the machine
/// receives, recording the fault so the run reports the real cause.
pub(crate) fn fatal(shared: &RunShared, failure: RunFailure) -> ComponentError {
    let code = match failure {
        RunFailure::CommandBudget => ErrorCode::CapacityExhausted,
        _ => ErrorCode::Platform,
    };
    shared.fault(failure);
    crate::component::component_error(code, "session host failure")
}
