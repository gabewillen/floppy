---
name: floppy-environment
description: Ports the original's Environment to floppy, one provable slice at a time - an environment document as the root session that invokes bots and devices as its participants, takes what they put into it, and delivers each stimulus to the participants that perceive it.
project: floppy
repository: github.com/gabewillen/floppy
branch: fix/e2e-live-findings for this plan; one branch per slice thread, merged into main when green
design: the environment is an SCXML document, not host code (D14); join is an invoke, leave is the invoke's end; every new name waits for the maintainer's approval (Names To Confirm) before slice 1 starts
created: 2026-10-12
---

<!-- mdscript: use the mdscript-exec skill or read [spec.md](https://raw.githubusercontent.com/gabewillen/mdscript/main/spec.md) -->

## Rules For Every Slice

* the phase 3 to 7 plans' rules hold: `.scx` documents with one root `<script src>` each (D65), `event:schema` payload checks (D41, D42), conds only correlate (D81: no `cond` on `_event.name` or `_event.invokeid`), 160-character `cond`/`expr` bound, I/O only in invoked services, nothing under `scxml.rs/` edited here
* the original's names stay byte for byte (`environment.sound`, `environment.visual`, `amplitude_db`, `received_level_db`, `bot.input`, `target_device`, `bot.focus_device`); every new name is marked `[floppy name]` or `[floppy field]` and is one the maintainer approved in [Names To Confirm](#names-to-confirm)
* no slice starts until the maintainer has answered Names To Confirm and the open decisions in [Decisions To Add](#decisions-to-add); a name changed there is changed in this plan before the slice uses it
* each slice starts with its red tests, run and seen failing for the stated reason, then the change, then the same tests green, then the gate in AGENTS.md (`cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`)
* a needed scxml.rs change is not designed around: the slice stops and reports it as a proposed upstream change ([scxml.rs Gaps](#scxmlrs-gaps))
* hard cutover (AGENTS.md, Engineering): the bot's old root-session paths, their tests and their docs go in the slice that replaces them

## The Gap

floppy dropped the original's Environment. Today `bots/floppy/floppy.scx` is the root session, so:

* what the bot puts into the environment goes nowhere: speaking's `environment.sound`, relayed by the communication, is a `<log label="environment.sound">` record ("the root has no parent") plus a fan-out to the bot's own input abilities
* `environment.sound` and `environment.visual` reach the bot only as host input on `scx run`'s standard input
* devices are a static `<data id="devices" expr="[]"/>` that `<bot:attention>` and `bot.focus_device` pick from, with a `device_source_refs` map nothing fills; no device runs, and a selected device action has nowhere to go
* no second bot or device can share a space with the bot, hear it, or be heard by it
* the model never sees the original's `<environment id="...">` world block

## The Original's Environment

* **A scope with two planes.** Addressing (every started machine in the scope, device firmware included, stays reachable and can emit into the environment) is separate from presence (the participant set). Only an instance admitted by `join` is a participant, and only participants receive a broadcast. Abilities run in a private scope under their bot: they can emit into the environment but never receive its broadcasts. Device firmware is never a participant; it gets stimuli through its device's forward, so a broadcast reaches it once.
* **Join and leave.** A bot's attach starts it in the environment, joins it (with an optional placement), and dispatches `bot.activate`; its detach dispatches `bot.deactivate` and leaves. A device joins when it starts (with its own placement) and leaves when it stops; a device powers its peripherals first, and each of them joins too. Joining requires a started instance, is idempotent, and refuses an instance started in another environment or one already placed elsewhere.
* **Broadcast.** `broadcast(event, origin)` delivers an `environment.sound` or `environment.visual` to every participant it is still audible to, one delivery per recipient. The emitter says how loud the sound is (`amplitude_db`, dB SPL at 1 m) and the environment says how far it carries: a participant with a placement `{position, threshold_db}` receives the sound at `received_level_db = amplitude_db - 20 log10(max(d, 0.01) / 1.0)` (free-field spreading, -6 dB per doubling; 1 cm clamp so an earpiece at the ear is not a domain error) and hears it only when that level is at least its threshold. With no amplitude, no origin, or no placement there is nothing to measure: the participant hears it and `received_level_db` is left as it was. A visual stimulus has no amplitude, so it reaches every participant. When the filter leaves nobody, nobody receives it (the original guards the case where an empty recipient list would mean "everybody"). The emitter is a participant too, so a bot hears its own speech; its efference copy is what withholds it.
* **Who emits.** Speaking's playout broadcasts `environment.sound` with its mouth's (speaker's) amplitude and placement as origin and the mouth as source. A speaker device transduces `devices.audio.output` into `environment.sound`. A phone broadcasts its ring as `environment.sound` (`kind` `phone.ringing`, the caller in the data), and tells the bots holding it about silent facts (a call connecting, ending) with `bot.input` sent straight to them, never to the room.
* **What the bot does with it.** While active, the bot's body fans each `environment.sound` and `environment.visual` it receives out to its input abilities, never to the cognition. A device's `bot.input` reaches the bot with `target_device`, or with a source id the body maps to a configured device reference, which grounds the turn's focus.
* **Devices and actions.** A bot is constructed with its devices (reference to device) and starts them in its environment during activation, as their owner. On each activation the body declares `owned_devices` (reference to the device's runtime id) on its snapshot. The focused device's enabled transitions are merged into the bot's snapshot, so the device's events become the turn's choices; a selection naming the device is dispatched to it as a real event, and the device's shell delivers it to its firmware.
* **What the model sees.** Each turn the environment renders the world block from the bot's perspective: `<environment id="ENV"><self state="..."> ... </self></environment>`. `id` is the environment's own stable identity (never an actor id); `<self>` is the bot's live snapshot, its state as an attribute and its snapshot attributes as children; `owned_devices` resolves each id in the environment's own scope into `<device ref="..." id="..." state="...">` with that device's attributes (a device not live is dropped; nested mappings become nested elements; `None` and opaque values render nothing). The block is the turn's instructions, the system message reasoning and intuition send.
* **Not carried over (and why).** Revival of a cancelled scope, the weak maps, the address registry (`/env/<id>/<actor>`) and studio telemetry are the original's runtime plumbing: SCXML sessions end with their parent and scxml.rs owns telemetry (D14). "Started in another environment" and "placed elsewhere" cannot happen: a session has exactly one invoker and its placement is the environment's data.

## The Mapping

| The original | floppy |
| --- | --- |
| `Environment` (a context) | an environment document, `environments/<name>/<name>.scx`, run as the root session by `scx run`; each bot and device it holds is an `<invoke type="scxml">` with a literal `id` and `src` |
| `join(instance, placement)` | the invoke starts when its state is entered; the environment adds the invoke id to `participants` on that entry; the placement is the environment's `placements[<id>]` data |
| `leave(instance)` | the invoke ends: leaving its state cancels it (SCXML 6.4), or the participant finishes (`done.invoke.<id>`) or fails (`error.communication.<id>`, D82); each removes the id from `participants` |
| Bot attach (start, join, `bot.activate`) and detach | entering the bot's state, then `bot.activate` sent to `#_<id>` with the environment's `activation[<id>]` data (open decision 5); detach is leaving the state |
| addressing vs presence; abilities in a private scope | only the environment's children are participants; a bot's abilities are its own children, never the environment's, so they cannot receive a broadcast except through their body's fan-out |
| `broadcast(event, origin)` from speaking, a speaker, a phone | the emitter sends `environment.broadcast {event, data}` [floppy name] to `#_parent`; inside a bot it is relayed child to parent (speaking to the communication to the body to the environment), each hop a `<send target="#_parent">` |
| per-recipient delivery with placement and loudness | the environment's `environment.broadcast` transition: a `<foreach>` over `participants`, an `<if cond="hears(...)">`, and one `<send eventexpr targetexpr="'#_' + id">` per recipient whose `<content expr>` fills `received_level_db`; the origin is the sender's placement, `placements[_event.invokeid]` read in an `expr` (never a `cond`); the dB law and threshold are functions of `environments/<name>/<name>.js` |
| the emitter hears itself | the emitter is in `participants`, so it receives the broadcast; the bot's efference copy withholds its own product as today (D98) |
| host stimuli (`scx run` input) | host input reaches only the root (scxml.rs live session), so it enters the environment: `environment.sound` and `environment.visual` from the host are broadcast with no origin; `bot.activate`, `bot.deactivate`, `bot.input` and `bot.ability.communication.input` go to every bot participant; `environment.input {participant, event, data}` [floppy name] addresses one |
| devices owned by a bot, started in its environment | device documents (`devices/<name>/<name>.scx`) invoked by the environment as participants (a device has a place in the world), their I/O an invoked service; `holdings[<bot id>]` names the device ids a bot holds, passed to the bot as `<param name="devices">`, which fills the body's existing `devices` data |
| device shell forwarding to firmware | one device session is shell and firmware; a device that has firmware as its own machine invokes it as its child and forwards to it, as the bot does for its abilities, so a broadcast reaches the firmware once |
| focused device's transitions as the turn's choices | each device sends `device.offers {id, state, attributes, choices}` [floppy name] to `#_parent` on start and on each change; the environment relays it to its holders; the body keeps each held device's latest offer and, while a device is focused, `<decision:assemble>` appends that device's choices tagged with its reference as `target` |
| a selection dispatched to the device | `routing` places a selection whose target is a held device and sends `environment.dispatch {device, event, data}` [floppy name] to `#_parent`; the environment sends `event` with `data` to `#_<device>` if the sender holds the device, where scxml.rs checks it against the device's `event:schema` (D41) and refuses it with `error.execution` in the environment otherwise |
| a phone telling the bots holding it (`bot.input` straight to them) | the device sends `bot.input` to `#_parent`; the environment sends it to each holder with `target_device` stamped as the device's reference, which `<bot:attention>` reads (the original's `_interrupt_reference` prefers a stamped `target_device`); `device_source_refs` goes |
| `owned_devices` on the bot's snapshot and `model_snapshot` | `<bot:world location environment devices/>` [floppy name], a `bot` namespace element: it reads the body's own configuration through `session-snapshot-host` (as `<decision:choice>` does) and renders `<environment id><self state><owned_devices><device ref id state>` from the environment id the bot was given and the held devices' latest `device.offers`, with the original's attribute rules; the body puts it on `bot.ability.cognition.input` as `world` [floppy field], and the cognition hands it to reasoning and intuition as the system message's world block |
| `environment_id` (stable, never an actor id) | the environment's `environment_id` `<data>`, passed to each bot as `<param name="environment">` (open decision 4 on its default) |

## scxml.rs Gaps

None of these blocks the design above; each is flagged, not designed around silently. The first two are the places where the original's directness costs floppy an envelope name; an upstream change would remove the envelope.

* **Session reach (`#_scxml_<sessionid>`).** scxml.rs reaches only the session itself, its invoker and its own live children (docs/limits.md, a platform choice SCXML 1.0 C.1 allows). Speaking cannot send to the environment directly (it is the environment's great-grandchild) and a bot cannot send to a device (a sibling). The plan relays through `#_parent` (`environment.broadcast`, `environment.dispatch`). **Proposed upstream change (optional):** let a session reach every ancestor (and, for devices, its invoker's live children) by `#_scxml_<sessionid>`; speaking would then send `environment.sound` to the environment's session id, passed down as a param, exactly as the original's ability broadcasts into the scope it was started in, and a bot would send a device action to the device's session id.
* **Host input addresses only the root.** `scx run`'s input record is `{name, data}` for the root session. The plan adds `environment.input {participant, event, data}`. **Proposed upstream change (optional):** a `target` field on the input record naming an invoked session.
* **Static membership.** SCXML invokes are declared in the document; `srcexpr` makes a child's source dynamic but not the number of children, so an environment cannot admit a participant its document does not name. Each environment is authored for its scene (the builder can author one); a scene with a variable cast uses a fixed set of participant states entered by condition. Not a scxml.rs change: it is SCXML.
* **Same name from parent and child.** The body takes `environment.sound` as a stimulus that reached it (from the environment); if it also received its own outward sound under that name (from the communication) it could tell them apart only by `_event.invokeid`, which D81 forbids in a `cond`. Hence the outward envelope `environment.broadcast`, a floppy transport name for the original's `broadcast` call; not a scxml.rs gap.
* **No cross-session snapshot.** A session cannot read another session's configuration, so the world block cannot read a device's state the way the original reads its live snapshot; each device reports it in `device.offers`. **Possible upstream change (not proposed):** `session-snapshot-host` reading a child's snapshot.
* **To verify in slice 1 (not known gaps):** a `<send target="#_<id>">` in the same macrostep that enters the invoking state (the invoke starts after the macrostep, SCXML 6.4) raises `error.communication`, which the environment must not take as a leave; `<send target="#_parent">` from the root raises `error.communication` (relevant only to ability harness tests); a child's `<log>` records keep their `label` with `parent` set, so the tests' `logs()` helper still finds them.

## What Changes In The Current Bot

* `bots/floppy/floppy.scx` header: "the root SCXML session" becomes "a participant its environment invokes"; the Speaking, Environment and Outputs paragraphs lose "the root has no parent"; the Running paragraph names the environment command
* the `environment.sound` transition on `active` (logging `environment.sound` and fanning out the bot's own speech): its `<log label="environment.sound">` goes (the root-has-no-parent branch); the fan-out stays and now carries only what the environment delivers
* new on `active`: `environment.broadcast` relayed to `#_parent`; `device.offers` kept per device; on `routing`, a device route sends `environment.dispatch` to `#_parent`
* `<data id="devices" expr="[]"/>`: filled by the environment's `<param name="devices">`; `<data id="device_source_refs">` and `<bot:attention sources>` go once every device input carries `target_device` (slice 4)
* `<param name="environment">` received and `<bot:world>` on `processing` entry, `world` added to `cognitionInput()` in `floppy.js` (slice 5)
* `abilities/speaking/speaking.scx` (its playout `<send event="environment.sound" target="#_parent">`, line 188) and `abilities/communication/communication.scx` (its relay, line 167) send and relay `environment.broadcast {event: "environment.sound", data}`; their schemas and twins follow (`abilities/schemas/`, `bots/floppy/schemas/`)
* README.md, `bots/floppy/README.md` and AGENTS.md's Bots section: the run command becomes `scx run --env-file <file> environments/<name>/<name>.scx`
* tests that inject `environment.*` as host input into the bot as the root, and must run it under an environment: `tests/tests/body.rs` (`environment_stimuli_fan_out_to_the_input_abilities_and_not_the_cognition`, `the_listenings_products_are_turn_stimuli`, and the case at line 982), `tests/tests/identity.rs` (line 208); tests that assert the bot's own `environment.sound` log record: `tests/tests/communication.rs` (lines 213 and 841); every test that starts `bots/floppy/floppy.scx` as the root (`body.rs`, `cognition.rs`, `communication.rs`, `identity.rs`, `reflection.rs`, `defaults.rs`, `floppy_live.rs`) through `tests/tests/support/mod.rs`, which gains an environment launcher; `tests/tests/contracts.rs` (the twin and name lists at lines 90, 130, 226, 534); the IRP cases that expect speaking's or the communication's `environment.sound` to the parent (`rg -l 'environment\.sound' abilities/*/irp` at the slice's start)
* unchanged: the listening, interpretation, identity and turn detector take `environment.sound` as they do; their harness tests inject it as host input into their own root and stay; the builder is no participant (reflection invokes it as a tool)

## Slices

In order: each slice builds on the one before. Fake backends throughout (the speech double or the speech service on a 127.0.0.1 fake, as `tests/tests/communication.rs` and `listening.rs` do); the live proof is slice 6.

### Confirm Names And Decisions

* goal: the maintainer answers [Names To Confirm](#names-to-confirm) and open decisions 1 to 6; this plan is updated to the answers and committed
* if any answer asks for a scxml.rs change (session reach, an input `target`), stop: that change goes upstream first and this plan is rewritten around it

### Add The Environment Root

* goal: `environments/room/room.scx` (with `room.js` and `schemas/`) invokes `floppy` (`src="../../bots/floppy/floppy.scx"`) as its one participant; host `bot.*` input reaches the bot; host `environment.sound` and `environment.visual` are broadcast to every participant; `environment.input` addresses one; a participant that finishes or fails leaves `participants`
* files: `environments/room/`, `tests/tests/environment.rs`, `tests/tests/support/mod.rs` (an environment launcher), `tests/fixtures/environment/two_bots.scx` (two floppy participants), `tests/tests/contracts.rs` (environments under the document structure rule and the twin rule)
* red tests (`tests/tests/environment.rs`): `host_bot_input_reaches_the_bot_through_its_environment` (bot.activate then bot.ability.communication.input to a bot invoked by the environment: `bot.activated` and a turn, red because no environment document exists); `a_host_sound_is_broadcast_to_every_participant_and_fans_out_to_the_input_abilities` (two bots, both listenings log the sound, neither cognition receives it); `environment_input_addresses_one_participant` (two bots, only `a` takes a turn); `a_participant_that_ends_leaves_the_environment` (a bot driven to `degraded` is dropped from `participants`; the next broadcast raises no `error.communication`, logged as `environment.left` [floppy name, open]); `the_environment_refuses_host_input_its_schemas_refuse` (`machine-invalid-input`)
* then move every test that injects `environment.*` into the bot as the root under the environment launcher (the body and identity cases above), unchanged in what they assert

### Put The Bot's Stimuli Into The Environment

* goal: speaking, the communication and the body relay `environment.broadcast`; the environment broadcasts it to every participant, the emitter included; the body's `environment.sound` log record goes
* files: `abilities/speaking/speaking.scx`, `abilities/communication/communication.scx`, `bots/floppy/floppy.scx`, `environments/room/room.scx`, the `environment.broadcast` schema and twins, `tests/tests/communication.rs`, `tests/tests/environment.rs`, the IRP cases named above
* red tests: `a_bots_speech_reaches_the_other_participant` (two bots, `a` answers a communication input through the speech double; `b`'s listening logs the sound and takes a turn on its product; red because the sound stops at `a`'s body); `a_bot_hears_its_own_speech_through_the_environment_and_withholds_it` (one bot: its listening receives its own sound from the environment, after the efference copy, and hands over no product, D98); communication's two assertions on the `environment.sound` record become assertions on the sound the environment delivered; an ability harness's `environment.broadcast` to a root parent is not taken as a turn
* hard cutover: no bot path logs `environment.sound` any more; the "root has no parent" text goes

### Place Participants And Carry Loudness

* goal: the original's reception: `placements[<id>] = {position: {x, y, z}, threshold_db}`, the received level by the original's law, the threshold, the emitter's placement as origin, `received_level_db` filled per recipient, the emitter's `amplitude_db` untouched; visual stimuli reach everyone
* files: `environments/room/room.js` (`receivedLevelDb`, `hears`), `room.scx` (`placements`, `reference_distance_m` 1.0, `minimum_distance_m` 0.01 as `<data>`), `tests/tests/environment.rs`, `tests/fixtures/environment/two_bots.scx`
* red tests: `a_sound_carries_by_distance` (60 dB at the emitter, a listener 2 m away receives `received_level_db` 53.979... and a listener at 0 m receives 100 dB, the 1 cm clamp); `a_participant_below_its_threshold_does_not_hear` (threshold 55 at 2 m: not delivered); `a_sound_too_quiet_for_everyone_reaches_nobody` (the original's empty-recipient guard: no send at all, not a send to all); `without_amplitude_or_placement_everyone_hears_and_no_level_is_filled`; `a_visual_reaches_every_participant`
* the speaking's sound carries no `amplitude_db` (the bot has no mouth device), so it reaches every participant at no measured level until a mouth is ported; this is the original's behavior without a speaker

### Hold Devices And Send Them Actions

* goal: a device document invoked by the environment, held by a bot (`holdings`), passed to it as `devices`; the device's `device.offers` reaches its holders; a focused device's choices join the turn's choices; a selected device action reaches the device as a real event through `environment.dispatch`; the device's `bot.input` reaches its holders with `target_device` stamped; the device can broadcast a sound
* files: `tests/fixtures/environment/lamp/lamp.scx` (a fixture device with `lamp.on`/`lamp.off` and their schemas; no real device is ported, devices stay out of scope otherwise), `environments/room/room.scx`, `bots/floppy/floppy.scx` and `floppy.js` (offers per device, the device route, `device_source_refs` removed), `namespaces/bot` (`<bot:attention>` without `sources`), the new schemas and twins, `tests/tests/environment.rs`, `tests/tests/body.rs` (`the_choices_are_the_bodys_and_the_offers_and_focus_is_grounded` runs its lamp under the environment instead of `devices` data)
* red tests: `a_held_devices_input_grounds_the_turns_focus` (the lamp's `bot.input` reaches the bot with `target_device` `lamp`; `floppy.focus` `lamp`); `a_focused_devices_choices_are_offered_and_a_selection_reaches_it` (focused on the lamp, the cognition double selects `lamp.on`; the lamp logs it and its next `device.offers` reports state `on`); `a_device_action_the_device_refuses_fails_in_the_environment` (data the lamp's schema refuses: `error.execution`, no delivery); `a_bot_cannot_act_on_a_device_it_does_not_hold`; `a_devices_sound_is_broadcast` (the lamp's click is heard by the bot's listening)
* AGENTS.md's devices rule changes with this slice (open decision 2)

### Show The Model Its World

* goal: `<bot:world>` renders the original's block; the body sends it as `world` on `bot.ability.cognition.input`; reasoning puts it in its system message and intuition in its decision instructions, as the original's turn instructions
* files: `namespaces/bot` (`world`), `bots/floppy/floppy.scx`, `floppy.js`, `abilities/cognition/`, `abilities/reasoning/`, `abilities/intuition/`, the `bot.ability.cognition.input` schema and twins, `tests/tests/cognition.rs`, the namespace's unit tests
* red tests: the element's unit tests (the exact XML for a bot with no devices, with the lamp, a device without a live offer dropped, a nested attribute mapping, a `null` attribute omitted, attribute values escaped); `reasonings_prompt_carries_the_environment_block` (a fake generate backend records the prompt: its system message holds `<environment id="ENV"><self state="processing"><owned_devices><device ref="lamp" id="..." state="off"/>`)

### Prove Two Participants Live

* goal: two floppy bots in one environment on the maintainer's backends: `a` is asked (through `environment.input`) to say something, speaks it, `b` hears it, decodes it and takes a turn on it, and `a` does not take its own speech as a turn
* files: `tests/tests/environment_live.rs` (ignored unless the live env files are present, as `floppy_live.rs`), `docs/live-proof.md`
* needs: `FLOPPY_BASE_URL`, `FLOPPY_API_KEY`, `FLOPPY_SPEECH_BASE_URL`, `FLOPPY_SPEECH_API_KEY`, and `FLOPPY_VOICE_BASE_URL` (without a voice backend the listening fails `voice_detection` and hands over no product, D98), with the speech service's `--dir` media grant shared by both bots
* done when: one recorded run passes (each bot's records, the environment's broadcasts, `b`'s `bot.processing.completed` on `a`'s words) and is entered in docs/live-proof.md with its model and backends; a failure is recorded as it happened

### Document The Environment

* goal: decisions D102 to D106 (below), AGENTS.md (the run command, the devices rule, `environments/` and `devices/` in the layout list), docs/port-plan.md (an Environment row in the concept map, the Devices row, conventions for broadcast and placement), docs/handoff.md, README.md, `bots/floppy/README.md`, `environments/room/README.md`

## Decisions To Add

The next free number is D102 (D101 is the latest). Each states what the slice decides and why; the open ones need the maintainer's answer before slice 1.

* D102: the environment is an SCXML document run as the root session (`environments/<name>/<name>.scx`); bots and devices are its invoked participants; join is an invoke starting, leave is the invoke ending (cancelled, done, or failed); presence is the environment's `participants`, and a bot's abilities are never participants (the original's private scope). A bot runs under an environment: `scx run ... environments/<name>/<name>.scx`. **Open decision 1:** is the bot's own document still a supported root for `scx run` (for its own tests), or does every bot run go through an environment (hard cutover; recommended)?
* D103: a participant puts a stimulus into the environment with `environment.broadcast {event, data}` to `#_parent`, relayed by its ancestors; the environment delivers it to every participant that perceives it, the emitter included, with the original's free-field law (1 m reference, 1 cm clamp), threshold and `received_level_db`, its origin the sender's placement. Why an envelope: SCXML reaches no ancestor beyond the parent, and a same-named event from parent and child could be told apart only by `_event.invokeid` (D81). **Open decision 3:** envelope (no scxml.rs change, recommended now) or the upstream session-reach change and a direct `environment.sound` to the environment's session id
* D104: devices are SCXML documents invoked by the environment (`devices/<name>/<name>.scx`), their I/O invoked services; `holdings` names which bot holds which, given to the bot as `devices`; a device reports `device.offers`; a selection for a held device goes `environment.dispatch` to the environment, which sends the real event to the device; a device's `bot.input` reaches its holders with `target_device` stamped, replacing `device_source_refs`. **Open decision 2:** AGENTS.md says a bot's devices MUST be invoked services; this replaces it with "a device is an SCXML document its environment invokes; its I/O is an invoked service" (recommended: the original's device is a state machine with firmware, and floppy is .scx-first)
* D105: the model sees the original's world block: `<bot:world>` renders `<environment id><self state><owned_devices>...` from the environment id and the held devices' latest offers; the body sends it as `world` [floppy field] on `bot.ability.cognition.input`; reasoning and intuition put it in their system instructions. **Open decision 4:** the environment's id: its `_sessionid` (set by `scx run --session-id`, default `session`; recommended, as the id must be stable for the run and is the environment's own) or a run-supplied `<data>`
* D106: host input enters the environment: `environment.sound` and `environment.visual` are broadcast with no origin, `bot.activate`, `bot.deactivate`, `bot.input` and `bot.ability.communication.input` go to every bot participant, and `environment.input {participant, event, data}` addresses one. **Open decision 5:** does joining activate the bot, as the original's attach does (recommended; the environment holds each bot's `activation` data and tests set bounds in their environment fixture), or does activation stay host input forwarded to the bot? **Open decision 6:** envelope `environment.input` now, or the upstream input-record `target` first

## Names To Confirm

Every new name this plan introduces; none is used before the maintainer approves it or gives another.

* directories and documents: `environments/<name>/<name>.scx` (with `<name>.js`, `schemas/`, `README.md`); `environments/room/room.scx` (the default environment holding one floppy); `devices/<name>/<name>.scx`; fixtures `tests/fixtures/environment/two_bots.scx` and `tests/fixtures/environment/lamp/lamp.scx`; tests `tests/tests/environment.rs` and `tests/tests/environment_live.rs`
* events [floppy name]: `environment.broadcast {event, data}` (a participant to the environment); `environment.dispatch {device, event, data}` (a bot to the environment, for a held device); `environment.input {participant, event, data}` (the host to the environment, for one participant); `device.offers {id, state, attributes, choices}` (a device to the environment and on to its holders); `environment.left` (the log label when a participant leaves; a log label, not an event, if preferred)
* fixture events: `lamp.on`, `lamp.off`
* params passed to a bot: `environment` (the environment id); `devices` (the held device references, the body's existing `devices` data)
* the environment's `<data>`: `environment_id`, `participants`, `bots`, `placements` (`{position: {x, y, z}, threshold_db}`), `holdings`, `activation`, `reference_distance_m` (1.0), `minimum_distance_m` (0.01)
* field [floppy field]: `world` on `bot.ability.cognition.input` (and the stage `<param>` that carries it to reasoning and intuition)
* namespace element: `<bot:world location environment devices/>` in `namespaces/bot`
* JavaScript functions in `environments/room/room.js`: `receivedLevelDb`, `hears`, `deliveredStimulus`
