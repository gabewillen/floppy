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

impl Host for FakeHost {
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

/// The memory JSON: sorted keys, compact separators, the content tagged
/// with its object entries in code point order, numbers as JSON numbers,
/// and the identities in identity order without repeats.
#[test]
fn to_json_writes_sorted_keys_tagged_content_and_sorted_identities() {
    for (message, text) in [
        (
            json!({"source_ids": ["zed", "alice", [0.5, 1]], "target_ids": ["bot", "\u{e9}"], "content": {"b": 1, "a": [true, null, 1.5], "\u{e9}": "x"}, "content_type": "application/json"}),
            r#"{"content":{"__stateforward_memory_content__":"mapping","value":[["a",{"__stateforward_memory_content__":"list","value":[true,null,1.5]}],["b",1],["é","x"]]},"content_type":"application/json","source_ids":["alice","zed",[0.5,1]],"target_ids":["bot","é"]}"#,
        ),
        (
            json!({"source_ids": [[1e-5, 2], [3, 4], "q\"\n"], "target_ids": [], "content": ["\u{1f600}", 1e-7, 12_345_678_901_234_567_000_u64, {"\u{7f}": [], "z": {}}], "content_type": "text/x"}),
            "{\"content\":{\"__stateforward_memory_content__\":\"list\",\"value\":[\"\u{1f600}\",1e-7,12345678901234567000,{\"__stateforward_memory_content__\":\"mapping\",\"value\":[[\"z\",{\"__stateforward_memory_content__\":\"mapping\",\"value\":[]}],[\"\u{7f}\",{\"__stateforward_memory_content__\":\"list\",\"value\":[]}]]}]},\"content_type\":\"text/x\",\"source_ids\":[\"q\\\"\\n\",[0.00001,2],[3,4]],\"target_ids\":[]}",
        ),
        (
            json!({"source_ids": ["alice", "alice", [1, 2], [1, 2]], "target_ids": ["bot"], "content": "hi", "content_type": "text/plain"}),
            r#"{"content":"hi","content_type":"text/plain","source_ids":["alice",[1,2]],"target_ids":["bot"]}"#,
        ),
        (
            json!({"source_ids": ["a"], "target_ids": [], "content": {"\u{1f600}": 1, "ab": 2, "a\u{e9}": 3, "a\"": 4, "a\\": 5, "A": [{"y": 1, "x": 2}]}, "content_type": "application/json"}),
            r#"{"content":{"__stateforward_memory_content__":"mapping","value":[["A",{"__stateforward_memory_content__":"list","value":[{"__stateforward_memory_content__":"mapping","value":[["x",2],["y",1]]}]}],["a\"",4],["a\\",5],["ab",2],["aé",3],["😀",1]]},"content_type":"application/json","source_ids":["a"],"target_ids":[]}"#,
        ),
        (
            json!({"source_ids": ["a"], "target_ids": [], "content": [1, -0.5, 3, 1e21, 2e-7, 100, 9_007_199_254_740_992_u64, 0.1, -1e-5], "content_type": "application/json"}),
            r#"{"content":{"__stateforward_memory_content__":"list","value":[1,-0.5,3,1e+21,2e-7,100,9007199254740992,0.1,-0.00001]},"content_type":"application/json","source_ids":["a"],"target_ids":[]}"#,
        ),
        (
            json!({"source_ids": [" x\u{1}"], "target_ids": ["\t"], "content": " \u{1}\u{1f600}", "content_type": "text/plain"}),
            "{\"content\":\" \\u0001\u{1f600}\",\"content_type\":\"text/plain\",\"source_ids\":[\" x\\u0001\"],\"target_ids\":[\"\\t\"]}",
        ),
    ] {
        assert_eq!(written(&message).as_deref(), Ok(text), "{message}");
    }
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

/// Each case of `testdata/cut_at_word.json` is cut to its text, with its
/// `cut`, within the bound.
#[test]
fn cut_at_word_cuts_each_fixture_case_to_its_text_within_the_bound() {
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

/// Each case of `testdata/stm_projection.json` (its data kept as JSON text
/// so an object's fields keep their order) is projected to its projection.
#[test]
fn stm_projection_projects_each_fixture_case_to_its_projection() {
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

// scored_candidates

/// The element's scores for `output`, `cue` and `weights`.
fn scored(output: &Json, cue: &Json, weights: &Json) -> Result<Json, &'static str> {
    let mut host = FakeHost {
        values: BTreeMap::from([
            ("output".to_owned(), data(output)),
            ("cue".to_owned(), data(cue)),
            ("weights".to_owned(), data(weights)),
        ]),
        ..FakeHost::default()
    };
    match execute(
        "scored_candidates",
        &[
            attr("location", "scored"),
            attr("output", "output"),
            attr("cue", "cue"),
            attr("weights", "weights"),
        ],
        &[],
        &mut host,
    ) {
        Ok(Executed::Handled) => match host.assigned.as_slice() {
            [(location, Data::JsonArray(text))] if location == "scored" => {
                Ok(serde_json::from_str(text).expect("the scores are JSON"))
            }
            other => panic!("unexpected assignment {other:?}"),
        },
        Err(Failure::Execution(detail)) => Err(detail),
        other => panic!("unexpected {other:?}"),
    }
}

/// The cue of the parity fixture: 256 exact binary fractions, so the Rust
/// element and the JavaScript it replaced hold identical doubles.
fn parity_cue() -> Json {
    let components: Vec<Json> = (0..256)
        .map(|i| {
            Json::from(match i % 3 {
                0 => 0.5,
                1 => -0.25,
                _ => 0.125,
            })
        })
        .collect();
    Json::Array(components)
}

/// The stored vectors of the parity fixture, as the JavaScript it replaced
/// writes them (`quantize` there, the encoding namespaces/conversation
/// ships).
fn parity_vectors() -> [String; 4] {
    ["QOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQOAQQA==",
     "wSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwSDwwQ==",
     "QBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQBDgQA==",
     "TGY="]
    .map(|body| format!("i8:{body}"))
}

/// The parity fixture's candidates output.
fn parity_output() -> Json {
    let vectors = parity_vectors();
    json!({"results": [{"rowcount": 6, "rows": [
        {"values": ["000b", "2026-01-02 03:04:05", 120, vectors[0]]},
        {"values": ["000a", "2025-12-31 03:04:05.25", 340, vectors[1]]},
        {"values": ["000c", "2026-01-01 12:00:00", 56, vectors[2]]},
        {"values": ["000d", "2025-06-01 00:00:00", 8, vectors[3]]},
        {"values": ["000e", "not-a-date", 8, null]},
        {"values": ["000f", "2026-01-03T09:30:00.125", 8, "i8:AAA="]},
    ]}]})
}

/// The scored candidates are exactly the ones the JavaScript the element
/// replaces computes: the same doubles, so the recall log and the ranked
/// ids do not move. One exception, asserted apart: `f64::powf` and
/// `Math.pow` differ by one ulp on the extreme recency exponent of the
/// unparsable-date row, whose score is 1e-33 either way.
#[allow(
    clippy::unreadable_literal,
    reason = "the expected doubles are the JavaScript's full-precision values"
)]
#[test]
fn scored_candidates_match_the_javascript_they_replaced() {
    let weights = json!({"similarity": 1, "recency": 0.35, "recency_half_life_ms": 172_800_000});
    let expected = json!([
        {"id": "000b", "length": 120, "order": 0, "similarity": 5.304773793480734, "score": 5.530309421666898},
        {"id": "000c", "length": 56, "order": 1, "similarity": 3.0514967518301184, "score": 3.2329298332247705},
        {"id": "000f", "length": 8, "order": 2, "similarity": 0, "score": 0.35},
        {"id": "000d", "length": 8, "order": 3, "similarity": 0, "score": 9.402632054978437e-34},
        {"id": "000e", "length": 8, "order": 4, "similarity": 0, "score": 0},
        {"id": "000a", "length": 340, "order": 5, "similarity": -5.304654209331108, "score": -5.191886282152478},
    ]);
    assert_eq!(
        scored(&parity_output(), &parity_cue(), &weights).expect("scores"),
        expected
    );
    // The one-ulp `Math.pow` difference, against the JavaScript value.
    let scored = scored(&parity_output(), &parity_cue(), &weights).expect("scores");
    let score = scored[3]["score"].as_f64().expect("a number");
    let javascript = 9.402632054978435e-34_f64;
    assert!((score - javascript).abs() / javascript < 1e-15, "{score}");
}

/// An episode without a vector, one of another space, and an all-zero
/// vector score by recency alone; ties keep their row order.
#[test]
fn scored_candidates_score_without_a_vector_by_recency_alone() {
    let weights = json!({"similarity": 1, "recency": 0.35, "recency_half_life_ms": 172_800_000});
    let output = json!({"results": [{"rowcount": 3, "rows": [
        {"values": ["a", "2026-01-03 00:00:00", 10, null]},
        {"values": ["b", "2026-01-01 00:00:00", 10, "i8:TACa"]},
        {"values": ["c", "2026-01-01 00:00:00", 10, "i8:AAA="]},
    ]}]});
    // A two-dimension cue: "b"'s stored vector is of another space (three
    // dimensions), and "c"'s is all zero.
    let scored = scored(&output, &json!([1.0, 0.0]), &weights).expect("scores");
    let ids: Vec<&str> = scored
        .as_array()
        .expect("an array")
        .iter()
        .map(|row| row["id"].as_str().expect("an id"))
        .collect();
    assert_eq!(ids, ["a", "b", "c"], "newest first, ties in row order");
    for row in scored.as_array().expect("an array") {
        assert_eq!(row["similarity"], json!(0), "{row}");
    }
    assert_eq!(scored[0]["score"], json!(0.35), "{scored:?}");
    assert_eq!(scored[1]["score"], scored[2]["score"], "the same recency");
}

/// A malformed output, cue or weights raises `error.execution` and assigns
/// nothing.
#[test]
fn malformed_scored_candidates_inputs_raise_error_execution() {
    let weights = json!({"similarity": 1, "recency": 0.35, "recency_half_life_ms": 172_800_000});
    let cue = parity_cue();
    for (output, cue, weights, detail) in [
        (
            json!({"results": []}),
            cue.clone(),
            weights.clone(),
            "memory:scored_candidates output has no first result",
        ),
        (
            json!({"results": [{"rowcount": 0}]}),
            cue.clone(),
            weights.clone(),
            "memory:scored_candidates output has no first result",
        ),
        (
            json!({"results": [{"rowcount": 0, "rows": [{}]}]}),
            cue.clone(),
            weights.clone(),
            "memory:scored_candidates output has a row without a values array",
        ),
        (
            json!({"results": [{"rowcount": 0, "rows": []}]}),
            json!("not an array"),
            weights.clone(),
            "memory:scored_candidates cue is not an array of finite numbers",
        ),
        (
            json!({"results": [{"rowcount": 0, "rows": []}]}),
            json!([0.5, "x"]),
            weights.clone(),
            "memory:scored_candidates cue is not an array of finite numbers",
        ),
        (
            json!({"results": [{"rowcount": 0, "rows": []}]}),
            cue.clone(),
            json!({"similarity": 1, "recency": 0.35}),
            "memory:scored_candidates weights are not numbers with a positive half-life",
        ),
        (
            json!({"results": [{"rowcount": 0, "rows": []}]}),
            cue.clone(),
            json!({"similarity": 1, "recency": 0.35, "recency_half_life_ms": 0}),
            "memory:scored_candidates weights are not numbers with a positive half-life",
        ),
    ] {
        assert_eq!(scored(&output, &cue, &weights), Err(detail), "{output}");
    }
}

/// The element's attribute rules and the failures of its expressions.
#[test]
fn scored_candidates_element_rules() {
    let mut host = FakeHost::default();
    assert_eq!(
        execute(
            "scored_candidates",
            &[
                attr("location", "scored"),
                attr("output", "output"),
                attr("cue", "cue"),
                attr("weights", "weights"),
                attr("extra", "1"),
            ],
            &[],
            &mut host
        ),
        Err(Failure::Execution(
            "memory:scored_candidates takes only the location, output, cue and weights attributes"
        ))
    );
    assert_eq!(
        execution(execute(
            "scored_candidates",
            &[attr("output", "output")],
            &[],
            &mut FakeHost::default()
        )),
        "memory:scored_candidates requires a location"
    );
    assert_eq!(
        execution(execute(
            "scored_candidates",
            &[
                attr("location", "scored"),
                attr("output", "output"),
                attr("cue", "cue"),
                attr("weights", ""),
            ],
            &[],
            &mut FakeHost::default()
        )),
        "memory:scored_candidates has an empty attribute"
    );
    assert_eq!(
        execution(execute(
            "scored_candidates",
            &[
                attr("location", "scored"),
                attr("output", "nowhere"),
                attr("cue", "cue"),
                attr("weights", "weights"),
            ],
            &[],
            &mut FakeHost::default()
        )),
        "memory:scored_candidates output did not evaluate"
    );
    assert!(host.assigned.is_empty());
}

/// A `created_at` of SQLite's and Postgres's UTC forms gives its
/// milliseconds; a date alone, text of another form, or a missing cell is 0.
#[allow(
    clippy::unreadable_literal,
    reason = "the expected double is the full-precision recency value"
)]
#[test]
fn created_at_milliseconds_of_the_stored_forms() {
    let weights = json!({"similarity": 0, "recency": 1, "recency_half_life_ms": 172_800_000});
    let output = json!({"results": [{"rowcount": 4, "rows": [
        // The newest: 2026-01-03 09:30:00.125 UTC.
        {"values": ["t", "2026-01-03T09:30:00.125", 1, null]},
        {"values": ["u", "2026-01-02 09:30:00.125", 1, null]},
        {"values": ["v", "January 3 2026", 1, null]},
        {"values": ["w", "2026-01-03 09:30:00", 1, null]},
    ]}]});
    let scored = scored(&output, &json!([]), &weights).expect("scores");
    // The order: the newest (recency 1), then the whole-seconds row 125 ms
    // older (recency just under 1), then 24 hours older (0.5^(1/2)), then
    // the unparsable date (0: its age is unbounded).
    let ids: Vec<&str> = scored
        .as_array()
        .expect("an array")
        .iter()
        .map(|row| row["id"].as_str().expect("an id"))
        .collect();
    assert_eq!(ids, ["t", "w", "u", "v"]);
    assert_eq!(scored[0]["score"], json!(1));
    assert_eq!(scored[1]["score"], json!(0.999999498591575));
    let half = 0.5_f64.sqrt();
    assert!((scored[2]["score"].as_f64().expect("a number") - half).abs() < 1e-12);
    assert_eq!(scored[3]["score"], json!(0), "an unparsable date is 0");
}
