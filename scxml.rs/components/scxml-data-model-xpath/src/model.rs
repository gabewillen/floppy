//! Single-owner `XPath` data-model state and its public operations.

use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasherDefault, Hasher};

use scxml_telemetry::{Module, Operation, observe};
use scxml_types::{
    AssignPlacement, AssignSource, DataInitializer, DataValue, Event, EventType, FiniteNumber,
};
use scxml_xpath::{
    AssignMode, AssignValue, Environment, Expression, NodeId, NodeKind, NodeRef, NodeSet,
    ParseOptions, QName, Store, Value, is_ncname, limits, number_to_string,
};

use crate::payload::{Escape, MAX_JSON_DEPTH, escape, scalar_text, space_normalized};
use crate::{
    DataModelError, DataModelSnapshot, EVALUATION_FUEL, ExecutionHost, MAX_CACHED_EXPRESSIONS,
    MAX_EXPRESSION_BYTES, MAX_SCRIPT_BYTES, MAX_SOURCE_BYTES, MAX_VALUE_BYTES, MAX_VARIABLES,
    SystemContext, Variable, VariableMarkup, finish,
};

/// FNV-1a hasher for the variable table: deterministic (no seed, so no
/// WASI randomness import) and cheap for short names.
#[derive(Clone, Copy, Debug)]
struct NameHasher(u64);

impl Default for NameHasher {
    fn default() -> Self {
        Self(0xCBF2_9CE4_8422_2325)
    }
}

impl Hasher for NameHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01B3);
        }
    }
}

/// Variable name to its binding.
type Variables = HashMap<String, Binding, BuildHasherDefault<NameHasher>>;

/// One variable: its element under `<datamodel>` (a declared `<data id>`
/// element, CR 2014-03-13 C.3.1), and the collection node a `<foreach>`
/// last assigned to it, which `$name` denotes while set (REC 4.6: the
/// processor "MUST assign it to the item variable").
#[derive(Clone, Copy, Debug)]
struct Binding {
    element: NodeId,
    item: Option<NodeId>,
}

impl Binding {
    /// The node `$name` denotes.
    fn node(self) -> NodeId {
        self.item.unwrap_or(self.element)
    }
}

/// Most `<foreach>` loops tracked at once; starting another forgets the
/// oldest (a loop an error left unfinished) (wasm builds only; unbounded
/// natively, see [`scxml_types::wasm_bound`]).
const MAX_FOREACH_FRAMES: usize = scxml_types::wasm_bound(32);

/// One `<foreach>` in progress (REC 4.6): the nodes of its shallow copy (a
/// namespace node, which has no tree identity, is `None`), the values
/// [`XPathDataModel::evaluate_items`] returned for them, the variable its
/// items are bound to (the first bound after it), and how many are bound.
#[derive(Clone, Debug)]
struct ForeachFrame {
    nodes: Vec<Option<NodeId>>,
    images: Vec<DataValue>,
    item: Option<String>,
    next: usize,
}

/// System variable names; user data may not declare them (SCXML 5.10).
pub(crate) const RESERVED_NAMES: [&str; 4] = ["_sessionid", "_name", "_ioprocessors", "_event"];

/// Store occupancy (slots or bytes, as a fraction of the evaluator bound)
/// above which garbage from rebinding and assignment is compacted away.
const COMPACT_NUMERATOR: usize = 1;
const COMPACT_DENOMINATOR: usize = 2;
/// Store size compaction is measured against: the wasm store bounds, on
/// every build (compaction is memory hygiene, not a bound, so a native
/// store with no bound still reclaims garbage at the same point).
const COMPACTION_STORE_NODES: usize = 262_144;
const COMPACTION_STORE_BYTES: usize = 16 * 1_048_576;

/// Single-owner deterministic `XPath` data-model state.
///
/// Every mutating operation validates before it commits: a failure leaves
/// committed state (variables, their values, and system context) unchanged.
/// The parse cache and the bound `_event` tree are derived state only.
#[derive(Clone, Debug, Default)]
pub struct XPathDataModel {
    state: Option<State>,
    /// Parsed expressions keyed by source; cleared when full.
    cache: ExpressionCache,
}

/// Parsed expressions keyed by source text, bucketed by a 64-bit FNV-1a
/// digest of the source so a lookup hashes the source once, compares
/// integers, and compares source text only against its own bucket. Holds
/// at most [`MAX_CACHED_EXPRESSIONS`] expressions and is cleared when full.
#[derive(Clone, Debug, Default)]
struct ExpressionCache {
    buckets: BTreeMap<u64, Vec<(String, Expression)>>,
    len: usize,
}

impl ExpressionCache {
    /// FNV-1a over the source bytes (deterministic, no hasher seed).
    fn digest(source: &str) -> u64 {
        source.bytes().fold(0xCBF2_9CE4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01B3)
        })
    }

    /// The parsed `source`, parsing and caching it on a miss.
    fn parsed(&mut self, source: &str) -> Result<&Expression, DataModelError> {
        let digest = Self::digest(source);
        let cached = self
            .buckets
            .get(&digest)
            .and_then(|bucket| bucket.iter().position(|(text, _)| text == source));
        let index = if let Some(index) = cached {
            index
        } else {
            let parsed = Expression::parse(source, &ParseOptions::scxml())
                .map_err(|_| DataModelError::InvalidInput)?;
            if self.len >= MAX_CACHED_EXPRESSIONS {
                self.buckets.clear();
                self.len = 0;
            }
            let bucket = self.buckets.entry(digest).or_default();
            bucket.push((source.to_owned(), parsed));
            self.len = self.len.saturating_add(1);
            bucket.len().saturating_sub(1)
        };
        self.buckets
            .get(&digest)
            .and_then(|bucket| bucket.get(index))
            .map(|(_, expression)| expression)
            .ok_or(DataModelError::InvalidInput)
    }
}

#[derive(Clone, Debug)]
struct State {
    system: SystemContext,
    store: Store,
    /// Document node owning `<datamodel>`.
    document: NodeId,
    /// `<datamodel>`: the `XPath` context node.
    datamodel: NodeId,
    /// `<_sessionid>`, `<_name>`, `<_ioprocessors>`.
    system_nodes: [NodeId; 3],
    /// Variable name to its binding.
    variables: Variables,
    /// `<foreach>` loops in progress, innermost last.
    foreach: Vec<ForeachFrame>,
    /// The current event and its `<_event>` element, attached while bound.
    event: Option<(Event, NodeId)>,
}

impl XPathDataModel {
    /// Creates an uninitialized model.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces all state with `source` and `system` after full validation.
    ///
    /// `source` is empty or a sequence of `<data id="name">value</data>`
    /// elements (whitespace between them allowed); each declares one
    /// variable whose children are its value.
    ///
    /// # Errors
    ///
    /// [`DataModelError::InvalidInput`] for malformed or over-bound source or
    /// system values, invalid, reserved, or duplicate variable names, or more
    /// than [`crate::MAX_VARIABLES`] variables. Failure preserves prior state.
    pub fn initialize(
        &mut self,
        source: &str,
        system: SystemContext,
    ) -> Result<(), DataModelError> {
        let span = observe(
            Module::DataModelXpathComponent,
            Operation::XpathDataModelInitialize,
        );
        let result = (|| {
            if source.len() > MAX_SOURCE_BYTES {
                return Err(DataModelError::InvalidInput);
            }
            let mut state = State::new(system)?;
            let nodes = state
                .store
                .parse_fragment(source)
                .map_err(|_| DataModelError::InvalidInput)?;
            for node in nodes {
                match state.store.kind(NodeRef::node(node)) {
                    Ok(NodeKind::Element) => {}
                    Ok(NodeKind::Text) if text_is_whitespace(&state.store, node)? => continue,
                    _ => return Err(DataModelError::InvalidInput),
                }
                let name = declared_name(&state.store, node)?;
                state.attach_variable(&name, node)?;
            }
            self.state = Some(state);
            Ok(())
        })();
        finish(&span, &result);
        result
    }

    /// Declares `declarations` (document order, unique) as empty node-sets
    /// and binds `system` (WIT `data-model-api.initialize`). `_ioprocessors`
    /// additionally binds [`crate::XPATH_TEMPLATE_PROCESSOR_NAME`] to the
    /// [`crate::SCXML_EVENT_PROCESSOR_NAME`] entry when one is present.
    ///
    /// # Errors
    ///
    /// As [`Self::initialize`] (duplicate declarations included). Failure
    /// preserves prior state.
    pub fn initialize_declarations<'a>(
        &mut self,
        declarations: impl IntoIterator<Item = &'a str>,
        mut system: SystemContext,
    ) -> Result<(), DataModelError> {
        if let DataValue::JsonObject(text) = &system.io_processors {
            let mut processors: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(text).map_err(|_| DataModelError::InvalidInput)?;
            if let Some(location) = processors.get(crate::SCXML_EVENT_PROCESSOR_NAME).cloned() {
                processors.insert(crate::XPATH_TEMPLATE_PROCESSOR_NAME.to_owned(), location);
            }
            system.io_processors = DataValue::JsonObject(
                serde_json::to_string(&serde_json::Value::Object(processors))
                    .map_err(|_| DataModelError::InvalidInput)?,
            );
        }
        self.initialize(&crate::declaration_source(declarations), system)
    }

    /// Values one declared `<data>` variable (WIT
    /// `data-model-api.initialize-data`; SCXML 5.2-5.3). An expression is
    /// evaluated with no current event. A materialized value is the
    /// `<data>` content (inline children or a `src` document): XML is
    /// inserted as is, and string content is inserted as its
    /// space-normalized literal (SCXML 1.0 CR 2014-03-13 §C.3.1).
    ///
    /// # Errors
    ///
    /// [`DataModelError::InvalidInput`] when `id` is not declared, and the
    /// failures of [`Self::evaluate_value`] and [`Self::set_declared`].
    pub fn initialize_data<H: ExecutionHost + ?Sized>(
        &mut self,
        id: &str,
        initializer: &DataInitializer,
        host: &mut H,
    ) -> Result<(), DataModelError> {
        let evaluated;
        let value = match initializer {
            DataInitializer::Expression(expression) => {
                evaluated = self.evaluate_value(expression, None, host)?;
                &evaluated
            }
            DataInitializer::Value(DataValue::String(content)) => {
                evaluated = DataValue::String(space_normalized(content));
                &evaluated
            }
            DataInitializer::Value(value) => value,
        };
        if self.set_declared(id, value)? {
            Ok(())
        } else {
            Err(DataModelError::InvalidInput)
        }
    }

    /// Evaluates a condition (`XPath` `boolean()` of the result) with `_event`
    /// bound to `event` (unbound when `None`).
    ///
    /// # Errors
    ///
    /// [`DataModelError::InvalidInput`] for malformed or over-bound source,
    /// [`DataModelError::Execution`] for evaluation failures (undefined
    /// variables, type errors, exhausted bounds), and
    /// [`DataModelError::Host`] when an `In()` query fails.
    pub fn evaluate_condition<H: ExecutionHost + ?Sized>(
        &mut self,
        expression: &str,
        event: Option<&Event>,
        host: &mut H,
    ) -> Result<bool, DataModelError> {
        let span = observe(
            Module::DataModelXpathComponent,
            Operation::XpathDataModelEvaluateCondition,
        );
        let result = self.evaluate_with(expression, event, host, |_, value| Ok(value.to_boolean()));
        finish(&span, &result);
        result
    }

    /// Evaluates a value expression to a portable value (see the crate
    /// contract for the node-set, string, number, and boolean mapping).
    ///
    /// # Errors
    ///
    /// As [`Self::evaluate_condition`]; additionally
    /// [`DataModelError::Execution`] when a node-set's markup exceeds
    /// [`MAX_VALUE_BYTES`].
    pub fn evaluate_value<H: ExecutionHost + ?Sized>(
        &mut self,
        expression: &str,
        event: Option<&Event>,
        host: &mut H,
    ) -> Result<DataValue, DataModelError> {
        let span = observe(
            Module::DataModelXpathComponent,
            Operation::XpathDataModelEvaluateValue,
        );
        let result = self.evaluate_with(expression, event, host, to_data_value);
        finish(&span, &result);
        result
    }

    /// Reads a location (`<param location>`, a `namelist` entry): the
    /// non-empty node-set it selects, as [`DataValue::Xml`] markup.
    ///
    /// # Errors
    ///
    /// As [`Self::evaluate_condition`]; additionally
    /// [`DataModelError::Execution`] when the location selects no nodes (it
    /// does not exist, SCXML 5.9/6.2.5) or its markup exceeds
    /// [`MAX_VALUE_BYTES`].
    pub fn evaluate_location<H: ExecutionHost + ?Sized>(
        &mut self,
        location: &str,
        event: Option<&Event>,
        host: &mut H,
    ) -> Result<DataValue, DataModelError> {
        let span = observe(
            Module::DataModelXpathComponent,
            Operation::XpathDataModelEvaluateValue,
        );
        let result = self.evaluate_with(location, event, host, |store, value| match value {
            Value::NodeSet(set) if !set.is_empty() => to_data_value(store, Value::NodeSet(set)),
            _ => Err(DataModelError::Execution),
        });
        finish(&span, &result);
        result
    }

    /// Evaluates an expression to its `XPath` `string()` value, for
    /// attributes that SCXML defines as strings (`eventexpr`, `targetexpr`,
    /// `typeexpr`, `delayexpr`, `sendidexpr`, `srcexpr`, `<log expr>`).
    ///
    /// # Errors
    ///
    /// As [`Self::evaluate_condition`]; additionally
    /// [`DataModelError::Execution`] for a string over [`MAX_VALUE_BYTES`].
    pub fn evaluate_string<H: ExecutionHost + ?Sized>(
        &mut self,
        expression: &str,
        event: Option<&Event>,
        host: &mut H,
    ) -> Result<String, DataModelError> {
        let span = observe(
            Module::DataModelXpathComponent,
            Operation::XpathDataModelEvaluateValue,
        );
        let result = self.evaluate_with(expression, event, host, |store, value| {
            let text = value.to_string(store).map_err(map_xpath_error)?;
            bounded_value(&text)?;
            Ok(text)
        });
        finish(&span, &result);
        result
    }

    /// Evaluates a `<foreach array>` expression to its items: a node-set,
    /// one [`DataValue::Xml`] per node in document order, none for an empty
    /// node-set (SCXML 1.0 CR 2014-03-13 §C.3.10: "the legal iterable
    /// collections are Node-Sets"). The snapshot is taken once, so body
    /// assignments that change the collection never change the iteration.
    /// A non-empty result starts a loop: binding its items in order to one
    /// variable ([`Self::bind`]) makes that variable each node in turn.
    ///
    /// # Errors
    ///
    /// As [`Self::evaluate_condition`]; additionally
    /// [`DataModelError::Execution`] when the result is not a node-set (not a
    /// legal iteration collection) or an item exceeds [`MAX_VALUE_BYTES`].
    pub fn evaluate_items<H: ExecutionHost + ?Sized>(
        &mut self,
        expression: &str,
        event: Option<&Event>,
        host: &mut H,
    ) -> Result<Vec<DataValue>, DataModelError> {
        let span = observe(
            Module::DataModelXpathComponent,
            Operation::XpathDataModelEvaluateValue,
        );
        let result = self
            .evaluate_with(expression, event, host, |store, value| {
                let Value::NodeSet(set) = value else {
                    return Err(DataModelError::Execution);
                };
                set.nodes()
                    .iter()
                    .map(|node| {
                        let mut out = String::new();
                        push_node_markup(store, *node, &mut out)?;
                        bounded_value(&out)?;
                        Ok((node.id(), DataValue::Xml(out)))
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .and_then(|items| {
                let (nodes, images): (Vec<_>, Vec<_>) = items.into_iter().unzip();
                if !images.is_empty() {
                    let state = self.state.as_mut().ok_or(DataModelError::NotInitialized)?;
                    if scxml_types::exceeds_wasm_bound(
                        state.foreach.len().saturating_add(1),
                        MAX_FOREACH_FRAMES,
                    ) {
                        // Forget the oldest loop (one an error left unfinished).
                        state.foreach.remove(0);
                    }
                    state.foreach.push(ForeachFrame {
                        nodes,
                        images: images.clone(),
                        item: None,
                        next: 0,
                    });
                }
                Ok(images)
            });
        finish(&span, &result);
        result
    }

    /// Forgets every `<foreach>` in progress (WIT
    /// `data-model-api.release-foreach`): the machine call that ran them
    /// returned, so each finished, or an error in its body ended it early
    /// (REC 4.6) and it never finishes. Without this, each loop an error
    /// ended would stay recorded for the rest of the session. Nothing to
    /// forget before initialization.
    pub fn release_foreach(&mut self) {
        if let Some(state) = self.state.as_mut() {
            state.foreach.clear();
        }
    }

    /// The `<foreach>` loops in progress (read-only).
    #[must_use]
    pub fn foreach_in_progress(&self) -> usize {
        self.state.as_ref().map_or(0, |state| state.foreach.len())
    }

    /// Assigns `value` to every node `location` selects, placed per `kind`
    /// (SCXML 5.4 and B.2), with `_event` bound to `event` while the location
    /// is evaluated. All-or-nothing across the selected nodes.
    ///
    /// # Errors
    ///
    /// [`DataModelError::InvalidInput`] for malformed or over-bound input,
    /// [`DataModelError::Execution`] when the location selects no nodes, a
    /// read-only node, a node the placement cannot express, or the value is
    /// not well-formed, [`DataModelError::UnsupportedValue`] for JSON values,
    /// and [`DataModelError::Host`] for failed `In()` queries.
    pub fn assign<H: ExecutionHost + ?Sized>(
        &mut self,
        location: &str,
        kind: &AssignPlacement,
        value: &DataValue,
        event: Option<&Event>,
        host: &mut H,
    ) -> Result<(), DataModelError> {
        let span = observe(
            Module::DataModelXpathComponent,
            Operation::XpathDataModelAssign,
        );
        let result = (|| {
            validate_value(value)?;
            // Reclaim garbage before any node handle is held.
            self.state
                .as_mut()
                .ok_or(DataModelError::NotInitialized)?
                .compact_if_needed()?;
            let targets = self.evaluate_with(location, event, host, |_, value| match value {
                Value::NodeSet(set) if !set.is_empty() => Ok(set.nodes().to_vec()),
                _ => Err(DataModelError::Execution),
            })?;
            let state = self.state.as_mut().ok_or(DataModelError::NotInitialized)?;
            state.assign_nodes(&targets, kind, value)
        })();
        finish(&span, &result);
        result
    }

    /// Executes one SCXML `<assign>` or `<foreach>` `item`/`index`
    /// assignment (SCXML 5.4, 4.6; SCXML 1.0 CR 2014-03-13 §C.3.6): the
    /// single definition of this data model's assignment semantics, which
    /// the WIT component executes for every machine.
    ///
    /// * [`AssignSource::Expression`] is evaluated with `_event` bound to
    ///   `event` and the result placed per `placement` ([`Self::assign`]).
    /// * [`AssignSource::Value`] places a materialized value
    ///   ([`Self::assign`]). The location is always an `XPath` location
    ///   expression that must select nodes: a plain name is a relative path
    ///   from `<datamodel>`, never a variable, so it declares nothing.
    /// * [`AssignSource::ForeachValue`] binds a `<foreach>` variable by name,
    ///   declaring it when absent ([`Self::bind`]); only
    ///   [`AssignPlacement::ReplaceChildren`] is accepted with it.
    ///
    /// # Errors
    ///
    /// As the operation the source selects, and
    /// [`DataModelError::InvalidInput`] for a `ForeachValue` with any other
    /// placement. Failure commits nothing.
    pub fn apply_assign<H: ExecutionHost + ?Sized>(
        &mut self,
        location: &str,
        source: &AssignSource,
        placement: &AssignPlacement,
        event: Option<&Event>,
        host: &mut H,
    ) -> Result<(), DataModelError> {
        match source {
            AssignSource::Expression(expression) => {
                let value = self.evaluate_value(expression, event, host)?;
                self.assign(location, placement, &value, event, host)
            }
            AssignSource::Value(value) => self.assign(location, placement, value, event, host),
            AssignSource::ForeachValue(value) => {
                if *placement != AssignPlacement::ReplaceChildren {
                    return Err(DataModelError::InvalidInput);
                }
                self.bind(location, value, event, host)
            }
        }
    }

    /// Binds a value to a variable name or assigns it to a location.
    ///
    /// A plain name (an XML `NCName`, as `<foreach item/index>` uses) binds
    /// that variable, declaring it when absent: the next item of a
    /// `<foreach>` in progress ([`Self::evaluate_items`]) makes `$name` the
    /// collection node itself (SCXML 1.0 §4.6), and any other value
    /// replaces the children of the variable's `<data id="name">` element.
    /// Anything else is a location and is assigned with `replacechildren`.
    ///
    /// # Errors
    ///
    /// [`DataModelError::Execution`] for reserved names, a full variable
    /// table, or location failures (see [`Self::assign`]).
    pub fn bind<H: ExecutionHost + ?Sized>(
        &mut self,
        name_or_location: &str,
        value: &DataValue,
        event: Option<&Event>,
        host: &mut H,
    ) -> Result<(), DataModelError> {
        if !is_ncname(name_or_location) {
            return self.assign(
                name_or_location,
                &AssignPlacement::ReplaceChildren,
                value,
                event,
                host,
            );
        }
        let span = observe(
            Module::DataModelXpathComponent,
            Operation::XpathDataModelAssign,
        );
        let result = (|| {
            validate_value(value)?;
            let state = self.state.as_mut().ok_or(DataModelError::NotInitialized)?;
            state.bind_variable(name_or_location, value)
        })();
        finish(&span, &result);
        result
    }

    /// Declares `name` as an empty `<data id="name"/>` variable when it is
    /// not declared yet (SCXML 5.3 late binding); an existing binding is kept.
    ///
    /// # Errors
    ///
    /// [`DataModelError::InvalidInput`] for invalid or reserved names and a
    /// full variable table.
    pub fn declare(&mut self, name: &str) -> Result<(), DataModelError> {
        let span = observe(
            Module::DataModelXpathComponent,
            Operation::XpathDataModelAssign,
        );
        let result = (|| {
            let state = self.state.as_mut().ok_or(DataModelError::NotInitialized)?;
            if state.variables.contains_key(name) {
                return Ok(());
            }
            let element = state.new_data_element(name)?;
            state.attach_variable(name, element)
        })();
        finish(&span, &result);
        result
    }

    /// Replaces the value of declared variable `name` (its children) and
    /// reports whether it was declared; an undeclared name changes nothing
    /// (SCXML 6.4.2 invoke parameters for absent locations).
    ///
    /// # Errors
    ///
    /// As [`Self::assign`] for the value.
    pub fn set_declared(&mut self, name: &str, value: &DataValue) -> Result<bool, DataModelError> {
        let span = observe(
            Module::DataModelXpathComponent,
            Operation::XpathDataModelAssign,
        );
        let result = (|| {
            validate_value(value)?;
            let state = self.state.as_mut().ok_or(DataModelError::NotInitialized)?;
            state.compact_if_needed()?;
            let Some(node) = state.variables.get(name).map(|binding| binding.node()) else {
                return Ok(false);
            };
            state.assign_nodes(
                &[NodeRef::node(node)],
                &AssignPlacement::ReplaceChildren,
                value,
            )?;
            Ok(true)
        })();
        finish(&span, &result);
        result
    }

    /// Executes one `<script>`. The `XPath` data model defines no script
    /// language: a script of only XML white space does nothing, any other
    /// script fails.
    ///
    /// # Errors
    ///
    /// [`DataModelError::InvalidInput`] above [`MAX_SCRIPT_BYTES`] and
    /// [`DataModelError::Execution`] for any non-empty script.
    pub fn execute(&mut self, script: &str) -> Result<(), DataModelError> {
        let span = observe(
            Module::DataModelXpathComponent,
            Operation::XpathDataModelExecute,
        );
        let result = (|| {
            self.state.as_ref().ok_or(DataModelError::NotInitialized)?;
            if scxml_types::exceeds_wasm_bound(script.len(), MAX_SCRIPT_BYTES) {
                return Err(DataModelError::InvalidInput);
            }
            if script.chars().all(scxml_types::is_xml_space) {
                Ok(())
            } else {
                Err(DataModelError::Execution)
            }
        })();
        finish(&span, &result);
        result
    }

    /// Returns the bound system variables.
    ///
    /// # Errors
    ///
    /// [`DataModelError::NotInitialized`] before initialization.
    pub fn system_context(&self) -> Result<SystemContext, DataModelError> {
        self.state
            .as_ref()
            .map(|state| state.system.clone())
            .ok_or(DataModelError::NotInitialized)
    }

    /// Shared evaluation: bounds and parses `expression` (cached), binds
    /// `_event`, evaluates against `<datamodel>`, and converts the result
    /// while the store is borrowed.
    fn evaluate_with<H: ExecutionHost + ?Sized, T>(
        &mut self,
        expression: &str,
        event: Option<&Event>,
        host: &mut H,
        convert: impl FnOnce(&Store, Value) -> Result<T, DataModelError>,
    ) -> Result<T, DataModelError> {
        if scxml_types::exceeds_wasm_bound(expression.len(), MAX_EXPRESSION_BYTES) {
            return Err(DataModelError::InvalidInput);
        }
        let state = self.state.as_mut().ok_or(DataModelError::NotInitialized)?;
        state.bind_event(event)?;
        let parsed = self.cache.parsed(expression)?;
        let mut scope = Scope {
            store: &state.store,
            variables: &state.variables,
            system_nodes: state.system_nodes,
            event: state.event.as_ref().map(|(_, node)| *node),
            host,
            host_failed: false,
        };
        let evaluated = parsed.evaluate(
            &state.store,
            NodeRef::node(state.datamodel),
            &mut scope,
            EVALUATION_FUEL,
        );
        let host_failed = scope.host_failed;
        match evaluated {
            Ok(value) => convert(&state.store, value),
            Err(_) if host_failed => Err(DataModelError::Host),
            Err(error) => Err(map_xpath_error(error)),
        }
    }
}

impl State {
    /// Builds the document, `<datamodel>`, and read-only system variables.
    fn new(system: SystemContext) -> Result<Self, DataModelError> {
        if system.session_id.is_empty()
            || system.machine_name.as_ref().is_some_and(String::is_empty)
            || system.session_id.len() > MAX_VALUE_BYTES
            || system
                .machine_name
                .as_ref()
                .is_some_and(|name| name.len() > MAX_VALUE_BYTES)
        {
            return Err(DataModelError::InvalidInput);
        }
        let DataValue::JsonObject(processors) = &system.io_processors else {
            return Err(DataModelError::InvalidInput);
        };
        if processors.len() > MAX_VALUE_BYTES {
            return Err(DataModelError::InvalidInput);
        }
        let parsed: serde_json::Value =
            serde_json::from_str(processors).map_err(|_| DataModelError::InvalidInput)?;
        // Canonical JSON only: compact, recursively sorted keys.
        if serde_json::to_string(&parsed).map_err(|_| DataModelError::InvalidInput)? != *processors
        {
            return Err(DataModelError::InvalidInput);
        }
        let processors = parsed;
        let mut store = Store::new();
        let document = store
            .create_root()
            .map_err(|_| DataModelError::InvalidInput)?;
        let datamodel = new_element(&mut store, "datamodel")?;
        append(&mut store, document, datamodel)?;
        let session_id = text_element(&mut store, "_sessionid", &system.session_id)?;
        // SCXML 5.10: `_name` is "the value of the 'name' attribute of the
        // <scxml> element"; a document without one leaves it empty.
        let name = text_element(
            &mut store,
            "_name",
            system.machine_name.as_deref().unwrap_or_default(),
        )?;
        let io_processors = io_processors_element(&mut store, &processors)?;
        for node in [session_id, name, io_processors] {
            append(&mut store, datamodel, node)?;
        }
        Ok(Self {
            system,
            store,
            document,
            datamodel,
            system_nodes: [session_id, name, io_processors],
            variables: Variables::default(),
            foreach: Vec::new(),
            event: None,
        })
    }

    /// Appends `node` under `<datamodel>` as variable `name`.
    fn attach_variable(&mut self, name: &str, node: NodeId) -> Result<(), DataModelError> {
        validate_variable_name(name)?;
        if self.variables.contains_key(name) || self.variables.len() >= MAX_VARIABLES {
            return Err(DataModelError::InvalidInput);
        }
        // Variables precede a bound `<_event>`, which stays last.
        match self.event.as_ref() {
            Some((_, event)) => self.store.insert_before(*event, node),
            None => self.store.append_child(self.datamodel, node),
        }
        .map_err(map_xpath_error)?;
        self.variables.insert(
            name.to_owned(),
            Binding {
                element: node,
                item: None,
            },
        );
        Ok(())
    }

    fn new_data_element(&mut self, name: &str) -> Result<NodeId, DataModelError> {
        validate_variable_name(name)?;
        let element = new_element(&mut self.store, "data")?;
        self.store
            .set_attribute(element, local_name("id")?, name)
            .map_err(map_xpath_error)?;
        Ok(element)
    }

    /// Binds `name` to `value` (see [`XPathDataModel::bind`]), declaring
    /// `name` when it is not declared yet. When a `<foreach>` in progress
    /// expects `value` as its next item under `name`, `$name` becomes that
    /// collection node itself (REC 4.6: "MUST act as if it has made a
    /// shallow copy of the collection", and a shallow copy of a node-set
    /// holds its nodes); otherwise `value` replaces the children of the
    /// variable's `<data>` element. Neither removes the element.
    fn bind_variable(&mut self, name: &str, value: &DataValue) -> Result<(), DataModelError> {
        validate_variable_name(name).map_err(|_| DataModelError::Execution)?;
        self.compact_if_needed()?;
        let item = self.foreach_node(name, value);
        let Some(element) = self.variables.get(name).map(|binding| binding.element) else {
            let element = self.new_data_element(name)?;
            if item.is_none() {
                self.fill(element, value)?;
            }
            self.attach_variable(name, element)
                .map_err(|_| DataModelError::Execution)?;
            if let Some(binding) = self.variables.get_mut(name) {
                binding.item = item;
            }
            return Ok(());
        };
        if item.is_none() {
            self.assign_nodes(
                &[NodeRef::node(element)],
                &AssignPlacement::ReplaceChildren,
                value,
            )?;
        }
        if let Some(binding) = self.variables.get_mut(name) {
            binding.item = item;
        }
        Ok(())
    }

    /// The collection node a `<foreach>` binds when `name` is assigned the
    /// item `value`: the next node of the innermost loop whose next item is
    /// `value` and whose item variable is `name` (or not yet known). That
    /// loop advances; one whose last item is bound ends, with every loop
    /// inside it. `None` when no loop expects the binding (an `index`, or a
    /// binding outside any loop) or the item has no tree node.
    fn foreach_node(&mut self, name: &str, value: &DataValue) -> Option<NodeId> {
        let position = self.foreach.iter().rposition(|frame| {
            frame.next < frame.images.len()
                && frame.item.as_deref().is_none_or(|item| item == name)
                && frame.images.get(frame.next) == Some(value)
        })?;
        let frame = self.foreach.get_mut(position)?;
        let node = frame.nodes.get(frame.next).copied().flatten();
        frame.item = Some(name.to_owned());
        frame.next += 1;
        if frame.next == frame.images.len() {
            self.foreach.truncate(position);
        }
        node
    }

    /// Appends `value` as the children of a detached element.
    fn fill(&mut self, element: NodeId, value: &DataValue) -> Result<(), DataModelError> {
        let input = self.assign_input(value)?;
        let refs = input.refs();
        self.store
            .assign(element, AssignMode::ReplaceChildren, input.value(&refs))
            .map_err(|_| DataModelError::Execution)
    }

    /// Applies one assignment to every target, all-or-nothing. Callers
    /// compact (if needed) before computing `targets`.
    fn assign_nodes(
        &mut self,
        targets: &[NodeRef],
        kind: &AssignPlacement,
        value: &DataValue,
    ) -> Result<(), DataModelError> {
        let mode = match kind {
            AssignPlacement::ReplaceChildren => AssignMode::ReplaceChildren,
            AssignPlacement::FirstChild => AssignMode::FirstChild,
            AssignPlacement::LastChild => AssignMode::LastChild,
            AssignPlacement::PreviousSibling => AssignMode::PreviousSibling,
            AssignPlacement::NextSibling => AssignMode::NextSibling,
            AssignPlacement::Replace => AssignMode::Replace,
            AssignPlacement::Delete => AssignMode::Delete,
            AssignPlacement::AddAttribute(name) => {
                AssignMode::AddAttribute(QName::local(name).map_err(|_| DataModelError::Execution)?)
            }
        };
        let removes_target = matches!(kind, AssignPlacement::Replace | AssignPlacement::Delete);
        let mut target_ids = Vec::with_capacity(targets.len());
        for target in targets {
            let id = target.id().ok_or(DataModelError::Execution)?;
            let owner = self.owning_variable(id)?.ok_or(DataModelError::Execution)?;
            if removes_target && owner == id {
                return Err(DataModelError::Execution);
            }
            target_ids.push(id);
        }
        let input = if matches!(kind, AssignPlacement::Delete) {
            AssignInput::Nodes(Vec::new())
        } else {
            self.assign_input(value)?
        };
        let refs = input.refs();
        let assigned = if matches!(kind, AssignPlacement::Delete) {
            AssignValue::None
        } else {
            input.value(&refs)
        };
        if let [target] = target_ids.as_slice() {
            // `Store::assign` is transactional on its own.
            return self
                .store
                .assign(*target, mode, assigned)
                .map_err(|_| DataModelError::Execution);
        }
        // Several targets: restore the pre-assignment store on any failure.
        let checkpoint = self.store.clone();
        for target in &target_ids {
            if self.store.assign(*target, mode.clone(), assigned).is_err() {
                self.store = checkpoint;
                return Err(DataModelError::Execution);
            }
        }
        Ok(())
    }

    /// The variable element whose tree contains `node`, or `None` when
    /// `node` is a system variable, `_event`, `<datamodel>`, or the document.
    fn owning_variable(&self, node: NodeId) -> Result<Option<NodeId>, DataModelError> {
        let mut current = node;
        for _ in 0..=limits::MAX_TREE_DEPTH {
            let parent = self
                .store
                .parent(NodeRef::node(current))
                .map_err(map_xpath_error)?;
            match parent {
                Some(parent) if parent == self.datamodel => {
                    return Ok(self
                        .variables
                        .values()
                        .any(|binding| binding.element == current)
                        .then_some(current));
                }
                Some(parent) => current = parent,
                None => return Ok(None),
            }
        }
        Err(DataModelError::Execution)
    }

    /// Converts a value into detached assignment input.
    fn assign_input(&mut self, value: &DataValue) -> Result<AssignInput, DataModelError> {
        if let DataValue::Xml(markup) = value {
            let nodes = self
                .store
                .parse_fragment(markup)
                .map_err(|_| DataModelError::Execution)?;
            return Ok(AssignInput::Nodes(nodes));
        }
        let text = scalar_text(value).ok_or(DataModelError::UnsupportedValue)?;
        Ok(if text.is_empty() {
            AssignInput::Nodes(Vec::new())
        } else {
            AssignInput::Text(text)
        })
    }

    /// Binds `_event` to `event`: reuses the attached tree for the same
    /// event, otherwise detaches it and (for `Some`) builds a fresh one.
    fn bind_event(&mut self, event: Option<&Event>) -> Result<(), DataModelError> {
        if let (Some(event), Some((bound, _))) = (event, self.event.as_ref())
            && bound == event
        {
            return Ok(());
        }
        if let Some((_, node)) = self.event.take() {
            self.store.detach(node).map_err(map_xpath_error)?;
        }
        self.compact_if_needed()?;
        let Some(event) = event else {
            return Ok(());
        };
        event.validate().map_err(|_| DataModelError::InvalidInput)?;
        let node = event_element(&mut self.store, event)?;
        append(&mut self.store, self.datamodel, node)?;
        self.event = Some((event.clone(), node));
        Ok(())
    }

    /// Reclaims detached garbage once the store passes half of a bound.
    fn compact_if_needed(&mut self) -> Result<(), DataModelError> {
        let slots = self.store.len();
        let bytes = self.store.retained_bytes();
        if slots.saturating_mul(COMPACT_DENOMINATOR)
            <= COMPACTION_STORE_NODES.saturating_mul(COMPACT_NUMERATOR)
            && bytes.saturating_mul(COMPACT_DENOMINATOR)
                <= COMPACTION_STORE_BYTES.saturating_mul(COMPACT_NUMERATOR)
        {
            return Ok(());
        }
        let names: Vec<String> = self.variables.keys().cloned().collect();
        let mut keep: Vec<NodeId> = vec![self.document, self.datamodel];
        keep.extend(self.system_nodes);
        for name in &names {
            let binding = self.variables.get(name).ok_or(DataModelError::Execution)?;
            keep.push(binding.element);
            keep.extend(binding.item);
        }
        if let Some((_, node)) = self.event.as_ref() {
            keep.push(*node);
        }
        // Loop items stay alive (and keep their identity) while the loop runs.
        keep.extend(
            self.foreach
                .iter()
                .flat_map(|frame| frame.nodes.iter().flatten()),
        );
        self.store.compact(&mut keep).map_err(map_xpath_error)?;
        let mut kept = keep.into_iter();
        let mut next = || kept.next().ok_or(DataModelError::Execution);
        self.document = next()?;
        self.datamodel = next()?;
        self.system_nodes = [next()?, next()?, next()?];
        for name in names {
            let element = next()?;
            let binding = self
                .variables
                .get_mut(&name)
                .ok_or(DataModelError::Execution)?;
            binding.element = element;
            if binding.item.is_some() {
                binding.item = Some(next()?);
            }
        }
        if let Some((_, node)) = self.event.as_mut() {
            *node = next()?;
        }
        for frame in &mut self.foreach {
            for node in frame.nodes.iter_mut().flatten() {
                *node = next()?;
            }
        }
        Ok(())
    }
}

impl XPathDataModel {
    /// Read-only image of the committed state (`docs/session-snapshot.md`,
    /// "Session object (keys in this order)"): the system context and every variable's element markup in
    /// document order of `<datamodel>`. Values are read by serialization,
    /// never by evaluating an expression, and nothing changes. Markup longer
    /// than [`MAX_VALUE_BYTES`] is [`VariableMarkup::Oversized`]; nothing is
    /// truncated.
    ///
    /// # Errors
    ///
    /// [`DataModelError::NotInitialized`] before initialization and
    /// [`DataModelError::Execution`] when a committed node does not
    /// serialize (a broken store invariant).
    pub fn snapshot(&self) -> Result<DataModelSnapshot, DataModelError> {
        let span = observe(
            Module::DataModelXpathComponent,
            Operation::XpathDataModelSnapshot,
        );
        let result = (|| {
            let state = self.state.as_ref().ok_or(DataModelError::NotInitialized)?;
            let mut variables = Vec::with_capacity(state.variables.len());
            for child in state
                .store
                .children(state.datamodel)
                .map_err(map_xpath_error)?
            {
                let Some((name, binding)) = state
                    .variables
                    .iter()
                    .find(|(_, binding)| binding.element == child)
                else {
                    continue;
                };
                let mut markup = String::new();
                state
                    .store
                    .serialize(NodeRef::node(binding.node()), &mut markup)
                    .map_err(map_xpath_error)?;
                let markup = if markup.len() > MAX_VALUE_BYTES {
                    VariableMarkup::Oversized(u64::try_from(markup.len()).unwrap_or(u64::MAX))
                } else {
                    VariableMarkup::Markup(markup)
                };
                variables.push(Variable {
                    name: name.clone(),
                    markup,
                });
            }
            Ok(DataModelSnapshot {
                system: state.system.clone(),
                variables,
            })
        })();
        finish(&span, &result);
        result
    }
}

/// Detached assignment input owned for the duration of one assignment.
enum AssignInput {
    Text(String),
    Nodes(Vec<NodeId>),
}

impl AssignInput {
    fn refs(&self) -> Vec<NodeRef> {
        match self {
            Self::Text(_) => Vec::new(),
            Self::Nodes(nodes) => nodes.iter().copied().map(NodeRef::node).collect(),
        }
    }

    /// The evaluator input for this value; `refs` is [`Self::refs`].
    fn value<'a>(&'a self, refs: &'a [NodeRef]) -> AssignValue<'a> {
        match self {
            Self::Text(text) => AssignValue::Text(text),
            Self::Nodes(_) => AssignValue::Nodes(refs),
        }
    }
}

/// Dynamic context of one evaluation.
struct Scope<'a, H: ?Sized> {
    store: &'a Store,
    variables: &'a Variables,
    system_nodes: [NodeId; 3],
    event: Option<NodeId>,
    host: &'a mut H,
    host_failed: bool,
}

impl<H: ExecutionHost + ?Sized> Environment for Scope<'_, H> {
    fn variable(&self, name: &str) -> Option<Value> {
        let node = match name {
            "_sessionid" => self.system_nodes[0],
            "_name" => self.system_nodes[1],
            "_ioprocessors" => self.system_nodes[2],
            "_event" => self.event?,
            other => self.variables.get(other)?.node(),
        };
        NodeSet::from_nodes(self.store, vec![NodeRef::node(node)])
            .ok()
            .map(Value::NodeSet)
    }

    fn call_extension(
        &mut self,
        name: &str,
        arguments: &[Value],
        store: &Store,
    ) -> Result<Value, scxml_xpath::Error> {
        let [argument] = arguments else {
            return Err(scxml_xpath::Error::Extension);
        };
        if name != "In" {
            return Err(scxml_xpath::Error::Extension);
        }
        let state_id = argument.to_string(store)?;
        if let Ok(active) = self.host.in_state(&state_id) {
            Ok(Value::Boolean(active))
        } else {
            self.host_failed = true;
            Err(scxml_xpath::Error::Extension)
        }
    }
}

/// Converts an evaluation result to a portable value.
fn to_data_value(store: &Store, value: Value) -> Result<DataValue, DataModelError> {
    Ok(match value {
        Value::Boolean(value) => DataValue::Bool(value),
        Value::Number(value) => match FiniteNumber::new(value) {
            Ok(number) => DataValue::Number(number),
            Err(_) => DataValue::String(number_to_string(value)),
        },
        Value::String(value) => {
            bounded_value(&value)?;
            DataValue::String(value)
        }
        Value::NodeSet(set) => {
            let mut out = String::new();
            for node in set.nodes() {
                push_node_markup(store, *node, &mut out)?;
                bounded_value(&out)?;
            }
            DataValue::Xml(out)
        }
    })
}

/// Appends the markup of one node: elements, text, comments, and
/// processing instructions serialize; the document contributes its
/// children; attribute and namespace nodes contribute their escaped
/// string-value (they cannot be serialized as content).
fn push_node_markup(store: &Store, node: NodeRef, out: &mut String) -> Result<(), DataModelError> {
    match store.kind(node).map_err(map_xpath_error)? {
        NodeKind::Attribute | NodeKind::Namespace => {
            let text = store.string_value(node).map_err(map_xpath_error)?;
            escape(out, &text, Escape::Text);
        }
        NodeKind::Root => {
            let id = node.id().ok_or(DataModelError::Execution)?;
            store.serialize_children(id, out).map_err(map_xpath_error)?;
        }
        NodeKind::Element
        | NodeKind::Text
        | NodeKind::Comment
        | NodeKind::ProcessingInstruction => {
            store.serialize(node, out).map_err(map_xpath_error)?;
        }
    }
    Ok(())
}

/// Builds `<_event>` with every field present (SCXML 5.10.1, B.2).
fn event_element(store: &mut Store, event: &Event) -> Result<NodeId, DataModelError> {
    let root = new_element(store, "_event")?;
    let event_type = match event.event_type {
        EventType::Internal => "internal",
        EventType::External => "external",
        EventType::Platform => "platform",
    };
    let fields = [
        ("name", Some(event.name.as_str())),
        ("type", Some(event_type)),
        (
            "sendid",
            event.send_id.as_ref().map(scxml_types::SendId::as_str),
        ),
        ("origin", event.origin.as_deref()),
        ("origintype", event.origin_type.as_deref()),
        (
            "invokeid",
            event.invoke_id.as_ref().map(scxml_types::InvokeId::as_str),
        ),
    ];
    for (field, value) in fields {
        let element = text_element(store, field, value.unwrap_or_default())?;
        append(store, root, element)?;
    }
    let data = new_element(store, "data")?;
    fill_event_data(store, data, &event.data)?;
    append(store, root, data)?;
    let raw = text_element(store, "raw", event.raw.as_deref().unwrap_or_default())?;
    append(store, root, raw)?;
    Ok(root)
}

/// Populates `_event`'s `<data>` child from the event payload in the order
/// SCXML 1.0 CR 2014-03-13 §C.3.7.1 prescribes. An XML value is the
/// sender's indication that the content is a node-set ("the Processor
/// SHOULD try to format the data according to those indications"; this
/// model's own `<param>`/namelist payloads arrive as `<data id>` markup) and
/// is inserted as is when it parses. Otherwise key-value pairs (a JSON
/// object, as other data models and I/O processors deliver them) become one
/// `<data id="key">` child per key; otherwise text that is a valid XML
/// document is inserted as XML; otherwise the content is a space-normalized
/// string literal (other text, a JSON array's text, markup that does not
/// parse, a scalar).
///
/// Every translation exists, so any valid [`Event`] binds: content is
/// inserted as nodes, never re-serialized as markup, so no value bound
/// applies beyond the evaluator's store limits.
fn fill_event_data(
    store: &mut Store,
    data: NodeId,
    value: &DataValue,
) -> Result<(), DataModelError> {
    match value {
        DataValue::Absent | DataValue::Undefined | DataValue::Null => Ok(()),
        DataValue::JsonObject(text) => match serde_json::from_str(text) {
            Ok(serde_json::Value::Object(members)) => append_members(store, data, &members, 0),
            _ => append_text(store, data, &space_normalized(text)),
        },
        DataValue::Xml(text) | DataValue::String(text) => {
            let nodes = store
                .parse_fragment(text)
                .ok()
                .filter(|nodes| matches!(value, DataValue::Xml(_)) || is_document(store, nodes));
            match nodes {
                Some(nodes) => nodes
                    .into_iter()
                    .try_for_each(|node| append(store, data, node)),
                None => append_text(store, data, &space_normalized(text)),
            }
        }
        DataValue::JsonArray(text) => append_text(store, data, &space_normalized(text)),
        DataValue::Bool(_) | DataValue::Number(_) => {
            append_text(store, data, &scalar_text(value).unwrap_or_default())
        }
    }
}

/// Whether `value` binds as `_event.data` ([`fill_event_data`] into a
/// fresh store succeeds); [`crate::accepts_event_data`].
pub(crate) fn event_data_binds(value: &DataValue) -> bool {
    let mut store = Store::new();
    new_element(&mut store, "data")
        .and_then(|data| fill_event_data(&mut store, data, value))
        .is_ok()
}

/// Whether parsed top-level `nodes` form an XML document's content: exactly
/// one element, with only comments, processing instructions, and whitespace
/// around it (XML 1.0 §2.1 `document ::= prolog element Misc*`).
fn is_document(store: &Store, nodes: &[NodeId]) -> bool {
    let mut elements = 0_usize;
    for node in nodes {
        match store.kind(NodeRef::node(*node)) {
            Ok(NodeKind::Element) => elements += 1,
            Ok(NodeKind::Comment | NodeKind::ProcessingInstruction) => {}
            Ok(NodeKind::Text) if text_is_whitespace(store, *node).unwrap_or(false) => {}
            _ => return false,
        }
    }
    elements == 1
}

/// Appends one `<data id="key">` child per key-value pair (§C.3.7.1): the
/// value is the element's content, inserted verbatim. Nested objects nest
/// up to [`MAX_JSON_DEPTH`]; an array, or an object nested deeper, is its
/// JSON text.
fn append_members(
    store: &mut Store,
    parent: NodeId,
    members: &serde_json::Map<String, serde_json::Value>,
    depth: usize,
) -> Result<(), DataModelError> {
    for (key, value) in members {
        let element = new_element(store, "data")?;
        store
            .set_attribute(element, local_name("id")?, key)
            .map_err(map_xpath_error)?;
        match value {
            serde_json::Value::Null => {}
            serde_json::Value::Bool(value) => {
                append_text(store, element, if *value { "true" } else { "false" })?;
            }
            serde_json::Value::Number(number) => {
                let text = number
                    .as_f64()
                    .map_or_else(|| number.to_string(), number_to_string);
                append_text(store, element, &text)?;
            }
            serde_json::Value::String(text) => append_text(store, element, text)?,
            serde_json::Value::Object(nested) if depth < MAX_JSON_DEPTH => {
                append_members(store, element, nested, depth + 1)?;
            }
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                let text = serde_json::to_string(value).map_err(|_| DataModelError::Execution)?;
                append_text(store, element, &text)?;
            }
        }
        append(store, parent, element)?;
    }
    Ok(())
}

/// Appends `text` as one text node (nothing when empty).
fn append_text(store: &mut Store, parent: NodeId, text: &str) -> Result<(), DataModelError> {
    if text.is_empty() {
        return Ok(());
    }
    let node = store.create_text(text).map_err(map_xpath_error)?;
    append(store, parent, node)
}

/// Builds `<_ioprocessors>` from the canonical JSON processor map: one
/// `<processor name="…">` per entry with each string field as a child.
fn io_processors_element(
    store: &mut Store,
    processors: &serde_json::Value,
) -> Result<NodeId, DataModelError> {
    let serde_json::Value::Object(processors) = processors else {
        return Err(DataModelError::InvalidInput);
    };
    let root = new_element(store, "_ioprocessors")?;
    for (name, fields) in processors {
        let processor = new_element(store, "processor")?;
        store
            .set_attribute(processor, local_name("name")?, name)
            .map_err(|_| DataModelError::InvalidInput)?;
        let serde_json::Value::Object(fields) = fields else {
            return Err(DataModelError::InvalidInput);
        };
        for (field, value) in fields {
            let serde_json::Value::String(value) = value else {
                return Err(DataModelError::InvalidInput);
            };
            let element = text_element(store, field, value)?;
            append(store, processor, element)?;
        }
        append(store, root, processor)?;
    }
    Ok(root)
}

fn new_element(store: &mut Store, name: &str) -> Result<NodeId, DataModelError> {
    store
        .create_element(local_name(name)?)
        .map_err(map_xpath_error)
}

/// A detached element holding `text` (no child when empty).
fn text_element(store: &mut Store, name: &str, text: &str) -> Result<NodeId, DataModelError> {
    if text.len() > MAX_VALUE_BYTES {
        return Err(DataModelError::InvalidInput);
    }
    let element = new_element(store, name)?;
    if !text.is_empty() {
        let child = store.create_text(text).map_err(map_xpath_error)?;
        append(store, element, child)?;
    }
    Ok(element)
}

fn append(store: &mut Store, parent: NodeId, child: NodeId) -> Result<(), DataModelError> {
    store.append_child(parent, child).map_err(map_xpath_error)
}

fn local_name(name: &str) -> Result<QName, DataModelError> {
    QName::local(name).map_err(|_| DataModelError::InvalidInput)
}

/// The `id` of a declared `<data>` element (null namespace).
fn declared_name(store: &Store, node: NodeId) -> Result<String, DataModelError> {
    let name = store
        .name(NodeRef::node(node))
        .map_err(|_| DataModelError::InvalidInput)?
        .ok_or(DataModelError::InvalidInput)?;
    if name.local_name != "data" || name.namespace_uri.is_some() {
        return Err(DataModelError::InvalidInput);
    }
    store
        .attribute(node, &local_name("id")?)
        .map_err(|_| DataModelError::InvalidInput)?
        .map(str::to_owned)
        .ok_or(DataModelError::InvalidInput)
}

fn text_is_whitespace(store: &Store, node: NodeId) -> Result<bool, DataModelError> {
    store
        .string_value(NodeRef::node(node))
        .map(|text| text.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r')))
        .map_err(|_| DataModelError::InvalidInput)
}

/// A variable name is a non-reserved `NCName` of at most
/// [`scxml_types::MAX_NAME_BYTES`] bytes, so every name fits a snapshot.
fn validate_variable_name(name: &str) -> Result<(), DataModelError> {
    if name.len() <= scxml_types::MAX_NAME_BYTES
        && is_ncname(name)
        && !RESERVED_NAMES.contains(&name)
    {
        Ok(())
    } else {
        Err(DataModelError::InvalidInput)
    }
}

fn validate_value(value: &DataValue) -> Result<(), DataModelError> {
    match value {
        DataValue::String(text) | DataValue::Xml(text) => bounded_value(text),
        DataValue::JsonArray(_) | DataValue::JsonObject(_) => Err(DataModelError::UnsupportedValue),
        DataValue::Absent
        | DataValue::Undefined
        | DataValue::Null
        | DataValue::Bool(_)
        | DataValue::Number(_) => Ok(()),
    }
}

fn bounded_value(text: &str) -> Result<(), DataModelError> {
    if text.len() <= MAX_VALUE_BYTES {
        Ok(())
    } else {
        Err(DataModelError::Execution)
    }
}

/// Maps evaluator failures onto the SCXML error model: malformed source is
/// invalid input, everything the evaluation itself hits (undefined
/// variables, type errors, exhausted bounds, impossible placements) is an
/// `error.execution` failure.
fn map_xpath_error(error: scxml_xpath::Error) -> DataModelError {
    match error {
        scxml_xpath::Error::Syntax(_) => DataModelError::InvalidInput,
        scxml_xpath::Error::UndefinedVariable
        | scxml_xpath::Error::Type
        | scxml_xpath::Error::Extension
        | scxml_xpath::Error::Limit(_)
        | scxml_xpath::Error::Xml(_)
        | scxml_xpath::Error::InvalidNode
        | scxml_xpath::Error::InvalidMutation => DataModelError::Execution,
    }
}
