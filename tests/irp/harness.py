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
  wait(delay)                       lets `delay` pass (a delayed send to
                                    itself; give the case clock="wall")
  expect(event, cond=None, assigns=())
                                    waits for `event` (from the child, or
                                    any event the harness receives) and
                                    takes it when `cond` holds, assigning
                                    each (location, expr)
  ignore(event)                     drops `event` wherever it arrives

and the case passes when every expect was met in order. Any other event
the harness receives (error.* included) fails it, logging the event.

A suite's media is a library of files, `S.media({name: bytes})` (built
with wav, pcm and png below): each case's irp/sources/<id>/media/ holds
exactly the library files its document or backend names as `media/<name>`,
so a name the library lacks (an output, or a file a case expects missing)
is never there.
"""

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


def js(value):
    return json.dumps(value, sort_keys=False)


def pcm(sample=0, seconds=0.1, rate=16000, channels=1):
    """Raw signed 16-bit little-endian PCM, every sample `sample`."""
    return struct.pack("<h", sample) * (round(seconds * rate) * channels)


def wav(sample=0, seconds=0.1, rate=16000, channels=1):
    """A canonical PCM WAV container of pcm(sample, seconds, rate, channels)."""
    data = pcm(sample, seconds, rate, channels)
    return (b"RIFF" + struct.pack("<I", 36 + len(data)) + b"WAVE"
            + b"fmt " + struct.pack("<IHHIIHH", 16, 1, channels, rate, rate * channels * 2, channels * 2, 16)
            + b"data" + struct.pack("<I", len(data)) + data)


def png(width=1, height=1):
    """A white 8-bit RGB PNG."""
    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))
    rows = b"".join(b"\x00" + b"\xff" * (3 * width) for _ in range(height))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b""))


MEDIA_NAME = re.compile(r"media/([A-Za-z0-9_.-]+)")


class Suite:
    def __init__(self, kind, name):
        self.kind, self.name = kind, name
        self.statements = []
        self.cases = []
        self.library = {}

    def media(self, files):
        self.library.update(files)

    def statement(self, number, text):
        self.statements.append((f"{self.name}-{number}", text))

    def case(self, test_id, number, comment, steps, data=(), params=(), ignore=(), backend=None, clock=None):
        self.cases.append(dict(id=test_id, assertion=f"{self.name}-{number}", comment=comment, steps=steps,
                               data=data, params=params, ignore=ignore, backend=backend, clock=clock))

    # ------------------------------------------------------------ writing
    def document(self, case):
        name = self.name
        lines = [
            '<?xml version="1.0" encoding="UTF-8"?>',
            f"<!-- {case['assertion']}: {escape(case['comment'])} -->",
            '<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="harness">',
        ]
        if case["data"]:
            lines.append("<datamodel>")
            for location, expr in case["data"]:
                lines.append(f"  <data id={quoteattr(location)} expr={quoteattr(expr)}/>")
            lines.append("</datamodel>")
        lines.append('<state id="harness" initial="boot">')
        invoke = f'  <invoke id="{name}" type="scxml" src="../../../{name}.scx"'
        if case["params"]:
            lines.append(invoke + ">")
            for pname, expr in case["params"]:
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
            if case["backend"] is not None or case["clock"]:
                with open(script, "w") as out:
                    json.dump({"clock": case["clock"] or "wall", "answers": case["backend"] or []}, out, indent=2)
                    out.write("\n")
            elif os.path.exists(script):
                os.remove(script)
            self.write_media(case, os.path.join(directory, "media"), document)
        for stale in set(os.listdir(sources)) - listed:
            raise SystemExit(f"{sources}/{stale} is no case of cases.py")
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
    runpy.run_path(path, init_globals=dict(suite=suite, send=send, expect=expect, wait=wait, js=js, wav=wav, pcm=pcm, png=png))
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
