//! Static proof of every repository namespace's own implementation report
//! plan (`namespaces/<name>/irp`): its manifest cites a numbered statement
//! of the namespace's README for every assertion, every source it names is
//! checked in, both prepared corpora load with their integrity checks and
//! store exactly what they reference, and each canonical baseline is bound
//! to its corpus, its `SHA256SUMS`, and the published counts. The baselines
//! are reproduced through the shipped components in `irp_components.rs`.
#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use scxml_conformance::{
    CorpusKind, ExpectedOutcome, NAMESPACE_SUITES, load_suite, parse_irp_manifest, sha256_bytes,
};

const SCXML_NAMESPACE: &str = "http://www.w3.org/2005/07/scxml";
const DATA_MODELS: [&str; 2] = ["ecmascript", "xpath"];

/// Per suite: the test count, how many of those tests the compiler must
/// reject, and the SHA-256 of each canonical baseline (ECMAScript, `XPath`).
const SUITES: [(&str, usize, usize, [&str; 2]); 2] = [
    (
        "env",
        20,
        6,
        [
            "049b9b265bb4059378422cef86fa56c07d60ca769a296a89c7d9a5f7cca75a15",
            "4caf93252866b53b756f3f07aa65c2d6f1c610d32d2488f39265408b682adfb5",
        ],
    ),
    (
        "event",
        29,
        17,
        [
            "6805d35b72df43823ca595d10731dc0327bf9dee2d9857ea08f9557d92beeaee",
            "9aa21d3fbc9c67075ecf907bdb86bf930cb12584e70ced0f879b18f36ba2b27e",
        ],
    ),
];

fn suite_root(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../namespaces")
        .join(name)
        .join("irp")
        .canonicalize()
        .expect("the namespace suite root resolves")
}

#[test]
fn the_suite_table_lists_every_namespace_suite() {
    assert_eq!(
        SUITES.map(|(name, ..)| name),
        NAMESPACE_SUITES.map(|(name, _)| name)
    );
}

#[test]
fn every_assertion_cites_a_numbered_readme_statement_and_every_source_is_checked_in() {
    for (name, tests, ..) in SUITES {
        let root = suite_root(name);
        let readme =
            std::fs::read_to_string(root.join("../README.md")).expect("the namespace README reads");
        let bytes = std::fs::read(root.join("manifest.xml")).expect("the manifest reads");
        let manifest = parse_irp_manifest("manifest.xml", None, &bytes)
            .expect("the namespace manifest parses");
        let mut count = 0;
        for (index, assertion) in manifest.assertions.iter().enumerate() {
            let number = index + 1;
            assert_eq!(assertion.id, format!("{name}-{number}"), "{name}");
            assert_eq!(assertion.spec_num, number.to_string(), "{name}");
            assert_eq!(assertion.spec_id, format!("#{name}-{number}"), "{name}");
            assert!(
                readme.contains(&format!(
                    "<a id=\"{name}-{number}\"></a>**{name}-{number}**"
                )),
                "{name}/README.md states {}",
                assertion.id
            );
            assert!(!assertion.tests.is_empty(), "{} has a test", assertion.id);
            for test in &assertion.tests {
                count += 1;
                assert!(!test.manual, "{name} {} runs unattended", test.id);
                for uri in test.starts.iter().chain(&test.dependencies) {
                    assert!(
                        root.join("sources").join(uri).is_file(),
                        "{name}: {uri} is checked in"
                    );
                }
            }
        }
        assert_eq!(count, tests, "{name} test count");
    }
}

#[test]
fn prepared_corpora_load_and_store_exactly_what_they_reference() {
    for (name, tests, rejections, _) in SUITES {
        for datamodel in DATA_MODELS {
            let root = suite_root(name).join("prepared").join(datamodel);
            let suite = load_suite(&root).expect("the prepared namespace corpus loads");
            let identity = &suite.manifest.suite;
            assert_eq!(identity.kind, CorpusKind::NamespaceIrp);
            assert_eq!(identity.id, format!("namespaces/{name}/irp"));
            assert_eq!(identity.datamodel, datamodel);
            assert_eq!(identity.errata, None);
            assert_eq!(
                identity.manifest_digest,
                sha256_bytes(
                    &std::fs::read(suite_root(name).join("manifest.xml"))
                        .expect("the manifest reads")
                )
            );
            assert_eq!(suite.cases.len(), tests);
            assert_eq!(
                suite
                    .cases
                    .iter()
                    .filter(|case| matches!(
                        case.manifest.expected,
                        ExpectedOutcome::DocumentRejection { .. }
                    ))
                    .count(),
                rejections,
                "{name} {datamodel} rejection cases"
            );
            for case in &suite.cases {
                assert!(
                    case.manifest.preparation_failure.is_none(),
                    "{name} {datamodel} {} prepared",
                    case.manifest.id
                );
                let source = case.source.as_deref().expect("every case has a source");
                let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
                    scxml_syntax::SourceId::new(1),
                    case.manifest.start_uri.as_str(),
                    source,
                ))
                .expect("a prepared case is well-formed XML");
                assert_eq!(
                    document.root.name.namespace_uri.as_deref(),
                    Some(SCXML_NAMESPACE)
                );
            }
            let mut referenced = BTreeSet::new();
            for case in &suite.manifest.cases {
                referenced.extend(case.source.iter().cloned());
                referenced.extend(case.source_artifact.iter().cloned());
                referenced.extend(case.dependencies.iter().cloned());
                referenced.extend(case.dependency_source_artifacts.values().cloned());
            }
            let mut on_disk = BTreeSet::new();
            walk(&root, &root, &mut on_disk);
            on_disk.remove(Path::new("corpus.json"));
            assert_eq!(
                on_disk, referenced,
                "{name} {datamodel}: the artifact store holds exactly the referenced files"
            );
        }
    }
}

fn walk(root: &Path, directory: &Path, out: &mut BTreeSet<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("the prepared store reads") {
        let path = entry.expect("a prepared entry reads").path();
        if path.is_dir() {
            walk(root, &path, out);
        } else {
            out.insert(
                path.strip_prefix(root)
                    .expect("a walked path is below the root")
                    .to_path_buf(),
            );
        }
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one pass checks every baseline's digests, identity, extensions, and counts"
)]
fn baselines_are_digest_bound_passing_namespace_runs() {
    for (name, tests, rejections, digests) in SUITES {
        let root = suite_root(name);
        let mut sums = String::new();
        for (datamodel, digest) in DATA_MODELS.into_iter().zip(digests) {
            let file = format!("{datamodel}.scxml-report.json");
            let bytes = std::fs::read(root.join("baselines").join(&file))
                .expect("the namespace baseline reads");
            assert_eq!(sha256_bytes(&bytes), digest, "{name} {file}");
            writeln!(sums, "{digest}  {file}").expect("a String accepts writes");
            let decoded = scxml_reporting::decode_canonical(&bytes)
                .expect("the namespace baseline decodes and validates");
            assert_eq!(
                bytes,
                scxml_reporting::encode_canonical(&decoded).expect("the baseline re-encodes"),
                "{name} {file} is canonical bytes"
            );
            let payload = decoded
                .conformance_payload()
                .expect("the baseline is a conformance payload");
            let suite = load_suite(&root.join("prepared").join(datamodel))
                .expect("the prepared corpus loads");
            let identity = &payload.identity;
            assert_eq!(
                identity.suite.kind,
                scxml_reporting::ConformanceCorpusKind::NamespaceIrp
            );
            assert_eq!(
                identity.suite.corpus_digest,
                suite.manifest.suite.corpus_digest
            );
            assert_eq!(identity.runner.name, "scxml-conformance/wasm");
            assert_eq!(identity.engine.name, "scxml-machine");
            assert_eq!(
                identity.filters,
                scxml_reporting::ConformanceFilters::default()
            );
            assert_eq!(identity.run_revision, None);
            let packages = NAMESPACE_SUITES
                .iter()
                .find(|(suite, _)| *suite == name)
                .map(|(_, packages)| *packages)
                .expect("a listed suite");
            assert_eq!(
                identity.extensions.len(),
                packages.len(),
                "{name} binds exactly its packages"
            );
            for (extension, package) in identity.extensions.iter().zip(packages) {
                let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .join(package);
                let manifest_bytes = std::fs::read(&manifest).expect("the package manifest reads");
                assert_eq!(
                    extension.manifest_sha256,
                    sha256_bytes(&manifest_bytes),
                    "{name}: the identity binds the exact package manifest"
                );
                let parsed: serde_json::Value =
                    serde_json::from_slice(&manifest_bytes).expect("the manifest is JSON");
                assert_eq!(
                    Some(extension.namespace_uri.as_str()),
                    parsed["namespace-uri"].as_str(),
                    "{name}: the identity binds the package's namespace"
                );
                assert_eq!(
                    Some(extension.schema_sha256.as_str()),
                    parsed["schema-sha256"].as_str(),
                    "{name}: the identity binds the package schema"
                );
            }
            let counts = &payload.summary.counts;
            let total = u64::try_from(tests).expect("a small count");
            assert_eq!(payload.summary.total_selected, total);
            assert_eq!(
                (
                    counts.applicable_selected,
                    counts.passed,
                    counts.not_passing
                ),
                (total, total, 0),
                "{name} {datamodel}"
            );
            assert_eq!(payload.summary.mandatory.passed, total);
            assert_eq!(
                payload
                    .cases
                    .iter()
                    .filter(|case| case.oracle
                        == Some(scxml_reporting::ConformanceOracleKind::DocumentRejection))
                    .count(),
                rejections,
                "{name} {datamodel} rejection oracle cases"
            );
        }
        assert_eq!(
            std::fs::read_to_string(root.join("baselines/SHA256SUMS")).expect("SHA256SUMS reads"),
            sums,
            "{name} SHA256SUMS"
        );
    }
}
