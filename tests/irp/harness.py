"""Writes the test documents of floppy's document suites (bots/<name>/irp,
abilities/<name>/irp) from each suite's cases.py.

Usage: python3 tests/irp/harness.py [NAME...]

A suite's cases.py lists its statements and their cases with the helpers
below; this writes irp/manifest.xml, irp/sources/<id>/test<id>.scxml and,
for a case that scripts the fake backend, irp/sources/<id>/backend.json,
and the "Statements" section of irp/README.md. tests/tests/document_irp.rs
runs them. The test documents are the suite's checked-in truth; this script
only keeps writing them uniform.

A case is a harness document: it invokes the document under test as its
child (id = the document's name, by its literal relative path), sends it
events, and expects its events in order. The harness first sends itself
`harness.go`, so the child has started before the first step runs. Each
step is one of

  send(event, expr)                 sends `event` to the child with
                                    <content expr>; expr None sends no data
  request(event, fields=(), request_id="r1")
                                    sends a long-lived child one request:
                                    `event` with {id: request_id} and each
                                    (name, expr) of `fields`; its answer is
                                    expected with answered(cond, request_id),
                                    `cond` holding of the answer without id
  wait(delay)                       lets `delay` pass (a delayed send to
                                    itself; give the case clock="wall")
  expect(event, cond=None, assigns=())
                                    waits for `event` (from the child, or
                                    any event the harness receives) and
                                    takes it when `cond` holds, assigning
                                    each (location, expr)
  offers(cond, undeclared=None)     reads what the child offers from its
                                    live session (<decision:offers
                                    location="offers">, decision D242) and
                                    goes on when `cond` holds of `offers`;
                                    otherwise the case fails, logging them
  snapshot(cond, within="30s")      reads the child's live state
                                    (<device:snapshot location="snapshot">,
                                    decision D109), again every 100 ms, and
                                    goes on once `cond` holds of `snapshot`;
                                    if it does not within `within` (a CSS2
                                    duration; give the case clock="wall"),
                                    the case fails, logging it
  ignore(event)                     drops `event` wherever it arrives

A case's params name the child's <param>s as (name, expr); memory(database)
in a param's place is the child's `memory` <param> (decision D266): the
harness invokes the long-lived memory ability (id `memory`) on the
`database` expr before the child and passes the child its address, or null
when `database` is "null".

and the case passes when every expect was met in order. Any other event
the harness receives (error.* included) fails it, logging the event.
A case's `websocket=[session, ...]` (written into its backend.json)
scripts the fake backend's WebSocket sessions, each connection taking the
next (the last repeats): a session is a list of rounds of frames, the
first sent once it opens, round n once its n-th binary frame arrives.
A case's `logs={label: [text, ...]}` (written as irp/sources/<id>/logs.json)
also requires the run's logs of each label to be exactly those texts, in
order.

A suite's learned behaviors are programs `S.behavior(text)` writes as
irp/behaviors/<digest>.scxml (the extension of test documents, which keeps
them out of the checks every floppy `.scx` document is held to), returning
that file's path relative to the document under test, which a behavior
row's source names (decision D121).

A suite's media is a library of files, `S.media({name: bytes})` (built
with wav, pcm and png below): each case's irp/sources/<id>/media/ holds
exactly the library files its document or backend names as `media/<name>`,
so a name the library lacks (an output, or a file a case expects missing)
is never there.
"""

import hashlib
import json
import os
import re
import runpy
import struct
import sys
import zlib
from xml.sax.saxutils import escape, quoteattr

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))


class Send:
    def __init__(self, event, expr=None, target=None, delay=None):
        self.event, self.expr, self.target, self.delay = event, expr, target, delay


class Expect:
    def __init__(self, event, cond=None, assigns=()):
        self.event, self.cond, self.assigns = event, cond, assigns


def send(event, expr=None, target=None):
    return Send(event, expr, target)


def wait(delay):
    """Steps that let `delay` (a CSS2 duration) pass on the wall clock."""
    return [Send("harness.waited", None, "self", delay), Expect("harness.waited")]


def expect(event, cond=None, assigns=()):
    return Expect(event, cond, assigns)


class Offers:
    def __init__(self, cond, undeclared=None):
        self.cond, self.undeclared = cond, undeclared


def offers(cond, undeclared=None):
    """A step that reads the child's offers from its live session and requires `cond` of them."""
    return Offers(cond, undeclared)


class Snapshot:
    def __init__(self, cond, within):
        self.cond, self.within = cond, within


def snapshot(cond, within="30s"):
    """A step that reads the child's live state until `cond` holds of it, for at most `within`."""
    return Snapshot(cond, within)


class Memory:
    def __init__(self, database):
        self.database = database


def memory(database):
    """The child's `memory` <param>: the address of a memory ability the harness serves on `database`."""
    return ("memory", Memory(database))


def js(value):
    return json.dumps(value, sort_keys=False)


def request(event, fields=(), request_id="r1"):
    """A step that sends a long-lived child one request (decision D266):
    `event` with data {id: request_id} and each (name, expr) of `fields`."""
    return Send(event, "({" + ", ".join([f"id: {js(request_id)}"] + [f"{name}: {expr}" for name, expr in fields]) + "})")


def answered(cond=None, request_id="r1"):
    """The cond of a long-lived child's answer to a request: its id is
    `request_id`, and `cond` (when given) holds of the answer without its
    id, as `_event`."""
    matched = f"_event.data.id === {js(request_id)}"
    if cond is None:
        return matched
    return (f"{matched} && (function (_event) {{ return {cond}; }})({{name: _event.name, data: "
            "(function (data) { var copy = JSON.parse(JSON.stringify(data)); delete copy.id; return copy; })(_event.data)})")


def pcm(sample=0, seconds=0.1, rate=16000, channels=1):
    """Raw signed 16-bit little-endian PCM, every sample `sample`."""
    return struct.pack("<h", sample) * (round(seconds * rate) * channels)


def wav(sample=0, seconds=0.1, rate=16000, channels=1):
    """A canonical PCM WAV container of pcm(sample, seconds, rate, channels)."""
    data = pcm(sample, seconds, rate, channels)
    return (b"RIFF" + struct.pack("<I", 36 + len(data)) + b"WAVE"
            + b"fmt " + struct.pack("<IHHIIHH", 16, 1, channels, rate, rate * channels * 2, channels * 2, 16)
            + b"data" + struct.pack("<I", len(data)) + data)


def png(width=1, height=1, shade=0xff):
    """An 8-bit RGB PNG whose every pixel is the gray `shade` (white by default)."""
    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))
    rows = b"".join(b"\x00" + bytes([shade]) * (3 * width) for _ in range(height))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b""))


MEDIA_NAME = re.compile(r"media/([A-Za-z0-9_.-]+)")


class Suite:
    def __init__(self, kind, name):
        self.kind, self.name = kind, name
        self.statements = []
        self.cases = []
        self.library = {}
        self.behaviors = {}

    def media(self, files):
        self.library.update(files)

    def behavior(self, text):
        name = hashlib.sha256(text.encode()).hexdigest()[:16]
        self.behaviors[name] = text
        return f"irp/behaviors/{name}.scxml"

    def statement(self, number, text):
        self.statements.append((f"{self.name}-{number}", text))

    def case(self, test_id, number, comment, steps, data=(), params=(), ignore=(), backend=None, clock=None,
             stream=False, logs=None, websocket=None):
        self.cases.append(dict(id=test_id, assertion=f"{self.name}-{number}", comment=comment, steps=steps,
                               data=data, params=params, ignore=ignore, backend=backend, clock=clock,
                               stream=stream, logs=logs, websocket=websocket))

    # ------------------------------------------------------------ writing
    def document(self, case):
        name = self.name
        items = [item for step in case["steps"] for item in (step if isinstance(step, list) else [step])]
        reads = any(isinstance(item, Offers) for item in items)
        snapshots = any(isinstance(item, Snapshot) for item in items)
        decision = ' xmlns:decision="https://github.com/stateforward/floppy/namespaces/decision"' if reads else ""
        if snapshots:
            decision += ' xmlns:device="https://github.com/stateforward/floppy/namespaces/device"'
        lines = [
            '<?xml version="1.0" encoding="UTF-8"?>',
            f"<!-- {case['assertion']}: {escape(case['comment'])} -->",
            f'<scxml xmlns="http://www.w3.org/2005/07/scxml"{decision} version="1.0" datamodel="ecmascript" initial="harness">',
        ]
        data = list(case["data"]) + ([("offers", "null")] if reads else []) + ([("snapshot", "null")] if snapshots else [])
        if data:
            lines.append("<datamodel>")
            for location, expr in data:
                lines.append(f"  <data id={quoteattr(location)} expr={quoteattr(expr)}/>")
            lines.append("</datamodel>")
        lines.append('<state id="harness" initial="boot">')
        params = []
        for pname, expr in case["params"]:
            if isinstance(expr, Memory):
                if expr.database == "null":
                    expr = "null"
                else:
                    depth = "../" * (len(self.kind.split("/")) + 4)
                    lines.append(f'  <invoke id="memory" type="scxml" src="{depth}abilities/memory/memory.scx">')
                    lines.append(f"    <param name=\"database\" expr={quoteattr(expr.database)}/>")
                    lines.append("  </invoke>")
                    expr = "'#_scxml_' + _sessionid + '.memory'"
            params.append((pname, expr))
        invoke = f'  <invoke id="{name}" type="scxml" src="../../../{name}.scx"'
        if params:
            lines.append(invoke + ">")
            for pname, expr in params:
                lines.append(f"    <param name={quoteattr(pname)} expr={quoteattr(expr)}/>")
            lines.append("  </invoke>")
        else:
            lines.append(invoke + "/>")
        lines.append('  <state id="boot">')
        lines.append('    <onentry><send event="harness.go"/></onentry>')
        lines.append('    <transition event="harness.go" target="step0"/>')
        lines.append("  </state>")
        step, pending = 0, []

        def flush_sends():
            out = []
            for item in pending:
                target = item.target or f"#_{name}"
                attributes = f"event={quoteattr(item.event)}"
                if target != "self":
                    attributes += f" target={quoteattr(target)}"
                if item.delay:
                    attributes += f" delay={quoteattr(item.delay)}"
                if item.expr is None:
                    out.append(f"      <send {attributes}/>")
                else:
                    out.append(f"      <send {attributes}><content expr={quoteattr(item.expr)}/></send>")
            pending.clear()
            return out

        steps = []
        for item in case["steps"]:
            steps.extend(item if isinstance(item, list) else [item])
        for item in steps:
            if isinstance(item, Send):
                pending.append(item)
                continue
            lines.append(f'  <state id="step{step}">')
            sends = flush_sends()
            if isinstance(item, Offers):
                undeclared = f" undeclared={quoteattr(item.undeclared)}" if item.undeclared else ""
                lines += ["    <onentry>"] + sends + [
                    f"      <decision:offers location=\"offers\" target=\"'#_{name}'\"{undeclared}/>",
                    "    </onentry>",
                    f"    <transition cond={quoteattr(item.cond)} target=\"step{step + 1}\"/>",
                    '    <transition target="fail"><log label="unexpected" expr="\'offers \' + JSON.stringify(offers)"/></transition>',
                    "  </state>",
                ]
                step += 1
                continue
            if isinstance(item, Snapshot):
                # Read again every 100 ms until `cond` holds, failing at the deadline.
                lines[-1] = f'  <state id="step{step}" initial="step{step}_read">'
                lines += ["    <onentry>"] + sends + [
                    f'      <send event="harness.snapshot.deadline" id="step{step}_deadline" delay={quoteattr(item.within)}/>',
                    "    </onentry>",
                    f'    <onexit><cancel sendid="step{step}_deadline"/></onexit>',
                    '    <transition event="harness.snapshot.deadline" target="fail"><log label="unexpected" expr="\'snapshot \' + JSON.stringify(snapshot)"/></transition>',
                    f'    <state id="step{step}_read">',
                    "      <onentry>",
                    f"        <device:snapshot location=\"snapshot\" target=\"'#_{name}'\"/>",
                    "      </onentry>",
                    f"      <transition cond={quoteattr(item.cond)} target=\"step{step + 1}\"/>",
                    f'      <transition target="step{step}_again"/>',
                    "    </state>",
                    f'    <state id="step{step}_again">',
                    '      <onentry><send event="harness.waited" delay="100ms"/></onentry>',
                    f'      <transition event="harness.waited" target="step{step}_read"/>',
                    "    </state>",
                    "  </state>",
                ]
                step += 1
                continue
            if sends:
                lines += ["    <onentry>"] + sends + ["    </onentry>"]
            cond = f" cond={quoteattr(item.cond)}" if item.cond else ""
            body = "".join(f"<assign location={quoteattr(loc)} expr={quoteattr(expr)}/>" for loc, expr in item.assigns)
            lines.append(f"    <transition event={quoteattr(item.event)}{cond} target=\"step{step + 1}\"" + (f">{body}</transition>" if body else "/>"))
            lines.append("  </state>")
            step += 1
        lines.append(f'  <state id="step{step}">')
        sends = flush_sends()
        if sends:
            lines += ["    <onentry>"] + sends + ["    </onentry>"]
        lines.append('    <transition target="pass"/>')
        lines.append("  </state>")
        for event in case["ignore"]:
            lines.append(f"  <transition event={quoteattr(event)}/>")
        lines.append('  <transition event="*" target="fail">'
                     '<log label="unexpected" expr="_event.name + \' \' + JSON.stringify(_event.data)"/></transition>')
        lines += [
            "</state>",
            '<final id="pass"><onentry><log label="Outcome" expr="\'pass\'"/></onentry></final>',
            '<final id="fail"><onentry><log label="Outcome" expr="\'fail\'"/></onentry></final>',
            "</scxml>",
            "",
        ]
        return "\n".join(lines)

    def write_media(self, case, directory, document):
        named = set(MEDIA_NAME.findall(document + json.dumps(case["backend"] or [])))
        files = {name: self.library[name] for name in sorted(named) if name in self.library}
        if os.path.isdir(directory):
            for stale in set(os.listdir(directory)) - set(files):
                os.remove(os.path.join(directory, stale))
        if not files:
            if os.path.isdir(directory):
                os.rmdir(directory)
            return
        os.makedirs(directory, exist_ok=True)
        for name, content in files.items():
            with open(os.path.join(directory, name), "wb") as out:
                out.write(content)

    def write(self):
        suite = os.path.join(ROOT, self.kind, self.name, "irp")
        sources = os.path.join(suite, "sources")
        os.makedirs(sources, exist_ok=True)
        listed = set()
        for case in self.cases:
            directory = os.path.join(sources, str(case["id"]))
            os.makedirs(directory, exist_ok=True)
            listed.add(str(case["id"]))
            document = self.document(case)
            with open(os.path.join(directory, f"test{case['id']}.scxml"), "w") as out:
                out.write(document)
            script = os.path.join(directory, "backend.json")
            if case["backend"] is not None or case["clock"] or case["websocket"] is not None:
                with open(script, "w") as out:
                    script = {"clock": case["clock"] or "wall", "answers": case["backend"] or []}
                    if case["stream"]:
                        script["stream"] = True
                    if case["websocket"] is not None:
                        script["websocket"] = case["websocket"]
                    json.dump(script, out, indent=2)
                    out.write("\n")
            elif os.path.exists(script):
                os.remove(script)
            logs = os.path.join(directory, "logs.json")
            if case["logs"] is not None:
                with open(logs, "w") as out:
                    json.dump(case["logs"], out, indent=2)
                    out.write("\n")
            elif os.path.exists(logs):
                os.remove(logs)
            self.write_media(case, os.path.join(directory, "media"), document)
        for stale in set(os.listdir(sources)) - listed:
            raise SystemExit(f"{sources}/{stale} is no case of cases.py")
        behaviors = os.path.join(suite, "behaviors")
        if os.path.isdir(behaviors):
            for stale in set(os.listdir(behaviors)) - {f"{name}.scxml" for name in self.behaviors}:
                os.remove(os.path.join(behaviors, stale))
        if self.behaviors:
            os.makedirs(behaviors, exist_ok=True)
            for name, text in sorted(self.behaviors.items()):
                with open(os.path.join(behaviors, f"{name}.scxml"), "w") as out:
                    out.write(text)
        manifest = [
            '<?xml version="1.0"?>',
            f"<!-- The {self.name} document's implementation report plan: each assertion is a",
            '     numbered statement of README.md ("Statements"). -->',
            f'<assertions baseuri="https://github.com/stateforward/floppy/tree/main/{self.kind}/{self.name}/irp/sources">',
        ]
        for assertion, text in self.statements:
            number = assertion.rsplit("-", 1)[1]
            manifest.append(f'  <assert id="{assertion}" specnum="{number}" specid="#{assertion}">')
            manifest.append(f"    <![CDATA[{text}]]>")
            for case in self.cases:
                if case["assertion"] == assertion:
                    manifest.append(f'    <test id="{case["id"]}" conformance="mandatory" manual="false">')
                    manifest.append(f'      <start uri="{case["id"]}/test{case["id"]}.scxml"/>')
                    manifest.append("    </test>")
            manifest.append("  </assert>")
        manifest.append("</assertions>")
        with open(os.path.join(suite, "manifest.xml"), "w") as out:
            out.write("\n".join(manifest) + "\n")
        readme = os.path.join(suite, "README.md")
        head = (
            f"# `{self.name}` implementation report plan\n\n"
            f"The implementation report plan of [`{self.name}.scx`](../{self.name}.scx): the W3C SCXML IRP's\n"
            "manifest format and `Outcome` oracle over the document's numbered\n"
            "statements below, one assertion each in [`manifest.xml`](manifest.xml),\n"
            "tested by the harness documents in [`sources/`](sources). Each harness\n"
            "invokes the document as its child, drives it, and ends in `pass` (logging\n"
            "`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case\n"
            "through `scx run` with the real invoked services (a case's\n"
            "`backend.json` scripts the fake HTTP backend they reach), and\n"
            "`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).\n\n"
            "## Statements\n\n"
        )
        body = "".join(f'- <a id="{assertion}"></a>**{assertion}**. {text}\n' for assertion, text in self.statements)
        with open(readme, "w") as out:
            out.write(head + body)


def load(kind, name):
    path = os.path.join(ROOT, kind, name, "irp", "cases.py")
    suite = Suite(kind, name)
    runpy.run_path(path, init_globals=dict(suite=suite, send=send, expect=expect, offers=offers, snapshot=snapshot, wait=wait, js=js, memory=memory, wav=wav,
                                           request=request, answered=answered,
                                           pcm=pcm, png=png))
    return suite


def main(names):
    for kind in ("bots", "abilities"):
        for name in sorted(os.listdir(os.path.join(ROOT, kind))):
            if names and name not in names:
                continue
            if os.path.exists(os.path.join(ROOT, kind, name, "irp", "cases.py")):
                load(kind, name).write()


if __name__ == "__main__":
    main(sys.argv[1:])
