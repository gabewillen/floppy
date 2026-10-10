//! The validator Wasm component, its `expression-syntax` import linked by
//! the host adapter to a real data-model component, against the CLI.
//!
//! `scripts/verify-wasm-capabilities.sh` builds and virtualizes the release
//! validator and data-model guests. Every `tests/ui` case whose machines use
//! exactly one checked data model (`ecmascript` or `xpath`) and no extra CLI
//! arguments is validated through a [`ComponentValidator`] with that data
//! model's component linked, and the component's canonical report bytes must
//! equal the case's `.json` snapshot, which `scx validate --format
//! json` (linking the native ECMAScript and `XPath` checkers) produced. So the
//! component reports the same expression diagnostics (codes, spans, labels,
//! notes, help, and fixes) as the CLI, not merely the same codes.
//!
//! The same component linked with no data model (or with the null data
//! model, which has no expression syntax) must report the unchecked
//! expressions with the `LINT010` note instead of checking them.

#![cfg(feature = "wasmtime")]

mod prebuilt;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use scxml_host_wasmtime::validator_types::{
    EventSchemaResource, InvokedDocument, SchemaPolicy, ScriptResource, ValidationRequest,
};
use scxml_host_wasmtime::{
    ArtifactKind, AuthenticatedComponent, ComponentValidator, TelemetryProviders,
    TrustedManifestDigest, WasmtimeHostConfig, authenticate_component,
};
use sha2::{Digest as _, Sha256};

struct Artifacts {
    workspace: PathBuf,
    validator: AuthenticatedComponent,
    ecmascript: AuthenticatedComponent,
    xpath: AuthenticatedComponent,
    null: AuthenticatedComponent,
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
    let target = workspace.join("target/validator-proof");
    let directory = prebuilt::release_dir().unwrap_or_else(|| {
        let directory = target.join("wasm-artifacts/validator-component");
        let status = Command::new("sh")
            .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
            .args([
                "scxml_data_model_ecmascript",
                "scxml_data_model_null",
                "scxml_data_model_xpath",
                "scxml_validator",
            ])
            .current_dir(&workspace)
            // The Cargo target every host suite builds the release components
            // in, so they compile once; this suite's artifacts stay its own.
            .env(
                "CARGO_TARGET_DIR",
                workspace.join("target/session-host-proof"),
            )
            .env("SCXML_WASM_ARTIFACTS_DIR", &directory)
            .status()
            .expect("the capability script runs");
        assert!(
            status.success(),
            "validator and data-model components must verify"
        );
        directory
    });
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
        ecmascript: authenticate(
            ArtifactKind::DataModelEcmascript,
            "scxml_data_model_ecmascript",
        ),
        xpath: authenticate(ArtifactKind::DataModelXpath, "scxml_data_model_xpath"),
        null: authenticate(ArtifactKind::DataModelNull, "scxml_data_model_null"),
        workspace,
    }
}

/// Host limits for whole-document validation: the bundled W3C schema set is
/// compiled inside the guest, which needs more memory and fuel than one
/// machine step.
fn config() -> WasmtimeHostConfig {
    let mut config = WasmtimeHostConfig::default();
    config.store.memory_bytes = 512 * 1024 * 1024;
    config.store.fuel = 200_000_000_000;
    config
}

/// One compiled validator with `data_model` linked (compiled once and
/// reused: every request still runs in fresh stores).
fn linked(data_model: Option<&AuthenticatedComponent>) -> ComponentValidator {
    ComponentValidator::new(
        &artifacts().validator,
        data_model,
        &config(),
        &TelemetryProviders::new(),
    )
    .expect("the validator component compiles and links")
}

/// The schema files the UI cases' `event:schema` references name, relative
/// to `tests/ui`, supplied as the CLI reads them for a document there.
fn ui_event_schemas() -> Vec<EventSchemaResource> {
    let directory = artifacts().workspace.join("tests/ui/event-schemas");
    let mut resources: Vec<EventSchemaResource> = std::fs::read_dir(&directory)
        .expect("tests/ui/event-schemas exists")
        .map(|entry| {
            let path = entry.expect("schema entry is readable").path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .expect("schema file names are UTF-8")
                .to_owned();
            EventSchemaResource {
                uri: format!("event-schemas/{name}"),
                bytes: read(&path),
            }
        })
        .collect();
    resources.sort_by(|left, right| left.uri.cmp(&right.uri));
    resources
}

/// The documents under `tests/ui/invoked/` the case `source`'s literal
/// `src` attributes name, supplied as `scx validate` reads them (their
/// schemas are inline, so they reference no files).
fn ui_invoked_documents(source: &str) -> Vec<InvokedDocument> {
    let ui = artifacts().workspace.join("tests/ui");
    let mut documents: Vec<InvokedDocument> = Vec::new();
    for (src, _) in source
        .split("src=\"")
        .skip(1)
        .filter_map(|rest| rest.split_once('"'))
    {
        if src.starts_with("invoked/") && documents.iter().all(|document| document.src != src) {
            documents.push(InvokedDocument {
                src: src.to_owned(),
                text: String::from_utf8(read(&ui.join(src)))
                    .expect("the invoked document is UTF-8"),
                event_schemas: Vec::new(),
            });
        }
    }
    documents
}

/// The files under `tests/ui/scripts/` the case `source`'s literal `src`
/// attributes name, supplied as `scx validate` reads them.
fn ui_scripts(source: &str) -> Vec<ScriptResource> {
    let ui = artifacts().workspace.join("tests/ui");
    let mut scripts: Vec<ScriptResource> = Vec::new();
    for (src, _) in source
        .split("src=\"")
        .skip(1)
        .filter_map(|rest| rest.split_once('"'))
    {
        if src.starts_with("scripts/") && scripts.iter().all(|script| script.src != src) {
            scripts.push(ScriptResource {
                src: src.to_owned(),
                text: String::from_utf8(read(&ui.join(src))).expect("the script is UTF-8"),
            });
        }
    }
    scripts
}

fn validate(validator: &ComponentValidator, source_name: &str, source: &str) -> String {
    let request = ValidationRequest {
        source_name: source_name.to_owned(),
        source: source.to_owned(),
        policy: SchemaPolicy::Strict,
        schemas: Vec::new(),
        packages: Vec::new(),
        event_schemas: ui_event_schemas(),
        invoked_documents: ui_invoked_documents(source),
        scripts: ui_scripts(source),
        trace_context: None,
    };
    let bytes = validator
        .validate(&request)
        .expect("the validator component returns a canonical report");
    String::from_utf8(bytes).expect("canonical report bytes are UTF-8")
}

/// The one checked data model every machine of `source` declares, if any.
fn single_checked_data_model(source: &str) -> Option<&'static str> {
    let declares = |name: &str| source.contains(&format!("datamodel=\"{name}\""));
    match (declares("ecmascript"), declares("xpath")) {
        (true, false) => Some("ecmascript"),
        (false, true) => Some("xpath"),
        _ => None,
    }
}

#[test]
fn component_expression_diagnostics_match_the_cli_snapshots() {
    let artifacts = artifacts();
    let ui = artifacts.workspace.join("tests/ui");
    let mut cases: Vec<String> = std::fs::read_dir(&ui)
        .expect("tests/ui exists")
        .filter_map(|entry| {
            let path = entry.expect("tests/ui entry is readable").path();
            (path.extension().is_some_and(|extension| extension == "scx"))
                .then(|| path.file_stem()?.to_str().map(str::to_owned))
                .flatten()
        })
        .collect();
    cases.sort();
    let ecmascript = linked(Some(&artifacts.ecmascript));
    let xpath = linked(Some(&artifacts.xpath));
    let mut compared = Vec::new();
    let mut mismatches = Vec::new();
    for case in cases {
        let source =
            String::from_utf8(read(&ui.join(format!("{case}.scx")))).expect("UI case is UTF-8");
        // Extra CLI arguments (schemas, policies) have no request field here.
        if source.trim_start().starts_with("<?ui-args") {
            continue;
        }
        let Some(data_model) = single_checked_data_model(&source) else {
            continue;
        };
        let validator = if data_model == "xpath" {
            &xpath
        } else {
            &ecmascript
        };
        let expected = String::from_utf8(read(&ui.join(format!("{case}.json"))))
            .expect("UI snapshot is UTF-8");
        let actual = validate(validator, &format!("tests/ui/{case}.scx"), &source);
        if actual.trim_end() != expected.trim_end() {
            mismatches.push(format!(
                "{case}\n  cli:       {}\n  component: {}",
                expected.trim_end(),
                actual.trim_end()
            ));
        }
        compared.push(case);
    }
    assert!(
        mismatches.is_empty(),
        "component reports differ from the CLI:\n{}",
        mismatches.join("\n")
    );
    // Both data models and the expression rules are exercised.
    for required in [
        "scxml038-expression-syntax-error",
        "scxml038-ecmascript-mixed-word-operators",
        "scxml038-xpath-mixed-habits",
        "scxml038-xpath-syntax-paths",
        "scxml039-location-not-assignable",
        "scxml039-xpath-location-not-a-path",
        "limit003-expression-over-bound",
        "limit003-xpath-over-nesting-bound",
        "scxml074-log-side-effects",
        "event011-incompatible-invoked-event-data",
        "lint020-condition-side-effects",
        "scxml077-system-variable-writes",
        "lint019-undecidable-side-effects",
        "valid-scxml074-local-writes",
        "scxml056-xpath-undeclared-variable-typo",
        "lint011-xpath-bare-true",
        "lint013-xpath-zero-position",
        "lint014-xpath-string-addition",
    ] {
        assert!(
            compared.iter().any(|case| case == required),
            "{required} must be compared; compared: {compared:?}"
        );
    }
}

/// `invoked-documents` is bounded at the request boundary: a repeated or
/// empty `src`, more than 64 documents, or more than 16 MiB of text and
/// schema bytes together is `invalid-request`, and the instance stays
/// usable.
#[test]
fn invoked_documents_are_bounded_at_the_request_boundary() {
    let validator = linked(Some(&artifacts().ecmascript));
    let document = |src: &str, text: String| InvokedDocument {
        src: src.to_owned(),
        text,
        event_schemas: Vec::new(),
    };
    let request = |invoked_documents: Vec<InvokedDocument>| {
        ValidationRequest {
        source_name: "parent.scx".to_owned(),
        source: r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><final id="f"/></scxml>"#.to_owned(),
        policy: SchemaPolicy::Strict,
        schemas: Vec::new(),
        packages: Vec::new(),
        event_schemas: Vec::new(),
        invoked_documents,
        scripts: Vec::new(),
        trace_context: None,
    }
    };
    for (what, invoked) in [
        (
            "repeated src",
            vec![
                document("a.scx", String::new()),
                document("a.scx", String::new()),
            ],
        ),
        ("empty src", vec![document("", String::new())]),
        (
            "65 documents",
            (0..65)
                .map(|index| document(&format!("{index}.scx"), String::new()))
                .collect(),
        ),
        (
            "over 16 MiB",
            vec![
                document("a.scx", "x".repeat(8 * 1024 * 1024)),
                document("b.scx", "x".repeat(8 * 1024 * 1024 + 1)),
            ],
        ),
    ] {
        assert!(validator.validate(&request(invoked)).is_err(), "{what}");
    }
    let at_bound = (0..64)
        .map(|index| document(&format!("{index}.scx"), String::new()))
        .collect();
    assert!(validator.validate(&request(at_bound)).is_ok());
}

/// `scripts` is bounded at the request boundary: a repeated or empty
/// `src`, more than 64 files, or more than 16 MiB of text together is
/// `invalid-request`, and the instance stays usable.
#[test]
fn scripts_are_bounded_at_the_request_boundary() {
    let validator = linked(Some(&artifacts().ecmascript));
    let script = |src: &str, text: String| ScriptResource {
        src: src.to_owned(),
        text,
    };
    let request = |scripts: Vec<ScriptResource>| {
        ValidationRequest {
        source_name: "document.scx".to_owned(),
        source: r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><final id="f"/></scxml>"#.to_owned(),
        policy: SchemaPolicy::Strict,
        schemas: Vec::new(),
        packages: Vec::new(),
        event_schemas: Vec::new(),
        invoked_documents: Vec::new(),
        scripts,
        trace_context: None,
    }
    };
    for (what, scripts) in [
        (
            "repeated src",
            vec![script("a.js", String::new()), script("a.js", String::new())],
        ),
        ("empty src", vec![script("", String::new())]),
        (
            "65 files",
            (0..65)
                .map(|index| script(&format!("{index}.js"), String::new()))
                .collect(),
        ),
        (
            "over 16 MiB",
            vec![
                script("a.js", "x".repeat(8 * 1024 * 1024)),
                script("b.js", "x".repeat(8 * 1024 * 1024 + 1)),
            ],
        ),
    ] {
        assert!(validator.validate(&request(scripts)).is_err(), "{what}");
    }
    let at_bound = (0..64)
        .map(|index| script(&format!("{index}.js"), String::new()))
        .collect();
    assert!(validator.validate(&request(at_bound)).is_ok());
}

/// Resource bounds apply to wasm builds only (`scxml_types::wasm_bound`), so
/// the two validators differ on purpose for a document over one: the native
/// CLI's snapshot is clean, while the validator component, a wasm build,
/// still reports the bound. The CLI side of each generated document is
/// `scx`'s `ui` test (`check_generated_budget_cases`).
#[test]
fn the_wasm_validator_keeps_the_bounds_the_native_cli_lifts() {
    const HEAD: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a" datamodel="ecmascript">"#;
    let artifacts = artifacts();
    let ecmascript = linked(Some(&artifacts.ecmascript));
    let reports = |report: &str, code: &str| report.contains(&format!("\"code\":\"{code}\""));

    let case = "valid-limit005-state-nesting-over-wasm-bound";
    let ui = artifacts.workspace.join("tests/ui");
    let source = String::from_utf8(read(&ui.join(format!("{case}.scx")))).expect("UTF-8");
    let native = String::from_utf8(read(&ui.join(format!("{case}.json")))).expect("UTF-8");
    assert!(!reports(&native, "LIMIT005"), "{native}");
    let wasm = validate(&ecmascript, &format!("tests/ui/{case}.scx"), &source);
    assert!(reports(&wasm, "LIMIT005"), "{wasm}");

    let chain = (0..65_536).fold(
        format!("{HEAD}\n<state id=\"a\"><transition event=\"go\" target=\"s0\"/></state>\n"),
        |mut chain, index| {
            let _ = writeln!(
                chain,
                "<state id=\"s{index}\"><transition event=\"go\" target=\"s{}\"/></state>",
                index + 1
            );
            chain
        },
    ) + "<final id=\"s65536\"/>\n</scxml>";
    assert!(reports(
        &validate(&ecmascript, "chain.scxml", &chain),
        "LIMIT002"
    ));

    let logs = format!(
        r#"{HEAD}<state id="a"><onentry>{}</onentry><onentry>{}</onentry><transition event="go" target="b"/></state><final id="b"/></scxml>"#,
        r#"<log expr="1"/>"#.repeat(8_193),
        r#"<log expr="1"/>"#.repeat(8_192)
    );
    assert!(reports(
        &validate(&ecmascript, "logs.scxml", &logs),
        "LIMIT004"
    ));

    let long = format!(
        r#"{HEAD}<state id="a"><onentry><log expr="1{}"/></onentry><transition event="go" target="b"/></state><final id="b"/></scxml>"#,
        " ".repeat(16_384)
    );
    assert!(reports(
        &validate(&ecmascript, "long.scxml", &long),
        "LIMIT003"
    ));
    // The XPath data model declines an expression over the same byte bound.
    let xpath = linked(Some(&artifacts.xpath));
    let long_xpath = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a" datamodel="xpath"><state id="a"><onentry><log expr="1{}"/></onentry><transition event="go" target="b"/></state><final id="b"/></scxml>"#,
        " ".repeat(16_384)
    );
    assert!(reports(
        &validate(&xpath, "long-xpath.scxml", &long_xpath),
        "LIMIT003"
    ));

    // One inline machine past the 64 nested machines the component checks.
    let root = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml">"#;
    let nested = root.repeat(65) + &"</scxml>".repeat(65);
    assert!(reports(
        &validate(&ecmascript, "nested.scxml", &nested),
        "LIMIT001"
    ));
}

/// The wasm validator bounds an attribute value as the compiler stores it:
/// a white-space-collapsing SCXML attribute by its collapsed value (an `id`
/// padded past 1 MiB is the short `a`), and an attribute of inline XML,
/// which the compiler keeps byte for byte, by its exact value (`LIMIT005`),
/// even on an element in the SCXML namespace.
#[test]
fn the_wasm_validator_bounds_attribute_values_as_the_compiler_stores_them() {
    const OVER_MIB: usize = 1024 * 1024 + 1;
    let artifacts = artifacts();
    let ecmascript = linked(Some(&artifacts.ecmascript));
    let reports = |report: &str, code: &str| report.contains(&format!("\"code\":\"{code}\""));
    let padding = " ".repeat(OVER_MIB);
    let padded_id = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a" datamodel="ecmascript"><state id="{padding}a"><transition event="go" target="b"/></state><final id="b"/></scxml>"#
    );
    let report = validate(&ecmascript, "padded-id.scxml", &padded_id);
    assert!(!reports(&report, "LIMIT005"), "{report}");
    let padded_payload = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a" datamodel="ecmascript"><state id="a"><onentry><send event="e"><content><state xmlns="http://www.w3.org/2005/07/scxml" id="{padding}x"/></content></send></onentry><transition event="go" target="b"/></state><final id="b"/></scxml>"#
    );
    let report = validate(&ecmascript, "padded-payload.scxml", &padded_payload);
    assert!(reports(&report, "LIMIT005"), "{report}");
}

#[test]
fn without_a_linked_checker_expressions_are_reported_unchecked() {
    let source = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript"><final id="f"><onentry><log expr="1 +"/></onentry></final></scxml>"#;
    for data_model in [None, Some(&artifacts().null)] {
        let report = validate(&linked(data_model), "unchecked.scxml", source);
        assert!(report.contains("\"code\":\"LINT010\""), "{report}");
        assert!(report.contains("\"severity\":\"note\""), "{report}");
        assert!(!report.contains("SCXML038"), "{report}");
    }
    // The ECMAScript component checks the same fragment.
    let report = validate(
        &linked(Some(&artifacts().ecmascript)),
        "checked.scxml",
        source,
    );
    assert!(report.contains("\"code\":\"SCXML038\""), "{report}");
    assert!(!report.contains("LINT010"), "{report}");
}

/// The validator guest's `validate` span, as the host adapter exported it:
/// its `scxml.outcome` and `error.type` attributes and its event names.
fn guest_validate_span(
    spans: &opentelemetry_sdk::trace::InMemorySpanExporter,
) -> (String, Option<String>, Vec<String>) {
    let finished = spans
        .get_finished_spans()
        .expect("in-memory spans are readable");
    let guest: Vec<_> = finished
        .iter()
        .filter(|span| {
            span.name == "validate" && span.instrumentation_scope.name() == "scxml-component-guest"
        })
        .collect();
    assert_eq!(guest.len(), 1, "one guest validate span per request");
    let attribute = |key: &str| {
        guest[0]
            .attributes
            .iter()
            .find(|attribute| attribute.key.as_str() == key)
            .map(|attribute| attribute.value.as_str().into_owned())
    };
    (
        attribute("scxml.outcome").expect("the guest span records its outcome"),
        attribute("error.type"),
        guest[0]
            .events
            .iter()
            .map(|event| event.name.to_string())
            .collect(),
    )
}

#[test]
fn guest_validate_span_records_its_events_and_error_category() {
    let exported = |request: &ValidationRequest| {
        let spans = opentelemetry_sdk::trace::InMemorySpanExporter::default();
        let tracer = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_simple_exporter(spans.clone())
            .build();
        let providers = TelemetryProviders::from_providers(
            tracer,
            opentelemetry_sdk::metrics::SdkMeterProvider::builder().build(),
        );
        let validator =
            ComponentValidator::new(&artifacts().validator, None, &config(), &providers)
                .expect("the validator component compiles and links");
        let result = validator.validate(request);
        drop(validator);
        providers.force_flush().expect("guest spans flush");
        (result, guest_validate_span(&spans))
    };
    let mut request = ValidationRequest {
        source_name: "valid.scx".to_owned(),
        source: r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><final id="f"/></scxml>"#.to_owned(),
        policy: SchemaPolicy::Strict,
        schemas: Vec::new(),
        packages: Vec::new(),
        event_schemas: Vec::new(),
        invoked_documents: Vec::new(),
        scripts: Vec::new(),
        // A sampled remote parent, so the guest span is recorded whatever
        // the process-global provider is.
        trace_context: Some(scxml_host_wasmtime::validator_types::TraceContext {
            traceparent: Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_owned()),
            tracestate: None,
        }),
    };
    let (result, span) = exported(&request);
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        span,
        ("ok".to_owned(), None, vec!["artifact-produced".to_owned()])
    );

    // An empty source name violates the request bound: the guest records
    // `input-rejected` and ends the span with its `invalid-input` category.
    request.source_name = String::new();
    let (result, span) = exported(&request);
    assert!(result.is_err(), "an empty source name is rejected");
    assert_eq!(
        span,
        (
            "failed".to_owned(),
            Some("invalid-input".to_owned()),
            vec!["input-rejected".to_owned()]
        )
    );
}

/// Without a carrier the guest span's parent is the ambient context, which under the no-op process-global provider (none is
/// installed in this test binary) holds the facade's invalid span. The
/// host must start a root span under its own providers, never refuse the
/// guest span.
#[test]
fn guest_validate_span_without_a_carrier_is_a_root_span_under_a_noop_global_provider() {
    let spans = opentelemetry_sdk::trace::InMemorySpanExporter::default();
    let tracer = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_simple_exporter(spans.clone())
        .build();
    let providers = TelemetryProviders::from_providers(
        tracer,
        opentelemetry_sdk::metrics::SdkMeterProvider::builder().build(),
    );
    let validator = ComponentValidator::new(&artifacts().validator, None, &config(), &providers)
        .expect("the validator component compiles and links");
    let result = validator.validate(&ValidationRequest {
        source_name: "valid.scx".to_owned(),
        source: r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><final id="f"/></scxml>"#.to_owned(),
        policy: SchemaPolicy::Strict,
        schemas: Vec::new(),
        packages: Vec::new(),
        event_schemas: Vec::new(),
        invoked_documents: Vec::new(),
        scripts: Vec::new(),
        trace_context: None,
    });
    drop(validator);
    providers.force_flush().expect("guest spans flush");
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        guest_validate_span(&spans),
        ("ok".to_owned(), None, vec!["artifact-produced".to_owned()])
    );
    let finished = spans
        .get_finished_spans()
        .expect("in-memory spans are readable");
    let guest = finished
        .iter()
        .find(|span| span.name == "validate")
        .expect("the guest span is exported");
    assert!(guest.span_context.is_valid(), "a recorded root span");
    assert!(guest.span_context.is_sampled());
    assert_eq!(
        guest.parent_span_id,
        opentelemetry::trace::SpanId::INVALID,
        "the invalid ambient span is not its parent"
    );
}
