//! Property tests at the XML trust boundary (`parse_xml`).
//!
//! Hostile input: arbitrary text and mutated well-formed documents must
//! either parse or fail with a typed error whose span lies inside the source,
//! never panic. Generated well-formed trees must parse to exactly the element
//! names, attribute values, and character data that were serialized, with
//! every retained span slicing the source. Runs are deterministic: a fixed
//! RNG seed, a fixed case count, and no failure-persistence files.

use std::fmt::Write as _;

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};
use scxml_syntax::{
    Document, Element, Node, SourceId, SourceText, XmlErrorKind, XmlLimits, parse_xml,
    parse_xml_with_limits,
};

/// The source id of the documents these tests parse.
const SOURCE: SourceId = SourceId::new(1);

fn config() -> Config {
    Config {
        cases: 256,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(0x0a11_ce55),
        ..Config::default()
    }
}

/// A generated well-formed element.
#[derive(Clone, Debug)]
struct Tree {
    name: String,
    attributes: Vec<String>,
    children: Vec<Child>,
}

#[derive(Clone, Debug)]
enum Child {
    Element(Tree),
    Text(String),
}

fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            other => escaped.push(other),
        }
    }
    escaped
}

fn serialize(tree: &Tree, output: &mut String) {
    output.push('<');
    output.push_str(&tree.name);
    for (index, value) in tree.attributes.iter().enumerate() {
        write!(output, " a{index}=\"{}\"", escape(value)).expect("writing to a String cannot fail");
    }
    if tree.children.is_empty() {
        output.push_str("/>");
        return;
    }
    output.push('>');
    for child in &tree.children {
        match child {
            Child::Element(element) => serialize(element, output),
            Child::Text(text) => output.push_str(&escape(text)),
        }
    }
    write!(output, "</{}>", tree.name).expect("writing to a String cannot fail");
}

/// Character data without whitespace (so no whitespace-only node can be
/// normalized away) and without characters outside XML 1.0 `Char`.
fn text() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9<>&\"'.;#é中😀]{1,8}"
}

fn tree() -> impl Strategy<Value = Tree> {
    let leaf = ("[a-z][a-z0-9]{0,6}", prop::collection::vec(text(), 0..3)).prop_map(
        |(name, attributes)| Tree {
            name,
            attributes,
            children: Vec::new(),
        },
    );
    leaf.prop_recursive(6, 48, 4, |inner| {
        (
            "[a-z][a-z0-9]{0,6}",
            prop::collection::vec(text(), 0..3),
            prop::collection::vec(
                prop_oneof![inner.prop_map(Child::Element), text().prop_map(Child::Text)],
                0..4,
            ),
        )
            .prop_map(|(name, attributes, children)| Tree {
                name,
                attributes,
                children,
            })
    })
}

fn source(text: &str) -> SourceText {
    SourceText::new(SOURCE, "property.xml", text)
}

/// Expected preorder rendering of names, attribute values, and text.
fn expected_events(tree: &Tree, events: &mut Vec<String>) {
    events.push(format!("<{}", tree.name));
    events.extend(tree.attributes.iter().map(|value| format!("@{value}")));
    let mut pending_text = String::new();
    for child in &tree.children {
        match child {
            Child::Text(text) => pending_text.push_str(text),
            Child::Element(element) => {
                if !pending_text.is_empty() {
                    events.push(format!("#{}", std::mem::take(&mut pending_text)));
                }
                expected_events(element, events);
            }
        }
    }
    if !pending_text.is_empty() {
        events.push(format!("#{pending_text}"));
    }
    events.push(">".into());
}

/// Actual preorder rendering; adjacent text nodes are coalesced so the
/// comparison does not depend on how the parser splits character data.
fn actual_events(element: &Element, document: &Document, events: &mut Vec<String>) {
    assert!(
        document.source.slice(element.span).is_some(),
        "element span slices the source"
    );
    assert!(
        document.source.slice(element.start_tag_span).is_some(),
        "start-tag span slices the source"
    );
    events.push(format!("<{}", element.name.local_name));
    events.extend(
        element
            .attributes
            .iter()
            .map(|attribute| format!("@{}", attribute.value)),
    );
    let mut pending_text = String::new();
    for child in &element.children {
        match child {
            Node::Text(text) => {
                assert!(
                    document.source.slice(text.span).is_some(),
                    "text span slices the source"
                );
                pending_text.push_str(&text.value);
            }
            Node::Element(child) => {
                if !pending_text.is_empty() {
                    events.push(format!("#{}", std::mem::take(&mut pending_text)));
                }
                actual_events(child, document, events);
            }
        }
    }
    if !pending_text.is_empty() {
        events.push(format!("#{pending_text}"));
    }
    events.push(">".into());
}

fn assert_error_span_in_bounds(text: &str, input: SourceText) {
    if let Err(error) = parse_xml(input) {
        assert!(error.span.start <= error.span.end, "error span is ordered");
        assert!(
            error.span.end <= text.len(),
            "error span lies inside the source"
        );
    }
}

proptest! {
    #![proptest_config(config())]

    /// Arbitrary text never panics the parser; failures carry an in-bounds span.
    #[test]
    fn parser_is_total_on_arbitrary_text(text in any::<String>()) {
        assert_error_span_in_bounds(&text, source(&text));
    }

    /// Markup-heavy noise exercises tag, reference, and namespace paths.
    #[test]
    fn parser_is_total_on_markup_noise(
        text in "[<>/=\"'&;#xa-c:!?\\[\\]CDATA -]{0,64}"
    ) {
        assert_error_span_in_bounds(&text, source(&text));
    }

    /// Well-formed generated trees parse to exactly what was serialized.
    #[test]
    fn well_formed_trees_round_trip(tree in tree()) {
        let mut text = String::new();
        serialize(&tree, &mut text);
        let document = parse_xml(source(&text));
        prop_assert!(document.is_ok(), "{text}: {document:?}");
        let Ok(document) = document else { return Ok(()) };
        let mut expected = Vec::new();
        expected_events(&tree, &mut expected);
        let mut actual = Vec::new();
        actual_events(&document.root, &document, &mut actual);
        prop_assert_eq!(actual, expected);
    }

    /// Deleting or duplicating one byte range of a well-formed document never
    /// panics the parser.
    #[test]
    fn mutated_documents_fail_closed(
        tree in tree(),
        start in any::<prop::sample::Index>(),
        length in 0_usize..8,
        duplicate in any::<bool>(),
    ) {
        let mut text = String::new();
        serialize(&tree, &mut text);
        let mut cut = start.index(text.len() + 1);
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        let mut end = (cut + length).min(text.len());
        while !text.is_char_boundary(end) {
            end += 1;
        }
        let mutated = if duplicate {
            format!("{}{}", &text[..end], &text[cut..])
        } else {
            format!("{}{}", &text[..cut], &text[end..])
        };
        assert_error_span_in_bounds(&mutated, source(&mutated));
    }

    /// Nesting deeper than the configured bound is a typed resource failure.
    #[test]
    fn depth_bound_is_enforced(max_depth in 1_usize..32, extra in 1_usize..8) {
        let depth = max_depth + extra;
        let text = format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth));
        let limits = XmlLimits { max_depth, ..XmlLimits::DEFAULT };
        let error = parse_xml_with_limits(source(&text), limits).err();
        prop_assert_eq!(error.map(|error| error.kind), Some(XmlErrorKind::ResourceLimit));
        let accepted = format!("{}{}", "<a>".repeat(max_depth), "</a>".repeat(max_depth));
        prop_assert!(parse_xml_with_limits(source(&accepted), limits).is_ok());
    }
}
