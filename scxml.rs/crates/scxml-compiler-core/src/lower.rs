use std::collections::BTreeMap;

use scxml_syntax::{Document, Element, Node, Span, collapse_whitespace, xml_tokens};
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};

use scxml_event_schema::{Resources, SCHEMA_ATTRIBUTE, SCHEMA_EXPR_ATTRIBUTE, Source};
use scxml_types::EVENT_NAMESPACE_URI;

use crate::{
    ActionBlockIr, AssignTypeIr, Binding, ContentIr, DataIr, DoneDataChildIr, DoneDataIr, ElseIfIr,
    EventSchemaIr, ExecutableContentIr, ExpandedNameIr, InitialIr, InvokeChildIr, InvokeIr,
    LowerError, MachineIr, ParamIr, SCXML_NAMESPACE, SendIr, StateIr, StateKind, TransitionIr,
    TransitionType, XmlAttributeIr, XmlNodeIr,
};

#[derive(Debug)]
struct PendingTransition {
    document_order: u32,
    source: u32,
    target_ids: Vec<String>,
    events: Vec<String>,
    condition: Option<String>,
    transition_type: TransitionType,
    actions: Vec<ExecutableContentIr>,
    event_schema: Option<EventSchemaIr>,
}

#[derive(Debug)]
struct PendingInitial {
    document_order: u32,
    target_ids: Vec<String>,
    actions: Vec<ExecutableContentIr>,
}

#[derive(Debug)]
struct Builder<'r> {
    /// The bytes of each schema file an `event:schema` references.
    resources: &'r Resources,
    next_order: u32,
    states: Vec<StateIr>,
    state_initials: Vec<Option<PendingInitial>>,
    transitions: Vec<PendingTransition>,
    ids: BTreeMap<String, u32>,
    /// Source span of the innermost construct whose lowering failed; set
    /// once, by the first (innermost) [`Builder::locate`] that sees the
    /// error, so the compiler reports the responsible element.
    failure: Option<Span>,
    /// The in-scope XML Base of each element that has one
    /// ([`Element::visit_with_base`]), by the element's start offset.
    bases: BTreeMap<usize, String>,
}

impl<'r> Builder<'r> {
    fn new(resources: &'r Resources) -> Self {
        Self {
            resources,
            next_order: 0,
            states: Vec::new(),
            state_initials: Vec::new(),
            transitions: Vec::new(),
            ids: BTreeMap::new(),
            failure: None,
            bases: BTreeMap::new(),
        }
    }

    /// The in-scope XML Base of `element`, when an ancestor-or-self has an
    /// `xml:base`.
    fn base(&self, element: &Element) -> Option<String> {
        self.bases.get(&element.span.start).cloned()
    }

    /// The `src` of a `<data>` or `<script>`, which always names a
    /// document to load: resolved against the element's in-scope XML Base
    /// ([`scxml_types::resolve_reference`]), so the host resolves the
    /// result against the document's own base URI.
    fn src(&self, element: &Element) -> Option<String> {
        token_attr(element, "src").map(|src| match self.base(element) {
            Some(base) => scxml_types::resolve_reference(&base, &src),
            None => src,
        })
    }

    /// Records `span` as the failure location when `result` is the first
    /// error seen, and passes `result` through.
    fn locate<T>(&mut self, span: Span, result: Result<T, LowerError>) -> Result<T, LowerError> {
        if result.is_err() && self.failure.is_none() {
            self.failure = Some(span);
        }
        result
    }

    /// Fails with `error` located at `span`.
    fn fail_at<T>(&mut self, span: Span, error: LowerError) -> Result<T, LowerError> {
        self.locate(span, Err(error))
    }

    /// The element of a structural child node; whitespace is skipped and
    /// any other text is rejected at its span.
    fn structural_element<'n>(
        &mut self,
        node: &'n Node,
    ) -> Result<Option<&'n Element>, LowerError> {
        match node {
            // Markup from another namespace outside executable content has
            // no behavior: its package (or the validator's `LINT022`)
            // judges it, and lowering has nothing to keep.
            Node::Element(element) if is_foreign(element) => Ok(None),
            Node::Element(element) => Ok(Some(element)),
            Node::Text(text) if scxml_syntax::is_xml_blank(&text.value) => Ok(None),
            Node::Text(text) => self.fail_at(
                text.span,
                LowerError::InvalidStructure("structural SCXML content cannot contain free text"),
            ),
        }
    }

    fn order(&mut self) -> Result<u32, LowerError> {
        let order = self.next_order;
        self.next_order = self
            .next_order
            .checked_add(1)
            .ok_or(LowerError::TooManySourceElements)?;
        Ok(order)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one source-order pass owns state reservation and every legal state child"
    )]
    fn lower_state(
        &mut self,
        element: &Element,
        parent: Option<u32>,
        depth: usize,
    ) -> Result<u32, LowerError> {
        if scxml_types::exceeds_wasm_bound(depth, crate::MAX_CONTENT_DEPTH) {
            return Err(LowerError::LimitExceeded {
                field: "state hierarchy depth",
                limit: crate::MAX_CONTENT_DEPTH,
            });
        }
        ensure_count(
            "source states",
            self.states.len().saturating_add(1),
            crate::MAX_TABLE_ITEMS,
        )?;
        ensure_count(
            "state child nodes",
            element.children.len(),
            crate::MAX_NESTED_ITEMS,
        )?;
        let local = element.name.local_name.as_str();
        let mut kind = match local {
            "state" => StateKind::Atomic,
            "parallel" => StateKind::Parallel,
            "final" => StateKind::Final,
            "history" => match optional_attr(element, "type").as_deref() {
                None | Some("shallow") => StateKind::ShallowHistory,
                Some("deep") => StateKind::DeepHistory,
                Some(value) => {
                    return Err(LowerError::InvalidAttributeValue {
                        element: "history",
                        attribute: "type",
                        value: crate::safe_value(value),
                    });
                }
            },
            _ => return Err(LowerError::UnsupportedElement(crate::safe_value(local))),
        };
        let allowed = match local {
            "state" => &["id", "initial"][..],
            "history" => &["id", "type"][..],
            _ => &["id"][..],
        };
        check_attributes(element, allowed)?;
        // SCXML 3.3, 3.4, 3.7, 3.10: `id` is optional on every state and
        // history pseudo-state. SCXML 3.14: "When such an attribute is
        // defined to be optional and the author omits it, then, for elements
        // other than <send> and <invoke>, the SCXML processor MUST generate a
        // unique id automatically at document load time." The generated id
        // is deterministic and no XML ID can equal it (`$` is not an NCName
        // character), so no document can name it.
        let id = Some(token_attr(element, "id").unwrap_or_else(|| {
            let state_index = self.states.len();
            let mut candidate = format!("${local}{state_index}");
            while self.ids.contains_key(&candidate) {
                candidate.push('$');
            }
            candidate
        }));
        let state_index = self.states.len();
        let index = u32::try_from(state_index).map_err(|_| LowerError::TooManyStates)?;
        if let Some(id) = id.as_ref()
            && self.ids.insert(id.clone(), index).is_some()
        {
            return Err(LowerError::DuplicateStateId(crate::safe_value(id)));
        }
        let document_order = self.order()?;
        self.states.push(StateIr {
            document_order,
            id,
            kind,
            parent,
            children: Vec::new(),
            initial: None,
            onentry: Vec::new(),
            onexit: Vec::new(),
            transitions: Vec::new(),
            data: Vec::new(),
            invokes: Vec::new(),
            donedata: None,
        });
        self.state_initials.push(None);

        let attribute_initial = token_attr(element, "initial").map(|value| PendingInitial {
            document_order,
            target_ids: split_tokens(&value),
            actions: Vec::new(),
        });
        let mut explicit_initial = None;
        for node in &element.children {
            let child = self.structural_element(node)?;
            let Some(child) = child else { continue };
            if child.name.namespace_uri.as_deref() != Some(SCXML_NAMESPACE) {
                return self.fail_at(
                    element_name_span(child),
                    LowerError::UnsupportedElement(crate::safe_value(&child.lexical_name)),
                );
            }
            match child.name.local_name.as_str() {
                "state" | "parallel" | "final" | "history" => {
                    let lowered = self.lower_state(child, Some(index), depth.saturating_add(1));
                    let child_index = self.locate(element_name_span(child), lowered)?;
                    self.states
                        .get_mut(state_index)
                        .ok_or(LowerError::InvalidReference("new state"))?
                        .children
                        .push(child_index);
                }
                "initial" => {
                    if explicit_initial.is_some() {
                        return self.fail_at(
                            element_name_span(child),
                            LowerError::InvalidStructure(
                                "state contains more than one initial element",
                            ),
                        );
                    }
                    let lowered = self.lower_initial(child);
                    explicit_initial = Some(self.locate(element_name_span(child), lowered)?);
                }
                "transition" => {
                    ensure_count(
                        "source transitions",
                        self.transitions.len().saturating_add(1),
                        crate::MAX_TABLE_ITEMS,
                    )?;
                    let lowered = self.lower_transition(child, index);
                    let transition = self.locate(element_name_span(child), lowered)?;
                    let transition_index = u32::try_from(self.transitions.len())
                        .map_err(|_| LowerError::TooManyTransitions)?;
                    self.transitions.push(transition);
                    self.states
                        .get_mut(state_index)
                        .ok_or(LowerError::InvalidReference("new state"))?
                        .transitions
                        .push(transition_index);
                }
                "onentry" => {
                    let checked = check_attributes(child, &[]);
                    self.locate(element_name_span(child), checked)?;
                    let order = self.order()?;
                    let lowered = self.lower_actions(&child.children, 0);
                    let actions = self.locate(element_name_span(child), lowered)?;
                    self.states
                        .get_mut(state_index)
                        .ok_or(LowerError::InvalidReference("new state"))?
                        .onentry
                        .push(ActionBlockIr {
                            document_order: order,
                            actions,
                        });
                }
                "onexit" => {
                    let checked = check_attributes(child, &[]);
                    self.locate(element_name_span(child), checked)?;
                    let order = self.order()?;
                    let lowered = self.lower_actions(&child.children, 0);
                    let actions = self.locate(element_name_span(child), lowered)?;
                    self.states
                        .get_mut(state_index)
                        .ok_or(LowerError::InvalidReference("new state"))?
                        .onexit
                        .push(ActionBlockIr {
                            document_order: order,
                            actions,
                        });
                }
                "datamodel" => {
                    let checked = check_attributes(child, &[]);
                    self.locate(element_name_span(child), checked)?;
                    // Consume this element's document-order ordinal so later ordinals
                    // stay aligned with source order; the element records none itself.
                    self.order()?;
                    let lowered = self.lower_datamodel(child);
                    let data = self.locate(element_name_span(child), lowered)?;
                    self.states
                        .get_mut(state_index)
                        .ok_or(LowerError::InvalidReference("new state"))?
                        .data
                        .extend(data);
                }
                "invoke" => {
                    let lowered = self.lower_invoke(child);
                    let invoke = self.locate(element_name_span(child), lowered)?;
                    self.states
                        .get_mut(state_index)
                        .ok_or(LowerError::InvalidReference("new state"))?
                        .invokes
                        .push(invoke);
                }
                "donedata" => {
                    if self
                        .states
                        .get(state_index)
                        .ok_or(LowerError::InvalidReference("new state"))?
                        .donedata
                        .is_some()
                    {
                        return self.fail_at(
                            element_name_span(child),
                            LowerError::InvalidStructure(
                                "final state contains more than one donedata element",
                            ),
                        );
                    }
                    let lowered = self.lower_done_data(child);
                    let done_data = self.locate(element_name_span(child), lowered)?;
                    self.states
                        .get_mut(state_index)
                        .ok_or(LowerError::InvalidReference("new state"))?
                        .donedata = Some(done_data);
                }
                other => {
                    return self.fail_at(
                        element_name_span(child),
                        LowerError::UnsupportedElement(crate::safe_value(other)),
                    );
                }
            }
        }
        if attribute_initial.is_some() && explicit_initial.is_some() {
            return Err(LowerError::InvalidStructure(
                "state cannot combine initial attribute and initial element",
            ));
        }
        if local == "state"
            && !self
                .states
                .get(state_index)
                .ok_or(LowerError::InvalidReference("new state"))?
                .children
                .is_empty()
        {
            kind = StateKind::Compound;
            self.states
                .get_mut(state_index)
                .ok_or(LowerError::InvalidReference("new state"))?
                .kind = kind;
            let default_initial = self.default_child_initial(index, document_order)?;
            *self
                .state_initials
                .get_mut(state_index)
                .ok_or(LowerError::InvalidReference("new state initial"))? = explicit_initial
                .or(attribute_initial)
                .or(Some(default_initial));
        } else {
            *self
                .state_initials
                .get_mut(state_index)
                .ok_or(LowerError::InvalidReference("new state initial"))? =
                explicit_initial.or(attribute_initial);
        }
        Ok(index)
    }

    fn default_child_initial(
        &self,
        owner: u32,
        document_order: u32,
    ) -> Result<PendingInitial, LowerError> {
        let owner = usize::try_from(owner)
            .map_err(|_| LowerError::InvalidReference("default initial owner"))?;
        let state = self
            .states
            .get(owner)
            .ok_or(LowerError::InvalidReference("default initial owner"))?;
        let mut initial_id = None;
        for child in &state.children {
            let child = usize::try_from(*child)
                .map_err(|_| LowerError::InvalidReference("default initial child"))?;
            let child = self
                .states
                .get(child)
                .ok_or(LowerError::InvalidReference("default initial child"))?;
            if !matches!(
                child.kind,
                StateKind::ShallowHistory | StateKind::DeepHistory
            ) {
                initial_id.clone_from(&child.id);
                break;
            }
        }
        let id = initial_id.ok_or(LowerError::InvalidStructure(
            "compound state has no initializable child",
        ))?;
        Ok(PendingInitial {
            document_order,
            target_ids: vec![id],
            actions: Vec::new(),
        })
    }

    fn lower_initial(&mut self, element: &Element) -> Result<PendingInitial, LowerError> {
        ensure_count(
            "initial child nodes",
            element.children.len(),
            crate::MAX_NESTED_ITEMS,
        )?;
        check_attributes(element, &[])?;
        let document_order = self.order()?;
        let mut transition = None;
        for node in &element.children {
            let child = self.structural_element(node)?;
            let Some(child) = child else { continue };
            let span = element_name_span(child);
            if !child.name.is(Some(SCXML_NAMESPACE), "transition") {
                return self.fail_at(
                    span,
                    LowerError::UnsupportedElement(crate::safe_value(&child.lexical_name)),
                );
            }
            if transition.is_some() {
                return self.fail_at(
                    span,
                    LowerError::InvalidStructure("initial element requires exactly one transition"),
                );
            }
            // SCXML 3.6.2 forbids only 'cond' and 'event' here ("this
            // transition MUST NOT contain 'cond' or 'event' attributes");
            // a valid `type` is accepted and has no effect, because the
            // transition only names default entry targets.
            let checked = check_attributes(child, &["target", "type"])
                .and_then(|()| transition_type(child))
                .and_then(|_| required_attr(child, "target"));
            let target_ids = split_tokens(&self.locate(span, checked)?);
            if target_ids.is_empty() {
                return self.fail_at(span, LowerError::TransitionWithoutTarget);
            }
            let _transition_order = self.order()?;
            let lowered = self.lower_actions(&child.children, 0);
            transition = Some(PendingInitial {
                document_order,
                target_ids,
                actions: self.locate(span, lowered)?,
            });
        }
        transition.ok_or(LowerError::InvalidStructure(
            "initial element requires exactly one transition",
        ))
    }

    fn lower_transition(
        &mut self,
        element: &Element,
        source: u32,
    ) -> Result<PendingTransition, LowerError> {
        check_attributes(element, &["event", "cond", "target", "type"])?;
        let transition_type = transition_type(element)?;
        let from_history = usize::try_from(source)
            .ok()
            .and_then(|source| self.states.get(source))
            .is_some_and(|state| {
                matches!(
                    state.kind,
                    StateKind::ShallowHistory | StateKind::DeepHistory
                )
            });
        // SCXML 3.10.2: "this transition MUST NOT contain 'cond' or 'event'
        // attributes". A `type` is allowed; the default history transition
        // is never selected, only its targets and content are used.
        if from_history
            && (element.attribute("event").is_some() || element.attribute("cond").is_some())
        {
            return Err(LowerError::InvalidStructure(
                "history fallback transition cannot have event or condition",
            ));
        }
        let event_schema = event_schema(element, self.resources)?;
        Ok(PendingTransition {
            event_schema,
            document_order: self.order()?,
            source,
            target_ids: token_attr(element, "target")
                .map_or_else(Vec::new, |value| split_tokens(&value)),
            events: token_attr(element, "event")
                .map_or_else(Vec::new, |value| split_tokens(&value)),
            condition: optional_attr(element, "cond"),
            transition_type,
            actions: self.lower_actions(&element.children, 0)?,
        })
    }

    fn lower_datamodel(&mut self, element: &Element) -> Result<Vec<DataIr>, LowerError> {
        ensure_count(
            "datamodel child nodes",
            element.children.len(),
            crate::MAX_NESTED_ITEMS,
        )?;
        let mut data = Vec::new();
        for node in &element.children {
            let child = self.structural_element(node)?;
            let Some(child) = child else { continue };
            let span = element_name_span(child);
            if !child.name.is(Some(SCXML_NAMESPACE), "data") {
                return self.fail_at(
                    span,
                    LowerError::UnsupportedElement(crate::safe_value(&child.lexical_name)),
                );
            }
            let checked = check_attributes(child, &["id", "src", "expr"])
                .and_then(|()| required_token_attr(child, "id"));
            let id = self.locate(span, checked)?;
            let lowered = lower_xml_nodes(&child.children, 0);
            let content = self.locate(span, lowered)?;
            data.push(DataIr {
                document_order: self.order()?,
                id,
                src: self.src(child),
                expr: optional_attr(child, "expr"),
                content,
            });
        }
        Ok(data)
    }

    fn lower_invoke(&mut self, element: &Element) -> Result<InvokeIr, LowerError> {
        ensure_count(
            "invoke child nodes",
            element.children.len(),
            crate::MAX_NESTED_ITEMS,
        )?;
        check_attributes(
            element,
            &[
                "type",
                "typeexpr",
                "src",
                "srcexpr",
                "id",
                "idlocation",
                "namelist",
                "autoforward",
            ],
        )?;
        let mut children = Vec::new();
        let (mut contents, mut finalizes) = (0_usize, 0_usize);
        for node in &element.children {
            let child = self.structural_element(node)?;
            let Some(child) = child else { continue };
            let span = element_name_span(child);
            if child.name.namespace_uri.as_deref() != Some(SCXML_NAMESPACE) {
                return self.fail_at(
                    span,
                    LowerError::UnsupportedElement(crate::safe_value(&child.lexical_name)),
                );
            }
            match child.name.local_name.as_str() {
                "param" => {
                    let lowered = lower_param(child);
                    children.push(InvokeChildIr::Param {
                        value: self.locate(span, lowered)?,
                    });
                }
                "content" | "finalize" => {
                    let count = if child.name.local_name == "content" {
                        &mut contents
                    } else {
                        &mut finalizes
                    };
                    *count += 1;
                    if *count > 1 {
                        return self.fail_at(
                            span,
                            LowerError::InvalidStructure(
                                "invoke permits at most one content and finalize child",
                            ),
                        );
                    }
                    if child.name.local_name == "content" {
                        let lowered = lower_content(child);
                        children.push(InvokeChildIr::Content {
                            value: self.locate(span, lowered)?,
                        });
                    } else {
                        let checked = check_attributes(child, &[]);
                        self.locate(span, checked)?;
                        // Consume this element's document-order ordinal so later ordinals
                        // stay aligned with source order; the element records none itself.
                        self.order()?;
                        let lowered = self.lower_actions(&child.children, 0);
                        children.push(InvokeChildIr::Finalize {
                            actions: self.locate(span, lowered)?,
                        });
                    }
                }
                other => {
                    return self.fail_at(
                        span,
                        LowerError::UnsupportedElement(crate::safe_value(other)),
                    );
                }
            }
        }
        Ok(InvokeIr {
            document_order: self.order()?,
            type_value: optional_attr(element, "type"),
            typeexpr: optional_attr(element, "typeexpr"),
            src: token_attr(element, "src"),
            srcexpr: optional_attr(element, "srcexpr"),
            xml_base: self.base(element),
            id: token_attr(element, "id"),
            idlocation: optional_attr(element, "idlocation"),
            namelist: optional_attr(element, "namelist")
                .map_or_else(Vec::new, |value| split_tokens(&value)),
            autoforward: match token_attr(element, "autoforward").as_deref() {
                None | Some("false") => false,
                Some("true") => true,
                Some(value) => {
                    return Err(LowerError::InvalidAttributeValue {
                        element: "invoke",
                        attribute: "autoforward",
                        value: crate::safe_value(value),
                    });
                }
            },
            children,
            attributes: invoke_env(element)?,
        })
    }

    fn lower_done_data(&mut self, element: &Element) -> Result<DoneDataIr, LowerError> {
        ensure_count(
            "donedata child nodes",
            element.children.len(),
            crate::MAX_NESTED_ITEMS,
        )?;
        check_attributes(element, &[])?;
        let document_order = self.order()?;
        let mut children = Vec::new();
        for node in &element.children {
            let child = self.structural_element(node)?;
            let Some(child) = child else { continue };
            let span = element_name_span(child);
            if child.name.is(Some(SCXML_NAMESPACE), "param") {
                let lowered = lower_param(child);
                children.push(DoneDataChildIr::Param {
                    value: self.locate(span, lowered)?,
                });
            } else if child.name.is(Some(SCXML_NAMESPACE), "content") {
                let lowered = lower_content(child);
                children.push(DoneDataChildIr::Content {
                    value: self.locate(span, lowered)?,
                });
            } else {
                return self.fail_at(
                    span,
                    LowerError::UnsupportedElement(crate::safe_value(&child.lexical_name)),
                );
            }
        }
        Ok(DoneDataIr {
            document_order,
            children,
            event_schema: event_schema(element, self.resources)?,
        })
    }

    fn lower_actions(
        &mut self,
        nodes: &[Node],
        depth: usize,
    ) -> Result<Vec<ExecutableContentIr>, LowerError> {
        if scxml_types::exceeds_wasm_bound(depth, crate::MAX_CONTENT_DEPTH) {
            return Err(LowerError::LimitExceeded {
                field: "source executable content depth",
                limit: crate::MAX_CONTENT_DEPTH,
            });
        }
        ensure_count(
            "executable child nodes",
            nodes.len(),
            crate::MAX_NESTED_ITEMS,
        )?;
        let mut actions = Vec::new();
        for node in nodes {
            match node {
                Node::Text(text) if scxml_syntax::is_xml_blank(&text.value) => {}
                Node::Text(text) => {
                    return self.fail_at(
                        text.span,
                        LowerError::InvalidStructure("executable content cannot contain free text"),
                    );
                }
                Node::Element(element) => {
                    let lowered = self.lower_action(element, depth);
                    actions.push(self.locate(element_name_span(element), lowered)?);
                }
            }
        }
        Ok(actions)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one function owns the SCXML executable-content element dispatch table"
    )]
    fn lower_action(
        &mut self,
        element: &Element,
        depth: usize,
    ) -> Result<ExecutableContentIr, LowerError> {
        // Consume this element's document-order ordinal so later ordinals
        // stay aligned with source order; the element records none itself.
        self.order()?;
        if element.name.namespace_uri.as_deref() == Some(EVENT_NAMESPACE_URI) {
            return lower_event_action(element);
        }
        if element.name.namespace_uri.as_deref() != Some(SCXML_NAMESPACE) {
            if element.name.namespace_uri.is_none() {
                return Err(LowerError::UnsupportedElement(crate::safe_value(
                    &element.lexical_name,
                )));
            }
            return Ok(ExecutableContentIr::Extension {
                element: lower_xml_element(element, 0)?,
            });
        }
        match element.name.local_name.as_str() {
            "raise" => {
                check_attributes(element, &["event"])?;
                require_no_content(element)?;
                Ok(ExecutableContentIr::Raise {
                    event: required_token_attr(element, "event")?,
                    event_schema: event_schema(element, self.resources)?,
                })
            }
            "log" => {
                check_attributes(element, &["label", "expr"])?;
                require_no_content(element)?;
                Ok(ExecutableContentIr::Log {
                    label: optional_attr(element, "label"),
                    expr: optional_expression_attr(element, "expr"),
                })
            }
            "assign" => {
                check_attributes(element, &["location", "expr", "type", "attr"])?;
                // SCXML 1.0 CR 2014-03-13 §C.3.6: the XPath placement type; absent is the
                // default `replacechildren`, which stays absent in IR.
                let assign_type = match token_attr(element, "type") {
                    None => None,
                    Some(value) => match AssignTypeIr::parse(&value) {
                        Some(AssignTypeIr::Replacechildren) => None,
                        Some(parsed) => Some(parsed),
                        None => {
                            return Err(LowerError::InvalidStructure(
                                "assign type is not an SCXML assign type",
                            ));
                        }
                    },
                };
                let attr = token_attr(element, "attr");
                if attr.is_some() != (assign_type == Some(AssignTypeIr::Addattribute)) {
                    return Err(LowerError::InvalidStructure(
                        "assign attr is required exactly for type addattribute",
                    ));
                }
                // SCXML 5.8: exactly one of `expr` and inline content
                // (none for B.2 `type="delete"`).
                let body = element
                    .children
                    .iter()
                    .filter_map(|node| match node {
                        Node::Element(element) => Some(lower_xml_element(element, 0)),
                        Node::Text(text) if !scxml_syntax::is_xml_blank(&text.value) => {
                            Some(Ok(XmlNodeIr::Text {
                                value: text.value.clone(),
                            }))
                        }
                        Node::Text(_) => None,
                    })
                    .collect::<Result<Vec<_>, LowerError>>()?;
                let expr = optional_attr(element, "expr");
                if expr.is_some() && !body.is_empty() {
                    return Err(LowerError::InvalidStructure(
                        "assign cannot combine expr and inline body",
                    ));
                }
                let delete = assign_type == Some(AssignTypeIr::Delete);
                // SCXML CR 2014 §C.3.6: "delete. Delete the node specified by
                // 'location'. ('expr' is ignored.)" The value of a deletion
                // is not lowered, so it is never evaluated.
                let (expr, body) = if delete {
                    (None, Vec::new())
                } else {
                    (expr, body)
                };
                if !delete && expr.is_none() && body.is_empty() {
                    return Err(LowerError::InvalidStructure(
                        "assign requires expr or inline body",
                    ));
                }
                Ok(ExecutableContentIr::Assign {
                    location: required_expression_attr(element, "location")?,
                    expr,
                    body,
                    assign_type,
                    attr,
                })
            }
            "if" => self.lower_if(element, depth),
            "foreach" => {
                check_attributes(element, &["array", "item", "index"])?;
                Ok(ExecutableContentIr::Foreach {
                    array: required_expression_attr(element, "array")?,
                    item: required_expression_attr(element, "item")?,
                    index: optional_attr(element, "index"),
                    actions: self.lower_actions(&element.children, depth.saturating_add(1))?,
                })
            }
            "send" => self.lower_send(element),
            "cancel" => {
                check_attributes(element, &["sendid", "sendidexpr"])?;
                require_no_content(element)?;
                Ok(ExecutableContentIr::Cancel {
                    sendid: token_attr(element, "sendid"),
                    sendidexpr: optional_attr(element, "sendidexpr"),
                })
            }
            "script" => {
                check_attributes(element, &["src"])?;
                let mut body = String::new();
                for node in &element.children {
                    match node {
                        Node::Text(text) => {
                            let length = body.len().checked_add(text.value.len()).ok_or(
                                LowerError::LimitExceeded {
                                    field: "script body",
                                    limit: crate::MAX_STRING_BYTES,
                                },
                            )?;
                            if scxml_types::exceeds_wasm_bound(length, crate::MAX_STRING_BYTES) {
                                return Err(LowerError::LimitExceeded {
                                    field: "script body",
                                    limit: crate::MAX_STRING_BYTES,
                                });
                            }
                            body.push_str(&text.value);
                        }
                        Node::Element(child) if is_foreign(child) => {}
                        Node::Element(_) => {
                            return Err(LowerError::InvalidStructure(
                                "script cannot contain child elements",
                            ));
                        }
                    }
                }
                // An authored `src` is lowered unpinned; the compiler
                // pins it from a script package (compile-time mode) or
                // leaves it for the host's runtime loader.
                Ok(ExecutableContentIr::Script {
                    src: self
                        .src(element)
                        .map(|uri| crate::ScriptSourceIr { uri, pinned: None }),
                    body,
                })
            }
            other => Err(LowerError::UnsupportedElement(crate::safe_value(other))),
        }
    }

    fn lower_if(
        &mut self,
        element: &Element,
        depth: usize,
    ) -> Result<ExecutableContentIr, LowerError> {
        check_attributes(element, &["cond"])?;
        let mut actions = Vec::new();
        let mut else_if = Vec::new();
        let mut else_actions = None;
        let mut active_else_if: Option<usize> = None;
        for node in &element.children {
            match node {
                Node::Text(text) if scxml_syntax::is_xml_blank(&text.value) => {}
                Node::Text(text) => {
                    return self.fail_at(
                        text.span,
                        LowerError::InvalidStructure(
                            "if executable content cannot contain free text",
                        ),
                    );
                }
                Node::Element(child) if child.name.is(Some(SCXML_NAMESPACE), "elseif") => {
                    let span = element_name_span(child);
                    if else_actions.is_some() {
                        return self.fail_at(
                            span,
                            LowerError::InvalidStructure("elseif appears after else"),
                        );
                    }
                    let checked = check_attributes(child, &["cond"])
                        .and_then(|()| require_no_content(child))
                        .and_then(|()| required_expression_attr(child, "cond"));
                    let condition = self.locate(span, checked)?;
                    else_if.push(ElseIfIr {
                        cond: condition,
                        actions: Vec::new(),
                    });
                    active_else_if = Some(else_if.len() - 1);
                }
                Node::Element(child) if child.name.is(Some(SCXML_NAMESPACE), "else") => {
                    let span = element_name_span(child);
                    if else_actions.is_some() {
                        return self.fail_at(
                            span,
                            LowerError::InvalidStructure("if contains more than one else"),
                        );
                    }
                    let checked =
                        check_attributes(child, &[]).and_then(|()| require_no_content(child));
                    self.locate(span, checked)?;
                    else_actions = Some(Vec::new());
                    active_else_if = None;
                }
                Node::Element(child) => {
                    let lowered = self.lower_action(child, depth.saturating_add(1));
                    let action = self.locate(element_name_span(child), lowered)?;
                    if let Some(values) = else_actions.as_mut() {
                        values.push(action);
                    } else if let Some(index) = active_else_if {
                        else_if
                            .get_mut(index)
                            .ok_or(LowerError::InvalidStructure(
                                "active elseif branch is missing",
                            ))?
                            .actions
                            .push(action);
                    } else {
                        actions.push(action);
                    }
                }
            }
        }
        Ok(ExecutableContentIr::If {
            cond: required_expression_attr(element, "cond")?,
            actions,
            elseif: else_if,
            else_actions,
        })
    }

    fn lower_send(&mut self, element: &Element) -> Result<ExecutableContentIr, LowerError> {
        ensure_count(
            "send child nodes",
            element.children.len(),
            crate::MAX_NESTED_ITEMS,
        )?;
        check_attributes(
            element,
            &[
                "event",
                "eventexpr",
                "target",
                "targetexpr",
                "type",
                "typeexpr",
                "id",
                "idlocation",
                "delay",
                "delayexpr",
                "namelist",
            ],
        )?;
        let mut params = Vec::new();
        let mut content = None;
        for node in &element.children {
            let child = self.structural_element(node)?;
            let Some(child) = child else { continue };
            let span = element_name_span(child);
            if child.name.is(Some(SCXML_NAMESPACE), "param") {
                let lowered = lower_param(child);
                params.push(self.locate(span, lowered)?);
            } else if child.name.is(Some(SCXML_NAMESPACE), "content") {
                if content.is_some() {
                    return self.fail_at(
                        span,
                        LowerError::InvalidStructure("send contains more than one content element"),
                    );
                }
                let lowered = lower_content(child);
                content = Some(self.locate(span, lowered)?);
            } else {
                return self.fail_at(
                    span,
                    LowerError::UnsupportedElement(crate::safe_value(&child.lexical_name)),
                );
            }
        }
        Ok(ExecutableContentIr::Send {
            value: Box::new(SendIr {
                event: token_attr(element, "event"),
                eventexpr: optional_attr(element, "eventexpr"),
                target: token_attr(element, "target"),
                targetexpr: optional_attr(element, "targetexpr"),
                type_value: optional_attr(element, "type"),
                typeexpr: optional_attr(element, "typeexpr"),
                id: token_attr(element, "id"),
                idlocation: optional_attr(element, "idlocation"),
                delay: optional_attr(element, "delay"),
                delayexpr: optional_attr(element, "delayexpr"),
                namelist: optional_attr(element, "namelist")
                    .map_or_else(Vec::new, |value| split_tokens(&value)),
                params,
                content,
                event_schema: event_schema(element, self.resources)?,
            }),
        })
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the final boundary takes each root-level canonical field explicitly"
    )]
    fn finish(
        self,
        root_initial: PendingInitial,
        name: Option<String>,
        data_model: Option<String>,
        binding: Binding,
        data: Vec<DataIr>,
        scripts: Vec<ActionBlockIr>,
        extension_namespaces: Vec<String>,
    ) -> Result<MachineIr, LowerError> {
        let resolve_initial = |pending: PendingInitial| -> Result<InitialIr, LowerError> {
            Ok(InitialIr {
                document_order: pending.document_order,
                targets: resolve_targets(&pending.target_ids, &self.ids)?,
                actions: pending.actions,
            })
        };
        let mut states = self.states;
        for (state, pending) in states.iter_mut().zip(self.state_initials) {
            state.initial = pending.map(&resolve_initial).transpose()?;
        }
        let transitions = self
            .transitions
            .into_iter()
            .map(|pending| {
                Ok(TransitionIr {
                    document_order: pending.document_order,
                    source: pending.source,
                    targets: resolve_targets(&pending.target_ids, &self.ids)?,
                    events: pending.events,
                    cond: pending.condition,
                    transition_type: pending.transition_type,
                    actions: pending.actions,
                    event_schema: pending.event_schema,
                })
            })
            .collect::<Result<Vec<_>, LowerError>>()?;
        let machine = MachineIr {
            version: crate::MACHINE_IR_VERSION,
            name,
            data_model,
            binding,
            initial: resolve_initial(root_initial)?,
            states,
            transitions,
            data,
            scripts,
            extension_namespaces,
        };
        machine.validate()?;
        Ok(machine)
    }
}

/// Lowers a namespace-resolved SCXML document without dropping source constructs.
///
/// # Errors
///
/// Returns a typed error for malformed structure, unsupported SCXML constructs,
/// invalid references, exceeded bounds, or an illegal state graph.
pub fn lower(document: &Document) -> Result<MachineIr, LowerError> {
    lower_with_event_schemas(document, &Resources::new())
}

/// [`lower`], with the bytes of each schema file the document's
/// `event:schema` attributes reference, keyed by the reference's resource
/// part as authored ([`scxml_event_schema::Source::Reference`]). [`lower`]
/// supplies none, so it lowers only inline schemas.
///
/// # Errors
///
/// As [`lower`], plus an `event:schema` that does not resolve.
pub fn lower_with_event_schemas(
    document: &Document,
    resources: &Resources,
) -> Result<MachineIr, LowerError> {
    lower_located(document, resources).map_err(|(error, _)| error)
}

/// [`lower`], with the source span of the construct whose lowering failed:
/// the name of the innermost responsible element (or the offending text),
/// or `None` when the failure is a whole-machine invariant (a Machine IR
/// bound or graph check) that no single element owns.
pub(crate) fn lower_located(
    document: &Document,
    resources: &Resources,
) -> Result<MachineIr, (LowerError, Option<Span>)> {
    let span = observe(Module::CompilerCore, Operation::CompilerLower);
    let mut builder = Builder::new(resources);
    document.root.visit_with_base(&mut |element, base| {
        if let Some(base) = base {
            builder.bases.insert(element.span.start, base.to_owned());
        }
    });
    let result = lower_inner(document, &mut builder);
    if result.is_err() {
        span.event(SignalEvent::InputRejected);
        span.fail(ErrorCategory::Lowering);
    } else {
        span.event(SignalEvent::TransformationCompleted);
    }
    result.map_err(|error| (error, builder.failure))
}

#[allow(
    clippy::too_many_lines,
    reason = "the root source-order pass handles the complete closed set of SCXML root children"
)]
fn lower_inner(document: &Document, builder: &mut Builder<'_>) -> Result<MachineIr, LowerError> {
    ensure_count(
        "root child nodes",
        document.root.children.len(),
        crate::MAX_NESTED_ITEMS,
    )?;
    if !document.root.name.is(Some(SCXML_NAMESPACE), "scxml") {
        return Err(LowerError::InvalidStructure(
            "document root must be the canonical SCXML element",
        ));
    }
    // `exmode` (`lax` or `strict`) is declared by the W3C schema and given no
    // semantics by the Recommendation, so it is accepted and lowers to
    // nothing.
    check_attributes(
        &document.root,
        &[
            "initial",
            "name",
            "version",
            "datamodel",
            "binding",
            "exmode",
        ],
    )?;
    match token_attr(&document.root, "version").as_deref() {
        Some("1.0") => {}
        Some(value) => {
            return Err(LowerError::InvalidAttributeValue {
                element: "scxml",
                attribute: "version",
                value: crate::safe_value(value),
            });
        }
        None => {
            return Err(LowerError::MissingAttribute {
                element: "scxml".into(),
                attribute: "version",
            });
        }
    }
    let binding = match token_attr(&document.root, "binding").as_deref() {
        None | Some("early") => Binding::Early,
        Some("late") => Binding::Late,
        Some(value) => {
            return Err(LowerError::InvalidAttributeValue {
                element: "scxml",
                attribute: "binding",
                value: crate::safe_value(value),
            });
        }
    };
    let root_order = builder.order()?;
    let mut data = Vec::new();
    let mut scripts = Vec::new();
    for node in &document.root.children {
        let child = builder.structural_element(node)?;
        let Some(child) = child else { continue };
        let span = element_name_span(child);
        if child.name.namespace_uri.as_deref() != Some(SCXML_NAMESPACE) {
            return builder.fail_at(
                span,
                LowerError::UnsupportedElement(crate::safe_value(&child.lexical_name)),
            );
        }
        match child.name.local_name.as_str() {
            "state" | "parallel" | "final" | "history" => {
                let lowered = builder.lower_state(child, None, 0);
                builder.locate(span, lowered)?;
            }
            "datamodel" => {
                let checked = check_attributes(child, &[]);
                builder.locate(span, checked)?;
                // Consume this element's document-order ordinal so later ordinals
                // stay aligned with source order; the element records none itself.
                builder.order()?;
                let lowered = builder.lower_datamodel(child);
                data.extend(builder.locate(span, lowered)?);
            }
            "script" => {
                let document_order = builder.order()?;
                let lowered = builder.lower_action(child, 0);
                let action = builder.locate(span, lowered)?;
                scripts.push(ActionBlockIr {
                    document_order,
                    actions: vec![action],
                });
            }
            other => {
                return builder.fail_at(
                    span,
                    LowerError::UnsupportedElement(crate::safe_value(other)),
                );
            }
        }
    }
    if builder.states.is_empty() {
        return Err(LowerError::NoStates);
    }
    let root_target_ids = if let Some(initial) = token_attr(&document.root, "initial") {
        split_tokens(&initial)
    } else {
        vec![
            builder
                .states
                .iter()
                .find(|state| {
                    state.parent.is_none()
                        && !matches!(
                            state.kind,
                            StateKind::ShallowHistory | StateKind::DeepHistory
                        )
                })
                .and_then(|state| state.id.clone())
                .ok_or(LowerError::NoStates)?,
        ]
    };
    if root_target_ids.is_empty() {
        return Err(LowerError::UnknownInitial(String::new()));
    }
    // Only custom actions run in an extension component; any other markup
    // from another namespace has no behavior to bind.
    let extension_namespaces = scxml_validator_core::extension_namespaces(document);
    // The whole-machine checks own no single element, so the failure span
    // stays unset for them.
    std::mem::replace(builder, Builder::new(builder.resources)).finish(
        PendingInitial {
            document_order: root_order,
            target_ids: root_target_ids,
            actions: Vec::new(),
        },
        token_attr(&document.root, "name"),
        // SCXML 3.2.1: the `datamodel` default is "platform-specific"; the
        // IR records the data model the document uses either way.
        Some(
            token_attr(&document.root, "datamodel")
                .unwrap_or_else(|| scxml_validator_core::DEFAULT_DATA_MODEL.to_owned()),
        ),
        binding,
        data,
        scripts,
        extension_namespaces,
    )
}

/// Resolves a target list to the set of states it names, in first-listed
/// order. SCXML 1.0 §3.11 defines "a legal state specification to be a set
/// of states such that 1) no state is an ancestor of any other state on the
/// list, and 2) a full legal state configuration results when all ancestors
/// and default initial descendants have been added": a state listed twice
/// is one member of that set, so the repeat is dropped here and the Machine
/// IR lists each target once. The list is bounded before the repeats are
/// dropped, so a listed state costs the same whether or not it repeats.
fn resolve_targets(
    target_ids: &[String],
    ids: &BTreeMap<String, u32>,
) -> Result<Vec<u32>, LowerError> {
    ensure_count("target list", target_ids.len(), crate::MAX_NESTED_ITEMS)?;
    let mut targets = Vec::with_capacity(target_ids.len());
    for id in target_ids {
        let target = ids
            .get(id)
            .copied()
            .ok_or_else(|| LowerError::UnknownTarget(crate::safe_value(id)))?;
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    Ok(targets)
}

/// The items of an `IDREFS` or event-descriptor list value, separated by
/// XML white space only ([`xml_tokens`]).
fn split_tokens(value: &str) -> Vec<String> {
    xml_tokens(value).map(str::to_owned).collect()
}

/// A required expression attribute, kept even when empty: an empty `cond`,
/// `array`, `item`, or `location` is an ill-formed expression, whose outcome
/// SCXML defines at run time ("MAY wait and place 'error.execution' in the
/// internal event queue at runtime when the expressions are evaluated", 5.9.4),
/// so the document is not rejected (the `SCXML066` warning).
fn required_expression_attr(element: &Element, name: &'static str) -> Result<String, LowerError> {
    element
        .attribute(name)
        .map(|attribute| attribute.value.clone())
        .ok_or_else(|| LowerError::MissingAttribute {
            element: crate::safe_value(&element.lexical_name),
            attribute: name,
        })
}

/// The `type` of one `<transition>` (SCXML 3.5.1: "internal" or
/// "external", default "external").
fn transition_type(element: &Element) -> Result<TransitionType, LowerError> {
    match token_attr(element, "type").as_deref() {
        None | Some("external") => Ok(TransitionType::External),
        Some("internal") => Ok(TransitionType::Internal),
        Some(value) => Err(LowerError::InvalidAttributeValue {
            element: "transition",
            attribute: "type",
            value: crate::safe_value(value),
        }),
    }
}

/// An attribute whose XSD type collapses white space (`xsd:token` and the
/// types derived from it: `NMTOKEN`, `NMTOKENS`, `ID`, `IDREF`, `IDREFS`,
/// and the SCXML enumerations and event types; `xsd:anyURI`;
/// `xsd:decimal`): its value in the type's value space, the
/// [`collapse_whitespace`] of its normalized attribute value.
fn token_attr(element: &Element, name: &str) -> Option<String> {
    element
        .attribute(name)
        .map(|attribute| collapse_whitespace(&attribute.value).into_owned())
}

/// A required attribute; an empty value is missing. An `IDREFS` value
/// (`<initial><transition target>`) is split into its tokens afterwards.
fn required_attr(element: &Element, name: &'static str) -> Result<String, LowerError> {
    element
        .attribute(name)
        .map(|attribute| attribute.value.clone())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| LowerError::MissingAttribute {
            element: crate::safe_value(&element.lexical_name),
            attribute: name,
        })
}

/// A required [`token_attr`]; an empty collapsed value is missing.
fn required_token_attr(element: &Element, name: &'static str) -> Result<String, LowerError> {
    token_attr(element, name)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| LowerError::MissingAttribute {
            element: crate::safe_value(&element.lexical_name),
            attribute: name,
        })
}

fn optional_attr(element: &Element, name: &str) -> Option<String> {
    element
        .attribute(name)
        .map(|attribute| attribute.value.clone())
}

/// An optional expression attribute whose blank value means "no value":
/// only `<log expr>` (SCXML 4.4 lets `<log>` omit `expr`, and the upstream
/// W3C IRP case 307 authors `<log expr="">` for its no-error results). A
/// blank value lowers exactly as the absent attribute. Every other
/// expression attribute, including `<transition cond>`, keeps its blank
/// text: a blank expression is invalid, so it raises `error.execution` when
/// evaluated (SCXML 5.9; a condition is then false, SCXML 3.13).
fn optional_expression_attr(element: &Element, name: &str) -> Option<String> {
    element
        .attribute(name)
        .filter(|attribute| !scxml_syntax::is_xml_blank(&attribute.value))
        .map(|attribute| attribute.value.clone())
}

fn check_attributes(element: &Element, allowed: &[&str]) -> Result<(), LowerError> {
    ensure_count(
        "element attributes",
        element.attributes.len(),
        crate::MAX_NESTED_ITEMS,
    )?;
    for attribute in &element.attributes {
        // The value lowering stores: a white-space-collapsing attribute's
        // collapsed form, any other attribute's exact text.
        let stored = if attribute.name.namespace_uri.is_none()
            && scxml_validator_core::collapses_whitespace(
                &element.name.local_name,
                &attribute.name.local_name,
            ) {
            scxml_syntax::collapse_whitespace(&attribute.value).len()
        } else {
            attribute.value.len()
        };
        if scxml_types::exceeds_wasm_bound(stored, crate::MAX_STRING_BYTES) {
            return Err(LowerError::LimitExceeded {
                field: "source attribute value",
                limit: crate::MAX_STRING_BYTES,
            });
        }
        // An attribute in another namespace (`xml:lang`, `xsi:*`, a
        // package's, or one no package binds, which the validator's
        // `LINT022` reports) carries no SCXML behavior: SCXML 1.0 and its
        // schema allow it (`##other`), and lowering has nothing to keep.
        // `xml:base` took effect on the `src` values it governs, and the
        // `event` and `env` vocabularies lower on their owner elements.
        if attribute.name.namespace_uri.is_some() {
            continue;
        }
        if !allowed.contains(&attribute.name.local_name.as_str()) {
            return Err(LowerError::UnsupportedAttribute {
                element: crate::safe_value(&element.lexical_name),
                attribute: crate::safe_value(&attribute.lexical_name),
            });
        }
    }
    Ok(())
}

/// Whether `attribute` is an `env` attribute ([`scxml_types::ENV_NAMESPACE_URI`])
/// on an `<invoke>` (lowered by [`invoke_env`]).
fn is_invoke_env_attribute(element: &Element, attribute: &scxml_syntax::Attribute) -> bool {
    attribute.name.namespace_uri.as_deref() == Some(scxml_types::ENV_NAMESPACE_URI)
        && element.name.is(Some(SCXML_NAMESPACE), "invoke")
}

/// The `env` attributes of one `<invoke>`, sorted by local name:
/// `env:<slot>` names a variable and `env:<slot>expr` an expression
/// evaluating to one (a slot never ends in `expr`). Only an invoked
/// service's `<invoke>` binds slots; which slots its package declares is the
/// validator's `ENV001`.
fn invoke_env(element: &Element) -> Result<Vec<crate::XmlAttributeIr>, LowerError> {
    let mut attributes: Vec<crate::XmlAttributeIr> = Vec::new();
    let mut slots = std::collections::BTreeSet::new();
    for attribute in &element.attributes {
        if !is_invoke_env_attribute(element, attribute) {
            continue;
        }
        let local = attribute.name.local_name.as_str();
        let (slot, expression) = crate::ir::invoke_env_slot(local);
        if !slots.insert(slot.to_owned()) {
            return Err(LowerError::InvalidStructure(
                "an invoke binds one env slot twice",
            ));
        }
        let valid = scxml_validator_core::is_env_slot(slot)
            && (expression || scxml_validator_core::is_env_name(&attribute.value));
        if !valid {
            return Err(LowerError::InvalidStructure(
                "an invoke env slot is not an env-slots name bound to a variable name",
            ));
        }
        attributes.push(crate::XmlAttributeIr {
            name: crate::ExpandedNameIr {
                namespace_uri: Some(scxml_types::ENV_NAMESPACE_URI.to_owned()),
                local_name: local.to_owned(),
            },
            value: attribute.value.clone(),
        });
    }
    if attributes.is_empty() {
        return Ok(attributes);
    }
    let service = match optional_attr(element, "type") {
        Some(type_value) => !scxml_types::is_scxml_invoke_type(&type_value),
        None => element.attribute("typeexpr").is_some(),
    };
    if !service {
        return Err(LowerError::InvalidStructure(
            "only an invoked service's invoke binds env slots",
        ));
    }
    attributes.sort_by(|first, second| first.name.local_name.cmp(&second.name.local_name));
    Ok(attributes)
}

/// The `event` vocabulary of one owner element: `event:schema` resolved
/// (inline, or from `resources`) into canonical JSON with its SHA-256, or
/// `event:schemaexpr` carried unevaluated. Lowering stores it as metadata
/// only; nothing in the engine reads it.
fn event_schema(
    element: &Element,
    resources: &Resources,
) -> Result<Option<EventSchemaIr>, LowerError> {
    let find = |name: &str| {
        element.attributes.iter().find(|attribute| {
            attribute.name.namespace_uri.as_deref() == Some(EVENT_NAMESPACE_URI)
                && attribute.name.local_name == name
        })
    };
    match (find(SCHEMA_ATTRIBUTE), find(SCHEMA_EXPR_ATTRIBUTE)) {
        (None, None) => Ok(None),
        (Some(_), Some(_)) => Err(LowerError::InvalidStructure(
            "event:schema and event:schemaexpr are mutually exclusive",
        )),
        (None, Some(expression)) => Ok(Some(EventSchemaIr::Schemaexpr {
            expr: expression.value.clone(),
        })),
        (Some(schema), None) => {
            let resolved = match scxml_event_schema::source(&schema.value) {
                Source::Inline(text) => scxml_event_schema::resolve(text.as_bytes(), None),
                Source::Reference { resource, fragment } => {
                    if scxml_event_schema::resource_problem(resource).is_some() {
                        return Err(LowerError::InvalidEventSchema(
                            "the schema reference names no file relative to the document".into(),
                        ));
                    }
                    let bytes = resources.get(resource).ok_or_else(|| {
                        LowerError::InvalidEventSchema(format!(
                            "no bytes were supplied for schema file `{}`",
                            crate::safe_value(resource)
                        ))
                    })?;
                    scxml_event_schema::resolve(bytes, fragment)
                }
            }
            .map_err(|error| {
                LowerError::InvalidEventSchema(crate::safe_value(&error.to_string()))
            })?;
            Ok(Some(EventSchemaIr::Schema {
                schema: resolved.schema,
                sha256: resolved.sha256,
            }))
        }
    }
}

/// Span of `element`'s name in its start tag: XML puts the name directly
/// after `<`, so it is `lexical_name.len()` bytes from there.
fn element_name_span(element: &Element) -> Span {
    let start = element.start_tag_span.start.saturating_add(1);
    Span::new(
        element.start_tag_span.source,
        start,
        start
            .saturating_add(element.lexical_name.len())
            .min(element.start_tag_span.end),
    )
}

/// Whether `element` is in a namespace other than SCXML's (an element in
/// no namespace is not: the schema rejects it, and so does lowering).
fn is_foreign(element: &Element) -> bool {
    element
        .name
        .namespace_uri
        .as_deref()
        .is_some_and(|namespace| namespace != SCXML_NAMESPACE)
}

/// The `event` vocabulary's one element, `<event:defer/>`: no attributes
/// and no content (`namespaces/event`, "Deferred events").
fn lower_event_action(element: &Element) -> Result<ExecutableContentIr, LowerError> {
    if element.name.local_name != "defer" {
        return Err(LowerError::UnsupportedElement(crate::safe_value(
            &element.lexical_name,
        )));
    }
    if !element.attributes.is_empty()
        || element.children.iter().any(|node| match node {
            Node::Text(text) => !scxml_syntax::is_xml_blank(&text.value),
            Node::Element(_) => true,
        })
    {
        return Err(LowerError::InvalidStructure(
            "defer has no attributes and no content",
        ));
    }
    Ok(ExecutableContentIr::Defer)
}

fn require_no_content(element: &Element) -> Result<(), LowerError> {
    if element.children.iter().all(|node| match node {
        Node::Text(text) => scxml_syntax::is_xml_blank(&text.value),
        Node::Element(child) => is_foreign(child),
    }) {
        Ok(())
    } else {
        Err(LowerError::InvalidStructure(
            "element does not permit child content",
        ))
    }
}

fn lower_param(element: &Element) -> Result<ParamIr, LowerError> {
    check_attributes(element, &["name", "expr", "location"])?;
    require_no_content(element)?;
    Ok(ParamIr {
        name: required_token_attr(element, "name")?,
        expr: optional_attr(element, "expr"),
        location: optional_attr(element, "location"),
    })
}

fn lower_content(element: &Element) -> Result<ContentIr, LowerError> {
    check_attributes(element, &["expr"])?;
    Ok(ContentIr {
        expr: optional_attr(element, "expr"),
        body: lower_xml_nodes(&element.children, 0)?,
    })
}

fn lower_xml_nodes(nodes: &[Node], depth: usize) -> Result<Vec<XmlNodeIr>, LowerError> {
    if scxml_types::exceeds_wasm_bound(depth, crate::MAX_CONTENT_DEPTH) {
        return Err(LowerError::LimitExceeded {
            field: "source XML depth",
            limit: crate::MAX_CONTENT_DEPTH,
        });
    }
    ensure_count("inline XML nodes", nodes.len(), crate::MAX_NESTED_ITEMS)?;
    nodes
        .iter()
        .map(|node| match node {
            Node::Text(text) => {
                if scxml_types::exceeds_wasm_bound(text.value.len(), crate::MAX_STRING_BYTES) {
                    return Err(LowerError::LimitExceeded {
                        field: "inline XML text",
                        limit: crate::MAX_STRING_BYTES,
                    });
                }
                Ok(XmlNodeIr::Text {
                    value: text.value.clone(),
                })
            }
            Node::Element(element) => lower_xml_element(element, depth.saturating_add(1)),
        })
        .collect()
}

fn lower_xml_element(element: &Element, depth: usize) -> Result<XmlNodeIr, LowerError> {
    if scxml_types::exceeds_wasm_bound(depth, crate::MAX_CONTENT_DEPTH) {
        return Err(LowerError::LimitExceeded {
            field: "source XML depth",
            limit: crate::MAX_CONTENT_DEPTH,
        });
    }
    Ok(XmlNodeIr::Element {
        name: ExpandedNameIr {
            namespace_uri: element.name.namespace_uri.clone(),
            local_name: element.name.local_name.clone(),
        },
        attributes: element
            .attributes
            .iter()
            .map(|attribute| XmlAttributeIr {
                name: ExpandedNameIr {
                    namespace_uri: attribute.name.namespace_uri.clone(),
                    local_name: attribute.name.local_name.clone(),
                },
                value: attribute.value.clone(),
            })
            .collect(),
        children: lower_xml_nodes(&element.children, depth.saturating_add(1))?,
    })
}

fn ensure_count(field: &'static str, count: usize, limit: usize) -> Result<(), LowerError> {
    if count > limit {
        Err(LowerError::LimitExceeded { field, limit })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, MutexGuard};

    use scxml_syntax::{SourceId, SourceText, parse_xml};
    use scxml_telemetry::{Recorder, install_recorder};

    use super::*;

    /// The source id of every parsed test document.
    const SOURCE: SourceId = SourceId::new(1);

    static RECORDER_LOCK: Mutex<()> = Mutex::new(());

    fn recorder_lock() -> MutexGuard<'static, ()> {
        RECORDER_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn located(body: &str) -> (String, LowerError, Option<String>) {
        let source = format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="xpath" initial="s"><datamodel><data id="x"/></datamodel>{body}<final id="f"/></scxml>"#
        );
        let document = parse_xml(SourceText::new(SOURCE, "located.scxml", source.as_str()))
            .expect("well-formed located-failure fixture must parse");
        let (error, span) =
            lower_located(&document, &Resources::new()).expect_err("fixture must fail lowering");
        let text = span.and_then(|span| source.get(span.start..span.end).map(str::to_owned));
        let before = span.and_then(|span| source.get(..span.start).map(str::to_owned));
        (before.unwrap_or_default(), error, text)
    }

    /// A white-space-collapsing attribute is bounded by its collapsed value,
    /// the one lowering stores: an `id` padded past 1 MiB lowers as `a`. An
    /// `xsd:string` attribute keeps every byte, so the same length is over
    /// the bound.
    #[cfg(target_arch = "wasm32")]
    #[test]
    fn wasm_only_a_padded_collapsing_attribute_is_bounded_by_its_collapsed_value() {
        let padding = " ".repeat(crate::MAX_STRING_BYTES + 1);
        let lowered = |body: &str| {
            let source = format!(
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a">{body}<final id="f"/></scxml>"#
            );
            let document = parse_xml(SourceText::new(SOURCE, "padded.scxml", source.as_str()))
                .expect("the fixture parses");
            lower(&document)
        };
        let padded_id = lowered(&format!(
            r#"<state id="{padding}a"><transition event="go" target="f"/></state>"#
        ))
        .expect("the collapsed id is within the bound");
        assert_eq!(padded_id.states[0].id.as_deref(), Some("a"));
        assert!(matches!(
            lowered(&format!(
                r#"<state id="a"><onentry><log label="{padding}l"/></onentry><transition event="go" target="f"/></state>"#
            )),
            Err(LowerError::LimitExceeded {
                field: "source attribute value",
                ..
            })
        ));
    }

    #[test]
    fn lowering_failures_name_the_responsible_element() {
        // Each rejection is located at the element that causes it (the
        // repeated child, the `<assign>`), not the root start tag.
        for (body, element, prefix_end, message) in [
            (
                r#"<state id="s"><onentry><send event="e"><content>1</content><content>2</content></send></onentry></state>"#,
                "content",
                "<content>1</content><",
                "send contains more than one content element",
            ),
            (
                r#"<state id="s"><invoke type="scxml"><content expr="1"/><content expr="2"/></invoke></state>"#,
                "content",
                r#"<content expr="1"/><"#,
                "invoke permits at most one content and finalize child",
            ),
            (
                r#"<state id="s"><invoke type="scxml" src="c.scxml"><finalize/><finalize/></invoke></state>"#,
                "finalize",
                "<finalize/><",
                "invoke permits at most one content and finalize child",
            ),
            (
                r#"<state id="s"><onentry><assign location="$x" expr="1"><a/></assign></onentry></state>"#,
                "assign",
                "<onentry><",
                "assign cannot combine expr and inline body",
            ),
            (
                r#"<state id="s"><onentry><assign location="$x" type="addattribute" expr="1"/></onentry></state>"#,
                "assign",
                "<onentry><",
                "assign attr is required exactly for type addattribute",
            ),
            (
                r#"<state id="s"><onentry>free text</onentry></state>"#,
                "free text",
                "<onentry>",
                "executable content cannot contain free text",
            ),
        ] {
            let (before, error, text) = located(body);
            assert!(
                error.to_string().contains(message),
                "{body}: unexpected error {error}"
            );
            assert_eq!(text.as_deref(), Some(element), "{body}: {error}");
            assert!(
                before.ends_with(prefix_end),
                "{body}: located after `{before}`"
            );
        }
        // A whole-machine Machine IR check owns no single element.
        let (_, error, text) = located(
            r#"<state id="s"><state id="a"><transition event="e" target="a b"/></state><state id="b"/></state>"#,
        );
        assert!(matches!(error, LowerError::InvalidStructure(_)), "{error}");
        assert_eq!(text, None);
    }

    #[test]
    fn initial_and_history_transitions_accept_a_type() {
        // SCXML 3.6.2 and 3.10.2 forbid only "'cond' or 'event' attributes"
        // on these transitions, so both `type` values lower.
        let document = parse_xml(SourceText::new(
            SOURCE,
            "typed.scxml",
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><state id="a"><initial><transition type="internal" target="a1"/></initial><history id="h"><transition type="internal" target="a1"/></history><state id="a1"/></state><state id="b"><initial><transition type="external" target="b1"/></initial><state id="b1"/></state></scxml>"#,
        ))
        .expect("typed initial/history fixture must parse");
        lower(&document).expect("a type on an initial or history transition is conformant");
        let document = parse_xml(SourceText::new(
            SOURCE,
            "bad-type.scxml",
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><state id="a"><initial><transition type="sideways" target="a1"/></initial><state id="a1"/></state></scxml>"#,
        ))
        .expect("bad-type fixture must parse");
        assert!(matches!(
            lower(&document),
            Err(LowerError::InvalidAttributeValue {
                attribute: "type",
                ..
            })
        ));
    }

    #[test]
    fn an_xpath_deletion_ignores_its_value() {
        // SCXML CR 2014 §C.3.6: "delete. Delete the node specified by
        // 'location'. ('expr' is ignored.)"
        for assign in [
            r#"<assign location="$x/a" type="delete" expr="no-such-function()"/>"#,
            r#"<assign location="$x/a" type="delete"><b/></assign>"#,
        ] {
            let document = parse_xml(SourceText::new(
                SOURCE,
                "delete.scxml",
                format!(
                    r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="xpath"><datamodel><data id="x"><a/></data></datamodel><state id="s"><onentry>{assign}</onentry></state></scxml>"#
                ),
            ))
            .expect("delete fixture must parse");
            let machine = lower(&document).expect("a deletion with a value is conformant");
            let actions = &machine.states[0].onentry[0].actions;
            assert!(
                matches!(
                    actions.as_slice(),
                    [ExecutableContentIr::Assign { expr: None, body, assign_type: Some(AssignTypeIr::Delete), .. }] if body.is_empty()
                ),
                "{assign}: {actions:?}"
            );
        }
    }

    #[test]
    fn anonymous_states_get_generated_ids() {
        let document = parse_xml(SourceText::new(
            SOURCE,
            "anonymous.scxml",
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><state><transition event="go" target="f"/></state><parallel><state id="r1"/><state id="r2"/></parallel><final id="f"/><state id="h"><history><transition target="h1"/></history><state id="h1"/></state></scxml>"#,
        ))
        .expect("anonymous-state fixture must parse");
        // SCXML 1.0 §3.14: "When such an attribute is defined to be optional
        // and the author omits it, then, for elements other than <send> and
        // <invoke>, the SCXML processor MUST generate a unique id
        // automatically at document load time."
        let machine = lower(&document).expect("SCXML 1.0 makes every state id optional");
        let ids: Vec<_> = machine
            .states
            .iter()
            .map(|state| state.id.clone())
            .collect();
        assert_eq!(
            ids,
            [
                Some("$state0".to_owned()),
                Some("$parallel1".to_owned()),
                Some("r1".to_owned()),
                Some("r2".to_owned()),
                Some("f".to_owned()),
                Some("h".to_owned()),
                Some("$history6".to_owned()),
                Some("h1".to_owned()),
            ]
        );
    }

    #[test]
    fn lowering_failure_is_one_boundary_observation() {
        let document = parse_xml(SourceText::new(
            SOURCE,
            "telemetry.scxml",
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><state id="s"><unsupported/></state></scxml>"#,
        ))
        .expect("well-formed lowering telemetry fixture must parse");
        let _lock = recorder_lock();
        let recorder = Recorder::default();
        let _guard = install_recorder(recorder.clone());

        lower(&document).expect_err("unsupported state child must fail closed");

        let observations: Vec<_> = recorder
            .observations()
            .into_iter()
            .filter(|observation| observation.module == Module::CompilerCore)
            .collect();
        assert_eq!(
            observations.len(),
            1,
            "only the lower boundary emits; nested helpers run inside its span"
        );
        let lowered = observations
            .first()
            .expect("the lower boundary observation is recorded");
        assert_eq!(lowered.operation, Operation::CompilerLower);
        assert_eq!(lowered.error, Some(ErrorCategory::Lowering));
        assert_eq!(lowered.events, [SignalEvent::InputRejected]);
    }
}
