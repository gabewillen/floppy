//! Literal invoked-service types: the Machine IR's `invoke_types` list and
//! the `COMP004` compile gate.

#[allow(
    dead_code,
    reason = "the shared helpers also serve validate_covers_lowering.rs"
)]
mod common;

use scxml_compiler_core::{
    CompileInput, Compiler, LowerError, MachineIr, SCXML_NAMESPACE, ScriptResolution, lower,
};
use scxml_syntax::{SourceId, SourceText, parse_xml};

const ECHO: &str = "https://stateforward.org/scxml/services/echo";

fn machine(invokes: &str) -> String {
    format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" initial="a"><state id="a">{invokes}<transition event="go" target="b"/></state><state id="b"><invoke type="urn:z:svc"/><invoke type="{ECHO}"/></state></scxml>"#
    )
}

fn lowered(source: &str) -> MachineIr {
    let document = parse_xml(SourceText::new(
        SourceId::new(1),
        "m.scxml",
        source.to_owned(),
    ))
    .expect("well formed");
    lower(&document).expect("lowers")
}

#[test]
fn invoke_types_list_the_literal_service_types_sorted_and_unique() {
    let ir = lowered(&machine(&format!(
        r#"<invoke type="{ECHO}"/><invoke type="scxml" src="c.scxml"/><invoke typeexpr="'urn:x:dynamic'"/>"#
    )));
    assert_eq!(ir.invoke_types, [ECHO, "urn:z:svc"]);
    let bytes = ir.to_json().expect("encodes");
    assert_eq!(MachineIr::from_json(&bytes).expect("decodes"), ir);
}

#[test]
fn a_machine_without_services_encodes_no_invoke_types_key() {
    let source = format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0"><state id="a"><invoke type="scxml" src="c.scxml"/></state></scxml>"#
    );
    let ir = lowered(&source);
    assert!(ir.invoke_types.is_empty());
    let text = String::from_utf8(ir.to_json().expect("encodes")).expect("utf8");
    assert!(!text.contains("invoke_types"), "{text}");
}

#[test]
fn decoded_invoke_types_must_match_the_literal_service_types() {
    let mut ir = lowered(&machine(""));
    ir.invoke_types.pop();
    assert!(matches!(
        ir.validate(),
        Err(LowerError::InvalidStructure(_))
    ));
    let mut ir = lowered(&machine(""));
    ir.invoke_types.reverse();
    assert!(matches!(
        ir.validate(),
        Err(LowerError::InvalidStructure(_))
    ));
    let mut ir = lowered(&machine(""));
    ir.invoke_types.push("urn:zz:extra".into());
    assert!(matches!(
        ir.validate(),
        Err(LowerError::InvalidStructure(_))
    ));
}

fn compile(source: &str, bound: Option<&[&str]>) -> scxml_compiler_core::CompileOutput {
    Compiler::new(common::validator()).compile(CompileInput {
        source_name: "case.scxml".into(),
        source: source.into(),
        packages: Vec::new(),
        package_schemas: Vec::new(),
        scripts: Vec::new(),
        script_resolution: ScriptResolution::Compile,
        service_types: bound.map(|bound| bound.iter().map(|uri| (*uri).to_owned()).collect()),
        event_schemas: std::collections::BTreeMap::new(),
    })
}

fn codes(output: &scxml_compiler_core::CompileOutput) -> Vec<String> {
    output
        .validation
        .report
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code.clone())
        .collect()
}

#[test]
fn compile_rejects_each_unbound_literal_service_type_with_comp004() {
    let source = machine("");
    let output = compile(&source, Some(&[]));
    assert!(output.artifact.is_none());
    assert_eq!(codes(&output), ["COMP004", "COMP004"]);
    let output = compile(&source, Some(&[ECHO]));
    assert!(output.artifact.is_none());
    assert_eq!(codes(&output), ["COMP004"]);
    let output = compile(&source, Some(&[ECHO, "urn:z:svc"]));
    assert!(output.artifact.is_some(), "{:?}", codes(&output));
    // The compiler component (run-time child compilation) judges no binding.
    assert!(compile(&source, None).artifact.is_some());
}

#[test]
fn invoked_services_lower_their_env_slots_sorted_by_slot() {
    use scxml_compiler_core::EnvSlotIr;
    let env = scxml_types::ENV_NAMESPACE_URI;
    let ir = lowered(&machine(&format!(
        r#"<invoke xmlns:env="{env}" type="{ECHO}" env:token="TOKEN" env:baseurlexpr="'BASE'"/>"#
    )));
    let invoke = &ir.states[0].invokes[0];
    assert_eq!(
        invoke.env,
        [
            EnvSlotIr::Expr {
                slot: "baseurl".into(),
                expr: "'BASE'".into()
            },
            EnvSlotIr::Name {
                slot: "token".into(),
                name: "TOKEN".into()
            },
        ]
    );
    let bytes = ir.to_json().expect("encodes");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON");
    assert_eq!(
        json["states"][0]["invokes"][0]["env"],
        serde_json::json!([
            {"kind": "expr", "slot": "baseurl", "expr": "'BASE'"},
            {"kind": "name", "slot": "token", "name": "TOKEN"}
        ])
    );
    assert_eq!(MachineIr::from_json(&bytes).expect("decodes"), ir);
    // Without slots the encoding has no `env` key: existing Machine IR bytes
    // are unchanged.
    assert!(
        !String::from_utf8(lowered(&machine("")).to_json().expect("encodes"))
            .expect("utf8")
            .contains("\"env\"")
    );
    // A decoded binding on an SCXML session's invoke, out of order, or
    // naming no variable is rejected.
    for tamper in [
        |ir: &mut MachineIr| ir.states[0].invokes[0].type_value = Some("scxml".into()),
        |ir: &mut MachineIr| ir.states[0].invokes[0].env.reverse(),
        |ir: &mut MachineIr| {
            ir.states[0].invokes[0].env[1] = EnvSlotIr::Name {
                slot: "token".into(),
                name: "1TOKEN".into(),
            };
        },
    ] {
        let mut tampered = ir.clone();
        tamper(&mut tampered);
        assert!(tampered.validate().is_err());
    }
}
