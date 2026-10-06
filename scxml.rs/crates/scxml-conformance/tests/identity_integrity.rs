#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use scxml_conformance::manifest::{MAX_COMMAND_BUDGET, MAX_MANIFEST_ITEMS};
#[cfg(feature = "wasmtime")]
mod support;

use scxml_conformance::{
    ConformanceLevel, IrpAssertion, IrpManifest, IrpTest, SuiteManifest, flatten_irp_manifest,
    load_suite,
};
#[cfg(feature = "wasmtime")]
use scxml_conformance::{RunOptions, RunReport};
#[cfg(feature = "wasmtime")]
use scxml_telemetry::{Operation, Recorder, SignalEvent, install_recorder};

fn fixture_manifest() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance/corpus.json")
}

fn temporary(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{name}-{}", std::process::id()))
}

fn copy_tree(source: &Path, destination: &Path) {
    let _ = fs::remove_dir_all(destination);
    fs::create_dir_all(destination).expect("temporary corpus root must be created");
    let mut pending = vec![(source.to_path_buf(), destination.to_path_buf())];
    while let Some((from, to)) = pending.pop() {
        for entry in fs::read_dir(&from).expect("fixture directory must be readable") {
            let entry = entry.expect("fixture entry must be readable");
            let target = to.join(entry.file_name());
            if entry
                .file_type()
                .expect("fixture type must be readable")
                .is_dir()
            {
                fs::create_dir_all(&target).expect("fixture directory must be copied");
                pending.push((entry.path(), target));
            } else {
                fs::copy(entry.path(), target).expect("fixture file must be copied");
            }
        }
    }
}

fn rewrite_manifest(directory: &Path, change: impl FnOnce(&mut SuiteManifest)) {
    let path = directory.join("corpus.json");
    let mut manifest: SuiteManifest =
        serde_json::from_slice(&fs::read(&path).expect("manifest must be readable"))
            .expect("manifest must decode");
    change(&mut manifest);
    let mut bytes = serde_json::to_vec_pretty(&manifest).expect("manifest must encode");
    bytes.push(b'\n');
    fs::write(path, bytes).expect("manifest must be writable");
}

#[test]
fn suite_identity_rejects_metadata_oracle_and_referenced_byte_tampering() {
    for (name, change) in [("provenance", 0_u8), ("oracle", 1_u8)] {
        let directory = temporary(&format!("scxml-identity-{name}"));
        copy_tree(
            fixture_manifest()
                .parent()
                .expect("fixture root must exist"),
            &directory,
        );
        rewrite_manifest(&directory, |manifest| {
            if change == 0 {
                manifest.suite.provenance = Some("tampered provenance".into());
            } else {
                manifest.cases[0].expected = scxml_conformance::ExpectedOutcome::TerminalStatus {
                    status: scxml_conformance::TerminalStatus::Running,
                };
            }
        });
        assert_eq!(
            load_suite(&directory)
                .expect_err("identity tampering must fail")
                .code,
            "suite-identity-tampered"
        );
        fs::remove_dir_all(directory).expect("temporary corpus must be removable");
    }

    let directory = temporary("scxml-identity-resource");
    copy_tree(
        fixture_manifest()
            .parent()
            .expect("fixture root must exist"),
        &directory,
    );
    fs::write(directory.join("resources/note.txt"), "tampered")
        .expect("resource fixture must be writable");
    assert_eq!(
        load_suite(&directory)
            .expect_err("referenced byte tampering must fail")
            .code,
        "artifact-tampered"
    );
    fs::remove_dir_all(directory).expect("temporary corpus must be removable");

    let directory = temporary("scxml-identity-source-metadata");
    copy_tree(
        fixture_manifest()
            .parent()
            .expect("fixture root must exist"),
        &directory,
    );
    rewrite_manifest(&directory, |manifest| {
        manifest.cases[0].source_digest = Some("0".repeat(64));
    });
    assert_eq!(
        load_suite(&directory)
            .expect_err("source-set metadata tampering must fail")
            .code,
        "artifact-tampered"
    );
    fs::remove_dir_all(directory).expect("temporary corpus must be removable");

    let directory = temporary("scxml-identity-source-bytes");
    copy_tree(
        fixture_manifest()
            .parent()
            .expect("fixture root must exist"),
        &directory,
    );
    let manifest: SuiteManifest = serde_json::from_slice(
        &fs::read(directory.join("corpus.json")).expect("manifest must be readable"),
    )
    .expect("manifest must decode");
    let source = manifest.cases[0]
        .source_artifact
        .as_ref()
        .expect("source artifact fixture must exist");
    fs::write(directory.join(source), "tampered")
        .expect("source artifact fixture must be writable");
    assert_eq!(
        load_suite(&directory)
            .expect_err("source artifact byte tampering must fail")
            .code,
        "artifact-tampered"
    );
    fs::remove_dir_all(directory).expect("temporary corpus must be removable");
}

#[cfg(feature = "wasmtime")]
#[test]
fn report_identity_rejects_filter_and_provenance_tampering() {
    let report = support::run_corpus(&fixture_manifest(), RunOptions::default());
    let directory = temporary("scxml-report-identity");
    fs::create_dir_all(&directory).expect("report directory must be created");
    let path = directory.join("report.json");
    for field in ["filter", "provenance"] {
        let mut value = serde_json::to_value(&report).expect("report must become JSON");
        if field == "filter" {
            value["filters"]["ids"] = serde_json::json!(["001"]);
        } else {
            value["git_commit"] = serde_json::json!("tampered-revision");
        }
        let mut bytes = serde_json::to_vec_pretty(&value).expect("report JSON must encode");
        bytes.push(b'\n');
        fs::write(&path, bytes).expect("tampered report must write");
        assert_eq!(
            RunReport::read_json(&path)
                .expect_err("report identity tampering must fail")
                .code,
            "report-identity-tampered"
        );
    }
    fs::remove_dir_all(directory).expect("report directory must be removable");
}

#[test]
fn manifest_command_budgets_are_finitely_bounded() {
    for case_override in [false, true] {
        let directory = temporary(if case_override {
            "scxml-case-budget"
        } else {
            "scxml-default-budget"
        });
        copy_tree(
            fixture_manifest()
                .parent()
                .expect("fixture root must exist"),
            &directory,
        );
        rewrite_manifest(&directory, |manifest| {
            if case_override {
                manifest.cases[0].max_commands = Some(MAX_COMMAND_BUDGET + 1);
            } else {
                manifest.default_command_budget = MAX_COMMAND_BUDGET + 1;
            }
        });
        assert_eq!(
            load_suite(&directory)
                .expect_err("over-bound command budget must fail")
                .code,
            if case_override {
                "case-command-budget"
            } else {
                "manifest-command-budget"
            }
        );
        fs::remove_dir_all(directory).expect("temporary corpus must be removable");
    }
}

#[test]
fn flattening_bounds_cases_and_allocates_collision_ids_uniquely() {
    let manifest = IrpManifest {
        base_uri: "https://example.invalid/irp".into(),
        assertions: vec![IrpAssertion {
            id: "collision".into(),
            spec_num: "1".into(),
            spec_id: "#collision".into(),
            description: String::new(),
            tests: vec![IrpTest {
                id: "same".into(),
                conformance: ConformanceLevel::Mandatory,
                manual: false,
                starts: vec!["same/testsame.txml".into(); MAX_MANIFEST_ITEMS],
                dependencies: Vec::new(),
                rejects: Vec::new(),
            }],
        }],
    };
    let cases = flatten_irp_manifest(&manifest).expect("bounded collisions must flatten");
    assert_eq!(cases.len(), MAX_MANIFEST_ITEMS);
    assert_eq!(
        cases
            .iter()
            .map(|case| case.id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        MAX_MANIFEST_ITEMS
    );
    let mut over_bound = manifest;
    over_bound.assertions[0].tests[0]
        .starts
        .push("same/testsame.txml".into());
    assert_eq!(
        flatten_irp_manifest(&over_bound)
            .expect_err("over-bound flattened cases must fail")
            .code,
        "irp-case-count"
    );

    let adversarial = IrpManifest {
        base_uri: "https://example.invalid/irp".into(),
        assertions: vec![IrpAssertion {
            id: "collision".into(),
            spec_num: "1".into(),
            spec_id: "#collision".into(),
            description: String::new(),
            tests: vec![
                IrpTest {
                    id: "same-2".into(),
                    conformance: ConformanceLevel::Mandatory,
                    manual: false,
                    starts: vec!["natural/testsame-2.txml".into()],
                    dependencies: Vec::new(),
                    rejects: Vec::new(),
                },
                IrpTest {
                    id: "same".into(),
                    conformance: ConformanceLevel::Mandatory,
                    manual: false,
                    starts: vec![
                        "generated/testsame.txml".into(),
                        "generated/testsame.txml".into(),
                    ],
                    dependencies: Vec::new(),
                    rejects: Vec::new(),
                },
            ],
        }],
    };
    assert_eq!(
        flatten_irp_manifest(&adversarial)
            .expect_err("generated ids must not probe through natural-id collisions")
            .code,
        "irp-case-id"
    );
}

#[test]
fn checked_in_producer_artifacts_load_with_recomputed_identity() {
    for datamodel in ["ecmascript", "xpath"] {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/conformance/w3c-irp/prepared")
            .join(datamodel);
        let suite = load_suite(&root).expect("prepared producer output must load and verify");
        assert_eq!(suite.manifest.suite.datamodel, datamodel);
        assert!(!suite.cases.is_empty());
    }
}

#[test]
fn official_loader_rejects_missing_revision_at_the_producer_boundary() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/conformance/w3c-irp/prepared/ecmascript");
    let directory = temporary("scxml-official-missing-revision");
    copy_tree(&source, &directory);
    rewrite_manifest(&directory, |manifest| manifest.suite.revision = None);
    let error = load_suite(&directory)
        .expect_err("OfficialIrp manifests without a pinned revision must fail closed");
    assert_eq!(error.code, "official-corpus-identity");
    fs::remove_dir_all(directory).expect("revision fixture must be removable");
}

#[cfg(feature = "wasmtime")]
#[test]
fn accepted_identity_boundaries_emit_bounded_otel_events() {
    let recorder = Recorder::default();
    let _guard = install_recorder(recorder.clone());
    let suite = load_suite(&fixture_manifest()).expect("fixture suite must verify");
    let report = support::run(suite, RunOptions::default());
    let directory = temporary("scxml-identity-telemetry");
    fs::create_dir_all(&directory).expect("telemetry report directory must be created");
    let report_path = directory.join("report.json");
    report.write_json(&report_path).expect("report must write");
    RunReport::read_json(&report_path).expect("report must verify");

    let observations = recorder.observations();
    assert!(observations.iter().any(|observation| {
        observation.operation == Operation::ManifestLoadSuite
            && observation.events.contains(&SignalEvent::ArtifactAccepted)
    }));
    assert!(observations.iter().any(|observation| {
        observation.operation == Operation::ReportReadJson
            && observation.events.contains(&SignalEvent::ArtifactAccepted)
    }));
    fs::remove_dir_all(directory).expect("telemetry report directory must be removable");
}

/// A prepared artifact path that names a FIFO is rejected without being
/// opened: opening it would block until a writer appears.
#[cfg(unix)]
#[test]
fn a_fifo_artifact_is_rejected_without_being_opened() {
    let directory =
        std::env::temp_dir().join(format!("scxml-conformance-fifo-{}", std::process::id()));
    fs::create_dir_all(&directory).expect("the fixture directory is created");
    let fifo = directory.join("artifact.json");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo runs");
    assert!(status.success(), "the FIFO fixture is created");
    let (sender, receiver) = std::sync::mpsc::channel();
    let path = fifo.clone();
    std::thread::spawn(move || {
        let _ = sender.send(scxml_conformance::sha256_file(&path).map(|_| ()));
    });
    let result = receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("hashing does not block on a FIFO");
    assert!(result.is_err(), "a FIFO is not a prepared artifact");
    fs::remove_dir_all(&directory).expect("the fixture directory is removed");
}
