//! The values the bundled SCXML schema assigns to white-space-collapsing
//! attributes.
//!
//! XML Schema 1.1 Part 2 §4.3.6 (`whiteSpace`): for `collapse`, "All
//! occurrences of #x9 (tab), #xA (line feed) and #xD (carriage return) are
//! replaced with #x20 (space)", then "contiguous sequences of #x20's are
//! collapsed to a single #x20, and initial and/or final #x20's are deleted".
//! `xsd:token` and every type derived from it (`NMTOKEN`, `NMTOKENS`, `ID`,
//! `IDREF`, `IDREFS`), `xsd:anyURI`, and `xsd:decimal` fix the facet to
//! `collapse`; `xsd:string` keeps `preserve`.
//!
//! In the W3C SCXML 1.0 schema those are the attributes in
//! [`COLLAPSED_ATTRIBUTES`]: `xsd:ID` (`id`), `xsd:IDREF` (`<cancel
//! sendid>`), `xsd:IDREFS` (`initial`, `<transition target>`),
//! `xsd:NMTOKEN` (`<scxml name>`, `<scxml datamodel>`, `<raise event>`,
//! `<param name>`, `<assign attr>`), `xsd:decimal` (`<scxml version>`), and
//! the schema's own types restricted from `xsd:token`, `NMTOKEN`,
//! `NMTOKENS`, or `xsd:anyURI` (`EventType.datatype`,
//! `EventTypes.datatype`, `Binding.datatype`, `Exmode.datatype`,
//! `TransitionType.datatype`, `AssignType.datatype`, `Boolean.datatype`,
//! `URI.datatype`). A padded value is valid and means its collapsed form,
//! so the validator's rules read the collapsed form, exactly as lowering
//! does (`scxml-compiler-core`): `<state id=" s ">` declares `s`, and
//! duplicate detection, target resolution, `In()`, bounds, and the engine
//! all see `s`. `<history type>`, `<send type>`, `<invoke type>`, `delay`,
//! `namelist`, `<foreach item>`, `label`, and every expression are
//! `xsd:string` (or restricted from it) and keep their exact value.

use std::borrow::Cow;

use scxml_syntax::{Attribute, Document, Element, Node, Span, collapse_whitespace, is_xml_space};

use crate::semantic::SCXML_NAMESPACE;

/// Every unqualified attribute of an SCXML element whose schema type
/// collapses white space, by element local name.
pub const COLLAPSED_ATTRIBUTES: &[(&str, &[&str])] = &[
    (
        "scxml",
        &[
            "initial",
            "name",
            "version",
            "datamodel",
            "binding",
            "exmode",
        ],
    ),
    ("state", &["id", "initial"]),
    ("parallel", &["id"]),
    ("final", &["id"]),
    ("history", &["id"]),
    ("transition", &["event", "target", "type"]),
    ("raise", &["event"]),
    ("data", &["id", "src"]),
    ("assign", &["type", "attr"]),
    ("param", &["name"]),
    ("script", &["src"]),
    ("send", &["event", "target", "id"]),
    ("cancel", &["sendid"]),
    ("invoke", &["src", "id", "autoforward"]),
];

/// Whether the schema type of `attribute` on the SCXML element `element`
/// collapses white space ([`COLLAPSED_ATTRIBUTES`]).
#[must_use]
pub fn collapses_whitespace(element: &str, attribute: &str) -> bool {
    COLLAPSED_ATTRIBUTES
        .iter()
        .any(|(name, attributes)| *name == element && attributes.contains(&attribute))
}

/// `document` with every [`COLLAPSED_ATTRIBUTES`] value replaced by its
/// collapsed form, borrowed when every such value is already collapsed.
///
/// A replaced value's `value_span` narrows to the source bytes from its
/// first to its last non-white-space character when the value has exact
/// source bytes (no character or entity references), so diagnostics and
/// repairs point at the value the schema reads.
pub(crate) fn schema_values(document: &Document) -> Cow<'_, Document> {
    let mut padded = false;
    document.root.visit(&mut |element| {
        padded |= collapsible(element)
            .any(|attribute| matches!(collapse_whitespace(&attribute.value), Cow::Owned(_)));
    });
    if !padded {
        return Cow::Borrowed(document);
    }
    let mut normalized = document.clone();
    let mut pending = vec![&mut normalized.root];
    while let Some(element) = pending.pop() {
        if is_scxml(element) {
            let local_name = element.name.local_name.clone();
            for attribute in &mut element.attributes {
                if attribute.name.namespace_uri.is_none()
                    && collapses_whitespace(&local_name, &attribute.name.local_name)
                {
                    collapse_attribute(document, attribute);
                }
            }
        }
        for child in &mut element.children {
            if let Node::Element(child) = child {
                pending.push(child);
            }
        }
    }
    Cow::Owned(normalized)
}

fn is_scxml(element: &Element) -> bool {
    element.name.namespace_uri.as_deref() == Some(SCXML_NAMESPACE)
}

fn collapsible(element: &Element) -> impl Iterator<Item = &Attribute> {
    let scxml = is_scxml(element);
    element.attributes.iter().filter(move |attribute| {
        scxml
            && attribute.name.namespace_uri.is_none()
            && collapses_whitespace(&element.name.local_name, &attribute.name.local_name)
    })
}

fn collapse_attribute(document: &Document, attribute: &mut Attribute) {
    let Cow::Owned(collapsed) = collapse_whitespace(&attribute.value) else {
        return;
    };
    if document.source.slice(attribute.value_span) == Some(attribute.value.as_str()) {
        attribute.value_span = trimmed_span(&attribute.value, attribute.value_span);
    }
    attribute.value = collapsed;
}

/// The part of `span` (whose source bytes are `lexical`) from its first to
/// its last non-white-space character; all of `span` when `lexical` is only
/// white space, so a diagnostic about the empty value still marks it.
fn trimmed_span(lexical: &str, span: Span) -> Span {
    let leading = lexical.len() - lexical.trim_start_matches(is_xml_space).len();
    if leading == lexical.len() {
        return span;
    }
    let trailing = lexical.len() - lexical.trim_end_matches(is_xml_space).len();
    Span::new(span.source, span.start + leading, span.end - trailing)
}
