#!/usr/bin/env python3
"""Talk to floppy live from a Mac: microphone in, speaker out.

Runs the environment (environments/environment/environment.scx) under `scx run` and is its host:

* the microphone, through ffmpeg (avfoundation), is read as 24 kHz mono
  16-bit PCM; each utterance (cut by loudness: speech over the environment's noise
  floor, ended by a pause) is written to the media directory and given to
  the environment as `environment.sound`, the way the live tests give a recorded
  one (tests/tests/floppy_live.rs, `live_listening`);
* each sound floppy's mouth makes (an `environment.sound` from the bot, as
  the environment logs it) is played on the speaker with afplay, and the
  microphone is muted while it plays, so floppy does not hear itself;
* a line typed on the terminal is sent as text
  (`bot.ability.communication.input`), and `--say FILE` sends a recorded
  utterance (24 kHz mono 16-bit PCM or WAV) instead of the microphone.

Speech goes to whatever the env file names: FLOPPY_SPEECH_BASE_URL and
FLOPPY_VOICE_BASE_URL (the lab's https://api.willen.dev/v1), FLOPPY_BASE_URL (chat),
TYPESAFE_BASE_URL (decisions). Standard library only; needs ffmpeg (ffmpeg
for the microphone, ffplay for the speaker).

Usage:
  python3 scripts/talk.py [--env-file .env] [--scx scx] [--mic :0]
                          [--say FILE] [--name gabe] [--verbose]
"""
from __future__ import annotations

import argparse
import array
import json
import math
import os
import queue
import shutil
import subprocess
import sys
import threading
import time
import wave
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SERVICES = "https://github.com/stateforward/floppy/services"
RATE = 24000
FRAME = RATE // 50  # 20 ms
BYTES_PER_FRAME = FRAME * 2


def frame_db(frame: bytes) -> float:
    samples = array.array("h", frame)
    if not samples:
        return -100.0
    power = sum(s * s for s in samples) / len(samples)
    return 10 * math.log10(power / (32768.0 * 32768.0) + 1e-12)


class Utterances:
    """Cuts a PCM stream into utterances by loudness over a tracked noise floor."""

    def __init__(self, margin_db: float, min_db: float, pause_ms: int, min_ms: int):
        self.margin_db = margin_db
        self.min_db = min_db
        self.pause_frames = pause_ms // 20
        self.min_frames = min_ms // 20
        self.floor = -60.0
        self.preroll: list[bytes] = []
        self.voiced: list[bytes] = []
        self.loud_run = 0
        self.quiet_run = 0
        self.speaking = False

    def reset(self) -> None:
        self.preroll.clear()
        self.voiced.clear()
        self.loud_run = self.quiet_run = 0
        self.speaking = False

    def push(self, frame: bytes) -> bytes | None:
        db = frame_db(frame)
        loud = db > max(self.floor + self.margin_db, self.min_db)
        if not self.speaking:
            # The floor follows quiet frames quickly and loud ones slowly.
            self.floor += (db - self.floor) * (0.05 if db < self.floor + 3 else 0.002)
            self.preroll = (self.preroll + [frame])[-15:]  # 300 ms
            self.loud_run = self.loud_run + 1 if loud else 0
            if self.loud_run >= 4:
                self.speaking = True
                self.voiced = list(self.preroll)
                self.quiet_run = 0
            return None
        self.voiced.append(frame)
        self.quiet_run = 0 if loud else self.quiet_run + 1
        if self.quiet_run < self.pause_frames:
            return None
        voiced = self.voiced[: len(self.voiced) - self.pause_frames + 10]  # keep 200 ms of the pause
        enough = len(voiced) - 15 >= self.min_frames
        self.reset()
        return b"".join(voiced) if enough else None


class Speaker:
    """Plays floppy's sounds in order with no gap between chunks: 24 kHz mono
    PCM goes to one ffplay reading its standard input, anything else to
    afplay. `busy` while the audio written is still playing."""

    def __init__(self, media: Path, tail_s: float):
        self.media = media
        self.tail_s = tail_s
        self.queue: queue.Queue[dict] = queue.Queue()
        self.until = 0.0
        self.player = subprocess.Popen(
            ["ffplay", "-hide_banner", "-loglevel", "error", "-nodisp", "-autoexit",
             "-f", "s16le", "-ar", str(RATE), "-ch_layout", "mono", "-"],
            stdin=subprocess.PIPE,
        )
        threading.Thread(target=self.run, daemon=True).start()

    def busy(self) -> bool:
        return not self.queue.empty() or time.monotonic() < self.until + self.tail_s

    def play(self, sound: dict) -> None:
        self.queue.put(sound)

    def run(self) -> None:
        while True:
            sound = self.queue.get()
            try:
                self.play_one(sound)
            except Exception as exc:  # a bad file must not stop the speaker
                print(f"[speaker] {exc}", file=sys.stderr)

    def play_one(self, sound: dict) -> None:
        ref = str(sound["audio"])
        path = self.media / ref.removeprefix("/").removeprefix("media/")
        pcm = (sound.get("media_type") in ("audio/pcm", "audio/l16") or path.suffix == ".pcm")
        if pcm and int(sound.get("sample_rate_hz") or RATE) == RATE and int(sound.get("channels") or 1) == 1:
            audio = path.read_bytes()
            start = max(time.monotonic(), self.until)
            self.until = start + len(audio) / 2 / RATE
            assert self.player.stdin is not None
            self.player.stdin.write(audio)
            self.player.stdin.flush()
            return
        while time.monotonic() < self.until:
            time.sleep(0.05)
        if pcm:
            wav = path.with_suffix(".play.wav")
            with wave.open(str(wav), "wb") as out:
                out.setnchannels(int(sound.get("channels") or 1))
                out.setsampwidth(2)
                out.setframerate(int(sound.get("sample_rate_hz") or RATE))
                out.writeframes(path.read_bytes())
            path = wav
        self.until = float("inf")
        try:
            subprocess.run(["afplay", str(path)], check=False)
        finally:
            self.until = time.monotonic()


class Environment:
    """`scx run` of the environment, fed on standard input, read on standard output."""

    def __init__(self, args: argparse.Namespace, media: Path, document: Path | None = None):
        grants = []
        for service in ("generate", "speech", "vision"):
            grants += ["--dir", f"{SERVICES}/{service}={media}::/media"]
        memory = ROOT / "memory"
        behaviors = ROOT / "behaviors"
        memory.mkdir(exist_ok=True)
        behaviors.mkdir(exist_ok=True)
        command = [
            args.scx, "run", "--offline",
            "--cache-dir", str(ROOT / "target/components/package-cache"),
            "--env-file", args.env_file,
            "--dir", f"{SERVICES}/sql={memory}::/memory",
            "--wasi", f"{SERVICES}/pgsql=inherit-network",
            *grants,
            "--dir", f"{SERVICES}/files={behaviors}::/behaviors",
            "--fuel", "9223372036854775807",
            str(document or ROOT / "environments/environment/environment.scx"),
        ]
        self.process = subprocess.Popen(
            command, cwd=ROOT, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1
        )
        self.lock = threading.Lock()

    def send(self, name: str, data: dict) -> None:
        """Sends an event to the environment; once the environment has exited it is dropped."""
        with self.lock:
            assert self.process.stdin is not None
            try:
                self.process.stdin.write(json.dumps({"name": name, "data": data}) + "\n")
                self.process.stdin.flush()
            except (BrokenPipeError, ValueError):
                pass

    def records(self):
        assert self.process.stdout is not None
        for line in self.process.stdout:
            try:
                yield json.loads(line)
            except json.JSONDecodeError:
                continue


def decoded(text) -> object:
    if isinstance(text, str):
        try:
            return json.loads(text)
        except json.JSONDecodeError:
            return text
    return text


def read_pcm(path: Path) -> bytes:
    if path.suffix.lower() == ".wav":
        with wave.open(str(path)) as source:
            if (source.getframerate(), source.getnchannels(), source.getsampwidth()) != (RATE, 1, 2):
                sys.exit(f"{path}: needs 24 kHz mono 16-bit PCM")
            return source.readframes(source.getnframes())
    return path.read_bytes()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--env-file", default=".env")
    parser.add_argument("--scx", default=shutil.which("scx") or "scx")
    parser.add_argument("--mic", default=":0", help="ffmpeg avfoundation input (`ffmpeg -f avfoundation -list_devices true -i ''`), or `none` to type only")
    parser.add_argument("--say", type=Path, help="send this utterance instead of listening to the microphone")
    parser.add_argument("--name", default="gabe", help="who is talking (the sound's source and text's source_ids)")
    parser.add_argument("--margin-db", type=float, default=12.0, help="speech is this far over the noise floor")
    parser.add_argument("--min-db", type=float, default=-50.0, help="and at least this loud (dBFS)")
    parser.add_argument("--pause-ms", type=int, default=700, help="a pause this long ends an utterance")
    parser.add_argument("--min-ms", type=int, default=300, help="shorter sounds are ignored")
    parser.add_argument("--verbose", action="store_true", help="print every record the environment writes")
    args = parser.parse_args()

    media = ROOT / "media" / "talk"
    media.mkdir(parents=True, exist_ok=True)
    environment = Environment(args, media)
    speaker = Speaker(media, tail_s=0.3)
    ready = threading.Event()
    done = threading.Event()
    count = 0
    count_lock = threading.Lock()
    mic = None

    def hear(pcm: bytes) -> None:
        nonlocal count
        with count_lock:
            count += 1
            name = f"heard-{os.getpid()}-{count}.pcm"
        (media / name).write_bytes(pcm)
        print(f"[you] ({len(pcm) / 2 / RATE:.1f} s)", flush=True)
        # A second of silence after it, as the live tests give: the listening
        # ends the utterance on the next sound that has no voice in it.
        silence = media / "silence.pcm"
        if not silence.exists():
            silence.write_bytes(bytes(2 * RATE))
        for audio in (f"media/{name}", "media/silence.pcm"):
            environment.send("environment.sound", {
                "audio": audio, "media_type": "audio/pcm",
                "sample_rate_hz": RATE, "channels": 1, "source": args.name,
            })

    def watch() -> None:
        for record in environment.records():
            if args.verbose:
                print(json.dumps(record), flush=True)
            if record.get("type") != "log":
                continue
            label, text = record.get("label"), record.get("text")
            if label == "bot.activated":
                ready.set()
                print("[floppy is here: talk, or type a line]", flush=True)
            elif label == "floppy.say":
                print(f"[floppy] {decoded(text)}", flush=True)
            elif label == "environment.sound":
                sound = decoded(text)
                data = sound.get("data", sound) if isinstance(sound, dict) else None
                if isinstance(data, dict) and data.get("audio") and data.get("source") != args.name:
                    speaker.play(data)
            elif label and (label.endswith(".failed") or label in ("supervision.exhausted",)):
                print(f"[{label}] {decoded(text)}", flush=True)
        done.set()

    def typed() -> None:
        for line in sys.stdin:
            if line.strip():
                environment.send("bot.ability.communication.input", {
                    "source_ids": [args.name], "target_ids": ["floppy"],
                    "content": line.strip(), "content_type": "text/plain",
                })

    threading.Thread(target=watch, daemon=True).start()
    print("[starting the environment]", flush=True)
    while not ready.wait(0.5):
        if done.is_set():
            sys.exit(f"the environment ended before floppy activated (exit {environment.process.wait()}); rerun with --verbose")
    threading.Thread(target=typed, daemon=True).start()

    try:
        if args.say:
            hear(read_pcm(args.say))
        if args.say or args.mic == "none":
            done.wait()
            return
        mic = subprocess.Popen(
            ["ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "avfoundation", "-i", args.mic,
             "-ac", "1", "-ar", str(RATE), "-f", "s16le", "-"],
            stdout=subprocess.PIPE,
        )
        cutter = Utterances(args.margin_db, args.min_db, args.pause_ms, args.min_ms)
        assert mic.stdout is not None
        while not done.is_set():
            frame = mic.stdout.read(BYTES_PER_FRAME)
            if len(frame) < BYTES_PER_FRAME:
                sys.exit("the microphone stopped (does this terminal have microphone access?)")
            if speaker.busy():
                cutter.reset()
                continue
            utterance = cutter.push(frame)
            if utterance:
                threading.Thread(target=hear, args=(utterance,), daemon=True).start()
    except KeyboardInterrupt:
        pass
    finally:
        if mic is not None:
            mic.terminate()
            mic.wait()
        environment.send("bot.deactivate", {})
        try:
            environment.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            environment.process.terminate()
        if speaker.player.stdin is not None:
            try:
                speaker.player.stdin.close()
            except BrokenPipeError:
                pass
        speaker.player.terminate()


if __name__ == "__main__":
    main()
