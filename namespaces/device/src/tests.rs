use std::collections::BTreeMap;

use serde_json::json;

use super::*;

/// A deterministic fake host: expressions read from a table (anything else
/// fails to evaluate), each target's snapshot is scripted (an unscripted
/// target names no running session, one in `failing` fails to read), and
/// assignments and snapshot requests are recorded.
#[derive(Default)]
struct FakeHost {
    values: BTreeMap<String, Data>,
    snapshots: BTreeMap<String, String>,
    failing: Vec<String>,
    snapshot_requests: Vec<(Vec<Section>, String)>,
    assigned: Vec<(String, Data)>,
    fail_assign: bool,
}

impl DeviceHost for FakeHost {
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

    fn snapshot(
        &mut self,
        exclude: &[Section],
        target: &str,
    ) -> Result<SnapshotRead, &'static str> {
        self.snapshot_requests
            .push((exclude.to_vec(), target.to_owned()));
        if self.failing.iter().any(|failing| failing == target) {
            return Ok(SnapshotRead::Failed);
        }
        Ok(self
            .snapshots
            .get(target)
            .cloned()
            .map_or(SnapshotRead::NotRunning, SnapshotRead::Document))
    }
}

const LAMP: &str = "#_scxml_session.lamp";

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

/// One snapshot transition listing from `source` taking `events`.
fn transition(source: &str, events: &[&str]) -> Json {
    json!({"source": source, "index": 0, "events": events, "targets": [], "type": "external", "has_cond": false})
}

/// A checkpoint snapshot of the session `id` in the atomic states `atomic`
/// with the data model `data` and the transitions `transitions`.
fn snapshot_of(id: &str, atomic: &[&str], data: &[Json], transitions: &[Json]) -> String {
    json!({
        "version": "scxml.session-snapshot/v2",
        "sha256": "0".repeat(64),
        "options": {"exclude": ["invocations", "pending_sends", "previews", "queues", "system"], "trace_context": "strip"},
        "snapshot": {"components": [], "clock": {"now_ms": 0}, "sessions": [{
            "session_id": id,
            "name": "lamp",
            "datamodel_name": "ecmascript",
            "status": "running",
            "invoked_by": {"parent_session_id": "session", "invoke_id": "lamp"},
            "configuration": {"atomic": atomic, "full": atomic},
            "data_model": data,
            "transitions": transitions,
        }]},
    })
    .to_string()
}

/// A lamp that is `off`, `brightness` 0, taking `lamp.on`.
fn lamp_off() -> String {
    snapshot_of(
        "session.lamp",
        &["off"],
        &[json!({"name": "brightness", "value": {"type": "number", "value": "0"}})],
        &[transition("off", &["lamp.on"])],
    )
}

/// A host whose `target` expression is the lamp's address and whose lamp
/// snapshot is `snapshot`.
fn host_with(snapshot: String) -> FakeHost {
    let mut host = FakeHost::default();
    host.values
        .insert("target".into(), Data::String(LAMP.into()));
    host.snapshots.insert(LAMP.into(), snapshot);
    host
}

fn read_element(host: &mut FakeHost) -> Result<Executed, Failure<&'static str>> {
    execute(
        "snapshot",
        &[attr("location", "read"), attr("target", "target")],
        &[],
        host,
    )
}

/// The one assignment `host` received, as JSON.
fn assigned(host: &FakeHost) -> Json {
    assert_eq!(host.assigned.len(), 1, "{:?}", host.assigned);
    let (location, value) = &host.assigned[0];
    assert_eq!(location, "read");
    let Data::JsonObject(text) = value else {
        panic!("the read is an object, got {value:?}");
    };
    serde_json::from_str(text).expect("the read is JSON")
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
        "../package/device.extension.json"
    ))
    .expect("the manifest is a valid extension package manifest");
    assert_eq!(manifest.namespace_uri, NAMESPACE_URI);
    assert_eq!(
        manifest.extension_artifact,
        "floppy_device.virtualized.wasm"
    );
    manifest
        .verify_schema(&scxml_validator_core::SchemaSource::new(
            "device.xsd",
            include_bytes!("../package/device.xsd").to_vec(),
        ))
        .expect("the schema digest, targetNamespace, and WIT identity verify");
}

#[test]
fn a_read_of_a_session_in_one_state_assigns_its_id_state_attributes_and_events() {
    let mut host = host_with(lamp_off());
    assert_eq!(read_element(&mut host), Ok(Executed::Handled));
    assert_eq!(
        assigned(&host),
        json!({"id": "session.lamp", "state": "off", "attributes": {"brightness": 0}, "events": ["lamp.on"]})
    );
    // The target is read as the session the expression names, without the
    // sections the read does not use.
    assert_eq!(
        host.snapshot_requests,
        [(SNAPSHOT_EXCLUDE.to_vec(), LAMP.to_owned())]
    );
}

#[test]
fn a_parallel_configuration_names_its_atomic_states_space_separated() {
    let mut host = host_with(snapshot_of(
        "session.lamp",
        &["light.off", "fan.on"],
        &[],
        &[
            transition("light.off", &["lamp.on"]),
            transition("fan.on", &["fan.off"]),
        ],
    ));
    assert_eq!(read_element(&mut host), Ok(Executed::Handled));
    let read = assigned(&host);
    assert_eq!(read["state"], json!("light.off fan.on"));
    assert_eq!(read["events"], json!(["lamp.on", "fan.off"]));
}

#[test]
fn a_wildcard_descriptor_names_no_event() {
    let mut host = host_with(snapshot_of(
        "session.lamp",
        &["off"],
        &[],
        &[transition("off", &["*"]), transition("off", &["lamp.on"])],
    ));
    assert_eq!(read_element(&mut host), Ok(Executed::Handled));
    assert_eq!(assigned(&host)["events"], json!(["lamp.on"]));
}

#[test]
fn an_event_two_transitions_accept_is_listed_once() {
    let mut host = host_with(snapshot_of(
        "session.lamp",
        &["off"],
        &[],
        &[
            transition("off", &["lamp.on"]),
            transition("off", &["lamp.on", "lamp.toggle"]),
        ],
    ));
    assert_eq!(read_element(&mut host), Ok(Executed::Handled));
    assert_eq!(assigned(&host)["events"], json!(["lamp.on", "lamp.toggle"]));
}

#[test]
fn the_attributes_are_the_data_model_values_and_a_value_with_no_portable_form_is_left_out() {
    let mut host = host_with(snapshot_of(
        "session.lamp",
        &["off"],
        &[
            json!({"name": "label", "value": {"type": "text", "value": "desk"}}),
            json!({"name": "dimmable", "value": {"type": "boolean", "value": true}}),
            json!({"name": "color", "value": {"type": "json-object", "value": "{\"hue\":30,\"tint\":null}"}}),
            json!({"name": "scenes", "value": {"type": "json-array", "value": "[\"read\"]"}}),
            json!({"name": "owner", "value": {"type": "null"}}),
            json!({"name": "pending", "value": {"type": "undefined"}}),
            json!({"name": "late", "value": {"type": "absent"}}),
            json!({"name": "toggle", "value": {"type": "text", "typeof": "function", "text": "function toggle() {}"}}),
            json!({"name": "log", "value": {"type": "oversized", "bytes": 70_000}}),
        ],
        &[],
    ));
    assert_eq!(read_element(&mut host), Ok(Executed::Handled));
    assert_eq!(
        assigned(&host)["attributes"],
        json!({
            "label": "desk",
            "dimmable": true,
            "color": {"hue": 30, "tint": null},
            "scenes": ["read"],
            "owner": null,
        })
    );
}

#[test]
fn a_target_that_is_not_running_assigns_nothing_and_raises_nothing() {
    let mut host = host_with(lamp_off());
    host.values
        .insert("target".into(), Data::String("#_scxml_session.gone".into()));
    assert_eq!(read_element(&mut host), Ok(Executed::Handled));
    assert!(host.assigned.is_empty());
}

#[test]
fn a_read_the_host_fails_is_an_execution_error_and_assigns_nothing() {
    let mut host = host_with(lamp_off());
    host.failing.push(LAMP.to_owned());
    assert_eq!(
        execution(read_element(&mut host)),
        "device:snapshot cannot read the target session"
    );
    assert!(host.assigned.is_empty());
}

#[test]
fn a_target_that_is_not_a_string_is_an_execution_error() {
    let mut host = host_with(lamp_off());
    host.values.insert("target".into(), Data::Null);
    assert_eq!(
        execution(read_element(&mut host)),
        "device:snapshot's target is not a string"
    );
    assert!(host.snapshot_requests.is_empty());
}

#[test]
fn a_malformed_snapshot_is_an_execution_error() {
    let mut host = host_with(r#"{"snapshot":{"sessions":[{}]}}"#.to_owned());
    assert_eq!(
        execution(read_element(&mut host)),
        "the session snapshot is malformed"
    );
    assert!(host.assigned.is_empty());
}

#[test]
fn a_failed_assignment_is_an_execution_error() {
    let mut host = host_with(lamp_off());
    host.fail_assign = true;
    assert_eq!(
        execution(read_element(&mut host)),
        "device:snapshot cannot assign its location"
    );
}

#[test]
fn a_malformed_element_is_an_execution_error() {
    let mut host = host_with(lamp_off());
    assert_eq!(
        execution(execute(
            "snapshot",
            &[attr("target", "target")],
            &[],
            &mut host
        )),
        "device:snapshot needs a location"
    );
    assert_eq!(
        execution(execute(
            "snapshot",
            &[attr("location", "read")],
            &[],
            &mut host
        )),
        "device:snapshot needs a target"
    );
    assert_eq!(
        execution(execute(
            "snapshot",
            &[
                attr("location", "read"),
                attr("target", "target"),
                attr("exclude", "x")
            ],
            &[],
            &mut host
        )),
        "device:snapshot takes only the location and target attributes"
    );
    assert_eq!(
        execution(execute(
            "snapshot",
            &[
                attr("location", "read"),
                attr("target", "target"),
                attr("location", "again")
            ],
            &[],
            &mut host
        )),
        "device:snapshot repeats an attribute"
    );
    assert_eq!(
        execution(execute(
            "snapshot",
            &[attr("location", "read"), attr("target", "target")],
            &[Content::Start, Content::End],
            &mut host
        )),
        "device:snapshot takes no content"
    );
    assert!(host.assigned.is_empty());
    assert!(host.snapshot_requests.is_empty());
}

#[test]
fn another_element_is_not_handled_and_a_host_failure_passes_through() {
    let mut host = host_with(lamp_off());
    assert_eq!(
        execute("state", &[], &[], &mut host),
        Ok(Executed::NotHandled)
    );
    assert_eq!(
        execute(
            "snapshot",
            &[attr("location", "read"), attr("target", "fatal")],
            &[],
            &mut host
        ),
        Err(Failure::Host("fatal"))
    );
}
