//! `scx snapshot run|show|check|diff`: read-only session snapshots
//! (`scxml.session-snapshot/v2`) for inspection by people and LLMs.
//!
//! `show`, `check`, and `diff` read documents through the natively linked
//! `scxml-snapshot` crate. `run` executes a document through the shipped
//! components under the Wasmtime session host and writes the requested
//! checkpoint snapshots. It needs the `wasmtime` feature. Documents can hold
//! data-model values and (under `preserve`) trace carriers, so this module
//! never logs their content, and its spans carry no ids, pointers, or codes.
//!
//! Exit codes: `run` and `show` exit 0 or 2. `check` exits 0 when the
//! document is valid, 1 when it has issues, and 2 on error. `diff` exits 0
//! when the documents are identical, 1 when they differ, and 2 on error.

use std::fmt;
use std::fs;
use std::io::{self, IsTerminal as _, Read as _, Write as _};
use std::path::{Path, PathBuf};

use scxml_reporting::{TerminalText, explain};
use scxml_snapshot::{
    Color, DiffOptions, MAX_DOCUMENT_BYTES, Options, Section, SnapshotError, SnapshotErrorCode,
    TraceContextPolicy,
};
use scxml_telemetry::{ErrorCategory, Module, Operation, observe};

use crate::components::ComponentArgs;

/// Marker error: `check` found issues and already reported them (exit 1).
#[derive(Debug)]
pub struct IssuesReported;

impl fmt::Display for IssuesReported {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the snapshot document has issues")
    }
}

impl std::error::Error for IssuesReported {}

/// Marker error: `diff` found differences and already rendered them (exit 1).
#[derive(Debug)]
pub struct DocumentsDiffer;

impl fmt::Display for DocumentsDiffer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the snapshot documents differ")
    }
}

impl std::error::Error for DocumentsDiffer {}

/// A document that failed to decode, with the `SNAP-<code>` diagnostic
/// shown in the same form as `check` (exit 2).
#[derive(Debug)]
pub struct DecodeFailure {
    rendered: String,
}

impl fmt::Display for DecodeFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.rendered.trim_end())
    }
}

impl std::error::Error for DecodeFailure {}

impl DecodeFailure {
    /// The already rendered diagnostic, for the terminal error sink.
    pub fn rendered(&self) -> &str {
        &self.rendered
    }
}

/// Output format of `show`, `check`, and `diff`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Format {
    /// Rustc-like text.
    #[default]
    Human,
    /// JSON bytes on stdout.
    Json,
}

/// Which checkpoints `run` snapshots.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Points {
    /// Only the final checkpoint (the default).
    #[default]
    Final,
    /// The listed checkpoint ordinals, ascending and unique (numbered as in
    /// docs/session-snapshot.md, "When a snapshot can be taken").
    At(Vec<u32>),
    /// Every checkpoint.
    Every,
}

/// Where `run` writes its documents.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Output {
    /// One document on stdout.
    #[default]
    Stdout,
    /// One document in FILE.
    File(PathBuf),
    /// `DIR/NNNN.snapshot.json`, one per checkpoint.
    Dir(PathBuf),
}

/// Parsed `snapshot run` arguments.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunArgs {
    /// The SCXML document.
    pub document: PathBuf,
    /// Release components and their manifest digest.
    pub components: ComponentArgs,
    /// Scripted external events, in order.
    pub events: Vec<String>,
    /// Requested checkpoints.
    pub points: Points,
    /// Exclusions and carrier policy.
    pub options: Options,
    /// Destination.
    pub output: Output,
    /// `URI=PATH` resources for invoked documents and external scripts.
    pub resources: Vec<(String, PathBuf)>,
    /// Extension packages, their extension components, and the environment
    /// the extension components see.
    pub(crate) namespaces: crate::extensions::RunFlags,
    /// Invoked-service packages and their components.
    pub(crate) services: crate::services::ServiceFlags,
}

/// One parsed `snapshot` subcommand.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SnapshotCommand {
    /// `snapshot run`.
    Run(Box<RunArgs>),
    /// `snapshot show`.
    Show {
        /// The document.
        path: PathBuf,
        /// Output format.
        format: Format,
        /// Explicit color choice (`None`: color when stdout is a terminal).
        color: Option<Color>,
    },
    /// `snapshot check`.
    Check {
        /// The document.
        path: PathBuf,
        /// Output format.
        format: Format,
    },
    /// `snapshot diff`.
    Diff {
        /// The earlier document.
        before: PathBuf,
        /// The later document.
        after: PathBuf,
        /// Output format.
        format: Format,
        /// Compare trace carriers too.
        include_trace_context: bool,
        /// Explicit color choice.
        color: Option<Color>,
    },
}

/// Options of `snapshot run`.
pub const RUN_OPTIONS: &[&str] = &[
    "--components",
    "--components-manifest-sha256",
    "--event",
    "--at",
    "--every",
    "--exclude",
    "--trace-context",
    "--output",
    "--output-dir",
    "--resource",
    "--extension-package",
    "--extension-component",
    "--extension-component-sha256",
    "--service-package",
    "--service-component",
    "--service-component-sha256",
    "--dir",
    "--wasi",
    "--env",
    "--env-file",
    "--offline",
    "--cache-dir",
    "--github-api-base",
    "--github-raw-base",
    "--github-download-base",
    "--github-token-env",
];
/// Options of `snapshot show`.
pub const SHOW_OPTIONS: &[&str] = &["--format", "--color"];
/// Options of `snapshot check`.
pub const CHECK_OPTIONS: &[&str] = &["--format"];
/// Options of `snapshot diff`.
pub const DIFF_OPTIONS: &[&str] = &["--format", "--include-trace-context", "--color"];

fn value<'a>(args: &'a [String], index: usize, flag: &str, what: &str) -> Result<&'a str, String> {
    args.get(index + 1)
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} requires {what}"))
}

fn parse_format(value: &str) -> Result<Format, String> {
    match value {
        "human" => Ok(Format::Human),
        "json" => Ok(Format::Json),
        _ => Err("--format requires human or json".into()),
    }
}

fn parse_color(value: &str) -> Result<Color, String> {
    match value {
        "always" => Ok(Color::Always),
        "never" => Ok(Color::Never),
        _ => Err("--color requires always or never".into()),
    }
}

/// Parses a section in its document spelling.
pub(crate) fn parse_section(value: &str) -> Result<Section, String> {
    match value {
        "system" => Ok(Section::System),
        "data_model" => Ok(Section::DataModel),
        "queues" => Ok(Section::Queues),
        "pending_sends" => Ok(Section::PendingSends),
        "transitions" => Ok(Section::Transitions),
        "previews" => Ok(Section::Previews),
        "invocations" => Ok(Section::Invocations),
        _ => Err(
            "--exclude requires system, data_model, queues, pending_sends, transitions, previews, or invocations"
                .into(),
        ),
    }
}

/// Splits operands from `--flag VALUE` / `--switch` options, rejecting
/// unknown options with a near-miss suggestion.
pub(crate) struct Parsed<'a> {
    pub(crate) operands: Vec<&'a str>,
    pub(crate) options: Vec<(&'a str, Option<&'a str>)>,
}

pub(crate) fn split<'a>(
    args: &'a [String],
    known: &[&str],
    switches: &[&str],
    what: &dyn Fn(&str) -> &'static str,
) -> Result<Parsed<'a>, String> {
    let mut parsed = Parsed {
        operands: Vec::new(),
        options: Vec::new(),
    };
    let mut index = 0;
    let mut ended = false;
    while let Some(argument) = args.get(index) {
        let argument = argument.as_str();
        if ended {
            parsed.operands.push(argument);
        } else if argument == "--" {
            ended = true;
        } else if switches.contains(&argument) {
            parsed.options.push((argument, None));
        } else if known.contains(&argument) {
            parsed.options.push((
                argument,
                Some(value(args, index, argument, what(argument))?),
            ));
            index += 1;
        } else if argument == "-" {
            return Err("standard input is not a document: pass the file's PATH".into());
        } else if argument.starts_with('-') {
            return Err(crate::unknown_option(argument, known));
        } else {
            parsed.operands.push(argument);
        }
        index += 1;
    }
    Ok(parsed)
}

/// Parses the arguments after `snapshot`.
///
/// # Errors
///
/// A usage message.
pub fn parse(args: &[String]) -> Result<SnapshotCommand, String> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("run") => parse_run(rest).map(|run| SnapshotCommand::Run(Box::new(run))),
        Some("show") => {
            let parsed = split(rest, SHOW_OPTIONS, &[], &|_| "a value")?;
            let path = one_operand(&parsed.operands, "snapshot show", "SNAP.json")?;
            let mut format = Format::Human;
            let mut color = None;
            for (flag, value) in parsed.options {
                let value = value.unwrap_or_default();
                match flag {
                    "--format" => format = parse_format(value)?,
                    _ => color = Some(parse_color(value)?),
                }
            }
            Ok(SnapshotCommand::Show {
                path,
                format,
                color,
            })
        }
        Some("check") => {
            let parsed = split(rest, CHECK_OPTIONS, &[], &|_| "a value")?;
            let path = one_operand(&parsed.operands, "snapshot check", "SNAP.json")?;
            let mut format = Format::Human;
            for (_, value) in parsed.options {
                format = parse_format(value.unwrap_or_default())?;
            }
            Ok(SnapshotCommand::Check { path, format })
        }
        Some("diff") => {
            let parsed = split(
                rest,
                DIFF_OPTIONS,
                &["--include-trace-context"],
                &|_| "a value",
            )?;
            let [before, after] = parsed.operands.as_slice() else {
                return Err("snapshot diff takes two documents: A.json B.json".into());
            };
            let mut format = Format::Human;
            let mut include_trace_context = false;
            let mut color = None;
            for (flag, value) in parsed.options {
                let value = value.unwrap_or_default();
                match flag {
                    "--format" => format = parse_format(value)?,
                    "--include-trace-context" => include_trace_context = true,
                    _ => color = Some(parse_color(value)?),
                }
            }
            Ok(SnapshotCommand::Diff {
                before: PathBuf::from(before),
                after: PathBuf::from(after),
                format,
                include_trace_context,
                color,
            })
        }
        Some(other) => Err(format!(
            "unknown snapshot command `{other}`\n  = help: use snapshot run, show, check, or diff"
        )),
        None => Err("snapshot requires run, show, check, or diff".into()),
    }
}

pub(crate) fn one_operand(operands: &[&str], command: &str, what: &str) -> Result<PathBuf, String> {
    match operands {
        [path] => Ok(PathBuf::from(path)),
        [] => Err(format!("{command} requires {what}")),
        [first, second, ..] => Err(format!(
            "unexpected argument `{second}`: `{command}` takes one {what} (already given `{first}`)"
        )),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one flat table maps every snapshot run flag to its field and its usage error"
)]
fn parse_run(args: &[String]) -> Result<RunArgs, String> {
    let parsed = split(
        args,
        RUN_OPTIONS,
        &["--every", crate::repository::RepositoryFlags::OFFLINE],
        &|flag| match flag {
            "--components" | "--output-dir" => "DIR",
            "--components-manifest-sha256" => "HEX",
            "--extension-package" | "--service-package" => "MANIFEST.json",
            "--extension-component-sha256" | "--service-component-sha256" => "URI=HEX",
            "--dir" => "URI=HOST::GUEST",
            "--wasi" => "URI=inherit-network or URI=allow-ip-name-lookup",
            "--env" => "NAME or NAME=VALUE",
            "--env-file" => "PATH",
            flag if crate::repository::RepositoryFlags::expects(flag).is_some() => {
                crate::repository::RepositoryFlags::expects(flag).unwrap_or("a value")
            }
            "--event" => "NAME",
            "--at" => "N",
            "--exclude" => "SECTION",
            "--trace-context" => "strip or preserve",
            "--output" => "FILE",
            _ => "URI=PATH",
        },
    )?;
    let document = one_operand(&parsed.operands, "snapshot run", "DOC.scx")?;
    let mut components_dir = None;
    let mut manifest = None;
    let mut events = Vec::new();
    let mut at = Vec::new();
    let mut every = false;
    let mut options = Options::default();
    let mut out = None;
    let mut out_dir = None;
    let mut resources = Vec::new();
    let mut namespaces = crate::extensions::RunFlags::default();
    let mut services = crate::services::ServiceFlags::default();
    for (flag, value) in parsed.options {
        let value = value.unwrap_or_default();
        match flag {
            "--components" => components_dir = Some(PathBuf::from(value)),
            "--components-manifest-sha256" => manifest = Some(value.to_owned()),
            service if crate::services::ServiceFlags::FLAGS.contains(&service) => {
                services.accept(service, value)?;
            }
            "--event" => {
                if value.is_empty() || value.chars().any(char::is_whitespace) {
                    return Err("--event requires an SCXML event NAME".into());
                }
                events.push(value.to_owned());
            }
            "--at" => at.push(
                value
                    .parse::<u32>()
                    .map_err(|_| "--at requires a checkpoint ordinal N (0 is after start)")?,
            ),
            "--every" => every = true,
            "--exclude" => options.exclude.push(parse_section(value)?),
            "--trace-context" => {
                options.trace_context = match value {
                    "strip" => TraceContextPolicy::Strip,
                    "preserve" => TraceContextPolicy::Preserve,
                    _ => return Err("--trace-context requires strip or preserve".into()),
                };
            }
            "--output" => out = Some(PathBuf::from(value)),
            "--output-dir" => out_dir = Some(PathBuf::from(value)),
            "--resource" => resources.push(parse_resource(value)?),
            namespace => namespaces.accept(namespace, value)?,
        }
    }
    options.exclude.sort_unstable();
    options.exclude.dedup();
    at.sort_unstable();
    at.dedup();
    let points = match (every, at.is_empty()) {
        (true, false) => return Err("--at and --every are mutually exclusive".into()),
        (true, true) => Points::Every,
        (false, false) => Points::At(at),
        (false, true) => Points::Final,
    };
    let several =
        matches!(&points, Points::Every) || matches!(&points, Points::At(at) if at.len() > 1);
    let output = match (out, out_dir) {
        (Some(_), Some(_)) => {
            return Err("--output and --output-dir are mutually exclusive".into());
        }
        (Some(_) | None, None) if several => {
            return Err("several checkpoints need --output-dir DIR".into());
        }
        (Some(file), None) => Output::File(file),
        (None, Some(dir)) => Output::Dir(dir),
        (None, None) => Output::Stdout,
    };
    let components = ComponentArgs::resolve(components_dir, manifest)?;
    Ok(RunArgs {
        document,
        components,
        events,
        points,
        options,
        output,
        resources,
        namespaces,
        services,
    })
}

pub(crate) fn parse_resource(value: &str) -> Result<(String, PathBuf), String> {
    match value.split_once('=') {
        Some((uri, path)) if !uri.is_empty() && !path.is_empty() => {
            Ok((uri.to_owned(), PathBuf::from(path)))
        }
        _ => Err("--resource requires non-empty URI=PATH".into()),
    }
}

/// Runs one parsed subcommand.
///
/// # Errors
///
/// [`IssuesReported`] or [`DocumentsDiffer`] for exit 1; any other error
/// (including [`DecodeFailure`]) for exit 2.
pub fn execute(command: SnapshotCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        SnapshotCommand::Run(args) => spanned(Operation::CliSnapshotRun, || run(&args)),
        SnapshotCommand::Show {
            path,
            format,
            color,
        } => spanned(Operation::CliSnapshotShow, || show(&path, format, color)),
        SnapshotCommand::Check { path, format } => {
            spanned(Operation::CliSnapshotCheck, || check(&path, format))
        }
        SnapshotCommand::Diff {
            before,
            after,
            format,
            include_trace_context,
            color,
        } => spanned(Operation::CliSnapshotDiff, || {
            diff(
                &before,
                &after,
                format,
                DiffOptions {
                    include_trace_context,
                },
                color,
            )
        }),
    }
}

/// One boundary span per subcommand; the category is the only attribute.
fn spanned(
    operation: Operation,
    body: impl FnOnce() -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let span = observe(Module::Cli, operation);
    let result = body();
    if let Err(error) = &result {
        span.fail(
            if error.is::<IssuesReported>() || error.is::<DecodeFailure>() {
                ErrorCategory::Validation
            } else if error.is::<DocumentsDiffer>() {
                ErrorCategory::Oracle
            } else {
                ErrorCategory::Harness
            },
        );
    }
    result
}

fn stdout_color(explicit: Option<Color>) -> Color {
    explicit.unwrap_or_else(|| {
        if io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() {
            Color::Always
        } else {
            Color::Never
        }
    })
}

/// Reads a document, bounded one byte past the maximum so an oversized file
/// is reported as `too-large` rather than read whole.
fn read_document(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let shown = path.display();
    let file = fs::File::open(path).map_err(|error| format!("cannot read `{shown}`: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot read `{shown}`: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("cannot read `{shown}`: not a regular file").into());
    }
    let limit = u64::try_from(MAX_DOCUMENT_BYTES).unwrap_or(u64::MAX) + 1;
    let mut bytes = Vec::new();
    file.take(limit)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read `{shown}`: {error}"))?;
    Ok(bytes)
}

/// The 1-based line and column where `bytes` stop being JSON, for a
/// `malformed-json` issue (`None` for every other code).
fn json_position(issue: &SnapshotError, bytes: &[u8]) -> Option<(usize, usize)> {
    if issue.code != SnapshotErrorCode::MalformedJson {
        return None;
    }
    serde_json::from_slice::<serde_json::Value>(bytes)
        .err()
        .map(|error| (error.line(), error.column()))
}

/// Renders one `SNAP-<code>` issue rustc-style. `bytes` are the document's
/// bytes, which locate a `malformed-json` issue as `path:line:column`.
#[must_use]
pub fn render_issue(path: &str, issue: &SnapshotError, bytes: &[u8]) -> String {
    let code = format!("SNAP-{}", issue.code.as_str());
    let entry = explain::lookup(&code);
    let title = entry.map_or("session snapshot error", |entry| entry.title);
    let position = json_position(issue, bytes)
        .map_or_else(String::new, |(line, column)| format!(":{line}:{column}"));
    let pointer = match issue.pointer.as_deref() {
        Some("") => "   = pointer: \"\" (the document root)\n".to_owned(),
        Some(pointer) => format!("   = pointer: {}\n", TerminalText(pointer)),
        None => String::new(),
    };
    let help = entry.map_or_else(String::new, |entry| format!("   = help: {}\n", entry.help));
    format!(
        "error[{code}]: {title}\n  --> {}{position}\n{pointer}{help}",
        TerminalText(path)
    )
}

fn decode_or_report(path: &Path, bytes: &[u8]) -> Result<scxml_snapshot::Document, DecodeFailure> {
    scxml_snapshot::decode(bytes).map_err(|error| {
        let shown = path.display().to_string();
        DecodeFailure {
            rendered: format!(
                "{}For more information about this error, try `scx --explain SNAP-{}`.\n",
                render_issue(&shown, &error, bytes),
                error.code.as_str()
            ),
        }
    })
}

fn show(
    path: &Path,
    format: Format,
    color: Option<Color>,
) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = read_document(path)?;
    let document = decode_or_report(path, &bytes)?;
    let mut stdout = io::stdout().lock();
    match format {
        Format::Json => stdout.write_all(&bytes)?,
        Format::Human => {
            stdout.write_all(
                scxml_snapshot::render_show(&document, stdout_color(color)).as_bytes(),
            )?;
        }
    }
    Ok(())
}

fn check(path: &Path, format: Format) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = read_document(path)?;
    let issues = match scxml_snapshot::check(&bytes) {
        Ok(issues) => issues,
        Err(error) => vec![error],
    };
    let shown = path.display().to_string();
    match format {
        Format::Json => {
            let json = serde_json::json!({
                "valid": issues.is_empty(),
                "issues": issues
                    .iter()
                    .map(|issue| {
                        let mut entry = serde_json::Map::new();
                        entry.insert("code".into(), format!("SNAP-{}", issue.code.as_str()).into());
                        if let Some(pointer) = &issue.pointer {
                            entry.insert("pointer".into(), pointer.clone().into());
                        }
                        if let Some((line, column)) = json_position(issue, &bytes) {
                            entry.insert("line".into(), line.into());
                            entry.insert("column".into(), column.into());
                        }
                        serde_json::Value::Object(entry)
                    })
                    .collect::<Vec<_>>(),
            });
            io::stdout().lock().write_all(&serde_json::to_vec(&json)?)?;
        }
        Format::Human if issues.is_empty() => {
            writeln!(
                io::stdout().lock(),
                "ok: `{}` is a valid {} document",
                TerminalText(&shown),
                scxml_snapshot::SESSION_SNAPSHOT_VERSION
            )?;
        }
        Format::Human => {
            let mut stderr = io::stderr().lock();
            for issue in &issues {
                stderr.write_all(render_issue(&shown, issue, &bytes).as_bytes())?;
                stderr.write_all(b"\n")?;
            }
            let count = issues.len();
            let noun = if count == 1 { "issue" } else { "issues" };
            writeln!(
                stderr,
                "error: `{}` has {count} {noun}",
                TerminalText(&shown)
            )?;
            if let Some(first) = issues.first() {
                writeln!(
                    stderr,
                    "For more information about an error, try `scx --explain SNAP-{}`.",
                    first.code.as_str()
                )?;
            }
        }
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(Box::new(IssuesReported))
    }
}

fn diff(
    before_path: &Path,
    after_path: &Path,
    format: Format,
    options: DiffOptions,
    color: Option<Color>,
) -> Result<(), Box<dyn std::error::Error>> {
    let before_bytes = read_document(before_path)?;
    let after_bytes = read_document(after_path)?;
    let before = decode_or_report(before_path, &before_bytes)?;
    let after = decode_or_report(after_path, &after_bytes)?;
    let document = scxml_snapshot::diff_documents(&before, &after, options);
    let mut stdout = io::stdout().lock();
    match format {
        Format::Json => stdout.write_all(&serde_json::to_vec(&document)?)?,
        Format::Human => stdout.write_all(
            scxml_snapshot::render_diff(
                &document,
                &before_path.display().to_string(),
                &after_path.display().to_string(),
                stdout_color(color),
            )
            .as_bytes(),
        )?,
    }
    if document.identical {
        Ok(())
    } else {
        Err(Box::new(DocumentsDiffer))
    }
}

/// One checkpoint document produced by `run`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckpointDocument {
    /// Checkpoint ordinal (docs/session-snapshot.md, "When a snapshot can be
    /// taken").
    pub checkpoint: u32,
    /// Canonical document bytes.
    pub bytes: Vec<u8>,
}

fn run(args: &RunArgs) -> Result<(), Box<dyn std::error::Error>> {
    host::available()?;
    // Extension packages and flags are checked before anything executes.
    let source = crate::read_source(&args.document)?;
    let packages = crate::extensions::Packages::resolve(
        &source,
        &args.namespaces.packages,
        &[],
        &args.namespaces.repository,
    )?;
    args.namespaces.check(&packages)?;
    let services = crate::services::resolve(
        &source,
        &args.services.packages,
        &args.namespaces.repository,
    )?;
    args.services.check(&services.packages)?;
    let values = args.namespaces.values(&packages.slot_uses)?;
    crate::mask::install(
        &crate::extensions::HeldValue::pairs(&values),
        &crate::extensions::declared_origins(&packages, &services.packages),
    );
    crate::extensions::write_notes(&packages.summary(Some(&values)));
    crate::extensions::write_notes(&crate::extensions::env_summary(
        &packages.slot_uses,
        &values,
    ));
    crate::extensions::write_notes(&crate::services::summary(
        &services.packages,
        Some(&args.services),
        None,
    ));
    let documents = host::run(args, &packages, &services)?;
    write_documents(&args.output, &documents)
}

/// Writes `run` output: one document to stdout or FILE, or every document
/// to `DIR/NNNN.snapshot.json` (the directory is created when missing).
///
/// # Errors
///
/// An I/O failure, or no document for a single-document destination.
pub fn write_documents(
    output: &Output,
    documents: &[CheckpointDocument],
) -> Result<(), Box<dyn std::error::Error>> {
    match output {
        Output::Stdout | Output::File(_) => {
            let [document] = documents else {
                return Err(format!(
                    "expected one snapshot document, the run produced {}",
                    documents.len()
                )
                .into());
            };
            match output {
                Output::File(path) => fs::write(path, &document.bytes)
                    .map_err(|error| format!("cannot write `{}`: {error}", path.display()))?,
                _ => io::stdout().lock().write_all(&document.bytes)?,
            }
        }
        Output::Dir(dir) => {
            fs::create_dir_all(dir)
                .map_err(|error| format!("cannot create `{}`: {error}", dir.display()))?;
            for document in documents {
                let path = dir.join(format!("{:04}.snapshot.json", document.checkpoint));
                fs::write(&path, &document.bytes)
                    .map_err(|error| format!("cannot write `{}`: {error}", path.display()))?;
            }
        }
    }
    Ok(())
}

/// Whether every [`SnapshotErrorCode`] has an `--explain` entry.
#[cfg(test)]
fn every_code_is_explained() -> bool {
    use scxml_snapshot::SnapshotErrorCode;
    [
        SnapshotErrorCode::TooLarge,
        SnapshotErrorCode::MalformedJson,
        SnapshotErrorCode::UnsupportedVersion,
        SnapshotErrorCode::SchemaViolation,
        SnapshotErrorCode::NonCanonical,
        SnapshotErrorCode::IntegrityMismatch,
        SnapshotErrorCode::BoundsExceeded,
        SnapshotErrorCode::InvalidName,
        SnapshotErrorCode::Inconsistent,
        SnapshotErrorCode::DepthExceeded,
        SnapshotErrorCode::ClockInvariant,
        SnapshotErrorCode::NotStable,
        SnapshotErrorCode::StatusInvalid,
        SnapshotErrorCode::CodecUnavailable,
    ]
    .iter()
    .all(|code| explain::lookup(&format!("SNAP-{}", code.as_str())).is_some())
}

/// The execution side of `snapshot run` (and the document, resource, and
/// namespace plumbing `run` shares).
pub(crate) mod host {
    #[cfg(feature = "wasmtime")]
    use super::Points;
    use super::{CheckpointDocument, RunArgs};

    /// Documents for `<invoke src>`, `<data src>`, and runtime scripts:
    /// exact `--resource URI=PATH` matches, read as bounded regular files;
    /// otherwise an `<invoke src>` at or below the running document's
    /// directory, under exactly the rules of an `event:schema` file reference
    /// (`crate::event_schemas`: relative, no scheme, query, fragment, `.` or
    /// `..` segment, percent-decoded, a regular file that does not resolve
    /// outside the directory through a symbolic link).
    #[cfg(feature = "wasmtime")]
    pub(crate) struct ResourceDocuments {
        pub(crate) resources: Vec<(String, std::path::PathBuf)>,
        /// The running document's directory, canonicalized; `None` when it
        /// cannot be, so only `--resource` documents load.
        pub(crate) directory: Option<std::path::PathBuf>,
    }

    #[cfg(feature = "wasmtime")]
    impl ResourceDocuments {
        /// `resources`, and the directory of `document` for sibling
        /// `<invoke src>` documents.
        pub(crate) fn new(
            resources: Vec<(String, std::path::PathBuf)>,
            document: &std::path::Path,
        ) -> Self {
            Self {
                resources,
                directory: crate::event_schemas::document_directory(document),
            }
        }
    }

    #[cfg(feature = "wasmtime")]
    impl scxml_host_wasmtime::SessionEmbedder for ResourceDocuments {
        fn document(
            &self,
            kind: scxml_host_wasmtime::DocumentKind,
            uri: &str,
        ) -> Option<scxml_host_wasmtime::EmbeddedDocument> {
            if let Some(path) = self
                .resources
                .iter()
                .find(|(name, _)| name == uri)
                .map(|(_, path)| path)
            {
                let bytes = crate::read_source(path).ok()?;
                let text = String::from_utf8(bytes).ok()?;
                return Some(scxml_host_wasmtime::EmbeddedDocument {
                    text,
                    media_type: media_type_of(path).map(str::to_owned),
                });
            }
            if kind != scxml_host_wasmtime::DocumentKind::Invocation {
                return None;
            }
            let directory = self.directory.as_deref()?;
            let bytes = crate::event_schemas::read_reference(directory, uri)?;
            let text = String::from_utf8(bytes).ok()?;
            Some(scxml_host_wasmtime::EmbeddedDocument {
                text,
                media_type: None,
            })
        }

        fn scripts(&self) -> scxml_host_wasmtime::MachineScripts {
            scxml_host_wasmtime::MachineScripts::none()
        }
    }

    /// The media type a resource file's extension registers (SCXML B.2.1
    /// reads a JSON or XML indication for `<data src>`): `.json` is
    /// `application/json` (RFC 8259 §11, "File extension(s): .json") and
    /// `.xml` is `application/xml` (RFC 7303 §9.1, "File extension(s):
    /// .xml"). Other extensions indicate nothing; `.txt` in particular stays
    /// unlabeled, because the official W3C cases ship JSON and XML in `.txt`
    /// files and expect them read by content.
    #[cfg(feature = "wasmtime")]
    fn media_type_of(path: &std::path::Path) -> Option<&'static str> {
        let extension = path.extension()?.to_str()?;
        if extension.eq_ignore_ascii_case("json") {
            Some("application/json")
        } else if extension.eq_ignore_ascii_case("xml") {
            Some("application/xml")
        } else {
            None
        }
    }

    /// Compiles the document canonically (the same validator and
    /// expression stack as `scx compile`) and returns the lowered
    /// Machine IR bytes and the linked data model's name.
    #[cfg(feature = "wasmtime")]
    pub(crate) fn compile(
        document: &std::path::Path,
        packages: &[crate::extensions::Package],
        services: &[crate::services::ServicePackage],
    ) -> Result<(Vec<u8>, Option<String>), Box<dyn std::error::Error>> {
        use scxml_compiler_core::{CompileInput, Compiler, MachineIr, ScriptResolution};
        use scxml_validator_core::{FileSchemaResolver, Validator};
        use scxml_validator_xsd::{BundledSchemaResolver, XsdSchemaBackend};

        let bytes = crate::read_source(document)?;
        let event_schemas = crate::event_schemas::load(document, &bytes);
        let source = crate::utf8_source(document, bytes)?;
        let (manifests, package_schemas) = crate::extensions::compile_inputs(packages);
        let compiler = Compiler::new(
            Validator::new(
                crate::extensions::PackageSchemaResolver::new(
                    BundledSchemaResolver::new(FileSchemaResolver::new()),
                    packages,
                ),
                XsdSchemaBackend,
            )
            .with_schema_mode(scxml_validator_core::SchemaMode::default())
            .with_extension_slots(
                packages
                    .iter()
                    .map(|package| {
                        (
                            package.manifest.namespace_uri.clone(),
                            package.manifest.env_slots.clone(),
                        )
                    })
                    .collect(),
            )
            .with_services(crate::services::bound_services(services))
            .with_expression_checker(std::sync::Arc::new(crate::expression::EcmaScriptSyntax))
            .with_expression_checker(std::sync::Arc::new(crate::expression::XPathSyntax)),
        );
        let input = CompileInput {
            source_name: document.display().to_string(),
            source,
            packages: manifests,
            package_schemas,
            scripts: Vec::new(),
            script_resolution: ScriptResolution::Compile,
            event_schemas,
        };
        let result = std::thread::scope(|scope| {
            std::thread::Builder::new()
                .stack_size(crate::expression::VALIDATION_STACK_BYTES)
                .spawn_scoped(scope, || compiler.compile(input))
                .map_err(|error| format!("could not start the compilation thread: {error}"))?
                .join()
                .map_err(|_| "the compilation thread failed".to_owned())
        })?;
        // Diagnostics render exactly as `compile` renders them.
        crate::emit(&result.canonical_artifact()?)?;
        let artifact = result
            .artifact
            .ok_or("compilation stopped after diagnostics")?;
        let ir = MachineIr::from_json(&artifact.bytes)
            .map_err(|error| format!("the compiler produced unusable Machine IR: {error}"))?;
        Ok((artifact.bytes, ir.data_model))
    }

    /// Runs the document through the shipped components under the
    /// Wasmtime session host (the scripted events, then drain) and
    /// returns the requested checkpoint documents.
    #[cfg(feature = "wasmtime")]
    pub fn run(
        args: &RunArgs,
        packages: &crate::extensions::Packages,
        services: &crate::services::Services,
    ) -> Result<Vec<CheckpointDocument>, Box<dyn std::error::Error>> {
        use std::rc::Rc;

        use scxml_host_wasmtime::{ScriptedEvent, SessionRun, SnapshotPoints, SnapshotRequest};

        let (ir_bytes, data_model) = compile(&args.document, &packages.all(), &services.packages)?;
        let host = args
            .components
            .session_host(scxml_host_wasmtime::DEFAULT_FUEL)?;
        let values = args.namespaces.values(&packages.slot_uses)?;
        let (services, _imports) = args.services.components(services, &values, true)?;
        let (extensions, _notes) = extensions(&args.namespaces, packages, &values, true)?;
        let host = host
            .with_extension_components(extensions)?
            .with_service_components(services)?;
        // `Final` snapshots every checkpoint and keeps the last document;
        // the run itself is identical either way.
        let points = match &args.points {
            Points::Final | Points::Every => SnapshotPoints::Every,
            Points::At(at) => SnapshotPoints::At(at.clone()),
        };
        let events: Vec<ScriptedEvent> = args.events.iter().map(ScriptedEvent::named).collect();
        let outcome = host.run(&SessionRun {
            machine_ir: &ir_bytes,
            data_model: data_model.as_deref(),
            session_id: "session",
            events: &events,
            external_client: None,
            embedder: Rc::new(ResourceDocuments::new(
                args.resources.clone(),
                &args.document,
            )),
            // No host-imposed command budget: resource limits apply to the
            // Wasm components only, whose own per-call command bound stops
            // every machine call.
            max_commands: u64::MAX,
            snapshots: SnapshotRequest {
                points,
                options: args.options.clone(),
            },
        });
        let outcome = outcome.map_err(|error| repository_failure(packages, error.into()))?;
        // A failed snapshot fails this command, never the run: `Final`
        // needs only the last checkpoint, every other mode every one it
        // names.
        let last = outcome.snapshots.last().map(|snapshot| snapshot.checkpoint);
        if let Some(failed) = outcome.snapshot_failures.iter().find(|failed| {
            !matches!(args.points, Points::Final)
                || last.is_none_or(|last| failed.checkpoint > last)
        }) {
            return Err(format!(
                "the snapshot of checkpoint {} failed: {}",
                failed.checkpoint, failed.failure
            )
            .into());
        }
        let mut documents: Vec<_> = outcome
            .snapshots
            .into_iter()
            .map(|snapshot| CheckpointDocument {
                checkpoint: snapshot.checkpoint,
                bytes: snapshot.bytes,
            })
            .collect();
        if matches!(args.points, Points::Final)
            && let Some(last) = documents.pop()
        {
            documents = vec![last];
        }
        Ok(documents)
    }

    /// The CLI's [`scxml_host_wasmtime::ExtensionObserver`]: it collects the
    /// origins each namespace's requests were refused, and dropping it (when
    /// the run's command ends, whatever its outcome) writes one note per
    /// refused origin. A missing value was already noted before the run (the
    /// element summary), so it is not repeated.
    #[cfg(feature = "wasmtime")]
    #[derive(Debug, Default)]
    pub(crate) struct Observer {
        refused: std::sync::Mutex<std::collections::BTreeSet<(String, String)>>,
    }

    #[cfg(feature = "wasmtime")]
    impl scxml_host_wasmtime::ExtensionObserver for Observer {
        fn request_refused(&self, namespace_uri: &str, origin: &str) {
            if let Ok(mut refused) = self.refused.lock() {
                refused.insert((namespace_uri.to_owned(), origin.to_owned()));
            }
        }

        fn value_missing(&self, _namespace_uri: &str, _slot: &str, _name: &str) {}
    }

    /// Writes the notes of an [`Observer`] (shared with the host) when the
    /// run's command ends.
    #[cfg(feature = "wasmtime")]
    #[derive(Debug)]
    pub(crate) struct Notes(pub(crate) std::sync::Arc<Observer>);

    #[cfg(feature = "wasmtime")]
    impl Drop for Notes {
        fn drop(&mut self) {
            let notes: Vec<String> = self
                .0
                .refused
                .lock()
                .map(|refused| {
                    refused
                        .iter()
                        .map(|(namespace_uri, origin)| {
                            crate::extensions::refused_note(namespace_uri, origin)
                        })
                        .collect()
                })
                .unwrap_or_default();
            crate::extensions::write_notes(&notes);
        }
    }

    /// A run failure, preceded by the repository namespace failure that caused
    /// it when the session start could not load one.
    #[cfg(feature = "wasmtime")]
    pub(crate) fn repository_failure(
        packages: &crate::extensions::Packages,
        error: Box<dyn std::error::Error>,
    ) -> Box<dyn std::error::Error> {
        match packages
            .store
            .as_ref()
            .and_then(|store| store.take_failure())
        {
            Some(failure) => format!("{failure}\n  = note: {error}").into(),
            None => error,
        }
    }

    /// The run's extension components: the values elements' `env` slots may
    /// reference, the outbound transport (none with `outbound` false: every
    /// request is refused), the CLI's observer, every `--extension-package`
    /// extension component authenticated by its pinned SHA-256 and bound, and
    /// the repository resolver that loads the repository namespaces a session's
    /// document uses when the session starts.
    #[cfg(feature = "wasmtime")]
    pub(crate) fn extensions(
        flags: &crate::extensions::RunFlags,
        packages: &crate::extensions::Packages,
        values: &[crate::extensions::HeldValue],
        outbound: bool,
    ) -> Result<(scxml_host_wasmtime::ExtensionComponents, Notes), Box<dyn std::error::Error>> {
        use scxml_host_wasmtime::{
            ExtensionComponents, TrustedComponentDigest, authenticate_extension_component,
        };

        let observer = std::sync::Arc::new(Observer::default());
        let mut extensions = ExtensionComponents::new()
            .with_values(crate::extensions::HeldValue::environment(values)?)
            .with_observer(std::sync::Arc::clone(&observer) as _);
        if outbound {
            extensions = extensions.with_outbound_http(std::sync::Arc::new(
                scxml_host_wasmtime::ExtensionHttpClient::new(
                    scxml_host_wasmtime::ExtensionHttpConfig::new(),
                )
                .map_err(|error| format!("[extension-http-invalid] {error}"))?,
            ));
        }
        for package in &packages.explicit {
            let path = flags.extension_component_path(package)?;
            let limit = u64::try_from(scxml_host_wasmtime::MAX_AUTHENTICATED_COMPONENT_BYTES)?;
            let bytes = crate::extensions::read_bounded(&path, limit).map_err(|error| {
                format!(
                    "cannot read the extension component `{}` of extension package `{}`: {error}",
                    path.display(),
                    package.label()
                )
            })?;
            let digest = flags
                .extension_component_sha256(package)
                .ok_or("every extension package needs --extension-component-sha256")?;
            let component = authenticate_extension_component(
                &bytes,
                TrustedComponentDigest::from_hex(digest)?,
            )
            .map_err(|error| {
                format!(
                    "the extension component `{}` of extension package `{}` is rejected: {error}",
                    path.display(),
                    package.label()
                )
            })?;
            extensions = extensions.bind(package.host_package(), &component)?;
        }
        if let Some(store) = &packages.store {
            extensions = extensions.with_resolver(std::sync::Arc::new(
                crate::repository::RepositoryResolver(std::sync::Arc::clone(store)),
            ));
        }
        Ok((extensions, Notes(observer)))
    }

    /// Whether this build executes documents (it does under Wasmtime).
    #[cfg(feature = "wasmtime")]
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the build without Wasmtime fails here"
    )]
    pub fn available() -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }

    /// Whether this build executes documents: without Wasmtime it fails
    /// closed before the document is read.
    #[cfg(not(feature = "wasmtime"))]
    pub fn available() -> Result<(), Box<dyn std::error::Error>> {
        Err(
            "[wasmtime-disabled] snapshot run executes only through the shipped components \
             under Wasmtime; rebuild scx with --features wasmtime. No document was \
             executed"
                .into(),
        )
    }

    /// Runs the document (never reached without Wasmtime: [`available`]
    /// fails first).
    #[cfg(not(feature = "wasmtime"))]
    pub fn run(
        _args: &RunArgs,
        _packages: &crate::extensions::Packages,
        _services: &crate::services::Services,
    ) -> Result<Vec<CheckpointDocument>, Box<dyn std::error::Error>> {
        available().map(|()| Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use scxml_snapshot::SnapshotErrorCode;

    use super::*;

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn run_args(extra: &[&str]) -> Result<RunArgs, String> {
        let mut values = vec![
            "run",
            "a.scxml",
            "--components",
            "dist",
            "--components-manifest-sha256",
            DIGEST,
        ];
        values.extend_from_slice(extra);
        match parse(&args(&values))? {
            SnapshotCommand::Run(run) => Ok(*run),
            other => panic!("expected run, got {other:?}"),
        }
    }

    #[test]
    fn run_parses_points_exclusions_and_destinations() {
        let run = run_args(&[]).expect("parses");
        assert_eq!(run.points, Points::Final);
        assert_eq!(run.output, Output::Stdout);
        let run = run_args(&[
            "--event",
            "go",
            "--at",
            "3",
            "--at",
            "1",
            "--at",
            "3",
            "--exclude",
            "queues",
            "--exclude",
            "data_model",
            "--trace-context",
            "preserve",
            "--output-dir",
            "out",
            "--resource",
            "child.scxml=c.scxml",
        ])
        .expect("parses");
        assert_eq!(run.events, ["go"]);
        assert_eq!(run.points, Points::At(vec![1, 3]));
        assert_eq!(run.options.exclude, [Section::DataModel, Section::Queues]);
        assert_eq!(run.options.trace_context, TraceContextPolicy::Preserve);
        assert_eq!(run.output, Output::Dir("out".into()));
        assert_eq!(
            run.resources,
            [("child.scxml".into(), PathBuf::from("c.scxml"))]
        );
        assert_eq!(
            run_args(&["--every", "--output-dir", "d"]).map(|run| run.points),
            Ok(Points::Every)
        );
        assert_eq!(
            run_args(&["--at", "2", "--output", "f.json"]).map(|run| run.output),
            Ok(Output::File("f.json".into()))
        );
        // Any checkpoint ordinal: the CLI imposes no checkpoint bound.
        assert_eq!(
            run_args(&["--at", "4294967295", "--output", "f.json"]).map(|run| run.points),
            Ok(Points::At(vec![u32::MAX]))
        );
    }

    #[test]
    fn run_rejects_conflicting_or_missing_options() {
        for (extra, needle) in [
            (&["--every", "--at", "1"][..], "mutually exclusive"),
            (&["--every"][..], "--output-dir"),
            (
                &["--at", "1", "--at", "2", "--output", "f"][..],
                "--output-dir",
            ),
            (
                &["--output", "f", "--output-dir", "d"][..],
                "mutually exclusive",
            ),
            (&["--exclude", "pending-sends"][..], "--exclude requires"),
            (&["--at", "x"][..], "--at requires"),
            (&["--trace-context", "keep"][..], "strip or preserve"),
            (&["--resource", "=p"][..], "URI=PATH"),
            (&["--evnt", "go"][..], "did you mean `--event`"),
        ] {
            let error = run_args(extra).expect_err("rejected");
            assert!(error.contains(needle), "{extra:?}: {error}");
        }
        let error = parse(&args(&[
            "run",
            "a.scx",
            "--components-manifest-sha256",
            "x",
        ]))
        .expect_err("one override flag alone");
        assert!(error.contains("together"), "{error}");
    }

    #[test]
    fn show_check_and_diff_parse_their_options() {
        assert_eq!(
            parse(&args(&[
                "show", "s.json", "--format", "json", "--color", "always"
            ])),
            Ok(SnapshotCommand::Show {
                path: "s.json".into(),
                format: Format::Json,
                color: Some(Color::Always),
            })
        );
        assert_eq!(
            parse(&args(&["check", "--format", "json", "s.json"])),
            Ok(SnapshotCommand::Check {
                path: "s.json".into(),
                format: Format::Json,
            })
        );
        assert_eq!(
            parse(&args(&[
                "diff",
                "a.json",
                "b.json",
                "--include-trace-context"
            ])),
            Ok(SnapshotCommand::Diff {
                before: "a.json".into(),
                after: "b.json".into(),
                format: Format::Human,
                include_trace_context: true,
                color: None,
            })
        );
        assert!(parse(&args(&["diff", "a.json"])).is_err());
        assert!(parse(&args(&["show"])).is_err());
        assert!(parse(&args(&["show", "a", "b"])).is_err());
        assert!(parse(&args(&["check", "s.json", "--format", "sarif"])).is_err());
        assert!(parse(&args(&["restore"])).is_err());
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn every_snapshot_code_has_an_explanation() {
        assert!(every_code_is_explained());
        let rendered = render_issue(
            "s.json",
            &SnapshotError::new(
                SnapshotErrorCode::NonCanonical,
                Some("/options/exclude".into()),
            ),
            b"{}",
        );
        assert!(rendered.starts_with(
            "error[SNAP-non-canonical]: the document is valid JSON but not its canonical encoding\n  --> s.json\n   = pointer: /options/exclude\n   = help: "
        ));
    }

    #[test]
    fn run_output_goes_to_numbered_files_or_one_destination() {
        let dir = std::env::temp_dir().join(format!("scx-snapshot-{}", std::process::id()));
        let documents = [
            CheckpointDocument {
                checkpoint: 0,
                bytes: b"{}".to_vec(),
            },
            CheckpointDocument {
                checkpoint: 12,
                bytes: b"{\"a\":1}".to_vec(),
            },
        ];
        write_documents(&Output::Dir(dir.clone()), &documents).expect("writes");
        assert_eq!(
            fs::read(dir.join("0000.snapshot.json")).expect("read"),
            b"{}"
        );
        assert_eq!(
            fs::read(dir.join("0012.snapshot.json")).expect("read"),
            b"{\"a\":1}"
        );
        let file = dir.join("one.json");
        write_documents(&Output::File(file.clone()), &documents[1..]).expect("writes");
        assert_eq!(fs::read(&file).expect("read"), b"{\"a\":1}");
        assert!(write_documents(&Output::File(file), &documents).is_err());
        fs::remove_dir_all(&dir).expect("cleanup");
    }
}
