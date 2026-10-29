//! The floppy bot's cognition cascade (`abilities/cognition/cognition.scx` with its
//! stages `abilities/autonomy/autonomy.scx`, `intuition.scx` (the decision
//! layer), and `reasoning.scx`),
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
//! `cognition.tuner`, `cognition.cancelled`,
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

use std::fmt::Write as _;
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
const FAKE_KEYS: &str = "FLOPPY_API_KEY=floppy-fake\nTYPESAFE_API_KEY=ts-floppy-fake\nFLOPPY_REFLECTION_API_KEY=floppy-fake\n";
const IGNORE: &str = "bot.ability.cognition.ignore";
const RESPOND: &str = "bot.ability.communication.respond";
/// The deliberative handoff the intuition tier is offered (decision D144).
const HANDOFF: &str = "bot.ability.reasoning.input";
/// An 8 s inactivity bound for reasoning, with the reflex tier (autonomy
/// and the decision layer) given its own 30 s step bound (decision D140), so
/// a loaded machine's slow decision never meets reasoning's bound.
/// The reflex tier's own 30 s step bound alone, for tests that shorten the
/// stage bounds in the cognition's `<data>` to bound reasoning.
const SLOW_REFLEX_BOUNDS: &str = "({reflex_timeout_ms: 30000})";
const STALLED_REASONING_BOUNDS: &str =
    "({child_operation_timeout_ms: 8000, reflex_timeout_ms: 30000})";
/// A model context whose budget (three bytes a token) holds the first
/// turn's prompt, the lesson offer's schema among its choices (decision
/// D149), but not the second's with its recalled episode.
const FITTED_TOKENS: u32 = 4_520;
/// The model a bot composing the intuition tier gives it.
const INTUITION_MODEL: &str = "chat-model";
const ADOPT: &str = "bot.ability.identity.adopt";
/// The lesson the cognition offers (decision D149).
const LEARN: &str = "bot.ability.learning.input";
/// The turn's choices as the body collects them, each `event@target`: the
/// cognition's offers (its ignore and a lesson), the communication's and the identity's, each tagged
/// with its offering session's id.
const CHOICES: &str = "bot.ability.cognition.ignore@session.floppy.cognition bot.ability.learning.input@session.floppy.cognition bot.ability.communication.respond@session.floppy.communication bot.ability.identity.adopt@session.floppy.identity";
/// The cognition's session id in the body, the target of its ignore and
/// its lesson.
const COGNITION: &str = "session.floppy.cognition";

/// The decision backend's scripted answer.
#[derive(Clone, Copy)]
enum Decision {
    /// Chooses ignore (0.8).
    Ignore,
    /// Chooses the pass to reasoning `__reasoning__` (0.8): reasoning
    /// handles the turn.
    Abstain,
    /// Chooses the pass to intuition `__intuition__` (0.8): the chat tier
    /// takes the turn when the bot composes one.
    ToIntuition,
    /// Chooses ignore with 0.34 (each other label 0.33).
    Low,
    /// Chooses respond with 0.34 (each other label 0.33).
    LowRespond,
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
            Self::ToIntuition => "tointuition",
            Self::Low => "low",
            Self::LowRespond => "lowrespond",
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
    /// The communication's respond with the text `hi`: the same action a
    /// low-confidence decision layer hands out for the stimulus `hi`.
    Repeat,
    /// Answers 500.
    Fail,
    /// Never answers.
    Hang,
    /// An ignore selection, streamed slowly but steadily: sixteen pieces a
    /// second apart, sixteen seconds in all ([`support::answer_slowly`]).
    Slow,
    /// An ignore selection whose stream stops after its first piece.
    Stall,
    /// A stream of keep-alive comments only, as a gateway sends while its
    /// model says nothing ([`support::answer_with_pings`]).
    Pings,
    /// The intuition tier's model ([`INTUITION_MODEL`]) says it is
    /// thinking and hands the turn on; any other answers as [`Self::Respond`].
    HandOff,
    /// The intuition tier's model answers 500; any other as [`Self::Respond`].
    IntuitionFails,
    /// A lesson for the learning and a reply that says it will try.
    Learn,
}

impl Generation {
    const fn path(self) -> &'static str {
        match self {
            Self::Slow => "gslow",
            Self::Stall => "gstall",
            Self::Pings => "gpings",
            Self::Ignore => "gignore",
            Self::OffChoice => "offchoice",
            Self::Respond => "grespond",
            Self::Repeat => "grepeat",
            Self::Fail => "gfail",
            Self::Hang => "ghang",
            Self::HandOff => "ghandoff",
            Self::IntuitionFails => "gintuitionfails",
            Self::Learn => "glearn",
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

/// Whether `body` is reflection's select call (its select instructions as
/// the system message).
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

/// Whether `body` is the learning's select call (its instructions as the
/// system message).
fn is_learning(body: &Json) -> bool {
    body["messages"].as_array().is_some_and(|messages| {
        messages.iter().any(|message| {
            message["role"] == "system"
                && message["content"]
                    .as_str()
                    .is_some_and(|text| text.starts_with(LEARNING_SELECT))
        })
    })
}

/// The start of the learning's select instructions.
const LEARNING_SELECT: &str = "You are taught a lesson";

/// The scripted answer to one request, or `None` to never answer.
fn answer(target: &str, body: &Json) -> Option<(&'static str, String)> {
    let ok = |text: String| Some(("200 OK", text));
    let fail = || {
        Some((
            "500 Internal Server Error",
            r#"{"error":{"message":"boom"}}"#.to_owned(),
        ))
    };
    let selection = |events: Json| {
        ok(completion(
            &json!({ "events": support::with_targets(body, &events) }).to_string(),
        ))
    };
    // Reflection's select after each output: an empty selection (no
    // behavior to revise), so reflection stores the turn's episode.
    // The learning's select: an empty selection, so the lesson fails.
    if is_reflection(body) || is_learning(body) {
        return selection(json!([]));
    }
    match target {
        "/ignore/v1/systemone" => ok(decided(body, IGNORE, 0.8)?),
        "/abstain/v1/systemone" => ok(decided(body, "__reasoning__", 0.8)?),
        "/tointuition/v1/systemone" => ok(decided(body, "__intuition__", 0.8)?),
        "/low/v1/systemone" => ok(decided_with(body, IGNORE, 0.34, Some(0.33))?),
        "/lowrespond/v1/systemone" => ok(decided_with(body, RESPOND, 0.34, Some(0.33))?),
        "/gignore/v1/chat/completions" => selection(json!([
            {"event": IGNORE, "data": {}, "reason": "nothing to say"}
        ])),
        "/grespond/v1/chat/completions" => selection(json!([
            {"event": RESPOND, "data": {"parts": [{"words": "Hi! I am floppy."}]}, "reason": "greet"}
        ])),
        "/ghandoff/v1/chat/completions" if body["model"] == INTUITION_MODEL => selection(json!([
            {"event": RESPOND, "data": {"parts": [{"words": "Let me think."}]}, "reason": "hold on"},
            {"event": HANDOFF, "data": {}, "reason": "this needs thought"}
        ])),
        "/gintuitionfails/v1/chat/completions" if body["model"] == INTUITION_MODEL => fail(),
        "/ghandoff/v1/chat/completions" | "/gintuitionfails/v1/chat/completions" => {
            selection(json!([
                {"event": RESPOND, "data": {"parts": [{"words": "Hi! I am floppy."}]}, "reason": "greet"}
            ]))
        }
        "/glearn/v1/chat/completions" => selection(json!([
            {"event": LEARN, "data": {"content": "when I knock, say who is there"}, "reason": "a lesson"},
            {"event": RESPOND, "data": {"parts": [{"words": "I will try."}]}, "reason": "answer"}
        ])),
        "/grepeat/v1/chat/completions" => selection(json!([
            {"event": RESPOND, "data": {"parts": [{"words": "hi"}]}, "reason": "answer"}
        ])),
        "/offchoice/v1/chat/completions" => selection(json!([
            {"event": "bot.focus_device", "data": {"device": "lamp"}, "reason": "look"}
        ])),
        "/dfail/v1/systemone" | "/gfail/v1/chat/completions" => fail(),
        _ => None,
    }
}

/// How long the fake decision service takes to decide.
const DECIDING: Duration = Duration::from_secs(1);

/// How long reasoning takes a turn intuition handed it, so what intuition
/// said is said before reasoning's reply arrives.
const HANDED: Duration = Duration::from_secs(1);

/// How long the backend waits before answering `target`: the decision
/// layer and a handed turn's reasoning answer late, so what runs before
/// them is the same every run.
fn delay(target: &str, body: &Json) {
    // The decision layer answers after the tier that took the turn as it
    // did (not yet reached, decision D275) has asked its model, as a real
    // decision outlasts a local recall, so what that tier does before the
    // decision is the same every run.
    if target.ends_with("/v1/systemone") {
        std::thread::sleep(DECIDING);
    }
    // Reasoning answers a handed turn after what intuition said first is
    // said, so that reply never waits behind it.
    if target == format!("/{}/v1/chat/completions", Generation::HandOff.path())
        && body["model"] != INTUITION_MODEL
        && !is_reflection(body)
    {
        std::thread::sleep(HANDED);
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
    // The embedder (decision D256): a text about color is the vector
    // (1, 0, 0, …), any other (0, 1, 0, …).
    if target == "/embed/v1/embeddings" {
        let dimensions = body["dimensions"].as_u64().unwrap_or(2);
        let data: Vec<Json> = body["input"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let hot = u64::from(!text.as_str().unwrap_or_default().contains("color"));
                let vector: Vec<f64> = (0..dimensions)
                    .map(|component| if component == hot { 1.0 } else { 0.0 })
                    .collect();
                json!({"object": "embedding", "index": index, "embedding": vector})
            })
            .collect();
        let text = json!({"object": "list", "data": data}).to_string();
        log.lock()
            .expect("the log lock")
            .push(Received { target, body });
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
            text.len()
        );
        return;
    }
    if target == format!("/{}/v1/chat/completions", Generation::Pings.path())
        && !is_reflection(&body)
    {
        log.lock().expect("the log lock").push(Received {
            target,
            body: body.clone(),
        });
        support::answer_with_pings(&mut stream, Duration::from_millis(500));
        return;
    }
    for (generation, stall) in [(Generation::Slow, false), (Generation::Stall, true)] {
        if target == format!("/{}/v1/chat/completions", generation.path()) && !is_reflection(&body)
        {
            log.lock().expect("the log lock").push(Received {
                target,
                body: body.clone(),
            });
            let ignore = json!({"events": support::with_targets(&body, &json!([
                {"event": IGNORE, "data": {}, "reason": "nothing to say"}
            ]))});
            support::answer_slowly(
                &mut stream,
                &body,
                &ignore.to_string(),
                16,
                Duration::from_secs(1),
                stall,
            );
            return;
        }
    }
    delay(&target, &body);
    let reply = answer(&target, &body)
        .map(|(status, text)| (status, support::answer_as_asked(&body, text)));
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

/// The scratch root `bot` is laid out in.
fn bot_layout(bot: &Bot) -> &Path {
    bot.body
        .ancestors()
        .nth(3)
        .expect("the body is <layout>/bots/floppy/floppy.scx")
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
        &workspace().join("bots/floppy/firmware"),
        &dir.join("bots/floppy/firmware"),
    );
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
            "{FAKE_KEYS}FLOPPY_BASE_URL={origin}/{generation}/v1\nFLOPPY_REFLECTION_BASE_URL={origin}/{generation}/v1\nTYPESAFE_BASE_URL={origin}/{decision}\n",
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

/// Reasoning that does not ask for a stream itself (its `stream` `<data>`
/// false, decision D262), so the run's `FLOPPY_STREAM` decides.
const NOT_STREAMED: &[(&str, &[(&str, &str)])] =
    &[("abilities/reasoning/reasoning.scx", &[("stream", "false")])];

/// Sets the run's `FLOPPY_STREAM` to `true`: the backend streams, so each
/// model call asks for a streamed reply (decision D101).
fn streaming(bot: &Bot) {
    set_stream(bot, "true");
}

/// Sets the run's `FLOPPY_STREAM` to `value`.
fn set_stream(bot: &Bot, value: &str) {
    let mut text = std::fs::read_to_string(&bot.env).expect("the env file is readable");
    text.push_str("FLOPPY_STREAM=");
    text.push_str(value);
    text.push('\n');
    std::fs::write(&bot.env, text).expect("the env file is writable");
}

fn start(bot: &Bot) -> Run {
    start_activated(bot, &[])
}

/// [`start`] with `params` as the bot's activation settings in the environment
/// that invokes it (decision D107); the bot activates on joining.
fn start_activated(bot: &Bot, params: &[(&str, &str)]) -> Run {
    let run = Run::start(
        &[
            "--env-file".to_owned(),
            bot.env.display().to_string(),
            support::environment_for(&bot.body, params),
        ],
        &[],
    );
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

/// Whether `record` is the cognition's log of a turn it takes.
fn turn_started(record: &Json) -> bool {
    record["type"] == "log" && record["label"] == "cognition"
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

/// Waits until reflection has settled every turn read so far, each output
/// and each failed or cancelled turn (decision D139; a run closed before it
/// settles would cut it short at a load-dependent point).
fn settle_reflection(run: &Run) {
    let outputs = run.count(label_is("floppy.output"))
        + run.count(label_is("cognition.failed"))
        + run.count(label_is("cognition.cancelled"));
    while run.count(reflection_settled) < outputs {
        run.until(WAIT, reflection_settled);
    }
    settle_speech(run);
    SETTLED.with_borrow_mut(|settled| settled.push(run.count(|_| true)));
}

thread_local! {
    /// How many records the run had read each time it settled.
    static SETTLED: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Waits until every utterance said so far has played out and each one
/// completed has reached the interpretation as the bot's own voice (its
/// efference copy), so a run closed after a reply is cut at a fixed point.
fn settle_speech(run: &Run) {
    let said = run.count(label_is("floppy.say"));
    while run.count(label_is("bot.ability.speaking.output")) < said {
        run.until(WAIT, label_is("bot.ability.speaking.output"));
    }
    let completed = run.count(completed_speech);
    while run.count(own_voice_settled) < completed {
        run.until(WAIT, own_voice_settled);
    }
}

/// Whether `record` is speaking's output of an utterance played out whole.
fn completed_speech(record: &Json) -> bool {
    label_is("bot.ability.speaking.output")(record)
        && record["text"]
            .as_str()
            .is_some_and(|text| text.contains(r#""outcome":"completed""#))
}

/// Whether `record` is the interpretation's log that it could not learn the
/// bot's own voice from a copy (no voice backend serves these runs).
fn own_voice_settled(record: &Json) -> bool {
    label_is("interpretation.unidentified")(record)
        && record["text"]
            .as_str()
            .is_some_and(|text| text.contains("the bot's own voice"))
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
    trace::assert_trace(test, &settled_order(&finished.all), &requests);
    finished
}

/// `records` with, in each stretch from a turn's end to where the run
/// settled after it, the reply's records moved after the rest and grouped by
/// session (each session's own records in order): after a turn, reflection,
/// the reply's playout and the interpretation of the bot's own voice run
/// side by side, and which logs first depends on the load.
fn settled_order(records: &[Json]) -> Vec<Json> {
    // The reply's records said while its turn was still in hand (a tier's
    // first words, sent before the turn ended, decision D275) follow the
    // turn's end, as the rest of the reply does.
    let mut ordered = Vec::with_capacity(records.len());
    let mut early = Vec::new();
    for record in records {
        if turn_started(record) {
            ordered.append(&mut early);
            ordered.push(record.clone());
        } else if of_the_reply(record) {
            early.push(record.clone());
        } else if turn_ended(record) {
            ordered.push(record.clone());
            ordered.append(&mut early);
        } else {
            ordered.push(record.clone());
        }
    }
    ordered.append(&mut early);
    for end in SETTLED.take() {
        let end = end.min(ordered.len());
        let Some(start) = ordered[..end].iter().rposition(turn_ended) else {
            continue;
        };
        let (mut reply, rest): (Vec<Json>, Vec<Json>) = ordered[start + 1..end]
            .iter()
            .cloned()
            .partition(of_the_reply);
        reply.sort_by(|a, b| {
            a["session"]
                .as_str()
                .unwrap_or_default()
                .cmp(b["session"].as_str().unwrap_or_default())
        });
        ordered.splice(start + 1..end, rest.into_iter().chain(reply));
    }
    ordered
}

/// Whether `record` is of saying the reply: the communication's, the
/// listening's (the bot's own voice heard back), the short-term memory's
/// (what was said), or the session's speaking events.
fn of_the_reply(record: &Json) -> bool {
    let session = record["session"].as_str().unwrap_or_default();
    session.starts_with("session.floppy.communication")
        || session.starts_with("session.floppy.listening")
        || session == "session.floppy.stm"
        || (session == "session"
            && record["label"].as_str().is_some_and(|label| {
                label == "environment.sound"
                    || label.starts_with("bot.ability.speaking.")
                    || label.starts_with("bot.ability.listening.")
            }))
}

/// The generate requests other than reflection's select calls (reasoning's).
fn reasoning_requests(mock: &Mock) -> Vec<Received> {
    mock.received("/chat/completions")
        .into_iter()
        .filter(|request| !is_reflection(&request.body) && !is_learning(&request.body))
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
fn the_decision_layer_handles_an_ignore_without_reasoning() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Ignore, Generation::Fail, &[]);
    let mut run = start(&bot);
    say(&mut run, "background noise");
    let records = run.until(WAIT, label_is("bot.processing.completed"));
    assert_eq!(
        json_text(&records, "floppy.output"),
        json!({"turn": 1, "output": [{"event": IGNORE, "data": {}, "target": COGNITION}]})
    );
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&[
            "autonomy unhandled (no-behaviors)",
            "decision handled (none)"
        ])
    );
    // The body gives the cognition its memory: autonomy looks the
    // stimulus's behaviors up there (a fresh database holds none).
    assert_eq!(
        texts(&records, "autonomy"),
        strings(&["turn 1: 0 matched for bot.input"])
    );
    // An ignore is the turn's terminal product, the cognition's only (the
    // original's without_ignore_selections): the decision layer sends it to the
    // cognition, its only offerer (the decision service names it), which
    // logs it.
    assert_eq!(texts(&records, "cognition.ignore"), strings(&["turn 1"]));
    // The decision offered the turn's choices it could fill (respond's
    // text and adopt's name have no grounded value). Reasoning took the
    // turn as the decision layer did, not yet reached, and asked its model
    // once (the backend fails it); the decision layer's ignore left it, so
    // it answered nothing and no reasoning stage ran.
    assert_eq!(selection_labels(&mock), strings(&["__reasoning__", IGNORE]));
    assert_eq!(reasoning_requests(&mock).len(), 1);
    let finished = close(
        run,
        &mock,
        "the_decision_layer_handles_an_ignore_without_reasoning",
    );
    // Reflection reflected on the turn after its output (the backend's
    // empty selection) and asked the model once, beside reasoning's one.
    assert_eq!(
        texts(&finished.all, "bot.ability.reflection.output"),
        strings(&["turn 1"])
    );
    assert_eq!(mock.received("/chat/completions").len(), 2);
    // The body read the cognition's two offers, ignore and a lesson, from
    // its live session (decision D242).
    let mut all = records;
    all.extend(finished.records);
    let choices = texts(&all, "floppy.choices");
    assert!(!choices.is_empty(), "{all:?}");
    assert!(
        choices
            .iter()
            .all(|t| support::choice_names(t).contains(&format!("{IGNORE}@"))
                && support::choice_names(t).contains(&format!("{LEARN}@"))),
        "{all:?}"
    );
}

/// The memory path every inventory test gives the cognition.
const DATABASE: &str = "/memory/floppy.sqlite";

/// One learned behavior row: name, trigger, status, and its program's
/// text, which [`inventory`] writes as the builder does (decision D121).
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

/// Runs `statements` (`{sql, parameters}` statements) in one transaction of the
/// Memory ability (`abilities/memory/memory.scx`) against `database` (a file in
/// `grant`'s directory, or a Postgres connection URI); returns its output.
fn transact(grant: &[String], database: &str, statements: &Json) -> Json {
    let document = format!(
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="applying">
  <datamodel><data id="statements" expr="{}"/></datamodel>
  <state id="applying">
    <onentry><send event="started"/></onentry>
    <invoke id="memory" type="scxml" src="memory/memory.scx">
      <param name="database" expr="'{database}'"/>
    </invoke>
    <transition event="started">
      <send event="bot.ability.memory.input" target="#_memory">
        <param name="id" expr="'transact'"/>
        <param name="statements" expr="statements"/>
      </send>
    </transition>
    <transition event="bot.ability.memory.output" target="done"><log label="output" expr="JSON.stringify(_event.data)"/></transition>
    <transition event="*" target="done"><log label="failed" expr="_event.name"/></transition>
  </state>
  <final id="done"/>
</scxml>"##,
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

/// `database` holding `learned` as behavior inventory rows
/// (`bot_behavior`, `bot_behavior_trigger`), in a fresh memory directory
/// for [`DATABASE`]; returns the run's grant. Each row's program is
/// written as `<name>.scx` into the behaviors directory of `layout` (the
/// scratch root holding `abilities/`), and its source is that file's path,
/// `../../behaviors/<name>.scx`, as the builder writes one (decision D121).
fn inventory(database: &str, layout: &Path, learned: &[Learned<'_>]) -> Vec<String> {
    let grant = if database == DATABASE {
        support::memory_grant()
    } else {
        support::pgsql_grant()
    };
    let behaviors = layout.join("behaviors");
    std::fs::create_dir_all(&behaviors).expect("the behaviors directory is creatable");
    let mut statements = Vec::new();
    for row in learned {
        std::fs::write(behaviors.join(format!("{}.scx", row.name)), row.source)
            .expect("the behavior is writable");
        statements.push(json!({
            "sql": "INSERT INTO bot_behavior (name, source, lifetime, status) VALUES (?, ?, 'turn', ?)",
            "parameters": [row.name, format!("../../behaviors/{}.scx", row.name), row.status],
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
        support::environment_for(&bot.body, &[]),
    ]);
    let run = Run::start(&full, &[]);
    run.until(WAIT, label_is("bot.activated"));
    run
}

/// Autonomy over the inventory: the ACTIVE turn behaviors the
/// stimulus triggers run in name order as services (the behavior double,
/// `tests/fixtures/phase4/behavior_double.scx`, is one); a silent one, one
/// that declines and one that cannot run advance, and the first that
/// handles the turn answers it before the decision layer runs. A DRAFT behavior and
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

/// Reasoning names the bot's active learned behaviors in its self block,
/// so the bot knows what it has learned (decision D154), in name order,
/// each description escaped: a draft is not one of them, and each row's
/// `lifetime` is the `'turn'` [`inventory`] writes.
#[test]
fn reasoning_is_told_the_bots_learned_behaviors() {
    let mock = Mock::start();
    let silent = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript"><state id="waiting"/></scxml>"#;
    let bot = layout(
        &mock,
        Decision::Abstain,
        Generation::Ignore,
        &[(
            "bots/floppy/floppy.scx",
            &[("database", format!("'{DATABASE}'").as_str())],
        )],
    );
    let grant = inventory(
        DATABASE,
        bot_layout(&bot),
        &[
            Learned {
                name: "Greeter",
                trigger: "environment.sound",
                status: "ACTIVE",
                source: silent,
            },
            Learned {
                name: "Unbuilt",
                trigger: "environment.sound",
                status: "DRAFT",
                source: silent,
            },
            Learned {
                name: "Answerer",
                trigger: "environment.sound",
                status: "ACTIVE",
                source: silent,
            },
        ],
    );
    transact(
        &grant,
        DATABASE,
        &json!([{
            "sql": "UPDATE bot_behavior SET description = ? WHERE name = ?",
            "parameters": ["Answers <knocks> & rings.", "Answerer"],
        }]),
    );
    support::use_database(&bot.body, DATABASE);
    let mut run = start_with(&bot, &grant);
    say(&mut run, "what can you do?");
    run.until(WAIT, label_is("bot.processing.completed"));
    let requests = reasoning_requests(&mock);
    assert_eq!(requests.len(), 1);
    let system = requests[0].body["messages"][0]["content"]
        .as_str()
        .expect("the system message")
        .to_owned();
    assert!(
        system.contains(r#"<behaviors learned_by="you"><behavior name="Answerer" lifetime="turn">Answers &lt;knocks&gt; &amp; rings.</behavior><behavior name="Greeter" lifetime="turn"></behavior></behaviors>"#),
        "{system}"
    );
    assert!(!system.contains("Unbuilt"), "{system}");
    close(run, &mock, "reasoning_is_told_the_bots_learned_behaviors");
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
    let bot = layout(
        &mock,
        Decision::Fail,
        Generation::Fail,
        &[(
            "bots/floppy/floppy.scx",
            &[("database", format!("'{database}'").as_str())],
        )],
    );
    let grant = inventory(
        database,
        bot_layout(&bot),
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
    support::use_database(&bot.body, database);
    let mut run = start_with(&bot, &grant);
    say(&mut run, "background noise");
    let records = run.until(WAIT, label_is("bot.processing.completed"));
    assert_eq!(
        texts(&records, "autonomy"),
        strings(&["turn 1: 4 matched for bot.input"])
    );
    assert_eq!(
        texts(&records, "autonomy.service"),
        strings(&["Asilent", "Bdeclines", "Cbroken", "Ddouble"])
    );
    assert_eq!(
        texts(&records, "autonomy.service.silent"),
        strings(&["Asilent after 1000 ms"])
    );
    assert_eq!(
        texts(&records, "autonomy.service.advance"),
        strings(&["Bdeclines (handled false)"])
    );
    assert_eq!(texts(&records, "behavior.double"), strings(&["turn 1"]));
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&["autonomy handled"])
    );
    assert_eq!(
        json_text(&records, "floppy.output"),
        json!({"turn": 1, "output": [{"event": IGNORE, "data": {}, "reason": "behavior: ignore", "target": COGNITION}]})
    );
    // Neither the decision layer nor reasoning ran: no backend but reflection's
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

/// Autonomy dispatches a handled behavior's selections before it
/// counts the use: one that cannot be sent (here one that names no target)
/// marks the behavior failed, not used, and fails the turn; the decision layer never
/// runs.
#[test]
fn a_learned_behavior_whose_selection_cannot_be_sent_fails_the_turn() {
    let mock = Mock::start();
    let unoffered = turn_behavior(
        "({turn: turn, handled: true, selected: [{event: 'bot.ability.reasoning.input', data: {}}]})",
    );
    let memory = format!("'{DATABASE}'");
    let bot = layout(
        &mock,
        Decision::Fail,
        Generation::Fail,
        &[("bots/floppy/floppy.scx", &[("database", memory.as_str())])],
    );
    let grant = inventory(
        DATABASE,
        bot_layout(&bot),
        &[Learned {
            name: "Unoffered",
            trigger: "bot.input",
            status: "ACTIVE",
            source: &unoffered,
        }],
    );
    support::use_database(&bot.body, DATABASE);
    let mut run = start_with(&bot, &grant);
    say(&mut run, "background noise");
    let records = until_turn_ended(&run, 1);
    let message = "Autonomy behavior dispatch failed: error.communication (selected bot.ability.reasoning.input with no target)";
    assert_eq!(
        texts(&records, "autonomy.service.failed"),
        strings(&[&format!("Unoffered ({message})")])
    );
    assert_eq!(
        json_text(&records, "bot.processing.failed"),
        json!({"message": message, "turn": 1})
    );
    assert!(texts(&records, "floppy.output").is_empty(), "{records:?}");
    assert!(texts(&records, "intuition").is_empty(), "{records:?}");
    // The handed-back failure of reflex is the next turn's stimulus, which
    // no behavior is triggered by; it skips the decision layer, and
    // reasoning (failing too) ends it without handing anything back.
    let records = until_turn_ended(&run, 2);
    assert_eq!(
        opened(&records, 2).as_deref(),
        Some("bot.ability.cognition.step.failed")
    );
    assert!(texts(&records, "intuition").is_empty(), "{records:?}");
    assert_eq!(
        texts(&records, "cognition.handed_back"),
        strings(&["bounded"])
    );
    close(
        run,
        &mock,
        "a_learned_behavior_whose_selection_cannot_be_sent_fails_the_turn",
    );
    assert_eq!(usage(&grant, DATABASE), json!([["Unoffered", "0", "1"]]));
}

/// When every triggered behavior declines, autonomy answers unhandled and
/// the cascade goes on to the decision layer.
#[test]
fn when_no_learned_behavior_handles_the_turn_the_decision_layer_runs() {
    let mock = Mock::start();
    let declines = turn_behavior("({turn: turn, handled: false})");
    let memory = format!("'{DATABASE}'");
    let bot = layout(
        &mock,
        Decision::Ignore,
        Generation::Fail,
        &[("bots/floppy/floppy.scx", &[("database", memory.as_str())])],
    );
    let grant = inventory(
        DATABASE,
        bot_layout(&bot),
        &[Learned {
            name: "Declines",
            trigger: "bot.input",
            status: "ACTIVE",
            source: &declines,
        }],
    );
    let mut run = start_with(&bot, &grant);
    say(&mut run, "background noise");
    let records = run.until(WAIT, label_is("bot.processing.completed"));
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&["autonomy unhandled (unhandled)", "decision handled (none)"])
    );
    assert_eq!(
        json_text(&records, "floppy.output"),
        json!({"turn": 1, "output": [{"event": IGNORE, "data": {}, "target": COGNITION}]})
    );
    close(
        run,
        &mock,
        "when_no_learned_behavior_handles_the_turn_the_decision_layer_runs",
    );
    assert_eq!(usage(&grant, DATABASE), json!([["Declines", "0", "0"]]));
}

/// The choices logged under `label`, each as `event@target` names.
fn logged_choices(records: &[Json], label: &str) -> Vec<String> {
    texts(records, label)
        .iter()
        .map(|text| support::choice_names(text))
        .collect()
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
        json!({"turn": 1, "output": [{"event": IGNORE, "data": {}, "reason": "nothing to say", "target": COGNITION}]})
    );
    // Reasoning selected from the body's collected choices (the
    // cognition's offer, tagged with its session id); the ignore goes to
    // the cognition, which only logs it.
    assert_eq!(
        logged_choices(&records, "floppy.choices"),
        strings(&[CHOICES])
    );
    assert_eq!(logged_choices(&records, "reasoning"), strings(&[CHOICES]));
    assert_eq!(texts(&records, "cognition.ignore"), strings(&["turn 1"]));
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&[
            "autonomy unhandled (no-behaviors)",
            "decision unhandled (empty_product)",
            "reasoning handled"
        ])
    );
    // The body gives the cognition a database: reasoning recalls from
    // memory (no seam logs absent).
    assert!(texts(&records, "memory").is_empty(), "{records:?}");
    // The model request: the dispatch tool of the choices, and
    // a prompt that carries the stimulus and what memory recalled (a fresh
    // database holds nothing).
    let requests = reasoning_requests(&mock);
    assert_eq!(requests.len(), 1);
    let body = &requests[0].body;
    let format = &body["tools"][0]["function"];
    assert_eq!(format["name"], "dispatch");
    assert!(
        format["description"]
            .as_str()
            .is_some_and(|description| description
                .starts_with("Dispatch all events necessary for the input. Call once.")),
        "{format}"
    );
    assert_eq!(body["tool_choice"], "required");
    let branches = format["parameters"]["properties"]["events"]["items"]["anyOf"]
        .as_array()
        .expect("one branch per choice");
    let pinned: Vec<String> = branches
        .iter()
        .map(|branch| {
            // Each branch: event, data and target required (D231),
            // reason optional.
            assert_eq!(
                branch["required"],
                json!(["event", "data", "target"]),
                "{branch}"
            );
            assert_eq!(branch["additionalProperties"], false);
            assert!(branch["properties"]["data"].get("examples").is_none());
            let event = branch["properties"]["event"]["enum"][0]
                .as_str()
                .expect("a pinned event")
                .to_owned();
            let target = if event == IGNORE || event == LEARN {
                COGNITION
            } else if event == ADOPT {
                "session.floppy.identity"
            } else {
                "session.floppy.communication"
            };
            assert_eq!(branch["properties"]["target"]["enum"], json!([target]));
            event
        })
        .collect();
    assert_eq!(pinned, strings(&[IGNORE, LEARN, RESPOND, ADOPT]));
    let prompt = body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .and_then(|message| message["content"].as_str())
        .expect("the user message");
    // The model-facing content (decision D141): the situation
    // (the environment block), then the stimulus as its own root; nothing
    // recalled yet, and no focus.
    assert!(
        prompt.starts_with("<environment id=\"session\">"),
        "{prompt}"
    );
    assert!(
        prompt.ends_with("</object>") && prompt.contains("what is the capital of France?"),
        "{prompt}"
    );
    assert!(
        !prompt.contains("prior_episodes") && !prompt.contains("standing_directives"),
        "{prompt}"
    );
    assert!(!prompt.contains("focus"), "{prompt}");
    let finished = close(run, &mock, "reasoning_selects_from_the_turns_choices");
    assert_eq!(
        texts(&finished.all, "bot.ability.reflection.output"),
        strings(&["turn 1"])
    );
}

/// A turn that selects a lesson sends it, with the turn it was taught on,
/// to the learning; the lesson's result is the next turn's stimulus, which
/// sends no lesson of its own (decision D149).
#[test]
fn a_lesson_is_learned_and_its_result_perceived() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Learn, &[]);
    let mut run = start(&bot);
    say(&mut run, "when I knock, say who is there");
    // The lesson's result came back as the next turn's stimulus, which
    // selected a lesson again but sent none.
    let records = run.until(WAIT, |record| {
        label_is("cognition.lesson")(record) && record["text"] == "bounded"
    });
    assert_eq!(
        texts(&records, "learning.failed"),
        strings(&["lesson of turn 1: Learning select returned an empty selection."])
    );
    let requests = reasoning_requests(&mock);
    assert_eq!(requests.len(), 2, "{requests:?}");
    let second = requests[1].body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .and_then(|message| message["content"].as_str())
        .expect("the user message")
        .to_owned();
    assert!(second.contains("bot.ability.learning.output"), "{second}");
    let learning: Vec<Received> = mock
        .received("/chat/completions")
        .into_iter()
        .filter(|request| is_learning(&request.body))
        .collect();
    assert_eq!(learning.len(), 1, "one lesson was learned");
    let finished = close(run, &mock, "a_lesson_is_learned_and_its_result_perceived");
    // Reflection only remembered each taught turn: the learning learns
    // the lesson itself.
    let reflected = texts(&finished.all, "reflection");
    assert!(
        reflected.contains(&"turn 1: taught".to_owned()),
        "{reflected:?}"
    );
}

/// The whole text path on fake backends: "say hi" escalates to reasoning,
/// which selects the communication's respond and sends it to the
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
        &["communication.exchange", "bot.ability.speaking.output"],
    );
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&[
            "autonomy unhandled (no-behaviors)",
            "decision unhandled (empty_product)",
            "reasoning handled"
        ])
    );
    assert_eq!(
        texts(&records, "floppy.say"),
        strings(&["Hi! I am floppy."])
    );
    assert_eq!(
        texts(&records, "bot.ability.speaking.output"),
        strings(&[
            r#"{"channels":1,"media_type":"audio/pcm","outcome":"completed","sample_rate_hz":24000,"spoken":"Hi! I am floppy.","text":"Hi! I am floppy.","unspoken":"","voice_id":null}"#
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

/// The user message of each reasoning request (the turn's model-facing XML).
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

/// Tells the bot `content` as gabe, through the communication, once
/// reflection has settled every earlier output (as [`say`]).
fn tell(run: &mut Run, content: &str) {
    settle_reflection(run);
    run.event(
        "bot.ability.communication.input",
        &json!({
            "source_ids": ["gabe"],
            "target_ids": ["floppy"],
            "content": content,
            "content_type": "text/plain"
        }),
    );
}

/// A conversation's messages render into the prompt: a turn the
/// conversation hands reasoning carries its exchange (decision D99), which
/// the next inbound message opens anew (D176), so the next turn's prompt
/// holds the new message, and what was said before reaches it as the one
/// episode reflection stored.
#[test]
fn a_conversation_turn_carries_its_exchange_to_reasoning() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Respond, &[]);
    support::use_database(&bot.body, DATABASE);
    let mut run = start(&bot);
    tell(&mut run, "My favorite color is green.");
    run.until(WAIT, turn_ended);
    settle_reflection(&run);
    tell(&mut run, "What is my favorite color?");
    run.until(WAIT, turn_ended);
    let prompts = prompts(&mock);
    assert_eq!(prompts.len(), 2, "{prompts:?}");
    let said = |text: &str| format!("<entry key=\"content\" value=\"{text}\" />");
    assert!(
        prompts[1].contains(&said("What is my favorite color?")),
        "{}",
        prompts[1]
    );
    for before in ["My favorite color is green.", "Hi! I am floppy."] {
        assert!(
            !prompts[1].contains(&said(before)),
            "{before}: {}",
            prompts[1]
        );
    }
    assert!(
        prompts[1].contains("\n  <prior_episodes count=\"1\">\n"),
        "{}",
        prompts[1]
    );
    close(
        run,
        &mock,
        "a_conversation_turn_carries_its_exchange_to_reasoning",
    );
}

/// Reasoning's recall: a reasoned turn's episode, which
/// reflection stores (decision D97), is the one the next turn's prompt
/// carries: each turn records one episode.
#[test]
fn a_reasoned_turn_is_recalled_by_the_next() {
    reasoned_turns_recalled(DATABASE, Some("a_reasoned_turn_is_recalled_by_the_next"));
}

/// Recall by association (decision D256): with an embedder, reflection
/// stores each episode's words embedded, and reasoning ranks the episodes
/// by their cosine with the turn's cue before their recency, so the
/// question about a color ranks the older episode that told it first.
#[test]
fn an_episode_like_the_cue_is_recalled_first() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Respond, &[]);
    support::use_database(&bot.body, DATABASE);
    let mut text = std::fs::read_to_string(&bot.env).expect("the env file is readable");
    writeln!(text, "FLOPPY_EMBEDDING_BASE_URL={}/embed/v1", mock.origin)
        .expect("the env text is writable");
    std::fs::write(&bot.env, text).expect("the env file is writable");
    let mut run = start(&bot);
    tell(&mut run, "My favorite color is green.");
    run.until(WAIT, turn_ended);
    tell(&mut run, "I bought milk today.");
    run.until(WAIT, turn_ended);
    tell(&mut run, "What is my favorite color?");
    let records = run.until(WAIT, turn_ended);
    // The best three candidates as id:cosine; the told color, the oldest,
    // ranks first.
    let recall = texts(&records, "reasoning.recall");
    let ranked: Vec<(u64, &str)> = recall
        .last()
        .unwrap_or_else(|| panic!("a reasoning.recall log: {records:?}"))
        .split(' ')
        .map(|entry| {
            let (id, cosine) = entry.split_once(':').expect("id:cosine");
            (id.parse().expect("a numeric id"), cosine)
        })
        .collect();
    assert_eq!(
        ranked.iter().map(|(_, cosine)| *cosine).collect::<Vec<_>>(),
        ["1.000", "0.000"],
        "{recall:?}"
    );
    assert!(ranked[0].0 < ranked[1].0, "{recall:?}");
    let asked: Vec<String> = mock
        .received("/embeddings")
        .iter()
        .map(|request| {
            request.body["input"][0]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    assert!(
        asked.contains(&"task: search result | query: What is my favorite color?".to_owned()),
        "{asked:?}"
    );
    assert!(
        asked.contains(&"title: none | text: My favorite color is green.".to_owned()),
        "{asked:?}"
    );
    close(run, &mock, "an_episode_like_the_cue_is_recalled_first");
}

/// Context fit (decision D143): with the bot's `bounds.max_context_tokens`
/// each prompt is fitted to its model's context. The first turn recalls
/// nothing and fits; the second's recalled episode does not fit the same
/// budget and is trimmed, while the stimulus is kept.
#[test]
fn a_prompt_over_its_context_has_its_recalled_memory_trimmed() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Ignore, &[]);
    support::use_database(&bot.body, DATABASE);
    let mut run = start_activated(
        &bot,
        &[(
            "bounds",
            &format!("({{max_context_tokens: {FITTED_TOKENS}}})"),
        )],
    );
    say(&mut run, "remember this");
    let first = run.until(WAIT, turn_ended);
    say(&mut run, "and this");
    let second = run.until(WAIT, turn_ended);
    assert_eq!(texts(&first, "cognition.fit"), ["fit"]);
    assert_eq!(texts(&second, "cognition.fit"), ["cut"]);
    let prompts = prompts(&mock);
    assert_eq!(prompts.len(), 2, "{prompts:?}");
    // The trimmed episode is counted as omitted.
    assert!(
        prompts[1].contains("<prior_episodes count=\"0\" omitted=\"1\" />"),
        "{}",
        prompts[1]
    );
    assert!(
        prompts[1].contains("<entry key=\"data\" value=\"and this\" />"),
        "{}",
        prompts[1]
    );
    drop(run.close(WAIT));
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
    // Reflection stores the turn's episode (reasoning retains none,
    // decision D97); `say` waits
    // for it, so the next turn's recall is fixed.
    say(&mut run, "and this");
    run.until(WAIT, turn_ended);
    let prompts = prompts(&mock);
    assert_eq!(prompts.len(), 2, "{prompts:?}");
    // No episode yet: the prompt recalls nothing (an empty collection
    // renders nothing, decision D141).
    assert!(!prompts[0].contains("prior_episodes"), "{}", prompts[0]);
    // The first turn's one episode, a cognition:cognitive_episode (the
    // original episodes.CognitiveEpisode).
    let episode = format!(
        "    <cognition:cognitive_episode stimulus_name=\"bot.input\">\n      <focus_candidates count=\"0\" />\n      <output count=\"1\">\n        <cognition:event target=\"session.floppy.cognition\" event=\"{IGNORE}\" reason=\"nothing to say\" />\n      </output>\n    </cognition:cognitive_episode>\n"
    );
    assert!(
        prompts[1].contains(&format!(
            "\n  <prior_episodes count=\"1\">\n{episode}  </prior_episodes>\n"
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
            "decision unhandled (empty_product)",
            "reasoning handled"
        ])
    );
    assert_eq!(texts(&records, "floppy.output").len(), 1);
    close(run, &mock, "an_abstention_escalates_to_reasoning");
}

/// Reasoning's system channel is the bot's self block (decision D141):
/// the cognition asks the identity for the bot's name until it is told,
/// and a bot that has taken none says so (the self block).
#[test]
fn reasoning_is_told_the_bots_self() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Respond, &[]);
    let mut run = start(&bot);
    say(&mut run, "hello");
    let records = run.until(WAIT, turn_ended);
    assert_eq!(
        texts(&records, "cognition.stage")
            .last()
            .map(String::as_str),
        Some("reasoning handled")
    );
    let request = &reasoning_requests(&mock)[0].body;
    assert_eq!(request["messages"][0]["role"], "system", "{request}");
    let system = request["messages"][0]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(
        system.contains("<persona>") && system.contains("<no_name_yet"),
        "{system}"
    );
    close(run, &mock, "reasoning_is_told_the_bots_self");
}

/// The persona tells its description as its stored row (`{text, reason,
/// traits, kind}`): the cognition keeps its text, so reasoning's self block
/// shows it and reflection still takes each turn (its input's `self` takes
/// the description as text; a row there was refused, and reflection never
/// heard of a turn again).
#[test]
fn a_revised_description_is_the_bots_and_reflection_still_runs() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Ignore, &[]);
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
    let text = "I keep asking people about their dogs.";
    run.event(
        "bot.ability.persona.output",
        &json!({"traits": ["curious"], "description": {
            "text": text, "reason": "I noticed it.", "traits": ["curious"], "kind": "fact"
        }}),
    );
    run.event(
        "bot.ability.cognition.input",
        &json!({
            "turn": 1,
            "stimulus": {"event": "bot.input", "data": {"parts": [{"words": "hi"}]}, "source": "conversation"},
            "focus": null,
            "focus_candidates": [],
            "choices": [
                {"event": IGNORE, "schema": offered_schema(IGNORE), "target": "session.cognition"},
            ],
        }),
    );
    let records = until_all(&run, &["harness.output", "bot.ability.reflection.output"]);
    assert_eq!(
        texts(&records, "bot.ability.reflection.output"),
        strings(&["turn 1"])
    );
    let system = reasoning_requests(&mock)[0].body["messages"][0]["content"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        system.contains(&format!(
            "<description written_by=\"you\">{text}</description>"
        )),
        "{system}"
    );
    drop(run.close(WAIT));
}

/// The activation settings of a bot that composes the intuition tier.
fn with_intuition() -> [(&'static str, &'static str); 1] {
    [("intuition_model", "'chat-model'")]
}

/// The chat requests the intuition tier sent (its model's).
fn intuition_requests(mock: &Mock) -> Vec<Received> {
    reasoning_requests(mock)
        .into_iter()
        .filter(|request| request.body["model"] == INTUITION_MODEL)
        .collect()
}

/// Whether a chat request's dispatch tool offers the handoff.
fn offers_handoff(request: &Received) -> bool {
    request.body["tools"].to_string().contains(HANDOFF)
}

/// The decision layer's pass names the tier it hands the turn to (its
/// `hand_to`, R-23): with the chat tier composed it is offered both passes,
/// and passing to reasoning skips intuition.
#[test]
fn a_pass_to_reasoning_skips_the_intuition_tier() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Ignore, &[]);
    let mut run = start_activated(&bot, &with_intuition());
    say(&mut run, "hello");
    let records = run.until(WAIT, turn_ended);
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&[
            "autonomy unhandled (no-behaviors)",
            "decision unhandled (empty_product)",
            "reasoning handled"
        ]),
        "{records:?}"
    );
    // Intuition took the turn as the decision layer did, not yet reached,
    // and the pass to reasoning left it: it asked its model at most once,
    // and nothing it selected was sent (no intuition stage ran).
    assert!(intuition_requests(&mock).len() <= 1);
    let decisions = mock.received("/systemone");
    let criteria = &decisions[0].body["questions"]["selection"]["criteria"];
    assert!(criteria.get("__intuition__").is_some(), "{criteria}");
    assert!(criteria.get("__reasoning__").is_some(), "{criteria}");
    drop(run.close(WAIT));
}

/// The intuition tier (decision D144): a bot that composes one answers in
/// words on its chat model, without thinking first, before reasoning is
/// ever asked; it is offered the deliberative handoff, the body never is.
#[test]
fn intuition_answers_in_words_without_reasoning() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::ToIntuition, Generation::Respond, &[]);
    let mut run = start_activated(&bot, &with_intuition());
    say(&mut run, "hello");
    let records = run.until(WAIT, turn_ended);
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&[
            "autonomy unhandled (no-behaviors)",
            "decision unhandled (empty_product)",
            "intuition handled"
        ]),
        "{records:?}"
    );
    let requests = reasoning_requests(&mock);
    assert_eq!(requests.len(), 1, "only intuition is asked");
    assert_eq!(requests[0].body["model"], INTUITION_MODEL);
    assert_eq!(requests[0].body["reasoning_effort"], "none");
    assert!(offers_handoff(&requests[0]));
    assert!(!texts(&records, "floppy.choices").concat().contains(HANDOFF));
    assert_eq!(texts(&records, "floppy.output").len(), 1);
    close(run, &mock, "intuition_answers_in_words_without_reasoning");
}

/// Intuition selecting the handoff hands the turn on to reasoning: what it
/// said first is sent, rides the turn's acted into reasoning's prompt, and
/// leads the turn's output.
#[test]
fn intuition_hands_the_turn_to_reasoning_after_what_it_said() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::ToIntuition, Generation::HandOff, &[]);
    let mut run = start_activated(&bot, &with_intuition());
    say(&mut run, "hello");
    let records = run.until(WAIT, turn_ended);
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&[
            "autonomy unhandled (no-behaviors)",
            "decision unhandled (empty_product)",
            "intuition unhandled (handoff)",
            "reasoning handled"
        ]),
        "{records:?}"
    );
    assert_eq!(intuition_requests(&mock).len(), 1);
    let reasoning: Vec<Received> = reasoning_requests(&mock)
        .into_iter()
        .filter(|request| request.body["model"] != INTUITION_MODEL)
        .collect();
    assert_eq!(reasoning.len(), 1);
    assert!(
        !offers_handoff(&reasoning[0]),
        "reasoning is never offered the handoff"
    );
    let prompt = reasoning[0].body["messages"].to_string();
    assert!(
        prompt.contains("cognition:acted") && prompt.contains("Let me think."),
        "{prompt}"
    );
    let output = &json_text(&records, "floppy.output")["output"];
    assert_eq!(output.as_array().map(Vec::len), Some(2), "{output}");
    assert_eq!(output[0]["data"]["parts"][0]["words"], "Let me think.");
    close(
        run,
        &mock,
        "intuition_hands_the_turn_to_reasoning_after_what_it_said",
    );
}

/// A failing intuition hands the same turn on to reasoning, as a failing
/// decision layer does.
#[test]
fn a_failing_intuition_cascades_to_reasoning() {
    let mock = Mock::start();
    let bot = layout(
        &mock,
        Decision::ToIntuition,
        Generation::IntuitionFails,
        &[],
    );
    let mut run = start_activated(&bot, &with_intuition());
    say(&mut run, "hello");
    let records = run.until(WAIT, turn_ended);
    assert_eq!(
        texts(&records, "cognition.cascaded"),
        strings(&[r#"{"stage":"intuition","reason":"the intuition failed (status)"}"#])
    );
    assert_eq!(
        texts(&records, "cognition.stage")
            .last()
            .map(String::as_str),
        Some("reasoning handled")
    );
    assert!(texts(&records, "cognition.handed_back").is_empty());
    close(run, &mock, "a_failing_intuition_cascades_to_reasoning");
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
        // the warmup the threshold is then the floor, 35, so 34 escalates.
        assert_eq!(
            texts(records, "intuition")[0],
            format!("turn {n}: {IGNORE} (confidence 34, escalate true)")
        );
        assert_eq!(
            texts(records, "cognition.stage")[1],
            "decision unhandled (low_confidence)"
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
        Decision::Ignore,
        Generation::Ignore,
        &[(
            "abilities/cognition/cognition.scx",
            &[("confidence_tuner", "({mean: 95, var: 100, n: 8})")],
        )],
    );
    let mut run = start(&bot);
    say(&mut run, "one");
    let first = run.until(WAIT, turn_ended);
    // Intuition observes before it compares: delta = 80 - 95; mean = 95
    // + 0.08 * delta = 93.8; variance = max(100, 0.92 * 100 + 0.08 *
    // delta^2) = 110; the threshold is then max(35 / 2, mean -
    // sqrt(variance)), about 83.3, so a confidence of 80, well over the
    // warmup's floor of 35, escalates.
    let tuner = json_text(&first, "cognition.tuner");
    let number = |key: &str| tuner[key].as_f64().expect("a number");
    assert!((number("mean") - 93.8).abs() < 1e-9, "{tuner}");
    assert!((number("var") - 110.0).abs() < 1e-9, "{tuner}");
    assert_eq!(tuner["n"], 9);
    assert_eq!(
        texts(&first, "intuition")[0],
        format!("turn 1: {IGNORE} (confidence 80, escalate true)")
    );
    assert_eq!(
        texts(&first, "cognition.stage")[1],
        "decision unhandled (low_confidence)"
    );
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

/// A low-confidence turn keeps the environment actions: the decision layer gets
/// the chosen event with its confidence (respond 0.34 with the stimulus's
/// parts), observes it, 34, and escalates under the floor 35, sending
/// respond before reasoning runs. Reasoning's prompt is the turn's
/// model-facing content.
/// Runs one `hi` turn, offering ignore and respond (its stimulus carries
/// respond's `parts`, the evidence the decision layer grounds respond in),
/// through the cognition harness of `bot` (which offers respond itself) and
/// returns its records up to the harness's output.
fn low_confidence_turn(bot: &Bot) -> (Run, Vec<Json>) {
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
            "stimulus": {"event": "bot.input", "data": {"parts": [{"words": "hi"}]}, "source": "conversation"},
            "focus": null,
            "focus_candidates": [],
            "choices": [
                {"event": IGNORE, "schema": offered_schema(IGNORE), "target": "session.cognition"},
                {"event": RESPOND, "schema": offered_schema(RESPOND), "target": "session"},
            ],
        }),
    );
    let records = run.until(WAIT, label_is("harness.output"));
    (run, records)
}

#[test]
fn low_confidence_hands_out_the_environment_actions_before_reasoning() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::LowRespond, Generation::Ignore, &[]);
    let (run, records) = low_confidence_turn(&bot);
    assert_eq!(
        texts(&records, "intuition"),
        strings(&[&format!("turn 1: {RESPOND} (confidence 34, escalate true)")])
    );
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&[
            "autonomy unhandled (no-behaviors)",
            "decision unhandled (low_confidence)",
            "reasoning handled"
        ])
    );
    // The escalating decision layer sent its environment action (respond's
    // grounded "hi") before reasoning ran; the output is that acted, then
    // reasoning's.
    assert_eq!(
        texts(&records, "harness.sent"),
        strings(&[r#"{"parts":[{"words":"hi"}]}"#])
    );
    assert_eq!(
        json_text(&records, "harness.output"),
        json!({"turn": 1, "output": [
            {"event": RESPOND, "data": {"parts": [{"words": "hi"}]}, "target": "session"},
            {"event": IGNORE, "data": {}, "reason": "nothing to say", "target": "session.cognition"},
        ]})
    );
    // The TypeSafe state is the stimulus's model-facing text, and respond's
    // parts are grounded in the stimulus's value.
    let decisions = mock.received("/systemone");
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        decisions[0].body["state"],
        json!({"turn": "<object stimulus:event=\"bot.input\" stimulus:source=\"conversation\">\n  <content>\n    <entry key=\"parts\">\n      <value count=\"1\">\n        <item>\n          <entry key=\"words\" value=\"hi\" />\n        </item>\n      </value>\n    </entry>\n  </content>\n</object>"})
    );
    assert_eq!(
        decisions[0].body["questions"][format!("{RESPOND}::parts")]["criteria"],
        json!({r#"[{"words":"hi"}]"#: format!("Which value fills \"parts\" for {RESPOND}?")})
    );
    // Reasoning asked twice: once as it took the turn beside the decision
    // layer, not yet told what the decision layer did, and again once the
    // turn reached it with the decision layer's acts (decision D275).
    let reasoning = reasoning_requests(&mock);
    assert_eq!(reasoning.len(), 2);
    let user = reasoning[1].body["messages"]
        .as_array()
        .and_then(|messages| messages.iter().find(|message| message["role"] == "user"))
        .and_then(|message| message["content"].as_str())
        .expect("a user message");
    // Reasoning is told what the decision layer already did, as a fact of
    // the turn's situation (`<cognition:acted>`), before the stimulus.
    assert_eq!(
        user,
        "<cognition:acted>\n  <percepts count=\"1\">\n    <item>0</item>\n  </percepts>\n  <acts count=\"1\">\n    <cognition:event target=\"session\" event=\"bot.ability.communication.respond\">\n      <data>\n        <entry key=\"parts\">\n          <value count=\"1\">\n            <item>\n              <entry key=\"words\" value=\"hi\" />\n            </item>\n          </value>\n        </entry>\n      </data>\n    </cognition:event>\n  </acts>\n</cognition:acted>\n<object stimulus:event=\"bot.input\" stimulus:source=\"conversation\">\n  <content>\n    <entry key=\"parts\">\n      <value count=\"1\">\n        <item>\n          <entry key=\"words\" value=\"hi\" />\n        </item>\n      </value>\n    </entry>\n  </content>\n</object>"
    );
    close(
        run,
        &mock,
        "low_confidence_hands_out_the_environment_actions_before_reasoning",
    );
}

/// Each stage sends its own selections: when reasoning selects the very
/// action the escalated decision layer already sent (respond with `hi`, the
/// decision layer's environment action), it is sent again, after it, as
/// reasoning's own selection; the output is the decision layer's acted,
/// then reasoning's.
#[test]
fn reasoning_sends_its_selections_after_the_environment_actions() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::LowRespond, Generation::Repeat, &[]);
    let (run, records) = low_confidence_turn(&bot);
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&[
            "autonomy unhandled (no-behaviors)",
            "decision unhandled (low_confidence)",
            "reasoning handled"
        ])
    );
    assert_eq!(
        texts(&records, "harness.sent"),
        strings(&[
            r#"{"parts":[{"words":"hi"}]}"#,
            r#"{"parts":[{"words":"hi"}]}"#
        ])
    );
    assert_eq!(
        json_text(&records, "harness.output"),
        json!({"turn": 1, "output": [
            {"event": RESPOND, "data": {"parts": [{"words": "hi"}]}, "target": "session"},
            {"event": RESPOND, "data": {"parts": [{"words": "hi"}]}, "reason": "answer", "target": "session"},
        ]})
    );
    close(
        run,
        &mock,
        "reasoning_sends_its_selections_after_the_environment_actions",
    );
}

#[test]
fn a_reasoning_failure_fails_the_turn() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Fail, &[]);
    let mut run = start(&bot);
    say(&mut run, "question");
    let records = until_turn_ended(&run, 1);
    assert_eq!(
        json_text(&records, "bot.processing.failed"),
        json!({"message": "the reasoning failed (status)", "turn": 1})
    );
    assert!(texts(&records, "floppy.output").is_empty());
    // The failed step is handed back as the next turn's stimulus (the
    // reference _hand_back_step_failure); its reasoning fails the same way,
    // and that turn hands nothing back.
    let records = until_turn_ended(&run, 2);
    assert_eq!(
        opened(&records, 2).as_deref(),
        Some("bot.ability.cognition.step.failed")
    );
    assert_eq!(
        json_text(&records, "bot.processing.failed"),
        json!({"message": "the reasoning failed (status)", "turn": 2})
    );
    assert_eq!(
        texts(&records, "cognition.handed_back"),
        strings(&["bounded"])
    );
    close(run, &mock, "a_reasoning_failure_fails_the_turn");
}

/// A failed turn is remembered (decision D139): reflection stores each
/// failed turn's episode with its outcome and failure, and selects nothing
/// for it.
#[test]
fn failed_turns_are_remembered_with_their_failure() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Fail, &[]);
    support::use_database(&bot.body, DATABASE);
    let grant = support::memory_grant();
    let mut run = start_with(&bot, &grant);
    say(&mut run, "question");
    until_turn_ended(&run, 1);
    until_turn_ended(&run, 2);
    settle_reflection(&run);
    drop(run.close(WAIT));
    assert!(
        !mock
            .received("")
            .iter()
            .any(|request| is_reflection(&request.body)),
        "reflection selected for a failed turn"
    );
    let output = transact(
        &grant,
        DATABASE,
        &json!([{"sql": "SELECT content FROM bot_memory WHERE query_tags = 'cognitive_episode' ORDER BY created_at, memory_id"}]),
    );
    let episodes: Vec<Json> = output["results"][0]["rows"]
        .as_array()
        .expect("the rows")
        .iter()
        .map(|row| {
            serde_json::from_str(row["values"][0].as_str().expect("the content")).expect("JSON")
        })
        .collect();
    let remembered: Vec<(Json, Json, Json)> = episodes
        .iter()
        .map(|e| {
            (
                e["stimulus_name"].clone(),
                e["outcome"].clone(),
                e["failure"].clone(),
            )
        })
        .collect();
    assert_eq!(
        remembered,
        [
            (
                json!("bot.input"),
                json!("failed"),
                json!("the reasoning failed (status)")
            ),
            (
                json!("bot.ability.cognition.step.failed"),
                json!("failed"),
                json!("the reasoning failed (status)")
            ),
        ]
    );
}

#[test]
fn a_selection_outside_the_choices_never_becomes_output() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::OffChoice, &[]);
    let mut run = start(&bot);
    say(&mut run, "look at the lamp");
    let records = until_turn_ended(&run, 1);
    // The generate service holds the reply to the choices' schema first;
    // reasoning asks again once with the rejection as feedback, and the
    // same rejection twice running fails the turn (decision D142), keyed by
    // where the selection first fails and told by its message (D126).
    assert_eq!(texts(&records, "reasoning.repair").len(), 1);
    assert_eq!(
        json_text(&records, "reasoning.repair"),
        json!({"attempt": 1, "key": "schema-mismatch:/events/0/event"})
    );
    assert_eq!(
        json_text(&records, "bot.processing.failed"),
        json!({
            "message": "Selection rejected: /events/0/event: value must be one of \"bot.ability.cognition.ignore\", \"bot.ability.learning.input\", \"bot.ability.communication.respond\", \"bot.ability.identity.adopt\"",
            "turn": 1
        })
    );
    assert!(texts(&records, "floppy.output").is_empty());
    let records = until_turn_ended(&run, 2);
    assert!(texts(&records, "floppy.output").is_empty());
    assert_eq!(
        texts(&records, "cognition.handed_back"),
        strings(&["bounded"])
    );
    close(
        run,
        &mock,
        "a_selection_outside_the_choices_never_becomes_output",
    );
}

/// A failing decision layer hands the same turn on to reasoning: nothing
/// is handed back.
#[test]
fn a_decision_layer_failure_cascades_to_reasoning() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Fail, Generation::Ignore, &[]);
    let mut run = start(&bot);
    say(&mut run, "hello");
    let records = run.until(WAIT, turn_ended);
    assert_eq!(
        texts(&records, "cognition.cascaded"),
        strings(&[r#"{"stage":"reflex","reason":"the decision failed (status)"}"#])
    );
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&["autonomy unhandled (no-behaviors)", "reasoning handled"])
    );
    assert_eq!(
        json_text(&records, "floppy.output"),
        json!({"turn": 1, "output": [{"event": IGNORE, "data": {}, "reason": "nothing to say", "target": COGNITION}]})
    );
    close(run, &mock, "a_decision_layer_failure_cascades_to_reasoning");
}

/// A decision service the run gives no base URL answers `unconfigured`, and the
/// turn goes on to reasoning, which answers it.
#[test]
fn an_unconfigured_decision_layer_cascades_to_reasoning() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Ignore, Generation::Ignore, &[]);
    let env = std::fs::read_to_string(&bot.env).expect("the env file is readable");
    std::fs::write(
        &bot.env,
        env.lines()
            .filter(|line| !line.starts_with("TYPESAFE_"))
            .fold(String::new(), |kept, line| kept + line + "\n"),
    )
    .expect("the env file is writable");
    let mut run = start(&bot);
    say(&mut run, "hello");
    let records = run.until(WAIT, turn_ended);
    assert_eq!(
        texts(&records, "cognition.cascaded"),
        strings(&[r#"{"stage":"reflex","reason":"the decision failed (unconfigured)"}"#]),
        "{records:?}"
    );
    assert_eq!(
        texts(&records, "cognition.stage")
            .last()
            .map(String::as_str),
        Some("reasoning handled")
    );
    assert!(mock.received("/systemone").is_empty());
    close(
        run,
        &mock,
        "an_unconfigured_decision_layer_cascades_to_reasoning",
    );
}

/// The decision layer's own bound (`decision_timeout_ms`) ends the decision
/// as a failure, which cascades.
#[test]
fn a_decision_not_made_in_time_cascades_to_reasoning() {
    let mock = Mock::start();
    let bot = layout(
        &mock,
        Decision::Hang,
        Generation::Ignore,
        &[(
            "abilities/cognition/cognition.scx",
            &[("decision_timeout_ms", "300")],
        )],
    );
    let mut run = start(&bot);
    say(&mut run, "hello");
    let records = run.until(WAIT, turn_ended);
    assert_eq!(
        texts(&records, "cognition.cascaded"),
        strings(&[r#"{"stage":"reflex","reason":"the decision timed out after 300 ms"}"#])
    );
    assert_eq!(
        texts(&records, "cognition.stage")
            .last()
            .map(String::as_str),
        Some("reasoning handled")
    );
    close(
        run,
        &mock,
        "a_decision_not_made_in_time_cascades_to_reasoning",
    );
}

/// Records until turn `turn` ends.
fn until_turn_ended(run: &Run, turn: u64) -> Vec<Json> {
    let mark = format!("\"turn\":{turn}");
    run.until(WAIT, |record| {
        turn_ended(record)
            && record["text"]
                .as_str()
                .is_some_and(|text| text.contains(&mark))
    })
}

/// The stimulus envelope the cognition handed the body back in the turn
/// the logged `cognition` record opens (`turn N: <event>`).
fn opened(records: &[Json], turn: u64) -> Option<String> {
    let prefix = format!("turn {turn}: ");
    texts(records, "cognition")
        .into_iter()
        .find_map(|text| text.strip_prefix(&prefix).map(str::to_owned))
}

/// Reasoning's reply streams (decision D101): a model that keeps answering
/// is never cut off, however long the whole reply takes. Its stream,
/// sixteen seconds in all, outlasts the 8 s bound of a stage without
/// progress (long enough for the earlier stages on a loaded machine), and
/// the turn is reasoned.
#[test]
fn a_reasoning_reply_that_keeps_streaming_outlasts_the_inactivity_bound() {
    let mock = Mock::start();
    let bot = layout(
        &mock,
        Decision::Abstain,
        Generation::Slow,
        &[(
            "abilities/cognition/cognition.scx",
            &[("child_operation_timeout_ms", "8000")],
        )],
    );
    streaming(&bot);
    let mut run = start_activated(&bot, &[("bounds", SLOW_REFLEX_BOUNDS)]);
    say(&mut run, "hello");
    let records = run.until(WAIT, turn_ended);
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&[
            "autonomy unhandled (no-behaviors)",
            "decision unhandled (empty_product)",
            "reasoning handled"
        ]),
        "{records:?}"
    );
    assert!(texts(&records, "cognition.hop.timed_out").is_empty());
    assert!(texts(&records, "bot.reboot").is_empty());
    let request = &reasoning_requests(&mock)[0].body;
    assert_eq!(request["stream"], true, "reasoning streams its reply");
    drop(run.close(WAIT));
}

/// A reply that is not streamed (reasoning leaves it to the run, whose
/// `FLOPPY_STREAM` is unset) reports no progress, so no inactivity bound applies to it: a
/// sixteen-second reply outlasts the 8 s bound and the turn is reasoned,
/// bounded by the stage's backstop alone (decision D101).
#[test]
fn a_reasoning_reply_that_is_not_streamed_is_bounded_by_the_backstop_alone() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Slow, NOT_STREAMED);
    let mut run = start_activated(&bot, &[("bounds", STALLED_REASONING_BOUNDS)]);
    say(&mut run, "hello");
    let records = run.until(WAIT, turn_ended);
    assert_eq!(
        texts(&records, "cognition.stage")
            .last()
            .map(String::as_str),
        Some("reasoning handled"),
        "{records:?}"
    );
    assert!(texts(&records, "cognition.hop.timed_out").is_empty());
    let request = &reasoning_requests(&mock)[0].body;
    assert_eq!(request.get("stream"), None, "the call is not streamed");
    drop(run.close(WAIT));
}

/// A run whose `FLOPPY_STREAM` is `false` asks for a reply that is not
/// streamed, and the backend's head arrives at once while its completion
/// takes sixteen seconds: neither is progress, so the 8 s inactivity bound
/// never arms and the turn is reasoned, bounded by the stage's backstop
/// alone (decision D101).
#[test]
fn a_reasoning_reply_the_run_does_not_stream_is_bounded_by_the_backstop_alone() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Slow, NOT_STREAMED);
    set_stream(&bot, "false");
    let mut run = start_activated(&bot, &[("bounds", STALLED_REASONING_BOUNDS)]);
    say(&mut run, "hello");
    let records = run.until(WAIT, turn_ended);
    assert_eq!(
        texts(&records, "cognition.stage")
            .last()
            .map(String::as_str),
        Some("reasoning handled"),
        "{records:?}"
    );
    assert!(texts(&records, "cognition.hop.timed_out").is_empty());
    let request = &reasoning_requests(&mock)[0].body;
    assert_eq!(
        request["stream"], false,
        "the run says the call is not streamed"
    );
    drop(run.close(WAIT));
}

/// A reasoning reply that stops arriving ends the stage at the inactivity
/// bound (decision D101), here the bot's `bounds` param: the step fails
/// only its turn, and nothing reboots.
#[test]
fn a_reasoning_reply_that_stalls_fails_its_turn_at_the_inactivity_bound() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Stall, &[]);
    streaming(&bot);
    let mut run = start_activated(&bot, &[("bounds", STALLED_REASONING_BOUNDS)]);
    say(&mut run, "hello");
    let records = until_turn_ended(&run, 1);
    assert_eq!(
        texts(&records, "cognition.hop.timed_out"),
        ["turn 1: reasoning after 8000 ms without progress"]
    );
    assert_eq!(
        json_text(&records, "bot.processing.failed"),
        json!({"message": "Cognition's reasoning step took longer than 8 seconds, so the turn failed.", "turn": 1})
    );
    assert!(texts(&records, "bot.reboot").is_empty());
    drop(run.close(WAIT));
}

/// Keep-alive comments are not progress (decision D101): a backend that
/// only keeps its connection alive while its model says nothing ends the
/// stage at the inactivity bound.
#[test]
fn a_reasoning_reply_of_keep_alive_comments_only_fails_its_turn_at_the_inactivity_bound() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Pings, &[]);
    streaming(&bot);
    let mut run = start_activated(&bot, &[("bounds", STALLED_REASONING_BOUNDS)]);
    say(&mut run, "hello");
    let records = until_turn_ended(&run, 1);
    assert_eq!(
        texts(&records, "cognition.hop.timed_out"),
        ["turn 1: reasoning after 8000 ms without progress"]
    );
    drop(run.close(WAIT));
}

/// The backstop (decision D101): a stage that keeps making progress still
/// ends once it has run `child_operation_ceiling_ms` in all.
#[test]
fn a_reasoning_reply_that_streams_past_the_ceiling_fails_its_turn() {
    let mock = Mock::start();
    let bot = layout(
        &mock,
        Decision::Abstain,
        Generation::Slow,
        &[(
            "abilities/cognition/cognition.scx",
            &[
                ("child_operation_timeout_ms", "8000"),
                ("child_operation_ceiling_ms", "10000"),
            ],
        )],
    );
    streaming(&bot);
    let mut run = start_activated(&bot, &[("bounds", SLOW_REFLEX_BOUNDS)]);
    say(&mut run, "hello");
    let records = until_turn_ended(&run, 1);
    assert_eq!(
        texts(&records, "cognition.hop.timed_out"),
        ["turn 1: reasoning after 10000 ms in all"]
    );
    assert!(texts(&records, "bot.reboot").is_empty());
    drop(run.close(WAIT));
}

/// `stage` never answers; `hop_ms` is long enough for the stages before
/// it to answer. The step fails only its turn: nothing reboots, and the
/// failure is handed back to the body as the stimulus of a new turn.
fn fails_its_turn_on_a_silent_stage(
    decision: Decision,
    generation: Generation,
    (stage, tier): (&str, &str),
    hop_ms: &str,
) -> (Run, Mock, Vec<Json>) {
    // A reasoning reply that does not stream (the default) is bounded by the
    // stage's backstop alone; the stages before it by inactivity from their
    // start (decision D101).
    let (bound, timed_out) = if stage == "reasoning" {
        (
            "child_operation_ceiling_ms",
            format!("turn 1: {stage} after {hop_ms} ms in all"),
        )
    } else {
        (
            "child_operation_timeout_ms",
            format!("turn 1: {stage} after {hop_ms} ms without progress"),
        )
    };
    let mock = Mock::start();
    let bot = layout(
        &mock,
        decision,
        generation,
        &[("abilities/cognition/cognition.scx", &[(bound, hop_ms)])],
    );
    let mut run = start(&bot);
    say(&mut run, "hello");
    let records = until_turn_ended(&run, 1);
    assert_eq!(texts(&records, "cognition.hop.timed_out"), [timed_out]);
    let seconds = hop_ms.parse::<f64>().expect("a number") / 1000.0;
    assert_eq!(
        json_text(&records, "bot.processing.failed"),
        json!({"message": format!("Cognition's {tier} step took longer than {seconds} seconds, so the turn failed."), "turn": 1})
    );
    assert!(texts(&records, "floppy.output").is_empty());
    let records = until_turn_ended(&run, 2);
    assert_eq!(
        opened(&records, 2).as_deref(),
        Some("bot.ability.cognition.step.timed_out"),
        "{records:?}"
    );
    assert!(texts(&records, "bot.reboot").is_empty());
    (run, mock, records)
}

/// A decision layer that outlasts the reflex tier's step fails its turn;
/// the handed-back failure of reflex skips the decision layer, so reasoning
/// answers the next turn.
#[test]
fn a_silent_decision_layer_fails_only_its_turn() {
    let test = "a_silent_decision_layer_fails_only_its_turn";
    let (run, mock, records) = fails_its_turn_on_a_silent_stage(
        Decision::Hang,
        Generation::Ignore,
        ("decision", "reflex"),
        "500",
    );
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&["autonomy unhandled (no-behaviors)", "reasoning handled"])
    );
    close(run, &mock, test);
}

/// A tier's own step bound (the bot's `bounds`, decision D140): the reflex
/// tier's, shorter than the decision layer's own `decision_timeout_ms`,
/// fails the turn before the decision can cascade.
#[test]
fn the_reflex_tiers_bound_fails_a_silent_decision_layers_turn() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Hang, Generation::Ignore, &[]);
    let mut run = start_activated(&bot, &[("bounds", "({reflex_timeout_ms: 1500})")]);
    say(&mut run, "hello");
    let records = until_turn_ended(&run, 1);
    assert_eq!(
        texts(&records, "cognition.hop.timed_out"),
        ["turn 1: decision after 1500 ms without progress"]
    );
    assert!(texts(&records, "cognition.cascaded").is_empty());
    assert_eq!(
        json_text(&records, "bot.processing.failed"),
        json!({"message": "Cognition's reflex step took longer than 1.5 seconds, so the turn failed.", "turn": 1})
    );
    drop(run.close(WAIT));
}

/// The reasoning tier's own bound replaces both of the stage's bounds
/// (decision D140): a reply that is not streamed is cut off at it.
#[test]
fn the_reasoning_tiers_bound_fails_a_silent_reasoning_turn() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Hang, &[]);
    let mut run = start_activated(&bot, &[("bounds", "({reasoning_timeout_ms: 1500})")]);
    say(&mut run, "hello");
    let records = until_turn_ended(&run, 1);
    assert_eq!(
        texts(&records, "cognition.hop.timed_out"),
        ["turn 1: reasoning after 1500 ms in all"]
    );
    assert_eq!(
        json_text(&records, "bot.processing.failed"),
        json!({"message": "Cognition's reasoning step took longer than 1.5 seconds, so the turn failed.", "turn": 1})
    );
    drop(run.close(WAIT));
}

/// A reasoning that outlasts its step fails its turn, and the handed-back
/// failure's own turn, failing the same way, hands nothing back (the
/// reference `_hand_back_failure`'s bound).
#[test]
fn a_silent_reasoning_fails_only_its_turn() {
    let test = "a_silent_reasoning_fails_only_its_turn";
    let (run, mock, records) = fails_its_turn_on_a_silent_stage(
        Decision::Abstain,
        Generation::Hang,
        ("reasoning", "reasoning"),
        "20000",
    );
    assert_eq!(
        texts(&records, "cognition.handed_back"),
        strings(&["bounded"])
    );
    close(run, &mock, test);
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
fn a_cancel_during_the_decision_layer_is_acknowledged() {
    cancels_in(
        "a_cancel_during_the_decision_layer_is_acknowledged",
        Decision::Hang,
        Generation::Ignore,
        "decision",
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

/// A decoded speech product (`what is the weather like?`) heard from
/// `source_ids`.
fn speech(source_ids: &Json) -> Json {
    json!({
        "content": "what is the weather like?",
        "content_type": "text/plain",
        "voice_detection": {"segments": [{"start_seconds": 0, "end_seconds": 0.4, "confidence": 0.9}]},
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

/// The selection the speech-heard seed makes: one
/// `bot.ability.communication.input` carrying the speech as its turn, its
/// parent the speech (mosfet.py's `TurnData.parent`).
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
                "parent": {"event": SPEECH, "data": speech(&json!(["caller"]))},
            },
            "reason": "seeded speech admit via communication",
        }],
        "services": ["SpeechHeard"],
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

/// The speech-heard seed without memory (seed-only autonomy): speech
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
        json!({"turn": 2, "handled": false, "services": ["SpeechHeard"], "reason": "unhandled"})
    );
    assert_eq!(
        autonomy_turn(
            &mut run,
            3,
            "bot.input",
            &json!({"observation": {"event": "text", "data": "hello"}})
        ),
        json!({"turn": 3, "handled": false, "services": [], "reason": "no-behaviors"})
    );
    let finished = run.close(WAIT);
    assert_eq!(
        texts(&finished.all, "autonomy.service"),
        strings(&["SpeechHeard", "SpeechHeard"])
    );
    assert_eq!(
        texts(&finished.all, "autonomy.service.advance"),
        strings(&["SpeechHeard (handled false)"])
    );
    trace::assert_trace(
        "the_speech_heard_seed_admits_identified_speech_as_communication_input",
        &finished.all,
        &[],
    );
}

/// The seeds run before the learned inventory. A
/// learned behavior the speech triggers runs only when the seed does not
/// handle the turn, and only its use is counted.
#[test]
fn autonomy_runs_the_seeds_before_the_learned_behaviors() {
    let double =
        std::fs::read_to_string(workspace().join("tests/fixtures/phase4/behavior_double.scx"))
            .expect("the double is readable")
            // The harness stands in for the cognition: the double's
            // selection goes to it.
            .replace("'session.floppy.cognition'", "'session'");
    let harness = autonomy_harness(&format!("'{DATABASE}'"));
    let grant = inventory(
        DATABASE,
        harness
            .parent()
            .and_then(Path::parent)
            .expect("the harness is <layout>/abilities/harness.scx"),
        &[Learned {
            name: "Double",
            trigger: SPEECH,
            status: "ACTIVE",
            source: &double,
        }],
    );
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
            "selected": [{"event": IGNORE, "data": {}, "reason": "behavior: ignore", "target": "session"}],
            "services": ["SpeechHeard", "Double"],
            "reason": "behavior",
        })
    );
    let finished = run.close(WAIT);
    assert_eq!(
        texts(&finished.all, "autonomy.service"),
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

/// Reasoning's prompt carries the environment block (decision D106): the
/// structured environment the body sends, rendered by its prompt renderer
/// (D99) as XML elements, values escaped. The bot holds the lamp the
/// environment runs.
#[test]
fn reasonings_prompt_carries_the_environment_block() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Ignore, &[]);
    let layout_dir = bot
        .body
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("the layout's root")
        .to_path_buf();
    let environment = support::environment_with_devices(
        &layout_dir,
        &["floppy"],
        &["lamp"],
        &[("floppy", &["lamp"])],
        &[],
    );
    let lamp = layout_dir.join("environments/environment/lamp/lamp.scx");
    let text = std::fs::read_to_string(&lamp).expect("the lamp copy is readable");
    std::fs::write(
        &lamp,
        text.replace(
            r#"<data id="click_db" expr="60"/>"#,
            r#"<data id="click_db" expr="60"/><data id="label" expr="'desk &lt;lamp&gt; &amp; co'"/>"#,
        ),
    )
    .expect("the lamp copy is writable");
    let mut run = Run::start(
        &[
            "--env-file".to_owned(),
            bot.env.display().to_string(),
            environment.display().to_string(),
        ],
        &[],
    );
    run.until(WAIT, label_is("bot.activated"));
    say(&mut run, "what is the capital of France?");
    run.until(WAIT, label_is("bot.processing.completed"));
    settle_reflection(&run);
    drop(run.close(WAIT));
    let prompts = prompts(&mock);
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    let prompt = &prompts[0];
    // The block's nesting (decision D106): <environment id><self
    // state><owned_devices><device ref id state> with the device's
    // attributes.
    let start = prompt.find("<environment ").expect("the environment block");
    let block = &prompt[start..];
    let block = &block[..block.find("</environment>").expect("its end") + "</environment>".len()];
    assert!(
        block.starts_with(r#"<environment id="session">"#),
        "{block}"
    );
    let lines: Vec<&str> = block.lines().map(str::trim).collect();
    assert_eq!(lines[1], r#"<self state="processing">"#, "{block}");
    // The bot's own attributes come first (decision D156).
    assert_eq!(lines[2], "<held_percepts>0</held_percepts>", "{block}");
    assert_eq!(lines[3], "<owned_devices>", "{block}");
    // ref, id and state are the device's only XML attributes; each of its
    // snapshot attributes is a child element, a string as its escaped
    // text, a number as its JSON text.
    assert_eq!(
        lines[4], r#"<device ref="lamp" id="session.lamp" state="off">"#,
        "{block}"
    );
    for element in [
        "<brightness>0</brightness>",
        "<click_db>60</click_db>",
        "<label>desk &lt;lamp&gt; &amp; co</label>",
    ] {
        assert!(lines.contains(&element), "{element} in {block}");
    }
    assert!(!block.contains("<entry"), "{block}");
}

/// Starts the bot with the run arguments `args` before the body and
/// `params` as its activation settings.
fn start_with_activated(bot: &Bot, args: &[String], params: &[(&str, &str)]) -> Run {
    let mut full = args.to_vec();
    full.extend([
        "--env-file".to_owned(),
        bot.env.display().to_string(),
        support::environment_for(&bot.body, params),
    ]);
    let run = Run::start(&full, &[]);
    run.until(WAIT, label_is("bot.activated"));
    run
}

/// After `rest_after_ms` without input the cognition rests and memory
/// consolidates (decisions D195 and D205); the next input leaves rest at
/// once and is a normal turn. Not pinned: when the rest falls on the wall
/// clock depends on load.
#[test]
fn the_cognition_rests_after_a_stretch_without_input_and_wakes_for_the_next_turn() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Ignore, Generation::Fail, &[]);
    let mut run = start_activated(
        &bot,
        &[("bounds", "({rest_after_ms: 500, reflex_timeout_ms: 30000})")],
    );
    let rested = run.until(WAIT, label_is("cognition.consolidated"));
    assert_eq!(texts(&rested, "cognition.rest"), strings(&["resting"]));
    // The body's fresh memory holds no one to consolidate.
    assert_eq!(
        json_text(&rested, "cognition.consolidated"),
        json!({"folded": 0, "merged": 0, "steps": 0, "failed": false, "reason": null})
    );
    say(&mut run, "background noise");
    let records = run.until(WAIT, label_is("bot.processing.completed"));
    assert_eq!(texts(&records, "cognition.rest")[0], "awake");
    assert_eq!(
        json_text(&records, "floppy.output"),
        json!({"turn": 1, "output": [{"event": IGNORE, "data": {}, "target": COGNITION}]})
    );
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&[
            "autonomy unhandled (no-behaviors)",
            "decision handled (none)"
        ])
    );
    settle_reflection(&run);
    drop(run.close(WAIT));
}

/// Input while memory consolidates leaves rest at once: the consolidation
/// is cancelled (it never reports) and the turn runs as any other, and
/// every representative it left is whole (each fold step one transaction:
/// a representative's weight is one more than the voices folded into it).
#[test]
fn input_during_a_consolidation_cancels_it_and_is_a_normal_turn() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Ignore, Generation::Fail, &[]);
    let grant = support::memory_grant();
    // Twelve people, each first heard as their own axis and then forty
    // times more: a consolidation of sixty fold steps.
    let mut statements = Vec::new();
    for index in 0..12 {
        let person = format!("person-{}", index + 1);
        let voice: Vec<f64> = (0..12)
            .map(|axis| f64::from(u8::from(axis == index)))
            .collect();
        let voice = Json::from(voice).to_string();
        statements.push(json!({"sql": "INSERT INTO bot_person (person_id) VALUES (?1)", "parameters": [person]}));
        statements.push(json!({
            "sql": "INSERT INTO bot_person_embedding (person_id, kind, embedding) VALUES (?1, 'voice', ?2)",
            "parameters": [person, voice],
        }));
        statements.push(json!({
            "sql": "INSERT INTO bot_person_centroid (person_id, kind, centroid, weight, first_seq, through_seq) SELECT ?1, 'voice', ?2, 1, MIN(seq), MIN(seq) FROM bot_person_embedding WHERE person_id = ?1",
            "parameters": [person, format!("i8:{}", axis_base64(index))],
        }));
        statements.push(json!({
            "sql": "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 40) INSERT INTO bot_person_embedding (person_id, kind, embedding) SELECT ?1, 'voice', ?2 FROM n",
            "parameters": [person, voice],
        }));
    }
    transact(&grant, DATABASE, &Json::Array(statements));
    support::use_database(&bot.body, DATABASE);
    let mut run = start_with_activated(
        &bot,
        &grant,
        &[("bounds", "({rest_after_ms: 500, reflex_timeout_ms: 30000})")],
    );
    let resting = run.until(WAIT, |record| {
        label_is("cognition.rest")(record) && record["text"] == "resting"
    });
    assert!(texts(&resting, "cognition.consolidated").is_empty());
    // A step follows each five-second pause: input comes after the first.
    std::thread::sleep(Duration::from_millis(9000));
    run.event(
        "bot.input",
        &json!({"observation": {"event": "text", "data": "background noise"}}),
    );
    let records = run.until(WAIT, label_is("bot.processing.completed"));
    assert_eq!(texts(&records, "cognition.rest")[0], "awake");
    // The consolidation was cancelled before it reported.
    let woke = records
        .iter()
        .position(|record| label_is("cognition.rest")(record))
        .expect("awake");
    assert!(
        texts(&records[..woke], "cognition.consolidated").is_empty(),
        "{records:?}"
    );
    assert_eq!(
        json_text(&records, "floppy.output"),
        json!({"turn": 1, "output": [{"event": IGNORE, "data": {}, "target": COGNITION}]})
    );
    settle_reflection(&run);
    drop(run.close(WAIT));
    let output = transact(
        &grant,
        DATABASE,
        &json!([{"sql": "SELECT c.person_id, c.weight, (SELECT COUNT(*) FROM bot_person_embedding AS e WHERE e.person_id = c.person_id AND e.seq <= c.through_seq) FROM bot_person_centroid AS c ORDER BY c.person_id"}]),
    );
    let rows = output["results"][0]["rows"].as_array().expect("the rows");
    assert_eq!(rows.len(), 12);
    let mut folded = 0;
    for row in rows {
        let values = row["values"].as_array().expect("values");
        let weight = values[1].as_i64().expect("a weight");
        assert_eq!(weight, values[2].as_i64().expect("a count"), "{rows:?}");
        folded += weight - 1;
    }
    assert!(folded > 0, "some steps committed: {rows:?}");
    assert!(
        folded < 12 * 40,
        "the consolidation was cut short: {rows:?}"
    );
}

/// The base64 of the signed bytes of `axis`'s unit vector at twelve
/// dimensions (127 at `axis`, 0 elsewhere).
fn axis_base64(axis: u8) -> String {
    const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = (0..12)
        .map(|index| if index == axis { 127 } else { 0 })
        .collect();
    let mut text = String::new();
    for chunk in bytes.chunks(3) {
        let [a, b, c] = [chunk[0], chunk[1], chunk[2]];
        for sextet in [
            a >> 2,
            ((a & 3) << 4) | (b >> 4),
            ((b & 15) << 2) | (c >> 6),
            c & 63,
        ] {
            text.push(char::from(BASE64[usize::from(sextet)]));
        }
    }
    text
}

/// The `<held>` block of the identity's adopt, its one held action.
const HELD_ADOPT: &str = "<held count=\"1\">\n  <event name=\"bot.ability.identity.adopt\">";

/// Effort ceilings (decision D206): the decision layer's ceiling (0) is
/// under the identity's adopt (effort 1), so its choices leave adopt out and
/// the state it decides about renders it held, after the turn's situation
/// (decision D254); reasoning, whose ceiling is
/// the whole frame, is offered it and holds nothing.
#[test]
fn the_decision_layer_holds_what_is_over_its_effort_ceiling() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::Abstain, Generation::Ignore, &[]);
    let mut run = start(&bot);
    say(&mut run, "hello");
    let records = run.until(WAIT, turn_ended);
    assert_eq!(
        texts(&records, "cognition.stage"),
        strings(&[
            "autonomy unhandled (no-behaviors)",
            "decision unhandled (empty_product)",
            "reasoning handled"
        ]),
        "{records:?}"
    );
    let decisions = mock.received("/systemone");
    let state = decisions[0].body["state"]["turn"]
        .as_str()
        .expect("the decision's state is the turn's text");
    let held = state.find(HELD_ADOPT).unwrap_or_else(|| panic!("{state}"));
    let situation = state
        .find("<environment ")
        .unwrap_or_else(|| panic!("{state}"));
    assert!(
        situation < held,
        "the situation comes before the held block (decision D254): {state}"
    );
    assert!(
        !decisions[0].body["questions"].to_string().contains(ADOPT),
        "the decision layer is not offered adopt"
    );
    let reasoning = reasoning_requests(&mock);
    assert_eq!(reasoning.len(), 1);
    assert!(reasoning[0].body["tools"].to_string().contains(ADOPT));
    assert!(!prompts(&mock)[0].contains("<held count"));
    drop(run.close(WAIT));
}

/// The intuition tier's ceiling (1) offers it adopt; the bot's bounds may
/// lower it (`intuition_effort_ceiling`), and then intuition is not
/// offered adopt, its prompt renders adopt held, and the handoff takes the
/// turn to reasoning, which is offered it.
#[test]
fn the_bounds_lower_the_intuition_tiers_effort_ceiling() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::ToIntuition, Generation::HandOff, &[]);
    let mut run = start_activated(
        &bot,
        &[
            with_intuition()[0],
            ("bounds", "({intuition_effort_ceiling: 0})"),
        ],
    );
    say(&mut run, "hello");
    let records = run.until(WAIT, turn_ended);
    assert_eq!(
        texts(&records, "cognition.stage")
            .last()
            .map(String::as_str),
        Some("reasoning handled"),
        "{records:?}"
    );
    let intuition = intuition_requests(&mock);
    assert_eq!(intuition.len(), 1);
    assert!(offers_handoff(&intuition[0]));
    assert!(!intuition[0].body["tools"].to_string().contains(ADOPT));
    let prompt = intuition[0].body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .and_then(|message| message["content"].as_str())
        .expect("the user message");
    assert!(prompt.contains(HELD_ADOPT), "{prompt}");
    let reasoning: Vec<Received> = reasoning_requests(&mock)
        .into_iter()
        .filter(|request| request.body["model"] != INTUITION_MODEL)
        .collect();
    assert_eq!(reasoning.len(), 1);
    assert!(reasoning[0].body["tools"].to_string().contains(ADOPT));
    drop(run.close(WAIT));
}

/// With its default ceiling the intuition tier is offered adopt and holds
/// nothing.
#[test]
fn the_intuition_tier_is_offered_what_is_within_its_ceiling() {
    let mock = Mock::start();
    let bot = layout(&mock, Decision::ToIntuition, Generation::Respond, &[]);
    let mut run = start_activated(&bot, &with_intuition());
    say(&mut run, "hello");
    run.until(WAIT, turn_ended);
    let intuition = intuition_requests(&mock);
    assert_eq!(intuition.len(), 1);
    assert!(intuition[0].body["tools"].to_string().contains(ADOPT));
    assert!(!prompts(&mock)[0].contains("<held count"));
    drop(run.close(WAIT));
}
