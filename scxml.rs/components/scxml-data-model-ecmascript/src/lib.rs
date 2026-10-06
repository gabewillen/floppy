//! Bounded ECMAScript SCXML data-model component.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; scripts/verify-component-authored-unsafe.py rejects authored unsafe"
    )
)]

use std::fmt;
use std::io::{self, Write};

use boa_engine::context::HostHooks;
use boa_engine::realm::Realm;
use boa_engine::{Context, JsNativeError, JsResult, JsString};
use scxml_telemetry::{ErrorCategory, FunctionSpan, Module, Operation, SignalEvent, observe};
use scxml_types::{AssignPlacement, AssignSource, DataInitializer, DataValue, Event, FiniteNumber};

/// Maximum initialization source bytes (every build: the `data-model-api`
/// `initialize` bound of the WIT contract, "the declarations' placeholder
/// encoding at most 65536 bytes").
pub const MAX_SOURCE_BYTES: usize = 65_536;
/// Maximum expression or location bytes (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_EXPRESSION_BYTES: usize = scxml_types::wasm_bound(16_384);
/// Maximum executable script bytes (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_SCRIPT_BYTES: usize = scxml_types::wasm_bound(65_536);
/// Maximum bytes in one string, JSON value, XML payload, or ID (every
/// build: the WIT value bound, "each text/JSON/XML value at most 65536
/// bytes").
pub const MAX_VALUE_BYTES: usize = 65_536;
/// Maximum declared `<data>` ids of one instance, so every snapshot fits one
/// `data-model-api.snapshot` image (every build: the WIT contract's "data
/// locations ... to 256").
pub const MAX_VARIABLES: usize = 256;
/// Maximum aggregate bytes of the initial values or of one operation's
/// inputs (wasm builds only; unbounded natively, see
/// [`scxml_types::wasm_bound`]).
pub const MAX_AGGREGATE_BYTES: usize = scxml_types::wasm_bound(1_048_576);
/// Boa loop-iteration limit per operation (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
/// A native operation's loop runs until the program ends it.
pub const MAX_LOOP_ITERATIONS: u64 = scxml_types::wasm_bound(100_000) as u64;
/// Boa recursion limit per operation, on every build: Boa recurses natively
/// for some calls (native functions, accessors), so this guards the native
/// stack rather than bounding a resource.
pub const MAX_RECURSION_DEPTH: usize = 128;
/// Boa value-stack limit per operation (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_STACK_SIZE: usize = scxml_types::wasm_bound(16_384);
/// Most `<foreach>` loops a data model tracks at once; starting another
/// forgets the oldest (a loop an error left unfinished) (wasm builds only;
/// unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_FOREACH_FRAMES: usize = scxml_types::wasm_bound(32);
/// Most items one `<foreach>` array may have (WIT `foreach-result`).
pub const MAX_FOREACH_ITEMS: usize = 10_000;
/// Most distinct state ids one operation may ask `In()` about (wasm builds
/// only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_IN_QUERIES: usize = scxml_types::wasm_bound(128);
/// Maximum value nesting accepted by guarded built-ins whose Boa
/// implementation recurses natively (`JSON.stringify`, `Array.prototype.join`,
/// `toString`, `toLocaleString`, `flat`, `flatMap`). Kept well below
/// [`MAX_RECURSION_DEPTH`] so the guard's catchable `RangeError` fires before
/// Boa's uncatchable recursion limit.
pub const MAX_NESTING_DEPTH: usize = 32;
/// Native stack reserved for every operation that hands source text to Boa.
///
/// Boa's parser, bytecode compiler, and AST drop recurse natively once per
/// syntactic nesting level. [`check_nesting`] bounds program nesting at
/// [`MAX_PARSE_NESTING`] units and `validate_value` bounds portable JSON
/// values at [`MAX_NESTING_DEPTH`] levels; at those bounds one operation needs
/// a few MiB of stack in unoptimized builds. Native (non-Wasm) builds
/// therefore run the Boa portion of every operation on the instance's
/// dedicated worker thread with this stack, so the verdict never depends on
/// the caller's stack. The Wasm component is single-threaded and relies on
/// the structural bounds alone; a stack exhaustion there traps inside the
/// component instead of aborting the host.
pub const PARSE_STACK_BYTES: usize = 64 * 1024 * 1024;

/// Host allocation ceiling for each ECMAScript `ArrayBuffer` or
/// `SharedArrayBuffer`. Boa interprets this hook in bytes despite an upstream
/// rustdoc wording bug that calls the unit bits.
pub const MAX_BUFFER_BYTES: u64 = MAX_VALUE_BYTES as u64;

/// Host hooks of every context; see each hook.
#[derive(Debug, Default)]
struct BoundedHostHooks;

impl HostHooks for BoundedHostHooks {
    fn max_buffer_size(&self, _context: &mut Context) -> u64 {
        MAX_BUFFER_BYTES
    }

    /// Every context runs in UTC: the host time zone is ambient state.
    fn local_timezone_offset_seconds(&self, _unix_time_seconds: i64) -> i32 {
        0
    }

    /// Bounds every runtime string compilation
    /// (`HostEnsureCanCompileStrings`, which ECMA-262 lets the host use to
    /// refuse a compilation by throwing).
    ///
    /// Boa consults this hook before parsing for direct and indirect `eval`
    /// and for the `Function`, `GeneratorFunction`, `AsyncFunction`, and
    /// `AsyncGeneratorFunction` constructors. Every program text handed to
    /// Boa must pass [`check_nesting`] (Boa's parser recurses natively), so
    /// the text the call compiles (the `eval` string, or the constructor's
    /// `function anonymous(...)` text) must pass it too and be at most
    /// [`MAX_SCRIPT_BYTES`]; otherwise the call throws a catchable
    /// `TypeError` and parses nothing.
    fn ensure_can_compile_strings(
        &self,
        _realm: Realm,
        parameters: &[JsString],
        body: &JsString,
        direct: bool,
        _context: &mut Context,
    ) -> JsResult<()> {
        let body = body.to_std_string_escaped();
        let text = if direct || parameters.is_empty() && !body.is_empty() {
            body
        } else {
            let parameters = parameters
                .iter()
                .map(JsString::to_std_string_escaped)
                .collect::<Vec<_>>()
                .join(",");
            format!("(function anonymous({parameters}\n) {{\n{body}\n}})")
        };
        if scxml_types::exceeds_wasm_bound(text.len(), MAX_SCRIPT_BYTES)
            || check_nesting(&text).is_err()
        {
            return Err(JsNativeError::typ()
                .with_message("runtime source exceeds the data model's parse bounds")
                .into());
        }
        Ok(())
    }
}

/// First script of every context: removes the shared-memory and
/// job-queue built-ins before any other script runs.
///
/// The realm, not the program text, enforces the removal: a reference to a
/// removed global evaluates to `undefined` (so using it raises a `TypeError`,
/// which is `error.execution`), while identifiers, strings, property names,
/// and comments that merely spell a removed name evaluate normally. Every
/// replaced property is non-writable and non-configurable, so user code
/// cannot restore it. `eval` and the `Function` constructors stay (B.2
/// requires ECMAScript 3rd edition, whose global object has both);
/// [`BoundedHostHooks::ensure_can_compile_strings`] bounds what they
/// compile, so no source text reaches Boa's parser without passing
/// [`check_nesting`].
const BOOTSTRAP: &str = r#"
for (const name of ["Promise","Proxy","WebAssembly","Atomics","SharedArrayBuffer"]) {
  Object.defineProperty(globalThis,name,{value:undefined,writable:false,configurable:false});
}
"#;
/// Bounded replacements for high-amplification Boa built-ins; see the file
/// header for the contract. Evaluated after [`BOOTSTRAP`] and before any
/// other script in every realm.
const BUILTIN_GUARD: &str = include_str!("builtin_guard.js");

/// Deterministic `<log>` text of a value (SCXML 4.7 leaves the
/// format to the platform), evaluated inside the adapter's helpers
/// (`session.rs`), whose captured built-ins it uses:
///
/// - a string is itself;
/// - `undefined` and the adapter's frozen absent-value marker are
///   `undefined`; an XML document (B.2.1) is its XML text (`xmlOf`, the
///   adapter's text lookup, answers it); every other
///   primitive is its ECMAScript `String()` conversion (`null`, `true`,
///   `NaN`, `2`, ...);
/// - an object or function is its bounded `JSON.stringify` text (undefined
///   members and nested absent markers omitted, as JSON defines for
///   undefined); when JSON has no text for it (a function, a cycle, a bound
///   violation) it is its fixed built-in tag (`[object Function]`,
///   `[object Object]`, ...). User `toString` overrides never run.
const LOG_TEXT: &str = r#"(function (value, xmlOf) {
  const marker = (v, kind) => v !== null && typeof v === "object" && isFrozen(v)
    && hasOwn(v, "$scxml") && describe(v, "$scxml").value === kind;
  if (typeof value === "string") return value;
  if (value === undefined || marker(value, "absent")) return "undefined";
  const xml = xmlOf(value);
  if (typeof xml === "string") return xml;
  if (value === null || (typeof value !== "object" && typeof value !== "function")) {
    return NativeString(value);
  }
  try {
    const json = stringify(value, (key, v) => marker(v, "absent") ? undefined : v);
    if (typeof json === "string") return json;
  } catch (error) {}
  return apply(objectToString, value, []);
})"#;

/// Immutable SCXML system variables.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemContext {
    /// `_sessionid` value.
    pub session_id: String,
    /// `_name` value: the document's `name` attribute, `None` when it has
    /// none (SCXML 5.10); `_name` is then `undefined`.
    pub machine_name: Option<String>,
    /// `_ioprocessors` canonical JSON object.
    pub io_processors: DataValue,
}

/// One question an operation asks its execution host while it runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostQuery {
    /// Whether the state with this id is active (`In()`, B.2.3).
    InState(String),
    /// The session clock's current time ([`ExecutionHost::now_ms`]).
    NowMs,
}

/// The execution host's answer to one [`HostQuery`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostReply {
    /// The answer to [`HostQuery::InState`].
    InState(bool),
    /// The answer to [`HostQuery::NowMs`].
    NowMs(u64),
}

/// A host that answers an operation's questions on its own, with no
/// borrow of the caller (see [`ExecutionHost::answerer`]).
pub type Answerer = Box<dyn FnMut(&HostQuery) -> Result<HostReply, DataModelError>>;

/// Explicit execution-host capability used by `In()` and `Date`.
pub trait ExecutionHost {
    /// Queries the current machine configuration.
    ///
    /// # Errors
    ///
    /// Returns [`DataModelError::Host`] when the imported query fails.
    fn in_state(&mut self, state_id: &str) -> Result<bool, DataModelError>;

    /// The session clock's current time in milliseconds since the Unix
    /// epoch (WIT `execution-host.now-ms`). ECMAScript `Date.now()` and
    /// `new Date()` read it (ECMA-262 15.9.3.3: "the time value (UTC)
    /// identifying the current time"). A host without a session clock is
    /// at the epoch (the default, 0).
    ///
    /// # Errors
    ///
    /// Returns [`DataModelError::Host`] when the imported query fails.
    fn now_ms(&mut self) -> Result<u64, DataModelError> {
        Ok(0)
    }

    /// An [`Answerer`] that asks what this host would answer, for a realm
    /// that runs in place on the caller's thread (the Wasm component); the
    /// default has none, and such a realm then fails every operation with
    /// [`DataModelError::Host`]. Native builds ask the host itself.
    fn answerer(&mut self) -> Option<Answerer> {
        None
    }
}

/// Bounded data-model failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataModelError {
    /// Input was malformed or over bound.
    InvalidInput,
    /// Boa rejected or exhausted bounded execution.
    Execution,
    /// A JavaScript value had no lossless portable representation.
    UnsupportedValue,
    /// Initialization has not completed.
    NotInitialized,
    /// The imported host query failed.
    Host,
}

impl fmt::Display for DataModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidInput => "ECMAScript input is invalid or exceeds its bound",
            Self::Execution => "bounded ECMAScript execution failed",
            Self::UnsupportedValue => "ECMAScript value is not losslessly portable",
            Self::NotInitialized => "ECMAScript data model is not initialized",
            Self::Host => "execution-host state query failed",
        })
    }
}

impl std::error::Error for DataModelError {}

/// The value of one declared variable in a read-only [`DataModelSnapshot`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VariableValue {
    /// Its portable value.
    Value(DataValue),
    /// A value with no portable form: its ECMAScript `typeof` (`kind`) and
    /// its string form (`text`, see `Session::string_form`).
    Text {
        /// The value's ECMAScript `typeof`.
        kind: String,
        /// The value's string form.
        text: String,
    },
    /// A value whose rendered text (string, JSON, or XML text, or string
    /// form) is longer than [`MAX_VALUE_BYTES`]: that text's exact length in
    /// bytes. The text is not carried.
    Oversized(u64),
}

impl VariableValue {
    /// `image`, or [`Self::Oversized`] when its rendered text is longer
    /// than [`MAX_VALUE_BYTES`].
    fn sized(image: Self) -> Self {
        let bytes = match &image {
            Self::Value(value) => rendered_bytes(value),
            Self::Text { text, .. } => text.len(),
            Self::Oversized(_) => 0,
        };
        if bytes > MAX_VALUE_BYTES {
            Self::Oversized(u64::try_from(bytes).unwrap_or(u64::MAX))
        } else {
            image
        }
    }
}

/// Byte length of the text a snapshot renders for `value` (0 for a value
/// with no text).
fn rendered_bytes(value: &DataValue) -> usize {
    match value {
        DataValue::String(text)
        | DataValue::JsonArray(text)
        | DataValue::JsonObject(text)
        | DataValue::Xml(text) => text.len(),
        DataValue::Absent
        | DataValue::Undefined
        | DataValue::Null
        | DataValue::Bool(_)
        | DataValue::Number(_) => 0,
    }
}

/// One declared variable in a read-only [`DataModelSnapshot`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Variable {
    /// The `<data>` id.
    pub name: String,
    /// Its current value.
    pub value: VariableValue,
}

/// Read-only image of the data model (`docs/session-snapshot.md`,
/// "Session object (keys in this order)").
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataModelSnapshot {
    /// SCXML 5.10 system variables.
    pub system: SystemContext,
    /// Every declared `<data>` id in document order (late-bound ids
    /// declared later follow), with its current value.
    pub variables: Vec<Variable>,
}

/// Single-owner ECMAScript data model: one Boa realm kept for the whole
/// session (`session.rs` states the contract), so every value, closure,
/// and built-in change a script makes persists as ECMAScript defines it
/// (SCXML 5.8, B.2). The instance records only what the realm cannot
/// answer: the system variables and the declared `<data>` ids in document
/// order.
#[derive(Default)]
pub struct EcmaScriptDataModel {
    system: Option<SystemContext>,
    declared: Vec<String>,
    engine: session::Engine,
}

impl fmt::Debug for EcmaScriptDataModel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EcmaScriptDataModel")
            .field("system", &self.system)
            .field("declared", &self.declared)
            .finish_non_exhaustive()
    }
}

impl EcmaScriptDataModel {
    /// Creates an uninitialized model.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates the session's realm from a bounded canonical JSON object of
    /// initial values (its members declared in key order) and `system`.
    ///
    /// # Errors
    ///
    /// Rejects malformed source, invalid/reserved names, bounds violations, or
    /// a non-object `_ioprocessors` value. Failure keeps no realm.
    pub fn initialize(
        &mut self,
        source: &str,
        system: SystemContext,
    ) -> Result<(), DataModelError> {
        let span = observe(
            Module::DataModelEcmascriptComponent,
            Operation::DataModelInitialize,
        );
        let result = (|| {
            bounded(source, MAX_SOURCE_BYTES)?;
            let parsed: serde_json::Value = if source.trim().is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(source).map_err(|_| DataModelError::InvalidInput)?
            };
            if !source.trim().is_empty()
                && serde_json::to_string(&parsed).map_err(|_| DataModelError::InvalidInput)?
                    != source.trim()
            {
                return Err(DataModelError::InvalidInput);
            }
            let object = parsed.as_object().ok_or(DataModelError::InvalidInput)?;
            let mut declarations = Vec::with_capacity(object.len());
            for (name, value) in object {
                declarations.push((name.clone(), from_json(value)?));
            }
            self.start(&span, declarations, system)
        })();
        complete(&span, &result);
        result
    }

    /// Declares `declarations` (document order, unique) with the ECMAScript
    /// placeholder `null` and binds `system` (WIT `data-model-api.initialize`;
    /// SCXML 5.2-5.3 before any initializer runs).
    ///
    /// # Errors
    ///
    /// [`DataModelError::InvalidInput`] for a duplicate declaration, and
    /// everything [`Self::initialize`] rejects. Failure keeps no realm.
    pub fn initialize_declarations<'a>(
        &mut self,
        declarations: impl IntoIterator<Item = &'a str>,
        system: SystemContext,
    ) -> Result<(), DataModelError> {
        let span = observe(
            Module::DataModelEcmascriptComponent,
            Operation::DataModelInitialize,
        );
        let result = (|| {
            let mut placeholders = Vec::new();
            for id in declarations {
                if placeholders.iter().any(|(declared, _)| declared == id) {
                    return Err(DataModelError::InvalidInput);
                }
                placeholders.push((id.to_owned(), DataValue::Null));
            }
            self.start(&span, placeholders, system)
        })();
        complete(&span, &result);
        result
    }

    /// Validates and creates the realm with `declarations` in order.
    fn start(
        &mut self,
        _span: &FunctionSpan,
        declarations: Vec<(String, DataValue)>,
        system: SystemContext,
    ) -> Result<(), DataModelError> {
        validate_system(&system)?;
        if declarations.len() > MAX_VARIABLES {
            return Err(DataModelError::InvalidInput);
        }
        let mut total = checked_sum([
            system.session_id.len(),
            system.machine_name.as_ref().map_or(0, String::len),
            value_bytes(&system.io_processors),
        ])?;
        for (name, value) in &declarations {
            validate_data_id(name)?;
            validate_value(value)?;
            total = checked_sum([total, name.len(), value_bytes(value)])?;
        }
        if scxml_types::exceeds_wasm_bound(total, MAX_AGGREGATE_BYTES) {
            return Err(DataModelError::InvalidInput);
        }
        self.system = None;
        self.declared.clear();
        let names = declarations.iter().map(|(name, _)| name.clone()).collect();
        self.engine.start(&system, declarations)?;
        self.system = Some(system);
        self.declared = names;
        Ok(())
    }

    /// Values one declared `<data>` variable (WIT
    /// `data-model-api.initialize-data`; SCXML 5.2-5.3). An expression is
    /// evaluated with no current event, so a function is a legal initial
    /// value (B.2, official 453).
    ///
    /// # Errors
    ///
    /// As [`Self::assign_expression_without_event`] or [`Self::assign`].
    pub fn initialize_data(
        &mut self,
        id: &str,
        initializer: &DataInitializer,
        host: &mut impl ExecutionHost,
    ) -> Result<(), DataModelError> {
        let span = observe(
            Module::DataModelEcmascriptComponent,
            Operation::DataModelInitialize,
        );
        let result = (|| {
            if !self.declared.iter().any(|declared| declared == id) {
                return Err(DataModelError::InvalidInput);
            }
            if !syntax::is_variable_name(id) {
                return self.initialize_global_property(id, initializer, host);
            }
            match initializer {
                DataInitializer::Expression(expression) => {
                    self.run_assignment(id, expression, None, host)
                }
                DataInitializer::Value(value) => self.assign_value(id, value, host),
            }
        })();
        complete(&span, &result);
        result
    }

    /// Values a declared `<data>` id that is not an ECMAScript identifier
    /// (`retry-count`, `class`): no location expression names it, so the
    /// value is set on the global object's own property of that name, the
    /// variable B.2.1 creates.
    fn initialize_global_property(
        &mut self,
        id: &str,
        initializer: &DataInitializer,
        host: &mut impl ExecutionHost,
    ) -> Result<(), DataModelError> {
        self.system_context()?;
        let name = id.to_owned();
        match initializer {
            DataInitializer::Expression(expression) => {
                validate_program(expression, MAX_EXPRESSION_BYTES)?;
                let source = checked_source(value_expression_source(expression))?;
                self.engine.run(host, None, move |session| {
                    let value = session
                        .eval(&source)
                        .map_err(|error| classify_js_error(&error))?;
                    session.declare(&name, value)
                })
            }
            DataInitializer::Value(value) => {
                validate_value(value)?;
                let value = value.clone();
                self.engine.run(host, None, move |session| {
                    let value = session.value_of(&value)?;
                    session.declare(&name, value)
                })
            }
        }
    }

    /// Evaluates a condition (its ECMAScript `ToBoolean`).
    ///
    /// # Errors
    ///
    /// Rejects blank or over-bound source, host failures, and runtime limits.
    pub fn evaluate_condition(
        &self,
        expression: &str,
        event: &Event,
        host: &mut impl ExecutionHost,
    ) -> Result<bool, DataModelError> {
        let span = observe(
            Module::DataModelEcmascriptComponent,
            Operation::DataModelEvaluateCondition,
        );
        let result = (|| {
            validate_program(expression, MAX_EXPRESSION_BYTES)?;
            validate_event(event)?;
            bounded_total([expression.len(), event_bytes(event)?], MAX_AGGREGATE_BYTES)?;
            self.evaluate(expression, Some(event), host, |value, _| {
                Ok(value.to_boolean())
            })
        })();
        complete(&span, &result);
        result
    }

    /// Evaluates a condition when SCXML defines no current event (startup,
    /// eventless transitions, root initial, and no-trigger entry
    /// execution): `_event` is unbound.
    ///
    /// # Errors
    ///
    /// Rejects blank or over-bound source, host failures, and runtime limits.
    pub fn evaluate_condition_without_event(
        &self,
        expression: &str,
        host: &mut impl ExecutionHost,
    ) -> Result<bool, DataModelError> {
        let span = observe(
            Module::DataModelEcmascriptComponent,
            Operation::DataModelEvaluateCondition,
        );
        let result = (|| {
            validate_program(expression, MAX_EXPRESSION_BYTES)?;
            self.evaluate(expression, None, host, |value, _| Ok(value.to_boolean()))
        })();
        complete(&span, &result);
        result
    }

    /// Evaluates to a portable value without lossy coercion.
    ///
    /// # Errors
    ///
    /// Functions, bigint, symbols, exotic/accessor/symbol-keyed/cyclic objects,
    /// sparse or undefined JSON members, NaN, and infinity are rejected.
    /// `ArrayBuffer` allocations are host-capped at [`MAX_BUFFER_BYTES`].
    /// Single-call amplifying built-ins (`repeat`, `padStart`, `padEnd`,
    /// string `concat`, `replace`, `replaceAll`, `RegExp.prototype[@@replace]`,
    /// `String.raw`, array `join`/`toString`/`toLocaleString`/`concat`/`flat`/
    /// `flatMap`, length-driven `Array.prototype` methods, array iterators,
    /// `Array.from`, `Function.prototype.apply`, `Reflect.apply`/`construct`,
    /// and `JSON.stringify`) reject with a `RangeError` before Boa allocates,
    /// iterates, or recurses past [`MAX_VALUE_BYTES`] or
    /// [`MAX_NESTING_DEPTH`]; `Proxy` is unavailable. Operator-level string
    /// concatenation and native iteration of user-defined iterables still
    /// allocate inside Boa before the portable-value bound; Wasm memory/fuel
    /// limits remain the outer bound for those.
    pub fn evaluate_value(
        &self,
        expression: &str,
        event: &Event,
        host: &mut impl ExecutionHost,
    ) -> Result<DataValue, DataModelError> {
        let span = observe(
            Module::DataModelEcmascriptComponent,
            Operation::DataModelEvaluateValue,
        );
        let result = (|| {
            validate_program(expression, MAX_EXPRESSION_BYTES)?;
            validate_event(event)?;
            bounded_total([expression.len(), event_bytes(event)?], MAX_AGGREGATE_BYTES)?;
            self.evaluate(expression, Some(event), host, |value, session| {
                session.portable_value(value)
            })
        })();
        complete(&span, &result);
        result
    }

    /// Evaluates a portable value when SCXML defines no current event
    /// (`_event` is unbound).
    ///
    /// # Errors
    ///
    /// As [`Self::evaluate_value`].
    pub fn evaluate_value_without_event(
        &self,
        expression: &str,
        host: &mut impl ExecutionHost,
    ) -> Result<DataValue, DataModelError> {
        let span = observe(
            Module::DataModelEcmascriptComponent,
            Operation::DataModelEvaluateValue,
        );
        let result = (|| {
            validate_program(expression, MAX_EXPRESSION_BYTES)?;
            self.evaluate(expression, None, host, |value, session| {
                session.portable_value(value)
            })
        })();
        complete(&span, &result);
        result
    }

    /// Evaluates a `<log expr>` to its deterministic text (SCXML 4.7).
    ///
    /// Unlike [`Self::evaluate_value`], every ECMAScript value is loggable:
    /// see `LOG_TEXT` for the exact rendering. An empty or blank `expr`
    /// is no expression (the attribute is optional) and logs empty text
    /// (official case 307).
    ///
    /// # Errors
    ///
    /// Rejects blank or over-bound source, host failures, and runtime
    /// limits; ECMAScript runtime errors are [`DataModelError::Execution`];
    /// text longer than [`MAX_VALUE_BYTES`] is [`DataModelError::InvalidInput`].
    pub fn evaluate_log(
        &self,
        expression: &str,
        event: &Event,
        host: &mut impl ExecutionHost,
    ) -> Result<String, DataModelError> {
        let span = observe(
            Module::DataModelEcmascriptComponent,
            Operation::DataModelEvaluateLog,
        );
        let result = (|| {
            if nesting::is_blank(expression) {
                bounded(expression, MAX_EXPRESSION_BYTES)?;
                return Ok(String::new());
            }
            validate_program(expression, MAX_EXPRESSION_BYTES)?;
            validate_event(event)?;
            bounded_total([expression.len(), event_bytes(event)?], MAX_AGGREGATE_BYTES)?;
            self.evaluate(expression, Some(event), host, |value, session| {
                session.log_text(value)
            })
        })();
        complete(&span, &result);
        result
    }

    /// Evaluates a `<log expr>` to its deterministic text when SCXML
    /// defines no current event; otherwise identical to
    /// [`Self::evaluate_log`].
    ///
    /// # Errors
    ///
    /// As for [`Self::evaluate_log`].
    pub fn evaluate_log_without_event(
        &self,
        expression: &str,
        host: &mut impl ExecutionHost,
    ) -> Result<String, DataModelError> {
        let span = observe(
            Module::DataModelEcmascriptComponent,
            Operation::DataModelEvaluateLog,
        );
        let result = (|| {
            if nesting::is_blank(expression) {
                bounded(expression, MAX_EXPRESSION_BYTES)?;
                return Ok(String::new());
            }
            validate_program(expression, MAX_EXPRESSION_BYTES)?;
            self.evaluate(expression, None, host, |value, session| {
                session.log_text(value)
            })
        })();
        complete(&span, &result);
        result
    }

    /// Evaluates the value expression `expression` with `_event` bound to
    /// `event` and converts the result with `finish`.
    fn evaluate<T: Send + 'static>(
        &self,
        expression: &str,
        event: Option<&Event>,
        host: &mut impl ExecutionHost,
        finish: fn(&boa_engine::JsValue, &mut session::Session) -> Result<T, DataModelError>,
    ) -> Result<T, DataModelError> {
        self.system_context()?;
        let source = checked_source(value_expression_source(expression))?;
        self.engine.run(host, event, move |session| {
            let value = session
                .eval(&source)
                .map_err(|error| classify_js_error(&error))?;
            finish(&value, session)
        })
    }

    /// Executes one SCXML `<assign>` or `<foreach>` `item`/`index`
    /// assignment (SCXML 5.4, 4.6, B.2.7): the single definition of this data
    /// model's assignment semantics, which the WIT component executes for
    /// every machine.
    ///
    /// * Only [`AssignPlacement::ReplaceChildren`] is defined; every other
    ///   placement (the `XPath` data model's, SCXML 1.0 CR 2014-03-13 §C.3.6)
    ///   is [`DataModelError::Execution`] and changes no data.
    /// * [`AssignSource::Expression`] is evaluated with `_event` bound to
    ///   `event` (unbound when `None`) and assigned, so any ECMAScript value
    ///   is assignable ([`Self::assign_expression`]).
    /// * [`AssignSource::Value`] assigns a materialized value
    ///   ([`Self::assign_with`]).
    /// * [`AssignSource::ForeachValue`] binds a `<foreach>` variable,
    ///   declaring a plain name that is not declared yet
    ///   ([`Self::declare_assign`]).
    ///
    /// # Errors
    ///
    /// As the operation the source selects.
    pub fn apply_assign(
        &mut self,
        location: &str,
        source: &AssignSource,
        placement: &AssignPlacement,
        event: Option<&Event>,
        host: &mut impl ExecutionHost,
    ) -> Result<(), DataModelError> {
        if *placement != AssignPlacement::ReplaceChildren {
            return Err(DataModelError::Execution);
        }
        match (source, event) {
            (AssignSource::Expression(expression), Some(event)) => {
                self.assign_expression(location, expression, event, host)
            }
            (AssignSource::Expression(expression), None) => {
                self.assign_expression_without_event(location, expression, host)
            }
            (AssignSource::Value(value), _) => self.assign_with(location, value, host),
            (AssignSource::ForeachValue(value), _) => self.declare_assign(location, value, host),
        }
    }

    /// Assigns a materialized value outside any state configuration (an
    /// `<invoke>` parameter set before the invoked session starts, SCXML
    /// 6.4.2): the configuration is empty, so an `In()` call in `location`
    /// answers false.
    ///
    /// # Errors
    ///
    /// As [`Self::assign_with`].
    pub fn assign(&mut self, location: &str, value: &DataValue) -> Result<(), DataModelError> {
        self.assign_with(location, value, &mut NoHost)
    }

    /// Assigns the materialized `value` to `location` (SCXML 5.4, B.2.7).
    ///
    /// # Errors
    ///
    /// [`DataModelError::InvalidInput`] for a malformed or over-bound
    /// location or value; [`DataModelError::Execution`] when `location` is
    /// not a location ECMAScript can assign (an unresolvable name, a
    /// read-only property, a system variable, or not an assignment target).
    pub fn assign_with(
        &mut self,
        location: &str,
        value: &DataValue,
        host: &mut impl ExecutionHost,
    ) -> Result<(), DataModelError> {
        let span = observe(
            Module::DataModelEcmascriptComponent,
            Operation::DataModelAssign,
        );
        let result = self.assign_value(location, value, host);
        complete(&span, &result);
        result
    }

    /// [`Self::assign_with`] without its span.
    fn assign_value(
        &mut self,
        location: &str,
        value: &DataValue,
        host: &mut impl ExecutionHost,
    ) -> Result<(), DataModelError> {
        self.system_context()?;
        let assigner = checked_location(location)?;
        validate_value(value)?;
        bounded_total(
            [location.len(), checked_scale(value_bytes(value), 6)?, 48],
            MAX_AGGREGATE_BYTES,
        )?;
        let value = value.clone();
        self.engine.run(host, None, move |session| {
            let value = session.value_of(&value)?;
            session
                .call_source(&assigner, &[value])
                .map(drop)
                .map_err(|error| classify_js_error(&error))
        })
    }

    /// Assigns the value of `expression` to `location` (SCXML 5.4, B.2.7):
    /// the expression is evaluated first, then assigned, so any ECMAScript
    /// value is assignable.
    ///
    /// # Errors
    ///
    /// [`DataModelError::InvalidInput`] for a malformed or over-bound
    /// location or expression; [`DataModelError::Execution`] for an
    /// ECMAScript syntax or runtime error, including a location ECMAScript
    /// cannot assign.
    pub fn assign_expression(
        &mut self,
        location: &str,
        expression: &str,
        event: &Event,
        host: &mut impl ExecutionHost,
    ) -> Result<(), DataModelError> {
        let span = observe(
            Module::DataModelEcmascriptComponent,
            Operation::DataModelAssign,
        );
        let result = (|| {
            validate_event(event)?;
            bounded_total(
                [location.len(), expression.len(), event_bytes(event)?],
                MAX_AGGREGATE_BYTES,
            )?;
            self.run_assignment(location, expression, Some(event), host)
        })();
        complete(&span, &result);
        result
    }

    /// [`Self::assign_expression`] when SCXML defines no current event
    /// (startup data initializers, eventless and no-trigger entry content).
    ///
    /// # Errors
    ///
    /// As for [`Self::assign_expression`].
    pub fn assign_expression_without_event(
        &mut self,
        location: &str,
        expression: &str,
        host: &mut impl ExecutionHost,
    ) -> Result<(), DataModelError> {
        let span = observe(
            Module::DataModelEcmascriptComponent,
            Operation::DataModelAssign,
        );
        let result = self.run_assignment(location, expression, None, host);
        complete(&span, &result);
        result
    }

    /// Evaluates `expression` and assigns it to `location`.
    fn run_assignment(
        &mut self,
        location: &str,
        expression: &str,
        event: Option<&Event>,
        host: &mut impl ExecutionHost,
    ) -> Result<(), DataModelError> {
        self.system_context()?;
        let assigner = checked_location(location)?;
        validate_program(expression, MAX_EXPRESSION_BYTES)?;
        let value = checked_source(value_expression_source(expression))?;
        self.engine.run(host, event, move |session| {
            let value = session
                .eval(&value)
                .map_err(|error| classify_js_error(&error))?;
            session
                .call_source(&assigner, &[value])
                .map(drop)
                .map_err(|error| classify_js_error(&error))
        })
    }

    /// Assigns `value` to the variable `name`, declaring it first when it
    /// is not a global variable yet (a `<foreach>` `item` or `index`, SCXML
    /// 4.6). When a `<foreach>` in progress expects `value` as its next item
    /// under `name`, the variable is bound to that element itself (B.2.11:
    /// the loop is "equivalent to `item = array_name[index]`").
    ///
    /// B.2.11: "The legal values for the 'item' attribute are legal
    /// ECMAScript variable names." `name` must be exactly one ECMAScript
    /// identifier (`syntax::is_variable_name`) and not a system variable
    /// (5.10: "The Processor MUST cause any attempt to change the value of
    /// a system variable to fail").
    ///
    /// # Errors
    ///
    /// [`DataModelError::InvalidInput`] for any other name; otherwise as
    /// [`Self::assign_with`].
    pub fn declare_assign(
        &mut self,
        name: &str,
        value: &DataValue,
        host: &mut impl ExecutionHost,
    ) -> Result<(), DataModelError> {
        let span = observe(
            Module::DataModelEcmascriptComponent,
            Operation::DataModelAssign,
        );
        let result = self.declare_value(name, value, host);
        complete(&span, &result);
        result
    }

    /// [`Self::declare_assign`] without its span.
    pub(crate) fn declare_value(
        &mut self,
        name: &str,
        value: &DataValue,
        host: &mut impl ExecutionHost,
    ) -> Result<(), DataModelError> {
        self.system_context()?;
        bounded(name, scxml_types::MAX_NAME_BYTES)?;
        if !syntax::is_variable_name(name) {
            return Err(DataModelError::InvalidInput);
        }
        validate_location_name(name)?;
        validate_value(value)?;
        let name = name.to_owned();
        let value = value.clone();
        self.engine.run(host, None, move |session| {
            let bound = match session.foreach_element(&name, &value)? {
                Some(element) => element,
                None => session.value_of(&value)?,
            };
            session.declare(&name, bound)
        })
    }

    /// Executes a bounded script in the global scope (SCXML 5.8):
    /// declarations persist as global variables, and a failure leaves every
    /// effect the script made before it failed, as ECMAScript defines.
    ///
    /// # Errors
    ///
    /// Blank or over-bound source; any ECMAScript failure (syntax, runtime,
    /// or a runtime bound) is [`DataModelError::Execution`].
    pub fn execute(
        &mut self,
        script: &str,
        event: &Event,
        host: &mut impl ExecutionHost,
    ) -> Result<(), DataModelError> {
        let span = observe(
            Module::DataModelEcmascriptComponent,
            Operation::DataModelExecute,
        );
        let result = (|| {
            validate_program(script, MAX_SCRIPT_BYTES)?;
            validate_event(event)?;
            bounded_total([script.len(), event_bytes(event)?], MAX_AGGREGATE_BYTES)?;
            self.run_script(script, Some(event), host)
        })();
        complete(&span, &result);
        result
    }

    /// Executes a bounded script when SCXML defines no current event
    /// (startup, root initial, eventless, and no-trigger entry executable
    /// content); otherwise identical to [`Self::execute`].
    ///
    /// # Errors
    ///
    /// As for [`Self::execute`].
    pub fn execute_without_event(
        &mut self,
        script: &str,
        host: &mut impl ExecutionHost,
    ) -> Result<(), DataModelError> {
        let span = observe(
            Module::DataModelEcmascriptComponent,
            Operation::DataModelExecute,
        );
        let result = (|| {
            validate_program(script, MAX_SCRIPT_BYTES)?;
            self.run_script(script, None, host)
        })();
        complete(&span, &result);
        result
    }

    /// Runs one checked script. SCXML 5.8: any script evaluation failure
    /// (syntax or runtime) is a recoverable execution error, so the
    /// interpreter raises `error.execution` around the aborted block.
    fn run_script(
        &mut self,
        script: &str,
        event: Option<&Event>,
        host: &mut impl ExecutionHost,
    ) -> Result<(), DataModelError> {
        self.system_context()?;
        let source = checked_source(script.to_owned())?;
        self.engine.run(host, event, move |session| {
            session
                .eval(&source)
                .map(drop)
                .map_err(|_| DataModelError::Execution)
        })
    }

    /// Whether `location` starts from a declared `<data>` id
    /// ([`location_root`]).
    pub(crate) fn starts_at_declared_variable(&self, location: &str) -> bool {
        location_root(location).is_some_and(|root| self.declared.iter().any(|id| id == root))
    }

    /// Forgets every `<foreach>` in progress (WIT
    /// `data-model-api.release-foreach`): the machine call that ran them
    /// returned, so each finished, or an error in its body ended it early
    /// (SCXML 4.6) and it never finishes. Without this, each loop an error
    /// ended would stay recorded for the rest of the session.
    ///
    /// # Errors
    ///
    /// [`DataModelError::NotInitialized`] before initialization.
    pub fn release_foreach(&self) -> Result<(), DataModelError> {
        self.system_context()?;
        self.engine.run(&mut NoHost, None, |session| {
            session.release_foreach();
            Ok(())
        })
    }

    /// The `<foreach>` loops in progress (read-only).
    ///
    /// # Errors
    ///
    /// [`DataModelError::NotInitialized`] before initialization.
    pub fn foreach_in_progress(&self) -> Result<usize, DataModelError> {
        self.system_context()?;
        self.engine.run(&mut NoHost, None, |session| {
            Ok(session.foreach_in_progress())
        })
    }

    /// Returns owned immutable system context.
    ///
    /// # Errors
    ///
    /// Fails before initialization.
    pub fn system_context(&self) -> Result<SystemContext, DataModelError> {
        self.system.clone().ok_or(DataModelError::NotInitialized)
    }

    /// Read-only image of the data model: its system variables and each
    /// declared `<data>` id in document order with its current value, read
    /// from the realm without running user code (`Session::shown`): its
    /// portable value, or else its `typeof` and string form. A value whose
    /// rendered text is longer than [`MAX_VALUE_BYTES`] is
    /// [`VariableValue::Oversized`]; nothing is truncated.
    ///
    /// # Errors
    ///
    /// [`DataModelError::NotInitialized`] before initialization.
    pub fn snapshot(&self) -> Result<DataModelSnapshot, DataModelError> {
        let span = observe(
            Module::DataModelEcmascriptComponent,
            Operation::DataModelSnapshot,
        );
        let result = (|| {
            let system = self.system_context()?;
            let names = self.declared.clone();
            let shown = self.engine.run(&mut NoHost, None, move |session| {
                Ok(names
                    .into_iter()
                    .filter_map(|name| {
                        let value = match session.shown(&name)? {
                            session::Shown::Value(value) => VariableValue::Value(value),
                            session::Shown::Text(kind, text) => VariableValue::Text { kind, text },
                        };
                        Some(Variable {
                            name,
                            value: VariableValue::sized(value),
                        })
                    })
                    .collect::<Vec<_>>())
            })?;
            Ok(DataModelSnapshot {
                system,
                variables: shown,
            })
        })();
        complete(&span, &result);
        result
    }
}

/// The execution host outside any state configuration: `In()` answers
/// false for every state, and the clock reads the epoch.
pub(crate) struct NoHost;

impl ExecutionHost for NoHost {
    fn in_state(&mut self, _state_id: &str) -> Result<bool, DataModelError> {
        Ok(false)
    }

    fn answerer(&mut self) -> Option<Answerer> {
        Some(Box::new(|query: &HostQuery| {
            Ok(match query {
                HostQuery::InState(_) => HostReply::InState(false),
                HostQuery::NowMs => HostReply::NowMs(0),
            })
        }))
    }
}

fn complete<T>(span: &FunctionSpan, result: &Result<T, DataModelError>) {
    match result {
        Ok(_) => span.event(SignalEvent::TransformationCompleted),
        Err(error) => {
            span.event(SignalEvent::InputRejected);
            span.fail(category(*error));
        }
    }
}

const fn category(error: DataModelError) -> ErrorCategory {
    match error {
        DataModelError::InvalidInput => ErrorCategory::InvalidInput,
        DataModelError::Execution => ErrorCategory::Execution,
        DataModelError::UnsupportedValue => ErrorCategory::Unsupported,
        DataModelError::NotInitialized | DataModelError::Host => ErrorCategory::Platform,
    }
}

fn bounded(value: &str, max: usize) -> Result<(), DataModelError> {
    if value.len() <= max {
        Ok(())
    } else {
        Err(DataModelError::InvalidInput)
    }
}

fn checked_sum(values: impl IntoIterator<Item = usize>) -> Result<usize, DataModelError> {
    values.into_iter().try_fold(0_usize, |total, value| {
        total.checked_add(value).ok_or(DataModelError::InvalidInput)
    })
}

fn checked_scale(value: usize, factor: usize) -> Result<usize, DataModelError> {
    value
        .checked_mul(factor)
        .ok_or(DataModelError::InvalidInput)
}

fn bounded_total(
    values: impl IntoIterator<Item = usize>,
    max: usize,
) -> Result<(), DataModelError> {
    if checked_sum(values)? <= max {
        Ok(())
    } else {
        Err(DataModelError::InvalidInput)
    }
}

fn value_bytes(value: &DataValue) -> usize {
    match value {
        DataValue::String(value)
        | DataValue::JsonArray(value)
        | DataValue::JsonObject(value)
        | DataValue::Xml(value) => value.len(),
        DataValue::Absent
        | DataValue::Undefined
        | DataValue::Null
        | DataValue::Bool(_)
        | DataValue::Number(_) => 8,
    }
}

fn validate_event(event: &Event) -> Result<(), DataModelError> {
    bounded(event.name.as_str(), scxml_types::MAX_NAME_BYTES)?;
    if let Some(value) = event.send_id.as_ref() {
        bounded(value.as_str(), scxml_types::MAX_NAME_BYTES)?;
    }
    if let Some(value) = event.origin.as_deref() {
        bounded(value, MAX_VALUE_BYTES)?;
    }
    if let Some(value) = event.origin_type.as_deref() {
        bounded(value, MAX_VALUE_BYTES)?;
    }
    if let Some(value) = event.invoke_id.as_ref() {
        bounded(value.as_str(), scxml_types::MAX_NAME_BYTES)?;
    }
    if let Some(value) = event.raw.as_deref() {
        bounded(value, MAX_VALUE_BYTES)?;
    }
    validate_value(&event.data)?;
    if let Some(context) = event.trace_context.as_ref() {
        context
            .validate()
            .map_err(|_| DataModelError::InvalidInput)?;
    }
    bounded_total([event_bytes(event)?], MAX_AGGREGATE_BYTES)?;
    bounded_total(
        [
            checked_scale(event.name.as_str().len(), 6)?,
            event
                .send_id
                .as_ref()
                .map_or(Ok(0), |value| checked_scale(value.as_str().len(), 6))?,
            event
                .origin
                .as_ref()
                .map_or(Ok(0), |value| checked_scale(value.len(), 6))?,
            event
                .origin_type
                .as_ref()
                .map_or(Ok(0), |value| checked_scale(value.len(), 6))?,
            event
                .invoke_id
                .as_ref()
                .map_or(Ok(0), |value| checked_scale(value.as_str().len(), 6))?,
            checked_scale(value_bytes(&event.data), 6)?,
            160,
        ],
        MAX_AGGREGATE_BYTES,
    )
}

fn event_bytes(event: &Event) -> Result<usize, DataModelError> {
    checked_sum([
        event.name.as_str().len(),
        event
            .send_id
            .as_ref()
            .map_or(0, |value| value.as_str().len()),
        event.origin.as_ref().map_or(0, String::len),
        event.origin_type.as_ref().map_or(0, String::len),
        event
            .invoke_id
            .as_ref()
            .map_or(0, |value| value.as_str().len()),
        value_bytes(&event.data),
        event.trace_context.as_ref().map_or(0, |context| {
            context.traceparent.len()
                + context.tracestate.as_ref().map_or(0, String::len)
                + context.baggage.as_ref().map_or(0, String::len)
        }),
    ])
}

struct BoundedBytes {
    bytes: Vec<u8>,
    max: usize,
}

impl BoundedBytes {
    fn new(max: usize) -> Self {
        Self {
            bytes: Vec::new(),
            max,
        }
    }
}

impl Write for BoundedBytes {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let next = self
            .bytes
            .len()
            .checked_add(buffer.len())
            .ok_or_else(|| io::Error::other("bounded serialization overflow"))?;
        if next > self.max {
            return Err(io::Error::other("bounded serialization limit exceeded"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn quote_bounded(value: &str) -> Result<String, DataModelError> {
    bounded(value, MAX_VALUE_BYTES)?;
    let mut output = BoundedBytes::new(MAX_AGGREGATE_BYTES);
    serde_json::to_writer(&mut output, value).map_err(|_| DataModelError::UnsupportedValue)?;
    String::from_utf8(output.bytes).map_err(|_| DataModelError::UnsupportedValue)
}

fn validate_system(value: &SystemContext) -> Result<(), DataModelError> {
    if value.session_id.is_empty() || value.machine_name.as_ref().is_some_and(String::is_empty) {
        return Err(DataModelError::InvalidInput);
    }
    bounded(&value.session_id, MAX_VALUE_BYTES)?;
    if let Some(name) = &value.machine_name {
        bounded(name, MAX_VALUE_BYTES)?;
    }
    if !matches!(value.io_processors, DataValue::JsonObject(_)) {
        return Err(DataModelError::InvalidInput);
    }
    validate_value(&value.io_processors)
}

/// The SCXML 5.10 system variables: never a user location.
pub(crate) const SYSTEM_VARIABLES: [&str; 4] = ["_event", "_sessionid", "_name", "_ioprocessors"];

/// A user location name: any property key a program can create on the
/// global object (B.2.10 accepts any program, whose global variables
/// become data locations, SCXML 5.8), except the empty name and the system
/// variables, at most [`scxml_types::MAX_NAME_BYTES`] bytes.
pub(crate) fn validate_location_name(name: &str) -> Result<(), DataModelError> {
    if name.is_empty()
        || name.len() > scxml_types::MAX_NAME_BYTES
        || SYSTEM_VARIABLES.contains(&name)
    {
        Err(DataModelError::InvalidInput)
    } else {
        Ok(())
    }
}

/// A `<data>` id: any bounded name that does not begin with `_` (5.10: "A
/// conformant SCXML document MUST NOT contain ids beginning with '_' in the
/// <data> element"). B.2.1: "the SCXML Processor MUST create an ECMAScript
/// variable object whose name is the value of the 'id' attribute of
/// <data>", so an XML ID that is not an ECMAScript identifier
/// (`retry-count`, `class`) is declared too, as a property of the global
/// (variable) object ([`EcmaScriptDataModel::initialize_data`]).
pub(crate) fn validate_data_id(name: &str) -> Result<(), DataModelError> {
    if name.is_empty()
        || name.starts_with('_')
        || name.contains('\0')
        || name.len() > scxml_types::MAX_NAME_BYTES
    {
        Err(DataModelError::InvalidInput)
    } else {
        Ok(())
    }
}

/// Bounds one expression or script text and rejects blank or NUL-bearing
/// input. It does not inspect what the program names: removed built-ins
/// are enforced by the realm ([`BOOTSTRAP`]), so a guard such as
/// `endDate > 0` or `_event.name == 'evaluate'` evaluates faithfully.
fn validate_program(value: &str, max: usize) -> Result<(), DataModelError> {
    bounded(value, max)?;
    if nesting::is_blank(value) || value.as_bytes().contains(&0) {
        return Err(DataModelError::InvalidInput);
    }
    Ok(())
}

/// Returns `source` once [`check_nesting`] accepts it: every program text
/// handed to Boa passes through here.
fn checked_source(source: String) -> Result<String, DataModelError> {
    check_nesting(&source).map(|()| source)
}

/// Runs a stateless parse ([`check_syntax`]) where native recursion is
/// safe.
///
/// Native builds run `work` on a dedicated thread with a
/// [`PARSE_STACK_BYTES`] stack (it emits no span of its own; the caller's
/// operation span covers it); a thread that cannot start or that panics
/// fails closed with [`DataModelError::Execution`]. The verdict never
/// depends on the thread: every structural bound is checked before `work`
/// runs. Data-model operations run on the instance's worker instead
/// (`session.rs`).
#[cfg(not(target_arch = "wasm32"))]
fn on_parse_stack<T: Send>(
    span: &FunctionSpan,
    work: impl FnOnce() -> Result<T, DataModelError> + Send,
) -> Result<T, DataModelError> {
    let result = std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("scxml-ecmascript".to_owned())
            .stack_size(PARSE_STACK_BYTES)
            .spawn_scoped(scope, work)
            .map_err(|_| DataModelError::Execution)?
            .join()
            .map_err(|_| DataModelError::Execution)?
    });
    if result.is_err() {
        span.event(SignalEvent::InputRejected);
    }
    result
}

/// The Wasm component is single-threaded: `work` runs in place, bounded by
/// the structural checks alone (a stack exhaustion traps the component).
#[cfg(target_arch = "wasm32")]
fn on_parse_stack<T>(
    span: &FunctionSpan,
    work: impl FnOnce() -> Result<T, DataModelError>,
) -> Result<T, DataModelError> {
    let result = work();
    if result.is_err() {
        span.event(SignalEvent::InputRejected);
    }
    result
}

/// Deepest `[`/`{` nesting of canonical JSON text, stopping once it exceeds
/// `max` (string contents are skipped).
fn json_depth_exceeds(json: &str, max: usize) -> bool {
    json_depth_scan(json, max)
}

fn json_depth_scan(json: &str, max: usize) -> bool {
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
    for byte in json.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth = depth.saturating_add(1);
                if depth > max {
                    return true;
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

fn validate_value(value: &DataValue) -> Result<(), DataModelError> {
    match value {
        DataValue::String(v) | DataValue::Xml(v) => bounded(v, MAX_VALUE_BYTES),
        DataValue::JsonArray(v) => bounded(v, MAX_VALUE_BYTES).and_then(|()| {
            if !json_depth_exceeds(v, MAX_NESTING_DEPTH)
                && DataValue::canonical_json_array(v) == Ok(DataValue::JsonArray(v.clone()))
            {
                Ok(())
            } else {
                Err(DataModelError::InvalidInput)
            }
        }),
        DataValue::JsonObject(v) => bounded(v, MAX_VALUE_BYTES).and_then(|()| {
            if !json_depth_exceeds(v, MAX_NESTING_DEPTH)
                && DataValue::canonical_json_object(v) == Ok(DataValue::JsonObject(v.clone()))
            {
                Ok(())
            } else {
                Err(DataModelError::InvalidInput)
            }
        }),
        _ => Ok(()),
    }
}

fn from_json(value: &serde_json::Value) -> Result<DataValue, DataModelError> {
    match value {
        serde_json::Value::Null => Ok(DataValue::Null),
        serde_json::Value::Bool(v) => Ok(DataValue::Bool(*v)),
        serde_json::Value::Number(v) => v
            .as_f64()
            .ok_or(DataModelError::UnsupportedValue)
            .and_then(|v| {
                FiniteNumber::new(v)
                    .map(DataValue::Number)
                    .map_err(|_| DataModelError::UnsupportedValue)
            }),
        serde_json::Value::String(v) => {
            bounded(v, MAX_VALUE_BYTES).map(|()| DataValue::String(v.clone()))
        }
        serde_json::Value::Array(_) => serde_json::to_string(value)
            .map_err(|_| DataModelError::UnsupportedValue)
            .and_then(|value| {
                bounded(&value, MAX_VALUE_BYTES)?;
                Ok(DataValue::JsonArray(value))
            }),
        serde_json::Value::Object(_) => serde_json::to_string(value)
            .map_err(|_| DataModelError::UnsupportedValue)
            .and_then(|value| {
                bounded(&value, MAX_VALUE_BYTES)?;
                Ok(DataValue::JsonObject(value))
            }),
    }
}

fn to_source(value: &DataValue) -> Result<String, DataModelError> {
    validate_value(value)?;
    let source = match value {
        DataValue::Absent => Ok("({\"$scxml\":\"absent\"})".into()),
        DataValue::Undefined => Ok("void 0".into()),
        DataValue::Null => Ok("null".into()),
        DataValue::Bool(v) => Ok(v.to_string()),
        DataValue::Number(v) => Ok(v.get().to_string()),
        DataValue::String(v) => quote_bounded(v),
        DataValue::JsonArray(v) => Ok(v.clone()),
        DataValue::JsonObject(v) => Ok(format!("({v})")),
        DataValue::Xml(v) => dom::dom_source(v),
    }?;
    bounded(&source, MAX_AGGREGATE_BYTES)?;
    Ok(source)
}

fn event_to_source(event: &Event) -> Result<String, DataModelError> {
    validate_event(event)?;
    let optional = |v: Option<&str>| v.map_or_else(|| Ok("undefined".into()), quote_bounded);
    let source = format!(
        "({{name:{},type:{},sendid:{},origin:{},origintype:{},invokeid:{},data:{},raw:{}}})",
        quote_bounded(event.name.as_str())?,
        quote_bounded(match event.event_type {
            scxml_types::EventType::Internal => "internal",
            scxml_types::EventType::External => "external",
            scxml_types::EventType::Platform => "platform",
        })?,
        optional(event.send_id.as_ref().map(scxml_types::SendId::as_str))?,
        optional(event.origin.as_deref())?,
        optional(event.origin_type.as_deref())?,
        optional(event.invoke_id.as_ref().map(scxml_types::InvokeId::as_str))?,
        // B.2.8: a field with no value "MUST" be ECMAScript undefined, so an
        // event without a payload has `data` undefined.
        match &event.data {
            DataValue::Absent => Ok("undefined".into()),
            data => to_source(data),
        }?,
        // `_event.raw`: the delivering I/O processor's message text
        // (BasicHTTP), undefined when the processor retains none.
        optional(event.raw.as_deref())?
    );
    bounded(&source, MAX_AGGREGATE_BYTES)?;
    Ok(source)
}

/// The variable a location expression starts from: its leading identifier
/// when the location is that identifier or a member access (`.` or `[`)
/// on it. `None` for a location that starts elsewhere (`(a).b`, `f().x`,
/// `this.x`): that location is evaluated as it is written.
///
/// SCXML 5.4: "If the location expression does not denote a valid
/// location in the data model ... the SCXML Processor MUST place the error
/// 'error.execution'": a leading identifier must be a data location (a
/// declared variable, never a system variable or a built-in global).
fn location_root(location: &str) -> Option<&str> {
    let trimmed = location.trim_start();
    let end = trimmed
        .char_indices()
        .find_map(|(i, c)| {
            let identifier = if i == 0 {
                is_identifier_start(c)
            } else {
                is_identifier_part(c)
            };
            (!identifier).then_some(i)
        })
        .unwrap_or(trimmed.len());
    let root = trimmed.get(..end)?;
    let rest = trimmed.get(end..)?.trim_start();
    let starts_identifier = !root.is_empty();
    let operand = !matches!(
        root,
        "this"
            | "new"
            | "super"
            | "null"
            | "true"
            | "false"
            | "function"
            | "class"
            | "void"
            | "typeof"
            | "delete"
            | "await"
            | "yield"
            | "import"
    );
    (starts_identifier && operand && (rest.is_empty() || rest.starts_with(['.', '['])))
        .then_some(root)
}

/// ECMAScript `IdentifierStart` (ECMA-262 §12.7): `$`, `_`, or a code point
/// with the Unicode property `ID_Start`, as Boa's lexer reads it.
fn is_identifier_start(c: char) -> bool {
    const ID_START: icu_properties::CodePointSetDataBorrowed<'static> =
        icu_properties::CodePointSetData::new::<icu_properties::props::IdStart>();
    matches!(c, '$' | '_') || ID_START.contains(c)
}

/// ECMAScript `IdentifierPart` (ECMA-262 §12.7): `$`, `_`, ZWNJ, ZWJ, or a
/// code point with the Unicode property `ID_Continue`, as Boa's lexer reads it.
fn is_identifier_part(c: char) -> bool {
    const ID_CONTINUE: icu_properties::CodePointSetDataBorrowed<'static> =
        icu_properties::CodePointSetData::new::<icu_properties::props::IdContinue>();
    matches!(c, '$' | '_' | '\u{200c}' | '\u{200d}') || ID_CONTINUE.contains(c)
}

/// Classifies one Boa evaluation failure.
///
/// An illegal expression (`SyntaxError`) and the ECMAScript runtime errors a
/// legal expression raises over the current data (`ReferenceError` for an
/// unbound identifier, `TypeError` for a member access on `undefined` or a
/// call of a non-function) are SCXML `error.execution` (5.9, B.2; official
/// case 245). Any other failure — a `RangeError` from a bounded built-in,
/// runtime-limit exhaustion, a thrown non-error value, or a host capability
/// gap — stays outside the supported slice and fails closed.
fn classify_js_error(error: &boa_engine::JsError) -> DataModelError {
    if error.as_native().is_some_and(|native| {
        matches!(
            native.kind(),
            boa_engine::JsNativeErrorKind::Syntax
                | boa_engine::JsNativeErrorKind::Reference
                | boa_engine::JsNativeErrorKind::Type
        )
    }) {
        DataModelError::Execution
    } else {
        DataModelError::UnsupportedValue
    }
}

/// The program text of a function that assigns its one argument to
/// `location` (an arrow, so `this` and `arguments` are the location's own).
/// The parameter is named `v`, then `v0`, `v1`, ...: the first name that
/// does not occur anywhere in `location`, so the location cannot refer to
/// it.
///
/// The function is strict code, so an assignment that cannot be made
/// throws instead of being silently skipped: an unresolvable name is a
/// `ReferenceError`, and a read-only or non-extensible target a `TypeError`
/// (B.2.7: "If it is unable to do so ... it MUST place the error
/// error.execution on the internal event queue"). A text that is not a
/// valid assignment target is a `SyntaxError`.
fn assignment_function(location: &str) -> String {
    let mut parameter = String::from("v");
    let mut suffix = 0_usize;
    while location.contains(parameter.as_str()) {
        parameter = format!("v{suffix}");
        suffix = suffix.saturating_add(1);
    }
    format!("(({parameter}) => {{ \"use strict\"; {location} = {parameter}; }})")
}

/// The checked program text of the function that assigns its one argument
/// to `location` ([`assignment_function`]): `location` is at most
/// [`MAX_EXPRESSION_BYTES`], not blank, without NUL, and passes
/// [`check_nesting`] on its own.
fn checked_location(location: &str) -> Result<String, DataModelError> {
    bounded(location, MAX_EXPRESSION_BYTES)?;
    if nesting::is_blank(location) || location.as_bytes().contains(&0) {
        return Err(DataModelError::InvalidInput);
    }
    check_nesting(location)?;
    checked_source(assignment_function(location))
}

/// SCXML value expressions are parenthesized before evaluation so function
/// expressions evaluate as values; a single trailing statement separator
/// inside the attribute (e.g. `new Test();`) is tolerated as one expression.
fn value_expression_source(source: &str) -> String {
    let trimmed = nesting::trim(source);
    if let Some(stripped) = trimmed.strip_suffix(';') {
        let head = nesting::trim(stripped);
        if !head.is_empty()
            && !["for", "if", "while", "do", "switch", "try"]
                .iter()
                .any(|keyword| head.starts_with(keyword))
        {
            return format!("({head})");
        }
    }
    format!("({source})")
}

mod dom;
pub mod effects;
pub mod nesting;
mod operations;
mod session;

pub use operations::{
    ForeachItems, InlineContent, event_data_field, event_payload, external_data, inline_content,
    namelist_key,
};

/// The value of fetched or inline data `text` when it is an XML document
/// (SCXML B.2.1: the processor creates the corresponding DOM): exactly one
/// well-formed element of at most [`MAX_VALUE_BYTES`] plus optional
/// comments, processing instructions, and XML white space (XML 1.0 §2.3
/// `S`: #x20, #x9, #xD, #xA). `None` otherwise, so the caller applies the
/// JSON-or-string rules.
#[must_use]
pub fn xml_document_value(text: &str) -> Option<DataValue> {
    (text.len() <= MAX_VALUE_BYTES && dom::is_document(text))
        .then(|| DataValue::Xml(text.to_owned()))
}
pub mod syntax;

pub use effects::{
    Certainty, EffectCause, EffectFinding, EffectFragment, EffectKind, EffectRole, EffectSpan,
    check_effects,
};
pub use nesting::{MAX_PARSE_NESTING, check_nesting};

pub use syntax::{MAX_SYNTAX_MESSAGE_BYTES, SyntaxIssue, SyntaxKind, check_syntax};

#[cfg(target_arch = "wasm32")]
mod guest;

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::Source;
    use boa_engine::context::Clock;
    use boa_engine::context::ContextBuilder;
    use boa_engine::context::time::JsInstant;
    use scxml_telemetry::{Recorder, install_recorder};
    use scxml_types::EventName;
    use std::rc::Rc;
    /// Test host whose active configuration is either exactly `ready` or
    /// empty.
    #[derive(Clone, Copy, PartialEq)]
    enum Host {
        InReady,
        Inactive,
    }
    impl ExecutionHost for Host {
        fn in_state(&mut self, id: &str) -> Result<bool, DataModelError> {
            Ok(*self == Self::InReady && id == "ready")
        }
    }
    #[test]
    fn contexts_observe_the_session_clock_not_host_time() {
        assert_eq!(session::SessionClock.now(), JsInstant::new(0, 0));
        // Outside an operation no session time is known: the epoch.
        assert_eq!(session::SessionClock.system_time_millis(), 0);
        assert_eq!(
            BoundedHostHooks.local_timezone_offset_seconds(1_700_000_000),
            0
        );
    }

    #[test]
    fn host_hooks_bound_string_compilation_on_every_path() {
        let mut context = ContextBuilder::new()
            .host_hooks(Rc::new(BoundedHostHooks))
            .clock(Rc::new(session::SessionClock))
            .build()
            .expect("bare bounded context builds");
        let deep = "[".repeat(crate::MAX_PARSE_NESTING + 8);
        for compile in [
            "Function('return BODY')",
            "new Function('a', 'return BODY')",
            "(function* () {}).constructor('yield BODY')",
            "Object.getPrototypeOf(async function () {}).constructor('return BODY')",
            "Object.getPrototypeOf(async function* () {}).constructor('yield BODY')",
            "(0, eval)('BODY')",
            "eval('BODY')",
        ] {
            for (body, expected) in [("1", "compiled"), (deep.as_str(), "denied")] {
                let source = compile.replace("BODY", body);
                let outcome = context
                    .eval(Source::from_bytes(&format!(
                        "(() => {{ try {{ {source}; return 'compiled'; }} \
                         catch (error) {{ return error instanceof TypeError ? 'denied' : error.name; }} }})()"
                    )))
                    .expect("probe script runs");
                assert_eq!(
                    outcome.as_string().map(|text| text.to_std_string_escaped()),
                    Some(expected.to_owned()),
                    "{source}"
                );
            }
        }
    }

    struct FailingHost;
    impl ExecutionHost for FailingHost {
        fn in_state(&mut self, _id: &str) -> Result<bool, DataModelError> {
            Err(DataModelError::Host)
        }
    }
    fn system() -> SystemContext {
        SystemContext {
            session_id: "s1".into(),
            machine_name: Some("m".into()),
            io_processors: DataValue::JsonObject("{}".into()),
        }
    }
    fn event() -> Event {
        Event::external(EventName::new("tick").expect("valid test event"))
    }
    fn number(value: f64) -> DataValue {
        DataValue::Number(FiniteNumber::new(value).expect("finite"))
    }

    #[test]
    fn initialize_declarations_binds_null_placeholders_then_initialize_data_values_them() {
        let mut m = EcmaScriptDataModel::new();
        m.initialize_declarations(["b", "a"], system())
            .expect("unique declarations initialize");
        assert_eq!(
            m.evaluate_value_without_event("a === null && b === null", &mut Host::Inactive),
            Ok(DataValue::Bool(true))
        );
        m.initialize_data(
            "a",
            &DataInitializer::Expression("function () { return 2; }".into()),
            &mut Host::Inactive,
        )
        .expect("a function is a legal initial value (B.2, official 453)");
        m.initialize_data(
            "b",
            &DataInitializer::Value(DataValue::String("x".into())),
            &mut Host::Inactive,
        )
        .expect("a materialized value initializes");
        assert_eq!(
            m.evaluate_value_without_event("a() + b", &mut Host::Inactive),
            Ok(DataValue::String("2x".into()))
        );
        assert_eq!(
            m.initialize_data(
                "c",
                &DataInitializer::Value(DataValue::Null),
                &mut Host::Inactive
            ),
            Err(DataModelError::InvalidInput),
            "an undeclared id is not a <data> location"
        );
        assert_eq!(
            m.initialize_declarations(["a", "a"], system()),
            Err(DataModelError::InvalidInput),
            "duplicate declarations are rejected"
        );
        assert_eq!(
            m.evaluate_value_without_event("b", &mut Host::Inactive),
            Ok(DataValue::String("x".into())),
            "a rejected initialization keeps the realm"
        );
    }

    #[test]
    fn evaluates_system_event_and_in() {
        let mut m = EcmaScriptDataModel::new();
        m.initialize(r#"{"count":1,"items":[1,2]}"#, system())
            .expect("initialize");
        let mut h = Host::InReady;
        assert_eq!(
            m.evaluate_condition(
                "count===1&&items.length===2&&_sessionid==='s1'&&In('ready')",
                &event(),
                &mut h
            ),
            Ok(true)
        );
        assert_eq!(
            m.evaluate_value("_event.name", &event(), &mut h),
            Ok(DataValue::String("tick".into()))
        );
    }

    #[test]
    fn in_answers_every_call_while_the_operation_runs() {
        let mut m = EcmaScriptDataModel::new();
        m.initialize("{}", system()).expect("initialize");
        let value = |expression: &str| m.evaluate_value(expression, &event(), &mut Host::InReady);
        assert_eq!(
            value(
                "(function () { function myIn(s) { return 'mine:' + s; } return myIn('ready'); })()"
            ),
            Ok(DataValue::String("mine:ready".into()))
        );
        assert_eq!(value("In('ready')"), Ok(DataValue::Bool(true)));
        assert_eq!(value("In('other')"), Ok(DataValue::Bool(false)));
        assert_eq!(
            value("`${In('ready')}`"),
            Ok(DataValue::String("true".into()))
        );
        // Any argument expression is evaluated like any other: an unbound
        // name is a ReferenceError, a computed one names its state.
        assert_eq!(value("In(x)"), Err(DataModelError::Execution));
        assert_eq!(
            value("(function (x) { return In(x); })('rea' + 'dy')"),
            Ok(DataValue::Bool(true))
        );
        // Asked once per id: side effects before the call run exactly once.
        m.execute_without_event("var calls = 0;", &mut Host::InReady)
            .expect("script");
        assert_eq!(
            m.evaluate_value(
                "(calls += 1, In('ready'), In('x' + calls), calls)",
                &event(),
                &mut Host::InReady
            ),
            Ok(number(1.0))
        );
    }

    #[test]
    fn events_without_send_id_expose_ecmascript_undefined() {
        let mut model = EcmaScriptDataModel::new();
        model.initialize("{}", system()).expect("initialize");
        let mut host = Host::Inactive;
        let external = Event::external(EventName::new("external").expect("valid test event"));
        let internal = Event::internal(EventName::new("internal").expect("valid test event"));
        let platform = Event {
            event_type: scxml_types::EventType::Platform,
            ..Event::external(EventName::new("platform").expect("valid test event"))
        };
        for event in [&external, &internal, &platform] {
            assert_eq!(
                model.evaluate_condition(
                    "'sendid' in _event && typeof _event.sendid === 'undefined' && typeof _event.origin === 'undefined' && typeof _event.origintype === 'undefined' && typeof _event.invokeid === 'undefined'",
                    event,
                    &mut host,
                ),
                Ok(true)
            );
        }
    }

    #[test]
    fn sent_event_exposes_bounded_send_id() {
        let mut model = EcmaScriptDataModel::new();
        model.initialize("{}", system()).expect("initialize");
        let mut host = Host::Inactive;
        let mut event = Event::external(EventName::new("delivered").expect("valid test event"));
        event.send_id =
            Some(scxml_types::SendId::new("send-42").expect("valid bounded test send id"));
        assert_eq!(
            model.evaluate_condition(
                "'sendid' in _event && _event.sendid === 'send-42'",
                &event,
                &mut host,
            ),
            Ok(true)
        );
    }

    #[test]
    fn processor_raw_message_is_exposed_as_event_raw() {
        let mut model = EcmaScriptDataModel::new();
        model.initialize("{}", system()).expect("initialize");
        let mut host = Host::Inactive;
        let mut event = Event::external(EventName::new("HTTP.POST").expect("valid test event"));
        assert_eq!(
            model.evaluate_condition(
                "'raw' in _event && _event.raw === undefined",
                &event,
                &mut host
            ),
            Ok(true)
        );
        event.raw = Some("POST /s HTTP/1.1\r\n\r\na=1".into());
        assert_eq!(
            model.evaluate_condition("_event.raw.search('POST') === 0", &event, &mut host),
            Ok(true)
        );
        event.raw = Some("x".repeat(MAX_VALUE_BYTES + 1));
        assert!(
            model.evaluate_condition("true", &event, &mut host).is_err(),
            "an over-bound raw message is rejected before evaluation"
        );
    }

    #[test]
    fn explicit_no_event_evaluation_exposes_no_event_fields() {
        let mut model = EcmaScriptDataModel::new();
        model
            .initialize(r#"{"count":1}"#, system())
            .expect("initialize no-event model");
        let mut host = Host::InReady;
        assert_eq!(
            model.evaluate_condition_without_event(
                "_event === undefined && typeof _event === 'undefined' && In('ready')",
                &mut host,
            ),
            Ok(true)
        );
        assert_eq!(
            model.evaluate_value_without_event("count", &mut host),
            Ok(number(1.0))
        );
    }

    #[test]
    fn script_failures_keep_the_effects_made_before_them() {
        // ECMAScript: an abrupt completion does not undo the assignments a
        // script made before it; SCXML 5.8 raises error.execution.
        let mut model = EcmaScriptDataModel::new();
        model
            .initialize(r#"{"count":1}"#, system())
            .expect("initialize no-event model");
        let mut host = Host::Inactive;
        model
            .execute_without_event(
                "if (_event !== undefined) { throw new Error(); } count=count+1;",
                &mut host,
            )
            .expect("execute with no current event");
        assert_eq!(
            model.evaluate_value_without_event("count", &mut host),
            Ok(number(2.0))
        );
        assert_eq!(
            model.execute_without_event("count=9;null.x;", &mut host),
            Err(DataModelError::Execution)
        );
        // Our loop-iteration limit applies to wasm builds only: a loop one
        // iteration past it completes natively. The host's component tests
        // check that it fails in the shipped data model
        // (`a_script_over_the_ecmascript_loop_or_stack_limit_raises_error_execution`).
        let loop_past_the_wasm_limit = "for (let i = 0; i < 100001; i++) {} count=10;";
        model
            .execute_without_event(loop_past_the_wasm_limit, &mut host)
            .expect("a native operation has no loop-iteration limit");
        assert_eq!(
            model.evaluate_value_without_event("count", &mut host),
            Ok(number(10.0))
        );
        model
            .execute_without_event("count=9;", &mut host)
            .expect("reset count");
        // Likewise our value-stack limit (16,384): a call passing 20,000
        // arguments completes natively.
        let wide_call = format!(
            "count = (function () {{ return arguments.length; }})({});",
            vec!["0"; 20_000].join(",")
        );
        model
            .execute_without_event(&wide_call, &mut host)
            .expect("a native operation has no value-stack limit");
        assert_eq!(
            model.evaluate_value_without_event("count", &mut host),
            Ok(number(20_000.0))
        );
        model
            .execute_without_event("count=9;", &mut host)
            .expect("reset count");
        assert_eq!(
            model.evaluate_value_without_event("count", &mut host),
            Ok(number(9.0))
        );
        // NUL is invalid input on every build (byte bounds apply to wasm only).
        let invalid_expression = "x\0".to_owned();
        assert_eq!(
            model.evaluate_condition_without_event(&invalid_expression, &mut host),
            Err(DataModelError::InvalidInput)
        );
        assert_eq!(
            model.evaluate_condition_without_event("In('ready')", &mut FailingHost),
            Err(DataModelError::Host)
        );
        // A host failure fails the operation even when the program catches it.
        assert_eq!(
            model.evaluate_condition_without_event(
                "(() => { try { return In('ready'); } catch (e) { return true; } })()",
                &mut FailingHost
            ),
            Err(DataModelError::Host)
        );
    }

    #[test]
    fn explicit_no_event_operations_emit_bounded_otel_outcomes() {
        let recorder = Recorder::default();
        let _guard = install_recorder(recorder.clone());
        let mut model = EcmaScriptDataModel::new();
        model
            .initialize(r#"{"count":1}"#, system())
            .expect("initialize telemetry model");
        let mut host = Host::Inactive;
        model
            .evaluate_condition_without_event("count === 1", &mut host)
            .expect("evaluate no-event condition");
        model
            .evaluate_value_without_event("count", &mut host)
            .expect("evaluate no-event value");
        model
            .execute_without_event("count=count+1", &mut host)
            .expect("execute no-event script");
        // NUL is invalid input on every build (byte bounds apply to wasm only).
        let invalid_expression = "x\0".to_owned();
        assert_eq!(
            model.evaluate_condition_without_event(&invalid_expression, &mut host),
            Err(DataModelError::InvalidInput)
        );
        assert_eq!(
            model.evaluate_condition_without_event("In('ready')", &mut FailingHost),
            Err(DataModelError::Host)
        );

        let observations = recorder.observations();
        for operation in [
            Operation::DataModelEvaluateCondition,
            Operation::DataModelEvaluateValue,
            Operation::DataModelExecute,
        ] {
            assert!(observations.iter().any(|observation| {
                observation.operation == operation
                    && observation.error.is_none()
                    && observation.events == vec![SignalEvent::TransformationCompleted]
            }));
        }
        assert!(observations.iter().any(|observation| {
            observation.operation == Operation::DataModelEvaluateCondition
                && observation.error == Some(ErrorCategory::InvalidInput)
                && observation.events == vec![SignalEvent::InputRejected]
        }));
        assert!(observations.iter().any(|observation| {
            observation.operation == Operation::DataModelEvaluateCondition
                && observation.error == Some(ErrorCategory::Platform)
                && observation.events == vec![SignalEvent::InputRejected]
        }));
    }

    /// The data-initialization boundaries (`declare`, `initialize-data`)
    /// each record one `initialize` observation, whether the id is an
    /// ECMAScript identifier or a global-object property, and no inner
    /// helper records an `assign` of its own.
    #[test]
    fn data_initialization_records_one_initialize_observation_per_call() {
        let recorder = Recorder::default();
        let _guard = install_recorder(recorder.clone());
        let mut model = EcmaScriptDataModel::new();
        model
            .initialize_declarations(["count", "retry-count"], system())
            .expect("declarations initialize");
        let before = recorder.observations().len();
        let mut host = Host::Inactive;
        model.declare("retry-count").expect("declared");
        model.declare("count").expect("declared");
        for id in ["retry-count", "count"] {
            for initializer in [
                DataInitializer::Value(DataValue::Bool(true)),
                DataInitializer::Expression("1 + 1".into()),
            ] {
                model
                    .initialize_data(id, &initializer, &mut host)
                    .expect("initialized");
            }
        }
        let observed: Vec<Operation> = recorder.observations()[before..]
            .iter()
            .map(|observation| observation.operation)
            .collect();
        assert_eq!(observed, [Operation::DataModelInitialize; 6]);
    }

    /// Blank program text is ECMAScript white space and line terminators only
    /// (ECMA-262 §12.2, §12.3): U+FEFF is blank, U+0085 is not, so it
    /// reaches the parser, which rejects it.
    #[test]
    fn blank_program_text_is_ecmascript_white_space_only() {
        const ECMASCRIPT_BLANK: &str = "\u{FEFF}\u{2028} \u{3000}";
        const NEXT_LINE: &str = "\u{85}";
        let mut m = EcmaScriptDataModel::new();
        m.initialize(r#"{"count":2}"#, system())
            .expect("initialize");
        let mut h = Host::Inactive;
        assert_eq!(
            m.evaluate_log(ECMASCRIPT_BLANK, &event(), &mut h),
            Ok(String::new())
        );
        assert_eq!(
            m.evaluate_log_without_event(ECMASCRIPT_BLANK, &mut h),
            Ok(String::new())
        );
        assert_eq!(
            m.evaluate_log(NEXT_LINE, &event(), &mut h),
            Err(DataModelError::Execution)
        );
        assert_eq!(
            m.evaluate_log_without_event(NEXT_LINE, &mut h),
            Err(DataModelError::Execution)
        );
        assert_eq!(
            m.evaluate_value(ECMASCRIPT_BLANK, &event(), &mut h),
            Err(DataModelError::InvalidInput)
        );
        assert_eq!(
            m.evaluate_value(NEXT_LINE, &event(), &mut h),
            Err(DataModelError::Execution)
        );
        assert_eq!(
            m.evaluate_value(&format!("count;{NEXT_LINE}"), &event(), &mut h),
            Err(DataModelError::Execution)
        );
        assert_eq!(
            m.evaluate_value(&format!("count{NEXT_LINE};"), &event(), &mut h),
            Err(DataModelError::Execution)
        );
        assert_eq!(
            m.evaluate_value(
                &format!("{ECMASCRIPT_BLANK}count;{ECMASCRIPT_BLANK}"),
                &event(),
                &mut h
            ),
            Ok(number(2.0))
        );
        assert_eq!(
            m.assign(ECMASCRIPT_BLANK, &number(1.0)),
            Err(DataModelError::InvalidInput)
        );
    }

    #[test]
    fn log_text_renders_every_value_deterministically() {
        let mut m = EcmaScriptDataModel::new();
        m.initialize(r#"{"count":2}"#, system())
            .expect("initialize");
        let mut h = Host::Inactive;
        let mut log = |expression: &str| m.evaluate_log(expression, &event(), &mut h);
        assert_eq!(log("'pass'"), Ok("pass".into()));
        assert_eq!(log(""), Ok(String::new()));
        assert_eq!(log("  "), Ok(String::new()));
        assert_eq!(log("undefined"), Ok("undefined".into()));
        assert_eq!(log("null"), Ok("null".into()));
        assert_eq!(log("count + 1"), Ok("3".into()));
        assert_eq!(log("count === 2"), Ok("true".into()));
        assert_eq!(log("NaN"), Ok("NaN".into()));
        assert_eq!(
            log("({b: 1, a: [1, 'x', undefined]})"),
            Ok(r#"{"b":1,"a":[1,"x",null]}"#.into())
        );
        // `_event` members that SCXML leaves unbound are omitted.
        assert_eq!(
            log("_event"),
            Ok(r#"{"name":"tick","type":"external"}"#.into())
        );
        assert_eq!(log("_event.sendid"), Ok("undefined".into()));
        assert_eq!(log("_event.data"), Ok("undefined".into()));
        // Values JSON cannot render use their fixed built-in tag; user
        // `toString` overrides never run.
        assert_eq!(log("(function f() {})"), Ok("[object Function]".into()));
        assert_eq!(
            log("(() => { const o = { toString() { return 'user'; } }; o.self = o; return o; })()"),
            Ok("[object Object]".into())
        );
        assert_eq!(log("missing"), Err(DataModelError::Execution));
        // Replaced globals never reach the rendering itself.
        m.execute_without_event(
            "String = function () { return 'hijacked'; }; Object.isFrozen = function () { return true; };",
            &mut h,
        )
        .expect("script");
        assert_eq!(
            m.evaluate_log("[count, null]", &event(), &mut h),
            Ok("[2,null]".into())
        );
        assert_eq!(m.evaluate_log("true", &event(), &mut h), Ok("true".into()));
        assert_eq!(
            m.evaluate_log_without_event("_event", &mut h),
            Ok("undefined".into())
        );
        assert_eq!(
            m.evaluate_log(
                &format!("'x'.repeat({MAX_VALUE_BYTES}) + 'y'"),
                &event(),
                &mut h
            ),
            Err(DataModelError::InvalidInput)
        );
    }

    #[test]
    fn runtime_reference_and_type_errors_are_recoverable_execution_errors() {
        // SCXML 5.9/B.2: an expression that raises an ECMAScript runtime
        // error (an unbound identifier, a member read of `undefined`) is an
        // `error.execution`, not an unsupported value (official case 245).
        let mut m = EcmaScriptDataModel::new();
        m.initialize("{}", system()).expect("initialize");
        let mut h = Host::Inactive;
        assert_eq!(
            m.evaluate_condition("Var2", &event(), &mut h),
            Err(DataModelError::Execution)
        );
        assert_eq!(
            m.evaluate_condition_without_event("Var2", &mut h),
            Err(DataModelError::Execution)
        );
        assert_eq!(
            m.evaluate_value("undefined.bar", &event(), &mut h),
            Err(DataModelError::Execution)
        );
        assert_eq!(
            m.evaluate_value_without_event("missing + 1", &mut h),
            Err(DataModelError::Execution)
        );
        // Bound violations stay fail-closed unsupported values.
        assert_eq!(
            m.evaluate_value(&format!("'ab'.repeat({MAX_VALUE_BYTES})"), &event(), &mut h),
            Err(DataModelError::UnsupportedValue)
        );
    }

    #[test]
    fn assigned_values_keep_their_identity_closures_and_prototypes() {
        // SCXML B.2: any ECMAScript expression is a value expression,
        // including a function assigned to a data location (official 453),
        // and one realm keeps every value for the whole session.
        let mut m = EcmaScriptDataModel::new();
        m.initialize(r#"{"counter":null,"foo":0,"var1":null}"#, system())
            .expect("initialize");
        let mut h = Host::Inactive;
        m.assign_expression_without_event("var1", "function(invar) {return invar + 1;}", &mut h)
            .expect("a function is an assignable value");
        assert_eq!(
            m.evaluate_condition("var1(2) == 3", &event(), &mut h),
            Ok(true)
        );
        m.execute_without_event(
            "function testobject() { this.bar = 0; } \
             class Point { constructor(x) { this.x = x; } get double() { return this.x * 2; } } \
             counter = (() => { let n = 0; return () => ++n; })();",
            &mut h,
        )
        .expect("script");
        m.assign_expression("foo", "new testobject();", &event(), &mut h)
            .expect("an instance of a script constructor is assignable");
        m.assign_expression("foo.bar", "1", &event(), &mut h)
            .expect("a member of the instance is assignable");
        for condition in [
            "foo.bar == 1 && foo instanceof testobject",
            "new Point(2).double === 4",
            "counter() === 1 && counter() === 2",
        ] {
            assert_eq!(
                m.evaluate_condition(condition, &event(), &mut h),
                Ok(true),
                "{condition}"
            );
        }
        assert_eq!(
            m.evaluate_value("counter()", &event(), &mut h),
            Ok(number(3.0)),
            "a closure keeps its captured scope across operations"
        );
        // The instance has no portable value to hand out.
        assert_eq!(
            m.evaluate_value("foo", &event(), &mut h),
            Err(DataModelError::UnsupportedValue)
        );
        // A changed built-in stays changed, as ECMAScript defines.
        m.execute_without_event("Array.prototype.total = function () { return 7; };", &mut h)
            .expect("script");
        assert_eq!(
            m.evaluate_value("[].total()", &event(), &mut h),
            Ok(number(7.0))
        );
    }

    #[test]
    fn snapshot_shows_each_declared_id_in_document_order_and_changes_nothing() {
        assert_eq!(
            EcmaScriptDataModel::new().snapshot(),
            Err(DataModelError::NotInitialized)
        );
        let mut m = EcmaScriptDataModel::new();
        m.initialize_declarations(["zed", "Big", "foo", "when", "odd"], system())
            .expect("initialize");
        let mut h = Host::Inactive;
        m.execute_without_event(
            "function testobject() { this.bar = 0; } \
             zed = 1; Big = 'x'; foo = new testobject(); when = new Date(0); odd = NaN; \
             var scripted = 1;",
            &mut h,
        )
        .expect("script");
        let image = m.snapshot().expect("snapshot");
        assert_eq!(m.snapshot(), Ok(image.clone()), "deterministic");
        assert_eq!(image.system, system());
        let shown: Vec<(&str, &VariableValue)> = image
            .variables
            .iter()
            .map(|variable| (variable.name.as_str(), &variable.value))
            .collect();
        let text = |kind: &str, text: &str| VariableValue::Text {
            kind: kind.into(),
            text: text.into(),
        };
        assert_eq!(
            shown,
            [
                ("zed", &VariableValue::Value(number(1.0))),
                ("Big", &VariableValue::Value(DataValue::String("x".into()))),
                ("foo", &text("object", "[object Object]")),
                ("when", &text("object", "Thu Jan 01 1970 00:00:00 GMT+0000")),
                ("odd", &text("number", "NaN")),
            ],
            "declared ids only, in document order"
        );
        assert_eq!(
            m.evaluate_value_without_event("zed", &mut h),
            Ok(number(1.0)),
            "a snapshot changes nothing"
        );
    }

    #[test]
    fn a_snapshot_shows_functions_by_their_source_and_never_runs_user_code() {
        let mut m = EcmaScriptDataModel::new();
        m.initialize_declarations(["f", "o"], system())
            .expect("initialize");
        let mut h = Host::Inactive;
        m.execute_without_event(
            "var calls = 0; f = function twice(n) { return n * 2; }; \
             o = { toJSON() { calls += 1; return 1; }, toString() { calls += 1; return 'user'; } }; \
             Object.defineProperty(o, 'g', { get() { calls += 1; return 1; }, enumerable: true });",
            &mut h,
        )
        .expect("script");
        let image = m.snapshot().expect("snapshot");
        assert_eq!(
            image.variables[0].value,
            VariableValue::Text {
                kind: "function".into(),
                text: "function twice(n) { return n * 2; }".into()
            }
        );
        assert_eq!(
            image.variables[1].value,
            VariableValue::Text {
                kind: "object".into(),
                text: "[object Object]".into()
            }
        );
        assert_eq!(
            m.evaluate_value_without_event("calls", &mut h),
            Ok(number(0.0))
        );
    }

    #[test]
    fn a_text_longer_than_one_data_value_is_reported_oversized() {
        let mut m = EcmaScriptDataModel::new();
        m.initialize_declarations(["f"], system())
            .expect("initialize");
        let mut h = Host::Inactive;
        let text = |bytes: usize| format!("Symbol({})", "x".repeat(bytes));
        let at_bound = MAX_VALUE_BYTES - text(0).len();
        m.assign_expression_without_event("f", &format!("Symbol('x'.repeat({at_bound}))"), &mut h)
            .expect("assign");
        let shown =
            |m: &EcmaScriptDataModel| m.snapshot().expect("snapshot").variables[0].value.clone();
        assert_eq!(
            shown(&m),
            VariableValue::Text {
                kind: "symbol".into(),
                text: text(at_bound)
            }
        );
        m.assign_expression_without_event(
            "f",
            &format!("Symbol('x'.repeat({at_bound}) + 'x')"),
            &mut h,
        )
        .expect("assign");
        assert_eq!(
            shown(&m),
            VariableValue::Oversized(u64::try_from(MAX_VALUE_BYTES + 1).expect("fits"))
        );
    }

    #[test]
    fn captured_system_values_keep_their_identity() {
        // SCXML 5.10: system variables are read-only, and a copy compares
        // equal to the variable it was read from (official 329).
        let mut m = EcmaScriptDataModel::new();
        m.initialize(r#"{"Var2":null,"Var4":null}"#, system())
            .expect("initialize");
        let mut h = Host::Inactive;
        m.assign_expression("Var2", "_event", &event(), &mut h)
            .expect("the current event object is assignable");
        m.assign_expression("Var4", "_ioprocessors", &event(), &mut h)
            .expect("_ioprocessors is assignable");
        for system_location in ["_event", "_sessionid", "_name", "_ioprocessors"] {
            assert_eq!(
                m.assign_expression(system_location, "27", &event(), &mut h),
                Err(DataModelError::Execution),
                "{system_location} is read-only"
            );
        }
        assert_eq!(
            m.evaluate_condition(
                "Var2 == _event && 'sendid' in Var2 && Var4 == _ioprocessors",
                &event(),
                &mut h
            ),
            Ok(true)
        );
        let other = Event::external(EventName::new("tock").expect("valid test event"));
        assert_eq!(
            m.evaluate_condition("Var2 == _event", &other, &mut h),
            Ok(false)
        );
        assert_eq!(
            m.evaluate_condition("Var2.name === 'tick'", &other, &mut h),
            Ok(true)
        );
    }

    const BOOKS: &str = r#"<books xmlns=""><book title="title1"/><!--c--><book title="title2">two &amp; <i>more</i></book></books>"#;
    #[test]
    fn xml_values_are_read_only_dom_documents() {
        // SCXML B.2.1/B.2.8.1: XML content in the ECMAScript data model is a DOM
        // (official cases 557 and 561).
        let mut m = EcmaScriptDataModel::new();
        m.initialize(r#"{"copy":null,"var1":null}"#, system())
            .expect("initialize");
        let mut h = Host::Inactive;
        let xml = DataValue::Xml(BOOKS.into());
        m.assign("var1", &xml).expect("an XML value is assignable");
        for condition in [
            "var1.getElementsByTagName('book')[0].getAttribute('title') == 'title1'",
            "var1.getElementsByTagName('book')[1].getAttribute('title') == 'title2'",
            "var1.nodeType === 9 && var1.nodeName === '#document'",
            "var1.documentElement.nodeName === 'books' && var1.documentElement.namespaceURI === null",
            "var1.documentElement.childNodes.length === 3 && var1.documentElement.childNodes[1].nodeType === 8",
            "var1.getElementsByTagName('*').length === 4",
            "var1.getElementsByTagName('book')[1].textContent === 'two & more'",
            "var1.getElementsByTagName('book')[0].getAttribute('missing') === null",
            "var1.getElementsByTagName('book')[0].hasAttribute('title')",
            "var1.documentElement.firstChild.nextSibling.nextSibling === var1.documentElement.lastChild",
            "var1.documentElement.parentNode === var1 && var1.documentElement.ownerDocument === var1",
            "Object.isFrozen(var1) && Object.isFrozen(var1.documentElement.childNodes)",
        ] {
            assert_eq!(
                m.evaluate_condition(condition, &event(), &mut h),
                Ok(true),
                "{condition}"
            );
        }
        assert_eq!(m.evaluate_value("var1", &event(), &mut h), Ok(xml.clone()));
        assert_eq!(
            m.evaluate_log("var1", &event(), &mut h),
            Ok(BOOKS.to_owned())
        );
        m.assign_expression("copy", "var1", &event(), &mut h)
            .expect("a document is assignable");
        assert_eq!(
            m.evaluate_condition("copy === var1", &event(), &mut h),
            Ok(true)
        );
        // A node inside the document is a value too, with no portable form.
        m.assign_expression("copy", "var1.documentElement", &event(), &mut h)
            .expect("a node is assignable");
        assert_eq!(
            m.evaluate_value("copy", &event(), &mut h),
            Err(DataModelError::UnsupportedValue)
        );
        let mut xml_event = event();
        xml_event.data = xml;
        assert_eq!(
            m.evaluate_condition(
                "_event.data.getElementsByTagName('book')[1].getAttribute('title') == 'title2'",
                &xml_event,
                &mut h
            ),
            Ok(true)
        );
    }

    #[test]
    fn only_xml_white_space_stands_beside_an_xml_document() {
        // XML 1.0 §2.1 and §2.8: beside the document element stand only
        // comments, processing instructions, and `S` (#x20 | #x9 | #xD |
        // #xA). Other Unicode white space is text, so the value is not a
        // document (B.2.1) and its DOM is a fragment.
        let padded = " \t\r\n<a/>\n";
        assert_eq!(
            xml_document_value(padded),
            Some(DataValue::Xml(padded.into()))
        );
        let mut m = EcmaScriptDataModel::new();
        m.initialize(r#"{"x":null}"#, system()).expect("initialize");
        let mut h = Host::Inactive;
        m.assign("x", &DataValue::Xml(padded.into()))
            .expect("a document is an XML value");
        assert_eq!(
            m.evaluate_condition("x.nodeType === 9", &event(), &mut h),
            Ok(true)
        );
        for text in ["\u{1680}<a/>", "<a/>\u{a0}"] {
            assert_eq!(xml_document_value(text), None, "{text:?}");
            m.assign("x", &DataValue::Xml(text.into()))
                .expect("a fragment is an XML value");
            assert_eq!(
                m.evaluate_condition("x.nodeType === 11", &event(), &mut h),
                Ok(true),
                "{text:?}"
            );
        }
    }

    #[test]
    fn xml_fragments_and_malformed_markup() {
        let mut m = EcmaScriptDataModel::new();
        m.initialize(r#"{"x":null}"#, system()).expect("initialize");
        let mut h = Host::Inactive;
        m.assign("x", &DataValue::Xml("<a/>text<b/>".into()))
            .expect("a fragment is an XML value");
        assert_eq!(
            m.evaluate_condition(
                "x.nodeType === 11 && x.childNodes.length === 3 && x.getElementsByTagName('b').length === 1",
                &event(),
                &mut h
            ),
            Ok(true)
        );
        assert_eq!(
            m.assign("x", &DataValue::Xml("<a>".into())),
            Err(DataModelError::InvalidInput)
        );
        assert_eq!(
            m.evaluate_condition("x.nodeType === 11", &event(), &mut h),
            Ok(true),
            "a rejected value assigns nothing"
        );
    }

    #[test]
    fn assignments_follow_ecmascript_assignment() {
        let mut m = EcmaScriptDataModel::new();
        m.initialize(r#"{"a":1}"#, system()).expect("initialize");
        let mut h = Host::Inactive;
        // SCXML 5.4: a location that does not denote a valid location is
        // error.execution; strict ECMAScript assignment decides.
        assert_eq!(
            m.assign_expression("missing", "1", &event(), &mut h),
            Err(DataModelError::Execution),
            "an unresolvable name is not a location"
        );
        assert_eq!(
            m.assign_expression("a", "missing + 1", &event(), &mut h),
            Err(DataModelError::Execution)
        );
        assert_eq!(
            m.evaluate_value("a", &event(), &mut h),
            Ok(number(1.0)),
            "a failed evaluation assigns nothing"
        );
        m.assign_expression("a", "new Map([[1, 2]])", &event(), &mut h)
            .expect("any value is assignable");
        assert_eq!(
            m.evaluate_value("a.get(1)", &event(), &mut h),
            Ok(number(2.0))
        );
        // SCXML B.2.7 defines only replacechildren.
        assert_eq!(
            m.apply_assign(
                "a",
                &AssignSource::Value(DataValue::Bool(true)),
                &AssignPlacement::LastChild,
                None,
                &mut h,
            ),
            Err(DataModelError::Execution)
        );
        // SCXML 4.6: a `<foreach>` item or index declares its variable.
        m.apply_assign(
            "item",
            &AssignSource::ForeachValue(DataValue::Bool(true)),
            &AssignPlacement::ReplaceChildren,
            None,
            &mut h,
        )
        .expect("a foreach binding declares its variable");
        assert_eq!(
            m.evaluate_value_without_event("item", &mut h),
            Ok(DataValue::Bool(true))
        );
        assert_eq!(
            m.apply_assign(
                "fresh",
                &AssignSource::Value(DataValue::Bool(true)),
                &AssignPlacement::ReplaceChildren,
                None,
                &mut h,
            ),
            Err(DataModelError::Execution),
            "an <assign> declares nothing"
        );
    }

    #[test]
    fn rejects_lossy_values() {
        let mut m = EcmaScriptDataModel::new();
        m.initialize("{}", system()).expect("initialize");
        let mut h = Host::Inactive;
        assert_eq!(
            m.evaluate_value("NaN", &event(), &mut h),
            Err(DataModelError::UnsupportedValue)
        );
        assert_eq!(
            m.evaluate_value("({value:undefined})", &event(), &mut h),
            Err(DataModelError::UnsupportedValue)
        );
    }

    #[test]
    fn bounds_fail_closed() {
        let mut m = EcmaScriptDataModel::new();
        assert_eq!(
            m.initialize(&"x".repeat(MAX_SOURCE_BYTES + 1), system()),
            Err(DataModelError::InvalidInput)
        );
        assert_eq!(
            m.initialize(r#"{"slot":null,"slot":true}"#, system()),
            Err(DataModelError::InvalidInput)
        );
        m.initialize(r#"{"slot":null}"#, system())
            .expect("initialize");
        let mut host = Host::InReady;
        for value in [
            DataValue::Absent,
            DataValue::Undefined,
            DataValue::Null,
            DataValue::Bool(true),
            DataValue::Number(FiniteNumber::new(1.5).expect("finite test number")),
            DataValue::String(String::new()),
            DataValue::JsonArray("[]".into()),
            DataValue::JsonObject("{}".into()),
            DataValue::Xml("<x/>".into()),
        ] {
            m.assign("slot", &value).expect("portable assignment");
            assert_eq!(m.evaluate_value("slot", &event(), &mut host), Ok(value));
        }
    }

    #[test]
    fn ingress_field_bounds_accept_exactly_and_reject_plus_one() {
        assert_eq!(
            bounded(&"x".repeat(MAX_SOURCE_BYTES), MAX_SOURCE_BYTES),
            Ok(())
        );
        assert_eq!(
            bounded(&"x".repeat(MAX_SOURCE_BYTES + 1), MAX_SOURCE_BYTES),
            Err(DataModelError::InvalidInput)
        );
        // The wasm expression and script bounds; natively both are unbounded.
        assert_eq!(MAX_EXPRESSION_BYTES, scxml_types::wasm_bound(16_384));
        assert_eq!(MAX_SCRIPT_BYTES, scxml_types::wasm_bound(65_536));
        for max in [16_384, 65_536] {
            assert_eq!(bounded(&"x".repeat(max), max), Ok(()));
            assert_eq!(
                bounded(&"x".repeat(max + 1), max),
                Err(DataModelError::InvalidInput)
            );
        }
        let exact_system = SystemContext {
            session_id: "s".repeat(MAX_VALUE_BYTES),
            machine_name: Some("m".into()),
            io_processors: DataValue::JsonObject("{}".into()),
        };
        assert_eq!(validate_system(&exact_system), Ok(()));
        let oversized_system = SystemContext {
            session_id: "s".repeat(MAX_VALUE_BYTES + 1),
            ..exact_system
        };
        assert_eq!(
            validate_system(&oversized_system),
            Err(DataModelError::InvalidInput)
        );
        assert!(to_source(&DataValue::String("\0".repeat(MAX_VALUE_BYTES))).is_ok());
        assert_eq!(
            to_source(&DataValue::String("x".repeat(MAX_VALUE_BYTES + 1))),
            Err(DataModelError::InvalidInput)
        );
        let mut exact_event = event();
        exact_event.origin = Some("o".repeat(MAX_VALUE_BYTES));
        assert_eq!(validate_event(&exact_event), Ok(()));
        exact_event.origin = Some("o".repeat(MAX_VALUE_BYTES + 1));
        assert_eq!(
            validate_event(&exact_event),
            Err(DataModelError::InvalidInput)
        );
        exact_event.origin = None;
        exact_event.origin_type = Some("t".repeat(MAX_VALUE_BYTES + 1));
        assert_eq!(
            validate_event(&exact_event),
            Err(DataModelError::InvalidInput)
        );
        exact_event.origin_type = None;
        exact_event.data = DataValue::Xml("x".repeat(MAX_VALUE_BYTES + 1));
        assert_eq!(
            validate_event(&exact_event),
            Err(DataModelError::InvalidInput)
        );
        exact_event.data = DataValue::Absent;
        exact_event.trace_context = Some(scxml_types::TraceContext {
            traceparent: "00-00000000000000000000000000000001-0000000000000001-01".into(),
            tracestate: Some("x".repeat(scxml_types::MAX_TRACESTATE_BYTES + 1)),
            baggage: None,
            baggage_policy: scxml_types::BaggagePolicy::Drop,
        });
        assert_eq!(
            validate_event(&exact_event),
            Err(DataModelError::InvalidInput)
        );
    }

    #[test]
    fn initial_values_and_bounded_encoding_are_capped() {
        // 1 MiB of initial values: over the 64 KiB `initialize` bound.
        let data = (0..=1_048_576 / MAX_VALUE_BYTES)
            .map(|index| format!("\"value{index}\":\"{}\"", "x".repeat(MAX_VALUE_BYTES - 32)))
            .collect::<Vec<_>>()
            .join(",");
        let mut m = EcmaScriptDataModel::new();
        assert_eq!(
            m.initialize(&format!("{{{data}}}"), system()),
            Err(DataModelError::InvalidInput)
        );
        let mut writer = BoundedBytes::new(4);
        assert!(writer.write_all(b"1234").is_ok());
        assert!(writer.write_all(b"5").is_err());
        assert_eq!(writer.bytes, b"1234");
    }

    /// Natively one operation's inputs are not bounded in aggregate: an
    /// expression longer than the wasm builds' 1 MiB evaluates.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_operations_have_no_aggregate_input_bound() {
        let mut model = EcmaScriptDataModel::new();
        model.initialize("{}", system()).expect("initialize");
        let mut host = Host::Inactive;
        let expression = format!("1 /* {} */", "x".repeat(1_048_576));
        assert_eq!(
            model.evaluate_value(&expression, &event(), &mut host),
            Ok(DataValue::Number(FiniteNumber::new(1.0).expect("finite")))
        );
    }

    #[test]
    fn host_hooks_reject_oversized_array_buffers() {
        let mut model = EcmaScriptDataModel::new();
        model.initialize("{}", system()).expect("initialize");
        let mut host = Host::Inactive;
        assert_eq!(
            model.evaluate_value(
                &format!("new ArrayBuffer({})", MAX_BUFFER_BYTES + 1),
                &event(),
                &mut host,
            ),
            // A rejected runtime allocation is a bound violation, not an
            // illegal expression: it fails closed as unsupported.
            Err(DataModelError::UnsupportedValue)
        );
        assert_eq!(
            model.evaluate_value(
                &format!("new ArrayBuffer({MAX_BUFFER_BYTES})"),
                &event(),
                &mut host,
            ),
            Err(DataModelError::UnsupportedValue)
        );
    }

    #[test]
    fn string_repeat_rejects_before_oversized_result_allocation() {
        let mut model = EcmaScriptDataModel::new();
        model.initialize("{}", system()).expect("initialize");
        let mut host = Host::Inactive;
        assert_eq!(
            model.evaluate_value(
                &format!("'ab'.repeat({})", MAX_VALUE_BYTES / 2),
                &event(),
                &mut host,
            ),
            Ok(DataValue::String("ab".repeat(MAX_VALUE_BYTES / 2)))
        );
        assert_eq!(
            model.evaluate_value(
                &format!("'ab'.repeat({})", MAX_VALUE_BYTES / 2 + 1),
                &event(),
                &mut host,
            ),
            // Same bound-violation classification as oversized buffers.
            Err(DataModelError::UnsupportedValue)
        );
    }
}
