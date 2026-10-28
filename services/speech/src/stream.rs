//! A streaming hearing session: speech to words while the person talks
//! (mosfet.py's `hearing.speech.stream`, served as its `speech_server`
//! provider serves it).
//!
//! A `stream` invocation opens one WebSocket connection,
//! `{base}/audio/stream?sample_rate=16000&language=L&chunk=C&turns=1`
//! (`outbound-websocket`; the `baseurl` slot's `https` URL names its `wss`
//! form), with `authorization: Bearer <apikey>` when the run holds a key,
//! and keeps it for the whole invocation. Once the server says it is
//! ready, the service returns [`READY_EVENT`] `{}` and reads the events the
//! invoking session sends it:
//!
//! * [`AUDIO_EVENT`] `{audio, media_type?, sample_rate_hz?, channels?}`:
//!   a sound heard (by reference, as `environment.sound` carries it: a WAV
//!   file, or raw signed 16-bit PCM at the rate and channels given), sent
//!   on as one binary message of 16 kHz mono signed 16-bit little-endian
//!   PCM ([`pcm`]);
//! * [`PRODUCING_EVENT`] `{producing}`: whether the bot's own mouth is
//!   producing now, sent on as `{"type":"agent","speaking":B}` when it
//!   changes ([`Producing`]).
//!
//! Each frame the server sends becomes one returned event ([`Decoder`]):
//! `ready` is [`READY_EVENT`]; `speech_start` and `speech_end`
//! [`VOICE_STARTED_EVENT`] and [`VOICE_ENDED_EVENT`] `{at_seconds}`;
//! `partial` [`PARTIAL_EVENT`] `{text}`; `final` [`TRANSCRIPT_EVENT`]
//! `{text, confidence, start_seconds, end_seconds}` (its span from the word
//! times when given); `turn_end` [`TURN_ENDED_EVENT`] `{at_seconds,
//! probability, text}`; and `interrupt` [`OVERLAP_EVENT`] `{at_seconds,
//! voiced_seconds}`. `eot`, `resumed` and `rejoin` are outside the contract
//! and dropped. Every time is in seconds of the audio this session has sent.
//!
//! The connection's loss ends the invocation with [`FAILED_EVENT`]
//! `{kind}`: an `error` frame, a lost connection, or a server that cannot
//! be reached is `unavailable`, close code 1013 `overloaded` and 1003
//! `invalid_input`, and a frame outside the protocol `invalid_response`
//! (mosfet.py's failure kinds). When the invoking session stops sending
//! (its state is left, which cancels the invocation) the connection
//! closes.

use serde_json::{Map, Value as Json};

use crate::FailureCode;
use crate::voice::{VAD_SAMPLE_RATE_HZ, vad_samples, wav_16bit};

/// The streaming path appended to the base URL.
pub const STREAM_PATH: &str = "/audio/stream";
/// The sample rate the session sends at.
pub const RATE_HZ: u32 = VAD_SAMPLE_RATE_HZ;
/// The server's decoding chunks, in seconds.
pub const CHUNKS: [&str; 4] = ["0.08", "0.16", "0.56", "1.12"];
/// The decoding chunk without a `chunk_seconds` parameter (mosfet.py's).
pub const DEFAULT_CHUNK: &str = "0.56";
/// The language without a `language` parameter (mosfet.py's).
pub const DEFAULT_LANGUAGE: &str = "en";
/// How long another voice is sustained over the bot before the server
/// says so (mosfet.py's `INTERRUPT_SECONDS`), when the voice's start is
/// unknown.
pub const INTERRUPT_SECONDS: f64 = 0.8;

/// A sound heard, from the invoking session.
pub const AUDIO_EVENT: &str = "bot.ability.hearing.speech.stream.audio";
/// Whether the bot's own mouth is producing, from the invoking session.
pub const PRODUCING_EVENT: &str = "bot.ability.hearing.speech.stream.producing";
/// The session can take audio.
pub const READY_EVENT: &str = "bot.ability.hearing.speech.stream.ready";
/// The hearing model heard a voice start.
pub const VOICE_STARTED_EVENT: &str = "bot.ability.hearing.speech.stream.voice_started";
/// The hearing model heard a voice stop.
pub const VOICE_ENDED_EVENT: &str = "bot.ability.hearing.speech.stream.voice_ended";
/// The words of the utterance being heard, so far.
pub const PARTIAL_EVENT: &str = "bot.ability.hearing.speech.stream.partial";
/// One finalized utterance.
pub const TRANSCRIPT_EVENT: &str = "bot.ability.hearing.speech.stream.transcript";
/// The hearing model perceives that the speaker's turn has ended.
pub const TURN_ENDED_EVENT: &str = "bot.ability.hearing.speech.stream.turn_ended";
/// Another voice has been sustained while the bot was producing.
pub const OVERLAP_EVENT: &str = "bot.ability.hearing.speech.stream.overlap";
/// The session lost what serves it.
pub const FAILED_EVENT: &str = "bot.ability.hearing.speech.stream.failed";

/// The most bytes of the text of one returned event.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;

/// A validated `stream` invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Streaming {
    /// The language the server hears.
    pub language: String,
    /// The server's decoding chunk, one of [`CHUNKS`].
    pub chunk: &'static str,
}

/// Validates a `stream` invocation's `language` and `chunk_seconds`.
///
/// # Errors
///
/// A language that is not 1 to 16 ASCII letters or `-`, or a chunk that is
/// none of [`CHUNKS`].
pub fn streaming(
    language: Option<&str>,
    chunk_seconds: Option<f64>,
) -> Result<Streaming, &'static str> {
    let language = language.unwrap_or(DEFAULT_LANGUAGE);
    if language.is_empty()
        || language.len() > 16
        || !language
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic() || byte == b'-')
    {
        return Err("speech stream language must be 1 to 16 letters or hyphens");
    }
    let chunk = match chunk_seconds {
        None => DEFAULT_CHUNK,
        Some(seconds) => CHUNKS
            .iter()
            .copied()
            .find(|chunk| {
                chunk
                    .parse::<f64>()
                    .is_ok_and(|each| (each - seconds).abs() < 1e-9)
            })
            .ok_or("speech stream chunk_seconds must be 0.08, 0.16, 0.56, or 1.12")?,
    };
    Ok(Streaming {
        language: language.to_owned(),
        chunk,
    })
}

/// What follows the base URL: the path and its query.
#[must_use]
pub fn stream_path(streaming: &Streaming) -> String {
    format!(
        "{STREAM_PATH}?sample_rate={RATE_HZ}&language={}&chunk={}&turns=1",
        streaming.language, streaming.chunk
    )
}

/// One sound's audio as the session sends it: 16 kHz mono signed 16-bit
/// little-endian PCM. A WAV file declares its own format; any other audio
/// is raw signed 16-bit PCM at `sample_rate_hz` and `channels`.
///
/// # Errors
///
/// [`FailureCode::Format`] for a WAV file that is not 16-bit PCM, or raw
/// audio without its rate, or whose length is not whole frames.
pub fn pcm(
    audio: &[u8],
    sample_rate_hz: Option<u32>,
    channels: Option<u16>,
) -> Result<Vec<u8>, FailureCode> {
    let (rate, channels, samples) = if audio.starts_with(b"RIFF") {
        wav_16bit(audio)?
    } else {
        let (Some(rate), Some(channels)) = (sample_rate_hz, channels) else {
            return Err(FailureCode::Format);
        };
        if rate == 0 || channels == 0 || !audio.len().is_multiple_of(2 * usize::from(channels)) {
            return Err(FailureCode::Format);
        }
        (rate, channels, audio.to_vec())
    };
    Ok(vad_samples(&samples, rate, channels)
        .into_iter()
        .flat_map(|sample| {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "clamped to the i16 range first"
            )]
            let value = (sample * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16;
            value.to_le_bytes()
        })
        .collect())
}

/// The bot's producing state as last told to the server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Producing(Option<bool>);

impl Producing {
    /// The text message for one [`PRODUCING_EVENT`]'s data, `None` when it
    /// is malformed or does not change what the server was told.
    pub fn change(&mut self, data: &Json) -> Option<String> {
        let producing = data.get("producing").and_then(Json::as_bool)?;
        if self.0 == Some(producing) {
            return None;
        }
        self.0 = Some(producing);
        Some(format!("{{\"type\":\"agent\",\"speaking\":{producing}}}"))
    }
}

/// Why a session lost what serves it, in mosfet.py's failure kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lost {
    /// Unreachable, dropped, or an `error` frame.
    Unavailable,
    /// Full (close code 1013).
    Overloaded,
    /// Refused the stream's parameters (close code 1003).
    InvalidInput,
    /// Said something outside its protocol.
    InvalidResponse,
}

impl Lost {
    /// The failure's kind.
    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Overloaded => "overloaded",
            Self::InvalidInput => "invalid_input",
            Self::InvalidResponse => "invalid_response",
        }
    }

    /// How a connection that closed with `code` (none when it was lost) is
    /// lost.
    #[must_use]
    pub const fn closed(code: Option<u16>) -> Self {
        match code {
            Some(1013) => Self::Overloaded,
            Some(1003) => Self::InvalidInput,
            _ => Self::Unavailable,
        }
    }

    /// [`FAILED_EVENT`]'s data.
    #[must_use]
    pub fn data(self) -> String {
        format!("{{\"kind\":\"{}\"}}", self.kind())
    }
}

/// One event the session returns: its name and canonical JSON data.
#[derive(Clone, Debug, PartialEq)]
pub struct Heard {
    /// One of this module's returned event names.
    pub name: &'static str,
    /// Canonical JSON object text.
    pub data: String,
}

/// Server frames to returned events, remembering when the current voice
/// started (for overlaps).
#[derive(Clone, Debug, Default)]
pub struct Decoder {
    voice_started_at: Option<f64>,
}

fn time(frame: &Map<String, Json>, key: &str) -> Result<f64, Lost> {
    frame
        .get(key)
        .and_then(Json::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or(Lost::InvalidResponse)
}

fn probability(frame: &Map<String, Json>, key: &str) -> Result<Json, Lost> {
    match frame.get(key) {
        None | Some(Json::Null) => Ok(Json::Null),
        Some(value) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(|value| Json::from(value.clamp(0.0, 1.0)))
            .ok_or(Lost::InvalidResponse),
    }
}

fn text(frame: &Map<String, Json>, key: &str, required: bool) -> Result<Json, Lost> {
    match frame.get(key) {
        None | Some(Json::Null) if !required => Ok(Json::Null),
        Some(Json::String(text)) if text.len() <= MAX_TEXT_BYTES => Ok(Json::String(text.clone())),
        _ => Err(Lost::InvalidResponse),
    }
}

fn heard(name: &'static str, data: &Json) -> Heard {
    Heard {
        name,
        data: scxml_event_schema::canonical_json(data),
    }
}

impl Decoder {
    /// The event one text frame says, `None` for a frame outside the
    /// contract.
    ///
    /// # Errors
    ///
    /// [`Lost::InvalidResponse`] for a frame that is not a JSON object of
    /// the protocol, [`Lost::Unavailable`] for the server's `error`.
    pub fn decode(&mut self, message: &str) -> Result<Option<Heard>, Lost> {
        let Ok(Json::Object(frame)) = serde_json::from_str::<Json>(message) else {
            return Err(Lost::InvalidResponse);
        };
        let kind = frame
            .get("type")
            .and_then(Json::as_str)
            .ok_or(Lost::InvalidResponse)?;
        Ok(Some(match kind {
            "ready" => heard(READY_EVENT, &serde_json::json!({})),
            "speech_start" => {
                let at = time(&frame, "t")?;
                self.voice_started_at = Some(at);
                heard(VOICE_STARTED_EVENT, &serde_json::json!({"at_seconds": at}))
            }
            "speech_end" => heard(
                VOICE_ENDED_EVENT,
                &serde_json::json!({"at_seconds": time(&frame, "t")?}),
            ),
            "partial" => heard(
                PARTIAL_EVENT,
                &serde_json::json!({"text": text(&frame, "text", true)?}),
            ),
            "final" => heard(TRANSCRIPT_EVENT, &self.transcript(&frame)?),
            "turn_end" => heard(
                TURN_ENDED_EVENT,
                &serde_json::json!({
                    "at_seconds": time(&frame, "t")?,
                    "probability": probability(&frame, "p")?,
                    "text": text(&frame, "text", false)?,
                }),
            ),
            "interrupt" => {
                let at = time(&frame, "t")?;
                let voiced = self
                    .voice_started_at
                    .filter(|started| *started <= at)
                    .map_or(INTERRUPT_SECONDS, |started| at - started);
                heard(
                    OVERLAP_EVENT,
                    &serde_json::json!({"at_seconds": at, "voiced_seconds": voiced}),
                )
            }
            "eot" | "resumed" | "rejoin" => return Ok(None),
            "error" => return Err(Lost::Unavailable),
            _ => return Err(Lost::InvalidResponse),
        }))
    }

    fn transcript(&self, frame: &Map<String, Json>) -> Result<Json, Lost> {
        let words = text(frame, "text", true)?;
        let times: Vec<f64> = frame
            .get("words")
            .and_then(Json::as_array)
            .map(|words| {
                words
                    .iter()
                    .filter_map(|word| word.get("t").and_then(Json::as_f64))
                    .filter(|at| at.is_finite() && *at >= 0.0)
                    .collect()
            })
            .unwrap_or_default();
        let start = times
            .first()
            .copied()
            .unwrap_or_else(|| self.voice_started_at.unwrap_or(0.0));
        let end = times.last().copied().unwrap_or(start).max(start);
        Ok(serde_json::json!({
            "text": words,
            "confidence": probability(frame, "confidence")?,
            "start_seconds": start,
            "end_seconds": end,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_takes_mosfets_defaults_and_refuses_other_chunks() {
        let defaults = streaming(None, None).expect("defaults");
        assert_eq!(
            stream_path(&defaults),
            "/audio/stream?sample_rate=16000&language=en&chunk=0.56&turns=1"
        );
        assert_eq!(
            streaming(Some("de"), Some(0.08)).expect("valid").chunk,
            "0.08"
        );
        assert!(streaming(Some(""), None).is_err());
        assert!(streaming(Some("en&x=1"), None).is_err());
        assert!(streaming(None, Some(0.5)).is_err());
    }

    #[test]
    fn audio_is_sent_as_16_khz_mono_pcm() {
        // 48 kHz stereo, 30 frames: 10 at 16 kHz mono.
        let raw: Vec<u8> = (0..60_i16).flat_map(|_| 1000_i16.to_le_bytes()).collect();
        let sent = pcm(&raw, Some(48_000), Some(2)).expect("raw PCM");
        assert_eq!(sent.len(), 20);
        assert_eq!(i16::from_le_bytes([sent[0], sent[1]]), 1000);
        // Already 16 kHz mono: unchanged.
        let same: Vec<u8> = [5_i16, -7, 300]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        assert_eq!(pcm(&same, Some(16_000), Some(1)).expect("raw"), same);
        let wav = crate::voice::wav_container(&same, 16_000, 1);
        assert_eq!(pcm(&wav, None, None).expect("wav"), same);
        assert_eq!(pcm(&same, None, Some(1)), Err(FailureCode::Format));
        assert_eq!(
            pcm(&same[..3], Some(16_000), Some(1)),
            Err(FailureCode::Format)
        );
    }

    #[test]
    fn producing_is_told_on_change_only() {
        let mut producing = Producing::default();
        let on = serde_json::json!({"producing": true});
        assert_eq!(
            producing.change(&on).as_deref(),
            Some("{\"type\":\"agent\",\"speaking\":true}")
        );
        assert_eq!(producing.change(&on), None);
        assert_eq!(producing.change(&serde_json::json!({})), None);
        assert!(
            producing
                .change(&serde_json::json!({"producing": false}))
                .is_some()
        );
    }

    #[test]
    fn server_frames_map_to_the_stream_contract() {
        let mut decoder = Decoder::default();
        let decode =
            |decoder: &mut Decoder, frame: &str| decoder.decode(frame).expect("in the protocol");
        let ready = decode(&mut decoder, r#"{"type":"ready"}"#).expect("ready");
        assert_eq!((ready.name, ready.data.as_str()), (READY_EVENT, "{}"));
        let started = decode(&mut decoder, r#"{"type":"speech_start","t":1.5}"#).expect("start");
        assert_eq!(started.name, VOICE_STARTED_EVENT);
        assert_eq!(started.data, r#"{"at_seconds":1.5}"#);
        let partial =
            decode(&mut decoder, r#"{"type":"partial","text":"hel","t":1.7}"#).expect("partial");
        assert_eq!(
            (partial.name, partial.data.as_str()),
            (PARTIAL_EVENT, r#"{"text":"hel"}"#)
        );
        let final_words = decode(
            &mut decoder,
            r#"{"type":"final","text":"hello there","confidence":1.4,"words":[{"w":"hello","t":1.6},{"w":"there","t":2.1}]}"#,
        )
        .expect("final");
        assert_eq!(final_words.name, TRANSCRIPT_EVENT);
        assert_eq!(
            final_words.data,
            r#"{"confidence":1.0,"end_seconds":2.1,"start_seconds":1.6,"text":"hello there"}"#
        );
        let bare = decode(&mut decoder, r#"{"type":"final","text":"hi"}"#).expect("final");
        assert_eq!(
            bare.data,
            r#"{"confidence":null,"end_seconds":1.5,"start_seconds":1.5,"text":"hi"}"#
        );
        let turn = decode(
            &mut decoder,
            r#"{"type":"turn_end","t":2.6,"p":0.9,"reason":"eot","text":"hello there"}"#,
        )
        .expect("turn");
        assert_eq!(turn.name, TURN_ENDED_EVENT);
        assert_eq!(
            turn.data,
            r#"{"at_seconds":2.6,"probability":0.9,"text":"hello there"}"#
        );
        let overlap = decode(&mut decoder, r#"{"type":"interrupt","t":2.5}"#).expect("overlap");
        assert_eq!(overlap.data, r#"{"at_seconds":2.5,"voiced_seconds":1.0}"#);
        assert_eq!(decoder.decode(r#"{"type":"eot","p":0.4}"#), Ok(None));
        assert_eq!(
            decoder.decode(r#"{"type":"error","message":"x"}"#),
            Err(Lost::Unavailable)
        );
        for outside in [
            "[]",
            "nope",
            r#"{"type":"partial"}"#,
            r#"{"type":"speech_end","t":-1}"#,
            r#"{"type":"what"}"#,
        ] {
            assert_eq!(
                decoder.decode(outside),
                Err(Lost::InvalidResponse),
                "{outside}"
            );
        }
        assert_eq!(Lost::closed(Some(1013)).data(), r#"{"kind":"overloaded"}"#);
        assert_eq!(Lost::closed(Some(1003)), Lost::InvalidInput);
        assert_eq!(Lost::closed(None), Lost::Unavailable);
    }
}
