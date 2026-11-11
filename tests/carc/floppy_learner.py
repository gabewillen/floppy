"""floppy as a Continual-ARC learner (https://github.com/unifyai/continual-arc).

The benchmark serves a stream of ARC grid puzzles whose tasks recur with fresh
inputs, and reads what a learner keeps against the same learner wiped before
every instance. This adapter is that learner's four methods over one running
floppy: the environment (`environments/environment/environment.scx`) under
`scx run`, fed text on standard input as `bot.ability.communication.input`
from `gabe`, as `scripts/talk.py` types a line, and read on standard output.

- Every message is the benchmark's own text protocol
  (`continual_arc_baselines.systems.protocol`): the rules on an instance's
  first message, the instance and the feedback since the last action, and
  one JSON action on the last line of the reply. A reply is the words of
  each `bot.ability.communication.respond` the turn selected (the
  `floppy.output` log, as `tests/eval/score.py` reads it), joined.
- A message is answered once its turn settles (`bot.ability.reflection.output`
  or a failure, as `tests/tests/floppy_eval.rs`'s SETTLED) and every lesson
  the learning ability opened meanwhile is learned or failed: what floppy
  learns from a turn is part of the turn.
- An instance ends with a closing message carrying its verdict, so the
  verdict reaches floppy's reflection while the instance is fresh.
- What floppy keeps is its memory (`memory/`) and its behaviors (the layout's
  `behaviors/`), both under the work directory; `reset` stops floppy and
  deletes them, which is the benchmark's reset reference.

Every model is hosted: in a scratch copy of the layout the bot's `model` and
`reflection_model` are both `model`, every model call names
the gateway's variables (`FLOPPY_REFLECTION_BASE_URL` / `_API_KEY` when an env
file sets them, else `FLOPPY_BASE_URL` / `_API_KEY`, as `.env.cpa` does), and nothing
streams (`stream=False`), since the gateway answers a streamed request with
keep-alive comments only. The lab's embedding, speech, voice and emotion
backends are left unconfigured (recall is by recency, and floppy replies in
text alone). The decision service is the env files' `TYPESAFE_*`. `FLOPPY_CARC_ENV` is one
env file or several joined by `:` (the last file's entry wins, as in `scx`):
`.env:.env.cpa` takes the decision key from `.env` and the gateway from
`.env.cpa`. The files are given to `scx` as they are; nothing here reads or
copies a value of them.

Usage (from the floppy checkout, with continual-arc and its baselines
installed in the same environment):

  FLOPPY_CARC_ENV=.env FLOPPY_CARC_MODEL=gpt-6-luna \
    continual-arc run --learner tests.carc.floppy_learner:FloppyLearner \
      --schedule stream.json --out runs/floppy-stream
"""

from __future__ import annotations

import json
import os
import queue
import shutil
import subprocess
import threading
import time
from pathlib import Path

from continual_arc.env import GiveUp, GiveUpFeedback, SubmitFeedback
from continual_arc.learner import BaseLearner
from continual_arc_baselines.systems import protocol

ROOT = Path(__file__).resolve().parent.parent.parent
SERVICES = "https://github.com/stateforward/floppy/services"
SETTLED = (
    "bot.ability.reflection.output",
    "cognition.reflection.failed",
    "bot.processing.failed",
    "floppy.degraded",
    "bot.activating.failed",
)
LESSON_OPEN = ("learning.recalled",)
LESSON_CLOSE = ("learning.learned", "learning.failed")
RESPOND = "bot.ability.communication.respond"
# The variables the layout's documents name, renamed: every model call goes to
# the hosted gateway (`<gateway>_BASE_URL` / `_API_KEY`), nothing streams, and
# the lab's own backends (embedding, speech, voices, emotion) are named by
# variables no env file sets, so they are unconfigured. The env files are
# passed to scx as they are (the last file's entry wins); no value of them is
# read or copied here.
def slots(gateway: str) -> dict[str, str]:
    return {
        **{f"{prefix}_{slot}": f"{gateway}_{slot}"
           for prefix in ("FLOPPY", "FLOPPY_REFLECTION") for slot in ("BASE_URL", "API_KEY")},
        "FLOPPY_STREAM": "FLOPPY_CARC_UNSET_STREAM",
        **{f"FLOPPY_{kind}_{slot}": f"FLOPPY_CARC_UNSET_{kind}_{slot}"
           for kind in ("EMBEDDING", "SPEECH", "VOICE", "EMOTION") for slot in ("BASE_URL", "API_KEY")},
    }


def read_names(path: Path) -> set[str]:
    """The variable names an env file sets (never its values)."""
    names = set()
    for line in path.read_text().splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            names.add(line.split("=", 1)[0].strip().removeprefix("export ").strip())
    return names


def reply_words(text: str) -> list[str] | None:
    try:
        output = json.loads(text).get("output")
    except (ValueError, AttributeError):
        return None
    if not isinstance(output, list):
        return None
    out = []
    for entry in output:
        if isinstance(entry, dict) and entry.get("event") == RESPOND:
            parts = (entry.get("data") or {}).get("parts") or []
            words = "".join(p.get("words", "") for p in parts if isinstance(p, dict))
            if words.strip():
                out.append(words)
    return out


class Floppy:
    """One running environment with floppy in it."""

    def __init__(self, layout: Path, memory: Path, media: Path, env_files: list[Path], scx: str, log: Path):
        command = [
            scx, "run", "--offline",
            "--cache-dir", str(ROOT / "target/components/package-cache"),
            *[arg for env_file in env_files for arg in ("--env-file", str(env_file))],
            "--dir", f"{SERVICES}/sql={memory}::/memory",
            "--wasi", f"{SERVICES}/pgsql=inherit-network",
            *[arg for service in ("generate", "speech", "vision")
              for arg in ("--dir", f"{SERVICES}/{service}={media}::/media")],
            "--dir", f"{SERVICES}/files={layout / 'behaviors'}::/behaviors",
            "--fuel", "9223372036854775807",
            str(layout / "environments/environment/environment.scx"),
        ]
        self.log = log.open("a")
        self.process = subprocess.Popen(command, cwd=layout, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=self.log, text=True, bufsize=1)
        self.records: queue.Queue = queue.Queue()
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self) -> None:
        assert self.process.stdout is not None
        for line in self.process.stdout:
            self.log.write(line)
            try:
                self.records.put(json.loads(line))
            except json.JSONDecodeError:
                continue
        self.records.put(None)

    def send(self, content: str) -> None:
        assert self.process.stdin is not None
        self.process.stdin.write(json.dumps({"name": "bot.ability.communication.input", "data": {
            "source_ids": ["gabe"], "target_ids": ["floppy"], "content": content, "content_type": "text/plain",
        }}) + "\n")
        self.process.stdin.flush()

    def next(self, deadline: float) -> dict | None:
        """The next log record, or None at the deadline; raises once floppy has exited."""
        while True:
            left = deadline - time.monotonic()
            if left <= 0:
                return None
            try:
                record = self.records.get(timeout=min(left, 1.0))
            except queue.Empty:
                continue
            if record is None:
                raise RuntimeError(f"floppy exited ({self.process.wait()})")
            if record.get("type") == "log":
                return record

    def stop(self) -> None:
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                self.process.kill()
        self.log.close()


class FloppyLearner(BaseLearner):
    def __init__(self, work_dir: str | None = None, env_file: str | None = None, model: str | None = None,
                 scx: str | None = None, start_timeout_s: float = 600, turn_timeout_s: float = 1800,
                 stream: bool = False):
        self.work = Path(work_dir or os.environ.get("FLOPPY_CARC_WORK") or ROOT / "target/floppy-carc/work").resolve()
        # One env file, or several joined by the path separator; a later file's entry wins.
        self.env_files = [Path(p).resolve() for p in
                          str(env_file or os.environ.get("FLOPPY_CARC_ENV") or ROOT / ".env").split(os.pathsep)]
        names = set().union(*(read_names(p) for p in self.env_files))
        # The variables naming the hosted gateway: FLOPPY_CARC_GATEWAY, else the reflection
        # endpoint's when an env file sets it, else FLOPPY_BASE_URL's (an env file such as .env.cpa).
        self.gateway = os.environ.get("FLOPPY_CARC_GATEWAY") or (
            "FLOPPY_REFLECTION" if "FLOPPY_REFLECTION_BASE_URL" in names else "FLOPPY")
        if f"{self.gateway}_BASE_URL" not in names:
            raise RuntimeError(f"no env file sets {self.gateway}_BASE_URL (the hosted gateway)")
        self.model = model or os.environ.get("FLOPPY_CARC_MODEL") or "gpt-6-luna"
        self.scx = scx or os.environ.get("FLOPPY_CARC_SCX") or shutil.which("scx") or "scx"
        self.start_timeout_s = start_timeout_s
        self.turn_timeout_s = turn_timeout_s
        self.stream = stream
        self.floppy: Floppy | None = None
        self.first_in_instance = True
        self.pending: list[str] = []
        self.counts = {"turns": 0, "unsettled_turns": 0, "lessons": 0, "restarts": 0, "seconds": 0.0}
        self.turns = None

    # -- layout and process ---------------------------------------------------

    @property
    def layout(self) -> Path:
        return self.work / "layout"

    @property
    def memory(self) -> Path:
        return self.work / "memory"

    def _lay_out(self) -> None:
        if (self.layout / "bots/floppy/floppy.scx").exists():
            return
        for sub in ("bots", "abilities", "environments", "namespaces"):
            if (ROOT / sub).is_dir():
                shutil.copytree(ROOT / sub, self.layout / sub, ignore=shutil.ignore_patterns("target"))
        body = self.layout / "bots/floppy/floppy.scx"
        text = body.read_text()
        for name in ("model", "reflection_model"):
            start = text.index(f'<data id="{name}" expr="')
            end = text.index("/>", start) + 2
            text = text[:start] + f'<data id="{name}" expr="\'{self.model}\'"/>' + text[end:]
        body.write_text(text)
        for document in self.layout.rglob("*.scx"):
            text = document.read_text()
            for old, new in slots(self.gateway).items():
                for quote in ('"', "'"):
                    text = text.replace(f"{quote}{old}{quote}", f"{quote}{new}{quote}")
            if not self.stream and document.name == "reasoning.scx":
                assert text.count('<data id="stream" expr="true"/>') == 1
                text = text.replace('<data id="stream" expr="true"/>', '<data id="stream" expr="false"/>')
            document.write_text(text)

    def _start(self) -> Floppy:
        if self.floppy is not None and self.floppy.process.poll() is None:
            return self.floppy
        self.work.mkdir(parents=True, exist_ok=True)
        self._lay_out()
        for directory in (self.memory, self.layout / "behaviors", self.work / "media"):
            directory.mkdir(parents=True, exist_ok=True)
        self.floppy = Floppy(self.layout, self.memory, self.work / "media", self.env_files, self.scx,
                             self.work / "floppy.log")
        self.turns = (self.work / "turns.jsonl").open("a")
        deadline = time.monotonic() + self.start_timeout_s
        while True:
            record = self.floppy.next(deadline)
            if record is None:
                raise RuntimeError("floppy did not activate in time")
            if record.get("label") == "bot.activated":
                return self.floppy
            if record.get("label") == "bot.activating.failed":
                raise RuntimeError(f"floppy failed to activate: {str(record.get('text'))[:300]}")

    def _stop(self) -> None:
        if self.floppy is not None:
            self.floppy.stop()
            self.floppy = None
        if self.turns is not None:
            self.turns.close()
            self.turns = None

    def ask(self, message: str) -> str:
        """One message, answered once its turn and its lessons settle."""
        began = time.monotonic()
        try:
            floppy = self._start()
        except RuntimeError:
            self._stop()
            self.counts["restarts"] += 1
            floppy = self._start()
        floppy.send(message)
        deadline = time.monotonic() + self.turn_timeout_s
        replies: list[str] = []
        settled = False
        open_lessons = 0
        labels: list[str] = []
        while True:
            record = floppy.next(deadline)
            if record is None:
                break
            label = record.get("label")
            labels.append(label)
            if label == "floppy.output":
                replies += reply_words(str(record.get("text"))) or []
            elif label in LESSON_OPEN:
                open_lessons += 1
                self.counts["lessons"] += 1
            elif label in LESSON_CLOSE:
                open_lessons = max(0, open_lessons - 1)
            elif label in SETTLED:
                settled = True
            if settled and open_lessons == 0 and label in SETTLED + LESSON_CLOSE:
                # A lesson's result comes back as a turn of its own; its settling ends the wait.
                break
        seconds = time.monotonic() - began
        self.counts["turns"] += 1
        self.counts["unsettled_turns"] += not settled
        self.counts["seconds"] += seconds
        reply = "\n".join(replies)
        if self.turns is not None:
            self.turns.write(json.dumps({"message": message, "reply": reply, "settled": settled,
                                         "seconds": round(seconds, 1), "lessons_open": open_lessons}) + "\n")
            self.turns.flush()
        return reply

    # -- the benchmark's four methods ----------------------------------------

    def reset(self) -> None:
        self._stop()
        for directory in (self.memory, self.layout / "behaviors", self.work / "media"):
            shutil.rmtree(directory, ignore_errors=True)
        self.first_in_instance = True
        self.pending = []

    def act(self, obs):
        fresh = obs.attempts_used == 0 and obs.demo_requests_used == 0
        if fresh:
            self.first_in_instance = True
        message = protocol.render_observation(obs, pending=self.pending, demo_cost=1.0, max_cost=16.0,
                                              first_in_session=self.first_in_instance)
        self.pending = []
        self.first_in_instance = False
        for attempt in range(3):
            reply = self.ask(message)
            try:
                return protocol.parse_action(reply)
            except protocol.ParseError as error:
                message = (f"I could not read an action from your reply ({error}). "
                           "Reply with exactly one action JSON on the last line.")
        return GiveUp()

    def observe(self, obs, action, feedback) -> None:
        self.pending.append(protocol.render_feedback(action, feedback))
        over = (isinstance(feedback, SubmitFeedback) and (feedback.correct or feedback.failed)) \
            or isinstance(feedback, GiveUpFeedback)
        if over:
            self.ask(protocol.render_closing(self.pending))
            self.pending = []
            self.first_in_instance = True

    def resources(self) -> dict[str, float]:
        return dict(self.counts)

    def __del__(self):
        try:
            self._stop()
        except Exception:
            pass
