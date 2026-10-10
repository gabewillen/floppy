"""Read the SCXML 1.0 XSDs (scxml.rs) and the extension namespace XSDs (scxml.rs + floppy)
and emit the block catalog the workshop is generated from."""
import json, os, sys, xml.etree.ElementTree as ET
XS='{http://www.w3.org/2001/XMLSchema}'
ROOT=os.path.join(os.path.dirname(os.path.abspath(__file__)),'..','..')
W3C=os.path.join(ROOT,'scxml.rs/crates/scxml-validator-xsd/schemas/w3c-scxml-1.0/')
SX=os.path.join(ROOT,'scxml.rs/namespaces/')
FL=os.path.join(ROOT,'namespaces/')
groups,agroups,stypes,ctypes,elems={},{},{},{},{}
def load(path):
    r=ET.parse(path).getroot()
    for c in r:
        n=c.get('name')
        if c.tag==XS+'group':groups[n]=c
        elif c.tag==XS+'attributeGroup':agroups[n]=c
        elif c.tag==XS+'simpleType':stypes[n]=c
        elif c.tag==XS+'complexType':ctypes[n]=c
        elif c.tag==XS+'element':elems[n]=c
    return r
for f in ['scxml-datatypes','scxml-attribs','scxml-contentmodels','scxml-module-core','scxml-module-data','scxml-module-external','scxml']:
    load(W3C+f+'.xsd')
def enum_of(st):
    if st is None:return None
    vals=[e.get('value') for e in st.iter(XS+'enumeration')]
    if vals:return vals
    for u in st.iter(XS+'union'):
        out=[]
        for m in (u.get('memberTypes') or '').split():
            v=enum_of(stypes.get(m));out+=v or []
        for s in u.findall(XS+'simpleType'):out+=enum_of(s) or []
        return out or None
    for r in st.iter(XS+'restriction'):
        b=r.get('base')
        if b in stypes:return enum_of(stypes[b])
    return None
def attrs_of(node,acc):
    for c in node:
        if c.tag==XS+'attribute' and c.get('name'):
            t=c.get('type') or ''
            en=enum_of(stypes.get(t)) if t in stypes else enum_of(c.find(XS+'simpleType'))
            acc.append({k:v for k,v in dict(name=c.get('name'),type=t.replace('xsd:','').replace('.datatype','') or 'string',
              use=c.get('use','optional'),default=c.get('default'),fixed=c.get('fixed'),enum=en).items() if v})
        elif c.tag==XS+'attributeGroup' and c.get('ref'):attrs_of(agroups[c.get('ref')],acc)
        elif c.tag in (XS+'complexType',XS+'simpleContent',XS+'complexContent',XS+'extension'):attrs_of(c,acc)
    return acc
def kids_of(node,acc):
    for c in node:
        if c.tag==XS+'element' and c.get('ref'):acc.add(c.get('ref').split(':')[-1])
        elif c.tag==XS+'group' and c.get('ref'):
            r=c.get('ref')
            if r=='scxml.extra.content':acc.add('##other')
            else:kids_of(groups[r],acc)
        elif c.tag==XS+'any':acc.add('##other')
        elif c.tag in (XS+'sequence',XS+'choice',XS+'all',XS+'complexType',XS+'complexContent',XS+'extension'):kids_of(c,acc)
    return acc
def text_of(node):
    return any(x.get('mixed')=='true' for x in node.iter(XS+'complexType')) or node.find('.//'+XS+'simpleContent') is not None
def describe(name,e):
    ct=ctypes.get(e.get('type')) if e.get('type') else e.find(XS+'complexType')
    if ct is None: return {'attrs':[],'children':[],'text':False}
    a=attrs_of(ct,[]);
    a=[x for x in a if x['name']!='space' ]
    return {'attrs':a,'children':sorted(kids_of(ct,set())),'text':text_of(ct)}
cat={'scxml':{},'ext':{},'globalAttrs':[]}
for n,e in elems.items():cat['scxml'][n]=describe(n,e)
# extension namespaces
NS=[('event',SX+'event/package/event.xsd','https://github.com/stateforward/scxml.rs/namespaces/event'),
    ('env',SX+'env/package/env.xsd','https://github.com/stateforward/scxml.rs/namespaces/env'),
    ('datamodel',SX+'datamodel/package/datamodel.xsd','https://github.com/stateforward/scxml.rs/namespaces/datamodel'),
    ('scx',SX+'scx/package/scx.xsd','https://github.com/stateforward/scxml.rs/namespaces/scx')]+[
    (p,FL+p+'/package/'+p+'.xsd','https://github.com/stateforward/floppy/namespaces/'+p) for p in ['decision','bot','conversation','migration']]
for p,path,uri in NS:
    r=ET.parse(path).getroot();ns={'uri':uri,'elements':{}}
    tops={c.get('name') for c in r if c.tag==XS+'element'}
    refd=set()
    for c in r.iter(XS+'element'):
        if c.get('ref'):refd.add(c.get('ref').split(':')[-1])
    for c in r:
        if c.tag==XS+'element':
            ct=c.find(XS+'complexType');a=attrs_of(ct,[]) if ct is not None else []
            ns['elements'][c.get('name')]={'attrs':a,'children':sorted(kids_of(ct,set())) if ct is not None else [],'text':ct is not None and text_of(c),'nested':c.get('name') in refd}
        if c.tag==XS+'attribute':cat['globalAttrs'].append({'prefix':p,'name':c.get('name'),'type':(c.get('type') or 'string').split(':')[-1]})
    cat['ext'][p]=ns
json.dump(cat,sys.stdout,indent=1)
