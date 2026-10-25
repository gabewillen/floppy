//! The voice operations: voice activity detection, diarization and voice
//! identification on `LocalAI`'s `OpenAI`-compatible voice API, and the
//! three steps the interpretation takes on PCM (normalizing a sound,
//! joining the open speech, clipping a voice span), which floppy, moving
//! media by reference, takes here on files.
//!
//! `LocalAI` serves the voice models (Silero VAD, pyannote segmentation and
//! Sortformer diarization, `WeSpeaker` embeddings) over HTTP, so the run
//! chooses one of its endpoints through the `baseurl` slot:
//!
//! * `detect`: `POST {base}/vad` with the canonical JSON body
//!   `{"audio": [f32...], "model"}` (16 kHz mono samples in `[-1, 1]`), answered
//!   `{"segments": [{"start", "end"}]}` in seconds ([`detection_request`]);
//! * `diarize`: `POST {base}/audio/diarization`, a `multipart/form-data` body
//!   of `model`, `response_format` (`json`), and `file` (a 16-bit PCM WAV
//!   file), answered `{"segments": [{"start", "end", ...}]}`
//!   ([`diarization_request`]);
//! * `identify`: one `POST {base}/voice/embed` per segment with the canonical
//!   JSON body `{"audio": <base64 WAV>, "model"}`, answered
//!   `{"embedding": [...]}` ([`embedding_request`]).
//!
//! The media steps (`normalize`, `append`, `clip`) call no backend: they read
//! and write files in the granted directory and complete with done data.

use scxml_event_schema::canonical_json;
use scxml_types::{DataValue as Data, exceeds_wasm_bound, wasm_bound};
use serde_json::{Value as Json, json};

use super::{
    FailureCode, MAX_AUDIO_BYTES, MAX_MODEL_BYTES, Media, Part, Request, Response, Returned,
    TransportFailure, answered_body, bearer, boundary, file_name, media_parameter,
};

/// The voice detection path appended to the base URL (`LocalAI`'s `/v1/vad`).
pub const VAD_PATH: &str = "/vad";
/// The voice diarization path appended to the base URL.
pub const DIARIZATION_PATH: &str = "/audio/diarization";
/// The voice embedding path appended to the base URL.
pub const EMBED_PATH: &str = "/voice/embed";

/// The rate voice detection samples are sent at: Silero VAD's 16 kHz.
pub const VAD_SAMPLE_RATE_HZ: u32 = 16_000;
/// The channel count of raw PCM that names none: mono (voice detection
/// also reads raw PCM that names no rate at [`VAD_SAMPLE_RATE_HZ`]).
pub const DEFAULT_CHANNELS: u16 = 1;

/// The voice spans of one clip.
pub const DETECTION_OUTPUT_EVENT: &str = "bot.ability.hearing.voice.detection.output";
/// The clipped voice segments.
pub const DIARIZATION_OUTPUT_EVENT: &str = "bot.ability.hearing.voice.diarization.output";
/// The voice embeddings.
pub const IDENTIFICATION_OUTPUT_EVENT: &str = "bot.ability.hearing.voice.identification.output";

/// Upper bound on the frames an `append` joins and the segments an
/// `identify` embeds (wasm builds only).
pub const MAX_ITEMS: usize = wasm_bound(256);
/// Upper bound on a sample rate (8 times 96 kHz).
pub const MAX_SAMPLE_RATE_HZ: u32 = 768_000;
/// Upper bound on a channel count.
pub const MAX_CHANNELS: u16 = 64;

/// The media types read as a WAV container.
const WAV_TYPES: [&str; 3] = ["audio/wav", "audio/wave", "audio/x-wav"];

/// A sound's format, each field only when known.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Format {
    /// Samples per second.
    pub sample_rate_hz: Option<u32>,
    /// Interleaved channels.
    pub channels: Option<u16>,
}

/// A validated voice detection: the clip and its format.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Detection {
    /// The model name.
    pub model: String,
    /// The clip: a WAV file, or raw 16-bit PCM.
    pub audio: Media,
    /// Its media type, when given.
    pub media_type: Option<String>,
    /// Its raw PCM format, when given.
    pub format: Format,
}

/// A validated voice diarization: the utterance, its format, and where its
/// segments are written (`<output>-<index>`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diarization {
    /// The model name.
    pub model: String,
    /// The utterance: a WAV file, or raw 16-bit PCM.
    pub audio: Media,
    /// Its raw PCM format, when given.
    pub format: Format,
    /// The reference each segment's file name extends.
    pub output: Media,
}

/// One voice segment: its clipped PCM and span.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    /// The clipped raw 16-bit PCM.
    pub audio: Media,
    /// Samples per second.
    pub sample_rate_hz: u32,
    /// Interleaved channels.
    pub channels: u16,
    /// Inclusive start in the source audio, in seconds.
    pub start_seconds: f64,
    /// Exclusive end in the source audio, in seconds.
    pub end_seconds: f64,
}

/// A validated voice identification: the segments to embed, in order.
#[derive(Clone, Debug, PartialEq)]
pub struct Identification {
    /// The model name.
    pub model: String,
    /// The segments.
    pub segments: Vec<Segment>,
}

/// A validated normalization: the sound and where its PCM is written when
/// it is a WAV container.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Normalization {
    /// The sound.
    pub audio: Media,
    /// Its media type, when given.
    pub media_type: Option<String>,
    /// Its format, when given.
    pub format: Format,
    /// Where a WAV container's PCM is written.
    pub output: Media,
}

/// One frame an `append` joins: its PCM and format.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    /// The raw 16-bit PCM.
    pub audio: Media,
    /// Its format, when known.
    pub format: Format,
}

/// A validated append (over the whole open speech at once): the frames, in
/// order, and the utterance file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Appending {
    /// The frames.
    pub frames: Vec<Frame>,
    /// The utterance's file.
    pub output: Media,
    /// Whether the utterance is written as a WAV container (the `wav`
    /// parameter), else as raw PCM.
    pub wav: bool,
}

/// A validated clip.
#[derive(Clone, Debug, PartialEq)]
pub struct Clipping {
    /// The source PCM.
    pub audio: Media,
    /// Its sample rate.
    pub sample_rate_hz: u32,
    /// Its channels.
    pub channels: u16,
    /// The span's start, in seconds.
    pub start_seconds: f64,
    /// The span's end, in seconds.
    pub end_seconds: f64,
    /// The segment's file.
    pub output: Media,
}

/// The parameters a voice operation reads, each `None` when not given.
#[derive(Default)]
pub(crate) struct Given<'a> {
    pub model: Option<String>,
    pub audio: Option<&'a Data>,
    pub media_type: Option<&'a Data>,
    pub sample_rate_hz: Option<&'a Data>,
    pub channels: Option<&'a Data>,
    pub output: Option<&'a Data>,
    pub frames: Option<&'a Data>,
    pub segments: Option<&'a Data>,
    pub wav: Option<&'a Data>,
    pub start_seconds: Option<&'a Data>,
    pub end_seconds: Option<&'a Data>,
}

impl Given<'_> {
    /// The names of the parameters given, in a fixed order.
    fn names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        for (name, given) in [
            ("audio", self.audio.is_some()),
            ("media_type", self.media_type.is_some()),
            ("sample_rate_hz", self.sample_rate_hz.is_some()),
            ("channels", self.channels.is_some()),
            ("output", self.output.is_some()),
            ("frames", self.frames.is_some()),
            ("segments", self.segments.is_some()),
            ("wav", self.wav.is_some()),
            ("start_seconds", self.start_seconds.is_some()),
            ("end_seconds", self.end_seconds.is_some()),
        ] {
            if given {
                names.push(name);
            }
        }
        names
    }

    /// Refuses any parameter outside `allowed`.
    fn only(&self, allowed: &[&str], refusal: &'static str) -> Result<(), &'static str> {
        if self.names().iter().all(|name| allowed.contains(name)) {
            Ok(())
        } else {
            Err(refusal)
        }
    }

    fn model(&self) -> Result<String, &'static str> {
        self.model
            .clone()
            .ok_or("speech requires a model parameter")
    }
}

fn number(value: &Data) -> Option<f64> {
    match value {
        Data::Number(number) => Some(number.get()),
        _ => None,
    }
}

fn json_number(value: &Json) -> Option<f64> {
    value.as_f64().filter(|value| value.is_finite())
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "checked integral and in range first"
)]
fn integer_in(value: f64, max: u32) -> Option<u32> {
    (value.fract() == 0.0 && value >= 1.0 && value <= f64::from(max)).then_some(value as u32)
}

fn rate(value: Option<&Data>) -> Result<Option<u32>, &'static str> {
    value
        .map(|value| {
            number(value)
                .and_then(|value| integer_in(value, MAX_SAMPLE_RATE_HZ))
                .ok_or("speech sample_rate_hz must be a positive integer")
        })
        .transpose()
}

fn channel_count(value: Option<&Data>) -> Result<Option<u16>, &'static str> {
    value
        .map(|value| {
            number(value)
                .and_then(|value| integer_in(value, u32::from(MAX_CHANNELS)))
                .and_then(|value| u16::try_from(value).ok())
                .ok_or("speech channels must be a positive integer")
        })
        .transpose()
}

fn seconds(value: Option<&Data>, missing: &'static str) -> Result<f64, &'static str> {
    number(value.ok_or(missing)?)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or("speech seconds must be non-negative numbers")
}

fn optional_media_type(value: Option<&Data>) -> Result<Option<String>, &'static str> {
    match value {
        None => Ok(None),
        Some(Data::String(media_type)) if super::is_media_type(media_type) => {
            Ok(Some(media_type.clone()))
        }
        Some(_) => Err("speech media_type must be a type/subtype"),
    }
}

fn audio(given: &Given<'_>, operation: &'static str) -> Result<Media, &'static str> {
    media_parameter(
        given.audio.ok_or(operation)?,
        "speech audio must be a file reference",
    )
}

fn output(given: &Given<'_>, missing: &'static str) -> Result<Media, &'static str> {
    media_parameter(
        given.output.ok_or(missing)?,
        "speech output must be a file reference",
    )
}

fn json_array(value: &Data, invalid: &'static str) -> Result<Vec<Json>, &'static str> {
    let Data::JsonArray(text) = value else {
        return Err(invalid);
    };
    match serde_json::from_str::<Json>(text) {
        Ok(Json::Array(items))
            if !items.is_empty() && !exceeds_wasm_bound(items.len(), MAX_ITEMS) =>
        {
            Ok(items)
        }
        _ => Err(invalid),
    }
}

fn reference(value: Option<&Json>, invalid: &'static str) -> Result<Media, &'static str> {
    let Some(Json::String(reference)) = value else {
        return Err(invalid);
    };
    Ok(Media {
        reference: reference.clone(),
        path: super::media_path(reference)?,
    })
}

fn field_rate(value: Option<&Json>, invalid: &'static str) -> Result<Option<u32>, &'static str> {
    match value {
        None | Some(Json::Null) => Ok(None),
        Some(value) => json_number(value)
            .and_then(|value| integer_in(value, MAX_SAMPLE_RATE_HZ))
            .map(Some)
            .ok_or(invalid),
    }
}

fn field_channels(
    value: Option<&Json>,
    invalid: &'static str,
) -> Result<Option<u16>, &'static str> {
    match value {
        None | Some(Json::Null) => Ok(None),
        Some(value) => json_number(value)
            .and_then(|value| integer_in(value, u32::from(MAX_CHANNELS)))
            .and_then(|value| u16::try_from(value).ok())
            .map(Some)
            .ok_or(invalid),
    }
}

fn checked_model(model: String) -> Result<String, &'static str> {
    if exceeds_wasm_bound(model.len(), MAX_MODEL_BYTES) {
        Err("speech model exceeds its bound")
    } else {
        Ok(model)
    }
}

/// `detect`: `model`, `audio`, and optionally `media_type`,
/// `sample_rate_hz`, and `channels`.
pub(crate) fn detection(given: &Given<'_>) -> Result<Detection, &'static str> {
    given.only(
        &["audio", "media_type", "sample_rate_hz", "channels"],
        "speech detect takes model, audio, media_type, sample_rate_hz, and channels only",
    )?;
    Ok(Detection {
        model: checked_model(given.model()?)?,
        audio: audio(given, "speech detect requires an audio parameter")?,
        media_type: optional_media_type(given.media_type)?,
        format: Format {
            sample_rate_hz: rate(given.sample_rate_hz)?,
            channels: channel_count(given.channels)?,
        },
    })
}

/// `diarize`: `model`, `audio`, `output`, and optionally `media_type`,
/// `sample_rate_hz`, and `channels`.
pub(crate) fn diarization(given: &Given<'_>) -> Result<Diarization, &'static str> {
    given.only(
        &[
            "audio",
            "media_type",
            "sample_rate_hz",
            "channels",
            "output",
        ],
        "speech diarize takes model, audio, media_type, sample_rate_hz, channels, and output only",
    )?;
    optional_media_type(given.media_type)?;
    Ok(Diarization {
        model: checked_model(given.model()?)?,
        audio: audio(given, "speech diarize requires an audio parameter")?,
        format: Format {
            sample_rate_hz: rate(given.sample_rate_hz)?,
            channels: channel_count(given.channels)?,
        },
        output: output(given, "speech diarize requires an output parameter")?,
    })
}

/// `identify`: `model` and `segments` (voice segments `[{audio,
/// media_type, sample_rate_hz, channels, start_seconds, end_seconds}]`,
/// ascending by start).
pub(crate) fn identification(given: &Given<'_>) -> Result<Identification, &'static str> {
    const INVALID: &str = "speech segments must be voice segments of audio/pcm in ascending order";
    given.only(
        &["segments"],
        "speech identify takes model and segments only",
    )?;
    let items = json_array(
        given
            .segments
            .ok_or("speech identify requires a segments parameter")?,
        INVALID,
    )?;
    let mut segments: Vec<Segment> = Vec::with_capacity(items.len());
    for item in &items {
        let Json::Object(fields) = item else {
            return Err(INVALID);
        };
        if fields.get("media_type") != Some(&json!("audio/pcm")) {
            return Err(INVALID);
        }
        let start_seconds = fields
            .get("start_seconds")
            .and_then(json_number)
            .filter(|value| *value >= 0.0)
            .ok_or(INVALID)?;
        let end_seconds = fields
            .get("end_seconds")
            .and_then(json_number)
            .filter(|value| *value > start_seconds)
            .ok_or(INVALID)?;
        if segments
            .last()
            .is_some_and(|previous| start_seconds < previous.start_seconds)
        {
            return Err(INVALID);
        }
        segments.push(Segment {
            audio: reference(fields.get("audio"), INVALID)?,
            sample_rate_hz: field_rate(fields.get("sample_rate_hz"), INVALID)?.ok_or(INVALID)?,
            channels: field_channels(fields.get("channels"), INVALID)?.ok_or(INVALID)?,
            start_seconds,
            end_seconds,
        });
    }
    Ok(Identification {
        model: checked_model(given.model()?)?,
        segments,
    })
}

/// `normalize`: `audio`, `output`, and optionally `media_type`,
/// `sample_rate_hz`, and `channels`; takes no model.
pub(crate) fn normalization(given: &Given<'_>) -> Result<Normalization, &'static str> {
    given.only(
        &[
            "audio",
            "media_type",
            "sample_rate_hz",
            "channels",
            "output",
        ],
        "speech normalize takes audio, media_type, sample_rate_hz, channels, and output only",
    )?;
    if given.model.is_some() {
        return Err("speech normalize takes no model parameter");
    }
    Ok(Normalization {
        audio: audio(given, "speech normalize requires an audio parameter")?,
        media_type: optional_media_type(given.media_type)?,
        format: Format {
            sample_rate_hz: rate(given.sample_rate_hz)?,
            channels: channel_count(given.channels)?,
        },
        output: output(given, "speech normalize requires an output parameter")?,
    })
}

/// `append`: `frames` (`[{audio, sample_rate_hz, channels}]`, in order),
/// `output`, and optionally `wav`; takes no model.
pub(crate) fn appending(given: &Given<'_>) -> Result<Appending, &'static str> {
    const INVALID: &str = "speech frames must be audio references with their formats";
    given.only(
        &["frames", "output", "wav"],
        "speech append takes frames, output, and wav only",
    )?;
    if given.model.is_some() {
        return Err("speech append takes no model parameter");
    }
    let items = json_array(
        given
            .frames
            .ok_or("speech append requires a frames parameter")?,
        INVALID,
    )?;
    let frames = items
        .iter()
        .map(|item| {
            let Json::Object(fields) = item else {
                return Err(INVALID);
            };
            Ok(Frame {
                audio: reference(fields.get("audio"), INVALID)?,
                format: Format {
                    sample_rate_hz: field_rate(fields.get("sample_rate_hz"), INVALID)?,
                    channels: field_channels(fields.get("channels"), INVALID)?,
                },
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let wav = match given.wav {
        None => false,
        Some(Data::Bool(wav)) => *wav,
        Some(_) => return Err("speech wav must be a boolean"),
    };
    Ok(Appending {
        frames,
        output: output(given, "speech append requires an output parameter")?,
        wav,
    })
}

/// `clip`: `audio`, `sample_rate_hz`, `channels`, `start_seconds`,
/// `end_seconds`, and `output`; takes no model.
pub(crate) fn clipping(given: &Given<'_>) -> Result<Clipping, &'static str> {
    given.only(
        &["audio", "sample_rate_hz", "channels", "start_seconds", "end_seconds", "output"],
        "speech clip takes audio, sample_rate_hz, channels, start_seconds, end_seconds, and output only",
    )?;
    if given.model.is_some() {
        return Err("speech clip takes no model parameter");
    }
    Ok(Clipping {
        audio: audio(given, "speech clip requires an audio parameter")?,
        sample_rate_hz: rate(given.sample_rate_hz)?
            .ok_or("speech clip requires a sample_rate_hz parameter")?,
        channels: channel_count(given.channels)?
            .ok_or("speech clip requires a channels parameter")?,
        start_seconds: seconds(
            given.start_seconds,
            "speech clip requires a start_seconds parameter",
        )?,
        end_seconds: seconds(
            given.end_seconds,
            "speech clip requires an end_seconds parameter",
        )?,
        output: output(given, "speech clip requires an output parameter")?,
    })
}

/// A WAV container's linear PCM: its rate, channels, bits per sample, and
/// the `data` chunk's bytes present (a streamed file's unknown size counts
/// the bytes present). `None` when the bytes are not a WAV container.
#[must_use]
pub fn wav_pcm(audio: &[u8]) -> Option<(u32, u16, u16, u16, &[u8])> {
    if audio.get(..4)? != b"RIFF" || audio.get(8..12)? != b"WAVE" {
        return None;
    }
    let mut format = None;
    let mut offset = 12_usize;
    while let Some(id) = audio.get(offset..offset + 4) {
        let size = usize::try_from(u32::from_le_bytes(
            audio.get(offset + 4..offset + 8)?.try_into().ok()?,
        ))
        .ok()?;
        let body = offset + 8;
        if id == b"fmt " {
            let field = |at: usize| -> Option<u16> {
                Some(u16::from_le_bytes(
                    audio.get(body + at..body + at + 2)?.try_into().ok()?,
                ))
            };
            let rate = u32::from_le_bytes(audio.get(body + 4..body + 8)?.try_into().ok()?);
            format = Some((field(0)?, field(2)?, rate, field(14)?));
        } else if id == b"data" {
            let (tag, channels, rate, bits) = format?;
            let end = body.saturating_add(size).min(audio.len());
            return Some((rate, channels, bits, tag, audio.get(body..end)?));
        }
        offset = body.checked_add(size)?.checked_add(size % 2)?;
    }
    None
}

/// A WAV container's signed 16-bit PCM and format (sample width 2).
///
/// # Errors
///
/// [`FailureCode::Format`] when the container is not integer 16-bit PCM
/// with a rate and channels, or its frames are incomplete.
pub fn wav_16bit(audio: &[u8]) -> Result<(u32, u16, Vec<u8>), FailureCode> {
    let (rate, channels, bits, tag, data) = wav_pcm(audio).ok_or(FailureCode::Format)?;
    if tag != 1 || bits != 16 || rate == 0 || channels == 0 {
        return Err(FailureCode::Format);
    }
    let frame = 2 * usize::from(channels);
    let whole = data.len() - data.len() % frame;
    Ok((rate, channels, data[..whole].to_vec()))
}

/// Signed 16-bit PCM in a canonical 44-byte WAV container.
#[must_use]
pub fn wav_container(pcm: &[u8], sample_rate_hz: u32, channels: u16) -> Vec<u8> {
    let data = u32::try_from(pcm.len()).unwrap_or(u32::MAX);
    let block = channels.saturating_mul(2);
    let mut out = Vec::with_capacity(pcm.len() + 44);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&data.saturating_add(36).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0");
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate_hz.to_le_bytes());
    out.extend_from_slice(
        &sample_rate_hz
            .saturating_mul(u32::from(block))
            .to_le_bytes(),
    );
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&16_u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

fn is_wav_type(media_type: Option<&str>) -> bool {
    media_type.is_some_and(|media_type| {
        WAV_TYPES.contains(&media_type.trim().to_ascii_lowercase().as_str())
    })
}

/// The sound's PCM, rate, and channels: a WAV container declares its own;
/// raw PCM takes `format`, else `default`.
fn pcm_of(
    audio: Vec<u8>,
    format: Format,
    default: Option<(u32, u16)>,
) -> Result<(u32, u16, Vec<u8>), FailureCode> {
    if wav_pcm(&audio).is_some() {
        return wav_16bit(&audio);
    }
    let (rate, channels) = match (format.sample_rate_hz, format.channels, default) {
        (Some(rate), Some(channels), _) => (rate, channels),
        (Some(rate), None, _) => (rate, DEFAULT_CHANNELS),
        (None, channels, Some((rate, default_channels))) => {
            (rate, channels.unwrap_or(default_channels))
        }
        (None, _, None) => return Err(FailureCode::Format),
    };
    if !audio.len().is_multiple_of(2 * usize::from(channels)) {
        return Err(FailureCode::Format);
    }
    Ok((rate, channels, audio))
}

/// 16-bit PCM as mono `f32` samples in `[-1, 1]` at [`VAD_SAMPLE_RATE_HZ`]:
/// channels averaged, then linearly resampled.
#[must_use]
pub fn vad_samples(pcm: &[u8], sample_rate_hz: u32, channels: u16) -> Vec<f32> {
    let channels = usize::from(channels.max(1));
    let mono: Vec<f32> = pcm
        .chunks_exact(2 * channels)
        .map(|frame| {
            let sum: f32 = frame
                .as_chunks::<2>()
                .0
                .iter()
                .map(|sample| f32::from(i16::from_le_bytes(*sample)))
                .sum();
            #[expect(clippy::cast_precision_loss, reason = "at most 64 channels")]
            let mean = sum / channels as f32;
            mean / 32_768.0
        })
        .collect();
    if sample_rate_hz == VAD_SAMPLE_RATE_HZ || mono.is_empty() {
        return mono;
    }
    let ratio = f64::from(sample_rate_hz) / f64::from(VAD_SAMPLE_RATE_HZ);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "sample counts are far below 2^52"
    )]
    let count = ((mono.len() as f64) / ratio).floor() as usize;
    (0..count)
        .map(|index| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "sample counts are far below 2^52"
            )]
            let position = index as f64 * ratio;
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "position is non-negative and below the sample count"
            )]
            let left = position.floor() as usize;
            let right = (left + 1).min(mono.len() - 1);
            #[expect(clippy::cast_possible_truncation, reason = "a fraction in [0, 1)")]
            let weight = (position - position.floor()) as f32;
            mono[left].mul_add(1.0 - weight, mono[right] * weight)
        })
        .collect()
}

fn model_json(model: &str) -> String {
    serde_json::to_string(model).unwrap_or_else(|_| "\"\"".to_owned())
}

fn json_post(base_url: u32, api_key: Option<u32>, path: &str, body: Vec<u8>) -> Request {
    Request {
        url: vec![Part::Reference(base_url), Part::Literal(path.into())],
        headers: bearer(api_key)
            .into_iter()
            .chain([(
                "content-type".into(),
                vec![Part::Literal("application/json".into())],
            )])
            .collect(),
        body,
    }
}

/// The voice detection request: `POST {base}/vad` with the canonical JSON
/// body `{"audio": [samples], "model"}`.
#[must_use]
pub fn detection_request(
    base_url: u32,
    api_key: Option<u32>,
    model: &str,
    samples: &[f32],
) -> Request {
    let mut body = String::with_capacity(samples.len() * 10 + 64);
    body.push_str("{\"audio\":[");
    for (index, sample) in samples.iter().enumerate() {
        if index > 0 {
            body.push(',');
        }
        if *sample == 0.0 {
            body.push('0');
        } else {
            body.push_str(&sample.to_string());
        }
    }
    body.push_str("],\"model\":");
    body.push_str(&model_json(model));
    body.push('}');
    json_post(base_url, api_key, VAD_PATH, body.into_bytes())
}

/// The voice diarization request: `POST {base}/audio/diarization` with a
/// `multipart/form-data` body of `model`, `response_format` (`json`), and
/// `file` (the WAV container).
#[must_use]
pub fn diarization_request(
    base_url: u32,
    api_key: Option<u32>,
    model: &str,
    name: &str,
    wav: &[u8],
) -> Request {
    let boundary = boundary(wav);
    let mut body = Vec::with_capacity(wav.len() + 512);
    for (field, value) in [("model", model), ("response_format", "json")] {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\ncontent-disposition: form-data; name=\"{field}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\ncontent-disposition: form-data; name=\"file\"; filename=\"{name}\"\r\ncontent-type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(wav);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Request {
        url: vec![
            Part::Reference(base_url),
            Part::Literal(DIARIZATION_PATH.into()),
        ],
        headers: bearer(api_key)
            .into_iter()
            .chain([
                (
                    "content-type".into(),
                    vec![Part::Literal(format!(
                        "multipart/form-data; boundary={boundary}"
                    ))],
                ),
                (
                    "accept".into(),
                    vec![Part::Literal("application/json".into())],
                ),
            ])
            .collect(),
        body,
    }
}

/// Standard base64 with padding (`LocalAI` takes raw base64 audio).
#[must_use]
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let triple = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for (index, shift) in [18_u32, 12, 6, 0].into_iter().enumerate() {
            if index <= chunk.len() {
                out.push(char::from(ALPHABET[((triple >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The voice embedding request: `POST {base}/voice/embed` with the canonical
/// JSON body `{"audio": <base64 WAV>, "model"}`.
#[must_use]
pub fn embedding_request(base_url: u32, api_key: Option<u32>, model: &str, wav: &[u8]) -> Request {
    let body = canonical_json(&json!({"audio": base64(wav), "model": model}));
    json_post(base_url, api_key, EMBED_PATH, body.into_bytes())
}

/// Spans of an answer's `segments` (`start`/`end`, or
/// `start_seconds`/`end_seconds`, and an optional `confidence`,
/// `probability`, or `score`).
fn answered_spans(body: &[u8]) -> Result<Vec<(f64, f64, Option<f64>)>, FailureCode> {
    let answer: Json = serde_json::from_slice(body).map_err(|_| FailureCode::Malformed)?;
    let Some(Json::Array(segments)) = answer.get("segments") else {
        return Err(FailureCode::Malformed);
    };
    segments
        .iter()
        .map(|segment| {
            let field = |names: &[&str]| names.iter().find_map(|name| segment.get(*name));
            let start = field(&["start_seconds", "start"])
                .and_then(json_number)
                .ok_or(FailureCode::Malformed)?;
            let end = field(&["end_seconds", "end"])
                .and_then(json_number)
                .ok_or(FailureCode::Malformed)?;
            let confidence = match field(&["confidence", "probability", "score"]) {
                None | Some(Json::Null) => None,
                Some(value) => Some(
                    json_number(value)
                        .filter(|value| (0.0..=1.0).contains(value))
                        .ok_or(FailureCode::Malformed)?,
                ),
            };
            Ok((start, end, confidence))
        })
        .collect()
}

fn duration(pcm_bytes: usize, sample_rate_hz: u32, channels: u16) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "audio byte counts are far below 2^52"
    )]
    let bytes = pcm_bytes as f64;
    bytes / (2.0 * f64::from(channels) * f64::from(sample_rate_hz))
}

/// What a settled voice detection becomes: [`DETECTION_OUTPUT_EVENT`] with
/// the clip-local spans (each clamped into the clip, empty ones dropped, in
/// ascending start order), or
/// [`super::FAILED_EVENT`].
#[must_use]
pub fn detection_outcome(
    direction: &'static str,
    answered: Result<Response, TransportFailure>,
    clip_seconds: f64,
) -> Returned {
    let spans = answered_body(answered).and_then(|body| answered_spans(&body));
    match spans {
        Ok(spans) => {
            let mut segments: Vec<(f64, f64, Option<f64>)> = spans
                .into_iter()
                .map(|(start, end, confidence)| {
                    (
                        start.clamp(0.0, clip_seconds),
                        end.clamp(0.0, clip_seconds),
                        confidence,
                    )
                })
                .filter(|(start, end, _)| end > start)
                .collect();
            segments.sort_by(|left, right| left.0.total_cmp(&right.0));
            let segments: Vec<Json> = segments
                .into_iter()
                .map(|(start, end, confidence)| {
                    json!({"start_seconds": start, "end_seconds": end, "confidence": confidence})
                })
                .collect();
            Returned {
                name: DETECTION_OUTPUT_EVENT,
                data: canonical_json(&json!({ "segments": segments })),
            }
        }
        Err(code) => Returned::failed_named(direction, code),
    }
}

/// Clips `[start, end)` of 16-bit PCM by whole frames (floor of the start,
/// ceiling of the end); `None` when the span holds no frame.
#[must_use]
pub fn clip_pcm(
    pcm: &[u8],
    sample_rate_hz: u32,
    channels: u16,
    start: f64,
    end: f64,
) -> Option<&[u8]> {
    let frame = 2 * usize::from(channels);
    let rate = f64::from(sample_rate_hz);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "non-negative and bounded by the audio's frame count"
    )]
    let (start_frame, end_frame) = (
        (start * rate).floor() as usize,
        (end * rate).ceil() as usize,
    );
    let frames = pcm.len() / frame;
    let slice = pcm.get(start_frame.min(frames) * frame..end_frame.min(frames) * frame)?;
    (!slice.is_empty()).then_some(slice)
}

/// The reference of the `index`th file under `output`: `<output>-<index>`.
#[must_use]
pub fn indexed(output: &Media, index: usize) -> Media {
    Media {
        reference: format!("{}-{index}", output.reference),
        path: format!("{}-{index}", output.path),
    }
}

/// How far past the utterance a diarized segment may end and still be cut
/// to the utterance's end: pyannote rounds a segment's end up to its frame
/// grid, and api.willen.dev's answers end up to 0.08 s past the audio they
/// were given (decision D244).
pub const DIARIZATION_END_SLACK_SECONDS: f64 = 0.1;

/// What a settled voice diarization becomes: each segment's PCM written to
/// `<output>-<index>`, then [`DIARIZATION_OUTPUT_EVENT`] (at least one
/// segment, ascending by start), or [`super::FAILED_EVENT`] (a span outside
/// the utterance fails it; an end at most
/// [`DIARIZATION_END_SLACK_SECONDS`] past it is the utterance's end).
#[must_use]
pub fn diarization_outcome(
    direction: &'static str,
    answered: Result<Response, TransportFailure>,
    utterance: (&[u8], u32, u16),
    output: &Media,
    write: &mut dyn FnMut(&str, &[u8]) -> std::io::Result<()>,
) -> Returned {
    let (pcm, rate, channels) = utterance;
    let total = duration(pcm.len(), rate, channels);
    let spans = match answered_body(answered).and_then(|body| answered_spans(&body)) {
        Ok(spans) if !spans.is_empty() => spans,
        Ok(_) => return Returned::failed_named(direction, FailureCode::Malformed),
        Err(code) => return Returned::failed_named(direction, code),
    };
    let mut spans = spans;
    spans.sort_by(|left, right| left.0.total_cmp(&right.0));
    let mut segments = Vec::with_capacity(spans.len());
    for (index, (start, end, confidence)) in spans.into_iter().enumerate() {
        let end = if end > total && end <= total + DIARIZATION_END_SLACK_SECONDS {
            total
        } else {
            end
        };
        if start < 0.0 || end <= start || end > total + 1e-9 {
            return Returned::failed_named(direction, FailureCode::Malformed);
        }
        let Some(clip) = clip_pcm(pcm, rate, channels, start, end) else {
            return Returned::failed_named(direction, FailureCode::Malformed);
        };
        let file = indexed(output, index);
        if write(&file.path, clip).is_err() {
            return Returned::failed_named(direction, FailureCode::Unwritable);
        }
        segments.push(json!({
            "audio": file.reference,
            "media_type": "audio/pcm",
            "sample_rate_hz": rate,
            "channels": channels,
            "start_seconds": start,
            "end_seconds": end,
            "confidence": confidence,
        }));
    }
    Returned {
        name: DIARIZATION_OUTPUT_EVENT,
        data: canonical_json(&json!({ "segments": segments })),
    }
}

/// One answered embedding: the answer's
/// `embedding`, every value finite, at least one.
///
/// # Errors
///
/// The transport's, the status's, or [`FailureCode::Malformed`].
pub fn embedding_outcome(
    answered: Result<Response, TransportFailure>,
    model: &str,
) -> Result<Json, FailureCode> {
    let body = answered_body(answered)?;
    let answer: Json = serde_json::from_slice(&body).map_err(|_| FailureCode::Malformed)?;
    let Some(Json::Array(values)) = answer.get("embedding") else {
        return Err(FailureCode::Malformed);
    };
    if values.is_empty() {
        return Err(FailureCode::Malformed);
    }
    let values = values
        .iter()
        .map(|value| json_number(value).ok_or(FailureCode::Malformed))
        .collect::<Result<Vec<f64>, _>>()?;
    Ok(json!({"embedding": values, "model": model, "confidence": null}))
}

/// [`IDENTIFICATION_OUTPUT_EVENT`] with one embedding per segment, in order.
#[must_use]
pub fn identification_returned(embeddings: &[Json]) -> Returned {
    Returned {
        name: IDENTIFICATION_OUTPUT_EVENT,
        data: canonical_json(&json!({ "embeddings": embeddings })),
    }
}

/// A sound's done data: `{audio, media_type, sample_rate_hz, channels}`.
fn sound_data(audio: &str, media_type: Option<&str>, format: Format) -> String {
    canonical_json(&json!({
        "audio": audio,
        "media_type": media_type,
        "sample_rate_hz": format.sample_rate_hz,
        "channels": format.channels,
    }))
}

/// Normalization, on files: a WAV container (by media type) is
/// read and its signed 16-bit PCM written to `output`; raw PCM, `audio/l16`,
/// `audio/raw`, or an unnamed type with a rate and channels is `audio/pcm`;
/// anything else, or a container that cannot be read, is the sound
/// unchanged. The done data is the
/// sound it becomes.
///
/// # Errors
///
/// [`FailureCode::Unwritable`] when the PCM cannot be written.
pub fn normalized(
    normalization: &Normalization,
    read: impl FnOnce(&str) -> Result<Vec<u8>, FailureCode>,
    write: impl FnOnce(&str, &[u8]) -> std::io::Result<()>,
) -> Result<String, FailureCode> {
    let unchanged = || {
        sound_data(
            &normalization.audio.reference,
            normalization.media_type.as_deref(),
            normalization.format,
        )
    };
    let media_type = normalization
        .media_type
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if ["", "audio/pcm", "audio/l16", "audio/raw"].contains(&media_type.as_str()) {
        let format = normalization.format;
        if media_type != "audio/pcm" && format.sample_rate_hz.is_some() && format.channels.is_some()
        {
            return Ok(sound_data(
                &normalization.audio.reference,
                Some("audio/pcm"),
                format,
            ));
        }
        return Ok(unchanged());
    }
    if !is_wav_type(Some(&media_type)) {
        return Ok(unchanged());
    }
    let Ok(audio) = read(&normalization.audio.path) else {
        return Ok(unchanged());
    };
    let Ok((rate, channels, pcm)) = wav_16bit(&audio) else {
        return Ok(unchanged());
    };
    if pcm.is_empty() {
        return Ok(unchanged());
    }
    write(&normalization.output.path, &pcm).map_err(|_| FailureCode::Unwritable)?;
    Ok(sound_data(
        &normalization.output.reference,
        Some("audio/pcm"),
        Format {
            sample_rate_hz: Some(rate),
            channels: Some(channels),
        },
    ))
}

/// The open speech, on files: the frames' PCM appended in order to
/// `output` (a WAV container when `wav`), the rate the first frame naming
/// one, the channels the first frame's (1 when it names none). The done
/// data is the utterance `{audio, media_type, sample_rate_hz, channels}`.
///
/// # Errors
///
/// A frame's read failure, [`FailureCode::TooLarge`] past
/// [`MAX_AUDIO_BYTES`], [`FailureCode::Format`] for a WAV utterance with no
/// rate, or [`FailureCode::Unwritable`].
pub fn appended(
    appending: &Appending,
    mut read: impl FnMut(&str) -> Result<Vec<u8>, FailureCode>,
    write: impl FnOnce(&str, &[u8]) -> std::io::Result<()>,
) -> Result<String, FailureCode> {
    let mut pcm = Vec::new();
    for frame in &appending.frames {
        let audio = match read(&frame.audio.path) {
            Ok(audio) => audio,
            Err(FailureCode::Empty) => Vec::new(),
            Err(code) => return Err(code),
        };
        pcm.extend_from_slice(&audio);
        if exceeds_wasm_bound(pcm.len(), MAX_AUDIO_BYTES) {
            return Err(FailureCode::TooLarge);
        }
    }
    let rate = appending
        .frames
        .iter()
        .find_map(|frame| frame.format.sample_rate_hz);
    let channels = appending.frames[0].format.channels.unwrap_or(1);
    let (bytes, media_type) = if appending.wav {
        (
            wav_container(&pcm, rate.ok_or(FailureCode::Format)?, channels),
            "audio/wav",
        )
    } else {
        (pcm, "audio/pcm")
    };
    write(&appending.output.path, &bytes).map_err(|_| FailureCode::Unwritable)?;
    Ok(sound_data(
        &appending.output.reference,
        Some(media_type),
        Format {
            sample_rate_hz: rate,
            channels: Some(channels),
        },
    ))
}

/// A detected span's voice segment, on files: the span clamped to
/// the audio's duration and clipped by whole frames to `output`. The done
/// data is the voice segment.
///
/// # Errors
///
/// [`FailureCode::Format`] for incomplete frames, [`FailureCode::Span`]
/// when the span holds no audio, a read failure, or
/// [`FailureCode::Unwritable`].
pub fn clipped(
    clipping: &Clipping,
    read: impl FnOnce(&str) -> Result<Vec<u8>, FailureCode>,
    write: impl FnOnce(&str, &[u8]) -> std::io::Result<()>,
) -> Result<String, FailureCode> {
    let pcm = read(&clipping.audio.path)?;
    if !pcm.len().is_multiple_of(2 * usize::from(clipping.channels)) {
        return Err(FailureCode::Format);
    }
    let end = clipping.end_seconds.min(duration(
        pcm.len(),
        clipping.sample_rate_hz,
        clipping.channels,
    ));
    if end <= clipping.start_seconds {
        return Err(FailureCode::Span);
    }
    let clip = clip_pcm(
        &pcm,
        clipping.sample_rate_hz,
        clipping.channels,
        clipping.start_seconds,
        end,
    )
    .ok_or(FailureCode::Span)?;
    write(&clipping.output.path, clip).map_err(|_| FailureCode::Unwritable)?;
    Ok(canonical_json(&json!({
        "audio": clipping.output.reference,
        "media_type": "audio/pcm",
        "sample_rate_hz": clipping.sample_rate_hz,
        "channels": clipping.channels,
        "start_seconds": clipping.start_seconds,
        "end_seconds": end,
    })))
}

/// The detection's samples and the clip's duration: the WAV itself when
/// the sound is one (by media type or bytes), else raw PCM in the sound's
/// format, 16 kHz mono when it names none.
///
/// # Errors
///
/// [`FailureCode::Format`] when the audio is not readable 16-bit PCM.
pub fn detection_samples(
    detection: &Detection,
    audio: Vec<u8>,
) -> Result<(Vec<f32>, f64), FailureCode> {
    let (rate, channels, pcm) = pcm_of(
        audio,
        detection.format,
        Some((VAD_SAMPLE_RATE_HZ, DEFAULT_CHANNELS)),
    )?;
    Ok((
        vad_samples(&pcm, rate, channels),
        duration(pcm.len(), rate, channels),
    ))
}

/// The utterance a diarization reads: its 16-bit PCM and format (a WAV
/// container declares its own; raw PCM needs both).
///
/// # Errors
///
/// [`FailureCode::Format`].
pub fn utterance(
    diarization: &Diarization,
    audio: Vec<u8>,
) -> Result<(u32, u16, Vec<u8>), FailureCode> {
    pcm_of(audio, diarization.format, None)
}

/// The diarization's file part name.
#[must_use]
pub fn diarization_file_name(diarization: &Diarization) -> String {
    let name = file_name(&diarization.audio.path);
    if std::path::Path::new(name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"))
    {
        name.to_owned()
    } else {
        format!("{name}.wav")
    }
}

#[cfg(test)]
mod tests;
