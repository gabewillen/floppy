//! Literal event data sent between a document and the literal
//! `<invoke src>` documents it invokes (`EVENT011`).
//!
//! The caller supplies the documents a validated document's literal
//! `<invoke src>` elements load ([`crate::Validator::with_invoked_documents`],
//! keyed by the `src` value as authored, XML white space collapsed), each
//! with the schema files its own `event:schema` attributes reference. For
//! every `<invoke>` of the validated document (nested `<content>` documents
//! excluded) with a literal `id`, a literal `src` the caller supplied, and
//! no `type` or an SCXML invoke type, a `<send event="x">` through the
//! SCXML Event I/O Processor (no `type`, or the SCXML processor's) with a
//! literal `target`:
//!
//! * from the validated document to `#_<id>` (an ECMAScript sender), whose
//!   data a transition of the invoked document handling `x` (SCXML 3.12.1
//!   descriptor matching) does not accept, or
//! * from the invoked document to `#_parent` (an ECMAScript sender), whose
//!   data a transition of the validated document handling `x` does not
//!   accept,
//!
//! is `EVENT011` (warning), decided exactly as `EVENT010` decides one
//! document's own sends: the literal data, or a witness of the sender's own
//! literal schema, fails the receiver's literal schema. Every label is in
//! the validated document: the finding names the invoked document by its
//! `src`. An invoked document that does not parse, and every schema that
//! does not resolve, is skipped silently (validating that document reports
//! it). Pure and bounded by the documents and their schemas; no telemetry.

use std::collections::BTreeMap;

use scxml_syntax::{Attribute, Document, Element, Node, SourceId, SourceText, XmlLimits};
use serde_json::Value;

use super::{Compiled, Handler, Producer, collect, descriptor_matches, quoted, rejected};
use crate::{Diagnostic, DiagnosticCategory, InvokedDocument, Label, SCXML_NAMESPACE};

/// The source id the invoked documents are parsed under; their spans are
/// never reported.
const INVOKED_SOURCE: SourceId = SourceId::new(2);

const SPEC_INVOKE: &str = "SCXML 1.0 §6.4 <invoke>";

/// One `<invoke>` of the validated document whose document was supplied.
struct Invoke<'a> {
    id: &'a str,
    src: &'a Attribute,
    key: String,
}

/// Every `EVENT011` finding of `document`, whose own schema files are
/// `resources`, against `invoked` (parsed within `limits`).
pub(crate) fn invoked_payload_diagnostics(
    document: &Document,
    resources: &crate::EventSchemaResources,
    invoked: &BTreeMap<String, InvokedDocument>,
    limits: XmlLimits,
) -> Vec<Diagnostic> {
    if invoked.is_empty() {
        return Vec::new();
    }
    let mut sites = Vec::new();
    invokes_of(&document.root, invoked, &mut sites);
    if sites.is_empty() {
        return Vec::new();
    }
    let parent_ecmascript = super::is_ecmascript(document);
    let mut parent_handlers = Vec::new();
    let mut parent_producers = Vec::new();
    collect(
        &document.root,
        &mut Vec::new(),
        resources,
        &mut parent_handlers,
        &mut parent_producers,
    );
    let mut parsed: BTreeMap<&str, Option<Document>> = BTreeMap::new();
    let mut compiled = Compiled::default();
    let mut diagnostics = Vec::new();
    for invoke in &sites {
        let Some(supplied) = invoked.get(&invoke.key) else {
            continue;
        };
        let child = parsed.entry(invoke.key.as_str()).or_insert_with(|| {
            scxml_syntax::parse_xml_with_limits(
                SourceText::new(INVOKED_SOURCE, invoke.key.clone(), supplied.text.clone()),
                limits,
            )
            .ok()
        });
        let Some(child) = child.as_ref() else {
            continue;
        };
        let mut child_handlers = Vec::new();
        let mut child_producers = Vec::new();
        collect(
            &child.root,
            &mut Vec::new(),
            &supplied.event_schemas,
            &mut child_handlers,
            &mut child_producers,
        );
        if parent_ecmascript {
            let target = format!("#_{}", invoke.id);
            for (producer, event) in sends_to(&parent_producers, &target) {
                for handler in handling(&child_handlers, event) {
                    if let Some((witness, span, literal)) =
                        rejected(&mut compiled, producer, handler)
                    {
                        diagnostics.push(to_child(invoke, event, span, &witness, literal));
                    }
                }
            }
        }
        if super::is_ecmascript(child) {
            for (producer, event) in sends_to(&child_producers, "#_parent") {
                for handler in handling(&parent_handlers, event) {
                    if let Some((witness, _, literal)) = rejected(&mut compiled, producer, handler)
                    {
                        diagnostics.push(to_parent(invoke, event, handler, &witness, literal));
                    }
                }
            }
        }
    }
    diagnostics
}

/// The `<invoke>`s of one machine (never descending into an SCXML
/// `<content>`, another document) that start a supplied document.
fn invokes_of<'a>(
    element: &'a Element,
    invoked: &BTreeMap<String, InvokedDocument>,
    out: &mut Vec<Invoke<'a>>,
) {
    if element.name.namespace_uri.as_deref() == Some(SCXML_NAMESPACE) {
        match element.name.local_name.as_str() {
            "content" => return,
            "invoke" => {
                let scxml_type = element
                    .attribute("type")
                    .is_none_or(|kind| scxml_types::is_scxml_invoke_type(&kind.value));
                if let (Some(id), Some(src), true, None) = (
                    element.attribute("id"),
                    element.attribute("src"),
                    scxml_type,
                    element.attribute("typeexpr"),
                ) {
                    let key = scxml_syntax::collapse_whitespace(&src.value).into_owned();
                    if invoked.contains_key(&key) {
                        out.push(Invoke {
                            id: id.value.as_str(),
                            src,
                            key,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    for child in &element.children {
        if let Node::Element(child) = child {
            invokes_of(child, invoked, out);
        }
    }
}

/// The `<send>`s among `producers` with a literal `event` that the SCXML
/// Event I/O Processor delivers to the literal `target`.
fn sends_to<'p, 'a>(
    producers: &'p [Producer<'a>],
    target: &'p str,
) -> impl Iterator<Item = (&'p Producer<'a>, &'a str)> + 'p {
    producers.iter().filter_map(move |producer| {
        let send = producer.element;
        let kind = send.attribute("type").map(|kind| kind.value.as_str());
        (send.name.local_name == "send"
            && send.attribute("targetexpr").is_none()
            && send.attribute("typeexpr").is_none()
            && send
                .attribute("target")
                .is_some_and(|attribute| attribute.value == target)
            && matches!(
                kind,
                None | Some("scxml" | "http://www.w3.org/TR/scxml/#SCXMLEventProcessor")
            ))
        .then(|| send.attribute("event"))
        .flatten()
        .map(|event| (producer, event.value.as_str()))
    })
}

/// The handlers among `handlers` whose event descriptors match `event`.
fn handling<'h, 'a>(
    handlers: &'h [Handler<'a>],
    event: &'h str,
) -> impl Iterator<Item = &'h Handler<'a>> + 'h {
    handlers.iter().filter(move |handler| {
        handler
            .descriptors
            .iter()
            .any(|descriptor| descriptor_matches(descriptor, event))
    })
}

fn what(literal: bool) -> &'static str {
    if literal {
        "its literal data"
    } else {
        "an instance of its `event:schema`"
    }
}

fn to_child(
    invoke: &Invoke<'_>,
    event: &str,
    span: scxml_syntax::Span,
    witness: &Value,
    literal: bool,
) -> Diagnostic {
    Diagnostic::warning(
        "EVENT011",
        format!(
            "`<send>` sends `{event}` to the session of `{}` with data a transition there handling it does not accept",
            invoke.key
        ),
        span,
    )
    .with_category(DiagnosticCategory::Semantic)
    .with_primary_message(format!(
        "{}, `{}`, fails the schema of a transition of `{}` handling `{event}`",
        what(literal),
        quoted(witness),
        invoke.key
    ))
    .with_secondary(
        Label::new(invoke.src.value_span)
            .with_message(format!("the invoked document, invoked as `#_{}`", invoke.id)),
    )
    .with_note("a transition's `event:schema` describes the `_event.data` it handles; a transition of the invoked document matches this event but its schema rejects this data (at run time the receiving session refuses it: the `<send>` is not routed and raises `error.execution`)")
    .with_help("make the two documents' schemas agree, or change the data")
    .with_spec_reference(SPEC_INVOKE)
}

fn to_parent(
    invoke: &Invoke<'_>,
    event: &str,
    handler: &Handler<'_>,
    witness: &Value,
    literal: bool,
) -> Diagnostic {
    Diagnostic::warning(
        "EVENT011",
        format!(
            "`{}` sends `{event}` to `#_parent` with data a transition here handling it does not accept",
            invoke.key
        ),
        invoke.src.value_span,
    )
    .with_category(DiagnosticCategory::Semantic)
    .with_primary_message(format!(
        "its `<send event=\"{event}\">`: {}, `{}`, ...",
        what(literal),
        quoted(witness)
    ))
    .with_secondary(
        Label::new(handler.schema.attribute.span)
            .with_message("... which fails the schema of this transition"),
    )
    .with_note("a transition's `event:schema` describes the `_event.data` it handles; the invoked document's `<send target=\"#_parent\">` creates data this transition's event descriptor matches but its schema rejects (at run time the receiving session refuses it: the `<send>` is not routed and raises `error.execution`)")
    .with_help("make the two documents' schemas agree, or change the data the invoked document sends")
    .with_spec_reference(SPEC_INVOKE)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use scxml_syntax::{SourceId, SourceText, XmlLimits, parse_xml};

    use super::invoked_payload_diagnostics;
    use crate::{EventSchemaResources, InvokedDocument, Severity};

    const EVENT: &str = "https://github.com/stateforward/scxml.rs/namespaces/event";
    const OBJECT_N_STRING: &str =
        r#"{"type":"object","properties":{"n":{"type":"string"}},"required":["n"]}"#;

    fn scxml(datamodel: &str, body: &str) -> String {
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="{EVENT}" version="1.0" datamodel="{datamodel}" initial="s"><state id="s">{body}</state></scxml>"#
        )
    }

    /// The `EVENT011` findings of a parent whose first state holds `body`,
    /// with `child.scx` (and its schema files) supplied, as (code,
    /// severity, primary span text).
    fn findings(
        body: &str,
        child: &str,
        schemas: &[(&str, &str)],
    ) -> Vec<(String, Severity, String)> {
        let source = scxml("ecmascript", body);
        let document = parse_xml(SourceText::new(
            SourceId::new(1),
            "parent.scx",
            source.clone(),
        ))
        .expect("the parent parses");
        let invoked = BTreeMap::from([(
            "child.scx".to_owned(),
            InvokedDocument {
                text: child.to_owned(),
                event_schemas: schemas
                    .iter()
                    .map(|(name, text)| ((*name).to_owned(), text.as_bytes().to_vec()))
                    .collect(),
            },
        )]);
        invoked_payload_diagnostics(
            &document,
            &EventSchemaResources::new(),
            &invoked,
            XmlLimits::DEFAULT,
        )
        .into_iter()
        .map(|diagnostic| {
            let span = diagnostic.primary.span;
            (
                diagnostic.code,
                diagnostic.severity,
                source
                    .get(span.start..span.end)
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect()
    }

    #[test]
    fn a_send_to_the_child_must_satisfy_the_child_transition_schema() {
        let child = scxml(
            "null",
            r#"<transition event="go" event:schema="schemas/go.json" target="s"/>"#,
        );
        let schemas = [("schemas/go.json", OBJECT_N_STRING)];
        let bad = r##"<invoke id="kid" src=" child.scx "/><onentry><send event="go" target="#_kid"><content>{"n": 1}</content></send></onentry>"##;
        let found = findings(bad, &child, &schemas);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, "EVENT011");
        assert_eq!(found[0].1, Severity::Warning);
        assert_eq!(found[0].2, "<content>{\"n\": 1}</content>");
        let good = bad.replace(r#"{"n": 1}"#, r#"{"n": "one"}"#);
        assert!(findings(&good, &child, &schemas).is_empty());
        // Without the child's schema file the reference does not resolve,
        // and nothing is judged.
        assert!(findings(bad, &child, &[]).is_empty());
    }

    #[test]
    fn a_send_schema_witness_is_judged_against_the_child() {
        let child = scxml(
            "null",
            &format!(r#"<transition event="go.*" event:schema='{OBJECT_N_STRING}' target="s"/>"#),
        );
        let bad = r##"<invoke id="kid" src="child.scx"/><onentry><send event="go.now" target="#_kid" event:schema='{"type":"object","properties":{"n":{"type":"integer"}},"required":["n"]}'/></onentry>"##;
        assert_eq!(findings(bad, &child, &[]).len(), 1);
    }

    #[test]
    fn a_child_send_to_its_parent_must_satisfy_the_parent_transition_schema() {
        let child = scxml(
            "ecmascript",
            r##"<onentry><send event="up" target="#_parent"><content>{"n": 1}</content></send></onentry>"##,
        );
        let body = format!(
            r#"<invoke id="kid" src="child.scx"/><transition event="up" event:schema='{OBJECT_N_STRING}' target="s"/>"#
        );
        let found = findings(&body, &child, &[]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].2, "child.scx", "labelled at the invoke's src");
        // A child in another data model has no literal JSON data.
        let null_child = child.replace("ecmascript", "null");
        assert!(findings(&body, &null_child, &[]).is_empty());
    }

    #[test]
    fn only_literal_scxml_invokes_and_their_own_targets_are_judged() {
        let child = scxml(
            "null",
            &format!(r#"<transition event="go" event:schema='{OBJECT_N_STRING}' target="s"/>"#),
        );
        let send = r##"<onentry><send event="go" target="#_kid"><content>{"n": 1}</content></send></onentry>"##;
        for invoke in [
            r#"<invoke idlocation="kid" src="child.scx"/>"#,
            r#"<invoke id="kid" srcexpr="'child.scx'"/>"#,
            r#"<invoke id="kid" src="other.scx"/>"#,
            r#"<invoke id="kid" type="https://example.com/service" src="child.scx"/>"#,
            r#"<invoke id="other" src="child.scx"/>"#,
            r#"<invoke id="kid" type="scxml" src="child.scx"><content/></invoke><invoke id="z" src="child.scx"><content><scxml xmlns="http://www.w3.org/2005/07/scxml"/></content></invoke>"#,
        ] {
            let expected = usize::from(invoke.contains(r#"id="kid" type="scxml""#));
            assert_eq!(
                findings(&format!("{invoke}{send}"), &child, &[]).len(),
                expected,
                "{invoke}"
            );
        }
        // A targetexpr, another target, or another I/O processor is not
        // judged.
        for other in [
            r#"<send event="go" targetexpr="'#_kid'"><content>{"n": 1}</content></send>"#,
            r##"<send event="go" target="#_parent"><content>{"n": 1}</content></send>"##,
            r##"<send event="go" target="#_kid" type="http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor"><content>{"n": 1}</content></send>"##,
        ] {
            let body = format!(r#"<invoke id="kid" src="child.scx"/><onentry>{other}</onentry>"#);
            assert!(findings(&body, &child, &[]).is_empty(), "{other}");
        }
    }

    #[test]
    fn an_invoked_document_that_does_not_parse_is_skipped() {
        let body = r##"<invoke id="kid" src="child.scx"/><onentry><send event="go" target="#_kid"><content>{"n": 1}</content></send></onentry>"##;
        assert!(findings(body, "<scxml", &[]).is_empty());
    }
}
