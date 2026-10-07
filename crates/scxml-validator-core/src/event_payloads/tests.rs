use scxml_syntax::{SourceId, SourceText, parse_xml};

use super::event_payload_diagnostics;
use crate::{EventSchemaResources, Severity};

/// The `SCXML102`/`SCXML103` findings of one ECMAScript machine whose
/// first state holds `body`.
fn findings(body: &str) -> Vec<(String, Severity, String)> {
    let source = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event" version="1.0" datamodel="ecmascript" initial="s">
  <state id="s">{body}</state>
</scxml>"#
    );
    let document = parse_xml(SourceText::new(SourceId::new(1), "doc.scxml", source))
        .expect("the test document parses");
    event_payload_diagnostics(&document, &EventSchemaResources::new())
        .into_iter()
        .map(|diagnostic| (diagnostic.code, diagnostic.severity, diagnostic.message))
        .collect()
}

fn codes(body: &str) -> Vec<String> {
    findings(body).into_iter().map(|(code, ..)| code).collect()
}

const OBJECT_N_STRING: &str =
    r#"{"type":"object","properties":{"n":{"type":"string"}},"required":["n"]}"#;
const OBJECT_N_INTEGER: &str =
    r#"{"type":"object","properties":{"n":{"type":"integer"}},"required":["n"]}"#;

#[test]
fn literal_content_must_satisfy_its_own_schema() {
    let bad = format!(
        r#"<onentry><send event="x" event:schema='{OBJECT_N_STRING}'><content>{{"n": 1}}</content></send></onentry>"#
    );
    let found = findings(&bad);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "SCXML102");
    assert_eq!(found[0].1, Severity::Error);
    let good = format!(
        r#"<onentry><send event="x" event:schema='{OBJECT_N_STRING}'><content>{{"n": "one"}}</content></send></onentry>"#
    );
    assert!(codes(&good).is_empty());
}

#[test]
fn literal_donedata_must_satisfy_its_own_schema() {
    let body = r#"<final id="f"><donedata event:schema='{"type":"array"}'><content>{"a":1}</content></donedata></final>"#;
    assert_eq!(codes(body), ["SCXML102"]);
    let fine = r#"<final id="f"><donedata event:schema='{"type":"array"}'><content>[1]</content></donedata></final>"#;
    assert!(codes(fine).is_empty());
}

#[test]
fn literal_content_must_satisfy_the_transitions_handling_its_event() {
    let body = format!(
        r#"<onentry><send event="order.new"><content>{{"n": 1}}</content></send></onentry>
           <transition event="order" event:schema='{OBJECT_N_STRING}' target="s"/>"#
    );
    let found = findings(&body);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "SCXML103");
    assert_eq!(found[0].1, Severity::Warning);
    // A send to the parent is never seen by this document's transitions.
    let parent = format!(
        r##"<onentry><send event="order.new" target="#_parent"><content>{{"n": 1}}</content></send></onentry>
           <transition event="order" event:schema='{OBJECT_N_STRING}' target="s"/>"##
    );
    assert!(codes(&parent).is_empty());
    // An unrelated descriptor does not handle it.
    let other = format!(
        r#"<onentry><send event="orders"><content>{{"n": 1}}</content></send></onentry>
           <transition event="order" event:schema='{OBJECT_N_STRING}' target="s"/>"#
    );
    assert!(codes(&other).is_empty());
}

#[test]
fn donedata_must_satisfy_the_transitions_handling_done_state() {
    let body = r#"<state id="inner"><final id="f"><donedata><content>{"ok":"yes"}</content></donedata></final></state>
        <transition event="done.state.inner" event:schema='{"type":"object","properties":{"ok":{"type":"boolean"}}}' target="s"/>"#;
    assert_eq!(codes(body), ["SCXML103"]);
}

#[test]
fn provably_incompatible_schemas_are_reported_with_a_witness() {
    let body = format!(
        r#"<onentry><send event="x" event:schema='{OBJECT_N_STRING}'><param name="n" expr="v"/></send></onentry>
           <transition event="x" event:schema='{OBJECT_N_INTEGER}' target="s"/>"#
    );
    let found = findings(&body);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "SCXML103");
}

#[test]
fn equal_or_undecided_schemas_are_never_reported() {
    let equal = format!(
        r#"<onentry><send event="x" event:schema='{OBJECT_N_STRING}'/></onentry>
           <transition event="x" event:schema='{OBJECT_N_STRING}' target="s"/>"#
    );
    assert!(codes(&equal).is_empty());
    // A narrower handler with no witness the sender's schema yields: the
    // minimal instance {"n": ""} satisfies both, so nothing is claimed.
    let undecided = format!(
        r#"<onentry><send event="x" event:schema='{OBJECT_N_STRING}'/></onentry>
           <transition event="x" event:schema='{{"type":"object","properties":{{"n":{{"type":"string","maxLength":3}}}}}}' target="s"/>"#
    );
    assert!(codes(&undecided).is_empty());
    // A compatible wider handler.
    let wider = format!(
        r#"<onentry><send event="x" event:schema='{OBJECT_N_STRING}'/></onentry>
           <transition event="x" event:schema='{{"type":"object"}}' target="s"/>"#
    );
    assert!(codes(&wider).is_empty());
}

#[test]
fn schemaexpr_is_skipped_silently() {
    let body = format!(
        r#"<onentry><send event="x" event:schemaexpr="schema"><content>{{"n": 1}}</content></send></onentry>
           <transition event="x" event:schemaexpr="other" target="s"/>
           <transition event="x" event:schema='{OBJECT_N_INTEGER}' target="s"/>"#
    );
    // The literal data {"n": 1} satisfies the one literal schema.
    assert!(codes(&body).is_empty());
    let only_expr = r#"<onentry><send event="x" event:schemaexpr="schema"/></onentry>
           <transition event="x" event:schemaexpr="other" target="s"/>"#;
    assert!(codes(only_expr).is_empty());
}

#[test]
fn other_data_models_and_non_json_content_are_not_checked() {
    let text = format!(
        r#"<onentry><send event="x" event:schema='{OBJECT_N_STRING}'><content>not json</content></send></onentry>"#
    );
    // Only literal JSON content is checked.
    assert!(codes(&text).is_empty(), "non-JSON text is not checked");
    let source = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event" version="1.0" datamodel="xpath" initial="s">
  <state id="s"><onentry><send event="x" event:schema='{OBJECT_N_STRING}'><content>{{"n": 1}}</content></send></onentry></state>
</scxml>"#
    );
    let document = parse_xml(SourceText::new(SourceId::new(1), "doc.scxml", source))
        .expect("the test document parses");
    assert!(event_payload_diagnostics(&document, &EventSchemaResources::new()).is_empty());
}
