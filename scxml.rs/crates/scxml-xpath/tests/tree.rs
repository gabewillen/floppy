//! XML data tree: parsing, serialization, copying, SCXML assign mutations,
//! compaction, and bounds.

#![allow(
    clippy::doc_markdown,
    reason = "XPath and XML are specification names, not code identifiers"
)]

mod common;

use std::fmt::Write as _;

use common::{TestEnvironment, document, eval_in};
use scxml_xpath::{
    AssignMode, AssignValue, Error, Limit, NodeId, NodeKind, NodeRef, QName, Store, Value,
    XmlError, limits,
};

fn serialize(store: &Store, node: NodeRef) -> String {
    let mut out = String::new();
    store
        .serialize(node, &mut out)
        .expect("serialization must succeed");
    out
}

fn parse_one(store: &mut Store, xml: &str) -> NodeId {
    let nodes = store.parse_fragment(xml).expect("fixture must parse");
    assert_eq!(nodes.len(), 1, "fixture must have one top-level node");
    nodes[0]
}

fn select(store: &Store, context: NodeRef, source: &str) -> Vec<NodeRef> {
    let mut env = TestEnvironment::default();
    match eval_in(store, context, &mut env, source) {
        Ok(Value::NodeSet(nodes)) => nodes.nodes().to_vec(),
        other => panic!("{source} must select nodes, got {other:?}"),
    }
}

#[test]
fn parse_and_serialize_round_trip_canonically() {
    let mut store = Store::new();
    let cases = [
        ("<a/>", "<a/>"),
        ("<a  b = 'x' >t</a >", "<a b=\"x\">t</a>"),
        (
            "<a>&lt;&amp;&gt;&quot;&apos;&#65;&#x42;</a>",
            "<a>&lt;&amp;&gt;\"'AB</a>",
        ),
        ("<a b=\"&quot;&#10;\t\"/>", "<a b=\"&quot;&#10; \"/>"),
        ("<a><![CDATA[<x>&]]></a>", "<a>&lt;x&gt;&amp;</a>"),
        ("<a><!-- c --><?t d?></a>", "<a><!-- c --><?t d?></a>"),
        (
            "<p:a xmlns:p=\"urn:p\" p:b=\"1\"/>",
            "<p:a xmlns:p=\"urn:p\" p:b=\"1\"/>",
        ),
        (
            "<a xmlns=\"urn:d\"><b xmlns=\"\"><c/></b></a>",
            "<a xmlns=\"urn:d\"><b xmlns=\"\"><c/></b></a>",
        ),
        ("<a xmlns=\"urn:d\"><b/></a>", "<a xmlns=\"urn:d\"><b/></a>"),
        ("<a xml:lang=\"en\"/>", "<a xml:lang=\"en\"/>"),
        ("<a>x\r\ny\rz</a>", "<a>x\ny\nz</a>"),
        ("<?xml version=\"1.0\"?><a/>", "<a/>"),
    ];
    for (input, expected) in cases {
        let node = parse_one(&mut store, input);
        assert_eq!(serialize(&store, NodeRef::node(node)), expected, "{input}");
        let reparsed = parse_one(&mut store, expected);
        assert_eq!(
            serialize(&store, NodeRef::node(reparsed)),
            expected,
            "reparse of {expected}"
        );
    }
}

#[test]
fn fragments_may_hold_several_top_level_nodes() {
    let mut store = Store::new();
    let nodes = store
        .parse_fragment("  <a/>text<b/><!--c-->")
        .expect("fragment must parse");
    assert_eq!(nodes.len(), 5);
    assert_eq!(store.kind(NodeRef::node(nodes[0])), Ok(NodeKind::Text));
    assert_eq!(store.kind(NodeRef::node(nodes[1])), Ok(NodeKind::Element));
    assert_eq!(store.kind(NodeRef::node(nodes[4])), Ok(NodeKind::Comment));
    assert!(
        store
            .parse_fragment("")
            .expect("empty fragment parses")
            .is_empty()
    );
    assert_eq!(
        store
            .parse_fragment("plain text")
            .expect("text parses")
            .len(),
        1
    );
}

#[test]
fn malformed_or_hostile_xml_is_rejected_without_state_change() {
    let mut store = Store::new();
    let before = store.len();
    let cases: &[(&str, Error)] = &[
        ("<!DOCTYPE a><a/>", Error::Xml(XmlError::Doctype)),
        ("<a>&ent;</a>", Error::Xml(XmlError::Entity)),
        ("<a>&#0;</a>", Error::Xml(XmlError::Entity)),
        ("<a>&#xD800;</a>", Error::Xml(XmlError::Entity)),
        ("<a>&amp</a>", Error::Xml(XmlError::Malformed)),
        ("<p:a/>", Error::Xml(XmlError::UndeclaredPrefix)),
        ("<a p:b='1'/>", Error::Xml(XmlError::UndeclaredPrefix)),
        ("<a><b></a></b>", Error::Xml(XmlError::Malformed)),
        ("<a>", Error::Xml(XmlError::Malformed)),
        ("</a>", Error::Xml(XmlError::Malformed)),
        ("<a b='1' b='2'/>", Error::Xml(XmlError::Malformed)),
        (
            "<a xmlns:p='urn:x' xmlns:q='urn:x' p:b='1' q:b='2'/>",
            Error::Xml(XmlError::Malformed),
        ),
        ("<a b='<'/>", Error::Xml(XmlError::Malformed)),
        ("<a b=1/>", Error::Xml(XmlError::Malformed)),
        ("<a\u{1}/>", Error::Xml(XmlError::Malformed)),
        ("<a>\u{1}</a>", Error::Xml(XmlError::Malformed)),
        ("<a>]]></a>", Error::Xml(XmlError::Malformed)),
        ("<!-- a -- b -->", Error::Xml(XmlError::Malformed)),
        ("<a xmlns:p=''/>", Error::Xml(XmlError::Malformed)),
        ("<a xmlns:xml='urn:x'/>", Error::Xml(XmlError::Malformed)),
        ("<xmlns:a/>", Error::Xml(XmlError::Malformed)),
        ("<a:b:c xmlns:a='urn:a'/>", Error::Xml(XmlError::Malformed)),
        ("<1a/>", Error::Xml(XmlError::Malformed)),
        ("<a b='1'c='2'/>", Error::Xml(XmlError::Malformed)),
        ("<![CDATA[x", Error::Xml(XmlError::Malformed)),
    ];
    for (input, expected) in cases {
        assert_eq!(store.parse_fragment(input), Err(*expected), "{input:?}");
        assert_eq!(store.len(), before, "{input:?} must not leave nodes behind");
        assert_eq!(store.retained_bytes(), 0);
    }
}

#[test]
fn xml_bounds_fail_closed() {
    let mut store = Store::new();
    let deep = format!(
        "{}{}",
        "<a>".repeat(limits::MAX_TREE_DEPTH),
        "</a>".repeat(limits::MAX_TREE_DEPTH)
    );
    assert_eq!(
        store.parse_fragment(&deep),
        Err(Error::Limit(Limit::TreeDepth))
    );
    let shallow = format!(
        "{}{}",
        "<a>".repeat(limits::MAX_TREE_DEPTH - 2),
        "</a>".repeat(limits::MAX_TREE_DEPTH - 2)
    );
    assert!(store.parse_fragment(&shallow).is_ok());
    let mut attributes = String::new();
    for index in 0..=limits::MAX_ATTRIBUTES {
        write!(attributes, " a{index}='1'").expect("writing to a String cannot fail");
    }
    assert_eq!(
        store.parse_fragment(&format!("<e{attributes}/>")),
        Err(Error::Limit(Limit::Attributes))
    );
    let huge = "x".repeat(limits::MAX_XML_BYTES + 1);
    assert_eq!(
        store.parse_fragment(&huge),
        Err(Error::Limit(Limit::XmlBytes))
    );
    let long_name = "n".repeat(limits::MAX_NAME_BYTES + 1);
    assert!(store.parse_fragment(&format!("<{long_name}/>")).is_err());
}

#[test]
fn namespace_serialization_generates_needed_declarations() {
    let mut store = Store::new();
    let element = store
        .create_element(QName::new(Some("urn:e"), "e", Some("p")).expect("valid name"))
        .expect("element allocates");
    store
        .set_attribute(
            element,
            QName::new(Some("urn:a"), "x", None).expect("valid name"),
            "1",
        )
        .expect("attribute sets");
    store
        .set_attribute(
            element,
            QName::new(Some("urn:e"), "y", Some("p")).expect("valid name"),
            "2",
        )
        .expect("attribute sets");
    let child = store
        .create_element(QName::local("c").expect("valid name"))
        .expect("child allocates");
    store.append_child(element, child).expect("child attaches");
    let xml = serialize(&store, NodeRef::node(element));
    assert_eq!(
        xml,
        "<p:e xmlns:p=\"urn:e\" xmlns:ns0=\"urn:a\" ns0:x=\"1\" p:y=\"2\"><c/></p:e>"
    );
    let reparsed = parse_one(&mut store, &xml);
    assert_eq!(serialize(&store, NodeRef::node(reparsed)), xml);
    // default-namespace parent with a null-namespace child
    let mut store = Store::new();
    let parent = store
        .create_element(QName::new(Some("urn:d"), "a", None).expect("valid"))
        .expect("allocates");
    let child = store
        .create_element(QName::local("b").expect("valid"))
        .expect("allocates");
    store.append_child(parent, child).expect("attaches");
    assert_eq!(
        serialize(&store, NodeRef::node(parent)),
        "<a xmlns=\"urn:d\"><b xmlns=\"\"/></a>"
    );
    assert!(QName::new(None, "a", Some("p")).is_err());
    assert!(QName::new(Some(""), "a", None).is_err());
    assert!(QName::local("a:b").is_err());
    assert_eq!(
        QName::new(Some("urn:e"), "e", Some("p"))
            .expect("valid")
            .lexical(),
        "p:e"
    );
}

#[test]
fn deep_copies_are_independent() {
    let mut store = Store::new();
    let original = parse_one(&mut store, "<a x='1'><b>t</b></a>");
    let copy = store
        .deep_copy(NodeRef::node(original))
        .expect("copy succeeds");
    assert_ne!(copy, original);
    assert_eq!(
        serialize(&store, NodeRef::node(copy)),
        "<a x=\"1\"><b>t</b></a>"
    );
    store
        .set_attribute(original, QName::local("x").expect("valid"), "2")
        .expect("sets");
    store.remove_children(original).expect("clears");
    assert_eq!(
        serialize(&store, NodeRef::node(copy)),
        "<a x=\"1\"><b>t</b></a>"
    );
    assert_eq!(serialize(&store, NodeRef::node(original)), "<a x=\"2\"/>");
    let root = store.create_root().expect("root allocates");
    let text = store.create_text("r").expect("text allocates");
    store.append_child(root, text).expect("attaches");
    let root_copy = store.deep_copy(NodeRef::node(root)).expect("root copies");
    assert_eq!(store.kind(NodeRef::node(root_copy)), Ok(NodeKind::Root));
    assert_eq!(store.string_value(NodeRef::node(root_copy)), Ok("r".into()));
}

#[test]
fn structural_mutations_reject_impossible_trees() {
    let mut store = Store::new();
    let a = parse_one(&mut store, "<a><b/></a>");
    let b = store
        .children(a)
        .expect("children read")
        .next()
        .expect("a has a child");
    let root = store.create_root().expect("root allocates");
    let loose = store
        .create_element(QName::local("loose").expect("valid"))
        .expect("allocates");
    // attached child, root child, cycle, attribute parent
    assert_eq!(store.append_child(loose, b), Err(Error::InvalidMutation));
    assert_eq!(store.append_child(a, root), Err(Error::InvalidMutation));
    assert_eq!(store.append_child(b, a), Err(Error::InvalidMutation));
    assert_eq!(store.append_child(a, a), Err(Error::InvalidMutation));
    store
        .set_attribute(a, QName::local("x").expect("valid"), "1")
        .expect("sets");
    let attribute = store
        .attributes(a)
        .expect("reads")
        .next()
        .expect("a has an attribute");
    assert_eq!(
        store.append_child(attribute, loose),
        Err(Error::InvalidMutation)
    );
    assert_eq!(
        store.append_child(loose, attribute),
        Err(Error::InvalidMutation)
    );
    assert_eq!(
        store.insert_before(a, loose),
        Err(Error::InvalidMutation),
        "parentless reference"
    );
    assert_eq!(
        store.insert_after(attribute, loose),
        Err(Error::InvalidMutation)
    );
    assert_eq!(
        store.set_attribute(a, QName::local("xmlns").expect("valid"), "urn:x"),
        Err(Error::InvalidMutation)
    );
    // valid placements
    store.insert_before(b, loose).expect("inserts before");
    let after = store.create_text("t").expect("allocates");
    store.insert_after(b, after).expect("inserts after");
    let first = store
        .create_element(QName::local("f").expect("valid"))
        .expect("allocates");
    store.prepend_child(a, first).expect("prepends");
    assert_eq!(
        serialize(&store, NodeRef::node(a)),
        "<a x=\"1\"><f/><loose/><b/>t</a>"
    );
    store.detach(attribute).expect("attribute detaches");
    store.detach(first).expect("child detaches");
    store.detach(first).expect("detaching twice is a no-op");
    assert_eq!(serialize(&store, NodeRef::node(a)), "<a><loose/><b/>t</a>");
    assert_eq!(store.parent(NodeRef::node(b)), Ok(Some(a)));
    assert_eq!(store.tree_root(NodeRef::node(b)), Ok(a));
    assert_eq!(store.is_ancestor_or_self(a, NodeRef::node(b)), Ok(true));
    assert_eq!(store.is_ancestor_or_self(b, NodeRef::node(a)), Ok(false));
}

#[test]
fn mutation_depth_is_bounded() {
    let mut store = Store::new();
    let depth = limits::MAX_TREE_DEPTH / 2 + 1;
    let chain = format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth));
    let first = parse_one(&mut store, &chain);
    let second = parse_one(&mut store, &chain);
    let mut deepest = first;
    while let Some(child) = store.children(deepest).expect("reads").next() {
        deepest = child;
    }
    assert_eq!(
        store.append_child(deepest, second),
        Err(Error::Limit(Limit::TreeDepth))
    );
    assert_eq!(
        store.assign(
            deepest,
            AssignMode::LastChild,
            AssignValue::Nodes(&[NodeRef::node(second)])
        ),
        Err(Error::Limit(Limit::TreeDepth))
    );
}

/// `<data id="v">` targets with a sibling-bearing parent, for assign modes.
fn assign_fixture() -> (Store, NodeRef, NodeId, NodeId) {
    let mut store = Store::new();
    let context = document(
        &mut store,
        "<dm><data id='v'><old/>x</data><src a='A'><n>1</n><n>2</n></src></dm>",
    );
    let target = select(&store, context, "/dm/data")[0]
        .id()
        .expect("tree node");
    let source = select(&store, context, "/dm/src")[0]
        .id()
        .expect("tree node");
    (store, context, target, source)
}

#[test]
fn assign_modes_follow_the_scxml_xpath_data_model() {
    let expectations = [
        (
            AssignMode::ReplaceChildren,
            "<dm><data id=\"v\"><n>1</n><n>2</n></data>",
        ),
        (
            AssignMode::FirstChild,
            "<dm><data id=\"v\"><n>1</n><n>2</n><old/>x</data>",
        ),
        (
            AssignMode::LastChild,
            "<dm><data id=\"v\"><old/>x<n>1</n><n>2</n></data>",
        ),
        (
            AssignMode::PreviousSibling,
            "<dm><n>1</n><n>2</n><data id=\"v\"><old/>x</data>",
        ),
        (
            AssignMode::NextSibling,
            "<dm><data id=\"v\"><old/>x</data><n>1</n><n>2</n>",
        ),
        (AssignMode::Replace, "<dm><n>1</n><n>2</n><src"),
        (AssignMode::Delete, "<dm><src"),
        (
            AssignMode::AddAttribute(QName::local("k").expect("valid")),
            "<dm><data id=\"v\" k=\"1\"><old/>x</data>",
        ),
    ];
    for (mode, prefix) in expectations {
        let (mut store, context, target, _) = assign_fixture();
        let value = select(&store, context, "/dm/src/n");
        store
            .assign(target, mode.clone(), AssignValue::Nodes(&value))
            .expect("assign succeeds");
        let xml = serialize(&store, context);
        assert!(xml.starts_with(prefix), "{mode:?}: {xml}");
        // the source is never aliased
        assert!(
            xml.ends_with("<src a=\"A\"><n>1</n><n>2</n></src></dm>"),
            "{mode:?}: {xml}"
        );
    }
}

#[test]
fn assign_values_are_copied_converted_and_transactional() {
    let (mut store, context, target, source) = assign_fixture();
    // text values
    store
        .assign(target, AssignMode::ReplaceChildren, AssignValue::Text("7"))
        .expect("text assigns");
    assert_eq!(store.string_value(NodeRef::node(target)), Ok("7".into()));
    store
        .assign(target, AssignMode::ReplaceChildren, AssignValue::Text(""))
        .expect("empty text assigns");
    assert_eq!(store.children(target).expect("reads").len(), 0);
    // attribute values become text; later source mutation does not leak
    let attribute = select(&store, context, "/dm/src/@a");
    store
        .assign(
            target,
            AssignMode::ReplaceChildren,
            AssignValue::Nodes(&attribute),
        )
        .expect("attribute assigns");
    assert_eq!(
        serialize(&store, NodeRef::node(target)),
        "<data id=\"v\">A</data>"
    );
    store.remove_children(source).expect("clears source");
    assert_eq!(
        serialize(&store, NodeRef::node(target)),
        "<data id=\"v\">A</data>"
    );
    // a document node contributes its children
    let root = store.create_root().expect("root allocates");
    let child = store
        .create_element(QName::local("r").expect("valid"))
        .expect("allocates");
    store.append_child(root, child).expect("attaches");
    store
        .assign(
            target,
            AssignMode::LastChild,
            AssignValue::Nodes(&[NodeRef::node(root)]),
        )
        .expect("root assigns");
    assert_eq!(
        serialize(&store, NodeRef::node(target)),
        "<data id=\"v\">A<r/></data>"
    );
    // addattribute with text and with no value
    store
        .assign(
            target,
            AssignMode::AddAttribute(QName::local("t").expect("valid")),
            AssignValue::Text("z"),
        )
        .expect("attribute adds");
    assert_eq!(
        store.attribute(target, &QName::local("t").expect("valid")),
        Ok(Some("z"))
    );
    // deleting an attribute node
    let attribute = select(&store, context, "/dm/data/@t")[0]
        .id()
        .expect("tree node");
    store
        .assign(attribute, AssignMode::Delete, AssignValue::None)
        .expect("attribute deletes");
    assert_eq!(
        store.attribute(target, &QName::local("t").expect("valid")),
        Ok(None)
    );
    // invalid targets leave the store unchanged
    let before = (store.len(), serialize(&store, context));
    let text = select(&store, context, "/dm/data/text()")[0]
        .id()
        .expect("tree node");
    for (target, mode) in [
        (text, AssignMode::ReplaceChildren),
        (
            text,
            AssignMode::AddAttribute(QName::local("a").expect("valid")),
        ),
        (context.id().expect("root"), AssignMode::Replace),
        (context.id().expect("root"), AssignMode::Delete),
        (context.id().expect("root"), AssignMode::NextSibling),
    ] {
        let value = select(&store, context, "/dm/data");
        assert!(
            store
                .assign(target, mode.clone(), AssignValue::Nodes(&value))
                .is_err(),
            "{mode:?}"
        );
        assert_eq!(
            (store.len(), serialize(&store, context)),
            before,
            "{mode:?} must roll back"
        );
    }
    // assigning an ancestor into its own descendant copies, never cycles
    let dm = select(&store, context, "/dm");
    store
        .assign(target, AssignMode::LastChild, AssignValue::Nodes(&dm))
        .expect("self-copy assigns");
    assert_eq!(select(&store, context, "/dm/data/dm").len(), 1);
}

#[test]
fn compaction_keeps_live_trees_and_rewrites_handles() {
    let mut store = Store::new();
    let keep = parse_one(&mut store, "<keep><x>1</x></keep>");
    for _ in 0..50 {
        parse_one(&mut store, "<garbage><y/></garbage>");
    }
    let inner = store
        .children(keep)
        .expect("reads")
        .next()
        .expect("keep has a child");
    let expected = serialize(&store, NodeRef::node(keep));
    let before = store.len();
    let mut handles = [inner, keep];
    store.compact(&mut handles).expect("compaction succeeds");
    assert!(store.len() < before);
    assert_eq!(store.len(), 3);
    assert_eq!(serialize(&store, NodeRef::node(handles[1])), expected);
    assert_eq!(
        store.parent(NodeRef::node(handles[0])),
        Ok(Some(handles[1]))
    );
}

/// Fills a store with 16 MiB plus one string and returns the first failure.
fn fill_past_sixteen_mib() -> Option<Error> {
    let mut store = Store::new();
    let text = "y".repeat(limits::MAX_STRING_BYTES);
    (0..=(16 * 1_048_576 / limits::MAX_STRING_BYTES)).find_map(|_| store.create_text(&text).err())
}

/// The store's byte bound (16 MiB) applies to wasm builds only: the
/// seventeenth MiB fails there.
#[cfg(target_arch = "wasm32")]
#[test]
fn wasm_only_the_store_byte_bound_fails_closed() {
    assert_eq!(
        fill_past_sixteen_mib(),
        Some(Error::Limit(Limit::StoreBytes))
    );
}

#[test]
fn store_capacity_is_bounded() {
    // Natively the store's byte bound does not apply: the seventeenth MiB is
    // retained.
    #[cfg(not(target_arch = "wasm32"))]
    assert_eq!(fill_past_sixteen_mib(), None);
    assert_eq!(
        Store::new().create_text(&"z".repeat(limits::MAX_STRING_BYTES + 1)),
        Err(Error::Limit(Limit::StringBytes))
    );
}

#[test]
fn stale_handles_are_typed_errors() {
    let mut big = Store::new();
    let far = parse_one(&mut big, "<a><b/><c/><d/></a>");
    let small = Store::new();
    let far = NodeRef::node(store_last_child(&big, far));
    assert_eq!(small.kind(far), Err(Error::InvalidNode));
    assert_eq!(small.string_value(far), Err(Error::InvalidNode));
    assert_eq!(small.tree_root(far), Err(Error::InvalidNode));
}

fn store_last_child(store: &Store, node: NodeId) -> NodeId {
    store
        .children(node)
        .expect("reads")
        .next_back()
        .expect("fixture has children")
}
