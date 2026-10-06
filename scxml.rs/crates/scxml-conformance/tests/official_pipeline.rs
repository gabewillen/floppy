#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;
use std::time::Duration;

use scxml_compiler_core::MachineIr;
use scxml_conformance::executor::HttpRequest;
use scxml_conformance::manifest::{
    MAX_IRP_MANIFEST_BYTES, MAX_MANIFEST_ITEMS, MAX_MANIFEST_STRING_BYTES, MAX_NESTED_ITEMS,
};
use scxml_conformance::{
    CaseExecutor, ConformanceLevel, CorpusKind, CorpusTransformer, ExecutionError, ExecutionResult,
    ExecutorIdentities, ExternalClientRun, Filters, IdRange, PrepareOptions, SaxonTransformer,
    TransformerIdentity, flatten_irp_manifest, parse_irp_manifest, prepare_corpus,
};
use scxml_telemetry::{Operation, Recorder, SignalEvent, install_recorder};

const OFFICIAL_SHAPE: &str = r##"
<assertions baseuri="https://www.w3.org/Voice/2013/scxml-irp">
  <assert id="403" specnum="3.13" specid="#SelectingTransitions"><![CDATA[ transition selection ]]>
    <test id="403" conformance="mandatory" manual="false">
      <start uri="403/test403a.txml"/>
      <start uri="403/test403b.txml"/>
      <dep uri="403/test403sub.txml"/>
    </test>
  </assert>
  <assert id="513" specnum="B.2" specid="#BasicHTTPEventProcessor">
    <test id="513" conformance="optional" manual="true">
      <start uri="513/test513.txt"/>
    </test>
  </assert>
</assertions>
"##;

const LIVE_MANIFEST_SHAPE: &str = r##"
<assertions>
  <assert id="355" specnum="3.2" specid="#scxml">
    <test id="355" conformance="mandatory" manual="false">
      <start uri="355/test355.txml"/>
    </test>
  </assert>
</assertions>
"##;

#[test]
fn official_shape_parses_flattens_and_filters() {
    let manifest = parse_irp_manifest("manifest.xml", None, OFFICIAL_SHAPE.as_bytes())
        .expect("official-shape manifest fixture must parse");
    assert_eq!(manifest.base_uri, "https://www.w3.org/Voice/2013/scxml-irp");
    let cases = flatten_irp_manifest(&manifest).expect("official cases must flatten");
    assert_eq!(
        cases
            .iter()
            .map(|case| case.id.as_str())
            .collect::<Vec<_>>(),
        ["403a", "403b", "513"]
    );
    assert_eq!(cases[0].dependencies, ["403/test403sub.txml"]);
    assert_eq!(cases[2].conformance, ConformanceLevel::Optional);
    assert!(cases[2].manual);

    let filters = Filters {
        id_ranges: vec![IdRange {
            start: 403,
            end: 403,
        }],
        spec_ids: vec!["SelectingTransitions".into()],
        conformance: vec![ConformanceLevel::Mandatory],
        datamodels: vec!["ecmascript".into()],
        ..Filters::default()
    };
    let case = synthetic_case(&cases[0]);
    assert!(filters.selects(&case));
}

#[test]
fn live_manifest_shape_requires_a_trusted_acquisition_base_uri() {
    let missing = parse_irp_manifest("manifest.xml", None, LIVE_MANIFEST_SHAPE.as_bytes())
        .expect_err("missing manifest and acquisition base URI must fail closed");
    assert_eq!(missing.code, "irp-base-uri");

    let manifest = parse_irp_manifest(
        "manifest.xml",
        Some("https://www.w3.org/Voice/2013/scxml-irp"),
        LIVE_MANIFEST_SHAPE.as_bytes(),
    )
    .expect("trusted acquisition base URI should support the live W3C manifest shape");
    assert_eq!(manifest.base_uri, "https://www.w3.org/Voice/2013/scxml-irp");
}

#[test]
fn official_manifest_rejects_over_bound_bytes_counts_and_strings() {
    let oversized_bytes = vec![b' '; MAX_IRP_MANIFEST_BYTES + 1];
    assert_eq!(
        parse_irp_manifest("manifest.xml", None, &oversized_bytes)
            .expect_err("over-bound manifest bytes must fail before XML parsing")
            .code,
        "irp-manifest-size"
    );

    let oversized_source_name = "s".repeat(MAX_MANIFEST_STRING_BYTES + 1);
    assert_eq!(
        parse_irp_manifest(&oversized_source_name, None, LIVE_MANIFEST_SHAPE.as_bytes())
            .expect_err("over-bound source names must fail before XML parsing")
            .code,
        "irp-manifest-string"
    );

    let mut assertions = String::from("<assertions baseuri=\"https://example.invalid\">");
    for index in 0..=MAX_MANIFEST_ITEMS {
        write!(
            assertions,
            "<assert id=\"{index}\" specnum=\"1\" specid=\"x\"/>"
        )
        .expect("writing to an in-memory manifest fixture must succeed");
    }
    assertions.push_str("</assertions>");
    assert_eq!(
        parse_irp_manifest("manifest.xml", None, assertions.as_bytes())
            .expect_err("over-bound assertion counts must fail closed")
            .code,
        "irp-manifest-count"
    );

    let starts = "<start uri=\"case.txml\"/>".repeat(MAX_NESTED_ITEMS + 1);
    let nested = format!(
        "<assertions baseuri=\"https://example.invalid\"><assert id=\"1\" specnum=\"1\" specid=\"x\"><test id=\"1\" conformance=\"mandatory\" manual=\"false\">{starts}</test></assert></assertions>"
    );
    assert_eq!(
        parse_irp_manifest("manifest.xml", None, nested.as_bytes())
            .expect_err("over-bound nested collections must fail closed")
            .code,
        "irp-manifest-count"
    );
}

#[test]
fn saxon_command_construction_is_explicit() {
    let transformer = SaxonTransformer::new(vec!["java".into(), "-jar".into(), "saxon.jar".into()])
        .expect("non-empty Saxon command fixture must be valid");
    assert_eq!(
        transformer
            .command_for(
                Path::new("source.txml"),
                Path::new("ecmascript.xsl"),
                Path::new("output.scxml")
            )
            .expect("online command construction must succeed"),
        [
            "java",
            "-jar",
            "saxon.jar",
            "-s:source.txml",
            "-xsl:ecmascript.xsl",
            "-o:output.scxml"
        ]
    );
}

#[cfg(target_os = "macos")]
#[test]
fn offline_saxon_command_owns_non_network_options() {
    let root = temporary("offline-sandbox-command");
    let jar = root.join("saxon.jar");
    let source = root.join("source.txml");
    let stylesheet = root.join("ecmascript.xsl");
    let output = root.join("output.scxml");
    fs::write(&jar, b"bounded test artifact").expect("test JAR bytes must be writable");
    fs::write(&source, "<fixture/>").expect("test source bytes must be writable");
    fs::write(&stylesheet, "<stylesheet/>").expect("test stylesheet bytes must be writable");
    let transformer = SaxonTransformer::offline(vec![
        "java".into(),
        "-jar".into(),
        jar.display().to_string(),
    ])
    .expect("macOS sandbox and local transformer bytes must satisfy offline construction");
    let command = transformer
        .command_for(&source, &stylesheet, &output)
        .expect("offline command must resolve exact local transformer bytes");
    assert_eq!(
        command.first().map(String::as_str),
        Some("/usr/bin/sandbox-exec")
    );
    assert_eq!(command.get(1).map(String::as_str), Some("-p"));
    assert!(command.get(2).is_some(), "generated policy must be present");
    assert!(command[2].contains("(deny default)"));
    assert!(!command[2].contains("(allow default)"));
    assert!(command[2].contains(source.to_string_lossy().as_ref()));
    assert!(command[2].contains(stylesheet.to_string_lossy().as_ref()));
    assert_eq!(command.get(3).map(String::as_str), Some("--"));
    assert!(Path::new(&command[4]).is_absolute());
    assert_ne!(
        command.first(),
        command.get(4),
        "raw transformer must not be spawned"
    );
    assert!(command.iter().any(|argument| argument == "-dtd:off"));
    assert_eq!(
        transformer
            .offline_capability()
            .map(|capability| capability.mechanism()),
        Some("macos-sandbox-exec-default-deny-v2")
    );
    fs::write(&jar, b"changed transformer artifact").expect("test JAR mutation must be writable");
    let error = transformer
        .command_for(&source, &stylesheet, &output)
        .expect_err("changed transformer bytes must fail before process launch");
    assert_eq!(error.code, "transformer-offline-artifact-changed");
    fs::remove_dir_all(root).expect("offline sandbox command fixture must be removable");
}

#[cfg(not(target_os = "macos"))]
#[test]
fn offline_saxon_fails_closed_without_verified_host_isolation() {
    let error = SaxonTransformer::offline(vec!["saxon".into()])
        .expect_err("unsupported hosts must reject offline transformation");
    assert_eq!(error.code, "transformer-offline-isolation-unavailable");
}

#[test]
fn offline_saxon_rejects_adversarial_command_wrappers_and_options() {
    for command in [
        vec!["sh".into(), "-c".into(), "network-attempt".into()],
        vec!["saxon".into(), "-r:NetworkResolver".into()],
        vec![
            "java".into(),
            "-jar".into(),
            "https://example.invalid/saxon.jar".into(),
        ],
        vec![
            "java".into(),
            "-jar".into(),
            "saxon.jar".into(),
            "-init:Network".into(),
        ],
    ] {
        let error = SaxonTransformer::offline(command)
            .expect_err("offline transformer wrappers and caller options must fail closed");
        assert_eq!(error.code, "transformer-offline-command");
    }
}

#[test]
fn saxon_timeout_must_be_non_zero() {
    let error = SaxonTransformer::with_timeout(vec!["saxon".into()], Duration::ZERO)
        .expect_err("a zero process timeout must fail closed");
    assert_eq!(error.code, "transformer-timeout-invalid");
}

#[cfg(unix)]
#[test]
fn saxon_identity_and_transform_time_out_and_reap_children() {
    let root = temporary("scxml-saxon-timeout");
    let identity_pid = root.join("identity.pid");
    let identity = SaxonTransformer::with_timeout(
        vec![
            "sh".into(),
            "-c".into(),
            format!("echo $$ > '{}'; exec sleep 30", identity_pid.display()),
        ],
        Duration::from_millis(500),
    )
    .expect("a bounded identity transformer should be valid");
    let error = identity
        .identity()
        .expect_err("a non-terminating identity command must time out");
    assert_eq!(error.code, "transformer-timeout");
    assert_process_was_reaped(&identity_pid);

    let transform_pid = root.join("transform.pid");
    let transform = SaxonTransformer::with_timeout(
        vec![
            "sh".into(),
            "-c".into(),
            format!("echo $$ > '{}'; exec sleep 30", transform_pid.display()),
        ],
        Duration::from_millis(500),
    )
    .expect("a bounded transform command should be valid");
    let transform_output = root.join("output.scxml");
    fs::write(&transform_output, b"").expect("descriptor-owned output fixture must exist");
    let error = transform
        .transform(
            Path::new("source.txml"),
            Path::new("template.xsl"),
            &transform_output,
        )
        .expect_err("a non-terminating transform command must time out");
    assert_eq!(error.code, "transformer-timeout");
    assert_process_was_reaped(&transform_pid);
    fs::remove_dir_all(root).expect("Saxon timeout fixture directory must be removable");
}

#[cfg(unix)]
#[test]
fn saxon_identity_output_is_bounded() {
    let transformer = SaxonTransformer::with_timeout(
        vec![
            "sh".into(),
            "-c".into(),
            "i=0; while [ $i -lt 10000 ]; do printf x; i=$((i + 1)); done".into(),
        ],
        Duration::from_secs(2),
    )
    .expect("a bounded identity transformer should be valid");
    let identity = transformer
        .identity()
        .expect("bounded identity capture should accept a successful command");
    assert_eq!(identity.version.len(), 4_096);
    assert!(identity.version.bytes().all(|byte| byte == b'x'));
}

#[test]
fn local_prepare_uses_fake_transformer_and_digest_keyed_artifacts() {
    let recorder = Recorder::default();
    let _guard = install_recorder(recorder.clone());
    let root = temporary("scxml-official-prepare");
    let source = root.join("source");
    let templates = root.join("templates");
    let cache = root.join("cache");
    let output = root.join("output");
    fs::create_dir_all(source.join("355")).expect("case 355 source directory must be created");
    fs::create_dir_all(&templates).expect("template fixture directory must be created");
    fs::write(
        source.join("manifest.xml"),
        r##"<assertions baseuri="https://example.invalid/irp"><assert id="355" specnum="3.2" specid="#scxml"><test id="355" conformance="mandatory" manual="false"><start uri="355/test355.txml"/></test></assert><assert id="513" specnum="B.2" specid="#BasicHTTPEventProcessor"><test id="513" conformance="optional" manual="true"><start uri="513/test513.txt"/></test></assert></assertions>"##,
    )
    .expect("offline manifest fixture must be writable");
    fs::write(source.join("355/test355.txml"), "<fixture/>")
        .expect("case 355 source fixture must be writable");
    fs::create_dir_all(source.join("513")).expect("case 513 source directory must be created");
    fs::write(source.join("513/test513.txt"), "manual fixture")
        .expect("case 513 source fixture must be writable");
    fs::write(templates.join("ecmascript.xsl"), "<stylesheet/>")
        .expect("ECMAScript template fixture must be writable");
    let options = PrepareOptions {
        errata: None,
        namespace: None,
        manifest: source.join("manifest.xml").display().to_string(),
        base_uri: None,
        templates,
        datamodel: "ecmascript".into(),
        cache,
        output: output.clone(),
        source_dir: Some(source),
        revision: Some("fixture-revision".into()),
        offline: false,
        force: false,
        max_commands: 32,
    };
    let transformer = FakeTransformer;
    let first =
        prepare_corpus(&options, &transformer).expect("valid offline corpus fixture must prepare");
    assert_eq!(first.selected_cases, 2);
    assert_eq!(first.acquired_cases, 2);
    assert_eq!(first.prepared_cases, 1);
    assert_eq!(first.preparation_gaps, 0);
    let corpus: scxml_conformance::SuiteManifest = serde_json::from_slice(
        &fs::read(&first.corpus).expect("prepared corpus manifest must be readable"),
    )
    .expect("prepared corpus manifest must contain valid JSON");
    assert_eq!(corpus.suite.kind, CorpusKind::OfficialIrp);
    assert_eq!(corpus.suite.revision.as_deref(), Some("fixture-revision"));
    assert_eq!(corpus.suite.source_digest.len(), 64);
    assert_eq!(
        corpus.cases[0]
            .output_digest
            .as_deref()
            .expect("prepared case must record an output digest")
            .len(),
        64
    );
    assert!(corpus.cases[0].source_artifact.is_some());
    assert!(corpus.cases[1].source.is_none());
    assert!(corpus.cases[1].source_artifact.is_some());
    let prepared = output.join(
        corpus.cases[0]
            .source
            .as_ref()
            .expect("prepared case must record its output path"),
    );
    assert!(prepared.is_file());
    let loaded = scxml_conformance::load_suite(&first.corpus)
        .expect("fresh producer output must pass loader identity verification");
    assert_eq!(loaded.manifest, corpus);
    let first_bytes = fs::read(&first.corpus).expect("first corpus bytes must be readable");
    let second = prepare_corpus(&options, &transformer)
        .expect("repeated offline corpus preparation must succeed");
    assert_eq!(second.reused_artifacts, 2);
    assert_eq!(
        fs::read(&second.corpus).expect("reused corpus bytes must be readable"),
        first_bytes
    );
    assert!(recorder.observations().iter().any(|observation| {
        observation.operation == Operation::PrepareCorpus
            && observation.events.contains(&SignalEvent::ArtifactReused)
    }));
    fs::remove_dir_all(root).expect("offline corpus fixture directory must be removable");
}

#[test]
fn transformer_argv_separates_same_name_and_version_artifacts_and_regenerates_deterministically() {
    let root = temporary("scxml-transformer-identity");
    let source = root.join("source");
    let templates = root.join("templates");
    fs::create_dir_all(source.join("355")).expect("source directory must be created");
    fs::create_dir_all(&templates).expect("template directory must be created");
    fs::write(
        source.join("manifest.xml"),
        r##"<assertions baseuri="https://example.invalid/irp"><assert id="355" specnum="3.2" specid="#scxml"><test id="355" conformance="mandatory" manual="false"><start uri="355/test355.txml"/></test></assert></assertions>"##,
    )
    .expect("manifest fixture must be writable");
    fs::write(source.join("355/test355.txml"), "<fixture/>")
        .expect("source fixture must be writable");
    fs::write(templates.join("ecmascript.xsl"), "<stylesheet/>")
        .expect("template fixture must be writable");
    let options = PrepareOptions {
        errata: None,
        namespace: None,
        manifest: source.join("manifest.xml").display().to_string(),
        base_uri: None,
        templates,
        datamodel: "ecmascript".into(),
        cache: root.join("cache"),
        output: root.join("output"),
        source_dir: Some(source),
        revision: Some("fixture-revision".into()),
        offline: false,
        force: false,
        max_commands: 32,
    };
    let first_transformer = IdentityTransformer::new("wrapper-a", "a");
    let second_transformer = IdentityTransformer::new("wrapper-b", "b");
    let first = prepare_corpus(&options, &first_transformer)
        .expect("first transformer must prepare the fixture");
    let first_manifest: scxml_conformance::SuiteManifest =
        serde_json::from_slice(&fs::read(&first.corpus).expect("first manifest must be readable"))
            .expect("first manifest must decode");
    let first_path = first_manifest.cases[0]
        .source
        .clone()
        .expect("first transform must produce an artifact");

    let second = prepare_corpus(&options, &second_transformer)
        .expect("second transformer must prepare independently");
    assert_eq!(second.reused_artifacts, 0);
    let second_bytes = fs::read(&second.corpus).expect("second manifest must be readable");
    let second_manifest: scxml_conformance::SuiteManifest =
        serde_json::from_slice(&second_bytes).expect("second manifest must decode");
    let second_path = second_manifest.cases[0]
        .source
        .clone()
        .expect("second transform must produce an artifact");
    assert_ne!(first_path, second_path);
    assert_eq!(
        fs::read_to_string(options.output.join(&second_path))
            .expect("second output must be readable"),
        second_transformer.output
    );

    let mut regenerated_options = options.clone();
    regenerated_options.output = root.join("regenerated");
    let regenerated = prepare_corpus(&regenerated_options, &second_transformer)
        .expect("same identity must regenerate deterministically");
    assert_eq!(
        fs::read(&regenerated.corpus).expect("regenerated manifest must be readable"),
        second_bytes
    );
    fs::remove_dir_all(root).expect("identity fixture directory must be removable");
}

#[test]
fn official_prepare_requires_revision_before_creating_output() {
    let root = temporary("scxml-official-revision");
    let output = root.join("output");
    let cache = root.join("cache");
    let error = prepare_corpus(
        &PrepareOptions {
            errata: None,
            namespace: None,
            manifest: root.join("missing.xml").display().to_string(),
            base_uri: None,
            templates: root.join("templates"),
            datamodel: "ecmascript".into(),
            cache: cache.clone(),
            output: output.clone(),
            source_dir: Some(root.join("source")),
            revision: None,
            offline: false,
            force: false,
            max_commands: 32,
        },
        &FakeTransformer,
    )
    .expect_err("missing official revision must fail before filesystem side effects");
    assert_eq!(error.code, "official-revision-required");
    assert!(!cache.exists());
    assert!(!output.exists());
    fs::remove_dir_all(root).expect("revision fixture directory must be removable");
}

#[test]
fn transform_failure_remains_a_discoverable_preparation_gap() {
    let root = temporary("scxml-official-failure");
    let source = root.join("source");
    let templates = root.join("templates");
    fs::create_dir_all(source.join("355")).expect("failure case source directory must be created");
    fs::create_dir_all(&templates).expect("failure template directory must be created");
    fs::write(
        source.join("manifest.xml"),
        r##"<assertions baseuri="https://example.invalid/irp"><assert id="355" specnum="3.2" specid="#scxml"><test id="355" conformance="mandatory" manual="false"><start uri="355/test355.txml"/></test></assert></assertions>"##,
    )
    .expect("failure manifest fixture must be writable");
    fs::write(source.join("355/test355.txml"), "<fixture/>")
        .expect("failure source fixture must be writable");
    fs::write(templates.join("ecmascript.xsl"), "<stylesheet/>")
        .expect("failure template fixture must be writable");
    let summary = prepare_corpus(
        &PrepareOptions {
            errata: None,
            namespace: None,
            manifest: source.join("manifest.xml").display().to_string(),
            base_uri: None,
            templates,
            datamodel: "ecmascript".into(),
            cache: root.join("cache"),
            output: root.join("output"),
            source_dir: Some(source),
            revision: Some("fixture-revision".into()),
            offline: false,
            force: false,
            max_commands: 32,
        },
        &FailingTransformer,
    )
    .expect("transform failure must remain representable in a prepared corpus");
    assert_eq!(summary.selected_cases, 1);
    assert_eq!(summary.acquired_cases, 1);
    assert_eq!(summary.preparation_gaps, 1);
    let report = scxml_conformance::run_corpus(
        &summary.corpus,
        &scxml_conformance::RunOptions::default(),
        &NeverExecutes,
    )
    .expect("corpus with a preparation gap must produce a run report");
    assert_eq!(report.cases.len(), 1);
    assert_eq!(
        report.cases[0].disposition,
        scxml_conformance::Disposition::HarnessError
    );
    assert_eq!(report.summary.mandatory.not_passing, 1);
    fs::remove_dir_all(root).expect("failure fixture directory must be removable");
}

/// An executor the runner must never call: a preparation gap is decided
/// before execution. Only its identities are read, for the report.
struct NeverExecutes;

impl CaseExecutor for NeverExecutes {
    fn identities(&self) -> ExecutorIdentities {
        let identity = |name: &str| scxml_reporting::ConformanceImplementationIdentity {
            name: name.into(),
            version: "0.1.0".into(),
            source_digest: "0".repeat(64),
        };
        ExecutorIdentities {
            engine: identity("never-executes"),
            runner: identity("never-executes"),
            data_models: BTreeMap::new(),
            unrouted_data_model: identity("never-executes"),
        }
    }

    fn execute(
        &self,
        _: &[u8],
        _: &MachineIr,
        _: &[String],
        _: u64,
        _: &BTreeMap<String, String>,
    ) -> Result<ExecutionResult, ExecutionError> {
        panic!("a preparation gap must never execute");
    }

    fn execute_external_client(
        &self,
        _: &[u8],
        _: &MachineIr,
        _: &dyn Fn(&str) -> HttpRequest,
        _: u64,
        _: &BTreeMap<String, String>,
    ) -> Result<ExternalClientRun, ExecutionError> {
        panic!("a preparation gap must never execute");
    }
}

#[cfg(unix)]
#[test]
fn local_prepare_rejects_symlinked_corpus_paths() {
    use std::os::unix::fs::symlink;

    let root = temporary("scxml-official-symlink");
    let source = root.join("source");
    let outside = root.join("outside");
    let templates = root.join("templates");
    fs::create_dir_all(&source).expect("symlink source directory must be created");
    fs::create_dir_all(&outside).expect("symlink outside directory must be created");
    fs::create_dir_all(&templates).expect("symlink template directory must be created");
    fs::write(
        source.join("manifest.xml"),
        r##"<assertions baseuri="https://example.invalid/irp"><assert id="355" specnum="3.2" specid="#scxml"><test id="355" conformance="mandatory" manual="false"><start uri="355/test355.txml"/></test></assert></assertions>"##,
    )
    .expect("symlink manifest fixture must be writable");
    fs::write(outside.join("test355.txml"), "<fixture/>")
        .expect("outside source fixture must be writable");
    symlink(&outside, source.join("355")).expect("corpus symlink fixture must be creatable");
    fs::write(templates.join("ecmascript.xsl"), "<stylesheet/>")
        .expect("symlink template fixture must be writable");

    let summary = prepare_corpus(
        &PrepareOptions {
            errata: None,
            namespace: None,
            manifest: source.join("manifest.xml").display().to_string(),
            base_uri: None,
            templates,
            datamodel: "ecmascript".into(),
            cache: root.join("cache"),
            output: root.join("output"),
            source_dir: Some(source),
            revision: Some("fixture-revision".into()),
            offline: false,
            force: false,
            max_commands: 32,
        },
        &FakeTransformer,
    )
    .expect("symlink rejection is represented as a preparation gap");
    assert_eq!(summary.acquired_cases, 0);
    assert_eq!(summary.preparation_gaps, 1);
    let corpus: scxml_conformance::SuiteManifest = serde_json::from_slice(
        &fs::read(summary.corpus).expect("symlink-rejection corpus must be readable"),
    )
    .expect("symlink-rejection corpus must contain valid JSON");
    assert_eq!(
        corpus.cases[0]
            .preparation_failure
            .as_ref()
            .map(|failure| failure.code.as_str()),
        Some("corpus-path-symlink")
    );
    fs::remove_dir_all(root).expect("symlink fixture directory must be removable");
}

struct FakeTransformer;

struct IdentityTransformer {
    command: String,
    output: String,
}

impl IdentityTransformer {
    fn new(command: &str, marker: &str) -> Self {
        Self {
            command: command.into(),
            output: format!(
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" initial="{marker}"><final id="{marker}"/></scxml>"#
            ),
        }
    }
}

impl CorpusTransformer for IdentityTransformer {
    fn identity(&self) -> Result<TransformerIdentity, scxml_conformance::PrepareError> {
        Ok(TransformerIdentity {
            name: "same-transformer".into(),
            version: "same-version".into(),
            command_argv: vec![self.command.clone(), "--fixed-option".into()],
            executable_digest: None,
        })
    }

    fn transform(
        &self,
        _source: &Path,
        _template: &Path,
        output: &Path,
    ) -> Result<(), scxml_conformance::PrepareError> {
        fs::write(output, &self.output).expect("identity transformer output must be writable");
        Ok(())
    }
}

impl CorpusTransformer for FakeTransformer {
    fn identity(&self) -> Result<TransformerIdentity, scxml_conformance::PrepareError> {
        Ok(TransformerIdentity {
            name: "fake-xslt".into(),
            version: "1".into(),
            command_argv: vec!["test-fixture".into()],
            executable_digest: None,
        })
    }

    fn transform(
        &self,
        _source: &Path,
        _template: &Path,
        output: &Path,
    ) -> Result<(), scxml_conformance::PrepareError> {
        fs::write(
            output,
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" initial="pass"><final id="pass"><onentry><log label="Outcome" expr="'pass'"/></onentry></final></scxml>"#,
        )
        .expect("fake transformer output fixture must be writable");
        Ok(())
    }
}

struct FailingTransformer;

impl CorpusTransformer for FailingTransformer {
    fn identity(&self) -> Result<TransformerIdentity, scxml_conformance::PrepareError> {
        Ok(TransformerIdentity {
            name: "failing-xslt".into(),
            version: "1".into(),
            command_argv: vec!["test-fixture".into()],
            executable_digest: None,
        })
    }

    fn transform(
        &self,
        _source: &Path,
        _template: &Path,
        _output: &Path,
    ) -> Result<(), scxml_conformance::PrepareError> {
        Err(test_prepare_error())
    }
}

fn test_prepare_error() -> scxml_conformance::PrepareError {
    let transformer = SaxonTransformer::new(vec!["definitely-missing-test-executable".into()])
        .expect("non-empty missing-executable command fixture must be valid");
    transformer
        .transform(Path::new("a"), Path::new("b"), Path::new("c"))
        .unwrap_err()
}

fn synthetic_case(case: &scxml_conformance::IrpCase) -> scxml_conformance::CaseManifest {
    scxml_conformance::CaseManifest {
        erratum: None,
        not_applicable: None,
        id: case.id.clone(),
        assertion_id: case.assertion_id.clone(),
        spec_num: case.spec_num.clone(),
        spec_id: case.spec_id.clone(),
        conformance: case.conformance,
        datamodel: "ecmascript".into(),
        start_uri: case.start_uri.clone(),
        source: None,
        source_digest: None,
        source_artifact: None,
        output_digest: None,
        dependencies: Vec::new(),
        dependency_digests: BTreeMap::default(),
        dependency_source_digests: BTreeMap::default(),
        dependency_source_artifacts: BTreeMap::default(),
        resources: Vec::new(),
        resource_digests: BTreeMap::default(),
        tags: Vec::new(),
        description: case.description.clone(),
        features: Vec::new(),
        manual: case.manual,
        events: Vec::new(),
        expected: scxml_conformance::ExpectedOutcome::W3cLogOutcome {
            outcome: scxml_conformance::LogOutcome::Pass,
        },
        timeout_ms: None,
        max_commands: None,
        preparation_failure: None,
    }
}

fn temporary(prefix: &str) -> PathBuf {
    let temporary_root = fs::canonicalize(std::env::temp_dir())
        .expect("system temporary directory must have a stable canonical path");
    let path = temporary_root.join(format!("{prefix}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("temporary test directory must be created");
    path
}

#[cfg(unix)]
fn assert_process_was_reaped(pid_file: &Path) {
    let pid = fs::read_to_string(pid_file)
        .expect("the child should record its process id before the timeout")
        .trim()
        .parse::<u32>()
        .expect("the recorded child process id should be numeric");
    let status = Command::new("sh")
        .args(["-c", &format!("! kill -0 {pid} 2>/dev/null")])
        .status()
        .expect("the process cleanup assertion should execute");
    assert!(
        status.success(),
        "timed-out child process {pid} is still live"
    );
}
