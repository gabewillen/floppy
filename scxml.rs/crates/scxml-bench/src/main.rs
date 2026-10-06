//! Deterministic engine hot-path benchmark.
//!
//! Runs two fixed workloads through the one host execution entry point,
//! [`Engine::run`], and prints nanoseconds per `run` call (one external
//! command to a stable configuration) and per engine command (one microstep-producing
//! engine command, the requested command plus its resumes):
//!
//! - `representative`: `tests/fixtures/representative-machine.scxml`
//!   (compound states, a two-region parallel state, deep history, an internal
//!   raise, `In()` guards), driven through `begin`, `step`, then repeated
//!   `pause`/`resume` round trips through deep history;
//! - `parallel-wide-<N>`: a synthetic parallel state with `N` two-state
//!   regions that all toggle on every `t` event, swept over [`REGIONS`] so
//!   the per-event cost's scaling with region count is visible (it must be
//!   linear).
//!
//! The executor is a minimal in-process host: it answers `In('id')` guards
//! (optionally joined by `&&`) from the configuration the engine's updates
//! built, feeds `<raise>` back into the engine, and ignores `<log>`. No data
//! model, I/O,
//! or telemetry provider is installed, so the numbers measure the engine plus
//! whatever the telemetry facade costs with the default no-op provider.
//!
//! Usage: `cargo run --release --locked -p scxml-bench -- [iterations]`.
//! Each workload runs `iterations` (default 200) event rounds per sample,
//! [`SAMPLES`] samples after one warm-up sample, and reports the median. The
//! work checksum printed with each result must be identical between runs
//! being compared; only the timings may differ.

#![forbid(unsafe_code)]

use std::error::Error;
use std::fmt::Write as _;
use std::time::Instant;

use scxml_compiler_core::lower;
use scxml_engine::{
    BatchEnd, Command, Effect, EffectExecutor, EffectFailure, Engine, EngineError, EngineStatus,
    EvaluationError, GuardEvaluator, PlannedEffect, RunStage,
};
use scxml_syntax::{SourceId, SourceText, parse_xml};
use scxml_types::{Event, EventName, StateId};

/// The source id of each benchmarked document.
const DOCUMENT_SOURCE: SourceId = SourceId::new(1);

/// Representative fixture, embedded so the benchmark is independent of the
/// working directory.
const REPRESENTATIVE: &str = include_str!("../../../tests/fixtures/representative-machine.scxml");

/// Region counts of the synthetic parallel-wide sweep.
const REGIONS: [usize; 4] = [8, 16, 32, 64];

/// Timed samples per workload (after one warm-up sample).
const SAMPLES: usize = 7;

/// Default event rounds per sample.
const DEFAULT_ITERATIONS: u32 = 200;

/// Minimal deterministic host for [`Engine::run`].
#[derive(Default)]
struct BenchHost {
    configuration: Vec<StateId>,
}

impl GuardEvaluator for BenchHost {
    fn evaluate_condition(
        &mut self,
        expression: &str,
        _event: Option<&Event>,
    ) -> Result<bool, EvaluationError> {
        let mut verdict = true;
        for term in expression.split("&&") {
            let id = term
                .trim()
                .strip_prefix("In('")
                .and_then(|rest| rest.strip_suffix("')"))
                .ok_or(EvaluationError::InvalidInput)?;
            verdict &= self.configuration.iter().any(|state| state.as_str() == id);
        }
        Ok(verdict)
    }
}

impl EffectExecutor for BenchHost {
    type Error = EngineError;

    fn admit_command(&mut self) -> Result<(), EngineError> {
        Ok(())
    }

    fn command_completed(&mut self) {}

    fn engine_error(&mut self, error: EngineError, _stage: RunStage) -> EngineError {
        error
    }

    fn update_configuration(
        &mut self,
        exited: &[StateId],
        entered: &[StateId],
    ) -> Result<(), EngineError> {
        self.configuration.retain(|state| !exited.contains(state));
        self.configuration.extend_from_slice(entered);
        Ok(())
    }

    fn begin_batch(&mut self, _trigger: Option<&Event>) -> Result<(), EngineError> {
        Ok(())
    }

    fn apply_effect(
        &mut self,
        engine: &mut Engine,
        planned: &PlannedEffect,
        _trigger: Option<&Event>,
    ) -> Result<(), EffectFailure<EngineError>> {
        match &planned.effect {
            Effect::Log { .. } => Ok(()),
            Effect::Raise(name) => {
                let name = EventName::new(name.clone())
                    .map_err(|_| EffectFailure::Fatal(EngineError::InvalidModel))?;
                Ok(engine.raise(Event::internal(name))?)
            }
            _ => Err(EffectFailure::Fatal(EngineError::UnsupportedModel)),
        }
    }

    fn end_batch(&mut self, _end: BatchEnd) {}
}

/// Work performed by one sample; identical across compared runs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Work {
    runs: u64,
    commands: u64,
    effects: u64,
}

/// One named workload: a machine source, its warm-up prefix, and the event
/// cycle repeated once per iteration.
struct Workload {
    name: String,
    source: String,
    prefix: &'static [&'static str],
    cycle: &'static [&'static str],
}

fn parallel_wide_source(regions: usize) -> Result<String, std::fmt::Error> {
    let mut source = String::from(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="wide" datamodel="null"><parallel id="wide">"#,
    );
    for region in 0..regions {
        write!(
            source,
            r#"<state id="r{region}" initial="r{region}a"><state id="r{region}a"><transition event="t" target="r{region}b"/></state><state id="r{region}b"><transition event="t" target="r{region}a"/></state></state>"#
        )?;
    }
    source.push_str("</parallel></scxml>");
    Ok(source)
}

fn engine_for(source: &str) -> Result<Engine, Box<dyn Error>> {
    let document = parse_xml(SourceText::new(DOCUMENT_SOURCE, "bench.scxml", source))?;
    Ok(Engine::new(lower(&document)?)?)
}

fn run_command(
    engine: &mut Engine,
    host: &mut BenchHost,
    command: Command<'_>,
    work: &mut Work,
) -> Result<(), Box<dyn Error>> {
    let report = engine
        .run(command, host)
        .map_err(|failure| format!("{:?}: {}", failure.kind, failure.error))?;
    work.runs += 1;
    work.commands += u64::from(report.commands);
    work.effects += u64::from(report.committed_effects);
    Ok(())
}

fn events(names: &[&str]) -> Result<Vec<Event>, Box<dyn Error>> {
    names
        .iter()
        .map(|name| Ok(Event::external(EventName::new(*name)?)))
        .collect()
}

/// Runs one sample and returns its elapsed nanoseconds and work.
fn sample(workload: &Workload, iterations: u32) -> Result<(u128, Work), Box<dyn Error>> {
    let prefix = events(workload.prefix)?;
    let cycle = events(workload.cycle)?;
    let mut engine = engine_for(&workload.source)?;
    let mut host = BenchHost::default();
    let mut work = Work::default();
    let started = Instant::now();
    run_command(&mut engine, &mut host, Command::Start, &mut work)?;
    for event in &prefix {
        run_command(&mut engine, &mut host, Command::Send(event), &mut work)?;
    }
    for _ in 0..iterations {
        for event in &cycle {
            run_command(&mut engine, &mut host, Command::Send(event), &mut work)?;
        }
    }
    let elapsed = started.elapsed().as_nanos();
    if *engine.status() != EngineStatus::Running {
        return Err(format!("{} left the running state", workload.name).into());
    }
    Ok((elapsed, work))
}

fn main() -> Result<(), Box<dyn Error>> {
    let iterations = match std::env::args().nth(1) {
        Some(value) => value.parse::<u32>()?,
        None => DEFAULT_ITERATIONS,
    };
    let mut workloads = vec![Workload {
        name: "representative".to_owned(),
        source: REPRESENTATIVE.to_owned(),
        prefix: &["begin", "step"],
        cycle: &["pause", "resume"],
    }];
    for regions in REGIONS {
        workloads.push(Workload {
            name: format!("parallel-wide-{regions}"),
            source: parallel_wide_source(regions)?,
            prefix: &[],
            cycle: &["t"],
        });
    }
    println!("scxml-bench iterations={iterations} samples={SAMPLES} (median reported)");
    for workload in &workloads {
        let (_, reference) = sample(workload, iterations)?;
        let mut timings = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let (elapsed, work) = sample(workload, iterations)?;
            if work != reference {
                return Err(format!("{} performed nondeterministic work", workload.name).into());
            }
            timings.push(elapsed);
        }
        timings.sort_unstable();
        let median = timings[SAMPLES / 2];
        let per_run = median / u128::from(reference.runs.max(1));
        let per_command = median / u128::from(reference.commands.max(1));
        println!(
            "{:<17} ns/run={per_run:>7} ns/command={per_command:>7} work(runs={}, commands={}, effects={})",
            workload.name, reference.runs, reference.commands, reference.effects
        );
    }
    Ok(())
}
