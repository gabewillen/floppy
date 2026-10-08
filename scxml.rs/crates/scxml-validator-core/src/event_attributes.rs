//! The `event` attribute vocabulary ([`scxml_types::EVENT_NAMESPACE_URI`]).
//!
//! `<transition>`, `<send>`, `<raise>`, and `<donedata>` may carry
//! `event:schema` (a JSON Schema, inline or by reference to a file relative
//! to the document) or `event:schemaexpr` (an expression whose value is the
//! schema) describing the event data the element handles or creates. The
//! attributes are metadata: they never change how a document executes. The
//! W3C schema admits attributes in other namespaces laxly, so these rules
//! check them:
//!
//! * `EVENT001`: an element in the `event` namespace (it has none), an
//!   `event` attribute on any other element, or an attribute name the
//!   vocabulary does not define.
//! * `EVENT002`: one element carries both `event:schema` and
//!   `event:schemaexpr`.
//! * `EVENT003`: the schema is not UTF-8 JSON.
//! * `EVENT004`: a referenced schema file is not available (it names no
//!   relative path below the document's directory, or its bytes were not
//!   supplied).
//! * `EVENT005`: the fragment is not a JSON Pointer, or names no value.
//! * `EVENT006`: the schema violates its dialect's metaschema or does not
//!   compile.
//! * `EVENT007`: `$schema` names an unsupported dialect.
//! * `EVENT008`: a reference that cannot be kept (another resource, a
//!   missing target, or a schema a pointer cannot move).
//! * `LIMIT005`: a schema over the Machine IR string bound (wasm builds
//!   only).
//!
//! `event:schemaexpr` is checked as a value expression by the data model's
//! syntax checker, like every other `*expr` attribute.
//!
//! The referenced files' bytes come from the caller
//! ([`crate::Validator::with_event_schema_resources`]); this module performs
//! no I/O. Pure and bounded by the document and the supplied bytes; it runs
//! inside the validator's boundary span and records no telemetry of its own.

use scxml_event_schema::{
    ReferenceReason, Resources, SCHEMA_ATTRIBUTE, SCHEMA_EXPR_ATTRIBUTE, SchemaError, Source,
};
use scxml_reporting::Applicability;
use scxml_syntax::{Attribute, Document, Element, Span};
use scxml_types::EVENT_NAMESPACE_URI;

use crate::spans::attribute_removal_span;
use crate::{Diagnostic, DiagnosticCategory, Fix, Label, SCXML_NAMESPACE};

/// The elements an `event` attribute may be on, as written in messages.
const OWNERS: &str = "`<transition>`, `<send>`, `<raise>`, or `<donedata>`";
const JSON_SPEC: &str = "RFC 8259 The JavaScript Object Notation (JSON) Data Interchange Format";
const POINTER_SPEC: &str = "RFC 6901 JavaScript Object Notation (JSON) Pointer";
const SCHEMA_SPEC: &str = "JSON Schema 2020-12 Core";
const VALIDATION_SPEC: &str = "JSON Schema 2020-12 Validation";

/// Every finding of the `event` vocabulary in `document`; `resources` holds
/// the bytes of each referenced schema file, keyed by the reference's
/// resource part as authored.
#[must_use]
pub fn event_attribute_diagnostics(document: &Document, resources: &Resources) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    document.root.visit(&mut |element| {
        if element.name.namespace_uri.as_deref() == Some(EVENT_NAMESPACE_URI) {
            diagnostics.push(event_element(element));
            return;
        }
        let attributes: Vec<&Attribute> = element
            .attributes
            .iter()
            .filter(|attribute| {
                attribute.name.namespace_uri.as_deref() == Some(EVENT_NAMESPACE_URI)
            })
            .collect();
        if attributes.is_empty() {
            return;
        }
        if !is_owner(element) {
            for attribute in attributes {
                diagnostics.push(misplaced(document, element, attribute));
            }
            return;
        }
        let mut schema = None;
        let mut expression = None;
        for attribute in attributes {
            match attribute.name.local_name.as_str() {
                SCHEMA_ATTRIBUTE => schema = Some(attribute),
                SCHEMA_EXPR_ATTRIBUTE => expression = Some(attribute),
                _ => diagnostics.push(unknown(element, attribute)),
            }
        }
        if let (Some(schema), Some(expression)) = (schema, expression) {
            diagnostics.push(both(document, element, schema, expression));
        }
        if let Some(schema) = schema {
            diagnostics.extend(schema_diagnostic(document, element, schema, resources));
        }
    });
    diagnostics
}

/// Whether `element` is an SCXML element that may carry the attributes.
fn is_owner(element: &Element) -> bool {
    element.name.namespace_uri.as_deref() == Some(SCXML_NAMESPACE)
        && scxml_event_schema::is_owner_element(&element.name.local_name)
}

fn event_element(element: &Element) -> Diagnostic {
    Diagnostic::error(
        "EVENT001",
        format!(
            "`<{}>` is an element in the `event` namespace, which has none",
            element.lexical_name
        ),
        element.start_tag_span,
    )
    .with_category(DiagnosticCategory::Semantic)
    .with_primary_message("not an element of any vocabulary")
    .with_note(format!(
        "`{EVENT_NAMESPACE_URI}` is an attribute vocabulary only: `event:schema` or `event:schemaexpr` on {OWNERS} describes the event data the element handles or creates"
    ))
    .with_help("remove the element, and describe the event data with an `event:schema` attribute on the element that handles or creates it")
}

fn misplaced(document: &Document, element: &Element, attribute: &Attribute) -> Diagnostic {
    Diagnostic::error(
        "EVENT001",
        format!(
            "`{}` is not allowed on `<{}>`",
            attribute.lexical_name, element.lexical_name
        ),
        attribute.span,
    )
    .with_category(DiagnosticCategory::Semantic)
    .with_primary_message("an `event` attribute on an element that handles or creates no event data")
    .with_note(format!(
        "the `event` vocabulary describes event data only on {OWNERS}: the `_event.data` a transition handles, the data a `<send>` or `<raise>` creates its event with, or the data of the `done.state.<id>` event of a `<donedata>`"
    ))
    .with_help(format!("move the attribute to {OWNERS}, or remove it"))
    .with_fix(Fix::replace(
        format!("remove `{}`", attribute.lexical_name),
        Applicability::MaybeIncorrect,
        attribute_removal_span(document, attribute),
        "",
    ))
}

fn unknown(element: &Element, attribute: &Attribute) -> Diagnostic {
    let local = attribute.name.local_name.as_str();
    let mut diagnostic = Diagnostic::error(
        "EVENT001",
        format!(
            "`{}` is not an attribute of the `event` vocabulary",
            attribute.lexical_name
        ),
        attribute.span,
    )
    .with_category(DiagnosticCategory::Semantic)
    .with_primary_message(format!("unknown on `<{}>`", element.lexical_name))
    .with_note("the `event` vocabulary has exactly two attributes: `event:schema` (a JSON Schema, inline or by reference) and `event:schemaexpr` (an expression whose value is the schema)");
    let typed: Vec<char> = local.chars().collect();
    let closest = [SCHEMA_ATTRIBUTE, SCHEMA_EXPR_ATTRIBUTE]
        .into_iter()
        .map(|name| {
            let candidate: Vec<char> = name.chars().collect();
            (crate::semantic::osa_distance(&typed, &candidate), name)
        })
        .filter(|(distance, _)| *distance <= 2)
        .min();
    if let Some((_, name)) = closest {
        let prefix_len = attribute.lexical_name.len().saturating_sub(local.len());
        let name_span = Span::new(
            attribute.span.source,
            attribute.span.start.saturating_add(prefix_len),
            attribute
                .span
                .start
                .saturating_add(attribute.lexical_name.len()),
        );
        diagnostic = diagnostic
            .with_help(format!("did you mean `{name}`?"))
            .with_fix(Fix::replace(
                format!("use `{name}`"),
                Applicability::MaybeIncorrect,
                name_span,
                name,
            ));
    } else {
        diagnostic = diagnostic
            .with_help("use `event:schema` or `event:schemaexpr`, or remove the attribute");
    }
    diagnostic
}

fn both(
    document: &Document,
    element: &Element,
    schema: &Attribute,
    expression: &Attribute,
) -> Diagnostic {
    Diagnostic::error(
        "EVENT002",
        format!(
            "`<{}>` has both `event:schema` and `event:schemaexpr`",
            element.lexical_name
        ),
        expression.span,
    )
    .with_category(DiagnosticCategory::Semantic)
    .with_primary_message("a second schema for the same event data")
    .with_secondary(Label::new(schema.span).with_message("the schema is given here"))
    .with_note("as with SCXML's other attribute/`expr` pairs, an element gives its schema either literally (`event:schema`) or as an expression (`event:schemaexpr`), not both")
    .with_help("keep one of the two attributes")
    .with_fix(Fix::replace(
        format!("remove `{}`", expression.lexical_name),
        Applicability::MaybeIncorrect,
        attribute_removal_span(document, expression),
        "",
    ))
}

/// The finding for one `event:schema` value, if its schema does not resolve.
fn schema_diagnostic(
    document: &Document,
    element: &Element,
    attribute: &Attribute,
    resources: &Resources,
) -> Option<Diagnostic> {
    let value = attribute.value.as_str();
    let offsets = document
        .source
        .slice(attribute.value_span)
        .and_then(|raw| crate::datamodel_check::decode_offsets(raw, attribute.value_span.start))
        // XML normalizes a literal tab or line break in an attribute value to
        // a space (XML 1.0 §3.3.3), byte for byte, so a schema written over
        // several lines still maps onto its source.
        .filter(|(decoded, _)| {
            decoded == value || decoded.replace(['\t', '\n', '\r'], " ") == value
        })
        .map(|(_, offsets)| offsets);
    // Maps a decoded byte range of the value onto the source.
    let span_of = |start: usize, end: usize| -> Span {
        offsets
            .as_ref()
            .and_then(|offsets| Some((*offsets.get(start)?, *offsets.get(end)?)))
            .map_or(attribute.value_span, |(start, end)| {
                Span::new(attribute.value_span.source, start, end)
            })
    };
    let mut pointer_fix = None;
    let (result, file, fragment_span) = match scxml_event_schema::source(value) {
        Source::Inline(text) => (
            scxml_event_schema::resolve(text.as_bytes(), None),
            None,
            None,
        ),
        Source::Reference { resource, fragment } => {
            if let Some(problem) = scxml_event_schema::resource_problem(resource) {
                return Some(unavailable(element, attribute, resource, problem));
            }
            let Some(bytes) = resources.get(resource) else {
                return Some(unavailable(
                    element,
                    attribute,
                    resource,
                    "no bytes were supplied for it: the file does not exist or could not be read relative to the document",
                ));
            };
            // The fragment's position in the value, after its `#`.
            let fragment_start = fragment.and_then(|fragment| {
                let end = value.trim_end_matches(scxml_types::is_xml_space).len();
                end.checked_sub(fragment.len())
            });
            let fragment_span = fragment_start.map(|start| {
                span_of(
                    start.saturating_sub(1),
                    start + fragment.map_or(0, str::len),
                )
            });
            let result = scxml_event_schema::resolve(bytes, fragment);
            if let (Err(SchemaError::PointerNotFound { pointer }), Some(fragment), Some(start)) =
                (&result, fragment, fragment_start)
            {
                pointer_fix = member_fix(bytes, fragment, pointer)
                    .map(|(from, to, name)| (span_of(start + from, start + to), name));
            }
            (result, Some(resource), fragment_span)
        }
    };
    let error = result.err()?;
    let what = file.map_or_else(
        || format!("the inline schema of `<{}>`", element.lexical_name),
        |file| format!("schema file `{file}`"),
    );
    // In an inline schema a finding points at its exact bytes: the byte the
    // JSON parser stopped at, or the value of the offending keyword.
    let mut place = Place {
        whole: attribute.value_span,
        at: None,
        fragment: fragment_span,
        type_fix: None,
        pointer_fix,
    };
    if file.is_none() {
        match &error {
            SchemaError::InvalidJson { offset, .. } => {
                let end = value
                    .get(*offset..)
                    .and_then(|rest| rest.chars().next())
                    .map_or(*offset, |character| offset + character.len_utf8());
                place.at = Some(span_of(*offset, end));
            }
            SchemaError::InvalidSchema { location, .. }
            | SchemaError::Reference { location, .. } => {
                if let Some((start, end)) = scxml_event_schema::locate(value, location) {
                    place.at = Some(span_of(start, end));
                    if location.ends_with("/type") {
                        place.type_fix = type_fix(value, start, end)
                            .map(|(start, end, name)| (span_of(start, end), name));
                    }
                }
            }
            SchemaError::UnsupportedDialect { .. } => {
                place.at = scxml_event_schema::locate(value, "/$schema")
                    .map(|(start, end)| span_of(start, end));
            }
            _ => {}
        }
    }
    Some(schema_error_diagnostic(&what, &error, &place))
}

/// Where one schema finding is in the document.
struct Place {
    /// The attribute value.
    whole: Span,
    /// The exact bytes in an inline schema, when known.
    at: Option<Span>,
    /// The fragment of a reference.
    fragment: Option<Span>,
    /// A misspelled `type` name's bytes (inside its quotes) and the type it
    /// is closest to.
    type_fix: Option<(Span, &'static str)>,
    /// A misspelled member name in a reference's JSON Pointer and the member
    /// it is closest to.
    pointer_fix: Option<(Span, String)>,
}

/// The member of the file closest to the last token of `pointer`, the
/// longest prefix of the fragment that names no value, with that token's
/// range in `fragment`; only when the fragment spells the pointer verbatim
/// (no percent-encoding) and the member needs no escaping.
fn member_fix(bytes: &[u8], fragment: &str, pointer: &str) -> Option<(usize, usize, String)> {
    if !fragment.starts_with(pointer) {
        return None;
    }
    let (parent, last) = pointer.rsplit_once('/')?;
    let typed: Vec<char> = last.chars().collect();
    scxml_event_schema::member_names(bytes, parent)
        .into_iter()
        .filter(|name| {
            name.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'$')
            })
        })
        .map(|name| {
            let candidate: Vec<char> = name.chars().collect();
            (crate::semantic::osa_distance(&typed, &candidate), name)
        })
        .filter(|(distance, _)| *distance > 0 && *distance <= 2)
        .min()
        .map(|(_, name)| (parent.len() + 1, pointer.len(), name))
}

/// The closest JSON Schema type name to the string at `start..end` of
/// `text` (a `type` value with no escapes), with the range inside its
/// quotes.
fn type_fix(text: &str, start: usize, end: usize) -> Option<(usize, usize, &'static str)> {
    let inner = text.get(start..end)?.strip_prefix('"')?.strip_suffix('"')?;
    if inner.contains('\\') {
        return None;
    }
    let typed: Vec<char> = inner.chars().collect();
    scxml_event_schema::TYPE_NAMES
        .into_iter()
        .map(|name| {
            let candidate: Vec<char> = name.chars().collect();
            (crate::semantic::osa_distance(&typed, &candidate), name)
        })
        .filter(|(distance, _)| *distance > 0 && *distance <= 2)
        .min()
        .map(|(_, name)| (start + 1, end - 1, name))
}

/// The finding for one schema that does not resolve.
fn schema_error_diagnostic(what: &str, error: &SchemaError, place: &Place) -> Diagnostic {
    let whole = place.whole;
    let at = place.at.unwrap_or(whole);
    match error {
        SchemaError::InvalidJson { .. } if place.at.is_some() => json_error(what, error, at),
        SchemaError::InvalidJson { .. } | SchemaError::InvalidUtf8 { .. } => {
            json_error(what, error, whole)
        }
        SchemaError::TooLarge { limit } => Diagnostic::error(
            "LIMIT005",
            format!("{what} exceeds the Machine IR string bound"),
            whole,
        )
        .with_category(DiagnosticCategory::Internal)
        .with_primary_message(format!("over {limit} bytes"))
        .with_help("split the schema, or describe less of the event data"),
        SchemaError::InvalidPointer { .. } | SchemaError::PointerNotFound { .. } => {
            let diagnostic = Diagnostic::error(
                "EVENT005",
                format!("the fragment of `event:schema` does not select a schema in {what}: {error}"),
                place.fragment.unwrap_or(whole),
            )
            .with_category(DiagnosticCategory::Semantic)
            .with_primary_message(match error {
                SchemaError::InvalidPointer { .. } => "not a JSON Pointer",
                _ => "names no value",
            })
            .with_note("the fragment of a schema reference is a JSON Pointer (RFC 6901), percent-encoded as a URI fragment: `#/$defs/Order` selects member `Order` of member `$defs`; `~1` stands for `/` and `~0` for `~` in a member name")
            .with_spec_reference(POINTER_SPEC);
            match &place.pointer_fix {
                Some((span, name)) => diagnostic
                    .with_help(format!("did you mean `{name}`?"))
                    .with_fix(Fix::replace(
                        format!("select member `{name}`"),
                        Applicability::MaybeIncorrect,
                        *span,
                        name.clone(),
                    )),
                None => diagnostic.with_help(
                    "point at an existing schema in the file, or drop the fragment to use the whole file",
                ),
            }
        }
        SchemaError::UnsupportedDialect { .. } => Diagnostic::error(
            "EVENT007",
            format!("{what} uses an unsupported JSON Schema dialect: {error}"),
            at,
        )
        .with_category(DiagnosticCategory::Semantic)
        .with_primary_message("unsupported `$schema`")
        .with_note("the supported dialects are JSON Schema 2020-12 (`https://json-schema.org/draft/2020-12/schema`, the default when `$schema` is absent), 2019-09 (`https://json-schema.org/draft/2019-09/schema`), and draft-07 (`http://json-schema.org/draft-07/schema#`)")
        .with_help("declare one of the supported dialects in `$schema`, or remove `$schema` to use 2020-12")
        .with_spec_reference(SCHEMA_SPEC),
        SchemaError::InvalidSchema { .. } => {
            let diagnostic = Diagnostic::error(
                "EVENT006",
                format!("{what} is not a valid JSON Schema: {error}"),
                at,
            )
            .with_category(DiagnosticCategory::Semantic)
            .with_primary_message(if place.type_fix.is_some() {
                "not a JSON Schema type"
            } else {
                "rejected by its dialect's metaschema"
            })
            .with_note("a schema is checked against the metaschema of its dialect and compiled once, so every keyword has a valid value and every `pattern` is a valid regular expression")
            .with_spec_reference(VALIDATION_SPEC);
            match place.type_fix {
                Some((span, name)) => diagnostic
                    .with_help(format!("did you mean `{name}`?"))
                    .with_fix(Fix::replace(
                        format!("use the type `{name}`"),
                        Applicability::MaybeIncorrect,
                        span,
                        name,
                    )),
                None => diagnostic.with_help("correct the keyword at the named location"),
            }
        }
        SchemaError::Reference { reason, .. } => Diagnostic::error(
            "EVENT008",
            format!("{what} has a reference the compiler cannot keep: {error}"),
            at,
        )
        .with_category(DiagnosticCategory::Semantic)
        .with_primary_message(match reason {
            ReferenceReason::External => "references another resource",
            ReferenceReason::Unresolved => "the reference names no value",
            ReferenceReason::NotRelocatable(_) => "cannot be moved out of its file",
            ReferenceReason::TooLarge => "too large or too deep to bundle",
        })
        .with_note("no schema is fetched: a `$ref` may only name a fragment of the same file (`#/$defs/Item`); when `event:schema` selects part of a file with a pointer, the references it uses are bundled into the result, which is impossible for schemas that rely on `$id`, `$anchor`, plain-name fragments, or dynamic references")
        .with_help("copy the referenced schema into the same file and reference it by JSON Pointer, or reference the whole file without a fragment")
        .with_spec_reference(SCHEMA_SPEC),
    }
}

fn json_error(what: &str, error: &SchemaError, span: Span) -> Diagnostic {
    Diagnostic::error(
        "EVENT003",
        format!("{what} is not JSON: {error}"),
        span,
    )
    .with_category(DiagnosticCategory::Syntax)
    .with_primary_message("invalid JSON")
    .with_note("an `event:schema` value whose first non-white-space character is `{` (or that is exactly `true` or `false`) is the schema's JSON text; any other value is a reference to a JSON file relative to the document")
    .with_help("write the schema as UTF-8 JSON; in a double-quoted XML attribute write `\"` as `&quot;`, or quote the attribute with `'`")
    .with_spec_reference(JSON_SPEC)
}

fn unavailable(
    element: &Element,
    attribute: &Attribute,
    resource: &str,
    problem: &str,
) -> Diagnostic {
    Diagnostic::error(
        "EVENT004",
        format!(
            "the schema file `{resource}` of `<{}>` is not available",
            element.lexical_name
        ),
        attribute.value_span,
    )
    .with_category(DiagnosticCategory::Semantic)
    .with_primary_message(problem.to_owned())
    .with_note("a schema reference names a JSON file by a path relative to the document, at or below its directory; the compiler reads it at compile time and embeds the schema in the Machine IR, so nothing is read or fetched while a document runs")
    .with_help("create the file, correct the path, or write the schema inline")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(text: &str) -> Document {
        scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
            scxml_syntax::SourceId::new(1),
            "d.scxml",
            text,
        ))
        .expect("the fixture parses")
    }

    fn machine(body: &str) -> String {
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="{EVENT_NAMESPACE_URI}" version="1.0"><state id="s">{body}</state><final id="f"/></scxml>"#
        )
    }

    fn codes_with(body: &str, resources: &Resources) -> Vec<String> {
        event_attribute_diagnostics(&document(&machine(body)), resources)
            .into_iter()
            .map(|diagnostic| diagnostic.code)
            .collect()
    }

    fn codes(body: &str) -> Vec<String> {
        codes_with(body, &Resources::new())
    }

    #[test]
    fn owners_accept_inline_schemas_and_expressions() {
        for body in [
            r#"<transition event="e" target="f" event:schema='{"type":"object"}'/>"#,
            r#"<transition event="e" target="f" event:schema="true"/>"#,
            r#"<transition event="e" target="f" event:schemaexpr="schemas.order"/>"#,
            r#"<onentry><raise event="r" event:schema='{"type":"null"}'/><send event="x" event:schema="false"/></onentry>"#,
        ] {
            assert!(codes(body).is_empty(), "{body}");
        }
    }

    #[test]
    fn placement_names_and_pairs_are_checked() {
        for (body, expected) in [
            ("<event:schema/>", &["EVENT001"][..]),
            (r#"<onentry event:schema="true"/>"#, &["EVENT001"]),
            (
                r#"<transition event="e" event:schma="true"/>"#,
                &["EVENT001"],
            ),
            (
                r#"<transition event="e" event:schema="true" event:schemaexpr="x"/>"#,
                &["EVENT002"],
            ),
        ] {
            assert_eq!(codes(body), expected, "{body}");
        }
        let typo = event_attribute_diagnostics(
            &document(&machine(r#"<transition event="e" event:schemexpr="x"/>"#)),
            &Resources::new(),
        );
        assert_eq!(typo[0].fixes[0].edits[0].replacement, "schemaexpr");
    }

    #[test]
    fn schema_failures_have_their_own_codes() {
        let resources = Resources::from([
            (
                "order.json".to_owned(),
                br##"{"$defs":{"Order":{"$ref":"#/$defs/Item"},"Item":{"type":"string"}}}"##
                    .to_vec(),
            ),
            ("broken.json".to_owned(), b"{".to_vec()),
        ]);
        for (body, expected) in [
            (
                r#"<transition event="e" event:schema='{"type":}'/>"#,
                &["EVENT003"][..],
            ),
            (
                r#"<transition event="e" event:schema="broken.json"/>"#,
                &["EVENT003"],
            ),
            (
                r#"<transition event="e" event:schema="missing.json"/>"#,
                &["EVENT004"],
            ),
            (
                r#"<transition event="e" event:schema="../order.json"/>"#,
                &["EVENT004"],
            ),
            (
                r#"<transition event="e" event:schema="order.json#/$defs/Nope"/>"#,
                &["EVENT005"],
            ),
            (
                r#"<transition event="e" event:schema='{"type":"strin"}'/>"#,
                &["EVENT006"],
            ),
            (
                r#"<transition event="e" event:schema='{"$schema":"http://json-schema.org/draft-04/schema#"}'/>"#,
                &["EVENT007"],
            ),
            (
                r#"<transition event="e" event:schema='{"$ref":"other.json"}'/>"#,
                &["EVENT008"],
            ),
            (
                r#"<transition event="e" event:schema="order.json#/$defs/Order"/>"#,
                &[],
            ),
            (r#"<transition event="e" event:schema="order.json"/>"#, &[]),
        ] {
            assert_eq!(codes_with(body, &resources), expected, "{body}");
        }
    }

    #[test]
    fn inline_json_errors_point_into_the_value() {
        let text = machine(r#"<transition event="e" event:schema='{"type": x}'/>"#);
        let found = event_attribute_diagnostics(&document(&text), &Resources::new());
        let span = found[0].primary.span;
        assert_eq!(&text[span.start..span.end], "x");
        // A schema over several lines (each line break normalized to a space
        // in the attribute value) still maps onto its exact bytes.
        let text =
            machine("<transition event=\"e\" event:schema='{\n  \"type\":\n    \"strng\"\n}'/>");
        let found = event_attribute_diagnostics(&document(&text), &Resources::new());
        let span = found[0].primary.span;
        assert_eq!(&text[span.start..span.end], "\"strng\"");
        assert_eq!(found[0].fixes[0].edits[0].replacement, "string");
    }
}
