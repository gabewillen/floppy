//! floppy's namespace implementation report plans
//! (`namespaces/<name>/irp`, decision, bot, conversation, migration and
//! device), in
//! the format and through the pipeline of scxml.rs's own namespace suites
//! (`scxml.rs/docs/conformance.md`, "Namespace suites").
//!
//! Checked here, for each suite:
//!
//! * every assertion of its `manifest.xml` is a numbered statement of the
//!   namespace's README.md ("Statements", `<a id="<name>-<n>">`), and every
//!   statement has an assertion with at least one test;
//! * every test has its source and is in the prepared ecmascript corpus;
//! * `scx conformance run` over the prepared corpus, with the namespace's
//!   own extension package and component bound, passes every case, and
//!   agrees with the checked-in baseline
//!   (`baselines/ecmascript.report.json`, the `--json` report, its digest
//!   in `SHA256SUMS`) on the suite identity, schema policy, IR version,
//!   filters and summary, and on Linux `x86_64`, the host the baselines are
//!   measured on, on every field of every case (fuel included); on any
//!   other host, where the components' bytes and so their fuel differ, on
//!   every case's stage, disposition, oracle, reason and configuration.
//!
//! `scripts/namespace-irp.sh` prepares the corpora and regenerates the
//! baselines.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::Value as Json;
use support::{components, output, scratch, sha256_file, workspace};

/// The namespaces that have a suite: every floppy namespace.
const SUITES: [&str; 5] = ["decision", "bot", "conversation", "migration", "device"];

/// Whether this host is the one the baselines are measured on.
const BASELINE_HOST: bool = cfg!(all(target_os = "linux", target_arch = "x86_64"));

fn suite(name: &str) -> PathBuf {
    workspace().join("namespaces").join(name).join("irp")
}

/// One release directory for every suite: the session components and each
/// namespace's component beside them (`scx conformance run` reads an
/// extension-artifact from `--components`). Returns it and the SHA-256 of
/// its `manifest.tsv`.
fn release() -> &'static (PathBuf, String) {
    static RELEASE: OnceLock<(PathBuf, String)> = OnceLock::new();
    RELEASE.get_or_init(|| {
        let components = components();
        let dir = scratch("irp-release");
        for entry in
            std::fs::read_dir(&components.session).expect("the session components are listable")
        {
            let path = entry.expect("the entry is readable").path();
            if path.is_file() {
                std::fs::copy(&path, dir.join(path.file_name().expect("a file name")))
                    .expect("the session component is copyable");
            }
        }
        for name in SUITES {
            let file = format!("floppy_{name}.virtualized.wasm");
            std::fs::copy(
                components.dir.join("namespaces").join(&file),
                dir.join(&file),
            )
            .expect("the namespace component is copyable");
        }
        let manifest = sha256_file(&dir.join("manifest.tsv"));
        (dir, manifest)
    })
}

/// The manifest's `(assertion id, [test id])` in document order.
fn assertions(name: &str) -> Vec<(String, Vec<String>)> {
    let text = std::fs::read_to_string(suite(name).join("manifest.xml"))
        .expect("the manifest is readable");
    let mut found: Vec<(String, Vec<String>)> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("<assert id=\"") {
            let id = rest.split('"').next().expect("an assertion id");
            found.push((id.to_owned(), Vec::new()));
        } else if let Some(rest) = line.strip_prefix("<test id=\"") {
            let id = rest.split('"').next().expect("a test id");
            found
                .last_mut()
                .expect("a test sits in an assertion")
                .1
                .push(id.to_owned());
        }
    }
    found
}

/// The statement ids of the namespace's README.md.
fn statements(name: &str) -> Vec<String> {
    let readme = workspace().join("namespaces").join(name).join("README.md");
    let text = std::fs::read_to_string(readme).expect("the README is readable");
    let prefix = format!("<a id=\"{name}-");
    text.match_indices(&prefix)
        .map(|(start, _)| {
            let rest = &text[start + "<a id=\"".len()..];
            rest.split('"').next().expect("a statement id").to_owned()
        })
        .collect()
}

#[test]
fn every_statement_is_an_assertion_with_tests() {
    for name in SUITES {
        let assertions = assertions(name);
        let asserted: Vec<&str> = assertions.iter().map(|(id, _)| id.as_str()).collect();
        let statements = statements(name);
        assert_eq!(
            asserted, statements,
            "{name}: the manifest's assertions are the README's statements, in order"
        );
        for (id, tests) in &assertions {
            assert!(!tests.is_empty(), "{name}: {id} has a test");
            for test in tests {
                let source = suite(name)
                    .join("sources")
                    .join(test)
                    .join(format!("test{test}.txml"));
                assert!(source.is_file(), "{name}: {} exists", source.display());
            }
        }
        let corpus: Json = serde_json::from_str(
            &std::fs::read_to_string(suite(name).join("prepared/ecmascript/corpus.json"))
                .expect("the prepared corpus is readable"),
        )
        .expect("the corpus is JSON");
        let prepared: BTreeSet<&str> = corpus["cases"]
            .as_array()
            .expect("the corpus lists cases")
            .iter()
            .map(|case| case["id"].as_str().expect("a case id"))
            .collect();
        let listed: BTreeSet<&str> = assertions
            .iter()
            .flat_map(|(_, tests)| tests.iter().map(String::as_str))
            .collect();
        assert_eq!(
            prepared, listed,
            "{name}: the prepared corpus is the manifest's tests"
        );
    }
}

/// The fields of a case record that do not depend on the components' bytes.
fn portable(case: &Json) -> Json {
    let mut kept = serde_json::Map::new();
    for field in [
        "id",
        "assertion_id",
        "stage",
        "disposition",
        "oracle",
        "reason_code",
        "configuration",
        "terminal_status",
    ] {
        kept.insert(field.into(), case.get(field).cloned().unwrap_or(Json::Null));
    }
    Json::Object(kept)
}

fn run_suite(name: &str) {
    let (release, manifest) = release();
    let uri = format!("https://github.com/stateforward/floppy/namespaces/{name}");
    let component = release.join(format!("floppy_{name}.virtualized.wasm"));
    let baseline = suite(name).join("baselines/ecmascript.report.json");
    let sums = std::fs::read_to_string(suite(name).join("baselines/SHA256SUMS"))
        .expect("SHA256SUMS is readable");
    assert_eq!(
        sums,
        format!("{}  ecmascript.report.json\n", sha256_file(&baseline)),
        "{name}: SHA256SUMS names the baseline's digest"
    );
    let report = scratch("irp-reports").join(format!("{name}.json"));
    // Workspace-relative paths (scx runs from the workspace root), as the
    // baseline records them.
    let run = output(&[
        "conformance".into(),
        "run".into(),
        "--corpus".into(),
        format!("namespaces/{name}/irp/prepared/ecmascript"),
        "--components".into(),
        release.display().to_string(),
        "--components-manifest-sha256".into(),
        manifest.clone(),
        "--extension-package".into(),
        format!("namespaces/{name}/package/{name}.extension.json"),
        "--extension-component-sha256".into(),
        format!("{uri}={}", sha256_file(&component)),
        "--json".into(),
        report.display().to_string(),
    ]);
    assert!(
        run.status.success(),
        "{name}: every case passes\n{}\n{}",
        run.stdout,
        run.stderr
    );
    let read = |path: &Path| -> Json {
        serde_json::from_str(&std::fs::read_to_string(path).expect("the report is readable"))
            .expect("the report is JSON")
    };
    let (fresh, expected) = (read(&report), read(&baseline));
    for field in ["suite", "schema_policy", "ir_version", "filters", "summary"] {
        assert_eq!(
            fresh[field], expected[field],
            "{name}: the report's {field} is the baseline's"
        );
    }
    let cases = |report: &Json| -> Vec<Json> {
        let cases = report["cases"].as_array().expect("the report lists cases");
        if BASELINE_HOST {
            cases.clone()
        } else {
            cases.iter().map(portable).collect()
        }
    };
    assert_eq!(
        cases(&fresh),
        cases(&expected),
        "{name}: every case agrees with the baseline"
    );
}

#[test]
fn decision_suite_passes() {
    run_suite("decision");
}

#[test]
fn bot_suite_passes() {
    run_suite("bot");
}

#[test]
fn conversation_suite_passes() {
    run_suite("conversation");
}

#[test]
fn migration_suite_passes() {
    run_suite("migration");
}

#[test]
fn device_suite_passes() {
    run_suite("device");
}

/// A namespace with no suite would leave its elements untested here.
#[test]
fn every_namespace_has_a_suite() {
    let mut namespaces: Vec<String> = std::fs::read_dir(workspace().join("namespaces"))
        .expect("namespaces/ is listable")
        .map(|entry| {
            entry
                .expect("the entry is readable")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    namespaces.sort();
    let mut suites: Vec<String> = SUITES.iter().map(|name| (*name).to_owned()).collect();
    suites.sort();
    assert_eq!(namespaces, suites);
}
