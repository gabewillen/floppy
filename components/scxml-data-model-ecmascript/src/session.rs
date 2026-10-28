//! The one Boa realm of a data-model instance, kept for the whole session.
//!
//! # Contract
//!
//! * One [`Engine`] per [`crate::EcmaScriptDataModel`] instance owns one
//!   [`Session`]: a Boa context created by `initialize` and never rebuilt,
//!   so every ECMAScript value keeps its identity, its closures, and its
//!   prototype chain across operations, and a script's changes to the
//!   global object or to built-ins persist (SCXML 5.8, B.2). It is never
//!   shared between instances, so no ECMAScript state crosses sessions.
//! * Creation order: bootstrap ([`crate::BOOTSTRAP`]), built-in guard
//!   ([`crate::BUILTIN_GUARD`]), DOM builder ([`dom::DOM_INSTALL`]), the
//!   adapter's helpers ([`helpers_source`]), the natives (`In`,
//!   `Math.random`), the pristine built-ins the adapter reads values with,
//!   then the system variables. Everything the adapter calls later was
//!   captured before any user code ran, so a script that replaces a global
//!   or a built-in never runs inside the adapter's own work.
//! * Everything before the system variables depends on no session, so the
//!   Wasm component creates it ahead of time (`Engine::prepare`, run once
//!   by Wizer at build time through `wizer-initialize`), and the first
//!   `initialize` of each instance adds the system variables to that realm
//!   instead of creating one; every later `initialize` creates its own.
//! * Each system variable (`_sessionid`, `_name`, `_ioprocessors`,
//!   `_event`) is a non-enumerable, non-configurable accessor whose setter
//!   throws a `TypeError` (SCXML 5.10, B.2.8); its getter returns a value
//!   sealed read-only (the `guard` helper): every object reachable from it
//!   is frozen and seen only through a proxy whose traps throw a `TypeError`
//!   on any attempted change, in strict and non-strict code alike, so every
//!   member write fails with `error.execution`. The `_event` getter returns
//!   the operation's event (`undefined` with none). The adapter reads a
//!   sealed value through its targets (the `unwrap` helper).
//! * An operation's questions to its execution host (`In()`, the clock
//!   behind `Date`) are asked synchronously while it runs. Native builds
//!   run every operation on the instance's own worker thread with a
//!   [`crate::PARSE_STACK_BYTES`] stack (Boa values are thread-local); the
//!   calling thread answers those questions through its host while it
//!   waits. A worker that panics loses the realm: every later operation
//!   fails with [`DataModelError::Execution`]. The Wasm component is
//!   single-threaded and runs in place with the host's
//!   [`crate::ExecutionHost::answerer`].
//! * Parsed programs are cached by exact source text (at most
//!   [`MAX_CACHED_SCRIPTS`] entries and [`MAX_CACHED_SCRIPT_BYTES`] bytes).
//! * Telemetry: none; the calling data-model operation's span covers the
//!   work.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use boa_engine::builtins::date::Date;
use boa_engine::builtins::error::Error;
use boa_engine::context::ContextBuilder;
use boa_engine::native_function::NativeFunction;
use boa_engine::object::FunctionObjectBuilder;
use boa_engine::object::IntegrityLevel;
use boa_engine::object::builtins::{JsArray, JsWeakMap};
use boa_engine::property::{Attribute, PropertyDescriptor, PropertyKey};
use boa_engine::{Context, JsObject, JsResult, JsString, JsValue, Script, Source};
use scxml_types::{DataValue, Event, FiniteNumber};

use crate::{
    Answerer, BOOTSTRAP, BUILTIN_GUARD, BoundedHostHooks, DataModelError, ExecutionHost, HostQuery,
    HostReply, LOG_TEXT, MAX_FOREACH_FRAMES, MAX_FOREACH_ITEMS, MAX_IN_QUERIES,
    MAX_LOOP_ITERATIONS, MAX_NESTING_DEPTH, MAX_RECURSION_DEPTH, MAX_STACK_SIZE, MAX_VALUE_BYTES,
    SystemContext, bounded, classify_js_error, dom, event_to_source, to_source, validate_event,
    validate_value,
};

/// Most parsed programs one realm caches.
pub const MAX_CACHED_SCRIPTS: usize = 256;
/// Most source bytes one realm's program cache holds (every build: evicting
/// a cached program is invisible to the document).
pub const MAX_CACHED_SCRIPT_BYTES: usize = 1_048_576;

/// Where a session's `Math.random` sequence stands: the session's seed and
/// how many numbers it has drawn.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RandomPosition {
    pub(crate) seed: u64,
    pub(crate) draws: u64,
}

impl RandomPosition {
    /// The start of the sequence of the session `session_id`, whose seed is
    /// the 64-bit FNV-1a hash of its id.
    pub(crate) fn of(session_id: &str) -> Self {
        let seed = session_id
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
            });
        Self { seed, draws: 0 }
    }

    /// The next number of the sequence, in `[0, 1)`: the `SplitMix64` output
    /// for (seed, draw index), scaled from its top 53 bits.
    fn draw(&mut self) -> f64 {
        let mut mixed = self.seed.wrapping_add(
            self.draws
                .wrapping_add(1)
                .wrapping_mul(0x9e37_79b9_7f4a_7c15),
        );
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        mixed ^= mixed >> 31;
        self.draws = self.draws.wrapping_add(1);
        // 53 bits: exact in an f64, and less than 2^53.
        let top = u32::try_from(mixed >> 43).unwrap_or(0);
        let low = u32::try_from((mixed >> 11) & 0xffff_ffff).unwrap_or(0);
        (f64::from(top) * 4_294_967_296.0 + f64::from(low)) / 9_007_199_254_740_992.0
    }
}

/// The operation running on this thread (each realm lives on one thread and
/// runs one operation at a time): how it asks its host, what it has been
/// answered, and its `Math.random` position.
struct Operation {
    answerer: Answerer,
    answers: BTreeMap<String, bool>,
    now: Option<u64>,
    random: RandomPosition,
    /// The first host failure or bound violation a question met; the
    /// operation fails with it whatever the program did afterwards.
    failed: Option<DataModelError>,
}

thread_local! {
    static OPERATION: RefCell<Option<Operation>> = const { RefCell::new(None) };
}

/// Starts an operation that asks `answerer` and draws from `random`.
fn begin_operation(answerer: Answerer, random: RandomPosition) {
    OPERATION.with(|operation| {
        *operation.borrow_mut() = Some(Operation {
            answerer,
            answers: BTreeMap::new(),
            now: None,
            random,
            failed: None,
        });
    });
}

/// Ends the operation: its final `Math.random` position and its failure.
fn finish_operation() -> (Option<RandomPosition>, Option<DataModelError>) {
    OPERATION.with(|operation| {
        operation
            .borrow_mut()
            .take()
            .map_or((None, Some(DataModelError::Execution)), |operation| {
                (Some(operation.random), operation.failed)
            })
    })
}

/// Asks the running operation's host `query`, recording a failure.
fn ask(query: &HostQuery) -> Option<HostReply> {
    OPERATION.with(|operation| {
        let mut operation = operation.borrow_mut();
        let operation = operation.as_mut()?;
        if operation.failed.is_some() {
            return None;
        }
        match (operation.answerer)(query) {
            Ok(reply) => Some(reply),
            Err(error) => {
                operation.failed = Some(error);
                None
            }
        }
    })
}

/// The time Boa observes: its monotonic clock is fixed at zero (Boa's job
/// scheduling observes no elapsed time), and its wall clock (`Date.now()`,
/// `new Date()`) is the session clock's current time in milliseconds since
/// the Unix epoch, asked of the operation's host (`ExecutionHost::now_ms`)
/// once per operation, when first read. The realm never reads an ambient
/// clock (under wasi-virt the ambient clocks are denied).
#[derive(Debug)]
pub(crate) struct SessionClock;

impl boa_engine::context::Clock for SessionClock {
    fn now(&self) -> boa_engine::context::time::JsInstant {
        boa_engine::context::time::JsInstant::new(0, 0)
    }

    fn system_time_millis(&self) -> i64 {
        let cached = OPERATION.with(|operation| operation.borrow().as_ref().and_then(|o| o.now));
        let now = cached.or_else(|| match ask(&HostQuery::NowMs) {
            Some(HostReply::NowMs(now)) => {
                OPERATION.with(|operation| {
                    if let Some(operation) = operation.borrow_mut().as_mut() {
                        operation.now = Some(now);
                    }
                });
                Some(now)
            }
            _ => None,
        });
        now.map_or(0, |now| i64::try_from(now).unwrap_or(i64::MAX))
    }
}

/// The realm's `Math.random` (ECMA-262 3rd edition 15.8.2.14: "using an
/// implementation-dependent algorithm or strategy"): the next number of the
/// session's deterministic sequence ([`RandomPosition::draw`]).
#[allow(
    clippy::unnecessary_wraps,
    reason = "the NativeFunction signature; drawing cannot fail"
)]
fn random(_this: &JsValue, _arguments: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(OPERATION.with(|operation| {
        operation
            .borrow_mut()
            .as_mut()
            .map_or(0.0, |operation| operation.random.draw())
    })))
}

/// The realm's `In(stateID)` (B.2.3: "a predicate function In(stateID)
/// that evaluates to true if the state machine is in the state"): the
/// argument's ECMAScript `ToString` names the state, answered by the
/// operation's host (once per id per operation: the configuration does not
/// change while an operation runs). An id longer than
/// [`crate::MAX_VALUE_BYTES`] names no state. More than
/// [`crate::MAX_IN_QUERIES`] distinct ids (a wasm build's bound), or a host
/// failure, fails the operation.
fn in_state(_this: &JsValue, arguments: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = arguments
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    if id.len() > MAX_VALUE_BYTES {
        return Ok(JsValue::from(false));
    }
    let known = OPERATION.with(|operation| {
        let mut operation = operation.borrow_mut();
        let operation = operation.as_mut()?;
        if let Some(answer) = operation.answers.get(&id) {
            return Some(Ok(*answer));
        }
        if scxml_types::exceeds_wasm_bound(
            operation.answers.len().saturating_add(1),
            MAX_IN_QUERIES,
        ) {
            operation.failed.get_or_insert(DataModelError::InvalidInput);
            return Some(Err(()));
        }
        None
    });
    let answer = match known {
        Some(Ok(answer)) => Some(answer),
        Some(Err(())) => None,
        None => match ask(&HostQuery::InState(id.clone())) {
            Some(HostReply::InState(answer)) => {
                OPERATION.with(|operation| {
                    if let Some(operation) = operation.borrow_mut().as_mut() {
                        operation.answers.insert(id, answer);
                    }
                });
                Some(answer)
            }
            _ => None,
        },
    };
    answer.map(JsValue::from).ok_or_else(|| {
        boa_engine::JsNativeError::range()
            .with_message("In() could not be answered")
            .into()
    })
}

/// The adapter's helpers, evaluated once per realm after the built-in guard
/// and the DOM builder, before any system variable or user code; called
/// with the DOM text lookup. Every built-in a helper uses is captured when
/// it is evaluated, so user code that later replaces a global never runs
/// inside a helper (the log rendering still converts the logged value
/// itself, as `JSON.stringify` does).
#[allow(
    clippy::too_many_lines,
    reason = "one JavaScript program text; its lines are not Rust statements"
)]
fn helpers_source() -> String {
    format!(
        r#"(function (xmlOf, NativeProxy) {{
  "use strict";
  let current;
  const freeze = Object.freeze;
  const isFrozen = Object.isFrozen;
  const ownKeys = Reflect.ownKeys;
  const describe = Object.getOwnPropertyDescriptor;
  const hasOwn = Object.hasOwn;
  const apply = Reflect.apply;
  const push = Array.prototype.push;
  const pop = Array.prototype.pop;
  const NativeSet = Set;
  const setHas = Set.prototype.has;
  const setAdd = Set.prototype.add;
  const NativeString = String;
  const stringify = JSON.stringify;
  const objectToString = Object.prototype.toString;
  const getPrototypeOf = Object.getPrototypeOf;
  const arrayPrototype = Array.prototype;
  const NativeNumber = Number;
  const floor = Math.floor;
  const NativeWeakMap = WeakMap;
  const weakGet = WeakMap.prototype.get;
  const weakSet = WeakMap.prototype.set;
  const weakHas = WeakMap.prototype.has;
  const reflectSet = Reflect.set;
  const reflectDefine = Reflect.defineProperty;
  const reflectDelete = Reflect.deleteProperty;
  const reflectSetPrototype = Reflect.setPrototypeOf;
  const defineProperty = Object.defineProperty;
  const NativeTypeError = TypeError;
  const isObject = (value) => value !== null && (typeof value === "object" || typeof value === "function");
  // Read-only system objects (5.10): each is a Proxy over its frozen
  // target that turns every attempted change into a TypeError, whatever
  // the caller's mode. `guards` maps a target to its proxy, `targets` a
  // proxy to its target. The handler has no prototype, so no user change to
  // Object.prototype adds a trap.
  const guards = new NativeWeakMap();
  const targets = new NativeWeakMap();
  const readOnly = () => {{ throw new NativeTypeError("system variables are read-only"); }};
  const handler = freeze({{
    __proto__: null,
    set(target, key, value, receiver) {{
      if (receiver === apply(weakGet, guards, [target])) readOnly();
      return reflectSet(target, key, value, receiver);
    }},
    defineProperty(target, key, descriptor) {{
      if (!reflectDefine(target, key, descriptor)) readOnly();
      return true;
    }},
    deleteProperty(target, key) {{
      if (!reflectDelete(target, key)) readOnly();
      return true;
    }},
    setPrototypeOf(target, prototype) {{
      if (!reflectSetPrototype(target, prototype)) readOnly();
      return true;
    }},
  }});
  const unwrap = (value) => isObject(value) && apply(weakHas, targets, [value]) ? apply(weakGet, targets, [value]) : value;
  const xmlOfAny = (value) => xmlOf(unwrap(value));
  const logText = {LOG_TEXT};
  const toLength = (value) => {{
    const number = NativeNumber(value);
    if (!(number > 0)) return 0;
    return number === Infinity ? 9007199254740991 : floor(number);
  }};
  const deepFreeze = (root) => {{
    const stack = [root];
    const seen = new NativeSet();
    while (stack.length > 0) {{
      const value = apply(pop, stack, []);
      if (value === null || (typeof value !== "object" && typeof value !== "function") || apply(setHas, seen, [value])) continue;
      apply(setAdd, seen, [value]);
      if (!isFrozen(value)) freeze(value);
      const keys = ownKeys(value);
      for (let index = 0; index < keys.length; index += 1) {{
        const descriptor = describe(value, keys[index]);
        if (descriptor !== undefined && hasOwn(descriptor, "value")) apply(push, stack, [descriptor.value]);
      }}
    }}
    return root;
  }};
  // Seals a fresh value as a read-only system value: every object reachable
  // through own data properties gets its proxy, each property holding an
  // object then holds that object's proxy, every object is frozen, and the
  // root's proxy is returned. An object that is already frozen is kept.
  const guard = (root) => {{
    if (!isObject(root)) return root;
    const order = [];
    const seen = new NativeSet();
    const stack = [root];
    while (stack.length > 0) {{
      const value = apply(pop, stack, []);
      if (!isObject(value) || apply(setHas, seen, [value]) || apply(weakHas, targets, [value])) continue;
      apply(setAdd, seen, [value]);
      apply(push, order, [value]);
      const keys = ownKeys(value);
      for (let index = 0; index < keys.length; index += 1) {{
        const descriptor = describe(value, keys[index]);
        if (descriptor !== undefined && hasOwn(descriptor, "value")) apply(push, stack, [descriptor.value]);
      }}
    }}
    for (let index = 0; index < order.length; index += 1) {{
      const value = order[index];
      if (!isFrozen(value)) {{
        const proxy = new NativeProxy(value, handler);
        apply(weakSet, guards, [value, proxy]);
        apply(weakSet, targets, [proxy, value]);
      }}
    }}
    for (let index = 0; index < order.length; index += 1) {{
      const value = order[index];
      if (!apply(weakHas, guards, [value]) || isFrozen(value)) continue;
      const keys = ownKeys(value);
      for (let key = 0; key < keys.length; key += 1) {{
        const descriptor = describe(value, keys[key]);
        if (descriptor !== undefined && hasOwn(descriptor, "value") && isObject(descriptor.value)
            && apply(weakHas, guards, [descriptor.value])) {{
          defineProperty(value, keys[key], {{ __proto__: null, value: apply(weakGet, guards, [descriptor.value]) }});
        }}
      }}
    }}
    for (let index = 0; index < order.length; index += 1) {{
      if (!isFrozen(order[index])) freeze(order[index]);
    }}
    return apply(weakHas, guards, [root]) ? apply(weakGet, guards, [root]) : root;
  }};
  return [
    (value) => {{ current = value; }},
    () => current,
    deepFreeze,
    (value) => logText(value, xmlOfAny),
    (value) => {{ const held = guard(value); return () => held; }},
    (value, max) => {{
      if (xmlOfAny(value) !== undefined) return ["illegal"];
      let prototype = value;
      let array = false;
      while (prototype !== null && (typeof prototype === "object" || typeof prototype === "function")) {{
        prototype = getPrototypeOf(prototype);
        if (prototype === arrayPrototype) {{ array = true; break; }}
      }}
      if (!array) return ["illegal"];
      const length = toLength(value.length);
      if (length > max) return ["large"];
      const copy = [];
      for (let index = 0; index < length; index += 1) apply(push, copy, [value[index]]);
      return ["array", copy];
    }},
    guard,
    unwrap,
    guards,
    targets,
    handler,
  ];
}})"#
    )
}

/// Handles to the helpers [`helpers_source`] returns, in its order.
struct Helpers {
    /// `(event)`: binds `_event` to `event` (sealed, or `undefined`).
    set_event: JsObject,
    /// `() => event`: the `_event` getter.
    get_event: JsObject,
    /// `(value) => value`: deep-freezes `value`.
    deep_freeze: JsObject,
    /// `(value) => text`: [`LOG_TEXT`].
    log_text: JsObject,
    /// `(value) => getter`: seals `value` as a read-only system value
    /// (`guard`) and returns a getter of it.
    constant: JsObject,
    /// `(value, max) => [kind, copy?]`: a `<foreach array>` value.
    array_copy: JsObject,
    /// `(value) => sealed`: a fresh value as a read-only system value, every
    /// reachable object behind a proxy whose traps reject every change with
    /// a `TypeError` (5.10: "any attempt to change the value of a system
    /// variable to fail").
    guard: JsObject,
    /// `(value) => target`: the object behind a read-only proxy, else the
    /// value itself.
    unwrap: JsObject,
    /// `guard`'s own state, for [`Session::guard_fresh`]: each guarded
    /// object's proxy, each proxy's object, the proxies' handler, and the
    /// `Proxy` intrinsic.
    guards: JsWeakMap,
    targets: JsWeakMap,
    handler: JsObject,
    proxy: JsObject,
}

impl Helpers {
    fn install(context: &mut Context, xml_text: &JsObject) -> Result<Self, DataModelError> {
        // `Proxy` is gone from the global object (BOOTSTRAP); the adapter
        // takes the intrinsic.
        let proxy = context.intrinsics().constructors().proxy().constructor();
        let proxy_constructor = proxy.clone();
        let list = context
            .eval(Source::from_bytes(&helpers_source()))
            .ok()
            .and_then(|value| value.as_callable())
            .ok_or(DataModelError::Execution)?
            .call(
                &JsValue::undefined(),
                &[xml_text.clone().into(), proxy.into()],
                context,
            )
            .map_err(|_| DataModelError::Execution)?
            .as_object()
            .ok_or(DataModelError::Execution)?;
        let mut next = 0_u32;
        let mut entry = |context: &mut Context| {
            let value = list
                .get(next, context)
                .ok()
                .and_then(|value| value.as_object());
            next += 1;
            value.ok_or(DataModelError::Execution)
        };
        let mut take = |context: &mut Context| {
            Some(entry(context)?)
                .filter(JsObject::is_callable)
                .ok_or(DataModelError::Execution)
        };
        let set_event = take(context)?;
        let get_event = take(context)?;
        let deep_freeze = take(context)?;
        let log_text = take(context)?;
        let constant = take(context)?;
        let array_copy = take(context)?;
        let guard = take(context)?;
        let unwrap = take(context)?;
        let mut weak_map = |context: &mut Context| {
            JsWeakMap::from_object(entry(context)?).map_err(|_| DataModelError::Execution)
        };
        let guards = weak_map(context)?;
        let targets = weak_map(context)?;
        let handler = entry(context)?;
        Ok(Self {
            set_event,
            get_event,
            deep_freeze,
            log_text,
            constant,
            array_copy,
            guard,
            unwrap,
            guards,
            targets,
            handler,
            proxy: proxy_constructor,
        })
    }
}

/// `HostEnsureCanCompileStrings`'s `direct` argument for the `Function`
/// constructor: a dynamic function is never a direct `eval` (ECMA-262
/// `CreateDynamicFunction`).
const FUNCTION_IS_DIRECT_EVAL: bool = false;

/// `Set`'s `Throw` argument when a declared global is redeclared: a write
/// the object refuses throws rather than failing silently (ECMA-262
/// Set(O, P, V, Throw)).
const THROW_ON_FAILED_SET: bool = true;

/// The realm's `Function` constructor (ECMA-262 `CreateDynamicFunction`).
///
/// Boa's own constructor parses the joined parameters and body as one text
/// (so text that closes the parameter list early compiles, where ECMA-262
/// 3rd edition 15.3.2.1 requires a `SyntaxError`) and keeps no source text
/// for the functions it creates (their `toString` is `function anonymous()
/// { [native code] }`). This constructor converts every argument with
/// `ToString`, asks the host hook exactly as Boa's would
/// ([`crate::BoundedHostHooks`]: the same bounds as `eval`), checks the
/// parameters and body as `CreateDynamicFunction` parses them
/// ([`crate::syntax::dynamic_function_source`]; a `SyntaxError` otherwise),
/// and evaluates the source text `function anonymous(<parameters>\n)
/// {\n<body>\n}` in the global scope, so the created function has that
/// text as its source.
fn construct_function(
    _this: &JsValue,
    arguments: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let mut texts = Vec::with_capacity(arguments.len());
    for argument in arguments {
        texts.push(argument.to_string(context)?);
    }
    let (body, parameters) = match texts.split_last() {
        Some((body, parameters)) => (body.clone(), parameters.to_vec()),
        None => (JsString::from(""), Vec::new()),
    };
    let realm = context.realm().clone();
    context.host_hooks().ensure_can_compile_strings(
        realm,
        &parameters,
        &body,
        FUNCTION_IS_DIRECT_EVAL,
        context,
    )?;
    let parameters = parameters
        .iter()
        .map(JsString::to_std_string_escaped)
        .collect::<Vec<_>>()
        .join(",");
    let source = crate::syntax::dynamic_function_source(&parameters, &body.to_std_string_escaped())
        .ok_or_else(|| {
            boa_engine::JsNativeError::syntax()
                .with_message("the Function arguments are not a function")
        })?;
    context.eval(Source::from_bytes(&source))
}

/// Whether a non-enumerable data property is writable and configurable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PropertyAttributes {
    /// Writable and configurable.
    Replaceable,
    /// Neither writable nor configurable.
    Fixed,
}

/// Replaces the global `Function` with [`construct_function`]: the same
/// `prototype` (`Function.prototype`, whose `constructor` it becomes),
/// name, length, and property attributes as the built-in.
fn install_function_constructor(context: &mut Context) -> Result<(), DataModelError> {
    let prototype = context.intrinsics().constructors().function().prototype();
    let function = FunctionObjectBuilder::new(
        context.realm(),
        NativeFunction::from_fn_ptr(construct_function),
    )
    .name(JsString::from("Function"))
    .length(1)
    .constructor(true)
    .build();
    let data = |value: JsValue, attributes: PropertyAttributes| {
        let replaceable = attributes == PropertyAttributes::Replaceable;
        PropertyDescriptor::builder()
            .value(value)
            .writable(replaceable)
            .enumerable(false)
            .configurable(replaceable)
    };
    function
        .define_property_or_throw(
            JsString::from("prototype"),
            data(prototype.clone().into(), PropertyAttributes::Fixed),
            context,
        )
        .and_then(|_| {
            prototype.define_property_or_throw(
                JsString::from("constructor"),
                data(function.clone().into(), PropertyAttributes::Replaceable),
                context,
            )
        })
        .and_then(|_| {
            context.global_object().define_property_or_throw(
                JsString::from("Function"),
                data(function.clone().into(), PropertyAttributes::Replaceable),
                context,
            )
        })
        .map(drop)
        .map_err(|_| DataModelError::Execution)
}

/// The setter of every system variable: throws a `TypeError` (5.10: "The
/// Processor MUST cause any attempt to change the value of a system
/// variable to fail").
fn read_only(_this: &JsValue, _arguments: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Err(boa_engine::JsNativeError::typ()
        .with_message("system variables are read-only")
        .into())
}

/// Installs the realm's native globals: `In` (B.2.3), the `Function`
/// constructor ([`install_function_constructor`]), and `Math.random`.
fn install_natives(context: &mut Context) -> Result<(), DataModelError> {
    let in_function =
        FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(in_state))
            .name(JsString::from("In"))
            .length(1)
            .constructor(false)
            .build();
    context
        .register_global_property(JsString::from("In"), in_function, Attribute::default())
        .map_err(|_| DataModelError::Execution)?;
    install_function_constructor(context)?;
    let random = FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(random))
        .name(JsString::from("random"))
        .length(0)
        .constructor(false)
        .build();
    context
        .intrinsics()
        .objects()
        .math()
        .define_property_or_throw(
            JsString::from("random"),
            PropertyDescriptor::builder()
                .value(random)
                .writable(true)
                .enumerable(false)
                .configurable(true),
            context,
        )
        .map_err(|_| DataModelError::Execution)?;
    Ok(())
}

/// One `<foreach>` in progress (SCXML 4.6, B.2.11): the shallow copy of its
/// array, the items `foreach_items` returned for it, the variable its items
/// are bound to (the first variable bound after it), and how many items
/// are bound.
struct ForeachFrame {
    copy: JsObject,
    images: Vec<DataValue>,
    item: Option<String>,
    next: usize,
}

/// How a `<foreach array>` value was found.
pub(crate) enum Copied {
    /// An object that satisfies `instanceof(Array)`: its items.
    Items(Vec<DataValue>),
    /// An array of more than [`MAX_FOREACH_ITEMS`] items.
    Large,
    /// Any other value.
    Illegal,
}

/// A value of a declared variable as the snapshot shows it.
pub(crate) enum Shown {
    /// Its portable value.
    Value(DataValue),
    /// Its ECMAScript `typeof` and its string form.
    Text(String, String),
}

/// The pristine built-ins the adapter reads values with, captured before
/// any user code ran.
struct Pristine {
    object_prototype: JsObject,
    function_to_string: JsObject,
    date_to_string: JsObject,
}

/// The one realm of an instance; see the module contract.
pub(crate) struct Session {
    // Boa handles drop before the context that owns their realm.
    scripts: BTreeMap<String, Script>,
    script_bytes: usize,
    helpers: Helpers,
    dom_build: JsObject,
    /// The DOM builder of an event payload (nodes left for `guard`).
    dom_build_open: JsObject,
    xml_text: JsObject,
    pristine: Pristine,
    /// The event `_event` was last bound to, by its source, so an event
    /// bound again is the same object.
    event: Option<(String, JsValue)>,
    random: RandomPosition,
    foreach: Vec<ForeachFrame>,
    context: Context,
}

impl Session {
    /// Creates the realm of a session with system variables `system`; see
    /// the module contract for the order.
    fn create(system: &SystemContext) -> Result<Self, DataModelError> {
        Self::prepare()?.started(system)
    }

    /// Creates a realm with everything of the module contract's order but
    /// the system variables: it depends on no session.
    fn prepare() -> Result<Self, DataModelError> {
        let mut context = ContextBuilder::new()
            .host_hooks(Rc::new(BoundedHostHooks))
            .clock(Rc::new(SessionClock))
            .build()
            .map_err(|_| DataModelError::Execution)?;
        context
            .runtime_limits_mut()
            .set_loop_iteration_limit(MAX_LOOP_ITERATIONS);
        context
            .runtime_limits_mut()
            .set_recursion_limit(MAX_RECURSION_DEPTH);
        context
            .runtime_limits_mut()
            .set_stack_size_limit(MAX_STACK_SIZE);
        context
            .eval(Source::from_bytes(BOOTSTRAP))
            .map_err(|_| DataModelError::Execution)?;
        // The guard takes its bounds as arguments: no bound is a global.
        let bounds = [
            JsValue::from(u32::try_from(MAX_VALUE_BYTES).map_err(|_| DataModelError::Execution)?),
            JsValue::from(u32::try_from(MAX_NESTING_DEPTH).map_err(|_| DataModelError::Execution)?),
        ];
        context
            .eval(Source::from_bytes(BUILTIN_GUARD))
            .ok()
            .and_then(|guard| guard.as_callable())
            .ok_or(DataModelError::Execution)?
            .call(&JsValue::undefined(), &bounds, &mut context)
            .map_err(|_| DataModelError::Execution)?;
        let pair = context
            .eval(Source::from_bytes(dom::DOM_INSTALL))
            .ok()
            .and_then(|value| value.as_object())
            .ok_or(DataModelError::Execution)?;
        let element = |index: u32, context: &mut Context| {
            pair.get(index, context)
                .ok()
                .and_then(|value| value.as_callable())
                .ok_or(DataModelError::Execution)
        };
        let dom_build = element(0, &mut context)?;
        let xml_text = element(1, &mut context)?;
        let dom_build_open = element(2, &mut context)?;
        let helpers = Helpers::install(&mut context, &xml_text)?;
        install_natives(&mut context)?;
        let pristine = Pristine::capture(&mut context)?;
        Ok(Self {
            scripts: BTreeMap::new(),
            script_bytes: 0,
            helpers,
            dom_build,
            dom_build_open,
            xml_text,
            pristine,
            event: None,
            random: RandomPosition::default(),
            foreach: Vec::new(),
            context,
        })
    }

    /// This prepared realm (see [`Self::prepare`]) as the realm of the
    /// session with system variables `system`.
    fn started(mut self, system: &SystemContext) -> Result<Self, DataModelError> {
        self.random = RandomPosition::of(&system.session_id);
        self.register_system(system)?;
        Ok(self)
    }

    /// Binds `_event` for an operation: the event built from its source
    /// (the same object as the last binding when the source is the same),
    /// sealed read-only (`guard`; 5.10: the system variables are a
    /// "protected portion of the data model", and "any attempt to change the
    /// value of a system variable" fails); `undefined` with no current
    /// event.
    fn bind_event(&mut self, event: Option<&Event>) -> Result<(), DataModelError> {
        let value = match event {
            Some(event) => {
                validate_event(event)?;
                let source = event_to_source(event)?;
                match &self.event {
                    Some((bound, value)) if *bound == source => value.clone(),
                    _ => {
                        let value = if let Some(value) = self.event_value(event) {
                            value
                        } else {
                            let builder = self.dom_build_open.clone();
                            self.eval_adapter_with(&source, &builder)?
                        };
                        let value = self.guard_fresh(value)?;
                        self.event = Some((source, value.clone()));
                        value
                    }
                }
            }
            None => JsValue::undefined(),
        };
        self.call(&self.helpers.set_event.clone(), &[value])
            .map(drop)
    }

    /// `guard` (the helper) natively, for a value [`Self::bind_event`] just
    /// built (directly, or by evaluating its source): the same proxies, in
    /// the same maps, with the same properties redefined and the same
    /// objects frozen, without interpreting the helper's loops for every
    /// property of a large event. Such a value's objects are fresh ordinary
    /// objects and arrays with data properties only; a value holding any
    /// other object is passed to the helper itself.
    fn guard_fresh(&mut self, root: JsValue) -> Result<JsValue, DataModelError> {
        let Some(root_object) = root.as_object() else {
            return Ok(root);
        };
        let context = &mut self.context;
        let error = |_| DataModelError::Execution;
        // Each reachable object, once, with its object-valued properties.
        let mut order: Vec<(JsObject, Vec<(PropertyKey, JsObject)>)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![root_object.clone()];
        while let Some(object) = stack.pop() {
            if !seen.insert(object.clone()) {
                continue;
            }
            if !(object.is_ordinary() || object.is_array()) {
                return self.call(&self.helpers.guard.clone(), &[root]);
            }
            if self.helpers.targets.has(&object, context).map_err(error)? {
                continue;
            }
            let mut children = Vec::new();
            for key in object.own_property_keys(context).map_err(error)? {
                if let Some(child) = object.get(key.clone(), context).map_err(error)?.as_object() {
                    stack.push(child.clone());
                    children.push((key, child));
                }
            }
            order.push((object, children));
        }
        let mut proxies = std::collections::HashMap::new();
        for (object, _) in &order {
            if !object
                .test_integrity_level(IntegrityLevel::Frozen, context)
                .map_err(error)?
            {
                let proxy = self
                    .helpers
                    .proxy
                    .construct(
                        &[object.clone().into(), self.helpers.handler.clone().into()],
                        None,
                        context,
                    )
                    .map_err(error)?;
                self.helpers
                    .guards
                    .set(object, proxy.clone().into(), context)
                    .map_err(error)?;
                self.helpers
                    .targets
                    .set(&proxy, object.clone().into(), context)
                    .map_err(error)?;
                proxies.insert(object.clone(), proxy);
            }
        }
        for (object, children) in &order {
            if !proxies.contains_key(object) {
                continue;
            }
            for (key, child) in children {
                if let Some(proxy) = proxies.get(child) {
                    object
                        .define_property_or_throw(
                            key.clone(),
                            PropertyDescriptor::builder().value(proxy.clone()),
                            context,
                        )
                        .map_err(error)?;
                }
            }
        }
        for (object, _) in &order {
            if proxies.contains_key(object) {
                object
                    .set_integrity_level(IntegrityLevel::Frozen, context)
                    .map_err(error)?;
            }
        }
        Ok(proxies.remove(&root_object).map_or(root, JsValue::from))
    }

    /// The `_event` object of `event` built directly, as its source
    /// ([`event_to_source`]) evaluates: its fields in that order, a field
    /// with no value `undefined`. `None` when its data is not a primitive or
    /// JSON that [`Self::json_to_value`] builds, so the caller evaluates the
    /// source.
    fn event_value(&mut self, event: &Event) -> Option<JsValue> {
        let data = match &event.data {
            DataValue::Absent => JsValue::undefined(),
            DataValue::JsonArray(json) | DataValue::JsonObject(json) => self.json_to_value(json)?,
            data => primitive(data)?,
        };
        let text = |value: Option<&str>| {
            value.map_or_else(JsValue::undefined, |value| {
                JsValue::from(JsString::from(value))
            })
        };
        let fields = [
            ("name", text(Some(event.name.as_str()))),
            (
                "type",
                text(Some(match event.event_type {
                    scxml_types::EventType::Internal => "internal",
                    scxml_types::EventType::External => "external",
                    scxml_types::EventType::Platform => "platform",
                })),
            ),
            (
                "sendid",
                text(event.send_id.as_ref().map(scxml_types::SendId::as_str)),
            ),
            ("origin", text(event.origin.as_deref())),
            ("origintype", text(event.origin_type.as_deref())),
            (
                "invokeid",
                text(event.invoke_id.as_ref().map(scxml_types::InvokeId::as_str)),
            ),
            ("data", data),
            ("raw", text(event.raw.as_deref())),
        ];
        let object = JsObject::with_object_proto(self.context.intrinsics());
        for (name, value) in fields {
            object
                .create_data_property_or_throw(JsString::from(name), value, &mut self.context)
                .ok()?;
        }
        Some(object.into())
    }

    /// The ECMAScript value of the JSON text `json` built directly, without
    /// the parser, as the same text evaluates as a literal: each object an
    /// ordinary object with its members in the text's order (a repeated
    /// member keeps its first place and its last value), each array an
    /// array. `None` when the text is not JSON this build takes (a number
    /// [`exact_numbers`] does not vouch for, a lone surrogate, nesting past
    /// [`MAX_NESTING_DEPTH`]) or a member is named `__proto__` (a literal
    /// sets the prototype there): the caller evaluates the text instead.
    pub(crate) fn json_to_value(&mut self, json: &str) -> Option<JsValue> {
        if !exact_numbers(json) {
            return None;
        }
        let mut deserializer = serde_json::Deserializer::from_str(json);
        let value = serde::de::DeserializeSeed::deserialize(
            JsonSeed {
                context: &mut self.context,
                depth: 0,
            },
            &mut deserializer,
        )
        .ok()?;
        deserializer.end().ok()?;
        Some(value)
    }

    /// Evaluates checked `source` in the global scope through the program
    /// cache.
    pub(crate) fn eval(&mut self, source: &str) -> JsResult<JsValue> {
        if let Some(script) = self.scripts.get(source) {
            let script = script.clone();
            return script.evaluate(&mut self.context);
        }
        let script = Script::parse(Source::from_bytes(source), None, &mut self.context)?;
        if self.scripts.len() >= MAX_CACHED_SCRIPTS
            || self.script_bytes.saturating_add(source.len()) > MAX_CACHED_SCRIPT_BYTES
        {
            self.scripts.clear();
            self.script_bytes = 0;
        }
        self.script_bytes = self.script_bytes.saturating_add(source.len());
        self.scripts.insert(source.to_owned(), script.clone());
        script.evaluate(&mut self.context)
    }

    /// Evaluates adapter-produced value `source` (a value or event source,
    /// never user text) as the body of a function whose parameter
    /// [`dom::DOM_BUILDER`] is the DOM builder.
    fn eval_adapter(&mut self, source: &str) -> Result<JsValue, DataModelError> {
        let builder = self.dom_build.clone();
        self.eval_adapter_with(source, &builder)
    }

    /// [`Self::eval_adapter`] with the DOM builder `builder`.
    fn eval_adapter_with(
        &mut self,
        source: &str,
        builder: &JsObject,
    ) -> Result<JsValue, DataModelError> {
        let body = format!("(function ({}) {{ return ({source}); }})", dom::DOM_BUILDER);
        let function = self
            .context
            .eval(Source::from_bytes(&body))
            .ok()
            .and_then(|value| value.as_callable())
            .ok_or(DataModelError::Execution)?;
        self.call(&function, &[builder.clone().into()])
    }

    /// The object behind a read-only system proxy, else `object` itself, so
    /// the adapter reads a system value's own properties as it reads any
    /// other value's.
    fn unwrap_guarded(&mut self, object: JsObject) -> JsObject {
        self.call(&self.helpers.unwrap.clone(), &[object.clone().into()])
            .ok()
            .and_then(|value| value.as_object())
            .unwrap_or(object)
    }

    /// The ECMAScript value of the portable `value`: a primitive directly,
    /// any other value from its source (an XML value as its DOM), the
    /// absent marker deep-frozen.
    pub(crate) fn value_of(&mut self, value: &DataValue) -> Result<JsValue, DataModelError> {
        if let Some(primitive) = primitive(value) {
            return Ok(primitive);
        }
        if let DataValue::JsonArray(json) | DataValue::JsonObject(json) = value {
            validate_value(value)?;
            if let Some(built) = self.json_to_value(json) {
                return Ok(built);
            }
        }
        let built = self.eval_adapter(&to_source(value)?)?;
        if matches!(value, DataValue::Absent) {
            return self.call(&self.helpers.deep_freeze.clone(), &[built]);
        }
        Ok(built)
    }

    /// Evaluates checked `source` to a function (through the program
    /// cache) and calls it with `arguments`.
    pub(crate) fn call_source(&mut self, source: &str, arguments: &[JsValue]) -> JsResult<JsValue> {
        let function = self.eval(source)?;
        let function = function.as_callable().ok_or_else(|| {
            boa_engine::JsNativeError::typ().with_message("adapter source is not a function")
        })?;
        function.call(&JsValue::undefined(), arguments, &mut self.context)
    }

    /// Declares the global variable `name` with `value`: sets it when the
    /// global object has it, and otherwise defines it as a `var`
    /// declaration does (writable, enumerable, non-configurable).
    pub(crate) fn declare(&mut self, name: &str, value: JsValue) -> Result<(), DataModelError> {
        let global = self.context.global_object();
        let key = JsString::from(name);
        let exists = global
            .has_own_property(key.clone(), &mut self.context)
            .map_err(|_| DataModelError::Execution)?;
        if exists {
            global
                .set(key, value, THROW_ON_FAILED_SET, &mut self.context)
                .map(drop)
                .map_err(|error| classify_js_error(&error))
        } else {
            global
                .define_property_or_throw(
                    key,
                    PropertyDescriptor::builder()
                        .value(value)
                        .writable(true)
                        .enumerable(true)
                        .configurable(false),
                    &mut self.context,
                )
                .map(drop)
                .map_err(|_| DataModelError::Execution)
        }
    }

    fn call(
        &mut self,
        helper: &JsObject,
        arguments: &[JsValue],
    ) -> Result<JsValue, DataModelError> {
        helper
            .call(&JsValue::undefined(), arguments, &mut self.context)
            .map_err(|_| DataModelError::Execution)
    }

    /// Calls a helper that returns a function.
    fn call_helper(
        &mut self,
        helper: &JsObject,
        arguments: &[JsValue],
    ) -> Result<JsObject, DataModelError> {
        self.call(helper, arguments)?
            .as_callable()
            .ok_or(DataModelError::Execution)
    }

    /// Registers the read-only system variables (SCXML 5.10):
    /// `_sessionid`, `_name`, `_ioprocessors`, and `_event`, in that order.
    fn register_system(&mut self, system: &SystemContext) -> Result<(), DataModelError> {
        let session_id = JsValue::from(JsString::from(system.session_id.as_str()));
        let getter = self.call_helper(&self.helpers.constant.clone(), &[session_id])?;
        self.define_system("_sessionid", getter)?;
        // SCXML 5.10: `_name` is "the value of the 'name' attribute of the
        // <scxml> element"; a document without one leaves it `undefined`.
        let name = system
            .machine_name
            .as_deref()
            .map_or_else(JsValue::undefined, |name| {
                JsValue::from(JsString::from(name))
            });
        let getter = self.call_helper(&self.helpers.constant.clone(), &[name])?;
        self.define_system("_name", getter)?;
        let io_processors = self.value_of(&system.io_processors)?;
        let getter = self.call_helper(&self.helpers.constant.clone(), &[io_processors])?;
        self.define_system("_ioprocessors", getter)?;
        self.call(&self.helpers.set_event.clone(), &[JsValue::undefined()])?;
        self.define_system("_event", self.helpers.get_event.clone())
    }

    /// Defines the system variable `name` (SCXML 5.10) as a non-enumerable,
    /// non-configurable accessor of the global object read through `getter`
    /// whose setter throws a `TypeError`: B.2.8 "The SCXML Processor MUST
    /// define an ECMAScript read-only variable for each system variable",
    /// and 5.10 "The Processor MUST cause any attempt to change the value
    /// of a system variable to fail and MUST place the error
    /// 'error.execution' on the internal event queue".
    fn define_system(&mut self, name: &str, getter: JsObject) -> Result<(), DataModelError> {
        let setter = FunctionObjectBuilder::new(
            self.context.realm(),
            NativeFunction::from_fn_ptr(read_only),
        )
        .name(JsString::from(""))
        .length(1)
        .constructor(false)
        .build();
        self.context
            .global_object()
            .define_property_or_throw(
                JsString::from(name),
                PropertyDescriptor::builder()
                    .get(getter)
                    .set(setter)
                    .enumerable(false)
                    .configurable(false),
                &mut self.context,
            )
            .map(drop)
            .map_err(|_| DataModelError::Execution)
    }

    /// The portable value of `value`, without lossy coercion and without
    /// running user code: a primitive (a finite number, a string), the absent marker, an XML document (its XML
    /// text, B.2.1), or a JSON tree ([`Self::json_text`]).
    ///
    /// # Errors
    ///
    /// [`DataModelError::UnsupportedValue`] for any other value;
    /// [`DataModelError::InvalidInput`] for text longer than
    /// [`MAX_VALUE_BYTES`].
    pub(crate) fn portable_value(&mut self, value: &JsValue) -> Result<DataValue, DataModelError> {
        if value.is_undefined() {
            return Ok(DataValue::Undefined);
        }
        if value.is_null() {
            return Ok(DataValue::Null);
        }
        if let Some(v) = value.as_boolean() {
            return Ok(DataValue::Bool(v));
        }
        if let Some(v) = value.as_number() {
            return FiniteNumber::new(v)
                .map(DataValue::Number)
                .map_err(|_| DataModelError::UnsupportedValue);
        }
        if let Some(v) = value.as_string() {
            let v = v
                .to_std_string()
                .map_err(|_| DataModelError::UnsupportedValue)?;
            return Ok(DataValue::String(v));
        }
        let object = value.as_object().ok_or(DataModelError::UnsupportedValue)?;
        let object = self.unwrap_guarded(object);
        let value = &JsValue::from(object.clone());
        if self.is_absent_marker(&object) {
            return Ok(DataValue::Absent);
        }
        if let Some(xml) = self.xml_of(value)? {
            if !dom::is_markup(&xml) {
                return Err(DataModelError::UnsupportedValue);
            }
            return Ok(DataValue::Xml(xml));
        }
        let mut text = String::new();
        let mut path = Vec::new();
        match self.json_text(&object, &mut text, &mut path) {
            Json::Done => {}
            Json::Unsupported => return Err(DataModelError::UnsupportedValue),
            Json::TooLong => return Err(DataModelError::InvalidInput),
        }
        if object.is_array() {
            DataValue::canonical_json_array(&text).map_err(|_| DataModelError::UnsupportedValue)
        } else {
            DataValue::canonical_json_object(&text).map_err(|_| DataModelError::UnsupportedValue)
        }
    }

    /// The XML text an XML document was built from; `None` for any other
    /// value.
    fn xml_of(&mut self, value: &JsValue) -> Result<Option<String>, DataModelError> {
        let text = self.call(&self.xml_text.clone(), std::slice::from_ref(value))?;
        text.as_string()
            .map(|text| {
                text.to_std_string()
                    .map_err(|_| DataModelError::UnsupportedValue)
            })
            .transpose()
    }

    /// Whether `object` is the adapter's absent marker: a frozen object
    /// whose one own property is the data property `$scxml` = `"absent"`.
    fn is_absent_marker(&mut self, object: &JsObject) -> bool {
        if object.is_extensible(&mut self.context) != Ok(false) {
            return false;
        }
        let Ok(keys) = object.own_property_keys(&mut self.context) else {
            return false;
        };
        let [PropertyKey::String(key)] = keys.as_slice() else {
            return false;
        };
        *key == "$scxml"
            && object
                .borrow()
                .properties()
                .get(&keys[0])
                .and_then(|descriptor| descriptor.value().cloned())
                .and_then(|value| value.as_string())
                .is_some_and(|value| value == "absent")
    }

    /// Appends the JSON text of `object` to `text`, as `JSON.stringify`
    /// would write it, when `object` is a JSON tree: an array, or an
    /// ordinary object whose prototype is `Object.prototype`; with no symbol
    /// keys; every own property a data property (enumerable or not) whose
    /// value is `null`, a boolean, a finite number, a string, or such a tree
    /// (never `undefined`, a function, a symbol, or a bigint); an array's
    /// enumerable keys exactly its indices; nested at most
    /// [`MAX_NESTING_DEPTH`] deep and without a cycle (`path` holds the
    /// enclosing objects). Enumerable properties are written in own-key
    /// order. Reads own properties only: no getter, `toJSON`, or other user
    /// code runs.
    fn json_text(
        &mut self,
        object: &JsObject,
        text: &mut String,
        path: &mut Vec<JsObject>,
    ) -> Json {
        let object = &self.unwrap_guarded(object.clone());
        if object.is_callable()
            || path.len() >= MAX_NESTING_DEPTH
            || path
                .iter()
                .any(|enclosing| JsObject::equals(enclosing, object))
        {
            return Json::Unsupported;
        }
        let array = object.is_array();
        let plain_object = object.is_ordinary()
            && object.prototype().is_some_and(|prototype| {
                JsObject::equals(&prototype, &self.pristine.object_prototype)
            });
        if !(array || plain_object) {
            return Json::Unsupported;
        }
        let Ok(keys) = object.own_property_keys(&mut self.context) else {
            return Json::Unsupported;
        };
        let mut members = Vec::with_capacity(keys.len());
        let mut length = None;
        for key in keys {
            let name = match &key {
                PropertyKey::Symbol(_) => return Json::Unsupported,
                PropertyKey::String(name) => name.clone(),
                PropertyKey::Index(index) => JsString::from(index.get().to_string().as_str()),
            };
            let Some(descriptor) = object.borrow().properties().get(&key) else {
                return Json::Unsupported;
            };
            let Some(value) = descriptor.value().cloned() else {
                return Json::Unsupported;
            };
            if array && name == "length" {
                length = value.as_number();
                continue;
            }
            if value.is_undefined() {
                return Json::Unsupported;
            }
            let enumerable = descriptor.enumerable() == Some(true);
            if array && enumerable && !matches!(key, PropertyKey::Index(_)) {
                return Json::Unsupported;
            }
            members.push((
                name,
                value,
                enumerable,
                matches!(key, PropertyKey::Index(_)),
            ));
        }
        if array {
            let indices = members.iter().filter(|member| member.3 && member.2).count();
            // Array indices are below 2^32, so the count is exact as an f64.
            let indices = u32::try_from(indices).ok().map(f64::from);
            if length != indices || members.iter().any(|member| member.3 && !member.2) {
                return Json::Unsupported;
            }
        }
        path.push(object.clone());
        text.push(if array { '[' } else { '{' });
        let mut first = true;
        for (name, value, enumerable, _) in members {
            if !first && enumerable {
                text.push(',');
            }
            if !array && enumerable {
                let Ok(name) = name.to_std_string() else {
                    return Json::Unsupported;
                };
                let Ok(quoted) = serde_json::to_string(&name) else {
                    return Json::Unsupported;
                };
                text.push_str(&quoted);
                text.push(':');
            }
            // A non-enumerable property is checked, not written.
            let mut scratch = String::new();
            let target = if enumerable { &mut *text } else { &mut scratch };
            let written = self.json_value(&value, target, path);
            if !matches!(written, Json::Done) {
                return written;
            }
            if enumerable {
                first = false;
            }
            if text.len() > MAX_VALUE_BYTES {
                return Json::TooLong;
            }
        }
        text.push(if array { ']' } else { '}' });
        path.pop();
        if text.len() > MAX_VALUE_BYTES {
            Json::TooLong
        } else {
            Json::Done
        }
    }

    /// Appends the JSON text of one member value (see [`Self::json_text`]).
    fn json_value(&mut self, value: &JsValue, text: &mut String, path: &mut Vec<JsObject>) -> Json {
        if value.is_null() {
            text.push_str("null");
        } else if let Some(v) = value.as_boolean() {
            text.push_str(if v { "true" } else { "false" });
        } else if let Some(v) = value.as_number() {
            if !v.is_finite() {
                return Json::Unsupported;
            }
            // ECMAScript Number::toString, as JSON.stringify writes it.
            let Ok(number) = JsValue::from(v).to_string(&mut self.context) else {
                return Json::Unsupported;
            };
            text.push_str(&number.to_std_string_escaped());
        } else if let Some(v) = value.as_string() {
            let Ok(v) = v.to_std_string() else {
                return Json::Unsupported;
            };
            let Ok(quoted) = serde_json::to_string(&v) else {
                return Json::Unsupported;
            };
            text.push_str(&quoted);
        } else if let Some(object) = value.as_object() {
            return self.json_text(&object, text, path);
        } else {
            return Json::Unsupported;
        }
        Json::Done
    }

    /// Renders `value` with [`LOG_TEXT`].
    pub(crate) fn log_text(&mut self, value: &JsValue) -> Result<String, DataModelError> {
        let text = self
            .helpers
            .log_text
            .clone()
            .call(
                &JsValue::undefined(),
                std::slice::from_ref(value),
                &mut self.context,
            )
            .map_err(|error| classify_js_error(&error))?
            .as_string()
            .ok_or(DataModelError::UnsupportedValue)?
            .to_std_string()
            .map_err(|_| DataModelError::UnsupportedValue)?;
        bounded(&text, MAX_VALUE_BYTES)?;
        Ok(text)
    }

    /// The declared variable `name` as the snapshot shows it: its portable
    /// value, or else its ECMAScript `typeof` and string form
    /// ([`Self::string_form`]). Reads the global object's own data property
    /// and runs no user code; `None` when it has no such property (a
    /// binding a script replaced with an accessor is shown by its `typeof`
    /// alone).
    pub(crate) fn shown(&mut self, name: &str) -> Option<Shown> {
        let descriptor = self
            .context
            .global_object()
            .borrow()
            .properties()
            .get(&PropertyKey::from(JsString::from(name)))?;
        let Some(value) = descriptor.value().cloned() else {
            return Some(Shown::Text("accessor".into(), String::new()));
        };
        match self.portable_value(&value) {
            Ok(portable) => Some(Shown::Value(portable)),
            Err(_) => Some(Shown::Text(
                value.type_of().to_owned(),
                self.string_form(&value),
            )),
        }
    }

    /// The string form of a value with no portable value, without running
    /// user code: a primitive's ECMAScript `ToString` (a symbol's
    /// descriptive string); a function's source text
    /// (`Function.prototype.toString`); a `Date`'s
    /// `Date.prototype.toString`; any other object `[object Tag]` with
    /// ECMA-262's built-in tag (`Object.prototype.toString` without
    /// `@@toStringTag`).
    fn string_form(&mut self, value: &JsValue) -> String {
        if let Some(symbol) = value.as_symbol() {
            return symbol.descriptive_string().to_std_string_escaped();
        }
        let Some(object) = value.as_object() else {
            return value
                .to_string(&mut self.context)
                .map(|text| text.to_std_string_escaped())
                .unwrap_or_default();
        };
        let object = self.unwrap_guarded(object);
        let pristine = if object.is_callable() {
            Some(self.pristine.function_to_string.clone())
        } else if object.is::<Date>() {
            Some(self.pristine.date_to_string.clone())
        } else {
            None
        };
        if let Some(to_string) = pristine {
            return to_string
                .call(value, &[], &mut self.context)
                .ok()
                .and_then(|text| text.as_string())
                .map(|text| text.to_std_string_escaped())
                .unwrap_or_default();
        }
        let tag = if object.is_array() {
            "Array"
        } else if object.is::<Error>() {
            "Error"
        } else if object.is::<bool>() {
            "Boolean"
        } else if object.is::<f64>() {
            "Number"
        } else if object.is::<JsString>() {
            "String"
        } else if object.is::<boa_engine::builtins::regexp::RegExp>() {
            "RegExp"
        } else {
            "Object"
        };
        format!("[object {tag}]")
    }

    /// Evaluates a `<foreach array>` value and starts its loop over a
    /// shallow copy (see [`crate::EcmaScriptDataModel::foreach_items`]).
    pub(crate) fn start_foreach(&mut self, value: &JsValue) -> Result<Copied, DataModelError> {
        let max =
            JsValue::from(u32::try_from(MAX_FOREACH_ITEMS).map_err(|_| DataModelError::Execution)?);
        let result = self
            .helpers
            .array_copy
            .clone()
            .call(
                &JsValue::undefined(),
                &[value.clone(), max],
                &mut self.context,
            )
            .map_err(|error| classify_js_error(&error))?
            .as_object()
            .ok_or(DataModelError::Execution)?;
        let kind = result
            .get(0, &mut self.context)
            .ok()
            .and_then(|kind| kind.as_string())
            .map(|kind| kind.to_std_string_escaped())
            .ok_or(DataModelError::Execution)?;
        let copy = match kind.as_str() {
            "array" => result
                .get(1, &mut self.context)
                .ok()
                .and_then(|copy| copy.as_object())
                .ok_or(DataModelError::Execution)?,
            "large" => return Ok(Copied::Large),
            _ => return Ok(Copied::Illegal),
        };
        let length = JsArray::from_object(copy.clone())
            .and_then(|array| array.length(&mut self.context))
            .map_err(|_| DataModelError::Execution)?;
        let mut images = Vec::new();
        for index in 0..length {
            let element = copy
                .get(
                    u32::try_from(index).map_err(|_| DataModelError::Execution)?,
                    &mut self.context,
                )
                .map_err(|_| DataModelError::Execution)?;
            let image = match self.portable_value(&element) {
                Ok(image) => image,
                Err(_) => DataValue::String(self.log_text(&element)?),
            };
            images.push(image);
        }
        if !images.is_empty() {
            if scxml_types::exceeds_wasm_bound(
                self.foreach.len().saturating_add(1),
                MAX_FOREACH_FRAMES,
            ) {
                // Forget the oldest loop (one an error left unfinished).
                self.foreach.remove(0);
            }
            self.foreach.push(ForeachFrame {
                copy,
                images: images.clone(),
                item: None,
                next: 0,
            });
        }
        Ok(Copied::Items(images))
    }

    /// Forgets every `<foreach>` in progress (see
    /// [`crate::EcmaScriptDataModel::release_foreach`]).
    pub(crate) fn release_foreach(&mut self) {
        self.foreach.clear();
    }

    /// The `<foreach>` loops in progress.
    pub(crate) fn foreach_in_progress(&self) -> usize {
        self.foreach.len()
    }

    /// The element a `<foreach>` binds when `name` is assigned the item
    /// `value` (4.6: the loop assigns each item of its shallow copy in
    /// turn): the next element of the innermost loop whose next item is
    /// `value` and whose item variable is `name` (or not yet known); a
    /// loop whose last item is bound ends, with every loop inside it (loops
    /// an error left unfinished). `None` when no loop expects that
    /// binding (an `index` variable, or a binding outside any loop).
    pub(crate) fn foreach_element(
        &mut self,
        name: &str,
        value: &DataValue,
    ) -> Result<Option<JsValue>, DataModelError> {
        let found = self.foreach.iter().rposition(|frame| {
            frame.next < frame.images.len()
                && frame.item.as_deref().is_none_or(|item| item == name)
                && frame.images.get(frame.next) == Some(value)
        });
        let Some(position) = found else {
            return Ok(None);
        };
        let frame = &mut self.foreach[position];
        let element = u32::try_from(frame.next).map_err(|_| DataModelError::Execution)?;
        frame.item = Some(name.to_owned());
        frame.next += 1;
        let done = frame.next == frame.images.len();
        let copy = frame.copy.clone();
        if done {
            self.foreach.truncate(position);
        }
        copy.get(element, &mut self.context)
            .map(Some)
            .map_err(|_| DataModelError::Execution)
    }
}

impl Pristine {
    /// Reads the built-ins the adapter uses from the realm's intrinsics
    /// before any user code runs.
    fn capture(context: &mut Context) -> Result<Self, DataModelError> {
        let constructors = context.intrinsics().constructors();
        let object_prototype = constructors.object().prototype();
        let function_prototype = constructors.function().prototype();
        let date_prototype = constructors.date().prototype();
        let method = |prototype: &JsObject, context: &mut Context| {
            prototype
                .get(JsString::from("toString"), context)
                .ok()
                .and_then(|value| value.as_callable())
                .ok_or(DataModelError::Execution)
        };
        Ok(Self {
            object_prototype,
            function_to_string: method(&function_prototype, context)?,
            date_to_string: method(&date_prototype, context)?,
        })
    }
}

/// How [`Session::json_text`] ended.
enum Json {
    Done,
    Unsupported,
    TooLong,
}

/// The ECMAScript value of a primitive `value`: exactly the value its
/// source ([`to_source`]) evaluates to, without parsing it. `None` for an
/// object (JSON, XML) or the absent marker.
/// Whether `serde_json` reads every number of the JSON text `json` to the
/// value an ECMAScript literal has: an integer of at most 19 digits (it is
/// read as an integer, then rounded once), or a number whose significant
/// digits fit 15 and whose decimal exponent, after its fraction, is within
/// 22 (one exact power of ten, so one rounding). Its other numbers it reads
/// by a best-effort multiplication that may miss by a unit in the last
/// place. A string's characters are skipped; the text is not checked
/// otherwise.
fn exact_numbers(json: &str) -> bool {
    let bytes = json.as_bytes();
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'"' => {
                index += 1;
                while let Some(&byte) = bytes.get(index) {
                    index += if byte == b'\\' { 2 } else { 1 };
                    if byte == b'"' {
                        break;
                    }
                }
            }
            b'-' | b'0'..=b'9' => {
                let start = index;
                while bytes.get(index).is_some_and(|byte| {
                    matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                }) {
                    index += 1;
                }
                if !exact_number(bytes.get(start..index).unwrap_or_default()) {
                    return false;
                }
            }
            _ => index += 1,
        }
    }
    true
}

/// Whether one number token is read exactly ([`exact_numbers`]).
fn exact_number(token: &[u8]) -> bool {
    let (mantissa, exponent) = match token.iter().position(|byte| matches!(byte, b'e' | b'E')) {
        Some(at) => (
            token.get(..at).unwrap_or_default(),
            std::str::from_utf8(token.get(at + 1..).unwrap_or_default())
                .ok()
                .and_then(|exponent| exponent.parse::<i64>().ok()),
        ),
        None => (token, Some(0)),
    };
    let Some(exponent) = exponent else {
        return false;
    };
    let digits: Vec<u8> = mantissa
        .iter()
        .copied()
        .filter(u8::is_ascii_digit)
        .collect();
    let fraction = mantissa
        .iter()
        .position(|&byte| byte == b'.')
        .map_or(0, |at| mantissa.len() - at - 1);
    if fraction == 0 && token.len() == mantissa.len() {
        return digits.len() <= 19;
    }
    let significant = digits.iter().skip_while(|&&digit| digit == b'0').count();
    let Ok(fraction) = i64::try_from(fraction) else {
        return false;
    };
    significant <= 15 && exponent.saturating_sub(fraction).abs() <= 22
}

/// Builds one JSON value in a realm ([`Session::json_to_value`]), `depth`
/// containers deep.
struct JsonSeed<'a> {
    context: &'a mut Context,
    depth: usize,
}

impl JsonSeed<'_> {
    /// The seed of a value inside the container this seed builds.
    fn inner(&mut self) -> Result<JsonSeed<'_>, JsonRefused> {
        let depth = self.depth.saturating_add(1);
        if depth > MAX_NESTING_DEPTH {
            return Err(JsonRefused);
        }
        Ok(JsonSeed {
            context: self.context,
            depth,
        })
    }
}

/// Why [`JsonSeed`] stopped: the text is left to the literal's evaluation.
#[derive(Debug)]
struct JsonRefused;

impl std::fmt::Display for JsonRefused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("refused")
    }
}

impl<'de> serde::de::DeserializeSeed<'de> for JsonSeed<'_> {
    type Value = JsValue;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<JsValue, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> serde::de::Visitor<'de> for JsonSeed<'_> {
    type Value = JsValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_unit<E>(self) -> Result<JsValue, E> {
        Ok(JsValue::null())
    }

    fn visit_bool<E>(self, value: bool) -> Result<JsValue, E> {
        Ok(JsValue::from(value))
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "an ECMAScript number literal rounds the same way"
    )]
    fn visit_i64<E>(self, value: i64) -> Result<JsValue, E> {
        Ok(JsValue::from(value as f64))
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "an ECMAScript number literal rounds the same way"
    )]
    fn visit_u64<E>(self, value: u64) -> Result<JsValue, E> {
        Ok(JsValue::from(value as f64))
    }

    fn visit_f64<E>(self, value: f64) -> Result<JsValue, E> {
        Ok(JsValue::from(value))
    }

    fn visit_str<E>(self, value: &str) -> Result<JsValue, E> {
        Ok(JsValue::from(JsString::from(value)))
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(
        mut self,
        mut sequence: A,
    ) -> Result<JsValue, A::Error> {
        let mut items = Vec::new();
        loop {
            let seed = self.inner().map_err(serde::de::Error::custom)?;
            match sequence.next_element_seed(seed)? {
                Some(item) => items.push(item),
                None => break,
            }
        }
        Ok(JsArray::from_iter(items, self.context).into())
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(mut self, mut map: A) -> Result<JsValue, A::Error> {
        let object = JsObject::with_object_proto(self.context.intrinsics());
        while let Some(key) = map.next_key::<std::borrow::Cow<'de, str>>()? {
            if key == "__proto__" {
                return Err(serde::de::Error::custom(JsonRefused));
            }
            let seed = self.inner().map_err(serde::de::Error::custom)?;
            let value = map.next_value_seed(seed)?;
            object
                .create_data_property_or_throw(JsString::from(key.as_ref()), value, self.context)
                .map_err(|_| serde::de::Error::custom(JsonRefused))?;
        }
        Ok(object.into())
    }
}

pub(crate) fn primitive(value: &DataValue) -> Option<JsValue> {
    match value {
        DataValue::Undefined => Some(JsValue::undefined()),
        DataValue::Null => Some(JsValue::null()),
        DataValue::Bool(value) => Some(JsValue::from(*value)),
        DataValue::Number(value) => Some(JsValue::from(value.get())),
        DataValue::String(value) => Some(JsValue::from(JsString::from(value.as_str()))),
        _ => None,
    }
}

/// Runs one operation in `slot`'s realm: installs `answerer` and the
/// session's `Math.random` position, binds `_event`, runs `work`, and
/// fails with any host failure or bound violation a question met.
fn run_in<T>(
    slot: &mut Option<Session>,
    answerer: Answerer,
    event: Option<&Event>,
    work: impl FnOnce(&mut Session) -> Result<T, DataModelError>,
) -> Result<T, DataModelError> {
    let session = slot.as_mut().ok_or(DataModelError::NotInitialized)?;
    begin_operation(answerer, session.random);
    let result = session.bind_event(event).and_then(|()| work(session));
    let (random, failed) = finish_operation();
    if let Some(random) = random {
        session.random = random;
    }
    match failed {
        Some(error) => Err(error),
        None => result,
    }
}

/// The realm owner of one data-model instance; see the module contract.
#[derive(Default)]
pub(crate) struct Engine {
    #[cfg(target_arch = "wasm32")]
    slot: RefCell<Option<Session>>,
    /// A realm [`Self::prepare`] created ahead of the first session, which
    /// [`Self::start`] takes instead of creating one.
    #[cfg(target_arch = "wasm32")]
    prepared: RefCell<Option<Session>>,
    #[cfg(not(target_arch = "wasm32"))]
    worker: RefCell<Option<worker::Worker>>,
}

impl Engine {
    /// Creates the realm of a session (replacing any earlier one) and
    /// declares `declarations` with their values in order.
    pub(crate) fn start(
        &self,
        system: &SystemContext,
        declarations: Vec<(String, DataValue)>,
    ) -> Result<(), DataModelError> {
        let system = system.clone();
        #[cfg(target_arch = "wasm32")]
        let prepared = self
            .prepared
            .try_borrow_mut()
            .map_err(|_| DataModelError::Execution)?
            .take();
        self.dispatch(
            &mut crate::NoHost,
            move |slot: &mut Option<Session>, _answerer: Answerer| {
                *slot = None;
                #[cfg(target_arch = "wasm32")]
                let mut session = match prepared {
                    Some(session) => session.started(&system)?,
                    None => Session::create(&system)?,
                };
                #[cfg(not(target_arch = "wasm32"))]
                let mut session = Session::create(&system)?;
                for (name, value) in &declarations {
                    let value = session.value_of(value)?;
                    session.declare(name, value)?;
                }
                *slot = Some(session);
                Ok(())
            },
        )
    }

    /// Creates the realm the next [`Self::start`] takes (Wasm only: a
    /// native realm lives on its worker thread), so a component
    /// pre-initialized by this call (`wizer-initialize`) starts each
    /// instance's first session from it. Every realm is created the same
    /// way from no input, so a session started from a prepared realm sees
    /// exactly what one created by `start` would.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn prepare(&self) -> Result<(), DataModelError> {
        let session = Session::prepare()?;
        *self
            .prepared
            .try_borrow_mut()
            .map_err(|_| DataModelError::Execution)? = Some(session);
        Ok(())
    }

    /// Runs `work` in the realm with `_event` bound to `event`, asking
    /// `host` every question it asks.
    pub(crate) fn run<T: Send + 'static>(
        &self,
        host: &mut impl ExecutionHost,
        event: Option<&Event>,
        work: impl FnOnce(&mut Session) -> Result<T, DataModelError> + Send + 'static,
    ) -> Result<T, DataModelError> {
        let event = event.cloned();
        self.dispatch(host, move |slot, answerer| {
            run_in(slot, answerer, event.as_ref(), work)
        })
    }

    #[cfg(target_arch = "wasm32")]
    fn dispatch<T>(
        &self,
        host: &mut impl ExecutionHost,
        job: impl FnOnce(&mut Option<Session>, Answerer) -> Result<T, DataModelError>,
    ) -> Result<T, DataModelError> {
        let answerer = host.answerer().ok_or(DataModelError::Host)?;
        let mut slot = self
            .slot
            .try_borrow_mut()
            .map_err(|_| DataModelError::Execution)?;
        job(&mut slot, answerer)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn dispatch<T: Send + 'static>(
        &self,
        host: &mut impl ExecutionHost,
        job: impl FnOnce(&mut Option<Session>, Answerer) -> Result<T, DataModelError> + Send + 'static,
    ) -> Result<T, DataModelError> {
        let mut worker = self
            .worker
            .try_borrow_mut()
            .map_err(|_| DataModelError::Execution)?;
        if worker.is_none() {
            *worker = Some(worker::Worker::spawn()?);
        }
        let running = worker.as_ref().ok_or(DataModelError::Execution)?;
        let mut answer = |query: &HostQuery| match query {
            HostQuery::InState(id) => host.in_state(id).map(HostReply::InState),
            HostQuery::NowMs => host.now_ms().map(HostReply::NowMs),
        };
        running
            .run(job, &mut answer)
            .unwrap_or(Err(DataModelError::Execution))
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod worker {
    //! Native worker thread owning one instance's realm.

    use std::sync::mpsc;
    use std::thread::JoinHandle;

    use super::Session;
    use crate::{Answerer, DataModelError, HostQuery, HostReply, PARSE_STACK_BYTES};

    type Job = Box<dyn FnOnce(&mut Option<Session>) + Send>;

    /// A message from a running job to the thread waiting for it.
    enum Message<T> {
        /// A question for the waiting thread's host.
        Ask(HostQuery),
        /// The job's result.
        Done(T),
    }

    /// One dedicated thread with a [`PARSE_STACK_BYTES`] stack; jobs run
    /// one at a time in submission order. Once a job panics the thread and
    /// its realm are gone, and every later job fails.
    pub(super) struct Worker {
        jobs: Option<mpsc::Sender<Job>>,
        thread: Option<JoinHandle<()>>,
    }

    impl Worker {
        pub(super) fn spawn() -> Result<Self, DataModelError> {
            let (jobs, receive) = mpsc::channel::<Job>();
            let thread = std::thread::Builder::new()
                .name("scxml-ecmascript".to_owned())
                .stack_size(PARSE_STACK_BYTES)
                .spawn(move || {
                    let mut session = None;
                    while let Ok(job) = receive.recv() {
                        job(&mut session);
                    }
                })
                .map_err(|_| DataModelError::Execution)?;
            Ok(Self {
                jobs: Some(jobs),
                thread: Some(thread),
            })
        }

        /// Runs `job` on the worker and waits for its result, answering
        /// every question it asks with `answer` meanwhile; `None` when the
        /// worker is gone (it panicked).
        pub(super) fn run<T: Send + 'static>(
            &self,
            job: impl FnOnce(&mut Option<Session>, Answerer) -> T + Send + 'static,
            answer: &mut dyn FnMut(&HostQuery) -> Result<HostReply, DataModelError>,
        ) -> Option<T> {
            let (send, messages) = mpsc::channel::<Message<T>>();
            let (reply, replies) = mpsc::channel::<Result<HostReply, DataModelError>>();
            let asking = send.clone();
            let job: Job = Box::new(move |session| {
                let answerer: Answerer = Box::new(move |query: &HostQuery| {
                    asking
                        .send(Message::Ask(query.clone()))
                        .map_err(|_| DataModelError::Host)?;
                    replies.recv().map_err(|_| DataModelError::Host)?
                });
                let result = job(session, answerer);
                // The caller waits on `messages` until this send or the
                // worker's unwinding drops every sender.
                if send.send(Message::Done(result)).is_err() {
                    // Unreachable while the caller waits; nothing to report.
                }
            });
            self.jobs.as_ref()?.send(job).ok()?;
            loop {
                match messages.recv().ok()? {
                    Message::Ask(query) => reply.send(answer(&query)).ok()?,
                    Message::Done(result) => return Some(result),
                }
            }
        }
    }

    impl Drop for Worker {
        fn drop(&mut self) {
            // Closing the queue ends the worker loop; its realm drops on its
            // own thread.
            self.jobs = None;
            if let Some(thread) = self.thread.take()
                && thread.join().is_err()
            {
                // The worker panicked; its failure was already reported.
            }
        }
    }
}

#[cfg(test)]
mod tests;
