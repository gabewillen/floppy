//! A deterministic fake hearing backend on 127.0.0.1 for the listening
//! tests. It speaks what the speech service (`services/speech`) sends:
//! `LocalAI`'s `POST /v1/vad` (voice detection), `POST /v1/audio/diarization`
//! and `POST /v1/voice/embed` (voice identification), and the Gemini API's
//! `POST /v1/interactions` (speech decoding, floppy's default) or the
//! `OpenAI`-compatible `POST /v1/audio/transcriptions`.
//!
//! A test chooses what is heard by writing it into the audio: [`voiced`]
//! spells text as signed 16-bit samples, one run of [`SAMPLES_PER_CHAR`]
//! equal samples per character (its code times 64, the sign alternating so
//! that a doubled letter is two runs), and [`silence`] is zero samples. The
//! backend reads the audio back the same way: voice detection and
//! diarization answer each run of non-zero samples as a span (diarization
//! names every span `SPEAKER_00`), transcription answers the spelled text,
//! and the embedding of a segment is `[characters, sum of codes]` of its
//! text. A transcription whose text starts with `fail` answers 500, and a
//! transcription or embedding of text starting with `hang` is never
//! answered (the interpretation's stage timer fails a transcription, and its
//! identification deadline identifies nobody). Voice detection of text
//! starting with `slow` answers only once the next voice detection has been
//! asked, or after 10 s, recording whether it was (`overlapped`). Every request is recorded; nothing reaches the network.
//!
//! It also serves the streaming hearing session, the WebSocket
//! `GET /v1/audio/stream` ([`stream`]), unless it was started with
//! [`Backend::start_without_stream`], when that path answers 404 as a
//! backend that does not stream does.

pub mod stream;

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Value as Json, json};

/// The fake key (not a secret): the mock checks where it arrives.
pub const FAKE_KEY: &str = "sk-floppy-hearing-fake";

/// Samples per spelled character (10 ms at 16 kHz).
pub const SAMPLES_PER_CHAR: usize = 160;

/// Voice detection's fixed rate (`LocalAI`'s VAD reads 16 kHz mono).
const VAD_RATE_HZ: f64 = 16_000.0;

/// One request: its `request_line`, whether it carried the fake key
/// (`keyed`: as the bearer, or Gemini's `x-goog-api-key`), and what it
/// asked: `heard` (the text read from its audio), `model`, and per
/// endpoint `form` (transcription and diarization: `model`,
/// `response_format`, `filename`, `content_type`), `prompt` and
/// `mime_type` (interactions), or `samples` (voice detection).
pub type Request = Json;

/// The running fake backend.
pub struct Backend {
    /// Its base URL, `http://127.0.0.1:<port>/v1`.
    pub base_url: String,
    received: Arc<Mutex<Vec<Request>>>,
}

/// The streaming path, under the base URL.
const STREAM_PATH: &str = "/v1/audio/stream";

/// `text` spelled as voiced samples.
pub fn voiced(text: &str) -> Vec<i16> {
    let mut samples = Vec::with_capacity(text.len() * SAMPLES_PER_CHAR);
    for (index, character) in text.chars().enumerate() {
        let code = i16::try_from(u32::from(character)).expect("an ASCII character") * 64;
        let sample = if index % 2 == 0 { code } else { -code };
        samples.extend(std::iter::repeat_n(sample, SAMPLES_PER_CHAR));
    }
    samples
}

/// `count` samples of silence.
pub fn silence(count: usize) -> Vec<i16> {
    vec![0; count]
}

/// Little-endian signed 16-bit PCM bytes.
pub fn pcm(samples: &[i16]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect()
}

/// A canonical 44-byte-header WAV container of `samples`.
pub fn wav(samples: &[i16], rate: u32, channels: u16) -> Vec<u8> {
    let data = pcm(samples);
    let length = u32::try_from(data.len()).expect("a small clip");
    let mut bytes = Vec::with_capacity(44 + data.len());
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + length).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
    bytes.extend_from_slice(&(channels * 2).to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(&data);
    bytes
}

/// The samples and rate of a WAV container, or of raw PCM (rate unknown).
fn samples_of(bytes: &[u8]) -> (Vec<i16>, Option<u32>) {
    let (data, rate) = if bytes.starts_with(b"RIFF") && bytes.len() >= 44 {
        let rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
        let mut at = 12;
        let mut data: &[u8] = &[];
        while at + 8 <= bytes.len() {
            let size =
                u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]])
                    as usize;
            if &bytes[at..at + 4] == b"data" {
                data = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
                break;
            }
            at += 8 + size + size % 2;
        }
        (data, Some(rate))
    } else {
        (bytes, None)
    };
    let samples = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair))
        .collect();
    (samples, rate)
}

/// The text spelled in `samples`.
fn spelled(samples: &[i16]) -> String {
    let mut text = String::new();
    let mut last = 0;
    for &sample in samples {
        if sample != 0 && sample != last {
            let code = u32::from(sample.unsigned_abs() / 64);
            text.extend(char::from_u32(code));
        }
        last = sample;
    }
    text
}

/// The runs of voice in `voiced` (one flag per sample) as `(start, end)`
/// sample indices.
fn runs(voiced: impl Iterator<Item = bool>) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut start = None;
    let mut count = 0;
    for (index, flag) in voiced.enumerate() {
        match (flag, start) {
            (true, None) => start = Some(index),
            (false, Some(from)) => {
                runs.push((from, index));
                start = None;
            }
            _ => {}
        }
        count = index + 1;
    }
    if let Some(from) = start {
        runs.push((from, count));
    }
    runs
}

#[expect(
    clippy::cast_precision_loss,
    reason = "test clips are far shorter than 2^52 samples"
)]
fn seconds(index: usize, rate: f64) -> f64 {
    index as f64 / rate
}

/// The bytes of standard base64.
fn unbase64(text: &str) -> Vec<u8> {
    let value = |byte: u8| match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut bytes = Vec::new();
    let mut buffer = 0_u32;
    let mut bits = 0;
    for sextet in text.bytes().filter_map(value) {
        buffer = (buffer << 6) | u32::from(sextet);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push(u8::try_from((buffer >> bits) & 0xff).expect("a byte"));
        }
    }
    bytes
}

/// One multipart part: its name, its file name and content type when it
/// is a file, and its bytes.
type Part = (String, Option<(String, String)>, Vec<u8>);

/// The parts of a multipart body: each part's name, its file name and
/// content type when it is a file, and its bytes.
fn parts(body: &[u8], boundary: &str) -> Vec<Part> {
    let delimiter = format!("--{boundary}");
    let delimiter = delimiter.as_bytes();
    let mut found = Vec::new();
    let mut positions = Vec::new();
    let mut at = 0;
    while let Some(offset) = body[at..]
        .windows(delimiter.len())
        .position(|window| window == delimiter)
    {
        positions.push(at + offset);
        at += offset + delimiter.len();
    }
    for pair in positions.windows(2) {
        let part = &body[pair[0] + delimiter.len()..pair[1]];
        let Some(split) = part.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&part[..split]).into_owned();
        let mut value = part[split + 4..].to_vec();
        if value.ends_with(b"\r\n") {
            value.truncate(value.len() - 2);
        }
        let quoted = |key: &str| {
            head.split(key)
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .map(str::to_owned)
        };
        let Some(name) = quoted(" name=\"") else {
            continue;
        };
        let file = quoted("filename=\"").map(|filename| {
            let content_type = head
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(key, _)| key.eq_ignore_ascii_case("content-type"))
                        .map(|(_, value)| value.trim().to_owned())
                })
                .unwrap_or_default();
            (filename, content_type)
        });
        found.push((name, file, value));
    }
    found
}

/// A multipart request's form fields, file description and file bytes.
fn form(body: &[u8], content_type: &str) -> (Json, Vec<u8>) {
    let boundary = content_type
        .split("boundary=")
        .nth(1)
        .unwrap_or_default()
        .trim_matches('"');
    let mut fields = serde_json::Map::new();
    let mut file = Vec::new();
    for (name, described, value) in parts(body, boundary) {
        if let Some((filename, content_type)) = described {
            fields.insert("filename".into(), json!(filename));
            fields.insert("content_type".into(), json!(content_type));
            file = value;
        } else {
            fields.insert(name, json!(String::from_utf8_lossy(&value)));
        }
    }
    (Json::Object(fields), file)
}

impl Backend {
    /// Starts the backend.
    pub fn start() -> Self {
        Self::serving(true)
    }

    /// Starts the backend without the streaming session: its path answers
    /// 404.
    pub fn start_without_stream() -> Self {
        Self::serving(false)
    }

    fn serving(streams: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock binds a loopback port");
        let base_url = format!(
            "http://{}/v1",
            listener.local_addr().expect("the mock has an address")
        );
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let log = Arc::clone(&log);
                std::thread::spawn(move || answer(stream, &log, streams));
            }
        });
        Self { base_url, received }
    }

    /// The requests received so far, in order.
    pub fn received(&self) -> Vec<Request> {
        self.received.lock().expect("the log lock").clone()
    }

    /// The requests to `path` (under `/v1`) received so far, in order.
    pub fn received_at(&self, path: &str) -> Vec<Request> {
        let line = format!("POST /v1{path} HTTP/1.1");
        self.received()
            .into_iter()
            .filter(|request| request["request_line"] == line)
            .collect()
    }

    /// The streaming sessions opened so far, in order: each one's request
    /// record, with the turns it heard (`heard`), the `speaking` it was
    /// told (`agent`), and whether it was told to end (`ended`).
    pub fn streams(&self) -> Vec<Request> {
        self.received()
            .into_iter()
            .filter(|request| {
                request["request_line"]
                    .as_str()
                    .is_some_and(|line| line.starts_with(&format!("GET {STREAM_PATH}")))
            })
            .collect()
    }

    /// The texts the backend was asked to transcribe, in order.
    pub fn decoded(&self) -> Vec<String> {
        self.received()
            .into_iter()
            .filter(|request| {
                ["/interactions", "/audio/transcriptions"]
                    .iter()
                    .any(|path| request["request_line"] == format!("POST /v1{path} HTTP/1.1"))
            })
            .map(|request| request["heard"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    /// The words the backend heard either way: each streaming session's
    /// turns, then the texts it was asked to transcribe.
    pub fn heard(&self) -> Vec<String> {
        self.streams()
            .iter()
            .flat_map(|stream| {
                stream["heard"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|text| text.as_str().map(str::to_owned))
            })
            .chain(self.decoded())
            .collect()
    }
}

/// The answer to one request: its status and JSON body, or `None` to
/// never answer.
fn answered(
    path: &str,
    content_type: &str,
    body: &[u8],
    record: &mut Json,
) -> Option<(&'static str, Json)> {
    let ok = "200 OK";
    if let Some(rate) = path.strip_prefix("/v1/emotion?sample_rate=") {
        record["sample_rate"] = json!(rate);
        record["heard"] = json!(spelled(&samples_of(body).0));
        return Some((
            ok,
            json!({"label": "neutral", "scores": {"neutral": 1.0}, "valence": 0.0, "confidence": 1.0}),
        ));
    }
    match path {
        "/v1/vad" => {
            let request: Json = serde_json::from_slice(body).unwrap_or(Json::Null);
            let samples: Vec<f64> = request["audio"]
                .as_array()
                .map(|audio| audio.iter().filter_map(Json::as_f64).collect())
                .unwrap_or_default();
            record["model"] = request["model"].clone();
            record["samples"] = json!(samples.len());
            #[expect(clippy::cast_possible_truncation, reason = "samples in [-1, 1]")]
            let pcm: Vec<i16> = samples
                .iter()
                .map(|sample| (sample * 32768.0).round() as i16)
                .collect();
            record["heard"] = json!(spelled(&pcm));
            let segments: Vec<Json> = runs(samples.iter().map(|sample| sample.abs() > 1e-4))
                .into_iter()
                .map(|(start, end)| json!({"start": seconds(start, VAD_RATE_HZ), "end": seconds(end, VAD_RATE_HZ)}))
                .collect();
            Some((ok, json!({"segments": segments})))
        }
        "/v1/audio/diarization" => {
            let (fields, file) = form(body, content_type);
            let (samples, rate) = samples_of(&file);
            let rate = f64::from(rate.unwrap_or(16_000));
            record["model"] = fields["model"].clone();
            record["form"] = fields;
            record["heard"] = json!(spelled(&samples));
            let segments: Vec<Json> = runs(samples.iter().map(|sample| *sample != 0))
                .into_iter()
                .enumerate()
                .map(|(id, (start, end))| {
                    json!({"id": id, "speaker": "SPEAKER_00", "label": "SPEAKER_00", "start": seconds(start, rate), "end": seconds(end, rate)})
                })
                .collect();
            Some((
                ok,
                json!({"task": "diarize", "duration": seconds(samples.len(), rate), "num_speakers": 1, "segments": segments}),
            ))
        }
        "/v1/voice/embed" => {
            let request: Json = serde_json::from_slice(body).unwrap_or(Json::Null);
            let audio = unbase64(request["audio"].as_str().unwrap_or_default());
            let text = spelled(&samples_of(&audio).0);
            record["model"] = request["model"].clone();
            record["heard"] = json!(text);
            if text.starts_with("hang") {
                return None;
            }
            let embedding = [
                text.chars().count(),
                text.chars().map(|character| character as usize).sum(),
            ];
            Some((
                ok,
                json!({"embedding": embedding, "dim": 2, "model": request["model"]}),
            ))
        }
        "/v1/interactions" => {
            let request: Json = serde_json::from_slice(body).unwrap_or(Json::Null);
            let audio = &request["input"][1];
            let text =
                spelled(&samples_of(&unbase64(audio["data"].as_str().unwrap_or_default())).0);
            record["model"] = request["model"].clone();
            record["prompt"] = request["input"][0]["text"].clone();
            record["mime_type"] = audio["mime_type"].clone();
            record["heard"] = json!(text);
            transcribed(
                &text,
                json!({"steps": [{"type": "model_output", "content": [{"type": "text", "text": text}]}]}),
            )
        }
        _ => {
            let (fields, file) = form(body, content_type);
            let text = spelled(&samples_of(&file).0);
            record["model"] = fields["model"].clone();
            record["form"] = fields;
            record["heard"] = json!(text);
            transcribed(&text, json!({"text": text}))
        }
    }
}

/// A transcription's answer: never for text starting with `hang`, 500 for
/// text starting with `fail`, else `ok`.
fn transcribed(text: &str, ok: Json) -> Option<(&'static str, Json)> {
    if text.starts_with("hang") {
        None
    } else if text.starts_with("fail") {
        Some((
            "500 Internal Server Error",
            json!({"error": {"message": "boom"}}),
        ))
    } else {
        Some(("200 OK", ok))
    }
}

fn answer(mut stream: std::net::TcpStream, log: &Mutex<Vec<Request>>, streams: bool) {
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(clone);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.trim_end().split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
    }
    let header = |wanted: &str| {
        headers
            .iter()
            .find(|(name, _)| name == wanted)
            .map(|(_, value)| value.clone())
    };
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    let keyed = header("authorization").as_deref() == Some(&format!("Bearer {FAKE_KEY}"))
        || header("x-goog-api-key").as_deref() == Some(FAKE_KEY);
    if path.starts_with(STREAM_PATH) {
        let record = json!({"request_line": request_line.trim_end(), "keyed": keyed});
        match header("sec-websocket-key") {
            Some(key) if streams => stream::serve(stream, &key, record, log),
            _ => {
                log.lock().expect("the log lock").push(record);
                let _ = write!(
                    stream,
                    "HTTP/1.1 404 Not Found\r\ncontent-type: application/json\r\ncontent-length: 22\r\nconnection: close\r\n\r\n{{\"detail\":\"Not Found\"}}"
                );
            }
        }
        return;
    }
    let length = header("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let mut record = json!({
        "request_line": request_line.trim_end(),
        "keyed": keyed,
    });
    let reply = answered(
        &path,
        &header("content-type").unwrap_or_default(),
        &body,
        &mut record,
    );
    // Voice detection of text starting with `slow` answers once the next
    // sound's voice detection has been asked (at most 10 s), and says
    // whether it was (`overlapped`).
    if path == "/v1/vad"
        && record["heard"]
            .as_str()
            .is_some_and(|heard| heard.starts_with("slow"))
    {
        let asked = log.lock().expect("the log lock").len();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut overlapped = false;
        while !overlapped && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
            overlapped = log.lock().expect("the log lock")[asked..]
                .iter()
                .any(|later| later["request_line"] == "POST /v1/vad HTTP/1.1");
        }
        record["overlapped"] = json!(overlapped);
    }
    log.lock().expect("the log lock").push(record);
    let Some((status, answer)) = reply else {
        // Never answered: hold the connection until the client closes it.
        let mut rest = Vec::new();
        let _ = reader.read_to_end(&mut rest);
        return;
    };
    let answer = answer.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
        answer.len()
    );
}

/// A run's hearing grants: the `scx run` flags of a run whose voice and
/// speech backend is `backend`, and its media directory (granted to the
/// speech service at `/media`, where `media/<file>` references resolve).
pub struct Granted {
    /// `--env-file` (the fake key and `backend`'s URL) and `--dir`.
    pub args: Vec<String>,
    /// The granted media directory.
    pub media: PathBuf,
}

/// Grants a fresh media directory holding each `(name, bytes)` of `files`.
pub fn granted(test: &str, backend: &Backend, files: &[(&str, Vec<u8>)]) -> Granted {
    let media = super::unique_scratch("media");
    for (name, bytes) in files {
        std::fs::write(media.join(name), bytes).expect("the media file is writable");
    }
    let env: PathBuf = super::scratch("hearing-env").join(format!("{test}.env"));
    std::fs::write(
        &env,
        format!(
            "FLOPPY_SPEECH_API_KEY={FAKE_KEY}\nFLOPPY_SPEECH_BASE_URL={url}\nFLOPPY_VOICE_API_KEY={FAKE_KEY}\nFLOPPY_VOICE_BASE_URL={url}\nFLOPPY_EMOTION_API_KEY={FAKE_KEY}\nFLOPPY_EMOTION_BASE_URL={url}\n",
            url = backend.base_url
        ),
    )
    .expect("the env file is writable");
    let mut args = vec!["--env-file".to_owned(), env.display().to_string()];
    args.extend(super::speech_grant(&media));
    Granted { args, media }
}

/// A 16 kHz mono WAV spelling `text`.
pub fn spoken_wav(text: &str) -> Vec<u8> {
    wav(&voiced(text), 16_000, 1)
}

/// A 16 kHz mono WAV of 100 ms of silence.
pub fn silent_wav() -> Vec<u8> {
    wav(&silence(1600), 16_000, 1)
}
