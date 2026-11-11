//! A GPT-Live session: speech to speech, full duplex (decision D285).
//!
//! A `live` invocation opens one WebSocket connection to `OpenAI`'s GPT-Live
//! (`gpt-live-1`), `{base}/live/sessions` (`outbound-websocket`; the
//! `baseurl` slot's `https` URL names its `wss` form, `https://api.openai.com/v1`
//! for `OpenAI`'s), with `authorization: Bearer <apikey>` when the run holds a
//! key, and keeps it for the whole invocation. Its first message is
//! `session.start` ([`start_message`]): the model, the instructions, 16 kHz
//! mono PCM both ways, the voice, and client delegation, so GPT-Live hands
//! every task to its backend, which is floppy's cognition.
//!
//! The session speaks the `stream` operation's hearing contract
//! ([`crate::stream`]), so the Transcription hears through it as it hears
//! through a streaming hearing session:
//!
//! * [`stream::AUDIO_EVENT`](crate::stream::AUDIO_EVENT) from the invoking
//!   session is sent on as `session.input_audio.append` ([`audio_message`]),
//!   its sound as 16 kHz mono signed 16-bit PCM ([`crate::stream::pcm`]),
//!   also written to its `output` when one is given; audio sent before the
//!   session has started is held (at most 2 s of it);
//! * `session.started` is [`crate::stream::READY_EVENT`];
//! * `session.input_transcript.delta` (the person's words as GPT-Live
//!   hears them) joins the turn being heard: its first opens it with
//!   [`crate::stream::VOICE_STARTED_EVENT`], and each returns the turn's
//!   words so far as [`crate::stream::PARTIAL_EVENT`];
//! * `session.delegation.created` (GPT-Live handing the turn to its
//!   backend; the event carries no text) ends a turn with words with
//!   [`crate::stream::VOICE_ENDED_EVENT`], [`crate::stream::TRANSCRIPT_EVENT`]
//!   and [`crate::stream::TURN_ENDED_EVENT`] `{text}`, the transcript
//!   deltas joined. Its id is the delegation the next answer answers.
//!
//! Times are seconds of the session's audio (the deltas' `start_ms` and
//! `end_ms`, the delegation's `offset_ms`).
//!
//! The mouth: [`ANSWER_EVENT`] `{words}` from the invoking session is sent
//! as `session.commentary.append` for the last delegation (none before the
//! first), which GPT-Live says in its own words ([`answer_messages`]), in
//! pieces of at most [`MAX_COMMENTARY_BYTES`]. With an `output` parameter,
//! GPT-Live's audio (`session.output_audio.delta`) is returned as
//! [`AUDIO_EVENT`] chunks, one per delta, written to the `output` reference
//! with `.<n>` before its extension ([`crate::chunks::chunk_media`]), and
//! its words (`session.output_transcript.delta`) as [`SAID_EVENT`] `{text}`;
//! without one both are dropped.
//!
//! An `error` event or `session.closed` ends the invocation with
//! [`crate::stream::FAILED_EVENT`] `{kind: "unavailable"}`, as a lost
//! connection does; a message that is not a JSON object with a `type` is
//! `invalid_response`. Every other event (acknowledgements, usage, the
//! resolved configuration) is outside the contract and dropped. When the
//! invoking session stops sending (its state is left, which cancels the
//! invocation), the service sends `session.close` and closes the
//! connection.

use serde_json::{Map, Value as Json};

use crate::Media;
use crate::stream::{
    Heard, Lost, PARTIAL_EVENT, READY_EVENT, TRANSCRIPT_EVENT, TURN_ENDED_EVENT, VOICE_ENDED_EVENT,
    VOICE_STARTED_EVENT,
};
use scxml_types::exceeds_wasm_bound;

/// The sessions path appended to the base URL.
pub const LIVE_PATH: &str = "/live/sessions";
/// The sample rate the session's audio is in, both ways.
pub const RATE_HZ: u32 = crate::stream::RATE_HZ;

/// What GPT-Live is to say for its backend, from the invoking session.
pub const ANSWER_EVENT: &str = "bot.ability.live.answer";
/// A chunk of what GPT-Live says.
pub const AUDIO_EVENT: &str = "bot.ability.live.audio";
/// The words GPT-Live says, as it says them.
pub const SAID_EVENT: &str = "bot.ability.live.said";

/// The most bytes of one `session.commentary.append`'s content: the API
/// takes up to 500 tokens, and 1500 bytes of English is well under them.
pub const MAX_COMMENTARY_BYTES: usize = 1500;
/// The most bytes of the words of one answer.
pub const MAX_ANSWER_BYTES: usize = 64 * 1024;
/// The most bytes of the instructions (the API takes up to 16,384 tokens).
pub const MAX_INSTRUCTIONS_BYTES: usize = 64 * 1024;
/// The most bytes of the words of one turn.
pub const MAX_TURN_BYTES: usize = crate::stream::MAX_TEXT_BYTES;
/// The most bytes of a voice name.
pub const MAX_VOICE_BYTES: usize = 128;

/// The instructions without an `instructions` parameter: GPT-Live voices
/// the conversation, and floppy decides what is said. `gpt-live-1`
/// delegates when it generates `<|SpawnThinking|>`, so the instructions
/// name that token (OpenAI's live prompting guide); asked only to
/// "delegate to the backend", it answers simple questions itself and
/// never delegates (the 2026-11-11 live run).
pub const DEFAULT_INSTRUCTIONS: &str = "You are the voice of Floppy, a conversational agent \
whose backend decides everything it says. Backend tools: the backend answers every turn the \
person takes. Generate <|SpawnThinking|> when: the person finishes saying anything at all, \
every turn, without exception. Do not answer from your own knowledge, and do not guess the \
result while waiting. When the backend's commentary arrives, say it in its words, briefly.";

/// A validated `live` invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Living {
    /// The GPT-Live model.
    pub model: String,
    /// The voice, the API's default without one.
    pub voice: Option<String>,
    /// The conversation's instructions.
    pub instructions: String,
    /// Where GPT-Live's audio is written, chunk by chunk; without it,
    /// what GPT-Live says is dropped.
    pub output: Option<Media>,
}

/// Validates a `live` invocation's `voice` and `instructions` (its
/// `model` and `output` are checked as every operation's are).
///
/// # Errors
///
/// A voice that is not `[A-Za-z0-9_.-]{1,128}`, or instructions that are
/// blank or over [`MAX_INSTRUCTIONS_BYTES`].
pub fn living(
    model: String,
    voice: Option<&str>,
    instructions: Option<&str>,
    output: Option<Media>,
) -> Result<Living, &'static str> {
    if let Some(voice) = voice
        && (voice.is_empty()
            || voice.len() > MAX_VOICE_BYTES
            || !voice
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte)))
    {
        return Err("speech live voice must be 1 to 128 letters, digits, _, ., or -");
    }
    let instructions = match instructions {
        None => DEFAULT_INSTRUCTIONS.to_owned(),
        Some(text) if text.trim().is_empty() => {
            return Err("speech live instructions must not be blank");
        }
        Some(text) if exceeds_wasm_bound(text.len(), MAX_INSTRUCTIONS_BYTES) => {
            return Err("speech live instructions exceed their bound");
        }
        Some(text) => text.to_owned(),
    };
    Ok(Living {
        model,
        voice: voice.map(str::to_owned),
        instructions,
        output,
    })
}

/// The first message: `session.start` with the model, the instructions,
/// 16 kHz PCM both ways, the voice when given, and client delegation.
#[must_use]
pub fn start_message(living: &Living) -> String {
    let mut audio = serde_json::json!({
        "format": {"type": "audio/pcm", "rate": RATE_HZ},
    });
    if let (Some(voice), Some(audio)) = (&living.voice, audio.as_object_mut()) {
        audio.insert("output".into(), serde_json::json!({"voice": voice}));
    }
    scxml_event_schema::canonical_json(&serde_json::json!({
        "type": "session.start",
        "session": {
            "model": living.model,
            "instructions": living.instructions,
            "audio": audio,
            "delegation": {"type": "client"},
        },
    }))
}

/// One sound's PCM as a `session.input_audio.append`.
#[must_use]
pub fn audio_message(pcm: &[u8]) -> String {
    scxml_event_schema::canonical_json(&serde_json::json!({
        "type": "session.input_audio.append",
        "audio": crate::voice::base64(pcm),
    }))
}

/// The message that ends the session.
pub const CLOSE_MESSAGE: &str = "{\"type\":\"session.close\"}";

/// The `session.commentary.append` messages one [`ANSWER_EVENT`]'s data
/// says, for `delegation` (`None` before the first): its words cut at
/// whitespace into pieces of at most [`MAX_COMMENTARY_BYTES`] (a word
/// longer than that is cut at a character boundary). Empty for data
/// without words, blank words, or words over [`MAX_ANSWER_BYTES`].
#[must_use]
pub fn answer_messages(data: &Json, delegation: Option<&str>) -> Vec<String> {
    let Some(words) = data.get("words").and_then(Json::as_str) else {
        return Vec::new();
    };
    if words.trim().is_empty() || words.len() > MAX_ANSWER_BYTES {
        return Vec::new();
    }
    pieces(words.trim(), MAX_COMMENTARY_BYTES)
        .into_iter()
        .map(|content| {
            scxml_event_schema::canonical_json(&serde_json::json!({
                "type": "session.commentary.append",
                "delegation_id": delegation,
                "content": content,
            }))
        })
        .collect()
}

/// `text` cut at whitespace into pieces of at most `bound` bytes.
fn pieces(text: &str, bound: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut piece = String::new();
    for word in text.split_whitespace() {
        let mut word = word;
        // A word longer than a piece is cut at character boundaries.
        while word.len() > bound {
            let mut cut = bound;
            while !word.is_char_boundary(cut) {
                cut -= 1;
            }
            if !piece.is_empty() {
                pieces.push(std::mem::take(&mut piece));
            }
            let (head, tail) = word.split_at(cut);
            pieces.push(head.to_owned());
            word = tail;
        }
        let needed = if piece.is_empty() {
            word.len()
        } else {
            piece.len() + 1 + word.len()
        };
        if needed > bound && !piece.is_empty() {
            pieces.push(std::mem::take(&mut piece));
        }
        if !piece.is_empty() {
            piece.push(' ');
        }
        piece.push_str(word);
    }
    if !piece.is_empty() {
        pieces.push(piece);
    }
    pieces
}

/// What one server message says.
#[derive(Clone, Debug, PartialEq)]
pub enum Said {
    /// An event of the hearing contract, or [`SAID_EVENT`].
    Event(Heard),
    /// A chunk of GPT-Live's audio: 16 kHz mono signed 16-bit PCM.
    Audio(Vec<u8>),
}

/// Server messages to returned events: the turn being heard and the
/// delegation the next answer answers.
#[derive(Clone, Debug, Default)]
pub struct Session {
    /// Whether GPT-Live's audio and words are returned.
    pub speaks: bool,
    words: String,
    started_ms: Option<u64>,
    ended_ms: u64,
    delegation: Option<String>,
}

fn milliseconds(frame: &Map<String, Json>, key: &str) -> Result<Option<u64>, Lost> {
    match frame.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or(Lost::InvalidResponse),
    }
}

fn delta(frame: &Map<String, Json>) -> Result<&str, Lost> {
    frame
        .get("delta")
        .and_then(Json::as_str)
        .ok_or(Lost::InvalidResponse)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "session milliseconds stay far below 2^52"
)]
fn seconds(ms: u64) -> f64 {
    ms as f64 / 1000.0
}

fn heard(name: &'static str, data: &Json) -> Said {
    Said::Event(Heard {
        name,
        data: scxml_event_schema::canonical_json(data),
    })
}

impl Session {
    /// A session that returns GPT-Live's audio and words when `speaks`.
    #[must_use]
    pub fn new(speaks: bool) -> Self {
        Self {
            speaks,
            ..Self::default()
        }
    }

    /// The delegation the next answer answers, `None` before the first.
    #[must_use]
    pub fn delegation(&self) -> Option<&str> {
        self.delegation.as_deref()
    }

    /// What one text message says, in order: nothing for an event outside
    /// the contract.
    ///
    /// # Errors
    ///
    /// [`Lost::InvalidResponse`] for a message that is not a JSON object
    /// with a `type`, or a contract event without its fields;
    /// [`Lost::Unavailable`] for `error` and `session.closed`.
    pub fn decode(&mut self, message: &str) -> Result<Vec<Said>, Lost> {
        let Ok(Json::Object(frame)) = serde_json::from_str::<Json>(message) else {
            return Err(Lost::InvalidResponse);
        };
        let kind = frame
            .get("type")
            .and_then(Json::as_str)
            .ok_or(Lost::InvalidResponse)?;
        Ok(match kind {
            "session.started" => vec![heard(READY_EVENT, &serde_json::json!({}))],
            "session.input_transcript.delta" => self.heard(&frame)?,
            "session.delegation.created" => self.delegated(&frame)?,
            "session.output_audio.delta" if self.speaks => {
                let audio = crate::gemini::base64_decoded(delta(&frame)?)
                    .filter(|audio| audio.len().is_multiple_of(2))
                    .ok_or(Lost::InvalidResponse)?;
                if audio.is_empty() {
                    Vec::new()
                } else {
                    vec![Said::Audio(audio)]
                }
            }
            "session.output_transcript.delta" if self.speaks => {
                let text = delta(&frame)?;
                if text.is_empty() || text.len() > MAX_TURN_BYTES {
                    Vec::new()
                } else {
                    vec![heard(SAID_EVENT, &serde_json::json!({"text": text}))]
                }
            }
            "error" | "session.closed" => return Err(Lost::Unavailable),
            _ => Vec::new(),
        })
    }

    fn heard(&mut self, frame: &Map<String, Json>) -> Result<Vec<Said>, Lost> {
        let text = delta(frame)?;
        let start = milliseconds(frame, "start_ms")?;
        let end = milliseconds(frame, "end_ms")?;
        let mut said = Vec::new();
        if text.is_empty() || self.words.len() + text.len() > MAX_TURN_BYTES {
            return Ok(said);
        }
        if self.started_ms.is_none() {
            let at = start.unwrap_or(self.ended_ms);
            self.started_ms = Some(at);
            self.ended_ms = self.ended_ms.max(at);
            said.push(heard(
                VOICE_STARTED_EVENT,
                &serde_json::json!({"at_seconds": seconds(at)}),
            ));
        }
        self.words.push_str(text);
        if let Some(end) = end.or(start) {
            self.ended_ms = self.ended_ms.max(end);
        }
        said.push(heard(
            PARTIAL_EVENT,
            &serde_json::json!({"text": self.words.trim()}),
        ));
        Ok(said)
    }

    fn delegated(&mut self, frame: &Map<String, Json>) -> Result<Vec<Said>, Lost> {
        let id = frame
            .get("delegation")
            .and_then(|delegation| delegation.get("id"))
            .and_then(Json::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 256)
            .ok_or(Lost::InvalidResponse)?;
        let offset = milliseconds(frame, "offset_ms")?;
        self.delegation = Some(id.to_owned());
        let words = std::mem::take(&mut self.words);
        let Some(started) = self.started_ms.take() else {
            return Ok(Vec::new());
        };
        let words = words.trim();
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let ended = self.ended_ms.max(started);
        let at = offset.map_or(ended, |offset| offset.max(ended));
        Ok(vec![
            heard(
                VOICE_ENDED_EVENT,
                &serde_json::json!({"at_seconds": seconds(ended)}),
            ),
            heard(
                TRANSCRIPT_EVENT,
                &serde_json::json!({
                    "text": words,
                    "confidence": null,
                    "start_seconds": seconds(started),
                    "end_seconds": seconds(ended),
                }),
            ),
            heard(
                TURN_ENDED_EVENT,
                &serde_json::json!({
                    "at_seconds": seconds(at),
                    "probability": null,
                    "text": words,
                }),
            ),
        ])
    }
}

/// [`AUDIO_EVENT`] of a chunk written at `media`: `{audio, sequence,
/// media_type, sample_rate_hz, channels, duration}`, as an encoding's
/// chunk names its own.
#[must_use]
pub fn audio_returned(media: &Media, sequence: u32, bytes: usize) -> crate::Returned {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a chunk's frames stay far below 2^52"
    )]
    let duration = (bytes / 2) as f64 / f64::from(RATE_HZ);
    crate::Returned {
        name: AUDIO_EVENT,
        data: scxml_event_schema::canonical_json(&serde_json::json!({
            "audio": media.reference,
            "sequence": sequence,
            "media_type": "audio/pcm",
            "sample_rate_hz": RATE_HZ,
            "channels": 1,
            "duration": duration,
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heard_event(said: &Said) -> (&'static str, &str) {
        match said {
            Said::Event(heard) => (heard.name, heard.data.as_str()),
            Said::Audio(_) => ("audio", ""),
        }
    }

    #[test]
    fn a_session_starts_with_client_delegation_at_16_khz() {
        let given = living("gpt-live-1".into(), Some("marin"), None, None).expect("valid");
        assert_eq!(
            start_message(&given),
            format!(
                r#"{{"session":{{"audio":{{"format":{{"rate":16000,"type":"audio/pcm"}},"output":{{"voice":"marin"}}}},"delegation":{{"type":"client"}},"instructions":"{DEFAULT_INSTRUCTIONS}","model":"gpt-live-1"}},"type":"session.start"}}"#
            )
        );
        // gpt-live-1 delegates only when it generates this token.
        assert!(DEFAULT_INSTRUCTIONS.contains("Generate <|SpawnThinking|> when:"));
        let bare = living("gpt-live-1".into(), None, Some("Be brief."), None).expect("valid");
        assert!(
            start_message(&bare)
                .contains(r#""audio":{"format":{"rate":16000,"type":"audio/pcm"}}"#)
        );
        assert!(start_message(&bare).contains(r#""instructions":"Be brief.""#));
        assert!(living("m".into(), Some("a b"), None, None).is_err());
        assert!(living("m".into(), Some(""), None, None).is_err());
        assert!(living("m".into(), None, Some("  "), None).is_err());
    }

    #[test]
    fn audio_is_appended_as_base64_pcm() {
        assert_eq!(
            audio_message(&[1, 2, 3]),
            r#"{"audio":"AQID","type":"session.input_audio.append"}"#
        );
    }

    #[test]
    fn an_answer_is_commentary_for_the_last_delegation() {
        let answer = serde_json::json!({"words": "  Paris is the capital.  "});
        assert_eq!(
            answer_messages(&answer, Some("item_1")),
            [
                r#"{"content":"Paris is the capital.","delegation_id":"item_1","type":"session.commentary.append"}"#
            ]
        );
        assert_eq!(
            answer_messages(&answer, None),
            [
                r#"{"content":"Paris is the capital.","delegation_id":null,"type":"session.commentary.append"}"#
            ]
        );
        assert!(answer_messages(&serde_json::json!({"words": " "}), None).is_empty());
        assert!(answer_messages(&serde_json::json!({}), None).is_empty());
        let long = "word ".repeat(700);
        let messages = answer_messages(&serde_json::json!({"words": long}), None);
        assert_eq!(messages.len(), 3);
        assert_eq!(pieces("abcdef gh", 4), ["abcd", "ef", "gh"]);
        assert_eq!(pieces("ab cd ef", 5), ["ab cd", "ef"]);
    }

    #[test]
    fn transcript_deltas_make_a_turn_that_a_delegation_ends() {
        let mut session = Session::new(false);
        let started = session
            .decode(r#"{"type":"session.started","session":{"id":"s"}}"#)
            .expect("started");
        assert_eq!(heard_event(&started[0]), (READY_EVENT, "{}"));
        let first = session
            .decode(r#"{"type":"session.input_transcript.delta","delta":"What is","start_ms":1000,"end_ms":1200}"#)
            .expect("delta");
        assert_eq!(
            heard_event(&first[0]),
            (VOICE_STARTED_EVENT, r#"{"at_seconds":1.0}"#)
        );
        assert_eq!(
            heard_event(&first[1]),
            (PARTIAL_EVENT, r#"{"text":"What is"}"#)
        );
        let second = session
            .decode(r#"{"type":"session.input_transcript.delta","delta":" the capital?","start_ms":1200,"end_ms":2500}"#)
            .expect("delta");
        assert_eq!(second.len(), 1);
        assert_eq!(
            heard_event(&second[0]),
            (PARTIAL_EVENT, r#"{"text":"What is the capital?"}"#)
        );
        let ended = session
            .decode(r#"{"type":"session.delegation.created","offset_ms":2700,"delegation":{"id":"item_9","type":"delegation","target":"client"}}"#)
            .expect("delegation");
        let ended: Vec<_> = ended.iter().map(heard_event).collect();
        assert_eq!(
            ended,
            [
                (VOICE_ENDED_EVENT, r#"{"at_seconds":2.5}"#),
                (
                    TRANSCRIPT_EVENT,
                    r#"{"confidence":null,"end_seconds":2.5,"start_seconds":1.0,"text":"What is the capital?"}"#
                ),
                (
                    TURN_ENDED_EVENT,
                    r#"{"at_seconds":2.7,"probability":null,"text":"What is the capital?"}"#
                ),
            ]
        );
        assert_eq!(session.delegation(), Some("item_9"));
        // A delegation with nothing heard since the last ends no turn.
        let quiet = session
            .decode(r#"{"type":"session.delegation.created","delegation":{"id":"item_10"}}"#)
            .expect("delegation");
        assert!(quiet.is_empty());
        assert_eq!(session.delegation(), Some("item_10"));
        // The next turn opens where the last ended when it names no start.
        let next = session
            .decode(r#"{"type":"session.input_transcript.delta","delta":"Hi"}"#)
            .expect("delta");
        assert_eq!(
            heard_event(&next[0]),
            (VOICE_STARTED_EVENT, r#"{"at_seconds":2.5}"#)
        );
    }

    #[test]
    fn what_gpt_live_says_is_returned_only_when_it_speaks() {
        let audio = r#"{"type":"session.output_audio.delta","delta":"AQIDBA=="}"#;
        let words = r#"{"type":"session.output_transcript.delta","delta":"Paris.","start_ms":0,"end_ms":10}"#;
        let mut silent = Session::new(false);
        assert!(silent.decode(audio).expect("dropped").is_empty());
        assert!(silent.decode(words).expect("dropped").is_empty());
        let mut speaking = Session::new(true);
        assert_eq!(
            speaking.decode(audio).expect("audio"),
            [Said::Audio(vec![1, 2, 3, 4])]
        );
        assert_eq!(
            heard_event(&speaking.decode(words).expect("said")[0]),
            (SAID_EVENT, r#"{"text":"Paris."}"#)
        );
        assert_eq!(
            speaking.decode(r#"{"type":"session.output_audio.delta","delta":"AQ=="}"#),
            Err(Lost::InvalidResponse),
            "an odd byte is not 16-bit PCM"
        );
        let media = Media {
            reference: "media/live.0.pcm".into(),
            path: "/media/live.0.pcm".into(),
        };
        assert_eq!(
            audio_returned(&media, 0, 3200).data,
            r#"{"audio":"media/live.0.pcm","channels":1,"duration":0.1,"media_type":"audio/pcm","sample_rate_hz":16000,"sequence":0}"#
        );
    }

    #[test]
    fn errors_and_closes_lose_the_session_and_the_rest_is_dropped() {
        let mut session = Session::new(true);
        for dropped in [
            r#"{"type":"session.commentary.appended","client_event_id":"x"}"#,
            r#"{"type":"session.usage.updated","usage":{"seconds":12}}"#,
            r#"{"type":"response.event","event":{"type":"response.output_item.done"}}"#,
        ] {
            assert!(
                session.decode(dropped).expect("outside").is_empty(),
                "{dropped}"
            );
        }
        assert_eq!(
            session.decode(r#"{"type":"error","error":{"code":"x"}}"#),
            Err(Lost::Unavailable)
        );
        assert_eq!(
            session.decode(r#"{"type":"session.closed","reason":"expired"}"#),
            Err(Lost::Unavailable)
        );
        for outside in [
            "[]",
            "nope",
            r#"{"delta":"x"}"#,
            r#"{"type":"session.input_transcript.delta"}"#,
            r#"{"type":"session.input_transcript.delta","delta":"x","start_ms":-1}"#,
            r#"{"type":"session.delegation.created","delegation":{}}"#,
        ] {
            assert_eq!(
                session.decode(outside),
                Err(Lost::InvalidResponse),
                "{outside}"
            );
        }
    }
}
