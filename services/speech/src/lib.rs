//! The `speech` invoked service: one speech decoding or encoding per
//! `<invoke>`, as a `stateforward:scxml/invoked-service` Wasm component. It
//! is mosfet's `SpeechDecoder` (hearing's `SpeechDecoding`) and
//! `SpeechEncoder` (vocal's `SpeechEncoding`) providers in one service
//! (decision D50): both directions are one `OpenAI`-compatible audio API.
//!
//! A model call is slow I/O, so it is a service, never executable content:
//! the invoking session keeps processing events and timers while the call is
//! in flight, a `<send delay>` in the invoking state bounds it, and leaving
//! that state cancels it. The backend is chosen by the run through the
//! `baseurl` slot, never by the document or the component, and there is no
//! default (decision D31). Documents name [`SERVICE_URI`]:
//!
//! ```xml
//! <invoke id="decoding" type="https://github.com/stateforward/floppy/services/speech"
//!         env:apikey="FLOPPY_SPEECH_API_KEY" env:baseurl="FLOPPY_SPEECH_BASE_URL">
//!   <param name="operation" expr="'decode'"/>
//!   <param name="model" expr="'whisper-1'"/>
//!   <param name="audio" expr="_event.data.audio"/>
//! </invoke>
//! ```
//!
//! # Media
//!
//! Media moves by reference, never as bytes (port plan, "Phase 4 event
//! contracts"): `audio` and `output` are URI references to files in a
//! directory the run grants with
//! `--dir https://github.com/stateforward/floppy/services/speech=HOST::GUEST`.
//! A reference is resolved against `file:///`, the guest's root
//! ([`media_path`]): `media/audio-1`, `/media/audio-1`, and
//! `file:///media/audio-1` all name the guest file `/media/audio-1`. A
//! reference with another scheme, an authority, a query, a fragment, an
//! empty, `.`, or `..` segment, or an escape that is not UTF-8 or decodes
//! to `/` or NUL is malformed. A file outside the granted directory cannot
//! be opened, and the invocation answers [`FAILED_EVENT`].
//!
//! # Parameters (`<param>`)
//!
//! * `operation` (required): `decode` (speech to text, mosfet's
//!   `SpeechDecoder`) or `encode` (text to speech, mosfet's
//!   `SpeechEncoder`).
//! * `model` (required, a non-empty string): the backend's model name.
//! * decode: `audio` (required, a reference to the speech to decode) and
//!   `media_type` (optional, a `type/subtype` sent as the file part's
//!   `content-type`, `application/octet-stream` without it).
//! * encode: `text` (required, a non-empty string), `voice` (required,
//!   `[A-Za-z0-9_.-]{1,128}`), `output` (required, a reference to the file
//!   the audio is written to, created or replaced), `media_type` (optional,
//!   one of [`FORMATS`], `audio/mpeg` without it, the default of both
//!   backends), and `provider` (optional, exactly `elevenlabs`: mosfet's
//!   `elevenlabs` provider instead of the `OpenAI`-compatible API).
//!
//! Any other parameter, a parameter given twice, one the operation does
//! not take, or a malformed value rejects the start: the `<invoke>` raises
//! `error.execution` and nothing is read, sent, or written.
//!
//! The `env` slots ([`API_KEY_SLOT`], [`BASE_URL_SLOT`]) are both required
//! at run time. When either names no variable the run holds, the invocation
//! reads, sends, and writes nothing and answers [`FAILED_EVENT`]
//! ([`FailureCode::Unconfigured`]).
//!
//! # The requests
//!
//! * decode: `POST {base}/audio/transcriptions` with
//!   `authorization: Bearer <apikey>` and a `multipart/form-data` body of
//!   `model`, `response_format` (`json`), and `file` (the audio's bytes,
//!   named by the reference's last segment) ([`decoding_request`]).
//! * encode: `POST {base}/audio/speech` with `authorization: Bearer
//!   <apikey>` and the canonical JSON body `{"input", "model",
//!   "response_format", "voice"}` ([`encoding_request`]); with `provider`
//!   `elevenlabs`, `POST {base}/text-to-speech/{voice}?output_format=F` with
//!   `xi-api-key: <apikey>` and the body `{"model_id", "text"}` (its
//!   `text-to-speech` API; `F` from [`FORMATS`]).
//!
//! # Events
//!
//! `invoke` (the component's one export, an `async func`) writes exactly one
//! event to the stream it returns, then resolves its result future with no
//! data (the invoking session receives `done.invoke.<id>` after the event).
//! The names and data are mosfet's (`services/speech/schemas/`):
//!
//! * [`DECODING_OUTPUT_EVENT`] `{"text": T}`: the decoded speech (empty when
//!   the backend heard none).
//! * [`ENCODING_OUTPUT_EVENT`] `{"audio": R, "media_type": M,
//!   "sample_rate_hz": N, "channels": C}`: `R` is the `output` reference, the
//!   audio is in its file, and `N` and `C` are numbers only when known (a
//!   WAV file's header, or the `ElevenLabs` format that was asked for), else
//!   `null`.
//! * [`FAILED_EVENT`] `{"message": M}` (mosfet's `FailureData`), `M` one
//!   closed message per [`FailureCode`] and direction.
//!
//! The service reads no event: it drops the event stream `invoke` receives.
//!
//! # Bounds
//!
//! The audio a decoding reads ([`MAX_AUDIO_BYTES`], inside the host's
//! request-body bound), the text an encoding sends ([`MAX_TEXT_BYTES`]),
//! the text a decoding returns ([`MAX_DECODED_TEXT_BYTES`], under the
//! session's event bound), a reference ([`MAX_REFERENCE_BYTES`]), and the
//! model name ([`MAX_MODEL_BYTES`]) are bounded on wasm builds only
//! ([`scxml_types::wasm_bound`]); the encoded audio is bounded by the host's
//! response-body bound.
//!
//! # Telemetry
//!
//! `invoke` records one observation through scxml.rs's `telemetry` WIT
//! facade (operation `start`: its closed outcome, `ok`, `rejected`, or
//! `failed`, and one closed signal event), and one closed signal event
//! (operation `send`) when it returns its event. No attribute carries the
//! text, the audio, a reference, the model, the voice, or a URL, so the
//! cardinality is the product of the closed enums.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; no authored unsafe code"
    )
)]

use std::io::Read as _;

use scxml_event_schema::canonical_json;
use scxml_types::{DataValue as Data, exceeds_wasm_bound, wasm_bound};
use serde_json::{Map, Value as Json};

/// The service's name: the last path segment of its repository service URI
/// before the tag.
pub const SERVICE_NAME: &str = "speech";

/// The exact service type URI documents name: tagless, so it names the
/// repository's latest release (decision D19); the manifest names it too.
pub const SERVICE_URI: &str = "https://github.com/stateforward/floppy/services/speech";

/// The `env` slot of the API key (`env:apikey`).
pub const API_KEY_SLOT: &str = "apikey";
/// The `env` slot of the API base URL (`env:baseurl`).
pub const BASE_URL_SLOT: &str = "baseurl";
/// The decoding path appended to the base URL.
pub const TRANSCRIPTIONS_PATH: &str = "/audio/transcriptions";
/// The encoding path appended to the base URL.
pub const SPEECH_PATH: &str = "/audio/speech";
/// The `ElevenLabs` encoding path appended to the base URL, before the voice.
pub const TEXT_TO_SPEECH_PATH: &str = "/text-to-speech/";
/// The encoded media type when the invocation names none: `mp3`, the
/// default of both backends.
pub const DEFAULT_MEDIA_TYPE: &str = "audio/mpeg";
/// The `content-type` of the decoded audio's file part when the invocation
/// names none.
pub const DEFAULT_AUDIO_MEDIA_TYPE: &str = "application/octet-stream";

/// The decoded speech (mosfet's `SpeechDecoding.output_event`).
pub const DECODING_OUTPUT_EVENT: &str = "bot.ability.hearing.speech.decoding.output";
/// The encoded speech (mosfet's `SpeechEncoding.output_event`).
pub const ENCODING_OUTPUT_EVENT: &str = "bot.ability.vocal.speech.encoding.output";
/// The failure (mosfet's `Ability.failed_event`, `FailureData`).
pub const FAILED_EVENT: &str = "bot.ability.failed";

/// Upper bound on the audio a decoding reads (wasm builds only): the host's
/// 4 MiB request-body bound less room for the form's other parts.
pub const MAX_AUDIO_BYTES: usize = wasm_bound(4 * 1024 * 1024 - 16 * 1024);
/// Upper bound on the text an encoding sends (wasm builds only).
pub const MAX_TEXT_BYTES: usize = wasm_bound(16 * 1024);
/// Upper bound on the text a decoding returns (wasm builds only): under
/// the session's 96 KiB event bound, leaving room for the envelope.
pub const MAX_DECODED_TEXT_BYTES: usize = wasm_bound(64 * 1024);
/// Upper bound on a media reference's bytes (wasm builds only).
pub const MAX_REFERENCE_BYTES: usize = wasm_bound(1024);
/// Upper bound on the model name's bytes (wasm builds only).
pub const MAX_MODEL_BYTES: usize = wasm_bound(256);
/// Upper bound on a voice's bytes (it is a URL path segment for
/// `ElevenLabs`).
pub const MAX_VOICE_BYTES: usize = 128;
/// Upper bound on a decoding's `media_type` (RFC 6838's 127 characters).
pub const MAX_MEDIA_TYPE_BYTES: usize = 127;

/// One encoded format: its media type and how each backend asks for it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Format {
    /// The media type documents name.
    pub media_type: &'static str,
    /// The `OpenAI` `response_format`.
    pub response_format: &'static str,
    /// The `ElevenLabs` `output_format` and the sample rate it names, when
    /// `ElevenLabs` encodes it.
    pub elevenlabs: Option<(&'static str, u32)>,
}

/// The formats an encoding may ask for: the `OpenAI` speech API's
/// `response_format`s; `ElevenLabs` encodes `mp3` as `mp3_44100_128`
/// (mosfet's `ElevenLabs` default) and `pcm` as `pcm_24000`.
pub const FORMATS: [Format; 6] = [
    Format {
        media_type: "audio/mpeg",
        response_format: "mp3",
        elevenlabs: Some(("mp3_44100_128", 44_100)),
    },
    Format {
        media_type: "audio/opus",
        response_format: "opus",
        elevenlabs: None,
    },
    Format {
        media_type: "audio/aac",
        response_format: "aac",
        elevenlabs: None,
    },
    Format {
        media_type: "audio/flac",
        response_format: "flac",
        elevenlabs: None,
    },
    Format {
        media_type: "audio/wav",
        response_format: "wav",
        elevenlabs: None,
    },
    Format {
        media_type: "audio/pcm",
        response_format: "pcm",
        elevenlabs: Some(("pcm_24000", 24_000)),
    },
];

/// One part of a URL or header value: literal text, or an opaque reference
/// the host substitutes with the value it holds for one slot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Part {
    /// Literal text.
    Literal(String),
    /// A slot's reference.
    Reference(u32),
}

/// One `POST` request to the backend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    /// The absolute target URL, in parts.
    pub url: Vec<Part>,
    /// Header fields, in order, their values in parts.
    pub headers: Vec<(String, Vec<Part>)>,
    /// The body.
    pub body: Vec<u8>,
}

/// The backend's answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Response {
    /// HTTP status.
    pub status: u16,
    /// Body bytes.
    pub body: Vec<u8>,
}

/// Why the transport answered no response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportFailure {
    /// No response: refused by the host before sending, unreachable, timed
    /// out, or failed.
    Unavailable,
    /// The response body exceeds the host's bound.
    TooLarge,
}

/// One media file: the reference the document gave and the guest path it
/// names.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Media {
    /// The reference, exactly as given.
    pub reference: String,
    /// The absolute guest path it resolves to.
    pub path: String,
}

/// The encoding backend (mosfet's provider packages).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Provider {
    /// An `OpenAI`-compatible audio API (mosfet's `openai_compat`).
    OpenaiCompat,
    /// `ElevenLabs`' `text-to-speech` API (mosfet's `elevenlabs`).
    Elevenlabs,
}

/// A validated decoding: speech to text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Decoding {
    /// The model name.
    pub model: String,
    /// The speech to decode.
    pub audio: Media,
    /// The file part's `content-type`.
    pub media_type: String,
}

/// A validated encoding: text to speech.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Encoding {
    /// The model name.
    pub model: String,
    /// The text to speak.
    pub text: String,
    /// The voice.
    pub voice: String,
    /// Where the audio is written.
    pub output: Media,
    /// The encoded format.
    pub format: Format,
    /// The backend.
    pub provider: Provider,
}

/// One validated invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Operation {
    /// `decode`.
    Decode(Decoding),
    /// `encode`.
    Encode(Encoding),
}

/// The closed failures of [`FAILED_EVENT`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureCode {
    /// The `apikey` or the `baseurl` slot names no variable the run holds.
    Unconfigured,
    /// The audio's file cannot be read (it does not exist, or is outside
    /// the granted directory).
    Unreadable,
    /// The audio's file is empty.
    Empty,
    /// The audio exceeds [`MAX_AUDIO_BYTES`].
    TooLarge,
    /// The output's file cannot be written.
    Unwritable,
    /// The transport failed: no response.
    Transport,
    /// The backend answered with a non-2xx status.
    Status,
    /// The answer is not a transcription or audio, or exceeds its bound.
    Malformed,
}

impl FailureCode {
    /// What failed, as the end of the closed message.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Unconfigured => "the backend is not configured",
            Self::Unreadable => "the audio cannot be read",
            Self::Empty => "the audio is empty",
            Self::TooLarge => "the audio exceeds its bound",
            Self::Unwritable => "the output cannot be written",
            Self::Transport => "the backend is unreachable",
            Self::Status => "the backend answered an error status",
            Self::Malformed => "the backend's answer is malformed",
        }
    }
}

/// The one event an invocation returns before it completes: its name and its
/// data as a canonical JSON object.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Returned {
    /// [`DECODING_OUTPUT_EVENT`], [`ENCODING_OUTPUT_EVENT`], or
    /// [`FAILED_EVENT`].
    pub name: &'static str,
    /// Canonical JSON object text.
    pub data: String,
}

impl Returned {
    /// [`FAILED_EVENT`] with the closed message of `code` for `operation`.
    #[must_use]
    pub fn failed(operation: &Operation, code: FailureCode) -> Self {
        let direction = match operation {
            Operation::Decode(_) => "Speech decoding",
            Operation::Encode(_) => "Speech encoding",
        };
        Self {
            name: FAILED_EVENT,
            data: canonical_json(&serde_json::json!({
                "message": format!("{direction} failed: {}.", code.reason()),
            })),
        }
    }
}

fn text_parameter(value: &Data, invalid: &'static str) -> Result<String, &'static str> {
    match value {
        Data::String(text) if !text.trim().is_empty() => Ok(text.clone()),
        _ => Err(invalid),
    }
}

/// The absolute guest path a media reference names: the reference resolved
/// against `file:///` (see the crate documentation, "Media").
///
/// # Errors
///
/// A static detail for a reference that is empty, over its bound, or not a
/// plain path in the guest's file system.
pub fn media_path(reference: &str) -> Result<String, &'static str> {
    const INVALID: &str = "speech media must be a file reference in the granted directory";
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

fn media_parameter(value: &Data, invalid: &'static str) -> Result<Media, &'static str> {
    let Data::String(reference) = value else {
        return Err(invalid);
    };
    Ok(Media {
        reference: reference.clone(),
        path: media_path(reference)?,
    })
}

/// Whether `text` is a `type/subtype` of RFC 6838 restricted-name
/// characters.
fn is_media_type(text: &str) -> bool {
    let name = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$&-^_.+".contains(&byte))
    };
    text.len() <= MAX_MEDIA_TYPE_BYTES
        && text
            .split_once('/')
            .is_some_and(|(kind, subtype)| name(kind) && name(subtype))
}

/// The validated operation of an invocation's parameters (see the crate
/// documentation, "Parameters").
///
/// # Errors
///
/// A static detail for an unknown or repeated parameter, one the operation
/// does not take, a missing or malformed one, or one outside its bound.
pub fn parse_parameters<'a>(
    parameters: impl IntoIterator<Item = (&'a str, &'a Data)>,
) -> Result<Operation, &'static str> {
    let mut operation = None;
    let mut model = None;
    let mut audio = None;
    let mut media_type = None;
    let mut text = None;
    let mut voice = None;
    let mut output = None;
    let mut provider = None;
    for (name, value) in parameters {
        let slot = match name {
            "operation" => &mut operation,
            "model" => &mut model,
            "audio" => &mut audio,
            "media_type" => &mut media_type,
            "text" => &mut text,
            "voice" => &mut voice,
            "output" => &mut output,
            "provider" => &mut provider,
            _ => return Err("speech takes no parameter of that name"),
        };
        if slot.replace(value).is_some() {
            return Err("speech takes each parameter once");
        }
    }
    let model = text_parameter(
        model.ok_or("speech requires a model parameter")?,
        "speech model must be a non-empty string",
    )?
    .trim()
    .to_owned();
    if exceeds_wasm_bound(model.len(), MAX_MODEL_BYTES) {
        return Err("speech model exceeds its bound");
    }
    match operation.ok_or("speech requires an operation parameter")? {
        Data::String(name) if name == "decode" => {
            if text.is_some() || voice.is_some() || output.is_some() || provider.is_some() {
                return Err("speech decode takes model, audio, and media_type only");
            }
            decoding(model, audio, media_type).map(Operation::Decode)
        }
        Data::String(name) if name == "encode" => {
            if audio.is_some() {
                return Err("speech encode takes no audio parameter");
            }
            encoding(model, text, voice, output, media_type, provider).map(Operation::Encode)
        }
        _ => Err("speech operation must be decode or encode"),
    }
}

fn decoding(
    model: String,
    audio: Option<&Data>,
    media_type: Option<&Data>,
) -> Result<Decoding, &'static str> {
    let audio = media_parameter(
        audio.ok_or("speech decode requires an audio parameter")?,
        "speech audio must be a file reference",
    )?;
    let media_type = match media_type {
        None => DEFAULT_AUDIO_MEDIA_TYPE.to_owned(),
        Some(Data::String(media_type)) if is_media_type(media_type) => media_type.clone(),
        Some(_) => return Err("speech media_type must be a type/subtype"),
    };
    Ok(Decoding {
        model,
        audio,
        media_type,
    })
}

fn encoding(
    model: String,
    text: Option<&Data>,
    voice: Option<&Data>,
    output: Option<&Data>,
    media_type: Option<&Data>,
    provider: Option<&Data>,
) -> Result<Encoding, &'static str> {
    let text = text_parameter(
        text.ok_or("speech encode requires a text parameter")?,
        "speech text must be a non-empty string",
    )?;
    if exceeds_wasm_bound(text.len(), MAX_TEXT_BYTES) {
        return Err("speech text exceeds its bound");
    }
    let voice = match voice.ok_or("speech encode requires a voice parameter")? {
        Data::String(voice)
            if !voice.is_empty()
                && voice.len() <= MAX_VOICE_BYTES
                && voice
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte)) =>
        {
            voice.clone()
        }
        _ => return Err("speech voice must match [A-Za-z0-9_.-]{1,128}"),
    };
    let output = media_parameter(
        output.ok_or("speech encode requires an output parameter")?,
        "speech output must be a file reference",
    )?;
    let provider = match provider {
        None => Provider::OpenaiCompat,
        Some(Data::String(name)) if name == "elevenlabs" => Provider::Elevenlabs,
        Some(_) => return Err("speech provider must be elevenlabs"),
    };
    let format = match media_type {
        None => DEFAULT_MEDIA_TYPE,
        Some(Data::String(media_type)) => media_type.as_str(),
        Some(_) => return Err("speech media_type must be a string"),
    };
    let format = FORMATS
        .into_iter()
        .find(|known| known.media_type == format)
        .filter(|known| provider == Provider::OpenaiCompat || known.elevenlabs.is_some())
        .ok_or("speech media_type is not one the backend encodes")?;
    Ok(Encoding {
        model,
        text,
        voice,
        output,
        format,
        provider,
    })
}

/// The invocation's endpoint, `(baseurl, apikey)`: the references the host
/// handed for the two slots. There is no default backend, so both are
/// required.
///
/// # Errors
///
/// [`FailureCode::Unconfigured`] when either slot names no variable the run
/// holds (the host hands no reference for it): nothing is read or sent.
pub const fn endpoint(
    base_url: Option<u32>,
    api_key: Option<u32>,
) -> Result<(u32, u32), FailureCode> {
    match (base_url, api_key) {
        (Some(base_url), Some(api_key)) => Ok((base_url, api_key)),
        _ => Err(FailureCode::Unconfigured),
    }
}

/// Reads the audio a decoding names from the granted directory.
///
/// # Errors
///
/// [`FailureCode::Unreadable`] when the file cannot be opened or read,
/// [`FailureCode::Empty`] when it is empty, and [`FailureCode::TooLarge`]
/// when it exceeds [`MAX_AUDIO_BYTES`].
pub fn read_audio(path: &str) -> Result<Vec<u8>, FailureCode> {
    let file = std::fs::File::open(path).map_err(|_| FailureCode::Unreadable)?;
    let mut audio = Vec::new();
    file.take(
        u64::try_from(MAX_AUDIO_BYTES)
            .unwrap_or(u64::MAX)
            .saturating_add(1),
    )
    .read_to_end(&mut audio)
    .map_err(|_| FailureCode::Unreadable)?;
    if audio.is_empty() {
        return Err(FailureCode::Empty);
    }
    if exceeds_wasm_bound(audio.len(), MAX_AUDIO_BYTES) {
        return Err(FailureCode::TooLarge);
    }
    Ok(audio)
}

/// The multipart boundary: a fixed prefix and the first counter that does
/// not occur in the audio.
fn boundary(audio: &[u8]) -> String {
    let mut counter = 0_u32;
    loop {
        let candidate = format!("floppy-speech-{counter:08x}");
        if !audio
            .windows(candidate.len())
            .any(|window| window == candidate.as_bytes())
        {
            return candidate;
        }
        counter = counter.wrapping_add(1);
    }
}

/// The file part's name: the path's last segment when it needs no quoting,
/// else `audio`.
fn file_name(path: &str) -> &str {
    let last = path.rsplit('/').next().unwrap_or_default();
    if !last.is_empty()
        && last
            .chars()
            .all(|character| !character.is_control() && character != '"' && character != '\\')
    {
        last
    } else {
        "audio"
    }
}

fn bearer(api_key: u32) -> (String, Vec<Part>) {
    (
        "authorization".into(),
        vec![Part::Literal("Bearer ".into()), Part::Reference(api_key)],
    )
}

/// The transcription request: `POST {base}/audio/transcriptions` with the
/// `apikey` slot's reference as a bearer key and a `multipart/form-data`
/// body of `model`, `response_format` (`json`), and `file` (`audio`).
#[must_use]
pub fn decoding_request(base_url: u32, api_key: u32, decoding: &Decoding, audio: &[u8]) -> Request {
    let boundary = boundary(audio);
    let mut body = Vec::with_capacity(audio.len() + 512);
    for (name, value) in [
        ("model", decoding.model.as_str()),
        ("response_format", "json"),
    ] {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\ncontent-disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\ncontent-disposition: form-data; name=\"file\"; filename=\"{}\"\r\ncontent-type: {}\r\n\r\n",
            file_name(&decoding.audio.path),
            decoding.media_type
        )
        .as_bytes(),
    );
    body.extend_from_slice(audio);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Request {
        url: vec![
            Part::Reference(base_url),
            Part::Literal(TRANSCRIPTIONS_PATH.into()),
        ],
        headers: vec![
            bearer(api_key),
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
        ],
        body,
    }
}

/// The speech request: `POST {base}/audio/speech` with a bearer key and the
/// canonical JSON body `{"input", "model", "response_format", "voice"}`,
/// or, for `ElevenLabs`, `POST {base}/text-to-speech/{voice}?output_format=F`
/// with `xi-api-key` and the body `{"model_id", "text"}`.
#[must_use]
pub fn encoding_request(base_url: u32, api_key: u32, encoding: &Encoding) -> Request {
    let content_type = (
        "content-type".to_owned(),
        vec![Part::Literal("application/json".into())],
    );
    let mut body = Map::new();
    if let (Provider::Elevenlabs, Some((output_format, _))) =
        (encoding.provider, encoding.format.elevenlabs)
    {
        body.insert("model_id".into(), encoding.model.clone().into());
        body.insert("text".into(), encoding.text.clone().into());
        return Request {
            url: vec![
                Part::Reference(base_url),
                Part::Literal(format!(
                    "{TEXT_TO_SPEECH_PATH}{}?output_format={output_format}",
                    encoding.voice
                )),
            ],
            headers: vec![
                ("xi-api-key".into(), vec![Part::Reference(api_key)]),
                content_type,
            ],
            body: canonical_json(&Json::Object(body)).into_bytes(),
        };
    }
    body.insert("input".into(), encoding.text.clone().into());
    body.insert("model".into(), encoding.model.clone().into());
    body.insert(
        "response_format".into(),
        encoding.format.response_format.into(),
    );
    body.insert("voice".into(), encoding.voice.clone().into());
    Request {
        url: vec![Part::Reference(base_url), Part::Literal(SPEECH_PATH.into())],
        headers: vec![bearer(api_key), content_type],
        body: canonical_json(&Json::Object(body)).into_bytes(),
    }
}

/// The response, or the failure of its transport or its status.
fn answered_body(answered: Result<Response, TransportFailure>) -> Result<Vec<u8>, FailureCode> {
    let response = match answered {
        Ok(response) => response,
        Err(TransportFailure::Unavailable) => return Err(FailureCode::Transport),
        Err(TransportFailure::TooLarge) => return Err(FailureCode::Malformed),
    };
    if !(200..300).contains(&response.status) {
        return Err(FailureCode::Status);
    }
    Ok(response.body)
}

/// What a settled transcription becomes: [`DECODING_OUTPUT_EVENT`] with the
/// answer's `text`, or [`FAILED_EVENT`].
#[must_use]
pub fn decoding_outcome(
    operation: &Operation,
    answered: Result<Response, TransportFailure>,
) -> Returned {
    let text = answered_body(answered).and_then(|body| {
        let answer: Json = serde_json::from_slice(&body).map_err(|_| FailureCode::Malformed)?;
        match answer.get("text") {
            Some(Json::String(text)) if !exceeds_wasm_bound(text.len(), MAX_DECODED_TEXT_BYTES) => {
                Ok(text.trim().to_owned())
            }
            _ => Err(FailureCode::Malformed),
        }
    });
    match text {
        Ok(text) => Returned {
            name: DECODING_OUTPUT_EVENT,
            data: canonical_json(&serde_json::json!({ "text": text })),
        },
        Err(code) => Returned::failed(operation, code),
    }
}

/// The sample rate and channels a WAV file's `fmt ` chunk declares.
#[must_use]
pub fn wav_header(audio: &[u8]) -> Option<(u32, u16)> {
    if audio.get(..4)? != b"RIFF" || audio.get(8..12)? != b"WAVE" {
        return None;
    }
    let mut offset = 12_usize;
    loop {
        let id = audio.get(offset..offset + 4)?;
        let size = u32::from_le_bytes(audio.get(offset + 4..offset + 8)?.try_into().ok()?);
        let body = offset + 8;
        if id == b"fmt " {
            let channels = u16::from_le_bytes(audio.get(body + 2..body + 4)?.try_into().ok()?);
            let rate = u32::from_le_bytes(audio.get(body + 4..body + 8)?.try_into().ok()?);
            return (channels > 0 && rate > 0).then_some((rate, channels));
        }
        // Chunks are padded to an even size.
        let size = usize::try_from(size).ok()?;
        offset = body.checked_add(size)?.checked_add(size % 2)?;
    }
}

/// What a settled speech request becomes: the audio written to the
/// output's file by `write`, then [`ENCODING_OUTPUT_EVENT`], or
/// [`FAILED_EVENT`] (nothing is written for a failed request).
#[must_use]
pub fn encoding_outcome(
    operation: &Operation,
    answered: Result<Response, TransportFailure>,
    write: impl FnOnce(&str, &[u8]) -> std::io::Result<()>,
) -> Returned {
    let Operation::Encode(encoding) = operation else {
        return Returned::failed(operation, FailureCode::Malformed);
    };
    let audio = match answered_body(answered) {
        Ok(audio) if !audio.is_empty() => audio,
        Ok(_) => return Returned::failed(operation, FailureCode::Malformed),
        Err(code) => return Returned::failed(operation, code),
    };
    if write(&encoding.output.path, &audio).is_err() {
        return Returned::failed(operation, FailureCode::Unwritable);
    }
    let (sample_rate_hz, channels) = match (encoding.provider, encoding.format) {
        (
            _,
            Format {
                response_format: "wav",
                ..
            },
        ) => wav_header(&audio).map_or((Json::Null, Json::Null), |(rate, channels)| {
            (rate.into(), channels.into())
        }),
        (
            Provider::Elevenlabs,
            Format {
                elevenlabs: Some((_, rate)),
                ..
            },
        ) => (rate.into(), Json::Null),
        _ => (Json::Null, Json::Null),
    };
    Returned {
        name: ENCODING_OUTPUT_EVENT,
        data: canonical_json(&serde_json::json!({
            "audio": encoding.output.reference,
            "media_type": encoding.format.media_type,
            "sample_rate_hz": sample_rate_hz,
            "channels": channels,
        })),
    }
}

/// Writes encoded audio to its granted file, creating or replacing it.
///
/// # Errors
///
/// The file system's error.
pub fn write_audio(path: &str, audio: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, audio)
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

    use exports::stateforward::scxml::invoked_service_api::Guest;
    use stateforward::scxml::outbound_http::{self, BodyPart, HttpHeader, HttpRequest, TextPart};
    use stateforward::scxml::telemetry::{self, Operation as Op, Outcome, SignalEvent};
    use stateforward::scxml::types::{
        ComponentError, DataValue, ErrorCode, ErrorVersion, Event, EventType, InvokeRequest,
        SlotReference,
    };

    use super::{Operation, Part, Request, Response, Returned, TransportFailure};

    /// The invocation's result: `ok` (no done data), or its error.
    type Done = Result<Option<DataValue>, ComponentError>;

    struct Component;

    fn error(code: ErrorCode, detail: &'static str) -> ComponentError {
        ComponentError {
            version: ErrorVersion::V1,
            code,
            detail: (detail.len() <= 160).then(|| detail.into()),
        }
    }

    fn text_parts(parts: &[Part]) -> Vec<TextPart> {
        parts
            .iter()
            .map(|part| match part {
                Part::Literal(text) => TextPart::Literal(text.clone()),
                Part::Reference(reference) => TextPart::Reference(*reference),
            })
            .collect()
    }

    fn native_value(value: DataValue) -> Result<scxml_types::DataValue, ComponentError> {
        let invalid = || error(ErrorCode::InvalidInput, "a parameter value is invalid");
        Ok(match value {
            DataValue::Absent => scxml_types::DataValue::Absent,
            DataValue::Undefined => scxml_types::DataValue::Undefined,
            DataValue::Null => scxml_types::DataValue::Null,
            DataValue::Boolean(value) => scxml_types::DataValue::Bool(value),
            DataValue::Number(value) => scxml_types::DataValue::Number(
                scxml_types::FiniteNumber::new(value).map_err(|_| invalid())?,
            ),
            DataValue::Text(value) => scxml_types::DataValue::String(value),
            DataValue::JsonArray(value) => {
                scxml_types::DataValue::canonical_json_array(&value).map_err(|_| invalid())?
            }
            DataValue::JsonObject(value) => {
                scxml_types::DataValue::canonical_json_object(&value).map_err(|_| invalid())?
            }
            DataValue::Xml(value) => scxml_types::DataValue::Xml(value),
        })
    }

    fn event(reply: &Returned) -> Event {
        Event {
            name: reply.name.to_owned(),
            event_type: EventType::External,
            send_id: None,
            origin: None,
            origin_type: None,
            invoke_id: None,
            data: DataValue::JsonObject(reply.data.clone()),
            raw: None,
            trace_context: None,
        }
    }

    fn slot(references: &[SlotReference], name: &str) -> Option<u32> {
        references
            .iter()
            .find(|bound| bound.slot == name)
            .map(|bound| bound.reference)
    }

    fn http(built: Request) -> HttpRequest {
        HttpRequest {
            method: "POST".into(),
            url: text_parts(&built.url),
            headers: built
                .headers
                .iter()
                .map(|(name, value)| HttpHeader {
                    name: name.clone(),
                    value: text_parts(value),
                })
                .collect(),
            body: vec![BodyPart::Bytes(built.body)],
        }
    }

    /// What `invoke` accepted: the operation and its endpoint, when the run
    /// configured one.
    type Plan = (Operation, Option<(u32, u32)>);

    fn plan(request: InvokeRequest) -> Result<Plan, ComponentError> {
        if request.type_uri != super::SERVICE_URI {
            return Err(error(ErrorCode::InvalidInput, "service type URI differs"));
        }
        let parameters = request
            .parameters
            .into_iter()
            .map(|entry| Ok((entry.name, native_value(entry.value)?)))
            .collect::<Result<Vec<_>, ComponentError>>()?;
        let operation = super::parse_parameters(
            parameters
                .iter()
                .map(|(name, value)| (name.as_str(), value)),
        )
        .map_err(|detail| error(ErrorCode::InvalidInput, detail))?;
        let endpoint = super::endpoint(
            slot(&request.references, super::BASE_URL_SLOT),
            slot(&request.references, super::API_KEY_SLOT),
        )
        .ok();
        Ok((operation, endpoint))
    }

    /// One request through `outbound-http`; awaiting it suspends only this
    /// task, never the invoking session.
    async fn fetch(built: Request) -> Result<Response, TransportFailure> {
        outbound_http::fetch(http(built))
            .await
            .map(|response| Response {
                status: response.status,
                body: response.body,
            })
            .map_err(|failure| match failure.code {
                ErrorCode::PayloadTooLarge => TransportFailure::TooLarge,
                _ => TransportFailure::Unavailable,
            })
    }

    /// Runs the accepted operation to its one event.
    async fn run(operation: Operation, endpoint: Option<(u32, u32)>) -> Returned {
        let Some((base_url, api_key)) = endpoint else {
            return Returned::failed(&operation, super::FailureCode::Unconfigured);
        };
        match &operation {
            Operation::Decode(decoding) => {
                let audio = match super::read_audio(&decoding.audio.path) {
                    Ok(audio) => audio,
                    Err(code) => return Returned::failed(&operation, code),
                };
                let built = super::decoding_request(base_url, api_key, decoding, &audio);
                drop(audio);
                let answered = fetch(built).await;
                super::decoding_outcome(&operation, answered)
            }
            Operation::Encode(encoding) => {
                let built = super::encoding_request(base_url, api_key, encoding);
                let answered = fetch(built).await;
                super::encoding_outcome(&operation, answered, super::write_audio)
            }
        }
    }

    #[allow(
        clippy::unused_async_trait_impl,
        reason = "the WIT `invoke` is an `async func`; it returns once its task is spawned"
    )]
    impl Guest for Component {
        async fn invoke(
            request: InvokeRequest,
            _events: wit_bindgen::StreamReader<Event>,
        ) -> Result<
            (
                wit_bindgen::StreamReader<Event>,
                wit_bindgen::FutureReader<Done>,
            ),
            ComponentError,
        > {
            // The service accepts no event: its input stream is dropped.
            let (operation, endpoint) = observed(Op::Start, || plan(request))?;
            let (mut returned, returned_reader) = wit_stream::new::<Event>();
            let (done, done_reader) = wit_future::new::<Done>(|| {
                Err(error(
                    ErrorCode::Platform,
                    "the service ended without a result",
                ))
            });
            wit_bindgen::spawn(async move {
                let reply = run(operation, endpoint).await;
                telemetry::event(
                    Op::Send,
                    if reply.name == super::FAILED_EVENT {
                        SignalEvent::InputRejected
                    } else {
                        SignalEvent::TransformationCompleted
                    },
                    None,
                );
                // The one event, then the result: `done.invoke.<id>` follows
                // it.
                if returned.write_one(event(&reply)).await.is_some() {
                    return;
                }
                drop(returned);
                let _ = done.write(Ok(None)).await;
            });
            Ok((returned_reader, done_reader))
        }
    }

    /// One call's closed telemetry: its outcome and one event (no attribute
    /// carries input).
    fn observed<T>(
        operation: Op,
        body: impl FnOnce() -> Result<T, ComponentError>,
    ) -> Result<T, ComponentError> {
        let result = body();
        let outcome = match &result {
            Ok(_) => Outcome::Ok,
            Err(failure) if failure.code == ErrorCode::InvalidInput => Outcome::Rejected,
            Err(_) => Outcome::Failed,
        };
        telemetry::observe(operation, outcome, None, None);
        telemetry::event(
            operation,
            if result.is_ok() {
                SignalEvent::TransformationCompleted
            } else {
                SignalEvent::InputRejected
            },
            None,
        );
        result
    }

    export!(Component);
}

#[cfg(test)]
mod tests;
