//! A streamed encoding's chunks (decision D203): the audio of each answer
//! cut into chunks as it arrives, so its first sound can play before the
//! answer has ended.
//!
//! An encoding's answers are read as they arrive (`outbound-http.send`),
//! one stretch at a time, and each [`Chunker::read`] of an answer's bytes
//! yields the chunks that are complete: [`CHUNK_BYTES`] at a time after
//! shorter first ones ([`Chunker::cut`], decision D280), cut at fixed
//! offsets of the answer (never where a read happened to end, so the
//! same answer is always the same chunks), and the rest of the stretch
//! when its answer ends ([`Chunker::end`]). How an answer is cut depends on
//! its format:
//!
//! * raw `pcm` (the `OpenAI`-compatible API, `ElevenLabs`' `pcm_24000`):
//!   whole frames of 24 kHz mono 16-bit PCM, each chunk raw PCM;
//! * `wav`: the answer's header first, then whole frames of its 16-bit PCM,
//!   each chunk in a WAV container of its own declaring the answer's rate
//!   and channels; an answer that is not 16-bit integer PCM is one chunk,
//!   as it is, when it ends;
//! * every other format (`mp3`, `opus`, `aac`, `flac`): the answer's bytes
//!   cut at fixed offsets, the chunks joined being the answer;
//! * Gemini, which answers each stretch whole: one chunk per stretch
//!   ([`Chunker::whole`]), its PCM raw or in a WAV container.
//!
//! A pause is a chunk of its silence ([`crate::Pause::frames`]) before the
//! stretch it opens, once the stretch's rate and channels are known (for a
//! WAV answer, from its header). The chunks are numbered from 0 across the
//! whole encoding, and chunk `n`'s file is the `output` reference with
//! `.<n>` before its last segment's extension ([`chunk_media`]).

use scxml_event_schema::canonical_json;

use crate::{
    Encoding, FailureCode, Media, PCM_SAMPLE_RATE_HZ, Pause, Provider, Returned, media_path, sound,
    voice,
};

/// The streamed encoding's chunk event.
pub const CHUNK_EVENT: &str = "bot.ability.vocal.speech.encoding.chunk";

/// The bytes of a chunk cut from a streamed answer: half a second of
/// 24 kHz mono 16-bit PCM, half of speaking's lookahead (decision D179);
/// a PCM chunk is the whole frames under it.
pub const CHUNK_BYTES: usize = 24_000;

/// The most bytes a WAV answer's header may take before its `data` chunk
/// starts; past it the answer is not cut.
pub const MAX_WAV_HEAD_BYTES: usize = 64 * 1024;

/// One chunk: its number and its file's bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Chunk {
    /// Its number, from 0, across the encoding.
    pub sequence: u32,
    /// The bytes its file holds.
    pub audio: Vec<u8>,
}

/// How an encoding's answers are cut.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Cut {
    /// Raw 24 kHz mono 16-bit PCM.
    Pcm,
    /// A WAV stream: its header, then 16-bit PCM.
    Wav,
    /// Opaque bytes, cut at fixed offsets.
    Opaque,
    /// One chunk per stretch (Gemini).
    Whole,
}

/// Where the WAV answer in hand is.
#[derive(Clone, Debug, Eq, PartialEq)]
enum WavAnswer {
    /// Its header is still arriving.
    Head(Vec<u8>),
    /// Its PCM is arriving: the bytes still to come (`None` when its
    /// header declares no size, as a streamed WAV does).
    Data(Option<usize>),
    /// It is not 16-bit integer PCM: one chunk, as it is, when it ends.
    Unsupported(Vec<u8>),
}

/// What a WAV answer's first bytes say.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WavHead {
    /// More bytes are needed.
    Incomplete,
    /// Its `data` chunk starts at `offset`, `size` bytes when declared.
    Data {
        rate: u32,
        channels: u16,
        offset: usize,
        size: Option<usize>,
    },
    /// It is not a WAV of 16-bit integer PCM.
    Unsupported,
}

/// The `data` chunk of a WAV answer's first bytes.
fn wav_head(audio: &[u8]) -> WavHead {
    let Some(riff) = audio.get(..12) else {
        let prefix = b"RIFF";
        return if audio.iter().zip(prefix).all(|(byte, want)| byte == want) {
            WavHead::Incomplete
        } else {
            WavHead::Unsupported
        };
    };
    if riff.get(..4) != Some(b"RIFF".as_slice()) || riff.get(8..12) != Some(b"WAVE".as_slice()) {
        return WavHead::Unsupported;
    }
    let mut format = None;
    let mut offset = 12_usize;
    loop {
        let (Some(id), Some(size)) = (
            audio.get(offset..offset + 4),
            audio
                .get(offset + 4..offset + 8)
                .and_then(|size| <[u8; 4]>::try_from(size).ok())
                .map(u32::from_le_bytes),
        ) else {
            return WavHead::Incomplete;
        };
        let body = offset + 8;
        let Ok(length) = usize::try_from(size) else {
            return WavHead::Unsupported;
        };
        if id == b"fmt " {
            let Some(fmt) = audio.get(body..body + 16) else {
                return WavHead::Incomplete;
            };
            let field = |at: usize| {
                fmt.get(at..at + 2)
                    .and_then(|bytes| <[u8; 2]>::try_from(bytes).ok())
                    .map_or(0, u16::from_le_bytes)
            };
            let rate = fmt
                .get(4..8)
                .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
                .map_or(0, u32::from_le_bytes);
            format = Some((field(0), field(2), rate, field(14)));
        } else if id == b"data" {
            return match format {
                Some((1, channels, rate, 16))
                    if channels > 0
                        && channels <= voice::MAX_CHANNELS
                        && rate > 0
                        && rate <= voice::MAX_SAMPLE_RATE_HZ =>
                {
                    WavHead::Data {
                        rate,
                        channels,
                        offset: body,
                        // A streamed WAV declares 0 or the largest size.
                        size: (size != 0 && size != u32::MAX).then_some(length),
                    }
                }
                _ => WavHead::Unsupported,
            };
        }
        let Some(next) = body
            .checked_add(length)
            .and_then(|next| next.checked_add(length % 2))
        else {
            return WavHead::Unsupported;
        };
        if next > MAX_WAV_HEAD_BYTES {
            return WavHead::Unsupported;
        }
        offset = next;
    }
}

/// Cuts an encoding's answers into chunks (see the module documentation).
#[derive(Clone, Debug)]
pub struct Chunker {
    cut: Cut,
    /// Whether PCM chunks are written in a WAV container.
    wav: bool,
    /// The rate and channels of the PCM, once known.
    shape: Option<(u32, u16)>,
    /// The pauses whose silence waits for the shape.
    pauses: Vec<Pause>,
    /// The WAV answer in hand.
    answer: WavAnswer,
    /// The answer's bytes not yet in a chunk.
    pending: Vec<u8>,
    /// The next chunk's number.
    next: u32,
}

impl Chunker {
    /// The chunker of `encoding`'s answers.
    #[must_use]
    pub fn new(encoding: &Encoding) -> Self {
        let wav = encoding.format.response_format == "wav";
        let cut = match (encoding.provider, encoding.format.response_format) {
            (Provider::Gemini, _) => Cut::Whole,
            (_, "pcm") => Cut::Pcm,
            (_, "wav") => Cut::Wav,
            _ => Cut::Opaque,
        };
        Self {
            cut,
            wav,
            shape: matches!(cut, Cut::Pcm | Cut::Whole).then_some((PCM_SAMPLE_RATE_HZ, 1)),
            pauses: Vec::new(),
            answer: WavAnswer::Head(Vec::new()),
            pending: Vec::new(),
            next: 0,
        }
    }

    fn chunk(&mut self, audio: Vec<u8>) -> Chunk {
        let sequence = self.next;
        self.next = self.next.saturating_add(1);
        Chunk { sequence, audio }
    }

    /// A chunk of `pcm` at the known shape: raw, or in a WAV container.
    fn pcm_chunk(&mut self, pcm: Vec<u8>) -> Chunk {
        let audio = match (self.wav, self.shape) {
            (true, Some((rate, channels))) => voice::wav_container(&pcm, rate, channels),
            _ => pcm,
        };
        self.chunk(audio)
    }

    /// The silences waiting for the shape, once it is known.
    fn silences(&mut self, chunks: &mut Vec<Chunk>) {
        let Some((rate, channels)) = self.shape else {
            return;
        };
        let frame = 2 * usize::from(channels);
        for pause in std::mem::take(&mut self.pauses) {
            let bytes = pause.frames(rate).saturating_mul(frame);
            let chunk = self.pcm_chunk(vec![0; bytes]);
            chunks.push(chunk);
        }
    }

    /// The whole chunks of the pending bytes: [`CHUNK_BYTES`], whole frames
    /// of PCM, but the encoding's first chunk a fifth of that (0.1 s of
    /// 24 kHz PCM) and each after it half again as long as the one before,
    /// down to a whole twentieth of a second, up to [`CHUNK_BYTES`]
    /// (0.1, 0.15, 0.2, 0.3, 0.45, then 0.5 s; decision D280): the first sound plays without
    /// waiting for half a second of audio, and while a chunk plays, the
    /// next arrives from any encoder at least one and a half times real
    /// time.
    fn cut(&mut self, chunks: &mut Vec<Chunk>) {
        let frame = match (self.cut, self.shape) {
            (Cut::Opaque, _) => 1,
            (_, Some((_, channels))) => 2 * usize::from(channels),
            (_, None) => return,
        };
        loop {
            let step = CHUNK_BYTES / 10;
            let mut bytes = CHUNK_BYTES / 5;
            for _ in 0..self.next {
                if bytes >= CHUNK_BYTES {
                    break;
                }
                bytes = (bytes * 3 / 2 / step * step).min(CHUNK_BYTES);
            }
            let size = bytes - bytes % frame;
            if self.pending.len() < size {
                break;
            }
            let rest = self.pending.split_off(size);
            let piece = std::mem::replace(&mut self.pending, rest);
            let chunk = if self.cut == Cut::Opaque {
                self.chunk(piece)
            } else {
                self.pcm_chunk(piece)
            };
            chunks.push(chunk);
        }
    }

    /// Opens a stretch, `pause` its opening pause: the chunks of the
    /// silences due before it, once the shape is known.
    pub fn open(&mut self, pause: Option<Pause>) -> Vec<Chunk> {
        self.pauses.extend(pause);
        self.answer = WavAnswer::Head(Vec::new());
        self.pending.clear();
        let mut chunks = Vec::new();
        self.silences(&mut chunks);
        chunks
    }

    /// The chunks `bytes`, the next bytes of the stretch's answer, complete.
    ///
    /// # Errors
    ///
    /// [`FailureCode::Malformed`] when a WAV answer declares a rate or
    /// channels other than an earlier stretch's.
    pub fn read(&mut self, bytes: &[u8]) -> Result<Vec<Chunk>, FailureCode> {
        let mut chunks = Vec::new();
        if self.cut != Cut::Wav {
            self.pending.extend_from_slice(bytes);
            self.cut(&mut chunks);
            return Ok(chunks);
        }
        let mut bytes = bytes;
        if let WavAnswer::Head(head) = &mut self.answer {
            head.extend_from_slice(bytes);
            match wav_head(head) {
                WavHead::Incomplete if head.len() <= MAX_WAV_HEAD_BYTES => return Ok(chunks),
                WavHead::Incomplete | WavHead::Unsupported => {
                    self.answer = WavAnswer::Unsupported(std::mem::take(head));
                    return Ok(chunks);
                }
                WavHead::Data {
                    rate,
                    channels,
                    offset,
                    size,
                } => {
                    if self
                        .shape
                        .replace((rate, channels))
                        .is_some_and(|known| known != (rate, channels))
                    {
                        return Err(FailureCode::Malformed);
                    }
                    let head = std::mem::take(head);
                    self.answer = WavAnswer::Data(size);
                    self.silences(&mut chunks);
                    let data = head.get(offset..).unwrap_or_default().to_vec();
                    self.take_data(&data);
                    self.cut(&mut chunks);
                    return Ok(chunks);
                }
            }
        }
        if let WavAnswer::Unsupported(whole) = &mut self.answer {
            whole.extend_from_slice(bytes);
            bytes = &[];
        }
        self.take_data(bytes);
        self.cut(&mut chunks);
        Ok(chunks)
    }

    /// Takes a WAV answer's PCM bytes, within its declared size.
    fn take_data(&mut self, bytes: &[u8]) {
        let WavAnswer::Data(left) = &mut self.answer else {
            return;
        };
        let taken = left.map_or(bytes.len(), |left| left.min(bytes.len()));
        if let Some(left) = left {
            *left -= taken;
        }
        self.pending
            .extend_from_slice(bytes.get(..taken).unwrap_or_default());
    }

    /// The stretch's last chunks once its answer has ended: the rest of
    /// its bytes (whole frames of PCM), or an answer that was not cut, as
    /// it is.
    pub fn end(&mut self) -> Vec<Chunk> {
        let mut chunks = Vec::new();
        let whole = match std::mem::replace(&mut self.answer, WavAnswer::Head(Vec::new())) {
            WavAnswer::Head(bytes) | WavAnswer::Unsupported(bytes) if self.cut == Cut::Wav => {
                Some(bytes)
            }
            _ => None,
        };
        if let Some(bytes) = whole {
            if !bytes.is_empty() {
                chunks.push(self.chunk(bytes));
            }
            return chunks;
        }
        let rest = std::mem::take(&mut self.pending);
        if self.cut == Cut::Opaque {
            if !rest.is_empty() {
                chunks.push(self.chunk(rest));
            }
            return chunks;
        }
        let frame = self
            .shape
            .map_or(2, |(_, channels)| 2 * usize::from(channels));
        let whole = rest.len() - rest.len() % frame;
        if whole > 0 {
            let mut rest = rest;
            rest.truncate(whole);
            chunks.push(self.pcm_chunk(rest));
        }
        chunks
    }

    /// A stretch Gemini answered whole: its PCM as one chunk.
    pub fn whole(&mut self, pcm: Vec<u8>) -> Vec<Chunk> {
        vec![self.pcm_chunk(pcm)]
    }

    /// The chunks of the silences after the last stretch.
    pub fn close(&mut self) -> Vec<Chunk> {
        let mut chunks = Vec::new();
        self.silences(&mut chunks);
        chunks
    }
}

/// Chunk `sequence`'s file: the `output` reference with `.<sequence>`
/// before its last segment's extension (`media/said.pcm` is chunked as
/// `media/said.0.pcm`, `media/said.1.pcm`, …), or after a last segment that
/// has none.
///
/// # Errors
///
/// [`FailureCode::Unwritable`] when the reference passes its bound.
pub fn chunk_media(output: &Media, sequence: u32) -> Result<Media, FailureCode> {
    let reference = &output.reference;
    let segment = reference.rfind('/').map_or(0, |slash| slash + 1);
    let reference = match reference
        .get(segment..)
        .and_then(|last| last.rfind('.'))
        .filter(|&dot| dot > 0)
    {
        Some(dot) => {
            let (stem, extension) = reference.split_at(segment + dot);
            format!("{stem}.{sequence}{extension}")
        }
        None => format!("{reference}.{sequence}"),
    };
    let path = media_path(&reference).map_err(|_| FailureCode::Unwritable)?;
    Ok(Media { reference, path })
}

/// [`CHUNK_EVENT`] of a chunk written at `media`: `{audio, sequence,
/// media_type, sample_rate_hz, channels, duration}`, the sound fields only
/// when known, as the output names them.
#[must_use]
pub fn chunk_returned(encoding: &Encoding, media: &Media, chunk: &Chunk) -> Returned {
    let sound = sound(encoding.provider, encoding.format, &chunk.audio);
    Returned {
        name: CHUNK_EVENT,
        data: canonical_json(&serde_json::json!({
            "audio": media.reference,
            "sequence": chunk.sequence,
            "media_type": encoding.format.media_type,
            "sample_rate_hz": sound.sample_rate_hz,
            "channels": sound.channels,
            "duration": sound.duration,
        })),
    }
}

#[cfg(test)]
mod tests;
