//! Bounded, deterministic SCXML `XPath` data model (SCXML 1.0 CR 2014-03-13 Appendix C.3;
//! the final SCXML 1.0 Recommendation omits the `XPath` data model).
//!
//! # Model
//!
//! All state lives in one [`scxml_xpath::Store`]: a document node owns a
//! `<datamodel>` element, which is the `XPath` context node of every
//! expression. Its children are, in order:
//!
//! * the read-only system variables `<_sessionid>`, `<_name>`, and
//!   `<_ioprocessors>` (one `<processor name="…">` child per advertised I/O
//!   processor, each with its fields as child elements, e.g. `<location>`);
//! * one `<data id="x">…</data>` element per variable (its children are
//!   the value; CR 2014-03-13 §C.3.1). While a `<foreach>` has assigned an
//!   item to a variable, `$x` denotes that collection node itself, wherever
//!   it is, and the `<data>` element stays in place (SCXML 1.0 §4.6: the
//!   processor "MUST act as if it has made a shallow copy of the
//!   collection", and a shallow copy of a node-set holds its nodes;
//!   `docs/spec-errata.md` §8);
//! * `<_event>` while — and only while — an operation runs with a current
//!   event. Its children are always `name`, `type`, `sendid`, `origin`,
//!   `origintype`, `invokeid`, `data`, and `raw`, empty when a field is
//!   absent. `data` holds the payload as §C.3.7.1 of the CR interprets it
//!   (key-value pairs as `<data id>` children, else XML, else a
//!   space-normalized string literal), a translation that exists for every
//!   valid event. With no current event `$_event` is unbound (SCXML 5.10).
//!
//! `$name` resolves to a node-set holding exactly the variable's element, so
//! `$Var1/text()` reads a declared value and relative paths such as
//! `_event/data` resolve against `<datamodel>`.
//!
//! # Contract
//!
//! * Values crossing the API are [`DataValue`]s: node-sets travel as
//!   [`DataValue::Xml`] markup (attribute and namespace nodes as their
//!   escaped string-value), strings as [`DataValue::String`], booleans as
//!   [`DataValue::Bool`], finite numbers as [`DataValue::Number`], and
//!   `NaN`/`±Infinity` as their `XPath` string form.
//! * Assignments only mutate nodes inside a variable's tree. The system
//!   variables, `_event`, `<datamodel>`, and the document are read-only;
//!   `replace` and `delete` may not remove a variable's own element. A
//!   location must select a non-empty node-set; the value is applied to each
//!   selected node in document order, all-or-nothing.
//! * Every failure is a typed [`DataModelError`]; failed operations leave
//!   committed state unchanged. Evaluation work is bounded by
//!   [`EVALUATION_FUEL`]; every input, value, and the variable
//!   count are bounded by the constants below, and the evaluator bounds
//!   expression, tree, and node-set sizes (`scxml_xpath::limits`).
//! * Deterministic: no clocks, randomness, or ambient state; `In()` is the
//!   only host query and goes through the explicit [`ExecutionHost`].
//! * Telemetry: public operations open one span through `scxml-telemetry`;
//!   evaluator internals and pure helpers carry none (the evaluator runs on
//!   guard hot paths). Span attributes are the closed module and operation
//!   enums only; expressions, names, and values never enter telemetry.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; scripts/verify-component-authored-unsafe.py rejects authored unsafe"
    )
)]

use std::fmt;

use scxml_types::DataValue;

mod model;
mod operations;
mod payload;
mod syntax;

#[cfg(target_arch = "wasm32")]
mod guest;

pub use model::XPathDataModel;
pub use operations::{
    ForeachItems, InlineContent, event_payload, external_data, inline_content, namelist_key,
};
pub use payload::{
    accepts_event_data, declaration_source, event_data, event_data_field, markup_value,
    namelist_name,
};
pub use syntax::{MAX_SYNTAX_MESSAGE_BYTES, SyntaxIssue, SyntaxKind, check_syntax};

/// W3C name of this data model.
pub const DATA_MODEL_NAME: &str = "xpath";
/// Processor name the W3C `XPath` IRP template reads for the SCXML Event I/O
/// Processor location (official cases 500 and 501). [`XPathDataModel::
/// initialize_declarations`] binds it in `_ioprocessors` to the
/// [`SCXML_EVENT_PROCESSOR_NAME`] entry.
pub const XPATH_TEMPLATE_PROCESSOR_NAME: &str = "http://www.w3.org/TR/scxml/#SCXMLProcessor";
/// Canonical SCXML Event I/O Processor type URI, the `_ioprocessors` entry
/// [`XPATH_TEMPLATE_PROCESSOR_NAME`] mirrors (equal to
/// `scxml_ioprocessor_scxml::SCXML_EVENT_PROCESSOR_URI`).
pub const SCXML_EVENT_PROCESSOR_NAME: &str = "http://www.w3.org/TR/scxml/#SCXMLEventProcessor";
/// Maximum initialization source bytes (every build: the `data-model-api`
/// `initialize` bound of the WIT contract, "the declarations' placeholder
/// encoding at most 65536 bytes").
pub const MAX_SOURCE_BYTES: usize = 65_536;
/// Maximum expression or location bytes (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_EXPRESSION_BYTES: usize = scxml_types::wasm_bound(16_384);
/// Maximum `<script>` bytes (the `XPath` data model has no script language;
/// the bound still applies before a non-empty script is rejected)
/// (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_SCRIPT_BYTES: usize = scxml_types::wasm_bound(65_536);
/// Longest variable markup a data-model snapshot carries; longer markup is
/// [`VariableMarkup::Oversized`] (the WIT `variable-value` contract). No
/// value has a byte bound (SCXML sets none).
pub const MAX_VALUE_BYTES: usize = 65_536;
/// Maximum variables retained by one instance (every build: the WIT
/// contract's `The XPath provider bounds declared variables to 256`, quoted
/// exactly).
pub const MAX_VARIABLES: usize = 256;
/// Evaluator work units granted to one expression evaluation in a wasm build.
pub const WASM_EVALUATION_FUEL: u64 = scxml_xpath::limits::DEFAULT_FUEL;
/// Evaluator work units granted to one expression evaluation:
/// [`WASM_EVALUATION_FUEL`] in a wasm build, unbounded natively (see
/// [`scxml_types::wasm_bound`]).
pub const EVALUATION_FUEL: u64 = if cfg!(target_arch = "wasm32") {
    WASM_EVALUATION_FUEL
} else {
    u64::MAX
};
/// Maximum parsed expressions retained by the per-instance parse cache.
pub const MAX_CACHED_EXPRESSIONS: usize = 256;

/// Immutable SCXML system variables (SCXML 1.0 section 5.10).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemContext {
    /// `_sessionid` value.
    pub session_id: String,
    /// `_name` value: the document's `name` attribute, `None` when it has
    /// none (SCXML 5.10); `<_name>` is then empty.
    pub machine_name: Option<String>,
    /// `_ioprocessors`: a canonical JSON object mapping each processor name
    /// to an object of string fields (for example `{"location": "…"}`).
    pub io_processors: DataValue,
}

/// One variable of a read-only [`DataModelSnapshot`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Variable {
    /// Variable name.
    pub name: String,
    /// The variable's element markup, or its length when too long to carry.
    pub markup: VariableMarkup,
}

/// The serialized element of one snapshot [`Variable`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VariableMarkup {
    /// The markup of the node `$x` denotes (`<data id="x">…</data>`, or
    /// the collection node a `<foreach>` assigned to it), at most
    /// [`MAX_VALUE_BYTES`] bytes.
    Markup(String),
    /// Markup longer than [`MAX_VALUE_BYTES`]: its exact length in bytes.
    /// The markup is not carried.
    Oversized(u64),
}

/// Read-only image of the committed data model (`docs/session-snapshot.md`,
/// "Session object (keys in this order)").
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataModelSnapshot {
    /// SCXML 5.10 system variables.
    pub system: SystemContext,
    /// Every variable in document order of `<datamodel>`.
    pub variables: Vec<Variable>,
}

/// Explicit execution-host capability used by `In()`.
pub trait ExecutionHost {
    /// Queries the current machine configuration.
    ///
    /// # Errors
    ///
    /// Returns [`DataModelError::Host`] when the host query fails.
    fn in_state(&mut self, state_id: &str) -> Result<bool, DataModelError>;
}

/// Bounded data-model failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataModelError {
    /// Input was malformed, over bound, or not an `XPath` expression.
    InvalidInput,
    /// Evaluation or assignment failed (SCXML `error.execution`).
    Execution,
    /// A value has no representation in the `XPath` data model.
    UnsupportedValue,
    /// Initialization has not completed.
    NotInitialized,
    /// The execution host failed an `In()` query.
    Host,
}

impl fmt::Display for DataModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidInput => "XPath input is invalid or exceeds its bound",
            Self::Execution => "bounded XPath evaluation or assignment failed",
            Self::UnsupportedValue => "value has no XPath data-model representation",
            Self::NotInitialized => "XPath data model is not initialized",
            Self::Host => "execution-host state query failed",
        })
    }
}

impl std::error::Error for DataModelError {}

/// Records one public operation's outcome on its span.
fn finish<T>(span: &scxml_telemetry::FunctionSpan, result: &Result<T, DataModelError>) {
    use scxml_telemetry::{ErrorCategory, SignalEvent};
    match result {
        Ok(_) => span.event(SignalEvent::TransformationCompleted),
        Err(error) => {
            span.event(SignalEvent::InputRejected);
            span.fail(match error {
                DataModelError::InvalidInput => ErrorCategory::InvalidInput,
                DataModelError::Execution => ErrorCategory::Execution,
                DataModelError::UnsupportedValue => ErrorCategory::Unsupported,
                DataModelError::NotInitialized => ErrorCategory::Harness,
                DataModelError::Host => ErrorCategory::Platform,
            });
        }
    }
}

#[cfg(test)]
mod tests;
