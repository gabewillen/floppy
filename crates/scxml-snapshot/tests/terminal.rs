//! The human renderers never write untrusted document text to a terminal
//! raw: control, DEL, C1, line-separator, and bidi characters print as
//! visible escapes, and a multi-line value stays on its own line.
//!
//! `ansi_xml.json` and `ansi_sysname.json` are checksum-valid documents
//! whose XML data value, session name, and `_name` carry OSC title-set,
//! clear-screen, SGR, BEL, 8-bit CSI, and RLO/PDF sequences.

use scxml_snapshot::{Color, DiffOptions, decode, diff, render_diff, render_show};

const ANSI_XML: &[u8] = include_bytes!("fixtures/terminal/ansi_xml.json");
const ANSI_SYSNAME: &[u8] = include_bytes!("fixtures/terminal/ansi_sysname.json");

/// Characters a terminal interprets rather than displays.
fn assert_inert(rendered: &str) {
    for (index, character) in rendered.char_indices() {
        let forbidden = (character.is_control() && character != '\n')
            || matches!(
                character,
                '\u{007f}'
                    | '\u{0080}'..='\u{009f}'
                    | '\u{2028}'
                    | '\u{2029}'
                    | '\u{061c}'
                    | '\u{200e}'
                    | '\u{200f}'
                    | '\u{202a}'..='\u{202e}'
                    | '\u{2066}'..='\u{2069}'
            );
        assert!(
            !forbidden,
            "raw U+{:04X} at byte {index} in:\n{rendered}",
            u32::from(character)
        );
    }
}

#[test]
fn hostile_documents_decode_as_valid() {
    decode(ANSI_XML).expect("the xml probe is a valid document");
    decode(ANSI_SYSNAME).expect("the name probe is a valid document");
}

#[test]
fn show_escapes_control_and_bidi_characters() {
    let document = decode(ANSI_XML).expect("valid");
    let shown = render_show(&document, Color::Never);
    assert_inert(&shown);
    assert!(
        shown.contains(r"cart = xml \u{001B}]0;PWNED-TITLE\u{0007}\u{001B}[2J\u{001B}[31mFAKE"),
        "{shown}"
    );
    assert!(
        shown.contains(r"\u{009B}31m C1 \u{202E}evil\u{202C}"),
        "{shown}"
    );

    let document = decode(ANSI_SYSNAME).expect("valid");
    let shown = render_show(&document, Color::Never);
    assert_inert(&shown);
    assert!(shown.contains(r"name `x\u{001B}[31mR`"), "{shown}");
    assert!(shown.contains(r"_name `x\u{001B}[31mR`"), "{shown}");
}

#[test]
fn colored_show_emits_only_renderer_sgr_codes() {
    let document = decode(ANSI_XML).expect("valid");
    let shown = render_show(&document, Color::Always);
    let stripped = shown
        .replace("\u{1b}[0m", "")
        .replace("\u{1b}[1m", "")
        .replace("\u{1b}[1;34m", "");
    assert_inert(&stripped);
}

#[test]
fn diff_escapes_control_and_bidi_characters() {
    let bytes = diff(ANSI_SYSNAME, ANSI_XML, DiffOptions::default()).expect("valid pair");
    let document = serde_json::from_slice(&bytes).expect("a diff document");
    let rendered = render_diff(
        &document,
        "before\u{1b}[2J.json",
        "after\u{202e}.json",
        Color::Never,
    );
    assert_inert(&rendered);
    assert!(
        rendered.starts_with("snapshot diff: before\\u{001B}[2J.json → after\\u{202E}.json\n"),
        "{rendered}"
    );
    assert!(rendered.contains(r"\u{001B}]0;PWNED-TITLE"), "{rendered}");
    assert!(
        rendered.contains(r#"_name: "x\u{1b}[31mR" -> "xpath""#),
        "{rendered}"
    );
}

#[test]
fn multi_line_values_keep_the_marker_layout() {
    // The golden's XML value spans three lines; it renders on one `~` line.
    let bytes = diff(ANSI_XML, ANSI_SYSNAME, DiffOptions::default()).expect("valid pair");
    let document = serde_json::from_slice(&bytes).expect("a diff document");
    let rendered = render_diff(&document, "a", "b", Color::Never);
    assert_inert(&rendered);
    let line = rendered
        .lines()
        .find(|line| line.contains("cart:"))
        .expect("the cart change is rendered");
    assert!(line.starts_with("       ~ cart: xml "), "{rendered}");
    assert!(
        line.ends_with(
            r#"-> xml <data id="cart">\n      <items><item>apple</item></items>\n    </data>"#
        ),
        "{rendered}"
    );
}
