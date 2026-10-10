"""Build the website and workshop into web/dist for GitHub Pages.

The workshop's block catalog is extracted from the XSDs at build time, so the
blocks always match the schemas in this repository.
"""
import json, os, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
SRC, DIST = os.path.join(HERE, "src"), os.path.join(HERE, "dist")


def read(name):
    with open(os.path.join(SRC, name), encoding="utf-8") as f:
        return f.read()


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


def fill(template, parts):
    for marker, value in parts.items():
        if template.count(marker) != 1:
            sys.exit(f"build: expected one {marker} in the template, found {template.count(marker)}")
        template = template.replace(marker, value)
    return template


catalog = subprocess.run([sys.executable, os.path.join(SRC, "extract.py")], check=True, capture_output=True, text=True).stdout
critter = read("critter.bundle.js")
write(os.path.join(DIST, "index.html"), fill(read("site.html"), {"/*DATA*/": read("site-data.js"), "/*SHEET*/": read("site-sheet.js"), "/*CRITTER*/": critter}))
write(os.path.join(DIST, "workshop", "index.html"), fill(read("workshop.html"), {"/*CRITTER*/": critter, "/*CATALOG*/": json.dumps(json.loads(catalog), separators=(",", ":"))}))
write(os.path.join(DIST, ".nojekyll"), "")
print("built", DIST)
