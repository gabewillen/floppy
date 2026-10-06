#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

//! `--extension-package` end to end (`wasmtime` feature): `validate` and
//! `compile` bind the example extension package
//! (`components/scxml-extension-example/package`), and `snapshot run`
//! executes its extension component through the shipped components.
//! Unbound namespaces stay rejected.
//!
//! The release components are built and verified once by
//! `scripts/verify-wasm-capabilities.sh` into this suite's own artifact
//! directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

use sha2::{Digest as _, Sha256};

const EXAMPLE_URI: &str = "https://stateforward.org/scxml/example";

/// The session components plus the example extension component.
const COMPONENTS: &[&str] = &[
    "scxml_clock",
    "scxml_data_model_ecmascript",
    "scxml_data_model_null",
    "scxml_data_model_xpath",
    "scxml_ioprocessor_scxml",
    "scxml_ioprocessor_basichttp",
    "scxml_extension_example",
    "scxml_compiler",
    "scxml_machine",
];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root resolves")
}

struct Release {
    dir: String,
    manifest_sha256: String,
    example_sha256: String,
}

fn release() -> &'static Release {
    static RELEASE: OnceLock<Release> = OnceLock::new();
    RELEASE.get_or_init(|| {
        let workspace = workspace();
        let target = workspace.join("target/cli-components");
        let dir = target.join("wasm-artifacts/extension-packages");
        let status = Command::new("sh")
            .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
            .args(COMPONENTS)
            .current_dir(&workspace)
            .env("CARGO_TARGET_DIR", &target)
            .env("SCXML_WASM_ARTIFACTS_DIR", &dir)
            .status()
            .expect("the capability script runs");
        assert!(status.success(), "the release capability proof must pass");
        let manifest = fs::read_to_string(dir.join("manifest.tsv")).expect("manifest");
        // Test-only trust anchors: the manifest the proof script just
        // verified, and the example component digest it recorded.
        let example_sha256 = manifest
            .lines()
            .find(|line| line.starts_with("artifact\tscxml_extension_example\t"))
            .and_then(|line| line.split('\t').nth(4))
            .expect("the manifest records the example component")
            .to_owned();
        Release {
            dir: dir.display().to_string(),
            manifest_sha256: format!("{:x}", Sha256::digest(manifest.as_bytes())),
            example_sha256,
        }
    })
}

fn package() -> String {
    workspace()
        .join("components/scxml-extension-example/package/example.extension.json")
        .display()
        .to_string()
}

fn fixture() -> String {
    workspace()
        .join("tests/fixtures/namespaces/example.scxml")
        .display()
        .to_string()
}

fn scratch(name: &str) -> PathBuf {
    let directory = workspace()
        .join("target/cli-extension-packages")
        .join(format!("{}-{name}", std::process::id()));
    fs::create_dir_all(&directory).expect("scratch directory must be creatable");
    directory
}

fn cli(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_scxml-cli"));
    command.args(args).env("NO_COLOR", "1");
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().expect("scxml-cli must run")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// `snapshot run` of `document` with the example package bound.
fn run_example(document: &str, extra: &[&str], env: &[(&str, &str)]) -> Output {
    let release = release();
    let behavior = format!(
        "{EXAMPLE_URI}={}/scxml_extension_example.virtualized.wasm",
        release.dir
    );
    let digest = format!("{EXAMPLE_URI}={}", release.example_sha256);
    let package = package();
    let mut args = vec![
        "snapshot",
        "run",
        document,
        "--components",
        release.dir.as_str(),
        "--components-manifest-sha256",
        release.manifest_sha256.as_str(),
        "--extension-package",
        package.as_str(),
        "--extension-component",
        behavior.as_str(),
        "--extension-component-sha256",
        digest.as_str(),
    ];
    args.extend_from_slice(extra);
    cli(&args, env)
}

#[test]
fn validate_uses_the_package_schema_for_its_namespace() {
    let document = fixture();
    let package = package();
    let bound = cli(
        &["validate", &document, "--extension-package", &package],
        &[],
    );
    assert!(bound.status.success(), "{}", text(&bound.stderr));

    let unbound = cli(&["validate", &document], &[]);
    assert_eq!(unbound.status.code(), Some(1));
    assert!(text(&unbound.stderr).contains("error[XSD001]"));

    // The package's schema governs structure: `location` is required.
    let scratch = scratch("validate");
    let invalid = scratch.join("missing-location.scxml");
    fs::write(
        &invalid,
        fs::read_to_string(&document)
            .expect("fixture")
            .replace(r#" location="seen""#, ""),
    )
    .expect("scratch document");
    let invalid = invalid.display().to_string();
    let rejected = cli(
        &["validate", &invalid, "--extension-package", &package],
        &[],
    );
    assert_eq!(
        rejected.status.code(),
        Some(1),
        "{}",
        text(&rejected.stderr)
    );
    assert!(text(&rejected.stderr).contains("location"));
}

#[test]
fn compile_binds_the_package_and_rejects_an_unbound_namespace() {
    let document = fixture();
    let package = package();
    let scratch = scratch("compile");
    let output = scratch.join("example.scxml-ir.json");
    let output = output.display().to_string();

    let rejected = cli(&["compile", &document, "--output", &output], &[]);
    assert_eq!(rejected.status.code(), Some(2));
    let stderr = text(&rejected.stderr);
    assert!(
        stderr.contains(&format!(
            "error[COMP002]: extension namespace `{EXAMPLE_URI}` has no extension package"
        )),
        "{stderr}"
    );
    assert!(
        stderr.contains("--extension-package MANIFEST.json"),
        "{stderr}"
    );
    assert!(!Path::new(&output).exists(), "no artifact on rejection");

    let compiled = cli(
        &[
            "compile",
            &document,
            "--output",
            &output,
            "--extension-package",
            &package,
        ],
        &[],
    );
    assert!(compiled.status.success(), "{}", text(&compiled.stderr));
    let recipe: serde_json::Value = serde_json::from_slice(
        &fs::read(format!("{output}.recipe.json")).expect("the recipe is written"),
    )
    .expect("the recipe is JSON");
    assert_eq!(
        recipe["linked_extension_components"],
        serde_json::json!(["scxml_extension_example.virtualized.wasm"])
    );
}

#[test]
fn snapshot_run_executes_the_bound_extension_component() {
    let document = fixture();
    let run = run_example(&document, &[], &[]);
    assert!(run.status.success(), "{}", text(&run.stderr));
    let snapshot = text(&run.stdout);
    assert!(snapshot.contains(r#""done""#), "{snapshot}");
}

#[test]
fn snapshot_run_requires_an_authenticated_extension_component() {
    let document = fixture();
    let release = release();
    let package = package();
    let missing = cli(
        &[
            "snapshot",
            "run",
            &document,
            "--components",
            &release.dir,
            "--components-manifest-sha256",
            &release.manifest_sha256,
            "--extension-package",
            &package,
        ],
        &[],
    );
    assert_eq!(missing.status.code(), Some(2));
    assert!(text(&missing.stderr).contains("needs --extension-component-sha256"));

    let wrong = run_example(
        &document,
        &[
            "--extension-component-sha256",
            &format!("{EXAMPLE_URI}={}", "0".repeat(64)),
        ],
        &[],
    );
    assert_eq!(wrong.status.code(), Some(2), "{}", text(&wrong.stderr));
    assert!(text(&wrong.stderr).contains("given twice"));

    let unrelated = run_example(
        &document,
        &[
            "--extension-component-sha256",
            &format!("https://example.org/other={}", "0".repeat(64)),
        ],
        &[],
    );
    assert_eq!(unrelated.status.code(), Some(2));
    assert!(text(&unrelated.stderr).contains("is not the namespace of any --extension-package"));

    let release_dir = &release.dir;
    let tampered = cli(
        &[
            "snapshot",
            "run",
            &document,
            "--components",
            release_dir,
            "--components-manifest-sha256",
            &release.manifest_sha256,
            "--extension-package",
            &package,
            "--extension-component",
            &format!("{EXAMPLE_URI}={release_dir}/scxml_extension_example.virtualized.wasm"),
            "--extension-component-sha256",
            &format!("{EXAMPLE_URI}={}", "0".repeat(64)),
        ],
        &[],
    );
    assert_eq!(tampered.status.code(), Some(2));
    assert!(
        text(&tampered.stderr).contains("is rejected"),
        "{}",
        text(&tampered.stderr)
    );
}

#[test]
fn snapshot_run_rejects_an_unbound_namespace_before_executing() {
    let release = release();
    let document = fixture();
    let rejected = cli(
        &[
            "snapshot",
            "run",
            &document,
            "--components",
            &release.dir,
            "--components-manifest-sha256",
            &release.manifest_sha256,
        ],
        &[],
    );
    assert_eq!(rejected.status.code(), Some(2));
    assert!(text(&rejected.stderr).contains("error[COMP002]"));
    assert!(rejected.stdout.is_empty(), "no document is written");
}
