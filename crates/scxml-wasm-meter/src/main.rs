//! `scxml-wasm-meter --initial-fuel N INPUT OUTPUT`: meters one core module.
//!
//! Reads at most [`MAX_MODULE_BYTES`] from `INPUT`, writes the metered module
//! to `OUTPUT`, and exits 0; any failure prints one static message to stderr,
//! writes nothing, and exits 1.
//!
//! Telemetry: `main` is the CLI boundary and opens the one
//! `scxml.wasm-meter.cli` span, failed with the closed category of whichever
//! step failed; the nested `meter()` call opens its own library boundary span.

#![forbid(unsafe_code)]

use std::io::Read as _;
use std::process::ExitCode;

use scxml_telemetry::{ErrorCategory, Module, Operation, observe};
use scxml_wasm_meter::{MAX_MODULE_BYTES, meter};

const USAGE: &str = "usage: scxml-wasm-meter --initial-fuel N INPUT OUTPUT";

/// A CLI failure: its closed telemetry category and a static message.
type CliError = (ErrorCategory, &'static str);

fn main() -> ExitCode {
    let span = observe(Module::WasmMeter, Operation::WasmMeterCli);
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err((category, message)) => {
            span.fail(category);
            eprintln!("scxml-wasm-meter: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Runs inside `main`'s span; opens no span of its own.
fn run(args: &[String]) -> Result<(), CliError> {
    let [flag, fuel, input, output] = args else {
        return Err((ErrorCategory::InvalidInput, USAGE));
    };
    if flag != "--initial-fuel" {
        return Err((ErrorCategory::InvalidInput, USAGE));
    }
    let initial_fuel = fuel.parse::<u64>().map_err(|_| {
        (
            ErrorCategory::InvalidInput,
            "--initial-fuel must be a non-negative integer",
        )
    })?;
    let file = std::fs::File::open(input)
        .map_err(|_| (ErrorCategory::InputRead, "cannot open the input module"))?;
    let limit = u64::try_from(MAX_MODULE_BYTES)
        .map_err(|_| (ErrorCategory::Harness, "module size bound does not fit u64"))?
        .saturating_add(1);
    let mut bytes = Vec::new();
    file.take(limit)
        .read_to_end(&mut bytes)
        .map_err(|_| (ErrorCategory::InputRead, "cannot read the input module"))?;
    let metered =
        meter(&bytes, initial_fuel).map_err(|error| (error.category(), error.message()))?;
    std::fs::write(output, metered)
        .map_err(|_| (ErrorCategory::Report, "cannot write the output module"))
}
