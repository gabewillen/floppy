//! XML Schema white-space collapsing for attribute values.
//!
//! XML Schema 1.1 Part 2 §4.3.6 defines the `whiteSpace` facet. Its
//! `collapse` value applies to `xsd:token` and every type derived from it
//! (`NMTOKEN`, `NMTOKENS`, `Name`, `NCName`, `ID`, `IDREF`, `IDREFS`, and
//! enumerations restricted from them), to `xsd:anyURI`, and to the numeric
//! types such as `xsd:decimal`: "All occurrences of #x9 (tab), #xA (line
//! feed) and #xD (carriage return) are replaced with #x20 (space)", then
//! "contiguous sequences of #x20's are collapsed to a single #x20, and
//! initial and/or final #x20's are deleted". A value of such a type means
//! its collapsed form, and the items of a list type (`IDREFS`, `NMTOKENS`)
//! are separated by those four characters only.
//!
//! This module is the one definition the validator, the compiler's
//! lowering, and the conformance harness share, so all of them read the
//! same value from the same attribute.

use std::borrow::Cow;

pub use scxml_types::is_xml_space;

/// Whether `value` is only XML white space (or empty): text an XML
/// element may hold where it allows no character data.
#[must_use]
pub fn is_xml_blank(value: &str) -> bool {
    value.chars().all(is_xml_space)
}

/// The XML Schema `whiteSpace="collapse"` form of `value` (Part 2 §4.3.6),
/// borrowed when `value` is already collapsed.
#[must_use]
pub fn collapse_whitespace(value: &str) -> Cow<'_, str> {
    if !value.contains(['\t', '\n', '\r'])
        && !value.starts_with(' ')
        && !value.ends_with(' ')
        && !value.contains("  ")
    {
        return Cow::Borrowed(value);
    }
    Cow::Owned(xml_tokens(value).collect::<Vec<_>>().join(" "))
}

/// The items of an XML Schema list value (`IDREFS`, `NMTOKENS`, the SCXML
/// event-descriptor list): the non-empty runs between XML white space.
pub fn xml_tokens(value: &str) -> impl Iterator<Item = &str> {
    value.split(is_xml_space).filter(|token| !token.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{collapse_whitespace, is_xml_space, xml_tokens};
    use std::borrow::Cow;

    #[test]
    fn collapse_replaces_trims_and_joins_xml_space_only() {
        assert!(matches!(collapse_whitespace("a b"), Cow::Borrowed("a b")));
        assert_eq!(collapse_whitespace(" \ta\r\n  b \n"), "a b");
        assert_eq!(collapse_whitespace("   "), "");
        assert_eq!(collapse_whitespace("\u{a0}a\u{a0}"), "\u{a0}a\u{a0}");
    }

    #[test]
    fn list_items_split_on_xml_space_only() {
        assert_eq!(
            xml_tokens(" a\u{a0}b\tc ").collect::<Vec<_>>(),
            ["a\u{a0}b", "c"]
        );
        assert!(!is_xml_space('\u{a0}'));
        assert!(!is_xml_space('\u{2028}'));
    }
}
