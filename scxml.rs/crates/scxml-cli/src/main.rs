//! Thin SCXML CLI composition root.

#![forbid(unsafe_code)]

mod components;
mod datamodels;
mod diagnostics;
mod documents;
mod env_file;
mod event_schemas;
mod expression;
mod extensions;
mod mask;
mod package_validators;
mod processors;
#[cfg_attr(
    not(feature = "wasmtime"),
    allow(
        dead_code,
        reason = "extension components resolve only when a session starts, which needs the wasmtime feature"
    )
)]
mod repository;
mod run;
mod scripts;
mod services;
mod snapshot;
mod snapshot_proof;

use std::env;
use std::fs;
use std::io::{self, IsTerminal as _, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use diagnostics::{OutputFormat, explain_code, parse_format, write_artifact};

use scxml_compiler_core::{ArtifactKind, CompileInput, Compiler};
use scxml_conformance::{
    ConformanceLevel, Disposition, Filters, IdRange, PrepareOptions, RunOptions, RunReport,
    SaxonTransformer, compare_reports, load_suite, prepare_corpus, run_suite,
};
use scxml_reporting::{ColorChoice, RustcRenderer, encode_canonical};
use scxml_telemetry::{ErrorCategory, Module, Operation, observe};
use scxml_validator_core::{FileSchemaResolver, SchemaMode, Validator};
use scxml_validator_xsd::{BundledSchemaResolver, XsdSchemaBackend};

/// The source id of the one document a validation reads: every span points into it.
const DOCUMENT_SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(1);

/// Pinned generic machine template for `--artifact component-wasm`.
#[derive(Debug)]
struct TemplateArgs {
    path: PathBuf,
    sha256: String,
}

/// Compile options beyond the source, schemas, and policy.
#[derive(Debug)]
struct CompileTarget {
    output: PathBuf,
    artifact: ArtifactKind,
    template: Option<TemplateArgs>,
    canonical: Option<PathBuf>,
    scripts: scripts::ScriptFlags,
}

/// Exit status of a completed run whose report contains errors.
const EXIT_DIAGNOSTICS: u8 = 1;
/// Exit status for usage, I/O, or internal failures.
const EXIT_FAILURE: u8 = 2;

/// Marker error: diagnostics were already rendered; only the exit status remains.
#[derive(Debug)]
struct DiagnosticsReported;

impl std::fmt::Display for DiagnosticsReported {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("validation failed")
    }
}

impl std::error::Error for DiagnosticsReported {}

/// Marker error: baseline mismatches were already listed; only the exit
/// status remains.
#[derive(Debug)]
struct BaselineMismatchReported;

impl std::fmt::Display for BaselineMismatchReported {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("conformance run does not reproduce the canonical baseline")
    }
}

impl std::error::Error for BaselineMismatchReported {}

fn main() -> ExitCode {
    let span = observe(Module::Cli, Operation::CliMain);
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error.is::<run::Exited>() => {
            // `run` already wrote its outcome as records; only the status
            // remains.
            let status = error
                .downcast_ref::<run::Exited>()
                .map_or(EXIT_FAILURE, |exited| exited.0);
            span.fail(ErrorCategory::Execution);
            ExitCode::from(status)
        }
        Err(error) if error.is::<DiagnosticsReported>() => {
            span.fail(ErrorCategory::Validation);
            ExitCode::from(EXIT_DIAGNOSTICS)
        }
        Err(error) if error.is::<BaselineMismatchReported>() => {
            span.fail(ErrorCategory::Oracle);
            ExitCode::from(EXIT_DIAGNOSTICS)
        }
        Err(error)
            if error.is::<snapshot::IssuesReported>()
                || error.is::<snapshot::DocumentsDiffer>() =>
        {
            span.fail(ErrorCategory::Validation);
            ExitCode::from(EXIT_DIAGNOSTICS)
        }
        Err(error) if error.is::<snapshot::DecodeFailure>() => {
            span.fail(ErrorCategory::Validation);
            if let Some(failure) = error.downcast_ref::<snapshot::DecodeFailure>() {
                // Terminal error sink, as below.
                let _ = io::stderr().write_all(failure.rendered().as_bytes());
            }
            ExitCode::from(EXIT_FAILURE)
        }
        Err(error) => {
            span.fail(ErrorCategory::Harness);
            // Terminal error sink: the exit code already reports the
            // failure, and a failed stderr write has no further channel to
            // report to, so its result is deliberately ignored.
            let _ = writeln!(io::stderr(), "error: {}", mask::scrub(&error.to_string()));
            ExitCode::from(EXIT_FAILURE)
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    match parse_args(&args)? {
        Command::Validate {
            path,
            schemas,
            packages,
            services,
            data_models,
            repository,
            mode,
            canonical,
            format,
        } => validate(
            &path,
            &SchemaFlags {
                schemas,
                packages,
                services,
                data_models,
                repository,
            },
            mode,
            canonical.as_deref(),
            format,
        ),
        Command::Explain(code) => {
            let text = explain_code(&code)?;
            io::stdout().lock().write_all(text.as_bytes())?;
            Ok(())
        }
        Command::Compile {
            path,
            target,
            schemas,
            packages,
            services,
            data_models,
            repository,
            mode,
        } => compile(
            &path,
            &target,
            &SchemaFlags {
                schemas,
                packages,
                services,
                data_models,
                repository,
            },
            mode,
        ),
        Command::Snapshot(command) => snapshot::execute(*command),
        Command::Run(args) => {
            let span = observe(Module::Cli, Operation::CliRun);
            let result = run::execute(&args);
            if let Err(error) = &result
                && !error.is::<run::Exited>()
            {
                span.fail(ErrorCategory::Harness);
            }
            result
        }
        Command::Conformance(options) => match *options {
            ConformanceCommand::Prepare(options) => conformance_prepare(&options),
            ConformanceCommand::Run(options) => conformance_run(&options),
        },
        Command::Help => {
            print_usage();
            Ok(())
        }
    }
}

#[derive(Debug)]
enum Command {
    Validate {
        path: PathBuf,
        schemas: Vec<(String, PathBuf)>,
        packages: Vec<PathBuf>,
        services: Vec<PathBuf>,
        data_models: Vec<PathBuf>,
        repository: repository::RepositoryFlags,
        mode: SchemaMode,
        canonical: Option<PathBuf>,
        format: OutputFormat,
    },
    Explain(String),
    Compile {
        path: PathBuf,
        target: CompileTarget,
        schemas: Vec<(String, PathBuf)>,
        packages: Vec<PathBuf>,
        services: Vec<PathBuf>,
        data_models: Vec<PathBuf>,
        repository: repository::RepositoryFlags,
        mode: SchemaMode,
    },
    Conformance(Box<ConformanceCommand>),
    Snapshot(Box<snapshot::SnapshotCommand>),
    Run(Box<run::RunArgs>),
    Help,
}

#[derive(Debug)]
enum ConformanceCommand {
    Prepare(ConformancePrepareOptions),
    Run(ConformanceRunOptions),
}

#[derive(Debug)]
struct ConformancePrepareOptions {
    options: PrepareOptions,
    saxon: Vec<String>,
}

#[derive(Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool mirrors one independent CLI flag (exit policy and the snapshot proof mode); they are orthogonal and combining them would obscure the one-to-one mapping to user-facing options"
)]
struct ConformanceRunOptions {
    corpus: PathBuf,
    components: components::ComponentArgs,
    /// `--extension-package` and `--extension-component-sha256`: the
    /// extension namespaces a namespace suite binds.
    extensions: components::ExtensionArgs,
    canonical_baseline: Option<PathBuf>,
    filters: Filters,
    json: Option<PathBuf>,
    /// `--report-copy PATH`: a second copy of the canonical report JSON.
    report_copy: Option<PathBuf>,
    baseline: Option<PathBuf>,
    fail_on_regression: bool,
    allow_not_passing: bool,
    allow_baseline_mismatch: bool,
    git_commit: Option<String>,
    /// `--snapshot-every-checkpoint`: collect one read-only session-snapshot
    /// document per reached checkpoint of every case, prove the snapshots
    /// do not perturb the run, check every document, prove two runs produce
    /// identical documents, and write the per-model summary to
    /// `target/snapshot-proof/<datamodel>.json` under the current directory
    /// (docs/session-snapshot.md, "Non-perturbation proof over the IRP").
    /// The plain run's report is then judged like any other run.
    snapshot_every_checkpoint: bool,
}

#[allow(
    clippy::too_many_lines,
    reason = "one dispatch over every command and its options"
)]
fn parse_args(args: &[String]) -> Result<Command, String> {
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(Command::Help);
    };
    if matches!(command, "help" | "--help" | "-h") {
        return Ok(Command::Help);
    }
    if matches!(command, "--explain" | "explain") {
        return diagnostics::parse_explain(&args[1..]).map(Command::Explain);
    }
    if command == "snapshot" {
        if args
            .iter()
            .skip(1)
            .take_while(|argument| *argument != "--")
            .any(|argument| matches!(argument.as_str(), "--help" | "-h"))
        {
            return Ok(Command::Help);
        }
        return snapshot::parse(&args[1..]).map(|command| Command::Snapshot(Box::new(command)));
    }
    if command == "run" {
        if args
            .iter()
            .skip(1)
            .take_while(|argument| *argument != "--")
            .any(|argument| matches!(argument.as_str(), "--help" | "-h"))
        {
            return Ok(Command::Help);
        }
        return run::parse(&args[1..]).map(|args| Command::Run(Box::new(args)));
    }
    if command == "conformance" {
        return parse_conformance(&args[1..])
            .map(|command| Command::Conformance(Box::new(command)));
    }
    if !matches!(command, "validate" | "compile") {
        return Err(format!("unknown command `{command}`"));
    }
    if args
        .iter()
        .skip(1)
        .take_while(|argument| *argument != "--")
        .any(|argument| matches!(argument.as_str(), "--help" | "-h"))
    {
        return Ok(Command::Help);
    }
    let CommandOptions {
        path,
        schemas,
        packages,
        services,
        data_models,
        repository,
        mode,
        output,
        canonical,
        packaging,
        format,
    } = CommandOptions::parse(command, &args[1..])?;
    let path = path.ok_or_else(|| format!("{command} requires PATH"))?;
    match command {
        "validate" => {
            if packaging.is_set() {
                return Err(COMPILE_ONLY_FLAGS.into());
            }
            Ok(Command::Validate {
                path: path.into(),
                schemas,
                packages,
                services,
                data_models,
                repository,
                mode,
                canonical,
                format: format.unwrap_or_default(),
            })
        }
        "compile" => {
            if format.is_some() {
                return Err("--format is valid only for validate".into());
            }
            let artifact = packaging.artifact.unwrap_or(ArtifactKind::LoweredIr);
            let template =
                bind_template(artifact, packaging.template_path, packaging.template_sha256)?;
            Ok(Command::Compile {
                path: path.into(),
                target: CompileTarget {
                    output: output.ok_or("compile requires --output PATH")?,
                    artifact,
                    template,
                    canonical,
                    scripts: packaging.scripts,
                },
                schemas,
                packages,
                services,
                data_models,
                repository,
                mode,
            })
        }
        _ => Err(format!("unknown command `{command}`")),
    }
}

/// PATH and options of `validate`/`compile`. Options take one value each and
/// may appear before or after PATH, as in `cargo`/`rustc` invocations.
struct CommandOptions<'a> {
    path: Option<&'a String>,
    schemas: Vec<(String, PathBuf)>,
    packages: Vec<PathBuf>,
    services: Vec<PathBuf>,
    data_models: Vec<PathBuf>,
    repository: repository::RepositoryFlags,
    mode: SchemaMode,
    output: Option<PathBuf>,
    canonical: Option<PathBuf>,
    packaging: PackagingFlags,
    format: Option<OutputFormat>,
}

impl<'a> CommandOptions<'a> {
    fn parse(command: &str, args: &'a [String]) -> Result<Self, String> {
        let mut options = Self {
            path: None,
            schemas: Vec::new(),
            packages: Vec::new(),
            services: Vec::new(),
            data_models: Vec::new(),
            repository: repository::RepositoryFlags::default(),
            mode: SchemaMode::Strict,
            output: None,
            canonical: None,
            packaging: PackagingFlags::default(),
            format: None,
        };
        let mut index = 0;
        let mut options_ended = false;
        while let Some(argument) = args.get(index) {
            let value = args.get(index + 1);
            if options_ended {
                options.operand(command, argument)?;
                index += 1;
                continue;
            }
            match argument.as_str() {
                "--" => {
                    options_ended = true;
                    index += 1;
                    continue;
                }
                "-" => {
                    return Err(format!(
                        "`{command}` does not read standard input: pass the SCXML file's PATH"
                    ));
                }
                repository::RepositoryFlags::OFFLINE => {
                    options.repository.accept(argument, "")?;
                    index += 1;
                    continue;
                }
                flag if repository::RepositoryFlags::expects(flag).is_some() => {
                    let expected = repository::RepositoryFlags::expects(flag).unwrap_or("a value");
                    options.repository.accept(
                        flag,
                        value.ok_or_else(|| format!("{flag} requires {expected}"))?,
                    )?;
                }
                "--schema" => options.schemas.push(parse_schema_mapping(value)?),
                "--extension-package" => options.packages.push(PathBuf::from(
                    value.ok_or("--extension-package requires MANIFEST.json")?,
                )),
                "--service-package" => options.services.push(PathBuf::from(
                    value.ok_or("--service-package requires MANIFEST.json")?,
                )),
                "--data-model-package" => options.data_models.push(PathBuf::from(
                    value.ok_or("--data-model-package requires MANIFEST.json")?,
                )),
                "--policy" => options.mode = parse_schema_mode(value)?,
                "--format" => options.format = Some(parse_format(value)?),
                "--output" => {
                    options.output = Some(PathBuf::from(value.ok_or("--output requires PATH")?));
                }
                "--canonical" => {
                    options.canonical =
                        Some(PathBuf::from(value.ok_or("--canonical requires PATH")?));
                }
                flag if PackagingFlags::is_compile_flag(flag) => {
                    options.packaging.accept(flag, value)?;
                }
                // Every other argument that starts with `-` is an option this
                // command does not have, never the PATH (a PATH that starts
                // with `-` follows `--`).
                unknown if unknown.starts_with('-') => {
                    return Err(unknown_option(unknown, COMMAND_OPTIONS));
                }
                _ => {
                    options.operand(command, argument)?;
                    index += 1;
                    continue;
                }
            }
            index += 2;
        }
        Ok(options)
    }

    /// Records the one PATH operand.
    fn operand(&mut self, command: &str, argument: &'a String) -> Result<(), String> {
        if let Some(first) = self.path {
            return Err(format!(
                "unexpected argument `{argument}`: `{command}` takes one PATH (already given `{first}`)"
            ));
        }
        self.path = Some(argument);
        Ok(())
    }
}

/// Options `validate` and `compile` accept (each command rejects the other's
/// with its own error).
const COMMAND_OPTIONS: &[&str] = &[
    "--schema",
    "--extension-package",
    "--service-package",
    "--data-model-package",
    "--offline",
    "--cache-dir",
    "--github-api-base",
    "--github-raw-base",
    "--github-download-base",
    "--github-token-env",
    "--policy",
    "--format",
    "--output",
    "--canonical",
    "--artifact",
    "--machine-template",
    "--machine-template-sha256",
    "--script",
    "--script-sha256",
    "--script-resolution",
];

/// Options of `conformance prepare`.
const PREPARE_OPTIONS: &[&str] = &[
    "--manifest",
    "--base-uri",
    "--templates",
    "--datamodel",
    "--cache",
    "--output",
    "--source-dir",
    "--revision",
    "--saxon",
    "--errata",
    "--namespace",
    "--max-commands",
    "--offline",
    "--force",
];

/// Options of `conformance run`.
const RUN_OPTIONS: &[&str] = &[
    "--corpus",
    "--components",
    "--components-manifest-sha256",
    "--extension-package",
    "--extension-component-sha256",
    "--canonical-baseline",
    "--id",
    "--id-range",
    "--spec",
    "--spec-id",
    "--spec-num",
    "--mandatory",
    "--optional",
    "--datamodel",
    "--tag",
    "--json",
    "--report-copy",
    "--baseline",
    "--git-commit",
    "--fail-on-regression",
    "--allow-not-passing",
    "--allow-baseline-mismatch",
    "--snapshot-every-checkpoint",
];

/// Rejects an argument that is not one of `known`, rustc-style: the error,
/// then a `help` line naming the closest known option (`-o` is `--output`;
/// a near-miss such as `--fromat` is `--format`) or where the options are
/// listed.
fn unknown_option(argument: &str, known: &[&str]) -> String {
    let name = argument.trim_start_matches('-');
    let short = argument.len() == 2 && !argument.starts_with("--");
    let suggestion = if short {
        // The repository flags came later and are never what a single
        // letter abbreviates (`-o` stays `--output`, not `--offline`).
        let mut matching = known.iter().filter(|option| {
            !repository::RepositoryFlags::is_repository_flag(option)
                && option
                    .strip_prefix("--")
                    .is_some_and(|long| long.starts_with(name))
        });
        match (matching.next(), matching.next()) {
            (Some(option), None) => Some(*option),
            _ => None,
        }
    } else {
        known
            .iter()
            .map(|option| (diagnostics::distance(argument, option), *option))
            .filter(|(distance, _)| *distance <= 2)
            .min()
            .map(|(_, option)| option)
    };
    match suggestion {
        Some(option) if short => format!(
            "unknown option `{argument}`\n  = help: did you mean `{option}`? (scx has only long options)"
        ),
        Some(option) => format!("unknown option `{argument}`\n  = help: did you mean `{option}`?"),
        None => format!(
            "unknown option `{argument}`\n  = help: run `scx --help` for the accepted options; a PATH that starts with `-` goes after `--`"
        ),
    }
}

/// Parses one `--schema URI=PATH` exact-namespace mapping.
fn parse_schema_mapping(value: Option<&String>) -> Result<(String, PathBuf), String> {
    value
        .and_then(|mapping| mapping.split_once('='))
        .ok_or_else(|| "--schema requires URI=PATH".to_owned())
        .and_then(|(uri, path)| {
            if uri.is_empty() || path.is_empty() {
                Err("--schema requires non-empty URI=PATH".to_owned())
            } else {
                Ok((uri.to_owned(), PathBuf::from(path)))
            }
        })
}

/// Rejection for compile-only flags passed to `validate`.
const COMPILE_ONLY_FLAGS: &str =
    "--artifact, --machine-template*, and --script* are valid only for compile";

/// Compile-only packaging and script flags collected by [`parse_args`].
#[derive(Debug, Default)]
struct PackagingFlags {
    artifact: Option<ArtifactKind>,
    template_path: Option<PathBuf>,
    template_sha256: Option<String>,
    scripts: scripts::ScriptFlags,
}

impl PackagingFlags {
    /// Whether `flag` is a compile-only packaging or script flag.
    fn is_compile_flag(flag: &str) -> bool {
        matches!(
            flag,
            "--artifact" | "--machine-template" | "--machine-template-sha256"
        ) || scripts::ScriptFlags::is_script_flag(flag)
    }

    /// Records one packaging flag and its required value.
    fn accept(&mut self, flag: &str, value: Option<&String>) -> Result<(), String> {
        match flag {
            "--artifact" => parse_artifact(value).map(|kind| self.artifact = Some(kind)),
            "--machine-template" => value
                .map(|path| self.template_path = Some(PathBuf::from(path)))
                .ok_or_else(|| "--machine-template requires PATH".to_owned()),
            flag if scripts::ScriptFlags::is_script_flag(flag) => self.scripts.accept(flag, value),
            _ => value
                .map(|sha256| self.template_sha256 = Some(sha256.clone()))
                .ok_or_else(|| "--machine-template-sha256 requires HEX".to_owned()),
        }
    }

    /// Whether any packaging flag was supplied.
    fn is_set(&self) -> bool {
        self.artifact.is_some()
            || self.template_path.is_some()
            || self.template_sha256.is_some()
            || self.scripts.is_set()
    }
}

/// Parses `--artifact`; `core-wasm` is rejected because it is never produced.
fn parse_artifact(value: Option<&String>) -> Result<ArtifactKind, String> {
    match value.map(String::as_str) {
        Some("lowered-ir") => Ok(ArtifactKind::LoweredIr),
        Some("component-wasm") => Ok(ArtifactKind::ComponentWasm),
        Some("core-wasm") => Err("--artifact core-wasm is not produced: packaging composes a component and never emits a standalone core module".into()),
        _ => Err("--artifact requires lowered-ir|component-wasm".into()),
    }
}

/// Requires the template flags exactly for `component-wasm`.
fn bind_template(
    artifact: ArtifactKind,
    path: Option<PathBuf>,
    sha256: Option<String>,
) -> Result<Option<TemplateArgs>, String> {
    match (artifact, path, sha256) {
        (ArtifactKind::ComponentWasm, Some(path), Some(sha256)) => {
            Ok(Some(TemplateArgs { path, sha256 }))
        }
        (ArtifactKind::ComponentWasm, _, _) => Err("--artifact component-wasm requires --machine-template PATH and --machine-template-sha256 HEX".into()),
        (_, None, None) => Ok(None),
        _ => Err("--machine-template* is valid only with --artifact component-wasm".into()),
    }
}

fn parse_conformance(args: &[String]) -> Result<ConformanceCommand, String> {
    match args.first().map(String::as_str) {
        Some("prepare") => parse_conformance_prepare(&args[1..]).map(ConformanceCommand::Prepare),
        Some("run") => parse_conformance_run(&args[1..]).map(ConformanceCommand::Run),
        Some(other) => Err(format!("unknown conformance command `{other}`")),
        None => Err("conformance requires prepare or run".into()),
    }
}

fn parse_conformance_prepare(args: &[String]) -> Result<ConformancePrepareOptions, String> {
    let mut manifest = None;
    let mut base_uri = None;
    let mut templates = None;
    let mut datamodel = None;
    let mut cache = None;
    let mut output = None;
    let mut source_dir = None;
    let mut revision = None;
    let mut saxon = None;
    let mut errata = None;
    let mut namespace = None;
    let mut offline = false;
    let mut force = false;
    let mut max_commands = 1_024_u64;
    let mut index = 0;
    while index < args.len() {
        let value = args.get(index + 1);
        match args[index].as_str() {
            "--manifest" => manifest = Some(required(value, "--manifest")?.clone()),
            "--base-uri" => base_uri = Some(required(value, "--base-uri")?.clone()),
            "--templates" => templates = Some(PathBuf::from(required(value, "--templates")?)),
            "--datamodel" => datamodel = Some(required(value, "--datamodel")?.clone()),
            "--cache" => cache = Some(PathBuf::from(required(value, "--cache")?)),
            "--output" => output = Some(PathBuf::from(required(value, "--output")?)),
            "--source-dir" => {
                source_dir = Some(PathBuf::from(required(value, "--source-dir")?));
            }
            "--revision" => revision = Some(required(value, "--revision")?.clone()),
            "--saxon" => saxon = Some(parse_command(required(value, "--saxon")?)?),
            "--errata" => errata = Some(PathBuf::from(required(value, "--errata")?)),
            "--namespace" => namespace = Some(required(value, "--namespace")?.clone()),
            "--max-commands" => {
                max_commands = required(value, "--max-commands")?
                    .parse()
                    .map_err(|_| "--max-commands requires a positive integer")?;
            }
            "--offline" => {
                offline = true;
                index += 1;
                continue;
            }
            "--force" => {
                force = true;
                index += 1;
                continue;
            }
            unknown => return Err(unknown_option(unknown, PREPARE_OPTIONS)),
        }
        index += 2;
    }
    let cache = cache.ok_or("conformance prepare requires --cache DIR")?;
    Ok(ConformancePrepareOptions {
        options: PrepareOptions {
            manifest: manifest.ok_or("conformance prepare requires --manifest URL|PATH")?,
            base_uri,
            templates: templates.ok_or("conformance prepare requires --templates DIR")?,
            datamodel: datamodel.ok_or("conformance prepare requires --datamodel NAME")?,
            output: output.unwrap_or_else(|| cache.join("prepared")),
            cache,
            source_dir,
            revision,
            offline,
            force,
            max_commands,
            errata,
            namespace,
        },
        saxon: saxon.ok_or("conformance prepare requires explicit --saxon CMD")?,
    })
}

#[allow(
    clippy::too_many_lines,
    reason = "one flat loop maps every conformance run flag to its option"
)]
fn parse_conformance_run(args: &[String]) -> Result<ConformanceRunOptions, String> {
    let mut corpus = None;
    let mut filters = Filters::default();
    let mut json = None;
    let mut report_copy = None;
    let mut baseline = None;
    let mut fail_on_regression = false;
    let mut allow_not_passing = false;
    let mut allow_baseline_mismatch = false;
    let mut git_commit = None;
    let mut components_dir = None;
    let mut components_digest = None;
    let mut extensions = components::ExtensionArgs::default();
    let mut canonical_baseline = None;
    let mut snapshot_every_checkpoint = false;
    let mut index = 0;
    while index < args.len() {
        let value = args.get(index + 1);
        match args[index].as_str() {
            "--corpus" => corpus = Some(PathBuf::from(required(value, "--corpus")?)),
            "--components" => {
                components_dir = Some(PathBuf::from(required(value, "--components")?));
            }
            "--components-manifest-sha256" => {
                components_digest = Some(required(value, "--components-manifest-sha256")?.clone());
            }
            "--extension-package" => extensions
                .manifests
                .push(PathBuf::from(required(value, "--extension-package")?)),
            "--extension-component-sha256" => {
                extensions.digest(required(value, "--extension-component-sha256")?)?;
            }
            "--canonical-baseline" => {
                canonical_baseline = Some(PathBuf::from(required(value, "--canonical-baseline")?));
            }
            "--id" => filters.ids.push(required(value, "--id")?.clone()),
            "--id-range" => filters
                .id_ranges
                .push(parse_id_range(required(value, "--id-range")?)?),
            "--spec" | "--spec-id" => filters.spec_ids.push(required(value, "--spec")?.clone()),
            "--spec-num" => filters
                .spec_nums
                .push(required(value, "--spec-num")?.clone()),
            "--mandatory" => {
                filters.conformance.push(ConformanceLevel::Mandatory);
                index += 1;
                continue;
            }
            "--optional" => {
                filters.conformance.push(ConformanceLevel::Optional);
                index += 1;
                continue;
            }
            "--datamodel" => filters
                .datamodels
                .push(required(value, "--datamodel")?.clone()),
            "--tag" => filters.tags.push(required(value, "--tag")?.clone()),
            "--json" => json = Some(PathBuf::from(required(value, "--json")?)),
            "--report-copy" => {
                report_copy = Some(PathBuf::from(required(value, "--report-copy")?));
            }
            "--baseline" => baseline = Some(PathBuf::from(required(value, "--baseline")?)),
            "--git-commit" => git_commit = Some(required(value, "--git-commit")?.clone()),
            "--fail-on-regression" => {
                fail_on_regression = true;
                index += 1;
                continue;
            }
            "--allow-not-passing" => {
                allow_not_passing = true;
                index += 1;
                continue;
            }
            "--allow-baseline-mismatch" => {
                allow_baseline_mismatch = true;
                index += 1;
                continue;
            }
            "--snapshot-every-checkpoint" => {
                snapshot_every_checkpoint = true;
                index += 1;
                continue;
            }
            unknown => return Err(unknown_option(unknown, RUN_OPTIONS)),
        }
        index += 2;
    }
    Ok(ConformanceRunOptions {
        corpus: corpus.ok_or("conformance run requires --corpus DIR")?,
        components: components::ComponentArgs::resolve(components_dir, components_digest)?,
        extensions,
        canonical_baseline,
        filters,
        json,
        report_copy,
        baseline,
        fail_on_regression,
        allow_not_passing,
        allow_baseline_mismatch,
        git_commit,
        snapshot_every_checkpoint,
    })
}

fn required<'a>(value: Option<&'a String>, flag: &str) -> Result<&'a String, String> {
    value.ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_command(value: &str) -> Result<Vec<String>, String> {
    let command = value
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if command.is_empty() {
        Err("--saxon requires a non-empty command".into())
    } else {
        Ok(command)
    }
}

fn parse_id_range(value: &str) -> Result<IdRange, String> {
    let (start, end) = value
        .split_once("..=")
        .or_else(|| value.split_once('-'))
        .ok_or("--id-range requires START-END or START..=END")?;
    let range = IdRange {
        start: start
            .parse()
            .map_err(|_| "--id-range start must be numeric")?,
        end: end.parse().map_err(|_| "--id-range end must be numeric")?,
    };
    if range.start > range.end {
        return Err("--id-range start must not exceed end".into());
    }
    Ok(range)
}

fn parse_schema_mode(value: Option<&String>) -> Result<SchemaMode, String> {
    match value.map(String::as_str) {
        Some("strict") => Ok(SchemaMode::Strict),
        Some("lax") => Ok(SchemaMode::Lax),
        Some("off") => Ok(SchemaMode::Off),
        _ => Err("--policy requires strict, lax, or off".into()),
    }
}

fn validate(
    path: &Path,
    schemas: &SchemaFlags,
    mode: SchemaMode,
    canonical: Option<&Path>,
    format: OutputFormat,
) -> Result<(), Box<dyn std::error::Error>> {
    let span = observe(Module::Cli, Operation::CliValidate);
    let result = (|| {
        let source = read_source(path)?;
        let resolved = schemas.packages(&source)?;
        let services =
            services::resolve(&source, &[], &schemas.services, &schemas.repository)?.packages;
        let data_models =
            datamodels::resolve(&source, &[], &schemas.data_models, &schemas.repository)?;
        // JSON and SARIF keep stderr clean; the human report lists the
        // elements first.
        if matches!(format, OutputFormat::Human) {
            extensions::write_notes(&resolved.summary());
            extensions::write_notes(&services::summary(&services, None, None));
            extensions::write_notes(&data_models.summary());
        }
        let packages = resolved.all();
        let mut validator = Validator::new(schemas.resolver(&packages), XsdSchemaBackend)
            .with_services(services::bound_services(&services))
            .with_data_models(data_models.bound())
            .with_event_schema_resources(event_schemas::load(path, &source))
            .with_invoked_documents(documents::local_invoked_documents(path, &source))
            .with_script_resources(documents::local_script_files(path, &source))
            .with_schema_mode(mode)
            .with_expression_checker(std::sync::Arc::new(expression::EcmaScriptSyntax))
            .with_expression_checker(std::sync::Arc::new(expression::XPathSyntax));
        if let Some(linked) = package_validators::linked(&packages, &services)? {
            validator = validator.with_package_validator(linked);
        }
        let name = path.display().to_string();
        // Expression parsing is recursive; run the bounded validation on a
        // thread with an explicit stack reservation instead of the ambient one.
        let outcome = std::thread::scope(|scope| {
            std::thread::Builder::new()
                .stack_size(expression::VALIDATION_STACK_BYTES)
                .spawn_scoped(scope, || {
                    validator.validate_bytes(DOCUMENT_SOURCE, name, &source)
                })
                .map_err(|error| format!("could not start the validation thread: {error}"))?
                .join()
                .map_err(|_| "the validation thread failed".to_owned())
        })?;
        let artifact = outcome.report.canonical_artifact(&outcome.source)?;
        if let Some(canonical) = canonical {
            fs::write(canonical, encode_canonical(&artifact)?)?;
        }
        write_artifact(&artifact, format)?;
        if outcome.report.has_errors() {
            Err(Box::new(DiagnosticsReported) as Box<dyn std::error::Error>)
        } else {
            if format == OutputFormat::Human {
                writeln!(io::stderr(), "validated {}", path.display())?;
            }
            Ok(())
        }
    })();
    if result.is_err() {
        span.fail(ErrorCategory::Validation);
    }
    result
}

#[allow(
    clippy::too_many_lines,
    reason = "one ordered pipeline: packages, services, data models, inputs, validator, compile, outputs"
)]
fn compile(
    path: &Path,
    target: &CompileTarget,
    schemas: &SchemaFlags,
    mode: SchemaMode,
) -> Result<(), Box<dyn std::error::Error>> {
    let span = observe(Module::Cli, Operation::CliCompile);
    let result = (|| {
        let bytes = read_source(path)?;
        let resolved = schemas.packages(&bytes)?;
        let services =
            services::resolve(&bytes, &[], &schemas.services, &schemas.repository)?.packages;
        let data_models =
            datamodels::resolve(&bytes, &[], &schemas.data_models, &schemas.repository)?;
        extensions::write_notes(&resolved.summary());
        extensions::write_notes(&services::summary(&services, None, None));
        extensions::write_notes(&data_models.summary());
        let extension_packages = resolved.all();
        let event_schemas = event_schemas::load(path, &bytes);
        let invoked_documents = documents::local_invoked_documents(path, &bytes);
        let script_files = documents::local_script_files(path, &bytes);
        let source = utf8_source(path, bytes)?;
        // The template is read and bounded before compilation so a missing
        // or oversized template never yields a partial artifact.
        let template = target
            .template
            .as_ref()
            .map(|template| read_template(&template.path))
            .transpose()?;
        let script_packages = target.scripts.load()?;
        let (manifests, package_schemas) = extensions::compile_inputs(&extension_packages);
        let wasm_scripts = script_packages
            .iter()
            .filter(|script| script.media_type == scxml_script::WASM_MEDIA_TYPE)
            .map(|script| (script.uri.clone(), script.bytes.clone()))
            .collect::<Vec<_>>();
        // Compile validates with the same expression checker as `validate`,
        // so a document `validate` rejects is never compiled.
        let mut validator = Validator::new(schemas.resolver(&extension_packages), XsdSchemaBackend)
            .with_schema_mode(mode)
            .with_invoked_documents(invoked_documents)
            .with_script_resources(script_files)
            .with_services(services::bound_services(&services))
            .with_data_models(data_models.bound())
            .with_expression_checker(std::sync::Arc::new(expression::EcmaScriptSyntax))
            .with_expression_checker(std::sync::Arc::new(expression::XPathSyntax));
        if let Some(linked) = package_validators::linked(&extension_packages, &services)? {
            validator = validator.with_package_validator(linked);
        }
        let compiler = Compiler::new(validator);
        let input = CompileInput {
            source_name: path.display().to_string(),
            source,
            packages: manifests,
            package_schemas,
            scripts: script_packages,
            script_resolution: target.scripts.resolution(),
            event_schemas,
        };
        let mut result = std::thread::scope(|scope| {
            std::thread::Builder::new()
                .stack_size(expression::VALIDATION_STACK_BYTES)
                .spawn_scoped(scope, || compiler.compile(input))
                .map_err(|error| format!("could not start the compilation thread: {error}"))?
                .join()
                .map_err(|_| "the compilation thread failed".to_owned())
        })?;
        if let (Some(bytes), Some(args), Some(lowered)) = (
            template.as_ref(),
            target.template.as_ref(),
            result.artifact.as_ref(),
        ) {
            let packaged = scxml_packager::package(
                lowered,
                scxml_packager::MachineTemplate {
                    bytes,
                    sha256: &args.sha256,
                },
                &wasm_scripts
                    .iter()
                    .map(|(uri, bytes)| scxml_packager::ScriptComponent { uri, bytes })
                    .collect::<Vec<_>>(),
            )?;
            result.artifact = Some(packaged);
        }
        let artifact = result.canonical_artifact()?;
        if let Some(canonical) = target.canonical.as_deref() {
            fs::write(canonical, encode_canonical(&artifact)?)?;
        }
        emit(&artifact)?;
        let Some(artifact) = result.artifact else {
            return Err("compilation stopped after diagnostics".into());
        };
        if artifact.kind != target.artifact {
            return Err("compiler produced an unexpected artifact phase".into());
        }
        fs::write(&target.output, &artifact.bytes)?;
        let recipe_path = PathBuf::from(format!("{}.recipe.json", target.output.display()));
        fs::write(&recipe_path, serde_json::to_vec_pretty(&artifact.recipe)?)?;
        match artifact.kind {
            ArtifactKind::LoweredIr => println!(
                "wrote lowered Machine IR (portable data, not WebAssembly) to {} and its package recipe to {}",
                target.output.display(),
                recipe_path.display()
            ),
            ArtifactKind::ComponentWasm => println!(
                "wrote validated component-wasm (generic machine template composed with a data-only ir-source provider) to {} and its package recipe to {}",
                target.output.display(),
                recipe_path.display()
            ),
            ArtifactKind::CoreWasm => return Err("core-wasm is never produced".into()),
        }
        Ok(())
    })();
    if result.is_err() {
        span.fail(ErrorCategory::Lowering);
    }
    result
}

fn read_template(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let span = observe(Module::Cli, Operation::CliReadTemplate);
    let result = (|| {
        let limit = u64::try_from(scxml_packager::MAX_TEMPLATE_BYTES)?;
        let Some((file, metadata)) =
            open_regular_file(path)?.filter(|(_, metadata)| metadata.len() <= limit)
        else {
            return Err(
                "machine template must be a regular file within the 8 MiB byte bound".into(),
            );
        };
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len())?);
        file.take(limit + 1).read_to_end(&mut bytes)?;
        if u64::try_from(bytes.len())? > limit {
            return Err("machine template exceeds the 8 MiB byte bound".into());
        }
        Ok(bytes)
    })();
    if result.is_err() {
        span.event(scxml_telemetry::SignalEvent::InputRejected);
        span.fail(ErrorCategory::InputRead);
    } else {
        span.event(scxml_telemetry::SignalEvent::ExternalReadCompleted);
    }
    result
}

/// Opens `path` with its metadata only when it is a regular file: the path is
/// checked before the open, since opening a FIFO blocks and a device reads
/// without end, and the handle after it, in case the path changed between.
/// `None` when either is not a regular file.
pub(crate) fn open_regular_file(path: &Path) -> io::Result<Option<(fs::File, fs::Metadata)>> {
    if !fs::metadata(path)?.is_file() {
        return Ok(None);
    }
    let file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    Ok(metadata.is_file().then_some((file, metadata)))
}

/// Reads the bytes of one SCXML source; every failure names the path. Decoding is the caller's: `validate` reports invalid UTF-8 as a
/// located diagnostic in every output format.
fn read_source(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let span = observe(Module::Cli, Operation::CliReadSource);
    let shown = path.display();
    let result = (|| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let (mut file, metadata) = open_regular_file(path)
            .map_err(|error| format!("cannot read `{shown}`: {error}"))?
            .ok_or_else(|| format!("cannot read `{shown}`: not a regular file"))?;
        // The native CLI reads a source of any size: resource bounds apply
        // to wasm builds only.
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len())?);
        file.read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read `{shown}`: {error}"))?;
        Ok(bytes)
    })();
    if result.is_err() {
        span.event(scxml_telemetry::SignalEvent::InputRejected);
        span.fail(ErrorCategory::InputRead);
    } else {
        span.event(scxml_telemetry::SignalEvent::ExternalReadCompleted);
    }
    result
}

/// Decodes a source for `compile`, naming the path and the line and column
/// of the first invalid byte (not a byte index) when it is not UTF-8.
fn utf8_source(path: &Path, bytes: Vec<u8>) -> Result<String, Box<dyn std::error::Error>> {
    String::from_utf8(bytes).map_err(|error| {
        let offset = error.utf8_error().valid_up_to();
        let valid = error.as_bytes().get(..offset).unwrap_or_default();
        let byte = error.as_bytes().get(offset).copied().unwrap_or_default();
        // Everything before the offset is valid UTF-8.
        let before = std::str::from_utf8(valid).unwrap_or_default();
        let line = before.matches('\n').count() + 1;
        let column = before
            .rsplit('\n')
            .next()
            .map_or(0, |line| line.chars().count())
            + 1;
        format!(
            "`{}` is not valid UTF-8: byte 0x{byte:02X} at {line}:{column} (run `scx validate` for the located diagnostic)",
            path.display()
        )
        .into()
    })
}

fn conformance_prepare(
    options: &ConformancePrepareOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let span = observe(Module::Cli, Operation::CliConformancePrepare);
    let result = (|| {
        let transformer = if options.options.offline {
            SaxonTransformer::offline(options.saxon.clone())?
        } else {
            SaxonTransformer::new(options.saxon.clone())?
        };
        let summary = prepare_corpus(&options.options, &transformer)?;
        println!(
            "acquired {} and prepared {} of {} official cases; preparation gaps {}; reused artifacts {}; corpus {}",
            summary.acquired_cases,
            summary.prepared_cases,
            summary.selected_cases,
            summary.preparation_gaps,
            summary.reused_artifacts,
            summary.corpus.display()
        );
        if summary.preparation_gaps > 0 {
            return Err("official corpus preparation contains actionable gaps".into());
        }
        Ok(())
    })();
    if result.is_err() {
        span.fail(ErrorCategory::Platform);
    }
    result
}

/// The runner options every conformance pass shares.
fn runner_options(options: &ConformanceRunOptions) -> RunOptions {
    RunOptions {
        filters: options.filters.clone(),
        schema_mode: SchemaMode::Off,
        git_commit: options.git_commit.clone(),
    }
}

/// Prints one executed case in the runner's stable one-line form.
fn print_case_line(case: &scxml_conformance::CaseResult) {
    println!(
        "{:<12} {:<14} conformance={:?} spec={} datamodel={} stage={:?}{}{}{}",
        case.id,
        disposition_name(case.disposition),
        case.conformance,
        case.spec_id,
        case.datamodel,
        case.stage,
        case.oracle.map_or_else(String::new, |oracle| format!(
            " oracle={}",
            match oracle {
                scxml_conformance::OracleKind::OutcomeLog => "outcome-log",
                scxml_conformance::OracleKind::ManualEquivalent => "manual-equivalent",
                scxml_conformance::OracleKind::SyntheticExpectation => "synthetic-expectation",
                scxml_conformance::OracleKind::DocumentRejection => "document-rejection",
            }
        )),
        case.erratum
            .as_ref()
            .map_or_else(String::new, |erratum| format!(" erratum={erratum}")),
        case.reason_code
            .as_ref()
            .map_or_else(String::new, |code| format!(" reason={code}"))
    );
}

fn conformance_run(options: &ConformanceRunOptions) -> Result<(), Box<dyn std::error::Error>> {
    let span = observe(Module::Cli, Operation::CliConformance);
    let result = (|| {
        // Link the components before loading anything so a build without
        // Wasmtime, or a rejected release, fails closed without running a
        // case.
        let executor = components::executor(&options.components, &options.extensions)?;
        let suite = load_suite(&options.corpus)?;
        let report = if options.snapshot_every_checkpoint {
            // The proof runs its own collecting passes; its plain
            // (non-collecting) report is then judged exactly like a plain
            // run's below: case lines, outputs, baselines, and every gate.
            let collecting = || {
                components::executor_with_snapshots(
                    &options.components,
                    &options.extensions,
                    components::SnapshotCollection::EveryCheckpoint,
                )
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)
            };
            snapshot_proof::run_snapshot_proof(
                &suite,
                &runner_options(options),
                &collecting,
                executor.as_ref(),
            )?
        } else {
            run_suite(&suite, &runner_options(options), executor.as_ref())?
        };
        for case in &report.cases {
            print_case_line(case);
        }
        println!("{}", report.summary.concise());
        println!(
            "corpus source={} revision={} digest={} source-digest={} manifest={} templates={:?} transformer={:?} errata={:?} datamodel={} runner={} manual-oracle={} ir={}",
            report.suite.source,
            report.suite.revision.as_deref().unwrap_or("unspecified"),
            report.suite.corpus_digest,
            report.suite.source_digest,
            report.suite.manifest_digest,
            report.suite.template_digests,
            report.suite.transformer,
            report.suite.errata,
            report.suite.datamodel,
            report.runner.version,
            report.manual_oracle.version,
            report.ir_version
        );
        if let Some(path) = &options.json {
            report.write_json(path)?;
        }
        if let Some(path) = &options.report_copy {
            report.write_json(path)?;
        }
        let comparison = if let Some(path) = &options.baseline {
            let baseline = RunReport::read_json(path)?;
            let identity_policy = if options.allow_baseline_mismatch {
                scxml_conformance::IdentityPolicy::AllowMismatch
            } else {
                scxml_conformance::IdentityPolicy::Require
            };
            let comparison = compare_reports(&baseline, &report, identity_policy)?;
            println!("{}", comparison.concise());
            Some(comparison)
        } else {
            None
        };
        if let Some(path) = &options.canonical_baseline {
            check_canonical_baseline(path, &report)?;
        }
        if !options.allow_not_passing && !report.summary.mandatory_complete() {
            return Err("applicable selected mandatory IRP cases are not all Passed".into());
        }
        if options.fail_on_regression
            && comparison.is_some_and(|comparison| comparison.has_regressions())
        {
            return Err("conformance baseline contains Pass -> non-Pass regressions".into());
        }
        Ok(())
    })();
    if result.is_err() {
        span.fail(ErrorCategory::Harness);
    }
    result
}

/// Requires `report` to reproduce the checked-in canonical baseline at
/// `path` case by case, listing every mismatch on stderr in sorted order
/// (`baseline` names the checked-in value, `fresh` this run's). The baseline
/// must be a regular file within `scxml_reporting::MAX_CANONICAL_BYTES`,
/// checked before it is read; at most one byte past the bound is read.
fn check_canonical_baseline(
    path: &Path,
    report: &RunReport,
) -> Result<(), Box<dyn std::error::Error>> {
    let limit = u64::try_from(scxml_reporting::MAX_CANONICAL_BYTES)?;
    let Some((file, metadata)) =
        open_regular_file(path)?.filter(|(_, metadata)| metadata.len() <= limit)
    else {
        return Err(
            "canonical baseline must be a regular file within the 32 MiB canonical report bound"
                .into(),
        );
    };
    let mut baseline = Vec::with_capacity(usize::try_from(metadata.len())?);
    file.take(limit + 1).read_to_end(&mut baseline)?;
    if u64::try_from(baseline.len())? > limit {
        return Err("canonical baseline exceeds the 32 MiB canonical report bound".into());
    }
    match scxml_conformance::compare_to_baseline(report, &baseline)? {
        Ok(()) => {
            println!("canonical-baseline: every case reproduces the baseline");
            Ok(())
        }
        Err(mismatches) => {
            let mut stderr = io::stderr().lock();
            writeln!(
                stderr,
                "canonical-baseline: {} mismatches",
                mismatches.len()
            )?;
            for mismatch in &mismatches {
                writeln!(stderr, "  {mismatch}")?;
            }
            Err(Box::new(BaselineMismatchReported))
        }
    }
}

const fn disposition_name(disposition: Disposition) -> &'static str {
    match disposition {
        Disposition::Passed => "passed",
        Disposition::Failed => "failed",
        Disposition::Unsupported => "unsupported",
        Disposition::Invalid => "invalid",
        Disposition::HarnessError => "harness-error",
        Disposition::Skipped => "skipped",
        Disposition::NotApplicable => "not-applicable",
    }
}

/// The schema flags of `validate` and `compile`.
struct SchemaFlags {
    /// `--schema URI=PATH` mappings.
    schemas: Vec<(String, PathBuf)>,
    /// `--extension-package` manifests.
    packages: Vec<PathBuf>,
    /// `--service-package` manifests.
    services: Vec<PathBuf>,
    /// `--data-model-package` manifests.
    data_models: Vec<PathBuf>,
    /// Repository namespace flags.
    repository: repository::RepositoryFlags,
}

impl SchemaFlags {
    /// The `--extension-package` packages and the repository namespaces
    /// `source` uses (manifest and schema only: validation and compilation
    /// never need an extension component).
    fn packages(&self, source: &[u8]) -> Result<extensions::Packages, Box<dyn std::error::Error>> {
        extensions::Packages::resolve(source, &[], &self.packages, &self.schemas, &self.repository)
    }

    /// Verified package schemas for their namespaces, explicit `--schema`
    /// mappings, and the bundled official W3C SCXML 1.0 schema set as the
    /// default for the exact SCXML namespace.
    fn resolver(
        &self,
        packages: &[extensions::Package],
    ) -> extensions::PackageSchemaResolver<BundledSchemaResolver<FileSchemaResolver>> {
        let mut resolver = FileSchemaResolver::new();
        for (uri, path) in &self.schemas {
            resolver.insert(uri.clone(), path.clone());
        }
        extensions::PackageSchemaResolver::new(BundledSchemaResolver::new(resolver), packages)
    }
}

fn emit(artifact: &scxml_reporting::ArtifactEnvelope) -> Result<(), Box<dyn std::error::Error>> {
    let span = observe(Module::Cli, Operation::CliEmit);
    let result = (|| {
        let rendered = RustcRenderer {
            color: if io::stderr().is_terminal() {
                ColorChoice::Always
            } else {
                ColorChoice::Never
            },
        }
        .render(artifact)?;
        io::stderr().lock().write_all(rendered.as_bytes())?;
        Ok(())
    })();
    if result.is_err() {
        span.fail(ErrorCategory::Report);
    }
    result
}

fn print_usage() {
    println!(
        "scx\n\n  validate [--] PATH [--format human|json|sarif] [--schema URI=PATH]... [--extension-package MANIFEST.json]... [--service-package MANIFEST.json]... [--data-model-package MANIFEST.json]... [--offline] [--cache-dir DIR] [--github-api-base URL] [--github-raw-base URL] [--github-download-base URL] [--github-token-env NAME] [--policy strict|lax|off] [--canonical PATH]\n  --explain CODE\n  compile [--] PATH --output FILE [--artifact lowered-ir|component-wasm] [--machine-template PATH --machine-template-sha256 HEX] [--canonical PATH] [--schema URI=PATH]... [--extension-package MANIFEST.json]... [--service-package MANIFEST.json]... [--data-model-package MANIFEST.json]... [--offline] [--cache-dir DIR] [--github-api-base URL] [--github-raw-base URL] [--github-download-base URL] [--github-token-env NAME] [--policy strict|lax|off] [--script URI=PATH --script-sha256 URI=HEX]... [--script-resolution compile|runtime]\n  conformance prepare --manifest URL|PATH [--base-uri URL] --templates DIR --datamodel NAME --cache DIR [--output DIR] [--source-dir DIR] [--revision REV] --saxon CMD [--errata FILE] [--namespace NAME] [--offline] [--force] [--max-commands N]\n  conformance run --corpus DIR [--id ID]... [--id-range START-END]... [--spec ID]... [--spec-num NUM]... [--mandatory|--optional] [--datamodel NAME]... [--json PATH] [--report-copy PATH] [--baseline PATH] [--fail-on-regression] [--allow-baseline-mismatch] [--allow-not-passing] [--snapshot-every-checkpoint] [--components DIR --components-manifest-sha256 HEX] [--extension-package MANIFEST.json --extension-component-sha256 URI=HEX]... [--canonical-baseline BASELINE.scxml-report.json]\n  snapshot run [--] DOC.scx [--components DIR --components-manifest-sha256 HEX] [--event NAME]... [--at N]... | [--every] [--exclude SECTION]... [--trace-context strip|preserve] [--output FILE | --output-dir DIR] [--extension-package MANIFEST.json [--extension-component URI=PATH] --extension-component-sha256 URI=HEX]... [--service-package MANIFEST.json [--service-component URI=PATH] --service-component-sha256 URI=HEX]... [--data-model-package MANIFEST.json [--data-model-component URI=PATH] --data-model-component-sha256 URI=HEX]... [--dir URI=HOST::GUEST]... [--wasi URI=inherit-network|URI=allow-ip-name-lookup]... [--env NAME[=VALUE]]... [--env-file PATH]... [--offline] [--cache-dir DIR] [--github-api-base URL] [--github-raw-base URL] [--github-download-base URL] [--github-token-env NAME]\n  snapshot show SNAP.json [--format human|json] [--color always|never]\n  run [--] DOC.scx [--components DIR --components-manifest-sha256 HEX] [--extension-package MANIFEST.json [--extension-component URI=PATH] --extension-component-sha256 URI=HEX]... [--service-package MANIFEST.json [--service-component URI=PATH] --service-component-sha256 URI=HEX]... [--data-model-package MANIFEST.json [--data-model-component URI=PATH] --data-model-component-sha256 URI=HEX]... [--dir URI=HOST::GUEST]... [--wasi URI=inherit-network|URI=allow-ip-name-lookup]... [--env NAME[=VALUE]]... [--env-file PATH]... [--offline] [--cache-dir DIR] [--github-api-base URL] [--github-raw-base URL] [--github-download-base URL] [--github-token-env NAME] [--fuel N] [--max-commands N] [--session-id NAME] [--clock wall|logical] [--snapshot-on call|done|never] [--exclude SECTION]... [--trace-context strip|preserve] [--on-eof wait|close] [--record FILE | --replay FILE] [--http-allow ORIGIN-OR-PREFIX]... [--http-listen PORT|IP:PORT [--http-public-url URL]]\n  snapshot check SNAP.json [--format human|json]\n  snapshot diff A.json B.json [--format human|json] [--include-trace-context] [--color always|never]\n\nEvery option is a long option and may come before or after PATH; a PATH that starts with `-` follows `--`, and standard input is not read. Validate prints rustc-style diagnostics to stderr by default; --format json writes the canonical scxml.report bytes and --format sarif writes SARIF 2.1.0 to stdout, and the exit status is 1 exactly when an error is reported. --explain CODE prints the long-form explanation of a diagnostic code (for example --explain SCXML005). Compile emits lowered-ir (portable Machine IR data, the default; a runtime host or browser feeds it to the generic machine through ir-source) or component-wasm (the pinned prebuilt generic machine template composed with a data-only ir-source provider; for non-browser ahead-of-time hosts; no code generation or native toolchain). core-wasm is never produced. --extension-package MANIFEST.json binds one extension namespace (a stateforward.scxml.extension-package/v2 manifest whose schema-artifact, relative to the manifest, is verified against its schema-sha256 and then validates that namespace; an extension element does no I/O, so the manifest declares no outbound origins and no env slots); compile rejects every used extension namespace without one under every schema policy (COMP002), and --schema does not supply executable behavior. A namespace URI of exactly https://github.com/OWNER/REPO/namespaces/NAME[@TAG] is a repository namespace (the env, event, and scx vocabularies scx provides excepted: they are never fetched): validate, compile, snapshot run, and run resolve it with no flag (its manifest and schema from the commit the tag names, or, without @TAG, the commit of the repository's latest release, and, when snapshot run or run binds it, the release asset the manifest names, verified against the release's SHA256SUMS). A literal <invoke type> of exactly https://github.com/OWNER/REPO/services/NAME[@TAG] that no --service-package binds is a repository service, resolved the same way. The manifest names the exact URI the document binds. Both cache the verified bytes under --cache-dir DIR (default ${{XDG_CACHE_HOME:-$HOME/.cache}}/scxml/extensions), pin the URI to that commit and those digests, and reuse the cache with no network afterwards (a tagless URI asks for the latest release once per run and re-pins when it moved); each URI is resolved once per run and shared by every invoked session; --offline forbids any fetch, and every failure fails closed. --github-api-base, --github-raw-base, and --github-download-base point at a mirror; --github-token-env NAME sends that variable's GitHub token to the API only. Snapshot run and run execute each bound namespace's extension component (extension-artifact relative to the manifest, or --extension-component URI=PATH), authenticated by --extension-component-sha256 URI=HEX; each extension component call runs under the host's DEFAULT_EXTENSION_CALL_FUEL instruction cap (one billion: enough for an element that compiles a whole XSD set in one call). --service-package MANIFEST.json binds one invoked service (a stateforward.scxml.service-package/v1 manifest naming its exact service-uri, which an <invoke type> names, its service-artifact, an optional schema-artifact verified against schema-sha256, its WIT world and version, and its declared http-origins and env-slots); validate warns on a literal invoke type no package binds (PKG004), which compiles and raises error.execution when its <invoke> runs; snapshot run and run execute each bound service's component (service-artifact relative to the manifest, or --service-component URI=PATH), authenticated by --service-component-sha256 URI=HEX, and list each bound service's URI, version, declared origins, slots, the WASI interfaces its component imports, and the WASI resources the run grants it before running. A data model other than the built-in ecmascript, null, and xpath is a package: <scxml datamodel=\"NAME\" datamodel:src=\"URI\"> (https://github.com/stateforward/scxml.rs/namespaces/datamodel) names its package URI, and NAME must be the name the package declares (DATAMODEL002). --data-model-package MANIFEST.json binds one (a stateforward.scxml.data-model-package/v1 manifest naming its data-model-uri, its datamodel name, its data-model-artifact, and its WIT world and version); a datamodel:src of exactly https://github.com/OWNER/REPO/datamodels/NAME[@TAG] that no --data-model-package binds is a repository data model, resolved like a repository service; a data model that is neither built in nor bound is PKG005. Snapshot run and run execute each bound data model's component (data-model-artifact relative to the manifest, or --data-model-component URI=PATH, authenticated by --data-model-component-sha256 URI=HEX; a repository data model's from its release, verified against SHA256SUMS) as the data model of every session whose datamodel names it. A service's outbound-http fetch suspends only that service's task: run performs it off the session thread and delivers its response in a call with cause http-response (--record records the response and --replay reproduces it without the network); leaving the invoking state cancels the invocation (SCXML 6.4) and closes its request's connection. A service component's own wasi:* imports name the WASI interfaces it uses, and only runtime resources need a grant, with wasmtime's option names scoped to the service URI (up to the first =): --dir URI=HOST::GUEST preopens HOST at the absolute GUEST path for wasi:filesystem, --wasi URI=inherit-network lets wasi:sockets/tcp connect to any address, and --wasi URI=allow-ip-name-lookup allows wasi:sockets/ip-name-lookup; an import without its grant, a grant without its import, and a wasi:* import the host never links fail closed before the session runs. The document scopes every variable: an invoked service's <invoke> names a variable for one of its slots with env:SLOT=\"NAME\" (https://github.com/stateforward/scxml.rs/namespaces/env), and the variable goes only to the services whose <invoke>s name it (an env:SLOTexpr slot names its variable when its <invoke> runs, so its service may receive any variable the run holds). --env-file PATH (Docker's format) and --env NAME or --env NAME=VALUE (repeatable) supply values: a variable holds its flag's VALUE, else its last --env-file entry, else, only when --env NAME names it, the calling shell's value; a variable no slot can name is dropped. The service receives an opaque reference for its whole invocation, never the value (a slot whose variable holds no value is absent), and the host substitutes the value only into that invocation's requests to an origin its package declares or one a value supplies entirely; every other request is refused. Before running, snapshot run and run list each variable the document names and the <invoke>s it goes to (names only). Values are masked in the text the CLI writes. Every command that reads a document (validate in its human format) first lists each bound element's namespace and version. Conformance runs and documents without repository namespaces never download anything, and nothing is ever transformed outside conformance prepare. Offline prepare accepts only saxon, saxon-he, or java -jar LOCAL_PATH and requires enforced host default-deny process isolation with a cleared environment and exact filesystem closure (macOS sandbox-exec; unsupported hosts fail closed). The default run exits nonzero when any applicable selected mandatory IRP case is Failed, Unsupported, Invalid, or HarnessError. --allow-not-passing is development-only and does not alter the report counts. Run, snapshot run, and conformance run execute through the shipped Wasm components under Wasmtime: by default the authenticated release session components scx embeds (built by scripts/verify-wasm-capabilities.sh, authenticated against their manifest's SHA-256 when scx is built and again when it runs); --components DIR with --components-manifest-sha256 HEX overrides them with another release directory and its trusted manifest digest. A build without the wasmtime feature (--no-default-features) embeds nothing and fails closed before running a case. Prepare --namespace NAME prepares a repository namespace's own implementation report plan (namespaces/NAME/irp; a namespace-irp corpus, whose manifest tests may list <reject code=...> children decided by the document-rejection oracle) instead of the W3C IRP; run --extension-package binds the namespaces such a suite uses, each extension-artifact read from --components DIR and authenticated by --extension-component-sha256 URI=HEX. --json and --report-copy each write the canonical report JSON. --snapshot-every-checkpoint also runs the selection twice more collecting one session snapshot per reached checkpoint and fails unless all three reports are byte-identical, the two collections are byte-identical, and every document passes snapshot check; it writes a per-model summary to target/snapshot-proof/<datamodel>.json under the current directory, and the plain report is then judged like any run (--baseline, --fail-on-regression, --canonical-baseline, and the mandatory gate). Snapshot writes and reads the read-only canonical session snapshot document scxml.session-snapshot/v2 (docs/session-snapshot.md): run executes DOC through the shipped components under Wasmtime and writes the final checkpoint to stdout or --output FILE (or --at/--every checkpoints to --output-dir DIR as DIR/NNNN.snapshot.json); show renders a document per session; check reports SNAP-<code> issues (exit 1) and --explain SNAP-<code> explains them; diff compares two documents (exit 1 when they differ). A snapshot is never restored. Snapshot run and run load <invoke src>, <script src>, and <data src> from https: URLs, file: URLs, and relative URIs (resolved against the base URI of the document that names them): every literal one, of the document and of every <invoke src> document it loads, once before the first macrostep, listed before running (a <script src> that cannot be fetched rejects the document; any other failure raises error.execution when its element runs), and an <invoke srcexpr> URI when the invoke runs; remote fetches are HTTPS only, bounded, without credentials, and forbidden by --offline. Run keeps one live session of DOC open (docs/live-session.md): it reads newline-delimited JSON inputs on standard input ({{\"name\":EVENT,\"data\":JSON}} events, {{\"type\":\"snapshot\"}}, {{\"type\":\"close\"}}), processes each event to a stable configuration as it arrives, fires delayed sends at their wall-clock deadline (--clock logical fires them at once), and writes one JSON record per line on standard output (opened, log, call, snapshot, done, error, closed); it exits 0 when the root reaches a top-level final state, 1 when the session fails, 3 when it closes while running (end of input, a close input, or the end of a replay), and 128+N on signal N. --record FILE writes the inputs with their arrival times, and --replay FILE reproduces that session on the logical clock. BasicHTTP (SCXML C.2) is off by default: a POST outside the session's own locations raises error.communication. --http-allow admits an origin or URL prefix (repeatable; targets are WHATWG-canonicalized first, and userinfo, redirects, cookies, and environment proxies are never used); a request is sent off the session thread and a failure (no response or a non-2xx status) arrives later as error.communication. --http-listen serves every session's BasicHTTP location (PORT binds 127.0.0.1), advertised as --http-public-url (default http://ADDR) + /sessions/<sessionid>/basichttp; the session then stays open after the end of input. --record also records the network inputs (inbound messages, late failures, and outbound accept or refuse decisions), and --replay reproduces them without the network, so the http flags cannot be given with --replay. --canonical-baseline compares the run with a checked-in canonical report (tests/conformance/w3c-irp/baselines) on the suite, schema policy, IR version, filters, summary, and every per-case field, and exits 1 listing every mismatch."
    );
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "scx-extension-package-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("unique CLI test directory must be creatable");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("CLI test directory must be removable");
        }
    }

    #[test]
    fn compile_requires_explicit_output() {
        assert!(parse_args(&["compile".into(), "a.scxml".into()]).is_err());
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn compile_artifact_phases_are_explicit_and_template_bound() {
        let Command::Compile { target, .. } = parse_args(&args(&[
            "compile",
            "a.scxml",
            "--output",
            "a.scxml-ir.json",
        ]))
        .expect("default compile parses") else {
            panic!("expected compile");
        };
        assert_eq!(target.artifact, ArtifactKind::LoweredIr);
        assert!(target.template.is_none());

        let Command::Compile { target, .. } = parse_args(&args(&[
            "compile",
            "a.scxml",
            "--output",
            "a.component.wasm",
            "--artifact",
            "component-wasm",
            "--machine-template",
            "machine.wasm",
            "--machine-template-sha256",
            "00",
            "--canonical",
            "a.report.json",
        ]))
        .expect("component compile parses") else {
            panic!("expected compile");
        };
        assert_eq!(target.artifact, ArtifactKind::ComponentWasm);
        assert!(target.canonical.is_some());
        let template = target.template.expect("template is bound");
        assert_eq!(template.path, PathBuf::from("machine.wasm"));
        assert_eq!(template.sha256, "00");

        for rejected in [
            args(&[
                "compile",
                "a.scxml",
                "--output",
                "o",
                "--artifact",
                "component-wasm",
            ]),
            args(&[
                "compile",
                "a.scxml",
                "--output",
                "o",
                "--artifact",
                "component-wasm",
                "--machine-template",
                "machine.wasm",
            ]),
            args(&[
                "compile",
                "a.scxml",
                "--output",
                "o",
                "--artifact",
                "core-wasm",
            ]),
            args(&[
                "compile",
                "a.scxml",
                "--output",
                "o",
                "--artifact",
                "bytecode",
            ]),
            args(&[
                "compile",
                "a.scxml",
                "--output",
                "o",
                "--machine-template",
                "machine.wasm",
                "--machine-template-sha256",
                "00",
            ]),
            args(&["validate", "a.scxml", "--artifact", "component-wasm"]),
        ] {
            assert!(
                parse_args(&rejected).is_err(),
                "{rejected:?} must be rejected"
            );
        }
    }

    #[test]
    fn component_compile_rejects_a_mismatched_template_before_writing_output() {
        let directory = TestDirectory::new();
        let source = directory.0.join("machine.scx");
        let template = directory.0.join("machine.wasm");
        let output = directory.0.join("machine.component.wasm");
        fs::write(
            &source,
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="ready"><state id="ready"/></scxml>"#,
        )
        .expect("source fixture is writable");
        fs::write(&template, b"not a component").expect("template fixture is writable");
        let target = CompileTarget {
            output: output.clone(),
            artifact: ArtifactKind::ComponentWasm,
            template: Some(TemplateArgs {
                path: template,
                sha256: "0".repeat(64),
            }),
            canonical: None,
            scripts: scripts::ScriptFlags::default(),
        };
        let error = compile(
            &source,
            &target,
            &SchemaFlags {
                schemas: Vec::new(),
                packages: Vec::new(),
                services: Vec::new(),
                data_models: Vec::new(),
                repository: repository::RepositoryFlags::default(),
            },
            SchemaMode::Off,
        )
        .expect_err("a template whose digest mismatches its pin is rejected");
        assert_eq!(
            error.to_string(),
            scxml_packager::PackageError::TemplateDigestMismatch.to_string()
        );
        assert!(!output.exists(), "a rejected package writes no component");
    }

    #[test]
    fn compile_rejects_extension_namespace_without_package_under_off_policy() {
        let directory = TestDirectory::new();
        let source = directory.0.join("custom.scxml");
        let output = directory.0.join("custom.scxml-ir.json");
        fs::write(
            &source,
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:x="urn:test:custom" version="1.0" initial="ready"><state id="ready"><onentry><x:action/></onentry></state></scxml>"#,
        )
        .expect("extension namespace CLI fixture must be writable");

        let target = CompileTarget {
            output: output.clone(),
            artifact: ArtifactKind::LoweredIr,
            template: None,
            canonical: None,
            scripts: scripts::ScriptFlags::default(),
        };
        let error = compile(
            &source,
            &target,
            &SchemaFlags {
                schemas: Vec::new(),
                packages: Vec::new(),
                services: Vec::new(),
                data_models: Vec::new(),
                repository: repository::RepositoryFlags::default(),
            },
            SchemaMode::Off,
        )
        .expect_err("CLI without package flags must reject an extension namespace");

        assert_eq!(error.to_string(), "compilation stopped after diagnostics");
        assert!(!output.exists(), "rejected compilation must emit no IR");
        assert!(
            !PathBuf::from(format!("{}.recipe.json", output.display())).exists(),
            "rejected compilation must emit no package recipe"
        );
    }

    #[test]
    fn compile_rejects_an_expression_the_runtime_would_decline() {
        let directory = TestDirectory::new();
        let source = directory.0.join("nested.scxml");
        let output = directory.0.join("nested.scxml-ir.json");
        // Twenty open parentheses exceed the shared nesting guard that both
        // the syntax checker and runtime evaluation apply (LIMIT003).
        let expression = format!("{}1{}", "(".repeat(20), ")".repeat(20));
        assert!(
            scxml_data_model_ecmascript::check_nesting(&expression).is_err(),
            "the runtime nesting guard declines the fixture expression"
        );
        fs::write(
            &source,
            format!(
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="a"><final id="a"><onentry><log expr="{expression}"/></onentry></final></scxml>"#
            ),
        )
        .expect("nested-expression CLI fixture must be writable");
        let target = CompileTarget {
            output: output.clone(),
            artifact: ArtifactKind::LoweredIr,
            template: None,
            canonical: None,
            scripts: scripts::ScriptFlags::default(),
        };
        let error = compile(
            &source,
            &target,
            &SchemaFlags {
                schemas: Vec::new(),
                packages: Vec::new(),
                services: Vec::new(),
                data_models: Vec::new(),
                repository: repository::RepositoryFlags::default(),
            },
            SchemaMode::Off,
        )
        .expect_err("an expression the runtime would reject must stop compilation");
        assert_eq!(error.to_string(), "compilation stopped after diagnostics");
        assert!(!output.exists(), "rejected compilation must emit no IR");
    }

    #[test]
    fn schema_mapping_is_exact_uri_to_path() {
        let command = parse_args(&[
            "validate".into(),
            "a.scxml".into(),
            "--schema".into(),
            "urn:a=a.xsd".into(),
            "--policy".into(),
            "lax".into(),
        ])
        .expect("valid schema-mapping arguments must parse");
        let Command::Validate {
            schemas,
            mode,
            canonical,
            ..
        } = command
        else {
            panic!("expected validate");
        };
        assert_eq!(schemas[0].0, "urn:a");
        assert_eq!(mode, SchemaMode::Lax);
        assert!(canonical.is_none());
    }

    #[test]
    fn options_may_come_before_or_after_the_path() {
        for arguments in [
            args(&["validate", "--format", "json", "a.scxml"]),
            args(&["validate", "a.scxml", "--format", "json"]),
            args(&["validate", "--policy", "lax", "a.scxml", "--format", "json"]),
        ] {
            let command = parse_args(&arguments).expect("option order must not matter");
            let Command::Validate { path, format, .. } = command else {
                panic!("expected validate");
            };
            assert_eq!(path, PathBuf::from("a.scxml"), "{arguments:?}");
            assert_eq!(format, OutputFormat::Json, "{arguments:?}");
        }
        let error = parse_args(&args(&["validate", "a.scxml", "b.scxml"]))
            .expect_err("a second path is rejected");
        assert!(
            error.contains("`b.scxml`") && error.contains("one PATH"),
            "{error}"
        );
        let error = parse_args(&args(&["validate", "--format", "json"]))
            .expect_err("a missing path is rejected");
        assert!(error.contains("requires PATH"), "{error}");
    }

    #[test]
    fn source_read_failures_name_the_path_and_invalid_utf8_its_line_and_column() {
        let directory = TestDirectory::new();
        let missing = directory.0.join("missing.scxml");
        let error = read_source(&missing).expect_err("a missing file is rejected");
        assert!(
            error
                .to_string()
                .starts_with(&format!("cannot read `{}`:", missing.display())),
            "{error}"
        );
        let error = utf8_source(Path::new("m.scxml"), b"<a>\n  \xE9</a>".to_vec())
            .expect_err("invalid UTF-8 is rejected");
        assert_eq!(
            error.to_string(),
            "`m.scxml` is not valid UTF-8: byte 0xE9 at 2:3 (run `scx validate` for the located diagnostic)"
        );
    }

    #[test]
    fn single_dash_arguments_are_unknown_options_never_the_path() {
        for (command, flag, suggestion) in [
            ("validate", "-v", None),
            ("validate", "-o", Some("--output")),
            ("validate", "-f", Some("--format")),
            ("compile", "-p", Some("--policy")),
            ("validate", "--fromat", Some("--format")),
            ("compile", "-x", None),
        ] {
            for arguments in [
                args(&[command, flag, "a.scxml"]),
                args(&[command, "a.scxml", flag]),
            ] {
                let error = parse_args(&arguments).expect_err("an unknown option is rejected");
                assert!(
                    error.starts_with(&format!("unknown option `{flag}`")),
                    "{arguments:?}: {error}"
                );
                assert!(!error.contains("one PATH"), "{arguments:?}: {error}");
                match suggestion {
                    Some(option) => assert!(
                        error.contains(&format!("did you mean `{option}`?")),
                        "{arguments:?}: {error}"
                    ),
                    None => assert!(error.contains("scx --help"), "{error}"),
                }
            }
        }
        for (arguments, flag) in [
            (args(&["conformance", "run", "-v"]), "-v"),
            (args(&["conformance", "prepare", "--ofline"]), "--ofline"),
            // A disposition is a run result, never a case selector.
            (
                args(&["conformance", "run", "--disposition", "passed"]),
                "--disposition",
            ),
        ] {
            let error = parse_args(&arguments).expect_err("an unknown option is rejected");
            assert!(
                error.starts_with(&format!("unknown option `{flag}`")),
                "{error}"
            );
        }
        let error = parse_args(&args(&["conformance", "prepare", "--ofline"]))
            .expect_err("a misspelled flag is rejected");
        assert!(error.contains("did you mean `--offline`?"), "{error}");
    }

    #[test]
    fn end_of_options_separator_admits_a_dash_path_and_stdin_is_named() {
        let Command::Validate { path, format, .. } = parse_args(&args(&[
            "validate",
            "--format",
            "json",
            "--",
            "-machine.scx",
        ]))
        .expect("a PATH after `--` may start with `-`") else {
            panic!("expected validate");
        };
        assert_eq!(path, PathBuf::from("-machine.scx"));
        assert_eq!(format, OutputFormat::Json);
        let error = parse_args(&args(&["validate", "--", "a.scxml", "--format"]))
            .expect_err("after `--` every argument is an operand");
        assert!(error.contains("one PATH"), "{error}");
        let error = parse_args(&args(&["validate", "-"])).expect_err("stdin is not read");
        assert!(error.contains("standard input"), "{error}");
        assert!(matches!(
            parse_args(&args(&["validate", "a.scxml", "-h"])),
            Ok(Command::Help)
        ));
        let error = parse_args(&args(&["valdate", "-v"])).expect_err("unknown command");
        assert!(error.contains("unknown command `valdate`"), "{error}");
    }

    #[test]
    fn validate_parses_canonical_output() {
        let command = parse_args(&[
            "validate".into(),
            "a.scxml".into(),
            "--canonical".into(),
            "a.report.json".into(),
        ])
        .expect("valid canonical-output arguments must parse");
        let Command::Validate { canonical, .. } = command else {
            panic!("expected validate");
        };
        assert_eq!(canonical, Some(PathBuf::from("a.report.json")));
    }

    #[test]
    fn conformance_parses_prepare_and_run_shapes() {
        let prepare = parse_args(&[
            "conformance".into(),
            "prepare".into(),
            "--manifest".into(),
            "manifest.xml".into(),
            "--base-uri".into(),
            "https://www.w3.org/Voice/2013/scxml-irp".into(),
            "--templates".into(),
            "templates".into(),
            "--datamodel".into(),
            "ecmascript".into(),
            "--cache".into(),
            "cache".into(),
            "--saxon".into(),
            "java -jar saxon.jar".into(),
            "--offline".into(),
        ])
        .expect("valid conformance prepare arguments must parse");
        let Command::Conformance(options) = prepare else {
            panic!("expected prepare");
        };
        let ConformanceCommand::Prepare(options) = *options else {
            panic!("expected prepare");
        };
        assert!(options.options.offline);
        assert_eq!(
            options.options.base_uri.as_deref(),
            Some("https://www.w3.org/Voice/2013/scxml-irp")
        );
        assert_eq!(options.saxon[0], "java");

        let run = parse_args(&[
            "conformance".into(),
            "run".into(),
            "--corpus".into(),
            "prepared".into(),
            "--id-range".into(),
            "1-8".into(),
            "--allow-not-passing".into(),
            "--components".into(),
            "release".into(),
            "--components-manifest-sha256".into(),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
            "--canonical-baseline".into(),
            "baseline.scxml-report.json".into(),
        ])
        .expect("valid conformance run arguments must parse");
        let Command::Conformance(options) = run else {
            panic!("expected run");
        };
        let ConformanceCommand::Run(options) = *options else {
            panic!("expected run");
        };
        assert_eq!(options.filters.id_ranges[0], IdRange { start: 1, end: 8 });
        assert!(options.allow_not_passing);
        assert!(matches!(
            &options.components,
            components::ComponentArgs::Directory { dir, .. } if dir == &PathBuf::from("release")
        ));
        assert_eq!(
            options.canonical_baseline,
            Some(PathBuf::from("baseline.scxml-report.json"))
        );
        assert!(
            matches!(
                parse_args(&[
                    "conformance".into(),
                    "run".into(),
                    "--corpus".into(),
                    "prepared".into(),
                ]),
                Ok(Command::Conformance(command))
                    if matches!(&*command, ConformanceCommand::Run(options)
                        if options.components == components::ComponentArgs::Embedded)
            ),
            "conformance run uses the embedded release by default"
        );
    }

    #[test]
    fn offline_prepare_rejects_command_wrapper_before_execution() {
        let command = parse_args(&[
            "conformance".into(),
            "prepare".into(),
            "--manifest".into(),
            "manifest.xml".into(),
            "--templates".into(),
            "templates".into(),
            "--datamodel".into(),
            "ecmascript".into(),
            "--cache".into(),
            "cache".into(),
            "--saxon".into(),
            "sh -c network-attempt".into(),
            "--offline".into(),
        ])
        .expect("adversarial command wrapper fixture must parse before policy validation");
        let Command::Conformance(options) = command else {
            panic!("expected conformance command");
        };
        let ConformanceCommand::Prepare(options) = *options else {
            panic!("expected conformance prepare command");
        };
        let error = conformance_prepare(&options)
            .expect_err("offline CLI must reject a command wrapper before process execution");
        assert!(error.to_string().contains("transformer-offline-command"));
    }

    /// A consistent empty report; the baseline read is checked before the
    /// report is consulted.
    fn empty_report() -> RunReport {
        let identity = |name: &str| scxml_reporting::ConformanceImplementationIdentity {
            name: name.into(),
            version: "0.1.0".into(),
            source_digest: "0".repeat(64),
        };
        RunReport {
            report_version: scxml_reporting::CONFORMANCE_REPORT_VERSION,
            suite: scxml_conformance::SuiteIdentity {
                errata: None,
                id: "synthetic-baseline-read".into(),
                version: "1.0.0".into(),
                kind: scxml_conformance::CorpusKind::SyntheticHarness,
                source: "in-memory fixture".into(),
                revision: None,
                corpus_digest: "a".repeat(64),
                source_digest: "b".repeat(64),
                manifest_digest: "c".repeat(64),
                template_digests: std::collections::BTreeMap::new(),
                transformer: None,
                datamodel: "null".into(),
                provenance: Some("synthetic; never an official IRP result".into()),
            },
            schema_policy: scxml_reporting::ConformanceSchemaPolicy::Off,
            compiler: identity("scxml-compiler-core"),
            engine: identity("scxml-machine"),
            data_model: identity("scxml-data-model-null"),
            runner: identity("scxml-conformance/wasm"),
            manual_oracle: identity("scxml-conformance-manual-oracle"),
            extensions: Vec::new(),
            ir_version: scxml_compiler_core::MACHINE_IR_VERSION,
            filters: scxml_conformance::Filters::default(),
            git_commit: None,
            identity_digest: String::new(),
            summary: scxml_conformance::Summary::default(),
            cases: Vec::new(),
        }
    }

    #[test]
    fn an_oversized_or_non_regular_canonical_baseline_is_rejected_before_it_is_read() {
        let directory = TestDirectory::new();
        let oversized = directory.0.join("oversized.scxml-report.json");
        // Sparse: only reading it would cost memory.
        fs::File::create(&oversized)
            .and_then(|file| file.set_len(1 << 30))
            .expect("the sparse baseline fixture is writable");
        let error = check_canonical_baseline(&oversized, &empty_report())
            .expect_err("an oversized baseline is rejected");
        assert!(
            error
                .to_string()
                .contains("canonical baseline must be a regular file"),
            "{error}"
        );

        let limit =
            u64::try_from(scxml_reporting::MAX_CANONICAL_BYTES).expect("the bound fits u64");
        fs::File::create(&oversized)
            .and_then(|file| file.set_len(limit + 1))
            .expect("the sparse baseline fixture is writable");
        assert!(check_canonical_baseline(&oversized, &empty_report()).is_err());

        let error = check_canonical_baseline(&directory.0, &empty_report())
            .expect_err("a directory is not a baseline");
        assert!(
            error
                .to_string()
                .contains("canonical baseline must be a regular file"),
            "{error}"
        );

        let malformed = directory.0.join("malformed.scxml-report.json");
        fs::write(&malformed, b"{}").expect("the malformed baseline fixture is writable");
        assert!(
            check_canonical_baseline(&malformed, &empty_report()).is_err(),
            "a bounded baseline still has to decode"
        );
    }

    /// A FIFO baseline is refused before it is opened: opening a FIFO blocks
    /// until a writer appears.
    #[cfg(unix)]
    #[test]
    fn a_fifo_canonical_baseline_is_refused_without_blocking() {
        let directory = TestDirectory::new();
        let fifo = directory.0.join("fifo.scxml-report.json");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo must run");
        assert!(status.success(), "mkfifo must create the FIFO fixture");
        let (sender, receiver) = std::sync::mpsc::channel();
        let path = fifo.clone();
        let reader = std::thread::spawn(move || {
            let result =
                check_canonical_baseline(&path, &empty_report()).map_err(|e| e.to_string());
            sender.send(result).expect("the test waits for the result");
        });
        let result = receiver.recv_timeout(std::time::Duration::from_secs(30));
        if result.is_err() {
            // Release the blocked open with a writer so the thread ends.
            drop(fs::OpenOptions::new().write(true).open(&fifo));
        }
        reader.join().expect("the baseline reader must not panic");
        let error = result
            .expect("a FIFO baseline must not block")
            .expect_err("a FIFO is not a baseline");
        assert!(
            error.contains("canonical baseline must be a regular file"),
            "{error}"
        );
    }
}
