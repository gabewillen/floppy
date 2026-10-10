# speech

The `speech` invoked service: one speech or voice operation per `<invoke>`,
as a Wasm component (`https://github.com/stateforward/floppy/services/speech`).
It is mosfet's `SpeechDecoder` and `SpeechEncoder` providers on one
OpenAI-compatible audio API (decision D57), and mosfet's local voice
classifiers on LocalAI's voice APIs (decision D66). Its crate documentation
([src/lib.rs](src/lib.rs), and [src/voice.rs](src/voice.rs) for the voice
operations) is the full contract: parameters, events, requests, bounds, and
telemetry.

## Operations

`<param name="operation">` picks one:

| Operation | mosfet | Request | Answer |
| --- | --- | --- | --- |
| `decode` | `SpeechDecoder` (hearing's `SpeechDecoding`) | `POST {base}/audio/transcriptions` | `bot.ability.hearing.speech.decoding.output {text}` |
| `encode` | `SpeechEncoder` (vocal's `SpeechEncoding`) | `POST {base}/audio/speech`, or with `provider` `elevenlabs` its `text-to-speech/{voice}` API | `bot.ability.vocal.speech.encoding.output {audio, media_type, sample_rate_hz, channels, duration}` |
| `detect` | `VoiceActivityClassifier` (`VoiceDetection`) | `POST {base}/vad` | `bot.ability.hearing.voice.detection.output` |
| `diarize` | `VoiceDiarizer` (`VoiceDiarization`) | `POST {base}/audio/diarization` | `bot.ability.hearing.voice.diarization.output` |
| `identify` | the voice identification `Classifier` (`VoiceIdentification`) | `POST {base}/voice/embed` per segment | `bot.ability.hearing.voice.identification.output` |
| `normalize`, `append`, `clip` | the interpretation's steps on PCM bytes | none: files in the granted directory | done data |

A failure answers `bot.ability.failed {message}` (mosfet's `FailureData`).
A malformed or missing parameter rejects the start: the `<invoke>` raises
`error.execution` and nothing is read, sent, or written. The event schemas
are in [schemas/](schemas/).

```xml
<invoke id="decoder" type="https://github.com/stateforward/floppy/services/speech"
        env:apikey="FLOPPY_SPEECH_API_KEY" env:baseurl="FLOPPY_SPEECH_BASE_URL">
  <param name="operation" expr="'decode'"/>
  <param name="model" expr="'whisper-1'"/>
  <param name="audio" expr="'media/hello.wav'"/>
</invoke>
```

## Backends

The package declares no `http-origins`, so there is no default backend
(decision D31): the operations that call one need the `baseurl` and
`apikey` slots bound to variables the run holds, else they answer
`bot.ability.failed` and send nothing. floppy's documents bind
`FLOPPY_SPEECH_BASE_URL` and `FLOPPY_SPEECH_API_KEY` for `decode` and
`encode`, and `FLOPPY_VOICE_BASE_URL` and `FLOPPY_VOICE_API_KEY` (a LocalAI
server) for `detect`, `diarize` and `identify`. The models are each
document's `model` `<data>`
([README Defaults](../../README.md#speech-and-voice-models)).

## Grants

Media moves by reference, never as bytes in a document: `audio` and
`output` are URI references to files in the directory the run grants, and
`media/hello.wav` names `/media/hello.wav`. The component imports
`wasi:filesystem`, so `scx run` refuses a run that binds it without a
`--dir` grant:

```console
mkdir -p media
scx run --env-file .env \
  --dir "https://github.com/stateforward/floppy/services/speech=$PWD/media::/media" \
  <document>
```

## Tests

`cargo test -p floppy-tests --test speech_service` runs the service against
a fake backend on 127.0.0.1; `-- --ignored live_speech_round_trip_through_an_env_file`
encodes and decodes a sentence on the backend `FLOPPY_LIVE_ENV_FILE` names.
The voice operations are tested in the crate ([src/voice/tests.rs](src/voice/tests.rs))
and through the listening (`--test listening`).
