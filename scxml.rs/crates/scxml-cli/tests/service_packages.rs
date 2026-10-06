#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

//! `--service-package` through the CLI binary: `validate` warns on a literal
//! `<invoke type>` no package binds (`SCXML094`), `compile` still accepts it
//! (invoking it is a runtime action, SCXML 6.4), and binding the example
//! service package
//! (`components/scxml-service-example/package/echo.service.json`) lists what
//! binding it consents to.

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
    Command::new(env!("CARGO_BIN_EXE_scx"))
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
        "scx-service-packages-{}-{name}",
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

    let unbound = cli(&["compile", source, "--output", output_path]);
    let stderr = String::from_utf8_lossy(&unbound.stderr);
    assert!(unbound.status.success(), "{stderr}");
    assert!(!stderr.contains("COMP"), "{stderr}");
    std::fs::remove_file(output_path).expect("the IR is written");

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
fn validate_lists_no_wasi_resource_for_a_service_it_does_not_run() {
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
            && stderr.contains("= WASI resources: none")
            && !stderr.contains("WASI imports"),
        "validate reads no component, so it lists no imports: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_package_with_a_removed_field_fails_closed() {
    let dir = scratch("removed-field");
    let source = files_document(&dir);
    let manifest = dir.join("old.service.json");
    std::fs::write(
        &manifest,
        r#"{"format":"stateforward.scxml.service-package/v1","service-uri":"urn:example:svc","package-version":"1","service-artifact":"svc.wasm","schema-artifact":null,"schema-sha256":null,"wit-world":"stateforward:scxml/invoked-service","wit-version":"0.2.0","http-origins":[],"env-slots":[],"wasi-permissions":[]}"#,
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
    assert!(
        stderr.contains("unknown field `wasi-permissions`"),
        "{stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
