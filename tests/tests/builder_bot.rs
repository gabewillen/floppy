//! Harness of the builder bot (`bots/builder/builder.scx`), run exactly as
//! its README says: `scx run --env-file <file> --dir <generate>=<dir>::/media
//! bots/builder/builder.scx` (the harness grants the media directory,
//! `support::media_grant`), with the `builder.request` on standard input. Its packages resolve from
//! their repository URIs (the test cache `scripts/build-components.sh`
//! published), its `env` attributes scope the values, and it ends in the
//! final state `done` or `failed`, whose `<donedata>` `scx run` prints in its
//! `done` record. The session binds the real `scx` (embedded in `scx`) and `decision`
//! extension components and the real `generate` and `decision` service
//! components; the services talk to one deterministic fake backend on
//! 127.0.0.1 (Chat Completions and `TypeSafe` `system_one`; nothing reaches
//! the network) whose canned drafts include an invalid one, so the repair
//! loop, the machine-applicable fixes, the decision, and the simulation as
//! an invoked child all run. One route answers its first plan late, so a
//! `builder.timeout` fires while that generation is in flight and cancels
//! it.
//!
//! The ignored `live_builder_through_an_env_file` test builds the same spec
//! with the real backends: `FLOPPY_LIVE_ENV_FILE=<paths> cargo test -p
//! floppy-tests --test builder_bot -- --ignored`, the paths joined like
//! `PATH` when `FLOPPY_API_KEY` and `TYPESAFE_API_KEY` live in separate
//! files. The values are the files'; `FLOPPY_BASE_URL` is required (the
//! generate service has no default backend), and without a
//! `TYPESAFE_BASE_URL` the decision service uses its package's declared
//! origin (decision D18). The model is `FLOPPY_LIVE_MODEL`, else
//! `gemma-4-26b-a4b`. It writes the produced bot under `target/builder-live/`, and prints pass or
//! fail and counts only; no value is ever printed.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value as Json, json};
use support::model_facing::{read, rendered};
use support::{Run, output, scratch, workspace, write};

/// The fake keys (not secrets): the mock checks they arrive as bearers.
const FAKE_FLOPPY_KEY: &str = "sk-floppy-fake";
const FAKE_TYPESAFE_KEY: &str = "ts-floppy-fake";
const SPEC: &str = "a traffic light with a pedestrian button";

fn builder_dir() -> PathBuf {
    workspace().join("bots/builder")
}

fn builder_source() -> String {
    std::fs::read_to_string(builder_dir().join("builder.scx")).expect("builder.scx is readable")
}

/// The builder document the README runs, laid out with its script and
/// schemas in a scratch layout of its own ([`builder_layout`]).
fn builder_document() -> PathBuf {
    builder_layout(&builder_source())
}

/// `builder` as `<layout>/bots/builder/builder.scx` in a fresh scratch
/// layout, with `builder.js` and `schemas/` beside it: the documents it
/// writes go to that layout's `behaviors/` (the run's files grant,
/// `support::behaviors_grant`, decision D121), so parallel tests never share
/// them.
fn builder_layout(builder: &str) -> PathBuf {
    let dir = support::unique_scratch("builder-layout").join("bots/builder");
    std::fs::create_dir_all(dir.join("schemas")).expect("the layout is creatable");
    std::fs::copy(builder_dir().join("builder.js"), dir.join("builder.js"))
        .expect("the builder's script is copyable");
    for entry in std::fs::read_dir(builder_dir().join("schemas")).expect("schemas/ is listable") {
        let path = entry.expect("a directory entry").path();
        std::fs::copy(
            &path,
            dir.join("schemas").join(path.file_name().expect("a name")),
        )
        .expect("the schema is copyable");
    }
    std::fs::create_dir_all(dir.join("xsd")).expect("the layout is creatable");
    std::fs::copy(
        builder_dir().join("xsd/scxml-1.0.json"),
        dir.join("xsd/scxml-1.0.json"),
    )
    .expect("the builder's schema is copyable");
    write(&dir, "builder.scx", builder)
}

/// The behaviors directory of the layout `document` (a [`builder_layout`])
/// sits in.
fn behaviors_of(document: &Path) -> PathBuf {
    document
        .ancestors()
        .nth(3)
        .expect("the document is <layout>/bots/builder/builder.scx")
        .join("behaviors")
}

/// The flags of the builder's README command on `clock`, running
/// `document` (the builder, or a variant of it).
fn builder_args(document: &Path, clock: &str) -> Vec<String> {
    vec![
        "--clock".to_owned(),
        clock.to_owned(),
        document.display().to_string(),
    ]
}

/// Starts `document` with `env_file` and sends it `request`.
fn start(
    document: &Path,
    clock: &str,
    env_files: &[PathBuf],
    request: &Json,
    env: &[(&str, &str)],
) -> Run {
    let mut args = builder_args(document, clock);
    for env_file in env_files {
        args.extend(["--env-file".to_owned(), env_file.display().to_string()]);
    }
    // The base URLs fall back to the calling shell's values (`env`).
    for (name, _) in env {
        args.extend(["--env".to_owned(), (*name).to_owned()]);
    }
    let mut run = Run::start(&args, env);
    run.event("builder.request", request);
    run.end_input();
    run
}

fn finished(records: &[Json]) -> bool {
    support::finished(records)
}

/// The builder's answer: the final state it ended in (`done` or `failed`,
/// its last progress `log` label) and that state's done data (the root's
/// `done` record).
fn answer(records: &[Json]) -> (String, Json) {
    let root: Vec<&Json> = records
        .iter()
        .filter(|record| record.get("parent").is_none())
        .collect();
    let done = root
        .iter()
        .find(|record| record["type"] == "done")
        .unwrap_or_else(|| panic!("the builder finished with done data: {records:?}"));
    let label = root
        .iter()
        .filter(|record| record["type"] == "log")
        .filter_map(|record| record["label"].as_str())
        .next_back()
        .expect("the builder logs its final state")
        .to_owned();
    assert!(
        matches!(label.as_str(), "done" | "failed"),
        "the last progress log names the final state: {label}"
    );
    (label, done["data"].clone())
}

/// `scx validate` of `path` with `bindings`: the exit status and the
/// diagnostics.
fn validate(path: &Path, bindings: &[String]) -> support::Output {
    let mut args = vec!["validate".to_owned(), path.display().to_string()];
    args.extend_from_slice(bindings);
    output(&args)
}

/// Validates and compiles the builder's documents in `data` (its done or
/// failed data: `scxml`, `schemas` and `script`) as `<name>.scx`,
/// `<name>.json` and `<name>.js` beside each other, with no package (as the
/// builder's drafts are): whether it compiled.
fn compiles(name: &str, data: &Json) -> bool {
    let dir = support::unique_scratch("compile");
    write(&dir, &format!("{name}.json"), &data["schemas"].to_string());
    write(
        &dir,
        &format!("{name}.js"),
        data["script"].as_str().unwrap_or_default(),
    );
    let path = write(
        &dir,
        &format!("{name}.scx"),
        data["scxml"].as_str().unwrap_or_default(),
    );
    let compiled = output(&[
        "compile".to_owned(),
        path.display().to_string(),
        "--output".to_owned(),
        dir.join("out.scxml-ir.json").display().to_string(),
    ]);
    compiled.status.success()
}

// ---------------------------------------------------------------------------
// Canned backend answers.

const PLAN: &str = r#"{"name":"traffic_light","summary":"A vehicle light that cycles green, yellow, red, and serves a pedestrian button by ending green early.","states":[{"id":"green","kind":"atomic","parent":"","purpose":"Vehicles go."},{"id":"yellow","kind":"atomic","parent":"","purpose":"Vehicles prepare to stop."},{"id":"red","kind":"atomic","parent":"","purpose":"Vehicles stop; pedestrians walk."}],"events":[{"name":"pedestrian.button","direction":"input","description":"A pedestrian pressed the button."},{"name":"light.green","direction":"output","description":"The light turned green."},{"name":"light.yellow","direction":"output","description":"The light turned yellow."},{"name":"light.red","direction":"output","description":"The light turned red."},{"name":"walk.on","direction":"output","description":"Pedestrians may cross."},{"name":"light.timer","direction":"internal","description":"The phase timer expired."}],"transitions":[{"source":"green","event":"pedestrian.button","target":"yellow","description":"End green early."},{"source":"green","event":"light.timer","target":"yellow","description":"Green expired."},{"source":"yellow","event":"light.timer","target":"red","description":"Yellow expired."},{"source":"red","event":"light.timer","target":"green","description":"Red expired."}]}"#;

/// The plan the slow route answers too late: the builder has cancelled
/// that generation by then, so it never reaches the report.
const LATE_PLAN: &str = r#"{"name":"late_plan","summary":"Answered after the timeout.","states":[],"events":[],"transitions":[]}"#;

/// The first event of the slow route's streamed first plan, after which it
/// stalls: the start of an assistant message, with nothing in it yet.
const STALLED_EVENT: &str = "data: {\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"finish_reason\":null}]}\n\n";

/// The longest the slow route holds its first plan answer while it waits
/// for the builder to ask for the redesign: a hang guard only. The answer is
/// released by the redesign request, so no assertion depends on time.
const SLOW_PLAN_HOLD_BOUND: Duration = Duration::from_mins(3);

const BUTTON_SCHEMA: &str = r#"{"type":"object","additionalProperties":false}"#;

/// The button's schema as the drafts' `schemas` carry it.
fn button_schema() -> Json {
    serde_json::from_str(BUTTON_SCHEMA).expect("the button schema is JSON")
}

/// The drafts' `schemas`: one schema per input event, under `$defs`.
fn schemas(events: &[&str]) -> Json {
    let defs: Map<String, Json> = events
        .iter()
        .map(|event| ((*event).to_owned(), button_schema()))
        .collect();
    json!({"$defs": defs})
}

/// The traffic light with `{green}` as its green state's transitions and
/// `{yellow_target}` as the target of yellow's timer.
fn traffic_light(green: &str, yellow_target: &str) -> String {
    format!(
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event" version="1.0" datamodel="ecmascript" name="traffic_light" initial="green">
  <state id="green">
    <onentry><send target="#_parent" event="light.green"/><send id="green-timer" event="light.timer" delay="30s"/></onentry>
    <onexit><cancel sendid="green-timer"/></onexit>
    {green}
  </state>
  <state id="yellow">
    <onentry><send target="#_parent" event="light.yellow"/><send id="yellow-timer" event="light.timer" delay="3s"/></onentry>
    <onexit><cancel sendid="yellow-timer"/></onexit>
    <transition event="light.timer" target="{yellow_target}"/>
  </state>
  <state id="red">
    <onentry><send target="#_parent" event="light.red"/><send target="#_parent" event="walk.on"/><send id="red-timer" event="light.timer" delay="20s"/></onentry>
    <onexit><cancel sendid="red-timer"/><send target="#_parent" event="walk.off"/></onexit>
    <transition event="light.timer" target="green"/>
  </state>
</scxml>"##
    )
}

/// The traffic light's green transitions, separately: the button's schema
/// is a reference into the draft's own `traffic_light.json` (the plan's
/// name).
fn green_separate() -> String {
    r#"<transition event="pedestrian.button" event:schema="traffic_light.json#/$defs/pedestrian.button" target="yellow"/>
    <transition event="light.timer" target="yellow"/>"#
        .to_owned()
}

/// The same transitions merged with a comma-separated descriptor list
/// (`SCXML013`, whose fix is machine-applicable). It carries no
/// `event:schema`: on a transition handling `light.timer` the button's
/// object schema would reject the timer's own data (none), so the draft's
/// `light.timer` send would raise `error.execution` (scxml.rs runtime event
/// contracts).
fn green_comma() -> String {
    r#"<transition event="pedestrian.button,light.timer" target="yellow"/>"#.to_owned()
}

/// The test scripts every draft carries.
fn tests() -> Json {
    json!([
        {
            "name": "button ends green early",
            "events": [{"name": "pedestrian.button", "data": {}, "delay_ms": 1000}],
            "settle_ms": 6000,
            "expect_events": ["light.green", "light.yellow", "light.red", "walk.on"],
            "expect_done": false,
        },
        {
            "name": "the light starts green",
            "events": [],
            "settle_ms": 2000,
            "expect_events": ["light.green"],
            "expect_done": false,
        },
    ])
}

/// A draft: the document, the button's schema, no script, and two test
/// scripts.
fn draft(scxml: &str) -> String {
    json!({
        "scxml": scxml,
        "schemas": schemas(&["pedestrian.button"]),
        "script": "",
        "tests": tests(),
    })
    .to_string()
}

/// The script of [`scripted_draft`].
const SCRIPT: &str = "function mayGo() {\n  return true;\n}\n";

/// A valid draft with a script: its root `<script src="traffic_light.js"/>`
/// and red's timer guarded by the script's `mayGo()`.
fn scripted_draft() -> String {
    json!({
        "scxml": traffic_light(&green_separate(), "red")
            .replacen(
                r#"initial="green">"#,
                r#"initial="green">
    <script src="traffic_light.js"/>"#,
                1,
            )
            .replace(
                r#"<transition event="light.timer" target="green"/>"#,
                r#"<transition event="light.timer" cond="mayGo()" target="green"/>"#,
            ),
        "schemas": schemas(&["pedestrian.button"]),
        "script": SCRIPT,
        "tests": tests(),
    })
    .to_string()
}

/// The invalid draft: yellow's timer targets a state that does not exist.
fn invalid_draft() -> String {
    draft(&traffic_light(&green_separate(), "amber"))
}

/// A draft whose only error has a machine-applicable fix.
fn fixable_draft() -> String {
    draft(&traffic_light(&green_comma(), "red"))
}

/// A draft whose green timer is a delayed send to `#_internal`
/// (`SCXML070`, whose fix is only maybe-incorrect, so it is not applied).
fn internal_delay_draft() -> String {
    draft(&traffic_light(&green_separate(), "red").replace(
        "<send id=\"green-timer\" event=\"light.timer\" delay=\"30s\"/>",
        "<send id=\"green-timer\" target=\"#_internal\" event=\"light.timer\" delay=\"30s\"/>",
    ))
}

/// [`invalid_draft`] (`SCXML005`) whose green timer is also a delayed send
/// to `#_internal` (`SCXML070`).
fn invalid_internal_delay_draft() -> String {
    draft(&traffic_light(&green_separate(), "amber").replace(
        "<send id=\"green-timer\" event=\"light.timer\" delay=\"30s\"/>",
        "<send id=\"green-timer\" target=\"#_internal\" event=\"light.timer\" delay=\"30s\"/>",
    ))
}

/// [`invalid_draft`] (`SCXML005`) whose red timer also has both `delay` and
/// `delayexpr` (`SCXML014`).
fn invalid_two_delays_draft() -> String {
    draft(&traffic_light(&green_separate(), "amber").replace(
        "<send id=\"red-timer\" event=\"light.timer\" delay=\"20s\"/>",
        "<send id=\"red-timer\" event=\"light.timer\" delay=\"20s\" delayexpr=\"'20s'\"/>",
    ))
}

/// A draft that invokes the `generate` service the builder's run binds,
/// with the builder's own `env` scopes, and reports its answer to its
/// invoker: its test expects `answer.ready` and the child to finish.
fn service_draft() -> String {
    json!({
        "scxml": r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env" version="1.0" datamodel="ecmascript" name="asker" initial="ask">
  <state id="ask">
    <invoke type="https://github.com/stateforward/floppy/services/generate" env:apikey="FLOPPY_API_KEY" env:baseurl="FLOPPY_BASE_URL">
      <param name="model" expr="'gemma-4-26b-a4b'"/>
      <param name="prompt" expr="'say hello'"/>
    </invoke>
    <transition event="generate.result" target="answered"/>
    <transition event="generate.failed" target="unanswered"/>
  </state>
  <final id="answered"><onentry><send target="#_parent" event="answer.ready"/></onentry></final>
  <final id="unanswered"><onentry><send target="#_parent" event="answer.failed"/></onentry></final>
</scxml>"##,
        "schemas": schemas(&[]),
        "script": "",
        "tests": [{
            "name": "the draft asks generate and answers",
            "events": [],
            "settle_ms": 60000,
            "expect_events": ["answer.ready"],
            "expect_done": true,
        }],
    })
    .to_string()
}

/// A valid draft whose green state sends the builder's own control event
/// names to `#_parent`; its test expects them as outputs.
fn impostor_draft() -> String {
    json!({
        "scxml": traffic_light(&green_separate(), "red").replace(
            "<onentry><send target=\"#_parent\" event=\"light.green\"/>",
            "<onentry><send target=\"#_parent\" event=\"sim.settle\"/><send target=\"#_parent\" event=\"sim.drive\"/><send target=\"#_parent\" event=\"light.green\"/>",
        ),
        "schemas": schemas(&["pedestrian.button"]),
        "script": "",
        "tests": [{
            "name": "control names are outputs",
            "events": [],
            "settle_ms": 2000,
            "expect_events": ["sim.settle", "sim.drive", "light.green"],
            "expect_done": false,
        }],
    })
    .to_string()
}

/// A valid draft whose green state first sends `error.stale` and then
/// `builder.timeout` to `#_parent`: the error ends each test, so the
/// `builder.timeout` is still queued when the candidate's invocation is
/// cancelled.
fn stale_timeout_draft() -> String {
    draft(&traffic_light(&green_separate(), "red").replace(
        "<onentry><send target=\"#_parent\" event=\"light.green\"/>",
        "<onentry><send target=\"#_parent\" event=\"error.stale\"/><send target=\"#_parent\" event=\"builder.timeout\"/><send target=\"#_parent\" event=\"light.green\"/>",
    ))
}

/// A valid draft whose green state raises `light.again` on entry and takes
/// it back into itself: its start never ends, so the candidate's start
/// stops at scxml.rs's per-call command bound (decision D122).
fn start_runaway_draft() -> String {
    draft(&traffic_light(
        &format!(
            "{}\n    <transition event=\"light.again\" target=\"green\"/>",
            green_separate()
        ),
        "red",
    )
    .replace(
        "<onentry><send target=\"#_parent\" event=\"light.green\"/>",
        "<onentry><raise event=\"light.again\"/><send target=\"#_parent\" event=\"light.green\"/>",
    ))
}

/// A valid draft whose button leads into `spin`, which raises `spin.again`
/// on entry and takes it back into itself: the call that delivers the
/// button stops at scxml.rs's per-call command bound (decision D122).
fn later_runaway_draft() -> String {
    draft(
        &traffic_light(&green_separate(), "red")
            .replace(
                r#"target="yellow"/>
    <transition event="light.timer" target="yellow"/>"#,
                r#"target="spin"/>
    <transition event="light.timer" target="yellow"/>"#,
            )
            .replace(
                "  </state>\n</scxml>",
                "  </state>\n  <state id=\"spin\">\n    <onentry><raise event=\"spin.again\"/></onentry>\n    <transition event=\"spin.again\" target=\"spin\"/>\n  </state>\n</scxml>",
            ),
    )
}

/// [`scripted_draft`] with an eventless transition out of green guarded by
/// `mayGo()`, whose script throws: the script is loaded and declares the
/// function, so the draft passes D123's pre-checks and is simulated, and
/// the cond fails on every evaluation (the 2026-10-20 live run 1's
/// `PingPongReply`, whose cond threw because its script was not loaded).
fn throwing_cond_draft() -> String {
    let mut draft: Json = serde_json::from_str(&scripted_draft()).expect("the draft is JSON");
    let scxml = draft["scxml"].as_str().expect("scxml is text").replacen(
        r#"<transition event="light.timer" target="yellow"/>"#,
        r#"<transition event="light.timer" target="yellow"/>
    <transition cond="mayGo()" target="yellow"/>"#,
        1,
    );
    draft["scxml"] = scxml.into();
    draft["script"] = "function mayGo() {\n  throw new Error('not yet');\n}\n".into();
    draft.to_string()
}

/// `/throwing/v1`'s `authors`th draft: [`throwing_cond_draft`], then
/// [`scripted_draft`].
fn throwing_route_draft(authors: usize) -> String {
    if authors == 1 {
        throwing_cond_draft()
    } else {
        scripted_draft()
    }
}

/// A draft whose timer transition `into` (yellow's or red's) carries a
/// cond with an unescaped `&&` (`XML001`): the light's transition on
/// `light.timer` into `into`, guarded by `true && true`.
fn ampersand_draft(into: &str) -> String {
    draft(&traffic_light(&green_separate(), "red").replace(
        &format!(r#"<transition event="light.timer" target="{into}"/>"#),
        &format!(r#"<transition event="light.timer" cond="true && true" target="{into}"/>"#),
    ))
}

/// [`scripted_draft`] without its root `<script src>`, as the 2026-10-20
/// live `PingPongReply` draft was (`script-not-loaded`), or without its
/// script (`undeclared-function`).
fn unloaded_script_draft(script: bool) -> String {
    let mut draft: Json = serde_json::from_str(&scripted_draft()).expect("the draft is JSON");
    if script {
        let scxml = draft["scxml"]
            .as_str()
            .expect("scxml is text")
            .replace(r#"<script src="traffic_light.js"/>"#, "");
        draft["scxml"] = scxml.into();
    } else {
        draft["script"] = "".into();
    }
    draft.to_string()
}

/// A draft that finishes at once, with one test that expects it to: its
/// `final` is inside `root`, which it only completes (the 2026-10-20 live
/// `PingPongReply` drafts), or `root`'s `done.state.root` leads to a
/// top-level `final`.
fn nested_final_draft(top_level: bool) -> String {
    let (exit, end) = if top_level {
        (
            r#"<transition event="done.state.root" target="out"/>"#,
            r#"<final id="out"/>"#,
        )
    } else {
        ("", "")
    };
    json!({
        "scxml": format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" name="finisher" initial="root">
  <state id="root" initial="go">
    {exit}
    <state id="go"><transition target="end"/></state>
    <final id="end"/>
  </state>
  {end}
</scxml>"#
        ),
        "schemas": schemas(&[]),
        "script": "",
        "tests": [{"name": "finishes", "events": [], "settle_ms": 1000, "expect_events": [], "expect_done": true}],
    })
    .to_string()
}

/// A draft whose `scxml` holds no document, as a model's stub call does.
fn placeholder_draft() -> String {
    draft("placeholder")
}

/// A draft that finishes on its second `ping`, with two tests that each
/// send one `ping` and expect it not to finish: a test whose `ping` is sent
/// twice fails.
fn two_ping_draft() -> String {
    json!({
        "scxml": r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event" version="1.0" datamodel="ecmascript" name="two_pings" initial="first">
  <state id="first">
    <onentry><send target="#_parent" event="ready"/></onentry>
    <transition event="ping" event:schema="traffic_light.json#/$defs/ping" target="second"/>
  </state>
  <state id="second">
    <transition event="ping" event:schema="traffic_light.json#/$defs/ping" target="end"/>
  </state>
  <final id="end"/>
</scxml>"##,
        "schemas": schemas(&["ping"]),
        "script": "",
        "tests": [
            {"name": "first run", "events": [{"name": "ping", "data": {}, "delay_ms": 0}],
             "settle_ms": 1000, "expect_events": ["ready"], "expect_done": false},
            {"name": "second run", "events": [{"name": "ping", "data": {}, "delay_ms": 0}],
             "settle_ms": 1000, "expect_events": ["ready"], "expect_done": false},
        ],
    })
    .to_string()
}

/// A persistent routine (it sends `bot.behavior.tick`) whose schedule is
/// `hourly.every`, armed with `delay` on entry of `waiting` and taken back
/// into it with a tick whose output is `output`, and one test that drives
/// nothing and expects no output (the builder forces the first tick).
fn routine_draft(delay: &str, output: &str) -> String {
    json!({
        "scxml": format!(
            r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" name="Hourly" initial="waiting">
  <state id="waiting">
    <onentry><send id="every" event="hourly.every" delay="{delay}"/></onentry>
    <onexit><cancel sendid="every"/></onexit>
    <transition event="hourly.every" target="waiting">
      <send event="bot.behavior.tick" target="#_parent">
        <content expr="({{name: 'Hourly', trigger: 'every', scheduled_at: new Date().toISOString(), fired_at: new Date().toISOString(), output: {output}}})"/>
      </send>
    </transition>
  </state>
</scxml>"##
        ),
        "schemas": schemas(&[]),
        "script": "",
        "tests": [{"name": "waits", "events": [], "settle_ms": 1000, "expect_events": [], "expect_done": false}],
    })
    .to_string()
}

/// A persistent routine that does its own I/O on each schedule (decision
/// D138): its tick, then a `BasicHTTP` POST of `hourly.report` to a hook,
/// whose refusal (no origin admitted in the simulation) ends the rest of
/// that block and comes back as `error.communication`.
fn http_routine_draft() -> String {
    json!({
        "scxml": format!(
            r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" name="Hourly" initial="waiting">
  <state id="waiting">
    <onentry><send id="every" event="hourly.every" delay="3600s"/></onentry>
    <onexit><cancel sendid="every"/></onexit>
    <transition event="hourly.every" target="waiting">
      <send event="bot.behavior.tick" target="#_parent">
        <content expr="({{name: 'Hourly', trigger: 'every', scheduled_at: new Date().toISOString(), fired_at: new Date().toISOString(), output: {CHIME}}})"/>
      </send>
      <send event="hourly.report" type="http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor" target="https://hooks.example/hourly"/>
    </transition>
    <transition event="error.communication"/>
  </state>
</scxml>"##
        ),
        "schemas": schemas(&[]),
        "script": "",
        "tests": [{"name": "waits", "events": [], "settle_ms": 1000, "expect_events": [], "expect_done": false}],
    })
    .to_string()
}

const CHIME: &str = "[{event: 'bot.speaking.say', data: {parts: [{words: 'Hourly chime.'}]}}]";

/// The request with the lifetime bounds (`min_every_ms`,
/// `turn_after_bound_ms`), as reflection sends it.
fn lifetime_request() -> Json {
    json!({"spec": "chime every hour", "min_every_ms": 60000, "turn_after_bound_ms": 1000})
}

/// The request with reflection's live turn (`apply`, decision D131): turn 3,
/// the behavior input autonomy builds for a knock at the door, and the one
/// session that turn's output names.
fn live_request() -> Json {
    json!({
        "spec": "answer a knock",
        "name": "Greet",
        "turn_after_bound_ms": 1000,
        "apply": {
            "turn": 3,
            "input": {"label": "knock", "focus": "door", "focus_candidates": ["door"]},
            "targets": ["communication"],
        },
    })
}

/// A selection that answers the knock: it reads `label` from the param
/// `input` (none without it), so it agrees with the live turn only when the
/// run was given that turn's input.
const RESPOND: &str = "{event: 'bot.ability.communication.respond', target: 'communication', data: {parts: [{words: 'Who is there?'}], label: input === null ? 'none' : input.label}}";

/// A turn behavior that declares the params `turn` and `input`, holds
/// `deciding` in its state `deciding`, and ends in `decided` with
/// `donedata`; its one test drives nothing and expects `expect_done`.
fn turn_draft(deciding: &str, donedata: &str, expect_done: bool) -> String {
    json!({
        "scxml": format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" name="Greet" initial="deciding">
  <datamodel>
    <data id="turn" expr="0"/>
    <data id="input" expr="null"/>
  </datamodel>
  <state id="deciding">
    {deciding}
  </state>
  <final id="decided">
    <donedata><content expr="{donedata}"/></donedata>
  </final>
</scxml>"#
        ),
        "schemas": {"$defs": {}},
        "script": "",
        "tests": [{"name": "decides", "events": [], "settle_ms": 1000, "expect_events": [], "expect_done": expect_done}],
    })
    .to_string()
}

/// A turn behavior that decides at once with `donedata`.
fn deciding_draft(donedata: &str) -> String {
    turn_draft(r#"<transition target="decided"/>"#, donedata, true)
}

/// The draft that selects [`RESPOND`] on the live turn.
fn live_ok_draft() -> String {
    deciding_draft(&format!(
        "({{turn: turn, handled: true, selected: [{RESPOND}]}})"
    ))
}

/// The draft whose selection hardcodes the label `doorbell`.
fn live_binding_draft() -> String {
    deciding_draft(&format!(
        "({{turn: turn, handled: true, selected: [{}]}})",
        RESPOND.replace(
            "label: input === null ? 'none' : input.label",
            "label: 'doorbell'"
        )
    ))
}

/// The `authors`th draft of a `/live*/v1` route (see [`Mock`]).
fn live_route_draft(path: &str, authors: usize) -> String {
    let route = path
        .strip_prefix('/')
        .and_then(|path| path.split('/').next())
        .unwrap_or_default();
    match route {
        "liveok" => live_ok_draft(),
        "livestuck" => turn_draft(
            r#"<transition event="never" target="decided"/>"#,
            "({turn: turn, handled: true, selected: []})",
            false,
        ),
        // Runs away only with the param input (decision D122 ends the
        // call); invoked with no param it decides at once.
        "liverunaway" => turn_draft(
            r#"<onentry><if cond="input !== null"><raise event="spin"/></if></onentry>
    <transition event="spin" target="deciding"/>
    <transition cond="input === null" target="decided"/>"#,
            "({turn: turn, handled: false, selected: []})",
            true,
        ),
        "liveshape" => deciding_draft("({turn: turn, selected: []})"),
        "liveturn" => deciding_draft(&format!(
            "({{turn: 0, handled: true, selected: [{RESPOND}]}})"
        )),
        "liveunhandled" => deciding_draft("({turn: turn, handled: false, selected: []})"),
        "livenoevent" => {
            deciding_draft("({turn: turn, handled: true, selected: [{target: 'communication'}]})")
        }
        "livenotarget" => deciding_draft(&format!(
            "({{turn: turn, handled: true, selected: [{}]}})",
            RESPOND.replace(" target: 'communication',", "")
        )),
        "liveforeign" => deciding_draft(&format!(
            "({{turn: turn, handled: true, selected: [{}]}})",
            RESPOND.replace("target: 'communication'", "target: 'speaking'")
        )),
        "liverepair" if authors > 1 => live_ok_draft(),
        _ => live_binding_draft(),
    }
}

/// The mock's answer on a `/live*/v1` route: the plan, then the
/// `authors`th draft.
fn live_reply(path: &str, schema: Option<&str>, authors: usize) -> Option<String> {
    match schema {
        Some("topology_plan") => Some(completion(PLAN)),
        Some("scxml_draft") => Some(completion(&live_route_draft(path, authors))),
        _ => None,
    }
}

/// A non-streamed Chat Completions answer carrying `content`, as the
/// generate service asks for one by default.
fn completion(content: &str) -> String {
    json!({
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "finish_reason": "stop",
            "message": {"role": "assistant", "content": content},
        }],
    })
    .to_string()
}

/// A reply cut off at the request's token bound (`finish_reason` `length`),
/// which the generate service fails as `truncated`.
fn truncated_completion() -> String {
    json!({
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "finish_reason": "length",
            "message": {"role": "assistant", "content": "{\"scxml\": \"<scxml"},
        }],
    })
    .to_string()
}

/// A `system_one` answer choosing `chosen` in the selection (0.8, the rest
/// share 0.2) and every other question's first label.
fn system_one(chosen: &str, body: &[u8]) -> Option<String> {
    let request: Json = serde_json::from_slice(body).ok()?;
    let questions = request.get("questions")?.as_object()?;
    let mut answers = Map::new();
    for (name, question) in questions {
        let labels: Vec<&String> = question.get("criteria")?.as_object()?.keys().collect();
        let mut probabilities = Map::new();
        let choice = if name == "selection" {
            let rest = labels
                .iter()
                .filter(|label| label.as_str() != chosen)
                .count();
            for label in &labels {
                let share = if label.as_str() == chosen {
                    0.8
                } else {
                    0.2 / f64::from(u32::try_from(rest.max(1)).ok()?)
                };
                probabilities.insert((*label).clone(), share.into());
            }
            chosen.to_owned()
        } else {
            let first = (*labels.first()?).clone();
            probabilities.insert(first.clone(), 1.0.into());
            first
        };
        answers.insert(
            name.clone(),
            json!({"type": "choice", "choice": choice, "confidence": 0.8, "probabilities": probabilities}),
        );
    }
    Some(json!({"model": "jev-latest", "answers": answers}).to_string())
}

/// The `authors`th draft of the routes that prove which failures repeat
/// (`/rendered/v1`, `/moved/v1`, `/again/v1`, `/changed/v1`; see [`Mock`]).
fn diagnostics_route_draft(path: &str, authors: usize) -> String {
    match (path, authors) {
        ("/rendered/v1/chat/completions" | "/moved/v1/chat/completions", 1) => {
            ampersand_draft("green")
        }
        ("/rendered/v1/chat/completions", 2) => placeholder_draft(),
        ("/rendered/v1/chat/completions", _) => draft(&traffic_light(&green_separate(), "red")),
        ("/moved/v1/chat/completions", _) => ampersand_draft("red"),
        ("/again/v1/chat/completions", 2) => internal_delay_draft(),
        ("/changed/v1/chat/completions", 1) => invalid_internal_delay_draft(),
        ("/changed/v1/chat/completions", _) => invalid_two_delays_draft(),
        _ => invalid_draft(),
    }
}

/// The routes whose every plan request is answered with [`PLAN`].
fn answers_plan(path: &str) -> bool {
    matches!(
        path,
        "/repair/v1/chat/completions"
            | "/truncated/v1/chat/completions"
            | "/truncatedplan/v1/chat/completions"
            | "/broken/v1/chat/completions"
            | "/service/v1/chat/completions"
            | "/impostor/v1/chat/completions"
            | "/stale/v1/chat/completions"
            | "/pings/v1/chat/completions"
            | "/progress/v1/chat/completions"
            | "/lifetime/v1/chat/completions"
            | "/emptytick/v1/chat/completions"
            | "/httproutine/v1/chat/completions"
            | "/selfschedule/v1/chat/completions"
            | "/afterbound/v1/chat/completions"
            | "/runaway/v1/chat/completions"
            | "/rendered/v1/chat/completions"
            | "/moved/v1/chat/completions"
            | "/again/v1/chat/completions"
            | "/changed/v1/chat/completions"
            | "/unloaded/v1/chat/completions"
            | "/throwing/v1/chat/completions"
            | "/nested/v1/chat/completions"
    )
}

/// Whether the request is one a reasoning model cut off at its token
/// bound: the first two drafts on `/truncated/v1` and the first two plans
/// on `/truncatedplan/v1`.
fn truncates(path: &str, schema: Option<&str>, authors: usize, plans: usize) -> bool {
    match (path, schema) {
        ("/truncated/v1/chat/completions", Some("scxml_draft")) => authors <= 2,
        ("/truncatedplan/v1/chat/completions", Some("topology_plan")) => plans <= 2,
        _ => false,
    }
}

/// The mock's answer to one request on `path` requiring the function of
/// `schema`, after `authors` draft and `plans` plan requests on that path.
fn mock_reply(
    path: &str,
    schema: Option<&str>,
    authors: usize,
    plans: usize,
    body: &[u8],
) -> Option<String> {
    if truncates(path, schema, authors, plans) {
        return Some(truncated_completion());
    }
    match (path, schema) {
        (_, Some("topology_plan")) if answers_plan(path) => Some(completion(PLAN)),
        ("/flaky/v1/chat/completions", Some("topology_plan")) => {
            (plans != 2).then(|| completion(PLAN))
        }
        ("/fastfail/v1/chat/completions", Some("topology_plan")) => {
            (plans != 1).then(|| completion(PLAN))
        }
        // The first plan is held (see `Mock::start`) and answered late.
        ("/slow/v1/chat/completions", Some("topology_plan")) => {
            Some(completion(if plans == 1 { LATE_PLAN } else { PLAN }))
        }
        (
            "/slow/v1/chat/completions"
            | "/afterbound/v1/chat/completions"
            | "/truncated/v1/chat/completions"
            | "/truncatedplan/v1/chat/completions",
            Some("scxml_draft"),
        ) => Some(completion(&draft(&traffic_light(&green_separate(), "red")))),
        ("/service/v1/chat/completions", Some("scxml_draft")) => Some(completion(&service_draft())),
        // The draft's own generation: text, without a schema.
        ("/service/v1/chat/completions", None) => Some(completion("hello")),
        ("/impostor/v1/chat/completions", Some("scxml_draft")) => {
            Some(completion(&impostor_draft()))
        }
        ("/stale/v1/chat/completions", Some("scxml_draft")) => Some(completion(&match authors {
            1 => stale_timeout_draft(),
            _ => draft(&traffic_light(&green_separate(), "red")),
        })),
        ("/pings/v1/chat/completions", Some("scxml_draft")) => Some(completion(&two_ping_draft())),
        ("/lifetime/v1/chat/completions", Some("scxml_draft")) => {
            Some(completion(&match authors {
                1 => routine_draft("30s", CHIME),
                _ => routine_draft("3600s", CHIME),
            }))
        }
        ("/selfschedule/v1/chat/completions", Some("scxml_draft")) => {
            Some(completion(&match authors {
                1 => routine_draft("3600s", CHIME).replace(
                    r#"event=\"hourly.every\" delay"#,
                    r##"event=\"hourly.every\" target=\"#_self\" delay"##,
                ),
                _ => routine_draft("3600s", CHIME),
            }))
        }
        ("/httproutine/v1/chat/completions", Some("scxml_draft")) => {
            Some(completion(&http_routine_draft()))
        }
        ("/emptytick/v1/chat/completions", Some("scxml_draft")) => {
            Some(completion(&routine_draft("3600s", "[]")))
        }
        ("/runaway/v1/chat/completions", Some("scxml_draft")) => Some(completion(&match authors {
            1 => start_runaway_draft(),
            2 => later_runaway_draft(),
            _ => draft(&traffic_light(&green_separate(), "red")),
        })),
        (
            "/rendered/v1/chat/completions"
            | "/moved/v1/chat/completions"
            | "/again/v1/chat/completions"
            | "/changed/v1/chat/completions",
            Some("scxml_draft"),
        ) => Some(completion(&diagnostics_route_draft(path, authors))),
        ("/unloaded/v1/chat/completions", Some("scxml_draft")) => {
            Some(completion(&match authors {
                1 => unloaded_script_draft(true),
                2 => unloaded_script_draft(false),
                _ => scripted_draft(),
            }))
        }
        ("/throwing/v1/chat/completions", Some("scxml_draft")) => {
            Some(completion(&throwing_route_draft(authors)))
        }
        ("/nested/v1/chat/completions", Some("scxml_draft")) => {
            Some(completion(&nested_final_draft(authors > 1)))
        }
        ("/repair/v1/chat/completions", Some("scxml_draft")) => Some(completion(&match authors {
            1 => invalid_draft(),
            2 => fixable_draft(),
            _ => draft(&traffic_light(&green_separate(), "red")),
        })),
        ("/progress/v1/chat/completions", Some("scxml_draft")) => {
            Some(completion(&match authors {
                1 => invalid_draft(),
                2 => internal_delay_draft(),
                _ => scripted_draft(),
            }))
        }
        (
            "/broken/v1/chat/completions"
            | "/flaky/v1/chat/completions"
            | "/fastfail/v1/chat/completions",
            Some("scxml_draft"),
        ) => Some(completion(&invalid_draft())),
        ("/typesafe/v1/systemone", _) => system_one("triage.repair", body),
        ("/redesign/v1/systemone", _) => system_one("triage.redesign", body),
        _ => None,
    }
}

/// One request the mock received.
#[derive(Clone, Debug)]
struct Received {
    path: String,
    authorization: Option<String>,
    body: Json,
    /// When it arrived.
    at: Instant,
    /// When its answer was written.
    answered: Arc<Mutex<Option<Instant>>>,
}

impl Received {
    /// The schema name of a Chat Completions request: the one tool it requires a call of.
    fn schema_name(&self) -> Option<&str> {
        self.body
            .pointer("/tools/0/function/name")
            .and_then(Json::as_str)
    }

    /// The user message of a Chat Completions request, read back (the
    /// builder sends its request as model-facing XML, decision D201).
    fn user_input(&self) -> Json {
        let text = self
            .body
            .get("messages")
            .and_then(Json::as_array)
            .and_then(|messages| {
                messages
                    .iter()
                    .find(|message| message.get("role").and_then(Json::as_str) == Some("user"))
            })
            .and_then(|message| message.get("content"))
            .and_then(Json::as_str)
            .expect("a user message");
        read(text)
    }
}

/// The deterministic fake backend on 127.0.0.1. `generate` routes:
/// `/repair/v1` answers the plan, then the invalid draft, then the fixable
/// one, then valid drafts; `/progress/v1` answers the plan, the invalid
/// draft, a draft with a delayed internal send, then valid drafts;
/// (`/progress/v1`'s valid one with a script, [`scripted_draft`]);
/// `/runaway/v1` answers the plan, a draft whose start runs away, a draft
/// whose button runs it away, then valid drafts; `/rendered/v1` answers the
/// plan, a draft with an unescaped `&&` in red's cond, one whose `scxml`
/// is a placeholder, then valid drafts; `/moved/v1` answers the plan, the
/// `&&` in red's cond, then in yellow's; `/again/v1` answers the plan, the
/// invalid draft, one with a delayed internal send, then the invalid draft
/// again; `/changed/v1` answers the plan, the invalid draft with a delayed
/// internal send, then the invalid draft with a send with both `delay` and
/// `delayexpr`;
/// `/unloaded/v1` answers the plan, a scripted draft without its root
/// `<script src>`, one without its script, then the scripted draft;
/// `/throwing/v1` answers the plan, a scripted draft whose eventless
/// cond throws, then the scripted draft;
/// `/nested/v1` answers the plan, a draft whose only final is inside a
/// state, then one that also has a top-level final;
/// `/broken/v1` answers the plan, then only invalid
/// drafts; `/service/v1` answers the plan, then a draft that invokes
/// `generate`, and that draft's own generation with text; `/impostor/v1` answers the plan, then a valid draft
/// that sends the builder's control event names; `/flaky/v1` fails the
/// second plan (404) and otherwise answers like `/broken/v1`; `/fastfail/v1`
/// fails the first plan (404) at once and otherwise answers like
/// `/broken/v1`; `/noplan/v1`
/// fails every plan (404); `/slow/v1` holds its first plan until its
/// second plan request arrives (at most [`SLOW_PLAN_HOLD_BOUND`]) and
/// answers [`LATE_PLAN`], then answers the plan and
/// valid drafts. The `/live*/v1` routes answer the plan, then turn
/// behaviors for a request with a live turn ([`live_route_draft`]):
/// `/liveok/v1` one that selects on it, `/livestuck/v1` one that never
/// finishes, `/liverunaway/v1` one that runs away with the param input,
/// `/liveshape/v1`, `/liveturn/v1`, `/liveunhandled/v1`, `/livenoevent/v1`,
/// `/livenotarget/v1` and `/liveforeign/v1` one whose donedata breaks the
/// contract that way, `/livebinding/v1` one that hardcodes an input value,
/// and `/liverepair/v1` that one first, then the one that selects on it.
/// Each connection is served on its own thread, so a held
/// answer delays nothing else. `decision` routes:
/// `/typesafe` chooses `triage.repair`, `/redesign` `triage.redesign`.
struct Mock {
    origin: String,
    /// Every request so far, and the signal that one more arrived.
    received: Arc<(Mutex<Vec<Received>>, Condvar)>,
}

impl Mock {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock binds a loopback port");
        let origin = format!(
            "http://{}",
            listener.local_addr().expect("the mock has an address")
        );
        let received = Arc::new((Mutex::new(Vec::<Received>::new()), Condvar::new()));
        let log = Arc::clone(&received);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let log = Arc::clone(&log);
                std::thread::spawn(move || serve(stream, &log.0, &log.1));
            }
        });
        Self { origin, received }
    }

    fn received(&self) -> Vec<Received> {
        self.received.0.lock().expect("the log lock").clone()
    }
}

/// Serves one connection of the mock: reads one request, records it (and
/// signals `arrived`), answers it (holding the slow route's first plan until
/// its second plan request arrives), and records when.
fn serve(mut stream: std::net::TcpStream, log: &Mutex<Vec<Received>>, arrived: &Condvar) {
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(clone);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.trim_end().split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let path = request_line.split(' ').nth(1).unwrap_or("").to_owned();
    let parsed: Json = serde_json::from_slice(&body).unwrap_or(Json::Null);
    let request = Received {
        path: path.clone(),
        authorization: headers.get("authorization").cloned(),
        body: parsed,
        at: Instant::now(),
        answered: Arc::new(Mutex::new(None)),
    };
    let count = |log: &[Received], schema: &str| {
        log.iter()
            .filter(|seen| seen.path == path && seen.schema_name() == Some(schema))
            .count()
    };
    let mut started = false;
    let (authors, plans) = {
        let mut log = log.lock().expect("the log lock");
        log.push(request.clone());
        arrived.notify_all();
        let counts = (count(&log, "scxml_draft"), count(&log, "topology_plan"));
        if path == "/slow/v1/chat/completions"
            && request.schema_name() == Some("topology_plan")
            && counts.1 == 1
        {
            // A streamed first plan starts (one event, the generation's
            // first progress) and then stalls.
            if request.body["stream"] == true {
                started = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{STALLED_EVENT}"
                )
                .and_then(|()| stream.flush())
                .is_ok();
            }
            // Hold the first plan until the builder asks for the redesign:
            // its timeout has preempted this generation by then.
            drop(
                arrived
                    .wait_timeout_while(log, SLOW_PLAN_HOLD_BOUND, |log| {
                        count(log, "topology_plan") < 2
                    })
                    .expect("the log lock"),
            );
        }
        counts
    };
    let reply = if path.starts_with("/live") {
        live_reply(&path, request.schema_name(), authors)
    } else {
        mock_reply(&path, request.schema_name(), authors, plans, &body)
    }
    .map(|reply| support::answer_as_asked(&request.body, reply));
    let (status, reply) = reply.map_or(("404 Not Found", "{}".to_owned()), |reply| {
        ("200 OK", reply)
    });
    let _ = if started {
        write!(stream, "{reply}")
    } else {
        write!(
            stream,
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
            reply.len()
        )
    };
    *request.answered.lock().expect("the answered lock") = Some(Instant::now());
}

/// Writes an env file holding the fake keys and base URLs of `env` (the
/// builder's document names all four; a variable the file does not set
/// holds no value, so its slot is absent).
fn env_file(
    test: &str,
    mock: &Mock,
    generate_route: &str,
    decision_route: &str,
    env: &[&str],
) -> PathBuf {
    let origin = &mock.origin;
    let values = [
        ("FLOPPY_API_KEY", FAKE_FLOPPY_KEY.to_owned()),
        ("FLOPPY_BASE_URL", format!("{origin}/{generate_route}/v1")),
        ("TYPESAFE_API_KEY", FAKE_TYPESAFE_KEY.to_owned()),
        ("TYPESAFE_BASE_URL", format!("{origin}/{decision_route}")),
    ];
    let mut contents = String::new();
    for (name, value) in values.iter().filter(|(name, _)| env.contains(name)) {
        contents.push_str(name);
        contents.push('=');
        contents.push_str(value);
        contents.push('\n');
    }
    let path = scratch("builder-env").join(format!("{test}.env"));
    std::fs::write(&path, contents).expect("the env file is writable");
    path
}

const ALL: [&str; 15] = support::GRANTED;

fn run(mock: &Mock, test: &str, route: &str, env: &[&str], request: &Json) -> Vec<Json> {
    run_deciding(mock, test, route, "typesafe", env, request)
}

/// [`run`] with `builder` as the text of the builder document, laid out
/// as [`builder_layout`].
fn run_builder(mock: &Mock, test: &str, route: &str, builder: &str, request: &Json) -> Vec<Json> {
    let document = builder_layout(builder);
    let env = env_file(test, mock, route, "typesafe", &ALL);
    start(&document, "logical", &[env], request, &[])
        .finish(Duration::from_mins(5))
        .records
}

/// [`run`] with the decision backend on `decision_route`.
fn run_deciding(
    mock: &Mock,
    test: &str,
    route: &str,
    decision_route: &str,
    env: &[&str],
    request: &Json,
) -> Vec<Json> {
    run_deciding_in(
        &builder_document(),
        mock,
        test,
        route,
        decision_route,
        env,
        request,
    )
}

/// [`run_deciding`] of the builder `document` (a [`builder_layout`]).
fn run_deciding_in(
    document: &Path,
    mock: &Mock,
    test: &str,
    route: &str,
    decision_route: &str,
    env: &[&str],
    request: &Json,
) -> Vec<Json> {
    let env = env_file(test, mock, route, decision_route, env);
    let finished = start(document, "logical", &[env], request, &[]).finish(Duration::from_mins(5));
    assert!(
        !finished.records.is_empty(),
        "the session ran: {}",
        finished.stderr
    );
    finished.records
}

/// `request` with the failure cap `max_attempts` (decision D133).
fn capped(mut request: Json, max_attempts: u64) -> Json {
    request["max_attempts"] = json!(max_attempts);
    request
}

fn chat_requests<'a>(received: &'a [Received], schema: &str) -> Vec<&'a Received> {
    received
        .iter()
        .filter(|request| request.schema_name() == Some(schema))
        .collect()
}

// ---------------------------------------------------------------------------
// The document itself.

#[test]
fn builder_validates_clean_with_its_repository_packages() {
    let validated = validate(&builder_dir().join("builder.scx"), &[]);
    assert!(validated.status.success(), "{}", validated.stderr);
    assert!(
        validated
            .stderr
            .contains("diagnostic summary: 0 error(s), 0 warning(s), 0 note(s)"),
        "builder.scx validates without an error, a warning, or a note: {}",
        validated.stderr
    );
}

/// The W3C SCXML 1.0 schema modules the author is given (decision D134), in
/// the order `scxml.xsd` includes them, from the scxml.rs subtree's copy.
const SCXML_XSD_MODULES: [&str; 7] = [
    "scxml.xsd",
    "scxml-module-core.xsd",
    "scxml-module-data.xsd",
    "scxml-module-external.xsd",
    "scxml-contentmodels.xsd",
    "scxml-attribs.xsd",
    "scxml-datatypes.xsd",
];

/// `text` without each span from `open` to the next `close` (both removed).
fn without_spans(text: &str, open: &str, close: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        kept.push_str(&rest[..start]);
        let after = &rest[start + open.len()..];
        let end = after.find(close).expect("every span is closed");
        rest = &after[end + close.len()..];
    }
    kept.push_str(rest);
    kept
}

/// The schema modules as the author reads them: each module headed by its
/// file name, without its XML declaration, comments, annotations, blank
/// lines and indentation.
fn author_xsd() -> String {
    let dir = workspace().join("scxml.rs/crates/scxml-validator-xsd/schemas/w3c-scxml-1.0");
    SCXML_XSD_MODULES
        .iter()
        .map(|module| {
            let text = std::fs::read_to_string(dir.join(module)).expect("the schema module reads");
            let text = without_spans(&text, "<?xml", "?>");
            let text = without_spans(&text, "<!--", "-->");
            let text = without_spans(&text, "<xsd:annotation>", "</xsd:annotation>");
            let lines: Vec<&str> = text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect();
            format!("{module}:\n{}", lines.join("\n"))
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// `bots/builder/xsd/scxml-1.0.json`, the `<data src>` of `scxml_xsd`, is the
/// subtree's SCXML schema as one JSON string (decision D134);
/// `FLOPPY_XSD_BLESS=1` rewrites it after a subtree pull.
#[test]
fn the_authors_xsd_is_the_subtrees_scxml_schema() {
    let path = builder_dir().join("xsd/scxml-1.0.json");
    let expected = format!("{}\n", Json::String(author_xsd()));
    if std::env::var_os("FLOPPY_XSD_BLESS").is_some() {
        std::fs::write(&path, &expected).expect("the xsd writes");
    }
    let actual = std::fs::read_to_string(&path).expect("bots/builder/xsd/scxml-1.0.json reads");
    assert!(
        actual == expected,
        "bots/builder/xsd/scxml-1.0.json differs from the subtree's schema: rerun with FLOPPY_XSD_BLESS=1"
    );
    assert!(
        builder_source().contains(r#"<data id="scxml_xsd" src="xsd/scxml-1.0.json"/>"#),
        "builder.scx loads it"
    );
}

/// Every `event:schema` of builder.scx references `schemas/<event>.json`
/// (an event taken in two states references the same file in both) or, on
/// a final state's `<donedata>`, `schemas/<final state id>.json`, and every
/// schema file is referenced.
#[test]
fn builder_event_schemas_reference_the_schema_files() {
    let source = builder_source();
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        scxml_syntax::SourceId::new(1),
        "builder.scx",
        source.as_str(),
    ))
    .expect("builder.scx is XML");
    let schema_of = |element: &scxml_syntax::Element| {
        element
            .attributes
            .iter()
            .find(|attribute| {
                attribute.name.namespace_uri.as_deref()
                    == Some(scxml_event_schema::EVENT_NAMESPACE_URI)
                    && attribute.name.local_name == scxml_event_schema::SCHEMA_ATTRIBUTE
            })
            .map(|attribute| attribute.value.clone())
    };
    let mut referenced = BTreeSet::new();
    document.root.visit(&mut |element| {
        let event = element
            .attributes
            .iter()
            .find(|attribute| {
                attribute.name.namespace_uri.is_none() && attribute.name.local_name == "event"
            })
            .map(|attribute| attribute.value.clone());
        if let (Some(schema), Some(event)) = (schema_of(element), event) {
            assert_eq!(schema, format!("schemas/{event}.json"), "{event}");
            referenced.insert(event);
        }
        if element.name.local_name == "final"
            && let Some(id) = element.attribute("id")
        {
            for child in &element.children {
                let scxml_syntax::Node::Element(donedata) = child else {
                    continue;
                };
                if donedata.name.local_name != "donedata" {
                    continue;
                }
                let schema =
                    schema_of(donedata).expect("each final state's donedata carries its schema");
                assert_eq!(schema, format!("schemas/{}.json", id.value), "{}", id.value);
                referenced.insert(id.value.clone());
            }
        }
    });
    let files: BTreeSet<String> = std::fs::read_dir(builder_dir().join("schemas"))
        .expect("schemas/ is listable")
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_str()
                .and_then(|name| name.strip_suffix(".json"))
                .expect("every schema file is <event>.json")
                .to_owned()
        })
        .collect();
    assert_eq!(referenced, files);
}

/// The bots live runs built (bots/builder/examples) still validate.
#[test]
fn the_saved_live_examples_validate() {
    for example in ["traffic_light", "traffic_light_luna"] {
        let validated = validate(
            &builder_dir().join(format!("examples/{example}/traffic_light.scx")),
            &[],
        );
        assert!(
            validated.status.success(),
            "{example}: {}",
            validated.stderr
        );
        assert!(
            validated.stderr.contains("0 error(s), 0 warning(s)"),
            "{example}: {}",
            validated.stderr
        );
    }
}

// ---------------------------------------------------------------------------
// The loop.

#[test]
fn the_builder_repairs_an_invalid_draft_fixes_it_simulates_it_and_reports_done() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "repair",
        "repair",
        &ALL,
        &json!({"spec": SPEC, "constraints": ["send every light change to the invoker"]}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");

    // The draft that came back validates on its own, and it is the
    // machine-fixed text of the second draft.
    let scxml = done["scxml"].as_str().expect("scxml is text");
    assert!(
        scxml.contains(r#"event="pedestrian.button light.timer""#),
        "{scxml}"
    );
    assert!(
        compiles("traffic_light", &done),
        "the built machine validates and compiles with its schema file"
    );
    assert_eq!(done["schemas"], schemas(&["pedestrian.button"]));
    assert_eq!(done["script"], "");
    assert_eq!(done["path"], "../../behaviors/traffic_light.scx");
    assert_eq!(done["tests"], tests());

    let report = &done["report"];
    assert_eq!(report["attempts"], 2, "{report}");
    assert_eq!(report["failures"], 1, "{report}");
    assert_eq!(report["fixes_applied"], 1, "{report}");
    assert_eq!(report["decisions"], json!(["triage.repair"]), "{report}");
    assert_eq!(report["plan"]["name"], "traffic_light");
    assert_eq!(report["history"][0]["diagnostics"][0]["code"], "SCXML005");
    let simulation = report["simulation"].as_array().expect("simulation results");
    assert_eq!(simulation.len(), 2, "{report}");
    assert!(
        simulation.iter().all(|result| result["passed"] == true),
        "{report}"
    );
    assert_eq!(
        simulation[0]["observed"],
        json!(["light.green", "light.yellow", "light.red", "walk.on"]),
        "the draft ran as an invoked child, driven through #_<invokeid>: {report}"
    );
    assert_eq!(
        simulation[1]["observed"],
        json!(["light.green"]),
        "{report}"
    );
    let first_id = simulation[0]["invokeid"].as_str().expect("an invoke id");
    let second_id = simulation[1]["invokeid"].as_str().expect("an invoke id");
    assert!(
        !first_id.is_empty() && first_id != second_id,
        "each test runs its own invocation, so nothing an earlier one queued counts: {report}"
    );

    assert_repair_requests(&mock.received());
}

/// The repair run's requests: one plan, two drafts (the second with the
/// first one's diagnostics, draft, and tests), one decision.
fn assert_repair_requests(received: &[Received]) {
    let designs = chat_requests(received, "topology_plan");
    let authors = chat_requests(received, "scxml_draft");
    assert_eq!(designs.len(), 1);
    assert_eq!(authors.len(), 2);
    // The author call (a whole document) has its own token bound (decision D131).
    assert_eq!(designs[0].body["max_completion_tokens"], 8192);
    assert!(
        authors
            .iter()
            .all(|request| request.body["max_completion_tokens"] == 16384)
    );
    assert_eq!(designs[0].user_input()["spec"], SPEC);
    assert_eq!(designs[0].user_input()["previous_plan"], Json::Null);
    let first = authors[0].user_input();
    assert_eq!(first["task"], "author");
    assert_eq!(first["plan"]["name"], "traffic_light");
    assert_eq!(
        first["name"], "traffic_light",
        "the draft is told the name its documents reference: the plan's"
    );
    let second = authors[1].user_input();
    assert_eq!(second["task"], "repair");
    assert_eq!(second["diagnostics"][0]["code"], "SCXML005");
    // The diagnostic carries what the schema declares for the element it
    // marks, the transition whose target is unknown (decision D137).
    assert!(
        second["diagnostics"][0]["schema"]
            .as_str()
            .is_some_and(|schema| schema
                .starts_with(r#"<xsd:attributeGroup name="scxml.transition.attlist">"#)
                && schema
                    .ends_with(r#"<xsd:element name="transition" type="scxml.transition.type"/>"#)),
        "{second}"
    );
    assert!(
        second["previous_scxml"]
            .as_str()
            .is_some_and(|text| text.contains(r#"target="amber""#)),
        "the repair sees the failed draft"
    );
    assert_eq!(
        second["previous_tests"],
        rendered("previous_tests", &tests()),
        "the repair sees the failed draft's tests"
    );
    assert_eq!(
        second["previous_schemas"],
        rendered("previous_schemas", &schemas(&["pedestrian.button"])),
        "the repair sees the failed draft's schemas"
    );
    assert_eq!(second["previous_script"], "");
    assert_eq!(first["previous_tests"], Json::Null);
    assert_eq!(first["previous_schemas"], Json::Null);
    assert_eq!(first["previous_script"], Json::Null);
    for request in designs.iter().chain(&authors) {
        assert_eq!(
            request.authorization.as_deref(),
            Some(format!("Bearer {FAKE_FLOPPY_KEY}").as_str())
        );
    }
    let decisions: Vec<&Received> = received
        .iter()
        .filter(|request| request.path == "/typesafe/v1/systemone")
        .collect();
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        decisions[0].authorization.as_deref(),
        Some(format!("Bearer {FAKE_TYPESAFE_KEY}").as_str())
    );
    let offered: Vec<&String> = decisions[0]
        .body
        .pointer("/questions/selection/criteria")
        .and_then(Json::as_object)
        .expect("the selection question")
        .keys()
        .collect();
    for event in ["triage.repair", "triage.redesign", "triage.abandon"] {
        assert!(
            offered.iter().any(|label| *label == event),
            "{event} in {offered:?}"
        );
    }
}

/// A `builder.timeout` fires while the design generation is in flight (the
/// slow route's streamed first plan starts and then stalls): it preempts
/// the call, leaving design cancels the invocation, and the late answer
/// reaches no one. The builder redesigns and finishes with the second plan.
/// Runs on the wall clock.
#[test]
fn a_builder_timeout_preempts_a_generation_in_flight_and_cancels_it() {
    let mock = Mock::start();
    let env = env_file("timeout", &mock, "slow", "redesign", &ALL);
    // Streamed replies, so the stall is bounded by inactivity (decision
    // D101): a whole-call ceiling would bound the redesign too, and a
    // loaded host can take longer than it to start and finish that call.
    let mut text = std::fs::read_to_string(&env).expect("the env file is readable");
    text.push_str("FLOPPY_STREAM=true\n");
    std::fs::write(&env, text).expect("the env file is writable");
    let mut args = builder_args(&builder_document(), "wall");
    args.extend(["--env-file".to_owned(), env.display().to_string()]);
    let mut run = Run::start(&args, &[]);
    let mut records = run.until(Duration::from_mins(2), |record| {
        record["type"] == "call" && record["cause"] == "start"
    });
    run.event(
        "builder.request",
        // The redesign streams whole at once, so only the stalled first
        // plan goes this long without progress.
        &json!({"spec": SPEC, "generate_timeout_ms": 5000}),
    );
    run.end_input();
    records.extend(run.finish(Duration::from_mins(3)).records);
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    let report = &done["report"];
    assert_eq!(
        report["history"][0]["diagnostics"][0]["code"], "design-timeout",
        "{report}"
    );
    assert_eq!(report["history"][0]["diagnostics"][0]["kind"], "timeout");
    // No plan yet, so the decision service is offered no repair: it
    // redesigns.
    assert_eq!(report["decisions"], json!(["triage.redesign"]), "{report}");
    assert_eq!(
        report["plan"]["name"], "traffic_light",
        "the cancelled generation's late plan never arrives: {report}"
    );
    let received = mock.received();
    let designs = chat_requests(&received, "topology_plan");
    assert_eq!(designs.len(), 2, "the timed-out design, then the redesign");
    let late = designs[0]
        .answered
        .lock()
        .expect("the answered lock")
        .expect("the held plan was answered");
    // The first plan is held until the redesign request arrives, so the
    // redesign was asked for while that generation was still in flight:
    // the timeout preempted it.
    assert!(
        designs[1].at <= late,
        "the builder moved on while the first generation was still in flight"
    );
}

/// An author generation cut off at its token bound doubles the bound for
/// the next author call, up to 65536 (decision D135): the request's 16384
/// becomes 32768 and then 65536, and the third draft is built.
#[test]
fn a_truncated_author_generation_doubles_the_next_token_bound() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "truncated",
        "truncated",
        &["FLOPPY_API_KEY", "FLOPPY_BASE_URL"],
        &json!({"spec": SPEC, "document_max_completion_tokens": 16384}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    let history = done["report"]["history"].as_array().expect("the history");
    assert_eq!(history.len(), 2, "{done}");
    assert!(
        history.iter().all(
            |failure| failure["diagnostics"][0]["code"] == "author-failed"
                && failure["diagnostics"][0]["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("(truncated)"))
        ),
        "{done}"
    );
    let received = mock.received();
    let bounds: Vec<&Json> = chat_requests(&received, "scxml_draft")
        .iter()
        .map(|request| &request.body["max_completion_tokens"])
        .collect();
    assert_eq!(bounds, [&json!(16384), &json!(32768), &json!(65536)]);
}

/// A design generation cut off at its token bound doubles the bound for
/// the next design call, up to 65536 (decision D223): 8192 becomes 16384
/// and then 32768, and the third plan is built.
#[test]
fn a_truncated_design_generation_doubles_the_next_token_bound() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "truncatedplan",
        "truncatedplan",
        &["FLOPPY_API_KEY", "FLOPPY_BASE_URL"],
        &json!({"spec": SPEC}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    let history = done["report"]["history"].as_array().expect("the history");
    assert_eq!(history.len(), 2, "{done}");
    assert!(
        history.iter().all(
            |failure| failure["diagnostics"][0]["code"] == "design-failed"
                && failure["diagnostics"][0]["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("(truncated)"))
        ),
        "{done}"
    );
    let received = mock.received();
    let bounds: Vec<&Json> = chat_requests(&received, "topology_plan")
        .iter()
        .map(|request| &request.body["max_completion_tokens"])
        .collect();
    assert_eq!(bounds, [&json!(8192), &json!(16384), &json!(32768)]);
}

/// Without the decision key the builder falls back to its cond-based
/// triage; a repair that fails with the same diagnostic as the draft before
/// it goes on (decision D133): the next author call carries that diagnostic
/// as `repeated`, and the build ends in `failed` (`attempts-exhausted`) at
/// the default cap of 8 failures, without a cap from the request.
#[test]
fn the_same_diagnostic_again_is_repeated_and_the_build_runs_to_its_cap() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "broken",
        "broken",
        &["FLOPPY_API_KEY", "FLOPPY_BASE_URL"],
        &json!({"spec": SPEC}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed", "{failed}");
    assert_eq!(failed["reason"], "attempts-exhausted");
    assert_eq!(failed["diagnostics"][0]["kind"], "validate");
    assert_eq!(failed["diagnostics"][0]["code"], "SCXML005");
    assert_eq!(
        failed["history"].as_array().map(Vec::len),
        Some(8),
        "{failed}"
    );
    // The last draft the validator rejected (the reflection stores the
    // rejected source as a draft).
    assert_eq!(
        failed["scxml"].as_str(),
        Some(traffic_light(&green_separate(), "amber").as_str()),
        "{failed}"
    );
    let received = mock.received();
    assert_eq!(chat_requests(&received, "topology_plan").len(), 1);
    let drafts = chat_requests(&received, "scxml_draft");
    assert_eq!(drafts.len(), 8);
    // Each author call's system prompt carries the SCXML schema (decision D134).
    let xsd = author_xsd();
    assert!(
        drafts.iter().all(|draft| draft.body["messages"]
            .as_array()
            .and_then(|messages| messages.iter().find(|message| message["role"] == "system"))
            .and_then(|message| message["content"].as_str())
            .is_some_and(|system| system.ends_with(&xsd))),
        "every author call is given the schema"
    );
    assert_eq!(drafts[0].user_input()["repeated"], Json::Null);
    assert_eq!(drafts[1].user_input()["repeated"], Json::Null);
    let repeated = drafts[2].user_input()["repeated"].clone();
    assert!(
        repeated
            .as_array()
            .is_some_and(|repeated| !repeated.is_empty()),
        "the third draft is told the second's diagnostic repeats the first's: {repeated}"
    );
    assert_eq!(
        repeated,
        drafts[2].user_input()["diagnostics"]
            .as_array()
            .expect("the diagnostics")
            .iter()
            .map(|diagnostic| diagnostic["message"].clone())
            .collect::<Json>()
    );
    assert!(
        received
            .iter()
            .all(|request| request.path != "/typesafe/v1/systemone"),
        "no decision request without a granted key"
    );
}

/// No default model backend: without `FLOPPY_BASE_URL` the first
/// generation answers `unconfigured` without sending anything, and the
/// builder fails fast in `failed` (`unconfigured`) with a diagnostic naming
/// the missing base URL (a key is not required: a keyless backend takes
/// none, decision D127), without retrying or asking the decision service.
#[test]
fn without_a_model_base_url_the_build_fails_fast_unconfigured() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "nobase",
        "broken",
        &["FLOPPY_API_KEY", "TYPESAFE_API_KEY", "TYPESAFE_BASE_URL"],
        &json!({"spec": SPEC}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed", "{failed}");
    assert_eq!(failed["reason"], "unconfigured", "{failed}");
    assert_eq!(failed["diagnostics"][0]["code"], "unconfigured", "{failed}");
    let message = failed["diagnostics"][0]["message"].as_str().unwrap_or("");
    assert!(
        message.contains("no FLOPPY_BASE_URL") && !message.contains("no FLOPPY_API_KEY"),
        "{failed}"
    );
    assert_eq!(failed["history"], json!([]), "no retry: {failed}");
    assert!(
        failed.get("scxml").is_none(),
        "no draft was rejected: {failed}"
    );
    let logs: Vec<String> = records
        .iter()
        .filter(|record| record.get("parent").is_none() && record["type"] == "log")
        .map(|record| format!("{} {record}", record["label"]))
        .collect();
    assert!(
        logs.iter().any(|log| log.starts_with("\"failed\"")
            && log.contains("unconfigured")
            && log.contains("FLOPPY_BASE_URL")),
        "the failed log names the missing configuration: {logs:?}"
    );
    assert!(
        mock.received().is_empty(),
        "nothing is sent: {:?}",
        mock.received().len()
    );
}

/// The lifetime check (E0009): with the request's lifetime bounds, a routine
/// whose schedule is shorter than the minimum interval is rejected and
/// repaired; the repaired routine is verified by one forced tick
/// (`<behavior:verify_tick>`), its schedule's event delivered at once, whose output
/// selects an event.
#[test]
fn a_routine_under_the_minimum_interval_is_repaired_and_its_first_tick_is_forced() {
    let mock = Mock::start();
    let records = run(&mock, "lifetime", "lifetime", &ALL, &lifetime_request());
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    let report = &done["report"];
    let rejected = &report["history"][0]["diagnostics"][0];
    assert_eq!(rejected["kind"], "lifetime", "{report}");
    assert_eq!(rejected["code"], "E0009", "{report}");
    assert!(
        rejected["message"]
            .as_str()
            .is_some_and(|message| message.starts_with(
                "every (the hourly.every delay of 30s) is shorter than the 60-second minimum routine interval."
            )),
        "{report}"
    );
    let simulation = report["simulation"].as_array().expect("simulation results");
    assert_eq!(
        simulation.len(),
        2,
        "the draft's test and the forced tick: {report}"
    );
    assert_eq!(simulation[1]["name"], "first tick", "{report}");
    assert_eq!(
        simulation[1]["observed"],
        json!(["bot.behavior.tick"]),
        "{report}"
    );
    assert_eq!(simulation[1]["apply"], json!([]), "{report}");
    assert!(
        simulation.iter().all(|result| result["passed"] == true),
        "{report}"
    );
    // The draft's own tests are what it carried.
    assert_eq!(done["tests"].as_array().map(Vec::len), Some(1), "{done}");
}

/// A schedule sent to a literal target never reaches the routine (the
/// Sparks builder's `#_self`, decision D147): the forced tick would pass it,
/// so the lifetime check rejects it, and the repaired routine is built.
#[test]
fn a_schedule_sent_to_a_target_is_rejected_and_repaired() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "selfschedule",
        "selfschedule",
        &ALL,
        &lifetime_request(),
    );
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    let rejected = &done["report"]["history"][0]["diagnostics"][0];
    assert_eq!(rejected["code"], "E0009", "{done}");
    assert!(
        rejected["message"]
            .as_str()
            .is_some_and(|message| message.starts_with(
                "the hourly.every schedule is sent to #_self, so it never reaches the routine"
            )),
        "{done}"
    );
}

/// A routine may do its own I/O (decision D138): one that POSTs through
/// the `BasicHTTP` Event I/O Processor after each tick is built, its forced
/// first tick observed although the simulation admits no origin.
#[test]
fn a_routine_that_does_its_own_io_is_built() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "httproutine",
        "httproutine",
        &ALL,
        &lifetime_request(),
    );
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    let simulation = done["report"]["simulation"]
        .as_array()
        .expect("simulation results");
    assert_eq!(simulation[1]["name"], "first tick", "{done}");
    assert_eq!(
        simulation[1]["observed"],
        json!(["bot.behavior.tick"]),
        "{done}"
    );
    assert!(
        done["scxml"]
            .as_str()
            .is_some_and(|scxml| scxml.contains("#BasicHTTPEventProcessor")),
        "{done}"
    );
}

/// The forced tick's check: a routine tick that selects nothing fails
/// (E0008), and the build ends with that draft in its failed data.
#[test]
fn a_routine_tick_with_no_selection_is_rejected_with_its_draft() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "emptytick",
        "emptytick",
        &ALL,
        &capped(lifetime_request(), 2),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed", "{failed}");
    assert_eq!(failed["reason"], "attempts-exhausted", "{failed}");
    assert_eq!(
        failed["diagnostics"],
        json!([{"kind": "apply", "code": "E0008", "message": "behavior output must select at least one event for the observed pattern."}]),
        "{failed}"
    );
    assert!(
        failed["scxml"]
            .as_str()
            .is_some_and(|scxml| scxml.contains("output: []")),
        "{failed}"
    );
}

/// The live-turn check of a turn behavior (decision D131): with the
/// request's live turn, the draft also runs once as the candidates runner
/// runs it, with the params turn and input, and its donedata selects on that
/// input, so it is built; the draft's own tests are what it carried.
#[test]
fn a_turn_behavior_that_selects_on_the_live_turn_is_built() {
    let mock = Mock::start();
    let document = builder_document();
    let records = run_deciding_in(
        &document,
        &mock,
        "liveok",
        "liveok",
        "typesafe",
        &ALL,
        &live_request(),
    );
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    let report = &done["report"];
    let simulation = report["simulation"].as_array().expect("simulation results");
    assert_eq!(
        simulation.len(),
        2,
        "the draft's test and the live turn: {report}"
    );
    assert_eq!(simulation[1]["name"], "live turn", "{report}");
    assert_eq!(simulation[1]["passed"], true, "{report}");
    assert_eq!(simulation[1]["done"], true, "{report}");
    assert_eq!(simulation[1]["apply"], json!([]), "{report}");
    assert_eq!(done["tests"].as_array().map(Vec::len), Some(1), "{done}");
    assert!(
        behaviors_of(&document).join("Greet.scx").is_file(),
        "the built document is written"
    );
}

/// Without a live turn a turn behavior runs only its own tests.
#[test]
fn without_a_live_turn_a_turn_behavior_runs_only_its_own_tests() {
    let mock = Mock::start();
    let mut request = live_request();
    request
        .as_object_mut()
        .expect("the request is an object")
        .remove("apply");
    let records = run(&mock, "liveok-none", "liveok", &ALL, &request);
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    assert_eq!(
        done["report"]["simulation"].as_array().map(Vec::len),
        Some(1),
        "{done}"
    );
}

/// A routine (it sends `bot.behavior.tick`) is checked by its forced tick,
/// not the live turn: the check branches on the lifetime (decision D131).
#[test]
fn a_routine_is_checked_by_its_first_tick_not_the_live_turn() {
    let mock = Mock::start();
    let mut request = lifetime_request();
    request["apply"] = live_request()["apply"].clone();
    let records = run(&mock, "lifetime-live", "lifetime", &ALL, &request);
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    let simulation = done["report"]["simulation"]
        .as_array()
        .expect("simulation results");
    assert_eq!(simulation.len(), 2, "{done}");
    assert_eq!(simulation[1]["name"], "first tick", "{done}");
}

/// Runs the live request on `route`, whose drafts all fail the live turn the
/// same way, and asserts the build ends at a cap of 2 (`attempts-exhausted`)
/// on its one E0008 `message`.
fn assert_live_refused(route: &str, message: &str) {
    let mock = Mock::start();
    let records = run(&mock, route, route, &ALL, &capped(live_request(), 2));
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed", "{failed}");
    assert_eq!(failed["reason"], "attempts-exhausted", "{failed}");
    assert_eq!(
        failed["diagnostics"],
        json!([{"kind": "apply", "code": "E0008", "message": message}]),
        "{failed}"
    );
    assert!(failed["scxml"].is_string(), "{failed}");
}

#[test]
fn a_live_turn_that_never_finishes_is_refused() {
    assert_live_refused(
        "livestuck",
        "turn behavior reached no top-level final state on the live turn's input within the turn's bound: it receives no events, so it decides with eventless transitions and ends at once.",
    );
}

/// The run on the live turn runs away (decision D122 ends the call) only
/// because it is given the param input; its own test, with none, passes.
#[test]
fn a_live_turn_that_fails_is_refused() {
    assert_live_refused(
        "liverunaway",
        "turn behavior failed on the live turn's input (error.execution) before it reached a top-level final state.",
    );
}

#[test]
fn a_live_turn_whose_donedata_is_not_the_contract_is_refused() {
    assert_live_refused(
        "liveshape",
        "turn behavior donedata must be the object {turn, handled, selected}: handled a boolean and selected a list of selections.",
    );
}

#[test]
fn a_live_turn_answering_another_turn_is_refused() {
    assert_live_refused(
        "liveturn",
        "turn behavior donedata turn 0 is not the live turn 3: return the turn param as it is.",
    );
}

/// A change whose lesson is not to answer the turn cannot pass: the
/// live-turn check requires the behavior to select on the live turn (decision D131).
#[test]
fn a_live_turn_that_selects_nothing_is_refused() {
    assert_live_refused(
        "liveunhandled",
        "behavior output must select at least one event for the observed pattern.",
    );
}

#[test]
fn a_live_selection_without_an_event_is_refused() {
    assert_live_refused(
        "livenoevent",
        "selected[0] has no event: each selection is an object {event, target, data?, reason?}.",
    );
}

#[test]
fn a_live_selection_without_a_target_is_refused() {
    assert_live_refused(
        "livenotarget",
        r#"selected[0] (bot.ability.communication.respond) names no target: name the session that receives its event, one of ["communication"]."#,
    );
}

#[test]
fn a_live_selection_to_a_session_the_turn_did_not_name_is_refused() {
    assert_live_refused(
        "liveforeign",
        r#"selected[0] (bot.ability.communication.respond) names the target "speaking", which is not one of this turn's targets ["communication"]: name the session exactly as this turn's cognition_output names it."#,
    );
}

#[test]
fn a_live_selection_that_hardcodes_an_input_value_is_refused() {
    assert_live_refused("livebinding", LIVE_BINDING);
}

/// The message of the `/livebinding/v1` draft's run on the live turn.
const LIVE_BINDING: &str = r#"selection data["label"]="doorbell" does not match live event value "knock"; read identifiers from input at runtime."#;

/// A live-turn failure is repaired like any other: the repair is told the
/// E0008 message, and its draft, which reads the label from input, is built.
#[test]
fn a_live_turn_failure_is_repaired_from_its_diagnostic() {
    let mock = Mock::start();
    let records = run(&mock, "liverepair", "liverepair", &ALL, &live_request());
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    let report = &done["report"];
    assert_eq!(report["failures"], 1, "{report}");
    assert_eq!(
        report["history"][0]["diagnostics"][0],
        json!({"kind": "apply", "code": "E0008", "message": LIVE_BINDING}),
        "{report}"
    );
    let received = mock.received();
    let drafts = chat_requests(&received, "scxml_draft");
    assert_eq!(drafts.len(), 2, "{report}");
    assert_eq!(
        drafts[1].user_input()["diagnostics"][0]["message"],
        LIVE_BINDING,
        "the repair is told the live turn's message"
    );
}

/// The lifetime check (E0009): with the request's lifetime bounds, a turn
/// behavior's delayed send longer than the silence bound never fires, so
/// the traffic light (a turn machine with 30 s, 3 s and 20 s timers) is
/// rejected each time; without the bounds the same draft is built (the
/// repair test).
#[test]
fn a_turn_behaviors_delayed_send_beyond_the_silence_bound_is_rejected() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "afterbound",
        "afterbound",
        &ALL,
        &capped(lifetime_request(), 2),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed", "{failed}");
    assert_eq!(failed["reason"], "attempts-exhausted", "{failed}");
    let diagnostics = failed["diagnostics"].as_array().expect("diagnostics");
    assert_eq!(diagnostics.len(), 3, "{failed}");
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic["kind"] == "lifetime"
                && diagnostic["code"] == "E0009"
                && diagnostic["message"]
                    .as_str()
                    .is_some_and(|message| message.contains(
                        "never fires in a turn behavior, which is silenced 1s after it starts"
                    ))),
        "{failed}"
    );
    assert!(
        diagnostics[0]["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("after (the light.timer delay of 30s)")),
        "{failed}"
    );
    assert!(failed["scxml"].is_string(), "{failed}");
}

/// A failure that differs from the one before it is progress: the builder
/// keeps repairing (an invalid draft, then one with a delayed internal send)
/// until the fixed draft, which has a script, ends in `done`. Its three
/// documents are the files the builder wrote beside each other under the
/// request's name, which the simulation loaded by path (decision D121).
#[test]
fn a_different_diagnostic_each_time_keeps_repairing_until_done() {
    let mock = Mock::start();
    let document = builder_document();
    let records = run_deciding_in(
        &document,
        &mock,
        "progress",
        "progress",
        "typesafe",
        &["FLOPPY_API_KEY", "FLOPPY_BASE_URL"],
        &json!({"spec": SPEC, "name": "traffic light"}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    let report = &done["report"];
    assert_eq!(report["attempts"], 3, "{report}");
    assert_eq!(report["failures"], 2, "{report}");
    assert_eq!(report["history"][0]["diagnostics"][0]["code"], "SCXML005");
    // The request's name, one plain file name: `traffic_light`.
    assert_eq!(done["path"], "../../behaviors/traffic_light.scx", "{done}");
    assert_eq!(done["script"], SCRIPT, "{done}");
    let behaviors = behaviors_of(&document);
    let read = |file: &str| {
        std::fs::read_to_string(behaviors.join(file))
            .unwrap_or_else(|error| panic!("{file} is written: {error}"))
    };
    assert_eq!(
        read("traffic_light.scx"),
        done["scxml"].as_str().unwrap_or_default()
    );
    assert_eq!(
        serde_json::from_str::<Json>(&read("traffic_light.json")).expect("JSON"),
        schemas(&["pedestrian.button"])
    );
    assert_eq!(read("traffic_light.js"), SCRIPT);
    assert!(
        report["simulation"]
            .as_array()
            .is_some_and(|results| results.iter().all(|result| result["passed"] == true)),
        "the simulation loaded the draft and its script from its path: {report}"
    );
    assert!(
        report["history"][1]["diagnostics"]
            .as_array()
            .is_some_and(|diagnostics| diagnostics
                .iter()
                .any(|diagnostic| diagnostic["code"] == "SCXML070")),
        "{report}"
    );
}

/// A repair is told where a validate diagnostic is, as `scx validate` renders
/// it (the 2026-10-20 live drafts repeated `XML001` without it), and of a
/// draft whose `scxml` holds no document. `/rendered/v1` answers an
/// unescaped `&&` in red's cond, then a placeholder, then a valid draft,
/// which ends in done.
#[test]
fn a_repair_is_told_the_line_of_each_diagnostic_and_of_a_draft_with_no_document() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "rendered",
        "rendered",
        &["FLOPPY_API_KEY", "FLOPPY_BASE_URL"],
        &json!({"spec": SPEC}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    let codes: Vec<&Json> = done["report"]["history"]
        .as_array()
        .expect("the history")
        .iter()
        .map(|attempt| &attempt["diagnostics"][0]["code"])
        .collect();
    assert_eq!(codes, [&json!("XML001"), &json!("no-document")], "{done}");

    let received = mock.received();
    let authors = chat_requests(&received, "scxml_draft");
    assert_eq!(authors.len(), 3);
    let parameters = &authors[0].body["tools"][0]["function"]["parameters"];
    assert!(
        parameters.pointer("/properties/scxml/minLength").is_none(),
        "no minLength on scxml: the live vLLM's decoding of one runs away into parallel calls until truncated: {parameters}"
    );
    let request = &authors[1];
    let draft = traffic_light(&green_separate(), "red").replace(
        r#"<transition event="light.timer" target="green"/>"#,
        r#"<transition event="light.timer" cond="true && true" target="green"/>"#,
    );
    let (index, line) = draft
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("&&"))
        .expect("the draft has its &&");
    let column = line.find("&&").expect("the line has its &&") + 1;
    let diagnostic = &request.user_input()["diagnostics"][0];
    assert_eq!(diagnostic["code"], "XML001", "{diagnostic}");
    assert_eq!(diagnostic["line"], line.trim(), "{diagnostic}");
    let rendered = diagnostic["rendered"].as_str().expect("rendered text");
    let number = index + 1;
    for expected in [
        "error[XML001]: ".to_owned(),
        format!(" --> traffic_light.scx:{number}:{column}\n"),
        format!("{number} | {line}\n"),
        format!("| {}^", " ".repeat(column - 1)),
        "= help: escape a literal `&` as `&amp;`".to_owned(),
    ] {
        assert!(rendered.contains(&expected), "{expected:?} in {rendered}");
    }
    let no_document = &authors[2].user_input()["diagnostics"][0];
    assert_eq!(no_document["kind"], "draft", "{no_document}");
    assert_eq!(no_document["code"], "no-document", "{no_document}");
    assert!(
        no_document["message"]
            .as_str()
            .is_some_and(|message| message.contains(r#"(it was "placeholder")"#)),
        "{no_document}"
    );
}

/// The same code and label on another line is the same failure (decision
/// D124: the same mistake moves around between drafts, so a validate
/// diagnostic's identity has no line), so the next author call is told it
/// repeats (decision D133). `/moved/v1` answers an unescaped `&&` in red's
/// cond, then in yellow's; the build ends at the request's cap of 3.
#[test]
fn the_same_validate_diagnostic_on_another_line_is_repeated() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "moved",
        "moved",
        &["FLOPPY_API_KEY", "FLOPPY_BASE_URL"],
        &json!({"spec": SPEC, "max_attempts": 3}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed", "{failed}");
    assert_eq!(failed["reason"], "attempts-exhausted", "{failed}");
    assert_eq!(failed["diagnostics"][0]["code"], "XML001", "{failed}");
    assert_eq!(
        failed["history"].as_array().map(Vec::len),
        Some(3),
        "{failed}"
    );
    let received = mock.received();
    let drafts = chat_requests(&received, "scxml_draft");
    assert_eq!(drafts.len(), 3, "no fourth draft");
    assert_eq!(drafts[1].user_input()["repeated"], Json::Null);
    assert_eq!(
        drafts[2].user_input()["repeated"].as_array().map(Vec::len),
        Some(1),
        "the moved XML001 repeats"
    );
}

/// A failure that repeats an earlier one is repeated even when another
/// failure came between them (decision D133). `/again/v1` answers the
/// invalid draft (`SCXML005`), one with a delayed internal send
/// (`SCXML070`), then the invalid draft again, so the fourth draft is
/// told `SCXML005` repeats; the build ends at the request's cap of 4.
#[test]
fn a_failure_after_another_is_repeated() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "again",
        "again",
        &["FLOPPY_API_KEY", "FLOPPY_BASE_URL"],
        &json!({"spec": SPEC, "max_attempts": 4}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed", "{failed}");
    assert_eq!(failed["reason"], "attempts-exhausted", "{failed}");
    let codes: Vec<&Json> = failed["history"]
        .as_array()
        .expect("the history")
        .iter()
        .map(|attempt| &attempt["diagnostics"][0]["code"])
        .collect();
    assert_eq!(
        codes,
        [
            &json!("SCXML005"),
            &json!("SCXML070"),
            &json!("SCXML005"),
            &json!("SCXML005")
        ],
        "{failed}"
    );
    let received = mock.received();
    let drafts = chat_requests(&received, "scxml_draft");
    assert_eq!(drafts.len(), 4);
    assert_eq!(drafts[2].user_input()["repeated"], Json::Null);
    assert!(
        drafts[3].user_input()["repeated"]
            .as_array()
            .is_some_and(|repeated| !repeated.is_empty()),
        "the third failure repeats the first"
    );
}

/// Only the diagnostic an earlier failure had is repeated when the rest of
/// the failure's diagnostics changed (decision D133; the 2026-10-20 live
/// `PingPongResponder` repeated `E0009` four times among other diagnostics).
/// `/changed/v1` answers a draft with `SCXML005` and `SCXML070`, then one
/// with `SCXML005` and `SCXML014`; the request's cap is 3.
#[test]
fn a_repeated_diagnostic_among_changed_ones_is_repeated_alone() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "changed",
        "changed",
        &["FLOPPY_API_KEY", "FLOPPY_BASE_URL"],
        &json!({"spec": SPEC, "max_attempts": 3}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed", "{failed}");
    assert_eq!(failed["reason"], "attempts-exhausted", "{failed}");
    let codes = |attempt: &Json| {
        let mut codes: Vec<String> = attempt["diagnostics"]
            .as_array()
            .expect("the diagnostics")
            .iter()
            .filter_map(|diagnostic| diagnostic["code"].as_str().map(str::to_owned))
            .collect();
        codes.sort();
        codes
    };
    let history = failed["history"].as_array().expect("the history");
    assert_eq!(history.len(), 3, "{failed}");
    assert_eq!(codes(&history[0]), ["SCXML005", "SCXML070"], "{failed}");
    assert_eq!(codes(&history[1]), ["SCXML005", "SCXML014"], "{failed}");
    let received = mock.received();
    let drafts = chat_requests(&received, "scxml_draft");
    assert_eq!(drafts.len(), 3);
    let third = drafts[2].user_input();
    assert_eq!(
        third["diagnostics"].as_array().map(Vec::len),
        Some(2),
        "{third}"
    );
    assert_eq!(
        third["repeated"].as_array().map(Vec::len),
        Some(1),
        "only SCXML005 repeats: {third}"
    );
}

/// A draft whose script is not loaded, or whose cond calls a function no
/// script declares, is told so before it is simulated (decision D123): its
/// conds would throw, and one on an eventless transition is retried every
/// microstep, which ended the 2026-10-20 live run (`machine session failed:
/// protocol`). `/unloaded/v1` answers the scripted draft without its root
/// `<script src>`, then without its script, then whole, which ends in done.
#[test]
fn a_draft_whose_script_is_not_loaded_or_not_declared_is_repaired_before_its_simulation() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "unloaded",
        "unloaded",
        &["FLOPPY_API_KEY", "FLOPPY_BASE_URL"],
        &json!({"spec": SPEC}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    assert_eq!(done["script"], SCRIPT, "{done}");
    let history = done["report"]["history"].as_array().expect("the history");
    assert_eq!(history.len(), 2, "{done}");
    assert_eq!(history[0]["diagnostics"][0]["code"], "script-not-loaded");
    assert!(
        history[0]["diagnostics"][0]["message"]
            .as_str()
            .is_some_and(|message| message.contains(r#"<script src="traffic_light.js"/>"#)),
        "{done}"
    );
    assert_eq!(history[1]["diagnostics"][0]["code"], "undeclared-function");
    assert!(
        history[1]["diagnostics"][0]["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("the document calls mayGo, ")),
        "{done}"
    );
    let simulated = records
        .iter()
        .filter(|record| record["type"] == "log" && record["label"] == "simulate")
        .count();
    assert_eq!(
        simulated, 1,
        "only the whole draft is simulated: {records:?}"
    );
}

/// A test that expects the draft to finish, failed by a draft whose only
/// final is inside a state, says so (decision D123): it reached no
/// top-level final state because it has none. `/nested/v1` answers that
/// draft, then one whose root leads to a top-level final, which ends in done.
#[test]
fn a_draft_with_no_top_level_final_is_told_so_when_its_test_expects_done() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "nested",
        "nested",
        &["FLOPPY_API_KEY", "FLOPPY_BASE_URL"],
        &json!({"spec": "a machine that finishes at once"}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    let diagnostic = &done["report"]["history"][0]["diagnostics"][0];
    assert_eq!(diagnostic["code"], "test-failed", "{done}");
    assert!(
        diagnostic["message"].as_str().is_some_and(|message| message.ends_with(
            "observed [] and done=false (it has no top-level final state: each of its final elements is inside a state, which it only completes; a final that ends the machine is a child of scxml itself)"
        )),
        "{diagnostic}"
    );
}

/// A redesign sends the plan it replaces as `previous_plan` (the request's
/// cap of 2 ends the build after it).
#[test]
fn a_redesign_sends_the_previous_plan() {
    let mock = Mock::start();
    let records = run_deciding(
        &mock,
        "redesign",
        "broken",
        "redesign",
        &ALL,
        &json!({"spec": SPEC, "max_attempts": 2}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed", "{failed}");
    let received = mock.received();
    let designs = chat_requests(&received, "topology_plan");
    assert_eq!(designs.len(), 2, "one design, then the redesign");
    assert_eq!(designs[0].user_input()["previous_plan"], Json::Null);
    let expected = rendered(
        "previous_plan",
        &serde_json::from_str(PLAN).expect("the plan is JSON"),
    );
    let second = designs[1].user_input();
    assert_eq!(
        second["previous_plan"], expected,
        "the redesign sees the plan it replaces"
    );
    assert_eq!(second["diagnostics"][0]["code"], "SCXML005");
}

/// A redesign whose generation failed keeps the plan it replaced: the next
/// redesign still sends it as `previous_plan`.
#[test]
fn a_failed_redesign_keeps_the_previous_plan() {
    let mock = Mock::start();
    let records = run_deciding(
        &mock,
        "flaky",
        "flaky",
        "redesign",
        &ALL,
        &json!({"spec": SPEC, "max_attempts": 3}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed", "{failed}");
    let received = mock.received();
    let designs = chat_requests(&received, "topology_plan");
    assert_eq!(designs.len(), 3, "a design, a failed redesign, a redesign");
    let expected = rendered(
        "previous_plan",
        &serde_json::from_str(PLAN).expect("the plan is JSON"),
    );
    assert_eq!(designs[1].user_input()["previous_plan"], expected);
    assert_eq!(
        designs[2].user_input()["previous_plan"],
        expected,
        "the failed redesign did not lose the plan it replaces"
    );
    assert_eq!(
        designs[2].user_input()["diagnostics"][0]["code"],
        "design-failed"
    );
}

/// A design generation that fails at once (the backend answers `404`), and
/// a later failure that ends the build: the design failure's cause (the
/// generate service's code, `status`) is in its `retry_gate` log and in the
/// `failed` done data's history, though the diagnostics that ended the
/// build are the later ones; the final `failed` log names the reason and
/// the cause that ended the build.
#[test]
fn a_fast_design_failure_keeps_its_cause_in_the_log_and_the_failed_done_data() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "fastfail",
        "fastfail",
        &["FLOPPY_API_KEY", "FLOPPY_BASE_URL"],
        &json!({"spec": SPEC, "max_attempts": 3}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed", "{failed}");
    assert_eq!(failed["reason"], "attempts-exhausted");
    assert_eq!(failed["diagnostics"][0]["code"], "SCXML005");
    let logs: Vec<String> = records
        .iter()
        .filter(|record| record.get("parent").is_none() && record["type"] == "log")
        .map(|record| format!("{} {record}", record["label"]))
        .collect();
    assert!(
        logs.iter().any(|log| log.starts_with("\"retry_gate\"")
            && log.contains("design-failed")
            && log.contains("(status)")),
        "the retry_gate log carries the design failure's cause: {logs:?}"
    );
    assert!(
        logs.iter().any(|log| log.starts_with("\"failed\"")
            && log.contains("attempts-exhausted")
            && log.contains("SCXML005")),
        "the failed log carries the reason and the cause that ended the build: {logs:?}"
    );
    let history = failed["history"]
        .as_array()
        .expect("the failed done data has a history");
    assert_eq!(history.len(), 3, "{failed}");
    assert_eq!(history[0]["diagnostics"][0]["code"], "design-failed");
    assert!(
        history[0]["diagnostics"][0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("(status)")),
        "{failed}"
    );
}

/// Without a plan, `decision:choice` leaves out `triage.repair` (its guard is
/// false), so the decision service is never offered it and `builder.done`
/// never reports a null plan.
#[test]
fn repair_is_not_offered_without_a_plan() {
    let mock = Mock::start();
    let records = run_deciding(
        &mock,
        "noplan",
        "noplan",
        "redesign",
        &ALL,
        &json!({"spec": SPEC, "max_attempts": 2}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed", "{failed}");
    assert_eq!(failed["reason"], "attempts-exhausted");
    assert_eq!(failed["diagnostics"][0]["code"], "design-failed");
    let received = mock.received();
    assert_eq!(chat_requests(&received, "topology_plan").len(), 2);
    assert!(chat_requests(&received, "scxml_draft").is_empty());
    let decisions: Vec<&Received> = received
        .iter()
        .filter(|request| request.path == "/redesign/v1/systemone")
        .collect();
    assert_eq!(decisions.len(), 1);
    let offered: Vec<&String> = decisions[0]
        .body
        .pointer("/questions/selection/criteria")
        .and_then(Json::as_object)
        .expect("the selection question")
        .keys()
        .collect();
    assert!(
        !offered.iter().any(|label| *label == "triage.repair"),
        "{offered:?}"
    );
    assert!(
        offered.iter().any(|label| *label == "triage.redesign"),
        "{offered:?}"
    );
}

/// The builder does not restrict drafts (decision D28): a draft that invokes
/// the `generate` service the run binds validates, and its simulation
/// reaches the service through the run's own `env` scopes and origins, so
/// the build ends in `done`.
#[test]
fn a_draft_that_invokes_the_runs_generate_service_is_simulated_through_it() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "service",
        "service",
        &["FLOPPY_API_KEY", "FLOPPY_BASE_URL"],
        &json!({"spec": "ask generate to say hello", "max_attempts": 1}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    assert_eq!(
        done.pointer("/report/simulation/0/observed"),
        Some(&json!(["answer.ready"])),
        "{done}"
    );
    let received = mock.received();
    let own = received
        .iter()
        .filter(|request| {
            request.path == "/service/v1/chat/completions" && request.schema_name().is_none()
        })
        .collect::<Vec<_>>();
    assert_eq!(own.len(), 1, "the draft generated once: {received:?}");
    assert_eq!(
        own[0].authorization.as_deref(),
        Some(format!("Bearer {FAKE_FLOPPY_KEY}").as_str()),
        "the draft's invoke reached the key through the builder's env scope"
    );
}

/// A candidate that sends `sim.settle` or `sim.drive` to its parent has them
/// recorded as outputs; only the builder's own (invoke-id-less) control
/// events end or drive a test.
#[test]
fn a_candidates_control_event_names_are_recorded_as_outputs() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "impostor",
        "impostor",
        &ALL,
        &json!({"spec": SPEC, "max_attempts": 1}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    let simulation = done
        .pointer("/report/simulation/0/observed")
        .cloned()
        .unwrap_or(Json::Null);
    assert_eq!(
        simulation,
        json!(["sim.settle", "sim.drive", "light.green"]),
        "{done}"
    );
}

/// A `builder.timeout` a cancelled candidate sent to `#_parent` does not
/// end the simulation as a timeout: the first draft's tests fail on its
/// `error.stale` (each test, not the whole simulation), and the repaired
/// draft passes.
#[test]
fn a_cancelled_candidates_builder_timeout_does_not_end_the_simulation() {
    let mock = Mock::start();
    let records = run(&mock, "stale", "stale", &ALL, &json!({"spec": SPEC}));
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    let report = &done["report"];
    assert_eq!(report["failures"], 1, "{report}");
    let diagnostics = report["history"][0]["diagnostics"]
        .as_array()
        .expect("the failure's diagnostics");
    assert_eq!(diagnostics.len(), 2, "both tests ran: {report}");
    assert!(
        diagnostics.iter().all(|d| d["code"] == "test-failed"
            && d["message"]
                .as_str()
                .is_some_and(|m| m.contains("error error.stale"))),
        "{report}"
    );
}

/// The `sim.drive` of a test whose candidate fails to start is not taken by
/// the next test: that test's `ping` is sent once. The builder here is
/// `builder.scx` with its first test's invocation path replaced by one where
/// no document is (a validated draft gives no way to make the child's start
/// fail).
#[test]
fn a_failed_invocations_sim_drive_does_not_drive_the_next_test() {
    let source = builder_source();
    let failing = source.replacen(
        r#"srcexpr="testPath()""#,
        r#"srcexpr="sim_index === 0 ? 'missing.scx' : testPath()""#,
        1,
    );
    assert_ne!(failing, source, "the builder invokes the draft's path");
    let mock = Mock::start();
    let records = run_builder(
        &mock,
        "pings",
        "pings",
        &failing,
        &json!({"spec": SPEC, "max_attempts": 1}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed", "{failed}");
    let diagnostics = failed["diagnostics"]
        .as_array()
        .expect("the failure's diagnostics");
    assert_eq!(
        diagnostics.len(),
        1,
        "only the test whose invocation failed fails: {failed}"
    );
    let message = diagnostics[0]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("test first run") && message.contains("error error.execution"),
        "{failed}"
    );
}

/// A draft whose simulation runs away (a raise that re-enters its own
/// state) fails its tests and is repaired, and the build runs on: scxml.rs
/// ends a candidate whose call stops at the per-call command bound as a
/// cancelled invocation, raising `error.execution` for a start that
/// stopped there and queueing `error.communication.<invokeid>` for a later
/// call (decision D122), where the run used to end with `command-bound`;
/// its diagnostic names that error without the invoke id (decision D124).
#[test]
fn a_runaway_drafts_simulation_fails_its_tests_and_is_repaired() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "runaway",
        "runaway",
        &["FLOPPY_API_KEY", "FLOPPY_BASE_URL"],
        &json!({"spec": SPEC}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    let report = &done["report"];
    assert_eq!(report["failures"], 2, "{report}");
    let diagnostics = |attempt: usize| {
        report["history"][attempt]["diagnostics"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    };
    let start = diagnostics(0);
    assert_eq!(start.len(), 2, "both tests' starts ran away: {report}");
    assert!(
        start.iter().all(|d| d["code"] == "test-failed"
            && d["message"]
                .as_str()
                .is_some_and(|m| m.contains("error error.execution"))),
        "{report}"
    );
    let later = diagnostics(1);
    assert_eq!(later.len(), 1, "only the button's test ran away: {report}");
    assert!(
        later[0]["message"]
            .as_str()
            .is_some_and(|m| m.contains("test button ends green early")
                && m.ends_with(", error error.communication")),
        "the error without the candidate's generated invoke id, new on every run (decision D124): {report}"
    );
}

/// A draft whose eventless transition's cond throws on every evaluation
/// fails its tests and is repaired, and the build runs on: scxml.rs
/// `cf8f293a` stops such a cond at the per-call command bound (one command
/// per failure) instead of running out of fuel, so the candidate's start is
/// ended as D122 ends a runaway (`error.execution`), where the 2026-10-20
/// live run 1 ended with `machine session failed: protocol`.
#[test]
fn a_draft_whose_eventless_cond_always_throws_fails_its_tests_and_is_repaired() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "throwing",
        "throwing",
        &["FLOPPY_API_KEY", "FLOPPY_BASE_URL"],
        &json!({"spec": SPEC}),
    );
    assert!(finished(&records), "{records:?}");
    let (label, done) = answer(&records);
    assert_eq!(label, "done", "{done}");
    assert_eq!(done["script"], SCRIPT, "{done}");
    let report = &done["report"];
    assert_eq!(report["failures"], 1, "{report}");
    let failed = report["history"][0]["diagnostics"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(failed.len(), 2, "both tests' starts failed: {report}");
    assert!(
        failed.iter().all(|d| d["code"] == "test-failed"
            && d["message"]
                .as_str()
                .is_some_and(|m| m.contains("error error.execution"))),
        "{report}"
    );
}

#[test]
fn a_request_its_schema_rejects_is_refused_before_the_builder() {
    let mock = Mock::start();
    let records = run(
        &mock,
        "refused",
        "repair",
        &ALL,
        &json!({"spec": "", "max_attempts": 99}),
    );
    // The intake transition's event:schema rejects it, so scx run refuses
    // the input and the builder stays in intake.
    assert!(
        records
            .iter()
            .any(|record| record["type"] == "error" && record["code"] == "machine-invalid-input"),
        "{records:?}"
    );
    assert!(!finished(&records), "{records:?}");
    assert!(mock.received().is_empty(), "nothing is generated");
}

/// A live turn its schema rejects is refused as the rest of the request
/// is: each bound of `apply` alone, the rest of the live turn valid (the
/// pass tests show that the valid live turn is accepted).
#[test]
fn a_live_turn_its_schema_rejects_is_refused_before_the_builder() {
    let mock = Mock::start();
    let many: Vec<String> = (0..33).map(|n| format!("s{n}")).collect();
    let bounds = [
        ("turn-zero", "/apply/turn", json!(0)),
        ("turn-fraction", "/apply/turn", json!(1.5)),
        ("input-array", "/apply/input", json!([])),
        ("input-null", "/apply/input", Json::Null),
        ("target-number", "/apply/targets", json!([1])),
        ("target-empty", "/apply/targets", json!([""])),
        ("targets-many", "/apply/targets", json!(many)),
        ("extra-member", "/apply/extra", json!(true)),
        ("handled-string", "/apply/handled", json!("no")),
        ("apply-null", "/apply", Json::Null),
    ];
    for (case, pointer, value) in bounds {
        let mut request = live_request();
        let (parent, member) = pointer.rsplit_once('/').expect("a pointer");
        request
            .pointer_mut(parent)
            .and_then(Json::as_object_mut)
            .expect("the member's object")
            .insert(member.to_owned(), value);
        let records = run(
            &mock,
            &format!("refused-apply-{case}"),
            "liveok",
            &ALL,
            &request,
        );
        assert!(
            records.iter().any(
                |record| record["type"] == "error" && record["code"] == "machine-invalid-input"
            ),
            "{case}: {records:?}"
        );
        assert!(!finished(&records), "{case}: {records:?}");
    }
    assert!(mock.received().is_empty(), "nothing is generated");
}

#[test]
fn a_blank_spec_is_answered_with_builder_failed() {
    let mock = Mock::start();
    let records = run(&mock, "invalid", "repair", &ALL, &json!({"spec": "   "}));
    assert!(finished(&records), "{records:?}");
    let (label, failed) = answer(&records);
    assert_eq!(label, "failed");
    assert_eq!(failed["reason"], "invalid-request");
    assert_eq!(failed["diagnostics"][0]["kind"], "request");
    assert!(mock.received().is_empty(), "nothing is generated");
}

// ---------------------------------------------------------------------------
// Live.

/// Builds the spec with the real `generate` (and `decision`) backends on the
/// wall clock (`FLOPPY_LIVE_ENV_FILE`; `FLOPPY_LIVE_MODEL`, default
/// [`support::DEFAULT_MODEL`]). Writes the produced bot under `target/builder-live/` and
/// prints only pass or fail and counts.
#[test]
#[ignore = "makes real model calls; needs FLOPPY_LIVE_ENV_FILE"]
fn live_builder_through_an_env_file() {
    let env_files = support::live_env_files();
    let model = support::live_model();
    let finished = start(
        &builder_document(),
        "wall",
        &env_files,
        &json!({
            "spec": SPEC,
            "constraints": [
                "send every light change and every walk signal to the invoker",
                "a button press during green ends green early",
            ],
            "model": model,
        }),
        // No values from the shell: the env files must hold FLOPPY_BASE_URL
        // (generate has no default, D31); the decision service uses its
        // package's declared origin unless they set TYPESAFE_BASE_URL (D18).
        &[],
    )
    .finish(Duration::from_mins(30));
    let out = workspace().join("target/builder-live");
    std::fs::create_dir_all(&out).expect("the output directory is creatable");
    let (label, data) = answer(&finished.records);
    let pass = if label == "done" {
        {
            let scxml = data["scxml"].as_str().unwrap_or_default().to_owned();
            let valid = compiles("traffic_light", &data);
            std::fs::write(out.join("traffic_light.scx"), &scxml).expect("writable");
            std::fs::write(
                out.join("traffic_light.js"),
                data["script"].as_str().unwrap_or_default(),
            )
            .expect("writable");
            for member in ["schemas", "tests", "report"] {
                std::fs::write(
                    out.join(format!("{member}.json")),
                    serde_json::to_string_pretty(&data[member]).expect("JSON"),
                )
                .expect("writable");
            }
            println!(
                "live builder: attempts {}, failures {}, fixes {}, decisions {}",
                data["report"]["attempts"],
                data["report"]["failures"],
                data["report"]["fixes_applied"],
                data["report"]["decisions"]
            );
            valid
        }
    } else {
        std::fs::write(
            out.join("failed.json"),
            serde_json::to_string_pretty(&data).expect("JSON"),
        )
        .expect("writable");
        false
    };
    println!("live builder: {}", if pass { "pass" } else { "fail" });
    assert!(pass, "the live build did not pass");
}
