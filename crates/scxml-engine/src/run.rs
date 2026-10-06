//! Engine-owned command driver: one effect transaction per engine command,
//! run to a stable configuration under one documented bound.
//!
//! Every surface that executes Machine IR (the generic machine component,
//! and native tests of the engine itself) calls [`Engine::run`]. The engine therefore owns, in exactly one place:
//!
//! - the effect-transaction protocol (begin, ordered batches, deferred
//!   `<if>`/`<foreach>` continuation batches, commit);
//! - block-scoped `error.execution`/`error.communication` recovery (SCXML
//!   3.4/5.7/5.10): a recoverable [`EffectFailure::Block`] aborts exactly
//!   its own executable-content block and later blocks keep running;
//! - macrostep completion: after the requested command the engine resumes
//!   until it is stable or finished, at most [`MAX_COMMANDS_PER_RUN`]
//!   engine commands per call;
//! - suspension: an effect the executor carries out later (a deferred
//!   `<invoke>` start or cancel, SCXML 6.4) suspends the transaction right
//!   after it, and [`Command::Continue`] applies that effect's outcome and
//!   the rest of the transaction exactly as one uninterrupted call would.
//!
//! The capability side ([`EffectExecutor`]) only evaluates guards, updates
//! the configuration observed by `In()`, and applies one effect at a time.
//! The engine hands it each configuration as a delta against the one it
//! installed last, so a microstep costs work linear in its effects plus the
//! configuration size.

use scxml_telemetry::{Module, Operation, SignalEvent, observe};
use scxml_types::{Event, StateId, TraceContext};

use crate::configuration::ConfigurationDelta;
use crate::{
    CommandResult, ConfigurationMark, Engine, EngineError, EngineStatus, GuardEvaluator,
    PlannedEffect, PlatformError,
};

/// Largest number of engine commands one [`Engine::run`] call executes: the
/// requested command plus at most `MAX_COMMANDS_PER_RUN - 1` resumes.
///
/// A run that reaches the bound while the machine still has runnable work
/// returns successfully with [`RunReport::stable`] `false`; the engine
/// state is consistent and a later [`Command::Resume`] continues it.
pub const MAX_COMMANDS_PER_RUN: usize = 1_024;

/// How an effect batch ended, passed to [`EffectExecutor::end_batch`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatchEnd {
    /// Every effect of the batch ran; recoverable block failures included.
    Completed,
    /// A fatal effect failure or an effect-count overflow stopped the batch.
    Aborted,
}

/// One externally requested engine command.
#[derive(Clone, Copy, Debug)]
pub enum Command<'a> {
    /// Enter the initial configuration (precondition: status `Ready`).
    Start,
    /// Process one external event (precondition: status `Running` and
    /// stable).
    Send(&'a Event),
    /// Continue pending work of a running machine (precondition: status
    /// `Running`); a stable machine completes an empty transaction.
    Resume,
    /// Cancel a running session (precondition: status `Running`): the W3C
    /// `exitInterpreter` procedure exits every active state in exit order,
    /// running its `<onexit>` content and cancelling its started
    /// invocations, then finishes the machine without a `done` event and
    /// discards every queued event. An invoking parent issues this for the
    /// invoked session it cancels (SCXML 6.4).
    Cancel,
    /// Continue the run an earlier call suspended (precondition:
    /// [`Engine::is_suspended`]): the suspended effect's outcome
    /// ([`EffectExecutor::resume_effect`]) settles as its application
    /// would have, then the rest of the transaction and the run follow.
    Continue,
}

/// Engine command position that failed, for host error classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunStage {
    /// The requested command itself (or its admission).
    Command,
    /// One engine-driven resume after the requested command.
    Resume,
    /// Opening one effect transaction.
    BeginEffects,
    /// Planning one deferred continuation batch.
    ContinueEffects,
    /// Committing one effect transaction.
    CompleteEffects,
    /// Aborting one executable-content block.
    AbortBlock,
}

/// Why one [`Engine::run`] call stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunFailureKind {
    /// The requested command was rejected before any effect or engine
    /// state change of this call; the engine is unchanged and reusable.
    Rejected,
    /// A later engine-driven command failed after earlier transactions of
    /// this call committed.
    Command,
    /// An effect or configuration capability failed fatally.
    Effect,
    /// The effect-transaction protocol broke.
    Protocol,
}

/// Failure of one effect applied through [`EffectExecutor::apply_effect`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectFailure<E> {
    /// Recoverable SCXML failure: the engine queues this platform error and
    /// aborts exactly the effect's executable-content block (SCXML
    /// 3.4/5.7/5.10); later blocks keep executing.
    Block(PlatformError),
    /// Recoverable failure of one `<send>`: as [`Self::Block`], and the
    /// queued error event carries the failing send's id as `_event.sendid`
    /// (SCXML 5.10.1).
    SendBlock(PlatformError, scxml_types::SendId),
    /// Unrecoverable capability failure: the run stops. Effects already
    /// applied stay committed; the engine is left mid-transaction and must
    /// be discarded by its owner.
    Fatal(E),
}

impl<E> From<E> for EffectFailure<E> {
    fn from(error: E) -> Self {
        Self::Fatal(error)
    }
}

/// Capability side of [`Engine::run`].
///
/// Calls are synchronous, ordered, and never re-entrant. The executor is
/// also the guard evaluator for transition selection and deferred `<if>`
/// decisions.
pub trait EffectExecutor: GuardEvaluator {
    /// Host failure type.
    type Error;

    /// Admits one engine command before it runs (for example a host step
    /// budget). An error stops the run before the command.
    ///
    /// # Errors
    ///
    /// Returns the host's own admission failure.
    fn admit_command(&mut self) -> Result<(), Self::Error>;

    /// Records that one admitted engine command produced its result.
    fn command_completed(&mut self);

    /// Records that the command just reported by
    /// [`Self::command_completed`] processed `event`: dequeued it and ran
    /// transition selection on it, whether a transition was selected or the
    /// event was ignored (W3C Appendix D `mainEventLoop`). Called once per
    /// processed event, in processing order, before the command's effects
    /// run. The default records nothing.
    fn event_processed(&mut self, _event: &Event) {}

    /// Maps one engine failure at `stage` to the host error.
    fn engine_error(&mut self, error: EngineError, stage: RunStage) -> Self::Error;

    /// Updates the configuration observed by data-model `In()` queries:
    /// removes every `exited` state (the others keep their order), then
    /// appends every `entered` state in order. A state in both lists is
    /// removed and then re-appended. The configuration starts empty, and the
    /// engine sends only changes: `exited` states are observed, and
    /// `entered` states are not observed once `exited` is removed. Each
    /// effect observes the configuration after the last update before it:
    /// exit content without the states exited before it, entry content with
    /// every state entered before it (SCXML 5.9.1; official case 409).
    ///
    /// # Errors
    ///
    /// Any failure stops the run as [`RunFailureKind::Effect`].
    fn update_configuration(
        &mut self,
        exited: &[StateId],
        entered: &[StateId],
    ) -> Result<(), Self::Error>;

    /// Opens one ordered effect batch whose `_event` is `trigger`.
    ///
    /// # Errors
    ///
    /// Any failure stops the run as [`RunFailureKind::Effect`].
    fn begin_batch(&mut self, trigger: Option<&Event>) -> Result<(), Self::Error>;

    /// The W3C carrier of the open batch: it rides on the
    /// `error.execution`/`error.communication` event a recoverable
    /// [`EffectFailure::Block`] or [`EffectFailure::SendBlock`] queues, so
    /// the error names the operation that failed. Carriers never change
    /// semantics; the default carries none.
    fn batch_trace_context(&self) -> Option<&TraceContext> {
        None
    }

    /// Applies one planned effect. Effect callbacks into the engine
    /// ([`Engine::raise`], [`Engine::enqueue_internal`],
    /// [`Engine::enqueue_external`], [`Engine::begin_iteration`],
    /// [`Engine::invocation_started`], [`Engine::queue_platform_error`]) go
    /// through `engine`.
    ///
    /// # Errors
    ///
    /// [`EffectFailure::Block`] for a recoverable SCXML failure and
    /// [`EffectFailure::Fatal`] for everything else.
    fn apply_effect(
        &mut self,
        engine: &mut Engine,
        planned: &PlannedEffect,
        trigger: Option<&Event>,
    ) -> Result<(), EffectFailure<Self::Error>>;

    /// Closes the batch opened by [`Self::begin_batch`]; `end` is
    /// [`BatchEnd::Aborted`] when a fatal effect failure stopped it.
    fn end_batch(&mut self, end: BatchEnd);

    /// Whether the effect [`Self::apply_effect`] just applied with `Ok`
    /// is carried out later: the run suspends right after it, with its
    /// batch still open, and [`Command::Continue`] settles it through
    /// [`Self::resume_effect`]. The default carries out every effect at
    /// once.
    fn suspend_requested(&mut self) -> bool {
        false
    }

    /// The outcome of the effect a suspended run stopped after, settled
    /// exactly as an [`Self::apply_effect`] result. Called once, by
    /// [`Command::Continue`]; never called when
    /// [`Self::suspend_requested`] never returns `true`.
    ///
    /// # Errors
    ///
    /// [`EffectFailure::Block`] for a recoverable SCXML failure and
    /// [`EffectFailure::Fatal`] for everything else; the default fails
    /// fatally, since it suspends nothing.
    fn resume_effect(
        &mut self,
        _engine: &mut Engine,
        _planned: &PlannedEffect,
        _trigger: Option<&Event>,
    ) -> Result<(), EffectFailure<Self::Error>> {
        Err(EffectFailure::Fatal(self.engine_error(
            EngineError::EffectProtocol,
            RunStage::BeginEffects,
        )))
    }
}

/// Where a suspended effect transaction stopped: the command's step, the
/// batch being applied, and the suspended effect's index in it.
#[derive(Debug)]
pub(crate) struct Suspension {
    step: CommandResult,
    /// The batch being applied.
    batch: AppliedBatch,
    /// Index of the suspended effect in the batch.
    index: usize,
    /// The suspended transaction's run stage.
    stage: RunStage,
    /// Engine commands the run completed before the suspended one.
    commands: u32,
}

/// The effect batch an applying transaction is in.
#[derive(Debug)]
enum AppliedBatch {
    /// The command's own [`CommandResult::planned_effects`].
    Command,
    /// One deferred `<if>`/`<foreach>` continuation batch and its trigger.
    Continuation(Box<ContinuationBatch>),
}

/// One deferred `<if>`/`<foreach>` continuation batch and its trigger.
#[derive(Debug)]
struct ContinuationBatch {
    effects: Vec<PlannedEffect>,
    trigger: Option<Event>,
}

/// How applying effects of one transaction ended.
enum Applied {
    /// Every effect ran; the transaction can commit.
    Completed,
    /// The executor suspended the run after the effect at this index.
    Suspended(usize),
}

/// How one [`Engine::transaction`] ended.
enum Transaction {
    /// The transaction committed with this command result.
    Committed(Box<CommandResult>),
    /// The transaction suspended at a deferred effect.
    Suspended(Box<Suspension>),
}

/// Result of one successful [`Engine::run`] call.
#[derive(Clone, Debug, PartialEq)]
pub struct RunReport {
    /// The last engine command's result, with the committed post-transaction state.
    pub result: CommandResult,
    /// Effects committed through the executor by every transaction of the
    /// call.
    pub committed_effects: u32,
    /// Engine commands executed (the requested command plus resumes).
    pub commands: u32,
    /// Whether the machine is stable or finished. `false` when the
    /// call stopped at [`MAX_COMMANDS_PER_RUN`] or suspended.
    pub stable: bool,
    /// Whether the call suspended at a deferred effect: `result` is the
    /// suspended command's result as planned (its status and leaves are the
    /// engine's at the suspension), and only [`Command::Continue`] runs
    /// next.
    pub suspended: bool,
}

/// Failure of one [`Engine::run`] call.
#[derive(Clone, Debug, PartialEq)]
pub struct RunFailure<E> {
    /// Host error.
    pub error: E,
    /// Why the call stopped.
    pub kind: RunFailureKind,
    /// Effects committed through the executor before the failure; they are
    /// never rolled back.
    pub committed_effects: u32,
}

impl Engine {
    /// Runs one command and then resumes until the machine is stable or
    /// finished, applying every effect transaction through `executor`.
    ///
    /// This is the only execution entry point for hosts. Each engine command
    /// is admitted through [`EffectExecutor::admit_command`], runs with the
    /// current full configuration observed, and its effect transaction is
    /// applied in planned order with block-scoped recovery. At most
    /// [`MAX_COMMANDS_PER_RUN`] engine commands run per call.
    ///
    /// # Examples
    ///
    /// A host supplies an [`EffectExecutor`]; this one records `<raise>`
    /// effects back into the engine and applies nothing else.
    ///
    /// ```
    /// use scxml_engine::{
    ///     BatchEnd, Command, Effect, EffectExecutor, EffectFailure, Engine, EngineError,
    ///     EngineStatus, EvaluationError, GuardEvaluator, PlannedEffect, RunStage,
    /// };
    /// use scxml_types::{Event, EventName, StateId};
    ///
    /// struct Host;
    ///
    /// impl GuardEvaluator for Host {
    ///     fn evaluate_condition(
    ///         &mut self,
    ///         _expression: &str,
    ///         _event: Option<&Event>,
    ///     ) -> Result<bool, EvaluationError> {
    ///         Err(EvaluationError::Unavailable)
    ///     }
    /// }
    ///
    /// impl EffectExecutor for Host {
    ///     type Error = EngineError;
    ///     fn admit_command(&mut self) -> Result<(), EngineError> {
    ///         Ok(())
    ///     }
    ///     fn command_completed(&mut self) {}
    ///     fn engine_error(&mut self, error: EngineError, _stage: RunStage) -> EngineError {
    ///         error
    ///     }
    ///     fn update_configuration(
    ///         &mut self,
    ///         _exited: &[StateId],
    ///         _entered: &[StateId],
    ///     ) -> Result<(), EngineError> {
    ///         Ok(())
    ///     }
    ///     fn begin_batch(&mut self, _trigger: Option<&Event>) -> Result<(), EngineError> {
    ///         Ok(())
    ///     }
    ///     fn apply_effect(
    ///         &mut self,
    ///         engine: &mut Engine,
    ///         planned: &PlannedEffect,
    ///         _trigger: Option<&Event>,
    ///     ) -> Result<(), EffectFailure<EngineError>> {
    ///         match &planned.effect {
    ///             Effect::Raise(name) => {
    ///                 let name = EventName::new(name.clone())
    ///                     .map_err(|_| EffectFailure::Fatal(EngineError::InvalidModel))?;
    ///                 Ok(engine.raise(Event::internal(name))?)
    ///             }
    ///             _ => Err(EffectFailure::Fatal(EngineError::UnsupportedModel)),
    ///         }
    ///     }
    ///     fn end_batch(&mut self, _end: BatchEnd) {}
    /// }
    ///
    /// let source = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"
    ///     initial="a" datamodel="null">
    ///   <state id="a">
    ///     <onentry><raise event="next"/></onentry>
    ///     <transition event="next" target="b"/>
    ///   </state>
    ///   <final id="b"/>
    /// </scxml>"#;
    /// let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
    ///     scxml_syntax::SourceId::new(1),
    ///     "example.scx",
    ///     source,
    /// ))?;
    /// let mut engine = Engine::new(scxml_compiler_core::lower(&document)?)?;
    /// // One call enters `a`, raises `next`, and runs to the final state.
    /// let report = engine
    ///     .run(Command::Start, &mut Host)
    ///     .map_err(|failure| failure.error)?;
    /// assert!(report.stable);
    /// assert_eq!(report.result.status, EngineStatus::Finished);
    /// assert_eq!(report.committed_effects, 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`RunFailure`] with [`RunFailureKind::Rejected`] when the
    /// requested command is rejected (lifecycle, admission, or selection
    /// failure) with the engine unchanged; any other kind means effects may
    /// have been committed and the engine must be discarded unless the kind
    /// is [`RunFailureKind::Command`] after a completed transaction.
    pub fn run<X: EffectExecutor>(
        &mut self,
        command: Command<'_>,
        executor: &mut X,
    ) -> Result<RunReport, RunFailure<X::Error>> {
        let span = observe(Module::Engine, Operation::EngineRun);
        let mut committed_effects = 0_u32;
        let mut commands = 0_u32;
        let mut current = command;
        // A suspended run continues only through `Command::Continue`, and
        // that command needs one.
        let mut suspended = match (self.suspension.is_some(), command) {
            (true, Command::Continue) => {
                let suspension = self.suspension.take();
                commands = suspension
                    .as_ref()
                    .map_or(0, |suspension| suspension.commands);
                suspension
            }
            (false, Command::Continue) | (true, _) => {
                return Err(RunFailure {
                    error: executor.engine_error(EngineError::EffectProtocol, RunStage::Command),
                    kind: RunFailureKind::Rejected,
                    committed_effects: 0,
                });
            }
            (false, _) => None,
        };
        let result = loop {
            let stage = if commands == 0 {
                RunStage::Command
            } else {
                RunStage::Resume
            };
            let transaction = match suspended.take() {
                Some(suspension) => {
                    self.continue_transaction(*suspension, executor, &mut committed_effects)
                }
                None => self.transaction(current, stage, executor, &mut committed_effects),
            };
            let step = match transaction {
                Ok(Transaction::Committed(step)) => *step,
                Ok(Transaction::Suspended(mut suspension)) => {
                    suspension.commands = commands;
                    let mut step = suspension.step.clone();
                    step.status = self.status.clone();
                    step.leaves.clone_from(&self.active_states);
                    step.stable = false;
                    self.suspension = Some(suspension);
                    break Ok(RunReport {
                        stable: false,
                        suspended: true,
                        result: step,
                        committed_effects,
                        commands,
                    });
                }
                Err(failure) => break Err(failure),
            };
            commands = commands.saturating_add(1);
            let settled = step.status != EngineStatus::Running || step.stable;
            let bound_reached =
                usize::try_from(commands).map_or(true, |count| count >= MAX_COMMANDS_PER_RUN);
            if settled || bound_reached {
                break Ok(RunReport {
                    stable: settled,
                    suspended: false,
                    result: step,
                    committed_effects,
                    commands,
                });
            }
            current = Command::Resume;
        };
        match &result {
            Ok(report) if report.stable || report.suspended => {
                span.event(SignalEvent::TransformationCompleted);
            }
            Ok(_) => span.event(SignalEvent::InputRejected),
            Err(_) => {
                span.event(SignalEvent::InputRejected);
                span.fail(scxml_telemetry::ErrorCategory::Execution);
            }
        }
        result
    }

    /// Runs one engine command and applies its whole effect transaction,
    /// unless the executor suspends it at a deferred effect.
    fn transaction<X: EffectExecutor>(
        &mut self,
        command: Command<'_>,
        stage: RunStage,
        executor: &mut X,
        committed_effects: &mut u32,
    ) -> Result<Transaction, RunFailure<X::Error>> {
        let command_kind = if stage == RunStage::Command {
            RunFailureKind::Rejected
        } else {
            RunFailureKind::Command
        };
        let fail = |error, kind, committed: u32| RunFailure {
            error,
            kind,
            committed_effects: committed,
        };
        executor
            .admit_command()
            .map_err(|error| fail(error, command_kind, *committed_effects))?;
        // Only commands change the configuration, so a transaction that
        // ended synchronized leaves the executor current.
        if !std::mem::take(&mut self.observed_current) {
            let configuration = match self.configuration_indices_all() {
                Ok(configuration) => configuration,
                Err(error) => {
                    return Err(fail(
                        executor.engine_error(error, stage),
                        command_kind,
                        *committed_effects,
                    ));
                }
            };
            self.observe_configuration(&configuration, executor, stage)
                .map_err(|failure| RunFailure {
                    committed_effects: *committed_effects,
                    ..failure
                })?;
        }
        let outcome = match command {
            Command::Start => self.start(),
            Command::Send(event) => self.send(event, executor),
            Command::Resume => self.resume(executor),
            Command::Cancel => self.cancel(),
            // `Engine::run` routes a continued run to
            // `continue_transaction`; nothing is suspended here.
            Command::Continue => Err(EngineError::EffectProtocol),
        };
        let step = outcome.map_err(|error| {
            fail(
                executor.engine_error(error, stage),
                command_kind,
                *committed_effects,
            )
        })?;
        executor.command_completed();
        for disposed in &step.event_dispositions {
            executor.event_processed(&disposed.event);
        }
        // The command's journal starts at the configuration just observed.
        self.observed_mark = ConfigurationMark::default();
        self.begin_effects(&step).map_err(|error| {
            fail(
                executor.engine_error(error, RunStage::BeginEffects),
                RunFailureKind::Protocol,
                *committed_effects,
            )
        })?;
        let trigger = step.trigger_event.clone();
        match self.apply_batch(
            &step.planned_effects,
            0,
            BatchOpening::Open,
            trigger.as_ref(),
            executor,
            committed_effects,
        )? {
            Applied::Completed => {}
            Applied::Suspended(index) => {
                return Ok(Transaction::Suspended(Box::new(Suspension {
                    step,
                    batch: AppliedBatch::Command,
                    index,
                    stage,
                    commands: 0,
                })));
            }
        }
        self.finish_transaction(step, stage, executor, committed_effects)
    }

    /// Settles the suspended effect with its outcome, then applies the rest
    /// of its batch and of the transaction as [`Self::transaction`] would
    /// have.
    fn continue_transaction<X: EffectExecutor>(
        &mut self,
        suspension: Suspension,
        executor: &mut X,
        committed_effects: &mut u32,
    ) -> Result<Transaction, RunFailure<X::Error>> {
        let Suspension {
            step,
            batch,
            index,
            stage,
            ..
        } = suspension;
        let (effects, trigger) = match &batch {
            AppliedBatch::Command => (step.planned_effects.as_slice(), step.trigger_event.clone()),
            AppliedBatch::Continuation(continuation) => (
                continuation.effects.as_slice(),
                continuation.trigger.clone(),
            ),
        };
        let Some(planned) = effects.get(index) else {
            return Err(RunFailure {
                error: executor.engine_error(EngineError::EffectProtocol, stage),
                kind: RunFailureKind::Protocol,
                committed_effects: *committed_effects,
            });
        };
        let applied = executor.resume_effect(self, planned, trigger.as_ref());
        self.settle_effect(applied, planned, executor, committed_effects)?;
        let rest = index.saturating_add(1);
        match self.apply_batch(
            effects,
            rest,
            BatchOpening::AlreadyOpen,
            trigger.as_ref(),
            executor,
            committed_effects,
        )? {
            Applied::Completed => {}
            Applied::Suspended(index) => {
                return Ok(Transaction::Suspended(Box::new(Suspension {
                    step,
                    batch,
                    index,
                    stage,
                    commands: 0,
                })));
            }
        }
        self.finish_transaction(step, stage, executor, committed_effects)
    }

    /// Once the batch being applied completed: applies every remaining
    /// continuation batch of the transaction, then commits it.
    fn finish_transaction<X: EffectExecutor>(
        &mut self,
        mut step: CommandResult,
        stage: RunStage,
        executor: &mut X,
        committed_effects: &mut u32,
    ) -> Result<Transaction, RunFailure<X::Error>> {
        if let Some(suspended) = self.apply_continuations(executor, committed_effects)? {
            return Ok(Transaction::Suspended(
                suspended.into_suspension(step, stage),
            ));
        }
        let fail = |error, kind, committed: u32| RunFailure {
            error,
            kind,
            committed_effects: committed,
        };
        self.complete_effects().map_err(|error| {
            fail(
                executor.engine_error(error, RunStage::CompleteEffects),
                RunFailureKind::Protocol,
                *committed_effects,
            )
        })?;
        step.status = self.status.clone();
        step.leaves.clone_from(&self.active_states);
        step.stable = self.stable;
        step.configuration = self.configuration_ids_all().map_err(|error| {
            fail(
                executor.engine_error(error, RunStage::CompleteEffects),
                RunFailureKind::Protocol,
                *committed_effects,
            )
        })?;
        let configuration = self.configuration_indices_all().map_err(|error| {
            fail(
                executor.engine_error(error, RunStage::CompleteEffects),
                RunFailureKind::Protocol,
                *committed_effects,
            )
        })?;
        self.observe_configuration(&configuration, executor, RunStage::CompleteEffects)
            .map_err(|failure| RunFailure {
                committed_effects: *committed_effects,
                ..failure
            })?;
        self.observed_current = true;
        Ok(Transaction::Committed(Box::new(step)))
    }

    /// Brings the executor's configuration to `target` with one delta.
    /// Linear in the configuration size; called once at each end of a
    /// transaction, never per effect.
    fn observe_configuration<X: EffectExecutor>(
        &mut self,
        target: &[usize],
        executor: &mut X,
        stage: RunStage,
    ) -> Result<(), RunFailure<X::Error>> {
        let delta = self.observed.delta_to(target);
        self.send_delta(&delta, executor, stage)
    }

    /// Brings the executor's configuration to the one journal `mark` names,
    /// with one delta linear in the journal changes since the last update.
    fn observe_mark<X: EffectExecutor>(
        &mut self,
        mark: ConfigurationMark,
        executor: &mut X,
        stage: RunStage,
    ) -> Result<(), RunFailure<X::Error>> {
        if mark == self.observed_mark {
            return Ok(());
        }
        let delta = self
            .journal
            .delta(self.observed_mark, mark)
            .map_err(|error| RunFailure {
                error: executor.engine_error(error, stage),
                kind: RunFailureKind::Protocol,
                committed_effects: 0,
            })?;
        self.send_delta(&delta, executor, stage)?;
        self.observed_mark = mark;
        Ok(())
    }

    /// Hands the executor one non-empty delta, as state ids, and records it
    /// as observed once the executor accepted it.
    fn send_delta<X: EffectExecutor>(
        &mut self,
        delta: &ConfigurationDelta,
        executor: &mut X,
        stage: RunStage,
    ) -> Result<(), RunFailure<X::Error>> {
        if delta.is_empty() {
            return Ok(());
        }
        let ids = self
            .state_ids(&delta.exited)
            .and_then(|exited| Ok((exited, self.state_ids(&delta.entered)?)));
        let (exited, entered) = ids.map_err(|error| RunFailure {
            error: executor.engine_error(error, stage),
            kind: RunFailureKind::Protocol,
            committed_effects: 0,
        })?;
        executor
            .update_configuration(&exited, &entered)
            .map_err(|error| RunFailure {
                error,
                kind: RunFailureKind::Effect,
                committed_effects: 0,
            })?;
        self.observed.apply(delta);
        Ok(())
    }

    /// Plans and applies every deferred `<if>`/`<foreach>` continuation
    /// batch of the open transaction, installing each pending decision's
    /// configuration first. Bounded by the engine's per-command effect limit.
    /// Returns where the executor suspended a continuation batch, if it
    /// did.
    fn apply_continuations<X: EffectExecutor>(
        &mut self,
        executor: &mut X,
        committed_effects: &mut u32,
    ) -> Result<Option<SuspendedContinuation>, RunFailure<X::Error>> {
        loop {
            if let Some(context) = self.pending_condition_context() {
                self.observe_mark(context.configuration, executor, RunStage::ContinueEffects)
                    .map_err(|failure| RunFailure {
                        committed_effects: *committed_effects,
                        ..failure
                    })?;
            }
            let next = self
                .continue_effects(executor)
                .map_err(|error| RunFailure {
                    error: executor.engine_error(error, RunStage::ContinueEffects),
                    kind: RunFailureKind::Effect,
                    committed_effects: *committed_effects,
                })?;
            let Some(next) = next else {
                return Ok(None);
            };
            let trigger = next.trigger_event.clone();
            match self.apply_batch(
                &next.planned_effects,
                0,
                BatchOpening::Open,
                trigger.as_ref(),
                executor,
                committed_effects,
            )? {
                Applied::Completed => {}
                Applied::Suspended(index) => {
                    return Ok(Some(SuspendedContinuation {
                        effects: next.planned_effects,
                        trigger,
                        index,
                    }));
                }
            }
        }
    }

    /// Applies one ordered effect batch from `from`, skipping the remaining
    /// effects of every block a recoverable failure aborted (SCXML
    /// 3.4/5.7/5.10). Stops right after an effect the executor suspends
    /// the run at, leaving the batch open.
    fn apply_batch<X: EffectExecutor>(
        &mut self,
        batch: &[PlannedEffect],
        from: usize,
        opening: BatchOpening,
        trigger: Option<&Event>,
        executor: &mut X,
        committed_effects: &mut u32,
    ) -> Result<Applied, RunFailure<X::Error>> {
        if opening == BatchOpening::Open {
            executor.begin_batch(trigger).map_err(|error| RunFailure {
                error,
                kind: RunFailureKind::Effect,
                committed_effects: *committed_effects,
            })?;
        }
        for (index, planned) in batch.iter().enumerate().skip(from) {
            if self.block_is_failed(planned.context.block) {
                continue;
            }
            if let Err(failure) = self.observe_mark(
                planned.context.configuration,
                executor,
                RunStage::BeginEffects,
            ) {
                executor.end_batch(BatchEnd::Aborted);
                return Err(RunFailure {
                    committed_effects: *committed_effects,
                    ..failure
                });
            }
            let applied = executor.apply_effect(self, planned, trigger);
            if applied.is_ok() && executor.suspend_requested() {
                return Ok(Applied::Suspended(index));
            }
            self.settle_effect(applied, planned, executor, committed_effects)?;
        }
        executor.end_batch(BatchEnd::Completed);
        Ok(Applied::Completed)
    }

    /// Settles one effect's application: counts a committed effect, aborts
    /// the block of a recoverable failure, and ends the batch on a fatal
    /// one.
    fn settle_effect<X: EffectExecutor>(
        &mut self,
        applied: Result<(), EffectFailure<X::Error>>,
        planned: &PlannedEffect,
        executor: &mut X,
        committed_effects: &mut u32,
    ) -> Result<(), RunFailure<X::Error>> {
        {
            match applied {
                Ok(()) => {
                    let Some(count) = committed_effects.checked_add(1) else {
                        executor.end_batch(BatchEnd::Aborted);
                        return Err(RunFailure {
                            error: executor
                                .engine_error(EngineError::EffectLimit, RunStage::AbortBlock),
                            kind: RunFailureKind::Protocol,
                            committed_effects: *committed_effects,
                        });
                    };
                    *committed_effects = count;
                }
                Err(EffectFailure::Block(error)) => {
                    let context = executor.batch_trace_context().cloned();
                    if let Err(abort) =
                        self.block_failed(error, None, planned.context.block, context)
                    {
                        executor.end_batch(BatchEnd::Aborted);
                        return Err(RunFailure {
                            error: executor.engine_error(abort, RunStage::AbortBlock),
                            kind: RunFailureKind::Protocol,
                            committed_effects: *committed_effects,
                        });
                    }
                }
                Err(EffectFailure::SendBlock(error, send_id)) => {
                    let context = executor.batch_trace_context().cloned();
                    if let Err(abort) =
                        self.block_failed(error, Some(send_id), planned.context.block, context)
                    {
                        executor.end_batch(BatchEnd::Aborted);
                        return Err(RunFailure {
                            error: executor.engine_error(abort, RunStage::AbortBlock),
                            kind: RunFailureKind::Protocol,
                            committed_effects: *committed_effects,
                        });
                    }
                }
                Err(EffectFailure::Fatal(error)) => {
                    executor.end_batch(BatchEnd::Aborted);
                    return Err(RunFailure {
                        error,
                        kind: RunFailureKind::Effect,
                        committed_effects: *committed_effects,
                    });
                }
            }
        }
        Ok(())
    }
}

/// Whether [`Engine::apply_batch`] opens its batch or continues one a
/// suspension left open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BatchOpening {
    /// Open the batch first ([`EffectExecutor::begin_batch`]).
    Open,
    /// The batch is already open.
    AlreadyOpen,
}

/// A continuation batch the executor suspended.
struct SuspendedContinuation {
    effects: Vec<PlannedEffect>,
    trigger: Option<Event>,
    index: usize,
}

impl SuspendedContinuation {
    fn into_suspension(self, step: CommandResult, stage: RunStage) -> Box<Suspension> {
        Box::new(Suspension {
            step,
            batch: AppliedBatch::Continuation(Box::new(ContinuationBatch {
                effects: self.effects,
                trigger: self.trigger,
            })),
            index: self.index,
            stage,
            commands: 0,
        })
    }
}
