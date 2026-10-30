//! Canonical, read-only SCXML session snapshot document
//! `scxml.session-snapshot/v2` and its diff `scxml.session-snapshot-diff/v2`.
//!
//! A snapshot is an introspection image of a running root session and its invoked sessions, taken
//! only at a stable checkpoint, for tools and LLMs to read. It is never
//! restored: SCXML defines no persistence, and nothing in this workspace
//! saves, restores, or resumes session state.
//!
//! This crate is pure (no Wasmtime, no engine, no I/O). Hosts assemble a
//! [`Capture`] from the components' read-only `snapshot` exports and each
//! session's external event queue, then [`encode`] it; the CLI [`decode`]s, [`check`]s, and
//! [`diff`]s documents. The Wasmtime host and the CLI link it natively; the
//! optional `scxml-snapshot` codec component wraps only [`encode`] and
//! [`diff`] for the browser host.
//!
//! # Contract
//!
//! * Encoding: compact `serde_json`, UTF-8, no whitespace, no trailing
//!   newline; keys in the struct order of the types below (never sorted);
//!   collections are arrays with a stated order; absent optionals omitted
//!   and `null` never written; empty arrays written; integers unsigned and at
//!   most 2^53 − 1; no JSON floats (numbers are [`Value::Number`] strings).
//! * `sha256` is the SHA-256 of the canonical bytes of `snapshot` as they
//!   appear in the document. It detects corruption; it is not authentication.
//! * The decoder checks, in order: JSON, `version`, shape with unknown
//!   fields rejected, re-encode byte equality, `sha256`, bounds, and
//!   consistency. It is authoritative; [`SESSION_SNAPSHOT_SCHEMA`] and
//!   [`SESSION_SNAPSHOT_DIFF_SCHEMA`] cannot express key order,
//!   canonicality, or the digest.
//! * Errors carry a closed [`SnapshotErrorCode`] and an optional RFC 6901
//!   pointer built only from fixed keys and indices (at most
//!   [`MAX_POINTER_BYTES`]); no name, identifier, or payload is reflected.
//! * Documents may hold trace carriers (under
//!   [`TraceContextPolicy::Preserve`]) and data-model values: they receive
//!   the protections of canonical validator artifacts and are never logged
//!   or indexed.
//!
//! The crate emits no telemetry: its callers own the boundary spans (the
//! Wasmtime host's `session-snapshot` span, the codec component's `encode`
//! and `diff` observations, and the CLI's `snapshot` subcommand spans).

#![forbid(unsafe_code)]

use std::fmt;

use serde::{Deserialize, Serialize};

mod diff;
mod encode;
mod render;
mod validate;

pub use diff::{
    BoundStatesDiff, Change, ClockChange, ComponentChange, ConfigurationDiff, CounterChange,
    CountersDiff, DiffDocument, DocumentRef, FifoDiff, HistoryChange, InvocationChange,
    InvocationMetadata, InvocationsDiff, KeyedChange, OptionalChange, PendingSendsDiff, QueuesDiff,
    SessionDiff, SessionList, StateSetDiff, StatusChange, SystemChange, SystemKey,
    TransitionChange, TransitionKey, TransitionsDiff, ValueChange, diff, diff_documents,
};
pub use encode::encode;
pub use render::{Color, ELIDE_BYTES, render_diff, render_show};
pub use validate::{check, decode};

/// `version` of every session snapshot document.
pub const SESSION_SNAPSHOT_VERSION: &str = "scxml.session-snapshot/v2";
/// `version` of every session snapshot diff document.
pub const SESSION_SNAPSHOT_DIFF_VERSION: &str = "scxml.session-snapshot-diff/v2";
// The bounds below are part of the `scxml.session-snapshot/v2` format
// contract (its published JSON Schemas pin them), the same on every build.
// The format bounds no count of sessions, queue entries, pending sends,
// transitions, states, or variables, no invoke depth, no identifier length,
// and no document size.

/// Longest variable text a snapshot writes inline; a longer one is
/// [`VariableValue::Oversized`]. Event data and other values have no byte
/// bound (SCXML sets none).
pub const MAX_VALUE_BYTES: usize = 65_536;
/// Largest `event_schema` text (a canonical JSON schema or an authored
/// `event:schemaexpr`), in bytes: the Machine IR string bound.
pub const MAX_EVENT_SCHEMA_BYTES: usize = 1_048_576;
/// Largest error pointer, in bytes.
pub const MAX_POINTER_BYTES: usize = 256;

/// JSON Schema (Draft 2020-12) of `scxml.session-snapshot/v2`.
pub const SESSION_SNAPSHOT_SCHEMA: &str =
    include_str!("../schema/scxml.session-snapshot.v2.schema.json");
/// JSON Schema (Draft 2020-12) of `scxml.session-snapshot-diff/v2`.
pub const SESSION_SNAPSHOT_DIFF_SCHEMA: &str =
    include_str!("../schema/scxml.session-snapshot-diff.v2.schema.json");

/// A session section that `options.exclude` can omit.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    /// The SCXML 5.10 system variables.
    System,
    /// User data-model variables.
    DataModel,
    /// The Appendix D internal and external event queues.
    Queues,
    /// Pending delayed sends (cancelables).
    PendingSends,
    /// Transitions an event could trigger.
    Transitions,
    /// The `exit`/`entry` previews of each listed transition.
    Previews,
    /// Invoked sessions.
    Invocations,
}

/// Whether a snapshot holds the invoked sessions: the `invocations`
/// section is either kept or excluded by `options.exclude`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Children {
    /// Every session lists its `invocations`.
    Included,
    /// No session lists `invocations`.
    Excluded,
}

impl Children {
    /// The `invocations` section's presence under the `exclude` list.
    pub(crate) fn under(exclude: &[Section]) -> Self {
        if exclude.contains(&Section::Invocations) {
            Self::Excluded
        } else {
            Self::Included
        }
    }
}

/// Whether W3C trace carriers are kept in the document.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceContextPolicy {
    /// Carriers are omitted (the default everywhere except explicit Rust
    /// callers).
    #[default]
    Strip,
    /// Carriers appear as held.
    Preserve,
}

/// Where in a run a document was taken (`options.point`).
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotPoint {
    /// At a stable checkpoint (the default, omitted from the document):
    /// the internal queue is empty.
    #[default]
    Checkpoint,
    /// While one executable-content extension executes (SCXML 1.0 section
    /// 4.10), for the extension component that asked
    /// (`docs/session-snapshot.md`, "Snapshots during executable content"):
    /// one session, mid-macrostep, so its internal queue may hold events;
    /// `invocations` and `previews` are always excluded.
    #[serde(rename = "executable-content")]
    ExecutableContent,
}

impl SnapshotPoint {
    /// Whether this is the default [`SnapshotPoint::Checkpoint`].
    #[must_use]
    pub const fn is_checkpoint(&self) -> bool {
        matches!(self, Self::Checkpoint)
    }
}

/// How a document was produced (`options`).
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    /// Excluded sections, sorted and unique.
    pub exclude: Vec<Section>,
    /// Trace-carrier policy.
    pub trace_context: TraceContextPolicy,
    /// Where the document was taken; omitted for a checkpoint.
    #[serde(default, skip_serializing_if = "SnapshotPoint::is_checkpoint")]
    pub point: SnapshotPoint,
}

/// The whole document.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    /// Always [`SESSION_SNAPSHOT_VERSION`].
    pub version: String,
    /// SHA-256 (64 lowercase hex) of the canonical bytes of `snapshot`.
    pub sha256: String,
    /// How the document was produced.
    pub options: Options,
    /// The root session and every session it invoked, at any depth.
    pub snapshot: Snapshot,
}

/// Role of one component in the snapshot's provenance list.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComponentRole {
    /// The logical clock.
    Clock,
    /// The ECMAScript data model.
    DataModelEcmascript,
    /// The null data model.
    DataModelNull,
    /// The `XPath` data model.
    DataModelXpath,
    /// The SCXML Event I/O Processor.
    IoProcessorScxml,
    /// The generic machine.
    Machine,
}

/// Release-artifact identity of one component.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentIdentity {
    /// Component role.
    pub role: ComponentRole,
    /// SHA-256 of the released artifact.
    pub sha256: String,
}

/// The shared virtual clock position.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Clock {
    /// Logical milliseconds; every session clock equals it.
    pub now_ms: u64,
}

/// `snapshot`: provenance, clock, and the sessions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    /// Components, sorted by role and unique.
    pub components: Vec<ComponentIdentity>,
    /// The shared virtual clock.
    pub clock: Clock,
    /// Pre-order: the root session first, then each session it invoked
    /// (in its `invocations` order) followed by the sessions that one
    /// invoked, at any depth. A session is linked to its invocation by
    /// `invoked_by`; the list is flat, so no depth nests the document.
    pub sessions: Vec<Session>,
}

impl Snapshot {
    /// The root session (the first of [`Self::sessions`]; a decoded
    /// document always has one).
    #[must_use]
    pub fn root(&self) -> Option<&Session> {
        self.sessions.first()
    }

    /// For every session (by index into [`Self::sessions`]), the sessions
    /// it invoked, in its `invocations` order, each with its invoke id; an
    /// invocation with no session in the document is left out.
    #[must_use]
    pub fn invoked(&self) -> Vec<Vec<(&str, usize)>> {
        let by_link: std::collections::BTreeMap<(&str, &str), usize> = self
            .sessions
            .iter()
            .enumerate()
            .filter_map(|(index, session)| {
                session.invoked_by.as_ref().map(|by| {
                    (
                        (by.parent_session_id.as_str(), by.invoke_id.as_str()),
                        index,
                    )
                })
            })
            .collect();
        self.sessions
            .iter()
            .map(|parent| {
                parent
                    .invocations
                    .iter()
                    .flatten()
                    .filter_map(|invocation| {
                        by_link
                            .get(&(parent.session_id.as_str(), invocation.invoke_id.as_str()))
                            .map(|child| (invocation.invoke_id.as_str(), *child))
                    })
                    .collect()
            })
            .collect()
    }

    /// For every session (by index into [`Self::sessions`]), the invoke ids
    /// from the root session down to it (empty for the root, and for a
    /// session no invocation reaches).
    #[must_use]
    pub fn paths(&self) -> Vec<Vec<&str>> {
        let invoked = self.invoked();
        let mut paths: Vec<Vec<&str>> = vec![Vec::new(); self.sessions.len()];
        // Pre-order: a session's path is known before its children's.
        for (index, children) in invoked.iter().enumerate() {
            for (invoke_id, child) in children {
                if *child > index {
                    let mut path = paths.get(index).cloned().unwrap_or_default();
                    path.push(invoke_id);
                    if let Some(slot) = paths.get_mut(*child) {
                        *slot = path;
                    }
                }
            }
        }
        paths
    }
}

/// Session lifecycle at a checkpoint (no `failed` or `shut-down` session is
/// ever snapshotted).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Constructed, not started.
    Ready,
    /// Started, not in a top-level final state.
    Running,
    /// A top-level final state was reached.
    Finished,
}

/// Digest of the exact Machine IR bytes a session loaded.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IrIdentity {
    /// Machine IR schema version.
    pub version: u32,
    /// SHA-256 of the IR bytes.
    pub sha256: String,
}

/// The invoking parent of a invoked session (SCXML 6.4).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InvokedBy {
    /// The parent's `_sessionid`.
    pub parent_session_id: String,
    /// The invoke id the parent assigned.
    pub invoke_id: String,
}

/// Active configuration (SCXML 3.11), both lists in document order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    /// Active atomic/final states, one per parallel region.
    pub atomic: Vec<String>,
    /// Every active state, ancestors included.
    pub full: Vec<String>,
}

/// A recorded history value (SCXML 3.10).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryValue {
    /// The history pseudostate.
    pub state: String,
    /// Recorded states in document order.
    pub value: Vec<String>,
}

/// Next generated send-id and invoke-id ordinals.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Counters {
    /// Next `sendid_<n>` ordinal.
    pub send: u64,
    /// Next generated invoke-id ordinal.
    pub invoke: u64,
}

/// A portable data value: `{"type":T,"value":V?}`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub enum Value {
    /// No value.
    Absent,
    /// ECMAScript `undefined`.
    Undefined,
    /// `null`.
    Null,
    /// A boolean.
    Boolean(bool),
    /// A finite number as Rust `f64` `Display` text (no exponent, `-0`).
    Number(String),
    /// A string.
    Text(String),
    /// Canonical JSON array text.
    JsonArray(String),
    /// Canonical JSON object text.
    JsonObject(String),
    /// XML text.
    Xml(String),
}

impl Value {
    /// A number as its canonical text: Rust `f64` `Display` (shortest round
    /// trip, no exponent, `-0` kept). A non-finite value yields text the
    /// decoder rejects as non-canonical.
    #[must_use]
    pub fn number(value: f64) -> Self {
        Self::Number(value.to_string())
    }
}

/// A declared variable's value: a portable value, the type and string form
/// of a value that has none, or only the size of a value too long to show.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum VariableValue {
    /// A portable data value.
    Value(Value),
    /// A value with no portable form, by its type and string form.
    Text(TextValue),
    /// A value whose text or string form is longer than [`MAX_VALUE_BYTES`].
    Oversized(OversizedValue),
}

/// The `type` tag of an [`OversizedValue`].
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OversizedTag {
    /// `"oversized"`.
    #[default]
    Oversized,
}

/// `{"type":"oversized","bytes":N}`: a variable whose rendered text (its
/// data-value text, or its string form) is longer than
/// [`MAX_VALUE_BYTES`]. The data-model provider reports only the UTF-8 byte
/// length, so nothing is truncated and a variable's size never makes a
/// snapshot fail.
/// `bytes` is always greater than [`MAX_VALUE_BYTES`]; a shorter value is
/// written inline.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OversizedValue {
    /// Always `"oversized"`.
    #[serde(rename = "type")]
    pub tag: OversizedTag,
    /// The UTF-8 byte length of the value's text or string form.
    pub bytes: u64,
}

impl OversizedValue {
    /// An oversized value of `bytes` UTF-8 bytes.
    #[must_use]
    pub const fn new(bytes: u64) -> Self {
        Self {
            tag: OversizedTag::Oversized,
            bytes,
        }
    }
}

/// Most bytes of a [`TextValue`]'s `typeof`.
pub const MAX_TYPEOF_BYTES: usize = 64;

/// The `type` tag of a [`TextValue`].
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TextTag {
    /// `"text"`.
    #[default]
    Text,
}

/// `{"type":"text","typeof":T,"text":S}`: a value with no portable form
/// (an ECMAScript function, `Date`, cyclic object, `NaN`, ...), by its type
/// in the data model's language (ECMAScript: its `typeof`) and its string
/// form (ECMAScript: a function's source text, a `Date`'s `toString`,
/// `[object Tag]` for any other object, a primitive's `ToString`).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextValue {
    /// Always `"text"`.
    #[serde(rename = "type")]
    pub tag: TextTag,
    /// The value's type (ECMAScript `typeof`), at most
    /// [`MAX_TYPEOF_BYTES`] bytes.
    #[serde(rename = "typeof")]
    pub kind: String,
    /// The value's string form.
    pub text: String,
}

impl TextValue {
    /// A value of type `kind` whose string form is `text`.
    #[must_use]
    pub fn new(kind: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            tag: TextTag::Text,
            kind: kind.into(),
            text: text.into(),
        }
    }
}

/// One user variable.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Variable {
    /// The declared `<data>` id.
    pub name: String,
    /// Its current value.
    pub value: VariableValue,
}

/// The SCXML 5.10 system variables.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct System {
    /// `_sessionid`.
    pub sessionid: String,
    /// `_name`: the document `name`; absent when the document has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `_ioprocessors`.
    pub ioprocessors: Value,
}

/// SCXML 5.10.1 event type.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    /// Raised by the machine.
    Internal,
    /// Delivered by an Event I/O Processor.
    External,
    /// Raised by the platform.
    Platform,
}

/// Whether W3C baggage crosses the boundary it is attached to.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BaggagePolicy {
    /// Baggage is dropped.
    Drop,
    /// Baggage propagates.
    Propagate,
}

/// A held W3C trace carrier (present only under
/// [`TraceContextPolicy::Preserve`]).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TraceContext {
    /// W3C `traceparent`.
    pub traceparent: String,
    /// W3C `tracestate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracestate: Option<String>,
    /// W3C baggage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baggage: Option<String>,
    /// Baggage policy.
    pub baggage_policy: BaggagePolicy,
}

/// One SCXML event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    /// Event name.
    pub name: String,
    /// Event type.
    #[serde(rename = "type")]
    pub event_type: EventType,
    /// `origin`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// `origintype`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origintype: Option<String>,
    /// `sendid`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sendid: Option<String>,
    /// `invokeid`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invokeid: Option<String>,
    /// `data`.
    pub data: Value,
    /// `raw`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    /// The held carrier, only under [`TraceContextPolicy::Preserve`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_context: Option<TraceContext>,
}

/// The session's SCXML Appendix D queues; each a FIFO.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Queues {
    /// The internal event queue (`[]` at every checkpoint).
    pub internal: Vec<Event>,
    /// The external event queue: events sent to the session, including
    /// those from the sessions it invoked (stamped with their `invokeid`)
    /// and their `done.invoke.<id>`, in arrival order.
    pub external: Vec<Event>,
}

/// One pending delayed send (SCXML 6.2, 6.3, C.1, C.2): exactly one of
/// `event` (a send of the SCXML Event I/O Processor) and `message` (a send
/// of another Event I/O Processor, such as `BasicHTTP`, which `type`
/// names).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PendingSend {
    /// The send id (clock timer id).
    pub sendid: String,
    /// The event an SCXML Event I/O Processor send delivers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<Event>,
    /// The message another Event I/O Processor's send carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<ProcessorMessage>,
    /// Authored target, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Authored processor type, if any.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub send_type: Option<String>,
    /// The authored delay.
    pub delay_ms: u64,
    /// Absolute deadline on the virtual clock.
    pub deadline_ms: u64,
}

/// The message of one pending send of an Event I/O Processor other than the
/// SCXML one (SCXML 6.2, C.2), as the machine evaluated it at the `<send>`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessorMessage {
    /// The evaluated `event`/`eventexpr` (a `BasicHTTP` send posts it as
    /// `_scxmleventname`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    /// `namelist` entries then `<param>` children, in document order.
    pub params: Vec<Param>,
    /// The evaluated `<content>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    /// The held carrier, only under [`TraceContextPolicy::Preserve`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_context: Option<TraceContext>,
}

/// One parameter of a pending processor send.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Param {
    /// Parameter name (not empty).
    pub name: String,
    /// Parameter value.
    pub value: Value,
}

/// SCXML 3.5 transition `type`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionType {
    /// `external`.
    External,
    /// `internal`.
    Internal,
}

/// A transition an event could trigger; its condition is never evaluated.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    /// Source state.
    pub source: String,
    /// Index among the source's transitions, document order.
    pub index: u32,
    /// Event descriptors (never empty).
    pub events: Vec<String>,
    /// Targets.
    pub targets: Vec<String>,
    /// Transition type.
    #[serde(rename = "type")]
    pub transition_type: TransitionType,
    /// Authored `cond` text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cond: Option<String>,
    /// `true` exactly when `cond` is present.
    pub has_cond: bool,
    /// SCXML 3.13 transition domain; absent when targetless or `<scxml>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Exit set of this transition alone, exit order (`previews`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<Vec<String>>,
    /// Entry set of this transition alone, entry order (`previews`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<Vec<String>>,
    /// The schema of the `_event.data` the transition handles (the `event`
    /// attribute vocabulary); absent when the transition has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_schema: Option<EventSchema>,
}

/// The `event` attribute vocabulary of one element
/// (`https://github.com/stateforward/scxml.rs/namespaces/event`), as the
/// Machine IR carries it. A snapshot only shows it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum EventSchema {
    /// `event:schema`, resolved at compile time.
    Schema {
        /// The schema as canonical JSON text.
        schema: String,
        /// SHA-256 of `schema`, 64 lowercase hex.
        sha256: String,
    },
    /// `event:schemaexpr`: the expression exactly as authored, never
    /// evaluated (a snapshot evaluates no expression).
    Schemaexpr {
        /// The authored expression.
        expr: String,
    },
}

/// A started, not yet cancelled invocation and its invoked session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Invocation {
    /// Invoke id.
    pub invoke_id: String,
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

/// One session object; excluded sections are absent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    /// `_sessionid`.
    pub session_id: String,
    /// `_name`: the document `name`; absent when the document has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Linked data model, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datamodel_name: Option<String>,
    /// Loaded IR identity.
    pub ir: IrIdentity,
    /// Lifecycle.
    pub status: Status,
    /// The invoking parent (children only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invoked_by: Option<InvokedBy>,
    /// Active configuration.
    pub configuration: Configuration,
    /// Recorded history values, document order of the history state.
    pub history: Vec<HistoryValue>,
    /// Late-binding states already bound (SCXML 5.3), document order.
    pub bound_states: Vec<String>,
    /// Id counters.
    pub counters: Counters,
    /// `system` section.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<System>,
    /// `data_model` section, provider order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_model: Option<Vec<Variable>>,
    /// `queues` section.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queues: Option<Queues>,
    /// `pending_sends` section, clock order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_sends: Option<Vec<PendingSend>>,
    /// `transitions` section: every transition an event could trigger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transitions: Option<Vec<Transition>>,
    /// `invocations` section, ascending invoke-id order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocations: Option<Vec<Invocation>>,
}

/// A started invocation as the machine reports it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvocationCapture {
    /// Invoke id.
    pub invoke_id: String,
    /// Invoking state.
    pub state: String,
    /// Index among the state's invokes.
    pub index: u32,
    /// Invoke type URI.
    pub type_uri: String,
    /// `autoforward`.
    pub autoforward: bool,
}

/// A triggerable transition as the machine reports it (WIT
/// `transition-snapshot`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransitionCapture {
    /// Source state.
    pub source: String,
    /// Index among the source's transitions.
    pub index: u32,
    /// Event descriptors.
    pub events: Vec<String>,
    /// Targets.
    pub targets: Vec<String>,
    /// Transition type.
    pub transition_type: TransitionType,
    /// Authored `cond` text.
    pub cond: Option<String>,
    /// Transition domain.
    pub domain: Option<String>,
    /// Exit preview.
    pub exit: Vec<String>,
    /// Entry preview.
    pub entry: Vec<String>,
    /// The transition's `event` vocabulary, as the Machine IR carries it.
    pub event_schema: Option<EventSchema>,
}

/// The machine's read-only image (WIT `machine-snapshot`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MachineCapture {
    /// Lifecycle. A host maps a `failed` or `shut-down` machine to
    /// [`SnapshotErrorCode::StatusInvalid`] itself and never encodes it.
    pub status: Status,
    /// `_name`: the document `name`; `None` when the document has none.
    pub name: Option<String>,
    /// Active atomic/final states, document order.
    pub atomic: Vec<String>,
    /// Full configuration, document order.
    pub configuration: Vec<String>,
    /// Internal queue.
    pub internal: Vec<Event>,
    /// History values.
    pub history: Vec<HistoryValue>,
    /// Bound late-binding states.
    pub bound_states: Vec<String>,
    /// Live invocations (the encoder lists them in ascending invoke-id
    /// order).
    pub invocations: Vec<InvocationCapture>,
    /// The invoking parent, for a child.
    pub invoked_by: Option<InvokedBy>,
    /// Next send-id ordinal.
    pub send_counter: u64,
    /// Next invoke-id ordinal.
    pub invoke_counter: u64,
    /// Triggerable transitions.
    pub transitions: Vec<TransitionCapture>,
}

/// The data model's read-only image (WIT `data-model-snapshot`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataModelCapture {
    /// System variables.
    pub system: System,
    /// User variables, provider order.
    pub variables: Vec<Variable>,
}

/// One session's gathered records (WIT `session-capture`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionCapture {
    /// Index of the parent in [`Capture::sessions`]; `None` for the root.
    pub parent: Option<u32>,
    /// `_sessionid`.
    pub session_id: String,
    /// SHA-256 of the IR bytes the session loaded.
    pub ir_sha256: String,
    /// Linked data model, if any.
    pub data_model_name: Option<String>,
    /// Machine image.
    pub machine: MachineCapture,
    /// Data-model image, unless both of its sections are excluded.
    pub data_model: Option<DataModelCapture>,
    /// Pending delayed sends, clock order.
    pub pending_sends: Vec<PendingSend>,
    /// The external event queue, oldest first.
    pub external: Vec<Event>,
    /// Whether an invoked session reached a top-level final state (always
    /// false for the root).
    pub finished: bool,
}

/// Everything a host gathered at one checkpoint (WIT `capture`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Capture {
    /// Component provenance.
    pub components: Vec<ComponentIdentity>,
    /// The shared virtual clock.
    pub now_ms: u64,
    /// Pre-order: the root first, each session after the session that
    /// invoked it, at any depth;
    /// a capture past them fails to encode.
    pub sessions: Vec<SessionCapture>,
}

/// Options of [`diff`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DiffOptions {
    /// Compare trace carriers too (ignored by default).
    pub include_trace_context: bool,
}

/// Closed failure class of the codec.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotErrorCode {
    /// Not JSON.
    MalformedJson,
    /// Another `version`.
    UnsupportedVersion,
    /// Wrong shape or an unknown field.
    SchemaViolation,
    /// Re-encoding differs from the input bytes.
    NonCanonical,
    /// `sha256` does not match.
    IntegrityMismatch,
    /// A documented bound was exceeded.
    BoundsExceeded,
    /// An identifier is not a valid name.
    InvalidName,
    /// A derived field disagrees, an id repeats, or a parent link is bad.
    Inconsistent,
    /// A child's clock differs from the shared clock.
    ClockInvariant,
    /// Taken outside a checkpoint.
    NotStable,
    /// A `failed` or `shut-down` session.
    StatusInvalid,
    /// The codec is not available (the browser without its component).
    CodecUnavailable,
}

impl SnapshotErrorCode {
    /// The stable kebab-case code (`SNAP-<code>` in rendered diagnostics).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MalformedJson => "malformed-json",
            Self::UnsupportedVersion => "unsupported-version",
            Self::SchemaViolation => "schema-violation",
            Self::NonCanonical => "non-canonical",
            Self::IntegrityMismatch => "integrity-mismatch",
            Self::BoundsExceeded => "bounds-exceeded",
            Self::InvalidName => "invalid-name",
            Self::Inconsistent => "inconsistent",
            Self::ClockInvariant => "clock-invariant",
            Self::NotStable => "not-stable",
            Self::StatusInvalid => "status-invalid",
            Self::CodecUnavailable => "codec-unavailable",
        }
    }
}

/// A typed codec failure; it never reflects a name or payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotError {
    /// Closed failure class.
    pub code: SnapshotErrorCode,
    /// RFC 6901 pointer from fixed keys and indices only.
    pub pointer: Option<String>,
}

impl SnapshotError {
    /// An error of `code` at `pointer`.
    #[must_use]
    pub const fn new(code: SnapshotErrorCode, pointer: Option<String>) -> Self {
        Self { code, pointer }
    }
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "session snapshot {}", self.code.as_str())?;
        if let Some(pointer) = &self.pointer {
            write!(formatter, " at {pointer}")?;
        }
        Ok(())
    }
}

impl std::error::Error for SnapshotError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_are_draft_2020_12_documents_with_their_ids() {
        for (schema, id, version) in [
            (
                SESSION_SNAPSHOT_SCHEMA,
                "urn:stateforward:scxml:session-snapshot:v2",
                SESSION_SNAPSHOT_VERSION,
            ),
            (
                SESSION_SNAPSHOT_DIFF_SCHEMA,
                "urn:stateforward:scxml:session-snapshot-diff:v2",
                SESSION_SNAPSHOT_DIFF_VERSION,
            ),
        ] {
            let value: serde_json::Value =
                serde_json::from_str(schema).expect("the schema is JSON");
            assert_eq!(
                value["$schema"],
                "https://json-schema.org/draft/2020-12/schema"
            );
            assert_eq!(value["$id"], id);
            assert_eq!(value["properties"]["version"]["const"], version);
            assert_eq!(value["additionalProperties"], false);
        }
    }

    /// The schema location and value of every decoder bound JSON Schema
    /// can express.
    fn expressible_bounds() -> Vec<(String, usize)> {
        [
            (
                "/$defs/variableValue/oneOf/1/properties/text/maxLength",
                MAX_VALUE_BYTES,
            ),
            (
                "/$defs/variableValue/oneOf/1/properties/typeof/maxLength",
                MAX_TYPEOF_BYTES,
            ),
            (
                "/$defs/traceContext/properties/tracestate/maxLength",
                scxml_types::MAX_TRACESTATE_BYTES,
            ),
            (
                "/$defs/traceContext/properties/baggage/maxLength",
                scxml_types::MAX_BAGGAGE_BYTES,
            ),
            (
                "/$defs/variableValue/oneOf/2/properties/bytes/minimum",
                MAX_VALUE_BYTES + 1,
            ),
        ]
        .into_iter()
        .map(|(pointer, bound)| (pointer.to_owned(), bound))
        .collect()
    }

    /// Every decoder bound JSON Schema can express is in the schema, at the
    /// decoder's value (`maxLength` bounds characters, an upper bound on the
    /// decoder's byte limit).
    #[test]
    fn schema_bounds_equal_the_decoder_bounds() {
        let schema: serde_json::Value =
            serde_json::from_str(SESSION_SNAPSHOT_SCHEMA).expect("the schema is JSON");
        let bounds = expressible_bounds();
        for (pointer, bound) in bounds {
            assert_eq!(
                schema.pointer(&pointer).and_then(serde_json::Value::as_u64),
                Some(bound as u64),
                "{pointer}"
            );
        }
        assert_eq!(
            schema.pointer("/$defs/uint/maximum"),
            Some(&serde_json::json!(9_007_199_254_740_991_u64))
        );
        let diff: serde_json::Value =
            serde_json::from_str(SESSION_SNAPSHOT_DIFF_SCHEMA).expect("the schema is JSON");
        assert!(
            diff["required"]
                .as_array()
                .is_some_and(|required| required.contains(&"skipped".into())),
            "the differ always writes `skipped`"
        );
    }

    #[test]
    fn section_and_role_spellings_match_the_schema() {
        let schema: serde_json::Value =
            serde_json::from_str(SESSION_SNAPSHOT_SCHEMA).expect("the schema is JSON");
        let sections = serde_json::to_value([
            Section::System,
            Section::DataModel,
            Section::Queues,
            Section::PendingSends,
            Section::Transitions,
            Section::Previews,
            Section::Invocations,
        ])
        .expect("sections serialize");
        assert_eq!(schema["$defs"]["section"]["enum"], sections);
        let roles = serde_json::to_value([
            ComponentRole::Clock,
            ComponentRole::DataModelEcmascript,
            ComponentRole::DataModelNull,
            ComponentRole::DataModelXpath,
            ComponentRole::IoProcessorScxml,
            ComponentRole::Machine,
        ])
        .expect("roles serialize");
        assert_eq!(
            schema["$defs"]["componentIdentity"]["properties"]["role"]["enum"],
            roles
        );
    }
}
