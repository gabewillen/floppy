//! Rules for the structures the compiler cannot lower to Machine IR.
//!
//! `validate` is the authoring gate `compile` relies on: every document that
//! `scxml-compiler-core` lowering rejects must get a validator diagnostic, at
//! the responsible bytes, instead of a compile-only `COMP001` at the root.
//! Most rejections restate a schema or semantic rule reported elsewhere; the
//! constraints below are the ones the bundled W3C schema allows and no other
//! rule reports. `crates/scxml-compiler-core/tests/validate_covers_lowering.rs`
//! enumerates every lowering rejection and proves each one has a validator
//! error.
//!
//! # Contract
//!
//! * `SCXML064` (error): a `<send>` or `<invoke>` has more than one
//!   `<content>`, an `<invoke>` more than one `<finalize>`, or a `<state>`
//!   more than one `<initial>` (SCXML 1.0 §6.2, §6.4, §3.3: each occurs at
//!   most once).
//! * `SCXML065` (error): an `<assign>` has no value (neither `expr` nor child
//!   content, and not `type="delete"`), or pairs `attr` with a `type` other than `addattribute` or omits it for
//!   `addattribute` (SCXML 1.0 §5.4; SCXML CR 2014 §C.3.6).
//! * `SCXML066` (warning): a required expression attribute is empty, so it
//!   raises `error.execution` whenever it runs: `<if>`/`<elseif>` `cond`, `<foreach>` `array`/`item`,
//!   `<assign>` `location`.
//! * `SCXML067` (error): an `<scxml>` has no child state.
//! * `LINT021` (warning): a `<parallel>` has no child state; conformant,
//!   but no transition is ever enabled through it.
//! * `LIMIT005` (error): the document exceeds a Machine IR bound: 65
//!   nested states, 64 levels of `<if>`/`<foreach>` nesting, 32 levels of
//!   inline XML (31 below a custom action), 10,000 child nodes, attributes, targets, event
//!   descriptors, names, branches, or data declarations per scope, 100,000
//!   targets across the multi-target sets of one machine, or 1 MiB per
//!   string.
//! * The walk follows exactly the elements lowering visits (the machine,
//!   states, their SCXML children, and executable content, not the bodies of
//!   `<data>`, `<content>`, `<assign>`, or custom actions, which lower as
//!   XML); each nested inline machine is walked as its own machine because
//!   hosts compile it when it is invoked. Recursion follows the parsed tree,
//!   whose depth the XML parser bounds (`XmlLimits::max_depth`), and at most
//!   [`MAX_BOUND_FINDINGS`] `LIMIT005` findings are reported per machine.
//! * Pure and bounded by the validated document; it runs inside the
//!   validator's boundary span and records no telemetry of its own.

use std::collections::BTreeSet;

use scxml_syntax::{Attribute, Document, Element, Node, Span, collapse_whitespace, xml_tokens};

use crate::semantic::{Rule, SCXML_NAMESPACE, child_elements, is_scxml, machine_roots};
use crate::spans::{attribute_insertion_point, attribute_removal_span};
use crate::{Applicability, Diagnostic, Fix, FixEdit, Label, Report};

/// Machine IR bounds the compiler enforces while lowering, mirrored so the
/// validator reports a document over them. `scxml-compiler-core` asserts
/// that each equals its own constant. Every bound applies to wasm builds
/// only ([`scxml_types::wasm_bound`]): natively each is `usize::MAX`, so a
/// native `validate` never reports `LIMIT005`.
pub mod bounds {
    use scxml_types::wasm_bound;

    /// Maximum nesting of states, and of `<if>`/`<foreach>` executable
    /// content (`scxml_compiler_core::MAX_CONTENT_DEPTH`).
    pub const MAX_CONTENT_DEPTH: usize = wasm_bound(64);
    /// Maximum child nodes, attributes, targets, descriptors, names, or data
    /// declarations per scope (`scxml_compiler_core::MAX_NESTED_ITEMS`).
    pub const MAX_NESTED_ITEMS: usize = wasm_bound(10_000);
    /// Maximum bytes of one attribute value, text run, script body, name,
    /// or namespace URI (`scxml_compiler_core::MAX_STRING_BYTES`).
    pub const MAX_STRING_BYTES: usize = wasm_bound(1024 * 1024);
    /// Maximum targets across the multi-target sets of one machine
    /// (`scxml_compiler_core::MAX_MULTI_TARGET_VALIDATION_ITEMS`).
    pub const MAX_MULTI_TARGET_ITEMS: usize = wasm_bound(100_000);
}

use bounds::{MAX_CONTENT_DEPTH, MAX_MULTI_TARGET_ITEMS, MAX_NESTED_ITEMS, MAX_STRING_BYTES};

/// Maximum nested states: a child of `<scxml>` is at depth 0, and lowering
/// rejects a state at depth greater than [`MAX_CONTENT_DEPTH`].
const MAX_STATE_NESTING: usize = MAX_CONTENT_DEPTH.saturating_add(1);
/// Maximum `LIMIT005` findings per machine.
const MAX_BOUND_FINDINGS: usize = 16;

/// First level of inline XML (1 for a direct child) that lowering rejects
/// inside `<data>`, `<content>`, or `<assign>` (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
const MAX_BODY_LEVEL: usize = scxml_types::wasm_bound(33);
/// First level below a custom action (its direct children are level 1)
/// that lowering rejects (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
const MAX_CUSTOM_LEVEL: usize = scxml_types::wasm_bound(32);

/// Whether `child` is no SCXML element: lowering takes only SCXML children
/// where this is asked, and skips any other (an element in another
/// namespace is `crate::foreign_namespaces`' `LINT022` or its package's; one
/// in no namespace is the schema's).
fn foreign_child(child: &Element) -> bool {
    child.name.namespace_uri.as_deref() != Some(SCXML_NAMESPACE)
}

const STATE_ELEMENTS: [&str; 4] = ["state", "parallel", "final", "history"];

/// `SCXML064`-`SCXML067` and `LIMIT005`: structures the compiler cannot lower.
pub(crate) struct LoweringRule;

impl Rule for LoweringRule {
    fn code(&self) -> &'static str {
        "SCXML064"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        for machine in machine_roots(&document.root) {
            let mut walk = Walk {
                document,
                findings: Vec::new(),
                bound_findings: 0,
                multi_target_items: 0,
                multi_target_reported: false,
                target_references: 0,
                target_findings: Vec::new(),
            };
            walk.machine(machine);
            if !scxml_types::exceeds_wasm_bound(walk.target_references, crate::MAX_GRAPH_TARGETS) {
                walk.findings.append(&mut walk.target_findings);
            }
            for finding in walk.findings {
                report.push(finding);
            }
        }
        extension_namespace_bounds(document, report);
    }
}

/// Where inline XML lowers, which fixes its nesting bounds.
#[derive(Clone, Copy)]
struct Body {
    /// First element level lowering rejects.
    max_level: usize,
    /// Machine IR validation depth of a level-0 node, when the body is
    /// validated inside executable content (`<send><content>`, a custom
    /// action): a node at level `k` fails when `offset + k` exceeds 64.
    ir_offset: Option<usize>,
}

struct Walk<'d> {
    document: &'d Document,
    findings: Vec<Diagnostic>,
    bound_findings: usize,
    multi_target_items: usize,
    multi_target_reported: bool,
    /// Tokens of every `target` and `initial` list of the machine.
    target_references: usize,
    /// `LIMIT005` findings about target lists, dropped when the machine is
    /// over [`crate::MAX_GRAPH_TARGETS`] (then `LIMIT002`).
    target_findings: Vec<Diagnostic>,
}

impl<'d> Walk<'d> {
    // -----------------------------------------------------------------------
    // Structure
    // -----------------------------------------------------------------------

    fn machine(&mut self, machine: &'d Element) {
        self.scxml_element(machine);
        self.child_count(machine, "child nodes");
        if let Some(initial) = machine.attribute("initial") {
            self.target_set(initial, "`initial`");
        }
        let mut data = 0_usize;
        let mut has_state = false;
        for child in child_elements(machine) {
            if foreign_child(child) {
                continue;
            }
            match child.name.local_name.as_str() {
                name if STATE_ELEMENTS.contains(&name) => {
                    has_state = true;
                    self.state(child, 0);
                }
                "datamodel" => self.datamodel(child, &mut data),
                "script" => self.action(child, 0),
                _ => {}
            }
        }
        if !has_state {
            self.findings
                .push(no_state_diagnostic(self.document, machine));
        }
    }

    fn state(&mut self, state: &'d Element, depth: usize) {
        if scxml_types::exceeds_wasm_bound(depth, MAX_CONTENT_DEPTH) {
            self.bound(
                state.start_tag_span,
                format!(
                    "`<{}>` is nested {} states deep; the compiler lowers at most {MAX_STATE_NESTING} nested states",
                    state.name.local_name,
                    depth + 1
                ),
                "flatten the state hierarchy, or move a deep part into a machine it invokes",
            );
            return;
        }
        self.scxml_element(state);
        self.child_count(state, "child nodes");
        if let Some(initial) = state.attribute("initial")
            && is_scxml(state, "state")
        {
            self.target_set(initial, "`initial`");
        }
        let mut data = 0_usize;
        let mut has_state = false;
        let mut initials: Vec<&Element> = Vec::new();
        for child in child_elements(state) {
            if foreign_child(child) {
                continue;
            }
            match child.name.local_name.as_str() {
                name if STATE_ELEMENTS.contains(&name) => {
                    has_state = true;
                    self.state(child, depth + 1);
                }
                "initial" => {
                    initials.push(child);
                    self.initial(child);
                }
                "transition" => self.transition(child),
                "onentry" | "onexit" => {
                    self.scxml_element(child);
                    self.actions(child, 0);
                }
                "datamodel" => self.datamodel(child, &mut data),
                "invoke" => self.invoke(child),
                "donedata" => self.donedata(child),
                _ => {}
            }
        }
        if is_scxml(state, "parallel") && !has_state {
            self.findings
                .push(no_state_diagnostic(self.document, state));
        }
        if let [first, extra @ ..] = initials.as_slice()
            && !extra.is_empty()
        {
            self.findings.push(repeated_child_diagnostic(
                self.document,
                state,
                first,
                extra,
                "SCXML 1.0 §3.3 <state>",
            ));
        }
    }

    fn initial(&mut self, initial: &'d Element) {
        self.scxml_element(initial);
        self.child_count(initial, "child nodes");
        for child in child_elements(initial) {
            if foreign_child(child) || !is_scxml(child, "transition") {
                continue;
            }
            self.transition(child);
        }
    }

    fn transition(&mut self, transition: &'d Element) {
        self.scxml_element(transition);
        if let Some(target) = transition.attribute("target") {
            self.target_set(target, "`target`");
        }
        if let Some(event) = transition.attribute("event") {
            self.token_count(event, "event descriptors");
        }
        self.actions(transition, 0);
    }

    fn datamodel(&mut self, datamodel: &'d Element, data: &mut usize) {
        self.scxml_element(datamodel);
        self.child_count(datamodel, "child nodes");
        for child in child_elements(datamodel) {
            if foreign_child(child) || !is_scxml(child, "data") {
                continue;
            }
            *data += 1;
            if *data == MAX_NESTED_ITEMS.saturating_add(1) {
                self.bound(
                    child.start_tag_span,
                    format!(
                        "this is the {}th `<data>` of its scope; the compiler lowers at most {MAX_NESTED_ITEMS} per `<scxml>` or state",
                        MAX_NESTED_ITEMS.saturating_add(1)
                    ),
                    "move some variables into a nested state's `<datamodel>` or combine them into structured values",
                );
            }
            self.scxml_element(child);
            self.body(
                child,
                Body {
                    max_level: MAX_BODY_LEVEL,
                    ir_offset: None,
                },
            );
        }
    }

    fn invoke(&mut self, invoke: &'d Element) {
        self.scxml_element(invoke);
        self.child_count(invoke, "child nodes");
        if let Some(namelist) = invoke.attribute("namelist") {
            self.token_count(namelist, "names");
        }
        let mut contents: Vec<&Element> = Vec::new();
        let mut finalizes: Vec<&Element> = Vec::new();
        for child in child_elements(invoke) {
            if foreign_child(child) {
                continue;
            }
            match child.name.local_name.as_str() {
                "param" => self.scxml_element(child),
                "content" => {
                    contents.push(child);
                    self.scxml_element(child);
                    self.body(
                        child,
                        Body {
                            max_level: MAX_BODY_LEVEL,
                            ir_offset: None,
                        },
                    );
                }
                "finalize" => {
                    finalizes.push(child);
                    self.scxml_element(child);
                    self.actions(child, 0);
                }
                _ => {}
            }
        }
        for repeated in [contents, finalizes] {
            if let [first, extra @ ..] = repeated.as_slice()
                && !extra.is_empty()
            {
                self.findings.push(repeated_child_diagnostic(
                    self.document,
                    invoke,
                    first,
                    extra,
                    "SCXML 1.0 §6.4 <invoke>",
                ));
            }
        }
    }

    fn donedata(&mut self, donedata: &'d Element) {
        self.scxml_element(donedata);
        self.child_count(donedata, "child nodes");
        for child in child_elements(donedata) {
            // `<donedata>` admits only `<content>` and `<param>` (no
            // `##other` wildcard), so the W3C schema rejects an element
            // from another namespace here.
            if child.name.namespace_uri.as_deref() != Some(SCXML_NAMESPACE) {
                continue;
            }
            self.scxml_element(child);
            if is_scxml(child, "content") {
                self.body(
                    child,
                    Body {
                        max_level: MAX_BODY_LEVEL,
                        ir_offset: None,
                    },
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Executable content
    // -----------------------------------------------------------------------

    /// The executable-content children of `owner`, a list at `depth`.
    fn actions(&mut self, owner: &'d Element, depth: usize) {
        self.child_count(owner, "child nodes");
        for child in child_elements(owner) {
            self.action(child, depth);
        }
    }

    /// One action in a list at `depth` (the `<if>`/`<foreach>` levels
    /// around it).
    fn action(&mut self, action: &'d Element, depth: usize) {
        if action.name.namespace_uri.as_deref() != Some(SCXML_NAMESPACE) {
            if action.name.namespace_uri.is_some() {
                self.custom(action, depth);
            }
            return;
        }
        self.scxml_element(action);
        match action.name.local_name.as_str() {
            "assign" => {
                self.findings
                    .extend(assign_diagnostics(self.document, action));
                self.empty_required(action, "location");
                self.assign_body(action);
            }
            "if" => {
                self.empty_required(action, "cond");
                if self.nested_too_deep(action, depth) {
                    return;
                }
                self.if_branches(action, depth);
            }
            "foreach" => {
                self.empty_required(action, "array");
                self.empty_required(action, "item");
                if self.nested_too_deep(action, depth) {
                    return;
                }
                self.actions(action, depth + 1);
            }
            "send" => self.send(action, depth),
            "script" => {
                let body: usize = action
                    .children
                    .iter()
                    .map(|node| match node {
                        Node::Text(text) => text.value.len(),
                        Node::Element(_) => 0,
                    })
                    .sum();
                if scxml_types::exceeds_wasm_bound(body, MAX_STRING_BYTES) {
                    self.bound(
                        action.start_tag_span,
                        format!(
                            "this `<script>` body is {body} bytes; the compiler lowers at most {MAX_STRING_BYTES} bytes of script"
                        ),
                        "split the script into several `<script>` elements, or load it with `src`",
                    );
                }
            }
            _ => {}
        }
    }

    /// Whether an `<if>`/`<foreach>` in a list at `depth` opens a list
    /// deeper than Machine IR allows (reported once).
    fn nested_too_deep(&mut self, action: &Element, depth: usize) -> bool {
        if depth < MAX_CONTENT_DEPTH {
            return false;
        }
        self.bound(
            action.start_tag_span,
            format!(
                "this `<{}>` is nested {} `<if>`/`<foreach>` levels deep; the compiler lowers at most {MAX_CONTENT_DEPTH}",
                action.name.local_name,
                depth + 1
            ),
            "flatten the nested conditions and loops, for example with `<elseif>` or a helper state",
        );
        true
    }

    fn if_branches(&mut self, action: &'d Element, depth: usize) {
        let mut branch = 0_usize;
        let mut else_ifs = 0_usize;
        let mut reported = false;
        for child in child_elements(action) {
            if is_scxml(child, "elseif") || is_scxml(child, "else") {
                self.scxml_element(child);
                if is_scxml(child, "elseif") {
                    self.empty_required(child, "cond");
                    else_ifs += 1;
                    if else_ifs == MAX_NESTED_ITEMS.saturating_add(1) {
                        self.bound(
                            child.start_tag_span,
                            format!(
                                "this `<if>` has more than {MAX_NESTED_ITEMS} `<elseif>` branches, the most the compiler lowers"
                            ),
                            "split the conditional, or dispatch with a data-model lookup",
                        );
                    }
                }
                branch = 0;
                continue;
            }
            branch += 1;
            if branch == MAX_NESTED_ITEMS.saturating_add(1) && !reported {
                reported = true;
                self.bound(
                    child.start_tag_span,
                    format!(
                        "this `<if>` branch has more than {MAX_NESTED_ITEMS} actions, the most the compiler lowers per branch"
                    ),
                    "move the actions into several blocks or states",
                );
            }
            self.action(child, depth + 1);
        }
    }

    fn send(&mut self, send: &'d Element, depth: usize) {
        self.child_count(send, "child nodes");
        if let Some(namelist) = send.attribute("namelist") {
            self.token_count(namelist, "names");
        }
        let mut contents: Vec<&Element> = Vec::new();
        for child in child_elements(send) {
            if foreign_child(child) {
                continue;
            }
            self.scxml_element(child);
            if is_scxml(child, "content") {
                contents.push(child);
                self.body(
                    child,
                    Body {
                        max_level: MAX_BODY_LEVEL,
                        ir_offset: Some(depth),
                    },
                );
            }
        }
        if let [first, extra @ ..] = contents.as_slice()
            && !extra.is_empty()
        {
            self.findings.push(repeated_child_diagnostic(
                self.document,
                send,
                first,
                extra,
                "SCXML 1.0 §6.2 <send>",
            ));
        }
    }

    /// A custom action (an element in another namespace) in a list at
    /// `depth`; it lowers as XML.
    fn custom(&mut self, custom: &'d Element, depth: usize) {
        let body = Body {
            max_level: MAX_CUSTOM_LEVEL,
            ir_offset: Some(depth + 1),
        };
        self.xml_element(custom, 0, body);
    }

    fn assign_body(&mut self, assign: &'d Element) {
        let significant = assign
            .children
            .iter()
            .filter(|node| match node {
                Node::Text(text) => !scxml_syntax::is_xml_blank(&text.value),
                Node::Element(_) => true,
            })
            .count();
        if scxml_types::exceeds_wasm_bound(significant, MAX_NESTED_ITEMS) {
            self.bound(
                assign.start_tag_span,
                format!(
                    "this `<assign>` value has {significant} nodes; the compiler lowers at most {MAX_NESTED_ITEMS}"
                ),
                "compute the value with `expr`, or split it",
            );
        }
        let body = Body {
            max_level: MAX_BODY_LEVEL,
            ir_offset: None,
        };
        for node in &assign.children {
            match node {
                Node::Element(child) => self.xml_element(child, 1, body),
                Node::Text(text) => self.xml_text(text.span, text.value.len(), 1, body),
            }
        }
    }

    // -----------------------------------------------------------------------
    // Inline XML
    // -----------------------------------------------------------------------

    /// The child nodes of a `<data>` or `<content>`, which lower as XML.
    fn body(&mut self, owner: &'d Element, body: Body) {
        self.xml_children(owner, 1, body);
    }

    fn xml_children(&mut self, owner: &'d Element, level: usize, body: Body) {
        self.child_count(owner, "child nodes");
        for node in &owner.children {
            match node {
                Node::Element(child) => self.xml_element(child, level, body),
                Node::Text(text) => self.xml_text(text.span, text.value.len(), level, body),
            }
        }
    }

    fn xml_element(&mut self, element: &'d Element, level: usize, body: Body) {
        let lowering = level >= body.max_level && level > 0;
        let validation = body.ir_offset.is_some_and(|offset| {
            scxml_types::exceeds_wasm_bound(offset.saturating_add(level), MAX_CONTENT_DEPTH)
        });
        if lowering || validation {
            self.bound(
                element.start_tag_span,
                format!(
                    "`<{}>` is nested {level} elements deep in lowered XML; the compiler lowers at most {} levels here",
                    element.lexical_name,
                    if lowering {
                        body.max_level - 1
                    } else {
                        MAX_CONTENT_DEPTH.saturating_sub(body.ir_offset.unwrap_or(0))
                    }
                ),
                "flatten the XML value, or load it with `src`/`expr` instead of inlining it",
            );
            return;
        }
        if scxml_types::exceeds_wasm_bound(element.attributes.len(), MAX_NESTED_ITEMS) {
            self.bound(
                element.start_tag_span,
                format!(
                    "`<{}>` has {} attributes; the compiler lowers at most {MAX_NESTED_ITEMS} per element",
                    element.lexical_name,
                    element.attributes.len()
                ),
                "move the values into child elements",
            );
        }
        self.string_bound(
            element.start_tag_span,
            element.name.local_name.len(),
            "element name",
        );
        if let Some(namespace) = &element.name.namespace_uri {
            self.string_bound(element.start_tag_span, namespace.len(), "namespace URI");
        }
        for attribute in &element.attributes {
            self.string_bound(
                attribute.value_span,
                attribute.value.len(),
                "attribute value",
            );
            self.string_bound(
                attribute.span,
                attribute.name.local_name.len(),
                "attribute name",
            );
        }
        self.xml_children(element, level + 1, body);
    }

    fn xml_text(&mut self, span: Span, length: usize, level: usize, body: Body) {
        if body.ir_offset.is_some_and(|offset| {
            scxml_types::exceeds_wasm_bound(offset.saturating_add(level), MAX_CONTENT_DEPTH)
        }) {
            self.bound(
                span,
                format!(
                    "this text is nested {level} levels deep in lowered XML; the compiler lowers at most {} levels here",
                    MAX_CONTENT_DEPTH.saturating_sub(body.ir_offset.unwrap_or(0))
                ),
                "flatten the XML value",
            );
            return;
        }
        self.string_bound(span, length, "text run");
    }

    // -----------------------------------------------------------------------
    // Shared checks
    // -----------------------------------------------------------------------

    /// The attribute-count and 1 MiB value bounds (`LIMIT005`) every
    /// lowered SCXML element gets. What an attribute in another namespace
    /// means is `crate::foreign_namespaces`' and the vocabularies' rules.
    fn scxml_element(&mut self, element: &Element) {
        let count = element.attributes.len();
        if scxml_types::exceeds_wasm_bound(count, MAX_NESTED_ITEMS) {
            self.bound(
                element.start_tag_span,
                format!(
                    "`<{}>` has {count} attributes; the compiler lowers at most {MAX_NESTED_ITEMS} per element",
                    element.lexical_name
                ),
                "remove the attributes the element does not need",
            );
        }
        for attribute in &element.attributes {
            // The value lowering stores.
            let stored = schema_value(element, attribute).len();
            self.string_bound(attribute.value_span, stored, "attribute value");
        }
    }

    fn empty_required(&mut self, element: &Element, name: &str) {
        if let Some(attribute) = element.attribute(name)
            && attribute.value.is_empty()
        {
            self.findings
                .push(empty_required_diagnostic(element, attribute));
        }
    }

    fn child_count(&mut self, element: &Element, what: &str) {
        let count = element.children.len();
        if scxml_types::exceeds_wasm_bound(count, MAX_NESTED_ITEMS) {
            self.bound(
                element.start_tag_span,
                format!(
                    "`<{}>` has {count} {what}; the compiler lowers at most {MAX_NESTED_ITEMS} per element",
                    element.lexical_name
                ),
                "split the element's children across several elements or states",
            );
        }
    }

    fn token_count(&mut self, attribute: &Attribute, what: &str) {
        let count = xml_tokens(&attribute.value).count();
        if scxml_types::exceeds_wasm_bound(count, MAX_NESTED_ITEMS) {
            self.bound(
                attribute.value_span,
                format!(
                    "`{}` lists {count} {what}; the compiler lowers at most {MAX_NESTED_ITEMS}",
                    attribute.lexical_name
                ),
                "shorten the list, for example with a wildcard or prefix event descriptor",
            );
        }
    }

    /// A target list (`target` or `initial`): at most 10,000 tokens, and
    /// multi-target sets share a 100,000-target budget per machine.
    fn target_set(&mut self, attribute: &Attribute, what: &str) {
        let count = xml_tokens(&attribute.value).count();
        self.target_references = self.target_references.saturating_add(count);
        if scxml_types::exceeds_wasm_bound(count, MAX_NESTED_ITEMS) {
            self.target_bound(
                attribute.value_span,
                format!(
                    "{what} lists {count} targets; the compiler lowers at most {MAX_NESTED_ITEMS} per transition"
                ),
                "target fewer states, or enter them through their parent's default entry",
            );
        }
        if count >= 2 {
            self.multi_target_items = self.multi_target_items.saturating_add(count);
            if scxml_types::exceeds_wasm_bound(self.multi_target_items, MAX_MULTI_TARGET_ITEMS)
                && !self.multi_target_reported
            {
                self.multi_target_reported = true;
                self.target_bound(
                    attribute.value_span,
                    format!(
                        "the machine's multi-target sets reach {} targets here; the compiler checks at most {MAX_MULTI_TARGET_ITEMS} per machine",
                        self.multi_target_items
                    ),
                    "target fewer states per transition, or split the machine",
                );
            }
        }
    }

    fn string_bound(&mut self, span: Span, length: usize, what: &str) {
        if scxml_types::exceeds_wasm_bound(length, MAX_STRING_BYTES) {
            self.bound(
                span,
                format!(
                    "this {what} is {length} bytes; the compiler lowers at most {MAX_STRING_BYTES} bytes per string"
                ),
                "shorten it, or load large data with `src`",
            );
        }
    }

    /// Records one `LIMIT005` finding, at most [`MAX_BOUND_FINDINGS`].
    fn bound(&mut self, span: Span, message: String, help: &str) {
        if self.bound_findings == MAX_BOUND_FINDINGS {
            return;
        }
        self.bound_findings += 1;
        self.findings.push(bound_diagnostic(span, message, help));
    }

    /// [`Self::bound`] for a target-list bound; kept apart so a machine the
    /// semantic graph already reports as over its target-reference bound
    /// (`LIMIT002`) is not reported twice for the same references.
    fn target_bound(&mut self, span: Span, message: String, help: &str) {
        if self.bound_findings == MAX_BOUND_FINDINGS {
            return;
        }
        self.bound_findings += 1;
        self.target_findings
            .push(bound_diagnostic(span, message, help));
    }
}

fn bound_diagnostic(span: Span, message: String, help: &str) -> Diagnostic {
    Diagnostic::error("LIMIT005", message, span)
        .with_primary_message("over the Machine IR bound")
        .with_note("Machine IR has fixed bounds so lowering and execution stay bounded (65 nested states, 64 nested `<if>`/`<foreach>`, 10,000 children or list items per element or scope, 1 MiB per string); `compile` rejects a document over them, and they are far above any hand-written machine")
        .with_help(help)
}

/// `LIMIT005` for more than 10,000 distinct extension namespaces, or a used
/// namespace URI over 1 MiB (the compiler records each one).
fn extension_namespace_bounds(document: &Document, report: &mut Report) {
    let used: BTreeSet<String> = document
        .used_namespaces()
        .into_iter()
        .filter(|namespace| {
            namespace != SCXML_NAMESPACE && namespace != "http://www.w3.org/XML/1998/namespace"
        })
        .collect();
    if scxml_types::exceeds_wasm_bound(used.len(), MAX_NESTED_ITEMS) {
        report.push(bound_diagnostic(
            document.root.start_tag_span,
            format!(
                "the document uses {} extension namespaces; the compiler records at most {MAX_NESTED_ITEMS}",
                used.len()
            ),
            "use fewer extension namespaces",
        ));
    }
    if used
        .iter()
        .any(|namespace| scxml_types::exceeds_wasm_bound(namespace.len(), MAX_STRING_BYTES))
    {
        report.push(bound_diagnostic(
            document.root.start_tag_span,
            format!(
                "the document uses a namespace URI longer than {MAX_STRING_BYTES} bytes, the most the compiler records"
            ),
            "use a shorter namespace URI",
        ));
    }
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

/// `SCXML064`: `extra` repeat `first`, a child `parent` allows once.
fn repeated_child_diagnostic(
    document: &Document,
    parent: &Element,
    first: &Element,
    extra: &[&Element],
    spec: &str,
) -> Diagnostic {
    let child = first.name.local_name.as_str();
    let parent_name = parent.name.local_name.as_str();
    let Some((second, rest)) = extra.split_first() else {
        return Diagnostic::error("SCXML064", "", first.start_tag_span);
    };
    let (note, help) = match (parent_name, child) {
        ("send", _) => (
            "`<send>` carries at most one `<content>`: it is the whole event payload, so a second one has no meaning",
            "keep the one `<content>` that is the payload; to send several values, use `<param>` children or one structured `<content>`",
        ),
        ("invoke", "content") => (
            "`<invoke>` takes at most one inline `<content>`: it is the one document the invoked service runs",
            "keep the one `<content>` that is the service's document",
        ),
        ("invoke", _) => (
            "`<invoke>` takes at most one `<finalize>`: it is the one block that processes each event the invoked service returns",
            "merge the executable content into one `<finalize>`",
        ),
        _ => (
            "a `<state>` has at most one `<initial>`: it is the one default entry transition of the state",
            "keep one `<initial>`; to enter several parallel descendants, list them all as targets of its one `<transition>`",
        ),
    };
    let count = extra.len() + 1;
    let mut diagnostic = Diagnostic::error(
        "SCXML064",
        format!("`<{parent_name}>` has {count} `<{child}>` children, but allows at most one"),
        second.start_tag_span,
    )
    .with_primary_message(format!("second `<{child}>`"))
    .with_secondary(Label::new(first.start_tag_span).with_message(format!("first `<{child}>`")));
    for element in rest {
        diagnostic = diagnostic.with_secondary(
            Label::new(element.start_tag_span).with_message(format!("another `<{child}>`")),
        );
    }
    diagnostic
        .with_note(note)
        .with_help(help)
        .with_fix(Fix {
            message: if extra.len() == 1 {
                format!("remove the second `<{child}>`")
            } else {
                format!("remove the {} extra `<{child}>` elements", extra.len())
            },
            applicability: Applicability::MaybeIncorrect,
            edits: extra
                .iter()
                .map(|element| FixEdit {
                    span: crate::graph::removal_span(document, element.span),
                    replacement: String::new(),
                })
                .collect(),
        })
        .with_spec_reference(spec)
}

/// The schema's value of `attribute` on the SCXML element `element`: its
/// collapsed form when its type collapses white space (XML Schema Part 2
/// §4.3.6), as lowering reads it. [`LoweringRule`] walks the document as
/// written so that its bounds measure the stored bytes, so every value it
/// compares goes through this.
fn schema_value<'a>(element: &Element, attribute: &'a Attribute) -> std::borrow::Cow<'a, str> {
    if attribute.name.namespace_uri.is_none()
        && crate::schema_values::collapses_whitespace(
            &element.name.local_name,
            &attribute.name.local_name,
        )
    {
        collapse_whitespace(&attribute.value)
    } else {
        std::borrow::Cow::Borrowed(attribute.value.as_str())
    }
}

/// `SCXML065`: the value forms of one `<assign>`.
fn assign_diagnostics(document: &Document, assign: &Element) -> Vec<Diagnostic> {
    const SPEC_ASSIGN: &str = "SCXML 1.0 §5.4 <assign>";
    const SPEC_XPATH_ASSIGN: &str = "SCXML CR 2014 §C.3.6 <assign>";
    let mut findings = Vec::new();
    let kind = assign.attribute("type");
    let attr = assign.attribute("attr");
    let expr = assign.attribute("expr");
    let body = assign.children.iter().find_map(|node| match node {
        Node::Element(child) => Some(child.span),
        Node::Text(text) => (!scxml_syntax::is_xml_blank(&text.value)).then_some(text.span),
    });
    let kind_value = kind.map(|kind| schema_value(assign, kind));
    let add_attribute = kind_value.as_deref() == Some("addattribute");
    let delete = kind_value.as_deref() == Some("delete");
    if let (Some(kind), None) = (kind.filter(|_| add_attribute), attr) {
        findings.push(
            Diagnostic::error(
                "SCXML065",
                "`<assign type=\"addattribute\">` names no attribute: `attr` is missing",
                kind.value_span,
            )
            .with_primary_message("adds an attribute")
            .with_note("`type=\"addattribute\"` adds an attribute to the selected element, so `attr` must name it; the value comes from `expr` or the child content")
            .with_help("add `attr=\"name\"` with the attribute to add")
            .with_fix(Fix::replace(
                "add the `attr` to set",
                Applicability::HasPlaceholders,
                attribute_insertion_point(document, assign),
                " attr=\"…\"",
            ))
            .with_spec_reference(SPEC_XPATH_ASSIGN),
        );
    }
    if let (Some(attr), false) = (attr, add_attribute) {
        let mut diagnostic = Diagnostic::error(
            "SCXML065",
            "`attr` on `<assign>` is used only with `type=\"addattribute\"`",
            attr.span,
        )
        .with_primary_message("attribute name without `addattribute`")
        .with_note("only `type=\"addattribute\"` adds an attribute named by `attr`; every other placement replaces or inserts nodes, so `attr` would be ignored")
        .with_help("remove `attr`, or set `type=\"addattribute\"` to add that attribute")
        .with_fix(Fix::replace(
            "remove `attr`",
            Applicability::MaybeIncorrect,
            attribute_removal_span(document, attr),
            "",
        ))
        .with_spec_reference(SPEC_XPATH_ASSIGN);
        if let Some(kind) = kind {
            diagnostic = diagnostic
                .with_secondary(Label::new(kind.span).with_message("the placement is set here"));
        }
        findings.push(diagnostic);
    }
    // SCXML CR 2014 §C.3.6: "delete. Delete the node specified by
    // 'location'. ('expr' is ignored.)"; SCXML 1.0 §5.4.2 still asks a
    // conformant document for "either "expr" or children", so a value on a
    // deletion is conformant and is not reported.
    if !delete && expr.is_none() && body.is_none() {
        findings.push(
            Diagnostic::error("SCXML065", "`<assign>` has no value", assign.start_tag_span)
                .with_primary_message("neither `expr` nor child content")
                .with_note("`<assign>` stores a value at `location`, taken from `expr` or from its child content; without either there is nothing to assign")
                .with_help("add `expr` with the value to assign, or give the value as child content")
                .with_fix(Fix::replace(
                    "add the `expr` to assign",
                    Applicability::HasPlaceholders,
                    attribute_insertion_point(document, assign),
                    " expr=\"…\"",
                ))
                .with_spec_reference(SPEC_ASSIGN),
        );
    }
    findings
}

/// `SCXML066`: a required expression attribute that is empty.
fn empty_required_diagnostic(element: &Element, attribute: &Attribute) -> Diagnostic {
    let name = attribute.name.local_name.as_str();
    let local = element.name.local_name.as_str();
    let (what, spec) = match (local, name) {
        ("if" | "elseif", _) => ("the condition of this branch", "SCXML 1.0 §4.3 <if>"),
        ("foreach", "array") => ("the collection to iterate", "SCXML 1.0 §4.6 <foreach>"),
        ("foreach", _) => ("the loop variable", "SCXML 1.0 §4.6 <foreach>"),
        _ => ("the location to assign", "SCXML 1.0 §5.4 <assign>"),
    };
    Diagnostic::warning(
        "SCXML066",
        format!("`{name}` on `<{local}>` is empty"),
        attribute.span,
    )
    .with_primary_message(format!("`{name}` must name {what}"))
    .with_note(format!(
        "`{name}` is required on `<{local}>`; an empty value is no expression at all, so every time it runs it raises `error.execution` (SCXML 1.0 §5.9.4: the processor \"MAY wait and place 'error.execution' in the internal event queue at runtime when the expressions are evaluated\")"
    ))
    .with_help(if local == "elseif" || local == "if" {
        format!("write the condition this `<{local}>` tests; a last branch that always runs is written as `<else/>`")
    } else {
        format!("write {what} in `{name}`")
    })
    .with_fix(Fix::replace(
        format!("fill in `{name}`"),
        Applicability::HasPlaceholders,
        attribute.value_span,
        "…",
    ))
    .with_spec_reference(spec)
}

/// `SCXML067`: an `<scxml>` without a child state (SCXML 1.0 §3.2.2: "A
/// conformant SCXML document MUST have at least one <state>, <parallel> or
/// <final> child"). `LINT021` (warning): a `<parallel>` without a child
/// state, which SCXML 1.0 §3.4.2 allows ("Occurs 0 or more times") but
/// which is never atomic, so no transition is ever enabled through it.
fn no_state_diagnostic(document: &Document, element: &Element) -> Diagnostic {
    if is_scxml(element, "parallel") {
        let diagnostic = Diagnostic::warning(
            "LINT021",
            "`<parallel>` has no child states",
            element.start_tag_span,
        )
        .with_primary_message("no transition is enabled through this state")
        .with_note("SCXML 1.0 §3.4.2 allows a `<parallel>` with no `<state>` children, but only atomic states enable transitions (§3.13: \"enabled by named event E in atomic state S\"), and a `<parallel>` is never atomic (§3.3: \"An atomic state is a <state> that has no <state>, <parallel> or <final> children\"); while it is active, its own transitions are never taken, its ancestors' only through other active atomic states, and it never raises `done.state`")
        .with_help("give the `<parallel>` its regions as child `<state>` elements, or make it an atomic `<state>`")
        .with_spec_reference("SCXML 1.0 §3.4 <parallel>");
        return match crate::graph::rename_element_edits(document, element, "state") {
            Some(edits) => diagnostic.with_fix(Fix {
                message: "change `<parallel>` to `<state>`".to_owned(),
                applicability: Applicability::MaybeIncorrect,
                edits,
            }),
            None => diagnostic,
        };
    }
    Diagnostic::error("SCXML067", "`<scxml>` has no states", element.start_tag_span)
        .with_primary_message("a machine needs at least one state")
        .with_note("a machine starts by entering its initial state, so without a `<state>`, `<parallel>`, or `<final>` child there is nothing to run")
        .with_help("add the machine's states, for example `<final id=\"done\"/>`")
        .with_spec_reference("SCXML 1.0 §3.2 <scxml>")
}
