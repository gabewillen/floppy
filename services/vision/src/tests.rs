use super::*;

/// A PNG of `width` × `height` RGB pixels `pixel(x, y)`.
fn png_image(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("the header encodes");
    let data: Vec<u8> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .flat_map(|(x, y)| pixel(x, y))
        .collect();
    writer.write_image_data(&data).expect("the pixels encode");
    writer.finish().expect("the image ends");
    bytes
}

/// A deterministic textured image.
fn textured(width: u32, height: u32) -> Vec<u8> {
    png_image(width, height, |x, y| {
        let wrap = |value: u32| u8::try_from(value % 256).expect("below 256");
        [
            wrap(x * 37 + y * 11),
            wrap(x * x + y * 5),
            wrap((x ^ y) * 3),
        ]
    })
}

/// A uniform image of one gray level.
fn uniform(level: u8) -> Vec<u8> {
    png_image(64, 48, |_, _| [level; 3])
}

/// The signature of a uniform image of `level`, as `OpenCV`'s `float32`.
fn level(level: u8) -> f64 {
    f64::from(f32::from(level) / 255.0)
}

fn request(parameters: &[(&str, Parameter<'_>)]) -> Result<VisionRequest, VisionError> {
    VisionRequest::from_parameters(parameters.iter().copied())
}

fn first_view(measure: Measure) -> VisionRequest {
    VisionRequest {
        measure,
        path: "/media/a.png".to_owned(),
        previous: None,
    }
}

fn after(measure: Measure, previous: Signature) -> VisionRequest {
    VisionRequest {
        previous: Some(previous),
        ..first_view(measure)
    }
}

/// `OpenCV`'s grids for the frame difference pipeline
/// (`cv2.GaussianBlur(cv2.resize(cv2.imdecode(image,
/// cv2.IMREAD_REDUCED_GRAYSCALE_8), (32, 32), interpolation=cv2.INTER_AREA),
/// (3, 3), 0)`, `OpenCV` 5.0.0 with libpng 1.6.58, libjpeg-turbo 3.1.2 and
/// libwebp 1.6.0): one line a fixture, its name and its grid row by row in
/// hex. The fixtures are the files beside it, and `textured-<w>x<h>` the
/// [`textured`] PNG of that size.
const OPENCV: &str = include_str!("testdata/opencv.txt");

fn opencv(name: &str) -> Vec<u8> {
    let hex = OPENCV
        .lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(' '))
        .expect("the fixture has a grid");
    (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).expect("hex"))
        .collect()
}

/// The grid of a signature, in 8-bit steps.
fn steps(values: &[f64]) -> Vec<u8> {
    values
        .iter()
        .map(|&value| {
            let step = (value * 255.0).round();
            assert!((0.0..=255.0).contains(&step), "{value}");
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "checked to be within 0..=255"
            )]
            let step = step as u8;
            step
        })
        .collect()
}

/// Fixtures beside [`OPENCV`], by name.
macro_rules! fixtures {
    ($($name:literal),* $(,)?) => {
        [$(($name, include_bytes!(concat!("testdata/", $name)).as_slice())),*]
    };
}

/// The signature of an image, in 8-bit steps.
fn signature_steps(image: &[u8]) -> Vec<u8> {
    steps(&frame_signature(image).expect("the image decodes"))
}

#[test]
fn the_frame_difference_is_opencvs_on_every_resize_path() {
    // Reduced to an eighth (`INTER_LINEAR_EXACT`, exactly and not), then
    // area resized: 100 × 75 area averaged, 20 × 20 enlarged, 64 × 64
    // halved, 96 × 96 by thirds, 32 × 32 as it is, 1 × 1 enlarged.
    for (width, height) in [
        (800, 600),
        (803, 601),
        (160, 160),
        (512, 512),
        (768, 768),
        (256, 256),
        (13, 9),
    ] {
        assert_eq!(
            signature_steps(&textured(width, height)),
            opencv(&format!("textured-{width}x{height}")),
            "{width}x{height}"
        );
    }
}

#[test]
fn a_png_is_decoded_as_opencvs_libpng_decodes_it() {
    // Color reduced through the file's gamma (gAMA or sRGB), 16-bit
    // samples reduced at 16 bits, a palette expanded, gray left alone
    // whatever its gamma, alpha dropped, and the eXIf orientation applied.
    for (name, image) in fixtures![
        "gamma.png",
        "srgb.png",
        "deep.png",
        "deep-gamma.png",
        "palette-srgb.png",
        "gray-gamma.png",
        "alpha.png",
        "bilevel.png",
        "oriented.png",
    ] {
        assert_eq!(signature_steps(image), opencv(name), "{name}");
    }
}

#[test]
fn a_jpeg_is_decoded_as_libjpeg_decodes_it_at_an_eighth() {
    // Baseline with 4:2:0 and 4:4:4, progressive, gray, CMYK, RGB (an Adobe
    // transform 0), restart intervals, no Huffman tables (Motion JPEG), an
    // Exif orientation, under 8 pixels a side, and entropy data cut short
    // at a marker.
    for (name, image) in fixtures![
        "color.jpg",
        "color-444.jpg",
        "progressive.jpg",
        "gray.jpg",
        "cmyk.jpg",
        "rgb.jpg",
        "restart.jpg",
        "mjpeg.jpg",
        "oriented.jpg",
        "tiny.jpg",
        "cut.jpg",
        "cut-progressive.jpg",
    ] {
        assert_eq!(signature_steps(image), opencv(name), "{name}");
    }
}

#[test]
fn a_webp_is_decoded_as_opencv_decodes_it() {
    for (name, image) in fixtures![
        "lossless.webp",
        "alpha.webp",
        "oriented.webp",
        "animated.webp",
    ] {
        assert_eq!(signature_steps(image), opencv(name), "{name}");
    }
    // A lossy WebP: image-webp's decoding and libwebp's differ by a step
    // in a few cells.
    let ours = signature_steps(include_bytes!("testdata/lossy.webp"));
    let differing: Vec<_> = ours
        .iter()
        .zip(opencv("lossy.webp"))
        .filter(|&(&left, right)| left != right)
        .map(|(&left, right)| left.abs_diff(right))
        .collect();
    assert!(
        differing.len() <= 8 && differing.iter().all(|&step| step == 1),
        "{differing:?}"
    );
}

#[test]
fn what_opencv_cannot_read_is_undecodable() {
    // Entropy data that runs into the end (with or without its scan
    // complete) and a progressive JPEG without EOI fail in OpenCV, whose
    // memory source cannot be refilled; a PNG under 8 pixels a side
    // reduces to nothing.
    for (name, image) in fixtures!["truncated.jpg", "unended.jpg", "unended-progressive.jpg",] {
        assert!(!OPENCV.contains(&format!("{name} ")), "{name}");
        assert_eq!(
            frame_signature(image),
            Err(VisionError::Undecodable),
            "{name}"
        );
    }
    assert_eq!(
        frame_signature(&textured(7, 40)),
        Err(VisionError::Undecodable)
    );
    assert!(frame_signature(&textured(8, 8)).is_ok());
}

#[test]
fn a_first_frame_has_no_change_and_its_signature() {
    let measurement = first_view(Measure::FrameDiff)
        .measured(&uniform(51))
        .expect("the image measures");
    assert_eq!(
        measurement,
        Measurement {
            change: None,
            signature: Signature::FrameDiff(vec![level(51); FRAME_VALUES]),
        }
    );
    let json = measurement.to_json();
    assert!(
        json.starts_with(
            r#"{"change":null,"signature":{"measure":"frame_diff","values":[0.20000000298023224,"#
        ),
        "{json}"
    );
}

#[test]
fn a_frame_change_is_the_mean_absolute_difference() {
    let black = Signature::FrameDiff(vec![0.0; FRAME_VALUES]);
    let same = after(Measure::FrameDiff, black.clone())
        .measured(&uniform(0))
        .expect("the image measures");
    assert_eq!(same.change, Some(0.0));
    let white = after(Measure::FrameDiff, black)
        .measured(&uniform(255))
        .expect("the image measures");
    assert_eq!(white.change, Some(1.0));
    let mut half = vec![0.0; FRAME_VALUES];
    half[..FRAME_VALUES / 2].fill(1.0);
    let gray = after(Measure::FrameDiff, Signature::FrameDiff(half))
        .measured(&uniform(51))
        .expect("the image measures");
    assert_eq!(gray.change, Some(0.5));
    // Values out of 0..1 are compared as given, then clamped.
    let far = after(
        Measure::FrameDiff,
        Signature::FrameDiff(vec![-3.0; FRAME_VALUES]),
    )
    .measured(&uniform(0))
    .expect("the image measures");
    assert_eq!(far.change, Some(1.0));
}

#[test]
fn byte_identity_is_the_files_sha256() {
    let first = first_view(Measure::ByteIdentity)
        .measured(b"abc")
        .expect("any bytes measure");
    let digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    assert_eq!(
        first,
        Measurement {
            change: None,
            signature: Signature::ByteIdentity(digest.to_owned()),
        }
    );
    assert_eq!(
        first.to_json(),
        format!(
            r#"{{"change":null,"signature":{{"digest":"{digest}","measure":"byte_identity"}}}}"#
        )
    );
    let same = after(Measure::ByteIdentity, first.signature.clone())
        .measured(b"abc")
        .expect("any bytes measure");
    assert_eq!(same.change, Some(0.0));
    assert_eq!(
        same.to_json(),
        format!(
            r#"{{"change":0.0,"signature":{{"digest":"{digest}","measure":"byte_identity"}}}}"#
        )
    );
    let different = after(Measure::ByteIdentity, first.signature)
        .measured(b"abd")
        .expect("any bytes measure");
    assert_eq!(different.change, Some(1.0));
}

#[test]
fn an_empty_or_undecodable_image_fails() {
    for measure in [Measure::ByteIdentity, Measure::FrameDiff] {
        assert_eq!(
            first_view(measure).measured(b""),
            Err(VisionError::Unreadable)
        );
    }
    for image in [
        b"GIF89a....".as_slice(),
        b"\x89PNG\r\n\x1a\nnot a png",
        &[0xff, 0xd8, 0xff, 0xe0, 0, 0],
        b"plain text",
    ] {
        assert_eq!(
            first_view(Measure::FrameDiff).measured(image),
            Err(VisionError::Undecodable)
        );
    }
}

#[test]
fn the_parameters_describe_a_request() {
    assert_eq!(
        request(&[
            ("measure", Parameter::Text("byte_identity")),
            ("image", Parameter::Text("media/a.png")),
        ]),
        Ok(first_view(Measure::ByteIdentity))
    );
    assert_eq!(
        request(&[
            ("image", Parameter::Text("/media/a.png")),
            ("previous", Parameter::Null),
            ("measure", Parameter::Text("frame_diff")),
        ]),
        Ok(first_view(Measure::FrameDiff))
    );
    assert_eq!(
        request(&[
            ("measure", Parameter::Text("byte_identity")),
            ("image", Parameter::Text("file:///media/a.png")),
            (
                "previous",
                Parameter::Object(r#"{"digest":"00","measure":"byte_identity"}"#)
            ),
        ]),
        Ok(after(
            Measure::ByteIdentity,
            Signature::ByteIdentity("00".to_owned())
        ))
    );
    let values = format!("[{}]", vec!["0.5"; FRAME_VALUES].join(","));
    let previous = format!(r#"{{"measure":"frame_diff","values":{values}}}"#);
    assert_eq!(
        request(&[
            ("measure", Parameter::Text("frame_diff")),
            ("image", Parameter::Text("media/a.png")),
            ("previous", Parameter::Object(&previous)),
        ]),
        Ok(after(
            Measure::FrameDiff,
            Signature::FrameDiff(vec![0.5; FRAME_VALUES])
        ))
    );
}

#[test]
fn a_bad_parameter_is_refused() {
    let measure = ("measure", Parameter::Text("frame_diff"));
    let image = ("image", Parameter::Text("media/a.png"));
    for parameters in [
        vec![image],
        vec![measure],
        vec![("measure", Parameter::Text("embedding")), image],
        vec![("measure", Parameter::Other), image],
        vec![measure, ("image", Parameter::Other)],
        vec![measure, ("image", Parameter::Text("media/../a.png"))],
        vec![
            measure,
            ("image", Parameter::Text("https://example.com/a.png")),
        ],
        vec![measure, image, image],
        vec![measure, image, ("size", Parameter::Text("32"))],
        vec![measure, image, ("previous", Parameter::Text("{}"))],
        vec![measure, image, ("previous", Parameter::Object("[]"))],
    ] {
        assert_eq!(
            request(&parameters),
            Err(VisionError::InvalidParameter),
            "{parameters:?}"
        );
    }
}

#[test]
fn a_previous_of_another_measure_or_size_is_refused() {
    let short = format!(
        r#"{{"measure":"frame_diff","values":[{}]}}"#,
        vec!["0"; FRAME_VALUES - 1].join(",")
    );
    let texts = format!(
        r#"{{"measure":"frame_diff","values":[{}]}}"#,
        vec![r#""0""#; FRAME_VALUES].join(",")
    );
    for (measure, previous) in [
        ("frame_diff", r#"{"digest":"00","measure":"byte_identity"}"#),
        ("byte_identity", r#"{"measure":"frame_diff","values":[]}"#),
        ("byte_identity", r#"{"digest":0,"measure":"byte_identity"}"#),
        (
            "byte_identity",
            r#"{"digest":"00","measure":"byte_identity","values":[]}"#,
        ),
        ("byte_identity", r#"{"measure":"byte_identity"}"#),
        ("frame_diff", short.as_str()),
        ("frame_diff", texts.as_str()),
    ] {
        assert_eq!(
            request(&[
                ("measure", Parameter::Text(measure)),
                ("image", Parameter::Text("media/a.png")),
                ("previous", Parameter::Object(previous)),
            ]),
            Err(VisionError::MismatchedPrevious),
            "{measure} {previous}"
        );
    }
}

#[test]
fn media_references_resolve_as_the_speech_services() {
    for (reference, path) in [
        ("media/a.png", "/media/a.png"),
        ("/media/a.png", "/media/a.png"),
        ("file:///media/a.png", "/media/a.png"),
        ("media/a%20b.png", "/media/a b.png"),
    ] {
        assert_eq!(media_path(reference).as_deref(), Ok(path), "{reference}");
    }
    for reference in [
        "",
        "//host/a.png",
        "file://host/a.png",
        "https://example.com/a.png",
        "media/a.png?x",
        "media/a.png#x",
        "media//a.png",
        "media/./a.png",
        "media/../a.png",
        "media/a%2fb.png",
        "media/a%00.png",
        "media/a%zz.png",
    ] {
        assert!(media_path(reference).is_err(), "{reference}");
    }
}

#[test]
fn the_package_manifest_binds_the_exact_uri_and_artifacts() {
    let manifest = scxml_validator_core::ServicePackageManifest::from_json(include_bytes!(
        "../package/vision.service.json"
    ))
    .expect("the manifest parses");
    manifest.verify(None).expect("the manifest verifies");
    assert_eq!(manifest.service_uri, SERVICE_URI);
    assert_eq!(
        SERVICE_URI,
        format!("https://github.com/stateforward/floppy/services/{SERVICE_NAME}")
    );
    assert_eq!(
        manifest.service_artifact,
        "floppy_service_vision.virtualized.wasm"
    );
    assert!(manifest.env_slots.is_empty());
    assert!(manifest.http_origins.is_empty());
}

#[test]
fn a_corrupt_image_fails_or_decodes_without_panicking() {
    // Deterministic byte edits and cuts of every format's fixtures; a
    // decoder that panics (or overflows, in this build) fails the test.
    let images = fixtures![
        "color.jpg",
        "progressive.jpg",
        "cmyk.jpg",
        "restart.jpg",
        "oriented.jpg",
        "deep-gamma.png",
        "oriented.png",
        "lossy.webp",
        "oriented.webp",
    ];
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = |bound: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        usize::try_from(state % u64::try_from(bound).expect("a small bound"))
            .expect("below the bound")
    };
    for (_, image) in images {
        for _ in 0..60 {
            let mut bytes = image.to_vec();
            for _ in 0..=next(4) {
                let at = next(bytes.len());
                bytes[at] = u8::try_from(next(256)).expect("below 256");
            }
            let cut = next(bytes.len());
            let _ = frame_signature(&bytes);
            let _ = frame_signature(&bytes[..cut]);
        }
    }
}
