//! The session-snapshot non-perturbation proof:
//! `conformance run --snapshot-every-checkpoint` (docs/session-snapshot.md,
//! "Non-perturbation proof over the IRP").
//!
//! The flag runs the selected suite three times through the same compiler,
//! engine, and canonical runner:
//!
//! 1. a plain run that collects nothing, producing report `A`;
//! 2. a run whose executor also gathers one read-only session-snapshot
//!    document per reached checkpoint (nothing excluded, `preserve`, pinned
//!    telemetry IDs), producing report `B` and documents `B`;
//! 3. a second collecting run, producing report `C` and documents `C`.
//!
//! The proof then requires, failing closed on any violation:
//!
//! - `A`, `B`, and `C` encode to byte-identical canonical report JSON, so
//!   snapshot collection never perturbed the run the oracle judged;
//! - documents `B` and `C` match case by case and checkpoint by checkpoint
//!   with byte-identical document bytes, so documents are reproducible;
//! - every document passes [`scxml_snapshot::check`] with no issue, which
//!   includes the decoder's canonical re-encode check.
//!
//! It finally writes a per-model summary to
//! `target/snapshot-proof/<datamodel>.json`, relative to the current
//! directory, recording for every executed case its reached checkpoint
//! ordinals and each document's SHA-256. The caller judges the returned
//! plain report like any other run.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use scxml_conformance::{
    CaseExecutor, CaseSnapshotDocument, LoadedSuite, RunOptions, RunReport, run_suite,
    run_suite_with_snapshot_sink, sha256_bytes,
};
use scxml_telemetry::{ErrorCategory, Module, Operation, observe};

/// Upper bound on the summary file so a runaway run cannot write unbounded
/// output; the bound covers the full official corpus comfortably.
const MAX_SUMMARY_BYTES: usize = 32 * 1024 * 1024;

/// One collecting pass's full output: the report and per-case documents.
type CollectingRun = (RunReport, Vec<(String, CollectedCase)>);

/// Documents collected for one case, in checkpoint order.
struct CollectedCase {
    checkpoints: Vec<u32>,
    documents: Vec<Vec<u8>>,
}

/// The non-perturbation proof over `suite`; `run_options` configures every pass
/// identically. Returns the plain (non-collecting) report so the caller can
/// apply its usual baseline and output handling to it.
///
/// # Errors
///
/// A plain `String` describing the first violated proof obligation, or the
/// runner error of the pass that failed to execute.
pub fn run_snapshot_proof(
    suite: &LoadedSuite,
    run_options: &RunOptions,
    collecting: &dyn Fn() -> Result<Box<dyn CaseExecutor>, Box<dyn std::error::Error>>,
    plain: &dyn CaseExecutor,
) -> Result<RunReport, Box<dyn std::error::Error>> {
    let span = observe(Module::Cli, Operation::CliConformanceSnapshotProof);
    let result = (|| {
        let report_a = run_suite(suite, run_options, plain)?;
        // Each collecting pass gets a fresh executor so its pinned ID
        // generators start from the same state; otherwise the second pass's
        // carriers would differ from the first's.
        let (report_b, documents_b) = run_collecting(suite, run_options, collecting)?;
        let (report_c, documents_c) = run_collecting(suite, run_options, collecting)?;

        // Collection never perturbed the run the oracle judged: all three
        // reports encode to the identical canonical JSON bytes.
        let bytes_a = canonical_report_bytes(&report_a)?;
        let bytes_b = canonical_report_bytes(&report_b)?;
        let bytes_c = canonical_report_bytes(&report_c)?;
        if bytes_a != bytes_b || bytes_b != bytes_c {
            let mut detail = String::new();
            if bytes_a != bytes_b {
                detail.push_str(" collecting run differs from the plain run");
            }
            if bytes_b != bytes_c {
                detail.push_str(" the two collecting runs differ");
            }
            return Err(Box::<dyn std::error::Error>::from(format!(
                "snapshot-every-checkpoint: report bytes are not identical:{detail}"
            )));
        }

        // The two collecting runs produced identical documents per case and
        // checkpoint, and every document is individually sound.
        if documents_b.len() != documents_c.len() {
            return Err(Box::<dyn std::error::Error>::from(format!(
                "snapshot-every-checkpoint: executed-case count differs between collecting runs ({} vs {})",
                documents_b.len(),
                documents_c.len()
            )));
        }
        let mut checkpoint_total = 0u64;
        for ((case_id, collected_b), (case_id_c, collected_c)) in
            documents_b.iter().zip(documents_c.iter())
        {
            if case_id != case_id_c {
                return Err(Box::<dyn std::error::Error>::from(format!(
                    "snapshot-every-checkpoint: case order differs between collecting runs ({case_id} vs {case_id_c})"
                )));
            }
            if collected_b.checkpoints != collected_c.checkpoints {
                return Err(Box::<dyn std::error::Error>::from(format!(
                    "snapshot-every-checkpoint: case {case_id} reached different checkpoints across runs"
                )));
            }
            for (index, (document_b, document_c)) in collected_b
                .documents
                .iter()
                .zip(collected_c.documents.iter())
                .enumerate()
            {
                if document_b != document_c {
                    return Err(Box::<dyn std::error::Error>::from(format!(
                        "snapshot-every-checkpoint: case {case_id} checkpoint {} document differs between runs",
                        collected_b.checkpoints[index]
                    )));
                }
            }
            for (index, document) in collected_b.documents.iter().enumerate() {
                let checkpoint = collected_b.checkpoints[index];
                checkpoint_total += 1;
                // `check` runs every decoder pass, canonical re-encode
                // included, and returns the issues it finds (an `Err` only
                // for a document it cannot parse at all).
                let issues = scxml_snapshot::check(document).map_err(|error| {
                    Box::<dyn std::error::Error>::from(format!(
                        "snapshot-every-checkpoint: case {case_id} checkpoint {checkpoint} document fails check: {error}"
                    ))
                })?;
                if let Some(issue) = issues.first() {
                    return Err(Box::<dyn std::error::Error>::from(format!(
                        "snapshot-every-checkpoint: case {case_id} checkpoint {checkpoint} document fails check: {issue} ({} issues)",
                        issues.len()
                    )));
                }
            }
        }

        write_summary(suite, &documents_b, checkpoint_total)?;
        println!(
            "snapshot-every-checkpoint: {} executed cases, {checkpoint_total} documents, all checked, reproducible, non-perturbing",
            documents_b.len()
        );
        Ok(report_a)
    })();
    if result.is_err() {
        span.fail(ErrorCategory::Harness);
    }
    result
}

/// One collecting pass: every executed case's documents, in run order.
fn run_collecting(
    suite: &LoadedSuite,
    run_options: &RunOptions,
    collecting: &dyn Fn() -> Result<Box<dyn CaseExecutor>, Box<dyn std::error::Error>>,
) -> Result<CollectingRun, Box<dyn std::error::Error>> {
    let documents = RefCell::new(Vec::new());
    let sink: scxml_conformance::SnapshotSink<'_> =
        &|case_id: &str, snapshots: &[CaseSnapshotDocument]| {
            documents.borrow_mut().push((
                case_id.to_owned(),
                CollectedCase {
                    checkpoints: snapshots.iter().map(|s| s.checkpoint).collect(),
                    documents: snapshots.iter().map(|s| s.document.clone()).collect(),
                },
            ));
        };
    let report =
        run_suite_with_snapshot_sink(suite, run_options, collecting()?.as_ref(), Some(sink))?;
    Ok((report, documents.into_inner()))
}

/// Canonical report JSON bytes (the exact bytes `write_json` persists,
/// without touching the filesystem).
fn canonical_report_bytes(report: &RunReport) -> Result<Vec<u8>, scxml_conformance::ReportError> {
    report.canonical_bytes()
}

/// Writes the per-model summary under `target/snapshot-proof/`.
fn write_summary(
    suite: &LoadedSuite,
    documents: &[(String, CollectedCase)],
    checkpoint_total: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let datamodel = &suite.manifest.suite.datamodel;
    let mut cases = BTreeMap::new();
    for (case_id, collected) in documents {
        let documents_json: Vec<serde_json::Value> = collected
            .documents
            .iter()
            .enumerate()
            .map(|(index, document)| {
                serde_json::json!({
                    "checkpoint": collected.checkpoints[index],
                    "sha256": sha256_bytes(document),
                })
            })
            .collect();
        cases.insert(
            case_id.clone(),
            serde_json::json!({
                "checkpoints": collected.checkpoints,
                "documents": documents_json,
            }),
        );
    }
    let summary = serde_json::json!({
        "datamodel": datamodel,
        "executed_cases": documents.len(),
        "checkpoint_documents": checkpoint_total,
        "cases": cases,
    });
    let mut bytes = serde_json::to_vec_pretty(&summary)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_SUMMARY_BYTES {
        return Err(Box::<dyn std::error::Error>::from(
            "snapshot-every-checkpoint: summary exceeds the 32 MiB bound",
        ));
    }
    let dir = std::env::current_dir()?
        .join("target")
        .join("snapshot-proof");
    fs::create_dir_all(&dir)?;
    let path: PathBuf = dir.join(format!("{datamodel}.json"));
    fs::write(&path, &bytes)?;
    println!(
        "snapshot-every-checkpoint: summary written to {}",
        path.display()
    );
    Ok(())
}
