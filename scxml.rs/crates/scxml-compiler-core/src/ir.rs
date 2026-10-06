use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};
use scxml_types::wasm_bound;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{LowerError, MACHINE_IR_VERSION};

// Machine IR bounds: every one applies to wasm builds only (the shipped
// compiler and machine components); a native compile lowers and encodes a
// document of any size. The `u32` index width is the format, not a bound.

/// Maximum accepted canonical IR byte length (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_IR_BYTES: usize = wasm_bound(16 * 1024 * 1024);
/// Maximum number of states or transitions in one machine (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_TABLE_ITEMS: usize = wasm_bound(100_000);
/// Maximum number of items in a nested collection (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_NESTED_ITEMS: usize = wasm_bound(10_000);
/// Maximum UTF-8 byte length of one scalar string (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_STRING_BYTES: usize = wasm_bound(1024 * 1024);
/// Maximum executable-content or inline-XML nesting depth (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_CONTENT_DEPTH: usize = wasm_bound(64);
/// Maximum aggregate target references subjected to multi-target validation
/// (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_MULTI_TARGET_VALIDATION_ITEMS: usize = wasm_bound(100_000);

/// SCXML state category preserved by lowering.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateKind {
    /// Leaf state with no child states.
    Atomic,
    /// State with one or more nested states.
    Compound,
    /// Parallel state whose child regions are entered together.
    Parallel,
    /// Terminal state.
    Final,
    /// Shallow history pseudostate.
    ShallowHistory,
    /// Deep history pseudostate.
    DeepHistory,
}

/// SCXML transition category.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransitionType {
    /// External transition.
    External,
    /// Internal transition.
    Internal,
}

/// Machine binding mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Binding {
    /// Data is bound before execution starts.
    Early,
    /// Data is bound when its state is first entered.
    Late,
}

/// One expanded XML name.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpandedNameIr {
    /// Exact namespace URI, or `None` for no namespace.
    pub namespace_uri: Option<String>,
    /// Local name.
    pub local_name: String,
}

/// One namespace-resolved XML attribute.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XmlAttributeIr {
    /// Expanded attribute name.
    pub name: ExpandedNameIr,
    /// Decoded attribute value.
    pub value: String,
}

/// External `<script src>` reference (SCXML 1.0 section 5.8).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptSourceIr {
    /// The `src` URI exactly as authored; never normalized.
    pub uri: String,
    /// Compile-time pin; absent when the host resolves the URI at load.
    pub pinned: Option<PinnedScriptIr>,
}

/// Compile-time identity of one external script, from a script package.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinnedScriptIr {
    /// Declared media type (`application/wasm` or a script language of the
    /// machine's data model).
    pub media_type: String,
    /// Lowercase hex SHA-256 of the exact script bytes.
    pub sha256: String,
    /// WIT world of a Wasm script component (`stateforward:scxml/script@0.2.0`);
    /// absent for data-model-language text.
    pub wit_world: Option<String>,
}

impl ScriptSourceIr {
    /// Whether the host must resolve this reference at document load: a
    /// pinned Wasm script component or an unpinned reference. Pinned
    /// data-model-language text is embedded in the IR instead.
    #[must_use]
    pub fn requires_host_load(&self) -> bool {
        self.pinned
            .as_ref()
            .is_none_or(|pinned| pinned.media_type == scxml_script::WASM_MEDIA_TYPE)
    }
}

/// Inline XML or executable content extension node.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum XmlNodeIr {
    /// Character data in document order.
    Text {
        /// Decoded text value.
        value: String,
    },
    /// Namespace-resolved element.
    Element {
        /// Expanded element name.
        name: ExpandedNameIr,
        /// Attributes in document order.
        attributes: Vec<XmlAttributeIr>,
        /// Children in document order.
        children: Vec<XmlNodeIr>,
    },
}

/// XML namespace bound to the reserved `xml` prefix.
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

/// Serializes inline IR nodes as a standalone, namespace-correct XML
/// fragment.
///
/// Each element restates its default namespace whenever it differs from the
/// inherited one (`xmlns=""` undeclares it), and namespaced attributes get
/// deterministic `ns<N>` prefixes declared on their element (`xml:` is
/// reserved). Text and attribute values are escaped by [`escape_markup`],
/// so the fragment reparses to exactly these nodes.
///
/// # Errors
///
/// [`LowerError::LimitExceeded`] when nesting exceeds [`MAX_CONTENT_DEPTH`]
/// or the output exceeds [`MAX_STRING_BYTES`].
pub fn xml_markup(nodes: &[XmlNodeIr]) -> Result<String, LowerError> {
    let mut out = String::new();
    write_markup(nodes, None, 0, &mut out)?;
    Ok(out)
}

/// Where escaped text lands in serialized inline XML.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MarkupContext {
    /// Element content.
    Text,
    /// A double-quoted attribute value.
    Attribute,
}

/// `text` escaped for `context`, so markup containing it reparses to
/// exactly `text`: `&`, `<`, `>`, and carriage return always (XML 1.0 2.11
/// would turn a literal CR into a line feed); in an attribute value also
/// `"` and the tab and line feed attribute-value normalization (XML 1.0
/// 3.3.3) would turn into spaces.
#[must_use]
pub fn escape_markup(text: &str, context: MarkupContext) -> Cow<'_, str> {
    let attribute = context == MarkupContext::Attribute;
    let replacement = |character: char| match character {
        '&' => Some("&amp;"),
        '<' => Some("&lt;"),
        '>' => Some("&gt;"),
        '"' if attribute => Some("&quot;"),
        '\t' if attribute => Some("&#9;"),
        '\n' if attribute => Some("&#10;"),
        '\r' => Some("&#13;"),
        _ => None,
    };
    if !text
        .chars()
        .any(|character| replacement(character).is_some())
    {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match replacement(character) {
            Some(entity) => out.push_str(entity),
            None => out.push(character),
        }
    }
    Cow::Owned(out)
}

fn write_markup(
    nodes: &[XmlNodeIr],
    default_namespace: Option<&str>,
    depth: usize,
    out: &mut String,
) -> Result<(), LowerError> {
    if scxml_types::exceeds_wasm_bound(depth, MAX_CONTENT_DEPTH) {
        return Err(LowerError::LimitExceeded {
            field: "inline XML depth",
            limit: MAX_CONTENT_DEPTH,
        });
    }
    let push = |out: &mut String, text: &str| {
        if scxml_types::exceeds_wasm_bound(out.len().saturating_add(text.len()), MAX_STRING_BYTES) {
            return Err(LowerError::LimitExceeded {
                field: "inline XML markup bytes",
                limit: MAX_STRING_BYTES,
            });
        }
        out.push_str(text);
        Ok(())
    };
    let escaped = |out: &mut String, text: &str, context: MarkupContext| {
        push(out, &escape_markup(text, context))
    };
    for node in nodes {
        match node {
            XmlNodeIr::Text { value } => escaped(out, value, MarkupContext::Text)?,
            XmlNodeIr::Element {
                name,
                attributes,
                children,
            } => {
                push(out, "<")?;
                push(out, &name.local_name)?;
                let namespace = name.namespace_uri.as_deref();
                if namespace != default_namespace {
                    push(out, " xmlns=\"")?;
                    escaped(out, namespace.unwrap_or_default(), MarkupContext::Attribute)?;
                    push(out, "\"")?;
                }
                let mut prefixes: Vec<&str> = Vec::new();
                for attribute in attributes {
                    let prefix = match attribute.name.namespace_uri.as_deref() {
                        None => None,
                        Some(XML_NAMESPACE) => Some("xml".to_owned()),
                        Some(uri) => {
                            let ordinal = if let Some(ordinal) =
                                prefixes.iter().position(|known| *known == uri)
                            {
                                ordinal
                            } else {
                                prefixes.push(uri);
                                let ordinal = prefixes.len().saturating_sub(1);
                                push(out, &format!(" xmlns:ns{ordinal}=\""))?;
                                escaped(out, uri, MarkupContext::Attribute)?;
                                push(out, "\"")?;
                                ordinal
                            };
                            Some(format!("ns{ordinal}"))
                        }
                    };
                    push(out, " ")?;
                    if let Some(prefix) = prefix {
                        push(out, &prefix)?;
                        push(out, ":")?;
                    }
                    push(out, &attribute.name.local_name)?;
                    push(out, "=\"")?;
                    escaped(out, &attribute.value, MarkupContext::Attribute)?;
                    push(out, "\"")?;
                }
                if children.is_empty() {
                    push(out, "/>")?;
                } else {
                    push(out, ">")?;
                    write_markup(children, namespace, depth.saturating_add(1), out)?;
                    push(out, "</")?;
                    push(out, &name.local_name)?;
                    push(out, ">")?;
                }
            }
        }
    }
    Ok(())
}

/// A parameter supplied to `<send>`, `<invoke>`, or `<donedata>`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamIr {
    /// Parameter name.
    pub name: String,
    /// Expression supplying the value, when present.
    pub expr: Option<String>,
    /// Location supplying the value, when present.
    pub location: Option<String>,
}

/// Inline `<content>` payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentIr {
    /// Expression supplying content, when present.
    pub expr: Option<String>,
    /// Inline XML/text content in document order.
    pub body: Vec<XmlNodeIr>,
}

/// One `elseif` branch.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElseIfIr {
    /// Branch condition expression.
    pub cond: String,
    /// Branch body in document order.
    pub actions: Vec<ExecutableContentIr>,
}

/// The `event` attribute vocabulary on one `<transition>`, `<send>`,
/// `<raise>`, or `<donedata>` ([`scxml_types::EVENT_NAMESPACE_URI`]): a JSON
/// Schema describing the event data the element handles or creates.
///
/// Metadata only: the engine never reads it while it executes a document,
/// so transition selection, `<send>`, `<raise>`, and `<donedata>` behave
/// exactly as without it. Hosts expose it to readers (the session
/// snapshot's transitions).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum EventSchemaIr {
    /// `event:schema`, resolved at compile time.
    Schema {
        /// The schema as canonical JSON text (sorted keys, no white space),
        /// with its references rebased or bundled so it stands alone.
        schema: String,
        /// Lowercase hex SHA-256 of `schema`'s UTF-8 bytes.
        sha256: String,
    },
    /// `event:schemaexpr`: an expression in the document's data model whose
    /// value is the schema, carried unevaluated.
    Schemaexpr {
        /// The expression exactly as authored.
        expr: String,
    },
}

/// Attributes and children of typed `<send>` executable content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendIr {
    /// Literal event name.
    pub event: Option<String>,
    /// Event expression.
    pub eventexpr: Option<String>,
    /// Literal target.
    pub target: Option<String>,
    /// Target expression.
    pub targetexpr: Option<String>,
    /// Literal I/O processor type.
    #[serde(rename = "type")]
    pub type_value: Option<String>,
    /// I/O processor type expression.
    pub typeexpr: Option<String>,
    /// Literal send identifier.
    pub id: Option<String>,
    /// Location receiving a generated send identifier.
    pub idlocation: Option<String>,
    /// Literal delay.
    pub delay: Option<String>,
    /// Delay expression.
    pub delayexpr: Option<String>,
    /// Space-separated data locations captured by the send.
    pub namelist: Vec<String>,
    /// Parameters in document order.
    pub params: Vec<ParamIr>,
    /// Optional content payload.
    pub content: Option<ContentIr>,
    /// The schema of the data the event is created with; absent when the
    /// element has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_schema: Option<EventSchemaIr>,
}

/// `XPath` data model `<assign type>` placement (SCXML 1.0 CR 2014-03-13 §C.3.6).
///
/// `replacechildren` is the default and is represented by an absent
/// `assign_type`; the other values are only meaningful to the `XPath` data
/// model, which places the value relative to the node the location selects.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssignTypeIr {
    /// Replace every child of the location node (the default).
    Replacechildren,
    /// Insert before the first child of the location node.
    Firstchild,
    /// Insert after the last child of the location node.
    Lastchild,
    /// Insert as the preceding sibling of the location node.
    Previoussibling,
    /// Insert as the following sibling of the location node.
    Nextsibling,
    /// Replace the location node itself.
    Replace,
    /// Delete the location node; no value is supplied.
    Delete,
    /// Add the attribute named by `attr` to the location node.
    Addattribute,
}

impl AssignTypeIr {
    /// Parses the exact SCXML attribute value; `None` for anything else.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "replacechildren" => Some(Self::Replacechildren),
            "firstchild" => Some(Self::Firstchild),
            "lastchild" => Some(Self::Lastchild),
            "previoussibling" => Some(Self::Previoussibling),
            "nextsibling" => Some(Self::Nextsibling),
            "replace" => Some(Self::Replace),
            "delete" => Some(Self::Delete),
            "addattribute" => Some(Self::Addattribute),
            _ => None,
        }
    }
}

/// Typed SCXML executable content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ExecutableContentIr {
    /// Raise an internal event.
    Raise {
        /// Raised event name.
        event: String,
        /// The schema of the event's data; absent when the element has none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_schema: Option<EventSchemaIr>,
    },
    /// Emit an implementation log record.
    Log {
        /// Optional label.
        label: Option<String>,
        /// Optional expression; never blank (a blank `expr` lowers as
        /// absent, so the record has no value).
        expr: Option<String>,
    },
    /// Assign an expression or inline markup content to a data location.
    Assign {
        /// Data-model location.
        location: String,
        /// Optional value expression.
        expr: Option<String>,
        /// Inline XML/text markup content in document order (SCXML 5.8:
        /// exactly one of `expr` and inline children is present). Empty when
        /// the assignment uses `expr`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        body: Vec<XmlNodeIr>,
        /// SCXML 1.0 CR 2014-03-13 §C.3.6 `type`: how the `XPath` data model places
        /// the value relative to the location node. Absent means the default
        /// `replacechildren`, the only placement other data models define.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[serde(rename = "type")]
        assign_type: Option<AssignTypeIr>,
        /// SCXML 1.0 CR 2014-03-13 §C.3.6 `attr`: the attribute name that
        /// `type="addattribute"` creates; present exactly for that type.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attr: Option<String>,
    },
    /// Conditional executable-content block.
    If {
        /// Initial condition.
        cond: String,
        /// Initial branch body.
        actions: Vec<ExecutableContentIr>,
        /// Subsequent conditional branches.
        elseif: Vec<ElseIfIr>,
        /// Optional final branch.
        else_actions: Option<Vec<ExecutableContentIr>>,
    },
    /// Iterate over a data-model collection.
    Foreach {
        /// Collection expression.
        array: String,
        /// Item location.
        item: String,
        /// Optional index location.
        index: Option<String>,
        /// Loop body.
        actions: Vec<ExecutableContentIr>,
    },
    /// Send an event through an I/O processor.
    Send {
        /// Boxed send fields keep this enum compact.
        value: Box<SendIr>,
    },
    /// Cancel a pending send.
    Cancel {
        /// Literal send identifier.
        sendid: Option<String>,
        /// Send identifier expression.
        sendidexpr: Option<String>,
    },
    /// Execute data-model script source (SCXML 1.0 section 5.8).
    ///
    /// Exactly one form: inline (`src` absent, `body` the inline text); an
    /// external script pinned as data-model-language text (`body` the
    /// fetched text, whose SHA-256 is the pin); or an external script the
    /// host resolves at document load (a pinned Wasm script component or an
    /// unpinned reference; `body` empty).
    Script {
        /// External script reference.
        src: Option<ScriptSourceIr>,
        /// Inline or pinned script text.
        body: String,
    },
    /// Namespace-defined executable content preserved without reinterpretation.
    Extension {
        /// Executable-content extension including all nested content.
        element: XmlNodeIr,
    },
}

/// One `<onentry>` or `<onexit>` block.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionBlockIr {
    /// Global source document-order ordinal.
    pub document_order: u32,
    /// Actions in document order.
    pub actions: Vec<ExecutableContentIr>,
}

/// Initial transition metadata for the document or a compound state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialIr {
    /// Global source document-order ordinal of an explicit `<initial>`, or the owner ordinal for an attribute/default initial.
    pub document_order: u32,
    /// Target state table indices.
    pub targets: Vec<u32>,
    /// Initial-transition executable content.
    pub actions: Vec<ExecutableContentIr>,
}

/// One data declaration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataIr {
    /// Global source document-order ordinal.
    pub document_order: u32,
    /// Data identifier.
    pub id: String,
    /// External data URI.
    pub src: Option<String>,
    /// Initializer expression.
    pub expr: Option<String>,
    /// Inline content in document order.
    pub content: Vec<XmlNodeIr>,
}

/// One invoke parameter/content/finalize child, retaining document order.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum InvokeChildIr {
    /// Invoke parameter.
    Param {
        /// Parameter value.
        value: ParamIr,
    },
    /// Invoke content payload.
    Content {
        /// Content value.
        value: ContentIr,
    },
    /// Finalization executable content.
    Finalize {
        /// Finalization actions.
        actions: Vec<ExecutableContentIr>,
    },
}

/// One invoked-service declaration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvokeIr {
    /// Global source document-order ordinal.
    pub document_order: u32,
    /// Literal invocation type.
    #[serde(rename = "type")]
    pub type_value: Option<String>,
    /// Invocation type expression.
    pub typeexpr: Option<String>,
    /// Literal source URI.
    pub src: Option<String>,
    /// Source expression.
    pub srcexpr: Option<String>,
    /// Literal invoke identifier.
    pub id: Option<String>,
    /// Location receiving a generated invoke identifier.
    pub idlocation: Option<String>,
    /// Space-separated data locations captured by the invocation.
    pub namelist: Vec<String>,
    /// Whether events are automatically forwarded.
    pub autoforward: bool,
    /// Parameters, content, and finalize blocks in document order.
    pub children: Vec<InvokeChildIr>,
    /// The `<invoke>`'s attributes in the `env` namespace
    /// ([`scxml_types::ENV_NAMESPACE_URI`]) as authored, sorted by local
    /// name: `env:<slot>="NAME"` names the variable bound to `<slot>`, and
    /// `env:<slot>expr="EXPR"` a value expression the machine evaluates to
    /// that name when the `<invoke>` runs; each slot once. Only on an invoked
    /// service's `<invoke>`. Omitted from the encoding when empty. Names are
    /// not secrets; no value ever appears in Machine IR.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<XmlAttributeIr>,
}

/// The `env` slot an `<invoke>` attribute in the `env` namespace binds, and
/// whether its value is an expression (`env:<slot>expr`) rather than a
/// variable name (`env:<slot>`).
#[must_use]
pub(crate) fn invoke_env_slot(local_name: &str) -> (&str, bool) {
    match local_name.strip_suffix("expr") {
        Some(slot) if !slot.is_empty() => (slot, true),
        _ => (local_name, false),
    }
}

/// Final-state result payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoneDataIr {
    /// Global source document-order ordinal.
    pub document_order: u32,
    /// Parameters and content in document order.
    pub children: Vec<DoneDataChildIr>,
    /// The schema of the `done.state.<id>` event's data; absent when the
    /// element has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_schema: Option<EventSchemaIr>,
}

/// One `<donedata>` child.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DoneDataChildIr {
    /// Result parameter.
    Param {
        /// Result parameter.
        value: ParamIr,
    },
    /// Result content.
    Content {
        /// Result content.
        value: ContentIr,
    },
}

/// Immutable lowered state row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateIr {
    /// Global source document-order ordinal.
    pub document_order: u32,
    /// State identifier. History states may omit it.
    pub id: Option<String>,
    /// State category.
    pub kind: StateKind,
    /// Parent state table index, or `None` for a top-level state.
    pub parent: Option<u32>,
    /// Direct child state indices in document order.
    pub children: Vec<u32>,
    /// Initial transition for compound states.
    pub initial: Option<InitialIr>,
    /// Entry blocks in document order.
    pub onentry: Vec<ActionBlockIr>,
    /// Exit blocks in document order.
    pub onexit: Vec<ActionBlockIr>,
    /// Outgoing transition indices in document order.
    pub transitions: Vec<u32>,
    /// State-scoped data declarations.
    pub data: Vec<DataIr>,
    /// State-scoped invoked services.
    pub invokes: Vec<InvokeIr>,
    /// Final-state result payload.
    pub donedata: Option<DoneDataIr>,
}

/// Immutable lowered transition row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionIr {
    /// Global source document-order ordinal.
    pub document_order: u32,
    /// Source state table index.
    pub source: u32,
    /// Target state table indices in source order. Empty means targetless.
    pub targets: Vec<u32>,
    /// Space-normalized event descriptors in source order. Empty means eventless.
    pub events: Vec<String>,
    /// Preserved condition expression; `None` means unconditional. A
    /// present but blank `cond` is preserved and evaluated: it is not a
    /// valid expression, so it raises `error.execution` and the condition
    /// is false (SCXML 3.13, 5.9).
    pub cond: Option<String>,
    /// Internal or external transition type.
    #[serde(rename = "type")]
    pub transition_type: TransitionType,
    /// Transition executable content.
    pub actions: Vec<ExecutableContentIr>,
    /// The schema of the `_event.data` the transition handles; absent when
    /// the element has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_schema: Option<EventSchemaIr>,
}

/// Deterministic portable machine representation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineIr {
    /// IR schema version.
    pub version: u32,
    /// Optional machine name.
    pub name: Option<String>,
    /// The data model the document uses: its `datamodel` attribute, or
    /// [`scxml_validator_core::DEFAULT_DATA_MODEL`] when it has none
    /// (lowering always records one; hand-built IR may leave it unset, and
    /// then no data model is linked).
    pub data_model: Option<String>,
    /// Data binding mode.
    pub binding: Binding,
    /// Root initial transition.
    pub initial: InitialIr,
    /// States in depth-first document order.
    pub states: Vec<StateIr>,
    /// Transitions in document order.
    pub transitions: Vec<TransitionIr>,
    /// Root data declarations.
    pub data: Vec<DataIr>,
    /// Root scripts in document order.
    pub scripts: Vec<ActionBlockIr>,
    /// Exact extension namespaces referenced by the source, sorted by URI.
    pub extension_namespaces: Vec<String>,
}

impl MachineIr {
    /// Validates all size, structural, and reference invariants without side effects.
    ///
    /// Multi-target validation builds one `O(states * log(states))` hierarchy
    /// index, admits at most [`MAX_MULTI_TARGET_VALIDATION_ITEMS`] aggregate
    /// target references, and checks `targets - 1` preorder adjacencies per set.
    ///
    /// # Errors
    ///
    /// Returns a typed failure before invalid IR can reach engine logic.
    pub fn validate(&self) -> Result<(), LowerError> {
        let span = observe(Module::CompilerCore, Operation::CompilerValidateIr);
        let result = self.validate_inner();
        if result.is_err() {
            span.fail(ErrorCategory::Validation);
        } else {
            span.event(SignalEvent::ArtifactAccepted);
        }
        result
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the single pass validates the complete cross-table Machine IR invariant"
    )]
    fn validate_inner(&self) -> Result<(), LowerError> {
        if self.version != MACHINE_IR_VERSION {
            return Err(LowerError::UnsupportedIrVersion(self.version));
        }
        bounded_count("states", self.states.len(), MAX_TABLE_ITEMS)?;
        bounded_count("transitions", self.transitions.len(), MAX_TABLE_ITEMS)?;
        bounded_count("root data", self.data.len(), MAX_NESTED_ITEMS)?;
        bounded_count("root scripts", self.scripts.len(), MAX_NESTED_ITEMS)?;
        bounded_count(
            "extension namespaces",
            self.extension_namespaces.len(),
            MAX_NESTED_ITEMS,
        )?;
        bounded_optional("machine name", self.name.as_deref())?;
        bounded_optional("data model", self.data_model.as_deref())?;
        if self.states.is_empty() {
            return Err(LowerError::NoStates);
        }
        validate_sorted_unique_strings("extension namespaces", &self.extension_namespaces)?;

        let mut ids = BTreeMap::new();
        let mut prior_order = None;
        for (index, state) in self.states.iter().enumerate() {
            let index_u32 = u32::try_from(index).map_err(|_| LowerError::TooManyStates)?;
            if prior_order.is_some_and(|prior| state.document_order <= prior) {
                return Err(LowerError::InvalidStructure(
                    "state document order must be strictly increasing",
                ));
            }
            prior_order = Some(state.document_order);
            if let Some(id) = state.id.as_deref() {
                checked_name("state id", id)?;
                if ids.insert(id, index_u32).is_some() {
                    return Err(LowerError::DuplicateStateId(crate::safe_value(id)));
                }
            } else if !matches!(
                state.kind,
                StateKind::ShallowHistory | StateKind::DeepHistory
            ) {
                return Err(LowerError::InvalidStructure(
                    "only history states may omit an id",
                ));
            }
            if let Some(parent) = state.parent
                && usize::try_from(parent).map_or(true, |value| value >= index)
            {
                return Err(LowerError::InvalidReference("state parent"));
            }
            validate_index_list("state children", &state.children, self.states.len(), true)?;
            validate_index_list(
                "state transitions",
                &state.transitions,
                self.transitions.len(),
                true,
            )?;
            bounded_count("onentry blocks", state.onentry.len(), MAX_NESTED_ITEMS)?;
            bounded_count("onexit blocks", state.onexit.len(), MAX_NESTED_ITEMS)?;
            bounded_count("state data", state.data.len(), MAX_NESTED_ITEMS)?;
            bounded_count("state invokes", state.invokes.len(), MAX_NESTED_ITEMS)?;
            validate_action_blocks(&state.onentry)?;
            validate_action_blocks(&state.onexit)?;
            validate_data(&state.data)?;
            validate_invokes(&state.invokes)?;
            if let Some(done_data) = &state.donedata {
                validate_done_data(done_data)?;
            }
            match state.kind {
                StateKind::Atomic => {
                    require_empty("atomic children", &state.children)?;
                    require_none("atomic initial", state.initial.as_ref())?;
                    require_none("atomic donedata", state.donedata.as_ref())?;
                }
                StateKind::Compound => {
                    if state.children.is_empty() || state.initial.is_none() {
                        return Err(LowerError::InvalidStructure(
                            "compound state requires children and an initial transition",
                        ));
                    }
                }
                // SCXML 3.4.2: the `<state>` children of a `<parallel>`
                // occur "0 or more times", so a parallel may have none.
                StateKind::Parallel => {
                    if state.initial.is_some() {
                        return Err(LowerError::InvalidStructure(
                            "parallel state forbids an initial transition",
                        ));
                    }
                }
                StateKind::Final => {
                    require_empty("final children", &state.children)?;
                    require_empty("final transitions", &state.transitions)?;
                    require_empty("final data", &state.data)?;
                    require_empty("final invokes", &state.invokes)?;
                    require_none("final initial", state.initial.as_ref())?;
                }
                StateKind::ShallowHistory | StateKind::DeepHistory => {
                    require_empty("history children", &state.children)?;
                    require_empty("history data", &state.data)?;
                    require_empty("history invokes", &state.invokes)?;
                    require_empty("history onentry", &state.onentry)?;
                    require_empty("history onexit", &state.onexit)?;
                    require_none("history initial", state.initial.as_ref())?;
                    require_none("history donedata", state.donedata.as_ref())?;
                    if state.parent.is_none() || state.transitions.len() > 1 {
                        return Err(LowerError::InvalidStructure(
                            "history requires a parent and at most one fallback transition",
                        ));
                    }
                }
            }
        }

        let mut expected_children = vec![Vec::new(); self.states.len()];
        for (index, state) in self.states.iter().enumerate() {
            if let Some(parent) = state.parent {
                let parent = usize::try_from(parent)
                    .map_err(|_| LowerError::InvalidReference("state parent"))?;
                expected_children[parent]
                    .push(u32::try_from(index).map_err(|_| LowerError::TooManyStates)?);
            }
        }
        for (state, expected) in self.states.iter().zip(expected_children) {
            if state.children != expected {
                return Err(LowerError::InvalidStructure(
                    "state children do not match parent references",
                ));
            }
        }

        validate_done_state_names(&self.states)?;

        let hierarchy = HierarchyIndex::new(&self.states)?;
        let mut multi_target_budget = MultiTargetBudget::new();
        validate_initial(
            &self.initial,
            None,
            &self.states,
            &hierarchy,
            &mut multi_target_budget,
        )?;
        for (index, state) in self.states.iter().enumerate() {
            if let Some(initial) = &state.initial {
                validate_initial(
                    initial,
                    Some(u32::try_from(index).map_err(|_| LowerError::TooManyStates)?),
                    &self.states,
                    &hierarchy,
                    &mut multi_target_budget,
                )?;
            }
        }

        let mut prior_transition_order = None;
        for (index, transition) in self.transitions.iter().enumerate() {
            if prior_transition_order.is_some_and(|prior| transition.document_order <= prior) {
                return Err(LowerError::InvalidStructure(
                    "transition document order must be strictly increasing",
                ));
            }
            prior_transition_order = Some(transition.document_order);
            let source = checked_index("transition source", transition.source, self.states.len())?;
            let transition_index =
                u32::try_from(index).map_err(|_| LowerError::TooManyTransitions)?;
            if !self.states[source].transitions.contains(&transition_index) {
                return Err(LowerError::InvalidReference("transition source ownership"));
            }
            validate_index_list(
                "transition targets",
                &transition.targets,
                self.states.len(),
                false,
            )?;
            bounded_count(
                "event descriptors",
                transition.events.len(),
                MAX_NESTED_ITEMS,
            )?;
            for event in &transition.events {
                bounded_required("event descriptor", event)?;
                if event.contains(scxml_syntax::is_xml_space) {
                    return Err(LowerError::InvalidStructure(
                        "event descriptors must be individually normalized",
                    ));
                }
            }
            bounded_optional("condition", transition.cond.as_deref())?;
            validate_event_schema(transition.event_schema.as_ref())?;
            validate_actions(&transition.actions, 0)?;
            validate_multi_target(
                &transition.targets,
                &self.states,
                &hierarchy,
                &mut multi_target_budget,
            )?;
            if matches!(
                self.states[source].kind,
                StateKind::ShallowHistory | StateKind::DeepHistory
            ) && (!transition.events.is_empty() || transition.cond.is_some())
            {
                return Err(LowerError::InvalidStructure(
                    "history fallback transition cannot have event or condition",
                ));
            }
            if matches!(
                self.states[source].kind,
                StateKind::ShallowHistory | StateKind::DeepHistory
            ) {
                let parent = self.states[source]
                    .parent
                    .ok_or(LowerError::InvalidReference("history parent"))?;
                for target in &transition.targets {
                    let target = checked_index("history target", *target, self.states.len())?;
                    if !is_descendant(target, parent, &self.states)? {
                        return Err(LowerError::InvalidStructure(
                            "history fallback target must be a descendant of its parent",
                        ));
                    }
                }
            }
        }
        for (state_index, state) in self.states.iter().enumerate() {
            for transition in &state.transitions {
                let transition =
                    checked_index("state transition", *transition, self.transitions.len())?;
                if usize::try_from(self.transitions[transition].source).ok() != Some(state_index) {
                    return Err(LowerError::InvalidReference("state transition source"));
                }
            }
        }
        validate_data(&self.data)?;
        validate_action_blocks(&self.scripts)?;
        self.validate_script_pins()
    }

    /// Encodes validated canonical pretty JSON with stable field and vector order.
    ///
    /// # Errors
    ///
    /// Returns a typed validation or serialization failure.
    pub fn to_json(&self) -> Result<Vec<u8>, LowerError> {
        let span = observe(Module::CompilerCore, Operation::CompilerEncodeIr);
        let result = (|| {
            self.validate()?;
            let bytes = serde_json::to_vec_pretty(self).map_err(|_| LowerError::IrSerialization)?;
            if scxml_types::exceeds_wasm_bound(bytes.len(), MAX_IR_BYTES) {
                return Err(LowerError::LimitExceeded {
                    field: "encoded IR bytes",
                    limit: MAX_IR_BYTES,
                });
            }
            Ok(bytes)
        })();
        if result.is_err() {
            span.fail(ErrorCategory::Lowering);
        } else {
            span.event(SignalEvent::ArtifactProduced);
        }
        result
    }

    /// Decodes untrusted JSON and establishes every Machine IR invariant.
    ///
    /// # Errors
    ///
    /// Rejects oversized input, syntax/schema errors, unknown fields or variants,
    /// unsupported versions, invalid references, and illegal state graphs.
    pub fn from_json(bytes: &[u8]) -> Result<Self, LowerError> {
        let span = observe(Module::CompilerCore, Operation::CompilerDecodeIr);
        let result = (|| {
            if scxml_types::exceeds_wasm_bound(bytes.len(), MAX_IR_BYTES) {
                return Err(LowerError::LimitExceeded {
                    field: "IR bytes",
                    limit: MAX_IR_BYTES,
                });
            }
            // Version first: IR of another version may use a different closed
            // schema, so it must fail as unsupported rather than as malformed.
            let probe: VersionProbe =
                serde_json::from_slice(bytes).map_err(|_| LowerError::InvalidIrSchema)?;
            if probe.version != MACHINE_IR_VERSION {
                return Err(LowerError::UnsupportedIrVersion(probe.version));
            }
            let value: Self =
                serde_json::from_slice(bytes).map_err(|_| LowerError::InvalidIrSchema)?;
            value.validate()?;
            Ok(value)
        })();
        if result.is_err() {
            span.fail(ErrorCategory::InvalidInput);
        } else {
            span.event(SignalEvent::ArtifactAccepted);
        }
        result
    }
}

impl MachineIr {
    /// Every executable-content block with the document order of its owner.
    fn action_blocks(&self) -> Vec<(u32, &[ExecutableContentIr])> {
        let mut blocks: Vec<(u32, &[ExecutableContentIr])> =
            vec![(self.initial.document_order, self.initial.actions.as_slice())];
        blocks.extend(
            self.scripts
                .iter()
                .map(|block| (block.document_order, block.actions.as_slice())),
        );
        for state in &self.states {
            if let Some(initial) = &state.initial {
                blocks.push((initial.document_order, initial.actions.as_slice()));
            }
            blocks.extend(
                state
                    .onentry
                    .iter()
                    .chain(&state.onexit)
                    .map(|block| (block.document_order, block.actions.as_slice())),
            );
            for invoke in &state.invokes {
                for child in &invoke.children {
                    if let InvokeChildIr::Finalize { actions } = child {
                        blocks.push((state.document_order, actions.as_slice()));
                    }
                }
            }
        }
        blocks.extend(
            self.transitions
                .iter()
                .map(|transition| (transition.document_order, transition.actions.as_slice())),
        );
        blocks
    }

    /// Requires every reference to one `src` URI to carry the same pin, so
    /// the host loads exactly one script per URI.
    fn validate_script_pins(&self) -> Result<(), LowerError> {
        let mut pins = BTreeMap::new();
        for (_, actions) in self.action_blocks() {
            let mut sources = Vec::new();
            collect_script_sources(actions, &mut sources);
            for source in sources {
                if pins
                    .insert(source.uri.as_str(), &source.pinned)
                    .is_some_and(|prior| prior != &source.pinned)
                {
                    return Err(LowerError::InvalidStructure(
                        "one script src URI carries two different pins",
                    ));
                }
            }
        }
        Ok(())
    }

    /// Distinct external script references the host must resolve at
    /// document load (see [`ScriptSourceIr::requires_host_load`]), ordered by
    /// the document order of their owning element and deduplicated by URI
    /// (first occurrence wins). Bounded by the validated IR size.
    #[must_use]
    pub fn host_loaded_scripts(&self) -> Vec<&ScriptSourceIr> {
        let blocks = self.action_blocks();
        let mut found: Vec<(u32, &ScriptSourceIr)> = Vec::new();
        for (order, actions) in blocks {
            let mut sources = Vec::new();
            collect_script_sources(actions, &mut sources);
            found.extend(sources.into_iter().map(|source| (order, source)));
        }
        found.sort_by_key(|(order, _)| *order);
        let mut seen = BTreeSet::new();
        found
            .into_iter()
            .map(|(_, source)| source)
            .filter(|source| source.requires_host_load() && seen.insert(source.uri.as_str()))
            .collect()
    }

    /// Applies `pin` to every external script reference in place; `pin`
    /// receives the reference and its body and may replace both. Used by
    /// the compiler to pin references from script packages before the IR is
    /// validated and encoded.
    ///
    /// # Errors
    ///
    /// Returns the first error `pin` returns; references visited before it
    /// may already be updated, so callers discard the IR on failure.
    pub fn try_update_script_sources<E>(
        &mut self,
        pin: &mut impl FnMut(&mut ScriptSourceIr, &mut String) -> Result<(), E>,
    ) -> Result<(), E> {
        update_script_sources(&mut self.initial.actions, pin)?;
        for block in &mut self.scripts {
            update_script_sources(&mut block.actions, pin)?;
        }
        for state in &mut self.states {
            if let Some(initial) = &mut state.initial {
                update_script_sources(&mut initial.actions, pin)?;
            }
            for block in state.onentry.iter_mut().chain(&mut state.onexit) {
                update_script_sources(&mut block.actions, pin)?;
            }
            for invoke in &mut state.invokes {
                for child in &mut invoke.children {
                    if let InvokeChildIr::Finalize { actions } = child {
                        update_script_sources(actions, pin)?;
                    }
                }
            }
        }
        for transition in &mut self.transitions {
            update_script_sources(&mut transition.actions, pin)?;
        }
        Ok(())
    }
}

/// Collects script sources in `actions`, depth-first in document order.
/// Recursion is bounded by the validated executable-content depth.
fn collect_script_sources<'a>(
    actions: &'a [ExecutableContentIr],
    sources: &mut Vec<&'a ScriptSourceIr>,
) {
    for action in actions {
        match action {
            ExecutableContentIr::Script { src: Some(src), .. } => sources.push(src),
            ExecutableContentIr::If {
                actions,
                elseif,
                else_actions,
                ..
            } => {
                collect_script_sources(actions, sources);
                for branch in elseif {
                    collect_script_sources(&branch.actions, sources);
                }
                if let Some(actions) = else_actions {
                    collect_script_sources(actions, sources);
                }
            }
            ExecutableContentIr::Foreach { actions, .. } => {
                collect_script_sources(actions, sources);
            }
            _ => {}
        }
    }
}

fn update_script_sources<E>(
    actions: &mut [ExecutableContentIr],
    pin: &mut impl FnMut(&mut ScriptSourceIr, &mut String) -> Result<(), E>,
) -> Result<(), E> {
    for action in actions {
        match action {
            ExecutableContentIr::Script {
                src: Some(src),
                body,
            } => pin(src, body)?,
            ExecutableContentIr::If {
                actions,
                elseif,
                else_actions,
                ..
            } => {
                update_script_sources(actions, pin)?;
                for branch in elseif {
                    update_script_sources(&mut branch.actions, pin)?;
                }
                if let Some(actions) = else_actions {
                    update_script_sources(actions, pin)?;
                }
            }
            ExecutableContentIr::Foreach { actions, .. } => {
                update_script_sources(actions, pin)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Open probe that reads only the top-level `version` of untrusted IR JSON.
#[derive(Deserialize)]
struct VersionProbe {
    version: u32,
}

fn bounded_count(field: &'static str, count: usize, limit: usize) -> Result<(), LowerError> {
    if count > limit {
        Err(LowerError::LimitExceeded { field, limit })
    } else {
        Ok(())
    }
}

fn bounded_required(field: &'static str, value: &str) -> Result<(), LowerError> {
    if value.is_empty() {
        return Err(LowerError::InvalidStructure(field));
    }
    bounded_optional(field, Some(value))
}

/// A field the engine turns into an event name, send ID, or invoke ID: a
/// name on every build (non-empty, no white space, at most
/// [`scxml_types::MAX_NAME_BYTES`] bytes), so the machine never meets one
/// it cannot represent while it runs.
fn checked_name(field: &'static str, value: &str) -> Result<(), LowerError> {
    scxml_types::EventName::new(value)
        .map(drop)
        .map_err(|_| LowerError::InvalidName(field))
}

/// Every state the engine can raise `done.state.<ID>` for has an ID that
/// leaves room for that event. SCXML 1.0 Appendix D raises it only on
/// entering a final state: for its parent (whatever that parent's kind),
/// and for its grandparent when that is a parallel state whose child states
/// are all in final states ("if isParallelState(grandparent): if
/// getChildStates(grandparent).every(isInFinalState)"). So a parallel state
/// can complete only when one of its child states is a compound state with
/// a final child and every one of them can be in a final state at once.
/// Children follow their parent in the table, so one reverse pass decides
/// every state from its children.
fn validate_done_state_names(states: &[StateIr]) -> Result<(), LowerError> {
    // Whether Appendix D `isInFinalState` can hold for each state: a
    // compound state with a final child, or a parallel state every child
    // state of which can (history excluded).
    let mut can_be_final = vec![false; states.len()];
    for (index, state) in states.iter().enumerate().rev() {
        let children: Vec<(usize, &StateIr)> = state
            .children
            .iter()
            .filter_map(|child| usize::try_from(*child).ok())
            .filter_map(|child| states.get(child).map(|state| (child, state)))
            .collect();
        let has_final_child = |state: &StateIr| {
            state.children.iter().any(|child| {
                usize::try_from(*child)
                    .ok()
                    .and_then(|child| states.get(child))
                    .is_some_and(|child| child.kind == StateKind::Final)
            })
        };
        let regions = || {
            children.iter().filter(|(_, child)| {
                !matches!(
                    child.kind,
                    StateKind::ShallowHistory | StateKind::DeepHistory
                )
            })
        };
        let all_regions_can_be_final =
            || regions().all(|(child, _)| can_be_final.get(*child).copied().unwrap_or(false));
        let in_final = match state.kind {
            StateKind::Compound => has_final_child(state),
            StateKind::Parallel => all_regions_can_be_final(),
            _ => false,
        };
        let can_complete = has_final_child(state)
            || state.kind == StateKind::Parallel
                && regions()
                    .any(|(_, child)| child.kind == StateKind::Compound && has_final_child(child))
                && all_regions_can_be_final();
        if let Some(slot) = can_be_final.get_mut(index) {
            *slot = in_final;
        }
        if can_complete && let Some(id) = &state.id {
            checked_name("state done event", &format!("done.state.{id}"))?;
        }
    }
    Ok(())
}

/// A required expression attribute (`<if cond>`, `<foreach array>`, `<assign
/// location>`, ...): an empty value is kept, because SCXML defines its run-time
/// outcome (5.9.4, 4.6, 5.9.2: `error.execution`); only the size is bounded.
fn bounded_expression(field: &'static str, value: &str) -> Result<(), LowerError> {
    bounded_optional(field, Some(value))
}

fn bounded_optional(field: &'static str, value: Option<&str>) -> Result<(), LowerError> {
    if value.is_some_and(|value| scxml_types::exceeds_wasm_bound(value.len(), MAX_STRING_BYTES)) {
        Err(LowerError::LimitExceeded {
            field,
            limit: MAX_STRING_BYTES,
        })
    } else {
        Ok(())
    }
}

/// Canonical IR never carries a blank `<log expr>`: lowering maps it to the
/// absent attribute.
fn reject_blank(value: Option<&str>, rule: &'static str) -> Result<(), LowerError> {
    if value.is_some_and(scxml_syntax::is_xml_blank) {
        Err(LowerError::InvalidStructure(rule))
    } else {
        Ok(())
    }
}

fn validate_sorted_unique_strings(
    field: &'static str,
    values: &[String],
) -> Result<(), LowerError> {
    let mut prior: Option<&str> = None;
    for value in values {
        bounded_required(field, value)?;
        if prior.is_some_and(|item| item >= value.as_str()) {
            return Err(LowerError::InvalidStructure(
                "sorted string table is not strictly increasing",
            ));
        }
        prior = Some(value);
    }
    Ok(())
}

fn checked_index(field: &'static str, value: u32, length: usize) -> Result<usize, LowerError> {
    let value = usize::try_from(value).map_err(|_| LowerError::InvalidReference(field))?;
    if value >= length {
        Err(LowerError::InvalidReference(field))
    } else {
        Ok(value)
    }
}

fn validate_index_list(
    field: &'static str,
    values: &[u32],
    length: usize,
    require_increasing: bool,
) -> Result<(), LowerError> {
    bounded_count(field, values.len(), MAX_NESTED_ITEMS)?;
    let mut seen = BTreeSet::new();
    let mut prior = None;
    for value in values {
        checked_index(field, *value, length)?;
        if !seen.insert(*value) {
            return Err(LowerError::InvalidReference(field));
        }
        if require_increasing && prior.is_some_and(|item| *value <= item) {
            return Err(LowerError::InvalidReference(field));
        }
        prior = Some(*value);
    }
    Ok(())
}

fn require_empty<T>(field: &'static str, values: &[T]) -> Result<(), LowerError> {
    if values.is_empty() {
        Ok(())
    } else {
        Err(LowerError::InvalidStructure(field))
    }
}

fn require_none<T>(field: &'static str, value: Option<&T>) -> Result<(), LowerError> {
    if value.is_none() {
        Ok(())
    } else {
        Err(LowerError::InvalidStructure(field))
    }
}

fn validate_initial(
    initial: &InitialIr,
    owner: Option<u32>,
    states: &[StateIr],
    hierarchy: &HierarchyIndex,
    multi_target_budget: &mut MultiTargetBudget,
) -> Result<(), LowerError> {
    if initial.targets.is_empty() {
        return Err(LowerError::InvalidStructure(
            "initial transition requires at least one target",
        ));
    }
    validate_index_list("initial targets", &initial.targets, states.len(), false)?;
    validate_actions(&initial.actions, 0)?;
    validate_multi_target(&initial.targets, states, hierarchy, multi_target_budget)?;
    for target in &initial.targets {
        let target = checked_index("initial target", *target, states.len())?;
        // Root initial targets may sit anywhere in the hierarchy: the SCXML
        // algorithm enters every proper ancestor of each target (official
        // cases 413 and 576 use deeply nested initial targets).
        if let Some(owner) = owner
            && !is_descendant(target, owner, states)?
        {
            return Err(LowerError::InvalidStructure(
                "state initial target must be a descendant",
            ));
        }
    }
    Ok(())
}

fn is_descendant(index: usize, ancestor: u32, states: &[StateIr]) -> Result<bool, LowerError> {
    let mut cursor = states[index].parent;
    let mut steps = 0usize;
    while let Some(parent) = cursor {
        if parent == ancestor {
            return Ok(true);
        }
        let parent_index = checked_index("ancestor", parent, states.len())?;
        cursor = states[parent_index].parent;
        steps = steps.saturating_add(1);
        if scxml_types::exceeds_wasm_bound(steps, MAX_CONTENT_DEPTH) {
            return Err(LowerError::LimitExceeded {
                field: "state hierarchy depth",
                limit: MAX_CONTENT_DEPTH,
            });
        }
    }
    Ok(false)
}

struct HierarchyIndex {
    depth: Vec<usize>,
    preorder: Vec<usize>,
    roots: Vec<usize>,
    jumps: Vec<Vec<usize>>,
}

impl HierarchyIndex {
    fn new(states: &[StateIr]) -> Result<Self, LowerError> {
        let mut depth: Vec<usize> = Vec::with_capacity(states.len());
        let mut roots: Vec<usize> = Vec::with_capacity(states.len());
        let mut parents: Vec<usize> = Vec::with_capacity(states.len());
        for (index, state) in states.iter().enumerate() {
            if let Some(parent) = state.parent {
                let parent = checked_index("state parent", parent, states.len())?;
                let parent_depth = depth
                    .get(parent)
                    .copied()
                    .ok_or(LowerError::InvalidReference("state parent order"))?;
                depth.push(
                    parent_depth
                        .checked_add(1)
                        .ok_or(LowerError::LimitExceeded {
                            field: "state hierarchy depth",
                            limit: states.len(),
                        })?,
                );
                roots.push(
                    roots
                        .get(parent)
                        .copied()
                        .ok_or(LowerError::InvalidReference("state parent order"))?,
                );
                parents.push(parent);
            } else {
                depth.push(0);
                roots.push(index);
                parents.push(index);
            }
        }

        let mut preorder = vec![usize::MAX; states.len()];
        let mut stack = roots
            .iter()
            .enumerate()
            .filter_map(|(index, root)| (*root == index).then_some(index))
            .collect::<Vec<_>>();
        stack.reverse();
        let mut next_preorder = 0usize;
        while let Some(index) = stack.pop() {
            let rank = preorder
                .get_mut(index)
                .ok_or(LowerError::InvalidReference("state hierarchy traversal"))?;
            if *rank != usize::MAX {
                return Err(LowerError::InvalidStructure(
                    "state hierarchy contains a cycle",
                ));
            }
            *rank = next_preorder;
            next_preorder = next_preorder
                .checked_add(1)
                .ok_or(LowerError::LimitExceeded {
                    field: "state hierarchy traversal",
                    limit: states.len(),
                })?;
            let state = states
                .get(index)
                .ok_or(LowerError::InvalidReference("state hierarchy traversal"))?;
            for child in state.children.iter().rev() {
                stack.push(checked_index("state child", *child, states.len())?);
            }
        }
        if next_preorder != states.len() {
            return Err(LowerError::InvalidStructure(
                "state hierarchy traversal must cover every state",
            ));
        }

        let mut jumps = vec![parents];
        let mut covered_distance = 1usize;
        while covered_distance < states.len() {
            let prior = jumps.last().ok_or(LowerError::InvalidStructure(
                "state ancestor index is empty",
            ))?;
            let mut next = Vec::with_capacity(states.len());
            for ancestor in prior {
                next.push(
                    prior
                        .get(*ancestor)
                        .copied()
                        .ok_or(LowerError::InvalidReference("state ancestor"))?,
                );
            }
            jumps.push(next);
            covered_distance =
                covered_distance
                    .checked_mul(2)
                    .ok_or(LowerError::LimitExceeded {
                        field: "state ancestor index distance",
                        limit: states.len(),
                    })?;
        }

        Ok(Self {
            depth,
            preorder,
            roots,
            jumps,
        })
    }

    fn least_common_ancestor(
        &self,
        left: usize,
        right: usize,
    ) -> Result<Option<usize>, LowerError> {
        if self.roots.get(left) != self.roots.get(right) {
            return Ok(None);
        }
        let mut left = left;
        let mut right = right;
        let left_depth = self
            .depth
            .get(left)
            .copied()
            .ok_or(LowerError::InvalidReference("target depth"))?;
        let right_depth = self
            .depth
            .get(right)
            .copied()
            .ok_or(LowerError::InvalidReference("target depth"))?;
        if left_depth > right_depth {
            left = self.lift(left, left_depth - right_depth)?;
        } else {
            right = self.lift(right, right_depth - left_depth)?;
        }
        if left == right {
            return Ok(Some(left));
        }
        for jump in self.jumps.iter().rev() {
            let left_ancestor = jump
                .get(left)
                .copied()
                .ok_or(LowerError::InvalidReference("target ancestor"))?;
            let right_ancestor = jump
                .get(right)
                .copied()
                .ok_or(LowerError::InvalidReference("target ancestor"))?;
            if left_ancestor != right_ancestor {
                left = left_ancestor;
                right = right_ancestor;
            }
        }
        self.jumps
            .first()
            .and_then(|parents| parents.get(left))
            .copied()
            .map(Some)
            .ok_or(LowerError::InvalidReference("target ancestor"))
    }

    fn lift(&self, mut index: usize, mut distance: usize) -> Result<usize, LowerError> {
        let mut bit = 0usize;
        while distance != 0 {
            if distance & 1 == 1 {
                index = self
                    .jumps
                    .get(bit)
                    .and_then(|jump| jump.get(index))
                    .copied()
                    .ok_or(LowerError::InvalidReference("target ancestor"))?;
            }
            distance >>= 1;
            bit = bit.checked_add(1).ok_or(LowerError::LimitExceeded {
                field: "state ancestor index level",
                limit: self.jumps.len(),
            })?;
        }
        Ok(index)
    }
}

struct MultiTargetBudget {
    remaining: usize,
    #[cfg(test)]
    adjacency_checks: usize,
}

impl MultiTargetBudget {
    fn new() -> Self {
        Self {
            remaining: MAX_MULTI_TARGET_VALIDATION_ITEMS,
            #[cfg(test)]
            adjacency_checks: 0,
        }
    }

    fn spend(&mut self, count: usize) -> Result<(), LowerError> {
        self.remaining = self
            .remaining
            .checked_sub(count)
            .ok_or(LowerError::LimitExceeded {
                field: "multi-target validation items",
                limit: MAX_MULTI_TARGET_VALIDATION_ITEMS,
            })?;
        Ok(())
    }
}

fn validate_multi_target(
    targets: &[u32],
    states: &[StateIr],
    hierarchy: &HierarchyIndex,
    budget: &mut MultiTargetBudget,
) -> Result<(), LowerError> {
    if targets.len() < 2 {
        return Ok(());
    }
    budget.spend(targets.len())?;
    let mut ordered = Vec::with_capacity(targets.len());
    for target in targets {
        let index = checked_index("transition target", *target, states.len())?;
        let preorder = hierarchy
            .preorder
            .get(index)
            .copied()
            .ok_or(LowerError::InvalidReference("target preorder"))?;
        ordered.push((preorder, index));
    }
    ordered.sort_unstable_by_key(|(preorder, _)| *preorder);

    // In preorder, every boundary between selected subtrees is represented by
    // adjacent targets. If each adjacent pair is in distinct regions of a
    // parallel LCA, every non-adjacent pair crosses only those valid boundaries.
    for pair in ordered.windows(2) {
        #[cfg(test)]
        {
            budget.adjacency_checks =
                budget
                    .adjacency_checks
                    .checked_add(1)
                    .ok_or(LowerError::LimitExceeded {
                        field: "multi-target adjacency checks",
                        limit: MAX_MULTI_TARGET_VALIDATION_ITEMS,
                    })?;
        }
        let [(_, left), (_, right)] = pair else {
            return Err(LowerError::InvalidStructure(
                "multi-target adjacency window must contain two targets",
            ));
        };
        let Some(common) = hierarchy.least_common_ancestor(*left, *right)? else {
            return Err(LowerError::InvalidStructure(
                "multi-target transition requires a common parallel ancestor",
            ));
        };
        if common == *left || common == *right {
            return Err(LowerError::InvalidStructure(
                "multi-target transition cannot target ancestor-related states",
            ));
        }
        if states
            .get(common)
            .ok_or(LowerError::InvalidReference("common ancestor"))?
            .kind
            != StateKind::Parallel
        {
            return Err(LowerError::InvalidStructure(
                "multi-target transition requires a parallel least common ancestor",
            ));
        }
    }
    Ok(())
}

fn validate_action_blocks(blocks: &[ActionBlockIr]) -> Result<(), LowerError> {
    bounded_count("action blocks", blocks.len(), MAX_NESTED_ITEMS)?;
    let mut prior = None;
    for block in blocks {
        if prior.is_some_and(|value| block.document_order <= value) {
            return Err(LowerError::InvalidStructure(
                "action block document order must be strictly increasing",
            ));
        }
        prior = Some(block.document_order);
        validate_actions(&block.actions, 0)?;
    }
    Ok(())
}

/// Validates one external script reference against the body it travels
/// with (SCXML 1.0 section 5.8: `src` or inline content, never both).
fn validate_script_source(src: &ScriptSourceIr, body: &str) -> Result<(), LowerError> {
    scxml_script::validate_uri(&src.uri)
        .map_err(|_| LowerError::InvalidStructure("script src URI is invalid"))?;
    let Some(pinned) = &src.pinned else {
        if !scxml_syntax::is_xml_blank(body) {
            return Err(LowerError::InvalidStructure(
                "script cannot combine src and inline body",
            ));
        }
        return Ok(());
    };
    bounded_required("script media type", &pinned.media_type)?;
    bounded_optional("script WIT world", pinned.wit_world.as_deref())?;
    if !scxml_script::is_sha256_hex(&pinned.sha256) {
        return Err(LowerError::InvalidStructure(
            "pinned script SHA-256 must be 64 lowercase hex characters",
        ));
    }
    if pinned.media_type == scxml_script::WASM_MEDIA_TYPE {
        if pinned.wit_world.as_deref() != Some(scxml_script::SCRIPT_WORLD) || !body.is_empty() {
            return Err(LowerError::InvalidStructure(
                "pinned Wasm script requires the script world and no embedded body",
            ));
        }
    } else if pinned.wit_world.is_some()
        || body.is_empty()
        || scxml_script::sha256_hex(body.as_bytes()) != pinned.sha256
    {
        return Err(LowerError::InvalidStructure(
            "pinned script text must be embedded and match its SHA-256",
        ));
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "all typed executable-content variants share one recursive bounded validator"
)]
fn validate_actions(actions: &[ExecutableContentIr], depth: usize) -> Result<(), LowerError> {
    if scxml_types::exceeds_wasm_bound(depth, MAX_CONTENT_DEPTH) {
        return Err(LowerError::LimitExceeded {
            field: "executable content depth",
            limit: MAX_CONTENT_DEPTH,
        });
    }
    bounded_count("executable actions", actions.len(), MAX_NESTED_ITEMS)?;
    for action in actions {
        match action {
            ExecutableContentIr::Raise {
                event,
                event_schema,
            } => {
                checked_name("raise event", event)?;
                validate_event_schema(event_schema.as_ref())?;
            }
            ExecutableContentIr::Log { label, expr } => {
                bounded_optional("log label", label.as_deref())?;
                bounded_optional("log expression", expr.as_deref())?;
                reject_blank(
                    expr.as_deref(),
                    "blank log expression is not canonical Machine IR",
                )?;
            }
            ExecutableContentIr::Assign {
                location,
                expr,
                body,
                assign_type,
                attr,
            } => {
                bounded_expression("assign location", location)?;
                bounded_optional("assign expression", expr.as_deref())?;
                bounded_optional("assign attr", attr.as_deref())?;
                bounded_count("assign body", body.len(), MAX_NESTED_ITEMS)?;
                if expr.is_some() && !body.is_empty() {
                    return Err(LowerError::InvalidStructure(
                        "assign cannot combine expr and inline body",
                    ));
                }
                if attr.is_some() != (*assign_type == Some(AssignTypeIr::Addattribute)) {
                    return Err(LowerError::InvalidStructure(
                        "assign attr is required exactly for type addattribute",
                    ));
                }
                if *assign_type == Some(AssignTypeIr::Delete)
                    && (expr.is_some() || !body.is_empty())
                {
                    return Err(LowerError::InvalidStructure(
                        "assign type delete takes no value",
                    ));
                }
                for node in body {
                    validate_xml_node(node, 0)?;
                }
            }
            ExecutableContentIr::If {
                cond,
                actions,
                elseif,
                else_actions,
            } => {
                bounded_expression("if condition", cond)?;
                validate_actions(actions, depth.saturating_add(1))?;
                bounded_count("elseif branches", elseif.len(), MAX_NESTED_ITEMS)?;
                for branch in elseif {
                    bounded_expression("elseif condition", &branch.cond)?;
                    validate_actions(&branch.actions, depth.saturating_add(1))?;
                }
                if let Some(actions) = else_actions {
                    validate_actions(actions, depth.saturating_add(1))?;
                }
            }
            ExecutableContentIr::Foreach {
                array,
                item,
                index,
                actions,
            } => {
                bounded_expression("foreach array", array)?;
                bounded_expression("foreach item", item)?;
                bounded_optional("foreach index", index.as_deref())?;
                validate_actions(actions, depth.saturating_add(1))?;
            }
            ExecutableContentIr::Send { value } => {
                if let Some(event) = &value.event {
                    checked_name("send event", event)?;
                }
                if let Some(id) = &value.id {
                    checked_name("send id", id)?;
                }
                for (field, value) in [
                    ("send eventexpr", value.eventexpr.as_deref()),
                    ("send target", value.target.as_deref()),
                    ("send targetexpr", value.targetexpr.as_deref()),
                    ("send type", value.type_value.as_deref()),
                    ("send typeexpr", value.typeexpr.as_deref()),
                    ("send idlocation", value.idlocation.as_deref()),
                    ("send delay", value.delay.as_deref()),
                    ("send delayexpr", value.delayexpr.as_deref()),
                ] {
                    bounded_optional(field, value)?;
                }
                validate_name_list(&value.namelist)?;
                validate_params(&value.params)?;
                validate_event_schema(value.event_schema.as_ref())?;
                if let Some(content) = &value.content {
                    validate_content(content, depth.saturating_add(1))?;
                }
                require_at_most_one(
                    "send event/eventexpr",
                    value.event.is_some(),
                    value.eventexpr.is_some(),
                )?;
                // SCXML 6.2: only the SCXML Event I/O Processor requires an
                // event name; a send without one must name its processor
                // (the BasicHTTP processor then sends `HTTP.POST`, SCXML
                // C.2). Which processor a literal type names is decided by
                // the validator and the executing host.
                if value.event.is_none()
                    && value.eventexpr.is_none()
                    && value.type_value.is_none()
                    && value.typeexpr.is_none()
                {
                    return Err(LowerError::InvalidStructure("send event/eventexpr"));
                }
                require_at_most_one(
                    "send target/targetexpr",
                    value.target.is_some(),
                    value.targetexpr.is_some(),
                )?;
                require_at_most_one(
                    "send type/typeexpr",
                    value.type_value.is_some(),
                    value.typeexpr.is_some(),
                )?;
                require_at_most_one(
                    "send id/idlocation",
                    value.id.is_some(),
                    value.idlocation.is_some(),
                )?;
                require_at_most_one(
                    "send delay/delayexpr",
                    value.delay.is_some(),
                    value.delayexpr.is_some(),
                )?;
                if value.content.is_some()
                    && (!value.namelist.is_empty() || !value.params.is_empty())
                {
                    return Err(LowerError::InvalidStructure(
                        "send content cannot be combined with namelist or params",
                    ));
                }
            }
            ExecutableContentIr::Cancel {
                sendid: send_id,
                sendidexpr,
            } => {
                bounded_optional("cancel sendid", send_id.as_deref())?;
                bounded_optional("cancel sendidexpr", sendidexpr.as_deref())?;
                require_exactly_one(
                    "cancel sendid/sendidexpr",
                    send_id.is_some(),
                    sendidexpr.is_some(),
                )?;
            }
            ExecutableContentIr::Script { src, body } => {
                bounded_optional("script body", Some(body))?;
                if let Some(src) = src {
                    validate_script_source(src, body)?;
                }
            }
            ExecutableContentIr::Extension { element } => {
                validate_xml_node(element, depth.saturating_add(1))?;
                let XmlNodeIr::Element { name, .. } = element else {
                    return Err(LowerError::InvalidStructure(
                        "executable content extension must be an element",
                    ));
                };
                if name.namespace_uri.is_none() {
                    return Err(LowerError::InvalidStructure(
                        "executable content extension requires a namespace",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_name_list(values: &[String]) -> Result<(), LowerError> {
    bounded_count("name list", values.len(), MAX_NESTED_ITEMS)?;
    for value in values {
        bounded_required("name list item", value)?;
        if value.contains(scxml_syntax::is_xml_space) {
            return Err(LowerError::InvalidStructure(
                "name list items must be individually normalized",
            ));
        }
    }
    Ok(())
}

fn require_at_most_one(field: &'static str, left: bool, right: bool) -> Result<(), LowerError> {
    if left && right {
        Err(LowerError::InvalidStructure(field))
    } else {
        Ok(())
    }
}

fn require_exactly_one(field: &'static str, left: bool, right: bool) -> Result<(), LowerError> {
    if left == right {
        Err(LowerError::InvalidStructure(field))
    } else {
        Ok(())
    }
}

fn validate_params(params: &[ParamIr]) -> Result<(), LowerError> {
    bounded_count("parameters", params.len(), MAX_NESTED_ITEMS)?;
    for param in params {
        bounded_required("parameter name", &param.name)?;
        bounded_optional("parameter expression", param.expr.as_deref())?;
        bounded_optional("parameter location", param.location.as_deref())?;
        if param.expr.is_some() == param.location.is_some() {
            return Err(LowerError::InvalidStructure(
                "parameter requires exactly one of expr or location",
            ));
        }
    }
    Ok(())
}

fn validate_content(content: &ContentIr, depth: usize) -> Result<(), LowerError> {
    bounded_optional("content expression", content.expr.as_deref())?;
    bounded_count("content body", content.body.len(), MAX_NESTED_ITEMS)?;
    if content.expr.is_some() && !content.body.is_empty() {
        return Err(LowerError::InvalidStructure(
            "content cannot combine expr and inline body",
        ));
    }
    for node in &content.body {
        validate_xml_node(node, depth)?;
    }
    Ok(())
}

fn validate_xml_node(node: &XmlNodeIr, depth: usize) -> Result<(), LowerError> {
    if scxml_types::exceeds_wasm_bound(depth, MAX_CONTENT_DEPTH) {
        return Err(LowerError::LimitExceeded {
            field: "inline XML depth",
            limit: MAX_CONTENT_DEPTH,
        });
    }
    match node {
        XmlNodeIr::Text { value } => bounded_optional("inline text", Some(value)),
        XmlNodeIr::Element {
            name,
            attributes,
            children,
        } => {
            bounded_optional("element namespace", name.namespace_uri.as_deref())?;
            bounded_required("element local name", &name.local_name)?;
            bounded_count("XML attributes", attributes.len(), MAX_NESTED_ITEMS)?;
            bounded_count("XML children", children.len(), MAX_NESTED_ITEMS)?;
            for attribute in attributes {
                bounded_optional(
                    "attribute namespace",
                    attribute.name.namespace_uri.as_deref(),
                )?;
                bounded_required("attribute local name", &attribute.name.local_name)?;
                bounded_optional("attribute value", Some(&attribute.value))?;
            }
            for child in children {
                validate_xml_node(child, depth.saturating_add(1))?;
            }
            Ok(())
        }
    }
}

fn validate_data(data: &[DataIr]) -> Result<(), LowerError> {
    bounded_count("data declarations", data.len(), MAX_NESTED_ITEMS)?;
    let mut ids = BTreeSet::new();
    for item in data {
        bounded_required("data id", &item.id)?;
        if !ids.insert(item.id.as_str()) {
            return Err(LowerError::InvalidStructure("duplicate data id in scope"));
        }
        bounded_optional("data src", item.src.as_deref())?;
        bounded_optional("data expr", item.expr.as_deref())?;
        let initializer_count = usize::from(item.src.is_some())
            + usize::from(item.expr.is_some())
            + usize::from(!item.content.is_empty());
        if initializer_count > 1 {
            return Err(LowerError::InvalidStructure(
                "data may have only one initializer form",
            ));
        }
        for node in &item.content {
            validate_xml_node(node, 0)?;
        }
    }
    Ok(())
}

fn validate_invokes(invokes: &[InvokeIr]) -> Result<(), LowerError> {
    bounded_count("invokes", invokes.len(), MAX_NESTED_ITEMS)?;
    for invoke in invokes {
        for (field, value) in [
            ("invoke type", invoke.type_value.as_deref()),
            ("invoke typeexpr", invoke.typeexpr.as_deref()),
            ("invoke src", invoke.src.as_deref()),
            ("invoke srcexpr", invoke.srcexpr.as_deref()),
            ("invoke idlocation", invoke.idlocation.as_deref()),
        ] {
            bounded_optional(field, value)?;
        }
        if let Some(id) = &invoke.id {
            checked_name("invoke id", id)?;
            checked_name("invoke done event", &format!("done.invoke.{id}"))?;
        }
        validate_name_list(&invoke.namelist)?;
        require_at_most_one(
            "invoke type/typeexpr",
            invoke.type_value.is_some(),
            invoke.typeexpr.is_some(),
        )?;
        require_at_most_one(
            "invoke src/srcexpr",
            invoke.src.is_some(),
            invoke.srcexpr.is_some(),
        )?;
        require_at_most_one(
            "invoke id/idlocation",
            invoke.id.is_some(),
            invoke.idlocation.is_some(),
        )?;
        bounded_count("invoke children", invoke.children.len(), MAX_NESTED_ITEMS)?;
        let mut content_count = 0usize;
        let mut finalize_count = 0usize;
        for child in &invoke.children {
            match child {
                InvokeChildIr::Param { value } => validate_params(std::slice::from_ref(value))?,
                InvokeChildIr::Content { value } => {
                    content_count = content_count.saturating_add(1);
                    validate_content(value, 0)?;
                }
                InvokeChildIr::Finalize { actions } => {
                    finalize_count = finalize_count.saturating_add(1);
                    validate_actions(actions, 0)?;
                }
            }
        }
        if content_count > 1 || finalize_count > 1 {
            return Err(LowerError::InvalidStructure(
                "invoke permits at most one content and finalize child",
            ));
        }
        validate_invoke_env(invoke)?;
    }
    Ok(())
}

/// An `<invoke>`'s `env` attributes: only on an invoked service's (a
/// literal type that is not an SCXML invoke type, or a `typeexpr`), every one
/// in the `env` namespace, sorted by local name with each slot once, every
/// slot an `env-slots` entry, every literal name a variable name, every
/// expression bounded.
fn validate_invoke_env(invoke: &InvokeIr) -> Result<(), LowerError> {
    if invoke.attributes.is_empty() {
        return Ok(());
    }
    if invoke.typeexpr.is_none()
        && invoke
            .type_value
            .as_deref()
            .is_none_or(scxml_types::is_scxml_invoke_type)
    {
        return Err(LowerError::InvalidStructure(
            "only an invoked service's invoke binds env slots",
        ));
    }
    bounded_count(
        "invoke env attributes",
        invoke.attributes.len(),
        MAX_NESTED_ITEMS,
    )?;
    if !invoke.attributes.windows(2).all(
        |pair| matches!(pair, [first, second] if first.name.local_name < second.name.local_name),
    ) {
        return Err(LowerError::InvalidStructure(
            "invoke env attributes must be sorted by local name",
        ));
    }
    let mut slots = std::collections::BTreeSet::new();
    for attribute in &invoke.attributes {
        if attribute.name.namespace_uri.as_deref() != Some(scxml_types::ENV_NAMESPACE_URI) {
            return Err(LowerError::InvalidStructure(
                "an invoke attribute is not in the env namespace",
            ));
        }
        let (slot, expression) = invoke_env_slot(&attribute.name.local_name);
        if !scxml_validator_core::is_env_slot(slot) || !slots.insert(slot) {
            return Err(LowerError::InvalidStructure(
                "an invoke env attribute binds no env-slots name, or one slot twice",
            ));
        }
        if expression {
            bounded_expression("invoke env attribute expr", &attribute.value)?;
        } else if !scxml_validator_core::is_env_name(&attribute.value) {
            return Err(LowerError::InvalidStructure(
                "an invoke env attribute names no valid variable",
            ));
        }
    }
    Ok(())
}

/// An event schema's bounds, digest, and JSON syntax. The engine never
/// interprets the schema, so decoding checks only that it is JSON whose
/// SHA-256 it carries; the compiler writes it canonical.
fn validate_event_schema(schema: Option<&EventSchemaIr>) -> Result<(), LowerError> {
    match schema {
        None => Ok(()),
        Some(EventSchemaIr::Schemaexpr { expr }) => bounded_expression("event schemaexpr", expr),
        Some(EventSchemaIr::Schema { schema, sha256 }) => {
            bounded_expression("event schema", schema)?;
            if *sha256 != hex::encode(Sha256::digest(schema.as_bytes())) {
                return Err(LowerError::InvalidStructure(
                    "event schema sha256 does not match its schema",
                ));
            }
            if serde_json::from_slice::<serde::de::IgnoredAny>(schema.as_bytes()).is_err() {
                return Err(LowerError::InvalidStructure("event schema is not JSON"));
            }
            Ok(())
        }
    }
}

fn validate_done_data(done_data: &DoneDataIr) -> Result<(), LowerError> {
    validate_event_schema(done_data.event_schema.as_ref())?;
    bounded_count(
        "donedata children",
        done_data.children.len(),
        MAX_NESTED_ITEMS,
    )?;
    let mut content_count = 0usize;
    for child in &done_data.children {
        match child {
            DoneDataChildIr::Param { value } => validate_params(std::slice::from_ref(value))?,
            DoneDataChildIr::Content { value } => {
                content_count = content_count.saturating_add(1);
                validate_content(value, 0)?;
            }
        }
    }
    if content_count > 1 {
        return Err(LowerError::InvalidStructure(
            "donedata permits at most one content child",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(kind: StateKind, parent: Option<u32>, children: Vec<u32>) -> StateIr {
        StateIr {
            document_order: 0,
            id: Some("state".to_owned()),
            kind,
            parent,
            children,
            initial: None,
            onentry: Vec::new(),
            onexit: Vec::new(),
            transitions: Vec::new(),
            data: Vec::new(),
            invokes: Vec::new(),
            donedata: None,
        }
    }

    fn parallel_children(child_count: usize) -> (Vec<u32>, Vec<StateIr>) {
        let children = (1..=child_count)
            .map(|index| u32::try_from(index).expect("test index must fit u32"))
            .collect::<Vec<_>>();
        let mut states = Vec::with_capacity(child_count + 1);
        states.push(state(StateKind::Parallel, None, children.clone()));
        states.extend((0..child_count).map(|_| state(StateKind::Atomic, Some(0), Vec::new())));
        (children, states)
    }

    fn machine_with_target_sets(child_count: usize, transition_count: usize) -> MachineIr {
        let (children, mut states) = parallel_children(child_count);
        for (index, state) in states.iter_mut().enumerate() {
            state.document_order = u32::try_from(index + 1).expect("test order must fit u32");
            state.id = Some(format!("state-{index}"));
        }
        states
            .first_mut()
            .expect("test machine must have a parallel state")
            .transitions = (0..transition_count)
            .map(|index| u32::try_from(index).expect("test transition index must fit u32"))
            .collect();
        let transitions = (0..transition_count)
            .map(|index| TransitionIr {
                document_order: u32::try_from(child_count + index + 2)
                    .expect("test order must fit u32"),
                source: 0,
                targets: children.clone(),
                events: Vec::new(),
                cond: None,
                transition_type: TransitionType::External,
                actions: Vec::new(),
                event_schema: None,
            })
            .collect();
        MachineIr {
            version: MACHINE_IR_VERSION,
            name: None,
            data_model: None,
            binding: Binding::Early,
            initial: InitialIr {
                document_order: 0,
                targets: vec![0],
                actions: Vec::new(),
            },
            states,
            transitions,
            data: Vec::new(),
            scripts: Vec::new(),
            extension_namespaces: Vec::new(),
        }
    }

    /// Natively the multi-target and byte bounds do not apply: eleven
    /// 10,000-target sets (110,000 targets, past the wasm aggregate budget of
    /// 100,000) decode and validate.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_machine_ir_has_no_aggregate_target_budget() {
        let machine = machine_with_target_sets(10_000, 11);
        let bytes = serde_json::to_vec(&machine).expect("test IR must serialize");
        assert_eq!(
            MachineIr::from_json(&bytes).expect("native decoding is unbounded"),
            machine
        );
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn wasm_only_hostile_maximum_multi_target_set_has_linear_structural_work() {
        let child_count = MAX_NESTED_ITEMS;
        let (children, states) = parallel_children(child_count);

        let hierarchy = HierarchyIndex::new(&states).expect("test hierarchy must index");
        let mut budget = MultiTargetBudget::new();
        validate_multi_target(&children, &states, &hierarchy, &mut budget)
            .expect("children in separate parallel regions must be valid targets");
        assert_eq!(
            budget.adjacency_checks,
            child_count - 1,
            "multi-target validation must inspect only adjacent preorder targets"
        );
        assert_eq!(
            budget.remaining,
            MAX_MULTI_TARGET_VALIDATION_ITEMS - child_count
        );
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn wasm_only_aggregate_multi_target_budget_accepts_boundary_and_rejects_next_item() {
        let (children, states) = parallel_children(MAX_NESTED_ITEMS);
        let hierarchy = HierarchyIndex::new(&states).expect("test hierarchy must index");
        let mut budget = MultiTargetBudget::new();
        for _ in 0..(MAX_MULTI_TARGET_VALIDATION_ITEMS / MAX_NESTED_ITEMS) {
            validate_multi_target(&children, &states, &hierarchy, &mut budget)
                .expect("work through the exact aggregate budget must be accepted");
        }
        assert_eq!(budget.remaining, 0);
        assert_eq!(
            validate_multi_target(&children[..2], &states, &hierarchy, &mut budget),
            Err(LowerError::LimitExceeded {
                field: "multi-target validation items",
                limit: MAX_MULTI_TARGET_VALIDATION_ITEMS,
            })
        );
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn wasm_only_hostile_machine_ir_with_maximum_target_set_decodes_under_byte_bound() {
        let machine = machine_with_target_sets(MAX_NESTED_ITEMS, 1);
        let bytes = serde_json::to_vec(&machine).expect("hostile test IR must serialize");
        assert!(bytes.len() < MAX_IR_BYTES);
        assert_eq!(
            MachineIr::from_json(&bytes).expect("bounded hostile IR must validate"),
            machine
        );
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn wasm_only_hostile_machine_ir_over_aggregate_target_budget_fails_closed() {
        let over_budget_set_count = MAX_MULTI_TARGET_VALIDATION_ITEMS / MAX_NESTED_ITEMS + 1;
        let machine = machine_with_target_sets(MAX_NESTED_ITEMS, over_budget_set_count);
        let bytes = serde_json::to_vec(&machine).expect("hostile test IR must serialize");
        assert!(bytes.len() < MAX_IR_BYTES);
        assert_eq!(
            MachineIr::from_json(&bytes),
            Err(LowerError::LimitExceeded {
                field: "multi-target validation items",
                limit: MAX_MULTI_TARGET_VALIDATION_ITEMS,
            })
        );
    }

    #[test]
    fn multi_target_semantics_cover_nested_regions_and_fail_closed_cases() {
        let states = vec![
            state(StateKind::Parallel, None, vec![1, 4]),
            state(StateKind::Compound, Some(0), vec![2, 3]),
            state(StateKind::Atomic, Some(1), Vec::new()),
            state(StateKind::Atomic, Some(1), Vec::new()),
            state(StateKind::Parallel, Some(0), vec![5, 6]),
            state(StateKind::Atomic, Some(4), Vec::new()),
            state(StateKind::Atomic, Some(4), Vec::new()),
            state(StateKind::Atomic, None, Vec::new()),
        ];
        let hierarchy = HierarchyIndex::new(&states).expect("test hierarchy must index");

        let mut valid_budget = MultiTargetBudget::new();
        validate_multi_target(&[6, 2, 5], &states, &hierarchy, &mut valid_budget)
            .expect("targets crossing distinct nested parallel regions must be valid");

        for (targets, expected) in [
            (
                &[1, 2][..],
                "multi-target transition cannot target ancestor-related states",
            ),
            (
                &[2, 3][..],
                "multi-target transition requires a parallel least common ancestor",
            ),
            (
                &[2, 7][..],
                "multi-target transition requires a common parallel ancestor",
            ),
        ] {
            let mut budget = MultiTargetBudget::new();
            assert_eq!(
                validate_multi_target(targets, &states, &hierarchy, &mut budget),
                Err(LowerError::InvalidStructure(expected))
            );
        }
    }

    #[test]
    fn adjacent_preorder_checks_match_all_pairs_on_representative_hierarchy() {
        let states = vec![
            state(StateKind::Parallel, None, vec![1, 4]),
            state(StateKind::Compound, Some(0), vec![2, 3]),
            state(StateKind::Atomic, Some(1), Vec::new()),
            state(StateKind::Atomic, Some(1), Vec::new()),
            state(StateKind::Parallel, Some(0), vec![5, 6]),
            state(StateKind::Atomic, Some(4), Vec::new()),
            state(StateKind::Atomic, Some(4), Vec::new()),
            state(StateKind::Atomic, None, Vec::new()),
        ];
        let hierarchy = HierarchyIndex::new(&states).expect("test hierarchy must index");
        for selected in 0_u16..(1_u16 << states.len()) {
            let targets = (0..states.len())
                .filter(|index| selected & (1_u16 << index) != 0)
                .map(|index| u32::try_from(index).expect("test index must fit u32"))
                .collect::<Vec<_>>();
            if targets.len() < 2 {
                continue;
            }
            let all_pairs_valid = targets.iter().enumerate().all(|(position, left)| {
                targets[position + 1..].iter().all(|right| {
                    let common = hierarchy
                        .least_common_ancestor(
                            usize::try_from(*left).expect("test index must fit usize"),
                            usize::try_from(*right).expect("test index must fit usize"),
                        )
                        .expect("test hierarchy lookup must succeed");
                    common.is_some_and(|common| {
                        common != usize::try_from(*left).expect("test index must fit usize")
                            && common != usize::try_from(*right).expect("test index must fit usize")
                            && states
                                .get(common)
                                .expect("common ancestor must identify a test state")
                                .kind
                                == StateKind::Parallel
                    })
                })
            });
            let mut budget = MultiTargetBudget::new();
            assert_eq!(
                validate_multi_target(&targets, &states, &hierarchy, &mut budget).is_ok(),
                all_pairs_valid,
                "adjacent and all-pairs validation must agree for targets {targets:?}"
            );
        }
    }
}
