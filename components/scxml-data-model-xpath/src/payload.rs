//! Pure value and payload helpers shared by the model and its hosts.
//!
//! No spans: these run inside public operations or are cheap host-side
//! construction and formatting helpers (including [`event_data`]) that
//! never touch state; their failures surface through the caller.

use scxml_types::DataValue;
use scxml_xpath::{NodeKind, NodeRef, Store, number_to_string};

use crate::{DataModelError, MAX_VALUE_BYTES};

/// Builds an SCXML `XPath` `_event.data` payload from `<param>` and
/// `namelist` entries (SCXML 1.0 CR 2014-03-13 §C.3.7.1): one
/// `<data id="name">value</data>` element per entry, in order. Node-set
/// values ([`DataValue::Xml`]) are copied as markup, scalars become text.
///
/// # Errors
///
/// [`DataModelError::UnsupportedValue`] for JSON arrays, which no `XPath`
/// expression evaluates to (a received array is still handled as
/// `_event.data`, [`accepts_event_data`]), and [`DataModelError::InvalidInput`] for
/// malformed markup or a payload over [`MAX_VALUE_BYTES`].
pub fn event_data(entries: &[(String, DataValue)]) -> Result<DataValue, DataModelError> {
    let mut out = String::new();
    for (name, value) in entries {
        out.push_str("<data id=\"");
        escape(&mut out, name, Escape::Attribute);
        out.push_str("\">");
        push_value_markup(&mut out, value)?;
        out.push_str("</data>");
        if out.len() > MAX_VALUE_BYTES {
            return Err(DataModelError::InvalidInput);
        }
    }
    // The concatenation of well-formed pieces is well-formed; parse it
    // once so a caller-supplied XML value can never smuggle markup.
    Store::new()
        .parse_fragment(&out)
        .map_err(|_| DataModelError::InvalidInput)?;
    Ok(DataValue::Xml(out))
}

/// Whether the `XPath` data model can handle `data` as `_event.data`
/// (SCXML 1.0 C.1: "If the receiving Processor cannot handle the data
/// format contained in the message, the receiving Processor MUST place the
/// error error.communication in internal queue of the session for which the
/// message was intended and MUST ignore the message."): whether the
/// `_event` `<data>` child can be built from it exactly as a bound event
/// builds it. SCXML 1.0 CR 2014-03-13 §C.3.7.1 gives every content a
/// translation, ending in "Otherwise, the Processor MUST treat the content
/// as a space-normalized string literal", so every value within the
/// portable event bounds (JSON arrays included) is handled.
#[must_use]
pub fn accepts_event_data(data: &DataValue) -> bool {
    crate::model::event_data_binds(data)
}

/// The value `_event.data` carries under `name` (SCXML 6.5.2 "any return
/// value that has a name that matches"): the content of the last top-level
/// `<data id="name">` element of an XML payload (SCXML 1.0 CR 2014-03-13
/// §C.3.7.1 builds one per `<param>`/`namelist` entry), as markup when it
/// holds an element and as text otherwise, or the member of a JSON object
/// payload. `None` when the data carries no such value.
#[must_use]
pub fn event_data_field(data: &DataValue, name: &str) -> Option<DataValue> {
    let DataValue::Xml(markup) = data else {
        return data.json_object_member(name);
    };
    let mut store = Store::new();
    let nodes = store.parse_fragment(markup).ok()?;
    let id = scxml_xpath::QName::local("id").ok()?;
    let mut found = None;
    for node in nodes {
        let is_data = store
            .name(NodeRef::node(node))
            .ok()
            .flatten()
            .is_some_and(|element| element.lexical() == "data");
        if is_data && store.attribute(node, &id).ok().flatten() == Some(name) {
            found = Some(node);
        }
    }
    let node = found?;
    let mut content = String::new();
    store.serialize_children(node, &mut content).ok()?;
    let has_element = store
        .children(node)
        .ok()?
        .any(|child| store.kind(NodeRef::node(child)) == Ok(NodeKind::Element));
    Some(if has_element {
        DataValue::Xml(content)
    } else {
        DataValue::String(store.string_value(NodeRef::node(node)).ok()?)
    })
}

/// Builds the canonical [`crate::XPathDataModel::initialize`] source that
/// declares each id, in order, as an empty `<data id="…"/>` variable.
#[must_use]
pub fn declaration_source<'a>(ids: impl IntoIterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for id in ids {
        out.push_str("<data id=\"");
        escape(&mut out, id, Escape::Attribute);
        out.push_str("\"/>");
    }
    out
}

/// Returns the payload name of one `XPath` `namelist` entry: the variable
/// name of a `$name` reference. `None` when the entry is not a plain
/// variable reference (SCXML 6.2.4 then raises `error.execution`).
#[must_use]
pub fn namelist_name(location: &str) -> Option<&str> {
    location
        .strip_prefix('$')
        .filter(|name| scxml_xpath::is_ncname(name))
}

/// Interprets external or inline data text for the `XPath` data model: text
/// that is a well-formed XML fragment containing an element becomes an XML
/// value (a node-set), anything else stays a string (SCXML 5.3).
#[must_use]
pub fn markup_value(text: &str) -> DataValue {
    let mut store = Store::new();
    let is_markup = text.len() <= MAX_VALUE_BYTES
        && store.parse_fragment(text).is_ok_and(|nodes| {
            nodes
                .iter()
                .any(|node| store.kind(NodeRef::node(*node)) == Ok(NodeKind::Element))
        });
    if is_markup {
        DataValue::Xml(text.to_owned())
    } else {
        DataValue::String(text.to_owned())
    }
}

/// Maximum JSON object nesting mapped onto nested `<data>` elements, here
/// and when `_event.data` is bound.
pub(crate) const MAX_JSON_DEPTH: usize = 32;

/// Appends the markup of one `<param>` or `namelist` value as element
/// content ([`event_data`]). A JSON object maps onto one `<data id="name">`
/// element per member, nesting objects; a JSON array is unsupported (no
/// `XPath` expression evaluates to one, so the sender raises
/// `error.execution`).
fn push_value_markup(out: &mut String, value: &DataValue) -> Result<(), DataModelError> {
    match value {
        DataValue::Absent | DataValue::Undefined | DataValue::Null => {}
        DataValue::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        DataValue::Number(value) => out.push_str(&number_to_string(value.get())),
        DataValue::String(value) => escape(out, value, Escape::Text),
        DataValue::Xml(value) => out.push_str(value),
        DataValue::JsonObject(text) => {
            let object: serde_json::Value =
                serde_json::from_str(text).map_err(|_| DataModelError::InvalidInput)?;
            push_json_members(out, &object, 0)?;
        }
        DataValue::JsonArray(_) => return Err(DataModelError::UnsupportedValue),
    }
    Ok(())
}

fn push_json_members(
    out: &mut String,
    object: &serde_json::Value,
    depth: usize,
) -> Result<(), DataModelError> {
    let serde_json::Value::Object(members) = object else {
        return Err(DataModelError::UnsupportedValue);
    };
    if depth >= MAX_JSON_DEPTH {
        return Err(DataModelError::UnsupportedValue);
    }
    for (name, value) in members {
        out.push_str("<data id=\"");
        escape(out, name, Escape::Attribute);
        out.push_str("\">");
        match value {
            serde_json::Value::Null => {}
            serde_json::Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            serde_json::Value::Number(number) => {
                let number = number.as_f64().ok_or(DataModelError::UnsupportedValue)?;
                out.push_str(&number_to_string(number));
            }
            serde_json::Value::String(text) => escape(out, text, Escape::Text),
            serde_json::Value::Object(_) => push_json_members(out, value, depth + 1)?,
            serde_json::Value::Array(_) => return Err(DataModelError::UnsupportedValue),
        }
        out.push_str("</data>");
        if out.len() > MAX_VALUE_BYTES {
            return Err(DataModelError::InvalidInput);
        }
    }
    Ok(())
}

/// The space-normalized form of `text` (SCXML 1.0 CR 2014-03-13 §C.3.1 and
/// §C.3.7.1 "space-normalized string literal"), as `XPath` 1.0
/// `normalize-space()` defines it: leading and trailing XML whitespace
/// (space, tab, carriage return, line feed) stripped and every internal run
/// of it replaced by one space.
pub(crate) fn space_normalized(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text
        .split([' ', '\t', '\r', '\n'])
        .filter(|word| !word.is_empty())
    {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// Scalar text of one value; `None` for XML and JSON values.
pub(crate) fn scalar_text(value: &DataValue) -> Option<String> {
    match value {
        DataValue::Absent | DataValue::Undefined | DataValue::Null => Some(String::new()),
        DataValue::Bool(value) => Some(if *value { "true" } else { "false" }.to_owned()),
        DataValue::Number(value) => Some(number_to_string(value.get())),
        DataValue::String(value) => Some(value.clone()),
        DataValue::Xml(_) | DataValue::JsonArray(_) | DataValue::JsonObject(_) => None,
    }
}

/// Where escaped text lands in the markup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Escape {
    /// Element content.
    Text,
    /// A double-quoted attribute value.
    Attribute,
}

/// XML-escapes `text`; attribute values also escape quotes and the
/// whitespace characters attribute normalization would collapse.
pub(crate) fn escape(out: &mut String, text: &str, context: Escape) {
    let attribute = context == Escape::Attribute;
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '\t' if attribute => out.push_str("&#9;"),
            '\n' if attribute => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            other => out.push(other),
        }
    }
}
