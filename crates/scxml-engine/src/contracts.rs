//! The event contracts a session checks when it creates an event (the
//! `event` vocabulary, `namespaces/event/README.md`, "At run time").
//!
//! A literal `event:schema` is a contract on the data of the events its
//! element creates or handles. The session checks the data of an event
//! before it creates it with an executed `<send>` (SCXML Event I/O
//! Processor) or `<raise>`:
//!
//! * the element's own literal schema, whatever the target;
//! * for an event the session delivers to itself (`<raise>`, and a `<send>`
//!   to `#_internal`, with no target, or to its own session address), the
//!   literal schema of every transition of this document whose event
//!   descriptor matches the event's name (SCXML 3.12.1), in any state.
//!
//! Data none of them rejects is admitted. `event:schemaexpr` is never
//! evaluated, so it constrains nothing. The checks are pure, deterministic,
//! and bounded by the Machine IR (its schemas are bounded at decoding) and
//! the event data (bounded at every boundary); a schema is compiled once,
//! on first use, and kept for the session. A schema that does not compile
//! (only Machine IR that no canonical compiler produced) admits nothing, so
//! the check fails closed.

use std::collections::BTreeMap;

use scxml_compiler_core::{EventSchemaIr, MachineIr, StateKind};
use scxml_event_schema::CompiledSchema;
use scxml_types::DataValue;

/// One transition's descriptors and its literal schema's digest.
struct Handler {
    descriptors: Vec<String>,
    sha256: String,
}

/// The literal schemas of one document and their compiled forms.
pub(crate) struct EventContracts {
    /// Every event transition of a non-history state that has a literal
    /// schema, in transition-table order.
    handlers: Vec<Handler>,
    /// Canonical schema text by digest: every transition's literal schema.
    /// Element schemas join on first use.
    sources: BTreeMap<String, String>,
    /// Compiled schemas by digest; `None` for one that does not compile.
    compiled: BTreeMap<String, Option<CompiledSchema>>,
}

impl EventContracts {
    /// The contracts of `machine`'s transitions. Infallible: a transition
    /// index outside the table names no transition (`Engine::new`
    /// rejects such IR before it uses this).
    pub(crate) fn new(machine: &MachineIr) -> Self {
        let mut handlers = Vec::new();
        let mut sources = BTreeMap::new();
        for state in &machine.states {
            if matches!(
                state.kind,
                StateKind::ShallowHistory | StateKind::DeepHistory
            ) {
                continue;
            }
            for transition in state.transitions.iter().filter_map(|&index| {
                usize::try_from(index)
                    .ok()
                    .and_then(|index| machine.transitions.get(index))
            }) {
                if transition.events.is_empty() {
                    continue;
                }
                if let Some(EventSchemaIr::Schema { schema, sha256 }) = &transition.event_schema {
                    sources
                        .entry(sha256.clone())
                        .or_insert_with(|| schema.clone());
                    handlers.push(Handler {
                        descriptors: transition.events.clone(),
                        sha256: sha256.clone(),
                    });
                }
            }
        }
        Self {
            handlers,
            sources,
            compiled: BTreeMap::new(),
        }
    }

    /// Whether the event `event` with `data` keeps its contracts: `own`,
    /// the creating element's schema, admits `data` (an absent or
    /// `event:schemaexpr` one admits everything), and, when `delivered_here`,
    /// so does every literal schema of a transition handling `event`.
    pub(crate) fn admits(
        &mut self,
        event: &str,
        data: &DataValue,
        own: Option<&EventSchemaIr>,
        delivered_here: bool,
    ) -> bool {
        if let Some(EventSchemaIr::Schema { schema, sha256 }) = own {
            if !self.sources.contains_key(sha256) {
                self.sources.insert(sha256.clone(), schema.clone());
            }
            if !self.check(sha256, data) {
                return false;
            }
        }
        if !delivered_here {
            return true;
        }
        let digests = self
            .handlers
            .iter()
            .filter(|handler| {
                handler
                    .descriptors
                    .iter()
                    .any(|descriptor| crate::descriptor_matches(descriptor, event))
            })
            .map(|handler| handler.sha256.clone())
            .collect::<std::collections::BTreeSet<_>>();
        digests.iter().all(|sha256| self.check(sha256, data))
    }

    /// Whether the schema with digest `sha256` admits `data`, compiling it
    /// on first use.
    fn check(&mut self, sha256: &str, data: &DataValue) -> bool {
        if !self.compiled.contains_key(sha256) {
            let compiled = self
                .sources
                .get(sha256)
                .and_then(|schema| CompiledSchema::new(schema).ok());
            self.compiled.insert(sha256.to_owned(), compiled);
        }
        self.compiled
            .get(sha256)
            .and_then(Option::as_ref)
            .is_some_and(|schema| schema.admits(data))
    }
}
