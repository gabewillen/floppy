//! Raw message text of an event carried by the SCXML Event I/O Processor
//! (SCXML 5.10.1 `_event.raw`, C.1).
//!
//! SCXML leaves the SCXML processor's message format to the platform. This
//! processor's message is one `application/x-www-form-urlencoded` line, the
//! same field encoding the `BasicHTTP` processor (C.2) sends:
//!
//! 1. `_scxmleventname=<event name>`;
//! 2. one `<name>=<value>` field per `<param>` and `namelist` entry, in
//!    document order and **with duplicates kept** (SCXML 6.2.4: "all
//!    attributes and values provided by param and/or namelist even if
//!    duplicates occur"; official case 178), or one `_scxmlcontent=<value>`
//!    field for a `<content>` payload.
//!
//! A value is rendered by [`field_text`]. Names and values are
//! percent-encoded: ASCII alphanumerics and `*-._` are kept, space is `+`,
//! every other UTF-8 byte is `%XX` (uppercase hex). The rendering is a pure
//! function of its inputs, so the text is byte-deterministic.

use scxml_types::{DataValue, EventName};

/// Form field that carries the event name.
pub const RAW_EVENT_NAME_FIELD: &str = "_scxmleventname";
/// Form field that carries a `<content>` payload.
pub const RAW_CONTENT_FIELD: &str = "_scxmlcontent";

/// The payload an SCXML Event I/O Processor message carries.
#[derive(Clone, Copy, Debug)]
pub enum RawPayload<'a> {
    /// No payload.
    None,
    /// `<param>` and `namelist` entries in document order, duplicates kept.
    Params(&'a [(String, DataValue)]),
    /// A `<content>` payload.
    Content(&'a DataValue),
}

/// Renders the raw message text of one event this processor carries. It
/// has no byte bound (SCXML sets none).
#[must_use]
pub fn raw_message(event: &EventName, payload: RawPayload<'_>) -> String {
    let mut raw = String::new();
    append_field(&mut raw, RAW_EVENT_NAME_FIELD, event.as_str());
    match payload {
        RawPayload::None => {}
        RawPayload::Params(fields) => {
            for (name, value) in fields {
                append_field(&mut raw, name, &field_text(value));
            }
        }
        RawPayload::Content(value) => {
            append_field(&mut raw, RAW_CONTENT_FIELD, &field_text(value));
        }
    }
    raw
}

/// Text of one field value: a string is itself; `null`, booleans, and
/// numbers are their JSON text; JSON arrays and objects are their canonical
/// JSON text; XML is its markup; an absent or undefined value is empty.
#[must_use]
pub fn field_text(value: &DataValue) -> String {
    match value {
        DataValue::Absent | DataValue::Undefined => String::new(),
        DataValue::Null => "null".to_owned(),
        DataValue::Bool(value) => value.to_string(),
        DataValue::Number(value) => value.get().to_string(),
        DataValue::String(value)
        | DataValue::JsonArray(value)
        | DataValue::JsonObject(value)
        | DataValue::Xml(value) => value.clone(),
    }
}

/// Appends `&name=value` (no `&` before the first field).
fn append_field(raw: &mut String, name: &str, value: &str) {
    if !raw.is_empty() {
        raw.push('&');
    }
    encode_into(raw, name);
    raw.push('=');
    encode_into(raw, value);
}

fn encode_into(raw: &mut String, text: &str) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'*' | b'-' | b'.' | b'_') {
            raw.push(char::from(byte));
        } else if byte == b' ' {
            raw.push('+');
        } else {
            let high = HEX.get(usize::from(byte >> 4)).copied().unwrap_or(b'0');
            let low = HEX.get(usize::from(byte & 0x0F)).copied().unwrap_or(b'0');
            raw.push('%');
            raw.push(char::from(high));
            raw.push(char::from(low));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scxml_types::FiniteNumber;

    fn name(value: &str) -> EventName {
        EventName::new(value.to_owned()).expect("valid test event name")
    }

    fn number(value: f64) -> DataValue {
        DataValue::Number(FiniteNumber::new(value).expect("finite test number"))
    }

    #[test]
    fn duplicate_params_are_kept_in_document_order() {
        let fields = [
            ("Var1".to_owned(), number(2.0)),
            ("Var1".to_owned(), number(3.0)),
        ];
        assert_eq!(
            raw_message(&name("event1"), RawPayload::Params(&fields)),
            "_scxmleventname=event1&Var1=2&Var1=3"
        );
    }

    #[test]
    fn every_value_kind_renders_deterministically_and_is_form_encoded() {
        let fields = [
            ("a b".to_owned(), DataValue::String("x&y=z é".into())),
            ("n".to_owned(), DataValue::Null),
            ("t".to_owned(), DataValue::Bool(true)),
            ("f".to_owned(), number(1.5)),
            ("o".to_owned(), DataValue::JsonObject(r#"{"k":[1]}"#.into())),
            ("u".to_owned(), DataValue::Undefined),
        ];
        assert_eq!(
            raw_message(&name("e.x"), RawPayload::Params(&fields)),
            "_scxmleventname=e.x&a+b=x%26y%3Dz+%C3%A9&n=null&t=true&f=1.5&o=%7B%22k%22%3A%5B1%5D%7D&u="
        );
        assert_eq!(
            raw_message(
                &name("e"),
                RawPayload::Content(&DataValue::String("hi".into()))
            ),
            "_scxmleventname=e&_scxmlcontent=hi"
        );
        assert_eq!(
            raw_message(&name("e"), RawPayload::None),
            "_scxmleventname=e"
        );
    }

    #[test]
    fn raw_message_has_no_byte_bound() {
        let framing = "_scxmleventname=e&_scxmlcontent=".len();
        let large = DataValue::String("a".repeat(4 * 65_536));
        assert_eq!(
            raw_message(&name("e"), RawPayload::Content(&large)).len(),
            framing + 4 * 65_536
        );
    }
}
