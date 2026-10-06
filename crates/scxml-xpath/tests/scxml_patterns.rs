//! Executable examples of the SCXML XPath data-model patterns used by the
//! official W3C IRP XPath corpus, built only on the public crate API.
//!
//! The data model shape mirrors the contract notes: the context node is a
//! `<datamodel>` element whose children are the system elements and the
//! `<data id="...">` elements; `$name` binds each to its element.

#![allow(
    clippy::doc_markdown,
    reason = "XPath, SCXML, and XML are specification names, not code identifiers"
)]

mod common;

use common::TestEnvironment;
use scxml_xpath::{
    AssignMode, AssignValue, Error, Expression, NodeId, NodeRef, NodeSet, ParseOptions, QName,
    Store, Value, limits,
};

struct Model {
    store: Store,
    datamodel: NodeId,
    env: TestEnvironment,
}

impl Model {
    fn new(declarations: &str) -> Self {
        let mut store = Store::new();
        let root = store.create_root().expect("root allocates");
        let datamodel = store
            .create_element(QName::local("datamodel").expect("valid"))
            .expect("allocates");
        store.append_child(root, datamodel).expect("attaches");
        let system = "<_sessionid>s-1</_sessionid><_name>machineName</_name>\
            <_ioprocessors><processor name=\"http://www.w3.org/TR/scxml/#SCXMLEventProcessor\">\
            <location>#_scxml_s-1</location></processor></_ioprocessors>";
        for node in store
            .parse_fragment(&format!("{system}{declarations}"))
            .expect("declarations parse")
        {
            store.append_child(datamodel, node).expect("attaches");
        }
        let mut model = Self {
            store,
            datamodel,
            env: TestEnvironment::default(),
        };
        model.rebind();
        model
    }

    /// Binds `$id` for every `<data id>` and `$_name` for system elements.
    fn rebind(&mut self) {
        self.env.variables.clear();
        for child in self
            .store
            .children(self.datamodel)
            .expect("reads")
            .collect::<Vec<_>>()
        {
            let name = self
                .store
                .name(NodeRef::node(child))
                .expect("reads")
                .expect("elements are named");
            let variable = match self
                .store
                .attribute(child, &QName::local("id").expect("valid"))
                .expect("reads")
            {
                Some(id) if name.local_name == "data" => id.to_owned(),
                _ => name.local_name,
            };
            let set =
                NodeSet::from_nodes(&self.store, vec![NodeRef::node(child)]).expect("valid node");
            self.env.variables.insert(variable, Value::NodeSet(set));
        }
    }

    fn eval(&mut self, source: &str) -> Result<Value, Error> {
        Expression::parse(source, &ParseOptions::scxml())?.evaluate(
            &self.store,
            NodeRef::node(self.datamodel),
            &mut self.env,
            limits::DEFAULT_FUEL,
        )
    }

    fn cond(&mut self, source: &str) -> bool {
        self.eval(source).expect("condition evaluates").to_boolean()
    }

    /// `<assign location expr [type attr]>`: every location node receives a
    /// copy of the value (node-set) or its string (scalar).
    fn assign(&mut self, location: &str, expr: &str, mode: &AssignMode) -> Result<(), Error> {
        let Value::NodeSet(targets) = self.eval(location)? else {
            return Err(Error::Type);
        };
        let value = self.eval(expr)?;
        for target in targets.nodes() {
            let target = target.id().ok_or(Error::InvalidNode)?;
            match &value {
                Value::NodeSet(nodes) => {
                    self.store
                        .assign(target, mode.clone(), AssignValue::Nodes(nodes.nodes()))?;
                }
                scalar => {
                    let text = scalar.to_string(&self.store)?;
                    self.store
                        .assign(target, mode.clone(), AssignValue::Text(&text))?;
                }
            }
        }
        Ok(())
    }
}

#[test]
fn data_values_arithmetic_and_comparisons() {
    let mut model = Model::new(
        "<data id=\"Var1\">1</data><data id=\"Var2\"/>\
         <data id=\"Var3\"><node>1</node><node>2</node><node>3</node></data>",
    );
    assert!(model.cond("$Var1/text() =1"));
    assert!(model.cond("$Var1 + 1 = 2"));
    assert!(model.cond("$Var2 and not($Var2/text()) and not($Var2/*)"));
    assert!(model.cond("not($Var2/text() or $Var2/*)"));
    assert!(model.cond("$Var3/* or $Var3/text()"));
    assert!(model.cond("count($Var3/*) = 3 and sum($Var3/*) = 6"));
    assert!(model.cond("$_name/text() = 'machineName'"));
    assert!(model.cond(
        "$_ioprocessors/processor[@name=\"http://www.w3.org/TR/scxml/#SCXMLEventProcessor\"]/location/text() = '#_scxml_s-1'"
    ));
    assert!(model.cond("contains($_ioprocessors//location, $_sessionid)"));
    assert!(model.cond("true()"));
    assert!(!model.cond("0>1"));
    assert!(!model.cond("'foo' = $Var1"));
    assert!(model.cond("'foo'"));
    assert_eq!(
        model
            .eval("$Var3/*[2]")
            .and_then(|v| v.to_string(&model.store)),
        Ok("2".into())
    );
    assert_eq!(
        model
            .eval("concat($Var1, $Var3/*[3])")
            .and_then(|v| v.to_string(&model.store)),
        Ok("13".into())
    );
}

#[test]
fn assignments_copy_values_and_respect_types() {
    let mut model = Model::new(
        "<data id=\"Var1\">0</data><data id=\"Var2\"/>\
         <data id=\"Var3\"><node>1</node><node>2</node><node>3</node></data>",
    );
    model
        .assign("$Var1", "$Var1 + 1", &AssignMode::ReplaceChildren)
        .expect("assigns");
    assert!(model.cond("$Var1/text() = 1"));
    model
        .assign("$Var2", "$_sessionid", &AssignMode::ReplaceChildren)
        .expect("assigns");
    assert!(model.cond("$Var2 = $_sessionid"));
    // `<assign type="lastchild" location="$Var3" expr="$Var3/*"/>` doubles the list
    model
        .assign("$Var3", "$Var3/*", &AssignMode::LastChild)
        .expect("assigns");
    assert!(model.cond("count($Var3/*) = 6"));
    model
        .assign("$Var3/*[1]", "'first'", &AssignMode::PreviousSibling)
        .expect("assigns");
    assert!(model.cond("$Var3/text() = 'first'"));
    model
        .assign(
            "$Var3",
            "'x'",
            &AssignMode::AddAttribute(QName::local("mark").expect("valid")),
        )
        .expect("assigns");
    assert!(model.cond("$Var3/@mark = 'x'"));
    model
        .assign("$Var3/*[position() > 3]", "''", &AssignMode::Delete)
        .expect("deletes");
    assert!(model.cond("count($Var3/*) = 3"));
    // an invalid expression or location is a typed error (error.execution)
    assert!(matches!(
        model.assign("$Var1", "!1", &AssignMode::ReplaceChildren),
        Err(Error::Syntax(_))
    ));
    assert_eq!(
        model.assign("$Var5", "1", &AssignMode::ReplaceChildren),
        Err(Error::UndefinedVariable)
    );
    assert_eq!(
        model.assign("1", "1", &AssignMode::ReplaceChildren),
        Err(Error::Type)
    );
}

#[test]
fn event_payload_shapes_used_by_the_corpus() {
    let mut model = Model::new("<data id=\"Var1\">1</data>");
    let event = "<_event><name>event1</name><type>external</type><sendid/><origin>#_scxml_s-1</origin>\
        <origintype>http://www.w3.org/TR/scxml/#SCXMLEventProcessor</origintype><invokeid/>\
        <data><data id=\"Var1\"><data id=\"Var1\">1</data></data><data id=\"param1\">2</data></data><raw/></_event>";
    for node in model.store.parse_fragment(event).expect("event parses") {
        model
            .store
            .append_child(model.datamodel, node)
            .expect("attaches");
    }
    model.rebind();
    assert!(model.cond("$_event/name and $_event/type and $_event/sendid and $_event/origin and $_event/origintype and $_event/invokeid and $_event/data"));
    assert!(model.cond("$_event/data/data[@id = 'Var1']/data/text() = 1"));
    assert!(model.cond("$_event/data/data[@id='param1']/text() = 2"));
    assert!(
        model.cond("_event/data/data[@id='param1'] = 2"),
        "relative _event path from the context node"
    );
    assert!(model.cond("not(string($_event/sendid))"));
    assert!(model.cond("$_event/type/text() = 'external'"));
    assert!(
        model.cond("$_event/origintype/text() = 'http://www.w3.org/TR/scxml/#SCXMLEventProcessor'")
    );
    // system elements are distinguishable for read-only enforcement
    let Value::NodeSet(target) = model.eval("$_event").expect("evaluates") else {
        panic!("expected node-set");
    };
    let target = target.first().expect("bound");
    let sessionid = match model.eval("$_sessionid").expect("evaluates") {
        Value::NodeSet(nodes) => nodes.first().and_then(NodeRef::id).expect("bound"),
        other => panic!("expected node-set, got {other:?}"),
    };
    assert!(
        model
            .store
            .is_ancestor_or_self(sessionid, NodeRef::node(sessionid))
            .expect("reads")
    );
    assert!(
        !model
            .store
            .is_ancestor_or_self(sessionid, target)
            .expect("reads")
    );
}

#[test]
fn foreach_arrays_and_in_predicate() {
    let mut model = Model::new(
        "<data id=\"Var3\"><node>1</node><node>2</node><node>3</node></data><data id=\"Var4\"/>",
    );
    let Value::NodeSet(items) = model.eval("$Var3/*").expect("evaluates") else {
        panic!("expected node-set");
    };
    let mut total = 0.0;
    for item in items.nodes() {
        total += Value::NodeSet(NodeSet::from_nodes(&model.store, vec![*item]).expect("valid"))
            .to_number(&model.store)
            .expect("numeric");
    }
    assert!((total - 6.0).abs() < f64::EPSILON);
    assert_eq!(
        model.eval("$Var4/text()"),
        Ok(Value::NodeSet(NodeSet::new()))
    );
    model.env.active = vec!["s01".into()];
    assert!(model.cond("In('s01')"));
    assert!(!model.cond("In('s1')"));
    assert!(matches!(model.eval("In()"), Err(Error::Syntax(_))));
}
