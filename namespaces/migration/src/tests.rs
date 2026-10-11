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

impl MigrationHost for FakeHost {
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

/// A data value of a JSON value (canonical text for arrays and objects).
fn data(value: &Json) -> Data {
    match value {
        Json::Null => Data::Null,
        Json::Bool(value) => Data::Bool(*value),
        Json::Number(value) => Data::Number(
            scxml_types::FiniteNumber::new(value.as_f64().expect("a finite number"))
                .expect("a finite number"),
        ),
        Json::String(text) => Data::String(text.clone()),
        Json::Array(_) => Data::JsonArray(value.to_string()),
        Json::Object(_) => Data::JsonObject(value.to_string()),
    }
}

/// The original's two revisions, with one marker statement each way.
fn reference_revisions() -> Json {
    json!([
        {"revision": "0002", "down_revision": "0001", "upgrade": [{"sql": "up 2"}], "downgrade": [{"sql": "down 2"}]},
        {"revision": "0001", "down_revision": null, "upgrade": [{"sql": "up 1"}], "downgrade": [{"sql": "down 1"}]}
    ])
}

fn plan_of(revisions: &Json, current: Option<&str>, target: &str) -> Result<Json, &'static str> {
    plan(revisions.as_array().expect("an array"), current, target).map(Json::Object)
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
        "../package/migration.extension.json"
    ))
    .expect("the manifest is a valid extension package manifest");
    assert_eq!(manifest.namespace_uri, NAMESPACE_URI);
    assert_eq!(
        manifest.extension_artifact,
        "floppy_migration.virtualized.wasm"
    );
    manifest
        .verify_schema(&scxml_validator_core::SchemaSource::new(
            "migration.xsd",
            include_bytes!("../package/migration.xsd").to_vec(),
        ))
        .expect("the schema digest, targetNamespace, and WIT identity verify");
}

#[test]
fn upgrading_from_base_to_head_runs_every_upgrade_oldest_first() {
    assert_eq!(
        plan_of(&reference_revisions(), None, "head"),
        Ok(json!({"from": null, "to": "0002", "steps": [
            {"revision": "0001", "direction": "upgrade", "statements": [{"sql": "up 1"}]},
            {"revision": "0002", "direction": "upgrade", "statements": [{"sql": "up 2"}]}
        ]}))
    );
}

#[test]
fn at_the_target_there_is_nothing_to_do() {
    assert_eq!(
        plan_of(&reference_revisions(), Some("0002"), "head"),
        Ok(json!({"from": "0002", "to": "0002", "steps": []}))
    );
    assert_eq!(
        plan_of(&json!([]), None, "head"),
        Ok(json!({"from": null, "to": null, "steps": []}))
    );
}

#[test]
fn downgrading_runs_the_downgrades_newest_first() {
    assert_eq!(
        plan_of(&reference_revisions(), Some("0002"), "base"),
        Ok(json!({"from": "0002", "to": null, "steps": [
            {"revision": "0002", "direction": "downgrade", "statements": [{"sql": "down 2"}]},
            {"revision": "0001", "direction": "downgrade", "statements": [{"sql": "down 1"}]}
        ]}))
    );
    assert_eq!(
        plan_of(&reference_revisions(), Some("0002"), "-1"),
        Ok(json!({"from": "0002", "to": "0001", "steps": [
            {"revision": "0002", "direction": "downgrade", "statements": [{"sql": "down 2"}]}
        ]}))
    );
}

#[test]
fn a_target_may_be_a_revision_or_a_relative_step() {
    assert_eq!(
        plan_of(&reference_revisions(), None, "0001").map(|plan| plan["to"].clone()),
        Ok(json!("0001"))
    );
    assert_eq!(
        plan_of(&reference_revisions(), Some("0001"), "+1").map(|plan| plan["steps"].as_array().map(Vec::len)),
        Ok(Some(1))
    );
    assert_eq!(
        plan_of(&reference_revisions(), Some("0001"), "+2"),
        Err("migration:plan target steps past head")
    );
    assert_eq!(
        plan_of(&reference_revisions(), None, "-1"),
        Err("migration:plan target steps past base")
    );
    assert_eq!(
        plan_of(&reference_revisions(), None, "0003"),
        Err("migration:plan target is not a revision of the history")
    );
    assert_eq!(
        plan_of(&reference_revisions(), Some("0009"), "head"),
        Err("migration:plan current is not a revision of the history")
    );
}

#[test]
fn only_a_linear_history_is_planned() {
    let up = json!([]);
    let revision = |name: &str, down: Json| json!({"revision": name, "down_revision": down, "upgrade": up, "downgrade": up});
    for (revisions, detail) in [
        (
            json!([
                revision("a", Json::Null),
                revision("b", json!("a")),
                revision("c", json!("a"))
            ]),
            "migration:plan history has multiple heads",
        ),
        (
            json!([revision("a", Json::Null), revision("a", json!("a"))]),
            "migration:plan names a revision twice",
        ),
        (
            json!([
                revision("a", Json::Null),
                revision("b", json!("c")),
                revision("c", json!("b"))
            ]),
            "migration:plan history is not one line from its first revision",
        ),
        (
            json!([revision("a", Json::Null), revision("b", json!("x"))]),
            "migration:plan history is not one line from its first revision",
        ),
        (
            json!([{"revision": "a", "down_revision": null, "upgrade": "x", "downgrade": []}]),
            "migration:plan upgrade and downgrade must be arrays of statements",
        ),
    ] {
        assert_eq!(plan_of(&revisions, None, "head"), Err(detail));
    }
}

#[test]
fn the_element_assigns_the_plan_to_its_location() {
    let mut host = FakeHost {
        values: BTreeMap::from([
            ("revisions".to_owned(), data(&reference_revisions())),
            ("current".to_owned(), Data::String("0001".into())),
            ("null".to_owned(), Data::Null),
        ]),
        ..FakeHost::default()
    };
    assert_eq!(
        execute(
            "plan",
            &[
                attr("location", "plan"),
                attr("revisions", "revisions"),
                attr("current", "current")
            ],
            &[],
            &mut host
        ),
        Ok(Executed::Handled)
    );
    let [(location, Data::JsonObject(text))] = host.assigned.as_slice() else {
        panic!("one object assigned: {:?}", host.assigned);
    };
    assert_eq!(location, "plan");
    assert_eq!(
        serde_json::from_str::<Json>(text).expect("JSON"),
        json!({"from": "0001", "to": "0002", "steps": [
            {"revision": "0002", "direction": "upgrade", "statements": [{"sql": "up 2"}]}
        ]})
    );
}

#[test]
fn malformed_elements_raise_error_execution() {
    let mut host = FakeHost {
        values: BTreeMap::from([
            ("revisions".to_owned(), data(&reference_revisions())),
            ("number".to_owned(), Data::String(String::new())),
        ]),
        ..FakeHost::default()
    };
    assert_eq!(
        execution(execute(
            "plan",
            &[attr("revisions", "revisions"), attr("current", "null")],
            &[],
            &mut host
        )),
        "migration:plan requires a location"
    );
    assert_eq!(
        execution(execute(
            "plan",
            &[
                attr("location", "p"),
                attr("revisions", "revisions"),
                attr("current", "missing")
            ],
            &[],
            &mut host
        )),
        "migration:plan current did not evaluate"
    );
    assert_eq!(
        execution(execute(
            "plan",
            &[
                attr("location", "p"),
                attr("revisions", "revisions"),
                attr("current", "number")
            ],
            &[],
            &mut host
        )),
        "migration:plan current is not a revision or null"
    );
    assert_eq!(
        execution(execute(
            "plan",
            &[
                attr("location", "p"),
                attr("revisions", "revisions"),
                attr("current", "x"),
                attr("other", "y")
            ],
            &[],
            &mut host
        )),
        "migration:plan takes only the location, revisions, current and target attributes"
    );
    assert_eq!(
        execute("compact", &[], &[], &mut host),
        Ok(Executed::NotHandled)
    );
    assert!(host.assigned.is_empty());
}
