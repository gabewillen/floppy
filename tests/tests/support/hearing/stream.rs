//! The fake backend's streaming hearing session: api.willen.dev's
//! WebSocket `GET /v1/audio/stream` (the speech service's `stream`
//! operation), hand-rolled over the accepted connection so no WebSocket
//! crate is needed.
//!
//! It says `ready` once open, then hears each binary frame of 16 kHz mono
//! signed 16-bit PCM as the backend hears any audio: a run of non-zero
//! samples is voice. A voice's first sample is `speech_start {t}`, and
//! [`ENDING_SAMPLES`] of silence after it end the turn: `speech_end {t}`,
//! `final {text, confidence}` with the text spelled in the turn's voice,
//! and `turn_end {reason, p, text}`. Text frames are recorded (`agent`
//! frames by their `speaking`); `{"type":"end"}` closes the session.

use std::io::{Read as _, Write as _};
use std::net::TcpStream;

use serde_json::{Value as Json, json};

use super::{Request, spelled};

/// Silence that ends a turn: 100 ms at 16 kHz.
pub const ENDING_SAMPLES: usize = 1600;

/// The confidence every transcript gives.
pub const CONFIDENCE: f64 = 0.9;

/// The `Sec-WebSocket-Accept` of a client's key (RFC 6455 section 4.2.2).
pub fn accept_key(key: &str) -> String {
    let digest = sha1(format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes());
    base64(&digest)
}

/// SHA-1 (FIPS 180-4), for the handshake only.
#[expect(clippy::many_single_char_names, reason = "the standard's names")]
fn sha1(message: &[u8]) -> [u8; 20] {
    let mut state: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&((message.len() as u64) * 8).to_be_bytes());
    for block in padded.chunks(64) {
        let mut words = [0_u32; 80];
        for (index, word) in block.chunks(4).enumerate() {
            words[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for index in 16..80 {
            words[index] =
                (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16])
                    .rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = state;
        for (index, word) in words.iter().enumerate() {
            let (f, k) = match index {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        for (held, added) in state.iter_mut().zip([a, b, c, d, e]) {
            *held = held.wrapping_add(added);
        }
    }
    let mut digest = [0_u8; 20];
    for (index, word) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::new();
    for chunk in bytes.chunks(3) {
        let value = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |value, (index, byte)| {
                value | (u32::from(*byte) << (16 - 8 * index))
            });
        for index in 0..4 {
            if index <= chunk.len() {
                text.push(char::from(
                    ALPHABET[((value >> (18 - 6 * index)) & 63) as usize],
                ));
            } else {
                text.push('=');
            }
        }
    }
    text
}

/// One frame from the client: its opcode and unmasked payload.
pub fn read_frame(stream: &mut TcpStream) -> Option<(u8, Vec<u8>)> {
    let mut head = [0_u8; 2];
    stream.read_exact(&mut head).ok()?;
    let opcode = head[0] & 0x0f;
    let masked = head[1] & 0x80 != 0;
    let length = match head[1] & 0x7f {
        126 => {
            let mut bytes = [0_u8; 2];
            stream.read_exact(&mut bytes).ok()?;
            usize::from(u16::from_be_bytes(bytes))
        }
        127 => {
            let mut bytes = [0_u8; 8];
            stream.read_exact(&mut bytes).ok()?;
            usize::try_from(u64::from_be_bytes(bytes)).ok()?
        }
        short => usize::from(short),
    };
    let mut mask = [0_u8; 4];
    if masked {
        stream.read_exact(&mut mask).ok()?;
    }
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload).ok()?;
    if masked {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    Some((opcode, payload))
}

/// Writes one unmasked server frame.
pub fn write_frame(stream: &mut TcpStream, opcode: u8, payload: &[u8]) -> bool {
    let mut frame = vec![0x80 | opcode];
    match payload.len() {
        length @ 0..=125 => frame.push(u8::try_from(length).expect("a short length")),
        length @ 126..=0xffff => {
            frame.push(126);
            frame.extend_from_slice(
                &u16::try_from(length)
                    .expect("a medium length")
                    .to_be_bytes(),
            );
        }
        length => {
            frame.push(127);
            frame.extend_from_slice(&(length as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(payload);
    stream.write_all(&frame).is_ok()
}

fn say(stream: &mut TcpStream, frame: &Json) -> bool {
    write_frame(stream, 1, frame.to_string().as_bytes())
}

#[expect(clippy::cast_precision_loss, reason = "a test's audio is seconds long")]
fn seconds(samples: usize) -> f64 {
    samples as f64 / 16_000.0
}

/// Serves one streaming session on `stream`, whose request asked for
/// `path` with the WebSocket `key`; `record` is its request record, which
/// gains `heard` (each turn's text), `agent` (each `speaking` told) and
/// `ended` (whether the client said `end`), and `log` the backend's.
pub fn serve(mut stream: TcpStream, key: &str, record: Json, log: &std::sync::Mutex<Vec<Request>>) {
    let index = {
        let mut log = log.lock().expect("the log lock");
        log.push(record);
        log.len() - 1
    };
    let update = |change: &dyn Fn(&mut Json)| {
        if let Some(record) = log.lock().expect("the log lock").get_mut(index) {
            change(record);
        }
    };
    update(&|record| {
        record["heard"] = json!([]);
        record["agent"] = json!([]);
        record["ended"] = json!(false);
    });
    if write!(
        stream,
        "HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-accept: {}\r\n\r\n",
        accept_key(key)
    )
    .is_err()
        || !say(&mut stream, &json!({"type": "ready"}))
    {
        return;
    }
    let mut heard = 0_usize;
    let mut voice: Vec<i16> = Vec::new();
    let mut silent = 0_usize;
    while let Some((opcode, payload)) = read_frame(&mut stream) {
        match opcode {
            2 => {
                for pair in payload.as_chunks::<2>().0 {
                    let sample = i16::from_le_bytes(*pair);
                    heard += 1;
                    if sample != 0 {
                        if voice.is_empty()
                            && !say(
                                &mut stream,
                                &json!({"type": "speech_start", "t": seconds(heard - 1)}),
                            )
                        {
                            return;
                        }
                        voice.push(sample);
                        silent = 0;
                        continue;
                    }
                    if voice.is_empty() {
                        continue;
                    }
                    silent += 1;
                    if silent < ENDING_SAMPLES {
                        continue;
                    }
                    let text = spelled(&voice);
                    let at = seconds(heard);
                    let said = [
                        json!({"type": "speech_end", "t": at}),
                        json!({"type": "final", "text": text, "confidence": CONFIDENCE, "words": [], "t": at}),
                        json!({"type": "turn_end", "reason": "eot", "p": CONFIDENCE, "text": text, "t": at}),
                    ];
                    if !said.iter().all(|frame| say(&mut stream, frame)) {
                        return;
                    }
                    update(&|record| {
                        if let Some(heard) = record["heard"].as_array_mut() {
                            heard.push(json!(text));
                        }
                    });
                    voice.clear();
                    silent = 0;
                }
            }
            1 => {
                let frame: Json = serde_json::from_slice(&payload).unwrap_or(Json::Null);
                match frame["type"].as_str() {
                    Some("agent") => update(&|record| {
                        if let Some(agent) = record["agent"].as_array_mut() {
                            agent.push(frame["speaking"].clone());
                        }
                    }),
                    Some("end") => {
                        update(&|record| record["ended"] = json!(true));
                        let _ = write_frame(&mut stream, 8, &1000_u16.to_be_bytes());
                        return;
                    }
                    _ => {}
                }
            }
            8 => {
                let _ = write_frame(&mut stream, 8, &payload);
                return;
            }
            9 if !write_frame(&mut stream, 10, &payload) => return,
            _ => {}
        }
    }
}
