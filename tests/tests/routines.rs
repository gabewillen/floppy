//! floppy's Routines ability (`abilities/routines/routines.scx`), run
//! through `scx run` on the logical clock under
//! `tests/fixtures/phase5/routines_harness.scx`, which stands in for the
//! cognition.
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

/// The routine the harness stores as `name`, ticking every 60 s with
/// `text`: the harness's `routine` template.
fn routine(name: &str, text: &str) -> String {
    format!(
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="waiting"><state id="waiting"><onentry><send event="every" delay="60s"/></onentry><transition event="every" target="waiting"><send event="bot.behavior.tick" target="#_parent"><content expr="({{name: '{name}', trigger: 'every', scheduled_at: '2026-10-05T08:00:00-04:00', fired_at: '2026-10-05T08:00:00-04:00', output: [{{event: 'bot.speaking.say', data: {{text: '{text}'}}}}]}})"/></send></transition></state></scxml>"##
    )
}

/// Writes each `(name, program)` of `programs` as `<name>.scx` into the
/// behaviors directory of the scratch root `dir`, where a row's source,
/// `../../behaviors/<name>.scx`, names it (decision D121).
fn write_programs(dir: &Path, programs: &[(&str, &str)]) {
    let behaviors = dir.join("behaviors");
    std::fs::create_dir_all(&behaviors).expect("the behaviors directory is creatable");
    for (name, program) in programs {
        std::fs::write(behaviors.join(format!("{name}.scx")), program)
            .expect("the program is writable");
    }
}

/// Lays out the routines, their runner and the ability schemas with the
/// harness beside them, and the programs of its rows (revision one);
/// `database` (an ECMAScript expression) replaces the harness's database
/// path. Returns the harness.
fn layout(database: &str) -> PathBuf {
    let dir = unique_scratch("routines");
    for name in ["memory", "routines", "routines_runner"] {
        support::copy_ability(name, &dir.join("abilities"));
    }
    let programs: Vec<(&str, String)> = ["Alpha", "Beta", "Gamma", "Delta"]
        .into_iter()
        .map(|name| (name, routine(name, "one")))
        .chain([("Epsilon", "not scxml".to_owned())])
        .collect();
    write_programs(
        &dir,
        &programs
            .iter()
            .map(|(name, program)| (*name, program.as_str()))
            .collect::<Vec<_>>(),
    );
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

/// Runs `harness` (`<root>/abilities/harness.scx`), granting the files
/// service its root's behaviors directory when it binds it.
fn run(harness: &Path) -> Finished {
    let root = harness
        .parent()
        .and_then(Path::parent)
        .expect("the harness is <root>/abilities/harness.scx");
    let mut args = vec!["--clock".to_owned(), "logical".to_owned()];
    if support::binds(harness, "files") {
        args.extend([
            "--dir".to_owned(),
            format!(
                "{}={}::/behaviors",
                support::service_uri("files"),
                root.join("behaviors").display()
            ),
        ]);
    }
    args.push(harness.display().to_string());
    Run::start(&args, &[]).finish(WAIT)
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
/// turn behavior do not, and one whose program is not SCXML stays stopped.
/// A reconcile after the inventory changed restarts the changed routine on
/// its new program, written over its path (its row's `updated_at` changed),
/// and stops the deleted one.
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
/// (empty here) and logs `bot.ability.routines.reconcile.failed`.
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
            "--dir".to_owned(),
            format!(
                "{}={}::/behaviors",
                support::service_uri("files"),
                unique_scratch("routines-behaviors").display()
            ),
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

/// The text of reflection's `next_after` on one line, as the persistent
/// routine contract gives it for a routine's `<data id="next_after">`.
fn next_after() -> String {
    let script = std::fs::read_to_string(workspace().join("abilities/reflection/reflection.js"))
        .expect("reflection.js is readable");
    let marker = "var next_after = ";
    let start = script
        .find(marker)
        .expect("reflection.js defines next_after")
        + marker.len();
    let end = start + script[start..].find("\n};").expect("next_after ends") + 2;
    script[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text` as an XML attribute value between double quotes.
fn attribute(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

/// The data model has no time zone database, so `next_after` reads the
/// zone's POSIX TZ rule (decision D83). On
/// `tests/fixtures/phase5/at_cases.json` (the tz database's answers for a
/// spring-forward gap, a fall-back overlap, the 1 s firing tolerance,
/// southern and negative-DST zones, and random zones, times and weekdays)
/// it finds the same instant and wall time.
#[test]
fn next_after_finds_the_occurrences_zoneinfo_finds() {
    let helper = next_after();
    assert!(
        !helper.contains(['<', '&', '"', '\'']),
        "a routine carries it in an attribute as given: {helper}"
    );
    let cases = std::fs::read_to_string(workspace().join("tests/fixtures/phase5/at_cases.json"))
        .expect("the cases are readable");
    let document = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="checking">
  <datamodel>
    <data id="next_after" expr="{helper}"/>
    <data id="cases" expr="{cases}"/>
  </datamodel>
  <state id="checking">
    <onentry>
      <log label="cases" expr="cases.length"/>
      <log label="mismatches" expr="JSON.stringify(cases.filter(c =&gt; JSON.stringify(next_after(c, c.now)) !== JSON.stringify({{ms: c.ms, iso: c.iso}})).map(c =&gt; [c.tz, next_after(c, c.now), c.iso]))"/>
    </onentry>
    <transition target="done"/>
  </state>
  <final id="done"/>
</scxml>"#,
        cases = attribute(cases.trim())
    );
    let finished = support::run_document("next_after.scx", &document, &[], &[], WAIT);
    assert!(support::finished(&finished.records), "{}", finished.stderr);
    assert_eq!(texts(&finished, "cases"), strings(&["50"]));
    assert_eq!(texts(&finished, "mismatches"), strings(&["[]"]));
}

/// A routine on an `at` schedule, written as the persistent routine
/// contract says: its data model computes the next weekday 08:00 in
/// `America/New_York` with `next_after` and ticks there with
/// `scheduled_at` that occurrence. A routine whose tick selects nothing has
/// its send refused by the tick's schema, so nothing reaches the runner's
/// parent.
#[test]
fn an_at_routine_ticks_at_its_occurrence_and_an_empty_tick_is_refused() {
    let dir = unique_scratch("routines-at");
    support::copy_ability("routines_runner", &dir.join("abilities"));
    let at = format!(
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" name="MorningBriefing" initial="waiting">
  <datamodel>
    <data id="at" expr="({{time: '08:00', days: ['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun'], tz: 'America/New_York', rule: 'EST5EDT,M3.2.0,M11.1.0'}})"/>
    <data id="next_after" expr="{helper}"/>
    <data id="due" expr="null"/>
  </datamodel>
  <state id="waiting">
    <onentry>
      <assign location="due" expr="next_after(at, Date.now())"/>
      <send id="briefing-at" event="briefing.at" delayexpr="(due.ms - Date.now()) + 'ms'"/>
    </onentry>
    <onexit><cancel sendid="briefing-at"/></onexit>
    <transition event="briefing.at" target="waiting">
      <send event="bot.behavior.tick" target="#_parent">
        <content expr="({{name: 'MorningBriefing', trigger: 'at', scheduled_at: due.iso, fired_at: new Date().toISOString(), output: [{{event: 'bot.speaking.say', data: {{text: 'Good morning.'}}}}]}})"/>
      </send>
    </transition>
  </state>
</scxml>"##,
        helper = next_after()
    );
    let silent = r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" name="Silent" initial="waiting">
  <state id="waiting">
    <onentry><send event="silent.every" delay="500ms"/></onentry>
    <transition event="silent.every">
      <send event="bot.behavior.tick" target="#_parent">
        <content expr="({name: 'Silent', trigger: 'every', scheduled_at: new Date().toISOString(), fired_at: new Date().toISOString(), output: []})"/>
      </send>
    </transition>
    <transition event="error.execution">
      <log label="silent.refused" expr="_event.name"/>
    </transition>
  </state>
</scxml>"##;
    write_programs(&dir, &[("Silent", silent), ("MorningBriefing", &at)]);
    let routines = serde_json::json!([
        {"name": "Silent", "source": "../../behaviors/Silent.scx"},
        {"name": "MorningBriefing", "source": "../../behaviors/MorningBriefing.scx"},
    ]);
    let harness = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" name="harness" initial="live">
  <datamodel>
    <data id="ticks" expr="[]"/>
  </datamodel>
  <state id="live">
    <invoke id="runner" type="scxml" src="routines_runner/routines_runner.scx">
      <param name="routines" expr="{routines}"/>
    </invoke>
    <transition event="bot.behavior.tick" cond="ticks.length &lt; 2">
      <assign location="ticks" expr="ticks.concat([_event.data])"/>
    </transition>
    <transition event="bot.behavior.tick" target="done">
      <log label="harness.ticks" expr="JSON.stringify(ticks.concat([_event.data]))"/>
    </transition>
  </state>
  <final id="done"/>
</scxml>"#,
        routines = attribute(&routines.to_string())
    );
    let path = dir.join("abilities/harness.scx");
    std::fs::write(&path, harness).expect("the harness is writable");
    let finished = run(&path);
    assert!(support::finished(&finished.records), "{}", finished.stderr);
    assert_eq!(
        texts(&finished, "silent.refused"),
        strings(&["error.execution"])
    );
    let ticks: serde_json::Value = serde_json::from_str(
        texts(&finished, "harness.ticks")
            .first()
            .expect("the harness logs the ticks"),
    )
    .expect("the ticks are JSON");
    let ticks = ticks.as_array().expect("a list of ticks");
    assert_eq!(ticks.len(), 3);
    // On the logical clock the data model's Date follows the session's
    // clock, so each tick fires exactly at its occurrence: 08:00 in New
    // York (EDT on the logical clock's dates) is 12:00 UTC, on
    // successive days.
    let mut previous = String::new();
    for tick in ticks {
        assert_eq!(tick["name"], "MorningBriefing", "{tick}");
        assert_eq!(tick["trigger"], "at", "{tick}");
        let scheduled = tick["scheduled_at"].as_str().expect("scheduled_at");
        let fired = tick["fired_at"].as_str().expect("fired_at");
        assert_eq!(scheduled.get(10..), Some("T08:00:00-04:00"), "{tick}");
        assert_eq!(fired.get(10..), Some("T12:00:00.000Z"), "{tick}");
        assert_eq!(scheduled.get(..10), fired.get(..10), "{tick}");
        assert!(scheduled > previous.as_str(), "{tick}");
        scheduled.clone_into(&mut previous);
    }
}
