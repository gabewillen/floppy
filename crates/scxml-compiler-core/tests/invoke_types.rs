//! Invoked-service `<invoke>`s in Machine IR: any literal type lowers (an
//! invoke type is a runtime action, SCXML 6.4.1), and the `env` attributes
//! of an invoked service's `<invoke>` lower as authored.

use scxml_compiler_core::{ExpandedNameIr, MachineIr, SCXML_NAMESPACE, XmlAttributeIr, lower};
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
fn any_literal_invoke_type_lowers_and_is_judged_when_the_invoke_runs() {
    // SCXML 6.4.1 allows platform-specific invoke types, so a type that is
    // not a URI lowers like any other.
    let ir = lowered(&machine(r#"<invoke type="echo"/>"#));
    assert_eq!(ir.states[0].invokes[0].type_value.as_deref(), Some("echo"));
    let bytes = ir.to_json().expect("encodes");
    assert_eq!(MachineIr::from_json(&bytes).expect("decodes"), ir);
}

fn env_attribute(local_name: &str, value: &str) -> XmlAttributeIr {
    XmlAttributeIr {
        name: ExpandedNameIr {
            namespace_uri: Some(scxml_types::ENV_NAMESPACE_URI.to_owned()),
            local_name: local_name.to_owned(),
        },
        value: value.to_owned(),
    }
}

#[test]
fn invoked_services_lower_their_env_attributes_sorted_by_local_name() {
    let env = scxml_types::ENV_NAMESPACE_URI;
    let ir = lowered(&machine(&format!(
        r#"<invoke xmlns:env="{env}" type="{ECHO}" env:token="TOKEN" env:baseurlexpr="'BASE'"/>"#
    )));
    let invoke = &ir.states[0].invokes[0];
    assert_eq!(
        invoke.attributes,
        [
            env_attribute("baseurlexpr", "'BASE'"),
            env_attribute("token", "TOKEN"),
        ]
    );
    let bytes = ir.to_json().expect("encodes");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON");
    assert_eq!(
        json["states"][0]["invokes"][0]["attributes"][1],
        serde_json::json!({"name": {"namespace_uri": env, "local_name": "token"}, "value": "TOKEN"})
    );
    assert_eq!(MachineIr::from_json(&bytes).expect("decodes"), ir);
    // Without env attributes the encoding has no `attributes` key: existing
    // Machine IR bytes are unchanged.
    assert!(
        !String::from_utf8(lowered(&machine("")).to_json().expect("encodes"))
            .expect("utf8")
            .contains("\"attributes\"")
    );
    // A decoded attribute on an SCXML session's invoke, out of order, outside
    // the env namespace, binding one slot twice, or naming no variable is
    // rejected.
    for tamper in [
        |ir: &mut MachineIr| ir.states[0].invokes[0].type_value = Some("scxml".into()),
        |ir: &mut MachineIr| ir.states[0].invokes[0].attributes.reverse(),
        |ir: &mut MachineIr| ir.states[0].invokes[0].attributes[1].name.namespace_uri = None,
        |ir: &mut MachineIr| {
            ir.states[0].invokes[0].attributes[1] = env_attribute("baseurl", "BASE");
            ir.states[0].invokes[0].attributes.swap(0, 1);
        },
        |ir: &mut MachineIr| ir.states[0].invokes[0].attributes[1].value = "1TOKEN".into(),
    ] {
        let mut tampered = ir.clone();
        tamper(&mut tampered);
        assert!(tampered.validate().is_err());
    }
}
