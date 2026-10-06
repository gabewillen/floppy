//! `<data>` binding (SCXML 5.2-5.3), invocation parameters (6.4.2), and
//! `<donedata>` payloads (5.5, 6.5).

use scxml_compiler_core::{
    Binding, DataIr, DoneDataChildIr, DoneDataIr, ExecutableContentIr, MachineIr,
};
use scxml_types::{DataInitializer, DataValue, Event, FiniteNumber, NamedValue};

use super::{
    CapabilityErrorKind, DocumentKind, DonedataErrors, FailureCategory, InterpretFailure,
    Interpreter, SessionCapabilities, inline_body, inline_text, normalize_whitespace,
};
use crate::{EffectFailure, Engine, PlatformError};

/// Name the `XPath` data model reports through `data-model-name`.
pub(super) const XPATH_DATA_MODEL: &str = "xpath";

/// Whether a document needs a data model at all: it declares data, or some
/// executable content, condition, or invocation evaluates through one.
/// A document that needs none never initializes a data model, so it runs
/// with no data model linked.
#[must_use]
pub fn requires_data_model(ir: &MachineIr) -> bool {
    !ir.data.is_empty()
        || ir.states.iter().any(|state| !state.data.is_empty())
        || executable_content_sites(ir)
            .flatten()
            .any(action_requires_data_model)
        || ir
            .transitions
            .iter()
            .any(|transition| transition.cond.is_some())
        // Invocation arguments, ids, and finalize content evaluate in the
        // invoking session's data model (SCXML 6.4).
        || ir.states.iter().any(|state| !state.invokes.is_empty())
        // A final state's <donedata> evaluates when the state is entered
        // (SCXML 5.7).
        || ir.states.iter().any(|state| state.donedata.is_some())
}

/// Every top-level executable-content block of the document, in one walk:
/// the root `<initial>` transition, top-level `<script>` blocks, each
/// state's `<initial>` transition, `<onentry>` and `<onexit>` blocks, and
/// every transition (history defaults included). Nested content (`<if>`,
/// `<foreach>`) needs no descent: its container already needs the data
/// model.
fn executable_content_sites(ir: &MachineIr) -> impl Iterator<Item = &[ExecutableContentIr]> {
    let root = std::iter::once(ir.initial.actions.as_slice());
    let scripts = ir.scripts.iter().map(|block| block.actions.as_slice());
    let states = ir.states.iter().flat_map(|state| {
        state
            .initial
            .iter()
            .map(|initial| initial.actions.as_slice())
            .chain(
                state
                    .onentry
                    .iter()
                    .chain(&state.onexit)
                    .map(|block| block.actions.as_slice()),
            )
    });
    let transitions = ir
        .transitions
        .iter()
        .map(|transition| transition.actions.as_slice());
    root.chain(scripts).chain(states).chain(transitions)
}

/// Whether one action evaluates through the data model. `<if>` needs one
/// for its guard; `<cancel>` only for `sendidexpr` (SCXML 6.3).
fn action_requires_data_model(action: &ExecutableContentIr) -> bool {
    match action {
        ExecutableContentIr::Log { .. }
        | ExecutableContentIr::Assign { .. }
        | ExecutableContentIr::Script { .. }
        | ExecutableContentIr::If { .. }
        | ExecutableContentIr::Foreach { .. }
        | ExecutableContentIr::Send { .. }
        | ExecutableContentIr::Cancel {
            sendidexpr: Some(_),
            ..
        } => true,
        ExecutableContentIr::Raise { .. }
        | ExecutableContentIr::Cancel {
            sendidexpr: None, ..
        }
        | ExecutableContentIr::Extension { .. } => false,
    }
}

/// Loads the extension components a ready session's document uses (SCXML
/// 4.10), before [`bind_initial_data`] and its first command: one
/// [`SessionCapabilities::load_extensions`] call with the Machine IR's
/// extension namespaces, and none for a document that uses no extension
/// namespace.
///
/// # Errors
///
/// The capability's error; the session must then be discarded.
pub fn load_extensions<C: SessionCapabilities>(
    engine: &Engine,
    caps: &mut C,
) -> Result<(), C::Error> {
    let namespaces = &engine.machine().extension_namespaces;
    if namespaces.is_empty() {
        return Ok(());
    }
    caps.load_extensions(namespaces)
}

/// Binds a ready session's data before its first command (SCXML 5.2-5.3,
/// W3C Appendix D `interpret`), with the `<param>`/`namelist` entries an
/// invoking parent passed (`parameters`, empty for a session nobody
/// invoked).
///
/// Initializes the data model with every declaration id in document order
/// (late-bound ones too: SCXML 5.3 creates them at initialization and
/// values them on first entry), values each early-bound declaration in
/// document order, and queues one `error.execution` per illegal
/// initializer ahead of every other event. A document that needs no data
/// model ([`requires_data_model`]) binds nothing.
///
/// SCXML 6.4.3: "If the 'name' of a `<param>` element in the `<invoke>`
/// matches the 'id' of a `<data>` element in the top-level data declarations
/// of the invoked session, the SCXML Processor MUST use the value of the
/// `<param>` element as the initial value of the corresponding `<data>`
/// element. ... (Note that this means that any value specified in the
/// `<data>` element is ignored.)" Such a declaration takes the parameter
/// (the last one of that name) in its place in document order, and its own
/// `expr`, `src`, or children are never evaluated. "If the names do not
/// match, the Processor MUST NOT add the value of the `<param>` element or
/// namelist key/value pair to the invoked session's data model": every
/// other parameter, including one naming state-scoped data, is dropped.
///
/// # Errors
///
/// A fatal capability error, duplicate early-bound ids, or an engine that
/// is not ready. The session must then be discarded.
pub fn bind_initial_data<C: SessionCapabilities>(
    engine: &mut Engine,
    caps: &mut C,
    parameters: &[NamedValue],
) -> Result<(), C::Error> {
    let ir = engine.machine();
    if !requires_data_model(ir) {
        return Ok(());
    }
    let mut declarations: Vec<&DataIr> = ir
        .data
        .iter()
        .chain(ir.states.iter().flat_map(|state| &state.data))
        .collect();
    declarations.sort_by_key(|declaration| declaration.document_order);
    // SCXML 5.3: with late binding only top-level declarations are valued
    // at start; state-scoped ones are valued on their state's first entry.
    let early: Vec<&DataIr> = if ir.binding == Binding::Late {
        declarations
            .iter()
            .copied()
            .filter(|declaration| {
                ir.data
                    .iter()
                    .any(|top| top.document_order == declaration.document_order)
            })
            .collect()
    } else {
        declarations.clone()
    };
    for (index, declaration) in early.iter().enumerate() {
        if early
            .get(..index)
            .is_some_and(|before| before.iter().any(|other| other.id == declaration.id))
        {
            return Err(caps.failure(InterpretFailure::new(
                FailureCategory::Harness,
                "duplicate-data-id",
            )));
        }
    }
    let mut ids: Vec<String> = Vec::with_capacity(declarations.len());
    for declaration in &declarations {
        if !ids.contains(&declaration.id) {
            ids.push(declaration.id.clone());
        }
    }
    // SCXML 5.10: "The SCXML processor MUST bind the variable _name at load
    // time to the value of the 'name' attribute of the <scxml> element."
    // A document without one gives `_name` no value.
    caps.initialize_data_model(&ids, ir.name.as_deref())?;
    for id in ids
        .iter()
        .filter(|id| !early.iter().any(|declaration| declaration.id == **id))
    {
        caps.initialize_data(id, &DataInitializer::Value(DataValue::Undefined))?;
    }
    let xpath = caps.data_model_name().as_deref() == Some(XPATH_DATA_MODEL);
    let early: Vec<DataIr> = early.into_iter().cloned().collect();
    let top_level: Vec<String> = ir.data.iter().map(|data| data.id.clone()).collect();
    let mut startup_errors = 0_usize;
    for declaration in &early {
        let parameter = parameters
            .iter()
            .rev()
            .find(|parameter| parameter.name == declaration.id)
            .filter(|_| top_level.contains(&declaration.id));
        if let Some(parameter) = parameter {
            caps.set_parameter(&parameter.name, &parameter.value)?;
        } else if bind_declaration(caps, declaration, xpath)? {
            startup_errors = startup_errors.saturating_add(1);
        }
    }
    // SCXML 5.3/5.10: illegal initializers queue error.execution before
    // the machine starts, so each error precedes every user event.
    for _ in 0..startup_errors {
        engine.queue_startup_error().map_err(|cause| {
            caps.failure(InterpretFailure::engine(cause, "engine-startup-error"))
        })?;
    }
    Ok(())
}

/// Values one declaration. Returns whether it raised `error.execution`: an
/// illegal initializer leaves the variable `undefined` (SCXML 5.3: "an
/// empty data element").
fn bind_declaration<C: SessionCapabilities>(
    caps: &mut C,
    declaration: &DataIr,
    xpath: bool,
) -> Result<bool, C::Error> {
    let failed = match materialize(caps, declaration, xpath)? {
        Some(DataInitializer::Expression(expression)) => {
            // An `expr` is evaluated and stored in one data-model call, so a
            // function is a legal initial value (B.2, official 453).
            match caps.initialize_data(&declaration.id, &DataInitializer::Expression(expression)) {
                Ok(()) => false,
                Err(error) if C::error_kind(&error).is_recoverable() => true,
                Err(error) => return Err(error),
            }
        }
        Some(initializer) => {
            caps.initialize_data(&declaration.id, &initializer)?;
            false
        }
        None => true,
    };
    if failed {
        caps.initialize_data(
            &declaration.id,
            &DataInitializer::Value(DataValue::Undefined),
        )?;
    }
    Ok(failed)
}

/// The initializer of one declaration (SCXML 5.2-5.3, B.2, and CR
/// 2014-03-13 §C.3.1): its `expr`, its external `src` document, or its
/// inline children. `None` when a recoverable failure (an unresolvable
/// `src`) or a value that is not a legal data value raises
/// `error.execution` (SCXML 5.3: "If the value specified for a <data>
/// element (by 'src', children, or the environment) is not a legal data
/// value, the SCXML Processor MUST raise place error.execution in the
/// internal event queue and MUST create an empty data element in the data
/// model with the specified id.").
fn materialize<C: SessionCapabilities>(
    caps: &mut C,
    declaration: &DataIr,
    xpath: bool,
) -> Result<Option<DataInitializer>, C::Error> {
    if let (None, Some(expression)) = (&declaration.src, &declaration.expr) {
        return Ok(Some(DataInitializer::Expression(expression.clone())));
    }
    if let Some(src) = &declaration.src {
        let document = match caps.load_document(DocumentKind::Data, src) {
            Ok(document) => document,
            Err(error)
                if C::error_kind(&error).is_recoverable()
                    || C::error_kind(&error) == CapabilityErrorKind::Unavailable =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let text = document.text;
        // B.2.1: "if an indication of the type of the content is available
        // (e.g., via a Content-Type header), then the Processor SHOULD try
        // to interpret the content according to that indication." The
        // ECMAScript data model tries a JSON or XML indication first and,
        // when the content is not that type, applies B.2.1's content rules
        // below. The XPath data model (CR 2014-03-13 C.3) states no such
        // rule, so its documents are read by content alone.
        if !xpath {
            match indicated_type(document.media_type.as_deref()) {
                Some(Indicated::Json) => {
                    let trimmed = text.trim_matches(scxml_types::is_xml_space);
                    if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
                        return Ok(json_value(trimmed).map(DataInitializer::Value));
                    }
                }
                Some(Indicated::Xml) => {
                    if let Some(value) = caps.external_data(&text) {
                        return Ok(Some(DataInitializer::Value(value)));
                    }
                }
                None => {}
            }
        }
        // XPath markup is a node-set and ECMAScript XML a DOM (B.2.1,
        // official 557); the data model decides those itself.
        if let Some(value) = caps.external_data(&text) {
            return Ok(Some(DataInitializer::Value(value)));
        }
        // JSON text is its JSON value (official 552); other text is a
        // whitespace-normalized string (official 558). JSON white space
        // (RFC 8259 §2) is XML white space.
        let trimmed = text.trim_matches(scxml_types::is_xml_space);
        let value = if json_initializer(trimmed) {
            json_value(trimmed)
        } else {
            Some(DataValue::String(normalize_whitespace(&text)))
        };
        return Ok(value.map(DataInitializer::Value));
    }
    // CR 2014-03-13 §C.3.1: XPath inline children are the value itself.
    // B.2.1: ECMAScript element children are an XML value (official 557).
    if xpath
        || declaration
            .content
            .iter()
            .any(|node| matches!(node, scxml_compiler_core::XmlNodeIr::Element { .. }))
    {
        return Ok(inline_body(&declaration.content)
            .and_then(|body| caps.inline_content(&body))
            .map(DataInitializer::Value));
    }
    let text = inline_text(&declaration.content);
    // JSON white space (RFC 8259 §2) is XML white space.
    let expression = text.trim_matches(scxml_types::is_xml_space);
    if expression.is_empty() {
        return Ok(Some(DataInitializer::Value(DataValue::Undefined)));
    }
    // W3C ECMAScript profile (5.3): inline text that parses as JSON is
    // that value (official 446), otherwise a normalized string (558).
    let value = if serde_json::from_str::<serde_json::Value>(expression).is_ok() {
        json_value(expression)
    } else {
        Some(DataValue::String(normalize_whitespace(&text)))
    };
    Ok(value.map(DataInitializer::Value))
}

/// A content type a `<data src>` source indicated that B.2.1's content
/// rules know how to interpret.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Indicated {
    /// `application/json` (RFC 8259 §11) or a `+json` structured syntax
    /// suffix (RFC 6839 §3.1).
    Json,
    /// `application/xml` or `text/xml` (RFC 7303 §3) or a `+xml` suffix
    /// (RFC 7303 §4.2).
    Xml,
}

/// Most bytes of a media type the machine reads (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
const MAX_MEDIA_TYPE_BYTES: usize = scxml_types::wasm_bound(256);

/// The type `media_type` indicates: its `type/subtype` before any
/// parameters, compared case-insensitively (RFC 9110 §8.3.1: "The type
/// and subtype tokens are case-insensitive."). Any other type, including
/// `text/plain`, indicates nothing B.2.1 interprets: `text/plain` is the
/// label a resource gets by default (RFC 2045 §5.2) and the one servers
/// give `.txt` files, and the official W3C cases ship JSON and XML in
/// `.txt` files (446, 552, 557) and expect them read by content. A value
/// over [`MAX_MEDIA_TYPE_BYTES`] or with a malformed type indicates nothing.
fn indicated_type(media_type: Option<&str>) -> Option<Indicated> {
    let media_type = media_type?;
    if scxml_types::exceeds_wasm_bound(media_type.len(), MAX_MEDIA_TYPE_BYTES) {
        return None;
    }
    let essence = media_type.split(';').next()?.trim().to_ascii_lowercase();
    let (kind, subtype) = essence.split_once('/')?;
    // RFC 9110 §5.6.2 `token`.
    let token = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
    };
    if !token(kind) || !token(subtype) {
        return None;
    }
    if (kind, subtype) == ("application", "json") || subtype.ends_with("+json") {
        Some(Indicated::Json)
    } else if matches!((kind, subtype), ("application" | "text", "xml"))
        || subtype.ends_with("+xml")
    {
        Some(Indicated::Xml)
    } else {
        None
    }
}

/// Whether external text is a JSON initializer: well-formed JSON whose
/// top-level form is an array, object, string, literal, or number.
fn json_initializer(trimmed: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(trimmed).is_ok()
        && (trimmed.starts_with('[')
            || trimmed.starts_with('{')
            || trimmed.starts_with('"')
            || matches!(trimmed, "true" | "false" | "null")
            || trimmed.parse::<f64>().is_ok())
}

/// The portable value of well-formed JSON text (arrays and objects become
/// canonical JSON); `None` when it is not a legal data value.
fn json_value(text: &str) -> Option<DataValue> {
    match serde_json::from_str::<serde_json::Value>(text).ok()? {
        serde_json::Value::Null => Some(DataValue::Null),
        serde_json::Value::Bool(value) => Some(DataValue::Bool(value)),
        serde_json::Value::Number(number) => number
            .as_f64()
            .and_then(|number| FiniteNumber::new(number).ok())
            .map(DataValue::Number),
        serde_json::Value::String(value) => Some(DataValue::String(value)),
        serde_json::Value::Array(_) => DataValue::canonical_json_array(text).ok(),
        serde_json::Value::Object(_) => DataValue::canonical_json_object(text).ok(),
    }
}

/// Applies one engine-emitted late-binding effect (SCXML 5.3): every
/// declaration binds in document order before the entered state's
/// `<onentry>`; an illegal initializer queues `error.execution` without
/// aborting the entry.
pub(super) fn bind_state_data<C: SessionCapabilities>(
    interpreter: &mut Interpreter<'_, C>,
    engine: &mut Engine,
    declarations: &[DataIr],
) -> Result<(), EffectFailure<C::Error>> {
    let xpath = interpreter.caps.data_model_name().as_deref() == Some(XPATH_DATA_MODEL);
    for declaration in declarations {
        interpreter.caps.declare(&declaration.id)?;
        if bind_declaration(interpreter.caps, declaration, xpath)? {
            interpreter.queue_error(engine, PlatformError::Execution)?;
        }
    }
    Ok(())
}

impl<C: SessionCapabilities> Interpreter<'_, C> {
    /// Whether a `<param location>` names something the data model can
    /// read: an ECMAScript identifier (its `namelist` key), or any `XPath`
    /// location (evaluation then decides).
    fn param_location_is_legal(&mut self, location: &str) -> bool {
        self.caps.data_model_name().as_deref() == Some(XPATH_DATA_MODEL)
            || self.caps.namelist_key(location).is_some()
    }

    /// Resolves one `<donedata>` payload (SCXML 6.5): `<param>` children
    /// and `<content>` follow the `<send>` payload rules.
    ///
    /// `Ok(None)` when the payload is illegal (params with content, two
    /// contents, an uncarriable value) or its content fails recoverably; a
    /// param whose location is illegal or whose evaluation fails recoverably
    /// is ignored and the other params still make the payload (SCXML 5.7.1),
    /// none when every param is ignored.
    /// Each failure queues `error.execution` when `errors` is
    /// [`DonedataErrors::Raise`]. A finished invoked session's
    /// top-level final passes [`DonedataErrors::Drop`]: its error events
    /// could never be processed.
    pub(super) fn donedata_payload(
        &mut self,
        engine: &mut Engine,
        done_data: &DoneDataIr,
        event: Option<&Event>,
        errors: DonedataErrors,
    ) -> Result<Option<DataValue>, EffectFailure<C::Error>> {
        let mut params = Vec::new();
        let mut content = None;
        let mut illegal = false;
        for child in &done_data.children {
            match child {
                DoneDataChildIr::Param { value } => params.push(value),
                DoneDataChildIr::Content { value } => {
                    illegal |= content.is_some();
                    content = Some(value);
                }
            }
        }
        if illegal || (!params.is_empty() && content.is_some()) {
            return self.donedata_failed(engine, errors);
        }
        if !params.is_empty() {
            let mut entries = Vec::with_capacity(params.len());
            for param in params {
                let evaluated = match (&param.expr, &param.location) {
                    (Some(expression), _) => Some(self.caps.evaluate_value(expression, event)),
                    (None, Some(location)) if self.param_location_is_legal(location) => {
                        Some(self.caps.evaluate_location(location, event))
                    }
                    // An illegal location, or a param with neither source.
                    (None, _) => None,
                };
                match evaluated {
                    Some(Ok(value)) => entries.push(NamedValue {
                        name: param.name.clone(),
                        value,
                    }),
                    Some(Err(error)) if !C::error_kind(&error).is_recoverable() => {
                        return Err(EffectFailure::Fatal(error));
                    }
                    // SCXML 5.7.1: "the SCXML Processor MUST place the error
                    // 'error.execution' on the internal event queue and MUST
                    // ignore the name and value": only this param is
                    // dropped; the others still make the data.
                    _ => {
                        self.donedata_failed(engine, errors)?;
                    }
                }
            }
            // Every param ignored: the done event has no data (official
            // cases 343 and 488, "empty event.data"); its errors are queued.
            if entries.is_empty() {
                return Ok(None);
            }
            return match self.caps.event_payload(&entries) {
                Some(payload) => Ok(Some(payload)),
                None => self.donedata_failed(engine, errors),
            };
        }
        if let Some(content) = content {
            if let Some(expression) = &content.expr {
                return match self.caps.evaluate_value(expression, event) {
                    Ok(value) => Ok(Some(value)),
                    Err(error) if C::error_kind(&error).is_recoverable() => {
                        self.donedata_failed(engine, errors)
                    }
                    Err(error) => Err(EffectFailure::Fatal(error)),
                };
            }
            // A body the data model cannot represent is no legal data
            // value (5.9.3): error.execution (3.12.2), not the session's end.
            let value = inline_body(&content.body).and_then(|body| self.caps.inline_content(&body));
            return match value {
                Some(value) => Ok(Some(value)),
                None => self.donedata_failed(engine, errors),
            };
        }
        Ok(Some(DataValue::Undefined))
    }

    /// An illegal or failed `<donedata>` payload: queues `error.execution`
    /// when `errors` is [`DonedataErrors::Raise`] and yields no payload.
    fn donedata_failed(
        &mut self,
        engine: &mut Engine,
        errors: DonedataErrors,
    ) -> Result<Option<DataValue>, EffectFailure<C::Error>> {
        if errors == DonedataErrors::Raise {
            self.queue_error(engine, PlatformError::Execution)?;
        }
        Ok(None)
    }
}
