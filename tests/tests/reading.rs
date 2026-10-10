//! The reading ability (`abilities/reading.scx`, mosfet's Reading) and its
//! visual classification child (`abilities/vision.scx`, mosfet's
//! `VisualClassification`), run as a harness document's child through
//! `scx run` on the logical clock.
//!
//! Each test lays out a scratch directory with the harness, copies of
//! `reading.scx` and `vision.scx`, and `abilities/schemas/` as `schemas/`.
//! The harness invokes reading as its child "reading", sends it each input
//! once it is invoked, logs every answer (`harness`, the event name and its
//! JSON), and ends after one answer per input.
//!
//! The image decoder is the generate service with an image part, which a
//! parallel slice adds; until it lands, the copy of reading invokes
//! `generate_double.scx` in its place (the same `<param>`s, one of
//! generate's events back). The double logs the `model` and `messages` it
//! was given (`double.model`, `double.messages`).

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use support::{Finished, Run, logs, unique_scratch, workspace};

/// The generate `<invoke>` of `abilities/reading.scx`, which the copy
/// replaces with the double.
const GENERATE_INVOKE: &str = r#"<invoke idlocation="gen_id" type="https://github.com/stateforward/floppy/services/generate"
                env:apikey="FLOPPY_API_KEY" env:baseurl="FLOPPY_BASE_URL">"#;

const DOUBLE_INVOKE: &str =
    r#"<invoke idlocation="gen_id" type="scxml" src="generate_double.scx">"#;

/// What the generate double does once invoked.
enum Generate {
    /// Answers `generate.result {text}` (an ECMAScript expression).
    Result(&'static str),
    /// Answers `generate.result {text}` after one logical second.
    Later(&'static str),
    /// Answers `generate.failed {code}`.
    Failed(&'static str),
    /// Ends without answering.
    Silent,
    /// Never answers.
    Hangs,
}

fn double(generate: &Generate) -> String {
    let reply = |text: &str| {
        format!(
            r##"<send event="generate.result" target="#_parent"><param name="text" expr="{text}"/></send>"##
        )
    };
    let (onentry, transitions) = match generate {
        Generate::Result(text) => (reply(text), r#"<transition target="done"/>"#.to_owned()),
        Generate::Later(text) => (
            r#"<send event="double.reply" delay="1s"/>"#.to_owned(),
            format!(
                r#"<transition event="double.reply" target="done">{}</transition>"#,
                reply(text)
            ),
        ),
        Generate::Failed(code) => (
            format!(
                r##"<send event="generate.failed" target="#_parent"><param name="code" expr="'{code}'"/></send>"##
            ),
            r#"<transition target="done"/>"#.to_owned(),
        ),
        Generate::Silent => (String::new(), r#"<transition target="done"/>"#.to_owned()),
        Generate::Hangs => (String::new(), String::new()),
    };
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="generating">
  <datamodel>
    <data id="model" expr="null"/>
    <data id="messages" expr="null"/>
  </datamodel>
  <state id="generating">
    <onentry>
      <log label="double.model" expr="model"/>
      <log label="double.messages" expr="JSON.stringify(messages)"/>
      {onentry}
    </onentry>
    {transitions}
  </state>
  <final id="done"/>
</scxml>"#
    )
}

/// A harness that sends reading `inputs` (an ECMAScript array expression)
/// and logs each answer.
fn harness(inputs: &str) -> String {
    format!(
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="run">
  <datamodel>
    <data id="inputs" expr="{inputs}"/>
    <data id="answers" expr="0"/>
  </datamodel>
  <state id="run">
    <onentry>
      <send event="harness.start" delay="10ms"/>
    </onentry>
    <invoke id="reading" type="scxml" src="reading.scx"/>
    <transition event="harness.start">
      <foreach array="inputs" item="input">
        <send event="bot.ability.reading.input" target="#_reading">
          <content expr="input"/>
        </send>
      </foreach>
    </transition>
    <transition event="bot.ability.reading.output bot.ability.reading.failed" cond="_event.invokeid === 'reading'">
      <log label="harness" expr="_event.name + ' ' + JSON.stringify(_event.data)"/>
      <assign location="answers" expr="answers + 1"/>
    </transition>
    <transition cond="answers === inputs.length" target="done"/>
  </state>
  <final id="done"/>
</scxml>"##
    )
}

/// Lays out the harness beside reading (its generate invoke replaced by
/// the double), vision and the abilities' schemas; returns the harness.
fn layout(inputs: &str, generate: &Generate) -> PathBuf {
    let dir = unique_scratch("reading");
    let reading = std::fs::read_to_string(workspace().join("abilities/reading.scx"))
        .expect("the reading ability is readable");
    let doubled = reading.replacen(GENERATE_INVOKE, DOUBLE_INVOKE, 1);
    assert_ne!(doubled, reading, "reading invokes the generate service");
    support::write(&dir, "reading.scx", &doubled);
    std::fs::copy(
        workspace().join("abilities/vision.scx"),
        dir.join("vision.scx"),
    )
    .expect("the vision ability is copyable");
    let schemas = dir.join("schemas");
    std::fs::create_dir_all(&schemas).expect("the schema directory is creatable");
    for entry in std::fs::read_dir(workspace().join("abilities/schemas"))
        .expect("the abilities' schemas are readable")
    {
        let path = entry.expect("a directory entry").path();
        std::fs::copy(&path, schemas.join(path.file_name().expect("a file name")))
            .expect("the schema is copyable");
    }
    support::write(&dir, "generate_double.scx", &double(generate));
    support::write(&dir, "harness.scx", &harness(inputs))
}

fn run(harness: &Path) -> Finished {
    let mut run = Run::start(
        &[
            "--clock".to_owned(),
            "logical".to_owned(),
            harness.display().to_string(),
        ],
        &[],
    );
    run.end_input();
    run.finish(Duration::from_secs(300))
}

/// The texts of the logs labelled `label`, in order.
fn texts(finished: &Finished, label: &str) -> Vec<String> {
    logs(&finished.records)
        .into_iter()
        .filter(|(l, _)| l == label)
        .map(|(_, text)| text)
        .collect()
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn text_is_read_as_text() {
    let finished = run(&layout(
        "[{kind: 'text', content: 'Read this note.'}]",
        &Generate::Hangs,
    ));
    assert_eq!(
        texts(&finished, "harness"),
        strings(&[
            r#"bot.ability.reading.output {"confidence":1,"source_kind":"text","text":"Read this note."}"#
        ]),
        "{}",
        finished.stderr
    );
    assert_eq!(
        texts(&finished, "vision.classification"),
        strings(&["text"])
    );
    assert_eq!(texts(&finished, "reading.focused"), strings(&["text"]));
    assert_eq!(texts(&finished, "reading.output"), strings(&["text"]));
    assert!(texts(&finished, "double.model").is_empty());
}

#[test]
fn blank_text_and_an_empty_image_reference_are_unreadable() {
    let finished = run(&layout(
        "[{kind: 'text', content: '  \\n '}, {kind: 'image', content: ''}]",
        &Generate::Hangs,
    ));
    assert_eq!(
        texts(&finished, "harness"),
        strings(&[
            r#"bot.ability.reading.output {"confidence":1,"source_kind":"unreadable","text":""}"#,
            r#"bot.ability.reading.output {"confidence":1,"source_kind":"unreadable","text":""}"#,
        ]),
        "{}",
        finished.stderr
    );
    assert_eq!(
        texts(&finished, "vision.classification"),
        strings(&["unreadable", "unreadable"])
    );
    assert!(texts(&finished, "double.model").is_empty());
}

#[test]
fn an_image_is_read_by_the_vision_model() {
    let finished = run(&layout(
        "[{kind: 'image', content: 'media/receipt.png'}]",
        &Generate::Result("'  Total due: $14.21\\n'"),
    ));
    assert_eq!(
        texts(&finished, "harness"),
        strings(&[
            r#"bot.ability.reading.output {"confidence":1,"source_kind":"image","text":"Total due: $14.21"}"#
        ]),
        "{}",
        finished.stderr
    );
    assert_eq!(
        texts(&finished, "double.model"),
        strings(&["Qwen/Qwen3.8-Flash-Next"])
    );
    assert_eq!(
        texts(&finished, "double.messages"),
        strings(&[
            r#"[{"content":[{"content":"Read all visible text in this image. Return only the text.","kind":"text"},{"content":"media/receipt.png","kind":"image"}],"role":"user"}]"#
        ])
    );
    assert_eq!(
        texts(&finished, "vision.classification"),
        strings(&["image"])
    );
}

#[test]
fn a_failed_image_decoding_fails_the_reading() {
    let finished = run(&layout(
        "[{kind: 'image', content: 'media/receipt.png'}]",
        &Generate::Failed("unconfigured"),
    ));
    assert_eq!(
        texts(&finished, "harness"),
        strings(&[
            r#"bot.ability.reading.failed {"message":"the image decoding failed (unconfigured)","stage":"image_decoding"}"#
        ]),
        "{}",
        finished.stderr
    );
    assert_eq!(
        texts(&finished, "reading.failed"),
        strings(&["image_decoding: the image decoding failed (unconfigured)"])
    );
}

#[test]
fn an_image_decoding_without_a_reply_fails_the_reading() {
    let finished = run(&layout(
        "[{kind: 'image', content: 'media/receipt.png'}]",
        &Generate::Silent,
    ));
    assert_eq!(
        texts(&finished, "harness"),
        strings(&[
            r#"bot.ability.reading.failed {"message":"the image decoding returned no text","stage":"image_decoding"}"#
        ]),
        "{}",
        finished.stderr
    );
}

#[test]
fn the_image_decoding_times_out_after_the_stage_timeout() {
    let finished = run(&layout(
        "[{kind: 'image', content: 'media/receipt.png'}]",
        &Generate::Hangs,
    ));
    assert_eq!(
        texts(&finished, "harness"),
        strings(&[
            r#"bot.ability.reading.failed {"message":"the image decoding timed out after 30000 ms","stage":"image_decoding"}"#
        ]),
        "{}",
        finished.stderr
    );
}

#[test]
fn an_input_that_arrives_while_focused_waits_its_turn() {
    let finished = run(&layout(
        "[{kind: 'image', content: 'media/receipt.png'}, {kind: 'text', content: 'Read this note.'}]",
        &Generate::Later("'Total due: $14.21'"),
    ));
    assert_eq!(
        texts(&finished, "harness"),
        strings(&[
            r#"bot.ability.reading.output {"confidence":1,"source_kind":"image","text":"Total due: $14.21"}"#,
            r#"bot.ability.reading.output {"confidence":1,"source_kind":"text","text":"Read this note."}"#,
        ]),
        "{}",
        finished.stderr
    );
    assert_eq!(
        texts(&finished, "reading.focused"),
        strings(&["image", "text"])
    );
}
