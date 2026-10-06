#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

//! Every applicable official IRP case of both checked-in prepared corpora
//! compiles through the path users run: `scx compile`, whose validator
//! links the ECMAScript and `XPath` expression-syntax checkers. The conformance
//! runner's compiler links no expression checker, so the IRP gate alone never
//! exercises these diagnostics on the official documents.
//!
//! The gate requires every such compile to exit successfully with zero error
//! diagnostics; warnings are allowed and listed on standard output. The one
//! exception is a document the W3C itself requires the processor to reject
//! ([`W3C_REJECTED`]): its compile must fail with exactly that rejection.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use scxml_conformance::load_suite;
use scxml_reporting::{Severity, decode_canonical};

/// Official cases whose W3C instruction requires the processor to reject
/// the document, with the only error code the compile may report: case 301
/// ("the processor should  reject this document because it can't download
/// the script", `301/test301.txml`), whose `<script src>` no script package
/// supplies (`COMP003`). `XPath` declares 301 not applicable.
const W3C_REJECTED: [(&str, &str, &str); 1] = [("ecmascript", "301", "COMP003")];

/// One applicable official case to compile.
struct OfficialCase {
    datamodel: &'static str,
    id: String,
    source: PathBuf,
}

/// What `scx compile` reported for one case.
struct Compiled {
    label: String,
    /// The error code the W3C requires, for a [`W3C_REJECTED`] case.
    rejected_with: Option<&'static str>,
    exit_ok: bool,
    stderr: String,
    /// Error diagnostics as (code, rendered line).
    errors: Vec<(String, String)>,
    warnings: Vec<String>,
}

fn applicable_cases() -> Vec<OfficialCase> {
    let prepared =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance/w3c-irp/prepared");
    let mut cases = Vec::new();
    for datamodel in ["ecmascript", "xpath"] {
        let suite = load_suite(&prepared.join(datamodel))
            .expect("the checked-in prepared corpus must load");
        for case in suite.cases {
            if case.manifest.not_applicable.is_some() {
                continue;
            }
            // The external-client case 513 has no SCXML document; every
            // other applicable case has its prepared executable source.
            let Some(source) = case.source_path else {
                continue;
            };
            if case.source.is_none() {
                continue;
            }
            cases.push(OfficialCase {
                datamodel,
                id: case.manifest.id,
                source,
            });
        }
    }
    cases
}

fn compile(case: &OfficialCase, output: &Path) -> Compiled {
    let label = format!("{} {}", case.datamodel, case.id);
    let stem = format!("{}-{}", case.datamodel, case.id);
    let canonical = output.join(format!("{stem}.report.json"));
    let result = Command::new(env!("CARGO_BIN_EXE_scx"))
        .arg("compile")
        .arg("--output")
        .arg(output.join(format!("{stem}.scxml-ir.json")))
        .arg("--canonical")
        .arg(&canonical)
        .arg("--")
        .arg(&case.source)
        .output()
        .expect("scx must spawn");
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    if let Ok(bytes) = std::fs::read(&canonical) {
        let artifact = decode_canonical(&bytes).expect("compile writes a canonical report");
        let payload = artifact
            .compiler_payload()
            .expect("compile's canonical report is a compiler artifact");
        for diagnostic in &payload.diagnostics {
            match diagnostic.severity {
                Severity::Error => errors.push((
                    diagnostic.code.clone(),
                    format!("{label}: {} {}", diagnostic.code, diagnostic.message),
                )),
                Severity::Warning => warnings.push(format!(
                    "{label}: {} {}",
                    diagnostic.code, diagnostic.message
                )),
                Severity::Note => {}
            }
        }
    }
    let rejected_with = W3C_REJECTED
        .iter()
        .find(|(datamodel, id, _)| *datamodel == case.datamodel && *id == case.id)
        .map(|(_, _, code)| *code);
    Compiled {
        label,
        rejected_with,
        exit_ok: result.status.success(),
        stderr: String::from_utf8_lossy(&result.stderr).into_owned(),
        errors,
        warnings,
    }
}

#[test]
fn every_applicable_official_case_compiles_through_the_validator_linked_cli() {
    let cases = applicable_cases();
    // ECMAScript: 202 cases less 436 (not applicable) and 513 (no
    // document); XPath: 202 less 24 not applicable and 513.
    assert_eq!(cases.len(), 200 + 177, "applicable official documents");
    let output =
        std::env::temp_dir().join(format!("scx-irp-validator-path-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&output);
    std::fs::create_dir_all(&output).expect("the output directory must be created");

    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    let chunk = cases.len().div_ceil(workers);
    let compiled: Vec<Compiled> = std::thread::scope(|scope| {
        let handles: Vec<_> = cases
            .chunks(chunk)
            .map(|chunk| {
                let output = &output;
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|case| compile(case, output))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("a compile worker must not panic"))
            .collect()
    });
    std::fs::remove_dir_all(&output).expect("the output directory must be removable");

    let warnings: Vec<&String> = compiled.iter().flat_map(|case| &case.warnings).collect();
    println!(
        "{} warning diagnostic(s) across {} official documents",
        warnings.len(),
        compiled.len()
    );
    for warning in &warnings {
        println!("warning: {warning}");
    }

    let mut failures = String::new();
    for case in &compiled {
        if let Some(code) = case.rejected_with {
            let rejected = !case.exit_ok
                && !case.errors.is_empty()
                && case.errors.iter().all(|(error_code, _)| error_code == code);
            if !rejected {
                writeln!(
                    failures,
                    "error: {}: the W3C requires rejecting this document with only {code}; exit ok {}, errors {:?}",
                    case.label, case.exit_ok, case.errors
                )
                .expect("writing to a String cannot fail");
            }
            continue;
        }
        for (_, error) in &case.errors {
            writeln!(failures, "error: {error}").expect("writing to a String cannot fail");
        }
        if !case.exit_ok && case.errors.is_empty() {
            writeln!(
                failures,
                "error: {}: compile failed without an error diagnostic: {}",
                case.label,
                case.stderr.trim()
            )
            .expect("writing to a String cannot fail");
        }
        if case.exit_ok && !case.errors.is_empty() {
            writeln!(
                failures,
                "error: {}: compile succeeded despite error diagnostics",
                case.label
            )
            .expect("writing to a String cannot fail");
        }
    }
    assert!(
        failures.is_empty(),
        "official IRP documents rejected by the validator-linked compile path:\n{failures}"
    );
}
