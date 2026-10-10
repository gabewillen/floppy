//! The floppy bot's cognition cascade (`abilities/cognition/cognition.scx` with its
//! stages `abilities/autonomy/autonomy.scx`, `intuition.scx`, and `reasoning.scx`),
//! run as the body's child through `scx run` on the wall clock.
//!
//! Each test lays out a scratch copy of the repository's shape (the body
//! under `bots/floppy/`, every ability document and schema under
//! `abilities/`), with `<data>` expressions rewritten to shorten bounds, and
//! runs it against one deterministic fake backend on 127.0.0.1 that serves
//! both services (nothing reaches the network): the decision service's
//! `TypeSafe` `system_one` endpoint and the generate service's Chat
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
const ADOPT: &str = "bot.ability.identity.adopt";
/// The turn's choices as the body logs them: the cognition's offer, the
/// communication's and the identity's.
const CHOICES: &str = "bot.ability.cognition.ignore@cognition bot.ability.communication.respond@communication bot.ability.identity.adopt@identity";

/// The decision backend's scripted answer.
#[derive(Clone, Copy)]
enum Decision {
    /// Chooses ignore (0.8).
    Ignore,
    /// Chooses the pass criterion `__unhandled__` (0.8): reasoning handles
    /// the turn.
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
    /// A response for the communication to say.
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

/// The `system_one` answer: the selection question's `chosen` label gets
/// `share`, the rest share what is left; every other question answers its
/// first label.
fn decided(body: &Json, chosen: &str, share: f64) -> Option<String> {
    decided_with(body, chosen, share, None)
}

/// [`decided`], with each other selection label given `other` instead of
/// an even split of what is left.
fn decided_with(body: &Json, chosen: &str, share: f64, other: Option<f64>) -> Option<String> {
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
                } else if let Some(other) = other {
                    other
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

/// Whether `body` is reflection's select call (mosfet
/// `SELECT_INSTRUCTIONS` as its system message).
fn is_reflection(body: &Json) -> bool {
    body["messages"].as_array().is_some_and(|messages| {
        messages.iter().any(|message| {
            message["role"] == "system"
                && message["content"]
                    .as_str()
                    .is_some_and(|text| text.starts_with(REFLECTION_SELECT))
        })
    })
}

/// The start of reflection's select instructions.
const REFLECTION_SELECT: &str = "Select at most one offered behavior inventory event";

/// The scripted answer to one request, or `None` to never answer.
fn answer(target: &str, body: &Json) -> Option<(&'static str, String)> {
    let ok = |text: String| Some(("200 OK", text));
    let fail = || {
        Some((
            "500 Internal Server Error",
            r#"{"error":{"message":"boom"}}"#.to_owned(),
        ))
    };
    let selection = |events: Json| ok(completion(&json!({ "events": events }).to_string()));
    // Reflection's select after each output: an empty selection (no
    // behavior to revise), so reflection stores the turn's episode.
    if is_reflection(body) {
        return selection(json!([]));
    }
    match target {
        "/ignore/v1/systemone" => ok(decided(body, IGNORE, 0.8)?),
        "/abstain/v1/systemone" => ok(decided(body, "__unhandled__", 0.8)?),
        "/low/v1/systemone" => ok(decided_with(body, IGNORE, 0.34, Some(0.33))?),
        "/gignore/v1/chat/completions" => selection(json!([
            {"event": IGNORE, "data": {}, "reason": "nothing to say", "target": "cognition"}
        ])),
        "/grespond/v1/chat/completions" => selection(json!([
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
    // A connection closed before its request line (a cancelled call's,
    // under load) is no request.
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
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
    support::copy_tree(&workspace().join("abilities"), &dir.join("abilities"));
    support::double_speech(&dir.join("abilities"), "'encode'");
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

/// Says `text` to the bot once reflection has settled every earlier
/// output: it runs beside the next turn, so a turn started before it
/// settles interleaves with it at a load-dependent point.
fn say(run: &mut Run, text: &str) {
    settle_reflection(run);
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

/// Whether `record` is the cognition's log of a settled reflection (its
/// output or its failure).
fn reflection_settled(record: &Json) -> bool {
    record["type"] == "log"
        && matches!(
            record["label"].as_str(),
            Some("bot.ability.reflection.output" | "cognition.reflection.failed")
        )
}

/// Waits until reflection has settled every output read so far (a run
/// closed before it settles would cut it short at a load-dependent point).
fn settle_reflection(run: &Run) {
    let outputs = run.count(label_is("floppy.output"));
    while run.count(reflection_settled) < outputs {
        run.until(WAIT, reflection_settled);
    }
}

/// Closes the run and asserts its pinned trace, with every pinned request
/// the mock received.
fn close(run: Run, mock: &Mock, test: &str) -> Finished {
    settle_reflection(&run);
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

/// The generate requests other than reflection's select calls (reasoning's).
fn reasoning_requests(mock: &Mock) -> Vec<Received> {
    mock.received("/chat/completions")
        .into_iter()
        .filter(|request| !is_reflection(&request.body))
        .collect()
}

/// Whether every request the backends received was reflection's select.
fn asked_only_reflection(mock: &Mock) -> bool {
    mock.received("")
        .iter()
        .all(|request| is_reflection(&request.body))
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

#[test]
fn intuition_handles_an_ignore_without_reasoning() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Ignore, Generation::Fail, &[]);
    let mut run = start(&bot);
    say(&mut run, "background noise");
    let records = run.until(WAIT, label_is("bot.processing.completed"));
    assert_eq!(
        json_text(&records, "floppy.output"),
        json!({"turn": 1, "output": [{"event": IGNORE, "data": {}}]})
    );
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&[
            "autonomy unhandled (no-behaviors)",
            "intuition handled (none)"
        ])
    );
    // The body gives the cognition its memory: autonomy looks the
    // stimulus's behaviors up there (a fresh database holds none).
    assert_eq!(
        texts(&records, "autonomy"),
        strings(&["turn 1: 0 matched for bot.input"])
    );
    // An ignore is the turn's terminal product: never dispatched (mosfet
    // without_ignore_selections).
    assert!(texts(&records, "floppy.route").is_empty(), "{records:?}");
    assert!(
        texts(&records, "cognition.ignore").is_empty(),
        "{records:?}"
    );
    // The decision offered the turn's choices it could fill (respond's
    // text and adopt's name have no grounded value); reasoning asked no
    // model.
    assert_eq!(selection_labels(&mock), strings(&["__unhandled__", IGNORE]));
    assert!(reasoning_requests(&mock).is_empty());
    let finished = close(run, &mock, "intuition_handles_an_ignore_without_reasoning");
    // Reflection reflected on the turn after its output (the backend's
    // empty selection) and asked the model once.
    assert_eq!(
        texts(&finished.all, "bot.ability.reflection.output"),
        strings(&["turn 1"])
    );
    assert_eq!(mock.received("/chat/completions").len(), 1);
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
/// Memory ability (`abilities/memory/memory.scx`) against `database` (a file in
/// `grant`'s directory, or a Postgres connection URI); returns its output.
fn transact(grant: &[String], database: &str, statements: &Json) -> Json {
    let document = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="applying">
  <datamodel><data id="statements" expr="{}"/></datamodel>
  <state id="applying">
    <invoke id="memory" type="scxml" src="memory/memory.scx">
      <param name="database" expr="'{database}'"/>
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
    support::copy_ability("memory", &dir);
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

/// `database` holding `learned` as mosfet's inventory rows
/// (`bot_behavior`, `bot_behavior_trigger`), in a fresh memory directory
/// for [`DATABASE`]; returns the run's grant.
fn inventory(database: &str, learned: &[Learned<'_>]) -> Vec<String> {
    let grant = if database == DATABASE {
        support::memory_grant()
    } else {
        support::pgsql_grant()
    };
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
    transact(&grant, database, &Json::Array(statements));
    grant
}

/// Each behavior's `[name, used_count, failed_count]`, by name.
fn usage(grant: &[String], database: &str) -> Json {
    let output = transact(
        grant,
        database,
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
    runs_the_learned_behaviors(
        DATABASE,
        Some("autonomy_runs_the_learned_behaviors_the_stimulus_triggers"),
    );
}

/// The same on the Postgres engine: the inventory and the cognition's
/// database are a `PGlite` server's.
#[test]
#[ignore = "needs a PGlite server: FLOPPY_PGLITE_SERVER"]
fn postgres_autonomy_runs_the_learned_behaviors_the_stimulus_triggers() {
    let server = support::Pglite::start().expect("FLOPPY_PGLITE_SERVER is set");
    runs_the_learned_behaviors(&server.database(), None);
}

/// Closes the run, asserting the pinned trace of `test` when there is one
/// (a Postgres run's trace carries its server's address).
fn close_as(run: Run, mock: &Mock, test: Option<&str>) {
    if let Some(test) = test {
        drop(close(run, mock, test));
    } else {
        settle_reflection(&run);
        drop(run.close(WAIT));
    }
}

/// [`autonomy_runs_the_learned_behaviors_the_stimulus_triggers`] on
/// `database`; `test` names its pinned trace.
fn runs_the_learned_behaviors(database: &str, test: Option<&str>) {
    let mock = Mock::start();
    let silent = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript"><state id="waiting"/></scxml>"#;
    let declines = turn_behavior("({turn: turn, handled: false})");
    let double =
        std::fs::read_to_string(workspace().join("tests/fixtures/phase4/behavior_double.scx"))
            .expect("the double is readable");
    let grant = inventory(
        database,
        &[
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
        ],
    );
    let memory = format!("'{database}'");
    let bot = layout(
        &mock,
        Decision::Fail,
        Generation::Fail,
        &[(
            "abilities/cognition/cognition.scx",
            &[("database", memory.as_str())],
        )],
    );
    support::use_database(&bot.body, database);
    let mut run = start_with(&bot, &grant);
    say(&mut run, "background noise");
    let records = run.until(WAIT, label_is("bot.processing.completed"));
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
        json_text(&records, "floppy.output"),
        json!({"turn": 1, "output": [{"event": IGNORE, "data": {}, "reason": "behavior: nothing to do", "target": "cognition"}]})
    );
    // Neither intuition nor reasoning ran: no backend but reflection's
    // was asked.
    assert!(texts(&records, "intuition").is_empty(), "{records:?}");
    assert!(asked_only_reflection(&mock));
    close_as(run, &mock, test);
    assert_eq!(
        usage(&grant, database),
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

/// mosfet's Autonomy dispatches a handled behavior's selections before it
/// counts the use: one the turn cannot place (here an event nobody offers)
/// marks the behavior failed, not used, and fails the turn with mosfet's
/// SelectionRejectionError message; intuition never runs.
#[test]
fn a_learned_behavior_whose_selection_cannot_be_placed_fails_the_turn() {
    let mock = Mock::start();
    let unoffered = turn_behavior(
        "({turn: turn, handled: true, selected: [{event: 'bot.ability.reasoning.input', data: {}}]})",
    );
    let grant = inventory(
        DATABASE,
        &[Learned {
            name: "Unoffered",
            trigger: "bot.input",
            status: "ACTIVE",
            source: &unoffered,
        }],
    );
    let memory = format!("'{DATABASE}'");
    let bot = layout(
        &mock,
        Decision::Fail,
        Generation::Fail,
        &[(
            "abilities/cognition/cognition.scx",
            &[("database", memory.as_str())],
        )],
    );
    support::use_database(&bot.body, DATABASE);
    let mut run = start_with(&bot, &grant);
    say(&mut run, "background noise");
    let records = run.until(WAIT, turn_ended);
    let message = "Processing selected unavailable event: bot.ability.reasoning.input.";
    assert_eq!(
        texts(&records, "autonomy.candidate.failed"),
        strings(&[&format!("Unoffered ({message})")])
    );
    assert_eq!(
        json_text(&records, "bot.processing.failed"),
        json!({"message": message, "turn": 1})
    );
    assert!(texts(&records, "floppy.output").is_empty(), "{records:?}");
    assert!(texts(&records, "intuition").is_empty(), "{records:?}");
    close(
        run,
        &mock,
        "a_learned_behavior_whose_selection_cannot_be_placed_fails_the_turn",
    );
    assert_eq!(usage(&grant, DATABASE), json!([["Unoffered", "0", "1"]]));
}

/// When every triggered behavior declines, autonomy answers unhandled and
/// the cascade goes on to intuition.
#[test]
fn when_no_learned_behavior_handles_the_turn_intuition_runs() {
    let mock = Mock::start();
    let declines = turn_behavior("({turn: turn, handled: false})");
    let grant = inventory(
        DATABASE,
        &[Learned {
            name: "Declines",
            trigger: "bot.input",
            status: "ACTIVE",
            source: &declines,
        }],
    );
    let memory = format!("'{DATABASE}'");
    let bot = layout(
        &mock,
        Decision::Ignore,
        Generation::Fail,
        &[(
            "abilities/cognition/cognition.scx",
            &[("database", memory.as_str())],
        )],
    );
    let mut run = start_with(&bot, &grant);
    say(&mut run, "background noise");
    let records = run.until(WAIT, label_is("bot.processing.completed"));
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&["autonomy unhandled (unhandled)", "intuition handled (none)"])
    );
    assert_eq!(
        json_text(&records, "floppy.output"),
        json!({"turn": 1, "output": [{"event": IGNORE, "data": {}}]})
    );
    close(
        run,
        &mock,
        "when_no_learned_behavior_handles_the_turn_intuition_runs",
    );
    assert_eq!(usage(&grant, DATABASE), json!([["Declines", "0", "0"]]));
}

#[test]
fn reasoning_selects_from_the_turns_choices() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Ignore, &[]);
    let mut run = start(&bot);
    say(&mut run, "what is the capital of France?");
    let records = run.until(WAIT, label_is("bot.processing.completed"));
    assert_eq!(
        json_text(&records, "floppy.output"),
        json!({"turn": 1, "output": [{"event": IGNORE, "data": {}, "reason": "nothing to say", "target": "cognition"}]})
    );
    // Reasoning selected from the body's assembled choices (the
    // cognition's offer, tagged); an ignore is never dispatched.
    assert_eq!(texts(&records, "floppy.choices"), strings(&[CHOICES]));
    assert_eq!(
        texts(&records, "reasoning"),
        strings(&[&format!("turn 1: {CHOICES}")])
    );
    assert!(texts(&records, "floppy.route").is_empty(), "{records:?}");
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&[
            "autonomy unhandled (no-behaviors)",
            "intuition unhandled (empty_product)",
            "reasoning handled"
        ])
    );
    // The body gives the cognition a database: reasoning recalls from and
    // retains to memory (no seam logs absent).
    assert!(texts(&records, "memory").is_empty(), "{records:?}");
    // The model request: the dispatch schema of the choices, strict, and
    // a prompt that carries the stimulus and what memory recalled (a fresh
    // database holds nothing).
    let requests = reasoning_requests(&mock);
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
    assert_eq!(pinned, strings(&[IGNORE, RESPOND, ADOPT]));
    let prompt = body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .and_then(|message| message["content"].as_str())
        .expect("the user message");
    // mosfet's model_facing_xml of the ProcessorInput: the host input
    // (stimulus and instructions), prior episodes and standing directives,
    // no focus.
    assert!(
        prompt.starts_with("<cognition:processor_input>\n  <host_input>"),
        "{prompt}"
    );
    assert!(
        prompt.contains("what is the capital of France?"),
        "{prompt}"
    );
    assert!(
        prompt.contains("<prior_episodes count=\"0\" />"),
        "{prompt}"
    );
    assert!(
        prompt.contains("<standing_directives count=\"0\" />"),
        "{prompt}"
    );
    assert!(!prompt.contains("focus"), "{prompt}");
    let finished = close(run, &mock, "reasoning_selects_from_the_turns_choices");
    assert_eq!(
        texts(&finished.all, "bot.ability.reflection.output"),
        strings(&["turn 1"])
    );
}

/// The whole text path on fake backends: "say hi" escalates to reasoning,
/// which selects the communication's respond; the body routes it to the
/// communication, which says the response through speaking
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
            "intuition unhandled (empty_product)",
            "reasoning handled"
        ])
    );
    assert_eq!(
        texts(&records, "floppy.route"),
        strings(&["bot.ability.communication.respond -> communication"])
    );
    assert_eq!(
        texts(&records, "floppy.say"),
        strings(&["Hi! I am floppy."])
    );
    assert_eq!(
        texts(&records, "bot.ability.speaking.output"),
        strings(&[
            r#"{"channels":1,"media_type":"audio/pcm","sample_rate_hz":24000,"text":"Hi! I am floppy."}"#
        ])
    );
    assert_eq!(texts(&records, "conversation"), strings(&["outbound 0"]));
    assert!(
        texts(&records, "bot.processing.failed").is_empty(),
        "{records:?}"
    );
    close(
        run,
        &mock,
        "a_reasoned_response_is_said_through_the_communication",
    );
}

/// The user message of each reasoning request (mosfet's model-facing XML).
fn prompts(mock: &Mock) -> Vec<String> {
    reasoning_requests(mock)
        .iter()
        .map(|request| {
            let prompt = request.body["messages"]
                .as_array()
                .and_then(|messages| messages.last())
                .and_then(|message| message["content"].as_str())
                .expect("the user message");
            prompt.to_owned()
        })
        .collect()
}

/// mosfet's Reasoning recall: a reasoned turn retains its episode, and the
/// next turn's prompt carries it.
#[test]
fn a_reasoned_turn_is_recalled_by_the_next() {
    reasoned_turns_recalled(DATABASE, Some("a_reasoned_turn_is_recalled_by_the_next"));
}

/// The same on the Postgres engine: the body's database is a `PGlite`
/// server's, so the cognition's autonomy and reasoning, the identity and
/// the conversation all keep their memory there.
#[test]
#[ignore = "needs a PGlite server: FLOPPY_PGLITE_SERVER"]
fn postgres_a_reasoned_turn_is_recalled_by_the_next() {
    let server = support::Pglite::start().expect("FLOPPY_PGLITE_SERVER is set");
    reasoned_turns_recalled(&server.database(), None);
}

/// [`a_reasoned_turn_is_recalled_by_the_next`] with the body's database
/// `database`; `test` names its pinned trace.
fn reasoned_turns_recalled(database: &str, test: Option<&str>) {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Ignore, &[]);
    support::use_database(&bot.body, database);
    let mut run = start(&bot);
    say(&mut run, "remember this");
    run.until(WAIT, turn_ended);
    // Reflection stores the turn's episode too (mosfet's Reasoning
    // `_retain_activity` and Reflection `_store_activity` both insert
    // one); `say` waits for it, so the next turn's recall is fixed.
    say(&mut run, "and this");
    run.until(WAIT, turn_ended);
    let prompts = prompts(&mock);
    assert_eq!(prompts.len(), 2, "{prompts:?}");
    assert!(
        prompts[0].contains("\n  <prior_episodes count=\"0\" />\n"),
        "{}",
        prompts[0]
    );
    // Reasoning's episode, then reflection's, each a
    // cognition:cognitive_episode (mosfet episodes.CognitiveEpisode).
    let episode = format!(
        "    <cognition:cognitive_episode stimulus_name=\"bot.input\">\n      <focus_candidates count=\"0\" />\n      <output count=\"1\">\n        <cognition:event target=\"cognition\" event=\"{IGNORE}\" reason=\"nothing to say\">\n          <data />\n        </cognition:event>\n      </output>\n    </cognition:cognitive_episode>\n"
    );
    assert!(
        prompts[1].contains(&format!(
            "\n  <prior_episodes count=\"2\">\n{episode}{episode}  </prior_episodes>\n"
        )),
        "{}",
        prompts[1]
    );
    close_as(run, &mock, test);
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
            "intuition unhandled (empty_product)",
            "reasoning handled"
        ])
    );
    assert_eq!(texts(&records, "floppy.output").len(), 1);
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
        // The tuner observes the selections' least confidence first; during
        // the warmup the threshold is then the floor, 35.
        assert_eq!(
            texts(records, "intuition")[0],
            format!("turn {n}: {IGNORE} (confidence 34, threshold 0.35)")
        );
        assert_eq!(
            texts(records, "cognition.stage")[1],
            "intuition unhandled (low_confidence)"
        );
        // The first observation sets the mean; an equal one keeps it, and
        // the variance stays at its floor.
        assert_eq!(
            json_text(records, "cognition.tuner"),
            json!({"mean": 34, "var": 100, "n": n})
        );
        assert_eq!(texts(records, "floppy.output").len(), 1);
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
            "abilities/cognition/cognition.scx",
            &[("tuner", "({mean: 70, var: 100, n: 8})")],
        )],
    );
    let mut run = start(&bot);
    // The threshold a turn's intuition log reports.
    let threshold = |records: &[Json], n: u64| -> f64 {
        texts(records, "intuition")[0]
            .strip_prefix(&format!("turn {n}: {IGNORE} (confidence 34, threshold "))
            .and_then(|rest| rest.strip_suffix(')'))
            .and_then(|value| value.parse().ok())
            .expect("a threshold")
    };
    say(&mut run, "one");
    let first = run.until(WAIT, turn_ended);
    // mosfet observes before it compares: delta = 34 - 70; mean = 70 +
    // 0.08 * delta; variance = max(100, 0.92 * 100 + 0.08 * delta^2); the
    // threshold is then max(35 / 2, mean - sqrt(variance)), about 53.1.
    let tuner = json_text(&first, "cognition.tuner");
    let number = |key: &str| tuner[key].as_f64().expect("a number");
    assert!((number("mean") - 67.12).abs() < 1e-9, "{tuner}");
    assert!((number("var") - 195.68).abs() < 1e-9, "{tuner}");
    assert_eq!(tuner["n"], 9);
    let expected = (67.12 - 195.68_f64.sqrt()) / 100.0;
    assert!((threshold(&first, 1) - expected).abs() < 1e-9);
    assert_eq!(
        texts(&first, "cognition.stage")[1],
        "intuition unhandled (low_confidence)"
    );
    say(&mut run, "two");
    let second = run.until(WAIT, turn_ended);
    let delta: f64 = 34.0 - 67.12;
    let mean = 67.12 + 0.08 * delta;
    let variance = (0.92 * 195.68 + 0.08 * delta * delta).max(100.0);
    let expected = (mean - variance.sqrt()) / 100.0;
    assert!((threshold(&second, 2) - expected).abs() < 1e-9);
    close(
        run,
        &mock,
        "after_the_warmup_the_threshold_follows_the_tuner",
    );
}

/// An ability schema as the body offers it (`$schema`, `title` and
/// `examples` compacted away).
fn offered_schema(event: &str) -> Json {
    let text = std::fs::read(workspace().join(format!("abilities/schemas/{event}.json")))
        .expect("the schema is readable");
    let mut schema: Json = serde_json::from_slice(&text).expect("the schema is JSON");
    let members = schema.as_object_mut().expect("the schema is an object");
    for key in ["$schema", "title", "examples"] {
        members.remove(key);
    }
    schema
}

/// mosfet's `low_confidence` keeps the environment actions: intuition gets
/// every groundable event with its own confidence (ignore 0.34, respond
/// 0.33 with the stimulus's text), observes their least, 33, and escalates
/// under the floor 35 with respond (ignore is no environment action); the
/// cognition routes respond first, then reasoning's selection. Reasoning's
/// prompt is mosfet's model-facing XML of its `ProcessorInput`.
#[test]
fn low_confidence_hands_out_the_environment_actions_before_reasoning() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Low, Generation::Ignore, &[]);
    let abilities = bot
        .body
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("the scratch root")
        .join("abilities");
    let harness = abilities.join("harness.scx");
    std::fs::copy(
        workspace().join("tests/fixtures/phase4/cognition_harness.scx"),
        &harness,
    )
    .expect("the harness is copyable");
    let mut run = Run::start(
        &[
            "--env-file".to_owned(),
            bot.env.display().to_string(),
            harness.display().to_string(),
        ],
        &[],
    );
    run.until(WAIT, label_is("harness.ready"));
    run.event(
        "bot.ability.cognition.input",
        &json!({
            "turn": 1,
            "stimulus": {"event": "bot.input", "data": {"text": "hi"}, "source": "conversation"},
            "focus": null,
            "focus_candidates": [],
            "choices": [
                {"event": IGNORE, "schema": offered_schema(IGNORE), "target": "cognition"},
                {"event": RESPOND, "schema": offered_schema(RESPOND), "target": "communication"},
            ],
        }),
    );
    let records = run.until(WAIT, label_is("harness.output"));
    assert_eq!(
        texts(&records, "intuition"),
        strings(&[&format!(
            "turn 1: {IGNORE} {RESPOND} (confidence 33, threshold 0.35)"
        )])
    );
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&[
            "autonomy unhandled (no-behaviors)",
            "intuition unhandled (low_confidence)",
            "reasoning handled"
        ])
    );
    assert_eq!(
        json_text(&records, "harness.output"),
        json!({"turn": 1, "output": [
            {"event": RESPOND, "data": {"text": "hi"}},
            {"event": IGNORE, "data": {}, "reason": "nothing to say", "target": "cognition"},
        ]})
    );
    // The TypeSafe state is mosfet's turn document, and respond's text is
    // grounded in the stimulus's value.
    let decisions = mock.received("/systemone");
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        decisions[0].body["state"],
        json!({"turn": {"envelope": {"name": "bot.input"}, "data": {"text": "hi"}}})
    );
    assert_eq!(
        decisions[0].body["questions"][format!("{RESPOND}::text")]["criteria"],
        json!({"hi": format!("Which value fills 'text' for {RESPOND}?")})
    );
    let reasoning = reasoning_requests(&mock);
    assert_eq!(reasoning.len(), 1);
    let user = reasoning[0].body["messages"]
        .as_array()
        .and_then(|messages| messages.iter().find(|message| message["role"] == "user"))
        .and_then(|message| message["content"].as_str())
        .expect("a user message");
    assert_eq!(
        user,
        "<cognition:processor_input>\n  <host_input>\n    <processing:input>\n      <input stimulus:event=\"bot.input\" stimulus:source=\"conversation\">\n        <content>\n          <entry key=\"text\" value=\"hi\" />\n        </content>\n      </input>\n    </processing:input>\n  </host_input>\n  <prior_episodes count=\"0\" />\n  <standing_directives count=\"0\" />\n</cognition:processor_input>"
    );
    close(
        run,
        &mock,
        "low_confidence_hands_out_the_environment_actions_before_reasoning",
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
    assert!(texts(&records, "floppy.output").is_empty());
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
    assert!(texts(&records, "floppy.output").is_empty());
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
        json!({"message": "the decision failed (status)", "turn": 1})
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
        &[(
            "abilities/intuition/intuition.scx",
            &[("decision_timeout_ms", "300")],
        )],
    );
    let mut run = start(&bot);
    say(&mut run, "hello");
    let records = run.until(WAIT, turn_ended);
    assert_eq!(
        json_text(&records, "bot.processing.failed"),
        json!({"message": "the decision timed out after 300 ms", "turn": 1})
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
        &[(
            "abilities/cognition/cognition.scx",
            &[("child_operation_timeout_ms", hop_ms)],
        )],
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
    assert!(texts(&records, "floppy.output").is_empty());
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
        json!({"message": format!("Bot processing timed out after {} seconds.", timeout_ms.parse::<f64>().unwrap() / 1000.0), "turn": 1})
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
    // The second turn waits on the same backend, so it times out too:
    // close only after its failure, or its timer races the close (a
    // trailing bot.processing.timed_out under load).
    let records = run.until(WAIT, |record| {
        record["type"] == "log"
            && record["label"] == "bot.processing.failed"
            && record["text"]
                .as_str()
                .is_some_and(|text| text.contains("\"turn\":2"))
    });
    assert_eq!(texts(&records, "cognition.cancelled"), strings(&[stage]));
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

/// A body without a database: the seam tests watch reflection and the
/// routines, not memory (with one, reflection asks the model;
/// `tests/tests/reflection.rs` covers it).
const NO_DATABASE: &[(&str, &str)] = &[("database", "null")];

/// The routines start (finding no routines, without a database), each
/// output is followed by reflection, and a settled reflection asks the
/// routines to reconcile.
#[test]
fn the_cognition_reflects_and_reconciles_the_routines() {
    let mock = Mock::start();
    let bot = layout(
        &mock,
        Decision::Abstain,
        Generation::Ignore,
        &[("bots/floppy/floppy.scx", NO_DATABASE)],
    );
    let mut run = start(&bot);
    say(&mut run, "hmm");
    let mut records = until_all(&run, &["floppy.output", "bot.ability.reflection.output"]);
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
        "the_cognition_reflects_and_reconciles_the_routines",
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
        &[("bots/floppy/floppy.scx", NO_DATABASE)],
    );
    std::fs::copy(
        workspace().join("tests/fixtures/phase4/routines_double.scx"),
        bot.body
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .expect("the scratch root")
            .join("abilities/routines/routines.scx"),
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
    assert_eq!(texts(&second, "floppy.output").len(), 1, "{second:?}");
    close(run, &mock, "a_routine_tick_becomes_a_turn");
}

// ---------------------------------------------------------------------------
// The SpeechHeard seed (abilities/behaviors/speech_heard/speech_heard.scx), run by
// autonomy under `tests/fixtures/phase6/autonomy_harness.scx`, which stands
// in for the cognition: the body cannot take a listening product yet, so
// each stimulus is a double of `bot.ability.listening.speech.output`.

const SPEECH: &str = "bot.ability.listening.speech.output";
const COMMUNICATION_INPUT: &str = "bot.ability.communication.input";

/// A scratch `abilities/` with every ability document, seed and schema,
/// and the harness beside them, its `<data id="database">` set to `database`.
fn autonomy_harness(database: &str) -> PathBuf {
    let dir = unique_scratch("autonomy");
    support::copy_tree(&workspace().join("abilities"), &dir.join("abilities"));
    let harness = dir.join("abilities/harness.scx");
    std::fs::copy(
        workspace().join("tests/fixtures/phase6/autonomy_harness.scx"),
        &harness,
    )
    .expect("the harness is copyable");
    rewrite(&harness, &[("database", database)]);
    harness
}

/// mosfet's `SpeechData` (its interpretation example, decoded) heard from
/// `source_ids`.
fn speech(source_ids: &Json) -> Json {
    json!({
        "content": "what is the weather like?",
        "content_type": "text/plain",
        "voice_detection": {"segments": [{"start_seconds": 0.0, "end_seconds": 0.4, "confidence": 0.9}]},
        "media_type": "audio/pcm",
        "sample_rate_hz": 48000,
        "channels": 1,
        "source_ids": source_ids,
    })
}

/// One turn of autonomy over `stimulus` (event, data): the cognition's
/// input, then its answer.
fn autonomy_turn(run: &mut Run, turn: u64, event: &str, data: &Json) -> Json {
    run.event(
        "bot.ability.cognition.input",
        &json!({
            "turn": turn,
            "stimulus": {"event": event, "data": data, "source": "listening"},
            "focus": null,
            "focus_candidates": [],
        }),
    );
    let records = run.until(WAIT, label_is("harness.output"));
    json_text(&records, "harness.output")
}

/// The selection mosfet's `SpeechHeard` makes: one
/// `bot.ability.communication.input` carrying the speech as `TurnData`.
fn admitted(turn: u64) -> Json {
    json!({
        "turn": turn,
        "handled": true,
        "selected": [{
            "event": COMMUNICATION_INPUT,
            "data": {
                "source_ids": ["caller"],
                "target_ids": [],
                "content": "what is the weather like?",
                "content_type": "text/plain",
                "sample_rate_hz": 48000,
                "channels": 1,
            },
            "reason": "seeded speech admit via communication",
        }],
        "candidates": ["SpeechHeard"],
        "reason": "behavior",
    })
}

/// Whether the schema file `abilities/schemas/<event>.json` admits `data`.
fn admits(event: &str, data: &Json) -> bool {
    let text = std::fs::read(workspace().join(format!("abilities/schemas/{event}.json")))
        .expect("the schema is readable");
    let resolved = scxml_event_schema::resolve(&text, None).expect("the schema resolves");
    let schema =
        scxml_event_schema::CompiledSchema::new(&resolved.schema).expect("the schema compiles");
    let canonical = scxml_event_schema::canonical_json(data);
    let value =
        scxml_types::DataValue::canonical_json_object(&canonical).expect("canonical JSON is data");
    schema.admits(&value)
}

/// mosfet's `SpeechHeard` seed without memory (seed-only autonomy): speech
/// with a source becomes a `bot.ability.communication.input` selection
/// whose data the communication's schema admits; speech with no source is
/// unhandled; a stimulus no seed triggers runs nothing.
#[test]
fn the_speech_heard_seed_admits_identified_speech_as_communication_input() {
    let harness = autonomy_harness("null");
    let mut run = Run::start(&[harness.display().to_string()], &[]);
    let first = autonomy_turn(&mut run, 1, SPEECH, &speech(&json!(["caller"])));
    assert_eq!(first, admitted(1));
    assert!(admits(COMMUNICATION_INPUT, &first["selected"][0]["data"]));
    assert_eq!(
        autonomy_turn(&mut run, 2, SPEECH, &speech(&json!([]))),
        json!({"turn": 2, "handled": false, "candidates": ["SpeechHeard"], "reason": "unhandled"})
    );
    assert_eq!(
        autonomy_turn(
            &mut run,
            3,
            "bot.input",
            &json!({"observation": {"event": "text", "data": "hello"}})
        ),
        json!({"turn": 3, "handled": false, "candidates": [], "reason": "no-behaviors"})
    );
    let finished = run.close(WAIT);
    assert_eq!(
        texts(&finished.all, "autonomy.candidate"),
        strings(&["SpeechHeard", "SpeechHeard"])
    );
    assert_eq!(
        texts(&finished.all, "autonomy.candidate.advance"),
        strings(&["SpeechHeard"])
    );
    trace::assert_trace(
        "the_speech_heard_seed_admits_identified_speech_as_communication_input",
        &finished.all,
        &[],
    );
}

/// mosfet `_prepare_match`: the seeds run before the learned inventory. A
/// learned behavior the speech triggers runs only when the seed does not
/// handle the turn, and only its use is counted.
#[test]
fn autonomy_runs_the_seeds_before_the_learned_behaviors() {
    let double =
        std::fs::read_to_string(workspace().join("tests/fixtures/phase4/behavior_double.scx"))
            .expect("the double is readable");
    let grant = inventory(
        DATABASE,
        &[Learned {
            name: "Double",
            trigger: SPEECH,
            status: "ACTIVE",
            source: &double,
        }],
    );
    let harness = autonomy_harness(&format!("'{DATABASE}'"));
    let mut args = grant.clone();
    args.push(harness.display().to_string());
    let mut run = Run::start(&args, &[]);
    assert_eq!(
        autonomy_turn(&mut run, 1, SPEECH, &speech(&json!(["caller"]))),
        admitted(1)
    );
    assert_eq!(
        autonomy_turn(&mut run, 2, SPEECH, &speech(&json!([]))),
        json!({
            "turn": 2,
            "handled": true,
            "selected": [{"event": IGNORE, "data": {}, "reason": "behavior: nothing to do", "target": "cognition"}],
            "candidates": ["SpeechHeard", "Double"],
            "reason": "behavior",
        })
    );
    let finished = run.close(WAIT);
    assert_eq!(
        texts(&finished.all, "autonomy.candidate"),
        strings(&["SpeechHeard", "SpeechHeard", "Double"])
    );
    assert_eq!(
        texts(&finished.all, "autonomy"),
        strings(&[&format!("turn 2: 1 matched for {SPEECH}")])
    );
    assert_eq!(
        texts(&finished.all, "behavior.double"),
        strings(&["turn 2"])
    );
    trace::assert_trace(
        "autonomy_runs_the_seeds_before_the_learned_behaviors",
        &finished.all,
        &[],
    );
    assert_eq!(usage(&grant, DATABASE), json!([["Double", "1", "0"]]));
}
