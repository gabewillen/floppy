use scxml_event_schema::CompiledSchema;
use scxml_types::DataValue as Data;
use serde_json::{Value as Json, json};

use super::*;
use crate::{Operation, parse_parameters};

fn encoding(parts: &Json, media_type: &str, provider: Option<&str>) -> Encoding {
    let mut parameters = vec![
        ("operation", Data::String("encode".into())),
        ("model", Data::String("tts-1".into())),
        (
            "parts",
            Data::canonical_json_array(&parts.to_string()).expect("the parts are a JSON array"),
        ),
        ("voice", Data::String("alloy".into())),
        ("output", Data::String("media/said.pcm".into())),
        ("media_type", Data::String(media_type.into())),
    ];
    if let Some(provider) = provider {
        parameters.push(("provider", Data::String(provider.into())));
    }
    match parse_parameters(parameters.iter().map(|(name, value)| (*name, value))) {
        Ok(Operation::Encode(encoding)) => encoding,
        other => panic!("a valid encoding: {other:?}"),
    }
}

fn hello(media_type: &str) -> Encoding {
    encoding(&json!([{"words": "Hello there."}]), media_type, None)
}

/// A fake streaming backend: drives a chunker as the service does, each
/// stretch with words answered by the next of `answers`, its bytes arriving
/// in the reads given (Gemini's whole, as one body).
fn streamed(encoding: &Encoding, answers: &[Vec<Vec<u8>>]) -> Result<Vec<Chunk>, FailureCode> {
    let mut chunker = Chunker::new(encoding);
    let mut answers = answers.iter();
    let mut chunks = Vec::new();
    for stretch in &encoding.stretches {
        chunks.extend(chunker.open(stretch.pause));
        if stretch.words.trim().is_empty() {
            continue;
        }
        let reads = answers.next().expect("an answer per stretch with words");
        if encoding.provider == Provider::Gemini {
            let pcm = crate::gemini::audio(&reads.concat())?;
            chunks.extend(chunker.whole(pcm));
            continue;
        }
        for read in reads {
            chunks.extend(chunker.read(read)?);
        }
        chunks.extend(chunker.end());
    }
    chunks.extend(chunker.close());
    Ok(chunks)
}

/// `bytes` in reads of the given sizes, the rest in one last read.
fn reads(bytes: &[u8], sizes: &[usize]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut rest = bytes;
    for &size in sizes {
        let (read, after) = rest.split_at(size.min(rest.len()));
        out.push(read.to_vec());
        rest = after;
    }
    out.push(rest.to_vec());
    out
}

fn ramp(len: usize) -> Vec<u8> {
    (0..len)
        .map(|at| u8::try_from(at % 251).expect("below 251"))
        .collect()
}

/// A 16-bit PCM WAV of `pcm`, declaring `size` as its data size.
fn wav(pcm: &[u8], rate: u32, channels: u16, size: u32) -> Vec<u8> {
    let mut audio = voice::wav_container(pcm, rate, channels);
    audio.splice(40..44, size.to_le_bytes());
    audio
}

fn sequences(chunks: &[Chunk]) -> Vec<u32> {
    chunks.iter().map(|chunk| chunk.sequence).collect()
}

#[test]
fn raw_pcm_is_cut_at_fixed_offsets_however_it_arrives() {
    let encoding = hello("audio/pcm");
    let answer = ramp(2 * CHUNK_BYTES + 12_000);
    let at_once = streamed(&encoding, &[vec![answer.clone()]]).expect("chunks");
    for sizes in [&[7, 1000, 30_000][..], &[1; 40], &[CHUNK_BYTES - 1, 2]] {
        assert_eq!(
            streamed(&encoding, &[reads(&answer, sizes)]).expect("chunks"),
            at_once,
            "the same answer is the same chunks"
        );
    }
    let lengths: Vec<usize> = at_once.iter().map(|chunk| chunk.audio.len()).collect();
    assert_eq!(lengths, [CHUNK_BYTES, CHUNK_BYTES, 12_000]);
    assert_eq!(sequences(&at_once), [0, 1, 2]);
    assert_eq!(
        at_once
            .iter()
            .flat_map(|chunk| chunk.audio.clone())
            .collect::<Vec<_>>(),
        answer,
        "the chunks joined are the answer"
    );
}

#[test]
fn a_chunk_is_returned_as_soon_as_its_bytes_arrive() {
    let encoding = hello("audio/pcm");
    let mut chunker = Chunker::new(&encoding);
    assert!(chunker.open(None).is_empty());
    assert!(
        chunker
            .read(&ramp(CHUNK_BYTES - 1))
            .expect("read")
            .is_empty()
    );
    let first = chunker.read(&[0, 0, 0]).expect("read");
    assert_eq!(
        sequences(&first),
        [0],
        "the first chunk before the answer ends"
    );
    assert_eq!(first[0].audio.len(), CHUNK_BYTES);
    let last = chunker.end();
    assert_eq!(last.len(), 1, "the rest when it ends");
    assert_eq!(last[0].audio, [0, 0]);
    // A trailing half frame is not a chunk of its own.
    let mut chunker = Chunker::new(&encoding);
    let _ = chunker.open(None);
    assert!(chunker.read(&[1]).expect("read").is_empty());
    assert!(chunker.end().is_empty());
}

#[test]
fn a_wav_stream_is_chunked_after_its_header_in_wav_containers() {
    let encoding = hello("audio/wav");
    let pcm = ramp(4 * 22_050 * 2 / 2);
    for size in [0, u32::MAX, u32::try_from(pcm.len()).expect("small")] {
        let answer = wav(&pcm, 22_050, 2, size);
        let chunks = streamed(&encoding, &[reads(&answer, &[3, 20, 30, 5000])]).expect("chunks");
        let mut joined = Vec::new();
        for chunk in &chunks {
            let (rate, channels, piece) = voice::wav_16bit(&chunk.audio).expect("a 16-bit WAV");
            assert_eq!((rate, channels), (22_050, 2));
            assert!(piece.len() <= CHUNK_BYTES && piece.len() % 4 == 0);
            joined.extend(piece);
        }
        assert_eq!(joined, pcm, "the chunks' PCM is the answer's");
        assert_eq!(sequences(&chunks), [0, 1, 2, 3]);
    }
    // Bytes after a declared size are not audio.
    let mut answer = wav(&pcm, 22_050, 2, 400);
    answer.extend_from_slice(b"LIST\x04\0\0\0abcd");
    let chunks = streamed(&encoding, &[vec![answer]]).expect("chunks");
    assert_eq!(chunks.len(), 1);
    assert_eq!(
        voice::wav_16bit(&chunks[0].audio).expect("WAV").2,
        pcm[..400]
    );
}

#[test]
fn a_wav_that_is_not_16_bit_pcm_is_one_chunk_as_it_is() {
    let encoding = hello("audio/wav");
    let mut answer = wav(&ramp(800), 16_000, 1, 800);
    // 8-bit samples.
    answer[34] = 8;
    let chunks = streamed(&encoding, &[reads(&answer, &[10])]).expect("chunks");
    assert_eq!(
        chunks,
        [Chunk {
            sequence: 0,
            audio: answer
        }]
    );
    let truncated = b"RIFF\0\0".to_vec();
    let chunks = streamed(&encoding, &[vec![truncated.clone()]]).expect("chunks");
    assert_eq!(
        chunks,
        [Chunk {
            sequence: 0,
            audio: truncated
        }]
    );
}

#[test]
fn opaque_audio_is_cut_at_fixed_offsets() {
    for (media_type, provider) in [
        ("audio/mpeg", None),
        ("audio/mpeg", Some("elevenlabs")),
        ("audio/opus", None),
    ] {
        let encoding = encoding(&json!([{"words": "Hi."}]), media_type, provider);
        let answer = ramp(CHUNK_BYTES + 5);
        let chunks = streamed(&encoding, &[reads(&answer, &[3, 9])]).expect("chunks");
        let lengths: Vec<usize> = chunks.iter().map(|chunk| chunk.audio.len()).collect();
        assert_eq!(lengths, [CHUNK_BYTES, 5]);
        assert_eq!(chunks.concat_audio(), answer);
    }
}

trait Joined {
    fn concat_audio(&self) -> Vec<u8>;
}

impl Joined for Vec<Chunk> {
    fn concat_audio(&self) -> Vec<u8> {
        self.iter().flat_map(|chunk| chunk.audio.clone()).collect()
    }
}

#[test]
fn a_pause_is_a_chunk_of_silence_before_its_stretch() {
    let encoding = encoding(
        &json!([
            {"expression": {"pause": "long"}},
            {"words": "One,"},
            {"expression": {"pause": "short"}, "words": " two."},
            {"expression": {"pause": "short"}}
        ]),
        "audio/pcm",
        None,
    );
    let chunks = streamed(&encoding, &[vec![vec![1; 100]], vec![vec![2; 200]]]).expect("chunks");
    let shapes: Vec<(usize, bool)> = chunks
        .iter()
        .map(|chunk| (chunk.audio.len(), chunk.audio.iter().all(|&byte| byte == 0)))
        .collect();
    // 1 s of silence, "One,", 0.4 s, " two.", 0.4 s.
    assert_eq!(
        shapes,
        [
            (48_000, true),
            (100, false),
            (19_200, true),
            (200, false),
            (19_200, true)
        ]
    );
    assert_eq!(sequences(&chunks), [0, 1, 2, 3, 4]);
}

#[test]
fn a_wav_pause_waits_for_the_rate_its_answer_declares() {
    let encoding = encoding(
        &json!([{"expression": {"pause": "short"}}, {"words": "Hello."}]),
        "audio/wav",
        None,
    );
    let answer = wav(&[3; 640], 16_000, 1, 640);
    let chunks = streamed(&encoding, &[reads(&answer, &[30])]).expect("chunks");
    assert_eq!(chunks.len(), 2);
    let (rate, channels, silence) = voice::wav_16bit(&chunks[0].audio).expect("a WAV");
    assert_eq!((rate, channels, silence.len()), (16_000, 1, 12_800));
    assert!(
        silence.iter().all(|&byte| byte == 0),
        "the silence comes first"
    );
    assert_eq!(
        voice::wav_16bit(&chunks[1].audio).expect("a WAV").2,
        [3; 640]
    );

    // Stretches of one reply share one format.
    let encoding = encoding_with_two_stretches("audio/wav");
    let first = wav(&[0; 64], 16_000, 1, 64);
    let second = wav(&[0; 64], 22_050, 1, 64);
    assert_eq!(
        streamed(&encoding, &[vec![first], vec![second]]),
        Err(FailureCode::Malformed)
    );
}

fn encoding_with_two_stretches(media_type: &str) -> Encoding {
    encoding(
        &json!([{"words": "A"}, {"expression": {"pause": "short"}, "words": "B"}]),
        media_type,
        None,
    )
}

#[test]
fn gemini_answers_each_stretch_as_one_chunk() {
    let answer = |pcm: &[u8]| {
        json!({"steps": [{"type": "model_output", "content": [{"type": "audio", "data": voice::base64(pcm)}]}]})
            .to_string()
            .into_bytes()
    };
    let long = vec![5; 3 * CHUNK_BYTES];
    for media_type in ["audio/pcm", "audio/wav"] {
        let encoding = encoding(
            &json!([{"words": "A"}, {"expression": {"pause": "short"}, "words": "B"}]),
            media_type,
            Some("gemini"),
        );
        let chunks =
            streamed(&encoding, &[vec![answer(&long)], vec![answer(&[6; 48])]]).expect("chunks");
        let pcm: Vec<Vec<u8>> = chunks
            .iter()
            .map(|chunk| {
                if media_type == "audio/wav" {
                    voice::wav_16bit(&chunk.audio).expect("a WAV").2
                } else {
                    chunk.audio.clone()
                }
            })
            .collect();
        assert_eq!(pcm, [long.clone(), vec![0; 19_200], vec![6; 48]]);
    }
}

#[test]
fn a_chunk_is_named_after_the_output() {
    let named = |reference: &str, sequence| {
        let output = Media {
            reference: reference.into(),
            path: media_path(reference).expect("a reference"),
        };
        let media = chunk_media(&output, sequence).expect("a chunk reference");
        (media.reference, media.path)
    };
    assert_eq!(
        named("media/said.pcm", 0),
        ("media/said.0.pcm".into(), "/media/said.0.pcm".into())
    );
    assert_eq!(
        named("file:///media/a.b.wav", 7),
        ("file:///media/a.b.7.wav".into(), "/media/a.b.7.wav".into())
    );
    assert_eq!(
        named("media/said", 2),
        ("media/said.2".into(), "/media/said.2".into())
    );
    assert_eq!(
        named("/media/.hidden", 3),
        ("/media/.hidden.3".into(), "/media/.hidden.3".into())
    );
    assert_eq!(
        named("media.d/said", 1),
        ("media.d/said.1".into(), "/media.d/said.1".into())
    );
    assert_eq!(
        named("media/a%20b.pcm", 4),
        ("media/a%20b.4.pcm".into(), "/media/a b.4.pcm".into())
    );
}

#[test]
fn a_chunk_names_what_its_audio_says_and_satisfies_its_schema() {
    let schema: Json = serde_json::from_str(include_str!(
        "../../../../abilities/schemas/bot.ability.vocal.speech.encoding.chunk.json"
    ))
    .expect("the schema is JSON");
    let schema = CompiledSchema::new(&canonical_json(&schema)).expect("the schema compiles");
    let cases = [
        (
            hello("audio/pcm"),
            vec![0; 24_000],
            json!({"audio": "media/said.0.pcm", "channels": 1, "duration": 0.5, "media_type": "audio/pcm", "sample_rate_hz": 24_000, "sequence": 0}),
        ),
        (
            hello("audio/wav"),
            voice::wav_container(&[0; 3200], 16_000, 1),
            json!({"audio": "media/said.0.pcm", "channels": 1, "duration": 0.1, "media_type": "audio/wav", "sample_rate_hz": 16_000, "sequence": 0}),
        ),
        (
            hello("audio/mpeg"),
            b"ID3".to_vec(),
            json!({"audio": "media/said.0.pcm", "channels": null, "duration": null, "media_type": "audio/mpeg", "sample_rate_hz": null, "sequence": 0}),
        ),
        (
            encoding(&json!([{"words": "Hi."}]), "audio/mpeg", Some("elevenlabs")),
            b"ID3".to_vec(),
            json!({"audio": "media/said.0.pcm", "channels": null, "duration": null, "media_type": "audio/mpeg", "sample_rate_hz": 44_100, "sequence": 0}),
        ),
    ];
    for (encoding, audio, expected) in cases {
        let chunk = Chunk { sequence: 0, audio };
        let media = chunk_media(&encoding.output, chunk.sequence).expect("a chunk reference");
        let returned = chunk_returned(&encoding, &media, &chunk);
        assert_eq!(returned.name, CHUNK_EVENT);
        assert_eq!(
            serde_json::from_str::<Json>(&returned.data).expect("JSON"),
            expected
        );
        assert!(
            schema.admits(&Data::JsonObject(returned.data.clone())),
            "{}",
            returned.data
        );
    }
}
