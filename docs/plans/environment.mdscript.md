---
name: floppy-environment
description: Ports the original's Environment to floppy, one provable slice at a time - an environment document as the root session that invokes bots and devices as its participants, takes what they put into it, and delivers each stimulus to the participants that perceive it.
project: floppy
repository: github.com/gabewillen/floppy
branch: fix/e2e-live-findings for this plan; one branch per slice thread, merged into main when green; the scxml.rs slice lands upstream first on feat/invoke-services
design: the environment is an SCXML document, not host code (D14); join is an invoke, leave is the invoke's end; the maintainer's six decisions of 2026-10-12 are recorded as D102 to D108; the remaining names wait for approval (Names To Confirm)
created: 2026-10-12
---

<!-- mdscript: use the mdscript-exec skill or read [spec.md](https://raw.githubusercontent.com/gabewillen/mdscript/main/spec.md) -->

## Rules For Every Slice

* the phase 3 to 7 plans' rules hold: `.scx` documents with one root `<script src>` each (D65), `event:schema` payload checks (D41, D42), conds only correlate (D81, with the one exception D105 adds), 160-character `cond`/`expr` bound, I/O only in invoked services, nothing under `scxml.rs/` edited here
* the original's names stay byte for byte (`environment.sound`, `environment.visual`, `amplitude_db`, `received_level_db`, `bot.input`, `target_device`, `target_ids`, `bot.focus_device`); the few new names are marked `[floppy name]` or `[floppy field]` and are the ones the maintainer approves in [Names To Confirm](#names-to-confirm); no slice uses a name before it is approved
* each slice starts with its red tests, run and seen failing for the stated reason, then the change, then the same tests green, then the gate in AGENTS.md (`cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`)
* hard cutover (AGENTS.md, Engineering): the bot's root-session paths, their tests and their docs go in the slice that replaces them; a bot never runs as the root again (D102)

## The Gap

floppy dropped the original's Environment. Today `bots/floppy/floppy.scx` is the root session, so:

* what the bot puts into the environment goes nowhere: speaking's `environment.sound`, relayed by the communication, is a `<log label="environment.sound">` record ("the root has no parent") plus a fan-out to the bot's own input abilities; every other event the body emits is a log record for the same reason
* `environment.sound` and `environment.visual` reach the bot only as host input on `scx run`'s standard input
* devices are a static `<data id="devices" expr="[]"/>` that `<bot:attention>` and `bot.focus_device` pick from, with a `device_source_refs` map nothing fills; no device runs, and a selected device action has nowhere to go
* no second bot or device can share a space with the bot, hear it, or be heard by it
* the model never sees the original's `<environment id="...">` world block

## The Original's Environment

* **A scope with two planes.** Addressing (every started machine in the scope, device firmware included, stays reachable and can emit into the environment) is separate from presence (the participant set). Only an instance admitted by `join` is a participant, and only participants receive a broadcast. Abilities run in a private scope under their bot: they can emit into the environment but never receive its broadcasts. Device firmware is never a participant; it gets stimuli through its device's forward, so a broadcast reaches it once.
* **Join and leave.** A bot's attach starts it in the environment, joins it (with an optional placement), and dispatches `bot.activate`; its detach dispatches `bot.deactivate` and leaves. A device joins when it starts (with its own placement) and leaves when it stops; a device powers its peripherals first, and each of them joins too. Joining requires a started instance, is idempotent, and refuses an instance started in another environment or one already placed elsewhere.
* **Broadcast.** `broadcast(event, origin)` delivers an `environment.sound` or `environment.visual` to every participant it is still audible to, one delivery per recipient. The emitter says how loud the sound is (`amplitude_db`, dB SPL at 1 m) and the environment says how far it carries: a participant with a placement `{position, threshold_db}` receives the sound at `received_level_db = amplitude_db - 20 log10(max(d, 0.01) / 1.0)` (free-field spreading, -6 dB per doubling; 1 cm clamp so an earpiece at the ear is not a domain error) and hears it only when that level is at least its threshold. With no amplitude, no origin, or no placement there is nothing to measure: the participant hears it and `received_level_db` is left as it was. A visual stimulus has no amplitude, so it reaches every participant. When the filter leaves nobody, nobody receives it (the original guards the case where an empty recipient list would mean "everybody"). The emitter is a participant too, so a bot hears its own speech; its efference copy is what withholds it.
* **Who emits.** Speaking's playout broadcasts `environment.sound` straight into the environment it was started in, with its mouth's (speaker's) amplitude and placement as origin when it has a mouth, and no origin when it has none. A speaker device transduces `devices.audio.output` into `environment.sound`. A phone broadcasts its ring as `environment.sound` (`kind` `phone.ringing`, the caller in the data), and tells the bots holding it about silent facts (a call connecting, ending) with `bot.input` sent straight to them, never to the room.
* **What the bot does with it.** While active, the bot's body fans each `environment.sound` and `environment.visual` it receives out to its input abilities, never to the cognition. A device's `bot.input` reaches the bot with `target_device`, or with a source id the body maps to a configured device reference, which grounds the turn's focus.
* **Devices and actions.** A bot is constructed with its devices (reference to device) and starts them in its environment during activation, as their owner; a device records the actors attached to it (its holders). On each activation the body declares `owned_devices` (reference to the device's runtime id) on its snapshot. The focused device's enabled transitions are merged into the bot's snapshot, so the device's events become the turn's choices; a selection naming the device is dispatched straight to it as a real event, and the device's shell delivers it to its firmware.
* **What the model sees.** Each turn the environment renders the world block from the bot's perspective: `<environment id="ENV"><self state="..."> ... </self></environment>`. `id` is the environment's own stable identity (never an actor id); `<self>` is the bot's live snapshot, its state as an attribute and its snapshot attributes as children; `owned_devices` resolves each id in the environment's own scope into `<device ref="..." id="..." state="...">` with that device's attributes (a device not live is dropped; nested mappings become nested elements; `None` and opaque values render nothing). The block is the turn's instructions, the system message reasoning and intuition send.
* **Not carried over (and why).** Revival of a cancelled scope, the weak maps, the address registry (`/env/<id>/<actor>`) and studio telemetry are the original's runtime plumbing: SCXML sessions end with their parent and scxml.rs owns telemetry (D14). "Started in another environment" and "placed elsewhere" cannot happen: a session has exactly one invoker and its placement is the environment's data.

## The Mapping

| The original | floppy |
| --- | --- |
| `Environment` (a context) | an environment document, `environments/<name>/<name>.scx`, run as the root session by `scx run`; each bot and device is an `<invoke type="scxml">` of it with a literal `id` and `src` (D102) |
| the environment's id | the root's `_sessionid` (D106), given to each bot as `<param name="environment">`; it is both the id the model sees and the address `#_scxml_<id>` |
| `join(instance, placement)` | the invoke starts when its state is entered; the environment adds the invoke id to `participants`; the placement is `placements[<id>]` |
| `leave(instance)` | the invoke ends: leaving its state cancels it (SCXML 6.4), or the participant finishes (`done.invoke.<id>`) or fails (`error.communication.<id>`, D82); each removes the id from `participants` |
| Bot attach (start, join, `bot.activate`) | the bot's `<invoke>` with its activation settings as `<param>`s (`model`, `bounds`, `database`, `environment`, `devices`: data ids the body already has, except `environment`); the body starts in `live` and activates at once, with no `bot.activate` (D107) |
| Bot detach (`bot.deactivate`, leave) | `bot.deactivate` ends the bot's session (a top-level final), which is its leave (D107) |
| addressing vs presence; abilities in a private scope | every session of the run is reachable by `#_scxml_<sessionid>` (D104), but only the environment's children are participants: a bot's abilities are its own children, so a broadcast reaches them only through their body's fan-out |
| speaking's `broadcast` | speaking sends `environment.sound` to `#_scxml_<environment>` (the `environment` param the body passes to the communication and the communication to speaking); no relay through the communication or the body (D103) |
| a device's `broadcast` (a speaker, a phone's ring) | the device sends `environment.sound` to `#_parent`, the environment |
| per-recipient delivery with placement and loudness | the environment's `environment.sound` transition: a `<foreach>` over `participants`, an `<if cond="hears(...)">`, and one `<send event="environment.sound" targetexpr="'#_' + id">` per recipient whose `<content expr>` fills `received_level_db`; the origin is the sender's placement, `placements[_event.invokeid]` read in an `expr` (never a `cond`): a device (a child) has one, speaking (not a child) and the host have none, as speaking without a mouth has none in the original; the law and threshold are functions of `environments/<name>/<name>.js` (D103) |
| the emitter hears itself | the emitter is in `participants`, so it receives the broadcast; the bot's efference copy withholds its own product as today (D98) |
| host input | it reaches only the root, so it enters the environment (D108): untargeted `environment.sound` and `environment.visual` are broadcast; `bot.ability.communication.input` goes to each participant its `target_ids` names (`targetexpr`), and to every bot when `target_ids` is empty (the original: "the input is ambient"); `bot.input` and `bot.deactivate`, which carry no target, go to every bot |
| the bot's emitted events (`bot.activated`, `bot.processing.completed`, ...) | sent to `#_parent`; the environment, now the root, logs each as `<log label="EVENT">` with its data, which keeps the run's records |
| devices owned by a bot, started in its environment | device documents (`devices/<name>/<name>.scx`) invoked by the environment; any real I/O a device does is an invoked service inside it (D105); `holdings[<bot id>]` names the device ids a bot holds, passed to the bot as `<param name="devices">` (the body's existing `devices` data) |
| the device's holders (its attached actors) | a device is invoked once each of its holders has sent `bot.activated`, whose `_event.origin` gives the environment the holder's address; the device gets its holders' addresses as `<param name="holders">` and its own reference as `<param name="device">` |
| device shell forwarding to firmware | one device session is shell and firmware; a device that has firmware as its own machine invokes it and forwards to it, as a bot does for its abilities, so a broadcast reaches the firmware once |
| focused device's transitions as the turn's choices | each device sends `device.offers {device, state, attributes, choices}` [floppy name] straight to each holder on start and on each change; the body keeps each held device's latest offer (by `device`, with the sender's `_event.origin` as its address in `device_source_refs`), and while a device is focused `<decision:assemble>` appends that device's choices tagged with its reference as `target` |
| a selection dispatched to the device | `routing` sends a selection whose target is a held device straight to the device's address `#_scxml_<sessionid>` as the real event; the device takes it only from a holder (`cond` on `_event.origin` against `holders`, the D81 exception D105 records) and scxml.rs checks its data against the device's `event:schema` (D41) |
| a phone telling the bots holding it (`bot.input` straight to them) | the device sends `bot.input` straight to each holder with `target_device` stamped as its reference, which `<bot:attention>` reads (the original's `_interrupt_reference` prefers a stamped `target_device`); `<bot:stimulus source>` becomes `_event.origin`, which `device_source_refs` maps to the reference as the original's source map does |
| `owned_devices` on the bot's snapshot and `model_snapshot` | `<bot:world location environment devices/>` [floppy name], a `bot` namespace element: it reads the body's own configuration through `session-snapshot-host` (as `<decision:choice>` does) and renders `<environment id><self state><owned_devices><device ref id state>` from the `environment` param and the held devices' latest offers, with the original's attribute rules; the body puts it on `bot.ability.cognition.input` as `world` [floppy field], and the cognition hands it to reasoning and intuition as the system message's world block (D106) |

### Why There Is No Envelope Event

The first form of this plan relayed a participant's sound as `environment.broadcast {event, data}`. With D104 it is not needed, and it goes:

* telling a participant's sound from host input is not needed: both are a sound in the environment and both are broadcast; the only difference is the origin, and that is read in an `expr` from `_event.invokeid` (a device, the environment's child, has a placement; the host and speaking have none), never in a `cond`
* the origin does not need to travel in the event: a device's placement is the environment's data; speaking has no mouth, so in the original too its sound has no origin; a mouth, when one is ported, is a speaker device with its own placement
* the bot no longer receives its own outward sound from a child (speaking sends past it), so `environment.sound` has one meaning at the body: a sound that reached it; there is nothing to tell apart by `_event.invokeid`

## scxml.rs Change

One change, upstream first (D104): `#_scxml_<sessionid>` reaches any session of the same run, as SCXML 1.0 C.1 lets the platform decide ("The set of SCXML sessions that are accessible to a given SCXML Processor is platform-dependent"); a session id that names no running session of the run raises `error.communication` ("If the sending SCXML session specifies a session that does not exist or is inaccessible, the SCXML Processor MUST place the error error.communication on the internal event queue"). Nothing else changes: `#_parent` and `#_<invokeid>` keep their meaning, `_event.origin` stays the sender's `#_scxml_<sessionid>` and `_event.invokeid` is set only for an event from an invoked child (SCXML 5.10.1), so a send to the parent by its session address still carries the invoke id and one to any other session does not. No new term beyond the spec's: the delivery class is "session".

* where: `components/scxml-ioprocessor-scxml` (`classify_target`: a well-formed `#_scxml_` id that is not the sender, its parent, or its invocation is a run session, else unreachable), the I/O processor WIT's `send-target`, `dispatch-decision` and `due-route` (a `session` case, at `@0.2.0`, unreleased, no bump), the engine's `SendTarget`/`Dispatch`/`DueRoute`, the Wasmtime host's run (deliver to the named session's external queue, refuse an ended one as `error.communication`), the browser host (parity), the record and replay log (a delivery between sessions is recorded as one), docs/limits.md ("Sessions `#_scxml_<sessionid>` reaches"), and the conformance baselines if any IRP case's result moves (none should: the IRP's `#_scxml_` cases use the sender's own session)
* how: per scxml.rs/AGENTS.md, in `~/VSCode/stateforward/scxml.rs-services` on `feat/invoke-services`, through its own review and proof; then `git subtree pull --prefix scxml.rs <scxml.rs> feat/invoke-services --squash` here, and the subtree table in docs/decisions.md updated

Still to verify in the environment slice (not known gaps): a `<send target="#_<id>">` in the same macrostep that enters the invoking state (the invoke starts after the macrostep, SCXML 6.4) raises `error.communication`, which the environment must not take as a leave; a child's `<log>` records keep their `label` with `parent` set.

Limits that stay (SCXML itself, no change proposed): invokes are declared in the document, so an environment admits only the participants it names (`srcexpr` makes a source dynamic, not the number of children); each environment is authored for its scene (the builder can author one). A session cannot read another's configuration, so a device reports its state in `device.offers`.

## What Changes In The Current Bot

* `bots/floppy/floppy.scx` header: "the root SCXML session" becomes "a participant its environment invokes"; the Speaking, Environment, Outputs and Running paragraphs lose "the root has no parent"
* every `<log label="EVENT">` the body writes for an event it emits (`bot.activated`, `bot.activating.failed`, `bot.deactivated`, `bot.processing.completed`, `bot.processing.failed`, `bot.processing.timed_out`, `bot.processing.cancel.timed_out`, `bot.reboot`, `bot.ability.speaking.output`, `bot.ability.communication.failed`, `bot.ability.conversation.failed`, `bot.ability.conversation.output`, `bot.ability.listening.failed`, `bot.ability.failed`) becomes a `<send target="#_parent">`; the `floppy.*` records stay logs (they are floppy's records, not events)
* the `environment.sound` transition on `active` loses its `<log>`; its fan-out stays and carries only what the environment delivers
* `inactive`, its `bot.activate` transition and `activationModel`/`activationBounds` in `floppy.js` go: `model`, `bounds`, `database`, `devices` and `environment` arrive as `<param>`s and the document starts in `live`; a failed activation and `bot.deactivate` end the session in a top-level final with the reason as donedata, as `degraded` does (open in Names To Confirm: the final's id)
* `devices` stays, filled by the `<param>`; `device_source_refs` stays, now filled from each held device's `device.offers` origin; new: the held devices' latest offers (`device_offers`), the device route in `routing`, `<bot:world>` on `processing` entry and `world` in `cognitionInput()`
* `<bot:stimulus source="_event.invokeid">` becomes `source="_event.origin"`
* `abilities/communication/communication.scx` loses its `environment.sound` relay (line 167) and passes `environment` to speaking; `abilities/speaking/speaking.scx`'s playout send (line 188) targets `'#_scxml_' + environment`
* `bots/floppy/schemas/bot.activate.json` and its twins go if no document takes `bot.activate` any more (the event stays the original's name for a bot that is not yet active; floppy no longer sends it)
* README.md, `bots/floppy/README.md` and AGENTS.md's Bots section: the run command becomes `scx run --env-file <file> environments/<name>/<name>.scx`, with no `bot.activate` on standard input
* tests that inject `environment.*` into the bot as the root, which now run under an environment: `tests/tests/body.rs` (`environment_stimuli_fan_out_to_the_input_abilities_and_not_the_cognition`, `the_listenings_products_are_turn_stimuli`, and the case at line 982), `tests/tests/identity.rs` (line 208); tests that assert the bot's own `environment.sound` log record: `tests/tests/communication.rs` (lines 213 and 841); every test that starts `bots/floppy/floppy.scx` as the root and sends `bot.activate` (`body.rs`, `cognition.rs`, `communication.rs`, `identity.rs`, `reflection.rs`, `defaults.rs`, `floppy_live.rs`), through `tests/tests/support/mod.rs`, which gains an environment launcher that writes each test's activation settings as the bot's `<param>`s; `tests/tests/body.rs`'s `the_choices_are_the_bodys_and_the_offers_and_focus_is_grounded` (its lamp becomes a device); `tests/tests/contracts.rs` (the twin and name lists at lines 90, 130, 226, 534, and environments and devices under the document structure rule); the IRP cases that expect speaking's or the communication's `environment.sound` to the parent (`rg -l 'environment\.sound' abilities/*/irp` at the slice's start)
* unchanged: the listening, interpretation, identity and turn detector take `environment.sound` as they do; their harness tests inject it as host input into their own root and stay; the builder is no participant (reflection invokes it as a tool)

## Slices

In order: each builds on the one before. Fake backends throughout (the speech double or the speech service on a 127.0.0.1 fake, as `tests/tests/communication.rs` and `listening.rs` do); the live proof is the last slice but one.

### Confirm Names

* goal: the maintainer answers [Names To Confirm](#names-to-confirm); this plan is updated to the answers and committed

### Reach Any Session Of The Run In scxml.rs

* goal: the [scxml.rs Change](#scxmlrs-change), upstream first, then the subtree pull here
* red tests (in scxml.rs, written with its own conventions): a grandchild's send to the root's `#_scxml_<sessionid>` is delivered with `_event.origin` the grandchild's address and no `_event.invokeid`; a child's send to a sibling's address is delivered; a send to a session that has ended, or to an id no session of the run has, raises `error.communication` in the sender; a child's send to its parent by `#_scxml_` still carries its invoke id; a delayed send to another session is delivered at its time and cancelled by `<cancel>`; record and replay reproduce a cross-session delivery; the browser host does the same
* done when: merged upstream after its review, pulled here, and D104's commit hashes recorded in docs/decisions.md

### Add The Environment Root

* goal: `environments/room/room.scx` (with `room.js` and `schemas/`) invokes `floppy` (`src="../../bots/floppy/floppy.scx"`) with its activation `<param>`s; the bot activates on join and sends its events to the environment, which logs them; host input is routed (D108); a participant that finishes or fails leaves `participants`; the bot's `inactive` and `bot.activate` go
* files: `environments/room/`, `bots/floppy/floppy.scx` and `floppy.js`, `tests/tests/environment.rs`, `tests/tests/support/mod.rs` (the environment launcher), `tests/fixtures/environment/two_bots.scx`, `tests/tests/contracts.rs`, then every test that started the bot as the root
* red tests (`tests/tests/environment.rs`): `a_bot_activates_on_joining_its_environment` (no `bot.activate` sent: the environment's log has `bot.activated`; red because no environment document exists); `host_communication_input_goes_to_the_participants_it_targets` (two bots, `target_ids` `["a"]`: only `a` takes a turn; `[]`: both do); `a_host_sound_is_broadcast_to_every_participant_and_fans_out_to_the_input_abilities` (both listenings log it, neither cognition gets it); `host_bot_input_goes_to_every_bot`; `a_deactivated_bot_leaves_its_environment` (`bot.deactivate`: the bot's session ends, `participants` drops it, the next broadcast raises no `error.communication`); `the_environment_refuses_host_input_its_schemas_refuse` (`machine-invalid-input`)

### Put The Bot's Speech Into The Environment

* goal: speaking sends `environment.sound` to the environment's session; the environment broadcasts it to every participant, the speaker included; the communication's relay and the body's `environment.sound` log go
* files: `abilities/speaking/speaking.scx`, `abilities/communication/communication.scx`, `bots/floppy/floppy.scx`, `environments/room/room.scx`, `tests/tests/communication.rs`, `tests/tests/environment.rs`, the IRP cases named above
* red tests: `a_bots_speech_reaches_the_other_participant` (two bots, `a` answers a targeted communication input through the speech double; `b`'s listening logs the sound and `b` takes a turn on its product; red because the sound stops at `a`'s body); `a_bot_hears_its_own_speech_through_the_environment_and_withholds_it` (one bot: its listening receives its own sound from the environment, after the efference copy, and hands over no product, D98); communication's two assertions on the `environment.sound` record become assertions on what the environment delivered; speaking's harness passes its own `_sessionid` as `environment` and receives the sound

### Place Participants And Carry Loudness

* goal: the original's reception: `placements[<id>] = {position: {x, y, z}, threshold_db}`, the received level by the original's law, the threshold, the sender's placement as origin, `received_level_db` filled per recipient, the emitter's `amplitude_db` untouched; visual stimuli reach everyone
* files: `environments/room/room.js` (`receivedLevelDb`, `hears`), `room.scx` (`placements`), `tests/tests/environment.rs`, a fixture device that makes a sound at a set amplitude (the lamp of the next slice, brought forward, or a fixture `clicker`)
* red tests: `a_sound_carries_by_distance` (a device 2 m from a bot emits 60 dB: the bot receives `received_level_db` 53.979...; at 0 m, 100 dB, the 1 cm clamp); `a_participant_below_its_threshold_does_not_hear` (threshold 55 at 2 m: not delivered); `a_sound_too_quiet_for_everyone_reaches_nobody` (no send at all, not a send to all); `speech_and_host_sounds_have_no_origin_and_reach_everyone` (no level filled); `a_visual_reaches_every_participant`

### Hold Devices And Send Them Actions

* goal: a device document invoked by the environment once its holders are active, with `holders` and `device`; its `device.offers` reaches each holder; a focused device's choices join the turn's choices; a selected device action goes straight to the device, which takes it only from a holder; the device's `bot.input` reaches its holders with `target_device` stamped
* files: `tests/fixtures/environment/lamp/lamp.scx` (a fixture device with `lamp.on`/`lamp.off` and their schemas; no real device is ported, devices stay out of scope otherwise), `environments/room/room.scx` (`holdings`), `bots/floppy/floppy.scx` and `floppy.js`, the `device.offers` schema and twins, AGENTS.md's devices rule, `tests/tests/environment.rs`, `tests/tests/body.rs`
* red tests: `a_held_devices_input_grounds_the_turns_focus` (the lamp's `bot.input` reaches the bot with `target_device` `lamp`; `floppy.focus` `lamp`); `a_focused_devices_choices_are_offered_and_a_selection_reaches_it` (focused on the lamp, the cognition double selects `lamp.on`; the lamp logs it and its next `device.offers` reports state `on`); `a_device_takes_actions_only_from_its_holders` (a second bot that does not hold the lamp sends `lamp.on` to its address: ignored); `a_device_action_the_device_refuses_is_an_error_in_the_sender` (data the lamp's schema refuses: `error.execution` in the bot, no delivery); `a_device_is_invoked_after_its_holders_activate`

### Show The Model Its World

* goal: `<bot:world>` renders the original's block; the body sends it as `world` on `bot.ability.cognition.input`; reasoning puts it in its system message and intuition in its decision instructions
* files: `namespaces/bot` (`world`), `bots/floppy/floppy.scx`, `floppy.js`, `abilities/cognition/`, `abilities/reasoning/`, `abilities/intuition/`, the `bot.ability.cognition.input` schema and twins, `tests/tests/cognition.rs`, the namespace's unit tests
* red tests: the element's unit tests (the exact XML for a bot with no devices, with the lamp, a device without a live offer dropped, a nested attribute mapping, a `null` attribute omitted, attribute values escaped); `reasonings_prompt_carries_the_environment_block` (a fake generate backend records the prompt: its system message holds `<environment id="session"><self state="processing"><owned_devices><device ref="lamp" id="..." state="off"/>`)

### Prove Two Participants Live

* goal: two floppy bots in one environment on the maintainer's backends: `a` is asked (a communication input with `target_ids` `["a"]`) to say something, speaks it, `b` hears it, decodes it and takes a turn on it, and `a` does not take its own speech as a turn
* files: `tests/tests/environment_live.rs` (ignored unless the live env files are present, as `floppy_live.rs`), `docs/live-proof.md`
* needs: `FLOPPY_BASE_URL`, `FLOPPY_API_KEY`, `FLOPPY_SPEECH_BASE_URL`, `FLOPPY_SPEECH_API_KEY`, and `FLOPPY_VOICE_BASE_URL` (without a voice backend the listening fails `voice_detection` and hands over no product, D98), with the speech service's `--dir` media grant shared by both bots
* done when: one recorded run passes (each bot's records, the environment's broadcasts, `b`'s `bot.processing.completed` on `a`'s words) and is entered in docs/live-proof.md with its model and backends; a failure is recorded as it happened

### Document The Environment

* goal: decisions D102 to D108 into docs/decisions.md, AGENTS.md (the run command; the devices line: "A bot's devices MUST be SCXML documents its environment invokes; any I/O a device does MUST be an invoked service inside it"; `environments/` and `devices/` in the layout list), docs/port-plan.md (an Environment row in the concept map, the Devices row, conventions for broadcast and placement), docs/handoff.md, README.md, `bots/floppy/README.md`, `environments/room/README.md`

## Decisions To Add

The maintainer decided these on 2026-10-12; D101 is the latest, so they are D102 to D108.

* D102: a bot always runs under an environment: the environment is an SCXML document run as the root session (`environments/<name>/<name>.scx`) whose invoked children are its participants; join is an invoke starting, leave is the invoke ending (cancelled, done, or failed); a bot's abilities are never participants (the original's private scope). There is no bot-as-root path: the body sends every event it emits to `#_parent`, and the environment, the root, logs them. The run command is `scx run ... environments/<name>/<name>.scx`.
* D103: speaking puts its sound straight into the environment (`environment.sound` to `#_scxml_<environment>`), and a device to its parent; the environment delivers every `environment.sound` and `environment.visual` it receives to each participant that perceives it, the emitter included, with the original's free-field law (1 m reference, 1 cm clamp), threshold and `received_level_db`, the origin being the sender's placement when the sender is a placed child (read from `_event.invokeid` in an `expr`). No envelope event: host and participant sounds are both broadcast, and the origin needs no field (see Why There Is No Envelope Event).
* D104: scxml.rs reaches any session of the same run by `#_scxml_<sessionid>` and raises `error.communication` for any other id (SCXML 1.0 C.1 leaves the set to the platform); done upstream first on `feat/invoke-services`, with no term beyond the spec's.
* D105: devices are SCXML documents the environment invokes (`devices/<name>/<name>.scx`); any real I/O a device does is an invoked service inside it (AGENTS.md's devices line is amended; "I/O must be a service" stays). A device knows its holders (`holders`, the addresses the environment learns from each holder's `bot.activated`, so a device is invoked once its holders are active) and its reference (`device`); it reports `device.offers` to its holders; a holder sends it actions straight to its address, and the device takes an action only from a holder. That check is a `cond` on `_event.origin`, an exception to D81 alongside D81's and D82's own: it authorizes, it does not route, and SCXML cannot express it otherwise once every session of the run is reachable.
* D106: the environment's id is the root's `_sessionid` (`scx run --session-id`, default `session`); there is no separate id. The model sees the original's world block, rendered by `<bot:world>` from that id, the body's configuration and the held devices' latest offers, sent as `world` on `bot.ability.cognition.input` and put by reasoning and intuition in their system instructions.
* D107: a bot activates on join: its activation settings are its `<invoke>`'s `<param>`s (`model`, `bounds`, `database`, `devices`, `environment`), and the body starts in `live`; the environment sends no `bot.activate`. Since nothing can activate the bot again, `inactive` goes: a failed activation and `bot.deactivate` end the bot's session, its leave; rejoining is the environment invoking it again (the original's detach and a new attach).
* D108: host input enters the environment and is routed by what its payload already says: `bot.ability.communication.input` to each participant its `target_ids` names (`targetexpr`), to every bot when empty; untargeted `environment.sound` and `environment.visual` are broadcast; `bot.input` and `bot.deactivate`, whose payloads name no target, go to every bot. No envelope event and no scxml.rs input change.

## What The Decisions Changed Or Broke

* D81 needs an exception: the device's holder check is a `cond` on who sent the action (D105)
* activation ordering: a device can be invoked only after its holders have activated, because it needs their addresses; a bot's turn can name a device before its offer arrives, which focus already tolerates (no candidates without the device's input)
* `inactive` and `bot.activate` leave the body, and with them re-activation; every test that sends `bot.activate` (with `model` or `bounds`) moves its settings into the environment launcher's `<param>`s
* the body's emitted events leave its own log for the environment's; a test reading a label across two bots filters by the record's `session`
* host `bot.input` and `bot.deactivate` carry no target, so they reach every bot in a multi-bot environment; a test that needs one bot uses a one-bot environment, or a device's `bot.input`
* `target_ids` now names environment participants (their invoke ids); the slice checks what the conversation does with a `target_ids` that names the bot itself (it reads `target_ids` today as the addressee)
* `<bot:stimulus source>` reads `_event.origin` for device input, so the stimulus's `source` is a session address, not an invoke id
* the scxml.rs subtree pull for D104 also brings whatever else `feat/invoke-services` holds by then

## Names To Confirm

Every new name; none is used before the maintainer approves it or gives another. Every other name in this plan is the original's, the spec's, or one floppy already has.

* directories and documents: `environments/<name>/<name>.scx` (with `<name>.js`, `schemas/`, `README.md`); `environments/room/room.scx` (the default environment, one floppy); `devices/<name>/<name>.scx`
* fixtures and tests: `tests/fixtures/environment/two_bots.scx`, `tests/fixtures/environment/lamp/lamp.scx` with `lamp.on` and `lamp.off`, `tests/tests/environment.rs`, `tests/tests/environment_live.rs`
* event [floppy name]: `device.offers {device, state, attributes, choices}`
* params: `environment` (to a bot, and from the body to the communication and from the communication to speaking); `device` and `holders` (to a device)
* the environment's `<data>`: `participants`, `placements` (`{position: {x, y, z}, threshold_db}`), `holdings`
* the body's `<data>`: `device_offers`; the id of the top-level final a deactivated or failed bot ends in (`inactive`, kept as a final, is the candidate that adds no name)
* field [floppy field]: `world` on `bot.ability.cognition.input`
* namespace element: `<bot:world location environment devices/>` in `namespaces/bot`
* scxml.rs: the `session` case of the I/O processor WIT's `send-target`, `dispatch-decision` and `due-route`
* functions in `environments/room/room.js`: `receivedLevelDb`, `hears`
