//! The `event` attribute vocabulary through `compile`: `event:schema` is
//! resolved (inline, or from a supplied file with an optional JSON Pointer)
//! into canonical JSON with its SHA-256 on the owning element's Machine IR,
//! `event:schemaexpr` is carried unevaluated, and every failure is a
//! validator error with no artifact. The schemas are metadata: removing them
//! leaves the rest of the Machine IR unchanged.

#[allow(dead_code, reason = "this test uses only the shared validator")]
mod common;

use scxml_compiler_core::{
    CompileInput, CompileOutput, Compiler, DoneDataIr, EventSchemaIr, ExecutableContentIr,
    LowerError, MachineIr, ScriptResolution,
};
use scxml_validator_core::EventSchemaResources;

const EVENT: &str = scxml_types::EVENT_NAMESPACE_URI;

fn compile_with(source: &str, files: &[(&str, &str)]) -> CompileOutput {
    Compiler::new(common::validator()).compile(CompileInput {
        source_name: "case.scxml".into(),
        source: source.into(),
        packages: Vec::new(),
        package_schemas: Vec::new(),
        scripts: Vec::new(),
        script_resolution: ScriptResolution::Compile,
        event_schemas: files
            .iter()
            .map(|(uri, text)| ((*uri).to_owned(), text.as_bytes().to_vec()))
            .collect::<EventSchemaResources>(),
    })
}

fn machine(output: &CompileOutput) -> MachineIr {
    let artifact = output.artifact.as_ref().unwrap_or_else(|| {
        panic!(
            "the document compiles: {:?}",
            output.validation.report.diagnostics()
        )
    });
    MachineIr::from_json(&artifact.bytes).expect("the emitted IR decodes")
}

fn codes(output: &CompileOutput) -> Vec<String> {
    output
        .validation
        .report
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.severity == scxml_validator_core::Severity::Error)
        .map(|diagnostic| diagnostic.code.clone())
        .collect()
}

/// A machine whose transition, `<raise>`, `<send>`, and `<donedata>` carry
/// `attributes[0..4]` respectively.
fn document(attributes: [&str; 4]) -> String {
    let [transition, raise, send, donedata] = attributes;
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="{EVENT}" version="1.0" datamodel="ecmascript" initial="s">
  <state id="s">
    <onentry>
      <raise event="r" {raise}/>
      <send event="x" delay="1s" {send}/>
    </onentry>
    <transition event="go" target="f" {transition}/>
  </state>
  <final id="f"><donedata {donedata}><param name="id" expr="1"/></donedata></final>
</scxml>"#
    )
}

const ORDER_FILE: &str = r##"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$defs": {
    "Order": {"type": "object", "required": ["items"], "properties": {"items": {"type": "array", "items": {"$ref": "#/$defs/Item"}}}},
    "Item": {"type": "string"}
  }
}"##;

#[test]
fn every_owner_carries_its_schema_in_machine_ir() {
    let output = compile_with(
        &document([
            r#"event:schema='{"type":"object","properties":{"id":{"type":"string"}}}'"#,
            r#"event:schema="schemas/order.json#/$defs/Item""#,
            r#"event:schemaexpr="({type: 'string'})""#,
            r#"event:schema="true""#,
        ]),
        &[("schemas/order.json", ORDER_FILE)],
    );
    let ir = machine(&output);
    let inline = r#"{"properties":{"id":{"type":"string"}},"type":"object"}"#;
    assert_eq!(
        ir.transitions[0].event_schema,
        Some(EventSchemaIr::Schema {
            schema: inline.to_owned(),
            sha256: scxml_event_schema_sha(inline),
        })
    );
    let onentry = &ir.states[0].onentry[0].actions;
    let ExecutableContentIr::Raise { event_schema, .. } = &onentry[0] else {
        panic!("raise first");
    };
    let item = r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"string"}"#;
    assert_eq!(
        event_schema,
        &Some(EventSchemaIr::Schema {
            schema: item.to_owned(),
            sha256: scxml_event_schema_sha(item),
        })
    );
    let ExecutableContentIr::Send { value } = &onentry[1] else {
        panic!("send second");
    };
    assert_eq!(
        value.event_schema,
        Some(EventSchemaIr::Schemaexpr {
            expr: "({type: 'string'})".to_owned()
        })
    );
    let Some(DoneDataIr { event_schema, .. }) = &ir.states[1].donedata else {
        panic!("donedata");
    };
    assert_eq!(
        event_schema,
        &Some(EventSchemaIr::Schema {
            schema: "true".to_owned(),
            sha256: scxml_event_schema_sha("true"),
        })
    );
}

fn scxml_event_schema_sha(text: &str) -> String {
    use sha2::Digest as _;
    hex::encode(sha2::Sha256::digest(text.as_bytes()))
}

#[test]
fn a_pointer_bundles_what_the_selected_schema_references() {
    let output = compile_with(
        &document([
            r#"event:schema="schemas/order.json#/$defs/Order""#,
            "",
            "",
            "",
        ]),
        &[("schemas/order.json", ORDER_FILE)],
    );
    let Some(EventSchemaIr::Schema { schema, .. }) =
        machine(&output).transitions[0].event_schema.clone()
    else {
        panic!("a resolved schema");
    };
    assert_eq!(
        schema,
        r##"{"$defs":{"/$defs/Item":{"type":"string"}},"$schema":"https://json-schema.org/draft/2020-12/schema","properties":{"items":{"items":{"$ref":"#/$defs/~1$defs~1Item"},"type":"array"}},"required":["items"],"type":"object"}"##
    );
}

#[test]
fn the_schemas_change_nothing_else_in_machine_ir() {
    let plain = machine(&compile_with(&document(["", "", "", ""]), &[]));
    let mut described = machine(&compile_with(
        &document([
            r#"event:schema='{"type":"object"}'"#,
            r#"event:schemaexpr="s""#,
            r#"event:schema="schemas/order.json""#,
            r#"event:schema="false""#,
        ]),
        &[("schemas/order.json", ORDER_FILE)],
    ));
    described.transitions[0].event_schema = None;
    for action in &mut described.states[0].onentry[0].actions {
        match action {
            ExecutableContentIr::Raise { event_schema, .. } => *event_schema = None,
            ExecutableContentIr::Send { value } => value.event_schema = None,
            _ => {}
        }
    }
    if let Some(donedata) = &mut described.states[1].donedata {
        donedata.event_schema = None;
    }
    assert_eq!(described, plain);
    let bytes = plain.to_json().expect("encodes");
    assert!(
        !String::from_utf8(bytes)
            .expect("IR is UTF-8")
            .contains("event_schema"),
        "an element without a schema keeps the Machine IR bytes it had"
    );
}

#[test]
fn every_failure_is_a_validator_error_without_an_artifact() {
    for (attribute, files, expected) in [
        (
            r#"event:schema="schemas/missing.json""#,
            &[][..],
            "EVENT004",
        ),
        (
            r#"event:schema="https://example.com/order.json""#,
            &[],
            "EVENT004",
        ),
        (r#"event:schema='{"type":'"#, &[], "EVENT003"),
        (
            r#"event:schema="schemas/order.json#/$defs/Nope""#,
            &[("schemas/order.json", ORDER_FILE)],
            "EVENT005",
        ),
        (r#"event:schema='{"minimum":"1"}'"#, &[], "EVENT006"),
        (
            r#"event:schema='{"$schema":"https://example.com/dialect"}'"#,
            &[],
            "EVENT007",
        ),
        (r#"event:schema='{"$ref":"item.json"}'"#, &[], "EVENT008"),
        (
            r#"event:schema="true" event:schemaexpr="s""#,
            &[],
            "EVENT002",
        ),
    ] {
        let output = compile_with(&document([attribute, "", "", ""]), files);
        assert!(output.artifact.is_none(), "{attribute}");
        assert!(
            codes(&output).iter().any(|code| code == expected),
            "{attribute}: {:?}",
            codes(&output)
        );
    }
    // An expression is checked like every other value expression: one that
    // does not parse is the SCXML038 warning (it would raise
    // error.execution if evaluated), and the document still compiles.
    let unparsed = compile_with(&document([r#"event:schemaexpr="1 +""#, "", "", ""]), &[]);
    assert!(unparsed.artifact.is_some());
    assert!(
        unparsed
            .validation
            .report
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == "SCXML038")
    );
    // Anywhere else the vocabulary is EVENT001.
    let misplaced = document(["", "", "", ""])
        .replace(r#"<state id="s">"#, r#"<state id="s" event:schema="true">"#);
    assert_eq!(codes(&compile_with(&misplaced, &[])), ["EVENT001"]);
}

#[test]
fn decoding_rejects_a_tampered_schema() {
    let output = compile_with(
        &document([r#"event:schema='{"type":"object"}'"#, "", "", ""]),
        &[],
    );
    let bytes = String::from_utf8(output.artifact.expect("compiles").bytes).expect("UTF-8");
    let digest = scxml_event_schema_sha(r#"{"type":"object"}"#);
    let tampered_digest = bytes.replace(&digest, &"0".repeat(64));
    assert_eq!(
        MachineIr::from_json(tampered_digest.as_bytes()),
        Err(LowerError::InvalidStructure(
            "event schema sha256 does not match its schema"
        ))
    );
    let truncated = bytes
        .replace(r#"{\"type\":\"object\"}"#, r#"{\"type\":"#)
        .replace(&digest, &scxml_event_schema_sha(r#"{"type":"#));
    assert_eq!(
        MachineIr::from_json(truncated.as_bytes()),
        Err(LowerError::InvalidStructure("event schema is not JSON"))
    );
}
