//! Checked-in, digest-bound corrections of upstream W3C IRP defects.
//!
//! An errata file names, per data model, the official cases whose upstream
//! template or `.txml` source is defective and the minimal textual correction
//! to the transformation input, together with the SHA-256 of the exact
//! upstream-produced output and of the corrected output. It also names the
//! cases the W3C itself declares not applicable to that data model.
//!
//! Contract:
//! - preconditions: the file is a bounded regular UTF-8 JSON file whose schema
//!   version is [`ERRATA_VERSION`]; unknown fields fail closed;
//! - postconditions: a returned [`LoadedErrata`] has unique erratum ids, each
//!   case appears at most once across corrections and not-applicable
//!   declarations, every digest is a lowercase SHA-256, and every correction
//!   has 1..=[`MAX_REPLACEMENTS`](crate::errata::MAX_REPLACEMENTS) non-empty find strings;
//! - application ([`apply_replacements`]) is deterministic: each find string
//!   must occur exactly once in the current text and is replaced in order;
//!   anything else fails closed so drifted inputs are never silently patched.
//!
//! Errata correct the official TEST inputs only; they never change how an
//! implementation is judged. Preparation additionally binds each correction
//! to the exact upstream output digest, so an upstream change fails closed.

use std::collections::BTreeSet;
use std::fmt;
use std::fs::File;
use std::io::Read as _;
use std::path::Path;

use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};
use serde::{Deserialize, Serialize};

use crate::manifest::{ErrataIdentity, ErratumStage, NotApplicable, sha256_bytes};

/// Supported errata file schema version.
pub const ERRATA_VERSION: u32 = 1;
/// Maximum accepted errata file size.
pub const MAX_ERRATA_FILE_BYTES: u64 = 1024 * 1024;
/// Maximum corrections plus not-applicable declarations in one file.
pub const MAX_ERRATA_ENTRIES: usize = 256;
/// Maximum ordered replacements in one correction.
pub const MAX_REPLACEMENTS: usize = 16;
/// Maximum UTF-8 bytes of one errata text field.
pub const MAX_ERRATA_TEXT_BYTES: usize = 16 * 1024;
/// Maximum evidence citations for one entry.
pub const MAX_EVIDENCE_ITEMS: usize = 16;
/// Maximum size of a corrected transformation input.
pub const MAX_CORRECTED_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Checked-in errata for one data-model specialization of the IRP corpus.
pub struct ErrataFile {
    /// Schema version; must equal [`ERRATA_VERSION`].
    pub errata_version: u32,
    /// Data model whose prepared corpus these errata govern.
    pub datamodel: String,
    /// SHA-256 of the unmodified official template the errata were authored
    /// against; preparation fails closed on any other template.
    pub template_sha256: String,
    /// Documented corrections of defective upstream inputs.
    pub errata: Vec<Erratum>,
    /// Cases the W3C declares not applicable to this data model.
    pub not_applicable: Vec<NotApplicableCase>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One documented correction of one official case.
pub struct Erratum {
    /// Stable erratum identifier (lowercase ASCII letters, digits, hyphens).
    pub id: String,
    /// Official case identifier the correction applies to.
    pub case: String,
    /// Transformation input the correction edits.
    pub stage: ErratumStage,
    /// Upstream defect description.
    pub defect: String,
    /// Exact template or source line citations proving the defect.
    pub evidence: Vec<String>,
    /// Description of the minimal correction and its specification basis.
    pub correction: String,
    /// Ordered exact-text replacements applied to the transformation input.
    pub replacements: Vec<Replacement>,
    /// SHA-256 of the output the unmodified upstream inputs produce.
    pub upstream_output_sha256: String,
    /// SHA-256 of the output the corrected inputs produce.
    pub corrected_output_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One exact-text replacement; `find` must occur exactly once.
pub struct Replacement {
    /// Exact upstream text to replace.
    pub find: String,
    /// Corrected text.
    pub replace: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One case the W3C declares not applicable to the errata's data model.
pub struct NotApplicableCase {
    /// Official case identifier.
    pub case: String,
    /// The W3C instruction that makes the case not applicable.
    pub reason: String,
    /// Exact template or manifest citations quoting that instruction.
    pub evidence: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// A validated errata file with its exact byte identity.
pub struct LoadedErrata {
    /// Validated errata content.
    pub file: ErrataFile,
    /// Portable file name recorded as provenance.
    pub source: String,
    /// SHA-256 of the exact file bytes.
    pub digest: String,
    /// The exact file bytes; preparation writes them into the prepared
    /// corpus beside its manifest, where the loader rehashes them.
    pub bytes: Vec<u8>,
}

impl LoadedErrata {
    /// The suite errata identity these errata produce in a prepared corpus:
    /// every correction by case and every not-applicable case, in canonical
    /// case order.
    #[must_use]
    pub fn identity(&self) -> ErrataIdentity {
        let mut not_applicable: Vec<String> = self
            .file
            .not_applicable
            .iter()
            .map(|declaration| declaration.case.clone())
            .collect();
        not_applicable.sort_by(|left, right| case_order_key(left).cmp(&case_order_key(right)));
        ErrataIdentity {
            source: self.source.clone(),
            digest: self.digest.clone(),
            applied: self
                .file
                .errata
                .iter()
                .map(|erratum| (erratum.case.clone(), erratum.id.clone()))
                .collect(),
            not_applicable,
        }
    }

    /// The not-applicable record a prepared case carries for `case_id`:
    /// the W3C reason followed by its evidence citations.
    #[must_use]
    pub fn not_applicable_record(&self, case_id: &str) -> Option<NotApplicable> {
        self.not_applicable_for(case_id)
            .map(|declaration| NotApplicable {
                reason: format!(
                    "{} Evidence: {}",
                    declaration.reason,
                    declaration.evidence.join("; ")
                ),
            })
    }

    #[must_use]
    /// Returns the correction for `case_id`, if any.
    pub fn erratum_for(&self, case_id: &str) -> Option<&Erratum> {
        self.file
            .errata
            .iter()
            .find(|erratum| erratum.case == case_id)
    }

    #[must_use]
    /// Returns the not-applicable declaration for `case_id`, if any.
    pub fn not_applicable_for(&self, case_id: &str) -> Option<&NotApplicableCase> {
        self.file
            .not_applicable
            .iter()
            .find(|declaration| declaration.case == case_id)
    }

    /// Returns every case id this file names, for unknown-case checks.
    pub fn case_ids(&self) -> impl Iterator<Item = &str> {
        self.file
            .errata
            .iter()
            .map(|erratum| erratum.case.as_str())
            .chain(
                self.file
                    .not_applicable
                    .iter()
                    .map(|declaration| declaration.case.as_str()),
            )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// A bounded errata loading, validation, or application failure.
pub struct ErrataError {
    /// Stable machine-readable failure code.
    pub code: &'static str,
    /// Human-readable failure description.
    pub message: String,
}

impl ErrataError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for ErrataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for ErrataError {}

/// Reads and validates one errata file.
///
/// # Errors
///
/// Returns [`ErrataError`] when the file is unavailable, not a bounded regular
/// file, not UTF-8 JSON of the supported schema, or violates any invariant in
/// the module contract.
pub fn load_errata(path: &Path) -> Result<LoadedErrata, ErrataError> {
    let span = observe(Module::ConformancePrepare, Operation::PrepareLoadErrata);
    let result = read_errata(path);
    match &result {
        Ok(_) => span.event(SignalEvent::InputAccepted),
        Err(_) => span.fail(ErrorCategory::InvalidInput),
    }
    result
}

/// Reads and validates one errata file without a telemetry boundary of its
/// own: [`load_errata`] (preparation) and the prepared-suite loader
/// ([`crate::manifest::load_suite`]) each call it under their own span.
///
/// # Errors
///
/// As [`load_errata`].
pub(crate) fn read_errata(path: &Path) -> Result<LoadedErrata, ErrataError> {
    {
        let file = File::open(path).map_err(|error| {
            ErrataError::new(
                "errata-read",
                format!("cannot open errata `{}`: {error}", path.display()),
            )
        })?;
        let metadata = file.metadata().map_err(|error| {
            ErrataError::new(
                "errata-read",
                format!("cannot inspect errata `{}`: {error}", path.display()),
            )
        })?;
        if !metadata.is_file() || metadata.len() > MAX_ERRATA_FILE_BYTES {
            return Err(ErrataError::new(
                "errata-size",
                format!("errata must be a regular file of at most {MAX_ERRATA_FILE_BYTES} bytes"),
            ));
        }
        let mut bytes = Vec::new();
        file.take(MAX_ERRATA_FILE_BYTES.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| {
                ErrataError::new(
                    "errata-read",
                    format!("cannot read errata `{}`: {error}", path.display()),
                )
            })?;
        if u64::try_from(bytes.len()).map_or(true, |length| length > MAX_ERRATA_FILE_BYTES) {
            return Err(ErrataError::new(
                "errata-size",
                format!("errata exceed {MAX_ERRATA_FILE_BYTES} bytes"),
            ));
        }
        let file: ErrataFile = serde_json::from_slice(&bytes).map_err(|error| {
            ErrataError::new("errata-json", format!("cannot parse errata: {error}"))
        })?;
        validate_errata(&file)?;
        let source = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty() && name.len() <= MAX_ERRATA_TEXT_BYTES)
            .ok_or_else(|| {
                ErrataError::new("errata-name", "errata path must end in a UTF-8 file name")
            })?
            .to_owned();
        Ok(LoadedErrata {
            file,
            source,
            digest: sha256_bytes(&bytes),
            bytes,
        })
    }
}

/// Validates every errata invariant in the module contract.
///
/// # Errors
///
/// Returns [`ErrataError`] naming the first violated invariant.
pub fn validate_errata(file: &ErrataFile) -> Result<(), ErrataError> {
    if file.errata_version != ERRATA_VERSION {
        return Err(ErrataError::new(
            "errata-version",
            format!(
                "unsupported errata version {}; expected {ERRATA_VERSION}",
                file.errata_version
            ),
        ));
    }
    require_text("datamodel", &file.datamodel)?;
    require_digest("template_sha256", &file.template_sha256)?;
    if file.errata.len().saturating_add(file.not_applicable.len()) > MAX_ERRATA_ENTRIES {
        return Err(ErrataError::new(
            "errata-count",
            format!("errata exceed {MAX_ERRATA_ENTRIES} entries"),
        ));
    }
    let mut cases = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for erratum in &file.errata {
        if erratum.id.is_empty()
            || erratum.id.len() > MAX_ERRATA_TEXT_BYTES
            || !erratum
                .id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(ErrataError::new(
                "errata-id",
                "erratum ids must be non-empty lowercase ASCII letters, digits, and hyphens",
            ));
        }
        if !ids.insert(erratum.id.as_str()) {
            return Err(ErrataError::new(
                "errata-duplicate-id",
                format!("erratum id `{}` is repeated", erratum.id),
            ));
        }
        require_case(&mut cases, &erratum.case)?;
        require_text("defect", &erratum.defect)?;
        require_text("correction", &erratum.correction)?;
        require_evidence(&erratum.evidence)?;
        if erratum.replacements.is_empty() || erratum.replacements.len() > MAX_REPLACEMENTS {
            return Err(ErrataError::new(
                "errata-replacements",
                format!(
                    "erratum `{}` requires 1 to {MAX_REPLACEMENTS} replacements",
                    erratum.id
                ),
            ));
        }
        for replacement in &erratum.replacements {
            require_text("replacement find", &replacement.find)?;
            if replacement.replace.len() > MAX_ERRATA_TEXT_BYTES
                || replacement.find == replacement.replace
            {
                return Err(ErrataError::new(
                    "errata-replacements",
                    format!(
                        "erratum `{}` replacements must be bounded and change the text",
                        erratum.id
                    ),
                ));
            }
        }
        require_digest("upstream_output_sha256", &erratum.upstream_output_sha256)?;
        require_digest("corrected_output_sha256", &erratum.corrected_output_sha256)?;
        if erratum.upstream_output_sha256 == erratum.corrected_output_sha256 {
            return Err(ErrataError::new(
                "errata-digests",
                format!(
                    "erratum `{}` corrected output must differ from the upstream output",
                    erratum.id
                ),
            ));
        }
    }
    for declaration in &file.not_applicable {
        require_case(&mut cases, &declaration.case)?;
        require_text("not-applicable reason", &declaration.reason)?;
        require_evidence(&declaration.evidence)?;
    }
    Ok(())
}

/// Applies ordered exact-text replacements to UTF-8 input bytes.
///
/// # Errors
///
/// Returns `erratum-input-utf8` for non-UTF-8 input, `erratum-patch-mismatch`
/// when a find string does not occur exactly once in the current text, and
/// `erratum-size` when the result exceeds [`MAX_CORRECTED_BYTES`].
pub fn apply_replacements(
    input: &[u8],
    replacements: &[Replacement],
) -> Result<Vec<u8>, ErrataError> {
    let mut text = std::str::from_utf8(input)
        .map_err(|error| {
            ErrataError::new(
                "erratum-input-utf8",
                format!("erratum input is not UTF-8: {error}"),
            )
        })?
        .to_owned();
    for (index, replacement) in replacements.iter().enumerate() {
        let occurrences = text.match_indices(replacement.find.as_str()).count();
        if occurrences != 1 {
            return Err(ErrataError::new(
                "erratum-patch-mismatch",
                format!(
                    "replacement {index} find text occurs {occurrences} times; exactly one is required"
                ),
            ));
        }
        text = text.replacen(replacement.find.as_str(), &replacement.replace, 1);
        if text.len() > MAX_CORRECTED_BYTES {
            return Err(ErrataError::new(
                "erratum-size",
                format!("corrected input exceeds {MAX_CORRECTED_BYTES} bytes"),
            ));
        }
    }
    Ok(text.into_bytes())
}

fn require_text(field: &str, value: &str) -> Result<(), ErrataError> {
    if value.trim().is_empty() || value.len() > MAX_ERRATA_TEXT_BYTES {
        return Err(ErrataError::new(
            "errata-text",
            format!("errata {field} must be non-blank and at most {MAX_ERRATA_TEXT_BYTES} bytes"),
        ));
    }
    Ok(())
}

fn require_digest(field: &str, value: &str) -> Result<(), ErrataError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ErrataError::new(
            "errata-digest",
            format!("errata {field} must be a lowercase SHA-256"),
        ));
    }
    Ok(())
}

fn require_evidence(evidence: &[String]) -> Result<(), ErrataError> {
    if evidence.is_empty() || evidence.len() > MAX_EVIDENCE_ITEMS {
        return Err(ErrataError::new(
            "errata-evidence",
            format!("errata entries require 1 to {MAX_EVIDENCE_ITEMS} evidence citations"),
        ));
    }
    for citation in evidence {
        require_text("evidence", citation)?;
    }
    Ok(())
}

/// Canonical case order: numeric prefix, then the full identifier.
fn case_order_key(id: &str) -> (u64, &str) {
    (crate::filter::numeric_case_id(id).unwrap_or(u64::MAX), id)
}

fn require_case<'a>(cases: &mut BTreeSet<&'a str>, case: &'a str) -> Result<(), ErrataError> {
    require_text("case", case)?;
    if !cases.insert(case) {
        return Err(ErrataError::new(
            "errata-duplicate-case",
            format!("case `{case}` appears in more than one errata entry"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replacement(find: &str, replace: &str) -> Replacement {
        Replacement {
            find: find.into(),
            replace: replace.into(),
        }
    }

    fn file() -> ErrataFile {
        ErrataFile {
            errata_version: ERRATA_VERSION,
            datamodel: "xpath".into(),
            template_sha256: "a".repeat(64),
            errata: vec![Erratum {
                id: "xpath-520".into(),
                case: "520".into(),
                stage: ErratumStage::Template,
                defect: "unquoted literal".into(),
                evidence: vec!["xpath.xsl:373".into()],
                correction: "quote it".into(),
                replacements: vec![replacement("a", "b")],
                upstream_output_sha256: "b".repeat(64),
                corrected_output_sha256: "c".repeat(64),
            }],
            not_applicable: vec![NotApplicableCase {
                case: "302".into(),
                reason: "scripting".into(),
                evidence: vec!["xpath.xsl:631".into()],
            }],
        }
    }

    #[test]
    fn replacements_apply_deterministically_in_order() {
        let replacements = [replacement("one", "two"), replacement("two three", "done")];
        let first = apply_replacements(b"one three", &replacements).expect("patch must apply");
        let second = apply_replacements(b"one three", &replacements).expect("patch must apply");
        assert_eq!(first, b"done");
        assert_eq!(first, second, "application must be byte-deterministic");
    }

    #[test]
    fn replacements_fail_closed_on_missing_or_repeated_find_text() {
        let missing = apply_replacements(b"abc", &[replacement("x", "y")])
            .expect_err("absent find text must fail closed");
        assert_eq!(missing.code, "erratum-patch-mismatch");
        let repeated = apply_replacements(b"aa", &[replacement("a", "b")])
            .expect_err("repeated find text must fail closed");
        assert_eq!(repeated.code, "erratum-patch-mismatch");
        let invalid = apply_replacements(&[0xff], &[replacement("a", "b")])
            .expect_err("non-UTF-8 input must fail closed");
        assert_eq!(invalid.code, "erratum-input-utf8");
    }

    #[test]
    fn validation_accepts_a_well_formed_file() {
        validate_errata(&file()).expect("well-formed errata must validate");
    }

    #[test]
    fn validation_rejects_duplicates_bad_ids_versions_and_digests() {
        let mut duplicate_case = file();
        duplicate_case.not_applicable[0].case = "520".into();
        assert_eq!(
            validate_errata(&duplicate_case)
                .expect_err("a case named twice must be rejected")
                .code,
            "errata-duplicate-case"
        );
        let mut bad_id = file();
        bad_id.errata[0].id = "XPath 520".into();
        assert_eq!(
            validate_errata(&bad_id)
                .expect_err("non-canonical id must be rejected")
                .code,
            "errata-id"
        );
        let mut version = file();
        version.errata_version = ERRATA_VERSION + 1;
        assert_eq!(
            validate_errata(&version)
                .expect_err("unknown version must be rejected")
                .code,
            "errata-version"
        );
        let mut same = file();
        same.errata[0].corrected_output_sha256 = same.errata[0].upstream_output_sha256.clone();
        assert_eq!(
            validate_errata(&same)
                .expect_err("a no-op correction must be rejected")
                .code,
            "errata-digests"
        );
        let mut no_evidence = file();
        no_evidence.errata[0].evidence.clear();
        assert_eq!(
            validate_errata(&no_evidence)
                .expect_err("evidence is mandatory")
                .code,
            "errata-evidence"
        );
    }

    #[test]
    fn decoding_rejects_unknown_fields_and_stages() {
        let mut value = serde_json::to_value(file()).expect("errata must encode");
        value["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ErrataFile>(value).is_err());
        let mut value = serde_json::to_value(file()).expect("errata must encode");
        value["errata"][0]["stage"] = serde_json::json!("output");
        assert!(serde_json::from_value::<ErrataFile>(value).is_err());
    }
}
