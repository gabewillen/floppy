use std::collections::BTreeMap;

use scxml_reporting::{decode_canonical, encode_canonical};

use super::*;

const VALID: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a"><state id="a"><transition event="go" target="b"/></state><final id="b"/></scxml>"#;

/// A draft whose only error has a machine-applicable fix (comma-separated
/// event descriptors).
const COMMAS: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a"><state id="a"><transition event="go,stop" target="b"/></state><final id="b"/></scxml>"#;

/// A draft whose error has no fix (an unknown transition target).
const MISSING_TARGET: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a"><state id="a"><transition event="go" target="missing"/></state></scxml>"#;

/// A draft whose transition references an `event:schema` file.
const SCHEMA_REFERENCE: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event" version="1.0" initial="a"><state id="a"><transition event="go" target="b" event:schema="events.json#/$defs/Go"/></state><final id="b"/></scxml>"#;

const EVENTS: &str = r#"{"$defs":{"Go":{"type":"object","properties":{"n":{"type":"integer"}}}}}"#;

fn report(json: &str) -> serde_json::Value {
    serde_json::from_str(json).expect("the report is JSON")
}

fn codes(report: &serde_json::Value) -> Vec<String> {
    report["validator"]["diagnostics"]
        .as_array()
        .expect("the report lists diagnostics")
        .iter()
        .map(|diagnostic| {
            diagnostic["code"]
                .as_str()
                .expect("every diagnostic has a code")
                .to_owned()
        })
        .collect()
}

fn errors(report: &serde_json::Value) -> u64 {
    report["validator"]["counts"]["errors"]
        .as_u64()
        .expect("the report counts errors")
}

#[test]
fn the_namespace_uri_is_the_tagless_scxml_rs_path_of_its_name() {
    assert_eq!(
        NAMESPACE_URI.rsplit('/').next(),
        Some(NAMESPACE_NAME),
        "the URI's last segment names the namespace"
    );
    assert!(!NAMESPACE_URI.contains('@'), "no release tag: built in");
}

#[test]
fn the_package_binds_the_exact_uri_schema_digest_and_world() {
    let manifest = include_bytes!("../package/scx.extension.json");
    let schema = include_bytes!("../package/scx.xsd");
    let manifest = scxml_validator_core::ExtensionPackageManifest::from_json(manifest)
        .expect("the manifest is a valid extension package manifest");
    assert_eq!(manifest.namespace_uri, NAMESPACE_URI);
    assert_eq!(manifest.extension_artifact, "scx.virtualized.wasm");
    manifest
        .verify_schema(&scxml_validator_core::SchemaSource::new(
            "scx.xsd",
            schema.to_vec(),
        ))
        .expect("the schema digest, targetNamespace, and WIT identity verify");
}

#[test]
fn a_valid_draft_reports_no_error_in_the_canonical_report() {
    let validation =
        validate("draft.scx", VALID, EventSchemaResources::new()).expect("a valid draft reports");
    let value = report(&validation.report);
    assert_eq!(errors(&value), 0, "{}", validation.report);
    assert_eq!(value["version"], "scxml.report/v3");
    assert_eq!(value["kind"], "validator");
    assert_eq!(value["validator"]["source_name"], "draft.scx");
    assert_eq!(value["validator"]["source_text"], VALID);
    assert!(
        scxml_event_schema::is_canonical_json(&validation.report),
        "the report is canonical JSON (a data-model json-object)"
    );
    let canonical = encode_canonical(&validation.artifact).expect("the artifact encodes");
    assert_eq!(
        decode_canonical(&canonical).expect("the canonical bytes decode"),
        validation.artifact
    );
    assert_eq!(
        value,
        serde_json::from_slice::<serde_json::Value>(&canonical).expect("canonical bytes are JSON"),
        "the report is the canonical artifact's JSON value, members sorted"
    );
}

#[test]
fn a_broken_draft_reports_its_code_span_and_help() {
    let validation = validate("draft.scx", MISSING_TARGET, EventSchemaResources::new())
        .expect("a broken draft reports");
    let value = report(&validation.report);
    assert_eq!(errors(&value), 1);
    assert_eq!(codes(&value), ["SCXML005"]);
    let diagnostic = &value["validator"]["diagnostics"][0];
    assert_eq!(diagnostic["severity"], "error");
    let start = usize::try_from(
        diagnostic["primary"]["span"]["start"]
            .as_u64()
            .expect("a span start"),
    )
    .expect("fits");
    let end = usize::try_from(
        diagnostic["primary"]["span"]["end"]
            .as_u64()
            .expect("a span end"),
    )
    .expect("fits");
    assert_eq!(
        &MISSING_TARGET[start..end],
        "missing",
        "the span is the target"
    );
    assert!(
        diagnostic["help"].is_string(),
        "the diagnostic carries help"
    );
}

#[test]
fn machine_applicable_fixes_are_applied_and_the_fixed_text_validates() {
    let validation =
        validate("draft.scx", COMMAS, EventSchemaResources::new()).expect("the draft reports");
    let value = report(&validation.report);
    assert_eq!(errors(&value), 1, "{}", validation.report);
    assert_eq!(
        value["validator"]["diagnostics"][0]["fixes"][0]["applicability"],
        "machine-applicable"
    );
    let fixed = apply_machine_applicable(&validation.artifact).expect("the fixes apply");
    assert_eq!(fixed, COMMAS.replace("go,stop", "go stop"));
    let again = validate("draft.scx", &fixed, EventSchemaResources::new())
        .expect("the fixed draft reports");
    assert_eq!(errors(&report(&again.report)), 0, "{}", again.report);
}

/// A builder draft that wrote `<cancel>`'s `sendid` on `<send>`: the
/// rename to `id` is machine-applicable, so the fix pass repairs it.
const SENDID_ON_SEND: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="red"><state id="red"><onentry><send event="timer.green" delay="3s" sendid="send_timer_green"/></onentry><transition event="timer.green" target="green"/></state><final id="green"/></scxml>"#;

#[test]
fn a_send_with_sendid_is_renamed_to_id_by_the_fix_pass() {
    let validation = validate("draft.scx", SENDID_ON_SEND, EventSchemaResources::new())
        .expect("the draft reports");
    let value = report(&validation.report);
    assert_eq!(codes(&value), ["XSD-cvc-complex-type.3.2.2"]);
    assert_eq!(
        value["validator"]["diagnostics"][0]["fixes"][0]["applicability"],
        "machine-applicable"
    );
    let fixed = apply_machine_applicable(&validation.artifact).expect("the fixes apply");
    assert_eq!(fixed, SENDID_ON_SEND.replace(" sendid=", " id="));
    let again = validate("draft.scx", &fixed, EventSchemaResources::new())
        .expect("the fixed draft reports");
    assert_eq!(errors(&report(&again.report)), 0, "{}", again.report);
}

#[test]
fn a_draft_without_machine_applicable_fixes_is_unchanged() {
    let validation = validate("draft.scx", MISSING_TARGET, EventSchemaResources::new())
        .expect("the draft reports");
    assert_eq!(
        apply_machine_applicable(&validation.artifact).expect("nothing to apply"),
        MISSING_TARGET
    );
}

#[test]
fn an_unsupplied_event_schema_file_is_reported_not_read() {
    let validation = validate("draft.scx", SCHEMA_REFERENCE, EventSchemaResources::new())
        .expect("the draft reports");
    assert_eq!(codes(&report(&validation.report)), ["EVENT004"]);
}

#[test]
fn a_supplied_event_schema_file_resolves() {
    let resources = BTreeMap::from([("events.json".to_owned(), EVENTS.as_bytes().to_vec())]);
    let validation = validate("draft.scx", SCHEMA_REFERENCE, resources).expect("the draft reports");
    assert_eq!(
        errors(&report(&validation.report)),
        0,
        "{}",
        validation.report
    );
}

#[test]
fn malformed_xml_is_a_report_not_a_failure() {
    let validation = validate("draft.scx", "<scxml", EventSchemaResources::new())
        .expect("malformed XML reports");
    let value = report(&validation.report);
    assert!(errors(&value) >= 1);
    assert!(codes(&value).iter().all(|code| code.starts_with("XML")));
}

// ---- the element, through a recording host ----

#[derive(Debug, Default)]
struct MockHost {
    values: BTreeMap<String, Result<Value, ()>>,
    assigned: Vec<(String, String)>,
    refuse_assign: bool,
    fail_host: bool,
}

impl MockHost {
    fn with(mut self, expression: &str, value: Value) -> Self {
        self.values.insert(expression.to_owned(), Ok(value));
        self
    }
}

impl ScxmlHost for MockHost {
    type Error = &'static str;

    fn evaluate(&mut self, expression: &str) -> Result<Result<Value, ()>, &'static str> {
        if self.fail_host {
            return Err("host failure");
        }
        Ok(self.values.get(expression).cloned().unwrap_or(Err(())))
    }

    fn assign_json_object(
        &mut self,
        location: &str,
        json: &str,
    ) -> Result<Result<(), ()>, &'static str> {
        self.assign_text(location, json)
    }

    fn assign_text(&mut self, location: &str, value: &str) -> Result<Result<(), ()>, &'static str> {
        if self.refuse_assign {
            return Ok(Err(()));
        }
        self.assigned.push((location.to_owned(), value.to_owned()));
        Ok(Ok(()))
    }
}

fn attribute(local_name: &str, value: &str) -> Attribute {
    Attribute {
        qualified: false,
        local_name: local_name.to_owned(),
        value: value.to_owned(),
    }
}

fn child(local_name: &str, attributes: Vec<Attribute>, text: &str) -> Vec<Content> {
    vec![
        Content::Start {
            namespace_uri: Some(NAMESPACE_URI.to_owned()),
            local_name: local_name.to_owned(),
            attributes,
        },
        Content::Text(text.to_owned()),
        Content::End,
    ]
}

fn run(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut MockHost,
) -> Result<Executed, Failure<&'static str>> {
    execute("validate", attributes, content, host)
}

fn failure(result: Result<Executed, Failure<&'static str>>) -> &'static str {
    match result {
        Err(Failure::Execution(detail)) => detail,
        other => panic!("expected an execution failure, got {other:?}"),
    }
}

#[test]
fn other_local_names_are_not_handled() {
    let mut host = MockHost::default();
    assert_eq!(
        execute("compile", &[], &[], &mut host),
        Ok(Executed::NotHandled)
    );
    assert!(host.assigned.is_empty());
}

#[test]
fn expr_text_is_validated_and_the_report_assigned_to_location() {
    let mut host = MockHost::default().with("draft", Value::Text(MISSING_TARGET.to_owned()));
    let result = run(
        &[attribute("location", "report"), attribute("expr", "draft")],
        &[],
        &mut host,
    );
    assert_eq!(result, Ok(Executed::Handled));
    assert_eq!(host.assigned.len(), 1);
    let (location, json) = &host.assigned[0];
    assert_eq!(location, "report");
    let value = report(json);
    assert_eq!(codes(&value), ["SCXML005"]);
    assert_eq!(value["validator"]["source_name"], DEFAULT_SOURCE_NAME);
}

#[test]
fn content_text_is_trimmed_and_named() {
    let mut host = MockHost::default();
    let content = child("content", Vec::new(), &format!("\n    {VALID}\n  "));
    let result = run(
        &[
            attribute("location", "report"),
            attribute("name", "bot.scxml"),
        ],
        &content,
        &mut host,
    );
    assert_eq!(result, Ok(Executed::Handled));
    let value = report(&host.assigned[0].1);
    assert_eq!(errors(&value), 0);
    assert_eq!(value["validator"]["source_text"], VALID);
    assert_eq!(value["validator"]["source_name"], "bot.scxml");
}

#[test]
fn applyfixes_assigns_the_report_then_the_fixed_text() {
    let mut host = MockHost::default().with("draft", Value::Text(COMMAS.to_owned()));
    let result = run(
        &[
            attribute("location", "report"),
            attribute("expr", "draft"),
            attribute("applyfixes", "machine-applicable"),
            attribute("fixedlocation", "fixed"),
        ],
        &[],
        &mut host,
    );
    assert_eq!(result, Ok(Executed::Handled));
    assert_eq!(host.assigned.len(), 2);
    assert_eq!(host.assigned[0].0, "report");
    assert_eq!(
        errors(&report(&host.assigned[0].1)),
        1,
        "the original's report"
    );
    assert_eq!(
        host.assigned[1],
        ("fixed".to_owned(), COMMAS.replace("go,stop", "go stop"))
    );
}

#[test]
fn event_schemas_are_supplied_as_text_or_by_expression() {
    let draft = Value::Text(SCHEMA_REFERENCE.to_owned());
    for (schema, mut host) in [
        (
            child(
                "eventschema",
                vec![attribute("uri", "events.json")],
                &format!("\n  {EVENTS}\n"),
            ),
            MockHost::default().with("draft", draft.clone()),
        ),
        (
            child(
                "eventschema",
                vec![
                    attribute("uri", "events.json"),
                    attribute("expr", "schemas"),
                ],
                "",
            ),
            MockHost::default()
                .with("draft", draft.clone())
                .with("schemas", Value::Json(EVENTS.to_owned())),
        ),
    ] {
        let result = run(
            &[attribute("location", "report"), attribute("expr", "draft")],
            &schema,
            &mut host,
        );
        assert_eq!(result, Ok(Executed::Handled));
        let value = report(&host.assigned[0].1);
        assert_eq!(errors(&value), 0, "{value}");
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one table of every malformed element"
)]
fn malformed_elements_fail_with_static_details() {
    let draft = || MockHost::default().with("draft", Value::Text(VALID.to_owned()));
    let location = attribute("location", "report");
    let expr = attribute("expr", "draft");
    let content = child("content", Vec::new(), VALID);
    let cases: Vec<(Vec<Attribute>, Vec<Content>, &str)> = vec![
        (
            vec![
                location.clone(),
                expr.clone(),
                attribute("profile", "draft"),
            ],
            Vec::new(),
            "scx:validate has an attribute this package version does not define",
        ),
        (
            vec![expr.clone()],
            Vec::new(),
            "scx:validate requires a non-empty location",
        ),
        (
            vec![attribute("location", "  "), expr.clone()],
            Vec::new(),
            "scx:validate requires a non-empty location",
        ),
        (
            vec![location.clone()],
            Vec::new(),
            "scx:validate takes exactly one of expr or scx:content",
        ),
        (
            vec![location.clone(), expr.clone()],
            content.clone(),
            "scx:validate takes exactly one of expr or scx:content",
        ),
        (
            vec![location.clone(), expr.clone(), attribute("srcexpr", "x")],
            Vec::new(),
            "scx:validate has an attribute this package version does not define",
        ),
        (
            vec![
                location.clone(),
                expr.clone(),
                Attribute {
                    qualified: true,
                    local_name: "apikey".into(),
                    value: "KEY".into(),
                },
            ],
            Vec::new(),
            "scx:validate takes no namespaced attributes",
        ),
        (
            vec![
                location.clone(),
                expr.clone(),
                attribute("applyfixes", "all"),
            ],
            Vec::new(),
            "scx:validate applyfixes accepts only machine-applicable",
        ),
        (
            vec![
                location.clone(),
                expr.clone(),
                attribute("applyfixes", "machine-applicable"),
            ],
            Vec::new(),
            "scx:validate applyfixes requires a non-empty fixedlocation",
        ),
        (
            vec![
                location.clone(),
                expr.clone(),
                attribute("fixedlocation", "f"),
            ],
            Vec::new(),
            "scx:validate fixedlocation requires applyfixes",
        ),
        (
            vec![location.clone(), expr.clone(), attribute("name", "")],
            Vec::new(),
            "scx:validate name must be non-empty and within the name bound",
        ),
        (
            vec![location.clone()],
            [content.clone(), content.clone()].concat(),
            "scx:validate holds at most one scx:content",
        ),
        (
            vec![location.clone()],
            vec![Content::Text("<scxml/>".into())],
            "scx:validate holds only scx:content and scx:eventschema children",
        ),
        (
            vec![location.clone()],
            child("script", Vec::new(), ""),
            "scx:validate holds only scx:content and scx:eventschema children",
        ),
        (
            vec![location.clone()],
            vec![
                Content::Start {
                    namespace_uri: Some(NAMESPACE_URI.into()),
                    local_name: "content".into(),
                    attributes: Vec::new(),
                },
                Content::Start {
                    namespace_uri: Some("http://www.w3.org/2005/07/scxml".into()),
                    local_name: "scxml".into(),
                    attributes: Vec::new(),
                },
                Content::End,
                Content::End,
            ],
            "scx:content and scx:eventschema hold text only, not elements",
        ),
        (
            vec![location.clone(), expr.clone()],
            child("eventschema", Vec::new(), EVENTS),
            "scx:eventschema requires a non-empty uri",
        ),
        (
            vec![location.clone(), expr.clone()],
            child(
                "eventschema",
                vec![attribute("uri", "e.json"), attribute("expr", "x")],
                EVENTS,
            ),
            "scx:eventschema holds exactly one of expr or text",
        ),
        (
            vec![location.clone(), expr.clone()],
            child("eventschema", vec![attribute("uri", "e.json")], "  "),
            "scx:eventschema holds exactly one of expr or text",
        ),
        (
            vec![location.clone(), expr.clone()],
            [
                child("eventschema", vec![attribute("uri", "e.json")], EVENTS),
                child("eventschema", vec![attribute("uri", "e.json")], EVENTS),
            ]
            .concat(),
            "scx:eventschema names one uri twice",
        ),
    ];
    for (attributes, content, expected) in cases {
        let mut host = draft();
        assert_eq!(
            failure(run(&attributes, &content, &mut host)),
            expected,
            "{attributes:?} {content:?}"
        );
        assert!(host.assigned.is_empty(), "nothing is assigned on failure");
    }
}

#[test]
fn evaluation_and_assignment_failures_are_execution_errors() {
    let attributes = [attribute("location", "report"), attribute("expr", "draft")];
    assert_eq!(
        failure(run(&attributes, &[], &mut MockHost::default())),
        "scx:validate expr could not be evaluated"
    );
    assert_eq!(
        failure(run(
            &attributes,
            &[],
            &mut MockHost::default().with("draft", Value::Other)
        )),
        "scx:validate expr must evaluate to a string"
    );
    assert_eq!(
        failure(run(
            &attributes,
            &[],
            &mut MockHost::default().with("draft", Value::Json("{}".into()))
        )),
        "scx:validate expr must evaluate to a string"
    );
    let mut refusing = MockHost {
        refuse_assign: true,
        ..MockHost::default().with("draft", Value::Text(VALID.to_owned()))
    };
    assert_eq!(
        failure(run(&attributes, &[], &mut refusing)),
        "scx:validate could not assign the report to location"
    );
}

#[test]
fn host_failures_pass_through_unchanged() {
    let mut host = MockHost {
        fail_host: true,
        ..MockHost::default()
    };
    assert_eq!(
        run(
            &[attribute("location", "report"), attribute("expr", "draft")],
            &[],
            &mut host
        ),
        Err(Failure::Host("host failure"))
    );
}
