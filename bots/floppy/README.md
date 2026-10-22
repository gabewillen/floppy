# floppy

The floppy bot is the port of the original's Bot: an SCXML machine,
[`floppy.scx`](floppy.scx), whose abilities are invoked children under
[`../../abilities/`](../../abilities/). The body runs
its lifecycle and processing turns, and its abilities are the
[cognition](../../abilities/cognition/cognition.scx), the original's cascade of stages
invoked per turn: [autonomy](../../abilities/autonomy/autonomy.scx) (the original's
SpeechHeard seed, then the learned turn behaviors the stimulus triggers,
kept by memory), [intuition](../../abilities/intuition/intuition.scx) (the
decision service selects every event it can ground, each with its
confidence, gated by the original's self-tuning confidence tuner; low confidence
hands out the environment actions and goes on to reasoning), and
[reasoning](../../abilities/reasoning/reasoning.scx)
(one structured model call over the turn's choices, prompted with the original's
model-facing XML; it recalls the
cognitive episodes reflection stored through memory), with
[reflection](../../abilities/reflection/reflection.scx) (after each output it
creates, changes or breaks a learned behavior through the builder bot and
stores the turn's one episode) and [routines](../../abilities/routines/routines.scx)
(the ACTIVE persistent behaviors, whose ticks become turns) running beside
the turns, and the [communication](../../abilities/communication/communication.scx),
which holds the [conversation](../../abilities/conversation/conversation.scx) (the
committed text history, one relationship per participant set; each input
is one turn per source, assembled by a [turn detector](../../abilities/turn_detector/turn_detector.scx);
it recalls and remembers a relationship's memories, and matching an
embedding source to an earlier participant's voice is not ported) and
[speaking](../../abilities/speaking/speaking.scx) (the words are encoded to speech
by [vocal](../../abilities/vocal/vocal.scx) through the speech service, logged as
`floppy.say`, played out as `environment.sound` after the efference copy to
Listening, and recorded in the conversation), and the
[listening](../../abilities/listening/listening.scx), its input ability
(the original's Listening, phase 6: see Sound below). Each turn the body
collects the choices (its own `bot.focus_device`, pinned to the turn's
focus candidates, and `bot.clear_focus` while a device is focused, plus
each ability's latest `bot.ability.offers` and the focused device's
events, each tagged with the session id of its offerer as `target`, made
model-facing and held to 32 KiB, `<processing:collect_offered_events>`).
As in the original, each stage of the cognition sends its own selections,
each to the session its target names (`#_scxml_` + target); the schemas
the services check a selection against (the pinned focus device, each
target an enum of its offerers) refuse what the original's attention
policy refused, before anything is sent, and an ignore goes to the
cognition, which only logs it. A selected
`bot.ability.communication.respond` is said through speaking and recorded
in the conversation. Input addressed to the bot as
`bot.ability.communication.input` is admitted into the conversation, whose
updated history is the next turn's stimulus; so is a turn's selected
`bot.ability.communication.input` (the original's SpeechHeard seed admits heard
speech this way), which names no target, so the body sends it to itself. The full Contract, Topology, Bindings, and Proof sections come
with the phase's saved live examples.

## Running

The bot runs in an environment that invokes it (decision D102): the room,
[`environments/room/room.scx`](../../environments/room/room.scx), the root
session, invokes it as its participant `floppy` and activates it on
joining (decision D107). Give the room inputs on standard input; it routes
`bot.input` and `bot.deactivate` to every bot, and
`bot.ability.communication.input` to the participants its `target_ids`
names (every bot when it names none):

```console
# The locally built packages (the dev cache under target/).
printf '%s\n' \
  '{"name":"bot.input","data":{"observation":{"event":"text","data":"hello"}}}' |
  scx run --offline --cache-dir target/components/package-cache --env-file .env --dir https://github.com/stateforward/floppy/services/sql=memory::/memory --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" --wasi https://github.com/stateforward/floppy/services/pgsql=inherit-network \
    --dir https://github.com/stateforward/floppy/services/speech=media::/media --dir https://github.com/stateforward/floppy/services/files=behaviors::/behaviors \
    --fuel 9223372036854775807 environments/room/room.scx
# A conversation: text addressed to the bot, answered through floppy.say.
printf '%s\n' \
  '{"name":"bot.ability.communication.input","data":{"source_ids":["gabe"],"target_ids":["floppy"],"content":"Hi floppy!","content_type":"text/plain"}}' |
  scx run --offline --cache-dir target/components/package-cache --env-file .env --dir https://github.com/stateforward/floppy/services/sql=memory::/memory --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" --wasi https://github.com/stateforward/floppy/services/pgsql=inherit-network \
    --dir https://github.com/stateforward/floppy/services/speech=media::/media --dir https://github.com/stateforward/floppy/services/files=behaviors::/behaviors \
    --fuel 9223372036854775807 environments/room/room.scx
# floppy's latest release, once one is published (phase 7).
printf '%s\n' \
  '{"name":"bot.input","data":{"observation":{"event":"text","data":"hello"}}}' |
  scx run --env-file .env --dir https://github.com/stateforward/floppy/services/sql=memory::/memory --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" --wasi https://github.com/stateforward/floppy/services/pgsql=inherit-network \
    --dir https://github.com/stateforward/floppy/services/speech=media::/media --dir https://github.com/stateforward/floppy/services/files=behaviors::/behaviors \
    --fuel 9223372036854775807 environments/room/room.scx
```

The files service's grant is the `behaviors/` directory beside `bots/`
and `abilities/` (create it first; it is ignored by git), where the builder
reflection invokes writes a learned behavior's three documents and the
candidates and routines runners load it by its path (decision D121).

`--fuel 9223372036854775807` (2^63 - 1, the most fuel Wasmtime puts into a store at
once) gives every host call the fuel a bot's start needs. Entering the room
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
`<param>`s: `model` (the default is `Qwen/Qwen3.8-Flash-Next`),
`database`, `environment` (the environment's id) and `bounds`, an object
that may set the time bounds of the model-bound stages (decision D101),
each in milliseconds:
`child_operation_timeout_ms` (a cognition stage without progress; a
streamed reasoning reply's progress starts it over),
`child_operation_ceiling_ms` (a stage in all), `revising_timeout_ms`
(reflection's revision through the builder without progress) and
`revising_ceiling_ms` (a revision in all); absent, each is the ability's
default (the root README's "Timeouts and bounds"). The memory ability (`abilities/memory/memory.scx`, the original's
Memory) keeps the bot's database, `/memory/floppy.sqlite`, through the sql
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
([Memory engines](../../README.md#memory-engines)). The `generate` service
imports `wasi:filesystem` too (it reads the images a message names), so
the run grants it a media directory the same way (create `media/` first;
it is ignored by git).
Speaking needs `FLOPPY_SPEECH_BASE_URL` (the speech service has no default
backend; `FLOPPY_SPEECH_API_KEY` too unless the backend is keyless) and the speech service granted the
media directory its audio is written to (`--dir
https://github.com/stateforward/floppy/services/speech=HOST::/media`);
without them a response fails with `bot.ability.communication.failed`.
[`.env.example`](../../.env.example) names the variables, and the README's
[Defaults](../../README.md#defaults) lists the model, the timeouts and the
bounds this bot ships with.

Sound (phase 6): `environment.sound` `{audio, media_type, sample_rate_hz,
channels, kind, ...}` goes to the [listening](../../abilities/listening/listening.scx),
the body's input ability (the original's Listening), and speaking's efference copy
goes to it too, so the bot can tell its own voice apart. Its
[sensitivity](../../abilities/sensitivity/sensitivity.scx) scores the sound against the
body's prediction and its [interpretation](../../abilities/interpretation/interpretation.scx)
(the original's Interpretation) works out what it was: voice detection on every
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
neither has a default backend. A sound that cannot be heard is logged as
`bot.ability.listening.failed`.

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
records, which are not the original events, are labelled `floppy.*`:
`floppy.output` is the turn's selected events, speaking logs the words as
`floppy.say`, `floppy.choices` is each turn's choices (JSON, each tagged
with its offering session's id as `target`), `floppy.focus` each focus
change, `floppy.deferred` each input deferred during a turn,
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
- `cargo test -p floppy-tests --test reading`: reading and vision under a
  harness, image decoding against a generate double (the original's Reading;
  nothing in the bot invokes it yet).
- `cargo test -p floppy-tests --test cognition`: the cascade against fake
  decision and model backends (each path, the tuner, failures, hop timeouts
  and reboot requests, cancels).
- `cargo test -p floppy-tests --test communication`: the communication
  path against the cognition double (a sent response said and recorded,
  conversation input as a turn whose stimulus carries the history,
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
