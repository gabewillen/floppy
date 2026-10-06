//! Session-level data-model operations behind the WIT `data-model-api`
//! (SCXML 1.0 CR 2014-03-13 Appendix C.3): invoke parameters,
//! string-typed attributes, `BasicHTTP` form values, `<foreach>`
//! node-sets, and the pure payload helpers.
//!
//! These are the single definition of the `XPath` semantics the machine
//! component drives through the WIT interface, and the checked-in IRP
//! baselines (errata cases included) are reproduced through the
//! component with exactly these semantics; no second implementation
//! exists.
//!
//! No spans: the evaluating operations delegate to the instrumented public
//! operations of [`XPathDataModel`] (one span per evaluation, each bounded
//! by [`crate::EVALUATION_FUEL`]), and the pure helpers are cheap
//! conversions whose failures surface as `None`.

use scxml_types::{DataValue, Event, FormValueSource};

use crate::{DataModelError, ExecutionHost, MAX_VALUE_BYTES, XPathDataModel};

/// Result of evaluating a `<foreach array>` expression (SCXML 4.6).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ForeachItems {
    /// One [`DataValue::Xml`] per selected node, in document order.
    Items(Vec<DataValue>),
    /// The value is not a legal collection: not a node-set, or an
    /// evaluation error (`error.execution`).
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

impl XPathDataModel {
    /// Sets one `<invoke>` parameter in this (child) data model (SCXML
    /// 6.4.2): replaces the children of the declared variable `name` and
    /// returns `true`; returns `false`, changing nothing, when `name` is not
    /// declared (official case 245).
    ///
    /// # Errors
    ///
    /// As [`Self::set_declared`].
    pub fn set_parameter(&mut self, name: &str, value: &DataValue) -> Result<bool, DataModelError> {
        self.set_declared(name, value)
    }

    /// Evaluates an attribute SCXML defines as a string (`eventexpr`,
    /// `targetexpr`, `typeexpr`, `delayexpr`, `sendidexpr`, `srcexpr`) with
    /// `XPath` `string()`; always `Some`.
    ///
    /// # Errors
    ///
    /// As [`Self::evaluate_string`].
    pub fn evaluate_text<H: ExecutionHost + ?Sized>(
        &mut self,
        expression: &str,
        event: Option<&Event>,
        host: &mut H,
    ) -> Result<Option<String>, DataModelError> {
        self.evaluate_string(expression, event, host).map(Some)
    }

    /// Evaluates one `BasicHTTP` form value (SCXML C.2): the `XPath`
    /// string-value of `source`. A `location` must first select a
    /// non-empty node-set.
    ///
    /// # Errors
    ///
    /// As [`Self::evaluate_location`] (for a location) and
    /// [`Self::evaluate_string`].
    pub fn evaluate_form_value<H: ExecutionHost + ?Sized>(
        &mut self,
        source: &str,
        kind: FormValueSource,
        event: Option<&Event>,
        host: &mut H,
    ) -> Result<DataValue, DataModelError> {
        if kind == FormValueSource::Location {
            self.evaluate_location(source, event, host)?;
        }
        self.evaluate_string(source, event, host)
            .map(DataValue::String)
    }

    /// Evaluates a `<foreach array>` expression to its node snapshot
    /// (SCXML 4.6, §C.3.10): a node-set, empty or not, iterates its nodes; every
    /// other result, and every recoverable evaluation failure, is
    /// [`ForeachItems::Illegal`].
    ///
    /// # Errors
    ///
    /// The unrecoverable failures of [`Self::evaluate_items`]
    /// ([`DataModelError::InvalidInput`], [`DataModelError::Host`],
    /// [`DataModelError::NotInitialized`], ...); only
    /// [`DataModelError::Execution`] becomes [`ForeachItems::Illegal`].
    pub fn foreach_items<H: ExecutionHost + ?Sized>(
        &mut self,
        expression: &str,
        event: Option<&Event>,
        host: &mut H,
    ) -> Result<ForeachItems, DataModelError> {
        match self.evaluate_items(expression, event, host) {
            Ok(items) => Ok(ForeachItems::Items(items)),
            Err(DataModelError::Execution) => Ok(ForeachItems::Illegal),
            Err(error) => Err(error),
        }
    }
}

/// The payload key of one `namelist` entry: the variable name of a
/// `$name` reference; `None` otherwise (SCXML 6.2.4 `error.execution`).
#[must_use]
pub fn namelist_key(location: &str) -> Option<&str> {
    crate::namelist_name(location)
}

/// Builds `_event.data` from `<param>`/`namelist` entries (§C.3.7.1): one
/// `<data id="name">` element per entry, in order, duplicates kept. `None`
/// when a value cannot be carried (JSON arrays, malformed markup, over
/// [`MAX_VALUE_BYTES`]) — `error.execution`.
#[must_use]
pub fn event_payload(entries: &[(String, DataValue)]) -> Option<DataValue> {
    crate::event_data(entries).ok()
}

/// Value of inline `<content>` or `<data>` children (SCXML 5.6, 6.2.6):
/// markup is an XML value (a node-set); text is kept verbatim as a string.
/// `None` when the input exceeds [`MAX_VALUE_BYTES`].
#[must_use]
pub fn inline_content(content: InlineContent<'_>) -> Option<DataValue> {
    match content {
        InlineContent::Markup(markup) => {
            (markup.len() <= MAX_VALUE_BYTES).then(|| DataValue::Xml(markup.to_owned()))
        }
        InlineContent::Text(text) => {
            (text.len() <= MAX_VALUE_BYTES).then(|| DataValue::String(text.to_owned()))
        }
    }
}

/// Value of an external `<data src>` document (SCXML 5.3): an XML value
/// when the text is a well-formed fragment containing an element, else the
/// text. Always `Some`: the `XPath` data model decides every document
/// itself.
#[must_use]
pub fn external_data(text: &str) -> Option<DataValue> {
    Some(crate::markup_value(text))
}
