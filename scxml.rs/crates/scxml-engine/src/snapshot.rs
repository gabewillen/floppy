//! Read-only engine image for LLM introspection (`docs/session-snapshot.md`,
//! "Document").
//!
//! [`Engine::snapshot`] copies the engine's own state and computes the
//! triggerable transitions with the same exit-set, domain, and entry-set
//! functions a microstep uses. It never evaluates a condition, matches an
//! event, runs content, or writes a field, so a snapshot cannot perturb the
//! run. Nothing here runs unless a caller asks for a snapshot.

use std::borrow::Cow;

use scxml_compiler_core::TransitionType;
use scxml_telemetry::{Module, Operation, observe};
use scxml_types::{Event, InvokeId, StateId};

use crate::{
    ActiveConfiguration, EffectPhase, Engine, EngineError, EngineStatus, StateKind, complete,
};

/// What [`Engine::snapshot`] computes beyond the configuration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SnapshotOptions {
    /// Whether to list the transitions an event could trigger.
    pub transitions: TransitionListing,
    /// Whether to fill each listed transition's `exit` and `entry` sets.
    pub previews: Previews,
}

/// Whether [`EngineSnapshot::transitions`] lists the triggerable
/// transitions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TransitionListing {
    /// Leave the list empty.
    #[default]
    Omit,
    /// List every transition an event could trigger.
    Include,
}

/// Whether each listed transition carries its `exit` and `entry` sets.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Previews {
    /// Leave `exit` and `entry` empty.
    #[default]
    Omit,
    /// Compute `exit` and `entry` for each listed transition.
    Include,
}

/// One recorded history value (SCXML 3.11).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryValue {
    /// The history state.
    pub state: StateId,
    /// Stored members in document order.
    pub value: Vec<StateId>,
}

/// A started, not yet cancelled invocation (SCXML 6.4).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvocationSnapshot {
    /// Invoke id.
    pub invoke_id: InvokeId,
    /// Invoking state.
    pub state: StateId,
    /// Index of the `<invoke>` among the state's invokes, document order.
    pub index: u32,
    /// The `<invoke autoforward>` flag.
    pub autoforward: bool,
    /// The invocation's type URI: the canonical SCXML one
    /// ([`scxml_types::SCXML_INVOKE_TYPE_URI`]) for an invoked session, the
    /// exact service type URI for an invoked service.
    pub type_uri: String,
}

/// A transition an event could trigger from the current configuration.
/// Its condition is authored text and is never evaluated.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransitionSnapshot {
    /// Source state.
    pub source: StateId,
    /// Index among the source's transitions, document order.
    pub index: u32,
    /// Event descriptors (never empty).
    pub events: Vec<String>,
    /// Target states; empty for a targetless transition.
    pub targets: Vec<StateId>,
    /// SCXML 3.5 transition type.
    pub transition_type: TransitionType,
    /// Authored `cond` text.
    pub cond: Option<String>,
    /// SCXML 3.13 transition domain; `None` when targetless or `<scxml>`.
    pub domain: Option<StateId>,
    /// This transition's exit set taken alone, exit order; empty unless
    /// previews were requested or when targetless.
    pub exit: Vec<StateId>,
    /// This transition's entry set taken alone, entry order; empty unless
    /// previews were requested or when targetless.
    pub entry: Vec<StateId>,
    /// The transition's `event` vocabulary, copied from the Machine IR
    /// (metadata only: the engine never reads it while it executes).
    pub event_schema: Option<scxml_compiler_core::EventSchemaIr>,
}

/// Read-only image of one engine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineSnapshot {
    /// Lifecycle state.
    pub status: EngineStatus,
    /// `_name`: the document `name`; `None` when the document has none.
    pub name: Option<String>,
    /// Active atomic/final states in document order.
    pub atomic: Vec<StateId>,
    /// Full active configuration, ancestors included, document order.
    pub configuration: Vec<StateId>,
    /// Internal queue, oldest first (empty in a stable configuration).
    pub internal: Vec<Event>,
    /// Recorded history values in document order of the history state.
    pub history: Vec<HistoryValue>,
    /// Late-binding states already bound (SCXML 5.3), document order.
    pub bound_states: Vec<StateId>,
    /// Live invocations in ascending invoke-id byte order.
    pub invocations: Vec<InvocationSnapshot>,
    /// Triggerable transitions; empty unless requested.
    pub transitions: Vec<TransitionSnapshot>,
}

impl Engine {
    /// Read-only image of the engine.
    ///
    /// Evaluates no condition, runs no content, and changes nothing. The
    /// internal queue is reported as held, so a caller that needs a
    /// stable checkpoint gates on [`Engine::is_stable`] itself.
    ///
    /// # Errors
    ///
    /// [`EngineError::EffectProtocol`] while an effect phase is open or a
    /// planned command result is not yet applied (the state is mid-transaction), and
    /// [`EngineError::BrokenInvariant`] for a table reference that does not
    /// resolve.
    pub fn snapshot(&self, options: SnapshotOptions) -> Result<EngineSnapshot, EngineError> {
        let span = observe(Module::Engine, Operation::EngineSnapshot);
        let result = self.capture(options);
        complete(&span, &result);
        result
    }

    /// The executable-content image: a read-only image of the engine at the
    /// effect being applied (`docs/session-snapshot.md`, "Snapshots during
    /// executable content"), for an executable-content extension effect whose
    /// extension component reads the session snapshot.
    ///
    /// `atomic` and `configuration` are the configuration the effect
    /// observes, the one `In()` answers (SCXML 5.9.1; official case 409):
    /// exit content no longer sees the states exited before it, entry
    /// content sees every state entered before it. `transitions` walks
    /// "Actions" from each innermost state of that configuration (a state
    /// none of whose children is in it; at a checkpoint exactly the atomic
    /// states), so the entry content of a compound state lists that
    /// state's transitions before its children are entered; `exit` and
    /// `entry` are always empty. History values, bound states,
    /// invocations, and the internal queue are reported as held at this
    /// effect (history this microstep records on exit is already recorded,
    /// SCXML 3.10). Like [`Engine::snapshot`], it evaluates nothing and
    /// changes nothing.
    ///
    /// # Errors
    ///
    /// [`EngineError::EffectProtocol`] when no effect phase is open, and
    /// [`EngineError::BrokenInvariant`] for a table reference that does not
    /// resolve.
    pub fn effect_snapshot(&self) -> Result<EngineSnapshot, EngineError> {
        let span = observe(Module::Engine, Operation::EngineSnapshot);
        let result = self.capture_effect();
        complete(&span, &result);
        result
    }

    fn capture(&self, options: SnapshotOptions) -> Result<EngineSnapshot, EngineError> {
        if matches!(self.effect_phase, EffectPhase::Applying { .. }) || self.pending_step.is_some()
        {
            return Err(EngineError::EffectProtocol);
        }
        let atomic = self.active_atomic_indices()?;
        let transitions = match options.transitions {
            TransitionListing::Include => {
                let active = match options.previews {
                    Previews::Include => Some(self.active_configuration()?),
                    Previews::Omit => None,
                };
                self.triggerable_transitions(&atomic, active.as_ref())?
            }
            TransitionListing::Omit => Vec::new(),
        };
        self.image(
            self.state_ids(&atomic)?,
            self.configuration_ids_all()?,
            transitions,
        )
    }

    fn capture_effect(&self) -> Result<EngineSnapshot, EngineError> {
        if !matches!(self.effect_phase, EffectPhase::Applying { .. }) {
            return Err(EngineError::EffectProtocol);
        }
        let mut configuration = self.observed.to_vec();
        self.sort_document_order(&mut configuration);
        let mut has_child = vec![false; self.machine.states.len()];
        for &index in &configuration {
            if let Some(parent) = self.state_by_index(index)?.parent {
                let parent = usize::try_from(parent).map_err(|_| EngineError::BrokenInvariant)?;
                *has_child
                    .get_mut(parent)
                    .ok_or(EngineError::BrokenInvariant)? = true;
            }
        }
        let mut atomic = Vec::new();
        let mut innermost = Vec::new();
        for &index in &configuration {
            if matches!(
                self.state_by_index(index)?.kind,
                StateKind::Atomic | StateKind::Final
            ) {
                atomic.push(index);
            }
            if !has_child.get(index).copied().unwrap_or(false) {
                innermost.push(index);
            }
        }
        let transitions = self.triggerable_transitions(&innermost, None)?;
        self.image(
            self.state_ids(&atomic)?,
            self.state_ids(&configuration)?,
            transitions,
        )
    }

    /// The image around one configuration view.
    fn image(
        &self,
        atomic: Vec<StateId>,
        configuration: Vec<StateId>,
        transitions: Vec<TransitionSnapshot>,
    ) -> Result<EngineSnapshot, EngineError> {
        let mut history = Vec::with_capacity(self.history_values.len());
        for (&state, members) in &self.history_values {
            let mut members = members.clone();
            self.sort_document_order(&mut members);
            history.push((state, members));
        }
        history.sort_by_key(|(state, _)| self.document_order(*state));
        let history = history
            .into_iter()
            .map(|(state, members)| {
                Ok(HistoryValue {
                    state: self.state_id_by_index(state)?,
                    value: self.state_ids(&members)?,
                })
            })
            .collect::<Result<_, EngineError>>()?;
        let mut bound: Vec<usize> = self.bound_states.iter().copied().collect();
        self.sort_document_order(&mut bound);
        let mut invocations = self
            .active_invocations
            .iter()
            .map(|active| {
                let state = self.state_by_index(active.state)?;
                let (index, invoke) = state
                    .invokes
                    .iter()
                    .enumerate()
                    .find(|(_, invoke)| invoke.document_order == active.invoke_order)
                    .ok_or(EngineError::BrokenInvariant)?;
                Ok(InvocationSnapshot {
                    invoke_id: active.invoke_id.clone(),
                    state: self.state_id_by_index(active.state)?,
                    index: u32::try_from(index).map_err(|_| EngineError::BrokenInvariant)?,
                    autoforward: invoke.autoforward,
                    type_uri: active.type_uri.clone(),
                })
            })
            .collect::<Result<Vec<_>, EngineError>>()?;
        invocations.sort_by(|a, b| a.invoke_id.as_str().cmp(b.invoke_id.as_str()));
        Ok(EngineSnapshot {
            status: self.status.clone(),
            name: self.machine.name.clone(),
            atomic,
            configuration,
            internal: self.internal.iter().cloned().collect(),
            history,
            bound_states: self.state_ids(&bound)?,
            invocations,
            transitions,
        })
    }

    /// The actions walk of `docs/session-snapshot.md` ("Session object"):
    /// for each of `leaves` (the active atomic states, or an
    /// executable-content image's innermost states) in document order,
    /// the state and then each ancestor up to `<scxml>`, each state's
    /// transitions in document order, each transition once; eventless
    /// transitions are skipped because no event can trigger them. With
    /// `active`, each listed transition carries its `exit` and `entry`
    /// previews against that configuration.
    fn triggerable_transitions(
        &self,
        leaves: &[usize],
        active: Option<&ActiveConfiguration>,
    ) -> Result<Vec<TransitionSnapshot>, EngineError> {
        let mut seen = std::collections::BTreeSet::new();
        let mut listed = Vec::new();
        for &leaf in leaves {
            let mut chain = vec![leaf];
            chain.extend(self.proper_ancestor_indices(leaf)?);
            for state_index in chain {
                let state = self.state_by_index(state_index)?;
                for (position, &transition_index) in state.transitions.iter().enumerate() {
                    if !seen.insert(transition_index) {
                        continue;
                    }
                    let transition = self.transition(transition_index)?;
                    if transition.events.is_empty() {
                        continue;
                    }
                    listed.push(self.transition_snapshot(
                        state_index,
                        position,
                        transition_index,
                        active,
                    )?);
                }
            }
        }
        Ok(listed)
    }

    fn transition_snapshot(
        &self,
        source: usize,
        position: usize,
        transition_index: u32,
        active: Option<&ActiveConfiguration>,
    ) -> Result<TransitionSnapshot, EngineError> {
        let transition = self.transition(transition_index)?;
        let targets = transition
            .targets
            .iter()
            .map(|&target| usize::try_from(target).map_err(|_| EngineError::BrokenInvariant))
            .collect::<Result<Vec<_>, _>>()?;
        let domain = if targets.is_empty() {
            None
        } else {
            self.transition_domain(transition)?
        };
        let (exit, entry) = if let Some(active) = active
            && !targets.is_empty()
        {
            // The same exit, history-recording, and entry functions
            // `apply_microstep` runs, for this transition alone. Firing
            // records history on exit before it resolves history targets,
            // so the entry set resolves through a scratch history map that
            // holds what this exit would record; the entry plan is scratch
            // too, so no default-history content or binding is kept.
            let mut exit: Vec<usize> = self
                .transition_exit_indices(transition, active)?
                .into_iter()
                .collect();
            exit.sort_unstable_by_key(|&index| std::cmp::Reverse(self.document_order(index)));
            let recorded = self.recorded_history(&exit)?;
            let history = if recorded.is_empty() {
                Cow::Borrowed(&self.history_values)
            } else {
                let mut history = self.history_values.clone();
                history.extend(recorded);
                Cow::Owned(history)
            };
            let mut entry = self
                .compute_entry_set(&targets, domain, &history)?
                .states_to_enter;
            self.sort_document_order(&mut entry);
            (self.state_ids(&exit)?, self.state_ids(&entry)?)
        } else {
            (Vec::new(), Vec::new())
        };
        Ok(TransitionSnapshot {
            source: self.state_id_by_index(source)?,
            index: u32::try_from(position).map_err(|_| EngineError::BrokenInvariant)?,
            events: transition.events.clone(),
            targets: self.state_ids(&targets)?,
            transition_type: transition.transition_type,
            cond: transition.cond.clone(),
            domain: domain
                .map(|index| self.state_id_by_index(index))
                .transpose()?,
            exit,
            entry,
            event_schema: transition.event_schema.clone(),
        })
    }

    fn document_order(&self, index: usize) -> u32 {
        self.state_by_index(index)
            .map_or(u32::MAX, |state| state.document_order)
    }

    fn sort_document_order(&self, indices: &mut [usize]) {
        indices.sort_unstable_by_key(|&index| self.document_order(index));
    }
}
