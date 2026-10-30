//! Flat arena XML data tree: nodes, inspection, copying, mutation, ordering,
//! and serialization.
//!
//! Every tree node is one fixed-size [`Node`] record in a single `Vec`,
//! addressed by index. Structure is intrusive links (parent, first/last
//! child, previous/next sibling, first/last attribute), so walks allocate
//! nothing and insertion or removal next to a known node is constant time.
//! Element, attribute, and processing-instruction names are interned once
//! per store ([`Names`]) and compared as small integers. Text, attribute,
//! comment, and processing-instruction values live in a leaf table
//! ([`Leaf`]) that keeps each value's XPath `number()` alongside its text,
//! so numeric comparisons neither reparse nor copy the text; the text stays
//! the single source of every string-typed result. Every walk is iterative
//! and bounded.

use alloc::borrow::Cow;
use alloc::borrow::ToOwned as _;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::Cell;

use crate::number::string_to_number;
use crate::{Error, Limit, XmlError, is_ncname, limits};

/// Node-sets up to this size sort by ancestry keys; larger ones by one
/// tree walk.
const SMALL_SORT: usize = 16;

/// The XML namespace bound to the `xml` prefix.
pub(crate) const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
/// The namespace of namespace declarations.
pub(crate) const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";

/// Absent link or index.
pub(crate) const NIL: u32 = u32::MAX;

/// Handle of one tree node (document, element, attribute, text, comment, or
/// processing instruction) in a [`Store`].
///
/// Handles are plain indices: they stay valid until [`Store::compact`], which
/// invalidates every handle except those it rewrites.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeId(u32);

impl NodeId {
    fn index(self) -> usize {
        // u32 -> usize never truncates on the supported 32- and 64-bit targets.
        self.0 as usize
    }

    pub(crate) const fn raw(self) -> u32 {
        self.0
    }

    /// The handle for a present link (`None` for [`NIL`]).
    pub(crate) const fn from_link(link: u32) -> Option<Self> {
        if link == NIL { None } else { Some(Self(link)) }
    }
}

/// Any XPath node: a tree node or a synthesized namespace node of an element.
///
/// Namespace-node handles identify the element's in-scope namespace by
/// position; they are valid only until the element's ancestry or namespace
/// declarations change.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeRef {
    id: NodeId,
    /// 0 for the tree node itself; `k > 0` for in-scope namespace `k - 1`.
    namespace: u32,
}

impl NodeRef {
    /// Filler for unused inline node-list slots; never read as a node.
    pub(crate) const PLACEHOLDER: Self = Self {
        id: NodeId(NIL),
        namespace: 0,
    };

    /// The XPath node for a tree node.
    #[must_use]
    pub const fn node(id: NodeId) -> Self {
        Self { id, namespace: 0 }
    }

    /// The tree node, or `None` for a namespace node.
    #[must_use]
    pub const fn id(self) -> Option<NodeId> {
        if self.namespace == 0 {
            Some(self.id)
        } else {
            None
        }
    }

    pub(crate) fn namespace_of(element: NodeId, index: usize) -> Result<Self, Error> {
        let namespace = u32::try_from(index)
            .ok()
            .and_then(|index| index.checked_add(1))
            .ok_or(Error::InvalidNode)?;
        Ok(Self {
            id: element,
            namespace,
        })
    }

    /// Owning element of a namespace node, with its in-scope index.
    pub(crate) fn namespace_parts(self) -> Option<(NodeId, usize)> {
        // u32 -> usize never truncates on supported targets.
        self.namespace
            .checked_sub(1)
            .map(|index| (self.id, index as usize))
    }

    pub(crate) const fn owner(self) -> NodeId {
        self.id
    }
}

impl From<NodeId> for NodeRef {
    fn from(id: NodeId) -> Self {
        Self::node(id)
    }
}

/// XPath node kinds.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NodeKind {
    /// Document (root) node.
    Root,
    /// Element node.
    Element,
    /// Attribute node.
    Attribute,
    /// Text node.
    Text,
    /// Comment node.
    Comment,
    /// Processing-instruction node.
    ProcessingInstruction,
    /// Namespace node.
    Namespace,
}

/// An expanded XML name with its lexical prefix.
///
/// Names are compared by `(namespace_uri, local_name)`; the prefix only
/// guides serialization and `name()`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct QName {
    /// Namespace URI; `None` is the null namespace.
    pub namespace_uri: Option<String>,
    /// Local part (an `NCName`).
    pub local_name: String,
    /// Lexical prefix, when any.
    pub prefix: Option<String>,
}

impl QName {
    /// A name in the null namespace.
    ///
    /// # Errors
    ///
    /// [`Error::Xml`] with [`XmlError::Malformed`] when `name` is not an
    /// `NCName`.
    pub fn local(name: &str) -> Result<Self, Error> {
        Self::new(None, name, None)
    }

    /// A validated expanded name.
    ///
    /// # Errors
    ///
    /// [`Error::Xml`] with [`XmlError::Malformed`] for a non-`NCName` local
    /// part or prefix, a prefix without a namespace, an empty namespace
    /// URI, or an over-bound name.
    pub fn new(
        namespace_uri: Option<&str>,
        local_name: &str,
        prefix: Option<&str>,
    ) -> Result<Self, Error> {
        let valid = is_ncname(local_name)
            && prefix.is_none_or(is_ncname)
            && (prefix.is_none() || namespace_uri.is_some())
            && namespace_uri
                .is_none_or(|uri| !uri.is_empty() && uri.len() <= limits::MAX_XML_BYTES);
        if !valid {
            return Err(Error::Xml(XmlError::Malformed));
        }
        Ok(Self {
            namespace_uri: namespace_uri.map(str::to_owned),
            local_name: local_name.to_owned(),
            prefix: prefix.map(str::to_owned),
        })
    }

    /// `prefix:local` or `local`.
    #[must_use]
    pub fn lexical(&self) -> String {
        match &self.prefix {
            Some(prefix) => format!("{prefix}:{}", self.local_name),
            None => self.local_name.clone(),
        }
    }

    fn bytes(&self) -> usize {
        self.local_name
            .len()
            .saturating_add(self.namespace_uri.as_ref().map_or(0, String::len))
            .saturating_add(self.prefix.as_ref().map_or(0, String::len))
    }
}

/// One namespace binding: `(prefix, uri)`; `None` prefix is the default.
pub(crate) type Binding = (Option<String>, String);

/// Kind of one arena [`Node`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    Root,
    Element,
    Attribute,
    Text,
    Comment,
    Pi,
}

/// One arena node: kind, links, interned name, and value index.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Node {
    pub(crate) kind: Kind,
    /// Parent element or document; for an attribute, its element.
    pub(crate) parent: u32,
    /// Previous sibling in the parent's child or attribute list.
    pub(crate) prev: u32,
    /// Next sibling in the parent's child or attribute list.
    pub(crate) next: u32,
    pub(crate) first_child: u32,
    pub(crate) last_child: u32,
    pub(crate) child_count: u32,
    pub(crate) first_attribute: u32,
    pub(crate) last_attribute: u32,
    pub(crate) attribute_count: u32,
    /// Interned name of an element, attribute, or processing instruction
    /// (its target); [`NIL`] otherwise.
    pub(crate) name: u32,
    /// Leaf index of an attribute, text, comment, or processing instruction
    /// (its data); declarations index of an element with source namespace
    /// declarations; [`NIL`] otherwise.
    pub(crate) value: u32,
}

impl Node {
    const fn new(kind: Kind, name: u32, value: u32) -> Self {
        Self {
            kind,
            parent: NIL,
            prev: NIL,
            next: NIL,
            first_child: NIL,
            last_child: NIL,
            child_count: 0,
            first_attribute: NIL,
            last_attribute: NIL,
            attribute_count: 0,
            name,
            value,
        }
    }

    pub(crate) const fn has_parent(&self) -> bool {
        self.parent != NIL
    }

    pub(crate) const fn node_kind(&self) -> NodeKind {
        match self.kind {
            Kind::Root => NodeKind::Root,
            Kind::Element => NodeKind::Element,
            Kind::Attribute => NodeKind::Attribute,
            Kind::Text => NodeKind::Text,
            Kind::Comment => NodeKind::Comment,
            Kind::Pi => NodeKind::ProcessingInstruction,
        }
    }
}

/// Bit pattern of a [`Leaf`] number not computed yet: a NaN payload that
/// [`string_to_number`] never returns (it yields only the canonical NaN).
const NUMBER_UNSET: u64 = 0x7FF8_5C58_4D4C_0001;

/// One leaf value: its text and, once first needed, that text's XPath
/// `number()` (kept so numeric comparisons never reparse).
#[derive(Clone, Debug)]
pub(crate) struct Leaf {
    pub(crate) text: String,
    number: Cell<u64>,
}

impl Leaf {
    const fn new(text: String) -> Self {
        Self {
            text,
            number: Cell::new(NUMBER_UNSET),
        }
    }

    /// XPath `number()` of the text, computed once.
    pub(crate) fn number(&self) -> f64 {
        let bits = self.number.get();
        if bits != NUMBER_UNSET {
            return f64::from_bits(bits);
        }
        let number = string_to_number(&self.text);
        self.number.set(number.to_bits());
        number
    }
}

/// One interned name and the id of its expanded name (the first interned
/// name with the same namespace URI and local part).
#[derive(Clone, Debug)]
pub(crate) struct NameEntry {
    pub(crate) name: QName,
    pub(crate) expanded: u32,
    /// [`name_hash`] of the local part.
    hash: u64,
}

/// FNV-1a digest of a local name (deterministic; no hasher seed).
fn name_hash(local: &str) -> u64 {
    local.bytes().fold(0xCBF2_9CE4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01B3)
    })
}

/// Name interner: each distinct `(namespace, local, prefix)` once, found
/// through an open-addressing index hashed on the local part, so a lookup
/// hashes the name once and compares text only for equal hashes.
#[derive(Clone, Debug, Default)]
pub(crate) struct Names {
    pub(crate) entries: Vec<NameEntry>,
    /// Entry ids by local-name hash, linear probing; [`NIL`] is empty. The
    /// length is zero or a power of two at least twice the entry count.
    index: Vec<u32>,
}

impl Names {
    /// Entries (with ids) whose local part hashes to `hash`, in probe order.
    fn chain(&self, hash: u64) -> impl Iterator<Item = (u32, &NameEntry)> {
        let mask = self.index.len().saturating_sub(1);
        // Truncation keeps the low bits, which index the table.
        #[allow(clippy::cast_possible_truncation, reason = "masked to the table size")]
        let start = (hash as usize) & mask;
        (0..self.index.len())
            .map(move |offset| {
                self.index
                    .get(start.wrapping_add(offset) & mask)
                    .copied()
                    .unwrap_or(NIL)
            })
            .take_while(|slot| *slot != NIL)
            .filter_map(|slot| self.entries.get(slot as usize).map(|entry| (slot, entry)))
            .filter(move |(_, entry)| entry.hash == hash)
    }

    fn find(&self, uri: Option<&str>, local: &str, prefix: Option<&str>) -> Option<u32> {
        self.chain(name_hash(local))
            .find(|(_, entry)| {
                entry.name.local_name == local
                    && entry.name.namespace_uri.as_deref() == uri
                    && entry.name.prefix.as_deref() == prefix
            })
            .map(|(id, _)| id)
    }

    /// The expanded-name id of `(uri, local)`, when any interned name has it.
    pub(crate) fn expanded(&self, uri: Option<&str>, local: &str) -> Option<u32> {
        self.chain(name_hash(local))
            .find(|(_, entry)| {
                entry.name.local_name == local && entry.name.namespace_uri.as_deref() == uri
            })
            .map(|(_, entry)| entry.expanded)
    }

    /// Interns `name`, reusing an existing entry.
    pub(crate) fn intern(&mut self, name: QName) -> Result<u32, Error> {
        if let Some(id) = self.find(
            name.namespace_uri.as_deref(),
            &name.local_name,
            name.prefix.as_deref(),
        ) {
            return Ok(id);
        }
        self.insert(name)
    }

    /// Interns the name with these parts, copying them only when new.
    pub(crate) fn intern_parts(
        &mut self,
        uri: Option<&str>,
        local: &str,
        prefix: Option<&str>,
    ) -> Result<u32, Error> {
        if let Some(id) = self.find(uri, local, prefix) {
            return Ok(id);
        }
        self.insert(QName {
            namespace_uri: uri.map(str::to_owned),
            local_name: local.to_owned(),
            prefix: prefix.map(str::to_owned),
        })
    }

    /// Appends a name known not to be interned yet.
    fn insert(&mut self, name: QName) -> Result<u32, Error> {
        let id = u32::try_from(self.entries.len())
            .ok()
            .filter(|id| *id != NIL)
            .ok_or(Error::Limit(Limit::StoreNodes))?;
        let expanded = self
            .expanded(name.namespace_uri.as_deref(), &name.local_name)
            .unwrap_or(id);
        let hash = name_hash(&name.local_name);
        self.entries.push(NameEntry {
            name,
            expanded,
            hash,
        });
        if self.entries.len().saturating_mul(2) > self.index.len() {
            self.rebuild();
        } else {
            self.place(id, hash);
        }
        Ok(id)
    }

    /// Puts entry `id` in its probe chain (the table has a free slot).
    fn place(&mut self, id: u32, hash: u64) {
        let mask = self.index.len().saturating_sub(1);
        #[allow(clippy::cast_possible_truncation, reason = "masked to the table size")]
        let mut slot = (hash as usize) & mask;
        for _ in 0..self.index.len() {
            match self.index.get_mut(slot) {
                Some(free) if *free == NIL => {
                    *free = id;
                    return;
                }
                _ => slot = slot.wrapping_add(1) & mask,
            }
        }
    }

    /// Rebuilds the index for the current entries.
    fn rebuild(&mut self) {
        let capacity = self
            .entries
            .len()
            .saturating_mul(2)
            .max(16)
            .checked_next_power_of_two()
            .unwrap_or(usize::MAX);
        self.index.clear();
        self.index.resize(capacity, NIL);
        let placements: Vec<(u32, u64)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(id, entry)| u32::try_from(id).ok().map(|id| (id, entry.hash)))
            .collect();
        for (id, hash) in placements {
            self.place(id, hash);
        }
    }

    /// Drops every entry interned at or after `length`.
    fn truncate(&mut self, length: usize) {
        if self.entries.len() > length {
            self.entries.truncate(length);
            self.rebuild();
        }
    }
}

/// How [`Store::assign`] places its value relative to the target
/// (SCXML XPath data model `<assign type="...">`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssignMode {
    /// `replacechildren` (default): the value replaces all children.
    ReplaceChildren,
    /// `firstchild`: the value is inserted before the first child.
    FirstChild,
    /// `lastchild`: the value is appended after the last child.
    LastChild,
    /// `previoussibling`: the value is inserted before the target.
    PreviousSibling,
    /// `nextsibling`: the value is inserted after the target.
    NextSibling,
    /// `replace`: the value replaces the target itself.
    Replace,
    /// `delete`: the target is removed; the value is ignored.
    Delete,
    /// `addattribute`: the target gains (or updates) attribute `attr` whose
    /// value is the string-value of the value.
    AddAttribute(QName),
}

/// The value of one [`Store::assign`].
#[derive(Clone, Copy, Debug)]
pub enum AssignValue<'a> {
    /// Nodes to deep-copy, in the order given (callers pass document order).
    Nodes(&'a [NodeRef]),
    /// A string placed as one text node (no node when empty).
    Text(&'a str),
    /// No value (for [`AssignMode::Delete`]).
    None,
}

/// The children or attributes of one node, in document order.
#[derive(Clone, Debug)]
pub struct Siblings<'a> {
    store: &'a Store,
    front: u32,
    back: u32,
    remaining: usize,
}

impl Iterator for Siblings<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        if self.remaining == 0 {
            return None;
        }
        let current = NodeId::from_link(self.front)?;
        self.remaining = self.remaining.saturating_sub(1);
        self.front = self
            .store
            .nodes
            .get(current.index())
            .map_or(NIL, |n| n.next);
        Some(current)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl DoubleEndedIterator for Siblings<'_> {
    fn next_back(&mut self) -> Option<NodeId> {
        if self.remaining == 0 {
            return None;
        }
        let current = NodeId::from_link(self.back)?;
        self.remaining = self.remaining.saturating_sub(1);
        self.back = self
            .store
            .nodes
            .get(current.index())
            .map_or(NIL, |n| n.prev);
        Some(current)
    }
}

impl ExactSizeIterator for Siblings<'_> {}

/// A flat arena of XML trees: one fixed-size record per tree node, linked by
/// index, with interned names and a leaf table.
///
/// The store is single-owner, bounded by [`limits::MAX_STORE_NODES`] slots
/// and [`limits::MAX_STORE_BYTES`] retained bytes (garbage included until
/// [`Store::compact`]). Failed operations leave it unchanged.
#[derive(Clone, Debug, Default)]
pub struct Store {
    pub(crate) nodes: Vec<Node>,
    pub(crate) leaves: Vec<Leaf>,
    pub(crate) declarations: Vec<Vec<Binding>>,
    pub(crate) names: Names,
    pub(crate) bytes: usize,
}

/// Rollback point for transactional operations that only allocate.
#[derive(Clone, Copy)]
pub(crate) struct Mark {
    nodes: usize,
    leaves: usize,
    declarations: usize,
    names: usize,
    bytes: usize,
}

impl Store {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocated slots, live and garbage.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether no slot is allocated.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Retained text, name, and value bytes, live and garbage.
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        self.bytes
    }

    pub(crate) fn mark(&self) -> Mark {
        Mark {
            nodes: self.nodes.len(),
            leaves: self.leaves.len(),
            declarations: self.declarations.len(),
            names: self.names.entries.len(),
            bytes: self.bytes,
        }
    }

    /// Drops every slot, leaf, declaration set, and name allocated after
    /// `mark`. Valid only when no node older than `mark` links a newer one.
    pub(crate) fn rollback(&mut self, mark: Mark) {
        self.nodes.truncate(mark.nodes);
        self.leaves.truncate(mark.leaves);
        self.declarations.truncate(mark.declarations);
        self.names.truncate(mark.names);
        self.bytes = mark.bytes;
    }

    /// Reserves `bytes` of the store byte bound.
    fn charge(&mut self, bytes: usize) -> Result<(), Error> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|total| !scxml_types::exceeds_wasm_bound(*total, limits::MAX_STORE_BYTES))
            .ok_or(Error::Limit(Limit::StoreBytes))?;
        Ok(())
    }

    /// Allocates one detached node. `bytes` is its retained-byte charge.
    pub(crate) fn alloc(
        &mut self,
        kind: Kind,
        name: u32,
        value: u32,
        bytes: usize,
    ) -> Result<NodeId, Error> {
        if scxml_types::exceeds_wasm_bound(
            self.nodes.len().saturating_add(1),
            limits::MAX_STORE_NODES,
        ) {
            return Err(Error::Limit(Limit::StoreNodes));
        }
        let total = self
            .bytes
            .checked_add(bytes)
            .filter(|total| !scxml_types::exceeds_wasm_bound(*total, limits::MAX_STORE_BYTES))
            .ok_or(Error::Limit(Limit::StoreBytes))?;
        let id =
            NodeId(u32::try_from(self.nodes.len()).map_err(|_| Error::Limit(Limit::StoreNodes))?);
        self.nodes.push(Node::new(kind, name, value));
        self.bytes = total;
        Ok(id)
    }

    /// A new declaration set (non-empty); its index, or [`NIL`] when empty.
    pub(crate) fn new_declarations(&mut self, bindings: Vec<Binding>) -> Result<u32, Error> {
        if bindings.is_empty() {
            return Ok(NIL);
        }
        let index = u32::try_from(self.declarations.len())
            .ok()
            .filter(|index| *index != NIL)
            .ok_or(Error::Limit(Limit::StoreNodes))?;
        self.declarations.push(bindings);
        Ok(index)
    }

    /// Allocates a leaf node (text, comment, attribute, or processing
    /// instruction) holding `text`.
    pub(crate) fn alloc_leaf(
        &mut self,
        kind: Kind,
        name: u32,
        text: String,
        bytes: usize,
    ) -> Result<NodeId, Error> {
        let leaf = u32::try_from(self.leaves.len())
            .ok()
            .filter(|index| *index != NIL)
            .ok_or(Error::Limit(Limit::StoreNodes))?;
        let id = self.alloc(kind, name, leaf, bytes)?;
        self.leaves.push(Leaf::new(text));
        Ok(id)
    }

    pub(crate) fn node(&self, id: NodeId) -> Result<&Node, Error> {
        self.nodes.get(id.index()).ok_or(Error::InvalidNode)
    }

    fn node_mut(&mut self, id: NodeId) -> Result<&mut Node, Error> {
        self.nodes.get_mut(id.index()).ok_or(Error::InvalidNode)
    }

    /// The node at a present link.
    fn linked(&self, link: u32) -> Result<&Node, Error> {
        self.nodes.get(link as usize).ok_or(Error::InvalidNode)
    }

    fn linked_mut(&mut self, link: u32) -> Result<&mut Node, Error> {
        self.nodes.get_mut(link as usize).ok_or(Error::InvalidNode)
    }

    /// The leaf of a node's value index.
    pub(crate) fn leaf(&self, node: &Node) -> Option<&Leaf> {
        match node.kind {
            Kind::Attribute | Kind::Text | Kind::Comment | Kind::Pi => {
                self.leaves.get(node.value as usize)
            }
            Kind::Root | Kind::Element => None,
        }
    }

    /// The leaf text of a node (empty when it has none).
    pub(crate) fn leaf_text(&self, node: &Node) -> &str {
        self.leaf(node).map_or("", |leaf| leaf.text.as_str())
    }

    /// The interned name of a node.
    pub(crate) fn qname(&self, node: &Node) -> Option<&QName> {
        self.names
            .entries
            .get(node.name as usize)
            .map(|entry| &entry.name)
    }

    /// The interned name entry of a node.
    pub(crate) fn name_entry(&self, node: &Node) -> Option<&NameEntry> {
        self.names.entries.get(node.name as usize)
    }

    /// Source namespace declarations of an element.
    pub(crate) fn declarations_of(&self, node: &Node) -> &[Binding] {
        if node.kind == Kind::Element {
            self.declarations
                .get(node.value as usize)
                .map_or(&[], Vec::as_slice)
        } else {
            &[]
        }
    }

    /// Creates a detached document node.
    ///
    /// # Errors
    ///
    /// [`Error::Limit`] when the store is full.
    pub fn create_root(&mut self) -> Result<NodeId, Error> {
        self.alloc(Kind::Root, NIL, NIL, 0)
    }

    /// Creates a detached element.
    ///
    /// # Errors
    ///
    /// [`Error::Limit`] when the store is full.
    pub fn create_element(&mut self, name: QName) -> Result<NodeId, Error> {
        let bytes = name.bytes();
        let mark = self.mark();
        let result = self
            .names
            .intern(name)
            .and_then(|name| self.alloc(Kind::Element, name, NIL, bytes));
        if result.is_err() {
            self.rollback(mark);
        }
        result
    }

    /// Creates a detached text node.
    ///
    /// # Errors
    ///
    /// [`Error::Limit`] when the store is full or the text exceeds
    /// [`limits::MAX_STRING_BYTES`].
    pub fn create_text(&mut self, text: &str) -> Result<NodeId, Error> {
        if text.len() > limits::MAX_STRING_BYTES {
            return Err(Error::Limit(Limit::StringBytes));
        }
        self.alloc_leaf(Kind::Text, NIL, text.to_owned(), text.len())
    }

    /// Adds attribute `name` to `element`, or replaces its value.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNode`] unless `element` is an element;
    /// [`Error::InvalidMutation`] for a namespace-declaration name;
    /// [`Error::Limit`] for too many attributes or an over-bound value.
    pub fn set_attribute(
        &mut self,
        element: NodeId,
        name: QName,
        value: &str,
    ) -> Result<(), Error> {
        if value.len() > limits::MAX_STRING_BYTES {
            return Err(Error::Limit(Limit::StringBytes));
        }
        if is_namespace_declaration(&name) {
            return Err(Error::InvalidMutation);
        }
        let owner = self.node(element)?;
        if owner.kind != Kind::Element {
            return Err(Error::InvalidNode);
        }
        let expanded = self
            .names
            .expanded(name.namespace_uri.as_deref(), &name.local_name);
        let mut existing = None;
        let mut cursor = owner.first_attribute;
        while cursor != NIL {
            let attribute = self.linked(cursor)?;
            if expanded.is_some_and(|expanded| {
                self.name_entry(attribute)
                    .is_some_and(|entry| entry.expanded == expanded)
            }) {
                existing = Some(cursor);
            }
            cursor = attribute.next;
        }
        if let Some(attribute) = existing {
            let leaf = self.linked(attribute)?.value;
            self.charge(value.len())?;
            if let Some(slot) = self.leaves.get_mut(leaf as usize) {
                *slot = Leaf::new(value.to_owned());
            }
            return Ok(());
        }
        if self.node(element)?.attribute_count as usize >= limits::MAX_ATTRIBUTES {
            return Err(Error::Limit(Limit::Attributes));
        }
        let bytes = name.bytes().saturating_add(value.len());
        let mark = self.mark();
        let result = self.names.intern(name).and_then(|name| {
            let attribute = self.alloc_leaf(Kind::Attribute, name, value.to_owned(), bytes)?;
            self.link_attribute(element, attribute)
        });
        if result.is_err() {
            self.rollback(mark);
        }
        result
    }

    /// Appends detached `attribute` to `element`'s attributes.
    pub(crate) fn link_attribute(
        &mut self,
        element: NodeId,
        attribute: NodeId,
    ) -> Result<(), Error> {
        let last = self.node(element)?.last_attribute;
        {
            let node = self.node_mut(attribute)?;
            node.parent = element.raw();
            node.prev = last;
            node.next = NIL;
        }
        if last == NIL {
            self.node_mut(element)?.first_attribute = attribute.raw();
        } else {
            self.linked_mut(last)?.next = attribute.raw();
        }
        let owner = self.node_mut(element)?;
        owner.last_attribute = attribute.raw();
        owner.attribute_count = owner.attribute_count.saturating_add(1);
        Ok(())
    }

    /// Links detached `child` into `parent`'s children before `before`
    /// ([`NIL`] appends). No validation.
    pub(crate) fn link_child(
        &mut self,
        parent: NodeId,
        before: u32,
        child: NodeId,
    ) -> Result<(), Error> {
        let prev = if before == NIL {
            self.node(parent)?.last_child
        } else {
            self.linked(before)?.prev
        };
        {
            let node = self.node_mut(child)?;
            node.parent = parent.raw();
            node.prev = prev;
            node.next = before;
        }
        if prev == NIL {
            self.node_mut(parent)?.first_child = child.raw();
        } else {
            self.linked_mut(prev)?.next = child.raw();
        }
        if before == NIL {
            self.node_mut(parent)?.last_child = child.raw();
        } else {
            self.linked_mut(before)?.prev = child.raw();
        }
        let owner = self.node_mut(parent)?;
        owner.child_count = owner.child_count.saturating_add(1);
        Ok(())
    }

    /// Node kind.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNode`] for a stale handle.
    pub fn kind(&self, node: NodeRef) -> Result<NodeKind, Error> {
        if node.namespace_parts().is_some() {
            self.namespace_binding(node)?;
            return Ok(NodeKind::Namespace);
        }
        Ok(self.node(node.owner())?.node_kind())
    }

    /// Expanded name of an element, attribute, processing instruction
    /// (target as local name), or namespace node (prefix as local name,
    /// empty for the default namespace); `None` for other kinds.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNode`] for a stale handle.
    pub fn name(&self, node: NodeRef) -> Result<Option<QName>, Error> {
        if node.namespace_parts().is_some() {
            let (prefix, _) = self.namespace_binding(node)?;
            return Ok(Some(QName {
                namespace_uri: None,
                local_name: prefix.unwrap_or_default(),
                prefix: None,
            }));
        }
        let slot = self.node(node.owner())?;
        Ok(match slot.kind {
            Kind::Element | Kind::Attribute | Kind::Pi => self.qname(slot).cloned(),
            Kind::Root | Kind::Text | Kind::Comment => None,
        })
    }

    /// Parent: the element of an attribute or namespace node, otherwise the
    /// tree parent.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNode`] for a stale handle.
    pub fn parent(&self, node: NodeRef) -> Result<Option<NodeId>, Error> {
        if node.namespace_parts().is_some() {
            self.namespace_binding(node)?;
            return Ok(Some(node.owner()));
        }
        Ok(NodeId::from_link(self.node(node.owner())?.parent))
    }

    /// Children of a document or element node in order (none for other
    /// kinds).
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNode`] for a stale handle.
    pub fn children(&self, node: NodeId) -> Result<Siblings<'_>, Error> {
        let slot = self.node(node)?;
        Ok(Siblings {
            store: self,
            front: slot.first_child,
            back: slot.last_child,
            remaining: slot.child_count as usize,
        })
    }

    /// Attributes of an element in order (none for other kinds).
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNode`] for a stale handle.
    pub fn attributes(&self, node: NodeId) -> Result<Siblings<'_>, Error> {
        let slot = self.node(node)?;
        Ok(Siblings {
            store: self,
            front: slot.first_attribute,
            back: slot.last_attribute,
            remaining: slot.attribute_count as usize,
        })
    }

    /// Value of attribute `name` on `element`, if present.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNode`] for a stale handle.
    pub fn attribute(&self, element: NodeId, name: &QName) -> Result<Option<&str>, Error> {
        let Some(expanded) = self
            .names
            .expanded(name.namespace_uri.as_deref(), &name.local_name)
        else {
            self.node(element)?;
            return Ok(None);
        };
        let mut cursor = self.node(element)?.first_attribute;
        while cursor != NIL {
            let attribute = self.linked(cursor)?;
            if self
                .name_entry(attribute)
                .is_some_and(|entry| entry.expanded == expanded)
            {
                return Ok(Some(self.leaf_text(attribute)));
            }
            cursor = attribute.next;
        }
        Ok(None)
    }

    /// Top-most ancestor-or-self of `node`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNode`] for a stale handle.
    pub fn tree_root(&self, node: NodeRef) -> Result<NodeId, Error> {
        let mut current = node.owner();
        for _ in 0..=limits::MAX_TREE_DEPTH.saturating_add(2) {
            match NodeId::from_link(self.node(current)?.parent) {
                Some(parent) => current = parent,
                None => return Ok(current),
            }
        }
        Err(Error::InvalidNode)
    }

    /// Whether `ancestor` is `node` or one of its ancestors (the element of
    /// an attribute or namespace node counts as its ancestor).
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNode`] for a stale handle.
    pub fn is_ancestor_or_self(&self, ancestor: NodeId, node: NodeRef) -> Result<bool, Error> {
        let mut current = node.owner();
        for _ in 0..=limits::MAX_TREE_DEPTH.saturating_add(2) {
            if current == ancestor {
                return Ok(true);
            }
            match NodeId::from_link(self.node(current)?.parent) {
                Some(parent) => current = parent,
                None => return Ok(false),
            }
        }
        Err(Error::InvalidNode)
    }

    /// Number of ancestors of a tree node (0 for a tree root).
    pub(crate) fn depth(&self, node: NodeId) -> Result<usize, Error> {
        let mut current = node;
        for depth in 0..=limits::MAX_TREE_DEPTH.saturating_add(2) {
            match NodeId::from_link(self.node(current)?.parent) {
                Some(parent) => current = parent,
                None => return Ok(depth),
            }
        }
        Err(Error::InvalidNode)
    }

    /// The next node after `current` in a pre-order walk of the subtree
    /// rooted at `top` (children only, attributes excluded), with its depth
    /// below `top`; `None` when the walk is done.
    pub(crate) fn next_in_subtree(
        &self,
        top: NodeId,
        current: NodeId,
        depth: usize,
    ) -> Result<Option<(NodeId, usize)>, Error> {
        let node = self.node(current)?;
        if let Some(child) = NodeId::from_link(node.first_child) {
            return Ok(Some((child, depth.saturating_add(1))));
        }
        let mut cursor = current;
        let mut level = depth;
        loop {
            if cursor == top {
                return Ok(None);
            }
            let node = self.node(cursor)?;
            if let Some(next) = NodeId::from_link(node.next) {
                return Ok(Some((next, level)));
            }
            cursor = NodeId::from_link(node.parent).ok_or(Error::InvalidNode)?;
            level = level.checked_sub(1).ok_or(Error::InvalidNode)?;
        }
    }

    /// Height of the subtree below `node` (0 for a leaf).
    fn height(&self, node: NodeId) -> Result<usize, Error> {
        let mut height = 0;
        let mut current = Some((node, 0_usize));
        while let Some((id, depth)) = current {
            height = height.max(depth);
            if depth > limits::MAX_TREE_DEPTH {
                return Err(Error::Limit(Limit::TreeDepth));
            }
            current = self.next_in_subtree(node, id, depth)?;
        }
        Ok(height)
    }

    /// XPath 1.0 string-value.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNode`] for a stale handle; [`Error::Limit`] when the
    /// value exceeds [`limits::MAX_STRING_BYTES`].
    pub fn string_value(&self, node: NodeRef) -> Result<String, Error> {
        let mut fuel = u64::MAX;
        self.string_value_with(node, &mut fuel).map(Cow::into_owned)
    }

    /// XPath 1.0 string-value, charging fuel for every node visited and
    /// every byte read; borrowed whenever it is one stored text.
    pub(crate) fn string_value_with(
        &self,
        node: NodeRef,
        fuel: &mut u64,
    ) -> Result<Cow<'_, str>, Error> {
        if node.namespace_parts().is_some() {
            let uri = self.namespace_binding(node)?.1;
            burn_bytes(fuel, uri.len())?;
            return Ok(Cow::Owned(uri));
        }
        let top = node.owner();
        let slot = self.node(top)?;
        match slot.kind {
            Kind::Attribute | Kind::Text | Kind::Comment | Kind::Pi => {
                let text = self.leaf_text(slot);
                burn_bytes(fuel, text.len())?;
                Ok(Cow::Borrowed(text))
            }
            Kind::Root | Kind::Element => {
                let mut out: Cow<'_, str> = Cow::Borrowed("");
                let mut current = Some((top, 0_usize));
                while let Some((id, depth)) = current {
                    burn(fuel)?;
                    let slot = self.node(id)?;
                    if slot.kind == Kind::Text {
                        let text = self.leaf_text(slot);
                        burn_bytes(fuel, text.len())?;
                        if out.is_empty() {
                            out = Cow::Borrowed(text);
                        } else {
                            push_bounded(out.to_mut(), text)?;
                        }
                    }
                    current = self.next_in_subtree(top, id, depth)?;
                }
                Ok(out)
            }
        }
    }

    /// XPath 1.0 `number()` of a node's string-value, with the fuel of
    /// [`Self::string_value_with`]; a single stored text answers from its
    /// kept number without reparsing.
    pub(crate) fn number_value_with(&self, node: NodeRef, fuel: &mut u64) -> Result<f64, Error> {
        if node.namespace_parts().is_some() {
            return Ok(string_to_number(&self.string_value_with(node, fuel)?));
        }
        let top = node.owner();
        let slot = self.node(top)?;
        match slot.kind {
            Kind::Attribute | Kind::Text | Kind::Comment | Kind::Pi => {
                let leaf = self.leaf(slot);
                burn_bytes(fuel, leaf.map_or(0, |leaf| leaf.text.len()))?;
                Ok(leaf.map_or(f64::NAN, Leaf::number))
            }
            Kind::Root | Kind::Element => {
                let mut single: Option<&Leaf> = None;
                let mut texts = 0_usize;
                let mut out = String::new();
                let mut current = Some((top, 0_usize));
                while let Some((id, depth)) = current {
                    burn(fuel)?;
                    let slot = self.node(id)?;
                    if slot.kind == Kind::Text {
                        let leaf = self.leaf(slot).ok_or(Error::InvalidNode)?;
                        burn_bytes(fuel, leaf.text.len())?;
                        if leaf.text.is_empty() {
                            // Contributes nothing, exactly as concatenation.
                        } else if texts == 0 {
                            single = Some(leaf);
                            texts = 1;
                        } else {
                            if texts == 1
                                && let Some(first) = single.take()
                            {
                                push_bounded(&mut out, &first.text)?;
                            }
                            texts = texts.saturating_add(1);
                            push_bounded(&mut out, &leaf.text)?;
                        }
                    }
                    current = self.next_in_subtree(top, id, depth)?;
                }
                Ok(match (texts, single) {
                    (0, _) => string_to_number(""),
                    (1, Some(leaf)) => leaf.number(),
                    _ => string_to_number(&out),
                })
            }
        }
    }

    /// In-scope namespaces of an element, sorted by prefix (default first),
    /// always including `xml`.
    pub(crate) fn in_scope_namespaces(&self, element: NodeId) -> Result<Vec<Binding>, Error> {
        let mut chain = Vec::new();
        let mut current = Some(element);
        while let Some(id) = current {
            if chain.len() > limits::MAX_TREE_DEPTH.saturating_add(2) {
                return Err(Error::InvalidNode);
            }
            chain.push(id);
            current = NodeId::from_link(self.node(id)?.parent);
        }
        let mut scope: BTreeMap<Option<String>, String> = BTreeMap::new();
        scope.insert(Some("xml".into()), XML_NAMESPACE.into());
        for id in chain.iter().rev() {
            let slot = self.node(*id)?;
            if slot.kind != Kind::Element {
                continue;
            }
            for (prefix, uri) in self.declarations_of(slot) {
                scope.insert(prefix.clone(), uri.clone());
            }
            if let Some(name) = self.qname(slot) {
                scope.insert(
                    name.prefix.clone(),
                    name.namespace_uri.clone().unwrap_or_default(),
                );
            }
            let mut cursor = slot.first_attribute;
            while cursor != NIL {
                let attribute = self.linked(cursor)?;
                if let Some(name) = self.qname(attribute)
                    && let (Some(prefix), Some(uri)) = (&name.prefix, &name.namespace_uri)
                {
                    scope.insert(Some(prefix.clone()), uri.clone());
                }
                cursor = attribute.next;
            }
        }
        scope.insert(Some("xml".into()), XML_NAMESPACE.into());
        Ok(scope
            .into_iter()
            .filter(|(_, uri)| !uri.is_empty())
            .collect())
    }

    pub(crate) fn namespace_binding(&self, node: NodeRef) -> Result<Binding, Error> {
        let (element, index) = node.namespace_parts().ok_or(Error::InvalidNode)?;
        if self.node(element)?.kind != Kind::Element {
            return Err(Error::InvalidNode);
        }
        self.in_scope_namespaces(element)?
            .into_iter()
            .nth(index)
            .ok_or(Error::InvalidNode)
    }

    /// Deep-copies `node` into a new detached tree and returns its root.
    ///
    /// A document node copies to a document node; an attribute copies to a
    /// detached attribute.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNode`] for a stale handle or a namespace node;
    /// [`Error::Limit`] when the store would overflow. The store is
    /// unchanged on failure.
    pub fn deep_copy(&mut self, node: NodeRef) -> Result<NodeId, Error> {
        let id = node.id().ok_or(Error::InvalidNode)?;
        let mark = self.mark();
        let result = self.copy_subtree(id);
        if result.is_err() {
            self.rollback(mark);
        }
        result
    }

    fn copy_subtree(&mut self, source: NodeId) -> Result<NodeId, Error> {
        let copy = self.copy_node(source)?;
        let mut stack = vec![(source, copy)];
        while let Some((from, to)) = stack.pop() {
            let mut child = self.node(from)?.first_child;
            while let Some(original) = NodeId::from_link(child) {
                let copied = self.copy_node(original)?;
                self.link_child(to, NIL, copied)?;
                stack.push((original, copied));
                child = self.node(original)?.next;
            }
        }
        Ok(copy)
    }

    /// Copies one node's own data and attributes (not its children).
    fn copy_node(&mut self, source: NodeId) -> Result<NodeId, Error> {
        let slot = *self.node(source)?;
        let copy = self.copy_own(&slot)?;
        let mut cursor = slot.first_attribute;
        while cursor != NIL {
            let attribute = *self.linked(cursor)?;
            let copied = self.copy_own(&attribute)?;
            self.link_attribute(copy, copied)?;
            cursor = attribute.next;
        }
        Ok(copy)
    }

    /// Copies a node's kind, name, and value (no links) within this store.
    fn copy_own(&mut self, slot: &Node) -> Result<NodeId, Error> {
        let bytes = self.node_bytes(slot);
        match slot.kind {
            Kind::Root => self.alloc(Kind::Root, NIL, NIL, bytes),
            Kind::Element => {
                let declarations = self.declarations_of(slot).to_vec();
                let mark = self.mark();
                let declarations = self.new_declarations(declarations)?;
                let result = self.alloc(Kind::Element, slot.name, declarations, bytes);
                if result.is_err() {
                    self.rollback(mark);
                }
                result
            }
            Kind::Attribute | Kind::Text | Kind::Comment | Kind::Pi => {
                let leaf = self.leaf(slot).cloned().ok_or(Error::InvalidNode)?;
                let index = u32::try_from(self.leaves.len())
                    .ok()
                    .filter(|index| *index != NIL)
                    .ok_or(Error::Limit(Limit::StoreNodes))?;
                let id = self.alloc(slot.kind, slot.name, index, bytes)?;
                self.leaves.push(leaf);
                Ok(id)
            }
        }
    }

    /// Retained-byte charge of a copy of `slot`.
    fn node_bytes(&self, slot: &Node) -> usize {
        let name = self.qname(slot).map_or(0, QName::bytes);
        match slot.kind {
            Kind::Root => 0,
            Kind::Element => name,
            Kind::Attribute | Kind::Pi => name.saturating_add(self.leaf_text(slot).len()),
            Kind::Text | Kind::Comment => self.leaf_text(slot).len(),
        }
    }

    /// Validates that `child` may become a child of `parent`.
    fn check_insert(&self, parent: NodeId, child: NodeId) -> Result<(), Error> {
        if !matches!(self.node(parent)?.kind, Kind::Root | Kind::Element) {
            return Err(Error::InvalidMutation);
        }
        let slot = self.node(child)?;
        if slot.has_parent() || matches!(slot.kind, Kind::Root | Kind::Attribute) {
            return Err(Error::InvalidMutation);
        }
        if self.tree_root(NodeRef::node(parent))? == child {
            return Err(Error::InvalidMutation);
        }
        let depth = self
            .depth(parent)?
            .saturating_add(1)
            .saturating_add(self.height(child)?);
        if depth >= limits::MAX_TREE_DEPTH {
            return Err(Error::Limit(Limit::TreeDepth));
        }
        Ok(())
    }

    /// Validates and links `child` into `parent` before `before` ([`NIL`]
    /// appends).
    fn insert_checked(&mut self, parent: NodeId, before: u32, child: NodeId) -> Result<(), Error> {
        self.check_insert(parent, child)?;
        self.link_child(parent, before, child)
    }

    /// Appends detached `child` to `parent`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidMutation`] when `parent` cannot have children, `child`
    /// is attached, a document or attribute node, or an ancestor of
    /// `parent`; [`Error::Limit`] for tree depth.
    pub fn append_child(&mut self, parent: NodeId, child: NodeId) -> Result<(), Error> {
        self.node(parent)?;
        self.insert_checked(parent, NIL, child)
    }

    /// Inserts detached `child` before the first child of `parent`.
    ///
    /// # Errors
    ///
    /// As [`Store::append_child`].
    pub fn prepend_child(&mut self, parent: NodeId, child: NodeId) -> Result<(), Error> {
        let first = self.node(parent)?.first_child;
        self.insert_checked(parent, first, child)
    }

    /// The parent of a non-attribute sibling-mode reference node.
    fn sibling_parent(&self, reference: NodeId) -> Result<NodeId, Error> {
        let slot = self.node(reference)?;
        if matches!(slot.kind, Kind::Attribute | Kind::Root) {
            return Err(Error::InvalidMutation);
        }
        NodeId::from_link(slot.parent).ok_or(Error::InvalidMutation)
    }

    /// Inserts detached `node` immediately before `reference`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidMutation`] when `reference` has no parent or is an
    /// attribute; otherwise as [`Store::append_child`].
    pub fn insert_before(&mut self, reference: NodeId, node: NodeId) -> Result<(), Error> {
        let parent = self.sibling_parent(reference)?;
        self.insert_checked(parent, reference.raw(), node)
    }

    /// Inserts detached `node` immediately after `reference`.
    ///
    /// # Errors
    ///
    /// As [`Store::insert_before`].
    pub fn insert_after(&mut self, reference: NodeId, node: NodeId) -> Result<(), Error> {
        let parent = self.sibling_parent(reference)?;
        let next = self.node(reference)?.next;
        self.insert_checked(parent, next, node)
    }

    /// Detaches `node` (an attribute leaves its element). Detaching a
    /// parentless node is a no-op.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNode`] for a stale handle.
    pub fn detach(&mut self, node: NodeId) -> Result<(), Error> {
        let slot = *self.node(node)?;
        let Some(parent) = NodeId::from_link(slot.parent) else {
            return Ok(());
        };
        let is_attribute = slot.kind == Kind::Attribute;
        if slot.prev == NIL {
            let owner = self.node_mut(parent)?;
            if is_attribute {
                owner.first_attribute = slot.next;
            } else {
                owner.first_child = slot.next;
            }
        } else {
            self.linked_mut(slot.prev)?.next = slot.next;
        }
        if slot.next == NIL {
            let owner = self.node_mut(parent)?;
            if is_attribute {
                owner.last_attribute = slot.prev;
            } else {
                owner.last_child = slot.prev;
            }
        } else {
            self.linked_mut(slot.next)?.prev = slot.prev;
        }
        let owner = self.node_mut(parent)?;
        if is_attribute {
            owner.attribute_count = owner.attribute_count.saturating_sub(1);
        } else {
            owner.child_count = owner.child_count.saturating_sub(1);
        }
        let detached = self.node_mut(node)?;
        detached.parent = NIL;
        detached.prev = NIL;
        detached.next = NIL;
        Ok(())
    }

    /// Detaches every child of `parent`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNode`] for a stale handle.
    pub fn remove_children(&mut self, parent: NodeId) -> Result<(), Error> {
        let mut cursor = self.node(parent)?.first_child;
        while cursor != NIL {
            let child = self.linked_mut(cursor)?;
            let next = child.next;
            child.parent = NIL;
            child.prev = NIL;
            child.next = NIL;
            cursor = next;
        }
        let owner = self.node_mut(parent)?;
        owner.first_child = NIL;
        owner.last_child = NIL;
        owner.child_count = 0;
        Ok(())
    }

    /// Applies one SCXML XPath `<assign>` to `target`.
    ///
    /// Node values are deep-copied first and never aliased: a copied document
    /// node contributes its children, and attribute or namespace nodes become
    /// text nodes of their string-value. Text values become one text node
    /// (none when empty). [`AssignMode::AddAttribute`] uses the string-value
    /// of the first value node (or the text). Child modes require a document
    /// or element target; sibling modes and [`AssignMode::Replace`] require a
    /// non-attribute target with a parent; [`AssignMode::Delete`] requires a
    /// parent (attributes allowed).
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNode`] for a stale or wrong-kind target,
    /// [`Error::InvalidMutation`] for a placement the tree cannot express,
    /// [`Error::Limit`] for store or depth bounds. The store is unchanged on
    /// failure.
    pub fn assign(
        &mut self,
        target: NodeId,
        mode: AssignMode,
        value: AssignValue<'_>,
    ) -> Result<(), Error> {
        let mark = self.mark();
        let result = self.assign_inner(target, mode, value);
        if result.is_err() {
            self.rollback(mark);
        }
        result
    }

    fn assign_inner(
        &mut self,
        target: NodeId,
        mode: AssignMode,
        value: AssignValue<'_>,
    ) -> Result<(), Error> {
        let target_kind = self.kind(NodeRef::node(target))?;
        match mode {
            AssignMode::Delete => {
                if !self.node(target)?.has_parent() {
                    return Err(Error::InvalidMutation);
                }
                return self.detach(target);
            }
            AssignMode::AddAttribute(name) => {
                if target_kind != NodeKind::Element {
                    return Err(Error::InvalidNode);
                }
                let text = match value {
                    AssignValue::Nodes(nodes) => match nodes.first() {
                        Some(node) => self.string_value(*node)?,
                        None => String::new(),
                    },
                    AssignValue::Text(text) => text.to_owned(),
                    AssignValue::None => String::new(),
                };
                return self.set_attribute(target, name, &text);
            }
            AssignMode::ReplaceChildren | AssignMode::FirstChild | AssignMode::LastChild => {
                if !matches!(target_kind, NodeKind::Root | NodeKind::Element) {
                    return Err(Error::InvalidNode);
                }
            }
            AssignMode::PreviousSibling | AssignMode::NextSibling | AssignMode::Replace => {
                self.sibling_parent(target)?;
            }
        }
        let copies = self.value_copies(value)?;
        let parent_for_check = match mode {
            AssignMode::ReplaceChildren | AssignMode::FirstChild | AssignMode::LastChild => target,
            _ => self.sibling_parent(target)?,
        };
        for copy in &copies {
            // Validate every placement before linking anything; the copies
            // are fresh detached trees, so only kind and depth can fail.
            self.check_insert(parent_for_check, *copy)?;
        }
        match mode {
            AssignMode::ReplaceChildren => {
                self.remove_children(target)?;
                for copy in copies {
                    self.append_child(target, copy)?;
                }
            }
            AssignMode::FirstChild => {
                let first = self.node(target)?.first_child;
                for copy in copies {
                    self.insert_checked(target, first, copy)?;
                }
            }
            AssignMode::LastChild => {
                for copy in copies {
                    self.append_child(target, copy)?;
                }
            }
            AssignMode::PreviousSibling => {
                for copy in copies {
                    self.insert_before(target, copy)?;
                }
            }
            AssignMode::NextSibling => {
                let mut reference = target;
                for copy in copies {
                    self.insert_after(reference, copy)?;
                    reference = copy;
                }
            }
            AssignMode::Replace => {
                for copy in copies {
                    self.insert_before(target, copy)?;
                }
                self.detach(target)?;
            }
            AssignMode::Delete | AssignMode::AddAttribute(_) => {}
        }
        Ok(())
    }

    /// Detached child-ready copies of an assign value.
    fn value_copies(&mut self, value: AssignValue<'_>) -> Result<Vec<NodeId>, Error> {
        let mut copies = Vec::new();
        match value {
            AssignValue::None => {}
            AssignValue::Text(text) => {
                if !text.is_empty() {
                    copies.push(self.create_text(text)?);
                }
            }
            AssignValue::Nodes(nodes) => {
                for node in nodes {
                    match self.kind(*node)? {
                        NodeKind::Root => {
                            let mut child = self.node(node.owner())?.first_child;
                            while let Some(original) = NodeId::from_link(child) {
                                copies.push(self.copy_subtree(original)?);
                                child = self.node(original)?.next;
                            }
                        }
                        NodeKind::Attribute | NodeKind::Namespace => {
                            let text = self.string_value(*node)?;
                            if !text.is_empty() {
                                copies.push(self.create_text(&text)?);
                            }
                        }
                        NodeKind::Element
                        | NodeKind::Text
                        | NodeKind::Comment
                        | NodeKind::ProcessingInstruction => {
                            copies.push(self.copy_subtree(node.owner())?);
                        }
                    }
                }
            }
        }
        Ok(copies)
    }

    /// Retains only the trees containing `keep`, reclaiming every other slot,
    /// and rewrites each `keep` handle to its new identity. Every handle not
    /// in `keep` (including values holding node-sets) is invalid afterwards.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNode`] for a stale handle; the store is unchanged on
    /// failure.
    pub fn compact(&mut self, keep: &mut [NodeId]) -> Result<(), Error> {
        let mut roots = Vec::new();
        for node in keep.iter() {
            let root = self.tree_root(NodeRef::node(*node))?;
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        roots.sort_unstable();
        let mut compacted = Self::new();
        let mut map: Vec<u32> = vec![NIL; self.nodes.len()];
        for root in roots {
            self.copy_tree_into(root, &mut compacted, &mut map)?;
        }
        for node in keep.iter_mut() {
            *node = map
                .get(node.index())
                .copied()
                .and_then(NodeId::from_link)
                .ok_or(Error::InvalidNode)?;
        }
        *self = compacted;
        Ok(())
    }

    /// Copies one node's kind, name, and value into `target` (no links).
    fn transfer_own(&self, slot: &Node, target: &mut Self) -> Result<NodeId, Error> {
        let bytes = self.node_bytes(slot);
        let name = match self.qname(slot) {
            Some(name) => target.names.intern(name.clone())?,
            None => NIL,
        };
        match slot.kind {
            Kind::Root => target.alloc(Kind::Root, NIL, NIL, bytes),
            Kind::Element => {
                let declarations = target.new_declarations(self.declarations_of(slot).to_vec())?;
                target.alloc(Kind::Element, name, declarations, bytes)
            }
            Kind::Attribute | Kind::Text | Kind::Comment | Kind::Pi => {
                let leaf = self.leaf(slot).cloned().ok_or(Error::InvalidNode)?;
                let index = u32::try_from(target.leaves.len())
                    .ok()
                    .filter(|index| *index != NIL)
                    .ok_or(Error::Limit(Limit::StoreNodes))?;
                let id = target.alloc(slot.kind, name, index, bytes)?;
                target.leaves.push(leaf);
                Ok(id)
            }
        }
    }

    fn copy_tree_into(
        &self,
        root: NodeId,
        target: &mut Self,
        map: &mut [u32],
    ) -> Result<(), Error> {
        let mut stack = vec![(root, None::<NodeId>)];
        while let Some((source, parent)) = stack.pop() {
            let slot = *self.node(source)?;
            let copy = self.transfer_own(&slot, target)?;
            if let Some(entry) = map.get_mut(source.index()) {
                *entry = copy.raw();
            }
            let mut cursor = slot.first_attribute;
            while cursor != NIL {
                let attribute = *self.linked(cursor)?;
                let copied = self.transfer_own(&attribute, target)?;
                target.link_attribute(copy, copied)?;
                if let Some(entry) = map.get_mut(cursor as usize) {
                    *entry = copied.raw();
                }
                cursor = attribute.next;
            }
            if let Some(parent) = parent {
                target.link_child(parent, NIL, copy)?;
            }
            let mut child = slot.last_child;
            while let Some(id) = NodeId::from_link(child) {
                stack.push((id, Some(copy)));
                child = self.node(id)?.prev;
            }
        }
        Ok(())
    }

    /// Sorts `nodes` into document order and removes duplicates. Trees are
    /// ordered by root identity (implementation-defined but stable).
    pub(crate) fn sort_document_order(
        &self,
        nodes: &mut Vec<NodeRef>,
        fuel: &mut u64,
    ) -> Result<(), Error> {
        if nodes.len() <= 1 {
            return Ok(());
        }
        if nodes.len() > SMALL_SORT {
            return self.sort_by_traversal(nodes, fuel);
        }
        let mut keyed = Vec::with_capacity(nodes.len());
        for node in nodes.iter() {
            keyed.push((self.order_key(*node, fuel)?, *node));
        }
        keyed.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        keyed.dedup_by(|left, right| left.0 == right.0);
        nodes.clear();
        nodes.extend(keyed.into_iter().map(|(_, node)| node));
        Ok(())
    }

    /// Ranks `nodes` by one document-order walk of every tree they belong
    /// to: linear in tree size, independent of fan-out.
    fn sort_by_traversal(&self, nodes: &mut Vec<NodeRef>, fuel: &mut u64) -> Result<(), Error> {
        let wanted: BTreeSet<NodeRef> = nodes.iter().copied().collect();
        let mut namespace_wanted: BTreeMap<NodeId, Vec<usize>> = BTreeMap::new();
        let mut roots = Vec::new();
        for node in &wanted {
            if let Some((element, index)) = node.namespace_parts() {
                namespace_wanted.entry(element).or_default().push(index);
            }
            let root = self.tree_root(*node)?;
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        for indices in namespace_wanted.values_mut() {
            indices.sort_unstable();
        }
        roots.sort_unstable();
        let mut ranks: BTreeMap<NodeRef, usize> = BTreeMap::new();
        let mut next = 0_usize;
        let mut rank = |node: NodeRef, ranks: &mut BTreeMap<NodeRef, usize>| {
            if wanted.contains(&node) {
                ranks.insert(node, next);
            }
            next = next.saturating_add(1);
        };
        for root in roots {
            let mut current = Some((root, 0_usize));
            while let Some((id, depth)) = current {
                burn(fuel)?;
                rank(NodeRef::node(id), &mut ranks);
                if let Some(indices) = namespace_wanted.get(&id) {
                    for index in indices {
                        rank(NodeRef::namespace_of(id, *index)?, &mut ranks);
                    }
                }
                let mut cursor = self.node(id)?.first_attribute;
                while let Some(attribute) = NodeId::from_link(cursor) {
                    rank(NodeRef::node(attribute), &mut ranks);
                    cursor = self.node(attribute)?.next;
                }
                current = self.next_in_subtree(root, id, depth)?;
            }
        }
        let mut ranked = Vec::with_capacity(ranks.len());
        for node in wanted {
            ranked.push((*ranks.get(&node).ok_or(Error::InvalidNode)?, node));
        }
        ranked.sort_unstable_by_key(|(rank, _)| *rank);
        nodes.clear();
        nodes.extend(ranked.into_iter().map(|(_, node)| node));
        Ok(())
    }

    /// Zero-based position of `node` in its parent's child or attribute
    /// list.
    fn position_in_parent(&self, node: NodeId) -> Result<u32, Error> {
        let mut index = 0_u32;
        let mut cursor = self.node(node)?.prev;
        while cursor != NIL {
            index = index.checked_add(1).ok_or(Error::InvalidNode)?;
            cursor = self.linked(cursor)?.prev;
        }
        Ok(index)
    }

    /// Document-order key: tree root id, then per level `2 + child index`;
    /// an element's namespace nodes append `[0, i]` and attributes `[1, i]`.
    fn order_key(&self, node: NodeRef, fuel: &mut u64) -> Result<Vec<u32>, Error> {
        let mut reversed: Vec<u32> = Vec::new();
        let mut current = node.owner();
        if let Some((_, index)) = node.namespace_parts() {
            reversed.push(u32::try_from(index).map_err(|_| Error::InvalidNode)?);
            reversed.push(0);
        } else if self.node(current)?.kind == Kind::Attribute {
            let parent = NodeId::from_link(self.node(current)?.parent).ok_or(Error::InvalidNode)?;
            reversed.push(self.position_in_parent(current)?);
            reversed.push(1);
            current = parent;
        }
        for _ in 0..=limits::MAX_TREE_DEPTH.saturating_add(2) {
            burn(fuel)?;
            let Some(parent) = NodeId::from_link(self.node(current)?.parent) else {
                reversed.push(current.0);
                reversed.reverse();
                return Ok(reversed);
            };
            let fanout = u64::from(self.node(parent)?.child_count);
            *fuel = fuel.checked_sub(fanout).ok_or(Error::Limit(Limit::Fuel))?;
            reversed.push(
                self.position_in_parent(current)?
                    .checked_add(2)
                    .ok_or(Error::InvalidNode)?,
            );
            current = parent;
        }
        Err(Error::InvalidNode)
    }

    /// Serializes `node` as canonical XML appended to `out`.
    ///
    /// Elements emit exactly the namespace declarations needed for their
    /// names and attributes plus their own source declarations; text is
    /// escaped (`&`, `<`, `>`), attribute values additionally escape `"`,
    /// tab, CR, and LF. A document node serializes its children; an
    /// attribute or namespace node serializes as escaped text of its value.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNode`] for a stale handle; [`Error::Limit`] when `out`
    /// would exceed [`limits::MAX_STRING_BYTES`].
    pub fn serialize(&self, node: NodeRef, out: &mut String) -> Result<(), Error> {
        match self.kind(node)? {
            NodeKind::Attribute | NodeKind::Namespace => {
                let value = self.string_value(node)?;
                escape_text(out, &value)
            }
            NodeKind::Root => self.serialize_children(node.owner(), out),
            _ => {
                let mut scope = NamespaceScope::new();
                self.serialize_tree(node.owner(), out, &mut scope)
            }
        }
    }

    /// Serializes the children of `node` in order.
    ///
    /// # Errors
    ///
    /// As [`Store::serialize`].
    pub fn serialize_children(&self, node: NodeId, out: &mut String) -> Result<(), Error> {
        for child in self.children(node)? {
            let mut scope = NamespaceScope::new();
            self.serialize_tree(child, out, &mut scope)?;
        }
        Ok(())
    }

    /// Appends `prefix:local` or `local`.
    fn push_lexical(out: &mut String, name: &QName) -> Result<(), Error> {
        if let Some(prefix) = &name.prefix {
            push_bounded(out, prefix)?;
            push_bounded(out, ":")?;
        }
        push_bounded(out, &name.local_name)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one iterative walk keeps element, namespace, attribute, and content emission together"
    )]
    fn serialize_tree(
        &self,
        node: NodeId,
        out: &mut String,
        scope: &mut NamespaceScope,
    ) -> Result<(), Error> {
        enum Work {
            Open(NodeId),
            Close(NodeId),
        }
        let mut stack = vec![Work::Open(node)];
        while let Some(work) = stack.pop() {
            let id = match work {
                Work::Close(element) => {
                    let name = self.qname(self.node(element)?).ok_or(Error::InvalidNode)?;
                    push_bounded(out, "</")?;
                    Self::push_lexical(out, name)?;
                    push_bounded(out, ">")?;
                    scope.pop();
                    continue;
                }
                Work::Open(id) => id,
            };
            let slot = self.node(id)?;
            match slot.kind {
                Kind::Text => escape_text(out, self.leaf_text(slot))?,
                Kind::Comment => {
                    push_bounded(out, "<!--")?;
                    push_bounded(out, self.leaf_text(slot))?;
                    push_bounded(out, "-->")?;
                }
                Kind::Pi => {
                    let target = self.qname(slot).ok_or(Error::InvalidNode)?;
                    let data = self.leaf_text(slot);
                    push_bounded(out, "<?")?;
                    push_bounded(out, &target.local_name)?;
                    if !data.is_empty() {
                        push_bounded(out, " ")?;
                        push_bounded(out, data)?;
                    }
                    push_bounded(out, "?>")?;
                }
                Kind::Root => {
                    let mut child = slot.last_child;
                    while let Some(child_id) = NodeId::from_link(child) {
                        stack.push(Work::Open(child_id));
                        child = self.node(child_id)?.prev;
                    }
                }
                Kind::Attribute => return Err(Error::InvalidNode),
                Kind::Element => {
                    let name = self.qname(slot).ok_or(Error::InvalidNode)?;
                    scope.push();
                    let mut emitted: Vec<Binding> = Vec::new();
                    let element_prefix = name.prefix.as_deref();
                    let element_uri = name.namespace_uri.as_deref().unwrap_or_default();
                    if scope.lookup(element_prefix) != Some(element_uri) {
                        let prefix = element_prefix.map(str::to_owned);
                        scope.bind(prefix.clone(), element_uri.to_owned());
                        emitted.push((prefix, element_uri.to_owned()));
                    }
                    let mut rendered_attributes: Vec<(Option<String>, &QName, &str)> =
                        Vec::with_capacity(slot.attribute_count as usize);
                    let mut cursor = slot.first_attribute;
                    while cursor != NIL {
                        let attribute = self.linked(cursor)?;
                        if attribute.kind != Kind::Attribute {
                            return Err(Error::InvalidNode);
                        }
                        let attribute_name = self.qname(attribute).ok_or(Error::InvalidNode)?;
                        let prefix = match attribute_name.namespace_uri.as_deref() {
                            None => None,
                            Some(XML_NAMESPACE) => Some("xml".to_owned()),
                            Some(uri) => {
                                let prefix = scope
                                    .prefix_for_attribute(attribute_name.prefix.as_deref(), uri);
                                if scope.lookup(Some(&prefix)) != Some(uri) {
                                    scope.bind(Some(prefix.clone()), uri.to_owned());
                                    emitted.push((Some(prefix.clone()), uri.to_owned()));
                                }
                                Some(prefix)
                            }
                        };
                        rendered_attributes.push((
                            prefix,
                            attribute_name,
                            self.leaf_text(attribute),
                        ));
                        cursor = attribute.next;
                    }
                    for (prefix, uri) in self.declarations_of(slot) {
                        if prefix.as_deref() == Some("xml") {
                            continue;
                        }
                        let bound_here = emitted.iter().any(|(bound, _)| bound == prefix);
                        if !bound_here && scope.lookup(prefix.as_deref()) != Some(uri.as_str()) {
                            scope.bind(prefix.clone(), uri.clone());
                            emitted.push((prefix.clone(), uri.clone()));
                        }
                    }
                    push_bounded(out, "<")?;
                    Self::push_lexical(out, name)?;
                    for (prefix, uri) in &emitted {
                        match prefix {
                            Some(prefix) => {
                                push_bounded(out, " xmlns:")?;
                                push_bounded(out, prefix)?;
                            }
                            None => push_bounded(out, " xmlns")?,
                        }
                        push_bounded(out, "=\"")?;
                        escape_attribute(out, uri)?;
                        push_bounded(out, "\"")?;
                    }
                    for (prefix, attribute_name, value) in rendered_attributes {
                        push_bounded(out, " ")?;
                        if let Some(prefix) = prefix {
                            push_bounded(out, &prefix)?;
                            push_bounded(out, ":")?;
                        }
                        push_bounded(out, &attribute_name.local_name)?;
                        push_bounded(out, "=\"")?;
                        escape_attribute(out, value)?;
                        push_bounded(out, "\"")?;
                    }
                    if slot.first_child == NIL {
                        push_bounded(out, "/>")?;
                        scope.pop();
                    } else {
                        push_bounded(out, ">")?;
                        stack.push(Work::Close(id));
                        let mut child = slot.last_child;
                        while let Some(child_id) = NodeId::from_link(child) {
                            stack.push(Work::Open(child_id));
                            child = self.node(child_id)?.prev;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Serializer namespace scope: a stack of per-element binding frames.
struct NamespaceScope {
    frames: Vec<Vec<Binding>>,
    generated: usize,
}

impl NamespaceScope {
    fn new() -> Self {
        Self {
            frames: Vec::new(),
            generated: 0,
        }
    }

    fn push(&mut self) {
        self.frames.push(Vec::new());
    }

    fn pop(&mut self) {
        self.frames.pop();
    }

    fn bind(&mut self, prefix: Option<String>, uri: String) {
        if let Some(frame) = self.frames.last_mut() {
            frame.push((prefix, uri));
        }
    }

    fn lookup(&self, prefix: Option<&str>) -> Option<&str> {
        if prefix == Some("xml") {
            return Some(XML_NAMESPACE);
        }
        for frame in self.frames.iter().rev() {
            for (bound, uri) in frame.iter().rev() {
                if bound.as_deref() == prefix {
                    return Some(uri);
                }
            }
        }
        // The default namespace is initially empty; other prefixes unbound.
        prefix.is_none().then_some("")
    }

    /// A prefix usable for a namespaced attribute: its own when free or
    /// already bound to `uri`, else a generated `nsN` not bound in scope.
    fn prefix_for_attribute(&mut self, preferred: Option<&str>, uri: &str) -> String {
        if let Some(prefix) = preferred
            && prefix != "xmlns"
            && prefix != "xml"
        {
            let bound = self.lookup(Some(prefix));
            if bound.is_none() || bound == Some(uri) {
                return prefix.to_owned();
            }
        }
        loop {
            let candidate = format!("ns{}", self.generated);
            self.generated = self.generated.saturating_add(1);
            if self.lookup(Some(&candidate)).is_none() {
                return candidate;
            }
        }
    }
}

fn is_namespace_declaration(name: &QName) -> bool {
    (name.namespace_uri.is_none() && name.local_name == "xmlns")
        || name.prefix.as_deref() == Some("xmlns")
        || name.namespace_uri.as_deref() == Some(XMLNS_NAMESPACE)
}

/// Consumes one unit of evaluation fuel.
pub(crate) fn burn(fuel: &mut u64) -> Result<(), Error> {
    burn_units(fuel, 1)
}

/// Consumes `units` of evaluation fuel, or none when fewer remain.
pub(crate) fn burn_units(fuel: &mut u64, units: usize) -> Result<(), Error> {
    let units = u64::try_from(units).map_err(|_| Error::Limit(Limit::Fuel))?;
    *fuel = fuel.checked_sub(units).ok_or(Error::Limit(Limit::Fuel))?;
    Ok(())
}

/// Consumes the fuel for `bytes` of string work: one unit per started
/// [`limits::STRING_BYTES_PER_FUEL`] bytes (see [`limits::DEFAULT_FUEL`]).
pub(crate) fn burn_bytes(fuel: &mut u64, bytes: usize) -> Result<(), Error> {
    burn_units(fuel, bytes.div_ceil(limits::STRING_BYTES_PER_FUEL))
}

/// Appends `text` to `out` within [`limits::MAX_STRING_BYTES`].
pub(crate) fn push_bounded(out: &mut String, text: &str) -> Result<(), Error> {
    if out.len().saturating_add(text.len()) > limits::MAX_STRING_BYTES {
        return Err(Error::Limit(Limit::StringBytes));
    }
    out.push_str(text);
    Ok(())
}

fn escape_text(out: &mut String, text: &str) -> Result<(), Error> {
    let mut start = 0;
    for (index, character) in text.char_indices() {
        let replacement = match character {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '\r' => "&#13;",
            _ => continue,
        };
        push_bounded(out, text.get(start..index).unwrap_or_default())?;
        push_bounded(out, replacement)?;
        start = index.saturating_add(character.len_utf8());
    }
    push_bounded(out, text.get(start..).unwrap_or_default())
}

fn escape_attribute(out: &mut String, text: &str) -> Result<(), Error> {
    let mut start = 0;
    for (index, character) in text.char_indices() {
        let replacement = match character {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '"' => "&quot;",
            '\t' => "&#9;",
            '\n' => "&#10;",
            '\r' => "&#13;",
            _ => continue,
        };
        push_bounded(out, text.get(start..index).unwrap_or_default())?;
        push_bounded(out, replacement)?;
        start = index.saturating_add(character.len_utf8());
    }
    push_bounded(out, text.get(start..).unwrap_or_default())
}
