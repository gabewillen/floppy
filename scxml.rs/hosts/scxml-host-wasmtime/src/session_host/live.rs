//! Live sessions: one root session (and the children it invokes) that stays
//! alive between calls, processing each external event to a stable configuration as it
//! arrives and firing delayed sends at their deadlines on a pluggable
//! clock. See `docs/live-session.md`.
//!
//! # Engine
//!
//! A live session drives the same [`RunEngine`] as a batch run
//! ([`super::SessionHost::run`]) through the same stepwise scheduler calls.
//! The only differences are how far each drain may move the logical clock
//! ([`Horizon::Until`] the session's current time instead of
//! [`Horizon::Unbounded`]), that every public call has its
//! own command budget and per-session `<log>` bound, and that outputs
//! stream to the embedder instead of being collected.
//!
//! # Time
//!
//! Every session clock is a logical millisecond counter that starts at 0
//! when the session opens. A [`LiveClock::Timed`] session reads its
//! [`SessionClock`] at each call and moves the logical clock to that time
//! (never backwards); a [`LiveClock::Logical`] session moves only when the
//! embedder asks ([`LiveSession::advance_to`],
//! [`LiveSession::run_until_idle`]). Nothing runs between calls: the
//! embedder owns the loop, waits until [`LiveSession::next_deadline`]
//! (or the next input), and calls [`LiveSession::advance`]. The host starts
//! no thread, so the single-threaded (`!Send`) session never crosses one.
//!
//! # Record and replay
//!
//! A session opened with `record` logs every input as an [`InputRecord`]
//! carrying the logical time it was applied at, including its `BasicHTTP`
//! network inputs: every inbound message that reached a running session
//! (the decoded event and the response status), every delivery of late
//! `error.communication` events (with their senders), and every outbound
//! transport decision (accepted or refused). Outbound requests themselves
//! are outputs and are not recorded. Opening a [`LiveClock::Logical`]
//! session over the same document with the records as
//! [`SessionOpen::replay`] (and the same public URL, but no transport) and
//! applying the records in order ([`LiveSession::apply`]) makes exactly the
//! same engine calls with exactly the same times and answers, without any
//! network, so it reproduces every output and every snapshot byte for
//! byte.
//!
//! # `BasicHTTP`
//!
//! A session opened with a [`LiveHttp`] network advertises
//! `{public_url}/sessions/{_sessionid}/basichttp` as every session's
//! `BasicHTTP` location (root and invoked sessions at any depth), accepts
//! inbound messages through [`LiveSession::receive_http`], and hands every
//! POST outside its own address space to the embedder's
//! [`super::OutboundHttp`] transport, which never blocks the session: a
//! request that fails after it was accepted comes back as a platform
//! `error.communication` on its sender's internal event queue through
//! [`LiveSession::deliver_communication_errors`] (see `network`).
//!
//! # Telemetry
//!
//! One boundary span per call with the closed operation labels
//! `session-open`, `session-send`, `session-advance` (only when a deadline
//! fires or the logical clock runs to idle; an advance that fires nothing
//! emits nothing), `session-receive-http`, `session-communication-error`,
//! and `session-close`, plus the existing `session-snapshot` span; all of
//! them run under the session's one `session-run` span. No attribute
//! carries a session id, URL, event name, time, or payload; commands and
//! fuel stay measurements.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Instant;

use scxml_scheduler::{DrainStop, Horizon};
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent};

use super::loopback::inbound_http_trace_context;
use super::network::{
    CommunicationSender, LiveHttp, OutboundDecision, OutboundJournal, OwedFailure, RunHttp,
    inbound_session_id,
};
use super::run::{Opened, RootSpec, RunEngine, RunMode, RunShared, RunSpan, no_hook};
use super::{HostCore, LogEntry, RunFailure, ScriptedEvent, SessionEmbedder, SessionHostError};
use crate::basichttp_types::{Delivery, HttpRequest, HttpResponse};
use crate::bindings::exports::stateforward::scxml::machine_api::Status;

/// A monotonic source of the time since a live session opened.
///
/// Implementations must never return a smaller value than an earlier call
/// did; the session never moves its logical clock backwards and fails
/// closed with `clock-regression` if asked to.
pub trait SessionClock {
    /// Milliseconds elapsed since the session opened.
    fn elapsed_ms(&self) -> u64;
}

/// The process's monotonic wall clock ([`Instant`]), from the moment it
/// was created.
#[derive(Clone, Copy, Debug)]
pub struct WallClock {
    origin: Instant,
}

impl WallClock {
    /// A wall clock whose time 0 is now.
    #[must_use]
    pub fn start() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl SessionClock for WallClock {
    fn elapsed_ms(&self) -> u64 {
        // Saturates after ~584 million years.
        u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// A clock the embedder sets by hand: deterministic tests of timed
/// sessions. Clones share one time.
#[derive(Clone, Debug, Default)]
pub struct ManualClock {
    now: Rc<Cell<u64>>,
}

impl ManualClock {
    /// A manual clock at 0 ms.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Moves the clock to `ms`; a smaller value than the current time is
    /// ignored (the clock is monotonic).
    pub fn set(&self, ms: u64) {
        self.now.set(self.now.get().max(ms));
    }

    /// Moves the clock forward by `delta` ms (saturating).
    pub fn advance(&self, delta: u64) {
        self.now.set(self.now.get().saturating_add(delta));
    }
}

impl SessionClock for ManualClock {
    fn elapsed_ms(&self) -> u64 {
        self.now.get()
    }
}

/// How a live session's logical clock moves.
pub enum LiveClock {
    /// Only when the embedder asks: [`LiveSession::advance`] fires the next
    /// deadline, [`LiveSession::advance_to`] moves to a time, and
    /// [`LiveSession::run_until_idle`] fires every deadline (the batch
    /// run's clock). Replay uses it.
    Logical,
    /// To the clock's time at every call: an event is processed at the time
    /// it arrives and [`LiveSession::advance`] fires every deadline the
    /// clock has reached.
    Timed(Box<dyn SessionClock>),
}

impl std::fmt::Debug for LiveClock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Logical => "Logical",
            Self::Timed(_) => "Timed",
        })
    }
}

/// How to open one live session.
pub struct SessionOpen<'a> {
    /// The root document's lowered Machine IR (untrusted; the host serves it
    /// unbounded and the wasm machine applies its own byte bound).
    pub machine_ir: &'a [u8],
    /// The root document's `datamodel` attribute, selecting its data model.
    pub data_model: Option<&'a str>,
    /// The root session's `_sessionid` (a valid SCXML name).
    pub session_id: &'a str,
    /// Documents and scripts for every session.
    pub embedder: Rc<dyn SessionEmbedder>,
    /// Budget of engine commands of each call (non-zero), shared by every
    /// session the call reaches.
    pub max_commands: u64,
    /// How the logical clock moves.
    pub clock: LiveClock,
    /// Whether to log every input as an [`InputRecord`]
    /// ([`LiveSession::take_recorded`]), network inputs included.
    pub record: bool,
    /// The records of a recorded session this one replays: its outbound
    /// `BasicHTTP` decisions ([`InputRecord::OutboundHttp`]) answer this
    /// session's outbound requests in order, from the start on. A replaying
    /// session runs on [`LiveClock::Logical`], does not record, and has no
    /// outbound transport (`replay-invalid` otherwise); the embedder then
    /// applies the records with [`LiveSession::apply`].
    pub replay: Option<&'a [InputRecord]>,
    /// The session's `BasicHTTP` network; the default is none (loopback
    /// locations, and every POST outside them undeliverable).
    pub http: LiveHttp,
}

/// One output a live session streams to its embedder, in the order the
/// sessions produced them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionOutput {
    /// One `<log>` record of any session.
    Log {
        /// The producing session's `_sessionid`.
        session: String,
        /// The invoking session's `_sessionid`; `None` for the root.
        parent: Option<String>,
        /// The record.
        entry: LogEntry,
    },
    /// The root reached a top-level final state (emitted once), with the
    /// evaluated `<donedata>` of that final state when it has one (SCXML
    /// 5.5: the processor "SHOULD return the data to the environment in an
    /// implementation-dependent manner"; `undefined` when its evaluation
    /// failed).
    Done {
        /// The root's `<donedata>` payload.
        data: Option<scxml_types::DataValue>,
    },
}

/// Status of a live session between calls.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveStatus {
    /// Running; it accepts events.
    Running,
    /// The root reached a top-level final state; it accepts no event.
    Finished,
    /// A call failed; only [`LiveSession::close`] remains.
    Failed,
}

/// What one call left behind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LiveCallResult {
    /// The logical time the call ran at, in ms since open.
    pub at_ms: u64,
    /// The session status after the call.
    pub status: LiveStatus,
    /// The earliest pending deadline after the call, in logical ms.
    pub next_deadline: Option<u64>,
    /// Engine commands the call charged.
    pub commands: u64,
}

/// One input of a live session, as recorded for replay. Times are the
/// logical ms since open at which the input was applied.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InputRecord {
    /// The start of the session clock: logical time 0 is `epoch_ms` ms
    /// after the Unix epoch (what ECMAScript `Date` reads is this plus the
    /// logical time). The first record of every recorded session; a
    /// replaying session starts its clock there.
    Start {
        /// The Unix epoch ms of logical time 0.
        epoch_ms: u64,
    },
    /// [`LiveSession::send`] of `event` at `at_ms`.
    Event {
        /// The logical time.
        at_ms: u64,
        /// The event.
        event: ScriptedEvent,
    },
    /// [`LiveSession::advance`] or [`LiveSession::advance_to`] to `to_ms`.
    Advance {
        /// The logical time.
        to_ms: u64,
    },
    /// [`LiveSession::run_until_idle`].
    Idle,
    /// [`LiveSession::snapshot`] with these options.
    Snapshot {
        /// The document options.
        options: scxml_snapshot::Options,
    },
    /// [`LiveSession::receive_http`] of one inbound `BasicHTTP` message that
    /// reached the running session `session_id` at `at_ms`: the event the
    /// component decoded and the response status it chose.
    Http {
        /// The logical time.
        at_ms: u64,
        /// The addressed session's `_sessionid`.
        session_id: String,
        /// The invoke ids from the root to the addressed session (empty for
        /// the root): its lineage. Replay addresses the session by
        /// `session_id` and delivers only to a session of this lineage; any
        /// other diverges.
        path: Vec<String>,
        /// The decoded external event.
        event: scxml_types::Event,
        /// The response status the client got.
        status: u16,
    },
    /// [`LiveSession::deliver_communication_errors`] of the late
    /// `error.communication` events owed to `senders`, at `at_ms`.
    CommunicationErrors {
        /// The logical time.
        at_ms: u64,
        /// The failed sends (session and send id) owed one event each, in
        /// delivery order: only those whose sending session was still in
        /// the run when the record was written (an event owed to a session
        /// that has left it reaches no one, so a replay needs none).
        senders: Vec<CommunicationSender>,
    },
    /// One outbound `BasicHTTP` request's transport decision, in request
    /// order: `accepted` (its outcome arrives later), or refused (the
    /// `<send>` raised `error.communication` at once). A replaying session
    /// consumes these when it opens ([`SessionOpen::replay`]).
    OutboundHttp {
        /// Whether the transport accepted the request.
        accepted: bool,
    },
    /// [`LiveSession::deliver_http_responses`] of the HTTP response to one
    /// request an invoked service made (`outbound-http.fetch`), delivered at
    /// `at_ms`. A replay delivers it from the record instead of the network.
    HttpResponse {
        /// The logical time.
        at_ms: u64,
        /// The invoke ids from the root to the invoked service: its
        /// lineage. Replay delivers only to the invocation of this lineage;
        /// any other diverges.
        path: Vec<String>,
        /// The request's number within its invocation, from 0 in the order
        /// the invocation made them.
        request: u64,
        /// The response, or the transport's failure.
        response: Result<crate::ExtensionHttpResponse, crate::ExtensionHttpFailure>,
    },
    /// [`LiveSession::deliver_http_responses`] of one part of the streamed
    /// response to one request an invoked service made
    /// (`outbound-http.send`), delivered at `at_ms`: its status, its next
    /// body bytes (those that arrived since the last delivery, joined), or
    /// its end. A replay delivers it from the record instead of the network.
    HttpResponsePart {
        /// The logical time.
        at_ms: u64,
        /// The invoke ids from the root to the invoked service: its
        /// lineage, as [`InputRecord::HttpResponse`]'s.
        path: Vec<String>,
        /// The request's number within its invocation.
        request: u64,
        /// The part, or the transport's failure, which ends the response.
        part: Result<crate::ExtensionHttpPart, crate::ExtensionHttpFailure>,
    },
}

/// What [`LiveSession::apply`] produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Applied {
    /// The result of an event or advance call.
    Call(LiveCallResult),
    /// A snapshot document.
    Snapshot(Vec<u8>),
    /// An inbound `BasicHTTP` message: its call result when the event was
    /// enqueued (as [`HttpReceipt::result`]).
    Received(Option<LiveCallResult>),
    /// An outbound decision, already consumed when the session opened;
    /// nothing ran.
    OutboundDecision,
    /// The clock's start, already used when the session opened; nothing
    /// ran.
    Started,
}

/// What [`LiveSession::close`] did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CloseReport {
    /// The status the session had when closed.
    pub status: LiveStatus,
    /// Delayed sends that were pending (root and children) and are now
    /// cancelled.
    pub cancelled_sends: usize,
    /// Invoked children that were live and are now cancelled.
    pub cancelled_children: usize,
    /// Outputs of the close itself (`<onexit>` logs of the exit).
    pub outputs: Vec<SessionOutput>,
}

/// A live root session. Single-threaded (`!Send`), like its host.
///
/// Every public call fails closed on a session that already
/// failed, charges its own command budget, and leaves every session
/// stable. A failed call moves the session to [`LiveStatus::Failed`];
/// an event the machine rejects without failing (and a snapshot that
/// fails) leaves it as it was. Dropping the session shuts it down like
/// [`Self::close`] without running the root's exit.
pub struct LiveSession {
    // Field order is drop order: every store is dropped before the run span
    // ends.
    engine: Box<RunEngine>,
    span: RunSpan,
    clock: LiveClock,
    /// The clock's reading when the root started: session time 0.
    origin_ms: u64,
    status: LiveStatus,
    done_reported: bool,
    recorded: Option<Vec<InputRecord>>,
}

impl std::fmt::Debug for LiveSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiveSession")
            .field("status", &self.status)
            .field("clock", &self.clock)
            .finish_non_exhaustive()
    }
}

fn harness(code: &'static str, message: &str) -> RunFailure {
    RunFailure::Harness {
        code,
        message: message.into(),
    }
}

const fn category(failure: &RunFailure) -> ErrorCategory {
    match failure {
        RunFailure::CommandBudget | RunFailure::CommandBound => ErrorCategory::Budget,
        RunFailure::Machine { .. } => ErrorCategory::Execution,
        RunFailure::Harness { .. } => ErrorCategory::Harness,
        RunFailure::Host(_) => ErrorCategory::Platform,
        RunFailure::Snapshot(_) => ErrorCategory::Validation,
    }
}

/// What one inbound `BasicHTTP` message did ([`LiveSession::receive_http`]).
#[derive(Debug)]
pub struct HttpReceipt {
    /// The response for the client: the one the `BasicHTTP` component
    /// chose for the delivery outcome (200 enqueued, 400/405/413 rejected,
    /// 404 no running session answers the target, 503 queue full), or a
    /// bare 500 when the host itself failed before the component answered.
    pub response: HttpResponse,
    /// The call result that processed the event: `Ok(None)` when nothing was
    /// enqueued. An event can be enqueued (200) by a call that then failed.
    pub result: Result<Option<LiveCallResult>, SessionHostError>,
}

/// The response when the host failed before the component could answer.
fn host_failure_response() -> HttpResponse {
    HttpResponse {
        status: 500,
        headers: Vec::new(),
        body: Vec::new(),
    }
}

/// The host's wall clock as Unix epoch ms (0 before the epoch).
fn wall_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn replay_diverged(message: &str) -> RunFailure {
    harness("replay-diverged", message)
}

/// What [`LiveSession::receive_http`] learned of one inbound message: the
/// delivery outcome (`None`: the host failed before the component could
/// answer), the call result, and what a recording session records of it.
type ReceivedHttp = (
    Option<Delivery>,
    Result<Option<LiveCallResult>, SessionHostError>,
    Option<(u64, String, Vec<String>, scxml_types::Event)>,
);

/// Opens one live session (see [`super::SessionHost::open`]).
pub(super) fn open(
    core: &Rc<HostCore>,
    mut open: SessionOpen<'_>,
) -> Result<LiveSession, SessionHostError> {
    let span = scxml_telemetry::observe(Module::HostWasmtime, Operation::HostWasmtimeSessionOpen);
    let http = std::mem::take(&mut open.http);
    let invalid_replay = open.replay.is_some()
        && (open.record || http.outbound.is_some() || matches!(open.clock, LiveClock::Timed(_)));
    let journal = match open.replay {
        Some(records) => OutboundJournal::Replaying(
            records
                .iter()
                .filter_map(|record| match record {
                    InputRecord::OutboundHttp { accepted } => {
                        Some(OutboundDecision::from(*accepted))
                    }
                    _ => None,
                })
                .collect(),
        ),
        None if open.record => OutboundJournal::Recording(Vec::new()),
        None => OutboundJournal::Off,
    };
    // Logical time 0 of the session clock (see `InputRecord::Start`): a
    // replay's recorded start, the wall clock for a timed session, and the
    // batch start otherwise.
    let start_epoch_ms = match (open.replay, &open.clock) {
        (Some(records), _) => records
            .iter()
            .find_map(|record| match record {
                InputRecord::Start { epoch_ms } => Some(*epoch_ms),
                _ => None,
            })
            .unwrap_or(super::run::BATCH_START_EPOCH_MS),
        (None, LiveClock::Timed(_)) => wall_epoch_ms(),
        (None, LiveClock::Logical) => super::run::BATCH_START_EPOCH_MS,
    };
    let shared = Rc::new(RunShared::new(
        open.max_commands,
        RunMode::Live,
        RunHttp::live(http, journal),
        start_epoch_ms,
    ));
    let result = if invalid_replay {
        Err(harness(
            "replay-invalid",
            "a replaying session runs on the logical clock, does not record, and has no outbound transport",
        ))
    } else {
        open_in(core, &shared, open)
    };
    match &result {
        Ok(_) => span.event(SignalEvent::TransformationCompleted),
        Err(failure) => {
            span.event(SignalEvent::InputRejected);
            span.fail(category(failure));
        }
    }
    result.map_err(|failure| SessionHostError {
        failure,
        commands: shared.commands(),
    })
}

fn open_in(
    core: &Rc<HostCore>,
    shared: &Rc<RunShared>,
    open: SessionOpen<'_>,
) -> Result<LiveSession, RunFailure> {
    if open.max_commands == 0 {
        return Err(harness(
            "command-budget-invalid",
            "max_commands must be greater than zero",
        ));
    }
    let span = RunSpan::start(core)?;
    shared.begin_step();
    let root = RootSpec {
        machine_ir: open.machine_ir,
        data_model: open.data_model,
        session_id: open.session_id,
        embedder: &open.embedder,
    };
    let opened = RunEngine::open(core, shared, root, span.context());
    let engine = match opened {
        Ok(Opened::Ready(engine)) => engine,
        Ok(Opened::Rejected(rejection)) => {
            span.fail();
            return Err(harness(
                rejection.code(),
                "the root document was rejected at load (SCXML 1.0 section 5.8)",
            ));
        }
        Err(failure) => {
            span.fail();
            return Err(failure);
        }
    };
    // Session time 0 is the moment the root starts, after construction (and
    // any first-use component compilation) finished.
    let origin_ms = match &open.clock {
        LiveClock::Logical => 0,
        LiveClock::Timed(clock) => clock.elapsed_ms(),
    };
    let mut session = LiveSession {
        engine,
        span,
        clock: open.clock,
        origin_ms,
        status: LiveStatus::Running,
        done_reported: false,
        recorded: open.record.then(|| {
            vec![InputRecord::Start {
                epoch_ms: shared.start_epoch_ms,
            }]
        }),
    };
    let started = session.engine.start(&mut no_hook()).and_then(|()| {
        session
            .engine
            .drain(Horizon::Until(0), &mut no_hook())
            .map(|_| ())
    });
    session.collect_outbound();
    match started.and_then(|()| session.observe()) {
        Ok(()) => Ok(session),
        Err(failure) => {
            session.span.fail();
            Err(failure)
        }
    }
}

/// Whether a failed input left the session as it was: the machine rejected
/// the event (or it arrived after the root finished) without failing.
fn is_rejection(failure: &RunFailure) -> bool {
    matches!(
        failure,
        RunFailure::Machine { .. }
            | RunFailure::Harness {
                code: "event-after-finish" | "event-name-invalid",
                ..
            }
    )
}

impl LiveSession {
    /// The session status.
    #[must_use]
    pub const fn status(&self) -> LiveStatus {
        self.status
    }

    fn error(&self, failure: RunFailure) -> SessionHostError {
        SessionHostError {
            failure,
            commands: self.engine.shared().step_commands(),
        }
    }

    fn require_open(&self) -> Result<(), SessionHostError> {
        if self.status == LiveStatus::Failed {
            return Err(self.error(harness(
                "session-failed",
                "the session failed; only close remains",
            )));
        }
        Ok(())
    }

    /// Records the root's status after a call and streams `Done` once.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "every call outcome flows through one fallible observation"
    )]
    fn observe(&mut self) -> Result<(), RunFailure> {
        if self.engine.root_status() == Status::Finished {
            self.status = LiveStatus::Finished;
            if !self.done_reported {
                self.done_reported = true;
                let data = self
                    .engine
                    .take_root_return_data()
                    .and_then(|value| crate::session::native_value(value).ok());
                self.engine.shared().emit(SessionOutput::Done { data });
            }
        }
        Ok(())
    }

    /// Runs one call under `operation`'s span (none when `operation` is
    /// `None`). `body` returns `Ok(Err(_))` for a rejection that leaves the
    /// session as it was.
    fn step(
        &mut self,
        operation: Option<Operation>,
        at_ms: u64,
        body: impl FnOnce(&mut RunEngine) -> Result<Result<(), RunFailure>, RunFailure>,
    ) -> Result<LiveCallResult, SessionHostError> {
        self.require_open()?;
        let span =
            operation.map(|operation| scxml_telemetry::observe(Module::HostWasmtime, operation));
        self.engine.shared().begin_step();
        let result = body(&mut self.engine).and_then(|outcome| {
            self.observe()?;
            Ok(outcome)
        });
        self.collect_outbound();
        let result = result.and_then(|outcome| {
            let next_deadline = match self.status {
                LiveStatus::Finished => None,
                _ => self.engine.next_deadline()?,
            };
            Ok(outcome.map(|()| next_deadline))
        });
        match result {
            Ok(Ok(next_deadline)) => {
                if let Some(span) = &span {
                    span.event(SignalEvent::TransformationCompleted);
                }
                Ok(LiveCallResult {
                    at_ms,
                    status: self.status,
                    next_deadline,
                    commands: self.engine.shared().step_commands(),
                })
            }
            Ok(Err(rejection)) => {
                if let Some(span) = &span {
                    span.event(SignalEvent::InputRejected);
                }
                Err(self.error(rejection))
            }
            Err(failure) => {
                if let Some(span) = &span {
                    span.event(SignalEvent::InputRejected);
                    span.fail(category(&failure));
                }
                self.status = LiveStatus::Failed;
                self.span.fail();
                Err(self.error(failure))
            }
        }
    }

    fn record(&mut self, record: InputRecord) {
        if let Some(recorded) = &mut self.recorded {
            recorded.push(record);
        }
    }

    /// Moves the outbound decisions journaled since the last call into the
    /// record log.
    fn collect_outbound(&mut self) {
        let decisions = self.engine.shared().http.take_decisions();
        if let Some(recorded) = &mut self.recorded {
            recorded.extend(
                decisions
                    .into_iter()
                    .map(|decision| InputRecord::OutboundHttp {
                        accepted: decision.accepted(),
                    }),
            );
        }
    }

    /// The logical time of a call now: the clock's time for a timed
    /// session (never before the current logical time), else the current
    /// logical time.
    fn time(&mut self) -> Result<u64, SessionHostError> {
        let now = self.engine.now().map_err(|failure| self.fatal(failure))?;
        Ok(self.clock_ms().map_or(now, |clock| clock.max(now)))
    }

    /// A timed session's clock reading in session time (ms since the root
    /// started); `None` for a logical session. An embedder waits until it
    /// reaches [`Self::next_deadline`].
    #[must_use]
    pub fn clock_ms(&self) -> Option<u64> {
        match &self.clock {
            LiveClock::Logical => None,
            LiveClock::Timed(clock) => Some(clock.elapsed_ms().saturating_sub(self.origin_ms)),
        }
    }

    fn fatal(&mut self, failure: RunFailure) -> SessionHostError {
        self.status = LiveStatus::Failed;
        self.span.fail();
        self.error(failure)
    }

    /// The current logical time, in ms since open.
    ///
    /// # Errors
    ///
    /// A clock component failure (the session fails).
    pub fn now_ms(&mut self) -> Result<u64, SessionHostError> {
        self.require_open()?;
        self.engine.now().map_err(|failure| self.fatal(failure))
    }

    /// The earliest pending deadline across every session, in logical ms
    /// since open; `None` when nothing is scheduled. A timed embedder waits
    /// until its clock reaches it, then calls [`Self::advance`].
    ///
    /// # Errors
    ///
    /// A clock component failure (the session fails).
    pub fn next_deadline(&mut self) -> Result<Option<u64>, SessionHostError> {
        self.require_open()?;
        if self.status == LiveStatus::Finished {
            return Ok(None);
        }
        self.engine
            .next_deadline()
            .map_err(|failure| self.fatal(failure))
    }

    /// Sends one external event to the root and processes it to
    /// a stable configuration: deadlines the clock already reached fire first (in
    /// deadline order), then the event is delivered, then every session
    /// runs until nothing is runnable at the current time. The event is
    /// validated exactly as a batch run's scripted events are.
    ///
    /// # Errors
    ///
    /// Without failing the session: an invalid event name
    /// (`event-name-invalid`), an event after the root finished
    /// (`event-after-finish`), or an event the machine rejects
    /// ([`RunFailure::Machine`] with the session still running). Failing
    /// it: budget exhaustion, a failed machine session, a scheduler
    /// invariant, or a host failure. A session that already failed answers
    /// `session-failed`.
    pub fn send(&mut self, event: &ScriptedEvent) -> Result<LiveCallResult, SessionHostError> {
        self.require_open()?;
        let at_ms = self.time()?;
        self.send_at(event, at_ms)
    }

    fn send_at(
        &mut self,
        event: &ScriptedEvent,
        at_ms: u64,
    ) -> Result<LiveCallResult, SessionHostError> {
        self.require_open()?;
        if self.status == LiveStatus::Finished {
            return Err(self.error(scxml_scheduler::Violation::EventAfterFinish.into()));
        }
        if let Err(error) = scxml_types::EventName::new(event.name.clone()) {
            return Err(self.error(RunFailure::Harness {
                code: "event-name-invalid",
                message: error.to_string(),
            }));
        }
        self.record(InputRecord::Event {
            at_ms,
            event: event.clone(),
        });
        self.step(Some(Operation::HostWasmtimeSessionSend), at_ms, |engine| {
            if engine.drain(Horizon::Until(at_ms), &mut no_hook())? == DrainStop::Finished {
                return Ok(Err(scxml_scheduler::Violation::EventAfterFinish.into()));
            }
            engine.advance_clock_to(at_ms)?;
            if let Err(failure) = engine.input(event, &mut no_hook()) {
                // Only a rejection asks the root's status: any other
                // failure is the session's, and a trapped root's instance
                // cannot be entered again, so asking would report that
                // instead of the trap.
                let rejected = is_rejection(&failure)
                    && matches!(
                        engine.machine_status(),
                        Ok(Status::Running | Status::Finished)
                    );
                return if rejected {
                    Ok(Err(failure))
                } else {
                    Err(failure)
                };
            }
            engine.drain(Horizon::Until(at_ms), &mut no_hook())?;
            Ok(Ok(()))
        })
    }

    /// Fires the deadlines that are due. A timed session moves to its
    /// clock's time, firing every deadline at or before it; a logical
    /// session moves to its next deadline, firing it (and nothing
    /// otherwise). An advance that fires nothing only moves the clock.
    ///
    /// # Errors
    ///
    /// As [`Self::send`], failing the session.
    pub fn advance(&mut self) -> Result<LiveCallResult, SessionHostError> {
        self.require_open()?;
        let to_ms = match &self.clock {
            LiveClock::Timed(_) => self.time()?,
            LiveClock::Logical => match self.next_deadline()? {
                Some(deadline) => deadline,
                None => self.now_ms()?,
            },
        };
        self.advance_at(to_ms)
    }

    /// Moves a logical session's clock to `to_ms`, firing every deadline
    /// at or before it in deadline order.
    ///
    /// # Errors
    ///
    /// `clock-timed` for a timed session (its clock decides the time) and
    /// `clock-regression` for a time before the current one, both without
    /// failing the session; otherwise as [`Self::advance`].
    pub fn advance_to(&mut self, to_ms: u64) -> Result<LiveCallResult, SessionHostError> {
        self.require_logical()?;
        self.advance_at(to_ms)
    }

    fn require_logical(&self) -> Result<(), SessionHostError> {
        self.require_open()?;
        match self.clock {
            LiveClock::Logical => Ok(()),
            LiveClock::Timed(_) => Err(self.error(harness(
                "clock-timed",
                "a timed session's clock decides its time",
            ))),
        }
    }

    fn advance_at(&mut self, to_ms: u64) -> Result<LiveCallResult, SessionHostError> {
        self.require_open()?;
        let now = self.now_ms()?;
        if to_ms < now {
            return Err(self.error(scxml_scheduler::Violation::ClockRegression.into()));
        }
        self.record(InputRecord::Advance { to_ms });
        let due = self.status == LiveStatus::Running
            && self
                .next_deadline()?
                .is_some_and(|deadline| deadline <= to_ms);
        let operation = due.then_some(Operation::HostWasmtimeSessionAdvance);
        let finished = self.status == LiveStatus::Finished;
        self.step(operation, to_ms, |engine| {
            if finished {
                return Ok(Ok(()));
            }
            if engine.drain(Horizon::Until(to_ms), &mut no_hook())? != DrainStop::Finished {
                engine.advance_clock_to(to_ms)?;
            }
            Ok(Ok(()))
        })
    }

    /// Fires every pending deadline of a logical session in deadline order
    /// until the root finishes or nothing is scheduled: the batch run's
    /// drain.
    ///
    /// # Errors
    ///
    /// `clock-timed` for a timed session (without failing it); otherwise
    /// as [`Self::advance`].
    pub fn run_until_idle(&mut self) -> Result<LiveCallResult, SessionHostError> {
        self.require_logical()?;
        self.record(InputRecord::Idle);
        let finished = self.status == LiveStatus::Finished;
        let operation = (!finished).then_some(Operation::HostWasmtimeSessionAdvance);
        let at_ms = self.now_ms()?;
        let mut step = self.step(operation, at_ms, |engine| {
            if finished {
                return Ok(Ok(()));
            }
            // Deadline by deadline, so the clock never moves past an invoked
            // service's request still in flight: its response must arrive at
            // the time the session waits at (from the network, or from the
            // record of a replay).
            loop {
                let now = engine.now()?;
                if engine.drain(Horizon::Until(now), &mut no_hook())? == DrainStop::Finished
                    || engine.awaiting_http_responses()
                {
                    break;
                }
                let Some(deadline) = engine.next_deadline()? else {
                    break;
                };
                if engine.drain(Horizon::Until(deadline), &mut no_hook())? == DrainStop::Finished {
                    break;
                }
            }
            Ok(Ok(()))
        })?;
        step.at_ms = self.now_ms()?;
        Ok(step)
    }

    /// One read-only canonical `scxml.session-snapshot/v2` document of the
    /// stable session: the same document a batch run's checkpoint
    /// snapshot produces for the same state.
    ///
    /// # Errors
    ///
    /// [`RunFailure::Snapshot`] (without failing the session), or a host
    /// failure of an export call (failing it).
    pub fn snapshot(
        &mut self,
        options: &scxml_snapshot::Options,
    ) -> Result<Vec<u8>, SessionHostError> {
        self.require_open()?;
        self.record(InputRecord::Snapshot {
            options: options.clone(),
        });
        match self.engine.snapshot(options) {
            Ok(bytes) => Ok(bytes),
            Err(failure @ RunFailure::Snapshot(_)) => Err(self.error(failure)),
            Err(failure) => Err(self.fatal(failure)),
        }
    }

    /// Applies one recorded input to a logical session at its recorded
    /// time (see the module documentation, "Record and replay").
    ///
    /// # Errors
    ///
    /// `clock-timed` for a timed session, and whatever the recorded call
    /// answers.
    pub fn apply(&mut self, record: &InputRecord) -> Result<Applied, SessionHostError> {
        self.require_logical()?;
        match record {
            InputRecord::Event { at_ms, event } => self.send_at(event, *at_ms).map(Applied::Call),
            InputRecord::Advance { to_ms } => self.advance_at(*to_ms).map(Applied::Call),
            InputRecord::Idle => self.run_until_idle().map(Applied::Call),
            InputRecord::Snapshot { options } => self.snapshot(options).map(Applied::Snapshot),
            InputRecord::Http {
                at_ms,
                session_id,
                path,
                event,
                status,
            } => self.replay_http(*at_ms, session_id, path, event, *status),
            InputRecord::CommunicationErrors { at_ms, senders } => self
                .communication_errors_at(
                    *at_ms,
                    senders.iter().cloned().map(Owed::Recorded).collect(),
                )
                .map(Applied::Call),
            InputRecord::HttpResponse {
                at_ms,
                path,
                request,
                response,
            } => self
                .replay_http_response(
                    *at_ms,
                    &super::RecordedResponse {
                        path: path.clone(),
                        request: *request,
                        answer: crate::ExtensionHttpAnswer::Response(response.clone()),
                    },
                )
                .map(Applied::Call),
            InputRecord::HttpResponsePart {
                at_ms,
                path,
                request,
                part,
            } => self
                .replay_http_response(
                    *at_ms,
                    &super::RecordedResponse {
                        path: path.clone(),
                        request: *request,
                        answer: crate::ExtensionHttpAnswer::Part(part.clone()),
                    },
                )
                .map(Applied::Call),
            InputRecord::OutboundHttp { .. } => Ok(Applied::OutboundDecision),
            InputRecord::Start { .. } => Ok(Applied::Started),
        }
    }

    /// Replays one recorded inbound message: the same event at the same
    /// time to the session of the recorded lineage, and the component must
    /// choose the recorded status (a session of another lineage is not the
    /// one addressed, so the message reaches none and the replay diverges).
    fn replay_http(
        &mut self,
        at_ms: u64,
        session_id: &str,
        path: &[String],
        event: &scxml_types::Event,
        status: u16,
    ) -> Result<Applied, SessionHostError> {
        self.require_open()?;
        let (delivery, step) = self.receive_at(
            at_ms,
            session_id,
            Some(path),
            crate::data_model::wit_event(event),
        );
        let response = self
            .engine
            .http_respond(delivery)
            .map_err(|error| self.fatal(RunFailure::Host(error)))?;
        let step = step?;
        if response.status != status {
            return Err(self.fatal(replay_diverged(
                "the replayed inbound BasicHTTP message got another response status",
            )));
        }
        Ok(Applied::Received(step))
    }

    /// The root's `_ioprocessors` entries: each linked processor's type URI
    /// and aliases with the root's location, sorted by key.
    ///
    /// # Errors
    ///
    /// A processor component failure (the session fails).
    pub fn io_processors(&mut self) -> Result<BTreeMap<String, String>, SessionHostError> {
        self.require_open()?;
        self.engine
            .io_processors()
            .map_err(|failure| self.fatal(failure))
    }

    /// The root's active leaf states in document order.
    ///
    /// # Errors
    ///
    /// A machine call failure (the session fails).
    pub fn configuration(&mut self) -> Result<Vec<String>, SessionHostError> {
        self.require_open()?;
        self.engine
            .configuration()
            .map_err(|failure| self.fatal(failure))
    }

    /// Delivers one inbound `BasicHTTP` message (SCXML C.2) addressed to
    /// `…/sessions/{_sessionid}/basichttp` (origin-form or absolute
    /// target): the component decodes it into an external event, which is
    /// placed on that running session's external queue (the root or an
    /// invoked session at any depth) at the session's current time and
    /// processed to a stable configuration, as [`Self::send`] processes an event.
    ///
    /// Never panics and always yields a response (see [`HttpReceipt`]); a
    /// failed, finished, or absent target session enqueues nothing. A
    /// decoding failure of the receiver component fails the session. A
    /// recording session records a message that reached a running session
    /// ([`InputRecord::Http`]).
    pub fn receive_http(&mut self, request: &HttpRequest) -> HttpReceipt {
        let (delivery, step, received) = self.receive_http_delivery(request);
        let response = match delivery {
            Some(delivery) => self.engine.http_respond(delivery).unwrap_or_else(|error| {
                self.engine.shared().fault(RunFailure::Host(error));
                host_failure_response()
            }),
            None => host_failure_response(),
        };
        if let Some((at_ms, session_id, path, event)) = received {
            self.record(InputRecord::Http {
                at_ms,
                session_id,
                path,
                event,
                status: response.status,
            });
        }
        HttpReceipt {
            response,
            result: step,
        }
    }

    /// The component's response to a message a listener rejected before it
    /// could build a request (an over-long body, or a malformed head); no
    /// session is touched.
    ///
    /// # Errors
    ///
    /// A receiver component failure.
    pub fn http_rejection(
        &mut self,
        rejection: crate::basichttp_types::Rejection,
    ) -> Result<HttpResponse, SessionHostError> {
        self.engine
            .http_respond(Delivery::Rejected(rejection))
            .map_err(|error| self.error(RunFailure::Host(error)))
    }

    /// The delivery outcome of one inbound message (`None`: the host failed
    /// before the component could answer), its call result, and, when a recording
    /// session ran the call, what to record of it.
    fn receive_http_delivery(&mut self, request: &HttpRequest) -> ReceivedHttp {
        if let Err(error) = self.require_open() {
            return (Some(Delivery::UnknownSession), Err(error), None);
        }
        if self.status == LiveStatus::Finished {
            return (Some(Delivery::UnknownSession), Ok(None), None);
        }
        let Some(session_id) = inbound_session_id(&request.uri).map(str::to_owned) else {
            return (Some(Delivery::UnknownSession), Ok(None), None);
        };
        let Some(key) = self.engine.session_key(&session_id) else {
            return (Some(Delivery::UnknownSession), Ok(None), None);
        };
        let at_ms = match self.time() {
            Ok(at_ms) => at_ms,
            Err(error) => return (None, Err(error), None),
        };
        let trace_context = inbound_http_trace_context(&request.headers);
        let event = match self.engine.http_receive(request, trace_context) {
            Ok(Ok(event)) => event,
            Ok(Err(rejection)) => return (Some(Delivery::Rejected(rejection)), Ok(None), None),
            Err(error) => return (None, Err(self.fatal(RunFailure::Host(error))), None),
        };
        let recorded = if self.recorded.is_some() {
            // Only a recording session needs the session's invoke-id path.
            let path = self.engine.session_path(key).unwrap_or_default();
            match crate::session::native_event(event.clone()) {
                Ok(native) => Some((at_ms, session_id.clone(), path, native)),
                Err(_) => {
                    return (
                        None,
                        Err(self.fatal(harness(
                            "record-event-invalid",
                            "the decoded inbound event cannot be recorded",
                        ))),
                        None,
                    );
                }
            }
        } else {
            None
        };
        let (delivery, step) = self.receive_at(at_ms, &session_id, None, event);
        (Some(delivery), step, recorded)
    }

    /// Places one decoded inbound event on the external queue of the
    /// running session `session_id` at `at_ms` and runs to a stable configuration (as
    /// [`Self::send`] processes an event): the delivery outcome and the
    /// call result when the event was enqueued. With `lineage`, only a session
    /// whose invoke-id path is exactly that receives it.
    fn receive_at(
        &mut self,
        at_ms: u64,
        session_id: &str,
        lineage: Option<&[String]>,
        event: crate::bindings::stateforward::scxml::types::Event,
    ) -> (Delivery, Result<Option<LiveCallResult>, SessionHostError>) {
        let mut delivery = Delivery::UnknownSession;
        let step = self.step(
            Some(Operation::HostWasmtimeSessionReceiveHttp),
            at_ms,
            |engine| {
                if engine.drain(Horizon::Until(at_ms), &mut no_hook())? == DrainStop::Finished {
                    return Ok(Ok(()));
                }
                engine.advance_clock_to(at_ms)?;
                // The deadlines that fired first may have finished the target.
                let Some(key) = engine.session_key(session_id) else {
                    return Ok(Ok(()));
                };
                if lineage.is_some_and(|path| !engine.session_has_path(key, path)) {
                    return Ok(Ok(()));
                }
                delivery = if engine.push_external(key, event) {
                    Delivery::Enqueued
                } else {
                    Delivery::UnknownSession
                };
                if matches!(delivery, Delivery::Enqueued) {
                    engine.drain(Horizon::Until(at_ms), &mut no_hook())?;
                }
                Ok(Ok(()))
            },
        );
        let step = step.map(|step| matches!(delivery, Delivery::Enqueued).then_some(step));
        (delivery, step)
    }

    /// Whether an accepted outbound `BasicHTTP` request may still fail or
    /// an `error.communication` is owed: an embedder keeps its loop alive
    /// (and wakes on [`LiveHttp::on_communication_error`]) while it is.
    #[must_use]
    pub fn communication_pending(&self) -> bool {
        self.engine.shared().http.failures.pending()
    }

    /// Delivers every owed `error.communication` (SCXML C.2): for each
    /// `BasicHTTP` send whose accepted request later failed, one
    /// `error.communication` whose `_event.type` is `platform` and whose
    /// `_event.sendid` is that `<send>`'s id (SCXML 5.10.1: the platform
    /// raised it for a failed send; no data, as the browser host delivers
    /// it) on the sending session's internal event queue (SCXML 3.12.2:
    /// errors go "in the internal event queue"; 6.2.4: "If it is unable to
    /// dispatch the message, the Processor MUST place the error
    /// error.communication on the internal event queue"), when that session
    /// still runs, at the session's current time. Each error runs, at once,
    /// the macrostep that drains that internal queue without dequeuing an
    /// external event (the machine's `communication-error`; Appendix D
    /// mapping in `Engine::queue_communication_error`); then every session
    /// runs to a stable configuration. `Ok(None)` when nothing was owed or the session
    /// no longer runs (owed events are then dropped).
    ///
    /// # Errors
    ///
    /// As [`Self::advance`], failing the session.
    pub fn deliver_communication_errors(
        &mut self,
    ) -> Result<Option<LiveCallResult>, SessionHostError> {
        self.require_open()?;
        let owed = self.engine.shared().http.failures.take();
        if owed.is_empty() || self.status != LiveStatus::Running {
            return Ok(None);
        }
        let at_ms = self.time()?;
        self.communication_errors_at(at_ms, owed.into_iter().map(Owed::Live).collect())
            .map(Some)
    }

    /// Delivers one platform `error.communication` to each of `owed` that
    /// still runs, at `at_ms`, then runs every session to a stable configuration.
    fn communication_errors_at(
        &mut self,
        at_ms: u64,
        owed: Vec<Owed>,
    ) -> Result<LiveCallResult, SessionHostError> {
        self.require_open()?;
        if self.recorded.is_some() {
            // A sender no longer in the run is left out: replayed, it
            // would be skipped as it is now (keys and `_sessionid`s are
            // never reused).
            let senders = owed
                .iter()
                .filter_map(|owed| owed.sender(&self.engine))
                .collect();
            self.record(InputRecord::CommunicationErrors { at_ms, senders });
        }
        self.step(
            Some(Operation::HostWasmtimeSessionCommunicationError),
            at_ms,
            |engine| {
                if engine.drain(Horizon::Until(at_ms), &mut no_hook())? == DrainStop::Finished {
                    return Ok(Ok(()));
                }
                engine.advance_clock_to(at_ms)?;
                for owed in owed {
                    // Only the very session that sent, while it still runs.
                    if let Some(key) = owed.running_key(engine) {
                        engine.communication_error(key, owed.into_send_id())?;
                    }
                }
                engine.drain(Horizon::Until(at_ms), &mut no_hook())?;
                Ok(Ok(()))
            },
        )
    }

    /// Delivers the earliest answer to an invoked service's request that
    /// arrived and is not yet delivered (a `fetch`'s response, or one part
    /// of a `send`'s; see [`LiveHttp::on_http_response`]) as one external
    /// input: at the session's current time (a timed session's clock first,
    /// firing what it reached), the answer reaches the invocation that waits
    /// for it, whose event loop then runs until it waits again, and every
    /// session runs to a stable configuration. An answer of an invocation
    /// that was cancelled or ended meanwhile is dropped. A recording session
    /// records the delivered answer ([`InputRecord::HttpResponse`],
    /// [`InputRecord::HttpResponsePart`]), so each delivery is one call of
    /// the record and of its replay. Returns the delivery's call result, or
    /// `None` when nothing waits for delivery; the embedder calls again
    /// while it returns one.
    ///
    /// # Errors
    ///
    /// As [`Self::send`], failing the session.
    pub fn deliver_http_responses(&mut self) -> Result<Option<LiveCallResult>, SessionHostError> {
        self.require_open()?;
        if self.status != LiveStatus::Running {
            return Ok(None);
        }
        let Some(item) = self
            .engine
            .shared()
            .http
            .http_responses
            .as_ref()
            .and_then(|responses| responses.take_first())
        else {
            return Ok(None);
        };
        let at_ms = self.time()?;
        let mut delivered = Vec::new();
        let step = self.step(
            Some(Operation::HostWasmtimeSessionHttpResponse),
            at_ms,
            |engine| {
                if engine.drain(Horizon::Until(at_ms), &mut no_hook())? == DrainStop::Finished {
                    return Ok(Ok(()));
                }
                engine.advance_clock_to(at_ms)?;
                delivered = engine.deliver_http_responses(vec![item])?;
                engine.drain(Horizon::Until(at_ms), &mut no_hook())?;
                Ok(Ok(()))
            },
        );
        for response in delivered {
            self.record(match response.answer {
                crate::ExtensionHttpAnswer::Response(outcome) => InputRecord::HttpResponse {
                    at_ms,
                    path: response.path,
                    request: response.request,
                    response: outcome,
                },
                crate::ExtensionHttpAnswer::Part(part) => InputRecord::HttpResponsePart {
                    at_ms,
                    path: response.path,
                    request: response.request,
                    part,
                },
            });
        }
        Ok(Some(step?))
    }

    /// Replays one recorded HTTP response at its recorded time to the
    /// invoked service of the recorded lineage; a response no invocation of
    /// that lineage waits for diverges.
    fn replay_http_response(
        &mut self,
        at_ms: u64,
        response: &super::RecordedResponse,
    ) -> Result<LiveCallResult, SessionHostError> {
        self.require_open()?;
        let now = self.now_ms()?;
        if at_ms < now {
            return Err(self.error(scxml_scheduler::Violation::ClockRegression.into()));
        }
        self.step(
            Some(Operation::HostWasmtimeSessionHttpResponse),
            at_ms,
            |engine| {
                if engine.drain(Horizon::Until(at_ms), &mut no_hook())? == DrainStop::Finished {
                    return Ok(Err(replay_diverged(
                        "the recorded HTTP response arrived after the session finished",
                    )));
                }
                engine.advance_clock_to(at_ms)?;
                if !engine.deliver_recorded_response(response)? {
                    return Err(replay_diverged(
                        "no invoked service of the recorded lineage waits for the recorded HTTP response",
                    ));
                }
                engine.drain(Horizon::Until(at_ms), &mut no_hook())?;
                Ok(Ok(()))
            },
        )
    }

    /// Whether the session waits for the HTTP response of an invoked
    /// service's request: one in flight (or, in a replay, still to come from
    /// the record), or one arrived and not yet delivered
    /// ([`Self::deliver_http_responses`]). A logical embedder waits for the
    /// response before it moves the clock again.
    #[must_use]
    pub fn awaiting_http_responses(&self) -> bool {
        self.status == LiveStatus::Running && self.engine.awaiting_http_responses()
    }

    /// Takes every output streamed since the last call, in order.
    pub fn take_outputs(&mut self) -> Vec<SessionOutput> {
        self.engine.shared().take_outputs()
    }

    /// Takes every input recorded since the last call, in order (empty
    /// when the session does not record).
    pub fn take_recorded(&mut self) -> Vec<InputRecord> {
        self.recorded
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// Closes the session gracefully: a running root exits (its
    /// `<onexit>` handlers run and every invocation is cancelled), every
    /// pending delayed send is cancelled, and every session is dropped, so
    /// each session's extension components shut down exactly once.
    ///
    /// # Errors
    ///
    /// A failure of the root's exit. The session is shut down and dropped
    /// either way.
    pub fn close(mut self) -> Result<CloseReport, SessionHostError> {
        let span =
            scxml_telemetry::observe(Module::HostWasmtime, Operation::HostWasmtimeSessionClose);
        let status = self.status;
        self.engine.shared().begin_step();
        let result = (|| {
            if status == LiveStatus::Failed {
                return Ok((0, 0));
            }
            let pending = self.engine.pending_sends()?;
            let children = self.engine.child_count();
            self.engine.cancel_root()?;
            Ok((pending, children))
        })();
        let outputs = self.engine.shared().take_outputs();
        let commands = self.engine.shared().step_commands();
        match result {
            Ok((cancelled_sends, cancelled_children)) => {
                span.event(SignalEvent::TransformationCompleted);
                Ok(CloseReport {
                    status,
                    cancelled_sends,
                    cancelled_children,
                    outputs,
                })
            }
            Err(failure) => {
                span.fail(category(&failure));
                self.span.fail();
                Err(SessionHostError { failure, commands })
            }
        }
    }
}

impl super::LoadRejection {
    /// The stable reason code a live session reports for this rejection.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ScriptUnavailable => "script-unavailable",
            Self::ScriptInvalid => "script-invalid",
        }
    }
}
/// One owed `error.communication`: a live failure, known by its sending
/// session's key, or one a replayed record names by `_sessionid` and
/// invoke-id path.
enum Owed {
    Live(OwedFailure),
    Recorded(CommunicationSender),
}

impl Owed {
    /// The record entry: a live sender's path is computed here, once;
    /// `None` for a live sender no longer in the run.
    fn sender(&self, engine: &RunEngine) -> Option<CommunicationSender> {
        match self {
            Self::Live(owed) => Some(CommunicationSender {
                session_id: owed.session_id.clone(),
                path: engine.session_path(owed.key)?,
                send_id: owed.send_id.clone(),
            }),
            Self::Recorded(sender) => Some(sender.clone()),
        }
    }

    /// The sending session, while it still runs.
    fn running_key(&self, engine: &RunEngine) -> Option<scxml_scheduler::SessionKey> {
        match self {
            Self::Live(owed) => engine
                .session_key(&owed.session_id)
                .filter(|key| *key == owed.key),
            Self::Recorded(sender) => engine
                .session_key(&sender.session_id)
                .filter(|key| engine.session_has_path(*key, &sender.path)),
        }
    }

    fn into_send_id(self) -> String {
        match self {
            Self::Live(owed) => owed.send_id,
            Self::Recorded(sender) => sender.send_id,
        }
    }
}
