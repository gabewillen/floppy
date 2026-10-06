//! External `<script src>` contracts shared by the compiler, the packager,
//! and hosts (SCXML 1.0 section 5.8).
//!
//! SCXML 1.0 section 5.8 gives `<script>` either a `src` URI or inline
//! content, never both; the data model defines the scripting language, and
//! a `src` that cannot be fetched or parsed at document load rejects the
//! document. This crate owns the single loader contract both resolution
//! paths share:
//!
//! * **compile time** — the compiler receives script packages (bytes, media
//!   type, expected SHA-256) in its request and pins each reference;
//! * **run time** — a host's embedder-supplied `script-loader` resolves the
//!   URI to bytes and a media type for dynamic (for example agent-authored)
//!   SCXML; browsers use only this path.
//!
//! Both paths call [`verify_script`], which dispatches on the media type:
//! [`WASM_MEDIA_TYPE`] must be a Component Model component implementing the
//! `stateforward:scxml/script` world ([`validate_script_component`]); the
//! active data model's own script language (see
//! [`data_model_script_media_types`]) must be bounded UTF-8 text; anything
//! else fails closed with [`ScriptError::UnsupportedMediaType`].
//!
//! # Contract
//!
//! * Every function is pure, deterministic, bounded, and side-effect free
//!   apart from closed-label telemetry; inputs are borrowed and never
//!   retained.
//! * Media types compare byte-for-byte against lowercase registered names;
//!   parameters (`;charset=...`), other spellings, and unknown types are
//!   rejected rather than normalized.
//! * The world check is by exact import/export name: a script component
//!   imports only names in [`SCRIPT_ALLOWED_IMPORTS`] and exports exactly
//!   [`SCRIPT_API_INTERFACE`]. Function signatures are enforced where the
//!   component is linked (a host's typed export lookup fails closed on any
//!   signature mismatch).
//! * Telemetry: one span per call with the static `script` module label and
//!   a closed operation; URIs, media types, digests, byte counts, and
//!   payloads never become telemetry attributes.
//!
//! # Example
//!
//! Authenticate script bytes against the digest pinned at compile time:
//!
//! ```
//! use scxml_script::{ScriptError, ScriptKind, sha256_hex, verify_script};
//!
//! let source = b"var doubled = 2 * input;";
//! let pin = sha256_hex(source);
//! let verified = verify_script(
//!     "scripts/double.js",
//!     "text/javascript",
//!     source,
//!     Some("ecmascript"),
//!     Some(&pin),
//! )?;
//! assert_eq!(verified.kind, ScriptKind::DataModelText);
//! assert_eq!(verified.sha256, pin);
//!
//! // Tampered bytes, and Wasm bytes that are not a script component, fail closed.
//! let tampered = verify_script(
//!     "scripts/double.js",
//!     "text/javascript",
//!     b"var doubled = 3 * input;",
//!     Some("ecmascript"),
//!     Some(&pin),
//! );
//! assert_eq!(tampered, Err(ScriptError::DigestMismatch));
//! let not_component = verify_script("double.wasm", "application/wasm", b"\0asm", None, None);
//! assert_eq!(not_component, Err(ScriptError::NotAComponent));
//! # Ok::<(), ScriptError>(())
//! ```

#![forbid(unsafe_code)]

use std::fmt;

use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};
use sha2::{Digest as _, Sha256};

/// Media type of a Wasm script component.
pub const WASM_MEDIA_TYPE: &str = "application/wasm";
/// WIT world every Wasm script component implements.
pub const SCRIPT_WORLD: &str = "stateforward:scxml/script@0.2.0";
/// The single interface a Wasm script component exports.
pub const SCRIPT_API_INTERFACE: &str = "stateforward:scxml/script-api@0.2.0";
/// Interface a packaged machine exports when it bundles script components.
pub const SCRIPT_BUNDLE_INTERFACE: &str = "stateforward:scxml/script-bundle@0.2.0";
/// Imports a Wasm script component may declare, sorted. The host links
/// `data-model-host` to the session's active data model.
pub const SCRIPT_ALLOWED_IMPORTS: [&str; 3] = [
    "stateforward:scxml/data-model-host@0.2.0",
    "stateforward:scxml/telemetry@0.2.0",
    "stateforward:scxml/types@0.2.0",
];
/// Maximum UTF-8 bytes of one `src` URI.
pub const MAX_SCRIPT_URI_BYTES: usize = 16 * 1024;
/// Maximum bytes of one Wasm script component (4 MiB), on every build: the
/// component's bytes are authenticated and validated before anything links
/// them.
pub const MAX_SCRIPT_COMPONENT_BYTES: usize = 4 * 1024 * 1024;
/// Maximum bytes of one data-model-language script, equal to the bundled
/// data-model providers' script bound (wasm builds only; unbounded
/// natively, see [`scxml_types::wasm_bound`]).
pub const MAX_SCRIPT_TEXT_BYTES: usize = scxml_types::wasm_bound(64 * 1024);
/// Maximum script packages accepted by one compile request.
pub const MAX_SCRIPT_PACKAGES: usize = 64;
/// Maximum aggregate script package bytes accepted by one compile request.
pub const MAX_TOTAL_SCRIPT_BYTES: usize = 16 * 1024 * 1024;
/// Media types the bundled ECMAScript data model executes as script text.
pub const ECMASCRIPT_MEDIA_TYPES: [&str; 4] = [
    "application/ecmascript",
    "application/javascript",
    "text/ecmascript",
    "text/javascript",
];

/// How a resolved external script executes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScriptKind {
    /// A Component Model component implementing [`SCRIPT_WORLD`].
    WasmComponent,
    /// Source text in the active data model's own script language.
    DataModelText,
}

/// Closed external-script failure. Messages are static and never echo input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScriptError {
    /// The `src` URI is empty, over [`MAX_SCRIPT_URI_BYTES`], or contains
    /// whitespace-only or control characters.
    InvalidUri,
    /// The media type is neither [`WASM_MEDIA_TYPE`] nor a script language
    /// of the active data model.
    UnsupportedMediaType,
    /// The bytes are empty or exceed the bound for their kind.
    TooLarge,
    /// A digest is not 64 lowercase hexadecimal characters.
    InvalidDigest,
    /// The bytes do not hash to the pinned SHA-256.
    DigestMismatch,
    /// [`WASM_MEDIA_TYPE`] bytes are not a valid Component Model component
    /// (for example a core module or arbitrary bytes).
    NotAComponent,
    /// The component's imports or exports differ from [`SCRIPT_WORLD`].
    WorldMismatch,
    /// Data-model-language script bytes are not UTF-8.
    InvalidText,
}

impl ScriptError {
    /// Static, payload-free description (at most 160 UTF-8 bytes).
    #[must_use]
    pub const fn detail(self) -> &'static str {
        match self {
            Self::InvalidUri => "script src URI is empty, oversized, or malformed",
            Self::UnsupportedMediaType => {
                "script media type is not a Wasm component or the data model's script language"
            }
            Self::TooLarge => "script bytes are empty or exceed their bound",
            Self::InvalidDigest => "script digest is not lowercase SHA-256 hex",
            Self::DigestMismatch => "script bytes do not match the pinned SHA-256",
            Self::NotAComponent => "script bytes are not a valid Wasm component",
            Self::WorldMismatch => "script component does not implement the script world",
            Self::InvalidText => "script text is not UTF-8",
        }
    }

    /// Stable kebab-case reason used in diagnostics and host errors.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::InvalidUri => "invalid-uri",
            Self::UnsupportedMediaType => "unsupported-media-type",
            Self::TooLarge => "too-large",
            Self::InvalidDigest => "invalid-digest",
            Self::DigestMismatch => "digest-mismatch",
            Self::NotAComponent => "not-a-component",
            Self::WorldMismatch => "world-mismatch",
            Self::InvalidText => "invalid-text",
        }
    }

    const fn category(self) -> ErrorCategory {
        match self {
            Self::InvalidUri | Self::TooLarge | Self::InvalidDigest => ErrorCategory::InvalidInput,
            Self::UnsupportedMediaType => ErrorCategory::Unsupported,
            Self::DigestMismatch
            | Self::NotAComponent
            | Self::WorldMismatch
            | Self::InvalidText => ErrorCategory::Validation,
        }
    }
}

impl fmt::Display for ScriptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.detail())
    }
}

impl std::error::Error for ScriptError {}

/// Result of authenticating one resolved external script.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedScript {
    /// How the script executes.
    pub kind: ScriptKind,
    /// Lowercase hex SHA-256 of the exact bytes.
    pub sha256: String,
}

fn record<T>(span: &scxml_telemetry::FunctionSpan, result: &Result<T, ScriptError>) {
    match result {
        Ok(_) => span.event(SignalEvent::InputAccepted),
        Err(error) => {
            span.event(SignalEvent::InputRejected);
            span.fail(error.category());
        }
    }
}

/// Script-language media types of the named data model (the root
/// `datamodel` attribute value), or an empty list when it has no script
/// language this project executes from `src` text.
#[must_use]
pub fn data_model_script_media_types(data_model: Option<&str>) -> &'static [&'static str] {
    match data_model {
        Some("ecmascript") => &ECMASCRIPT_MEDIA_TYPES,
        _ => &[],
    }
}

/// Dispatches one media type (SCXML 1.0 section 5.8: the data model defines
/// the scripting language).
///
/// # Errors
///
/// [`ScriptError::UnsupportedMediaType`] for any other media type.
pub fn classify(media_type: &str, data_model: Option<&str>) -> Result<ScriptKind, ScriptError> {
    if media_type == WASM_MEDIA_TYPE {
        Ok(ScriptKind::WasmComponent)
    } else if data_model_script_media_types(data_model).contains(&media_type) {
        Ok(ScriptKind::DataModelText)
    } else {
        Err(ScriptError::UnsupportedMediaType)
    }
}

/// Checks one `src` URI: not blank in XML white space (#x20, #x9, #xD,
/// #xA; other Unicode white space is URI text), at most
/// [`MAX_SCRIPT_URI_BYTES`], and free of control characters. The URI is
/// otherwise opaque: it is never normalized or rewritten.
///
/// # Errors
///
/// [`ScriptError::InvalidUri`].
pub fn validate_uri(uri: &str) -> Result<(), ScriptError> {
    if uri.chars().all(scxml_types::is_xml_space)
        || uri.len() > MAX_SCRIPT_URI_BYTES
        || uri.chars().any(char::is_control)
    {
        Err(ScriptError::InvalidUri)
    } else {
        Ok(())
    }
}

/// Whether `value` is exactly 64 lowercase hexadecimal characters.
#[must_use]
pub fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// Lowercase hex SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Authenticates one resolved external script; the one check both the
/// compile-time and the runtime resolution paths run.
///
/// Checks, in order: URI, media-type dispatch, the byte bound of the kind
/// (empty is rejected), expected-digest syntax, exact SHA-256 identity when
/// `expected_sha256` is present, then UTF-8 text or
/// [`validate_script_component`].
///
/// # Errors
///
/// The first failing check as a closed [`ScriptError`].
pub fn verify_script(
    uri: &str,
    media_type: &str,
    bytes: &[u8],
    data_model: Option<&str>,
    expected_sha256: Option<&str>,
) -> Result<VerifiedScript, ScriptError> {
    let span = observe(Module::Script, Operation::ScriptVerify);
    let result = (|| {
        validate_uri(uri)?;
        let kind = classify(media_type, data_model)?;
        let too_large = match kind {
            ScriptKind::WasmComponent => bytes.len() > MAX_SCRIPT_COMPONENT_BYTES,
            ScriptKind::DataModelText => {
                scxml_types::exceeds_wasm_bound(bytes.len(), MAX_SCRIPT_TEXT_BYTES)
            }
        };
        if bytes.is_empty() || too_large {
            return Err(ScriptError::TooLarge);
        }
        if expected_sha256.is_some_and(|digest| !is_sha256_hex(digest)) {
            return Err(ScriptError::InvalidDigest);
        }
        let sha256 = sha256_hex(bytes);
        if expected_sha256.is_some_and(|digest| digest != sha256) {
            return Err(ScriptError::DigestMismatch);
        }
        match kind {
            ScriptKind::WasmComponent => validate_script_component(bytes)?,
            ScriptKind::DataModelText => {
                std::str::from_utf8(bytes).map_err(|_| ScriptError::InvalidText)?;
            }
        }
        Ok(VerifiedScript { kind, sha256 })
    })();
    record(&span, &result);
    result
}

/// Validates Wasm script component bytes against [`SCRIPT_WORLD`].
///
/// Checks: at most [`MAX_SCRIPT_COMPONENT_BYTES`], the component encoding
/// (a core module is rejected), full validation with the default Wasm
/// feature set, top-level imports a subset of [`SCRIPT_ALLOWED_IMPORTS`]
/// (no WASI or other ambient capability), and exactly one top-level export,
/// [`SCRIPT_API_INTERFACE`].
///
/// # Errors
///
/// [`ScriptError::TooLarge`], [`ScriptError::NotAComponent`], or
/// [`ScriptError::WorldMismatch`].
pub fn validate_script_component(bytes: &[u8]) -> Result<(), ScriptError> {
    let span = observe(Module::Script, Operation::ScriptValidateComponent);
    let result = (|| {
        if bytes.is_empty() || bytes.len() > MAX_SCRIPT_COMPONENT_BYTES {
            return Err(ScriptError::TooLarge);
        }
        if !wasmparser::Parser::is_component(bytes) {
            return Err(ScriptError::NotAComponent);
        }
        wasmparser::Validator::new_with_features(wasmparser::WasmFeatures::default())
            .validate_all(bytes)
            .map_err(|_| ScriptError::NotAComponent)?;
        let mut imports = Vec::new();
        let mut exports = Vec::new();
        // `parse_all` also yields nested module and component payloads; only
        // the outermost component (depth 1) is the public world. Depth is
        // bounded by the validated bytes.
        let mut depth = 0_usize;
        for payload in wasmparser::Parser::new(0).parse_all(bytes) {
            match payload.map_err(|_| ScriptError::NotAComponent)? {
                wasmparser::Payload::Version { .. } => depth = depth.saturating_add(1),
                wasmparser::Payload::End(_) => depth = depth.saturating_sub(1),
                wasmparser::Payload::ComponentImportSection(section) if depth == 1 => {
                    for import in section {
                        let import = import.map_err(|_| ScriptError::NotAComponent)?;
                        imports.push(import.name.name.to_owned());
                    }
                }
                wasmparser::Payload::ComponentExportSection(section) if depth == 1 => {
                    for export in section {
                        let export = export.map_err(|_| ScriptError::NotAComponent)?;
                        exports.push(export.name.name.to_owned());
                    }
                }
                _ => {}
            }
        }
        if imports
            .iter()
            .any(|import| !SCRIPT_ALLOWED_IMPORTS.contains(&import.as_str()))
            || exports != [SCRIPT_API_INTERFACE]
        {
            return Err(ScriptError::WorldMismatch);
        }
        Ok(())
    })();
    record(&span, &result);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_encoder::{
        Component, ComponentExportKind, ComponentExportSection, ComponentImportSection,
        ComponentInstanceSection, ComponentTypeRef, ComponentTypeSection, InstanceType,
    };

    /// A minimal component importing `imports` (empty instance types) and
    /// exporting one empty instance under each name in `exports`.
    fn component(imports: &[&str], exports: &[&str]) -> Vec<u8> {
        let mut component = Component::new();
        let mut types = ComponentTypeSection::new();
        types.instance(&InstanceType::new());
        component.section(&types);
        let mut import_section = ComponentImportSection::new();
        for name in imports {
            import_section.import(*name, ComponentTypeRef::Instance(0));
        }
        component.section(&import_section);
        let mut instances = ComponentInstanceSection::new();
        for _ in exports {
            instances.export_items(Vec::<(&str, ComponentExportKind, u32)>::new());
        }
        component.section(&instances);
        let base = u32::try_from(imports.len()).expect("test import count fits u32");
        let mut export_section = ComponentExportSection::new();
        for (index, name) in exports.iter().enumerate() {
            let index = u32::try_from(index).expect("test export count fits u32");
            export_section.export(*name, ComponentExportKind::Instance, base + index, None);
        }
        component.section(&export_section);
        component.finish()
    }

    #[test]
    fn script_world_component_is_accepted_and_pinned() {
        let bytes = component(&SCRIPT_ALLOWED_IMPORTS, &[SCRIPT_API_INTERFACE]);
        let digest = sha256_hex(&bytes);
        let verified = verify_script(
            "example.wasm",
            WASM_MEDIA_TYPE,
            &bytes,
            Some("ecmascript"),
            Some(&digest),
        )
        .expect("a script-world component verifies");
        assert_eq!(verified.kind, ScriptKind::WasmComponent);
        assert_eq!(verified.sha256, digest);
        // Imports are an allowlist: a script that needs no telemetry passes.
        let minimal = component(
            &[
                "stateforward:scxml/types@0.2.0",
                "stateforward:scxml/data-model-host@0.2.0",
            ],
            &[SCRIPT_API_INTERFACE],
        );
        assert_eq!(validate_script_component(&minimal), Ok(()));
    }

    #[test]
    fn hostile_scripts_fail_closed_with_typed_errors() {
        let good = component(&SCRIPT_ALLOWED_IMPORTS, &[SCRIPT_API_INTERFACE]);
        let digest = sha256_hex(&good);
        let verify = |uri: &str, media: &str, bytes: &[u8], pin: Option<&str>| {
            verify_script(uri, media, bytes, Some("ecmascript"), pin).map(|v| v.kind)
        };
        let mut tampered = good.clone();
        if let Some(last) = tampered.last_mut() {
            *last ^= 1;
        }
        assert_eq!(
            verify("a.wasm", WASM_MEDIA_TYPE, &tampered, Some(&digest)),
            Err(ScriptError::DigestMismatch)
        );
        assert_eq!(
            verify("a.wasm", "application/octet-stream", &good, None),
            Err(ScriptError::UnsupportedMediaType)
        );
        assert_eq!(
            verify("a.wasm", "Application/Wasm", &good, None),
            Err(ScriptError::UnsupportedMediaType),
            "media types are never case-folded"
        );
        assert_eq!(
            verify("a.wasm", WASM_MEDIA_TYPE, b"not wasm at all", None),
            Err(ScriptError::NotAComponent)
        );
        let core_module = wasm_encoder::Module::new().finish();
        assert_eq!(
            verify("a.wasm", WASM_MEDIA_TYPE, &core_module, None),
            Err(ScriptError::NotAComponent),
            "a core module is not a component"
        );
        let wasi = component(
            &[
                "wasi:cli/environment@0.2.0",
                "stateforward:scxml/types@0.2.0",
            ],
            &[SCRIPT_API_INTERFACE],
        );
        assert_eq!(
            verify("a.wasm", WASM_MEDIA_TYPE, &wasi, None),
            Err(ScriptError::WorldMismatch)
        );
        let wrong_export = component(&[], &["stateforward:scxml/machine-api@0.2.0"]);
        assert_eq!(
            verify("a.wasm", WASM_MEDIA_TYPE, &wrong_export, None),
            Err(ScriptError::WorldMismatch)
        );
        let oversized = vec![0_u8; MAX_SCRIPT_COMPONENT_BYTES + 1];
        assert_eq!(
            verify("a.wasm", WASM_MEDIA_TYPE, &oversized, None),
            Err(ScriptError::TooLarge)
        );
        assert_eq!(
            verify("a.wasm", WASM_MEDIA_TYPE, &[], None),
            Err(ScriptError::TooLarge)
        );
        assert_eq!(
            verify("a.wasm", WASM_MEDIA_TYPE, &good, Some("ABC")),
            Err(ScriptError::InvalidDigest)
        );
        for uri in ["", "  ", "a\u{0}b", &"x".repeat(MAX_SCRIPT_URI_BYTES + 1)] {
            assert_eq!(
                verify(uri, WASM_MEDIA_TYPE, &good, None),
                Err(ScriptError::InvalidUri)
            );
        }
    }

    /// A `src` is blank only in XML white space (#x20, #x9, #xD, #xA), as
    /// the browser host's mirror reads it: U+1680, U+00A0, and U+FEFF are
    /// URI text.
    #[test]
    fn a_uri_is_blank_only_in_xml_white_space() {
        for blank in ["", " \t\r\n"] {
            assert_eq!(
                validate_uri(blank),
                Err(ScriptError::InvalidUri),
                "{blank:?}"
            );
        }
        for text in ["\u{1680}", "\u{a0}", "\u{feff}"] {
            assert_eq!(validate_uri(text), Ok(()), "{text:?}");
        }
    }

    #[test]
    fn data_model_text_dispatches_by_the_data_models_language() {
        assert_eq!(
            verify_script(
                "lib.js",
                "text/javascript",
                b"x = 1;",
                Some("ecmascript"),
                None
            )
            .map(|verified| verified.kind),
            Ok(ScriptKind::DataModelText)
        );
        assert_eq!(
            classify("text/javascript", Some("xpath")),
            Err(ScriptError::UnsupportedMediaType),
            "a data model without a script language rejects script text"
        );
        assert_eq!(
            classify("text/javascript", None),
            Err(ScriptError::UnsupportedMediaType)
        );
        assert_eq!(
            classify(WASM_MEDIA_TYPE, Some("null")),
            Ok(ScriptKind::WasmComponent)
        );
        assert_eq!(
            verify_script(
                "lib.js",
                "text/javascript",
                &[0xff, 0xfe],
                Some("ecmascript"),
                None
            ),
            Err(ScriptError::InvalidText)
        );
        // Natively script text has no size bound.
        let large = vec![b' '; 65 * 1024];
        assert!(
            verify_script(
                "lib.js",
                "text/javascript",
                &large,
                Some("ecmascript"),
                None
            )
            .is_ok()
        );
    }

    /// A wasm build bounds script text at 64 KiB.
    #[cfg(target_arch = "wasm32")]
    #[test]
    fn wasm_only_script_text_over_its_bound_is_too_large() {
        let oversized = vec![b' '; MAX_SCRIPT_TEXT_BYTES + 1];
        assert_eq!(
            verify_script(
                "lib.js",
                "text/javascript",
                &oversized,
                Some("ecmascript"),
                None
            ),
            Err(ScriptError::TooLarge)
        );
    }

    #[test]
    fn error_details_are_static_bounded_and_distinct() {
        let all = [
            ScriptError::InvalidUri,
            ScriptError::UnsupportedMediaType,
            ScriptError::TooLarge,
            ScriptError::InvalidDigest,
            ScriptError::DigestMismatch,
            ScriptError::NotAComponent,
            ScriptError::WorldMismatch,
            ScriptError::InvalidText,
        ];
        let mut details = all.map(ScriptError::detail).to_vec();
        let mut reasons = all.map(ScriptError::reason).to_vec();
        assert!(details.iter().all(|detail| detail.len() <= 160));
        details.sort_unstable();
        details.dedup();
        reasons.sort_unstable();
        reasons.dedup();
        assert_eq!(details.len(), all.len());
        assert_eq!(reasons.len(), all.len());
    }
}
