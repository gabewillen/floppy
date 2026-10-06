//! Read-only DOM for XML values (SCXML 1.0 B.2.1 and B.2.8.1).
//!
//! When the content of a `<data>` element, an external `src` document, or an
//! event payload is XML, the ECMAScript data model exposes it as a DOM
//! structure (official cases 557 and 561). An XML value stays the canonical
//! [`scxml_types::DataValue::Xml`] text; every context build that registers
//! one parses it with the bounded `scxml-xpath` fragment parser and hands the
//! resulting flat node list to [`DOM_INSTALL`], which builds a frozen DOM:
//!
//! * a `Document` (`nodeType` 9) when the fragment is exactly one element
//!   plus optional comments, processing instructions, and white space;
//!   otherwise a `DocumentFragment` (`nodeType` 11) holding every top-level
//!   node;
//! * `Element` (1), `Attr` (2), `Text` (3; CDATA sections are text),
//!   `ProcessingInstruction` (7), and `Comment` (8) nodes with the DOM Core
//!   read accessors: `nodeType`, `nodeName`, `nodeValue`, `parentNode`,
//!   `ownerDocument`, `childNodes`, `firstChild`, `lastChild`,
//!   `previousSibling`, `nextSibling`, `textContent`, `hasChildNodes()`;
//!   on elements `tagName`, `localName`, `namespaceURI`, `prefix`,
//!   `attributes`, `getAttribute[NS]`, `hasAttribute[NS]`,
//!   `getAttributeNode`; on documents and elements
//!   `getElementsByTagName[NS]` (a static, frozen array in document order);
//!   on documents `documentElement`.
//!
//! Contract:
//!
//! * The DOM is immutable: every node, node list, and prototype is frozen
//!   (an event payload's DOM by the adapter's read-only seal, whose proxies
//!   also reject every write with a `TypeError`), and no mutation method
//!   exists, so the XML text a document was built from is always its exact
//!   value. A document converts back to that text
//!   (the adapter's text lookup), and only a document does: a node inside it
//!   has no portable value.
//! * Building and every accessor are iterative (no native recursion), and
//!   the node count is bounded by the parser over a value of at most
//!   [`crate::MAX_VALUE_BYTES`].
//! * [`DOM_INSTALL`] evaluates, once per context on first use, to
//!   `[build, text, build_open]` that the adapter keeps in its own closures:
//!   none is a global (SCXML 1.0 section 5.10 reserves platform-specific variables
//!   to the `_x` root), and the document-to-text table is closed over and
//!   unreachable from scripts.

use scxml_xpath::{NodeId, NodeKind, NodeRef, QName, Store};
use serde_json::{Value, json};

use crate::DataModelError;

/// Evaluates to `[build, text, build_open]`: the DOM builder
/// `(nodes, xml) => document`, the lookup `(value) => xml text` (the text a
/// document was built from; `undefined` for any other value), and the
/// builder of an event payload's DOM, which leaves its nodes unfrozen for
/// the adapter to seal as a read-only system value.
///
/// `nodes` is the flat pre-order list [`dom_source`] emits; each entry
/// starts with its kind and the index of its parent entry (`-1` at top
/// level).
pub(crate) const DOM_INSTALL: &str = r##"(function () {
  "use strict";
  const freeze = Object.freeze;
  const create = Object.create;
  const define = Object.defineProperty;
  const texts = new WeakMap();
  const collect = (root, test) => {
    const found = [];
    const stack = [];
    for (let i = root.childNodes.length - 1; i >= 0; i -= 1) stack.push(root.childNodes[i]);
    while (stack.length > 0) {
      const current = stack.pop();
      if (current.nodeType !== 1) continue;
      if (test(current)) found.push(current);
      for (let i = current.childNodes.length - 1; i >= 0; i -= 1) stack.push(current.childNodes[i]);
    }
    return freeze(found);
  };
  const textOf = (root) => {
    let text = "";
    const stack = [root];
    while (stack.length > 0) {
      const current = stack.pop();
      if (current.nodeType === 3) text += current.data;
      for (let i = current.childNodes.length - 1; i >= 0; i -= 1) stack.push(current.childNodes[i]);
    }
    return text;
  };
  const nsOrNull = (ns) => (ns === null || ns === undefined ? null : String(ns));
  const node = create(Object.prototype);
  node.hasChildNodes = function () { return this.childNodes.length > 0; };
  define(node, "firstChild", { get() { return this.childNodes.length > 0 ? this.childNodes[0] : null; } });
  define(node, "lastChild", {
    get() { return this.childNodes.length > 0 ? this.childNodes[this.childNodes.length - 1] : null; },
  });
  define(node, "textContent", {
    get() {
      if (this.nodeType === 9) return null;
      if (this.nodeType === 1 || this.nodeType === 11) return textOf(this);
      return this.nodeValue;
    },
  });
  const container = create(node);
  container.getElementsByTagName = function (name) {
    const wanted = String(name);
    return collect(this, (element) => wanted === "*" || element.nodeName === wanted);
  };
  container.getElementsByTagNameNS = function (namespace, local) {
    const ns = nsOrNull(namespace);
    const wanted = String(local);
    return collect(this, (element) =>
      (ns === "*" || element.namespaceURI === ns) && (wanted === "*" || element.localName === wanted));
  };
  const element = create(container);
  element.getAttributeNode = function (name) {
    const wanted = String(name);
    const found = this.attributes.find((attribute) => attribute.name === wanted);
    return found === undefined ? null : found;
  };
  element.getAttribute = function (name) {
    const found = this.getAttributeNode(name);
    return found === null ? null : found.value;
  };
  element.hasAttribute = function (name) { return this.getAttributeNode(name) !== null; };
  element.getAttributeNS = function (namespace, local) {
    const ns = nsOrNull(namespace);
    const wanted = String(local);
    const found = this.attributes.find((attribute) => attribute.namespaceURI === ns && attribute.localName === wanted);
    return found === undefined ? null : found.value;
  };
  element.hasAttributeNS = function (namespace, local) { return this.getAttributeNS(namespace, local) !== null; };
  freeze(node);
  freeze(container);
  freeze(element);
  const leaf = (type, name, value) => {
    const created = create(node);
    created.nodeType = type;
    created.nodeName = name;
    created.nodeValue = value;
    return created;
  };
  // `seal` is applied to every built node: `freeze`, or `unsealed` for a
  // DOM the adapter seals itself.
  const unsealed = (value) => value;
  const build = (nodes, xml, seal) => {
    const built = [];
    const children = [];
    const top = [];
    const all = [];
    for (let n = 0; n < nodes.length; n += 1) {
      const entry = nodes[n];
      const kind = entry[0];
      let created;
      if (kind === "e") {
        created = create(element);
        created.nodeType = 1;
        created.nodeName = entry[2];
        created.tagName = entry[2];
        created.namespaceURI = entry[3];
        created.localName = entry[4];
        created.prefix = entry[5];
        created.nodeValue = null;
        const attributes = [];
        for (let a = 0; a < entry[6].length; a += 1) {
          const item = entry[6][a];
          const attribute = leaf(2, item[0], item[4]);
          attribute.name = item[0];
          attribute.namespaceURI = item[1];
          attribute.localName = item[2];
          attribute.prefix = item[3];
          attribute.value = item[4];
          attribute.ownerElement = created;
          attribute.parentNode = null;
          attribute.childNodes = seal([]);
          attribute.previousSibling = null;
          attribute.nextSibling = null;
          attributes.push(attribute);
          all.push(attribute);
        }
        created.attributes = seal(attributes);
      } else if (kind === "t") {
        created = leaf(3, "#text", entry[2]);
        created.data = entry[2];
      } else if (kind === "c") {
        created = leaf(8, "#comment", entry[2]);
        created.data = entry[2];
      } else {
        created = leaf(7, entry[2], entry[3]);
        created.target = entry[2];
        created.data = entry[3];
      }
      built.push(created);
      children.push([]);
      all.push(created);
      const parent = entry[1];
      if (parent < 0) top.push(created); else children[parent].push(created);
    }
    const elements = top.filter((item) => item.nodeType === 1);
    const document = elements.length === 1 &&
      top.every((item) => item.nodeType !== 3 || /^[\x20\x09\x0d\x0a]*$/.test(item.data));
    const root = create(container);
    root.nodeType = document ? 9 : 11;
    root.nodeName = document ? "#document" : "#document-fragment";
    root.nodeValue = null;
    root.parentNode = null;
    root.ownerDocument = null;
    root.previousSibling = null;
    root.nextSibling = null;
    if (document) root.documentElement = elements[0];
    const rootChildren = document ? top.filter((item) => item.nodeType !== 3) : top;
    const link = (parent, list) => {
      for (let i = 0; i < list.length; i += 1) {
        list[i].parentNode = parent;
        list[i].previousSibling = i > 0 ? list[i - 1] : null;
        list[i].nextSibling = i + 1 < list.length ? list[i + 1] : null;
      }
      parent.childNodes = seal(list);
    };
    link(root, rootChildren);
    for (let t = 0; t < top.length; t += 1) {
      const item = top[t];
      if (!rootChildren.includes(item)) {
        item.parentNode = null;
        item.previousSibling = null;
        item.nextSibling = null;
      }
    }
    for (let i = 0; i < built.length; i += 1) link(built[i], children[i]);
    for (let n = 0; n < all.length; n += 1) {
      all[n].ownerDocument = root;
      seal(all[n]);
    }
    seal(root);
    texts.set(root, xml);
    return root;
  };
  return [
    freeze((nodes, xml) => build(nodes, xml, freeze)),
    freeze((value) => texts.get(value)),
    freeze((nodes, xml) => build(nodes, xml, unsealed)),
  ];
})()"##;

/// The parameter name under which [`dom_source`] calls the DOM builder:
/// adapter value sources are evaluated as the body of a function taking
/// the builder under this name, never as a global.
pub(crate) const DOM_BUILDER: &str = "__dom";

/// Program text that evaluates to the read-only DOM of `xml` where the DOM
/// builder is bound to [`DOM_BUILDER`].
///
/// # Errors
///
/// [`DataModelError::InvalidInput`] when `xml` is not a well-formed,
/// in-bound XML fragment.
pub(crate) fn dom_source(xml: &str) -> Result<String, DataModelError> {
    let nodes =
        serde_json::to_string(&node_list(xml)?).map_err(|_| DataModelError::InvalidInput)?;
    let text = serde_json::to_string(xml).map_err(|_| DataModelError::InvalidInput)?;
    Ok(format!("{DOM_BUILDER}({nodes},{text})"))
}

/// Whether `xml` is a well-formed, in-bound XML fragment (the only text an
/// XML value may carry back out of a context).
pub(crate) fn is_markup(xml: &str) -> bool {
    node_list(xml).is_ok()
}

/// Whether `text` is an XML document: a well-formed, in-bound fragment of
/// exactly one element plus optional comments, processing instructions,
/// and XML white space (the form B.2.1 turns into a DOM `Document`).
pub(crate) fn is_document(text: &str) -> bool {
    let mut store = Store::new();
    let Ok(top) = store.parse_fragment(text) else {
        return false;
    };
    let mut elements = 0_usize;
    for id in top {
        let reference = NodeRef::node(id);
        match store.kind(reference) {
            Ok(NodeKind::Element) => elements = elements.saturating_add(1),
            Ok(NodeKind::Text) => {
                if !store
                    .string_value(reference)
                    .is_ok_and(|value| value.chars().all(scxml_types::is_xml_space))
                {
                    return false;
                }
            }
            Ok(NodeKind::Comment | NodeKind::ProcessingInstruction) => {}
            _ => return false,
        }
    }
    elements == 1
}

/// The flat pre-order node list of `xml` (see [`DOM_INSTALL`]).
fn node_list(xml: &str) -> Result<Vec<Value>, DataModelError> {
    let invalid = |_| DataModelError::InvalidInput;
    let mut store = Store::new();
    let top = store.parse_fragment(xml).map_err(invalid)?;
    let mut nodes = Vec::new();
    let mut stack: Vec<(NodeId, i64)> = top.iter().rev().map(|id| (*id, -1)).collect();
    while let Some((id, parent)) = stack.pop() {
        let index = i64::try_from(nodes.len()).map_err(|_| DataModelError::InvalidInput)?;
        let reference = NodeRef::node(id);
        let entry = match store.kind(reference).map_err(invalid)? {
            NodeKind::Element => {
                let name = store
                    .name(reference)
                    .map_err(invalid)?
                    .ok_or(DataModelError::InvalidInput)?;
                let mut attributes = Vec::new();
                for attribute in store.attributes(id).map_err(invalid)? {
                    let attribute_ref = NodeRef::node(attribute);
                    let attribute_name = store
                        .name(attribute_ref)
                        .map_err(invalid)?
                        .ok_or(DataModelError::InvalidInput)?;
                    let mut item = name_parts(&attribute_name);
                    item.push(json!(store.string_value(attribute_ref).map_err(invalid)?));
                    attributes.push(Value::Array(item));
                }
                for child in store.children(id).map_err(invalid)?.rev() {
                    stack.push((child, index));
                }
                let mut entry = vec![json!("e"), json!(parent)];
                entry.extend(name_parts(&name));
                entry.push(Value::Array(attributes));
                entry
            }
            NodeKind::Text => vec![
                json!("t"),
                json!(parent),
                json!(store.string_value(reference).map_err(invalid)?),
            ],
            NodeKind::Comment => vec![
                json!("c"),
                json!(parent),
                json!(store.string_value(reference).map_err(invalid)?),
            ],
            NodeKind::ProcessingInstruction => {
                let target = store
                    .name(reference)
                    .map_err(invalid)?
                    .ok_or(DataModelError::InvalidInput)?;
                vec![
                    json!("p"),
                    json!(parent),
                    json!(target.local_name),
                    json!(store.string_value(reference).map_err(invalid)?),
                ]
            }
            NodeKind::Root | NodeKind::Attribute | NodeKind::Namespace => {
                return Err(DataModelError::InvalidInput);
            }
        };
        nodes.push(Value::Array(entry));
    }
    Ok(nodes)
}

/// `[lexical name, namespace URI or null, local name, prefix or null]`.
fn name_parts(name: &QName) -> Vec<Value> {
    vec![
        json!(name.lexical()),
        json!(name.namespace_uri),
        json!(name.local_name),
        json!(name.prefix),
    ]
}
