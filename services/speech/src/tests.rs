use scxml_event_schema::CompiledSchema;

use super::*;

fn text(value: &str) -> Data {
    Data::String(value.into())
}

fn parts(value: &Json) -> Data {
    Data::canonical_json_array(&value.to_string()).expect("the parts are a JSON array")
}

fn hello() -> Data {
    parts(&serde_json::json!([{"words": "Hello there."}]))
}

fn parse(parameters: &[(&str, Data)]) -> Result<Operation, &'static str> {
    parse_parameters(parameters.iter().map(|(name, value)| (*name, value)))
}

fn decode() -> Operation {
    parse(&[
        ("operation", text("decode")),
        ("model", text("whisper-1")),
        ("audio", text("media/heard.wav")),
        ("media_type", text("audio/wav")),
    ])
    .expect("a valid decoding")
}

fn encode(extra: &[(&str, Data)]) -> Operation {
    let mut parameters = vec![
        ("operation", text("encode")),
        ("model", text("tts-1")),
        ("parts", hello()),
        ("voice", text("alloy")),
        ("output", text("/media/said.mp3")),
    ];
    parameters.extend(extra.iter().cloned());
    parse(&parameters).expect("a valid encoding")
}

fn data(returned: &Returned) -> Json {
    serde_json::from_str(&returned.data).expect("the data is JSON")
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "builds the answered arm of a transport outcome"
)]
fn ok(body: &[u8]) -> Result<Response, TransportFailure> {
    Ok(Response {
        status: 200,
        body: body.to_vec(),
    })
}

/// A minimal 16-bit PCM WAV file of a tenth of a second: a `fmt ` chunk
/// after a padded odd-sized chunk, then its `data`.
fn wav(rate: u32, channels: u16) -> Vec<u8> {
    let data = usize::try_from(rate / 10).expect("small") * usize::from(channels) * 2;
    let mut audio = b"RIFF\0\0\0\0WAVE".to_vec();
    audio.extend_from_slice(b"LIST\x03\0\0\0abc\0");
    audio.extend_from_slice(b"fmt \x10\0\0\0\x01\0");
    audio.extend_from_slice(&channels.to_le_bytes());
    audio.extend_from_slice(&rate.to_le_bytes());
    audio.extend_from_slice(&[0, 0, 0, 0, 0, 0, 16, 0]);
    audio.extend_from_slice(b"data");
    audio.extend_from_slice(&u32::try_from(data).expect("small").to_le_bytes());
    audio.resize(audio.len() + data, 0);
    audio
}

#[test]
fn references_resolve_against_the_guest_root() {
    for reference in [
        "media/audio-1",
        "/media/audio-1",
        "file:///media/audio-1",
        "media/audio%2D1",
    ] {
        assert_eq!(
            media_path(reference).as_deref(),
            Ok("/media/audio-1"),
            "{reference}"
        );
    }
    assert_eq!(media_path("a%20b.wav").as_deref(), Ok("/a b.wav"));
    for reference in [
        "",
        "/",
        "media/",
        "media//a",
        "media/../etc/passwd",
        "./a",
        "https://example.com/a.wav",
        "data:audio/wav;base64,AAAA",
        "//host/a",
        "file://host/a",
        "a?b",
        "a#b",
        "a%2Fb",
        "a%00b",
        "a%zz",
        "a%ff",
        "a%2",
    ] {
        assert!(media_path(reference).is_err(), "{reference:?}");
    }
}

#[test]
fn the_parameters_select_one_direction() {
    let Operation::Decode(decoding) = decode() else {
        panic!("a decoding");
    };
    assert_eq!(decoding.model, "whisper-1");
    assert_eq!(decoding.audio.reference, "media/heard.wav");
    assert_eq!(decoding.audio.path, "/media/heard.wav");
    assert_eq!(decoding.media_type, "audio/wav");
    let Operation::Encode(encoding) = encode(&[]) else {
        panic!("an encoding");
    };
    assert_eq!(encoding.format.media_type, DEFAULT_MEDIA_TYPE);
    assert_eq!(encoding.provider, Provider::OpenaiCompat);
    assert_eq!(encoding.output.path, "/media/said.mp3");
    let Operation::Encode(elevenlabs) = encode(&[
        ("provider", text("elevenlabs")),
        ("media_type", text("audio/pcm")),
    ]) else {
        panic!("an encoding");
    };
    assert_eq!(elevenlabs.provider, Provider::Elevenlabs);
    assert_eq!(elevenlabs.format.elevenlabs, Some(("pcm_24000", 24_000)));
}

fn base(operation: &str) -> Vec<(&'static str, Data)> {
    vec![("operation", text(operation)), ("model", text("m"))]
}

fn with(operation: &str, extra: &[(&'static str, Data)]) -> Vec<(&'static str, Data)> {
    let mut parameters = base(operation);
    parameters.extend(extra.iter().cloned());
    parameters
}

fn refuses(cases: Vec<(Vec<(&'static str, Data)>, &'static str)>) {
    for (parameters, expected) in cases {
        assert_eq!(parse(&parameters), Err(expected), "{parameters:?}");
    }
}

#[test]
fn malformed_parameters_reject_the_start() {
    refuses(vec![
        (
            vec![("model", text("m"))],
            "speech requires an operation parameter",
        ),
        (
            base("transcribe"),
            "speech operation must be decode, encode, detect, diarize, identify, classify, normalize, append, or clip",
        ),
        (
            vec![("operation", text("decode")), ("audio", text("a"))],
            "speech requires a model parameter",
        ),
        (
            with("decode", &[("audio", text("a")), ("audio", text("b"))]),
            "speech takes each parameter once",
        ),
        (
            with("decode", &[("audio", text("a")), ("language", text("en"))]),
            "speech takes no parameter of that name",
        ),
        (base("decode"), "speech decode requires an audio parameter"),
        (
            with("decode", &[("audio", Data::Bool(true))]),
            "speech audio must be a file reference",
        ),
        (
            with("decode", &[("audio", text("../a"))]),
            "speech media must be a file reference in the granted directory",
        ),
        (
            with(
                "decode",
                &[
                    ("audio", text("a")),
                    ("media_type", text("audio/wav\r\nx: y")),
                ],
            ),
            "speech media_type must be a type/subtype",
        ),
        (
            with("decode", &[("audio", text("a")), ("voice", text("alloy"))]),
            "speech decode takes model, audio, media_type, and provider only",
        ),
        (
            with(
                "decode",
                &[("audio", text("a")), ("provider", text("elevenlabs"))],
            ),
            "speech decode provider must be gemini",
        ),
    ]);
}

#[test]
fn malformed_encoding_parameters_reject_the_start() {
    let encoding = [
        ("parts", hello()),
        ("voice", text("alloy")),
        ("output", text("o.mp3")),
    ];
    refuses(vec![
        (
            with("encode", &[("voice", text("alloy")), ("output", text("o"))]),
            "speech drafts need audio/pcm or audio/wav",
        ),
        (
            with(
                "encode",
                &[
                    ("text", text("hi")),
                    ("voice", text("alloy")),
                    ("output", text("o")),
                ],
            ),
            "speech takes no parameter of that name",
        ),
        (
            with(
                "encode",
                &[
                    ("parts", hello()),
                    ("voice", text("a/b")),
                    ("output", text("o")),
                ],
            ),
            "speech voice must match [A-Za-z0-9_.-]{1,128}",
        ),
        (
            with("encode", &[("parts", hello()), ("voice", text("alloy"))]),
            "speech encode requires an output parameter",
        ),
        (
            with(
                "encode",
                &[
                    encoding[0].clone(),
                    encoding[1].clone(),
                    encoding[2].clone(),
                    ("audio", text("a")),
                ],
            ),
            "speech encode takes no audio parameter",
        ),
        (
            with(
                "encode",
                &[
                    encoding[0].clone(),
                    encoding[1].clone(),
                    encoding[2].clone(),
                    ("provider", text("openai")),
                ],
            ),
            "speech encode provider must be elevenlabs or gemini",
        ),
        (
            with(
                "encode",
                &[
                    encoding[0].clone(),
                    encoding[1].clone(),
                    encoding[2].clone(),
                    ("provider", text("gemini")),
                ],
            ),
            "speech media_type is not one the backend encodes",
        ),
        (
            with(
                "encode",
                &[
                    encoding[0].clone(),
                    encoding[1].clone(),
                    encoding[2].clone(),
                    ("media_type", text("audio/ogg")),
                ],
            ),
            "speech media_type is not one the backend encodes",
        ),
        (
            with(
                "encode",
                &[
                    encoding[0].clone(),
                    encoding[1].clone(),
                    encoding[2].clone(),
                    ("provider", text("elevenlabs")),
                    ("media_type", text("audio/wav")),
                ],
            ),
            "speech media_type is not one the backend encodes",
        ),
    ]);
}

#[test]
fn a_decoding_is_a_multipart_transcription_request() {
    let Operation::Decode(decoding) = decode() else {
        panic!("a decoding");
    };
    let request = decoding_request(2, Some(1), &decoding, b"RIFF-audio");
    assert_eq!(
        request.url,
        [
            Part::Reference(2),
            Part::Literal(TRANSCRIPTIONS_PATH.into())
        ]
    );
    assert_eq!(
        request.headers[0],
        (
            "authorization".to_owned(),
            vec![Part::Literal("Bearer ".into()), Part::Reference(1)]
        )
    );
    assert_eq!(
        request.headers[1].1,
        [Part::Literal(
            "multipart/form-data; boundary=floppy-speech-00000000".into()
        )]
    );
    assert_eq!(
        String::from_utf8(request.body).expect("UTF-8"),
        "--floppy-speech-00000000\r\ncontent-disposition: form-data; name=\"model\"\r\n\r\nwhisper-1\r\n\
         --floppy-speech-00000000\r\ncontent-disposition: form-data; name=\"response_format\"\r\n\r\njson\r\n\
         --floppy-speech-00000000\r\ncontent-disposition: form-data; name=\"file\"; filename=\"heard.wav\"\r\ncontent-type: audio/wav\r\n\r\n\
         RIFF-audio\r\n--floppy-speech-00000000--\r\n"
    );
}

#[test]
fn the_boundary_never_occurs_in_the_audio() {
    let Operation::Decode(decoding) = decode() else {
        panic!("a decoding");
    };
    let request = decoding_request(2, Some(1), &decoding, b"..floppy-speech-00000000..");
    assert_eq!(
        request.headers[1].1,
        [Part::Literal(
            "multipart/form-data; boundary=floppy-speech-00000001".into()
        )]
    );
}

#[test]
fn an_encoding_is_a_speech_request() {
    let Operation::Encode(encoding) = encode(&[]) else {
        panic!("an encoding");
    };
    assert_eq!(encoding.said().collect::<Vec<_>>(), ["Hello there."]);
    let request = encoding_request(2, Some(1), &encoding, "Hello there.");
    assert_eq!(
        request.url,
        [Part::Reference(2), Part::Literal(SPEECH_PATH.into())]
    );
    assert_eq!(request.headers[0].0, "authorization");
    assert_eq!(
        String::from_utf8(request.body).expect("UTF-8"),
        r#"{"input":"Hello there.","model":"tts-1","response_format":"mp3","voice":"alloy"}"#
    );
}

#[test]
fn an_elevenlabs_encoding_is_its_text_to_speech_request() {
    let Operation::Encode(encoding) = encode(&[("provider", text("elevenlabs"))]) else {
        panic!("an encoding");
    };
    let request = encoding_request(2, Some(1), &encoding, "Hello there.");
    assert_eq!(
        request.url,
        [
            Part::Reference(2),
            Part::Literal("/text-to-speech/alloy/stream?output_format=mp3_44100_128".into())
        ]
    );
    assert_eq!(
        request.headers[0],
        ("xi-api-key".to_owned(), vec![Part::Reference(1)])
    );
    assert_eq!(
        String::from_utf8(request.body).expect("UTF-8"),
        r#"{"model_id":"tts-1","text":"Hello there."}"#
    );
}

#[test]
fn a_transcription_answers_the_decoded_text() {
    let operation = decode();
    let returned = decoding_outcome(&operation, ok(br#"{"text":" hello there "}"#));
    assert_eq!(returned.name, DECODING_OUTPUT_EVENT);
    assert_eq!(returned.data, r#"{"text":"hello there"}"#);
    for (answered, reason) in [
        (ok(b"not json"), "the backend's answer is malformed"),
        (ok(br#"{"words":[]}"#), "the backend's answer is malformed"),
        (
            Ok(Response {
                status: 401,
                body: Vec::new(),
            }),
            "the backend answered an error status",
        ),
        (
            Err(TransportFailure::Unavailable),
            "the backend is unreachable",
        ),
        (
            Err(TransportFailure::TooLarge),
            "the backend's answer is malformed",
        ),
    ] {
        let returned = decoding_outcome(&operation, answered);
        assert_eq!(returned.name, FAILED_EVENT);
        assert_eq!(
            data(&returned)["message"],
            format!("Speech decoding failed: {reason}.")
        );
    }
}

#[test]
fn encoded_audio_is_written_before_the_output_names_it() {
    let operation = encode(&[]);
    let mut written = None;
    let returned = encoding_outcome(&operation, [ok(b"ID3-mp3")], |path, audio| {
        written = Some((path.to_owned(), audio.to_vec()));
        Ok(())
    });
    assert_eq!(
        written,
        Some(("/media/said.mp3".to_owned(), b"ID3-mp3".to_vec()))
    );
    assert_eq!(returned.name, ENCODING_OUTPUT_EVENT);
    assert_eq!(
        returned.data,
        r#"{"audio":"/media/said.mp3","channels":null,"duration":null,"media_type":"audio/mpeg","sample_rate_hz":null}"#
    );
    let failed = encoding_outcome(&operation, [ok(b"ID3-mp3")], |_, _| {
        Err(std::io::Error::other("denied"))
    });
    assert_eq!(
        data(&failed)["message"],
        "Speech encoding failed: the output cannot be written."
    );
    let empty = encoding_outcome(&operation, [ok(b"")], |_, _| {
        panic!("nothing is written for an empty answer")
    });
    assert_eq!(
        data(&empty)["message"],
        "Speech encoding failed: the backend's answer is malformed."
    );
}

#[test]
fn the_rate_channels_and_duration_are_named_only_when_known() {
    let wav_encoding = encode(&[("media_type", text("audio/wav"))]);
    let returned = encoding_outcome(&wav_encoding, [ok(&wav(22_050, 2))], |_, _| Ok(()));
    assert_eq!(data(&returned)["sample_rate_hz"], 22_050);
    assert_eq!(data(&returned)["channels"], 2);
    assert_eq!(data(&returned)["duration"], 0.1);
    let unreadable = encoding_outcome(&wav_encoding, [ok(b"RIFF")], |_, _| Ok(()));
    assert_eq!(data(&unreadable)["sample_rate_hz"], Json::Null);
    assert_eq!(data(&unreadable)["duration"], Json::Null);
    // A streamed WAV's unknown data size counts the bytes present.
    let mut streamed = wav(24_000, 1);
    let at = streamed.len() - 4800 - 4;
    streamed[at..at + 4].copy_from_slice(&[0xff; 4]);
    assert_eq!(
        wav_sound(&streamed).and_then(|sound| sound.duration),
        Some(0.1)
    );
    // Raw PCM is 24 kHz mono 16-bit from both backends.
    for provider in [vec![("provider", text("elevenlabs"))], vec![]] {
        let mut parameters = vec![("media_type", text("audio/pcm"))];
        parameters.extend(provider);
        let pcm = encode(&parameters);
        let returned = encoding_outcome(&pcm, [ok(&[0; 4800])], |_, _| Ok(()));
        assert_eq!(data(&returned)["sample_rate_hz"], 24_000);
        assert_eq!(data(&returned)["channels"], 1);
        assert_eq!(data(&returned)["duration"], 0.1);
    }
    let elevenlabs = encode(&[("provider", text("elevenlabs"))]);
    let returned = encoding_outcome(&elevenlabs, [ok(b"ID3")], |_, _| Ok(()));
    assert_eq!(data(&returned)["sample_rate_hz"], 44_100);
    assert_eq!(data(&returned)["duration"], Json::Null);
}

#[test]
fn a_null_parameter_is_not_given() {
    assert_eq!(
        parse(&[
            ("operation", text("encode")),
            ("model", Data::Null),
            ("parts", hello()),
            ("voice", text("alloy")),
            ("output", text("o.pcm")),
        ]),
        Err("speech requires a model parameter")
    );
    assert_eq!(encode(&[("media_type", Data::Null)]), encode(&[]));
}

#[test]
fn audio_is_read_from_its_file_within_its_bound() {
    let dir = std::env::temp_dir().join(format!("floppy-speech-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let heard = dir.join("heard.wav");
    std::fs::write(&heard, b"RIFF").expect("the audio is written");
    let empty = dir.join("empty.wav");
    std::fs::write(&empty, b"").expect("the empty audio is written");
    let path = |file: &std::path::Path| file.to_str().expect("UTF-8").to_owned();
    assert_eq!(read_audio(&path(&heard)), Ok(b"RIFF".to_vec()));
    assert_eq!(read_audio(&path(&empty)), Err(FailureCode::Empty));
    assert_eq!(
        read_audio(&path(&dir.join("missing.wav"))),
        Err(FailureCode::Unreadable)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_event_satisfies_its_published_schema() {
    let compile = |text: &str| {
        let value: Json = serde_json::from_str(text).expect("the schema is JSON");
        CompiledSchema::new(&canonical_json(&value)).expect("the schema compiles")
    };
    let decoded = compile(include_str!(
        "../schemas/bot.ability.hearing.speech.decoding.output.json"
    ));
    let encoded = compile(include_str!(
        "../../../abilities/schemas/bot.ability.vocal.speech.encoding.output.json"
    ));
    let failed = compile(include_str!(
        "../../../abilities/schemas/bot.ability.failed.json"
    ));
    let returned = decoding_outcome(&decode(), ok(br#"{"text":""}"#));
    assert!(decoded.admits(&Data::JsonObject(returned.data)));
    let paused = encode_parts(
        &serde_json::json!([{"words": "A"}, {"expression": {"pause": "short"}, "words": "B"}]),
        "audio/wav",
    );
    for operation in [
        encode(&[]),
        encode(&[("media_type", text("audio/wav"))]),
        encode(&[("provider", text("elevenlabs"))]),
        paused,
    ] {
        let audio = wav(16_000, 1);
        let returned = encoding_outcome(&operation, [ok(&audio), ok(&audio)], |_, _| Ok(()));
        assert!(
            encoded.admits(&Data::JsonObject(returned.data.clone())),
            "{}",
            returned.data
        );
    }
    for operation in [decode(), encode(&[])] {
        for code in [
            FailureCode::Unconfigured,
            FailureCode::Unreadable,
            FailureCode::Empty,
            FailureCode::TooLarge,
            FailureCode::Unwritable,
            FailureCode::Transport,
            FailureCode::Status,
            FailureCode::Malformed,
        ] {
            assert!(failed.admits(&Data::JsonObject(Returned::failed(&operation, code).data)));
        }
    }
}

/// The base URL is required (no default backend), the key is not: every
/// request to a keyless backend is the keyed one without its key header
/// (decision D127).
#[test]
fn the_base_url_is_required_and_a_keyless_request_has_no_key_header() {
    let manifest = scxml_validator_core::ServicePackageManifest::from_json(include_bytes!(
        "../package/speech.service.json"
    ))
    .expect("the manifest parses");
    assert!(
        manifest.http_origins.is_empty(),
        "no default backend: the baseurl value supplies the one origin"
    );
    assert_eq!(endpoint(Some(2), Some(1)), Ok((2, Some(1))));
    assert_eq!(endpoint(Some(2), None), Ok((2, None)));
    for api_key in [Some(1), None] {
        assert_eq!(endpoint(None, api_key), Err(FailureCode::Unconfigured));
    }
    let (Operation::Decode(decoding), Operation::Decode(gemini_decoding)) =
        (decode(), gemini_decode())
    else {
        panic!("decodings");
    };
    let encodings: Vec<Encoding> = [
        encode(&[]),
        encode(&[("provider", text("elevenlabs"))]),
        gemini_encode("audio/wav"),
    ]
    .into_iter()
    .map(|operation| match operation {
        Operation::Encode(encoding) => encoding,
        _ => panic!("an encoding"),
    })
    .collect();
    let built = |api_key: Option<u32>| -> Vec<Request> {
        let mut requests = vec![
            decoding_request(2, api_key, &decoding, b"RIFF-audio"),
            decoding_request(2, api_key, &gemini_decoding, b"RIFF-audio"),
            voice::detection_request(2, api_key, "silero-vad", &[0.0, 0.5]),
            voice::diarization_request(2, api_key, "pyannote", "utterance.wav", b"RIFF"),
            voice::embedding_request(2, api_key, "wespeaker", b"RIFF"),
        ];
        requests.extend(
            encodings
                .iter()
                .map(|encoding| encoding_request(2, api_key, encoding, "hi")),
        );
        requests
    };
    for (keyed, keyless) in built(Some(1)).into_iter().zip(built(None)) {
        let key = ["authorization", "xi-api-key", "x-goog-api-key"];
        assert!(key.contains(&keyed.headers[0].0.as_str()), "{keyed:?}");
        assert!(
            keyless
                .headers
                .iter()
                .all(|(name, _)| !key.contains(&name.as_str())),
            "{keyless:?}"
        );
        assert_eq!(keyless.headers, keyed.headers[1..]);
        assert_eq!((keyless.url, keyless.body), (keyed.url, keyed.body));
    }
}

#[test]
fn the_package_manifest_binds_the_exact_uri_and_artifacts() {
    let manifest = scxml_validator_core::ServicePackageManifest::from_json(include_bytes!(
        "../package/speech.service.json"
    ))
    .expect("the manifest parses");
    manifest.verify(None).expect("the manifest verifies");
    assert_eq!(manifest.service_uri, SERVICE_URI);
    assert_eq!(
        SERVICE_URI,
        format!("https://github.com/stateforward/floppy/services/{SERVICE_NAME}")
    );
    assert_eq!(
        manifest.service_artifact,
        "floppy_service_speech.virtualized.wasm"
    );
    assert_eq!(manifest.env_slots, [API_KEY_SLOT, BASE_URL_SLOT]);
}

fn gemini_decode() -> Operation {
    parse(&[
        ("operation", text("decode")),
        ("model", text("gemini-3.5-flash")),
        ("audio", text("media/heard.wav")),
        ("provider", text("gemini")),
    ])
    .expect("a valid Gemini decoding")
}

fn gemini_encode(media_type: &str) -> Operation {
    parse(&[
        ("operation", text("encode")),
        ("model", text("gemini-3.1-flash-tts-preview")),
        ("parts", hello()),
        ("voice", text("Kore")),
        ("output", text("/media/said.pcm")),
        ("media_type", text(media_type)),
        ("provider", text("gemini")),
    ])
    .expect("a valid Gemini encoding")
}

fn json_body(request: &Request) -> Json {
    serde_json::from_slice(&request.body).expect("the body is JSON")
}

#[test]
fn a_gemini_decoding_is_an_interaction_with_the_audio_inline() {
    let Operation::Decode(decoding) = gemini_decode() else {
        panic!("a decoding");
    };
    assert_eq!(decoding.media_type, GEMINI_AUDIO_MEDIA_TYPE);
    let request = decoding_request(2, Some(1), &decoding, b"RIFF-audio");
    assert_eq!(
        request.url,
        [
            Part::Reference(2),
            Part::Literal(gemini::INTERACTIONS_PATH.into())
        ]
    );
    assert_eq!(
        request.headers[0],
        ("x-goog-api-key".to_owned(), vec![Part::Reference(1)])
    );
    assert_eq!(
        json_body(&request),
        serde_json::json!({
            "input": [
                {"text": gemini::DECODING_PROMPT, "type": "text"},
                {"data": "UklGRi1hdWRpbw==", "mime_type": "audio/wav", "type": "audio"},
            ],
            "model": "gemini-3.5-flash",
            "store": false,
        })
    );
}

#[test]
fn a_gemini_encoding_is_an_interaction_answering_audio() {
    let Operation::Encode(encoding) = gemini_encode("audio/pcm") else {
        panic!("an encoding");
    };
    let request = encoding_request(2, Some(1), &encoding, "Hello there.");
    assert_eq!(
        request.url,
        [
            Part::Reference(2),
            Part::Literal(gemini::INTERACTIONS_PATH.into())
        ]
    );
    assert_eq!(
        json_body(&request),
        serde_json::json!({
            "generation_config": {"speech_config": [{"voice": "Kore"}]},
            "input": "Hello there.",
            "model": "gemini-3.1-flash-tts-preview",
            "response_format": {"type": "audio"},
            "store": false,
        })
    );
}

#[test]
fn a_gemini_transcript_is_the_text_of_its_model_output() {
    let operation = gemini_decode();
    for (body, expected) in [
        (
            br#"{"steps":[{"type":"thought","signature":"x"},{"type":"model_output","content":[{"type":"text","text":" Hello there! "}]}]}"#.as_slice(),
            r#"{"text":"Hello there!"}"#,
        ),
        (br#"{"output_text":"hi","steps":[]}"#.as_slice(), r#"{"text":"hi"}"#),
    ] {
        let returned = decoding_outcome(&operation, ok(body));
        assert_eq!(returned.name, DECODING_OUTPUT_EVENT);
        assert_eq!(returned.data, expected);
    }
    let silent = decoding_outcome(
        &operation,
        ok(br#"{"steps":[{"type":"model_output","content":[]}]}"#),
    );
    assert_eq!(silent.data, r#"{"text":""}"#);
    for body in [b"not json".as_slice(), br#"{"steps":[{"type":"thought"}]}"#] {
        assert_eq!(decoding_outcome(&operation, ok(body)).name, FAILED_EVENT);
    }
}

#[test]
fn gemini_audio_is_written_raw_or_in_a_wav_container() {
    // Two 16-bit samples, base64 of [1, 0, 2, 0].
    let answer = br#"{"steps":[{"type":"model_output","content":[{"type":"audio","data":"AQACAA==","sample_rate":24000,"channels":1,"mime_type":"audio/l16; rate=24000; channels=1"}]}]}"#;
    let mut written = None;
    let returned = encoding_outcome(&gemini_encode("audio/pcm"), [ok(answer)], |path, audio| {
        written = Some((path.to_owned(), audio.to_vec()));
        Ok(())
    });
    assert_eq!(
        written,
        Some(("/media/said.pcm".to_owned(), vec![1, 0, 2, 0]))
    );
    assert_eq!(data(&returned)["sample_rate_hz"], 24_000);
    assert_eq!(data(&returned)["channels"], 1);

    let mut wav = Vec::new();
    let returned = encoding_outcome(&gemini_encode("audio/wav"), [ok(answer)], |_, audio| {
        wav = audio.to_vec();
        Ok(())
    });
    assert_eq!(returned.name, ENCODING_OUTPUT_EVENT);
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(&wav[wav.len() - 4..], &[1, 0, 2, 0]);
    assert_eq!(data(&returned)["media_type"], "audio/wav");

    for body in [
        br#"{"steps":[]}"#.as_slice(),
        br#"{"steps":[{"content":[{"type":"audio","data":"AQACAA==","sample_rate":16000}]}]}"#,
        br#"{"steps":[{"content":[{"type":"audio","data":"not*base64"}]}]}"#,
    ] {
        let returned = encoding_outcome(&gemini_encode("audio/pcm"), [ok(body)], |_, _| Ok(()));
        assert_eq!(
            returned.name,
            FAILED_EVENT,
            "{}",
            String::from_utf8_lossy(body)
        );
    }
}

#[test]
fn base64_round_trips() {
    for bytes in [&b""[..], b"a", b"ab", b"abc", b"RIFF-audio"] {
        assert_eq!(
            gemini::base64_decoded(&voice::base64(bytes)).as_deref(),
            Some(bytes)
        );
    }
    assert_eq!(gemini::base64_decoded("A"), None);
}

fn encode_parts(value: &Json, media_type: &str) -> Operation {
    parse(&[
        ("operation", text("encode")),
        ("model", text("tts-1")),
        ("parts", parts(value)),
        ("voice", text("alloy")),
        ("output", text("/media/said")),
        ("media_type", text(media_type)),
    ])
    .expect("a valid encoding")
}

fn stretches_of(operation: &Operation) -> &[Stretch] {
    let Operation::Encode(encoding) = operation else {
        panic!("an encoding");
    };
    &encoding.stretches
}

fn stretch(pause: Option<Pause>, words: &str) -> Stretch {
    Stretch {
        pause,
        words: words.into(),
    }
}

#[test]
fn malformed_parts_reject_the_start() {
    use serde_json::json;
    let refused = |value: Data, expected: &'static str| {
        assert_eq!(
            parse(&with(
                "encode",
                &[
                    ("parts", value.clone()),
                    ("voice", text("alloy")),
                    ("output", text("o.pcm")),
                    ("media_type", text("audio/pcm")),
                ],
            )),
            Err(expected),
            "{value:?}"
        );
    };
    let shape = "speech parts must be 1 to 64 parts of {expression?, words?}";
    let unspoken = "speech parts' words must not be blank or hold * < > [ ] { } ( )";
    let expression = "speech parts' expression must be an object of Expression fields";
    refused(text("Hello."), shape);
    refused(parts(&json!([])), shape);
    refused(
        parts(&json!(vec![json!({"words": "a"}); MAX_PARTS + 1])),
        shape,
    );
    refused(parts(&json!(["Hello."])), shape);
    refused(parts(&json!([{"words": "a", "text": "b"}])), shape);
    refused(parts(&json!([{"words": " \n"}])), unspoken);
    refused(parts(&json!([{"words": ""}])), unspoken);
    refused(parts(&json!([{"words": 1}])), unspoken);
    for marked in ["*waves*", "<break/>", "[laugh]", "{x}", "(chuckle) hi"] {
        refused(parts(&json!([{"words": marked}])), unspoken);
    }
    refused(
        parts(&json!([{"words": "a".repeat(MAX_WORDS_CHARS + 1)}])),
        "speech parts' words must be at most 2000 characters",
    );
    refused(
        parts(&json!([{"expression": "warm", "words": "a"}])),
        expression,
    );
    refused(
        parts(&json!([{"expression": {"tone": "warm"}, "words": "a"}])),
        expression,
    );
    refused(
        parts(&json!([{"expression": {"pause": "medium"}, "words": "a"}])),
        "speech parts' pause must be short or long",
    );
    refused(
        parts(&json!([{"expression": {"sound": 1}, "words": "a"}])),
        "speech parts' sound must be a string",
    );
    refused(
        parts(&json!([{"words": "a"}, {"expression": {"emotion": "warm"}}])),
        "speech parts must each have words, a pause, or a sound",
    );
    refused(
        parts(&json!([{"words": "a"}, {}])),
        "speech parts must each have words, a pause, or a sound",
    );
    refused(
        parts(&json!([{"expression": {"pause": "short"}}, {"expression": {"sound": "sigh"}}])),
        "speech parts have no words",
    );
    // At the bound, null members are absent and other cues are read past.
    let at_bound = encode_parts(
        &json!([
            {"words": "a".repeat(MAX_WORDS_CHARS)},
            {"expression": {"sound": "laugh", "sound_at": "after"}},
            {"expression": {"instructions": "softly", "pace": 9, "pause": null, "sound": null}, "words": "b"},
        ]),
        "audio/mpeg",
    );
    assert_eq!(
        stretches_of(&at_bound),
        [stretch(None, &format!("{}b", "a".repeat(MAX_WORDS_CHARS)))]
    );
}

#[test]
fn a_pause_needs_pcm_or_wav() {
    let paused = serde_json::json!([{"words": "a"}, {"expression": {"pause": "long"}}]);
    for media_type in ["audio/mpeg", "audio/opus", "audio/flac"] {
        assert_eq!(
            parse(&[
                ("operation", text("encode")),
                ("model", text("tts-1")),
                ("parts", parts(&paused)),
                ("voice", text("alloy")),
                ("output", text("o")),
                ("media_type", text(media_type)),
            ]),
            Err("speech pauses need audio/pcm or audio/wav"),
        );
    }
    assert_eq!(
        parse(&[
            ("operation", text("encode")),
            ("model", text("tts-1")),
            ("parts", parts(&paused)),
            ("voice", text("alloy")),
            ("output", text("o")),
        ]),
        Err("speech pauses need audio/pcm or audio/wav"),
    );
    for media_type in ["audio/pcm", "audio/wav"] {
        let Operation::Encode(encoding) = encode_parts(&paused, media_type) else {
            panic!("an encoding");
        };
        assert!(encoding.pauses(), "{media_type}");
    }
}

#[test]
fn a_pause_opens_a_stretch_and_words_join_verbatim() {
    let operation = encode_parts(
        &serde_json::json!([
            {"words": "Hi, "},
            {"expression": {"emotion": "warm", "sound": "laugh"}, "words": "there."},
            {"expression": {"sound": "sigh"}},
            {"expression": {"pause": "short"}},
            {"words": " Then"},
            {"expression": {"pause": "long", "style": "whisper"}, "words": " more."},
        ]),
        "audio/pcm",
    );
    assert_eq!(
        stretches_of(&operation),
        [
            stretch(None, "Hi, there."),
            stretch(Some(Pause::Short), " Then"),
            stretch(Some(Pause::Long), " more."),
        ]
    );
    // A pause that opens the reply opens its first stretch, and a stretch
    // of no words holds only its pause.
    let operation = encode_parts(
        &serde_json::json!([
            {"expression": {"pause": "long"}},
            {"words": "A"},
            {"expression": {"pause": "short"}},
            {"expression": {"pause": "long"}, "words": "B"},
        ]),
        "audio/pcm",
    );
    assert_eq!(
        stretches_of(&operation),
        [
            stretch(Some(Pause::Long), "A"),
            stretch(Some(Pause::Short), ""),
            stretch(Some(Pause::Long), "B"),
        ]
    );
}

#[test]
fn each_stretch_with_words_is_one_request_saying_them() {
    let reply = serde_json::json!([
        {"words": "A"},
        {"expression": {"pause": "short"}},
        {"expression": {"pause": "long"}, "words": "B"},
    ]);
    for (provider, field) in [
        (None, "input"),
        (Some("elevenlabs"), "text"),
        (Some("gemini"), "input"),
    ] {
        let mut parameters = vec![
            ("operation", text("encode")),
            ("model", text("m")),
            ("parts", parts(&reply)),
            ("voice", text("v")),
            ("output", text("o")),
            ("media_type", text("audio/pcm")),
        ];
        if let Some(provider) = provider {
            parameters.push(("provider", text(provider)));
        }
        let Ok(Operation::Encode(encoding)) = parse(&parameters) else {
            panic!("an encoding");
        };
        let said: Vec<&str> = encoding.said().collect();
        assert_eq!(said, ["A", "B"], "{provider:?}");
        for words in said {
            let request = encoding_request(2, Some(1), &encoding, words);
            assert_eq!(json_body(&request)[field], words, "{provider:?}");
        }
    }
}

#[test]
fn a_pause_is_silence_of_its_length() {
    assert!((Pause::Short.seconds() - 0.4).abs() < f64::EPSILON);
    assert!((Pause::Long.seconds() - 1.0).abs() < f64::EPSILON);
    assert_eq!(Pause::Short.frames(PCM_SAMPLE_RATE_HZ), 9600);
    assert_eq!(Pause::Long.frames(PCM_SAMPLE_RATE_HZ), 24_000);
    assert_eq!(Pause::Short.frames(22_050), 8820);
    assert_eq!(Pause::Short.frames(11_025), 4410);

    let operation = encode_parts(
        &serde_json::json!([{"words": "A"}, {"expression": {"pause": "short"}, "words": "B"}]),
        "audio/pcm",
    );
    let mut written = None;
    let returned = encoding_outcome(
        &operation,
        [ok(&[1, 0]), ok(&[2, 0, 3, 0])],
        |path, audio| {
            written = Some((path.to_owned(), audio.to_vec()));
            Ok(())
        },
    );
    let mut expected = vec![1, 0];
    expected.resize(2 + 19_200, 0);
    expected.extend_from_slice(&[2, 0, 3, 0]);
    assert_eq!(written, Some(("/media/said".to_owned(), expected)));
    assert_eq!(returned.name, ENCODING_OUTPUT_EVENT);
    assert_eq!(data(&returned)["sample_rate_hz"], 24_000);
    assert_eq!(data(&returned)["channels"], 1);
    assert_eq!(data(&returned)["duration"], 19_206.0 / 48_000.0);
}

#[test]
fn a_pause_that_opens_the_reply_comes_first() {
    let reply = serde_json::json!([{"expression": {"pause": "long"}}, {"words": "A"}]);
    let mut written = Vec::new();
    let returned = encoding_outcome(
        &encode_parts(&reply, "audio/pcm"),
        [ok(&[7, 0])],
        |_, audio| {
            written = audio.to_vec();
            Ok(())
        },
    );
    assert_eq!(returned.name, ENCODING_OUTPUT_EVENT);
    assert_eq!(written.len(), 48_002);
    assert!(written[..48_000].iter().all(|&byte| byte == 0));
    assert_eq!(&written[48_000..], &[7, 0]);
    // Gemini's PCM is assembled the same way.
    let Operation::Encode(mut encoding) = encode_parts(&reply, "audio/pcm") else {
        panic!("an encoding");
    };
    encoding.provider = Provider::Gemini;
    let answer = br#"{"steps":[{"content":[{"type":"audio","data":"BwA="}]}]}"#;
    let mut gemini = Vec::new();
    let returned = encoding_outcome(&Operation::Encode(encoding), [ok(answer)], |_, audio| {
        gemini = audio.to_vec();
        Ok(())
    });
    assert_eq!(returned.name, ENCODING_OUTPUT_EVENT);
    assert_eq!(gemini, written);
}

#[test]
fn wav_stretches_are_joined_in_one_wav() {
    let operation = encode_parts(
        &serde_json::json!([{"words": "A"}, {"expression": {"pause": "short"}, "words": "B"}]),
        "audio/wav",
    );
    let mut written = Vec::new();
    let returned = encoding_outcome(
        &operation,
        [ok(&wav(16_000, 2)), ok(&wav(16_000, 2))],
        |_, audio| {
            written = audio.to_vec();
            Ok(())
        },
    );
    assert_eq!(returned.name, ENCODING_OUTPUT_EVENT);
    let (rate, channels, pcm) = voice::wav_16bit(&written).expect("a 16-bit WAV");
    assert_eq!((rate, channels), (16_000, 2));
    // A tenth of a second, 0.4 seconds of silence, a tenth of a second.
    assert_eq!(pcm.len(), 6400 + 6400 * 4 + 6400);
    assert_eq!(data(&returned)["sample_rate_hz"], 16_000);
    assert_eq!(data(&returned)["channels"], 2);
    assert_eq!(data(&returned)["duration"], 0.6);

    for (first, second) in [
        (wav(16_000, 1), wav(22_050, 1)),
        (wav(16_000, 1), wav(16_000, 2)),
        (wav(16_000, 1), b"RIFF".to_vec()),
    ] {
        let returned = encoding_outcome(&operation, [ok(&first), ok(&second)], |_, _| {
            panic!("nothing is written for stretches that disagree")
        });
        assert_eq!(
            data(&returned)["message"],
            "Speech encoding failed: the backend's answer is malformed."
        );
    }
}

#[test]
fn a_failed_stretch_ends_the_reply_and_writes_nothing() {
    let operation = encode_parts(
        &serde_json::json!([{"words": "A"}, {"expression": {"pause": "short"}, "words": "B"}]),
        "audio/pcm",
    );
    for (answered, reason) in [
        (
            vec![ok(&[1, 0]), Err(TransportFailure::Unavailable)],
            "the backend is unreachable",
        ),
        (
            vec![
                ok(&[1, 0]),
                Ok(Response {
                    status: 500,
                    body: Vec::new(),
                }),
            ],
            "the backend answered an error status",
        ),
        (
            vec![ok(&[1, 0]), ok(b"")],
            "the backend's answer is malformed",
        ),
        (vec![ok(&[1, 0])], "the backend's answer is malformed"),
    ] {
        let returned = encoding_outcome(&operation, answered, |_, _| {
            panic!("nothing is written for a failed stretch")
        });
        assert_eq!(
            data(&returned)["message"],
            format!("Speech encoding failed: {reason}.")
        );
    }
}

#[test]
fn an_encoding_without_parts_is_drafted_and_needs_linear_pcm() {
    let drafted = parse(&with(
        "encode",
        &[
            ("voice", text("alloy")),
            ("output", text("o.pcm")),
            ("media_type", text("audio/pcm")),
        ],
    ));
    let Ok(Operation::Encode(encoding)) = drafted else {
        panic!("a drafted encoding: {drafted:?}");
    };
    assert!(encoding.drafted && encoding.stretches.is_empty());
    let Ok(Operation::Encode(whole)) = parse(&with(
        "encode",
        &[
            ("parts", hello()),
            ("voice", text("alloy")),
            ("output", text("o.mp3")),
        ],
    )) else {
        panic!("an encoding");
    };
    assert!(!whole.drafted);
}

#[test]
fn a_reply_said_in_several_requests_is_joined_without_a_pause() {
    let Ok(Operation::Encode(encoding)) = parse(&with(
        "encode",
        &[
            ("voice", text("alloy")),
            ("output", text("o.pcm")),
            ("media_type", text("audio/pcm")),
        ],
    )) else {
        panic!("a drafted encoding");
    };
    let said = Operation::Encode(Encoding {
        stretches: vec![
            Stretch {
                pause: None,
                words: "One. ".into(),
            },
            Stretch {
                pause: None,
                words: "Two.".into(),
            },
        ],
        ..encoding
    });
    let answer = |bytes: &[u8]| {
        Ok(Response {
            status: 200,
            body: bytes.to_vec(),
        })
    };
    let mut written = Vec::new();
    let returned = encoding_outcome(
        &said,
        [answer(&[1, 0]), answer(&[2, 0, 3, 0])],
        |_, audio| {
            written = audio.to_vec();
            Ok(())
        },
    );
    assert_eq!(returned.name, ENCODING_OUTPUT_EVENT);
    assert_eq!(written, [1, 0, 2, 0, 3, 0]);
}
