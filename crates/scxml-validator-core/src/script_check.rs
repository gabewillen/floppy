//! External script source checks (SCXML 1.0 §5.8 `<script>`).
//!
//! # Contract
//!
//! * `SCXML050` (error): a `<script>` has both a `src` attribute and inline
//!   content (non-whitespace text or child elements). SCXML 1.0 §5.8 allows
//!   exactly one of the two.
//! * `SCXML051` (error): a `src` value cannot name a script at all: it is
//!   empty or whitespace-only, contains control characters, or exceeds
//!   [`scxml_script::MAX_SCRIPT_URI_BYTES`]. Per §5.8 a `src` that cannot be
//!   fetched at document load rejects the document.
//! * Whether a well-formed `src` actually resolves is decided by the
//!   compiler (compile-time script packages, `COMP003`) or by the host's
//!   runtime script loader at document load; the validator never fetches.
//! * Pure and bounded by the validated document; it runs inside the
//!   validator's boundary span and records no telemetry of its own.

use scxml_syntax::{Document, Element, Node};

use crate::graph::attribute_removal_span;
use crate::semantic::{Rule, is_scxml};
use crate::{Applicability, Diagnostic, Fix, Label, Report};

const SPEC_SCRIPT: &str = "SCXML 1.0 §5.8 <script>";

/// `SCXML050`/`SCXML051`: `<script>` `src` XOR content, and unusable `src`.
pub(crate) struct ScriptSourceRule;

impl Rule for ScriptSourceRule {
    fn code(&self) -> &'static str {
        "SCXML050"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        let mut found = false;
        document.root.visit(&mut |element| {
            if !is_scxml(element, "script") {
                return;
            }
            let Some(src) = element.attribute("src") else {
                return;
            };
            if let Some(content) = first_content(element) {
                found = true;
                report.push(
                    Diagnostic::error(
                        self.code(),
                        "`<script>` cannot have both a `src` attribute and inline content",
                        src.span,
                    )
                    .with_primary_message("external script source")
                    .with_secondary(Label::new(content).with_message("inline content here"))
                    .with_note("SCXML 1.0 §5.8: `<script>` takes either a `src` URI or inline script content, never both")
                    .with_help("keep the external `src` and delete the inline content, or delete `src` and keep the inline script")
                    .with_fix(Fix::replace(
                        "remove the `src` attribute and keep the inline script",
                        Applicability::MaybeIncorrect,
                        attribute_removal_span(document, src.span),
                        "",
                    ))
                    .with_spec_reference(SPEC_SCRIPT),
                );
            }
            if scxml_script::validate_uri(&src.value).is_err() {
                found = true;
                report.push(
                    Diagnostic::error(
                        "SCXML051",
                        "`<script src>` does not name a fetchable script",
                        src.value_span,
                    )
                    .with_primary_message("empty, oversized, or control-character URI")
                    .with_note("SCXML 1.0 §5.8: if the script specified by `src` cannot be downloaded at document load time, the document is rejected")
                    .with_help(format!(
                        "set `src` to the URI of a Wasm script component or a script in the data model's language (non-empty, without control characters, at most {} bytes), or remove `src` and inline the script",
                        scxml_script::MAX_SCRIPT_URI_BYTES
                    ))
                    .with_spec_reference(SPEC_SCRIPT),
                );
            }
        });
    }
}

/// Span of the first inline-content node: non-whitespace text or an element.
fn first_content(element: &Element) -> Option<scxml_syntax::Span> {
    element.children.iter().find_map(|node| match node {
        Node::Text(text) if !scxml_syntax::is_xml_blank(&text.value) => Some(text.span),
        Node::Element(child) => Some(child.span),
        Node::Text(_) => None,
    })
}

#[cfg(test)]
mod tests {
    use crate::semantic::validate_semantics;
    use crate::{Diagnostic, SCXML_NAMESPACE};

    /// The source id of the documents these tests parse.
    const SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(1);

    fn diagnostics(body: &str) -> Vec<Diagnostic> {
        let source = format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" datamodel="ecmascript" initial="s">{body}<state id="s"><transition event="go" target="f"/></state><final id="f"/></scxml>"#
        );
        let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
            SOURCE,
            "script.scxml",
            source,
        ))
        .expect("fixture parses");
        validate_semantics(&document)
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.code.starts_with("SCXML05"))
            .cloned()
            .collect()
    }

    #[test]
    fn src_and_inline_content_are_mutually_exclusive() {
        let found = diagnostics(r#"<script src="lib.wasm">x = 1;</script>"#);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, "SCXML050");
        assert!(!found[0].fixes.is_empty(), "a removal fix is offered");
        let with_element = diagnostics(r#"<script src="lib.wasm"><foo xmlns="urn:x"/></script>"#);
        assert_eq!(with_element.len(), 1, "{with_element:?}");
        assert!(diagnostics(r#"<script src="lib.wasm">   </script>"#).is_empty());
        assert!(diagnostics(r#"<script src="lib.wasm"/>"#).is_empty());
        assert!(diagnostics("<script>x = 1;</script>").is_empty());
    }

    #[test]
    fn unusable_src_values_are_rejected_on_the_value_span() {
        for src in ["", "   ", "a&#x7F;b", "a&#x85;b"] {
            let found = diagnostics(&format!(r#"<script src="{src}"/>"#));
            assert_eq!(found.len(), 1, "{src:?}: {found:?}");
            assert_eq!(found[0].code, "SCXML051");
        }
        // `src` is `xsd:anyURI`, which collapses white space (XML Schema
        // Part 2 §4.3.6): a tab or line feed is a space, as lowering reads it.
        assert!(diagnostics(r#"<script src="&#xA;lib.wasm&#x9;"/>"#).is_empty());
        // Opaque but well-formed URIs are left to the compiler or loader.
        assert!(diagnostics(r#"<script src="D:\foo"/>"#).is_empty());
    }
}
