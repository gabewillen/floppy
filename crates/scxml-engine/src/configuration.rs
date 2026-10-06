//! Per-effect active configurations as deltas.
//!
//! Every planned effect observes one active configuration through `In()`
//! (SCXML 5.9.1): exit content sees the configuration with the states exited
//! before it already removed, entry content sees every state entered before
//! it (official case 409). Copying that list into each effect costs work
//! quadratic in the configuration size for one wide microstep, so the engine
//! records each command's configuration changes once, in a
//! [`ConfigurationJournal`], and every effect carries only its
//! [`ConfigurationMark`] in it. [`crate::Engine::run`] hands the executor the
//! delta between the configuration it installed last and the one the next
//! effect observes, so the work per event is linear in the effects plus the
//! configuration size.
//!
//! A configuration is an ordered list of distinct states, here state table
//! indices. Every delta applies the same way: remove each exited state (the
//! others keep their order), then append each entered state in order.

use std::collections::BTreeMap;

use scxml_compiler_core::MAX_NESTED_ITEMS;

use crate::EngineError;

/// Bound on the configuration changes one command records: every state is
/// exited or entered at most a few times per command (exit, entry, and the
/// top-level-final and termination views), so four per possible state is
/// never reached by a valid machine.
const MAX_JOURNAL_OPS: usize = MAX_NESTED_ITEMS.saturating_mul(4);

/// Position in the configuration journal of the engine's current command:
/// the configuration one effect observes. Marks of one command only move
/// forward in plan order.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ConfigurationMark(usize);

/// The change between two configurations, as state table indices: remove
/// each `exited` state (the others keep their order), then append each
/// `entered` state in order. A state in both lists is removed and then
/// re-appended.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ConfigurationDelta {
    pub(crate) exited: Vec<usize>,
    pub(crate) entered: Vec<usize>,
}

impl ConfigurationDelta {
    /// Whether the delta changes nothing.
    pub(crate) const fn is_empty(&self) -> bool {
        self.exited.is_empty() && self.entered.is_empty()
    }
}

#[derive(Clone, Copy, Debug)]
enum ConfigurationOp {
    Exit(usize),
    Enter(usize),
}

/// One command's configurations: the configuration when the command began
/// (document order) followed by every state exit and entry the command's
/// plan makes, in plan order.
#[derive(Clone, Debug, Default)]
pub(crate) struct ConfigurationJournal {
    base: Vec<usize>,
    ops: Vec<ConfigurationOp>,
}

impl ConfigurationJournal {
    /// Starts a command whose configuration is `base`.
    pub(crate) fn reset(&mut self, base: Vec<usize>) {
        self.base = base;
        self.ops.clear();
    }

    /// The configuration after every change recorded so far.
    pub(crate) const fn mark(&self) -> ConfigurationMark {
        ConfigurationMark(self.ops.len())
    }

    /// Removes `state` from the current configuration.
    pub(crate) fn exit(&mut self, state: usize) -> Result<(), EngineError> {
        self.push(ConfigurationOp::Exit(state))
    }

    /// Appends `state` to the current configuration.
    pub(crate) fn enter(&mut self, state: usize) -> Result<(), EngineError> {
        self.push(ConfigurationOp::Enter(state))
    }

    fn push(&mut self, op: ConfigurationOp) -> Result<(), EngineError> {
        if scxml_types::exceeds_wasm_bound(self.ops.len().saturating_add(1), MAX_JOURNAL_OPS) {
            return Err(EngineError::BrokenInvariant);
        }
        self.ops.push(op);
        Ok(())
    }

    /// The configuration at `mark`, materialized in full (linear in the
    /// configuration and the recorded changes; for inspection, never per
    /// effect).
    pub(crate) fn configuration_at(
        &self,
        mark: ConfigurationMark,
    ) -> Result<Vec<usize>, EngineError> {
        let mut configuration = ObservedConfiguration::default();
        configuration.apply(&ConfigurationDelta {
            exited: Vec::new(),
            entered: self.base.clone(),
        });
        configuration.apply(&self.delta(ConfigurationMark(0), mark)?);
        Ok(configuration.to_vec())
    }

    /// The single delta from the configuration at `from` to the one at `to`
    /// (`from <= to`), linear in the changes between them.
    pub(crate) fn delta(
        &self,
        from: ConfigurationMark,
        to: ConfigurationMark,
    ) -> Result<ConfigurationDelta, EngineError> {
        let ops = self
            .ops
            .get(from.0..to.0)
            .ok_or(EngineError::BrokenInvariant)?;
        let mut exited = Vec::new();
        let mut entered: Vec<Option<usize>> = Vec::new();
        let mut pending: BTreeMap<usize, usize> = BTreeMap::new();
        for &op in ops {
            match op {
                ConfigurationOp::Exit(state) => match pending.remove(&state) {
                    // Entered within the range: the entry is undone. When it
                    // was also present before the range, its earlier exit is
                    // already listed.
                    Some(position) => {
                        let slot = entered
                            .get_mut(position)
                            .ok_or(EngineError::BrokenInvariant)?;
                        *slot = None;
                    }
                    None => exited.push(state),
                },
                ConfigurationOp::Enter(state) => {
                    pending.insert(state, entered.len());
                    entered.push(Some(state));
                }
            }
        }
        Ok(ConfigurationDelta {
            exited,
            entered: entered.into_iter().flatten().collect(),
        })
    }
}

/// An ordered configuration that applies [`ConfigurationDelta`]s in
/// amortized constant time per changed state: the mirror of what an
/// executor installed. States live in `slots` in order, with removed states
/// left as holes that compaction drops; `position` maps a state table index
/// to its slot.
#[derive(Clone, Debug, Default)]
pub(crate) struct ObservedConfiguration {
    slots: Vec<Option<usize>>,
    position: Vec<Option<usize>>,
    live: usize,
}

impl ObservedConfiguration {
    /// Applies one delta.
    pub(crate) fn apply(&mut self, delta: &ConfigurationDelta) {
        for &state in &delta.exited {
            self.remove(state);
        }
        for &state in &delta.entered {
            self.remove(state);
            if self.position.len() <= state {
                self.position.resize(state.saturating_add(1), None);
            }
            if let Some(position) = self.position.get_mut(state) {
                *position = Some(self.slots.len());
            }
            self.slots.push(Some(state));
            self.live = self.live.saturating_add(1);
        }
        if self.slots.len() > self.live.saturating_mul(2).saturating_add(16) {
            self.slots.retain(Option::is_some);
            for (slot, state) in self.slots.iter().enumerate() {
                if let Some(position) = state.and_then(|state| self.position.get_mut(state)) {
                    *position = Some(slot);
                }
            }
        }
    }

    fn remove(&mut self, state: usize) {
        if let Some(slot) = self.position.get_mut(state).and_then(Option::take)
            && let Some(entry) = self.slots.get_mut(slot)
        {
            *entry = None;
            self.live = self.live.saturating_sub(1);
        }
    }

    fn states(&self) -> impl Iterator<Item = usize> + '_ {
        self.slots.iter().flatten().copied()
    }

    /// The configuration, in order.
    pub(crate) fn to_vec(&self) -> Vec<usize> {
        self.states().collect()
    }

    /// The smallest delta of the remove-then-append form that turns this
    /// configuration into `target`: states absent from `target` and every
    /// kept state after the longest in-order prefix `target` shares are
    /// removed, and `target`'s remainder is appended. Empty, after one
    /// comparison pass, when the configuration already is `target`.
    pub(crate) fn delta_to(&self, target: &[usize]) -> ConfigurationDelta {
        if self.live == target.len() && self.states().eq(target.iter().copied()) {
            return ConfigurationDelta::default();
        }
        let size = target.iter().max().map_or(0, |max| max.saturating_add(1));
        let mut wanted = vec![false; size];
        for &state in target {
            if let Some(slot) = wanted.get_mut(state) {
                *slot = true;
            }
        }
        let mut exited = Vec::new();
        let mut shared = 0_usize;
        let mut diverged = false;
        for state in self.states() {
            if !wanted.get(state).copied().unwrap_or(false) {
                exited.push(state);
            } else if !diverged && target.get(shared) == Some(&state) {
                shared = shared.saturating_add(1);
            } else {
                diverged = true;
                exited.push(state);
            }
        }
        ConfigurationDelta {
            exited,
            entered: target.get(shared..).unwrap_or_default().to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed(states: &[usize]) -> ObservedConfiguration {
        let mut configuration = ObservedConfiguration::default();
        configuration.apply(&ConfigurationDelta {
            exited: Vec::new(),
            entered: states.to_vec(),
        });
        configuration
    }

    #[test]
    fn deltas_compose_to_the_journaled_configurations() {
        let mut journal = ConfigurationJournal::default();
        journal.reset(vec![0, 1, 2]);
        let start = journal.mark();
        journal.exit(1).expect("bounded");
        let after_exit = journal.mark();
        journal.enter(3).expect("bounded");
        journal.exit(3).expect("bounded");
        journal.exit(2).expect("bounded");
        journal.enter(2).expect("bounded");
        journal.enter(4).expect("bounded");
        let end = journal.mark();
        assert_eq!(
            journal.configuration_at(start).expect("valid mark"),
            [0, 1, 2]
        );
        assert_eq!(
            journal.configuration_at(after_exit).expect("valid mark"),
            [0, 2]
        );
        assert_eq!(
            journal.configuration_at(end).expect("valid mark"),
            [0, 2, 4]
        );
        let delta = journal.delta(after_exit, end).expect("ordered marks");
        assert_eq!(delta.exited, [2]);
        assert_eq!(delta.entered, [2, 4]);
        let mut mirror = observed(&[0, 2]);
        mirror.apply(&delta);
        assert_eq!(mirror.to_vec(), [0, 2, 4]);
        assert!(journal.delta(end, after_exit).is_err());
    }

    #[test]
    fn delta_to_reorders_with_the_shortest_suffix() {
        let current = observed(&[0, 1, 2, 5, 3]);
        let target = [0, 1, 2, 3, 5];
        let delta = current.delta_to(&target);
        assert_eq!(delta.exited, [5, 3]);
        assert_eq!(delta.entered, [3, 5]);
        let mut applied = current;
        applied.apply(&delta);
        assert_eq!(applied.to_vec(), target);
        assert!(applied.delta_to(&target).is_empty());
        let gone = applied.delta_to(&[7]);
        applied.apply(&gone);
        assert_eq!(applied.to_vec(), [7]);
    }

    #[test]
    fn compaction_keeps_order_and_membership() {
        let mut mirror = observed(&[0, 1, 2, 3]);
        let mut expected = vec![0, 1, 2, 3];
        for round in 0..100 {
            let state = round % 4;
            mirror.apply(&ConfigurationDelta {
                exited: vec![state],
                entered: vec![state],
            });
            expected.retain(|&kept| kept != state);
            expected.push(state);
            assert_eq!(mirror.to_vec(), expected);
        }
        assert!(mirror.slots.len() <= 2 * 4 + 16);
        assert!(mirror.delta_to(&expected).is_empty());
    }
}
