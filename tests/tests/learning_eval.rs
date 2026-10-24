//! The driver of floppy's learning eval (`tests/eval/`, docs/eval.md): it
//! runs the floppy bot (`bots/floppy/floppy.scx`, in the room
//! `environments/room/room.scx`, as `floppy_live` does) through the
//! scenarios of one plan and writes every run's records, step by step, for
//! `tests/eval/eval.py` to score and judge. It is ignored by default, and
//! it asserts nothing about the bot: a scenario the bot fails is a score,
//! not a test failure.
//!
//! `FLOPPY_EVAL_PLAN=<plan.json> FLOPPY_LIVE_ENV_FILE=<paths> cargo test -p
//! floppy-tests --locked --test learning_eval -- --ignored --nocapture`
//!
//! The plan is `{"parallel": N, "jobs": [{"id", "scenario", "overlay",
//! "out", "env_files"?, "model"?}]}`; a job's `env_files` and `model`
//! replace `FLOPPY_LIVE_ENV_FILE` and `FLOPPY_LIVE_MODEL` for it. Each job lays out a scratch copy of the bot and the builder
//! its reflection invokes, copies the files of `overlay` (a directory laid
//! out like the workspace, or null) over it, and runs the scenario's
//! sessions one after another on one memory directory and one behaviors
//! directory, so a later session is the bot restarted with what it
//! learned. A session's steps are `{"say": text, "source"?, "timeout_s"?}`
//! (one `bot.ability.communication.input`, waited on until its reflection
//! settles or its turn fails, and then, bounded again by its timeout, until
//! each lesson the turn gave the learning ability is learned or fails and
//! a turn settles for its result) and `{"wait_s": seconds}` (the wall clock
//! passes with no input, for routines). A scenario's `bot_name`, when not
//! null, is the name the identity starts with. Each job writes
//! `<out>/session-<n>.json` (the records), `<out>/steps.json` (each step's
//! range of records, wall-clock bounds, whether its turn settled, the
//! number of lessons closed during it and whether they settled) and
//! `<out>/done.json`. Values of the env files are never printed.

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
/// it and the identity starting with `name`; returns the body's path.
fn layout(overlay: Option<&Path>, name: Option<&str>) -> PathBuf {
    let dir = unique_scratch("learning-eval");
    for sub in ["bots/floppy", "bots/builder"] {
        support::copy_tree(&workspace().join(sub), &dir.join(sub));
    }
    support::copy_tree(&workspace().join("abilities"), &dir.join("abilities"));
    if let Some(overlay) = overlay {
        support::copy_tree(overlay, &dir);
    }
    if let Some(name) = name {
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

/// Starts the room with the bot at `body`, its memory in `memory`: the
/// job's `env_files` and `model` when it names them, else the live tests'.
fn start(body: &Path, memory: &Path, job: &Json) -> Run {
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
    let layout = body
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("the body is <layout>/bots/floppy/floppy.scx");
    let room = support::environment_with_devices(
        layout,
        &["floppy"],
        &[],
        &[("floppy", &[])],
        &[("model", &model)],
    );
    args.push("--dir".to_owned());
    args.push(format!(
        "{}={}::/memory",
        support::service_uri("sql"),
        memory.display()
    ));
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

/// Runs one job of the plan.
fn run_job(job: &Json, clock: Instant) {
    let id = job["id"].as_str().expect("a job has an id");
    let out = PathBuf::from(job["out"].as_str().expect("a job has an out directory"));
    std::fs::create_dir_all(&out).expect("the out directory is creatable");
    let scenario = &job["scenario"];
    let overlay = job["overlay"].as_str().map(PathBuf::from);
    let body = layout(overlay.as_deref(), scenario["bot_name"].as_str());
    let memory = unique_scratch("learning-eval-memory");
    let mut steps = Vec::new();
    let sessions = scenario["sessions"].as_array().cloned().unwrap_or_default();
    for (session, spec) in sessions.iter().enumerate() {
        let mut run = start(&body, &memory, job);
        for (step, spec) in spec["steps"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let start_index = run.count(|_| true);
            let started_ms = millis(clock);
            let (kind, completed, lessons, lessons_settled) =
                if let Some(content) = spec["say"].as_str() {
                    let source = spec["source"].as_str().unwrap_or("gabe");
                    let before = Tally::of(&run);
                    run.event(
                        "bot.ability.communication.input",
                        &json!({
                            "source_ids": [source],
                            "target_ids": ["floppy"],
                            "content": content,
                            "content_type": "text/plain"
                        }),
                    );
                    let limit = spec["timeout_s"]
                        .as_u64()
                        .map_or(SAY_TIMEOUT, Duration::from_secs);
                    let settled = wait(&run, limit, is_settled);
                    if settled {
                        // The reply is spoken after the turn's reflection may
                        // have settled; let it land in this step, not the next.
                        wait(&run, SAY_GRACE, |_| false);
                        // A lesson the turn selected is learned after the turn:
                        // the step is what was taught, up to its result, with a
                        // bound of its own.
                        let (learned, lessons) = lessons_settle(&run, limit, before);
                        ("say", true, lessons, learned)
                    } else {
                        // A stuck turn may still have closed lessons: count
                        // them, so it reads apart from one that taught nothing.
                        let lessons = Tally::of(&run).closed.saturating_sub(before.closed);
                        ("say", false, lessons, false)
                    }
                } else {
                    let seconds = spec["wait_s"].as_u64().unwrap_or(0);
                    let before = Tally::of(&run);
                    // Nothing ends a wait but its time; the lessons a
                    // routine's turns closed in it are counted, not awaited.
                    wait(&run, Duration::from_secs(seconds), |_| false);
                    let lessons = Tally::of(&run).closed.saturating_sub(before.closed);
                    ("wait", true, lessons, true)
                };
            steps.push(json!({
                "session": session,
                "step": step,
                "kind": kind,
                "start_index": start_index,
                "end_index": run.count(|_| true),
                "started_ms": started_ms,
                "ended_ms": millis(clock),
                "completed": completed,
                "lessons": lessons,
                "lessons_settled": lessons_settled,
            }));
            // A said step that never settled leaves the bot mid-turn: the
            // rest of the session would be scored on a stuck bot.
            if !completed {
                break;
            }
        }
        let finished = run.close(Duration::from_mins(1));
        std::fs::write(
            out.join(format!("session-{session}.json")),
            serde_json::to_string_pretty(&finished.all).expect("JSON"),
        )
        .expect("the records are writable");
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
    println!("learning eval job {id}: done");
}

#[test]
#[ignore = "makes real decision and model calls; needs FLOPPY_EVAL_PLAN and FLOPPY_LIVE_ENV_FILE"]
fn learning_eval() {
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
                        println!("learning eval job {id}: crashed");
                    }
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().expect("a worker ends");
    }
}
