//! Report-wide diagnostic precision pass.
//!
//! Individual rules and the XSD adapter produce findings; this pass enforces
//! two presentation invariants for every diagnostic before canonical output:
//!
//! 1. Element-level labels point at the element name, never at a whole
//!    (possibly multi-line) start tag. A label whose span equals an element's
//!    start-tag span is narrowed to that element's `QName` bytes, unless the
//!    rule built it with `Label::whole_element` (a self-closing element whose
//!    whole range is the finding).
//! 2. Every diagnostic whose code is documented in the canonical explain
//!    table carries a `note:` explaining why the rule exists and an
//!    actionable `help:` when the producing rule supplied none.
//!
//! The pass never changes codes, severities, messages, or fixes, and it is
//! deterministic: it depends only on the parsed document and the report.

use std::collections::BTreeMap;

use scxml_syntax::{Document, Element, Node, Span};

use crate::spans::element_name_span;
use crate::{Diagnostic, Report};
use scxml_reporting::explain;

/// Applies the precision invariants to every diagnostic in `report`.
pub(crate) fn refine_report(document: Option<&Document>, report: &mut Report) {
    let start_tags = document.map(start_tag_index).unwrap_or_default();
    report.map_diagnostics(|diagnostic| {
        if let Some(document) = document {
            for label in std::iter::once(&mut diagnostic.primary).chain(&mut diagnostic.secondary) {
                if !label.is_exact() {
                    narrow(document, &start_tags, &mut label.span);
                }
            }
        }
        document_from_table(diagnostic);
    });
}

fn start_tag_index(document: &Document) -> BTreeMap<(usize, usize), &Element> {
    let mut index = BTreeMap::new();
    let mut pending = vec![&document.root];
    while let Some(element) = pending.pop() {
        index.insert(
            (element.start_tag_span.start, element.start_tag_span.end),
            element,
        );
        for child in &element.children {
            if let Node::Element(child) = child {
                pending.push(child);
            }
        }
    }
    index
}

fn narrow(document: &Document, start_tags: &BTreeMap<(usize, usize), &Element>, span: &mut Span) {
    if span.source != document.source.id() {
        return;
    }
    if let Some(element) = start_tags.get(&(span.start, span.end)) {
        *span = element_name_span(document, element);
    }
}

fn document_from_table(diagnostic: &mut Diagnostic) {
    let Some(entry) = explain::lookup(&diagnostic.code) else {
        return;
    };
    if diagnostic.notes.is_empty() {
        diagnostic.notes.push(entry.why.to_owned());
    }
    if diagnostic.help.is_none() {
        diagnostic.help = Some(entry.help.to_owned());
    }
    if diagnostic.spec_references.is_empty() && !entry.spec.is_empty() {
        diagnostic.spec_references.push(entry.spec.to_owned());
    }
}
