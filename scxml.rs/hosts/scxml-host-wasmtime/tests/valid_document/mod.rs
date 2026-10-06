//! The check that a measured or pinned document is one `compile` accepts:
//! the bundled W3C SCXML schema and the semantic and data-model rules, as
//! `scx validate` runs them (expression syntax aside, which the data
//! model checks again when it runs the document).

use scxml_compiler_core::has_errors;
use scxml_syntax::SourceId;
use scxml_validator_core::{MemorySchemaResolver, Validator};
use scxml_validator_xsd::{BundledSchemaResolver, XsdSchemaBackend};

/// The source id of the validated document.
const SOURCE: SourceId = SourceId::new(1);

/// Panics with the diagnostics unless `source` validates without errors.
pub fn assert_valid(name: &str, source: &str) {
    let outcome = Validator::new(
        BundledSchemaResolver::new(MemorySchemaResolver::new()),
        XsdSchemaBackend,
    )
    .validate_source(SOURCE, name, source);
    assert!(
        !has_errors(&outcome.report),
        "{name} must validate: {:?}",
        outcome
            .report
            .diagnostics()
            .iter()
            .map(|diagnostic| (&diagnostic.code, &diagnostic.message))
            .collect::<Vec<_>>()
    );
}
