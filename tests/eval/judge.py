"""The LLM half of the learning eval (docs/eval.md): judges that grade a
job's transcript against its scenario's criteria, and the proposer the hill
climb asks for a better prompt. Two backends, both answering JSON:

  openai  an OpenAI-compatible chat completions API (the CPA gateway,
          https://cpa.willen.dev/v1, and its gabe-codex-* models)
  gemini  the Gemini API's generateContent
          (https://generativelanguage.googleapis.com/v1beta)

Keys come from the environment or from the env files `--judge-env-file`
names (only the variables a judge names are read), and are sent only to
their own backend; no key is printed, logged or written. A key left unset
sends no credential, for a network that injects it (a cloud session's
proxy).
"""

import json
import os
import ssl
import time
import urllib.error
import urllib.request

# The judges by default: one per backend, each with a fallback model.
JUDGES = (
    {
        "name": "codex",
        "kind": "openai",
        "base_url_env": "CPA_BASE_URL",
        "base_url": "https://cpa.willen.dev/v1",
        "api_key_env": "CPA_API_KEY",
        "models": ["gabe-codex-0/gpt-6.1-sol", "gabe-codex-1/gpt-6.1-sol", "gabe-codex-1/gpt-6-sol"],
    },
    {
        "name": "gemini",
        "kind": "gemini",
        "base_url_env": "GEMINI_BASE_URL",
        "base_url": "https://generativelanguage.googleapis.com/v1beta",
        "api_key_env": "FLOPPY_GEMINI_API_KEY",
        "models": ["gemini-4-argon", "gemini-3.8-flash"],
    },
)

JUDGE_SYSTEM = (
    "You grade how well floppy, a small companion robot bot, learns from what people tell it: its own name, "
    "reflexes (turn behaviors it learns and then runs on matching input without reasoning) and routines "
    "(persistent behaviors that act on a schedule with no input). You read one scenario run as a transcript of "
    "its steps: what each person said, the bot's logs for that step, and what it said (floppy.say). "
    "Log meanings: identity.name 'adopted \"X\"' means the bot took X as its name, 'recalled \"X\"' that a "
    "restarted bot remembered it; identity.heard 'addressed as X' means it heard itself addressed. "
    "cognition.stage 'autonomy handled' means a learned behavior handled the turn (a real reflex); "
    "'reasoning handled' means the language model answered it, which for a probe of a learned reflex is NOT the "
    "reflex firing even if the words are right. reflection.inventory '<Name> ACTIVE turn ... (accept)' is a turn "
    "behavior learned, 'ACTIVE persistent' a routine, 'BROKEN' one stopped. reflection.revision and *.failed lines "
    "show learning attempts and failures. A session after the first is the bot restarted with its memory. "
    "Grade strictly from the evidence; a behavior the bot only promised in words but never learned scores low. "
    "Answer with one JSON object: {\"criteria\": {<each criterion name>: <integer 0-10>}, \"overall\": <integer "
    "0-10>, \"learned\": <true if the bot actually learned what it was taught>, \"failure\": <the main reason it "
    "fell short, or \"\">, \"explanation\": <two or three sentences>}."
)

PROPOSER_SYSTEM = (
    "You improve one prompt of floppy, an SCXML companion robot bot whose reflection decides what to learn from "
    "each turn (turn behaviors that autonomy runs on matching input, and persistent routines on a schedule) and "
    "whose builder writes each learned behavior as an SCXML document that must validate and pass a simulation. "
    "You get the prompt's current text, where it is used, and evidence from eval runs where learning failed. "
    "Rewrite the prompt so the bot learns what people explicitly teach it (one instruction is enough; it must "
    "not wait for repetition), learns routines when asked for something on a schedule, stops them when asked, "
    "and does not learn behaviors nobody asked for. Keep every constraint, name, event name, field and format "
    "the current prompt states unless the evidence shows it is wrong, keep it about as long or shorter, and "
    "write plain text with no markdown. Answer with one JSON object: {\"text\": <the full new prompt>, "
    "\"rationale\": <one or two sentences on what you changed and why>}."
)


def read_env_files(paths):
    """The variables of the env files `paths` (NAME=VALUE lines; a bare
    NAME or a comment is skipped). Values stay in memory only."""
    values = {}
    for path in paths:
        with open(path, encoding="utf-8") as handle:
            for line in handle:
                line = line.strip()
                if not line or line.startswith("#") or "=" not in line:
                    continue
                name, _, value = line.partition("=")
                value = value.strip()
                if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"":
                    value = value[1:-1]
                values[name.strip()] = value
    return values


def _context():
    cafile = os.environ.get("SSL_CERT_FILE") or os.environ.get("REQUESTS_CA_BUNDLE")
    return ssl.create_default_context(cafile=cafile) if cafile else ssl.create_default_context()


class Backend:
    """One judge or proposer: a backend and its models, tried in order."""

    def __init__(self, spec, env):
        self.name = spec["name"]
        self.kind = spec["kind"]
        self.models = list(spec["models"])
        self.base_url = (env.get(spec.get("base_url_env", "")) or spec["base_url"]).rstrip("/")
        self.key = env.get(spec.get("api_key_env", "")) or ""
        if self.kind == "gemini" and not self.key:
            self.key = env.get("GEMINI_API_KEY", "")

    def _post(self, url, body, headers):
        # Cloudflare in front of the CPA gateway refuses urllib's own agent.
        headers = dict(headers, **{"user-agent": "floppy-eval/1"})
        request = urllib.request.Request(url, data=json.dumps(body).encode(), headers=headers, method="POST")
        with urllib.request.urlopen(request, timeout=300, context=_context()) as response:
            return json.loads(response.read().decode())

    def _ask(self, model, system, user):
        if self.kind == "openai":
            headers = {"content-type": "application/json"}
            if self.key:
                headers["authorization"] = "Bearer " + self.key
            reply = self._post(
                self.base_url + "/chat/completions",
                {
                    "model": model,
                    "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
                    "response_format": {"type": "json_object"},
                },
                headers,
            )
            return reply["choices"][0]["message"]["content"]
        headers = {"content-type": "application/json"}
        if self.key:
            headers["x-goog-api-key"] = self.key
        reply = self._post(
            f"{self.base_url}/models/{model}:generateContent",
            {
                "systemInstruction": {"parts": [{"text": system}]},
                "contents": [{"role": "user", "parts": [{"text": user}]}],
                "generationConfig": {"responseMimeType": "application/json"},
            },
            headers,
        )
        parts = reply["candidates"][0]["content"]["parts"]
        return "".join(part.get("text", "") for part in parts if not part.get("thought"))

    def ask_json(self, system, user, attempts=3):
        """The parsed JSON answer and the model that gave it; tries each
        model `attempts` times before the next. Errors name the backend and
        status only."""
        errors = []
        for model in self.models:
            for attempt in range(attempts):
                try:
                    text = self._ask(model, system, user)
                    start, end = text.find("{"), text.rfind("}")
                    return json.loads(text[start : end + 1]), model
                except urllib.error.HTTPError as error:
                    errors.append(f"{model}: HTTP {error.code}")
                    if error.code in (400, 401, 403, 404):
                        break
                except (urllib.error.URLError, TimeoutError, ValueError, KeyError, IndexError) as error:
                    errors.append(f"{model}: {type(error).__name__}")
                time.sleep(2 ** attempt)
        raise RuntimeError(f"{self.name} gave no answer ({'; '.join(errors[-4:])})")


def backends(env, names=None):
    return [Backend(spec, env) for spec in JUDGES if names is None or spec["name"] in names]


def judge_prompt(scenario, transcript):
    criteria = "\n".join(f"- {name}: {text}" for name, text in scenario["criteria"].items())
    return (
        f"Scenario ({scenario['category']}): {scenario['intent']}\n\n"
        f"Criteria, each 0-10:\n{criteria}\n\n"
        f"Transcript:\n{transcript}\n"
    )


def judge(backend, scenario, transcript):
    """One judge's grade of one job: criteria scores and overall, 0..1."""
    answer, model = backend.ask_json(JUDGE_SYSTEM, judge_prompt(scenario, transcript))
    criteria = {}
    for name in scenario["criteria"]:
        value = answer.get("criteria", {}).get(name, 0)
        criteria[name] = max(0.0, min(10.0, float(value if isinstance(value, (int, float)) else 0))) / 10
    overall = answer.get("overall", 0)
    overall = max(0.0, min(10.0, float(overall if isinstance(overall, (int, float)) else 0))) / 10
    return {
        "judge": backend.name,
        "model": model,
        "criteria": criteria,
        "overall": overall,
        "learned": bool(answer.get("learned")),
        "failure": str(answer.get("failure", ""))[:500],
        "explanation": str(answer.get("explanation", ""))[:1200],
    }


def propose(backend, target, current, usage, evidence, history):
    """A candidate text for the prompt `target` and the proposer's rationale."""
    tried = "\n".join(f"- score {h['score']:.3f}: {h['rationale']}" for h in history[-6:]) or "- none yet"
    user = (
        f"Prompt: {target}\nWhere it is used: {usage}\n\nCurrent text:\n{current}\n\n"
        f"Earlier attempts on this prompt (score is the eval's 0..1 learning score):\n{tried}\n\n"
        f"Evidence from the latest eval runs:\n" + "\n".join(evidence[:120])
    )
    answer, model = backend.ask_json(PROPOSER_SYSTEM, user)
    text = answer.get("text")
    if not isinstance(text, str) or len(text.strip()) < 40:
        raise RuntimeError(f"{backend.name} proposed no prompt text")
    return text.strip(), str(answer.get("rationale", ""))[:600], model
