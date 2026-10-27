use scxml_event_schema::CompiledSchema;

use super::super::{Operation, parse_parameters};
use super::*;

fn text(value: &str) -> Data {
    Data::String(value.into())
}

fn number(value: f64) -> Data {
    Data::Number(scxml_types::FiniteNumber::new(value).expect("finite"))
}

fn array(value: &Json) -> Data {
    Data::canonical_json_array(&value.to_string()).expect("an array")
}

fn parse(parameters: &[(&str, Data)]) -> Result<Operation, &'static str> {
    parse_parameters(parameters.iter().map(|(name, value)| (*name, value)))
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

fn data(returned: &Returned) -> Json {
    serde_json::from_str(&returned.data).expect("the data is JSON")
}

fn media(reference: &str) -> Media {
    Media {
        reference: reference.into(),
        path: super::super::media_path(reference).expect("a reference"),
    }
}

/// `samples` as 16-bit little-endian PCM.
fn pcm(samples: &[i16]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect()
}

fn detection_of(extra: &[(&str, Data)]) -> Detection {
    let mut parameters = vec![
        ("operation", text("detect")),
        ("model", text("silero-vad")),
        ("audio", text("media/heard.pcm")),
    ];
    parameters.extend(extra.iter().cloned());
    match parse(&parameters) {
        Ok(Operation::Detect(detection)) => detection,
        other => panic!("a detection: {other:?}"),
    }
}

#[test]
fn each_voice_operation_takes_its_parameters() {
    let detection = detection_of(&[
        ("media_type", text("audio/pcm")),
        ("sample_rate_hz", number(48_000.0)),
        ("channels", number(2.0)),
    ]);
    assert_eq!(detection.model, "silero-vad");
    assert_eq!(detection.audio.path, "/media/heard.pcm");
    assert_eq!(
        detection.format,
        Format {
            sample_rate_hz: Some(48_000),
            channels: Some(2)
        }
    );
    let Ok(Operation::Diarize(diarization)) = parse(&[
        ("operation", text("diarize")),
        ("model", text("pyannote-diarization")),
        ("audio", text("media/utterance.wav")),
        ("output", text("media/diarized")),
    ]) else {
        panic!("a diarization");
    };
    assert_eq!(diarization.output.path, "/media/diarized");
    let Ok(Operation::Identify(identification)) = parse(&[
        ("operation", text("identify")),
        ("model", text("voice-detect-wespeaker-resnet34")),
        (
            "segments",
            array(&json!([
                {"audio": "media/a", "media_type": "audio/pcm", "sample_rate_hz": 16000, "channels": 1, "start_seconds": 0.0, "end_seconds": 0.5},
                {"audio": "media/b", "media_type": "audio/pcm", "sample_rate_hz": 16000, "channels": 1, "start_seconds": 0.5, "end_seconds": 1.0, "confidence": null}
            ])),
        ),
    ]) else {
        panic!("an identification");
    };
    assert_eq!(identification.segments.len(), 2);
    assert_eq!(identification.segments[1].audio.path, "/media/b");
    let Ok(Operation::Append(appending)) = parse(&[
        ("operation", text("append")),
        (
            "frames",
            array(
                &json!([{"audio": "media/f1", "sample_rate_hz": 16000, "channels": 1}, {"audio": "media/f2", "sample_rate_hz": null, "channels": null}]),
            ),
        ),
        ("output", text("media/utterance")),
        ("wav", Data::Bool(true)),
    ]) else {
        panic!("an append");
    };
    assert!(appending.wav);
    assert_eq!(appending.frames[1].format, Format::default());
    assert!(matches!(
        parse(&[
            ("operation", text("clip")),
            ("audio", text("media/s")),
            ("sample_rate_hz", number(16_000.0)),
            ("channels", number(1.0)),
            ("start_seconds", number(0.0)),
            ("end_seconds", number(0.5)),
            ("output", text("media/c")),
        ]),
        Ok(Operation::Clip(_))
    ));
    assert!(matches!(
        parse(&[
            ("operation", text("normalize")),
            ("audio", text("media/s.wav")),
            ("media_type", text("audio/wav")),
            ("output", text("media/s.pcm")),
        ]),
        Ok(Operation::Normalize(_))
    ));
}

#[test]
#[expect(clippy::too_many_lines, reason = "one table of refusals")]
fn malformed_voice_parameters_reject_the_start() {
    let base = |operation: &str| vec![("operation", text(operation)), ("model", text("m"))];
    let with = |operation: &str, extra: Vec<(&'static str, Data)>| {
        let mut parameters = base(operation);
        parameters.extend(extra);
        parameters
    };
    for (parameters, expected) in [
        (base("detect"), "speech detect requires an audio parameter"),
        (
            with(
                "detect",
                vec![("audio", text("a")), ("sample_rate_hz", number(0.5))],
            ),
            "speech sample_rate_hz must be a positive integer",
        ),
        (
            with(
                "detect",
                vec![("audio", text("a")), ("channels", number(0.0))],
            ),
            "speech channels must be a positive integer",
        ),
        (
            with("detect", vec![("audio", text("a")), ("output", text("o"))]),
            "speech detect takes model, audio, media_type, sample_rate_hz, and channels only",
        ),
        (
            vec![("operation", text("detect")), ("audio", text("a"))],
            "speech requires a model parameter",
        ),
        (
            with("diarize", vec![("audio", text("a"))]),
            "speech diarize requires an output parameter",
        ),
        (
            base("identify"),
            "speech identify requires a segments parameter",
        ),
        (
            with("identify", vec![("segments", array(&json!([])))]),
            "speech segments must be voice segments of audio/pcm in ascending order",
        ),
        (
            with(
                "identify",
                vec![(
                    "segments",
                    array(&json!([
                        {"audio": "media/a", "media_type": "audio/pcm", "sample_rate_hz": 16000, "channels": 1, "start_seconds": 1.0, "end_seconds": 2.0},
                        {"audio": "media/b", "media_type": "audio/pcm", "sample_rate_hz": 16000, "channels": 1, "start_seconds": 0.0, "end_seconds": 0.5}
                    ])),
                )],
            ),
            "speech segments must be voice segments of audio/pcm in ascending order",
        ),
        (
            with(
                "identify",
                vec![(
                    "segments",
                    array(
                        &json!([{"audio": "media/a", "media_type": "audio/wav", "sample_rate_hz": 16000, "channels": 1, "start_seconds": 0.0, "end_seconds": 0.5}]),
                    ),
                )],
            ),
            "speech segments must be voice segments of audio/pcm in ascending order",
        ),
        (
            with(
                "normalize",
                vec![("audio", text("a")), ("output", text("o"))],
            ),
            "speech normalize takes no model parameter",
        ),
        (
            vec![
                ("operation", text("append")),
                ("frames", array(&json!([{"audio": "../a"}]))),
                ("output", text("o")),
            ],
            "speech media must be a file reference in the granted directory",
        ),
        (
            vec![
                ("operation", text("append")),
                ("frames", array(&json!([{"audio": "a"}]))),
                ("output", text("o")),
                ("wav", text("yes")),
            ],
            "speech wav must be a boolean",
        ),
        (
            vec![
                ("operation", text("clip")),
                ("audio", text("a")),
                ("sample_rate_hz", number(16_000.0)),
                ("channels", number(1.0)),
                ("start_seconds", number(-1.0)),
                ("end_seconds", number(1.0)),
                ("output", text("o")),
            ],
            "speech seconds must be non-negative numbers",
        ),
        (
            with(
                "decode",
                vec![("audio", text("a")), ("channels", number(1.0))],
            ),
            "speech takes that parameter only for a voice operation",
        ),
        (
            with(
                "detect",
                vec![("audio", text("a")), ("voice", text("alloy"))],
            ),
            "speech takes parts, voice, and provider only to encode",
        ),
    ] {
        assert_eq!(parse(&parameters), Err(expected), "{parameters:?}");
    }
}

#[test]
fn detection_sends_sixteen_kilohertz_mono_samples() {
    // Stereo 32 kHz: each frame averages its channels, then every other
    // frame is kept.
    let stereo = pcm(&[16_384, 0, -16_384, 0, 8_192, 8_192, 0, 0]);
    let samples = vad_samples(&stereo, 32_000, 2);
    assert_eq!(samples, vec![0.25, 0.25]);
    let samples = vad_samples(&pcm(&[0, 16_384, -32_768]), 16_000, 1);
    assert_eq!(samples, vec![0.0, 0.5, -1.0]);
    let request = detection_request(3, Some(4), "silero-vad", &samples);
    assert_eq!(
        request.url,
        vec![Part::Reference(3), Part::Literal("/vad".into())]
    );
    // The clip goes between half a second of silence on each side.
    let silence = vec!["0"; VAD_LEAD_IN_SAMPLES].join(",");
    assert_eq!(
        String::from_utf8(request.body).expect("UTF-8"),
        format!(r#"{{"audio":[{silence},0,0.5,-1,{silence}],"model":"silero-vad"}}"#)
    );
    assert_eq!(
        request.headers[0],
        (
            "authorization".to_owned(),
            vec![Part::Literal("Bearer ".into()), Part::Reference(4)]
        )
    );
}

#[test]
fn a_wav_clip_declares_its_own_format_and_raw_pcm_takes_the_default_format() {
    let detection = detection_of(&[]);
    let wav = wav_container(&pcm(&[1_000; 8_000]), 8_000, 1);
    let (samples, seconds) = detection_samples(&detection, wav).expect("a WAV clip");
    assert_eq!(samples.len(), 16_000);
    assert!((seconds - 1.0).abs() < 1e-9);
    let (samples, seconds) = detection_samples(&detection, pcm(&[0; 1_600])).expect("raw PCM");
    assert_eq!(samples.len(), 1_600);
    assert!((seconds - 0.1).abs() < 1e-9);
    let mut eight_bit = wav_container(&[0; 4], 8_000, 1);
    eight_bit[34] = 8;
    assert_eq!(
        detection_samples(&detection, eight_bit),
        Err(FailureCode::Format)
    );
    let stereo = detection_of(&[("channels", number(2.0))]);
    assert_eq!(
        detection_samples(&stereo, pcm(&[0, 0, 0])),
        Err(FailureCode::Format)
    );
}

#[test]
fn detected_spans_are_clamped_into_the_clip_and_ordered() {
    // The answer's spans are in the padded audio: half a second later than
    // the clip's.
    let returned = detection_outcome(
        "Voice detection",
        ok(br#"{"segments":[{"start":1.25,"end":3.0},{"start":0.375,"end":0.75},{"start":0.875,"end":0.875}]}"#),
        1.0,
    );
    assert_eq!(returned.name, DETECTION_OUTPUT_EVENT);
    assert_eq!(
        data(&returned),
        json!({"segments": [
            {"start_seconds": 0.0, "end_seconds": 0.25, "confidence": null},
            {"start_seconds": 0.75, "end_seconds": 1.0, "confidence": null}
        ]})
    );
    let silence = detection_outcome("Voice detection", ok(br#"{"segments":[]}"#), 1.0);
    assert_eq!(data(&silence), json!({"segments": []}));
    for answer in [
        ok(b"{}"),
        ok(br#"{"segments":[{"start":"0"}]}"#),
        ok(br#"{"segments":[{"start":0,"end":1,"confidence":2}]}"#),
    ] {
        let returned = detection_outcome("Voice detection", answer, 1.0);
        assert_eq!(
            data(&returned),
            json!({"message": "Voice detection failed: the backend's answer is malformed."})
        );
    }
    let refused = detection_outcome(
        "Voice detection",
        Ok(Response {
            status: 500,
            body: Vec::new(),
        }),
        1.0,
    );
    assert_eq!(
        data(&refused),
        json!({"message": "Voice detection failed: the backend answered an error status."})
    );
}

#[test]
fn diarization_uploads_a_wav_and_clips_each_segment() {
    let request = diarization_request(1, Some(2), "pyannote-diarization", "utterance.wav", b"RIFF");
    let body = String::from_utf8(request.body).expect("UTF-8");
    assert!(body.contains("name=\"model\"\r\n\r\npyannote-diarization\r\n"));
    assert!(body.contains("name=\"response_format\"\r\n\r\njson\r\n"));
    assert!(body.contains("filename=\"utterance.wav\"\r\ncontent-type: audio/wav\r\n\r\nRIFF\r\n"));
    assert_eq!(
        request.url,
        vec![
            Part::Reference(1),
            Part::Literal("/audio/diarization".into())
        ]
    );
    // One second at 4 Hz: frames 0..4.
    let utterance = pcm(&[1, 2, 3, 4]);
    let mut written = Vec::new();
    let mut write = |path: &str, bytes: &[u8]| {
        written.push((path.to_owned(), bytes.to_vec()));
        Ok(())
    };
    let returned = diarization_outcome(
        "Voice diarization",
        ok(br#"{"task":"diarize","segments":[{"id":1,"speaker":"SPEAKER_01","start":0.5,"end":1.0},{"id":0,"speaker":"SPEAKER_00","start":0.0,"end":0.4}]}"#),
        (&utterance, 4, 1),
        &media("media/diarized"),
        &mut write,
    );
    assert_eq!(returned.name, DIARIZATION_OUTPUT_EVENT);
    assert_eq!(
        data(&returned),
        json!({"segments": [
            {"audio": "media/diarized-0", "media_type": "audio/pcm", "sample_rate_hz": 4, "channels": 1, "start_seconds": 0.0, "end_seconds": 0.4, "confidence": null},
            {"audio": "media/diarized-1", "media_type": "audio/pcm", "sample_rate_hz": 4, "channels": 1, "start_seconds": 0.5, "end_seconds": 1.0, "confidence": null}
        ]})
    );
    assert_eq!(
        written,
        vec![
            ("/media/diarized-0".to_owned(), pcm(&[1, 2])),
            ("/media/diarized-1".to_owned(), pcm(&[3, 4])),
        ]
    );
    // One speaker's turns are one voice: joined, first start to last end.
    let mut written = Vec::new();
    let returned = diarization_outcome(
        "Voice diarization",
        ok(br#"{"segments":[{"speaker":"SPEAKER_00","start":0.0,"end":0.25,"confidence":0.9},{"speaker":"SPEAKER_01","start":0.25,"end":0.5},{"speaker":"SPEAKER_00","start":0.75,"end":1.0,"confidence":0.7}]}"#),
        (&utterance, 4, 1),
        &media("media/diarized"),
        &mut |path: &str, bytes: &[u8]| {
            written.push((path.to_owned(), bytes.to_vec()));
            Ok(())
        },
    );
    assert_eq!(
        data(&returned),
        json!({"segments": [
            {"audio": "media/diarized-0", "media_type": "audio/pcm", "sample_rate_hz": 4, "channels": 1, "start_seconds": 0.0, "end_seconds": 1.0, "confidence": 0.7},
            {"audio": "media/diarized-1", "media_type": "audio/pcm", "sample_rate_hz": 4, "channels": 1, "start_seconds": 0.25, "end_seconds": 0.5, "confidence": null}
        ]})
    );
    assert_eq!(
        written,
        vec![
            ("/media/diarized-0".to_owned(), pcm(&[1, 4])),
            ("/media/diarized-1".to_owned(), pcm(&[2])),
        ]
    );
    let returned = diarization_outcome(
        "Voice diarization",
        ok(br#"{"segments":[{"start":0.5,"end":1.08}]}"#),
        (&utterance, 4, 1),
        &media("media/d"),
        &mut |_, _| Ok(()),
    );
    assert_eq!(returned.name, DIARIZATION_OUTPUT_EVENT);
    assert_eq!(data(&returned)["segments"][0]["end_seconds"], json!(1.0));
    for answer in [
        ok(br#"{"segments":[]}"#),
        ok(br#"{"segments":[{"start":0.5,"end":1.5}]}"#),
        ok(br#"{"segments":[{"start":1.0,"end":1.05}]}"#),
    ] {
        let returned = diarization_outcome(
            "Voice diarization",
            answer,
            (&utterance, 4, 1),
            &media("media/d"),
            &mut |_, _| Ok(()),
        );
        assert_eq!(
            data(&returned),
            json!({"message": "Voice diarization failed: the backend's answer is malformed."})
        );
    }
}

#[test]
fn identification_embeds_each_segment() {
    assert_eq!(base64(b""), "");
    assert_eq!(base64(b"f"), "Zg==");
    assert_eq!(base64(b"fo"), "Zm8=");
    assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    let request = embedding_request(5, Some(6), "voice-detect-wespeaker-resnet34", b"RIFF");
    assert_eq!(
        request.url,
        vec![Part::Reference(5), Part::Literal("/voice/embed".into())]
    );
    assert_eq!(
        String::from_utf8(request.body).expect("UTF-8"),
        r#"{"audio":"UklGRg==","model":"voice-detect-wespeaker-resnet34"}"#
    );
    let embedding = embedding_outcome(
        ok(br#"{"embedding":[0.5,-0.25],"dim":2,"model":"sha256:abc"}"#),
        "voice-detect-wespeaker-resnet34",
    )
    .expect("an embedding");
    assert_eq!(
        embedding,
        json!({"embedding": [0.5, -0.25], "model": "voice-detect-wespeaker-resnet34", "confidence": null})
    );
    assert_eq!(
        embedding_outcome(ok(br#"{"embedding":[]}"#), "m"),
        Err(FailureCode::Malformed)
    );
    let returned = identification_returned(&[embedding]);
    assert_eq!(returned.name, IDENTIFICATION_OUTPUT_EVENT);
}

fn classification_of(extra: &[(&str, Data)]) -> Classification {
    let mut parameters = vec![
        ("operation", text("classify")),
        ("audio", text("media/heard.pcm")),
    ];
    parameters.extend(extra.iter().cloned());
    match parse(&parameters) {
        Ok(Operation::Classify(classification)) => classification,
        other => panic!("a classification: {other:?}"),
    }
}

#[test]
fn classification_takes_a_segment_and_no_model() {
    let classification = classification_of(&[
        ("media_type", text("audio/pcm")),
        ("sample_rate_hz", number(24_000.0)),
        ("channels", number(1.0)),
    ]);
    assert_eq!(classification.audio.path, "/media/heard.pcm");
    assert_eq!(
        classification.format,
        Format {
            sample_rate_hz: Some(24_000),
            channels: Some(1)
        }
    );
    for (parameters, refusal) in [
        (
            vec![
                ("operation", text("classify")),
                ("model", text("emotion2vec")),
                ("audio", text("media/a.pcm")),
            ],
            "speech classify takes no model parameter",
        ),
        (
            vec![("operation", text("classify"))],
            "speech classify requires an audio parameter",
        ),
        (
            vec![
                ("operation", text("classify")),
                ("audio", text("media/a.pcm")),
                ("output", text("media/b.pcm")),
            ],
            "speech classify takes audio, media_type, sample_rate_hz, and channels only",
        ),
    ] {
        assert_eq!(parse(&parameters).err(), Some(refusal));
    }
}

#[test]
fn classification_sends_mono_pcm_at_a_rate_the_server_takes() {
    let mono = classification_of(&[("sample_rate_hz", number(24_000.0))]);
    assert_eq!(
        affect_pcm(&mono, pcm(&[1, -2, 3])),
        Ok((24_000, pcm(&[1, -2, 3])))
    );
    let stereo = classification_of(&[
        ("sample_rate_hz", number(48_000.0)),
        ("channels", number(2.0)),
    ]);
    assert_eq!(
        affect_pcm(&stereo, pcm(&[100, 300, -50, -150])),
        Ok((48_000, pcm(&[200, -100])))
    );
    let other = classification_of(&[("sample_rate_hz", number(32_000.0))]);
    let (rate, resampled) = affect_pcm(&other, pcm(&[0, 0, 16_384, 16_384])).expect("resampled");
    assert_eq!(rate, VAD_SAMPLE_RATE_HZ);
    assert_eq!(resampled, pcm(&[0, 16_384]));
    let wav = classification_of(&[("media_type", text("audio/wav"))]);
    assert_eq!(
        affect_pcm(&wav, wav_container(&pcm(&[7, 8]), 16_000, 1)),
        Ok((16_000, pcm(&[7, 8])))
    );
    let unknown = classification_of(&[]);
    assert_eq!(affect_pcm(&unknown, pcm(&[1, 2])), Err(FailureCode::Format));
    assert_eq!(affect_pcm(&mono, Vec::new()), Err(FailureCode::Empty));
    let request = affect_request(5, Some(6), 24_000, pcm(&[1]));
    assert_eq!(
        request.url,
        vec![
            Part::Reference(5),
            Part::Literal("/emotion?sample_rate=24000".into())
        ]
    );
    assert_eq!(request.body, pcm(&[1]));
    assert_eq!(
        affect_request(5, None, 16_000, Vec::new())
            .headers
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["content-type", "accept"]
    );
}

#[test]
fn an_affect_is_the_answers_label_scores_valence_and_confidence() {
    let returned = affect_outcome(
        "Affect classification",
        ok(br#"{"label":"happy","scores":{"happy":0.9997,"sad":0.0},"valence":0.9997,"confidence":0.9997,"snr_db":15.8,"model":"emotion2vec/emotion2vec_plus_large","seconds":3.2}"#),
    );
    assert_eq!(returned.name, CLASSIFICATION_OUTPUT_EVENT);
    assert_eq!(
        data(&returned),
        json!({"label": "happy", "scores": {"happy": 0.9997, "sad": 0.0}, "valence": 0.9997, "confidence": 0.9997})
    );
    let bare = affect_outcome("Affect classification", ok(br#"{"label":"neutral"}"#));
    assert_eq!(
        data(&bare),
        json!({"label": "neutral", "scores": {}, "valence": null, "confidence": null})
    );
    for body in [
        &br#"{"scores":{}}"#[..],
        br#"{"label":""}"#,
        br#"{"label":"happy","scores":{"happy":1.5}}"#,
        br#"{"label":"happy","valence":-2}"#,
        br#"{"label":"happy","confidence":"high"}"#,
        b"not json",
    ] {
        assert_eq!(
            data(&affect_outcome("Affect classification", ok(body))),
            json!({"message": "Affect classification failed: the backend's answer is malformed."})
        );
    }
    assert_eq!(
        data(&affect_outcome(
            "Affect classification",
            Ok(Response {
                status: 422,
                body: br#"{"detail":"too short: 0.10 s"}"#.to_vec()
            })
        )),
        json!({"message": "Affect classification failed: the backend answered an error status."})
    );
}

fn normalization(media_type: Option<&str>, format: Format) -> Normalization {
    Normalization {
        audio: media("media/s.wav"),
        media_type: media_type.map(str::to_owned),
        format,
        output: media("media/s.pcm"),
    }
}

#[test]
fn normalization_unwraps_a_wav_container_into_raw_pcm_and_keeps_other_sound() {
    let wav = wav_container(&pcm(&[7, 8]), 8_000, 1);
    let mut written = None;
    let done = normalized(
        &normalization(Some("audio/wav"), Format::default()),
        |_| Ok(wav.clone()),
        |path, bytes| {
            written = Some((path.to_owned(), bytes.to_vec()));
            Ok(())
        },
    )
    .expect("normalized");
    assert_eq!(
        serde_json::from_str::<Json>(&done).expect("JSON"),
        json!({"audio": "media/s.pcm", "media_type": "audio/pcm", "sample_rate_hz": 8000, "channels": 1})
    );
    assert_eq!(written, Some(("/media/s.pcm".to_owned(), pcm(&[7, 8]))));
    let known = Format {
        sample_rate_hz: Some(16_000),
        channels: Some(1),
    };
    for (media_type, format, expected) in [
        (
            Some("audio/l16"),
            known,
            json!({"audio": "media/s.wav", "media_type": "audio/pcm", "sample_rate_hz": 16000, "channels": 1}),
        ),
        (
            None,
            known,
            json!({"audio": "media/s.wav", "media_type": "audio/pcm", "sample_rate_hz": 16000, "channels": 1}),
        ),
        (
            Some("audio/pcm"),
            Format::default(),
            json!({"audio": "media/s.wav", "media_type": "audio/pcm", "sample_rate_hz": null, "channels": null}),
        ),
        (
            Some("audio/mpeg"),
            known,
            json!({"audio": "media/s.wav", "media_type": "audio/mpeg", "sample_rate_hz": 16000, "channels": 1}),
        ),
        (
            Some("audio/raw"),
            Format::default(),
            json!({"audio": "media/s.wav", "media_type": "audio/raw", "sample_rate_hz": null, "channels": null}),
        ),
    ] {
        let done = normalized(
            &normalization(media_type, format),
            |_| panic!("only a WAV container is read"),
            |_, _| panic!("nothing is written"),
        )
        .expect("normalized");
        assert_eq!(serde_json::from_str::<Json>(&done).expect("JSON"), expected);
    }
    // A container that cannot be read leaves the sound unchanged.
    let done = normalized(
        &normalization(Some("audio/wav"), Format::default()),
        |_| Ok(b"not a wav".to_vec()),
        |_, _| panic!("nothing is written"),
    )
    .expect("normalized");
    assert_eq!(
        serde_json::from_str::<Json>(&done).expect("JSON"),
        json!({"audio": "media/s.wav", "media_type": "audio/wav", "sample_rate_hz": null, "channels": null})
    );
}

#[test]
fn open_speech_appends_every_frame_in_order() {
    let appending = Appending {
        frames: vec![
            Frame {
                audio: media("media/f1"),
                format: Format {
                    sample_rate_hz: None,
                    channels: Some(1),
                },
            },
            Frame {
                audio: media("media/f2"),
                format: Format {
                    sample_rate_hz: Some(8_000),
                    channels: Some(2),
                },
            },
        ],
        output: media("media/utterance"),
        wav: true,
    };
    let mut written = None;
    let done = appended(
        &appending,
        |path| {
            Ok(if path == "/media/f1" {
                pcm(&[1, 2])
            } else {
                pcm(&[3])
            })
        },
        |path, bytes| {
            written = Some((path.to_owned(), bytes.to_vec()));
            Ok(())
        },
    )
    .expect("appended");
    assert_eq!(
        serde_json::from_str::<Json>(&done).expect("JSON"),
        json!({"audio": "media/utterance", "media_type": "audio/wav", "sample_rate_hz": 8000, "channels": 1})
    );
    let (path, bytes) = written.expect("written");
    assert_eq!(path, "/media/utterance");
    assert_eq!(wav_16bit(&bytes), Ok((8_000, 1, pcm(&[1, 2, 3]))));
    let raw = Appending {
        wav: false,
        ..appending.clone()
    };
    let done = appended(
        &raw,
        |_| Ok(pcm(&[9])),
        |_, bytes| {
            assert_eq!(bytes, pcm(&[9, 9]));
            Ok(())
        },
    )
    .expect("appended");
    assert!(done.contains("\"audio/pcm\""));
    let unrated = Appending {
        frames: vec![appending.frames[0].clone()],
        ..appending
    };
    assert_eq!(
        appended(&unrated, |_| Ok(pcm(&[1])), |_, _| Ok(())),
        Err(FailureCode::Format)
    );
}

#[test]
fn a_clip_is_the_pcm_of_its_span_with_the_end_clamped_to_the_sound() {
    let clipping = Clipping {
        audio: media("media/s"),
        sample_rate_hz: 4,
        channels: 1,
        start_seconds: 0.3,
        end_seconds: 9.0,
        output: media("media/c"),
    };
    let mut written = None;
    let done = clipped(
        &clipping,
        |_| Ok(pcm(&[1, 2, 3, 4])),
        |path, bytes| {
            written = Some((path.to_owned(), bytes.to_vec()));
            Ok(())
        },
    )
    .expect("clipped");
    // floor(0.3 * 4) = 1 to the clamped end, one second.
    assert_eq!(
        serde_json::from_str::<Json>(&done).expect("JSON"),
        json!({"audio": "media/c", "media_type": "audio/pcm", "sample_rate_hz": 4, "channels": 1, "start_seconds": 0.3, "end_seconds": 1.0})
    );
    assert_eq!(written, Some(("/media/c".to_owned(), pcm(&[2, 3, 4]))));
    let late = Clipping {
        start_seconds: 2.0,
        ..clipping
    };
    assert_eq!(
        clipped(&late, |_| Ok(pcm(&[1, 2, 3, 4])), |_, _| Ok(())),
        Err(FailureCode::Span)
    );
}

#[test]
fn every_voice_event_satisfies_its_published_schema() {
    let compile = |text: &str| {
        let value: Json = serde_json::from_str(text).expect("the schema is JSON");
        CompiledSchema::new(&canonical_json(&value)).expect("the schema compiles")
    };
    let detected = compile(include_str!(
        "../../../../abilities/schemas/bot.ability.hearing.voice.detection.output.json"
    ));
    let diarized = compile(include_str!(
        "../../../../abilities/schemas/bot.ability.hearing.voice.diarization.output.json"
    ));
    let identified = compile(include_str!(
        "../../../../abilities/schemas/bot.ability.hearing.voice.identification.output.json"
    ));
    let returned = detection_outcome(
        "Voice detection",
        ok(br#"{"segments":[{"start":0.1,"end":0.2}]}"#),
        1.0,
    );
    assert!(detected.admits(&Data::JsonObject(returned.data)));
    let returned = diarization_outcome(
        "Voice diarization",
        ok(br#"{"segments":[{"start":0.0,"end":0.5}]}"#),
        (&pcm(&[1, 2, 3, 4]), 4, 1),
        &media("media/d"),
        &mut |_, _| Ok(()),
    );
    assert!(
        diarized.admits(&Data::JsonObject(returned.data.clone())),
        "{}",
        returned.data
    );
    let embedding = embedding_outcome(ok(br#"{"embedding":[0.5]}"#), "m").expect("an embedding");
    let returned = identification_returned(&[embedding]);
    assert!(identified.admits(&Data::JsonObject(returned.data)));
    let classified = compile(include_str!(
        "../../../../abilities/schemas/bot.ability.classifying.output.json"
    ));
    for body in [
        &br#"{"label":"sad","scores":{"sad":0.8,"neutral":0.2},"valence":-0.7,"confidence":0.8}"#[..],
        br#"{"label":"neutral"}"#,
    ] {
        let returned = affect_outcome("Affect classification", ok(body));
        assert!(
            classified.admits(&Data::JsonObject(returned.data.clone())),
            "{}",
            returned.data
        );
    }
}
