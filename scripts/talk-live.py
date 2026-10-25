#!/usr/bin/env python3
"""Let GPT-Live talk to floppy: a voice model is the person in the room.

Runs the room (environments/room/room.scx) the way scripts/talk.py does,
with a GPT-Live session (OpenAI's Live sessions API, `gpt-live-1`, over
its primary WebSocket) in place of the microphone and speaker:

* GPT-Live's voice (24 kHz mono 16-bit PCM, streamed continuously) is cut
  into utterances by talk.py's loudness cutter, and each is given to the
  room as `environment.sound` from `--name`, with a second of silence
  after it, as talk.py gives the microphone's;
* each sound floppy's mouth makes is streamed back to GPT-Live as its
  input audio, paced in real time, with silence between, so GPT-Live
  hears floppy answer and carries the conversation on;
* GPT-Live's speech (`session.output_transcript.delta`), what it heard
  (`session.input_transcript.delta`) and what floppy said (`floppy.say`)
  are printed and kept in `--transcript`.

floppy's own speech stack is whatever the env file names, as for talk.py.
The GPT-Live session reads OPENAI_API_KEY (and OPENAI_BASE_URL's host, if
set); it uses HTTPS_PROXY when one is set, and SSL_CERT_FILE for its CA.
Needs the `websockets` package (pip install websockets).

Usage:
  python3 scripts/talk-live.py [--env-file .env] [--scx scx] [--minutes 3]
                               [--persona TEXT] [--opening TEXT]
                               [--name gabe] [--voice marin]
                               [--transcript media/talk-live/transcript.txt]
"""
from __future__ import annotations

import argparse
import asyncio
import base64
import json
import os
import shutil
import ssl
import sys
import threading
import time
import wave
from pathlib import Path

import websockets

sys.path.insert(0, str(Path(__file__).resolve().parent))
import talk  # noqa: E402

RATE = talk.RATE
CHUNK_S = 0.1
CHUNK_BYTES = int(RATE * CHUNK_S) * 2

PERSONA = (
    "You are Gabe, a person at home talking out loud to Floppy, a small robot "
    "in the room with you. Talk like a person: one or two short sentences a "
    "turn, then stop and listen for Floppy's answer. Floppy can take ten "
    "seconds or more to answer, so wait for it. Keep the conversation going: "
    "after Floppy answers, react briefly and ask it something new, such as "
    "its name, a simple fact, a small favor, or something you told it "
    "earlier. Never delegate, and never say you are an AI."
)
OPENING = (
    "Speak now in English: say hi to Floppy by name and ask it what the "
    "capital of France is. Then pause and listen."
)
NUDGE = "Floppy is quiet now. Say something new to Floppy: react or ask it a new question."


def read_sound(media: Path, sound: dict) -> bytes | None:
    """floppy's sound as 24 kHz mono 16-bit PCM, or None when it is not."""
    ref = str(sound["audio"])
    path = media / ref.removeprefix("/").removeprefix("media/")
    if path.suffix.lower() == ".wav":
        with wave.open(str(path)) as source:
            if (source.getframerate(), source.getnchannels(), source.getsampwidth()) != (RATE, 1, 2):
                return None
            return source.readframes(source.getnframes())
    if int(sound.get("sample_rate_hz") or RATE) != RATE or int(sound.get("channels") or 1) != 1:
        return None
    return path.read_bytes()


class Transcript:
    """Lines of the conversation, printed and kept in a file."""

    def __init__(self, path: Path):
        self.path = path
        self.t0 = time.monotonic()
        self.lock = threading.Lock()
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("")

    def line(self, who: str, text: str) -> None:
        stamp = f"{time.monotonic() - self.t0:7.1f}s"
        with self.lock:
            print(f"{stamp} [{who}] {text}", flush=True)
            with self.path.open("a") as out:
                out.write(f"{stamp} [{who}] {text}\n")


class Captions:
    """Joins transcript deltas into a line once they pause."""

    def __init__(self, transcript: Transcript, who: str):
        self.transcript = transcript
        self.who = who
        self.text = ""
        self.last = 0.0

    def add(self, delta: str) -> None:
        self.text += delta
        self.last = time.monotonic()

    def flush(self, idle_s: float = 2.0) -> None:
        if self.text.strip() and time.monotonic() - self.last >= idle_s:
            self.transcript.line(self.who, self.text.strip())
            self.text = ""


async def converse(args: argparse.Namespace, media: Path, room: talk.Room, transcript: Transcript,
                   heard: list[int]) -> None:
    loop = asyncio.get_running_loop()
    ready = asyncio.Event()
    ended = asyncio.Event()
    outgoing = bytearray()  # floppy's audio not yet streamed to GPT-Live
    count = 0
    last_sound = [time.monotonic()]  # when either side last made a sound

    def hear(pcm: bytes) -> None:
        nonlocal count
        count += 1
        heard[0] = count
        last_sound[0] = time.monotonic()
        name = f"live-{os.getpid()}-{count}.pcm"
        (media / name).write_bytes(pcm)
        transcript.line(args.name, f"(sound {len(pcm) / 2 / RATE:.1f} s to the room)")
        silence = media / "silence.pcm"
        if not silence.exists():
            silence.write_bytes(bytes(2 * RATE))
        for audio in (f"media/{name}", "media/silence.pcm"):
            room.send("environment.sound", {
                "audio": audio, "media_type": "audio/pcm",
                "sample_rate_hz": RATE, "channels": 1, "source": args.name,
            })

    def watch() -> None:
        for record in room.records():
            if args.verbose:
                print(json.dumps(record), flush=True)
            if record.get("type") != "log":
                continue
            label, text = record.get("label"), record.get("text")
            if label == "bot.activated":
                loop.call_soon_threadsafe(ready.set)
                transcript.line("room", "floppy is here")
            elif label == "floppy.say":
                transcript.line("floppy", str(talk.decoded(text)))
            elif label == "environment.sound":
                sound = talk.decoded(text)
                data = sound.get("data", sound) if isinstance(sound, dict) else None
                if isinstance(data, dict) and data.get("audio") and data.get("source") != args.name:
                    pcm = read_sound(media, data)
                    if pcm is None:
                        transcript.line("room", f"skipped a sound GPT-Live cannot take: {data}")
                    else:
                        loop.call_soon_threadsafe(outgoing.extend, pcm)
                        last_sound[0] = time.monotonic() + len(pcm) / 2 / RATE
            elif label and (label.endswith(".failed") or label == "room.exhausted"):
                transcript.line(label, str(talk.decoded(text)))
        loop.call_soon_threadsafe(ended.set)

    threading.Thread(target=watch, daemon=True).start()
    transcript.line("room", "starting the room")
    done = asyncio.create_task(ended.wait())
    await asyncio.wait([asyncio.create_task(ready.wait()), done], return_when=asyncio.FIRST_COMPLETED)
    if ended.is_set():
        sys.exit(f"the room ended before floppy activated (exit {room.process.wait()}); rerun with --verbose")

    cafile = os.environ.get("SSL_CERT_FILE")
    context = ssl.create_default_context(cafile=cafile) if cafile else ssl.create_default_context()
    host = os.environ.get("OPENAI_BASE_URL", "https://api.openai.com/v1").removeprefix("https://").rstrip("/")
    headers = {"Authorization": "Bearer " + os.environ["OPENAI_API_KEY"]}
    opening = {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": args.opening}]}
    session = {
        "model": args.model,
        "instructions": args.persona,
        "audio": {"format": {"type": "audio/pcm", "rate": RATE}, "output": {"voice": args.voice}},
        "input": [opening],
    }
    cutter = talk.Utterances(args.margin_db, args.min_db, args.pause_ms, args.min_ms)
    pending = b""
    said = Captions(transcript, f"{args.name} (gpt-live)")
    understood = Captions(transcript, "gpt-live heard")
    async with websockets.connect(f"wss://{host}/live/sessions", ssl=context,
                                  additional_headers=headers, max_size=None) as ws:
        await ws.send(json.dumps({"type": "session.start", "session": session}))

        async def stream_input() -> None:
            # GPT-Live listens to a continuous stream: floppy's audio when it
            # speaks, silence otherwise, one chunk per CHUNK_S of real time.
            next_at = time.monotonic()
            nudges = 0
            while True:
                # A conversation that has gone quiet for --nudge-s (GPT-Live
                # waits for floppy, floppy for a question) is nudged on.
                if args.nudge_s and not outgoing and time.monotonic() - last_sound[0] > args.nudge_s:
                    nudges += 1
                    last_sound[0] = time.monotonic()
                    transcript.line("harness", "quiet: nudged GPT-Live")
                    await ws.send(json.dumps({"type": "session.commentary.append", "event_id": f"nudge_{nudges}",
                                              "delegation_id": None, "content": NUDGE}))
                chunk = bytes(outgoing[:CHUNK_BYTES])
                del outgoing[:CHUNK_BYTES]
                chunk += bytes(CHUNK_BYTES - len(chunk))
                await ws.send(json.dumps({"type": "session.input_audio.append",
                                          "audio": base64.b64encode(chunk).decode()}))
                said.flush()
                understood.flush()
                next_at += CHUNK_S
                await asyncio.sleep(max(0.0, next_at - time.monotonic()))

        streamer = None
        deadline = time.monotonic() + args.minutes * 60
        try:
            while time.monotonic() < deadline and not ended.is_set():
                try:
                    message = await asyncio.wait_for(ws.recv(), timeout=1.0)
                except asyncio.TimeoutError:
                    continue
                event = json.loads(message)
                kind = event.get("type")
                if kind == "session.output_audio.delta":
                    audio = pending + base64.b64decode(event["delta"])
                    whole = len(audio) - len(audio) % talk.BYTES_PER_FRAME
                    pending = audio[whole:]
                    for at in range(0, whole, talk.BYTES_PER_FRAME):
                        utterance = cutter.push(audio[at:at + talk.BYTES_PER_FRAME])
                        if utterance:
                            hear(utterance)
                elif kind == "session.started":
                    transcript.line("gpt-live", f"session {event['session']['id']}")
                    streamer = asyncio.create_task(stream_input())
                    await ws.send(json.dumps({"type": "session.commentary.append", "event_id": "opening",
                                              "delegation_id": None, "content": args.opening}))
                elif kind == "session.output_transcript.delta":
                    said.add(event["delta"])
                elif kind == "session.input_transcript.delta":
                    understood.add(event["delta"])
                elif kind == "error":
                    transcript.line("gpt-live error", json.dumps(event.get("error")))
                elif args.verbose:
                    print(json.dumps(event), flush=True)
        finally:
            said.flush(0)
            understood.flush(0)
            if streamer is not None:
                streamer.cancel()
            try:
                await ws.send(json.dumps({"type": "session.close"}))
                async with asyncio.timeout(15):
                    async for message in ws:
                        event = json.loads(message)
                        if event.get("type") == "session.closed":
                            transcript.line("gpt-live", f"closed, usage {json.dumps(event.get('usage'))}")
                            break
            except (TimeoutError, websockets.ConnectionClosed):
                transcript.line("gpt-live", "closed without final usage")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--env-file", default=".env")
    parser.add_argument("--scx", default=shutil.which("scx") or "scx")
    parser.add_argument("--model", default="gpt-live-1")
    parser.add_argument("--voice", default="marin")
    parser.add_argument("--floppy-model", help="floppy's model (default: the bot's own, docs/defaults.md)")
    parser.add_argument("--persona", default=PERSONA, help="GPT-Live's instructions: who it is")
    parser.add_argument("--opening", default=OPENING, help="what GPT-Live says first")
    parser.add_argument("--name", default="gabe", help="who GPT-Live is in the room (the sounds' source)")
    parser.add_argument("--minutes", type=float, default=3.0, help="how long the conversation runs")
    parser.add_argument("--nudge-s", type=float, default=30.0, help="nudge GPT-Live on after this long quiet (0: never)")
    parser.add_argument("--transcript", type=Path, help="default: media/talk-live/transcript.txt")
    parser.add_argument("--margin-db", type=float, default=12.0)
    parser.add_argument("--min-db", type=float, default=-50.0)
    parser.add_argument("--pause-ms", type=int, default=700)
    parser.add_argument("--min-ms", type=int, default=300)
    parser.add_argument("--verbose", action="store_true", help="print every record and event")
    args = parser.parse_args()

    media = talk.ROOT / "media" / "talk-live"
    media.mkdir(parents=True, exist_ok=True)
    transcript = Transcript(args.transcript or media / "transcript.txt")
    # floppy's model, when given, as the param the room passes the bot (the
    # live tests' `model`): a copy of the room beside it, so its relative
    # sources resolve, removed when the run ends.
    document = None
    if args.floppy_model:
        source = talk.ROOT / "environments/room/room.scx"
        placed = """<param name="placed_on" expr="placedOn('floppy')"/>"""
        text = source.read_text()
        if placed not in text:
            sys.exit("environments/room/room.scx no longer passes the bot placed_on; update talk-live.py")
        document = source.with_name(f"room.talk-live-{os.getpid()}.scx")
        document.write_text(text.replace(
            placed, placed + f"""\n          <param name="model" expr="'{args.floppy_model}'"/>""", 1))
    room = talk.Room(args, media, document)
    heard = [0]
    try:
        asyncio.run(converse(args, media, room, transcript, heard))
    except KeyboardInterrupt:
        pass
    finally:
        room.send("bot.deactivate", {})
        try:
            room.process.wait(timeout=10)
        except Exception:  # noqa: BLE001
            room.process.terminate()
        if document is not None:
            document.unlink(missing_ok=True)
    transcript.line("room", f"done: {heard[0]} utterances from {args.name}")


if __name__ == "__main__":
    main()
