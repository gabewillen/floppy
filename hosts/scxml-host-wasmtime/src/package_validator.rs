//! Package validator components (`stateforward:scxml-validator/package-validator`).
//!
//! An extension or service package may ship a validator component (manifest
//! `validator-artifact` and `validator-sha256`) that checks what the
//! package's XSD cannot. This module is the host adapter: [`PackageValidators`]
//! binds each one, authenticated by its digest, to the package's exact URI
//! (an extension namespace or a service type URI) and runs each call in a
//! fresh store bounded like the validator's (memory, table, and instance
//! limits, and the configured fuel). The world imports nothing, so a package
//! validator gets no WASI, no I/O, and no clock: its answer depends only on
//! its input. The host stays a thin linker: it never parses SCXML; the
//! validator (native or component) builds the document context, chooses the
//! targets, and checks every returned span and edit itself.
//!
//! Routing for the validator component's `package-validator-api` import
//! ([`crate::ComponentValidator::with_package_validators`]): a call goes to
//! the package bound to its first target's exact namespace URI, or, for an
//! SCXML `<invoke>`, to its literal `type`; a package with no bound
//! validator answers an empty list.
//!
//! Telemetry: one closed `scxml.host-wasmtime.package-validator-validate`
//! span per call that reaches a bound component, with its closed outcome; the
//! package URI, document, and diagnostics never become attributes.

use std::collections::BTreeMap;

use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent};
use wasmtime::component::Linker;

use crate::memory_growth::MeteredLimits;
use crate::validator::validator_bindings::stateforward::scxml_validator::{document, types};
use crate::{
    ArtifactError, ArtifactKind, AuthenticatedComponent, HostError, LifecycleLease,
    TelemetryProviders, WasmtimeHostConfig, component_engine, record_store_limits,
};

#[allow(
    missing_docs,
    reason = "generated WIT bindings; the canonical documentation is validator.wit"
)]
mod bindings {
    wasmtime::component::bindgen!({
        path: "../../crates/scxml-wit/wit/validator",
        world: "package-validator",
        with: {
            "stateforward:scxml-validator/types": crate::validator::validator_bindings::stateforward::scxml_validator::types,
            "stateforward:scxml-validator/document": crate::validator::validator_bindings::stateforward::scxml_validator::document,
        },
    });
}

/// The SCXML namespace, whose `<invoke>` elements route by their `type`.
const SCXML_NAMESPACE: &str = "http://www.w3.org/2005/07/scxml";

/// The types-only interfaces a package validator imports for its export's
/// record types; each is linked as an empty instance.
const TYPE_INTERFACES: [&str; 2] = [
    "stateforward:scxml-validator/types@0.2.0",
    "stateforward:scxml-validator/document@0.2.0",
];

/// Store state of one package validator call.
struct PackageValidatorStoreData {
    limits: MeteredLimits,
    _store_lease: LifecycleLease,
}

/// Package validator components bound by exact package URI. Single owner;
/// calls are synchronous and share no guest state.
pub struct PackageValidators {
    engine: wasmtime::Engine,
    linker: Linker<PackageValidatorStoreData>,
    bindings: BTreeMap<String, wasmtime::component::Component>,
    config: WasmtimeHostConfig,
    providers: TelemetryProviders,
}

impl std::fmt::Debug for PackageValidators {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PackageValidators")
            .field("bindings", &self.bindings.len())
            .finish_non_exhaustive()
    }
}

impl PackageValidators {
    /// No bindings; every call is bounded by `config`.
    ///
    /// # Errors
    ///
    /// Invalid host limits or engine configuration.
    pub fn new(
        config: &WasmtimeHostConfig,
        providers: &TelemetryProviders,
    ) -> Result<Self, HostError> {
        config.validate()?;
        let engine = component_engine(config)?;
        let mut linker = Linker::new(&engine);
        for interface in TYPE_INTERFACES {
            linker.instance(interface)?;
        }
        Ok(Self {
            linker,
            engine,
            bindings: BTreeMap::new(),
            config: config.clone(),
            providers: providers.clone(),
        })
    }

    /// Binds `component` (an authenticated
    /// [`ArtifactKind::PackageValidatorComponent`], or the release's
    /// [`ArtifactKind::PackageValidatorExample`]) to the exact package URI
    /// `package`.
    ///
    /// # Errors
    ///
    /// [`ArtifactError::ArtifactKindMismatch`] for another kind,
    /// [`ArtifactError::ArtifactTooLarge`] over the component bound,
    /// [`ArtifactError::NamespaceBinding`] for a URI already bound,
    /// [`ArtifactError::UnknownImport`] for a component that imports
    /// anything but the types-only `types` and `document` interfaces, and
    /// compile failures.
    pub fn bind(
        &mut self,
        package: &str,
        component: &AuthenticatedComponent,
    ) -> Result<(), HostError> {
        if !matches!(
            component.kind(),
            ArtifactKind::PackageValidatorComponent | ArtifactKind::PackageValidatorExample
        ) {
            return Err(ArtifactError::ArtifactKindMismatch.into());
        }
        if component.bytes().len() > self.config.component_bytes {
            return Err(ArtifactError::ArtifactTooLarge.into());
        }
        if self.bindings.contains_key(package) {
            return Err(ArtifactError::NamespaceBinding.into());
        }
        let compiled =
            wasmtime::component::Component::from_binary(&self.engine, component.bytes())?;
        // The world imports nothing; its export's record types arrive as
        // imports of the types-only `types` and `document` interfaces, which
        // carry no function and no resource.
        let types_only = compiled.component_type().imports(&self.engine).all(|(name, item)| {
            TYPE_INTERFACES.contains(&name)
                && matches!(item.ty, wasmtime::component::types::ComponentItem::ComponentInstance(instance)
                    if instance.exports(&self.engine).all(|(_, export)| {
                        matches!(export.ty, wasmtime::component::types::ComponentItem::Type(_))
                    }))
        });
        if !types_only {
            return Err(ArtifactError::UnknownImport.into());
        }
        self.bindings.insert(package.to_owned(), compiled);
        Ok(())
    }

    /// Whether a validator is bound to `package`.
    #[must_use]
    pub fn is_bound(&self, package: &str) -> bool {
        self.bindings.contains_key(package)
    }

    /// Runs the validator bound to `package` over `targets` in a fresh
    /// bounded store; an empty list when none is bound.
    ///
    /// # Errors
    ///
    /// A trap (fuel exhaustion included), an instantiation failure, or the
    /// component's `err` ([`HostError::Component`]). Nothing is returned on
    /// failure.
    pub fn validate(
        &self,
        package: &str,
        document: &document::Context,
        targets: &[u32],
    ) -> Result<Vec<types::Diagnostic>, HostError> {
        let Some(component) = self.bindings.get(package) else {
            return Ok(Vec::new());
        };
        let span = scxml_telemetry::observe(
            Module::HostWasmtime,
            Operation::HostWasmtimePackageValidatorValidate,
        );
        let result = self.call(component, document, targets);
        match &result {
            Ok(_) => span.event(SignalEvent::ArtifactProduced),
            Err(HostError::Component(_)) => {
                span.event(SignalEvent::InputRejected);
                span.fail(ErrorCategory::Validation);
            }
            Err(_) => span.fail(ErrorCategory::Platform),
        }
        result
    }

    /// The package a call with `targets` belongs to: the first target's
    /// namespace URI, or the literal `type` of an SCXML `<invoke>`.
    #[must_use]
    pub fn route<'a>(document: &'a document::Context, targets: &[u32]) -> Option<&'a str> {
        let element = document
            .elements
            .get(usize::try_from(*targets.first()?).ok()?)?;
        let namespace = element.name.namespace_uri.as_deref()?;
        if namespace == SCXML_NAMESPACE && element.name.local_name == "invoke" {
            return element
                .attributes
                .iter()
                .find(|attribute| {
                    attribute.name.namespace_uri.is_none() && attribute.name.local_name == "type"
                })
                .map(|attribute| attribute.value.as_str());
        }
        Some(namespace)
    }

    fn call(
        &self,
        component: &wasmtime::component::Component,
        document: &document::Context,
        targets: &[u32],
    ) -> Result<Vec<types::Diagnostic>, HostError> {
        let config = &self.config;
        let limits = wasmtime::StoreLimitsBuilder::new()
            .memory_size(config.store.memory_bytes)
            .table_elements(config.store.table_elements)
            .instances(config.store.instances)
            .tables(config.store.tables)
            .memories(config.store.memories)
            .trap_on_grow_failure(true)
            .build();
        let limits = MeteredLimits::metered(limits, None, config.store.fuel);
        let budget = std::sync::Arc::clone(limits.budget());
        let mut store = wasmtime::Store::new(
            &self.engine,
            PackageValidatorStoreData {
                limits,
                _store_lease: self.providers.acquire_store()?,
            },
        );
        store.limiter(|state| &mut state.limits);
        budget.share(&mut store);
        store.set_fuel(config.store.fuel)?;
        record_store_limits(&config.store);
        let instance =
            bindings::PackageValidator::instantiate(&mut store, component, &self.linker)?;
        instance
            .stateforward_scxml_validator_package_validator_api()
            .call_validate(&mut store, document, targets)?
            .map_err(|_| HostError::Component("package-validator-failed".into()))
    }
}
