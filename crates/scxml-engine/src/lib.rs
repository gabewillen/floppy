//! Deterministic SCXML execution kernel over canonical Machine IR.
//!
//! The engine owns transition selection, configuration mutation, and the
//! ordering of every externally visible action. External behavior is
//! represented as ordered planned effects, while condition evaluation is a
//! borrowed pure capability.
//!
//! Supported semantics:
//!
//! - compound hierarchy, parallel regions, and shallow/deep history
//!   pseudostates, with W3C `selectTransitions`, conflict resolution, exit
//!   and entry sets, and exit/entry ordering;
//! - `done.state` done events for compound and parallel states,
//!   including `<donedata>` payload effects;
//! - ordered executable content with engine-owned `<if>` branch decisions and
//!   `<foreach>` iteration planning, block-scoped error recovery, and
//!   `error.execution`/`error.communication` platform events;
//! - late data binding (SCXML 5.3): one [`Effect::BindStateData`] per state on
//!   its first entry, before its `<onentry>` content;
//! - the SCXML 6.4 invocation lifecycle: [`Effect::Invoke`] for every state
//!   entered and not exited during a macrostep, emitted when the macrostep
//!   ends; [`Effect::CancelInvoke`] after an invoking state's `<onexit>`
//!   content; and `<finalize>` content planned before transition selection
//!   for events carrying a started invocation's id.
//!
//! [`interpret`] gives every planned effect its SCXML meaning (data
//! binding, `<send>`, `<cancel>`, `<log>`, `<foreach>`, `<donedata>`, and
//! invocations) over an explicit session capability boundary; hosts only
//! implement that boundary.
//!
//! Every executed command reports the full ordered entered and exited state
//! sets and the complete active configuration (ancestors included).
//!
//! External `<script src>` (SCXML 1.0 section 5.8) plans
//! [`Effect::RunScript`] unless its data-model text is pinned into the IR,
//! which plans [`Effect::ExecuteScript`].
//!
//! Transitions and initial transitions may name several orthogonal targets:
//! the W3C `computeExitSet`/`computeEntrySet` walk runs over every target,
//! so each named region leaf is entered and uncovered regions take their
//! default entry.
//!
//! [`Engine::new`] fails closed with [`EngineError::UnsupportedModel`] for
//! id-less states, compound
//! states without children or an initial transition, and atomic or final
//! states with children. Ancestor walks are bounded by
//! [`MAX_HIERARCHY_DEPTH`].

#![forbid(unsafe_code)]

mod configuration;
mod contracts;
pub mod interpret;
mod run;
mod snapshot;

pub use configuration::ConfigurationMark;

pub use run::{
    BatchEnd, Command, EffectExecutor, EffectFailure, MAX_COMMANDS_PER_RUN, RunFailure,
    RunFailureKind, RunReport, RunStage,
};
pub use snapshot::{
    EngineSnapshot, HistoryValue, InvocationSnapshot, Previews, SnapshotOptions, TransitionListing,
    TransitionSnapshot,
};

use std::collections::VecDeque;
use std::fmt;

use configuration::{ConfigurationJournal, ObservedConfiguration};

use scxml_compiler_core::{
    AssignTypeIr, Binding, DataIr, ElseIfIr, ExecutableContentIr, InvokeChildIr, InvokeIr,
    MAX_NESTED_ITEMS, MAX_STRING_BYTES, MachineIr, SendIr, StateIr, StateKind, TransitionIr,
    TransitionType, XmlNodeIr,
};
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};
use scxml_types::{
    AssignPlacement, AssignSource, DataValue, Event, EventName, FiniteNumber, InvokeId, StateId,
    TraceContext,
};

/// Maximum effects returned by one engine command (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_EFFECTS_PER_COMMAND: usize = scxml_types::wasm_bound(10_000);

/// Fails with [`EngineError::EffectLimit`] when a command result that already holds
/// `effects` effects has no room for one more ([`MAX_EFFECTS_PER_COMMAND`]).
fn ensure_effect_room(effects: usize) -> Result<(), EngineError> {
    if scxml_types::exceeds_wasm_bound(effects.saturating_add(1), MAX_EFFECTS_PER_COMMAND) {
        Err(EngineError::EffectLimit)
    } else {
        Ok(())
    }
}

/// Maximum ancestor-chain depth walked by hierarchy queries.
///
/// `MachineIr::validate` guarantees every parent index is smaller than the
/// child index, so ancestor walks terminate; this bound additionally fails
/// closed before any chain could exceed the IR nested-item bound (a wasm
/// build bound; natively `usize::MAX`).
pub const MAX_HIERARCHY_DEPTH: usize = MAX_NESTED_ITEMS;

/// Pure data-model capability used only while selecting transitions.
pub trait GuardEvaluator {
    /// Evaluates one condition against the current event context.
    ///
    /// Implementations must not mutate data-model state or perform I/O.
    /// `None` denotes eventless selection outside an event dispatch.
    ///
    /// # Errors
    ///
    /// Returns a closed failure category without exposing expression text.
    fn evaluate_condition(
        &mut self,
        expression: &str,
        event: Option<&Event>,
    ) -> Result<bool, EvaluationError>;

    /// The W3C carrier of the effect batch this evaluator last opened in
    /// the current [`Engine::run`] call: the batch whose effects the next
    /// condition observes. The `error.execution` a failed condition queues
    /// carries it, or, before the call's first batch, the carrier of the
    /// event under evaluation.
    /// Carriers never change semantics; the default carries none.
    fn last_batch_trace_context(&self) -> Option<&TraceContext> {
        None
    }
}

/// Closed guard-evaluation failure crossing the engine capability boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvaluationError {
    /// The expression or event context was invalid.
    InvalidInput,
    /// The selected data model rejected execution.
    Execution,
    /// The selected data model is unavailable.
    Unavailable,
    /// The capability provider failed.
    Platform,
}

impl fmt::Display for EvaluationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidInput => "guard input is invalid",
            Self::Execution => "guard evaluation failed",
            Self::Unavailable => "guard evaluator is unavailable",
            Self::Platform => "guard evaluator platform failed",
        })
    }
}

impl std::error::Error for EvaluationError {}

/// Engine-owned durable lifecycle state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EngineStatus {
    /// The machine has not entered its initial state.
    Ready,
    /// The machine is active.
    Running,
    /// The machine reached a top-level final state.
    Finished,
}

/// One request the deterministic engine makes of its host.
///
/// "Effect" is host vocabulary, not an SCXML term. Most effects carry out
/// executable content (SCXML 4); the others generate a done event with its
/// `<donedata>`, return the finished session's `<donedata>`, start, cancel,
/// or forward to an `<invoke>` (SCXML 6.4), or bind a state's `<data>`
/// (SCXML 5.3). The host carries them out in order through the session's
/// capabilities.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Execute extension namespace content through its linked extension component.
    ExecuteExtension(XmlNodeIr),
    /// Emit one implementation log after evaluating its optional expression.
    Log {
        /// Optional producer label.
        label: Option<String>,
        /// Optional data-model expression.
        expression: Option<String>,
    },
    /// Raise one internal event: its name; whether a `<raise>` element
    /// creates it (`true`), so the session first checks its event contracts
    /// (the element's own literal `event:schema` and those of the
    /// document's transitions handling it, `namespaces/event`), or the
    /// engine generates it (`false`: a bare `done.state.<id>`, never
    /// checked); and the element's `event:schema`, if any.
    Raise(String, bool, Option<scxml_compiler_core::EventSchemaIr>),
    /// Generate the done event `done.state.<id>` of a compound state whose
    /// `<final>` child declares `<donedata>`, with that `<donedata>`
    /// evaluated as its data (SCXML 3.7.1, 5.5; W3C Appendix D
    /// `enterStates`). A `<final>` without `<donedata>` plans a bare
    /// [`Effect::Raise`] of the same event instead.
    DoneEvent {
        /// The done event's name, `done.state.<id>`.
        event: String,
        /// The final state's `<donedata>`.
        donedata: std::boxed::Box<scxml_compiler_core::DoneDataIr>,
    },
    /// Evaluate the `<donedata>` of the top-level `<final>` that finished
    /// the machine as the session's return value (W3C Appendix D
    /// `exitInterpreter`/`returnDoneEvent`; SCXML 6.4).
    ///
    /// Planned once, after that final state's `<onexit>` content, only when
    /// the final declares `<donedata>`. A host that invoked this session
    /// attaches the value to the `done.invoke.<id>` event it delivers to the
    /// invoking session; a failed evaluation leaves that event's data
    /// undefined. Nothing is raised: the machine has already finished. For a
    /// session nobody invoked the host returns the value to the environment
    /// (SCXML 5.5: the processor "SHOULD return the data to the environment
    /// in an implementation-dependent manner").
    ReturnDoneEvent(std::boxed::Box<scxml_compiler_core::DoneDataIr>),
    /// Evaluate a value and assign it to a data-model location.
    Assign {
        /// Data-model location.
        location: String,
        /// Optional value expression.
        expression: Option<String>,
        /// Inline `<assign>` children (SCXML 5.8) already materialized: an
        /// XML value when the children contain an element, else their text.
        /// `None` when the value comes from `expression` (or, for the B.2
        /// `delete` type, when there is no value).
        content: Option<DataValue>,
        /// `XPath` placement (SCXML 1.0 CR 2014-03-13 §C.3.6); `None` is the default
        /// `replacechildren`.
        assign_type: Option<AssignTypeIr>,
        /// `XPath` attribute name for `addattribute` (SCXML 1.0 CR 2014-03-13 §C.3.6).
        attr: Option<String>,
    },
    /// One automatic data-model update of an empty `<finalize>` (SCXML
    /// 6.5.2): the capability side assigns, as if by `<assign>`, the value
    /// the invoked child's event carries under `name` to `location`, and
    /// changes nothing when the event carries no such value.
    UpdateFromReturnedValue {
        /// The `<param>` name; `None` for a `namelist` item, whose name is
        /// the item's namelist key.
        name: Option<String>,
        /// The `namelist` item or `<param location>` to update.
        location: String,
    },
    /// Assign an already-materialized value to a data-model location.
    ///
    /// The engine synthesizes this for `<foreach>` item/index assignments from
    /// the capability-side array snapshot; hosts assign the value directly
    /// without expression evaluation.
    AssignValue {
        /// Data-model location.
        location: String,
        /// Materialized value from the engine-owned iteration state.
        value: DataValue,
    },
    /// Execute one inline data-model script.
    ExecuteScript(String),
    /// Run one external script the host resolved at document load (SCXML
    /// 1.0 section 5.8): a pinned Wasm script component or an unpinned
    /// reference. The capability side runs it against the active data model;
    /// a recoverable failure raises `error.execution`.
    RunScript(scxml_compiler_core::ScriptSourceIr),
    /// Begin one `<foreach>` iteration over a data-model array expression.
    ///
    /// The capability side evaluates `array_expression` once, reports the
    /// element count through [`Engine::begin_iteration`], and then executes the
    /// planned body batches the engine returns through
    /// `Engine::continue_effects`. The engine owns per-item `item`/`index`
    /// assignments and body planning; hosts never select loop semantics.
    Iterate {
        /// Collection expression evaluated by the capability side.
        array_expression: String,
        /// Data-model location receiving each element.
        item_location: String,
        /// Optional data-model location receiving each index, counted from
        /// the base the capability side reports to
        /// [`Engine::begin_iteration`].
        index_location: Option<String>,
    },
    /// Submit a preserved send request for capability-side evaluation.
    Send(Box<SendIr>),
    /// Cancel a literal or expression-produced send identifier.
    Cancel {
        /// Literal send identifier.
        send_id: Option<String>,
        /// Send identifier expression.
        send_id_expression: Option<String>,
    },
    /// Bind one entered state's late-bound data declarations (SCXML 5.3
    /// `binding="late"`).
    ///
    /// Emitted exactly once per state, on its first entry, immediately before
    /// that state's `<onentry>` content. The capability side evaluates the
    /// declarations in document order; a recoverable initializer failure
    /// leaves the variable undefined and is reported through
    /// [`Engine::queue_platform_error`] without aborting the entry.
    BindStateData {
        /// Entered state owning the declarations.
        state: StateId,
        /// Declarations in document order.
        declarations: Vec<DataIr>,
    },
    /// Start one `<invoke>` of a state entered and not exited during the
    /// macrostep that just ended (SCXML 6.4).
    ///
    /// Emitted in entry order and invoke document order. The capability side
    /// reports a successful start through [`Engine::invocation_started`];
    /// a failed start queues `error.execution` through
    /// [`Engine::queue_platform_error`] and is never started.
    Invoke {
        /// Invoking state.
        state: StateId,
        /// Invocation declaration.
        invoke: Box<InvokeIr>,
    },
    /// Cancel one started invocation because its invoking state exited
    /// (SCXML 6.4). Emitted after the invoking state's `<onexit>` content
    /// and before the state leaves the configuration.
    CancelInvoke {
        /// Identifier reported through [`Engine::invocation_started`].
        invoke_id: InvokeId,
    },
    /// Forward an exact copy of one external event to one started
    /// invocation declared `autoforward="true"` (SCXML 6.4.1).
    ///
    /// The engine emits one `Forward` per live autoforward invocation, in
    /// configuration document order and invoke document order, for every
    /// external event it dequeues (a `send` command and the self-addressed
    /// `pending_external` drain alike), in the same pre-selection batch as
    /// the event's `<finalize>` content (Appendix D `mainEventLoop`). The
    /// effect runs in [`EffectExecutionPhase::Finalize`]; the capability
    /// side only delivers the event to the invoked session it owns.
    Forward {
        /// Identifier reported through [`Engine::invocation_started`].
        invoke_id: InvokeId,
        /// The external event exactly as the engine dequeued it.
        event: Box<Event>,
    },
}

/// Platform error event queued by the capability side (SCXML 5.10/6.2).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformError {
    /// `error.execution`: recoverable data-model or content failure.
    Execution,
    /// `error.communication`: an event could not be delivered.
    Communication,
}

impl PlatformError {
    /// Canonical SCXML event name of this platform error.
    #[must_use]
    pub const fn event_name(self) -> &'static str {
        match self {
            Self::Execution => "error.execution",
            Self::Communication => "error.communication",
        }
    }
}

/// SCXML execution phase in which an emitted effect must run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectExecutionPhase {
    /// Root script content before the initial transition executes.
    Initialization,
    /// Exit content while the pre-transition configuration is still observable.
    Exit,
    /// Transition content after exited states leave and before targets enter.
    Transition,
    /// Entry content after target states join the observable configuration.
    Entry,
    /// End-of-macrostep invocation start with the settled configuration.
    Invoke,
    /// Invocation `<finalize>` content with `_event` bound to the child
    /// event, before transition selection for that event (SCXML 6.4).
    Finalize,
}

/// Immutable machine context that applies while one effect executes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectContext {
    /// Execution phase that owns the effect.
    pub phase: EffectExecutionPhase,
    /// The active configuration visible to data-model `In()` queries while
    /// the effect executes, as a position in the planning command's
    /// configuration journal ([`Engine::effect_configuration`] lists it).
    pub configuration: ConfigurationMark,
    /// Monotonic executable-content block ordinal. Effects from one
    /// `<onentry>`/`<onexit>`/transition block share an ordinal, so a
    /// recoverable error can abort exactly its own block (SCXML 3.4/5.7:
    /// each handler is an independent block).
    pub block: u64,
}

/// One effect paired with the exact configuration visible during its phase.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedEffect {
    /// Effect requested from the capability boundary.
    pub effect: Effect,
    /// Phase and configuration to install while executing the effect.
    pub context: EffectContext,
}

/// Selection outcome for an event examined during one bounded command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventDisposition {
    /// The event selected a transition.
    Selected,
    /// No transition matched and the event was intentionally consumed.
    Ignored,
}

/// Ordered observable disposition of one examined event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisposedEvent {
    /// Event examined by transition selection.
    pub event: Event,
    /// Deterministic selection outcome.
    pub disposition: EventDisposition,
}

/// Result of one bounded engine command.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandResult {
    /// Lifecycle state after the command.
    pub status: EngineStatus,
    /// Active configuration leaves after the command, in document order,
    /// one leaf per parallel region: atomic and final states, and a
    /// `<parallel>` without regions; ancestors are implicit (SCXML 3.4).
    pub leaves: Vec<StateId>,
    /// Ordered effects that must execute before the next command.
    ///
    /// This is the ordered compatibility projection of [`Self::planned_effects`].
    /// Phase-sensitive executors must use `planned_effects` so data-model queries
    /// observe the configuration required by SCXML execution ordering.
    pub effects: Vec<Effect>,
    /// Ordered effects paired with their phase-specific execution configuration.
    pub planned_effects: Vec<PlannedEffect>,
    /// SCXML current event visible while applying this command's effects.
    ///
    /// Eventless transitions inherit the event that initiated the active macrostep.
    /// This is `None` only when no event is current, such as initial eventless work.
    pub trigger_event: Option<Event>,
    /// Ordered outcomes for every event examined by this command.
    pub event_dispositions: Vec<DisposedEvent>,
    /// States this command exited, in W3C exit order (descendants before
    /// ancestors, reverse document order). Empty unless a microstep with
    /// targets executed.
    pub exited_states: Vec<StateId>,
    /// States this command entered, in W3C entry order (ancestors before
    /// descendants, document order). Empty unless a start or targeted
    /// microstep executed.
    pub entered_states: Vec<StateId>,
    /// Complete active configuration after the command, ancestors included,
    /// in document order.
    pub configuration: Vec<StateId>,
    /// Whether the machine may accept another external event.
    pub stable: bool,
}

/// Stored history members keyed by history-state table index (SCXML 3.11).
type HistoryValues = std::collections::BTreeMap<usize, Vec<usize>>;

/// Deterministic single-owner interpreter state for the supported slice.
///
/// The active configuration is stored as its leaf set: one leaf (atomic,
/// final, or a `<parallel>` without regions) per parallel region, every leaf distinct, every parallel region fully
/// covered. Ancestors are derived on demand.
pub struct Engine {
    machine: MachineIr,
    /// State table index by declared id, built once from the validated
    /// (unique-id) table so id resolution never scans the table.
    state_indices: std::collections::BTreeMap<String, usize>,
    /// Which event names any transition descriptor can match, so selection
    /// skips the configuration walk for an event nothing listens for.
    event_index: EventIndex,
    /// Whether any non-history state declares an eventless transition.
    has_eventless: bool,
    /// The current command's configurations: its starting configuration
    /// and every exit and entry its plan makes, which effect contexts mark.
    journal: ConfigurationJournal,
    /// The configuration [`Engine::run`] last handed its executor, and its
    /// journal mark while the current command's effects apply.
    observed: ObservedConfiguration,
    observed_mark: ConfigurationMark,
    /// Whether `observed` is the active configuration in document order:
    /// set when a transaction ends synchronized, cleared when one starts.
    observed_current: bool,
    status: EngineStatus,
    /// Active configuration leaves in document order.
    active_states: Vec<StateId>,
    internal: VecDeque<Event>,
    /// Self-addressed external events queued by `<send>` effects.
    ///
    /// The engine drains this queue only after the internal queue is empty,
    /// preserving the macrostep rule that internal events process before
    /// externally generated ones.
    pending_external: VecDeque<Event>,
    current_event: Option<Event>,
    stable: bool,
    /// Stored history values keyed by history-state table index (SCXML 3.11).
    /// Deep history stores atomic/final leaf descendants; shallow history
    /// stores direct children. Bounded by the history-state count and the IR
    /// nested-item limit per value.
    history_values: HistoryValues,
    pending_step: Option<CommandResult>,
    effect_phase: EffectPhase,
    armed_plan: Option<PendingPlan>,
    /// Monotonic allocator for executable-content block ordinals. Blocks are
    /// numbered across the whole engine lifetime so every planned effect
    /// carries a unique block identity and a recoverable error aborts
    /// exactly its own block (SCXML 3.4/5.7).
    next_effect_block: u64,
    /// W3C `statesToInvoke`: states entered during the current macrostep
    /// that declare invocations, keyed by state table index. Bounded by the
    /// state table.
    states_to_invoke: std::collections::BTreeSet<usize>,
    /// Invocations reported started and not yet cancelled, in start order.
    /// Bounded by [`MAX_NESTED_ITEMS`].
    active_invocations: Vec<ActiveInvocation>,
    /// `(state index, invoke document order)` pairs emitted as
    /// [`Effect::Invoke`] in the applying phase and not yet reported started.
    awaiting_invocations: Vec<(usize, u32)>,
    /// External event whose `<finalize>` content was planned and whose
    /// transition selection runs on the next [`Engine::resume`] (SCXML 6.4
    /// finalize-before-selection ordering).
    pending_dispatch: Option<Event>,
    /// States whose late-bound data declarations were already bound (SCXML
    /// 5.3 first-entry binding). Bounded by the state table.
    bound_states: std::collections::BTreeSet<usize>,
    /// Executable-content block ordinals issued to the capability side in
    /// the current applying phase; bounded by [`MAX_EFFECTS_PER_COMMAND`].
    issued_blocks: std::collections::BTreeSet<u64>,
    /// The effect transaction a [`Engine::run`] call suspended at a
    /// deferred effect; [`Command::Continue`] resumes it.
    suspension: Option<Box<run::Suspension>>,
    /// The literal `event:schema` contracts `<send>` and `<raise>` check
    /// (`contracts`), with the schemas compiled so far.
    contracts: contracts::EventContracts,
}

/// One started invocation owned by an active invoking state (SCXML 6.4).
#[derive(Clone, Debug, Eq, PartialEq)]
struct ActiveInvocation {
    /// Invoking state table index.
    state: usize,
    /// Invoke document-order ordinal within the machine.
    invoke_order: u32,
    /// Identifier reported by the capability side.
    invoke_id: InvokeId,
    /// The started invocation's type URI: the canonical SCXML one for an
    /// invoked session, the exact service type URI for an invoked service.
    type_uri: String,
}

#[derive(Default)]
enum EffectPhase {
    #[default]
    Idle,
    Applying {
        /// Expected raise event names with their executable-content block
        /// ordinals, so a failed block can drop exactly its own raises while
        /// later blocks keep theirs (SCXML 3.4/5.7).
        pending_raises: VecDeque<(String, u64)>,
        planned_effects: usize,
        pending_plan: Option<PendingPlan>,
        /// Blocks aborted by a recoverable error in this phase; their
        /// remaining effects are skipped. Bounded by the issued blocks.
        failed_blocks: std::collections::BTreeSet<u64>,
    },
}

/// One deferred `<if>` branch decision inside an applying effect phase.
///
/// The decision is owned by the engine and evaluated only at the effect-execution
/// point through the borrowed guard capability, so earlier effects of the same
/// ordered block can influence the selected branch.
#[derive(Clone, Debug, PartialEq)]
struct DeferredConditional {
    /// Condition evaluated at the next continuation.
    condition: String,
    /// Branch body planned when the condition evaluates true.
    branch: Vec<ExecutableContentIr>,
    /// Subsequent conditional branches, in document order.
    else_if: VecDeque<ElseIfIr>,
    /// Optional final branch.
    else_actions: Option<Vec<ExecutableContentIr>>,
    /// Phase and configuration observable while evaluating the condition.
    context: EffectContext,
}

/// One active `<foreach>` iteration awaiting its capability-side array snapshot.
///
/// The engine owns loop semantics: the capability side only evaluates the array
/// expression once and reports its bounded element snapshot through
/// [`Engine::begin_iteration`]. Each continuation then emits one ordered batch
/// of item/index assignment effects followed by the body's next planned work,
/// so a failing body element (SCXML error.execution) aborts remaining
/// iterations and leaves partial effects committed.
#[derive(Clone, Debug, PartialEq)]
struct ActiveIteration {
    /// Data-model location receiving each element.
    item_location: String,
    /// Optional data-model location receiving each index.
    index_location: Option<String>,
    /// Shallow element snapshot reported by the capability side, oldest first.
    elements: VecDeque<DataValue>,
    /// Zero-based ordinal of the next element to iterate.
    next_index: u64,
    /// Loop body planned once per iteration in document order.
    body: Vec<ExecutableContentIr>,
    /// Phase and configuration observable while planning body work.
    context: EffectContext,
}

/// One item of remaining ordered planning work for one engine command.
#[derive(Clone, Debug, PartialEq)]
enum PlanItem {
    /// An executable-content action planned as one effect with its context.
    Effect {
        action: ExecutableContentIr,
        context: EffectContext,
    },
    /// A conditional decision awaiting guard evaluation.
    Decide(Box<DeferredConditional>),
    /// A foreach whose array expression the capability side is evaluating.
    Iterate(Box<ActiveIteration>),
    /// An engine-synthesized effect emitted directly into the next batch.
    Emit {
        effect: Effect,
        context: EffectContext,
    },
    /// An ordered sub-plan planned after the current frame drains.
    Frame(VecDeque<PlanItem>),
}

/// Remaining ordered planning work of one engine command after a deferred decision.
///
/// Frames form a stack: the top frame is the innermost active block. Planning
/// always consumes the top frame and pushes one frame per selected branch, so
/// branch bodies complete before the block that contained the `<if>` continues.
#[derive(Clone, Debug, Default, PartialEq)]
struct PendingPlan {
    frames: Vec<VecDeque<PlanItem>>,
}

/// Outcome of flattening one macrostep's ordered work.
struct MacrostepPlan {
    /// Effects before the first deferred conditional decision.
    effects: Vec<PlannedEffect>,
    /// Remaining work; empty frames mean the macrostep is fully planned.
    plan: PendingPlan,
}

/// The active configuration of one microstep: every member (the union of
/// the active leaves' ancestor chains) as a membership table indexed by state
/// table index, and in document order. Built once per selection or
/// microstep so exit sets and conflicts never rescan the leaf set.
struct ActiveConfiguration {
    /// `member[index]` is whether that state is active.
    member: Vec<bool>,
    /// Active states, root-first in document order.
    ordered: Vec<usize>,
}

/// Outcome of flattening one entry set in entry order.
struct EnteredStates {
    /// Every entered state in W3C entry order.
    order: Vec<usize>,
    /// Entered atomic/final leaves in entry order.
    leaves: Vec<usize>,
    /// States whose late-bound data declarations bind in this entry.
    newly_bound: Vec<usize>,
}

/// Bounded working state for the W3C entry-set computation
/// (`computeEntrySet`). Membership is deduplicated; every list stays within
/// the IR table bounds because it only accumulates distinct state indices.
#[derive(Default)]
struct EntryPlan {
    /// States to enter, in discovery order (sorted to entry order when
    /// flattened).
    states_to_enter: Vec<usize>,
    /// Compound states whose initial-transition content executes during
    /// entry.
    states_for_default_entry: std::collections::BTreeSet<usize>,
    /// Membership guard against duplicate entries.
    entered: std::collections::BTreeSet<usize>,
    /// Every proper ancestor of every state to enter: W3C's "some state to
    /// enter is a descendant of this region" test becomes one lookup. Closed
    /// under ancestors, so marking stops at the first marked ancestor.
    entered_descendant_of: std::collections::BTreeSet<usize>,
    /// Parallel states whose regions were already completed. Completion
    /// leaves every non-history region covered (or, for an atomic region,
    /// entered), and coverage only grows, so completing a parallel again is
    /// a no-op and is skipped.
    completed_parallels: std::collections::BTreeSet<usize>,
    /// Default history content keyed by the history state's parent index
    /// (SCXML 3.11 `defaultHistoryContent`). Populated only when a history
    /// state has no stored value; executed once per entry, after the
    /// parent's onentry and initial content.
    default_history_content: std::collections::BTreeMap<usize, Vec<ExecutableContentIr>>,
}

impl EntryPlan {
    /// Adds one state, deduplicating and enforcing the IR table bound, and
    /// marks its proper ancestors (resolved through `states`) as having a
    /// state to enter below them.
    fn insert(&mut self, index: usize, states: &[StateIr]) -> Result<(), EngineError> {
        if self.entered.insert(index) {
            if scxml_types::exceeds_wasm_bound(
                self.states_to_enter.len().saturating_add(1),
                MAX_NESTED_ITEMS,
            ) {
                return Err(EngineError::BrokenInvariant);
            }
            self.states_to_enter.push(index);
            let mut current = index;
            for _ in 0..=MAX_HIERARCHY_DEPTH {
                count_work(1);
                let Some(parent) = states
                    .get(current)
                    .ok_or(EngineError::BrokenInvariant)?
                    .parent
                else {
                    return Ok(());
                };
                current = usize::try_from(parent).map_err(|_| EngineError::BrokenInvariant)?;
                if !self.entered_descendant_of.insert(current) {
                    return Ok(());
                }
            }
            return Err(EngineError::BrokenInvariant);
        }
        Ok(())
    }

    /// Whether some state to enter is a proper descendant of `state`.
    fn covers(&self, state: usize) -> bool {
        self.entered_descendant_of.contains(&state)
    }

    /// Marks one compound state for default initial content.
    fn insert_default_entry(&mut self, index: usize) -> Result<(), EngineError> {
        if !self.states_for_default_entry.contains(&index) {
            if scxml_types::exceeds_wasm_bound(
                self.states_for_default_entry.len().saturating_add(1),
                MAX_NESTED_ITEMS,
            ) {
                return Err(EngineError::BrokenInvariant);
            }
            self.states_for_default_entry.insert(index);
        }
        Ok(())
    }

    /// Records one history parent's default history content (W3C
    /// `defaultHistoryContent[parent]`); the last recorded wins, matching
    /// the spec's map assignment.
    fn insert_default_history_content(
        &mut self,
        parent: usize,
        actions: Vec<ExecutableContentIr>,
    ) -> Result<(), EngineError> {
        if scxml_types::exceeds_wasm_bound(actions.len(), MAX_NESTED_ITEMS) {
            return Err(EngineError::BrokenInvariant);
        }
        self.default_history_content.insert(parent, actions);
        Ok(())
    }
}

impl Engine {
    /// Allocates a unique executable-content block ordinal (SCXML 3.4/5.7).
    fn alloc_effect_block(&mut self) -> u64 {
        let block = self.next_effect_block;
        self.next_effect_block = self.next_effect_block.saturating_add(1);
        block
    }

    /// Creates a ready engine after validating the canonical IR and supported subset.
    ///
    /// The engine takes ownership of immutable machine tables. No effects occur
    /// until `Self::start`.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::InvalidModel`] for IR that fails
    /// `MachineIr::validate` (including non-orthogonal multi-target sets),
    /// and [`EngineError::UnsupportedModel`] for id-less states,
    /// compound states without children or an initial transition, atomic or
    /// final states with children, parallel states without regions, and
    /// executable content nested deeper than [`MAX_HIERARCHY_DEPTH`].
    pub fn new(machine: MachineIr) -> Result<Self, EngineError> {
        let span = observe(Module::Engine, Operation::EngineNew);
        let result = (|| {
            machine.validate().map_err(|_| EngineError::InvalidModel)?;
            validate_supported_model(&machine)?;
            let mut state_indices = std::collections::BTreeMap::new();
            for (index, state) in machine.states.iter().enumerate() {
                if let Some(id) = &state.id {
                    state_indices.entry(id.clone()).or_insert(index);
                }
            }
            let mut event_index = EventIndex::default();
            let mut has_eventless = false;
            for state in &machine.states {
                if matches!(
                    state.kind,
                    StateKind::ShallowHistory | StateKind::DeepHistory
                ) {
                    continue;
                }
                for &transition in &state.transitions {
                    let transition = usize::try_from(transition)
                        .ok()
                        .and_then(|index| machine.transitions.get(index))
                        .ok_or(EngineError::InvalidModel)?;
                    has_eventless |= transition.events.is_empty();
                    for descriptor in &transition.events {
                        event_index.insert(descriptor);
                    }
                }
            }
            let contracts = contracts::EventContracts::new(&machine);
            Ok(Self {
                machine,
                state_indices,
                event_index,
                has_eventless,
                journal: ConfigurationJournal::default(),
                observed: ObservedConfiguration::default(),
                observed_mark: ConfigurationMark::default(),
                observed_current: false,
                status: EngineStatus::Ready,
                active_states: Vec::new(),
                internal: VecDeque::new(),
                pending_external: VecDeque::new(),
                current_event: None,
                stable: true,
                history_values: std::collections::BTreeMap::new(),
                pending_step: None,
                effect_phase: EffectPhase::Idle,
                armed_plan: None,
                next_effect_block: 0,
                states_to_invoke: std::collections::BTreeSet::new(),
                active_invocations: Vec::new(),
                awaiting_invocations: Vec::new(),
                pending_dispatch: None,
                bound_states: std::collections::BTreeSet::new(),
                issued_blocks: std::collections::BTreeSet::new(),
                suspension: None,
                contracts,
            })
        })();
        complete(&span, &result);
        result
    }

    /// Whether an event `event` this session creates with `data` keeps its
    /// literal `event:schema` contracts: `own`, the creating `<send>`'s or
    /// `<raise>`'s schema, and, when the session delivers the event to
    /// itself (`delivered_here`), the schema of every transition of this
    /// document handling `event` (the `contracts` module). Pure apart
    /// from compiling each schema once.
    pub(crate) fn admits_event_data(
        &mut self,
        event: &str,
        data: &DataValue,
        own: Option<&scxml_compiler_core::EventSchemaIr>,
        delivered_here: bool,
    ) -> bool {
        self.contracts.admits(event, data, own, delivered_here)
    }

    /// Enters the root initial targets' W3C entry set and emits ordered
    /// script/initial/entry effects.
    ///
    /// Entry follows the SCXML algorithm's entry order (ancestors before
    /// descendants, document order between siblings and parallel regions).
    /// Entering a final child of a compound state plans a
    /// `done.state.<parent-id>` raise in entry phase; a parallel grandparent
    /// whose regions are all final plans its own done event (SCXML 3.13).
    ///
    /// # Errors
    ///
    /// Rejects repeated starts or a broken validated-model invariant without
    /// changing engine state.
    pub(crate) fn start(&mut self) -> Result<CommandResult, EngineError> {
        let span = observe(Module::Engine, Operation::EngineStart);
        let result = (|| {
            if self.status != EngineStatus::Ready {
                return Err(EngineError::AlreadyStarted);
            }
            let targets = self
                .machine
                .initial
                .targets
                .iter()
                .map(|&target| usize::try_from(target).map_err(|_| EngineError::BrokenInvariant))
                .collect::<Result<Vec<_>, _>>()?;
            if targets.is_empty() {
                return Err(EngineError::BrokenInvariant);
            }
            self.begin_command_journal()?;
            let entry = self.compute_entry_set(&targets, None, &self.history_values)?;
            let initialization_context = EffectContext {
                phase: EffectExecutionPhase::Initialization,
                configuration: self.journal.mark(),
                block: self.alloc_effect_block(),
            };
            let transition_context = EffectContext {
                phase: EffectExecutionPhase::Transition,
                configuration: self.journal.mark(),
                block: self.alloc_effect_block(),
            };
            let mut items = VecDeque::new();
            flatten_blocks(
                &mut items,
                &self.machine.scripts,
                &initialization_context,
                &mut self.next_effect_block,
            )?;
            flatten_actions(
                &mut items,
                &self.machine.initial.actions,
                &transition_context,
            )?;
            let entered = self.flatten_entry_effects(&mut items, &entry, &[])?;
            let plan = plan_step(items)?;
            self.armed_plan = if plan.plan.frames.is_empty() {
                None
            } else {
                Some(plan.plan)
            };
            // Start replaces the whole leaf set with the initial entry set.
            let mut new_leaves: Vec<usize> = entered.leaves.clone();
            sort_document_order(&mut new_leaves);
            new_leaves.dedup();
            self.active_states = new_leaves
                .iter()
                .map(|&index| self.state_id_by_index(index))
                .collect::<Result<_, _>>()?;
            self.status = if self.enters_top_level_final(&entry.states_to_enter)? {
                EngineStatus::Finished
            } else {
                EngineStatus::Running
            };
            self.commit_entry(&entered)?;
            self.settle_stable()?;
            let entered_ids = self.state_ids(&entered.order)?;
            self.step_with_sets(plan.effects, None, Vec::new(), Vec::new(), entered_ids)
        })();
        complete(&span, &result);
        result
    }

    /// Processes one external event using document-order transition selection.
    ///
    /// Event descriptors use case-sensitive dot-token prefix matching. Guards
    /// are evaluated only after trigger matching and cannot mutate engine state.
    ///
    /// # Errors
    ///
    /// Rejects invalid lifecycle, capability failure, or broken IR without
    /// partially committing a transition.
    pub(crate) fn send(
        &mut self,
        event: &Event,
        evaluator: &mut impl GuardEvaluator,
    ) -> Result<CommandResult, EngineError> {
        let span = observe(Module::Engine, Operation::EngineSend);
        span.event(SignalEvent::ScxmlEventReceived);
        let result = (|| {
            self.require_dispatchable()?;
            self.begin_command_journal()?;
            // SCXML 6.4 / Appendix D `mainEventLoop`: `<finalize>` content of
            // the started invocation that produced this event, and the
            // autoforward copies, run before transition selection.
            // Selection then happens on the next `resume`, ahead of any
            // internal event finalize raised.
            if let Some(step) = self.external_prelude_step(event, Vec::new())? {
                return Ok(step);
            }
            self.dispatch_external(event, evaluator)
        })();
        complete(&span, &result);
        result
    }

    /// Cancels a running session through the W3C `exitInterpreter`
    /// procedure (SCXML 6.4 invocation cancellation, 3.13 exit order).
    ///
    /// Every active state exits in exit order (descendants before
    /// ancestors, reverse document order): its `<onexit>` blocks run in
    /// [`EffectExecutionPhase::Exit`] with the not-yet-exited configuration
    /// observable, followed by one [`Effect::CancelInvoke`] per started
    /// invocation it owns. The machine then finishes with an empty
    /// configuration and without a `done` event; every queued internal and
    /// external event, and every event its exit content raises, is
    /// discarded, and no history is recorded.
    ///
    /// # Errors
    ///
    /// Rejects an open effect phase or pending command result, a session that is not
    /// started, and a finished session, without changing engine state.
    pub(crate) fn cancel(&mut self) -> Result<CommandResult, EngineError> {
        let span = observe(Module::Engine, Operation::EngineCancel);
        let result = (|| {
            match (&self.status, &self.effect_phase, &self.pending_step) {
                (_, EffectPhase::Applying { .. }, _) | (_, _, Some(_)) => {
                    return Err(EngineError::EffectProtocol);
                }
                (EngineStatus::Ready, EffectPhase::Idle, None) => {
                    return Err(EngineError::NotStarted);
                }
                (EngineStatus::Finished, EffectPhase::Idle, None) => {
                    return Err(EngineError::Finished);
                }
                (EngineStatus::Running, EffectPhase::Idle, None) => {}
            }
            self.begin_command_journal()?;
            let mut ordered = self
                .configuration_indices_all()?
                .into_iter()
                .map(|index| Ok((self.state_by_index(index)?.document_order, index)))
                .collect::<Result<Vec<_>, EngineError>>()?;
            ordered.sort_unstable_by_key(|&(order, _)| std::cmp::Reverse(order));
            let exits: Vec<usize> = ordered.into_iter().map(|(_, index)| index).collect();
            let mut items = VecDeque::new();
            let mut block_counter = self.next_effect_block;
            let mut cancelled: Vec<usize> = Vec::new();
            for &exited in &exits {
                flatten_blocks(
                    &mut items,
                    &self.state_by_index(exited)?.onexit,
                    &EffectContext {
                        phase: EffectExecutionPhase::Exit,
                        configuration: self.journal.mark(),
                        block: block_counter,
                    },
                    &mut block_counter,
                )?;
                let cancel_block = block_counter;
                block_counter = block_counter.saturating_add(1);
                self.push_cancellations(
                    &mut items,
                    |state| state == exited,
                    &mut cancelled,
                    &EffectContext {
                        phase: EffectExecutionPhase::Exit,
                        configuration: self.journal.mark(),
                        block: cancel_block,
                    },
                )?;
                self.journal.exit(exited)?;
            }
            let plan = plan_step(items)?;
            let exited_ids = self.state_ids(&exits)?;
            // Commit only after planning succeeded: the session is over.
            self.next_effect_block = block_counter;
            self.armed_plan = if plan.plan.frames.is_empty() {
                None
            } else {
                Some(plan.plan)
            };
            self.active_states.clear();
            self.active_invocations.clear();
            self.states_to_invoke.clear();
            self.internal.clear();
            self.pending_external.clear();
            self.pending_dispatch = None;
            self.status = EngineStatus::Finished;
            self.stable = true;
            let trigger = self.current_event.clone();
            self.step_with_sets(plan.effects, trigger, Vec::new(), exited_ids, Vec::new())
        })();
        complete(&span, &result);
        result
    }

    /// Queues one platform error event (SCXML 3.12/5.5/6.2) without
    /// aborting any executable-content block.
    ///
    /// Used for failures whose surrounding content keeps executing, such as
    /// a late-bound data initializer (SCXML 5.3), a `<donedata>` payload
    /// (SCXML 5.5.2), or a failed invocation start (SCXML 6.4).
    /// `trace_context` is the carrier of the effect batch that failed; it
    /// rides on the error event and never changes semantics.
    ///
    /// # Errors
    ///
    /// Rejects a machine that is not started, a finished machine, and queue
    /// overflow. A finished machine has no surviving block to recover, so a
    /// late error is surfaced instead of silently extending a final
    /// configuration.
    pub fn queue_platform_error(
        &mut self,
        error: PlatformError,
        trace_context: Option<TraceContext>,
    ) -> Result<(), EngineError> {
        let span = observe(Module::Engine, Operation::EngineQueuePlatformError);
        let result = (|| {
            match self.status {
                EngineStatus::Ready => return Err(EngineError::NotStarted),
                EngineStatus::Finished => return Err(EngineError::Finished),
                EngineStatus::Running => {}
            }
            self.push_platform_error(error, None, trace_context)
        })();
        complete(&span, &result);
        result
    }

    /// Places the asynchronous `error.communication` of one of this
    /// session's `<send>`s on the internal event queue: a failure the
    /// platform learns of only after that `<send>` completed (a delayed send
    /// whose target no longer answers when it falls due, or a `BasicHTTP`
    /// request that fails after it was accepted), or of an invoked service
    /// that failed after its start (WIT `invocation-message`
    /// `communication-error`: its future resolved `err`, it trapped, or it
    /// returned an invalid event). SCXML 3.12.2: the processor "MUST place
    /// these events in the internal event queue and MUST process them like
    /// any other event"; 6.2.4: "If it is unable to dispatch the message,
    /// the Processor MUST place the error error.communication on the
    /// internal event queue." The event is a platform event whose `sendid`
    /// is `send_id` and whose `invokeid` is `invoke_id` (5.10.1). No
    /// liveness check: the message of an invocation already cancelled is
    /// still delivered, as its events are.
    ///
    /// Appendix D mapping: `mainEventLoop` blocks on the external queue only
    /// once the macrostep and its internal-queue drain have finished. An
    /// error that arrives while the session waits there is placed on the
    /// internal queue and the next [`Command::Resume`] runs the macrostep
    /// that drains it (the eventless-then-internal loop of `mainEventLoop`),
    /// dequeuing no external event; one that arrives while a call stopped at
    /// its command bound joins the internal queue behind the events already
    /// there and is drained by the same resumed macrostep.
    ///
    /// # Errors
    ///
    /// Rejects a machine that is not started, a finished machine, and queue
    /// overflow.
    pub fn queue_communication_error(
        &mut self,
        send_id: Option<scxml_types::SendId>,
        invoke_id: Option<InvokeId>,
        trace_context: Option<TraceContext>,
    ) -> Result<(), EngineError> {
        let span = observe(Module::Engine, Operation::EngineQueuePlatformError);
        let result = (|| {
            match self.status {
                EngineStatus::Ready => return Err(EngineError::NotStarted),
                EngineStatus::Finished => return Err(EngineError::Finished),
                EngineStatus::Running => {}
            }
            self.push_platform_error(PlatformError::Communication, send_id, trace_context)?;
            if invoke_id.is_some()
                && let Some(event) = self.internal.back_mut()
            {
                event.invoke_id = invoke_id;
            }
            Ok(())
        })();
        complete(&span, &result);
        result
    }

    /// The carrier of the `error.execution` a failed transition or
    /// `<if>`/`<elseif>` condition queues (SCXML 5.9): the carrier of the
    /// effect batch the evaluator last opened in this run call (the batch
    /// whose effects the condition observed; for a deferred `<if>`, the
    /// batch that ran the content before it), else the carrier of the event
    /// under evaluation. Selection runs before its transaction opens a
    /// batch, so the error cannot carry that batch's own carrier.
    fn condition_error_trace_context(
        evaluator: &impl GuardEvaluator,
        current_event: Option<&Event>,
    ) -> Option<TraceContext> {
        evaluator
            .last_batch_trace_context()
            .or_else(|| current_event.and_then(|event| event.trace_context.as_ref()))
            .cloned()
    }

    /// Queues one platform error event and marks the machine not stable.
    /// `send_id` is the failing `<send>`'s id, carried as `_event.sendid`
    /// (SCXML 5.10.1); `trace_context` is the failing batch's carrier.
    fn push_platform_error(
        &mut self,
        error: PlatformError,
        send_id: Option<scxml_types::SendId>,
        trace_context: Option<TraceContext>,
    ) -> Result<(), EngineError> {
        if scxml_types::exceeds_wasm_bound(self.internal.len().saturating_add(1), MAX_NESTED_ITEMS)
        {
            return Err(EngineError::QueueLimit);
        }
        // SCXML 3.12/5.5: processor-generated error events are platform
        // events; `_event.type` must report "platform" (case 331).
        let name = EventName::new(error.event_name().to_owned())
            .map_err(|_| EngineError::BrokenInvariant)?;
        let mut event = Event::platform(name);
        event.send_id = send_id;
        event.trace_context = trace_context;
        self.internal.push_back(event);
        self.stable = false;
        Ok(())
    }

    /// Queues one platform error and aborts one executable-content block.
    ///
    /// SCXML 3.4/5.7 with 5.10 and 6.2: each `<onentry>`/`<onexit>` handler
    /// and each transition's content is an independent block, so a
    /// recoverable failure aborts exactly its own block. The failed block's
    /// pending raises and its remaining deferred planning work (`<if>`
    /// branches and `<foreach>` iterations) are dropped; every other block's
    /// raises and plan stay armed and execute. Effects applied before the
    /// failure stay committed. [`Engine::run`] skips the failed block's
    /// remaining effects; hosts never choose recovery semantics. The error
    /// event carries `trace_context`, the failing batch's carrier.
    ///
    /// # Errors
    ///
    /// Rejects calls outside an applying effect phase, a block ordinal that
    /// was not issued in the current phase ([`EngineError::UnknownBlock`]),
    /// and queue overflow, without dropping the plan.
    fn block_failed(
        &mut self,
        error: PlatformError,
        send_id: Option<scxml_types::SendId>,
        failed_block: u64,
        trace_context: Option<TraceContext>,
    ) -> Result<(), EngineError> {
        let span = observe(Module::Engine, Operation::EngineBlockFailed);
        let result = (|| {
            if !matches!(self.effect_phase, EffectPhase::Applying { .. }) {
                return Err(EngineError::EffectProtocol);
            }
            if !self.issued_blocks.contains(&failed_block) {
                return Err(EngineError::UnknownBlock);
            }
            self.push_platform_error(error, send_id, trace_context)?;
            if let EffectPhase::Applying {
                pending_raises,
                pending_plan,
                failed_blocks,
                ..
            } = &mut self.effect_phase
            {
                pending_raises.retain(|(_, block)| *block != failed_block);
                if let Some(plan) = pending_plan.as_mut() {
                    for frame in &mut plan.frames {
                        retain_outside_block(frame, failed_block);
                    }
                    if plan.frames.iter().all(VecDeque::is_empty) {
                        *pending_plan = None;
                    }
                }
                failed_blocks.insert(failed_block);
            } else {
                return Err(EngineError::BrokenInvariant);
            }
            Ok(())
        })();
        complete(&span, &result);
        result
    }

    /// Whether a recoverable error aborted `block` in the current applying
    /// phase, so its remaining effects must be skipped.
    fn block_is_failed(&self, block: u64) -> bool {
        matches!(
            &self.effect_phase,
            EffectPhase::Applying { failed_blocks, .. } if failed_blocks.contains(&block)
        )
    }

    /// Records that the capability side started one invocation emitted as
    /// [`Effect::Invoke`] in the current applying phase (SCXML 6.4).
    ///
    /// The engine then owns the invocation's lifecycle: events carrying
    /// `invoke_id` run the declaration's `<finalize>` content before
    /// transition selection, and exiting the invoking state emits
    /// [`Effect::CancelInvoke`].
    ///
    /// # Errors
    ///
    /// Rejects calls outside an applying phase, a pair that was not emitted
    /// in this phase or was already reported, a duplicate live invoke id,
    /// and more than [`MAX_NESTED_ITEMS`] live invocations.
    pub fn invocation_started(
        &mut self,
        state: &StateId,
        invoke_order: u32,
        invoke_id: InvokeId,
        type_uri: String,
    ) -> Result<(), EngineError> {
        if !matches!(self.effect_phase, EffectPhase::Applying { .. }) {
            return Err(EngineError::EffectProtocol);
        }
        let state_index = self.state_index_by_id(state)?;
        let Some(position) = self
            .awaiting_invocations
            .iter()
            .position(|pair| *pair == (state_index, invoke_order))
        else {
            return Err(EngineError::EffectProtocol);
        };
        if self
            .active_invocations
            .iter()
            .any(|active| active.invoke_id == invoke_id)
        {
            return Err(EngineError::EffectProtocol);
        }
        if scxml_types::exceeds_wasm_bound(
            self.active_invocations.len().saturating_add(1),
            MAX_NESTED_ITEMS,
        ) {
            return Err(EngineError::QueueLimit);
        }
        self.awaiting_invocations.remove(position);
        self.active_invocations.push(ActiveInvocation {
            state: state_index,
            invoke_order,
            invoke_id,
            type_uri,
        });
        Ok(())
    }

    /// Queues one internal event before the machine starts.
    ///
    /// The startup path uses this to deliver `error.execution` for data
    /// initializer failures before any user event is processed.
    ///
    /// # Errors
    ///
    /// Rejects calls after start and queue overflow.
    pub fn queue_startup_error(&mut self) -> Result<(), EngineError> {
        if self.status != EngineStatus::Ready {
            return Err(EngineError::EffectProtocol);
        }
        if scxml_types::exceeds_wasm_bound(self.internal.len().saturating_add(1), MAX_NESTED_ITEMS)
        {
            return Err(EngineError::QueueLimit);
        }
        // SCXML 3.12/5.5: processor-generated error events are platform
        // events; `_event.type` must report "platform" (case 331).
        let event = Event::platform(
            EventName::new("error.execution".to_owned())
                .map_err(|_| EngineError::BrokenInvariant)?,
        );
        self.internal.push_back(event);
        Ok(())
    }

    /// Queues one internal event and marks the machine not stable.
    ///
    /// # Errors
    ///
    /// Rejects events before start, after finish outside an active effect phase,
    /// out-of-order engine-emitted raises, or queues above the IR nested-item bound.
    pub fn raise(&mut self, event: Event) -> Result<(), EngineError> {
        if self.pending_step.is_some() {
            return Err(EngineError::EffectProtocol);
        }
        if self.status == EngineStatus::Ready {
            return Err(EngineError::NotStarted);
        }
        if self.status == EngineStatus::Finished
            && !matches!(self.effect_phase, EffectPhase::Applying { .. })
        {
            return Err(EngineError::Finished);
        }
        if let EffectPhase::Applying { pending_raises, .. } = &mut self.effect_phase {
            match pending_raises.front() {
                Some((expected, _)) if expected == event.name.as_str() => {
                    pending_raises.pop_front();
                }
                _ => return Err(EngineError::EffectProtocol),
            }
        }
        if scxml_types::exceeds_wasm_bound(self.internal.len().saturating_add(1), MAX_NESTED_ITEMS)
        {
            return Err(EngineError::QueueLimit);
        }
        self.internal.push_back(event);
        self.stable = false;
        Ok(())
    }

    /// Queues one internal event generated by a `<send>` effect.
    ///
    /// Unlike [`Self::raise`], this accepts arbitrary engine-generated events
    /// without a matching planned-raise entry, because `<send target="#_internal">`
    /// produces events the capability side materializes at effect time.
    /// Bounded by the IR nested-item queue limit.
    ///
    /// # Errors
    ///
    /// Rejects calls outside an applying effect phase and queue overflow.
    pub fn enqueue_internal(&mut self, event: Event) -> Result<(), EngineError> {
        if !matches!(self.effect_phase, EffectPhase::Applying { .. }) {
            return Err(EngineError::EffectProtocol);
        }
        if scxml_types::exceeds_wasm_bound(self.internal.len().saturating_add(1), MAX_NESTED_ITEMS)
        {
            return Err(EngineError::QueueLimit);
        }
        self.internal.push_back(event);
        self.stable = false;
        Ok(())
    }

    /// Queues one self-addressed external event generated by a `<send>` effect.
    ///
    /// The engine drains this queue after the internal queue empties, so the
    /// current macrostep completes before externally addressed delivery.
    /// Bounded by the IR nested-item queue limit.
    ///
    /// # Errors
    ///
    /// Rejects calls outside an applying effect phase and queue overflow.
    pub fn enqueue_external(&mut self, event: Event) -> Result<(), EngineError> {
        if !matches!(self.effect_phase, EffectPhase::Applying { .. }) {
            return Err(EngineError::EffectProtocol);
        }
        if scxml_types::exceeds_wasm_bound(
            self.pending_external.len().saturating_add(1),
            MAX_NESTED_ITEMS,
        ) {
            return Err(EngineError::QueueLimit);
        }
        self.pending_external.push_back(event);
        self.stable = false;
        Ok(())
    }

    /// Starts the capability-side application phase for one engine-produced step.
    ///
    /// The caller must apply [`CommandResult::planned_effects`] in document order, install
    /// each effect's phase-specific configuration for data-model queries, and route
    /// every [`Effect::Raise`] back through [`Self::raise`]. The flattened
    /// [`CommandResult::effects`] projection must remain unchanged.
    ///
    /// # Errors
    ///
    /// Rejects nested phases, stale or foreign command results, and an excessive raise count.
    pub(crate) fn begin_effects(&mut self, step: &CommandResult) -> Result<(), EngineError> {
        if !matches!(self.effect_phase, EffectPhase::Idle)
            || self.pending_step.as_ref() != Some(step)
        {
            return Err(EngineError::EffectProtocol);
        }
        // A done event with `<donedata>` rides the pending-raise
        // protocol: the `DoneEvent` effect delivers the same event
        // name the bare final entry would have raised (SCXML 5.5.2/6.5).
        let pending_raises = step
            .planned_effects
            .iter()
            .filter_map(|planned| match &planned.effect {
                Effect::Raise(name, ..) => Some((name.clone(), planned.context.block)),
                Effect::DoneEvent { event, .. } => Some((event.clone(), planned.context.block)),
                _ => None,
            })
            .collect::<VecDeque<_>>();
        if scxml_types::exceeds_wasm_bound(pending_raises.len(), MAX_NESTED_ITEMS) {
            return Err(EngineError::EffectLimit);
        }
        self.issued_blocks = step
            .planned_effects
            .iter()
            .map(|planned| planned.context.block)
            .collect();
        self.pending_step = None;
        self.effect_phase = EffectPhase::Applying {
            pending_raises,
            planned_effects: step.planned_effects.len(),
            pending_plan: self.armed_plan.take(),
            failed_blocks: std::collections::BTreeSet::new(),
        };
        Ok(())
    }

    /// Commits a successfully applied engine effect phase.
    ///
    /// Top-level finality becomes externally stable only at this boundary. Internal
    /// events raised during final-state entry are deterministically discarded because
    /// a top-level final configuration performs no further macrosteps.
    ///
    /// # Errors
    ///
    /// Rejects completion without a phase or before all emitted raises were applied.
    pub(crate) fn complete_effects(&mut self) -> Result<(), EngineError> {
        match &self.effect_phase {
            EffectPhase::Applying {
                pending_raises,
                pending_plan,
                ..
            } if pending_raises.is_empty() && pending_plan.is_none() => {}
            EffectPhase::Applying { .. } | EffectPhase::Idle => {
                return Err(EngineError::EffectProtocol);
            }
        }
        self.effect_phase = EffectPhase::Idle;
        self.issued_blocks.clear();
        // Emitted invocations that were not reported started failed to
        // start (SCXML 6.4) and are never cancelled.
        self.awaiting_invocations.clear();
        if self.status == EngineStatus::Finished {
            self.internal.clear();
            self.pending_external.clear();
            self.states_to_invoke.clear();
            self.pending_dispatch = None;
            self.stable = true;
        }
        if self.stable {
            self.current_event = None;
        }
        Ok(())
    }

    /// Reports the capability-side shallow element snapshot of one active
    /// `<foreach>` array expression.
    ///
    /// The engine expands the loop: each subsequent effect batch assigns the
    /// next element to the item location (and its index, `first_index` plus
    /// the element's zero-based ordinal, to the optional index location)
    /// before planning the body's ordered work. SCXML 4.6 leaves the index
    /// base to the data model: ECMAScript counts from 0, the `XPath` data
    /// model from 1 (SCXML 1.0 CR 2014-03-13 §C.3.10). The snapshot is
    /// immutable for the whole loop, so body effects that mutate the array
    /// never change iteration behavior.
    ///
    /// # Errors
    ///
    /// Rejects calls outside an applying effect phase, a non-empty snapshot,
    /// and expansions above the per-command effect bound without committing.
    /// The expansion counts, per element, the item and optional index
    /// assignments plus every body action that plans one effect directly
    /// (every action except `<if>`).
    pub fn begin_iteration(
        &mut self,
        elements: &[DataValue],
        first_index: u32,
    ) -> Result<(), EngineError> {
        let EffectPhase::Applying {
            planned_effects,
            pending_plan: Some(plan),
            ..
        } = &mut self.effect_phase
        else {
            return Err(EngineError::EffectProtocol);
        };
        let Some(frame) = plan.frames.last_mut() else {
            return Err(EngineError::EffectProtocol);
        };
        let Some(PlanItem::Iterate(iteration)) = frame.front() else {
            return Err(EngineError::EffectProtocol);
        };
        if !iteration.elements.is_empty() {
            return Err(EngineError::EffectProtocol);
        }
        // Pre-flight accounting: every element plans its item (and
        // optional index) assignment plus one effect per body action
        // that is not an `<if>` (whose branch effects are counted when
        // the branch is planned). Rejecting here keeps an over-budget
        // loop from committing early iterations before a later batch
        // would hit the bound.
        let per_element = 1_usize
            .saturating_add(usize::from(iteration.index_location.is_some()))
            .saturating_add(direct_effect_count(&iteration.body));
        let planned = planned_effects.saturating_add(elements.len().saturating_mul(per_element));
        if scxml_types::exceeds_wasm_bound(planned.saturating_add(1), MAX_EFFECTS_PER_COMMAND) {
            return Err(EngineError::EffectLimit);
        }
        let Some(PlanItem::Iterate(iteration)) = frame.pop_front() else {
            return Err(EngineError::BrokenInvariant);
        };
        *planned_effects = planned;
        // Expand newest-first: push_front is LIFO, so push the body
        // frame first, then index, then item, yielding the execution
        // order item → index → body per iteration.
        let count = elements.len();
        for ordinal in (0..count).rev() {
            let value = elements[ordinal].clone();
            let mut body_frame = VecDeque::new();
            for action in iteration.body.iter().cloned() {
                body_frame.push_back(PlanItem::Effect {
                    action,
                    context: iteration.context.clone(),
                });
            }
            frame.push_front(PlanItem::Frame(body_frame));
            if let Some(index_location) = &iteration.index_location {
                let ordinal_number = f64::from(
                    u32::try_from(ordinal)
                        .ok()
                        .and_then(|ordinal| ordinal.checked_add(first_index))
                        .ok_or(EngineError::BrokenInvariant)?,
                );
                let index_value = FiniteNumber::new(ordinal_number)
                    .map(DataValue::Number)
                    .map_err(|_| EngineError::BrokenInvariant)?;
                frame.push_front(PlanItem::Emit {
                    effect: Effect::AssignValue {
                        location: index_location.clone(),
                        value: index_value,
                    },
                    context: iteration.context.clone(),
                });
            }
            frame.push_front(PlanItem::Emit {
                effect: Effect::AssignValue {
                    location: iteration.item_location.clone(),
                    value,
                },
                context: iteration.context.clone(),
            });
        }
        Ok(())
    }

    /// Returns the effect context whose configuration data-model queries
    /// must observe during the next deferred conditional evaluation.
    ///
    /// The context belongs to the `<if>` element awaiting branch selection;
    /// [`Engine::run`] updates the capability side to its configuration so
    /// `In()` queries inside the condition observe the configuration
    /// required by SCXML execution ordering.
    #[must_use]
    pub(crate) fn pending_condition_context(&self) -> Option<EffectContext> {
        match &self.effect_phase {
            EffectPhase::Applying {
                pending_plan: Some(plan),
                ..
            } => plan.frames.last().and_then(|frame| match frame.front() {
                Some(PlanItem::Decide(deferred)) => Some(deferred.context.clone()),
                _ => None,
            }),
            _ => None,
        }
    }

    /// Plans the next ordered effect batch of the applying command.
    ///
    /// Called after the preceding batch's effects have been applied. Each call
    /// resolves at most one deferred `<if>` decision through the borrowed guard
    /// capability and returns the next ordered batch, or `None` when no
    /// conditional planning work remains. The engine owns branch selection;
    /// capability sides never select branches themselves.
    ///
    /// # Errors
    ///
    /// Rejects calls outside an applying effect phase, guard-evaluation failures,
    /// and cumulative effect or raise bound violations without committing the
    /// remaining plan.
    #[allow(
        clippy::too_many_lines,
        reason = "one function owns the ordered deferred-decision and iteration batch planning of the W3C algorithm"
    )]
    pub(crate) fn continue_effects(
        &mut self,
        evaluator: &mut impl GuardEvaluator,
    ) -> Result<Option<CommandResult>, EngineError> {
        let EffectPhase::Applying {
            pending_raises,
            planned_effects,
            pending_plan,
            ..
        } = &mut self.effect_phase
        else {
            return Err(EngineError::EffectProtocol);
        };
        let Some(mut plan) = pending_plan.take() else {
            return Ok(None);
        };
        let current_event = self.current_event.clone();
        let mut batch: Vec<PlannedEffect> = Vec::new();
        let mut failure = None;
        loop {
            let front_is_decide = plan
                .frames
                .last()
                .is_some_and(|frame| matches!(frame.front(), Some(PlanItem::Decide(_))));
            if front_is_decide {
                let Some(mut deferred) =
                    plan.frames
                        .last_mut()
                        .and_then(|frame| match frame.front() {
                            Some(PlanItem::Decide(deferred)) => Some(deferred.clone()),
                            _ => None,
                        })
                else {
                    break;
                };
                let taken = match evaluator
                    .evaluate_condition(&deferred.condition, current_event.as_ref())
                {
                    Ok(taken) => taken,
                    // SCXML 5.9 and the WIT `evaluate-condition` contract:
                    // a clause whose cond is rejected or fails evaluation
                    // queues error.execution and is treated as false, so
                    // later `<elseif>`/`<else>` clauses and the rest of the
                    // block still run. Unavailable or failed evaluators
                    // still fail closed with the decision kept pending.
                    Err(EvaluationError::InvalidInput | EvaluationError::Execution) => {
                        if scxml_types::exceeds_wasm_bound(
                            self.internal.len().saturating_add(1),
                            MAX_NESTED_ITEMS,
                        ) {
                            failure = Some(EngineError::QueueLimit);
                            break;
                        }
                        let name = EventName::new(PlatformError::Execution.event_name().to_owned())
                            .map_err(|_| EngineError::BrokenInvariant)?;
                        let mut event = Event::platform(name);
                        event.trace_context =
                            Self::condition_error_trace_context(evaluator, current_event.as_ref());
                        self.internal.push_back(event);
                        self.stable = false;
                        false
                    }
                    Err(error) => {
                        failure = Some(EngineError::Evaluation(error));
                        break;
                    }
                };
                if taken {
                    plan.frames.last_mut().and_then(VecDeque::pop_front);
                    let mut branch = VecDeque::new();
                    if let Err(error) =
                        flatten_actions(&mut branch, &deferred.branch, &deferred.context)
                    {
                        failure = Some(error);
                        break;
                    }
                    plan.frames.push(branch);
                } else {
                    plan.frames.last_mut().and_then(VecDeque::pop_front);
                    if let Some(next) = deferred.else_if.pop_front() {
                        plan.frames
                            .last_mut()
                            .ok_or(EngineError::BrokenInvariant)?
                            .push_front(PlanItem::Decide(Box::new(DeferredConditional {
                                condition: next.cond,
                                branch: next.actions,
                                else_if: deferred.else_if,
                                else_actions: deferred.else_actions,
                                context: deferred.context,
                            })));
                        // The next condition is evaluated only after this
                        // batch has been applied.
                        break;
                    }
                    if let Some(actions) = deferred.else_actions {
                        let mut else_frame = VecDeque::new();
                        if let Err(error) =
                            flatten_actions(&mut else_frame, &actions, &deferred.context)
                        {
                            failure = Some(error);
                            break;
                        }
                        plan.frames.push(else_frame);
                    }
                }
                continue;
            }
            let item = plan.frames.last_mut().and_then(VecDeque::pop_front);
            match item {
                None => {
                    plan.frames.pop();
                    if plan.frames.is_empty() {
                        break;
                    }
                }
                // Decisions and iteration fronts stop batch planning so the
                // capability side resolves them between batches.
                Some(PlanItem::Decide(_) | PlanItem::Iterate(_)) => break,
                // A popped sub-frame (foreach body batch) becomes the
                // active frame; its ordered effects are planned next.
                Some(PlanItem::Frame(sub_frame)) => {
                    plan.frames.push(sub_frame);
                }
                Some(PlanItem::Emit { effect, context }) => {
                    if scxml_types::exceeds_wasm_bound(
                        planned_effects
                            .saturating_add(batch.len())
                            .saturating_add(1),
                        MAX_EFFECTS_PER_COMMAND,
                    ) {
                        failure = Some(EngineError::EffectLimit);
                        break;
                    }
                    batch.push(PlannedEffect { effect, context });
                }
                Some(PlanItem::Effect { action, context }) => {
                    if let ExecutableContentIr::If {
                        cond: condition,
                        actions,
                        elseif: else_if,
                        else_actions,
                    } = action
                    {
                        plan.frames
                            .last_mut()
                            .ok_or(EngineError::BrokenInvariant)?
                            .push_front(PlanItem::Decide(Box::new(DeferredConditional {
                                condition,
                                branch: actions,
                                else_if: else_if.into(),
                                else_actions,
                                context,
                            })));
                        break;
                    }
                    if scxml_types::exceeds_wasm_bound(
                        planned_effects
                            .saturating_add(batch.len())
                            .saturating_add(1),
                        MAX_EFFECTS_PER_COMMAND,
                    ) {
                        failure = Some(EngineError::EffectLimit);
                        break;
                    }
                    // A `<foreach>` nested in a loop body or a branch
                    // opens its own iteration exactly as at plan time:
                    // its `Iterate` effect ends the batch and the engine
                    // expands it once the capability side reports the
                    // array snapshot through `begin_iteration`.
                    if let ExecutableContentIr::Foreach {
                        array,
                        item,
                        index,
                        actions,
                    } = action
                    {
                        batch.push(PlannedEffect {
                            effect: Effect::Iterate {
                                array_expression: array,
                                item_location: item.clone(),
                                index_location: index.clone(),
                            },
                            context: context.clone(),
                        });
                        plan.frames
                            .last_mut()
                            .ok_or(EngineError::BrokenInvariant)?
                            .push_front(PlanItem::Iterate(Box::new(ActiveIteration {
                                item_location: item,
                                index_location: index,
                                elements: VecDeque::new(),
                                next_index: 0,
                                body: actions,
                                context,
                            })));
                        break;
                    }
                    match action_effect(action) {
                        Ok(Some(effect)) => batch.push(PlannedEffect { effect, context }),
                        Ok(None) => {
                            failure = Some(EngineError::UnsupportedModel);
                            break;
                        }
                        Err(error) => {
                            failure = Some(error);
                            break;
                        }
                    }
                }
            }
        }
        if let Some(error) = failure {
            *pending_plan = Some(plan);
            return Err(error);
        }
        for planned in &batch {
            if scxml_types::exceeds_wasm_bound(
                self.issued_blocks.len().saturating_add(1),
                MAX_EFFECTS_PER_COMMAND,
            ) {
                *pending_plan = Some(plan);
                return Err(EngineError::EffectLimit);
            }
            self.issued_blocks.insert(planned.context.block);
            if let Effect::Raise(name, ..) = &planned.effect {
                if scxml_types::exceeds_wasm_bound(
                    pending_raises.len().saturating_add(1),
                    MAX_NESTED_ITEMS,
                ) {
                    *pending_plan = Some(plan);
                    return Err(EngineError::EffectLimit);
                }
                pending_raises.push_back((name.clone(), planned.context.block));
            }
        }
        *planned_effects = planned_effects.saturating_add(batch.len());
        *pending_plan = if plan.frames.is_empty() {
            None
        } else {
            Some(plan)
        };
        // Continuation batches never become pending command results: the
        // effect phase already owns this transaction, and raises routed
        // back through [`Self::raise`] require `pending_step` to stay empty.
        let effects = batch.iter().map(|planned| planned.effect.clone()).collect();
        Ok(Some(CommandResult {
            status: self.status.clone(),
            leaves: self.active_states.clone(),
            effects,
            planned_effects: batch,
            trigger_event: current_event,
            event_dispositions: Vec::new(),
            exited_states: Vec::new(),
            entered_states: Vec::new(),
            configuration: self.configuration_ids_all()?,
            stable: self.stable,
        }))
    }

    /// Processes one eventless transition or one queued internal event.
    ///
    /// Eventless selection has priority. If no transition is enabled and no
    /// internal event remains, the machine becomes stable.
    ///
    /// # Errors
    ///
    /// Returns explicit lifecycle, capability, model, queue, or effect failures.
    pub(crate) fn resume(
        &mut self,
        evaluator: &mut impl GuardEvaluator,
    ) -> Result<CommandResult, EngineError> {
        let span = observe(Module::Engine, Operation::EngineResume);
        let result = (|| {
            if !matches!(self.effect_phase, EffectPhase::Idle) || self.pending_step.is_some() {
                return Err(EngineError::EffectProtocol);
            }
            if self.status == EngineStatus::Ready {
                return Err(EngineError::NotStarted);
            }
            if self.status == EngineStatus::Finished {
                return Err(EngineError::Finished);
            }
            self.begin_command_journal()?;
            // An external event whose `<finalize>` content already ran is
            // selected before any internal event that content raised
            // (SCXML 6.4; PAT-RTC-001).
            if let Some(event) = self.pending_dispatch.take() {
                let dispatched = self.dispatch_external(&event, evaluator);
                if dispatched.is_err() {
                    self.pending_dispatch = Some(event);
                }
                return dispatched;
            }
            if self.stable {
                return self.step(Vec::new(), None, Vec::new());
            }
            let current_event = self.current_event.clone();
            // Queue length before any selection of this resume: a failed
            // scan restores the internal queue to exactly this state.
            let (queue_len, stable) = (self.internal.len(), self.stable);
            let transitions = self.select_transitions(None, current_event.as_ref(), evaluator)?;
            if !transitions.is_empty() {
                return self.with_selection_rollback(queue_len, stable, |engine| {
                    engine.apply_microstep(&transitions, current_event.as_ref(), Vec::new())
                });
            }
            let mut dispositions =
                match self.scan_internal_queue(queue_len, current_event, evaluator)? {
                    InternalScan::CommandResult(step) => return Ok(*step),
                    InternalScan::Drained(dispositions) => dispositions,
                };
            // The macrostep ended: start the invocations of every state
            // entered and not exited during it before any external event is
            // processed (SCXML 6.4 / Appendix D `mainEventLoop`).
            if self.status == EngineStatus::Running && !self.states_to_invoke.is_empty() {
                return self.invoke_step(dispositions);
            }
            // Self-addressed external `<send>` events process only after the
            // internal queue empties (macrostep ordering). Eventless
            // transitions were already considered above.
            // The event stays queued until selection succeeds and is
            // restored if the microstep fails, so a failed command never
            // drops it.
            if let Some(event) = self.pending_external.front().cloned() {
                // Appendix D: every dequeued external event runs its
                // `<finalize>`/autoforward prelude before selection; the
                // event then dispatches on the next resume.
                if let Some(event) = self.pending_external.pop_front() {
                    match self.external_prelude_step(&event, dispositions.clone()) {
                        Ok(Some(step)) => return Ok(step),
                        Ok(None) => self.pending_external.push_front(event),
                        Err(error) => {
                            self.pending_external.push_front(event);
                            return Err(error);
                        }
                    }
                }
                let (queue_len, stable) = (self.internal.len(), self.stable);
                let transitions = self.select_transitions(Some(&event), Some(&event), evaluator)?;
                self.pending_external.pop_front();
                if !transitions.is_empty() {
                    dispositions.push(DisposedEvent {
                        event: event.clone(),
                        disposition: EventDisposition::Selected,
                    });
                    let applied = self.with_selection_rollback(queue_len, stable, |engine| {
                        engine.apply_microstep(&transitions, Some(&event), dispositions)
                    });
                    if applied.is_err() {
                        self.pending_external.push_front(event);
                    }
                    return applied;
                }
                dispositions.push(DisposedEvent {
                    event: event.clone(),
                    disposition: EventDisposition::Ignored,
                });
                // Appendix D: the ignored external event stays `_event` for
                // the eventless selection that starts the next macrostep.
                self.current_event = Some(event);
                self.settle_stable()?;
                if self.stable {
                    self.current_event = None;
                }
                return self.step(Vec::new(), None, dispositions);
            }
            // Appendix D: no eventless transition is enabled and the internal
            // queue is empty, so the macrostep is done; with no invocation or
            // external work left the machine waits for an external event.
            // Stability is decided by enabled transitions here, never by the
            // mere presence of a guarded eventless transition.
            self.stable = self.internal.is_empty()
                && self.pending_external.is_empty()
                && self.pending_dispatch.is_none()
                && self.states_to_invoke.is_empty();
            if self.stable {
                self.current_event = None;
            }
            self.step(Vec::new(), None, dispositions)
        })();
        complete(&span, &result);
        result
    }

    /// Internal-queue scan of [`Self::resume`] (W3C Appendix D
    /// `mainEventLoop`), entered with no enabled eventless transition.
    ///
    /// # Errors
    ///
    /// Returns selection or microstep failures after restoring the internal
    /// queue to the `queue_len` events it held before this resume's first
    /// selection (dropping every error event this resume appended) and
    /// restoring `_event`.
    fn scan_internal_queue(
        &mut self,
        queue_len: usize,
        current_event: Option<Event>,
        evaluator: &mut impl GuardEvaluator,
    ) -> Result<InternalScan, EngineError> {
        // W3C Appendix D `mainEventLoop`: with no enabled eventless
        // transition, dequeue one internal event, bind it as `_event`,
        // and select on it. When it enables nothing, eventless selection
        // runs again with `_event` still bound before the next dequeue.
        // On failure the consumed prefix and `_event` are restored.
        let mut dispositions = Vec::new();
        let mut consumed = Vec::new();
        while let Some(event) = self.internal.front().cloned() {
            if scxml_types::exceeds_wasm_bound(consumed.len().saturating_add(1), MAX_NESTED_ITEMS) {
                // Bounded scan: the next resume continues from here with
                // `_event` bound to the last ignored event.
                return self
                    .step(Vec::new(), None, dispositions)
                    .map(|step| InternalScan::CommandResult(Box::new(step)));
            }
            let transitions = match self.select_transitions(Some(&event), Some(&event), evaluator) {
                Ok(transitions) => transitions,
                Err(error) => {
                    self.rollback_internal_scan(queue_len, consumed, current_event)?;
                    return Err(error);
                }
            };
            self.internal.pop_front();
            consumed.push(event.clone());
            self.current_event = Some(event.clone());
            let (transitions, disposition) = if transitions.is_empty() {
                let eventless = match self.select_transitions(None, Some(&event), evaluator) {
                    Ok(eventless) => eventless,
                    Err(error) => {
                        self.rollback_internal_scan(queue_len, consumed, current_event)?;
                        return Err(error);
                    }
                };
                (eventless, EventDisposition::Ignored)
            } else {
                (transitions, EventDisposition::Selected)
            };
            dispositions.push(DisposedEvent {
                event: event.clone(),
                disposition,
            });
            if !transitions.is_empty() {
                let result = self.apply_microstep(&transitions, Some(&event), dispositions);
                if result.is_err() {
                    self.rollback_internal_scan(queue_len, consumed, current_event)?;
                }
                return result.map(|step| InternalScan::CommandResult(Box::new(step)));
            }
        }
        Ok(InternalScan::Drained(dispositions))
    }

    /// Undoes one failed internal-queue scan of [`Self::resume`]: events the
    /// scan appended (error.execution from recoverable guard failures of
    /// selections that succeeded before the failure) are discarded, the
    /// consumed prefix returns to the queue front in order, and `_event`
    /// returns to its value before the scan. The scan only pops from the
    /// front and appends at the back, so the original queue of `queue_len`
    /// events is exactly the consumed prefix followed by the first
    /// `queue_len - consumed.len()` events still queued.
    fn rollback_internal_scan(
        &mut self,
        queue_len: usize,
        consumed: Vec<Event>,
        current_event: Option<Event>,
    ) -> Result<(), EngineError> {
        let remaining = queue_len
            .checked_sub(consumed.len())
            .ok_or(EngineError::BrokenInvariant)?;
        if self.internal.len() < remaining {
            return Err(EngineError::BrokenInvariant);
        }
        self.internal.truncate(remaining);
        let result = self.restore_internal_prefix(consumed);
        self.current_event = current_event;
        result
    }

    /// The validated Machine IR this engine executes.
    #[must_use]
    pub const fn machine(&self) -> &MachineIr {
        &self.machine
    }

    /// Lifecycle state.
    #[must_use]
    pub const fn status(&self) -> &EngineStatus {
        &self.status
    }

    /// Active configuration leaves in document order, one leaf per parallel
    /// region (atomic and final states, and a `<parallel>` without
    /// regions); ancestors are implicit (SCXML 3.4).
    #[must_use]
    pub fn leaves(&self) -> &[StateId] {
        &self.active_states
    }

    /// Number of internal events waiting for processing (SCXML Appendix D
    /// internal queue); zero whenever the engine is stable.
    #[must_use]
    pub fn internal_len(&self) -> usize {
        self.internal.len()
    }

    /// The internal event the next command processes first, if any.
    #[must_use]
    pub fn next_internal(&self) -> Option<&Event> {
        self.internal.front()
    }

    /// Whether the engine may accept another external event: no internal
    /// event or enabled eventless transition is pending (SCXML Appendix D).
    #[must_use]
    pub const fn is_stable(&self) -> bool {
        self.stable
    }

    /// Whether an [`Engine::run`] call suspended its effect transaction at
    /// a deferred effect: only [`Command::Continue`] runs next.
    #[must_use]
    pub const fn is_suspended(&self) -> bool {
        self.suspension.is_some()
    }

    fn require_dispatchable(&self) -> Result<(), EngineError> {
        match (&self.status, &self.effect_phase, &self.pending_step) {
            (_, EffectPhase::Applying { .. }, _) | (_, _, Some(_)) => {
                Err(EngineError::EffectProtocol)
            }
            (EngineStatus::Ready, EffectPhase::Idle, None) => Err(EngineError::NotStarted),
            (EngineStatus::Finished, EffectPhase::Idle, None) => Err(EngineError::Finished),
            (EngineStatus::Running, EffectPhase::Idle, None) if !self.stable => {
                Err(EngineError::NotStable)
            }
            (EngineStatus::Running, EffectPhase::Idle, None) => Ok(()),
        }
    }

    fn step(
        &mut self,
        planned_effects: Vec<PlannedEffect>,
        trigger_event: Option<Event>,
        event_dispositions: Vec<DisposedEvent>,
    ) -> Result<CommandResult, EngineError> {
        self.step_with_sets(
            planned_effects,
            trigger_event,
            event_dispositions,
            Vec::new(),
            Vec::new(),
        )
    }

    /// Builds and arms one command result carrying its ordered exited and entered
    /// state sets and the complete configuration after the command.
    fn step_with_sets(
        &mut self,
        planned_effects: Vec<PlannedEffect>,
        trigger_event: Option<Event>,
        event_dispositions: Vec<DisposedEvent>,
        exited_states: Vec<StateId>,
        entered_states: Vec<StateId>,
    ) -> Result<CommandResult, EngineError> {
        let effects = planned_effects
            .iter()
            .map(|planned| planned.effect.clone())
            .collect();
        let step = CommandResult {
            status: self.status.clone(),
            leaves: self.active_states.clone(),
            effects,
            planned_effects,
            trigger_event,
            event_dispositions,
            exited_states,
            entered_states,
            configuration: self.configuration_ids_all()?,
            stable: self.stable,
        };
        self.pending_step = Some(step.clone());
        Ok(step)
    }

    /// Conservative post-command stability hint: the machine is stable
    /// when finished, or when no internal, self-addressed external, pending
    /// dispatch, or invocation work remains and the configuration holds no
    /// eventless transition at all. A configuration that does hold one stays
    /// not stable so the next [`Self::resume`] evaluates its guards; that
    /// resume, not this structural probe, declares a stable configuration when none is
    /// enabled (W3C Appendix D).
    fn settle_stable(&mut self) -> Result<(), EngineError> {
        self.stable = self.status == EngineStatus::Finished
            || (self.internal.is_empty()
                && self.pending_external.is_empty()
                && self.pending_dispatch.is_none()
                && self.states_to_invoke.is_empty()
                && !self.configuration_has_eventless_transition_all()?);
        Ok(())
    }

    /// Selects and applies transitions for one external event (W3C
    /// `mainEventLoop` external branch after `<finalize>` ran).
    fn dispatch_external(
        &mut self,
        event: &Event,
        evaluator: &mut impl GuardEvaluator,
    ) -> Result<CommandResult, EngineError> {
        let (queue_len, stable) = (self.internal.len(), self.stable);
        let transitions = self.select_transitions(Some(event), Some(event), evaluator)?;
        if transitions.is_empty() {
            self.current_event = Some(event.clone());
            self.settle_stable()?;
            return self.step(
                Vec::new(),
                Some(event.clone()),
                vec![DisposedEvent {
                    event: event.clone(),
                    disposition: EventDisposition::Ignored,
                }],
            );
        }
        self.with_selection_rollback(queue_len, stable, |engine| {
            engine.apply_microstep(
                &transitions,
                Some(event),
                vec![DisposedEvent {
                    event: event.clone(),
                    disposition: EventDisposition::Selected,
                }],
            )
        })
    }

    /// Runs the microstep of a committed selection and, when it fails,
    /// truncates the internal queue back to `queue_len` and restores
    /// `stable`, both recorded before that selection. The selection's
    /// error.execution events for recoverable guard failures are thereby
    /// discarded with the rejected command, which leaves the engine
    /// unchanged (PAT-RTC-001; [`Engine::run`] `Rejected` postcondition).
    /// `work` must only append to the internal queue, never pop from it.
    fn with_selection_rollback<T>(
        &mut self,
        queue_len: usize,
        stable: bool,
        work: impl FnOnce(&mut Self) -> Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        let result = work(self);
        if result.is_err() {
            self.internal.truncate(queue_len);
            self.stable = stable;
        }
        result
    }

    /// Plans the Appendix D external-event prelude for `event`: for every
    /// started invocation, in configuration document order and invoke
    /// document order, the `<finalize>` content when the event carries its
    /// invoke id (SCXML 6.4 `applyFinalize`), then one [`Effect::Forward`]
    /// when the invocation declared `autoforward` (SCXML 6.4.1).
    fn external_prelude_items(&mut self, event: &Event) -> Result<VecDeque<PlanItem>, EngineError> {
        let mut items = VecDeque::new();
        if self.active_invocations.is_empty() {
            return Ok(items);
        }
        let mut ordered: Vec<(u32, u32, usize, InvokeId)> = Vec::new();
        for active in &self.active_invocations {
            let order = self.state_by_index(active.state)?.document_order;
            ordered.push((
                order,
                active.invoke_order,
                active.state,
                active.invoke_id.clone(),
            ));
        }
        ordered.sort_unstable();
        let configuration = self.journal.mark();
        for (_, invoke_order, state, invoke_id) in ordered {
            let declaration = self
                .state_by_index(state)?
                .invokes
                .iter()
                .find(|invoke| invoke.document_order == invoke_order)
                .ok_or(EngineError::BrokenInvariant)?;
            let auto_forward = declaration.autoforward;
            let finalize = if event.invoke_id.as_ref() == Some(&invoke_id) {
                declaration.children.iter().find_map(|child| match child {
                    InvokeChildIr::Finalize { actions } => Some(actions.clone()),
                    _ => None,
                })
            } else {
                None
            };
            match finalize {
                Some(actions) if !actions.is_empty() => {
                    let context = EffectContext {
                        phase: EffectExecutionPhase::Finalize,
                        configuration,
                        block: self.alloc_effect_block(),
                    };
                    flatten_actions(&mut items, &actions, &context)?;
                }
                // SCXML 6.5.2: "If no executable content is specified, the
                // SCXML Processor MUST update the data model each time an
                // event is received from the child process ... for each
                // item in the 'namelist' attribute and each such <param>
                // element, the Processor MUST update the corresponding
                // location as if by <assign> with any return value that has
                // a name that matches the 'namelist' item or the 'name' of
                // the <param> element." One block, in namelist then param
                // order. An absent <finalize> updates nothing.
                Some(_) => {
                    let updates: Vec<Effect> = declaration
                        .namelist
                        .iter()
                        .map(|location| Effect::UpdateFromReturnedValue {
                            name: None,
                            location: location.clone(),
                        })
                        .chain(declaration.children.iter().filter_map(|child| match child {
                            InvokeChildIr::Param { value } => {
                                value.location.as_ref().map(|location| {
                                    Effect::UpdateFromReturnedValue {
                                        name: Some(value.name.clone()),
                                        location: location.clone(),
                                    }
                                })
                            }
                            _ => None,
                        }))
                        .collect();
                    if !updates.is_empty() {
                        let context = EffectContext {
                            phase: EffectExecutionPhase::Finalize,
                            configuration,
                            block: self.alloc_effect_block(),
                        };
                        for effect in updates {
                            ensure_effect_room(items.len())?;
                            items.push_back(PlanItem::Emit {
                                effect,
                                context: context.clone(),
                            });
                        }
                    }
                }
                None => {}
            }
            if auto_forward {
                ensure_effect_room(items.len())?;
                let context = EffectContext {
                    phase: EffectExecutionPhase::Finalize,
                    configuration,
                    block: self.alloc_effect_block(),
                };
                items.push_back(PlanItem::Emit {
                    effect: Effect::Forward {
                        invoke_id,
                        event: Box::new(event.clone()),
                    },
                    context,
                });
            }
        }
        Ok(items)
    }

    /// Arms the external-event prelude command result for `event` when it has any
    /// `<finalize>` or autoforward work; selection for the event then runs
    /// on the next `resume` through `pending_dispatch`. Returns `None`, with
    /// no state change, when the prelude is empty.
    fn external_prelude_step(
        &mut self,
        event: &Event,
        event_dispositions: Vec<DisposedEvent>,
    ) -> Result<Option<CommandResult>, EngineError> {
        let block_before = self.next_effect_block;
        let items = self.external_prelude_items(event)?;
        if items.is_empty() {
            self.next_effect_block = block_before;
            return Ok(None);
        }
        let plan = plan_step(items)?;
        self.armed_plan = if plan.plan.frames.is_empty() {
            None
        } else {
            Some(plan.plan)
        };
        self.current_event = Some(event.clone());
        self.pending_dispatch = Some(event.clone());
        self.stable = false;
        self.step(plan.effects, Some(event.clone()), event_dispositions)
            .map(Some)
    }

    /// Emits one end-of-macrostep invocation batch: every invoke of every
    /// state entered and not exited during the macrostep, in entry order
    /// and invoke document order (SCXML 6.4 / Appendix D `mainEventLoop`).
    fn invoke_step(
        &mut self,
        event_dispositions: Vec<DisposedEvent>,
    ) -> Result<CommandResult, EngineError> {
        let mut states: Vec<usize> = self.states_to_invoke.iter().copied().collect();
        states.sort_by_key(|&index| {
            self.state_by_index(index)
                .map_or(u32::MAX, |state| state.document_order)
        });
        let configuration = self.journal.mark();
        let mut effects = Vec::new();
        let mut awaiting = Vec::new();
        let mut block_counter = self.next_effect_block;
        for state_index in states {
            let state = self.state_by_index(state_index)?;
            let state_id = state_id(state)?;
            let mut invokes = state.invokes.clone();
            invokes.sort_by_key(|invoke| invoke.document_order);
            for invoke in invokes {
                ensure_effect_room(effects.len())?;
                awaiting.push((state_index, invoke.document_order));
                effects.push(PlannedEffect {
                    effect: Effect::Invoke {
                        state: state_id.clone(),
                        invoke: Box::new(invoke),
                    },
                    context: EffectContext {
                        phase: EffectExecutionPhase::Invoke,
                        configuration,
                        block: block_counter,
                    },
                });
                block_counter = block_counter.saturating_add(1);
            }
        }
        self.next_effect_block = block_counter;
        self.states_to_invoke.clear();
        self.awaiting_invocations = awaiting;
        self.settle_stable()?;
        let current_event = self.current_event.clone();
        self.step(effects, current_event, event_dispositions)
    }

    /// State ids for table indices, preserving order.
    fn state_ids(&self, indices: &[usize]) -> Result<Vec<StateId>, EngineError> {
        indices
            .iter()
            .map(|&index| self.state_id_by_index(index))
            .collect()
    }

    /// Commits the lifecycle bookkeeping of one successfully planned entry:
    /// entered invoking states join `statesToInvoke` and newly bound
    /// late-binding states are recorded (SCXML 5.3/6.4).
    fn commit_entry(&mut self, entered: &EnteredStates) -> Result<(), EngineError> {
        for &index in &entered.order {
            if !self.state_by_index(index)?.invokes.is_empty() {
                self.states_to_invoke.insert(index);
            }
        }
        self.bound_states
            .extend(entered.newly_bound.iter().copied());
        Ok(())
    }

    /// W3C selection: every active leaf (document order) walks its own chain
    /// of transitions, inner transitions preempting outer ones; the same
    /// transition is selected at most once. Conflicting transitions whose
    /// exit sets intersect are resolved by source ancestry (SCXML algorithm
    /// `selectTransitions`/`removeConflictingTransitions`).
    #[allow(
        clippy::too_many_lines,
        reason = "one function owns the W3C selectTransitions/preemption/conflict-resolution walk"
    )]
    fn select_transitions(
        &mut self,
        trigger: Option<&Event>,
        current_event: Option<&Event>,
        evaluator: &mut impl GuardEvaluator,
    ) -> Result<Vec<TransitionIr>, EngineError> {
        // No transition can match: nothing is evaluated, nothing selected.
        let unmatched = match trigger {
            Some(event) => !self.event_index.may_match(event.name.as_str()),
            None => !self.has_eventless,
        };
        if unmatched {
            return Ok(Vec::new());
        }
        let mut enabled: Vec<TransitionIr> = Vec::new();
        let mut enabled_indices = std::collections::BTreeSet::new();
        // Recoverable guard failures (SCXML 5.9/5.10 error.execution), queued
        // only when the whole selection succeeds.
        let mut staged_errors = 0_usize;
        for leaf in self.active_atomic_indices()? {
            let mut current = leaf;
            // W3C `break loop`: once one transition along the leaf's
            // ancestor chain is enabled, selection moves to the next
            // atomic state; outer ancestors are never considered.
            let mut selected_for_leaf = false;
            while !selected_for_leaf {
                let state = self.state_by_index(current)?;
                for transition_index in &state.transitions {
                    let transition = self.transition(*transition_index)?;
                    if !transition_matches(transition, trigger) {
                        continue;
                    }
                    let enabled_cond = match transition.cond.as_deref() {
                        Some(condition) => {
                            match evaluator.evaluate_condition(condition, current_event) {
                                Ok(enabled) => enabled,
                                // A cond expression that is rejected or cannot be
                                // evaluated as a boolean raises error.execution and
                                // evaluates as false; transition selection continues
                                // (SCXML 5.9/5.10, WIT `evaluate-condition`).
                                // Unavailable or failed evaluators still fail closed.
                                // The error is staged and queued only when the
                                // whole selection succeeds, so a later fatal
                                // guard failure leaves the engine unchanged.
                                Err(EvaluationError::InvalidInput | EvaluationError::Execution) => {
                                    staged_errors = staged_errors.saturating_add(1);
                                    false
                                }
                                Err(error) => return Err(EngineError::Evaluation(error)),
                            }
                        }
                        None => true,
                    };
                    if enabled_cond {
                        // Optimally-enabled set semantics: the same
                        // transition selected from several regions of one
                        // parallel still executes exactly once.
                        if enabled_indices.insert(*transition_index) {
                            enabled.push(transition.clone());
                        }
                        selected_for_leaf = true;
                        break;
                    }
                }
                if selected_for_leaf {
                    break;
                }
                match state.parent {
                    Some(parent) => {
                        current =
                            usize::try_from(parent).map_err(|_| EngineError::BrokenInvariant)?;
                        if current >= self.machine.states.len()
                            || current >= MAX_HIERARCHY_DEPTH.saturating_add(1)
                        {
                            return Err(EngineError::BrokenInvariant);
                        }
                    }
                    None => break,
                }
            }
        }
        // No re-sort: `enabled` already follows the document order of the
        // selecting atomic states (`active_leaf_indices`), as W3C requires.
        //
        // W3C `removeConflictingTransitions`: a transition is preempted when
        // its exit set intersects a kept transition whose source is not its
        // proper ancestor; otherwise it replaces every kept transition it
        // intersects. Kept exit sets stay pairwise disjoint, so `owner` maps
        // each exited state to the one kept slot exiting it and every
        // intersection is found by lookup instead of a pairwise rescan.
        let active = if enabled.len() > 1 {
            Some(self.active_configuration()?)
        } else {
            None
        };
        let mut kept: Vec<Option<(TransitionIr, std::collections::BTreeSet<usize>)>> = Vec::new();
        let mut owner: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
        for selected in enabled {
            let Some(active) = &active else {
                kept.push(Some((selected, std::collections::BTreeSet::new())));
                continue;
            };
            let exit_set = self.transition_exit_indices(&selected, active)?;
            let selected_source =
                usize::try_from(selected.source).map_err(|_| EngineError::BrokenInvariant)?;
            let mut preempted = false;
            let mut to_remove = std::collections::BTreeSet::new();
            for exited in &exit_set {
                let Some(&slot) = owner.get(exited) else {
                    continue;
                };
                if to_remove.contains(&slot) {
                    continue;
                }
                let (kept_transition, _) = kept
                    .get(slot)
                    .and_then(Option::as_ref)
                    .ok_or(EngineError::BrokenInvariant)?;
                let kept_source = usize::try_from(kept_transition.source)
                    .map_err(|_| EngineError::BrokenInvariant)?;
                if self.is_proper_descendant(selected_source, kept_source)? {
                    to_remove.insert(slot);
                } else {
                    preempted = true;
                    break;
                }
            }
            if !preempted {
                for slot in to_remove {
                    let (_, removed_exits) = kept
                        .get_mut(slot)
                        .and_then(Option::take)
                        .ok_or(EngineError::BrokenInvariant)?;
                    for exited in removed_exits {
                        owner.remove(&exited);
                    }
                }
                let slot = kept.len();
                for &exited in &exit_set {
                    owner.insert(exited, slot);
                }
                kept.push(Some((selected, exit_set)));
            }
        }
        let filtered = kept.into_iter().flatten().map(|(selected, _)| selected);
        // Commit the staged recoverable guard errors only now that selection
        // succeeded; the capacity check comes first so a queue overflow also
        // rejects without queueing any of them.
        if scxml_types::exceeds_wasm_bound(
            self.internal.len().saturating_add(staged_errors),
            MAX_NESTED_ITEMS,
        ) {
            return Err(EngineError::QueueLimit);
        }
        let context = (staged_errors > 0)
            .then(|| Self::condition_error_trace_context(evaluator, current_event))
            .flatten();
        for _ in 0..staged_errors {
            self.push_platform_error(PlatformError::Execution, None, context.clone())?;
        }
        Ok(filtered.collect())
    }

    /// Active states exited by one transition: empty for targetless
    /// transitions, otherwise every member of `active` that is a proper
    /// descendant of the transition domain (SCXML `computeExitSet`).
    ///
    /// The configuration is closed under ancestors, so its members below the
    /// domain are exactly those reached from the domain through active
    /// children; the walk visits only them and their direct children.
    fn transition_exit_indices(
        &self,
        transition: &TransitionIr,
        active: &ActiveConfiguration,
    ) -> Result<std::collections::BTreeSet<usize>, EngineError> {
        if transition.targets.is_empty() {
            return Ok(std::collections::BTreeSet::new());
        }
        let Some(domain) = self.transition_domain(transition)? else {
            return Ok(active.ordered.iter().copied().collect());
        };
        let mut exit_set = std::collections::BTreeSet::new();
        let mut pending = vec![domain];
        while let Some(state) = pending.pop() {
            for &child in &self.state_by_index(state)?.children {
                let child = usize::try_from(child).map_err(|_| EngineError::BrokenInvariant)?;
                if *active
                    .member
                    .get(child)
                    .ok_or(EngineError::BrokenInvariant)?
                    && exit_set.insert(child)
                {
                    pending.push(child);
                }
            }
        }
        Ok(exit_set)
    }

    /// Applies one microstep of selected transitions with W3C hierarchy
    /// semantics: exit every active state below the transition domains
    /// innermost-first, execute transition content with the retained
    /// configuration active, then enter the computed entry set in entry order
    /// with each state's partial entry prefix observable. Targetless
    /// transitions execute content while preserving the whole configuration.
    #[allow(
        clippy::too_many_lines,
        reason = "one microstep owns the full W3C exit/content/entry ordering contract"
    )]
    fn apply_microstep(
        &mut self,
        transitions: &[TransitionIr],
        trigger_event: Option<&Event>,
        event_dispositions: Vec<DisposedEvent>,
    ) -> Result<CommandResult, EngineError> {
        let mut items = VecDeque::new();
        let mut block_counter = self.next_effect_block;
        // SCXML 3.13: "It MUST then execute the executable content contained
        // in the transitions in document order." Appendix D (informative)
        // runs it in the order the atomic states selected the transitions
        // (docs/spec-errata.md); the normative prose governs.
        let mut in_document_order: Vec<&TransitionIr> = transitions.iter().collect();
        in_document_order.sort_by_key(|transition| transition.document_order);
        if transitions
            .iter()
            .all(|transition| transition.targets.is_empty())
        {
            for transition in &in_document_order {
                let block = block_counter;
                block_counter = block_counter.saturating_add(1);
                if !transition.actions.is_empty() {
                    flatten_actions(
                        &mut items,
                        &transition.actions,
                        &EffectContext {
                            phase: EffectExecutionPhase::Transition,
                            configuration: self.journal.mark(),
                            block,
                        },
                    )?;
                }
            }
            let plan = plan_step(items)?;
            self.next_effect_block = block_counter;
            self.armed_plan = if plan.plan.frames.is_empty() {
                None
            } else {
                Some(plan.plan)
            };
            self.current_event = trigger_event.cloned();
            self.settle_stable()?;
            return self.step(plan.effects, trigger_event.cloned(), event_dispositions);
        }
        // Union exit set over every target-bearing transition. Conflict
        // resolution already guaranteed pairwise-disjoint exit sets, so
        // the union is exact.
        let active = self.active_configuration()?;
        let mut exit_set = std::collections::BTreeSet::new();
        for transition in transitions {
            if transition.targets.is_empty() {
                continue;
            }
            exit_set.extend(self.transition_exit_indices(transition, &active)?);
        }
        // Reverse table order is reverse document order (see
        // [`sort_document_order`]): innermost-first exit order.
        let exits: Vec<usize> = exit_set.iter().rev().copied().collect();
        // History is recorded before any exited state leaves (W3C
        // `exitStates`), so the entry set below resolves through it.
        for (history, stored) in self.recorded_history(&exits)? {
            self.history_values.insert(history, stored);
        }
        // Exit content runs innermost-first, and each state stops being
        // active once its exit content finished: an outer `onexit` must
        // not observe already-exited descendants through `In()`
        // (official case 409 oracle). Each exit is one journal change.
        let mut cancelled: Vec<usize> = Vec::new();
        for &exited in &exits {
            let exit_block = block_counter;
            block_counter = block_counter.saturating_add(1);
            let on_exit = &self.state_by_index(exited)?.onexit;
            // With no blocks `flatten_blocks` allocates no block number,
            // so skipping it keeps block numbering unchanged.
            if !on_exit.is_empty() {
                flatten_blocks(
                    &mut items,
                    on_exit,
                    &EffectContext {
                        phase: EffectExecutionPhase::Exit,
                        configuration: self.journal.mark(),
                        block: exit_block,
                    },
                    &mut block_counter,
                )?;
            }
            // SCXML 6.4 `exitStates`: cancel every started invocation of
            // the exited state after its onexit content, while the state
            // is still in the configuration.
            let cancel_block = block_counter;
            block_counter = block_counter.saturating_add(1);
            if self
                .active_invocations
                .iter()
                .any(|invocation| invocation.state == exited)
            {
                self.push_cancellations(
                    &mut items,
                    |state| state == exited,
                    &mut cancelled,
                    &EffectContext {
                        phase: EffectExecutionPhase::Exit,
                        configuration: self.journal.mark(),
                        block: cancel_block,
                    },
                )?;
            }
            self.journal.exit(exited)?;
        }
        for transition in &in_document_order {
            let block = block_counter;
            block_counter = block_counter.saturating_add(1);
            if !transition.actions.is_empty() {
                flatten_actions(
                    &mut items,
                    &transition.actions,
                    &EffectContext {
                        phase: EffectExecutionPhase::Transition,
                        configuration: self.journal.mark(),
                        block,
                    },
                )?;
            }
        }
        let mut entry = EntryPlan::default();
        for transition in transitions {
            if transition.targets.is_empty() {
                continue;
            }
            let domain = self.transition_domain(transition)?;
            for &target in &transition.targets {
                let target_index =
                    usize::try_from(target).map_err(|_| EngineError::BrokenInvariant)?;
                self.add_descendant_states_to_enter(
                    target_index,
                    &self.history_values,
                    &mut entry,
                    0,
                )?;
            }
            for &target in &transition.targets {
                let target_index =
                    usize::try_from(target).map_err(|_| EngineError::BrokenInvariant)?;
                self.add_ancestor_states_to_enter(
                    target_index,
                    domain,
                    &self.history_values,
                    &mut entry,
                )?;
            }
        }
        let retained_indices: Vec<usize> = active
            .ordered
            .iter()
            .copied()
            .filter(|index| !exit_set.contains(index))
            .collect();
        self.next_effect_block = block_counter;
        let entered = self.flatten_entry_effects(&mut items, &entry, &retained_indices)?;
        let finishing = self.enters_top_level_final(&entry.states_to_enter)?;
        if finishing {
            // Entering a top-level final terminates the session: every
            // remaining started invocation is cancelled (SCXML 6.4,
            // Appendix D `exitInterpreter`) while the entered states alone
            // stay observable.
            for &retained in &retained_indices {
                self.journal.exit(retained)?;
            }
            let cancel_context = EffectContext {
                phase: EffectExecutionPhase::Exit,
                configuration: self.journal.mark(),
                block: self.alloc_effect_block(),
            };
            self.push_cancellations(&mut items, |_| true, &mut cancelled, &cancel_context)?;
        }
        let plan = plan_step(items)?;
        self.armed_plan = if plan.plan.frames.is_empty() {
            None
        } else {
            Some(plan.plan)
        };
        // New leaf set: leaves that survived the exit phase plus the
        // leaves entered by this microstep. Replacing the whole set
        // would drop sibling parallel regions outside the domain.
        let mut new_leaves: Vec<usize> = retained_indices
            .iter()
            .copied()
            .filter(|index| {
                self.state_by_index(*index)
                    .is_ok_and(|state| is_configuration_leaf(state, &self.machine.states))
            })
            .collect();
        new_leaves.extend(entered.leaves.iter().copied());
        sort_document_order(&mut new_leaves);
        new_leaves.dedup();
        self.active_states = new_leaves
            .iter()
            .map(|&index| self.state_id_by_index(index))
            .collect::<Result<_, _>>()?;
        self.status = if finishing {
            EngineStatus::Finished
        } else {
            EngineStatus::Running
        };
        // Lifecycle commit: exited states leave `statesToInvoke`, their
        // cancelled invocations stop being live, and entered invoking
        // states join `statesToInvoke` (SCXML 6.4).
        for exited in &exits {
            self.states_to_invoke.remove(exited);
        }
        let mut position = 0_usize;
        self.active_invocations.retain(|_| {
            let keep = !cancelled.contains(&position);
            position = position.saturating_add(1);
            keep
        });
        self.commit_entry(&entered)?;
        self.current_event = trigger_event.cloned();
        self.settle_stable()?;
        let exited_ids = self.state_ids(&exits)?;
        let entered_ids = self.state_ids(&entered.order)?;
        self.step_with_sets(
            plan.effects,
            trigger_event.cloned(),
            event_dispositions,
            exited_ids,
            entered_ids,
        )
    }

    /// Appends entry-phase plan items for one computed entry set in entry
    /// order (ancestors before descendants, document order otherwise).
    ///
    /// Each entered state observes the retained configuration plus every
    /// state entered before it. A compound state whose entry came from its
    /// initial transition executes that transition's content between its own
    /// onentry and the child entries (official case 412 oracle). Entering a
    /// final state with a compound parent plans a `done.state.<parent-id>`
    /// raise; a parallel grandparent whose regions are all final plans its
    /// own done event (SCXML 3.13). Returns the new atomic/final leaves in
    /// document order.
    #[allow(
        clippy::too_many_lines,
        reason = "entry flattening interleaves on-entry, initial, donedata, and parallel done content per the W3C order"
    )]
    fn flatten_entry_effects(
        &mut self,
        items: &mut VecDeque<PlanItem>,
        entry: &EntryPlan,
        retained_indices: &[usize],
    ) -> Result<EnteredStates, EngineError> {
        let mut block_counter = self.next_effect_block;
        // Each entered state is one journal change; the journal leaves
        // `self` while the closure borrows the machine.
        let mut journal = std::mem::take(&mut self.journal);
        let result = (|| {
            let mut next_block = || {
                let block = block_counter;
                block_counter = block_counter.saturating_add(1);
                block
            };
            let mut order = entry.states_to_enter.clone();
            sort_document_order(&mut order);
            // Membership of the retained and entered states, for the
            // parallel done check.
            let mut entered_member = vec![false; self.machine.states.len()];
            for &index in retained_indices {
                *entered_member
                    .get_mut(index)
                    .ok_or(EngineError::BrokenInvariant)? = true;
            }
            let mut leaves = Vec::new();
            let mut newly_bound = Vec::new();
            let mut history_content_run = std::collections::BTreeSet::new();
            // Per parallel, the position of its first region not yet known
            // to be in a final state. Membership only grows while entering,
            // so a final region stays final and each region is passed once.
            let mut final_regions = std::collections::BTreeMap::new();
            let late_binding = self.machine.binding == Binding::Late;
            for &index in &order {
                let entered_id = self.state_id_by_index(index)?;
                journal.enter(index)?;
                let entered_mark = journal.mark();
                *entered_member
                    .get_mut(index)
                    .ok_or(EngineError::BrokenInvariant)? = true;
                let state = self.state_by_index(index)?;
                // SCXML 5.3 late binding: a state's data binds on its first
                // entry, before its onentry content runs.
                if late_binding && !state.data.is_empty() && !self.bound_states.contains(&index) {
                    ensure_effect_room(items.len())?;
                    items.push_back(PlanItem::Emit {
                        effect: Effect::BindStateData {
                            state: entered_id.clone(),
                            declarations: state.data.clone(),
                        },
                        context: EffectContext {
                            phase: EffectExecutionPhase::Entry,
                            configuration: entered_mark,
                            block: next_block(),
                        },
                    });
                    newly_bound.push(index);
                }
                // Block numbers are allocated whether or not content exists.
                let entry_context = |block| EffectContext {
                    phase: EffectExecutionPhase::Entry,
                    configuration: entered_mark,
                    block,
                };
                // When the history's parent is the transition domain and stays
                // active, it has no onentry in this microstep; the content
                // still runs (3.10.2), where the parent's entry would end:
                // before the first state entered below it. Appendix D
                // (informative) drops it (docs/spec-errata.md).
                if let Some(owner) = state
                    .parent
                    .and_then(|parent| usize::try_from(parent).ok())
                    .filter(|parent| !entry.entered.contains(parent))
                    && entry.default_history_content.contains_key(&owner)
                    && history_content_run.insert(owner)
                    && let Some(actions) = entry.default_history_content.get(&owner)
                {
                    let number = next_block();
                    if !actions.is_empty() {
                        flatten_actions(items, actions, &entry_context(number))?;
                    }
                }
                for block in &state.onentry {
                    let number = next_block();
                    if !block.actions.is_empty() {
                        flatten_actions(items, &block.actions, &entry_context(number))?;
                    }
                }
                if entry.states_for_default_entry.contains(&index) {
                    let initial = state.initial.as_ref().ok_or(EngineError::BrokenInvariant)?;
                    let number = next_block();
                    if !initial.actions.is_empty() {
                        flatten_actions(items, &initial.actions, &entry_context(number))?;
                    }
                }
                // Default history content (SCXML 3.10.2: "The Process MUST
                // execute any executable content in the transition after the
                // parent state's onentry handlers"): when the entry set
                // descends from a history pseudostate without a stored value,
                // the fallback transition's content executes once, after the
                // parent's onentry and initial content (W3C enterStates
                // `defaultHistoryContent[s.id]`; official case 579).
                if entry.default_history_content.contains_key(&index)
                    && history_content_run.insert(index)
                    && let Some(actions) = entry.default_history_content.get(&index)
                {
                    let number = next_block();
                    if !actions.is_empty() {
                        flatten_actions(items, actions, &entry_context(number))?;
                    }
                }
                if state.kind == StateKind::Final {
                    // Only a final child of a compound state raises done.state;
                    // a top-level final state terminates the machine instead.
                    if let Some(parent) = state.parent {
                        let parent_index =
                            usize::try_from(parent).map_err(|_| EngineError::BrokenInvariant)?;
                        let parent_id = self.state_id_by_index(parent_index)?;
                        let done_event = format!("done.state.{}", parent_id.as_str());
                        // `<donedata>` evaluates as the done event's data
                        // through the `DoneEvent` effect; absent
                        // `<donedata>` the raise stays bare (SCXML
                        // 5.5.2/6.5).
                        let effect = match state.donedata.clone() {
                            Some(donedata) => Effect::DoneEvent {
                                event: done_event,
                                donedata: Box::new(donedata),
                            },
                            None => Effect::Raise(done_event, false, None),
                        };
                        ensure_effect_room(items.len())?;
                        items.push_back(PlanItem::Emit {
                            effect,
                            context: entry_context(next_block()),
                        });
                        // When the grandparent is a parallel state and every
                        // one of its regions is now in a final state, the
                        // parallel itself raises `done.state.<grandparent-id>`
                        // immediately after its child region's done event
                        // (SCXML 3.13; official cases 417 and 570). States
                        // surviving the exit phase plus everything entered so
                        // far are checked; the pre-microstep configuration
                        // would wrongly count exited sibling regions.
                        if let Some(grandparent) = self.state_by_index(parent_index)?.parent {
                            let grandparent_index = usize::try_from(grandparent)
                                .map_err(|_| EngineError::BrokenInvariant)?;
                            if self.state_by_index(grandparent_index)?.kind == StateKind::Parallel
                                && self.regions_all_final_from(
                                    grandparent_index,
                                    &entered_member,
                                    final_regions.entry(grandparent_index).or_insert(0),
                                )?
                            {
                                let grandparent_id = self.state_id_by_index(grandparent_index)?;
                                ensure_effect_room(items.len())?;
                                items.push_back(PlanItem::Emit {
                                    effect: Effect::Raise(
                                        format!("done.state.{}", grandparent_id.as_str()),
                                        false,
                                        None,
                                    ),
                                    context: entry_context(next_block()),
                                });
                            }
                        }
                    }
                }
                // Entering a top-level final terminates the machine; the W3C
                // algorithm exits every configuration member first, so the
                // final state's own `onexit` content still executes before
                // termination (SCXML 3.12 exitStates; official case 236).
                if state.kind == StateKind::Final && state.parent.is_none() {
                    for block in &state.onexit {
                        flatten_actions(
                            items,
                            &block.actions,
                            &EffectContext {
                                phase: EffectExecutionPhase::Exit,
                                configuration: entered_mark,
                                block: next_block(),
                            },
                        )?;
                    }
                    // W3C exitInterpreter: once the final state has left the
                    // configuration, returnDoneEvent(s.donedata) hands its
                    // payload to the invoking session (SCXML 6.4). The final
                    // state, the last one entered, is re-entered afterwards
                    // so the journal keeps the entered order.
                    if let Some(done_data) = state.donedata.clone() {
                        ensure_effect_room(items.len())?;
                        journal.exit(index)?;
                        items.push_back(PlanItem::Emit {
                            effect: Effect::ReturnDoneEvent(Box::new(done_data)),
                            context: EffectContext {
                                phase: EffectExecutionPhase::Exit,
                                configuration: journal.mark(),
                                block: next_block(),
                            },
                        });
                        journal.enter(index)?;
                    }
                }
                if is_configuration_leaf(state, &self.machine.states) {
                    leaves.push(index);
                }
            }
            Ok(EnteredStates {
                order,
                leaves,
                newly_bound,
            })
        })();
        self.journal = journal;
        if result.is_ok() {
            self.next_effect_block = block_counter;
        }
        result
    }

    /// Starts the current command's configuration journal at the active
    /// configuration, in document order.
    fn begin_command_journal(&mut self) -> Result<(), EngineError> {
        let base = self.configuration_indices_all()?;
        self.journal.reset(base);
        Ok(())
    }

    /// The configuration `context` makes visible to data-model `In()`
    /// queries: the retained states in document order, then the states
    /// entered before (and including) the effect's state, in entry order.
    /// Materialized in full, so it is meant for inspection, not per effect.
    ///
    /// # Errors
    ///
    /// [`EngineError::BrokenInvariant`] when `context` was not planned by
    /// the engine's most recent command.
    pub fn effect_configuration(
        &self,
        context: &EffectContext,
    ) -> Result<Vec<StateId>, EngineError> {
        self.state_ids(&self.journal.configuration_at(context.configuration)?)
    }

    /// Appends one [`Effect::CancelInvoke`] per started invocation whose
    /// invoking state matches `owned_by`, in start order, recording the
    /// cancelled positions so the caller commits them after planning
    /// succeeds (SCXML 6.4).
    fn push_cancellations(
        &self,
        items: &mut VecDeque<PlanItem>,
        owned_by: impl Fn(usize) -> bool,
        cancelled: &mut Vec<usize>,
        context: &EffectContext,
    ) -> Result<(), EngineError> {
        for (position, active) in self.active_invocations.iter().enumerate() {
            if !owned_by(active.state) || cancelled.contains(&position) {
                continue;
            }
            if scxml_types::exceeds_wasm_bound(
                items.len().saturating_add(1),
                MAX_EFFECTS_PER_COMMAND,
            ) {
                return Err(EngineError::EffectLimit);
            }
            items.push_back(PlanItem::Emit {
                effect: Effect::CancelInvoke {
                    invoke_id: active.invoke_id.clone(),
                },
                context: context.clone(),
            });
            cancelled.push(position);
        }
        Ok(())
    }

    /// Whether one state is a `<history>` pseudostate. W3C `getChildStates`
    /// excludes history, so a history child of a `<parallel>` is never a
    /// region: it is neither entered by default, required to finish, nor
    /// checked for coverage.
    fn is_history(&self, index: usize) -> Result<bool, EngineError> {
        Ok(matches!(
            self.state_by_index(index)?.kind,
            StateKind::ShallowHistory | StateKind::DeepHistory
        ))
    }

    /// W3C `isInFinalState` for one state against a candidate configuration
    /// given as a membership table (`config[index]`): compound states
    /// finalize when one final child is a member; parallel states finalize
    /// when every region finalizes; leaves never do.
    fn is_in_final_state(&self, index: usize, config: &[bool]) -> Result<bool, EngineError> {
        let state = self.state_by_index(index)?;
        match state.kind {
            StateKind::Compound => {
                for &child in &state.children {
                    let child_index =
                        usize::try_from(child).map_err(|_| EngineError::BrokenInvariant)?;
                    if self.state_by_index(child_index)?.kind == StateKind::Final
                        && config.get(child_index).copied().unwrap_or(false)
                    {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            StateKind::Parallel => self.regions_all_in_final_state(index, config),
            StateKind::Atomic
            | StateKind::Final
            | StateKind::ShallowHistory
            | StateKind::DeepHistory => Ok(false),
        }
    }
    /// Whether every child region of one parallel state is in a final state
    /// with respect to the given configuration.
    /// [`Self::regions_all_in_final_state`] for a configuration that only
    /// grows between calls: regions before `*cursor` are already known to be
    /// final, and `*cursor` advances past every region found final.
    fn regions_all_final_from(
        &self,
        parallel: usize,
        config: &[bool],
        cursor: &mut usize,
    ) -> Result<bool, EngineError> {
        let children = &self.state_by_index(parallel)?.children;
        while let Some(&child) = children.get(*cursor) {
            let child_index = usize::try_from(child).map_err(|_| EngineError::BrokenInvariant)?;
            if !self.is_history(child_index)? && !self.is_in_final_state(child_index, config)? {
                return Ok(false);
            }
            *cursor = cursor.saturating_add(1);
        }
        Ok(true)
    }

    fn regions_all_in_final_state(
        &self,
        parallel: usize,
        config: &[bool],
    ) -> Result<bool, EngineError> {
        let state = self.state_by_index(parallel)?;
        for &child in &state.children {
            let child_index = usize::try_from(child).map_err(|_| EngineError::BrokenInvariant)?;
            if self.is_history(child_index)? {
                continue;
            }
            if !self.is_in_final_state(child_index, config)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn restore_internal_prefix(&mut self, consumed: Vec<Event>) -> Result<(), EngineError> {
        let restored_len = self
            .internal
            .len()
            .checked_add(consumed.len())
            .ok_or(EngineError::BrokenInvariant)?;
        if scxml_types::exceeds_wasm_bound(restored_len, MAX_NESTED_ITEMS) {
            return Err(EngineError::BrokenInvariant);
        }
        for event in consumed.into_iter().rev() {
            self.internal.push_front(event);
        }
        Ok(())
    }

    fn state_by_index(&self, index: usize) -> Result<&StateIr, EngineError> {
        count_work(1);
        self.machine
            .states
            .get(index)
            .ok_or(EngineError::BrokenInvariant)
    }

    fn transition(&self, index: u32) -> Result<&TransitionIr, EngineError> {
        count_work(1);
        usize::try_from(index)
            .ok()
            .and_then(|index| self.machine.transitions.get(index))
            .ok_or(EngineError::BrokenInvariant)
    }

    fn state_index_by_id(&self, id: &StateId) -> Result<usize, EngineError> {
        self.declared_state_index(id)
            .ok_or(EngineError::BrokenInvariant)
    }

    fn declared_state_index(&self, id: &StateId) -> Option<usize> {
        count_work(1);
        self.state_indices.get(id.as_str()).copied()
    }

    /// Active leaves sorted in document order.
    fn active_leaf_indices(&self) -> Result<Vec<usize>, EngineError> {
        let mut leaves = Vec::with_capacity(self.active_states.len());
        for id in &self.active_states {
            leaves.push(self.state_index_by_id(id)?);
        }
        sort_document_order(&mut leaves);
        Ok(leaves)
    }

    /// The active atomic states in document order: the configuration
    /// leaves except a `<parallel>` without regions. SCXML 3.13 enables
    /// transitions only "in atomic state S", and a `<parallel>` is never
    /// atomic (SCXML 3.3: "An atomic state is a <state> that has no
    /// <state>, <parallel> or <final> children"; Appendix D counts a
    /// `<final>` as atomic too).
    fn active_atomic_indices(&self) -> Result<Vec<usize>, EngineError> {
        let mut atomic = self.active_leaf_indices()?;
        atomic.retain(|&index| {
            self.machine
                .states
                .get(index)
                .is_some_and(|state| matches!(state.kind, StateKind::Atomic | StateKind::Final))
        });
        Ok(atomic)
    }

    fn state_id_by_index(&self, index: usize) -> Result<StateId, EngineError> {
        state_id(self.state_by_index(index)?)
    }

    /// Proper ancestors of one state from the nearest parent up to the root.
    fn proper_ancestor_indices(&self, index: usize) -> Result<Vec<usize>, EngineError> {
        let mut chain = Vec::new();
        let mut current = index;
        while let Some(parent) = self.state_by_index(current)?.parent {
            let parent_index = usize::try_from(parent).map_err(|_| EngineError::BrokenInvariant)?;
            if parent_index >= self.machine.states.len()
                || scxml_types::exceeds_wasm_bound(
                    chain.len().saturating_add(1),
                    MAX_HIERARCHY_DEPTH,
                )
            {
                return Err(EngineError::BrokenInvariant);
            }
            chain.push(parent_index);
            current = parent_index;
        }
        Ok(chain)
    }

    /// Whether `candidate` is a strict descendant of `ancestor`.
    fn is_proper_descendant(&self, candidate: usize, ancestor: usize) -> Result<bool, EngineError> {
        let mut chain_len = 0_usize;
        let mut current = candidate;
        while let Some(parent) = self.state_by_index(current)?.parent {
            let parent_index = usize::try_from(parent).map_err(|_| EngineError::BrokenInvariant)?;
            if parent_index >= self.machine.states.len()
                || scxml_types::exceeds_wasm_bound(chain_len.saturating_add(1), MAX_HIERARCHY_DEPTH)
            {
                return Err(EngineError::BrokenInvariant);
            }
            if parent_index == ancestor {
                return Ok(true);
            }
            chain_len = chain_len
                .checked_add(1)
                .ok_or(EngineError::BrokenInvariant)?;
            current = parent_index;
        }
        Ok(false)
    }

    /// The active configuration: the union of every active leaf's ancestor
    /// chain (SCXML 3.4), each chain walked only up to the first member an
    /// earlier leaf already added, so building it is linear in the
    /// configuration size.
    fn active_configuration(&self) -> Result<ActiveConfiguration, EngineError> {
        let mut member = vec![false; self.machine.states.len()];
        let mut ordered = Vec::new();
        for leaf in self.active_leaf_indices()? {
            let mut current = leaf;
            let mut depth = 0_usize;
            loop {
                let slot = member
                    .get_mut(current)
                    .ok_or(EngineError::BrokenInvariant)?;
                if *slot {
                    break;
                }
                *slot = true;
                ordered.push(current);
                match self.state_by_index(current)?.parent {
                    Some(parent) => {
                        depth = depth.saturating_add(1);
                        if scxml_types::exceeds_wasm_bound(depth, MAX_HIERARCHY_DEPTH) {
                            return Err(EngineError::BrokenInvariant);
                        }
                        current =
                            usize::try_from(parent).map_err(|_| EngineError::BrokenInvariant)?;
                    }
                    None => break,
                }
            }
        }
        sort_document_order(&mut ordered);
        Ok(ActiveConfiguration { member, ordered })
    }

    /// Full active configuration as root-first indices in document order:
    /// the union of every active leaf's ancestor chain (SCXML 3.4).
    fn configuration_indices_all(&self) -> Result<Vec<usize>, EngineError> {
        Ok(self.active_configuration()?.ordered)
    }

    /// Full active configuration as root-first state ids in document order.
    fn configuration_ids_all(&self) -> Result<Vec<StateId>, EngineError> {
        self.configuration_indices_all()?
            .iter()
            .map(|&index| self.state_id_by_index(index))
            .collect()
    }

    /// W3C transition domain (SCXML `getTransitionDomain`): the source for
    /// internal transitions of a compound source whose targets are all its
    /// descendants, otherwise the least common compound ancestor of the
    /// source and every target (`findLCCA`; parallels are never domains).
    /// `None` is the virtual document root used when no concrete common
    /// ancestor exists (sibling top-level states).
    fn transition_domain(&self, transition: &TransitionIr) -> Result<Option<usize>, EngineError> {
        let source =
            usize::try_from(transition.source).map_err(|_| EngineError::BrokenInvariant)?;
        if transition.transition_type == TransitionType::Internal
            && !transition.targets.is_empty()
            && self.state_by_index(source)?.kind == StateKind::Compound
        {
            let all_descendants = transition.targets.iter().try_fold(true, |all, &target| {
                let target_index =
                    usize::try_from(target).map_err(|_| EngineError::BrokenInvariant)?;
                Ok(all && self.is_proper_descendant(target_index, source)?)
            })?;
            if all_descendants {
                return Ok(Some(source));
            }
        }
        let mut participants = vec![source];
        for &target in &transition.targets {
            participants.push(usize::try_from(target).map_err(|_| EngineError::BrokenInvariant)?);
        }
        self.find_least_common_compound_ancestor(&participants)
    }

    /// The history values that exiting `exits` records (SCXML 3.11), taken
    /// against the current, pre-exit configuration, exactly as W3C
    /// `exitStates` records history before the exited states leave: deep
    /// history stores every atomic/final leaf that is a proper descendant
    /// of the exited state; shallow history stores its direct children that
    /// were active. Bounded by the IR nested-item limit per value.
    fn recorded_history(&self, exits: &[usize]) -> Result<Vec<(usize, Vec<usize>)>, EngineError> {
        let mut recorded = Vec::new();
        let pre_exit_configuration = self.configuration_indices_all()?;
        for &exited in exits {
            let history_children: Vec<(usize, bool)> = self
                .state_by_index(exited)?
                .children
                .iter()
                .map(|&child| {
                    let child_index =
                        usize::try_from(child).map_err(|_| EngineError::BrokenInvariant)?;
                    Ok(match self.state_by_index(child_index)?.kind {
                        StateKind::DeepHistory => Some((child_index, true)),
                        StateKind::ShallowHistory => Some((child_index, false)),
                        _ => None,
                    })
                })
                .collect::<Result<Vec<Option<(usize, bool)>>, EngineError>>()?
                .into_iter()
                .flatten()
                .collect();
            for (child_index, deep) in history_children {
                let mut stored = Vec::new();
                for &member in &pre_exit_configuration {
                    let member_active = if deep {
                        matches!(
                            self.state_by_index(member)?.kind,
                            StateKind::Atomic | StateKind::Final
                        ) && self.is_proper_descendant(member, exited)?
                    } else {
                        self.state_by_index(member)?.parent
                            == Some(
                                u32::try_from(exited).map_err(|_| EngineError::BrokenInvariant)?,
                            )
                    };
                    if member_active {
                        stored.push(member);
                        if scxml_types::exceeds_wasm_bound(stored.len(), MAX_NESTED_ITEMS) {
                            return Err(EngineError::BrokenInvariant);
                        }
                    }
                }
                recorded.push((child_index, stored));
            }
        }
        Ok(recorded)
    }

    /// W3C `computeEntrySet` for one target list and its transition domain:
    /// every target's default descent plus every ancestor up to the domain,
    /// completing every parallel region encountered along the way.
    fn compute_entry_set(
        &self,
        targets: &[usize],
        domain: Option<usize>,
        history: &HistoryValues,
    ) -> Result<EntryPlan, EngineError> {
        let mut plan = EntryPlan::default();
        for &target in targets {
            self.add_descendant_states_to_enter(target, history, &mut plan, 0)?;
        }
        for &target in targets {
            self.add_ancestor_states_to_enter(target, domain, history, &mut plan)?;
        }
        Ok(plan)
    }

    /// W3C `addDescendantStatesToEnter`: adds the state itself, the default
    /// descent of a compound state, or every uncovered region of a parallel
    /// state. History states resolve through the stored history value or,
    /// absent one, through their fallback transition whose content becomes
    /// the parent's default history content (SCXML 3.11); the history state
    /// itself never joins the entry set.
    fn add_descendant_states_to_enter(
        &self,
        state: usize,
        history: &HistoryValues,
        plan: &mut EntryPlan,
        depth: usize,
    ) -> Result<(), EngineError> {
        if scxml_types::exceeds_wasm_bound(depth, MAX_HIERARCHY_DEPTH)
            || state >= self.machine.states.len()
        {
            return Err(EngineError::BrokenInvariant);
        }
        let state_ref = self.state_by_index(state)?;
        if matches!(
            state_ref.kind,
            StateKind::ShallowHistory | StateKind::DeepHistory
        ) {
            let parent = state_ref.parent.ok_or(EngineError::BrokenInvariant)?;
            let parent_index = usize::try_from(parent).map_err(|_| EngineError::BrokenInvariant)?;
            if let Some(stored) = history.get(&state) {
                for &member in stored {
                    self.add_descendant_states_to_enter(member, history, plan, depth + 1)?;
                }
                for &member in stored {
                    self.add_ancestor_states_to_enter(member, Some(parent_index), history, plan)?;
                }
            } else {
                // No stored value: the fallback transition's content is
                // the parent's default history content and its targets
                // seed the entry set (SCXML 3.11; official case 579).
                let fallback_index = state_ref
                    .transitions
                    .first()
                    .ok_or(EngineError::BrokenInvariant)?;
                let fallback = self.transition(*fallback_index)?;
                plan.insert_default_history_content(parent_index, fallback.actions.clone())?;
                for &target in &fallback.targets {
                    let target_index =
                        usize::try_from(target).map_err(|_| EngineError::BrokenInvariant)?;
                    self.add_descendant_states_to_enter(target_index, history, plan, depth + 1)?;
                }
                for &target in &fallback.targets {
                    let target_index =
                        usize::try_from(target).map_err(|_| EngineError::BrokenInvariant)?;
                    self.add_ancestor_states_to_enter(
                        target_index,
                        Some(parent_index),
                        history,
                        plan,
                    )?;
                }
            }
            return Ok(());
        }
        plan.insert(state, &self.machine.states)?;
        let state_ref = self.state_by_index(state)?;
        if state_ref.kind == StateKind::Compound {
            let initial = state_ref
                .initial
                .as_ref()
                .ok_or(EngineError::BrokenInvariant)?;
            plan.insert_default_entry(state)?;
            for &target in &initial.targets {
                let target_index =
                    usize::try_from(target).map_err(|_| EngineError::BrokenInvariant)?;
                self.add_descendant_states_to_enter(target_index, history, plan, depth + 1)?;
            }
            for &target in &initial.targets {
                let target_index =
                    usize::try_from(target).map_err(|_| EngineError::BrokenInvariant)?;
                self.add_ancestor_states_to_enter(target_index, Some(state), history, plan)?;
            }
        } else if state_ref.kind == StateKind::Parallel {
            self.complete_parallel_regions(state, history, plan, depth + 1)?;
        }
        Ok(())
    }

    /// Adds the default descent of every region of `parallel` that no state
    /// to enter covers yet (the region loop shared by W3C
    /// `addDescendantStatesToEnter` and `addAncestorStatesToEnter`); history
    /// children are not regions. `depth` is the descent depth of the regions.
    fn complete_parallel_regions(
        &self,
        parallel: usize,
        history: &HistoryValues,
        plan: &mut EntryPlan,
        depth: usize,
    ) -> Result<(), EngineError> {
        if !plan.completed_parallels.insert(parallel) {
            return Ok(());
        }
        for &child in &self.state_by_index(parallel)?.children {
            let child_index = usize::try_from(child).map_err(|_| EngineError::BrokenInvariant)?;
            if self.is_history(child_index)? || plan.covers(child_index) {
                continue;
            }
            self.add_descendant_states_to_enter(child_index, history, plan, depth)?;
        }
        Ok(())
    }

    /// W3C `addAncestorStatesToEnter`: adds every proper ancestor of the
    /// state up to (excluding) the domain; parallel ancestors complete their
    /// uncovered regions.
    fn add_ancestor_states_to_enter(
        &self,
        state: usize,
        ancestor: Option<usize>,
        history: &HistoryValues,
        plan: &mut EntryPlan,
    ) -> Result<(), EngineError> {
        for anc in self.proper_ancestor_indices(state)? {
            if Some(anc) == ancestor {
                break;
            }
            plan.insert(anc, &self.machine.states)?;
            if self.state_by_index(anc)?.kind == StateKind::Parallel {
                self.complete_parallel_regions(anc, history, plan, 0)?;
            }
        }
        Ok(())
    }

    /// W3C `findLCCA`: the nearest proper compound ancestor of the first
    /// participant that contains every participant. Parallel states are
    /// never domains, so cross-region transitions re-enter sibling regions.
    fn find_least_common_compound_ancestor(
        &self,
        participants: &[usize],
    ) -> Result<Option<usize>, EngineError> {
        let first = *participants.first().ok_or(EngineError::BrokenInvariant)?;
        for candidate in self.proper_ancestor_indices(first)? {
            if self.state_by_index(candidate)?.kind != StateKind::Compound {
                continue;
            }
            // The domain must be a proper ancestor of every participant
            // (W3C getLCCA over proper-ancestor lists); a participant
            // itself never qualifies, so a transition targeting its own
            // ancestor exits that ancestor (official case 579).
            let covers_all = participants.iter().try_fold(true, |all, &participant| {
                Ok(all && self.is_proper_descendant(participant, candidate)?)
            })?;
            if covers_all {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    /// Whether the entry path enters a top-level final state.
    fn enters_top_level_final(&self, entry_path: &[usize]) -> Result<bool, EngineError> {
        entry_path.iter().try_fold(false, |found, &index| {
            let state = self.state_by_index(index)?;
            Ok(found || (state.kind == StateKind::Final && state.parent.is_none()))
        })
    }

    /// Whether the active configuration contains an eventless transition on
    /// any member (leaf or ancestor) of any active region.
    fn configuration_has_eventless_transition_all(&self) -> Result<bool, EngineError> {
        for index in self.configuration_indices_all()? {
            let state = self.state_by_index(index)?;
            for transition_index in &state.transitions {
                let transition = self.transition(*transition_index)?;
                if transition.events.is_empty() {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

/// Whether `state` is a leaf of an active configuration: an atomic or final
/// state, or a `<parallel>` without regions (SCXML 3.4.2 lets its `<state>`
/// children occur "0 or more times"; history children are not regions,
/// W3C `getChildStates`).
fn is_configuration_leaf(state: &StateIr, states: &[StateIr]) -> bool {
    match state.kind {
        StateKind::Atomic | StateKind::Final => true,
        StateKind::Parallel => !state.children.iter().any(|&child| {
            usize::try_from(child)
                .ok()
                .and_then(|child| states.get(child))
                .is_some_and(|child| {
                    !matches!(
                        child.kind,
                        StateKind::ShallowHistory | StateKind::DeepHistory
                    )
                })
        }),
        StateKind::Compound | StateKind::ShallowHistory | StateKind::DeepHistory => false,
    }
}

/// Sorts state table indices into document order.
///
/// [`MachineIr::validate`], which [`Engine::new`] requires, rejects any table
/// whose state document order does not strictly increase with the table
/// index, so table order is document order and sorting needs no table reads.
fn sort_document_order(indices: &mut [usize]) {
    indices.sort_unstable();
}

#[cfg(test)]
thread_local! {
    /// Test-only count of engine work units on this thread (state and
    /// transition table reads, plus the configuration entries test executors
    /// receive): a deterministic, wall-clock-independent measure of work.
    static ENGINE_WORK: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Records `units` units of work for the test-only work counter; a no-op
/// outside tests.
#[inline]
fn count_work(units: usize) {
    #[cfg(test)]
    ENGINE_WORK.with(|counter| {
        counter.set(
            counter
                .get()
                .saturating_add(u64::try_from(units).unwrap_or(u64::MAX)),
        );
    });
    #[cfg(not(test))]
    let _ = units;
}

fn validate_supported_model(machine: &MachineIr) -> Result<(), EngineError> {
    if machine.initial.targets.is_empty() {
        return Err(EngineError::UnsupportedModel);
    }
    validate_actions(&machine.initial.actions)?;
    for block in &machine.scripts {
        validate_actions(&block.actions)?;
    }
    for state in &machine.states {
        // Every engine-visible state carries an id: lowering generates one
        // for each state or history the author left without (SCXML 3.14),
        // so only hand-built IR reaches this rejection.
        if state.id.is_none() {
            return Err(EngineError::UnsupportedModel);
        }
        // `<invoke>` declarations stay inert inside the engine: the
        // hosting session adapter reads them through `machine()` and
        // owns invoked-session lifecycle (SCXML 6.4), so invoke-bearing
        // machines remain valid engine models.
        match state.kind {
            StateKind::Compound => {
                if state.children.is_empty() || state.initial.is_none() {
                    return Err(EngineError::UnsupportedModel);
                }
            }
            StateKind::Atomic | StateKind::Final => {
                if !state.children.is_empty() {
                    return Err(EngineError::UnsupportedModel);
                }
            }
            // SCXML 3.4.2: a `<parallel>` has "0 or more" regions; one
            // without any is a configuration leaf (`is_configuration_leaf`).
            StateKind::Parallel | StateKind::ShallowHistory | StateKind::DeepHistory => {}
        }
        for block in state.onentry.iter().chain(&state.onexit) {
            validate_actions(&block.actions)?;
        }
    }
    // Multi-target transitions need no engine-side gate: `MachineIr::validate`
    // already rejects target sets that are not orthogonal (distinct
    // regions of one parallel, SCXML 3.13; validator SCXML030).
    for transition in &machine.transitions {
        validate_actions(&transition.actions)?;
    }
    Ok(())
}

fn validate_actions(actions: &[ExecutableContentIr]) -> Result<(), EngineError> {
    validate_actions_bounded(actions, 0)
}

fn validate_actions_bounded(
    actions: &[ExecutableContentIr],
    depth: usize,
) -> Result<(), EngineError> {
    validate_actions_bounded_inner(actions, depth)
}

fn validate_actions_bounded_inner(
    actions: &[ExecutableContentIr],
    depth: usize,
) -> Result<(), EngineError> {
    if scxml_types::exceeds_wasm_bound(depth, crate::MAX_HIERARCHY_DEPTH) {
        return Err(EngineError::UnsupportedModel);
    }
    actions.iter().try_for_each(|action| match action {
        ExecutableContentIr::Foreach { actions, .. } => {
            validate_actions_bounded(actions, depth.saturating_add(1))
        }
        ExecutableContentIr::If {
            actions,
            elseif: else_if,
            else_actions,
            ..
        } => {
            validate_actions_bounded(actions, depth.saturating_add(1))?;
            for branch in else_if {
                validate_actions_bounded(&branch.actions, depth.saturating_add(1))?;
            }
            if let Some(actions) = else_actions {
                validate_actions_bounded(actions, depth.saturating_add(1))?;
            }
            Ok(())
        }
        ExecutableContentIr::Raise { .. }
        | ExecutableContentIr::Log { .. }
        | ExecutableContentIr::Assign { .. }
        | ExecutableContentIr::Send { .. }
        | ExecutableContentIr::Cancel { .. }
        | ExecutableContentIr::Script { .. }
        | ExecutableContentIr::Extension { .. } => Ok(()),
    })
}

fn transition_matches(transition: &TransitionIr, event: Option<&Event>) -> bool {
    match event {
        None => transition.events.is_empty(),
        Some(event) => transition
            .events
            .iter()
            .any(|descriptor| descriptor_matches(descriptor, event.name.as_str())),
    }
}

/// Token trie of every transition event descriptor, answering whether any
/// descriptor can match an event name in time linear in its tokens.
#[derive(Debug, Default)]
struct EventIndex {
    /// A descriptor ends here: it matches this token prefix and every
    /// event below it (the root: `*` or an empty prefix, which match all).
    terminal: bool,
    children: std::collections::BTreeMap<String, EventIndex>,
}

impl EventIndex {
    /// Records one descriptor, normalized as [`descriptor_matches`] does.
    fn insert(&mut self, descriptor: &str) {
        let prefix = descriptor
            .strip_suffix(".*")
            .or_else(|| descriptor.strip_suffix('.'))
            .unwrap_or(descriptor);
        let mut node = self;
        if descriptor != "*" && !prefix.is_empty() {
            for token in prefix.split('.') {
                node = node.children.entry(token.to_owned()).or_default();
            }
        }
        node.terminal = true;
    }

    /// Whether some recorded descriptor matches `event`
    /// ([`descriptor_matches`]).
    fn may_match(&self, event: &str) -> bool {
        let mut node = self;
        if node.terminal {
            return true;
        }
        for token in event.split('.') {
            match node.children.get(token) {
                Some(child) if child.terminal => return true,
                Some(child) => node = child,
                None => return false,
            }
        }
        false
    }
}

fn descriptor_matches(descriptor: &str, event: &str) -> bool {
    let prefix = descriptor
        .strip_suffix(".*")
        .or_else(|| descriptor.strip_suffix('.'))
        .unwrap_or(descriptor);
    descriptor == "*"
        || prefix.is_empty()
        || prefix == event
        || event
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('.'))
}

fn state_id(state: &StateIr) -> Result<StateId, EngineError> {
    state
        .id
        .clone()
        .ok_or(EngineError::BrokenInvariant)
        .and_then(|id| StateId::new(id).map_err(|_| EngineError::BrokenInvariant))
}

/// Number of actions in one executable-content list that each plan exactly
/// one effect when reached: every action except `<if>`, whose selected
/// branch is planned (and bounded) separately. A pure count inside the
/// enclosing engine command; it emits no telemetry of its own.
fn direct_effect_count(actions: &[ExecutableContentIr]) -> usize {
    actions
        .iter()
        .filter(|action| !matches!(action, ExecutableContentIr::If { .. }))
        .count()
}

/// Removes every plan item owned by `block` from one plan frame,
/// recursively through nested sub-frames, so a failed block's remaining
/// deferred work (branch bodies, foreach iterations) never executes while
/// other blocks' work is kept in order (SCXML 3.4/5.7).
fn retain_outside_block(frame: &mut VecDeque<PlanItem>, block: u64) {
    frame.retain_mut(|item| match item {
        PlanItem::Effect { context, .. } | PlanItem::Emit { context, .. } => context.block != block,
        PlanItem::Decide(deferred) => deferred.context.block != block,
        PlanItem::Iterate(iteration) => iteration.context.block != block,
        PlanItem::Frame(sub_frame) => {
            retain_outside_block(sub_frame, block);
            !sub_frame.is_empty()
        }
    });
}

/// Pushes one ordered action list into plan items with one shared context.
fn flatten_actions(
    items: &mut VecDeque<PlanItem>,
    actions: &[ExecutableContentIr],
    context: &EffectContext,
) -> Result<(), EngineError> {
    for action in actions {
        push_effect_item(items, action.clone(), context)?;
    }
    Ok(())
}

/// Pushes ordered action blocks into plan items with one shared context.
fn flatten_blocks(
    items: &mut VecDeque<PlanItem>,
    blocks: &[scxml_compiler_core::ActionBlockIr],
    context: &EffectContext,
    block_counter: &mut u64,
) -> Result<(), EngineError> {
    blocks.iter().try_for_each(|block_actions| {
        let block_context = EffectContext {
            block: *block_counter,
            ..context.clone()
        };
        *block_counter = block_counter.saturating_add(1);
        flatten_actions(items, &block_actions.actions, &block_context)
    })
}

/// Pushes one effect plan item while enforcing the per-command effect bound.
fn push_effect_item(
    items: &mut VecDeque<PlanItem>,
    action: ExecutableContentIr,
    context: &EffectContext,
) -> Result<(), EngineError> {
    ensure_effect_room(items.len())?;
    items.push_back(PlanItem::Effect {
        action,
        context: context.clone(),
    });
    Ok(())
}

/// Plans one macrostep's flattened work.
///
/// Effects are planned in document order until the first `<if>` decision, whose
/// branch selection is deferred to the effect-execution point. The remaining
/// work stays in [`PendingPlan`] frames for [`Engine::continue_effects`].
/// Handler-block abort scopes end the batch without being consumed.
#[allow(
    clippy::too_many_lines,
    reason = "one dispatch covers every plan item shape in document order; splitting it would hide the ordering contract"
)]
fn plan_step(items: VecDeque<PlanItem>) -> Result<MacrostepPlan, EngineError> {
    let mut plan = PendingPlan {
        frames: vec![items],
    };
    let mut effects = Vec::new();
    loop {
        let boundary = plan.frames.last().is_some_and(|frame| {
            matches!(
                frame.front(),
                Some(PlanItem::Decide(_) | PlanItem::Iterate(_) | PlanItem::Frame(_))
            )
        });
        if boundary {
            break;
        }
        let item = plan.frames.last_mut().and_then(VecDeque::pop_front);
        match item {
            None => {
                plan.frames.pop();
                if plan.frames.is_empty() {
                    break;
                }
            }
            Some(PlanItem::Decide(_) | PlanItem::Iterate(_)) => {
                // Boundary items are handled exclusively by
                // resolve_plan_boundary above.
                return Err(EngineError::BrokenInvariant);
            }
            Some(PlanItem::Frame(frame)) => {
                plan.frames.push(frame);
            }
            Some(PlanItem::Emit { effect, context }) => {
                ensure_effect_room(effects.len())?;
                effects.push(PlannedEffect { effect, context });
            }
            Some(PlanItem::Effect { action, context }) => {
                if let ExecutableContentIr::If {
                    cond: condition,
                    actions,
                    elseif: else_if,
                    else_actions,
                } = action
                {
                    plan.frames
                        .last_mut()
                        .ok_or(EngineError::BrokenInvariant)?
                        .push_front(PlanItem::Decide(Box::new(DeferredConditional {
                            condition,
                            branch: actions,
                            else_if: else_if.into(),
                            else_actions,
                            context,
                        })));
                    break;
                }
                if let ExecutableContentIr::Foreach {
                    array,
                    item,
                    index,
                    actions,
                } = action
                {
                    ensure_effect_room(effects.len())?;
                    effects.push(PlannedEffect {
                        effect: Effect::Iterate {
                            array_expression: array,
                            item_location: item.clone(),
                            index_location: index.clone(),
                        },
                        context: context.clone(),
                    });
                    plan.frames
                        .last_mut()
                        .ok_or(EngineError::BrokenInvariant)?
                        .push_front(PlanItem::Iterate(Box::new(ActiveIteration {
                            item_location: item,
                            index_location: index,
                            elements: VecDeque::new(),
                            next_index: 0,
                            body: actions,
                            context,
                        })));
                    break;
                }
                ensure_effect_room(effects.len())?;
                let effect = action_effect(action)?;
                if let Some(effect) = effect {
                    effects.push(PlannedEffect { effect, context });
                }
            }
        }
    }
    Ok(MacrostepPlan { effects, plan })
}

/// The data-model placement of an [`Effect::Assign`] from its `XPath`
/// `assign_type` and `attr` (SCXML 1.0 CR 2014-03-13 §C.3.6; an absent
/// `assign_type` is `replacechildren`).
///
/// # Errors
///
/// [`EngineError::BrokenInvariant`] when `attr` is present exactly when the
/// type is not `addattribute`; validated Machine IR never produces that.
pub fn assign_placement(
    assign_type: Option<AssignTypeIr>,
    attr: Option<&str>,
) -> Result<AssignPlacement, EngineError> {
    Ok(match (assign_type, attr) {
        (Some(AssignTypeIr::Addattribute), Some(name)) => {
            AssignPlacement::AddAttribute(name.to_owned())
        }
        (Some(AssignTypeIr::Addattribute), None) | (_, Some(_)) => {
            return Err(EngineError::BrokenInvariant);
        }
        (None | Some(AssignTypeIr::Replacechildren), None) => AssignPlacement::ReplaceChildren,
        (Some(AssignTypeIr::Firstchild), None) => AssignPlacement::FirstChild,
        (Some(AssignTypeIr::Lastchild), None) => AssignPlacement::LastChild,
        (Some(AssignTypeIr::Previoussibling), None) => AssignPlacement::PreviousSibling,
        (Some(AssignTypeIr::Nextsibling), None) => AssignPlacement::NextSibling,
        (Some(AssignTypeIr::Replace), None) => AssignPlacement::Replace,
        (Some(AssignTypeIr::Delete), None) => AssignPlacement::Delete,
    })
}

/// The data-model source of an [`Effect::Assign`]: the materialized inline
/// content when present, else the `expr` expression for the data model to
/// evaluate and assign in one transaction, else [`DataValue::Absent`] (the
/// value-less B.2 `delete`).
#[must_use]
pub fn assign_source(expression: Option<&str>, content: Option<&DataValue>) -> AssignSource {
    match (content, expression) {
        (Some(content), _) => AssignSource::Value(content.clone()),
        (None, Some(expression)) => AssignSource::Expression(expression.to_owned()),
        (None, None) => AssignSource::Value(DataValue::Absent),
    }
}

/// Maps one executable-content action to its observable effect.
fn action_effect(action: ExecutableContentIr) -> Result<Option<Effect>, EngineError> {
    match action {
        ExecutableContentIr::Raise {
            event,
            event_schema,
        } => Ok(Some(Effect::Raise(event, true, event_schema))),
        ExecutableContentIr::Log { label, expr } => Ok(Some(Effect::Log {
            label,
            expression: expr,
        })),
        ExecutableContentIr::Assign {
            location,
            expr,
            body,
            assign_type,
            attr,
        } => {
            // SCXML 5.8 inline content is materialized once, without
            // expression evaluation: element children are an XML value
            // (namespace-correct markup the data model parses), while
            // text-only children stay the plain string they denote.
            let content = if body.is_empty() {
                None
            } else if body
                .iter()
                .any(|node| matches!(node, XmlNodeIr::Element { .. }))
            {
                Some(DataValue::Xml(
                    scxml_compiler_core::xml_markup(&body)
                        .map_err(|_| EngineError::BrokenInvariant)?,
                ))
            } else {
                let mut text = String::new();
                for node in &body {
                    if let XmlNodeIr::Text { value } = node {
                        if scxml_types::exceeds_wasm_bound(
                            text.len().saturating_add(value.len()),
                            MAX_STRING_BYTES,
                        ) {
                            return Err(EngineError::BrokenInvariant);
                        }
                        text.push_str(value);
                    }
                }
                Some(DataValue::String(text))
            };
            Ok(Some(Effect::Assign {
                location,
                expression: expr,
                content,
                assign_type,
                attr,
            }))
        }
        ExecutableContentIr::Foreach {
            array, item, index, ..
        } => Ok(Some(Effect::Iterate {
            array_expression: array,
            item_location: item,
            index_location: index,
        })),
        ExecutableContentIr::Send { value } => Ok(Some(Effect::Send(value))),
        ExecutableContentIr::Cancel {
            sendid: send_id,
            sendidexpr: send_id_expr,
        } => Ok(Some(Effect::Cancel {
            send_id,
            send_id_expression: send_id_expr,
        })),
        ExecutableContentIr::Script { src: None, body } => Ok(Some(Effect::ExecuteScript(body))),
        // Pinned data-model-language text is embedded and authenticated by
        // the IR; everything else is resolved by the host at document load.
        ExecutableContentIr::Script {
            src: Some(src),
            body,
        } => Ok(Some(if src.requires_host_load() {
            Effect::RunScript(src)
        } else {
            Effect::ExecuteScript(body)
        })),
        ExecutableContentIr::Extension { element } => Ok(Some(Effect::ExecuteExtension(element))),
        ExecutableContentIr::If { .. } => Err(EngineError::UnsupportedModel),
    }
}

fn complete<T>(span: &scxml_telemetry::FunctionSpan, result: &Result<T, EngineError>) {
    match result {
        Ok(_) => span.event(SignalEvent::TransformationCompleted),
        Err(error) => {
            span.event(SignalEvent::InputRejected);
            span.fail(match error {
                EngineError::InvalidModel => ErrorCategory::InvalidInput,
                EngineError::UnsupportedModel
                | EngineError::Evaluation(EvaluationError::Unavailable) => {
                    ErrorCategory::Unsupported
                }
                EngineError::Evaluation(EvaluationError::Platform) => ErrorCategory::Platform,
                EngineError::Evaluation(_)
                | EngineError::AlreadyStarted
                | EngineError::NotStarted
                | EngineError::Finished
                | EngineError::NotStable
                | EngineError::BrokenInvariant
                | EngineError::QueueLimit
                | EngineError::EffectLimit
                | EngineError::EffectProtocol
                | EngineError::UnknownBlock => ErrorCategory::Execution,
            });
        }
    }
}

/// Result of one internal-queue scan inside [`Engine::resume`].
enum InternalScan {
    /// The scan produced the command's step (a microstep, or a bounded
    /// partial scan).
    CommandResult(Box<CommandResult>),
    /// The internal queue drained without enabling a transition; the
    /// ignored events in FIFO order.
    Drained(Vec<DisposedEvent>),
}

/// Expected engine command failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineError {
    /// Canonical IR failed validation.
    InvalidModel,
    /// Valid IR uses semantics outside this bounded increment.
    UnsupportedModel,
    /// `start` was called more than once.
    AlreadyStarted,
    /// An event was sent before `start`.
    NotStarted,
    /// The machine is already final.
    Finished,
    /// An external event was offered before stabilization.
    NotStable,
    /// A validated model invariant was unexpectedly broken.
    BrokenInvariant,
    /// Guard evaluation failed.
    Evaluation(EvaluationError),
    /// Internal queue capacity was exhausted.
    QueueLimit,
    /// One engine command exceeded the effect bound.
    EffectLimit,
    /// The capability-side effect phase was stale, nested, incomplete, or out of order.
    EffectProtocol,
    /// A block ordinal named by the capability side was not issued in the
    /// current effect phase.
    UnknownBlock,
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidModel => formatter.write_str("Machine IR is invalid"),
            Self::UnsupportedModel => {
                formatter.write_str("Machine IR uses unsupported runtime semantics")
            }
            Self::AlreadyStarted => formatter.write_str("engine is already started"),
            Self::NotStarted => formatter.write_str("engine is not started"),
            Self::Finished => formatter.write_str("engine is finished"),
            Self::NotStable => formatter.write_str("engine has pending internal work"),
            Self::BrokenInvariant => formatter.write_str("compiled machine invariant is broken"),
            Self::Evaluation(error) => write!(formatter, "{error}"),
            Self::QueueLimit => formatter.write_str("internal event queue capacity was exhausted"),
            Self::EffectLimit => formatter.write_str("engine step exceeded its effect bound"),
            Self::EffectProtocol => formatter.write_str("engine effect protocol was violated"),
            Self::UnknownBlock => {
                formatter.write_str("executable-content block was not issued in this phase")
            }
        }
    }
}

impl std::error::Error for EngineError {}

#[cfg(test)]
mod tests {
    use super::*;
    use scxml_compiler_core::{ActionBlockIr, InitialIr};
    use scxml_types::{BaggagePolicy, EventName, EventType, TraceContext};

    /// The source id of the documents these tests parse.
    const SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(1);

    struct Evaluator {
        allow: bool,
        calls: usize,
    }

    impl GuardEvaluator for Evaluator {
        fn evaluate_condition(
            &mut self,
            _expression: &str,
            _event: Option<&Event>,
        ) -> Result<bool, EvaluationError> {
            self.calls += 1;
            Ok(self.allow)
        }
    }

    struct FailingEvaluator;

    /// Whether the configuration `context` makes visible includes `state`.
    fn observes(engine: &Engine, context: &EffectContext, state: &StateId) -> bool {
        engine
            .effect_configuration(context)
            .expect("the context belongs to the engine's last command")
            .contains(state)
    }

    /// Fails every guard with the fatal `Platform` class.
    struct PlatformFailingEvaluator;

    impl GuardEvaluator for PlatformFailingEvaluator {
        fn evaluate_condition(
            &mut self,
            _expression: &str,
            _event: Option<&Event>,
        ) -> Result<bool, EvaluationError> {
            Err(EvaluationError::Platform)
        }
    }

    impl GuardEvaluator for FailingEvaluator {
        fn evaluate_condition(
            &mut self,
            _expression: &str,
            _event: Option<&Event>,
        ) -> Result<bool, EvaluationError> {
            Err(EvaluationError::Execution)
        }
    }

    #[derive(Default)]
    struct CurrentEventEvaluator {
        events: Vec<Option<String>>,
    }

    impl GuardEvaluator for CurrentEventEvaluator {
        fn evaluate_condition(
            &mut self,
            _expression: &str,
            event: Option<&Event>,
        ) -> Result<bool, EvaluationError> {
            self.events
                .push(event.map(|event| event.name.as_str().to_owned()));
            Ok(true)
        }
    }

    fn machine(condition: Option<&str>, targetless: bool) -> MachineIr {
        MachineIr {
            version: scxml_compiler_core::MACHINE_IR_VERSION,
            name: Some("test".into()),
            data_model: Some("ecmascript".into()),
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
                    onentry: vec![ActionBlockIr {
                        document_order: 4,
                        actions: vec![ExecutableContentIr::Log {
                            label: Some("Outcome".into()),
                            expr: Some("'pass'".into()),
                        }],
                    }],
                    onexit: Vec::new(),
                    transitions: Vec::new(),
                    data: Vec::new(),
                    invokes: Vec::new(),
                    donedata: None,
                },
            ],
            transitions: vec![TransitionIr {
                document_order: 2,
                source: 0,
                targets: if targetless { Vec::new() } else { vec![1] },
                events: vec!["finish".into()],
                cond: condition.map(str::to_owned),
                transition_type: TransitionType::External,
                actions: vec![ExecutableContentIr::Raise {
                    event: "raised".into(),
                    event_schema: None,
                }],
                event_schema: None,
            }],
            data: Vec::new(),
            scripts: Vec::new(),
            extension_namespaces: Vec::new(),
        }
    }

    fn phase_machine() -> MachineIr {
        let mut machine = machine(None, false);
        machine.states[0].onexit = vec![ActionBlockIr {
            document_order: 2,
            actions: vec![ExecutableContentIr::Log {
                label: Some("exit-source".into()),
                expr: Some("In('ready')".into()),
            }],
        }];
        machine.transitions[0].actions = vec![ExecutableContentIr::Log {
            label: Some("transition".into()),
            expr: Some("!In('ready')&&!In('done')".into()),
        }];
        machine.states[1].onentry = vec![ActionBlockIr {
            document_order: 4,
            actions: vec![ExecutableContentIr::Log {
                label: Some("entry-target".into()),
                expr: Some("In('done')".into()),
            }],
        }];
        machine
    }

    fn eventless_followup_machine() -> MachineIr {
        let mut machine = machine(None, false);
        machine.states[0].transitions = vec![0];
        machine.states.insert(
            1,
            StateIr {
                document_order: 3,
                id: Some("between".into()),
                kind: StateKind::Atomic,
                parent: None,
                children: Vec::new(),
                initial: None,
                onentry: Vec::new(),
                onexit: Vec::new(),
                transitions: vec![1],
                data: Vec::new(),
                invokes: Vec::new(),
                donedata: None,
            },
        );
        machine.states[2].document_order = 5;
        machine.transitions = vec![
            TransitionIr {
                document_order: 2,
                source: 0,
                targets: vec![1],
                events: vec!["advance".into()],
                cond: None,
                transition_type: TransitionType::External,
                actions: Vec::new(),
                event_schema: None,
            },
            TransitionIr {
                document_order: 4,
                source: 1,
                targets: vec![2],
                events: Vec::new(),
                cond: Some("current-event-is-advance".into()),
                transition_type: TransitionType::External,
                actions: vec![ExecutableContentIr::Log {
                    label: Some("eventless".into()),
                    expr: Some("_event.name".into()),
                }],
                event_schema: None,
            },
        ];
        machine
    }

    /// `ready` declares one invocation whose `<finalize>` raises `fin`;
    /// `ready --child--> done` and `ready --fin--> done`.
    fn invoke_machine() -> MachineIr {
        let mut machine = machine(None, false);
        machine.states[0].invokes = vec![InvokeIr {
            document_order: 10,
            type_value: None,
            typeexpr: None,
            src: None,
            srcexpr: None,
            xml_base: None,
            id: Some("inv".into()),
            idlocation: None,
            namelist: Vec::new(),
            autoforward: false,
            children: vec![InvokeChildIr::Finalize {
                actions: vec![ExecutableContentIr::Raise {
                    event: "fin".into(),
                    event_schema: None,
                }],
            }],
            attributes: Vec::new(),
        }];
        machine.states[0].transitions = vec![0, 1];
        machine.transitions[0].events = vec!["child".into()];
        machine.transitions[0].actions = Vec::new();
        machine.transitions.push(TransitionIr {
            document_order: 11,
            source: 0,
            targets: vec![1],
            events: vec!["fin".into()],
            cond: None,
            transition_type: TransitionType::External,
            actions: Vec::new(),
            event_schema: None,
        });
        machine
    }

    fn sid(id: &str) -> StateId {
        StateId::new(id).expect("test state id must be valid")
    }

    /// Starts `invoke_machine`, applies the end-of-macrostep Invoke effect,
    /// and reports the invocation started as `inv`.
    fn started_invoke_engine() -> Engine {
        let mut engine = Engine::new(invoke_machine()).expect("invoke machine must construct");
        let started = engine.start().expect("invoke machine must start");
        assert_eq!(started.entered_states, vec![sid("ready")]);
        assert_eq!(started.configuration, vec![sid("ready")]);
        assert!(
            !started.stable,
            "a pending invocation keeps the macrostep open"
        );
        complete_empty_effect_step(&mut engine, &started);
        let mut evaluator = CurrentEventEvaluator::default();
        let invoke_step = engine
            .resume(&mut evaluator)
            .expect("macrostep end must emit invocations");
        let [planned] = invoke_step.planned_effects.as_slice() else {
            panic!("exactly one Invoke effect expected: {invoke_step:?}");
        };
        assert_eq!(planned.context.phase, EffectExecutionPhase::Invoke);
        let Effect::Invoke {
            state: invoking,
            invoke,
        } = &planned.effect
        else {
            panic!("Invoke effect expected: {planned:?}");
        };
        assert_eq!(invoking, &sid("ready"));
        engine
            .begin_effects(&invoke_step)
            .expect("invoke step must begin");
        engine
            .invocation_started(
                invoking,
                invoke.document_order,
                InvokeId::new("inv").expect("id"),
                scxml_types::SCXML_INVOKE_TYPE_URI.to_owned(),
            )
            .expect("emitted invocation must be accepted as started");
        engine
            .complete_effects()
            .expect("invoke step must complete");
        assert!(engine.stable);
        engine
    }

    #[test]
    fn failed_selection_keeps_the_self_addressed_external_event_queued() {
        struct PlatformFailure;
        impl GuardEvaluator for PlatformFailure {
            fn evaluate_condition(
                &mut self,
                _expression: &str,
                _event: Option<&Event>,
            ) -> Result<bool, EvaluationError> {
                Err(EvaluationError::Platform)
            }
        }
        let mut engine =
            Engine::new(machine(Some("guard"), false)).expect("guarded machine must construct");
        let started = engine.start().expect("guarded machine must start");
        engine.begin_effects(&started).expect("begin");
        engine
            .enqueue_external(external("finish"))
            .expect("self-addressed send must queue");
        engine.complete_effects().expect("complete");
        assert_eq!(
            engine.resume(&mut PlatformFailure).map(|_| ()),
            Err(EngineError::Evaluation(EvaluationError::Platform))
        );
        assert_eq!(
            engine.pending_external,
            [external("finish")],
            "a failed selection must not drop the popped external event"
        );
    }

    #[test]
    fn invocations_start_at_macrostep_end_and_cancel_after_onexit() {
        let mut engine = started_invoke_engine();
        let mut evaluator = CurrentEventEvaluator::default();
        let step = engine
            .send(&external("child"), &mut evaluator)
            .expect("child transition must execute");
        assert_eq!(step.exited_states, vec![sid("ready")]);
        assert_eq!(step.entered_states, vec![sid("done")]);
        let cancel = step
            .planned_effects
            .iter()
            .find(|planned| matches!(planned.effect, Effect::CancelInvoke { .. }))
            .expect("exiting the invoking state must cancel its invocation");
        assert_eq!(cancel.context.phase, EffectExecutionPhase::Exit);
        assert_eq!(
            cancel.effect,
            Effect::CancelInvoke {
                invoke_id: InvokeId::new("inv").expect("id")
            }
        );
    }

    #[test]
    fn unreported_invocations_are_never_started_or_cancelled() {
        let mut engine = Engine::new(invoke_machine()).expect("invoke machine must construct");
        let started = engine.start().expect("invoke machine must start");
        complete_empty_effect_step(&mut engine, &started);
        let mut evaluator = CurrentEventEvaluator::default();
        let invoke_step = engine.resume(&mut evaluator).expect("invoke step");
        // A failed start: the host queues error.execution and never reports.
        engine.begin_effects(&invoke_step).expect("begin");
        engine
            .queue_platform_error(PlatformError::Execution, None)
            .expect("failed start queues error.execution");
        engine.complete_effects().expect("complete");
        assert_eq!(
            engine.invocation_started(
                &sid("ready"),
                10,
                InvokeId::new("late").expect("id"),
                scxml_types::SCXML_INVOKE_TYPE_URI.to_owned()
            ),
            Err(EngineError::EffectProtocol),
            "a start report outside its phase is rejected"
        );
        let error_step = engine.resume(&mut evaluator).expect("error event step");
        assert_eq!(
            error_step
                .event_dispositions
                .first()
                .map(|disposed| disposed.event.name.as_str()),
            Some("error.execution")
        );
        complete_empty_effect_step(&mut engine, &error_step);
        let step = engine
            .send(&external("child"), &mut evaluator)
            .expect("child transition must execute");
        assert!(
            !step
                .planned_effects
                .iter()
                .any(|planned| matches!(planned.effect, Effect::CancelInvoke { .. })),
            "an invocation that never started is never cancelled"
        );
    }

    #[test]
    fn finalize_runs_before_selection_and_its_raises_do_not_preempt_the_event() {
        let mut engine = started_invoke_engine();
        let mut evaluator = CurrentEventEvaluator::default();
        let mut child = external("child");
        child.invoke_id = Some(InvokeId::new("inv").expect("id"));
        let finalize = engine
            .send(&child, &mut evaluator)
            .expect("invocation event must plan finalize");
        assert!(finalize.event_dispositions.is_empty());
        assert_eq!(finalize.trigger_event.as_ref(), Some(&child));
        let [planned] = finalize.planned_effects.as_slice() else {
            panic!("finalize must plan exactly its raise: {finalize:?}");
        };
        assert_eq!(planned.context.phase, EffectExecutionPhase::Finalize);
        let mut applied = Vec::new();
        drive_transaction(&mut engine, finalize, &mut evaluator, &mut applied);
        let selected = engine
            .resume(&mut evaluator)
            .expect("pending dispatch must select");
        assert_eq!(
            selected.event_dispositions,
            vec![DisposedEvent {
                event: child,
                disposition: EventDisposition::Selected,
            }],
            "the external event is selected before the finalize-raised internal event"
        );
        assert_eq!(selected.entered_states, vec![sid("done")]);
    }

    #[test]
    fn late_binding_binds_each_state_once_before_its_onentry() {
        let mut machine = machine(None, false);
        machine.binding = Binding::Late;
        machine.states[0].data = vec![DataIr {
            document_order: 20,
            id: "x".into(),
            src: None,
            expr: Some("1".into()),
            content: Vec::new(),
        }];
        machine.states[0].onentry = vec![ActionBlockIr {
            document_order: 21,
            actions: vec![ExecutableContentIr::Log {
                label: Some("entry".into()),
                expr: None,
            }],
        }];
        let mut engine = Engine::new(machine).expect("late-binding machine must construct");
        let started = engine.start().expect("late-binding machine must start");
        let labels: Vec<&str> = started
            .planned_effects
            .iter()
            .map(|planned| match &planned.effect {
                Effect::BindStateData { .. } => "bind",
                Effect::Log { .. } => "log",
                _ => "other",
            })
            .collect();
        assert_eq!(labels, ["bind", "log"]);
        assert!(matches!(
            &started.planned_effects[0].effect,
            Effect::BindStateData { state: bound, declarations } if bound == &sid("ready") && declarations.len() == 1
        ));
    }

    #[test]
    fn snapshot_lists_live_invocations_and_bound_states_and_rejects_open_transactions() {
        let options = SnapshotOptions {
            transitions: TransitionListing::Include,
            previews: Previews::Include,
        };
        let mut engine = Engine::new(invoke_machine()).expect("invoke machine must construct");
        let ready = engine.snapshot(options).expect("a ready engine snapshots");
        assert_eq!(ready.status, EngineStatus::Ready);
        assert_eq!(ready.name.as_deref(), Some("test"), "the IR name");
        assert!(ready.atomic.is_empty() && ready.configuration.is_empty());
        assert!(ready.transitions.is_empty());
        let started = engine.start().expect("invoke machine must start");
        assert_eq!(
            engine.snapshot(options),
            Err(EngineError::EffectProtocol),
            "a planned step not yet applied is mid-transaction"
        );
        engine.begin_effects(&started).expect("begin");
        assert_eq!(
            engine.snapshot(options),
            Err(EngineError::EffectProtocol),
            "an open effect phase is mid-transaction"
        );
        engine.complete_effects().expect("complete");

        let engine = started_invoke_engine();
        let image = engine.snapshot(options).expect("a stable engine snapshots");
        assert_eq!(
            image.invocations,
            [InvocationSnapshot {
                invoke_id: InvokeId::new("inv").expect("id"),
                state: sid("ready"),
                index: 0,
                autoforward: false,
                type_uri: scxml_types::SCXML_INVOKE_TYPE_URI.to_owned(),
            }]
        );
        assert_eq!(image.atomic, [sid("ready")]);
        assert!(image.internal.is_empty() && image.history.is_empty());

        let mut late = machine(None, false);
        late.binding = Binding::Late;
        late.name = None;
        late.states[0].data = vec![DataIr {
            document_order: 20,
            id: "x".into(),
            src: None,
            expr: Some("1".into()),
            content: Vec::new(),
        }];
        let mut engine = Engine::new(late).expect("late-binding machine must construct");
        let started = engine.start().expect("late-binding machine must start");
        complete_empty_effect_step(&mut engine, &started);
        let image = engine.snapshot(options).expect("snapshot");
        assert_eq!(image.name, None, "no IR name: no `_name` value");
        assert_eq!(image.bound_states, [sid("ready")]);
    }

    #[test]
    fn block_failure_rejects_unissued_blocks() {
        let mut logged = machine(None, false);
        logged.states[0].onentry = vec![ActionBlockIr {
            document_order: 21,
            actions: vec![ExecutableContentIr::Log {
                label: Some("entry".into()),
                expr: None,
            }],
        }];
        let mut engine = Engine::new(logged).expect("logging machine must construct");
        let started = engine.start().expect("logging machine must start");
        engine.begin_effects(&started).expect("begin");
        assert_eq!(
            engine.block_failed(PlatformError::Communication, None, u64::MAX, None),
            Err(EngineError::UnknownBlock)
        );
        let issued = started
            .planned_effects
            .first()
            .map(|planned| planned.context.block)
            .expect("entry log must be planned");
        engine
            .block_failed(PlatformError::Communication, None, issued, None)
            .expect("issued block must fail");
        engine.complete_effects().expect("complete");
        assert_eq!(
            engine.internal.front().map(|event| event.name.as_str()),
            Some("error.communication")
        );
    }

    fn external(name: &str) -> Event {
        Event {
            name: EventName::new(name).expect("test event name must be valid"),
            event_type: EventType::External,
            send_id: None,
            origin: None,
            origin_type: None,
            invoke_id: None,
            data: scxml_types::DataValue::Absent,
            raw: None,
            trace_context: None,
        }
    }

    fn complete_empty_effect_step(engine: &mut Engine, step: &CommandResult) {
        engine
            .begin_effects(step)
            .expect("test step must begin its effect phase");
        engine
            .complete_effects()
            .expect("test effect phase must complete");
    }

    #[test]
    fn guard_selection_and_effect_order_are_authoritative() {
        let mut engine = Engine::new(machine(Some("allowed"), false))
            .expect("valid flat Machine IR must construct an engine");
        let start = engine.start().expect("ready engine must start");
        assert!(start.stable);
        complete_empty_effect_step(&mut engine, &start);
        let mut denied = Evaluator {
            allow: false,
            calls: 0,
        };
        let step = engine
            .send(&external("finish.child"), &mut denied)
            .expect("false guard must be a valid non-transition");
        assert_eq!(step.leaves.first().map(StateId::as_str), Some("ready"));
        assert_eq!(denied.calls, 1);
        complete_empty_effect_step(&mut engine, &step);
        let mut allowed = Evaluator {
            allow: true,
            calls: 0,
        };
        let step = engine
            .send(&external("finish.child"), &mut allowed)
            .expect("true guard must select the prefix-matched transition");
        assert_eq!(step.status, EngineStatus::Finished);
        assert_eq!(
            step.effects,
            vec![
                Effect::Raise("raised".into(), true, None),
                Effect::Log {
                    label: Some("Outcome".into()),
                    expression: Some("'pass'".into()),
                },
            ]
        );
        engine
            .begin_effects(&step)
            .expect("selected step must begin its effect phase");
        let mut ordering = Vec::new();
        for effect in &step.effects {
            match effect {
                Effect::Raise(name, ..) => {
                    ordering.push(format!("raise:{name}"));
                    engine
                        .raise(Event::internal(
                            EventName::new(name.clone()).expect("test raise name must be valid"),
                        ))
                        .expect("raise emitted during final entry must execute before finality");
                }
                Effect::Log { label, .. } => {
                    ordering.push(format!(
                        "log:{}",
                        label.as_deref().expect("test log label must be present")
                    ));
                }
                _ => panic!("test fixture must emit only raise and log effects"),
            }
        }
        engine
            .complete_effects()
            .expect("ordered effects must complete the final transition");
        assert_eq!(ordering, ["raise:raised", "log:Outcome"]);
        assert_eq!(engine.status, EngineStatus::Finished);
        assert_eq!(engine.internal_len(), 0);
    }

    #[test]
    fn effects_expose_scxml_configuration_by_execution_phase() {
        let mut engine =
            Engine::new(phase_machine()).expect("phase fixture must construct a valid flat engine");
        let start = engine.start().expect("phase fixture must start");
        complete_empty_effect_step(&mut engine, &start);
        let mut evaluator = Evaluator {
            allow: true,
            calls: 0,
        };
        let step = engine
            .send(&external("finish"), &mut evaluator)
            .expect("phase fixture transition must execute");
        let source = StateId::new("ready").expect("source state ID must be valid");
        let target = StateId::new("done").expect("target state ID must be valid");

        assert_eq!(step.planned_effects.len(), 3);
        assert_eq!(step.effects.len(), step.planned_effects.len());
        assert_eq!(
            step.planned_effects
                .iter()
                .map(|planned| planned.context.phase)
                .collect::<Vec<_>>(),
            [
                EffectExecutionPhase::Exit,
                EffectExecutionPhase::Transition,
                EffectExecutionPhase::Entry,
            ]
        );

        let exit = &step.planned_effects[0].context;
        assert!(observes(&engine, exit, &source));
        assert!(!observes(&engine, exit, &target));

        let transition = &step.planned_effects[1].context;
        assert!(!observes(&engine, transition, &source));
        assert!(!observes(&engine, transition, &target));

        let entry = &step.planned_effects[2].context;
        assert!(!observes(&engine, entry, &source));
        assert!(observes(&engine, entry, &target));

        assert_eq!(step.leaves.first().map(StateId::as_str), Some("done"));
        assert_eq!(
            step.event_dispositions,
            vec![DisposedEvent {
                event: external("finish"),
                disposition: EventDisposition::Selected,
            }]
        );
    }

    #[test]
    fn begin_effects_rejects_tampered_phase_context_without_consuming_canonical_step() {
        let mut engine = Engine::new(phase_machine())
            .expect("effect-protocol fixture must construct a valid flat engine");
        let start = engine.start().expect("effect-protocol fixture must start");
        complete_empty_effect_step(&mut engine, &start);
        let mut evaluator = Evaluator {
            allow: true,
            calls: 0,
        };
        let step = engine
            .send(&external("finish"), &mut evaluator)
            .expect("effect-protocol fixture transition must execute");
        let mut tampered = step.clone();
        tampered.planned_effects[0].context.phase = EffectExecutionPhase::Entry;

        assert_eq!(
            engine.begin_effects(&tampered),
            Err(EngineError::EffectProtocol)
        );
        engine
            .begin_effects(&step)
            .expect("rejected tampering must leave the canonical step available");
    }

    #[test]
    fn resume_reports_unmatched_internal_events_before_selected_event_in_fifo_order() {
        let mut engine = Engine::new(machine(None, false))
            .expect("internal-event fixture must construct a valid flat engine");
        let start = engine.start().expect("internal-event fixture must start");
        complete_empty_effect_step(&mut engine, &start);
        for name in ["noise.one", "noise.two", "finish", "noise.after"] {
            engine
                .raise(Event::internal(
                    EventName::new(name).expect("internal test event name must be valid"),
                ))
                .expect("bounded internal test event must enqueue");
        }
        let mut evaluator = Evaluator {
            allow: true,
            calls: 0,
        };
        let step = engine
            .resume(&mut evaluator)
            .expect("resume must select the first matching internal event");

        assert_eq!(
            step.event_dispositions
                .iter()
                .map(|disposed| (disposed.event.name.as_str(), disposed.disposition))
                .collect::<Vec<_>>(),
            [
                ("noise.one", EventDisposition::Ignored),
                ("noise.two", EventDisposition::Ignored),
                ("finish", EventDisposition::Selected),
            ]
        );
        assert_eq!(
            step.trigger_event.as_ref().map(|event| event.name.as_str()),
            Some("finish")
        );
        assert_eq!(
            engine
                .internal
                .iter()
                .map(|event| event.name.as_str())
                .collect::<Vec<_>>(),
            ["noise.after"]
        );
    }

    #[test]
    fn next_internal_is_the_oldest_queued_event_with_its_trace_context() {
        let mut engine = Engine::new(machine(None, false))
            .expect("ignored-event fixture must construct a valid flat engine");
        let start = engine.start().expect("ignored-event fixture must start");
        complete_empty_effect_step(&mut engine, &start);
        assert!(engine.is_stable());
        assert_eq!(engine.next_internal(), None);
        let parent = TraceContext::new(
            "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01",
            Some("vendor=value".into()),
            Some("tenant=example".into()),
            BaggagePolicy::Propagate,
        )
        .expect("trace context fixture must be canonical");
        let mut first = Event::internal(
            EventName::new("first").expect("internal test event name must be valid"),
        );
        first.trace_context = Some(parent.clone());
        engine
            .raise(first)
            .expect("first internal event must enqueue");
        engine
            .raise(Event::internal(
                EventName::new("second").expect("internal test event name must be valid"),
            ))
            .expect("second internal event must enqueue");
        assert!(!engine.is_stable());
        assert_eq!(engine.internal_len(), 2);
        let next = engine.next_internal().expect("an internal event is queued");
        assert_eq!(next.name.as_str(), "first");
        assert_eq!(next.trace_context.as_ref(), Some(&parent));
    }

    #[test]
    fn resume_reports_all_unmatched_internal_events_when_queue_stabilizes() {
        let mut engine = Engine::new(machine(None, false))
            .expect("ignored-event fixture must construct a valid flat engine");
        let start = engine.start().expect("ignored-event fixture must start");
        complete_empty_effect_step(&mut engine, &start);
        for name in ["noise.one", "noise.two"] {
            engine
                .raise(Event::internal(
                    EventName::new(name).expect("internal test event name must be valid"),
                ))
                .expect("bounded internal test event must enqueue");
        }
        let mut evaluator = Evaluator {
            allow: true,
            calls: 0,
        };
        let step = engine
            .resume(&mut evaluator)
            .expect("unmatched internal events must stabilize explicitly");

        assert!(step.stable);
        assert!(step.effects.is_empty());
        assert_eq!(engine.internal_len(), 0);
        assert_eq!(
            step.event_dispositions
                .iter()
                .map(|disposed| (disposed.event.name.as_str(), disposed.disposition))
                .collect::<Vec<_>>(),
            [
                ("noise.one", EventDisposition::Ignored),
                ("noise.two", EventDisposition::Ignored),
            ]
        );
    }

    #[test]
    fn failed_internal_selection_preserves_the_entire_fifo_queue() {
        let mut engine = Engine::new(machine(Some("condition"), false))
            .expect("selection-failure fixture must construct a valid flat engine");
        let start = engine
            .start()
            .expect("selection-failure fixture must start");
        complete_empty_effect_step(&mut engine, &start);
        for name in ["noise", "finish", "after"] {
            engine
                .raise(Event::internal(
                    EventName::new(name).expect("internal test event name must be valid"),
                ))
                .expect("bounded internal test event must enqueue");
        }

        // A cond that cannot be evaluated as a boolean raises error.execution
        // and evaluates as false; every previously queued internal event stays
        // ordered and accounted for in the same processing round.
        let step = engine
            .resume(&mut FailingEvaluator)
            .expect("a cond evaluation failure must not abort the round");
        assert_eq!(
            step.event_dispositions
                .iter()
                .map(|disposed| (
                    disposed.event.name.as_str().to_owned(),
                    disposed.disposition
                ))
                .collect::<Vec<_>>(),
            vec![
                ("noise".to_owned(), EventDisposition::Ignored),
                ("finish".to_owned(), EventDisposition::Ignored),
                ("after".to_owned(), EventDisposition::Ignored),
                ("error.execution".to_owned(), EventDisposition::Ignored),
            ],
            "queued events keep FIFO order and the raised error joins last"
        );
        assert!(step.stable);
    }

    #[test]
    fn targetless_transition_preserves_configuration() {
        let mut engine = Engine::new(machine(None, true))
            .expect("valid targetless Machine IR must construct an engine");
        let start = engine.start().expect("ready engine must start");
        complete_empty_effect_step(&mut engine, &start);
        let mut evaluator = Evaluator {
            allow: true,
            calls: 0,
        };
        let step = engine
            .send(&external("finish"), &mut evaluator)
            .expect("targetless transition must execute");
        assert_eq!(step.leaves.first().map(StateId::as_str), Some("ready"));
        assert_eq!(step.status, EngineStatus::Running);
        assert_eq!(
            step.effects,
            vec![Effect::Raise("raised".into(), true, None)]
        );
        let context = &step.planned_effects[0].context;
        assert_eq!(context.phase, EffectExecutionPhase::Transition);
        assert!(observes(
            &engine,
            context,
            &StateId::new("ready").expect("targetless source state ID must be valid")
        ));
    }

    #[test]
    fn eventless_followup_retains_the_macrostep_current_event() {
        let mut engine = Engine::new(eventless_followup_machine())
            .expect("eventless follow-up fixture must construct a valid flat engine");
        let start = engine
            .start()
            .expect("eventless follow-up fixture must start");
        complete_empty_effect_step(&mut engine, &start);
        let mut evaluator = CurrentEventEvaluator::default();
        let event = external("advance");
        let first = engine
            .send(&event, &mut evaluator)
            .expect("external transition must enter the eventless source state");
        complete_empty_effect_step(&mut engine, &first);

        let eventless = engine
            .resume(&mut evaluator)
            .expect("eventless follow-up must execute in the same macrostep");

        assert_eq!(evaluator.events, [Some("advance".to_owned())]);
        assert_eq!(eventless.trigger_event, Some(event));
        assert_eq!(eventless.status, EngineStatus::Finished);
    }

    #[test]
    fn queue_platform_error_rejects_finished_engine() {
        let mut engine = Engine::new(machine(None, false))
            .expect("valid flat Machine IR must construct an engine");
        let started = engine.start().expect("fixture machine must start");
        complete_empty_effect_step(&mut engine, &started);
        let mut evaluator = CurrentEventEvaluator::default();
        let step = engine
            .send(&external("finish"), &mut evaluator)
            .expect("finish transition must execute");
        engine
            .begin_effects(&step)
            .expect("finish step must begin its effect phase");
        for planned in &step.planned_effects {
            if let Effect::Raise(name, ..) = &planned.effect {
                engine
                    .raise(Event::internal(
                        EventName::new(name.clone()).expect("test raise name must be valid"),
                    ))
                    .expect("raise emitted during final entry must execute before finality");
            }
        }
        engine
            .complete_effects()
            .expect("finish step must complete its effect phase");
        assert_eq!(engine.status, EngineStatus::Finished);
        assert_eq!(
            engine.queue_platform_error(PlatformError::Execution, None),
            Err(EngineError::Finished)
        );
        // The rejected queue attempt leaves the final state untouched.
        assert_eq!(engine.internal_len(), 0);
    }

    /// Compound fixture:
    ///
    /// - `s0` compound with children `s1` (atomic leaf), `s2` (final child)
    /// - `done` top-level final state
    /// - `s1 --go--> s2` (entering `s2` raises `done.state.s0`)
    /// - `s0 --leave--> done` (selected from the ancestor while `s1` is leaf)
    fn log_block(document_order: u32, label: &str, expression: &str) -> ActionBlockIr {
        ActionBlockIr {
            document_order,
            actions: vec![ExecutableContentIr::Log {
                label: Some(label.to_owned()),
                expr: Some(expression.to_owned()),
            }],
        }
    }

    struct StateRow {
        document_order: u32,
        id: &'static str,
        kind: StateKind,
        parent: Option<u32>,
        children: Vec<u32>,
        initial: Option<InitialIr>,
        on_entry: Vec<ActionBlockIr>,
        on_exit: Vec<ActionBlockIr>,
        transitions: Vec<u32>,
    }

    fn state(row: StateRow) -> StateIr {
        StateIr {
            document_order: row.document_order,
            id: Some(row.id.to_owned()),
            kind: row.kind,
            parent: row.parent,
            children: row.children,
            initial: row.initial,
            onentry: row.on_entry,
            onexit: row.on_exit,
            transitions: row.transitions,
            data: Vec::new(),
            invokes: Vec::new(),
            donedata: None,
        }
    }

    fn transition(
        document_order: u32,
        source: u32,
        targets: Vec<u32>,
        events: Vec<&str>,
    ) -> TransitionIr {
        TransitionIr {
            document_order,
            source,
            targets,
            events: events.into_iter().map(str::to_owned).collect(),
            cond: None,
            transition_type: TransitionType::External,
            actions: Vec::new(),
            event_schema: None,
        }
    }

    fn compound_machine() -> MachineIr {
        MachineIr {
            version: scxml_compiler_core::MACHINE_IR_VERSION,
            name: Some("compound".into()),
            data_model: Some("ecmascript".into()),
            binding: Binding::Early,
            initial: InitialIr {
                document_order: 0,
                targets: vec![0],
                actions: Vec::new(),
            },
            states: vec![
                state(StateRow {
                    document_order: 1,
                    id: "s0",
                    kind: StateKind::Compound,
                    parent: None,
                    children: vec![1, 2],
                    initial: Some(InitialIr {
                        document_order: 2,
                        targets: vec![1],
                        actions: Vec::new(),
                    }),
                    on_entry: vec![log_block(2, "enter-s0", "In('s0')")],
                    on_exit: vec![log_block(2, "exit-s0", "In('s0')")],
                    transitions: vec![1],
                }),
                state(StateRow {
                    document_order: 3,
                    id: "s1",
                    kind: StateKind::Atomic,
                    parent: Some(0),
                    children: Vec::new(),
                    initial: None,
                    on_entry: vec![log_block(4, "enter-s1", "In('s1')")],
                    on_exit: vec![log_block(4, "exit-s1", "In('s1')")],
                    transitions: vec![0],
                }),
                state(StateRow {
                    document_order: 5,
                    id: "s2",
                    kind: StateKind::Final,
                    parent: Some(0),
                    children: Vec::new(),
                    initial: None,
                    on_entry: Vec::new(),
                    on_exit: Vec::new(),
                    transitions: Vec::new(),
                }),
                state(StateRow {
                    document_order: 6,
                    id: "done",
                    kind: StateKind::Final,
                    parent: None,
                    children: Vec::new(),
                    initial: None,
                    on_entry: Vec::new(),
                    on_exit: Vec::new(),
                    transitions: Vec::new(),
                }),
            ],
            transitions: vec![
                transition(4, 1, vec![2], vec!["go"]),
                transition(7, 0, vec![3], vec!["leave"]),
            ],
            data: Vec::new(),
            scripts: Vec::new(),
            extension_namespaces: Vec::new(),
        }
    }

    #[test]
    fn compound_start_enters_default_initial_descent_with_ancestor_first_entry() {
        let mut engine =
            Engine::new(compound_machine()).expect("compound Machine IR must construct an engine");
        let start = engine.start().expect("compound engine must start");
        assert_eq!(
            start
                .planned_effects
                .iter()
                .map(|planned| planned.context.phase)
                .collect::<Vec<_>>(),
            [EffectExecutionPhase::Entry, EffectExecutionPhase::Entry],
        );
        let s0 = StateId::new("s0").expect("s0 id must be valid");
        let s1 = StateId::new("s1").expect("s1 id must be valid");
        // s0's entry observes only s0; s1's entry observes the full prefix.
        assert!(observes(&engine, &start.planned_effects[0].context, &s0));
        assert!(!observes(&engine, &start.planned_effects[0].context, &s1));
        assert!(observes(&engine, &start.planned_effects[1].context, &s0));
        assert!(observes(&engine, &start.planned_effects[1].context, &s1));
        assert_eq!(start.leaves.first().map(StateId::as_str), Some("s1"));
        assert!(start.stable);
    }

    #[test]
    fn ancestor_transition_selects_when_leaf_has_no_match_and_enters_in_w3c_order() {
        let mut engine =
            Engine::new(compound_machine()).expect("compound Machine IR must construct an engine");
        let start = engine.start().expect("compound engine must start");
        complete_empty_effect_step(&mut engine, &start);
        let mut evaluator = Evaluator {
            allow: true,
            calls: 0,
        };
        let step = engine
            .send(&external("leave"), &mut evaluator)
            .expect("ancestor transition must be reachable from the leaf");
        assert_eq!(step.status, EngineStatus::Finished);
        assert_eq!(
            step.planned_effects
                .iter()
                .map(|planned| planned.context.phase)
                .collect::<Vec<_>>(),
            [EffectExecutionPhase::Exit, EffectExecutionPhase::Exit]
        );
        let s0 = StateId::new("s0").expect("s0 id must be valid");
        let s1 = StateId::new("s1").expect("s1 id must be valid");
        // Exit runs innermost-first and the full pre-exit configuration stays
        // observable to every exited state's on-exit handler.
        assert!(observes(&engine, &step.planned_effects[0].context, &s1));
        assert!(observes(&engine, &step.planned_effects[0].context, &s0));
        assert!(observes(&engine, &step.planned_effects[1].context, &s0));
        assert_eq!(step.leaves.first().map(StateId::as_str), Some("done"));
    }

    #[test]
    fn final_child_entry_plans_done_state_raise_for_compound_parent() {
        let mut engine =
            Engine::new(compound_machine()).expect("compound Machine IR must construct an engine");
        let start = engine.start().expect("compound engine must start");
        complete_empty_effect_step(&mut engine, &start);
        let mut evaluator = Evaluator {
            allow: true,
            calls: 0,
        };
        let step = engine
            .send(&external("go"), &mut evaluator)
            .expect("leaf transition into the final child must execute");
        assert_eq!(step.status, EngineStatus::Running);
        let raises = step
            .planned_effects
            .iter()
            .filter_map(|planned| match &planned.effect {
                Effect::Raise(name, ..) => Some(name.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(raises, ["done.state.s0"]);
        let done_state = step
            .planned_effects
            .iter()
            .find_map(|planned| match &planned.effect {
                Effect::Raise(name, ..) if name == "done.state.s0" => Some(planned.context.clone()),
                _ => None,
            })
            .expect("done.state raise must carry an entry context");
        assert_eq!(done_state.phase, EffectExecutionPhase::Entry);
        engine
            .begin_effects(&step)
            .expect("final-child entry must begin its effect phase");
        engine
            .raise(Event::internal(
                EventName::new("done.state.s0").expect("done.state name must be valid"),
            ))
            .expect("done.state raise must enqueue through the protocol");
        engine
            .complete_effects()
            .expect("final-child entry effect phase must complete");
        assert!(!engine.stable);
        assert_eq!(
            engine
                .internal
                .front()
                .map(|event| event.name.as_str().to_owned()),
            Some("done.state.s0".to_owned())
        );
        let next = engine
            .resume(&mut evaluator)
            .expect("done.state must process to a stable configuration");
        assert!(next.stable);
        assert_eq!(
            next.event_dispositions
                .iter()
                .map(|disposed| (disposed.event.name.as_str(), disposed.disposition))
                .collect::<Vec<_>>(),
            [("done.state.s0", EventDisposition::Ignored)]
        );
    }

    #[test]
    fn eventless_ancestor_transition_follows_internal_event_priority() {
        let mut machine = compound_machine();
        machine.transitions.push(TransitionIr {
            document_order: 8,
            source: 0,
            targets: vec![3],
            events: Vec::new(),
            cond: None,
            transition_type: TransitionType::External,
            actions: Vec::new(),
            event_schema: None,
        });
        machine.states[0].transitions = vec![1, 2];
        let mut engine =
            Engine::new(machine).expect("eventless compound Machine IR must construct an engine");
        let start = engine.start().expect("compound engine must start");
        complete_empty_effect_step(&mut engine, &start);
        assert!(!start.stable);
        let mut evaluator = Evaluator {
            allow: true,
            calls: 0,
        };
        // With the queue empty, resume must take the ancestor eventless path.
        let step = engine
            .resume(&mut evaluator)
            .expect("ancestor eventless transition must be selectable");
        assert_eq!(step.status, EngineStatus::Finished);
        assert_eq!(step.leaves.first().map(StateId::as_str), Some("done"));
    }

    /// Compound machine with one shallow-history child of `s0`. Shape:
    /// `s0 { h, s01, s02 }` plus top-level `s2` and `done`; `swap` moves
    /// between the children of `s0`, `back` targets the history pseudostate,
    /// `leave` finishes. The fallback transition records a default-history
    /// raise so default-entry ordering is observable.
    #[allow(
        clippy::too_many_lines,
        reason = "the history fixture machine spells out every state and transition row explicitly"
    )]
    fn history_machine(initial_target: usize) -> MachineIr {
        MachineIr {
            version: scxml_compiler_core::MACHINE_IR_VERSION,
            name: Some("history".into()),
            data_model: Some("ecmascript".into()),
            binding: Binding::Early,
            initial: InitialIr {
                document_order: 0,
                targets: vec![u32::try_from(initial_target).expect("test index must fit u32")],
                actions: Vec::new(),
            },
            states: vec![
                state(StateRow {
                    document_order: 1,
                    id: "s0",
                    kind: StateKind::Compound,
                    parent: None,
                    children: vec![1, 2, 3],
                    initial: Some(InitialIr {
                        document_order: 2,
                        targets: vec![2],
                        actions: Vec::new(),
                    }),
                    on_entry: vec![log_block(2, "enter-s0", "In('s0')")],
                    on_exit: Vec::new(),
                    transitions: Vec::new(),
                }),
                state(StateRow {
                    document_order: 3,
                    id: "h",
                    kind: StateKind::ShallowHistory,
                    parent: Some(0),
                    children: Vec::new(),
                    initial: None,
                    on_entry: Vec::new(),
                    on_exit: Vec::new(),
                    transitions: vec![0],
                }),
                state(StateRow {
                    document_order: 4,
                    id: "s01",
                    kind: StateKind::Atomic,
                    parent: Some(0),
                    children: Vec::new(),
                    initial: None,
                    on_entry: Vec::new(),
                    on_exit: Vec::new(),
                    transitions: vec![1],
                }),
                state(StateRow {
                    document_order: 5,
                    id: "s02",
                    kind: StateKind::Atomic,
                    parent: Some(0),
                    children: Vec::new(),
                    initial: None,
                    on_entry: vec![log_block(6, "enter-s02", "In('s02')")],
                    on_exit: Vec::new(),
                    transitions: vec![2],
                }),
                state(StateRow {
                    document_order: 6,
                    id: "s2",
                    kind: StateKind::Atomic,
                    parent: None,
                    children: Vec::new(),
                    initial: None,
                    on_entry: Vec::new(),
                    on_exit: Vec::new(),
                    transitions: vec![3, 4],
                }),
                state(StateRow {
                    document_order: 7,
                    id: "done",
                    kind: StateKind::Final,
                    parent: None,
                    children: Vec::new(),
                    initial: None,
                    on_entry: Vec::new(),
                    on_exit: Vec::new(),
                    transitions: Vec::new(),
                }),
            ],
            transitions: vec![
                TransitionIr {
                    document_order: 8,
                    source: 1,
                    targets: vec![3],
                    events: Vec::new(),
                    cond: None,
                    transition_type: TransitionType::External,
                    actions: vec![ExecutableContentIr::Raise {
                        event: "default-history-content".to_owned(),
                        event_schema: None,
                    }],
                    event_schema: None,
                },
                transition(9, 2, vec![4], vec!["go"]),
                transition(10, 3, vec![4], vec!["go"]),
                transition(11, 4, vec![1], vec!["back"]),
                transition(12, 4, vec![5], vec!["leave"]),
            ],
            data: Vec::new(),
            scripts: Vec::new(),
            extension_namespaces: Vec::new(),
        }
    }

    #[test]
    fn history_default_entry_enters_fallback_targets_and_defers_content() {
        let mut engine =
            Engine::new(history_machine(1)).expect("history machine IR must construct an engine");
        let start = engine.start().expect("history machine must start");
        // Entry order: parent s0's onentry, then the default history
        // content, then the fallback leaf s02 (SCXML 3.10.2; W3C
        // enterStates runs `defaultHistoryContent[s.id]` for the parent
        // right after its onentry and initial content).
        let s0 = StateId::new("s0").expect("s0 id must be valid");
        let s02 = StateId::new("s02").expect("s02 id must be valid");
        let entry_effects = start
            .planned_effects
            .iter()
            .filter(|planned| planned.context.phase == EffectExecutionPhase::Entry)
            .collect::<Vec<_>>();
        assert_eq!(entry_effects.len(), 3);
        assert!(observes(&engine, &entry_effects[0].context, &s0));
        assert!(!observes(&engine, &entry_effects[0].context, &s02));
        assert!(matches!(
            &entry_effects[1].effect,
            Effect::Raise(name, ..) if name == "default-history-content"
        ));
        assert!(observes(&engine, &entry_effects[1].context, &s0));
        assert!(!observes(&engine, &entry_effects[1].context, &s02));
        assert!(observes(&engine, &entry_effects[2].context, &s02));
        assert_eq!(start.leaves.first().map(StateId::as_str), Some("s02"));
    }

    #[test]
    fn history_capture_stores_shallow_children_and_reenters_stored_leaves() {
        let mut engine =
            Engine::new(history_machine(2)).expect("history machine IR must construct an engine");
        let start = engine.start().expect("history machine must start");
        complete_empty_effect_step(&mut engine, &start);
        let mut evaluator = Evaluator {
            allow: true,
            calls: 0,
        };
        let go = engine
            .send(&external("go"), &mut evaluator)
            .expect("go transition must execute");
        complete_empty_effect_step(&mut engine, &go);
        // Exiting s0's subtree captured the shallow history value s01.
        assert_eq!(engine.history_values.get(&1), Some(&vec![2]));
        let back = engine
            .send(&external("back"), &mut evaluator)
            .expect("back transition must execute");
        assert_eq!(back.leaves.first().map(StateId::as_str), Some("s01"));
        // Stored history entry executes no default history content.
        assert!(!back.planned_effects.iter().any(|planned| matches!(
            &planned.effect,
            Effect::Raise(name, ..) if name == "default-history-content"
        )));
        complete_empty_effect_step(&mut engine, &back);
    }

    #[test]
    fn event_descriptors_follow_token_prefix_and_wildcard_rules() {
        assert!(descriptor_matches("error", "error"));
        assert!(descriptor_matches("error", "error.send.failed"));
        assert!(descriptor_matches("error.", "error"));
        assert!(descriptor_matches("error.*", "error"));
        assert!(descriptor_matches("error.*", "error.send"));
        assert!(descriptor_matches(".*", "anything.here"));
        assert!(descriptor_matches("*", "anything.here"));
        assert!(!descriptor_matches("error", "errors.send"));
        assert!(!descriptor_matches("foo", "foobar"));
    }

    /// Builds one atomic `ready` state whose on-entry content is `entry_actions`,
    /// plus a final `done` target for the `finish` event.
    fn entry_machine(entry_actions: Vec<ExecutableContentIr>) -> MachineIr {
        MachineIr {
            version: scxml_compiler_core::MACHINE_IR_VERSION,
            name: Some("conditional".into()),
            data_model: Some("ecmascript".into()),
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
                    onentry: vec![ActionBlockIr {
                        document_order: 0,
                        actions: entry_actions,
                    }],
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
            transitions: vec![TransitionIr {
                document_order: 2,
                source: 0,
                targets: vec![1],
                events: vec!["finish".into()],
                cond: None,
                transition_type: TransitionType::External,
                actions: Vec::new(),
                event_schema: None,
            }],
            data: Vec::new(),
            scripts: Vec::new(),
            extension_namespaces: Vec::new(),
        }
    }

    /// Counts condition evaluations and treats only the literal `true` as
    /// taken, mirroring the fixture conditions used by these tests.
    #[derive(Default)]
    struct ExpressionEvaluator {
        calls: usize,
    }
    impl GuardEvaluator for ExpressionEvaluator {
        fn evaluate_condition(
            &mut self,
            expression: &str,
            _event: Option<&Event>,
        ) -> Result<bool, EvaluationError> {
            self.calls = self.calls.saturating_add(1);
            Ok(expression == "true")
        }
    }

    fn raise(event: &str) -> ExecutableContentIr {
        ExecutableContentIr::Raise {
            event: event.to_owned(),
            event_schema: None,
        }
    }

    fn assign(location: &str) -> ExecutableContentIr {
        ExecutableContentIr::Assign {
            location: location.to_owned(),
            expr: None,
            body: Vec::new(),
            assign_type: None,
            attr: None,
        }
    }

    fn log(label: &str) -> ExecutableContentIr {
        ExecutableContentIr::Log {
            label: Some(label.to_owned()),
            expr: None,
        }
    }

    fn if_action(
        condition: &str,
        actions: Vec<ExecutableContentIr>,
        else_if: Vec<scxml_compiler_core::ElseIfIr>,
        else_actions: Option<Vec<ExecutableContentIr>>,
    ) -> ExecutableContentIr {
        ExecutableContentIr::If {
            cond: condition.to_owned(),
            actions,
            elseif: else_if,
            else_actions,
        }
    }

    /// Applies one command's ordered effects, mirroring a session host:
    /// raises return through [`Engine::raise`], other effects collect labels.
    fn apply_effects(engine: &mut Engine, step: &CommandResult, applied: &mut Vec<String>) {
        for planned in &step.planned_effects {
            match &planned.effect {
                Effect::Raise(event, ..) => engine
                    .raise(Event::internal(
                        EventName::new(event.clone()).expect("test event name must be valid"),
                    ))
                    .expect("engine raise must accept the planned raise"),
                Effect::Log { label, .. } => {
                    applied.push(format!("log:{}", label.clone().unwrap_or_default()));
                }
                Effect::Assign { location, .. } => {
                    applied.push(format!("assign:{location}"));
                }
                Effect::AssignValue { location, .. } => {
                    applied.push(format!("assign-value:{location}"));
                }
                Effect::UpdateFromReturnedValue { location, .. } => {
                    applied.push(format!("update-from-returned-value:{location}"));
                }
                Effect::Iterate { .. } => applied.push("iterate".to_owned()),
                Effect::ExecuteScript(_) => applied.push("script".to_owned()),
                Effect::RunScript(source) => applied.push(format!("run-script:{}", source.uri)),
                Effect::Send(_) => applied.push("send".to_owned()),
                Effect::Cancel { .. } => applied.push("cancel".to_owned()),
                Effect::ExecuteExtension(_) => applied.push("extension".to_owned()),
                Effect::DoneEvent { .. } => applied.push("done-event".to_owned()),
                Effect::ReturnDoneEvent(_) => applied.push("return-done-event".to_owned()),
                Effect::BindStateData { state, .. } => {
                    applied.push(format!("bind:{}", state.as_str()));
                }
                Effect::Invoke { state, invoke } => {
                    applied.push(format!(
                        "invoke:{}:{}",
                        state.as_str(),
                        invoke.document_order
                    ));
                }
                Effect::CancelInvoke { invoke_id } => {
                    applied.push(format!("cancel-invoke:{}", invoke_id.as_str()));
                }
                Effect::Forward { invoke_id, event } => {
                    applied.push(format!(
                        "forward:{}:{}",
                        invoke_id.as_str(),
                        event.name.as_str()
                    ));
                }
            }
        }
    }

    /// Drives one engine command through the full effect transaction,
    /// including deferred conditional continuation batches.
    fn drive_transaction(
        engine: &mut Engine,
        step: CommandResult,
        evaluator: &mut impl GuardEvaluator,
        applied: &mut Vec<String>,
    ) -> CommandResult {
        engine
            .begin_effects(&step)
            .expect("begin_effects must accept a fresh engine step");
        apply_effects(engine, &step, applied);
        let mut final_step = step;
        while let Some(next) = engine
            .continue_effects(evaluator)
            .expect("conditional continuation must plan")
        {
            apply_effects(engine, &next, applied);
            final_step = next;
        }
        engine
            .complete_effects()
            .expect("complete_effects must commit the effect phase");
        final_step.status = engine.status.clone();
        final_step.leaves.clone_from(&engine.active_states);
        final_step.stable = engine.stable;
        final_step
    }

    /// Test executor for [`Engine::run`]: records applied effects, routes
    /// planned raises back into the engine, and fails the `<log>` whose
    /// label is `fail_label` recoverably.
    struct RecordingExecutor {
        applied: Vec<String>,
        fail_label: Option<&'static str>,
        guard: Result<bool, EvaluationError>,
    }

    impl GuardEvaluator for RecordingExecutor {
        fn evaluate_condition(
            &mut self,
            _expression: &str,
            _event: Option<&Event>,
        ) -> Result<bool, EvaluationError> {
            self.guard
        }
    }

    impl EffectExecutor for RecordingExecutor {
        type Error = EngineError;

        fn admit_command(&mut self) -> Result<(), EngineError> {
            Ok(())
        }

        fn command_completed(&mut self) {}

        fn engine_error(&mut self, error: EngineError, _stage: RunStage) -> EngineError {
            error
        }

        fn update_configuration(
            &mut self,
            _exited: &[StateId],
            _entered: &[StateId],
        ) -> Result<(), EngineError> {
            Ok(())
        }

        fn begin_batch(&mut self, _trigger: Option<&Event>) -> Result<(), EngineError> {
            Ok(())
        }

        fn apply_effect(
            &mut self,
            engine: &mut Engine,
            planned: &PlannedEffect,
            _trigger: Option<&Event>,
        ) -> Result<(), EffectFailure<EngineError>> {
            match &planned.effect {
                Effect::Log { label, .. } if label.as_deref() == self.fail_label => {
                    Err(EffectFailure::Block(PlatformError::Execution))
                }
                Effect::Log { label, .. } => {
                    self.applied
                        .push(format!("log:{}", label.clone().unwrap_or_default()));
                    Ok(())
                }
                Effect::Raise(name, ..) => {
                    self.applied.push(format!("raise:{name}"));
                    engine
                        .raise(Event::internal(
                            EventName::new(name.clone()).expect("test event name must be valid"),
                        ))
                        .map_err(EffectFailure::Fatal)
                }
                Effect::Forward { invoke_id, event } => {
                    self.applied.push(format!(
                        "forward:{}:{}",
                        invoke_id.as_str(),
                        event.name.as_str()
                    ));
                    Ok(())
                }
                other => {
                    self.applied.push(format!("other:{other:?}"));
                    Ok(())
                }
            }
        }

        fn end_batch(&mut self, _end: BatchEnd) {}
    }

    fn two_block_machine(second_block: Vec<ExecutableContentIr>) -> MachineIr {
        let mut machine = entry_machine(Vec::new());
        machine.states[0].onentry = vec![
            ActionBlockIr {
                document_order: 10,
                actions: vec![log("first"), log("first-unreachable")],
            },
            ActionBlockIr {
                document_order: 11,
                actions: second_block,
            },
        ];
        machine
    }

    #[test]
    fn recoverable_failure_aborts_only_its_own_block_and_later_blocks_run() {
        // SCXML 3.4/5.7/5.10: each <onentry> handler is an independent
        // block; a recoverable failure in the first must not drop the second
        // block's log or raise (PAT-HSM-002: the engine, not the host, owns
        // recovery).
        let mut engine = Engine::new(two_block_machine(vec![log("second"), raise("finish")]))
            .expect("two-block machine must construct");
        let mut executor = RecordingExecutor {
            applied: Vec::new(),
            fail_label: Some("first"),
            guard: Ok(true),
        };
        let report = engine
            .run(Command::Start, &mut executor)
            .expect("block-scoped recovery must not fail the run");
        assert_eq!(executor.applied, vec!["log:second", "raise:finish"]);
        assert!(report.stable);
        assert_eq!(report.result.status, EngineStatus::Finished);
        assert_eq!(report.result.leaves, vec![sid("done")]);
        assert_eq!(report.committed_effects, 2);
    }

    #[test]
    fn recoverable_failure_keeps_later_blocks_deferred_conditional_work() {
        // The failed block's plan is dropped; a later block's deferred <if>
        // decision still runs.
        let mut engine = Engine::new(two_block_machine(vec![if_action(
            "true",
            vec![log("branch")],
            Vec::new(),
            None,
        )]))
        .expect("two-block conditional machine must construct");
        let mut executor = RecordingExecutor {
            applied: Vec::new(),
            fail_label: Some("first"),
            guard: Ok(true),
        };
        engine
            .run(Command::Start, &mut executor)
            .expect("block-scoped recovery must not fail the run");
        assert_eq!(executor.applied, vec!["log:branch"]);
        assert_eq!(
            engine.status,
            EngineStatus::Running,
            "error.execution was queued and consumed without a transition"
        );
    }

    fn wide_parallel_source(regions: usize) -> String {
        let mut source = String::from(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="wide" datamodel="null"><parallel id="wide">"#,
        );
        for region in 0..regions {
            std::fmt::Write::write_fmt(
                &mut source,
                format_args!(
                    r#"<state id="r{region}" initial="r{region}a"><state id="r{region}a"><transition event="t" target="r{region}b"/></state><state id="r{region}b"><transition event="t" target="r{region}a"/></state></state>"#
                ),
            )
            .expect("writing to a String cannot fail");
        }
        source.push_str("</parallel></scxml>");
        source
    }

    /// Table reads spent by one `t` event on a started `regions`-wide
    /// parallel toggle.
    fn wide_parallel_event_reads(regions: usize) -> u64 {
        let source = wide_parallel_source(regions);
        let document =
            scxml_syntax::parse_xml(scxml_syntax::SourceText::new(SOURCE, "wide.scxml", source))
                .expect("the wide parallel source parses");
        let machine = scxml_compiler_core::lower(&document).expect("the wide parallel lowers");
        let mut engine = Engine::new(machine).expect("machine must construct");
        let mut executor = RecordingExecutor {
            applied: Vec::new(),
            fail_label: None,
            guard: Ok(true),
        };
        engine
            .run(Command::Start, &mut executor)
            .expect("start must run to a stable configuration");
        // One warm toggle, so the measured event starts from a leaf set the
        // engine itself produced.
        engine
            .run(Command::Send(&external("t")), &mut executor)
            .expect("t must run to a stable configuration");
        let before = ENGINE_WORK.with(std::cell::Cell::get);
        engine
            .run(Command::Send(&external("t")), &mut executor)
            .expect("t must run to a stable configuration");
        let reads = ENGINE_WORK.with(std::cell::Cell::get) - before;
        let expected: Vec<StateId> = (0..regions)
            .map(|region| sid(&format!("r{region}a")))
            .collect();
        assert_eq!(engine.leaves(), expected.as_slice());
        reads
    }

    /// Asserts that doubling the region count at most ~doubles the work:
    /// the integer form of `cost(2N) / cost(N) <= 2.5` for each adjacent
    /// pair of `(regions, cost)` samples.
    fn assert_linear_in_regions(label: &str, samples: &[(usize, u64)]) {
        eprintln!("{label} work: {samples:?}");
        for pair in samples.windows(2) {
            let ((narrow, narrow_reads), (wide, wide_reads)) = (pair[0], pair[1]);
            assert!(
                wide_reads * 2 <= narrow_reads * 5,
                "{label}: {wide} regions cost {wide_reads} work vs {narrow_reads} for {narrow}: {samples:?}"
            );
        }
    }

    /// One event on a parallel state costs work linear in its region count:
    /// doubling the regions of an all-toggling parallel at most ~doubles the
    /// engine's table reads per event.
    #[test]
    fn wide_parallel_event_work_scales_linearly_with_regions() {
        let reads: Vec<(usize, u64)> = [8, 16, 32, 64]
            .into_iter()
            .map(|regions| (regions, wide_parallel_event_reads(regions)))
            .collect();
        assert_linear_in_regions("toggle event", &reads);
    }

    /// `p` holds a deep history `h` (default `wide`) and the parallel `wide`
    /// of `regions` two-state regions toggling on `t`; `out` leaves `p` for
    /// `idle`, whose `go` re-enters `p` by default and `restore` through `h`.
    fn wide_parallel_lifecycle_source(regions: usize) -> String {
        let mut source = String::from(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="p" datamodel="null"><state id="p" initial="wide"><history id="h" type="deep"><transition target="wide"/></history><parallel id="wide">"#,
        );
        for region in 0..regions {
            std::fmt::Write::write_fmt(
                &mut source,
                format_args!(
                    r#"<state id="r{region}" initial="r{region}a"><state id="r{region}a"><transition event="t" target="r{region}b"/></state><state id="r{region}b"><transition event="t" target="r{region}a"/></state></state>"#
                ),
            )
            .expect("writing to a String cannot fail");
        }
        source.push_str(
            r#"</parallel><transition event="out" target="idle"/></state><state id="idle"><transition event="go" target="p"/><transition event="restore" target="h"/></state></scxml>"#,
        );
        source
    }

    /// Table reads of each whole-parallel lifecycle step on a
    /// `regions`-wide parallel: `[start, exit, history restore, default
    /// entry]`.
    fn wide_parallel_lifecycle_reads(regions: usize) -> [u64; 4] {
        let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
            SOURCE,
            "wide-lifecycle.scxml",
            wide_parallel_lifecycle_source(regions),
        ))
        .expect("the lifecycle source parses");
        let machine = scxml_compiler_core::lower(&document).expect("the lifecycle lowers");
        let mut engine = Engine::new(machine).expect("machine must construct");
        let mut executor = RecordingExecutor {
            applied: Vec::new(),
            fail_label: None,
            guard: Ok(true),
        };
        let leaves = |suffix: char| -> Vec<StateId> {
            (0..regions)
                .map(|region| sid(&format!("r{region}{suffix}")))
                .collect()
        };
        let mut measure = |engine: &mut Engine, command: Command<'_>| {
            let before = ENGINE_WORK.with(std::cell::Cell::get);
            engine
                .run(command, &mut executor)
                .expect("the command must run to a stable configuration");
            ENGINE_WORK.with(std::cell::Cell::get) - before
        };
        let start = measure(&mut engine, Command::Start);
        assert_eq!(engine.leaves(), leaves('a').as_slice());
        measure(&mut engine, Command::Send(&external("t")));
        let exit = measure(&mut engine, Command::Send(&external("out")));
        assert_eq!(engine.leaves(), [sid("idle")].as_slice());
        let restore = measure(&mut engine, Command::Send(&external("restore")));
        assert_eq!(engine.leaves(), leaves('b').as_slice());
        measure(&mut engine, Command::Send(&external("out")));
        let entry = measure(&mut engine, Command::Send(&external("go")));
        assert_eq!(engine.leaves(), leaves('a').as_slice());
        [start, exit, restore, entry]
    }

    /// Entering a parallel state as a whole (at start, by default entry, or
    /// through deep history) and exiting it cost work linear in its region
    /// count.
    #[test]
    fn wide_parallel_entry_and_exit_work_scales_linearly_with_regions() {
        let samples: Vec<(usize, [u64; 4])> = [8, 16, 32, 64]
            .into_iter()
            .map(|regions| (regions, wide_parallel_lifecycle_reads(regions)))
            .collect();
        eprintln!("[start, exit, history restore, default entry] work: {samples:?}");
        for (step, label) in ["start", "exit", "history restore", "default entry"]
            .into_iter()
            .enumerate()
        {
            let reads: Vec<(usize, u64)> = samples
                .iter()
                .map(|(regions, costs)| (*regions, costs[step]))
                .collect();
            assert_linear_in_regions(label, &reads);
        }
    }

    /// Test executor that meters one run: the engine work spent by each
    /// command (recorded at each admission) and every configuration entry
    /// the engine hands it. Applies `<raise>` and accepts every other effect.
    #[derive(Default)]
    struct MeteredExecutor {
        admitted_at: Vec<u64>,
    }

    impl MeteredExecutor {
        /// Work of each command of the last run, in order, given the
        /// counter value when the run ended.
        fn command_work(&self, ended_at: u64) -> Vec<u64> {
            self.admitted_at
                .iter()
                .zip(self.admitted_at.iter().skip(1).chain([&ended_at]))
                .map(|(start, end)| end - start)
                .collect()
        }
    }

    impl GuardEvaluator for MeteredExecutor {
        fn evaluate_condition(
            &mut self,
            _expression: &str,
            _event: Option<&Event>,
        ) -> Result<bool, EvaluationError> {
            Ok(true)
        }
    }

    impl EffectExecutor for MeteredExecutor {
        type Error = EngineError;

        fn admit_command(&mut self) -> Result<(), EngineError> {
            self.admitted_at
                .push(ENGINE_WORK.with(std::cell::Cell::get));
            Ok(())
        }

        fn command_completed(&mut self) {}

        fn engine_error(&mut self, error: EngineError, _stage: RunStage) -> EngineError {
            error
        }

        fn update_configuration(
            &mut self,
            exited: &[StateId],
            entered: &[StateId],
        ) -> Result<(), EngineError> {
            count_work(exited.len().saturating_add(entered.len()));
            Ok(())
        }

        fn begin_batch(&mut self, _trigger: Option<&Event>) -> Result<(), EngineError> {
            Ok(())
        }

        fn apply_effect(
            &mut self,
            engine: &mut Engine,
            planned: &PlannedEffect,
            _trigger: Option<&Event>,
        ) -> Result<(), EffectFailure<EngineError>> {
            if let Effect::Raise(name, ..) = &planned.effect {
                engine
                    .raise(Event::internal(
                        EventName::new(name.clone()).expect("test event name must be valid"),
                    ))
                    .map_err(EffectFailure::Fatal)?;
            }
            Ok(())
        }

        fn end_batch(&mut self, _end: BatchEnd) {}
    }

    /// A `regions`-wide parallel whose regions carry `<onentry>` log and
    /// `<onexit>` assign content: `t` toggles every region, `finish` moves
    /// every region to its final state in one microstep.
    fn wide_parallel_content_source(regions: usize) -> String {
        let mut source = String::from(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="wide" datamodel="ecmascript"><datamodel><data id="x" expr="0"/></datamodel><parallel id="wide">"#,
        );
        for region in 0..regions {
            std::fmt::Write::write_fmt(
                &mut source,
                format_args!(
                    r#"<state id="r{region}" initial="r{region}a"><state id="r{region}a"><onentry><log label="a" expr="x"/></onentry><onexit><assign location="x" expr="x + 1"/></onexit><transition event="t" target="r{region}b"/><transition event="finish" target="r{region}f"/></state><state id="r{region}b"><onentry><log label="b" expr="x"/></onentry><onexit><assign location="x" expr="x + 1"/></onexit><transition event="t" target="r{region}a"/></state><final id="r{region}f"><onentry><log label="f" expr="x"/></onentry></final></state>"#
                ),
            )
            .expect("writing to a String cannot fail");
        }
        source.push_str("</parallel></scxml>");
        source
    }

    /// Work of `[one toggle event, the all-final microstep command, the
    /// whole all-final event]` on a started `regions`-wide parallel with
    /// entry/exit content.
    fn wide_parallel_content_work(regions: usize) -> [u64; 3] {
        let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
            SOURCE,
            "wide-content.scxml",
            wide_parallel_content_source(regions),
        ))
        .expect("the content source parses");
        let machine = scxml_compiler_core::lower(&document).expect("the content source lowers");
        let mut engine = Engine::new(machine).expect("machine must construct");
        let mut executor = MeteredExecutor::default();
        engine
            .run(Command::Start, &mut executor)
            .expect("start must run to a stable configuration");
        engine
            .run(Command::Send(&external("t")), &mut executor)
            .expect("t must run to a stable configuration");
        let before = ENGINE_WORK.with(std::cell::Cell::get);
        engine
            .run(Command::Send(&external("t")), &mut executor)
            .expect("t must run to a stable configuration");
        let toggle = ENGINE_WORK.with(std::cell::Cell::get) - before;
        let toggled: Vec<StateId> = (0..regions)
            .map(|region| sid(&format!("r{region}a")))
            .collect();
        assert_eq!(engine.leaves(), toggled.as_slice());
        executor.admitted_at.clear();
        let finish = engine
            .run(Command::Send(&external("finish")), &mut executor)
            .expect("finish must run to a stable configuration");
        let finished: Vec<StateId> = (0..regions)
            .map(|region| sid(&format!("r{region}f")))
            .collect();
        assert_eq!(engine.leaves(), finished.as_slice());
        // The microstep, then one resume that disposes every
        // `done.state.r<i>` and `done.state.wide` (none is matched).
        let commands = executor.command_work(ENGINE_WORK.with(std::cell::Cell::get));
        assert_eq!(finish.commands, 2);
        [toggle, commands[0], commands.iter().sum()]
    }

    /// With `<onentry>`/`<onexit>` content in every region, one toggle event
    /// and the microstep that finishes every region cost work linear in the
    /// region count, including the configuration each effect observes.
    #[test]
    fn wide_parallel_content_work_scales_linearly_with_regions() {
        let samples: Vec<(usize, [u64; 3])> = [8, 16, 32, 64]
            .into_iter()
            .map(|regions| (regions, wide_parallel_content_work(regions)))
            .collect();
        eprintln!("[toggle, all-final microstep, all-final event] work: {samples:?}");
        for (step, label) in [
            "toggle with content",
            "all-final microstep",
            "all-final event",
        ]
        .into_iter()
        .enumerate()
        {
            let work: Vec<(usize, u64)> = samples
                .iter()
                .map(|(regions, costs)| (*regions, costs[step]))
                .collect();
            assert_linear_in_regions(label, &work);
        }
    }

    #[test]
    fn run_resumes_to_a_stable_configuration_under_one_bound() {
        let mut engine =
            Engine::new(two_block_machine(vec![raise("noise")])).expect("machine must construct");
        let mut executor = RecordingExecutor {
            applied: Vec::new(),
            fail_label: None,
            guard: Ok(true),
        };
        let report = engine
            .run(Command::Start, &mut executor)
            .expect("start must run to a stable configuration");
        assert!(report.stable);
        assert_eq!(engine.internal_len(), 0);
        assert!(report.commands >= 2, "the raised event needed a resume");
        let finished = engine
            .run(Command::Send(&external("finish")), &mut executor)
            .expect("send must run to a stable configuration");
        assert_eq!(finished.result.status, EngineStatus::Finished);
        assert_eq!(
            engine
                .run(Command::Send(&external("finish")), &mut executor)
                .map_err(|failure| (failure.kind, failure.error)),
            Err((RunFailureKind::Rejected, EngineError::Finished)),
            "a rejected command leaves the engine unchanged"
        );
    }

    /// `wait` holds an eventless transition to the final `done` guarded by
    /// `condition`, then a targetless `go` transition.
    fn guarded_eventless_machine(condition: &str) -> MachineIr {
        let mut eventless = transition(2, 0, vec![1], Vec::new());
        eventless.cond = Some(condition.to_owned());
        MachineIr {
            version: scxml_compiler_core::MACHINE_IR_VERSION,
            name: Some("guarded-eventless".into()),
            data_model: Some("ecmascript".into()),
            binding: Binding::Early,
            initial: InitialIr {
                document_order: 0,
                targets: vec![0],
                actions: Vec::new(),
            },
            states: vec![
                state(StateRow {
                    document_order: 1,
                    id: "wait",
                    kind: StateKind::Atomic,
                    parent: None,
                    children: Vec::new(),
                    initial: None,
                    on_entry: Vec::new(),
                    on_exit: Vec::new(),
                    transitions: vec![0, 1],
                }),
                state(StateRow {
                    document_order: 4,
                    id: "done",
                    kind: StateKind::Final,
                    parent: None,
                    children: Vec::new(),
                    initial: None,
                    on_entry: Vec::new(),
                    on_exit: Vec::new(),
                    transitions: Vec::new(),
                }),
            ],
            transitions: vec![eventless, transition(3, 0, Vec::new(), vec!["go"])],
            data: Vec::new(),
            scripts: Vec::new(),
            extension_namespaces: Vec::new(),
        }
    }

    #[test]
    fn false_guarded_eventless_transition_settles_stable_and_accepts_send() {
        // W3C Appendix D: the macrostep ends when no eventless transition is
        // *enabled* and the internal queue is empty. A structurally present
        // eventless transition whose guard is false must not keep the
        // machine busy (PAT-RTC-001).
        let mut engine =
            Engine::new(guarded_eventless_machine("flag")).expect("fixture must construct");
        let mut executor = RecordingExecutor {
            applied: Vec::new(),
            fail_label: None,
            guard: Ok(false),
        };
        let start = engine
            .run(Command::Start, &mut executor)
            .expect("start must settle");
        assert!(start.stable, "a false guard leaves the machine waiting");
        assert_eq!(
            start.commands, 2,
            "start plus one resume that finds no enabled eventless transition"
        );
        assert_eq!(start.result.leaves, vec![sid("wait")]);

        let waiting = engine
            .run(Command::Send(&external("go")), &mut executor)
            .expect("a waiting machine must accept an external event");
        assert!(waiting.stable);
        assert_eq!(waiting.result.leaves, vec![sid("wait")]);

        // Once the guard holds, the next macrostep takes the eventless
        // transition.
        executor.guard = Ok(true);
        let finished = engine
            .run(Command::Send(&external("go")), &mut executor)
            .expect("the enabled eventless transition must be taken");
        assert_eq!(finished.result.status, EngineStatus::Finished);
        assert_eq!(finished.result.leaves, vec![sid("done")]);
    }

    /// Enables a guard only when `_event` is bound to `name`, recording the
    /// `_event` name every evaluation observed.
    struct EventNameGuard {
        name: &'static str,
        observed: Vec<Option<String>>,
    }

    impl GuardEvaluator for EventNameGuard {
        fn evaluate_condition(
            &mut self,
            _expression: &str,
            event: Option<&Event>,
        ) -> Result<bool, EvaluationError> {
            let observed = event.map(|event| event.name.as_str().to_owned());
            let enabled = observed.as_deref() == Some(self.name);
            self.observed.push(observed);
            Ok(enabled)
        }
    }

    #[test]
    fn eventless_selection_reruns_with_event_bound_after_each_ignored_internal_event() {
        // W3C Appendix D `mainEventLoop`: after an internal event enables no
        // transition, eventless selection runs again with `_event` bound to
        // that event before the next internal event is dequeued.
        let mut machine = guarded_eventless_machine("_event.name == 'noise'");
        machine.states[0].onentry = vec![ActionBlockIr {
            document_order: 5,
            actions: vec![raise("noise"), raise("other")],
        }];
        let mut engine = Engine::new(machine).expect("fixture must construct");
        let mut guard = EventNameGuard {
            name: "noise",
            observed: Vec::new(),
        };
        let start = engine.start().expect("start must plan");
        let mut applied = Vec::new();
        let start = drive_transaction(&mut engine, start, &mut guard, &mut applied);
        assert!(!start.stable);

        let step = engine
            .resume(&mut guard)
            .expect("the eventless transition must be selected");
        assert_eq!(
            guard.observed,
            vec![None, Some("noise".to_owned())],
            "eventless guards run without _event, then with the ignored internal event"
        );
        assert_eq!(
            step.event_dispositions
                .iter()
                .map(|disposed| (disposed.event.name.as_str(), disposed.disposition))
                .collect::<Vec<_>>(),
            vec![("noise", EventDisposition::Ignored)],
            "`other` stays queued behind the eventless microstep"
        );
        assert_eq!(
            step.trigger_event.as_ref().map(|event| event.name.as_str()),
            Some("noise"),
            "the eventless microstep keeps `_event` bound to the ignored event"
        );
        assert_eq!(step.status, EngineStatus::Finished);
    }

    #[test]
    fn ignored_self_addressed_external_event_stays_bound_for_eventless_selection() {
        // W3C Appendix D: an external event that enables no transition stays
        // `_event` for the eventless selection that starts the next
        // macrostep.
        let mut engine = Engine::new(guarded_eventless_machine("_event.name == 'other'"))
            .expect("fixture must construct");
        let mut guard = EventNameGuard {
            name: "other",
            observed: Vec::new(),
        };
        let start = engine.start().expect("start must plan");
        engine
            .begin_effects(&start)
            .expect("begin_effects must accept the start step");
        engine
            .enqueue_external(external("other"))
            .expect("a self-addressed external event must enqueue");
        engine
            .complete_effects()
            .expect("the start transaction must commit");

        let mut last = None;
        for _ in 0..4 {
            let step = engine.resume(&mut guard).expect("resume must progress");
            complete_empty_effect_step(&mut engine, &step);
            let done = step.status == EngineStatus::Finished || step.stable;
            last = Some(step);
            if done {
                break;
            }
        }
        let last = last.expect("at least one resume ran");
        assert_eq!(
            last.status,
            EngineStatus::Finished,
            "eventless selection must observe the ignored external event: {:?}",
            guard.observed
        );
        assert_eq!(guard.observed.last(), Some(&Some("other".to_owned())));
    }

    #[test]
    fn recoverable_transition_guard_failures_raise_error_execution_and_evaluate_false() {
        // SCXML 5.9/5.10 and the WIT `evaluate-condition` contract: a cond
        // that returns `invalid-input` or `execution` raises error.execution
        // and is treated as false; the run succeeds.
        for failure in [EvaluationError::InvalidInput, EvaluationError::Execution] {
            let mut machine = machine(Some("cond"), false);
            machine.transitions.push(TransitionIr {
                document_order: 5,
                source: 0,
                targets: vec![1],
                events: vec!["error.execution".into()],
                cond: None,
                transition_type: TransitionType::External,
                actions: Vec::new(),
                event_schema: None,
            });
            machine.states[0].transitions = vec![0, 1];
            let mut engine = Engine::new(machine).expect("fixture must construct");
            let mut executor = RecordingExecutor {
                applied: Vec::new(),
                fail_label: None,
                guard: Err(failure),
            };
            engine
                .run(Command::Start, &mut executor)
                .expect("start must settle");
            let report = engine
                .run(Command::Send(&external("finish")), &mut executor)
                .unwrap_or_else(|run| panic!("{failure:?} guard must not fail the run: {run:?}"));
            assert!(report.stable);
            assert_eq!(
                report.result.leaves,
                vec![sid("done")],
                "{failure:?}: the guarded transition was skipped and error.execution was taken"
            );
            assert!(
                !executor.applied.contains(&"raise:raised".to_owned()),
                "{failure:?}: the guarded transition's content must not run"
            );
        }
    }

    #[test]
    fn fatal_transition_guard_failures_still_fail_the_run() {
        for failure in [EvaluationError::Unavailable, EvaluationError::Platform] {
            let mut engine =
                Engine::new(machine(Some("cond"), false)).expect("fixture must construct");
            let mut executor = RecordingExecutor {
                applied: Vec::new(),
                fail_label: None,
                guard: Err(failure),
            };
            engine
                .run(Command::Start, &mut executor)
                .expect("start must settle");
            let run = engine
                .run(Command::Send(&external("finish")), &mut executor)
                .expect_err("a platform-class guard failure must fail closed");
            assert_eq!(run.error, EngineError::Evaluation(failure));
        }
    }

    #[test]
    fn recoverable_conditional_failures_raise_error_execution_and_take_the_else_branch() {
        // SCXML 5.9: an `<if>`/`<elseif>` cond that cannot be evaluated is
        // treated as false and raises error.execution; later clauses and the
        // `<else>` still run and the run succeeds.
        for failure in [EvaluationError::InvalidInput, EvaluationError::Execution] {
            let mut machine = entry_machine(vec![
                if_action(
                    "cond",
                    vec![log("taken")],
                    Vec::new(),
                    Some(vec![log("else")]),
                ),
                log("after"),
            ]);
            machine.transitions[0].events = vec!["error.execution".into()];
            let mut engine = Engine::new(machine).expect("fixture must construct");
            let mut executor = RecordingExecutor {
                applied: Vec::new(),
                fail_label: None,
                guard: Err(failure),
            };
            let report = engine
                .run(Command::Start, &mut executor)
                .unwrap_or_else(|run| panic!("{failure:?} `<if>` cond must not fail: {run:?}"));
            assert_eq!(
                executor.applied,
                vec!["log:else", "log:after"],
                "{failure:?}: the failed clause is false and the block continues"
            );
            assert_eq!(
                report.result.status,
                EngineStatus::Finished,
                "{failure:?}: error.execution was raised and selected"
            );
        }
    }

    #[test]
    fn fatal_conditional_failures_still_fail_the_run() {
        for failure in [EvaluationError::Unavailable, EvaluationError::Platform] {
            let mut engine = Engine::new(entry_machine(vec![if_action(
                "cond",
                vec![log("taken")],
                Vec::new(),
                Some(vec![log("else")]),
            )]))
            .expect("fixture must construct");
            let mut executor = RecordingExecutor {
                applied: Vec::new(),
                fail_label: None,
                guard: Err(failure),
            };
            let run = engine
                .run(Command::Start, &mut executor)
                .expect_err("a platform-class `<if>` failure must fail closed");
            assert_eq!(run.error, EngineError::Evaluation(failure));
            assert_eq!(run.kind, RunFailureKind::Effect);
            assert!(executor.applied.is_empty());
        }
    }

    #[test]
    fn autoforward_forwards_every_dequeued_external_event_before_selection() {
        // SCXML 6.4.1 / Appendix D: sent events and the engine's own
        // self-addressed external queue are both forwarded.
        let mut machine = invoke_machine();
        machine.states[0].invokes[0].autoforward = true;
        machine.states[0].invokes[0].children = Vec::new();
        let mut engine = Engine::new(machine).expect("autoforward machine must construct");
        let started = engine.start().expect("start");
        complete_empty_effect_step(&mut engine, &started);
        let mut evaluator = CurrentEventEvaluator::default();
        let invoke_step = engine.resume(&mut evaluator).expect("invoke step");
        engine.begin_effects(&invoke_step).expect("begin");
        engine
            .invocation_started(
                &sid("ready"),
                10,
                InvokeId::new("inv").expect("id"),
                scxml_types::SCXML_INVOKE_TYPE_URI.to_owned(),
            )
            .expect("started");
        engine.complete_effects().expect("complete");

        let sent = engine
            .send(&external("other"), &mut evaluator)
            .expect("send plans the forward prelude");
        assert!(sent.event_dispositions.is_empty(), "selection is deferred");
        assert_eq!(
            sent.effects,
            vec![Effect::Forward {
                invoke_id: InvokeId::new("inv").expect("id"),
                event: Box::new(external("other")),
            }]
        );
        engine.begin_effects(&sent).expect("begin");
        engine
            .enqueue_external(external("ping"))
            .expect("self-addressed send queues");
        engine.complete_effects().expect("complete");
        let dispatched = engine.resume(&mut evaluator).expect("pending dispatch");
        assert_eq!(dispatched.event_dispositions[0].event, external("other"));
        complete_empty_effect_step(&mut engine, &dispatched);
        let drained = engine
            .resume(&mut evaluator)
            .expect("the self-addressed external event is dequeued");
        assert_eq!(
            drained.effects,
            vec![Effect::Forward {
                invoke_id: InvokeId::new("inv").expect("id"),
                event: Box::new(external("ping")),
            }],
            "an event drained from the engine's own external queue is forwarded"
        );
    }

    #[test]
    fn conditional_selects_the_first_true_clause_in_document_order() {
        let machine = entry_machine(vec![
            if_action(
                "false",
                vec![raise("foo"), log("if-body")],
                vec![
                    scxml_compiler_core::ElseIfIr {
                        cond: "false".to_owned(),
                        actions: vec![log("elseif-1-body")],
                    },
                    scxml_compiler_core::ElseIfIr {
                        cond: "true".to_owned(),
                        actions: vec![log("elseif-2-body")],
                    },
                ],
                Some(vec![log("else-body")]),
            ),
            log("after-if"),
        ]);
        let mut engine = Engine::new(machine).expect("conditional Machine IR must be valid");
        let mut applied = Vec::new();
        let mut evaluator = ExpressionEvaluator::default();
        let started = engine.start().expect("start must plan");
        let step = drive_transaction(&mut engine, started, &mut evaluator, &mut applied);
        // The untaken clauses never execute and the trailing content does.
        assert_eq!(
            applied,
            vec!["log:elseif-2-body", "log:after-if"],
            "first true elseif wins and remaining content follows"
        );
        assert_eq!(step.status, EngineStatus::Running);
        assert_eq!(
            evaluator.calls, 3,
            "false, false, true are evaluated in order"
        );
    }

    #[test]
    fn conditional_else_runs_when_every_condition_is_false() {
        let machine = entry_machine(vec![
            if_action(
                "false",
                vec![log("if-body")],
                vec![scxml_compiler_core::ElseIfIr {
                    cond: "false".to_owned(),
                    actions: vec![log("elseif-body")],
                }],
                Some(vec![raise("else-raised")]),
            ),
            log("after-if"),
        ]);
        let mut engine = Engine::new(machine).expect("conditional Machine IR must be valid");
        let mut applied = Vec::new();
        let mut evaluator = Evaluator {
            allow: false,
            calls: 0,
        };
        let started = engine.start().expect("start must plan");
        drive_transaction(&mut engine, started, &mut evaluator, &mut applied);
        assert_eq!(
            applied,
            vec!["log:after-if"],
            "logs collect in the host while the else raise enters the internal queue"
        );
        assert_eq!(
            engine
                .internal
                .iter()
                .map(|event| event.name.as_str().to_owned())
                .collect::<Vec<_>>(),
            vec!["else-raised"],
            "the else branch raised its internal event"
        );
        assert_eq!(evaluator.calls, 2, "if and elseif conditions are evaluated");
    }

    #[test]
    fn conditional_decision_observes_effects_applied_before_it() {
        // The evaluator answers false until the prior assign effect has been
        // applied, then true: branch selection must see the mutated state.
        struct LazyEvaluator {
            assigned: bool,
        }
        impl GuardEvaluator for LazyEvaluator {
            fn evaluate_condition(
                &mut self,
                _expression: &str,
                _event: Option<&Event>,
            ) -> Result<bool, EvaluationError> {
                Ok(self.assigned)
            }
        }
        let machine = entry_machine(vec![
            assign("flag"),
            if_action(
                "flag",
                vec![log("taken")],
                Vec::new(),
                Some(vec![log("else")]),
            ),
        ]);
        let mut engine = Engine::new(machine).expect("lazy conditional Machine IR must be valid");
        let step = engine.start().expect("start must plan");
        // The first batch contains only the assign: the decision waits.
        assert_eq!(step.planned_effects.len(), 1);
        assert!(matches!(
            step.planned_effects[0].effect,
            Effect::Assign { .. }
        ));
        let mut applied = Vec::new();
        engine
            .begin_effects(&step)
            .expect("begin_effects must accept the start step");
        apply_effects(&mut engine, &step, &mut applied);
        let mut evaluator = LazyEvaluator { assigned: false };
        let pending_context = engine
            .pending_condition_context()
            .expect("a deferred conditional decision must be pending");
        assert_eq!(pending_context.phase, EffectExecutionPhase::Entry);
        // The assign effect executes before the condition is evaluated.
        evaluator.assigned = true;
        let next = engine
            .continue_effects(&mut evaluator)
            .expect("continuation must resolve the decision")
            .expect("a taken branch must plan its body");
        apply_effects(&mut engine, &next, &mut applied);
        assert!(
            engine
                .continue_effects(&mut evaluator)
                .expect("continuation must drain")
                .is_none()
        );
        engine
            .complete_effects()
            .expect("complete_effects must commit the effect phase");
        assert_eq!(applied, vec!["assign:flag", "log:taken"]);
    }

    #[test]
    fn nested_conditional_completes_before_trailing_content() {
        let machine = entry_machine(vec![
            if_action(
                "true",
                vec![
                    if_action("true", vec![log("inner")], Vec::new(), None),
                    log("outer-branch-tail"),
                ],
                Vec::new(),
                None,
            ),
            log("after-if"),
        ]);
        let mut engine = Engine::new(machine).expect("nested conditional Machine IR must be valid");
        let mut applied = Vec::new();
        let mut evaluator = Evaluator {
            allow: true,
            calls: 0,
        };
        let started = engine.start().expect("start must plan");
        drive_transaction(&mut engine, started, &mut evaluator, &mut applied);
        assert_eq!(
            applied,
            vec!["log:inner", "log:outer-branch-tail", "log:after-if"],
            "nested branch completes in document order before trailing content"
        );
        assert_eq!(evaluator.calls, 2);
    }

    #[test]
    fn complete_effects_fails_closed_while_conditional_work_remains() {
        let machine = entry_machine(vec![if_action(
            "true",
            vec![log("taken")],
            Vec::new(),
            None,
        )]);
        let mut engine = Engine::new(machine).expect("conditional Machine IR must be valid");
        let step = engine.start().expect("start must plan");
        engine
            .begin_effects(&step)
            .expect("begin_effects must accept the start step");
        let error = engine
            .complete_effects()
            .expect_err("dropping pending conditional work must fail closed");
        assert_eq!(error, EngineError::EffectProtocol);
    }

    #[test]
    fn continue_effects_outside_the_effect_phase_fails_closed() {
        let machine = entry_machine(Vec::new());
        let mut engine = Engine::new(machine).expect("Machine IR must be valid");
        let mut evaluator = Evaluator {
            allow: true,
            calls: 0,
        };
        let error = engine
            .continue_effects(&mut evaluator)
            .expect_err("continuation outside an effect phase must fail closed");
        assert_eq!(error, EngineError::EffectProtocol);
    }

    #[test]
    fn fatal_conditional_evaluation_failure_preserves_the_pending_plan() {
        let machine = entry_machine(vec![if_action(
            "flag",
            vec![log("taken")],
            Vec::new(),
            None,
        )]);
        let mut engine = Engine::new(machine).expect("conditional Machine IR must be valid");
        let step = engine.start().expect("start must plan");
        engine
            .begin_effects(&step)
            .expect("begin_effects must accept the start step");
        let error = engine
            .continue_effects(&mut PlatformFailingEvaluator)
            .expect_err("guard failure must propagate");
        assert_eq!(error, EngineError::Evaluation(EvaluationError::Platform));
        // The decision stays pending so complete_effects still fails closed.
        let error = engine
            .complete_effects()
            .expect_err("a failed decision must not commit the effect phase");
        assert_eq!(error, EngineError::EffectProtocol);
    }

    #[test]
    fn fatal_conditional_evaluation_failure_is_retried_not_skipped() {
        struct OnceFailing {
            failed: bool,
        }
        impl GuardEvaluator for OnceFailing {
            fn evaluate_condition(
                &mut self,
                _expression: &str,
                _event: Option<&Event>,
            ) -> Result<bool, EvaluationError> {
                if self.failed {
                    Ok(true)
                } else {
                    self.failed = true;
                    Err(EvaluationError::Platform)
                }
            }
        }
        // A fatal evaluator failure must leave the decision pending: a retry
        // re-evaluates the same condition and the branch body executes
        // (recoverable failures are false clauses; see
        // `recoverable_conditional_failures_raise_error_execution_and_take_the_else_branch`).
        let machine = entry_machine(vec![if_action(
            "flag",
            vec![log("taken")],
            Vec::new(),
            Some(vec![log("else")]),
        )]);
        let mut engine = Engine::new(machine).expect("conditional Machine IR must be valid");
        let step = engine.start().expect("start must plan");
        engine
            .begin_effects(&step)
            .expect("begin_effects must accept the start step");
        assert!(engine.pending_condition_context().is_some());
        let error = engine
            .continue_effects(&mut PlatformFailingEvaluator)
            .expect_err("guard failure must propagate");
        assert_eq!(error, EngineError::Evaluation(EvaluationError::Platform));
        assert!(
            engine.pending_condition_context().is_some(),
            "the failed decision must remain pending for retry"
        );
        let mut retry = OnceFailing { failed: false };
        // The first retry fails again and must still preserve the decision.
        let error = engine
            .continue_effects(&mut retry)
            .expect_err("the retry's own first evaluation fails as scripted");
        assert_eq!(error, EngineError::Evaluation(EvaluationError::Platform));
        assert!(engine.pending_condition_context().is_some());
        let next = engine
            .continue_effects(&mut retry)
            .expect("a retry after fatal failures must resolve the decision")
            .expect("the taken branch must plan its body");
        let labels = next
            .planned_effects
            .iter()
            .filter_map(|planned| match &planned.effect {
                Effect::Log { label, .. } => label.clone(),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            vec!["taken"],
            "the retried decision selects the if branch"
        );
    }

    #[test]
    fn conditional_content_in_transition_actions_executes_lazily() {
        let mut machine = entry_machine(Vec::new());
        machine.transitions[0].actions = vec![
            assign("flag"),
            if_action("flag", vec![raise("from-branch")], Vec::new(), None),
            log("after-if"),
        ];
        let mut engine =
            Engine::new(machine).expect("transition conditional Machine IR must be valid");
        let mut applied = Vec::new();
        let initial = engine.start().expect("start must plan");
        let mut evaluator = Evaluator {
            allow: true,
            calls: 0,
        };
        drive_transaction(&mut engine, initial, &mut evaluator, &mut applied);
        let event = Event::external(EventName::new("finish".to_owned()).expect("event name"));
        let step = engine
            .send(&event, &mut evaluator)
            .expect("finish event must select the transition");
        let mut transition_applied = Vec::new();
        let step = drive_transaction(&mut engine, step, &mut evaluator, &mut transition_applied);
        assert_eq!(
            transition_applied,
            vec!["assign:flag", "log:after-if"],
            "the branch raise joins the internal queue while trailing content executes"
        );
        assert_eq!(step.status, EngineStatus::Finished);
    }

    fn foreach(
        array: &str,
        index: Option<&str>,
        actions: Vec<ExecutableContentIr>,
    ) -> ExecutableContentIr {
        ExecutableContentIr::Foreach {
            array: array.to_owned(),
            item: "item".to_owned(),
            index: index.map(str::to_owned),
            actions,
        }
    }

    /// Executor for `<foreach>` tests: an `Iterate` whose array expression
    /// is a decimal count reports that many numeric elements through
    /// [`Engine::begin_iteration`]; logs and item/index assignments record.
    #[derive(Default)]
    struct ForeachExecutor {
        applied: Vec<String>,
    }

    impl GuardEvaluator for ForeachExecutor {
        fn evaluate_condition(
            &mut self,
            expression: &str,
            _event: Option<&Event>,
        ) -> Result<bool, EvaluationError> {
            Ok(expression == "true")
        }
    }

    impl EffectExecutor for ForeachExecutor {
        type Error = EngineError;

        fn admit_command(&mut self) -> Result<(), EngineError> {
            Ok(())
        }

        fn command_completed(&mut self) {}

        fn engine_error(&mut self, error: EngineError, _stage: RunStage) -> EngineError {
            error
        }

        fn update_configuration(
            &mut self,
            _exited: &[StateId],
            _entered: &[StateId],
        ) -> Result<(), EngineError> {
            Ok(())
        }

        fn begin_batch(&mut self, _trigger: Option<&Event>) -> Result<(), EngineError> {
            Ok(())
        }

        fn apply_effect(
            &mut self,
            engine: &mut Engine,
            planned: &PlannedEffect,
            _trigger: Option<&Event>,
        ) -> Result<(), EffectFailure<EngineError>> {
            match &planned.effect {
                Effect::Iterate {
                    array_expression, ..
                } => {
                    let count: u32 = array_expression
                        .parse()
                        .expect("fixture foreach arrays are decimal counts");
                    let elements = (0..count)
                        .map(|value| {
                            DataValue::Number(
                                FiniteNumber::new(f64::from(value)).expect("finite element"),
                            )
                        })
                        .collect::<Vec<_>>();
                    self.applied.push(format!("iterate:{count}"));
                    engine
                        .begin_iteration(&elements, 0)
                        .map_err(EffectFailure::Fatal)
                }
                Effect::AssignValue { location, value } => {
                    self.applied.push(format!("{location}={value:?}"));
                    Ok(())
                }
                Effect::Log { label, .. } => {
                    self.applied
                        .push(format!("log:{}", label.clone().unwrap_or_default()));
                    Ok(())
                }
                other => {
                    self.applied.push(format!("other:{other:?}"));
                    Ok(())
                }
            }
        }

        fn end_batch(&mut self, _end: BatchEnd) {}
    }

    /// Natively one command has no effect bound: a loop planning more than the
    /// wasm bound (10,000 effects) runs to completion.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_steps_have_no_effect_bound() {
        let elements = 10_000 / 3 + 1;
        let mut engine = Engine::new(entry_machine(vec![foreach(
            &elements.to_string(),
            None,
            vec![
                log("a"),
                log("b"),
                if_action("false", Vec::new(), Vec::new(), None),
            ],
        )]))
        .expect("foreach machine must construct");
        let mut executor = ForeachExecutor::default();
        engine
            .run(Command::Start, &mut executor)
            .expect("natively a step plans any number of effects");
        assert_eq!(
            executor
                .applied
                .iter()
                .filter(|applied| applied.starts_with("log:a"))
                .count(),
            elements
        );
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn wasm_only_foreach_preflight_counts_body_effects_per_element() {
        // Every element plans one item assignment plus the body's two log
        // effects (SCXML 4.6). The body's `<if>` splits the loop into
        // separately applied batches, so a bound that counts only the
        // item/index assignments would let early iterations commit before a
        // later batch hits the per-command bound. The pre-flight bound must count
        // the body and reject the loop before any iteration commits.
        let body = vec![
            log("a"),
            log("b"),
            if_action("false", Vec::new(), Vec::new(), None),
        ];
        let per_element = 3;
        let elements = MAX_EFFECTS_PER_COMMAND / per_element + 1;
        let mut engine = Engine::new(entry_machine(vec![foreach(
            &elements.to_string(),
            None,
            body,
        )]))
        .expect("foreach machine must construct");
        let mut executor = ForeachExecutor::default();
        let failure = engine
            .run(Command::Start, &mut executor)
            .expect_err("a loop whose body expansion exceeds the step bound must fail");
        assert_eq!(failure.error, EngineError::EffectLimit);
        assert_eq!(
            executor.applied,
            [format!("iterate:{elements}")],
            "no iteration may commit before the bound rejects the loop"
        );
    }

    #[test]
    fn foreach_preflight_accepts_a_body_expansion_within_the_bound() {
        let mut engine = Engine::new(entry_machine(vec![foreach(
            "2",
            Some("index"),
            vec![log("a"), log("b")],
        )]))
        .expect("foreach machine must construct");
        let mut executor = ForeachExecutor::default();
        engine
            .run(Command::Start, &mut executor)
            .expect("an in-bound loop runs");
        assert_eq!(
            executor.applied,
            [
                "iterate:2",
                "item=Number(FiniteNumber(0.0))",
                "index=Number(FiniteNumber(0.0))",
                "log:a",
                "log:b",
                "item=Number(FiniteNumber(1.0))",
                "index=Number(FiniteNumber(1.0))",
                "log:a",
                "log:b",
            ]
        );
    }

    #[test]
    fn foreach_nested_in_a_foreach_body_or_an_if_branch_iterates() {
        // SCXML 4.6/4.3: `<foreach>` is ordinary executable content, so it may
        // appear inside another loop body or a conditional branch; each one
        // is iterated by the engine, never emitted without its iteration.
        let mut engine = Engine::new(entry_machine(vec![
            foreach("2", None, vec![foreach("1", None, vec![log("inner")])]),
            if_action(
                "true",
                vec![foreach("1", None, vec![log("branch")])],
                Vec::new(),
                None,
            ),
        ]))
        .expect("nested foreach machine must construct");
        let mut executor = ForeachExecutor::default();
        engine
            .run(Command::Start, &mut executor)
            .expect("nested loops run");
        assert_eq!(
            executor.applied,
            [
                "iterate:2",
                "item=Number(FiniteNumber(0.0))",
                "iterate:1",
                "item=Number(FiniteNumber(0.0))",
                "log:inner",
                "item=Number(FiniteNumber(1.0))",
                "iterate:1",
                "item=Number(FiniteNumber(0.0))",
                "log:inner",
                "iterate:1",
                "item=Number(FiniteNumber(0.0))",
                "log:branch",
            ]
        );
    }

    #[test]
    fn assign_placement_maps_every_b2_type_and_rejects_inconsistent_attr() {
        use scxml_types::AssignPlacement as P;
        for (assign_type, attr, expected) in [
            (None, None, P::ReplaceChildren),
            (
                Some(AssignTypeIr::Replacechildren),
                None,
                P::ReplaceChildren,
            ),
            (Some(AssignTypeIr::Firstchild), None, P::FirstChild),
            (Some(AssignTypeIr::Lastchild), None, P::LastChild),
            (
                Some(AssignTypeIr::Previoussibling),
                None,
                P::PreviousSibling,
            ),
            (Some(AssignTypeIr::Nextsibling), None, P::NextSibling),
            (Some(AssignTypeIr::Replace), None, P::Replace),
            (Some(AssignTypeIr::Delete), None, P::Delete),
            (
                Some(AssignTypeIr::Addattribute),
                Some("n"),
                P::AddAttribute("n".into()),
            ),
        ] {
            assert_eq!(assign_placement(assign_type, attr), Ok(expected));
        }
        assert_eq!(
            assign_placement(Some(AssignTypeIr::Addattribute), None),
            Err(EngineError::BrokenInvariant)
        );
        assert_eq!(
            assign_placement(Some(AssignTypeIr::Firstchild), Some("n")),
            Err(EngineError::BrokenInvariant)
        );
    }

    #[test]
    fn assign_source_prefers_content_then_expression_then_absent() {
        let xml = DataValue::Xml("<a/>".into());
        assert_eq!(
            assign_source(Some("1"), Some(&xml)),
            AssignSource::Value(xml)
        );
        assert_eq!(
            assign_source(Some("1"), None),
            AssignSource::Expression("1".into())
        );
        assert_eq!(
            assign_source(None, None),
            AssignSource::Value(DataValue::Absent)
        );
    }

    #[test]
    fn cancel_cancels_started_invocations_of_every_exited_state() {
        // W3C exitInterpreter: cancelling a session also cancels every
        // invocation its active states started (SCXML 6.4).
        let mut engine = started_invoke_engine();
        let mut executor = RecordingExecutor {
            applied: Vec::new(),
            fail_label: None,
            guard: Ok(true),
        };
        let report = engine
            .run(Command::Cancel, &mut executor)
            .expect("a running invoking session cancels");
        assert_eq!(
            executor.applied,
            [format!(
                "other:{:?}",
                Effect::CancelInvoke {
                    invoke_id: InvokeId::new("inv").expect("id"),
                }
            )]
        );
        assert_eq!(report.result.status, EngineStatus::Finished);
        assert_eq!(report.result.exited_states, vec![sid("ready")]);
    }
}
