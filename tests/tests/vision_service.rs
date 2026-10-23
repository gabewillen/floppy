//! Integration test of the `vision` service through `scx run`: a session
//! binds the real `floppy_service_vision` component (resolved from its
//! repository URI) and invokes it
//! (`<invoke type="https://github.com/stateforward/floppy/services/vision">`)
//! against the media directory the run grants with
//! `--dir URI=HOST::/media`. It measures a view with a change measure
//! (`byte_identity`, `frame_diff`) and answers the measurement as the done
//! data, `{change, signature}`: no change for a
//! first view, then the change against the `previous` signature the
//! document hands back. A reference that would leave the directory, a
//! missing or undecodable image, and a `previous` of another measure are
//! refused (`error.execution`), and a run that binds it with no grant is
//! refused before it starts.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use sha2::{Digest as _, Sha256};
use support::{Finished, logs, run_document, service_uri, unique_scratch};

/// The CRC-32 (ISO-HDLC) of `bytes`, a PNG chunk's checksum.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A `width` × `height` 8-bit grayscale PNG of one `level`, its pixels in
/// stored (uncompressed) deflate blocks.
fn png(width: u32, height: u32, level: u8) -> Vec<u8> {
    let row = usize::try_from(width).expect("small") + 1;
    let mut raw = Vec::new();
    for _ in 0..height {
        raw.push(0);
        raw.resize(raw.len() + row - 1, level);
    }
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (index, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(index + 1 == blocks.len()));
        let length = u16::try_from(block.len()).expect("a stored block");
        zlib.extend_from_slice(&length.to_le_bytes());
        zlib.extend_from_slice(&(!length).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    let (mut a, mut b) = (1_u32, 0_u32);
    for &byte in &raw {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut header = width.to_be_bytes().to_vec();
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 0, 0, 0, 0]);
    let mut image = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, data) in [
        (b"IHDR", header.as_slice()),
        (b"IDAT", &zlib),
        (b"IEND", &[]),
    ] {
        image.extend_from_slice(&u32::try_from(data.len()).expect("small").to_be_bytes());
        let mut chunk = kind.to_vec();
        chunk.extend_from_slice(data);
        image.extend_from_slice(&chunk);
        image.extend_from_slice(&crc32(&chunk).to_be_bytes());
    }
    image
}

/// A fresh media directory holding `files`, granted to the vision service
/// at `/media`.
fn granted(files: &[(&str, &[u8])]) -> (PathBuf, Vec<String>) {
    let media = unique_scratch("vision-media");
    for (name, bytes) in files {
        std::fs::write(media.join(name), bytes).expect("the media file is writable");
    }
    let grant = vec![
        "--dir".to_owned(),
        format!("{}={}::/media", service_uri("vision"), media.display()),
    ];
    (media, grant)
}

fn open(args: &[String], document: &str) -> Finished {
    run_document("vision.scx", document, args, &[], Duration::from_secs(60))
}

/// Measures `first`, then `second` against the first's signature, then
/// `third` against the second's, with `measure`, logging each done data's
/// change and the signature's measure and size.
fn chain(measure: &str, [first, second, third]: [&str; 3]) -> String {
    let step = |id: &str, image: &str, next: &str| {
        format!(
            r#"<state id="{id}">
    <invoke id="measuring-{id}" type="{service}">
      <param name="measure" expr="'{measure}'"/>
      <param name="image" expr="'{image}'"/>
      <param name="previous" expr="seen"/>
    </invoke>
    <transition event="done.invoke.measuring-{id}" target="{next}">
      <log label="{id}" expr="JSON.stringify(_event.data.change) + ' ' + _event.data.signature.measure + ' ' + (_event.data.signature.digest || _event.data.signature.values.length)"/>
      <assign location="seen" expr="_event.data.signature"/>
    </transition>
    <transition event="error.*" target="failed"/>
  </state>
  "#,
            service = service_uri("vision")
        )
    };
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="first">
  <datamodel><data id="seen" expr="null"/></datamodel>
  {}{}{}<final id="done"/>
  <final id="failed"/>
</scxml>"#,
        step("first", first, "second"),
        step("second", second, "third"),
        step("third", third, "done"),
    )
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

#[test]
fn byte_identity_answers_no_change_then_the_digests_comparison() {
    let (a, b) = (png(40, 30, 10), png(40, 30, 11));
    let (_media, grant) = granted(&[("a.png", &a), ("b.png", &b)]);
    let finished = open(
        &grant,
        &chain(
            "byte_identity",
            ["media/a.png", "/media/a.png", "media/b.png"],
        ),
    );
    assert_eq!(
        logs(&finished.records),
        [
            (
                "first".to_owned(),
                format!("null byte_identity {}", hex(&a))
            ),
            ("second".to_owned(), format!("0 byte_identity {}", hex(&a))),
            ("third".to_owned(), format!("1 byte_identity {}", hex(&b))),
        ],
        "{}",
        finished.stderr
    );
    assert!(support::finished(&finished.records));
}

#[test]
fn frame_diff_answers_the_mean_absolute_difference_of_its_grids() {
    let (_media, grant) = granted(&[
        ("black.png", &png(64, 48, 0)),
        ("also-black.png", &png(320, 240, 0)),
        ("white.png", &png(33, 100, 255)),
    ]);
    let finished = open(
        &grant,
        &chain(
            "frame_diff",
            ["media/black.png", "media/also-black.png", "media/white.png"],
        ),
    );
    assert_eq!(
        logs(&finished.records),
        [
            ("first".to_owned(), "null frame_diff 1024".to_owned()),
            ("second".to_owned(), "0 frame_diff 1024".to_owned()),
            ("third".to_owned(), "1 frame_diff 1024".to_owned()),
        ],
        "{}",
        finished.stderr
    );
    assert!(support::finished(&finished.records));
}

/// Invokes the service with `params`, logging `refused` on
/// `error.execution` and `answered` on done data.
fn single(params: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="measuring">
  <state id="measuring">
    <invoke id="measure" type="{service}">
      {params}
    </invoke>
    <transition event="done.invoke.measure" target="done">
      <log label="answered" expr="JSON.stringify(_event.data)"/>
    </transition>
    <transition event="error.execution" target="done">
      <log label="refused" expr="_event.name"/>
    </transition>
  </state>
  <final id="done"/>
</scxml>"#,
        service = service_uri("vision")
    )
}

#[test]
fn a_bad_reference_image_or_previous_is_refused() {
    let (media, grant) = granted(&[("a.png", &png(8, 8, 1)), ("a.txt", b"not an image")]);
    std::fs::write(
        media
            .parent()
            .expect("a scratch parent")
            .join("outside.png"),
        png(8, 8, 1),
    )
    .expect("the outside file is writable");
    for params in [
        r#"<param name="measure" expr="'byte_identity'"/><param name="image" expr="'media/../outside.png'"/>"#,
        r#"<param name="measure" expr="'byte_identity'"/><param name="image" expr="'media/missing.png'"/>"#,
        r#"<param name="measure" expr="'frame_diff'"/><param name="image" expr="'media/a.txt'"/>"#,
        r#"<param name="measure" expr="'embedding'"/><param name="image" expr="'media/a.png'"/>"#,
        r#"<param name="measure" expr="'frame_diff'"/><param name="image" expr="'media/a.png'"/><param name="previous" expr="({measure: 'byte_identity', digest: '00'})"/>"#,
    ] {
        let finished = open(&grant, &single(params));
        assert_eq!(
            logs(&finished.records),
            [("refused".to_owned(), "error.execution".to_owned())],
            "{params}: {}",
            finished.stderr
        );
    }
    // Any bytes have an identity.
    let finished = open(
        &grant,
        &single(
            r#"<param name="measure" expr="'byte_identity'"/><param name="image" expr="'file:///media/a.txt'"/>"#,
        ),
    );
    assert_eq!(
        logs(&finished.records),
        [(
            "answered".to_owned(),
            format!(
                r#"{{"change":null,"signature":{{"digest":"{}","measure":"byte_identity"}}}}"#,
                hex(b"not an image")
            )
        )],
        "{}",
        finished.stderr
    );
}

#[test]
fn a_run_that_binds_it_with_no_grant_is_refused() {
    let path = support::write(
        &unique_scratch("vision-ungranted"),
        "vision.scx",
        &chain("byte_identity", ["media/a.png"; 3]),
    );
    let output = support::output(&[
        "run".to_owned(),
        "--clock".to_owned(),
        "logical".to_owned(),
        path.display().to_string(),
    ]);
    assert!(!output.status.success(), "{}", output.stderr);
    assert!(
        output
            .stderr
            .contains("whose resource the run does not grant"),
        "{}",
        output.stderr
    );
}
