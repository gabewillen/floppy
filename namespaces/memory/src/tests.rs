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

impl MemoryHost for FakeHost {
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

/// What `<memory:to_json>` assigns for `message`.
fn written(message: &Json) -> Result<String, &'static str> {
    let mut host = FakeHost {
        values: BTreeMap::from([("input".to_owned(), data(message))]),
        ..FakeHost::default()
    };
    match execute(
        "to_json",
        &[attr("location", "payload"), attr("message", "input")],
        &[],
        &mut host,
    ) {
        Ok(Executed::Handled) => match host.assigned.as_slice() {
            [(location, Data::String(text))] if location == "payload" => Ok(text.clone()),
            other => panic!("unexpected assignment {other:?}"),
        },
        Err(Failure::Execution(detail)) => Err(detail),
        other => panic!("unexpected {other:?}"),
    }
}

/// What `<memory:contents>` assigns for `result`.
fn read_contents(result: &Json) -> Result<Json, &'static str> {
    let mut host = FakeHost {
        values: BTreeMap::from([("result".to_owned(), data(result))]),
        ..FakeHost::default()
    };
    match execute(
        "contents",
        &[attr("location", "contents"), attr("result", "result")],
        &[],
        &mut host,
    ) {
        Ok(Executed::Handled) => match host.assigned.as_slice() {
            [(location, Data::JsonArray(text))] if location == "contents" => {
                Ok(serde_json::from_str(text).expect("the contents are JSON"))
            }
            other => panic!("unexpected assignment {other:?}"),
        },
        Err(Failure::Execution(detail)) => Err(detail),
        other => panic!("unexpected {other:?}"),
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
        "../package/memory.extension.json"
    ))
    .expect("the manifest is a valid extension package manifest");
    assert_eq!(manifest.namespace_uri, NAMESPACE_URI);
    assert_eq!(
        manifest.extension_artifact,
        "floppy_memory.virtualized.wasm"
    );
    manifest
        .verify_schema(&scxml_validator_core::SchemaSource::new(
            "memory.xsd",
            include_bytes!("../package/memory.xsd").to_vec(),
        ))
        .expect("the schema digest, targetNamespace, and WIT identity verify");
}

#[test]
fn other_local_names_are_not_handled() {
    let mut host = FakeHost::default();
    assert_eq!(
        execute("apply", &[attr("location", "x")], &[], &mut host),
        Ok(Executed::NotHandled)
    );
    assert!(host.assigned.is_empty());
}

/// The memory JSON `memoryPayload()` in `abilities/conversation/conversation.js`
/// wrote, before this element replaced it (`testdata/to_json.json`, written by
/// that JavaScript), is written byte for byte.
#[test]
fn to_json_writes_what_the_javascript_it_replaces_wrote() {
    let cases: Json =
        serde_json::from_str(include_str!("testdata/to_json.json")).expect("the cases are JSON");
    for case in cases.as_array().expect("an array of cases") {
        assert_eq!(
            written(&case["message"]).as_deref(),
            Ok(case["json"].as_str().expect("a text")),
            "{case}"
        );
    }
}

/// The original's own texts (tests/tests/communication.rs,
/// `the_remembered_input_is_the_original_memory_json`).
#[test]
fn to_json_writes_the_original_memory_json() {
    assert_eq!(
        written(&json!({"source_ids": ["zed", "alice", [0.5, 1]], "target_ids": ["bot", "\u{e9}"], "content": {"b": 1, "a": [true, null, 1.5], "\u{e9}": "x"}, "content_type": "application/json"})).as_deref(),
        Ok(r#"{"content":{"__stateforward_memory_content__":"mapping","value":[["é","x"],["a",{"__stateforward_memory_content__":"list","value":[true,null,1.5]}],["b",1]]},"content_type":"application/json","source_ids":["alice","zed",[0.5,1.0]],"target_ids":["bot","é"]}"#)
    );
}

#[test]
fn to_json_writes_absent_content_as_null() {
    assert_eq!(
        written(&json!({"source_ids": ["a"], "target_ids": [], "content_type": "text/plain"}))
            .as_deref(),
        Ok(r#"{"content":null,"content_type":"text/plain","source_ids":["a"],"target_ids":[]}"#)
    );
}

#[test]
fn malformed_to_json_raises_error_execution_and_assigns_nothing() {
    for (message, detail) in [
        (json!("hi"), "memory:to_json message is not an object"),
        (
            json!({"source_ids": "a", "target_ids": [], "content_type": "text/plain"}),
            "memory:to_json message source_ids is not an array of identities",
        ),
        (
            json!({"source_ids": [], "target_ids": [{"a": 1}], "content_type": "text/plain"}),
            "memory:to_json message target_ids is not an array of identities",
        ),
        (
            json!({"source_ids": [], "target_ids": [], "content_type": 1}),
            "memory:to_json message content_type is not a string",
        ),
    ] {
        assert_eq!(written(&message), Err(detail));
    }
    let mut host = FakeHost::default();
    assert_eq!(
        execution(execute(
            "to_json",
            &[attr("location", "payload")],
            &[],
            &mut host
        )),
        "memory:to_json requires a message"
    );
    assert_eq!(
        execution(execute(
            "to_json",
            &[
                attr("location", "payload"),
                attr("message", "input"),
                attr("kind", "x")
            ],
            &[],
            &mut host
        )),
        "memory:to_json takes only the location and message attributes"
    );
    assert!(host.assigned.is_empty());
}

/// The contents `recalledContents()` in `abilities/reasoning/reasoning.js`
/// read, before this element replaced it (`testdata/contents.json`, written by
/// that JavaScript), are read identically.
#[test]
fn contents_reads_what_the_javascript_it_replaces_read() {
    let cases: Json =
        serde_json::from_str(include_str!("testdata/contents.json")).expect("the cases are JSON");
    for case in cases.as_array().expect("an array of cases") {
        assert_eq!(
            read_contents(&case["result"]),
            Ok(case["contents"].clone()),
            "{case}"
        );
    }
}

#[test]
fn malformed_contents_raise_error_execution_and_assign_nothing() {
    for (result, detail) in [
        (json!([]), "memory:contents result is not an object"),
        (
            json!({"rowcount": 0}),
            "memory:contents result has no rows array",
        ),
        (
            json!({"rows": [{"columns": []}]}),
            "memory:contents result has a row without a values array",
        ),
    ] {
        assert_eq!(read_contents(&result), Err(detail));
    }
    let mut host = FakeHost::default();
    assert_eq!(
        execution(execute(
            "contents",
            &[attr("result", "result")],
            &[],
            &mut host
        )),
        "memory:contents requires a location"
    );
    assert_eq!(
        execution(execute(
            "contents",
            &[attr("location", "c"), attr("result", "result")],
            &[Content::Text("x".into())],
            &mut host
        )),
        "memory:contents holds no content"
    );
    assert!(host.assigned.is_empty());
}

#[test]
fn a_failed_assignment_or_host_failure_is_reported() {
    let mut host = FakeHost {
        values: BTreeMap::from([("result".to_owned(), data(&json!({"rows": []})))]),
        fail_assign: true,
        ..FakeHost::default()
    };
    assert_eq!(
        execution(execute(
            "contents",
            &[attr("location", "c"), attr("result", "result")],
            &[],
            &mut host
        )),
        "memory:contents could not assign the contents"
    );
    assert_eq!(
        execute(
            "to_json",
            &[attr("location", "payload"), attr("message", "fatal")],
            &[],
            &mut FakeHost::default()
        ),
        Err(Failure::Host("fatal"))
    );
}

/// What `<memory:cut_at_word>` assigns for `words`.
fn cut(words: &Data) -> Result<Json, &'static str> {
    let mut host = FakeHost {
        values: BTreeMap::from([("words".to_owned(), words.clone())]),
        ..FakeHost::default()
    };
    match execute(
        "cut_at_word",
        &[attr("location", "turn"), attr("words", "words")],
        &[],
        &mut host,
    ) {
        Ok(Executed::Handled) => match host.assigned.as_slice() {
            [(location, Data::JsonObject(text))] if location == "turn" => {
                Ok(serde_json::from_str(text).expect("the cut words are JSON"))
            }
            other => panic!("unexpected assignment {other:?}"),
        },
        Err(Failure::Execution(detail)) => Err(detail),
        other => panic!("unexpected {other:?}"),
    }
}

/// What `<memory:stm_projection>` assigns for `data`.
fn projected(data: &Data) -> Result<Json, &'static str> {
    let mut host = FakeHost {
        values: BTreeMap::from([("data".to_owned(), data.clone())]),
        ..FakeHost::default()
    };
    match execute(
        "stm_projection",
        &[attr("location", "payload"), attr("data", "data")],
        &[],
        &mut host,
    ) {
        Ok(Executed::Handled) => match host.assigned.as_slice() {
            [(location, Data::JsonObject(text))] if location == "payload" => {
                Ok(serde_json::from_str(text).expect("the projection is JSON"))
            }
            other => panic!("unexpected assignment {other:?}"),
        },
        Err(Failure::Execution(detail)) => Err(detail),
        other => panic!("unexpected {other:?}"),
    }
}

/// The original's own `_cut_at_word` (`testdata/cut_at_word.json`, written by
/// running it on each case's words): the same text and the same `cut`.
#[test]
fn cut_at_word_cuts_as_the_original_cuts() {
    let cases: Json = serde_json::from_str(include_str!("testdata/cut_at_word.json"))
        .expect("the cases are JSON");
    for case in cases.as_array().expect("an array of cases") {
        let words = case["words"].as_str().expect("the words are a string");
        let turn = cut(&Data::String(words.to_owned())).expect("the words are cut");
        assert_eq!(turn["text"], case["text"], "{words:?}");
        assert_eq!(turn["cut"], case["cut"], "{words:?}");
        let text = turn["text"].as_str().expect("the text is a string");
        assert!(text.len() <= MAX_TURN_TEXT_BYTES);
    }
}

#[test]
fn malformed_cut_at_word_raises_error_execution_and_assigns_nothing() {
    assert_eq!(
        cut(&Data::Null),
        Err("memory:cut_at_word words are not a string")
    );
    assert_eq!(
        cut(&data(&json!(["a"]))),
        Err("memory:cut_at_word words are not a string")
    );
    let mut host = FakeHost::default();
    assert_eq!(
        execution(execute(
            "cut_at_word",
            &[attr("location", "turn")],
            &[],
            &mut host
        )),
        "memory:cut_at_word requires words"
    );
    assert_eq!(
        execution(execute(
            "cut_at_word",
            &[
                attr("location", "turn"),
                attr("words", "words"),
                attr("bytes", "8")
            ],
            &[],
            &mut host
        )),
        "memory:cut_at_word takes only the location and words attributes"
    );
    assert_eq!(
        execution(execute(
            "cut_at_word",
            &[attr("location", "turn"), attr("words", "words")],
            &[Content::Start, Content::End],
            &mut host
        )),
        "memory:cut_at_word holds no content"
    );
    assert_eq!(
        execution(execute(
            "cut_at_word",
            &[attr("location", "turn"), attr("words", "nowhere")],
            &[],
            &mut host
        )),
        "memory:cut_at_word words did not evaluate"
    );
    assert!(host.assigned.is_empty());
}

/// The original's own `_payload_projection` (`testdata/stm_projection.json`,
/// written by running it on each case's data, kept as JSON text so an
/// object's fields keep their order).
#[test]
fn stm_projection_projects_as_the_original_projects() {
    let cases: Json = serde_json::from_str(include_str!("testdata/stm_projection.json"))
        .expect("the cases are JSON");
    for case in cases.as_array().expect("an array of cases") {
        let text = case["data"].as_str().expect("the data is JSON text");
        let value: Json = serde_json::from_str(text).expect("the data parses");
        let input = match value {
            Json::Object(_) => Data::JsonObject(text.to_owned()),
            other => data(&other),
        };
        assert_eq!(projected(&input), Ok(case["projection"].clone()), "{text}");
    }
}

#[test]
fn stm_projection_keeps_the_first_fields_in_their_order() {
    let fields: Vec<String> = (0..20).rev().map(|i| format!("\"k{i:02}\":{i}")).collect();
    let projection = projected(&Data::JsonObject(format!("{{{}}}", fields.join(","))))
        .expect("the object is projected");
    assert_eq!(
        projection.as_object().expect("an object").len(),
        MAX_PROJECTION_FIELDS
    );
    assert!(projection.get("k19").is_some() && projection.get("k04").is_some());
    assert!(projection.get("k03").is_none());
    assert_eq!(
        projected(&Data::JsonObject(r#"{"a":1,"a":"x"}"#.to_owned())),
        Ok(json!({"a": "x"}))
    );
    assert_eq!(projected(&Data::Undefined), Ok(json!({})));
}

#[test]
fn malformed_stm_projection_raises_error_execution_and_assigns_nothing() {
    assert_eq!(
        projected(&Data::JsonObject("[1]".to_owned())),
        Err("memory:stm_projection data is not a JSON object")
    );
    let mut host = FakeHost::default();
    assert_eq!(
        execution(execute(
            "stm_projection",
            &[attr("data", "data")],
            &[],
            &mut host
        )),
        "memory:stm_projection requires a location"
    );
    assert_eq!(
        execution(execute(
            "stm_projection",
            &[attr("location", "p"), attr("data", "")],
            &[],
            &mut host
        )),
        "memory:stm_projection has an empty attribute"
    );
    assert_eq!(
        execution(execute(
            "stm_projection",
            &[attr("location", "p"), attr("data", "nowhere")],
            &[],
            &mut host
        )),
        "memory:stm_projection data did not evaluate"
    );
    assert!(host.assigned.is_empty());
}
