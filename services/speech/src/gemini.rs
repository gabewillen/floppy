//! The Gemini API's speech (the original's `gemini` provider's
//! `SpeechEncoder` and `SpeechDecoder`): both directions are one
//! Interactions API call, `POST {base}/interactions` with
//! `x-goog-api-key: <apikey>`, where `{base}` is the API's version root
//! (`https://generativelanguage.googleapis.com/v1beta`).
//!
//! * encode: the canonical JSON body `{"generation_config":
//!   {"speech_config": [{"voice": V}]}, "input": W, "model": M,
//!   "response_format": {"type": "audio"}, "store": false}`, `W` one
//!   stretch's words (one request per stretch); the answer's audio is the
//!   first `steps[].content[]` part of `type` `audio`, base64 linear PCM
//!   (16-bit mono at [`crate::PCM_SAMPLE_RATE_HZ`], the original's
//!   `_DEFAULT_SAMPLE_RATE_HZ`), written raw for `audio/pcm` or in a WAV
//!   container for `audio/wav` ([`encoding_request`], [`audio`]).
//! * decode: the body `{"input": [{"text": P, "type": "text"}, {"data": A,
//!   "mime_type": M, "type": "audio"}], "model": M, "store": false}`, `P`
//!   the original's prompt ([`DECODING_PROMPT`]) and `A` the audio in
//!   base64; the answer's text is its `output_text`, else the text parts of
//!   its `model_output` steps, joined by newlines (empty when they hold none) ([`decoding_request`],
//!   [`transcript`]).

use scxml_event_schema::canonical_json;
use serde_json::{Value as Json, json};

use crate::{Decoding, Encoding, FailureCode, PCM_SAMPLE_RATE_HZ, Part, Request, keyed, voice};

/// The path appended to the base URL.
pub const INTERACTIONS_PATH: &str = "/interactions";

/// What a decoding asks the model (the original's `_DEFAULT_PROMPT`).
pub const DECODING_PROMPT: &str =
    "Generate a transcript of the speech. Return only the spoken words as plain text.";

fn request(base_url: u32, api_key: Option<u32>, body: &Json) -> Request {
    Request {
        url: vec![
            Part::Reference(base_url),
            Part::Literal(INTERACTIONS_PATH.into()),
        ],
        headers: keyed(
            "x-goog-api-key",
            api_key,
            [(
                "content-type".into(),
                vec![Part::Literal("application/json".into())],
            )],
        ),
        body: canonical_json(body).into_bytes(),
    }
}

/// The speech request of one stretch's `words`.
#[must_use]
pub fn encoding_request(
    base_url: u32,
    api_key: Option<u32>,
    encoding: &Encoding,
    words: &str,
) -> Request {
    request(
        base_url,
        api_key,
        &json!({
            "generation_config": {"speech_config": [{"voice": encoding.voice}]},
            "input": words,
            "model": encoding.model,
            "response_format": {"type": "audio"},
            "store": false,
        }),
    )
}

/// The transcription request of a decoding's audio.
#[must_use]
pub fn decoding_request(
    base_url: u32,
    api_key: Option<u32>,
    decoding: &Decoding,
    audio: &[u8],
) -> Request {
    request(
        base_url,
        api_key,
        &json!({
            "input": [
                {"text": DECODING_PROMPT, "type": "text"},
                {"data": voice::base64(audio), "mime_type": decoding.media_type, "type": "audio"},
            ],
            "model": decoding.model,
            "store": false,
        }),
    )
}

/// The parts of the answer's steps, in order.
fn parts(answer: &Json) -> impl Iterator<Item = &Json> {
    answer
        .get("steps")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|step| step.get("content").and_then(Json::as_array))
        .flatten()
}

/// The PCM of an encoding's answer.
///
/// # Errors
///
/// [`FailureCode::Malformed`] when the answer is not JSON, holds no audio
/// part, its data is not base64 or is empty, or it names a rate other than
/// [`PCM_SAMPLE_RATE_HZ`] or more than one channel (raw PCM has no header
/// to carry either, so the original refuses a rate it was not configured
/// for).
pub fn audio(body: &[u8]) -> Result<Vec<u8>, FailureCode> {
    let answer: Json = serde_json::from_slice(body).map_err(|_| FailureCode::Malformed)?;
    let part = parts(&answer)
        .find(|part| part.get("type").and_then(Json::as_str) == Some("audio"))
        .ok_or(FailureCode::Malformed)?;
    let rate = part.get("sample_rate").and_then(Json::as_u64);
    let channels = part.get("channels").and_then(Json::as_u64);
    if rate.is_some_and(|rate| rate != u64::from(PCM_SAMPLE_RATE_HZ))
        || channels.is_some_and(|channels| channels != 1)
    {
        return Err(FailureCode::Malformed);
    }
    part.get("data")
        .and_then(Json::as_str)
        .and_then(base64_decoded)
        .filter(|pcm| !pcm.is_empty())
        .ok_or(FailureCode::Malformed)
}

/// The text of a decoding's answer, trimmed.
///
/// # Errors
///
/// [`FailureCode::Malformed`] when the answer is not JSON or holds no
/// `output_text` and no `model_output` step; a model output with no text
/// is speech the model heard none of (empty, as an `OpenAI` transcription
/// of silence is), where the original raises.
pub fn transcript(body: &[u8]) -> Result<String, FailureCode> {
    let answer: Json = serde_json::from_slice(body).map_err(|_| FailureCode::Malformed)?;
    if let Some(text) = answer.get("output_text").and_then(Json::as_str)
        && !text.trim().is_empty()
    {
        return Ok(text.trim().to_owned());
    }
    let outputs: Vec<&Json> = answer
        .get("steps")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter(|step| step.get("type").and_then(Json::as_str) == Some("model_output"))
        .collect();
    if outputs.is_empty() {
        return Err(FailureCode::Malformed);
    }
    let chunks: Vec<&str> = outputs
        .into_iter()
        .filter_map(|step| step.get("content").and_then(Json::as_array))
        .flatten()
        .filter_map(|part| part.get("text").and_then(Json::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect();
    Ok(chunks.join("\n"))
}

/// Standard base64 (with or without padding) decoded, or `None` when
/// `text` holds anything else.
#[must_use]
pub fn base64_decoded(text: &str) -> Option<Vec<u8>> {
    let digits = text.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(digits.len() / 4 * 3 + 2);
    let mut buffer = 0_u32;
    let mut bits = 0_u32;
    for &byte in digits {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buffer >> bits) & 0xff).ok()?);
        }
    }
    // A lone trailing digit carries fewer than 8 bits: not base64.
    (bits < 6).then_some(out)
}
