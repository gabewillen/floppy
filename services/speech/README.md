# speech

The `speech` invoked service: one speech or voice operation per `<invoke>`,
as a Wasm component (`https://github.com/stateforward/floppy/services/speech`).
It decodes and encodes speech on one OpenAI-compatible audio API (decision
D57) or, with `provider` `gemini`, the Gemini API's Interactions API
(decision D94), and detects, diarizes and identifies voices on LocalAI's
voice APIs (decision D66). Its crate documentation
([src/lib.rs](src/lib.rs), and [src/voice.rs](src/voice.rs) for the voice
operations) is the full contract: parameters, events, requests, bounds, and
telemetry.

## Operations

`<param name="operation">` picks one:

| Operation | Invoked by | Request | Answer |
| --- | --- | --- | --- |
| `decode` | `speech_decoding` | `POST {base}/audio/transcriptions`, or with `provider` `gemini` `POST {base}/interactions` | `bot.ability.hearing.speech.decoding.output {text}` |
| `encode` | `vocal` | `POST {base}/audio/speech`, or with `provider` `elevenlabs` its streaming `text-to-speech/{voice}/stream` API, or with `gemini` `POST {base}/interactions` (`audio/pcm` or `audio/wav`) | each `bot.ability.vocal.speech.encoding.chunk {audio, sequence, media_type, sample_rate_hz, channels, duration}` as it is encoded, then `bot.ability.vocal.speech.encoding.output {audio, media_type, sample_rate_hz, channels, duration}` |
| `stream` | `transcription` | WebSocket `GET {base}/audio/stream?sample_rate=16000&language=L&chunk=C&turns=1`, held for the whole invocation | `bot.ability.hearing.speech.stream.ready`, then as the server hears them `voice_started`, `voice_ended`, `partial`, `transcript`, `turn_ended` and `overlap`; `failed {kind}` when it is lost |
| `detect` | `voice_detection` | `POST {base}/vad` | `bot.ability.hearing.voice.detection.output` |
| `diarize` | `voice_diarization` | `POST {base}/audio/diarization` | `bot.ability.hearing.voice.diarization.output` |
| `identify` | `voice_identification` | `POST {base}/voice/embed` per segment | `bot.ability.hearing.voice.identification.output` |
| `classify` | `voice_affect` | `POST {base}/emotion?sample_rate=R`, the segment's raw 16-bit mono PCM | `bot.ability.classifying.output {label, scores, valence, confidence}` |
| `normalize`, `append`, `clip` | `voice_detection` (`normalize`), `interpretation` (`append`), `voice_identification` (`clip`): steps on PCM | none: files in the granted directory | done data |

A failure answers `bot.ability.failed {message}`.
A malformed or missing parameter rejects the start: the `<invoke>` raises
`error.execution` and nothing is read, sent, or written. The event schemas
are in [schemas/](schemas/).

## Streamed encoding

`encode` reads each backend answer as it arrives (scxml.rs's
`outbound-http.send`) and returns its audio in chunks before the answer
has ended (decision D203), so speaking can start the first sound on the
first chunk:

- the OpenAI-compatible `/audio/speech` and ElevenLabs'
  `/text-to-speech/{voice}/stream` send their audio as it is synthesized;
  the service cuts it at fixed offsets of the answer, 24000 bytes at a
  time (half a second of 24 kHz mono 16-bit PCM), whatever the reads
  were: raw `pcm` as whole frames, `wav` as whole frames of its PCM, each
  chunk in a WAV container of its own (an answer that is not 16-bit PCM
  is one chunk, as it is), and any other format (`mp3`, `opus`, `aac`,
  `flac`) at byte offsets, the chunks joined being the answer;
- Gemini's Interactions API answers each stretch whole, so each stretch
  is one chunk;
- a pause is a chunk of its silence, before the stretch it opens.

Chunk `n` is a file of its own, the `output` reference with `.<n>`
before its extension (`media/speaking-1.pcm` is chunked as
`media/speaking-1.0.pcm`, `media/speaking-1.1.pcm`, …), written before
the chunk event names it; its sound fields are those its audio says, as
the output's are. The output is still the whole reply, written once
every answer has ended. A failure after some chunks answers
`bot.ability.failed`, and cancelling the invocation (leaving the
invoking state) closes the answer's connection mid-stream.

## Streamed hearing

`stream` is mosfet.py's `hearing.speech.stream` session, served as its
`speech_server` provider serves it, over scxml.rs's `outbound-websocket`:
the `baseurl` slot's `https` URL names its `wss` form, and the key goes as
`authorization: Bearer <apikey>`, a reference the component never reads.
The invoking session sends it `bot.ability.hearing.speech.stream.audio
{audio, media_type?, sample_rate_hz?, channels?, output?}` for each sound
heard (by reference; sent on as 16 kHz mono 16-bit PCM, also written to
`output` when given, held until the server is ready, at most 2 s of it) and `bot.ability.hearing.speech.stream.producing
{producing}` when the bot's own mouth starts or stops (sent on as
`{"type":"agent","speaking":B}` only on change). Each server frame is one
returned event; times are seconds of the audio the session sent. Closing
the session (leaving the invoking state) sends `{"type":"end"}` and closes
the connection. Losing it answers `failed {kind}`: `unavailable`,
`overloaded` (close 1013), `invalid_input` (1003) or `invalid_response`
(mosfet.py's kinds). The listening's transcription
(`abilities/transcription/transcription.scx`) invokes it.

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
(decision D31): the operations that call one need the `baseurl` slot
bound to a variable the run holds, else they answer `bot.ability.failed`
and send nothing. A run that holds a base URL and no key reaches a keyless
server: the request carries no key header at all (decision D127). floppy's documents bind
`FLOPPY_SPEECH_BASE_URL` and `FLOPPY_SPEECH_API_KEY` for `decode` and
`encode`, and `FLOPPY_VOICE_BASE_URL` and `FLOPPY_VOICE_API_KEY` (a LocalAI
server) for `detect`, `diarize` and `identify`, and `FLOPPY_EMOTION_BASE_URL`
and `FLOPPY_EMOTION_API_KEY` (a speech server's emotion API, emotion2vec+)
for `classify`. The models are each
document's `model` `<data>`
([Defaults](../../docs/defaults.md#speech-and-voice-models)).

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
a fake backend on 127.0.0.1 (a streamed answer among them, and one a
session cancels after its first chunk); the crate's
[src/chunks/tests.rs](src/chunks/tests.rs) cuts answers that arrive in
reads of every size; `-- --ignored live_speech_round_trip_through_an_env_file`
encodes and decodes a sentence on the backend `FLOPPY_LIVE_ENV_FILE` names.
The voice operations are tested in the crate ([src/voice/tests.rs](src/voice/tests.rs))
and through the listening (`--test listening`).
