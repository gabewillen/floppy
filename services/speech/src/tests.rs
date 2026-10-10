use scxml_event_schema::CompiledSchema;

use super::*;

fn text(value: &str) -> Data {
    Data::String(value.into())
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
        ("text", text("Hello there.")),
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
            "speech operation must be decode or encode",
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
            "speech decode takes model, audio, and media_type only",
        ),
    ]);
}

#[test]
fn malformed_encoding_parameters_reject_the_start() {
    let encoding = [
        ("text", text("hi")),
        ("voice", text("alloy")),
        ("output", text("o.mp3")),
    ];
    refuses(vec![
        (
            with("encode", &[("voice", text("alloy")), ("output", text("o"))]),
            "speech encode requires a text parameter",
        ),
        (
            with(
                "encode",
                &[
                    ("text", text(" ")),
                    ("voice", text("alloy")),
                    ("output", text("o")),
                ],
            ),
            "speech text must be a non-empty string",
        ),
        (
            with(
                "encode",
                &[
                    ("text", text("hi")),
                    ("voice", text("a/b")),
                    ("output", text("o")),
                ],
            ),
            "speech voice must match [A-Za-z0-9_.-]{1,128}",
        ),
        (
            with("encode", &[("text", text("hi")), ("voice", text("alloy"))]),
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
                    ("provider", text("gemini")),
                ],
            ),
            "speech provider must be elevenlabs",
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
    let request = decoding_request(2, 1, &decoding, b"RIFF-audio");
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
    let request = decoding_request(2, 1, &decoding, b"..floppy-speech-00000000..");
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
    let request = encoding_request(2, 1, &encoding);
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
    let request = encoding_request(2, 1, &encoding);
    assert_eq!(
        request.url,
        [
            Part::Reference(2),
            Part::Literal("/text-to-speech/alloy?output_format=mp3_44100_128".into())
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
    let returned = encoding_outcome(&operation, ok(b"ID3-mp3"), |path, audio| {
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
    let failed = encoding_outcome(&operation, ok(b"ID3-mp3"), |_, _| {
        Err(std::io::Error::other("denied"))
    });
    assert_eq!(
        data(&failed)["message"],
        "Speech encoding failed: the output cannot be written."
    );
    let empty = encoding_outcome(&operation, ok(b""), |_, _| {
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
    let returned = encoding_outcome(&wav_encoding, ok(&wav(22_050, 2)), |_, _| Ok(()));
    assert_eq!(data(&returned)["sample_rate_hz"], 22_050);
    assert_eq!(data(&returned)["channels"], 2);
    assert_eq!(data(&returned)["duration"], 0.1);
    let unreadable = encoding_outcome(&wav_encoding, ok(b"RIFF"), |_, _| Ok(()));
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
        let returned = encoding_outcome(&pcm, ok(&[0; 4800]), |_, _| Ok(()));
        assert_eq!(data(&returned)["sample_rate_hz"], 24_000);
        assert_eq!(data(&returned)["channels"], 1);
        assert_eq!(data(&returned)["duration"], 0.1);
    }
    let elevenlabs = encode(&[("provider", text("elevenlabs"))]);
    let returned = encoding_outcome(&elevenlabs, ok(b"ID3"), |_, _| Ok(()));
    assert_eq!(data(&returned)["sample_rate_hz"], 44_100);
    assert_eq!(data(&returned)["duration"], Json::Null);
}

#[test]
fn a_null_parameter_is_not_given() {
    assert_eq!(
        parse(&[
            ("operation", text("encode")),
            ("model", Data::Null),
            ("text", text("hi")),
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
        "../schemas/bot.ability.vocal.speech.encoding.output.json"
    ));
    let failed = compile(include_str!(
        "../../../abilities/schemas/bot.ability.failed.json"
    ));
    let returned = decoding_outcome(&decode(), ok(br#"{"text":""}"#));
    assert!(decoded.admits(&Data::JsonObject(returned.data)));
    for operation in [
        encode(&[]),
        encode(&[("media_type", text("audio/wav"))]),
        encode(&[("provider", text("elevenlabs"))]),
    ] {
        let audio = wav(16_000, 1);
        let returned = encoding_outcome(&operation, ok(&audio), |_, _| Ok(()));
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

#[test]
fn both_slots_are_required_and_the_package_declares_no_origin() {
    let manifest = scxml_validator_core::ServicePackageManifest::from_json(include_bytes!(
        "../package/speech.service.json"
    ))
    .expect("the manifest parses");
    assert!(
        manifest.http_origins.is_empty(),
        "no default backend: the baseurl value supplies the one origin"
    );
    assert_eq!(endpoint(Some(2), Some(1)), Ok((2, 1)));
    for (base_url, api_key) in [(None, Some(1)), (Some(2), None), (None, None)] {
        assert_eq!(endpoint(base_url, api_key), Err(FailureCode::Unconfigured));
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
