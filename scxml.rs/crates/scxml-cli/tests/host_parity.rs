#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

//! The cross-host parity corpus (`tests/fixtures/host-parity/cases.json`):
//! each case runs through `scx run` (the Wasmtime session host) and its
//! outcome must equal the checked-in golden `<name>.golden.json`, which the
//! browser host's `test/host-parity.test.mjs` holds the browser host to as
//! well: the root session's `<log>` records in order, its final
//! configuration and status, and its `<donedata>` value. A case binds the
//! extension packages it names (the example namespace through
//! `--extension-package`, the `scx` namespace embedded in `scx`), and the
//! event schema files it names are read relative to its document, as `scx`
//! always reads them. `SCXML_BLESS=1` rewrites the goldens.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

const EXAMPLE_URI: &str = "https://stateforward.org/scxml/example";

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root resolves")
}

/// The release directory `scx` embeds (its build reads the same one).
fn release() -> PathBuf {
    std::env::var_os("SCXML_WASM_ARTIFACTS_DIR").map_or_else(
        || workspace().join("target/wasm-artifacts/release"),
        PathBuf::from,
    )
}

/// The flags binding one extension namespace a case names.
fn extension_flags(uri: &str) -> Vec<String> {
    match uri {
        EXAMPLE_URI => {
            let component = release().join("scxml_extension_example.virtualized.wasm");
            let bytes = fs::read(&component).expect("the example component is built");
            vec![
                "--extension-package".into(),
                workspace()
                    .join("components/scxml-extension-example/package/example.extension.json")
                    .display()
                    .to_string(),
                "--extension-component".into(),
                format!("{EXAMPLE_URI}={}", component.display()),
                "--extension-component-sha256".into(),
                format!("{EXAMPLE_URI}={:x}", Sha256::digest(&bytes)),
            ]
        }
        // `scx` embeds the `scx` namespace.
        "https://github.com/stateforward/scxml.rs/namespaces/scx" => Vec::new(),
        other => panic!("the corpus names an unknown extension namespace {other}"),
    }
}

/// One case's outcome through `scx run`.
fn run(case: &Value) -> Value {
    let document = workspace().join(case["document"].as_str().expect("a document"));
    let mut args = vec![
        "run".to_owned(),
        "--clock".into(),
        "logical".into(),
        "--on-eof".into(),
        "close".into(),
        document.display().to_string(),
    ];
    for uri in case["extensions"].as_array().expect("extensions") {
        args.extend(extension_flags(uri.as_str().expect("a URI")));
    }
    let mut input = String::new();
    for event in case["events"].as_array().expect("events") {
        let record = match event {
            Value::String(name) => json!({ "name": name }),
            other => other.clone(),
        };
        input.push_str(&record.to_string());
        input.push('\n');
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_scx"))
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("scx must start");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input.as_bytes())
        .expect("input is writable");
    let output = child.wait_with_output().expect("scx must finish");
    let mut logs = Vec::new();
    let mut configuration = Value::Null;
    let mut status = Value::Null;
    let mut done = Value::Null;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let record: Value = serde_json::from_str(line).expect("one JSON record");
        match record["type"].as_str() {
            Some("log") if record.get("parent").is_none() => {
                logs.push(json!([record["label"], record["text"]]));
            }
            Some("call") => {
                configuration = record["configuration"].clone();
                status = record["status"].clone();
            }
            Some("done") => done = record.get("data").cloned().unwrap_or(Value::Null),
            Some("error") => panic!(
                "{}: the run failed: {line}\n{}",
                case["name"],
                String::from_utf8_lossy(&output.stderr)
            ),
            _ => {}
        }
    }
    json!({ "logs": logs, "configuration": configuration, "status": status, "done": done })
}

#[test]
fn the_parity_corpus_matches_its_goldens_under_wasmtime() {
    let root = workspace().join("tests/fixtures/host-parity");
    let cases: Value =
        serde_json::from_slice(&fs::read(root.join("cases.json")).expect("the corpus reads"))
            .expect("the corpus is JSON");
    assert_eq!(cases["version"], 1);
    let cases = cases["cases"].as_array().expect("cases");
    assert!(!cases.is_empty(), "the corpus has cases");
    let bless = std::env::var_os("SCXML_BLESS").is_some();
    let mut mismatches = Vec::new();
    for case in cases {
        let name = case["name"].as_str().expect("a name");
        let outcome = run(case);
        let golden = root.join(format!("{name}.golden.json"));
        let rendered = format!(
            "{}\n",
            serde_json::to_string_pretty(&outcome).expect("the outcome serializes")
        );
        if bless {
            fs::write(&golden, rendered).expect("the golden writes");
            continue;
        }
        let expected = fs::read_to_string(&golden).unwrap_or_default();
        if expected != rendered {
            mismatches.push(format!(
                "{name}:\n--- golden\n{expected}--- actual\n{rendered}"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
