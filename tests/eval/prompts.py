"""Reads and writes the prompt <data> of floppy's documents for the eval's
hill climb (docs/eval.md, "Hill climbing").

A prompt is a `<data id="ID" expr="'...'"/>` whose expr is one ECMAScript
string literal inside an XML attribute: the text is XML-escaped (&amp;
&lt; &gt; &quot;) around a single-quoted literal (\\' and \\\\). A target
is `<path>#<id>`, the path relative to the workspace. The climb never edits
the workspace: `overlay` writes changed copies into a directory laid out
like it, which tests/tests/learning_eval.rs copies over each job's scratch
layout.
"""

import difflib
import os
import re

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))

# The prompts the climb may change, when no --target names others, by
# <data> id: the ones that decide what floppy learns (reflection's selection
# and change instructions) and how the builder writes it. Each is found in
# whichever document of the bot declares it (`default_targets`), so a
# prompt that moves to another document stays a default.
DEFAULT_IDS = ("select_instructions", "learn_instructions", "change_instructions", "author_system")


# Any <data> whose id names a prompt, for `eval.py targets`.
PROMPT_ID = re.compile(r"(instructions|system|contract)$")


def _pattern(data_id):
    return re.compile(r'(<data id="' + re.escape(data_id) + r'" expr=")([^"]*)("\s*/>)')


def _xml_decode(text):
    return (
        text.replace("&quot;", '"')
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
    )


def _xml_encode(text):
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace('"', "&quot;")


def _js_decode(literal):
    if len(literal) < 2 or literal[0] != "'" or literal[-1] != "'":
        raise ValueError("the expr is not one single-quoted string literal")
    out = []
    body = literal[1:-1]
    index = 0
    escapes = {"n": "\n", "t": "\t", "'": "'", "\\": "\\", '"': '"'}
    while index < len(body):
        char = body[index]
        if char == "\\" and index + 1 < len(body):
            nxt = body[index + 1]
            if nxt not in escapes:
                raise ValueError("an escape the codec does not know: \\" + nxt)
            out.append(escapes[nxt])
            index += 2
            continue
        if char == "'":
            raise ValueError("an unescaped quote ends the literal early")
        out.append(char)
        index += 1
    return "".join(out)


def _js_encode(text):
    return "'" + text.replace("\\", "\\\\").replace("'", "\\'").replace("\n", "\\n").replace("\t", "\\t") + "'"


def split(target):
    path, _, data_id = target.partition("#")
    if not path or not data_id:
        raise ValueError("a target is <path>#<data id>: " + target)
    return path, data_id


def read(target, base=ROOT):
    """The text of the prompt `target` in the layout `base`."""
    path, data_id = split(target)
    with open(os.path.join(base, path), encoding="utf-8") as handle:
        document = handle.read()
    found = _pattern(data_id).findall(document)
    if len(found) != 1:
        raise ValueError(f"{target}: expected one <data> with a string expr, found {len(found)}")
    return _js_decode(_xml_decode(found[0][1]))


def write_text(document, data_id, text):
    """`document` with the prompt `data_id` set to `text`."""
    pattern = _pattern(data_id)
    encoded = _xml_encode(_js_encode(text))
    if len(pattern.findall(document)) != 1:
        raise ValueError(f"{data_id}: expected exactly one <data> to replace")
    return pattern.sub(lambda m: m.group(1) + encoded + m.group(3), document, count=1)


def roundtrips(target):
    """Whether writing the prompt's own text back leaves its document byte
    for byte the same (the codec is exact for it)."""
    path, data_id = split(target)
    with open(os.path.join(ROOT, path), encoding="utf-8") as handle:
        document = handle.read()
    return write_text(document, data_id, read(target)) == document


def discover():
    """Every prompt <data> of the bot's and the builder's documents."""
    found = []
    for top in ("abilities", "bots"):
        for directory, _, files in os.walk(os.path.join(ROOT, top)):
            if os.sep + "irp" in directory or os.sep + "examples" in directory:
                continue
            for name in sorted(files):
                if not name.endswith(".scx"):
                    continue
                full = os.path.join(directory, name)
                rel = os.path.relpath(full, ROOT)
                with open(full, encoding="utf-8") as handle:
                    document = handle.read()
                for data_id in re.findall(r'<data id="([A-Za-z_][\w.-]*)" expr="\'', document):
                    if PROMPT_ID.search(data_id):
                        found.append(rel + "#" + data_id)
    return sorted(found)


def default_targets():
    """The targets of DEFAULT_IDS, each where the workspace declares it."""
    found = discover()
    targets = []
    for data_id in DEFAULT_IDS:
        matches = [target for target in found if split(target)[1] == data_id]
        if len(matches) != 1:
            raise ValueError(f"{data_id}: expected one document to declare it, found {matches}")
        targets.extend(matches)
    return targets


def overlay(prompts, directory):
    """Writes the documents of `prompts` ({target: text}) changed into
    `directory`, laid out like the workspace; returns the unified diff."""
    by_path = {}
    for target, text in prompts.items():
        path, data_id = split(target)
        by_path.setdefault(path, []).append((data_id, text))
    diffs = []
    for path, changes in sorted(by_path.items()):
        with open(os.path.join(ROOT, path), encoding="utf-8") as handle:
            original = handle.read()
        document = original
        for data_id, text in changes:
            document = write_text(document, data_id, text)
        out = os.path.join(directory, path)
        os.makedirs(os.path.dirname(out), exist_ok=True)
        with open(out, "w", encoding="utf-8") as handle:
            handle.write(document)
        diffs.extend(
            difflib.unified_diff(
                original.splitlines(keepends=True),
                document.splitlines(keepends=True),
                fromfile="a/" + path,
                tofile="b/" + path,
            )
        )
    return "".join(diffs)
