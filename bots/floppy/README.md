# floppy

The floppy bot is the body: an SCXML machine,
[`floppy.scx`](floppy.scx), whose abilities are invoked children under
[`../../abilities/`](../../abilities/). The body runs
its lifecycle and processing turns, and its abilities are the
[cognition](../../abilities/cognition/cognition.scx), the cascade of stages
invoked per turn: [autonomy](../../abilities/autonomy/autonomy.scx) (the
SpeechHeard seed, then the learned turn behaviors the stimulus triggers,
kept by memory), [intuition](../../abilities/intuition/intuition.scx) (the
decision service selects every event it can ground, each with its
confidence, gated by a self-tuning confidence tuner; low confidence
hands out the environment actions and goes on to reasoning), and
[reasoning](../../abilities/reasoning/reasoning.scx)
(one structured model call over the turn's choices, prompted with
model-facing XML; it recalls the
cognitive episodes reflection stored through memory), with
[reflection](../../abilities/reflection/reflection.scx) (after each output it
creates, changes or breaks a learned behavior through the builder bot and
stores the turn's one episode) and [routines](../../abilities/routines/routines.scx)
(the ACTIVE persistent behaviors, whose ticks become turns) running beside
the turns, and the [communication](../../abilities/communication/communication.scx),
which holds the [conversation](../../abilities/conversation/conversation.scx) (the
current exchange of committed messages, one relationship per participant set; each input
is one turn per source, assembled by a [turn detector](../../abilities/turn_detector/turn_detector.scx);
it recalls and remembers a relationship's memories, and matches each voice
embedding to a person long-term memory holds, `person-<n>`, or makes a new
one, so the bot knows a voice again across turns and restarts, decision D194) and
[speaking](../../abilities/speaking/speaking.scx) (the words are encoded to speech
by [vocal](../../abilities/vocal/vocal.scx) through the speech service, which
streams them in chunks, logged as `floppy.say`, played out chunk by chunk as
it arrives, each chunk an `environment.sound`, after the efference copy to
Listening, and recorded in the conversation; decision D203), and the
[listening](../../abilities/listening/listening.scx), its input ability
(phase 6: see Sound below). Each turn the body
collects the choices (its own `bot.focus_device`, which it takes only for
one of the turn's focus candidates, plus
what each ability offers, read from its live session that turn
(`<decision:offers>`, decision D242), and the focused device's
events, each tagged with the session id of its offerer as `target`, made
model-facing and held to 32 KiB, `<processing:collect_offered_events>`);
nothing the bot selects clears the focus: it stays on a device until the
bot focuses another (decision D155).
Each stage of the cognition sends its own selections,
each to the session its target names (`#_scxml_` + target); the schemas
the services check a selection against (the pinned focus device, each
target an enum of its offerers) refuse what the attention
policy does not allow, before anything is sent, and an ignore goes to the
cognition, which only logs it. A selected
`bot.ability.communication.respond` is said through speaking and recorded
in the conversation. Input addressed to the bot as
`bot.ability.communication.input` is admitted into the conversation, whose
updated exchange is the next turn's stimulus; so is a turn's selected
`bot.ability.communication.input` (the SpeechHeard seed admits heard
speech this way), which names no target, so the body sends it to itself. The full Contract, Topology, Bindings, and Proof sections come
with the phase's saved live examples.

## Running

The bot runs in an environment that invokes it (decision D102): the environment,
[`environments/environment/environment.scx`](../../environments/environment/environment.scx), the root
session, invokes it as its participant `floppy` and activates it on
joining (decision D107). Give the environment inputs on standard input; it routes
`bot.input` and `bot.deactivate` to every bot, and
`bot.ability.communication.input` to the participants its `target_ids`
names (every bot when it names none):

```console
# The locally built packages (the dev cache under target/).
printf '%s\n' \
  '{"name":"bot.input","data":{"observation":{"event":"text","data":"hello"}}}' |
  scx run --offline --cache-dir target/components/package-cache --env-file .env --dir https://github.com/stateforward/floppy/services/sql=memory::/memory --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" --wasi https://github.com/stateforward/floppy/services/pgsql=inherit-network \
    --dir https://github.com/stateforward/floppy/services/speech=media::/media --dir https://github.com/stateforward/floppy/services/files=behaviors::/behaviors \
    --dir https://github.com/stateforward/floppy/services/vision=media::/media \
    --fuel 9223372036854775807 environments/environment/environment.scx
# A conversation: text addressed to the bot, answered through floppy.say.
printf '%s\n' \
  '{"name":"bot.ability.communication.input","data":{"source_ids":["gabe"],"target_ids":["floppy"],"content":"Hi floppy!","content_type":"text/plain"}}' |
  scx run --offline --cache-dir target/components/package-cache --env-file .env --dir https://github.com/stateforward/floppy/services/sql=memory::/memory --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" --wasi https://github.com/stateforward/floppy/services/pgsql=inherit-network \
    --dir https://github.com/stateforward/floppy/services/speech=media::/media --dir https://github.com/stateforward/floppy/services/files=behaviors::/behaviors \
    --dir https://github.com/stateforward/floppy/services/vision=media::/media \
    --fuel 9223372036854775807 environments/environment/environment.scx
# floppy's latest release, once one is published (phase 7).
printf '%s\n' \
  '{"name":"bot.input","data":{"observation":{"event":"text","data":"hello"}}}' |
  scx run --env-file .env --dir https://github.com/stateforward/floppy/services/sql=memory::/memory --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" --wasi https://github.com/stateforward/floppy/services/pgsql=inherit-network \
    --dir https://github.com/stateforward/floppy/services/speech=media::/media --dir https://github.com/stateforward/floppy/services/files=behaviors::/behaviors \
    --dir https://github.com/stateforward/floppy/services/vision=media::/media \
    --fuel 9223372036854775807 environments/environment/environment.scx
```

The files service's grant is the `behaviors/` directory beside `bots/`
and `abilities/` (create it first; it is ignored by git), where the builder
reflection invokes writes a learned behavior's three documents and the
candidates and routines runners load it by its path (decision D121).

`--fuel 9223372036854775807` (2^63 - 1, the most fuel Wasmtime puts into a store at
once) gives every host call the fuel a bot's start needs. Entering the environment
starts the bot and all of its abilities, about 20 sessions, on one host
call's budget (scxml.rs docs/limits.md, "Fuel"), which two bots already
exceed at `scx`'s default of 3 billion. It stays this high until average
usage is measured (decision D111).

The env file must set `FLOPPY_BASE_URL` (the reasoning's `generate`
service has no default backend, decision D31; `FLOPPY_API_KEY` too unless
the backend is keyless, decision D127) and
`TYPESAFE_API_KEY` (the intuition's `decision` service; `TYPESAFE_BASE_URL`
is optional, `https://api.typesafe.ai` without it). Without the TypeSafe key
every turn fails with `the decision failed (unconfigured)`; without the model endpoint a turn that reaches reasoning
fails with `the reasoning failed (unconfigured)`.
The environment's `<invoke>` gives the bot its activation settings as
`<param>`s: `model` (the default is `gemma-4-26b-a4b`),
`reflection_model` (the model of reflection, learning and the builder, on
`FLOPPY_REFLECTION_BASE_URL`; the default is `sparks/qwen3.8-flash-next`,
null keeps them on `model`, decision D260),
`database`, `environment` (the environment's id) and `bounds`, an object
that may set the time bounds of the model-bound stages (decision D101),
each in milliseconds:
`child_operation_timeout_ms` (a cognition stage without progress; a
streamed reasoning reply's progress starts it over),
`child_operation_ceiling_ms` (a stage in all), `revising_timeout_ms`
(a revision through the builder without progress, reflection's or a
lesson's) and `revising_ceiling_ms` (a revision in all); absent, each is the ability's
default ([docs/defaults.md](../../docs/defaults.md#timeouts-and-bounds)). The memory ability
(`abilities/memory/memory.scx`) keeps the bot's database,
`/memory/floppy.sqlite`, through the sql
service (`services/sql`, SQLite), in the directory the run grants that
service: `--dir
https://github.com/stateforward/floppy/services/sql=HOST::/memory`
preopens the host directory `HOST` (here `memory/`, created first with
`mkdir -p memory`) at `/memory`, and the run is refused without it. The
memory ability also names the Postgres engine, the pgsql service
(`services/pgsql`, Postgres over its wire protocol), so the run grants it
the network too (`--wasi
https://github.com/stateforward/floppy/services/pgsql=inherit-network`),
though it is used only when the database is a Postgres connection URI
(`postgresql://USER@IP:PORT/DBNAME`, for example PGlite served by
`pglite-server` from `@electric-sql/pglite-socket`); the body's `database`
`<data>` names the engine, and Postgres support is tested on Postgres 16
([memory engines](../../docs/running.md#memory-engines)). The `generate` service
imports `wasi:filesystem` too (it reads the images a message names), so
the run grants it a media directory the same way (create `media/` first;
it is ignored by git).
The body invokes [seeing](../../abilities/seeing/seeing.scx), which
measures views through the `vision` service; that service imports
`wasi:filesystem` (it reads the images a view names), so every run grants
it the media directory too (`--dir
https://github.com/stateforward/floppy/services/vision=HOST::/media`),
whether or not seeing is on (see Sight below).
Speaking needs `FLOPPY_SPEECH_BASE_URL` (the speech service has no default
backend; `FLOPPY_SPEECH_API_KEY` too unless the backend is keyless) and the speech service granted the
media directory its audio is written to (`--dir
https://github.com/stateforward/floppy/services/speech=HOST::/media`);
without them a response fails with `bot.ability.communication.failed`.
[`.env.example`](../../.env.example) names the variables, and
[docs/defaults.md](../../docs/defaults.md) lists the model, the timeouts and the
bounds this bot ships with.

Sound (phase 6): `environment.sound` `{audio, media_type, sample_rate_hz,
channels, kind, ...}` goes to the [listening](../../abilities/listening/listening.scx),
the body's input ability, and speaking's efference copy
goes to it too, so the bot can tell its own voice apart. Its
[sensitivity](../../abilities/sensitivity/sensitivity.scx) scores the sound against the
body's prediction and its [interpretation](../../abilities/interpretation/interpretation.scx)
works out what it was: voice detection on every
sound, an open-speech window that joins voiced sounds into one utterance until
a sound without voice closes it, then (as configured in its data) diarization,
voice identification and speech decoding. floppy's default configuration is
voice detection, sound classification and speech decoding: a sound whose
`kind` labels it is non-speech and comes back as the turn's `environment.sound`
stimulus, an unlabelled sound without voice is dropped, and an utterance is
decoded by the speech service into the turn's
`bot.ability.listening.speech.output` stimulus (`content_type` `text/plain`).
`audio` is a reference to a file in the directory the run grants the speech
service (`media/hello.wav` is `hello.wav` there); voiced audio is signed
16-bit PCM, in a WAV container or raw with its rate and channels.
Voice detection, diarization and voice identification call
`FLOPPY_VOICE_BASE_URL` with `FLOPPY_VOICE_API_KEY` (LocalAI's `/vad`,
`/audio/diarization` and `/voice/embed`, with its `silero-vad`,
`pyannote-diarization` and `voice-detect-wespeaker-resnet34` models), and
speech decoding calls `FLOPPY_SPEECH_BASE_URL` with `FLOPPY_SPEECH_API_KEY`;
neither has a default backend. Beside decoding, the tone of the utterance's
longest voice is heard on `FLOPPY_EMOTION_BASE_URL` with
`FLOPPY_EMOTION_API_KEY` (a speech server's `/emotion`, emotion2vec+), and
the decoded stimulus carries it as `affect` `{label, scores, valence,
confidence}` (decision D247); without that backend it carries none. A sound that cannot be heard is logged as
`bot.ability.listening.failed`.

Memory recalls by association (decision D256): the Embedding ability
(`abilities/embedding/embedding.scx`) embeds each episode reflection stores
and each turn's cue reasoning recalls with, through
`FLOPPY_EMBEDDING_BASE_URL` with `FLOPPY_EMBEDDING_API_KEY` (an
`OpenAI`-compatible `/embeddings`, `embeddinggemma-2` at 256 dimensions,
which `https://api.willen.dev/v1` serves). Without it floppy recalls its
newest episodes, as before.

Sight (D185 to D189): the body's `seeing_enabled` `<data>` (`false` by
default; the environment's `<invoke>` may pass it as a `<param>`) turns the
[seeing](../../abilities/seeing/seeing.scx) ability on. On, every
`environment.visual` `{image, media_type, kind, source, participants}` goes
to it as well as to the input abilities: it holds the newest view of each
`source` (a view without one is dropped), measures how much it changed
since the last view of that source it handed on (the `vision` service,
`byte_identity` by default), and hands a first view, or one that changed by
at least 0.1, to the body as `bot.ability.cognition.input` whose stimulus
is `bot.ability.seeing.sight` `{cause, views, participants}`, which is
held and perceived like any handoff (D156); its offer `bot.ability.seeing.look` is among the turn's choices, and
a selected look hands on every fresh view. `image` is a reference to a file
in the directory the run grants the vision service (`media/frame.png` is
`frame.png` there). Object detection and description (its `detect` and
`describe` data) are off by default. Off, the body still invokes seeing,
but sends it no stimulus, does not take its offers and ignores its ending,
while activating, while the firmware starts and while active (decision
D189), so the bot behaves as before.

Each event the body emits goes to its environment, which writes it as a
`log` record labelled with its name, its text the event's data as JSON:
`bot.activated`, `bot.activating.failed`, `bot.deactivated`, `bot.reboot`,
`bot.processing.completed`, `bot.processing.failed`,
`bot.processing.timed_out`, `bot.processing.cancel.timed_out`,
`bot.degraded` (`{stage: "processing_cancel"}`, when an unacknowledged
cancel degrades it), and, from
the communication,
`bot.ability.speaking.output` (what was said),
`bot.ability.communication.failed`, `bot.ability.conversation.failed`
and `bot.ability.conversation.output`; the body logs `environment.sound`
(speaking's playout, by reference) itself. The bot's session ends in
`inactive` when its activation fails or it is deactivated, and in
`degraded` when it degrades; the environment logs each as `done.invoke`
with the bot's invoke id and its result. floppy's own
log records, which are not events, are labelled `floppy.*`:
`floppy.output` is the turn's selected events, speaking logs the words as
`floppy.say`, `floppy.choices` is each turn's choices (JSON, each tagged
with its offering session's id as `target`), `floppy.focus` each focus
change, `floppy.held` how many percepts are held after each arrival
during a turn, `floppy.released` what was still held when the bot left
active (`{count, reason}`),
`floppy.degraded` the reason the bot degraded, `floppy.stale` a reply to
an earlier turn, `floppy.diagnostic` why activation failed, and
`floppy.rejected` an event the body refused.

## Proof

- `cargo test -p floppy-tests --test body`: the lifecycle and turn matrix
  against a cognition double (`tests/fixtures/phase4/cognition_double.scx`),
  and the choices (the offers tagged with their sessions, focus grounding,
  the body's guards on its own events, ignore, the 32 KiB budget).
- `cargo test -p floppy-tests --test listening`: the listening under a
  harness, with the speech service decoding against a fake backend
  (`tests/tests/support/hearing.rs`): decoded speech, labelled non-speech,
  failures, the stage timer, the product threshold and order.
- `cargo test -p floppy-tests --test speech_service`: the speech service
  against a fake backend (decoding, encoding, ElevenLabs, a missing grant,
  an unconfigured or failing backend, malformed parameters).
- `cargo test -p floppy-tests --test seeing`: the body with seeing off
  (a visual reaches only the input abilities and starts no turn) and on (a
  visual with a source becomes a turn whose stimulus is
  `bot.ability.seeing.sight`, the look is offered); `FLOPPY_IRP_SUITE=seeing
  cargo test -p floppy-tests --test document_irp` runs seeing's own suite.
- `cargo test -p floppy-tests --test reading`: reading and vision under a
  harness, image decoding against a generate double (nothing in the bot
  invokes reading yet).
- `cargo test -p floppy-tests --test cognition`: the cascade against fake
  decision and model backends (each path, the tuner, failures, hop timeouts
  and reboot requests, cancels).
- `cargo test -p floppy-tests --test communication`: the communication
  path against the cognition double (a sent response said and recorded,
  conversation input as a turn whose stimulus carries its exchange,
  a blank response, a response deferred behind one in flight and the hop
  timeout, malformed input, a child that cannot start), and one response
  spoken through the real speech service on a fake backend, its efference
  copy reaching the listening before the sound.
- `cargo test -p floppy-tests --test cognition` also runs the whole text
  path on fake backends: "say hi" escalates to reasoning, whose activate
  and respond reasoning sends to the communication, which says it.
- `cargo test -p floppy-tests --test contracts`: every event reference and
  cross-document send of the body and its abilities.
- `FLOPPY_LIVE_ENV_FILE=<paths> cargo test -p floppy-tests --test
  floppy_live -- --ignored live_cascade`: one live question turn that
  escalates to reasoning, which sends its selection to its target.
- `... -- --ignored live_choices`: one live turn on a bot with a lamp: the
  choices are the body's focus (pinned to the lamp) and the cognition's and
  communication's offers, each tagged with its session id, and every
  selection is sent to its target.
- `... -- --ignored live_chat`: one live text turn through
  `bot.ability.communication.input`: the conversation's product is the
  stimulus, the reply is selected as `bot.ability.communication.respond`,
  said (`floppy.say`), and recorded in the conversation.
