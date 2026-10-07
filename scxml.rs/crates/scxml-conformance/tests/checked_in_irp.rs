//! Static proof of the checked-in W3C IRP corpora and their canonical
//! baselines: exact upstream and prepared-corpus identity, the prepared
//! artifact store, the official oracle, and the digests binding each
//! baseline. The baselines are reproduced by running both corpora through
//! the shipped components in `irp_components.rs`.
#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use scxml_conformance::{
    ExpectedOutcome, LogOutcome, SuiteManifest, load_suite, parse_irp_manifest, sha256_bytes,
    sha256_file,
};
use scxml_syntax::{SourceId, SourceText, parse_xml};

/// The source id of the documents these tests parse.
const SOURCE: SourceId = SourceId::new(1);

const BASE_URI: &str = "https://www.w3.org/Voice/2013/scxml-irp";
const MANIFEST_DIGEST: &str = "5974da27031fbe0deacf1101028a9afaaf1757d50b4c4b89ce514ef07310a73e";
const SOURCE_SET_DIGEST: &str = "b2f199cf442a78a6a58a94fbab287a873cb3fea6ef9fb2568e915e502c9bc9c8";
const SCXML_NAMESPACE: &str = "http://www.w3.org/2005/07/scxml";
const ECMASCRIPT_CORPUS_DIGEST: &str =
    "9f005f20c9cbcd7188507833d8fd8a71158cc49cf70e9428fd3fc807e11396ca";
const ECMASCRIPT_REPORT_DIGEST: &str =
    "d4244615f8aabc6d821b0338ae8d3a5f90012b26686e757edc2ea7897f072e4c";
const ECMASCRIPT_ARTIFACT_DIGEST: &str =
    "10fe0cad8b6b97cbcefd7ba7169f6662c59bccdb8bd8c45339a6ff5d5a25360c";
const XPATH_CORPUS_DIGEST: &str =
    "a661ef4af8bc62fc064cea5e137a6b469ede4b7da80ececf932d66d37cc93bca";
const XPATH_REPORT_DIGEST: &str =
    "1711b1f91e8a4c3640e8418d413b8a421fb709a47b8c001affce55a89f57c0d9";
const XPATH_ARTIFACT_DIGEST: &str =
    "f44362b64b407c7453ce04c42ea50845931d13f71c3fa92d9da0d532e39363db";

#[test]
fn checked_in_official_corpus_is_complete_and_exact_namespace() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/conformance/w3c-irp")
        .canonicalize()
        .expect("checked-in W3C IRP root should resolve");
    let manifest_path = root.join("upstream/manifest.xml");
    assert_eq!(
        sha256_file(&manifest_path).expect("checked-in manifest should hash"),
        MANIFEST_DIGEST
    );
    let manifest_bytes = std::fs::read(&manifest_path).expect("checked-in manifest should read");
    let manifest = parse_irp_manifest("manifest.xml", Some(BASE_URI), &manifest_bytes)
        .expect("checked-in live W3C manifest shape should parse");

    let mut referenced = BTreeSet::new();
    for assertion in &manifest.assertions {
        for test in &assertion.tests {
            referenced.extend(test.starts.iter().cloned());
            referenced.extend(test.dependencies.iter().cloned());
        }
    }
    assert_eq!(referenced.len(), 211);
    for relative in &referenced {
        assert!(
            root.join("upstream/sources").join(relative).is_file(),
            "manifest-referenced source is missing: {relative}"
        );
    }

    for datamodel in ["ecmascript", "xpath"] {
        let suite = load_suite(&root.join("prepared").join(datamodel))
            .expect("checked-in prepared suite should pass integrity validation");
        assert_eq!(suite.manifest.cases.len(), 202);
        assert_eq!(suite.manifest.suite.source_digest, SOURCE_SET_DIGEST);
        assert_eq!(suite.manifest.suite.manifest_digest, MANIFEST_DIGEST);
        assert_eq!(
            suite
                .manifest
                .cases
                .iter()
                .filter(|case| case.source_artifact.is_some())
                .count(),
            202
        );
        assert_eq!(
            suite
                .manifest
                .cases
                .iter()
                .filter(|case| case.source.is_some())
                .count(),
            201
        );
        assert_eq!(
            suite
                .manifest
                .cases
                .iter()
                .map(|case| case.dependencies.len())
                .sum::<usize>(),
            9
        );

        for case in suite.cases.iter().filter(|case| case.source.is_some()) {
            let source = case
                .source
                .as_deref()
                .expect("prepared case with source path should have source text");
            let document = parse_xml(SourceText::new(
                SOURCE,
                case.manifest.start_uri.as_str(),
                source,
            ))
            .expect("prepared W3C case should be well-formed XML");
            assert_eq!(
                document.root.name.namespace_uri.as_deref(),
                Some(SCXML_NAMESPACE),
                "prepared case must retain the exact SCXML namespace: {}",
                case.manifest.id
            );
            assert_eq!(document.root.name.local_name, "scxml");
        }
    }
}

#[test]
fn checked_in_baselines_are_digest_bound_component_runs() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/conformance/w3c-irp")
        .canonicalize()
        .expect("checked-in W3C IRP root should resolve");
    for datamodel in ["ecmascript", "xpath"] {
        assert_baseline_identity(&root, datamodel);
    }
    assert_eq!(
        std::fs::read_to_string(root.join("baselines/SHA256SUMS"))
            .expect("checked-in baseline checksum manifest must be readable"),
        format!(
            "{ECMASCRIPT_ARTIFACT_DIGEST}  ecmascript.scxml-report.json\n\
             {XPATH_ARTIFACT_DIGEST}  xpath.scxml-report.json\n"
        )
    );
}

#[test]
fn prepared_artifact_store_contains_exactly_the_corpus_referenced_set() {
    // Fail-closed drift gate: every prepared file under each datamodel's
    // artifact store must be referenced by that store's corpus.json, and
    // every referenced file must exist. Superseded generations must be hard
    // cut over (removed in the same change), never left to ship alongside
    // the canonical corpus (AGENTS.md pre-1.0 hard-cutover rule).
    for datamodel in ["ecmascript", "xpath"] {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/conformance/w3c-irp/prepared")
            .join(datamodel);
        let manifest: SuiteManifest = serde_json::from_slice(
            &std::fs::read(root.join("corpus.json")).expect("official manifest must be readable"),
        )
        .expect("official manifest must decode");
        let mut referenced = std::collections::BTreeSet::new();
        collect_referenced_paths(&manifest, &mut referenced);
        assert!(!referenced.is_empty(), "corpus must reference artifacts");
        // Beside corpus.json the store keeps exactly one file: a verbatim
        // copy of the checked-in errata file whose digest the corpus binds.
        let errata = manifest
            .suite
            .errata
            .as_ref()
            .expect("both official corpora bind their errata");
        assert_eq!(errata.source, format!("{datamodel}.json"));
        assert_eq!(
            std::fs::read(root.join(&errata.source))
                .expect("the bound errata copy must be readable"),
            std::fs::read(root.join("../../errata").join(&errata.source))
                .expect("the checked-in errata file must be readable"),
            "{datamodel} corpus must carry the checked-in errata bytes"
        );
        let mut top_level_files = std::collections::BTreeSet::new();
        let mut on_disk = std::collections::BTreeSet::new();
        for entry in std::fs::read_dir(&root).expect("prepared store must be readable") {
            let entry = entry.expect("prepared store entry must be readable").path();
            if entry.is_file() {
                top_level_files.insert(
                    entry
                        .file_name()
                        .and_then(|name| name.to_str())
                        .expect("prepared file names are UTF-8")
                        .to_owned(),
                );
                continue;
            }
            for path in walk_files(&entry) {
                on_disk.insert(
                    path.strip_prefix(&root)
                        .expect("walked path must be under root")
                        .to_path_buf(),
                );
            }
        }
        assert_eq!(
            top_level_files,
            std::collections::BTreeSet::from(["corpus.json".to_owned(), errata.source.clone()]),
            "{datamodel} prepared store keeps only its manifest and bound errata beside the artifacts"
        );
        let unreferenced: Vec<_> = on_disk.difference(&referenced).collect();
        assert!(
            unreferenced.is_empty(),
            "{datamodel} artifact store ships files not referenced by corpus.json \
             (stale generation must be hard-cut): {unreferenced:?}"
        );
        let missing: Vec<_> = referenced.difference(&on_disk).collect();
        assert!(
            missing.is_empty(),
            "{datamodel} corpus.json references missing files: {missing:?}"
        );
    }
}

fn collect_referenced_paths(
    manifest: &SuiteManifest,
    out: &mut std::collections::BTreeSet<PathBuf>,
) {
    let mut visit = |value: &std::path::Path| {
        let text = value.to_string_lossy();
        if text.contains("/cases/")
            || text.contains("/sources/")
            || text.contains("/dependencies/")
            || text.starts_with("artifacts/")
        {
            out.insert(PathBuf::from(text.to_string()));
        }
    };
    for case in &manifest.cases {
        if let Some(value) = &case.source_artifact {
            visit(value);
        }
        if let Some(value) = &case.source {
            visit(value);
        }
        for value in case.dependency_source_artifacts.values() {
            visit(value);
        }
        for value in &case.dependencies {
            visit(value);
        }
        for value in &case.resources {
            visit(value);
        }
        // An erratum case executes its corrected output (`source`) and
        // retains its corrected input and the exact upstream output.
        if let Some(erratum) = &case.erratum {
            visit(&erratum.corrected_input);
            visit(&erratum.upstream_output);
        }
    }
}

fn walk_files(root: &std::path::Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current).expect("artifact store must be readable") {
            let entry = entry.expect("artifact entry must be readable").path();
            if entry.is_dir() {
                stack.push(entry);
            } else {
                files.push(entry);
            }
        }
    }
    files
}

#[test]
fn official_corpus_manifest_requires_the_exact_pass_oracle() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/conformance/w3c-irp/prepared/ecmascript");
    let mut manifest: SuiteManifest = serde_json::from_slice(
        &std::fs::read(root.join("corpus.json")).expect("official manifest must be readable"),
    )
    .expect("official manifest must decode");
    manifest.manifest_version = scxml_conformance::manifest::MANIFEST_VERSION;

    for (expected, label) in [
        (
            ExpectedOutcome::W3cLogOutcome {
                outcome: LogOutcome::Fail,
            },
            "fail expectation",
        ),
        (
            ExpectedOutcome::FinalState {
                state: "pass".into(),
            },
            "final-state expectation",
        ),
    ] {
        let mut invalid_manifest = manifest.clone();
        invalid_manifest.cases[0].expected = expected;
        let path = std::env::temp_dir().join(format!(
            "scxml-official-oracle-{label}-{}.json",
            std::process::id()
        ));
        let error = scxml_conformance::manifest::write_suite(&path, &invalid_manifest)
            .expect_err("official manifests must require exactly Outcome=pass");
        assert_eq!(error.code, "official-case-oracle", "{label}");
    }
}

/// The checked-in baseline decodes as a validated canonical artifact bound
/// to the exact corpus, report, and artifact digests, produced by the
/// component runner over the complete corpus with no gap.
fn assert_baseline_identity(root: &std::path::Path, datamodel: &str) {
    let (corpus_digest, report_digest, artifact_digest) = match datamodel {
        "ecmascript" => (
            ECMASCRIPT_CORPUS_DIGEST,
            ECMASCRIPT_REPORT_DIGEST,
            ECMASCRIPT_ARTIFACT_DIGEST,
        ),
        "xpath" => (
            XPATH_CORPUS_DIGEST,
            XPATH_REPORT_DIGEST,
            XPATH_ARTIFACT_DIGEST,
        ),
        _ => panic!("test covers only the checked-in ECMAScript and XPath baselines"),
    };
    let path = root
        .join("baselines")
        .join(format!("{datamodel}.scxml-report.json"));
    let bytes = std::fs::read(&path).expect("checked-in canonical baseline must be readable");
    assert_eq!(sha256_bytes(&bytes), artifact_digest);

    let decoded = scxml_reporting::decode_canonical(&bytes)
        .expect("checked-in canonical baseline must decode and validate");
    assert_eq!(
        bytes,
        scxml_reporting::encode_canonical(&decoded)
            .expect("checked-in canonical baseline must re-encode deterministically"),
        "the checked-in baseline is canonical bytes"
    );
    let payload = decoded
        .conformance_payload()
        .expect("checked-in baseline must contain a conformance payload");
    assert_eq!(payload.identity.suite.corpus_digest, corpus_digest);
    assert_eq!(payload.identity.suite.source_set_digest, SOURCE_SET_DIGEST);
    assert_eq!(payload.identity.suite.manifest_digest, MANIFEST_DIGEST);
    assert_eq!(payload.identity.runner.name, "scxml-conformance/wasm");
    assert_eq!(payload.identity.runner.version, "0.2.0");
    assert_eq!(payload.identity.engine.name, "scxml-machine");
    assert_eq!(
        payload.identity.data_model.name,
        format!("scxml-data-model-{datamodel}")
    );
    assert_eq!(
        payload.identity.ir_version,
        scxml_compiler_core::MACHINE_IR_VERSION
    );
    assert_eq!(
        payload.identity.filters,
        scxml_reporting::ConformanceFilters::default()
    );
    assert_eq!(payload.identity.run_revision, None);
    assert_eq!(payload.identity.report_digest, report_digest);
    let counts = &payload.summary.counts;
    // XPath: 302-304 and 301 (scripting only), 436 (null profile), and the
    // 19 ECMAScript-profile cases whose documents fix datamodel="ecmascript"
    // (tests/conformance/w3c-irp/errata/xpath.json).
    // ECMAScript: 436, whose document fixes datamodel="null"
    // (tests/conformance/w3c-irp/errata/ecmascript.json).
    let not_applicable = if datamodel == "xpath" { 24 } else { 1 };
    assert_eq!(payload.summary.total_selected, 202);
    assert_eq!(counts.applicable_selected, 202 - not_applicable);
    assert_eq!(counts.passed, 202 - not_applicable);
    assert_eq!(counts.not_applicable, not_applicable);
    assert_eq!(
        counts.passed_under_erratum,
        if datamodel == "xpath" { 5 } else { 0 }
    );
    assert_eq!(counts.failed, 0);
    assert_eq!(counts.unsupported, 0);
    assert_eq!(counts.invalid, 0);
    assert_eq!(counts.harness_error, 0);
    assert_eq!(counts.skipped, 0);
    assert_eq!(counts.not_passing, 0);
}
