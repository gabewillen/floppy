//! Precise sub-spans of parsed XML constructs for diagnostic labels and edits.
//!
//! The parser records complete start-tag, element, attribute, and value spans.
//! Diagnostics should label the most specific responsible bytes, so these
//! helpers derive element-name, end-tag-name, attribute-name, insertion, and
//! removal spans from the preserved source without re-parsing. Every helper is
//! total: when the source does not have the expected lexical shape (for
//! example a hand-built tree in a test), it falls back to the recorded span
//! instead of guessing.

use scxml_syntax::{Attribute, Document, Element, Span};

/// Span of the element's lexical `QName` inside its start tag.
///
/// XML forbids whitespace between `<` and the name, so the name occupies
/// exactly `lexical_name.len()` bytes after the opening `<`.
#[must_use]
pub fn element_name_span(document: &Document, element: &Element) -> Span {
    let start = element.start_tag_span.start.saturating_add(1);
    let end = start.saturating_add(element.lexical_name.len());
    let candidate = Span::new(element.start_tag_span.source, start, end);

    if document.source.slice(candidate) == Some(element.lexical_name.as_str()) {
        candidate
    } else {
        element.start_tag_span
    }
}

/// Span of the element's name inside its end tag, when it has one.
///
/// Returns `None` for self-closing elements.
#[must_use]
pub fn end_tag_name_span(document: &Document, element: &Element) -> Option<Span> {
    let text = document.source.slice(element.span)?;

    if text.ends_with("/>") && element.span.end == element.start_tag_span.end {
        None
    } else {
        let closing = text.rfind("</")?;
        let start = element.span.start.saturating_add(closing).saturating_add(2);
        let end = start.saturating_add(element.lexical_name.len());
        let candidate = Span::new(element.span.source, start, end);
        (document.source.slice(candidate) == Some(element.lexical_name.as_str()))
            .then_some(candidate)
    }
}

/// Span of the attribute's lexical `QName`.
#[must_use]
pub fn attribute_name_span(document: &Document, attribute: &Attribute) -> Span {
    let end = attribute
        .span
        .start
        .saturating_add(attribute.lexical_name.len());
    let candidate = Span::new(attribute.span.source, attribute.span.start, end);

    if document.source.slice(candidate) == Some(attribute.lexical_name.as_str()) {
        candidate
    } else {
        attribute.span
    }
}

/// Span removing the attribute together with the whitespace that separates
/// it from the previous token, so applying the edit leaves canonical spacing.
#[must_use]
pub fn attribute_removal_span(document: &Document, attribute: &Attribute) -> Span {
    let text = document.source.text();
    let mut start = attribute.span.start;
    while start > 0 {
        let Some(previous) = text
            .get(..start)
            .and_then(|prefix| prefix.chars().next_back())
        else {
            break;
        };
        if !matches!(previous, ' ' | '\t' | '\r' | '\n') {
            break;
        }
        start -= previous.len_utf8();
    }
    Span::new(attribute.span.source, start, attribute.span.end)
}

/// Empty span where a new attribute should be inserted: directly after the
/// last attribute or namespace declaration, or after the element name.
#[must_use]
pub fn attribute_insertion_point(document: &Document, element: &Element) -> Span {
    let name_end = element_name_span(document, element).end;
    let end = element
        .attributes
        .iter()
        .map(|attribute| attribute.span.end)
        .chain(
            element
                .namespaces
                .iter()
                .map(|declaration| declaration.span.end),
        )
        .filter(|end| *end <= element.start_tag_span.end)
        .max()
        .unwrap_or(name_end)
        .max(name_end);
    Span::new(element.start_tag_span.source, end, end)
}

/// Value span of the first namespace declaration binding `namespace_uri`.
#[must_use]
pub fn namespace_declaration_value(document: &Document, namespace_uri: &str) -> Option<Span> {
    let mut result = None;
    document.root.visit(&mut |element| {
        if result.is_none() {
            result = element
                .namespaces
                .iter()
                .find(|declaration| declaration.uri == namespace_uri)
                .map(|declaration| declaration.value_span);
        }
    });
    result
}

/// Span of the first element name or attribute name in `namespace_uri`.
#[must_use]
pub fn first_namespace_use(document: &Document, namespace_uri: &str) -> Option<Span> {
    let mut result = None;
    document.root.visit(&mut |element| {
        if result.is_none() && element.name.namespace_uri.as_deref() == Some(namespace_uri) {
            result = Some(element_name_span(document, element));
        }
        if result.is_none() {
            result = element
                .attributes
                .iter()
                .find(|attribute| attribute.name.namespace_uri.as_deref() == Some(namespace_uri))
                .map(|attribute| attribute_name_span(document, attribute));
        }
    });
    result
}

#[cfg(test)]
mod tests {
    use scxml_syntax::{SourceId, SourceText, parse_xml};

    use super::*;

    /// The source id of the documents these tests parse.
    const SOURCE: SourceId = SourceId::new(1);

    fn document(xml: &str) -> Document {
        parse_xml(SourceText::new(SOURCE, "spans.xml", xml)).expect("span fixture must parse")
    }

    #[test]
    fn derives_exact_name_insertion_and_removal_spans() {
        let document = document("<root a=\"1\"\n  b='2'><child/></root>");
        let root = &document.root;
        assert_eq!(
            document.source.slice(element_name_span(&document, root)),
            Some("root")
        );
        let end = end_tag_name_span(&document, root).expect("root has an end tag");
        assert_eq!(document.source.slice(end), Some("root"));
        assert_eq!(end.start, document.source.text().len() - 5);
        let b = root.attribute("b").expect("b attribute");
        assert_eq!(
            document.source.slice(attribute_name_span(&document, b)),
            Some("b")
        );
        assert_eq!(
            document.source.slice(attribute_removal_span(&document, b)),
            Some("\n  b='2'")
        );
        let insertion = attribute_insertion_point(&document, root);
        assert_eq!(insertion.start, b.span.end);
        let scxml_syntax::Node::Element(child) = &root.children[0] else {
            panic!("fixture child must be an element");
        };
        assert_eq!(end_tag_name_span(&document, child), None);
        assert_eq!(
            document
                .source
                .slice(attribute_insertion_point(&document, child)),
            Some("")
        );
        assert_eq!(
            attribute_insertion_point(&document, child).start,
            child.start_tag_span.start + 6
        );
    }
}
