//! Build-time pre-initialization of a component with Wizer.
//!
//! [`wizen_component`] instantiates a component once, calls its
//! `wizer-initialize` export, and returns the component rewritten so that
//! every later instance starts from the memories and globals that call left
//! (Wizer's snapshot), with the `wizer-initialize` export removed. A
//! component whose initialization builds the same state in every instance
//! (the ECMAScript data model's realm) then starts without rebuilding it.
//!
//! # Contract
//!
//! * Precondition: `component` is a component Wizer accepts (no nested
//!   components with modules, each core module instantiated at most once)
//!   that exports `wizer-initialize: func()`. Its initialization may call
//!   only `wasi:random/insecure-seed`, and only when `insecure_seed` names
//!   a provider component, whose export it is answered from; every other
//!   import traps, so initialization that reaches one fails the build
//!   instead of capturing host state in the snapshot.
//! * Bound: initialization runs on at most [`WIZEN_FUEL`] instructions, the
//!   fuel of one data-model call, and the stores' default resource limits.
//! * Postcondition: the output is a valid component with the input's
//!   imports and every export but `wizer-initialize`.
//! * Determinism: the output depends only on the input bytes and the
//!   provider's seed: initialization sees no clock, randomness, or
//!   environment of the build host.
//! * Failure: a typed [`HostError`]; no partial output.
//!
//! Telemetry: [`wizen_component`] is the boundary and opens one
//! `scxml.host-wasmtime.wizen` span.

use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent};
use wasmtime::component::{Component, Instance, Linker, WasmList};
use wasmtime_wizer::{ComponentInstanceState, Wizer};

use crate::{DEFAULT_DATA_MODEL_CALL_FUEL, HostError, WasmtimeHostConfig, component_engine};

/// Fuel of the `wizer-initialize` call: the per-call fuel of a data-model
/// call, which today creates the same realm inside `initialize`.
pub const WIZEN_FUEL: u64 = DEFAULT_DATA_MODEL_CALL_FUEL;

/// Wizer's initialization export.
const INIT_FUNC: &str = "wizer-initialize";

/// The interface a seed provider answers, without its version.
const INSECURE_SEED: &str = "wasi:random/insecure-seed@";

/// Pre-initializes `component` (see the module contract); `insecure_seed`
/// is the provider component that answers `wasi:random/insecure-seed`
/// while initialization runs.
///
/// # Errors
///
/// [`HostError::Wasmtime`] when Wizer rejects the component, the provider
/// exports no `wasi:random/insecure-seed`, compilation or instantiation
/// fails, initialization traps (an import other than the seed, or fuel),
/// or reading the snapshot fails.
pub fn wizen_component(
    component: &[u8],
    insecure_seed: Option<&[u8]>,
) -> Result<Vec<u8>, HostError> {
    let span = scxml_telemetry::observe(Module::HostWasmtime, Operation::HostWasmtimeWizen);
    let result = wizen(component, insecure_seed);
    if result.is_ok() {
        span.event(SignalEvent::TransformationCompleted);
    } else {
        span.event(SignalEvent::InputRejected);
        span.fail(ErrorCategory::Packaging);
    }
    result
}

fn wizen(component: &[u8], insecure_seed: Option<&[u8]>) -> Result<Vec<u8>, HostError> {
    let engine = component_engine(&WasmtimeHostConfig::default())?;
    let mut linker = Linker::<()>::new(&engine);
    if let Some(provider) = insecure_seed {
        let (name, seed) = read_seed(&engine, provider)?;
        linker
            .instance(&name)?
            .func_wrap("insecure-seed", move |_store, (): ()| Ok((seed,)))?;
    }
    // Wizer would remove the export from the core module and the component
    // both, and the component's lift of it would then name a core export
    // that no longer exists; the component's export alone is removed below.
    let mut wizer = Wizer::new();
    wizer.keep_init_func(true);
    let (context, instrumented) = wizer.instrument_component(component)?;
    let compiled = Component::new(&engine, &instrumented)?;
    linker.define_unknown_imports_as_traps(&compiled)?;
    let mut store = wasmtime::Store::new(&engine, ());
    store.set_fuel(WIZEN_FUEL)?;
    let instance = linker.instantiate(&mut store, &compiled)?;
    instance
        .get_typed_func::<(), ()>(&mut store, INIT_FUNC)?
        .call(&mut store, ())?;
    let mut state = Snapshot {
        store: &mut store,
        instance,
        failure: None,
    };
    let output = crate::service::block_on(wizer.snapshot_component(&context, &mut state));
    if let Some(error) = state.failure {
        return Err(error.into());
    }
    let output = without_export(&output?, INIT_FUNC)?;
    // The output must compile, and its lift of the core export must
    // resolve, before it is returned.
    Component::new(&engine, &output)?;
    Ok(output)
}

/// `component` without its top-level export `name` (a function); every
/// later reference to a component function index is renumbered. Core
/// modules and nested components are copied as they are.
fn without_export(component: &[u8], name: &str) -> Result<Vec<u8>, HostError> {
    use wasmparser::{Chunk, Payload};
    let mut encoder = wasm_encoder::Component::new();
    let mut renumber = Renumber {
        funcs: 0,
        removed: None,
        name,
    };
    let mut parser = wasmparser::Parser::new(0);
    let mut offset = 0;
    loop {
        let rest = component.get(offset..).unwrap_or_default();
        let (consumed, payload) = match parser.parse(rest, true).map_err(malformed)? {
            Chunk::Parsed { consumed, payload } => (consumed, payload),
            Chunk::NeedMoreData(_) => {
                return Err(wasmtime::Error::msg("truncated component").into());
            }
        };
        offset += consumed;
        let Some(other) = renumber.reencode(&mut encoder, payload)? else {
            continue;
        };
        match other {
            Payload::End(_) => break,
            Payload::Version { .. } => {}
            Payload::ComponentStartSection { .. } => {
                return Err(
                    wasmtime::Error::msg("component start functions are not supported").into(),
                );
            }
            other => {
                let (id, range) = other
                    .as_section()
                    .ok_or_else(|| wasmtime::Error::msg("unexpected component payload"))?;
                let range = usize::try_from(range.start).map_err(wasmtime::Error::msg)?
                    ..usize::try_from(range.end).map_err(wasmtime::Error::msg)?;
                let data = component
                    .get(range.clone())
                    .ok_or_else(|| wasmtime::Error::msg("section out of range"))?;
                encoder.section(&wasm_encoder::RawSection { id, data });
                // The parser steps over a nested module or component
                // itself; its bytes are past what it reports consumed.
                if matches!(
                    other,
                    Payload::ModuleSection { .. } | Payload::ComponentSection { .. }
                ) {
                    offset = range.end;
                }
            }
        }
    }
    if renumber.removed.is_none() {
        return Err(wasmtime::Error::msg(format!("the component exports no {name}")).into());
    }
    Ok(encoder.finish())
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the map_err adapter of wasmparser's error"
)]
fn malformed(error: wasmparser::BinaryReaderError) -> wasmtime::Error {
    wasmtime::Error::msg(error.to_string())
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the map_err adapter of wasm-encoder's error"
)]
fn reencoding(error: wasm_encoder::reencode::Error) -> wasmtime::Error {
    wasmtime::Error::msg(error.to_string())
}

/// Renumbers component function indices past the removed export's.
struct Renumber<'a> {
    /// Component functions defined so far.
    funcs: u32,
    /// The index the removed export defined.
    removed: Option<u32>,
    name: &'a str,
}

impl Renumber<'_> {
    /// Re-encodes `payload` into `encoder` when it is a section that
    /// defines or refers to component functions, counting the functions it
    /// defines; any other payload is handed back.
    fn reencode<'a>(
        &mut self,
        encoder: &mut wasm_encoder::Component,
        payload: wasmparser::Payload<'a>,
    ) -> Result<Option<wasmparser::Payload<'a>>, HostError> {
        use wasm_encoder::reencode::component_utils as utils;
        use wasmparser::{ComponentExternalKind, Payload};
        match payload {
            Payload::ComponentAliasSection(reader) => {
                let mut section = wasm_encoder::ComponentAliasSection::new();
                for alias in reader.clone() {
                    if let wasmparser::ComponentAlias::InstanceExport {
                        kind: ComponentExternalKind::Func,
                        ..
                    } = alias.map_err(malformed)?
                    {
                        self.funcs += 1;
                    }
                }
                utils::parse_component_alias_section(self, &mut section, reader)
                    .map_err(reencoding)?;
                encoder.section(&section);
            }
            Payload::ComponentCanonicalSection(reader) => {
                let mut section = wasm_encoder::CanonicalFunctionSection::new();
                for canonical in reader.clone() {
                    if let wasmparser::CanonicalFunction::Lift { .. } =
                        canonical.map_err(malformed)?
                    {
                        self.funcs += 1;
                    }
                }
                utils::parse_component_canonical_section(self, &mut section, reader)
                    .map_err(reencoding)?;
                encoder.section(&section);
            }
            Payload::ComponentImportSection(reader) => {
                let mut section = wasm_encoder::ComponentImportSection::new();
                for import in reader.clone() {
                    if import.map_err(malformed)?.ty.kind() == ComponentExternalKind::Func {
                        self.funcs += 1;
                    }
                }
                utils::parse_component_import_section(self, &mut section, reader)
                    .map_err(reencoding)?;
                encoder.section(&section);
            }
            Payload::ComponentInstanceSection(reader) => {
                let mut section = wasm_encoder::ComponentInstanceSection::new();
                utils::parse_component_instance_section(self, &mut section, reader)
                    .map_err(reencoding)?;
                encoder.section(&section);
            }
            Payload::ComponentExportSection(reader) => {
                let mut section = wasm_encoder::ComponentExportSection::new();
                for export in reader {
                    let export = export.map_err(malformed)?;
                    let func = export.kind == ComponentExternalKind::Func;
                    if func && export.name.name == self.name && self.removed.is_none() {
                        self.removed = Some(self.funcs);
                    } else {
                        if func {
                            self.funcs += 1;
                        }
                        utils::parse_component_export(self, &mut section, export)
                            .map_err(reencoding)?;
                    }
                }
                encoder.section(&section);
            }
            other => return Ok(Some(other)),
        }
        Ok(None)
    }
}

impl wasm_encoder::reencode::Reencode for Renumber<'_> {
    type Error = std::convert::Infallible;
}

impl wasm_encoder::reencode::ReencodeComponent for Renumber<'_> {
    fn component_func_index(&mut self, index: u32) -> u32 {
        match self.removed {
            Some(removed) if index > removed => index - 1,
            _ => index,
        }
    }
}

/// The seed `provider` answers, and the exact name of the interface it
/// exports.
fn read_seed(
    engine: &wasmtime::Engine,
    provider: &[u8],
) -> Result<(String, (u64, u64)), HostError> {
    let compiled = Component::new(engine, provider)?;
    let name = compiled
        .component_type()
        .exports(engine)
        .map(|(name, _)| name)
        .find(|name| name.starts_with(INSECURE_SEED))
        .map(str::to_owned)
        .ok_or_else(|| wasmtime::format_err!("the seed provider exports no {INSECURE_SEED}*"))?;
    let mut store = wasmtime::Store::new(engine, ());
    store.set_fuel(WIZEN_FUEL)?;
    let instance = Linker::<()>::new(engine).instantiate(&mut store, &compiled)?;
    let interface = instance
        .get_export_index(&mut store, None, &name)
        .ok_or_else(|| wasmtime::format_err!("the seed provider's {name} is missing"))?;
    let function = instance
        .get_export_index(&mut store, Some(&interface), "insecure-seed")
        .ok_or_else(|| wasmtime::format_err!("{name} exports no insecure-seed"))?;
    let (seed,) = instance
        .get_typed_func::<(), ((u64, u64),)>(&mut store, &function)?
        .call(&mut store, ())?;
    Ok((name, seed))
}

/// The instance Wizer reads its snapshot from; the first failed read is
/// kept, since Wizer's reads return no error.
struct Snapshot<'a> {
    store: &'a mut wasmtime::Store<()>,
    instance: Instance,
    failure: Option<wasmtime::Error>,
}

impl Snapshot<'_> {
    fn call<R: wasmtime::component::ComponentType + wasmtime::component::Lift + 'static, V>(
        &mut self,
        instance: &str,
        func: &str,
        read: impl FnOnce(&mut wasmtime::Store<()>, R) -> V,
    ) -> Option<V> {
        let result = (|| {
            let interface = self
                .instance
                .get_export_index(&mut *self.store, None, instance)
                .ok_or_else(|| {
                    wasmtime::format_err!("the snapshot export {instance} is missing")
                })?;
            let function = self
                .instance
                .get_export_index(&mut *self.store, Some(&interface), func)
                .ok_or_else(|| wasmtime::format_err!("the snapshot export {func} is missing"))?;
            let typed = self
                .instance
                .get_typed_func::<(), (R,)>(&mut *self.store, &function)?;
            let (value,) = typed.call(&mut *self.store, ())?;
            Ok(read(self.store, value))
        })();
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.failure.get_or_insert(error);
                None
            }
        }
    }
}

// Every read is a synchronous call, so each answer is ready at once.
impl ComponentInstanceState for Snapshot<'_> {
    fn call_func_ret_list_u8(
        &mut self,
        instance: &str,
        func: &str,
        contents: impl FnOnce(&[u8]) + Send,
    ) -> impl Future<Output = ()> + Send {
        let read = |store: &mut wasmtime::Store<()>, list: WasmList<u8>| {
            contents(list.as_le_slice(&*store));
        };
        if self.call(instance, func, read).is_none() {
            // Recorded in `failure`; the snapshot is discarded.
        }
        std::future::ready(())
    }

    fn call_func_ret_s32(
        &mut self,
        instance: &str,
        func: &str,
    ) -> impl Future<Output = i32> + Send {
        std::future::ready(
            self.call(instance, func, |_, value: i32| value)
                .unwrap_or(0),
        )
    }

    fn call_func_ret_s64(
        &mut self,
        instance: &str,
        func: &str,
    ) -> impl Future<Output = i64> + Send {
        std::future::ready(
            self.call(instance, func, |_, value: i64| value)
                .unwrap_or(0),
        )
    }

    fn call_func_ret_f32(
        &mut self,
        instance: &str,
        func: &str,
    ) -> impl Future<Output = u32> + Send {
        std::future::ready(
            self.call(instance, func, |_, value: f32| value.to_bits())
                .unwrap_or(0),
        )
    }

    fn call_func_ret_f64(
        &mut self,
        instance: &str,
        func: &str,
    ) -> impl Future<Output = u64> + Send {
        std::future::ready(
            self.call(instance, func, |_, value: f64| value.to_bits())
                .unwrap_or(0),
        )
    }
}
