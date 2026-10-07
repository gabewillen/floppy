//! Pure-Rust XSD 1.0 validation backend for `scxml-validator-core`.
//!
//! Schemas and instance bytes are supplied by the caller. This backend does not
//! install a filesystem or network resolver, so imports/includes that are not
//! already represented by the supplied schema set fail closed. Before invoking
//! the external backend, a deterministic preflight bounds total schema nodes,
//! compositor/occurrence expansion, and projected document-validation work.
//!
//! # Example
//!
//! Plug the backend into `scxml-validator-core` with the bundled official W3C
//! SCXML 1.0 schema set as the fallback for the exact SCXML namespace, or
//! call it directly with [`w3c_scxml_schema_set`]:
//!
//! ```
//! use scxml_syntax::{SourceId, SourceText, parse_xml};
//! use scxml_validator_core::{MemorySchemaResolver, SchemaValidator as _, Validator};
//! use scxml_validator_xsd::{BundledSchemaResolver, XsdSchemaBackend, w3c_scxml_schema_set};
//!
//! let validator = Validator::new(
//!     BundledSchemaResolver::new(MemorySchemaResolver::new()),
//!     XsdSchemaBackend,
//! );
//! let valid = validator.validate_source(
//!     SourceId::new(1),
//!     "ok.scxml",
//!     r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a">
//!   <state id="a"><transition event="go" target="b"/></state>
//!   <final id="b"/>
//! </scxml>"#,
//! );
//! assert!(!valid.report.has_errors());
//!
//! // `priority` is not an attribute of `<state>`: an XSD structure error.
//! let invalid_text = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0">
//!   <state id="a" priority="high"/>
//! </scxml>"#;
//! let invalid = validator.validate_source(SourceId::new(2), "bad.scxml", invalid_text);
//! assert!(invalid.report.has_errors());
//!
//! // The backend alone reports the same structure error against the set.
//! let document = parse_xml(SourceText::new(SourceId::new(3), "bad.scxml", invalid_text))
//!     .expect("the document is well-formed XML");
//! let report = XsdSchemaBackend
//!     .validate(&document, &w3c_scxml_schema_set())
//!     .expect("the bundled schema set compiles");
//! assert!(report.has_errors());
//! for diagnostic in report.diagnostics() {
//!     assert!(invalid.report.diagnostics().iter().any(|seen| seen.code == diagnostic.code));
//! }
//! ```

#![forbid(unsafe_code)]

mod bundled;
mod project;
pub mod vocabulary;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

pub use bundled::{
    BundledSchemaDocument, BundledSchemaResolver, W3C_SCXML_SCHEMA_BASE_URL,
    W3C_SCXML_SCHEMA_DOCUMENTS, w3c_scxml_schema_set,
};
use scxml_syntax::telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};
use scxml_syntax::{
    Document, Element, Node, SourceId, SourceText, XmlLimits, parse_xml_with_limits,
    validate_document_limits,
};
use scxml_types::wasm_bound;
use scxml_validator_core::{
    Report, SchemaBackendError, SchemaSource, SchemaValidator as BackendContract,
};
use xsd_schema::validation::{SchemaValidator, ValidationFlags, drive_quick_xml_with};
use xsd_schema::{SchemaSet, parse_schema_only, process_loaded_schemas};

/// XSD 1.0 backend implemented by `xsd-schema` 0.2.x with optional features off.
#[derive(Clone, Copy, Debug, Default)]
pub struct XsdSchemaBackend;

// Schema count, size, node, message, and document-work limits are resource
// bounds of wasm builds only (`scxml_types::wasm_bound`); natively they are
// unbounded. The content-model expansion and schema compilation limits apply
// to every build: `maxOccurs` expands a content model multiplicatively and
// compilation is quadratic, so they guard against amplification.
const MAX_SCHEMA_COUNT: usize = wasm_bound(64);
const MAX_SCHEMA_BYTES: usize = wasm_bound(4 * 1024 * 1024);
const MAX_TOTAL_SCHEMA_BYTES: usize = wasm_bound(16 * 1024 * 1024);
const MAX_VALIDATION_MESSAGES: usize = wasm_bound(10_000);
const MAX_TOTAL_SCHEMA_NODES: u64 = wasm_bound(250_000) as u64;
const MAX_CONTENT_MODEL_EXPANSION: u64 = 1_000_000;
/// Quadratic projection of automaton construction over schema nodes plus
/// content-model positions. The bundled official W3C SCXML 1.0 schema set
/// measures 117,570,649 units (pinned by a unit test), so the budget keeps
/// roughly 2x headroom for extension-namespace schemas composed with it.
const MAX_SCHEMA_COMPILATION_WORK: u64 = 256_000_000;
/// Linear instance-validation budget: source bytes plus document work nodes
/// (elements, attributes, text nodes) times the widest single content model.
/// The backend rejects non-deterministic content models (UPA, `XSD003`), so
/// each instance event advances one content-model position and matches at
/// most the widest model's positions. With the bundled W3C schema set every
/// document inside `XmlLimits::DEFAULT` fits with about 2x headroom (widest
/// model 815 positions, pinned by a unit test); the
/// headroom admits extension-namespace schemas with wider models.
const MAX_DOCUMENT_VALIDATION_WORK: u64 = wasm_bound(2_000_000_000) as u64;
const XML_SCHEMA_NAMESPACE: &str = "http://www.w3.org/2001/XMLSchema";
const XSD_XML_LIMITS: XmlLimits = XmlLimits {
    max_source_bytes: MAX_SCHEMA_BYTES,
    max_depth: 256,
    max_events: wasm_bound(500_000),
    max_nodes: wasm_bound(125_000),
    max_attributes: wasm_bound(500_000),
};

impl BackendContract for XsdSchemaBackend {
    fn validate(
        &self,
        document: &Document,
        schemas: &[SchemaSource],
    ) -> Result<Report, SchemaBackendError> {
        let span = observe(Module::Validator, Operation::ValidatorXsdValidate);
        let result = validate_bounded(document, schemas);
        match &result {
            Ok(report) if report.has_errors() => {
                span.event(SignalEvent::InputRejected);
                span.fail(ErrorCategory::Validation);
            }
            Ok(_) => span.event(SignalEvent::InputAccepted),
            Err(error) => {
                span.event(SignalEvent::InputRejected);
                span.fail(if matches!(error.code.as_str(), "XSD006" | "XSD007") {
                    ErrorCategory::Budget
                } else {
                    ErrorCategory::Validation
                });
            }
        }
        result
    }
}

// Keep schema preparation, one bounded driver run, and diagnostic projection in
// one function so all borrowed backend state is dropped before returning.
#[allow(
    clippy::too_many_lines,
    reason = "schema preparation, one bounded driver run, and projection share borrowed backend state that must drop before returning"
)]
fn validate_bounded(
    document: &Document,
    schemas: &[SchemaSource],
) -> Result<Report, SchemaBackendError> {
    validate_document_limits(document, XmlLimits::DEFAULT)
        .map_err(|error| SchemaBackendError::new("XSD006", error.message))?;
    if scxml_types::exceeds_wasm_bound(schemas.len(), MAX_SCHEMA_COUNT) {
        return Err(limit_error("schema count", MAX_SCHEMA_COUNT));
    }
    let mut total_schema_bytes = 0usize;
    let mut schema_documents = Vec::with_capacity(schemas.len());
    for (index, schema) in schemas.iter().enumerate() {
        if scxml_types::exceeds_wasm_bound(schema.bytes.len(), MAX_SCHEMA_BYTES) {
            return Err(limit_error("schema bytes", MAX_SCHEMA_BYTES));
        }
        total_schema_bytes = total_schema_bytes
            .checked_add(schema.bytes.len())
            .ok_or_else(|| limit_error("total schema bytes", MAX_TOTAL_SCHEMA_BYTES))?;
        if scxml_types::exceeds_wasm_bound(total_schema_bytes, MAX_TOTAL_SCHEMA_BYTES) {
            return Err(limit_error("total schema bytes", MAX_TOTAL_SCHEMA_BYTES));
        }
        let schema_text = std::str::from_utf8(&schema.bytes)
            .map_err(|_| SchemaBackendError::new("XSD002", "schema source is not UTF-8 XML"))?;
        let source_index =
            u32::try_from(index).map_err(|_| limit_error("schema count", MAX_SCHEMA_COUNT))?;
        let schema_document = parse_xml_with_limits(
            SourceText::new(
                SourceId::new(source_index),
                schema.system_id.clone(),
                schema_text.to_owned(),
            ),
            XSD_XML_LIMITS,
        )
        .map_err(|error| {
            SchemaBackendError::new("XSD006", format!("schema XML limit failed: {error}"))
        })?;
        schema_documents.push(schema_document);
    }
    preflight_xsd_work(document, &schema_documents, XsdWorkLimits::DEFAULT)
        .map_err(PreflightError::into_backend_error)?;
    let mut schema_set = SchemaSet::new();
    for schema in schemas {
        parse_schema_only(&schema.bytes[..], &schema.system_id, &mut schema_set).map_err(
            |error| {
                SchemaBackendError::new(
                    "XSD002",
                    format!(
                        "schema parse failed: {}",
                        stable_backend_message(&error.to_string())
                    ),
                )
            },
        )?;
    }
    process_loaded_schemas(&mut schema_set).map_err(|error| {
        SchemaBackendError::new(
            "XSD003",
            format!(
                "schema compilation failed: {}",
                stable_backend_message(&error.to_string())
            ),
        )
    })?;

    let validator = SchemaValidator::new(&schema_set, ValidationFlags::default());
    if let Some((_, reason)) = validator.content_model_failures().first() {
        return Err(SchemaBackendError::new(
            "XSD004",
            format!("schema content model could not be prepared: {reason}"),
        ));
    }
    // XML Schema 1.0 Part 1 §2.1: "The input to schema-validity assessment
    // is an information set"; the backend's instance driver reads XML text
    // and knows no DTD. A document whose internal DTD subset changes its
    // infoset (included entities, supplied defaults, normalized values,
    // XML 1.0 §5.1) is validated as the serialization of the infoset the
    // parser built, and so is one whose namespace or `<invoke type>` URIs
    // pinned a release (`@<tag>`, split off when the document was parsed,
    // `scxml_types::strip_tag`): its elements are in the tagless namespace
    // a package's schema targets. Any other document is validated as its
    // source bytes.
    let infoset =
        (has_internal_subset(document) || has_tag(document)).then(|| infoset_text(document));
    let instance = infoset.as_deref().unwrap_or_else(|| document.source.text());
    let findings = RefCell::new(project::Findings::new(MAX_VALIDATION_MESSAGES));
    let declared;
    {
        let mut runtime = validator.start_run(project::SharedSink(&findings));
        let mut tagger = project::Tagger::new(&findings, &schema_set);
        drive_quick_xml_with(instance.as_bytes(), &mut runtime, &schema_set, &mut tagger).map_err(
            |error| SchemaBackendError::new("XSD005", format!("instance driver failed: {error}")),
        )?;
        let completion = runtime.end_validation();
        let mut findings = findings.borrow_mut();
        findings.claim(&project::Phase::Document);
        if let Err(error) = completion {
            return Err(SchemaBackendError::new(
                "XSD005",
                format!("instance validation could not complete: {}", error.message),
            ));
        }
        declared = tagger.declared;
    }
    let findings = findings.into_inner();
    if findings.truncated {
        return Err(limit_error("validation messages", MAX_VALIDATION_MESSAGES));
    }
    let room = MAX_VALIDATION_MESSAGES.saturating_sub(findings.tagged.len());
    let element_only_text = project::non_xml_space_text(document, room)
        .ok_or_else(|| limit_error("validation messages", MAX_VALIDATION_MESSAGES))?;
    let mut report = project::project(document, findings.tagged, &declared);
    for diagnostic in element_only_text {
        report.push(diagnostic);
    }
    Ok(report)
}

/// Whether the parser split a release tag off any namespace declaration or
/// attribute of the document.
fn has_tag(document: &Document) -> bool {
    let mut found = false;
    document.root.visit(&mut |element| {
        found = found
            || element
                .namespaces
                .iter()
                .any(|declaration| declaration.tag.is_some())
            || element
                .attributes
                .iter()
                .any(|attribute| attribute.tag.is_some());
    });
    found
}

/// Whether the prolog holds a document type declaration with an internal
/// subset, which may change the infoset (XML 1.0 §5.1).
fn has_internal_subset(document: &Document) -> bool {
    document
        .source
        .text()
        .get(..document.root.start_tag_span.start)
        .and_then(|prolog| prolog.find("<!DOCTYPE").and_then(|at| prolog.get(at..)))
        .is_some_and(|doctype| doctype.contains('['))
}

/// Serializes the parsed document's infoset as XML text with the same
/// elements in the same document order: every element with its namespace
/// declarations (supplied defaults included) and attributes (normalized
/// values, escaped so they read back unchanged), and every text node.
/// Comments, processing instructions, and the DTD carry nothing XML Schema
/// validates. Bounded by the parsed tree.
fn infoset_text(document: &Document) -> String {
    enum Step<'a> {
        Open(&'a Element),
        Text(&'a str),
        Close(&'a Element),
    }
    let mut out = String::with_capacity(document.source.text().len());
    let mut pending = vec![Step::Open(&document.root)];
    while let Some(step) = pending.pop() {
        match step {
            Step::Open(element) => {
                out.push('<');
                out.push_str(&element.lexical_name);
                for declaration in &element.namespaces {
                    out.push_str(" xmlns");
                    if let Some(prefix) = &declaration.prefix {
                        out.push(':');
                        out.push_str(prefix);
                    }
                    out.push_str("=\"");
                    escape_markup(&mut out, &declaration.uri, Escaping::AttributeValue);
                    out.push('"');
                }
                for attribute in &element.attributes {
                    out.push(' ');
                    out.push_str(&attribute.lexical_name);
                    out.push_str("=\"");
                    escape_markup(&mut out, &attribute.value, Escaping::AttributeValue);
                    out.push('"');
                }
                if element.children.is_empty() {
                    out.push_str("/>");
                    continue;
                }
                out.push('>');
                pending.push(Step::Close(element));
                for child in element.children.iter().rev() {
                    pending.push(match child {
                        Node::Element(child) => Step::Open(child),
                        Node::Text(text) => Step::Text(&text.value),
                    });
                }
            }
            Step::Text(text) => escape_markup(&mut out, text, Escaping::Text),
            Step::Close(element) => {
                out.push_str("</");
                out.push_str(&element.lexical_name);
                out.push('>');
            }
        }
    }
    out
}

/// Where escaped markup is written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Escaping {
    /// A double-quoted attribute value: quotes and whitespace are escaped too.
    AttributeValue,
    /// Character data.
    Text,
}

/// Appends `text` escaped as XML character data, or as a double-quoted
/// attribute value, so it reads back as exactly `text`: white space other
/// than a space is a character reference in a value (XML 1.0 §3.3.3 would
/// normalize it), and a carriage return everywhere (§2.11).
fn escape_markup(out: &mut String, text: &str, escaping: Escaping) {
    let attribute = escaping == Escaping::AttributeValue;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct XsdWorkSummary {
    schema_nodes: u64,
    content_model_expansion: u64,
    widest_content_model: u64,
    schema_compilation_work: u64,
    validation_work: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct XsdWorkLimits {
    schema_nodes: u64,
    content_model_expansion: u64,
    schema_compilation_work: u64,
    validation_work: u64,
}

impl XsdWorkLimits {
    const DEFAULT: Self = Self {
        schema_nodes: MAX_TOTAL_SCHEMA_NODES,
        content_model_expansion: MAX_CONTENT_MODEL_EXPANSION,
        schema_compilation_work: MAX_SCHEMA_COMPILATION_WORK,
        validation_work: MAX_DOCUMENT_VALIDATION_WORK,
    };

    #[cfg(test)]
    const UNBOUNDED_FOR_MEASUREMENT: Self = Self {
        schema_nodes: u64::MAX,
        content_model_expansion: u64::MAX,
        schema_compilation_work: u64::MAX,
        validation_work: u64::MAX,
    };

    #[cfg(test)]
    const fn from_summary(summary: XsdWorkSummary) -> Self {
        Self {
            schema_nodes: summary.schema_nodes,
            content_model_expansion: summary.content_model_expansion,
            schema_compilation_work: summary.schema_compilation_work,
            validation_work: summary.validation_work,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreflightError {
    Limit {
        resource: &'static str,
        maximum: u64,
    },
    /// The instance itself exceeds the linear validation budget.
    DocumentWork {
        work_nodes: u64,
        source_bytes: u64,
        widest_content_model: u64,
        maximum: u64,
    },
    InvalidOccurrence,
    UnresolvedGroupReference,
    CircularGroupReference,
}

impl PreflightError {
    fn into_backend_error(self) -> SchemaBackendError {
        match self {
            Self::Limit { resource, maximum } => SchemaBackendError::new(
                "XSD006",
                format!("XSD {resource} limit exceeded (maximum {maximum})"),
            ),
            Self::DocumentWork {
                work_nodes,
                source_bytes,
                widest_content_model,
                maximum,
            } => SchemaBackendError::new(
                "XSD006",
                format!(
                    "document is too large for XSD validation: {work_nodes} elements, attributes, and text nodes against a widest content model of {widest_content_model} positions, plus {source_bytes} source bytes, exceed the budget of {maximum} work units"
                ),
            )
            .with_help(
                "split the document into smaller machines (for example with `<invoke>` of child `<scxml>` documents), or narrow the widest content model of any extension-namespace schema; the document was not structurally validated",
            ),
            Self::InvalidOccurrence => SchemaBackendError::new(
                "XSD007",
                "XSD content-model occurrence is not a supported non-negative 32-bit value",
            ),
            Self::UnresolvedGroupReference => SchemaBackendError::new(
                "XSD007",
                "XSD model-group reference does not name a group defined in the supplied schema set",
            ),
            Self::CircularGroupReference => SchemaBackendError::new(
                "XSD007",
                "XSD model-group definitions reference each other circularly",
            ),
        }
    }
}

/// Computes a conservative deterministic work bound before `xsd-schema` sees
/// any bytes. Schema-node work includes elements, attributes, and text nodes.
/// Content-model work follows compositor nesting and the backend's occurrence
/// strategy: small finite ranges are expanded, large finite ranges retain their
/// exact counter range, and `unbounded` has constant star construction cost.
fn preflight_xsd_work(
    document: &Document,
    schemas: &[Document],
    limits: XsdWorkLimits,
) -> Result<XsdWorkSummary, PreflightError> {
    let mut schema_nodes = 0u64;
    let mut content_model_expansion = 0u64;
    // Expanded positions per content model: index 0 collects global element
    // declarations; every `complexType` opens its own model.
    let mut model_widths = vec![0u64];
    let group_costs = group_expansion_costs(schemas, limits)?;
    for schema in schemas {
        let mut pending = vec![(&schema.root, 1u64, 0usize)];
        while let Some((element, parent_expansion, model)) = pending.pop() {
            add_schema_node_work(element, &mut schema_nodes, limits.schema_nodes)?;
            let is_xsd = element.name.namespace_uri.as_deref() == Some(XML_SCHEMA_NAMESPACE);
            if is_xsd
                && element.name.local_name == "group"
                && let Some(name) = element.attribute("name")
            {
                // A named definition contributes its body's expansion once;
                // every reference below adds the use-site instantiation. The
                // body's nodes still count toward schema-node work.
                let cost = group_costs
                    .get(name.value.as_str())
                    .copied()
                    .ok_or(PreflightError::UnresolvedGroupReference)?;
                add_expansion(&mut content_model_expansion, cost, limits)?;
                add_group_body_nodes(element, &mut schema_nodes, limits.schema_nodes)?;
                continue;
            }
            if is_xsd
                && element.name.local_name == "group"
                && let Some(reference) = element.attribute("ref")
            {
                // A reference instantiates the referenced group's complete
                // content model at this occurrence.
                let cost = group_costs
                    .get(local_part(&reference.value))
                    .copied()
                    .ok_or(PreflightError::UnresolvedGroupReference)?;
                let expanded = parent_expansion
                    .checked_mul(occurrence_factor(element)?)
                    .and_then(|value| value.checked_mul(cost.max(1)))
                    .ok_or(PreflightError::Limit {
                        resource: "content-model expansion",
                        maximum: limits.content_model_expansion,
                    })?;
                add_expansion(&mut content_model_expansion, expanded, limits)?;
                add_model_width(&mut model_widths, model, expanded, limits)?;
                continue;
            }
            let mut child_model = model;
            let expansion = if is_xsd && is_content_model_node(&element.name.local_name) {
                let expanded = occurrence_factor(element)?
                    .checked_mul(compositor_branching_factor(element)?)
                    .and_then(|factor| parent_expansion.checked_mul(factor))
                    .ok_or(PreflightError::Limit {
                        resource: "content-model expansion",
                        maximum: limits.content_model_expansion,
                    })?;
                add_expansion(&mut content_model_expansion, expanded, limits)?;
                add_model_width(&mut model_widths, model, expanded, limits)?;
                expanded
            } else if is_xsd && element.name.local_name == "complexType" {
                child_model = model_widths.len();
                model_widths.push(0);
                1
            } else {
                parent_expansion
            };
            for child in element.children.iter().rev() {
                if let Node::Element(child) = child {
                    pending.push((child, expansion, child_model));
                }
            }
        }
    }

    let model_work = schema_nodes
        .checked_add(content_model_expansion.max(1))
        .ok_or(PreflightError::Limit {
            resource: "schema compilation work",
            maximum: limits.schema_compilation_work,
        })?;
    let schema_compilation_work =
        model_work
            .checked_mul(model_work)
            .ok_or(PreflightError::Limit {
                resource: "schema compilation work",
                maximum: limits.schema_compilation_work,
            })?;
    require_at_most(
        schema_compilation_work,
        limits.schema_compilation_work,
        "schema compilation work",
    )?;
    let (widest_content_model, validation_work) =
        instance_validation_work(document, &model_widths, limits)?;
    Ok(XsdWorkSummary {
        schema_nodes,
        content_model_expansion,
        widest_content_model,
        schema_compilation_work,
        validation_work,
    })
}

/// Linear instance-validation work: every document work node advances at
/// most the widest content model's positions, plus one unit per source byte.
/// Returns the widest model width and the total work.
fn instance_validation_work(
    document: &Document,
    model_widths: &[u64],
    limits: XsdWorkLimits,
) -> Result<(u64, u64), PreflightError> {
    let widest_content_model = model_widths.iter().copied().max().unwrap_or(0).max(1);
    let work_nodes = count_document_work_nodes(document);
    let source_bytes = u64::try_from(document.source.text().len()).unwrap_or(u64::MAX);
    let over = PreflightError::DocumentWork {
        work_nodes,
        source_bytes,
        widest_content_model,
        maximum: limits.validation_work,
    };
    let validation_work = work_nodes
        .checked_mul(widest_content_model)
        .and_then(|work| work.checked_add(source_bytes))
        .ok_or(over)?;
    if validation_work > limits.validation_work {
        return Err(over);
    }
    Ok((widest_content_model, validation_work))
}

/// Adds `amount` expanded positions to one content model's width. Every model
/// width is bounded by the total expansion, which is already budgeted.
fn add_model_width(
    widths: &mut [u64],
    model: usize,
    amount: u64,
    limits: XsdWorkLimits,
) -> Result<(), PreflightError> {
    let over = PreflightError::Limit {
        resource: "content-model expansion",
        maximum: limits.content_model_expansion,
    };
    let width = widths.get_mut(model).ok_or(over)?;
    *width = width.checked_add(amount).ok_or(over)?;
    require_at_most(
        *width,
        limits.content_model_expansion,
        "content-model expansion",
    )
}

/// Adds `amount` to the running content-model expansion within its budget.
fn add_expansion(
    total: &mut u64,
    amount: u64,
    limits: XsdWorkLimits,
) -> Result<(), PreflightError> {
    *total = total.checked_add(amount).ok_or(PreflightError::Limit {
        resource: "content-model expansion",
        maximum: limits.content_model_expansion,
    })?;
    require_at_most(
        *total,
        limits.content_model_expansion,
        "content-model expansion",
    )
}

/// Adds the schema-node work of every descendant of one named group body.
fn add_group_body_nodes(
    group: &Element,
    schema_nodes: &mut u64,
    maximum: u64,
) -> Result<(), PreflightError> {
    let element = group;
    let limits_schema_nodes = maximum;
    let mut body: Vec<&Element> = element
        .children
        .iter()
        .filter_map(|child| match child {
            Node::Element(child) => Some(child),
            Node::Text(_) => None,
        })
        .collect();
    while let Some(node) = body.pop() {
        add_schema_node_work(node, schema_nodes, limits_schema_nodes)?;
        body.extend(node.children.iter().filter_map(|child| match child {
            Node::Element(child) => Some(child),
            Node::Text(_) => None,
        }));
    }
    Ok(())
}

/// Removes backend source locations from a schema error message.
///
/// `xsd-schema` reports the two particles of some violations (for example
/// Unique Particle Attribution) in hash-iteration order, so the same schema
/// can yield `at A (see also B)` or `at B (see also A)`. Canonical reports
/// must be byte-deterministic, so both location forms are dropped; the
/// constraint name and the violation text are kept.
fn stable_backend_message(message: &str) -> String {
    let mut text = message.to_owned();
    while let Some(start) = text.find(" (see also ") {
        let Some(end) = text[start..].find(')') else {
            break;
        };
        text.replace_range(start..=start + end, "");
    }
    let mut result = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(index) = rest.find(" at ") {
        let after = &rest[index + 4..];
        let token_end = after.find(": ").unwrap_or(after.len());
        let token = &after[..token_end];
        let is_location = !token.contains(' ')
            && token
                .rsplit(':')
                .take(2)
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
        result.push_str(&rest[..index]);
        if is_location {
            rest = &after[token_end..];
        } else {
            result.push_str(" at ");
            rest = after;
        }
    }
    result.push_str(rest);
    result
}

/// Local part of a lexical `QName` reference.
fn local_part(qname: &str) -> &str {
    qname.rsplit_once(':').map_or(qname, |(_, local)| local)
}

/// Computes the content-model expansion of every top-level named model group.
///
/// A group's cost is the expansion of its body with parent expansion 1, where
/// each nested `<xs:group ref>` contributes its occurrence factor times the
/// referenced group's cost. Groups are keyed by local name; when several
/// namespaces define the same local name the maximum cost is used, which is a
/// conservative over-approximation. Circular definitions (illegal in XSD 1.0)
/// and references to undefined groups fail closed. Work is bounded by the
/// number of group definitions times their node counts.
fn group_expansion_costs(
    schemas: &[Document],
    limits: XsdWorkLimits,
) -> Result<BTreeMap<String, u64>, PreflightError> {
    let mut definitions: BTreeMap<String, Vec<&Element>> = BTreeMap::new();
    for schema in schemas {
        for child in &schema.root.children {
            if let Node::Element(group) = child
                && group.name.namespace_uri.as_deref() == Some(XML_SCHEMA_NAMESPACE)
                && group.name.local_name == "group"
                && let Some(name) = group.attribute("name")
            {
                definitions
                    .entry(name.value.clone())
                    .or_default()
                    .push(group);
            }
        }
    }
    let mut costs: BTreeMap<String, u64> = BTreeMap::new();
    for name in definitions.keys() {
        let mut visiting = BTreeSet::new();
        group_cost(name, &definitions, &mut costs, &mut visiting, limits)?;
    }
    Ok(costs)
}

fn group_cost(
    name: &str,
    definitions: &BTreeMap<String, Vec<&Element>>,
    costs: &mut BTreeMap<String, u64>,
    visiting: &mut BTreeSet<String>,
    limits: XsdWorkLimits,
) -> Result<u64, PreflightError> {
    if let Some(cost) = costs.get(name) {
        return Ok(*cost);
    }
    let bodies = definitions
        .get(name)
        .ok_or(PreflightError::UnresolvedGroupReference)?;
    if !visiting.insert(name.to_owned()) {
        return Err(PreflightError::CircularGroupReference);
    }
    // Recursion depth is bounded by the number of distinct group names
    // because `visiting` rejects any repeated name on the current path.
    let over = PreflightError::Limit {
        resource: "content-model expansion",
        maximum: limits.content_model_expansion,
    };
    let mut maximum = 0u64;
    for body in bodies {
        let mut total = 0u64;
        let mut pending: Vec<(&Element, u64)> = body
            .children
            .iter()
            .filter_map(|child| match child {
                Node::Element(child) => Some((child, 1u64)),
                Node::Text(_) => None,
            })
            .collect();
        while let Some((element, parent_expansion)) = pending.pop() {
            if element.name.namespace_uri.as_deref() != Some(XML_SCHEMA_NAMESPACE) {
                continue;
            }
            let local = element.name.local_name.as_str();
            let expanded = if local == "group"
                && let Some(reference) = element.attribute("ref")
            {
                let nested = group_cost(
                    local_part(&reference.value),
                    definitions,
                    costs,
                    visiting,
                    limits,
                )?;
                parent_expansion
                    .checked_mul(occurrence_factor(element)?)
                    .and_then(|value| value.checked_mul(nested.max(1)))
                    .ok_or(over)?
            } else if is_content_model_node(local) {
                occurrence_factor(element)?
                    .checked_mul(compositor_branching_factor(element)?)
                    .and_then(|factor| parent_expansion.checked_mul(factor))
                    .ok_or(over)?
            } else {
                continue;
            };
            total = total.checked_add(expanded).ok_or(over)?;
            require_at_most(
                total,
                limits.content_model_expansion,
                "content-model expansion",
            )?;
            if local != "group" {
                for child in &element.children {
                    if let Node::Element(child) = child {
                        pending.push((child, expanded));
                    }
                }
            }
        }
        maximum = maximum.max(total);
    }
    visiting.remove(name);
    costs.insert(name.to_owned(), maximum);
    Ok(maximum)
}

fn add_schema_node_work(
    element: &Element,
    total: &mut u64,
    maximum: u64,
) -> Result<(), PreflightError> {
    let attributes =
        u64::try_from(element.attributes.len()).map_err(|_| PreflightError::Limit {
            resource: "schema nodes",
            maximum,
        })?;
    let text_nodes = u64::try_from(
        element
            .children
            .iter()
            .filter(|node| matches!(node, Node::Text(_)))
            .count(),
    )
    .map_err(|_| PreflightError::Limit {
        resource: "schema nodes",
        maximum,
    })?;
    *total = total
        .checked_add(1)
        .and_then(|value| value.checked_add(attributes))
        .and_then(|value| value.checked_add(text_nodes))
        .ok_or(PreflightError::Limit {
            resource: "schema nodes",
            maximum,
        })?;
    require_at_most(*total, maximum, "schema nodes")
}

/// Counts elements, attributes, and text nodes. The parsed document is
/// already bounded by `XmlLimits`, so the count saturates instead of failing.
fn count_document_work_nodes(document: &Document) -> u64 {
    let mut total = 0u64;
    let mut pending = vec![&document.root];
    while let Some(element) = pending.pop() {
        let attributes = u64::try_from(element.attributes.len()).unwrap_or(u64::MAX);
        total = total.saturating_add(1).saturating_add(attributes);
        for child in &element.children {
            match child {
                Node::Element(child) => pending.push(child),
                Node::Text(_) => total = total.saturating_add(1),
            }
        }
    }
    total
}

fn is_content_model_node(local_name: &str) -> bool {
    matches!(
        local_name,
        "all" | "any" | "choice" | "element" | "group" | "sequence"
    )
}

fn occurrence_factor(element: &Element) -> Result<u64, PreflightError> {
    let minimum = element.attribute("minOccurs").map_or(Ok(1), |attribute| {
        attribute
            .value
            .parse::<u32>()
            .map_err(|_| PreflightError::InvalidOccurrence)
    })?;
    let Some(max_occurs) = element.attribute("maxOccurs") else {
        return Ok(u64::from(minimum.max(1)));
    };
    if max_occurs.value == "unbounded" {
        return Ok(u64::from(minimum.max(2)));
    }
    let maximum = max_occurs
        .value
        .parse::<u32>()
        .map_err(|_| PreflightError::InvalidOccurrence)?;
    if minimum > maximum {
        return Err(PreflightError::InvalidOccurrence);
    }
    Ok(u64::from(maximum.max(1)))
}

fn compositor_branching_factor(element: &Element) -> Result<u64, PreflightError> {
    if !matches!(element.name.local_name.as_str(), "all" | "choice") {
        return Ok(1);
    }
    let branches = element
        .children
        .iter()
        .filter(|node| {
            matches!(
                node,
                Node::Element(child)
                    if child.name.namespace_uri.as_deref() == Some(XML_SCHEMA_NAMESPACE)
                        && is_content_model_node(&child.name.local_name)
            )
        })
        .count();
    u64::try_from(branches.max(1)).map_err(|_| PreflightError::Limit {
        resource: "content-model expansion",
        maximum: MAX_CONTENT_MODEL_EXPANSION,
    })
}

fn require_at_most(
    observed: u64,
    maximum: u64,
    resource: &'static str,
) -> Result<(), PreflightError> {
    if observed > maximum {
        Err(PreflightError::Limit { resource, maximum })
    } else {
        Ok(())
    }
}

fn limit_error(resource: &str, limit: usize) -> SchemaBackendError {
    SchemaBackendError::new(
        "XSD006",
        format!("XSD {resource} limit exceeded (maximum {limit})"),
    )
}

#[cfg(test)]
mod tests {
    use scxml_syntax::telemetry::{Recorder, install_recorder};
    use scxml_syntax::{SourceId, SourceText, parse_xml};
    use scxml_validator_core::{Applicability, SchemaSource};

    use super::*;

    /// The source id of the documents these tests parse.
    const SOURCE: SourceId = SourceId::new(1);

    const XSD: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
        targetNamespace="urn:test" xmlns:t="urn:test" elementFormDefault="qualified">
      <xs:element name="root">
        <xs:complexType><xs:sequence><xs:element name="item" type="xs:int"/></xs:sequence></xs:complexType>
      </xs:element>
    </xs:schema>"#;

    fn document(xml: &str) -> Document {
        parse_xml(SourceText::new(SOURCE, "test.xml", xml))
            .expect("XSD test XML fixture must parse")
    }

    #[test]
    fn validates_supplied_xsd_without_external_resolution() {
        let schemas = [SchemaSource::new("memory:test.xsd", XSD.as_bytes())];
        let valid = XsdSchemaBackend
            .validate(
                &document(r#"<root xmlns="urn:test"><item>42</item></root>"#),
                &schemas,
            )
            .expect("valid XSD fixture must produce a validation report");
        assert!(!valid.has_errors());

        let invalid = XsdSchemaBackend
            .validate(
                &document(r#"<root xmlns="urn:test"><item>nope</item></root>"#),
                &schemas,
            )
            .expect("invalid XSD fixture must still produce a validation report");
        assert!(invalid.has_errors());
        assert!(
            invalid
                .diagnostics()
                .iter()
                .any(|item| item.code.starts_with("XSD-cvc-"))
        );
    }

    /// XML Schema 1.0 Part 1 §2.1: "The input to schema-validity assessment
    /// is an information set"; XML 1.0 §5.1 has the internal subset change
    /// it (included entities, supplied defaults, normalized values). The
    /// infoset is validated and every finding lands on its element.
    #[test]
    fn a_document_with_an_internal_subset_validates_as_its_infoset() {
        let schemas = [SchemaSource::new("memory:test.xsd", XSD.as_bytes())];
        let subset = "<!DOCTYPE root [<!ENTITY item \"<item>&n;</item>\"><!ENTITY n \"42\"><!ATTLIST root xmlns CDATA #FIXED 'urn:test'>]>";
        let valid = XsdSchemaBackend
            .validate(&document(&format!("{subset}<root>&item;</root>")), &schemas)
            .expect("the infoset validates");
        assert!(!valid.has_errors(), "{:?}", valid.diagnostics());

        let invalid_xml = format!("{subset}<root>&item;<item>nope</item></root>");
        let invalid = XsdSchemaBackend
            .validate(&document(&invalid_xml), &schemas)
            .expect("the infoset validates");
        let finding = invalid
            .diagnostics()
            .iter()
            .find(|item| item.code.starts_with("XSD-cvc-"))
            .expect("the second item is not allowed");
        let start = invalid_xml.rfind("<item>").expect("the source item");
        assert!(
            finding.primary.span.start >= start,
            "the finding lands on the source element, not the included one: {finding:?}"
        );

        // Only a document with an internal subset is validated as its
        // serialized infoset; any other as its source bytes.
        assert!(!has_internal_subset(&document("<root xmlns='urn:test'/>")));
        assert!(has_internal_subset(&document(&format!("{subset}<root/>"))));
    }

    /// A pinned repository namespace (`@<tag>`) is in the tagless namespace
    /// once parsed, which its package's schema targets; findings keep the
    /// source spans of what the author wrote.
    #[test]
    fn a_pinned_namespace_validates_against_its_tagless_schema() {
        let schema = XSD.replace("urn:test", "https://github.com/o/r/namespaces/t");
        let schemas = [SchemaSource::new("memory:t.xsd", schema.into_bytes())];
        let pinned =
            r#"<root xmlns="https://github.com/o/r/namespaces/t@v1.0.0"><item>42</item></root>"#;
        let valid = XsdSchemaBackend
            .validate(&document(pinned), &schemas)
            .expect("the pinned document validates");
        assert!(!valid.has_errors(), "{:?}", valid.diagnostics());
        let tagless = XsdSchemaBackend
            .validate(
                &document(
                    r#"<root xmlns="https://github.com/o/r/namespaces/t"><item>42</item></root>"#,
                ),
                &schemas,
            )
            .expect("the tagless document validates");
        assert!(!tagless.has_errors(), "{:?}", tagless.diagnostics());

        let invalid_xml =
            r#"<root xmlns="https://github.com/o/r/namespaces/t@v1.0.0"><item>nope</item></root>"#;
        let invalid = XsdSchemaBackend
            .validate(&document(invalid_xml), &schemas)
            .expect("the pinned document validates");
        let finding = invalid
            .diagnostics()
            .iter()
            .find(|item| item.code.starts_with("XSD-cvc-"))
            .expect("the item is not an int");
        let start = invalid_xml.find("<item>").expect("the source item");
        assert!(
            finding.primary.span.start >= start,
            "the finding lands on the source element: {finding:?}"
        );
        assert!(has_tag(&document(pinned)));
        assert!(!has_tag(&document("<root xmlns='urn:test'/>")));
    }

    #[test]
    fn the_infoset_serialization_reads_back_as_the_same_values() {
        let parsed = document(
            "<!DOCTYPE r [<!ATTLIST r a CDATA 'x&#9;y&#10;&#13;&quot;&lt;&amp;'>]><r xmlns:p='urn:p'><p:c>a&amp;b&lt;&#13;]]&gt;</p:c></r>",
        );
        let text = infoset_text(&parsed);
        let reparsed = document(&text);
        assert_eq!(reparsed.root.attributes, {
            let mut attributes = parsed.root.attributes.clone();
            for attribute in &mut attributes {
                attribute.span = reparsed.root.attributes[0].span;
                attribute.value_span = reparsed.root.attributes[0].value_span;
            }
            attributes
        });
        let (Node::Element(original), Node::Element(copy)) =
            (&parsed.root.children[0], &reparsed.root.children[0])
        else {
            panic!("expected the child element");
        };
        assert_eq!(original.name, copy.name);
        let (Node::Text(original), Node::Text(copy)) = (&original.children[0], &copy.children[0])
        else {
            panic!("expected text");
        };
        assert_eq!(original.value, copy.value);
    }

    #[test]
    fn validates_normal_core_and_custom_schema_fixtures() {
        let schemas = [
            SchemaSource::new(
                "memory:scxml-subset.xsd",
                include_bytes!("../../../tests/fixtures/scxml-subset.xsd"),
            ),
            SchemaSource::new(
                "memory:extension-example.xsd",
                include_bytes!("../../../tests/fixtures/extension-example.xsd"),
            ),
        ];
        let report = XsdSchemaBackend
            .validate(
                &document(
                    r#"<scxml xmlns="http://www.w3.org/2005/07/scxml"
                         xmlns:c="urn:example:custom" initial="ready">
                      <state id="ready"><c:mark value="42"/></state>
                    </scxml>"#,
                ),
                &schemas,
            )
            .expect("normal core and custom schemas must stay within work budgets");
        assert!(!report.has_errors());
    }

    /// Natively the schema count and size are unbounded: one schema past
    /// the wasm count bound (64) validates.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_validation_has_no_schema_count_budget() {
        let document = document(r#"<root xmlns="urn:test"><item>42</item></root>"#);
        let schemas = (0..=64)
            .map(|index| {
                let schema = if index == 0 {
                    XSD.to_owned()
                } else {
                    XSD.replace("urn:test", &format!("urn:test:{index}"))
                };
                SchemaSource::new(format!("memory:{index}.xsd"), schema.into_bytes())
            })
            .collect::<Vec<_>>();
        let report = XsdSchemaBackend
            .validate(&document, &schemas)
            .expect("native schema count is unbounded");
        assert!(!report.has_errors(), "{report:?}");
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn wasm_only_rejects_schema_count_and_byte_budgets_before_backend_parsing() {
        let document = document(r#"<root xmlns="urn:test"><item>42</item></root>"#);
        let schemas = (0..=MAX_SCHEMA_COUNT)
            .map(|index| SchemaSource::new(format!("memory:{index}.xsd"), XSD.as_bytes()))
            .collect::<Vec<_>>();
        let count_error = XsdSchemaBackend
            .validate(&document, &schemas)
            .expect_err("schema count must be bounded");
        assert_eq!(count_error.code, "XSD006");
        assert!(count_error.message.contains("schema count limit exceeded"));

        let byte_error = XsdSchemaBackend
            .validate(
                &document,
                &[SchemaSource::new(
                    "memory:large.xsd",
                    vec![b' '; MAX_SCHEMA_BYTES + 1],
                )],
            )
            .expect_err("schema bytes must be bounded");
        assert_eq!(byte_error.code, "XSD006");
        assert!(byte_error.message.contains("schema bytes limit exceeded"));
    }

    /// A budget rejection records one bounded `command-budget` failure. The
    /// content-model expansion budget applies to every build, so the
    /// recorder (a native facility) observes it here.
    #[test]
    fn records_xsd_budget_rejection_with_bounded_telemetry() {
        let recorder = Recorder::default();
        let _guard = install_recorder(recorder.clone());
        let expanding = XSD.replace(
            r#"<xs:element name="item" type="xs:int"/>"#,
            r#"<xs:element name="item" type="xs:int" maxOccurs="2000000"/>"#,
        );
        let error = XsdSchemaBackend
            .validate(
                &document(r#"<root xmlns="urn:test"><item>42</item></root>"#),
                &[SchemaSource::new(
                    "memory:expanding.xsd",
                    expanding.as_bytes(),
                )],
            )
            .expect_err("content-model expansion budget must reject input");
        assert_eq!(error.code, "XSD006");
        // Only the fixture parse, the schema parse, and the XSD boundary
        // record spans; preflight, budget errors, and projection run inside
        // the latter.
        assert_eq!(
            recorder
                .observations()
                .iter()
                .map(|item| item.operation)
                .collect::<Vec<_>>(),
            [
                Operation::SyntaxParseXml,
                Operation::SyntaxParseXml,
                Operation::ValidatorXsdValidate
            ]
        );
        assert!(recorder.observations().iter().any(|item| {
            item.module == Module::Validator
                && item.operation == Operation::ValidatorXsdValidate
                && item.error == Some(ErrorCategory::Budget)
                && item.events.contains(&SignalEvent::InputRejected)
        }));
    }

    #[test]
    fn preflight_accepts_exact_work_budget_and_rejects_one_less() {
        let schema = document(XSD);
        let instance = document(r#"<root xmlns="urn:test"><item>42</item></root>"#);
        let measured = preflight_xsd_work(
            &instance,
            std::slice::from_ref(&schema),
            XsdWorkLimits::UNBOUNDED_FOR_MEASUREMENT,
        )
        .expect("small schema work must be measurable");

        let exact = XsdWorkLimits::from_summary(measured);
        assert_eq!(
            preflight_xsd_work(&instance, std::slice::from_ref(&schema), exact),
            Ok(measured)
        );

        for (limits, resource, maximum) in [
            (
                XsdWorkLimits {
                    schema_nodes: measured.schema_nodes - 1,
                    ..exact
                },
                "schema nodes",
                measured.schema_nodes - 1,
            ),
            (
                XsdWorkLimits {
                    content_model_expansion: measured.content_model_expansion - 1,
                    ..exact
                },
                "content-model expansion",
                measured.content_model_expansion - 1,
            ),
            (
                XsdWorkLimits {
                    schema_compilation_work: measured.schema_compilation_work - 1,
                    ..exact
                },
                "schema compilation work",
                measured.schema_compilation_work - 1,
            ),
        ] {
            let error = preflight_xsd_work(&instance, std::slice::from_ref(&schema), limits)
                .expect_err("one work unit over budget must be rejected")
                .into_backend_error();
            assert_eq!(error.code, "XSD006");
            assert_eq!(
                error.message,
                format!("XSD {resource} limit exceeded (maximum {maximum})")
            );
        }
    }

    #[test]
    fn rejects_nested_choice_occurrence_amplification_before_backend() {
        let recorder = Recorder::default();
        let _guard = install_recorder(recorder.clone());
        let schema = format!(
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
              <xs:element name="root"><xs:complexType><xs:sequence maxOccurs="{}">
                <xs:choice maxOccurs="{}">
                  <xs:element name="a"/><xs:element name="b"/>
                </xs:choice>
              </xs:sequence></xs:complexType></xs:element>
            </xs:schema>"#,
            u32::MAX,
            u32::MAX
        );
        let error = XsdSchemaBackend
            .validate(
                &document("<root/>"),
                &[SchemaSource::new("memory:amplified.xsd", schema)],
            )
            .expect_err("nested counted occurrences must fail the preflight budget");
        assert_eq!(error.code, "XSD006");
        assert!(error.message.contains("content-model expansion"));
        assert!(recorder.observations().iter().any(|item| {
            item.module == Module::Validator
                && item.operation == Operation::ValidatorXsdValidate
                && item.error == Some(ErrorCategory::Budget)
                && item.events.contains(&SignalEvent::InputRejected)
        }));
    }

    #[test]
    fn backend_schema_error_locations_are_removed_deterministically() {
        assert_eq!(
            stable_backend_message(
                "Schema structural error at a.xsd:7:22: UPA violation: elements 'a' and 'a' conflict at state 6 (see also a.xsd:6:9) (constraint: cos-nonambig)"
            ),
            "Schema structural error: UPA violation: elements 'a' and 'a' conflict at state 6 (constraint: cos-nonambig)"
        );
        assert_eq!(
            stable_backend_message(
                "Schema structural error at a.xsd:6:9: UPA violation: elements 'a' and 'a' conflict at state 6 (see also a.xsd:7:22) (constraint: cos-nonambig)"
            ),
            stable_backend_message(
                "Schema structural error at a.xsd:7:22: UPA violation: elements 'a' and 'a' conflict at state 6 (see also a.xsd:6:9) (constraint: cos-nonambig)"
            )
        );
    }

    #[test]
    fn budgets_defined_group_references_and_fails_closed_on_bad_ones() {
        let defined = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
          <xs:group name="items"><xs:choice><xs:element name="a"/></xs:choice></xs:group>
          <xs:element name="root"><xs:complexType><xs:group ref="items"/></xs:complexType></xs:element>
        </xs:schema>"#;
        let report = XsdSchemaBackend
            .validate(
                &document("<root><a/></root>"),
                &[SchemaSource::new("memory:group.xsd", defined)],
            )
            .expect("a defined model-group reference must be budgeted and validated");
        assert!(!report.has_errors(), "{report:?}");

        for (schema, message) in [
            (
                r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
                  <xs:element name="root"><xs:complexType><xs:group ref="missing"/></xs:complexType></xs:element>
                </xs:schema>"#,
                "XSD model-group reference does not name a group defined in the supplied schema set",
            ),
            (
                r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
                  <xs:group name="a"><xs:sequence><xs:group ref="b"/></xs:sequence></xs:group>
                  <xs:group name="b"><xs:sequence><xs:group ref="a"/></xs:sequence></xs:group>
                  <xs:element name="root"><xs:complexType><xs:group ref="a"/></xs:complexType></xs:element>
                </xs:schema>"#,
                "XSD model-group definitions reference each other circularly",
            ),
        ] {
            let error = XsdSchemaBackend
                .validate(
                    &document("<root/>"),
                    &[SchemaSource::new("memory:group.xsd", schema)],
                )
                .expect_err("unbudgetable group references must fail closed before the backend");
            assert_eq!(error.code, "XSD007");
            assert_eq!(error.message, message);
        }
    }

    #[test]
    fn group_reference_amplification_is_rejected_by_the_expansion_budget() {
        let schema = format!(
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
              <xs:group name="leaf"><xs:choice maxOccurs="{max}"><xs:element name="a"/><xs:element name="b"/></xs:choice></xs:group>
              <xs:group name="mid"><xs:sequence maxOccurs="{max}"><xs:group ref="leaf" maxOccurs="{max}"/></xs:sequence></xs:group>
              <xs:element name="root"><xs:complexType><xs:group ref="mid" maxOccurs="{max}"/></xs:complexType></xs:element>
            </xs:schema>"#,
            max = 1000
        );
        let error = XsdSchemaBackend
            .validate(
                &document("<root/>"),
                &[SchemaSource::new("memory:amplified-group.xsd", schema)],
            )
            .expect_err("group reference amplification must fail the preflight budget");
        assert_eq!(error.code, "XSD006");
        assert!(error.message.contains("content-model expansion"));
    }

    fn official_schema_documents() -> Vec<Document> {
        w3c_scxml_schema_set()
            .iter()
            .enumerate()
            .map(|(index, schema)| {
                parse_xml_with_limits(
                    SourceText::new(
                        SourceId::new(u32::try_from(index).expect("schema index fits u32")),
                        schema.system_id.clone(),
                        String::from_utf8(schema.bytes.clone()).expect("official schema is UTF-8"),
                    ),
                    XSD_XML_LIMITS,
                )
                .expect("official schema parses under schema XML limits")
            })
            .collect()
    }

    #[test]
    fn official_w3c_schema_set_work_is_pinned_below_the_budget() {
        let documents = official_schema_documents();
        let instance =
            document(r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"/>"#);
        let summary = preflight_xsd_work(
            &instance,
            &documents,
            XsdWorkLimits::UNBOUNDED_FOR_MEASUREMENT,
        )
        .expect("official schema work must be measurable");
        assert_eq!(summary.schema_nodes, 2114);
        assert_eq!(summary.content_model_expansion, 8729);
        assert_eq!(summary.schema_compilation_work, 117_570_649);
        assert_eq!(summary.widest_content_model, 815);
        assert!(summary.schema_compilation_work * 2 < MAX_SCHEMA_COMPILATION_WORK);
    }

    fn flat_machine(states: usize, logs: usize) -> String {
        use std::fmt::Write as _;

        let mut xml = String::from(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="s0">"#,
        );
        for index in 0..states {
            let next = (index + 1) % states;
            write!(xml, r#"<state id="s{index}"><onentry>"#)
                .expect("writing to a String cannot fail");
            for _ in 0..logs {
                xml.push_str(r#"<log label="l" expr="1"/>"#);
            }
            write!(
                xml,
                r#"</onentry><transition event="go" target="s{next}"/></state>"#
            )
            .expect("writing to a String cannot fail");
        }
        xml.push_str("</scxml>");
        xml
    }

    #[test]
    fn large_valid_documents_validate_within_the_linear_instance_budget() {
        let schemas = w3c_scxml_schema_set();
        for (states, logs) in [(150, 0), (1, 400), (10_000, 1), (1, 20_000)] {
            let report = XsdSchemaBackend
                .validate(&document(&flat_machine(states, logs)), &schemas)
                .unwrap_or_else(|error| {
                    panic!("valid {states}-state/{logs}-log document must fit the budget: {error}")
                });
            assert!(!report.has_errors(), "{states}/{logs}: {report:?}");
        }
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn wasm_only_document_validation_budget_admits_every_document_within_xml_limits() {
        let documents = official_schema_documents();
        let instance =
            document(r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"/>"#);
        let summary = preflight_xsd_work(
            &instance,
            &documents,
            XsdWorkLimits::UNBOUNDED_FOR_MEASUREMENT,
        )
        .expect("official schema work must be measurable");
        let limits = XmlLimits::DEFAULT;
        let nodes = u64::try_from(limits.max_nodes).expect("node limit fits u64")
            + u64::try_from(limits.max_attributes).expect("attribute limit fits u64");
        let bytes = u64::try_from(limits.max_source_bytes).expect("byte limit fits u64");
        let worst_case = nodes * summary.widest_content_model + bytes;
        assert!(
            worst_case <= MAX_DOCUMENT_VALIDATION_WORK,
            "a document at the XML limits ({worst_case} work units) must fit the bundled-schema budget"
        );
    }

    #[test]
    fn document_validation_limit_names_the_instance_size_and_the_widest_model() {
        let schema = document(XSD);
        let instance = document(r#"<root xmlns="urn:test"><item>42</item></root>"#);
        let measured = preflight_xsd_work(
            &instance,
            std::slice::from_ref(&schema),
            XsdWorkLimits::UNBOUNDED_FOR_MEASUREMENT,
        )
        .expect("small schema work must be measurable");
        let limits = XsdWorkLimits {
            validation_work: measured.validation_work - 1,
            ..XsdWorkLimits::from_summary(measured)
        };
        let error = preflight_xsd_work(&instance, std::slice::from_ref(&schema), limits)
            .expect_err("one work unit over budget must be rejected")
            .into_backend_error();
        assert_eq!(error.code, "XSD006");
        assert!(
            error
                .message
                .starts_with("document is too large for XSD validation"),
            "{}",
            error.message
        );
        let help = error
            .help
            .expect("instance-size limit must carry specific help");
        assert!(help.contains("split"), "{help}");
    }

    #[test]
    fn official_w3c_schema_set_compiles_within_budgets_and_validates() {
        let schemas = w3c_scxml_schema_set();
        let valid = XsdSchemaBackend
            .validate(
                &document(
                    r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a">
                      <state id="a"><onentry><log expr="1"/></onentry><transition event="go" target="b"/></state>
                      <final id="b"/>
                    </scxml>"#,
                ),
                &schemas,
            )
            .expect("the bundled official schema set must compile within budgets");
        assert!(!valid.has_errors(), "{valid:?}");
    }

    fn apply(xml: &str, fix: &scxml_validator_core::Fix) -> String {
        let mut edits: Vec<_> = fix.edits.iter().collect();
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.span.start));
        let mut output = xml.to_owned();
        for edit in edits {
            output.replace_range(edit.span.start..edit.span.end, &edit.replacement);
        }
        output
    }

    /// `<state>` of `count` `<onentry>` elements, each holding U+1680,
    /// which the schema engine skips as white space although XML does not.
    fn ogham_onentries(count: usize) -> String {
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="s"><state id="s">{}</state></scxml>"#,
            "<onentry>&#x1680;</onentry>".repeat(count)
        )
    }

    /// An inline child machine is SCXML structure as its root is (hosts
    /// compile it when it is invoked), so its element-only content is
    /// checked although its `<content>` parent is mixed.
    #[test]
    fn element_only_text_is_checked_in_an_inline_child_machine() {
        let xml = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="s"><state id="s"><invoke type="scxml"><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="c"><state id="c">&#x1680;</state></scxml></content></invoke></state></scxml>"#;
        let report = XsdSchemaBackend
            .validate(&document(xml), &w3c_scxml_schema_set())
            .expect("schema validation runs");
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == "XSD-cvc-complex-type.2.3"),
            "{report:?}"
        );
    }

    /// Lax wildcards (`<data>`, `<assign>`, `<content>`, and `##other`
    /// extension elements) assess every SCXML element under them against
    /// its global declaration, however deep: text of U+1680 in each is the
    /// finding the schema engine gives text of `x` there.
    #[test]
    fn element_only_text_is_checked_in_every_scxml_element_under_lax_content() {
        const SCXML: &str = "http://www.w3.org/2005/07/scxml";
        const UNEXPECTED_TEXT: &str = "XSD-cvc-complex-type.2.3";
        let placements = [
            format!(
                r#"<datamodel><data id="d"><state xmlns="{SCXML}" id="q">TEXT</state></data></datamodel>"#
            ),
            format!(
                r#"<datamodel><data id="d"><foo xmlns=""><state xmlns="{SCXML}" id="q">TEXT</state></foo></data></datamodel>"#
            ),
            format!(
                r#"<datamodel><data id="d"><state xmlns="{SCXML}" id="q"><onentry>TEXT</onentry></state></data></datamodel>"#
            ),
            format!(
                r#"<datamodel><data id="d"><log xmlns="{SCXML}">TEXT</log></data></datamodel>"#
            ),
            format!(
                r#"<e:foo xmlns:e="urn:e"><e:bar><onentry xmlns="{SCXML}">TEXT</onentry></e:bar></e:foo>"#
            ),
            format!(
                r#"<onentry><assign location="a"><e:foo xmlns:e="urn:e"><state xmlns="{SCXML}" id="q">TEXT</state></e:foo></assign></onentry>"#
            ),
        ];
        let unexpected_text = |body: &str| {
            let xml = format!(
                r#"<scxml xmlns="{SCXML}" version="1.0" initial="s"><state id="s">{body}</state></scxml>"#
            );
            XsdSchemaBackend
                .validate(&document(&xml), &w3c_scxml_schema_set())
                .expect("schema validation runs")
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.code == UNEXPECTED_TEXT)
                .count()
        };
        for placement in placements {
            let text = unexpected_text(&placement.replace("TEXT", "x"));
            assert_eq!(text, 1, "{placement}");
            assert_eq!(
                unexpected_text(&placement.replace("TEXT", "&#x1680;")),
                text,
                "{placement}"
            );
        }
    }

    /// Neither an SCXML-namespace name the schema does not declare nor a
    /// declared element with mixed content has element-only content to
    /// assess, so text of other Unicode white space in it draws what text of
    /// `x` draws, under lax content and in a structural position alike.
    #[test]
    fn element_only_text_is_not_checked_without_element_only_content() {
        const SCXML: &str = "http://www.w3.org/2005/07/scxml";
        const UNEXPECTED_TEXT: &str = "XSD-cvc-complex-type.2.3";
        let placements = [
            format!(
                r#"<datamodel><data id="d"><foo xmlns="{SCXML}">TEXT</foo></data></datamodel>"#
            ),
            format!(
                r#"<onentry><assign location="a"><foo xmlns="{SCXML}">TEXT</foo></assign></onentry>"#
            ),
            "<foo>TEXT</foo>".to_owned(),
            "<datamodel><data id=\"d\">TEXT</data></datamodel>".to_owned(),
            "<onentry><script>TEXT</script></onentry>".to_owned(),
        ];
        let unexpected_text = |body: &str| {
            let xml = format!(
                r#"<scxml xmlns="{SCXML}" version="1.0" initial="s"><state id="s">{body}</state></scxml>"#
            );
            XsdSchemaBackend
                .validate(&document(&xml), &w3c_scxml_schema_set())
                .expect("schema validation runs")
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.code == UNEXPECTED_TEXT)
                .count()
        };
        for placement in placements {
            assert_eq!(
                unexpected_text(&placement.replace("TEXT", "&#x1680;")),
                unexpected_text(&placement.replace("TEXT", "x")),
                "{placement}"
            );
        }
    }

    /// The element-only text findings count against the message bound with
    /// the schema engine's own findings.
    #[test]
    fn element_only_text_findings_stop_at_the_message_bound() {
        const BOUND: usize = 2;
        let within = document(&ogham_onentries(BOUND));
        assert_eq!(
            project::non_xml_space_text(&within, BOUND).map(|found| found.len()),
            Some(BOUND)
        );
        let past = document(&ogham_onentries(BOUND + 1));
        assert!(project::non_xml_space_text(&past, BOUND).is_none());
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn wasm_only_element_only_text_findings_count_against_the_message_bound() {
        let error = XsdSchemaBackend
            .validate(
                &document(&ogham_onentries(MAX_VALIDATION_MESSAGES + 1)),
                &w3c_scxml_schema_set(),
            )
            .expect_err("one finding past the bound is the limit error");
        assert_eq!(
            error,
            limit_error("validation messages", MAX_VALIDATION_MESSAGES)
        );
    }

    #[test]
    fn misordered_if_branches_name_the_order_and_move_the_branch() {
        let xml = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a"><state id="a"><onentry><if cond="true"><log expr="1"/><else/><log expr="2"/><elseif cond="false"/><log expr="3"/></if></onentry></state></scxml>"#;
        let report = XsdSchemaBackend
            .validate(&document(xml), &w3c_scxml_schema_set())
            .expect("schema validation runs");
        let diagnostic = report
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.code == "XSD-cvc-complex-type.2.4")
            .expect("ordering finding");
        let help = diagnostic.help.as_deref().unwrap_or_default();
        assert!(
            help.contains("before `<else>`"),
            "help must name the ordering constraint: {help}"
        );
        assert!(
            diagnostic
                .secondary
                .iter()
                .any(|label| &xml[label.span.start..label.span.end] == "else"),
            "the `<else>` it must precede is labeled: {diagnostic:?}"
        );
        let fixed = apply(xml, &diagnostic.fixes[0]);
        assert!(
            fixed.contains(
                r#"<log expr="1"/><elseif cond="false"/><log expr="3"/><else/><log expr="2"/></if>"#
            ),
            "{fixed}"
        );
        let revalidated = XsdSchemaBackend
            .validate(&document(&fixed), &w3c_scxml_schema_set())
            .expect("schema validation runs");
        assert!(!revalidated.has_errors(), "{revalidated:?}");
    }

    #[test]
    fn extra_pseudo_state_transitions_are_left_to_the_semantic_rules() {
        for xml in [
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="p"><state id="p"><history id="h"><transition target="a"/><transition target="b"/></history><state id="a"/><state id="b"/></state></scxml>"#,
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="p"><state id="p"><initial><transition target="a"/><transition target="b"/></initial><state id="a"/><state id="b"/></state></scxml>"#,
        ] {
            let report = XsdSchemaBackend
                .validate(&document(xml), &w3c_scxml_schema_set())
                .expect("schema validation runs");
            assert!(
                !report.has_errors(),
                "SCXML009/SCXML019 own the single-transition rule: {report:?}"
            );
        }
    }

    #[test]
    fn state_type_final_offers_the_element_rename() {
        let xml = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a"><state id="a"><transition event="go" target="f"/></state><state id="f" type="final"></state></scxml>"#;
        let report = XsdSchemaBackend
            .validate(&document(xml), &w3c_scxml_schema_set())
            .expect("schema validation runs");
        let diagnostic = report
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.code.starts_with("XSD-cvc-complex-type.3.2"))
            .expect("unknown attribute finding");
        assert!(
            diagnostic
                .help
                .as_deref()
                .unwrap_or_default()
                .contains("<final>"),
            "{diagnostic:?}"
        );
        let fixed = apply(xml, &diagnostic.fixes[0]);
        assert!(fixed.contains(r#"<final id="f"></final>"#), "{fixed}");
        let revalidated = XsdSchemaBackend
            .validate(&document(&fixed), &w3c_scxml_schema_set())
            .expect("schema validation runs");
        assert!(!revalidated.has_errors(), "{revalidated:?}");
    }

    /// The one fix of the first finding with `code`, applied, revalidated.
    fn only_fix(xml: &str, schemas: &[SchemaSource], code: &str) -> scxml_validator_core::Fix {
        let report = XsdSchemaBackend
            .validate(&document(xml), schemas)
            .expect("schema validation runs");
        let diagnostic = report
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.code == code)
            .unwrap_or_else(|| panic!("{code} in {report:?}"));
        assert_eq!(diagnostic.fixes.len(), 1, "{diagnostic:?}");
        diagnostic.fixes[0].clone()
    }

    /// Each counterpart is one rename that keeps the value; the renamed
    /// document is schema-valid. Applicability follows whether the value
    /// keeps its meaning.
    #[test]
    fn cross_element_attributes_are_renamed_to_the_counterpart() {
        let wrap = |body: &str| {
            format!(
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a"><state id="a"><onentry>{body}</onentry><transition event="go" target="b"/></state><final id="b"/></scxml>"#
            )
        };
        let cases = [
            (
                wrap(r#"<send event="t" delay="3s" sendid="s1"/>"#),
                "XSD-cvc-complex-type.3.2.2",
                r#"<send event="t" delay="3s" id="s1"/>"#,
                Applicability::MachineApplicable,
            ),
            (
                wrap(r#"<cancel id="s1"/>"#),
                "XSD-cvc-complex-type.3.2.2",
                r#"<cancel sendid="s1"/>"#,
                Applicability::MachineApplicable,
            ),
            (
                wrap(r#"<if expr="true"><log expr="1"/></if>"#),
                "XSD-cvc-complex-type.3.2.2",
                r#"<if cond="true">"#,
                Applicability::MachineApplicable,
            ),
            (
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a"><state id="a"><transition event="go" expr="true" target="b"/></state><final id="b"/></scxml>"#.to_owned(),
                "XSD-cvc-complex-type.3.2.2",
                r#"<transition event="go" cond="true" target="b"/>"#,
                Applicability::MachineApplicable,
            ),
            (
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a"><state id="a" target="b"><state id="b"/></state></scxml>"#.to_owned(),
                "XSD-cvc-complex-type.3.2.2",
                r#"<state id="a" initial="b">"#,
                Applicability::MaybeIncorrect,
            ),
        ];
        let schemas = w3c_scxml_schema_set();
        for (xml, code, expected, applicability) in cases {
            let fix = only_fix(&xml, &schemas, code);
            assert_eq!(fix.applicability, applicability, "{xml}: {fix:?}");
            let fixed = apply(&xml, &fix);
            assert!(fixed.contains(expected), "{fixed}");
            let revalidated = XsdSchemaBackend
                .validate(&document(&fixed), &schemas)
                .expect("schema validation runs");
            assert!(!revalidated.has_errors(), "{revalidated:?}");
        }
    }

    /// A rename onto an attribute the element already carries would
    /// duplicate it, so it is never offered.
    #[test]
    fn an_occupied_counterpart_is_not_a_rename() {
        let xml = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a"><state id="a"><onentry><send event="t" id="s0" sendid="s1"/></onentry></state></scxml>"#;
        let fix = only_fix(xml, &w3c_scxml_schema_set(), "XSD-cvc-complex-type.3.2.2");
        assert_eq!(fix.applicability, Applicability::MaybeIncorrect);
        assert!(fix.message.starts_with("remove"), "{fix:?}");
    }

    /// One closest name is machine-applicable; a tie offers every closest
    /// name as maybe-incorrect.
    #[test]
    fn typo_applicability_follows_ambiguity() {
        let schemas = w3c_scxml_schema_set();
        let unique = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a"><state id="a"><transition evnt="go" target="b"/></state><final id="b"/></scxml>"#;
        let fix = only_fix(unique, &schemas, "XSD-cvc-complex-type.3.2.2");
        assert_eq!(fix.applicability, Applicability::MachineApplicable);

        let enumeration = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a"><state id="a"><transition event="go" tpye="inner" target="b"/></state><final id="b"/></scxml>"#;
        let fix = only_fix(enumeration, &schemas, "XSD-cvc-complex-type.3.2.2");
        assert_eq!(
            fix.applicability,
            Applicability::MaybeIncorrect,
            "`inner` is not a `type` value"
        );

        let ambiguous = r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a"><state id="a"><onentry><send event="go" tage="#_internal"/></onentry></state></scxml>"##;
        let report = XsdSchemaBackend
            .validate(&document(ambiguous), &schemas)
            .expect("schema validation runs");
        let diagnostic = report
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.code == "XSD-cvc-complex-type.3.2.2")
            .expect("unknown attribute");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("did you mean `target` or `type`?")
        );
        let renames: Vec<_> = diagnostic
            .fixes
            .iter()
            .map(|fix| (fix.applicability, fix.edits[0].replacement.as_str()))
            .collect();
        assert_eq!(
            renames,
            [
                (Applicability::MaybeIncorrect, "target"),
                (Applicability::MaybeIncorrect, "type")
            ]
        );
    }

    /// An extension element's attributes come from its package schema,
    /// including those of attribute groups and base types.
    #[test]
    fn extension_attribute_typos_are_suggested_from_the_package_schema() {
        let package = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:a="urn:action"
            targetNamespace="urn:action" elementFormDefault="qualified">
          <xs:attributeGroup name="addressed"><xs:attribute name="channel" type="xs:string"/></xs:attributeGroup>
          <xs:complexType name="base"><xs:attribute name="priority" type="xs:int"/></xs:complexType>
          <xs:element name="notify">
            <xs:complexType><xs:complexContent><xs:extension base="a:base">
              <xs:attribute name="message" type="xs:string"/>
              <xs:attributeGroup ref="a:addressed"/>
            </xs:extension></xs:complexContent></xs:complexType>
          </xs:element>
        </xs:schema>"#;
        let mut schemas = w3c_scxml_schema_set();
        schemas.push(SchemaSource::new("memory:action.xsd", package.as_bytes()));
        let wrap = |attributes: &str| {
            format!(
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:x="urn:action" version="1.0" initial="a"><state id="a"><onentry><x:notify {attributes}/></onentry></state></scxml>"#
            )
        };
        for (typo, name) in [
            ("chanel", "channel"),
            ("mesage", "message"),
            ("priorty", "priority"),
        ] {
            let xml = wrap(&format!(r#"{typo}="1""#));
            let fix = only_fix(&xml, &schemas, "XSD-cvc-complex-type.3.2.2");
            assert_eq!(fix.applicability, Applicability::MaybeIncorrect, "{fix:?}");
            let fixed = apply(&xml, &fix);
            assert!(fixed.contains(&format!(r#"{name}="1""#)), "{fixed}");
            let revalidated = XsdSchemaBackend
                .validate(&document(&fixed), &schemas)
                .expect("schema validation runs");
            assert!(!revalidated.has_errors(), "{revalidated:?}");
        }
        let report = XsdSchemaBackend
            .validate(&document(&wrap(r#"zzz="1""#)), &schemas)
            .expect("schema validation runs");
        let help = report
            .diagnostics()
            .iter()
            .find_map(|diagnostic| diagnostic.help.clone())
            .unwrap_or_default();
        assert!(
            help.starts_with("`<x:notify>` accepts `channel`, `message`, `priority`"),
            "{help}"
        );
    }
}
