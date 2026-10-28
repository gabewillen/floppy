//! The `speech` invoked service: one speech or voice operation per
//! `<invoke>`, as a `stateforward:scxml/invoked-service` Wasm component. It
//! decodes speech for hearing's `SpeechDecoding` and encodes it for vocal's
//! `SpeechEncoding` in one service (decision D57): both directions are one
//! `OpenAI`-compatible audio API, or, with `provider` `gemini`, the Gemini
//! API ([`gemini`], decision D94). It also runs the voice models a document
//! cannot: voice activity detection (`VoiceDetection`), diarization
//! (`VoiceDiarization`) and voice embedding (`VoiceIdentification`),
//! through `LocalAI`'s voice APIs ([`voice`]), and the interpretation's
//! media steps (normalizing a sound to PCM, joining the open speech,
//! clipping a voice span), on files.
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
//! * `operation` (required): `decode` (speech to text), `encode`
//!   (text to speech), `stream` (speech to words while the person talks,
//!   [`stream`]), or one of the voice operations `detect`, `diarize`,
//!   `identify`, `classify`, `normalize`, `append`, and `clip` ([`voice`] documents
//!   their parameters, requests, and results).
//! * `model` (required for decode and encode, optional for the backend
//!   voice operations but `classify`, refused by `classify` and the media
//!   steps, a non-empty string): the backend's model name.
//! * decode: `audio` (required, a reference to the speech to decode),
//!   `media_type` (optional, a `type/subtype` sent as the file part's
//!   `content-type`, `application/octet-stream` without it, or as Gemini's
//!   `mime_type`, `audio/wav` without it), and `provider` (optional,
//!   exactly `gemini`: the Gemini API).
//! * encode: `parts` (the reply to say, a JSON array of 1 to
//!   [`MAX_PARTS`] parts; see "Parts"; without it the reply is drafted:
//!   it arrives as events, see "Drafts"), `voice` (required,
//!   `[A-Za-z0-9_.-]{1,128}`), `output` (required, a reference to the file
//!   the audio is written to, created or replaced), `media_type` (optional,
//!   one of [`FORMATS`], `audio/mpeg` without it, the default of both
//!   backends; a reply with a pause, and a drafted one, needs `audio/pcm`
//!   or `audio/wav`), and
//!   `provider` (optional, `elevenlabs` or `gemini`: the backend of that
//!   name instead of the `OpenAI`-compatible API; Gemini encodes
//!   only `audio/pcm` and `audio/wav`).
//! * stream: `language` (optional, a non-empty string, `en` without it)
//!   and `chunk_seconds` (optional, one of the server's decoding chunks
//!   [`stream::CHUNKS`], `0.56` without it), and nothing else: its audio
//!   arrives as events ([`stream`]).
//!
//! A parameter whose value is `null` is not given. Any other parameter, a
//! parameter given twice, one the operation does
//! not take, or a malformed value rejects the start: the `<invoke>` raises
//! `error.execution` and nothing is read, sent, or written.
//!
//! The `env` slots ([`API_KEY_SLOT`], [`BASE_URL_SLOT`]) serve the
//! operations that call a backend (all but `normalize`, `append`, and
//! `clip`). The `baseurl` slot is required at run time: when it names no
//! variable the run holds, the invocation reads, sends, and writes nothing
//! and answers [`FAILED_EVENT`] ([`FailureCode::Unconfigured`]). When only
//! the `apikey` slot names none, the backend is keyless (a local speech
//! server that takes no key):
//! every request is sent with no key header at all, `authorization`,
//! `xi-api-key`, or `x-goog-api-key` (decision D127).
//!
//! # Parts
//!
//! An encoding's `parts` are a structured reply (decision D178): each part is an object with only the optional
//! members `expression` and `words` (`null` is absent). `words` is a
//! string of 1 to [`MAX_WORDS_CHARS`] characters, not blank, holding none
//! of `* < > [ ] { } ( )`; `expression` is an object whose members are
//! among the expression fields ([`EXPRESSION_FIELDS`]), of
//! which the service reads only `pause` (`short` or `long`) and `sound` (a
//! string). Each part has words, a pause, or a sound, and at least one part
//! has words.
//!
//! The service applies no expression cue but a pause (decision D178): a
//! sound and every other cue are ignored. Walking the parts in order, a part with a pause opens a new
//! [`Stretch`], the pause before its words; the words within a stretch are
//! joined verbatim with nothing added.
//!
//! # Drafts
//!
//! An `encode` without `parts` is drafted (decision D262, [`drafting`]):
//! the reply is said while it is still being written. The service first
//! returns [`drafting::READY_EVENT`] `{}`, then reads the events the
//! invoking session sends it: [`drafting::DRAFT_EVENT`] `{part, words,
//! offset, expression?}`, the words newly written into a part, and then
//! [`drafting::FINAL_EVENT`] `{parts}`, the whole reply. Each request says
//! what is ready as soon as it is: the words through the last sentence
//! written, a part that pauses opening a request of its own, and the rest
//! once the whole reply is in. The whole reply is authoritative: when it
//! diverges from what was said, nothing more is said. The answers are
//! joined into the output as a reply with a pause is, and the chunks are
//! returned as they are cut, numbered across every request. When the
//! invoking session stops sending before the whole reply, the invocation
//! ends with no result.
//!
//! # The requests
//!
//! * decode: `POST {base}/audio/transcriptions` with
//!   `authorization: Bearer <apikey>` (none for a keyless backend, as for
//!   every key header below) and a `multipart/form-data` body of
//!   `model`, `response_format` (`json`), and `file` (the audio's bytes,
//!   named by the reference's last segment) ([`decoding_request`]).
//! * encode: one request per stretch whose words are not blank, one at a
//!   time in order, saying the stretch's words: `POST {base}/audio/speech`
//!   with `authorization: Bearer <apikey>` and the canonical JSON body
//!   `{"input", "model", "response_format", "voice"}`
//!   ([`encoding_request`]); with `provider` `elevenlabs`,
//!   `POST {base}/text-to-speech/{voice}/stream?output_format=F` with
//!   `xi-api-key: <apikey>` and the body `{"model_id", "text"}` (its
//!   streaming `text-to-speech` API; `F` from [`FORMATS`]). Both answer
//!   the audio as it is synthesized, and the service reads each answer as
//!   it arrives (`outbound-http.send`), returning its chunks
//!   ([`chunks`]) before the answer ends. A reply without a pause
//!   is one request, and its answer is written as it is. With a pause (or
//!   drafted and said in more than one request), the
//!   answers are joined as signed 16-bit little-endian PCM (raw `pcm` at
//!   [`PCM_SAMPLE_RATE_HZ`] mono, a WAV answer at its own rate and
//!   channels, which every stretch shares, Gemini's PCM), each pause is its
//!   silence at its place ([`Pause::frames`]), and the PCM is written raw for `audio/pcm` or in a
//!   WAV container for `audio/wav`. The first failed answer ends the
//!   invocation, and nothing is written.
//! * with `provider` `gemini`, either direction: `POST {base}/interactions`
//!   with `x-goog-api-key: <apikey>` ([`gemini`] documents the bodies).
//!   Gemini answers each stretch whole, so each is one chunk.
//!
//! # Events
//!
//! `invoke` (the component's one export, an `async func`) writes at most
//! one event to the stream it returns, then resolves its result future (the
//! invoking session receives `done.invoke.<id>` after the event): with no
//! data, except that a media step that succeeds writes no event and
//! resolves with its result as the done data, and that an encoding writes
//! each of its chunks first, as it is cut (decision D203). The names and
//! data are those of the event schemas (`services/speech/schemas/` and
//! `abilities/schemas/`):
//!
//! * [`chunks::CHUNK_EVENT`] `{"audio": R, "sequence": S, "media_type": M,
//!   "sample_rate_hz": N, "channels": C, "duration": D}`, before the
//!   output, once per chunk: `R` is chunk `S`'s file (the `output`
//!   reference with `.<S>` before its extension), written before the event
//!   names it, `S` counts from 0, and `N`, `C` and `D` are those of its
//!   audio, as the output's are. The chunks in order are the whole reply,
//!   each pause a chunk of its silence; a failure after some chunks still
//!   answers [`FAILED_EVENT`], and the chunks' files stay.
//! * [`DECODING_OUTPUT_EVENT`] `{"text": T}`: the decoded speech (empty when
//!   the backend heard none).
//! * [`ENCODING_OUTPUT_EVENT`] `{"audio": R, "media_type": M,
//!   "sample_rate_hz": N, "channels": C, "duration": D}`: `R` is the
//!   `output` reference, the audio is in its file, and `N`, `C`, and `D`
//!   are numbers only when known ([`sound`]: raw `pcm` is 24 kHz mono
//!   16-bit, a WAV file declares its own, and `ElevenLabs` names its `mp3`
//!   rate), else `null`. `D` is the seconds of sound measured from the bytes
//!   of linear PCM.
//! * [`voice::DETECTION_OUTPUT_EVENT`], [`voice::DIARIZATION_OUTPUT_EVENT`],
//!   [`voice::IDENTIFICATION_OUTPUT_EVENT`], and
//!   [`voice::CLASSIFICATION_OUTPUT_EVENT`]: the voice operations' results
//!   ([`voice`]).
//! * [`FAILED_EVENT`] `{"message": M}`, `M` one closed message per
//!   [`FailureCode`] and direction.
//!
//! Only a drafted encoding reads events (see "Drafts"); every other
//! operation drops the event stream `invoke` receives.
//!
//! # Bounds
//!
//! The audio a decoding reads ([`MAX_AUDIO_BYTES`], inside the host's
//! request-body bound), the words an encoding sends (all its parts' words
//! joined, [`MAX_TEXT_BYTES`]), the PCM a reply with pauses joins
//! ([`MAX_ASSEMBLED_BYTES`]), the text a decoding returns ([`MAX_DECODED_TEXT_BYTES`], under the
//! session's event bound), a reference ([`MAX_REFERENCE_BYTES`]), and the
//! model name ([`MAX_MODEL_BYTES`]) are bounded on wasm builds only
//! ([`scxml_types::wasm_bound`]); the encoded audio of each request is
//! bounded by the host's response-body bound. The parts ([`MAX_PARTS`])
//! and each part's words ([`MAX_WORDS_CHARS`]) are bounded on every build,
//! as the schema's contract (decision D178).
//!
//! # Telemetry
//!
//! `invoke` records one observation through scxml.rs's `telemetry` WIT
//! facade (operation `start`: its closed outcome, `ok`, `rejected`, or
//! `failed`, and one closed signal event), and one closed signal event
//! (operation `send`) when it returns its last event (an encoding's chunks
//! record none). No attribute carries the
//! words, the text, the audio, a reference, the model, the voice, or a URL, so the
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

pub mod chunks;
pub mod drafting;
pub mod gemini;
pub mod stream;
pub mod voice;

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
/// The `ElevenLabs` streaming path after the voice: its answer's audio
/// arrives as it is synthesized.
pub const STREAM_PATH: &str = "/stream";
/// The encoded media type when the invocation names none: `mp3`, the
/// default of both backends.
pub const DEFAULT_MEDIA_TYPE: &str = "audio/mpeg";
/// The `content-type` of the decoded audio's file part when the invocation
/// names none.
pub const DEFAULT_AUDIO_MEDIA_TYPE: &str = "application/octet-stream";
/// A Gemini decoding's audio `mime_type` when the invocation names none.
pub const GEMINI_AUDIO_MEDIA_TYPE: &str = "audio/wav";

/// The decoded speech.
pub const DECODING_OUTPUT_EVENT: &str = "bot.ability.hearing.speech.decoding.output";
/// The encoded speech.
pub const ENCODING_OUTPUT_EVENT: &str = "bot.ability.vocal.speech.encoding.output";
/// The failure.
pub const FAILED_EVENT: &str = "bot.ability.failed";

/// Upper bound on the audio a decoding reads (wasm builds only): the host's
/// 4 MiB request-body bound less room for the form's other parts.
pub const MAX_AUDIO_BYTES: usize = wasm_bound(4 * 1024 * 1024 - 16 * 1024);
/// Upper bound on a request's body (wasm builds only): the host's 4 MiB
/// request-body bound, which a Gemini decoding's base64 audio reaches
/// before [`MAX_AUDIO_BYTES`] does.
pub const MAX_REQUEST_BYTES: usize = wasm_bound(4 * 1024 * 1024);
/// Upper bound on the words an encoding sends, all its parts' words joined
/// (wasm builds only).
pub const MAX_TEXT_BYTES: usize = wasm_bound(16 * 1024);
/// Upper bound on the PCM a reply with pauses joins, its silences included
/// (wasm builds only).
pub const MAX_ASSEMBLED_BYTES: usize = wasm_bound(64 * 1024 * 1024);
/// Upper bound on an encoding's parts.
pub const MAX_PARTS: usize = 64;
/// Upper bound on one part's words, in characters.
pub const MAX_WORDS_CHARS: usize = 2000;
/// The members a part's `expression` may have.
pub const EXPRESSION_FIELDS: [&str; 11] = [
    "instructions",
    "emotion",
    "style",
    "intensity",
    "pace",
    "pitch",
    "volume",
    "emphasis",
    "pause",
    "sound",
    "sound_at",
];
/// What a part's words must not hold: the characters of stage directions
/// and markup, which a voice would read aloud.
pub const UNSPOKEN_CHARS: [char; 9] = ['*', '<', '>', '[', ']', '{', '}', '(', ')'];
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
/// and `pcm` as `pcm_24000`.
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

/// The backend's API.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Provider {
    /// An `OpenAI`-compatible audio API.
    OpenaiCompat,
    /// `ElevenLabs`' `text-to-speech` API, encode only.
    Elevenlabs,
    /// The Gemini API's Interactions API ([`gemini`]).
    Gemini,
}

/// A validated decoding: speech to text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Decoding {
    /// The model name.
    pub model: String,
    /// The speech to decode.
    pub audio: Media,
    /// The file part's `content-type` (the audio part's `mime_type` for
    /// Gemini).
    pub media_type: String,
    /// The backend.
    pub provider: Provider,
}

/// A silence in a reply, short or long.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pause {
    /// `short`: a beat between phrases.
    Short,
    /// `long`: a held breath.
    Long,
}

impl Pause {
    /// How long the pause is silent: 0.4
    /// seconds short, 1 second long.
    #[must_use]
    pub const fn seconds(self) -> f64 {
        match self {
            Self::Short => 0.4,
            Self::Long => 1.0,
        }
    }

    /// The whole frames of its silence at `sample_rate_hz`: its seconds
    /// times the rate, rounded.
    #[must_use]
    pub fn frames(self, sample_rate_hz: u32) -> usize {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "at most a second of a u32 rate: finite, non-negative, and below 2^32"
        )]
        let frames = (self.seconds() * f64::from(sample_rate_hz)).round() as u64;
        usize::try_from(frames).unwrap_or(usize::MAX)
    }
}

/// One stretch of a reply between pauses: one synthesis request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stretch {
    /// The pause that opens it.
    pub pause: Option<Pause>,
    /// Its parts' words, joined verbatim; blank words send no request.
    pub words: String,
}

/// A validated encoding: a reply to speech.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Encoding {
    /// The model name.
    pub model: String,
    /// The reply's stretches, in order: one unless a part pauses. None
    /// when drafted: they arrive as events ([`drafting`]).
    pub stretches: Vec<Stretch>,
    /// Whether the reply is drafted: no `parts`, read from the events the
    /// invoking session sends ([`drafting`]).
    pub drafted: bool,
    /// The voice.
    pub voice: String,
    /// Where the audio is written.
    pub output: Media,
    /// The encoded format.
    pub format: Format,
    /// The backend.
    pub provider: Provider,
}

impl Encoding {
    /// The words each request says, in order: those of every stretch whose
    /// words are not blank.
    pub fn said(&self) -> impl Iterator<Item = &str> {
        self.stretches
            .iter()
            .map(|stretch| stretch.words.as_str())
            .filter(|words| !words.trim().is_empty())
    }

    /// Whether the reply pauses.
    #[must_use]
    pub fn pauses(&self) -> bool {
        self.stretches.iter().any(|stretch| stretch.pause.is_some())
    }
}

/// One validated invocation.
#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    /// `decode`.
    Decode(Decoding),
    /// `encode`.
    Encode(Encoding),
    /// `detect` (voice detection).
    Detect(voice::Detection),
    /// `diarize` (voice diarization).
    Diarize(voice::Diarization),
    /// `identify` (voice identification).
    Identify(voice::Identification),
    /// `classify` (a voice segment's affect).
    Classify(voice::Classification),
    /// `normalize` (a sound to PCM).
    Normalize(voice::Normalization),
    /// `append` (open speech to one utterance).
    Append(voice::Appending),
    /// `clip` (a voice segment from a span).
    Clip(voice::Clipping),
    /// `stream` (a streaming hearing session, [`stream`]).
    Stream(stream::Streaming),
}

impl Operation {
    /// What the operation is, as its failure message begins.
    #[must_use]
    pub const fn direction(&self) -> &'static str {
        match self {
            Self::Decode(_) => "Speech decoding",
            Self::Encode(_) => "Speech encoding",
            Self::Detect(_) => "Voice detection",
            Self::Diarize(_) => "Voice diarization",
            Self::Identify(_) => "Voice identification",
            Self::Classify(_) => "Affect classification",
            Self::Normalize(_) => "Sound normalization",
            Self::Append(_) => "Open speech",
            Self::Clip(_) => "Voice segment clipping",
            Self::Stream(_) => "Speech streaming",
        }
    }

    /// Whether the operation calls the backend (the media steps do not).
    #[must_use]
    pub const fn calls_backend(&self) -> bool {
        !matches!(self, Self::Normalize(_) | Self::Append(_) | Self::Clip(_))
    }
}

/// The closed failures of [`FAILED_EVENT`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureCode {
    /// The `baseurl` slot names no variable the run holds.
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
    /// The audio is not signed 16-bit PCM with a known rate and channels.
    Format,
    /// The span holds no audio.
    Span,
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
            Self::Format => "the audio is not signed 16-bit PCM with a known format",
            Self::Span => "the span holds no audio",
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
        Self::failed_named(operation.direction(), code)
    }

    /// [`FAILED_EVENT`] with the closed message of `code` for `direction`.
    #[must_use]
    pub fn failed_named(direction: &'static str, code: FailureCode) -> Self {
        Self {
            name: FAILED_EVENT,
            data: canonical_json(&serde_json::json!({
                "message": format!("{direction} failed: {}.", code.reason()),
            })),
        }
    }
}

/// How an invocation ends: the one event it returns (none for a media step
/// that succeeded), then its done data (a media step's result).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Finished {
    /// The event returned before completing.
    pub event: Option<Returned>,
    /// The canonical JSON object `done.invoke.<id>` carries.
    pub done: Option<String>,
}

impl Finished {
    /// One event, then no done data.
    #[must_use]
    pub const fn event(returned: Returned) -> Self {
        Self {
            event: Some(returned),
            done: None,
        }
    }

    /// A media step's result: no event, then its done data, or its failure.
    #[must_use]
    pub fn step(operation: &Operation, result: Result<String, FailureCode>) -> Self {
        match result {
            Ok(done) => Self {
                event: None,
                done: Some(done),
            },
            Err(code) => Self::event(Returned::failed(operation, code)),
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

/// A `stream` invocation of its `language` and `chunk_seconds`.
fn stream_operation(
    language: Option<&Data>,
    chunk_seconds: Option<&Data>,
) -> Result<Operation, &'static str> {
    let language = match language {
        None => None,
        Some(Data::String(language)) => Some(language.as_str()),
        Some(_) => return Err("speech stream language must be a string"),
    };
    let chunk_seconds = match chunk_seconds {
        None => None,
        Some(Data::Number(seconds)) => Some(seconds.get()),
        Some(_) => return Err("speech stream chunk_seconds must be a number"),
    };
    stream::streaming(language, chunk_seconds).map(Operation::Stream)
}

/// Parses one voice operation's parameters.
type VoiceParser = fn(&voice::Given<'_>) -> Result<Operation, &'static str>;

/// The validated operation of an invocation's parameters (see the crate
/// documentation, "Parameters").
///
/// # Errors
///
/// A static detail for an unknown or repeated parameter, one the operation
/// does not take, a missing or malformed one, or one outside its bound.
#[expect(
    clippy::too_many_lines,
    reason = "one slot per parameter of every operation"
)]
pub fn parse_parameters<'a>(
    parameters: impl IntoIterator<Item = (&'a str, &'a Data)>,
) -> Result<Operation, &'static str> {
    let mut operation = None;
    let mut model = None;
    let mut audio = None;
    let mut media_type = None;
    let mut parts = None;
    let mut voice = None;
    let mut output = None;
    let mut provider = None;
    let mut language = None;
    let mut chunk_seconds = None;
    let mut given = voice::Given::default();
    for (name, value) in parameters {
        // A null value is a parameter not given (an invoker passes its
        // unset data as null).
        if matches!(value, Data::Null) {
            continue;
        }
        let slot = match name {
            "operation" => &mut operation,
            "model" => &mut model,
            "audio" => &mut audio,
            "media_type" => &mut media_type,
            "parts" => &mut parts,
            "voice" => &mut voice,
            "output" => &mut output,
            "provider" => &mut provider,
            "language" => &mut language,
            "chunk_seconds" => &mut chunk_seconds,
            "sample_rate_hz" => &mut given.sample_rate_hz,
            "channels" => &mut given.channels,
            "frames" => &mut given.frames,
            "segments" => &mut given.segments,
            "wav" => &mut given.wav,
            "start_seconds" => &mut given.start_seconds,
            "end_seconds" => &mut given.end_seconds,
            _ => return Err("speech takes no parameter of that name"),
        };
        if slot.replace(value).is_some() {
            return Err("speech takes each parameter once");
        }
    }
    let operation = operation.ok_or("speech requires an operation parameter")?;
    if matches!(operation, Data::String(name) if name == "stream") {
        if model.is_some()
            || audio.is_some()
            || media_type.is_some()
            || parts.is_some()
            || voice.is_some()
            || output.is_some()
            || provider.is_some()
            || !given.names().is_empty()
        {
            return Err("speech stream takes language and chunk_seconds only");
        }
        return stream_operation(language, chunk_seconds);
    }
    if language.is_some() || chunk_seconds.is_some() {
        return Err("speech takes language and chunk_seconds only to stream");
    }
    let model = model
        .map(|model| {
            text_parameter(model, "speech model must be a non-empty string")
                .map(|model| model.trim().to_owned())
        })
        .transpose()?;
    if model
        .as_ref()
        .is_some_and(|model| exceeds_wasm_bound(model.len(), MAX_MODEL_BYTES))
    {
        return Err("speech model exceeds its bound");
    }
    if let Data::String(name) = operation {
        let voice_operation: Option<VoiceParser> = match name.as_str() {
            "detect" => Some(|given| voice::detection(given).map(Operation::Detect)),
            "diarize" => Some(|given| voice::diarization(given).map(Operation::Diarize)),
            "identify" => Some(|given| voice::identification(given).map(Operation::Identify)),
            "classify" => Some(|given| voice::classification(given).map(Operation::Classify)),
            "normalize" => Some(|given| voice::normalization(given).map(Operation::Normalize)),
            "append" => Some(|given| voice::appending(given).map(Operation::Append)),
            "clip" => Some(|given| voice::clipping(given).map(Operation::Clip)),
            _ => None,
        };
        if let Some(parse) = voice_operation {
            if parts.is_some() || voice.is_some() || provider.is_some() {
                return Err("speech takes parts, voice, and provider only to encode");
            }
            given.model = model;
            given.audio = audio;
            given.media_type = media_type;
            given.output = output;
            return parse(&given);
        }
    }
    if given.sample_rate_hz.is_some()
        || given.channels.is_some()
        || given.frames.is_some()
        || given.segments.is_some()
        || given.wav.is_some()
        || given.start_seconds.is_some()
        || given.end_seconds.is_some()
    {
        return Err("speech takes that parameter only for a voice operation");
    }
    let model = model.ok_or("speech requires a model parameter")?;
    match operation {
        Data::String(name) if name == "decode" => {
            if parts.is_some() || voice.is_some() || output.is_some() {
                return Err("speech decode takes model, audio, media_type, and provider only");
            }
            decoding(model, audio, media_type, provider).map(Operation::Decode)
        }
        Data::String(name) if name == "encode" => {
            if audio.is_some() {
                return Err("speech encode takes no audio parameter");
            }
            encoding(model, parts, voice, output, media_type, provider).map(Operation::Encode)
        }
        _ => Err(
            "speech operation must be decode, encode, stream, detect, diarize, identify, classify, normalize, append, or clip",
        ),
    }
}

fn decoding(
    model: String,
    audio: Option<&Data>,
    media_type: Option<&Data>,
    provider: Option<&Data>,
) -> Result<Decoding, &'static str> {
    let audio = media_parameter(
        audio.ok_or("speech decode requires an audio parameter")?,
        "speech audio must be a file reference",
    )?;
    let provider = match provider {
        None => Provider::OpenaiCompat,
        Some(Data::String(name)) if name == "gemini" => Provider::Gemini,
        Some(_) => return Err("speech decode provider must be gemini"),
    };
    let media_type = match media_type {
        None if provider == Provider::Gemini => GEMINI_AUDIO_MEDIA_TYPE.to_owned(),
        None => DEFAULT_AUDIO_MEDIA_TYPE.to_owned(),
        Some(Data::String(media_type)) if is_media_type(media_type) => media_type.clone(),
        Some(_) => return Err("speech media_type must be a type/subtype"),
    };
    Ok(Decoding {
        model,
        audio,
        media_type,
        provider,
    })
}

/// What a part's expression directs that the service reads: its pause, and
/// whether it makes a sound.
fn expression_cues(expression: &Json) -> Result<(Option<Pause>, bool), &'static str> {
    const INVALID: &str = "speech parts' expression must be an object of Expression fields";
    let expression = match expression {
        Json::Null => return Ok((None, false)),
        Json::Object(expression) => expression,
        _ => return Err(INVALID),
    };
    if expression
        .keys()
        .any(|name| !EXPRESSION_FIELDS.contains(&name.as_str()))
    {
        return Err(INVALID);
    }
    let pause = match expression.get("pause") {
        None | Some(Json::Null) => None,
        Some(Json::String(pause)) if pause == "short" => Some(Pause::Short),
        Some(Json::String(pause)) if pause == "long" => Some(Pause::Long),
        Some(_) => return Err("speech parts' pause must be short or long"),
    };
    let sound = match expression.get("sound") {
        None | Some(Json::Null) => false,
        Some(Json::String(_)) => true,
        Some(_) => return Err("speech parts' sound must be a string"),
    };
    Ok((pause, sound))
}

/// The stretches of an encoding's `parts` (see the crate documentation,
/// "Parts").
fn stretches(parts: &Data) -> Result<Vec<Stretch>, &'static str> {
    const INVALID: &str = "speech parts must be 1 to 64 parts of {expression?, words?}";
    let Data::JsonArray(parts) = parts else {
        return Err(INVALID);
    };
    let Ok(Json::Array(parts)) = serde_json::from_str::<Json>(parts) else {
        return Err(INVALID);
    };
    if parts.is_empty() || parts.len() > MAX_PARTS {
        return Err(INVALID);
    }
    let mut stretches = Vec::new();
    let mut stretch = Stretch {
        pause: None,
        words: String::new(),
    };
    let mut worded = false;
    let mut said = 0_usize;
    for part in &parts {
        let Json::Object(part) = part else {
            return Err(INVALID);
        };
        if part
            .keys()
            .any(|name| name != "expression" && name != "words")
        {
            return Err(INVALID);
        }
        let (pause, sound) = expression_cues(part.get("expression").unwrap_or(&Json::Null))?;
        let words = match part.get("words") {
            None | Some(Json::Null) => None,
            Some(Json::String(words)) if words.chars().count() > MAX_WORDS_CHARS => {
                return Err("speech parts' words must be at most 2000 characters");
            }
            Some(Json::String(words))
                if !words.trim().is_empty() && !words.contains(UNSPOKEN_CHARS) =>
            {
                Some(words.as_str())
            }
            Some(_) => {
                return Err("speech parts' words must not be blank or hold * < > [ ] { } ( )");
            }
        };
        if words.is_none() && pause.is_none() && !sound {
            return Err("speech parts must each have words, a pause, or a sound");
        }
        if let Some(pause) = pause {
            // A pause opens a new stretch; the reply's first opens its first.
            if stretch.pause.is_some() || !stretch.words.is_empty() {
                stretches.push(std::mem::replace(
                    &mut stretch,
                    Stretch {
                        pause: Some(pause),
                        words: String::new(),
                    },
                ));
            } else {
                stretch.pause = Some(pause);
            }
        }
        if let Some(words) = words {
            worded = true;
            said = said.saturating_add(words.len());
            stretch.words.push_str(words);
        }
    }
    if !worded {
        return Err("speech parts have no words");
    }
    if exceeds_wasm_bound(said, MAX_TEXT_BYTES) {
        return Err("speech parts' words exceed their bound");
    }
    stretches.push(stretch);
    Ok(stretches)
}

fn encoding(
    model: String,
    parts: Option<&Data>,
    voice: Option<&Data>,
    output: Option<&Data>,
    media_type: Option<&Data>,
    provider: Option<&Data>,
) -> Result<Encoding, &'static str> {
    // Without parts, the reply is drafted: it arrives as events.
    let stretches = parts.map(stretches).transpose()?.unwrap_or_default();
    let drafted = parts.is_none();
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
        Some(Data::String(name)) if name == "gemini" => Provider::Gemini,
        Some(_) => return Err("speech encode provider must be elevenlabs or gemini"),
    };
    let format = match media_type {
        None => DEFAULT_MEDIA_TYPE,
        Some(Data::String(media_type)) => media_type.as_str(),
        Some(_) => return Err("speech media_type must be a string"),
    };
    let format = FORMATS
        .into_iter()
        .find(|known| known.media_type == format)
        .filter(|known| match provider {
            Provider::OpenaiCompat => true,
            Provider::Elevenlabs => known.elevenlabs.is_some(),
            Provider::Gemini => matches!(known.response_format, "pcm" | "wav"),
        })
        .ok_or("speech media_type is not one the backend encodes")?;
    if stretches.iter().any(|stretch| stretch.pause.is_some())
        && !matches!(format.response_format, "pcm" | "wav")
    {
        return Err("speech pauses need audio/pcm or audio/wav");
    }
    if drafted && !matches!(format.response_format, "pcm" | "wav") {
        return Err("speech drafts need audio/pcm or audio/wav");
    }
    Ok(Encoding {
        model,
        stretches,
        drafted,
        voice,
        output,
        format,
        provider,
    })
}

/// The invocation's endpoint, `(baseurl, apikey)`: the references the host
/// handed for the two slots. There is no default backend, so the base URL
/// is required; without a key the backend is keyless and no request
/// carries a key header (decision D127).
///
/// # Errors
///
/// [`FailureCode::Unconfigured`] when the `baseurl` slot names no variable
/// the run holds (the host hands no reference for it): nothing is read or
/// sent.
pub const fn endpoint(
    base_url: Option<u32>,
    api_key: Option<u32>,
) -> Result<(u32, Option<u32>), FailureCode> {
    match base_url {
        Some(base_url) => Ok((base_url, api_key)),
        None => Err(FailureCode::Unconfigured),
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

/// The `authorization` bearer header of `api_key`; none for a keyless
/// backend.
fn bearer(api_key: Option<u32>) -> Option<(String, Vec<Part>)> {
    api_key.map(|api_key| {
        (
            "authorization".into(),
            vec![Part::Literal("Bearer ".into()), Part::Reference(api_key)],
        )
    })
}

/// The header `name` carrying `api_key` as it is, then `headers`; no key
/// header for a keyless backend.
fn keyed(
    name: &str,
    api_key: Option<u32>,
    headers: impl IntoIterator<Item = (String, Vec<Part>)>,
) -> Vec<(String, Vec<Part>)> {
    api_key
        .map(|api_key| (name.to_owned(), vec![Part::Reference(api_key)]))
        .into_iter()
        .chain(headers)
        .collect()
}

/// The transcription request: `POST {base}/audio/transcriptions` with the
/// `apikey` slot's reference as a bearer key and a `multipart/form-data`
/// body of `model`, `response_format` (`json`), and `file` (`audio`).
#[must_use]
pub fn decoding_request(
    base_url: u32,
    api_key: Option<u32>,
    decoding: &Decoding,
    audio: &[u8],
) -> Request {
    if decoding.provider == Provider::Gemini {
        return gemini::decoding_request(base_url, api_key, decoding, audio);
    }
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

/// The speech request of one stretch's `words`: `POST {base}/audio/speech`
/// with a bearer key and the canonical JSON body `{"input", "model",
/// "response_format", "voice"}`, or, for `ElevenLabs`,
/// `POST {base}/text-to-speech/{voice}/stream?output_format=F` (its
/// streaming endpoint) with `xi-api-key` and the body `{"model_id",
/// "text"}`.
#[must_use]
pub fn encoding_request(
    base_url: u32,
    api_key: Option<u32>,
    encoding: &Encoding,
    words: &str,
) -> Request {
    if encoding.provider == Provider::Gemini {
        return gemini::encoding_request(base_url, api_key, encoding, words);
    }
    let content_type = (
        "content-type".to_owned(),
        vec![Part::Literal("application/json".into())],
    );
    let mut body = Map::new();
    if let (Provider::Elevenlabs, Some((output_format, _))) =
        (encoding.provider, encoding.format.elevenlabs)
    {
        body.insert("model_id".into(), encoding.model.clone().into());
        body.insert("text".into(), words.into());
        return Request {
            url: vec![
                Part::Reference(base_url),
                Part::Literal(format!(
                    "{TEXT_TO_SPEECH_PATH}{}{STREAM_PATH}?output_format={output_format}",
                    encoding.voice
                )),
            ],
            headers: keyed("xi-api-key", api_key, [content_type]),
            body: canonical_json(&Json::Object(body)).into_bytes(),
        };
    }
    body.insert("input".into(), words.into());
    body.insert("model".into(), encoding.model.clone().into());
    body.insert(
        "response_format".into(),
        encoding.format.response_format.into(),
    );
    body.insert("voice".into(), encoding.voice.clone().into());
    Request {
        url: vec![Part::Reference(base_url), Part::Literal(SPEECH_PATH.into())],
        headers: bearer(api_key).into_iter().chain([content_type]).collect(),
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
    let gemini =
        matches!(operation, Operation::Decode(decoding) if decoding.provider == Provider::Gemini);
    let text = answered_body(answered).and_then(|body| {
        let text = if gemini {
            gemini::transcript(&body)?
        } else {
            let answer: Json = serde_json::from_slice(&body).map_err(|_| FailureCode::Malformed)?;
            match answer.get("text") {
                Some(Json::String(text)) => text.trim().to_owned(),
                _ => return Err(FailureCode::Malformed),
            }
        };
        if exceeds_wasm_bound(text.len(), MAX_DECODED_TEXT_BYTES) {
            return Err(FailureCode::Malformed);
        }
        Ok(text)
    });
    match text {
        Ok(text) => Returned {
            name: DECODING_OUTPUT_EVENT,
            data: canonical_json(&serde_json::json!({ "text": text })),
        },
        Err(code) => Returned::failed(operation, code),
    }
}

/// The sound fields of encoded audio, each only when known.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sound {
    /// Samples per second.
    pub sample_rate_hz: Option<u32>,
    /// Channels.
    pub channels: Option<u16>,
    /// Seconds of sound, measured from the bytes of linear PCM.
    pub duration: Option<f64>,
}

impl Sound {
    const UNKNOWN: Self = Self {
        sample_rate_hz: None,
        channels: None,
        duration: None,
    };

    /// Linear PCM of `bytes` sample bytes: its duration is the byte count
    /// over the frame rate.
    fn linear(rate: u32, channels: u16, bytes_per_sample: u16, bytes: usize) -> Self {
        let frame_rate = f64::from(rate) * f64::from(channels) * f64::from(bytes_per_sample);
        #[expect(
            clippy::cast_precision_loss,
            reason = "audio byte counts are far below 2^52"
        )]
        let duration = bytes as f64 / frame_rate;
        Self {
            sample_rate_hz: Some(rate),
            channels: Some(channels),
            duration: (duration > 0.0 && duration.is_finite()).then_some(duration),
        }
    }
}

/// The rate of raw `pcm`: 16-bit signed little-endian mono samples at
/// 24 kHz, as the `OpenAI` speech API documents its `pcm` and `ElevenLabs`
/// names `pcm_24000`.
pub const PCM_SAMPLE_RATE_HZ: u32 = 24_000;

/// The sound a WAV file declares: its `fmt ` chunk's rate and channels, and,
/// for integer PCM (format 1), the duration of its `data` chunk (a streamed
/// file's unknown `data` size counts the bytes present).
#[must_use]
pub fn wav_sound(audio: &[u8]) -> Option<Sound> {
    if audio.get(..4)? != b"RIFF" || audio.get(8..12)? != b"WAVE" {
        return None;
    }
    let mut format = None;
    let mut offset = 12_usize;
    while let Some(id) = audio.get(offset..offset + 4) {
        let size = u32::from_le_bytes(audio.get(offset + 4..offset + 8)?.try_into().ok()?);
        let body = offset + 8;
        let size = usize::try_from(size).ok()?;
        if id == b"fmt " {
            let field = |at: usize| -> Option<u16> {
                Some(u16::from_le_bytes(
                    audio.get(body + at..body + at + 2)?.try_into().ok()?,
                ))
            };
            let tag = field(0)?;
            let channels = field(2)?;
            let rate = u32::from_le_bytes(audio.get(body + 4..body + 8)?.try_into().ok()?);
            let bits = field(14)?;
            if channels == 0 || rate == 0 {
                return None;
            }
            format = Some((tag, rate, channels, bits));
        } else if id == b"data" {
            let (tag, rate, channels, bits) = format?;
            let present = audio.len().saturating_sub(body).min(size);
            return Some(if tag == 1 && bits > 0 && bits % 8 == 0 {
                Sound::linear(rate, channels, bits / 8, present)
            } else {
                Sound {
                    sample_rate_hz: Some(rate),
                    channels: Some(channels),
                    duration: None,
                }
            });
        }
        // Chunks are padded to an even size.
        offset = body.checked_add(size)?.checked_add(size % 2)?;
    }
    format.map(|(_, rate, channels, _)| Sound {
        sample_rate_hz: Some(rate),
        channels: Some(channels),
        duration: None,
    })
}

/// The sound of encoded audio in `format` from `provider`: raw `pcm` is
/// [`PCM_SAMPLE_RATE_HZ`] mono 16-bit, a WAV file declares its own, and
/// `ElevenLabs`' `mp3` names its rate; anything else is unknown.
#[must_use]
pub fn sound(provider: Provider, format: Format, audio: &[u8]) -> Sound {
    match format.response_format {
        "pcm" => Sound::linear(PCM_SAMPLE_RATE_HZ, 1, 2, audio.len()),
        "wav" => wav_sound(audio).unwrap_or(Sound::UNKNOWN),
        _ => match (provider, format.elevenlabs) {
            (Provider::Elevenlabs, Some((_, rate))) => Sound {
                sample_rate_hz: Some(rate),
                ..Sound::UNKNOWN
            },
            _ => Sound::UNKNOWN,
        },
    }
}

/// The audio of a reply without a pause: its one answer as it is, or, from
/// Gemini, its PCM raw or in a WAV container.
fn answered_audio(
    encoding: &Encoding,
    answered: Result<Response, TransportFailure>,
) -> Result<Vec<u8>, FailureCode> {
    let body = answered_body(answered)?;
    if encoding.provider == Provider::Gemini {
        let pcm = gemini::audio(&body)?;
        return Ok(if encoding.format.response_format == "wav" {
            voice::wav_container(&pcm, PCM_SAMPLE_RATE_HZ, 1)
        } else {
            pcm
        });
    }
    if body.is_empty() {
        return Err(FailureCode::Malformed);
    }
    Ok(body)
}

/// One stretch's answer as signed 16-bit PCM of whole frames: its rate,
/// channels, and samples.
fn answered_pcm(
    encoding: &Encoding,
    answered: Result<Response, TransportFailure>,
) -> Result<(u32, u16, Vec<u8>), FailureCode> {
    let body = answered_body(answered)?;
    let (rate, channels, pcm) = if encoding.provider == Provider::Gemini {
        (PCM_SAMPLE_RATE_HZ, 1, gemini::audio(&body)?)
    } else if encoding.format.response_format == "wav" {
        voice::wav_16bit(&body).map_err(|_| FailureCode::Malformed)?
    } else {
        (PCM_SAMPLE_RATE_HZ, 1, body)
    };
    if pcm.is_empty()
        || rate > voice::MAX_SAMPLE_RATE_HZ
        || channels > voice::MAX_CHANNELS
        || !pcm.len().is_multiple_of(2 * usize::from(channels))
    {
        return Err(FailureCode::Malformed);
    }
    Ok((rate, channels, pcm))
}

/// The audio of a reply with a pause, or said in more than one request:
/// the stretches' answers in order as
/// one PCM, each pause its silence at its place, raw for `audio/pcm` or in
/// a WAV container for `audio/wav`.
fn assembled(
    encoding: &Encoding,
    answered: impl IntoIterator<Item = Result<Response, TransportFailure>>,
) -> Result<Vec<u8>, FailureCode> {
    let mut answered = answered.into_iter();
    let mut voiced = Vec::with_capacity(encoding.stretches.len());
    let mut shape = None;
    for stretch in &encoding.stretches {
        if stretch.words.trim().is_empty() {
            voiced.push(None);
            continue;
        }
        let (rate, channels, pcm) =
            answered_pcm(encoding, answered.next().ok_or(FailureCode::Malformed)?)?;
        // Every stretch is one voice in one format.
        if shape
            .replace((rate, channels))
            .is_some_and(|known| known != (rate, channels))
        {
            return Err(FailureCode::Malformed);
        }
        voiced.push(Some(pcm));
    }
    let (rate, channels) = shape.ok_or(FailureCode::Malformed)?;
    let frame = 2 * usize::from(channels);
    let mut pcm = Vec::new();
    for (stretch, words) in encoding.stretches.iter().zip(voiced) {
        let silence = stretch
            .pause
            .map_or(Some(0), |pause| pause.frames(rate).checked_mul(frame));
        let spoken = words.as_ref().map_or(0, Vec::len);
        let silent = silence
            .and_then(|silence| pcm.len().checked_add(silence))
            .filter(|&silent| {
                silent
                    .checked_add(spoken)
                    .is_some_and(|length| !exceeds_wasm_bound(length, MAX_ASSEMBLED_BYTES))
            })
            .ok_or(FailureCode::Malformed)?;
        pcm.resize(silent, 0);
        if let Some(words) = words {
            pcm.extend_from_slice(&words);
        }
    }
    Ok(if encoding.format.response_format == "wav" {
        voice::wav_container(&pcm, rate, channels)
    } else {
        pcm
    })
}

/// What a reply's settled speech requests become, `answered` their answers
/// in order (one per stretch whose words are not blank, ending at the first
/// that failed): the audio written to the output's file by `write`, then
/// [`ENCODING_OUTPUT_EVENT`], or [`FAILED_EVENT`] with the first failure
/// (nothing is written).
#[must_use]
pub fn encoding_outcome(
    operation: &Operation,
    answered: impl IntoIterator<Item = Result<Response, TransportFailure>>,
    write: impl FnOnce(&str, &[u8]) -> std::io::Result<()>,
) -> Returned {
    let Operation::Encode(encoding) = operation else {
        return Returned::failed(operation, FailureCode::Malformed);
    };
    let audio = if encoding.pauses() || encoding.said().nth(1).is_some() {
        assembled(encoding, answered)
    } else {
        answered
            .into_iter()
            .next()
            .ok_or(FailureCode::Malformed)
            .and_then(|answered| answered_audio(encoding, answered))
    };
    let audio = match audio {
        Ok(audio) => audio,
        Err(code) => return Returned::failed(operation, code),
    };
    if write(&encoding.output.path, &audio).is_err() {
        return Returned::failed(operation, FailureCode::Unwritable);
    }
    let sound = sound(encoding.provider, encoding.format, &audio);
    Returned {
        name: ENCODING_OUTPUT_EVENT,
        data: canonical_json(&serde_json::json!({
            "audio": encoding.output.reference,
            "media_type": encoding.format.media_type,
            "sample_rate_hz": sound.sample_rate_hz,
            "channels": sound.channels,
            "duration": sound.duration,
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
    use stateforward::scxml::outbound_websocket::{
        self, Message, WebsocketHeader, WebsocketRequest,
    };
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
    type Plan = (Operation, Option<(u32, Option<u32>)>);

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
            .map_err(|failure| transport_failure(&failure))
    }

    /// Runs the accepted operation to how it ends, `None` when the invoking
    /// session stopped reading `returned` (an encoding returns its chunks
    /// there as they are written).
    async fn run(
        operation: Operation,
        endpoint: Option<(u32, Option<u32>)>,
        events: Option<wit_bindgen::StreamReader<Event>>,
        returned: &mut wit_bindgen::StreamWriter<Event>,
    ) -> Option<super::Finished> {
        use super::{Finished, voice};
        match &operation {
            Operation::Normalize(normalization) => {
                return Some(Finished::step(
                    &operation,
                    voice::normalized(normalization, super::read_audio, super::write_audio),
                ));
            }
            Operation::Append(appending) => {
                return Some(Finished::step(
                    &operation,
                    voice::appended(appending, super::read_audio, super::write_audio),
                ));
            }
            Operation::Clip(clipping) => {
                return Some(Finished::step(
                    &operation,
                    voice::clipped(clipping, super::read_audio, super::write_audio),
                ));
            }
            _ => {}
        }
        let Some((base_url, api_key)) = endpoint else {
            return Some(Finished::event(Returned::failed(
                &operation,
                super::FailureCode::Unconfigured,
            )));
        };
        let failed = |code| Some(Finished::event(Returned::failed(&operation, code)));
        Some(Finished::event(match &operation {
            Operation::Decode(decoding) => {
                let audio = match super::read_audio(&decoding.audio.path) {
                    Ok(audio) => audio,
                    Err(code) => return failed(code),
                };
                let built = super::decoding_request(base_url, api_key, decoding, &audio);
                drop(audio);
                if scxml_types::exceeds_wasm_bound(built.body.len(), super::MAX_REQUEST_BYTES) {
                    return failed(super::FailureCode::TooLarge);
                }
                let answered = fetch(built).await;
                super::decoding_outcome(&operation, answered)
            }
            Operation::Encode(encoding) => {
                if let Some(events) = events {
                    return encode_drafted(
                        &operation, encoding, base_url, api_key, events, returned,
                    )
                    .await;
                }
                return encode(&operation, encoding, base_url, api_key, returned).await;
            }
            Operation::Stream(streaming) => {
                let events = events?;
                return stream(streaming, base_url, api_key, events, returned).await;
            }
            _ => return Some(run_voice(&operation, base_url, api_key).await),
        }))
    }

    /// The next item of a stream, with the stream handed back, so a read
    /// can wait beside another without being cancelled.
    type Next<T> = std::pin::Pin<
        Box<dyn std::future::Future<Output = (Option<T>, wit_bindgen::StreamReader<T>)>>,
    >;

    fn next_event(mut reader: wit_bindgen::StreamReader<Event>) -> Next<Event> {
        Box::pin(async move { (reader.next().await, reader) })
    }

    fn next_message(mut reader: wit_bindgen::StreamReader<Message>) -> Next<Message> {
        Box::pin(async move { (reader.next().await, reader) })
    }

    /// What woke a streaming session.
    enum Woke {
        /// An event from the invoking session (none: it stopped sending).
        Session(Option<Event>, wit_bindgen::StreamReader<Event>),
        /// A message from the server (none: the connection ended).
        Server(Option<Message>, wit_bindgen::StreamReader<Message>),
    }

    /// The most audio held before the server is ready: 2 s at 16 kHz
    /// (mosfet.py's `MAX_UNREADY_SECONDS`); the oldest is let go.
    const MAX_UNREADY_BYTES: usize = 2 * 2 * 16_000;

    /// A streaming hearing session ([`super::stream`]): `None` when the
    /// invoking session stopped reading or sending, else how it was lost.
    async fn stream(
        streaming: &super::stream::Streaming,
        base_url: u32,
        api_key: Option<u32>,
        events: wit_bindgen::StreamReader<Event>,
        returned: &mut wit_bindgen::StreamWriter<Event>,
    ) -> Option<super::Finished> {
        use super::stream::{AUDIO_EVENT, Decoder, Lost, PRODUCING_EVENT, Producing, READY_EVENT};
        let lost = |lost: Lost| {
            Some(super::Finished::event(Returned {
                name: super::stream::FAILED_EVENT,
                data: lost.data(),
            }))
        };
        let request = WebsocketRequest {
            url: vec![
                TextPart::Reference(base_url),
                TextPart::Literal(super::stream::stream_path(streaming)),
            ],
            headers: super::bearer(api_key)
                .map(|(name, value)| WebsocketHeader {
                    name,
                    value: text_parts(&value),
                })
                .into_iter()
                .collect(),
            protocols: Vec::new(),
        };
        let (mut outgoing, outgoing_reader) = wit_stream::new::<Message>();
        let Ok((_, incoming, end)) = outbound_websocket::connect(request, outgoing_reader).await
        else {
            return lost(Lost::Unavailable);
        };
        let mut decoder = Decoder::default();
        let mut producing = Producing::default();
        let mut ready = false;
        let mut unready: std::collections::VecDeque<Vec<u8>> = std::collections::VecDeque::new();
        let mut held = 0_usize;
        let mut from_session = Some(next_event(events));
        let mut from_server = Some(next_message(incoming));
        loop {
            let woke = std::future::poll_fn(|cx| {
                if let Some(next) = from_session.as_mut()
                    && let std::task::Poll::Ready((item, reader)) = next.as_mut().poll(cx)
                {
                    from_session = None;
                    return std::task::Poll::Ready(Woke::Session(item, reader));
                }
                if let Some(next) = from_server.as_mut()
                    && let std::task::Poll::Ready((item, reader)) = next.as_mut().poll(cx)
                {
                    from_server = None;
                    return std::task::Poll::Ready(Woke::Server(item, reader));
                }
                std::task::Poll::Pending
            })
            .await;
            match woke {
                Woke::Session(None, _) => {
                    // The invoking session stopped sending: say so, and close.
                    let _ = outgoing
                        .write_one(Message::Text("{\"type\":\"end\"}".into()))
                        .await;
                    return None;
                }
                Woke::Session(Some(sent), reader) => {
                    from_session = Some(next_event(reader));
                    let Ok(scxml_types::DataValue::JsonObject(object)) = native_value(sent.data)
                    else {
                        continue;
                    };
                    let data =
                        serde_json::from_str::<serde_json::Value>(&object).unwrap_or_default();
                    let message = match sent.name.as_str() {
                        AUDIO_EVENT => match heard_audio(&data) {
                            Some(pcm) if ready => Message::Binary(pcm),
                            Some(pcm) => {
                                held += pcm.len();
                                unready.push_back(pcm);
                                while held > MAX_UNREADY_BYTES {
                                    held -= unready.pop_front().map_or(held, |old| old.len());
                                }
                                continue;
                            }
                            None => continue,
                        },
                        PRODUCING_EVENT => match producing.change(&data) {
                            Some(text) => Message::Text(text),
                            None => continue,
                        },
                        _ => continue,
                    };
                    if outgoing.write_one(message).await.is_some() {
                        return lost(Lost::Unavailable);
                    }
                }
                Woke::Server(Some(Message::Text(text)), reader) => {
                    from_server = Some(next_message(reader));
                    let heard = match decoder.decode(&text) {
                        Ok(Some(heard)) => heard,
                        Ok(None) => continue,
                        Err(failure) => return lost(failure),
                    };
                    let became_ready = heard.name == READY_EVENT && !ready;
                    let reply = Returned {
                        name: heard.name,
                        data: heard.data,
                    };
                    if returned.write_one(event(&reply)).await.is_some() {
                        return None;
                    }
                    if became_ready {
                        ready = true;
                        held = 0;
                        while let Some(pcm) = unready.pop_front() {
                            if outgoing.write_one(Message::Binary(pcm)).await.is_some() {
                                return lost(Lost::Unavailable);
                            }
                        }
                    }
                }
                Woke::Server(Some(Message::Binary(_)), _) => {
                    return lost(Lost::InvalidResponse);
                }
                Woke::Server(None, _) => {
                    let code = match end.await {
                        Ok(Some(frame)) => Some(frame.code),
                        _ => None,
                    };
                    return lost(Lost::closed(code));
                }
            }
        }
    }

    /// One [`super::stream::AUDIO_EVENT`]'s sound as the session sends it,
    /// `None` when it cannot be read or is not 16-bit PCM.
    fn heard_audio(data: &serde_json::Value) -> Option<Vec<u8>> {
        let path = super::media_path(data.get("audio")?.as_str()?).ok()?;
        let audio = super::read_audio(&path).ok()?;
        let rate = data
            .get("sample_rate_hz")
            .and_then(serde_json::Value::as_u64)
            .and_then(|rate| u32::try_from(rate).ok());
        let channels = data
            .get("channels")
            .and_then(serde_json::Value::as_u64)
            .and_then(|channels| u16::try_from(channels).ok());
        super::stream::pcm(&audio, rate, channels).ok()
    }

    /// Bytes of one read of a streamed answer's body.
    const READ_BYTES: usize = 64 * 1024;

    /// Why an encoding stopped before its outcome.
    enum Stopped {
        /// It failed: its one event.
        Failed(super::FailureCode),
        /// The invoking session stopped reading.
        Gone,
    }

    /// Writes each chunk's file, then returns its event (decision D203).
    async fn emit(
        encoding: &super::Encoding,
        chunks: Vec<super::chunks::Chunk>,
        returned: &mut wit_bindgen::StreamWriter<Event>,
    ) -> Result<(), Stopped> {
        for chunk in chunks {
            let media = super::chunks::chunk_media(&encoding.output, chunk.sequence)
                .map_err(Stopped::Failed)?;
            super::write_audio(&media.path, &chunk.audio)
                .map_err(|_| Stopped::Failed(super::FailureCode::Unwritable))?;
            let reply = super::chunks::chunk_returned(encoding, &media, &chunk);
            if returned.write_one(event(&reply)).await.is_some() {
                return Err(Stopped::Gone);
            }
        }
        Ok(())
    }

    /// One stretch's request, its answer read as it arrives
    /// (`outbound-http.send`) and cut into chunks while it is a success;
    /// Gemini's, which answers whole, fetched and returned as one chunk.
    async fn stretch(
        encoding: &super::Encoding,
        built: Request,
        chunker: &mut super::chunks::Chunker,
        returned: &mut wit_bindgen::StreamWriter<Event>,
        held: &mut usize,
    ) -> Result<Result<Response, TransportFailure>, Stopped> {
        let mut count = |bytes: usize| {
            *held = held.saturating_add(bytes);
            if scxml_types::exceeds_wasm_bound(*held, super::MAX_ASSEMBLED_BYTES) {
                Err(Stopped::Failed(super::FailureCode::Malformed))
            } else {
                Ok(())
            }
        };
        if encoding.provider == super::Provider::Gemini {
            let answer = fetch(built).await;
            if let Ok(response) = &answer {
                count(response.body.len())?;
                if (200..300).contains(&response.status)
                    && let Ok(pcm) = super::gemini::audio(&response.body)
                {
                    emit(encoding, chunker.whole(pcm), returned).await?;
                }
            }
            return Ok(answer);
        }
        let (status, mut body, end) = match outbound_http::send(http(built)).await {
            Ok(opened) => opened,
            Err(failure) => return Ok(Err(transport_failure(&failure))),
        };
        let success = (200..300).contains(&status);
        let mut bytes = Vec::new();
        loop {
            let (result, read) = body.read(Vec::with_capacity(READ_BYTES)).await;
            if !read.is_empty() {
                count(read.len())?;
                if success {
                    let chunks = chunker.read(&read).map_err(Stopped::Failed)?;
                    emit(encoding, chunks, returned).await?;
                }
                bytes.extend_from_slice(&read);
            }
            if !matches!(result, wit_bindgen::StreamResult::Complete(_)) {
                break;
            }
        }
        drop(body);
        if let Err(failure) = end.await {
            return Ok(Err(transport_failure(&failure)));
        }
        if success {
            emit(encoding, chunker.end(), returned).await?;
        }
        Ok(Ok(Response {
            status,
            body: bytes,
        }))
    }

    /// Runs an encoding: one stretch at a time, in order, until one fails
    /// or the answers together pass what may be assembled, returning each
    /// chunk as it is cut, then its outcome.
    async fn encode(
        operation: &Operation,
        encoding: &super::Encoding,
        base_url: u32,
        api_key: Option<u32>,
        returned: &mut wit_bindgen::StreamWriter<Event>,
    ) -> Option<super::Finished> {
        use super::Finished;
        let mut chunker = super::chunks::Chunker::new(encoding);
        let mut answered = Vec::with_capacity(encoding.stretches.len());
        let mut held = 0_usize;
        let mut settled = true;
        for each in &encoding.stretches {
            let streamed = async {
                emit(encoding, chunker.open(each.pause), returned).await?;
                if each.words.trim().is_empty() {
                    return Ok(None);
                }
                let built = super::encoding_request(base_url, api_key, encoding, &each.words);
                stretch(encoding, built, &mut chunker, returned, &mut held)
                    .await
                    .map(Some)
            }
            .await;
            let answer = match streamed {
                Ok(Some(answer)) => answer,
                Ok(None) => continue,
                Err(Stopped::Gone) => return None,
                Err(Stopped::Failed(code)) => {
                    return Some(Finished::event(Returned::failed(operation, code)));
                }
            };
            settled = matches!(&answer, Ok(response) if (200..300).contains(&response.status));
            answered.push(answer);
            if !settled {
                break;
            }
        }
        if settled {
            match emit(encoding, chunker.close(), returned).await {
                Ok(()) => {}
                Err(Stopped::Gone) => return None,
                Err(Stopped::Failed(code)) => {
                    return Some(Finished::event(Returned::failed(operation, code)));
                }
            }
        }
        Some(Finished::event(super::encoding_outcome(
            operation,
            answered,
            super::write_audio,
        )))
    }

    /// Runs a drafted encoding ([`super::drafting`]): reads the drafts and
    /// then the whole reply from `events`, and says each part as soon as it
    /// is ready, one request at a time, returning each chunk as it is cut;
    /// then its outcome, the requests' answers joined. `None` when the
    /// invoking session stopped reading, or stopped sending before the
    /// whole reply.
    async fn encode_drafted(
        operation: &Operation,
        encoding: &super::Encoding,
        base_url: u32,
        api_key: Option<u32>,
        mut events: wit_bindgen::StreamReader<Event>,
        returned: &mut wit_bindgen::StreamWriter<Event>,
    ) -> Option<super::Finished> {
        use super::Finished;
        use super::drafting::{DRAFT_EVENT, Drafting, FINAL_EVENT, READY_EVENT};
        let ready = Returned {
            name: READY_EVENT,
            data: "{}".into(),
        };
        if returned.write_one(event(&ready)).await.is_some() {
            return None;
        }
        let mut chunker = super::chunks::Chunker::new(encoding);
        let mut drafting = Drafting::default();
        let mut said = Vec::new();
        let mut answered = Vec::new();
        let mut held = 0_usize;
        let mut settled = true;
        'saying: loop {
            while let Some(each) = drafting.next_request() {
                let streamed = async {
                    emit(encoding, chunker.open(each.pause), returned).await?;
                    if each.words.trim().is_empty() {
                        return Ok(None);
                    }
                    let built = super::encoding_request(base_url, api_key, encoding, &each.words);
                    stretch(encoding, built, &mut chunker, returned, &mut held)
                        .await
                        .map(Some)
                }
                .await;
                let answer = match streamed {
                    Ok(Some(answer)) => answer,
                    Ok(None) => {
                        said.push(each);
                        continue;
                    }
                    Err(Stopped::Gone) => return None,
                    Err(Stopped::Failed(code)) => {
                        return Some(Finished::event(Returned::failed(operation, code)));
                    }
                };
                settled = matches!(&answer, Ok(response) if (200..300).contains(&response.status));
                said.push(each);
                answered.push(answer);
                if !settled {
                    break 'saying;
                }
            }
            if drafting.finished() {
                break;
            }
            let event = events.next().await?;
            let Ok(data) = native_value(event.data) else {
                continue;
            };
            let data = match data {
                scxml_types::DataValue::JsonObject(object) => {
                    serde_json::from_str::<serde_json::Value>(&object).unwrap_or_default()
                }
                _ => continue,
            };
            match event.name.as_str() {
                DRAFT_EVENT => drafting.draft(&data),
                FINAL_EVENT => {
                    let parts = data.get("parts").cloned().unwrap_or_default();
                    let checked = scxml_types::DataValue::canonical_json_array(
                        &serde_json::to_string(&parts).unwrap_or_default(),
                    )
                    .ok()
                    .and_then(|parts| super::stretches(&parts).ok());
                    match (checked, parts.as_array()) {
                        (Some(_), Some(parts)) => drafting.settle(parts),
                        _ => drafting.settle(&[]),
                    }
                }
                _ => {}
            }
        }
        drop(events);
        if settled {
            match emit(encoding, chunker.close(), returned).await {
                Ok(()) => {}
                Err(Stopped::Gone) => return None,
                Err(Stopped::Failed(code)) => {
                    return Some(Finished::event(Returned::failed(operation, code)));
                }
            }
        }
        let said = Operation::Encode(super::Encoding {
            stretches: said,
            ..encoding.clone()
        });
        Some(Finished::event(super::encoding_outcome(
            &said,
            answered,
            super::write_audio,
        )))
    }

    /// What a transport failure is for the answer.
    fn transport_failure(failure: &ComponentError) -> TransportFailure {
        match failure.code {
            ErrorCode::PayloadTooLarge => TransportFailure::TooLarge,
            _ => TransportFailure::Unavailable,
        }
    }

    /// Runs a voice classifier operation to its one event.
    async fn run_voice(
        operation: &Operation,
        base_url: u32,
        api_key: Option<u32>,
    ) -> super::Finished {
        use super::{Finished, voice};
        let failed = |code| Finished::event(Returned::failed(operation, code));
        Finished::event(match operation {
            Operation::Detect(detection) => {
                let samples = super::read_audio(&detection.audio.path)
                    .and_then(|audio| voice::detection_samples(detection, audio));
                let (samples, seconds) = match samples {
                    Ok(samples) => samples,
                    Err(code) => return failed(code),
                };
                let built = voice::detection_request(base_url, api_key, &detection.model, &samples);
                drop(samples);
                if scxml_types::exceeds_wasm_bound(built.body.len(), super::MAX_AUDIO_BYTES) {
                    return failed(super::FailureCode::TooLarge);
                }
                let answered = fetch(built).await;
                voice::detection_outcome(operation.direction(), answered, seconds)
            }
            Operation::Diarize(diarization) => {
                let utterance = super::read_audio(&diarization.audio.path)
                    .and_then(|audio| voice::utterance(diarization, audio));
                let (rate, channels, pcm) = match utterance {
                    Ok(utterance) => utterance,
                    Err(code) => return failed(code),
                };
                let built = voice::diarization_request(
                    base_url,
                    api_key,
                    &diarization.model,
                    &voice::diarization_file_name(diarization),
                    &voice::wav_container(&pcm, rate, channels),
                );
                let answered = fetch(built).await;
                let mut write = super::write_audio;
                voice::diarization_outcome(
                    operation.direction(),
                    answered,
                    (&pcm, rate, channels),
                    &diarization.output,
                    &mut write,
                )
            }
            Operation::Identify(identification) => {
                let mut embeddings = Vec::with_capacity(identification.segments.len());
                for segment in &identification.segments {
                    let pcm = match super::read_audio(&segment.audio.path) {
                        Ok(pcm) => pcm,
                        Err(code) => return failed(code),
                    };
                    let wav = voice::wav_container(&pcm, segment.sample_rate_hz, segment.channels);
                    let built =
                        voice::embedding_request(base_url, api_key, &identification.model, &wav);
                    if scxml_types::exceeds_wasm_bound(built.body.len(), super::MAX_AUDIO_BYTES) {
                        return failed(super::FailureCode::TooLarge);
                    }
                    match voice::embedding_outcome(fetch(built).await, &identification.model) {
                        Ok(embedding) => embeddings.push(embedding),
                        Err(code) => return failed(code),
                    }
                }
                voice::identification_returned(&embeddings)
            }
            Operation::Classify(classification) => {
                let segment = super::read_audio(&classification.audio.path)
                    .and_then(|audio| voice::affect_pcm(classification, audio));
                let (rate, pcm) = match segment {
                    Ok(segment) => segment,
                    Err(code) => return failed(code),
                };
                let built = voice::affect_request(base_url, api_key, rate, pcm);
                if scxml_types::exceeds_wasm_bound(built.body.len(), super::MAX_AUDIO_BYTES) {
                    return failed(super::FailureCode::TooLarge);
                }
                voice::affect_outcome(operation.direction(), fetch(built).await)
            }
            _ => return failed(super::FailureCode::Malformed),
        })
    }

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
            // Only a drafted encoding and a stream read events; any other
            // drops its input stream.
            let (operation, endpoint) = observed(Op::Start, || plan(request))?;
            let events = match &operation {
                Operation::Encode(encoding) if encoding.drafted => Some(events),
                Operation::Stream(_) => Some(events),
                _ => None,
            };
            let (mut returned, returned_reader) = wit_stream::new::<Event>();
            let (done, done_reader) = wit_future::new::<Done>(|| {
                Err(error(
                    ErrorCode::Platform,
                    "the service ended without a result",
                ))
            });
            wit_bindgen::spawn(async move {
                let Some(finished) = run(operation, endpoint, events, &mut returned).await else {
                    // The invoking session stopped reading.
                    return;
                };
                telemetry::event(
                    Op::Send,
                    if finished.event.as_ref().is_some_and(|reply| {
                        reply.name == super::FAILED_EVENT
                            || reply.name == super::stream::FAILED_EVENT
                    }) {
                        SignalEvent::InputRejected
                    } else {
                        SignalEvent::TransformationCompleted
                    },
                    None,
                );
                // The one event, then the result: `done.invoke.<id>` follows
                // it, carrying a media step's done data.
                if let Some(reply) = &finished.event
                    && returned.write_one(event(reply)).await.is_some()
                {
                    return;
                }
                drop(returned);
                let _ = done
                    .write(Ok(finished.done.map(DataValue::JsonObject)))
                    .await;
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
