//! The `vision` invoked service: measures how much a view changed since the
//! last one handed on, as a `stateforward:scxml/invoked-service` Wasm
//! component. It is the original's change measures
//! (`mosfet.abilities.vision.change`): `ByteIdentity`, and the `opencv`
//! provider's `FrameDiff`, which floppy computes in the component instead of
//! through `OpenCV`. The seeing ability (`abilities/seeing/seeing.scx`)
//! invokes it once per measured view, with the signature of the last view it
//! handed on, and keeps the signature it answers when it hands this one on.
//!
//! Reading and decoding an image is I/O and work that may take a while, so
//! this is a service, never executable content: the invoking state bounds it
//! with a `<send delay>`, and leaving that state cancels it. Documents name
//! [`SERVICE_URI`]:
//!
//! ```xml
//! <invoke id="vision" type="https://github.com/stateforward/floppy/services/vision">
//!   <param name="measure" expr="'frame_diff'"/>
//!   <param name="image" expr="measuredImage()"/>
//!   <param name="previous" expr="previousSignature()"/>
//! </invoke>
//! ```
//!
//! (seeing's own, whose functions read the view it measures and the
//! signature it keeps for that view's source).
//!
//! # Media
//!
//! Media moves by reference, never as bytes (port plan, "Phase 4 event
//! contracts"): `image` is a URI reference to a file in a directory the run
//! grants with
//! `--dir https://github.com/stateforward/floppy/services/vision=HOST::/media`
//! (`scx run` refuses a run that binds the service without it). A reference
//! is resolved exactly as the speech service resolves its media
//! ([`media_path`]): `media/a.png`, `/media/a.png`, and `file:///media/a.png`
//! all name the guest file `/media/a.png`. A reference with another scheme,
//! an authority, a query, a fragment, an empty, `.`, or `..` segment, or an
//! escape that is not UTF-8 or decodes to `/` or NUL is malformed. A file
//! outside the granted directory cannot be opened.
//!
//! # Parameters (`<param>`)
//!
//! * `measure` (required): [`Measure::ByteIdentity`] (`byte_identity`, the
//!   original's `ByteIdentity`) or [`Measure::FrameDiff`] (`frame_diff`, the
//!   original's `opencv.FrameDiff` at its default size, [`FRAME_SIZE`]).
//! * `image` (required): a reference to the image (see "Media").
//! * `previous` (optional): the signature of the last view handed on, an
//!   object exactly as this service answered it; `null` or absent for a
//!   first view.
//!
//! Any other parameter, a parameter given twice, or a malformed value
//! rejects the start: the `<invoke>` raises `error.execution` and nothing is
//! read.
//!
//! # Result
//!
//! `invoke` reads the image, measures it, and resolves its future at once
//! with the done data, the original's `Measurement` as canonical JSON (the
//! invoking session receives it as `done.invoke.<id>`), and returns no
//! event:
//!
//! * `{"change": C, "signature": {"digest": D, "measure": "byte_identity"}}`:
//!   `D` is the SHA-256 of the file's bytes in lower-case hex, and `C` is
//!   `null` without `previous`, else `0.0` when the digests are equal and
//!   `1.0` when they differ.
//! * `{"change": C, "signature": {"measure": "frame_diff", "values": V}}`:
//!   `V` is the [`FRAME_SIZE`] × [`FRAME_SIZE`] grid of the image, row by
//!   row, each value in 0..1 ([`frame_signature`]), and `C` is `null`
//!   without `previous`, else the mean absolute difference of `V` and the
//!   previous values, clamped to 0..1.
//!
//! A failure rejects the start, as the files service's does (the `<invoke>`
//! raises `error.execution`; the original's `change.Error`): an invalid
//! parameter, a `previous` that is not a signature of the requested measure
//! and size, an unreadable or empty file, a file over [`MAX_IMAGE_BYTES`],
//! and, for `frame_diff`, an image the original's `OpenCV` decode would not
//! read (see "The frame difference") or one over [`MAX_PIXELS`]
//! ([`VisionError`]).
//!
//! # The frame difference
//!
//! [`frame_signature`] is the original's `OpenCV` pipeline,
//! `cv2.GaussianBlur(cv2.resize(cv2.imdecode(image,
//! cv2.IMREAD_REDUCED_GRAYSCALE_8), (32, 32), interpolation=cv2.INTER_AREA),
//! (3, 3), 0)`, computed in the component. The reduced decode reads the
//! image in 8-bit grayscale at an eighth of each side, as `OpenCV` 5's
//! codecs read it:
//!
//! * a PNG through libpng's transformations (a palette and low bit depths
//!   expanded, alpha dropped, color reduced by `png_set_rgb_to_gray(0.299,
//!   0.587)` through the file's gamma when its `gAMA` or `sRGB` chunk makes
//!   it significant, 16-bit samples reduced at 16 bits then cut to their
//!   high byte), then reduced by `OpenCV`'s bit-exact bilinear resize
//!   (`INTER_LINEAR_EXACT`) to `width / 8` × `height / 8`, so a PNG under 8
//!   pixels a side fails, as it fails in `OpenCV`;
//! * a JPEG at libjpeg's 1/8 scale, one pixel a block from its DC
//!   coefficient, read by this crate's own decoder (`jpeg.rs`, whose
//!   documentation lists what it does not decode): the luma of a gray or
//!   YCbCr JPEG, libjpeg's gray of an RGB one, `OpenCV`'s of a CMYK or
//!   YCCK one;
//! * a WebP through `image-webp`, reduced by `cvtColor`'s BT.601 weights,
//!   then by `INTER_LINEAR_EXACT` as a PNG is.
//!
//! The image is then turned by its Exif orientation (a JPEG's `Exif`
//! segment, a PNG's `eXIf` chunk, a WebP's `EXIF` chunk), as `imdecode`
//! turns it; resized to [`FRAME_SIZE`] × [`FRAME_SIZE`] by `OpenCV`'s
//! `INTER_AREA`; blurred with the kernel `OpenCV` uses for a 3 × 3 Gaussian
//! with sigma 0 (`[1, 2, 1] / 4` each way, not the sigma 0.8 its formula
//! would give) with the border reflected about its edge pixel
//! (`BORDER_REFLECT_101`); and scaled to 0..1 as `float32`. Each step
//! rounds to 8 bits exactly as `OpenCV`'s does, so a PNG's, a JPEG's and a
//! lossless WebP's grid is `OpenCV` 5.0.0's to the bit (the unit tests hold
//! its grids); a lossy WebP's may differ by one step in a few cells,
//! `image-webp`'s decoding and libwebp's own difference. Any other format
//! fails.
//!
//! # Bounds
//!
//! The file's bytes ([`MAX_IMAGE_BYTES`], the original's
//! `media.MAX_IMAGE_BYTES`), the decoded pixels ([`MAX_PIXELS`]), and a
//! reference ([`MAX_REFERENCE_BYTES`]) are bounded on wasm builds only
//! ([`scxml_types::wasm_bound`]). The done data is at most
//! [`FRAME_SIZE`]² numbers, under the session's event bound.
//!
//! # Telemetry
//!
//! `invoke` records one observation through scxml.rs's `telemetry` WIT
//! facade (operation `start`: its closed outcome, `ok`, `rejected` or
//! `failed`, and one closed signal event). No attribute carries a
//! reference, a path, a digest or a value, so the cardinality is the
//! product of the closed enums.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; no authored unsafe code"
    )
)]

use std::fmt::Write as _;
use std::io::Cursor;

use scxml_event_schema::canonical_json;
use scxml_types::{exceeds_wasm_bound, wasm_bound};
use serde_json::{Value as Json, json};
use sha2::{Digest as _, Sha256};

/// The service's name: the last path segment of its repository service URI
/// before the tag.
pub const SERVICE_NAME: &str = "vision";

/// The exact service type URI documents name: tagless, so it names the
/// repository's latest release (decision D19); the manifest names it too.
pub const SERVICE_URI: &str = "https://github.com/stateforward/floppy/services/vision";

/// Upper bound on an image file's bytes (wasm builds only): the original's
/// `media.MAX_IMAGE_BYTES`.
pub const MAX_IMAGE_BYTES: usize = wasm_bound(4 * 1024 * 1024);

/// Upper bound on a decoded image's pixels (wasm builds only): 16
/// megapixels, so a small file cannot decode into an unbounded buffer.
pub const MAX_PIXELS: usize = wasm_bound(4096 * 4096);

/// Upper bound on a media reference's bytes (wasm builds only), the speech
/// service's.
pub const MAX_REFERENCE_BYTES: usize = wasm_bound(1024);

/// The side of the frame difference grid: the original's `FrameDiff`
/// default size.
pub const FRAME_SIZE: usize = 32;

/// The values of a frame difference signature.
pub const FRAME_VALUES: usize = FRAME_SIZE * FRAME_SIZE;

/// Why the service rejected its start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VisionError {
    /// A parameter is unknown, repeated, missing, or malformed.
    InvalidParameter,
    /// `previous` is not a signature of the requested measure and size (the
    /// original's `change.Error`).
    MismatchedPrevious,
    /// The image file could not be read, or is empty.
    Unreadable,
    /// The image file is over [`MAX_IMAGE_BYTES`].
    TooLarge,
    /// The image is not one the frame difference reads (see the crate
    /// documentation, "The frame difference"), or is over [`MAX_PIXELS`].
    Undecodable,
}

impl VisionError {
    /// The rejection's static detail (never a reference, a path or a value).
    #[must_use]
    pub const fn detail(self) -> &'static str {
        match self {
            Self::InvalidParameter => "a vision parameter is invalid",
            Self::MismatchedPrevious => {
                "the previous signature is not one of this measure and size"
            }
            Self::Unreadable => "the image could not be read",
            Self::TooLarge => "the image is over its byte bound",
            Self::Undecodable => "the image could not be decoded",
        }
    }
}

/// One invocation parameter's value as the service reads it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Parameter<'a> {
    /// A text value.
    Text(&'a str),
    /// A JSON object, as its canonical text.
    Object(&'a str),
    /// `null`, or no value.
    Null,
    /// Any other value.
    Other,
}

/// A change measure (the `measure` parameter).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Measure {
    /// `byte_identity`: changed unless the file's bytes are identical.
    ByteIdentity,
    /// `frame_diff`: the mean absolute difference of two small, blurred
    /// grayscale grids.
    FrameDiff,
}

impl Measure {
    /// The measure's name, as the parameter and the signature carry it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::ByteIdentity => "byte_identity",
            Self::FrameDiff => "frame_diff",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        [Self::ByteIdentity, Self::FrameDiff]
            .into_iter()
            .find(|measure| measure.name() == name)
    }
}

/// A view's signature: what the invoker keeps when it hands the view on.
#[derive(Clone, Debug, PartialEq)]
pub enum Signature {
    /// The SHA-256 of the file's bytes, in lower-case hex.
    ByteIdentity(String),
    /// The [`FRAME_VALUES`] values of the grid, each in 0..1.
    FrameDiff(Vec<f64>),
}

impl Signature {
    /// The signature as a JSON object.
    fn to_json(&self) -> Json {
        match self {
            Self::ByteIdentity(digest) => {
                json!({"measure": Measure::ByteIdentity.name(), "digest": digest})
            }
            Self::FrameDiff(values) => {
                json!({"measure": Measure::FrameDiff.name(), "values": values})
            }
        }
    }

    /// The `previous` signature of `measure` its canonical JSON object
    /// text describes: exactly the keys this service answers, the
    /// measure's own name, and, for a frame difference, [`FRAME_VALUES`]
    /// numbers.
    ///
    /// # Errors
    ///
    /// [`VisionError::InvalidParameter`] for text that is not a JSON
    /// object, and [`VisionError::MismatchedPrevious`] for an object that
    /// is not a signature of `measure` and its size.
    pub fn previous(measure: Measure, text: &str) -> Result<Self, VisionError> {
        let Ok(Json::Object(object)) = serde_json::from_str::<Json>(text) else {
            return Err(VisionError::InvalidParameter);
        };
        let mismatched = VisionError::MismatchedPrevious;
        if object.get("measure").and_then(Json::as_str) != Some(measure.name()) || object.len() != 2
        {
            return Err(mismatched);
        }
        match measure {
            Measure::ByteIdentity => object
                .get("digest")
                .and_then(Json::as_str)
                .map(|digest| Self::ByteIdentity(digest.to_owned()))
                .ok_or(mismatched),
            Measure::FrameDiff => {
                let values = object
                    .get("values")
                    .and_then(Json::as_array)
                    .filter(|values| values.len() == FRAME_VALUES)
                    .ok_or(mismatched)?;
                values
                    .iter()
                    .map(|value| value.as_f64().ok_or(mismatched))
                    .collect::<Result<Vec<_>, _>>()
                    .map(Self::FrameDiff)
            }
        }
    }
}

/// One measured view, the original's `Measurement`.
#[derive(Clone, Debug, PartialEq)]
pub struct Measurement {
    /// How much the view changed, 0 (same) to 1 (entirely different);
    /// `None` without a previous signature.
    pub change: Option<f64>,
    /// The view's signature.
    pub signature: Signature,
}

impl Measurement {
    /// The done data, canonical JSON: `{"change": C, "signature": S}`.
    #[must_use]
    pub fn to_json(&self) -> String {
        canonical_json(&json!({
            "change": self.change,
            "signature": self.signature.to_json(),
        }))
    }
}

/// What one start does.
#[derive(Clone, Debug, PartialEq)]
pub struct VisionRequest {
    /// The measure.
    pub measure: Measure,
    /// The absolute guest path of the image ([`media_path`]).
    pub path: String,
    /// The signature of the last view handed on.
    pub previous: Option<Signature>,
}

impl VisionRequest {
    /// The request the invocation's parameters (in document order)
    /// describe.
    ///
    /// # Errors
    ///
    /// [`VisionError::InvalidParameter`] for an unknown, repeated, missing
    /// or malformed parameter, and [`VisionError::MismatchedPrevious`] for a
    /// `previous` that is not a signature of the measure
    /// ([`Signature::previous`]).
    pub fn from_parameters<'a>(
        parameters: impl IntoIterator<Item = (&'a str, Parameter<'a>)>,
    ) -> Result<Self, VisionError> {
        let invalid = VisionError::InvalidParameter;
        let mut measure = None;
        let mut image = None;
        let mut previous = None;
        for (name, value) in parameters {
            let slot = match name {
                "measure" => &mut measure,
                "image" => &mut image,
                "previous" => &mut previous,
                _ => return Err(invalid),
            };
            if slot.replace(value).is_some() {
                return Err(invalid);
            }
        }
        let measure = match measure {
            Some(Parameter::Text(name)) => Measure::from_name(name).ok_or(invalid)?,
            _ => return Err(invalid),
        };
        let path = match image {
            Some(Parameter::Text(reference)) => media_path(reference).map_err(|_| invalid)?,
            _ => return Err(invalid),
        };
        let previous = match previous {
            None | Some(Parameter::Null) => None,
            Some(Parameter::Object(text)) => Some(Signature::previous(measure, text)?),
            Some(_) => return Err(invalid),
        };
        Ok(Self {
            measure,
            path,
            previous,
        })
    }

    /// Measures the image's bytes against the previous signature.
    ///
    /// # Errors
    ///
    /// [`VisionError::Unreadable`] for no bytes, [`VisionError::TooLarge`]
    /// for bytes over [`MAX_IMAGE_BYTES`], and, for a frame difference,
    /// [`VisionError::Undecodable`] ([`frame_signature`]).
    pub fn measured(&self, image: &[u8]) -> Result<Measurement, VisionError> {
        if image.is_empty() {
            return Err(VisionError::Unreadable);
        }
        if exceeds_wasm_bound(image.len(), MAX_IMAGE_BYTES) {
            return Err(VisionError::TooLarge);
        }
        let (signature, change) = match self.measure {
            Measure::ByteIdentity => {
                let digest = hex_digest(image);
                let change = match &self.previous {
                    Some(Signature::ByteIdentity(previous)) => {
                        Some(if *previous == digest { 0.0 } else { 1.0 })
                    }
                    _ => None,
                };
                (Signature::ByteIdentity(digest), change)
            }
            Measure::FrameDiff => {
                let values = frame_signature(image)?;
                let change = match &self.previous {
                    Some(Signature::FrameDiff(previous)) => {
                        Some(mean_difference(previous, &values))
                    }
                    _ => None,
                };
                (Signature::FrameDiff(values), change)
            }
        };
        Ok(Measurement { change, signature })
    }
}

/// The SHA-256 of `bytes` in lower-case hex.
#[must_use]
pub fn hex_digest(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// The mean absolute difference of two grids, clamped to 0..1.
fn mean_difference(previous: &[f64], current: &[f64]) -> f64 {
    let total: f64 = previous
        .iter()
        .zip(current)
        .map(|(left, right)| (left - right).abs())
        .sum();
    #[allow(
        clippy::cast_precision_loss,
        reason = "FRAME_VALUES is 1024, exact in f64"
    )]
    let mean = total / FRAME_VALUES as f64;
    mean.clamp(0.0, 1.0)
}

/// The absolute guest path a media reference names: the reference resolved
/// against `file:///`, the speech service's rule (see the crate
/// documentation, "Media").
///
/// # Errors
///
/// A static detail for a reference that is empty, over its bound, or not a
/// plain path in the guest's file system.
pub fn media_path(reference: &str) -> Result<String, &'static str> {
    const INVALID: &str = "vision media must be a file reference in the granted directory";
    if reference.is_empty() || exceeds_wasm_bound(reference.len(), MAX_REFERENCE_BYTES) {
        return Err(INVALID);
    }
    let path = if let Some(rest) = reference.strip_prefix("file:") {
        // `file:///path`: an empty authority, then an absolute path.
        rest.strip_prefix("//")
            .filter(|path| path.starts_with('/'))
            .ok_or(INVALID)?
    } else if reference.starts_with("//") {
        return Err(INVALID);
    } else {
        reference
    };
    // A scheme other than `file` (a colon before the first `/`), a query,
    // or a fragment.
    let first_segment = path.split('/').next().unwrap_or_default();
    if first_segment.contains(':') || path.contains(['?', '#']) {
        return Err(INVALID);
    }
    let relative = path.strip_prefix('/').unwrap_or(path);
    let mut resolved = String::with_capacity(path.len() + 1);
    for segment in relative.split('/') {
        let segment = percent_decoded(segment).ok_or(INVALID)?;
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(INVALID);
        }
        resolved.push('/');
        resolved.push_str(&segment);
    }
    Ok(resolved)
}

/// One path segment with its `%XX` escapes decoded, or `None` when an
/// escape is malformed, the result is not UTF-8, or it holds `/` or NUL.
fn percent_decoded(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'%' {
            let high = char::from(*bytes.get(index + 1)?).to_digit(16)?;
            let low = char::from(*bytes.get(index + 2)?).to_digit(16)?;
            out.push(u8::try_from(high * 16 + low).ok()?);
            index += 3;
        } else {
            out.push(byte);
            index += 1;
        }
    }
    let decoded = String::from_utf8(out).ok()?;
    (!decoded.contains(['/', '\0'])).then_some(decoded)
}

/// An 8-bit grayscale image, row by row.
struct Gray {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

/// The frame difference signature of an encoded image: its
/// [`FRAME_VALUES`] values, row by row, each in 0..1 (see the crate
/// documentation, "The frame difference").
///
/// # Errors
///
/// [`VisionError::Undecodable`] for an image that is not a PNG, JPEG or
/// WebP this service decodes, is over [`MAX_PIXELS`], or, as a PNG or a
/// WebP, is under [`REDUCTION`] pixels on a side.
pub fn frame_signature(image: &[u8]) -> Result<Vec<f64>, VisionError> {
    let (reduced, orientation) = if image.starts_with(b"\x89PNG\r\n\x1a\n") {
        png_gray(image).and_then(|(gray, orientation)| Some((reduced(&gray)?, orientation)))
    } else if image.starts_with(&[0xff, 0xd8]) {
        jpeg::reduced(image)
    } else if image.starts_with(b"RIFF") && image.get(8..12) == Some(b"WEBP".as_slice()) {
        webp_gray(image).and_then(|(gray, orientation)| Some((reduced(&gray)?, orientation)))
    } else {
        None
    }
    .ok_or(VisionError::Undecodable)?;
    let reduced = exif::oriented(reduced, orientation);
    let small = area_resized(&reduced).ok_or(VisionError::Undecodable)?;
    // OpenCV scales its 8-bit result as `float32` (`astype(numpy.float32) /
    // 255.0`); the original's numbers are those `float32` values.
    Ok(blurred(&small)
        .iter()
        .map(|&value| f64::from(f32::from(value) / 255.0))
        .collect())
}

/// The factor the original's decode reduces each side by
/// (`IMREAD_REDUCED_GRAYSCALE_8`).
pub const REDUCTION: usize = 8;

/// Whether `width` × `height` is a decodable, bounded image.
fn bounded(width: usize, height: usize) -> bool {
    width > 0
        && height > 0
        && width
            .checked_mul(height)
            .is_some_and(|pixels| !exceeds_wasm_bound(pixels, MAX_PIXELS))
}

/// The weights `OpenCV`'s `cvtColor(..., COLOR_BGR2GRAY)` gives red, green
/// and blue in 1/16384ths (BT.601, rounded), which its WebP decoder and its
/// CMYK conversion use too.
const CV_GRAY: [u32; 3] = [4899, 9617, 1868];

/// `OpenCV`'s 8-bit BT.601 gray of a color (`cvtColor`'s fixed point,
/// rounded).
fn cv_gray(red: u8, green: u8, blue: u8) -> u8 {
    let [r, g, b] = CV_GRAY;
    let weighted = u32::from(red) * r + u32::from(green) * g + u32::from(blue) * b;
    u8::try_from((weighted + 8192) >> 14).unwrap_or(u8::MAX)
}

/// An image reduced to an eighth of each side as `OpenCV`'s reduced decode
/// reduces a PNG or a WebP: `resize(..., (width / 8, height / 8),
/// INTER_LINEAR_EXACT)`, its bit-exact bilinear resize. Each destination
/// pixel samples the source at `(x + 0.5) * scale - 0.5` on each axis, with
/// weights in 1/256ths, each row rounded in 8.8 fixed point and the column
/// rounded to 8 bits; for sides that are multiples of 8 that is the
/// rounded mean of the 2 × 2 pixels at the centre of each 8 × 8 block. An
/// image under 8 pixels on a side reduces to nothing, which `OpenCV`
/// refuses (`None`).
fn reduced(gray: &Gray) -> Option<Gray> {
    let (width, height) = (gray.width / REDUCTION, gray.height / REDUCTION);
    if width == 0 || height == 0 {
        return None;
    }
    let columns = exact_weights(gray.width, width);
    let rows = exact_weights(gray.height, height);
    let pixel = |x: usize, y: usize| {
        gray.pixels
            .get(y.checked_mul(gray.width)?.checked_add(x)?)
            .copied()
    };
    // One source row's horizontal pass, in 8.8 fixed point.
    let line = |y: usize, (x, left, right): (usize, u32, u32)| -> Option<u32> {
        let next = (x + 1).min(gray.width - 1);
        Some(left * u32::from(pixel(x, y)?) + right * u32::from(pixel(next, y)?))
    };
    let mut pixels = Vec::with_capacity(width * height);
    for &(y, top, bottom) in &rows {
        let next = (y + 1).min(gray.height - 1);
        for &column in &columns {
            let sum = line(y, column)? * top + line(next, column)? * bottom;
            pixels.push(u8::try_from(((sum + (1 << 15)) >> 16).min(255)).ok()?);
        }
    }
    Some(Gray {
        width,
        height,
        pixels,
    })
}

/// One axis of `OpenCV`'s `INTER_LINEAR_EXACT` resize that shrinks by at
/// least 8: for each destination index, the source index and the weights of
/// it and the next one, in 1/256ths (`interpolationLinear::getCoeffs` with
/// `ufixedpoint16`). The sample never falls outside the source, so the
/// edge cases of the general resize do not arise.
fn exact_weights(source: usize, destination: usize) -> Vec<(usize, u32, u32)> {
    let scale = 1.0 / (real(destination) / real(source));
    (0..destination)
        .map(|at| {
            let sample = scale * (real(at) + 0.5) - 0.5;
            let floor = sample.floor();
            let from = index(floor).min(source.saturating_sub(2));
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a fraction in 0..1, times 256"
            )]
            let right = ((sample - floor) * 256.0).round_ties_even() as u32;
            (from, 256 - right.min(256), right.min(256))
        })
        .collect()
}

/// libpng's fixed point gamma of 1.0 (`PNG_FP_1`).
const PNG_FP_1: i64 = 100_000;

/// The gamma libpng gives an image with an `sRGB` chunk
/// (`PNG_GAMMA_sRGB_INVERSE`).
const PNG_SRGB_GAMMA: i64 = 45_455;

/// libpng's `RGB_TO_GRAY` weights for `png_set_rgb_to_gray(0.299, 0.587)`,
/// which `OpenCV` asks for, in 1/32768ths: red and green truncated, blue the
/// rest.
const PNG_GRAY: [u32; 3] = [9797, 19234, 3737];

/// Whether libpng corrects for a gamma (`png_gamma_significant`: more than
/// 5% from 1.0).
fn png_significant(gamma: i64) -> bool {
    !(PNG_FP_1 - 5_000..=PNG_FP_1 + 5_000).contains(&gamma)
}

/// A double rounded to libpng's fixed point, or 0 when it overflows.
fn png_fixed(value: f64) -> i64 {
    let rounded = (value + 0.5).floor();
    if (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&rounded) {
        #[allow(clippy::cast_possible_truncation, reason = "within i32")]
        let fixed = rounded as i64;
        fixed
    } else {
        0
    }
}

/// libpng's `png_reciprocal`.
fn png_reciprocal(gamma: i64) -> i64 {
    if gamma == 0 {
        return 0;
    }
    #[allow(clippy::cast_precision_loss, reason = "a fixed point gamma")]
    let gamma = gamma as f64;
    png_fixed(1e10 / gamma)
}

/// libpng's `png_reciprocal2`.
fn png_reciprocal2(left: i64, right: i64) -> i64 {
    if left == 0 || right == 0 {
        return 0;
    }
    #[allow(clippy::cast_precision_loss, reason = "fixed point gammas")]
    let (left, right) = (left as f64, right as f64);
    png_fixed(1e15 / left / right)
}

/// libpng's `pow` of a gamma, as its floating point arithmetic computes it.
fn png_pow(base: f64, gamma: i64) -> f64 {
    #[allow(clippy::cast_precision_loss, reason = "a fixed point gamma")]
    let exponent = gamma as f64 * 0.00001;
    base.powf(exponent)
}

/// A double that is a rounded sample as libpng truncates it to an integer.
fn png_sample(value: f64) -> u32 {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a rounded sample within 0..=65535"
    )]
    let sample = value.clamp(0.0, 65535.0) as u32;
    sample
}

/// libpng's 8-bit gamma table (`png_build_8bit_table`).
fn png_table8(gamma: i64) -> Vec<u8> {
    (0..=u8::MAX)
        .map(|value| {
            if png_significant(gamma) && value > 0 && value < u8::MAX {
                let corrected = (255.0 * png_pow(f64::from(value) / 255.0, gamma) + 0.5).floor();
                u8::try_from(png_sample(corrected)).unwrap_or(u8::MAX)
            } else {
                value
            }
        })
        .collect()
}

/// libpng's 16-bit gamma table (`png_build_16bit_table`), indexed by a
/// sample shifted right by `shift`.
fn png_table16(shift: u32, gamma: i64) -> Vec<u32> {
    let max = (1_u32 << (16 - shift)) - 1;
    let fmax = 1.0 / f64::from(max);
    (0..=max)
        .map(|sample| {
            if png_significant(gamma) {
                png_sample((65535.0 * png_pow(f64::from(sample) * fmax, gamma) + 0.5).floor())
            } else if shift != 0 {
                (sample * 65535 + (1 << (15 - shift))) / max
            } else {
                sample
            }
        })
        .collect()
}

/// libpng's table of 16-bit samples to the 16-bit form of their nearest
/// 8-bit value (`png_build_16to8_table`), indexed as [`png_table16`].
fn png_table16to8(shift: u32, gamma: i64) -> Vec<u32> {
    let max = (1_u32 << (16 - shift)) - 1;
    let mut table = Vec::with_capacity(1 << (16 - shift));
    for out in (0..255).map(|value: u32| value * 257) {
        let input = out + 128;
        let bound = if input > 0 && input < 65535 {
            png_sample((65535.0 * png_pow(f64::from(input) / 65535.0, gamma) + 0.5).floor())
        } else {
            input
        };
        let bound = (bound * max + 32768) / 65535 + 1;
        while u32::try_from(table.len()).is_ok_and(|last| last < bound) {
            table.push(out);
        }
    }
    table.resize(1 << (16 - shift), 65535);
    table
}

/// The gamma tables libpng builds for `RGB_TO_GRAY` when an image's gamma is
/// significant, at the image's bit depth.
enum PngGamma {
    /// No significant gamma: the plain weighted sum.
    None,
    /// 8-bit samples: to linear, from linear, and the overall correction.
    Eight([Vec<u8>; 3]),
    /// 16-bit samples: the shift that indexes the tables, and to linear,
    /// from linear, and the overall correction to 8 bits.
    Sixteen(u32, [Vec<u32>; 3]),
}

impl PngGamma {
    /// libpng's tables for an image (`png_init_gamma_values` and
    /// `png_build_gamma_table`): the file gamma is the `sRGB` chunk's, else
    /// the `gAMA` chunk's (PNG v3's precedence; libpng reads no `cICP` or
    /// `iCCP` gamma), and the screen gamma, which `OpenCV` never sets, is its
    /// reciprocal.
    fn of(info: &png::Info<'_>) -> Self {
        let chunk = if info.srgb.is_some() {
            PNG_SRGB_GAMMA
        } else {
            info.gama_chunk
                .map_or(0, |gamma| i64::from(gamma.into_scaled()))
        };
        let (file, screen) = if chunk > 0 {
            (chunk, png_reciprocal(chunk))
        } else {
            (PNG_FP_1, PNG_FP_1)
        };
        if !png_significant(file) && !png_significant(screen) {
            return Self::None;
        }
        let to_linear = png_reciprocal(file);
        let from_linear = png_reciprocal(screen);
        let correction = png_reciprocal2(screen, file);
        if info.bit_depth != png::BitDepth::Sixteen {
            return Self::Eight([
                png_table8(to_linear),
                png_table8(from_linear),
                png_table8(correction),
            ]);
        }
        // The significant bits (`sBIT`, the most of red, green and blue),
        // at least 5 dropped because the samples end in 8 bits, at most 8.
        let significant = info
            .sbit
            .as_deref()
            .map_or(0, |bits| bits.iter().take(3).copied().max().unwrap_or(0));
        let shift = if significant > 0 && significant < 16 {
            16 - u32::from(significant)
        } else {
            0
        }
        .clamp(5, 8);
        Self::Sixteen(
            shift,
            [
                png_table16(shift, to_linear),
                png_table16(shift, from_linear),
                png_table16to8(shift, png_reciprocal(correction)),
            ],
        )
    }

    /// The 8-bit gray of an 8-bit color (`png_do_rgb_to_gray`).
    fn gray8(&self, [red, green, blue]: [u8; 3]) -> Option<u8> {
        let [r, g, b] = PNG_GRAY;
        let Self::Eight([to_linear, from_linear, correction]) = self else {
            // The historical sum, truncated.
            let weighted = r * u32::from(red) + g * u32::from(green) + b * u32::from(blue);
            return u8::try_from(weighted >> 15).ok();
        };
        if red == green && red == blue {
            return correction.get(usize::from(red)).copied();
        }
        let linear = |value: u8| to_linear.get(usize::from(value)).copied().map(u32::from);
        let weighted = r * linear(red)? + g * linear(green)? + b * linear(blue)?;
        from_linear
            .get(usize::try_from((weighted + 16384) >> 15).ok()?)
            .copied()
    }

    /// The 8-bit gray of a 16-bit color: `png_do_rgb_to_gray` at 16 bits,
    /// rounded, then cut to the high byte (`png_set_strip_16`).
    fn gray16(&self, [red, green, blue]: [u32; 3]) -> Option<u8> {
        let [r, g, b] = PNG_GRAY;
        let wide = match self {
            Self::Sixteen(shift, [to_linear, from_linear, correction]) => {
                let at = |table: &Vec<u32>, value: u32| {
                    table.get(usize::try_from(value >> shift).ok()?).copied()
                };
                if red == green && red == blue {
                    at(correction, red)?
                } else {
                    let weighted = r * at(to_linear, red)?
                        + g * at(to_linear, green)?
                        + b * at(to_linear, blue)?;
                    at(from_linear, (weighted + 16384) >> 15)?
                }
            }
            _ => (r * red + g * green + b * blue + 16384) >> 15,
        };
        u8::try_from(wide >> 8).ok()
    }
}

/// A PNG decoded in 8-bit grayscale as `OpenCV`'s (libpng's) decoder does:
/// a palette and gray under 8 bits expanded, alpha dropped, color reduced
/// as libpng's `png_set_rgb_to_gray(0.299, 0.587)` reduces it, through the
/// image's gamma when that is significant ([`PngGamma`]), and 16-bit
/// samples cut to their high byte after that.
fn png_gray(image: &[u8]) -> Option<(Gray, Option<u16>)> {
    let mut decoder = png::Decoder::new(Cursor::new(image));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().ok()?;
    let width = usize::try_from(reader.info().width).ok()?;
    let height = usize::try_from(reader.info().height).ok()?;
    if !bounded(width, height) {
        return None;
    }
    let gamma = PngGamma::of(reader.info());
    let orientation = reader
        .info()
        .exif_metadata
        .as_deref()
        .and_then(exif::orientation);
    let mut buffer = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut buffer).ok()?;
    let wide = frame.bit_depth == png::BitDepth::Sixteen;
    let bytes = if wide { 2 } else { 1 };
    let channels = frame.color_type.samples();
    let mut pixels = Vec::with_capacity(width * height);
    for row in buffer.chunks(frame.line_size).take(height) {
        for sample in row.chunks(channels * bytes).take(width) {
            let value = |at: usize| -> Option<u32> {
                if wide {
                    let high = *sample.get(2 * at)?;
                    let low = *sample.get(2 * at + 1)?;
                    Some(u32::from(u16::from_be_bytes([high, low])))
                } else {
                    sample.get(at).copied().map(u32::from)
                }
            };
            pixels.push(match channels {
                1 | 2 if wide => u8::try_from(value(0)? >> 8).ok()?,
                1 | 2 => u8::try_from(value(0)?).ok()?,
                3 | 4 if wide => gamma.gray16([value(0)?, value(1)?, value(2)?])?,
                3 | 4 => gamma.gray8([
                    u8::try_from(value(0)?).ok()?,
                    u8::try_from(value(1)?).ok()?,
                    u8::try_from(value(2)?).ok()?,
                ])?,
                _ => return None,
            });
        }
    }
    (pixels.len() == width * height).then_some((
        Gray {
            width,
            height,
            pixels,
        },
        orientation,
    ))
}

/// A WebP decoded in 8-bit grayscale as `OpenCV`'s decoder does: decoded to
/// color (libwebp's `WebPDecodeBGRInto`, here `image-webp`), alpha dropped,
/// and reduced with `cvtColor`'s BT.601 weights; and the orientation of its
/// first `EXIF` chunk, which `OpenCV`'s WebP decoder always reads. An
/// animated WebP is read as its first frame.
fn webp_gray(image: &[u8]) -> Option<(Gray, Option<u16>)> {
    let mut decoder = image_webp::WebPDecoder::new(Cursor::new(image)).ok()?;
    let (width, height) = decoder.dimensions();
    let (width, height) = (usize::try_from(width).ok()?, usize::try_from(height).ok()?);
    if !bounded(width, height) {
        return None;
    }
    let channels = if decoder.has_alpha() { 4 } else { 3 };
    let mut buffer = vec![0; decoder.output_buffer_size()?];
    decoder.read_image(&mut buffer).ok()?;
    let pixels: Vec<u8> = buffer
        .chunks_exact(channels)
        .map(|sample| match *sample {
            [red, green, blue, ..] => cv_gray(red, green, blue),
            _ => 0,
        })
        .collect();
    (pixels.len() == width * height).then_some((
        Gray {
            width,
            height,
            pixels,
        },
        webp_orientation(image),
    ))
}

/// The orientation of an extended WebP's first `EXIF` chunk: its RIFF
/// chunks after `VP8X`, each a code, a little-endian size, and its data
/// padded to an even length (as libwebp's demuxer reads them).
fn webp_orientation(image: &[u8]) -> Option<u16> {
    if image.get(12..16) != Some(b"VP8X".as_slice()) {
        return None;
    }
    let mut at: usize = 12;
    while let Some(header) = image.get(at..at.checked_add(8)?) {
        let (code, size) = header.split_at(4);
        let size = usize::try_from(u32::from_le_bytes(size.try_into().ok()?)).ok()?;
        let data = image.get(at + 8..(at + 8).checked_add(size)?)?;
        if code == b"EXIF" {
            return exif::orientation(data);
        }
        at = at + 8 + size + size % 2;
    }
    None
}

/// `OpenCV`'s rounding of a `float` sum to 8 bits (`saturate_cast<uchar>`:
/// to nearest, ties to even).
fn saturated(value: f32) -> u8 {
    let rounded = value.round_ties_even().clamp(0.0, 255.0);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=255 first"
    )]
    let byte = rounded as u8;
    byte
}

/// `value` (non-negative, within an image side) as an index.
fn index(value: f64) -> usize {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "callers pass a non-negative whole number within an image side"
    )]
    let index = value.max(0.0) as usize;
    index
}

/// `value` as an `f64` (image sides are bounded far below 2^52).
fn real(value: usize) -> f64 {
    #[allow(clippy::cast_precision_loss, reason = "bounded far below 2^52")]
    let real = value as f64;
    real
}

/// `value` as `OpenCV`'s `float` weight.
fn narrow(value: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation, reason = "OpenCV's float weights")]
    let narrow = value as f32;
    narrow
}

/// One axis of an area resize that shrinks: for each destination index,
/// the source indices it covers and their weights (`OpenCV`'s
/// `computeResizeAreaTab`).
fn area_weights(source: usize, destination: usize) -> Vec<Vec<(usize, f32)>> {
    let scale = real(source) / real(destination);
    (0..destination)
        .map(|at| {
            let start = real(at) * scale;
            let end = start + scale;
            let cell = scale.min(real(source) - start);
            let last = index(end.floor()).min(source.saturating_sub(1));
            let first = index(start.ceil()).min(last);
            let mut weights = Vec::new();
            if real(first) - start > 1e-3
                && let Some(before) = first.checked_sub(1)
            {
                weights.push((before, narrow((real(first) - start) / cell)));
            }
            for covered in first..last {
                weights.push((covered, narrow(1.0 / cell)));
            }
            if end - real(last) > 1e-3 {
                weights.push((last, narrow((end - real(last)).min(1.0).min(cell) / cell)));
            }
            weights
        })
        .collect()
}

/// One axis of `OpenCV`'s `INTER_AREA` resize when it does not shrink on
/// both axes: for each destination index, the source index and the
/// weights of it and the next one, in `OpenCV`'s fixed point
/// (`INTER_RESIZE_COEF_SCALE`, 2048).
fn linear_weights(source: usize, destination: usize) -> Vec<(usize, i32, i32)> {
    let scale = real(source) / real(destination);
    let inverse = real(destination) / real(source);
    let fixed = |weight: f32| {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "a weight in 0..=1, times 2048"
        )]
        let fixed = (weight * 2048.0).round_ties_even() as i32;
        fixed
    };
    (0..destination)
        .map(|at| {
            let floor = (real(at) * scale).floor();
            let mut fraction = narrow(real(at) + 1.0 - (floor + 1.0) * inverse);
            fraction = if fraction <= 0.0 {
                0.0
            } else {
                fraction - fraction.floor()
            };
            let mut from = index(floor);
            if from + 1 >= source {
                fraction = 0.0;
                from = source.saturating_sub(1);
            }
            (from, fixed(1.0 - fraction), fixed(fraction))
        })
        .collect()
}

/// The grid of a grayscale image: `OpenCV`'s `INTER_AREA` resize to
/// [`FRAME_SIZE`] × [`FRAME_SIZE`], rounded to 8 bits. An image that
/// shrinks on both axes is area averaged (exactly, by whole cells, when
/// both sides are multiples of [`FRAME_SIZE`]); any other is resized with
/// `OpenCV`'s bilinear form of it.
fn area_resized(gray: &Gray) -> Option<Vec<u8>> {
    let (width, height) = (gray.width, gray.height);
    let pixel = |x: usize, y: usize| {
        gray.pixels
            .get(y.checked_mul(width)?.checked_add(x)?)
            .copied()
    };
    let mut grid = Vec::with_capacity(FRAME_VALUES);
    if width % FRAME_SIZE == 0 && height % FRAME_SIZE == 0 {
        // OpenCV's `resizeAreaFast`: the cell's integer sum times the
        // reciprocal of its area, or, for 2 × 2 cells, rounded half up.
        let (cell_width, cell_height) = (width / FRAME_SIZE, height / FRAME_SIZE);
        let area = cell_width * cell_height;
        let reciprocal = narrow(1.0 / real(area));
        for row in 0..FRAME_SIZE {
            for column in 0..FRAME_SIZE {
                let mut sum = 0_u32;
                for y in row * cell_height..(row + 1) * cell_height {
                    for x in column * cell_width..(column + 1) * cell_width {
                        sum += u32::from(pixel(x, y)?);
                    }
                }
                grid.push(if area == 4 {
                    u8::try_from((sum + 2) >> 2).ok()?
                } else {
                    #[allow(clippy::cast_precision_loss, reason = "at most 2^24 * 255")]
                    let sum = sum as f32;
                    saturated(sum * reciprocal)
                });
            }
        }
    } else if width >= FRAME_SIZE && height >= FRAME_SIZE {
        let columns = area_weights(width, FRAME_SIZE);
        let rows = area_weights(height, FRAME_SIZE);
        for row in &rows {
            for column in &columns {
                let mut sum = 0.0_f32;
                for &(y, row_weight) in row {
                    let mut line = 0.0_f32;
                    for &(x, column_weight) in column {
                        line += column_weight * f32::from(pixel(x, y)?);
                    }
                    sum += row_weight * line;
                }
                grid.push(saturated(sum));
            }
        }
    } else {
        let columns = linear_weights(width, FRAME_SIZE);
        let rows = linear_weights(height, FRAME_SIZE);
        for &(y, top, bottom) in &rows {
            let next_y = (y + 1).min(height - 1);
            for &(x, left, right) in &columns {
                let next_x = (x + 1).min(width - 1);
                let line = |y| -> Option<i32> {
                    Some(left * i32::from(pixel(x, y)?) + right * i32::from(pixel(next_x, y)?))
                };
                // OpenCV's vectorized vertical pass: each row cut to 16
                // bits, multiplied keeping the high half, then rounded.
                let sum = (((line(y)? >> 4) * top) >> 16) + (((line(next_y)? >> 4) * bottom) >> 16);
                grid.push(u8::try_from(((sum + 2) >> 2).clamp(0, 255)).ok()?);
            }
        }
    }
    Some(grid)
}

/// The grid blurred with `OpenCV`'s 3 × 3 Gaussian kernel for sigma 0,
/// `[1, 2, 1] / 4` each way, the border reflected about its edge pixel
/// (`BORDER_REFLECT_101`), rounded to 8 bits (`OpenCV`'s bit-exact 8-bit
/// Gaussian blur).
fn blurred(grid: &[u8]) -> Vec<u8> {
    let reflect = |index: usize, offset: isize| -> usize {
        match index.checked_add_signed(offset) {
            Some(inside) if inside < FRAME_SIZE => inside,
            Some(_) => FRAME_SIZE - 2,
            None => 1,
        }
    };
    let at = |x: usize, y: usize| u32::from(grid.get(y * FRAME_SIZE + x).copied().unwrap_or(0));
    let mut out = Vec::with_capacity(FRAME_VALUES);
    for y in 0..FRAME_SIZE {
        for x in 0..FRAME_SIZE {
            let mut sum = 0;
            for (dy, row_weight) in [(-1, 1), (0, 2), (1, 1)] {
                for (dx, column_weight) in [(-1, 1), (0, 2), (1, 1)] {
                    sum += row_weight * column_weight * at(reflect(x, dx), reflect(y, dy));
                }
            }
            out.push(u8::try_from((sum + 8) >> 4).unwrap_or(u8::MAX));
        }
    }
    out
}

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    wit_bindgen::generate!({
        world: "invoked-service",
        path: "../../scxml.rs/crates/scxml-wit/wit/scxml",
        // The world's `wasi:clocks` interfaces too.
        generate_all,
    });

    use std::io::Read as _;

    use exports::stateforward::scxml::invoked_service_api::Guest;
    use stateforward::scxml::telemetry::{self, Operation, Outcome, SignalEvent};
    use stateforward::scxml::types::{
        ComponentError, DataValue, ErrorCode, ErrorVersion, Event, InvokeRequest,
    };

    use super::{Parameter, VisionError, VisionRequest};

    /// The invocation's result: `ok` with its done data, or its error.
    type Done = Result<Option<DataValue>, ComponentError>;

    struct Component;

    #[allow(
        clippy::unused_async_trait_impl,
        reason = "the WIT `invoke` is an `async func`; it returns once its task is spawned"
    )]
    impl Guest for Component {
        async fn invoke(
            request: InvokeRequest,
            events: wit_bindgen::StreamReader<Event>,
        ) -> Result<
            (
                wit_bindgen::StreamReader<Event>,
                wit_bindgen::FutureReader<Done>,
            ),
            ComponentError,
        > {
            // The service reads no event.
            drop(events);
            let result = run(&request);
            let outcome = match &result {
                Ok(_) => Outcome::Ok,
                Err(failure) if failure.code == ErrorCode::InvalidInput => Outcome::Rejected,
                Err(_) => Outcome::Failed,
            };
            telemetry::observe(Operation::Start, outcome, None, None);
            telemetry::event(
                Operation::Start,
                if result.is_ok() {
                    SignalEvent::TransformationCompleted
                } else {
                    SignalEvent::InputRejected
                },
                None,
            );
            let data = result?;
            // No event; the future resolves at once with the done data.
            let (returned, returned_reader) = wit_stream::new::<Event>();
            drop(returned);
            let (done, done_reader) = wit_future::new::<Done>(|| {
                Err(error(
                    ErrorCode::Platform,
                    "the service ended without a result",
                ))
            });
            wit_bindgen::spawn(async move {
                let _ = done.write(Ok(Some(DataValue::JsonObject(data)))).await;
            });
            Ok((returned_reader, done_reader))
        }
    }

    /// Reads and measures the image as the request asks; the done data.
    fn run(request: &InvokeRequest) -> Result<String, ComponentError> {
        if request.type_uri != super::SERVICE_URI {
            return Err(error(ErrorCode::InvalidInput, "service type URI differs"));
        }
        let vision = VisionRequest::from_parameters(request.parameters.iter().map(|entry| {
            let value = match &entry.value {
                DataValue::Text(text) => Parameter::Text(text),
                DataValue::JsonObject(text) => Parameter::Object(text),
                DataValue::Null | DataValue::Absent => Parameter::Null,
                _ => Parameter::Other,
            };
            (entry.name.as_str(), value)
        }))
        .map_err(vision_error)?;
        let image = read(&vision.path)?;
        let measurement = vision.measured(&image).map_err(vision_error)?;
        Ok(measurement.to_json())
    }

    /// The image's bytes, at most one past [`super::MAX_IMAGE_BYTES`] (so
    /// an over-bound file is told apart).
    fn read(path: &str) -> Result<Vec<u8>, ComponentError> {
        let unreadable = || vision_error(VisionError::Unreadable);
        let file = std::fs::File::open(path).map_err(|_| unreadable())?;
        let limit = u64::try_from(super::MAX_IMAGE_BYTES).unwrap_or(u64::MAX);
        let mut bytes = Vec::new();
        file.take(limit.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| unreadable())?;
        Ok(bytes)
    }

    fn vision_error(failure: VisionError) -> ComponentError {
        let code = match failure {
            VisionError::InvalidParameter
            | VisionError::MismatchedPrevious
            | VisionError::Undecodable => ErrorCode::InvalidInput,
            VisionError::Unreadable => ErrorCode::Unavailable,
            VisionError::TooLarge => ErrorCode::PayloadTooLarge,
        };
        error(code, failure.detail())
    }

    fn error(code: ErrorCode, detail: &str) -> ComponentError {
        ComponentError {
            version: ErrorVersion::V1,
            code,
            detail: Some(detail.to_owned()),
        }
    }

    export!(Component);
}

mod exif;
mod jpeg;

#[cfg(test)]
mod tests;
