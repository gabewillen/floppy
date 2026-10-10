---
name: floppy-phase6-speech-vision
description: Executes floppy port phase 6 (mosfet's hearing, listening, vocal, speaking audio, vision and reading) on OpenAI-compatible audio and vision APIs, one provable slice at a time, with the slices split so separate threads can build them in parallel.
project: floppy
repository: github.com/gabewillen/floppy
branch: one branch per slice thread, merged into claude/project-thread-brkerr and main when green
mosfet_source: github.com/stateforward/mosfet.py src/mosfet/abilities/{hearing,listening,vocal,speaking,vision,reading}, src/mosfet/abilities/communication/behaviors.py, src/providers/{openai_compat,elevenlabs,gemini,mlx_audio,mlx_vlm,pyannote}
design: mosfet's abilities, events, payload fields and state names unchanged; providers become invoked Wasm services speaking OpenAI-compatible HTTP (served from the Sparks); media by reference, never bytes
created: 2026-10-10
---

<!-- mdscript: use the mdscript-exec skill or read [spec.md](https://raw.githubusercontent.com/gabewillen/mdscript/main/spec.md) -->

## Rules For Every Slice

* the phase 3, 4 and 5 plans' rules hold: `.scx` for abilities and behaviors, no `<script>`, `event:schema` payload checks, conds only correlate, every cross-document event has a schema, 160-character `cond`/`expr` bound, I/O only in invoked services
* names are mosfet's (events, payload fields, ability and state names; mosfet = floppy); a floppy field is marked `[floppy field]` in its schema
* media moves by reference (phase 4 decision): mosfet's `bytes` fields (`SoundData.audio`, `VisualData.image`, `SpeechData.content` before decoding) are URI references to files in a directory the run grants; a service reads and writes those files, documents never hold media
* backends are chosen by the run, never by a document: each service reads its base URL and key from env slots, with no package default (D31); the Sparks serve the OpenAI-compatible endpoints; ElevenLabs is an optional speech-encoding backend
* models that only run locally in mosfet (MLX voice activity, diarization, voice identification, pyannote) have no OpenAI-compatible API: their stages are honest seams that log `absent` and answer as mosfet does when the stage is not configured (`speech_decoder=None` style), never fabricated
* devices (phone, LiveKit, audio capture and playout hardware) are out of scope
* each slice keeps every existing trace unchanged unless it says it changes them; commit atomically, push the slice branch, and say which commits to merge

## Map

| mosfet | floppy |
| --- | --- |
| `SpeechDecoder` providers (gemini, mlx_audio, moonshine) | `services/speech`, `decode`: `POST {base}/audio/transcriptions` (OpenAI-compatible, Whisper on the Sparks) |
| `SpeechEncoder` providers (gemini, elevenlabs, mlx_audio, moonshine) | `services/speech`, `encode`: `POST {base}/audio/speech` (OpenAI-compatible), or ElevenLabs `text-to-speech` when the run names it |
| `ImageDecoder` (mlx_vlm Qwen2-VL, "Read all visible text in this image. Return only the text.") | the generate service with an image content part (`image_url`, a `data:` URL the service builds from the referenced file) |
| `SpeechDecoding`, `SoundClassification`, `VoiceDetection`, `VoiceDiarization`, `VoiceIdentification` (hearing leaves, `idle` → `applying` → `idle`) | `abilities/hearing/*.scx`, each invoking its service or logging `absent` |
| `Listening` (`Perceiving{Listening, Sensing}`, `Sensitivity`, `Interpretation`) | `abilities/listening/listening.scx` and its children, the product `bot.ability.listening.speech.output` as the cognition's stimulus |
| `SpeechHeard` seed (`bot.behavior.speech_heard.input`/`.output`) | `abilities/behaviors/speech_heard.scx`, a turn behavior autonomy runs before the learned ones |
| `SpeechEncoding` (vocal) and `Speaking` (`idle` → `encoding` → `playing` → `recording`) | `abilities/vocal/vocal.scx` and `abilities/speaking/speaking.scx` in audio mode: playout is an `environment.sound` the body broadcasts, with the efference copy to listening |
| `VisualClassification`, `Reading` (`Unfocused`, `Focused{Classifying, DecodingText, DecodingImage, PreparingUnreadableOutput, EncodingOutput}`) | `abilities/vision/vision.scx`, `abilities/reading/reading.scx` |

## Slices

The slices below run in parallel. Each depends only on what is already on `main` plus the service contract the first two slices publish in their schemas; a slice that needs a service before it lands uses a test double of that service's events.

### Add Speech Service

* goal: mosfet's speech decoder and encoder as one invoked Wasm service on OpenAI-compatible audio APIs
* files: `services/speech/` (`Cargo.toml`, `src/lib.rs`, `src/tests.rs`, `package/speech.service.json`), `Cargo.toml`, `Cargo.lock`, `scripts/build-components.sh`, `tests/tests/speech_service.rs`
* `<param>`s: `operation` (`decode` or `encode`), `model`, `audio` (decode: a URI reference to a file in the granted media directory), `text` and `voice` (encode), `output` (encode: where to write the audio), `media_type`; env slots `baseurl`, `apikey` (the run binds them, e.g. to `FLOPPY_SPEECH_BASE_URL` and `FLOPPY_SPEECH_API_KEY`; no default)
* answers mosfet's `bot.ability.hearing.speech.decoding.output` `{text}` and `bot.ability.vocal.speech.encoding.output` `{audio, media_type, sample_rate_hz, channels}` (the audio as a reference), or `bot.ability.failed`
* ElevenLabs: the same `encode` with `provider` `elevenlabs` (its `text-to-speech/{voice_id}` endpoint and `xi-api-key`), only when the run names it
* tests: a fake backend on 127.0.0.1 for both endpoints and ElevenLabs; refusals for a missing grant, an unconfigured backend, and a malformed param

### Add Image Reading To Generate

* goal: the generate service accepts an image content part, so the vision model on the Sparks reads images as mosfet's `ImageDecoder` does
* files: `services/generate/src/lib.rs`, `src/tests.rs`, the generate validator, `services/generate/README.md`, `tests/tests/generate_service.rs`
* a `messages` entry's `content` may be an array of mosfet's parts (`text`, `image` with a URI reference); the service reads the referenced file from its granted media directory and sends an OpenAI `image_url` part with a `data:` URL
* tests: the fake backend sees the data URL; a reference outside the grant is refused

### Add Hearing And Listening

* goal: `environment.sound` becomes the cognition's stimulus `bot.ability.listening.speech.output` through mosfet's Listening and Interpretation
* files: `abilities/hearing/` (speech decoding, sound classification, voice detection, diarization, identification), `abilities/listening/listening.scx` (and its `sensitivity` and `interpretation` children), their schemas, the body's input abilities (fan-out C already sends `environment.sound` to them), tests
* sound classification is mosfet's `KindSoundClassifier` (the `kind` label, no model); voice detection, diarization and identification are seams; speech decoding invokes the speech service, so the product's `content_type` becomes `text/plain`
* labeled non-speech sound passes through as `environment.sound`, as mosfet does
* tests use a double of the speech service's decoding events until the speech service lands

### Add Speech Heard Seed

* goal: mosfet's `SpeechHeard` seed: autonomy runs the trusted seeds before the learned behaviors, and an admissible `bot.ability.listening.speech.output` (non-empty `source_ids`) becomes `bot.ability.communication.input`
* files: `abilities/behaviors/speech_heard.scx`, `abilities/autonomy/autonomy.scx` (seeds first, then the inventory, as `_prepare_match`), tests
* the seed's selection is mosfet's `SelectedEvent` (`event: bot.ability.communication.input`, `data: TurnData`)

### Add Vocal And Speaking Audio

* goal: speaking leaves text mode (D38): `encoding` invokes the vocal encoding (the speech service), `playing` broadcasts `environment.sound` with the encoded audio and sends listening the efference copy (`bot.ability.speaking.efference`), and `recording` records the text in the conversation, each with mosfet's 5 s bound
* files: `abilities/vocal/vocal.scx`, `abilities/speaking/speaking.scx`, `abilities/communication/communication.scx` if the efference route needs it, schemas, tests; `bot.say` stays as a log of the text
* tests use a double of the speech service's encoding events until it lands

### Add Vision And Reading

* goal: mosfet's `VisualClassification` (rule-based: `text` stays text, an image is `image`) and `Reading` (`bot.ability.reading.input` → `bot.ability.reading.output` `{text, source_kind, confidence}` or `bot.ability.reading.failed`), image decoding through the generate service's image parts
* files: `abilities/vision/vision.scx`, `abilities/reading/reading.scx`, schemas, tests; nothing maps `environment.visual` to reading (mosfet has no such route)

### Document Phase 6

* goal: decisions, port plan (phase 6 row, providers table), handoff and READMEs say what was built; D38 is superseded

## Decisions Made Under Delegation

* D50: one speech service holds both directions (mosfet's decoder and encoder providers are one OpenAI-compatible audio API), chosen by `operation`; ElevenLabs is a second encode backend selected by the run
* D51: image reading is the generate service with an image part, not a new service: the Sparks' vision model speaks Chat Completions
* D52: stages mosfet runs only on local models (voice activity, diarization, voice identification) are seams in phase 6
