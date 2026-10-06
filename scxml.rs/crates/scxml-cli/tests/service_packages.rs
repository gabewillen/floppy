#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

//! `--service-package` through the CLI binary: `validate` warns on a literal
//! `<invoke type>` no package binds (`SCXML094`), `compile` rejects it
//! (`COMP004`), and binding the example service package
//! (`components/scxml-service-example/package/echo.service.json`) accepts
//! it and lists what binding it consents to.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const ECHO: &str = "https://stateforward.org/scxml/services/echo";

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root resolves")
}

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_scxml-cli"))
        .args(args)
        .current_dir(workspace())
        .output()
        .expect("the CLI runs")
}

fn document(dir: &Path) -> PathBuf {
    let path = dir.join("service.scxml");
    std::fs::write(
        &path,
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s"><state id="s"><invoke type="{ECHO}" id="echo"/><transition event="done.invoke.echo" target="f"/></state><final id="f"/></scxml>"#
        ),
    )
    .expect("the fixture is written");
    path
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "scxml-cli-service-packages-{}-{name}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("the scratch directory is created");
    dir
}

#[test]
fn validate_and_compile_judge_literal_service_types_by_their_bound_packages() {
    let dir = scratch("judge");
    let source = document(&dir);
    let source = source.to_str().expect("utf8 path");
    let manifest = workspace().join("components/scxml-service-example/package/echo.service.json");
    let manifest = manifest.to_str().expect("utf8 path");
    let output_path = dir.join("out.scxml-ir.json");
    let output_path = output_path.to_str().expect("utf8 path");

    let unbound = cli(&["validate", source]);
    let stderr = String::from_utf8_lossy(&unbound.stderr);
    assert!(unbound.status.success(), "a warning only: {stderr}");
    assert!(stderr.contains("warning[SCXML094]"), "{stderr}");

    let bound = cli(&["validate", source, "--service-package", manifest]);
    let stderr = String::from_utf8_lossy(&bound.stderr);
    assert!(bound.status.success(), "{stderr}");
    assert!(!stderr.contains("SCXML094"), "{stderr}");
    assert!(
        stderr.contains(&format!("service `{ECHO}`")) && stderr.contains("= outbound HTTP: none"),
        "the consent is listed: {stderr}"
    );

    let rejected = cli(&["compile", source, "--output", output_path]);
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(!rejected.status.success(), "{stderr}");
    assert!(stderr.contains("error[COMP004]"), "{stderr}");
    assert!(!Path::new(output_path).exists(), "no artifact on rejection");

    let compiled = cli(&[
        "compile",
        source,
        "--output",
        output_path,
        "--service-package",
        manifest,
    ]);
    let stderr = String::from_utf8_lossy(&compiled.stderr);
    assert!(compiled.status.success(), "{stderr}");
    let ir = std::fs::read_to_string(output_path).expect("the IR is written");
    assert!(ir.contains("\"invoke_types\""), "{ir}");
    assert!(ir.contains(ECHO), "{ir}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_malformed_service_package_fails_closed() {
    let dir = scratch("malformed");
    let source = document(&dir);
    let manifest = dir.join("bad.service.json");
    std::fs::write(
        &manifest,
        r#"{"format":"stateforward.scxml.service-package/v1","extra":1}"#,
    )
    .expect("written");
    let output = cli(&[
        "validate",
        source.to_str().expect("utf8"),
        "--service-package",
        manifest.to_str().expect("utf8"),
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("unknown field `extra`"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

const FILES: &str = "https://stateforward.org/scxml/services/files";

fn files_document(dir: &Path) -> PathBuf {
    let path = dir.join("files.scxml");
    std::fs::write(
        &path,
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s"><state id="s"><invoke type="{FILES}" id="files"/><transition event="done.invoke.files" target="f"/></state><final id="f"/></scxml>"#
        ),
    )
    .expect("the fixture is written");
    path
}

#[test]
fn declared_wasi_permissions_are_listed_before_anything_runs() {
    let dir = scratch("wasi-listing");
    let source = files_document(&dir);
    let manifest = workspace().join("components/scxml-service-files/package/files.service.json");
    let output = cli(&[
        "validate",
        source.to_str().expect("utf8"),
        "--service-package",
        manifest.to_str().expect("utf8"),
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains(&format!("service `{FILES}`"))
            && stderr.contains("= WASI: wasi:filesystem (mount `workspace`, read-write)"),
        "the declared permission is listed: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `run` checks the grants and the replay log's limits before it loads any
/// component, so these need no release directory.
#[cfg(feature = "wasmtime")]
#[test]
fn run_refuses_ungranted_permissions_and_recording_a_wasi_package() {
    let dir = scratch("wasi-run");
    let source = files_document(&dir);
    let manifest = workspace().join("components/scxml-service-files/package/files.service.json");
    let digest = format!("{FILES}={}", "a".repeat(64));
    let grant = format!("{FILES}=wasi:filesystem=workspace={}", dir.display());
    let base = [
        "run",
        source.to_str().expect("utf8"),
        "--components",
        dir.to_str().expect("utf8"),
        "--components-manifest-sha256",
        &"b".repeat(64),
        "--service-package",
        manifest.to_str().expect("utf8"),
        "--service-component-sha256",
        &digest,
    ];

    let ungranted = cli(&base);
    let stderr = String::from_utf8_lossy(&ungranted.stderr);
    assert!(!ungranted.status.success(), "{stderr}");
    assert!(
        stderr.contains("which the run does not grant")
            && stderr.contains(&format!("--grant {FILES}=wasi:filesystem=workspace=DIR")),
        "{stderr}"
    );

    let undeclared = cli(&[&base[..], &["--grant", &format!("{FILES}=wasi:random")]].concat());
    let stderr = String::from_utf8_lossy(&undeclared.stderr);
    assert!(!undeclared.status.success(), "{stderr}");
    assert!(stderr.contains("does not declare"), "{stderr}");

    let record = dir.join("session.log");
    let recorded = cli(&[
        &base[..],
        &[
            "--grant",
            &grant,
            "--record",
            record.to_str().expect("utf8"),
        ],
    ]
    .concat());
    let stderr = String::from_utf8_lossy(&recorded.stderr);
    assert!(!recorded.status.success(), "{stderr}");
    assert!(stderr.contains("[record-wasi]"), "{stderr}");
    assert!(!record.exists(), "nothing was recorded");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn validate_cross_checks_transitions_against_a_declared_event_contract() {
    let dir = scratch("contract");
    let schema = br#"{"$defs":{"Reply":{"required":["text"],"type":"object"}}}"#;
    std::fs::write(dir.join("events.json"), schema).expect("the schema is written");
    let digest = {
        use sha2::Digest as _;
        format!("{:x}", sha2::Sha256::digest(schema))
    };
    let manifest = dir.join("svc.service.json");
    std::fs::write(
        &manifest,
        format!(
            r#"{{"format":"stateforward.scxml.service-package/v1","service-uri":"urn:example:svc","package-version":"1","service-artifact":"svc.wasm","schema-artifact":null,"schema-sha256":null,"wit-world":"stateforward:scxml/invoked-service","wit-version":"0.2.0","http-origins":[],"env-slots":[],"events":{{"emits":[{{"name":"svc.reply","schema-artifact":"events.json#/$defs/Reply","schema-sha256":"{digest}"}}],"accepts":[]}}}}"#
        ),
    )
    .expect("the manifest is written");
    let manifest = manifest.to_str().expect("utf8 path");
    let validate = |transition: &str| {
        let path = dir.join("contract.scxml");
        std::fs::write(
            &path,
            format!(
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event" version="1.0" initial="s"><state id="s"><invoke type="urn:example:svc"/>{transition}</state></scxml>"#
            ),
        )
        .expect("the document is written");
        cli(&[
            "validate",
            path.to_str().expect("utf8 path"),
            "--service-package",
            manifest,
        ])
    };
    let same = validate(
        r#"<transition event="svc.reply" event:schema='{"type":"object","required":["text"]}'/>"#,
    );
    let stderr = String::from_utf8_lossy(&same.stderr);
    assert!(same.status.success(), "{stderr}");
    assert!(!stderr.contains("SCXML09"), "{stderr}");
    assert!(
        stderr.contains("= events: emits svc.reply; accepts none"),
        "{stderr}"
    );

    let different = validate(r#"<transition event="svc.reply" event:schema='{"type":"string"}'/>"#);
    let stderr = String::from_utf8_lossy(&different.stderr);
    assert!(!different.status.success(), "{stderr}");
    assert!(stderr.contains("error[SCXML095]"), "{stderr}");

    let missing = validate(r#"<transition event="svc.reply"/>"#);
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(missing.status.success(), "a note only: {stderr}");
    assert!(stderr.contains("note[SCXML096]"), "{stderr}");
    std::fs::remove_dir_all(dir).expect("cleanup");
}
