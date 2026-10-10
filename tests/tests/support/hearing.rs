//! A deterministic fake speech backend on 127.0.0.1 for the listening
//! tests: it speaks the `OpenAI`-compatible `POST /v1/audio/transcriptions`
//! the speech service (`services/speech`, operation `decode`) sends, and
//! decodes the uploaded file's name into words, so a test chooses what is
//! heard by naming the audio: `media/reply-to-me.wav` is heard as
//! `reply to me`. A name starting with `fail` answers 500, and one starting
//! with `hang` is never answered (the interpretation's stage timer fails
//! it). Every request is recorded; nothing reaches the network.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Value as Json, json};

/// The fake key (not a secret): the mock checks where it arrives.
pub const FAKE_KEY: &str = "sk-floppy-hearing-fake";

/// One transcription request: its request line, whether it carried the
/// fake key, and its form fields (`model`, `response_format`) and file
/// (`filename`, `content_type`).
pub type Request = Json;

/// The running fake backend.
pub struct Backend {
    /// Its base URL, `http://127.0.0.1:<port>/v1`.
    pub base_url: String,
    received: Arc<Mutex<Vec<Request>>>,
}

/// The form fields of a multipart body (`model`, `response_format`) and
/// its file part's `filename` and `content_type`.
fn form(body: &str) -> Json {
    let mut fields = serde_json::Map::new();
    let quoted = |head: &str, key: &str| {
        head.split(key)
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .map(str::to_owned)
    };
    for part in body
        .split("\r\n--")
        .filter(|part| part.contains("\r\n\r\n"))
    {
        let (head, value) = part.split_once("\r\n\r\n").unwrap_or((part, ""));
        let Some(name) = quoted(head, " name=\"") else {
            continue;
        };
        if let Some(filename) = quoted(head, "filename=\"") {
            let content_type = head
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(key, _)| key.eq_ignore_ascii_case("content-type"))
                        .map(|(_, value)| value.trim().to_owned())
                })
                .unwrap_or_default();
            fields.insert("filename".into(), json!(filename));
            fields.insert("content_type".into(), json!(content_type));
        } else {
            fields.insert(name, json!(value));
        }
    }
    Json::Object(fields)
}

impl Backend {
    /// Starts the backend.
    pub fn start() -> Self {
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
                std::thread::spawn(move || answer(stream, &log));
            }
        });
        Self { base_url, received }
    }

    /// The requests received so far, in order.
    pub fn received(&self) -> Vec<Request> {
        self.received.lock().expect("the log lock").clone()
    }
}

fn answer(mut stream: std::net::TcpStream, log: &Mutex<Vec<Request>>) {
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
    let length = header("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let fields = form(&String::from_utf8_lossy(&body));
    let filename = fields["filename"].as_str().unwrap_or_default().to_owned();
    log.lock().expect("the log lock").push(json!({
        "request_line": request_line.trim_end(),
        "keyed": header("authorization").as_deref() == Some(&format!("Bearer {FAKE_KEY}")),
        "form": fields,
    }));
    let name = filename.split('.').next().unwrap_or_default();
    let (status, answer) = if name.starts_with("hang") {
        // Never answered: hold the connection until the client closes it.
        let mut rest = Vec::new();
        let _ = reader.read_to_end(&mut rest);
        return;
    } else if name.starts_with("fail") {
        (
            "500 Internal Server Error",
            r#"{"error":{"message":"boom"}}"#.to_owned(),
        )
    } else {
        (
            "200 OK",
            json!({"text": name.replace('-', " ")}).to_string(),
        )
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
        answer.len()
    );
}

/// A run's speech grants: the `scx run` flags of a run whose speech
/// backend is `backend`, and its media directory (granted to the speech
/// service at `/media`, where `media/<file>` references resolve).
pub struct Granted {
    /// `--env-file` (the fake key and `backend`'s URL) and `--dir`.
    pub args: Vec<String>,
    /// The granted media directory.
    pub media: PathBuf,
}

/// Grants a fresh media directory holding a small fake audio file for each
/// name in `files`.
pub fn granted(test: &str, backend: &Backend, files: &[&str]) -> Granted {
    let media = super::unique_scratch("media");
    for name in files {
        std::fs::write(media.join(name), b"RIFF-fake-audio").expect("the media file is writable");
    }
    let env: PathBuf = super::scratch("hearing-env").join(format!("{test}.env"));
    std::fs::write(
        &env,
        format!(
            "FLOPPY_SPEECH_API_KEY={FAKE_KEY}\nFLOPPY_SPEECH_BASE_URL={}\n",
            backend.base_url
        ),
    )
    .expect("the env file is writable");
    let mut args = vec!["--env-file".to_owned(), env.display().to_string()];
    args.extend(super::media_grant(&media));
    Granted { args, media }
}
