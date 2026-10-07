use floppy_service_generate::{FailureCode, Generation, parse_parameters};
use scxml_syntax::{Node, SourceId, SourceText, parse_xml};
use scxml_types::DataValue;

use super::{
    Attribute, Document, Element, FAILED_EVENT, FAILURE_CODES, Finding, PARAMETERS,
    REASONING_EFFORTS, RESULT_EVENT, SERVICE_URI, Severity, Transition, check,
};

const EVENT: &str = "https://github.com/stateforward/scxml.rs/namespaces/event";

fn span(span: scxml_syntax::Span) -> super::Span {
    (
        u32::try_from(span.start).expect("small"),
        u32::try_from(span.end).expect("small"),
    )
}

/// `source` parsed into the plain document and its `<invoke>` targets, as
/// scxml.rs builds the context.
fn document(source: &str) -> (Document, Vec<u32>) {
    let parsed = parse_xml(SourceText::new(
        SourceId::new(1),
        "doc.scx",
        source.to_owned(),
    ))
    .expect("the test document parses");
    let mut document = Document {
        datamodel: parsed
            .root
            .attribute("datamodel")
            .map(|attribute| attribute.value.clone()),
        ..Document::default()
    };
    let mut targets = Vec::new();
    let mut pending = vec![(&parsed.root, None)];
    while let Some((element, parent)) = pending.pop() {
        let index = u32::try_from(document.elements.len()).expect("small");
        if element.name.local_name == "invoke"
            && element
                .attribute("type")
                .is_some_and(|kind| kind.value == SERVICE_URI)
        {
            targets.push(index);
        }
        if element.name.local_name == "transition" {
            document.transitions.push(Transition {
                event: element.attribute("event").map(|event| event.value.clone()),
                schema: element
                    .attributes
                    .iter()
                    .find(|attribute| {
                        attribute.name.namespace_uri.as_deref() == Some(EVENT)
                            && attribute.name.local_name == "schema"
                    })
                    .map(|attribute| {
                        scxml_event_schema::resolve(attribute.value.as_bytes(), None)
                            .expect("the test schema resolves")
                            .schema
                    }),
            });
        }
        if let Some(parent) = parent {
            let parent: &mut Element =
                &mut document.elements[usize::try_from(parent).expect("small")];
            parent.children.push(index);
        }
        document.elements.push(Element {
            namespace: element.name.namespace_uri.clone(),
            local: element.name.local_name.clone(),
            span: span(element.span),
            attributes: element
                .attributes
                .iter()
                .map(|attribute| Attribute {
                    namespace: attribute.name.namespace_uri.clone(),
                    local: attribute.name.local_name.clone(),
                    value: attribute.value.clone(),
                    span: span(attribute.span),
                })
                .collect(),
            children: Vec::new(),
            parent,
        });
        for child in element.children.iter().rev() {
            if let Node::Element(child) = child {
                pending.push((child, Some(index)));
            }
        }
    }
    (document, targets)
}

fn scxml(body: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="{EVENT}" version="1.0" datamodel="ecmascript" initial="s"><state id="s">{body}</state></scxml>"#
    )
}

fn findings(body: &str) -> (String, Vec<Finding>) {
    let source = scxml(body);
    let (document, targets) = document(&source);
    let found = check(&document, &targets);
    (source, found)
}

fn codes(found: &[Finding]) -> Vec<&'static str> {
    found.iter().map(|finding| finding.code).collect()
}

/// `source` with the one fix of `finding` applied.
fn fixed(source: &str, finding: &Finding) -> String {
    let fix = finding.fix.as_ref().expect("a fix");
    let (start, end) = (
        usize::try_from(fix.span.0).expect("small"),
        usize::try_from(fix.span.1).expect("small"),
    );
    format!("{}{}{}", &source[..start], fix.replacement, &source[end..])
}

fn invoke(params: &str) -> String {
    format!(r#"<invoke type="{SERVICE_URI}">{params}</invoke>"#)
}

const MODEL: &str = r#"<param name="model" expr="'m'"/>"#;
const PROMPT: &str = r#"<param name="prompt" expr="'p'"/>"#;

#[test]
fn a_well_formed_invocation_is_clean() {
    let body = invoke(&format!(
        r#"{MODEL}{PROMPT}<param name="reasoning_effort" expr="'high'"/>"#
    ));
    assert!(findings(&body).1.is_empty());
}

#[test]
fn a_reasoning_effort_typo_gets_a_machine_applicable_fix() {
    let body = invoke(&format!(
        r#"{MODEL}{PROMPT}<param name="reasoning_effort" expr="'hgh'"/>"#
    ));
    let (source, found) = findings(&body);
    assert_eq!(codes(&found), ["invalid_value"]);
    assert!(found[0].message.contains("hgh"));
    let repaired = fixed(&source, &found[0]);
    assert!(repaired.contains(r#"expr="'high'""#), "{repaired}");
    assert!(findings_of(&repaired).is_empty());
    // Too far from any value: no guess.
    let far = invoke(&format!(
        r#"{MODEL}{PROMPT}<param name="reasoning_effort" expr="'maximal effort'"/>"#
    ));
    let (_, found) = findings(&far);
    assert_eq!(codes(&found), ["invalid_value"]);
    assert!(found[0].fix.is_none());
}

fn findings_of(source: &str) -> Vec<Finding> {
    let (document, targets) = document(source);
    check(&document, &targets)
}

#[test]
fn an_unknown_parameter_close_to_one_is_renamed() {
    let body = invoke(&format!(r#"{MODEL}<param name="promt" expr="'p'"/>"#));
    let (source, found) = findings(&body);
    assert_eq!(codes(&found), ["unknown_param", "missing_param"]);
    let repaired = fixed(&source, &found[0]);
    assert!(repaired.contains(r#"name="prompt""#), "{repaired}");
    assert!(findings_of(&repaired).is_empty());
}

#[test]
fn required_repeated_and_dependent_parameters_are_checked() {
    let (_, found) = findings(&invoke(PROMPT));
    assert_eq!(codes(&found), ["missing_param"]);
    let (_, found) = findings(&invoke(&format!(
        r#"{MODEL}{PROMPT}<param name="messages" expr="m"/>"#
    )));
    assert_eq!(codes(&found), ["missing_param"]);
    let (_, found) = findings(&invoke(&format!("{MODEL}{PROMPT}{MODEL}")));
    assert_eq!(codes(&found), ["duplicate_param"]);
    let (_, found) = findings(&invoke(&format!(
        r#"{MODEL}{PROMPT}<param name="strict" expr="true"/>"#
    )));
    assert_eq!(codes(&found), ["missing_param"]);
    let (_, found) = findings(&invoke(&format!(
        r#"{MODEL}{PROMPT}<param name="schema" expr="s"/><param name="strict" expr="'yes'"/><param name="schemaname" expr="'a b'"/>"#
    )));
    assert_eq!(codes(&found), ["invalid_value", "invalid_value"]);
}

#[test]
fn transitions_that_reject_what_generate_returns_are_schema_mismatches() {
    let missing_code = r#"{"type":"object","properties":{"code":{"enum":["transport","status"]}},"required":["code"]}"#;
    let body = format!(
        r#"{}<transition event="generate.failed" event:schema='{missing_code}'/>"#,
        invoke(&format!("{MODEL}{PROMPT}"))
    );
    let (_, found) = findings(&body);
    assert_eq!(codes(&found), ["schema_mismatch"]);
    assert_eq!(found[0].severity, Severity::Warning);
    assert!(
        found[0]
            .help
            .as_deref()
            .is_some_and(|help| help.contains("\"schema-mismatch\"")),
        "{found:?}"
    );
    // Without a schema parameter, the reply is {"text": …}.
    let json_only = r#"{"type":"object","properties":{"json":true},"required":["json"]}"#;
    let body = format!(
        r#"{}<transition event="generate.result" event:schema='{json_only}'/>"#,
        invoke(&format!("{MODEL}{PROMPT}"))
    );
    assert_eq!(codes(&findings(&body).1), ["schema_mismatch"]);
    // With one, the reply's value is not known here: nothing is claimed.
    let body = format!(
        r#"{}<transition event="generate.result" event:schema='{json_only}'/>"#,
        invoke(&format!(
            r#"{MODEL}{PROMPT}<param name="schema" expr="s"/>"#
        ))
    );
    assert!(findings(&body).1.is_empty());
}

#[test]
fn the_builder_bot_schemas_match_what_generate_returns() {
    let failed = r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"code":{"enum":["unconfigured","transport","status","malformed","truncated","filtered","refusal","not-json","schema-mismatch"]}},"required":["code"],"additionalProperties":false}"#;
    let result = r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","oneOf":[{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false},{"type":"object","properties":{"json":true},"required":["json"],"additionalProperties":false}]}"#;
    let body = format!(
        r#"{}<transition event="generate.result" event:schema='{result}'/><transition event="generate.failed" event:schema='{failed}'/>"#,
        invoke(&format!("{MODEL}{PROMPT}"))
    );
    assert!(findings(&body).1.is_empty());
}

#[test]
fn the_contract_is_the_service_s_own() {
    assert_eq!(
        REASONING_EFFORTS,
        floppy_service_generate::REASONING_EFFORTS
    );
    assert_eq!(SERVICE_URI, floppy_service_generate::SERVICE_URI);
    assert_eq!(RESULT_EVENT, floppy_service_generate::RESULT_EVENT);
    assert_eq!(FAILED_EVENT, floppy_service_generate::FAILED_EVENT);
    let codes: Vec<&str> = [
        FailureCode::Unconfigured,
        FailureCode::Transport,
        FailureCode::Status,
        FailureCode::Malformed,
        FailureCode::Truncated,
        FailureCode::Filtered,
        FailureCode::Refusal,
        FailureCode::NotJson,
        FailureCode::SchemaMismatch,
    ]
    .iter()
    .map(|code| code.name())
    .collect();
    assert_eq!(codes, FAILURE_CODES);
    // Every listed parameter is one the service takes: given twice, it is
    // refused as repeated, never as unknown.
    let text = DataValue::String("x".to_owned());
    for name in PARAMETERS {
        let error = parse_parameters([(name, &text), (name, &text)])
            .expect_err("a repeated parameter is refused");
        assert_eq!(error, "generate takes each parameter once", "{name}");
    }
    let accepted: Result<Generation, _> = parse_parameters([("model", &text), ("prompt", &text)]);
    assert!(accepted.is_ok());
}
