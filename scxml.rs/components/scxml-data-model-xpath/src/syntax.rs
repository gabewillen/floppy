//! Parse-only `XPath` syntax checks for validators (WIT `check-syntax`).

use scxml_telemetry::{Module, Operation, observe};
use scxml_xpath::{Expression, ParseOptions};

use crate::{DataModelError, finish};

/// Maximum bytes of one issue message (WIT contract bound).
pub const MAX_SYNTAX_MESSAGE_BYTES: usize = 512;

/// Syntactic category of one checked fragment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyntaxKind {
    /// A boolean `cond` expression.
    Condition,
    /// A value expression.
    Value,
    /// An assignable location (or a `<foreach>` item/index variable name).
    Location,
    /// A `<script>` body.
    Script,
}

/// One syntax problem located in the caller's source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxIssue {
    /// Inclusive UTF-8 byte offset.
    pub start: usize,
    /// Exclusive UTF-8 byte offset.
    pub end: usize,
    /// Static message without positions or source text.
    pub message: String,
}

/// Parses `source` as `kind` without evaluating it; never reads or changes
/// data-model state.
///
/// Conditions and values must be `XPath` 1.0 expressions (with the SCXML
/// `In()` extension); locations must additionally be expressions that can
/// yield a node-set; scripts must be empty (only XML white space) because
/// the `XPath` data model defines no script language. Returns an empty list
/// for valid fragments, otherwise exactly one issue with an exact byte span.
///
/// # Errors
///
/// [`DataModelError::InvalidInput`] when its parse exceeds an `XPath` parser bound (nesting or size); the data
/// model applies the same bound at evaluation, so a validator reports the
/// decline as `LIMIT003` rather than as a syntax error.
pub fn check_syntax(kind: SyntaxKind, source: &str) -> Result<Vec<SyntaxIssue>, DataModelError> {
    let span = observe(
        Module::DataModelXpathComponent,
        Operation::XpathDataModelCheckSyntax,
    );
    let result = (|| {
        let whole = |message: &str| SyntaxIssue {
            start: 0,
            end: source.len(),
            message: message.to_owned(),
        };
        if kind == SyntaxKind::Script {
            return Ok(if source.chars().all(scxml_types::is_xml_space) {
                Vec::new()
            } else {
                vec![whole(
                    "the XPath data model defines no script language; <script> must be empty",
                )]
            });
        }
        Ok(match Expression::parse(source, &ParseOptions::scxml()) {
            Err(scxml_xpath::Error::Syntax(error)) => vec![SyntaxIssue {
                start: error.start,
                end: error.end,
                message: error.message.to_owned(),
            }],
            Err(_) => return Err(DataModelError::InvalidInput),
            Ok(expression) if kind == SyntaxKind::Location && !expression.may_yield_node_set() => {
                vec![whole(
                    "a location must be an XPath expression that selects nodes",
                )]
            }
            Ok(_) => Vec::new(),
        })
    })();
    finish(&span, &result);
    result
}
