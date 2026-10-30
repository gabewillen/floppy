//! `<invoke>` (SCXML 6.4) of an invoked SCXML session or an invoked
//! service.

use scxml_compiler_core::{
    ContentIr, InvokeChildIr, InvokeIr, MachineIr, MarkupContext, XmlNodeIr,
};
use scxml_types::{
    DataValue, ExtensionAttribute, InvokeId, NamedValue, NamespaceUri, QualifiedName,
    SCXML_INVOKE_TYPE_URI, StateId, is_scxml_invoke_type,
};

use super::{
    CapabilityErrorKind, DeferredRequest, DocumentKind, Interpreter, Invocation,
    InvocationProgress, InvokeRequest, InvokedSession, SessionCapabilities,
};
use crate::{EffectFailure, Engine, PlatformError};

/// Source name of a child document given by `<content expr>`.
const CONTENT_EXPR_SOURCE: &str = "invoke-content-expr.scxml";
/// Source name of a child document given by inline `<content>` markup.
const CONTENT_SOURCE: &str = "invoke-content.scxml";

/// An invocation's id (as text and typed) and its evaluated parameters.
type InvocationArguments = (String, InvokeId, Vec<NamedValue>);

/// Outcome of one invocation-argument step.
enum CommandResult<T> {
    /// The step produced its value.
    Value(T),
    /// The invocation failed: `error.execution` is already queued and the
    /// invocation is never started (SCXML 6.4, official case 554).
    Failed,
}

impl<C: SessionCapabilities> Interpreter<'_, C> {
    /// Applies one engine-emitted [`crate::Effect::Invoke`] (SCXML 6.4):
    /// resolves the type, then starts an invoked SCXML session (an absent
    /// type or an SCXML invoke type) or an invoked service (any other
    /// type). Every recoverable argument or document failure queues
    /// `error.execution` and leaves the invocation unstarted, so no
    /// `done.invoke` follows.
    pub(super) fn start_invocation(
        &mut self,
        engine: &mut Engine,
        state: &StateId,
        invoke: &InvokeIr,
    ) -> Result<(), EffectFailure<C::Error>> {
        // Type: literal or expression (official case 215). An omitted
        // type is the SCXML processor (422); any other type names an
        // invoked service, which fails like any start when no host binds
        // it (216).
        let invocation_type =
            match self.invoke_text(engine, invoke.type_value.as_ref(), invoke.typeexpr.as_ref())? {
                CommandResult::Value(value) => value,
                CommandResult::Failed => return Ok(()),
            };
        match invocation_type {
            Some(type_uri) if !is_scxml_invoke_type(&type_uri) => {
                self.start_service_invocation(engine, state, invoke, type_uri)
            }
            // `env` slots belong to invoked services; a `typeexpr` that
            // evaluates to the SCXML type with slots bound fails closed.
            _ if !invoke.attributes.is_empty() => self.invocation_failed(engine),
            _ => self.start_session_invocation(engine, state, invoke),
        }
    }

    /// Starts one invoked SCXML session: resolves the child document, the
    /// invoke id, and the parameters in the invoking session (with no
    /// current event), has the host start the child, and reports the start
    /// to the engine.
    fn start_session_invocation(
        &mut self,
        engine: &mut Engine,
        state: &StateId,
        invoke: &InvokeIr,
    ) -> Result<(), EffectFailure<C::Error>> {
        // Child document: `src`/`srcexpr` through the host's document
        // loader, else inline `<content>` (official 216, 220, 226, 239,
        // 242, 276). No resolvable source fails the invocation.
        let reference =
            match self.invoke_text(engine, invoke.src.as_ref(), invoke.srcexpr.as_ref())? {
                CommandResult::Value(value) => value,
                CommandResult::Failed => return Ok(()),
            };
        // XML Base: the URI resolves against the `<invoke>`'s in-scope
        // base, and the result against the document's own base URI.
        let reference = reference.map(|reference| match &invoke.xml_base {
            Some(base) => scxml_types::resolve_reference(base, &reference),
            None => reference,
        });
        let (source_name, source) = if let Some(reference) = reference {
            match self
                .caps
                .load_document(DocumentKind::Invocation, &reference)
            {
                // A child document is SCXML whatever its label.
                Ok(document) => {
                    let name = reference.strip_prefix("file:").unwrap_or(&reference);
                    (name.to_owned(), document.text)
                }
                Err(error) if document_load_error::<C>(&error) => {
                    return self.invocation_failed(engine);
                }
                Err(error) => return Err(EffectFailure::Fatal(error)),
            }
        } else {
            let Some(content) = invoke_content(invoke) else {
                return self.invocation_failed(engine);
            };
            if let Some(expression) = &content.expr {
                match self.caps.evaluate_value(expression, None) {
                    // A document is markup: an XML value or a string
                    // holding it.
                    Ok(DataValue::String(text) | DataValue::Xml(text)) => {
                        (CONTENT_EXPR_SOURCE.to_owned(), text)
                    }
                    Ok(_) => return self.invocation_failed(engine),
                    Err(error) if C::error_kind(&error).is_recoverable() => {
                        return self.invocation_failed(engine);
                    }
                    Err(error) => return Err(EffectFailure::Fatal(error)),
                }
            } else {
                // Content that serializes past the string bound (wasm builds
                // only) cannot be handed to the compiler: the invocation
                // fails (SCXML 3.12.2).
                let mut text = String::new();
                if serialize_child_document(&content.body, &mut text, None).is_err() {
                    return self.invocation_failed(engine);
                }
                (CONTENT_SOURCE.to_owned(), text)
            }
        };
        // The host compiles the child through the compiler component; a
        // document that does not compile fails the invocation.
        let machine_ir = match self.caps.compile_invocation(&source_name, &source) {
            Ok(bytes) => bytes,
            Err(error) if C::error_kind(&error).is_recoverable() => {
                return self.invocation_failed(engine);
            }
            Err(error) => return Err(EffectFailure::Fatal(error)),
        };
        // Compiled IR that does not decode or validate (a page-supplied
        // compiler's output, for one) fails the invocation like a document
        // that does not compile (SCXML 3.12.2).
        let Ok(child) = MachineIr::from_json(&machine_ir) else {
            return self.invocation_failed(engine);
        };
        let (invoke_id, typed_invoke_id, parameters) =
            match self.invocation_arguments(engine, state, invoke)? {
                CommandResult::Value(arguments) => arguments,
                CommandResult::Failed => return Ok(()),
            };
        let request = InvokedSession {
            invoke_id,
            machine_ir,
            data_model: child.data_model,
            parameters,
            trace_context: self.outbound.clone(),
        };
        // A host that starts the child later suspends the run right after
        // this effect; the start's result settles when the run continues,
        // exactly as it does here.
        let result = self.caps.start_invocation(Invocation::Session(request));
        self.started(
            engine,
            state,
            invoke.document_order,
            typed_invoke_id,
            SCXML_INVOKE_TYPE_URI.to_owned(),
            result,
        )
    }

    /// Starts one invoked service (SCXML 6.4), which the host runs through
    /// the service component bound to exactly `type_uri`. The `src`/`srcexpr`
    /// text and the `<content>` value are evaluated here and handed to the
    /// service as data (nothing is loaded or compiled); the invoke id and
    /// parameters are resolved as for a session. A type no component is
    /// bound to (`unavailable`; SCXML 6.4.1 allows platform-specific types,
    /// which the host judges) or a rejected start fails the invocation with
    /// `error.execution`.
    fn start_service_invocation(
        &mut self,
        engine: &mut Engine,
        state: &StateId,
        invoke: &InvokeIr,
        type_uri: String,
    ) -> Result<(), EffectFailure<C::Error>> {
        let source = match self.invoke_text(engine, invoke.src.as_ref(), invoke.srcexpr.as_ref())? {
            CommandResult::Value(value) => value,
            CommandResult::Failed => return Ok(()),
        };
        let content = match invoke_content(invoke) {
            None => None,
            Some(content) => {
                let value = if let Some(expression) = &content.expr {
                    self.caps.evaluate_value(expression, None)
                } else {
                    // SCXML 6.2.6 as for `<send>`: a body the data model
                    // cannot represent is no legal value (error.execution).
                    match super::inline_body(&content.body)
                        .and_then(|body| self.caps.inline_content(&body))
                    {
                        Some(value) => Ok(value),
                        None => return self.invocation_failed(engine),
                    }
                };
                match self.argument(engine, value)? {
                    CommandResult::Value(value) => Some(value),
                    CommandResult::Failed => return Ok(()),
                }
            }
        };
        let (invoke_id, typed_invoke_id, parameters) =
            match self.invocation_arguments(engine, state, invoke)? {
                CommandResult::Value(arguments) => arguments,
                CommandResult::Failed => return Ok(()),
            };
        // `env` attributes: each names its variable literally or by an
        // expression evaluated here, with no current event; the host binds
        // the value (never seen by the machine or the service) by reference.
        let mut attributes = Vec::with_capacity(invoke.attributes.len());
        for attribute in &invoke.attributes {
            let local = attribute.name.local_name.as_str();
            let (slot, name) = match local.strip_suffix("expr") {
                Some(slot) if !slot.is_empty() => {
                    match self.invoke_text(engine, None, Some(&attribute.value))? {
                        CommandResult::Value(Some(name)) => (slot, name),
                        CommandResult::Value(None) => return self.invocation_failed(engine),
                        CommandResult::Failed => return Ok(()),
                    }
                }
                _ => (local, attribute.value.clone()),
            };
            // Machine IR validation admits only the `env` namespace here.
            let Ok(namespace_uri) = NamespaceUri::new(scxml_types::ENV_NAMESPACE_URI) else {
                return self.invocation_failed(engine);
            };
            attributes.push(ExtensionAttribute {
                name: QualifiedName {
                    namespace_uri: Some(namespace_uri),
                    local_name: slot.to_owned(),
                },
                value: name,
            });
        }
        attributes.sort_by(|first, second| first.name.local_name.cmp(&second.name.local_name));
        let request = InvokeRequest {
            invoke_id,
            type_uri: type_uri.clone(),
            source,
            content,
            parameters,
            attributes,
            trace_context: self.outbound.clone(),
        };
        let result = match self.caps.start_invocation(Invocation::Service(request)) {
            // No service component is bound to the type: the invocation
            // fails, never the session (SCXML 6.4).
            Err(error) if C::error_kind(&error) == CapabilityErrorKind::Unavailable => {
                return self.invocation_failed(engine);
            }
            result => result,
        };
        self.started(
            engine,
            state,
            invoke.document_order,
            typed_invoke_id,
            type_uri,
            result,
        )
    }

    /// Settles one start request's immediate answer: a deferred start
    /// suspends the run right after this effect, every other answer settles
    /// at once.
    fn started(
        &mut self,
        engine: &mut Engine,
        state: &StateId,
        document_order: u32,
        invoke_id: InvokeId,
        type_uri: String,
        result: Result<InvocationProgress, C::Error>,
    ) -> Result<(), EffectFailure<C::Error>> {
        let result = match result {
            Ok(InvocationProgress::Completed) => Ok(()),
            Ok(InvocationProgress::Deferred) => {
                self.defer(DeferredRequest::Start {
                    state: state.clone(),
                    document_order,
                    invoke_id,
                    type_uri,
                });
                return Ok(());
            }
            Err(error) => Err(error),
        };
        self.invocation_start_result(engine, state, document_order, invoke_id, type_uri, result)
    }

    /// The invoke id (literal, or generated `<state>.<n>`, SCXML
    /// 6.4.1; official 224, 225), optionally stored at `idlocation`, and the
    /// `<param>` then `namelist` entries (SCXML 6.4.2), evaluated in the
    /// invoking session with no current event. Only a generated id takes
    /// an ordinal. Any recoverable failure fails the invocation.
    fn invocation_arguments(
        &mut self,
        engine: &mut Engine,
        state: &StateId,
        invoke: &InvokeIr,
    ) -> Result<CommandResult<InvocationArguments>, EffectFailure<C::Error>> {
        let failed = |interpreter: &mut Self, engine: &mut Engine| {
            interpreter
                .invocation_failed(engine)
                .map(|()| CommandResult::Failed)
        };
        let invoke_id = match &invoke.id {
            Some(id) => id.clone(),
            None => self.state.next_invoke_id(state),
        };
        // An id that is not a name fails this invocation (SCXML 3.12.2: the
        // processor "MUST signal any errors that occur by raising SCXML
        // events whose names begin with 'error.'").
        let Ok(typed_invoke_id) = InvokeId::new(invoke_id.clone()) else {
            return failed(self, engine);
        };
        if let Some(location) = &invoke.idlocation
            && self
                .caps
                .assign(
                    location,
                    &scxml_types::AssignSource::Value(DataValue::String(invoke_id.clone())),
                    &scxml_types::AssignPlacement::ReplaceChildren,
                    None,
                    None,
                )
                .is_err()
        {
            return failed(self, engine);
        }
        let mut parameters = Vec::new();
        for child_ir in &invoke.children {
            let InvokeChildIr::Param { value } = child_ir else {
                continue;
            };
            let evaluated = match (&value.expr, &value.location) {
                (Some(expression), _) => self.caps.evaluate_value(expression, None),
                (None, Some(location)) => self.caps.evaluate_location(location, None),
                // A param without a source raises error.execution (6.2.5);
                // the invocation fails the same way (554).
                (None, None) => return failed(self, engine),
            };
            match self.argument(engine, evaluated)? {
                CommandResult::Value(evaluated) => parameters.push(NamedValue {
                    name: value.name.clone(),
                    value: evaluated,
                }),
                CommandResult::Failed => return Ok(CommandResult::Failed),
            }
        }
        for location in &invoke.namelist {
            let Some(name) = self.caps.namelist_key(location) else {
                return failed(self, engine);
            };
            let evaluated = self.caps.evaluate_location(location, None);
            match self.argument(engine, evaluated)? {
                CommandResult::Value(value) => parameters.push(NamedValue { name, value }),
                CommandResult::Failed => return Ok(CommandResult::Failed),
            }
        }
        Ok(CommandResult::Value((
            invoke_id,
            typed_invoke_id,
            parameters,
        )))
    }

    /// Settles the result of one `start-invocation` request. A child the
    /// host rejects at load (`invalid-input`) fails the invocation (SCXML
    /// 6.4). Every other start failure, `unavailable` and
    /// `capacity-exhausted` included, ends the session (WIT
    /// `start-invocation`).
    pub(super) fn invocation_start_result(
        &mut self,
        engine: &mut Engine,
        state: &StateId,
        document_order: u32,
        invoke_id: InvokeId,
        type_uri: String,
        result: Result<(), C::Error>,
    ) -> Result<(), EffectFailure<C::Error>> {
        match result {
            Ok(()) => {}
            Err(error) if C::error_kind(&error).is_recoverable() => {
                return self.invocation_failed(engine);
            }
            Err(error) => return Err(EffectFailure::Fatal(error)),
        }
        engine
            .invocation_started(state, document_order, invoke_id, type_uri)
            .map_err(|cause| self.engine_fatal(cause, "engine-invocation-started"))
    }

    /// A string-typed invoke attribute (`type`/`typeexpr`,
    /// `src`/`srcexpr`): the literal, else the expression's text.
    fn invoke_text(
        &mut self,
        engine: &mut Engine,
        literal: Option<&String>,
        expression: Option<&String>,
    ) -> Result<CommandResult<Option<String>>, EffectFailure<C::Error>> {
        match (literal, expression) {
            (Some(value), _) => Ok(CommandResult::Value(Some(value.clone()))),
            (None, Some(expression)) => match self.caps.evaluate_text(expression, None) {
                Ok(Some(value)) => Ok(CommandResult::Value(Some(value))),
                Ok(None) => self
                    .invocation_failed(engine)
                    .map(|()| CommandResult::Failed),
                Err(error) => self
                    .argument::<()>(engine, Err(error))
                    .map(|_| CommandResult::Failed),
            },
            (None, None) => Ok(CommandResult::Value(None)),
        }
    }

    /// Classifies one evaluated invocation argument: a recoverable failure
    /// fails the invocation, any other one the session.
    fn argument<T>(
        &mut self,
        engine: &mut Engine,
        evaluated: Result<T, C::Error>,
    ) -> Result<CommandResult<T>, EffectFailure<C::Error>> {
        match evaluated {
            Ok(value) => Ok(CommandResult::Value(value)),
            Err(error) if C::error_kind(&error).is_recoverable() => self
                .invocation_failed(engine)
                .map(|()| CommandResult::Failed),
            Err(error) => Err(EffectFailure::Fatal(error)),
        }
    }

    /// Fails the invocation: `error.execution`, and it is never started.
    fn invocation_failed(&mut self, engine: &mut Engine) -> Result<(), EffectFailure<C::Error>> {
        self.queue_error(engine, PlatformError::Execution)
    }
}

/// The `<invoke>`'s `<content>` child, if any.
fn invoke_content(invoke: &InvokeIr) -> Option<&ContentIr> {
    invoke.children.iter().find_map(|child| match child {
        InvokeChildIr::Content { value } => Some(value),
        InvokeChildIr::Param { .. } | InvokeChildIr::Finalize { .. } => None,
    })
}

/// Whether an `<invoke src>` document-load failure fails only the invocation
/// (`error.execution`): a recoverable failure, or `unavailable` for a URI
/// that does not resolve (WIT `load-document`). Every other failure is fatal
/// to the invoking session.
fn document_load_error<C: SessionCapabilities>(error: &C::Error) -> bool {
    let kind = C::error_kind(error);
    kind.is_recoverable() || kind == CapabilityErrorKind::Unavailable
}

/// XML namespace bound to the reserved `xml` prefix.
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

/// Serializes inline `<invoke><content>` markup back to document text. The
/// child is compiled as a standalone document, so every element restates
/// its namespace wherever it differs from the one it inherits (the root
/// SCXML element restates the canonical SCXML namespace; an extension-namespace
/// element such as `<example:raise>` keeps its exact namespace URI), and
/// namespaced attributes get deterministic `ns<N>` prefixes declared on
/// their element (`xml:` is reserved). A pure-SCXML child serializes exactly
/// as before: only its root restates the namespace.
///
/// # Errors
///
/// When the text would exceed [`scxml_compiler_core::MAX_STRING_BYTES`].
fn serialize_child_document(
    nodes: &[XmlNodeIr],
    out: &mut String,
    inherited: Option<&str>,
) -> Result<(), ()> {
    let push = |out: &mut String, text: &str| {
        if scxml_types::exceeds_wasm_bound(
            out.len().saturating_add(text.len()),
            scxml_compiler_core::MAX_STRING_BYTES,
        ) {
            return Err(());
        }
        out.push_str(text);
        Ok(())
    };
    let push_escaped = |out: &mut String, value: &str, context: MarkupContext| {
        push(out, &scxml_compiler_core::escape_markup(value, context))
    };
    for node in nodes {
        match node {
            XmlNodeIr::Text { value } => push_escaped(out, value, MarkupContext::Text)?,
            XmlNodeIr::Element {
                name,
                attributes,
                children,
            } => {
                push(out, "<")?;
                push(out, &name.local_name)?;
                let namespace = name.namespace_uri.as_deref();
                if namespace != inherited
                    && !attributes
                        .iter()
                        .any(|attribute| attribute.name.local_name == "xmlns")
                {
                    push(out, " xmlns=\"")?;
                    push_escaped(out, namespace.unwrap_or_default(), MarkupContext::Attribute)?;
                    push(out, "\"")?;
                }
                let mut prefixes: Vec<&str> = Vec::new();
                for attribute in attributes {
                    push(out, " ")?;
                    match attribute.name.namespace_uri.as_deref() {
                        None => {}
                        Some(XML_NAMESPACE) => push(out, "xml:")?,
                        Some(uri) => {
                            let index = if let Some(index) =
                                prefixes.iter().position(|bound| *bound == uri)
                            {
                                index
                            } else {
                                prefixes.push(uri);
                                prefixes.len() - 1
                            };
                            push(out, &format!("ns{index}:"))?;
                        }
                    }
                    push(out, &attribute.name.local_name)?;
                    push(out, "=\"")?;
                    push_escaped(out, &attribute.value, MarkupContext::Attribute)?;
                    push(out, "\"")?;
                }
                for (index, uri) in prefixes.iter().enumerate() {
                    push(out, &format!(" xmlns:ns{index}=\""))?;
                    push_escaped(out, uri, MarkupContext::Attribute)?;
                    push(out, "\"")?;
                }
                if children.is_empty() {
                    push(out, "/>")?;
                } else {
                    push(out, ">")?;
                    serialize_child_document(children, out, namespace)?;
                    push(out, "</")?;
                    push(out, &name.local_name)?;
                    push(out, ">")?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::serialize_child_document;
    use scxml_compiler_core::{ExpandedNameIr, XmlAttributeIr, XmlNodeIr};

    #[test]
    fn child_document_serialization_restates_the_root_namespace_and_keeps_utf8() {
        let nodes = [XmlNodeIr::Element {
            name: ExpandedNameIr {
                namespace_uri: Some(scxml_compiler_core::SCXML_NAMESPACE.to_owned()),
                local_name: "scxml".to_owned(),
            },
            attributes: vec![XmlAttributeIr {
                name: ExpandedNameIr {
                    namespace_uri: None,
                    local_name: "name".to_owned(),
                },
                value: "caf\u{e9} \"&<".to_owned(),
            }],
            children: vec![XmlNodeIr::Element {
                name: ExpandedNameIr {
                    namespace_uri: Some(scxml_compiler_core::SCXML_NAMESPACE.to_owned()),
                    local_name: "final".to_owned(),
                },
                attributes: Vec::new(),
                children: Vec::new(),
            }],
        }];
        let mut out = String::new();
        serialize_child_document(&nodes, &mut out, None).expect("fixture fits the bound");
        assert_eq!(
            out,
            "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" name=\"caf\u{e9} &quot;&amp;&lt;\"><final/></scxml>"
        );
    }

    #[test]
    fn extension_namespace_elements_and_namespaced_attributes_keep_their_namespaces() {
        let name = |namespace: Option<&str>, local: &str| ExpandedNameIr {
            namespace_uri: namespace.map(str::to_owned),
            local_name: local.to_owned(),
        };
        let example = "https://stateforward.org/scxml/example";
        let nodes = [XmlNodeIr::Element {
            name: name(Some(scxml_compiler_core::SCXML_NAMESPACE), "scxml"),
            attributes: Vec::new(),
            children: vec![XmlNodeIr::Element {
                name: name(Some(example), "raise"),
                attributes: vec![
                    XmlAttributeIr {
                        name: name(None, "name"),
                        value: "HOME".to_owned(),
                    },
                    XmlAttributeIr {
                        name: name(Some("https://example.org/a"), "flag"),
                        value: "1".to_owned(),
                    },
                    XmlAttributeIr {
                        name: name(Some("http://www.w3.org/XML/1998/namespace"), "lang"),
                        value: "en".to_owned(),
                    },
                ],
                children: vec![XmlNodeIr::Element {
                    name: name(None, "plain"),
                    attributes: Vec::new(),
                    children: Vec::new(),
                }],
            }],
        }];
        let mut out = String::new();
        serialize_child_document(&nodes, &mut out, None).expect("fixture fits the bound");
        assert_eq!(
            out,
            "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\"><raise xmlns=\"https://stateforward.org/scxml/example\" name=\"HOME\" ns0:flag=\"1\" xml:lang=\"en\" xmlns:ns0=\"https://example.org/a\"><plain xmlns=\"\"/></raise></scxml>"
        );
    }
}
