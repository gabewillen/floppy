//! Span-exact projection of `xsd-schema` findings into validator diagnostics.
//!
//! The backend reports instance errors without byte locations, so the adapter
//! drives validation through `drive_quick_xml_with` and tags every finding
//! with the validator event that produced it: element start, one attribute,
//! end of attributes, a text run, element end, or end of document. Each
//! handler hook runs immediately after the corresponding runtime call, so the
//! findings that arrived since the previous hook belong to that call. An
//! element is identified by its position in document order, which the
//! validated XML (the source, or the serialized infoset of a document whose
//! internal DTD subset changes it) shares with the parsed [`Document`], and
//! the tag is resolved against that element to the most specific
//! responsible bytes: an element name, an attribute name or value, a text
//! run, or an end tag.
//!
//! Findings that duplicate an SCXML semantic rule with better scoping are
//! superseded (see [`superseded_by`]); everything else is kept.

use std::cell::RefCell;
use std::collections::BTreeMap;

use scxml_syntax::{Attribute, Document, Element, Node, Span, is_xml_blank, is_xml_space};
use scxml_validator_core::spans::{
    attribute_insertion_point, attribute_name_span, attribute_removal_span, element_name_span,
    end_tag_name_span,
};
use scxml_validator_core::{
    Applicability, Diagnostic, Fix, FixEdit, Label, Report, SCXML_NAMESPACE, Severity,
};
use xsd_schema::parser::frames::{AttributeUseKind, AttributeUseResult};
use xsd_schema::validation::{
    AttributeView, ElementStartView, EndElementInfo, EndOfAttributesView, SchemaInfo, TextKind,
    ValidationError, ValidationEventHandler, ValidationSink, ValidationWarning,
};

use xsd_schema::{SchemaSet, TypeKey};

use crate::vocabulary::{self, ElementSpec};

/// Validator event that produced one backend finding. Elements are
/// identified by their index in document order (pre-order).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Phase {
    /// `validate_element` for the element with this document-order index.
    ElementStart(usize),
    /// `validate_attribute` for one attribute of the element with this index.
    Attribute {
        element: usize,
        namespace_uri: String,
        local_name: String,
    },
    /// `validate_end_of_attributes` for the element with this index.
    EndOfAttributes(usize),
    /// Text validation inside the element with this index.
    Text(usize),
    /// `validate_end_element` for the element with this index.
    EndElement(usize),
    /// `end_validation` (document-wide identity checks).
    Document,
}

/// One backend finding with its producing event.
#[derive(Clone, Debug)]
pub(crate) struct Finding {
    pub severity: Severity,
    pub constraint: String,
    pub message: String,
    pub phase: Phase,
}

/// Findings shared between the runtime-owned sink and the driver handler.
#[derive(Debug, Default)]
pub(crate) struct Findings {
    pending: Vec<(Severity, String, String)>,
    pub tagged: Vec<Finding>,
    pub truncated: bool,
    limit: usize,
}

impl Findings {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            ..Self::default()
        }
    }

    fn push(&mut self, severity: Severity, constraint: String, message: String) {
        if self.pending.len().saturating_add(self.tagged.len()) >= self.limit {
            self.truncated = true;
            return;
        }
        self.pending.push((severity, constraint, message));
    }

    /// Attributes every pending finding to `phase`.
    pub fn claim(&mut self, phase: &Phase) {
        for (severity, constraint, message) in self.pending.drain(..) {
            self.tagged.push(Finding {
                severity,
                constraint,
                message,
                phase: phase.clone(),
            });
        }
    }
}

/// Runtime-owned sink that records findings in the shared collection.
pub(crate) struct SharedSink<'a>(pub &'a RefCell<Findings>);

impl ValidationSink for SharedSink<'_> {
    fn on_error(&mut self, error: ValidationError) {
        self.0
            .borrow_mut()
            .push(Severity::Error, error.constraint.to_owned(), error.message);
    }

    fn on_warning(&mut self, warning: ValidationWarning) {
        self.0
            .borrow_mut()
            .push(Severity::Warning, warning.code.to_owned(), warning.message);
    }
}

/// Driver hooks that attribute findings to validator events.
pub(crate) struct Tagger<'a> {
    pub findings: &'a RefCell<Findings>,
    schema_set: &'a SchemaSet,
    /// Unqualified attribute names the governing schema declares for each
    /// element outside the SCXML namespace that it declares, keyed by
    /// document-order index (suggestions for unknown extension attributes).
    pub declared: BTreeMap<usize, Vec<String>>,
    /// Document-order index of the next element to start.
    next: usize,
    open: Vec<usize>,
}

impl<'a> Tagger<'a> {
    pub fn new(findings: &'a RefCell<Findings>, schema_set: &'a SchemaSet) -> Self {
        Self {
            findings,
            schema_set,
            declared: BTreeMap::new(),
            next: 0,
            open: Vec::new(),
        }
    }

    fn current(&self) -> usize {
        self.open.last().copied().unwrap_or(0)
    }
}

impl ValidationEventHandler for Tagger<'_> {
    type Error = std::convert::Infallible;

    fn before_element(&mut self, _view: ElementStartView<'_>) -> Result<(), Self::Error> {
        self.open.push(self.next);
        self.next = self.next.saturating_add(1);
        Ok(())
    }

    fn after_element(
        &mut self,
        view: ElementStartView<'_>,
        info: &SchemaInfo,
    ) -> Result<(), Self::Error> {
        if view.namespace_uri != SCXML_NAMESPACE
            && info.element_decl.is_some()
            && let Some(schema_type) = info.schema_type
        {
            self.declared.insert(
                self.current(),
                declared_attributes(self.schema_set, schema_type),
            );
        }
        let phase = Phase::ElementStart(self.current());
        self.findings.borrow_mut().claim(&phase);
        Ok(())
    }

    fn after_attribute(
        &mut self,
        view: AttributeView<'_>,
        _info: &SchemaInfo,
    ) -> Result<(), Self::Error> {
        let phase = Phase::Attribute {
            element: self.current(),
            namespace_uri: view.namespace_uri.to_owned(),
            local_name: view.local_name.to_owned(),
        };
        self.findings.borrow_mut().claim(&phase);
        Ok(())
    }

    fn after_end_of_attributes(
        &mut self,
        _view: EndOfAttributesView<'_>,
    ) -> Result<(), Self::Error> {
        let phase = Phase::EndOfAttributes(self.current());
        self.findings.borrow_mut().claim(&phase);
        Ok(())
    }

    fn on_text(&mut self, _kind: TextKind, _text: &str) -> Result<(), Self::Error> {
        let phase = Phase::Text(self.current());
        self.findings.borrow_mut().claim(&phase);
        Ok(())
    }

    fn after_end_element(
        &mut self,
        _info: &EndElementInfo,
        _depth: usize,
    ) -> Result<(), Self::Error> {
        let element = self.open.pop().unwrap_or(0);
        self.findings
            .borrow_mut()
            .claim(&Phase::EndElement(element));
        Ok(())
    }
}

/// Most base types and attribute groups one type's attribute walk follows.
const MAX_ATTRIBUTE_SOURCES: usize = 64;

/// Unqualified attribute names a complex type declares, directly, through
/// its attribute groups, or inherited from its base types (less those a
/// restriction prohibits), sorted. Global attribute references are
/// namespace-qualified, so they are not candidates for an unprefixed name.
fn declared_attributes(schema_set: &SchemaSet, schema_type: TypeKey) -> Vec<String> {
    let mut names = Vec::new();
    let mut prohibited = Vec::new();
    let mut collect = |uses: &[AttributeUseResult]| {
        for attribute_use in uses {
            let attribute = &attribute_use.attribute;
            let (Some(name), None) = (attribute.name, &attribute.ref_name) else {
                continue;
            };
            if attribute.form.as_deref() == Some("qualified") {
                continue;
            }
            let name = schema_set.name_table.resolve(name);
            if attribute_use.use_kind == AttributeUseKind::Prohibited {
                prohibited.push(name);
            } else {
                names.push(name);
            }
        }
    };
    let mut groups = Vec::new();
    let mut next = Some(schema_type);
    let mut sources = 0_usize;
    while let Some(TypeKey::Complex(key)) = next
        && sources < MAX_ATTRIBUTE_SOURCES
        && let Some(complex) = schema_set.arenas.get_complex_type(key)
    {
        sources += 1;
        collect(&complex.attributes);
        groups.extend(complex.resolved_attribute_groups.iter().copied());
        next = complex.resolved_base_type;
    }
    while let Some(key) = groups.pop()
        && sources < MAX_ATTRIBUTE_SOURCES
    {
        sources += 1;
        if let Some(group) = schema_set.arenas.get_attribute_group(key) {
            collect(&group.attributes);
            groups.extend(group.resolved_ref);
            groups.extend(group.resolved_attribute_groups.iter().copied());
        }
    }
    names.retain(|name| !prohibited.contains(name));
    names.sort();
    names.dedup();
    names
}

/// Resolution context for one parsed instance document.
struct Instance<'a> {
    document: &'a Document,
    elements: BTreeMap<usize, (&'a Element, Option<&'a Element>)>,
}

impl<'a> Instance<'a> {
    fn new(document: &'a Document) -> Self {
        let mut elements = BTreeMap::new();
        let mut pending: Vec<(&Element, Option<&Element>)> = vec![(&document.root, None)];
        // Pre-order: children are pushed in reverse so the first pops first.
        while let Some((element, parent)) = pending.pop() {
            elements.insert(elements.len(), (element, parent));
            for child in element.children.iter().rev() {
                if let Node::Element(child) = child {
                    pending.push((child, Some(element)));
                }
            }
        }
        Self { document, elements }
    }

    fn element(&self, index: usize) -> Option<(&'a Element, Option<&'a Element>)> {
        self.elements.get(&index).copied()
    }
}

fn is_scxml(element: &Element) -> bool {
    element.name.namespace_uri.as_deref() == Some(SCXML_NAMESPACE)
}

fn scxml_spec(element: &Element) -> Option<&'static ElementSpec> {
    is_scxml(element)
        .then(|| vocabulary::element(&element.name.local_name))
        .flatten()
}

fn find_attribute<'a>(
    element: &'a Element,
    namespace_uri: &str,
    local_name: &str,
) -> Option<&'a Attribute> {
    element.attributes.iter().find(|attribute| {
        attribute.name.local_name == local_name
            && attribute.name.namespace_uri.as_deref().unwrap_or("") == namespace_uri
    })
}

/// Returns the semantic rule that owns this finding, when the XSD finding
/// merely restates a constraint the SCXML rules report with better scoping.
///
/// - `cvc-id.*`: SCXML identity is per document root (SCXML002/005/006);
///   XSD ID/IDREF uniqueness wrongly spans inline `<invoke><content>` roots.
/// - lexical errors on SCXML `id`, `initial`, and `target`: SCXML003/005/006.
/// - lexical errors on `<transition event>`: SCXML013.
/// - missing `<param name>`: SCXML007.
/// - `<initial>`/`<history>` without their single `<transition>`, or with
///   more than one: SCXML009/019.
/// - SCXML children or `initial` on `<final>`: SCXML031.
/// - `<donedata>` outside `<final>` or repeated: SCXML032.
/// - a `<content>`/`<param>` out of place in a `<donedata>` that has both:
///   SCXML018 (the children exclude each other; reordering cannot help).
pub(crate) fn superseded_by(
    instance_element: Option<&Element>,
    parent: Option<&Element>,
    finding: &Finding,
) -> Option<&'static str> {
    if finding.constraint.starts_with("cvc-id.") {
        return match instance_element {
            Some(element) if !is_scxml(element) => None,
            _ => Some("SCXML002"),
        };
    }
    let element = instance_element.filter(|element| is_scxml(element))?;
    let local = element.name.local_name.as_str();
    match &finding.phase {
        Phase::Attribute {
            namespace_uri,
            local_name,
            ..
        } if namespace_uri.is_empty() && is_value_constraint(&finding.constraint) => {
            match (local, local_name.as_str()) {
                (_, "id") => Some("SCXML003"),
                ("transition", "target") => Some("SCXML005"),
                ("scxml" | "state", "initial") => Some("SCXML006"),
                ("transition", "event") => Some("SCXML013"),
                _ => None,
            }
        }
        Phase::Attribute {
            namespace_uri,
            local_name,
            ..
        } if namespace_uri.is_empty()
            && local == "final"
            && local_name == "initial"
            && finding.constraint.starts_with("cvc-complex-type.3.2") =>
        {
            Some("SCXML031")
        }
        Phase::ElementStart(_) if finding.constraint == "cvc-complex-type.2.4" => {
            let parent = parent.filter(|parent| is_scxml(parent))?;
            match (parent.name.local_name.as_str(), local) {
                (_, "donedata") => Some("SCXML032"),
                ("donedata", "content" | "param") if has_content_and_param(parent) => {
                    Some("SCXML018")
                }
                ("initial", "transition") => Some("SCXML009"),
                ("history", "transition") => Some("SCXML019"),
                ("final", child) if !matches!(child, "onentry" | "onexit") => Some("SCXML031"),
                _ => None,
            }
        }
        Phase::EndOfAttributes(_)
            if finding.constraint == "cvc-complex-type.4"
                && local == "param"
                && finding.message.contains("'name'") =>
        {
            Some("SCXML007")
        }
        Phase::EndElement(_)
            if finding.constraint == "cvc-complex-type.2.4"
                && matches!(local, "initial" | "history") =>
        {
            Some("SCXML009")
        }
        _ => None,
    }
}

/// Whether an SCXML `<donedata>` has both `<content>` and `<param>` children.
fn has_content_and_param(donedata: &Element) -> bool {
    let has = |name: &str| {
        donedata.children.iter().any(|child| {
            matches!(child, scxml_syntax::Node::Element(child)
                if is_scxml(child) && child.name.local_name == name)
        })
    };
    has("content") && has("param")
}

fn is_value_constraint(constraint: &str) -> bool {
    [
        "cvc-datatype-valid",
        "cvc-enumeration-valid",
        "cvc-pattern-valid",
        "cvc-attribute",
        "cvc-simple-type",
        "cvc-length-valid",
        "cvc-minLength-valid",
        "cvc-maxLength-valid",
        "cvc-type",
    ]
    .iter()
    .any(|prefix| constraint.starts_with(prefix))
}

/// XML Schema specification anchor for one constraint family.
fn xsd_reference(constraint: &str) -> String {
    let section = if constraint.starts_with("cvc-complex-type") {
        "Part 1 §3.4.4 Element Locally Valid (Complex Type)"
    } else if constraint.starts_with("cvc-elt") {
        "Part 1 §3.3.4 Element Locally Valid (Element)"
    } else if constraint.starts_with("cvc-attribute") {
        "Part 1 §3.2.4 Attribute Locally Valid"
    } else if constraint.starts_with("cvc-datatype-valid") {
        "Part 2 §4.1.4 Datatype Valid"
    } else if constraint.starts_with("cvc-enumeration-valid") {
        "Part 2 §4.3.5.4 enumeration valid"
    } else if constraint.starts_with("cvc-pattern-valid") {
        "Part 2 §4.3.4.4 pattern valid"
    } else if constraint.starts_with("cvc-simple-type") || constraint.starts_with("cvc-type") {
        "Part 1 §3.14.4 String Valid"
    } else if constraint.starts_with("cvc-id") {
        "Part 1 §3.15.5 Validation Rule: Validation Root Valid (ID/IDREF Table)"
    } else {
        "Part 1 §3 Schema Component Details"
    };
    format!("XML Schema 1.0 {section} (`{constraint}`)")
}

/// Projects tagged findings into span-exact diagnostics. `declared` holds
/// the attribute names the governing schema declares for extension elements
/// (see [`Tagger::declared`]).
pub(crate) fn project(
    document: &Document,
    findings: Vec<Finding>,
    declared: &BTreeMap<usize, Vec<String>>,
) -> Report {
    let instance = Instance::new(document);
    let misnamed = misnamed_attributes(&instance, &findings);
    let mut report = Report::new();
    // Elements already diagnosed as undeclared; a following "not allowed
    // here" finding for the same element is the same root cause.
    let mut undeclared = Vec::new();
    for finding in &findings {
        if finding.constraint == "cvc-elt.1"
            && let Phase::ElementStart(offset) = finding.phase
        {
            undeclared.push(offset);
        }
    }
    for finding in findings {
        let element_offset = match &finding.phase {
            Phase::ElementStart(offset)
            | Phase::EndOfAttributes(offset)
            | Phase::Text(offset)
            | Phase::EndElement(offset) => Some(*offset),
            Phase::Attribute { element, .. } => Some(*element),
            Phase::Document => None,
        };
        let resolved = element_offset.and_then(|offset| instance.element(offset));
        if superseded_by(
            resolved.map(|(element, _)| element),
            resolved.and_then(|(_, parent)| parent),
            &finding,
        )
        .is_some()
        {
            continue;
        }
        if finding.constraint == "cvc-complex-type.2.4"
            && matches!(finding.phase, Phase::ElementStart(offset) if undeclared.contains(&offset))
        {
            continue;
        }
        // The missing attribute is reported once, on the misnamed one.
        if finding.constraint == "cvc-complex-type.4"
            && matches!(finding.phase, Phase::EndOfAttributes(offset) if misnamed.contains_key(&offset))
        {
            continue;
        }
        let declared = element_offset.and_then(|offset| declared.get(&offset));
        let mut diagnostic = diagnose(&instance, resolved, declared, &finding);
        if let Phase::Attribute {
            element: offset, ..
        } = &finding.phase
            && let Some((attribute, missing)) = misnamed.get(offset)
            && finding.constraint.starts_with("cvc-complex-type.3.2")
            && let Some((element, _)) = resolved
        {
            diagnostic = rename_to_required(document, element, attribute, missing, diagnostic);
        }
        report.push(diagnostic);
    }
    report.sort();
    report
}

/// Elements with exactly one missing required attribute and exactly one
/// undeclared unprefixed attribute (`<raise name="go"/>`), keyed by element
/// offset: the author misnamed the attribute, so the repair renames it and
/// keeps its value. Only offered when the value suits the required attribute
/// (a listed value for an enumeration, otherwise a non-empty token) and no
/// closer attribute name explains the misspelling.
fn misnamed_attributes<'a>(
    instance: &Instance<'a>,
    findings: &[Finding],
) -> BTreeMap<usize, (&'a Attribute, String)> {
    let mut missing: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut unknown: BTreeMap<usize, Vec<(&str, &str)>> = BTreeMap::new();
    for finding in findings {
        match (&finding.phase, finding.constraint.as_str()) {
            (Phase::EndOfAttributes(offset), "cvc-complex-type.4") => {
                if let Some(name) = finding.message.split('\'').nth(1) {
                    missing.entry(*offset).or_default().push(name.to_owned());
                }
            }
            (
                Phase::Attribute {
                    element,
                    namespace_uri,
                    local_name,
                },
                constraint,
            ) if constraint.starts_with("cvc-complex-type.3.2") => {
                unknown
                    .entry(*element)
                    .or_default()
                    .push((namespace_uri.as_str(), local_name.as_str()));
            }
            _ => {}
        }
    }
    let mut misnamed = BTreeMap::new();
    for (offset, names) in missing {
        let ([required], Some([(namespace_uri, local_name)])) =
            (names.as_slice(), unknown.get(&offset).map(Vec::as_slice))
        else {
            continue;
        };
        let Some((element, _)) = instance.element(offset) else {
            continue;
        };
        let Some(spec) = scxml_spec(element) else {
            continue;
        };
        let Some(attribute) = find_attribute(element, namespace_uri, local_name) else {
            continue;
        };
        let value = attribute.value.as_str();
        let suits = spec.attribute(required).is_some_and(|required_spec| {
            if required_spec.values.is_empty() {
                !value.is_empty() && !value.contains(is_xml_space)
            } else {
                required_spec.values.contains(&value)
            }
        });
        let closer = vocabulary::nearest(
            local_name,
            spec.attributes.iter().map(|attribute| attribute.name),
        )
        .is_some_and(|nearest| nearest != required);
        if namespace_uri.is_empty() && suits && !closer {
            misnamed.insert(offset, (attribute, required.clone()));
        }
    }
    misnamed
}

/// `<raise name="go"/>`: one finding that renames the undeclared attribute
/// to the missing required one, keeping the author's value.
fn rename_to_required(
    document: &Document,
    element: &Element,
    attribute: &Attribute,
    required: &str,
    mut diagnostic: Diagnostic,
) -> Diagnostic {
    let lexical = element.lexical_name.as_str();
    let unknown = attribute.lexical_name.as_str();
    let value = attribute.value.as_str();
    let counterpart = vocabulary::counterpart(&element.name.local_name, &attribute.name.local_name)
        .filter(|entry| entry.rename == required);
    diagnostic.message = format!(
        "`<{lexical}>` requires `{required}` but has `{unknown}`, which it does not accept"
    );
    diagnostic.help = Some(match counterpart {
        Some(entry) => format!("did you mean `{required}=\"{value}\"`? {}", entry.reason),
        None => format!(
            "did you mean `{required}=\"{value}\"`? rename `{unknown}` to `{required}` to keep the value"
        ),
    });
    diagnostic.fixes = vec![Fix::replace(
        format!("rename `{unknown}` to `{required}`"),
        applicability(counterpart.is_some_and(|entry| entry.same_value)),
        attribute_name_span(document, attribute),
        required,
    )];
    diagnostic.with_secondary(
        Label::new(element_name_span(document, element))
            .with_message(format!("missing required `{required}`")),
    )
}

fn base_diagnostic(finding: &Finding, message: String, span: Span) -> Diagnostic {
    let code = format!("XSD-{}", finding.constraint);
    let diagnostic = match finding.severity {
        Severity::Error => Diagnostic::error(code, message, span),
        _ => Diagnostic::warning(code, message, span)
            .with_category(scxml_validator_core::DiagnosticCategory::Schema),
    };
    diagnostic.with_spec_reference(xsd_reference(&finding.constraint))
}

fn schema_note(element: &Element, finding: &Finding) -> String {
    let schema = if is_scxml(element) {
        "the W3C SCXML 1.0 schema".to_owned()
    } else {
        match element.name.namespace_uri.as_deref() {
            Some(uri) => format!("the schema registered for `{uri}`"),
            None => "the registered schema".to_owned(),
        }
    };
    format!(
        "{schema} rejects this under XML Schema constraint `{}`",
        finding.constraint
    )
}

fn quoted_list(values: &[&str]) -> String {
    values
        .iter()
        .map(|value| format!("`{value}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `a`, `a or b`, `a, b, or c`, each written by `quote`.
fn alternatives(values: &[&str], quote: impl Fn(&str) -> String) -> String {
    let quoted: Vec<String> = values.iter().map(|value| quote(value)).collect();
    match quoted.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [first, second] => format!("{first} or {second}"),
        [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
    }
}

/// Machine-applicable when the repair is exact, maybe-incorrect otherwise.
fn applicability(exact: bool) -> Applicability {
    if exact {
        Applicability::MachineApplicable
    } else {
        Applicability::MaybeIncorrect
    }
}

fn tag_list(values: &[&str]) -> String {
    values
        .iter()
        .map(|value| format!("`<{value}>`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[allow(
    clippy::too_many_lines,
    reason = "one exhaustive match maps every backend constraint family to its span, message, help, and fix"
)]
fn diagnose(
    instance: &Instance<'_>,
    resolved: Option<(&Element, Option<&Element>)>,
    declared: Option<&Vec<String>>,
    finding: &Finding,
) -> Diagnostic {
    let document = instance.document;
    let Some((element, parent)) = resolved else {
        let root = element_name_span(document, &document.root);
        return base_diagnostic(finding, finding.message.clone(), root)
            .with_primary_message("rejected by the document-wide schema check")
            .with_note(schema_note(&document.root, finding));
    };
    let name = element_name_span(document, element);
    let lexical = element.lexical_name.as_str();
    let spec = scxml_spec(element);
    let parent_spec = parent.and_then(scxml_spec);
    let local = element.name.local_name.as_str();
    let mut diagnostic = match (&finding.phase, finding.constraint.as_str()) {
        (Phase::ElementStart(_), "cvc-elt.1") => {
            let mut diagnostic = base_diagnostic(
                finding,
                if is_scxml(element) {
                    format!("`<{lexical}>` is not an SCXML 1.0 element")
                } else {
                    format!("element `<{lexical}>` is not declared by its schema")
                },
                name,
            )
            .with_primary_message("unknown element");
            let candidates = parent_spec.map_or_else(
                || {
                    vocabulary::ELEMENTS
                        .iter()
                        .map(|spec| spec.name)
                        .collect::<Vec<_>>()
                },
                ElementSpec::allowed_children,
            );
            let suggestions = if is_scxml(element) {
                vocabulary::closest(&element.name.local_name, candidates.iter().copied())
            } else {
                Vec::new()
            };
            if !suggestions.is_empty() {
                let prefix = lexical
                    .split_once(':')
                    .map_or(String::new(), |(prefix, _)| format!("{prefix}:"));
                // One closest element that declares every unprefixed
                // attribute the misspelled one carries is the whole repair.
                let exact = match suggestions.as_slice() {
                    [suggestion] => vocabulary::element(suggestion).is_some_and(|spec| {
                        element.attributes.iter().all(|attribute| {
                            attribute.name.namespace_uri.is_some()
                                || spec.attribute(&attribute.name.local_name).is_some()
                        })
                    }),
                    _ => false,
                };
                diagnostic = diagnostic.with_help(format!(
                    "did you mean {}?",
                    alternatives(&suggestions, |suggestion| format!("`<{suggestion}>`"))
                ));
                for suggestion in &suggestions {
                    let replacement = format!("{prefix}{suggestion}");
                    let mut edits = vec![FixEdit {
                        span: name,
                        replacement: replacement.clone(),
                    }];
                    if let Some(end) = end_tag_name_span(document, element) {
                        edits.push(FixEdit {
                            span: end,
                            replacement,
                        });
                    }
                    diagnostic = diagnostic.with_fix(Fix {
                        message: format!(
                            "an SCXML element with a similar name exists: `<{suggestion}>`"
                        ),
                        applicability: applicability(exact),
                        edits,
                    });
                }
            } else if let Some(parent_spec) = parent_spec {
                diagnostic = diagnostic.with_help(format!(
                    "`<{}>` accepts {}; elements from another namespace must use a namespace prefix",
                    parent_spec.name,
                    tag_list(&parent_spec.allowed_children())
                ));
            } else {
                diagnostic = diagnostic.with_help(
                    "declare the element in the namespace's schema, or correct its name or namespace",
                );
            }
            if let Some(end) = end_tag_name_span(document, element) {
                diagnostic = diagnostic.with_secondary(
                    Label::new(end).with_message("closing tag of the unknown element"),
                );
            }
            diagnostic
        }
        (Phase::ElementStart(_), "cvc-complex-type.2.4") => {
            let parent_name = parent.map_or("parent", |parent| parent.lexical_name.as_str());
            let mut diagnostic = base_diagnostic(
                finding,
                format!("`<{lexical}>` is not allowed here inside `<{parent_name}>`"),
                name,
            )
            .with_primary_message("not allowed at this position");
            if let Some(parent) = parent {
                diagnostic = diagnostic.with_secondary(
                    Label::new(element_name_span(document, parent))
                        .with_message(format!("in the content of this `<{parent_name}>`")),
                );
            }
            let is_executable = is_scxml(element)
                && vocabulary::EXECUTABLE_CONTENT.contains(&element.name.local_name.as_str());
            if is_executable && parent_spec.is_some_and(|spec| !spec.executable_children) {
                diagnostic = diagnostic.with_help(format!(
                    "executable content such as `<{}>` belongs inside `<onentry>`, `<onexit>`, or `<transition>`",
                    element.name.local_name
                ));
                if parent_spec.is_some_and(|spec| spec.children.contains(&"onentry")) {
                    let prefix = lexical
                        .split_once(':')
                        .map_or(String::new(), |(prefix, _)| format!("{prefix}:"));
                    diagnostic = diagnostic.with_fix(Fix {
                        message: "run it on entry by wrapping it in `<onentry>`".to_owned(),
                        applicability: Applicability::MaybeIncorrect,
                        edits: vec![
                            FixEdit {
                                span: Span::new(
                                    element.span.source,
                                    element.span.start,
                                    element.span.start,
                                ),
                                replacement: format!("<{prefix}onentry>"),
                            },
                            FixEdit {
                                span: Span::new(
                                    element.span.source,
                                    element.span.end,
                                    element.span.end,
                                ),
                                replacement: format!("</{prefix}onentry>"),
                            },
                        ],
                    });
                }
            } else if let (Some(parent), Some(parent_spec)) = (parent, parent_spec)
                && is_scxml(element)
                && parent_spec
                    .allowed_children()
                    .contains(&element.name.local_name.as_str())
            {
                diagnostic = misplaced_child(document, parent, element, diagnostic);
            } else if let Some(parent_spec) = parent_spec {
                let allowed = parent_spec.allowed_children();
                diagnostic = diagnostic.with_help(if allowed.is_empty() {
                    format!("`<{}>` accepts no SCXML child elements", parent_spec.name)
                } else {
                    format!(
                        "`<{}>` accepts {} (each within its schema cardinality and order)",
                        parent_spec.name,
                        tag_list(&allowed)
                    )
                });
            }
            diagnostic
        }
        (
            Phase::Attribute {
                namespace_uri,
                local_name,
                ..
            },
            constraint,
        ) => {
            let attribute = find_attribute(element, namespace_uri, local_name);
            let attribute_lexical = attribute.map_or(local_name.as_str(), |attribute| {
                attribute.lexical_name.as_str()
            });
            if constraint.starts_with("cvc-complex-type.3.2") {
                let span =
                    attribute.map_or(name, |attribute| attribute_name_span(document, attribute));
                let mut diagnostic = base_diagnostic(
                    finding,
                    format!("attribute `{attribute_lexical}` is not allowed on `<{lexical}>`"),
                    span,
                )
                .with_primary_message("unknown attribute");
                let kind_as_attribute = attribute.filter(|attribute| {
                    local == "state"
                        && is_scxml(element)
                        && attribute.name.namespace_uri.is_none()
                        && attribute.name.local_name == "type"
                        && matches!(attribute.value.as_str(), "final" | "parallel")
                });
                // Names the element declares that it does not already carry
                // (a rename onto one of those would duplicate it).
                let accepted: Vec<&str> = match (spec, declared) {
                    (Some(spec), _) => spec
                        .attributes
                        .iter()
                        .map(|attribute| attribute.name)
                        .collect(),
                    (None, Some(declared)) => declared.iter().map(String::as_str).collect(),
                    (None, None) => Vec::new(),
                };
                let absent: Vec<&str> = accepted
                    .iter()
                    .copied()
                    .filter(|candidate| find_attribute(element, "", candidate).is_none())
                    .collect();
                let counterpart = spec
                    .filter(|_| namespace_uri.is_empty())
                    .and_then(|_| vocabulary::counterpart(local, local_name))
                    .filter(|entry| absent.contains(&entry.rename));
                let suggestions = if namespace_uri.is_empty() {
                    vocabulary::closest(local_name, absent.iter().copied())
                } else {
                    Vec::new()
                };
                if let Some(attribute) = kind_as_attribute {
                    diagnostic = state_kind_rename(document, element, attribute, diagnostic);
                } else if let (Some(entry), Some(attribute)) = (counterpart, attribute) {
                    let owner_spec = vocabulary::element(entry.owner).map(|owner| owner.spec);
                    diagnostic = diagnostic
                        .with_help(format!("did you mean `{}`? {}", entry.rename, entry.reason))
                        .with_fix(Fix::replace(
                            format!("rename `{attribute_lexical}` to `{}`", entry.rename),
                            applicability(entry.same_value),
                            attribute_name_span(document, attribute),
                            entry.rename,
                        ));
                    if let Some(owner_spec) = owner_spec {
                        diagnostic = diagnostic.with_spec_reference(owner_spec);
                    }
                } else if let (false, Some(attribute)) = (suggestions.is_empty(), attribute) {
                    // One closest name whose declared values admit this value
                    // is the whole repair; a tie, or a value the attribute
                    // does not accept, is a guess.
                    let exact = match (suggestions.as_slice(), spec) {
                        ([suggestion], Some(spec)) => {
                            spec.attribute(suggestion).is_some_and(|target| {
                                target.values.is_empty()
                                    || target.values.contains(&attribute.value.as_str())
                            })
                        }
                        _ => false,
                    };
                    diagnostic = diagnostic.with_help(format!(
                        "did you mean {}?",
                        alternatives(&suggestions, |suggestion| format!("`{suggestion}`"))
                    ));
                    for suggestion in &suggestions {
                        diagnostic = diagnostic.with_fix(Fix::replace(
                            format!(
                                "`<{lexical}>` has an attribute with a similar name: `{suggestion}`"
                            ),
                            applicability(exact),
                            attribute_name_span(document, attribute),
                            *suggestion,
                        ));
                    }
                } else {
                    if spec.is_some() || declared.is_some() {
                        diagnostic = diagnostic.with_help(if accepted.is_empty() {
                            format!("`<{lexical}>` declares no attributes; extension attributes must use a namespace prefix")
                        } else {
                            format!(
                                "`<{lexical}>` accepts {}; extension attributes must use a namespace prefix",
                                quoted_list(&accepted)
                            )
                        });
                    }
                    if let Some(attribute) = attribute {
                        diagnostic = diagnostic.with_fix(Fix::replace(
                            format!("remove the `{attribute_lexical}` attribute"),
                            Applicability::MaybeIncorrect,
                            attribute_removal_span(document, attribute),
                            "",
                        ));
                    }
                }
                diagnostic
            } else {
                let span = attribute.map_or(name, |attribute| attribute.value_span);
                let value = attribute.map_or("", |attribute| attribute.value.as_str());
                let mut diagnostic = base_diagnostic(
                    finding,
                    format!("invalid value `{value}` for attribute `{attribute_lexical}` on `<{lexical}>`"),
                    span,
                )
                .with_primary_message(finding.message.clone());
                if let Some(attribute_spec) = spec
                    .filter(|_| namespace_uri.is_empty())
                    .and_then(|spec| spec.attribute(local_name))
                {
                    if attribute_spec.values.is_empty() {
                        diagnostic = diagnostic.with_help(format!(
                            "`{attribute_lexical}` expects {}",
                            attribute_spec.lexical
                        ));
                    } else {
                        diagnostic = diagnostic.with_help(format!(
                            "`{attribute_lexical}` must be one of {}",
                            quoted_list(attribute_spec.values)
                        ));
                        let suggestion = if attribute_spec.values.len() == 1 {
                            attribute_spec.values.first().copied()
                        } else {
                            vocabulary::nearest(value, attribute_spec.values.iter().copied())
                        };
                        if let (Some(suggestion), Some(attribute)) = (suggestion, attribute) {
                            diagnostic = diagnostic.with_fix(Fix::replace(
                                format!("use the allowed value `{suggestion}`"),
                                if attribute_spec.values.len() == 1 {
                                    Applicability::MachineApplicable
                                } else {
                                    Applicability::MaybeIncorrect
                                },
                                attribute.value_span,
                                suggestion,
                            ));
                        }
                    }
                }
                diagnostic
            }
        }
        (Phase::EndOfAttributes(_), "cvc-complex-type.4") => {
            let missing = finding
                .message
                .split('\'')
                .nth(1)
                .unwrap_or_default()
                .to_owned();
            let mut diagnostic = base_diagnostic(
                finding,
                format!("`<{lexical}>` is missing required attribute `{missing}`"),
                name,
            )
            .with_primary_message(format!("missing `{missing}`"));
            let attribute_spec = spec.and_then(|spec| spec.attribute(&missing));
            let insertion = attribute_insertion_point(document, element);
            match attribute_spec {
                Some(attribute_spec) if attribute_spec.values.len() == 1 => {
                    let value = attribute_spec.values.first().copied().unwrap_or_default();
                    diagnostic = diagnostic
                        .with_help(format!("`{missing}` must be `{value}`"))
                        .with_fix(Fix::replace(
                            format!("add `{missing}=\"{value}\"`"),
                            Applicability::MachineApplicable,
                            insertion,
                            format!(" {missing}=\"{value}\""),
                        ));
                }
                Some(attribute_spec) => {
                    diagnostic = diagnostic
                        .with_help(format!("add `{missing}` with {}", attribute_spec.lexical))
                        .with_fix(Fix::replace(
                            format!("add the required `{missing}` attribute"),
                            Applicability::HasPlaceholders,
                            insertion,
                            format!(" {missing}=\"…\""),
                        ));
                }
                None if !missing.is_empty() => {
                    diagnostic = diagnostic
                        .with_help(format!(
                            "add the `{missing}` attribute required by the schema"
                        ))
                        .with_fix(Fix::replace(
                            format!("add the required `{missing}` attribute"),
                            Applicability::HasPlaceholders,
                            insertion,
                            format!(" {missing}=\"…\""),
                        ));
                }
                None => {}
            }
            diagnostic
        }
        (Phase::Text(_), constraint) if constraint.starts_with("cvc-complex-type.2") => {
            let text = first_text(element);
            let span = text.map_or(name, |text| trimmed_span(document, text));
            let mut diagnostic = base_diagnostic(
                finding,
                format!("`<{lexical}>` cannot contain character data"),
                span,
            )
            .with_primary_message("unexpected text")
            .with_secondary(Label::new(name).with_message(format!("`<{lexical}>` has element-only content")))
            .with_help("remove the text; put values in `<data>`, `<content>`, `<script>`, or a `<log expr=\"…\">`");
            if let Some(text) = text {
                diagnostic = diagnostic.with_fix(Fix::replace(
                    "remove the stray text",
                    Applicability::MaybeIncorrect,
                    trimmed_span(document, text),
                    "",
                ));
            }
            diagnostic
        }
        (Phase::EndElement(_), "cvc-complex-type.2.4") => {
            let mut diagnostic = base_diagnostic(
                finding,
                format!("`<{lexical}>` is missing required child elements"),
                name,
            )
            .with_primary_message("content model is incomplete");
            if let Some(end) = end_tag_name_span(document, element) {
                diagnostic = diagnostic.with_secondary(
                    Label::new(end).with_message("expected more content before this closing tag"),
                );
            }
            if let Some(spec) = spec {
                diagnostic = diagnostic.with_help(format!(
                    "`<{}>` requires children from {}",
                    spec.name,
                    tag_list(&spec.allowed_children())
                ));
            }
            diagnostic
        }
        (Phase::ElementStart(_) | Phase::EndOfAttributes(_) | Phase::EndElement(_), _) => {
            base_diagnostic(finding, finding.message.clone(), name)
                .with_primary_message("rejected by the schema")
        }
        (Phase::Text(_), _) => {
            let span = first_text(element).map_or(name, |text| trimmed_span(document, text));
            base_diagnostic(finding, finding.message.clone(), span)
                .with_primary_message("rejected by the schema")
        }
        (Phase::Document, _) => base_diagnostic(finding, finding.message.clone(), name),
    };
    let backend_shown = diagnostic.message == finding.message
        || diagnostic.primary.message.as_deref() == Some(finding.message.as_str());
    // A child the content model rejects is rejected by the schema of the
    // element whose content model it is, not by the child's own schema.
    let owner = match (&finding.phase, parent) {
        (Phase::ElementStart(_), Some(parent)) if finding.constraint == "cvc-complex-type.2.4" => {
            parent
        }
        _ => element,
    };
    diagnostic = diagnostic.with_note(if backend_shown {
        schema_note(owner, finding)
    } else {
        format!("{}: {}", schema_note(owner, finding), finding.message)
    });
    if let Some(spec) = spec {
        diagnostic = diagnostic.with_spec_reference(spec.spec);
    }
    diagnostic
}

/// Help for a child the parent accepts, but not at this position: names the
/// sibling it conflicts with and, for `<if>` branches, moves the branch.
fn misplaced_child(
    document: &Document,
    parent: &Element,
    element: &Element,
    diagnostic: Diagnostic,
) -> Diagnostic {
    let siblings: Vec<&Element> = parent
        .children
        .iter()
        .filter_map(|child| match child {
            Node::Element(child) => Some(child),
            Node::Text(_) => None,
        })
        .collect();
    let before: Vec<&Element> = siblings
        .iter()
        .copied()
        .take_while(|sibling| sibling.span.start < element.span.start)
        .collect();
    let local = element.name.local_name.as_str();
    let parent_local = parent.name.local_name.as_str();
    let earlier_else = before
        .iter()
        .copied()
        .find(|sibling| is_scxml(sibling) && sibling.name.local_name == "else");
    if parent_local == "if"
        && matches!(local, "elseif" | "else")
        && let Some(earlier_else) = earlier_else
    {
        let else_label = Label::new(element_name_span(document, earlier_else));
        if local == "else" {
            return diagnostic
                .with_secondary(else_label.with_message("the first `<else>` is here"))
                .with_help("`<if>` has at most one `<else>`, and it must be the last branch: merge the two `<else>` branches or turn one into `<elseif cond=\"…\">` before `<else>`");
        }
        // An `<elseif>` partitions `<if>`: its branch is every sibling up to
        // the next `<elseif>`/`<else>` or the end of `<if>`.
        let branch_end = siblings
            .iter()
            .copied()
            .find(|sibling| {
                sibling.span.start > element.span.start
                    && is_scxml(sibling)
                    && matches!(sibling.name.local_name.as_str(), "elseif" | "else")
            })
            .map(|sibling| sibling.span.start)
            .or_else(|| {
                end_tag_name_span(document, parent).map(|name| name.start.saturating_sub(2))
            });
        let diagnostic = diagnostic
            .with_secondary(else_label.with_message("`<else>` starts the last branch here"))
            .with_help("`<else>` must be the last branch of `<if>`: move this `<elseif>` and its content before `<else>`");
        let Some(branch) = branch_end
            .map(|end| Span::new(element.span.source, element.span.start, end))
            .filter(|branch| branch.end > branch.start)
        else {
            return diagnostic;
        };
        let Some(text) = document.source.slice(branch) else {
            return diagnostic;
        };
        return diagnostic.with_fix(Fix {
            message: "move the `<elseif>` branch before `<else>`".to_owned(),
            applicability: Applicability::MaybeIncorrect,
            edits: vec![
                FixEdit {
                    span: Span::new(
                        earlier_else.span.source,
                        earlier_else.span.start,
                        earlier_else.span.start,
                    ),
                    replacement: text.to_owned(),
                },
                FixEdit {
                    span: branch,
                    replacement: String::new(),
                },
            ],
        });
    }
    let mut diagnostic = diagnostic.with_help(format!(
        "`<{parent_local}>` accepts `<{local}>`, but not at this position: its content model fixes where `<{local}>` may appear and how often; move or remove this element"
    ));
    if let Some(previous) = before.last() {
        diagnostic = diagnostic.with_secondary(
            Label::new(element_name_span(document, previous)).with_message(format!(
                "`<{local}>` cannot follow this `<{}>`",
                previous.lexical_name
            )),
        );
    }
    diagnostic
}

/// `<state type="final">` (or `"parallel"`): SCXML spells the state kind as
/// the element name, so the repair renames the element and drops `type`.
fn state_kind_rename(
    document: &Document,
    element: &Element,
    attribute: &Attribute,
    diagnostic: Diagnostic,
) -> Diagnostic {
    let kind = attribute.value.as_str();
    let prefix = element
        .lexical_name
        .split_once(':')
        .map_or(String::new(), |(prefix, _)| format!("{prefix}:"));
    let replacement = format!("{prefix}{kind}");
    let mut edits = vec![
        FixEdit {
            span: element_name_span(document, element),
            replacement: replacement.clone(),
        },
        FixEdit {
            span: attribute_removal_span(document, attribute),
            replacement: String::new(),
        },
    ];
    if let Some(end) = end_tag_name_span(document, element) {
        edits.push(FixEdit {
            span: end,
            replacement,
        });
    }
    diagnostic
        .with_help(format!(
            "SCXML has no state `type` attribute: a {kind} state is written as the element `<{kind}>`"
        ))
        .with_fix(Fix {
            message: format!("write the state as `<{kind}>`"),
            applicability: Applicability::MaybeIncorrect,
            edits,
        })
}

fn first_text(element: &Element) -> Option<&scxml_syntax::Text> {
    element.children.iter().find_map(|child| match child {
        Node::Text(text) if !is_xml_blank(&text.value) => Some(text),
        _ => None,
    })
}

/// Span of the text node without leading and trailing XML whitespace.
fn trimmed_span(document: &Document, text: &scxml_syntax::Text) -> Span {
    let Some(raw) = document.source.slice(text.span) else {
        return text.span;
    };
    let leading = raw.len() - raw.trim_start_matches(is_xml_space).len();
    let trailing = raw.len() - raw.trim_end_matches(is_xml_space).len();
    Span::new(
        text.span.source,
        text.span.start + leading,
        text.span
            .end
            .saturating_sub(trailing)
            .max(text.span.start + leading),
    )
}

/// `cvc-complex-type.2.3` for character data the schema engine reads as
/// white space but XML does not: element-only or empty content may hold
/// XML white space (XML 1.0 §2.3 `S`: #x20, #x9, #xD, #xA) only, and the
/// engine skips any Unicode white space (such as U+1680 or U+00A0). Checks
/// every SCXML element the schema declares with content that is not mixed,
/// wherever it stands: the schema's wildcards are all
/// `processContents="lax"`, so the engine assesses an SCXML element against
/// its global declaration under `<data>`, `<assign>`, `<content>`, or an
/// extension element too, at any depth. An SCXML-namespace name with no
/// declaration has no element-only content to assess under a lax wildcard
/// (XSD Part 1 §3.10.1), so it is skipped. The walk is iterative, so its depth is not bounded by the stack.
/// `None` once the findings would exceed `limit` (the messages the schema
/// engine left room for).
pub(crate) fn non_xml_space_text(document: &Document, limit: usize) -> Option<Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let mut pending = vec![&document.root];
    while let Some(element) = pending.pop() {
        for child in element.children.iter().rev() {
            if let Node::Element(child) = child {
                pending.push(child);
            }
        }
        if scxml_spec(element).is_none_or(|spec| spec.mixed) {
            continue;
        }
        let missed = element.children.iter().find_map(|child| match child {
            Node::Text(text) if !is_xml_blank(&text.value) && text.value.trim().is_empty() => {
                Some(text)
            }
            _ => None,
        });
        if let Some(text) = missed {
            if diagnostics.len() >= limit {
                return None;
            }
            let lexical = &element.lexical_name;
            let span = trimmed_span(document, text);
            let constraint = "cvc-complex-type.2.3";
            diagnostics.push(
                Diagnostic::error(
                    format!("XSD-{constraint}"),
                    format!("`<{lexical}>` cannot contain character data"),
                    span,
                )
                .with_spec_reference(xsd_reference(constraint))
                .with_primary_message("unexpected text")
                .with_secondary(
                    Label::new(element_name_span(document, element))
                        .with_message(format!("`<{lexical}>` has element-only content")),
                )
                .with_help("remove the text; put values in `<data>`, `<content>`, `<script>`, or a `<log expr=\"…\">`")
                .with_fix(Fix::replace(
                    "remove the stray text",
                    Applicability::MaybeIncorrect,
                    span,
                    "",
                )),
            );
        }
    }
    Some(diagnostics)
}
