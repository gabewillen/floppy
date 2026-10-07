use std::sync::{Arc, Mutex};

use scxml_syntax::{Document, SourceId, SourceText, parse_xml};

use super::types::{Applicability, Diagnostic, Edit, Fix, Label, Severity, Span};
use super::{PackageValidator, PackageValidatorFailure, document};
use crate::{
    MemorySchemaResolver, Report, SchemaBackendError, SchemaMode, SchemaSource, Validator,
};

const EXAMPLE: &str = "https://example.com/namespaces/ex";
const SERVICE: &str = "https://github.com/acme/bots/services/generate";

type Answer = Box<
    dyn Fn(&document::Context, &[u32]) -> Result<Vec<Diagnostic>, PackageValidatorFailure> + Send,
>;

/// What one call answers, recorded with its targets.
struct Scripted {
    answer: Mutex<Option<Answer>>,
    calls: Mutex<Vec<(String, Vec<u32>)>>,
}

impl std::fmt::Debug for Scripted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Scripted")
    }
}

impl PackageValidator for Scripted {
    fn validate(
        &self,
        package: &str,
        document: &document::Context,
        targets: &[u32],
    ) -> Result<Vec<Diagnostic>, PackageValidatorFailure> {
        self.calls
            .lock()
            .expect("the call log is never poisoned")
            .push((package.to_owned(), targets.to_vec()));
        match &*self.answer.lock().expect("the answer is never poisoned") {
            Some(answer) => answer(document, targets),
            None => Ok(Vec::new()),
        }
    }
}

fn scripted(
    answer: impl Fn(&document::Context, &[u32]) -> Result<Vec<Diagnostic>, PackageValidatorFailure>
    + Send
    + 'static,
) -> Arc<Scripted> {
    Arc::new(Scripted {
        answer: Mutex::new(Some(Box::new(answer))),
        calls: Mutex::new(Vec::new()),
    })
}

#[derive(Debug)]
struct NoSchema;

impl crate::SchemaValidator for NoSchema {
    fn validate(
        &self,
        _document: &Document,
        _schemas: &[SchemaSource],
    ) -> Result<Report, SchemaBackendError> {
        Ok(Report::new())
    }
}

fn source() -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:ex="{EXAMPLE}" xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event" version="1.0" initial="a">
  <state id="a">
    <onentry><ex:hello greeting="hi"/></onentry>
    <invoke type="{SERVICE}"><param name="prompt" expr="'x'"/></invoke>
    <transition event="go" event:schema='{{"type":"object"}}' target="b"/>
    <transition event="other" event:schemaexpr="s" target="b"/>
  </state>
  <final id="b"/>
</scxml>"#
    )
}

fn validate(validator: Arc<dyn PackageValidator>) -> Report {
    Validator::new(MemorySchemaResolver::new(), NoSchema)
        .with_schema_mode(SchemaMode::Off)
        .with_package_validator(validator)
        .validate_source(SourceId::new(1), "doc.scxml", source())
        .report
}

/// The span of the first occurrence of `needle` in the test document.
fn span_of(needle: &str) -> Span {
    let text = source();
    let start = text.find(needle).expect("the needle is in the document");
    Span {
        start: u32::try_from(start).expect("small"),
        end: u32::try_from(start + needle.len()).expect("small"),
    }
}

fn finding(code: &str, primary: Span, fixes: Vec<Fix>) -> Diagnostic {
    Diagnostic {
        code: code.to_owned(),
        severity: Severity::Warning,
        message: "a finding".to_owned(),
        primary: Label {
            span: primary,
            message: Some("here".to_owned()),
        },
        secondary: Vec::new(),
        help: Some("do this".to_owned()),
        fixes,
    }
}

fn package_codes(report: &Report) -> Vec<String> {
    report
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code.contains("::") || diagnostic.code.starts_with("PKG"))
        .map(|diagnostic| diagnostic.code.clone())
        .collect()
}

#[test]
fn each_package_is_called_once_with_its_targets_and_codes_are_prefixed() {
    let validator = scripted(|context, targets| {
        let target = context
            .elements
            .get(usize::try_from(targets[0]).expect("small"))
            .expect("targets index elements");
        let code = if target.name.local_name == "invoke" {
            "bad_param"
        } else {
            "bad_greeting"
        };
        Ok(vec![finding(code, target.span, Vec::new())])
    });
    let report = validate(validator.clone());
    let mut codes = package_codes(&report);
    codes.sort();
    assert_eq!(codes, ["ex::bad_greeting", "generate::bad_param"]);
    let calls = validator.calls.lock().expect("not poisoned").clone();
    assert_eq!(calls.len(), 2, "one call per package: {calls:?}");
    assert_eq!(calls[0].0, EXAMPLE);
    assert_eq!(calls[1].0, SERVICE);
    let rendered = report
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code == "ex::bad_greeting")
        .expect("rendered");
    assert_eq!(rendered.severity, Severity::Warning);
    assert_eq!(rendered.category, crate::DiagnosticCategory::Package);
    assert_eq!(rendered.help.as_deref(), Some("do this"));
}

#[test]
fn the_context_carries_transitions_with_literal_schemas_only() {
    let validator = scripted(|context, _| {
        assert_eq!(context.state_ids, ["a", "b"]);
        assert_eq!(context.datamodel, None);
        assert_eq!(context.transitions.len(), 2);
        assert_eq!(context.transitions[0].state, "a");
        assert_eq!(context.transitions[0].event.as_deref(), Some("go"));
        assert_eq!(
            context.transitions[0].event_schema.as_deref(),
            Some(r#"{"type":"object"}"#)
        );
        // `event:schemaexpr` is skipped silently.
        assert_eq!(context.transitions[1].event_schema, None);
        let root = &context.elements[0];
        assert_eq!(root.parent, None);
        for (index, element) in context.elements.iter().enumerate() {
            for child in &element.children {
                assert_eq!(
                    context.elements[usize::try_from(*child).expect("small")].parent,
                    Some(u32::try_from(index).expect("small"))
                );
            }
        }
        Ok(Vec::new())
    });
    let report = validate(validator);
    assert!(package_codes(&report).is_empty(), "{report:?}");
}

#[test]
fn package_prefixes_are_the_document_prefix_or_the_last_path_segment() {
    let document = parse_xml(SourceText::new(SourceId::new(1), "doc.scxml", source()))
        .expect("the document parses");
    let mut hello = None;
    document.root.visit(&mut |element| {
        if element.name.local_name == "hello" {
            hello = Some(element);
        }
    });
    assert_eq!(super::package_prefix(EXAMPLE, hello), "ex");
    assert_eq!(super::package_prefix(SERVICE, None), "generate");
    assert_eq!(
        super::package_prefix("https://stateforward.org/scxml/services/echo/", None),
        "echo"
    );
}

/// The single `PKG002` a failing package leaves, with no other finding of
/// it.
fn assert_fails_closed(
    answer: impl Fn(&document::Context, &[u32]) -> Result<Vec<Diagnostic>, PackageValidatorFailure>
    + Send
    + 'static,
    reason: &str,
) {
    let report = validate(scripted(answer));
    let failures: Vec<_> = report
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == "PKG002")
        .collect();
    assert_eq!(failures.len(), 2, "both packages fail closed: {report:?}");
    assert!(
        failures
            .iter()
            .any(|diagnostic| diagnostic.message.contains(EXAMPLE)
                && diagnostic.message.contains(reason)),
        "{failures:?}"
    );
    assert!(
        failures
            .iter()
            .all(|diagnostic| diagnostic.severity == Severity::Error)
    );
    assert!(
        !report
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code.contains("::")),
        "{report:?}"
    );
}

#[test]
fn failures_fail_closed() {
    assert_fails_closed(
        |_, _| Err(PackageValidatorFailure),
        "it trapped (fuel exhaustion included) or returned an error",
    );
}

#[test]
fn spans_out_of_range_or_off_character_boundaries_fail_closed() {
    assert_fails_closed(
        |_, _| {
            Ok(vec![finding(
                "late",
                Span {
                    start: 0,
                    end: 1_000_000,
                },
                Vec::new(),
            )])
        },
        "a span is outside the document",
    );
    assert_fails_closed(
        |_, _| {
            Ok(vec![finding(
                "inverted",
                Span { start: 5, end: 4 },
                Vec::new(),
            )])
        },
        "a span is outside the document",
    );
}

#[test]
fn overlapping_edits_fail_closed() {
    assert_fails_closed(
        |context, targets| {
            let span = context.elements[usize::try_from(targets[0]).expect("small")].span;
            let edit = |start: u32, end: u32| Edit {
                span: Span { start, end },
                replacement: "x".to_owned(),
            };
            Ok(vec![finding(
                "overlap",
                span,
                vec![Fix {
                    message: "two edits".to_owned(),
                    applicability: Applicability::MachineApplicable,
                    edits: vec![
                        edit(span.start + 1, span.start + 4),
                        edit(span.start + 2, span.start + 5),
                    ],
                }],
            )])
        },
        "a fix has overlapping edits",
    );
}

#[test]
fn edits_outside_the_targets_fail_closed() {
    assert_fails_closed(
        |_, _| {
            let outside = span_of("initial=\"a\"");
            Ok(vec![finding(
                "outside",
                outside,
                vec![Fix {
                    message: "edit the root".to_owned(),
                    applicability: Applicability::MachineApplicable,
                    edits: vec![Edit {
                        span: outside,
                        replacement: "initial=\"b\"".to_owned(),
                    }],
                }],
            )])
        },
        "an edit is outside the package's elements",
    );
}

#[test]
fn codes_outside_the_contract_fail_closed() {
    assert_fails_closed(
        |_, _| Ok(vec![finding("Not-A-Code", span_of("<scxml"), Vec::new())]),
        "a diagnostic code is not",
    );
}

#[test]
fn edits_inside_a_target_are_kept_with_their_applicability() {
    let validator = scripted(|context, targets| {
        let target = &context.elements[usize::try_from(targets[0]).expect("small")];
        if target.name.local_name != "hello" {
            return Ok(Vec::new());
        }
        let value = span_of("\"hi\"");
        Ok(vec![finding(
            "greeting",
            value,
            vec![Fix {
                message: "say hello".to_owned(),
                applicability: Applicability::MachineApplicable,
                edits: vec![Edit {
                    span: value,
                    replacement: "\"hello\"".to_owned(),
                }],
            }],
        )])
    });
    let report = validate(validator);
    let rendered = report
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code == "ex::greeting")
        .expect("kept");
    assert_eq!(rendered.fixes.len(), 1);
    assert_eq!(
        rendered.fixes[0].applicability,
        Applicability::MachineApplicable
    );
    assert_eq!(rendered.fixes[0].edits[0].replacement, "\"hello\"");
}
