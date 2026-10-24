# Defaults

The defaults floppy ships, in one place. Each is a top-level `<data>` of the
document named, so a parent that invokes the document overrides it with a
`<param>` of the same name (SCXML 6.4); the run-time overrides are listed.
`tests/tests/defaults.rs` checks this page against the documents and
`.env.example`.

## Variables

[`.env.example`](../.env.example) names every variable a bot reads, without
values. No package has a default model endpoint (decision D31).

| Variable | Read by | Required |
| --- | --- | --- |
| `FLOPPY_BASE_URL` | the `generate` service (`env:baseurl`): the builder's design and author stages, the floppy bot's reasoning | yes: without it a generation fails `unconfigured` |
| `FLOPPY_API_KEY` | the `generate` service (`env:apikey`), as above | for a keyed backend: without it the request carries no key header, for a keyless one (decision D127) |
| `FLOPPY_STREAM` | the `generate` service (`env:stream`): whether the model endpoint streams; `true` or `false` | no: unset (the default), no call is streamed and each model-bound stage is bounded by its backstop alone; `true` makes reasoning's, reflection's and the builder's calls streamed and bounded by inactivity once they stream (decision D101). The CPA gateway does not stream (it answers a streamed request with keep-alive comments only), the maintainer's vLLM does |
| `TYPESAFE_BASE_URL` | the `decision` service (`env:baseurl`): the builder's triage, the floppy bot's intuition | no: the service's declared origin, `https://api.typesafe.ai`, when a key is held; without `TYPESAFE_API_KEY` a held base URL is asked keyless, with no authorization header (decision D150) |
| `TYPESAFE_API_KEY` | the `decision` service (`env:apikey`), as above | for the floppy bot, unless `TYPESAFE_BASE_URL` names a keyless endpoint: with neither, every turn fails (`unconfigured`); the builder triages with its fallback |
| `FLOPPY_SPEECH_BASE_URL` | the `speech` service (`env:baseurl`): the floppy bot's speech decoding (listening) and speech encoding (speaking) | yes: without it decoding and speaking fail `unconfigured` |
| `FLOPPY_SPEECH_API_KEY` | the `speech` service (`env:apikey`), as above | for a keyed backend, as `FLOPPY_API_KEY` |
| `FLOPPY_VOICE_BASE_URL` | the `speech` service (`env:baseurl`): the floppy bot's voice detection, diarization and voice identification (listening), on LocalAI's voice APIs | yes: without it every sound the bot hears fails voice detection |
| `FLOPPY_VOICE_API_KEY` | the `speech` service (`env:apikey`), as above | for a keyed backend, as `FLOPPY_API_KEY` |

The live tests read `FLOPPY_LIVE_ENV_FILE` (the env files, joined like
`PATH`) and `FLOPPY_LIVE_MODEL` (default: the model below); the trace tests
read `FLOPPY_TRACE_BLESS`. Those are test settings, not bot variables.

## Model

`Qwen/Qwen3.8-Flash-Next`, the model the maintainer's DGX Sparks serve
(direct vLLM; the CPA gateway lists it as `sparks/qwen3.8-flash-next`). It is
the `model` `<data>` of `bots/builder/builder.scx` (a `builder.request`'s
`model` overrides it), `bots/floppy/floppy.scx` (its environment's `model`
`<param>` overrides it), `abilities/cognition/cognition.scx`,
`abilities/reasoning/reasoning.scx` and `abilities/reflection/reflection.scx`
(each takes its invoker's by `<param>`), `abilities/reading/reading.scx`
(the image decoder), and `abilities/seeing/seeing.scx`, which passes its own
by `<param>` to `abilities/object_detection/object_detection.scx` and
`abilities/description/description.scx` (each carries the same `<data>`).

A bot composes the chat intuition tier only when its environment gives an
`intuition_model` `<param>` (none by default; decision D144): the cognition
then asks that chat model for the turn before reasoning.

## Speech and voice models

The listening's hearing leaves name the speech service's models:
`abilities/voice_detection/voice_detection.scx`'s `model` is `silero-vad`,
`abilities/voice_diarization/voice_diarization.scx`'s is
`pyannote-diarization`, and
`abilities/voice_identification/voice_identification.scx`'s is
`voice-detect-wespeaker-resnet34` (LocalAI's model names);
`abilities/speech_decoding/speech_decoding.scx`'s `provider` is `gemini` and
its `model` `gemini-3.5-flash`. `abilities/speaking/speaking.scx`'s
`provider` is `gemini`, its `model` `gemini-3.1-flash-tts-preview` and its
`voice` `Kore`, which it passes to vocal for the speech service (vocal's own
are `null`: its invoker names them). Both directions therefore call the
Gemini API, so `FLOPPY_SPEECH_BASE_URL` is its version root,
`https://generativelanguage.googleapis.com/v1beta` (decision D94); a
`provider` of `null` is the OpenAI-compatible API (`whisper-1`, `tts-1`).

## Timeouts and bounds

| Document | `<data>` | Default | What it bounds |
| --- | --- | --- | --- |
| `bots/builder/builder.scx` | `generate_timeout_ms` | `30000` | one design or author generation without progress, once its reply streams: the first progress arms it, each later one re-arms it (decision D101; a `builder.request`'s `generate_timeout_ms` overrides it) |
| `bots/builder/builder.scx` | `generate_ceiling_ms` | `1200000` | one design or author generation in all, the backstop, and the only bound of a reply that is not streamed (decision D101; a `builder.request`'s `generate_ceiling_ms` overrides it) |
| `bots/builder/builder.scx` | `max_completion_tokens` | `8192` | the tokens of one design or author reply, reasoning included: a model that reasons on without answering fails `truncated` (decision D101) |
| `bots/builder/builder.scx` | `decision_timeout_ms` | `60000` | one triage decision |
| `bots/builder/builder.scx` | `max_tests` | `8` | the test scripts one draft may carry |
| `bots/builder/builder.scx` | `sim_test_cap_ms` | `120000` | each test script's simulation run |
| `bots/floppy/floppy.scx` | `activation_timeout_ms` | `30000` | activation until every ability offers |
| `bots/floppy/floppy.scx` | `firmware_timeout_ms` | `30000` | the firmware's start until its first offers, once every ability is ready (decision D160) |
| `bots/floppy/floppy.scx` | `processing_timeout_ms` | `300000` | one turn |
| `bots/floppy/floppy.scx` | `reading_turns_timeout_ms` | `1000` | reading the short-term register's recent turns before each turn; the turn starts without them after it |
| `abilities/cognition/cognition.scx` | `child_operation_timeout_ms` | `30000` | each cascade stage without progress: autonomy's and the decision layer's from their start, reasoning's once its reply streams (the first progress arms it, each later one re-arms it); live streamed replies had at most 6.2 s between chunks (decision D101; the bot's `bounds` param's `child_operation_timeout_ms` overrides it) |
| `abilities/cognition/cognition.scx` | `child_operation_ceiling_ms` | `240000` | each cascade stage in all, the backstop and the only bound of a reasoning reply that is not streamed, inside the body's `processing_timeout_ms` with the stages before reasoning (decision D101; the bot's `bounds` param's `child_operation_ceiling_ms` overrides it) |
| `abilities/cognition/cognition.scx` | `rest_after_ms` | `300000` | idle with no input before the cognition rests and memory consolidates: the short-term register's recency window, 5 minutes, after which no exchange is recent (decisions D195 and D205; the bot's `bounds` param's `rest_after_ms` overrides it) |
| `abilities/consolidation/consolidation.scx` | `step_timeout_ms` | `30000` | each consolidation step's memory transaction (a read or a guarded write); one that does not answer in time ends the rest's consolidation, and the next rest begins again |
| `abilities/consolidation/consolidation.scx` | `step_pause_ms` | `1000` | the pause before each consolidation step, longer than a step takes, so the run takes input between two steps and input that leaves rest cancels the consolidation there |
| `abilities/consolidation/consolidation.scx` | `max_steps` | `256` | the writes one rest's consolidation makes at most |
| `abilities/cognition/cognition.scx` | `decision_timeout_ms` | `20000` | the decision layer's own bound on its decision, passed to it as its `decision_timeout_ms` param; a decision not made in time cascades the turn to reasoning; the bot's `bounds` param's `reflex_timeout_ms` is the reflex step's bound, which fails the turn |
| `abilities/intuition/intuition.scx` | `decision_timeout_ms` | `20000` | the decision layer's decision when its invoker passes no bound |
| `abilities/autonomy/autonomy.scx` | `silence_ms` | `1000` | a behavior's answer |
| `abilities/autonomy/autonomy.scx` | `memory_timeout_ms` | `10000` | one memory transaction |
| `abilities/candidates_runner/candidates_runner.scx` | `memory_timeout_ms` | `10000` | one candidate's memory transaction (autonomy passes its own) |
| `abilities/conversation/conversation.scx` | `memory_timeout_ms` | `5000` | each of the conversation's memory operations, the recall before the turns and the remember after them |
| `abilities/communication/communication.scx` | `response_timeout_ms` | `36000` | speaking going quiet on one response, re-armed by each of its progress events (speaking's 30 s synthesis bound plus 6 s; decisions D98, D179) |
| `abilities/communication/communication.scx` | `encoding_timeout_ms` | `30000` | speaking's speech synthesis (communication passes its own; decision D98) |
| `abilities/reflection/reflection.scx` | `select_timeout_ms` | `30000` | reflection's select generation without progress once its reply streams (decision D101) |
| `abilities/reflection/reflection.scx` | `select_ceiling_ms` | `600000` | reflection's select generation in all, the backstop (decision D101) |
| `abilities/reflection/reflection.scx` | `revising_timeout_ms` | `30000` | reflection's revising through the builder without progress while a builder generation streams: its first progress arms it, each later one re-arms it, and the builder's next stage disarms it (decision D101; the bot's `bounds` param's `revising_timeout_ms` overrides it) |
| `abilities/reflection/reflection.scx` | `revising_ceiling_ms` | `1800000` | reflection's revising in all, the backstop (live revisions on the maintainer's vLLM ran past 600 s while making progress; decision D101; the bot's `bounds` param's `revising_ceiling_ms` overrides it) |
| `abilities/identity/identity.scx` | `recall_attempt_timeout_ms` | `1000` | one attempt to recall the adopted name from memory, asked again after `recall_retry_delay_ms` |
| `abilities/identity/identity.scx` | `bring_up_timeout_ms` | `30000` | recalling the name until memory answers, then the identity wakes with the session's name and says so (the body passes half its `activation_timeout_ms`) |
| `abilities/persona/persona.scx` | `recall_attempt_timeout_ms` | `1000` | one attempt to recall the self-written description from memory, asked again after `recall_retry_delay_ms` |
| `abilities/persona/persona.scx` | `bring_up_timeout_ms` | `30000` | recalling the description until memory answers, then the persona wakes with none and says so (the body passes half its `activation_timeout_ms`) |
| `abilities/memory/memory.scx` | `operation_timeout_ms` | `60000` | one memory operation, its migration and transaction together, which then fails with kind `timeout` |
| `abilities/stm/stm.scx` | `open_timeout_ms` | `30000` | opening the short-term register (its database migrated), after which it is degraded |
| `abilities/reading/reading.scx` | `stage_timeout_ms` | `30000` | each reading stage |
| `abilities/conversation/conversation.scx` | `turn_timeout_ms` | `5000` | a turn detector's readiness, and then its turn, for each source of an input |
| `abilities/turn_detector/turn_detector.scx` | `end_of_turn_silence_seconds` | `0.5` | the silence that closes an open or paused turn (at least 0.01 s) |
| `abilities/speaking/speaking.scx` | `encoding_timeout_ms` | `30000` | one speech synthesis, until its output: playout starts on the first chunk (decision D203), but the bound still covers the whole synthesis, and Gemini answers each stretch only once it is synthesized (decision D98) |
| `abilities/speaking/speaking.scx` | `recording_timeout_ms` | `5000` | recording the spoken words in the conversation |
| `abilities/speaking/speaking.scx` | `lookahead_ms` | `1000` | the most playout commanded ahead of the mouth; each played piece is at most half of it and never past its chunk's end (decisions D179, D203) |
| `abilities/listening/listening.scx` | `sensitivity_timeout_ms` | `30000` | scoring one sound |
| `abilities/interpretation/interpretation.scx` | `stage_timeout_ms` | `30000` | each interpretation stage |
| `abilities/interpretation/interpretation.scx` | `recognize_timeout_ms` | `2000` | identification's extra time past the stage timeout (decision D177) |
| `abilities/interpretation/interpretation.scx` | `product_threshold_db` | `3.0` | the perceived level a listening product needs |
| `abilities/seeing/seeing.scx` | `measure_timeout_ms` | `2000` | one change measurement through the vision service |
| `abilities/seeing/seeing.scx` | `measure_every_ms` | `500` | the pace between measurements, whatever the frame rate |
| `abilities/seeing/seeing.scx` | `settle_after_ms` | `5000` | the settle after every sight: at most one sight per settle, plus looks |
| `abilities/seeing/seeing.scx` | `max_view_age_ms` | `60000` | how long a held view stays fresh |
| `abilities/seeing/seeing.scx` | `stage_timeout_ms` | `30000` | each object detection or description call |

The bot's `bounds` param may also bound a cognition tier's step on its own
([floppy fields]):
`reflex_timeout_ms` bounds the learned behaviors and the decision layer (a
step bound shorter than the decision layer's own `decision_timeout_ms` fails
the turn before the decision cascades), `intuition_timeout_ms` the chat tier,
and `reasoning_timeout_ms` reasoning, each as both the step's inactivity
bound and its backstop; a tier with none takes `child_operation_timeout_ms`
and `child_operation_ceiling_ms`. A step that runs out of time or fails fails
only its turn, which is handed back to the cognition as a
`bot.ability.cognition.step.timed_out` or `bot.ability.cognition.step.failed`
stimulus.

The bot's `bounds` param may also give reasoning's model context,
`max_context_tokens` (none by default: nothing is fitted), and the tokens
reserved for its reply, `completion_reserve_tokens` (0), [floppy fields]: each prompt is then
fitted to that context (decision D143).

## The floppy bot

The cognition always runs reflection and routines beside its turns. The
floppy bot's `database` is `/memory/floppy.sqlite`, so its runs grant the
`sql` service a memory directory; without a database reflection and routines
log `absent`. Its `traits`, which give it its persona, are `['warm',
'patient', 'curious', 'honest']`, and the floppy bot always has a persona; an
environment's `<invoke>` may give others as a `<param>`, one to 24 traits of
at most 200 characters each.
