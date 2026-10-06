//! A document's `event:schema` cross-checked against the event contracts of
//! the invoked services it binds.
//!
//! A service package may declare the events its service emits, each with a
//! JSON Schema for the event's data ([`crate::PackageEvents`]). A transition
//! that can handle such an event while the service runs (one in the state
//! that invokes it, or in a descendant of that state) and names the event
//! exactly among its descriptors describes the same data, so:
//!
//! * `SCXML095` (error): the transition's `event:schema` is not the
//!   package's declared schema for that event: their canonical resolved
//!   JSON differs (the boolean schema `true` is compatible with any). A
//!   machine-applicable repair is not honest (the document may be the one
//!   that is right), so the fix that replaces it with the package's schema
//!   is `maybe-incorrect`.
//! * `SCXML096` (note): the transition has no `event:schema` at all; the fix
//!   inserts the package's schema (declaring the `event` namespace on the
//!   transition when no prefix for it is in scope).
//!
//! Compatibility is exact identity of canonical schemas, not JSON Schema
//! subsumption (which is undecidable in general). `event:schemaexpr`, a
//! descriptor that only prefixes the event name (`echo`, `echo.*`, `*`), an
//! `<invoke typeexpr>`, and a service whose package the validator was not
//! given are not judged; a schema that does not resolve is reported by the
//! `event` vocabulary's own rules (`SCXML087`-`SCXML092`) instead.
//!
//! The check never changes execution: a document's `event:schema` stays
//! metadata only; the session host enforces the package's contract itself.
//! Pure and bounded by the document, the given packages, and the supplied
//! schema bytes; it runs inside the validator's boundary span and records no
//! telemetry of its own.

use std::collections::BTreeMap;

use scxml_event_schema::{Resources, SCHEMA_ATTRIBUTE, SCHEMA_EXPR_ATTRIBUTE, Source};
use scxml_reporting::Applicability;
use scxml_syntax::{Attribute, Document, Element, Node, Span};
use scxml_types::EVENT_NAMESPACE_URI;

use crate::spans::attribute_insertion_point;
use crate::{BoundService, DeclaredEvent, Diagnostic, DiagnosticCategory, Fix, SCXML_NAMESPACE};

const SPEC_INVOKE: &str = "SCXML 1.0 §6.4 <invoke>";
/// The states whose transitions may handle an invoked service's events.
const STATE_ELEMENTS: [&str; 3] = ["scxml", "state", "parallel"];

/// One service whose emitted events are in scope: its type URI and
/// declared emitted events.
type InScope<'a> = (&'a str, &'a [DeclaredEvent]);
/// The namespace declarations on a path, innermost last.
type Prefixes = Vec<(Option<String>, String)>;
/// One state still to visit, with the services and prefixes in scope.
type Pending<'a> = (&'a Element, Vec<InScope<'a>>, Prefixes);

/// Every `SCXML095` and `SCXML096` finding of `document`, in document order.
#[must_use]
pub fn service_event_diagnostics(
    document: &Document,
    services: Option<&BTreeMap<String, BoundService>>,
    resources: &Resources,
) -> Vec<Diagnostic> {
    let Some(services) = services else {
        return Vec::new();
    };
    if services
        .values()
        .all(|service| service.events.emits.is_empty())
    {
        return Vec::new();
    }
    let mut diagnostics = Vec::new();
    // Depth-first over states, carrying the services in scope and the
    // namespace prefixes declared on the path (innermost last). Inline
    // `<invoke><content>` machines are not descended into: their
    // transitions do not run while this service does.
    let mut pending: Vec<Pending<'_>> = vec![(&document.root, Vec::new(), Vec::new())];
    while let Some((element, inherited, mut prefixes)) = pending.pop() {
        prefixes.extend(
            element
                .namespaces
                .iter()
                .map(|declaration| (declaration.prefix.clone(), declaration.uri.clone())),
        );
        if !is_state(element) {
            continue;
        }
        let mut scope = inherited;
        for invoke in children(element).filter(|child| is_scxml(child, "invoke")) {
            if let Some(type_uri) = invoke
                .attribute("type")
                .map(|attribute| attribute.value.as_str())
                && let Some(service) = services.get(type_uri)
                && !service.events.emits.is_empty()
            {
                scope.push((type_uri, &service.events.emits));
            }
        }
        if !scope.is_empty() {
            for transition in children(element).filter(|child| is_scxml(child, "transition")) {
                let mut transition_prefixes = prefixes.clone();
                transition_prefixes.extend(
                    transition
                        .namespaces
                        .iter()
                        .map(|declaration| (declaration.prefix.clone(), declaration.uri.clone())),
                );
                check_transition(
                    document,
                    transition,
                    &scope,
                    &transition_prefixes,
                    resources,
                    &mut diagnostics,
                );
            }
        }
        for child in children(element).collect::<Vec<_>>().into_iter().rev() {
            if is_state(child) {
                pending.push((child, scope.clone(), prefixes.clone()));
            }
        }
    }
    diagnostics
}

fn is_scxml(element: &Element, local_name: &str) -> bool {
    element.name.is(Some(SCXML_NAMESPACE), local_name)
}

fn is_state(element: &Element) -> bool {
    element.name.namespace_uri.as_deref() == Some(SCXML_NAMESPACE)
        && STATE_ELEMENTS.contains(&element.name.local_name.as_str())
}

fn children(element: &Element) -> impl Iterator<Item = &Element> {
    element.children.iter().filter_map(|child| match child {
        Node::Element(element) => Some(element),
        Node::Text(_) => None,
    })
}

fn check_transition(
    document: &Document,
    transition: &Element,
    scope: &[InScope<'_>],
    prefixes: &[(Option<String>, String)],
    resources: &Resources,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(events) = transition.attribute("event") else {
        return;
    };
    // Every declared event the transition names exactly, with its service.
    let mut handled: Vec<(&str, &DeclaredEvent)> = Vec::new();
    for descriptor in events.value.split(scxml_types::is_xml_space) {
        for (type_uri, emits) in scope {
            if let Some(declared) = emits.iter().find(|event| event.name == descriptor)
                && !handled
                    .iter()
                    .any(|(_, seen)| seen.name == declared.name && seen.sha256 == declared.sha256)
            {
                handled.push((type_uri, declared));
            }
        }
    }
    if handled.is_empty() {
        return;
    }
    let vocabulary = |name: &str| {
        transition.attributes.iter().find(|attribute| {
            attribute.name.namespace_uri.as_deref() == Some(EVENT_NAMESPACE_URI)
                && attribute.name.local_name == name
        })
    };
    if vocabulary(SCHEMA_EXPR_ATTRIBUTE).is_some() {
        return;
    }
    if let Some(schema) = vocabulary(SCHEMA_ATTRIBUTE) {
        let Some(resolved) = resolve(schema, resources) else {
            return;
        };
        if resolved == "true" {
            return;
        }
        for (type_uri, declared) in &handled {
            if declared.schema != resolved {
                diagnostics.push(mismatch(schema, type_uri, declared, handled.len() == 1));
            }
        }
        return;
    }
    // One schema can describe the data only when every handled event
    // declares the same one.
    let Some((type_uri, declared)) = handled.first() else {
        return;
    };
    if handled
        .iter()
        .any(|(_, other)| other.sha256 != declared.sha256)
    {
        return;
    }
    diagnostics.push(missing(
        document, transition, events, type_uri, declared, prefixes,
    ));
}

/// The canonical resolved text of an `event:schema`, when it resolves.
fn resolve(schema: &Attribute, resources: &Resources) -> Option<String> {
    let resolved = match scxml_event_schema::source(&schema.value) {
        Source::Inline(text) => scxml_event_schema::resolve(text.as_bytes(), None),
        Source::Reference { resource, fragment } => {
            scxml_event_schema::resolve(resources.get(resource)?, fragment)
        }
    };
    resolved.ok().map(|resolved| resolved.schema)
}

/// The schema as a single-quoted XML attribute value.
fn quoted(schema: &str) -> String {
    let mut out = String::with_capacity(schema.len().saturating_add(2));
    out.push('\'');
    for character in schema.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out.push('\'');
    out
}

fn short(sha256: &str) -> &str {
    sha256.get(..12).unwrap_or(sha256)
}

fn mismatch(
    schema: &Attribute,
    type_uri: &str,
    declared: &DeclaredEvent,
    fixable: bool,
) -> Diagnostic {
    let mut diagnostic = Diagnostic::error(
        "SCXML095",
        format!(
            "this `event:schema` is not the schema the service package declares for `{}`",
            declared.name
        ),
        schema.value_span,
    )
    .with_category(DiagnosticCategory::Package)
    .with_primary_message("differs from the service's declared event schema")
    .with_note(format!(
        "the service package of `{type_uri}` declares that it emits `{}` with the schema sha256:{} (`{}`); a transition that handles the event while the service runs describes the same data, so its schema must be that one (canonical JSON, compared exactly) or `true`",
        declared.name,
        short(&declared.sha256),
        declared.schema
    ))
    .with_help("use the package's schema for the event, or correct the service package if the document is right")
    .with_spec_reference(SPEC_INVOKE);
    if fixable {
        // Replaces the whole value, quotes included, so the schema's own
        // quotes stay well formed.
        diagnostic = diagnostic.with_fix(Fix::replace(
            format!(
                "use the schema the package declares for `{}`",
                declared.name
            ),
            Applicability::MaybeIncorrect,
            value_with_quotes(schema),
            quoted(&declared.schema),
        ));
    }
    diagnostic
}

/// The attribute value's span widened to its quotes.
fn value_with_quotes(attribute: &Attribute) -> Span {
    Span::new(
        attribute.value_span.source,
        attribute.value_span.start.saturating_sub(1),
        attribute.value_span.end.saturating_add(1),
    )
}

fn missing(
    document: &Document,
    transition: &Element,
    events: &Attribute,
    type_uri: &str,
    declared: &DeclaredEvent,
    prefixes: &[(Option<String>, String)],
) -> Diagnostic {
    // The innermost declaration of each prefix wins.
    let bound = |prefix: &str| {
        prefixes
            .iter()
            .rev()
            .find(|(declared, _)| declared.as_deref() == Some(prefix))
            .map(|(_, uri)| uri.as_str())
    };
    let in_scope = prefixes
        .iter()
        .rev()
        .filter_map(|(prefix, uri)| prefix.as_deref().map(|prefix| (prefix, uri)))
        .find(|(prefix, uri)| uri.as_str() == EVENT_NAMESPACE_URI && bound(prefix) == Some(uri))
        .map(|(prefix, _)| prefix.to_owned());
    let insertion = if let Some(prefix) = in_scope {
        format!(" {prefix}:schema={}", quoted(&declared.schema))
    } else {
        // `event`, else the first `event<N>` no declaration on the path
        // binds (the path declares finitely many prefixes).
        let prefix = std::iter::once("event".to_owned())
            .chain((1..=prefixes.len()).map(|index| format!("event{index}")))
            .find(|prefix| bound(prefix).is_none())
            .unwrap_or_else(|| format!("event{}", prefixes.len().saturating_add(1)));
        format!(
            " xmlns:{prefix}=\"{EVENT_NAMESPACE_URI}\" {prefix}:schema={}",
            quoted(&declared.schema)
        )
    };
    let mut diagnostic = Diagnostic::error(
        "SCXML096",
        format!(
            "this transition handles `{}` from an invoked service without an `event:schema`",
            declared.name
        ),
        events.span,
    )
    .with_category(DiagnosticCategory::Package)
    .with_primary_message("the service package declares this event's data schema")
    .with_note(format!(
        "the service package of `{type_uri}` declares that it emits `{}` with the schema `{}`",
        declared.name, declared.schema
    ))
    .with_help(
        "describe the event data the transition handles with the package's schema (`event:schema`)",
    )
    .with_spec_reference(SPEC_INVOKE)
    .with_fix(Fix::replace(
        format!(
            "add the schema the package declares for `{}`",
            declared.name
        ),
        Applicability::MaybeIncorrect,
        attribute_insertion_point(document, transition),
        insertion,
    ));
    diagnostic.severity = crate::Severity::Note;
    diagnostic
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DeclaredEvents;

    const URI: &str = "urn:svc";
    const REPLY: &str = r#"{"required":["text"],"type":"object"}"#;

    fn services() -> BTreeMap<String, BoundService> {
        BTreeMap::from([(
            URI.to_owned(),
            BoundService {
                env_slots: Vec::new(),
                events: DeclaredEvents {
                    declared: true,
                    emits: vec![DeclaredEvent {
                        name: "svc.reply".into(),
                        schema: REPLY.into(),
                        sha256: scxml_event_schema::sha256_hex(REPLY.as_bytes()),
                    }],
                    accepts: Vec::new(),
                },
            },
        )])
    }

    fn findings(body: &str, head: &str) -> Vec<Diagnostic> {
        let text = format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" {head} version="1.0" initial="s"><state id="s"><invoke type="{URI}"/>{body}</state></scxml>"#
        );
        let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
            scxml_syntax::SourceId::new(1),
            "d.scxml",
            text,
        ))
        .expect("the fixture parses");
        service_event_diagnostics(&document, Some(&services()), &Resources::new())
    }

    fn codes(body: &str) -> Vec<String> {
        findings(body, &format!(r#"xmlns:event="{EVENT_NAMESPACE_URI}""#))
            .into_iter()
            .map(|diagnostic| diagnostic.code)
            .collect()
    }

    #[test]
    fn the_declared_schema_or_true_is_compatible() {
        assert!(
            codes(&format!(
                r#"<transition event="svc.reply" event:schema='{REPLY}'/>"#
            ))
            .is_empty()
        );
        // The same schema written differently canonicalizes the same.
        assert!(codes(
            r#"<transition event="svc.reply" event:schema='{ "type": "object", "required": ["text"] }'/>"#
        )
        .is_empty());
        assert!(codes(r#"<transition event="svc.reply" event:schema="true"/>"#).is_empty());
        // Not judged: expressions, prefix descriptors, and other events.
        for body in [
            r#"<transition event="svc.reply" event:schemaexpr="s"/>"#,
            r#"<transition event="svc" event:schema='{"type":"string"}'/>"#,
            r#"<transition event="svc.*"/>"#,
            r#"<transition event="other" event:schema='{"type":"string"}'/>"#,
        ] {
            assert!(codes(body).is_empty(), "{body}");
        }
    }

    #[test]
    fn a_different_schema_is_scxml095_with_the_package_schema_as_fix() {
        let found = findings(
            r#"<transition event="svc.reply" event:schema='{"type":"string"}'/>"#,
            &format!(r#"xmlns:event="{EVENT_NAMESPACE_URI}""#),
        );
        let [finding] = found.as_slice() else {
            panic!("one finding: {found:?}");
        };
        assert_eq!(finding.code, "SCXML095");
        assert_eq!(finding.severity, crate::Severity::Error);
        assert_eq!(finding.fixes[0].edits[0].replacement, format!("'{REPLY}'"));
        // In a descendant state too.
        assert_eq!(
            codes(
                r#"<state id="inner"><transition event="svc.reply" event:schema='{"type":"string"}'/></state>"#
            ),
            ["SCXML095"]
        );
    }

    #[test]
    fn a_missing_schema_is_a_scxml096_note_whose_fix_adds_the_package_schema() {
        let found = findings(
            r#"<transition event="svc.reply"/>"#,
            &format!(r#"xmlns:ev="{EVENT_NAMESPACE_URI}""#),
        );
        let [finding] = found.as_slice() else {
            panic!("one finding: {found:?}");
        };
        assert_eq!(finding.code, "SCXML096");
        assert_eq!(finding.severity, crate::Severity::Note);
        assert_eq!(
            finding.fixes[0].edits[0].replacement,
            format!(" ev:schema='{REPLY}'")
        );
        // Without a prefix in scope, the fix declares one on the transition.
        let found = findings(r#"<transition event="svc.reply"/>"#, "");
        assert_eq!(
            found[0].fixes[0].edits[0].replacement,
            format!(r#" xmlns:event="{EVENT_NAMESPACE_URI}" event:schema='{REPLY}'"#)
        );
    }

    #[test]
    fn without_packages_or_contracts_nothing_is_judged() {
        let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
            scxml_syntax::SourceId::new(1),
            "d.scxml",
            format!(
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><state id="s"><invoke type="{URI}"/><transition event="svc.reply"/></state></scxml>"#
            ),
        ))
        .expect("parses");
        assert!(service_event_diagnostics(&document, None, &Resources::new()).is_empty());
        let mut bare = services();
        for service in bare.values_mut() {
            service.events = DeclaredEvents::default();
        }
        assert!(service_event_diagnostics(&document, Some(&bare), &Resources::new()).is_empty());
    }
}
