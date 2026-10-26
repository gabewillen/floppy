"""The eval's ears and the person's voice (docs/eval.md): a heard step's
utterance is synthesized on a speech backend's `/audio/speech` in the
person's voice, and a `speaks` check transcribes the sounds floppy made on
its `/audio/transcriptions`, so the voice path is graded on what a person
in the room would hear.

The backend is OpenAI-compatible: `FLOPPY_EVAL_SPEECH_BASE_URL` (default
`https://api.willen.dev/v1`, the lab's shared model API, decision D244)
with the key `FLOPPY_EVAL_SPEECH_API_KEY`, from the environment or the
`--judge-env-file` files; a key left unset sends no credential, for a
network that injects it (a cloud session's proxy). No key is printed or
written. Standard library only.
"""

import hashlib
import io
import json
import os
import time
import urllib.error
import urllib.request
import uuid
import wave

import judge
import prompts
import score

RATE = 24000
BASE_URL = "https://api.willen.dev/v1"
# The person's voice by default; floppy speaks in the speaking ability's own.
VOICE = "canvas"
STT_MODEL = "nemotron-speech-streaming-en-0.6b"


class Backend:
    def __init__(self, env):
        self.base_url = (env.get("FLOPPY_EVAL_SPEECH_BASE_URL") or BASE_URL).rstrip("/")
        self.key = env.get("FLOPPY_EVAL_SPEECH_API_KEY") or ""

    def _post(self, path, body, content_type, attempts=4):
        headers = {"content-type": content_type, "user-agent": "floppy-eval/1"}
        if self.key:
            headers["authorization"] = "Bearer " + self.key
        errors = []
        for attempt in range(attempts):
            request = urllib.request.Request(self.base_url + path, data=body, headers=headers, method="POST")
            try:
                with urllib.request.urlopen(request, timeout=180, context=judge._context()) as response:
                    return response.read()
            except urllib.error.HTTPError as error:
                errors.append(f"HTTP {error.code}")
                if error.code in (400, 401, 403, 404, 422):
                    break
            except (urllib.error.URLError, TimeoutError) as error:
                errors.append(type(error).__name__)
            time.sleep(2 ** attempt)
        raise RuntimeError(f"the speech backend gave no answer to {path} ({'; '.join(errors)})")

    def speak(self, text, voice=VOICE, instructions=None):
        """`text` in `voice` as 24 kHz mono 16-bit PCM, said as `instructions`
        tell when given (the speech API's `instructions`: the tone)."""
        request = {"input": text, "voice": voice, "response_format": "wav"}
        if instructions:
            request["instructions"] = instructions
        body = json.dumps(request).encode()
        return pcm_of_wav(self._post("/audio/speech", body, "application/json"))

    def transcribe(self, pcm):
        """The words of 24 kHz mono 16-bit PCM, or "" for none."""
        boundary = uuid.uuid4().hex
        parts = [
            f'--{boundary}\r\ncontent-disposition: form-data; name="model"\r\n\r\n{STT_MODEL}\r\n'.encode(),
            f'--{boundary}\r\ncontent-disposition: form-data; name="file"; filename="floppy.wav"\r\n'
            "content-type: audio/wav\r\n\r\n".encode() + wav_of_pcm(pcm) + b"\r\n",
            f"--{boundary}--\r\n".encode(),
        ]
        answer = self._post("/audio/transcriptions", b"".join(parts), f"multipart/form-data; boundary={boundary}")
        return str(json.loads(answer.decode()).get("text", "")).strip()


def pcm_of_wav(data):
    """The 16-bit mono samples of a WAV, at 24 kHz. A WAV streamed with an
    unknown length declares one larger than it holds: what follows its
    header is read as is."""
    with wave.open(io.BytesIO(data)) as source:
        rate, channels, width = source.getframerate(), source.getnchannels(), source.getsampwidth()
    if (rate, channels, width) != (RATE, 1, 2):
        raise RuntimeError(f"the speech backend answered {rate} Hz, {channels} channel(s), {8 * width}-bit audio")
    at = data.find(b"data")
    if at < 0:
        raise RuntimeError("the speech backend's WAV has no data chunk")
    pcm = data[at + 8 :]
    return pcm[: len(pcm) - len(pcm) % 2]


def wav_of_pcm(pcm, rate=RATE):
    out = io.BytesIO()
    with wave.open(out, "wb") as sink:
        sink.setnchannels(1)
        sink.setsampwidth(2)
        sink.setframerate(rate)
        sink.writeframes(pcm)
    return out.getvalue()


def silence(seconds):
    return bytes(2 * int(RATE * seconds))


def utterance(backend, step, cache):
    """The PCM of a heard step: its `segments` (text in the step's voice,
    a number a pause of that many seconds) or its `hear` text, said as its
    `instructions` tell (a tone) when it has them, each piece of speech
    synthesized once into `cache`."""
    voice = step.get("voice", VOICE)
    instructions = step.get("instructions")
    pieces = step.get("segments") or [step["hear"]]
    pcm = b""
    for piece in pieces:
        if isinstance(piece, (int, float)):
            pcm += silence(piece)
            continue
        key = hashlib.sha256(json.dumps([voice, piece] + ([instructions] if instructions else [])).encode()).hexdigest()[:24]
        path = os.path.join(cache, f"{key}.pcm")
        if not os.path.exists(path):
            os.makedirs(cache, exist_ok=True)
            spoken = backend.speak(piece, voice, instructions)
            with open(path + ".part", "wb") as handle:
                handle.write(spoken)
            os.replace(path + ".part", path)
        with open(path, "rb") as handle:
            pcm += handle.read()
    return pcm


def prepare(scenario, out, backend, cache):
    """A copy of `scenario` whose heard steps name their audio, written
    under `<out>/voice/` (none without a `backend`: a plan written dry),
    and whose shown images are absolute paths."""
    scenario = json.loads(json.dumps(scenario))
    for session_index, session in enumerate(scenario["sessions"]):
        for step_index, step in enumerate(session["steps"]):
            if "hear" in step and backend is not None:
                path = os.path.join(out, "voice", f"{session_index}-{step_index}.pcm")
                os.makedirs(os.path.dirname(path), exist_ok=True)
                with open(path, "wb") as handle:
                    handle.write(utterance(backend, step, cache))
                step["audio"] = path
            if "show" in step:
                step["show"] = os.path.join(prompts.ROOT, step["show"])
    return scenario


def read_sound(media, data):
    """24 kHz mono 16-bit PCM of a sound floppy made (an environment.sound
    whose audio is in the job's media directory), or None."""
    ref = str(data.get("audio", ""))
    path = os.path.join(media, ref.removeprefix("/").removeprefix("media/"))
    if not os.path.isfile(path):
        return None
    with open(path, "rb") as handle:
        raw = handle.read()
    if raw[:4] == b"RIFF":
        try:
            return pcm_of_wav(raw)
        except (RuntimeError, wave.Error, EOFError):
            return None
    if int(data.get("sample_rate_hz") or RATE) != RATE or int(data.get("channels") or 1) != 1:
        return None
    return raw[: len(raw) - len(raw) % 2]


def transcripts(job_out, groups, backend):
    """{key: words} for each group of floppy's sounds (the data of its
    environment.sound records in one step, in order), the group's audio
    joined and transcribed once, keyed by its references joined with "|"
    (a reply streamed in chunks is heard whole, as a person hears it), and
    kept in `<job_out>/heard.json`; a group none of whose sounds can be
    read, or that cannot be transcribed, is left out."""
    path = os.path.join(job_out, "heard.json")
    heard = {}
    if os.path.exists(path):
        with open(path, encoding="utf-8") as handle:
            heard = json.load(handle)
    changed = False
    for session, group in groups:
        key = score.group_key(session, group)
        if not group or key in heard or backend is None:
            continue
        media = media_dir(job_out, session)
        pcm = b"".join(p for p in (read_sound(media, data) for data in group) if p)
        if not pcm:
            continue
        try:
            heard[key] = backend.transcribe(pcm)
        except RuntimeError:
            continue
        changed = True
    if changed:
        with open(path, "w", encoding="utf-8") as handle:
            json.dump(heard, handle, indent=2)
    return heard


def media_dir(job_out, session):
    """A session's media directory: the driver copies each session's to
    `media-<session>` when it ends (a run from before that has one `media`)."""
    path = os.path.join(job_out, f"media-{session}")
    return path if os.path.isdir(path) else os.path.join(job_out, "media")
