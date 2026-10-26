use floppy_service_generate::{FailureCode, Generation, ImageReadFailure, parse_parameters};
use scxml_syntax::{Node, SourceId, SourceText, parse_xml};
use scxml_types::DataValue;

use super::{
    Attribute, Document, Element, FAILED_EVENT, FAILURE_CODES, Finding, MAX_COMPLETION_TOKENS,
    MAX_DIMENSIONS, MAX_TEMPERATURE, PARAMETERS, REASONING_EFFORTS, RESULT_EVENT, SERVICE_URI,
    Severity, TOOL_CHOICES, TOOLS_REASONING_EFFORT, Transition, check,
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
        r#"{MODEL}{PROMPT}<param name="schemaname" expr="'dispatch'"/>"#
    )));
    assert_eq!(codes(&found), ["missing_param"]);
    let (_, found) = findings(&invoke(&format!(
        r#"{MODEL}{PROMPT}<param name="schema" expr="s"/><param name="stream" expr="'yes'"/><param name="schemaname" expr="'a b'"/>"#
    )));
    assert_eq!(codes(&found), ["invalid_value", "invalid_value"]);
}

#[test]
fn the_tool_choice_and_the_sampling_parameters_are_checked() {
    const SCHEMA: &str = r#"<param name="schema" expr="s"/>"#;
    let clean = invoke(&format!(
        r#"{MODEL}{PROMPT}{SCHEMA}<param name="tool_choice" expr="'auto'"/><param name="temperature" expr="0"/><param name="max_completion_tokens" expr="512"/><param name="thinking_token_budget" expr="1024"/>"#
    ));
    assert!(findings(&clean).1.is_empty());
    // A typo gets a machine-applicable fix.
    let (source, found) = findings(&invoke(&format!(
        r#"{MODEL}{PROMPT}{SCHEMA}<param name="tool_choice" expr="'aut'"/>"#
    )));
    assert_eq!(codes(&found), ["invalid_value"]);
    let repaired = fixed(&source, &found[0]);
    assert!(repaired.contains(r#"expr="'auto'""#), "{repaired}");
    assert!(findings_of(&repaired).is_empty());
    // Only a schema takes a tool choice, and a count or a temperature is a
    // number.
    let (_, found) = findings(&invoke(&format!(
        r#"{MODEL}{PROMPT}<param name="tool_choice" expr="'auto'"/>"#
    )));
    assert_eq!(codes(&found), ["missing_param"]);
    let (_, found) = findings(&invoke(&format!(
        r#"{MODEL}{PROMPT}<param name="temperature" expr="'0'"/><param name="thinking_token_budget" expr="true"/>"#
    )));
    assert_eq!(codes(&found), ["invalid_value", "invalid_value"]);
    // A literal number is checked against the service's bounds, and a tool
    // choice is never one.
    for (name, number) in [
        ("temperature", "3"),
        ("temperature", "-0.5"),
        ("max_completion_tokens", "0"),
        ("max_completion_tokens", "1.5"),
        ("thinking_token_budget", "2000000"),
        ("temperature", "+3"),
        ("temperature", "Infinity"),
        ("temperature", "-Infinity"),
        ("temperature", "NaN"),
        ("max_completion_tokens", "1e999"),
    ] {
        let (_, found) = findings(&invoke(&format!(
            r#"{MODEL}{PROMPT}<param name="{name}" expr="{number}"/>"#
        )));
        assert_eq!(codes(&found), ["invalid_value"], "{name} {number}");
    }
    let (_, found) = findings(&invoke(&format!(
        r#"{MODEL}{PROMPT}{SCHEMA}<param name="tool_choice" expr="1"/>"#
    )));
    assert_eq!(codes(&found), ["invalid_value"]);
    let bounds = invoke(&format!(
        r#"{MODEL}{PROMPT}<param name="temperature" expr="+0.5"/>"#
    ));
    assert!(findings(&bounds).1.is_empty());
    // With a schema the service asks for a call with no reasoning, and
    // refuses any other reasoning_effort.
    let (source, found) = findings(&invoke(&format!(
        r#"{MODEL}{PROMPT}{SCHEMA}<param name="reasoning_effort" expr="'high'"/>"#
    )));
    assert_eq!(codes(&found), ["invalid_value"]);
    let repaired = fixed(&source, &found[0]);
    assert!(repaired.contains(r#"expr="'none'""#), "{repaired}");
    assert!(findings_of(&repaired).is_empty());
    let bounds = invoke(&format!(
        r#"{MODEL}{PROMPT}<param name="temperature" expr="2"/><param name="max_completion_tokens" expr="1048576"/><param name="thinking_token_budget" expr="1e3"/>"#
    ));
    assert!(findings(&bounds).1.is_empty());
    // Under a tool choice that may be `auto` the reply may hold no call.
    let calls_only = r#"{"type":"object","properties":{"tool_calls":{"type":"array","minItems":1}},"required":["tool_calls"]}"#;
    let body = format!(
        r#"{}<transition event="generate.result" event:schema='{calls_only}'/>"#,
        invoke(&format!(
            r#"{MODEL}{PROMPT}{SCHEMA}<param name="tool_choice" expr="choice"/>"#
        ))
    );
    assert_eq!(codes(&findings(&body).1), ["schema_mismatch"]);
    // Under a literal `required` it always holds one.
    let body = format!(
        r#"{}<transition event="generate.result" event:schema='{calls_only}'/>"#,
        invoke(&format!(
            r#"{MODEL}{PROMPT}{SCHEMA}<param name="tool_choice" expr="'required'"/>"#
        ))
    );
    assert!(findings(&body).1.is_empty());
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
    // Without a schema parameter, the reply is a text reply.
    let calls_only = r#"{"type":"object","properties":{"tool_calls":{"type":"array","minItems":1}},"required":["tool_calls"]}"#;
    let body = format!(
        r#"{}<transition event="generate.result" event:schema='{calls_only}'/>"#,
        invoke(&format!("{MODEL}{PROMPT}"))
    );
    assert_eq!(codes(&findings(&body).1), ["schema_mismatch"]);
    // With one, it holds a tool call.
    let body = format!(
        r#"{}<transition event="generate.result" event:schema='{calls_only}'/>"#,
        invoke(&format!(
            r#"{MODEL}{PROMPT}<param name="schema" expr="s"/>"#
        ))
    );
    assert!(findings(&body).1.is_empty());
    let text_only =
        r#"{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}"#;
    let body = format!(
        r#"{}<transition event="generate.result" event:schema='{text_only}'/>"#,
        invoke(&format!(
            r#"{MODEL}{PROMPT}<param name="schema" expr="s"/>"#
        ))
    );
    assert_eq!(codes(&findings(&body).1), ["schema_mismatch"]);
}

#[test]
fn the_builder_bot_schemas_match_what_generate_returns() {
    let failed = r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"code":{"enum":["unconfigured","transport","status","malformed","truncated","filtered","refusal","not-json","schema-mismatch"]},"rejection":{"type":"object","properties":{"key":{"type":"string","minLength":1,"maxLength":200},"message":{"type":"string","minLength":1,"maxLength":200}},"required":["key","message"],"additionalProperties":false}},"required":["code"],"additionalProperties":false}"#;
    let result = r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"content":{"type":"string"},"request_bytes":{"type":"integer","minimum":0},"tool_calls":{"type":"array","items":{"type":"object","properties":{"id":{"type":"string"},"name":{"type":"string"},"args":{"type":"object"}},"required":["id","name","args"],"additionalProperties":false}},"usage":{"type":"object","properties":{"input_tokens":{"type":"integer","minimum":0},"output_tokens":{"type":"integer","minimum":0},"reasoning_tokens":{"type":"integer","minimum":0},"total_tokens":{"type":"integer","minimum":0}},"additionalProperties":false}},"required":["content","tool_calls","request_bytes"],"additionalProperties":false}"#;
    let body = format!(
        r#"{}<transition event="generate.result" event:schema='{result}'/><transition event="generate.failed" event:schema='{failed}'/>"#,
        invoke(&format!("{MODEL}{PROMPT}"))
    );
    assert!(findings(&body).1.is_empty());
    // A closed result schema without the request's bytes and usage, or a
    // failed one without the rejection, rejects what the service returns.
    let closed_result = r#"{"type":"object","properties":{"content":{"type":"string"},"tool_calls":{"type":"array"}},"required":["content","tool_calls"],"additionalProperties":false}"#;
    let closed_failed = r#"{"type":"object","properties":{"code":{"type":"string"}},"required":["code"],"additionalProperties":false}"#;
    let body = format!(
        r#"{}<transition event="generate.result" event:schema='{closed_result}'/><transition event="generate.failed" event:schema='{closed_failed}'/>"#,
        invoke(&format!("{MODEL}{PROMPT}"))
    );
    let (_, found) = findings(&body);
    assert_eq!(codes(&found), ["schema_mismatch", "schema_mismatch"]);
    assert!(
        found[0].primary.1.contains("request_bytes") && found[1].primary.1.contains("rejection"),
        "{found:?}"
    );
}

#[test]
fn the_contract_is_the_service_s_own() {
    assert_eq!(
        REASONING_EFFORTS,
        floppy_service_generate::REASONING_EFFORTS
    );
    assert_eq!(SERVICE_URI, floppy_service_generate::SERVICE_URI);
    assert_eq!(TOOL_CHOICES, floppy_service_generate::TOOL_CHOICES);
    assert_eq!(
        TOOLS_REASONING_EFFORT,
        floppy_service_generate::TOOLS_REASONING_EFFORT
    );
    assert_eq!(
        MAX_COMPLETION_TOKENS,
        floppy_service_generate::MAX_COMPLETION_TOKENS
    );
    assert!((MAX_TEMPERATURE - floppy_service_generate::MAX_TEMPERATURE).abs() < f64::EPSILON);
    assert_eq!(MAX_DIMENSIONS, floppy_service_generate::MAX_DIMENSIONS);
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
    // No image is read here.
    let no_files = |_: &str, _: usize| Err(ImageReadFailure::Unreadable);
    // Every listed parameter is one the service takes: given twice, it is
    // refused as repeated, never as unknown.
    let text = DataValue::String("x".to_owned());
    for name in PARAMETERS {
        let error = parse_parameters([(name, &text), (name, &text)], no_files)
            .expect_err("a repeated parameter is refused");
        assert_eq!(error, "generate takes each parameter once", "{name}");
    }
    let accepted: Result<Generation, _> =
        parse_parameters([("model", &text), ("prompt", &text)], no_files);
    assert!(accepted.is_ok());
}

#[test]
fn an_embeddings_invocation_takes_only_a_model_and_dimensions() {
    const INPUT: &str = r#"<param name="input" expr="texts()"/>"#;
    let clean = invoke(&format!(
        r#"{MODEL}{INPUT}<param name="dimensions" expr="256"/>"#
    ));
    assert!(findings(&clean).1.is_empty());
    let (_, found) = findings(&invoke(&format!("{MODEL}{INPUT}{PROMPT}")));
    assert_eq!(codes(&found), ["missing_param"]);
    assert!(
        found[0].message.contains("`prompt`"),
        "{}",
        found[0].message
    );
    let (_, found) = findings(&invoke(&format!(
        r#"{MODEL}{PROMPT}<param name="dimensions" expr="256"/>"#
    )));
    assert_eq!(codes(&found), ["missing_param"]);
    for value in ["0", "4097", "2.5", "'256'"] {
        let (_, found) = findings(&invoke(&format!(
            r#"{MODEL}{INPUT}<param name="dimensions" expr="{value}"/>"#
        )));
        assert_eq!(codes(&found), ["invalid_value"], "{value}");
    }
    // What it returns is vectors: a transition expecting a text reply
    // does not take them.
    let (_, found) = findings(&format!(
        r#"{}<transition event="generate.result" event:schema='{{"type":"object","required":["content"]}}'/>"#,
        invoke(&format!("{MODEL}{INPUT}"))
    ));
    assert_eq!(codes(&found), ["schema_mismatch"]);
}
