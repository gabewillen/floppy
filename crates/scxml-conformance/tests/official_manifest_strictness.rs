#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

use scxml_conformance::parse_irp_manifest;

#[test]
fn rejects_unknown_elements_in_every_structural_context() {
    let adversarial = [
        (
            "root new-case",
            r#"<assertions baseuri="https://example.invalid"><new-case/></assertions>"#,
        ),
        (
            "assertion new-case",
            r#"<assertions baseuri="https://example.invalid"><assert id="1" specnum="1" specid="x"><new-case/></assert></assertions>"#,
        ),
        (
            "test dependency alias",
            r#"<assertions baseuri="https://example.invalid"><assert id="1" specnum="1" specid="x"><test id="1" conformance="mandatory" manual="false"><start uri="case.txml"/><dependency uri="dep.txml"/></test></assert></assertions>"#,
        ),
        (
            "start child",
            r#"<assertions baseuri="https://example.invalid"><assert id="1" specnum="1" specid="x"><test id="1" conformance="mandatory" manual="false"><start uri="case.txml"><new-case/></start></test></assert></assertions>"#,
        ),
        (
            "dependency child",
            r#"<assertions baseuri="https://example.invalid"><assert id="1" specnum="1" specid="x"><test id="1" conformance="mandatory" manual="false"><start uri="case.txml"/><dep uri="dep.txml"><new-case/></dep></test></assert></assertions>"#,
        ),
        (
            "namespaced assertion lookalike",
            r#"<assertions xmlns:adversary="urn:adversary" baseuri="https://example.invalid"><adversary:assert id="1" specnum="1" specid="x"/></assertions>"#,
        ),
    ];

    for (label, xml) in adversarial {
        let error = parse_irp_manifest("manifest.xml", None, xml.as_bytes())
            .expect_err("unknown structural elements must fail closed");
        assert_eq!(error.code, "irp-unknown-element", "fixture: {label}");
    }
}

#[test]
fn rejects_a_namespaced_root_instead_of_matching_only_its_local_name() {
    let error = parse_irp_manifest(
        "manifest.xml",
        None,
        br#"<assertions xmlns="urn:adversary" baseuri="https://example.invalid"/>"#,
    )
    .expect_err("a namespaced root lookalike must fail closed");

    assert_eq!(error.code, "irp-manifest-root");
}

#[test]
fn rejects_unknown_attributes_in_every_structural_context() {
    let adversarial = [
        (
            "root attribute",
            r#"<assertions baseuri="https://example.invalid" new-case="true"><assert id="1" specnum="1" specid="x"><test id="1" conformance="mandatory" manual="false"><start uri="case.txml"/></test></assert></assertions>"#,
        ),
        (
            "assertion attribute",
            r#"<assertions baseuri="https://example.invalid"><assert id="1" specnum="1" specid="x" new-case="true"><test id="1" conformance="mandatory" manual="false"><start uri="case.txml"/></test></assert></assertions>"#,
        ),
        (
            "test attribute",
            r#"<assertions baseuri="https://example.invalid"><assert id="1" specnum="1" specid="x"><test id="1" conformance="mandatory" manual="false" new-case="true"><start uri="case.txml"/></test></assert></assertions>"#,
        ),
        (
            "start attribute",
            r#"<assertions baseuri="https://example.invalid"><assert id="1" specnum="1" specid="x"><test id="1" conformance="mandatory" manual="false"><start uri="case.txml" new-case="true"/></test></assert></assertions>"#,
        ),
        (
            "dependency attribute",
            r#"<assertions baseuri="https://example.invalid"><assert id="1" specnum="1" specid="x"><test id="1" conformance="mandatory" manual="false"><start uri="case.txml"/><dep uri="dep.txml" new-case="true"/></test></assert></assertions>"#,
        ),
        (
            "namespaced id lookalike",
            r#"<assertions xmlns:adversary="urn:adversary" baseuri="https://example.invalid"><assert id="1" adversary:id="2" specnum="1" specid="x"><test id="1" conformance="mandatory" manual="false"><start uri="case.txml"/></test></assert></assertions>"#,
        ),
    ];

    for (label, xml) in adversarial {
        let error = parse_irp_manifest("manifest.xml", None, xml.as_bytes())
            .expect_err("unknown structural attributes must fail closed");
        assert_eq!(error.code, "irp-unknown-attribute", "fixture: {label}");
    }
}

#[test]
fn accepts_the_exact_pinned_manifest() {
    let bytes = include_bytes!("../../../tests/conformance/w3c-irp/upstream/manifest.xml");
    let manifest = parse_irp_manifest(
        "manifest.xml",
        Some("https://www.w3.org/Voice/2013/scxml-irp"),
        bytes,
    )
    .expect("the exact pinned W3C IRP manifest must remain accepted");

    assert_eq!(manifest.assertions.len(), 200);
    assert_eq!(
        manifest
            .assertions
            .iter()
            .flat_map(|assertion| &assertion.tests)
            .flat_map(|test| &test.starts)
            .count(),
        202
    );
    assert!(
        manifest
            .assertions
            .iter()
            .flat_map(|assertion| &assertion.tests)
            .all(|test| test.rejects.is_empty()),
        "the W3C IRP declares no document rejection"
    );
}

/// A namespace manifest's `<reject code="..."/>` children are the sorted,
/// distinct diagnostic codes a document must be rejected with; a code that is
/// not a diagnostic code, an unknown attribute, or a child fails closed.
#[test]
fn reject_children_carry_exact_diagnostic_codes() {
    let manifest = |rejects: &str| {
        format!(
            r#"<assertions baseuri="https://example.invalid"><assert id="1" specnum="1" specid="x"><test id="1" conformance="mandatory" manual="false"><start uri="case.txml"/>{rejects}</test></assert></assertions>"#
        )
    };
    let parsed = parse_irp_manifest(
        "manifest.xml",
        None,
        manifest(r#"<reject code="EVENT006"/><reject code="EVENT003"/><reject code="EVENT006"/>"#)
            .as_bytes(),
    )
    .expect("reject children parse");
    assert_eq!(
        parsed.assertions[0].tests[0].rejects,
        ["EVENT003", "EVENT006"]
    );
    for (label, rejects, code) in [
        (
            "lowercase code",
            r#"<reject code="event003"/>"#,
            "irp-test-reject",
        ),
        (
            "empty code",
            r#"<reject code=""/>"#,
            "irp-required-attribute",
        ),
        (
            "unknown attribute",
            r#"<reject code="EVENT003" note="x"/>"#,
            "irp-unknown-attribute",
        ),
        (
            "child",
            r#"<reject code="EVENT003"><x/></reject>"#,
            "irp-unknown-element",
        ),
    ] {
        let error = parse_irp_manifest("manifest.xml", None, manifest(rejects).as_bytes())
            .expect_err("a malformed reject fails closed");
        assert_eq!(error.code, code, "fixture: {label}");
    }
}
