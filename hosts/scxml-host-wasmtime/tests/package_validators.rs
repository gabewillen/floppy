//! Package validator components under the Wasmtime host: the example
//! package validator (`components/scxml-package-validator-example`) bound in
//! [`PackageValidators`], called directly and through the validator
//! component's `package-validator-api` import
//! ([`ComponentValidator::with_package_validators`]).
//!
//! `scripts/verify-wasm-capabilities.sh` builds and virtualizes the release
//! validator, the example package validator, and the example extension
//! component.

#![cfg(feature = "wasmtime")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use scxml_host_wasmtime::validator_types::{
    Applicability, Attribute, Context, Element, ExtensionPackage, QualifiedName, SchemaPolicy,
    SchemaResource, Span, ValidationRequest,
};
use scxml_host_wasmtime::{
    ArtifactError, ArtifactKind, AuthenticatedComponent, ComponentValidator, HostError,
    PackageValidators, TelemetryProviders, TrustedComponentDigest, TrustedManifestDigest,
    WasmtimeHostConfig, authenticate_component, authenticate_package_validator_component,
};
use sha2::{Digest as _, Sha256};

const EXAMPLE: &str = "https://stateforward.org/scxml/example";

struct Artifacts {
    workspace: PathBuf,
    validator: AuthenticatedComponent,
    example: AuthenticatedComponent,
    extension_bytes: Vec<u8>,
}

fn artifacts() -> &'static Artifacts {
    static ARTIFACTS: OnceLock<Artifacts> = OnceLock::new();
    ARTIFACTS.get_or_init(build_artifacts)
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()))
}

fn build_artifacts() -> Artifacts {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("host crate has a workspace parent")
        .to_path_buf();
    let directory = workspace.join("target/validator-proof/wasm-artifacts/package-validators");
    let status = Command::new("sh")
        .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
        .args([
            "scxml_extension_example",
            "scxml_package_validator_example",
            "scxml_validator",
        ])
        .current_dir(&workspace)
        .env(
            "CARGO_TARGET_DIR",
            workspace.join("target/session-host-proof"),
        )
        .env("SCXML_WASM_ARTIFACTS_DIR", &directory)
        .status()
        .expect("the capability script runs");
    assert!(status.success(), "the components must verify");
    let manifest = read(&directory.join("manifest.tsv"));
    let trusted = TrustedManifestDigest::from_hex(&format!("{:x}", Sha256::digest(&manifest)))
        .expect("manifest digest is canonical");
    let authenticate = |kind, name: &str| {
        let path = format!("{name}.virtualized.wasm");
        authenticate_component(
            &manifest,
            trusted,
            kind,
            &path,
            &read(&directory.join(&path)),
        )
        .expect("the virtualized component authenticates")
    };
    Artifacts {
        validator: authenticate(ArtifactKind::Validator, "scxml_validator"),
        example: authenticate(
            ArtifactKind::PackageValidatorExample,
            "scxml_package_validator_example",
        ),
        extension_bytes: read(&directory.join("scxml_extension_example.virtualized.wasm")),
        workspace,
    }
}

fn config() -> WasmtimeHostConfig {
    let mut config = WasmtimeHostConfig::default();
    config.store.memory_bytes = 512 * 1024 * 1024;
    config.store.fuel = 200_000_000_000;
    config
}

/// The example package validator bound to the example namespace, on the
/// default fuel bound.
fn bound() -> PackageValidators {
    let mut validators =
        PackageValidators::new(&WasmtimeHostConfig::default(), &TelemetryProviders::new())
            .expect("the host starts");
    validators
        .bind(EXAMPLE, &artifacts().example)
        .expect("the example package validator binds");
    validators
}

fn name(namespace: Option<&str>, local: &str) -> QualifiedName {
    QualifiedName {
        namespace_uri: namespace.map(str::to_owned),
        local_name: local.to_owned(),
    }
}

/// A context whose one target, element 1, is `<example:raise>` with
/// `attributes`.
fn context(attributes: &[(&str, &str)]) -> Context {
    let span = Span { start: 0, end: 10 };
    Context {
        elements: vec![
            Element {
                name: name(Some("http://www.w3.org/2005/07/scxml"), "scxml"),
                span: Span { start: 0, end: 100 },
                attributes: Vec::new(),
                children: vec![1],
                parent: None,
            },
            Element {
                name: name(Some(EXAMPLE), "raise"),
                span,
                attributes: attributes
                    .iter()
                    .map(|(local, value)| Attribute {
                        name: name(None, local),
                        value: (*value).to_owned(),
                        span: Span { start: 2, end: 8 },
                    })
                    .collect(),
                children: Vec::new(),
                parent: Some(0),
            },
        ],
        data_ids: Vec::new(),
        state_ids: vec!["idle".to_owned(), "busy".to_owned()],
        transitions: Vec::new(),
        datamodel: None,
    }
}

#[test]
fn the_example_reports_an_unknown_state_with_a_machine_applicable_fix() {
    let diagnostics = bound()
        .validate(EXAMPLE, &context(&[("in", "bsy")]), &[1])
        .expect("the call succeeds");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code, "unknown_state");
    assert_eq!(
        diagnostics[0].fixes[0].applicability,
        Applicability::MachineApplicable
    );
    assert_eq!(diagnostics[0].fixes[0].edits[0].replacement, "in=\"busy\"");
    assert!(
        bound()
            .validate(EXAMPLE, &context(&[("in", "busy")]), &[1])
            .expect("the call succeeds")
            .is_empty()
    );
    // An unbound package answers an empty list.
    assert!(
        bound()
            .validate("urn:other", &context(&[("in", "bsy")]), &[1])
            .expect("the call succeeds")
            .is_empty()
    );
}

#[test]
fn traps_and_fuel_exhaustion_fail_the_call() {
    for event in ["example.validator.trap", "example.validator.spin"] {
        let result = bound().validate(EXAMPLE, &context(&[("event", event)]), &[1]);
        assert!(
            matches!(result, Err(HostError::Wasmtime(_))),
            "{event}: {result:?}"
        );
    }
}

#[test]
fn a_component_that_imports_anything_is_refused() {
    let bytes = &artifacts().extension_bytes;
    let digest = TrustedComponentDigest::from_hex(&format!("{:x}", Sha256::digest(bytes)))
        .expect("digest is canonical");
    let extension = authenticate_package_validator_component(bytes, digest)
        .expect("the bytes authenticate by digest");
    let mut validators =
        PackageValidators::new(&WasmtimeHostConfig::default(), &TelemetryProviders::new())
            .expect("the host starts");
    assert!(matches!(
        validators.bind(EXAMPLE, &extension),
        Err(HostError::Artifact(ArtifactError::UnknownImport))
    ));
    validators
        .bind(EXAMPLE, &artifacts().example)
        .expect("the example binds");
    assert!(matches!(
        validators.bind(EXAMPLE, &artifacts().example),
        Err(HostError::Artifact(ArtifactError::NamespaceBinding))
    ));
}

/// The validator component's report for `body` inside a state `s`, the
/// example namespace bound to its package and the example validator.
fn component_report(body: &str) -> serde_json::Value {
    let schema = read(
        &artifacts()
            .workspace
            .join("components/scxml-extension-example/package/example.xsd"),
    );
    let source = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:ex="{EXAMPLE}" version="1.0" initial="busy"><final id="busy"><onentry>{body}</onentry></final></scxml>"#
    );
    let validator = ComponentValidator::new(
        &artifacts().validator,
        None,
        &config(),
        &TelemetryProviders::new(),
    )
    .expect("the validator links")
    .with_package_validators(Arc::new(bound()));
    let bytes = validator
        .validate(&ValidationRequest {
            source_name: "doc.scxml".into(),
            source,
            policy: SchemaPolicy::Strict,
            schemas: vec![SchemaResource {
                namespace_uri: EXAMPLE.into(),
                system_id: "example.xsd".into(),
                bytes: schema.clone(),
            }],
            packages: vec![ExtensionPackage {
                namespace_uri: EXAMPLE.into(),
                package_version: "0.2.0".into(),
                extension_artifact: "scxml_extension_example.virtualized.wasm".into(),
                schema_artifact: "example.xsd".into(),
                schema_sha256: format!("{:x}", Sha256::digest(&schema)),
                wit_world: "stateforward:scxml/executable-content-extension".into(),
                wit_version: "0.2.0".into(),
                validator_artifact: None,
                validator_sha256: None,
            }],
            event_schemas: Vec::new(),
            invoked_documents: Vec::new(),
            trace_context: None,
        })
        .expect("the validator answers");
    serde_json::from_slice(&bytes).expect("the report is JSON")
}

fn codes(report: &serde_json::Value) -> Vec<String> {
    report["validator"]["diagnostics"]
        .as_array()
        .expect("the report lists diagnostics")
        .iter()
        .map(|diagnostic| diagnostic["code"].as_str().expect("a code").to_owned())
        .collect()
}

#[test]
fn the_validator_component_renders_package_codes_with_the_document_prefix() {
    let report = component_report(r#"<ex:raise event="e" in="bsy"/>"#);
    assert_eq!(codes(&report), ["ex::unknown_state"], "{report}");
    let clean = component_report(r#"<ex:raise event="e" in="busy"/>"#);
    assert!(codes(&clean).is_empty(), "{clean}");
}

#[test]
fn the_validator_component_fails_closed_when_a_package_validator_traps() {
    let report = component_report(r#"<ex:raise event="example.validator.trap"/>"#);
    assert_eq!(codes(&report), ["PKG002"], "{report}");
}
