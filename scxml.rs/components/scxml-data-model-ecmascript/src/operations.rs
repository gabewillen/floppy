//! Session-level data-model operations behind the WIT `data-model-api`
//! (SCXML 1.0 Appendix B.2): late binding, invoke parameters, locations,
//! string-typed attributes, `BasicHTTP` form values, `<foreach>`
//! collections, and the pure payload helpers.
//!
//! These are the single definition of the ECMAScript semantics the machine
//! component drives through the WIT interface, and the checked-in IRP
//! baselines are reproduced through the component with exactly these
//! semantics.
//!
//! No spans: the evaluating operations delegate to the instrumented public
//! operations of [`EcmaScriptDataModel`] (one span per evaluation), and the
//! pure helpers are cheap conversions whose failures surface as `None`.

use scxml_telemetry::{Module, Operation, observe};
use scxml_types::{DataInitializer, DataValue, Event, FiniteNumber, FormValueSource};

use crate::session::Copied;
use crate::{DataModelError, EcmaScriptDataModel, ExecutionHost};

/// Result of evaluating a `<foreach array>` expression (SCXML 4.6).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ForeachItems {
    /// Shallow snapshot of the array's items, in iteration order.
    Items(Vec<DataValue>),
    /// The value is not a legal collection (`error.execution`): anything
    /// but an `instanceof(Array)` value, an XML (DOM) value included.
    Illegal,
}

/// Inline `<content>` or `<data>` children (SCXML 5.6, 6.2.6).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InlineContent<'a> {
    /// Text-only children, concatenated in document order.
    Text(&'a str),
    /// Children containing at least one element, as standalone markup.
    Markup(&'a str),
}

impl EcmaScriptDataModel {
    /// Declares one late-bound `<data>` variable (SCXML 5.3) as
    /// `undefined`, replacing any value it holds, before its initializer
    /// runs on first entry of its state.
    ///
    /// # Errors
    ///
    /// As [`Self::declare_assign`]: invalid or reserved names and a full
    /// variable table are [`DataModelError::InvalidInput`];
    /// [`DataModelError::NotInitialized`] before initialization.
    pub fn declare(&mut self, id: &str) -> Result<(), DataModelError> {
        let span = scxml_telemetry::observe(
            scxml_telemetry::Module::DataModelEcmascriptComponent,
            scxml_telemetry::Operation::DataModelInitialize,
        );
        let result = self.declare_undefined(id);
        crate::complete(&span, &result);
        result
    }

    /// [`Self::declare`] without its span.
    fn declare_undefined(&mut self, id: &str) -> Result<(), DataModelError> {
        self.system_context()?;
        crate::validate_data_id(id)?;
        if crate::syntax::is_variable_name(id) {
            self.declare_value(id, &DataValue::Undefined, &mut crate::NoHost)?;
        } else {
            // B.2.1: an id that is not an ECMAScript identifier is still a
            // variable, a property of the global object.
            self.initialize_global_property(
                id,
                &DataInitializer::Value(DataValue::Undefined),
                &mut crate::NoHost,
            )?;
        }
        if !self.declared.iter().any(|declared| declared == id) {
            self.declared.push(id.to_owned());
        }
        Ok(())
    }

    /// Sets one `<invoke>` parameter in this (child) data model (SCXML
    /// 6.4.2). Returns `false`, changing nothing, when the location's root
    /// is not declared or the location is malformed: SCXML leaves such a
    /// parameter unset (official case 245).
    ///
    /// # Errors
    ///
    /// As [`Self::assign`], except that [`DataModelError::InvalidInput`]
    /// becomes `Ok(false)`.
    pub fn set_parameter(&mut self, name: &str, value: &DataValue) -> Result<bool, DataModelError> {
        self.system_context()?;
        // A parameter names a variable of the child (6.4.2); a name that
        // does not start from a declared variable sets nothing.
        if !self.starts_at_declared_variable(name) {
            return Ok(false);
        }
        match self.assign(name, value) {
            Ok(()) => Ok(true),
            Err(DataModelError::InvalidInput) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Evaluates a value expression with `_event` bound to `event`
    /// (unbound when `None`).
    fn evaluate_optional(
        &self,
        expression: &str,
        event: Option<&Event>,
        host: &mut impl ExecutionHost,
    ) -> Result<DataValue, DataModelError> {
        match event {
            Some(event) => self.evaluate_value(expression, event, host),
            None => self.evaluate_value_without_event(expression, host),
        }
    }

    /// Reads one location (`<param location>`, a `namelist` entry): the
    /// ECMAScript value of the location expression. A missing variable is a
    /// `ReferenceError`, which is [`DataModelError::Execution`].
    ///
    /// # Errors
    ///
    /// As [`Self::evaluate_value`].
    pub fn evaluate_location(
        &self,
        location: &str,
        event: Option<&Event>,
        host: &mut impl ExecutionHost,
    ) -> Result<DataValue, DataModelError> {
        self.evaluate_optional(location, event, host)
    }

    /// Evaluates an attribute SCXML defines as a string (`eventexpr`,
    /// `targetexpr`, `typeexpr`, `delayexpr`, `sendidexpr`, `srcexpr`):
    /// `Some` for an ECMAScript string, `None` for any other value (the
    /// caller raises `error.execution`).
    ///
    /// # Errors
    ///
    /// As [`Self::evaluate_value`].
    pub fn evaluate_text(
        &self,
        expression: &str,
        event: Option<&Event>,
        host: &mut impl ExecutionHost,
    ) -> Result<Option<String>, DataModelError> {
        Ok(match self.evaluate_optional(expression, event, host)? {
            DataValue::String(value) => Some(value),
            _ => None,
        })
    }

    /// Evaluates one `BasicHTTP` form value (SCXML C.2): the portable value
    /// of the location or value expression, which the processor encodes as
    /// a form field.
    ///
    /// # Errors
    ///
    /// As [`Self::evaluate_location`].
    pub fn evaluate_form_value(
        &self,
        source: &str,
        kind: FormValueSource,
        event: Option<&Event>,
        host: &mut impl ExecutionHost,
    ) -> Result<DataValue, DataModelError> {
        match kind {
            FormValueSource::Location => self.evaluate_location(source, event, host),
            FormValueSource::Expression => self.evaluate_optional(source, event, host),
        }
    }

    /// Evaluates a `<foreach array>` expression and starts the loop over a
    /// shallow copy of the array (SCXML 4.6: "The SCXML processor MUST act
    /// as if it has made a shallow copy of the collection produced by the
    /// evaluation of 'array'"; B.2.11: "the SCXML Processor MUST support
    /// iteration over objects that satisfy instanceof(Array)").
    ///
    /// The copy reads `array[index]` for every index below its `length`
    /// (a hole or an undefined element is `undefined`, which 4.6 allows).
    /// The returned items are each element's portable value, or its log
    /// text when it has none; the loop's item variable is then bound to
    /// the element itself ([`Self::declare_assign`], B.2.11: "equivalent to
    /// `item = array_name[index]`"). Any other value, an XML (DOM) value included, is
    /// [`ForeachItems::Illegal`] (4.6: not "a legal iterable collection").
    ///
    /// # Errors
    ///
    /// As [`Self::evaluate_value`], except that no element needs a portable
    /// value.
    pub fn foreach_items(
        &self,
        expression: &str,
        event: Option<&Event>,
        host: &mut impl ExecutionHost,
    ) -> Result<ForeachItems, DataModelError> {
        let span = observe(
            Module::DataModelEcmascriptComponent,
            Operation::DataModelEvaluateValue,
        );
        let result = (|| {
            self.system_context()?;
            crate::validate_program(expression)?;
            if let Some(event) = event {
                self.validate_event(event)?;
            }
            let source = crate::checked_source(crate::value_expression_source(expression))?;
            self.engine.run(host, event, move |session| {
                let value = session
                    .eval(&source)
                    .map_err(|error| crate::classify_js_error(&error))?;
                Ok(match session.start_foreach(&value)? {
                    Copied::Items(items) => ForeachItems::Items(items),
                    Copied::Illegal => ForeachItems::Illegal,
                })
            })
        })();
        crate::complete(&span, &result);
        result
    }
}

/// The payload key of one `namelist` entry (SCXML 6.2.4): the entry itself
/// when it is a plain ECMAScript variable name (ASCII letters, `_`, `$`,
/// and non-leading digits); `None` otherwise (`error.execution`, official
/// case 553).
#[must_use]
pub fn namelist_key(location: &str) -> Option<&str> {
    let legal = !location.is_empty()
        && location.chars().enumerate().all(|(index, character)| {
            character.is_ascii_alphabetic()
                || character == '_'
                || character == '$'
                || (index > 0 && character.is_ascii_digit())
        });
    legal.then_some(location)
}

/// Builds `_event.data` from `<param>`/`namelist` entries (SCXML
/// 6.2.4-6.2.5): one object member per entry (a later duplicate name
/// replaces an earlier one), as canonical JSON. `None` when a value has no
/// JSON representation (`undefined`, absent, XML) (`error.execution`).
#[must_use]
pub fn event_payload(entries: &[(String, DataValue)]) -> Option<DataValue> {
    let mut data = serde_json::Map::new();
    for (name, value) in entries {
        data.insert(name.clone(), value_to_json(value)?);
    }
    serde_json::to_string(&serde_json::Value::Object(data))
        .ok()
        .map(DataValue::JsonObject)
}

/// Value of inline `<content>` or `<data>` children (SCXML 5.6, 6.2.6):
/// markup is an XML value (a DOM, B.2.1; official 561); text is
/// whitespace-normalized and read as JSON when it parses, else kept as a
/// string.
#[must_use]
pub fn inline_content(content: InlineContent<'_>) -> Option<DataValue> {
    match content {
        InlineContent::Markup(markup) => Some(DataValue::Xml(markup.to_owned())),
        InlineContent::Text(text) => {
            let normalized = normalize_whitespace(text);
            Some(
                match serde_json::from_str::<serde_json::Value>(&normalized) {
                    Ok(json) => json_to_value(&json).unwrap_or(DataValue::String(normalized)),
                    Err(_) => DataValue::String(normalized),
                },
            )
        }
    }
}

/// The value `_event.data` carries under `name` (SCXML 6.5.2 "any return
/// value that has a name that matches"): the member of the object a sender
/// built from `<param>`/`namelist` entries. `None` when the data is not an
/// object or has no such member.
#[must_use]
pub fn event_data_field(data: &DataValue, name: &str) -> Option<DataValue> {
    data.json_object_member(name)
}

/// Value of an external `<data src>` document this data model decides
/// itself: an XML value when the text is an XML document (B.2.1). `None`
/// leaves the caller's JSON-or-text rule.
#[must_use]
pub fn external_data(text: &str) -> Option<DataValue> {
    crate::xml_document_value(text)
}

/// SCXML's "space-normalized string literal" (5.3, 6.2.6; B.2.1 and
/// B.2.8.1), as `XPath` 1.0 `normalize-space()` defines it and the `XPath` data
/// model applies it: leading and trailing white space stripped and every
/// internal run of it replaced by one space, where white space is "the same
/// as those allowed by the S production in XML" (space, tab, carriage
/// return, line feed); every other character is content.
fn normalize_whitespace(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut pending_space = false;
    for character in text.chars() {
        if matches!(character, ' ' | '\t' | '\r' | '\n') {
            pending_space = !normalized.is_empty();
        } else {
            if pending_space {
                normalized.push(' ');
                pending_space = false;
            }
            normalized.push(character);
        }
    }
    normalized
}

/// JSON of one portable payload value; `None` for values without a JSON
/// representation.
fn value_to_json(value: &DataValue) -> Option<serde_json::Value> {
    match value {
        DataValue::Absent | DataValue::Undefined | DataValue::Xml(_) => None,
        DataValue::Null => Some(serde_json::Value::Null),
        DataValue::Bool(value) => Some(serde_json::Value::Bool(*value)),
        DataValue::Number(number) => {
            serde_json::Number::from_f64(number.get()).map(serde_json::Value::Number)
        }
        DataValue::String(value) => Some(serde_json::Value::String(value.clone())),
        DataValue::JsonArray(value) | DataValue::JsonObject(value) => {
            serde_json::from_str(value).ok()
        }
    }
}

/// Portable value of parsed inline JSON (arrays and objects re-emitted
/// canonically); `None` when a number is not finite or a collection has no
/// canonical form.
fn json_to_value(value: &serde_json::Value) -> Option<DataValue> {
    Some(match value {
        serde_json::Value::Null => DataValue::Null,
        serde_json::Value::Bool(value) => DataValue::Bool(*value),
        serde_json::Value::Number(number) => {
            DataValue::Number(FiniteNumber::new(number.as_f64()?).ok()?)
        }
        serde_json::Value::String(value) => DataValue::String(value.clone()),
        serde_json::Value::Array(_) => {
            DataValue::canonical_json_array(&serde_json::to_string(value).ok()?).ok()?
        }
        serde_json::Value::Object(_) => {
            DataValue::canonical_json_object(&serde_json::to_string(value).ok()?).ok()?
        }
    })
}
