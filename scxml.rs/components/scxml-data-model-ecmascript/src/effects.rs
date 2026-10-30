//! Parse-only side-effect analysis of ECMAScript conditions and `<log expr>`.
//!
//! SCXML 1.0 governs three places:
//!
//! * §4.7.3: "The SCXML processor MUST ensure that `<log>` has no
//!   side-effects on document interpretation."
//! * Appendix B, on every data model's condition language: "This language
//!   MUST not have side effects".
//! * §5.9.1: "Conditional expressions in conformant SCXML documents SHOULD
//!   NOT have side effects."
//!
//! Those texts cover only `cond` (on `<transition>`, `<if>`, `<elseif>`)
//! and `<log expr>`, so only those fragments are checked; every other
//! fragment of the document is read for its declarations and writes.
//!
//! Separately, in every fragment, an assignment, compound assignment,
//! update, or `delete` of a system variable itself (`_event`, `_sessionid`,
//! `_name`, `_ioprocessors`, and the platform root `_x`) outside a function
//! that declares the name is reported as [`EffectKind::SystemVariableWrite`]
//! (§5.10: "The Processor MUST cause any attempt to change the value of a
//! system variable to fail"; the data model cannot make a non-strict
//! `delete` of one fail at run time).
//!
//! # Contract
//!
//! * Pure: every fragment is parsed with Boa's parser, exactly as
//!   [`crate::syntax::check_syntax`] parses it, and never evaluated; no
//!   data-model state, host capability, clock, or I/O is touched, and nothing
//!   is retained after the call.
//! * Conservative: a [`Certainty::Certain`] finding names syntax that
//!   certainly changes state outside the checked expression when it runs
//!   (an assignment, update, or `delete` of a variable or property outside
//!   it, a built-in method known to change its receiver or argument,
//!   `Math.random` (whose session sequence advances), or a call of a
//!   document function whose body, transitively, does one of those). What
//!   the analysis cannot decide (a call it cannot see, dynamic code, a
//!   `with` scope, an unread `<script src>`, a document getter or conversion
//!   method that has an effect, a write through a reference it cannot
//!   trace) is a [`Certainty::Undecidable`] finding. Writes to variables
//!   and fresh objects local to the checked expression are not effects.
//! * Interprocedural: document functions (declared by static `<script>`
//!   elements or bound by a `<data expr>` function) are analyzed at each call
//!   with the origins of their arguments and receiver, so a function that
//!   touches only its locals and parameters' fresh objects is pure. A
//!   function is only resolved when the document declares its name exactly
//!   once and never assigns it.
//! * Bounds: at most [`MAX_EFFECT_FRAGMENTS`] fragments and
//!   [`MAX_EFFECT_SOURCE_BYTES`] source bytes; each fragment within the data
//!   model's expression or script bound and parse nesting bound (a fragment
//!   outside them is skipped, and a skipped script counts as unread). At most
//!   [`MAX_FINDINGS_PER_FRAGMENT`] findings per checked fragment; the walk of
//!   one checked fragment is bounded by [`MAX_ANALYSIS_STEPS`] steps and
//!   [`MAX_CALL_DEPTH`] nested calls (exceeding either is one undecidable
//!   finding). Over-bound requests fail with [`DataModelError::InvalidInput`].
//! * Deterministic: findings are sorted and depend only on the input.

use std::collections::{BTreeMap, BTreeSet};

use boa_engine::ast::declaration::{Binding, LexicalDeclaration, Variable};
use boa_engine::ast::expression::access::{PropertyAccess, PropertyAccessField};
use boa_engine::ast::expression::literal::{
    LiteralKind, ObjectMethodDefinition, PropertyDefinition,
};
use boa_engine::ast::expression::operator::assign::{AssignOp, AssignTarget};
use boa_engine::ast::expression::operator::binary::BinaryOp;
use boa_engine::ast::expression::operator::unary::UnaryOp;
use boa_engine::ast::expression::operator::update::UpdateTarget;
use boa_engine::ast::expression::{Expression, Identifier};
use boa_engine::ast::function::{ClassElement, FormalParameterList, FunctionBody};
use boa_engine::ast::pattern::{ArrayPatternElement, ObjectPatternElement, Pattern};
use boa_engine::ast::property::MethodDefinitionKind;
use boa_engine::ast::property::PropertyName;
use boa_engine::ast::scope::Scope;
use boa_engine::ast::statement::Statement;
use boa_engine::ast::statement::iteration::{ForLoopInitializer, IterableLoopInitializer};
use boa_engine::ast::visitor::{VisitWith, Visitor};
use boa_engine::ast::{Declaration, Position, Script, Span, Spanned, StatementListItem};
use boa_engine::interner::{Interner, Sym};
use boa_engine::parser::{Parser, Source};
use scxml_telemetry::{Module, Operation, SignalEvent, observe};
use scxml_types::ExternalScripts;

use crate::{DataModelError, check_nesting, nesting, on_parse_stack};

/// Most fragments one request may carry (the validator's bound)
/// (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_EFFECT_FRAGMENTS: usize = scxml_types::wasm_bound(16_384);
/// Most source bytes one request may carry (the validator's bound)
/// (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_EFFECT_SOURCE_BYTES: usize = scxml_types::wasm_bound(4 * 1024 * 1024);
/// Most findings reported for one checked fragment.
pub const MAX_FINDINGS_PER_FRAGMENT: usize = 4;
/// Most bytes of one finding's subject or function name.
pub const MAX_EFFECT_SUBJECT_BYTES: usize = 256;
/// Most walk steps spent on one checked fragment, callees included (every
/// build): local functions are walked at each call, so the walk can grow
/// exponentially in the fragment's size, and exhausting the budget only
/// turns the fragment's result into one undecidable finding.
pub const MAX_ANALYSIS_STEPS: usize = 200_000;
/// Most nested document or local function calls followed from one checked
/// fragment.
pub const MAX_CALL_DEPTH: usize = 16;

/// Whether a call expression calls its callee or constructs with it (`new`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CallKind {
    /// A call: `f()`, a tagged template.
    Call,
    /// A construction: `new F()`.
    Construct,
}

impl CallKind {
    /// The origin of `this` inside the callee.
    const fn this(self) -> Origin {
        match self {
            Self::Call => Origin::NonLocal,
            Self::Construct => Origin::FreshShallow,
        }
    }
}

/// Where a walked function comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CalleeOrigin {
    /// A document function: its effects are reported at the call with
    /// their cause, and its walk is memoized.
    Document,
    /// A function in the walked text: it reports its own effects.
    Walked,
}

/// Whether a function has its own `arguments` and function scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FunctionForm {
    /// An arrow function: no own `arguments`.
    Arrow,
    /// Any other function.
    Ordinary,
}

/// How a function binding is declared.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BindingForm {
    /// A function declaration, which takes no other assignment.
    Declaration,
    /// A variable initialized with a function (`var g = function`).
    Initializer,
}

/// What a fragment is in its document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectRole<'a> {
    /// A `cond` on `<transition>`, `<if>`, or `<elseif>` (checked).
    Condition,
    /// A `<log expr>` (checked).
    Log,
    /// A static `<script>` body: its declarations and writes.
    Script,
    /// A `<data id expr>`: binds `id` to the value.
    Data(&'a str),
    /// Any other value expression: its writes.
    Value,
    /// A location (`<assign location>`, `<foreach item>`, ...): its root
    /// variable is written.
    Location,
}

impl EffectRole<'_> {
    const fn checked(self) -> bool {
        matches!(self, Self::Condition | Self::Log)
    }
}

/// One fragment of a document's ECMAScript source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EffectFragment<'a> {
    /// The fragment's role.
    pub role: EffectRole<'a>,
    /// Its decoded source text.
    pub source: &'a str,
}

/// What a finding (or its cause) does.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EffectKind {
    /// Assigns a variable or property outside the checked expression.
    Assignment,
    /// Increments or decrements one (`++`, `--`).
    Update,
    /// Deletes one (`delete`).
    Deletion,
    /// Calls a built-in method that changes its receiver or an argument.
    MutatingCall,
    /// Draws from `Math.random`, whose session sequence then advances.
    Random,
    /// Calls a document function; the cause says what its body does.
    FunctionCall,
    /// Calls a function the analysis cannot see.
    UnknownCall,
    /// Runs text as code (`eval`, `Function`).
    DynamicCode,
    /// Reads a value while the document defines a getter or conversion
    /// method that has an effect.
    ImplicitCall,
    /// Writes through a reference whose object the analysis cannot trace.
    UnknownWrite,
    /// Uses a `with` scope.
    DynamicScope,
    /// Calls something an unread `<script src>` may define or replace.
    UnseenScript,
    /// Exceeds the analysis bounds.
    TooComplex,
    /// Assigns, updates, or deletes a system variable itself (reported for
    /// any fragment, not only checked ones).
    SystemVariableWrite,
}

/// How sure a finding is.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Certainty {
    /// The syntax certainly has the effect when it runs.
    Certain,
    /// The analysis cannot decide.
    Undecidable,
}

/// A byte range of one fragment.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EffectSpan {
    /// Index of the fragment in the request.
    pub fragment: usize,
    /// Inclusive UTF-8 byte offset into that fragment's source.
    pub start: usize,
    /// Exclusive UTF-8 byte offset (`start <= end`).
    pub end: usize,
}

/// Where a function-call finding's effect happens.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EffectCause {
    /// What happens there.
    pub kind: EffectKind,
    /// Where (in a script or data fragment, or a checked fragment).
    pub span: EffectSpan,
    /// What it touches or calls (a variable, method, or function name).
    pub subject: String,
    /// The function whose body contains it.
    pub function: String,
}

/// One effect of a checked fragment.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EffectFinding {
    /// Certain or undecidable.
    pub certainty: Certainty,
    /// What the checked fragment does.
    pub kind: EffectKind,
    /// Where in the checked fragment.
    pub span: EffectSpan,
    /// What it touches or calls.
    pub subject: String,
    /// For a call, where the callee's effect happens.
    pub cause: Option<EffectCause>,
    /// The `=` of a checked condition that is exactly one plain assignment
    /// (`x = 2`), which a comparison was probably meant as.
    pub operator: Option<EffectSpan>,
}

/// Analyzes the checked fragments (`cond` and `<log expr>`) of one
/// document's ECMAScript source; see the module contract.
///
/// `external_scripts` says whether the document has a `<script src>` the
/// analysis cannot read.
///
/// # Errors
///
/// [`DataModelError::InvalidInput`] when the request exceeds
/// [`MAX_EFFECT_FRAGMENTS`] or [`MAX_EFFECT_SOURCE_BYTES`], or the analysis
/// cannot run.
pub fn check_effects(
    fragments: &[EffectFragment<'_>],
    external_scripts: ExternalScripts,
) -> Result<Vec<EffectFinding>, DataModelError> {
    let span = observe(
        Module::DataModelEcmascriptComponent,
        Operation::DataModelCheckEffects,
    );
    let bytes = fragments.iter().try_fold(0_usize, |total, fragment| {
        total.checked_add(fragment.source.len())
    });
    if scxml_types::exceeds_wasm_bound(fragments.len(), MAX_EFFECT_FRAGMENTS)
        || bytes.is_none_or(|bytes| scxml_types::exceeds_wasm_bound(bytes, MAX_EFFECT_SOURCE_BYTES))
    {
        span.event(SignalEvent::InputRejected);
        span.fail(scxml_telemetry::ErrorCategory::Validation);
        return Err(DataModelError::InvalidInput);
    }
    let result = on_parse_stack(&span, || Ok(analyze(fragments, external_scripts)));
    match &result {
        Ok(_) => span.event(SignalEvent::InputAccepted),
        Err(_) => span.fail(scxml_telemetry::ErrorCategory::Validation),
    }
    result
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// One parsed fragment and how its text maps back to the caller's source.
struct Parsed {
    role_checked: bool,
    data_id: Option<String>,
    location: bool,
    script: bool,
    text: String,
    prefix: usize,
    inner_start: usize,
    inner_end: usize,
    ast: Script,
}

impl Parsed {
    /// The expression of an expression fragment (inside the parentheses the
    /// runtime adds).
    fn expression(&self) -> Option<&Expression> {
        if self.script {
            return None;
        }
        let [StatementListItem::Statement(statement)] = self.ast.statements().statements() else {
            return None;
        };
        let Statement::Expression(Expression::Parenthesized(inner)) = statement.as_ref() else {
            return None;
        };
        Some(inner.expression())
    }

    /// Byte range of `span` in the caller's source.
    fn range(&self, span: Span) -> (usize, usize) {
        let map = |position: Position| {
            crate::syntax::byte_offset(&self.text, position)
                .saturating_sub(self.prefix)
                .saturating_add(self.inner_start)
                .clamp(self.inner_start, self.inner_end)
        };
        let start = map(span.start());
        let end = map(span.end()).max(start);
        (start, end)
    }
}

fn parse(fragment: &EffectFragment<'_>, interner: &mut Interner) -> Option<Parsed> {
    let script = fragment.role == EffectRole::Script;
    let source = fragment.source;
    if source.as_bytes().contains(&0) || nesting::is_blank(source) {
        return None;
    }
    let location = fragment.role == EffectRole::Location;
    let (text, prefix, inner_start, inner_end) = if script {
        (source.to_owned(), 0, 0, source.len())
    } else if location {
        (format!("({source})"), 1, 0, source.len())
    } else {
        let (start, inner) = crate::syntax::expression_body(source);
        (format!("({inner})"), 1, start, start + inner.len())
    };
    if check_nesting(&text).is_err() {
        return None;
    }
    let ast = Parser::new(Source::from_bytes(&text))
        .parse_script(&Scope::new_global(), interner)
        .ok()?;
    let parsed = Parsed {
        role_checked: fragment.role.checked(),
        data_id: match fragment.role {
            EffectRole::Data(id) => Some(id.to_owned()),
            _ => None,
        },
        location,
        script,
        text,
        prefix,
        inner_start,
        inner_end,
        ast,
    };
    if !script && parsed.expression().is_none() {
        return None;
    }
    Some(parsed)
}

// ---------------------------------------------------------------------------
// Built-in knowledge (ECMA-262 standard library)
// ---------------------------------------------------------------------------

/// Global functions whose call changes nothing (ECMA-262 §19.2 and the
/// constructors called as functions), plus the data model's `In` (B.2.3).
const PURE_GLOBAL_FUNCTIONS: &[&str] = &[
    "In",
    "parseInt",
    "parseFloat",
    "isNaN",
    "isFinite",
    "encodeURI",
    "encodeURIComponent",
    "decodeURI",
    "decodeURIComponent",
    "escape",
    "unescape",
    "String",
    "Number",
    "Boolean",
    "Array",
    "Object",
    "Symbol",
    "BigInt",
    "Date",
    "RegExp",
    "Error",
    "TypeError",
    "RangeError",
    "SyntaxError",
    "ReferenceError",
    "EvalError",
    "URIError",
    "AggregateError",
];

/// Constructors whose `new` creates a fresh object and changes nothing
/// else.
const PURE_CONSTRUCTORS: &[&str] = &[
    "Object",
    "Array",
    "Date",
    "RegExp",
    "Map",
    "Set",
    "WeakMap",
    "WeakSet",
    "WeakRef",
    "Error",
    "TypeError",
    "RangeError",
    "SyntaxError",
    "ReferenceError",
    "EvalError",
    "URIError",
    "AggregateError",
    "String",
    "Number",
    "Boolean",
    "ArrayBuffer",
    "DataView",
    "Int8Array",
    "Uint8Array",
    "Uint8ClampedArray",
    "Int16Array",
    "Uint16Array",
    "Int32Array",
    "Uint32Array",
    "Float32Array",
    "Float64Array",
    "BigInt64Array",
    "BigUint64Array",
];

/// Built-in global names: a write rooted at one changes the standard
/// library for every later expression.
const BUILTIN_NAMES: &[&str] = &[
    "Math",
    "JSON",
    "Object",
    "Array",
    "String",
    "Number",
    "Boolean",
    "Date",
    "RegExp",
    "Function",
    "Symbol",
    "Reflect",
    "Map",
    "Set",
    "WeakMap",
    "WeakSet",
    "Error",
    "globalThis",
    "In",
    "parseInt",
    "parseFloat",
    "isNaN",
    "isFinite",
    "eval",
];

/// Methods ECMA-262 defines to change their receiver: `Array.prototype`
/// (§23.1.3: `copyWithin`, `fill`, `pop`, `push`, `reverse`, `shift`,
/// `sort`, `splice`, `unshift`), typed arrays (`set`), `Map`/`Set`/
/// `WeakMap`/`WeakSet` (`set`, `add`, `delete`, `clear`),
/// `Date.prototype` setters, `DataView` setters, `RegExp.prototype.compile`,
/// resizable buffers, and iterator and generator stepping (`next`,
/// `return`, `throw`).
const MUTATING_METHODS: &[&str] = &[
    "push",
    "pop",
    "shift",
    "unshift",
    "splice",
    "sort",
    "reverse",
    "fill",
    "copyWithin",
    "set",
    "add",
    "delete",
    "clear",
    "setFullYear",
    "setMonth",
    "setDate",
    "setHours",
    "setMinutes",
    "setSeconds",
    "setMilliseconds",
    "setTime",
    "setYear",
    "setUTCFullYear",
    "setUTCMonth",
    "setUTCDate",
    "setUTCHours",
    "setUTCMinutes",
    "setUTCSeconds",
    "setUTCMilliseconds",
    "setInt8",
    "setUint8",
    "setInt16",
    "setUint16",
    "setInt32",
    "setUint32",
    "setFloat32",
    "setFloat64",
    "setBigInt64",
    "setBigUint64",
    "compile",
    "resize",
    "transfer",
    "transferToFixedLength",
    "next",
    "return",
    "throw",
];

/// `RegExp.prototype` methods that advance `lastIndex` on a global or
/// sticky expression (ECMA-262 §22.2.7.2 `RegExpBuiltinExec`).
const REGEXP_STEPPING_METHODS: &[&str] = &["exec", "test"];

/// `String.prototype` methods that run a `RegExp` argument's matcher, which
/// updates a global or sticky expression's `lastIndex`.
const REGEXP_ARGUMENT_METHODS: &[&str] = &[
    "match",
    "matchAll",
    "replace",
    "replaceAll",
    "search",
    "split",
];

/// Methods that change nothing: reads of `Array.prototype`,
/// `String.prototype`, `Number.prototype`, `Object.prototype`,
/// `Date.prototype` getters and conversions, `Map`/`Set` reads, `Function`
/// reads, and the data model's read-only DOM (B.2.1).
const PURE_METHODS: &[&str] = &[
    "at",
    "concat",
    "entries",
    "every",
    "filter",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "flat",
    "flatMap",
    "forEach",
    "includes",
    "indexOf",
    "join",
    "keys",
    "lastIndexOf",
    "map",
    "reduce",
    "reduceRight",
    "slice",
    "some",
    "toLocaleString",
    "toString",
    "values",
    "toReversed",
    "toSorted",
    "toSpliced",
    "with",
    "charAt",
    "charCodeAt",
    "codePointAt",
    "endsWith",
    "localeCompare",
    "normalize",
    "padEnd",
    "padStart",
    "repeat",
    "startsWith",
    "substring",
    "substr",
    "toLowerCase",
    "toUpperCase",
    "toLocaleLowerCase",
    "toLocaleUpperCase",
    "trim",
    "trimStart",
    "trimEnd",
    "trimLeft",
    "trimRight",
    "isWellFormed",
    "toWellFormed",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "valueOf",
    "toFixed",
    "toPrecision",
    "toExponential",
    "getDate",
    "getDay",
    "getFullYear",
    "getHours",
    "getMilliseconds",
    "getMinutes",
    "getMonth",
    "getSeconds",
    "getTime",
    "getTimezoneOffset",
    "getUTCDate",
    "getUTCDay",
    "getUTCFullYear",
    "getUTCHours",
    "getUTCMilliseconds",
    "getUTCMinutes",
    "getUTCMonth",
    "getUTCSeconds",
    "getYear",
    "toDateString",
    "toISOString",
    "toJSON",
    "toTimeString",
    "toUTCString",
    "toGMTString",
    "toLocaleDateString",
    "toLocaleTimeString",
    "get",
    "has",
    "bind",
    "getElementsByTagName",
    "getElementsByTagNameNS",
    "getAttribute",
    "getAttributeNS",
    "getAttributeNode",
    "hasAttribute",
    "hasAttributeNS",
    "hasChildNodes",
];

/// Pure methods whose function arguments are called (with the receiver's
/// elements).
const CALLBACK_METHODS: &[&str] = &[
    "every",
    "filter",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "flatMap",
    "forEach",
    "map",
    "reduce",
    "reduceRight",
    "some",
    "toSorted",
    "sort",
    "replace",
    "replaceAll",
];

/// Methods whose result is a fresh object (a shallow copy or a new value).
const FRESH_RESULT_METHODS: &[&str] = &[
    "concat",
    "filter",
    "flat",
    "flatMap",
    "map",
    "slice",
    "toReversed",
    "toSorted",
    "toSpliced",
    "with",
    "split",
    "keys",
    "values",
    "entries",
    "match",
    "matchAll",
];

/// Namespace functions that change nothing: `Math` (except `random`),
/// `JSON`, `Object` and `Reflect` reads, `Array`, `Number`, `String`,
/// `Date`, and `Symbol` statics.
#[allow(
    clippy::too_many_lines,
    reason = "one closed table of the ECMA-262 namespace functions"
)]
fn namespace_function(namespace: &str, name: &str) -> Option<Namespaced> {
    let pure = match namespace {
        "Math" => {
            if name == "random" {
                return Some(Namespaced::Random);
            }
            matches!(
                name,
                "abs"
                    | "acos"
                    | "acosh"
                    | "asin"
                    | "asinh"
                    | "atan"
                    | "atanh"
                    | "atan2"
                    | "cbrt"
                    | "ceil"
                    | "clz32"
                    | "cos"
                    | "cosh"
                    | "exp"
                    | "expm1"
                    | "floor"
                    | "fround"
                    | "hypot"
                    | "imul"
                    | "log"
                    | "log1p"
                    | "log10"
                    | "log2"
                    | "max"
                    | "min"
                    | "pow"
                    | "round"
                    | "sign"
                    | "sin"
                    | "sinh"
                    | "sqrt"
                    | "tan"
                    | "tanh"
                    | "trunc"
                    | "f16round"
            )
        }
        "JSON" => matches!(name, "parse" | "stringify"),
        "Object" => {
            if matches!(
                name,
                "assign"
                    | "defineProperty"
                    | "defineProperties"
                    | "setPrototypeOf"
                    | "freeze"
                    | "seal"
                    | "preventExtensions"
            ) {
                return Some(Namespaced::MutatesFirstArgument);
            }
            matches!(
                name,
                "keys"
                    | "values"
                    | "entries"
                    | "getOwnPropertyNames"
                    | "getOwnPropertySymbols"
                    | "getOwnPropertyDescriptor"
                    | "getOwnPropertyDescriptors"
                    | "getPrototypeOf"
                    | "is"
                    | "isFrozen"
                    | "isSealed"
                    | "isExtensible"
                    | "hasOwn"
                    | "fromEntries"
                    | "create"
                    | "groupBy"
            )
        }
        "Reflect" => {
            if matches!(
                name,
                "set"
                    | "defineProperty"
                    | "deleteProperty"
                    | "setPrototypeOf"
                    | "preventExtensions"
            ) {
                return Some(Namespaced::MutatesFirstArgument);
            }
            matches!(
                name,
                "get"
                    | "has"
                    | "ownKeys"
                    | "getOwnPropertyDescriptor"
                    | "getPrototypeOf"
                    | "isExtensible"
            )
        }
        "Array" => matches!(name, "isArray" | "from" | "of"),
        "Number" => matches!(
            name,
            "isFinite" | "isInteger" | "isNaN" | "isSafeInteger" | "parseFloat" | "parseInt"
        ),
        "String" => matches!(name, "fromCharCode" | "fromCodePoint" | "raw"),
        "Date" => matches!(name, "now" | "parse" | "UTC"),
        "Symbol" => matches!(name, "for" | "keyFor"),
        _ => return None,
    };
    Some(if pure {
        Namespaced::Pure
    } else {
        Namespaced::Unknown
    })
}

/// A built-in namespace function's class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Namespaced {
    Pure,
    Random,
    MutatesFirstArgument,
    Unknown,
}

/// Property names a document conversion or iteration protocol calls
/// implicitly.
const IMPLICIT_METHOD_NAMES: &[&str] = &["valueOf", "toString", "toJSON", "toLocaleString"];

// ---------------------------------------------------------------------------
// Origins
// ---------------------------------------------------------------------------

/// Where a value's object comes from, ordered from most to least local; the
/// join of two origins is the later one.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum Origin {
    /// A fresh value whose reachable objects are all fresh (a literal of
    /// primitives, a function), or a primitive.
    FreshDeep,
    /// A fresh object whose members may be shared (a copy, a `new` object).
    FreshShallow,
    /// Unknown.
    Unknown,
    /// Reachable from outside the checked expression (a data-model
    /// variable, the global object, a built-in).
    NonLocal,
}

impl Origin {
    const fn member(self) -> Self {
        match self {
            Self::FreshDeep => Self::FreshDeep,
            Self::FreshShallow | Self::Unknown => Self::Unknown,
            Self::NonLocal => Self::NonLocal,
        }
    }

    fn join(self, other: Self) -> Self {
        self.max(other)
    }
}

// ---------------------------------------------------------------------------
// Document survey
// ---------------------------------------------------------------------------

/// A function the analysis can walk.
#[derive(Clone, Copy)]
struct Function<'ast> {
    parameters: &'ast FormalParameterList,
    body: &'ast FunctionBody,
    arrow: bool,
    generator: bool,
    fragment: usize,
    name: Sym,
}

impl Function<'_> {
    fn key(&self) -> usize {
        std::ptr::from_ref(self.body) as usize
    }
}

/// What the whole document declares and writes.
#[derive(Default)]
struct Survey<'ast> {
    /// Document functions by name, with how many times the name is
    /// declared.
    functions: BTreeMap<Sym, Function<'ast>>,
    declarations: BTreeMap<Sym, usize>,
    /// Names assigned anywhere (so a function bound to one may change).
    assigned: BTreeSet<Sym>,
    /// A write reaches a built-in or a prototype.
    builtins_changed: bool,
    /// Methods the document defines, by name.
    methods: BTreeMap<Sym, Vec<Function<'ast>>>,
    /// Getters, setters, conversion methods, and generators.
    implicit: Vec<Function<'ast>>,
}

struct SurveyVisitor<'s, 'ast> {
    survey: &'s mut Survey<'ast>,
    interner: &'s Interner,
    fragment: usize,
}

impl<'ast> SurveyVisitor<'_, 'ast> {
    fn name(&self, sym: Sym) -> String {
        self.interner
            .resolve(sym)
            .map(|name| name.to_string())
            .unwrap_or_default()
    }

    fn is_builtin(&self, sym: Sym) -> bool {
        BUILTIN_NAMES.contains(&self.name(sym).as_str())
    }

    fn function(&self, expression: &'ast Expression, name: Sym) -> Option<Function<'ast>> {
        function_of(expression, self.fragment, name)
    }

    fn member_write(&mut self, access: &'ast PropertyAccess) {
        let mut current = access;
        loop {
            let PropertyAccess::Simple(simple) = current else {
                self.survey.builtins_changed = true;
                return;
            };
            if let PropertyAccessField::Const(field) = simple.field() {
                let field = self.name(field.sym());
                if matches!(field.as_str(), "prototype" | "__proto__" | "constructor") {
                    self.survey.builtins_changed = true;
                }
            }
            match strip(simple.target()) {
                Expression::PropertyAccess(inner) => current = inner,
                Expression::Identifier(root) => {
                    if self.is_builtin(root.sym()) {
                        self.survey.builtins_changed = true;
                    }
                    return;
                }
                Expression::This(_) => {
                    self.survey.builtins_changed = true;
                    return;
                }
                _ => return,
            }
        }
    }

    fn pattern_writes(&mut self, pattern: &'ast Pattern) {
        match pattern {
            Pattern::Object(object) => {
                for element in object.bindings() {
                    match element {
                        ObjectPatternElement::SingleName { ident, .. }
                        | ObjectPatternElement::RestProperty { ident, .. } => {
                            self.survey.assigned.insert(ident.sym());
                        }
                        ObjectPatternElement::AssignmentPropertyAccess { access, .. }
                        | ObjectPatternElement::AssignmentRestPropertyAccess { access, .. } => {
                            self.member_write(access);
                        }
                        ObjectPatternElement::Pattern { pattern, .. } => {
                            self.pattern_writes(pattern);
                        }
                    }
                }
            }
            Pattern::Array(array) => {
                for element in array.bindings() {
                    match element {
                        ArrayPatternElement::SingleName { ident, .. }
                        | ArrayPatternElement::SingleNameRest { ident } => {
                            self.survey.assigned.insert(ident.sym());
                        }
                        ArrayPatternElement::PropertyAccess { access, .. }
                        | ArrayPatternElement::PropertyAccessRest { access } => {
                            self.member_write(access);
                        }
                        ArrayPatternElement::Pattern { pattern, .. }
                        | ArrayPatternElement::PatternRest { pattern } => {
                            self.pattern_writes(pattern);
                        }
                        ArrayPatternElement::Elision => {}
                    }
                }
            }
        }
    }

    fn method(
        &mut self,
        name: &'ast PropertyName,
        function: Function<'ast>,
        kind: MethodDefinitionKind,
    ) {
        let getter_or_setter =
            matches!(kind, MethodDefinitionKind::Get | MethodDefinitionKind::Set);
        let generator = matches!(
            kind,
            MethodDefinitionKind::Generator | MethodDefinitionKind::AsyncGenerator
        );
        match name {
            PropertyName::Literal(identifier) => {
                let sym = identifier.sym();
                let implicit = IMPLICIT_METHOD_NAMES.contains(&self.name(sym).as_str());
                if getter_or_setter || generator || implicit {
                    self.survey.implicit.push(function);
                } else {
                    self.survey.methods.entry(sym).or_default().push(function);
                }
            }
            PropertyName::Computed(_) => self.survey.implicit.push(function),
        }
    }
}

impl<'ast> Visitor<'ast> for SurveyVisitor<'_, 'ast> {
    type BreakTy = std::convert::Infallible;

    fn visit_assign(
        &mut self,
        node: &'ast boa_engine::ast::expression::operator::Assign,
    ) -> std::ops::ControlFlow<Self::BreakTy> {
        match node.lhs() {
            AssignTarget::Identifier(identifier) => {
                self.survey.assigned.insert(identifier.sym());
                if self.is_builtin(identifier.sym()) {
                    self.survey.builtins_changed = true;
                }
            }
            AssignTarget::Access(access) => {
                self.member_write(access);
                if let PropertyAccess::Simple(simple) = access
                    && let PropertyAccessField::Const(field) = simple.field()
                    && let Some(function) = self.function(node.rhs(), field.sym())
                {
                    self.survey
                        .methods
                        .entry(field.sym())
                        .or_default()
                        .push(function);
                    if IMPLICIT_METHOD_NAMES.contains(&self.name(field.sym()).as_str()) {
                        self.survey.implicit.push(function);
                    }
                }
            }
            AssignTarget::Pattern(pattern) => self.pattern_writes(pattern),
        }
        node.visit_with(self)
    }

    fn visit_update(
        &mut self,
        node: &'ast boa_engine::ast::expression::operator::Update,
    ) -> std::ops::ControlFlow<Self::BreakTy> {
        match node.target() {
            UpdateTarget::Identifier(identifier) => {
                self.survey.assigned.insert(identifier.sym());
            }
            UpdateTarget::PropertyAccess(access) => self.member_write(access),
        }
        node.visit_with(self)
    }

    fn visit_unary(
        &mut self,
        node: &'ast boa_engine::ast::expression::operator::Unary,
    ) -> std::ops::ControlFlow<Self::BreakTy> {
        if node.op() == UnaryOp::Delete {
            match strip(node.target()) {
                Expression::Identifier(identifier) => {
                    self.survey.assigned.insert(identifier.sym());
                }
                Expression::PropertyAccess(access) => self.member_write(access),
                _ => {}
            }
        }
        node.visit_with(self)
    }

    fn visit_call(
        &mut self,
        node: &'ast boa_engine::ast::expression::Call,
    ) -> std::ops::ControlFlow<Self::BreakTy> {
        // `Object.defineProperty(target, ...)` and kin change `target`; a
        // descriptor with `get`/`set` defines an accessor.
        if let Expression::PropertyAccess(PropertyAccess::Simple(simple)) = strip(node.function())
            && let PropertyAccessField::Const(method) = simple.field()
        {
            let method = self.name(method.sym());
            let namespace = match strip(simple.target()) {
                Expression::Identifier(identifier) => Some(self.name(identifier.sym())),
                _ => None,
            };
            if let Some(namespace) = namespace.as_deref()
                && namespace_function(namespace, &method) == Some(Namespaced::MutatesFirstArgument)
                && let Some(first) = node.args().first()
            {
                match strip(first) {
                    Expression::Identifier(identifier) => {
                        if self.is_builtin(identifier.sym()) {
                            self.survey.builtins_changed = true;
                        }
                        self.survey.assigned.insert(identifier.sym());
                    }
                    Expression::PropertyAccess(access) => self.member_write(access),
                    Expression::This(_) => self.survey.builtins_changed = true,
                    _ => {}
                }
            }
            if matches!(method.as_str(), "__defineGetter__" | "__defineSetter__") {
                for argument in node.args() {
                    if let Some(function) = self.function(argument, Sym::EMPTY_STRING) {
                        self.survey.implicit.push(function);
                    }
                }
            }
            if matches!(method.as_str(), "defineProperty" | "defineProperties") {
                for argument in node.args() {
                    collect_accessor_descriptors(
                        argument,
                        self.fragment,
                        &mut self.survey.implicit,
                        self.interner,
                    );
                }
            }
        }
        node.visit_with(self)
    }

    fn visit_for_in_loop(
        &mut self,
        node: &'ast boa_engine::ast::statement::iteration::ForInLoop,
    ) -> std::ops::ControlFlow<Self::BreakTy> {
        self.loop_head(node.initializer());
        node.visit_with(self)
    }

    fn visit_for_of_loop(
        &mut self,
        node: &'ast boa_engine::ast::statement::iteration::ForOfLoop,
    ) -> std::ops::ControlFlow<Self::BreakTy> {
        self.loop_head(node.initializer());
        node.visit_with(self)
    }

    fn visit_property_definition(
        &mut self,
        node: &'ast PropertyDefinition,
    ) -> std::ops::ControlFlow<Self::BreakTy> {
        match node {
            PropertyDefinition::MethodDefinition(method) => {
                let function = Function {
                    parameters: method.parameters(),
                    body: method.body(),
                    arrow: false,
                    generator: matches!(
                        method.kind(),
                        MethodDefinitionKind::Generator | MethodDefinitionKind::AsyncGenerator
                    ),
                    fragment: self.fragment,
                    name: property_sym(method.name()),
                };
                self.method(method.name(), function, method.kind());
            }
            PropertyDefinition::Property(name, value) => {
                if let Some(function) = self.function(value, property_sym(name)) {
                    self.method(name, function, MethodDefinitionKind::Ordinary);
                }
            }
            _ => {}
        }
        node.visit_with(self)
    }

    fn visit_class_element(
        &mut self,
        node: &'ast ClassElement,
    ) -> std::ops::ControlFlow<Self::BreakTy> {
        if let ClassElement::MethodDefinition(method) = node {
            let function = Function {
                parameters: method.parameters(),
                body: method.body(),
                arrow: false,
                generator: matches!(
                    method.kind(),
                    MethodDefinitionKind::Generator | MethodDefinitionKind::AsyncGenerator
                ),
                fragment: self.fragment,
                name: Sym::EMPTY_STRING,
            };
            match method.name() {
                boa_engine::ast::function::ClassElementName::PropertyName(name) => {
                    self.method(name, function, method.kind());
                }
                boa_engine::ast::function::ClassElementName::PrivateName(_) => {
                    if matches!(
                        method.kind(),
                        MethodDefinitionKind::Get | MethodDefinitionKind::Set
                    ) {
                        self.survey.implicit.push(function);
                    }
                }
            }
        }
        node.visit_with(self)
    }

    fn visit_generator_expression(
        &mut self,
        node: &'ast boa_engine::ast::function::GeneratorExpression,
    ) -> std::ops::ControlFlow<Self::BreakTy> {
        self.survey.implicit.push(Function {
            parameters: node.parameters(),
            body: node.body(),
            arrow: false,
            generator: true,
            fragment: self.fragment,
            name: node.name().map_or(Sym::EMPTY_STRING, Identifier::sym),
        });
        node.visit_with(self)
    }

    fn visit_generator_declaration(
        &mut self,
        node: &'ast boa_engine::ast::function::GeneratorDeclaration,
    ) -> std::ops::ControlFlow<Self::BreakTy> {
        self.survey.implicit.push(Function {
            parameters: node.parameters(),
            body: node.body(),
            arrow: false,
            generator: true,
            fragment: self.fragment,
            name: node.name().sym(),
        });
        node.visit_with(self)
    }

    fn visit_async_generator_expression(
        &mut self,
        node: &'ast boa_engine::ast::function::AsyncGeneratorExpression,
    ) -> std::ops::ControlFlow<Self::BreakTy> {
        self.survey.implicit.push(Function {
            parameters: node.parameters(),
            body: node.body(),
            arrow: false,
            generator: true,
            fragment: self.fragment,
            name: node.name().map_or(Sym::EMPTY_STRING, Identifier::sym),
        });
        node.visit_with(self)
    }

    fn visit_async_generator_declaration(
        &mut self,
        node: &'ast boa_engine::ast::function::AsyncGeneratorDeclaration,
    ) -> std::ops::ControlFlow<Self::BreakTy> {
        self.survey.implicit.push(Function {
            parameters: node.parameters(),
            body: node.body(),
            arrow: false,
            generator: true,
            fragment: self.fragment,
            name: node.name().sym(),
        });
        node.visit_with(self)
    }
}

impl<'ast> SurveyVisitor<'_, 'ast> {
    fn loop_head(&mut self, initializer: &'ast IterableLoopInitializer) {
        match initializer {
            IterableLoopInitializer::Identifier(identifier) => {
                self.survey.assigned.insert(identifier.sym());
            }
            IterableLoopInitializer::Access(access) => self.member_write(access),
            IterableLoopInitializer::Pattern(pattern) => self.pattern_writes(pattern),
            IterableLoopInitializer::Var(_)
            | IterableLoopInitializer::Let(_)
            | IterableLoopInitializer::Const(_) => {}
        }
    }
}

/// Accessor functions of `{ get() {...}, set: function () {...} }`
/// descriptor literals inside `expression`.
fn collect_accessor_descriptors<'ast>(
    expression: &'ast Expression,
    fragment: usize,
    implicit: &mut Vec<Function<'ast>>,
    interner: &Interner,
) {
    let Expression::ObjectLiteral(object) = strip(expression) else {
        return;
    };
    for property in object.properties() {
        match property {
            PropertyDefinition::MethodDefinition(method) => {
                let name = property_name(method.name(), interner);
                if matches!(name.as_deref(), Some("get" | "set")) {
                    implicit.push(method_function(method, fragment));
                }
            }
            PropertyDefinition::Property(name, value) => {
                if matches!(
                    property_name(name, interner).as_deref(),
                    Some("get" | "set")
                ) && let Some(function) = function_of(value, fragment, Sym::EMPTY_STRING)
                {
                    implicit.push(function);
                } else {
                    collect_accessor_descriptors(value, fragment, implicit, interner);
                }
            }
            _ => {}
        }
    }
}

fn method_function(method: &ObjectMethodDefinition, fragment: usize) -> Function<'_> {
    Function {
        parameters: method.parameters(),
        body: method.body(),
        arrow: false,
        generator: matches!(
            method.kind(),
            MethodDefinitionKind::Generator | MethodDefinitionKind::AsyncGenerator
        ),
        fragment,
        name: property_sym(method.name()),
    }
}

fn property_sym(name: &PropertyName) -> Sym {
    match name {
        PropertyName::Literal(identifier) => identifier.sym(),
        PropertyName::Computed(_) => Sym::EMPTY_STRING,
    }
}

fn property_name(name: &PropertyName, interner: &Interner) -> Option<String> {
    match name {
        PropertyName::Literal(identifier) => interner
            .resolve(identifier.sym())
            .map(|name| name.to_string()),
        PropertyName::Computed(expression) => match strip(expression) {
            Expression::Literal(literal) => literal
                .as_string()
                .and_then(|sym| interner.resolve(sym))
                .map(|name| name.to_string()),
            _ => None,
        },
    }
}

/// Where `expression` starts. Boa gives a call the span of its arguments,
/// and expressions built on one (a member access, a binary operation)
/// inherit that start, so the start is recomputed from the leftmost
/// operand.
fn start_of(expression: &Expression) -> Position {
    match expression {
        Expression::Call(call) => start_of(call.function()),
        Expression::PropertyAccess(PropertyAccess::Simple(simple)) => start_of(simple.target()),
        Expression::PropertyAccess(PropertyAccess::Private(private)) => start_of(private.target()),
        Expression::Binary(binary) => start_of(binary.lhs()),
        Expression::Conditional(conditional) => start_of(conditional.condition()),
        Expression::Optional(optional) => start_of(optional.target()),
        Expression::TaggedTemplate(tagged) => start_of(tagged.tag()),
        Expression::Assign(assign) => match assign.lhs() {
            AssignTarget::Access(access) => access_span(access).start(),
            _ => assign.span().start(),
        },
        other => other.span().start(),
    }
}

/// The whole range of `expression`.
fn full_span(expression: &Expression) -> Span {
    Span::new(start_of(expression), expression.span().end())
}

/// The whole range of a property access.
fn access_span(access: &PropertyAccess) -> Span {
    match access {
        PropertyAccess::Simple(simple) => Span::new(start_of(simple.target()), simple.span().end()),
        PropertyAccess::Private(private) => {
            Span::new(start_of(private.target()), private.span().end())
        }
        PropertyAccess::Super(sup) => sup.span(),
    }
}

/// `expression` without enclosing parentheses.
fn strip(mut expression: &Expression) -> &Expression {
    while let Expression::Parenthesized(inner) = expression {
        expression = inner.expression();
    }
    expression
}

/// The function a function or arrow expression denotes.
fn function_of(expression: &Expression, fragment: usize, name: Sym) -> Option<Function<'_>> {
    Some(match strip(expression) {
        Expression::FunctionExpression(function) => Function {
            parameters: function.parameters(),
            body: function.body(),
            arrow: false,
            generator: false,
            fragment,
            name: function.name().map_or(name, Identifier::sym),
        },
        Expression::ArrowFunction(function) => Function {
            parameters: function.parameters(),
            body: function.body(),
            arrow: true,
            generator: false,
            fragment,
            name: function.name().map_or(name, Identifier::sym),
        },
        Expression::AsyncFunctionExpression(function) => Function {
            parameters: function.parameters(),
            body: function.body(),
            arrow: false,
            generator: false,
            fragment,
            name: function.name().map_or(name, Identifier::sym),
        },
        Expression::AsyncArrowFunction(function) => Function {
            parameters: function.parameters(),
            body: function.body(),
            arrow: true,
            generator: false,
            fragment,
            name: function.name().map_or(name, Identifier::sym),
        },
        Expression::GeneratorExpression(function) => Function {
            parameters: function.parameters(),
            body: function.body(),
            arrow: false,
            generator: true,
            fragment,
            name: function.name().map_or(name, Identifier::sym),
        },
        Expression::AsyncGeneratorExpression(function) => Function {
            parameters: function.parameters(),
            body: function.body(),
            arrow: false,
            generator: true,
            fragment,
            name: function.name().map_or(name, Identifier::sym),
        },
        _ => return None,
    })
}

/// Declares `name` (once more) and, when `value` is a function, binds it.
fn declare<'ast>(survey: &mut Survey<'ast>, name: Sym, function: Option<Function<'ast>>) {
    *survey.declarations.entry(name).or_default() += 1;
    if let Some(function) = function {
        survey.functions.insert(name, function);
    }
}

/// The top-level declarations of a script.
fn declare_script<'ast>(survey: &mut Survey<'ast>, script: &'ast Script, fragment: usize) {
    for item in script.statements().statements() {
        match item {
            StatementListItem::Declaration(declaration) => match declaration.as_ref() {
                Declaration::FunctionDeclaration(function) => declare(
                    survey,
                    function.name().sym(),
                    Some(Function {
                        parameters: function.parameters(),
                        body: function.body(),
                        arrow: false,
                        generator: false,
                        fragment,
                        name: function.name().sym(),
                    }),
                ),
                Declaration::AsyncFunctionDeclaration(function) => declare(
                    survey,
                    function.name().sym(),
                    Some(Function {
                        parameters: function.parameters(),
                        body: function.body(),
                        arrow: false,
                        generator: false,
                        fragment,
                        name: function.name().sym(),
                    }),
                ),
                Declaration::GeneratorDeclaration(function) => declare(
                    survey,
                    function.name().sym(),
                    Some(Function {
                        parameters: function.parameters(),
                        body: function.body(),
                        arrow: false,
                        generator: true,
                        fragment,
                        name: function.name().sym(),
                    }),
                ),
                Declaration::AsyncGeneratorDeclaration(function) => declare(
                    survey,
                    function.name().sym(),
                    Some(Function {
                        parameters: function.parameters(),
                        body: function.body(),
                        arrow: false,
                        generator: true,
                        fragment,
                        name: function.name().sym(),
                    }),
                ),
                Declaration::ClassDeclaration(class) => declare(survey, class.name().sym(), None),
                Declaration::Lexical(lexical) => {
                    declare_variables(survey, lexical_variables(lexical), fragment);
                }
            },
            StatementListItem::Statement(statement) => {
                if let Statement::Var(var) = statement.as_ref() {
                    declare_variables(survey, var.0.as_ref(), fragment);
                }
            }
        }
    }
}

fn lexical_variables(lexical: &LexicalDeclaration) -> &[Variable] {
    lexical.variable_list().as_ref()
}

fn declare_variables<'ast>(
    survey: &mut Survey<'ast>,
    variables: &'ast [Variable],
    fragment: usize,
) {
    for variable in variables {
        match variable.binding() {
            Binding::Identifier(identifier) => {
                let function = variable
                    .init()
                    .and_then(|init| function_of(init, fragment, identifier.sym()));
                declare(survey, identifier.sym(), function);
            }
            Binding::Pattern(pattern) => {
                for name in boa_engine::ast::operations::bound_names(pattern) {
                    declare(survey, name, None);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The walk
// ---------------------------------------------------------------------------

/// One effect found while walking, before it is reported.
#[derive(Clone, Debug)]
struct Event {
    certainty: Certainty,
    kind: EffectKind,
    span: EffectSpan,
    subject: String,
    cause: Option<EffectCause>,
}

/// A function scope: its bindings and their origins.
struct Frame<'ast> {
    names: BTreeMap<Sym, Origin>,
    functions: BTreeMap<Sym, Function<'ast>>,
    /// `this` of a non-arrow function.
    this: Option<Origin>,
}

/// Whole-analysis context shared by every walk.
struct Context<'a, 'ast> {
    parsed: &'a [Option<Parsed>],
    interner: &'a Interner,
    survey: &'a Survey<'ast>,
    /// A script the analysis could not read exists.
    unseen: bool,
    /// Effects of the document's implicit functions, when any has one.
    implicit_cause: Option<EffectCause>,
}

impl<'ast> Context<'_, 'ast> {
    fn name(&self, sym: Sym) -> String {
        bounded(
            self.interner
                .resolve(sym)
                .map(|name| name.to_string())
                .unwrap_or_default(),
        )
    }

    /// A stable document function named `sym`.
    fn document_function(&self, sym: Sym) -> Option<Function<'ast>> {
        let function = self.survey.functions.get(&sym)?;
        (self.survey.declarations.get(&sym) == Some(&1) && !self.survey.assigned.contains(&sym))
            .then_some(*function)
    }

    fn declared(&self, sym: Sym) -> bool {
        self.survey.declarations.contains_key(&sym)
    }
}

fn bounded(mut text: String) -> String {
    if text.len() > MAX_EFFECT_SUBJECT_BYTES {
        let mut end = MAX_EFFECT_SUBJECT_BYTES;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

/// Memoized outcome of one document function call shape.
type CallKey = (usize, Vec<Origin>, Origin);

struct Walker<'c, 'a, 'ast> {
    context: &'c Context<'a, 'ast>,
    frames: Vec<Frame<'ast>>,
    fragment: usize,
    events: Vec<Event>,
    active: Vec<usize>,
    steps: usize,
    exhausted: bool,
    reads_global: bool,
    memo: &'c mut BTreeMap<CallKey, (Vec<Event>, bool)>,
}

type Flow = std::ops::ControlFlow<std::convert::Infallible>;
const GO: Flow = std::ops::ControlFlow::Continue(());

impl<'a, 'ast> Walker<'_, 'a, 'ast> {
    fn parsed(&self, fragment: usize) -> Option<&'a Parsed> {
        self.context.parsed.get(fragment).and_then(Option::as_ref)
    }

    fn span(&self, span: Span) -> EffectSpan {
        let (start, end) = self
            .parsed(self.fragment)
            .map_or((0, 0), |parsed| parsed.range(span));
        EffectSpan {
            fragment: self.fragment,
            start,
            end,
        }
    }

    fn text(&self, span: Span) -> String {
        let range = self.span(span);
        let source = self.parsed(self.fragment).map_or("", |parsed| {
            let start = range
                .start
                .saturating_sub(parsed.inner_start)
                .saturating_add(parsed.prefix);
            let end = range
                .end
                .saturating_sub(parsed.inner_start)
                .saturating_add(parsed.prefix);
            parsed.text.get(start..end).unwrap_or("")
        });
        bounded(source.to_owned())
    }

    fn push(&mut self, certainty: Certainty, kind: EffectKind, span: Span, subject: String) {
        let span = self.span(span);
        self.events.push(Event {
            certainty,
            kind,
            span,
            subject,
            cause: None,
        });
    }

    /// Where `sym` is bound: a local origin, or `None` for a global.
    fn resolve(&self, sym: Sym) -> Option<Origin> {
        self.frames
            .iter()
            .rev()
            .find_map(|frame| frame.names.get(&sym).copied())
    }

    fn local_function(&self, sym: Sym) -> Option<Function<'ast>> {
        for frame in self.frames.iter().rev() {
            if let Some(function) = frame.functions.get(&sym) {
                return Some(*function);
            }
            if frame.names.contains_key(&sym) {
                return None;
            }
        }
        None
    }

    fn this_origin(&self) -> Origin {
        self.frames
            .iter()
            .rev()
            .find_map(|frame| frame.this)
            .unwrap_or(Origin::NonLocal)
    }

    /// The origin of the value `expression` evaluates to.
    fn origin(&self, expression: &'ast Expression) -> Origin {
        match strip(expression) {
            Expression::Identifier(identifier) => {
                self.resolve(identifier.sym()).unwrap_or(Origin::NonLocal)
            }
            Expression::This(_) => self.this_origin(),
            Expression::Literal(_)
            | Expression::TemplateLiteral(_)
            | Expression::RegExpLiteral(_)
            | Expression::FunctionExpression(_)
            | Expression::ArrowFunction(_)
            | Expression::AsyncArrowFunction(_)
            | Expression::GeneratorExpression(_)
            | Expression::AsyncFunctionExpression(_)
            | Expression::AsyncGeneratorExpression(_)
            | Expression::ClassExpression(_)
            | Expression::Unary(_)
            | Expression::Update(_) => Origin::FreshDeep,
            Expression::ArrayLiteral(array) => array
                .as_ref()
                .iter()
                .flatten()
                .fold(Origin::FreshDeep, |origin, element| {
                    origin.join(self.member_origin(element))
                }),
            Expression::ObjectLiteral(object) => object.properties().iter().fold(
                Origin::FreshDeep,
                |origin, property| match property {
                    PropertyDefinition::Property(_, value) => {
                        origin.join(self.member_origin(value))
                    }
                    PropertyDefinition::IdentifierReference(identifier) => origin.join(match self
                        .resolve(identifier.sym())
                        .unwrap_or(Origin::NonLocal)
                    {
                        Origin::FreshDeep => Origin::FreshDeep,
                        _ => Origin::FreshShallow,
                    }),
                    PropertyDefinition::MethodDefinition(_) => origin,
                    _ => origin.join(Origin::FreshShallow),
                },
            ),
            Expression::PropertyAccess(PropertyAccess::Simple(simple)) => {
                self.origin(simple.target()).member()
            }
            Expression::PropertyAccess(PropertyAccess::Private(private)) => {
                self.origin(private.target()).member()
            }
            Expression::New(_) => Origin::FreshShallow,
            Expression::Call(call) => self.call_result_origin(call.function()),
            Expression::Binary(binary) => match binary.op() {
                BinaryOp::Logical(_) => self.origin(binary.lhs()).join(self.origin(binary.rhs())),
                BinaryOp::Comma => self.origin(binary.rhs()),
                _ => Origin::FreshDeep,
            },
            Expression::Conditional(conditional) => self
                .origin(conditional.if_true())
                .join(self.origin(conditional.if_false())),
            Expression::Assign(assign) => self.origin(assign.rhs()),
            _ => Origin::Unknown,
        }
    }

    /// A member value inside a fresh literal: fresh-deep only when its own
    /// objects are.
    fn member_origin(&self, expression: &'ast Expression) -> Origin {
        match self.origin(expression) {
            Origin::FreshDeep => Origin::FreshDeep,
            _ => Origin::FreshShallow,
        }
    }

    fn call_result_origin(&self, callee: &'ast Expression) -> Origin {
        match strip(callee) {
            Expression::PropertyAccess(PropertyAccess::Simple(simple)) => {
                let Some(method) = self.field_name(simple.field()) else {
                    return Origin::Unknown;
                };
                if FRESH_RESULT_METHODS.contains(&method.as_str()) {
                    return Origin::FreshShallow;
                }
                if let Expression::Identifier(namespace) = strip(simple.target())
                    && self.resolve(namespace.sym()).is_none()
                    && matches!(
                        (self.context.name(namespace.sym()).as_str(), method.as_str()),
                        ("JSON", "parse")
                            | (
                                "Object",
                                "keys" | "values" | "entries" | "create" | "fromEntries"
                            )
                            | ("Array", "from" | "of")
                    )
                {
                    return Origin::FreshShallow;
                }
                Origin::Unknown
            }
            Expression::Identifier(identifier)
                if self.resolve(identifier.sym()).is_none()
                    && PURE_GLOBAL_FUNCTIONS
                        .contains(&self.context.name(identifier.sym()).as_str())
                    && !self.context.declared(identifier.sym()) =>
            {
                Origin::FreshShallow
            }
            _ => Origin::Unknown,
        }
    }

    fn field_name(&self, field: &PropertyAccessField) -> Option<String> {
        match field {
            PropertyAccessField::Const(identifier) => Some(self.context.name(identifier.sym())),
            PropertyAccessField::Expr(expression) => match strip(expression) {
                Expression::Literal(literal) => {
                    literal.as_string().map(|sym| self.context.name(sym))
                }
                _ => None,
            },
        }
    }

    fn step(&mut self) -> bool {
        self.steps = self.steps.saturating_add(1);
        if self.steps > MAX_ANALYSIS_STEPS {
            self.exhausted = true;
        }
        !self.exhausted
    }

    // -- writes ------------------------------------------------------------

    /// A write of the variable `identifier`.
    fn write_identifier(&mut self, kind: EffectKind, identifier: &Identifier) {
        // A write of a system variable itself changes nothing (it fails at
        // run time); the system-variable pass reports it.
        if self.resolve(identifier.sym()).is_none()
            && !SYSTEM_VARIABLES.contains(&self.context.name(identifier.sym()).as_str())
        {
            let subject = self.context.name(identifier.sym());
            self.push(Certainty::Certain, kind, identifier.span(), subject);
        }
    }

    /// A write into the object `origin` at `span`.
    fn write_origin(&mut self, kind: EffectKind, origin: Origin, span: Span, subject: String) {
        match origin {
            Origin::FreshDeep | Origin::FreshShallow => {}
            Origin::Unknown => {
                let kind = if kind == EffectKind::MutatingCall {
                    kind
                } else {
                    EffectKind::UnknownWrite
                };
                self.push(Certainty::Undecidable, kind, span, subject);
            }
            Origin::NonLocal => self.push(Certainty::Certain, kind, span, subject),
        }
    }

    /// A write of a property of the object `access` reads.
    fn write_access(&mut self, kind: EffectKind, access: &'ast PropertyAccess) {
        let origin = match access {
            PropertyAccess::Simple(simple) => self.origin(simple.target()),
            PropertyAccess::Private(private) => self.origin(private.target()),
            PropertyAccess::Super(_) => self.this_origin(),
        };
        let span = access_span(access);
        let subject = self.text(span);
        self.write_origin(kind, origin, span, subject);
    }

    fn write_pattern(&mut self, pattern: &'ast Pattern) {
        match pattern {
            Pattern::Object(object) => {
                for element in object.bindings() {
                    match element {
                        ObjectPatternElement::SingleName { ident, .. }
                        | ObjectPatternElement::RestProperty { ident, .. } => {
                            self.write_identifier(EffectKind::Assignment, ident);
                        }
                        ObjectPatternElement::AssignmentPropertyAccess { access, .. }
                        | ObjectPatternElement::AssignmentRestPropertyAccess { access, .. } => {
                            self.write_access(EffectKind::Assignment, access);
                        }
                        ObjectPatternElement::Pattern { pattern, .. } => {
                            self.write_pattern(pattern);
                        }
                    }
                }
            }
            Pattern::Array(array) => {
                for element in array.bindings() {
                    match element {
                        ArrayPatternElement::SingleName { ident, .. }
                        | ArrayPatternElement::SingleNameRest { ident } => {
                            self.write_identifier(EffectKind::Assignment, ident);
                        }
                        ArrayPatternElement::PropertyAccess { access, .. }
                        | ArrayPatternElement::PropertyAccessRest { access } => {
                            self.write_access(EffectKind::Assignment, access);
                        }
                        ArrayPatternElement::Pattern { pattern, .. }
                        | ArrayPatternElement::PatternRest { pattern } => {
                            self.write_pattern(pattern);
                        }
                        ArrayPatternElement::Elision => {}
                    }
                }
            }
        }
    }

    // -- calls ---------------------------------------------------------------

    /// Walks the arguments that run before a call.
    fn visit_arguments(&mut self, arguments: &'ast [Expression]) {
        for argument in arguments {
            if function_of(argument, self.fragment, Sym::EMPTY_STRING).is_none() {
                let _ = self.visit_expression(argument);
            }
        }
    }

    /// A call of an unresolved callee.
    fn unknown_call(&mut self, span: Span, subject: String) {
        let kind = if self.context.unseen {
            EffectKind::UnseenScript
        } else {
            EffectKind::UnknownCall
        };
        self.push(Certainty::Undecidable, kind, span, subject);
    }

    fn call(
        &mut self,
        callee: &'ast Expression,
        arguments: &'ast [Expression],
        span: Span,
        kind: CallKind,
    ) {
        let callee = strip(callee);
        match callee {
            Expression::Identifier(identifier) => {
                self.visit_arguments(arguments);
                self.call_identifier(identifier, arguments, span, kind);
            }
            Expression::PropertyAccess(PropertyAccess::Simple(simple))
                if kind == CallKind::Call =>
            {
                let _ = self.visit_expression(simple.target());
                if let PropertyAccessField::Expr(key) = simple.field() {
                    let _ = self.visit_expression(key);
                }
                self.visit_arguments(arguments);
                self.call_method(simple.target(), simple.field(), arguments, span);
            }
            _ => {
                if let Some(function) = function_of(callee, self.fragment, Sym::EMPTY_STRING) {
                    self.visit_arguments(arguments);
                    let origins: Vec<Origin> = arguments
                        .iter()
                        .map(|argument| self.origin(argument))
                        .collect();
                    self.invoke(function, &origins, kind.this(), span, CalleeOrigin::Walked);
                    return;
                }
                let _ = self.visit_expression(callee);
                self.visit_arguments(arguments);
                let subject = self.text(full_span(callee));
                self.unknown_call(span, subject);
            }
        }
    }

    fn call_identifier(
        &mut self,
        identifier: &Identifier,
        arguments: &'ast [Expression],
        span: Span,
        kind: CallKind,
    ) {
        let sym = identifier.sym();
        let name = self.context.name(sym);
        let this = kind.this();
        if self.resolve(sym).is_some() {
            match self.local_function(sym) {
                Some(function) => {
                    let origins: Vec<Origin> = arguments
                        .iter()
                        .map(|argument| self.origin(argument))
                        .collect();
                    self.invoke(function, &origins, this, span, CalleeOrigin::Walked);
                }
                None => self.unknown_call(span, name),
            }
            return;
        }
        if let Some(function) = self.context.document_function(sym) {
            if self.context.unseen {
                self.push(Certainty::Undecidable, EffectKind::UnseenScript, span, name);
                return;
            }
            let origins: Vec<Origin> = arguments
                .iter()
                .map(|argument| self.origin(argument))
                .collect();
            self.invoke(function, &origins, this, span, CalleeOrigin::Document);
            return;
        }
        if self.context.declared(sym) || self.context.survey.assigned.contains(&sym) {
            self.unknown_call(span, name);
            return;
        }
        if matches!(name.as_str(), "eval" | "Function") {
            self.push(Certainty::Undecidable, EffectKind::DynamicCode, span, name);
            return;
        }
        let known = match kind {
            CallKind::Construct => PURE_CONSTRUCTORS.contains(&name.as_str()),
            CallKind::Call => PURE_GLOBAL_FUNCTIONS.contains(&name.as_str()),
        };
        if known && !self.context.survey.builtins_changed && !self.context.unseen {
            return;
        }
        self.unknown_call(span, name);
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one ordered classification of a method call: namespaces, call/apply, document methods, built-ins"
    )]
    fn call_method(
        &mut self,
        receiver: &'ast Expression,
        field: &'ast PropertyAccessField,
        arguments: &'ast [Expression],
        span: Span,
    ) {
        let Some(method) = self.field_name(field) else {
            let subject = self.text(span);
            self.unknown_call(span, subject);
            return;
        };
        let builtins_trusted = !self.context.survey.builtins_changed && !self.context.unseen;
        // Built-in namespaces: `Math.max(...)`, `Object.assign(...)`.
        if let Expression::Identifier(namespace) = strip(receiver)
            && self.resolve(namespace.sym()).is_none()
            && !self.context.declared(namespace.sym())
            && !self.context.survey.assigned.contains(&namespace.sym())
            && let Some(class) = namespace_function(&self.context.name(namespace.sym()), &method)
        {
            let subject = format!("{}.{method}", self.context.name(namespace.sym()));
            if !builtins_trusted {
                self.unknown_call(span, subject);
                return;
            }
            match class {
                Namespaced::Pure => self.callbacks(arguments, Origin::Unknown),
                Namespaced::Random => {
                    self.push(Certainty::Certain, EffectKind::Random, span, subject);
                }
                Namespaced::MutatesFirstArgument => {
                    let origin = arguments
                        .first()
                        .map_or(Origin::FreshDeep, |first| self.origin(first));
                    self.write_origin(EffectKind::MutatingCall, origin, span, subject);
                }
                Namespaced::Unknown => self.unknown_call(span, subject),
            }
            return;
        }
        // `f.call(thisArg, ...)` and `f.apply(thisArg, args)` of a function
        // the analysis can see; `Array.prototype.push.call(target, ...)`.
        if matches!(method.as_str(), "call" | "apply") {
            let this = arguments
                .first()
                .map_or(Origin::NonLocal, |first| self.origin(first));
            if let Some(function) = self.resolve_callee(receiver) {
                let origins: Vec<Origin> = if method == "call" {
                    arguments
                        .iter()
                        .skip(1)
                        .map(|argument| self.origin(argument))
                        .collect()
                } else {
                    vec![Origin::Unknown; function.parameters.as_ref().len()]
                };
                let origin = self.callee_origin(receiver);
                self.invoke(function, &origins, this, span, origin);
                return;
            }
            if let Expression::PropertyAccess(PropertyAccess::Simple(inner)) = strip(receiver)
                && let Some(inner_method) = self.field_name(inner.field())
                && builtins_trusted
            {
                if MUTATING_METHODS.contains(&inner_method.as_str()) {
                    self.write_origin(EffectKind::MutatingCall, this, span, inner_method);
                    return;
                }
                if PURE_METHODS.contains(&inner_method.as_str()) {
                    return;
                }
            }
            let subject = self.text(full_span(receiver));
            self.unknown_call(span, subject);
            return;
        }
        if method == "bind" && builtins_trusted {
            return;
        }
        let origin = self.origin(receiver);
        let sym = match field {
            PropertyAccessField::Const(identifier) => Some(identifier.sym()),
            PropertyAccessField::Expr(_) => None,
        };
        // A method the document defines under this name may be the one
        // called.
        if let Some(definitions) = sym.and_then(|sym| self.context.survey.methods.get(&sym)) {
            let mut outcome = Vec::new();
            for definition in definitions.clone() {
                let before = self.events.len();
                let origins: Vec<Origin> = arguments
                    .iter()
                    .map(|argument| self.origin(argument))
                    .collect();
                self.invoke(definition, &origins, origin, span, CalleeOrigin::Document);
                outcome.extend(self.events.drain(before..));
            }
            let builtin_pure = PURE_METHODS.contains(&method.as_str()) && builtins_trusted;
            if outcome.is_empty()
                && (builtin_pure
                    || !PURE_METHODS.contains(&method.as_str())
                        && !MUTATING_METHODS.contains(&method.as_str()))
            {
                if !builtin_pure && !matches!(origin, Origin::FreshDeep | Origin::FreshShallow) {
                    // The receiver may lack the document's method.
                    self.unknown_call(span, method);
                }
                return;
            }
            let cause = outcome.into_iter().find_map(|event| {
                event.cause.or(Some(EffectCause {
                    kind: event.kind,
                    span: event.span,
                    subject: event.subject,
                    function: String::new(),
                }))
            });
            let span = self.span(span);
            self.events.push(Event {
                certainty: Certainty::Undecidable,
                kind: EffectKind::UnknownCall,
                span,
                subject: method,
                cause,
            });
            return;
        }
        if !builtins_trusted {
            self.unknown_call(span, method);
            return;
        }
        if MUTATING_METHODS.contains(&method.as_str()) {
            self.write_origin(EffectKind::MutatingCall, origin, span, method);
            return;
        }
        if REGEXP_STEPPING_METHODS.contains(&method.as_str()) {
            if !matches!(origin, Origin::FreshDeep | Origin::FreshShallow) {
                self.push(
                    Certainty::Undecidable,
                    EffectKind::MutatingCall,
                    span,
                    method,
                );
            }
            return;
        }
        if REGEXP_ARGUMENT_METHODS.contains(&method.as_str())
            && let Some(first) = arguments.first()
            && !matches!(
                strip(first),
                Expression::Literal(_)
                    | Expression::RegExpLiteral(_)
                    | Expression::TemplateLiteral(_)
            )
            && !matches!(self.origin(first), Origin::FreshDeep | Origin::FreshShallow)
        {
            self.push(
                Certainty::Undecidable,
                EffectKind::MutatingCall,
                span,
                method.clone(),
            );
        }
        if PURE_METHODS.contains(&method.as_str())
            || REGEXP_ARGUMENT_METHODS.contains(&method.as_str())
        {
            if CALLBACK_METHODS.contains(&method.as_str()) {
                self.callbacks(arguments, origin.member().join(Origin::Unknown));
            }
            return;
        }
        self.unknown_call(span, method);
    }

    /// Calls the function arguments of a built-in that calls them, each
    /// parameter bound to `element`.
    fn callbacks(&mut self, arguments: &'ast [Expression], element: Origin) {
        for argument in arguments {
            let function = function_of(argument, self.fragment, Sym::EMPTY_STRING)
                .or_else(|| self.resolve_callee(argument));
            if let Some(function) = function {
                let origins = vec![element; function.parameters.as_ref().len().max(1)];
                let origin = self.callee_origin(argument);
                self.invoke(
                    function,
                    &origins,
                    Origin::NonLocal,
                    full_span(argument),
                    origin,
                );
            }
        }
    }

    fn resolve_callee(&self, expression: &'ast Expression) -> Option<Function<'ast>> {
        match strip(expression) {
            Expression::Identifier(identifier) => {
                if self.resolve(identifier.sym()).is_some() {
                    self.local_function(identifier.sym())
                } else {
                    self.context.document_function(identifier.sym())
                }
            }
            other => function_of(other, self.fragment, Sym::EMPTY_STRING),
        }
    }

    fn callee_origin(&self, expression: &'ast Expression) -> CalleeOrigin {
        if matches!(strip(expression), Expression::Identifier(identifier) if self.resolve(identifier.sym()).is_none())
        {
            CalleeOrigin::Document
        } else {
            CalleeOrigin::Walked
        }
    }

    /// Walks `function` called with argument origins `origins` and receiver
    /// `this`. A document function's effects are reported at `span` with
    /// their cause; a function in the walked text reports its own.
    fn invoke(
        &mut self,
        function: Function<'ast>,
        origins: &[Origin],
        this: Origin,
        span: Span,
        origin: CalleeOrigin,
    ) {
        let document = origin == CalleeOrigin::Document;
        if function.generator {
            return;
        }
        let key = function.key();
        if self.active.contains(&key) {
            return;
        }
        let name = self.context.name(function.name);
        if self.active.len() >= MAX_CALL_DEPTH {
            self.push(Certainty::Undecidable, EffectKind::TooComplex, span, name);
            return;
        }
        // A strict function called without a receiver has `this` undefined
        // (ECMA-262 OrdinaryCallBindThis), so a write through it throws.
        let this = if function.arrow {
            self.this_origin()
        } else if function.body.strict() && this == Origin::NonLocal {
            Origin::FreshDeep
        } else {
            this
        };
        let memo_key = (key, origins.to_vec(), this);
        let (inner, reads) = if document && let Some(found) = self.memo.get(&memo_key) {
            found.clone()
        } else {
            let saved_frames = if document {
                std::mem::take(&mut self.frames)
            } else {
                Vec::new()
            };
            let saved_fragment = self.fragment;
            let saved_events = std::mem::take(&mut self.events);
            let saved_reads = std::mem::replace(&mut self.reads_global, false);
            self.fragment = function.fragment;
            self.active.push(key);
            self.push_frame(function, origins, (!function.arrow).then_some(this));
            let _ = self.visit_function_body(function.body);
            self.frames.pop();
            self.active.pop();
            self.fragment = saved_fragment;
            let inner = std::mem::replace(&mut self.events, saved_events);
            let reads = std::mem::replace(&mut self.reads_global, saved_reads);
            if document {
                self.frames = saved_frames;
                if !self.exhausted {
                    self.memo.insert(memo_key, (inner.clone(), reads));
                }
            }
            (inner, reads)
        };
        self.reads_global |= reads;
        if !document {
            self.events.extend(inner);
            return;
        }
        let at = self.span(span);
        for event in inner {
            let cause = event.cause.unwrap_or(EffectCause {
                kind: event.kind,
                span: event.span,
                subject: event.subject,
                function: name.clone(),
            });
            self.events.push(Event {
                certainty: event.certainty,
                kind: EffectKind::FunctionCall,
                span: at,
                subject: name.clone(),
                cause: Some(cause),
            });
        }
    }

    /// A function the checked expression creates (a function or arrow
    /// literal, or an object literal's method, getter, setter, or
    /// conversion method) runs only when something calls it: a built-in
    /// that takes it as a callback, a property read or a conversion the
    /// expression triggers, a generator it drains. When the walk found
    /// nothing, the first such function whose body has an effect is
    /// reported as undecidable at the function, with that effect as its
    /// cause, so the expression is never silently clean.
    fn created_functions(&mut self, expression: &'ast Expression) {
        let mut created = Created::default();
        let _ = created.visit_expression(expression);
        let mut candidates: Vec<(Function<'ast>, Span, String)> = created
            .found
            .iter()
            .filter_map(|literal| {
                let function = function_of(literal, self.fragment, Sym::EMPTY_STRING)?;
                let span = full_span(literal);
                Some((function, span, self.text(span)))
            })
            .collect();
        let whole = full_span(expression);
        let fragment = self.fragment;
        let survey = &self.context.survey;
        for function in survey
            .implicit
            .iter()
            .chain(survey.methods.values().flatten())
            .filter(|function| function.fragment == fragment)
        {
            candidates.push((*function, whole, self.context.name(function.name)));
        }
        for (function, span, subject) in candidates {
            let before = self.events.len();
            let mut plain = function;
            plain.generator = false;
            let origins = vec![Origin::Unknown; function.parameters.as_ref().len().max(1)];
            self.invoke(
                plain,
                &origins,
                Origin::Unknown,
                span,
                CalleeOrigin::Document,
            );
            let mut found = self.events.drain(before..);
            if let Some(event) = found.next() {
                drop(found);
                self.events.push(Event {
                    certainty: Certainty::Undecidable,
                    kind: EffectKind::FunctionCall,
                    span: event.span,
                    subject: bounded(subject),
                    cause: event.cause,
                });
                return;
            }
        }
    }

    /// Pushes the scope of `function` called with `origins`.
    fn push_frame(&mut self, function: Function<'ast>, origins: &[Origin], this: Option<Origin>) {
        let mut names = BTreeMap::new();
        let mut functions = BTreeMap::new();
        let mut parameters = BTreeSet::new();
        for (index, parameter) in function.parameters.as_ref().iter().enumerate() {
            let origin = if parameter.is_rest_param() {
                Origin::FreshShallow
            } else {
                origins.get(index).copied().unwrap_or(Origin::FreshDeep)
            };
            match parameter.variable().binding() {
                Binding::Identifier(identifier) => {
                    names.insert(identifier.sym(), origin);
                    parameters.insert(identifier.sym());
                }
                Binding::Pattern(pattern) => {
                    for name in boa_engine::ast::operations::bound_names(pattern) {
                        names.insert(name, origin.member());
                        parameters.insert(name);
                    }
                }
            }
        }
        if !function.arrow {
            names.insert(Sym::ARGUMENTS, Origin::FreshShallow);
        }
        let mut declared = Declared::default();
        let _ = declared.visit_function_body(function.body);
        // Assignments a function binding may take besides its own: none
        // for a declaration, the initializer for `var g = function`.
        let mut allowed = BTreeMap::new();
        for (name, init, form) in declared.functions {
            *allowed.entry(name).or_insert(0_usize) +=
                usize::from(form == BindingForm::Initializer);
            functions.insert(
                name,
                Function {
                    fragment: function.fragment,
                    ..init
                },
            );
            names.insert(name, Origin::FreshDeep);
        }
        for name in declared.names {
            names.entry(name).or_insert(Origin::FreshDeep);
        }
        let mut frame = Frame {
            names,
            functions: BTreeMap::new(),
            this,
        };
        // Flow-insensitive origins of the local variables: the join of
        // every value assigned to them in the body.
        let mut assignments = Assignments {
            targets: frame.names.keys().copied().collect(),
            shadowed: Vec::new(),
            found: Vec::new(),
            nested: 0,
        };
        let _ = assignments.visit_function_body(function.body);
        for (name, allowed) in allowed {
            let count = assignments
                .found
                .iter()
                .filter(|(found, _)| *found == name)
                .count();
            if count > allowed {
                functions.remove(&name);
            }
        }
        frame.functions = functions;
        // The origins are computed with the frame in scope, so a value built
        // from other locals takes their origins.
        self.frames.push(frame);
        for _ in 0..8 {
            let mut changed = false;
            for (name, value) in &assignments.found {
                let origin = match value {
                    Assigned::Value(expression) => self.origin(expression),
                    Assigned::Unknown | Assigned::Nested => Origin::Unknown,
                    Assigned::Primitive => Origin::FreshDeep,
                };
                if let Some(current) = self
                    .frames
                    .last_mut()
                    .and_then(|frame| frame.names.get_mut(name))
                {
                    let joined = current.join(origin);
                    if joined != *current {
                        *current = joined;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }
}

/// The function literals of an expression, outermost only (a literal's
/// body is walked when the literal is).
#[derive(Default)]
struct Created<'ast> {
    found: Vec<&'ast Expression>,
}

impl<'ast> Visitor<'ast> for Created<'ast> {
    type BreakTy = std::convert::Infallible;

    fn visit_expression(&mut self, node: &'ast Expression) -> Flow {
        if function_of(node, 0, Sym::EMPTY_STRING).is_some() {
            self.found.push(node);
            return GO;
        }
        node.visit_with(self)
    }
}

/// Names declared in a function body outside nested functions.
#[derive(Default)]
struct Declared<'ast> {
    names: BTreeSet<Sym>,
    /// Function bindings: name, function, and how it is declared.
    functions: Vec<(Sym, Function<'ast>, BindingForm)>,
}

impl<'ast> Visitor<'ast> for Declared<'ast> {
    type BreakTy = std::convert::Infallible;

    fn visit_variable(&mut self, node: &'ast Variable) -> Flow {
        match node.binding() {
            Binding::Identifier(identifier) => {
                self.names.insert(identifier.sym());
                if let Some(function) = node
                    .init()
                    .and_then(|init| function_of(init, 0, identifier.sym()))
                {
                    self.functions
                        .push((identifier.sym(), function, BindingForm::Initializer));
                }
            }
            Binding::Pattern(pattern) => {
                self.names
                    .extend(boa_engine::ast::operations::bound_names(pattern));
            }
        }
        GO
    }

    fn visit_binding(&mut self, node: &'ast Binding) -> Flow {
        match node {
            Binding::Identifier(identifier) => {
                self.names.insert(identifier.sym());
            }
            Binding::Pattern(pattern) => {
                self.names
                    .extend(boa_engine::ast::operations::bound_names(pattern));
            }
        }
        GO
    }

    fn visit_expression(&mut self, _node: &'ast Expression) -> Flow {
        GO
    }

    fn visit_function_declaration(
        &mut self,
        node: &'ast boa_engine::ast::function::FunctionDeclaration,
    ) -> Flow {
        self.names.insert(node.name().sym());
        self.functions.push((
            node.name().sym(),
            Function {
                parameters: node.parameters(),
                body: node.body(),
                arrow: false,
                generator: false,
                fragment: 0,
                name: node.name().sym(),
            },
            BindingForm::Declaration,
        ));
        GO
    }

    fn visit_async_function_declaration(
        &mut self,
        node: &'ast boa_engine::ast::function::AsyncFunctionDeclaration,
    ) -> Flow {
        self.names.insert(node.name().sym());
        GO
    }

    fn visit_generator_declaration(
        &mut self,
        node: &'ast boa_engine::ast::function::GeneratorDeclaration,
    ) -> Flow {
        self.names.insert(node.name().sym());
        GO
    }

    fn visit_async_generator_declaration(
        &mut self,
        node: &'ast boa_engine::ast::function::AsyncGeneratorDeclaration,
    ) -> Flow {
        self.names.insert(node.name().sym());
        GO
    }

    fn visit_class_declaration(
        &mut self,
        node: &'ast boa_engine::ast::function::ClassDeclaration,
    ) -> Flow {
        self.names.insert(node.name().sym());
        GO
    }
}

/// One value assigned to a tracked local.
enum Assigned<'ast> {
    Value(&'ast Expression),
    /// A value from a pattern, loop head, or `catch`.
    Unknown,
    /// Assigned inside a nested function.
    Nested,
    /// A compound assignment's primitive result.
    Primitive,
}

/// Every assignment to `targets` in a function body, nested functions
/// included (where a name they redeclare is skipped).
struct Assignments<'ast> {
    targets: BTreeSet<Sym>,
    shadowed: Vec<BTreeSet<Sym>>,
    found: Vec<(Sym, Assigned<'ast>)>,
    nested: usize,
}

impl<'ast> Assignments<'ast> {
    fn tracked(&self, sym: Sym) -> bool {
        self.targets.contains(&sym) && !self.shadowed.iter().any(|set| set.contains(&sym))
    }

    fn record(&mut self, sym: Sym, value: Assigned<'ast>) {
        if self.tracked(sym) {
            let value = if self.nested > 0 {
                Assigned::Nested
            } else {
                value
            };
            self.found.push((sym, value));
        }
    }

    fn nested_function(
        &mut self,
        parameters: &'ast FormalParameterList,
        body: &'ast FunctionBody,
    ) -> Flow {
        let mut declared = Declared::default();
        let _ = declared.visit_function_body(body);
        let mut shadow = declared.names;
        shadow.extend(boa_engine::ast::operations::bound_names(parameters));
        shadow.insert(Sym::ARGUMENTS);
        self.shadowed.push(shadow);
        self.nested += 1;
        let _ = self.visit_function_body(body);
        self.nested -= 1;
        self.shadowed.pop();
        GO
    }

    fn pattern(&mut self, pattern: &'ast Pattern) {
        for name in boa_engine::ast::operations::bound_names(pattern) {
            self.record(name, Assigned::Unknown);
        }
    }
}

impl<'ast> Visitor<'ast> for Assignments<'ast> {
    type BreakTy = std::convert::Infallible;

    fn visit_variable(&mut self, node: &'ast Variable) -> Flow {
        match node.binding() {
            Binding::Identifier(identifier) => {
                if let Some(init) = node.init() {
                    self.record(identifier.sym(), Assigned::Value(init));
                }
            }
            Binding::Pattern(pattern) => self.pattern(pattern),
        }
        node.visit_with(self)
    }

    fn visit_assign(&mut self, node: &'ast boa_engine::ast::expression::operator::Assign) -> Flow {
        match node.lhs() {
            AssignTarget::Identifier(identifier) => {
                let value = match node.op() {
                    AssignOp::Assign
                    | AssignOp::BoolAnd
                    | AssignOp::BoolOr
                    | AssignOp::Coalesce => Assigned::Value(node.rhs()),
                    _ => Assigned::Primitive,
                };
                self.record(identifier.sym(), value);
            }
            AssignTarget::Pattern(pattern) => self.pattern(pattern),
            AssignTarget::Access(_) => {}
        }
        node.visit_with(self)
    }

    fn visit_iterable_loop_initializer(&mut self, node: &'ast IterableLoopInitializer) -> Flow {
        match node {
            IterableLoopInitializer::Identifier(identifier) => {
                self.record(identifier.sym(), Assigned::Unknown);
            }
            IterableLoopInitializer::Var(variable) => match variable.binding() {
                Binding::Identifier(identifier) => self.record(identifier.sym(), Assigned::Unknown),
                Binding::Pattern(pattern) => self.pattern(pattern),
            },
            IterableLoopInitializer::Let(binding) | IterableLoopInitializer::Const(binding) => {
                match binding {
                    Binding::Identifier(identifier) => {
                        self.record(identifier.sym(), Assigned::Unknown);
                    }
                    Binding::Pattern(pattern) => self.pattern(pattern),
                }
            }
            IterableLoopInitializer::Pattern(pattern) => self.pattern(pattern),
            IterableLoopInitializer::Access(_) => {}
        }
        node.visit_with(self)
    }

    fn visit_catch(&mut self, node: &'ast boa_engine::ast::statement::Catch) -> Flow {
        if let Some(binding) = node.parameter() {
            match binding {
                Binding::Identifier(identifier) => self.record(identifier.sym(), Assigned::Unknown),
                Binding::Pattern(pattern) => self.pattern(pattern),
            }
        }
        node.visit_with(self)
    }

    fn visit_function_expression(
        &mut self,
        node: &'ast boa_engine::ast::function::FunctionExpression,
    ) -> Flow {
        self.nested_function(node.parameters(), node.body())
    }

    fn visit_function_declaration(
        &mut self,
        node: &'ast boa_engine::ast::function::FunctionDeclaration,
    ) -> Flow {
        self.nested_function(node.parameters(), node.body())
    }

    fn visit_arrow_function(
        &mut self,
        node: &'ast boa_engine::ast::function::ArrowFunction,
    ) -> Flow {
        self.nested_function(node.parameters(), node.body())
    }

    fn visit_async_arrow_function(
        &mut self,
        node: &'ast boa_engine::ast::function::AsyncArrowFunction,
    ) -> Flow {
        self.nested_function(node.parameters(), node.body())
    }

    fn visit_async_function_expression(
        &mut self,
        node: &'ast boa_engine::ast::function::AsyncFunctionExpression,
    ) -> Flow {
        self.nested_function(node.parameters(), node.body())
    }

    fn visit_async_function_declaration(
        &mut self,
        node: &'ast boa_engine::ast::function::AsyncFunctionDeclaration,
    ) -> Flow {
        self.nested_function(node.parameters(), node.body())
    }

    fn visit_generator_expression(
        &mut self,
        node: &'ast boa_engine::ast::function::GeneratorExpression,
    ) -> Flow {
        self.nested_function(node.parameters(), node.body())
    }

    fn visit_generator_declaration(
        &mut self,
        node: &'ast boa_engine::ast::function::GeneratorDeclaration,
    ) -> Flow {
        self.nested_function(node.parameters(), node.body())
    }

    fn visit_object_method_definition(&mut self, node: &'ast ObjectMethodDefinition) -> Flow {
        self.nested_function(node.parameters(), node.body())
    }
}

impl<'ast> Visitor<'ast> for Walker<'_, '_, 'ast> {
    type BreakTy = std::convert::Infallible;

    #[allow(
        clippy::too_many_lines,
        reason = "one dispatch over the expression kinds the analysis treats specially"
    )]
    fn visit_expression(&mut self, node: &'ast Expression) -> Flow {
        if !self.step() {
            return GO;
        }
        match node {
            Expression::Identifier(identifier) => {
                if self.resolve(identifier.sym()).is_none() {
                    self.reads_global = true;
                }
                GO
            }
            Expression::This(_) => {
                if self.this_origin() == Origin::NonLocal {
                    self.reads_global = true;
                }
                GO
            }
            // A function or class is only created here; its body runs when
            // it is called.
            Expression::FunctionExpression(_)
            | Expression::ArrowFunction(_)
            | Expression::AsyncArrowFunction(_)
            | Expression::GeneratorExpression(_)
            | Expression::AsyncFunctionExpression(_)
            | Expression::AsyncGeneratorExpression(_) => GO,
            Expression::ClassExpression(class) => {
                if let Some(heritage) = class.super_ref() {
                    let _ = self.visit_expression(heritage);
                }
                self.class_statics(class.elements());
                GO
            }
            Expression::Assign(assign) => {
                match assign.lhs() {
                    AssignTarget::Identifier(identifier) => {
                        self.write_identifier(EffectKind::Assignment, identifier);
                    }
                    AssignTarget::Access(access) => {
                        self.visit_access_operands(access);
                        self.write_access(EffectKind::Assignment, access);
                    }
                    AssignTarget::Pattern(pattern) => {
                        let _ = self.visit_pattern(pattern);
                        self.write_pattern(pattern);
                    }
                }
                self.visit_expression(assign.rhs())
            }
            Expression::Update(update) => {
                match update.target() {
                    UpdateTarget::Identifier(identifier) => {
                        self.write_identifier(EffectKind::Update, identifier);
                    }
                    UpdateTarget::PropertyAccess(access) => {
                        self.visit_access_operands(access);
                        self.write_access(EffectKind::Update, access);
                    }
                }
                GO
            }
            Expression::Unary(unary) if unary.op() == UnaryOp::Delete => {
                match strip(unary.target()) {
                    Expression::Identifier(identifier) => {
                        self.write_identifier(EffectKind::Deletion, identifier);
                    }
                    Expression::PropertyAccess(access) => {
                        self.visit_access_operands(access);
                        self.write_access(EffectKind::Deletion, access);
                    }
                    other => {
                        let _ = self.visit_expression(other);
                    }
                }
                GO
            }
            Expression::Call(call) => {
                let span = full_span(node);
                self.call(call.function(), call.args(), span, CallKind::Call);
                GO
            }
            Expression::New(new) => {
                self.call(
                    new.constructor(),
                    new.arguments(),
                    full_span(node),
                    CallKind::Construct,
                );
                GO
            }
            Expression::TaggedTemplate(tagged) => {
                for expression in tagged.exprs() {
                    let _ = self.visit_expression(expression);
                }
                self.call(tagged.tag(), &[], full_span(node), CallKind::Call);
                GO
            }
            Expression::Optional(optional) => {
                let _ = self.visit_expression(optional.target());
                for operation in optional.chain() {
                    if let boa_engine::ast::expression::OptionalOperationKind::Call { args } =
                        operation.kind()
                    {
                        self.visit_arguments(args);
                        let subject = self.text(full_span(optional.target()));
                        self.unknown_call(full_span(node), subject);
                    } else {
                        let _ = self.visit_optional_operation(operation);
                    }
                }
                GO
            }
            Expression::ImportCall(import) => {
                self.push(
                    Certainty::Undecidable,
                    EffectKind::DynamicCode,
                    import.span(),
                    "import".to_owned(),
                );
                GO
            }
            _ => node.visit_with(self),
        }
    }

    fn visit_property_access_field(&mut self, node: &'ast PropertyAccessField) -> Flow {
        match node {
            PropertyAccessField::Const(_) => GO,
            PropertyAccessField::Expr(expression) => self.visit_expression(expression),
        }
    }

    fn visit_property_definition(&mut self, node: &'ast PropertyDefinition) -> Flow {
        match node {
            PropertyDefinition::MethodDefinition(method) => match method.name() {
                PropertyName::Computed(expression) => self.visit_expression(expression),
                PropertyName::Literal(_) => GO,
            },
            PropertyDefinition::IdentifierReference(identifier) => {
                if self.resolve(identifier.sym()).is_none() {
                    self.reads_global = true;
                }
                GO
            }
            _ => node.visit_with(self),
        }
    }

    fn visit_statement_list_item(&mut self, node: &'ast StatementListItem) -> Flow {
        if !self.step() {
            return GO;
        }
        match node {
            StatementListItem::Declaration(declaration) => match declaration.as_ref() {
                Declaration::Lexical(lexical) => self.visit_lexical_declaration(lexical),
                Declaration::ClassDeclaration(class) => {
                    if let Some(heritage) = class.super_ref() {
                        let _ = self.visit_expression(heritage);
                    }
                    self.class_statics(class.elements());
                    GO
                }
                _ => GO,
            },
            StatementListItem::Statement(statement) => self.visit_statement(statement),
        }
    }

    fn visit_statement(&mut self, node: &'ast Statement) -> Flow {
        if !self.step() {
            return GO;
        }
        if let Statement::With(with) = node {
            self.push(
                Certainty::Undecidable,
                EffectKind::DynamicScope,
                full_span(with.expression()),
                "with".to_owned(),
            );
        }
        node.visit_with(self)
    }

    fn visit_iterable_loop_initializer(&mut self, node: &'ast IterableLoopInitializer) -> Flow {
        match node {
            IterableLoopInitializer::Identifier(identifier) => {
                self.write_identifier(EffectKind::Assignment, identifier);
            }
            IterableLoopInitializer::Access(access) => {
                self.visit_access_operands(access);
                self.write_access(EffectKind::Assignment, access);
            }
            IterableLoopInitializer::Pattern(pattern) => self.write_pattern(pattern),
            _ => {}
        }
        node.visit_with(self)
    }

    fn visit_for_loop_initializer(&mut self, node: &'ast ForLoopInitializer) -> Flow {
        node.visit_with(self)
    }

    fn visit_binding(&mut self, node: &'ast Binding) -> Flow {
        match node {
            Binding::Identifier(_) => GO,
            Binding::Pattern(pattern) => self.visit_pattern(pattern),
        }
    }

    fn visit_pattern(&mut self, node: &'ast Pattern) -> Flow {
        // Only the default initializers and computed keys of a pattern run.
        match node {
            Pattern::Object(object) => {
                for element in object.bindings() {
                    match element {
                        ObjectPatternElement::SingleName {
                            name, default_init, ..
                        }
                        | ObjectPatternElement::AssignmentPropertyAccess {
                            name,
                            default_init,
                            ..
                        } => {
                            if let PropertyName::Computed(key) = name {
                                let _ = self.visit_expression(key);
                            }
                            if let Some(init) = default_init {
                                let _ = self.visit_expression(init);
                            }
                        }
                        ObjectPatternElement::Pattern {
                            name,
                            pattern,
                            default_init,
                        } => {
                            if let PropertyName::Computed(key) = name {
                                let _ = self.visit_expression(key);
                            }
                            let _ = self.visit_pattern(pattern);
                            if let Some(init) = default_init {
                                let _ = self.visit_expression(init);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Pattern::Array(array) => {
                for element in array.bindings() {
                    match element {
                        ArrayPatternElement::SingleName { default_init, .. }
                        | ArrayPatternElement::PropertyAccess { default_init, .. } => {
                            if let Some(init) = default_init {
                                let _ = self.visit_expression(init);
                            }
                        }
                        ArrayPatternElement::Pattern {
                            pattern,
                            default_init,
                        } => {
                            let _ = self.visit_pattern(pattern);
                            if let Some(init) = default_init {
                                let _ = self.visit_expression(init);
                            }
                        }
                        ArrayPatternElement::PatternRest { pattern } => {
                            let _ = self.visit_pattern(pattern);
                        }
                        _ => {}
                    }
                }
            }
        }
        GO
    }
}

impl<'ast> Walker<'_, '_, 'ast> {
    /// Walks what a property access evaluates before its write: the object
    /// and a computed key.
    fn visit_access_operands(&mut self, access: &'ast PropertyAccess) {
        match access {
            PropertyAccess::Simple(simple) => {
                let _ = self.visit_expression(simple.target());
                let _ = self.visit_property_access_field(simple.field());
            }
            PropertyAccess::Private(private) => {
                let _ = self.visit_expression(private.target());
            }
            PropertyAccess::Super(sup) => {
                let _ = self.visit_property_access_field(sup.field());
            }
        }
    }

    /// Static field initializers and static blocks run when a class is
    /// defined; `this` there is the fresh class.
    fn class_statics(&mut self, elements: &'ast [ClassElement]) {
        for element in elements {
            match element {
                ClassElement::StaticFieldDefinition(field) => {
                    if let Some(init) = field.initializer() {
                        self.frames.push(Frame {
                            names: BTreeMap::new(),
                            functions: BTreeMap::new(),
                            this: Some(Origin::FreshDeep),
                        });
                        let _ = self.visit_expression(init);
                        self.frames.pop();
                    }
                }
                ClassElement::StaticBlock(block) => {
                    self.frames.push(Frame {
                        names: BTreeMap::new(),
                        functions: BTreeMap::new(),
                        this: Some(Origin::FreshDeep),
                    });
                    let _ = self.visit_function_body(block.statements());
                    self.frames.pop();
                }
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

fn analyze(
    fragments: &[EffectFragment<'_>],
    external_scripts: ExternalScripts,
) -> Vec<EffectFinding> {
    let mut interner = Interner::default();
    let parsed: Vec<Option<Parsed>> = fragments
        .iter()
        .map(|fragment| parse(fragment, &mut interner))
        .collect();
    let unseen = external_scripts == ExternalScripts::Present
        || fragments.iter().zip(&parsed).any(|(fragment, parsed)| {
            fragment.role == EffectRole::Script
                && parsed.is_none()
                && !nesting::is_blank(fragment.source)
        });
    let survey = survey(&parsed, &mut interner);
    let mut context = Context {
        parsed: &parsed,
        interner: &interner,
        survey: &survey,
        unseen,
        implicit_cause: None,
    };
    let mut memo = BTreeMap::new();
    context.implicit_cause = implicit_cause(&context, &mut memo);
    let mut findings = Vec::new();
    for (index, parsed) in parsed.iter().enumerate() {
        let Some(parsed) = parsed.as_ref().filter(|parsed| parsed.role_checked) else {
            continue;
        };
        if let Some(expression) = parsed.expression() {
            findings.extend(check_fragment(
                &context, &mut memo, parsed, index, expression,
            ));
        }
    }
    for (index, parsed) in parsed.iter().enumerate() {
        let Some(parsed) = parsed else {
            continue;
        };
        let mut writes = SystemWrites {
            interner: &interner,
            parsed,
            fragment: index,
            locals: Vec::new(),
            functions: 0,
            found: Vec::new(),
        };
        let _ = writes.visit_script(&parsed.ast);
        writes.found.sort();
        writes.found.dedup();
        findings.extend(writes.found);
    }
    findings.sort();
    // At most MAX_FINDINGS_PER_FRAGMENT per fragment, in sorted order.
    let mut kept: BTreeMap<usize, usize> = BTreeMap::new();
    findings.retain(|finding| {
        let count = kept.entry(finding.span.fragment).or_default();
        *count += 1;
        *count <= MAX_FINDINGS_PER_FRAGMENT
    });
    findings
}

/// The SCXML 1.0 §5.10 system variables of the ECMAScript data model, and
/// `_x`, the root 5.10 reserves for platform-specific ones.
const SYSTEM_VARIABLES: &[&str] = &["_event", "_sessionid", "_name", "_ioprocessors", "_x"];

/// Every assignment, compound assignment, update, or `delete` of a system
/// variable itself in one fragment (5.10: "The Processor MUST cause any
/// attempt to change the value of a system variable to fail"), outside a
/// scope that declares the name as its own: a function's variables and
/// parameters, and the `let`/`const`/`class` bindings of a block, a `for`
/// head, or a `switch`, and a `catch` parameter. The static forms that
/// write one through the global object count too: `globalThis._event = 1`,
/// `delete globalThis['_event']`, `this._name++` outside any function, and
/// `Reflect.deleteProperty`, `Reflect.defineProperty`, `Reflect.set`, or
/// `Object.defineProperty` with the global object and a string literal.
struct SystemWrites<'p, 'i> {
    interner: &'i Interner,
    parsed: &'p Parsed,
    fragment: usize,
    locals: Vec<BTreeSet<Sym>>,
    /// Enclosing non-arrow functions; `this` is the global object at 0.
    functions: usize,
    found: Vec<EffectFinding>,
}

impl SystemWrites<'_, '_> {
    fn name(&self, sym: Sym) -> String {
        self.interner
            .resolve(sym)
            .map(|name| name.to_string())
            .unwrap_or_default()
    }

    fn local(&self, sym: Sym) -> bool {
        self.locals.iter().any(|locals| locals.contains(&sym))
    }

    fn write(&mut self, identifier: &Identifier) {
        let sym = identifier.sym();
        if self.local(sym) {
            return;
        }
        let name = self.name(sym);
        self.record(name, identifier.span());
    }

    /// Whether `expression` is certainly the global object: `globalThis`
    /// not shadowed, or `this` outside every non-arrow function.
    fn global_object(&self, expression: &Expression) -> bool {
        match strip(expression) {
            Expression::Identifier(identifier) => {
                !self.local(identifier.sym()) && self.name(identifier.sym()) == "globalThis"
            }
            Expression::This(_) => self.functions == 0,
            _ => false,
        }
    }

    /// The name a string literal or property field spells, with its span.
    fn literal_name(&self, expression: &Expression) -> Option<(String, Span)> {
        match strip(expression) {
            Expression::Literal(literal) => match literal.kind() {
                LiteralKind::String(sym) => Some((self.name(*sym), literal.span())),
                _ => None,
            },
            _ => None,
        }
    }

    /// A write of `access` when it is a system variable of the global
    /// object (`globalThis._event`, `this['_name']`).
    fn member_write(&mut self, access: &PropertyAccess) {
        let PropertyAccess::Simple(simple) = access else {
            return;
        };
        if !self.global_object(simple.target()) {
            return;
        }
        let named = match simple.field() {
            PropertyAccessField::Const(identifier) => {
                Some((self.name(identifier.sym()), identifier.span()))
            }
            PropertyAccessField::Expr(expression) => self.literal_name(expression),
        };
        if let Some((name, span)) = named {
            self.record(name, span);
        }
    }

    /// `Reflect.deleteProperty(globalThis, '_event')` and the other static
    /// reflective writers of a global property.
    fn reflective_write(&mut self, call: &boa_engine::ast::expression::Call) {
        let Expression::PropertyAccess(PropertyAccess::Simple(callee)) = strip(call.function())
        else {
            return;
        };
        let (Expression::Identifier(object), PropertyAccessField::Const(method)) =
            (strip(callee.target()), callee.field())
        else {
            return;
        };
        if self.local(object.sym()) {
            return;
        }
        let writer = matches!(
            (
                self.name(object.sym()).as_str(),
                self.name(method.sym()).as_str()
            ),
            ("Reflect", "deleteProperty" | "defineProperty" | "set") | ("Object", "defineProperty")
        );
        if !writer {
            return;
        }
        if let [target, key, ..] = call.args()
            && self.global_object(target)
            && let Some((name, span)) = self.literal_name(key)
        {
            self.record(name, span);
        }
    }

    fn record(&mut self, name: String, span: Span) {
        if !SYSTEM_VARIABLES.contains(&name.as_str()) {
            return;
        }
        let (start, end) = self.parsed.range(span);
        self.found.push(EffectFinding {
            certainty: Certainty::Certain,
            kind: EffectKind::SystemVariableWrite,
            span: EffectSpan {
                fragment: self.fragment,
                start,
                end,
            },
            subject: name,
            cause: None,
            operator: None,
        });
    }

    fn pattern(&mut self, pattern: &Pattern) {
        for name in boa_engine::ast::operations::bound_names(pattern) {
            if let Some(identifier) = pattern_identifier(pattern, name) {
                self.write(&identifier);
            }
        }
    }

    fn scoped(
        &mut self,
        parameters: &FormalParameterList,
        body: &FunctionBody,
        form: FunctionForm,
    ) {
        let mut declared = Declared::default();
        let _ = declared.visit_function_body(body);
        let mut locals = declared.names;
        locals.extend(boa_engine::ast::operations::bound_names(parameters));
        if form == FunctionForm::Ordinary {
            locals.insert(Sym::ARGUMENTS);
            self.functions += 1;
        }
        self.locals.push(locals);
    }

    fn unscoped(&mut self, form: FunctionForm) {
        self.locals.pop();
        if form == FunctionForm::Ordinary {
            self.functions = self.functions.saturating_sub(1);
        }
    }

    /// Visits `node` with `names` bound as its own (a block scope).
    fn within<N: VisitWith>(&mut self, names: Vec<Sym>, node: &N) -> Flow {
        self.locals.push(names.into_iter().collect());
        let flow = node.visit_with(self);
        self.locals.pop();
        flow
    }
}

/// The identifier `name` binds in `pattern`, with its span.
fn pattern_identifier(pattern: &Pattern, name: Sym) -> Option<Identifier> {
    struct Find {
        name: Sym,
        found: Option<Identifier>,
    }
    impl<'ast> Visitor<'ast> for Find {
        type BreakTy = ();
        fn visit_identifier(&mut self, node: &'ast Identifier) -> std::ops::ControlFlow<()> {
            if node.sym() == self.name {
                self.found = Some(*node);
                return std::ops::ControlFlow::Break(());
            }
            std::ops::ControlFlow::Continue(())
        }
    }
    let mut find = Find { name, found: None };
    let _ = find.visit_pattern(pattern);
    find.found
}

macro_rules! scoped_visit {
    ($method:ident, $node:ty, $arrow:expr) => {
        fn $method(&mut self, node: &'ast $node) -> Flow {
            self.scoped(node.parameters(), node.body(), $arrow);
            let flow = node.visit_with(self);
            self.unscoped($arrow);
            flow
        }
    };
}

impl<'ast> Visitor<'ast> for SystemWrites<'_, '_> {
    type BreakTy = std::convert::Infallible;

    scoped_visit!(
        visit_function_expression,
        boa_engine::ast::function::FunctionExpression,
        FunctionForm::Ordinary
    );
    scoped_visit!(
        visit_function_declaration,
        boa_engine::ast::function::FunctionDeclaration,
        FunctionForm::Ordinary
    );
    scoped_visit!(
        visit_arrow_function,
        boa_engine::ast::function::ArrowFunction,
        FunctionForm::Arrow
    );
    scoped_visit!(
        visit_async_arrow_function,
        boa_engine::ast::function::AsyncArrowFunction,
        FunctionForm::Arrow
    );
    scoped_visit!(
        visit_async_function_expression,
        boa_engine::ast::function::AsyncFunctionExpression,
        FunctionForm::Ordinary
    );
    scoped_visit!(
        visit_async_function_declaration,
        boa_engine::ast::function::AsyncFunctionDeclaration,
        FunctionForm::Ordinary
    );
    scoped_visit!(
        visit_generator_expression,
        boa_engine::ast::function::GeneratorExpression,
        FunctionForm::Ordinary
    );
    scoped_visit!(
        visit_generator_declaration,
        boa_engine::ast::function::GeneratorDeclaration,
        FunctionForm::Ordinary
    );
    scoped_visit!(
        visit_async_generator_expression,
        boa_engine::ast::function::AsyncGeneratorExpression,
        FunctionForm::Ordinary
    );
    scoped_visit!(
        visit_async_generator_declaration,
        boa_engine::ast::function::AsyncGeneratorDeclaration,
        FunctionForm::Ordinary
    );
    scoped_visit!(
        visit_object_method_definition,
        ObjectMethodDefinition,
        FunctionForm::Ordinary
    );

    fn visit_class_element(&mut self, node: &'ast ClassElement) -> Flow {
        if let ClassElement::MethodDefinition(method) = node {
            self.scoped(method.parameters(), method.body(), FunctionForm::Ordinary);
            let flow = node.visit_with(self);
            self.unscoped(FunctionForm::Ordinary);
            return flow;
        }
        node.visit_with(self)
    }

    fn visit_block(&mut self, node: &'ast boa_engine::ast::statement::Block) -> Flow {
        self.within(
            boa_engine::ast::operations::lexically_declared_names(node),
            node,
        )
    }

    fn visit_switch(&mut self, node: &'ast boa_engine::ast::statement::Switch) -> Flow {
        self.within(
            boa_engine::ast::operations::lexically_declared_names(node),
            node,
        )
    }

    fn visit_catch(&mut self, node: &'ast boa_engine::ast::statement::Catch) -> Flow {
        let names = node
            .parameter()
            .map(boa_engine::ast::operations::bound_names)
            .unwrap_or_default();
        self.within(names, node)
    }

    fn visit_for_loop(
        &mut self,
        node: &'ast boa_engine::ast::statement::iteration::ForLoop,
    ) -> Flow {
        let names = match node.init() {
            Some(ForLoopInitializer::Lexical(lexical)) => {
                boa_engine::ast::operations::bound_names(lexical.declaration())
            }
            _ => Vec::new(),
        };
        self.within(names, node)
    }

    fn visit_for_in_loop(
        &mut self,
        node: &'ast boa_engine::ast::statement::iteration::ForInLoop,
    ) -> Flow {
        self.within(loop_bindings(node.initializer()), node)
    }

    fn visit_for_of_loop(
        &mut self,
        node: &'ast boa_engine::ast::statement::iteration::ForOfLoop,
    ) -> Flow {
        self.within(loop_bindings(node.initializer()), node)
    }

    fn visit_assign(&mut self, node: &'ast boa_engine::ast::expression::operator::Assign) -> Flow {
        match node.lhs() {
            AssignTarget::Identifier(identifier) => self.write(identifier),
            AssignTarget::Pattern(pattern) => self.pattern(pattern),
            AssignTarget::Access(access) => self.member_write(access),
        }
        node.visit_with(self)
    }

    fn visit_update(&mut self, node: &'ast boa_engine::ast::expression::operator::Update) -> Flow {
        match node.target() {
            UpdateTarget::Identifier(identifier) => self.write(identifier),
            UpdateTarget::PropertyAccess(access) => self.member_write(access),
        }
        node.visit_with(self)
    }

    fn visit_unary(&mut self, node: &'ast boa_engine::ast::expression::operator::Unary) -> Flow {
        if node.op() == UnaryOp::Delete {
            match strip(node.target()) {
                Expression::Identifier(identifier) => self.write(identifier),
                Expression::PropertyAccess(access) => self.member_write(access),
                _ => {}
            }
        }
        node.visit_with(self)
    }

    fn visit_call(&mut self, node: &'ast boa_engine::ast::expression::Call) -> Flow {
        self.reflective_write(node);
        node.visit_with(self)
    }

    fn visit_iterable_loop_initializer(&mut self, node: &'ast IterableLoopInitializer) -> Flow {
        match node {
            IterableLoopInitializer::Identifier(identifier) => self.write(identifier),
            IterableLoopInitializer::Pattern(pattern) => self.pattern(pattern),
            _ => {}
        }
        node.visit_with(self)
    }
}

/// The `let` or `const` bindings a `for-in`/`for-of` head declares.
fn loop_bindings(initializer: &IterableLoopInitializer) -> Vec<Sym> {
    match initializer {
        IterableLoopInitializer::Let(binding) | IterableLoopInitializer::Const(binding) => {
            match binding {
                Binding::Identifier(identifier) => vec![identifier.sym()],
                Binding::Pattern(pattern) => boa_engine::ast::operations::bound_names(pattern),
            }
        }
        _ => Vec::new(),
    }
}

/// What the document declares and writes, over every parsed fragment.
fn survey<'a>(parsed: &'a [Option<Parsed>], interner: &mut Interner) -> Survey<'a> {
    let mut survey = Survey::default();
    for (index, parsed) in parsed.iter().enumerate() {
        let Some(parsed) = parsed else {
            continue;
        };
        if parsed.script {
            declare_script(&mut survey, &parsed.ast, index);
        } else if let Some(id) = &parsed.data_id {
            let sym = interner.get_or_intern(id.as_str());
            let function = parsed
                .expression()
                .and_then(|expression| function_of(expression, index, sym));
            declare(&mut survey, sym, function);
        }
        if parsed.location
            && let Some(root) = parsed.expression().and_then(location_root)
        {
            survey.assigned.insert(root);
        }
    }
    for (index, parsed) in parsed.iter().enumerate() {
        let Some(parsed) = parsed else {
            continue;
        };
        let mut visitor = SurveyVisitor {
            survey: &mut survey,
            interner,
            fragment: index,
        };
        let _ = visitor.visit_script(&parsed.ast);
    }
    survey
}

impl<'c, 'a, 'ast> Walker<'c, 'a, 'ast> {
    fn new(
        context: &'c Context<'a, 'ast>,
        fragment: usize,
        memo: &'c mut BTreeMap<CallKey, (Vec<Event>, bool)>,
    ) -> Self {
        Self {
            context,
            frames: Vec::new(),
            fragment,
            events: Vec::new(),
            active: Vec::new(),
            steps: 0,
            exhausted: false,
            reads_global: false,
            memo,
        }
    }
}

/// The document's getters, setters, conversion methods, and generators run
/// implicitly: the first effect any of them has.
fn implicit_cause(
    context: &Context<'_, '_>,
    memo: &mut BTreeMap<CallKey, (Vec<Event>, bool)>,
) -> Option<EffectCause> {
    for function in &context.survey.implicit {
        let mut walker = Walker::new(context, function.fragment, memo);
        let origins = vec![Origin::Unknown; function.parameters.as_ref().len()];
        let mut plain = *function;
        plain.generator = false;
        walker.invoke(
            plain,
            &origins,
            Origin::Unknown,
            Span::new((1, 1), (1, 1)),
            CalleeOrigin::Document,
        );
        if let Some(event) = walker.events.into_iter().next() {
            return event.cause;
        }
    }
    None
}

/// The findings of one checked fragment, sorted and bounded.
fn check_fragment(
    context: &Context<'_, '_>,
    memo: &mut BTreeMap<CallKey, (Vec<Event>, bool)>,
    parsed: &Parsed,
    index: usize,
    expression: &Expression,
) -> Vec<EffectFinding> {
    let mut walker = Walker::new(context, index, memo);
    let _ = walker.visit_expression(expression);
    if walker.events.is_empty() && !walker.exhausted {
        walker.created_functions(expression);
    }
    let whole = walker.span(full_span(expression));
    if walker.exhausted {
        walker.events.push(Event {
            certainty: Certainty::Undecidable,
            kind: EffectKind::TooComplex,
            span: whole,
            subject: String::new(),
            cause: None,
        });
    }
    if walker.reads_global
        && let Some(cause) = &context.implicit_cause
    {
        walker.events.push(Event {
            certainty: Certainty::Undecidable,
            kind: EffectKind::ImplicitCall,
            span: whole,
            subject: cause.function.clone(),
            cause: Some(cause.clone()),
        });
    }
    let operator = plain_assignment_operator(parsed, expression, index);
    let mut local: Vec<EffectFinding> = walker
        .events
        .into_iter()
        .map(|event| EffectFinding {
            certainty: event.certainty,
            operator: operator
                .filter(|(target, _)| {
                    event.kind == EffectKind::Assignment
                        && *target == (event.span.start, event.span.end)
                })
                .map(|(_, at)| at),
            kind: event.kind,
            span: event.span,
            subject: event.subject,
            cause: event.cause,
        })
        .collect();
    local.sort();
    local.dedup_by(|left, right| {
        left.certainty == right.certainty
            && left.kind == right.kind
            && left.span == right.span
            && left.subject == right.subject
    });
    local.truncate(MAX_FINDINGS_PER_FRAGMENT);
    local
}

/// The root variable of a location (`a` of `a.b[0]`).
fn location_root(expression: &Expression) -> Option<Sym> {
    let mut current = strip(expression);
    loop {
        match current {
            Expression::Identifier(identifier) => return Some(identifier.sym()),
            Expression::PropertyAccess(PropertyAccess::Simple(simple)) => {
                current = strip(simple.target());
            }
            _ => return None,
        }
    }
}

/// For a checked fragment that is exactly one plain `=` assignment whose
/// operator stands alone between its operands: the assignment's range (as
/// its finding reports it) and the `=`.
fn plain_assignment_operator(
    parsed: &Parsed,
    expression: &Expression,
    fragment: usize,
) -> Option<((usize, usize), EffectSpan)> {
    let Expression::Assign(assign) = strip(expression) else {
        return None;
    };
    if assign.op() != AssignOp::Assign {
        return None;
    }
    let target_span = match assign.lhs() {
        AssignTarget::Identifier(identifier) => identifier.span(),
        AssignTarget::Access(access) => access_span(access),
        AssignTarget::Pattern(_) => return None,
    };
    let (_, lhs_end) = parsed.range(target_span);
    let (rhs_start, _) = parsed.range(full_span(assign.rhs()));
    let source_start = lhs_end
        .saturating_sub(parsed.inner_start)
        .saturating_add(parsed.prefix);
    let source_end = rhs_start
        .saturating_sub(parsed.inner_start)
        .saturating_add(parsed.prefix);
    let gap = parsed.text.get(source_start..source_end)?;
    let offset = gap.find('=')?;
    if gap.trim() != "=" {
        return None;
    }
    let at = lhs_end + offset;
    let whole = parsed.range(target_span);
    Some((
        whole,
        EffectSpan {
            fragment,
            start: at,
            end: at + 1,
        },
    ))
}
