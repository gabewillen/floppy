use std::borrow::Cow;

use scxml_ioprocessor_scxml::{ProcessorKind, classify_processor_type};
use scxml_syntax::{Document, Element, collapse_whitespace, xml_tokens};

use crate::manifest::{ExpectedOutcome, LoadedCase};
use crate::report::Stage;

const SCXML_NAMESPACE: &str = "http://www.w3.org/2005/07/scxml";

#[derive(Clone, Debug, Eq, PartialEq)]
/// A bounded explanation of a capability that prevents case execution.
pub struct UnsupportedFeature {
    /// Pipeline stage at which the unsupported capability was detected.
    pub stage: Stage,
    /// Stable machine-readable reason code.
    pub code: &'static str,
    /// Human-readable explanation that does not contain source payloads.
    pub message: String,
}

/// Detects whether a loaded case requires behavior outside the executable slice.
///
/// An element of an extension namespace in `bound` (the namespaces the
/// executor binds an extension component for) is executable content its
/// component runs (SCXML 1.0 section 4.10); its children are that
/// component's content. Every other extension namespace is unsupported.
///
/// Returns the first deterministic unsupported-feature disposition, or `None`
/// when the document can be attempted by the current compiler and engine.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "one ordered gate walk keeps every capability disposition and its precedence together"
)]
pub fn detect(
    case: &LoadedCase,
    document: &Document,
    bound: &std::collections::BTreeSet<&str>,
) -> Option<UnsupportedFeature> {
    if !matches!(
        case.manifest.datamodel.as_str(),
        "ecmascript" | "xpath" | "null"
    ) {
        return unsupported(
            Stage::Capability,
            "unsupported-case-data-model",
            "conformance executes only through the ECMAScript, XPath, and null data-model components",
        );
    }
    // The document's own `datamodel` selects the executing data model
    // (W3C `B.2` ECMAScript cases appear in every suite). Its default is
    // "platform-specific" (SCXML 1.0 3.2.1) and this platform's default
    // is ECMAScript, the data model lowering records for a document that
    // declares none. Both expression data models represent XML-element
    // payloads and initializers as XML values: XPath as node-sets (SCXML
    // 1.0 CR 2014-03-13 §C.3.1), ECMAScript as a read-only DOM (B.2.1).
    // `datamodel` (`xsd:NMTOKEN`) and `binding` (restricted from it)
    // collapse white space (XML Schema Part 2 §4.3.6): the schema's value
    // is the collapsed one, as lowering reads it.
    let document_data_model = document
        .root
        .attribute("datamodel")
        .map_or(Cow::Borrowed("ecmascript"), |value| {
            collapse_whitespace(&value.value)
        });
    let xml_values = if matches!(&*document_data_model, "ecmascript" | "xpath") {
        XmlValues::Represented
    } else {
        XmlValues::Absent
    };
    if let Some(binding) = document.root.attribute("binding")
        && !matches!(&*collapse_whitespace(&binding.value), "early" | "late")
    {
        return unsupported(
            Stage::Capability,
            "unsupported-data-binding",
            "the executable data declaration slice supports only early and late binding",
        );
    }
    // A declared data model is not itself executable behavior. Fail closed on
    // expressions and data-model elements below, while allowing structural IRP
    // cases that declare ecmascript but never evaluate it.
    // Documented manual cases are decided by their manual-equivalent
    // oracle (`crate::manual_oracle`), not by an `Outcome` log, so the
    // Outcome-reachability gates below do not apply to them.
    let requires_outcome = matches!(
        case.manifest.expected,
        ExpectedOutcome::W3cLogOutcome { .. }
    ) && crate::manual_oracle::ManualOracle::for_case(&case.manifest)
        .is_none();
    let mut result = None;
    let mut requires_data_model = requires_outcome;
    // Direct send payload children are classified by
    // `send_payload_issue`; the blanket data-model gate must not fire
    // for them. The traversal is pre-order without exit callbacks, so
    // parents are recorded by child element identity (its unique
    // source span) instead of a stack.
    let mut payload_children: std::collections::BTreeSet<scxml_syntax::Span> =
        std::collections::BTreeSet::new();
    // CR 2014-03-13 §C.3.1 (XPath) and B.2.1 (ECMAScript): markup inside
    // `<data>`, `<assign>`, and payload
    // `<content>` is a data value (an XPath node-set or an ECMAScript
    // DOM, including `xmlns=""` content), not executable content,
    // so its elements are never gated as an extension component. An
    // `<invoke>` `<content>` stays executable: it is a child document.
    let mut invoke_contents: std::collections::BTreeSet<scxml_syntax::Span> =
        std::collections::BTreeSet::new();
    let mut inline_markup: std::collections::BTreeSet<scxml_syntax::Span> =
        std::collections::BTreeSet::new();
    // Loading external scripts is the first act of document load (SCXML
    // 1.0 section 5.8) and its outcome is decided only by the host's
    // script loader: an unfetchable source rejects the document before
    // any data model exists. The static data-model gate therefore defers
    // to execution, whose data-model initialization fails closed with
    // the same `unsupported-executable-data-model` code once every script
    // has loaded.
    let mut loads_external_scripts = false;
    document.root.visit(&mut |element| {
    if inline_markup.contains(&element.span) {
        return;
    }
    for child in &element.children {
        // Invoke payload children (`<content>`, `<param>`, `<finalize>`)
        // belong to the invoked-session slice, not the data-model gate
        // (SCXML 6.4).
        if matches!(element.name.local_name.as_str(), "send" | "donedata" | "invoke")
            && let scxml_syntax::Node::Element(child) = child
        {
            payload_children.insert(child.span);
            if element.name.local_name == "invoke" {
                invoke_contents.insert(child.span);
            }
        }
    }
    if xml_values == XmlValues::Represented
        && element.name.namespace_uri.as_deref() == Some(SCXML_NAMESPACE)
        && (matches!(element.name.local_name.as_str(), "data" | "assign")
            || (element.name.local_name == "content"
                && !invoke_contents.contains(&element.span)))
    {
        collect_descendant_elements(element, &mut inline_markup);
    }
    if result.is_some() {
        return;
    }
    if element
        .name
        .namespace_uri
        .as_deref()
        .is_some_and(|namespace| bound.contains(namespace))
    {
        // Its extension component evaluates through the data model.
        requires_data_model = true;
        collect_descendant_elements(element, &mut inline_markup);
        return;
    }
    if element.name.namespace_uri.as_deref() != Some(SCXML_NAMESPACE) {
        result = unsupported(
            Stage::Capability,
            "unsupported-extension-namespace",
            "extension component is not linked into the local IR adapter",
        );
        return;
    }
    result = match element.name.local_name.as_str() {
        "history" => history_issue(element),
        "invoke" => invoke_issue(element),
        "send" => {
            // SCXML C.2: a BasicHTTP send may omit the event name; the
            // receiving processor then constructs HTTP.POST (official
            // cases 520, 531, 532). Every other processor requires an
            // event name.
            let basichttp = element.attribute("type").is_some_and(|value| {
                classify_processor_type(value.value.as_str()) == ProcessorKind::BasicHttp
            });
            if element.attribute("event").is_none()
                && element.attribute("eventexpr").is_none()
                && !basichttp
            {
                requires_data_model = true;
                unsupported(
                    Stage::Capability,
                    "unsupported-send",
                    "send requires an `event` or `eventexpr` attribute",
                )
            } else {
                requires_data_model = true;
                send_issue(element, xml_values)
            }
        }
        "data" => data_declaration_issue(element, xml_values),
        "content" | "param"
            if payload_children.contains(&element.span) => None,
        "content" | "param" => unsupported(
            Stage::Capability,
            "unsupported-datamodel-operation",
            format!(
                "SCXML data-model element `{}` is parsed but not executable by the IR adapter",
                element.name.local_name
            ),
        ),
        "donedata" => donedata_payload_issue(element, xml_values),
        // `<cancel>` (SCXML 6.3) executes through the engine's Cancel
        // effect: literal `sendid` cancels directly, `sendidexpr` is
        // evaluated by the execution host, and illegal attribute
        // combinations raise `error.execution` at effect time.
        "foreach" => {
            requires_data_model = true;
            foreach_issue(element)
        }

        // `<script src>` resolves through the executor's host script
        // loader at document load (SCXML 1.0 section 5.8); an
        // unfetchable source rejects the document.
        "script" if element.attribute("src").is_some() => {
            loads_external_scripts = true;
            None
        }
        "script" => unsupported_if_has_element_children(
            element,
            "unsupported-script-content",
            "inline scripts must contain text only",
        ),
        "assign" => assign_issue(element),
        "if" | "log" => {
            requires_data_model = true;
            None
        }
        "onentry" | "onexit" if only_supported_children(element, bound) => None,
        "onentry" | "onexit" => unsupported_if_has_element_children(
            element,
            "unsupported-executable-content",
            "the official-corpus gate currently accepts ordered conditional log, raise, assign, script, if, and foreach content",
        ),
        // Compound states (nested state/final children) fall through to the
        // wildcard: they execute through the engine's ancestor-chain
        // selection, LCA domains, default initial descent, and done.state
        // raises. Parallel and history remain gated above by their own
        // element presence.
        "transition" => {
            if element.attribute("event").is_some_and(|events| {
                xml_tokens(&events.value).any(|event| {
                    matches!(event, "error.execution" | "error.communication")
                })
            }) {
                // Interpreter error delivery is implemented for both
                // event names; such transitions still require the data
                // model for guards.
                requires_data_model = true;
                transition_structure_issue(element, bound)
            } else if element.attribute("cond").is_some() {
                requires_data_model = true;
                transition_structure_issue(element, bound)
            } else {
                transition_structure_issue(element, bound)
            }
        }
        _ => None,
    };
});
    if result.is_none()
        && requires_data_model
        && !loads_external_scripts
        && !matches!(&*document_data_model, "null" | "ecmascript" | "xpath")
    {
        return unsupported(
            Stage::Capability,
            "unsupported-executable-data-model",
            "the supported guard and executable-content slice requires the null, ECMAScript, or XPath data model",
        );
    }
    if result.is_none() && requires_outcome && !contains_outcome_log(&document.root) {
        return unsupported(
            Stage::Oracle,
            "outcome-log-missing-source",
            "prepared official case contains no executable Outcome log",
        );
    }
    // Official oracle-contract reachability: the pass oracle requires an
    // `Outcome` log whose value can evaluate to `pass`. Manual
    // visual-inspection cases print diagnostics but can never log `pass`
    // (official case 178 has only an Outcome=`fail` final state), so the
    // oracle is unsatisfiable by construction and the case stays an
    // honest Unsupported rather than a false Failed.
    if result.is_none()
        && requires_outcome
        && matches!(
            case.manifest.expected,
            ExpectedOutcome::W3cLogOutcome {
                outcome: crate::manifest::LogOutcome::Pass,
            }
        )
        && !contains_outcome_pass_log(&document.root)
    {
        result = unsupported(
            Stage::Oracle,
            "outcome-pass-unreachable",
            "official case documents no Outcome log that can evaluate to pass",
        );
    }
    result
}

/// Records the span of every element below `element` (iterative; the parser
/// bounds document depth and size).
fn collect_descendant_elements(
    element: &Element,
    spans: &mut std::collections::BTreeSet<scxml_syntax::Span>,
) {
    let mut pending: Vec<&Element> = vec![element];
    while let Some(current) = pending.pop() {
        for child in &current.children {
            if let scxml_syntax::Node::Element(child) = child {
                spans.insert(child.span);
                pending.push(child);
            }
        }
    }
}

fn only_supported_children(element: &Element, bound: &std::collections::BTreeSet<&str>) -> bool {
    element.children.iter().all(|child| match child {
        scxml_syntax::Node::Text(text) => scxml_syntax::is_xml_blank(&text.value),
        scxml_syntax::Node::Element(child) => {
            child
                .name
                .namespace_uri
                .as_deref()
                .is_some_and(|namespace| bound.contains(namespace))
                || child.name.namespace_uri.as_deref() == Some(SCXML_NAMESPACE)
                    && matches!(
                        child.name.local_name.as_str(),
                        "log"
                            | "raise"
                            | "assign"
                            | "script"
                            | "if"
                            | "elseif"
                            | "else"
                            | "foreach"
                            | "send"
                            | "cancel"
                    )
        }
    })
}

fn has_element_children(element: &Element) -> bool {
    element
        .children
        .iter()
        .any(|child| matches!(child, scxml_syntax::Node::Element(_)))
}

/// Whether the document's data model represents XML-element payloads and
/// initializers as XML values (ECMAScript as a read-only DOM, `XPath` as
/// node-sets); without them such content must fail closed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum XmlValues {
    /// The ECMAScript or `XPath` data model.
    Represented,
    /// The null data model, which has no values.
    Absent,
}

fn data_declaration_issue(element: &Element, xml_values: XmlValues) -> Option<UnsupportedFeature> {
    let expression = element
        .attribute("expr")
        .map(|attribute| attribute.value.as_str());
    let source = element
        .attribute("src")
        .map(|attribute| attribute.value.as_str());
    let _external_source = match source {
        // SCXML 5.3: an external data source must be a `file:` URI and
        // cannot combine with expr or non-whitespace inline content.
        Some(uri) if uri.starts_with("file:") => {
            if expression.is_some() || has_non_whitespace_content(element) {
                return unsupported(
                    Stage::Capability,
                    "unsupported-data-initializer",
                    "external data sources accept neither expr nor non-whitespace inline content",
                );
            }
            Some(uri)
        }
        Some(_) => {
            return unsupported(
                Stage::Capability,
                "unsupported-data-initializer",
                "only `file:` external data sources are within the local-access bound",
            );
        }
        None => None,
    };
    if (xml_values == XmlValues::Absent
        && expression.is_none()
        && source.is_none()
        && has_non_text_content(element))
        || (expression.is_some() && has_non_whitespace_content(element))
    {
        unsupported(
            Stage::Capability,
            "unsupported-data-initializer",
            "the executable data declaration slice accepts only expression, inline-text, or file: src initializers",
        )
    } else {
        None
    }
}

fn foreach_issue(element: &Element) -> Option<UnsupportedFeature> {
    if element.attribute("array").is_none()
        || element.attribute("item").is_none()
        || element
            .attribute("index")
            .is_some_and(|index| index.value.is_empty())
    {
        unsupported(
            Stage::Capability,
            "unsupported-foreach-attributes",
            "foreach requires array and item attributes with a non-empty index when present",
        )
    } else {
        None
    }
}

/// Returns whether the element has element children; text children are the
/// inline-content initializer form and are not element content.
fn has_non_text_content(element: &Element) -> bool {
    element
        .children
        .iter()
        .any(|child| matches!(child, scxml_syntax::Node::Element(_)))
}

/// Returns the unsupported feature for a `<send>` element, or `None` when the
/// static shape falls inside the conformance slice.
///
/// The slice accepts sends deliverable by the SCXML Event I/O processor
/// loopback (any target resolves at effect time; undeliverable targets raise
/// the spec send failure), with optional literal or expression event name,
/// send id, id location, and name list. Delayed delivery resolves through
/// the conformance runtime's deterministic virtual clock. Child payloads and
/// literal non-SCXML I/O processor types stay gated (the param/content slice).
/// Returns whether the `<send>` child payload shape is outside the slice.
///
/// Accepted payload children: `<param name="...">` with `expr` or `location`,
/// and one inline `<content>` with `expr`, character data, or XML-element
/// content (an XML value in the expression data models; gated for the null
/// data model, which has no values); `namelist` merges freely
/// with `<param>` (SCXML 6.2.4/6.2.5) and combines with `<content>` only as
/// a spec error.execution resolved at effect time.
fn send_payload_issue(element: &Element, xml_values: XmlValues) -> bool {
    let mut saw_content = false;
    let mut gated = false;
    for child in &element.children {
        match child {
            scxml_syntax::Node::Text(_) => {
                // Inline `<content>` character data; whitespace is
                // normalized at effect time.
            }
            scxml_syntax::Node::Element(child) => match child.name.local_name.as_str() {
                "param" => {
                    gated |= child.attribute("name").is_none()
                        || (child.attribute("expr").is_none()
                            && child.attribute("location").is_none());
                }
                "content" => {
                    gated |= saw_content;
                    saw_content = true;
                    // XML-element content is an XML value (an ECMAScript DOM
                    // or an XPath node-set); without XML values it must fail
                    // closed even when an `expr` is also present: silently
                    // evaluating the expression would drop the element body.
                    gated |= xml_values == XmlValues::Absent
                        && child
                            .children
                            .iter()
                            .any(|inner| matches!(inner, scxml_syntax::Node::Element(_)));
                }
                _ => gated = true,
            },
        }
    }
    // `namelist` stays runtime-reachable alone and merged with `<param>`
    // (SCXML 6.2.4/6.2.5, official case 354): invalid entries raise
    // error.execution through the generic abort path (case 553). Only
    // namelist-plus-content is gated here; content combined with params is
    // a spec error.execution resolved at effect time.
    gated |= element.attribute("namelist").is_some() && saw_content;
    gated
}

/// Classifies `<donedata>` child payload shape (SCXML 6.5).
///
/// Accepted children: `<param name>` with `expr` or `location`, and one
/// inline `<content>` with `expr`, character data, or XML-element content
/// (an XML value in the expression data models; gated for the null data
/// model); a content payload combined with
/// params is a spec error.execution resolved at effect time.
/// Typed invoke constraints (SCXML 6.4; slice-16 capability gate). The
/// `scxml` type (literal or full URI) executes through the invoked-session
/// slice; unknown types stay runtime `error.execution` and are not gated
/// here. Inline `<content>` must carry lowered SCXML markup; `src` source
/// resolution against the prepared dependency set happens at invocation
/// start (a missing dependency fails the invocation, not the gate).
fn invoke_issue(element: &Element) -> Option<UnsupportedFeature> {
    let content_children = element.children.iter().any(|child| {
        matches!(child, scxml_syntax::Node::Element(child)
            if child.name.namespace_uri.as_deref() == Some(SCXML_NAMESPACE)
                && child.name.local_name.as_str() == "content")
    });
    // An inline content body must contain a lowered SCXML root element;
    // anything else cannot lower into a child machine.
    let bad_content = element.children.iter().any(|child| {
        matches!(child, scxml_syntax::Node::Element(child)
        if child.name.namespace_uri.as_deref() == Some(SCXML_NAMESPACE)
            && child.name.local_name.as_str() == "content"
            && child.children.iter().any(|grandchild| {
                matches!(grandchild, scxml_syntax::Node::Element(grandchild)
                    if grandchild.name.namespace_uri.as_deref() == Some(SCXML_NAMESPACE)
                        && grandchild.name.local_name.as_str() != "scxml")
            }))
    });
    if bad_content {
        unsupported(
            Stage::Capability,
            "unsupported-invoke-content",
            "invoke content must carry an inline SCXML document",
        )
    } else if content_children && element.attribute("src").is_some() {
        unsupported(
            Stage::Capability,
            "unsupported-invoke-source-conflict",
            "invoke cannot carry both src and content sources",
        )
    } else {
        None
    }
}

/// History pseudostates execute through the engine's stored-history entry
/// machinery (SCXML 3.11): shallow history records direct children active at
/// exit, deep history records atomic/final descendants, and a history block's
/// transition supplies default history content when nothing is stored.
fn history_issue(element: &Element) -> Option<UnsupportedFeature> {
    if element
        .attribute("id")
        .is_none_or(|id| scxml_syntax::is_xml_blank(&id.value))
    {
        unsupported(
            Stage::Capability,
            "unsupported-history",
            "history states must carry a non-empty id",
        )
    } else if let Some(kind) = element
        .attribute("type")
        .map(|value| value.value.as_str())
        .filter(|kind| *kind != "shallow" && *kind != "deep")
    {
        unsupported(
            Stage::Capability,
            "unsupported-history",
            format!("history type `{kind}` is not shallow or deep"),
        )
    } else {
        None
    }
}

fn donedata_payload_issue(element: &Element, xml_values: XmlValues) -> Option<UnsupportedFeature> {
    let mut saw_content = false;
    let mut gated = false;
    for child in &element.children {
        match child {
            scxml_syntax::Node::Text(_) => {
                // Inline `<content>` character data; whitespace is
                // normalized at effect time.
            }
            scxml_syntax::Node::Element(child) => match child.name.local_name.as_str() {
                "param" => {
                    gated |= child.attribute("name").is_none()
                        || (child.attribute("expr").is_none()
                            && child.attribute("location").is_none());
                }
                "content" => {
                    gated |= saw_content;
                    saw_content = true;
                    // XML-element content is an XML value (an ECMAScript DOM
                    // or an XPath node-set); without XML values it must fail
                    // closed even when an `expr` is also present: silently
                    // evaluating the expression would drop the element body.
                    gated |= xml_values == XmlValues::Absent
                        && child
                            .children
                            .iter()
                            .any(|inner| matches!(inner, scxml_syntax::Node::Element(_)));
                }
                _ => gated = true,
            },
        }
    }
    gated.then(|| UnsupportedFeature {
        stage: Stage::Capability,
        code: "unsupported-donedata",
        message: "donedata with XML-element or repeated content, an incomplete param, or another child element is not supported".to_owned(),
    })
}

fn send_issue(element: &Element, xml_values: XmlValues) -> Option<UnsupportedFeature> {
    // The SCXML and BasicHTTP Event I/O processors are both linked; the
    // payload gate applies to either.
    if send_payload_issue(element, xml_values) {
        unsupported(
            Stage::Capability,
            "unsupported-send",
            "send with XML-element or repeated content, an incomplete param, another child element, or a namelist combined with payload children is not supported",
        )
    } else {
        // An unknown processor type is an invalid send, not an unsupported
        // capability: execution raises error.execution (SCXML 6.2, official
        // case 199).
        None
    }
}

fn assign_issue(element: &Element) -> Option<UnsupportedFeature> {
    // SCXML 5.8: exactly one of `expr` and inline content. Inline markup
    // content assigns the serialized child markup string (case 530).
    let has_markup = has_non_whitespace_content(element);
    match (element.attribute("expr").is_some(), has_markup) {
        (true, true) | (false, false) => unsupported(
            Stage::Capability,
            "unsupported-assign-value",
            "assign requires exactly one of expr and inline content",
        ),
        (true, false) | (false, true) => None,
    }
}

fn has_non_whitespace_content(element: &Element) -> bool {
    element.children.iter().any(|child| match child {
        scxml_syntax::Node::Text(text) => !scxml_syntax::is_xml_blank(&text.value),
        scxml_syntax::Node::Element(_) => true,
    })
}

fn transition_structure_issue(
    element: &Element,
    bound: &std::collections::BTreeSet<&str>,
) -> Option<UnsupportedFeature> {
    if has_element_children(element) && !only_supported_children(element, bound) {
        unsupported(
            Stage::Capability,
            "unsupported-executable-content",
            "the official-corpus gate currently accepts ordered conditional log, raise, assign, script, if, and foreach transition content",
        )
    } else {
        None
    }
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "every gate helper returns Option<UnsupportedFeature>; this constructor yields that shape directly"
)]
fn unsupported(
    stage: Stage,
    code: &'static str,
    message: impl Into<String>,
) -> Option<UnsupportedFeature> {
    Some(UnsupportedFeature {
        stage,
        code,
        message: message.into(),
    })
}

fn unsupported_if_has_element_children(
    element: &Element,
    code: &'static str,
    message: &'static str,
) -> Option<UnsupportedFeature> {
    has_element_children(element).then(|| UnsupportedFeature {
        stage: Stage::Capability,
        code,
        message: message.into(),
    })
}

fn contains_outcome_pass_log(element: &Element) -> bool {
    let mut found = false;
    element.visit(&mut |candidate| {
        if candidate.name.namespace_uri.as_deref() == Some(SCXML_NAMESPACE)
            && candidate.name.local_name == "log"
            && candidate
                .attribute("label")
                .is_some_and(|label| label.value == crate::oracle::OUTCOME_LABEL)
            && candidate
                .attribute("expr")
                .is_some_and(|expression| expression.value.contains("pass"))
        {
            found = true;
        }
    });
    found
}

fn contains_outcome_log(element: &Element) -> bool {
    let mut found = false;
    element.visit(&mut |candidate| {
        if candidate.name.namespace_uri.as_deref() == Some(SCXML_NAMESPACE)
            && candidate.name.local_name == "log"
            && candidate
                .attribute("label")
                .is_some_and(|label| label.value == crate::oracle::OUTCOME_LABEL)
        {
            found = true;
        }
    });
    found
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use scxml_syntax::{SourceId, SourceText, parse_xml};

    use super::detect;
    use crate::manifest::{CaseManifest, ConformanceLevel, ExpectedOutcome, LoadedCase};

    /// The source id of the documents these tests parse.
    const SOURCE: SourceId = SourceId::new(0);

    fn case() -> LoadedCase {
        LoadedCase {
            manifest: CaseManifest {
                erratum: None,
                not_applicable: None,
                id: "capability-payload-identity".into(),
                assertion_id: "0".into(),
                spec_num: "0".into(),
                spec_id: "0".into(),
                conformance: ConformanceLevel::Mandatory,
                datamodel: "ecmascript".into(),
                start_uri: "capability.scxml".into(),
                source: None,
                source_digest: None,
                source_artifact: None,
                output_digest: None,
                dependencies: Vec::new(),
                dependency_digests: BTreeMap::default(),
                dependency_source_digests: BTreeMap::default(),
                dependency_source_artifacts: BTreeMap::default(),
                resources: Vec::new(),
                resource_digests: BTreeMap::default(),
                tags: Vec::new(),
                description: String::new(),
                features: Vec::new(),
                manual: false,
                events: Vec::new(),
                expected: ExpectedOutcome::FinalState {
                    state: "done".into(),
                },
                timeout_ms: None,
                max_commands: None,
                preparation_failure: None,
            },
            source_path: None,
            source: None,
            dependency_paths: Vec::new(),
            dependency_sources: Vec::new(),
            resource_paths: Vec::new(),
        }
    }

    fn detected_code(body: &str) -> Result<Option<&'static str>, String> {
        detected_code_in("", body)
    }

    fn detected_code_in(root_attributes: &str, body: &str) -> Result<Option<&'static str>, String> {
        let text = format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="s"{root_attributes}>{body}<final id="done"/></scxml>"#
        );
        let document = parse_xml(SourceText::new(SOURCE, "capability.scxml", text))
            .map_err(|error| error.to_string())?;
        Ok(
            detect(&case(), &document, &std::collections::BTreeSet::new())
                .map(|feature| feature.code),
        )
    }

    #[test]
    fn a_document_without_datamodel_runs_on_the_ecmascript_default() {
        // SCXML 1.0 3.2.1 leaves the `datamodel` default platform-specific;
        // this platform's default is ECMAScript, so an expression in a
        // document that declares no data model reaches the engine ...
        assert_eq!(
            detected_code(
                r#"<state id="s"><onentry><log label="x" expr="'y'"/></onentry><transition target="done"/></state>"#
            ),
            Ok(None)
        );
        // ... and inline XML in `<data>` is a value (a DOM, B.2.1), exactly
        // as under an explicit `datamodel="ecmascript"`.
        let inline_xml = r#"<datamodel><data id="d"><item/></data></datamodel><state id="s"><transition target="done"/></state>"#;
        assert_eq!(detected_code(inline_xml), Ok(None));
        assert_eq!(
            detected_code_in(r#" datamodel="ecmascript""#, inline_xml),
            Ok(None)
        );
        assert_eq!(
            detected_code_in(r#" datamodel="null""#, inline_xml),
            Ok(Some("unsupported-data-initializer"))
        );
    }

    #[test]
    fn external_scripts_defer_the_data_model_gate_to_document_load() {
        // Under a data model no component executes, an executable `<log>`
        // is statically gated ...
        assert_eq!(
            detected_code_in(
                r#" datamodel="unknown""#,
                r#"<state id="s"><onentry><log label="x" expr="'y'"/></onentry><transition target="done"/></state>"#
            ),
            Ok(Some("unsupported-executable-data-model"))
        );
        // ... unless the document loads an external script: the host's
        // script loader decides the load outcome first (official case 301).
        assert_eq!(
            detected_code_in(
                r#" datamodel="unknown""#,
                r#"<script src="file:missing.js"/><state id="s"><onentry><log label="x" expr="'y'"/></onentry><transition target="done"/></state>"#
            ),
            Ok(None)
        );
    }

    #[test]
    fn payload_children_are_identified_by_source_span() {
        // A `<param>` that is a direct `<donedata>` payload child is exempt
        // from the blanket data-model gate ...
        let exempt = detected_code(
            r#"<state id="s"><transition target="done"/></state><final id="f"><donedata><param name="p" expr="1"/></donedata></final>"#,
        );
        assert!(exempt.is_ok(), "fixture parses: {exempt:?}");
        assert_ne!(exempt, Ok(Some("unsupported-datamodel-operation")));
        // ... while a textually identical `<param>` outside any payload parent
        // is a distinct element (distinct span) and stays gated.
        assert_eq!(
            detected_code(
                r#"<state id="s"><param name="p" expr="1"/><transition target="done"/></state><final id="f"><donedata><param name="p" expr="1"/></donedata></final>"#
            ),
            Ok(Some("unsupported-datamodel-operation"))
        );
    }

    #[test]
    fn multi_target_transitions_reach_the_engine() {
        // The engine executes orthogonal multi-target transitions (SCXML
        // 3.13, official case 364), so the capability stage never gates them.
        assert_eq!(
            detected_code(
                r#"<state id="s"><transition event="go" target="a b"/></state><parallel id="p"><state id="a"/><state id="b"/></parallel>"#
            ),
            Ok(None)
        );
    }
}
