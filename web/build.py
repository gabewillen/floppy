"""Build the website and workshop into web/dist for GitHub Pages.

The workshop's block catalog is extracted from the XSDs at build time, so the
blocks always match the schemas in this repository. Its runtime is scxml.rs's
browser host as web/build-host.sh builds it into target/web/host: run that
first. The bots, abilities and environments are copied beside the pages, so a
document the workshop opens loads its scripts, schemas and children by the
same relative URIs `scx run` does.
"""
import glob, json, os, re, shutil, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
SRC, DIST = os.path.join(HERE, "src"), os.path.join(HERE, "dist")
HOST = os.path.join(ROOT, "target", "web")


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
facts = subprocess.run([sys.executable, os.path.join(SRC, "register.py")], check=True, capture_output=True, text=True).stdout
write(os.path.join(DIST, "index.html"), fill(read("site.html"), {"/*DATA*/": fill(read("site-data.js"), {"/*FACTS*/": facts}), "/*SHEET*/": read("site-sheet.js"), "/*CRITTER*/": critter}))
write(os.path.join(DIST, "workshop", "index.html"), fill(read("workshop.html"), {"/*CRITTER*/": critter, "/*CATALOG*/": json.dumps(json.loads(catalog), separators=(",", ":"))}))
write(os.path.join(DIST, ".nojekyll"), "")

# The workshop's runtime.
if not os.path.isfile(os.path.join(HOST, "host", "dist", "build-manifest.json")):
    sys.exit("build: the browser host is not built; run sh web/build-host.sh first")
workshop = os.path.join(DIST, "workshop")
shutil.copytree(os.path.join(HOST, "host"), os.path.join(workshop, "host"), dirs_exist_ok=True)
shutil.copy(os.path.join(SRC, "runtime.js"), os.path.join(workshop, "runtime.js"))

# The namespace packages the runtime binds: scx and floppy's, each with its
# schema and its component (the bytes authenticateExtensionComponent hashes).
def package(manifest_path, component_dir):
    with open(manifest_path, encoding="utf-8") as f:
        m = json.load(f)
    name = os.path.basename(os.path.dirname(os.path.dirname(manifest_path)))
    target = os.path.join(workshop, "packages", name)
    os.makedirs(target, exist_ok=True)
    shutil.copy(os.path.join(os.path.dirname(manifest_path), m["schema-artifact"]), target)
    shutil.copy(os.path.join(component_dir, m["extension-artifact"]), target)
    return {"manifest": {"namespaceUri": m["namespace-uri"], "packageVersion": m["package-version"], "extensionArtifact": m["extension-artifact"],
                         "schemaArtifact": m["schema-artifact"], "schemaSha256": m["schema-sha256"], "witWorld": m["wit-world"], "witVersion": m["wit-version"]},
            "schema": f"packages/{name}/{m['schema-artifact']}", "component": f"packages/{name}/{m['extension-artifact']}"}

packages = [package(os.path.join(ROOT, "scxml.rs", "namespaces", "scx", "package", "scx.extension.json"), os.path.join(HOST, "scxml"))]
packages += [package(path, os.path.join(HOST, "components", "namespaces")) for path in sorted(glob.glob(os.path.join(ROOT, "namespaces", "*", "package", "*.extension.json")))]
write(os.path.join(workshop, "packages.json"), json.dumps({"packages": packages}, indent=1))

# The service packages the runtime binds: each one the host build shipped
# (web/build-host.sh's --services), with its component (the bytes
# authenticateServiceComponent hashes), the directory its `--dir` grant
# names under `scx run`, and the environment variables the documents bind
# to service slots (env:<slot>="NAME"), which the TestBench asks for.
with open(os.path.join(HOST, "services", "manifest.tsv"), encoding="utf-8") as f:
    shipped = {row.split("\t")[2] for row in f.read().splitlines() if row.startswith("artifact\t")}
GRANTS = {"sql": "/memory", "files": "/behaviors", "generate": "/media", "speech": "/media", "vision": "/media"}
services = []
for path in sorted(glob.glob(os.path.join(ROOT, "services", "*", "package", "*.service.json"))):
    with open(path, encoding="utf-8") as f:
        m = json.load(f)
    if m["service-artifact"] not in shipped:
        continue
    name = os.path.basename(os.path.dirname(os.path.dirname(path)))
    target = os.path.join(workshop, "packages", "services", name)
    os.makedirs(target, exist_ok=True)
    shutil.copy(os.path.join(HOST, "services", m["service-artifact"]), target)
    services.append({"manifest": {"serviceUri": m["service-uri"], "packageVersion": m["package-version"], "serviceArtifact": m["service-artifact"],
                                  "witWorld": m["wit-world"], "witVersion": m["wit-version"], "httpOrigins": m["http-origins"], "envSlots": m["env-slots"]},
                     "component": f"packages/services/{name}/{m['service-artifact']}", "dir": GRANTS.get(name)})
variables = set()
for top in ("bots", "abilities", "environments"):
    for path in glob.glob(os.path.join(ROOT, top, "**", "*.scx"), recursive=True):
        with open(path, encoding="utf-8") as f:
            variables.update(re.findall(r'\benv:[a-z][a-z0-9_-]*="([A-Za-z_][A-Za-z0-9_]*)"', f.read()))
write(os.path.join(workshop, "services.json"), json.dumps({"services": services, "variables": sorted(variables)}, indent=1))

# Bots, abilities and environments: their documents, scripts and schemas,
# and the list of documents the workshop opens.
documents = sorted(os.path.relpath(p, ROOT).replace(os.sep, "/") for top in ("bots", "abilities", "environments")
                   for p in glob.glob(os.path.join(ROOT, top, "*", "*.scx")) + glob.glob(os.path.join(ROOT, top, "*", "examples", "*", "*.scx")))
write(os.path.join(workshop, "documents.json"), json.dumps({"documents": documents}, indent=1))
for top in ("bots", "abilities", "environments"):
    for path in glob.glob(os.path.join(ROOT, top, "**", "*"), recursive=True):
        rel = os.path.relpath(path, ROOT)
        if os.path.isfile(path) and os.path.splitext(path)[1] in (".scx", ".js", ".json") and "/irp/" not in "/" + rel.replace(os.sep, "/"):
            os.makedirs(os.path.dirname(os.path.join(DIST, rel)), exist_ok=True)
            shutil.copy(path, os.path.join(DIST, rel))
print("built", DIST)
