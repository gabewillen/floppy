use std::collections::BTreeMap;

use serde_json::json;

use super::*;

/// A deterministic fake host: expressions read from a table (anything else
/// fails to evaluate), and assignments are recorded.
#[derive(Default)]
struct FakeHost {
    values: BTreeMap<String, Data>,
    assigned: Vec<(String, Data)>,
    fail_assign: bool,
}

impl BehaviorHost for FakeHost {
    type Error = &'static str;

    fn read(&mut self, expression: &str) -> Result<Result<Data, ()>, &'static str> {
        if expression == "fatal" {
            return Err("fatal");
        }
        Ok(self.values.get(expression).cloned().ok_or(()))
    }

    fn assign(&mut self, location: &str, value: &Data) -> Result<Result<(), ()>, &'static str> {
        if self.fail_assign {
            return Ok(Err(()));
        }
        self.assigned.push((location.to_owned(), value.clone()));
        Ok(Ok(()))
    }
}

fn attr(local_name: &str, value: &str) -> Attribute {
    Attribute {
        namespace_uri: None,
        local_name: local_name.into(),
        value: value.into(),
    }
}

fn execution(result: Result<Executed, Failure<&'static str>>) -> &'static str {
    match result {
        Err(Failure::Execution(detail)) => detail,
        other => panic!("expected error.execution, got {other:?}"),
    }
}

/// The data value of a JSON value (JSON text for arrays and objects).
fn data(value: &Json) -> Data {
    match value {
        Json::Null => Data::Null,
        Json::Bool(value) => Data::Bool(*value),
        Json::Number(value) => Data::Number(
            scxml_types::FiniteNumber::new(value.as_f64().expect("a number")).expect("finite"),
        ),
        Json::String(text) => Data::String(text.clone()),
        Json::Array(_) => Data::JsonArray(value.to_string()),
        Json::Object(_) => Data::JsonObject(value.to_string()),
    }
}

/// The JSON of an assigned value.
fn json(value: &Data) -> Json {
    json_of(value).expect("the assigned value is JSON")
}

/// Runs one element over `values` and returns what it assigned, by location.
fn run(
    local_name: &str,
    attributes: &[Attribute],
    values: &[(&str, Json)],
) -> Result<BTreeMap<String, Json>, &'static str> {
    let mut host = FakeHost {
        values: values
            .iter()
            .map(|(name, value)| ((*name).to_owned(), data(value)))
            .collect(),
        ..FakeHost::default()
    };
    match execute(local_name, attributes, &[], &mut host) {
        Ok(Executed::Handled) => Ok(host
            .assigned
            .iter()
            .map(|(location, value)| (location.clone(), json(value)))
            .collect()),
        Err(Failure::Execution(detail)) => {
            assert!(host.assigned.is_empty(), "a failure assigns nothing");
            Err(detail)
        }
        other => panic!("unexpected {other:?}"),
    }
}

/// Every `testdata/<name>.json` case.
fn cases(text: &str) -> Vec<Json> {
    match serde_json::from_str(text).expect("the cases are JSON") {
        Json::Array(cases) => cases,
        other => panic!("not an array of cases: {other}"),
    }
}

#[test]
fn the_namespace_uri_is_the_tagless_repository_path_of_its_name() {
    assert_eq!(
        NAMESPACE_URI,
        format!("https://github.com/stateforward/floppy/namespaces/{NAMESPACE_NAME}")
    );
}

#[test]
fn the_package_binds_the_exact_uri_and_schema_and_declares_no_io() {
    let manifest = scxml_validator_core::ExtensionPackageManifest::from_json(include_bytes!(
        "../package/behavior.extension.json"
    ))
    .expect("the manifest is a valid extension package manifest");
    assert_eq!(manifest.namespace_uri, NAMESPACE_URI);
    assert_eq!(
        manifest.extension_artifact,
        "floppy_behavior.virtualized.wasm"
    );
    manifest
        .verify_schema(&scxml_validator_core::SchemaSource::new(
            "behavior.xsd",
            include_bytes!("../package/behavior.xsd").to_vec(),
        ))
        .expect("the schema digest, targetNamespace, and WIT identity verify");
}

#[test]
fn other_local_names_are_not_handled() {
    let mut host = FakeHost::default();
    assert_eq!(
        execute("next_after", &[attr("location", "x")], &[], &mut host),
        Ok(Executed::NotHandled)
    );
    assert!(host.assigned.is_empty());
}

/// The instances `assembledBehaviors()` in `abilities/reflection/reflection.js`
/// built, before this element replaced it (`testdata/instances_from_behavior_results.json`,
/// written by that JavaScript), are built member for member.
#[test]
fn instances_are_what_the_javascript_it_replaces_built() {
    for case in cases(include_str!(
        "testdata/instances_from_behavior_results.json"
    )) {
        let assigned = run(
            "instances_from_behavior_results",
            &[
                attr("location", "behaviors"),
                attr("behaviors", "b"),
                attr("triggers", "t"),
            ],
            &[
                ("b", case["behaviors"].clone()),
                ("t", case["triggers"].clone()),
            ],
        )
        .expect("the case runs");
        assert_eq!(assigned["behaviors"], case["instances"], "{}", case["name"]);
    }
}

#[test]
fn malformed_results_raise_error_execution_and_assign_nothing() {
    let rows = json!({"rows": []});
    for (behaviors, triggers, detail) in [
        (
            json!({"rowcount": 0}),
            rows.clone(),
            "behavior:instances_from_behavior_results behaviors is not a result with rows",
        ),
        (
            rows.clone(),
            json!({"rows": [{"values": []}]}),
            "behavior:instances_from_behavior_results triggers is not a result with rows",
        ),
        (
            json!({"rows": [{"columns": ["name"], "values": ["A"]}]}),
            rows.clone(),
            "behavior:instances_from_behavior_results has a behavior row without examples",
        ),
        (
            json!({"rows": [{"columns": ["name", "examples"], "values": ["A", "[1,"]}]}),
            rows.clone(),
            "behavior:instances_from_behavior_results has examples that are not JSON",
        ),
    ] {
        assert_eq!(
            run(
                "instances_from_behavior_results",
                &[
                    attr("location", "i"),
                    attr("behaviors", "b"),
                    attr("triggers", "t")
                ],
                &[("b", behaviors), ("t", triggers)],
            ),
            Err(detail)
        );
    }
    assert_eq!(
        run(
            "instances_from_behavior_results",
            &[attr("location", "i"), attr("behaviors", "b")],
            &[("b", rows)],
        ),
        Err("behavior:instances_from_behavior_results requires triggers")
    );
}

/// The accepting statements `behaviorRow*()` and `replaceBehavior*()` in
/// `abilities/reflection/reflection.js` built, before this element replaced
/// them (`testdata/replace_behavior_clauses.json`, written by that
/// JavaScript), are built byte for byte from the instance the reflection
/// now hands it, but for the row's status: the JavaScript's insert wrote
/// `'ACTIVE'` into its SQL, and the element's takes it as a parameter (a
/// draft is replaced the same way).
#[test]
fn accepting_statements_are_what_the_javascript_it_replaces_built() {
    for case in cases(include_str!("testdata/replace_behavior_clauses.json")) {
        let mut expected = case["statements"].clone();
        let insert = &mut expected[2];
        assert_eq!(insert["sql"], "OLD INSERT");
        insert["sql"] = Json::String(INSERT_BEHAVIOR.into());
        insert["parameters"]
            .as_array_mut()
            .expect("the row's parameters")
            .insert(5, Json::String("ACTIVE".into()));
        let assigned = run(
            "replace_behavior_clauses",
            &[attr("location", "s"), attr("instance", "i")],
            &[("i", case["instance"].clone())],
        )
        .expect("the case runs");
        assert_eq!(assigned["s"], expected, "{}", case["name"]);
    }
}

/// The original's `_draft` stub, stored as `store_behavior` stores it: its
/// defaults fill what a draft does not name.
#[test]
fn a_draft_is_replaced_with_the_instance_defaults() {
    let assigned = run(
        "replace_behavior_clauses",
        &[attr("location", "s"), attr("instance", "i")],
        &[(
            "i",
            json!({"name": "Morning", "source": "", "description": "", "status": "DRAFT", "triggers": ["a", "b", "a"]}),
        )],
    )
    .expect("the draft runs");
    assert_eq!(
        assigned["s"],
        json!([
            {"sql": DELETE_TRIGGERS, "parameters": ["Morning"]},
            {"sql": DELETE_BEHAVIOR, "parameters": ["Morning"]},
            {"sql": INSERT_BEHAVIOR, "parameters": ["Morning", "", "", "[]", "turn", "DRAFT", "0", "", "0", ""]},
            {"sql": INSERT_TRIGGER, "parameters": ["Morning", "a"]},
            {"sql": INSERT_TRIGGER, "parameters": ["Morning", "b"]},
        ])
    );
}

#[test]
fn malformed_instances_raise_error_execution_and_assign_nothing() {
    for (instance, detail) in [
        (
            json!([]),
            "behavior:replace_behavior_clauses instance is not an object",
        ),
        (
            json!({"source": ""}),
            "behavior:replace_behavior_clauses instance has no name",
        ),
        (
            json!({"name": "A"}),
            "behavior:replace_behavior_clauses instance source is not a string",
        ),
        (
            json!({"name": "A", "source": "", "triggers": [1]}),
            "behavior:replace_behavior_clauses instance triggers is not an array of strings",
        ),
        (
            json!({"name": "A", "source": "", "used_count": [1]}),
            "behavior:replace_behavior_clauses instance has a counter that is not a scalar",
        ),
        (
            json!({"name": "A", "source": "", "status": 1}),
            "behavior:replace_behavior_clauses instance status is not a string",
        ),
    ] {
        assert_eq!(
            run(
                "replace_behavior_clauses",
                &[attr("location", "s"), attr("instance", "i")],
                &[("i", instance)],
            ),
            Err(detail)
        );
    }
}

/// What `lifetimeErrors()`, `scheduleEvents()` and `draftLifetime()` in
/// `bots/builder/builder.js` found, before this element replaced them
/// (`testdata/lifetime_errors.json`, written by that JavaScript), is found
/// with an XML parser: the diagnostics byte for byte, the lifetime and the
/// schedules, under every combination of bounds.
#[test]
fn lifetime_errors_are_what_the_javascript_it_replaces_found() {
    for case in cases(include_str!("testdata/lifetime_errors.json")) {
        let mut values = vec![("text", case["scxml"].clone())];
        let mut attributes = vec![
            attr("location", "errors"),
            attr("scxml", "text"),
            attr("lifetimelocation", "lifetime"),
            attr("scheduleslocation", "schedules"),
        ];
        for bound in ["min_every", "turn_after_bound"] {
            if !case[bound].is_null() {
                values.push((bound, case[bound].clone()));
                attributes.push(attr(bound, bound));
            }
        }
        let assigned = run("lifetime_errors", &attributes, &values).expect("the case runs");
        let label = format!(
            "{} {} {}",
            case["name"], case["min_every"], case["turn_after_bound"]
        );
        assert_eq!(assigned["errors"], case["errors"], "{label}");
        assert_eq!(assigned["lifetime"], case["lifetime"], "{label}");
        assert_eq!(assigned["schedules"], case["schedules"], "{label}");
    }
}

/// The XML parser reads what the regular expressions misread: a tick named
/// only in a comment is no tick, an attribute's entities are resolved, and
/// a send outside the SCXML namespace is not one.
#[test]
fn the_lifetime_check_reads_the_document_as_xml() {
    let text = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:x="urn:x" version="1.0">
  <!-- sends bot.behavior.tick -->
  <state id="s"><onentry><send event="late" delay="&#x35;s"/><x:send event="other" delay="9s"/></onentry></state>
</scxml>"#;
    let checked = lifetime_errors(text, None, Some(1000.0));
    assert_eq!(checked.lifetime, "turn");
    assert_eq!(checked.schedules, Vec::<String>::new());
    assert_eq!(checked.errors.len(), 1);
    assert!(
        checked.errors[0]["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("after (the late delay of 5s)")),
        "{:?}",
        checked.errors
    );
    let unparsed = lifetime_errors("<scxml><send delay='9s'>", Some(60000.0), Some(1000.0));
    assert_eq!(
        unparsed,
        Lifetime {
            lifetime: "turn",
            errors: Vec::new(),
            schedules: Vec::new()
        }
    );
}

#[test]
fn malformed_lifetime_checks_raise_error_execution_and_assign_nothing() {
    assert_eq!(
        run(
            "lifetime_errors",
            &[attr("location", "e"), attr("scxml", "text")],
            &[("text", json!(1))],
        ),
        Err("behavior:lifetime_errors scxml is not a string")
    );
    assert_eq!(
        run(
            "lifetime_errors",
            &[
                attr("location", "e"),
                attr("scxml", "text"),
                attr("min_every", "m")
            ],
            &[("text", json!("<scxml/>")), ("m", json!("60s"))],
        ),
        Err("behavior:lifetime_errors a bound is not a number or null")
    );
    assert_eq!(
        run("lifetime_errors", &[attr("location", "e")], &[]),
        Err("behavior:lifetime_errors requires an scxml")
    );
}

/// The messages `applyErrors()` in `bots/builder/builder.js` wrote for the
/// forced tick, before this element replaced it (`testdata/verify_tick.json`,
/// written by that JavaScript), are written byte for byte.
#[test]
fn verify_tick_writes_what_the_javascript_it_replaces_wrote() {
    for case in cases(include_str!("testdata/verify_tick.json")) {
        let assigned = run(
            "verify_tick",
            &[attr("location", "apply"), attr("ticks", "ticks")],
            &[("ticks", case["ticks"].clone())],
        )
        .expect("the case runs");
        assert_eq!(assigned["apply"], case["messages"], "{}", case["name"]);
    }
}

#[test]
fn malformed_tick_checks_raise_error_execution_and_assign_nothing() {
    assert_eq!(
        run(
            "verify_tick",
            &[attr("location", "apply"), attr("ticks", "ticks")],
            &[("ticks", json!({}))],
        ),
        Err("behavior:verify_tick ticks is not an array")
    );
    assert_eq!(
        run(
            "verify_tick",
            &[
                attr("location", "apply"),
                attr("ticks", "ticks"),
                attr("tick", "t")
            ],
            &[("ticks", json!([]))],
        ),
        Err("behavior:verify_tick takes only the location and ticks attributes")
    );
    let mut host = FakeHost::default();
    assert_eq!(
        execution(execute(
            "verify_tick",
            &[attr("location", "a"), attr("ticks", "t")],
            &[Content::Text("x".into())],
            &mut host
        )),
        "behavior:verify_tick holds no content"
    );
}

#[test]
fn a_failed_assignment_or_host_failure_is_reported() {
    let mut host = FakeHost {
        values: BTreeMap::from([("ticks".to_owned(), data(&json!([])))]),
        fail_assign: true,
        ..FakeHost::default()
    };
    assert_eq!(
        execution(execute(
            "verify_tick",
            &[attr("location", "a"), attr("ticks", "ticks")],
            &[],
            &mut host
        )),
        "behavior:verify_tick could not assign the messages"
    );
    assert_eq!(
        execute(
            "replace_behavior_clauses",
            &[attr("location", "s"), attr("instance", "fatal")],
            &[],
            &mut FakeHost::default()
        ),
        Err(Failure::Host("fatal"))
    );
}
