//! Pure, deterministic cooperative executor of one SCXML root session and
//! every session it invoked.
//!
//! The root session and the sessions it invokes (SCXML 6.4), at any nested invoke depth
//! (limited only by the embedder's [`InvokeLimits`]). Each session owns its machine, data model,
//! Event I/O Processor, and clock, an internal event queue (inside the
//! machine), and an external event queue ([`InvokedSessions`]); each runs the
//! SCXML 1.0 Appendix D event loop, one external event at a time. The
//! executor decides only *which* session processes its next external event
//! and when delayed sends come due on their one shared logical clock; it knows
//! nothing of SCXML documents, events, data models, or I/O processors.
//! Callers supply sessions through the [`Sessions`] trait, whose associated
//! types stay opaque here.
//!
//! It is the one execution order of the Wasmtime session host, for batch
//! runs (the W3C IRP baselines reproduce through it) and live sessions
//! alike:
//!
//! * [`Scheduler::run_scripted`] runs the root with scripted inputs;
//! * [`Scheduler::run_external_client`] runs the root as the receiver of a
//!   harness-as-external-client post;
//! * [`Scheduler::drain`] runs every session to a stable configuration over the shared
//!   logical clock;
//! * [`Scheduler::run_scripted_with`] and
//!   [`Scheduler::run_external_client_with`] run the same schedules and call
//!   a caller hook at every stable [`Checkpoint`] (read-only session
//!   snapshots are taken there and nowhere else);
//! * the stepwise calls those runs are made of ([`Scheduler::start`],
//!   [`Scheduler::input`], [`Scheduler::deliver`],
//!   [`Scheduler::drain_until`] with a [`Horizon`],
//!   [`Scheduler::advance_clock_to`], and [`Scheduler::end`]) drive a
//!   long-lived session one call at a time: the host's live session
//!   delivers each event, then drains only the deadlines its wall clock has
//!   reached, and waits for the [`Scheduler::next_deadline`] in between.
//!
//! # Example
//!
//! A root session without invoked sessions: `start` schedules a delayed
//! `timeout` at 100 ms, the scripted input `ping` is delivered next, and the
//! drain advances the logical clock to the deadline and delivers `timeout`,
//! which finishes the root.
//!
//! ```
//! use scxml_scheduler::{
//!     Delivery, DueRoute, InvokedSessions, Scheduler, SessionKey, Sessions, Violation,
//! };
//!
//! /// One root session that records what it processed.
//! #[derive(Default)]
//! struct Root {
//!     now: u64,
//!     pending: Vec<(u64, String)>,
//!     processed: Vec<String>,
//!     finished: bool,
//! }
//!
//! type Invocations = InvokedSessions<String, String, String>;
//!
//! impl Root {
//!     fn process(&mut self, event: &str) {
//!         self.processed.push(event.to_owned());
//!         match event {
//!             "start" => self.pending.push((self.now + 100, "timeout".to_owned())),
//!             "timeout" => self.finished = true,
//!             _ => {}
//!         }
//!     }
//! }
//!
//! impl Sessions for Root {
//!     type Id = String;
//!     type Event = String;
//!     type Value = String;
//!     type Due = String;
//!     type Input = String;
//!     type SendId = String;
//!     type Error = Violation;
//!
//!     fn require_budget(&self) -> Result<(), Violation> {
//!         Ok(())
//!     }
//!     fn start_root(&mut self, _: &mut Invocations) -> Result<(), Violation> {
//!         self.process("start");
//!         Ok(())
//!     }
//!     fn send_input(&mut self, _: &mut Invocations, input: &String) -> Result<(), Violation> {
//!         self.process(input);
//!         Ok(())
//!     }
//!     fn deliver(
//!         &mut self,
//!         _: &mut Invocations,
//!         _: SessionKey,
//!         delivery: Delivery<String, String, String>,
//!     ) -> Result<(), Violation> {
//!         if let Delivery::External(event) = delivery {
//!             self.process(&event);
//!         }
//!         Ok(())
//!     }
//!     fn communication_error(
//!         &mut self,
//!         _: &mut Invocations,
//!         _: SessionKey,
//!         send_id: String,
//!     ) -> Result<(), Violation> {
//!         self.process(&format!("error.communication:{send_id}"));
//!         Ok(())
//!     }
//!     fn finished(&self, _: SessionKey) -> bool {
//!         self.finished
//!     }
//!     fn take_return_data(&mut self, _: SessionKey) -> Option<String> {
//!         None
//!     }
//!     fn next_deadline(&mut self, _: SessionKey) -> Result<Option<u64>, Violation> {
//!         Ok(self.pending.iter().map(|(deadline, _)| *deadline).min())
//!     }
//!     fn now(&mut self) -> Result<u64, Violation> {
//!         Ok(self.now)
//!     }
//!     fn advance_clock(&mut self, delta: u64) -> Result<(), Violation> {
//!         self.now += delta;
//!         Ok(())
//!     }
//!     fn drain_due(&mut self, _: SessionKey) -> Result<Vec<String>, Violation> {
//!         let now = self.now;
//!         let (due, later) = self.pending.drain(..).partition(|(deadline, _)| *deadline <= now);
//!         self.pending = later;
//!         Ok(due.into_iter().map(|(_, event)| event).collect())
//!     }
//!     fn route_due(
//!         &mut self,
//!         _: &mut Invocations,
//!         _: SessionKey,
//!         due: String,
//!     ) -> Result<DueRoute<String, String, String>, Violation> {
//!         Ok(DueRoute::Local(due))
//!     }
//! }
//!
//! let mut root = Root::default();
//! Scheduler::new().run_scripted(&mut root, &["ping".to_owned()])?;
//! assert_eq!(root.processed, ["start", "ping", "timeout"]);
//! assert_eq!(root.now, 100);
//! assert!(root.finished);
//!
//! // A scripted input left after the root finished is a typed violation.
//! let mut root = Root::default();
//! root.finished = true;
//! assert_eq!(
//!     Scheduler::new().run_scripted(&mut root, &["late".to_owned()]),
//!     Err(Violation::EventAfterFinish)
//! );
//! # Ok::<(), Violation>(())
//! ```
//!
//! # Contract
//!
//! * Pure: no I/O, clocks, randomness, environment, threads, or telemetry.
//!   Time is only the logical ms values callers pass in ([`Horizon`],
//!   [`Scheduler::advance_clock_to`]) or sessions report.
//!   The executor runs inside its caller's boundary span (the session
//!   host's run or live-session call), whose typed failure reports every
//!   executor failure; it adds no span of its own (AGENTS.md boundary-only
//!   telemetry), so it has no telemetry cardinality surface.
//! * Deterministic: identical session behavior yields identical call order.
//!   Sessions are visited in invocation order ([`InvokedSessions`]).
//! * Depth-independent: sessions are addressed by [`SessionKey`], every
//!   walk of the invoke tree is iterative, and one drain iteration reads
//!   only the sessions a command ran on since the last one (their status
//!   and deadline) plus indexes of queued sessions and deadlines; the
//!   shared clock advances once per iteration for the whole run
//!   ([`Sessions::advance_clock`]). So the executor's work and stack do
//!   not grow with the number of sessions or their nesting depth.
//! * Bounded only where the embedder chooses: the invoked sessions stay
//!   within the embedder's [`InvokeLimits`] (none by default), and queues
//!   hold what the sessions send. Loop progress is bounded by the
//!   caller's command budget, checked through [`Sessions::require_budget`] at
//!   exactly two places: once per drain iteration and once before each due
//!   delayed send. Checkpoint ordinals are `u32`; a run that would pass
//!   `u32::MAX` of them fails with [`Violation::CheckpointLimit`].
//! * Checkpoints: a [`CheckpointHook`] is called exactly at the stable
//!   points [`CheckpointKind`] names, never between `drain_due` and the
//!   delivery of what it returned. It receives the sessions and a shared
//!   borrow of the sessions; the executor's calls to [`Sessions`] are the same,
//!   in the same order, with and without a hook.
//! * Fail-closed: executor invariant breaches are typed [`Violation`]s with
//!   stable reason codes, converted into the caller's error type.
//!   Reachable paths never panic.
#![forbid(unsafe_code)]

mod invoked_sessions;
mod pre_order;
mod work;

#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::fmt;

pub use invoked_sessions::{Delivery, InvokeLimits, Invoked, InvokedSessions, SessionKey};

/// What the executor read of one session after a command ran on it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Reading {
    /// It reached a top-level final state.
    Finished,
    /// It runs, with this earliest pending deadline.
    Running(Option<u64>),
}

/// An executor invariant breach. Each variant carries a stable reason code
/// ([`Violation::code`]) and message ([`Violation::message`]); callers map
/// it into their harness failure category through `From<Violation>`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Violation {
    /// A scripted input remained after the root session finished.
    EventAfterFinish,
    /// A delayed-send deadline preceded the shared logical clock.
    SendClockInvariant,
    /// Registering one more session would exceed the invoke limits.
    InvokeLimit,
    /// A call addressed a session the executor does not hold: a caller defect.
    SessionMissing,
    /// The run reached more checkpoints than a `u32` ordinal counts.
    CheckpointLimit,
    /// A caller asked to move the shared logical clock backwards.
    ClockRegression,
    /// A session registered an invoke id one of its live invocations has.
    DuplicateInvocation,
}

impl Violation {
    /// Stable, bounded reason code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::EventAfterFinish => "event-after-finish",
            Self::SendClockInvariant => "send-clock-invariant",
            Self::InvokeLimit => "invoke-limit-exceeded",
            Self::SessionMissing => "session-missing",
            Self::CheckpointLimit => "checkpoint-limit",
            Self::ClockRegression => "clock-regression",
            Self::DuplicateInvocation => "duplicate-invocation",
        }
    }

    /// Static human-readable description.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::EventAfterFinish => "scripted event remained after the machine finished",
            Self::SendClockInvariant => "a delayed-send deadline preceded the shared virtual clock",
            Self::InvokeLimit => "the invoked sessions exceeded its configured bounds",
            Self::SessionMissing => "a call addressed a session the invoked_sessions does not hold",
            Self::CheckpointLimit => "the run exceeded the checkpoint ordinal range",
            Self::ClockRegression => "the shared logical clock cannot move backwards",
            Self::DuplicateInvocation => "a session registered an invoke id that is already live",
        }
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for Violation {}

/// Delivery-time route of one due delayed send, as the sender's Event I/O
/// Processor classifies it against the sessions currently invoked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DueRoute<K, E, S> {
    /// The sender's own external queue: processed at once.
    Local(E),
    /// The sender's invoking session (`#_parent`).
    Parent(E),
    /// The session the sender invoked as `K`.
    Invocation(K, E),
    /// Nobody answers the target any more (e.g. an invocation that
    /// finished or was cancelled), so the send cannot be dispatched: the
    /// sender raises the `error.communication` of the send `S` on its
    /// internal queue at once ([`Sessions::communication_error`]; SCXML
    /// 3.12.2, 6.2.4, C.1).
    Undeliverable(S),
    /// The sender's Event I/O Processor dispatched the send itself when it
    /// fell due (a `BasicHTTP` POST, SCXML C.2), delivering whatever it
    /// delivered through the sessions it was given: nothing is left to
    /// route.
    Dispatched,
}

/// A root session and the sessions it invoked, as the executor sees them. A
/// session is addressed by its [`SessionKey`]; the root is
/// [`SessionKey::ROOT`].
///
/// Every command method runs one engine command to a stable configuration on one
/// session and charges the caller's command budget itself. Commands receive the
/// sessions so their effects can push onto other sessions' external queues
/// ([`InvokedSessions::push_external`], [`InvokedSessions::push_to_parent`]) and
/// register or remove the sessions they invoke ([`InvokedSessions::register`],
/// [`InvokedSessions::begin_cancel`], [`InvokedSessions::remove`]).
///
/// # Caller contract
///
/// * `finished` reports whether a present session reached a top-level final
///   state (for the root: its latest completed command's status). It does
///   not change except through a command.
/// * `finished` and `next_deadline` of a session change only through a
///   command on it or a `drain_due` of it. The executor touches the session
///   of each of its own calls; a command that runs commands on other
///   sessions (starting or cancelling the sessions it invokes) touches
///   them ([`InvokedSessions::touch`]).
/// * A session is present in [`InvokedSessions`] exactly while the implementation
///   holds it.
/// * `drain_due` returns a bounded batch in (deadline, schedule) order.
pub trait Sessions {
    /// One invoke id: an invoking session's name for a session it invoked.
    type Id: Ord + Clone;
    /// An event delivered to a session.
    type Event;
    /// A session's return data (`<donedata>` payload).
    type Value;
    /// One due delayed send, routed at delivery time.
    type Due;
    /// One scripted root input, validated when it is delivered.
    type Input;
    /// The id of a failed `<send>`, carried by its `error.communication`.
    type SendId;
    /// The caller's failure type; executor violations convert into it.
    type Error: From<Violation>;

    /// Fails closed when the shared command budget is exhausted.
    ///
    /// # Errors
    ///
    /// The caller's budget failure.
    fn require_budget(&self) -> Result<(), Self::Error>;

    /// Runs the root's `Start` command.
    ///
    /// # Errors
    ///
    /// Any command failure; it aborts the run.
    fn start_root(
        &mut self,
        invoked_sessions: &mut InvokedSessions<Self::Id, Self::Event, Self::Value>,
    ) -> Result<(), Self::Error>;

    /// Validates one scripted input and delivers it to the root as an
    /// external event.
    ///
    /// # Errors
    ///
    /// Invalid input or any command failure; it aborts the run.
    fn send_input(
        &mut self,
        invoked_sessions: &mut InvokedSessions<Self::Id, Self::Event, Self::Value>,
        input: &Self::Input,
    ) -> Result<(), Self::Error>;

    /// Processes one entry of a session's external queue: an external event,
    /// or a message from a session it invoked (`Done` becomes
    /// `done.invoke.<id>`).
    ///
    /// # Errors
    ///
    /// Any command failure; it aborts the run.
    fn deliver(
        &mut self,
        invoked_sessions: &mut InvokedSessions<Self::Id, Self::Event, Self::Value>,
        session: SessionKey,
        delivery: Delivery<Self::Id, Self::Event, Self::Value>,
    ) -> Result<(), Self::Error>;

    /// Places the asynchronous `error.communication` of the session's
    /// `<send>` `send_id` on its internal event queue and runs the
    /// macrostep that processes it, dequeuing no external event (SCXML
    /// 3.12.2: errors go "in the internal event queue"; 6.2.4: "If it is
    /// unable to dispatch the message, the Processor MUST place the error
    /// error.communication on the internal event queue").
    ///
    /// # Errors
    ///
    /// Any command failure; it aborts the run.
    fn communication_error(
        &mut self,
        invoked_sessions: &mut InvokedSessions<Self::Id, Self::Event, Self::Value>,
        session: SessionKey,
        send_id: Self::SendId,
    ) -> Result<(), Self::Error>;

    /// Whether the session `session` finished.
    fn finished(&self, session: SessionKey) -> bool;

    /// Takes a finished invoked session's return data, if any.
    fn take_return_data(&mut self, session: SessionKey) -> Option<Self::Value>;

    /// A session's earliest pending delayed-send deadline, in logical ms.
    /// Takes `&mut self` because a clock component query runs guest code.
    ///
    /// # Errors
    ///
    /// The caller's clock failure; it aborts the run.
    fn next_deadline(&mut self, session: SessionKey) -> Result<Option<u64>, Self::Error>;

    /// The root session's logical clock, in ms (the shared clock).
    ///
    /// # Errors
    ///
    /// The caller's clock failure; it aborts the run.
    fn now(&mut self) -> Result<u64, Self::Error>;

    /// Advances the shared logical clock of every session by `delta` ms
    /// (> 0). An implementation may move each session's own clock lazily,
    /// before its next use; every session reads the shared time.
    ///
    /// # Errors
    ///
    /// The caller's clock failure; it aborts the run.
    fn advance_clock(&mut self, delta: u64) -> Result<(), Self::Error>;

    /// Removes a session's due delayed sends in (deadline, schedule) order.
    ///
    /// # Errors
    ///
    /// The caller's drain failure; it aborts the run.
    fn drain_due(&mut self, session: SessionKey) -> Result<Vec<Self::Due>, Self::Error>;

    /// Classifies one due send of `session` at delivery time, or dispatches
    /// it through its Event I/O Processor ([`DueRoute::Dispatched`]); a
    /// dispatch may push onto other sessions' external queues.
    ///
    /// # Errors
    ///
    /// The caller's routing failure; it aborts the run.
    fn route_due(
        &mut self,
        invoked_sessions: &mut InvokedSessions<Self::Id, Self::Event, Self::Value>,
        session: SessionKey,
        due: Self::Due,
    ) -> Result<SessionsDueRoute<Self>, Self::Error>;
}

/// The [`DueRoute`] type of a [`Sessions`] implementation.
pub type SessionsDueRoute<S> =
    DueRoute<<S as Sessions>::Id, <S as Sessions>::Event, <S as Sessions>::SendId>;

/// The [`InvokedSessions`] type of a [`Sessions`] implementation.
pub type SessionsInvocations<S> =
    InvokedSessions<<S as Sessions>::Id, <S as Sessions>::Event, <S as Sessions>::Value>;

/// Where a stable [`Checkpoint`] falls in a run. Ordinals number every
/// checkpoint of one run in the order reached, from 0.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckpointKind {
    /// After the root's `start` (always ordinal 0).
    Start,
    /// After one scripted input, or one external-client arrival, was
    /// delivered to the root.
    Input,
    /// At the top of one drain iteration that runs: after the root-finished
    /// check, before budget, `done.invoke`, or any delivery. Each iteration
    /// delivers one queued external event or one batch of due delayed sends.
    Drain,
    /// After the drain returned: the last checkpoint of a successful run.
    End,
}

/// One stable checkpoint: every session's last command returned
/// stable, no scheduling operation is in progress, and no session has
/// failed (a failure ends the run before the next checkpoint).
#[derive(Debug)]
pub struct Checkpoint<'a, E> {
    /// Ordinal of this checkpoint in the run (0 after `start`).
    pub ordinal: u32,
    /// Where it falls.
    pub kind: CheckpointKind,
    /// During an external-client run's delivery phase, the arrivals not yet
    /// delivered to the root (its host-held external queue); `None`
    /// everywhere else.
    pub arrivals: Option<&'a VecDeque<E>>,
}

/// A checkpoint callback. It may call the sessions (a read-only snapshot
/// runs guest code) and read the sessions' queues; an error ends the run.
pub trait CheckpointHook<S: Sessions>:
    FnMut(&mut S, &SessionsInvocations<S>, Checkpoint<'_, S::Event>) -> Result<(), S::Error>
{
}

impl<S: Sessions, H> CheckpointHook<S> for H where
    H: FnMut(&mut S, &SessionsInvocations<S>, Checkpoint<'_, S::Event>) -> Result<(), S::Error>
{
}

/// The hook of runs without checkpoints: does nothing.
fn no_checkpoint<S: Sessions>() -> impl CheckpointHook<S> {
    |_: &mut S, _: &SessionsInvocations<S>, _: Checkpoint<'_, S::Event>| Ok(())
}

/// How far one [`Scheduler::drain_until`] may move the shared logical
/// clock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Horizon {
    /// Advance through every pending deadline (the deterministic logical
    /// clock of a batch run).
    Unbounded,
    /// Deliver only deadlines at or before this logical time, in ms; a
    /// later deadline stops the drain with [`DrainStop::Waiting`].
    Until(u64),
}

/// Where one [`Scheduler::drain_until`] stopped. Every session is
/// stable in each case.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DrainStop {
    /// The root session finished.
    Finished,
    /// No session has a queued event or a pending deadline.
    Idle,
    /// The earliest pending deadline lies beyond the horizon.
    Waiting {
        /// That deadline, in logical ms.
        deadline: u64,
    },
}

/// The cooperative executor of one root session and every session it
/// invoked. It owns their [`InvokedSessions`] and
/// numbers the run's checkpoints across calls.
///
/// A run is a sequence of stepwise calls: [`Self::start`] once, then any
/// interleaving of [`Self::input`], [`Self::deliver`],
/// [`Self::drain_until`], and [`Self::advance_clock_to`], and finally
/// [`Self::end`]. [`Self::run_scripted_with`] and
/// [`Self::run_external_client_with`] are exactly such sequences.
pub struct Scheduler<S: Sessions> {
    invoked_sessions: SessionsInvocations<S>,
    /// The next checkpoint ordinal; `None` once `u32::MAX` was used.
    next_checkpoint: Option<u32>,
}

impl<S: Sessions> Default for Scheduler<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: Sessions> fmt::Debug for Scheduler<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Scheduler")
            .field("sessions", &self.invoked_sessions.len())
            .field("next_checkpoint", &self.next_checkpoint)
            .finish()
    }
}

impl<S: Sessions> Scheduler<S> {
    /// An executor holding only the root session, with the default
    /// [`InvokeLimits`]; its next checkpoint is ordinal 0.
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(InvokeLimits::default())
    }

    /// An executor under the embedder's `limits`.
    #[must_use]
    pub fn with_limits(limits: InvokeLimits) -> Self {
        Self {
            invoked_sessions: InvokedSessions::new(limits),
            next_checkpoint: Some(0),
        }
    }

    /// The root session, its invoked sessions, and their external queues.
    #[must_use]
    pub fn invoked_sessions(&self) -> &SessionsInvocations<S> {
        &self.invoked_sessions
    }

    /// The root session, its invoked sessions, and their external queues, mutably: for a command outside the executor's own calls
    /// (a live session's close runs the root's exit through it).
    pub fn invoked_sessions_mut(&mut self) -> &mut SessionsInvocations<S> {
        &mut self.invoked_sessions
    }

    /// Numbers one checkpoint and calls `hook` at it.
    fn reach(
        &mut self,
        sessions: &mut S,
        hook: &mut impl CheckpointHook<S>,
        kind: CheckpointKind,
        arrivals: Option<&VecDeque<S::Event>>,
    ) -> Result<(), S::Error> {
        let ordinal = self.next_checkpoint.ok_or(Violation::CheckpointLimit)?;
        self.next_checkpoint = ordinal.checked_add(1);
        hook(
            sessions,
            &self.invoked_sessions,
            Checkpoint {
                ordinal,
                kind,
                arrivals,
            },
        )
    }

    /// Runs the root's `start`, then reaches the [`CheckpointKind::Start`]
    /// checkpoint.
    ///
    /// Precondition: called once, first; the caller already validated its
    /// budget and loaded the root session.
    ///
    /// # Errors
    ///
    /// Any session or hook failure, or [`Violation`].
    pub fn start(
        &mut self,
        sessions: &mut S,
        hook: &mut impl CheckpointHook<S>,
    ) -> Result<(), S::Error> {
        self.invoked_sessions.touch(SessionKey::ROOT);
        sessions.start_root(&mut self.invoked_sessions)?;
        self.reach(sessions, hook, CheckpointKind::Start, None)
    }

    /// Delivers one scripted input to the root, then reaches an
    /// [`CheckpointKind::Input`] checkpoint. An input after the root
    /// finished fails with [`Violation::EventAfterFinish`] before it is
    /// validated. Nothing is drained.
    ///
    /// # Errors
    ///
    /// Any session or hook failure, or [`Violation`].
    pub fn input(
        &mut self,
        sessions: &mut S,
        input: &S::Input,
        hook: &mut impl CheckpointHook<S>,
    ) -> Result<(), S::Error> {
        if sessions.finished(SessionKey::ROOT) {
            return Err(Violation::EventAfterFinish.into());
        }
        self.invoked_sessions.touch(SessionKey::ROOT);
        sessions.send_input(&mut self.invoked_sessions, input)?;
        self.reach(sessions, hook, CheckpointKind::Input, None)
    }

    /// Delivers one external-client arrival to the root, then reaches an
    /// [`CheckpointKind::Input`] checkpoint carrying the arrivals still
    /// queued.
    ///
    /// # Errors
    ///
    /// Any session or hook failure, or [`Violation`].
    pub fn deliver(
        &mut self,
        sessions: &mut S,
        arrival: S::Event,
        arrivals: &VecDeque<S::Event>,
        hook: &mut impl CheckpointHook<S>,
    ) -> Result<(), S::Error> {
        self.invoked_sessions.touch(SessionKey::ROOT);
        sessions.deliver(
            &mut self.invoked_sessions,
            SessionKey::ROOT,
            Delivery::External(arrival),
        )?;
        self.reach(sessions, hook, CheckpointKind::Input, Some(arrivals))
    }

    /// Raises the asynchronous `error.communication` of the `<send>`
    /// `send_id` in the session at `path` at once
    /// ([`Sessions::communication_error`]): a failure the caller learned of
    /// after that `<send>` completed, such as a `BasicHTTP` request that
    /// failed after it was accepted. The caller checks that the session is
    /// still present.
    ///
    /// # Errors
    ///
    /// Any session failure.
    pub fn communication_error(
        &mut self,
        sessions: &mut S,
        session: SessionKey,
        send_id: S::SendId,
    ) -> Result<(), S::Error> {
        self.invoked_sessions.touch(session);
        sessions.communication_error(&mut self.invoked_sessions, session, send_id)
    }

    /// Reaches the [`CheckpointKind::End`] checkpoint: the last of a run.
    ///
    /// # Errors
    ///
    /// Any hook failure, or [`Violation::CheckpointLimit`].
    pub fn end(
        &mut self,
        sessions: &mut S,
        hook: &mut impl CheckpointHook<S>,
    ) -> Result<(), S::Error> {
        self.reach(sessions, hook, CheckpointKind::End, None)
    }

    /// Scripted run: starts the root, delivers each input in order, then
    /// [`Self::drain`]s. An input left after the root finished fails with
    /// [`Violation::EventAfterFinish`] before it is validated.
    ///
    /// Precondition: the caller already validated its budget and loaded the
    /// root session.
    ///
    /// # Errors
    ///
    /// Any session failure or [`Violation`].
    pub fn run_scripted(&mut self, sessions: &mut S, inputs: &[S::Input]) -> Result<(), S::Error> {
        self.run_scripted_with(sessions, inputs, no_checkpoint::<S>())
    }

    /// [`Self::run_scripted`], calling `hook` at every checkpoint: 0 after
    /// `start`, one after each input, one at the top of each drain
    /// iteration that runs, and one after the drain returned. The calls to
    /// `sessions` are exactly those of [`Self::run_scripted`]. It is
    /// [`Self::start`], [`Self::input`] per input,
    /// [`Self::drain_until`] with [`Horizon::Unbounded`], and [`Self::end`].
    ///
    /// # Errors
    ///
    /// Any session failure, hook failure, or [`Violation`]; a failure ends
    /// the run at once.
    pub fn run_scripted_with(
        &mut self,
        sessions: &mut S,
        inputs: &[S::Input],
        mut hook: impl CheckpointHook<S>,
    ) -> Result<(), S::Error> {
        self.start(sessions, &mut hook)?;
        for input in inputs {
            self.input(sessions, input, &mut hook)?;
        }
        self.drain_until(sessions, Horizon::Unbounded, &mut hook)?;
        self.end(sessions, &mut hook)
    }

    /// External-client run: starts the root, lets `client` post into an
    /// arrival queue, delivers
    /// arrivals in order until the root finishes (later arrivals are
    /// discarded), then [`Self::drain`]s. Returns the client's result.
    ///
    /// # Errors
    ///
    /// Any failure `client` returns, any session failure, or [`Violation`].
    pub fn run_external_client<R>(
        &mut self,
        sessions: &mut S,
        client: impl FnOnce(&mut S, &mut VecDeque<S::Event>) -> Result<R, S::Error>,
    ) -> Result<R, S::Error> {
        self.run_external_client_with(sessions, client, no_checkpoint::<S>())
    }

    /// [`Self::run_external_client`], calling `hook` at every checkpoint: 0
    /// after `start` (before `client` posts), one after each delivered
    /// arrival (carrying the arrivals still queued), one at the top of each
    /// drain iteration that runs, and one after the drain returned. It is
    /// [`Self::start`], [`Self::external_client`], [`Self::drain_until`]
    /// with [`Horizon::Unbounded`], and [`Self::end`].
    ///
    /// # Errors
    ///
    /// Any failure `client` or `hook` returns, any session failure, or
    /// [`Violation`]; a failure ends the run at once.
    pub fn run_external_client_with<R>(
        &mut self,
        sessions: &mut S,
        client: impl FnOnce(&mut S, &mut VecDeque<S::Event>) -> Result<R, S::Error>,
        mut hook: impl CheckpointHook<S>,
    ) -> Result<R, S::Error> {
        self.start(sessions, &mut hook)?;
        let response = self.external_client(sessions, client, &mut hook)?;
        self.drain_until(sessions, Horizon::Unbounded, &mut hook)?;
        self.end(sessions, &mut hook)?;
        Ok(response)
    }

    /// The external-client phase after `start`: lets `client` post into an
    /// arrival queue, then delivers
    /// arrivals in order ([`Self::deliver`]) until the root finishes (later
    /// arrivals are discarded). Returns the client's result.
    ///
    /// # Errors
    ///
    /// Any failure `client` or `hook` returns, any session failure, or
    /// [`Violation`].
    pub fn external_client<R>(
        &mut self,
        sessions: &mut S,
        client: impl FnOnce(&mut S, &mut VecDeque<S::Event>) -> Result<R, S::Error>,
        hook: &mut impl CheckpointHook<S>,
    ) -> Result<R, S::Error> {
        let mut arrivals = VecDeque::new();
        let response = client(sessions, &mut arrivals)?;
        while let Some(arrival) = arrivals.pop_front() {
            if sessions.finished(SessionKey::ROOT) {
                break;
            }
            self.deliver(sessions, arrival, &arrivals, hook)?;
        }
        Ok(response)
    }

    /// Runs every session until the root finishes or no session has
    /// runnable work: [`Self::drain_until`] with [`Horizon::Unbounded`] and
    /// no checkpoint hook.
    ///
    /// # Errors
    ///
    /// Any session failure or [`Violation`].
    pub fn drain(&mut self, sessions: &mut S) -> Result<(), S::Error> {
        self.drain_until(sessions, Horizon::Unbounded, &mut no_checkpoint::<S>())
            .map(|_| ())
    }

    /// Runs every session until the root finishes, no session has runnable
    /// work, or the earliest deadline lies beyond `horizon`. Each
    /// iteration, in order:
    ///
    /// 1. stop ([`DrainStop::Finished`]) if the root finished;
    /// 2. reach a [`CheckpointKind::Drain`] checkpoint, then require
    ///    budget;
    /// 3. queue `done.invoke` on the invoking session for each invoked
    ///    session that finished since (invocation order), after every event it sent
    ///    (official case 236);
    /// 4. deliver the oldest queued entry of the first unfinished session,
    ///    in invocation order, whose external queue holds one, then restart;
    /// 5. take the earliest deadline across the unfinished sessions (none:
    ///    stop with [`DrainStop::Idle`]; beyond `horizon`: stop with
    ///    [`DrainStop::Waiting`]), fail with
    ///    [`Violation::SendClockInvariant`] when it precedes the root clock,
    ///    and advance every session by the difference;
    /// 6. drain the root's due sends, requiring budget before each: local
    ///    sends are processed at once (stop once the root finishes), sends
    ///    to an invoked session are queued on it, unreachable ones drop
    ///    (case 187);
    /// 7. for each other session, in invocation order, still present and
    ///    unfinished at that point whose next deadline equals the earliest,
    ///    drain its due sends, requiring budget before each: `#_parent`
    ///    sends are queued on the invoking session, sends to an invoked
    ///    session are queued on it, unreachable ones drop, and local ones
    ///    are processed at once (the whole batch, even if the session
    ///    finishes part-way).
    ///
    /// # Errors
    ///
    /// Any session or hook failure, or [`Violation`].
    pub fn drain_until(
        &mut self,
        sessions: &mut S,
        horizon: Horizon,
        hook: &mut impl CheckpointHook<S>,
    ) -> Result<DrainStop, S::Error> {
        loop {
            if sessions.finished(SessionKey::ROOT) {
                return Ok(DrainStop::Finished);
            }
            self.reach(sessions, hook, CheckpointKind::Drain, None)?;
            sessions.require_budget()?;
            self.refresh(sessions)?;
            for key in self.invoked_sessions.take_done_due() {
                let return_data = sessions.take_return_data(key);
                self.invoked_sessions
                    .push_to_parent(key, Invoked::Done(return_data));
            }
            if let Some((key, delivery)) = self
                .invoked_sessions
                .pop_next(|key| !sessions.finished(key))
            {
                self.invoked_sessions.touch(key);
                sessions.deliver(&mut self.invoked_sessions, key, delivery)?;
                continue;
            }
            let Some(earliest) = self.invoked_sessions.earliest_deadline() else {
                return Ok(DrainStop::Idle);
            };
            if let Horizon::Until(limit) = horizon
                && earliest > limit
            {
                return Ok(DrainStop::Waiting { deadline: earliest });
            }
            let now = sessions.now()?;
            if earliest < now {
                return Err(Violation::SendClockInvariant.into());
            }
            let delta = earliest.saturating_sub(now);
            if delta > 0 {
                sessions.advance_clock(delta)?;
            }
            self.invoked_sessions.touch(SessionKey::ROOT);
            for due in sessions.drain_due(SessionKey::ROOT)? {
                sessions.require_budget()?;
                match sessions.route_due(&mut self.invoked_sessions, SessionKey::ROOT, due)? {
                    DueRoute::Invocation(id, event) => {
                        if let Some(child) = self.invoked_sessions.child(SessionKey::ROOT, &id) {
                            self.invoked_sessions.push_external(child, event);
                        }
                        continue;
                    }
                    DueRoute::Parent(_) | DueRoute::Dispatched => continue,
                    DueRoute::Local(event) => {
                        sessions.deliver(
                            &mut self.invoked_sessions,
                            SessionKey::ROOT,
                            Delivery::External(event),
                        )?;
                    }
                    DueRoute::Undeliverable(send_id) => {
                        sessions.communication_error(
                            &mut self.invoked_sessions,
                            SessionKey::ROOT,
                            send_id,
                        )?;
                    }
                }
                if sessions.finished(SessionKey::ROOT) {
                    break;
                }
            }
            if sessions.finished(SessionKey::ROOT) {
                return Ok(DrainStop::Finished);
            }
            self.refresh(sessions)?;
            for key in self.invoked_sessions.invoked_due_at(earliest) {
                // What earlier drains of this iteration did to `key`.
                self.refresh(sessions)?;
                if !self.invoked_sessions.contains(key)
                    || sessions.finished(key)
                    || self.invoked_sessions.deadline(key) != Some(earliest)
                {
                    continue;
                }
                self.invoked_sessions.touch(key);
                for due in sessions.drain_due(key)? {
                    sessions.require_budget()?;
                    let route = sessions.route_due(&mut self.invoked_sessions, key, due)?;
                    self.deliver_invoked_due(sessions, key, route)?;
                }
            }
        }
    }

    /// Reads again the status and deadline of every session a command ran
    /// on since the last reading.
    fn refresh(&mut self, sessions: &mut S) -> Result<(), S::Error> {
        for key in self.invoked_sessions.take_touched() {
            if !self.invoked_sessions.contains(key) {
                continue;
            }
            let reading = if sessions.finished(key) {
                Reading::Finished
            } else {
                Reading::Running(sessions.next_deadline(key)?)
            };
            self.invoked_sessions.record(key, reading);
        }
        Ok(())
    }

    /// Delivers one due send of the invoked session `key` where `route`
    /// says (step 7 of [`Self::drain_until`]).
    fn deliver_invoked_due(
        &mut self,
        sessions: &mut S,
        key: SessionKey,
        route: SessionsDueRoute<S>,
    ) -> Result<(), S::Error> {
        match route {
            DueRoute::Parent(event) => {
                self.invoked_sessions
                    .push_to_parent(key, Invoked::Event(event));
            }
            DueRoute::Invocation(id, event) => {
                if let Some(child) = self.invoked_sessions.child(key, &id) {
                    self.invoked_sessions.push_external(child, event);
                }
            }
            DueRoute::Local(event) => {
                sessions.deliver(&mut self.invoked_sessions, key, Delivery::External(event))?;
            }
            DueRoute::Undeliverable(send_id) => {
                sessions.communication_error(&mut self.invoked_sessions, key, send_id)?;
            }
            DueRoute::Dispatched => {}
        }
        Ok(())
    }

    /// Moves the shared logical clock of a stable run forward to `to`
    /// ms without delivering anything: advances every session by the
    /// difference. A `to` equal to the current time does nothing. Callers
    /// first run [`Self::drain_until`] with [`Horizon::Until`]`(to)`, so no
    /// deadline before `to` is pending.
    ///
    /// # Errors
    ///
    /// [`Violation::ClockRegression`] when `to` precedes the root clock,
    /// [`Violation::SendClockInvariant`] when a deadline before `to` is
    /// still pending, or the caller's clock failure.
    pub fn advance_clock_to(&mut self, sessions: &mut S, to: u64) -> Result<(), S::Error> {
        let now = sessions.now()?;
        if to < now {
            return Err(Violation::ClockRegression.into());
        }
        if to == now {
            return Ok(());
        }
        if self
            .next_deadline(sessions)?
            .is_some_and(|deadline| deadline < to)
        {
            return Err(Violation::SendClockInvariant.into());
        }
        sessions.advance_clock(to - now)
    }

    /// Earliest pending deadline across the root and every unfinished
    /// invoked session, in logical ms.
    ///
    /// # Errors
    ///
    /// The caller's clock failure.
    pub fn next_deadline(&mut self, sessions: &mut S) -> Result<Option<u64>, S::Error> {
        self.refresh(sessions)?;
        Ok(self.invoked_sessions.earliest_deadline())
    }
}
