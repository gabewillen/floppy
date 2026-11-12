//! Reasoning's frame check (`abilities/reasoning/reasoning.js`
//! `isSameFrame`, decision D275): the input that brings a turn to this
//! tier must frame it as the turn in hand was framed. A harness loads
//! reasoning's ECMAScript, holds one frame, and checks inputs against it
//! through `scx run`.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::fmt::Write as _;
use std::time::Duration;

use serde_json::{Value as Json, json};
use support::{Run, logs, unique_scratch, workspace};

const LIMIT: Duration = Duration::from_secs(300);

/// The frame's keys, as reasoning holds them.
const KEYS: [&str; 8] = [
    "focus",
    "focus_candidates",
    "choices",
    "held",
    "environment",
    "self",
    "acted",
    "recent",
];

/// A frame whose recent conversation is `messages` grids of 30 by 30
/// digits, about 1 KB each.
fn frame(messages: usize) -> Json {
    let grid = |turn: usize| -> String {
        (0..30)
            .map(|row| {
                let mut line: String = (0..30)
                    .map(|column| {
                        char::from(
                            b'0' + u8::try_from((turn * 7 + row * 3 + column) % 10)
                                .expect("a digit"),
                        )
                    })
                    .collect();
                line.push('\n');
                line
            })
            .collect()
    };
    json!({
        "focus": {"device": "chat", "source_ids": ["gabe"]},
        "focus_candidates": [],
        "choices": (0..30).map(|i| json!({"event": format!("bot.ability.choice{i}"), "target": "t", "description": "d".repeat(300)})).collect::<Vec<_>>(),
        "held": [],
        "environment": {"devices": []},
        "self": {"name": "floppy"},
        "acted": [],
        "recent": (0..messages).map(|turn| json!({"source_ids": ["gabe"], "target_ids": ["floppy"], "content": grid(turn), "content_type": "text/plain"})).collect::<Vec<_>>(),
    })
}

/// Runs the harness: it holds `held`, then logs `same` or `different` for
/// each of `inputs`.
fn checked(held: &Json, inputs: &[Json]) -> (Vec<String>, u64) {
    let dir = unique_scratch("reasoning");
    std::fs::copy(
        workspace().join("abilities/reasoning/reasoning.js"),
        dir.join("reasoning.js"),
    )
    .expect("reasoning's ECMAScript is copyable");
    let mut data = String::new();
    let mut assignments = String::new();
    for key in KEYS {
        write!(data, r#"<data id="{key}" expr="null"/>"#).expect("a String takes writes");
        write!(
            assignments,
            r#"<assign location="{key}" expr="_event.data.{key}"/>"#
        )
        .expect("a String takes writes");
    }
    let document = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="holding">
  <datamodel>{data}</datamodel>
  <script src="reasoning.js"/>
  <state id="holding">
    <transition event="hold" target="checking">{assignments}</transition>
  </state>
  <state id="checking">
    <transition event="check" cond="isSameFrame(_event.data)"><log label="frame" expr="'same'"/></transition>
    <transition event="check"><log label="frame" expr="'different'"/></transition>
    <transition event="end" target="done"/>
  </state>
  <final id="done"/>
</scxml>"#
    );
    let path = support::write(&dir, "harness.scx", &document);
    let mut run = Run::start(
        &[
            "--fuel".to_owned(),
            support::FUEL.to_owned(),
            path.display().to_string(),
        ],
        &[],
    );
    run.event("hold", held);
    for input in inputs {
        run.event("check", input);
    }
    run.event("end", &json!({}));
    let finished = run.finish(LIMIT);
    let verdicts = logs(&finished.all)
        .into_iter()
        .filter(|(label, _)| label == "frame")
        .map(|(_, text)| text)
        .collect();
    let peak = finished
        .all
        .iter()
        .filter_map(|record| record["data_model_memory_peak_bytes"].as_u64())
        .max()
        .unwrap_or(0);
    (verdicts, peak)
}

/// The same frame is the same, and one changed digit of the oldest message,
/// another focus source, or a missing message is not.
#[test]
fn an_input_is_checked_against_the_frame_in_hand() {
    let held = frame(3);
    let mut digit = held.clone();
    digit["recent"][0]["content"] = Json::from(
        digit["recent"][0]["content"]
            .as_str()
            .expect("a grid")
            .replacen('0', "1", 1),
    );
    let mut focus = held.clone();
    focus["focus"]["source_ids"] = json!(["ana"]);
    let mut shorter = held.clone();
    shorter["recent"]
        .as_array_mut()
        .expect("the recent conversation")
        .pop();
    let (verdicts, _) = checked(&held, &[held.clone(), digit, focus, shorter]);
    assert_eq!(verdicts, ["same", "different", "different", "different"]);
}

/// A long conversation's frame is checked without growing the data
/// model's memory: comparing it as JSON text grew it by about 4 MB for
/// 300 messages (the Continual-ARC stream's turns reached 64 MiB).
#[test]
fn a_long_frame_is_checked_without_growing_memory() {
    let held = frame(300);
    let (verdicts, peak) = checked(&held, &[held.clone(), held.clone()]);
    assert_eq!(verdicts, ["same", "same"]);
    assert!(peak > 0, "the call records carry the data model's peak");
    assert!(peak <= 8 * 1024 * 1024, "peak {peak}");
}
