//! Bounded loading of one release components directory: the
//! `manifest.tsv` plus the `*.virtualized.wasm` artifacts that
//! `scripts/verify-wasm-capabilities.sh` writes.
//!
//! Every file is read through [`read_bounded_file`], which never loads more
//! than its bound plus one byte, even when the file grows after its
//! metadata was checked. Every artifact is then authenticated against the
//! caller-trusted manifest digest. A rejection names the artifact, the
//! manifest, and how to build a complete release directory.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use super::SessionHostComponents;
use crate::{
    ArtifactError, ArtifactKind, AuthenticatedComponent, HostError,
    MAX_AUTHENTICATED_COMPONENT_BYTES, TrustedManifestDigest, authenticate_component,
};

/// Byte bound of a release `manifest.tsv` (the authenticator's own 64 KiB
/// manifest bound, enforced before the manifest is read).
pub const MAX_RELEASE_MANIFEST_BYTES: u64 = 64 * 1024;

/// The file name of a release directory's manifest.
const MANIFEST_FILE: &str = "manifest.tsv";

/// How to produce a release directory every session component is in.
const RELEASE_HELP: &str = "build every release component into one directory with \
`sh scripts/verify-wasm-capabilities.sh` (with no component arguments it builds all of \
them into `$CARGO_TARGET_DIR/wasm-artifacts/release`), then pass that directory and the \
SHA-256 of its `manifest.tsv`";

/// Why a release components directory was not loaded.
#[derive(Debug)]
pub enum ReleaseError {
    /// A release file could not be opened or read.
    Read {
        /// The file.
        path: PathBuf,
        /// The I/O failure.
        error: std::io::Error,
    },
    /// A release file is not a regular file or exceeds its byte bound;
    /// nothing past the bound was read.
    Oversized {
        /// The file.
        path: PathBuf,
        /// The byte bound.
        limit: u64,
    },
    /// An artifact failed authentication against the trusted manifest.
    Rejected {
        /// The artifact kind the host requested.
        kind: ArtifactKind,
        /// The artifact file the host read (or would have read).
        artifact: PathBuf,
        /// The manifest the artifact was checked against.
        manifest: PathBuf,
        /// The authentication failure.
        error: ArtifactError,
    },
}

impl std::fmt::Display for ReleaseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read { path, error } => write!(
                formatter,
                "cannot read the release file `{}`: {error}\n  = help: {RELEASE_HELP}",
                path.display()
            ),
            Self::Oversized { path, limit } => write!(
                formatter,
                "the release file `{}` must be a regular file of at most {limit} bytes\n  = help: {RELEASE_HELP}",
                path.display()
            ),
            Self::Rejected {
                kind,
                artifact,
                manifest,
                error,
            } => write!(
                formatter,
                "component artifact rejected: the {} artifact: {error}\n  --> {}\n  = note: expected artifact `{}`\n  = help: {RELEASE_HELP}",
                kind.description(),
                manifest.display(),
                artifact.display(),
            ),
        }
    }
}

impl std::error::Error for ReleaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { error, .. } => Some(error),
            Self::Oversized { .. } | Self::Rejected { .. } => None,
        }
    }
}

impl From<ReleaseError> for HostError {
    fn from(error: ReleaseError) -> Self {
        Self::Release(error)
    }
}

/// Reads `path` only when it is a regular file of at most `limit` bytes,
/// reading at most `limit + 1` bytes, so a file that grows after the
/// metadata check is still rejected without being loaded.
///
/// # Errors
///
/// [`ReleaseError::Read`] when the file cannot be opened or read;
/// [`ReleaseError::Oversized`] when it is not a regular file or holds more
/// than `limit` bytes.
pub fn read_bounded_file(path: &Path, limit: u64) -> Result<Vec<u8>, ReleaseError> {
    let read_error = |error| ReleaseError::Read {
        path: path.to_path_buf(),
        error,
    };
    let oversized = || ReleaseError::Oversized {
        path: path.to_path_buf(),
        limit,
    };
    let file = std::fs::File::open(path).map_err(read_error)?;
    let metadata = file.metadata().map_err(read_error)?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(oversized());
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| oversized())?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(read_error)?;
    if u64::try_from(bytes.len()).map_or(true, |read| read > limit) {
        return Err(oversized());
    }
    Ok(bytes)
}

/// The eight authenticated components one session host links, loaded from
/// one release directory.
pub struct ReleaseComponents {
    machine: AuthenticatedComponent,
    compiler: AuthenticatedComponent,
    data_model_ecmascript: AuthenticatedComponent,
    data_model_xpath: AuthenticatedComponent,
    data_model_null: AuthenticatedComponent,
    io_scxml: AuthenticatedComponent,
    io_basichttp: AuthenticatedComponent,
    clock: AuthenticatedComponent,
}

impl std::fmt::Debug for ReleaseComponents {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReleaseComponents")
            .finish_non_exhaustive()
    }
}

impl ReleaseComponents {
    /// Reads `dir/manifest.tsv` (at most [`MAX_RELEASE_MANIFEST_BYTES`]) and
    /// each session component's `<name>.virtualized.wasm` (at most
    /// [`MAX_AUTHENTICATED_COMPONENT_BYTES`]) through
    /// [`read_bounded_file`], and authenticates every artifact against
    /// `trusted`. Reads nothing else.
    ///
    /// # Errors
    ///
    /// [`HostError::Release`] naming the file and the failure: an unreadable
    /// or oversized file, or an artifact the manifest does not authenticate
    /// (for example one missing from the manifest).
    pub fn load(dir: &Path, trusted: TrustedManifestDigest) -> Result<Self, HostError> {
        let manifest_path = dir.join(MANIFEST_FILE);
        let manifest = read_bounded_file(&manifest_path, MAX_RELEASE_MANIFEST_BYTES)?;
        let component_limit = u64::try_from(MAX_AUTHENTICATED_COMPONENT_BYTES).unwrap_or(u64::MAX);
        let load = |kind: ArtifactKind| -> Result<AuthenticatedComponent, HostError> {
            let file = kind.artifact_file();
            let path = dir.join(&file);
            let bytes = read_bounded_file(&path, component_limit)?;
            authenticate_component(&manifest, trusted, kind, &file, &bytes).map_err(|error| {
                match error {
                    HostError::Artifact(error) => ReleaseError::Rejected {
                        kind,
                        artifact: path,
                        manifest: manifest_path.clone(),
                        error,
                    }
                    .into(),
                    other => other,
                }
            })
        };
        Ok(Self {
            machine: load(ArtifactKind::Machine)?,
            compiler: load(ArtifactKind::Compiler)?,
            data_model_ecmascript: load(ArtifactKind::DataModelEcmascript)?,
            data_model_xpath: load(ArtifactKind::DataModelXpath)?,
            data_model_null: load(ArtifactKind::DataModelNull)?,
            io_scxml: load(ArtifactKind::IoProcessorScxml)?,
            io_basichttp: load(ArtifactKind::IoProcessorBasicHttp)?,
            clock: load(ArtifactKind::Clock)?,
        })
    }

    /// The components as one session host links them.
    #[must_use]
    pub fn session_components(&self) -> SessionHostComponents<'_> {
        SessionHostComponents {
            machine: &self.machine,
            compiler: &self.compiler,
            data_model_ecmascript: &self.data_model_ecmascript,
            data_model_xpath: &self.data_model_xpath,
            data_model_null: &self.data_model_null,
            io_scxml: &self.io_scxml,
            io_basichttp: &self.io_basichttp,
            clock: &self.clock,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("scxml-host-release-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("the scratch directory is creatable");
        dir
    }

    #[test]
    fn bounded_reads_accept_the_bound_and_reject_one_byte_more() {
        let dir = scratch("bounded");
        let path = dir.join("file");
        std::fs::write(&path, [7_u8; 4]).expect("the file is writable");
        assert_eq!(
            read_bounded_file(&path, 4).expect("four bytes fit a four-byte bound"),
            vec![7_u8; 4]
        );
        assert!(matches!(
            read_bounded_file(&path, 3),
            Err(ReleaseError::Oversized { limit: 3, .. })
        ));
        assert!(matches!(
            read_bounded_file(&dir, 1_024),
            Err(ReleaseError::Oversized { .. })
        ));
        assert!(matches!(
            read_bounded_file(&dir.join("absent"), 1_024),
            Err(ReleaseError::Read { .. })
        ));
        std::fs::remove_dir_all(&dir).expect("the scratch directory is removable");
    }

    #[test]
    fn a_missing_artifact_names_its_kind_manifest_and_how_to_build_it() {
        let error = ReleaseError::Rejected {
            kind: ArtifactKind::IoProcessorBasicHttp,
            artifact: PathBuf::from("release/scxml_ioprocessor_basichttp.virtualized.wasm"),
            manifest: PathBuf::from("release/manifest.tsv"),
            error: ArtifactError::ArtifactIdentity,
        };
        assert_eq!(
            error.to_string(),
            format!(
                "component artifact rejected: the BasicHTTP Event I/O Processor artifact: \
the release manifest does not list this artifact exactly once\n  --> release/manifest.tsv\n  \
= note: expected artifact `release/scxml_ioprocessor_basichttp.virtualized.wasm`\n  = help: \
{RELEASE_HELP}"
            )
        );
    }

    #[test]
    fn a_manifest_without_an_artifact_is_rejected_with_its_paths() {
        use sha2::Digest as _;
        let dir = scratch("identity");
        // A canonical manifest that lists only the clock: every other
        // session component is absent from it.
        let clock = b"clock";
        let manifest = format!(
            "schema\tstateforward.wasm-artifacts/v1\nprofile\trelease\ntarget\twasm32-wasip2\nartifact-count\t1\nwasm-tools\twasm-tools 1.244.0\nwasi-virt\twasi-virt 0.2.0\nartifact\tscxml_clock\tscxml_clock.virtualized.wasm\t{}\t{}\n",
            clock.len(),
            crate::artifact::lower_hex(&sha2::Sha256::digest(clock)),
        );
        std::fs::write(dir.join(MANIFEST_FILE), &manifest).expect("the manifest is writable");
        std::fs::write(dir.join("scxml_machine.virtualized.wasm"), b"machine")
            .expect("the artifact is writable");
        let trusted = TrustedManifestDigest::from_hex(&crate::artifact::lower_hex(
            &sha2::Sha256::digest(manifest.as_bytes()),
        ))
        .expect("the digest is canonical");
        let error = ReleaseComponents::load(&dir, trusted).expect_err("the machine is not listed");
        let HostError::Release(ReleaseError::Rejected {
            kind,
            artifact,
            manifest: manifest_path,
            error,
        }) = error
        else {
            panic!("expected a rejected artifact, got {error}");
        };
        assert_eq!(kind, ArtifactKind::Machine);
        assert_eq!(artifact, dir.join("scxml_machine.virtualized.wasm"));
        assert_eq!(manifest_path, dir.join(MANIFEST_FILE));
        assert_eq!(error, ArtifactError::ArtifactIdentity);
        std::fs::remove_dir_all(&dir).expect("the scratch directory is removable");
    }
}
