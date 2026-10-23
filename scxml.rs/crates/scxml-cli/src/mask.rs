//! Masking of the run's `--env` and `--env-file` values in everything the CLI
//! writes (defense in depth: components never receive a value, so one
//! reaches output only if a document or a service echoes it).
//!
//! The values are installed once, at the composition root, before anything
//! runs ([`install`]); afterwards every text the run writes passes through
//! [`scrub`] (notes, diagnostics, and error messages), which replaces each
//! value, and its JSON-escaped form, with [`MASK`], or through
//! [`scrub_json`] (live-session records), which masks them only inside the
//! record's JSON strings: a value is masked wherever a string holds it, but
//! never in a literal or a number, so a value such as `true` or `8100` leaves
//! every record one JSON line. A
//! canonical session snapshot document is written exactly: its `sha256`
//! covers its bytes, so it is never masked, and it gets the protections of
//! the canonical artifacts it is (it holds data-model values). A value shorter
//! than [`MIN_MASKED_BYTES`] is not masked (it would mask ordinary text), nor
//! is a value that is byte for byte one of the bound service packages'
//! declared `http-origins`: that string is public (the package manifest declares it,
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
    let _ = VALUES.set(forms(values, public));
}

/// Each value to mask and its JSON-escaped form, the longest first.
fn forms(values: &[(String, String)], public: &[String]) -> Vec<String> {
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
    forms
}

/// `text` with every installed value masked.
pub(crate) fn scrub(text: &str) -> Cow<'_, str> {
    scrub_with(VALUES.get().map_or(&[], Vec::as_slice), text)
}

fn scrub_with<'a>(values: &[String], text: &'a str) -> Cow<'a, str> {
    let mut text = Cow::Borrowed(text);
    for value in values {
        if text.contains(value.as_str()) {
            text = Cow::Owned(text.replace(value.as_str(), MASK));
        }
    }
    text
}

/// `line`, one JSON value, with every installed value masked inside each of
/// its strings (keys included) and nothing else changed: a string holding a
/// value is decoded, masked, and encoded again, so the mask never splits an
/// escape.
#[cfg(feature = "wasmtime")]
pub(crate) fn scrub_json(line: &str) -> Cow<'_, str> {
    scrub_json_with(VALUES.get().map_or(&[], Vec::as_slice), line)
}

#[cfg(feature = "wasmtime")]
fn scrub_json_with<'a>(values: &[String], line: &'a str) -> Cow<'a, str> {
    if values.is_empty() {
        return Cow::Borrowed(line);
    }
    let mut masked = String::new();
    let mut copied = 0;
    let mut tokens = line.char_indices();
    while let Some((start, character)) = tokens.next() {
        if character != '"' {
            continue;
        }
        let mut end = None;
        while let Some((index, character)) = tokens.next() {
            match character {
                '\\' => {
                    tokens.next();
                }
                '"' => {
                    end = Some(index + 1);
                    break;
                }
                _ => {}
            }
        }
        // Not one JSON value: masked as text, which may cost its shape but
        // never shows a value.
        let Some(end) = end else {
            return Cow::Owned(scrub_with(values, line).into_owned());
        };
        let Ok(decoded) = serde_json::from_str::<String>(&line[start..end]) else {
            return Cow::Owned(scrub_with(values, line).into_owned());
        };
        if let Cow::Owned(scrubbed) = scrub_with(values, &decoded) {
            masked.push_str(&line[copied..start]);
            masked.push_str(&serde_json::Value::String(scrubbed).to_string());
            copied = end;
        }
    }
    if copied == 0 {
        return Cow::Borrowed(line);
    }
    masked.push_str(&line[copied..]);
    Cow::Owned(masked)
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

    #[cfg(feature = "wasmtime")]
    #[test]
    fn json_values_are_masked_only_inside_strings() {
        let values = forms(
            &["true", "1234", "sk-secret-1", "a\"b\\c-long", "nabc"]
                .map(|value| (String::new(), value.to_owned())),
            &[],
        );
        let scrub_json = |line| scrub_json_with(&values, line);
        assert!(matches!(scrub_json(r#"{"a":"nothing"}"#), Cow::Borrowed(_)));
        assert!(matches!(scrub_json(r#"{"a":true}"#), Cow::Borrowed(_)));
        assert_eq!(
            scrub_json(r#"{"type":"error","fatal":true,"count":1234.0,"text":"is true"}"#),
            r#"{"type":"error","fatal":true,"count":1234.0,"text":"is [masked]"}"#,
            "literals and numbers are never masked"
        );
        assert_eq!(
            scrub_json(r#"{"sk-secret-1":"x","v":"a\"b\\c-long","e":"\nabc"}"#),
            r#"{"[masked]":"x","v":"[masked]","e":"\nabc"}"#,
            "keys and escaped values are masked; an escape is never split"
        );
        assert_eq!(
            scrub_json(r#"{"text":"open sk-secret-1"#),
            r#"{"text":"open [masked]"#,
            "a line that is not JSON is masked as text"
        );
    }
}
