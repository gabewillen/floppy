use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt::{self, Write as _};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
#[cfg(unix)]
use std::sync::atomic::Ordering;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::ffi::OsStr;
#[cfg(unix)]
use std::os::fd::AsRawFd;

#[cfg(unix)]
use rustix::fs::{
    AtFlags, CWD, FileType, Mode, OFlags, mkdirat, openat, renameat, statat, unlinkat,
};
#[cfg(unix)]
use rustix::io::{Errno, FdFlags, fcntl_setfd};
use sha2::{Digest, Sha256};

use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};

use crate::errata::{ErrataError, Erratum, LoadedErrata, apply_replacements, load_errata};
use crate::manifest::{
    CORPUS_FILE, CaseErratum, CaseManifest, CorpusKind, ErratumStage, ExpectedOutcome, IrpCase,
    LogOutcome, MANIFEST_VERSION, MAX_COMMAND_BUDGET, MAX_IRP_MANIFEST_BYTES, ManifestError,
    PreparationFailure, SuiteIdentity, SuiteManifest, TransformerIdentity, flatten_irp_manifest,
    parse_irp_manifest, sha256_bytes, source_set_digest, suite_identity_digest, write_suite,
};

/// The source id of a screened offline stylesheet.
const STYLESHEET_SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(0);

const DEFAULT_SAXON_TIMEOUT: Duration = Duration::from_secs(120);
/// Poll interval of [`wait_for_child`]; see that function for why the wait
/// polls instead of blocking on an exit event.
const SAXON_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_IDENTITY_STREAM_BYTES: usize = 4_096;
const MAX_TRANSFORMER_LAUNCHER_BYTES: usize = 4_096;
const MAX_TRANSFORMER_ARGUMENT_BYTES: usize = 4_096;
const MAX_EXECUTABLE_SEARCH_ENTRIES: usize = 256;
const OFFLINE_SAXON_OPTIONS: [&str; 7] = [
    "-a:off",
    "-p:off",
    "-dtd:off",
    "-expand:off",
    "-ext:off",
    "-xi:off",
    "-xsiloc:off",
];
const MAX_OFFLINE_XML_BYTES: usize = 8_388_608;
const MAX_DOWNLOAD_BYTES: u64 = 67_108_864;
const MAX_PATH_BYTES: usize = 4_096;
const MAX_PATH_COMPONENTS: usize = 256;
const MAX_URL_BYTES: usize = 8_192;
// Retained on every platform: these document the exact network-acquisition
// bounds enforced by the host subprocess path, which wasm guests cannot spawn.
#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
const MAX_REDIRECTS: usize = 8;
#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
const MAX_RESOLVED_ADDRESSES: usize = 32;
#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
const MAX_RESPONSE_HEADERS: usize = 131_072;
#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
const MAX_TEMP_ATTEMPTS: usize = 64;
#[cfg(unix)]
static TEMP_FILE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(target_os = "macos")]
const MACOS_SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
#[cfg(target_os = "macos")]
const MACOS_DYLD: &str = "/usr/lib/dyld";
const OFFLINE_ENVIRONMENT: [(&str, &str); 3] = [("LANG", "C"), ("LC_ALL", "C"), ("TZ", "UTC")];

#[derive(Clone, Debug, Eq, PartialEq)]
struct HttpsOrigin {
    host: String,
    port: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PinnedOrigin {
    origin: HttpsOrigin,
    addresses: Vec<IpAddr>,
}

/// Pluggable, explicitly configured transformer used during corpus preparation.
pub trait CorpusTransformer {
    /// Returns proof that this adapter owns enforced offline process isolation.
    ///
    /// The default rejects offline preparation. [`OfflineCapability`] cannot be
    /// constructed outside this module, so transformer implementations cannot
    /// self-attest that they prevent network access.
    fn offline_capability(&self) -> Option<OfflineCapability<'_>> {
        None
    }

    /// Returns the stable identity recorded in the prepared suite.
    ///
    /// # Errors
    ///
    /// Returns [`PrepareError`] when transformer identity cannot be obtained
    /// within configured process, output, or platform bounds.
    fn identity(&self) -> Result<TransformerIdentity, PrepareError>;
    /// Transforms one acquired source with one data-model template.
    ///
    /// # Errors
    ///
    /// Returns [`PrepareError`] when the transformer cannot start, times out,
    /// exits unsuccessfully, or does not create the requested output.
    fn transform(&self, source: &Path, template: &Path, output: &Path) -> Result<(), PrepareError>;
}

/// Proof that a transformer owns an enforced no-network process boundary.
///
/// The proof is borrowed from the configured transformer and has no public
/// constructor. Its mechanism name is a release-bounded value suitable for
/// diagnostics, not caller-provided metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OfflineCapability<'a> {
    mechanism: &'a str,
}

impl<'a> OfflineCapability<'a> {
    /// Returns the validated host isolation mechanism name.
    #[must_use]
    pub fn mechanism(&self) -> &'a str {
        self.mechanism
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OfflineIsolation {
    launcher: PathBuf,
    mechanism: &'static str,
    identity_digest: String,
}

/// Whether a Saxon process runs inside the verified offline sandbox.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Sandbox {
    /// Offline: the process launches through the no-network sandbox with a
    /// cleared environment.
    Enabled,
    /// Online: the configured command runs directly.
    Disabled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// External Saxon process adapter with bounded execution and identity capture.
pub struct SaxonTransformer {
    command: Vec<String>,
    timeout: Duration,
    offline_isolation: Option<OfflineIsolation>,
}

impl SaxonTransformer {
    /// Creates a transformer with a fixed 120-second timeout per Saxon process.
    ///
    /// The transformer owns each spawned process until it exits or is killed and
    /// reaped after the deadline. Use [`Self::with_timeout`] to select another
    /// non-zero timeout.
    ///
    /// # Errors
    ///
    /// Returns [`PrepareError`] when the command is empty or contains an empty
    /// argument.
    pub fn new(command: Vec<String>) -> Result<Self, PrepareError> {
        Self::configured(command, DEFAULT_SAXON_TIMEOUT, Sandbox::Disabled)
    }

    /// Creates a Saxon transformer for an offline preparation run.
    ///
    /// Offline execution accepts only a direct `saxon`/`saxon-he` executable or
    /// `java -jar LOCAL_PATH`. The adapter disables associated-stylesheet,
    /// query-parameter, DTD, entity expansion, extension, `XInclude`,
    /// and schema-location processing; callers cannot inject resolver, catalog,
    /// initializer, source, stylesheet, or output options. [`prepare_corpus`] additionally
    /// rejects source constructs that can request external XML, and parses each
    /// stylesheet with namespace resolution and rejects every XSLT element or
    /// attribute, literal-result namespace, and `XPath` function call outside the
    /// closed vocabulary the pinned W3C templates use (no named function
    /// references or dynamic calls), so no external or dynamic resource can be
    /// requested.
    ///
    /// # Errors
    ///
    /// Returns [`PrepareError`] when the command does not match the closed
    /// offline Saxon command contract, its files cannot be resolved and hashed,
    /// or the host cannot prove an enforced no-network process sandbox.
    pub fn offline(command: Vec<String>) -> Result<Self, PrepareError> {
        Self::configured(command, DEFAULT_SAXON_TIMEOUT, Sandbox::Enabled)
    }

    /// Creates a transformer with an explicit non-zero process timeout.
    ///
    /// Each identity or transformation child is synchronously observed by the
    /// caller. A child that reaches the deadline is killed and reaped before a
    /// typed `transformer-timeout` error is returned. Identity stdout and stderr
    /// are drained to avoid pipe deadlock, while at most 4 KiB from each stream
    /// is retained in memory.
    ///
    /// # Errors
    ///
    /// Returns a typed input [`PrepareError`] when the command is empty,
    /// contains an empty argument, or `timeout` is zero or not representable by
    /// the monotonic clock.
    pub fn with_timeout(command: Vec<String>, timeout: Duration) -> Result<Self, PrepareError> {
        Self::configured(command, timeout, Sandbox::Disabled)
    }

    fn sandbox(&self) -> Sandbox {
        if self.offline_isolation.is_some() {
            Sandbox::Enabled
        } else {
            Sandbox::Disabled
        }
    }

    fn configured(
        command: Vec<String>,
        timeout: Duration,
        sandbox: Sandbox,
    ) -> Result<Self, PrepareError> {
        if command.is_empty()
            || command.len() > crate::manifest::MAX_TRANSFORMER_COMMAND_ARGS
            || command.iter().any(String::is_empty)
            || command
                .iter()
                .any(|argument| argument.len() > MAX_TRANSFORMER_ARGUMENT_BYTES)
        {
            return Err(PrepareError::new(
                "transformer-command",
                "Saxon command must contain 1..=64 non-empty arguments of at most 4096 bytes",
            ));
        }
        if timeout.is_zero() || Instant::now().checked_add(timeout).is_none() {
            return Err(PrepareError::new(
                "transformer-timeout-invalid",
                "Saxon timeout must be non-zero and representable by the monotonic clock",
            ));
        }
        if sandbox == Sandbox::Enabled
            && let Err(error) = validate_offline_saxon_command(&command)
        {
            return Err(error);
        }
        let (command, offline_isolation) = if sandbox == Sandbox::Enabled {
            let mut isolation = offline_isolation()?;
            let command = resolve_offline_transformer_command(&command)?;
            let prefix = offline_identity_command(&command, &isolation)?;
            isolation.identity_digest =
                command_digest(&prefix, Some(&isolation))?.ok_or_else(|| {
                    PrepareError::new(
                        "transformer-offline-identity",
                        "offline command digest was not produced",
                    )
                })?;
            (command, Some(isolation))
        } else {
            (command, None)
        };
        Ok(Self {
            command,
            timeout,
            offline_isolation,
        })
    }

    /// Builds the exact process argument vector for one transformation.
    ///
    /// In offline mode the returned vector starts with the validated host
    /// sandbox and its network-denial profile. The resolved transformer command
    /// follows after the sandbox arguments; the raw transformer is never the
    /// spawned executable.
    ///
    /// # Errors
    ///
    /// Returns [`PrepareError`] when an offline executable or JAR cannot be
    /// resolved, bounded, inspected, or represented by the host sandbox.
    pub fn command_for(
        &self,
        source: &Path,
        template: &Path,
        output: &Path,
    ) -> Result<Vec<String>, PrepareError> {
        if let Some(isolation) = &self.offline_isolation {
            verify_offline_artifacts(&self.command, isolation)?;
            return offline_transform_command(&self.command, isolation, source, template, output);
        }
        let mut command = self.command.clone();
        command.push(format!("-s:{}", source.display()));
        command.push(format!("-xsl:{}", template.display()));
        command.push(format!("-o:{}", output.display()));
        Ok(command)
    }

    fn command_for_stdout(
        &self,
        source: &Path,
        template: &Path,
        output: &Path,
    ) -> Result<Vec<String>, PrepareError> {
        if let Some(isolation) = &self.offline_isolation {
            verify_offline_artifacts(&self.command, isolation)?;
            return offline_transform_command_stdout(
                &self.command,
                isolation,
                source,
                template,
                output,
            );
        }
        let mut command = self.command.clone();
        command.push(format!("-s:{}", source.display()));
        command.push(format!("-xsl:{}", template.display()));
        Ok(command)
    }

    fn command_prefix(&self) -> Result<Vec<String>, PrepareError> {
        let command = self.command.clone();
        if let Some(isolation) = &self.offline_isolation {
            let sandboxed = offline_identity_command(&command, isolation)?;
            let current_digest = command_digest(&sandboxed, Some(isolation))?.ok_or_else(|| {
                PrepareError::new(
                    "transformer-offline-identity",
                    "offline command digest was not produced",
                )
            })?;
            if current_digest != isolation.identity_digest {
                return Err(PrepareError::new(
                    "transformer-offline-artifact-changed",
                    "offline sandbox or transformer bytes changed after configuration",
                ));
            }
            return Ok(sandboxed);
        }
        Ok(command)
    }
}

impl CorpusTransformer for SaxonTransformer {
    fn offline_capability(&self) -> Option<OfflineCapability<'_>> {
        self.offline_isolation
            .as_ref()
            .map(|isolation| OfflineCapability {
                mechanism: isolation.mechanism,
            })
    }

    fn identity(&self) -> Result<TransformerIdentity, PrepareError> {
        let span = observe(
            Module::ConformancePrepare,
            Operation::PrepareTransformerIdentity,
        );
        let result = self.identity_bounded();
        if result.is_err() {
            span.fail(ErrorCategory::Platform);
        } else {
            span.event(SignalEvent::ExternalReadCompleted);
        }
        result
    }

    fn transform(&self, source: &Path, template: &Path, output: &Path) -> Result<(), PrepareError> {
        let span = observe(
            Module::ConformancePrepare,
            Operation::PrepareTransformerTransform,
        );
        let result = (|| {
            let command = self.command_for_stdout(source, template, output)?;
            let output_file = File::options()
                .write(true)
                .truncate(true)
                .open(output)
                .map_err(|error| {
                    PrepareError::new(
                        "transformer-output",
                        format!("cannot open descriptor-owned transformer output: {error}"),
                    )
                })?;
            let started_at = Instant::now();
            let (executable, arguments) = command.split_first().ok_or_else(|| {
                PrepareError::new("transformer-command", "Saxon command is empty")
            })?;
            let mut child_command = Command::new(executable);
            child_command
                .args(arguments)
                .stdout(Stdio::from(output_file));
            configure_offline_child(&mut child_command, self.sandbox());
            let mut child = child_command.spawn().map_err(|error| {
                PrepareError::new(
                    "transformer-spawn",
                    format!("cannot execute configured Saxon command: {error}"),
                )
            })?;
            let status = wait_for_child(&mut child, started_at, self.timeout)?;
            if !status.success() {
                return Err(PrepareError::new(
                    "transformer-failed",
                    format!("Saxon exited with status {status}"),
                ));
            }
            let metadata = fs::metadata(output).map_err(|error| {
                PrepareError::new(
                    "transformer-output",
                    format!("cannot inspect descriptor-owned transformer output: {error}"),
                )
            })?;
            if !metadata.is_file() || metadata.len() > MAX_DOWNLOAD_BYTES {
                return Err(PrepareError::new(
                    "transformer-output",
                    "Saxon did not produce a bounded regular output",
                ));
            }
            Ok(())
        })();
        if result.is_err() {
            span.fail(ErrorCategory::Harness);
        } else {
            span.event(SignalEvent::TransformationCompleted);
        }
        result
    }
}

impl SaxonTransformer {
    fn identity_bounded(&self) -> Result<TransformerIdentity, PrepareError> {
        let started_at = Instant::now();
        let command = self.command_prefix()?;
        let (executable, arguments) = command
            .split_first()
            .ok_or_else(|| PrepareError::new("transformer-command", "Saxon command is empty"))?;
        let mut child_command = Command::new(executable);
        child_command
            .args(arguments)
            .arg("-t")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_offline_child(&mut child_command, self.sandbox());
        let mut child = child_command.spawn().map_err(|error| {
            PrepareError::new(
                "transformer-version-spawn",
                format!("cannot execute configured Saxon command: {error}"),
            )
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            PrepareError::new(
                "transformer-version-output",
                "configured Saxon command did not expose stdout for identity capture",
            )
        });
        let stdout = match stdout {
            Ok(stdout) => stdout,
            Err(error) => {
                terminate_and_reap(&mut child)?;
                return Err(error);
            }
        };
        let stderr = child.stderr.take().ok_or_else(|| {
            PrepareError::new(
                "transformer-version-output",
                "configured Saxon command did not expose stderr for identity capture",
            )
        });
        let stderr = match stderr {
            Ok(stderr) => stderr,
            Err(error) => {
                terminate_and_reap(&mut child)?;
                return Err(error);
            }
        };
        let stdout_reader = match spawn_bounded_reader(stdout, "saxon-stdout") {
            Ok(reader) => reader,
            Err(error) => {
                terminate_and_reap(&mut child)?;
                return Err(error);
            }
        };
        let stderr_reader = match spawn_bounded_reader(stderr, "saxon-stderr") {
            Ok(reader) => reader,
            Err(error) => {
                terminate_and_reap(&mut child)?;
                let _captured = join_bounded_reader(stdout_reader)?;
                return Err(error);
            }
        };
        let status = wait_for_child(&mut child, started_at, self.timeout);
        let stdout = join_bounded_reader(stdout_reader);
        let stderr = join_bounded_reader(stderr_reader);
        let status = status?;
        let stdout = stdout?;
        let stderr = stderr?;
        let version = first_identity_line(&stdout, &stderr);
        if !status.success() && version == "unknown" {
            return Err(PrepareError::new(
                "transformer-version",
                "configured Saxon command did not expose an identity with -t",
            ));
        }
        let (command_argv, executable_digest) =
            portable_transformer_identity(&command, self.sandbox())?;
        Ok(TransformerIdentity {
            name: if self.offline_isolation.is_some() {
                "saxon-process-sandboxed"
            } else {
                "saxon-process"
            }
            .into(),
            version,
            command_argv,
            executable_digest,
        })
    }
}

/// Index of the transformer executable inside a sandboxed launch vector
/// (`sandbox-exec -p <profile> -- <transformer...>`).
const SANDBOXED_TRANSFORMER_INDEX: usize = 4;

/// Computes the portable, host-independent transformer identity recorded in
/// prepared corpora and reports.
///
/// The recorded argument vector is the transformer's own argv with every
/// absolute install path reduced to its file name, so the identity does not
/// embed one machine's install layout; the host sandbox wrapper is recorded
/// only through the `-sandboxed` transformer name. The digest binds the
/// portable argv and the transformer artifact bytes: the Saxon JAR for
/// `java -jar` launches (the JVM is a platform runtime outside the portable
/// identity) or the transformer executable otherwise. Host-specific
/// sandbox, policy, and runtime bytes stay bound by the in-process
/// configuration integrity check instead.
///
/// # Errors
///
/// Fails closed when the sandboxed launch vector is malformed, an artifact
/// cannot be hashed, or the identity cannot be encoded.
fn portable_transformer_identity(
    command: &[String],
    sandbox: Sandbox,
) -> Result<(Vec<String>, Option<String>), PrepareError> {
    let transformer = if sandbox == Sandbox::Enabled {
        command.get(SANDBOXED_TRANSFORMER_INDEX..).ok_or_else(|| {
            PrepareError::new(
                "transformer-offline-identity",
                "sandboxed command is missing its resolved transformer executable",
            )
        })?
    } else {
        command
    };
    let portable_argv = transformer
        .iter()
        .map(|argument| {
            let path = Path::new(argument);
            if path.is_absolute() {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        PrepareError::new(
                            "transformer-identity-invalid",
                            "transformer install path has no UTF-8 file name",
                        )
                    })
            } else {
                Ok(argument.clone())
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let jar = transformer
        .iter()
        .position(|argument| argument == "-jar")
        .and_then(|position| transformer.get(position.saturating_add(1)));
    let artifact = match jar {
        Some(jar) if Path::new(jar).is_absolute() => {
            Some(("transformer-jar", sha256_bounded_file(Path::new(jar))?))
        }
        Some(_) => None,
        None => match transformer.first() {
            Some(executable) if Path::new(executable).is_absolute() => Some((
                "transformer-executable",
                sha256_bounded_file(Path::new(executable))?,
            )),
            _ => executable_digest(transformer).map(|digest| ("transformer-executable", digest)),
        },
    };
    let Some(artifact) = artifact else {
        return Ok((portable_argv, None));
    };
    let encoded = serde_json::to_vec(&(&portable_argv, artifact)).map_err(|error| {
        PrepareError::new(
            "transformer-identity-invalid",
            format!("cannot encode portable transformer identity: {error}"),
        )
    })?;
    Ok((portable_argv, Some(sha256_bytes(&encoded))))
}

fn validate_offline_saxon_command(command: &[String]) -> Result<(), PrepareError> {
    let executable = command.first().ok_or_else(|| {
        PrepareError::new(
            "transformer-command",
            "offline Saxon command requires an executable",
        )
    })?;
    let direct = matches!(executable.as_str(), "saxon" | "saxon-he") && command.len() == 1;
    let java_jar = matches!(executable.as_str(), "java" | "java.exe")
        && command.len() == 3
        && command.get(1).is_some_and(|argument| argument == "-jar")
        && command.get(2).is_some_and(|jar| {
            !jar.starts_with('-')
                && !is_http_url(jar)
                && Path::new(jar)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("jar"))
        });
    if !direct && !java_jar {
        return Err(PrepareError::new(
            "transformer-offline-command",
            "offline preparation accepts only `saxon`, `saxon-he`, or `java -jar LOCAL_PATH`; transformer options are owned by scxml-conformance",
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn offline_isolation() -> Result<OfflineIsolation, PrepareError> {
    let launcher = canonical_regular_file(
        Path::new(MACOS_SANDBOX_EXEC),
        "transformer-offline-sandbox",
        "macOS sandbox-exec",
    )?;
    let _launcher_digest = sha256_bounded_file(&launcher)?;
    verify_macos_sandbox(&launcher)?;
    Ok(OfflineIsolation {
        launcher,
        mechanism: "macos-sandbox-exec-default-deny-v2",
        identity_digest: String::new(),
    })
}

#[cfg(not(target_os = "macos"))]
fn offline_isolation() -> Result<OfflineIsolation, PrepareError> {
    Err(PrepareError::new(
        "transformer-offline-isolation-unavailable",
        "offline transformation is unsupported on this host: a verified no-network process isolation mechanism is required",
    ))
}

#[cfg(target_os = "macos")]
fn verify_macos_sandbox(launcher: &Path) -> Result<(), PrepareError> {
    let probe = canonical_regular_file(
        Path::new("/usr/bin/true"),
        "transformer-offline-isolation-unavailable",
        "macOS sandbox network probe",
    )?;
    let started_at = Instant::now();
    let profile = macos_offline_profile(
        &[probe.to_string_lossy().into_owned()],
        &[PathBuf::from("/"), PathBuf::from(MACOS_DYLD), probe.clone()],
        None,
    )?;
    let mut child = Command::new(launcher)
        .args(["-p", &profile, "--", &probe.to_string_lossy()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            PrepareError::new(
                "transformer-offline-isolation-unavailable",
                format!("cannot start macOS sandbox verification: {error}"),
            )
        })?;
    let status = wait_for_child(&mut child, started_at, Duration::from_secs(5))?;
    if !status.success() {
        return Err(PrepareError::new(
            "transformer-offline-isolation-unavailable",
            format!("macOS default-deny sandbox verification failed with status {status}"),
        ));
    }
    Ok(())
}

fn resolve_offline_transformer_command(command: &[String]) -> Result<Vec<String>, PrepareError> {
    let executable = command.first().ok_or_else(|| {
        PrepareError::new(
            "transformer-command",
            "offline Saxon command requires an executable",
        )
    })?;
    #[cfg(target_os = "macos")]
    let resolved_executable = if matches!(executable.as_str(), "java" | "java.exe") {
        resolve_macos_java_executable()?
    } else {
        resolve_executable(executable)?
    };
    #[cfg(not(target_os = "macos"))]
    let resolved_executable = resolve_executable(executable)?;
    let mut resolved = if matches!(executable.as_str(), "saxon" | "saxon-he") {
        resolve_saxon_launcher(&resolved_executable)?
    } else {
        vec![resolved_executable.to_string_lossy().into_owned()]
    };
    if matches!(executable.as_str(), "java" | "java.exe") {
        let jar = command.get(2).ok_or_else(|| {
            PrepareError::new(
                "transformer-offline-command",
                "offline Java Saxon command requires a local JAR path",
            )
        })?;
        let canonical_jar =
            canonical_regular_file(Path::new(jar), "transformer-offline-artifact", "Saxon JAR")?;
        resolved.push("-jar".into());
        resolved.push(canonical_jar.to_string_lossy().into_owned());
    }
    Ok(resolved)
}

fn resolve_saxon_launcher(executable: &Path) -> Result<Vec<String>, PrepareError> {
    let bytes = read_bounded_file(
        executable,
        MAX_TRANSFORMER_LAUNCHER_BYTES,
        "transformer-offline-launcher",
        "transformer-offline-launcher-size",
    )?;
    if !bytes.starts_with(b"#!") {
        return Ok(vec![executable.to_string_lossy().into_owned()]);
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        PrepareError::new(
            "transformer-offline-launcher",
            "offline Saxon launcher script must be bounded UTF-8",
        )
    })?;
    let mut lines = text.lines();
    let shebang = lines.next();
    let java_home = lines
        .next()
        .and_then(|line| line.strip_prefix("export JAVA_HOME=\"${JAVA_HOME:-"))
        .and_then(|line| line.strip_suffix("}\""));
    let jar = lines
        .next()
        .and_then(|line| line.strip_prefix("exec \"${JAVA_HOME}/bin/java\""))
        .map(str::trim_start)
        .and_then(|line| line.strip_prefix("-jar \""))
        .and_then(|line| line.strip_suffix("\" \"$@\""));
    if shebang != Some("#!/bin/bash") || lines.any(|line| !line.trim().is_empty()) {
        return Err(PrepareError::new(
            "transformer-offline-launcher",
            "offline Saxon launcher script does not match the supported fixed Java/JAR form",
        ));
    }
    let java = java_home.ok_or_else(|| {
        PrepareError::new(
            "transformer-offline-launcher",
            "offline Saxon launcher does not declare a fixed default Java home",
        )
    })?;
    let jar = jar.ok_or_else(|| {
        PrepareError::new(
            "transformer-offline-launcher",
            "offline Saxon launcher does not declare a fixed Saxon JAR",
        )
    })?;
    let java = canonical_regular_file(
        &Path::new(java).join("bin/java"),
        "transformer-offline-runtime",
        "Saxon launcher Java executable",
    )?;
    let jar = canonical_regular_file(
        Path::new(jar),
        "transformer-offline-artifact",
        "Saxon launcher JAR",
    )?;
    Ok(vec![
        java.to_string_lossy().into_owned(),
        "-jar".into(),
        jar.to_string_lossy().into_owned(),
    ])
}

#[cfg(target_os = "macos")]
fn resolve_macos_java_executable() -> Result<PathBuf, PrepareError> {
    let helper = canonical_regular_file(
        Path::new("/usr/libexec/java_home"),
        "transformer-offline-runtime",
        "macOS Java runtime resolver",
    )?;
    let started_at = Instant::now();
    let mut child = Command::new(helper)
        .env_clear()
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            PrepareError::new(
                "transformer-offline-runtime",
                format!("cannot start macOS Java runtime resolver: {error}"),
            )
        })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        PrepareError::new(
            "transformer-offline-runtime",
            "macOS Java runtime resolver did not expose stdout",
        )
    })?;
    let reader = spawn_bounded_reader(stdout, "java-home-stdout")?;
    let status = wait_for_child(&mut child, started_at, Duration::from_secs(5));
    let captured = join_bounded_reader(reader);
    let status = status?;
    let captured = captured?;
    if !status.success() {
        return Err(PrepareError::new(
            "transformer-offline-runtime",
            format!("macOS Java runtime resolver failed with status {status}"),
        ));
    }
    let home = std::str::from_utf8(&captured)
        .map_err(|_| {
            PrepareError::new(
                "transformer-offline-runtime",
                "macOS Java runtime resolver output is not UTF-8",
            )
        })?
        .trim();
    if home.is_empty() {
        return Err(PrepareError::new(
            "transformer-offline-runtime",
            "macOS Java runtime resolver returned an empty path",
        ));
    }
    let executable = canonical_regular_file(
        &Path::new(home).join("bin/java"),
        "transformer-offline-runtime",
        "resolved Java executable",
    )?;
    Ok(executable)
}

fn sandboxed_command_prefix(
    command: &[String],
    isolation: &OfflineIsolation,
    profile: String,
) -> Vec<String> {
    let mut sandboxed = vec![
        isolation.launcher.to_string_lossy().into_owned(),
        "-p".into(),
        profile,
        "--".into(),
    ];
    sandboxed.extend(command.iter().cloned());
    sandboxed
}

fn configure_offline_child(command: &mut Command, sandbox: Sandbox) {
    if sandbox == Sandbox::Enabled {
        command
            .env_clear()
            .envs(OFFLINE_ENVIRONMENT)
            .current_dir(Path::new("/"));
    }
}

fn offline_identity_command(
    command: &[String],
    isolation: &OfflineIsolation,
) -> Result<Vec<String>, PrepareError> {
    let mut transformer = command.to_vec();
    append_offline_saxon_options(&mut transformer);
    let read_paths = offline_runtime_read_paths(command)?;
    let profile = macos_offline_profile(command, &read_paths, None)?;
    Ok(sandboxed_command_prefix(&transformer, isolation, profile))
}

fn offline_transform_command(
    command: &[String],
    isolation: &OfflineIsolation,
    source: &Path,
    template: &Path,
    output: &Path,
) -> Result<Vec<String>, PrepareError> {
    let (source, template, output, write_area) =
        canonical_transform_paths(source, template, output)?;
    let mut transformer = command.to_vec();
    append_offline_saxon_options(&mut transformer);
    transformer.push(format!("-s:{}", source.display()));
    transformer.push(format!("-xsl:{}", template.display()));
    transformer.push(format!("-o:{}", output.display()));
    sandboxed_transform_command(
        command,
        isolation,
        &transformer,
        &source,
        &template,
        &output,
        &write_area,
    )
}

fn offline_transform_command_stdout(
    command: &[String],
    isolation: &OfflineIsolation,
    source: &Path,
    template: &Path,
    output: &Path,
) -> Result<Vec<String>, PrepareError> {
    let (source, template, output, write_area) =
        canonical_transform_paths(source, template, output)?;
    let mut transformer = command.to_vec();
    append_offline_saxon_options(&mut transformer);
    transformer.push(format!("-s:{}", source.display()));
    transformer.push(format!("-xsl:{}", template.display()));
    sandboxed_transform_command(
        command,
        isolation,
        &transformer,
        &source,
        &template,
        &output,
        &write_area,
    )
}

fn sandboxed_transform_command(
    command: &[String],
    isolation: &OfflineIsolation,
    transformer: &[String],
    source: &Path,
    template: &Path,
    output: &Path,
    write_area: &Path,
) -> Result<Vec<String>, PrepareError> {
    let mut read_paths = offline_runtime_read_paths(command)?;
    read_paths.extend([
        source.to_path_buf(),
        template.to_path_buf(),
        output.to_path_buf(),
    ]);
    read_paths.sort();
    read_paths.dedup();
    let profile = macos_offline_profile(command, &read_paths, Some(write_area))?;
    Ok(sandboxed_command_prefix(transformer, isolation, profile))
}

fn canonical_transform_paths(
    source: &Path,
    template: &Path,
    output: &Path,
) -> Result<(PathBuf, PathBuf, PathBuf, PathBuf), PrepareError> {
    let source = canonical_offline_input(
        source,
        "transformer-offline-source",
        "offline transformation source",
    )?;
    let template = canonical_offline_input(
        template,
        "transformer-offline-stylesheet",
        "offline transformation stylesheet",
    )?;
    let parent = output.parent().ok_or_else(|| {
        PrepareError::new(
            "transformer-offline-output",
            "offline transformation output requires a parent directory",
        )
    })?;
    let parent = parent.canonicalize().map_err(|error| {
        PrepareError::new(
            "transformer-offline-output",
            format!("cannot resolve offline output directory: {error}"),
        )
    })?;
    if !parent.is_dir() {
        return Err(PrepareError::new(
            "transformer-offline-output",
            "offline transformation output parent must be a directory",
        ));
    }
    let file_name = output.file_name().ok_or_else(|| {
        PrepareError::new(
            "transformer-offline-output",
            "offline transformation output requires a file name",
        )
    })?;
    Ok((source, template, parent.join(file_name), parent.clone()))
}

fn canonical_offline_input(
    path: &Path,
    code: &'static str,
    label: &str,
) -> Result<PathBuf, PrepareError> {
    #[cfg(target_os = "macos")]
    if path.starts_with("/dev/fd") {
        let metadata = fs::metadata(path)
            .map_err(|error| PrepareError::new(code, format!("cannot inspect {label}: {error}")))?;
        if metadata.is_file() && path.as_os_str().as_encoded_bytes().len() <= MAX_PATH_BYTES {
            return Ok(path.to_path_buf());
        }
        return Err(PrepareError::new(
            code,
            format!("{label} descriptor must identify a bounded regular file"),
        ));
    }
    canonical_regular_file(path, code, label)
}

fn verify_offline_artifacts(
    command: &[String],
    isolation: &OfflineIsolation,
) -> Result<(), PrepareError> {
    let identity = offline_identity_command(command, isolation)?;
    let current = command_digest(&identity, Some(isolation))?.ok_or_else(|| {
        PrepareError::new(
            "transformer-offline-identity",
            "offline command digest was not produced",
        )
    })?;
    if current != isolation.identity_digest {
        return Err(PrepareError::new(
            "transformer-offline-artifact-changed",
            "offline sandbox, policy, runtime closure, or transformer bytes changed after configuration",
        ));
    }
    Ok(())
}

fn offline_runtime_read_paths(command: &[String]) -> Result<Vec<PathBuf>, PrepareError> {
    let executable = command.first().ok_or_else(|| {
        PrepareError::new(
            "transformer-offline-identity",
            "offline command is missing its executable",
        )
    })?;
    let mut paths = vec![PathBuf::from("/"), PathBuf::from(executable)];
    #[cfg(target_os = "macos")]
    paths.push(PathBuf::from(MACOS_DYLD));
    if command.get(1).is_some_and(|argument| argument == "-jar") {
        let jar = command.get(2).ok_or_else(|| {
            PrepareError::new(
                "transformer-offline-identity",
                "sandboxed Java command is missing its resolved Saxon JAR",
            )
        })?;
        let jar_path = PathBuf::from(jar);
        paths.push(jar_path.clone());
        if let Some(parent) = jar_path.parent() {
            let dependencies = parent.join("lib");
            if dependencies.is_dir() {
                paths.push(dependencies);
            }
        }
        let runtime = Path::new(executable)
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| {
                PrepareError::new(
                    "transformer-offline-runtime",
                    "offline Java executable has no bounded runtime root",
                )
            })?;
        paths.push(runtime.to_path_buf());
        for path in [
            "/System/Library",
            "/usr/lib",
            "/usr/share",
            "/private/etc",
            "/private/var/db/timezone",
            "/dev/null",
            "/dev/random",
            "/dev/urandom",
        ] {
            paths.push(PathBuf::from(path));
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn macos_offline_profile(
    command: &[String],
    read_paths: &[PathBuf],
    write_area: Option<&Path>,
) -> Result<String, PrepareError> {
    let executable = command.first().ok_or_else(|| {
        PrepareError::new(
            "transformer-offline-identity",
            "offline command is missing its executable",
        )
    })?;
    let java_runtime = command.get(1).is_some_and(|argument| argument == "-jar");
    let recursive_paths = read_paths
        .iter()
        .filter(|path| path.as_path() != Path::new("/") && path.is_dir())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut closed_paths = BTreeSet::new();
    for path in read_paths {
        let mut current = Some(path.as_path());
        while let Some(candidate) = current {
            closed_paths.insert(candidate.to_path_buf());
            current = candidate.parent();
        }
    }
    let mut profile = format!(
        "(version 1) (deny default) (allow process-exec (literal \"{}\")) (allow file-read*",
        sandbox_path(Path::new(executable))?
    );
    for path in &closed_paths {
        let matcher = if recursive_paths.contains(path) {
            "subpath"
        } else {
            "literal"
        };
        write!(profile, " ({matcher} \"{}\")", sandbox_path(path)?).map_err(|_| {
            PrepareError::new(
                "transformer-offline-policy",
                "cannot encode offline sandbox read policy",
            )
        })?;
    }
    profile.push(')');
    if let Some(write_area) = write_area {
        write!(
            profile,
            " (allow file-write* (subpath \"{}\"))",
            sandbox_path(write_area)?
        )
        .map_err(|_| {
            PrepareError::new(
                "transformer-offline-policy",
                "cannot encode offline sandbox write policy",
            )
        })?;
    }
    if java_runtime {
        profile.push_str(" (allow sysctl*) (allow mach*) (allow ipc*) (allow iokit*)");
    }
    Ok(profile)
}

fn sandbox_path(path: &Path) -> Result<String, PrepareError> {
    let value = path.to_str().ok_or_else(|| {
        PrepareError::new(
            "transformer-offline-path",
            "offline sandbox paths must be valid UTF-8",
        )
    })?;
    if value.len() > MAX_PATH_BYTES || value.contains(['\n', '\r', '\0']) {
        return Err(PrepareError::new(
            "transformer-offline-path",
            "offline sandbox path is invalid or exceeds the byte limit",
        ));
    }
    Ok(value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn append_offline_saxon_options(command: &mut Vec<String>) {
    command.extend(OFFLINE_SAXON_OPTIONS.into_iter().map(str::to_owned));
}

fn resolve_executable(value: &str) -> Result<PathBuf, PrepareError> {
    if Path::new(value).components().count() > 1 {
        return canonical_regular_file(
            Path::new(value),
            "transformer-offline-executable",
            "transformer executable",
        );
    }
    let search_path = std::env::var_os("PATH").ok_or_else(|| {
        PrepareError::new(
            "transformer-offline-executable",
            "PATH is unavailable while resolving the offline transformer executable",
        )
    })?;
    for directory in std::env::split_paths(&search_path).take(MAX_EXECUTABLE_SEARCH_ENTRIES) {
        let candidate = directory.join(value);
        if candidate.is_file() {
            let resolved = canonical_regular_file(
                &candidate,
                "transformer-offline-executable",
                "transformer executable",
            )?;
            return Ok(resolved);
        }
    }
    Err(PrepareError::new(
        "transformer-offline-executable",
        format!(
            "offline transformer executable `{value}` was not found in the bounded PATH search"
        ),
    ))
}

fn canonical_regular_file(
    path: &Path,
    code: &'static str,
    label: &str,
) -> Result<PathBuf, PrepareError> {
    if path.as_os_str().as_encoded_bytes().len() > MAX_PATH_BYTES {
        return Err(PrepareError::new(
            code,
            format!("{label} path exceeds the byte limit"),
        ));
    }
    let canonical = path.canonicalize().map_err(|error| {
        PrepareError::new(
            code,
            format!("cannot resolve {label} `{}`: {error}", path.display()),
        )
    })?;
    if !canonical.is_file() || canonical.as_os_str().as_encoded_bytes().len() > MAX_PATH_BYTES {
        return Err(PrepareError::new(
            code,
            format!("{label} must resolve to a bounded regular file"),
        ));
    }
    Ok(canonical)
}

fn command_digest(
    command: &[String],
    isolation: Option<&OfflineIsolation>,
) -> Result<Option<String>, PrepareError> {
    let Some(isolation) = isolation else {
        return Ok(executable_digest(command));
    };
    let transformer_index = SANDBOXED_TRANSFORMER_INDEX;
    let transformer = command.get(transformer_index).ok_or_else(|| {
        PrepareError::new(
            "transformer-offline-identity",
            "sandboxed command is missing its resolved transformer executable",
        )
    })?;
    let mut digests = BTreeMap::new();
    digests.insert("sandbox-exec", sha256_bounded_file(&isolation.launcher)?);
    let profile = command.get(2).ok_or_else(|| {
        PrepareError::new(
            "transformer-offline-identity",
            "sandboxed command is missing its generated policy",
        )
    })?;
    digests.insert("sandbox-profile", sha256_bytes(profile.as_bytes()));
    digests.insert(
        "transformer-executable",
        sha256_bounded_file(Path::new(transformer))?,
    );
    if command
        .get(transformer_index + 1)
        .is_some_and(|value| value == "-jar")
    {
        let jar = command.get(transformer_index + 2).ok_or_else(|| {
            PrepareError::new(
                "transformer-offline-identity",
                "sandboxed Java command is missing its resolved Saxon JAR",
            )
        })?;
        digests.insert("transformer-jar", sha256_bounded_file(Path::new(jar))?);
    }
    let encoded = serde_json::to_vec(&(command, digests)).map_err(|error| {
        PrepareError::new(
            "transformer-offline-identity",
            format!("cannot encode sandboxed command identity: {error}"),
        )
    })?;
    Ok(Some(sha256_bytes(&encoded)))
}

fn require_offline_transformer(transformer: &dyn CorpusTransformer) -> Result<(), PrepareError> {
    if transformer.offline_capability().is_none() {
        return Err(PrepareError::new(
            "transformer-offline-policy",
            "offline preparation requires the built-in transformer with enforced host process network isolation",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OfflineXmlKind {
    Source,
    Stylesheet,
}

fn validate_offline_xml(path: &Path, kind: OfflineXmlKind) -> Result<(), PrepareError> {
    let bytes = read_bounded_file(
        path,
        MAX_OFFLINE_XML_BYTES,
        "transformer-offline-input-read",
        "transformer-offline-input-size",
    )?;
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        PrepareError::new(
            "transformer-offline-input-encoding",
            "offline transformation inputs must be UTF-8 XML",
        )
    })?;
    let normalized = normalize_offline_xml(text)?;
    for forbidden in ["<!doctype", "<!entity", "<?xml-stylesheet"] {
        if normalized.contains(forbidden) {
            return Err(PrepareError::new(
                "transformer-offline-xml-resolution",
                "offline transformation input contains a forbidden external XML resolution construct",
            ));
        }
    }
    if kind == OfflineXmlKind::Stylesheet {
        screen_offline_stylesheet(path, text)?;
    } else if normalized.contains("xinclude")
        || normalized.contains(":include")
        || normalized.contains("<include")
    {
        return Err(PrepareError::new(
            "transformer-offline-xml-resolution",
            "offline source contains a forbidden include construct",
        ));
    }
    Ok(())
}

const XSLT_NAMESPACE: &str = "http://www.w3.org/1999/XSL/Transform";

/// How the offline stylesheet screen treats one allowed XSLT attribute.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StylesheetAttribute {
    /// A name or version that XSLT never evaluates.
    Plain,
    /// An `XPath` expression or pattern.
    Expression,
    /// An attribute value template (`{expr}` parts are `XPath`).
    ValueTemplate,
}

/// The closed offline stylesheet vocabulary: exactly the XSLT elements and
/// their no-namespace attributes that the pinned W3C IRP templates
/// (`tests/conformance/w3c-irp/templates`) and their checked-in errata use.
/// Everything else in the XSLT namespace (includes, imports, packages,
/// `evaluate`, `result-document`, `source-document`, functions, static
/// parameters, `use-when`, shadow attributes, text value templates, extension
/// declarations, ...) is rejected under any prefix.
const OFFLINE_XSLT_ELEMENTS: &[(&str, &[(&str, StylesheetAttribute)])] = &[
    ("stylesheet", &[("version", StylesheetAttribute::Plain)]),
    ("template", &[("match", StylesheetAttribute::Expression)]),
    (
        "apply-templates",
        &[("select", StylesheetAttribute::Expression)],
    ),
    ("copy", &[]),
    ("attribute", &[("name", StylesheetAttribute::ValueTemplate)]),
    ("value-of", &[("select", StylesheetAttribute::Expression)]),
    (
        "analyze-string",
        &[
            ("select", StylesheetAttribute::Expression),
            ("regex", StylesheetAttribute::ValueTemplate),
        ],
    ),
    ("matching-substring", &[]),
    ("text", &[]),
    ("variable", &[("name", StylesheetAttribute::Plain)]),
    ("choose", &[]),
    ("when", &[("test", StylesheetAttribute::Expression)]),
    ("otherwise", &[]),
];

/// Namespaces a literal result element may use: no namespace and SCXML, the
/// only ones the pinned templates emit.
const OFFLINE_LITERAL_NAMESPACES: &[Option<&str>] =
    &[None, Some("http://www.w3.org/2005/07/scxml")];

/// The only names an offline stylesheet's `XPath` may follow with `(`: the
/// `node()` kind test and `regex-group`, the one function the pinned
/// templates call. Unprefixed, so they resolve in the standard function
/// namespace; every prefixed or `Q{uri}` name is rejected.
const OFFLINE_XPATH_CALLS: &[&str] = &["node", "regex-group"];

/// `XPath` keywords that may directly precede a parenthesized expression
/// without calling anything (`$a and (...)`, `if (...)`, `for $x in (...)`).
/// None of them names a standard function.
const XPATH_KEYWORDS_BEFORE_PARENTHESIS: &[&str] = &[
    "and",
    "div",
    "else",
    "eq",
    "except",
    "ge",
    "gt",
    "idiv",
    "if",
    "in",
    "intersect",
    "is",
    "le",
    "lt",
    "mod",
    "ne",
    "or",
    "return",
    "satisfies",
    "then",
    "to",
    "union",
];

fn stylesheet_denied(message: &'static str) -> PrepareError {
    PrepareError::new("transformer-offline-stylesheet-resolution", message)
}

/// Screens an offline stylesheet against the closed vocabulary above.
///
/// The stylesheet is parsed with namespace resolution (no DTD, no entities
/// other than the predefined five and character references, bounded by the
/// repository XML limits), so element and attribute identity is the expanded
/// name, never the lexical prefix, and every attribute value is already
/// reference-decoded. Every XSLT element and attribute must be in the
/// vocabulary; every `XPath` expression, pattern, and attribute value template
/// is tokenized and may call only [`OFFLINE_XPATH_CALLS`], with no named
/// function reference (`name#arity`) and no dynamic function call.
fn screen_offline_stylesheet(path: &Path, text: &str) -> Result<(), PrepareError> {
    let source = scxml_syntax::SourceText::new(STYLESHEET_SOURCE, path.display().to_string(), text);
    let document = scxml_syntax::parse_xml(source).map_err(|_| {
        PrepareError::new(
            "transformer-offline-input-xml",
            "offline stylesheet is not well-formed, namespace-valid, bounded XML",
        )
    })?;
    if !document.root.name.is(Some(XSLT_NAMESPACE), "stylesheet") {
        return Err(stylesheet_denied(
            "offline stylesheet root must be an XSLT stylesheet element",
        ));
    }
    let mut outcome = Ok(());
    document.root.visit(&mut |element| {
        if outcome.is_ok() {
            outcome = screen_stylesheet_element(element);
        }
    });
    outcome
}

fn screen_stylesheet_element(element: &scxml_syntax::Element) -> Result<(), PrepareError> {
    let namespace = element.name.namespace_uri.as_deref();
    if namespace == Some(XSLT_NAMESPACE) {
        let allowed = OFFLINE_XSLT_ELEMENTS
            .iter()
            .find(|(name, _)| *name == element.name.local_name)
            .map(|(_, attributes)| *attributes)
            .ok_or_else(|| {
                stylesheet_denied(
                    "offline stylesheet uses an XSLT element outside the pinned template vocabulary",
                )
            })?;
        for attribute in &element.attributes {
            let usage = allowed
                .iter()
                .find(|(name, _)| {
                    attribute.name.namespace_uri.is_none() && *name == attribute.name.local_name
                })
                .map(|(_, usage)| *usage)
                .ok_or_else(|| {
                    stylesheet_denied(
                        "offline stylesheet uses an XSLT attribute outside the pinned template vocabulary",
                    )
                })?;
            match usage {
                StylesheetAttribute::Plain => {}
                StylesheetAttribute::Expression => screen_xpath(&attribute.value)?,
                StylesheetAttribute::ValueTemplate => screen_value_template(&attribute.value)?,
            }
        }
        return Ok(());
    }
    if !OFFLINE_LITERAL_NAMESPACES.contains(&namespace) {
        return Err(stylesheet_denied(
            "offline stylesheet emits a literal element outside the pinned template namespaces",
        ));
    }
    for attribute in &element.attributes {
        if attribute.name.namespace_uri.as_deref() == Some(XSLT_NAMESPACE) {
            return Err(stylesheet_denied(
                "offline stylesheet uses an XSLT attribute on a literal result element",
            ));
        }
        screen_value_template(&attribute.value)?;
    }
    Ok(())
}

/// Screens every `{expr}` part of an attribute value template; `{{` and `}}`
/// are literal braces.
fn screen_value_template(value: &str) -> Result<(), PrepareError> {
    let characters: Vec<char> = value.chars().collect();
    let mut index = 0;
    while let Some(&character) = characters.get(index) {
        index += 1;
        match character {
            '{' if characters.get(index) == Some(&'{') => index += 1,
            '}' if characters.get(index) == Some(&'}') => index += 1,
            '}' => {
                return Err(stylesheet_denied(
                    "offline stylesheet attribute value template has an unmatched '}'",
                ));
            }
            '{' => {
                let start = index;
                let end = value_template_expression_end(&characters, start)?;
                let expression: String = characters
                    .get(start..end)
                    .map(|slice| slice.iter().collect())
                    .unwrap_or_default();
                screen_xpath(&expression)?;
                index = end + 1;
            }
            _ => {}
        }
    }
    Ok(())
}

/// The index of the `}` closing the value-template expression that starts at
/// `start`, skipping string literals and nested braces.
fn value_template_expression_end(characters: &[char], start: usize) -> Result<usize, PrepareError> {
    let mut depth = 0_usize;
    let mut index = start;
    while let Some(&character) = characters.get(index) {
        match character {
            '"' | '\'' => index = xpath_string_end(characters, index)?,
            '{' => depth += 1,
            '}' if depth == 0 => return Ok(index),
            '}' => depth -= 1,
            _ => {}
        }
        index += 1;
    }
    Err(stylesheet_denied(
        "offline stylesheet attribute value template has an unterminated expression",
    ))
}

/// The index of the quote closing the `XPath` string literal opened at
/// `start` (a doubled quote is an escaped quote).
fn xpath_string_end(characters: &[char], start: usize) -> Result<usize, PrepareError> {
    let quote = characters.get(start).copied();
    let mut index = start + 1;
    while let Some(&character) = characters.get(index) {
        if Some(character) == quote {
            if characters.get(index + 1).copied() == quote {
                index += 2;
                continue;
            }
            return Ok(index);
        }
        index += 1;
    }
    Err(stylesheet_denied(
        "offline stylesheet XPath has an unterminated string literal",
    ))
}

/// The index just past the `XPath` comment `(: ... :)` opened at `start`
/// (comments nest).
fn xpath_comment_end(characters: &[char], start: usize) -> Result<usize, PrepareError> {
    let mut depth = 0_usize;
    let mut index = start;
    while let Some(&character) = characters.get(index) {
        let next = characters.get(index + 1).copied();
        if character == '(' && next == Some(':') {
            depth += 1;
            index += 2;
        } else if character == ':' && next == Some(')') {
            depth = depth.saturating_sub(1);
            index += 2;
            if depth == 0 {
                return Ok(index);
            }
        } else {
            index += 1;
        }
    }
    Err(stylesheet_denied(
        "offline stylesheet XPath has an unterminated comment",
    ))
}

fn is_xpath_name_start(character: char) -> bool {
    character == '_' || character.is_alphabetic()
}

fn is_xpath_name_character(character: char) -> bool {
    is_xpath_name_start(character) || character.is_numeric() || matches!(character, '-' | '.')
}

/// What the token before a `(` was, which decides whether that `(` calls a
/// function.
#[derive(Clone, Debug, Eq, PartialEq)]
enum XpathPrevious {
    /// Start of the expression or an operator: `(` only groups.
    Operator,
    /// A name: `(` calls it (or is a kind test or keyword grouping).
    Name(String),
    /// A variable, literal, or closing bracket: `(` is a dynamic call.
    Operand,
}

/// Tokenizes one `XPath` expression or pattern and rejects every function call
/// except [`OFFLINE_XPATH_CALLS`], every named function reference
/// (`name#arity`, under any prefix or `Q{uri}` form), and every dynamic
/// function call (`$f(...)`, `(...)(...)`, `...}(...)`).
fn screen_xpath(expression: &str) -> Result<(), PrepareError> {
    let characters: Vec<char> = expression.chars().collect();
    let mut previous = XpathPrevious::Operator;
    let mut index = 0;
    while let Some(&character) = characters.get(index) {
        let next = characters.get(index + 1).copied();
        match character {
            _ if character.is_whitespace() => index += 1,
            '"' | '\'' => {
                index = xpath_string_end(&characters, index)? + 1;
                previous = XpathPrevious::Operand;
            }
            '(' if next == Some(':') => index = xpath_comment_end(&characters, index)?,
            '(' => {
                match &previous {
                    XpathPrevious::Operator => {}
                    XpathPrevious::Name(name)
                        if XPATH_KEYWORDS_BEFORE_PARENTHESIS.contains(&name.as_str())
                            || OFFLINE_XPATH_CALLS.contains(&name.as_str()) => {}
                    XpathPrevious::Name(_) => {
                        return Err(stylesheet_denied(
                            "offline stylesheet XPath calls a function outside the pinned template vocabulary",
                        ));
                    }
                    XpathPrevious::Operand => {
                        return Err(stylesheet_denied(
                            "offline stylesheet XPath makes a dynamic function call",
                        ));
                    }
                }
                index += 1;
                previous = XpathPrevious::Operator;
            }
            '#' => {
                return Err(stylesheet_denied(
                    "offline stylesheet XPath uses a named function reference",
                ));
            }
            '$' => {
                index += 1;
                while characters.get(index).is_some_and(|c| c.is_whitespace()) {
                    index += 1;
                }
                let (end, _) = xpath_name_end(&characters, index)?;
                index = end;
                previous = XpathPrevious::Operand;
            }
            ')' | ']' | '}' => {
                index += 1;
                previous = XpathPrevious::Operand;
            }
            _ if character.is_ascii_digit()
                || (character == '.' && next.is_some_and(|c| c.is_ascii_digit())) =>
            {
                index += 1;
                while characters
                    .get(index)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '.')
                {
                    index += 1;
                }
                previous = XpathPrevious::Operand;
            }
            '.' => {
                index += 1;
                if characters.get(index) == Some(&'.') {
                    index += 1;
                }
                previous = XpathPrevious::Operand;
            }
            _ if is_xpath_name_start(character) => {
                let (end, name) = xpath_name_end(&characters, index)?;
                index = end;
                previous = XpathPrevious::Name(name);
            }
            _ => {
                index += 1;
                previous = XpathPrevious::Operator;
            }
        }
    }
    Ok(())
}

/// Reads the unprefixed, prefixed (`prefix:local`), or `Q{uri}local` name
/// that starts at `start`, returning the index past it and its lexical form.
fn xpath_name_end(characters: &[char], start: usize) -> Result<(usize, String), PrepareError> {
    let ncname_end = |from: usize| {
        let mut index = from;
        if characters
            .get(index)
            .is_some_and(|c| is_xpath_name_start(*c))
        {
            index += 1;
            while characters
                .get(index)
                .is_some_and(|c| is_xpath_name_character(*c))
            {
                index += 1;
            }
        }
        index
    };
    let lexical = |end: usize| -> String {
        characters
            .get(start..end)
            .map(|slice| slice.iter().collect())
            .unwrap_or_default()
    };
    if characters.get(start) == Some(&'Q') && characters.get(start + 1) == Some(&'{') {
        let close = characters
            .get(start + 2..)
            .and_then(|rest| rest.iter().position(|c| *c == '}'))
            .map(|offset| start + 2 + offset)
            .ok_or_else(|| {
                stylesheet_denied("offline stylesheet XPath has an unterminated Q{} name")
            })?;
        let end = ncname_end(close + 1);
        return Ok((end, lexical(end)));
    }
    let mut end = ncname_end(start);
    if end == start {
        return Err(stylesheet_denied(
            "offline stylesheet XPath has a '$' without a variable name",
        ));
    }
    if characters.get(end) == Some(&':')
        && characters
            .get(end + 1)
            .is_some_and(|c| is_xpath_name_start(*c))
    {
        end = ncname_end(end + 1);
    }
    Ok((end, lexical(end)))
}

fn normalize_offline_xml(text: &str) -> Result<String, PrepareError> {
    let mut uncommented = String::with_capacity(text.len());
    let mut remaining = text;
    while let Some(start) = remaining.find("<!--") {
        uncommented.push_str(&remaining[..start]);
        let after_start = &remaining[start + 4..];
        let end = after_start.find("-->").ok_or_else(|| {
            PrepareError::new(
                "transformer-offline-input-xml",
                "offline transformation input contains an unterminated XML comment",
            )
        })?;
        remaining = &after_start[end + 3..];
    }
    uncommented.push_str(remaining);

    let mut normalized = String::with_capacity(uncommented.len());
    let mut characters = uncommented.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '&' {
            let mut reference = String::new();
            while reference.len() <= 16 {
                let Some(next) = characters.next() else {
                    break;
                };
                if next == ';' {
                    break;
                }
                reference.push(next);
            }
            if let Some(decoded) = decode_xml_reference(&reference) {
                if !decoded.is_ascii_whitespace() {
                    normalized.push(decoded.to_ascii_lowercase());
                }
            } else {
                normalized.push('&');
                normalized.push_str(&reference.to_ascii_lowercase());
                normalized.push(';');
            }
        } else if !character.is_ascii_whitespace() {
            normalized.extend(character.to_lowercase());
        }
    }
    Ok(normalized)
}

fn decode_xml_reference(reference: &str) -> Option<char> {
    match reference {
        "lt" => Some('<'),
        "gt" => Some('>'),
        "amp" => Some('&'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        value if value.starts_with("#x") => u32::from_str_radix(value.get(2..)?, 16)
            .ok()
            .and_then(char::from_u32),
        value if value.starts_with('#') => {
            value.get(1..)?.parse::<u32>().ok().and_then(char::from_u32)
        }
        _ => None,
    }
}

/// Waits for `child` to exit, killing and reaping it when `timeout` elapses
/// from `started_at` or when its status cannot be observed.
///
/// Polling is deliberate (PAT-EVENT-002): no safe, portable exit event with a
/// deadline exists for a `std::process::Child`. `Child` has no
/// wait-with-timeout. Blocking in `Child::wait` on a waiter thread (with a
/// channel `recv_timeout` here) would move the `Child` into that thread, so
/// the timeout path could no longer call `Child::kill`; killing by raw pid
/// instead races the waiter's reap (the pid may already be reused by an
/// unrelated process). The race-free exit events are platform-specific
/// descriptors (`pidfd` on Linux, `kqueue` `EVFILT_PROC` on macOS) with no
/// equivalent on other hosts, and this crate keeps `#![forbid(unsafe_code)]`
/// and one code path. The poll is bounded: it sleeps at most
/// [`SAXON_WAIT_POLL_INTERVAL`] (never past the deadline) between
/// `try_wait` calls, so the loop runs at most `timeout / 10 ms + 1` times and
/// adds at most 10 ms of exit latency to a one-shot preparation command.
fn wait_for_child(
    child: &mut Child,
    started_at: Instant,
    timeout: Duration,
) -> Result<ExitStatus, PrepareError> {
    let deadline = started_at.checked_add(timeout).ok_or_else(|| {
        PrepareError::new(
            "transformer-timeout-invalid",
            "Saxon timeout cannot be represented by the monotonic clock",
        )
    })?;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ok(status);
            }
            Ok(None) => {}
            Err(error) => {
                terminate_and_reap(child)?;
                return Err(PrepareError::new(
                    "transformer-wait",
                    format!("cannot observe configured Saxon command: {error}"),
                ));
            }
        }
        let now = Instant::now();
        if now >= deadline {
            terminate_and_reap(child)?;
            return Err(PrepareError::new(
                "transformer-timeout",
                format!(
                    "configured Saxon command exceeded {} ms",
                    timeout.as_millis()
                ),
            ));
        }
        thread::sleep(SAXON_WAIT_POLL_INTERVAL.min(deadline.saturating_duration_since(now)));
    }
}

fn terminate_and_reap(child: &mut Child) -> Result<(), PrepareError> {
    if let Err(error) = child.kill()
        && error.kind() != io::ErrorKind::InvalidInput
    {
        return Err(PrepareError::new(
            "transformer-kill",
            format!("cannot kill configured Saxon command: {error}"),
        ));
    }
    child.wait().map_err(|error| {
        PrepareError::new(
            "transformer-reap",
            format!("cannot reap configured Saxon command: {error}"),
        )
    })?;
    Ok(())
}

fn spawn_bounded_reader<R>(
    mut reader: R,
    thread_name: &'static str,
) -> Result<JoinHandle<io::Result<Vec<u8>>>, PrepareError>
where
    R: Read + Send + 'static,
{
    thread::Builder::new()
        .name(thread_name.into())
        .spawn(move || {
            let mut captured = Vec::with_capacity(MAX_IDENTITY_STREAM_BYTES);
            let mut buffer = [0_u8; 4_096];
            loop {
                let count = match reader.read(&mut buffer) {
                    Ok(count) => count,
                    Err(error) => {
                        return Err(error);
                    }
                };
                if count == 0 {
                    return Ok(captured);
                }
                let remaining = MAX_IDENTITY_STREAM_BYTES.saturating_sub(captured.len());
                captured.extend_from_slice(&buffer[..count.min(remaining)]);
            }
        })
        .map_err(|error| {
            PrepareError::new(
                "transformer-version-reader",
                format!("cannot start bounded Saxon identity reader: {error}"),
            )
        })
}

fn join_bounded_reader(handle: JoinHandle<io::Result<Vec<u8>>>) -> Result<Vec<u8>, PrepareError> {
    handle
        .join()
        .map_err(|_| {
            PrepareError::new(
                "transformer-version-reader",
                "bounded Saxon identity reader terminated unexpectedly",
            )
        })?
        .map_err(|error| {
            PrepareError::new(
                "transformer-version-read",
                format!("cannot read bounded Saxon identity output: {error}"),
            )
        })
}

fn first_identity_line(stdout: &[u8], stderr: &[u8]) -> String {
    [stdout, stderr]
        .into_iter()
        .flat_map(|bytes| bytes.split(|byte| *byte == b'\n'))
        .find_map(|line| {
            let text = String::from_utf8_lossy(line);
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        })
        .unwrap_or_else(|| "unknown".into())
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Inputs and trust-boundary policy for preparing one corpus.
pub struct PrepareOptions {
    /// Official manifest URI or local path.
    pub manifest: String,
    /// Trusted corpus base URI when the manifest does not declare `baseuri`.
    pub base_uri: Option<String>,
    /// Directory containing pinned data-model XSLT templates.
    pub templates: PathBuf,
    /// Data-model template name to apply.
    pub datamodel: String,
    /// Cache directory for exact acquired source bytes.
    pub cache: PathBuf,
    /// Destination directory for the prepared corpus.
    pub output: PathBuf,
    /// Optional local source tree used instead of network acquisition.
    pub source_dir: Option<PathBuf>,
    /// Required non-empty `OfficialIrp` revision recorded as provenance.
    pub revision: Option<String>,
    /// Whether preparation must avoid all network access.
    pub offline: bool,
    /// Whether existing prepared artifacts may be replaced.
    pub force: bool,
    /// Default execution command budget recorded in the suite manifest.
    pub max_commands: u64,
    /// Optional checked-in errata file for this data model
    /// ([`crate::errata`]); its identity is recorded in the prepared suite.
    pub errata: Option<PathBuf>,
    /// The repository namespace (`namespaces/<name>`) whose own
    /// implementation report plan this manifest is: the corpus is then
    /// [`CorpusKind::NamespaceIrp`] with suite id `namespaces/<name>/irp`.
    /// `None` prepares the official W3C IRP ([`CorpusKind::OfficialIrp`]).
    pub namespace: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Counts and output path produced by a completed preparation run.
pub struct PrepareSummary {
    /// Path to the written canonical corpus manifest.
    pub corpus: PathBuf,
    /// Number of official start documents selected for preparation.
    pub selected_cases: usize,
    /// Selected starts whose exact upstream bytes were acquired and hashed.
    pub acquired_cases: usize,
    /// Number of selected cases transformed successfully.
    pub prepared_cases: usize,
    /// Number of selected cases retained with explicit preparation failures.
    pub preparation_gaps: usize,
    /// Number of immutable artifacts reused from the existing output.
    pub reused_artifacts: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// A bounded corpus acquisition, transformation, or output failure.
pub struct PrepareError {
    /// Stable machine-readable failure code.
    pub code: &'static str,
    /// Human-readable failure description.
    pub message: String,
}

impl PrepareError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for PrepareError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for PrepareError {}

impl From<ErrataError> for PrepareError {
    fn from(error: ErrataError) -> Self {
        Self::new(error.code, error.message)
    }
}

impl From<ManifestError> for PrepareError {
    fn from(error: ManifestError) -> Self {
        Self::new(error.code, error.message)
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one explicit preparation pipeline owns acquisition, transformation, and digest binding in order"
)]
/// Prepares an immutable, digest-bound corpus from official manifest inputs.
///
/// Acquisition is local-only when `options.offline` is set. All selected case
/// failures that can be represented safely are retained as explicit manifest
/// preparation gaps rather than silently removed.
///
/// # Errors
///
/// Returns [`PrepareError`] when options are invalid; trusted inputs, templates,
/// cache, or output paths cannot be validated; acquisition violates URI,
/// redirect, size, or offline policy; transformation fails; digests cannot be
/// computed; or the canonical suite manifest cannot be written. Before the
/// atomic rename, failures leave the previous destination unchanged and remove
/// the temporary file. A directory-sync failure after rename means the new
/// bytes are visible but their crash durability is unknown.
pub fn prepare_corpus(
    options: &PrepareOptions,
    transformer: &dyn CorpusTransformer,
) -> Result<PrepareSummary, PrepareError> {
    let span = observe(Module::ConformancePrepare, Operation::PrepareCorpus);
    let result = (|| {
        let revision = options.revision.as_deref().ok_or_else(|| {
            PrepareError::new(
                "official-revision-required",
                "official IRP preparation requires a non-empty pinned revision",
            )
        })?;
        if revision.is_empty() || revision.len() > crate::manifest::MAX_MANIFEST_STRING_BYTES {
            return Err(PrepareError::new(
                "official-revision-required",
                "official IRP preparation requires a non-empty bounded pinned revision",
            ));
        }
        if !(1..=MAX_COMMAND_BUDGET).contains(&options.max_commands) {
            return Err(PrepareError::new(
                "command-budget-invalid",
                format!("max_commands must be between 1 and {MAX_COMMAND_BUDGET}"),
            ));
        }
        ensure_directory(&options.cache, "cache-create")?;
        ensure_directory(&options.output, "output-create")?;
        let manifest_file = acquire_manifest(options)?;
        let manifest_bytes = read_bounded_file(
            &manifest_file,
            MAX_IRP_MANIFEST_BYTES,
            "manifest-read",
            "manifest-size",
        )?;
        let manifest_digest = sha256_bytes(&manifest_bytes);
        let inferred_base_uri = options
            .base_uri
            .as_deref()
            .or_else(|| http_parent(&options.manifest));
        let manifest = parse_irp_manifest(
            &manifest_file.display().to_string(),
            inferred_base_uri,
            &manifest_bytes,
        )?;
        let trusted_origin = trusted_online_origin(options)?;
        if let Some(origin) = &trusted_origin {
            require_same_origin(&manifest.base_uri, origin, "manifest base URI")?;
        } else if !options.offline && options.source_dir.is_none() {
            return Err(PrepareError::new(
                "download-origin-missing",
                "online corpus acquisition requires an explicit manifest URL or base URI",
            ));
        }
        let cases = flatten_irp_manifest(&manifest)?;
        let (suite_id, kind) = if let Some(name) = options.namespace.as_deref() {
            if !is_namespace_name(name) {
                return Err(PrepareError::new(
                    "namespace-name",
                    "a namespace name is [a-z][a-z0-9-]* of at most 64 bytes",
                ));
            }
            if options.errata.is_some() {
                return Err(PrepareError::new(
                    "namespace-errata",
                    "errata correct official W3C cases only; a namespace suite takes none",
                ));
            }
            (format!("namespaces/{name}/irp"), CorpusKind::NamespaceIrp)
        } else {
            if let Some(case) = cases.iter().find(|case| !case.rejects.is_empty()) {
                return Err(PrepareError::new(
                    "official-case-oracle",
                    format!(
                        "official case `{}` declares a rejection; the W3C IRP has none",
                        case.id
                    ),
                ));
            }
            ("w3c-scxml-irp".to_owned(), CorpusKind::OfficialIrp)
        };
        validate_existing_directory(&options.templates, "template-directory")?;
        let template_relative = safe_relative(&format!("{}.xsl", options.datamodel))?;
        let template = confined_path(&options.templates, &template_relative)?;
        validate_regular_file(&template, "template-missing", "template")?;
        if options.offline {
            require_offline_transformer(transformer)?;
            validate_offline_xml(&template, OfflineXmlKind::Stylesheet)?;
        }
        let template_digest = sha256_bounded_file(&template)?;
        let errata = options
            .errata
            .as_deref()
            .map(|path| load_bound_errata(path, options, &template_digest, &cases))
            .transpose()?;
        let transformer_identity = transformer.identity()?;
        let source_root = options.source_dir.clone().unwrap_or_else(|| {
            if is_http_url(&options.manifest) {
                options.cache.join("sources")
            } else {
                manifest_file
                    .parent()
                    .map_or_else(|| options.cache.join("sources"), Path::to_path_buf)
            }
        });
        ensure_directory(&source_root, "source-cache-create")?;
        validate_transformer_identity(&transformer_identity)?;
        let artifact_key = transformer_artifact_key(
            &manifest_digest,
            &template_digest,
            &options.datamodel,
            &transformer_identity,
        )?;
        let artifact_root = options.output.join("artifacts").join(&artifact_key);
        ensure_directory(&artifact_root, "artifact-create")?;
        let mut prepared = Vec::with_capacity(cases.len());
        let mut acquired_cases = 0_usize;
        let mut prepared_cases = 0_usize;
        let mut preparation_gaps = 0_usize;
        let mut reused_artifacts = 0_usize;
        for case in &cases {
            let (prepared_case, reused) = prepare_case(
                options,
                transformer,
                &manifest.base_uri,
                &source_root,
                &artifact_root,
                case,
                errata.as_ref(),
            )?;
            if let Some(erratum) = errata
                .as_ref()
                .and_then(|errata| errata.erratum_for(&case.id))
                && prepared_case.erratum.is_none()
            {
                return Err(PrepareError::new(
                    "erratum-upstream-unavailable",
                    format!(
                        "erratum `{}` cannot be applied because case `{}` has no upstream output",
                        erratum.id, case.id
                    ),
                ));
            }
            if prepared_case.source_digest.is_some() {
                acquired_cases += 1;
            }
            if prepared_case.preparation_failure.is_some() {
                preparation_gaps += 1;
            } else if prepared_case.source.is_some() {
                prepared_cases += 1;
            }
            if reused {
                reused_artifacts += 1;
            }
            prepared.push(prepared_case);
        }
        let source_digest = source_set_digest(&prepared)?;
        let mut template_digests = BTreeMap::new();
        template_digests.insert(options.datamodel.clone(), template_digest);
        let suite = SuiteManifest {
            manifest_version: MANIFEST_VERSION,
            suite: SuiteIdentity {
                id: suite_id,
                version: revision.to_owned(),
                kind,
                source: persisted_manifest_source(&options.manifest)?,
                revision: options.revision.clone(),
                corpus_digest: String::new(),
                source_digest,
                manifest_digest,
                template_digests,
                transformer: Some(transformer_identity),
                datamodel: options.datamodel.clone(),
                provenance: Some(format!(
                    "baseuri={}",
                    persisted_manifest_source(&manifest.base_uri)?
                )),
                errata: errata.as_ref().map(LoadedErrata::identity),
            },
            default_command_budget: options.max_commands,
            cases: prepared,
        };
        let mut suite = suite;
        suite.suite.corpus_digest = suite_identity_digest(&suite)?;
        // The corpus carries the exact errata bytes it binds, beside its
        // manifest, so loading rehashes them against the declared digest.
        if let Some(errata) = &errata {
            let bound = options.output.join(bound_errata_name(&errata.source)?);
            atomic_output_file(&bound, "errata", |writer| {
                writer.write_all(&errata.bytes).map_err(|error| {
                    PrepareError::new(
                        "errata-write",
                        format!("cannot write the corpus errata copy: {error}"),
                    )
                })
            })?;
        }
        let corpus = options.output.join(CORPUS_FILE);
        atomic_output(&corpus, "suite", |temporary| {
            write_suite(temporary, &suite).map_err(PrepareError::from)
        })?;
        if reused_artifacts > 0 {
            span.event(SignalEvent::ArtifactReused);
        }
        span.event(SignalEvent::ArtifactProduced);
        Ok(PrepareSummary {
            corpus,
            selected_cases: cases.len(),
            acquired_cases,
            prepared_cases,
            preparation_gaps,
            reused_artifacts,
        })
    })();
    if result.is_err() {
        span.fail(ErrorCategory::Platform);
    }
    result
}

/// Loads the errata file and binds it to this preparation: its data model and
/// template digest must match, and every case it names must be an official
/// case of the manifest.
fn load_bound_errata(
    path: &Path,
    options: &PrepareOptions,
    template_digest: &str,
    cases: &[IrpCase],
) -> Result<LoadedErrata, PrepareError> {
    let errata = load_errata(path)?;
    if errata.file.datamodel != options.datamodel {
        return Err(PrepareError::new(
            "errata-datamodel-mismatch",
            format!(
                "errata govern data model `{}` but preparation selected `{}`",
                errata.file.datamodel, options.datamodel
            ),
        ));
    }
    if errata.file.template_sha256 != template_digest {
        return Err(PrepareError::new(
            "errata-template-drift",
            format!(
                "errata were authored against template {} but the pinned template is {template_digest}",
                errata.file.template_sha256
            ),
        ));
    }
    if let Some(unknown) = errata
        .case_ids()
        .find(|id| !cases.iter().any(|case| case.id == *id))
    {
        return Err(PrepareError::new(
            "errata-unknown-case",
            format!("errata name case `{unknown}`, which the official manifest does not define"),
        ));
    }
    Ok(errata)
}

/// The errata file name a prepared corpus keeps beside its manifest: one
/// plain file name that is not the manifest itself
/// ([`crate::manifest::bound_errata_path`] applies the same rule on load).
fn bound_errata_name(source: &str) -> Result<&str, PrepareError> {
    let mut components = Path::new(source).components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(name)), None)
            if name == source && source != CORPUS_FILE =>
        {
            Ok(source)
        }
        _ => Err(PrepareError::new(
            "errata-name",
            "the errata file name cannot be written beside the corpus manifest",
        )),
    }
}

/// Paths and digests of one applied erratum.
struct AppliedErratum {
    corrected_input: PathBuf,
    corrected_input_digest: String,
    corrected_output: PathBuf,
    corrected_output_digest: String,
    upstream_output_digest: String,
}

/// Applies one erratum: verifies the exact upstream output digest, corrects
/// the named transformation input, transforms it, and verifies the corrected
/// output digest. Every mismatch fails closed.
#[allow(
    clippy::too_many_arguments,
    reason = "one erratum binds the case source, template, upstream output, and artifact root explicitly"
)]
fn apply_case_erratum(
    options: &PrepareOptions,
    transformer: &dyn CorpusTransformer,
    erratum: &Erratum,
    source: &Path,
    template: &Path,
    upstream_output: &Path,
    case_artifact_root: &Path,
    start_uri: &str,
    output_relative: &Path,
) -> Result<AppliedErratum, PrepareError> {
    let upstream_output_digest = sha256_bounded_file(upstream_output)?;
    if upstream_output_digest != erratum.upstream_output_sha256 {
        return Err(PrepareError::new(
            "erratum-upstream-drift",
            format!(
                "erratum `{}` expects upstream output {} but the official inputs produced {upstream_output_digest}",
                erratum.id, erratum.upstream_output_sha256
            ),
        ));
    }
    let erratum_root = case_artifact_root
        .join("errata")
        .join(safe_relative(&erratum.id)?);
    let (input, input_relative) = match erratum.stage {
        ErratumStage::Template => (
            template,
            safe_relative(&format!("{}.xsl", options.datamodel))?,
        ),
        ErratumStage::Source => (source, safe_relative(start_uri)?),
    };
    let original = read_bounded_file(
        input,
        MAX_OFFLINE_XML_BYTES,
        "erratum-input-read",
        "erratum-input-size",
    )?;
    let corrected = apply_replacements(&original, &erratum.replacements)?;
    let corrected_input = erratum_root.join("input").join(input_relative);
    ensure_parent(&corrected_input)?;
    atomic_output_file(&corrected_input, "erratum-input", |writer| {
        writer.write_all(&corrected).map_err(|error| {
            PrepareError::new(
                "erratum-input-write",
                format!("cannot write corrected erratum input: {error}"),
            )
        })
    })?;
    let corrected_output = erratum_root.join("cases").join(output_relative);
    ensure_parent(&corrected_output)?;
    match erratum.stage {
        ErratumStage::Template => {
            if options.offline {
                validate_offline_xml(&corrected_input, OfflineXmlKind::Stylesheet)?;
            }
            atomic_transform(transformer, source, &corrected_input, &corrected_output)?;
        }
        ErratumStage::Source => {
            if options.offline {
                validate_offline_xml(&corrected_input, OfflineXmlKind::Source)?;
            }
            atomic_transform(transformer, &corrected_input, template, &corrected_output)?;
        }
    }
    let corrected_output_digest = sha256_bounded_file(&corrected_output)?;
    if corrected_output_digest != erratum.corrected_output_sha256 {
        return Err(PrepareError::new(
            "erratum-corrected-drift",
            format!(
                "erratum `{}` expects corrected output {} but the corrected inputs produced {corrected_output_digest}",
                erratum.id, erratum.corrected_output_sha256
            ),
        ));
    }
    Ok(AppliedErratum {
        corrected_input_digest: sha256_bytes(&corrected),
        corrected_input,
        corrected_output,
        corrected_output_digest,
        upstream_output_digest,
    })
}

fn validate_transformer_identity(identity: &TransformerIdentity) -> Result<(), PrepareError> {
    let valid = !identity.name.is_empty()
        && identity.name.len() <= crate::manifest::MAX_MANIFEST_STRING_BYTES
        && !identity.version.is_empty()
        && identity.version.len() <= crate::manifest::MAX_MANIFEST_STRING_BYTES
        && !identity.command_argv.is_empty()
        && identity.command_argv.len() <= crate::manifest::MAX_TRANSFORMER_COMMAND_ARGS
        && identity.command_argv.iter().all(|argument| {
            !argument.is_empty() && argument.len() <= MAX_TRANSFORMER_ARGUMENT_BYTES
        })
        && identity.executable_digest.as_ref().is_none_or(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        });
    if !valid {
        return Err(PrepareError::new(
            "transformer-identity-invalid",
            "transformer identity must contain bounded name, version, canonical argv, and optional SHA-256 command-artifact digest",
        ));
    }
    Ok(())
}

fn transformer_artifact_key(
    manifest_digest: &str,
    template_digest: &str,
    datamodel: &str,
    identity: &TransformerIdentity,
) -> Result<String, PrepareError> {
    let identity_bytes = serde_json::to_vec(identity).map_err(|error| {
        PrepareError::new(
            "transformer-identity-encode",
            format!("cannot encode canonical transformer identity: {error}"),
        )
    })?;
    let mut bytes = Vec::new();
    for field in [
        manifest_digest.as_bytes(),
        template_digest.as_bytes(),
        datamodel.as_bytes(),
        identity_bytes.as_slice(),
    ] {
        let length = u64::try_from(field.len()).map_err(|_| {
            PrepareError::new(
                "transformer-identity-size",
                "transformer artifact identity field is too large",
            )
        })?;
        bytes.extend_from_slice(&length.to_le_bytes());
        bytes.extend_from_slice(field);
    }
    Ok(sha256_bytes(&bytes))
}

fn executable_digest(command: &[String]) -> Option<String> {
    let executable = command.first()?;
    let digest_target = if matches!(executable.as_str(), "java" | "java.exe")
        && command.get(1).is_some_and(|argument| argument == "-jar")
    {
        command.get(2).map(String::as_str)?
    } else {
        executable
    };
    let candidates = if Path::new(digest_target).components().count() > 1 {
        vec![PathBuf::from(digest_target)]
    } else {
        // Ambient-read containment note: the PATH lookup here is the same
        // explicit-resolution surface as `resolve_transformer_command_for`
        // above and is bounded the same way. A missing PATH is fail-closed
        // by outcome: the empty candidate list yields no digest, and the
        // caller records the transformer identity as unresolvable rather
        // than fabricating provenance. The search path is deliberately not
        // configurable at this site so prepared provenance always records
        // the same transformer the command resolution step selected.
        std::env::var_os("PATH").map_or_else(Vec::new, |path| {
            std::env::split_paths(&path)
                .map(|directory| directory.join(digest_target))
                .collect()
        })
    };
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .and_then(|candidate| sha256_bounded_file(&candidate).ok())
}

fn acquire_manifest(options: &PrepareOptions) -> Result<PathBuf, PrepareError> {
    let span = observe(
        Module::ConformancePrepare,
        Operation::PrepareAcquireManifest,
    );
    let result = if is_http_url(&options.manifest) {
        let origin = parse_https_origin(&options.manifest)?;
        let cached = options.cache.join("manifest.xml");
        if options.offline {
            if cached.is_file() {
                Ok(cached)
            } else {
                Err(PrepareError::new(
                    "offline-cache-miss",
                    "offline preparation requires a cached manifest",
                ))
            }
        } else if cached.is_file() && !options.force {
            Ok(cached)
        } else {
            download(&options.manifest, &origin, &cached).map(|()| cached)
        }
    } else {
        let path = PathBuf::from(&options.manifest);
        validate_path_bound(&path)?;
        if path.is_file() {
            let parent = path.parent().ok_or_else(|| {
                PrepareError::new("manifest-missing", "manifest path has no parent directory")
            })?;
            validate_existing_directory(
                if parent.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    parent
                },
                "manifest-missing",
            )?;
            validate_regular_file(&path, "manifest-missing", "manifest")?;
            Ok(path)
        } else if path.is_dir() {
            ensure_directory(&path, "manifest-missing")?;
            let manifest = confined_path(&path, Path::new("manifest.xml"))?;
            validate_regular_file(&manifest, "manifest-missing", "manifest")?;
            Ok(manifest)
        } else {
            Err(PrepareError::new(
                "manifest-missing",
                format!("manifest `{}` does not exist", path.display()),
            ))
        }
    };
    if result.is_err() {
        span.fail(ErrorCategory::InputRead);
    } else {
        span.event(SignalEvent::InputAccepted);
    }
    result
}

#[allow(
    clippy::too_many_lines,
    reason = "one case pipeline owns source, dependency, transform, and output digest recording"
)]
fn prepare_case(
    options: &PrepareOptions,
    transformer: &dyn CorpusTransformer,
    base_uri: &str,
    source_root: &Path,
    artifact_root: &Path,
    case: &IrpCase,
    errata: Option<&LoadedErrata>,
) -> Result<(CaseManifest, bool), PrepareError> {
    let mut reused = false;
    let mut prepared = CaseManifest {
        id: case.id.clone(),
        assertion_id: case.assertion_id.clone(),
        spec_num: case.spec_num.clone(),
        spec_id: case.spec_id.clone(),
        conformance: case.conformance,
        datamodel: options.datamodel.clone(),
        start_uri: case.start_uri.clone(),
        source: None,
        source_digest: None,
        source_artifact: None,
        output_digest: None,
        dependencies: Vec::new(),
        dependency_digests: BTreeMap::new(),
        dependency_source_digests: BTreeMap::new(),
        dependency_source_artifacts: BTreeMap::new(),
        resources: Vec::new(),
        resource_digests: BTreeMap::new(),
        tags: Vec::new(),
        description: case.description.clone(),
        features: Vec::new(),
        manual: case.manual,
        events: Vec::new(),
        expected: if case.rejects.is_empty() {
            ExpectedOutcome::W3cLogOutcome {
                outcome: LogOutcome::Pass,
            }
        } else {
            ExpectedOutcome::DocumentRejection {
                codes: case.rejects.clone(),
            }
        },
        timeout_ms: None,
        max_commands: None,
        preparation_failure: None,
        erratum: None,
        not_applicable: errata.and_then(|errata| errata.not_applicable_record(&case.id)),
    };
    let source = match acquire_corpus_file(options, base_uri, source_root, &case.start_uri) {
        Ok(source) => source,
        Err(error) => {
            prepared.preparation_failure = Some(PreparationFailure {
                code: error.code.into(),
                message: error.message,
            });
            return Ok((prepared, reused));
        }
    };
    if options.offline
        && has_extension(&case.start_uri, "txml")
        && let Err(error) = validate_offline_xml(&source, OfflineXmlKind::Source)
    {
        prepared.preparation_failure = Some(PreparationFailure {
            code: error.code.into(),
            message: error.message,
        });
        return Ok((prepared, reused));
    }
    let source_digest = sha256_bounded_file(&source)?;
    prepared.source_digest = Some(source_digest.clone());
    let case_artifact_root = artifact_root.join(&source_digest);
    let source_artifact = case_artifact_root
        .join("sources")
        .join(safe_relative(&case.start_uri)?);
    ensure_parent(&source_artifact)?;
    if options.force || !source_artifact.is_file() {
        atomic_copy(&source, &source_artifact, "source-copy", &source_digest)?;
    } else if sha256_bounded_file(&source_artifact)? == source_digest {
        reused = true;
    } else {
        atomic_copy(&source, &source_artifact, "source-copy", &source_digest)?;
    }
    let source_relative = source_artifact
        .strip_prefix(&options.output)
        .map_err(|_| PrepareError::new("artifact-path", "source artifact escaped output"))?;
    prepared.source_artifact = Some(source_relative.to_path_buf());
    for dependency_uri in &case.dependencies {
        match acquire_corpus_file(options, base_uri, source_root, dependency_uri) {
            Ok(dependency) => {
                if options.offline
                    && has_extension(dependency_uri, "txml")
                    && let Err(error) = validate_offline_xml(&dependency, OfflineXmlKind::Source)
                {
                    prepared.preparation_failure = Some(PreparationFailure {
                        code: error.code.into(),
                        message: error.message,
                    });
                    return Ok((prepared, reused));
                }
                let dependency_source_digest = sha256_bounded_file(&dependency)?;
                prepared
                    .dependency_source_digests
                    .insert(dependency_uri.clone(), dependency_source_digest.clone());
                let dependency_root = artifact_root.join(&dependency_source_digest);
                let dependency_source_artifact = dependency_root
                    .join("sources")
                    .join(safe_relative(dependency_uri)?);
                ensure_parent(&dependency_source_artifact)?;
                if options.force || !dependency_source_artifact.is_file() {
                    atomic_copy(
                        &dependency,
                        &dependency_source_artifact,
                        "dependency-source-copy",
                        &dependency_source_digest,
                    )?;
                } else if sha256_bounded_file(&dependency_source_artifact)?
                    == dependency_source_digest
                {
                    reused = true;
                } else {
                    atomic_copy(
                        &dependency,
                        &dependency_source_artifact,
                        "dependency-source-copy",
                        &dependency_source_digest,
                    )?;
                }
                let source_relative = dependency_source_artifact
                    .strip_prefix(&options.output)
                    .map_err(|_| {
                        PrepareError::new(
                            "artifact-path",
                            "dependency source artifact escaped output",
                        )
                    })?;
                prepared
                    .dependency_source_artifacts
                    .insert(dependency_uri.clone(), source_relative.to_path_buf());
                let output = if has_extension(dependency_uri, "txml") {
                    let relative = replace_extension(dependency_uri, "scxml")?;
                    let path = dependency_root.join("dependencies").join(&relative);
                    let template = options.templates.join(format!("{}.xsl", options.datamodel));
                    if options.force || !path.is_file() {
                        atomic_transform(transformer, &dependency, &template, &path)?;
                    } else {
                        reused = true;
                    }
                    path
                } else {
                    let path = dependency_root
                        .join("dependencies")
                        .join(safe_relative(dependency_uri)?);
                    if options.force || !path.is_file() {
                        atomic_copy(
                            &dependency,
                            &path,
                            "dependency-copy",
                            &dependency_source_digest,
                        )?;
                    } else {
                        reused = true;
                    }
                    path
                };
                let relative = output.strip_prefix(&options.output).map_err(|_| {
                    PrepareError::new("artifact-path", "dependency artifact escaped output")
                })?;
                let relative = relative.to_path_buf();
                prepared
                    .dependency_digests
                    .insert(slash_path(&relative), sha256_bounded_file(&output)?);
                prepared.dependencies.push(relative);
            }
            Err(error) => {
                prepared.preparation_failure = Some(PreparationFailure {
                    code: error.code.into(),
                    message: error.message,
                });
                return Ok((prepared, reused));
            }
        }
    }
    if !has_extension(&case.start_uri, "txml") {
        return Ok((prepared, reused));
    }
    let relative = replace_extension(&case.start_uri, "scxml")?;
    let output = case_artifact_root.join("cases").join(&relative);
    let template = options.templates.join(format!("{}.xsl", options.datamodel));
    if output.is_file() && !options.force {
        reused = true;
    } else if let Err(error) = atomic_transform(transformer, &source, &template, &output) {
        prepared.preparation_failure = Some(PreparationFailure {
            code: error.code.into(),
            message: error.message,
        });
        return Ok((prepared, reused));
    }
    let output_relative = relative;
    let relative = output
        .strip_prefix(&options.output)
        .map_err(|_| PrepareError::new("artifact-path", "prepared artifact escaped output"))?;
    prepared.source = Some(relative.to_path_buf());
    prepared.output_digest = Some(sha256_bounded_file(&output)?);
    if let Some(erratum) = errata.and_then(|errata| errata.erratum_for(&case.id)) {
        let applied = apply_case_erratum(
            options,
            transformer,
            erratum,
            &source,
            &template,
            &output,
            &case_artifact_root,
            &case.start_uri,
            &output_relative,
        )?;
        let relative_to_output = |path: &Path| {
            path.strip_prefix(&options.output)
                .map(Path::to_path_buf)
                .map_err(|_| PrepareError::new("artifact-path", "erratum artifact escaped output"))
        };
        prepared.erratum = Some(CaseErratum {
            id: erratum.id.clone(),
            stage: erratum.stage,
            corrected_input: relative_to_output(&applied.corrected_input)?,
            corrected_input_digest: applied.corrected_input_digest,
            upstream_output: relative.to_path_buf(),
            upstream_output_digest: applied.upstream_output_digest,
        });
        prepared.source = Some(relative_to_output(&applied.corrected_output)?);
        prepared.output_digest = Some(applied.corrected_output_digest);
    }
    Ok((prepared, reused))
}

fn acquire_corpus_file(
    options: &PrepareOptions,
    base_uri: &str,
    source_root: &Path,
    uri: &str,
) -> Result<PathBuf, PrepareError> {
    let span = observe(
        Module::ConformancePrepare,
        Operation::PrepareAcquireCorpusFile,
    );
    let result = (|| {
        let relative = safe_relative(uri)?;
        if let Some(local) = &options.source_dir {
            let path = confined_path(local, &relative)?;
            validate_regular_file(&path, "source-missing", "local corpus file")?;
            return Ok(path);
        }
        let cached = confined_path(source_root, &relative)?;
        if cached.is_file() && !options.force {
            validate_regular_file(&cached, "source-missing", "cached corpus file")?;
            return Ok(cached);
        }
        if options.offline {
            return Err(PrepareError::new(
                "offline-cache-miss",
                "offline corpus cache is missing a selected artifact",
            ));
        }
        let url = format!("{}/{}", base_uri.trim_end_matches('/'), uri);
        let origin = trusted_online_origin(options)?.ok_or_else(|| {
            PrepareError::new(
                "download-origin-missing",
                "online corpus acquisition requires a caller-authorized origin",
            )
        })?;
        require_same_origin(&url, &origin, "corpus URL")?;
        download(&url, &origin, &cached)?;
        Ok(cached)
    })();
    if result.is_err() {
        span.fail(ErrorCategory::InputRead);
    } else {
        span.event(SignalEvent::InputAccepted);
    }
    result
}

#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CurlMode {
    RedirectProbe,
    Download,
}

#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
fn resolve_public_origin(origin: &HttpsOrigin) -> Result<PinnedOrigin, PrepareError> {
    let mut addresses = if let Ok(address) = origin.host.parse::<IpAddr>() {
        vec![address]
    } else {
        (origin.host.as_str(), origin.port)
            .to_socket_addrs()
            .map_err(|error| {
                PrepareError::new(
                    "download-dns",
                    format!("cannot resolve authorized download host: {error}"),
                )
            })?
            .take(MAX_RESOLVED_ADDRESSES + 1)
            .map(|address| address.ip())
            .collect::<Vec<_>>()
    };
    if addresses.is_empty() || addresses.len() > MAX_RESOLVED_ADDRESSES {
        return Err(PrepareError::new(
            "download-dns-count",
            "authorized download host resolved outside the configured address-count bound",
        ));
    }
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.iter().any(|address| !is_global_address(*address)) {
        return Err(PrepareError::new(
            "download-address-forbidden",
            "authorized download host resolved to a non-global address",
        ));
    }
    Ok(PinnedOrigin {
        origin: origin.clone(),
        addresses,
    })
}

// Explicit network acquisition runs curl through host subprocess plumbing;
// wasm guest builds fail closed because they cannot spawn processes.
#[cfg(any(unix, windows))]
fn curl_arguments(pinned: &PinnedOrigin, mode: CurlMode) -> Result<Vec<String>, PrepareError> {
    if pinned.addresses.is_empty() || pinned.addresses.len() > MAX_RESOLVED_ADDRESSES {
        return Err(PrepareError::new(
            "download-address-count",
            "download address pin set is empty or exceeds the configured bound",
        ));
    }
    let mut arguments = vec![
        "--disable".into(),
        "--fail".into(),
        "--location".into(),
        "--max-redirs".into(),
        "0".into(),
        "--proto".into(),
        "=https".into(),
        "--proto-redir".into(),
        "=https".into(),
        "--noproxy".into(),
        "*".into(),
        "--proxy".into(),
        String::new(),
        "--connect-timeout".into(),
        "10".into(),
        "--max-time".into(),
        "120".into(),
        "--max-filesize".into(),
        MAX_DOWNLOAD_BYTES.to_string(),
        "--silent".into(),
        "--show-error".into(),
    ];
    let address = pinned.addresses.first().copied().ok_or_else(|| {
        PrepareError::new(
            "download-address-count",
            "download address pin set is empty",
        )
    })?;
    arguments.push("--resolve".into());
    arguments.push(curl_resolve_value(&pinned.origin, address));
    #[cfg(any(unix, windows))]
    if mode == CurlMode::RedirectProbe {
        arguments.extend([
            "--output".into(),
            null_output_path().into(),
            "--write-out".into(),
            "%{redirect_url}".into(),
        ]);
    }
    Ok(arguments)
}

#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
fn curl_resolve_value(origin: &HttpsOrigin, address: IpAddr) -> String {
    match address {
        IpAddr::V4(address) => format!("{}:{}:{address}", origin.host, origin.port),
        IpAddr::V6(address) => format!("{}:{}:[{address}]", origin.host, origin.port),
    }
}

// Explicit network acquisition runs curl through host subprocess plumbing;
// wasm guest builds fail closed because they cannot spawn processes.
#[cfg(any(unix, windows))]
fn curl_command(pinned: &PinnedOrigin, mode: CurlMode) -> Result<Command, PrepareError> {
    curl_arguments(pinned, mode).map(|arguments| {
        let mut command = Command::new("curl");
        command.args(arguments);
        for variable in [
            "ALL_PROXY",
            "HTTPS_PROXY",
            "HTTP_PROXY",
            "NO_PROXY",
            "all_proxy",
            "https_proxy",
            "http_proxy",
            "no_proxy",
        ] {
            command.env_remove(variable);
        }
        command
    })
}

// Platform null-sink path for redirect probes; wasip2 guests never reach
// network download paths, so they fail closed without a null device.
#[cfg(unix)]
const fn null_output_path() -> &'static str {
    "/dev/null"
}

#[cfg(windows)]
const fn null_output_path() -> &'static str {
    "NUL"
}

// Explicit network acquisition runs curl through host subprocess plumbing;
// wasm guest builds fail closed because they cannot spawn processes.
#[cfg(any(unix, windows))]
fn download(url: &str, authorized_origin: &HttpsOrigin, output: &Path) -> Result<(), PrepareError> {
    let span = observe(Module::ConformancePrepare, Operation::PrepareDownload);
    let result = (|| {
        let pinned = resolve_public_origin(authorized_origin)?;
        let final_url = resolve_redirects(url, &pinned)?;
        atomic_output_file(output, "download", |temporary| {
            let file = temporary.try_clone().map_err(|error| {
                PrepareError::new(
                    "download-temp",
                    format!("cannot clone exclusive download descriptor: {error}"),
                )
            })?;
            let mut command = curl_command(&pinned, CurlMode::Download)?;
            let status = command
                .arg(&final_url)
                .stdout(Stdio::from(file))
                .status()
                .map_err(|error| {
                    PrepareError::new(
                        "download-spawn",
                        format!("cannot execute curl for explicit prepare: {error}"),
                    )
                })?;
            if !status.success() {
                return Err(PrepareError::new(
                    "download-failed",
                    format!("curl failed with status {status}"),
                ));
            }
            let metadata = temporary.metadata().map_err(|error| {
                PrepareError::new(
                    "download-stat",
                    format!("cannot inspect completed download: {error}"),
                )
            })?;
            if !metadata.is_file() || metadata.len() > MAX_DOWNLOAD_BYTES {
                return Err(PrepareError::new(
                    "download-size",
                    "download is not a bounded regular file",
                ));
            }
            Ok(())
        })
    })();
    if result.is_err() {
        span.fail(ErrorCategory::Platform);
    } else {
        span.event(SignalEvent::ExternalReadCompleted);
    }
    result
}

// Explicit network acquisition runs curl through host subprocess plumbing;
// wasm guest builds fail closed because they cannot spawn processes.
#[cfg(not(any(unix, windows)))]
fn download(
    _url: &str,
    _authorized_origin: &HttpsOrigin,
    _output: &Path,
) -> Result<(), PrepareError> {
    let span = observe(Module::ConformancePrepare, Operation::PrepareDownload);
    span.fail(ErrorCategory::Platform);
    Err(PrepareError::new(
        "download-spawn",
        "explicit network acquisition requires a host that can spawn processes",
    ))
}

// Explicit network acquisition runs curl through host subprocess plumbing;
// wasm guest builds fail closed because they cannot spawn processes.
#[cfg(any(unix, windows))]
fn resolve_redirects(url: &str, pinned: &PinnedOrigin) -> Result<String, PrepareError> {
    let mut current = url.to_owned();
    for _ in 0..=MAX_REDIRECTS {
        require_same_origin(&current, &pinned.origin, "download URL")?;
        let mut command = curl_command(pinned, CurlMode::RedirectProbe)?;
        let output = command.arg(&current).output().map_err(|error| {
            PrepareError::new(
                "download-spawn",
                format!("cannot execute curl for redirect validation: {error}"),
            )
        })?;
        if output.stdout.len() > MAX_RESPONSE_HEADERS {
            return Err(PrepareError::new(
                "download-redirect",
                "redirect metadata exceeds the configured bound",
            ));
        }
        let redirect = std::str::from_utf8(&output.stdout)
            .map_err(|_| PrepareError::new("download-redirect", "redirect URL is not valid UTF-8"))?
            .trim();
        if redirect.is_empty() {
            if output.status.success() {
                return Ok(current);
            }
            return Err(PrepareError::new(
                "download-failed",
                format!("curl failed with status {}", output.status),
            ));
        }
        require_same_origin(redirect, &pinned.origin, "redirect URL")?;
        redirect.clone_into(&mut current);
    }
    Err(PrepareError::new(
        "download-redirect-limit",
        "download exceeded the redirect limit",
    ))
}

fn read_bounded_file(
    path: &Path,
    maximum: usize,
    read_code: &'static str,
    size_code: &'static str,
) -> Result<Vec<u8>, PrepareError> {
    let maximum_u64 = u64::try_from(maximum)
        .map_err(|_| PrepareError::new(size_code, "configured byte bound is not representable"))?;
    let file = open_regular_file_nofollow(path, maximum_u64, read_code, size_code)?;
    let metadata = file.metadata().map_err(|error| {
        PrepareError::new(read_code, format!("cannot inspect bounded input: {error}"))
    })?;
    if metadata.len() > maximum_u64 {
        return Err(PrepareError::new(
            size_code,
            "input exceeds the configured byte bound",
        ));
    }
    let capacity = usize::try_from(metadata.len())
        .map_err(|_| PrepareError::new(size_code, "input size is not representable in memory"))?;
    let mut bytes = Vec::with_capacity(capacity);
    Read::take(file, maximum_u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            PrepareError::new(read_code, format!("cannot read bounded input: {error}"))
        })?;
    if bytes.len() > maximum {
        return Err(PrepareError::new(
            size_code,
            "input exceeds the configured byte bound",
        ));
    }
    Ok(bytes)
}

fn sha256_bounded_file(path: &Path) -> Result<String, PrepareError> {
    let mut file =
        open_regular_file_nofollow(path, MAX_DOWNLOAD_BYTES, "source-read", "source-size")?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 16_384];
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            PrepareError::new("source-read", format!("cannot hash bounded input: {error}"))
        })?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(u64::try_from(read).map_err(|_| {
                PrepareError::new("source-size", "hash read size is not representable")
            })?)
            .ok_or_else(|| PrepareError::new("source-size", "hash byte count overflowed"))?;
        if total > MAX_DOWNLOAD_BYTES {
            return Err(PrepareError::new(
                "source-size",
                "input exceeds the configured byte bound while hashing",
            ));
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}

/// Whether a descriptor-relative directory walk may create missing
/// components. A walk that creates also requires the final directory to be
/// owned by this process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DirectoryOpen {
    /// Create missing components (mode 0700) and require ownership.
    Create,
    /// Open existing components only.
    Existing,
}

#[cfg(unix)]
fn open_directory_nofollow(
    path: &Path,
    open: DirectoryOpen,
    code: &'static str,
) -> Result<File, PrepareError> {
    validate_path_bound(path)?;
    #[cfg(target_os = "macos")]
    let path = normalize_macos_temporary_alias(path, code)?;
    let mut directory = File::from(
        openat(
            CWD,
            if path.is_absolute() {
                Path::new("/")
            } else {
                Path::new(".")
            },
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| PrepareError::new(code, format!("cannot open directory root: {error}")))?,
    );
    for component in path.components() {
        let Component::Normal(name) = component else {
            if matches!(component, Component::ParentDir | Component::Prefix(_)) {
                return Err(PrepareError::new(
                    "path-unsafe",
                    "filesystem paths must not contain parent traversal or prefixes",
                ));
            }
            continue;
        };
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let next = match openat(&directory, name, flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(Errno::NOENT) if open == DirectoryOpen::Create => {
                match mkdirat(&directory, name, Mode::RWXU) {
                    Ok(()) | Err(Errno::EXIST) => {}
                    Err(error) => {
                        return Err(PrepareError::new(
                            code,
                            format!("cannot create directory component: {error}"),
                        ));
                    }
                }
                openat(&directory, name, flags, Mode::empty()).map_err(|error| {
                    PrepareError::new(
                        code,
                        format!("cannot open created directory component safely: {error}"),
                    )
                })?
            }
            Err(error) => {
                return Err(PrepareError::new(
                    code,
                    format!("cannot open symlink-free directory component: {error}"),
                ));
            }
        };
        directory = File::from(next);
    }
    validate_directory_ownership(&directory, open, code)?;
    Ok(directory)
}

#[cfg(target_os = "macos")]
fn normalize_macos_temporary_alias(
    path: &Path,
    code: &'static str,
) -> Result<PathBuf, PrepareError> {
    use std::os::unix::fs::MetadataExt;

    let Ok(suffix) = path.strip_prefix("/tmp") else {
        return Ok(path.to_path_buf());
    };
    let alias = Path::new("/tmp");
    let metadata = fs::symlink_metadata(alias).map_err(|error| {
        PrepareError::new(
            code,
            format!("cannot inspect macOS temporary directory alias: {error}"),
        )
    })?;
    if !metadata.file_type().is_symlink() || metadata.uid() != 0 {
        return Err(PrepareError::new(
            "output-path-symlink",
            "macOS temporary directory alias must be a root-owned symbolic link",
        ));
    }
    let canonical = alias.canonicalize().map_err(|error| {
        PrepareError::new(
            code,
            format!("cannot resolve macOS temporary directory alias: {error}"),
        )
    })?;
    if !canonical.is_absolute() || !canonical.is_dir() {
        return Err(PrepareError::new(
            "output-path-symlink",
            "macOS temporary directory alias must resolve to an absolute directory",
        ));
    }
    let normalized = canonical.join(suffix);
    validate_path_bound(&normalized)?;
    Ok(normalized)
}

#[cfg(unix)]
fn validate_directory_ownership(
    directory: &File,
    open: DirectoryOpen,
    code: &'static str,
) -> Result<(), PrepareError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = directory.metadata().map_err(|error| {
        PrepareError::new(code, format!("cannot inspect opened directory: {error}"))
    })?;
    let owned_by_process = metadata.uid() == rustix::process::geteuid().as_raw();
    let writable_by_others = metadata.mode() & 0o022 != 0;
    if !owned_by_process && (open == DirectoryOpen::Create || writable_by_others) {
        return Err(PrepareError::new(
            "output-root-ownership",
            "filesystem path traverses a writable directory not owned by this process",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
fn open_directory_nofollow(
    _path: &Path,
    _open: DirectoryOpen,
    _code: &'static str,
) -> Result<File, PrepareError> {
    Err(PrepareError::new(
        "filesystem-security-unsupported",
        "secure descriptor-relative filesystem preparation is unsupported on this platform",
    ))
}

#[cfg(unix)]
fn open_parent_directory_nofollow(
    path: &Path,
    open: DirectoryOpen,
    code: &'static str,
) -> Result<(File, OsString), PrepareError> {
    validate_path_bound(path)?;
    let name = path
        .file_name()
        .ok_or_else(|| PrepareError::new(code, "filesystem path must name a final entry"))?;
    if name == OsStr::new(".") || name == OsStr::new("..") {
        return Err(PrepareError::new(code, "filesystem entry name is unsafe"));
    }
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok((
        open_directory_nofollow(parent, open, code)?,
        name.to_owned(),
    ))
}

#[cfg(not(unix))]
#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
fn open_parent_directory_nofollow(
    _path: &Path,
    _open: DirectoryOpen,
    _code: &'static str,
) -> Result<(File, OsString), PrepareError> {
    Err(PrepareError::new(
        "filesystem-security-unsupported",
        "secure descriptor-relative filesystem preparation is unsupported on this platform",
    ))
}

#[cfg(unix)]
fn open_regular_file_nofollow(
    path: &Path,
    maximum: u64,
    read_code: &'static str,
    size_code: &'static str,
) -> Result<File, PrepareError> {
    let (parent, name) = open_parent_directory_nofollow(path, DirectoryOpen::Existing, read_code)?;
    let file = File::from(
        openat(
            &parent,
            &name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| {
            PrepareError::new(
                read_code,
                format!("cannot open bounded regular file: {error}"),
            )
        })?,
    );
    let metadata = file.metadata().map_err(|error| {
        PrepareError::new(
            read_code,
            format!("cannot inspect bounded regular file: {error}"),
        )
    })?;
    if !metadata.is_file() {
        return Err(PrepareError::new(read_code, "input must be a regular file"));
    }
    if metadata.len() > maximum {
        return Err(PrepareError::new(
            size_code,
            "input exceeds the configured byte bound",
        ));
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_regular_file_nofollow(
    _path: &Path,
    _maximum: u64,
    _read_code: &'static str,
    _size_code: &'static str,
) -> Result<File, PrepareError> {
    Err(PrepareError::new(
        "filesystem-security-unsupported",
        "secure no-follow file reads are unsupported on this platform",
    ))
}

#[cfg(not(unix))]
fn ensure_directory(path: &Path, code: &'static str) -> Result<(), PrepareError> {
    validate_path_bound(path)?;
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                return Err(PrepareError::new(
                    "output-path-unsafe",
                    "directory paths must not contain parent traversal",
                ));
            }
            Component::CurDir => continue,
            _ => current.push(component.as_os_str()),
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(PrepareError::new(
                    "output-path-symlink",
                    format!(
                        "directory path `{}` traverses a symbolic link",
                        current.display()
                    ),
                ));
            }
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(PrepareError::new(
                    code,
                    format!(
                        "directory path component `{}` is not a directory",
                        current.display()
                    ),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::create_dir(&current) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(PrepareError::new(
                            code,
                            format!("cannot create `{}`: {error}", current.display()),
                        ));
                    }
                }
                let metadata = fs::symlink_metadata(&current).map_err(|error| {
                    PrepareError::new(
                        code,
                        format!(
                            "cannot inspect `{}` after creation: {error}",
                            current.display()
                        ),
                    )
                })?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(PrepareError::new(
                        "output-path-symlink",
                        format!(
                            "created directory path `{}` was replaced by an unsafe entry",
                            current.display()
                        ),
                    ));
                }
            }
            Err(error) => {
                return Err(PrepareError::new(
                    code,
                    format!("cannot inspect `{}`: {error}", current.display()),
                ));
            }
        }
    }
    if current.as_os_str().is_empty() {
        return Err(PrepareError::new(
            "output-path-unsafe",
            "directory path must not be empty",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn ensure_directory(path: &Path, code: &'static str) -> Result<(), PrepareError> {
    open_directory_nofollow(path, DirectoryOpen::Create, code).map(drop)
}

#[cfg(not(unix))]
fn validate_existing_directory(path: &Path, code: &'static str) -> Result<(), PrepareError> {
    validate_path_bound(path)?;
    let mut current = PathBuf::new();
    for component in path.components() {
        if matches!(component, Component::ParentDir) {
            return Err(PrepareError::new(
                "path-unsafe",
                "directory paths must not contain parent traversal",
            ));
        }
        if matches!(component, Component::CurDir) {
            continue;
        }
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            PrepareError::new(
                code,
                format!("cannot inspect directory `{}`: {error}", current.display()),
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(PrepareError::new(
                "path-symlink",
                format!(
                    "directory path `{}` is not a symlink-free directory",
                    current.display()
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn validate_existing_directory(path: &Path, code: &'static str) -> Result<(), PrepareError> {
    open_directory_nofollow(path, DirectoryOpen::Existing, code).map(drop)
}

fn validate_regular_file(
    path: &Path,
    code: &'static str,
    kind: &'static str,
) -> Result<(), PrepareError> {
    let _file = open_regular_file_nofollow(path, MAX_DOWNLOAD_BYTES, code, "source-size").map_err(
        |error| {
            if error.code == code || error.code == "source-size" {
                error
            } else {
                PrepareError::new(code, format!("cannot validate {kind}: {error}"))
            }
        },
    )?;
    Ok(())
}

fn atomic_copy(
    source: &Path,
    output: &Path,
    label: &'static str,
    expected_digest: &str,
) -> Result<(), PrepareError> {
    let mut reader =
        open_regular_file_nofollow(source, MAX_DOWNLOAD_BYTES, "source-read", "source-size")?;
    atomic_output_file(output, label, |writer| {
        let mut digest = Sha256::new();
        let mut copied = 0_u64;
        let mut buffer = [0_u8; 16_384];
        loop {
            let read = reader.read(&mut buffer).map_err(|error| {
                PrepareError::new(
                    "source-copy",
                    format!("cannot read bounded source while copying: {error}"),
                )
            })?;
            if read == 0 {
                break;
            }
            copied = copied
                .checked_add(u64::try_from(read).map_err(|_| {
                    PrepareError::new("source-size", "copy read size is not representable")
                })?)
                .ok_or_else(|| PrepareError::new("source-size", "copy byte count overflowed"))?;
            if copied > MAX_DOWNLOAD_BYTES {
                return Err(PrepareError::new(
                    "source-size",
                    "source exceeds the configured byte bound",
                ));
            }
            writer.write_all(&buffer[..read]).map_err(|error| {
                PrepareError::new(
                    "source-copy",
                    format!("cannot write bounded source copy: {error}"),
                )
            })?;
            digest.update(&buffer[..read]);
        }
        if hex::encode(digest.finalize()) != expected_digest {
            return Err(PrepareError::new(
                "source-changed",
                "source bytes changed after their artifact identity was established",
            ));
        }
        writer.flush().map_err(|error| {
            PrepareError::new(
                "source-copy",
                format!("cannot flush copied source: {error}"),
            )
        })
    })
}

fn atomic_transform(
    transformer: &dyn CorpusTransformer,
    source: &Path,
    template: &Path,
    output: &Path,
) -> Result<(), PrepareError> {
    let source_file =
        open_regular_file_nofollow(source, MAX_DOWNLOAD_BYTES, "source-read", "source-size")?;
    let template_file = open_regular_file_nofollow(
        template,
        MAX_DOWNLOAD_BYTES,
        "template-read",
        "template-size",
    )?;
    let source_path = inheritable_descriptor_path(&source_file)?;
    let template_path = inheritable_descriptor_path(&template_file)?;
    atomic_output(output, "transform", |temporary| {
        transformer.transform(&source_path, &template_path, temporary)
    })
}

#[cfg(unix)]
fn inheritable_descriptor_path(file: &File) -> Result<PathBuf, PrepareError> {
    fcntl_setfd(file, FdFlags::empty()).map_err(|error| {
        PrepareError::new(
            "filesystem-descriptor-inherit",
            format!("cannot make a trusted input descriptor inheritable: {error}"),
        )
    })?;
    descriptor_file_path(file)
}

#[cfg(not(unix))]
fn inheritable_descriptor_path(_file: &File) -> Result<PathBuf, PrepareError> {
    Err(PrepareError::new(
        "filesystem-security-unsupported",
        "descriptor-owned transformer inputs are unsupported on this platform",
    ))
}

fn atomic_output<F>(output: &Path, label: &'static str, write: F) -> Result<(), PrepareError>
where
    F: FnOnce(&Path) -> Result<(), PrepareError>,
{
    let temporary = create_temporary_output(output, label)?;
    if let Err(error) = write(&temporary.path) {
        remove_temporary(&temporary)?;
        return Err(error);
    }
    commit_temporary_output(&temporary)
}

fn atomic_output_file<F>(output: &Path, label: &'static str, write: F) -> Result<(), PrepareError>
where
    F: FnOnce(&mut File) -> Result<(), PrepareError>,
{
    let mut temporary = create_temporary_output(output, label)?;
    if let Err(error) = write(&mut temporary.file) {
        remove_temporary(&temporary)?;
        return Err(error);
    }
    commit_temporary_output(&temporary)
}

#[cfg(unix)]
struct TemporaryOutput {
    parent: File,
    output_name: OsString,
    temporary_name: OsString,
    path: PathBuf,
    file: File,
}

#[cfg(not(unix))]
struct TemporaryOutput {
    path: PathBuf,
    file: File,
}

#[cfg(unix)]
fn create_temporary_output(
    output: &Path,
    label: &'static str,
) -> Result<TemporaryOutput, PrepareError> {
    let (parent, output_name) =
        open_parent_directory_nofollow(output, DirectoryOpen::Create, "output-parent")?;
    reject_output_entry(&parent, &output_name)?;
    let parent_path = output
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf();
    for _ in 0..MAX_TEMP_ATTEMPTS {
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary_name = OsString::from(format!(
            ".{}.{}.{}.{}.tmp",
            output_name.to_string_lossy(),
            label,
            std::process::id(),
            sequence
        ));
        let entry_path = parent_path.join(&temporary_name);
        validate_path_bound(&entry_path)?;
        match openat(
            &parent,
            &temporary_name,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        ) {
            Ok(file) => {
                let file = File::from(file);
                let path = descriptor_file_path(&file)?;
                return Ok(TemporaryOutput {
                    parent,
                    output_name,
                    temporary_name,
                    path,
                    file,
                });
            }
            Err(Errno::EXIST) => {}
            Err(error) => {
                return Err(PrepareError::new(
                    "output-temp",
                    format!("cannot create exclusive temporary output: {error}"),
                ));
            }
        }
    }
    Err(PrepareError::new(
        "output-temp-exhausted",
        "cannot allocate an exclusive temporary output within the attempt bound",
    ))
}

#[cfg(target_os = "linux")]
fn descriptor_file_path(file: &File) -> Result<PathBuf, PrepareError> {
    let path = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
    validate_path_bound(&path)?;
    Ok(path)
}

#[cfg(target_os = "macos")]
fn descriptor_file_path(file: &File) -> Result<PathBuf, PrepareError> {
    let path = PathBuf::from(format!("/dev/fd/{}", file.as_raw_fd()));
    validate_path_bound(&path)?;
    Ok(path)
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn descriptor_file_path(_file: &File) -> Result<PathBuf, PrepareError> {
    Err(PrepareError::new(
        "filesystem-security-unsupported",
        "descriptor-owned output paths are unsupported on this Unix platform",
    ))
}

#[cfg(not(unix))]
fn create_temporary_output(
    _output: &Path,
    _label: &'static str,
) -> Result<TemporaryOutput, PrepareError> {
    Err(PrepareError::new(
        "filesystem-security-unsupported",
        "secure atomic output is unsupported on this platform",
    ))
}

#[cfg(unix)]
fn commit_temporary_output(temporary: &TemporaryOutput) -> Result<(), PrepareError> {
    let result = (|| {
        let metadata = temporary.file.metadata().map_err(|error| {
            PrepareError::new(
                "output-temp-stat",
                format!("cannot inspect completed temporary output: {error}"),
            )
        })?;
        if !metadata.is_file() || metadata.len() > MAX_DOWNLOAD_BYTES {
            return Err(PrepareError::new(
                "output-size",
                "completed output is not a bounded regular file",
            ));
        }
        let reopened = openat(
            &temporary.parent,
            &temporary.temporary_name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(|error| {
            PrepareError::new(
                "output-temp-stat",
                format!("cannot reopen completed temporary output safely: {error}"),
            )
        })?;
        let reopened_metadata = reopened.metadata().map_err(|error| {
            PrepareError::new(
                "output-temp-stat",
                format!("cannot inspect reopened temporary output: {error}"),
            )
        })?;
        if !same_file_identity(&metadata, &reopened_metadata) {
            return Err(PrepareError::new(
                "output-temp-changed",
                "temporary output identity changed before commit",
            ));
        }
        temporary.file.sync_all().map_err(|error| {
            PrepareError::new(
                "output-sync",
                format!("cannot synchronize temporary output: {error}"),
            )
        })?;
        reject_output_entry(&temporary.parent, &temporary.output_name)?;
        renameat(
            &temporary.parent,
            &temporary.temporary_name,
            &temporary.parent,
            &temporary.output_name,
        )
        .map_err(|error| {
            PrepareError::new(
                "output-commit",
                format!("cannot atomically commit output: {error}"),
            )
        })?;
        verify_committed_output(temporary, &metadata)?;
        temporary.parent.sync_all().map_err(|error| {
            PrepareError::new(
                "output-parent-sync",
                format!("cannot synchronize output directory: {error}"),
            )
        })
    })();
    if let Err(error) = result {
        remove_temporary(temporary)?;
        return Err(error);
    }
    Ok(())
}

#[cfg(not(unix))]
fn commit_temporary_output(_temporary: &TemporaryOutput) -> Result<(), PrepareError> {
    Err(PrepareError::new(
        "filesystem-security-unsupported",
        "secure atomic output is unsupported on this platform",
    ))
}

#[cfg(unix)]
fn verify_committed_output(
    temporary: &TemporaryOutput,
    expected: &fs::Metadata,
) -> Result<(), PrepareError> {
    let committed = openat(
        &temporary.parent,
        &temporary.output_name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|error| {
        PrepareError::new(
            "output-commit-identity",
            format!("cannot reopen committed output safely: {error}"),
        )
    })?;
    let actual = committed.metadata().map_err(|error| {
        PrepareError::new(
            "output-commit-identity",
            format!("cannot inspect committed output safely: {error}"),
        )
    })?;
    if same_file_identity(expected, &actual) {
        return Ok(());
    }
    unlinkat(&temporary.parent, &temporary.output_name, AtFlags::empty()).map_err(|error| {
        PrepareError::new(
            "output-commit-identity",
            format!("cannot remove identity-mismatched committed output: {error}"),
        )
    })?;
    Err(PrepareError::new(
        "output-commit-identity",
        "committed output identity changed during atomic publication",
    ))
}

#[cfg(unix)]
fn remove_temporary(temporary: &TemporaryOutput) -> Result<(), PrepareError> {
    match unlinkat(
        &temporary.parent,
        &temporary.temporary_name,
        AtFlags::empty(),
    ) {
        Ok(()) | Err(Errno::NOENT) => Ok(()),
        Err(error) => Err(PrepareError::new(
            "output-temp-cleanup",
            format!("cannot remove temporary output: {error}"),
        )),
    }
}

#[cfg(not(unix))]
fn remove_temporary(_temporary: &TemporaryOutput) -> Result<(), PrepareError> {
    Err(PrepareError::new(
        "filesystem-security-unsupported",
        "secure temporary cleanup is unsupported on this platform",
    ))
}

#[cfg(unix)]
fn reject_output_entry(parent: &File, name: &OsStr) -> Result<(), PrepareError> {
    match statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(metadata) if FileType::from_raw_mode(metadata.st_mode).is_symlink() => Err(
            PrepareError::new("output-path-symlink", "output path is a symbolic link"),
        ),
        Ok(metadata) if FileType::from_raw_mode(metadata.st_mode).is_file() => Ok(()),
        Ok(_) => Err(PrepareError::new(
            "output-path-type",
            "output path is not a regular file",
        )),
        Err(Errno::NOENT) => Ok(()),
        Err(error) => Err(PrepareError::new(
            "output-path-stat",
            format!("cannot inspect output path safely: {error}"),
        )),
    }
}

#[cfg(unix)]
fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn validate_path_bound(path: &Path) -> Result<(), PrepareError> {
    let component_count = path.components().count();
    if path.as_os_str().is_empty()
        || path.as_os_str().as_encoded_bytes().len() > MAX_PATH_BYTES
        || component_count > MAX_PATH_COMPONENTS
    {
        Err(PrepareError::new(
            "path-bound",
            "filesystem path exceeds the configured byte or component bound",
        ))
    } else {
        Ok(())
    }
}

fn safe_relative(value: &str) -> Result<PathBuf, PrepareError> {
    let path = Path::new(value);
    if value.len() > MAX_PATH_BYTES
        || path.components().count() > MAX_PATH_COMPONENTS
        || value.is_empty()
        || value.contains(['?', '#', '\\'])
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(PrepareError::new(
            "corpus-path-unsafe",
            "corpus URI is not a bounded safe relative path",
        ));
    }
    Ok(path.to_path_buf())
}

fn confined_path(root: &Path, relative: &Path) -> Result<PathBuf, PrepareError> {
    let mut candidate = root.to_path_buf();
    for component in relative.components() {
        candidate.push(component.as_os_str());
        reject_symlink_path(&candidate)?;
    }
    Ok(candidate)
}

fn reject_symlink_path(path: &Path) -> Result<(), PrepareError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(PrepareError::new(
            "corpus-path-symlink",
            format!("corpus path `{}` traverses a symbolic link", path.display()),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(PrepareError::new(
            "corpus-path-stat",
            format!("cannot inspect corpus path `{}`: {error}", path.display()),
        )),
    }
}

fn trusted_online_origin(options: &PrepareOptions) -> Result<Option<HttpsOrigin>, PrepareError> {
    options
        .base_uri
        .as_deref()
        .filter(|value| is_http_url(value))
        .or_else(|| is_http_url(&options.manifest).then_some(options.manifest.as_str()))
        .map(parse_https_origin)
        .transpose()
}

fn persisted_manifest_source(value: &str) -> Result<String, PrepareError> {
    if is_http_url(value) {
        let origin = parse_https_origin(value)?;
        let authority_end = value["https://".len()..]
            .find('/')
            .map_or(value.len(), |index| "https://".len() + index);
        let path = &value[authority_end..];
        let host = if origin.host.contains(':') {
            format!("[{}]", origin.host)
        } else {
            origin.host
        };
        let port = if origin.port == 443 {
            String::new()
        } else {
            format!(":{}", origin.port)
        };
        Ok(format!("https://{host}{port}{path}"))
    } else {
        Ok(value.to_owned())
    }
}

fn parse_https_origin(value: &str) -> Result<HttpsOrigin, PrepareError> {
    if value.len() > MAX_URL_BYTES {
        return Err(PrepareError::new(
            "download-url-size",
            "online corpus URL exceeds the configured byte bound",
        ));
    }
    if value.contains(['?', '#']) {
        return Err(PrepareError::new(
            "download-url-secret",
            "online corpus URLs must not contain query strings or fragments",
        ));
    }
    let rest = value.strip_prefix("https://").ok_or_else(|| {
        PrepareError::new(
            "download-protocol",
            "online corpus acquisition requires an HTTPS URL",
        )
    })?;
    if rest.is_empty() || rest.contains('\\') {
        return Err(PrepareError::new(
            "download-url",
            "online corpus URL has an invalid authority",
        ));
    }
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .ok_or_else(|| PrepareError::new("download-url", "online corpus URL has no host"))?;
    if authority.is_empty() || authority.contains('@') {
        return Err(PrepareError::new(
            "download-userinfo",
            "online corpus URLs must not contain user information",
        ));
    }
    let (host, port) = if authority.starts_with('[') {
        parse_ipv6_authority(authority)?
    } else {
        parse_named_or_ipv4_authority(authority)?
    };
    Ok(HttpsOrigin { host, port })
}

fn parse_ipv6_authority(authority: &str) -> Result<(String, u16), PrepareError> {
    let bracketed = authority.strip_prefix('[').ok_or_else(|| {
        PrepareError::new("download-host", "IPv6 URL host must start with a bracket")
    })?;
    let closing = bracketed.find(']').ok_or_else(|| {
        PrepareError::new(
            "download-host",
            "IPv6 URL host is missing a closing bracket",
        )
    })?;
    let host = &bracketed[..closing];
    let suffix = &bracketed[closing + 1..];
    let port = parse_url_port(suffix)?;
    let address = host.parse::<Ipv6Addr>().map_err(|_| {
        PrepareError::new(
            "download-host",
            "online corpus URL has an invalid IPv6 host",
        )
    })?;
    if is_forbidden_ipv6(address) {
        return Err(PrepareError::new(
            "download-host-forbidden",
            "online corpus URL uses a non-public literal host",
        ));
    }
    Ok((address.to_string(), port))
}

fn parse_named_or_ipv4_authority(authority: &str) -> Result<(String, u16), PrepareError> {
    let (host, suffix) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => (host, format!(":{port}")),
        Some(_) => {
            return Err(PrepareError::new(
                "download-host",
                "IPv6 URL hosts must use brackets",
            ));
        }
        None => (authority, String::new()),
    };
    let port = parse_url_port(&suffix)?;
    if host.ends_with('.') {
        return Err(PrepareError::new(
            "download-host",
            "online corpus URL hosts must use their canonical form",
        ));
    }
    let normalized = host.to_ascii_lowercase();
    if normalized.is_empty() || normalized == "localhost" || normalized.ends_with(".localhost") {
        return Err(PrepareError::new(
            "download-host-forbidden",
            "online corpus URL uses a non-public host",
        ));
    }
    if let Ok(address) = normalized.parse::<Ipv4Addr>()
        && is_forbidden_ipv4(address)
    {
        return Err(PrepareError::new(
            "download-host-forbidden",
            "online corpus URL uses a non-public literal host",
        ));
    }
    if normalized
        .bytes()
        .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')))
    {
        return Err(PrepareError::new(
            "download-host",
            "online corpus URL has an invalid host",
        ));
    }
    Ok((normalized, port))
}

fn parse_url_port(suffix: &str) -> Result<u16, PrepareError> {
    if suffix.is_empty() {
        return Ok(443);
    }
    let value = suffix.strip_prefix(':').ok_or_else(|| {
        PrepareError::new("download-port", "online corpus URL has an invalid port")
    })?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(PrepareError::new(
            "download-port",
            "online corpus URL has an invalid port",
        ));
    }
    let port = value.parse::<u16>().map_err(|_| {
        PrepareError::new("download-port", "online corpus URL port is out of range")
    })?;
    if port == 0 {
        return Err(PrepareError::new(
            "download-port",
            "online corpus URL port must be non-zero",
        ));
    }
    Ok(port)
}

fn is_forbidden_ipv4(address: Ipv4Addr) -> bool {
    !is_global_ipv4(address)
}

fn is_forbidden_ipv6(address: Ipv6Addr) -> bool {
    !is_global_ipv6(address)
}

#[cfg_attr(not(unix), allow(dead_code))]
fn is_global_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_global_ipv4(address),
        IpAddr::V6(address) => is_global_ipv6(address),
    }
}

fn is_global_ipv4(address: Ipv4Addr) -> bool {
    let [first, second, third, fourth] = address.octets();
    if first == 192 && second == 0 && third == 0 {
        return matches!(fourth, 9 | 10);
    }
    !(first == 0
        || first == 10
        || first == 127
        || first >= 224
        || (first == 100 && (64..=127).contains(&second))
        || (first == 169 && second == 254)
        || (first == 172 && (16..=31).contains(&second))
        || (first == 192 && second == 0 && third == 2)
        || (first == 192 && second == 88 && third == 99)
        || (first == 192 && second == 168)
        || (first == 198 && matches!(second, 18 | 19))
        || (first == 198 && second == 51 && third == 100)
        || (first == 203 && second == 0 && third == 113))
}

fn is_global_ipv6(address: Ipv6Addr) -> bool {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_global_ipv4(mapped);
    }
    let segments = address.segments();
    let well_known_nat64 = segments[0] == 0x0064
        && segments[1] == 0xff9b
        && segments[2..6].iter().all(|segment| *segment == 0);
    let in_global_unicast = (segments[0] & 0xe000) == 0x2000;
    let documentation = (segments[0] == 0x2001 && segments[1] == 0x0db8)
        || (segments[0] == 0x3fff && (segments[1] & 0xf000) == 0);
    let globally_routable_2001_special = segments == [0x2001, 1, 0, 0, 0, 0, 0, 1]
        || segments == [0x2001, 1, 0, 0, 0, 0, 0, 2]
        || segments[1] == 0x0003
        || (segments[1] == 0x0004 && segments[2] == 0x0112)
        || (0x0020..=0x003f).contains(&segments[1]);
    let special_2001 =
        segments[0] == 0x2001 && segments[1] < 0x0200 && !globally_routable_2001_special;
    let local_or_multicast = (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] & 0xffc0) == 0xfec0
        || (segments[0] & 0xff00) == 0xff00
        || segments[0] == 0x5f00;
    well_known_nat64
        || (in_global_unicast && !documentation && !special_2001 && !local_or_multicast)
}

fn require_same_origin(
    value: &str,
    authorized: &HttpsOrigin,
    kind: &'static str,
) -> Result<(), PrepareError> {
    let actual = parse_https_origin(value)?;
    if actual != *authorized {
        return Err(PrepareError::new(
            "download-origin",
            format!("{kind} does not match the caller-authorized origin"),
        ));
    }
    Ok(())
}

/// A repository namespace name (`namespaces/<name>`): `[a-z][a-z0-9-]*`, at
/// most 64 bytes.
fn is_namespace_name(name: &str) -> bool {
    name.len() <= 64
        && name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn replace_extension(value: &str, extension: &str) -> Result<PathBuf, PrepareError> {
    let mut path = safe_relative(value)?;
    path.set_extension(extension);
    Ok(path)
}

fn ensure_parent(path: &Path) -> Result<(), PrepareError> {
    let parent = path.parent().ok_or_else(|| {
        PrepareError::new(
            "output-parent",
            format!("path `{}` has no parent", path.display()),
        )
    })?;
    ensure_directory(parent, "output-create")
}

fn has_extension(value: &str, extension: &str) -> bool {
    Path::new(value)
        .extension()
        .is_some_and(|actual| actual.eq_ignore_ascii_case(extension))
}

fn is_http_url(value: &str) -> bool {
    value.starts_with("https://") || value.starts_with("http://")
}

fn http_parent(value: &str) -> Option<&str> {
    if !is_http_url(value) || value.contains(['?', '#']) {
        return None;
    }
    let (parent, file_name) = value.rsplit_once('/')?;
    if parent.is_empty() || file_name.is_empty() {
        None
    } else {
        Some(parent)
    }
}

fn slash_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
// Preparation tests exercise host subprocess behavior (Saxon, curl, sandbox
// execution) through unix-only process plumbing; they are not meaningful on
// wasm guests, which cannot spawn processes.
#[cfg(unix)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn fixed_saxon_launcher_resolves_canonical_runtime_and_jar_identity() {
        let root = temporary("fixed-saxon-launcher");
        let java_home = root.join("jdk");
        let java = java_home.join("bin/java");
        let jar = root.join("saxon-he.jar");
        let launcher = root.join("saxon");
        fs::create_dir_all(java.parent().expect("Java fixture must have a parent"))
            .expect("Java fixture directory must be creatable");
        fs::write(&java, b"fixed-java-runtime").expect("Java fixture must be writable");
        fs::write(&jar, b"fixed-saxon-jar").expect("Saxon JAR fixture must be writable");
        fs::write(
            &launcher,
            format!(
                "#!/bin/bash\nexport JAVA_HOME=\"${{JAVA_HOME:-{}}}\"\nexec \"${{JAVA_HOME}}/bin/java\" -jar \"{}\" \"$@\"\n",
                java_home.display(),
                jar.display()
            ),
        )
        .expect("fixed launcher fixture must be writable");

        let resolved = resolve_saxon_launcher(&launcher)
            .expect("fixed launcher must resolve without executing its wrapper");
        assert_eq!(
            resolved,
            vec![
                java.canonicalize()
                    .expect("Java fixture must canonicalize")
                    .to_string_lossy()
                    .into_owned(),
                "-jar".into(),
                jar.canonicalize()
                    .expect("JAR fixture must canonicalize")
                    .to_string_lossy()
                    .into_owned(),
            ]
        );
        let isolation = OfflineIsolation {
            launcher: launcher
                .canonicalize()
                .expect("launcher fixture must canonicalize"),
            mechanism: "synthetic-default-deny",
            identity_digest: String::new(),
        };
        let sandboxed = sandboxed_command_prefix(&resolved, &isolation, "fixed-profile".into());
        let initial_identity = command_digest(&sandboxed, Some(&isolation))
            .expect("resolved fixed launcher identity must be hashable")
            .expect("resolved fixed launcher must have an artifact identity");
        assert_eq!(
            command_digest(&sandboxed, Some(&isolation))
                .expect("repeated fixed launcher identity must be hashable")
                .expect("repeated fixed launcher must have an artifact identity"),
            initial_identity,
            "unchanged canonical launcher dependencies must have deterministic identity"
        );
        fs::write(&jar, b"changed-saxon-jar").expect("Saxon JAR fixture must be replaceable");
        assert_ne!(
            command_digest(&sandboxed, Some(&isolation))
                .expect("changed resolved launcher identity must be hashable")
                .expect("changed resolved launcher must retain an artifact identity"),
            initial_identity,
            "launcher identity must include the resolved JAR bytes"
        );
        fs::write(&jar, b"fixed-saxon-jar").expect("Saxon JAR fixture must be restorable");
        fs::write(&java, b"changed-java-runtime").expect("Java fixture must be replaceable");
        assert_ne!(
            command_digest(&sandboxed, Some(&isolation))
                .expect("changed Java identity must be hashable")
                .expect("changed Java runtime must retain an artifact identity"),
            initial_identity,
            "launcher identity must include the resolved Java runtime bytes"
        );
        fs::remove_dir_all(root).expect("fixed launcher fixture must be removable");
    }

    #[test]
    fn saxon_launcher_rejects_dynamic_and_wrapper_forms() {
        let root = temporary("hostile-saxon-launcher");
        let launcher = root.join("saxon");
        for script in [
            "#!/bin/bash\nexport JAVA_HOME=\"$(/usr/libexec/java_home)\"\nexec \"${JAVA_HOME}/bin/java\" -jar \"/fixed/saxon.jar\" \"$@\"\n",
            "#!/bin/bash\nexport JAVA_HOME=\"${JAVA_HOME:-/fixed/jdk}\"\necho wrapper\nexec \"${JAVA_HOME}/bin/java\" -jar \"/fixed/saxon.jar\" \"$@\"\n",
            "#!/usr/bin/env bash\nexport JAVA_HOME=\"${JAVA_HOME:-/fixed/jdk}\"\nexec \"${JAVA_HOME}/bin/java\" -jar \"/fixed/saxon.jar\" \"$@\"\n",
        ] {
            fs::write(&launcher, script).expect("hostile launcher fixture must be writable");
            assert_eq!(
                resolve_saxon_launcher(&launcher)
                    .expect_err("dynamic or wrapper launcher must fail closed")
                    .code,
                "transformer-offline-launcher"
            );
        }
        fs::write(&launcher, vec![b'x'; MAX_TRANSFORMER_LAUNCHER_BYTES + 1])
            .expect("oversized launcher fixture must be writable");
        assert_eq!(
            resolve_saxon_launcher(&launcher)
                .expect_err("oversized launcher must fail before parsing")
                .code,
            "transformer-offline-launcher-size"
        );
        fs::remove_dir_all(root).expect("hostile launcher fixture must be removable");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn literal_tmp_output_root_is_created_safely() {
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = PathBuf::from("/tmp").join(format!(
            "scxml-prepare-literal-tmp-{}-{sequence}",
            std::process::id()
        ));
        let output = root.join("output");

        ensure_directory(&output, "output-create")
            .expect("literal /tmp output root must support secure descriptor-relative creation");
        assert!(output.is_dir());
        fs::remove_dir_all(root).expect("literal /tmp output fixture must be removable");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn literal_tmp_output_root_still_rejects_nested_symlinks() {
        use std::os::unix::fs::symlink;

        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = PathBuf::from("/tmp").join(format!(
            "scxml-prepare-literal-tmp-symlink-{}-{sequence}",
            std::process::id()
        ));
        let attacker = root.join("attacker");
        let redirected = root.join("redirected");
        fs::create_dir_all(&attacker).expect("attacker fixture must be creatable");
        symlink(&attacker, &redirected).expect("nested symlink fixture must be creatable");

        let error = ensure_directory(&redirected.join("output"), "output-create")
            .expect_err("literal /tmp normalization must not permit nested symlink traversal");
        assert_eq!(error.code, "output-create");
        assert!(!attacker.join("output").exists());
        fs::remove_dir_all(root).expect("literal /tmp symlink fixture must be removable");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn offline_launch_wraps_raw_transformer_in_network_denial_sandbox() {
        let root = temporary("offline-command-policy");
        let source = root.join("source.txml");
        let template = root.join("template.xsl");
        let output = root.join("output.scxml");
        fs::write(&source, "<fixture/>").expect("offline source fixture must be writable");
        fs::write(&template, "<stylesheet/>").expect("offline stylesheet fixture must be writable");
        let mut isolation = offline_isolation().expect("macOS host sandbox must be available");
        let raw_command = vec!["/usr/bin/true".into()];
        let prefix = offline_identity_command(&raw_command, &isolation)
            .expect("identity policy must be constructible");
        isolation.identity_digest = command_digest(&prefix, Some(&isolation))
            .expect("test command identity must be hashable")
            .expect("offline test command must have a digest");
        let transformer = SaxonTransformer {
            command: raw_command,
            timeout: DEFAULT_SAXON_TIMEOUT,
            offline_isolation: Some(isolation),
        };
        let command = transformer
            .command_for(&source, &template, &output)
            .expect("sandboxed command must be constructible without network access");
        assert_eq!(
            command.first().map(String::as_str),
            Some(MACOS_SANDBOX_EXEC)
        );
        assert_eq!(command.get(1).map(String::as_str), Some("-p"));
        let profile = command.get(2).expect("generated policy must be present");
        assert!(profile.contains("(deny default)"));
        assert!(!profile.contains("(allow default)"));
        assert!(profile.contains(&sandbox_path(&source).expect("source path must encode")));
        assert!(profile.contains(&sandbox_path(&template).expect("template path must encode")));
        assert!(profile.contains(&sandbox_path(&root).expect("output area must encode")));
        assert_eq!(command.get(3).map(String::as_str), Some("--"));
        assert_eq!(command.get(4).map(String::as_str), Some("/usr/bin/true"));
        assert_ne!(
            command.first(),
            command.get(4),
            "raw transformer must not be spawned"
        );
        let identity = transformer
            .identity()
            .expect("sandboxed identity probe must execute through the wrapper");
        assert_eq!(identity.name, "saxon-process-sandboxed");
        // The recorded identity is portable: no sandbox policy, no absolute
        // install paths, only the transformer argv with file names.
        assert_eq!(
            identity.command_argv.first().map(String::as_str),
            Some("true")
        );
        assert!(
            identity
                .command_argv
                .iter()
                .all(|argument| !argument.starts_with('/') && !argument.contains("(version 1)")),
            "portable identity must not embed host paths: {:?}",
            identity.command_argv
        );
        assert_eq!(
            identity.command_argv.get(1..),
            prefix.get(SANDBOXED_TRANSFORMER_INDEX + 1..)
        );
        assert_eq!(
            identity.executable_digest.as_deref().map(str::len),
            Some(64)
        );
        fs::remove_dir_all(root).expect("offline command fixture must be removable");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_offline_profile_denies_network_without_live_endpoint() {
        let profile = "(version 1) (deny default) (allow process-exec (literal \"/usr/bin/python3\")) (allow file-read*)";
        let output = Command::new(MACOS_SANDBOX_EXEC)
            .args([
                "-p",
                profile,
                "--",
                "/usr/bin/python3",
                "-c",
                "import socket; socket.socket().connect(('127.0.0.1', 9))",
            ])
            .output()
            .expect("macOS sandbox network-denial probe must start");
        assert!(!output.status.success(), "sandboxed network call must fail");
        let stderr = String::from_utf8(output.stderr)
            .expect("macOS Python sandbox diagnostic must be valid UTF-8");
        assert!(
            stderr.contains("Operation not permitted"),
            "failure must come from sandbox denial, not a live endpoint"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn offline_child_environment_is_explicit_and_secret_free() {
        let mut command = Command::new("/usr/bin/env");
        command.env("SCXML_OFFLINE_SECRET", "must-not-cross-boundary");
        configure_offline_child(&mut command, Sandbox::Enabled);
        let output = command
            .output()
            .expect("minimal offline environment probe must execute");
        assert!(output.status.success());
        let environment =
            String::from_utf8(output.stdout).expect("offline environment probe must emit UTF-8");
        assert_eq!(
            environment
                .lines()
                .collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from(["LANG=C", "LC_ALL=C", "TZ=UTC"])
        );
        assert!(!environment.contains("SCXML_OFFLINE_SECRET"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn offline_profile_denies_filesystem_and_process_escalation() {
        let root = temporary("offline-hostile-access");
        let secret = root.join("secret.txt");
        fs::write(&secret, "must-not-be-readable")
            .expect("hostile filesystem fixture must be writable");

        let cat = vec!["/bin/cat".to_owned()];
        let cat_profile = macos_offline_profile(
            &cat,
            &[
                PathBuf::from("/"),
                PathBuf::from(MACOS_DYLD),
                PathBuf::from("/bin/cat"),
            ],
            None,
        )
        .expect("filesystem denial profile must be constructible");
        let output = Command::new(MACOS_SANDBOX_EXEC)
            .args(["-p", &cat_profile, "--", "/bin/cat"])
            .arg(&secret)
            .output()
            .expect("filesystem denial probe must start");
        assert!(!output.status.success());
        assert!(
            !output
                .stdout
                .windows(20)
                .any(|window| window == b"must-not-be-readable")
        );

        let shell = vec!["/bin/sh".to_owned()];
        let shell_profile = macos_offline_profile(
            &shell,
            &[
                PathBuf::from("/"),
                PathBuf::from(MACOS_DYLD),
                PathBuf::from("/bin/sh"),
            ],
            None,
        )
        .expect("process denial profile must be constructible");
        let status = Command::new(MACOS_SANDBOX_EXEC)
            .args(["-p", &shell_profile, "--", "/bin/sh", "-c", "/usr/bin/true"])
            .status()
            .expect("process escalation probe must start");
        assert!(
            !status.success(),
            "unlisted child executable must be denied"
        );
        fs::remove_dir_all(root).expect("hostile access fixture must be removable");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn offline_launch_fails_closed_without_verified_isolation() {
        let error = SaxonTransformer::offline(vec!["saxon".into()])
            .expect_err("unsupported hosts must reject offline transformation");
        assert_eq!(error.code, "transformer-offline-isolation-unavailable");
    }

    struct NetworkAttemptTransformer {
        invoked: Cell<bool>,
    }

    impl CorpusTransformer for NetworkAttemptTransformer {
        fn identity(&self) -> Result<TransformerIdentity, PrepareError> {
            self.invoked.set(true);
            Err(PrepareError::new(
                "test-network-attempt",
                "simulated network attempt",
            ))
        }

        fn transform(
            &self,
            _source: &Path,
            _template: &Path,
            _output: &Path,
        ) -> Result<(), PrepareError> {
            self.invoked.set(true);
            Err(PrepareError::new(
                "test-network-attempt",
                "simulated network attempt",
            ))
        }
    }

    #[cfg(unix)]
    fn temporary(name: &str) -> PathBuf {
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary_root = fs::canonicalize(std::env::temp_dir())
            .expect("system temporary directory must have a stable canonical path");
        let path = temporary_root.join(format!(
            "scxml-prepare-{name}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("isolated preparation test directory must be creatable");
        path
    }

    #[test]
    fn acquisition_urls_reject_secrets_and_unbounded_values() {
        for value in [
            "https://user:secret@example.com/corpus",
            "https://example.com/corpus?token=secret",
            "https://example.com/corpus#secret",
        ] {
            assert!(
                parse_https_origin(value).is_err(),
                "credential-, query-, and fragment-bearing URLs must fail closed"
            );
        }
        let oversized = format!("https://example.com/{}", "a".repeat(MAX_URL_BYTES));
        assert_eq!(
            parse_https_origin(&oversized)
                .expect_err("over-bound URLs must fail before parsing")
                .code,
            "download-url-size"
        );
    }

    #[test]
    fn offline_policy_rejects_network_capable_fake_before_invocation() {
        let root = temporary("offline-network-fake");
        let source = root.join("source");
        let templates = root.join("templates");
        fs::create_dir_all(&source).expect("network fake source directory must be creatable");
        fs::create_dir_all(&templates).expect("network fake template directory must be creatable");
        fs::write(
            source.join("manifest.xml"),
            r##"<assertions baseuri="https://example.invalid/irp"><assert id="1" specnum="1" specid="#offline"><test id="1" conformance="mandatory" manual="false"><start uri="case.txml"/></test></assert></assertions>"##,
        )
        .expect("network fake manifest must be writable");
        fs::write(source.join("case.txml"), "<fixture/>")
            .expect("network fake source must be writable");
        fs::write(templates.join("ecmascript.xsl"), "<stylesheet/>")
            .expect("network fake stylesheet must be writable");
        let transformer = NetworkAttemptTransformer {
            invoked: Cell::new(false),
        };
        let error = prepare_corpus(
            &PrepareOptions {
                errata: None,
                namespace: None,
                manifest: source.join("manifest.xml").display().to_string(),
                base_uri: None,
                templates,
                datamodel: "ecmascript".into(),
                cache: root.join("cache"),
                output: root.join("output"),
                source_dir: Some(source),
                revision: Some("fixture-revision".into()),
                offline: true,
                force: false,
                max_commands: 32,
            },
            &transformer,
        )
        .expect_err("offline policy must reject an unrestricted fake transformer");
        assert_eq!(error.code, "transformer-offline-policy");
        assert!(
            !transformer.invoked.get(),
            "offline policy rejection must occur before a simulated network attempt"
        );
        fs::remove_dir_all(root).expect("network fake fixture must be removable");
    }

    #[test]
    fn offline_xml_preflight_rejects_external_resolution_without_network() {
        let root = temporary("offline-xml-resolution");
        let stylesheet = root.join("network.xsl");
        let source = root.join("network.txml");
        for attack in [
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:import href="https://example.invalid/network.xsl"/></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:value-of select="document('https://example.invalid/data.xml')"/></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:value-of select="doc&#x28;'https://example.invalid/data.xml')"/></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:value-of select="environment-variable('SCXML_OFFLINE_SECRET')"/></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:value-of select="available-environment-variables()"/></xsl:stylesheet>"#,
            // XPath 3.x named function references, under any spelling.
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:value-of select="doc#1('https://example.invalid/data.xml')"/></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:value-of select="unparsed-text#1('file:///etc/passwd')"/></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:value-of select="environment-variable#1('HOME')"/></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:value-of select="transform#1(map{'stylesheet-location':'https://example.invalid/x.xsl'})"/></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:value-of select="let $f := doc#1 return $f('https://example.invalid/data.xml')"/></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:value-of select="let $f := unparsed-text#1 return $f('file:///etc/passwd')"/></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:value-of select="Q{http://www.w3.org/2005/xpath-functions}doc#1"/></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:value-of select="doc &#x23; 1"/></xsl:stylesheet>"#,
            // Prefixed, EQName, commented, and arrow spellings of a call.
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform" xmlns:fn="http://www.w3.org/2005/xpath-functions"><xsl:value-of select="fn:doc('https://example.invalid/data.xml')"/></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:value-of select="Q{http://www.w3.org/2005/xpath-functions}doc('https://example.invalid/data.xml')"/></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:value-of select="doc (: split :) ('https://example.invalid/data.xml')"/></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:value-of select="'file:///etc/passwd' => unparsed-text()"/></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:value-of select="function-lookup(xs:QName('fn:doc'), 1)('https://example.invalid/data.xml')"/></xsl:stylesheet>"#,
            // Dynamic function calls.
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:variable name="f"/><xsl:value-of select="$f('https://example.invalid/data.xml')"/></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:value-of select="(node())('https://example.invalid/data.xml')"/></xsl:stylesheet>"#,
            // XSLT instructions and declarations under a rebound prefix.
            r#"<x:stylesheet xmlns:x="http://www.w3.org/1999/XSL/Transform"><x:template match="/"><x:evaluate xpath="'doc(''https://example.invalid/data.xml'')'"/></x:template></x:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:template match="/"><x:evaluate xmlns:x="http://www.w3.org/1999/XSL/Transform" xpath="$p"/></xsl:template></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><y:include xmlns:y="http://www.w3.org/1999/XSL/Transform" href="https://example.invalid/network.xsl"/></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:template match="/"><xsl:source-document href="https://example.invalid/data.xml"/></xsl:template></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:template match="/"><xsl:result-document href="file:///tmp/escape.xml"/></xsl:template></xsl:stylesheet>"#,
            // Evaluated attributes outside the vocabulary.
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:template match="/" use-when="environment-variable('HOME') = ''"/></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:value-of _select="doc('https://example.invalid/data.xml')"/></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform" expand-text="yes"><xsl:template match="/">{doc('https://example.invalid/data.xml')}</xsl:template></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform" xmlns:saxon="http://saxon.sf.net/" extension-element-prefixes="saxon"/>"#,
            // Literal result elements: value templates, XSLT attributes, and
            // namespaces outside the pinned output.
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:template match="/"><log expr="{unparsed-text('file:///etc/passwd')}"/></xsl:template></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:template match="/"><log expr="{{literal}} {doc#1}"/></xsl:template></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:template match="/"><log xsl:use-when="environment-variable('HOME')"/></xsl:template></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:template match="/"><xi:include xmlns:xi="http://www.w3.org/2001/XInclude" href="https://example.invalid/data.xml"/></xsl:template></xsl:stylesheet>"#,
            r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:template match="/"><saxon:do xmlns:saxon="http://saxon.sf.net/" action="doc('x')"/></xsl:template></xsl:stylesheet>"#,
            // A simplified stylesheet (literal root) is outside the contract.
            r#"<out xmlns:xsl="http://www.w3.org/1999/XSL/Transform" xsl:version="3.0"/>"#,
        ] {
            fs::write(&stylesheet, attack)
                .expect("adversarial offline stylesheet fixture must be writable");
            assert_eq!(
                validate_offline_xml(&stylesheet, OfflineXmlKind::Stylesheet)
                    .expect_err("external stylesheet resolution must fail before execution")
                    .code,
                "transformer-offline-stylesheet-resolution"
            );
        }
        fs::write(
            &source,
            r#"<!DOCTYPE fixture SYSTEM "https://example.invalid/network.dtd"><fixture/>"#,
        )
        .expect("adversarial offline source fixture must be writable");
        assert_eq!(
            validate_offline_xml(&source, OfflineXmlKind::Source)
                .expect_err("external source entity resolution must fail before execution")
                .code,
            "transformer-offline-xml-resolution"
        );
        fs::remove_dir_all(root).expect("offline XML fixture must be removable");
    }

    #[test]
    fn offline_xml_preflight_accepts_pinned_w3c_stylesheets() {
        let templates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/conformance/w3c-irp/templates")
            .canonicalize()
            .expect("pinned template directory must have a canonical path");
        for name in ["ecmascript.xsl", "xpath.xsl"] {
            validate_offline_xml(&templates.join(name), OfflineXmlKind::Stylesheet)
                .expect("pinned W3C stylesheet must satisfy the offline execution contract");
        }
        // The checked-in template-stage errata inputs are corrected pinned
        // stylesheets and are screened the same way during preparation.
        let artifacts = templates
            .join("../prepared/xpath/artifacts")
            .canonicalize()
            .expect("prepared XPath artifact directory must have a canonical path");
        let mut corrected = Vec::new();
        for run in fs::read_dir(&artifacts).expect("prepared XPath artifacts must be listable") {
            let run = run
                .expect("prepared XPath artifact entry must be readable")
                .path();
            for stage in fs::read_dir(&run).expect("prepared artifact run must be listable") {
                let errata = stage
                    .expect("prepared artifact stage must be readable")
                    .path()
                    .join("errata");
                let Ok(entries) = fs::read_dir(&errata) else {
                    continue;
                };
                for erratum in entries {
                    let input = erratum
                        .expect("erratum entry must be readable")
                        .path()
                        .join("input/xpath.xsl");
                    if input.is_file() {
                        corrected.push(input);
                    }
                }
            }
        }
        assert!(
            corrected.len() >= 3,
            "the checked-in XPath template errata inputs must be found (found {})",
            corrected.len()
        );
        for input in corrected {
            validate_offline_xml(&input, OfflineXmlKind::Stylesheet).unwrap_or_else(|error| {
                panic!(
                    "checked-in erratum stylesheet {} must satisfy the offline execution contract: {error:?}",
                    input.display()
                )
            });
        }
    }

    #[test]
    fn offline_xpath_screen_allows_only_grouping_and_the_pinned_calls() {
        for accepted in [
            "@* | node()",
            "//@conf:targetpass",
            "regex-group(1)",
            "child::node()",
            "$op = '==' and ($value or ($targetArray))",
            "if ($op) then (1) else ()",
            "'doc(''x'') and #1 and $f()' (: doc#1('x') :)",
            "for $x in (1, 2) return ($x)",
            "1.5e3 div (2)",
        ] {
            screen_xpath(accepted).unwrap_or_else(|error| {
                panic!("{accepted:?} must pass the offline XPath screen: {error:?}")
            });
        }
        for rejected in [
            "doc('x')",
            "doc#1",
            "$f(1)",
            "(regex-group#1)(1)",
            "map{}('k')",
            "node()(1)",
            "'unterminated",
            "(: unterminated",
            "prefix:regex-group(1)",
            "Q{http://www.w3.org/2005/xpath-functions}node()",
        ] {
            assert_eq!(
                screen_xpath(rejected)
                    .expect_err("a call outside the vocabulary must be rejected")
                    .code,
                "transformer-offline-stylesheet-resolution",
                "{rejected:?}"
            );
        }
        screen_value_template("{{literal}} and {regex-group(1)}")
            .expect("escaped braces and an allowed call must pass");
        for rejected in ["{doc('x')}", "a}b", "{unterminated", "{'}'} {doc#1}"] {
            assert!(
                screen_value_template(rejected).is_err(),
                "{rejected:?} must be rejected"
            );
        }
    }

    #[test]
    fn address_policy_rejects_non_global_ranges() {
        for address in [
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
            IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(192, 0, 0, 8)),
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
            IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(240, 0, 0, 1)),
            IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            IpAddr::V6("fc00::1".parse().expect("ULA fixture must parse")),
            IpAddr::V6("fe80::1".parse().expect("link-local fixture must parse")),
            IpAddr::V6("ff02::1".parse().expect("multicast fixture must parse")),
            IpAddr::V6(
                "2001:db8::1"
                    .parse()
                    .expect("documentation fixture must parse"),
            ),
            IpAddr::V6(
                "::ffff:10.0.0.1"
                    .parse()
                    .expect("mapped private fixture must parse"),
            ),
            IpAddr::V6(
                "::ffff:198.18.0.1"
                    .parse()
                    .expect("mapped benchmark fixture must parse"),
            ),
        ] {
            assert!(!is_global_address(address));
        }
        assert!(is_global_address(IpAddr::V4(Ipv4Addr::new(
            93, 184, 216, 34
        ))));
        assert!(is_global_address(IpAddr::V6(
            "2606:2800:220:1:248:1893:25c8:1946"
                .parse()
                .expect("global IPv6 fixture must parse")
        )));
        assert!(is_global_address(IpAddr::V6(
            "::ffff:93.184.216.34"
                .parse()
                .expect("mapped global fixture must parse")
        )));
    }

    #[test]
    fn curl_arguments_pin_addresses_and_disable_ambient_configuration() {
        let pinned = PinnedOrigin {
            origin: HttpsOrigin {
                host: "example.com".into(),
                port: 443,
            },
            addresses: vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))],
        };
        let arguments = curl_arguments(&pinned, CurlMode::Download)
            .expect("bounded public address pin must build curl arguments");
        assert_eq!(arguments.first().map(String::as_str), Some("--disable"));
        assert!(arguments.windows(2).any(|pair| pair == ["--noproxy", "*"]));
        assert!(arguments.windows(2).any(|pair| pair == ["--proxy", ""]));
        assert!(
            arguments
                .windows(2)
                .any(|pair| { pair == ["--resolve", "example.com:443:93.184.216.34"] })
        );
    }

    #[test]
    fn bounded_read_rejects_oversized_local_manifest_before_full_read() {
        let root = temporary("bounded-read");
        let manifest = root.join("manifest.xml");
        let file = File::create(&manifest).expect("bounded-read fixture must be creatable");
        file.set_len(
            u64::try_from(MAX_IRP_MANIFEST_BYTES + 1).expect("manifest test bound must fit in u64"),
        )
        .expect("bounded-read fixture length must be settable");
        assert_eq!(
            read_bounded_file(
                &manifest,
                MAX_IRP_MANIFEST_BYTES,
                "manifest-read",
                "manifest-size"
            )
            .expect_err("oversized local manifest must fail before allocation")
            .code,
            "manifest-size"
        );
        fs::remove_dir_all(root).expect("bounded-read fixture must be removable");
    }

    #[test]
    fn atomic_output_failure_preserves_destination_and_cleans_temporary_file() {
        let root = temporary("atomic-failure");
        let output = root.join("corpus.json");
        fs::write(&output, b"previous").expect("atomic destination fixture must be writable");
        let error = atomic_output(&output, "test", |_temporary| {
            Err(PrepareError::new("test-write", "intentional test failure"))
        })
        .expect_err("failed atomic writer must report its failure");
        assert_eq!(error.code, "test-write");
        assert_eq!(
            fs::read(&output).expect("previous destination must remain readable"),
            b"previous"
        );
        assert_eq!(
            fs::read_dir(&root)
                .expect("atomic fixture directory must be readable")
                .count(),
            1
        );
        fs::remove_dir_all(root).expect("atomic fixture must be removable");
    }

    #[cfg(unix)]
    #[test]
    fn bounded_open_rejects_symlink_substitution() {
        use std::os::unix::fs::symlink;

        let root = temporary("bounded-open-symlink");
        let trusted = root.join("trusted.xml");
        let replacement = root.join("replacement.xml");
        fs::write(&trusted, b"trusted").expect("trusted input fixture must be writable");
        fs::write(&replacement, b"attacker").expect("replacement fixture must be writable");
        fs::remove_file(&trusted).expect("trusted fixture must be replaceable");
        symlink(&replacement, &trusted).expect("symlink substitution fixture must be creatable");

        let error = read_bounded_file(&trusted, 32, "test-read", "test-size")
            .expect_err("a symlink substituted for a bounded input must fail closed");
        assert_eq!(error.code, "test-read");
        fs::remove_dir_all(root).expect("symlink substitution fixture must be removable");
    }

    #[cfg(unix)]
    #[test]
    fn descriptor_owned_read_is_not_redirected_by_same_size_rename() {
        let root = temporary("bounded-open-rename");
        let input = root.join("input.xml");
        let displaced = root.join("displaced.xml");
        fs::write(&input, b"trusted").expect("trusted input fixture must be writable");
        let mut file = open_regular_file_nofollow(&input, 32, "test-read", "test-size")
            .expect("trusted regular file must open without following links");
        fs::rename(&input, &displaced).expect("opened fixture must be movable");
        fs::write(&input, b"hostile").expect("same-size replacement must be writable");

        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .expect("descriptor-owned input must remain readable");
        assert_eq!(bytes, b"trusted");
        fs::remove_dir_all(root).expect("rename substitution fixture must be removable");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_copy_rejects_same_size_source_swap_after_digest() {
        let root = temporary("atomic-copy-source-swap");
        let source = root.join("source.xml");
        let displaced = root.join("displaced.xml");
        let output = root.join("output.xml");
        fs::write(&source, b"trusted").expect("trusted copy source must be writable");
        let digest = sha256_bounded_file(&source).expect("trusted copy source must be hashable");
        fs::rename(&source, &displaced).expect("trusted copy source must be movable");
        fs::write(&source, b"hostile").expect("same-size copy replacement must be writable");

        let error = atomic_copy(&source, &output, "test", &digest)
            .expect_err("copy must reject bytes that differ from the established digest");
        assert_eq!(error.code, "source-changed");
        assert!(
            !output.exists(),
            "digest-mismatched output must not publish"
        );
        fs::remove_dir_all(root).expect("copy substitution fixture must be removable");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_output_stays_in_opened_parent_after_directory_swap() {
        use std::os::unix::fs::symlink;

        let root = temporary("atomic-parent-swap");
        let owned = root.join("owned");
        let moved = root.join("moved");
        let attacker = root.join("attacker");
        fs::create_dir(&owned).expect("owned output directory must be creatable");
        fs::create_dir(&attacker).expect("attacker directory must be creatable");
        let output = owned.join("corpus.json");

        atomic_output_file(&output, "test", |writer| {
            fs::rename(&owned, &moved).expect("owned directory swap fixture must be movable");
            symlink(&attacker, &owned).expect("attacker directory symlink must be creatable");
            writer
                .write_all(b"descriptor-owned")
                .map_err(|error| PrepareError::new("test-write", error.to_string()))
        })
        .expect("descriptor-relative commit must survive pathname substitution");

        assert_eq!(
            fs::read(moved.join("corpus.json"))
                .expect("committed output must remain under opened parent"),
            b"descriptor-owned"
        );
        assert!(
            !attacker.join("corpus.json").exists(),
            "attacker-controlled replacement directory must receive no output"
        );
        fs::remove_dir_all(root).expect("atomic parent swap fixture must be removable");
    }
}
