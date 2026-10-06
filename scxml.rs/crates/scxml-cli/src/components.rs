//! The component execution path of `conformance run`.
//!
//! Every case runs through the shipped Wasm components under the Wasmtime
//! session host (`scxml_conformance::wasm::WasmExecutor`); there is no other
//! execution path. `--components DIR` names the release artifacts written by
//! `scripts/verify-wasm-capabilities.sh` and
//! `--components-manifest-sha256 HEX` their trusted manifest digest; the
//! executor authenticates every component against it and fails closed.
//! Wasmtime stays behind this crate's `wasmtime` feature: a build without
//! it reports `ComponentsError::WasmtimeDisabled` before running any case.
//!
//! A namespace suite (`namespaces/<name>/irp`) also binds the extension
//! namespaces it uses: `--extension-package MANIFEST.json` names each
//! package, whose `extension-artifact` is read from `--components DIR` and
//! authenticated by `--extension-component-sha256 URI=HEX`
//! ([`ExtensionArgs`]).

use std::fmt;
use std::path::PathBuf;

use scxml_conformance::CaseExecutor;

/// Release component artifacts selected for `conformance run`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentArgs {
    /// Directory of release component artifacts.
    pub dir: PathBuf,
    /// Trusted lowercase SHA-256 of that directory's `manifest.tsv`.
    pub manifest_sha256: String,
}

impl ComponentArgs {
    /// Resolves the raw `--components` and `--components-manifest-sha256`
    /// values.
    ///
    /// # Errors
    ///
    /// A usage message when either flag is missing or the digest is not 64
    /// lowercase hex characters.
    pub fn resolve(dir: Option<PathBuf>, manifest_sha256: Option<String>) -> Result<Self, String> {
        let dir = dir.ok_or("conformance run requires --components DIR")?;
        let manifest_sha256 =
            manifest_sha256.ok_or("conformance run requires --components-manifest-sha256 HEX")?;
        if manifest_sha256.len() != 64
            || !manifest_sha256
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err("--components-manifest-sha256 must be 64 lowercase hex characters".into());
        }
        Ok(Self {
            dir,
            manifest_sha256,
        })
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
                 components under Wasmtime; rebuild scxml-cli with --features wasmtime. No case \
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
        let mut release = scxml_conformance::ExtensionRelease::read(manifest, &args.dir, "")
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
    let built = match collection {
        SnapshotCollection::EveryCheckpoint => {
            scxml_conformance::WasmExecutor::from_release_dir_collecting_snapshots(
                &args.dir,
                &args.manifest_sha256,
            )
        }
        SnapshotCollection::Off => {
            scxml_conformance::WasmExecutor::from_release_dir(&args.dir, &args.manifest_sha256)
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
    fn both_component_flags_and_an_exact_digest_are_required() {
        assert_eq!(
            ComponentArgs::resolve(Some("dir".into()), Some(DIGEST.into())),
            Ok(ComponentArgs {
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
        let args = ComponentArgs::resolve(Some("dir".into()), Some(DIGEST.into()))
            .expect("valid flags resolve");
        assert!(matches!(
            executor(&args, &ExtensionArgs::default()),
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
