//! The release session components `run`, `snapshot run`, and
//! `conformance run` execute through.
//!
//! Every session runs through the shipped Wasm components under the
//! Wasmtime session host; there is no other execution path. By default they
//! are the release `scx` embeds (`build.rs`: the authenticated
//! `scripts/verify-wasm-capabilities.sh` release, whose manifest SHA-256 is
//! the trust anchor). `--components DIR` with
//! `--components-manifest-sha256 HEX` overrides them with another release
//! directory and its trusted manifest digest; every component is
//! authenticated against the manifest and fails closed either way.
//! Wasmtime stays behind this crate's `wasmtime` feature (on by default): a
//! build without it embeds nothing and reports
//! `ComponentsError::WasmtimeDisabled` before running any case.
//!
//! A namespace suite (`namespaces/<name>/irp`) also binds the extension
//! namespaces it uses: `--extension-package MANIFEST.json` names each
//! package, whose `extension-artifact` is read from `--components DIR` and
//! authenticated by `--extension-component-sha256 URI=HEX`
//! ([`ExtensionArgs`]).

use std::fmt;
use std::path::PathBuf;

use scxml_conformance::CaseExecutor;

/// The release session components a run executes through.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComponentArgs {
    /// The release `scx` embeds (the default).
    Embedded,
    /// `--components DIR` and `--components-manifest-sha256 HEX`.
    Directory {
        /// Directory of release component artifacts.
        dir: PathBuf,
        /// Trusted lowercase SHA-256 of that directory's `manifest.tsv`.
        manifest_sha256: String,
    },
}

/// The release `scx` embeds (`build.rs`).
#[cfg(feature = "wasmtime")]
pub(crate) mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded_release.rs"));

    /// The embedded bytes of the artifact file `name`.
    pub(crate) fn artifact(name: &str) -> Option<&'static [u8]> {
        ARTIFACTS
            .iter()
            .find(|(file, _)| *file == name)
            .map(|(_, bytes)| *bytes)
    }
}

impl ComponentArgs {
    /// Resolves the raw `--components` and `--components-manifest-sha256`
    /// values: neither selects the embedded release, both a release
    /// directory.
    ///
    /// # Errors
    ///
    /// A usage message when only one flag is given or the digest is not 64
    /// lowercase hex characters.
    pub fn resolve(dir: Option<PathBuf>, manifest_sha256: Option<String>) -> Result<Self, String> {
        match (dir, manifest_sha256) {
            (None, None) => Ok(Self::Embedded),
            (Some(dir), Some(manifest_sha256)) => {
                if manifest_sha256.len() != 64
                    || !manifest_sha256
                        .bytes()
                        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
                {
                    return Err(
                        "--components-manifest-sha256 must be 64 lowercase hex characters".into(),
                    );
                }
                Ok(Self::Directory {
                    dir,
                    manifest_sha256,
                })
            }
            _ => Err("--components DIR and --components-manifest-sha256 HEX override the embedded components together".into()),
        }
    }

    /// The trusted SHA-256 of the release's `manifest.tsv`.
    #[cfg(feature = "wasmtime")]
    pub fn manifest_sha256(&self) -> &str {
        match self {
            Self::Embedded => embedded::MANIFEST_SHA256,
            Self::Directory {
                manifest_sha256, ..
            } => manifest_sha256,
        }
    }

    /// Authenticates the release's session components and links them into
    /// one session host whose runs get `fuel`.
    ///
    /// # Errors
    ///
    /// A release that does not authenticate, or a Wasmtime compilation or
    /// linker failure.
    #[cfg(feature = "wasmtime")]
    pub fn session_host(
        &self,
        fuel: u64,
    ) -> Result<scxml_host_wasmtime::SessionHost, Box<dyn std::error::Error>> {
        use scxml_host_wasmtime::{
            ArtifactKind, SessionHost, SessionHostComponents, TelemetryProviders,
            TrustedManifestDigest, WasmtimeHostConfig, WasmtimeStoreConfig, authenticate_component,
        };
        let (dir, manifest_sha256) = match self {
            Self::Directory {
                dir,
                manifest_sha256,
            } => return Ok(SessionHost::from_release_dir(dir, manifest_sha256, fuel)?),
            Self::Embedded => (embedded::MANIFEST, embedded::MANIFEST_SHA256),
        };
        let trusted = TrustedManifestDigest::from_hex(manifest_sha256)?;
        let load = |kind: ArtifactKind| {
            let file = kind.artifact_file();
            let bytes = embedded::artifact(&file)
                .ok_or_else(|| format!("the embedded release has no `{file}`"))?;
            authenticate_component(dir, trusted, kind, &file, bytes)
                .map_err(|error| format!("the embedded `{file}` does not authenticate: {error}"))
        };
        let machine = load(ArtifactKind::Machine)?;
        let compiler = load(ArtifactKind::Compiler)?;
        let data_model_ecmascript = load(ArtifactKind::DataModelEcmascript)?;
        let data_model_xpath = load(ArtifactKind::DataModelXpath)?;
        let data_model_null = load(ArtifactKind::DataModelNull)?;
        let io_scxml = load(ArtifactKind::IoProcessorScxml)?;
        let io_basichttp = load(ArtifactKind::IoProcessorBasicHttp)?;
        let clock = load(ArtifactKind::Clock)?;
        let config = WasmtimeHostConfig {
            store: WasmtimeStoreConfig {
                fuel,
                ..WasmtimeStoreConfig::default()
            },
            ..WasmtimeHostConfig::default()
        };
        Ok(SessionHost::new(
            &SessionHostComponents {
                machine: &machine,
                compiler: &compiler,
                data_model_ecmascript: &data_model_ecmascript,
                data_model_xpath: &data_model_xpath,
                data_model_null: &data_model_null,
                io_scxml: &io_scxml,
                io_basichttp: &io_basichttp,
                clock: &clock,
            },
            &config,
            &TelemetryProviders::new(),
        )?)
    }
}

/// The extension namespaces `conformance run` binds.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExtensionArgs {
    /// `--extension-package` manifests, in flag order.
    pub manifests: Vec<PathBuf>,
    /// `--extension-component-sha256 URI=HEX`, by namespace URI.
    pub digests: Vec<(String, String)>,
}

impl ExtensionArgs {
    /// Records one `--extension-component-sha256 URI=HEX` value.
    ///
    /// # Errors
    ///
    /// A usage message for a value without `=`, a digest that is not 64
    /// lowercase hex characters, or a URI given twice.
    pub fn digest(&mut self, value: &str) -> Result<(), String> {
        let (uri, digest) = value
            .rsplit_once('=')
            .filter(|(uri, _)| !uri.is_empty())
            .ok_or("--extension-component-sha256 requires URI=HEX")?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err("--extension-component-sha256 needs 64 lowercase hex characters".into());
        }
        if self.digests.iter().any(|(bound, _)| bound == uri) {
            return Err("--extension-component-sha256 names one namespace twice".into());
        }
        self.digests.push((uri.to_owned(), digest.to_owned()));
        Ok(())
    }
}

/// Failure to construct the component executor.
#[derive(Debug)]
pub enum ComponentsError {
    /// This build has no Wasmtime component host (`wasmtime` feature off).
    #[cfg(not(feature = "wasmtime"))]
    WasmtimeDisabled,
    /// The release components failed to authenticate, read, or compile.
    #[cfg(feature = "wasmtime")]
    Release(scxml_conformance::WasmExecutorError),
    /// An `--extension-package` has no `--extension-component-sha256`, or a
    /// digest names no bound package.
    #[cfg(feature = "wasmtime")]
    ExtensionDigest(String),
}

impl fmt::Display for ComponentsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(not(feature = "wasmtime"))]
            Self::WasmtimeDisabled => formatter.write_str(
                "[wasmtime-disabled] conformance run executes only through the shipped \
                 components under Wasmtime; rebuild scx with --features wasmtime. No case \
                 was executed",
            ),
            #[cfg(feature = "wasmtime")]
            Self::Release(error) => write!(formatter, "[components-rejected] {error}"),
            #[cfg(feature = "wasmtime")]
            Self::ExtensionDigest(message) => write!(formatter, "[components-rejected] {message}"),
        }
    }
}

impl std::error::Error for ComponentsError {}

/// Constructs the component executor for `args`.
///
/// # Errors
///
/// [`ComponentsError`] when this build has no Wasmtime host or the release
/// is rejected.
pub fn executor(
    args: &ComponentArgs,
    extensions: &ExtensionArgs,
) -> Result<Box<dyn CaseExecutor>, ComponentsError> {
    executor_with_snapshots(args, extensions, SnapshotCollection::Off)
}

/// Whether a component executor collects session-snapshot documents.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotCollection {
    /// Collect nothing.
    Off,
    /// One read-only document per reached checkpoint, on telemetry
    /// providers whose IDs are pinned so documents are byte-stable across
    /// runs (docs/session-snapshot.md, "Non-perturbation proof over the
    /// IRP").
    EveryCheckpoint,
}

/// Constructs the component executor for `args`, collecting snapshots as
/// `collection` says.
///
/// # Errors
///
/// [`ComponentsError`] when this build has no Wasmtime host or the release
/// is rejected.
#[cfg(feature = "wasmtime")]
pub fn executor_with_snapshots(
    args: &ComponentArgs,
    extensions: &ExtensionArgs,
    collection: SnapshotCollection,
) -> Result<Box<dyn CaseExecutor>, ComponentsError> {
    let mut releases = Vec::with_capacity(extensions.manifests.len());
    for manifest in &extensions.manifests {
        // A namespace suite's extension components are release artifacts
        // too, read from the release directory.
        let ComponentArgs::Directory { dir, .. } = args else {
            return Err(ComponentsError::ExtensionDigest(
                "--extension-package reads its extension-artifact from --components DIR: give --components and --components-manifest-sha256".into(),
            ));
        };
        let mut release = scxml_conformance::ExtensionRelease::read(manifest, dir, "")
            .map_err(ComponentsError::Release)?;
        let uri = &release.manifest.namespace_uri;
        release.component_sha256 = extensions
            .digests
            .iter()
            .find(|(bound, _)| bound == uri)
            .map(|(_, digest)| digest.clone())
            .ok_or_else(|| {
                ComponentsError::ExtensionDigest(format!(
                    "extension package `{}` needs --extension-component-sha256 for its namespace",
                    manifest.display()
                ))
            })?;
        releases.push(release);
    }
    if let Some((uri, _)) = extensions.digests.iter().find(|(uri, _)| {
        !releases
            .iter()
            .any(|release| release.manifest.namespace_uri == *uri)
    }) {
        return Err(ComponentsError::ExtensionDigest(format!(
            "--extension-component-sha256 names `{uri}`, which no --extension-package binds"
        )));
    }
    let built = match (args, collection) {
        (
            ComponentArgs::Directory {
                dir,
                manifest_sha256,
            },
            SnapshotCollection::EveryCheckpoint,
        ) => scxml_conformance::WasmExecutor::from_release_dir_collecting_snapshots(
            dir,
            manifest_sha256,
        ),
        (
            ComponentArgs::Directory {
                dir,
                manifest_sha256,
            },
            SnapshotCollection::Off,
        ) => scxml_conformance::WasmExecutor::from_release_dir(dir, manifest_sha256),
        (ComponentArgs::Embedded, SnapshotCollection::EveryCheckpoint) => {
            scxml_conformance::WasmExecutor::from_release_bytes_collecting_snapshots(
                embedded::MANIFEST,
                embedded::MANIFEST_SHA256,
                &|name| embedded::artifact(name).map(<[u8]>::to_vec),
            )
        }
        (ComponentArgs::Embedded, SnapshotCollection::Off) => {
            scxml_conformance::WasmExecutor::from_release_bytes(
                embedded::MANIFEST,
                embedded::MANIFEST_SHA256,
                &|name| embedded::artifact(name).map(<[u8]>::to_vec),
            )
        }
    };
    let built = if releases.is_empty() {
        built
    } else {
        built.and_then(|executor| executor.with_extensions(releases))
    };
    built
        .map(|executor| Box::new(executor) as Box<dyn CaseExecutor>)
        .map_err(ComponentsError::Release)
}

/// Constructs the component executor for `args`.
///
/// # Errors
///
/// Always [`ComponentsError::WasmtimeDisabled`]: this build links no
/// Wasmtime host.
#[cfg(not(feature = "wasmtime"))]
pub const fn executor_with_snapshots(
    _args: &ComponentArgs,
    _extensions: &ExtensionArgs,
    _collection: SnapshotCollection,
) -> Result<Box<dyn CaseExecutor>, ComponentsError> {
    Err(ComponentsError::WasmtimeDisabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn the_embedded_release_is_the_default_and_both_flags_override_it() {
        assert_eq!(
            ComponentArgs::resolve(None, None),
            Ok(ComponentArgs::Embedded)
        );
        assert_eq!(
            ComponentArgs::resolve(Some("dir".into()), Some(DIGEST.into())),
            Ok(ComponentArgs::Directory {
                dir: PathBuf::from("dir"),
                manifest_sha256: DIGEST.into(),
            })
        );
        assert!(ComponentArgs::resolve(None, Some(DIGEST.into())).is_err());
        assert!(ComponentArgs::resolve(Some("dir".into()), None).is_err());
        assert!(ComponentArgs::resolve(Some("dir".into()), Some(DIGEST.to_uppercase())).is_err());
        assert!(ComponentArgs::resolve(Some("dir".into()), Some(DIGEST[1..].into())).is_err());
    }

    #[cfg(not(feature = "wasmtime"))]
    #[test]
    fn builds_without_wasmtime_fail_closed() {
        assert!(matches!(
            executor(&ComponentArgs::Embedded, &ExtensionArgs::default()),
            Err(ComponentsError::WasmtimeDisabled)
        ));
    }

    #[cfg(feature = "wasmtime")]
    #[test]
    fn a_missing_release_is_rejected() {
        let args = ComponentArgs::resolve(Some("missing-components".into()), Some(DIGEST.into()))
            .expect("valid flags resolve");
        assert!(matches!(
            executor(&args, &ExtensionArgs::default()),
            Err(ComponentsError::Release(_))
        ));
    }

    #[cfg(feature = "wasmtime")]
    #[test]
    fn the_embedded_release_authenticates() {
        assert_eq!(embedded::ARTIFACTS.len(), 8, "the eight session components");
        ComponentArgs::Embedded
            .session_host(scxml_host_wasmtime::DEFAULT_FUEL)
            .expect("the embedded release authenticates and links");
    }

    #[test]
    fn extension_digests_are_exact_and_named_once() {
        let mut extensions = ExtensionArgs::default();
        assert_eq!(extensions.digest(&format!("urn:x={DIGEST}")), Ok(()));
        assert!(extensions.digest(&format!("urn:x={DIGEST}")).is_err());
        assert!(extensions.digest(DIGEST).is_err());
        assert!(extensions.digest(&format!("={DIGEST}")).is_err());
        assert!(
            extensions
                .digest(&format!("urn:y={}", DIGEST.to_uppercase()))
                .is_err()
        );
        assert_eq!(
            extensions.digests,
            [("urn:x".to_owned(), DIGEST.to_owned())]
        );
    }
}
