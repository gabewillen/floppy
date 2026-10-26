"""Read the parts register's facts from this repository and print them as JSON:
every service package (services/*/package/*.service.json), every extension
namespace package (namespaces/*/package/*.extension.json and its XSD), and,
for each, the documents that use it. site-data.js keeps the prose (summaries,
snippets, grants) and takes these facts over its own, so the register lists
every package and element main has.
"""
import glob, json, os, re, sys, xml.etree.ElementTree as ET

XS = '{http://www.w3.org/2001/XMLSchema}'
ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..')
GH = 'https://github.com/stateforward/floppy/'


def documents():
    out = {}
    for top in ('bots', 'abilities', 'environments'):
        for path in glob.glob(os.path.join(ROOT, top, '**', '*.scx'), recursive=True):
            rel = os.path.relpath(path, ROOT).replace(os.sep, '/')
            if '/irp/' in '/' + rel or '/examples/' in rel:
                continue
            with open(path, encoding='utf-8') as f:
                out['/'.join(rel.split('/')[:2])] = out.get('/'.join(rel.split('/')[:2]), '') + f.read()
    return out


def elements(xsd):
    root = ET.parse(xsd).getroot()
    out = []
    for el in root.findall(XS + 'element'):
        attrs = []
        for a in el.iter(XS + 'attribute'):
            if a.get('name'):
                attrs.append(a.get('name') + ('' if a.get('use') == 'required' else '?'))
        out.append({'n': el.get('name'), 'attrs': attrs})
    return out


docs = documents()
facts = {'services': [], 'namespaces': []}
for manifest in sorted(glob.glob(os.path.join(ROOT, 'services', '*', 'package', '*.service.json'))):
    with open(manifest, encoding='utf-8') as f:
        m = json.load(f)
    name = os.path.basename(os.path.dirname(os.path.dirname(manifest)))
    uri = m['service-uri']
    facts['services'].append({
        'id': name, 'uri': uri, 'ver': m['package-version'], 'artifact': m['service-artifact'],
        'wit': f"{m['wit-world']} {m['wit-version']}", 'env': m['env-slots'], 'origins': m['http-origins'],
        'used': sorted(d for d, text in docs.items() if f'type="{uri}"' in text or f'type="{uri}@' in text),
    })
for manifest in sorted(glob.glob(os.path.join(ROOT, 'namespaces', '*', 'package', '*.extension.json'))):
    with open(manifest, encoding='utf-8') as f:
        m = json.load(f)
    name = os.path.basename(os.path.dirname(os.path.dirname(manifest)))
    uri = m['namespace-uri']
    els = elements(os.path.join(os.path.dirname(manifest), m['schema-artifact']))
    users = {d: text for d, text in docs.items() if uri in text}
    for e in els:
        e['uses'] = sum(1 for text in users.values() if re.search(r'<\w+:' + re.escape(e['n']) + r'[\s/>]', text))
    facts['namespaces'].append({
        'id': name, 'uri': uri, 'ver': m['package-version'], 'artifact': m['extension-artifact'],
        'wit': f"{m['wit-world']} {m['wit-version']}", 'elements': els, 'used': sorted(users),
    })
json.dump(facts, sys.stdout, separators=(',', ':'))
