//! External `<script src>` loading and Wasm script-component execution
//! (SCXML 1.0 section 5.8).
//!
//! The machine component owns script semantics: it decides which references
//! load at document load (`load-script`) and when a script runs
//! (`run-script`). This module is the host side of those two capabilities
//! and stays a thin linker:
//!
//! * **Resolution** — a pinned reference is served from the packaged
//!   artifact's `script-bundle` when present, otherwise from the embedder's
//!   runtime [`ScriptResolver`] (the `script-loader` contract, the only path
//!   browsers use). Resolver results must also be allowlisted by SHA-256 in
//!   the [`ScriptPolicy`]; bundle entries are authenticated by the IR pin
//!   inside the already-authenticated packaged artifact.
//! * **Authentication** — every resolved script runs through
//!   [`scxml_script::verify_script`] (URI, media-type dispatch, size, pinned
//!   SHA-256, component encoding, and `script` world), the same check the
//!   compiler runs for compile-time packages, plus the policy byte bound.
//! * **Execution** — a Wasm script component is compiled once at load and
//!   instantiated fresh in its own bounded store for every run, with
//!   `data-model-host` linked to the session's active data model (the
//!   provider is moved into the script store for the call and always moved
//!   back); data-model-language text runs through the data model's own
//!   `execute`.
//!
//! Failures are typed WIT errors: load failures reject the document (the
//! machine starts `failed`), run failures are `execution` or `invalid-input`
//! (SCXML `error.execution`), and a run of a reference that was never loaded
//! is a `protocol` violation.
//!
//! Telemetry: closed operation labels only; URIs, media types, digests, and
//! script payloads never become attributes. Script guests receive point
//! signals (`observe`/`event`); their `telemetry.start` returns the documented
//! "unavailable" id 0, so guests continue unchanged.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use opentelemetry::Context;
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent};
use sha2::{Digest as _, Sha256};
use wasmtime::component::{HasSelf, Linker};

use crate::bindings::stateforward::scxml::telemetry;
use crate::bindings::stateforward::scxml::types::{
    ComponentError, DataValue as WitDataValue, ErrorCode, ErrorVersion, Event as WitEvent,
    ScriptReference, TraceContext as WitTraceContext,
};
use crate::call_budget::CallBudget;
use crate::component::{GuestSignal, record_guest_signal};
use crate::memory_growth::MeteredLimits;
use crate::session::{MachineDataModel, failure_code, native_value, wit_value};
use crate::{
    HostError, InvalidLimit, LifecycleLease, TelemetryProviders, TrustedComponentDigest,
    WasmtimeStoreConfig,
};

mod script_bindings {
    wasmtime::component::bindgen!({
        path: "../../crates/scxml-wit/wit/scxml",
        world: "script",
        with: {
            "stateforward:scxml/types": crate::bindings::stateforward::scxml::types,
            "stateforward:scxml/telemetry": crate::bindings::stateforward::scxml::telemetry,
        },
    });
}

use script_bindings::stateforward::scxml::data_model_host;

/// Default instruction fuel for one script run.
pub const DEFAULT_SCRIPT_RUN_FUEL: u64 = 1_000_000;
/// Maximum entries accepted from one packaged `script-bundle`.
pub const MAX_SCRIPT_BUNDLE_ENTRIES: usize = scxml_script::MAX_SCRIPT_PACKAGES;

/// Bytes and media type an embedder's resolver fetched for one `src` URI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedScript {
    /// Media type, for example `application/wasm`; compared byte-for-byte.
    pub media_type: String,
    /// Exact fetched bytes.
    pub bytes: Vec<u8>,
}

/// Closed resolver failure; each one rejects the document at load.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScriptResolveError {
    /// The URI does not name an available script.
    NotFound,
    /// The embedder refuses to serve the URI.
    Denied,
    /// Fetching failed.
    Platform,
}

/// Embedder-supplied runtime `script-loader` capability: resolves one `src`
/// URI exactly as authored to bytes and a media type.
///
/// Implementations own their I/O and policy; the host authenticates
/// everything they return and never trusts the media type or bytes.
pub trait ScriptResolver {
    /// Resolves one URI.
    ///
    /// # Errors
    ///
    /// Returns the closed failure; the machine session starts `failed`.
    fn resolve(&mut self, uri: &str) -> Result<ResolvedScript, ScriptResolveError>;
}

/// Runtime script trust policy: the SHA-256 allowlist, a byte bound, and
/// per-run fuel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptPolicy {
    trusted: BTreeSet<TrustedComponentDigest>,
    max_script_bytes: usize,
    run_fuel: u64,
}

impl ScriptPolicy {
    /// Allows exactly the scripts whose SHA-256 is in `trusted`, bounded by
    /// [`scxml_script::MAX_SCRIPT_COMPONENT_BYTES`] and
    /// [`DEFAULT_SCRIPT_RUN_FUEL`] per run.
    #[must_use]
    pub fn new(trusted: impl IntoIterator<Item = TrustedComponentDigest>) -> Self {
        Self {
            trusted: trusted.into_iter().collect(),
            max_script_bytes: scxml_script::MAX_SCRIPT_COMPONENT_BYTES,
            run_fuel: DEFAULT_SCRIPT_RUN_FUEL,
        }
    }

    /// Tightens the byte bound for resolved scripts.
    ///
    /// # Errors
    ///
    /// [`HostError::InvalidLimit`] for 0 or more than
    /// [`scxml_script::MAX_SCRIPT_COMPONENT_BYTES`].
    pub fn with_max_script_bytes(mut self, bytes: usize) -> Result<Self, HostError> {
        if bytes == 0 || bytes > scxml_script::MAX_SCRIPT_COMPONENT_BYTES {
            return Err(HostError::InvalidLimit(InvalidLimit::ScriptBytes));
        }
        self.max_script_bytes = bytes;
        Ok(self)
    }

    /// Sets the instruction fuel of one script run.
    ///
    /// # Errors
    ///
    /// [`HostError::InvalidLimit`] for zero fuel.
    pub fn with_run_fuel(mut self, fuel: u64) -> Result<Self, HostError> {
        if fuel == 0 {
            return Err(HostError::InvalidLimit(InvalidLimit::Fuel));
        }
        self.run_fuel = fuel;
        Ok(self)
    }
}

/// Script capability linked into one machine session.
pub struct MachineScripts {
    resolver: Option<Box<dyn ScriptResolver>>,
    policy: ScriptPolicy,
}

impl MachineScripts {
    /// No runtime loader: only scripts bundled in a packaged machine load;
    /// any other `<script src>` rejects the document with `unavailable`.
    #[must_use]
    pub fn none() -> Self {
        Self {
            resolver: None,
            policy: ScriptPolicy::new([]),
        }
    }

    /// Runtime loading through `resolver` under `policy` (dynamic or
    /// agent-authored SCXML; the browser model).
    #[must_use]
    pub fn runtime(resolver: Box<dyn ScriptResolver>, policy: ScriptPolicy) -> Self {
        Self {
            resolver: Some(resolver),
            policy,
        }
    }
}

/// Comparable identity of one loaded reference.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ReferenceKey {
    uri: String,
    pinned: Option<(String, String, Option<String>)>,
}

impl ReferenceKey {
    fn of(reference: &ScriptReference) -> Self {
        Self {
            uri: reference.uri.clone(),
            pinned: reference.pinned.as_ref().map(|pinned| {
                (
                    pinned.media_type.clone(),
                    pinned.sha256.clone(),
                    pinned.wit_world.clone(),
                )
            }),
        }
    }
}

enum LoadedKind {
    Component(wasmtime::component::Component),
    Text(String),
}

struct LoadedScript {
    key: ReferenceKey,
    kind: LoadedKind,
}

/// Session-owned script state inside the machine store.
pub(crate) struct ScriptHost {
    resolver: Option<Box<dyn ScriptResolver>>,
    policy: ScriptPolicy,
    bundle: BTreeMap<String, (String, Vec<u8>)>,
    loaded: BTreeMap<String, LoadedScript>,
    engine: wasmtime::Engine,
    store_config: WasmtimeStoreConfig,
    /// The session host's per-call fuel budget, which script runs are
    /// charged to; `None` outside the session host.
    call_budget: Option<Arc<CallBudget>>,
}

fn component_error(code: ErrorCode, detail: &'static str) -> ComponentError {
    ComponentError {
        version: ErrorVersion::V1,
        code,
        detail: Some(detail.into()),
    }
}

fn script_error(error: scxml_script::ScriptError) -> ComponentError {
    let code = match error {
        scxml_script::ScriptError::TooLarge => ErrorCode::PayloadTooLarge,
        scxml_script::ScriptError::InvalidUri
        | scxml_script::ScriptError::UnsupportedMediaType
        | scxml_script::ScriptError::InvalidDigest
        | scxml_script::ScriptError::DigestMismatch
        | scxml_script::ScriptError::NotAComponent
        | scxml_script::ScriptError::WorldMismatch
        | scxml_script::ScriptError::InvalidText => ErrorCode::InvalidInput,
    };
    component_error(code, error.detail())
}

impl ScriptHost {
    pub(crate) fn new(
        scripts: MachineScripts,
        engine: wasmtime::Engine,
        store_config: WasmtimeStoreConfig,
    ) -> Self {
        Self {
            resolver: scripts.resolver,
            policy: scripts.policy,
            bundle: BTreeMap::new(),
            loaded: BTreeMap::new(),
            engine,
            store_config,
            call_budget: None,
        }
    }

    /// Charges every script run to `budget` (the session host's per-call
    /// fuel budget).
    pub(crate) fn sharing(mut self, budget: &Arc<CallBudget>) -> Self {
        self.call_budget = Some(Arc::clone(budget));
        self
    }

    /// Installs the packaged artifact's `script-bundle` entries.
    ///
    /// # Errors
    ///
    /// [`HostError::Component`] when the bundle is over-bound, unsorted,
    /// duplicated, or carries an invalid URI.
    pub(crate) fn install_bundle(
        &mut self,
        entries: Vec<(String, String, Vec<u8>)>,
    ) -> Result<(), HostError> {
        if entries.len() > MAX_SCRIPT_BUNDLE_ENTRIES {
            return Err(HostError::Component("invalid-script-bundle".into()));
        }
        let mut previous: Option<String> = None;
        for (uri, media_type, bytes) in entries {
            if scxml_script::validate_uri(&uri).is_err()
                || previous.as_ref().is_some_and(|prior| *prior >= uri)
            {
                return Err(HostError::Component("invalid-script-bundle".into()));
            }
            previous = Some(uri.clone());
            self.bundle.insert(uri, (media_type, bytes));
        }
        Ok(())
    }

    /// Resolves, authenticates, and prepares one reference (`load-script`).
    pub(crate) fn load(
        &mut self,
        reference: &ScriptReference,
        data_model: Option<&str>,
    ) -> Result<(), ComponentError> {
        let span =
            scxml_telemetry::observe(Module::HostWasmtime, Operation::HostWasmtimeScriptLoad);
        let result = self.load_inner(reference, data_model);
        if result.is_ok() {
            span.event(SignalEvent::ArtifactAccepted);
        } else {
            span.event(SignalEvent::InputRejected);
            span.fail(ErrorCategory::Validation);
        }
        result
    }

    fn load_inner(
        &mut self,
        reference: &ScriptReference,
        data_model: Option<&str>,
    ) -> Result<(), ComponentError> {
        scxml_script::validate_uri(&reference.uri).map_err(script_error)?;
        let key = ReferenceKey::of(reference);
        if let Some(loaded) = self.loaded.get(&reference.uri) {
            return if loaded.key == key {
                Ok(())
            } else {
                Err(component_error(
                    ErrorCode::InvalidInput,
                    "one script URI carries two different pins",
                ))
            };
        }
        let expected = reference
            .pinned
            .as_ref()
            .map(|pinned| pinned.sha256.as_str());
        let bundled = expected.and_then(|_| self.bundle.get(&reference.uri));
        let (media_type, bytes, from_bundle) = if let Some((media_type, bytes)) = bundled {
            (media_type.clone(), bytes.clone(), true)
        } else {
            let resolver = self.resolver.as_mut().ok_or_else(|| {
                component_error(
                    ErrorCode::Unavailable,
                    "no script loader is linked for this session",
                )
            })?;
            let fetched = resolver
                .resolve(&reference.uri)
                .map_err(|error| match error {
                    ScriptResolveError::NotFound | ScriptResolveError::Denied => component_error(
                        ErrorCode::Unavailable,
                        "script loader did not provide the script",
                    ),
                    ScriptResolveError::Platform => {
                        component_error(ErrorCode::Platform, "script loader failed")
                    }
                })?;
            (fetched.media_type, fetched.bytes, false)
        };
        if bytes.len() > self.policy.max_script_bytes {
            return Err(component_error(
                ErrorCode::PayloadTooLarge,
                "script exceeds the host script byte bound",
            ));
        }
        if let Some(pinned) = &reference.pinned
            && pinned.media_type != media_type
        {
            return Err(component_error(
                ErrorCode::InvalidInput,
                "resolved script media type differs from its pin",
            ));
        }
        let verified =
            scxml_script::verify_script(&reference.uri, &media_type, &bytes, data_model, expected)
                .map_err(script_error)?;
        if !from_bundle {
            let digest: [u8; 32] = Sha256::digest(&bytes).into();
            if !self
                .policy
                .trusted
                .iter()
                .any(|trusted| trusted.as_bytes() == &digest)
            {
                return Err(component_error(
                    ErrorCode::InvalidInput,
                    "script digest is not in the host allowlist",
                ));
            }
        }
        let kind = match verified.kind {
            scxml_script::ScriptKind::WasmComponent => LoadedKind::Component(
                wasmtime::component::Component::from_binary(&self.engine, &bytes).map_err(
                    |_| {
                        component_error(
                            ErrorCode::InvalidInput,
                            "script component failed to compile",
                        )
                    },
                )?,
            ),
            scxml_script::ScriptKind::DataModelText => LoadedKind::Text(
                String::from_utf8(bytes)
                    .map_err(|_| script_error(scxml_script::ScriptError::InvalidText))?,
            ),
        };
        self.loaded
            .insert(reference.uri.clone(), LoadedScript { key, kind });
        Ok(())
    }

    /// Runs one loaded reference (`run-script`) against `data_model`, which
    /// is moved into the script store for the call and always restored.
    pub(crate) fn run(
        &self,
        reference: &ScriptReference,
        event: Option<scxml_types::Event>,
        wit_event: Option<&WitEvent>,
        data_model: &mut Option<Box<dyn MachineDataModel>>,
        telemetry: ScriptTelemetry<'_>,
    ) -> Result<(), ComponentError> {
        let span = scxml_telemetry::observe(Module::HostWasmtime, Operation::HostWasmtimeScriptRun);
        let result = (|| {
            let loaded = self
                .loaded
                .get(&reference.uri)
                .filter(|loaded| loaded.key == ReferenceKey::of(reference))
                .ok_or_else(|| {
                    component_error(ErrorCode::Protocol, "script reference was not loaded")
                })?;
            match &loaded.kind {
                LoadedKind::Text(text) => {
                    let model = data_model.as_deref_mut().ok_or_else(|| {
                        component_error(ErrorCode::Unavailable, "no data model is linked")
                    })?;
                    model.execute(text, event.as_ref()).map_err(failure_code)
                }
                LoadedKind::Component(component) => {
                    self.run_component(component, event, wit_event, data_model, telemetry)
                }
            }
        })();
        if result.is_ok() {
            span.event(SignalEvent::TransformationCompleted);
        } else {
            span.event(SignalEvent::InputRejected);
            span.fail(ErrorCategory::Execution);
        }
        result
    }

    fn run_component(
        &self,
        component: &wasmtime::component::Component,
        event: Option<scxml_types::Event>,
        wit_event: Option<&WitEvent>,
        data_model: &mut Option<Box<dyn MachineDataModel>>,
        telemetry: ScriptTelemetry<'_>,
    ) -> Result<(), ComponentError> {
        let lease = telemetry.providers.acquire_store().map_err(|_| {
            component_error(ErrorCode::Platform, "telemetry providers are shut down")
        })?;
        let model = data_model
            .take()
            .ok_or_else(|| component_error(ErrorCode::Unavailable, "no data model is linked"))?;
        let limits = wasmtime::StoreLimitsBuilder::new()
            .memory_size(self.store_config.memory_bytes)
            .table_elements(self.store_config.table_elements)
            .instances(self.store_config.instances)
            .tables(self.store_config.tables)
            .memories(self.store_config.memories)
            .trap_on_grow_failure(true)
            .build();
        let limits =
            MeteredLimits::metered(limits, self.call_budget.as_ref(), self.policy.run_fuel);
        let budget = Arc::clone(limits.budget());
        let mut store = wasmtime::Store::new(
            &self.engine,
            ScriptStoreData {
                limits,
                data_model: model,
                event,
                providers: telemetry.providers.clone(),
                execution_context: telemetry.execution_context.clone(),
                guest_spans: crate::guest_spans::GuestSpanTable::new(),
                _store_lease: lease,
            },
        );
        store.limiter(|state| &mut state.limits);
        budget.share(&mut store);
        let outcome = (|| -> wasmtime::Result<Result<(), ComponentError>> {
            store.set_fuel(self.policy.run_fuel)?;
            let mut linker = Linker::<ScriptStoreData>::new(&self.engine);
            script_bindings::Script::add_to_linker::<_, HasSelf<ScriptStoreData>>(
                &mut linker,
                |state| state,
            )?;
            let script = script_bindings::Script::instantiate(&mut store, component, &linker)?;
            script
                .stateforward_scxml_script_api()
                .call_run(&mut store, wit_event)
        })();
        // The data model always returns to the machine store, whatever the
        // script did.
        *data_model = Some(store.into_data().data_model);
        match outcome {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(sanitize(&error)),
            // A trap (including fuel exhaustion) is a script failure: SCXML
            // raises `error.execution` and the session continues.
            Err(_) => Err(component_error(
                ErrorCode::Execution,
                "script component trapped or exhausted its fuel",
            )),
        }
    }
}

/// Keeps only the script contract's recoverable codes; any other code or a
/// malformed error is a contract violation (`protocol`).
fn sanitize(error: &ComponentError) -> ComponentError {
    match error.code {
        ErrorCode::InvalidInput if error.version == ErrorVersion::V1 => {
            component_error(ErrorCode::InvalidInput, "script rejected its input")
        }
        ErrorCode::Execution if error.version == ErrorVersion::V1 => {
            component_error(ErrorCode::Execution, "script execution failed")
        }
        _ => component_error(
            ErrorCode::Protocol,
            "script component broke the script-api error contract",
        ),
    }
}

/// Telemetry parents for one script run.
#[derive(Clone, Copy)]
pub(crate) struct ScriptTelemetry<'a> {
    pub(crate) providers: &'a TelemetryProviders,
    pub(crate) execution_context: &'a Context,
}

struct ScriptStoreData {
    limits: MeteredLimits,
    data_model: Box<dyn MachineDataModel>,
    event: Option<scxml_types::Event>,
    providers: TelemetryProviders,
    execution_context: Context,
    /// Spans the guest opened through `telemetry.start`.
    guest_spans: crate::guest_spans::GuestSpanTable,
    _store_lease: LifecycleLease,
}

impl crate::bindings::stateforward::scxml::types::Host for ScriptStoreData {}

impl data_model_host::Host for ScriptStoreData {
    fn evaluate(&mut self, expression: String) -> Result<WitDataValue, ComponentError> {
        self.data_model
            .evaluate_value(&expression, self.event.as_ref())
            .map(wit_value)
            .map_err(failure_code)
    }

    fn assign(&mut self, location: String, value: WitDataValue) -> Result<(), ComponentError> {
        native_value(value).and_then(|value| {
            self.data_model
                .assign(
                    &location,
                    &scxml_types::AssignSource::Value(value),
                    &scxml_types::AssignPlacement::ReplaceChildren,
                    self.event.as_ref(),
                )
                .map_err(failure_code)
        })
    }
}

impl telemetry::Host for ScriptStoreData {
    fn start(
        &mut self,
        operation: telemetry::Operation,
        parent: Option<WitTraceContext>,
    ) -> (u64, Option<WitTraceContext>) {
        let Ok(parent) = crate::component::parent_context(&self.execution_context, parent.as_ref())
        else {
            crate::component::guest_telemetry_refused(&self.execution_context);
            return (0, None);
        };
        let started = self.guest_spans.start(
            &self.providers,
            "scxml-component-guest",
            "script-component",
            crate::component::operation_name(operation),
            &parent,
        );
        crate::component::guest_span_started(&self.execution_context, started)
    }

    fn end(&mut self, span: u64, outcome: telemetry::Outcome) {
        let ended = self
            .guest_spans
            .end(span, crate::component::outcome_name(outcome));
        crate::component::guest_span_ended(&self.execution_context, &ended);
    }

    fn observe(
        &mut self,
        operation: telemetry::Operation,
        outcome: telemetry::Outcome,
        _measurement: Option<telemetry::Measurement>,
        context: Option<WitTraceContext>,
    ) {
        let recorded = record_guest_signal(
            &self.providers,
            &self.execution_context,
            "script-component",
            GuestSignal {
                operation: crate::component::operation_name(operation),
                outcome: Some(crate::component::outcome_name(outcome)),
                event: None,
            },
            context.as_ref(),
        );
        crate::component::guest_signal_recorded(&self.execution_context, &recorded);
    }

    fn event(
        &mut self,
        operation: telemetry::Operation,
        event: telemetry::SignalEvent,
        context: Option<WitTraceContext>,
    ) {
        let recorded = record_guest_signal(
            &self.providers,
            &self.execution_context,
            "script-component",
            GuestSignal {
                operation: crate::component::operation_name(operation),
                outcome: None,
                event: Some(crate::component::event_name(event)),
            },
            context.as_ref(),
        );
        crate::component::guest_signal_recorded(&self.execution_context, &recorded);
    }
}

#[cfg(test)]
mod tests {
    use super::MAX_SCRIPT_BUNDLE_ENTRIES;

    /// The WIT `load-script` and `script-bundle` docs state the bounds this
    /// host enforces; drift between them fails here.
    #[test]
    fn documented_script_bounds_match_enforced_constants() {
        let wit = include_str!("../../../crates/scxml-wit/wit/scxml/scxml.wit");
        let text = wit
            .lines()
            .map(|line| line.trim_start().trim_start_matches("///").trim())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            text.contains("no bound on the distinct URIs one session loads"),
            "load-script documents that the host sets no loaded-script bound"
        );
        assert!(
            text.contains(&format!("at most {MAX_SCRIPT_BUNDLE_ENTRIES} entries")),
            "script-bundle documents the entry bound"
        );
        assert!(
            text.contains(&format!("more than {MAX_SCRIPT_BUNDLE_ENTRIES} entries")),
            "script-bundle documents host rejection"
        );
        assert!(text.contains(&format!(
            "each script at most {} bytes when it is a Wasm component (every build",
            scxml_script::MAX_SCRIPT_COMPONENT_BYTES
        )));
        let documented_text_bytes: usize = 65_536;
        assert!(text.contains(&format!(
            "in a wasm build only, {documented_text_bytes} bytes when it is data-model text"
        )));
        assert_eq!(
            scxml_script::MAX_SCRIPT_TEXT_BYTES,
            scxml_types::wasm_bound(documented_text_bytes)
        );
        assert!(text.contains(&format!(
            "each `uri` a valid script URI of at most {} bytes",
            scxml_script::MAX_SCRIPT_URI_BYTES
        )));
    }
}
