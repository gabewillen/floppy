//! `wizer [--insecure-seed PROVIDER] INPUT OUTPUT`: pre-initializes one
//! component at build time ([`scxml_host_wasmtime::wizen_component`]).
//!
//! Reads the component at `INPUT` (and the seed provider component at
//! `PROVIDER`), writes the pre-initialized component to `OUTPUT`, and exits
//! 0; any failure prints the failure to stderr, writes nothing, and exits 1.
//! Usage errors exit 2.
//!
//! Telemetry: none of its own; the `wizen_component` call opens the
//! boundary span.

#![forbid(unsafe_code)]

use std::process::ExitCode;

const USAGE: &str = "usage: wizer [--insecure-seed PROVIDER] INPUT OUTPUT";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let (seed, input, output) = match arguments.as_slice() {
        [flag, seed, input, output] if flag == "--insecure-seed" => {
            (Some(seed.as_str()), input, output)
        }
        [input, output] if !input.starts_with('-') => (None, input, output),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let read = |path: &str| {
        std::fs::read(path).map_err(|error| format!("wizer: cannot read {path}: {error}"))
    };
    let result = (|| {
        let component = read(input)?;
        let seed = seed.map(read).transpose()?;
        let wizened = scxml_host_wasmtime::wizen_component(&component, seed.as_deref())
            .map_err(|error| format!("wizer: {input}: {error}"))?;
        std::fs::write(output, wizened)
            .map_err(|error| format!("wizer: cannot write {output}: {error}"))
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
