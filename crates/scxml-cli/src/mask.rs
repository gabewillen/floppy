//! Masking of the run's `--env` and `--env-file` values in everything the CLI
//! writes (defense in depth: extension components never receive a value, so
//! one reaches output only if a document or a service echoes it).
//!
//! The values are installed once, at the composition root, before anything
//! runs ([`install`]); afterwards every text the run writes (live-session
//! records, notes, diagnostics, and error messages) passes through [`scrub`],
//! which replaces each value, and its JSON-escaped form, with [`MASK`]. A
//! canonical session snapshot document is written exactly: its `sha256`
//! covers its bytes, so it is never masked, and it gets the protections of
//! the canonical artifacts it is (it holds data-model values). A value shorter
//! than [`MIN_MASKED_BYTES`] is not masked (it would mask ordinary text), nor
//! is a value that is byte for byte one of the bound packages' declared
//! `http-origins`: that string is public (the package manifest declares it,
//! and the consent listing must show it), so masking it would hide what the
//! run consents to and protect nothing. The cost is bounded: at most the
//! number of values times the output length.

use std::borrow::Cow;
use std::sync::OnceLock;

/// What a masked value becomes.
pub(crate) const MASK: &str = "[masked]";
/// The shortest value that is masked.
pub(crate) const MIN_MASKED_BYTES: usize = 4;

static VALUES: OnceLock<Vec<String>> = OnceLock::new();

/// Installs the run's values, except any equal to one of `public` (the
/// bound packages' declared origins); only the first call has an effect.
pub(crate) fn install(values: &[(String, String)], public: &[String]) {
    let mut forms: Vec<String> = Vec::new();
    for (_, value) in values {
        if value.len() < MIN_MASKED_BYTES || public.contains(value) {
            continue;
        }
        forms.push(value.clone());
        let escaped = serde_json::Value::String(value.clone()).to_string();
        let escaped = escaped
            .strip_prefix('"')
            .and_then(|inner| inner.strip_suffix('"'))
            .unwrap_or(&escaped)
            .to_owned();
        if escaped != *value {
            forms.push(escaped);
        }
    }
    // The longest first, so a value containing another is masked whole.
    forms.sort_by_key(|form| std::cmp::Reverse(form.len()));
    forms.dedup();
    let _ = VALUES.set(forms);
}

/// `text` with every installed value masked.
pub(crate) fn scrub(text: &str) -> Cow<'_, str> {
    let Some(values) = VALUES.get().filter(|values| !values.is_empty()) else {
        return Cow::Borrowed(text);
    };
    let mut text = Cow::Borrowed(text);
    for value in values {
        if text.contains(value.as_str()) {
            text = Cow::Owned(text.replace(value.as_str(), MASK));
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_and_their_json_forms_are_masked() {
        install(
            &[
                ("KEY".into(), "sk-secret-1".into()),
                ("QUOTED".into(), "a\"b\\c-long".into()),
                ("SHORT".into(), "abc".into()),
                ("BASE".into(), "https://api.example".into()),
            ],
            &["https://api.example".to_owned()],
        );
        assert_eq!(scrub("key sk-secret-1 end"), "key [masked] end");
        assert_eq!(
            scrub(r#"{"v":"a\"b\\c-long"}"#),
            r#"{"v":"[masked]"}"#,
            "the JSON-escaped form is masked too"
        );
        assert_eq!(
            scrub("abc stays"),
            "abc stays",
            "short values are not masked"
        );
        assert_eq!(
            scrub("outbound HTTP: https://api.example"),
            "outbound HTTP: https://api.example",
            "a value that is a declared origin is public"
        );
        assert!(matches!(scrub("nothing"), Cow::Borrowed(_)));
    }
}
