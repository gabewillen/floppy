//! floppy's Routines ability (`abilities/routines/routines.scx`, mosfet
//! `abilities/cognition/routines.py`), run through `scx run` on the logical
//! clock under `tests/fixtures/phase5/routines_harness.scx`, which stands in
//! for the cognition.
//!
//! The harness stores behavior inventory rows through the Memory ability in
//! a directory the run grants, invokes the routines with the database path
//! as `<param name="database">`, changes the inventory and asks for a
//! reconcile. The routines log `bot.ability.routines.reconciled` (the
//! running set's names) and `bot.ability.routines.reconcile.failed`; the
//! harness logs `harness.tick` (the first tick of each routine revision),
//! `harness.stale` and `harness.unexpected`.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use support::{Finished, Run, logs, unique_scratch, workspace};

/// Bounded wait for the whole run (scx startup included).
const WAIT: Duration = Duration::from_secs(120);

/// Lays out the routines, their runner and the ability schemas with the
/// harness beside them; `database` (an ECMAScript expression) replaces the
/// harness's database path. Returns the harness.
fn layout(database: &str) -> PathBuf {
    let dir = unique_scratch("routines");
    for name in ["memory", "routines", "routines_runner"] {
        support::copy_ability(name, &dir.join("abilities"));
    }
    let harness =
        std::fs::read_to_string(workspace().join("tests/fixtures/phase5/routines_harness.scx"))
            .expect("the harness is readable")
            .replace(
                r#"<data id="database" expr="'/memory/floppy.sqlite'"/>"#,
                &format!(r#"<data id="database" expr="{database}"/>"#),
            );
    let path = dir.join("abilities/harness.scx");
    std::fs::write(&path, harness).expect("the harness is writable");
    path
}

fn run(harness: &Path) -> Finished {
    let run = Run::start(
        &[
            "--clock".to_owned(),
            "logical".to_owned(),
            harness.display().to_string(),
        ],
        &[],
    );
    run.finish(WAIT)
}

fn texts(finished: &Finished, label: &str) -> Vec<String> {
    logs(&finished.records)
        .into_iter()
        .filter(|(found, _)| found == label)
        .map(|(_, text)| text)
        .collect()
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|&value| value.to_owned()).collect()
}

/// The ACTIVE persistent routines start and tick; a BROKEN routine and a
/// turn behavior do not, and one whose source is not SCXML stays stopped. A reconcile after the inventory changed restarts
/// the changed routine on its new source and stops the deleted one.
#[test]
fn routines_run_the_active_persistent_rows_and_reconcile_their_changes() {
    let finished = run(&layout("'/memory/floppy.sqlite'"));
    assert!(support::finished(&finished.records), "{}", finished.stderr);
    assert_eq!(
        texts(&finished, "bot.ability.routines.reconciled"),
        strings(&["Alpha Beta Epsilon", "Alpha Epsilon"]),
        "{:?}",
        logs(&finished.records)
    );
    assert_eq!(
        texts(&finished, "harness.tick"),
        strings(&["Alpha one", "Beta one", "Alpha two"])
    );
    assert_eq!(texts(&finished, "harness.stale"), strings(&["[]"]));
    assert!(texts(&finished, "harness.unexpected").is_empty());
    assert!(texts(&finished, "bot.ability.routines.reconcile.failed").is_empty());
    // A source that is not SCXML stays stopped; the others run.
    assert_eq!(
        texts(&finished, "routines.failed"),
        strings(&["Epsilon (error.execution)", "Epsilon (error.execution)"])
    );
}

/// An inventory the Memory ability cannot open keeps the running set
/// (empty here) and logs mosfet's reconcile failure.
#[test]
fn a_failed_inventory_load_keeps_the_running_set() {
    let harness = layout("'/memory/floppy.sqlite'");
    let text = std::fs::read_to_string(&harness).expect("the harness is readable");
    // The routines get a database path the service refuses.
    std::fs::write(
        &harness,
        text.replace(
            r#"src="routines/routines.scx">
        <param name="database" expr="database"/>"#,
            r#"src="routines/routines.scx">
        <param name="database" expr="'relative.sqlite'"/>"#,
        ),
    )
    .expect("the harness is writable");
    let finished = Run::start(
        &[
            "--clock".to_owned(),
            "logical".to_owned(),
            harness.display().to_string(),
        ],
        &[],
    );
    let records = finished.until(WAIT, |record| {
        record["type"] == "log" && record["label"] == "bot.ability.routines.reconcile.failed"
    });
    let failures: Vec<_> = logs(&records)
        .into_iter()
        .filter(|(label, _)| label == "bot.ability.routines.reconcile.failed")
        .collect();
    assert_eq!(failures.len(), 1, "{records:?}");
    assert!(
        failures[0].1.starts_with("Routine inventory load failed: "),
        "{failures:?}"
    );
    assert!(
        logs(&records)
            .iter()
            .all(|(label, _)| label != "bot.ability.routines.reconciled")
    );
    finished.close(WAIT);
}
