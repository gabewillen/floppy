//! Literal event data against literal `event:schema`s (`EVENT009`,
//! `EVENT010`).
//!
//! In an ECMAScript document (`datamodel="ecmascript"`, or none), the data a
//! `<send>` or `<donedata>` creates from literal JSON `<content>` (no `expr`,
//! text only, parsed after the data model's own white-space normalization) is
//! known before the document runs, and so is a literal `event:schema` (inline,
//! or a file whose bytes were supplied):
//!
//! * `EVENT009` (error): the literal data does not satisfy the literal schema
//!   of its own `<send>` or `<donedata>`.
//! * `EVENT010` (warning): a `<send event="x">` delivered to its own session
//!   (no `target` or `#_internal`, the SCXML Event I/O Processor) or a
//!   `<donedata>` of a compound state (its `done.state.<id>`) creates data a
//!   transition of the same document handling that event (SCXML 3.12.1
//!   descriptor matching) does not accept: its literal data, or an instance
//!   of its own literal schema, fails the transition's literal schema. Two
//!   equal schemas are compatible; otherwise only a witness is reported, an
//!   instance the producer's schema admits and the transition's rejects,
//!   drawn from the producer's schema (`const`, `enum`, `examples`,
//!   `default`, and a minimal instance of each `type`), so a finding is never
//!   a guess.
//!
//! `event:schemaexpr` is skipped silently, as is every schema that does not
//! resolve (`EVENT003`-`EVENT008` report it). Nested documents (an
//! `<invoke>`'s inline `<content>`) are not part of the document. Pure and
//! bounded by the document and its schemas; no telemetry.

use std::collections::BTreeMap;

use scxml_event_schema::{
    CompiledSchema, Resolved, SCHEMA_ATTRIBUTE, SCHEMA_EXPR_ATTRIBUTE, Source, canonical_json,
};
use scxml_syntax::{Attribute, Document, Element, Node, Span};
use scxml_types::{DataValue, EVENT_NAMESPACE_URI, FiniteNumber};
use serde_json::Value;

use crate::{
    DEFAULT_DATA_MODEL, Diagnostic, DiagnosticCategory, EventSchemaResources, Label,
    SCXML_NAMESPACE,
};

/// Upper bound on the witnesses drawn from one schema.
const MAX_WITNESSES: usize = 32;
/// Upper bound on the nesting of one minimal instance.
const MAX_WITNESS_DEPTH: usize = 6;
/// Upper bound on the bytes of a witness quoted in a message.
const MAX_QUOTED_BYTES: usize = 200;

/// The resolved literal `event:schema` of `element`: `None` without one,
/// with `event:schemaexpr`, or when it does not resolve.
pub(crate) fn literal_schema(
    element: &Element,
    resources: &EventSchemaResources,
) -> Option<Resolved> {
    schema_attribute(element).and_then(|attribute| resolve(attribute, resources))
}

fn schema_attribute(element: &Element) -> Option<&Attribute> {
    let mut schema = None;
    for attribute in &element.attributes {
        if attribute.name.namespace_uri.as_deref() != Some(EVENT_NAMESPACE_URI) {
            continue;
        }
        match attribute.name.local_name.as_str() {
            SCHEMA_ATTRIBUTE => schema = Some(attribute),
            SCHEMA_EXPR_ATTRIBUTE => return None,
            _ => {}
        }
    }
    schema
}

fn resolve(attribute: &Attribute, resources: &EventSchemaResources) -> Option<Resolved> {
    match scxml_event_schema::source(&attribute.value) {
        Source::Inline(text) => scxml_event_schema::resolve(text.as_bytes(), None).ok(),
        Source::Reference { resource, fragment } => {
            if scxml_event_schema::resource_problem(resource).is_some() {
                return None;
            }
            scxml_event_schema::resolve(resources.get(resource)?, fragment).ok()
        }
    }
}

/// One literal schema and the attribute that gives it.
struct Schema<'a> {
    resolved: Resolved,
    attribute: &'a Attribute,
}

/// One transition with a literal schema.
struct Handler<'a> {
    descriptors: Vec<&'a str>,
    schema: Schema<'a>,
}

/// One `<send>` or `<donedata>`.
struct Producer<'a> {
    element: &'a Element,
    /// The event it creates when the document's own transitions see it.
    event: Option<String>,
    schema: Option<Schema<'a>>,
    /// Its literal data and the `<content>` that gives it.
    payload: Option<(Value, &'a Element)>,
}

/// Every `EVENT009` and `EVENT010` finding of `document`.
pub(crate) fn event_payload_diagnostics(
    document: &Document,
    resources: &EventSchemaResources,
) -> Vec<Diagnostic> {
    let datamodel = document
        .root
        .attribute("datamodel")
        .map_or(DEFAULT_DATA_MODEL, |attribute| attribute.value.as_str());
    if datamodel != "ecmascript" {
        return Vec::new();
    }
    let mut handlers = Vec::new();
    let mut producers = Vec::new();
    collect(
        &document.root,
        &mut Vec::new(),
        resources,
        &mut handlers,
        &mut producers,
    );
    let mut compiled = Compiled::default();
    let mut diagnostics = Vec::new();
    for producer in &producers {
        if let (Some((payload, content)), Some(schema)) = (&producer.payload, &producer.schema)
            && compiled.rejects(&schema.resolved.schema, payload)
        {
            diagnostics.push(own_violation(producer.element, content, schema, payload));
        }
        let Some(event) = &producer.event else {
            continue;
        };
        for handler in handlers.iter().filter(|handler| {
            handler
                .descriptors
                .iter()
                .any(|descriptor| descriptor_matches(descriptor, event))
        }) {
            let rejected = match (&producer.payload, &producer.schema) {
                (Some((payload, content)), _) => compiled
                    .rejects(&handler.schema.resolved.schema, payload)
                    .then(|| (payload.clone(), content.span, true)),
                (None, Some(schema))
                    if schema.resolved.schema != handler.schema.resolved.schema =>
                {
                    compiled
                        .witness(&schema.resolved.schema, &handler.schema.resolved.schema)
                        .map(|witness| (witness, schema.attribute.span, false))
                }
                (None, _) => None,
            };
            if let Some((witness, span, literal)) = rejected {
                diagnostics.push(incompatible(
                    producer.element,
                    event,
                    span,
                    handler,
                    &witness,
                    literal,
                ));
            }
        }
    }
    diagnostics
}

/// Collects the handlers and producers of one machine, never descending
/// into an SCXML `<content>` (another document).
fn collect<'a>(
    element: &'a Element,
    ancestors: &mut Vec<&'a Element>,
    resources: &EventSchemaResources,
    handlers: &mut Vec<Handler<'a>>,
    producers: &mut Vec<Producer<'a>>,
) {
    let scxml = element.name.namespace_uri.as_deref() == Some(SCXML_NAMESPACE);
    if scxml {
        let schema = || {
            schema_attribute(element).and_then(|attribute| {
                resolve(attribute, resources).map(|resolved| Schema {
                    resolved,
                    attribute,
                })
            })
        };
        match element.name.local_name.as_str() {
            "content" => return,
            "transition" => {
                if let (Some(event), Some(schema)) = (element.attribute("event"), schema()) {
                    handlers.push(Handler {
                        descriptors: event.value.split_ascii_whitespace().collect(),
                        schema,
                    });
                }
            }
            "send" => {
                let event = element
                    .attribute("event")
                    .filter(|_| delivered_to_self(element))
                    .map(|attribute| attribute.value.clone());
                producers.push(Producer {
                    element,
                    event,
                    schema: schema(),
                    payload: literal_payload(element),
                });
            }
            "donedata" => {
                producers.push(Producer {
                    element,
                    event: done_state_event(ancestors),
                    schema: schema(),
                    payload: literal_payload(element),
                });
            }
            _ => {}
        }
    }
    ancestors.push(element);
    for child in &element.children {
        if let Node::Element(child) = child {
            collect(child, ancestors, resources, handlers, producers);
        }
    }
    ancestors.pop();
}

/// Whether a `<send>` delivers its event to its own session.
fn delivered_to_self(send: &Element) -> bool {
    let target = send
        .attribute("target")
        .map(|attribute| attribute.value.as_str());
    let kind = send
        .attribute("type")
        .map(|attribute| attribute.value.as_str());
    send.attribute("targetexpr").is_none()
        && send.attribute("typeexpr").is_none()
        && matches!(target, None | Some("#_internal"))
        && matches!(
            kind,
            None | Some("scxml" | "http://www.w3.org/TR/scxml/#SCXMLEventProcessor")
        )
}

/// `done.state.<id>` of the state a `<final>` holding a `<donedata>`
/// completes (`ancestors` ends with the final); `None` for a top-level
/// final, whose data goes to the invoker, and for a state without an `id`.
fn done_state_event(ancestors: &[&Element]) -> Option<String> {
    let [.., state, final_state] = ancestors else {
        return None;
    };
    (final_state.name.is(Some(SCXML_NAMESPACE), "final")
        && state.name.namespace_uri.as_deref() == Some(SCXML_NAMESPACE)
        && state.name.local_name == "state")
        .then(|| state.attribute("id"))
        .flatten()
        .map(|id| format!("done.state.{}", id.value))
}

/// The literal JSON data of a `<send>` or `<donedata>`'s `<content>`.
fn literal_payload(element: &Element) -> Option<(Value, &Element)> {
    let mut contents = element.children.iter().filter_map(|child| match child {
        Node::Element(child)
            if child.name.namespace_uri.as_deref() == Some(SCXML_NAMESPACE)
                && child.name.local_name == "content" =>
        {
            Some(child)
        }
        _ => None,
    });
    let content = contents.next()?;
    if contents.next().is_some() || content.attribute("expr").is_some() {
        return None;
    }
    let mut text = String::new();
    for child in &content.children {
        match child {
            Node::Text(part) => text.push_str(&part.value),
            Node::Element(_) => return None,
        }
    }
    let value = serde_json::from_str::<Value>(&normalize_whitespace(&text)).ok()?;
    Some((value, content))
}

/// The ECMAScript data model's normalization of `<content>` text before it
/// is parsed as JSON.
fn normalize_whitespace(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut pending_space = false;
    for character in text.chars() {
        if matches!(character, ' ' | '\t' | '\r' | '\n') {
            pending_space = !normalized.is_empty();
        } else {
            if pending_space {
                normalized.push(' ');
                pending_space = false;
            }
            normalized.push(character);
        }
    }
    normalized
}

/// SCXML 3.12.1 event descriptor matching.
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

/// Schemas compiled once each; one that does not compile checks nothing.
#[derive(Default)]
struct Compiled {
    schemas: BTreeMap<String, Option<CompiledSchema>>,
}

impl Compiled {
    fn get(&mut self, schema: &str) -> Option<&CompiledSchema> {
        self.schemas
            .entry(schema.to_owned())
            .or_insert_with(|| CompiledSchema::new(schema).ok())
            .as_ref()
    }

    /// Whether `schema` provably rejects `value`.
    fn rejects(&mut self, schema: &str, value: &Value) -> bool {
        let Some(data) = data_value(value) else {
            return false;
        };
        self.get(schema)
            .is_some_and(|compiled| !compiled.admits(&data))
    }

    fn admits(&mut self, schema: &str, value: &Value) -> bool {
        let Some(data) = data_value(value) else {
            return false;
        };
        self.get(schema)
            .is_some_and(|compiled| compiled.admits(&data))
    }

    /// An instance `producer` admits and `handler` rejects.
    fn witness(&mut self, producer: &str, handler: &str) -> Option<Value> {
        let schema: Value = serde_json::from_str(producer).ok()?;
        let mut candidates = Vec::new();
        candidates_of(&schema, 0, &mut candidates);
        candidates
            .into_iter()
            .find(|candidate| self.admits(producer, candidate) && self.rejects(handler, candidate))
    }
}

/// The event data a JSON value is.
fn data_value(value: &Value) -> Option<DataValue> {
    Some(match value {
        Value::Null => DataValue::Null,
        Value::Bool(value) => DataValue::Bool(*value),
        Value::Number(number) => DataValue::Number(FiniteNumber::new(number.as_f64()?).ok()?),
        Value::String(text) => DataValue::String(text.clone()),
        Value::Array(_) => DataValue::JsonArray(canonical_json(value)),
        Value::Object(_) => DataValue::JsonObject(canonical_json(value)),
    })
}

/// Candidate instances of `schema`: its `const`, `enum`, `examples`, and
/// `default` values, then a minimal instance of each `type`.
fn candidates_of(schema: &Value, depth: usize, out: &mut Vec<Value>) {
    let push = |out: &mut Vec<Value>, value: Value| {
        if out.len() < MAX_WITNESSES && !out.contains(&value) {
            out.push(value);
        }
    };
    let object = match schema {
        Value::Bool(true) => {
            push(out, Value::Null);
            return;
        }
        Value::Object(object) => object,
        _ => return,
    };
    if let Some(value) = object.get("const") {
        push(out, value.clone());
    }
    for key in ["enum", "examples"] {
        if let Some(Value::Array(values)) = object.get(key) {
            for value in values {
                push(out, value.clone());
            }
        }
    }
    if let Some(value) = object.get("default") {
        push(out, value.clone());
    }
    let types: Vec<&str> = match object.get("type") {
        Some(Value::String(name)) => vec![name.as_str()],
        Some(Value::Array(names)) => names.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    for name in types {
        if let Some(value) = minimal(name, object, depth) {
            push(out, value);
        }
    }
}

/// A minimal instance of `type` `name` under `schema`'s simplest keywords.
fn minimal(name: &str, schema: &serde_json::Map<String, Value>, depth: usize) -> Option<Value> {
    let count = |key: &str| {
        schema
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(0)
    };
    Some(match name {
        "null" => Value::Null,
        "boolean" => Value::Bool(false),
        "integer" | "number" => schema
            .get("minimum")
            .filter(|minimum| minimum.is_number())
            .cloned()
            .unwrap_or_else(|| Value::from(0)),
        "string" => Value::String("a".repeat(count("minLength").min(64))),
        "array" => {
            let items = count("minItems").min(8);
            if items == 0 {
                Value::Array(Vec::new())
            } else {
                let item = first_candidate(schema.get("items")?, depth)?;
                Value::Array(vec![item; items])
            }
        }
        "object" => {
            let mut members = serde_json::Map::new();
            if let Some(Value::Array(required)) = schema.get("required") {
                for member in required.iter().filter_map(Value::as_str).take(32) {
                    let property = schema
                        .get("properties")
                        .and_then(|properties| properties.get(member))
                        .unwrap_or(&Value::Bool(true));
                    members.insert(member.to_owned(), first_candidate(property, depth)?);
                }
            }
            Value::Object(members)
        }
        _ => return None,
    })
}

fn first_candidate(schema: &Value, depth: usize) -> Option<Value> {
    if depth >= MAX_WITNESS_DEPTH {
        return None;
    }
    let mut candidates = Vec::new();
    candidates_of(schema, depth + 1, &mut candidates);
    candidates.into_iter().next()
}

fn quoted(value: &Value) -> String {
    let text = canonical_json(value);
    if text.len() <= MAX_QUOTED_BYTES {
        return text;
    }
    let mut end = MAX_QUOTED_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", text.get(..end).unwrap_or_default())
}

fn own_violation(
    element: &Element,
    content: &Element,
    schema: &Schema<'_>,
    payload: &Value,
) -> Diagnostic {
    Diagnostic::error(
        "EVENT009",
        format!(
            "the literal data of `<{}>` does not satisfy its own `event:schema`",
            element.lexical_name
        ),
        content.span,
    )
    .with_category(DiagnosticCategory::Semantic)
    .with_primary_message(format!("`{}` is rejected", quoted(payload)))
    .with_secondary(Label::new(schema.attribute.span).with_message("the schema it must satisfy"))
    .with_note("the `<content>` is literal JSON, so the event data it creates is known before the document runs, and the element's literal `event:schema` describes that data")
    .with_help("change the `<content>` to data the schema admits, or correct the schema")
    .with_spec_reference("JSON Schema 2020-12 Validation")
}

fn incompatible(
    producer: &Element,
    event: &str,
    span: Span,
    handler: &Handler<'_>,
    witness: &Value,
    literal: bool,
) -> Diagnostic {
    let what = if literal {
        "its literal data"
    } else {
        "an instance of its `event:schema`"
    };
    Diagnostic::warning(
        "EVENT010",
        format!(
            "`<{}>` creates `{event}` data that a transition handling it does not accept",
            producer.lexical_name
        ),
        span,
    )
    .with_category(DiagnosticCategory::Semantic)
    .with_primary_message(format!("{what}, `{}`, ...", quoted(witness)))
    .with_secondary(
        Label::new(handler.schema.attribute.span)
            .with_message("... fails the schema of this transition"),
    )
    .with_note("a transition's `event:schema` describes the `_event.data` it handles; this data matches its event descriptor but not its schema (the schemas are metadata: the transition is still taken)")
    .with_help("make the two schemas agree, or change the data")
    .with_spec_reference("SCXML 1.0 §3.12.1 Event Descriptors")
}

#[cfg(test)]
mod tests;
