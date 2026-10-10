//! The floppy bot's cognition cascade (`abilities/cognition.scx` with its
//! stages `abilities/autonomy.scx`, `intuition.scx`, and `reasoning.scx`),
//! run as the body's child through `scx run` on the wall clock.
//!
//! Each test lays out a scratch copy of the repository's shape (the body
//! under `bots/floppy/`, every ability document and schema under
//! `abilities/`), with `<data>` expressions rewritten to shorten bounds, and
//! runs it against one deterministic fake backend on 127.0.0.1 that serves
//! both services (nothing reaches the network): the decision service's
//! `TypeSafe` `system_one` endpoint and the generate service's streamed Chat
//! Completions endpoint. The first path segment of each base URL picks the
//! scripted answer ([`Decision`], [`Generation`]); every request is
//! recorded. The keys are fake and reach the run only through an env file.
//!
//! The stages log what they do (`autonomy`, `intuition`, `reasoning`,
//! `memory`), the cognition its cascade (`cognition.stage`,
//! `cognition.tuner`, `cognition.cancelled`, `cognition.offers`,
//! `reflection`), and the body each event it emits.
//!
//! Every test ends by comparing its whole run, and every request the fake
//! backend received, with its pinned trace under
//! `tests/fixtures/phase4/traces/` (`support::trace`;
//! `FLOPPY_TRACE_BLESS=1` rewrites them).

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value as Json, json};
use support::{Finished, Run, logs, trace, unique_scratch, workspace};

/// Bounded wait for any one expected record (scx startup included).
const WAIT: Duration = Duration::from_secs(300);
/// How long the mock must receive nothing before a blessing run takes its
/// requests as complete.
const SETTLE: Duration = Duration::from_secs(1);
/// The fake keys (not secrets).
const FAKE_KEYS: &str = "FLOPPY_API_KEY=floppy-fake\nTYPESAFE_API_KEY=ts-floppy-fake\n";
const IGNORE: &str = "bot.ability.cognition.ignore";
const RESPOND: &str = "bot.ability.communication.respond";
const ACTIVATE: &str = "bot.ability.communication.activate";
const ADOPT: &str = "bot.ability.identity.adopt";
/// The turn's choices as the body logs them: the cognition's offer, the
/// communication's and the identity's.
const CHOICES: &str = "bot.ability.cognition.ignore@cognition bot.ability.communication.respond@communication bot.ability.communication.activate@communication bot.ability.identity.adopt@identity";

/// The decision backend's scripted answer.
#[derive(Clone, Copy)]
enum Decision {
    /// Chooses ignore (0.8).
    Ignore,
    /// Chooses the communication's activate (0.8).
    Activate,
    /// Chooses "none of these" (0.8): reasoning handles the turn.
    Abstain,
    /// Chooses ignore with 0.34 (each other label 0.33).
    Low,
    /// Answers 500.
    Fail,
    /// Never answers.
    Hang,
}

impl Decision {
    const fn path(self) -> &'static str {
        match self {
            Self::Ignore => "ignore",
            Self::Activate => "activate",
            Self::Abstain => "abstain",
            Self::Low => "low",
            Self::Fail => "dfail",
            Self::Hang => "dhang",
        }
    }
}

/// The generate backend's scripted answer.
#[derive(Clone, Copy)]
enum Generation {
    /// One `bot.ability.cognition.ignore` selection for the cognition.
    Ignore,
    /// A selection of an event outside the choices.
    OffChoice,
    /// The communication's activate, then a response to say.
    Respond,
    /// Answers 500.
    Fail,
    /// Never answers.
    Hang,
}

impl Generation {
    const fn path(self) -> &'static str {
        match self {
            Self::Ignore => "gignore",
            Self::OffChoice => "offchoice",
            Self::Respond => "grespond",
            Self::Fail => "gfail",
            Self::Hang => "ghang",
        }
    }
}

/// One request the fake backend received.
#[derive(Clone, Debug)]
struct Received {
    target: String,
    body: Json,
}

/// A streamed Chat Completions answer carrying `content`.
fn streamed(content: &str) -> String {
    let chunk = |delta: Json, finish: Option<&str>| {
        json!({
            "object": "chat.completion.chunk",
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
        })
    };
    [
        chunk(json!({"role": "assistant", "content": ""}), None),
        chunk(json!({"content": content}), None),
        chunk(json!({}), Some("stop")),
    ]
    .iter()
    .map(|chunk| format!("data: {chunk}\n\n"))
    .chain(std::iter::once("data: [DONE]\n\n".to_owned()))
    .collect()
}

/// The `system_one` answer: the selection question's `chosen` label gets
/// `share`, the rest share what is left; every other question answers its
/// first label.
fn decided(body: &Json, chosen: &str, share: f64) -> Option<String> {
    let questions = body.get("questions")?.as_object()?;
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
                let p = if label.as_str() == chosen {
                    share
                } else {
                    (1.0 - share) / f64::from(u32::try_from(rest.max(1)).ok()?)
                };
                probabilities.insert((*label).clone(), p.into());
            }
            chosen.to_owned()
        } else {
            let first = (*labels.first()?).clone();
            probabilities.insert(first.clone(), 1.0.into());
            first
        };
        answers.insert(
            name.clone(),
            json!({"type": "choice", "choice": choice, "confidence": share, "probabilities": probabilities}),
        );
    }
    Some(json!({"model": "fake", "answers": answers, "usage": {"input_tokens": 1, "output_tokens": 1}}).to_string())
}

/// The scripted answer to one request, or `None` to never answer.
fn answer(target: &str, body: &Json) -> Option<(&'static str, String)> {
    let ok = |text: String| Some(("200 OK", text));
    let fail = || {
        Some((
            "500 Internal Server Error",
            r#"{"error":{"message":"boom"}}"#.to_owned(),
        ))
    };
    let selection = |events: Json| ok(streamed(&json!({ "events": events }).to_string()));
    match target {
        "/ignore/v1/systemone" => ok(decided(body, IGNORE, 0.8)?),
        "/activate/v1/systemone" => ok(decided(body, ACTIVATE, 0.8)?),
        "/abstain/v1/systemone" => ok(decided(body, "none of these", 0.8)?),
        "/low/v1/systemone" => ok(decided(body, IGNORE, 0.34)?),
        "/gignore/v1/chat/completions" => selection(json!([
            {"event": IGNORE, "data": {}, "reason": "nothing to say", "target": "cognition"}
        ])),
        "/grespond/v1/chat/completions" => selection(json!([
            {"event": ACTIVATE, "data": {}, "reason": "talk here", "target": "communication"},
            {"event": RESPOND, "data": {"text": "Hi! I am floppy."}, "reason": "greet", "target": "communication"}
        ])),
        "/offchoice/v1/chat/completions" => selection(json!([
            {"event": "bot.focus_device", "data": {"device": "lamp"}, "reason": "look"}
        ])),
        "/dfail/v1/systemone" | "/gfail/v1/chat/completions" => fail(),
        _ => None,
    }
}

fn serve(mut stream: TcpStream, log: &Arc<Mutex<Vec<Received>>>) {
    let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut length = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.trim_end().split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let target = request_line.split(' ').nth(1).unwrap_or("").to_owned();
    let body: Json = serde_json::from_slice(&body).unwrap_or(Json::Null);
    let reply = answer(&target, &body);
    log.lock()
        .expect("the log lock")
        .push(Received { target, body });
    match reply {
        Some((status, text)) => {
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                text.len()
            );
        }
        // Hold the connection open, answering nothing.
        None => std::thread::sleep(Duration::from_secs(180)),
    }
}

/// The fake backend of both services.
struct Mock {
    origin: String,
    received: Arc<Mutex<Vec<Received>>>,
}

impl Mock {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock binds a loopback port");
        let origin = format!(
            "http://{}",
            listener.local_addr().expect("the mock has an address")
        );
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let log = Arc::clone(&log);
                std::thread::spawn(move || serve(stream, &log));
            }
        });
        Self { origin, received }
    }

    /// Waits until every request the run sent has reached the mock: a
    /// stage that timed out still sends its hanging backend's request
    /// (the service call is not withdrawn), and under load that request
    /// can arrive after the run has closed. With `expected` (the pinned
    /// trace's count), waits until that many arrived, at most [`WAIT`];
    /// without it (blessing), until none arrived for [`SETTLE`].
    fn settle(&self, expected: Option<usize>) {
        let count = || {
            self.received
                .lock()
                .expect("the log lock")
                .iter()
                .filter(|request| pinned(&request.target))
                .count()
        };
        let start = Instant::now();
        let mut seen = count();
        let mut quiet = Instant::now();
        while start.elapsed() < WAIT {
            match expected {
                Some(expected) if seen >= expected => return,
                None if quiet.elapsed() >= SETTLE => return,
                _ => {}
            }
            std::thread::sleep(Duration::from_millis(20));
            let now = count();
            if now != seen {
                seen = now;
                quiet = Instant::now();
            }
        }
    }

    fn received(&self, suffix: &str) -> Vec<Received> {
        self.received
            .lock()
            .expect("the log lock")
            .iter()
            .filter(|request| request.target.ends_with(suffix))
            .cloned()
            .collect()
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("the directory is creatable");
    for entry in std::fs::read_dir(from).expect("the directory is readable") {
        let path = entry.expect("a directory entry").path();
        if path.is_file() {
            std::fs::copy(&path, to.join(path.file_name().expect("a file name")))
                .expect("the file is copyable");
        }
    }
}

/// Replaces the `<data id>` expressions of the document at `path`.
fn rewrite(path: &Path, data: &[(&str, &str)]) {
    let mut text = std::fs::read_to_string(path).expect("the document is readable");
    for (id, expr) in data {
        let start = text
            .find(&format!(r#"<data id="{id}" expr=""#))
            .unwrap_or_else(|| panic!("{} declares <data id=\"{id}\">", path.display()));
        let end = start + text[start..].find("/>").expect("the <data> element closes");
        text.replace_range(start..end, &format!(r#"<data id="{id}" expr="{expr}""#));
    }
    std::fs::write(path, text).expect("the document copy is writable");
}

/// One scratch bot: its documents and the env file pointing both services
/// at the mock.
struct Bot {
    body: PathBuf,
    env: PathBuf,
}

/// Lays out the bot with `edits` (`(path under the scratch root, [(id,
/// expr)])`) and an env file for `decision` and `generation`.
fn layout(
    mock: &Mock,
    decision: Decision,
    generation: Generation,
    edits: &[(&str, &[(&str, &str)])],
) -> Bot {
    let dir = unique_scratch("cognition");
    copy_dir(&workspace().join("bots/floppy"), &dir.join("bots/floppy"));
    copy_dir(
        &workspace().join("bots/floppy/schemas"),
        &dir.join("bots/floppy/schemas"),
    );
    copy_dir(&workspace().join("abilities"), &dir.join("abilities"));
    copy_dir(
        &workspace().join("abilities/schemas"),
        &dir.join("abilities/schemas"),
    );
    for (path, data) in edits {
        rewrite(&dir.join(path), data);
    }
    let env = dir.join("fake.env");
    std::fs::write(
        &env,
        format!(
            "{FAKE_KEYS}FLOPPY_BASE_URL={origin}/{generation}/v1\nTYPESAFE_BASE_URL={origin}/{decision}\n",
            origin = mock.origin,
            generation = generation.path(),
            decision = decision.path(),
        ),
    )
    .expect("the env file is writable");
    Bot {
        body: dir.join("bots/floppy/floppy.scx"),
        env,
    }
}

fn start(bot: &Bot) -> Run {
    let mut run = Run::start(
        &[
            "--env-file".to_owned(),
            bot.env.display().to_string(),
            bot.body.display().to_string(),
        ],
        &[],
    );
    run.event("bot.activate", &json!({}));
    run.until(WAIT, label_is("bot.activated"));
    run
}

fn say(run: &mut Run, text: &str) {
    run.event(
        "bot.input",
        &json!({"observation": {"event": "text", "data": text}}),
    );
}

fn label_is(label: &'static str) -> impl Fn(&Json) -> bool {
    move |record| record["type"] == "log" && record["label"] == label
}

/// Records until a log of each of `labels` has been seen (their order
/// across sessions is not fixed).
fn until_all(run: &Run, labels: &[&str]) -> Vec<Json> {
    let seen = std::cell::RefCell::new(std::collections::BTreeSet::new());
    run.until(WAIT, |record| {
        if record["type"] == "log"
            && let Some(label) = record["label"].as_str()
            && labels.contains(&label)
        {
            seen.borrow_mut().insert(label.to_owned());
        }
        seen.borrow().len() == labels.len()
    })
}

fn turn_ended(record: &Json) -> bool {
    record["type"] == "log"
        && matches!(
            record["label"].as_str(),
            Some("bot.processing.completed" | "bot.processing.failed")
        )
}

/// The texts of the logs labelled `label`, in order.
fn texts(records: &[Json], label: &str) -> Vec<String> {
    logs(records)
        .into_iter()
        .filter(|(l, _)| l == label)
        .map(|(_, text)| text)
        .collect()
}

fn json_text(records: &[Json], label: &str) -> Json {
    let text = texts(records, label)
        .pop()
        .unwrap_or_else(|| panic!("a {label} log: {records:?}"));
    serde_json::from_str(&text).expect("the log text is JSON")
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// Whether a request to `target` is pinned in a trace. A request to a
/// backend that never answers is not: a stage cancelled or timed out
/// before its service sends it (a load-dependent race) never makes it.
/// Nor is a connection closed before its request line (no target), as a
/// cancelled stage's client may leave behind.
fn pinned(target: &str) -> bool {
    !target.is_empty()
        && !target.starts_with(&format!("/{}/", Decision::Hang.path()))
        && !target.starts_with(&format!("/{}/", Generation::Hang.path()))
}

/// Closes the run and asserts its pinned trace, with every pinned request
/// the mock received.
fn close(run: Run, mock: &Mock, test: &str) -> Finished {
    let finished = run.close(WAIT);
    mock.settle(trace::pinned_requests(test));
    let requests: Vec<(String, Json)> = mock
        .received("")
        .into_iter()
        .filter(|request| pinned(&request.target))
        .map(|request| (request.target, request.body))
        .collect();
    trace::assert_trace(test, &finished.all, &requests);
    finished
}

/// The labels of the one decision's selection question, sorted.
fn selection_labels(mock: &Mock) -> Vec<String> {
    let decisions = mock.received("/systemone");
    assert_eq!(decisions.len(), 1, "one decision per turn");
    let mut labels: Vec<String> = decisions[0].body["questions"]["selection"]["criteria"]
        .as_object()
        .expect("the selection's criteria")
        .keys()
        .cloned()
        .collect();
    labels.sort();
    labels
}

/// mosfet's intuition dispatch: the decision chooses from the turn's
/// enabled transitions, not two of its own, and the body routes the chosen
/// event to the ability offering it without asking reasoning.
#[test]
fn intuition_dispatches_an_offered_event_to_its_target() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Activate, Generation::Fail, &[]);
    let mut run = start(&bot);
    say(&mut run, "hello there");
    let records = run.until(WAIT, label_is("communication.activate"));
    assert_eq!(texts(&records, "bot.choices"), strings(&[CHOICES]));
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&[
            "autonomy unhandled (no-behaviors)",
            "intuition handled (none)"
        ])
    );
    assert_eq!(
        json_text(&records, "bot.output"),
        json!({"turn": 1, "output": [{"event": ACTIVATE, "data": {}}]})
    );
    assert_eq!(
        texts(&records, "bot.route"),
        strings(&["bot.ability.communication.activate -> communication"])
    );
    assert_eq!(
        selection_labels(&mock),
        strings(&[IGNORE, ACTIVATE, "none of these"])
    );
    assert!(mock.received("/chat/completions").is_empty());
    close(
        run,
        &mock,
        "intuition_dispatches_an_offered_event_to_its_target",
    );
}

#[test]
fn intuition_handles_an_ignore_without_reasoning() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Ignore, Generation::Fail, &[]);
    let mut run = start(&bot);
    say(&mut run, "background noise");
    let records = run.until(WAIT, label_is("cognition.ignore"));
    assert_eq!(
        json_text(&records, "bot.output"),
        json!({"turn": 1, "output": [{"event": IGNORE, "data": {}}]})
    );
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&[
            "autonomy unhandled (no-behaviors)",
            "intuition handled (none)"
        ])
    );
    assert_eq!(
        texts(&records, "autonomy"),
        strings(&["turn 1: 0 matched for bot.input"])
    );
    assert_eq!(
        texts(&records, "reflection"),
        strings(&["absent (phase 5)"])
    );
    // The body routed the selection to the only ability offering it.
    assert_eq!(
        texts(&records, "bot.route"),
        strings(&["bot.ability.cognition.ignore -> cognition"])
    );
    assert_eq!(texts(&records, "cognition.ignore"), strings(&["idle"]));
    // The decision offered the turn's choices it could fill (respond's
    // text and adopt's name have no grounded value); no model was asked.
    assert_eq!(
        selection_labels(&mock),
        strings(&[IGNORE, ACTIVATE, "none of these"])
    );
    assert!(mock.received("/chat/completions").is_empty());
    let finished = close(run, &mock, "intuition_handles_an_ignore_without_reasoning");
    // The cognition offers ignore from each idle entry (the activation's
    // and the turn's end).
    let mut all = records;
    all.extend(finished.records);
    assert!(
        texts(&all, "cognition.offers").iter().all(|t| t == IGNORE),
        "{all:?}"
    );
}

/// The memory path every inventory test gives the cognition.
const DATABASE: &str = "/memory/floppy.sqlite";

/// One learned behavior row: name, trigger, status, lifetime and source.
struct Learned<'a> {
    name: &'a str,
    trigger: &'a str,
    status: &'a str,
    source: &'a str,
}

/// A turn behavior document (phase 5 plan, "Turn Behavior Contract") that
/// answers `donedata` (an ECMAScript expression over `turn`).
fn turn_behavior(donedata: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="done"><datamodel><data id="turn" expr="0"/><data id="input" expr="null"/></datamodel><final id="done"><donedata><content expr="{donedata}"/></donedata></final></scxml>"#
    )
}

/// XML-escapes `text` for an attribute value.
fn attribute(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Runs `statements` (mosfet `Statement`s) in one transaction of the
/// Memory ability (`abilities/memory.scx`) against the database in
/// `grant`'s directory; returns its output.
fn transact(grant: &[String], statements: &Json) -> Json {
    let document = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="applying">
  <datamodel><data id="statements" expr="{}"/></datamodel>
  <state id="applying">
    <invoke id="memory" type="scxml" src="memory.scx">
      <param name="database" expr="'{DATABASE}'"/>
      <param name="statements" expr="statements"/>
    </invoke>
    <transition event="bot.ability.memory.output" target="done"><log label="output" expr="JSON.stringify(_event.data)"/></transition>
    <transition event="*" target="done"><log label="failed" expr="_event.name"/></transition>
  </state>
  <final id="done"/>
</scxml>"#,
        attribute(&statements.to_string()),
    );
    let dir = unique_scratch("inventory");
    std::fs::copy(
        workspace().join("abilities/memory.scx"),
        dir.join("memory.scx"),
    )
    .expect("the ability is copyable");
    let path = support::write(&dir, "inventory.scx", &document);
    let mut args = grant.to_vec();
    args.extend([
        "--clock".to_owned(),
        "logical".to_owned(),
        path.display().to_string(),
    ]);
    let mut run = Run::start(&args, &[]);
    run.end_input();
    let finished = run.finish(WAIT);
    let output = logs(&finished.all)
        .into_iter()
        .find(|(label, _)| label == "output")
        .unwrap_or_else(|| panic!("the transaction committed: {:?}", finished.all));
    serde_json::from_str(&output.1).expect("the output is JSON")
}

/// A memory directory holding `learned` as mosfet's inventory rows
/// (`bot_behavior`, `bot_behavior_trigger`); returns its `--dir` grant.
fn inventory(learned: &[Learned<'_>]) -> Vec<String> {
    let grant = support::memory_grant();
    let mut statements = Vec::new();
    for row in learned {
        statements.push(json!({
            "sql": "INSERT INTO bot_behavior (name, source, lifetime, status) VALUES (?, ?, 'turn', ?)",
            "parameters": [row.name, row.source, row.status],
        }));
        statements.push(json!({
            "sql": "INSERT INTO bot_behavior_trigger (behavior_name, \"trigger\") VALUES (?, ?)",
            "parameters": [row.name, row.trigger],
        }));
    }
    transact(&grant, &Json::Array(statements));
    grant
}

/// Each behavior's `[name, used_count, failed_count]`, by name.
fn usage(grant: &[String]) -> Json {
    let output = transact(
        grant,
        &json!([{"sql": "SELECT name, used_count, failed_count FROM bot_behavior ORDER BY name"}]),
    );
    Json::Array(
        output["results"][0]["rows"]
            .as_array()
            .expect("the rows")
            .iter()
            .map(|row| row["values"].clone())
            .collect(),
    )
}

/// Starts the bot with the run arguments `args` before the body.
fn start_with(bot: &Bot, args: &[String]) -> Run {
    let mut full = args.to_vec();
    full.extend([
        "--env-file".to_owned(),
        bot.env.display().to_string(),
        bot.body.display().to_string(),
    ]);
    let mut run = Run::start(&full, &[]);
    run.event("bot.activate", &json!({}));
    run.until(WAIT, label_is("bot.activated"));
    run
}

/// mosfet's Autonomy over the inventory: the ACTIVE turn behaviors the
/// stimulus triggers run in name order as candidates (the behavior double,
/// `tests/fixtures/phase4/behavior_double.scx`, is one); a silent one, one
/// that declines and one that cannot run advance, and the first that
/// handles the turn answers it before intuition runs. A DRAFT behavior and
/// one triggered by another event never run; uses and failures are
/// counted.
#[test]
fn autonomy_runs_the_learned_behaviors_the_stimulus_triggers() {
    let mock = Mock::start();
    let silent = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript"><state id="waiting"/></scxml>"#;
    let declines = turn_behavior("({turn: turn, handled: false})");
    let double =
        std::fs::read_to_string(workspace().join("tests/fixtures/phase4/behavior_double.scx"))
            .expect("the double is readable");
    let grant = inventory(&[
        Learned {
            name: "Asilent",
            trigger: "bot.input",
            status: "ACTIVE",
            source: silent,
        },
        Learned {
            name: "Bdeclines",
            trigger: "bot.input",
            status: "ACTIVE",
            source: &declines,
        },
        Learned {
            name: "Cbroken",
            trigger: "bot.input",
            status: "ACTIVE",
            source: "<scxml",
        },
        Learned {
            name: "Ddouble",
            trigger: "bot.input",
            status: "ACTIVE",
            source: &double,
        },
        Learned {
            name: "Edraft",
            trigger: "bot.input",
            status: "DRAFT",
            source: &double,
        },
        Learned {
            name: "Fother",
            trigger: "environment.sound",
            status: "ACTIVE",
            source: &double,
        },
    ]);
    let memory = format!("'{DATABASE}'");
    let bot = layout(
        &mock,
        Decision::Fail,
        Generation::Fail,
        &[("abilities/cognition.scx", &[("database", memory.as_str())])],
    );
    let mut run = start_with(&bot, &grant);
    say(&mut run, "background noise");
    let records = run.until(WAIT, label_is("cognition.ignore"));
    assert_eq!(
        texts(&records, "autonomy"),
        strings(&["turn 1: 4 matched for bot.input"])
    );
    assert_eq!(
        texts(&records, "autonomy.candidate"),
        strings(&["Asilent", "Bdeclines", "Cbroken", "Ddouble"])
    );
    assert_eq!(
        texts(&records, "autonomy.candidate.silent"),
        strings(&["Asilent after 1000 ms"])
    );
    assert_eq!(
        texts(&records, "autonomy.candidate.advance"),
        strings(&["Bdeclines"])
    );
    assert_eq!(texts(&records, "behavior.double"), strings(&["turn 1"]));
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&["autonomy handled"])
    );
    assert_eq!(
        json_text(&records, "bot.output"),
        json!({"turn": 1, "output": [{"event": IGNORE, "data": {}, "reason": "behavior: nothing to do", "target": "cognition"}]})
    );
    // Neither intuition nor reasoning ran: no backend was asked.
    assert!(texts(&records, "intuition").is_empty(), "{records:?}");
    assert!(mock.received("").is_empty());
    close(
        run,
        &mock,
        "autonomy_runs_the_learned_behaviors_the_stimulus_triggers",
    );
    assert_eq!(
        usage(&grant),
        json!([
            ["Asilent", "0", "0"],
            ["Bdeclines", "0", "0"],
            ["Cbroken", "0", "1"],
            ["Ddouble", "1", "0"],
            ["Edraft", "0", "0"],
            ["Fother", "0", "0"]
        ])
    );
}

/// When every triggered behavior declines, autonomy answers unhandled and
/// the cascade goes on to intuition.
#[test]
fn when_no_learned_behavior_handles_the_turn_intuition_runs() {
    let mock = Mock::start();
    let declines = turn_behavior("({turn: turn, handled: false})");
    let grant = inventory(&[Learned {
        name: "Declines",
        trigger: "bot.input",
        status: "ACTIVE",
        source: &declines,
    }]);
    let memory = format!("'{DATABASE}'");
    let bot = layout(
        &mock,
        Decision::Ignore,
        Generation::Fail,
        &[("abilities/cognition.scx", &[("database", memory.as_str())])],
    );
    let mut run = start_with(&bot, &grant);
    say(&mut run, "background noise");
    let records = run.until(WAIT, label_is("cognition.ignore"));
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&["autonomy unhandled (unhandled)", "intuition handled (none)"])
    );
    assert_eq!(
        json_text(&records, "bot.output"),
        json!({"turn": 1, "output": [{"event": IGNORE, "data": {}}]})
    );
    close(
        run,
        &mock,
        "when_no_learned_behavior_handles_the_turn_intuition_runs",
    );
    assert_eq!(usage(&grant), json!([["Declines", "0", "0"]]));
}

#[test]
fn reasoning_selects_from_the_turns_choices() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Ignore, &[]);
    let mut run = start(&bot);
    say(&mut run, "what is the capital of France?");
    let records = run.until(WAIT, label_is("cognition.ignore"));
    assert_eq!(
        json_text(&records, "bot.output"),
        json!({"turn": 1, "output": [{"event": IGNORE, "data": {}, "reason": "nothing to say", "target": "cognition"}]})
    );
    // Reasoning selected from the body's assembled choices (the
    // cognition's offer, tagged), and the body routed the selection back.
    assert_eq!(texts(&records, "bot.choices"), strings(&[CHOICES]));
    assert_eq!(
        texts(&records, "reasoning"),
        strings(&[&format!("turn 1: {CHOICES}")])
    );
    assert_eq!(
        texts(&records, "bot.route"),
        strings(&["bot.ability.cognition.ignore -> cognition"])
    );
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&[
            "autonomy unhandled (no-behaviors)",
            "intuition unhandled (declined)",
            "reasoning handled"
        ])
    );
    // The body gives the cognition a database: reasoning recalls from and
    // retains to memory (no seam logs absent); reflection is absent.
    assert!(texts(&records, "memory").is_empty(), "{records:?}");
    assert_eq!(
        texts(&records, "reflection"),
        strings(&["absent (phase 5)"])
    );
    // The model request: the dispatch schema of the choices, strict, and
    // a prompt that carries the stimulus and what memory recalled (a fresh
    // database holds nothing).
    let requests = mock.received("/chat/completions");
    assert_eq!(requests.len(), 1);
    let body = &requests[0].body;
    let format = &body["response_format"]["json_schema"];
    assert_eq!(format["name"], "dispatch");
    assert_eq!(format["strict"], true);
    let branches = format["schema"]["properties"]["events"]["items"]["anyOf"]
        .as_array()
        .expect("one branch per choice");
    let pinned: Vec<String> = branches
        .iter()
        .map(|branch| {
            assert_eq!(
                branch["required"],
                json!(["event", "data", "reason", "target"]),
                "{branch}"
            );
            assert_eq!(branch["additionalProperties"], false);
            assert!(branch["properties"]["data"].get("examples").is_none());
            let event = branch["properties"]["event"]["enum"][0]
                .as_str()
                .expect("a pinned event")
                .to_owned();
            let target = if event == IGNORE {
                "cognition"
            } else if event == ADOPT {
                "identity"
            } else {
                "communication"
            };
            assert_eq!(branch["properties"]["target"]["enum"], json!([target]));
            event
        })
        .collect();
    assert_eq!(pinned, strings(&[IGNORE, RESPOND, ACTIVATE, ADOPT]));
    let prompt = body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .and_then(|message| message["content"].as_str())
        .expect("the user message");
    let prompt: Json = serde_json::from_str(prompt).expect("the prompt is JSON");
    assert_eq!(
        prompt["stimulus"]["data"]["observation"]["data"],
        "what is the capital of France?"
    );
    let keys: Vec<&String> = prompt.as_object().expect("an object").keys().collect();
    assert_eq!(
        keys,
        [
            "focus",
            "focus_candidates",
            "prior_episodes",
            "standing_directives",
            "stimulus"
        ]
    );
    assert_eq!(prompt["prior_episodes"], json!([]));
    assert_eq!(prompt["standing_directives"], json!([]));
    close(run, &mock, "reasoning_selects_from_the_turns_choices");
}

/// The whole text path on fake backends: "say hi" escalates to reasoning,
/// which selects the communication's activate and respond; the body routes
/// both to the communication, which says the response through speaking
/// and records it in its conversation.
#[test]
fn a_reasoned_response_is_said_through_the_communication() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Respond, &[]);
    let mut run = start(&bot);
    say(&mut run, "say hi");
    let records = until_all(
        &run,
        &["communication.history", "bot.ability.speaking.output"],
    );
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&[
            "autonomy unhandled (no-behaviors)",
            "intuition unhandled (declined)",
            "reasoning handled"
        ])
    );
    assert_eq!(
        texts(&records, "bot.route"),
        strings(&[
            "bot.ability.communication.activate -> communication",
            "bot.ability.communication.respond -> communication"
        ])
    );
    assert_eq!(
        texts(&records, "communication.activate"),
        strings(&["conversation"])
    );
    assert_eq!(texts(&records, "bot.say"), strings(&["Hi! I am floppy."]));
    assert_eq!(
        texts(&records, "bot.ability.speaking.output"),
        strings(&[r#"{"text":"Hi! I am floppy."}"#])
    );
    assert_eq!(texts(&records, "conversation"), strings(&["outbound 0"]));
    assert!(
        texts(&records, "bot.route.dropped").is_empty(),
        "{records:?}"
    );
    close(
        run,
        &mock,
        "a_reasoned_response_is_said_through_the_communication",
    );
}

/// The user message of each model request, parsed.
fn prompts(mock: &Mock) -> Vec<Json> {
    mock.received("/chat/completions")
        .iter()
        .map(|request| {
            let prompt = request.body["messages"]
                .as_array()
                .and_then(|messages| messages.last())
                .and_then(|message| message["content"].as_str())
                .expect("the user message");
            serde_json::from_str(prompt).expect("the prompt is JSON")
        })
        .collect()
}

/// mosfet's Reasoning recall: a reasoned turn retains its episode, and the
/// next turn's prompt carries it.
#[test]
fn a_reasoned_turn_is_recalled_by_the_next() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Ignore, &[]);
    let mut run = start(&bot);
    say(&mut run, "remember this");
    run.until(WAIT, turn_ended);
    say(&mut run, "and this");
    run.until(WAIT, turn_ended);
    let prompts = prompts(&mock);
    assert_eq!(prompts.len(), 2, "{prompts:?}");
    assert_eq!(prompts[0]["prior_episodes"], json!([]));
    assert_eq!(
        prompts[1]["prior_episodes"],
        json!([{
            "focus": null,
            "focus_candidates": [],
            "stimulus_name": "bot.input",
            "output": [{"event": IGNORE, "data": {}, "reason": "nothing to say", "target": "cognition"}],
            "behavior": null
        }])
    );
    close(run, &mock, "a_reasoned_turn_is_recalled_by_the_next");
}

#[test]
fn an_abstention_escalates_to_reasoning() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Ignore, &[]);
    let mut run = start(&bot);
    say(&mut run, "hmm");
    let records = run.until(WAIT, turn_ended);
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&[
            "autonomy unhandled (no-behaviors)",
            "intuition unhandled (declined)",
            "reasoning handled"
        ])
    );
    assert_eq!(texts(&records, "bot.output").len(), 1);
    close(run, &mock, "an_abstention_escalates_to_reasoning");
}

#[test]
fn low_confidence_is_observed_by_the_tuner_and_escalates() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Low, Generation::Ignore, &[]);
    let mut run = start(&bot);
    say(&mut run, "one");
    let first = run.until(WAIT, turn_ended);
    say(&mut run, "two");
    let second = run.until(WAIT, turn_ended);
    for (records, n) in [(&first, 1), (&second, 2)] {
        // During the warmup the threshold is the floor, 35.
        assert_eq!(
            texts(records, "intuition")[0],
            format!("turn {n}: threshold 0.35")
        );
        assert_eq!(
            texts(records, "cognition.stage")[1],
            "intuition unhandled (low_confidence)"
        );
        // The first observation sets the mean; an equal one keeps it, and
        // the variance stays at its floor.
        assert_eq!(
            json_text(records, "cognition.tuner"),
            json!({"mean": 34, "variance": 100, "n": n})
        );
        assert_eq!(texts(records, "bot.output").len(), 1);
    }
    close(
        run,
        &mock,
        "low_confidence_is_observed_by_the_tuner_and_escalates",
    );
}

#[test]
fn after_the_warmup_the_threshold_follows_the_tuner() {
    let mock = Mock::start();
    let bot = layout(
        &mock,
        Decision::Low,
        Generation::Ignore,
        &[(
            "abilities/cognition.scx",
            &[("tuner", "({mean: 70, variance: 100, n: 8})")],
        )],
    );
    let mut run = start(&bot);
    say(&mut run, "one");
    let first = run.until(WAIT, turn_ended);
    // max(35 / 2, 70 - 1 * sqrt(100)) = 60.
    assert_eq!(texts(&first, "intuition")[0], "turn 1: threshold 0.6");
    let tuner = json_text(&first, "cognition.tuner");
    let number = |key: &str| tuner[key].as_f64().expect("a number");
    // delta = 34 - 70; mean = 70 + 0.08 * delta; variance =
    // max(100, 0.92 * 100 + 0.08 * delta^2).
    assert!((number("mean") - 67.12).abs() < 1e-9, "{tuner}");
    assert!((number("variance") - 195.68).abs() < 1e-9, "{tuner}");
    assert_eq!(tuner["n"], 9);
    say(&mut run, "two");
    let second = run.until(WAIT, turn_ended);
    let threshold: f64 = texts(&second, "intuition")[0]
        .strip_prefix("turn 2: threshold ")
        .and_then(|value| value.parse().ok())
        .expect("a threshold");
    let expected = (67.12 - 195.68_f64.sqrt()) / 100.0;
    assert!((threshold - expected).abs() < 1e-9, "{threshold}");
    close(
        run,
        &mock,
        "after_the_warmup_the_threshold_follows_the_tuner",
    );
}

#[test]
fn a_reasoning_failure_fails_the_turn() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Fail, &[]);
    let mut run = start(&bot);
    say(&mut run, "question");
    let records = run.until(WAIT, turn_ended);
    assert_eq!(
        json_text(&records, "bot.processing.failed"),
        json!({"message": "the reasoning failed (status)", "turn": 1})
    );
    assert!(texts(&records, "bot.output").is_empty());
    close(run, &mock, "a_reasoning_failure_fails_the_turn");
}

#[test]
fn a_selection_outside_the_choices_never_becomes_output() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::OffChoice, &[]);
    let mut run = start(&bot);
    say(&mut run, "look at the lamp");
    let records = run.until(WAIT, turn_ended);
    // The generate service holds the reply to the choices' schema first.
    assert_eq!(
        json_text(&records, "bot.processing.failed"),
        json!({"message": "the reasoning failed (schema-mismatch)", "turn": 1})
    );
    assert!(texts(&records, "bot.output").is_empty());
    close(
        run,
        &mock,
        "a_selection_outside_the_choices_never_becomes_output",
    );
}

#[test]
fn an_intuition_failure_fails_the_turn() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Fail, Generation::Ignore, &[]);
    let mut run = start(&bot);
    say(&mut run, "hello");
    let records = run.until(WAIT, turn_ended);
    assert_eq!(
        json_text(&records, "bot.processing.failed"),
        json!({"message": "the intuition failed: the decision failed (status)", "turn": 1})
    );
    assert!(mock.received("/chat/completions").is_empty());
    close(run, &mock, "an_intuition_failure_fails_the_turn");
}

#[test]
fn an_intuition_that_does_not_decide_in_time_fails_the_turn() {
    let mock = Mock::start();
    let bot = layout(
        &mock,
        Decision::Hang,
        Generation::Ignore,
        &[("abilities/intuition.scx", &[("decision_timeout_ms", "300")])],
    );
    let mut run = start(&bot);
    say(&mut run, "hello");
    let records = run.until(WAIT, turn_ended);
    assert_eq!(
        json_text(&records, "bot.processing.failed"),
        json!({"message": "the intuition failed: the decision timed out after 300 ms", "turn": 1})
    );
    close(
        run,
        &mock,
        "an_intuition_that_does_not_decide_in_time_fails_the_turn",
    );
}

/// `stage` never answers; `hop_ms` is long enough for the stages before
/// it to answer.
fn reboots_on_a_silent_stage(
    test: &str,
    decision: Decision,
    generation: Generation,
    stage: &str,
    hop_ms: &str,
) {
    let mock = Mock::start();
    let bot = layout(
        &mock,
        decision,
        generation,
        &[("abilities/cognition.scx", &[("hop_timeout_ms", hop_ms)])],
    );
    let mut run = start(&bot);
    say(&mut run, "hello");
    let records = run.until(WAIT, label_is("bot.activated"));
    assert_eq!(
        texts(&records, "cognition.hop.timed_out"),
        [format!("turn 1: {stage} after {hop_ms} ms")]
    );
    assert_eq!(
        texts(&records, "bot.reboot"),
        strings(&[r#"{"reason":"cognition_child_teardown_failed"}"#])
    );
    assert!(texts(&records, "bot.output").is_empty());
    close(run, &mock, test);
}

#[test]
fn a_silent_intuition_asks_the_body_for_a_reboot() {
    reboots_on_a_silent_stage(
        "a_silent_intuition_asks_the_body_for_a_reboot",
        Decision::Hang,
        Generation::Ignore,
        "intuition",
        "500",
    );
}

#[test]
fn a_silent_reasoning_asks_the_body_for_a_reboot() {
    reboots_on_a_silent_stage(
        "a_silent_reasoning_asks_the_body_for_a_reboot",
        Decision::Abstain,
        Generation::Hang,
        "reasoning",
        "20000",
    );
}

/// The turn times out (`timeout_ms`, long enough for the stages before
/// `stage` to answer) while `stage` waits on its backend.
fn cancels_in(
    test: &str,
    decision: Decision,
    generation: Generation,
    stage: &str,
    timeout_ms: &str,
) {
    let mock = Mock::start();
    let bot = layout(
        &mock,
        decision,
        generation,
        &[(
            "bots/floppy/floppy.scx",
            &[("processing_timeout_ms", timeout_ms)],
        )],
    );
    let mut run = start(&bot);
    say(&mut run, "hello");
    let records = run.until(WAIT, turn_ended);
    assert_eq!(texts(&records, "cognition.cancelled"), strings(&[stage]));
    assert_eq!(
        json_text(&records, "bot.processing.failed"),
        json!({"message": format!("the turn timed out after {timeout_ms} ms and was cancelled"), "turn": 1})
    );
    // The cognition is idle again: it offers, and the bot takes the next
    // turn.
    say(&mut run, "again");
    let records = run.until(WAIT, |record| {
        record["type"] == "log"
            && record["label"] == "cognition"
            && record["text"] == "turn 2: bot.input"
    });
    assert!(texts(&records, "bot.reboot").is_empty());
    if stage == "reasoning" {
        // The second turn's decision request is pinned: wait for its
        // answer before closing.
        run.until(WAIT, |record| {
            record["type"] == "log"
                && record["label"] == "cognition.stage"
                && record["text"]
                    .as_str()
                    .is_some_and(|text| text.starts_with("intuition"))
        });
    }
    close(run, &mock, test);
}

#[test]
fn a_cancel_during_intuition_is_acknowledged() {
    cancels_in(
        "a_cancel_during_intuition_is_acknowledged",
        Decision::Hang,
        Generation::Ignore,
        "intuition",
        "1500",
    );
}

#[test]
fn a_cancel_during_reasoning_is_acknowledged() {
    cancels_in(
        "a_cancel_during_reasoning_is_acknowledged",
        Decision::Abstain,
        Generation::Hang,
        "reasoning",
        "8000",
    );
}

/// The phase 5 seams' gates, on.
const GATES_ON: &[(&str, &str)] = &[("reflection_enabled", "true"), ("routines_enabled", "true")];

/// A body without a database: the seams' gate tests watch the seams, not
/// memory (with one, reflection asks the model; `tests/tests/reflection.rs`
/// covers it).
const NO_DATABASE: &[(&str, &str)] = &[("database", "null")];

/// With both gates on, the routines start (finding no routines), each
/// output is followed by the reflection seam, and a settled reflection
/// asks the routines to reconcile.
#[test]
fn with_its_gates_on_the_cognition_reflects_and_reconciles_the_routines() {
    let mock = Mock::start();
    let bot = layout(
        &mock,
        Decision::Abstain,
        Generation::Ignore,
        &[
            ("abilities/cognition.scx", GATES_ON),
            ("bots/floppy/floppy.scx", NO_DATABASE),
        ],
    );
    let mut run = start(&bot);
    say(&mut run, "hmm");
    let mut records = until_all(&run, &["bot.output", "bot.ability.reflection.output"]);
    if texts(&records, "routines").is_empty() {
        records.extend(run.until(WAIT, label_is("routines")));
    }
    assert_eq!(texts(&records, "reflection"), strings(&["absent"]));
    assert_eq!(
        texts(&records, "bot.ability.reflection.output"),
        strings(&["turn 1"])
    );
    let finished = close(
        run,
        &mock,
        "with_its_gates_on_the_cognition_reflects_and_reconciles_the_routines",
    );
    // Once when the routines start, once for the reconcile.
    assert_eq!(
        texts(&finished.all, "routines"),
        strings(&["absent", "absent"])
    );
}

/// A routine's tick goes from the routines through the cognition to the
/// body, which grants it a turn whose stimulus is the tick.
#[test]
fn a_routine_tick_becomes_a_turn() {
    let mock = Mock::start();
    let bot = layout(
        &mock,
        Decision::Abstain,
        Generation::Ignore,
        &[
            ("abilities/cognition.scx", GATES_ON),
            ("bots/floppy/floppy.scx", NO_DATABASE),
        ],
    );
    std::fs::copy(
        workspace().join("tests/fixtures/phase4/routines_double.scx"),
        bot.body
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .expect("the scratch root")
            .join("abilities/routines.scx"),
    )
    .expect("the double is copyable");
    let mut run = start(&bot);
    say(&mut run, "hmm");
    let first = run.until(WAIT, |record| {
        record["type"] == "log"
            && record["label"] == "cognition"
            && record["text"] == "turn 2: bot.behavior.tick"
    });
    let second = run.until(WAIT, turn_ended);
    assert_eq!(texts(&first, "routines.double"), strings(&["tick"]));
    assert_eq!(texts(&second, "bot.output").len(), 1, "{second:?}");
    close(run, &mock, "a_routine_tick_becomes_a_turn");
}

/// With both gates off (phase 4), nothing is invoked for them: the output
/// is followed only by the reflection's absence.
#[test]
fn with_its_gates_off_the_cognition_invokes_no_seam() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Ignore, &[]);
    let mut run = start(&bot);
    say(&mut run, "hmm");
    run.until(WAIT, turn_ended);
    let finished = close(
        run,
        &mock,
        "with_its_gates_off_the_cognition_invokes_no_seam",
    );
    assert_eq!(
        texts(&finished.all, "reflection"),
        strings(&["absent (phase 5)"])
    );
    assert!(texts(&finished.all, "routines").is_empty());
    assert!(texts(&finished.all, "bot.ability.reflection.output").is_empty());
}
