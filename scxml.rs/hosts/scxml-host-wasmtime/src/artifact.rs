//! Authenticated, bounded Component Model artifact loading.

use std::sync::Arc;

use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent};
use sha2::{Digest as _, Sha256};

use crate::{ArtifactError, HostError, component_size_admitted};

const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_MANIFEST_PATH_BYTES: usize = 4 * 1024;
const MANIFEST_HEADER_LINES: usize = 6;
const MAX_ARTIFACTS: usize = 15;

/// Closed component identities accepted from the deterministic artifact manifest.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ArtifactKind {
    /// Deterministic clock capability.
    Clock,
    /// ECMAScript data-model adapter.
    DataModelEcmascript,
    /// Null data-model adapter.
    DataModelNull,
    /// `XPath` data-model adapter.
    DataModelXpath,
    /// SCXML I/O processor.
    IoProcessorScxml,
    /// `BasicHTTP` Event I/O processor (imports only `http-transport`).
    IoProcessorBasicHttp,
    /// Example invoked-service component (`stateforward:scxml/invoked-service`).
    ServiceExample,
    /// Example extension component.
    ExtensionExample,
    /// Example Wasm script component (`stateforward:scxml/script`).
    ScriptExample,
    /// Validator component.
    Validator,
    /// Rustc-style reporter component.
    ReporterRustc,
    /// SARIF reporter component.
    ReporterSarif,
    /// Compiler component.
    Compiler,
    /// Machine component: the generic machine (runtime `ir-source` mode) or,
    /// via [`authenticate_packaged_machine`], a packaged `component-wasm`.
    Machine,
    /// Optional capability-free session snapshot codec (`encode` and `diff`
    /// for hosts without the native `scxml-snapshot` crate). The Wasmtime
    /// host links the crate natively and never instantiates it; the kind is
    /// known so a release manifest that lists it authenticates.
    SnapshotCodec,
    /// A caller's extension component
    /// (`stateforward:scxml/executable-content-extension`), authenticated by
    /// its exact SHA-256 through [`authenticate_extension_component`]. Never
    /// listed in a release manifest.
    ExtensionComponent,
    /// A caller's invoked-service component
    /// (`stateforward:scxml/invoked-service`), authenticated by its exact
    /// SHA-256 through [`authenticate_service_component`]. Never listed in a
    /// release manifest.
    ServiceComponent,
}

impl ArtifactKind {
    /// What the artifact is, for diagnostics (for example `BasicHTTP Event
    /// I/O Processor`).
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Clock => "clock",
            Self::DataModelEcmascript => "ECMAScript data model",
            Self::DataModelNull => "null data model",
            Self::DataModelXpath => "XPath data model",
            Self::IoProcessorScxml => "SCXML Event I/O Processor",
            Self::IoProcessorBasicHttp => "BasicHTTP Event I/O Processor",
            Self::ServiceExample => "invoked-service example",
            Self::ExtensionExample => "extension-namespace example",
            Self::ScriptExample => "script example",
            Self::Validator => "validator",
            Self::ReporterRustc => "rustc-style reporter",
            Self::ReporterSarif => "SARIF reporter",
            Self::Compiler => "compiler",
            Self::Machine => "machine",
            Self::SnapshotCodec => "session snapshot codec",
            Self::ExtensionComponent => "extension component",
            Self::ServiceComponent => "invoked-service component",
        }
    }

    /// The canonical release file name, `<name>.virtualized.wasm`.
    #[must_use]
    pub fn artifact_file(self) -> String {
        format!("{}.virtualized.wasm", self.manifest_name())
    }

    pub(crate) fn manifest_name(self) -> &'static str {
        match self {
            Self::Clock => "scxml_clock",
            Self::DataModelEcmascript => "scxml_data_model_ecmascript",
            Self::DataModelNull => "scxml_data_model_null",
            Self::DataModelXpath => "scxml_data_model_xpath",
            Self::IoProcessorScxml => "scxml_ioprocessor_scxml",
            Self::IoProcessorBasicHttp => "scxml_ioprocessor_basichttp",
            Self::ServiceExample => "scxml_service_example",
            Self::ExtensionExample => "scxml_extension_example",
            Self::ScriptExample => "scxml_script_example",
            Self::Validator => "scxml_validator",
            Self::ReporterRustc => "scxml_reporter_rustc",
            Self::ReporterSarif => "scxml_reporter_sarif",
            Self::Compiler => "scxml_compiler",
            Self::Machine => "scxml_machine",
            Self::SnapshotCodec => "scxml_snapshot_codec",
            // Never parsed from a manifest (see `parse`): a caller's extension
            // component is authenticated by its own digest.
            Self::ExtensionComponent => "scxml_extension",
            // Never parsed from a manifest either.
            Self::ServiceComponent => "scxml_service",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "scxml_clock" => Some(Self::Clock),
            "scxml_data_model_ecmascript" => Some(Self::DataModelEcmascript),
            "scxml_data_model_null" => Some(Self::DataModelNull),
            "scxml_data_model_xpath" => Some(Self::DataModelXpath),
            "scxml_ioprocessor_scxml" => Some(Self::IoProcessorScxml),
            "scxml_ioprocessor_basichttp" => Some(Self::IoProcessorBasicHttp),
            "scxml_service_example" => Some(Self::ServiceExample),
            "scxml_extension_example" => Some(Self::ExtensionExample),
            "scxml_script_example" => Some(Self::ScriptExample),
            "scxml_validator" => Some(Self::Validator),
            "scxml_reporter_rustc" => Some(Self::ReporterRustc),
            "scxml_reporter_sarif" => Some(Self::ReporterSarif),
            "scxml_compiler" => Some(Self::Compiler),
            "scxml_machine" => Some(Self::Machine),
            "scxml_snapshot_codec" => Some(Self::SnapshotCodec),
            _ => None,
        }
    }
}

/// Caller-owned trust anchor for one deterministic artifact manifest.
///
/// Construct this only from a digest delivered through the application's
/// authenticated release/configuration channel. Computing it from the same
/// untrusted manifest being loaded does not establish provenance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrustedManifestDigest([u8; 32]);

impl TrustedManifestDigest {
    /// Parses exactly 64 lowercase hexadecimal characters.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactError::InvalidTrustedDigest`] for any other input.
    pub fn from_hex(value: &str) -> Result<Self, ArtifactError> {
        parse_digest(value)
            .map(Self)
            .ok_or(ArtifactError::InvalidTrustedDigest)
    }
}

/// Caller-owned trust anchor for one packaged `component-wasm` artifact: the
/// exact SHA-256 of the component bytes.
///
/// Construct this only from a digest delivered through an authenticated
/// channel, for example the `sha256` of a canonical compiler report the
/// caller trusts. Computing it from the untrusted bytes being loaded does not
/// establish provenance.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TrustedComponentDigest([u8; 32]);

impl TrustedComponentDigest {
    /// The raw SHA-256 bytes.
    pub(crate) const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Parses exactly 64 lowercase hexadecimal characters.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactError::InvalidTrustedDigest`] for any other input.
    pub fn from_hex(value: &str) -> Result<Self, ArtifactError> {
        parse_digest(value)
            .map(Self)
            .ok_or(ArtifactError::InvalidTrustedDigest)
    }
}

/// Authenticates one packaged machine `component-wasm` without invoking
/// Wasmtime.
///
/// The bytes are bounded to the host's fixed pre-compilation ceiling,
/// [`crate::MAX_AUTHENTICATED_COMPONENT_BYTES`] (GitHub's per-release-asset
/// limit, 2 GiB − 1), and must hash exactly to `trusted_digest`. The result
/// has kind [`ArtifactKind::Machine`]; execution additionally requires that
/// it does not import `ir-source` (see `MachineIrSource::Packaged`).
/// Deterministic, performs no I/O, and copies accepted bytes into an owned
/// value.
///
/// # Errors
///
/// Fails closed with [`HostError::Artifact`] for oversized, empty, or
/// digest-mismatched bytes.
pub fn authenticate_packaged_machine(
    component_bytes: &[u8],
    trusted_digest: TrustedComponentDigest,
) -> Result<AuthenticatedComponent, HostError> {
    authenticate_by_digest(component_bytes, trusted_digest, ArtifactKind::Machine)
}

/// Authenticates one caller-supplied extension component
/// (`stateforward:scxml/executable-content-extension`) without invoking
/// Wasmtime.
///
/// The bytes are bounded to the host's fixed pre-compilation ceiling,
/// [`crate::MAX_AUTHENTICATED_COMPONENT_BYTES`] (GitHub's per-release-asset
/// limit, 2 GiB − 1), and must hash exactly to `trusted_digest`, which the
/// caller takes from an authenticated channel (for example its own
/// configuration or a release manifest it trusts). The result has kind
/// [`ArtifactKind::ExtensionComponent`]; the session host additionally checks
/// the component's world and its `namespace-uri` when it instantiates it.
/// Deterministic, performs no I/O, and copies accepted bytes into an owned
/// value.
///
/// # Errors
///
/// Fails closed with [`HostError::Artifact`] for oversized, empty, or
/// digest-mismatched bytes.
pub fn authenticate_extension_component(
    component_bytes: &[u8],
    trusted_digest: TrustedComponentDigest,
) -> Result<AuthenticatedComponent, HostError> {
    authenticate_by_digest(
        component_bytes,
        trusted_digest,
        ArtifactKind::ExtensionComponent,
    )
}

/// Authenticates one caller-supplied invoked-service component
/// (`stateforward:scxml/invoked-service`) without invoking Wasmtime, exactly
/// as [`authenticate_extension_component`]: bounded to
/// [`crate::MAX_AUTHENTICATED_COMPONENT_BYTES`] and hashing exactly to
/// `trusted_digest`. The result has kind [`ArtifactKind::ServiceComponent`];
/// the session host additionally checks the component's world and its
/// `type-uri` when it instantiates it for an invocation.
///
/// # Errors
///
/// Fails closed with [`HostError::Artifact`] for oversized, empty, or
/// digest-mismatched bytes.
pub fn authenticate_service_component(
    component_bytes: &[u8],
    trusted_digest: TrustedComponentDigest,
) -> Result<AuthenticatedComponent, HostError> {
    authenticate_by_digest(
        component_bytes,
        trusted_digest,
        ArtifactKind::ServiceComponent,
    )
}

fn authenticate_by_digest(
    component_bytes: &[u8],
    trusted_digest: TrustedComponentDigest,
    kind: ArtifactKind,
) -> Result<AuthenticatedComponent, HostError> {
    let span = scxml_telemetry::observe(
        Module::HostWasmtime,
        Operation::HostWasmtimeAuthenticateArtifact,
    );
    let result = if !component_size_admitted(component_bytes.len()) {
        Err(ArtifactError::ArtifactTooLarge.into())
    } else if Sha256::digest(component_bytes).as_slice() != trusted_digest.0 {
        Err(ArtifactError::ArtifactDigestMismatch.into())
    } else {
        Ok(AuthenticatedComponent {
            kind,
            digest: trusted_digest.0,
            bytes: Arc::from(component_bytes),
        })
    };
    if let Err(error) = result.as_ref() {
        span.event(SignalEvent::InputRejected);
        span.fail(artifact_error_category(error));
    } else {
        span.event(SignalEvent::InputAccepted);
    }
    result
}

/// Shared, immutable component bytes whose release manifest, identity, path, size, and
/// SHA-256 digest were authenticated before exposure.
///
/// Fields are private so public execution APIs cannot receive raw caller bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticatedComponent {
    kind: ArtifactKind,
    /// The authenticated SHA-256 of `bytes`.
    digest: [u8; 32],
    bytes: Arc<[u8]>,
}

/// Explicit public execution input distinguishing authenticated artifacts from
/// raw caller-controlled bytes.
pub enum ComponentSource<'a> {
    /// A component accepted by [`authenticate_component`].
    Authenticated(&'a AuthenticatedComponent),
    /// Raw bytes, which are always rejected before engine construction or
    /// Wasmtime compilation.
    Unauthenticated(&'a [u8]),
}

impl<'a> ComponentSource<'a> {
    pub(crate) fn authenticated(self) -> Result<&'a AuthenticatedComponent, HostError> {
        match self {
            Self::Authenticated(component) => Ok(component),
            Self::Unauthenticated(_) => Err(ArtifactError::Unauthenticated.into()),
        }
    }
}

impl AuthenticatedComponent {
    /// Returns the authenticated closed artifact identity.
    #[must_use]
    pub fn kind(&self) -> ArtifactKind {
        self.kind
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The SHA-256 of [`Self::bytes`], verified at authentication: the
    /// identity the compiled-component cache keys on.
    pub(crate) fn digest(&self) -> [u8; 32] {
        self.digest
    }

    /// The SHA-256 of the authenticated bytes as 64 lowercase hex digits:
    /// the release-artifact digest session snapshots record as component
    /// provenance. It is the digest verified at authentication; nothing is
    /// rehashed.
    #[must_use]
    pub fn sha256(&self) -> String {
        lower_hex(&self.digest)
    }
}

/// `bytes` as lowercase hexadecimal, two digits per byte: the one digest
/// rendering of this crate.
pub(crate) fn lower_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut hex = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        hex.push(char::from(DIGITS[usize::from(byte >> 4)]));
        hex.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    hex
}

struct ManifestEntry {
    kind: ArtifactKind,
    size: usize,
    digest: [u8; 32],
}

/// Authenticates one release component without invoking Wasmtime.
///
/// `manifest_bytes` are bounded to 64 KiB and authenticated against the caller's
/// trusted release digest. `artifact_path` must be the exact canonical relative
/// `<identity>.virtualized.wasm` filename recorded in that manifest. Artifact
/// bytes are bounded to [`crate::MAX_AUTHENTICATED_COMPONENT_BYTES`] (GitHub's
/// per-release-asset limit, 2 GiB − 1) and must match its exact declared size
/// and digest.
/// The caller owns file or network acquisition; this deterministic function
/// performs no I/O, compilation, instantiation, execution, or mutation. It
/// copies accepted artifact bytes into an owned immutable value.
///
/// # Errors
///
/// Fails closed with [`HostError::Artifact`] when the trust anchor, manifest,
/// path, identity, size, or artifact digest does not satisfy the contract.
pub fn authenticate_component(
    manifest_bytes: &[u8],
    trusted_digest: TrustedManifestDigest,
    requested_kind: ArtifactKind,
    artifact_path: &str,
    artifact_bytes: &[u8],
) -> Result<AuthenticatedComponent, HostError> {
    let span = scxml_telemetry::observe(
        Module::HostWasmtime,
        Operation::HostWasmtimeAuthenticateArtifact,
    );
    let result = authenticate_component_inner(
        manifest_bytes,
        trusted_digest,
        requested_kind,
        artifact_path,
        artifact_bytes,
    );
    if let Err(error) = result.as_ref() {
        span.event(SignalEvent::InputRejected);
        span.fail(artifact_error_category(error));
    } else {
        span.event(SignalEvent::InputAccepted);
    }
    result
}

fn authenticate_component_inner(
    manifest: &[u8],
    trusted_digest: TrustedManifestDigest,
    requested_kind: ArtifactKind,
    artifact_path: &str,
    artifact_bytes: &[u8],
) -> Result<AuthenticatedComponent, HostError> {
    if manifest.len() > MAX_MANIFEST_BYTES {
        return Err(ArtifactError::ManifestTooLarge.into());
    }
    if Sha256::digest(manifest).as_slice() != trusted_digest.0 {
        return Err(ArtifactError::ManifestDigestMismatch.into());
    }
    let manifest_text =
        std::str::from_utf8(manifest).map_err(|_| ArtifactError::ManifestMalformed)?;
    let entry = parse_manifest(manifest_text, requested_kind)?;
    let expected_path = format!("{}.virtualized.wasm", requested_kind.manifest_name());
    if artifact_path != expected_path || artifact_path.len() > MAX_MANIFEST_PATH_BYTES {
        return Err(ArtifactError::NonCanonicalPath.into());
    }
    if !component_size_admitted(artifact_bytes.len()) {
        return Err(ArtifactError::ArtifactTooLarge.into());
    }
    if artifact_bytes.len() != entry.size {
        return Err(ArtifactError::ArtifactSizeMismatch.into());
    }
    if Sha256::digest(artifact_bytes).as_slice() != entry.digest {
        return Err(ArtifactError::ArtifactDigestMismatch.into());
    }
    Ok(AuthenticatedComponent {
        kind: entry.kind,
        digest: entry.digest,
        bytes: Arc::from(artifact_bytes),
    })
}

fn parse_manifest(
    manifest: &str,
    requested_kind: ArtifactKind,
) -> Result<ManifestEntry, HostError> {
    if !manifest.ends_with('\n') || manifest.contains('\r') {
        return Err(ArtifactError::ManifestMalformed.into());
    }
    let lines = manifest.lines().collect::<Vec<_>>();
    let headers = [
        "schema\tstateforward.wasm-artifacts/v1",
        "profile\trelease",
        "target\twasm32-wasip2",
    ];
    if lines.get(..headers.len()) != Some(headers.as_slice()) {
        return Err(ArtifactError::ManifestMalformed.into());
    }
    let count = parse_header_count(lines.get(3).copied())?;
    if count == 0 || count > MAX_ARTIFACTS || lines.len() != MANIFEST_HEADER_LINES + count {
        return Err(ArtifactError::ManifestMalformed.into());
    }
    if lines.get(4) != Some(&"wasm-tools\twasm-tools 1.240.0")
        || lines.get(5) != Some(&"wasi-virt\twasi-virt 0.2.0")
    {
        return Err(ArtifactError::ManifestMalformed.into());
    }
    let mut previous_kind = None;
    let mut selected = None;
    for line in &lines[MANIFEST_HEADER_LINES..] {
        let entry = parse_entry(line)?;
        if previous_kind.is_some_and(|previous| previous >= entry.kind) {
            return Err(ArtifactError::ManifestMalformed.into());
        }
        previous_kind = Some(entry.kind);
        if entry.kind == requested_kind && selected.replace(entry).is_some() {
            return Err(ArtifactError::ArtifactIdentity.into());
        }
    }
    selected.ok_or_else(|| HostError::from(ArtifactError::ArtifactIdentity))
}

fn parse_header_count(line: Option<&str>) -> Result<usize, HostError> {
    line.and_then(|line| line.strip_prefix("artifact-count\t"))
        .filter(|value| !value.is_empty() && !value.starts_with('0'))
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or_else(|| HostError::from(ArtifactError::ManifestMalformed))
}

fn parse_entry(line: &str) -> Result<ManifestEntry, HostError> {
    let mut fields = line.split('\t');
    if fields.next() != Some("artifact") {
        return Err(ArtifactError::ManifestMalformed.into());
    }
    let kind = fields
        .next()
        .and_then(ArtifactKind::parse)
        .ok_or(ArtifactError::ManifestMalformed)?;
    let path_text = fields.next().ok_or(ArtifactError::ManifestMalformed)?;
    let size_text = fields.next().ok_or(ArtifactError::ManifestMalformed)?;
    let digest_text = fields.next().ok_or(ArtifactError::ManifestMalformed)?;
    if fields.next().is_some() || path_text.is_empty() || path_text.len() > MAX_MANIFEST_PATH_BYTES
    {
        return Err(ArtifactError::ManifestMalformed.into());
    }
    let expected_filename = format!("{}.virtualized.wasm", kind.manifest_name());
    if path_text != expected_filename {
        return Err(ArtifactError::NonCanonicalPath.into());
    }
    let size = parse_canonical_size(size_text)?;
    let digest = parse_digest(digest_text).ok_or(ArtifactError::ManifestMalformed)?;
    Ok(ManifestEntry { kind, size, digest })
}

fn parse_canonical_size(value: &str) -> Result<usize, HostError> {
    value
        .parse::<usize>()
        .ok()
        .filter(|size| component_size_admitted(*size))
        .filter(|size| size.to_string() == value)
        .ok_or_else(|| HostError::from(ArtifactError::ArtifactTooLarge))
}

fn parse_digest(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || !value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return None;
    }
    let mut digest = [0_u8; 32];
    let (pairs, remainder) = value.as_bytes().as_chunks::<2>();
    if !remainder.is_empty() {
        return None;
    }
    for (output, pair) in digest.iter_mut().zip(pairs) {
        let high = hex_nibble(pair[0])?;
        let low = hex_nibble(pair[1])?;
        *output = high.checked_mul(16)?.checked_add(low)?;
    }
    Some(digest)
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => value.checked_sub(b'0'),
        b'a'..=b'f' => value
            .checked_sub(b'a')
            .and_then(|digit| digit.checked_add(10)),
        _ => None,
    }
}

fn artifact_error_category(error: &HostError) -> ErrorCategory {
    match error {
        HostError::Wasmtime(_)
        | HostError::TelemetryShutdown
        | HostError::TelemetryInUse
        | HostError::Release(crate::ReleaseError::Read { .. })
        | HostError::Component(_) => ErrorCategory::Platform,
        HostError::Artifact(_)
        | HostError::InvalidLimit(_)
        | HostError::UnboundedExecution
        | HostError::TraceContext(_)
        | HostError::Release(_) => ErrorCategory::Validation,
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;
    use crate::MAX_AUTHENTICATED_COMPONENT_BYTES;

    /// The one size predicate admits exactly 1..=2 GiB − 1 (GitHub's
    /// per-release-asset limit). Every authentication path applies it to the
    /// slice length before reading a byte, so this boundary proof stands in
    /// for a 2 GiB slice no test allocates; the integration tests prove the
    /// wiring through the empty slice and a manifest that declares one byte
    /// over the ceiling.
    #[test]
    fn component_size_bound_is_the_github_release_asset_limit() {
        assert_eq!(MAX_AUTHENTICATED_COMPONENT_BYTES, 2_147_483_647);
        assert_eq!(
            u32::try_from(MAX_AUTHENTICATED_COMPONENT_BYTES + 1),
            Ok(1_u32 << 31),
            "the ceiling and one past it fit a 32-bit usize"
        );
        assert!(!component_size_admitted(0));
        assert!(component_size_admitted(1));
        assert!(component_size_admitted(MAX_AUTHENTICATED_COMPONENT_BYTES));
        assert!(!component_size_admitted(
            MAX_AUTHENTICATED_COMPONENT_BYTES + 1
        ));
        assert!(!component_size_admitted(usize::MAX));
    }

    /// A manifest size field admits the ceiling itself and rejects one past
    /// it, without any artifact bytes.
    #[test]
    fn manifest_size_field_admits_exactly_the_ceiling() {
        let ceiling = MAX_AUTHENTICATED_COMPONENT_BYTES.to_string();
        assert_eq!(
            parse_canonical_size(&ceiling).ok(),
            Some(MAX_AUTHENTICATED_COMPONENT_BYTES)
        );
        for over in [
            (MAX_AUTHENTICATED_COMPONENT_BYTES + 1).to_string(),
            "0".to_owned(),
        ] {
            assert!(matches!(
                parse_canonical_size(&over),
                Err(HostError::Artifact(ArtifactError::ArtifactTooLarge))
            ));
        }
    }

    const ALL: [ArtifactKind; MAX_ARTIFACTS] = [
        ArtifactKind::Clock,
        ArtifactKind::DataModelEcmascript,
        ArtifactKind::DataModelNull,
        ArtifactKind::DataModelXpath,
        ArtifactKind::IoProcessorScxml,
        ArtifactKind::IoProcessorBasicHttp,
        ArtifactKind::ServiceExample,
        ArtifactKind::ExtensionExample,
        ArtifactKind::ScriptExample,
        ArtifactKind::Validator,
        ArtifactKind::ReporterRustc,
        ArtifactKind::ReporterSarif,
        ArtifactKind::Compiler,
        ArtifactKind::Machine,
        ArtifactKind::SnapshotCodec,
    ];

    /// The release manifest shape `verify-wasm-capabilities.sh` writes, one
    /// entry per kind in `kinds`, each artifact being its own name's bytes.
    fn manifest(kinds: &[ArtifactKind]) -> String {
        let mut text = format!(
            "schema\tstateforward.wasm-artifacts/v1\nprofile\trelease\ntarget\twasm32-wasip2\nartifact-count\t{}\nwasm-tools\twasm-tools 1.240.0\nwasi-virt\twasi-virt 0.2.0\n",
            kinds.len()
        );
        for kind in kinds {
            let name = kind.manifest_name();
            writeln!(
                text,
                "artifact\t{name}\t{name}.virtualized.wasm\t{}\t{}",
                name.len(),
                super::lower_hex(&Sha256::digest(name.as_bytes()))
            )
            .expect("writing to a string cannot fail");
        }
        text
    }

    fn authenticate(
        manifest: &str,
        kind: ArtifactKind,
    ) -> Result<AuthenticatedComponent, HostError> {
        let trusted = TrustedManifestDigest::from_hex(&super::lower_hex(&Sha256::digest(
            manifest.as_bytes(),
        )))
        .expect("digest is canonical");
        let name = kind.manifest_name();
        authenticate_component(
            manifest.as_bytes(),
            trusted,
            kind,
            &format!("{name}.virtualized.wasm"),
            name.as_bytes(),
        )
    }

    #[test]
    fn a_release_manifest_listing_the_snapshot_codec_authenticates_every_kind() {
        let text = manifest(&ALL);
        for kind in ALL {
            let component = authenticate(&text, kind).expect("every listed kind authenticates");
            assert_eq!(component.kind(), kind);
            assert_eq!(
                component.sha256(),
                super::lower_hex(&Sha256::digest(kind.manifest_name().as_bytes()))
            );
        }
        assert_eq!(
            ArtifactKind::parse("scxml_snapshot_codec"),
            Some(ArtifactKind::SnapshotCodec)
        );
    }

    #[test]
    fn the_codec_is_optional_and_ordered_after_the_machine() {
        let without = manifest(&ALL[..MAX_ARTIFACTS - 1]);
        authenticate(&without, ArtifactKind::Machine).expect("the codec is optional");
        assert!(matches!(
            authenticate(&without, ArtifactKind::SnapshotCodec),
            Err(HostError::Artifact(ArtifactError::ArtifactIdentity))
        ));
        let mut misordered = ALL;
        misordered.swap(MAX_ARTIFACTS - 2, MAX_ARTIFACTS - 1);
        assert!(matches!(
            authenticate(&manifest(&misordered), ArtifactKind::Clock),
            Err(HostError::Artifact(ArtifactError::ManifestMalformed))
        ));
    }

    #[test]
    fn sha256_is_lowercase_hex_of_the_bytes() {
        let bytes = b"abc";
        let component = authenticate_packaged_machine(
            bytes,
            TrustedComponentDigest::from_hex(&super::lower_hex(&Sha256::digest(bytes)))
                .expect("digest is canonical"),
        )
        .expect("the digest matches");
        assert_eq!(
            component.sha256(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
