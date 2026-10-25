//! The driver of floppy's eval (`tests/eval/`, docs/eval.md): it runs the
//! floppy bot (`bots/floppy/floppy.scx`, in the room
//! `environments/room/room.scx`, as `floppy_live` does) through the
//! scenarios of one plan and writes every run's records, step by step, for
//! `tests/eval/eval.py` to score and judge. It is ignored by default, and
//! it asserts nothing about the bot: a scenario the bot fails is a score,
//! not a test failure.
//!
//! `FLOPPY_EVAL_PLAN=<plan.json> FLOPPY_LIVE_ENV_FILE=<paths> cargo test -p
//! floppy-tests --locked --test floppy_eval -- --ignored --nocapture`
//!
//! The plan is `{"parallel": N, "jobs": [{"id", "scenario", "overlay",
//! "out", "env_files"?, "model"?}]}`; a job's `env_files` and `model`
//! replace `FLOPPY_LIVE_ENV_FILE` and `FLOPPY_LIVE_MODEL` for it. Each job lays out a scratch copy of the bot and the builder
//! its reflection invokes, copies the files of `overlay` (a directory laid
//! out like the workspace, or null) over it, applies the scenario's `edits`
//! (`[{path, old, new}]`, each `old` found exactly once in the layout's
//! `path`), and runs the scenario's sessions one after another on one
//! memory directory, one behaviors directory and one media directory
//! (`<out>/media`, granted to the speech, generate and vision services,
//! and moved to `<out>/media-<session>` when the session ends), so a later
//! session is the bot restarted with what it learned. The room
//! passes the bot the scenario's `params` (`{name: expr}`) beside its
//! model. A session's steps are:
//!
//! - `{"say": text, "source"?, "targets"?, "timeout_s"?, "settle"?}`: one
//!   `bot.ability.communication.input` from `source` (default `gabe`) to
//!   `targets` (default `["floppy"]`; `[]` is ambient), waited on until its reflection
//!   settles or its turn fails, and then, bounded again by its timeout,
//!   until each lesson the turn gave the learning ability is learned or
//!   fails and a turn settles for its result;
//! - `{"hear": text, "audio": path, "source"?, "timeout_s"?, "settle"?}`:
//!   the person's voice saying `text`, synthesized by `eval.py` into the
//!   24 kHz mono 16-bit PCM file `audio`, copied into the media directory
//!   and given to the room as `environment.sound` from `source`, with a
//!   second of silence after it (as `scripts/talk-live.py` gives GPT-Live's
//!   voice), waited on as a said step;
//! - `{"show": path, "media_type"?, "source"?, "wait_s"?}`: the image
//!   `path` copied into the media directory and given to the room as
//!   `environment.visual` from `source` (default `camera`), then
//!   `wait_s` (default 20) of wall clock;
//! - `{"wait_s": seconds}`: the wall clock passes with no input, for
//!   routines.
//!
//! A said or heard step's `settle` is `"turn"` (the default, as above),
//! `"said"`: the step ends at floppy's first `floppy.say` (or its turn's
//! settling), and the next step's wait counts the settling it still owes,
//! so a person can speak again while floppy's reflection runs, or
//! `"quiet"`: as `"turn"`, but a turn that has not settled by the step's
//! timeout and a further [`QUIET_GRACE`] ends the step as completed (input
//! floppy may rightly leave alone, such as people talking to each other);
//! a late turn, ignored or answered, still settles in its own step. A
//! scenario's `bot_name`, when not null, is the name the identity starts
//! with. Each job writes `<out>/session-<n>.json` (the records),
//! `<out>/steps.json` (each step's range of records, wall-clock bounds,
//! whether its turn settled, the number of lessons closed during it and
//! whether they settled) and `<out>/done.json`. Values of the env files
//! are never printed.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value as Json, json};

use support::{Run, unique_scratch, workspace};

/// The bound of a said step that names none: longer than reflection's
/// select step and a builder revision of a few attempts.
const SAY_TIMEOUT: Duration = Duration::from_mins(15);

/// How long a settled said step keeps collecting records: the turn's
/// reply can be said after its reflection settled.
const SAY_GRACE: Duration = Duration::from_secs(15);

/// How much longer a `"quiet"` step waits for its turn past its timeout:
/// floppy takes up and settles every input, ignored or answered, so a turn
/// that is only slow (a bot still starting) settles in its own step, not
/// the next, whose reply it would otherwise be read as.
const QUIET_GRACE: Duration = Duration::from_mins(3);

/// The message of [`Run::until`]'s panic on its limit, which [`wait`]
/// expects for every wait step.
const TIMED_OUT: &str = "no expected record within";

/// The records that end a said step's wait: its reflection settled, or its
/// turn (or the bot) failed.
const SETTLED: &[&str] = &[
    "bot.ability.reflection.output",
    "cognition.reflection.failed",
    "bot.processing.failed",
    "floppy.degraded",
    "bot.activating.failed",
];

/// A scratch copy of the bot and the builder, with `overlay` copied over
/// it, the scenario's `edits` applied and the identity starting with
/// `name`; returns the body's path.
fn layout(overlay: Option<&Path>, scenario: &Json) -> PathBuf {
    let dir = unique_scratch("floppy-eval");
    for sub in ["bots/floppy", "bots/builder"] {
        support::copy_tree(&workspace().join(sub), &dir.join(sub));
    }
    support::copy_tree(&workspace().join("abilities"), &dir.join("abilities"));
    if let Some(overlay) = overlay {
        support::copy_tree(overlay, &dir);
    }
    for edit in scenario["edits"].as_array().cloned().unwrap_or_default() {
        let path = dir.join(edit["path"].as_str().expect("an edit names its path"));
        let old = edit["old"]
            .as_str()
            .expect("an edit names the text it replaces");
        let new = edit["new"].as_str().expect("an edit names its new text");
        let text = std::fs::read_to_string(&path).expect("an edited file is readable");
        assert_eq!(
            text.matches(old).count(),
            1,
            "an edit's text occurs exactly once in {}",
            path.display()
        );
        std::fs::write(&path, text.replacen(old, new, 1)).expect("an edited file is writable");
    }
    if let Some(name) = scenario["bot_name"].as_str() {
        let identity = dir.join("abilities/identity/identity.scx");
        let text = std::fs::read_to_string(&identity).expect("the identity is readable");
        let declared = r#"<data id="name" expr="null"/>"#;
        assert!(text.contains(declared), "the identity declares its name");
        std::fs::write(
            &identity,
            text.replace(declared, &format!(r#"<data id="name" expr="'{name}'"/>"#)),
        )
        .expect("the identity copy is writable");
    }
    dir.join("bots/floppy/floppy.scx")
}

/// Starts the room with the bot at `body`, its memory in `memory` and its
/// media in `media`: the job's `env_files` and `model` when it names them,
/// else the live tests', and the scenario's `params`.
fn start(body: &Path, memory: &Path, media: &Path, job: &Json) -> Run {
    let mut args = Vec::new();
    let env_files: Vec<PathBuf> = match job["env_files"].as_array() {
        Some(files) => files
            .iter()
            .filter_map(Json::as_str)
            .map(PathBuf::from)
            .collect(),
        None => support::live_env_files(),
    };
    for file in env_files {
        args.push("--env-file".to_owned());
        args.push(file.display().to_string());
    }
    let model = format!(
        "'{}'",
        job["model"]
            .as_str()
            .map_or_else(support::live_model, str::to_owned)
    );
    let mut params = vec![("model".to_owned(), model)];
    if let Some(extra) = job["scenario"]["params"].as_object() {
        for (name, expr) in extra {
            params.push((
                name.clone(),
                expr.as_str().expect("a param is an expression").to_owned(),
            ));
        }
    }
    let params: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, expr)| (name.as_str(), expr.as_str()))
        .collect();
    let layout = body
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("the body is <layout>/bots/floppy/floppy.scx");
    let room =
        support::environment_with_devices(layout, &["floppy"], &[], &[("floppy", &[])], &params);
    args.push("--dir".to_owned());
    args.push(format!(
        "{}={}::/memory",
        support::service_uri("sql"),
        memory.display()
    ));
    for service in ["speech", "generate", "vision"] {
        args.push("--dir".to_owned());
        args.push(format!(
            "{}={}::/media",
            support::service_uri(service),
            media.display()
        ));
    }
    args.extend(support::behaviors_grant(&room));
    args.push(room.display().to_string());
    Run::start(&args, &[])
}

/// The labels that open a lesson the learning ability took (logged once
/// per lesson, when it has recalled its memory) and that close one: it was
/// learned or it failed. Each closed lesson's result comes back as a turn
/// of its own, which settles like any turn.
const LESSON_OPEN_LABELS: &[&str] = &["learning.recalled"];
const LESSON_CLOSE_LABELS: &[&str] = &["learning.learned", "learning.failed"];

fn is_log_of(record: &Json, labels: &[&str]) -> bool {
    record["type"] == "log"
        && record["label"]
            .as_str()
            .is_some_and(|label| labels.contains(&label))
}

fn is_settled(record: &Json) -> bool {
    is_log_of(record, SETTLED)
}

/// The records of a run so far that open a lesson, close one, and settle a
/// turn.
#[derive(Clone, Copy)]
struct Tally {
    opened: usize,
    closed: usize,
    settled: usize,
}

impl Tally {
    fn of(run: &Run) -> Self {
        Self {
            opened: run.count(|r| is_log_of(r, LESSON_OPEN_LABELS)),
            closed: run.count(|r| is_log_of(r, LESSON_CLOSE_LABELS)),
            settled: run.count(is_settled),
        }
    }
}

/// Waits, once a said step's own turn settled, until every lesson opened
/// since `before` has closed and one more turn has settled for each closed
/// lesson (the turn carrying its result), at most `limit`; whether they
/// did, and how many lessons closed. A turn that settles in the meantime
/// for another reason (a routine's tick) counts too, so this can end
/// early, never late.
fn lessons_settle(run: &Run, limit: Duration, before: Tally) -> (bool, usize) {
    let deadline = Instant::now() + limit;
    let mut waited = false;
    loop {
        let now = Tally::of(run);
        let opened = now.opened.saturating_sub(before.opened);
        let closed = now.closed.saturating_sub(before.closed);
        // The step's own turn is the first settle.
        let results = now.settled.saturating_sub(before.settled).saturating_sub(1);
        if opened <= closed && results >= closed {
            if waited {
                wait(run, SAY_GRACE, |_| false);
            }
            return (true, closed);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        // Past the deadline, only the records already queued would end the
        // wait: it ends here, not at the next one.
        if left.is_zero() {
            return (false, closed);
        }
        if !wait(run, left, |r| {
            is_log_of(r, LESSON_CLOSE_LABELS) || is_settled(r)
        }) {
            return (false, closed);
        }
        waited = true;
    }
}

/// Waits on `run` until `done` holds or `limit` passes; whether it held.
/// [`Run::until`] panics on its limit, and every record it read stays in
/// the run's journal either way.
fn wait(run: &Run, limit: Duration, done: impl Fn(&Json) -> bool) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run.until(limit, done))).is_ok()
}

fn millis(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// The record that ends a step whose `settle` is `"said"`: floppy began
/// to say something.
const SAID: &[&str] = &["floppy.say"];

/// The length of the silence given to the room after each heard
/// utterance, as `scripts/talk-live.py` gives it (24 kHz, 16-bit mono).
const SILENCE_BYTES: usize = 2 * 24_000;

/// Waits until `run` holds `target` settled turns in all, at most `limit`;
/// whether it did.
fn settles_to(run: &Run, limit: Duration, target: usize) -> bool {
    let deadline = Instant::now() + limit;
    while Tally::of(run).settled < target {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || !wait(run, left, is_settled) {
            return Tally::of(run).settled >= target;
        }
    }
    true
}

/// What one step did: its kind, whether it completed, how many lessons
/// closed during it and whether they settled.
struct Done {
    kind: &'static str,
    completed: bool,
    lessons: usize,
    lessons_settled: bool,
}

/// Gives the room what a said or heard step carries.
fn give(run: &mut Run, spec: &Json, media: &Path, name: &str) -> &'static str {
    let source = spec["source"].as_str().unwrap_or("gabe");
    if let Some(content) = spec["say"].as_str() {
        // An empty `targets` is ambient input: the room gives it to every
        // participant, and floppy overhears it.
        let targets = spec["targets"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| vec![json!("floppy")]);
        run.event(
            "bot.ability.communication.input",
            &json!({
                "source_ids": [source],
                "target_ids": targets,
                "content": content,
                "content_type": "text/plain"
            }),
        );
        return "say";
    }
    let audio = spec["audio"]
        .as_str()
        .expect("a heard step names its audio");
    let file = format!("{name}.pcm");
    std::fs::copy(audio, media.join(&file)).expect("a heard step's audio is copyable");
    let silence = media.join("silence.pcm");
    if !silence.exists() {
        std::fs::write(&silence, vec![0_u8; SILENCE_BYTES]).expect("the silence is writable");
    }
    for audio in [format!("media/{file}"), "media/silence.pcm".to_owned()] {
        run.event(
            "environment.sound",
            &json!({
                "audio": audio,
                "media_type": "audio/pcm",
                "sample_rate_hz": 24_000,
                "channels": 1,
                "source": source
            }),
        );
    }
    "hear"
}

/// Runs one said or heard step. `target` is how many settled turns the
/// session must hold once every turn given so far has settled; this step's
/// turn adds one, and a `"said"` step leaves its settling to a later wait.
fn converse(run: &mut Run, spec: &Json, media: &Path, name: &str, target: &mut usize) -> Done {
    let mut before = Tally::of(run);
    *target = (*target).max(before.settled) + 1;
    let kind = give(run, spec, media, name);
    let limit = spec["timeout_s"]
        .as_u64()
        .map_or(SAY_TIMEOUT, Duration::from_secs);
    if spec["settle"].as_str() == Some("said") {
        // The step ends once floppy speaks (or its turn settles); its
        // turn's settling is waited on by a later step.
        let said = wait(run, limit, |r| is_log_of(r, SAID) || is_settled(r));
        return Done {
            kind,
            completed: said,
            lessons: Tally::of(run).closed.saturating_sub(before.closed),
            lessons_settled: true,
        };
    }
    let quiet = spec["settle"].as_str() == Some("quiet");
    let settled =
        settles_to(run, limit, *target) || (quiet && settles_to(run, QUIET_GRACE, *target));
    if !settled && quiet {
        // Input floppy may rightly leave alone need not settle a turn: the
        // step ends at its bound, and owes nothing.
        *target -= 1;
        return Done {
            kind,
            completed: true,
            lessons: Tally::of(run).closed.saturating_sub(before.closed),
            lessons_settled: true,
        };
    }
    if !settled {
        // A stuck turn may still have closed lessons: count them, so it
        // reads apart from one that taught nothing.
        let lessons = Tally::of(run).closed.saturating_sub(before.closed);
        return Done {
            kind,
            completed: false,
            lessons,
            lessons_settled: false,
        };
    }
    // The reply is spoken after the turn's reflection may have settled;
    // let it land in this step, not the next.
    wait(run, SAY_GRACE, |_| false);
    // A lesson the turn selected is learned after the turn: the step is
    // what was taught, up to its result, with a bound of its own. Its own
    // settle is the last one `target` counts.
    before.settled = target.saturating_sub(1);
    let (learned, lessons) = lessons_settle(run, limit, before);
    Done {
        kind,
        completed: true,
        lessons,
        lessons_settled: learned,
    }
}

/// Runs one shown step: the image given to the room, then its wait.
fn show(run: &mut Run, spec: &Json, media: &Path, name: &str) -> Done {
    let image = Path::new(spec["show"].as_str().expect("a shown step names its image"));
    let extension = image
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("png");
    let file = format!("{name}.{extension}");
    std::fs::copy(image, media.join(&file)).expect("a shown step's image is copyable");
    let media_type = spec["media_type"]
        .as_str()
        .map_or_else(|| format!("image/{extension}"), str::to_owned);
    let before = Tally::of(run);
    run.event(
        "environment.visual",
        &json!({
            "image": format!("media/{file}"),
            "media_type": media_type,
            "source": spec["source"].as_str().unwrap_or("camera")
        }),
    );
    let seconds = spec["wait_s"].as_u64().unwrap_or(20);
    wait(run, Duration::from_secs(seconds), |_| false);
    Done {
        kind: "show",
        completed: true,
        lessons: Tally::of(run).closed.saturating_sub(before.closed),
        lessons_settled: true,
    }
}

/// Runs one job of the plan.
fn run_job(job: &Json, clock: Instant) {
    let id = job["id"].as_str().expect("a job has an id");
    let out = PathBuf::from(job["out"].as_str().expect("a job has an out directory"));
    std::fs::create_dir_all(&out).expect("the out directory is creatable");
    let scenario = &job["scenario"];
    let overlay = job["overlay"].as_str().map(PathBuf::from);
    let body = layout(overlay.as_deref(), scenario);
    let memory = unique_scratch("floppy-eval-memory");
    // Inside the out directory: the grade reads the sounds floppy made.
    let media = out.join("media");
    std::fs::create_dir_all(&media).expect("the media directory is creatable");
    let mut steps = Vec::new();
    let sessions = scenario["sessions"].as_array().cloned().unwrap_or_default();
    for (session, spec) in sessions.iter().enumerate() {
        let mut run = start(&body, &memory, &media, job);
        let mut target = 0;
        for (step, spec) in spec["steps"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let start_index = run.count(|_| true);
            let started_ms = millis(clock);
            let name = format!("step-{session}-{step}");
            let done = if spec["say"].is_string() || spec["hear"].is_string() {
                converse(&mut run, spec, &media, &name, &mut target)
            } else if spec["show"].is_string() {
                show(&mut run, spec, &media, &name)
            } else {
                let seconds = spec["wait_s"].as_u64().unwrap_or(0);
                let before = Tally::of(&run);
                // Nothing ends a wait but its time; the lessons a
                // routine's turns closed in it are counted, not awaited.
                wait(&run, Duration::from_secs(seconds), |_| false);
                Done {
                    kind: "wait",
                    completed: true,
                    lessons: Tally::of(&run).closed.saturating_sub(before.closed),
                    lessons_settled: true,
                }
            };
            steps.push(json!({
                "session": session,
                "step": step,
                "kind": done.kind,
                "start_index": start_index,
                "end_index": run.count(|_| true),
                "started_ms": started_ms,
                "ended_ms": millis(clock),
                "completed": done.completed,
                "lessons": done.lessons,
                "lessons_settled": done.lessons_settled,
            }));
            // A said step that never settled leaves the bot mid-turn: the
            // rest of the session would be scored on a stuck bot.
            if !done.completed {
                break;
            }
        }
        // A turn of a `"said"` step still settling lands in the last
        // step's records.
        let stuck = steps.last().is_some_and(|last| last["completed"] == false);
        if !stuck && Tally::of(&run).settled < target {
            let _settled = settles_to(&run, SAY_TIMEOUT, target);
            if let Some(last) = steps.last_mut() {
                last["end_index"] = json!(run.count(|_| true));
                last["ended_ms"] = json!(millis(clock));
            }
        }
        let finished = run.close(Duration::from_mins(1));
        std::fs::write(
            out.join(format!("session-{session}.json")),
            serde_json::to_string_pretty(&finished.all).expect("JSON"),
        )
        .expect("the records are writable");
        // A restarted floppy numbers its utterances from 1 again: each
        // session's sounds are kept apart for the grade to hear.
        std::fs::rename(&media, out.join(format!("media-{session}")))
            .expect("the session's media directory is movable");
        std::fs::create_dir_all(&media).expect("the media directory is creatable");
    }
    std::fs::write(
        out.join("steps.json"),
        serde_json::to_string_pretty(&steps).expect("JSON"),
    )
    .expect("the steps are writable");
    std::fs::write(
        out.join("done.json"),
        json!({"id": id, "ms": millis(clock)}).to_string(),
    )
    .expect("the done marker is writable");
    println!("floppy eval job {id}: done");
}

#[test]
#[ignore = "makes real decision and model calls; needs FLOPPY_EVAL_PLAN and FLOPPY_LIVE_ENV_FILE"]
fn floppy_eval() {
    let plan_path = std::env::var_os("FLOPPY_EVAL_PLAN")
        .expect("set FLOPPY_EVAL_PLAN to the plan tests/eval/eval.py writes");
    let plan: Json =
        serde_json::from_str(&std::fs::read_to_string(&plan_path).expect("the plan is readable"))
            .expect("the plan is JSON");
    // A wait reaching its limit is how a wait step ends: keep its panic out
    // of the output, and report every other one as usual.
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| info.payload().downcast_ref::<&str>().copied())
            .unwrap_or_default();
        if !message.starts_with(TIMED_OUT) {
            report(info);
        }
    }));
    // Build the components and scx once, before the jobs share them.
    let _cli = support::cli();
    let parallel = plan["parallel"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(1)
        .max(1);
    let queue = Arc::new(Mutex::new(
        plan["jobs"].as_array().cloned().unwrap_or_default(),
    ));
    let clock = Instant::now();
    let workers: Vec<_> = (0..parallel)
        .map(|_| {
            let queue = Arc::clone(&queue);
            std::thread::spawn(move || {
                loop {
                    let job = queue.lock().expect("the queue is not poisoned").pop();
                    let Some(job) = job else { break };
                    // One job's panic (a layout it cannot write) leaves the
                    // others running; the scorer sees no done.json for it.
                    let id = job["id"].as_str().unwrap_or_default().to_owned();
                    if std::panic::catch_unwind(|| run_job(&job, clock)).is_err() {
                        println!("floppy eval job {id}: crashed");
                    }
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().expect("a worker ends");
    }
}
