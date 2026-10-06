//! Deterministic validator and reporter throughput benchmark.
//!
//! Runs three fixed workloads through the public validation and rendering
//! entry points and prints the median microseconds per iteration:
//!
//! - `fixtures`: every `tests/fixtures/*.scx` document (valid, invalid,
//!   adversarial, representative, script) validated once per iteration under
//!   the strict schema policy with the bundled W3C SCXML schema and the
//!   fixture extension-namespace schema;
//! - `generated-2k`: a synthetic valid document with [`GENERATED_STATES`]
//!   states ([`GROUPS`] compound states of [`GROUP_SIZE`] children, each child
//!   carrying two transitions), validated under the same strict policy;
//! - `generated-2k-no-xsd`: the same document with schema validation off,
//!   isolating XML parsing plus the semantic, graph, and expression rules;
//! - `render`: the canonical artifact of the adversarial fixture rendered
//!   once through the rustc-style renderer and once to SARIF per iteration.
//!
//! No telemetry provider is installed, so the numbers measure validation plus
//! whatever the telemetry facade costs with the default no-op provider. The
//! work checksum printed with each result (diagnostic count and rendered
//! bytes) must be identical between the runs being compared; only the timings
//! may differ.
//!
//! Usage: `cargo run --release --locked -p scxml-bench --bin validator -- [iterations]`
//! (default 5 iterations per sample, [`SAMPLES`] samples after one warm-up
//! sample).

#![forbid(unsafe_code)]

use std::error::Error;
use std::fmt::Write as _;
use std::time::Instant;

use scxml_reporting::{ColorChoice, RustcRenderer, render_sarif};
use scxml_syntax::SourceId;
use scxml_validator_core::{MemorySchemaResolver, SchemaMode, SchemaSource, Validator};
use scxml_validator_xsd::{BundledSchemaResolver, XsdSchemaBackend};

/// The source id of each benchmarked document.
const DOCUMENT_SOURCE: SourceId = SourceId::new(1);

/// Fixture documents, embedded so the benchmark is independent of the
/// working directory.
const FIXTURES: [(&str, &str); 5] = [
    (
        "valid.scx",
        include_str!("../../../../tests/fixtures/valid.scx"),
    ),
    (
        "invalid.scx",
        include_str!("../../../../tests/fixtures/invalid.scx"),
    ),
    ("validator-adversarial.scx", ADVERSARIAL),
    (
        "representative-machine.scx",
        include_str!("../../../../tests/fixtures/representative-machine.scx"),
    ),
    (
        "script-machine.scx",
        include_str!("../../../../tests/fixtures/script-machine.scx"),
    ),
];

/// Adversarial fixture; its report is also the rendering workload input.
const ADVERSARIAL: &str = include_str!("../../../../tests/fixtures/validator-adversarial.scx");

/// Extension-namespace schema the valid and invalid fixtures use.
const CUSTOM_SCHEMA: &str = include_str!("../../../../tests/fixtures/extension-example.xsd");
const EXTENSION_NAMESPACE: &str = "urn:example:custom";

/// Compound states in the generated document.
const GROUPS: usize = 50;
/// Child states per compound state in the generated document.
const GROUP_SIZE: usize = 39;
/// Total states in the generated document (groups plus their children).
const GENERATED_STATES: usize = GROUPS * (GROUP_SIZE + 1);

/// Timed samples per workload (after one warm-up sample).
const SAMPLES: usize = 7;

/// Default iterations per sample.
const DEFAULT_ITERATIONS: u32 = 5;

type BenchValidator = Validator<BundledSchemaResolver<MemorySchemaResolver>, XsdSchemaBackend>;

fn bench_validator(schema_mode: SchemaMode) -> BenchValidator {
    let mut resolver = MemorySchemaResolver::new();
    resolver.insert(
        EXTENSION_NAMESPACE,
        SchemaSource::new("extension-example.xsd", CUSTOM_SCHEMA.as_bytes()),
    );
    Validator::new(BundledSchemaResolver::new(resolver), XsdSchemaBackend)
        .with_schema_mode(schema_mode)
}

/// A valid, deterministic document of [`GENERATED_STATES`] states.
fn generated_source() -> Result<String, std::fmt::Error> {
    let mut source = String::from(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="g0" datamodel="null">"#,
    );
    for group in 0..GROUPS {
        let next_group = (group + 1) % GROUPS;
        write!(source, r#"<state id="g{group}" initial="g{group}s0">"#)?;
        for child in 0..GROUP_SIZE {
            let next_child = (child + 1) % GROUP_SIZE;
            write!(
                source,
                r#"<state id="g{group}s{child}"><transition event="next" target="g{group}s{next_child}"/><transition event="jump.{child}" target="g{next_group}s{child}"/></state>"#
            )?;
        }
        source.push_str("</state>");
    }
    source.push_str("</scxml>");
    Ok(source)
}

/// Work performed by one sample; identical across compared runs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Work {
    documents: u64,
    diagnostics: u64,
    rendered_bytes: u64,
}

/// One named workload body, run `iterations` times per sample.
type Body<'a> = Box<dyn Fn(&mut Work) -> Result<(), Box<dyn Error>> + 'a>;

fn validate_into(
    validator: &BenchValidator,
    name: &str,
    text: &str,
    work: &mut Work,
) -> Result<(), Box<dyn Error>> {
    let outcome = validator.validate_source(DOCUMENT_SOURCE, name, text);
    work.documents += 1;
    work.diagnostics += u64::try_from(outcome.report.diagnostics().len())?;
    Ok(())
}

fn sample(body: &Body<'_>, iterations: u32) -> Result<(u128, Work), Box<dyn Error>> {
    let mut work = Work::default();
    let started = Instant::now();
    for _ in 0..iterations {
        body(&mut work)?;
    }
    Ok((started.elapsed().as_nanos(), work))
}

fn main() -> Result<(), Box<dyn Error>> {
    let iterations = match std::env::args().nth(1) {
        Some(value) => value.parse::<u32>()?,
        None => DEFAULT_ITERATIONS,
    };
    let validator = bench_validator(SchemaMode::Strict);
    let semantic_validator = bench_validator(SchemaMode::Off);
    let generated = generated_source()?;
    let adversarial =
        validator.validate_source(DOCUMENT_SOURCE, "validator-adversarial.scx", ADVERSARIAL);
    let artifact = adversarial.report.canonical_artifact(&adversarial.source)?;
    let workloads: [(&str, Body<'_>); 4] = [
        (
            "fixtures",
            Box::new(|work| {
                for (name, text) in FIXTURES {
                    validate_into(&validator, name, text, work)?;
                }
                Ok(())
            }),
        ),
        (
            "generated-2k",
            Box::new(|work| validate_into(&validator, "generated.scxml", &generated, work)),
        ),
        (
            "generated-2k-no-xsd",
            Box::new(|work| {
                validate_into(&semantic_validator, "generated.scxml", &generated, work)
            }),
        ),
        (
            "render",
            Box::new(|work| {
                let human = RustcRenderer {
                    color: ColorChoice::Never,
                }
                .render(&artifact)?;
                let sarif = render_sarif(&artifact)?;
                work.documents += 1;
                work.rendered_bytes += u64::try_from(human.len() + sarif.len())?;
                Ok(())
            }),
        ),
    ];
    println!(
        "scxml-bench validator iterations={iterations} samples={SAMPLES} generated-states={GENERATED_STATES} (median reported)"
    );
    for (name, body) in &workloads {
        let (_, reference) = sample(body, iterations)?;
        let mut timings = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let (elapsed, work) = sample(body, iterations)?;
            if work != reference {
                return Err(format!("{name} performed nondeterministic work").into());
            }
            timings.push(elapsed);
        }
        timings.sort_unstable();
        let median = timings.get(SAMPLES / 2).copied().unwrap_or_default();
        let per_iteration = median / u128::from(iterations.max(1)) / 1_000;
        println!(
            "{name:<19} us/iter={per_iteration:>8} work(documents={}, diagnostics={}, rendered-bytes={})",
            reference.documents, reference.diagnostics, reference.rendered_bytes
        );
    }
    Ok(())
}
