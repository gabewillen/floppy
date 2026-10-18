# Parity with the original's latest

These are the original's commits floppy has not ported, measured on 2026-10-18 against the original's HEAD `0134485` (2026-10-17). The only sync point floppy records is `5f37c5c` (2026-10-07; [phase4-bot-core](phase4-bot-core.mdscript.md)), where the phase 4 schemas were extracted. The original's commits from 10-01 to 10-07 (persistent routines, migrations) are ported. The 92 commits from `a809c8b` to `0134485` (10-13 to 10-17) are not.

[original-domains.md](original-domains.md) only renames and moves logic floppy already has, so none of these gaps is part of it.

## Gaps

| # | Gap | The original | floppy today | Size | Status |
|---|---|---|---|---|---|
| 1 | Recall the most recent window of memories, oldest first within it | `abilities/memory/store.py` `recent_select` (`30f638e`, `2443929`) | Every recall returns the oldest 50 rows (`ORDER BY created_at, memory_id LIMIT 50` in reasoning, conversation, identity, reflection) | S | to do |
| 2 | A revision keeps the behavior's own triggers as evidence | `cognition/reflection/revision.py` (`325ff36`) | `evidenceStimulusNames()` in `abilities/reflection/reflection.js` leaves out `existing_behavior.triggers` | S | to do |
| 3 | Trim recalled memory to the prompt budget instead of failing the turn | `cognition/event.py`, `processing.py` (`b1654d6`) | `reasoning.js` throws "budget exceeded"; lands in the map's J1 | M | after J1 |
| 4 | Stimuli and cognition inputs carry producer-stamped `participants` | `f6c4e08` | Absent from `bot.ability.cognition.input` and the room's sound and visual schemas | M | to do |
| 5 | Lifecycle requests answered to their activator; `bot.degraded` announced | `bot.py` (`a2cbaa8`) | `degraded` ends with donedata only | M | to do |
| 6 | `bot.ability.speaking.stop`, with completed, stopped and abandoned outcomes | `speaking/speaking.py`, `communication.py` (`fb91ed3`) | No stop | M | to do |
| 7 | Replies as structured parts with optional expression; per-part reports | `vocal/speech/part.py` (`ab2e75b`, `a497d82`, `e6ed881`) | `respond` is text only | M | to do |
| 8 | Persona: owner-given traits, a self-description shown every turn, revisable by reflection | `abilities/persona/*` (`edc93ec`, `d550e7d`, `c5b45bf`, `d327bf5`) | None | L | to do |
| 9 | Episodic graph memory with vectors, associative recall, `bot.ability.memory.recall`, consolidation while resting | `memory/{graph,associative,consolidation,index}.py` (`28cc70e`, `acebeb0`, `3d52ac7`, `c183a7a`, `4a4c2f3`) | Not ported; no rest state | L | needs decisions 3, 7 |
| 10 | Embedding ability (text and image) and a remote embedder | `abilities/embedding/*` (`50d89be`, `67f9471`) | Voice embeddings only | M | with 9 |
| 11 | Streaming speech: streaming TTS, drafts spoken while generating, streaming STT with end of turn | `c88b6cd`, `9782dfd`, `c8ec365`, `1cc7f9d`, `3fd81ac`, `86c1d69` | Whole-file encode and decode | L | to do |
| 12 | Hearing: identity refs through an identification registry, overlap while speaking, tone, own-voice enrollment | `identification/*`, `listening/{overlap,tone,enrollment}.py` (`78ab958` and others) | Embeddings only, no registry | L | needs decision 6 |
| 13 | Multimodal content: images as placeholders and attached parts, turn attachments, typed `VisualData.image` | `media.py` (`1360749`, `cf8d41d`, `0d0f5e2`, `82543ff`) | generate accepts an image part (D58); `turn_detector.js` keeps the `image/*` branch the original deleted | M | partly after J1 |
| 14 | Vision: change measures, the `seeing` ability, faces, objects, face identity | `vision/*`, `seeing/seeing.py` (`0d77480` and others) | A rule classifier only | L | to do |
| 15 | Input abilities are cognition-addressable | `bot.py` (`a085af9`) | Probably present (listening offers); to verify | S | to verify |
| 16 | Each stage narrows the frame to its own effort ceiling | `cognition/input.py` `within_effort` (`e7f1f59`) | No effort model | M | needs decision 8 |
| 17 | Sandbox ability and a Docker or Podman host | `abilities/sandbox/*` (`c8aa3d6` and others) | None | L | needs decision 5 |
| 18 | Devices: computer, HID keyboard and pointer, audio microphone and speaker; phone by message; owner-command routing in Device | `devices/*`, `device/device.py` (`f42a279` and others) | None ported (the lamp fixture only) | L | needs decision 5 |
| 19 | Example `builder_bot` | `examples/builder_bot` (`59c17f0`, `0134485`) | None; depends on 8, 17, 18 | L | needs decision 4 |
| 20 | Examples `minimal_bot`, `listen_speak_bot`, `phone_bot`, `sms_chat_bot` | `examples/*` (`b332869`, `24c29f9`) | None | M each | after their abilities |
| 21 | Voice catalogs, design and inception | `vocal/voice/*` (`ee601e9`, `8122301`, `16eac51`) | Encode only | M | to do |
| 22 | Process runtime, CLI, Docker image | `runtime/*`, `cli.py` | Not applicable (D14) | — | decision 9 |
| 23 | Transform codecs, transport peers, yamux over WebSocket, a LiveKit room | `protocols/*` | Not ported (D6) | L | needs decision 5 |
| 24 | Telemetry metrics; never capture biometric values | `telemetry/*` (`58d4841`, `f8e40b3`) | To check: interpretation logs `voice_embedding` | S | to verify |

## Decisions for the maintainer

1. **Sync baseline:** record `0134485` as the commit floppy tracks, and decide whether to re-extract the phase 4 schemas from it.
2. **Postgres:** the original deleted its Postgres memory store (`b040355`). Should floppy keep `services/pgsql` and D69–D72?
3. **Migrations:** the original rewrote its `0001` in place (graph and identity tables). Should floppy follow that, which breaks existing databases, or add a `0003`?
4. **The name "builder":** the original's new `builder_bot` is a user-facing bot. floppy's `bots/builder` is the behavior author. One of them needs another name.
5. **Devices and transport scope:** the plan and handoff keep them out of scope; items 17, 18, 19 and 23 need them.
6. **Participant identity:** D79 conflicts with the original's identification registry and `voice:`/`face:` refs.
7. **Rest and consolidation:** port cognition's rest state?
8. **Effort ceilings:** port `within_effort`?
9. **Container image:** is a Docker image for `scx run` wanted?
