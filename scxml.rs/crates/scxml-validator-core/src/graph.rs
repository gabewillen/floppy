//! Bounded semantic machine graph and the graph-derived SCXML rules.
//!
//! # Contract
//!
//! * Precondition: the document already passed [`scxml_syntax::XmlLimits`]
//!   tree checks; each `<scxml>` root (including inline `<invoke><content>`
//!   machines) is analyzed as an independent machine.
//! * Bounds: one machine graph holds at most [`MAX_GRAPH_NODES`] state nodes,
//!   [`MAX_GRAPH_TRANSITIONS`] transitions, and [`MAX_GRAPH_TARGETS`] target
//!   references (wasm builds only), and every analysis spends from one work
//!   budget ([`graph_work_budget`], every build) that grows with the machine,
//!   so only super-linear work exhausts it natively.
//!   When a bound is reached the machine receives exactly one fail-closed
//!   `SCXML028` error and no partial graph findings.
//! * Honesty: graph findings are reported only when the static tree proves
//!   them. Reachability assumes every event can arrive and every `cond` can be
//!   true; liveness findings are reported only when no `cond` can change the
//!   selected transition.
//! * Determinism: the arena, every traversal, and every finding depend only on
//!   the parsed tree; the caller sorts the report.
//! * Side effects: none. The graph runs inside the validator's boundary span
//!   and records no telemetry of its own.

use std::collections::BTreeMap;

use scxml_syntax::{Attribute, Document, Element, Span, xml_tokens};
use scxml_types::wasm_bound;

use crate::semantic::{
    Rule, attribute_token_spans, child_elements, has_unknown_scxml_child, is_scxml, machine_roots,
    valid_event_descriptor, visit_machine,
};
use crate::{Applicability, Diagnostic, DiagnosticCategory, Fix, FixEdit, Label, Report};

/// Maximum `<state>`, `<parallel>`, `<final>`, `<history>`, and `<initial>`
/// nodes (plus the root) in one machine graph (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_GRAPH_NODES: usize = wasm_bound(65_536);
/// Maximum transitions in one machine graph (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_GRAPH_TRANSITIONS: usize = wasm_bound(65_536);
/// Maximum target and `initial` identifier references in one machine graph
/// (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_GRAPH_TARGETS: usize = wasm_bound(131_072);
/// Abstract work units all graph analyses of one machine may spend whatever
/// its size (every build; see [`graph_work_budget`]).
pub const MAX_GRAPH_WORK: usize = 8_000_000;
/// Work units granted per element and per `target` or `initial` reference of
/// a machine (every build). Each linear analysis spends at most a few units
/// per element and XML nesting level (256 at most), so only work that grows
/// faster than the machine, such as the pairwise checks of one state's
/// transitions or one target set, exhausts the budget.
pub const GRAPH_WORK_PER_ITEM: usize = 1_024;
/// Most work units the grant per item adds up to (wasm builds only;
/// unbounded natively, see [`scxml_types::wasm_bound`]).
const MAX_SCALED_GRAPH_WORK: usize = wasm_bound(MAX_GRAPH_WORK);

/// The work budget of a machine of `items` elements and references (every
/// build): [`MAX_GRAPH_WORK`], or [`GRAPH_WORK_PER_ITEM`] per item when that
/// is more (at most [`MAX_GRAPH_WORK`] on wasm builds).
#[must_use]
pub fn graph_work_budget(items: usize) -> usize {
    let scaled = items.saturating_mul(GRAPH_WORK_PER_ITEM);
    if scxml_types::exceeds_wasm_bound(scaled, MAX_SCALED_GRAPH_WORK) {
        MAX_SCALED_GRAPH_WORK
    } else {
        MAX_GRAPH_WORK.max(scaled)
    }
}

/// The elements of `machine` (nested machines excluded) and the tokens of
/// their `target` and `initial` attributes.
fn machine_items(machine: &Element) -> usize {
    let mut items = 0_usize;
    visit_machine(machine, &mut |element| {
        items = items.saturating_add(1);
        for name in ["target", "initial"] {
            if let Some(attribute) = element.attribute(name) {
                items = items.saturating_add(attribute.value.split_ascii_whitespace().count());
            }
        }
    });
    items
}

const SPEC_CONFIGURATION: &str = "SCXML 1.0 §3.11 Legal State Configurations and Specifications";
const SPEC_SELECTION: &str = "SCXML 1.0 §3.13 Selecting and Executing Transitions";
const SPEC_TRANSITION: &str = "SCXML 1.0 §3.5 <transition>";
const SPEC_FINAL: &str = "SCXML 1.0 §3.7 <final>";
const SPEC_PARALLEL: &str = "SCXML 1.0 §3.4 <parallel>";
const SPEC_DONEDATA: &str = "SCXML 1.0 §5.5 <donedata>";

/// Runs every graph-derived rule for each machine root.
pub(crate) struct GraphRule;

impl Rule for GraphRule {
    fn code(&self) -> &'static str {
        "SCXML028"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        for machine in machine_roots(&document.root) {
            check_final_content(machine, document, report);
            check_donedata_placement(machine, document, report);
            let work = graph_work_budget(machine_items(machine));
            let mut budget = Budget::new(work);
            let mut findings = Report::new();
            if analyze(document, machine, &mut budget, &mut findings).is_ok() {
                report.extend(findings);
            } else {
                report.push(budget_diagnostic(self.code(), machine));
            }
        }
    }
}

fn budget_diagnostic(code: &str, machine: &Element) -> Diagnostic {
    Diagnostic::error(
        code,
        "state machine is too large for bounded semantic graph analysis",
        machine.start_tag_span,
    )
    .with_primary_message("graph analysis budget exceeded in this machine")
    .with_note(if cfg!(target_arch = "wasm32") {
        format!(
            "the validator analyzes at most {MAX_GRAPH_NODES} states, {MAX_GRAPH_TRANSITIONS} transitions, {MAX_GRAPH_TARGETS} target references, and {MAX_GRAPH_WORK} work units per machine; reachability, target legality, and liveness were not checked"
        )
    } else {
        format!(
            "the validator spends at most {MAX_GRAPH_WORK} work units per machine, or {GRAPH_WORK_PER_ITEM} per element and target reference when that is more, so work that grows faster than the machine is cut off; reachability, target legality, and liveness were not checked"
        )
    })
    .with_help("split the machine into smaller machines composed with `<invoke>`, or reduce the number of states and transitions")
    .with_spec_reference(SPEC_CONFIGURATION)
}

// ---------------------------------------------------------------------------
// Budget
// ---------------------------------------------------------------------------

/// Lazily computed baseline reachability and the separate work budget used to
/// check that a suggested target removal strands no state. Exhausting it only
/// withholds fixes; it never fails the graph analysis.
#[derive(Debug)]
struct RemovalSafety {
    budget: Budget,
    baseline: Option<Result<Vec<bool>, Exhausted>>,
}

impl RemovalSafety {
    const fn new(units: usize) -> Self {
        Self {
            budget: Budget::new(units),
            baseline: None,
        }
    }
}

/// Marker for an exhausted analysis budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Exhausted;

#[derive(Debug)]
struct Budget {
    remaining: usize,
}

impl Budget {
    const fn new(units: usize) -> Self {
        Self { remaining: units }
    }

    fn spend(&mut self, units: usize) -> Result<(), Exhausted> {
        if let Some(remaining) = self.remaining.checked_sub(units) {
            self.remaining = remaining;
            Ok(())
        } else {
            self.remaining = 0;
            Err(Exhausted)
        }
    }
}

// ---------------------------------------------------------------------------
// Graph model
// ---------------------------------------------------------------------------

/// The first reference (by span) to each node, keyed by node index.
type References = BTreeMap<usize, Span>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Root,
    State,
    Parallel,
    Final,
    History,
    Initial,
}

#[derive(Debug)]
struct Node<'a> {
    kind: Kind,
    element: &'a Element,
    parent: Option<usize>,
    /// `<state>`, `<parallel>`, and `<final>` children in document order.
    children: Vec<usize>,
    /// First `<initial>` pseudo-state child.
    initial: Option<usize>,
    /// Transitions owned by this node in document order.
    transitions: Vec<usize>,
    /// Resolved `initial` attribute tokens (root or `<state>` only).
    initial_attribute: Option<Vec<Target>>,
}

impl<'a> Node<'a> {
    const fn new(kind: Kind, element: &'a Element, parent: Option<usize>) -> Self {
        Self {
            kind,
            element,
            parent,
            children: Vec::new(),
            initial: None,
            transitions: Vec::new(),
            initial_attribute: None,
        }
    }

    fn id(&self) -> Option<&'a Attribute> {
        match self.kind {
            Kind::State | Kind::Parallel | Kind::Final | Kind::History => {
                self.element.attribute("id")
            }
            Kind::Root | Kind::Initial => None,
        }
    }

    /// Best span for pointing at this node: its `id` value or its start tag.
    fn anchor(&self) -> Span {
        self.id()
            .map_or(self.element.start_tag_span, |id| id.value_span)
    }

    fn name(&self) -> String {
        match (self.kind, self.id()) {
            (Kind::Root, _) => "the <scxml> root".to_owned(),
            (_, Some(id)) => format!("`{}`", id.value),
            (Kind::Parallel, None) => "an anonymous <parallel>".to_owned(),
            (Kind::Final, None) => "an anonymous <final>".to_owned(),
            (Kind::History, None) => "an anonymous <history>".to_owned(),
            (Kind::Initial, None) => "an <initial> pseudo-state".to_owned(),
            (Kind::State, None) => "an anonymous <state>".to_owned(),
        }
    }

    const fn is_state_like(&self) -> bool {
        matches!(self.kind, Kind::State | Kind::Parallel | Kind::Final)
    }

    fn is_atomic(&self) -> bool {
        match self.kind {
            Kind::Final => true,
            Kind::State => self.children.is_empty(),
            Kind::Root | Kind::Parallel | Kind::History | Kind::Initial => false,
        }
    }
}

#[derive(Clone, Debug)]
struct Target {
    token: String,
    span: Span,
    node: Option<usize>,
}

#[derive(Debug)]
struct Transition<'a> {
    owner: usize,
    element: &'a Element,
    /// `None` for an eventless transition; the raw descriptor tokens otherwise.
    events: Option<Vec<String>>,
    conditional: bool,
    target: Option<&'a Attribute>,
    targets: Vec<Target>,
}

impl<'a> Transition<'a> {
    fn new(owner: usize, element: &'a Element) -> Self {
        Self {
            owner,
            element,
            events: element
                .attribute("event")
                .map(|event| xml_tokens(&event.value).map(str::to_owned).collect()),
            conditional: element.attribute("cond").is_some(),
            target: element.attribute("target"),
            targets: Vec::new(),
        }
    }

    /// The single resolved target, when the transition has exactly one.
    fn single_target(&self) -> Option<(&Target, usize)> {
        match self.targets.as_slice() {
            [target] => target.node.map(|node| (target, node)),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct Graph<'a> {
    nodes: Vec<Node<'a>>,
    transitions: Vec<Transition<'a>>,
    /// Default entry of every node, computed once after construction.
    defaults: Vec<Entry>,
    /// Per transition: an earlier transition of the same state shadows it
    /// (`SCXML034`), so it can never be selected. Reachability does not
    /// follow it, as the `SCXML026` cycle check does not, so removing it (the
    /// `SCXML034` fix) never reveals a new `SCXML029`.
    shadowed: Vec<bool>,
}

/// Default entry targets of one node.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Entry {
    /// The node enters exactly these nodes by default (empty when atomic).
    Targets(Vec<usize>),
    /// The default entry is malformed or unresolved; other rules report it.
    Unknown,
}

/// Resolves every `target` and `initial` identifier reference.
fn resolve_references(
    document: &Document,
    ids: &BTreeMap<String, usize>,
    nodes: &mut [Node<'_>],
    transitions: &mut [Transition<'_>],
    budget: &mut Budget,
) -> Result<(), Exhausted> {
    let mut references = 0_usize;
    let mut resolve =
        |attribute: &Attribute, budget: &mut Budget| -> Result<Vec<Target>, Exhausted> {
            let mut targets = Vec::new();
            for (token, span) in attribute_token_spans(document, attribute) {
                budget.spend(1)?;
                references = references.saturating_add(1);
                if scxml_types::exceeds_wasm_bound(references, MAX_GRAPH_TARGETS) {
                    return Err(Exhausted);
                }
                let node = ids.get(&token).copied();
                targets.push(Target { token, span, node });
            }
            Ok(targets)
        };
    for transition in transitions.iter_mut() {
        if let Some(attribute) = transition.target {
            transition.targets = resolve(attribute, budget)?;
        }
    }
    for node in nodes.iter_mut() {
        if matches!(node.kind, Kind::Root | Kind::State)
            && let Some(attribute) = node.element.attribute("initial")
        {
            node.initial_attribute = Some(resolve(attribute, budget)?);
        }
    }
    Ok(())
}

fn analyze(
    document: &Document,
    machine: &Element,
    budget: &mut Budget,
    report: &mut Report,
) -> Result<(), Exhausted> {
    let graph = Graph::build(document, machine, budget)?;
    // A misspelled SCXML element (reported by the schema) usually hides the
    // transition that would reach a state; reachability over the broken
    // structure would only cascade from that one mistake.
    let mut misspelled = false;
    visit_machine(machine, &mut |element| {
        misspelled |= has_unknown_scxml_child(element);
    });
    // An unresolved `target` or `initial` token (SCXML005/SCXML006) is
    // usually a typo for a declared state; that state would otherwise be
    // reported as unreachable although the one typo is the root cause.
    if !misspelled && !graph.has_unresolved_reference() {
        graph.check_unreachable(budget, report)?;
    }
    graph.check_target_orthogonality(document, budget, report)?;
    graph.check_single_region_parallel(document, report);
    graph.check_shadowed_transitions(document, budget, report)?;
    graph.check_eventless_liveness(budget, report)?;
    Ok(())
}

impl<'a> Graph<'a> {
    /// Whether any `target` or `initial` token names no declared state.
    fn has_unresolved_reference(&self) -> bool {
        let unresolved = |targets: &[Target]| targets.iter().any(|target| target.node.is_none());
        self.transitions
            .iter()
            .any(|transition| unresolved(&transition.targets))
            || self
                .nodes
                .iter()
                .any(|node| node.initial_attribute.as_deref().is_some_and(unresolved))
    }

    fn build(
        document: &Document,
        machine: &'a Element,
        budget: &mut Budget,
    ) -> Result<Self, Exhausted> {
        let mut nodes = vec![Node::new(Kind::Root, machine, None)];
        let mut transitions = Vec::new();
        let mut ids: BTreeMap<String, usize> = BTreeMap::new();
        let mut pending = vec![0_usize];
        while let Some(index) = pending.pop() {
            let Some((element, kind)) = nodes.get(index).map(|node| (node.element, node.kind))
            else {
                continue;
            };
            let hosts_states = matches!(kind, Kind::Root | Kind::State | Kind::Parallel);
            let mut created = Vec::new();
            for child in child_elements(element) {
                budget.spend(1)?;
                if is_scxml(child, "transition") {
                    if scxml_types::exceeds_wasm_bound(
                        transitions.len().saturating_add(1),
                        MAX_GRAPH_TRANSITIONS,
                    ) {
                        return Err(Exhausted);
                    }
                    let transition = transitions.len();
                    transitions.push(Transition::new(index, child));
                    if let Some(node) = nodes.get_mut(index) {
                        node.transitions.push(transition);
                    }
                    continue;
                }
                if !hosts_states {
                    continue;
                }
                let child_kind = if is_scxml(child, "state") {
                    Kind::State
                } else if is_scxml(child, "parallel") {
                    Kind::Parallel
                } else if is_scxml(child, "final") {
                    Kind::Final
                } else if is_scxml(child, "history") {
                    Kind::History
                } else if is_scxml(child, "initial") {
                    Kind::Initial
                } else {
                    continue;
                };
                if scxml_types::exceeds_wasm_bound(nodes.len().saturating_add(1), MAX_GRAPH_NODES) {
                    return Err(Exhausted);
                }
                let created_index = nodes.len();
                let node = Node::new(child_kind, child, Some(index));
                if let Some(id) = node.id() {
                    ids.entry(id.value.clone()).or_insert(created_index);
                }
                nodes.push(node);
                if let Some(parent) = nodes.get_mut(index) {
                    match child_kind {
                        Kind::State | Kind::Parallel | Kind::Final => {
                            parent.children.push(created_index);
                        }
                        Kind::Initial if parent.initial.is_none() => {
                            parent.initial = Some(created_index);
                        }
                        Kind::Initial | Kind::History | Kind::Root => {}
                    }
                }
                created.push(created_index);
            }
            pending.extend(created.into_iter().rev());
        }

        resolve_references(document, &ids, &mut nodes, &mut transitions, budget)?;
        let mut graph = Self {
            nodes,
            transitions,
            defaults: Vec::new(),
            shadowed: Vec::new(),
        };
        let mut defaults = Vec::with_capacity(graph.nodes.len());
        for index in 0..graph.nodes.len() {
            defaults.push(graph.compute_default(index, budget)?);
        }
        graph.defaults = defaults;
        graph.shadowed = graph.compute_shadowed(budget)?;
        Ok(graph)
    }

    /// Marks every transition that an earlier transition of the same
    /// `<state>`, `<parallel>`, or `<final>` shadows (see [`shadows`]).
    fn compute_shadowed(&self, budget: &mut Budget) -> Result<Vec<bool>, Exhausted> {
        let mut shadowed = vec![false; self.transitions.len()];
        for node in &self.nodes {
            if !matches!(node.kind, Kind::State | Kind::Parallel | Kind::Final) {
                continue;
            }
            for (position, later_index) in node.transitions.iter().enumerate() {
                let Some(later) = self.transitions.get(*later_index) else {
                    continue;
                };
                for earlier_index in node.transitions.iter().take(position) {
                    budget.spend(1)?;
                    if self
                        .transitions
                        .get(*earlier_index)
                        .is_some_and(|earlier| shadows(earlier, later))
                    {
                        if let Some(flag) = shadowed.get_mut(*later_index) {
                            *flag = true;
                        }
                        break;
                    }
                }
            }
        }
        Ok(shadowed)
    }

    fn node(&self, index: usize) -> Option<&Node<'a>> {
        self.nodes.get(index)
    }

    fn kind(&self, index: usize) -> Option<Kind> {
        self.nodes.get(index).map(|node| node.kind)
    }

    fn parent(&self, index: usize) -> Option<usize> {
        self.nodes.get(index).and_then(|node| node.parent)
    }

    fn name(&self, index: usize) -> String {
        self.node(index)
            .map_or_else(|| "an unknown state".to_owned(), Node::name)
    }

    /// Whether `ancestor` is a proper ancestor of `index`.
    fn is_proper_ancestor(
        &self,
        ancestor: usize,
        index: usize,
        budget: &mut Budget,
    ) -> Result<bool, Exhausted> {
        let mut current = self.parent(index);
        while let Some(node) = current {
            budget.spend(1)?;
            if node == ancestor {
                return Ok(true);
            }
            current = self.parent(node);
        }
        Ok(false)
    }

    /// Whether a proper ancestor of `index` is a `<parallel>`.
    fn has_parallel_ancestor(&self, index: usize, budget: &mut Budget) -> Result<bool, Exhausted> {
        let mut current = self.parent(index);
        while let Some(node) = current {
            budget.spend(1)?;
            if self.kind(node) == Some(Kind::Parallel) {
                return Ok(true);
            }
            current = self.parent(node);
        }
        Ok(false)
    }

    /// Lowest common ancestor-or-self of two nodes.
    fn lowest_common_ancestor(
        &self,
        left: usize,
        right: usize,
        budget: &mut Budget,
    ) -> Result<Option<usize>, Exhausted> {
        let mut chain = Vec::new();
        let mut current = Some(left);
        while let Some(node) = current {
            budget.spend(1)?;
            chain.push(node);
            current = self.parent(node);
        }
        let mut current = Some(right);
        while let Some(node) = current {
            budget.spend(chain.len().max(1))?;
            if chain.contains(&node) {
                return Ok(Some(node));
            }
            current = self.parent(node);
        }
        Ok(None)
    }

    fn compute_default(&self, index: usize, budget: &mut Budget) -> Result<Entry, Exhausted> {
        let Some(node) = self.node(index) else {
            return Ok(Entry::Unknown);
        };
        match node.kind {
            Kind::Parallel => Ok(Entry::Targets(node.children.clone())),
            Kind::Root | Kind::State if !node.children.is_empty() => {
                let targets: Option<Vec<Target>> = if let Some(targets) = &node.initial_attribute {
                    Some(targets.clone())
                } else if let Some(initial) = node.initial {
                    let transitions = self
                        .node(initial)
                        .map(|pseudo| pseudo.transitions.as_slice())
                        .unwrap_or_default();
                    match transitions {
                        [only] => self
                            .transitions
                            .get(*only)
                            .map(|transition| transition.targets.clone()),
                        _ => None,
                    }
                } else {
                    return Ok(Entry::Targets(
                        node.children.first().copied().into_iter().collect(),
                    ));
                };
                let Some(targets) = targets else {
                    return Ok(Entry::Unknown);
                };
                if targets.is_empty() {
                    return Ok(Entry::Unknown);
                }
                let mut resolved = Vec::with_capacity(targets.len());
                for target in targets {
                    let Some(target_node) = target.node else {
                        return Ok(Entry::Unknown);
                    };
                    if !self.is_proper_ancestor(index, target_node, budget)? {
                        return Ok(Entry::Unknown);
                    }
                    resolved.push(target_node);
                }
                Ok(Entry::Targets(resolved))
            }
            Kind::Root | Kind::State | Kind::Final | Kind::History | Kind::Initial => {
                Ok(Entry::Targets(Vec::new()))
            }
        }
    }

    /// Default-entry declaration label for a compound node.
    fn default_label(&self, index: usize) -> Option<Label> {
        let node = self.node(index)?;
        if matches!(node.kind, Kind::Root | Kind::State)
            && let Some(attribute) = node.element.attribute("initial")
        {
            return Some(Label::new(attribute.value_span).with_message(format!(
                "{} enters `{}` by default",
                node.name(),
                attribute.value.trim_matches(scxml_syntax::is_xml_space)
            )));
        }
        if let Some(initial) = node.initial.and_then(|initial| self.node(initial)) {
            return Some(
                Label::new(initial.element.start_tag_span).with_message(format!(
                    "{} enters its `<initial>` target by default",
                    node.name()
                )),
            );
        }
        let first = node.children.first().and_then(|first| self.node(*first))?;
        Some(Label::new(first.anchor()).with_message(format!(
            "{} enters its first child state by default",
            node.name()
        )))
    }

    // -----------------------------------------------------------------------
    // SCXML029: unreachable states
    // -----------------------------------------------------------------------

    /// Over-approximates every state that can ever be active.
    ///
    /// `excluded` names one target token (by its span) whose removal is being
    /// evaluated: that reference neither enters its state nor contributes to
    /// its owner's default entry.
    fn reachable(
        &self,
        excluded: Option<Span>,
        budget: &mut Budget,
    ) -> Result<Vec<bool>, Exhausted> {
        let count = self.nodes.len();
        let mut reachable = vec![false; count];
        let mut entered = vec![false; count];
        let mut pending = vec![0_usize];
        loop {
            self.drain_entries(&mut pending, &mut reachable, &mut entered, excluded, budget)?;
            // A shallow history restores the parent's previously active child
            // and enters it with that child's default entry. Any reachable
            // child of an entered shallow history's parent may therefore be
            // entered by default; iterate to a fixpoint.
            for (index, node) in self.nodes.iter().enumerate() {
                budget.spend(1)?;
                if node.kind != Kind::History
                    || !entered.get(index).copied().unwrap_or(false)
                    || node
                        .element
                        .attribute("type")
                        .is_some_and(|kind| kind.value == "deep")
                {
                    continue;
                }
                let Some(parent) = node.parent.and_then(|parent| self.node(parent)) else {
                    continue;
                };
                pending.extend(parent.children.iter().copied().filter(|child| {
                    reachable.get(*child).copied().unwrap_or(false)
                        && !entered.get(*child).copied().unwrap_or(true)
                }));
            }
            if pending.is_empty() {
                return Ok(reachable);
            }
        }
    }

    /// Enters every pending node with its ancestors, parallel siblings, and
    /// default descendants, and follows the transitions of every newly
    /// reachable node.
    fn drain_entries(
        &self,
        pending: &mut Vec<usize>,
        reachable: &mut [bool],
        entered: &mut [bool],
        excluded: Option<Span>,
        budget: &mut Budget,
    ) -> Result<(), Exhausted> {
        let kept = |target: &Target| (Some(target.span) != excluded).then_some(target.node)?;
        while let Some(index) = pending.pop() {
            budget.spend(1)?;
            let Some(slot) = entered.get_mut(index) else {
                continue;
            };
            if *slot {
                continue;
            }
            *slot = true;
            // Entering a node activates every proper ancestor; a `<parallel>`
            // ancestor also enters every other region by default.
            let mut below = index;
            let mut current = Some(index);
            while let Some(node_index) = current {
                budget.spend(1)?;
                let Some(node) = self.node(node_index) else {
                    break;
                };
                if node.kind == Kind::Parallel && node_index != index {
                    pending.extend(
                        node.children
                            .iter()
                            .copied()
                            .filter(|child| *child != below),
                    );
                }
                if let Some(flag) = reachable.get_mut(node_index)
                    && !*flag
                {
                    *flag = true;
                    for transition in &node.transitions {
                        if self.shadowed.get(*transition).copied().unwrap_or(false) {
                            continue;
                        }
                        if let Some(transition) = self.transitions.get(*transition) {
                            budget.spend(transition.targets.len().max(1))?;
                            pending.extend(transition.targets.iter().filter_map(kept));
                        }
                    }
                }
                below = node_index;
                current = node.parent;
            }
            let Some(node) = self.node(index) else {
                continue;
            };
            match node.kind {
                Kind::History => {
                    // Restoration re-enters previously active (already
                    // reachable) states; an unvisited history takes its
                    // default transition, or its parent's default entry.
                    let default = node
                        .transitions
                        .first()
                        .and_then(|transition| self.transitions.get(*transition))
                        .filter(|transition| {
                            !transition.targets.is_empty()
                                && transition
                                    .targets
                                    .iter()
                                    .all(|target| target.node.is_some())
                        });
                    if let Some(default) = default {
                        pending.extend(default.targets.iter().filter_map(kept));
                    } else if let Some(parent) = node.parent {
                        self.push_default(parent, excluded, pending);
                    }
                }
                Kind::Root | Kind::State | Kind::Parallel => {
                    self.push_default(index, excluded, pending);
                }
                Kind::Final | Kind::Initial => {}
            }
        }
        Ok(())
    }

    fn push_default(&self, index: usize, excluded: Option<Span>, pending: &mut Vec<usize>) {
        if let Some(span) = excluded
            && let Some(targets) = self.default_source(index)
            && targets.iter().any(|target| target.span == span)
        {
            // The evaluated removal edits this node's default entry: enter
            // the remaining tokens of the same `initial` declaration.
            pending.extend(
                targets
                    .iter()
                    .filter(|target| target.span != span)
                    .filter_map(|target| target.node),
            );
            return;
        }
        match self.defaults.get(index) {
            Some(Entry::Targets(targets)) => pending.extend(targets.iter().copied()),
            // A malformed default is reported elsewhere; over-approximate so no
            // child is falsely reported unreachable.
            Some(Entry::Unknown) | None => {
                if let Some(node) = self.node(index) {
                    pending.extend(node.children.iter().copied());
                }
            }
        }
    }

    /// The target tokens that declare a compound node's default entry: its
    /// `initial` attribute, or the only transition of its `<initial>` child.
    fn default_source(&self, index: usize) -> Option<&[Target]> {
        let node = self.node(index)?;
        if let Some(targets) = &node.initial_attribute {
            return Some(targets);
        }
        let pseudo = self.node(node.initial?)?;
        match pseudo.transitions.as_slice() {
            [only] => self
                .transitions
                .get(*only)
                .map(|transition| transition.targets.as_slice()),
            _ => None,
        }
    }

    /// The first reference to each node from transitions that never run:
    /// those of unreachable states, and (second map) those of an `<initial>`
    /// its reachable parent ignores (a second `<initial>`, `SCXML064`, or one
    /// beside an `initial` attribute, `SCXML011`), whose source state is
    /// reachable.
    fn unrun_references(
        &self,
        reachable: &[bool],
        budget: &mut Budget,
    ) -> Result<(References, References), Exhausted> {
        let mut first_unreachable_reference = References::new();
        let mut first_ignored_initial_reference = References::new();
        for transition in &self.transitions {
            if reachable.get(transition.owner).copied().unwrap_or(false) {
                continue;
            }
            let references = match self.node(transition.owner) {
                Some(owner)
                    if owner.kind == Kind::Initial
                        && owner.parent.is_some_and(|parent| {
                            reachable.get(parent).copied().unwrap_or(false)
                        }) =>
                {
                    &mut first_ignored_initial_reference
                }
                _ => &mut first_unreachable_reference,
            };
            for target in &transition.targets {
                budget.spend(1)?;
                if let Some(node) = target.node {
                    references
                        .entry(node)
                        .and_modify(|span| *span = (*span).min(target.span))
                        .or_insert(target.span);
                }
            }
        }
        Ok((first_unreachable_reference, first_ignored_initial_reference))
    }

    fn check_unreachable(&self, budget: &mut Budget, report: &mut Report) -> Result<(), Exhausted> {
        let reachable = self.reachable(None, budget)?;
        // A duplicated ID resolves to its first declaration only; SCXML002
        // reports the duplicate, so later declarations are not also reported
        // as unreachable.
        let mut declarations: BTreeMap<&str, usize> = BTreeMap::new();
        for node in &self.nodes {
            if let Some(id) = node.id() {
                *declarations.entry(id.value.as_str()).or_default() += 1;
            }
        }
        let (first_unreachable_reference, first_ignored_initial_reference) =
            self.unrun_references(&reachable, budget)?;
        for (index, node) in self.nodes.iter().enumerate() {
            budget.spend(1)?;
            if !node.is_state_like() || reachable.get(index).copied().unwrap_or(true) {
                continue;
            }
            if node
                .id()
                .is_some_and(|id| declarations.get(id.value.as_str()).copied().unwrap_or(0) > 1)
            {
                continue;
            }
            let Some(parent) = node.parent else {
                continue;
            };
            if !reachable.get(parent).copied().unwrap_or(false) {
                continue;
            }
            let descendants = self.descendant_count(index, budget)?;
            let mut diagnostic = Diagnostic::warning(
                "SCXML029",
                format!(
                    "{} {} can never become active",
                    kind_noun(node.kind),
                    node.name()
                ),
                node.anchor(),
            )
            .with_primary_message("unreachable from the initial configuration")
            .with_note("reachability assumes every event can arrive and every `cond` can be true; no reachable transition, `initial`, or history default enters this state or any of its descendants")
            .with_help(format!(
                "target {} from a transition of a reachable state, make it the initial state of {}, or remove it",
                node.name(),
                self.name(parent)
            ))
            .with_spec_reference(SPEC_CONFIGURATION)
            .with_spec_reference(SPEC_SELECTION);
            if let Some(label) = self.default_label(parent) {
                diagnostic = diagnostic.with_secondary(label);
            }
            if let Some(reference) =
                self.first_reference_within(index, &first_unreachable_reference, budget)?
            {
                diagnostic = diagnostic.with_secondary(
                    Label::new(reference)
                        .with_message("targeted here, but the source state is itself unreachable"),
                );
            } else if let Some(reference) =
                self.first_reference_within(index, &first_ignored_initial_reference, budget)?
            {
                diagnostic =
                    diagnostic.with_secondary(Label::new(reference).with_message(format!(
                        "targeted here, but {} ignores this `<initial>`, so it never runs",
                        self.name(parent)
                    )));
            }
            if descendants > 0 {
                diagnostic = diagnostic.with_note(if descendants == 1 {
                    "its descendant state is unreachable as well and is not reported separately"
                        .to_owned()
                } else {
                    format!(
                        "its {descendants} descendant states are unreachable as well and are not reported separately"
                    )
                });
            }
            report.push(diagnostic);
        }
        Ok(())
    }

    fn descendant_count(&self, index: usize, budget: &mut Budget) -> Result<usize, Exhausted> {
        let mut count = 0_usize;
        let mut pending: Vec<usize> = self
            .node(index)
            .map(|node| node.children.clone())
            .unwrap_or_default();
        while let Some(child) = pending.pop() {
            budget.spend(1)?;
            count = count.saturating_add(1);
            if let Some(node) = self.node(child) {
                pending.extend(node.children.iter().copied());
            }
        }
        Ok(count)
    }

    /// First reference (by span) to `index` or one of its descendants.
    fn first_reference_within(
        &self,
        index: usize,
        references: &BTreeMap<usize, Span>,
        budget: &mut Budget,
    ) -> Result<Option<Span>, Exhausted> {
        let mut best: Option<Span> = None;
        for (node, span) in references {
            budget.spend(1)?;
            if *node == index || self.is_proper_ancestor(index, *node, budget)? {
                best = Some(best.map_or(*span, |current| current.min(*span)));
            }
        }
        Ok(best)
    }

    // -----------------------------------------------------------------------
    // SCXML030: non-orthogonal target sets
    // -----------------------------------------------------------------------

    fn check_target_orthogonality(
        &self,
        document: &Document,
        budget: &mut Budget,
        report: &mut Report,
    ) -> Result<(), Exhausted> {
        let references: usize = self
            .transitions
            .iter()
            .map(|transition| transition.targets.len())
            .chain(
                self.nodes
                    .iter()
                    .map(|node| node.initial_attribute.as_ref().map_or(0, Vec::len)),
            )
            .fold(0, usize::saturating_add);
        let mut safety = RemovalSafety::new(graph_work_budget(
            self.nodes
                .len()
                .saturating_add(self.transitions.len())
                .saturating_add(references),
        ));
        for transition in &self.transitions {
            if transition.targets.len() > 1 {
                self.check_target_set(
                    document,
                    &transition.targets,
                    "transition target",
                    budget,
                    &mut safety,
                    report,
                )?;
            }
        }
        for node in &self.nodes {
            if let Some(targets) = &node.initial_attribute
                && targets.len() > 1
            {
                self.check_target_set(
                    document,
                    targets,
                    "initial state",
                    budget,
                    &mut safety,
                    report,
                )?;
            }
        }
        Ok(())
    }

    /// Evaluates removing the target token at `span`: `Ok(())` when every
    /// currently reachable state stays reachable, otherwise the topmost state
    /// the removal would strand (a new `SCXML029`). `Err(())` when the
    /// separate fix-safety budget is exhausted, so no removal is offered.
    fn removal_strands(&self, span: Span, safety: &mut RemovalSafety) -> Result<Option<usize>, ()> {
        let baseline = match &safety.baseline {
            Some(Ok(baseline)) => baseline.clone(),
            Some(Err(Exhausted)) => return Err(()),
            None => {
                let computed = self.reachable(None, &mut safety.budget);
                safety.baseline = Some(computed.clone());
                computed.map_err(|Exhausted| ())?
            }
        };
        let after = self
            .reachable(Some(span), &mut safety.budget)
            .map_err(|Exhausted| ())?;
        let stranded = |index: usize| {
            baseline.get(index).copied().unwrap_or(false)
                && !after.get(index).copied().unwrap_or(false)
        };
        Ok((0..self.nodes.len()).find(|index| {
            stranded(*index)
                && self
                    .node(*index)
                    .and_then(|node| node.parent)
                    .is_none_or(|parent| !stranded(parent))
        }))
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the rule threads the analysis and fix-safety budgets explicitly"
    )]
    fn check_target_set(
        &self,
        document: &Document,
        targets: &[Target],
        noun: &str,
        budget: &mut Budget,
        safety: &mut RemovalSafety,
        report: &mut Report,
    ) -> Result<(), Exhausted> {
        for (later_index, later) in targets.iter().enumerate().skip(1) {
            let Some(later_node) = later.node else {
                continue;
            };
            for earlier in targets.iter().take(later_index) {
                budget.spend(1)?;
                let Some(earlier_node) = earlier.node else {
                    continue;
                };
                let Some(conflict) =
                    self.orthogonality_conflict(earlier_node, later_node, budget)?
                else {
                    if let Some(diagnostic) = self.history_overlap(noun, earlier, later, budget)? {
                        report.push(diagnostic);
                    }
                    continue;
                };
                let Some(mut diagnostic) = self.conflict_diagnostic(noun, conflict, earlier, later)
                else {
                    report.push(duplicate_target_diagnostic(
                        document, noun, earlier, later, targets,
                    ));
                    break;
                };
                // Offer a removal only when it strands no state: a removal
                // that leaves a state unreachable would trade this error for
                // a new SCXML029.
                let first = targets.first().map(|first| first.span);
                let mut offered = 0_usize;
                for (removed, kept) in [(later, earlier), (earlier, later)] {
                    let Some(edit) = token_removal_edit(document, removed.span, first) else {
                        continue;
                    };
                    match self.removal_strands(removed.span, safety) {
                        Ok(None) => {
                            offered += 1;
                            diagnostic = diagnostic.with_fix(Fix {
                                message: format!(
                                    "remove the target `{}`, keeping `{}`",
                                    removed.token, kept.token
                                ),
                                applicability: Applicability::MaybeIncorrect,
                                edits: vec![edit],
                            });
                        }
                        Ok(Some(stranded)) => {
                            diagnostic = diagnostic.with_note(format!(
                                "removing `{}` (keeping `{}`) would leave {} unreachable: no other transition, `initial`, or history default enters it",
                                removed.token,
                                kept.token,
                                self.name(stranded)
                            ));
                        }
                        Err(()) => {}
                    }
                }
                diagnostic = diagnostic.with_help(if offered == 0 {
                    "keep one target and add a transition that enters the other state, or place both in different regions of a common `<parallel>` ancestor"
                } else {
                    "keep only one of these targets, or place them in different regions of a common `<parallel>` ancestor"
                });
                report.push(diagnostic);
                break;
            }
        }
        Ok(())
    }

    /// `SCXML080`: two targets of one set where one is a `<history>` and the
    /// other lies below that history's parent. SCXML 1.0 §3.10.2 enters the
    /// history's recorded or default configuration, which covers the parent's
    /// subtree, so the set is a legal state specification (§3.11) only when
    /// that configuration happens to agree with the other target; otherwise
    /// the document is not conformant ("A conformant SCXML document MUST
    /// place the state machine only in legal state configurations") and
    /// §A.2 leaves the processor's behavior undefined. The recorded
    /// configuration is known only at run time, so this is a warning.
    fn history_overlap(
        &self,
        noun: &str,
        earlier: &Target,
        later: &Target,
        budget: &mut Budget,
    ) -> Result<Option<Diagnostic>, Exhausted> {
        for (history, other) in [(earlier, later), (later, earlier)] {
            let (Some(history_node), Some(other_node)) = (history.node, other.node) else {
                continue;
            };
            let Some(node) = self.node(history_node) else {
                continue;
            };
            if node.kind != Kind::History {
                continue;
            }
            let Some(parent) = node.parent else {
                continue;
            };
            if other_node == history_node || !self.is_proper_ancestor(parent, other_node, budget)? {
                continue;
            }
            return Ok(Some(
                Diagnostic::warning(
                    "SCXML080",
                    format!(
                        "{noun}s `{}` and `{}` can place the machine in an illegal configuration",
                        history.token, other.token
                    ),
                    later.span,
                )
                .with_primary_message(format!(
                    "`{}` lies inside what `{}` restores",
                    other.token, history.token
                ))
                .with_secondary(Label::new(earlier.span).with_message(format!(
                    "`{}` is entered here",
                    earlier.token
                )))
                .with_note(format!(
                    "entering the history `{}` enters its recorded (or default) configuration of {} (SCXML 1.0 §3.10.2: \"the SCXML processor MUST behave as if the transition had taken the stored state configuration for that history state as its target\"); unless that configuration contains `{}`, the machine ends up with two active children of one state, and \"A conformant SCXML document MUST place the state machine only in legal state configurations\" (§3.11)",
                    history.token,
                    self.name(parent),
                    other.token
                ))
                .with_help(format!(
                    "target either the history `{}` or `{}`, not both",
                    history.token, other.token
                ))
                .with_spec_reference(SPEC_CONFIGURATION)
                .with_spec_reference("SCXML 1.0 §3.10 <history>"),
            ));
        }
        Ok(None)
    }

    /// The `SCXML030` finding, without fixes, for two targets of one set
    /// that lie in different children of a compound state or are nested;
    /// `None` for a repeated target, which has its own finding.
    fn conflict_diagnostic(
        &self,
        noun: &str,
        conflict: Conflict,
        earlier: &Target,
        later: &Target,
    ) -> Option<Diagnostic> {
        let (message, primary_message, note) = match conflict {
            Conflict::Duplicate => return None,
            Conflict::Siblings(common) => (
                format!(
                    "{noun}s `{}` and `{}` cannot be active at the same time",
                    earlier.token, later.token
                ),
                format!("conflicts with `{}`", earlier.token),
                format!(
                    "both targets lie in different children of {}, which is not a <parallel> and can have only one active child; a multi-target set must form a legal state configuration",
                    self.name(common)
                ),
            ),
            Conflict::Nested { ancestor } => {
                let (outer, inner) = if Some(ancestor) == earlier.node {
                    (earlier, later)
                } else {
                    (later, earlier)
                };
                (
                    format!(
                        "{noun}s `{}` and `{}` are nested: `{}` contains `{}`",
                        earlier.token, later.token, outer.token, inner.token
                    ),
                    format!("nested with `{}`", earlier.token),
                    format!(
                        "entering `{}` already enters `{}`; each target of a multi-target set must lie in a different region of a common <parallel>, so a set that names a state and one of its descendants is rejected by the compiler",
                        inner.token, outer.token
                    ),
                )
            }
        };
        let mut diagnostic = Diagnostic::error("SCXML030", message, later.span)
            .with_primary_message(primary_message)
            .with_secondary(
                Label::new(earlier.span)
                    .with_message(format!("`{}` is entered here", earlier.token)),
            )
            .with_note(note)
            .with_spec_reference(SPEC_CONFIGURATION)
            .with_spec_reference(SPEC_TRANSITION);
        if let Conflict::Siblings(common) = conflict
            && let Some(node) = self.node(common)
        {
            diagnostic = diagnostic.with_secondary(
                Label::new(node.element.start_tag_span)
                    .with_message("only one child of this state can be active"),
            );
        }
        Some(diagnostic)
    }

    /// Why two targets of one set cannot form a legal configuration, exactly
    /// as the compiler's Machine IR check decides it: every pair of distinct
    /// targets (a `<history>` counts as itself, a child of its parent) must
    /// lie in different regions of a common `<parallel>`.
    fn orthogonality_conflict(
        &self,
        left: usize,
        right: usize,
        budget: &mut Budget,
    ) -> Result<Option<Conflict>, Exhausted> {
        if left == right {
            return Ok(Some(Conflict::Duplicate));
        }
        if self.is_proper_ancestor(left, right, budget)? {
            return Ok(Some(Conflict::Nested { ancestor: left }));
        }
        if self.is_proper_ancestor(right, left, budget)? {
            return Ok(Some(Conflict::Nested { ancestor: right }));
        }
        let Some(common) = self.lowest_common_ancestor(left, right, budget)? else {
            return Ok(None);
        };
        Ok(match self.kind(common) {
            Some(Kind::Root | Kind::State) => Some(Conflict::Siblings(common)),
            _ => None,
        })
    }

    // -----------------------------------------------------------------------
    // SCXML033: single-region parallel
    // -----------------------------------------------------------------------

    fn check_single_region_parallel(&self, document: &Document, report: &mut Report) {
        for node in &self.nodes {
            if node.kind != Kind::Parallel || node.children.len() != 1 {
                continue;
            }
            let Some(region) = node.children.first().and_then(|child| self.node(*child)) else {
                continue;
            };
            let mut diagnostic = Diagnostic::warning(
                "SCXML033",
                format!("<parallel> {} has only one region", node.name()),
                node.element.start_tag_span,
            )
            .with_category(DiagnosticCategory::Lint)
            .with_primary_message("a parallel state with one child runs nothing in parallel")
            .with_secondary(Label::new(region.anchor()).with_message("the only region"))
            .with_note("as a <state>, the region becomes the default child; completion differs: a <parallel> completes when all regions reach final states, while a <state> completes only when one of its own <final> children is entered")
            .with_help("use `<state>` instead, or add the missing orthogonal regions")
            .with_spec_reference(SPEC_PARALLEL);
            if let Some(edits) = rename_element_edits(document, node.element, "state") {
                diagnostic = diagnostic.with_fix(Fix {
                    message: "change `<parallel>` to `<state>`".to_owned(),
                    applicability: Applicability::MaybeIncorrect,
                    edits,
                });
            }
            report.push(diagnostic);
        }
    }

    // -----------------------------------------------------------------------
    // SCXML034: shadowed transitions
    // -----------------------------------------------------------------------

    fn check_shadowed_transitions(
        &self,
        document: &Document,
        budget: &mut Budget,
        report: &mut Report,
    ) -> Result<(), Exhausted> {
        for node in &self.nodes {
            if !matches!(node.kind, Kind::State | Kind::Parallel | Kind::Final) {
                continue;
            }
            for (position, later_index) in node.transitions.iter().enumerate() {
                let Some(later) = self.transitions.get(*later_index) else {
                    continue;
                };
                for (earlier_position, earlier_index) in
                    node.transitions.iter().enumerate().take(position)
                {
                    budget.spend(1)?;
                    let Some(earlier) = self.transitions.get(*earlier_index) else {
                        continue;
                    };
                    if !shadows(earlier, later) {
                        continue;
                    }
                    // Moving `later` above `earlier` resolves the finding
                    // only when it then shadows none of the transitions it
                    // moves above (for example not when both match exactly
                    // the same events, which would just swap the roles).
                    let mut reorder_resolves = true;
                    for moved_over in node
                        .transitions
                        .get(earlier_position..position)
                        .unwrap_or_default()
                    {
                        budget.spend(1)?;
                        if self
                            .transitions
                            .get(*moved_over)
                            .is_none_or(|transition| shadows(later, transition))
                        {
                            reorder_resolves = false;
                            break;
                        }
                    }
                    report.push(shadow_diagnostic(
                        document,
                        node,
                        earlier,
                        later,
                        reorder_resolves,
                    ));
                    break;
                }
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // SCXML036: eventless liveness
    // -----------------------------------------------------------------------

    /// The transition the interpreter must select in every microstep while
    /// `atomic` is the (non-parallel) active configuration.
    fn eventless_step(&self, atomic: usize, budget: &mut Budget) -> Result<Step, Exhausted> {
        let mut current = Some(atomic);
        while let Some(index) = current {
            let Some(node) = self.node(index) else {
                return Ok(Step::Unknown);
            };
            for transition_index in &node.transitions {
                budget.spend(1)?;
                let Some(transition) = self.transitions.get(*transition_index) else {
                    return Ok(Step::Unknown);
                };
                match &transition.events {
                    Some(events) if !events.is_empty() => continue,
                    Some(_) => return Ok(Step::Unknown),
                    None => {}
                }
                if transition.conditional {
                    return Ok(Step::Unknown);
                }
                if transition.target.is_none() {
                    // Neither `event`, `cond`, nor `target`: the SCXML004
                    // error (SCXML 1.0 §3.5.2); nothing more to decide.
                    return Ok(Step::Unknown);
                }
                let Some((_, target)) = transition.single_target() else {
                    return Ok(Step::Unknown);
                };
                return Ok(match self.landing(target, budget)? {
                    Some(Landing::Atomic(landing)) => Step::Enter {
                        transition: *transition_index,
                        landing,
                    },
                    Some(Landing::Halt) | None => Step::Unknown,
                });
            }
            current = node.parent;
        }
        Ok(Step::Stable)
    }

    /// The single atomic state reached by entering `target` and its defaults.
    fn landing(&self, target: usize, budget: &mut Budget) -> Result<Option<Landing>, Exhausted> {
        let mut current = target;
        loop {
            budget.spend(1)?;
            if self.has_parallel_ancestor(current, budget)? {
                return Ok(None);
            }
            let Some(node) = self.node(current) else {
                return Ok(None);
            };
            match node.kind {
                Kind::Final if node.parent == Some(0) => return Ok(Some(Landing::Halt)),
                _ if node.is_atomic() => return Ok(Some(Landing::Atomic(current))),
                Kind::State | Kind::Root => match self.defaults.get(current) {
                    Some(Entry::Targets(targets)) => match targets.as_slice() {
                        [next]
                            if matches!(
                                self.kind(*next),
                                Some(Kind::State | Kind::Parallel | Kind::Final)
                            ) =>
                        {
                            current = *next;
                        }
                        _ => return Ok(None),
                    },
                    Some(Entry::Unknown) | None => return Ok(None),
                },
                Kind::Final | Kind::Parallel | Kind::History | Kind::Initial => return Ok(None),
            }
        }
    }

    fn check_eventless_liveness(
        &self,
        budget: &mut Budget,
        report: &mut Report,
    ) -> Result<(), Exhausted> {
        let count = self.nodes.len();
        let mut successor: Vec<Option<(usize, usize)>> = vec![None; count];
        for (index, node) in self.nodes.iter().enumerate() {
            budget.spend(1)?;
            if !node.is_atomic() || self.has_parallel_ancestor(index, budget)? {
                continue;
            }
            match self.eventless_step(index, budget)? {
                Step::Enter {
                    transition,
                    landing,
                } => {
                    if let Some(slot) = successor.get_mut(index) {
                        *slot = Some((landing, transition));
                    }
                }
                Step::Stable | Step::Unknown => {}
            }
        }
        for cycle in functional_cycles(&successor, budget)? {
            if let Some(diagnostic) = self.cycle_diagnostic(&cycle, &successor) {
                report.push(diagnostic);
            }
        }
        Ok(())
    }

    fn cycle_diagnostic(
        &self,
        cycle: &[usize],
        successor: &[Option<(usize, usize)>],
    ) -> Option<Diagnostic> {
        // (active atomic state, selected transition) for each step.
        let mut steps: Vec<(usize, usize)> = cycle
            .iter()
            .filter_map(|state| {
                successor
                    .get(*state)
                    .copied()
                    .flatten()
                    .map(|(_, transition)| (*state, transition))
            })
            .collect();
        if steps.len() != cycle.len() || steps.is_empty() {
            return None;
        }
        // Start at the transition declared first in the document.
        let start = steps
            .iter()
            .enumerate()
            .min_by_key(|(_, (_, transition))| {
                self.transitions
                    .get(*transition)
                    .map_or(usize::MAX, |transition| transition.element.span.start)
            })
            .map_or(0, |(position, _)| position);
        steps.rotate_left(start);
        if self.direct_cycle_is_reported(&steps) {
            return None;
        }
        let mut names: Vec<String> = steps.iter().map(|(state, _)| self.name(*state)).collect();
        if let Some(first) = names.first().cloned() {
            names.push(first);
        }
        let (_, first_transition) = *steps.first()?;
        let first = self.transitions.get(first_transition)?;
        let (first_target, _) = first.single_target()?;
        let mut diagnostic = Diagnostic::warning(
            "SCXML036",
            format!(
                "eventless transitions loop forever without consuming an event: {}",
                names.join(" -> ")
            ),
            first_target.span,
        )
        .with_primary_message(self.step_message(0, &steps))
        .with_note("each step is the first eventless transition without `cond` on the active state's ancestor chain, so it is always selected; default entry through `initial` targets closes the loop")
        .with_help("add an `event` or `cond` to one of these transitions, or retarget one so the macrostep can reach a stable configuration")
        .with_spec_reference(SPEC_SELECTION);
        for position in 0..steps.len() {
            let (state, transition_index) = steps.get(position).copied()?;
            let transition = self.transitions.get(transition_index)?;
            if position > 0
                && let Some((target, _)) = transition.single_target()
            {
                diagnostic = diagnostic.with_secondary(
                    Label::new(target.span).with_message(self.step_message(position, &steps)),
                );
            }
            if transition.owner != state {
                diagnostic = diagnostic.with_secondary(
                    Label::new(transition.element.start_tag_span).with_message(format!(
                        "declared on {} and selected while {} is active",
                        self.name(transition.owner),
                        self.name(state)
                    )),
                );
            }
        }
        Some(diagnostic)
    }

    fn step_message(&self, position: usize, steps: &[(usize, usize)]) -> String {
        let next = steps
            .get((position + 1) % steps.len().max(1))
            .map_or(0, |(state, _)| *state);
        let target = steps
            .get(position)
            .and_then(|(_, transition)| self.transitions.get(*transition))
            .and_then(|transition| transition.single_target())
            .map(|(_, node)| node);
        match target {
            Some(target) if target != next => format!(
                "enters {}, which lands in {} by default",
                self.name(target),
                self.name(next)
            ),
            _ => format!("enters {}", self.name(next)),
        }
    }

    /// Whether `SCXML026` already reports this loop as direct state-to-state
    /// targets (each target owns the next selected transition).
    fn direct_cycle_is_reported(&self, steps: &[(usize, usize)]) -> bool {
        steps.iter().enumerate().all(|(position, (_, transition))| {
            let next_transition = steps
                .get((position + 1) % steps.len())
                .and_then(|(_, next)| self.transitions.get(*next));
            let target = self
                .transitions
                .get(*transition)
                .and_then(|transition| transition.single_target());
            match (target, next_transition) {
                (Some((target, _)), Some(next)) => self
                    .node(next.owner)
                    .and_then(Node::id)
                    .is_some_and(|id| id.value == target.token),
                _ => false,
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    /// No eventless transition is enabled.
    Stable,
    /// A condition, history, parallel region, or malformed target makes the
    /// outcome undecidable from the static tree.
    Unknown,
    /// This transition is selected and lands in `landing`.
    Enter { transition: usize, landing: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Landing {
    Atomic(usize),
    Halt,
}

/// Finds every cycle of a functional graph (each node has at most one
/// successor), reporting each cycle once in traversal order.
fn functional_cycles(
    successor: &[Option<(usize, usize)>],
    budget: &mut Budget,
) -> Result<Vec<Vec<usize>>, Exhausted> {
    const UNSEEN: u8 = 0;
    const ON_PATH: u8 = 1;
    const DONE: u8 = 2;
    let mut color = vec![UNSEEN; successor.len()];
    let mut cycles = Vec::new();
    for start in 0..successor.len() {
        if color.get(start).copied() != Some(UNSEEN)
            || successor.get(start).copied().flatten().is_none()
        {
            continue;
        }
        let mut path = Vec::new();
        let mut current = start;
        loop {
            budget.spend(1)?;
            match color.get(current).copied() {
                Some(ON_PATH) => {
                    if let Some(position) = path.iter().position(|node| *node == current) {
                        budget.spend(path.len())?;
                        cycles.push(path.get(position..).unwrap_or_default().to_vec());
                    }
                    break;
                }
                Some(UNSEEN) => {}
                _ => break,
            }
            if let Some(slot) = color.get_mut(current) {
                *slot = ON_PATH;
            }
            path.push(current);
            match successor.get(current).copied().flatten() {
                Some((next, _)) => current = next,
                None => break,
            }
        }
        for node in path {
            if let Some(slot) = color.get_mut(node) {
                *slot = DONE;
            }
        }
    }
    Ok(cycles)
}

fn kind_noun(kind: Kind) -> &'static str {
    match kind {
        Kind::Parallel => "parallel state",
        Kind::Final => "final state",
        Kind::Root | Kind::State | Kind::History | Kind::Initial => "state",
    }
}

// ---------------------------------------------------------------------------
// Shadowing
// ---------------------------------------------------------------------------

/// Whether `earlier` always wins document-order selection over `later`.
fn shadows(earlier: &Transition<'_>, later: &Transition<'_>) -> bool {
    if earlier.conditional {
        return false;
    }
    match (&earlier.events, &later.events) {
        (None, None) => true,
        (Some(earlier), Some(later)) => {
            if earlier.is_empty()
                || later.is_empty()
                || !earlier
                    .iter()
                    .chain(later)
                    .all(|token| valid_event_descriptor(token))
            {
                return false;
            }
            later.iter().all(|descriptor| {
                earlier
                    .iter()
                    .any(|candidate| descriptor_covers(candidate, descriptor))
            })
        }
        _ => false,
    }
}

/// Whether every event matched by `narrow` is also matched by `broad`.
fn descriptor_covers(broad: &str, narrow: &str) -> bool {
    let normalize = |token: &str| -> Option<Vec<String>> {
        if token == "*" || token == ".*" {
            return None;
        }
        let token = token.strip_suffix(".*").unwrap_or(token);
        let token = token.strip_suffix('.').unwrap_or(token);
        Some(token.split('.').map(str::to_owned).collect())
    };
    match (normalize(broad), normalize(narrow)) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(broad), Some(narrow)) => narrow.starts_with(&broad),
    }
}

fn shadow_diagnostic(
    document: &Document,
    owner: &Node<'_>,
    earlier: &Transition<'_>,
    later: &Transition<'_>,
    reorder_resolves: bool,
) -> Diagnostic {
    let (message, primary, reason) = match &later.events {
        Some(events) => (
            format!(
                "transition on `{}` in {} can never be selected",
                events.join(" "),
                owner.name()
            ),
            later
                .element
                .attribute("event")
                .map_or(later.element.start_tag_span, |event| event.value_span),
            "every event it matches is matched first by an earlier unconditional transition",
        ),
        None => (
            format!(
                "eventless transition in {} can never be selected",
                owner.name()
            ),
            later.element.start_tag_span,
            "an earlier eventless transition without `cond` is always enabled",
        ),
    };
    let mut diagnostic = Diagnostic::warning("SCXML034", message, primary)
        .with_category(DiagnosticCategory::Lint)
        .with_primary_message("shadowed transition")
        .with_secondary(
            Label::new(earlier.element.start_tag_span)
                .with_message("this earlier transition has no `cond` and always matches first"),
        )
        .with_note(format!(
            "within one state the interpreter selects the first enabled transition in document order; {reason}"
        ))
        .with_help(if reorder_resolves {
            "move this transition above the earlier one if it should take priority, add a `cond` to the earlier transition, or remove this one"
        } else {
            "both transitions match the same events, so reordering would only shadow the other one: add a `cond` to the one that should be taken only sometimes, or remove this one"
        })
        .with_spec_reference(SPEC_SELECTION);
    let removal = removal_span(document, later.element.span);
    if reorder_resolves && let Some(text) = document.source.slice(later.element.span) {
        let indentation = line_indentation(document, earlier.element.span.start);
        diagnostic = diagnostic.with_fix(Fix {
            message: "move this transition before the earlier one".to_owned(),
            applicability: Applicability::MaybeIncorrect,
            edits: vec![
                FixEdit {
                    span: Span::new(
                        earlier.element.span.source,
                        earlier.element.span.start,
                        earlier.element.span.start,
                    ),
                    replacement: format!("{text}\n{indentation}"),
                },
                FixEdit {
                    span: removal,
                    replacement: String::new(),
                },
            ],
        });
    }
    diagnostic.with_fix(Fix {
        message: "remove the unreachable transition".to_owned(),
        applicability: Applicability::MaybeIncorrect,
        edits: vec![FixEdit {
            span: removal,
            replacement: String::new(),
        }],
    })
}

// ---------------------------------------------------------------------------
// Element-level rules that do not need the graph
// ---------------------------------------------------------------------------

/// SCXML031: `<final>` may contain only `<onentry>`, `<onexit>`, and `<donedata>`.
fn check_final_content(machine: &Element, document: &Document, report: &mut Report) {
    visit_machine(machine, &mut |element| {
        if !is_scxml(element, "final") {
            return;
        }
        if let Some(initial) = element.attribute("initial") {
            report.push(
                Diagnostic::error(
                    "SCXML031",
                    "final state cannot declare an `initial` state",
                    initial.span,
                )
                .with_primary_message("a final state is atomic")
                .with_secondary(
                    Label::new(element.start_tag_span).with_message("final state declared here"),
                )
                .with_help("remove the `initial` attribute")
                .with_fix(Fix {
                    message: "remove `initial`".to_owned(),
                    applicability: Applicability::MachineApplicable,
                    edits: vec![FixEdit {
                        span: attribute_removal_span(document, initial.span),
                        replacement: String::new(),
                    }],
                })
                .with_spec_reference(SPEC_FINAL),
            );
        }
        let mut reported_rename = false;
        for child in child_elements(element) {
            if child.name.namespace_uri.as_deref() != Some(crate::SCXML_NAMESPACE)
                || ["onentry", "onexit", "donedata"].contains(&child.name.local_name.as_str())
            {
                continue;
            }
            let local = child.name.local_name.as_str();
            let structural = matches!(
                local,
                "transition" | "state" | "parallel" | "final" | "initial" | "history" | "invoke"
            );
            let mut diagnostic = Diagnostic::error(
                "SCXML031",
                format!("final state cannot contain `<{local}>`"),
                child.start_tag_span,
            )
            .with_primary_message(format!("`<{local}>` is not allowed in `<final>`"))
            .with_secondary(
                Label::new(element.start_tag_span).with_message("final state declared here"),
            )
            .with_note("a <final> state is atomic and completes its parent when entered; it may contain only <onentry>, <onexit>, and <donedata>")
            .with_help(if structural {
                "change `<final>` to `<state>` if it must react or contain children, or move this element to a non-final state"
            } else {
                "move this element into `<onentry>` or `<onexit>`, or remove it"
            })
            .with_spec_reference(SPEC_FINAL);
            if structural
                && !reported_rename
                && let Some(edits) = rename_element_edits(document, element, "state")
            {
                reported_rename = true;
                diagnostic = diagnostic.with_fix(Fix {
                    message: "change `<final>` to `<state>`".to_owned(),
                    applicability: Applicability::MaybeIncorrect,
                    edits,
                });
            }
            diagnostic = diagnostic.with_fix(Fix {
                message: format!("remove this `<{local}>`"),
                applicability: Applicability::MaybeIncorrect,
                edits: vec![FixEdit {
                    span: removal_span(document, child.span),
                    replacement: String::new(),
                }],
            });
            report.push(diagnostic);
        }
    });
}

/// SCXML032: `<donedata>` belongs only in `<final>`, at most once.
fn check_donedata_placement(machine: &Element, document: &Document, report: &mut Report) {
    visit_machine(machine, &mut |element| {
        let is_final = is_scxml(element, "final");
        let mut first: Option<&Element> = None;
        for child in child_elements(element) {
            if !is_scxml(child, "donedata") {
                continue;
            }
            if !is_final {
                report.push(
                    Diagnostic::error(
                        "SCXML032",
                        format!(
                            "`<donedata>` is only allowed in `<final>`, not in `<{}>`",
                            element.name.local_name
                        ),
                        child.start_tag_span,
                    )
                    .with_primary_message("donedata outside a final state")
                    .with_secondary(
                        Label::new(element.start_tag_span).with_message("parent declared here"),
                    )
                    .with_note("<donedata> supplies the payload of the done.state event raised when a <final> state is entered; nothing reads it anywhere else")
                    .with_help("move this `<donedata>` into the `<final>` state whose completion should carry the data, or remove it")
                    .with_fix(Fix {
                        message: "remove this `<donedata>`".to_owned(),
                        applicability: Applicability::MaybeIncorrect,
                        edits: vec![FixEdit {
                            span: removal_span(document, child.span),
                            replacement: String::new(),
                        }],
                    })
                    .with_spec_reference(SPEC_DONEDATA),
                );
                continue;
            }
            if let Some(first) = first {
                report.push(
                    Diagnostic::error(
                        "SCXML032",
                        "`<final>` may contain at most one `<donedata>`",
                        child.start_tag_span,
                    )
                    .with_primary_message("second donedata")
                    .with_secondary(
                        Label::new(first.start_tag_span).with_message("first donedata here"),
                    )
                    .with_help("merge the payloads into a single `<donedata>`")
                    .with_spec_reference(SPEC_DONEDATA)
                    .with_spec_reference(SPEC_FINAL),
                );
            } else {
                first = Some(child);
            }
        }
    });
}

/// Why two targets of one multi-target set conflict.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Conflict {
    /// The same state is listed twice.
    Duplicate,
    /// One target is a proper ancestor of the other.
    Nested {
        /// The containing target.
        ancestor: usize,
    },
    /// The targets lie in different children of this non-parallel state.
    Siblings(usize),
}

/// `SCXML072` (warning) for a state listed twice in one target set. SCXML
/// 1.0 §3.11 defines "a legal state specification to be a set of states
/// such that 1) no state is an ancestor of any other state on the list, and
/// 2) a full legal state configuration results when all ancestors and
/// default initial descendants have been added": the repeat names the same
/// member of the set, so the document is conforming and the compiler
/// enters the state once. The repeat is removed exactly (the set enters the
/// same configuration).
fn duplicate_target_diagnostic(
    document: &Document,
    noun: &str,
    earlier: &Target,
    later: &Target,
    targets: &[Target],
) -> Diagnostic {
    let diagnostic = Diagnostic::warning(
        "SCXML072",
        format!("{noun} `{}` is listed twice", later.token),
        later.span,
    )
    .with_primary_message("listed again here")
    .with_secondary(Label::new(earlier.span).with_message("first listed here"))
    .with_note("a target list names a set of states, so the repeat adds nothing: the state is entered once")
    .with_help(format!("remove the repeated `{}`", later.token))
    .with_spec_reference(SPEC_CONFIGURATION)
    .with_spec_reference(SPEC_TRANSITION);
    match token_removal_edit(
        document,
        later.span,
        targets.first().map(|first| first.span),
    ) {
        Some(edit) => diagnostic.with_fix(Fix {
            message: format!("remove the repeated `{}`", later.token),
            applicability: Applicability::MachineApplicable,
            edits: vec![edit],
        }),
        None => diagnostic,
    }
}

// ---------------------------------------------------------------------------
// Source edit helpers
// ---------------------------------------------------------------------------

/// Removes a whole element, including its line when it stands alone.
pub(crate) fn removal_span(document: &Document, span: Span) -> Span {
    let text = document.source.text();
    let (Some(before), Some(after)) = (text.get(..span.start), text.get(span.end..)) else {
        return span;
    };
    let line_start = before.rfind('\n');
    let leading = line_start.map_or(before, |newline| before.get(newline + 1..).unwrap_or(""));
    let trailing = after.split('\n').next().unwrap_or("");
    let blank = |value: &str| {
        value
            .chars()
            .all(|character| character == ' ' || character == '\t')
    };
    match line_start {
        Some(newline) if blank(leading) && blank(trailing) => {
            Span::new(span.source, newline, span.end + trailing.len())
        }
        _ => span,
    }
}

/// Removes one attribute together with the whitespace separating it from
/// the previous token.
pub(crate) fn attribute_removal_span(document: &Document, span: Span) -> Span {
    let Some(before) = document.source.text().get(..span.start) else {
        return span;
    };
    let whitespace = before.len() - before.trim_end().len();
    Span::new(span.source, span.start - whitespace, span.end)
}

/// Removes one whitespace-separated token from an attribute value.
/// Removes one whitespace-separated token from an attribute value: a later
/// token with its preceding whitespace, the first token with its following
/// whitespace. A lone token is never removed (the attribute would be empty).
fn token_removal_edit(document: &Document, token: Span, first: Option<Span>) -> Option<FixEdit> {
    if first == Some(token) {
        let after = document.source.text().get(token.end..)?;
        let whitespace = after.len() - after.trim_start().len();
        let next = after.get(whitespace..)?.chars().next()?;
        if whitespace == 0 || matches!(next, '"' | '\'') {
            return None;
        }
        return Some(FixEdit {
            span: Span::new(token.source, token.start, token.end + whitespace),
            replacement: String::new(),
        });
    }
    let before = document.source.text().get(..token.start)?;
    let whitespace = before.len() - before.trim_end().len();
    Some(FixEdit {
        span: Span::new(token.source, token.start - whitespace, token.end),
        replacement: String::new(),
    })
}

fn line_indentation(document: &Document, offset: usize) -> String {
    let before = document.source.text().get(..offset).unwrap_or("");
    let line = before
        .rfind('\n')
        .map_or(before, |newline| before.get(newline + 1..).unwrap_or(""));
    if line
        .chars()
        .all(|character| character == ' ' || character == '\t')
    {
        line.to_owned()
    } else {
        String::new()
    }
}

/// Renames an element's local name in its start tag and (when present) its
/// end tag, preserving any namespace prefix.
pub(crate) fn rename_element_edits(
    document: &Document,
    element: &Element,
    local_name: &str,
) -> Option<Vec<FixEdit>> {
    let text = document.source.text();
    let lexical = element.lexical_name.as_str();
    let prefix_len = lexical.len().checked_sub(element.name.local_name.len())?;
    let start_name = element.start_tag_span.start.checked_add(1)?;
    if text.get(start_name..start_name.checked_add(lexical.len())?)? != lexical {
        return None;
    }
    let local_start = start_name + prefix_len;
    let mut edits = vec![FixEdit {
        span: Span::new(
            element.span.source,
            local_start,
            local_start + element.name.local_name.len(),
        ),
        replacement: local_name.to_owned(),
    }];
    if element.span.end > element.start_tag_span.end {
        let whole = text.get(element.span.start..element.span.end)?;
        let close = whole.rfind("</")?;
        let close_name = element.span.start + close + 2;
        if text.get(close_name..close_name.checked_add(lexical.len())?)? != lexical {
            return None;
        }
        let local_close = close_name + prefix_len;
        edits.push(FixEdit {
            span: Span::new(
                element.span.source,
                local_close,
                local_close + element.name.local_name.len(),
            ),
            replacement: local_name.to_owned(),
        });
    }
    Some(edits)
}

#[cfg(test)]
mod tests;
