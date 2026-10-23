//! Integration test of the `speech` service through `scx run`: a session
//! binds the real `floppy_service_speech` component (resolved from its
//! repository URI) and invokes it
//! (`<invoke type="https://github.com/stateforward/floppy/services/speech">`)
//! against a deterministic fake backend on 127.0.0.1 that speaks the
//! `OpenAI`-compatible audio API (`/audio/transcriptions`, `/audio/speech`)
//! and `ElevenLabs`' streaming `text-to-speech` (nothing reaches the
//! network). Media moves by reference: the run grants the service a
//! directory with `--dir URI=HOST::/media`, a decoding reads its audio from
//! there, and an encoding writes its audio there. The service answers
//! `bot.ability.hearing.speech.decoding.output`,
//! `bot.ability.vocal.speech.encoding.output` (after each of its
//! `bot.ability.vocal.speech.encoding.chunk`s, decision D203), or
//! `bot.ability.failed`, then completes.
//!
//! The ignored `live_speech_round_trip_through_an_env_file` test makes two
//! real calls (encode, then decode what was encoded):
//! `FLOPPY_LIVE_ENV_FILE=<path> cargo test -p floppy-tests --test
//! speech_service -- --ignored`. The files must set
//! `FLOPPY_SPEECH_BASE_URL` and `FLOPPY_SPEECH_API_KEY` (the service has no
//! default backend). The models are `FLOPPY_LIVE_SPEECH_ENCODE_MODEL` and
//! `FLOPPY_LIVE_SPEECH_DECODE_MODEL`, else `tts-1` and `whisper-1`, and
//! the voice `FLOPPY_LIVE_SPEECH_VOICE`, else `alloy`, and the API
//! `FLOPPY_LIVE_SPEECH_PROVIDER` (`gemini`), else the `OpenAI`-compatible
//! one. It reports pass or
//! fail only; no value is ever printed.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use support::{Finished, logs, run_document, scratch, service_uri, unique_scratch};

/// The fake key (not a secret): the mock checks where it arrives.
const FAKE_KEY: &str = "sk-floppy-speech-fake";

/// What the fake transcription answers.
const HEARD: &str = r#"{"text":" Hello from the fake backend. "}"#;

/// The fake `OpenAI`-compatible MP3 and `ElevenLabs` MP3.
const MP3: &[u8] = b"ID3-fake-openai-mp3";
const ELEVENLABS_MP3: &[u8] = b"ID3-fake-elevenlabs-mp3";

fn speech() -> String {
    service_uri("speech")
}

/// A minimal 16-bit PCM WAV file of a tenth of a second declaring `rate`
/// and `channels`.
fn wav(rate: u32, channels: u16) -> Vec<u8> {
    let data = usize::try_from(rate / 10).expect("small") * usize::from(channels) * 2;
    let mut audio = b"RIFF\x24\0\0\0WAVEfmt \x10\0\0\0\x01\0".to_vec();
    audio.extend_from_slice(&channels.to_le_bytes());
    audio.extend_from_slice(&rate.to_le_bytes());
    audio.extend_from_slice(&[0, 0, 0, 0, 2, 0, 16, 0]);
    audio.extend_from_slice(b"data");
    audio.extend_from_slice(&u32::try_from(data).expect("small").to_le_bytes());
    audio.resize(audio.len() + data, 0);
    audio
}

/// One request the mock received.
#[derive(Clone, Debug)]
struct Received {
    request_line: String,
    authorization: Option<String>,
    xi_api_key: Option<String>,
    content_type: Option<String>,
    body: Vec<u8>,
}

/// The bytes of each piece of [`streamed_pcm`] the mock sends.
const STREAM_PIECE: usize = 10_000;
/// The bytes a stalled answer sends before it waits for the client.
const STALLED_BYTES: usize = 30_000;

/// What `/stream/v1/audio/speech` answers, in pieces: 1.25 s of 24 kHz
/// mono 16-bit PCM, two whole chunks and a quarter-second rest.
fn streamed_pcm() -> Vec<u8> {
    (0..60_000_u32)
        .map(|at| u8::try_from(at % 199).expect("below 199"))
        .collect()
}

/// A deterministic fake audio backend on 127.0.0.1: `/v1/...` answers,
/// `/fail/v1/...` answers 500, `/stream/v1/audio/speech` streams its PCM,
/// and `/stall/v1/audio/speech` sends one chunk's bytes and then waits for
/// the client to close; every request is recorded.
struct Mock {
    origin: String,
    received: Arc<Mutex<Vec<Received>>>,
    closed: Arc<Mutex<bool>>,
}

impl Mock {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock binds a loopback port");
        let origin = format!(
            "http://{}",
            listener.local_addr().expect("the mock has an address")
        );
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        let closed = Arc::new(Mutex::new(false));
        let stalled = Arc::clone(&closed);
        std::thread::spawn(move || {
            let closed = stalled;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
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
                    continue;
                }
                let target = request_line.split(' ').nth(1).unwrap_or("").to_owned();
                let speech_format = serde_json::from_slice::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|body| body["response_format"].as_str().map(str::to_owned));
                log.lock().expect("the log lock").push(Received {
                    request_line: request_line.trim_end().to_owned(),
                    authorization: header("authorization"),
                    xi_api_key: header("xi-api-key"),
                    content_type: header("content-type"),
                    body,
                });
                // A streamed answer: its PCM in chunked pieces as it is
                // "synthesized", or its first chunk and then nothing until
                // the client closes the connection.
                if target == "/stream/v1/audio/speech" {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n"
                    );
                    for piece in streamed_pcm().chunks(STREAM_PIECE) {
                        let _ = write!(stream, "{:x}\r\n", piece.len());
                        let _ = stream.write_all(piece);
                        let _ = stream.write_all(b"\r\n");
                        let _ = stream.flush();
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    let _ = stream.write_all(b"0\r\n\r\n");
                    continue;
                }
                if target == "/stall/v1/audio/speech" {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n{:x}\r\n",
                        STALLED_BYTES
                    );
                    let _ = stream.write_all(&vec![0; STALLED_BYTES]);
                    let _ = stream.write_all(b"\r\n");
                    let _ = stream.flush();
                    // Nothing more until the client goes away.
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
                    let mut rest = [0_u8; 64];
                    if matches!(reader.read(&mut rest), Ok(0)) {
                        *closed.lock().expect("the closed lock") = true;
                    }
                    continue;
                }
                let (status, answer): (&str, Vec<u8>) = match target.as_str() {
                    "/v1/audio/transcriptions" => ("200 OK", HEARD.as_bytes().to_vec()),
                    "/v1/audio/speech" if speech_format.as_deref() == Some("wav") => {
                        ("200 OK", wav(22_050, 1))
                    }
                    "/v1/audio/speech" => ("200 OK", MP3.to_vec()),
                    "/v1/text-to-speech/rachel/stream?output_format=mp3_44100_128" => {
                        ("200 OK", ELEVENLABS_MP3.to_vec())
                    }
                    target if target.starts_with("/fail/") => (
                        "500 Internal Server Error",
                        br#"{"error":{"message":"boom"}}"#.to_vec(),
                    ),
                    _ => ("404 Not Found", b"{}".to_vec()),
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    answer.len()
                );
                let _ = stream.write_all(&answer);
            }
        });
        Self {
            origin,
            received,
            closed,
        }
    }

    /// Whether the client closed a stalled answer's connection.
    fn closed(&self) -> bool {
        *self.closed.lock().expect("the closed lock")
    }

    fn received(&self) -> Vec<Received> {
        self.received.lock().expect("the log lock").clone()
    }
}

/// An env file of exactly `contents`.
fn env_file(test: &str, contents: &str) -> PathBuf {
    let path = scratch("speech-env").join(format!("{test}.env"));
    std::fs::write(&path, contents).expect("the env file is writable");
    path
}

/// The flags of a run whose backend is `base_url` and whose media
/// directory is a fresh one holding `files`, granted at `/media`.
fn granted(test: &str, base_url: &str, files: &[(&str, &[u8])]) -> (PathBuf, Vec<String>) {
    let media = unique_scratch("speech-media");
    for (name, bytes) in files {
        std::fs::write(media.join(name), bytes).expect("the media file is writable");
    }
    let env = env_file(
        test,
        &format!("FLOPPY_SPEECH_API_KEY={FAKE_KEY}\nFLOPPY_SPEECH_BASE_URL={base_url}\n"),
    );
    let args = vec![
        "--env-file".to_owned(),
        env.display().to_string(),
        "--dir".to_owned(),
        format!("{}={}::/media", speech(), media.display()),
    ];
    (media, args)
}

/// A document that invokes the speech service with `params` and logs the
/// one event it answers (`event`: its name and data), finishing in `done` on
/// `done.invoke`, or in `rejected` on `error.execution` (a rejected start).
fn document(params: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env"
       version="1.0" datamodel="ecmascript" initial="speaking">
  <state id="speaking">
    <invoke id="speech" type="{service}" env:apikey="FLOPPY_SPEECH_API_KEY" env:baseurl="FLOPPY_SPEECH_BASE_URL">
      {params}
    </invoke>
    <transition event="bot.ability.*"><log label="event" expr="_event.name + ' ' + JSON.stringify(_event.data)"/></transition>
    <transition event="done.invoke.speech" target="done"/>
    <transition event="error.execution" target="rejected"/>
  </state>
  <final id="done"><onentry><log label="final" expr="'done'"/></onentry></final>
  <final id="rejected"><onentry><log label="final" expr="'rejected'"/></onentry></final>
</scxml>"#,
        service = speech()
    )
}

const DECODE: &str = r#"<param name="operation" expr="'decode'"/>
      <param name="model" expr="'whisper-1'"/>
      <param name="audio" expr="'media/heard.wav'"/>
      <param name="media_type" expr="'audio/wav'"/>"#;

fn encode(extra: &str) -> String {
    format!(
        r#"<param name="operation" expr="'encode'"/>
      <param name="model" expr="'tts-1'"/>
      <param name="parts" expr="[{{words: 'Hello there.'}}]"/>
      {extra}"#
    )
}

fn open(args: &[String], params: &str) -> Finished {
    let finished = run_document(
        "speech.scx",
        &document(params),
        args,
        &[],
        Duration::from_secs(60),
    );
    let consent = &finished.stderr;
    assert!(
        consent.contains(&format!("service `{}`", speech()))
            && consent.contains("outbound HTTP: none")
            && consent.contains("env slots: apikey, baseurl"),
        "the consent listing names the package, no origin, and its slots: {consent}"
    );
    assert!(!consent.contains(FAKE_KEY), "names only: {consent}");
    finished
}

fn events(finished: &Finished) -> Vec<(String, String)> {
    logs(&finished.records)
}

fn labelled(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    support::labelled(pairs)
}

#[test]
fn a_decoding_posts_the_granted_audio_and_answers_the_text() {
    let mock = Mock::start();
    let audio = wav(16_000, 1);
    let (_, args) = granted(
        "decode",
        &format!("{}/v1", mock.origin),
        &[("heard.wav", &audio)],
    );
    let finished = open(&args, DECODE);
    assert_eq!(
        events(&finished),
        labelled(&[
            (
                "event",
                r#"bot.ability.hearing.speech.decoding.output {"text":"Hello from the fake backend."}"#
            ),
            ("final", "done"),
        ]),
        "{}",
        finished.stderr
    );
    let received = mock.received();
    assert_eq!(received.len(), 1, "exactly one request");
    assert_eq!(
        received[0].request_line,
        "POST /v1/audio/transcriptions HTTP/1.1"
    );
    assert_eq!(
        received[0].authorization.as_deref(),
        Some(format!("Bearer {FAKE_KEY}").as_str()),
        "the host substitutes the env-file key into the bearer header"
    );
    let boundary = received[0]
        .content_type
        .as_deref()
        .and_then(|value| value.strip_prefix("multipart/form-data; boundary="))
        .expect("a multipart body");
    let mut expected = format!(
        "--{boundary}\r\ncontent-disposition: form-data; name=\"model\"\r\n\r\nwhisper-1\r\n\
         --{boundary}\r\ncontent-disposition: form-data; name=\"response_format\"\r\n\r\njson\r\n\
         --{boundary}\r\ncontent-disposition: form-data; name=\"file\"; filename=\"heard.wav\"\r\ncontent-type: audio/wav\r\n\r\n"
    )
    .into_bytes();
    expected.extend_from_slice(&audio);
    expected.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    assert_eq!(received[0].body, expected, "the audio's bytes, in a form");
}

#[test]
fn an_encoding_writes_the_audio_and_answers_its_reference() {
    let mock = Mock::start();
    let (media, args) = granted("encode", &format!("{}/v1", mock.origin), &[]);
    let finished = open(
        &args,
        &encode(
            r#"<param name="voice" expr="'alloy'"/>
      <param name="output" expr="'media/said.mp3'"/>"#,
        ),
    );
    assert_eq!(
        events(&finished),
        labelled(&[
            (
                "event",
                r#"bot.ability.vocal.speech.encoding.chunk {"audio":"media/said.0.mp3","channels":null,"duration":null,"media_type":"audio/mpeg","sample_rate_hz":null,"sequence":0}"#
            ),
            (
                "event",
                r#"bot.ability.vocal.speech.encoding.output {"audio":"media/said.mp3","channels":null,"duration":null,"media_type":"audio/mpeg","sample_rate_hz":null}"#
            ),
            ("final", "done"),
        ]),
        "{}",
        finished.stderr
    );
    assert_eq!(
        std::fs::read(media.join("said.mp3")).expect("the audio is in the granted directory"),
        MP3
    );
    assert_eq!(
        std::fs::read(media.join("said.0.mp3")).expect("the chunk is in the granted directory"),
        MP3,
        "a short answer is one chunk"
    );
    let received = mock.received();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].request_line, "POST /v1/audio/speech HTTP/1.1");
    assert_eq!(
        String::from_utf8(received[0].body.clone()).expect("UTF-8"),
        r#"{"input":"Hello there.","model":"tts-1","response_format":"mp3","voice":"alloy"}"#
    );
}

#[test]
fn a_wav_encoding_names_the_rate_and_channels_its_header_declares() {
    let mock = Mock::start();
    let (media, args) = granted("wav", &format!("{}/v1", mock.origin), &[]);
    let finished = open(
        &args,
        &encode(
            r#"<param name="voice" expr="'alloy'"/>
      <param name="output" expr="'/media/said.wav'"/>
      <param name="media_type" expr="'audio/wav'"/>"#,
        ),
    );
    assert_eq!(
        events(&finished),
        labelled(&[
            (
                "event",
                r#"bot.ability.vocal.speech.encoding.chunk {"audio":"/media/said.0.wav","channels":1,"duration":0.1,"media_type":"audio/wav","sample_rate_hz":22050,"sequence":0}"#
            ),
            (
                "event",
                r#"bot.ability.vocal.speech.encoding.output {"audio":"/media/said.wav","channels":1,"duration":0.1,"media_type":"audio/wav","sample_rate_hz":22050}"#
            ),
            ("final", "done"),
        ]),
        "{}",
        finished.stderr
    );
    assert_eq!(
        std::fs::read(media.join("said.wav")).expect("the audio is written"),
        wav(22_050, 1)
    );
}

/// A streamed answer is returned chunk by chunk as it arrives, cut at
/// fixed offsets (decision D203), then its output (the whole answer).
#[test]
fn a_streamed_answer_is_returned_chunk_by_chunk_before_its_output() {
    let mock = Mock::start();
    let (media, args) = granted("stream", &format!("{}/stream/v1", mock.origin), &[]);
    let finished = open(
        &args,
        &encode(
            r#"<param name="voice" expr="'alloy'"/>
      <param name="output" expr="'media/said.pcm'"/>
      <param name="media_type" expr="'audio/pcm'"/>"#,
        ),
    );
    assert_eq!(
        events(&finished),
        labelled(&[
            (
                "event",
                r#"bot.ability.vocal.speech.encoding.chunk {"audio":"media/said.0.pcm","channels":1,"duration":0.5,"media_type":"audio/pcm","sample_rate_hz":24000,"sequence":0}"#
            ),
            (
                "event",
                r#"bot.ability.vocal.speech.encoding.chunk {"audio":"media/said.1.pcm","channels":1,"duration":0.5,"media_type":"audio/pcm","sample_rate_hz":24000,"sequence":1}"#
            ),
            (
                "event",
                r#"bot.ability.vocal.speech.encoding.chunk {"audio":"media/said.2.pcm","channels":1,"duration":0.25,"media_type":"audio/pcm","sample_rate_hz":24000,"sequence":2}"#
            ),
            (
                "event",
                r#"bot.ability.vocal.speech.encoding.output {"audio":"media/said.pcm","channels":1,"duration":1.25,"media_type":"audio/pcm","sample_rate_hz":24000}"#
            ),
            ("final", "done"),
        ]),
        "{}",
        finished.stderr
    );
    let pcm = streamed_pcm();
    assert_eq!(
        std::fs::read(media.join("said.pcm")).expect("the audio is written"),
        pcm
    );
    let chunks: Vec<u8> = (0..3)
        .flat_map(|at| {
            std::fs::read(media.join(format!("said.{at}.pcm"))).expect("the chunk is written")
        })
        .collect();
    assert_eq!(chunks, pcm, "the chunks in order are the answer");
}

/// The first chunk is returned while its answer is still arriving: a
/// session that leaves the invoking state on it cancels the invocation,
/// which closes the connection before the backend sends the rest.
#[test]
fn the_first_chunk_arrives_before_its_answer_ends() {
    let mock = Mock::start();
    let (media, args) = granted("stall", &format!("{}/stall/v1", mock.origin), &[]);
    let finished = run_document(
        "speech.scx",
        &format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env"
       version="1.0" datamodel="ecmascript" initial="speaking">
  <state id="speaking">
    <invoke id="speech" type="{service}" env:apikey="FLOPPY_SPEECH_API_KEY" env:baseurl="FLOPPY_SPEECH_BASE_URL">
      {params}
    </invoke>
    <transition event="bot.ability.vocal.speech.encoding.chunk" target="stopped">
      <log label="event" expr="_event.name + ' ' + JSON.stringify(_event.data)"/>
    </transition>
    <transition event="*" target="unexpected"><log label="event" expr="_event.name"/></transition>
  </state>
  <final id="stopped"><onentry><log label="final" expr="'stopped'"/></onentry></final>
  <final id="unexpected"/>
</scxml>"#,
            service = speech(),
            params = encode(
                r#"<param name="voice" expr="'alloy'"/>
      <param name="output" expr="'media/said.pcm'"/>
      <param name="media_type" expr="'audio/pcm'"/>"#
            )
        ),
        &args,
        &[],
        Duration::from_secs(60),
    );
    assert_eq!(
        events(&finished),
        labelled(&[
            (
                "event",
                r#"bot.ability.vocal.speech.encoding.chunk {"audio":"media/said.0.pcm","channels":1,"duration":0.5,"media_type":"audio/pcm","sample_rate_hz":24000,"sequence":0}"#
            ),
            ("final", "stopped"),
        ]),
        "{}",
        finished.stderr
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !mock.closed() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        mock.closed(),
        "the cancelled invocation closed its connection"
    );
    assert!(
        !media.join("said.pcm").exists(),
        "a cancelled encoding writes no output"
    );
}

#[test]
fn an_elevenlabs_encoding_uses_its_text_to_speech_api() {
    let mock = Mock::start();
    let (media, args) = granted("elevenlabs", &format!("{}/v1", mock.origin), &[]);
    let finished = open(
        &args,
        &encode(
            r#"<param name="voice" expr="'rachel'"/>
      <param name="output" expr="'file:///media/said.mp3'"/>
      <param name="provider" expr="'elevenlabs'"/>"#,
        ),
    );
    assert_eq!(
        events(&finished),
        labelled(&[
            (
                "event",
                r#"bot.ability.vocal.speech.encoding.chunk {"audio":"file:///media/said.0.mp3","channels":null,"duration":null,"media_type":"audio/mpeg","sample_rate_hz":44100,"sequence":0}"#
            ),
            (
                "event",
                r#"bot.ability.vocal.speech.encoding.output {"audio":"file:///media/said.mp3","channels":null,"duration":null,"media_type":"audio/mpeg","sample_rate_hz":44100}"#
            ),
            ("final", "done"),
        ]),
        "{}",
        finished.stderr
    );
    assert_eq!(
        std::fs::read(media.join("said.mp3")).expect("the audio is written"),
        ELEVENLABS_MP3
    );
    let received = mock.received();
    assert_eq!(received.len(), 1);
    assert_eq!(
        received[0].request_line,
        "POST /v1/text-to-speech/rachel/stream?output_format=mp3_44100_128 HTTP/1.1"
    );
    assert_eq!(received[0].xi_api_key.as_deref(), Some(FAKE_KEY));
    assert_eq!(
        received[0].authorization, None,
        "ElevenLabs takes xi-api-key"
    );
    assert_eq!(
        String::from_utf8(received[0].body.clone()).expect("UTF-8"),
        r#"{"model_id":"tts-1","text":"Hello there."}"#
    );
}

/// (`support::output` runs `scx` as given: no directory is granted.)
#[test]
fn a_run_that_binds_the_service_without_a_directory_is_refused() {
    let path = support::write(
        &unique_scratch("speech-documents"),
        "speech.scx",
        &document(DECODE),
    );
    let refused = support::output(&[
        "run".to_owned(),
        "--clock".to_owned(),
        "logical".to_owned(),
        "--on-eof".to_owned(),
        "close".to_owned(),
        path.display().to_string(),
    ]);
    assert_eq!(refused.status.code(), Some(2), "{}", refused.stderr);
    assert!(refused.stdout.is_empty(), "nothing ran: {}", refused.stdout);
    assert!(
        refused
            .stderr
            .contains("imports a WASI interface whose resource the run does not grant"),
        "{}",
        refused.stderr
    );
}

#[test]
fn an_unconfigured_backend_answers_failed_and_reads_and_sends_nothing() {
    let mock = Mock::start();
    let (media, mut args) = granted("unconfigured", "unused", &[]);
    let empty = env_file("unconfigured-empty", "");
    args[1] = empty.display().to_string();
    let finished = open(
        &args,
        &encode(
            r#"<param name="voice" expr="'alloy'"/>
      <param name="output" expr="'media/said.mp3'"/>"#,
        ),
    );
    assert_eq!(
        events(&finished),
        labelled(&[
            (
                "event",
                r#"bot.ability.failed {"message":"Speech encoding failed: the backend is not configured."}"#
            ),
            ("final", "done"),
        ]),
        "{}",
        finished.stderr
    );
    assert!(mock.received().is_empty());
    assert!(!media.join("said.mp3").exists(), "nothing is written");
}

/// A run that holds a base URL and no key asks a keyless speech server:
/// the request carries no key header at all (decision D127).
#[test]
fn without_a_held_key_the_request_carries_no_key_header() {
    let mock = Mock::start();
    let audio = wav(16_000, 1);
    let (_, mut args) = granted(
        "keyless",
        &format!("{}/v1", mock.origin),
        &[("heard.wav", &audio)],
    );
    let keyless = env_file(
        "keyless-base",
        &format!("FLOPPY_SPEECH_BASE_URL={}/v1\n", mock.origin),
    );
    args[1] = keyless.display().to_string();
    let finished = open(&args, DECODE);
    assert_eq!(
        events(&finished),
        labelled(&[
            (
                "event",
                r#"bot.ability.hearing.speech.decoding.output {"text":"Hello from the fake backend."}"#
            ),
            ("final", "done"),
        ]),
        "{}",
        finished.stderr
    );
    let received = mock.received();
    assert_eq!(received.len(), 1, "exactly one request");
    assert_eq!(
        (&received[0].authorization, &received[0].xi_api_key),
        (&None, &None),
        "a keyless request has no key header"
    );
}

#[test]
fn a_malformed_parameter_raises_error_execution() {
    let mock = Mock::start();
    let (_, args) = granted("malformed", &format!("{}/v1", mock.origin), &[]);
    let finished = open(
        &args,
        &encode(
            r#"<param name="voice" expr="'alloy'"/>
      <param name="output" expr="'media/../said.mp3'"/>"#,
        ),
    );
    assert_eq!(
        events(&finished),
        labelled(&[("final", "rejected")]),
        "{}",
        finished.stderr
    );
    assert!(mock.received().is_empty());
}

#[test]
fn audio_outside_the_granted_directory_cannot_be_read() {
    let mock = Mock::start();
    let (_, args) = granted("outside", &format!("{}/v1", mock.origin), &[]);
    let finished = open(
        &args,
        r#"<param name="operation" expr="'decode'"/>
      <param name="model" expr="'whisper-1'"/>
      <param name="audio" expr="'etc/hostname'"/>"#,
    );
    assert_eq!(
        events(&finished),
        labelled(&[
            (
                "event",
                r#"bot.ability.failed {"message":"Speech decoding failed: the audio cannot be read."}"#
            ),
            ("final", "done"),
        ]),
        "{}",
        finished.stderr
    );
    assert!(mock.received().is_empty());
}

#[test]
fn a_failing_backend_answers_failed_and_writes_nothing() {
    let mock = Mock::start();
    let (media, args) = granted("fail", &format!("{}/fail/v1", mock.origin), &[]);
    let finished = open(
        &args,
        &encode(
            r#"<param name="voice" expr="'alloy'"/>
      <param name="output" expr="'media/said.mp3'"/>"#,
        ),
    );
    assert_eq!(
        events(&finished),
        labelled(&[
            (
                "event",
                r#"bot.ability.failed {"message":"Speech encoding failed: the backend answered an error status."}"#
            ),
            ("final", "done"),
        ]),
        "{}",
        finished.stderr
    );
    assert_eq!(mock.received().len(), 1);
    assert!(!media.join("said.mp3").exists(), "nothing is written");
}

fn live_setting(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

/// Encodes a sentence on the live backend, then decodes the audio it wrote.
#[test]
#[ignore = "live: needs FLOPPY_LIVE_ENV_FILE with FLOPPY_SPEECH_BASE_URL and FLOPPY_SPEECH_API_KEY"]
fn live_speech_round_trip_through_an_env_file() {
    let media = unique_scratch("speech-live-media");
    let mut args = Vec::new();
    for file in support::live_env_files() {
        args.extend(["--env-file".to_owned(), file.display().to_string()]);
    }
    args.extend([
        "--dir".to_owned(),
        format!("{}={}::/media", speech(), media.display()),
    ]);
    // `FLOPPY_LIVE_SPEECH_PROVIDER` (`gemini`, say) names the API; unset,
    // the OpenAI-compatible one.
    let provider = std::env::var("FLOPPY_LIVE_SPEECH_PROVIDER")
        .map(|name| format!("\n      <param name=\"provider\" expr=\"'{name}'\"/>"))
        .unwrap_or_default();
    let encoded = run_document(
        "speech.scx",
        &document(&format!(
            r#"<param name="operation" expr="'encode'"/>{provider}
      <param name="model" expr="'{}'"/>
      <param name="parts" expr="[{{words: 'The quick brown fox jumps over the lazy dog.'}}]"/>
      <param name="voice" expr="'{}'"/>
      <param name="output" expr="'media/said.wav'"/>
      <param name="media_type" expr="'audio/wav'"/>"#,
            live_setting("FLOPPY_LIVE_SPEECH_ENCODE_MODEL", "tts-1"),
            live_setting("FLOPPY_LIVE_SPEECH_VOICE", "alloy"),
        )),
        &args,
        &[],
        Duration::from_secs(120),
    );
    let encoded_events = events(&encoded);
    assert!(
        encoded_events.first().is_some_and(
            |(_, event)| event.starts_with("bot.ability.vocal.speech.encoding.output ")
        ),
        "the encoding answered its output: {encoded_events:?}"
    );
    assert!(
        Path::new(&media.join("said.wav"))
            .metadata()
            .is_ok_and(|file| file.len() > 44),
        "the audio is written"
    );
    let decoded = run_document(
        "speech.scx",
        &document(&format!(
            r#"<param name="operation" expr="'decode'"/>{provider}
      <param name="model" expr="'{}'"/>
      <param name="audio" expr="'media/said.wav'"/>
      <param name="media_type" expr="'audio/wav'"/>"#,
            live_setting("FLOPPY_LIVE_SPEECH_DECODE_MODEL", "whisper-1"),
        )),
        &args,
        &[],
        Duration::from_secs(120),
    );
    let decoded_events = events(&decoded);
    let heard = decoded_events
        .first()
        .and_then(|(_, event)| event.strip_prefix("bot.ability.hearing.speech.decoding.output "))
        .and_then(|data| serde_json::from_str::<serde_json::Value>(data).ok())
        .and_then(|data| data["text"].as_str().map(str::to_lowercase))
        .unwrap_or_default();
    assert!(
        heard.contains("fox") && heard.contains("lazy"),
        "the decoding heard the sentence"
    );
}
