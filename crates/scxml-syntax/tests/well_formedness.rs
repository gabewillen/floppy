//! Curated XML 1.0 well-formedness conformance set for the SCXML trust
//! boundary.
//!
//! Each not-well-formed case is a construct that AI authors and editors
//! actually produce (a Markdown code fence around the document, trailing
//! prose, a raw `<` in an attribute or script, `--` inside a comment, a late
//! XML declaration, duplicate attributes, two roots). Every one must be
//! rejected with a [`XmlErrorKind::Malformed`] error whose primary span is the
//! responsible bytes and whose [`XmlErrorDetail`] names the violated
//! constraint. The well-formed cases guard against false rejections of the
//! legal neighbours (a UTF-8 byte order mark, a declaration at offset 0,
//! `>` in an attribute, comments and processing instructions after the root).
//!
//! Spans always index the original source bytes, including a leading BOM.

use scxml_syntax::{
    Document, Node, SourceId, SourceText, XmlError, XmlErrorDetail, XmlErrorKind, parse_xml,
};

/// The source id of the documents these tests parse.
const SOURCE: SourceId = SourceId::new(1);

fn parse(xml: &str) -> Result<Document, XmlError> {
    parse_xml(SourceText::new(SOURCE, "wf.xml", xml))
}

fn reject(xml: &str) -> XmlError {
    match parse(xml) {
        Ok(_) => panic!("not-well-formed input was accepted: {xml:?}"),
        Err(error) => error,
    }
}

fn slice(xml: &str, error: &XmlError) -> String {
    xml.get(error.span.start..error.span.end)
        .unwrap_or_else(|| panic!("error span {:?} must slice {xml:?}", error.span))
        .to_owned()
}

/// (production or constraint, input, exact primary bytes)
const NOT_WELL_FORMED: &[(&str, &str, &str)] = &[
    (
        "§3.1 WFC: No < in Attribute Values",
        r#"<a cond="x < 3"/>"#,
        "<",
    ),
    (
        "§3.1 WFC: No < in Attribute Values (single quotes, second attribute)",
        "<a b='1' cond='x<3'/>",
        "<",
    ),
    (
        "§2.1 document ::= prolog element Misc* (Markdown fence before and after)",
        "```xml\n<a/>\n```\n",
        "```xml",
    ),
    (
        "§2.1 document (prose before the root)",
        "Here is the machine:\n<a/>",
        "Here is the machine:",
    ),
    (
        "§2.1 document (prose after the root)",
        "<a/>\nLet me know if you need changes.\n",
        "Let me know if you need changes.",
    ),
    (
        "§2.1 document (CDATA outside the root)",
        "<![CDATA[x]]><a/>",
        "<![CDATA[x]]>",
    ),
    (
        "§2.5 Comments must not contain `--`",
        "<a><!-- bad -- comment --></a>",
        "--",
    ),
    (
        "§2.5 Comments must not end with `--->`",
        "<a><!-- trailing ---></a>",
        "-",
    ),
    (
        "§2.8 XMLDecl only at the start of the document (after whitespace)",
        "  \n<?xml version=\"1.0\"?>\n<a/>",
        "<?xml version=\"1.0\"?>",
    ),
    (
        "§2.8 XMLDecl only at the start of the document (after the root)",
        "<a/><?xml version=\"1.0\"?>",
        "<?xml version=\"1.0\"?>",
    ),
    (
        "§3.1 WFC: Unique Att Spec",
        r#"<a x="1" x="2"/>"#,
        r#"x="2""#,
    ),
    ("§2.1 exactly one root element", "<a/>\n<b/>", "b"),
    (
        "§2.4 `<` in character data (comparison in script)",
        "<a>for (i = 0; i < 3; i++) {}</a>",
        "<",
    ),
    (
        "§2.4 `<` in character data (followed by a digit)",
        "<a>1 <2</a>",
        "<",
    ),
];

#[test]
fn curated_not_well_formed_documents_are_rejected_at_the_responsible_bytes() {
    for (constraint, xml, expected) in NOT_WELL_FORMED {
        let error = reject(xml);
        assert_eq!(error.kind, XmlErrorKind::Malformed, "{constraint}: {xml:?}");
        assert_eq!(
            slice(xml, &error),
            *expected,
            "{constraint}: {xml:?} -> {}",
            error.message
        );
        assert!(
            !error.message.contains("position"),
            "{constraint}: parser byte positions must not leak into messages: {}",
            error.message
        );
        assert_ne!(
            error.detail,
            XmlErrorDetail::Unclassified,
            "{constraint}: every curated case carries a classified detail"
        );
    }
}

#[test]
fn every_well_formedness_detail_carries_its_repair_spans() {
    let error = reject("```xml\n<a/>\n```\n");
    let XmlErrorDetail::TextOutsideRoot { removals } = &error.detail else {
        panic!("fence must be text outside the root: {:?}", error.detail);
    };
    let fixed = apply_removals("```xml\n<a/>\n```\n", removals);
    assert_eq!(fixed, "<a/>\n", "removing both fences leaves the document");
    parse(&fixed).expect("fence removal must produce a well-formed document");

    let xml = "<a/>\nLet me know.\n<!-- c -->\n";
    let error = reject(xml);
    let XmlErrorDetail::TextOutsideRoot { removals } = &error.detail else {
        panic!("epilog prose must be text outside the root");
    };
    parse(&apply_removals(xml, removals)).expect("epilog removal must be well formed");

    let xml = "  \n<?xml version=\"1.0\"?>\n<a/>";
    let error = reject(xml);
    let XmlErrorDetail::MisplacedXmlDeclaration {
        leading: Some(leading),
    } = error.detail
    else {
        panic!(
            "late declaration must name its leading bytes: {:?}",
            error.detail
        );
    };
    parse(&apply_removals(xml, &[leading])).expect("removing leading whitespace fixes it");

    let xml = r#"<a x="1" y="2" x="3"/>"#;
    let error = reject(xml);
    let XmlErrorDetail::DuplicateAttribute { first, removal } = error.detail else {
        panic!("duplicate attribute detail: {:?}", error.detail);
    };
    assert_eq!(&xml[first.start..first.end], "x");
    assert_eq!(&xml[removal.start..removal.end], r#" x="3""#);
    parse(&apply_removals(xml, &[removal])).expect("removing the duplicate fixes it");

    let xml = "<a><!-- bad -- comment ---></a>";
    let error = reject(xml);
    let XmlErrorDetail::DoubleHyphenInComment { content } = error.detail else {
        panic!("comment detail: {:?}", error.detail);
    };
    assert_eq!(&xml[content.start..content.end], " bad -- comment -");

    let xml = "<a/>\n<b/>";
    let error = reject(xml);
    let XmlErrorDetail::MultipleRootElements { first_root } = error.detail else {
        panic!("two roots detail: {:?}", error.detail);
    };
    assert_eq!(&xml[first_root.start..first_root.end], "a");

    let xml = r#"<a cond="x < 3 &amp;&amp; y <= 4"/>"#;
    let error = reject(xml);
    let XmlErrorDetail::LessThanInAttributeValue { occurrences } = &error.detail else {
        panic!("attribute `<` detail: {:?}", error.detail);
    };
    assert_eq!(occurrences.len(), 2, "every `<` in the value is located");
    assert!(
        occurrences
            .iter()
            .all(|span| &xml[span.start..span.end] == "<")
    );

    let xml = "<a>if (x < 3) {}</a>";
    let error = reject(xml);
    let XmlErrorDetail::LessThanInText { element } = &error.detail else {
        panic!("text `<` detail: {:?}", error.detail);
    };
    assert_eq!(element.as_deref(), Some("a"));
}

fn apply_removals(xml: &str, removals: &[scxml_syntax::Span]) -> String {
    let mut ordered = removals.to_vec();
    ordered.sort_by_key(|span| std::cmp::Reverse(span.start));
    let mut fixed = xml.to_owned();
    for span in ordered {
        fixed.replace_range(span.start..span.end, "");
    }
    fixed
}

#[test]
fn legal_neighbours_of_the_rejected_constructs_are_accepted() {
    for xml in [
        "<?xml version=\"1.0\"?>\n<a/>",
        "\u{FEFF}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<a/>",
        "\u{FEFF}<a/>",
        r#"<a cond="a > b"/>"#,
        "<!-- a - b --><a/>",
        "<a/>\n<!-- trailing comment -->\n<?pi data?>\n",
        "\n\n<a/>\n\n",
        "<a><![CDATA[if (x < 3 && y) {}]]></a>",
        "<a>x &lt; 3</a>",
    ] {
        parse(xml).unwrap_or_else(|error| panic!("{xml:?} must be accepted: {}", error.message));
    }
}

#[test]
fn byte_order_mark_keeps_every_span_on_the_original_bytes() {
    let xml = "\u{FEFF}<scxml initial=\"idle\">\n  <state id=\"idle\"><transition event=\"go\" target=\"done\"/></state>\n</scxml>";
    let document = parse(xml).expect("a UTF-8 BOM is legal (XML 1.0 §4.3.3)");
    let root = &document.root;
    assert_eq!(
        &xml[root.start_tag_span.start..root.start_tag_span.end],
        "<scxml initial=\"idle\">"
    );
    let initial = root.attribute("initial").expect("initial attribute");
    assert_eq!(
        &xml[initial.value_span.start..initial.value_span.end],
        "idle"
    );
    assert_eq!(
        &xml[initial.span.start..initial.span.end],
        "initial=\"idle\""
    );
    let Some(Node::Element(state)) = root
        .children
        .iter()
        .find(|node| matches!(node, Node::Element(_)))
    else {
        panic!("state child");
    };
    let id = state.attribute("id").expect("id");
    assert_eq!(&xml[id.value_span.start..id.value_span.end], "idle");
    let Some(Node::Element(transition)) = state.children.first() else {
        panic!("transition child");
    };
    for (name, value) in [("event", "go"), ("target", "done")] {
        let attribute = transition.attribute(name).expect("transition attribute");
        assert_eq!(
            &xml[attribute.value_span.start..attribute.value_span.end],
            value
        );
    }
    assert_eq!(
        &xml[root.span.start..root.span.end],
        xml.trim_start_matches('\u{FEFF}')
    );

    let xml = "\u{FEFF}<a v=\"x < 3\"/>";
    let error = reject(xml);
    assert_eq!(slice(xml, &error), "<", "error spans include the BOM too");
    let xml = "\u{FEFF}<a><b></a>";
    let error = reject(xml);
    assert!(slice(xml, &error).contains("</a>"), "{:?}", error.span);
}

#[test]
fn error_spans_never_split_characters_or_point_into_the_byte_order_mark() {
    for xml in [
        "\u{FEFF}",
        "\u{FEFF}hello",
        "\u{FEFF}<",
        "\u{FEFF}<a>",
        "é<",
        "<a>é",
        "\u{FEFF}\u{FEFF}<a/>",
    ] {
        let error = reject(xml);
        let bom = if xml.starts_with('\u{FEFF}') { 3 } else { 0 };
        assert!(
            error.span.start >= bom,
            "{xml:?}: {:?} points into the BOM",
            error.span
        );
        assert!(
            xml.is_char_boundary(error.span.start) && xml.is_char_boundary(error.span.end),
            "{xml:?}: {:?} splits a character",
            error.span
        );
    }
}
