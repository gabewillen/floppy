//! Drift tests for the canonical explain table.

use std::collections::BTreeSet;

use super::*;

/// Every source file that constructs diagnostics with a literal stable code.
const PRODUCER_SOURCES: &[(&str, &str)] = &[
    ("validator-core/lib.rs", include_str!("../lib.rs")),
    ("validator-core/semantic.rs", include_str!("../semantic.rs")),
    ("validator-core/graph.rs", include_str!("../graph.rs")),
    (
        "validator-core/datamodel_attributes.rs",
        include_str!("../datamodel_attributes.rs"),
    ),
    (
        "validator-core/foreign_namespaces.rs",
        include_str!("../foreign_namespaces.rs"),
    ),
    (
        "validator-core/datamodel_check.rs",
        include_str!("../datamodel_check.rs"),
    ),
    (
        "validator-core/datamodel_check/data_ids.rs",
        include_str!("../datamodel_check/data_ids.rs"),
    ),
    (
        "validator-core/datamodel_check/effects.rs",
        include_str!("../datamodel_check/effects.rs"),
    ),
    (
        "validator-core/datamodel_check/in_predicate.rs",
        include_str!("../datamodel_check/in_predicate.rs"),
    ),
    (
        "validator-core/datamodel_check/xpath.rs",
        include_str!("../datamodel_check/xpath.rs"),
    ),
    (
        "validator-core/datamodel_check/xpath/references.rs",
        include_str!("../datamodel_check/xpath/references.rs"),
    ),
    (
        "validator-core/script_check.rs",
        include_str!("../script_check.rs"),
    ),
    ("validator-core/lowering.rs", include_str!("../lowering.rs")),
    (
        "validator-core/env_attributes.rs",
        include_str!("../env_attributes.rs"),
    ),
    (
        "validator-core/event_attributes.rs",
        include_str!("../event_attributes.rs"),
    ),
    (
        "validator-core/event_payloads.rs",
        include_str!("../event_payloads.rs"),
    ),
    (
        "validator-core/package_validator.rs",
        include_str!("../package_validator.rs"),
    ),
    (
        "validator-core/invoke_types.rs",
        include_str!("../invoke_types.rs"),
    ),
    (
        "validator-core/text_direction.rs",
        include_str!("../text_direction.rs"),
    ),
    (
        "validator-core/xml_error.rs",
        include_str!("../xml_error.rs"),
    ),
    (
        "validator-xsd/lib.rs",
        include_str!("../../../scxml-validator-xsd/src/lib.rs"),
    ),
    (
        "compiler-core/lib.rs",
        include_str!("../../../scxml-compiler-core/src/lib.rs"),
    ),
    (
        "validator-component/lib.rs",
        include_str!("../../../../components/scxml-validator/src/lib.rs"),
    ),
];

fn codes_in(text: &str) -> BTreeSet<String> {
    let mut codes = BTreeSet::new();
    for prefix in [
        "\"SCXML",
        "\"XML",
        "\"XSD",
        "\"COMP",
        "\"PKG",
        "\"ENV",
        "\"EVENT",
        "\"DATAMODEL",
        "\"LINT",
        "\"LIMIT",
    ] {
        for (offset, _) in text.match_indices(prefix) {
            let rest = &text[offset + 1..];
            let end = rest.find('"').unwrap_or(0);
            let candidate = &rest[..end];
            let digits = candidate.trim_start_matches(char::is_alphabetic);
            if digits.len() == 3
                && digits.chars().all(|c| c.is_ascii_digit())
                && candidate != "SCXML999"
            {
                codes.insert(candidate.to_owned());
            }
        }
    }
    codes
}

#[test]
fn every_literal_producer_code_is_documented_and_no_entry_is_stale() {
    let mut produced = BTreeSet::new();
    for (name, text) in PRODUCER_SOURCES {
        for code in codes_in(text) {
            assert!(
                lookup(&code).is_some(),
                "{name} emits undocumented code {code}"
            );
            produced.insert(code);
        }
    }
    for entry in CODES.iter().filter(|entry| !entry.family) {
        let dynamic = entry.code.starts_with("XML");
        assert!(
            produced.contains(entry.code) || dynamic,
            "explain entry {} is not emitted by any producer",
            entry.code
        );
    }
}

#[test]
fn table_is_unique_complete_and_family_lookup_is_prefix_bounded() {
    let mut seen = BTreeSet::new();
    for entry in CODES {
        assert!(
            seen.insert(entry.code),
            "duplicate explain entry {}",
            entry.code
        );
        assert!(!entry.title.is_empty() && !entry.why.is_empty() && !entry.help.is_empty());
        assert!(matches!(entry.severity, "error" | "warning" | "note"));
        assert!(
            scxml_reporting::code_family(entry.code).is_some(),
            "{} is in no code family",
            entry.code
        );
        if scxml_reporting::code_family(entry.code) == Some("SCXML") {
            assert!(
                entry.spec.starts_with("SCXML 1.0 ")
                    && entry.url.starts_with(scxml_recommendation_url()),
                "{} cites no SCXML 1.0 section",
                entry.code
            );
        }
        assert_eq!(
            entry.spec.is_empty(),
            entry.url.is_empty(),
            "{}",
            entry.code
        );
    }
    assert_eq!(
        lookup("XSD-cvc-complex-type.4").map(|entry| entry.code),
        Some("XSD-cvc-complex-type.4")
    );
    assert_eq!(
        lookup("XSD-cvc-complex-type.2.4.1").map(|entry| entry.code),
        Some("XSD-cvc-complex-type.2.4")
    );
    assert_eq!(
        lookup("XSD-cvc-complex-type.5").map(|entry| entry.code),
        Some("XSD-cvc-complex-type")
    );
    assert_eq!(
        lookup("XSD-cvc-elt.10").map(|entry| entry.code),
        Some("XSD-cvc-elt")
    );
    assert!(lookup("XSD-cvc-eltx").is_none());
    assert!(lookup("SCXML999").is_none());
    assert!(scxml_recommendation_url().starts_with("https://"));
}

#[test]
fn validator_rules_document_embeds_the_generated_code_table() {
    const BEGIN: &str =
        "<!-- BEGIN GENERATED: diagnostic codes (scxml_reporting::explain::markdown_table) -->\n";
    const END: &str = "<!-- END GENERATED: diagnostic codes -->";
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/validator-rules.md");
    let document = std::fs::read_to_string(path).expect("docs/validator-rules.md must be readable");
    let start = document
        .find(BEGIN)
        .expect("docs must contain the generated-table begin marker");
    let end = document
        .find(END)
        .expect("docs must contain the generated-table end marker");
    let current = &document[start + BEGIN.len()..end];
    let expected = markdown_table();
    if current != expected {
        if std::env::var_os("SCXML_BLESS").is_some() {
            let updated = format!(
                "{}{expected}{}",
                &document[..start + BEGIN.len()],
                &document[end..]
            );
            std::fs::write(path, updated).expect("docs must be writable when blessing");
        } else {
            panic!(
                "docs/validator-rules.md code table drifted from explain::CODES; run `sh scripts/bless-fixtures.sh --validator-rules`"
            );
        }
    }
}

#[test]
fn explanations_have_no_doubled_or_stray_sentence_punctuation() {
    for entry in CODES {
        let text = explanation(entry);
        for line in text
            .lines()
            .filter(|line| line.starts_with("Why: ") || line.starts_with("How to fix: "))
        {
            for doubled in ["..", ".).", ";.", ",.", ":.", "?.", "!."] {
                assert!(
                    !line.ends_with(doubled),
                    "{}: `--explain` sentence ends with `{doubled}`: {line}",
                    entry.code
                );
            }
            assert!(
                line.ends_with('.') || line.ends_with(".)"),
                "{}: `--explain` sentence does not end as a sentence: {line}",
                entry.code
            );
        }
    }
}
