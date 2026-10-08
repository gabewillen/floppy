//! Attributes and elements from other namespaces on and in SCXML elements.
//!
//! SCXML 1.0 and its W3C schema allow both (the schema's `##other`
//! wildcards). What one means depends on its namespace:
//!
//! * the XML namespace (`xml:base`, which governs the `src` URIs it is in
//!   scope for, and `xml:lang` and `xml:space`, which carry no SCXML
//!   behavior) and the XML Schema instance namespace (`xsi:*`) are accepted
//!   and ignored;
//! * the built-in vocabularies (`env`, `event`, `datamodel`) have their own
//!   rules (`ENV`, `EVENT`, `DATAMODEL`);
//! * an element in another namespace in executable content is a custom
//!   action (SCXML 1.0 §4.10), which runs in its namespace's extension
//!   component: its namespace is an extension namespace
//!   ([`extension_namespaces`]), which needs a package;
//! * any other attribute or element is metadata the processor does not
//!   execute: a package that binds its namespace (one with a registered
//!   schema) checks it with its schema and validator, and one in a
//!   namespace nothing binds has no effect, which `LINT022` reports.
//!
//! * `LINT022` (warning): attributes in a namespace nothing binds on one
//!   SCXML element (one finding, the first primary, the rest labeled), or
//!   an element in such a namespace outside executable content, with a
//!   maybe-incorrect removal. The walk visits the SCXML elements lowering
//!   visits: each machine's states and their SCXML children, not the
//!   bodies of `<data>`, `<content>`, or `<assign>` (XML values) or of
//!   custom actions. A foreign child of `<donedata>`, `<raise>`,
//!   `<elseif>`, or `<else>`, whose content models have no wildcard, is
//!   the schema's finding.
//!
//! Pure and bounded by the document; it runs inside the validator's
//! boundary span and records no telemetry of its own.

use std::collections::BTreeSet;

use scxml_reporting::Applicability;
use scxml_syntax::{Attribute, Document, Element, Node};
use scxml_types::{DATAMODEL_NAMESPACE_URI, ENV_NAMESPACE_URI, EVENT_NAMESPACE_URI};

use crate::semantic::{SCXML_NAMESPACE, machine_roots};
use crate::spans::attribute_removal_span;
use crate::{Diagnostic, DiagnosticCategory, Fix, FixEdit, Label};

pub(crate) const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
pub(crate) const XSI_NAMESPACE: &str = "http://www.w3.org/2001/XMLSchema-instance";
/// Maximum further attributes labeled on one `LINT022` finding.
const MAX_LABELS: usize = 16;
/// SCXML elements whose element children are executable content.
const EXECUTABLE_CONTENT_PARENTS: [&str; 6] = [
    "onentry",
    "onexit",
    "transition",
    "finalize",
    "if",
    "foreach",
];
/// SCXML elements whose content is an XML value lowering keeps as data.
const XML_VALUE_PARENTS: [&str; 3] = ["data", "content", "assign"];
/// SCXML elements whose content models admit no element from another
/// namespace (the schema reports one).
const CLOSED_PARENTS: [&str; 4] = ["donedata", "raise", "elseif", "else"];

/// Whether `namespace` is built into XML, XML Schema, SCXML, or this
/// toolchain's vocabularies, so no package binds it.
pub(crate) fn is_built_in(namespace: &str) -> bool {
    matches!(
        namespace,
        SCXML_NAMESPACE
            | XML_NAMESPACE
            | XSI_NAMESPACE
            | ENV_NAMESPACE_URI
            | EVENT_NAMESPACE_URI
            | DATAMODEL_NAMESPACE_URI
    )
}

/// The extension namespaces of `document`, sorted: the namespaces of its
/// custom actions (an element in another namespace whose parent is an SCXML
/// `<onentry>`, `<onexit>`, `<transition>`, `<finalize>`, `<if>`, or
/// `<foreach>`) and of the elements and attributes inside them, other than
/// the built-in ones. Each needs an extension package; an attribute or
/// element from another namespace anywhere else has no behavior to bind.
#[must_use]
pub fn extension_namespaces(document: &Document) -> Vec<String> {
    let mut namespaces = BTreeSet::new();
    let mut pending = vec![&document.root];
    while let Some(element) = pending.pop() {
        let container = element.name.namespace_uri.as_deref() == Some(SCXML_NAMESPACE)
            && EXECUTABLE_CONTENT_PARENTS.contains(&element.name.local_name.as_str());
        for child in element.children.iter().filter_map(element_node) {
            let foreign = child
                .name
                .namespace_uri
                .as_deref()
                .is_some_and(|namespace| namespace != SCXML_NAMESPACE);
            if container && foreign {
                child.visit(&mut |inner| {
                    namespaces.extend(
                        inner
                            .name
                            .namespace_uri
                            .iter()
                            .chain(
                                inner
                                    .attributes
                                    .iter()
                                    .filter_map(|attribute| attribute.name.namespace_uri.as_ref()),
                            )
                            .filter(|namespace| !is_built_in(namespace))
                            .cloned(),
                    );
                });
            } else {
                pending.push(child);
            }
        }
    }
    namespaces.into_iter().collect()
}

/// `LINT022` for every attribute and element in a namespace `is_bound`
/// rejects (and that is not built in) that the walk reaches.
pub(crate) fn unbound_namespace_diagnostics(
    document: &Document,
    is_bound: &dyn Fn(&str) -> bool,
) -> Vec<Diagnostic> {
    let unbound = |namespace: Option<&str>| {
        namespace.is_some_and(|namespace| !is_built_in(namespace) && !is_bound(namespace))
    };
    let mut diagnostics = Vec::new();
    for machine in machine_roots(&document.root) {
        let mut pending = vec![machine];
        while let Some(element) = pending.pop() {
            let attributes: Vec<&Attribute> = element
                .attributes
                .iter()
                .filter(|attribute| unbound(attribute.name.namespace_uri.as_deref()))
                .collect();
            if !attributes.is_empty() {
                diagnostics.push(unbound_attributes(document, element, &attributes));
            }
            let local = element.name.local_name.as_str();
            if XML_VALUE_PARENTS.contains(&local) {
                continue;
            }
            let custom_actions = EXECUTABLE_CONTENT_PARENTS.contains(&local);
            for child in element.children.iter().filter_map(element_node).rev() {
                match child.name.namespace_uri.as_deref() {
                    Some(SCXML_NAMESPACE) => {
                        // A nested `<scxml>` is a machine of its own.
                        if child.name.local_name != "scxml" {
                            pending.push(child);
                        }
                    }
                    namespace
                        if !custom_actions
                            && !CLOSED_PARENTS.contains(&local)
                            && unbound(namespace) =>
                    {
                        diagnostics.push(unbound_element(document, element, child));
                    }
                    _ => {}
                }
            }
        }
    }
    diagnostics
}

fn element_node(node: &Node) -> Option<&Element> {
    match node {
        Node::Element(element) => Some(element),
        Node::Text(_) => None,
    }
}

const NOTE: &str = "SCXML 1.0 allows attributes and elements from other namespaces on and in its elements (the W3C schema's `##other` wildcards), and the processor executes none of them: only a package that binds the namespace (its schema and validator) reads them, and no extension package or registered schema binds this one";

fn unbound_attributes(
    document: &Document,
    element: &Element,
    attributes: &[&Attribute],
) -> Diagnostic {
    let Some((first, rest)) = attributes.split_first() else {
        return Diagnostic::warning("LINT022", "", element.start_tag_span);
    };
    let namespace = |attribute: &Attribute| {
        format!(
            "namespace `{}`",
            attribute.name.namespace_uri.as_deref().unwrap_or_default()
        )
    };
    let mut diagnostic = Diagnostic::warning(
        "LINT022",
        if rest.is_empty() {
            format!(
                "attribute `{}` in an unbound namespace has no effect",
                first.lexical_name
            )
        } else {
            format!(
                "{} attributes in unbound namespaces on `<{}>` have no effect",
                attributes.len(),
                element.lexical_name
            )
        },
        first.span,
    )
    .with_category(DiagnosticCategory::Lint)
    .with_primary_message(namespace(first));
    for attribute in rest.iter().take(MAX_LABELS) {
        diagnostic = diagnostic
            .with_secondary(Label::new(attribute.span).with_message(namespace(attribute)));
    }
    diagnostic
        .with_note(NOTE)
        .with_help("bind the namespace's package (`--extension-package MANIFEST.json`) or register its schema (`--schema URI=PATH`), or remove the attribute if it is a leftover")
        .with_fix(Fix {
            message: if rest.is_empty() {
                format!("remove `{}`", first.lexical_name)
            } else {
                format!("remove the {} attributes", attributes.len())
            },
            applicability: Applicability::MaybeIncorrect,
            edits: attributes
                .iter()
                .map(|attribute| FixEdit {
                    span: attribute_removal_span(document, attribute),
                    replacement: String::new(),
                })
                .collect(),
        })
}

fn unbound_element(document: &Document, parent: &Element, child: &Element) -> Diagnostic {
    Diagnostic::warning(
        "LINT022",
        format!(
            "element `<{}>` in an unbound namespace has no effect",
            child.lexical_name
        ),
        child.start_tag_span,
    )
    .with_category(DiagnosticCategory::Lint)
    .with_primary_message(format!(
        "namespace `{}`",
        child.name.namespace_uri.as_deref().unwrap_or_default()
    ))
    .with_secondary(Label::new(parent.start_tag_span).with_message(format!(
        "not executable content: `<{}>` runs no custom action",
        parent.name.local_name
    )))
    .with_note(NOTE)
    .with_help("bind the namespace's package (`--extension-package MANIFEST.json`) or register its schema (`--schema URI=PATH`), or remove the element if it is a leftover")
    .with_fix(Fix::replace(
        format!("remove `<{}>`", child.lexical_name),
        Applicability::MaybeIncorrect,
        crate::graph::removal_span(document, child.span),
        "",
    ))
}
