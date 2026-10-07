//! The draft profile ([`crate::Profile::Draft`]): what a document a machine
//! writes and then runs as an invoked child (a builder bot's draft) may use.
//!
//! A draft is untrusted text that the writing machine executes, so the
//! profile admits a document that is self-contained, loads nothing, starts
//! nothing, and talks only through the SCXML Event I/O Processor:
//!
//! * `SCXML097`: a namespace other than the SCXML namespace, the `event`
//!   attribute vocabulary ([`scxml_types::EVENT_NAMESPACE_URI`]), and the
//!   XML namespace is declared or used (an extension namespace, or the
//!   host's `env` vocabulary, would reach packages the writer binds).
//! * `SCXML098`: an `<invoke>` or `<script>` element (SCXML 6.4, 5.8):
//!   a draft starts no session or service and runs no script.
//! * `SCXML099`: a `src` or `srcexpr` attribute (`<data>`, `<script>`,
//!   `<invoke>`; SCXML 5.3, 5.8, 6.4): a draft loads nothing.
//! * `SCXML100`: a `typeexpr` attribute, or a `<send type>` other than the
//!   SCXML Event I/O Processor (absent, or exactly
//!   `http://www.w3.org/TR/scxml/#SCXMLEventProcessor`; SCXML 6.2, C.1).
//! * `SCXML101`: a document type declaration (XML 1.0 §2.8): a draft
//!   declares no entity or attribute default.
//!
//! Every check reads the parsed XML tree (names are compared as expanded
//! names, never by prefix); the DTD check reads the prolog before the root
//! element. Nothing is judged without the profile.

use scxml_syntax::{Document, Element, Span};
use scxml_types::EVENT_NAMESPACE_URI;

use crate::{Diagnostic, DiagnosticCategory, SCXML_NAMESPACE};

/// The XML namespace, built into XML (`xml:lang`, `xml:space`).
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
/// The SCXML Event I/O Processor's type URI (SCXML C.1).
const SCXML_EVENT_PROCESSOR: &str = "http://www.w3.org/TR/scxml/#SCXMLEventProcessor";

/// Whether the draft profile admits `namespace_uri`.
pub(crate) fn admits_namespace(namespace_uri: &str) -> bool {
    namespace_uri == SCXML_NAMESPACE
        || namespace_uri == EVENT_NAMESPACE_URI
        || namespace_uri == XML_NAMESPACE
}

fn profile_error(code: &str, message: impl Into<String>, span: Span) -> Diagnostic {
    Diagnostic::error(code, message, span)
        .with_category(DiagnosticCategory::Semantic)
        .with_note("the draft profile admits a self-contained document that loads nothing, starts nothing, and sends only through the SCXML Event I/O Processor, because the machine that wrote it runs it")
}

/// Every draft-profile finding of `document`, in document order.
#[must_use]
pub(crate) fn draft_profile_diagnostics(document: &Document) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if let Some(span) = doctype_span(document) {
        diagnostics.push(
            profile_error(
                "SCXML101",
                "a draft has a document type declaration",
                span,
            )
            .with_primary_message("drafts declare no DTD")
            .with_help("remove the `<!DOCTYPE …>` declaration; write characters as numeric references such as `&#160;`")
            .with_spec_reference("XML 1.0 (Fifth Edition) §2.8 Prolog and Document Type Declaration"),
        );
    }
    document
        .root
        .visit(&mut |element| check_element(element, &mut diagnostics));
    diagnostics
}

fn check_element(element: &Element, diagnostics: &mut Vec<Diagnostic>) {
    for declaration in &element.namespaces {
        if !declaration.uri.is_empty() && !admits_namespace(&declaration.uri) {
            diagnostics.push(
                profile_error(
                    "SCXML097",
                    "a draft declares a namespace the draft profile does not admit",
                    declaration.value_span,
                )
                .with_primary_message("only the SCXML namespace and the event namespace are admitted")
                .with_help(format!(
                    "remove the declaration and every element or attribute of that namespace; a draft uses core SCXML and `event:schema` (`{EVENT_NAMESPACE_URI}`) only"
                ))
                .with_spec_reference("SCXML 1.0 §3.2 <scxml>"),
            );
        }
    }
    let scxml = |name: &str| element.name.is(Some(SCXML_NAMESPACE), name);
    if let Some(uri) = element.name.namespace_uri.as_deref()
        && !admits_namespace(uri)
        && !declared_here(element, uri)
    {
        diagnostics.push(
            profile_error(
                "SCXML097",
                "a draft uses an element of a namespace the draft profile does not admit",
                element.start_tag_span,
            )
            .with_primary_message("not an SCXML element")
            .with_help("remove the element; a draft uses core SCXML elements only")
            .with_spec_reference("SCXML 1.0 §3.2 <scxml>"),
        );
    }
    if scxml("invoke") || scxml("script") {
        let (what, reference) = if scxml("invoke") {
            ("an `<invoke>`", "SCXML 1.0 §6.4 <invoke>")
        } else {
            ("a `<script>`", "SCXML 1.0 §5.8 <script>")
        };
        diagnostics.push(
            profile_error(
                "SCXML098",
                format!("a draft holds {what} element"),
                element.start_tag_span,
            )
            .with_primary_message("drafts start no session or service and run no script")
            .with_help("express the behaviour with states, transitions, `<assign>`, and `<send>` to `#_parent`")
            .with_spec_reference(reference),
        );
    }
    check_attributes(element, diagnostics);
}

/// The attribute findings of one element.
fn check_attributes(element: &Element, diagnostics: &mut Vec<Diagnostic>) {
    let scxml = |name: &str| element.name.is(Some(SCXML_NAMESPACE), name);
    for attribute in &element.attributes {
        if let Some(uri) = attribute.name.namespace_uri.as_deref() {
            if !admits_namespace(uri) && !declared_here(element, uri) {
                diagnostics.push(
                    profile_error(
                        "SCXML097",
                        "a draft uses an attribute of a namespace the draft profile does not admit",
                        attribute.span,
                    )
                    .with_primary_message("only `event:schema` attributes are admitted")
                    .with_help("remove the attribute")
                    .with_spec_reference("SCXML 1.0 §3.2 <scxml>"),
                );
            }
            continue;
        }
        if element.name.namespace_uri.as_deref() != Some(SCXML_NAMESPACE) {
            continue;
        }
        match attribute.name.local_name.as_str() {
            "src" | "srcexpr" => diagnostics.push(
                profile_error(
                    "SCXML099",
                    format!(
                        "a draft has a `{}` attribute",
                        attribute.name.local_name
                    ),
                    attribute.span,
                )
                .with_primary_message("drafts load nothing")
                .with_help("give the value inline (`expr` or child content) instead of loading it")
                .with_spec_reference("SCXML 1.0 §5.3 <data>"),
            ),
            "typeexpr" => diagnostics.push(
                profile_error(
                    "SCXML100",
                    "a draft names a type by expression",
                    attribute.span,
                )
                .with_primary_message("drafts send only through the SCXML Event I/O Processor")
                .with_help("remove `typeexpr`; a `<send>` without `type` uses the SCXML Event I/O Processor")
                .with_spec_reference("SCXML 1.0 §6.2 <send>"),
            ),
            "type" if scxml("send") && attribute.value != SCXML_EVENT_PROCESSOR => diagnostics
                .push(
                    profile_error(
                        "SCXML100",
                        "a draft sends through an Event I/O Processor other than SCXML's",
                        attribute.value_span,
                    )
                    .with_primary_message("drafts send only through the SCXML Event I/O Processor")
                    .with_help(format!(
                        "remove `type`, or write exactly `{SCXML_EVENT_PROCESSOR}`"
                    ))
                    .with_spec_reference("SCXML 1.0 §6.2 <send>"),
                ),
            _ => {}
        }
    }
}

/// Whether `element` itself declares `uri`, which [`check_element`] already
/// reported at the declaration (one mistake, one finding).
fn declared_here(element: &Element, uri: &str) -> bool {
    element
        .namespaces
        .iter()
        .any(|declaration| declaration.uri == uri)
}

/// The span of `<!DOCTYPE` in the prolog (the text before the root start
/// tag), outside comments and processing instructions.
fn doctype_span(document: &Document) -> Option<Span> {
    let text = document.source.text();
    let prolog = text.get(..document.root.start_tag_span.start)?;
    let mut position = 0;
    while let Some(offset) = prolog.get(position..)?.find('<') {
        let at = position.checked_add(offset)?;
        let rest = prolog.get(at..)?;
        if rest.starts_with("<!--") {
            position = at.checked_add(rest.find("-->")?.checked_add(3)?)?;
        } else if rest.starts_with("<?") {
            position = at.checked_add(rest.find("?>")?.checked_add(2)?)?;
        } else if rest.starts_with("<!DOCTYPE") {
            return Some(Span::new(
                document.root.start_tag_span.source,
                at,
                at.checked_add("<!DOCTYPE".len())?,
            ));
        } else {
            position = at.checked_add(1)?;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::draft_profile_diagnostics;

    fn codes(source: &str) -> Vec<String> {
        let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
            scxml_syntax::SourceId::new(0),
            "draft.scx",
            source,
        ))
        .expect("well formed");
        draft_profile_diagnostics(&document)
            .into_iter()
            .map(|diagnostic| diagnostic.code)
            .collect()
    }

    fn machine(body: &str) -> String {
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event" version="1.0" datamodel="ecmascript"><state id="s">{body}</state></scxml>"#
        )
    }

    #[test]
    fn a_self_contained_draft_has_no_finding() {
        let draft = machine(
            r##"<onentry><send event="ready" target="#_parent"/><send event="t" delay="1s" type="http://www.w3.org/TR/scxml/#SCXMLEventProcessor"/></onentry><transition event="go" event:schema='{"type":"object"}' type="internal" target="s"/><history id="h" type="deep"><transition target="s"/></history>"##,
        );
        assert!(codes(&draft).is_empty(), "{:?}", codes(&draft));
    }

    #[test]
    fn foreign_namespaces_are_scxml097_once_per_mistake() {
        let draft = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:x="urn:x" version="1.0"><state id="s"><onentry><x:do/></onentry></state></scxml>"#;
        assert_eq!(codes(draft), ["SCXML097", "SCXML097"]);
        let local = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><state id="s"><onentry><do xmlns="urn:x"/></onentry></state></scxml>"#;
        assert_eq!(codes(local), ["SCXML097"]);
        let env = machine("").replace(
            "version=",
            r#"xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env" version="#,
        );
        assert_eq!(codes(&env), ["SCXML097"]);
        let unused = machine("").replace("version=", r#"xmlns:y="urn:y" version="#);
        assert_eq!(codes(&unused), ["SCXML097"]);
    }

    #[test]
    fn invoke_script_and_loads_are_reported() {
        assert_eq!(codes(&machine(r#"<invoke type="scxml"/>"#)), ["SCXML098"]);
        assert_eq!(
            codes(&machine(r#"<invoke src="child.scx"/>"#)),
            ["SCXML098", "SCXML099"]
        );
        let script = machine("").replace(
            r#"<state id="s">"#,
            r#"<script src="lib.js"/><state id="s">"#,
        );
        assert_eq!(codes(&script), ["SCXML098", "SCXML099"]);
        let data = machine("").replace(
            r#"<state id="s">"#,
            r#"<datamodel><data id="d" srcexpr="'x.json'"/></datamodel><state id="s">"#,
        );
        assert_eq!(codes(&data), ["SCXML099"]);
    }

    #[test]
    fn other_processors_and_typeexpr_are_scxml100() {
        assert_eq!(
            codes(&machine(
                r#"<onentry><send event="e" type="http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor"/></onentry>"#
            )),
            ["SCXML100"]
        );
        assert_eq!(
            codes(&machine(
                r#"<onentry><send event="e" typeexpr="'x'"/></onentry>"#
            )),
            ["SCXML100"]
        );
    }

    #[test]
    fn a_doctype_is_scxml101_and_comments_are_skipped() {
        let draft = format!("<!-- <!DOCTYPE x> -->\n<!DOCTYPE scxml>\n{}", machine(""));
        assert_eq!(codes(&draft), ["SCXML101"]);
        let comment = format!("<!-- <!DOCTYPE x> -->\n{}", machine(""));
        assert!(codes(&comment).is_empty());
    }
}
