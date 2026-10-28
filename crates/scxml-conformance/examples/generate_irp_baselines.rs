//! Regenerates every checked-in canonical IRP baseline by running each
//! prepared corpus through the shipped Wasm components: the W3C IRP
//! (`tests/conformance/w3c-irp`) and every repository namespace's own suite
//! (`namespaces/<name>/irp`, [`scxml_conformance::NAMESPACE_SUITES`]).
//!
//! Usage:
//!
//! ```text
//! cargo run -p scxml-conformance --features wasmtime \
//!   --example generate_irp_baselines -- \
//!   --components DIR --components-manifest-sha256 HEX \
//!   --extension-component-sha256 HEX
//! ```
//!
//! `--components DIR` names a release written by
//! `scripts/verify-wasm-capabilities.sh` (with `scxml_extension_example`) and
//! `--components-manifest-sha256 HEX` the trusted lowercase SHA-256 of its
//! `manifest.tsv`; the executor authenticates every component against it.
//! `--extension-component-sha256 HEX` is the trusted SHA-256 of the example
//! extension component in DIR, which the namespace suites bind. Every flag is required, each at
//! most once, and any other argument is rejected. The settings are read
//! only from these flags, as `scx conformance run` reads them.

#![forbid(unsafe_code)]

use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use scxml_conformance::{
    ExtensionRelease, NAMESPACE_SUITES, RunOptions, WasmExecutor, load_suite, run_suite,
    sha256_bytes,
};
use scxml_reporting::encode_canonical;

const DATA_MODELS: [&str; 2] = ["ecmascript", "xpath"];

const USAGE: &str = "usage: generate_irp_baselines --components DIR \
                     --components-manifest-sha256 HEX --extension-component-sha256 HEX";

/// The component release this run authenticates and executes.
#[derive(Debug, PartialEq, Eq)]
struct Args {
    components: PathBuf,
    manifest_sha256: String,
    extension_sha256: String,
}

/// Parses the command line (without the program name).
///
/// Rejects a missing, empty, or repeated flag, a flag without its value, and
/// any other argument, with a usage message naming the problem.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut components = None;
    let mut manifest_sha256 = None;
    let mut extension_sha256 = None;
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        let slot = match flag.as_str() {
            "--components" => &mut components,
            "--components-manifest-sha256" => &mut manifest_sha256,
            "--extension-component-sha256" => &mut extension_sha256,
            _ => return Err(format!("unknown argument `{flag}`\n{USAGE}")),
        };
        if slot.is_some() {
            return Err(format!("{flag} given more than once\n{USAGE}"));
        }
        let value = args
            .next()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("{flag} requires a value\n{USAGE}"))?;
        *slot = Some(value);
    }
    Ok(Args {
        components: PathBuf::from(
            components.ok_or_else(|| format!("--components DIR is required\n{USAGE}"))?,
        ),
        manifest_sha256: manifest_sha256
            .ok_or_else(|| format!("--components-manifest-sha256 HEX is required\n{USAGE}"))?,
        extension_sha256: extension_sha256
            .ok_or_else(|| format!("--extension-component-sha256 HEX is required\n{USAGE}"))?,
    })
}

/// Runs both data-model corpora under `root/prepared` through `executor` and
/// writes their canonical reports and `SHA256SUMS` to `root/baselines`.
fn regenerate(root: &Path, executor: &WasmExecutor) -> Result<(), Box<dyn Error>> {
    let output = root.join("baselines");
    fs::create_dir_all(&output)?;
    let mut checksums = String::new();
    for data_model in DATA_MODELS {
        let suite = load_suite(&root.join("prepared").join(data_model))?;
        let report = run_suite(&suite, &RunOptions::default(), executor)?;
        let bytes = encode_canonical(&report.canonical_artifact()?)?;
        let file_name = format!("{data_model}.scxml-report.json");
        fs::write(output.join(&file_name), &bytes)?;
        writeln!(checksums, "{}  {file_name}", sha256_bytes(&bytes))?;
    }
    fs::write(output.join("SHA256SUMS"), checksums)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args(std::env::args().skip(1))?;
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let executor = || WasmExecutor::from_release_dir(&args.components, &args.manifest_sha256);
    regenerate(&workspace.join("tests/conformance/w3c-irp"), &executor()?)?;
    for (name, packages) in NAMESPACE_SUITES {
        let releases = packages
            .iter()
            .map(|manifest| {
                ExtensionRelease::read(
                    &workspace.join(manifest),
                    &args.components,
                    &args.extension_sha256,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let executor = executor()?.with_extensions(releases)?;
        regenerate(
            &workspace.join("namespaces").join(name).join("irp"),
            &executor,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Args, parse_args};
    use std::path::PathBuf;

    fn parse(args: &[&str]) -> Result<Args, String> {
        parse_args(args.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn every_flag_in_any_order_is_accepted() {
        let expected = || {
            Ok(Args {
                components: PathBuf::from("release"),
                manifest_sha256: "ab".to_owned(),
                extension_sha256: "cd".to_owned(),
            })
        };
        assert_eq!(
            parse(&[
                "--components",
                "release",
                "--components-manifest-sha256",
                "ab",
                "--extension-component-sha256",
                "cd"
            ]),
            expected()
        );
        assert_eq!(
            parse(&[
                "--extension-component-sha256",
                "cd",
                "--components-manifest-sha256",
                "ab",
                "--components",
                "release"
            ]),
            expected()
        );
    }

    #[test]
    fn missing_repeated_empty_and_unknown_arguments_are_rejected() {
        for (args, message) in [
            (&[][..], "--components DIR is required"),
            (
                &["--components", "release"][..],
                "--components-manifest-sha256 HEX is required",
            ),
            (
                &[
                    "--components",
                    "release",
                    "--components-manifest-sha256",
                    "ab",
                ][..],
                "--extension-component-sha256 HEX is required",
            ),
            (&["--components"][..], "--components requires a value"),
            (&["--components", ""][..], "--components requires a value"),
            (
                &["--components", "a", "--components", "b"][..],
                "--components given more than once",
            ),
            (&["release"][..], "unknown argument `release`"),
        ] {
            let error = parse(args).expect_err("invalid command line is rejected");
            assert!(error.starts_with(message), "{args:?}: {error}");
            assert!(error.ends_with(super::USAGE), "{args:?}: usage is shown");
        }
    }
}
