//! Rustc-style UI snapshot tests for the validator.
//!
//! Every `tests/ui/<case>.scx` is validated through the real CLI binary in
//! three formats and compared byte-for-byte with its snapshots:
//!
//! - `<case>.stderr`: rustc-style human rendering (no color);
//! - `<case>.json`: canonical `scxml.report` bytes (`--format json`);
//! - `<case>.sarif`: SARIF 2.1.0 (`--format sarif`);
//! - `<case>.fixed`: the source after applying every machine-applicable fix
//!   (only for cases that have one).
//!
//! A case named `valid-*` is a must-not-fire case: it must produce no
//! diagnostic of any severity, so false positives on correct documents fail
//! the suite.
//!
//! A case may start with the processing instruction `<?ui-args ARG...?>` to
//! pass extra CLI arguments (for example `--schema URI=PATH`); a comment
//! cannot carry them because XML forbids `--` inside comments. Run
//! `sh scripts/bless-fixtures.sh --ui` to rewrite snapshots after an
//! intentional change; review the diff before committing. The suite also proves determinism (two runs are identical),
//! exit-status consistency, diagnostic-code coverage against the canonical
//! explain table, and the rustfix round trips: applying all machine-applicable
//! edits removes those diagnostics and introduces no new diagnostic of any
//! severity, and applying any single fix, of any applicability, resolves the
//! diagnostic that offers it and introduces none either (after an XML parse
//! failure only new errors count, because later phases had not run).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use scxml_reporting::explain;
use scxml_reporting::{Applicability, ArtifactEnvelope, decode_canonical};

/// Codes that cannot be produced by `scx validate` on a document; each
/// is covered by a unit test at its producer instead.
/// The resource bounds behind these codes apply to wasm builds only
/// (`scxml_types::wasm_bound`); the native CLI never emits them, and
/// [`check_generated_budget_cases`] proves documents over each bound
/// validate without them.
const WASM_ONLY_BOUND: &str = "a resource bound of wasm builds only; the native CLI never emits it";

const NOT_REACHABLE_FROM_VALIDATE: &[(&str, &str)] = &[
    ("COMP001", "produced only by `compile` lowering"),
    ("LIMIT001", WASM_ONLY_BOUND),
    ("LIMIT002", WASM_ONLY_BOUND),
    ("LIMIT004", WASM_ONLY_BOUND),
    ("LIMIT005", WASM_ONLY_BOUND),
    ("COMP002", "produced only by `compile` package binding"),
    (
        "PKG002",
        "needs a failing package validator component; covered by validator-core package_validator tests and the CLI extension_packages test",
    ),
    (
        "COMP003",
        "produced only by `compile` script-package pinning; covered by compiler-core script_packages tests",
    ),
    (
        "PKG001",
        "produced only by the validator component's package requests",
    ),
    (
        "LINT010",
        "a note only when no checker for the machine's data model is linked, and `validate` links both; covered by the datamodel_check unit tests and the validator component test without_a_linked_checker_expressions_are_reported_unchecked",
    ),
    (
        "XML005",
        "caller configuration error; covered by xml_error unit tests",
    ),
    (
        "XSD004",
        "backend content-model preparation failure after successful compilation; ambiguous models are rejected earlier as XSD003",
    ),
    (
        "XSD005",
        "backend driver failure after successful parsing; no known document triggers it",
    ),
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root must resolve")
}

fn bless() -> bool {
    std::env::var_os("SCXML_BLESS").is_some()
}

fn cases(root: &Path) -> Vec<String> {
    let mut cases: Vec<String> = fs::read_dir(root.join("tests/ui"))
        .expect("tests/ui must exist")
        .filter_map(|entry| {
            let path = entry.expect("tests/ui entry must be readable").path();
            (path.extension().and_then(|extension| extension.to_str()) == Some("scx")).then(|| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .expect("UI case names must be UTF-8")
                    .to_owned()
            })
        })
        .collect();
    cases.sort();
    cases
}

fn case_args(source: &str) -> Vec<String> {
    source
        .lines()
        .next()
        .and_then(|line| line.trim().strip_prefix("<?ui-args"))
        .and_then(|rest| rest.strip_suffix("?>"))
        .map(|args| args.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

fn run(root: &Path, relative: &str, args: &[String], format: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_scx"))
        .current_dir(root)
        .arg("validate")
        .arg(relative)
        .args(args)
        .args(["--format", format])
        .output()
        .expect("scx must run")
}

fn compare(path: &Path, actual: &[u8], failures: &mut Vec<String>) {
    if bless() {
        fs::write(path, actual).expect("snapshot must be writable");
        return;
    }
    match fs::read(path) {
        Ok(expected) if expected == actual => {}
        Ok(expected) => failures.push(format!(
            "{} differs from actual output (run `sh scripts/bless-fixtures.sh --ui` to update)\n--- expected\n{}\n--- actual\n{}",
            path.display(),
            String::from_utf8_lossy(&expected),
            String::from_utf8_lossy(actual)
        )),
        Err(_) => failures.push(format!(
            "{} is missing (run `sh scripts/bless-fixtures.sh --ui` to create it)",
            path.display()
        )),
    }
}

fn artifact(bytes: &[u8]) -> ArtifactEnvelope {
    decode_canonical(bytes).expect("--format json must emit canonical validator bytes")
}

/// Applies every machine-applicable edit, skipping edits that overlap an
/// already accepted edit (rustfix semantics). Returns the fixed source and the
/// number of applied fixes per code.
fn apply_machine_applicable(artifact: &ArtifactEnvelope) -> (String, BTreeMap<String, usize>) {
    let payload = artifact
        .validator_payload()
        .expect("UI snapshots are validator artifacts");
    let mut edits = Vec::new();
    let mut applied = BTreeMap::new();
    for diagnostic in &payload.diagnostics {
        for fix in &diagnostic.fixes {
            if fix.applicability != Applicability::MachineApplicable {
                continue;
            }
            let overlaps = fix.edits.iter().any(|edit| {
                edits.iter().any(|(start, end, _): &(u64, u64, String)| {
                    edit.span.start < *end && *start < edit.span.end
                        || (edit.span.start == *start && edit.span.end == *end)
                })
            });
            if overlaps {
                continue;
            }
            for edit in &fix.edits {
                edits.push((edit.span.start, edit.span.end, edit.replacement.clone()));
            }
            *applied.entry(diagnostic.code.clone()).or_insert(0) += 1;
        }
    }
    edits.sort();
    let mut fixed = payload.source_text.clone();
    for (start, end, replacement) in edits.iter().rev() {
        let start = usize::try_from(*start).expect("span fits usize");
        let end = usize::try_from(*end).expect("span fits usize");
        fixed.replace_range(start..end, replacement);
    }
    (fixed, applied)
}

fn code_counts(artifact: &ArtifactEnvelope, errors_only: bool) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for diagnostic in &artifact
        .validator_payload()
        .expect("validator payload")
        .diagnostics
    {
        if errors_only && diagnostic.severity != scxml_reporting::Severity::Error {
            continue;
        }
        *counts.entry(diagnostic.code.clone()).or_insert(0) += 1;
    }
    counts
}

/// Snapshots one case in every format; returns its canonical report.
fn check_snapshots(
    root: &Path,
    case: &str,
    args: &[String],
    failures: &mut Vec<String>,
) -> ArtifactEnvelope {
    let relative = format!("tests/ui/{case}.scx");
    let human = run(root, &relative, args, "human");
    let human_again = run(root, &relative, args, "human");
    assert_eq!(
        human.stderr, human_again.stderr,
        "{case}: human output must be deterministic"
    );
    compare(
        &root.join(format!("tests/ui/{case}.stderr")),
        &human.stderr,
        failures,
    );

    let json = run(root, &relative, args, "json");
    let json_again = run(root, &relative, args, "json");
    assert_eq!(
        json.stdout, json_again.stdout,
        "{case}: canonical bytes must be deterministic"
    );
    assert!(
        json.stderr.is_empty(),
        "{case}: --format json must keep stderr clean"
    );
    compare(
        &root.join(format!("tests/ui/{case}.json")),
        &json.stdout,
        failures,
    );

    let sarif = run(root, &relative, args, "sarif");
    compare(
        &root.join(format!("tests/ui/{case}.sarif")),
        &sarif.stdout,
        failures,
    );

    let report = artifact(&json.stdout);
    let payload = report.validator_payload().expect("validator payload");
    let has_errors = payload.counts.errors > 0;
    for output in [&human, &json, &sarif] {
        assert_eq!(
            output.status.code(),
            Some(i32::from(has_errors)),
            "{case}: exit status must be 1 exactly when the report has errors"
        );
    }
    for diagnostic in &payload.diagnostics {
        assert!(
            diagnostic.help.is_some() && !diagnostic.notes.is_empty(),
            "{case}: {} must carry a note and actionable help",
            diagnostic.code
        );
        assert!(
            explain::lookup(&diagnostic.code).is_some(),
            "{case}: {} has no `--explain` entry",
            diagnostic.code
        );
    }
    report
}

/// Applies every machine-applicable fix and proves the rustfix round trip.
fn check_rustfix(
    root: &Path,
    case: &str,
    args: &[String],
    report: &ArtifactEnvelope,
    failures: &mut Vec<String>,
) {
    let (fixed_source, applied) = apply_machine_applicable(report);
    let fixed_snapshot = root.join(format!("tests/ui/{case}.fixed"));
    if applied.is_empty() {
        if fixed_snapshot.exists() {
            if bless() {
                fs::remove_file(&fixed_snapshot).expect("stale .fixed snapshot must be removable");
            } else {
                failures.push(format!(
                    "{case}: stale .fixed snapshot without machine-applicable fixes"
                ));
            }
        }
        return;
    }
    compare(&fixed_snapshot, fixed_source.as_bytes(), failures);
    let fixed_dir = root.join(RUSTFIX_DIR);
    let fixed_path = fixed_dir.join(format!("{case}.scx"));
    fs::write(&fixed_path, &fixed_source).expect("fixed source must be writable");
    let fixed_relative = fixed_path
        .strip_prefix(root)
        .expect("rustfix scratch is inside the workspace")
        .to_string_lossy()
        .into_owned();
    let revalidated = artifact(&run(root, &fixed_relative, args, "json").stdout);
    let before = code_counts(report, false);
    let after = code_counts(&revalidated, false);
    for (code, fixed_count) in &applied {
        let remaining = after.get(code).copied().unwrap_or(0);
        let original = before.get(code).copied().unwrap_or(0);
        assert!(
            remaining + fixed_count <= original,
            "{case}: applying machine-applicable fixes for {code} left {remaining} of {original}"
        );
    }
    let introduced = introduced_diagnostics(report, &revalidated);
    assert!(
        introduced.is_empty(),
        "{case}: machine-applicable fixes introduced new diagnostics: {introduced:?}"
    );
}

/// Whether the report stopped at the XML trust boundary, so no schema or
/// semantic rule has run yet.
fn parse_stage(report: &ArtifactEnvelope) -> bool {
    report
        .validator_payload()
        .expect("validator payload")
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code.starts_with("XML"))
}

/// Codes whose count grew from `before` to `after`. A fix must never trade
/// one finding for another (rustc's bar for suggestions). When `before`
/// stopped at XML parsing, later phases had not run, so only errors count
/// (a warning about the now-parsable document is information, not a
/// regression); otherwise every severity counts, schema findings included.
fn introduced_diagnostics(
    before: &ArtifactEnvelope,
    after: &ArtifactEnvelope,
) -> Vec<(String, usize, usize)> {
    let errors_only = parse_stage(before);
    let prior = code_counts(before, errors_only);
    code_counts(after, errors_only)
        .into_iter()
        .filter_map(|(code, count)| {
            let was = prior.get(&code).copied().unwrap_or(0);
            (count > was).then_some((code, was, count))
        })
        .collect()
}

/// Maybe-incorrect fixes whose application is expected to surface findings
/// that the original report could not contain, with the reason. Every other
/// maybe-incorrect fix must introduce no new diagnostic of any severity.
const MAYBE_INCORRECT_EXCEPTIONS: &[(&str, &str, &str)] = &[(
    "pkg005-unknown-datamodel",
    "PKG005",
    "selecting a recognized data model turns on expression checking, which reports the fixture's pre-existing invalid script",
)];

/// The JSON Schema files the cases' `event:schema` references name relative
/// to `tests/ui` (`tests/ui/event-schemas`), copied beside the fixed sources
/// in `dir`, so a fixed source resolves them exactly as its case does.
/// The scratch directory of the machine-applicable rustfix round trips.
const RUSTFIX_DIR: &str = "target/ui-rustfix";

/// The scratch directory of the per-fix round trips.
const EACH_FIX_DIR: &str = "target/ui-each-fix";

/// Creates both fix scratch directories with their schema mirrors once,
/// before any case runs: the cases run concurrently, and each writes only
/// files named after itself.
fn prepare_fix_scratch(root: &Path) {
    for dir in [RUSTFIX_DIR, EACH_FIX_DIR] {
        let dir = root.join(dir);
        fs::create_dir_all(&dir).expect("fix scratch directory must be creatable");
        mirror_schema_files(root, &dir);
    }
}

fn mirror_schema_files(root: &Path, dir: &Path) {
    let source = root.join("tests/ui/event-schemas");
    let target = dir.join("event-schemas");
    fs::create_dir_all(&target).expect("the schema mirror must be creatable");
    for entry in fs::read_dir(&source).expect("tests/ui/event-schemas must exist") {
        let path = entry.expect("schema entry must be readable").path();
        if let Some(name) = path.file_name() {
            fs::copy(&path, target.join(name)).expect("schema file must be copyable");
        }
    }
}

/// Applies every fix on its own (machine-applicable, maybe-incorrect, and
/// placeholder alike) and proves two things: it resolves the diagnostic that
/// offers it (one fewer finding with that code), and, unless it has
/// placeholders or is listed in [`MAYBE_INCORRECT_EXCEPTIONS`], it introduces
/// no new diagnostic. A
/// suggestion may be the wrong intent, but it must never leave its own
/// finding standing (such as a `= =` repair that yields `====`) or turn a
/// document into a new schema or semantic error.
fn check_each_fix_round_trip(
    root: &Path,
    case: &str,
    args: &[String],
    report: &ArtifactEnvelope,
    failures: &mut Vec<String>,
) {
    let payload = report
        .validator_payload()
        .expect("UI snapshots are validator artifacts");
    let fixed_dir = root.join(EACH_FIX_DIR);
    let before = code_counts(report, false);
    for (diagnostic_index, diagnostic) in payload.diagnostics.iter().enumerate() {
        for (fix_index, fix) in diagnostic.fixes.iter().enumerate() {
            // A placeholder is not valid until the author replaces it, so
            // only its resolution is checked.
            let excepted = fix.applicability == Applicability::HasPlaceholders
                || fix.applicability == Applicability::MaybeIncorrect
                    && MAYBE_INCORRECT_EXCEPTIONS
                        .iter()
                        .any(|(exception_case, code, _)| {
                            *exception_case == case && *code == diagnostic.code
                        });
            let mut edits: Vec<_> = fix
                .edits
                .iter()
                .map(|edit| (edit.span.start, edit.span.end, edit.replacement.clone()))
                .collect();
            edits.sort();
            let mut fixed = payload.source_text.clone();
            for (start, end, replacement) in edits.iter().rev() {
                let start = usize::try_from(*start).expect("span fits usize");
                let end = usize::try_from(*end).expect("span fits usize");
                fixed.replace_range(start..end, replacement);
            }
            let path = fixed_dir.join(format!("{case}-{diagnostic_index}-{fix_index}.scx"));
            fs::write(&path, &fixed).expect("fixed source must be writable");
            let relative = path
                .strip_prefix(root)
                .expect("scratch is inside the workspace")
                .to_string_lossy()
                .into_owned();
            let revalidated = artifact(&run(root, &relative, args, "json").stdout);
            let original = before.get(&diagnostic.code).copied().unwrap_or(0);
            let remaining = code_counts(&revalidated, false)
                .get(&diagnostic.code)
                .copied()
                .unwrap_or(0);
            if remaining >= original {
                failures.push(format!(
                    "{case}: fix `{}` for {} does not resolve it ({remaining} of {original} remain)",
                    fix.message, diagnostic.code
                ));
            }
            if excepted {
                continue;
            }
            let introduced = introduced_diagnostics(report, &revalidated);
            if !introduced.is_empty() {
                failures.push(format!(
                    "{case}: fix `{}` for {} introduced new diagnostics {introduced:?}",
                    fix.message, diagnostic.code
                ));
            }
        }
    }
}

/// Budget codes whose smallest trigger is a ~256 KiB document. Their inputs
/// are generated here instead of checked in (a snapshot would embed the whole
/// source three times); each must produce exactly its one fail-closed error.
fn generated_budget_cases() -> Vec<(&'static str, String)> {
    const HEAD: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a" datamodel="ecmascript">"#;
    vec![
        (
            // One more state than the wasm graph bound (65,536 nodes, with
            // the root), one per line.
            "LIMIT002",
            format!(
                "{HEAD}\n<state id=\"a\"><transition event=\"go\" target=\"s0\"/></state>\n{}<final id=\"s65536\"/>\n</scxml>",
                (0..65_536).fold(String::new(), |mut states, index| {
                    let _ = writeln!(
                        states,
                        "<state id=\"s{index}\"><transition event=\"go\" target=\"s{}\"/></state>",
                        index + 1
                    );
                    states
                })
            ),
        ),
        (
            // One byte past the wasm expression bound (16 KiB). LIMIT003
            // stays reachable natively through the nesting guard, a stack
            // guard on every build (`tests/ui/limit003-*`).
            "LIMIT003",
            format!(
                r#"{HEAD}<state id="a"><onentry><log expr="1{}"/></onentry><transition event="go" target="b"/></state><final id="b"/></scxml>"#,
                " ".repeat(16_384)
            ),
        ),
        (
            // One more expression than the wasm checking bound (16,384), in
            // two blocks so no element exceeds the wasm Machine IR bound of
            // 10,000 child nodes either.
            "LIMIT004",
            format!(
                r#"{HEAD}<state id="a"><onentry>{}</onentry><onentry>{}</onentry><transition event="go" target="b"/></state><final id="b"/></scxml>"#,
                r#"<log expr="1"/>"#.repeat(8_193),
                r#"<log expr="1"/>"#.repeat(8_192)
            ),
        ),
    ]
}

/// Runs every generated case over a wasm-only bound through the real
/// (native) CLI, which must not report the bound's code.
fn check_generated_budget_cases(root: &Path, observed_codes: &mut BTreeSet<String>) {
    let directory = root.join("target/ui-generated");
    fs::create_dir_all(&directory).expect("generated UI directory must be creatable");
    for (code, source) in generated_budget_cases() {
        let path = directory.join(format!("{code}.scx"));
        fs::write(&path, source).expect("generated UI case must be writable");
        let relative = path
            .strip_prefix(root)
            .expect("generated cases are inside the workspace")
            .to_string_lossy()
            .into_owned();
        let output = run(root, &relative, &[], "json");
        let again = run(root, &relative, &[], "json");
        assert_eq!(
            output.stdout, again.stdout,
            "{code}: output must be deterministic"
        );
        let report = artifact(&output.stdout);
        let diagnostics = &report
            .validator_payload()
            .expect("validator payload")
            .diagnostics;
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.severity != scxml_reporting::Severity::Error),
            "{code}: a document over the wasm bound validates natively: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code.as_str())
                .collect::<Vec<_>>()
        );
        observed_codes.extend(diagnostics.iter().map(|diagnostic| diagnostic.code.clone()));
    }
}

/// Every documented code reachable from `validate` has at least one case.
fn check_coverage(observed_codes: &BTreeSet<String>) {
    let unreachable: BTreeSet<&str> = NOT_REACHABLE_FROM_VALIDATE
        .iter()
        .map(|(code, _)| *code)
        .collect();
    for entry in explain::CODES {
        if unreachable.contains(entry.code) {
            continue;
        }
        let covered = if entry.family {
            !entry.self_checking
                || observed_codes.iter().any(|code| {
                    code.strip_prefix(entry.code)
                        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
                })
        } else {
            observed_codes.contains(entry.code)
        };
        assert!(covered, "no tests/ui case covers {}", entry.code);
    }
}

/// The mismatches one case found and the diagnostic codes it observed.
type CaseResult = (Vec<String>, BTreeSet<String>);

/// Checks one case: its snapshots, its must-not-fire rule, and its fix round
/// trips. Returns the mismatches it found and the codes it observed.
fn check_case(root: &Path, case: &str) -> CaseResult {
    let mut failures = Vec::new();
    let source = fs::read_to_string(root.join(format!("tests/ui/{case}.scx")))
        .expect("UI case must be UTF-8");
    let args = case_args(&source);
    let report = check_snapshots(root, case, &args, &mut failures);
    if case.starts_with("valid-") {
        let diagnostics = &report
            .validator_payload()
            .expect("validator payload")
            .diagnostics;
        if !diagnostics.is_empty() {
            failures.push(format!(
                "{case}: must-not-fire case produced {:?}",
                diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.code.as_str())
                    .collect::<Vec<_>>()
            ));
        }
    }
    let observed_codes = report
        .validator_payload()
        .expect("validator payload")
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.clone())
        .collect();
    check_rustfix(root, case, &args, &report, &mut failures);
    check_each_fix_round_trip(root, case, &args, &report, &mut failures);
    (failures, observed_codes)
}

#[test]
fn ui_snapshots_round_trips_and_code_coverage() {
    let root = workspace_root();
    let cases = cases(&root);
    assert!(!cases.is_empty(), "tests/ui must contain cases");
    prepare_fix_scratch(&root);
    // Every case is independent (its own `scx` processes and scratch
    // files), so the cases run on a pool of threads; their results are
    // merged in case order, so the report reads as a sequential run's.
    let next = AtomicUsize::new(0);
    let results: Vec<Mutex<Option<CaseResult>>> = cases.iter().map(|_| Mutex::new(None)).collect();
    let workers = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    let mut observed_codes = BTreeSet::new();
    std::thread::scope(|scope| {
        let generated = scope.spawn(|| {
            let mut codes = BTreeSet::new();
            check_generated_budget_cases(&root, &mut codes);
            codes
        });
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(case) = cases.get(index) else { break };
                    let result = check_case(&root, case);
                    *results[index]
                        .lock()
                        .expect("no case panicked holding its slot") = Some(result);
                }
            });
        }
        observed_codes = generated
            .join()
            .expect("the generated budget cases complete");
    });
    let mut failures = Vec::new();
    for slot in results {
        let (case_failures, case_codes) = slot
            .into_inner()
            .expect("no case panicked holding its slot")
            .expect("every case ran");
        failures.extend(case_failures);
        observed_codes.extend(case_codes);
    }
    assert!(
        failures.is_empty(),
        "{} UI snapshot mismatch(es):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    check_coverage(&observed_codes);
}
