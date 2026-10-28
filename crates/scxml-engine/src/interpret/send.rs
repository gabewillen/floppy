//! `<send>` (SCXML 6.2, C.1, C.2) and `<cancel>` (6.3).

use scxml_compiler_core::{SendIr, XmlNodeIr};
use scxml_types::{
    AssignPlacement, AssignSource, DataValue, Event, EventName, FormValueSource, NamedValue,
    ProcessorKind, SendId, SendMessage, SendRequest, classify_processor_type,
};

use super::{
    CapabilityErrorKind, FailureCategory, Interpreter, ProcessorSend, SendDisposition, SendTarget,
    SessionCapabilities, inline_body, normalize_whitespace, recover,
};
use crate::{EffectFailure, Engine, PlatformError};

impl<C: SessionCapabilities> Interpreter<'_, C> {
    /// Evaluates one string-typed attribute (`typeexpr`, `eventexpr`,
    /// `targetexpr`, `delayexpr`, `sendidexpr`); `None` is a non-string
    /// value. A recoverable evaluation failure aborts the block.
    fn text(
        &mut self,
        expression: &str,
        event: Option<&Event>,
    ) -> Result<Option<String>, EffectFailure<C::Error>> {
        self.caps
            .evaluate_text(expression, event)
            .map_err(recover::<C>)
    }

    /// A literal attribute, else its expression's text; `Ok(None)` when
    /// neither is present. A non-string expression value aborts the block
    /// with `error.execution` (SCXML 6.2).
    fn literal_or_text(
        &mut self,
        literal: Option<&String>,
        expression: Option<&String>,
        event: Option<&Event>,
    ) -> Result<Option<String>, EffectFailure<C::Error>> {
        match (literal, expression) {
            (Some(value), _) => Ok(Some(value.clone())),
            (None, Some(expression)) => match self.text(expression, event)? {
                Some(value) => Ok(Some(value)),
                None => Err(EffectFailure::Block(PlatformError::Execution)),
            },
            (None, None) => Ok(None),
        }
    }

    /// The `delay`/`delayexpr` in milliseconds (SCXML 6.2.3: a CSS2 time
    /// designation); `None` for an illegal value. A recoverable evaluation
    /// failure aborts the block.
    fn delay_ms(
        &mut self,
        send: &SendIr,
        event: Option<&Event>,
    ) -> Result<Option<u64>, EffectFailure<C::Error>> {
        Ok(match (&send.delay, &send.delayexpr) {
            (Some(value), _) => scxml_types::parse_delay_ms(value).ok(),
            (None, Some(expression)) => self
                .text(expression, event)?
                .and_then(|value| scxml_types::parse_delay_ms(&value).ok()),
            (None, None) => Some(0),
        })
    }

    /// Stores a generated id at `idlocation` (SCXML 6.2.4); a recoverable
    /// failure aborts the block.
    fn store_generated_id(
        &mut self,
        location: &str,
        id: &str,
        event: Option<&Event>,
    ) -> Result<(), EffectFailure<C::Error>> {
        let context = event.and_then(|event| event.trace_context.as_ref());
        self.caps
            .assign(
                location,
                &AssignSource::Value(DataValue::String(id.to_owned())),
                &AssignPlacement::ReplaceChildren,
                event,
                context,
            )
            .map_err(recover::<C>)
    }

    /// A new platform-generated send id (SCXML 3.14: generated each time a
    /// `<send>` without an authored `id` executes).
    fn generated_send_id(&mut self) -> Result<SendId, EffectFailure<C::Error>> {
        let generated = self.state.next_send_id();
        SendId::new(generated).map_err(|_| self.fatal(FailureCategory::Harness, "send-id-invalid"))
    }

    /// Applies one `<send>`.
    ///
    /// The send's identity comes first (SCXML 3.14, 6.2.4): the authored
    /// `id`, else for `idlocation` an id generated and stored when the
    /// element is evaluated. Every recoverable error the send then raises
    /// (`error.execution` or `error.communication`, whichever processor it
    /// selects) carries that id as `_event.sendid` (SCXML 5.10.1, official
    /// case 332); a send the author gave neither attribute gets its
    /// generated id when it first needs one (a delay, a `BasicHTTP`
    /// request, or an error).
    pub(super) fn send(
        &mut self,
        engine: &mut Engine,
        send: &SendIr,
        event: Option<&Event>,
    ) -> Result<(), EffectFailure<C::Error>> {
        let mut send_id = match &send.id {
            Some(literal) => match SendId::new(literal.clone()) {
                Ok(id) => Some(id),
                Err(_) => return Err(self.fatal(FailureCategory::Harness, "send-id-invalid")),
            },
            None => None,
        };
        let applied = match (&send_id, &send.idlocation) {
            (None, Some(location)) => match self.generated_send_id() {
                Ok(generated) => {
                    let stored = self.store_generated_id(location, generated.as_str(), event);
                    send_id = Some(generated);
                    stored.and_then(|()| self.send_identified(engine, send, event, &mut send_id))
                }
                Err(failure) => Err(failure),
            },
            _ => self.send_identified(engine, send, event, &mut send_id),
        };
        match applied {
            Err(EffectFailure::Block(error)) => {
                let id = match send_id {
                    Some(id) => id,
                    None => self.generated_send_id()?,
                };
                Err(EffectFailure::SendBlock(error, id))
            }
            other => other,
        }
    }

    /// Applies one `<send>` whose identity is `send_id` (see [`Self::send`];
    /// `None` until a generated id is needed).
    ///
    /// Evaluation order and error precedence (SCXML 6.2, 5.10): processor
    /// type, event name, target, target classification, delay, payload,
    /// then delivery.
    #[allow(
        clippy::too_many_lines,
        reason = "one ordered evaluation of every send attribute and child keeps SCXML 6.2 error precedence together"
    )]
    fn send_identified(
        &mut self,
        engine: &mut Engine,
        send: &SendIr,
        event: Option<&Event>,
        send_id: &mut Option<SendId>,
    ) -> Result<(), EffectFailure<C::Error>> {
        // The processor type: literal or expression. A type expression
        // whose value is not a string names no type this processor
        // supports: SCXML 6.2.5 "If the SCXML Processor does not support
        // the type that is specified, it MUST place the event
        // error.execution on the internal event queue."
        let processor_type =
            self.literal_or_text(send.type_value.as_ref(), send.typeexpr.as_ref(), event)?;
        // Any other type goes to the processor the host binds to it; a type
        // no processor answers is invalid (SCXML 6.2, official case 199),
        // which the host reports as `invalid-input` (`error.execution`).
        if let Some(other) = processor_type.as_deref()
            && classify_processor_type(other) != ProcessorKind::Scxml
        {
            return self.send_processor(engine, send, event, send_id, other.to_owned());
        }
        // The SCXML processor requires an event name; a non-string or
        // invalid `eventexpr` is illegal (SCXML 6.2.3).
        let event_name = match (&send.event, &send.eventexpr) {
            (Some(name), _) => match EventName::new(name.clone()) {
                Ok(name) => name,
                Err(_) => {
                    return Err(self.fatal(FailureCategory::Harness, "send-event-name-invalid"));
                }
            },
            (None, Some(expression)) => match self.text(expression, event)?.map(EventName::new) {
                Some(Ok(name)) => name,
                _ => return Err(EffectFailure::Block(PlatformError::Execution)),
            },
            (None, None) => return Err(EffectFailure::Block(PlatformError::Execution)),
        };
        let target = self.literal_or_text(send.target.as_ref(), send.targetexpr.as_ref(), event)?;
        // SCXML 6.2: "The Processor MUST evaluate all arguments to <send>
        // when the <send> element is evaluated ... If the evaluation of
        // <send>'s arguments produces an error, the Processor MUST discard
        // the message without attempting to deliver it." Every argument is
        // evaluated before the target is looked up, so an argument error is
        // error.execution even for an unreachable target.
        let Some(delay_ms) = self.delay_ms(send, event)? else {
            return Err(EffectFailure::Block(PlatformError::Execution));
        };
        let (data, message) = self.send_payload(send, event)?;
        // An unanswered session address is error.communication, any other
        // unknown form error.execution (SCXML 6.2.4).
        let delivery = self.caps.classify_target(target.as_deref());
        match delivery {
            SendTarget::Unreachable => {
                return Err(EffectFailure::Block(PlatformError::Communication));
            }
            SendTarget::Invalid => return Err(EffectFailure::Block(PlatformError::Execution)),
            // SCXML 6.2.1: 'delay' and 'delayexpr' "Must not occur ... when
            // the attribute 'target' has the value "_internal"", so a
            // delayed internal send is not a supported target (6.2.4: "If
            // the value of the 'target' or 'targetexpr' attribute is not
            // supported or invalid, the Processor MUST place the error
            // error.execution on the internal event queue").
            SendTarget::Internal if send.delay.is_some() || send.delayexpr.is_some() => {
                return Err(EffectFailure::Block(PlatformError::Execution));
            }
            SendTarget::Internal
            | SendTarget::Local
            | SendTarget::Parent
            | SendTarget::Invocation(_)
            | SendTarget::Session(_) => {}
        }
        // The event's contracts (namespaces/event): data its own literal
        // `event:schema` rejects, or for an event this session delivers to
        // itself the schema of a transition handling it, is not dispatched;
        // SCXML 6.2: "If the evaluation of <send>'s arguments produces an
        // error, the Processor MUST discard the message without attempting
        // to deliver it", and the error is error.execution (5.10, 3.12.2).
        let delivered_here = matches!(delivery, SendTarget::Internal | SendTarget::Local);
        if !engine.admits_event_data(
            event_name.as_str(),
            &data,
            send.event_schema.as_ref(),
            delivered_here,
        ) {
            return Err(EffectFailure::Block(PlatformError::Execution));
        }
        // SCXML C.1: the delivered `_event.sendid` is the send's id when the
        // author gave `id` or `idlocation`, and is left empty otherwise
        // (official case 333), whether or not the send is delayed.
        let authored_send_id = if send.id.is_some() || send.idlocation.is_some() {
            send_id.clone()
        } else {
            None
        };
        // SCXML 6.2: `#_internal` places the event on the internal queue
        // without processor stamping.
        if delivery == SendTarget::Internal {
            let mut delivered = Event::internal(event_name);
            delivered.send_id = authored_send_id;
            delivered.data = data;
            delivered.trace_context.clone_from(&self.outbound);
            return engine
                .enqueue_internal(delivered)
                .map_err(|cause| self.engine_fatal(cause, "engine-enqueue-internal"));
        }
        let mut delivered = Event::external(event_name);
        delivered.data = data;
        delivered.trace_context.clone_from(&self.outbound);
        delivered.send_id = authored_send_id;
        // A delayed send needs a clock id: the authored one, so `<cancel>`
        // stays valid, else one generated for this execution (SCXML 3.14),
        // which the delivered event does not carry.
        if delay_ms > 0 && send_id.is_none() {
            *send_id = Some(self.generated_send_id()?);
        }
        let request = SendRequest {
            send_id: if delay_ms > 0 {
                send_id.clone()
            } else {
                delivered.send_id.clone()
            },
            event: delivered,
            target,
            processor_type,
            delay_ms,
            message,
        };
        match self.caps.send(request) {
            Ok(SendDisposition::EnqueueExternal(event)) => engine
                .enqueue_external(*event)
                .map_err(|cause| self.engine_fatal(cause, "engine-enqueue-external")),
            Ok(SendDisposition::Routed | SendDisposition::Scheduled) => Ok(()),
            Ok(SendDisposition::Undeliverable) => {
                Err(EffectFailure::Block(PlatformError::Communication))
            }
            // The processor rejects a message it cannot carry (for
            // example a raw message over its bound) as error.execution,
            // and so is a delayed send its clock has no room for: SCXML
            // 3.12.2, "it MUST signal any errors that occur by raising
            // SCXML events whose names begin with 'error.'".
            Err(error)
                if matches!(
                    C::error_kind(&error),
                    CapabilityErrorKind::InvalidInput
                        | CapabilityErrorKind::PayloadTooLarge
                        | CapabilityErrorKind::CapacityExhausted
                ) =>
            {
                Err(EffectFailure::Block(PlatformError::Execution))
            }
            Err(error) => Err(EffectFailure::Fatal(error)),
        }
    }

    /// The `_event.data` payload and processor message of one SCXML
    /// processor `<send>` (SCXML 6.2.4-6.2.6): `<param>` then `namelist`
    /// entries become fields (duplicates kept for `_event.raw`, official
    /// case 178), `<content>` becomes the data itself. Content combined
    /// with fields, a param without a source, an illegal `namelist` entry,
    /// and an uncarriable value raise `error.execution`.
    fn send_payload(
        &mut self,
        send: &SendIr,
        event: Option<&Event>,
    ) -> Result<(DataValue, SendMessage), EffectFailure<C::Error>> {
        if !send.params.is_empty() || !send.namelist.is_empty() {
            if send.content.is_some() {
                return Err(EffectFailure::Block(PlatformError::Execution));
            }
            let mut entries = Vec::with_capacity(send.params.len() + send.namelist.len());
            for param in &send.params {
                let value = match (&param.expr, &param.location) {
                    (Some(expression), _) => self.caps.evaluate_value(expression, event),
                    (None, Some(location)) => self.caps.evaluate_location(location, event),
                    (None, None) => return Err(EffectFailure::Block(PlatformError::Execution)),
                }
                .map_err(recover::<C>)?;
                entries.push(NamedValue {
                    name: param.name.clone(),
                    value,
                });
            }
            for location in &send.namelist {
                // SCXML 6.2.4: an illegal entry raises error.execution
                // (official case 553).
                let Some(name) = self.caps.namelist_key(location) else {
                    return Err(EffectFailure::Block(PlatformError::Execution));
                };
                let value = self
                    .caps
                    .evaluate_location(location, event)
                    .map_err(recover::<C>)?;
                entries.push(NamedValue { name, value });
            }
            let Some(payload) = self.caps.event_payload(&entries) else {
                return Err(EffectFailure::Block(PlatformError::Execution));
            };
            return Ok((payload, SendMessage::Params(entries)));
        }
        let Some(content) = &send.content else {
            return Ok((DataValue::Undefined, SendMessage::Empty));
        };
        let value = if let Some(expression) = &content.expr {
            self.caps
                .evaluate_value(expression, event)
                .map_err(recover::<C>)?
        } else {
            // SCXML 6.2.6: element children are an XML value (a DOM in
            // ECMAScript, official 561); ECMAScript reads text as JSON or a
            // string. A body the data model cannot represent is no legal
            // data value (5.9.3), an error the processor signals as
            // error.execution (3.12.2) instead of ending the session.
            match inline_body(&content.body).and_then(|body| self.caps.inline_content(&body)) {
                Some(value) => value,
                None => return Err(EffectFailure::Block(PlatformError::Execution)),
            }
        };
        Ok((value, SendMessage::Content))
    }

    /// Applies one `<send>` addressed to an Event I/O Processor other than
    /// the SCXML one, such as `BasicHTTP` (SCXML C.2) or a processor a
    /// package binds: the interpreter evaluates every expression, the host
    /// routes the request by `processor_type` and the processor encodes and
    /// sends it. An invalid request (or a type no processor answers) raises
    /// `error.execution`, an undeliverable one `error.communication`
    /// (official case 577). The request carries the send's id (generated
    /// when the author gave none), so a failure the host learns of only
    /// after this `<send>` returned still names it as `_event.sendid`
    /// (SCXML 5.10.1).
    fn send_processor(
        &mut self,
        engine: &mut Engine,
        send: &SendIr,
        event: Option<&Event>,
        send_id: &mut Option<SendId>,
        processor_type: String,
    ) -> Result<(), EffectFailure<C::Error>> {
        // The event name is optional (SCXML C.2: a `BasicHTTP` receiver
        // then uses `HTTP.POST`); a non-string `eventexpr` is illegal
        // (SCXML 6.2).
        let event_name =
            self.literal_or_text(send.event.as_ref(), send.eventexpr.as_ref(), event)?;
        let target = self.literal_or_text(send.target.as_ref(), send.targetexpr.as_ref(), event)?;
        let Some(delay_ms) = self.delay_ms(send, event)? else {
            return Err(EffectFailure::Block(PlatformError::Execution));
        };
        // SCXML C.2: `namelist` entries then `<param>` children become
        // form fields in document order, keyed by the data model's
        // `namelist` key.
        let mut fields = Vec::with_capacity(send.namelist.len() + send.params.len());
        for location in &send.namelist {
            let Some(name) = self.caps.namelist_key(location) else {
                return Err(EffectFailure::Block(PlatformError::Execution));
            };
            let value = self
                .caps
                .evaluate_form_value(location, FormValueSource::Location, event)
                .map_err(recover::<C>)?;
            fields.push(NamedValue { name, value });
        }
        for param in &send.params {
            let (source, kind) = match (&param.expr, &param.location) {
                (Some(expression), _) => (expression.as_str(), FormValueSource::Expression),
                (None, Some(location)) => (location.as_str(), FormValueSource::Location),
                (None, None) => return Err(EffectFailure::Block(PlatformError::Execution)),
            };
            let value = self
                .caps
                .evaluate_form_value(source, kind, event)
                .map_err(recover::<C>)?;
            fields.push(NamedValue {
                name: param.name.clone(),
                value,
            });
        }
        // SCXML C.2: "If a <content> child is present, the SCXML Processor
        // MUST use its value as the body of the message": the evaluated
        // `expr`, the inline markup (5.6.2: children "MAY consist of text,
        // XML from any namespace, or a mixture of both"; "the Processor
        // MUST use the children of <content> as the output"), or the
        // whitespace-normalized inline text. Markup over its serialization
        // bound is error.execution.
        let content = match &send.content {
            None => None,
            Some(content) => Some(if let Some(expression) = &content.expr {
                self.caps
                    .evaluate_value(expression, event)
                    .map_err(recover::<C>)?
            } else if content
                .body
                .iter()
                .any(|node| matches!(node, XmlNodeIr::Element { .. }))
            {
                match scxml_compiler_core::xml_markup(&content.body) {
                    Ok(markup) => DataValue::String(markup),
                    Err(_) => return Err(EffectFailure::Block(PlatformError::Execution)),
                }
            } else {
                DataValue::String(normalize_whitespace(&super::inline_text(&content.body)))
            }),
        };
        let send_id = match send_id {
            Some(id) => id.clone(),
            None => send_id.insert(self.generated_send_id()?).clone(),
        };
        let request = ProcessorSend {
            processor_type,
            send_id,
            event_name,
            target,
            params: fields,
            content,
            delay_ms,
            trace_context: self.outbound.clone(),
        };
        match self.caps.send_processor(request) {
            Ok(SendDisposition::EnqueueExternal(event)) => engine
                .enqueue_external(*event)
                .map_err(|cause| self.engine_fatal(cause, "engine-enqueue-external")),
            Ok(SendDisposition::Routed | SendDisposition::Scheduled) => Ok(()),
            Ok(SendDisposition::Undeliverable) => {
                Err(EffectFailure::Block(PlatformError::Communication))
            }
            Err(error)
                if matches!(
                    C::error_kind(&error),
                    CapabilityErrorKind::InvalidInput
                        | CapabilityErrorKind::PayloadTooLarge
                        | CapabilityErrorKind::CapacityExhausted
                ) =>
            {
                Err(EffectFailure::Block(PlatformError::Execution))
            }
            Err(error) => Err(EffectFailure::Fatal(error)),
        }
    }

    /// Applies one `<cancel>` (SCXML 6.3): a literal `sendid`, or the
    /// `sendidexpr` text (a non-string value raises `error.execution`).
    /// Cancelling an unknown or already delivered id is silently ignored by
    /// the processor.
    pub(super) fn cancel(
        &mut self,
        send_id: Option<&str>,
        send_id_expression: Option<&str>,
        event: Option<&Event>,
    ) -> Result<(), EffectFailure<C::Error>> {
        let resolved = match send_id_expression {
            Some(expression) => match self.text(expression, event)? {
                Some(value) => Some(value),
                None => return Err(EffectFailure::Block(PlatformError::Execution)),
            },
            None => send_id.map(str::to_owned),
        };
        let Some(id) = resolved else {
            return Ok(());
        };
        // SCXML 6.3.1: `sendidexpr` "evaluates to the ID of a delayed event";
        // a value no send id can be (empty, white space, over the id bound)
        // is not a legal value, so it is error.execution (5.9.3), not a
        // session failure.
        let Ok(id) = SendId::new(id) else {
            return Err(EffectFailure::Block(PlatformError::Execution));
        };
        let context = event.and_then(|event| event.trace_context.as_ref());
        self.caps.cancel(id.as_str(), context).map_err(recover::<C>)
    }
}
