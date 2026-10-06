//! Structural diff of two valid documents (`scxml.session-snapshot-diff/v2`).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::validate::decode;
use crate::{
    ComponentRole, Configuration, DiffOptions, Document, Event, Invocation, InvokedBy, IrIdentity,
    PendingSend, SESSION_SNAPSHOT_DIFF_VERSION, Section, Session, SnapshotError, SnapshotErrorCode,
    Status, Transition, Value, VariableValue,
};

/// A reference to one compared document.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentRef {
    /// The document's `sha256` (of its `snapshot`).
    pub sha256: String,
}

/// How one keyed item changed.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    /// Only in `after`.
    Added,
    /// Only in `before`.
    Removed,
    /// In both, different.
    Changed,
}

/// The shared clock moved.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClockChange {
    /// `before` position.
    pub from_ms: u64,
    /// `after` position.
    pub to_ms: u64,
}

/// A status transition.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StatusChange {
    /// `before` status.
    pub from: Status,
    /// `after` status.
    pub to: Status,
}

/// A changed required value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ValueChange<T> {
    /// `before` value.
    pub from: T,
    /// `after` value.
    pub to: T,
}

/// A changed optional value; an absent side is omitted.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, bound(deserialize = "T: Deserialize<'de>"))]
pub struct OptionalChange<T> {
    /// `before` value (absent when `before` has none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<T>,
    /// `after` value (absent when `after` has none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<T>,
}

/// One changed component identity, keyed by role.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentChange {
    /// Component role.
    pub role: ComponentRole,
    /// How it changed.
    pub change: Change,
    /// `before` digest (absent when added).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// `after` digest (absent when removed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

/// Set differences of one state list, document order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StateSetDiff {
    /// Listed before, not after (`before` order).
    pub exited: Vec<String>,
    /// Listed after, not before (`after` order).
    pub entered: Vec<String>,
}

/// Set differences of the configuration, document order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationDiff {
    /// Active before, not after (`before` order).
    pub exited: Vec<String>,
    /// Active after, not before (`after` order).
    pub entered: Vec<String>,
    /// Set differences of `atomic`, present only when they hold a state
    /// that is not in `exited` or `entered` (a state still active whose
    /// atomic listing changed, as across an IR change).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub atomic: Option<StateSetDiff>,
}

/// Late-binding states newly bound or no longer bound.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BoundStatesDiff {
    /// Bound after, not before (`after` order).
    pub added: Vec<String>,
    /// Bound before, not after (`before` order).
    pub removed: Vec<String>,
}

/// A session list whose entries are equal on both sides but whose order
/// differs.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionList {
    /// `configuration.full` or `configuration.atomic`.
    Configuration,
    /// `history`.
    History,
    /// `bound_states`.
    BoundStates,
    /// `data_model`.
    DataModel,
    /// `pending_sends`.
    PendingSends,
    /// `invocations`.
    Invocations,
    /// `transitions`.
    Transitions,
}

/// Which system variable changed.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemKey {
    /// `_sessionid`.
    Sessionid,
    /// `_name`.
    Name,
    /// `_ioprocessors`.
    Ioprocessors,
}

/// One changed system variable (identifiers shown as `text` values).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SystemChange {
    /// Which variable.
    pub name: SystemKey,
    /// How it changed.
    pub change: Change,
    /// `before` value (absent when added).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Value>,
    /// `after` value (absent when removed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Value>,
}

/// One changed user variable.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KeyedChange {
    /// Variable name.
    pub name: String,
    /// How it changed.
    pub change: Change,
    /// `before` value (absent when added).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<VariableValue>,
    /// `after` value (absent when removed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<VariableValue>,
}

/// One changed history value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryChange {
    /// History pseudostate.
    pub state: String,
    /// How it changed.
    pub change: Change,
    /// `before` members (absent when added).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Vec<String>>,
    /// `after` members (absent when removed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Vec<String>>,
}

/// One changed counter.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CounterChange {
    /// `before` ordinal.
    pub from: u64,
    /// `after` ordinal.
    pub to: u64,
}

/// Changed counters.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CountersDiff {
    /// Send-id ordinal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send: Option<CounterChange>,
    /// Invoke-id ordinal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invoke: Option<CounterChange>,
}

/// A FIFO change: `consumed` entries left the head and `appended` joined the
/// tail. `consumed` is the smallest such count, and one always exists
/// (consuming the whole `before` queue relates any two queues), so every
/// FIFO change has this one form.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FifoDiff<T> {
    /// Entries removed from the head.
    pub consumed: u64,
    /// Entries added to the tail.
    pub appended: Vec<T>,
}

/// Changed queues.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QueuesDiff {
    /// Internal queue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub internal: Option<FifoDiff<Event>>,
    /// External queue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<FifoDiff<Event>>,
}

impl QueuesDiff {
    fn is_empty(&self) -> bool {
        self.internal.is_none() && self.external.is_none()
    }
}

/// Pending-send changes, keyed by `sendid`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PendingSendsDiff {
    /// New or changed sends (`after` order).
    pub added: Vec<PendingSend>,
    /// Gone or changed send ids (`before` order).
    pub removed: Vec<String>,
}

/// The metadata of one invocation (its invoked session is diffed on its own
/// path).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationMetadata {
    /// Invoke type URI.
    #[serde(rename = "type")]
    pub invoke_type: String,
    /// Invoking state.
    pub state: String,
    /// Index among the state's invokes.
    pub index: u32,
    /// `autoforward`.
    pub autoforward: bool,
    /// Whether the child finished.
    pub finished: bool,
}

impl InvocationMetadata {
    fn of(invocation: &Invocation) -> Self {
        Self {
            invoke_type: invocation.invoke_type.clone(),
            state: invocation.state.clone(),
            index: invocation.index,
            autoforward: invocation.autoforward,
            finished: invocation.finished,
        }
    }
}

/// An invocation present on both sides whose metadata changed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationChange {
    /// Invoke id.
    pub invoke_id: String,
    /// `before` metadata.
    pub from: InvocationMetadata,
    /// `after` metadata.
    pub to: InvocationMetadata,
}

/// Invocation changes, keyed by invoke id.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationsDiff {
    /// New invoke ids (`after` order).
    pub added: Vec<String>,
    /// Gone invoke ids (`before` order).
    pub removed: Vec<String>,
    /// Invocations on both sides whose metadata changed (`after` order).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed: Option<Vec<InvocationChange>>,
}

/// A transition key `(source, index)`.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionKey {
    /// Source state.
    pub source: String,
    /// Index among the source's transitions.
    pub index: u32,
}

/// A transition listed on both sides whose listing changed (events,
/// targets, type, `cond`, domain, or its `exit`/`entry` previews).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionChange {
    /// Source state.
    pub source: String,
    /// Index among the source's transitions.
    pub index: u32,
    /// `before` listing.
    pub from: Transition,
    /// `after` listing.
    pub to: Transition,
}

/// Changes of the transitions an event could trigger, keyed by
/// `(source, index)`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionsDiff {
    /// Newly available (`after` order).
    pub added: Vec<TransitionKey>,
    /// No longer available (`before` order).
    pub removed: Vec<TransitionKey>,
    /// Listed on both sides with a different listing (`after` order).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed: Option<Vec<TransitionChange>>,
}

/// One session's changes; unchanged keys are omitted.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionDiff {
    /// `["root"]` or `["root", invoke_id]`.
    pub path: Vec<String>,
    /// How the session changed.
    pub change: Change,
    /// `session_id` change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<ValueChange<String>>,
    /// `name` change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<OptionalChange<String>>,
    /// `datamodel_name` change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datamodel_name: Option<OptionalChange<String>>,
    /// Loaded IR change (it also clears `compatible`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ir: Option<ValueChange<IrIdentity>>,
    /// Status transition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<StatusChange>,
    /// `invoked_by` change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invoked_by: Option<OptionalChange<InvokedBy>>,
    /// Configuration change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<ConfigurationDiff>,
    /// Late-binding change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_states: Option<BoundStatesDiff>,
    /// System variable changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<Vec<SystemChange>>,
    /// User variable changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_model: Option<Vec<KeyedChange>>,
    /// History changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<Vec<HistoryChange>>,
    /// Counter changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counters: Option<CountersDiff>,
    /// Queue changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queues: Option<QueuesDiff>,
    /// Pending-send changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_sends: Option<PendingSendsDiff>,
    /// Invocation changes (root only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocations: Option<InvocationsDiff>,
    /// Changes of the transitions an event could trigger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transitions: Option<TransitionsDiff>,
    /// Lists whose entries are unchanged but reordered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reordered: Option<Vec<SessionList>>,
}

impl SessionDiff {
    const fn new(path: Vec<String>, change: Change) -> Self {
        Self {
            path,
            change,
            session_id: None,
            name: None,
            datamodel_name: None,
            ir: None,
            status: None,
            invoked_by: None,
            configuration: None,
            bound_states: None,
            system: None,
            data_model: None,
            history: None,
            counters: None,
            queues: None,
            pending_sends: None,
            invocations: None,
            transitions: None,
            reordered: None,
        }
    }

    const fn has_changes(&self) -> bool {
        self.session_id.is_some()
            || self.name.is_some()
            || self.datamodel_name.is_some()
            || self.ir.is_some()
            || self.status.is_some()
            || self.invoked_by.is_some()
            || self.configuration.is_some()
            || self.bound_states.is_some()
            || self.system.is_some()
            || self.data_model.is_some()
            || self.history.is_some()
            || self.counters.is_some()
            || self.queues.is_some()
            || self.pending_sends.is_some()
            || self.invocations.is_some()
            || self.transitions.is_some()
            || self.reordered.is_some()
    }
}

/// The whole diff document.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiffDocument {
    /// Always [`SESSION_SNAPSHOT_DIFF_VERSION`].
    pub version: String,
    /// The earlier document.
    pub before: DocumentRef,
    /// The later document.
    pub after: DocumentRef,
    /// No difference was found (carriers ignored unless requested).
    pub identical: bool,
    /// Every session matched by path loaded the same IR.
    pub compatible: bool,
    /// `options.exclude` changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude: Option<ValueChange<Vec<Section>>>,
    /// Component identity changes, keyed by role (role order).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub components: Option<Vec<ComponentChange>>,
    /// The clock moved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<ClockChange>,
    /// Sections excluded on either side, sorted; always present, and `[]`
    /// when neither document excluded a section.
    pub skipped: Vec<Section>,
    /// Changed sessions: `after` pre-order, then removed ones in `before`
    /// pre-order.
    pub sessions: Vec<SessionDiff>,
}

impl DiffDocument {
    /// Whether any change is reported. It is always `!identical`.
    #[must_use]
    pub const fn has_changes(&self) -> bool {
        self.exclude.is_some()
            || self.components.is_some()
            || self.clock.is_some()
            || !self.sessions.is_empty()
    }
}

/// Canonical `scxml.session-snapshot-diff/v2` bytes of two valid documents.
///
/// # Errors
///
/// The decode failure of either input; its pointer is prefixed with
/// `/before` or `/after`.
pub fn diff(before: &[u8], after: &[u8], options: DiffOptions) -> Result<Vec<u8>, SnapshotError> {
    let prefix = |side: &'static str| {
        move |error: SnapshotError| {
            let pointer = format!("/{side}{}", error.pointer.unwrap_or_default());
            SnapshotError::new(error.code, Some(pointer))
        }
    };
    let before = decode(before).map_err(prefix("before"))?;
    let after = decode(after).map_err(prefix("after"))?;
    let document = diff_documents(&before, &after, options);
    serde_json::to_vec(&document)
        .map_err(|_| SnapshotError::new(SnapshotErrorCode::SchemaViolation, None))
}

fn strip_session(session: &mut Session) {
    let strip = |event: &mut Event| event.trace_context = None;
    if let Some(queues) = &mut session.queues {
        queues.internal.iter_mut().for_each(strip);
        queues.external.iter_mut().for_each(strip);
    }
    if let Some(pending) = &mut session.pending_sends {
        for send in pending.iter_mut() {
            if let Some(event) = &mut send.event {
                strip(event);
            }
            if let Some(message) = &mut send.basichttp {
                message.trace_context = None;
            }
        }
    }
    if let Some(invocations) = &mut session.invocations {
        for invocation in invocations {
            strip_session(&mut invocation.session);
        }
    }
}

/// The structural diff of two decoded documents.
#[must_use]
pub fn diff_documents(before: &Document, after: &Document, options: DiffOptions) -> DiffDocument {
    let mut before_root = before.snapshot.root.clone();
    let mut after_root = after.snapshot.root.clone();
    if !options.include_trace_context {
        strip_session(&mut before_root);
        strip_session(&mut after_root);
    }
    let skipped: BTreeSet<Section> = before
        .options
        .exclude
        .iter()
        .chain(&after.options.exclude)
        .copied()
        .collect();
    let identical = before.options.exclude == after.options.exclude
        && before.snapshot.components == after.snapshot.components
        && before.snapshot.clock == after.snapshot.clock
        && before_root == after_root;
    let mut differ = Differ {
        skipped: &skipped,
        compatible: true,
        sessions: Vec::new(),
    };
    differ.root_and_invoked(&before_root, &after_root);
    let compatible = differ.compatible;
    let sessions = differ.sessions;
    let clock = (before.snapshot.clock != after.snapshot.clock).then_some(ClockChange {
        from_ms: before.snapshot.clock.now_ms,
        to_ms: after.snapshot.clock.now_ms,
    });
    let exclude = (before.options.exclude != after.options.exclude).then(|| ValueChange {
        from: before.options.exclude.clone(),
        to: after.options.exclude.clone(),
    });
    let identities = |document: &Document| {
        document
            .snapshot
            .components
            .iter()
            .map(|component| (component.role, component.sha256.clone()))
            .collect::<Vec<_>>()
    };
    // Roles are unique and sorted in a valid document, so the keyed
    // comparison sees every change.
    let components = keyed(
        &identities(before),
        &identities(after),
        |role, change, from, to| ComponentChange {
            role,
            change,
            from,
            to,
        },
    );
    DiffDocument {
        version: SESSION_SNAPSHOT_DIFF_VERSION.into(),
        before: DocumentRef {
            sha256: before.sha256.clone(),
        },
        after: DocumentRef {
            sha256: after.sha256.clone(),
        },
        identical,
        compatible,
        exclude,
        components,
        clock,
        // Canonical documents write `[]` rather than omit an empty array.
        skipped: skipped.into_iter().collect(),
        sessions,
    }
}

struct Differ<'a> {
    skipped: &'a BTreeSet<Section>,
    compatible: bool,
    sessions: Vec<SessionDiff>,
}

fn children(session: &Session) -> Vec<(&str, &Session)> {
    session
        .invocations
        .iter()
        .flatten()
        .map(|invocation| (invocation.invoke_id.as_str(), invocation.session.as_ref()))
        .collect()
}

impl Differ<'_> {
    fn root_and_invoked(&mut self, before: &Session, after: &Session) {
        self.subtree(&["root".to_owned()], before, after);
    }

    /// The session at `path`, then the sessions it invoked (recursively,
    /// in the after document's order), then the ones it no longer holds.
    fn subtree(&mut self, path: &[String], before: &Session, after: &Session) {
        self.session(path.to_vec(), before, after);
        if self.skipped.contains(&Section::Invocations) {
            return;
        }
        let before_children = children(before);
        let after_children = children(after);
        let child_path = |invoke_id: &str| {
            let mut child = path.to_vec();
            child.push(invoke_id.to_owned());
            child
        };
        for (invoke_id, child) in &after_children {
            match before_children.iter().find(|(id, _)| id == invoke_id) {
                Some((_, previous)) => self.subtree(&child_path(invoke_id), previous, child),
                None => self
                    .sessions
                    .push(SessionDiff::new(child_path(invoke_id), Change::Added)),
            }
        }
        for (invoke_id, _) in &before_children {
            if !after_children.iter().any(|(id, _)| id == invoke_id) {
                self.sessions
                    .push(SessionDiff::new(child_path(invoke_id), Change::Removed));
            }
        }
    }

    fn skip(&self, section: Section) -> bool {
        self.skipped.contains(&section)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one linear comparison of the session keys in document order"
    )]
    fn session(&mut self, path: Vec<String>, before: &Session, after: &Session) {
        let mut diff = SessionDiff::new(path, Change::Changed);
        let mut reordered = Vec::new();
        diff.session_id = changed(&before.session_id, &after.session_id);
        if before.name != after.name {
            diff.name = Some(OptionalChange {
                from: before.name.clone(),
                to: after.name.clone(),
            });
        }
        if before.datamodel_name != after.datamodel_name {
            diff.datamodel_name = Some(OptionalChange {
                from: before.datamodel_name.clone(),
                to: after.datamodel_name.clone(),
            });
        }
        diff.ir = changed(&before.ir, &after.ir);
        if diff.ir.is_some() {
            self.compatible = false;
        }
        if before.status != after.status {
            diff.status = Some(StatusChange {
                from: before.status,
                to: after.status,
            });
        }
        if before.invoked_by != after.invoked_by {
            diff.invoked_by = Some(OptionalChange {
                from: before.invoked_by.clone(),
                to: after.invoked_by.clone(),
            });
        }
        diff.configuration = configuration(&before.configuration, &after.configuration);
        if diff.configuration.is_none() && before.configuration != after.configuration {
            reordered.push(SessionList::Configuration);
        }
        diff.bound_states = set_diff(&before.bound_states, &after.bound_states).map(
            |StateSetDiff { exited, entered }| BoundStatesDiff {
                added: entered,
                removed: exited,
            },
        );
        if diff.bound_states.is_none() && before.bound_states != after.bound_states {
            reordered.push(SessionList::BoundStates);
        }
        if !self.skip(Section::System) {
            diff.system = system(before, after);
        }
        if !self.skip(Section::DataModel) {
            diff.data_model = data_model_changes(before, after);
            if diff.data_model.is_none() && before.data_model != after.data_model {
                reordered.push(SessionList::DataModel);
            }
        }
        diff.history = history(before, after);
        if diff.history.is_none() && before.history != after.history {
            reordered.push(SessionList::History);
        }
        let counters = CountersDiff {
            send: counter(before.counters.send, after.counters.send),
            invoke: counter(before.counters.invoke, after.counters.invoke),
        };
        if counters != CountersDiff::default() {
            diff.counters = Some(counters);
        }
        if !self.skip(Section::Queues)
            && let (Some(before_queues), Some(after_queues)) = (&before.queues, &after.queues)
        {
            let queues = QueuesDiff {
                internal: fifo(&before_queues.internal, &after_queues.internal),
                external: fifo(&before_queues.external, &after_queues.external),
            };
            if !queues.is_empty() {
                diff.queues = Some(queues);
            }
        }
        if !self.skip(Section::PendingSends)
            && let (Some(before_sends), Some(after_sends)) =
                (&before.pending_sends, &after.pending_sends)
        {
            let added: Vec<PendingSend> = after_sends
                .iter()
                .filter(|send| !before_sends.contains(send))
                .cloned()
                .collect();
            let removed: Vec<String> = before_sends
                .iter()
                .filter(|send| !after_sends.contains(send))
                .map(|send| send.sendid.clone())
                .collect();
            if !added.is_empty() || !removed.is_empty() {
                diff.pending_sends = Some(PendingSendsDiff { added, removed });
            } else if before_sends != after_sends {
                reordered.push(SessionList::PendingSends);
            }
        }
        if !self.skip(Section::Invocations) {
            let metadata = |session: &Session| {
                session
                    .invocations
                    .iter()
                    .flatten()
                    .map(|invocation| {
                        (
                            invocation.invoke_id.clone(),
                            InvocationMetadata::of(invocation),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let (before_list, after_list) = (metadata(before), metadata(after));
            let mut invocations = InvocationsDiff {
                added: Vec::new(),
                removed: Vec::new(),
                changed: None,
            };
            let mut changed_list = Vec::new();
            for change in keyed(&before_list, &after_list, |key, change, from, to| {
                (key, change, from, to)
            })
            .into_iter()
            .flatten()
            {
                match change {
                    (invoke_id, Change::Added, _, _) => invocations.added.push(invoke_id),
                    (invoke_id, Change::Removed, _, _) => invocations.removed.push(invoke_id),
                    (invoke_id, Change::Changed, Some(from), Some(to)) => {
                        changed_list.push(InvocationChange {
                            invoke_id,
                            from,
                            to,
                        });
                    }
                    (_, Change::Changed, _, _) => {}
                }
            }
            if !changed_list.is_empty() {
                invocations.changed = Some(changed_list);
            }
            if !invocations.added.is_empty()
                || !invocations.removed.is_empty()
                || invocations.changed.is_some()
            {
                diff.invocations = Some(invocations);
            } else if before_list != after_list {
                reordered.push(SessionList::Invocations);
            }
        }
        if !self.skip(Section::Transitions)
            && let (Some(before_transitions), Some(after_transitions)) =
                (&before.transitions, &after.transitions)
        {
            let previews = !self.skip(Section::Previews);
            let listing = |listed: &[crate::Transition]| {
                listed
                    .iter()
                    .map(|transition| {
                        let mut transition = transition.clone();
                        if !previews {
                            transition.exit = None;
                            transition.entry = None;
                        }
                        (
                            TransitionKey {
                                source: transition.source.clone(),
                                index: transition.index,
                            },
                            transition,
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let (before_list, after_list) =
                (listing(before_transitions), listing(after_transitions));
            let mut transitions = TransitionsDiff {
                added: Vec::new(),
                removed: Vec::new(),
                changed: None,
            };
            let mut changed_list = Vec::new();
            for change in keyed(&before_list, &after_list, |key, change, from, to| {
                (key, change, from, to)
            })
            .into_iter()
            .flatten()
            {
                match change {
                    (key, Change::Added, _, _) => transitions.added.push(key),
                    (key, Change::Removed, _, _) => transitions.removed.push(key),
                    (key, Change::Changed, Some(from), Some(to)) => {
                        changed_list.push(TransitionChange {
                            source: key.source,
                            index: key.index,
                            from,
                            to,
                        });
                    }
                    (_, Change::Changed, _, _) => {}
                }
            }
            if !changed_list.is_empty() {
                transitions.changed = Some(changed_list);
            }
            let listed = !transitions.added.is_empty()
                || !transitions.removed.is_empty()
                || transitions.changed.is_some();
            if listed {
                diff.transitions = Some(transitions);
            } else if before_list != after_list {
                reordered.push(SessionList::Transitions);
            }
        }
        if !reordered.is_empty() {
            diff.reordered = Some(reordered);
        }
        if diff.has_changes() {
            self.sessions.push(diff);
        }
    }
}

fn changed<T: Clone + PartialEq>(before: &T, after: &T) -> Option<ValueChange<T>> {
    (before != after).then(|| ValueChange {
        from: before.clone(),
        to: after.clone(),
    })
}

fn set_diff(before: &[String], after: &[String]) -> Option<StateSetDiff> {
    let before_set: BTreeSet<&String> = before.iter().collect();
    let after_set: BTreeSet<&String> = after.iter().collect();
    let exited: Vec<String> = before
        .iter()
        .filter(|id| !after_set.contains(id))
        .cloned()
        .collect();
    let entered: Vec<String> = after
        .iter()
        .filter(|id| !before_set.contains(id))
        .cloned()
        .collect();
    (!exited.is_empty() || !entered.is_empty()).then_some(StateSetDiff { exited, entered })
}

fn configuration(before: &Configuration, after: &Configuration) -> Option<ConfigurationDiff> {
    let full = set_diff(&before.full, &after.full);
    let StateSetDiff { exited, entered } = full.unwrap_or(StateSetDiff {
        exited: Vec::new(),
        entered: Vec::new(),
    });
    // `atomic` is a subset of `full`, so an atomic state that left or
    // joined `full` is already in `exited`/`entered`. Report `atomic` only
    // when it holds a change they do not: a state that stayed active but
    // stopped or started being listed as atomic.
    let atomic = set_diff(&before.atomic, &after.atomic).filter(|atomic| {
        atomic.exited.iter().any(|id| !exited.contains(id))
            || atomic.entered.iter().any(|id| !entered.contains(id))
    });
    if exited.is_empty() && entered.is_empty() && atomic.is_none() {
        return None;
    }
    Some(ConfigurationDiff {
        exited,
        entered,
        atomic,
    })
}

const fn counter(from: u64, to: u64) -> Option<CounterChange> {
    if from == to {
        None
    } else {
        Some(CounterChange { from, to })
    }
}

/// `consumed` is the smallest k such that `before[k..]` is a prefix of
/// `after`; `appended` is the rest of `after`.
fn fifo<T: Clone + PartialEq>(before: &[T], after: &[T]) -> Option<FifoDiff<T>> {
    if before == after {
        return None;
    }
    (0..=before.len())
        .find(|&k| after.starts_with(&before[k..]))
        .map(|k| FifoDiff {
            consumed: k as u64,
            appended: after[before.len() - k..].to_vec(),
        })
}

/// Keyed changes: changed and added in `after` order, then removed in
/// `before` order.
fn keyed<K: Ord + Clone, V: Clone + PartialEq, C>(
    before: &[(K, V)],
    after: &[(K, V)],
    make: impl Fn(K, Change, Option<V>, Option<V>) -> C,
) -> Option<Vec<C>> {
    let before_map: BTreeMap<&K, &V> = before.iter().map(|(key, value)| (key, value)).collect();
    let after_keys: BTreeSet<&K> = after.iter().map(|(key, _)| key).collect();
    let mut changes = Vec::new();
    for (key, value) in after {
        match before_map.get(key) {
            Some(previous) if *previous == value => {}
            Some(previous) => changes.push(make(
                key.clone(),
                Change::Changed,
                Some((*previous).clone()),
                Some(value.clone()),
            )),
            None => changes.push(make(key.clone(), Change::Added, None, Some(value.clone()))),
        }
    }
    for (key, value) in before {
        if !after_keys.contains(key) {
            changes.push(make(
                key.clone(),
                Change::Removed,
                Some(value.clone()),
                None,
            ));
        }
    }
    (!changes.is_empty()).then_some(changes)
}

fn system(before: &Session, after: &Session) -> Option<Vec<SystemChange>> {
    let entries = |session: &Session| {
        session
            .system
            .as_ref()
            .map(|system| {
                vec![
                    (0_u8, Value::Text(system.sessionid.clone())),
                    (1, system.name.clone().map_or(Value::Null, Value::Text)),
                    (2, system.ioprocessors.clone()),
                ]
            })
            .unwrap_or_default()
    };
    keyed(
        &entries(before),
        &entries(after),
        |key, change, from, to| SystemChange {
            name: match key {
                0 => SystemKey::Sessionid,
                1 => SystemKey::Name,
                _ => SystemKey::Ioprocessors,
            },
            change,
            from,
            to,
        },
    )
}

fn data_model_changes(before: &Session, after: &Session) -> Option<Vec<KeyedChange>> {
    let entries = |session: &Session| {
        session
            .data_model
            .iter()
            .flatten()
            .map(|variable| (variable.name.clone(), variable.value.clone()))
            .collect::<Vec<_>>()
    };
    keyed(
        &entries(before),
        &entries(after),
        |name, change, from, to| KeyedChange {
            name,
            change,
            from,
            to,
        },
    )
}

fn history(before: &Session, after: &Session) -> Option<Vec<HistoryChange>> {
    let entries = |session: &Session| {
        session
            .history
            .iter()
            .map(|value| (value.state.clone(), value.value.clone()))
            .collect::<Vec<_>>()
    };
    keyed(
        &entries(before),
        &entries(after),
        |state, change, from, to| HistoryChange {
            state,
            change,
            from,
            to,
        },
    )
}
