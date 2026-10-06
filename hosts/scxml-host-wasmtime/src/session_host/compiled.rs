//! Compiled components shared by every session host of the process.
//!
//! Cranelift compilation dominates session-host construction (seconds for
//! the multi-megabyte compiler and ECMAScript components), while a Wasmtime
//! `Engine` and its compiled `Component`s are `Send + Sync`. This module
//! keeps one engine per engine configuration and, within it, one compiled
//! component per authenticated SHA-256, so hosts on any thread share
//! compiled code instead of recompiling it; a [`SessionHost`] itself stays
//! single-threaded.
//!
//! Only authenticated components enter the cache: the key is the digest
//! [`AuthenticatedComponent`] verified against the caller-trusted manifest,
//! so equal keys mean equal bytes. Nothing is read from or written to disk
//! and no native code is deserialized; every entry is compiled in-process
//! from authenticated bytes.
//!
//! Entries are weak. The cache never keeps an engine or compiled code alive
//! on its own: each host holds its engine and the components it compiled or
//! reused, and both are released when the last host holding them drops.
//! Requests for different digests compile in parallel.
//!
//! Concurrent requests for one digest compile it once, with one exception
//! that keeps the cache deadlock-free. Wasmtime compiles on rayon's global
//! pool, and a pool worker that waits for part of a compilation runs other
//! queued pool jobs meanwhile; if one of those jobs constructed a host and
//! waited for the in-flight compilation of the same digest, the compilation
//! could wait on that very worker forever. So a rayon worker never waits:
//! it compiles the component itself, and the first finished compilation is
//! the one every later request shares. Other threads wait for an in-flight
//! compilation, which only needs pool workers to finish.
//!
//! Each compilation (a cache miss; a hit emits nothing) is one
//! `component-compile` span with the closed `scxml.outcome` label on the
//! global tracer provider, not on any host's providers: the cache is
//! process-wide, so one compilation can serve hosts with different
//! providers, and whether a request misses depends on which other hosts
//! are alive. Recording it on a host's providers would make that host's
//! (possibly deterministic) span-id sequence depend on the rest of the
//! process.
//!
//! [`SessionHost`]: super::SessionHost

use std::cell::OnceCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

use opentelemetry::trace::{Span as _, Status, Tracer as _};
use opentelemetry::{KeyValue, global};
use wasmtime::component::Component;

use crate::{AuthenticatedComponent, HostError, WasmtimeHostConfig};

/// Live shared engines, at most one per engine configuration.
static ENGINES: Mutex<Vec<Weak<SharedEngine>>> = Mutex::new(Vec::new());

/// One engine and the components compiled on it.
pub(crate) struct SharedEngine {
    engine: wasmtime::Engine,
    /// The only [`WasmtimeHostConfig`] input of [`crate::component_engine`];
    /// hosts whose configurations agree on it share this engine.
    max_wasm_stack_bytes: usize,
    compiled: Mutex<HashMap<[u8; 32], Weak<CompiledSlot>>>,
}

/// One authenticated component's compiled form, published once.
struct CompiledSlot {
    /// Held by a non-pool thread while it compiles, so other non-pool
    /// threads wait for it instead of compiling again. Never taken by a
    /// rayon worker.
    compiling: Mutex<()>,
    component: OnceLock<Component>,
    /// Compilations this slot ran (a race on rayon workers can run more
    /// than one; only the first is published).
    #[cfg(test)]
    compilations: std::sync::atomic::AtomicUsize,
}

/// A compiled component a host holds; keeps the shared entry alive.
pub(crate) struct Compiled {
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "held only to keep the shared cache entry alive")
    )]
    slot: Arc<CompiledSlot>,
    component: Component,
}

impl Compiled {
    pub(crate) fn component(&self) -> &Component {
        &self.component
    }
}

impl SharedEngine {
    /// The live engine for `config`, or a new one (a
    /// `component-engine` span).
    pub(crate) fn for_config(config: &WasmtimeHostConfig) -> Result<Arc<Self>, HostError> {
        let mut engines = ENGINES.lock().unwrap_or_else(PoisonError::into_inner);
        engines.retain(|entry| entry.strong_count() > 0);
        if let Some(shared) = engines
            .iter()
            .filter_map(Weak::upgrade)
            .find(|shared| shared.max_wasm_stack_bytes == config.max_wasm_stack_bytes)
        {
            return Ok(shared);
        }
        let shared = Arc::new(Self {
            engine: crate::component_engine(config)?,
            max_wasm_stack_bytes: config.max_wasm_stack_bytes,
            compiled: Mutex::new(HashMap::new()),
        });
        engines.push(Arc::downgrade(&shared));
        Ok(shared)
    }

    pub(crate) fn engine(&self) -> &wasmtime::Engine {
        &self.engine
    }

    /// `component` compiled on this engine: shared when any live host
    /// already compiled the same digest, else compiled now (one
    /// `component-compile` span).
    pub(crate) fn compile(
        &self,
        component: &AuthenticatedComponent,
    ) -> Result<Compiled, HostError> {
        let slot = {
            let mut compiled = self.compiled.lock().unwrap_or_else(PoisonError::into_inner);
            compiled.retain(|_, entry| entry.strong_count() > 0);
            let digest = component.digest();
            if let Some(slot) = compiled.get(&digest).and_then(Weak::upgrade) {
                slot
            } else {
                let slot = Arc::new(CompiledSlot {
                    compiling: Mutex::new(()),
                    component: OnceLock::new(),
                    #[cfg(test)]
                    compilations: std::sync::atomic::AtomicUsize::new(0),
                });
                compiled.insert(digest, Arc::downgrade(&slot));
                slot
            }
        };
        let compiled = slot.get_or_compile(&self.engine, component)?;
        Ok(Compiled {
            slot,
            component: compiled,
        })
    }
}

impl CompiledSlot {
    fn get_or_compile(
        &self,
        engine: &wasmtime::Engine,
        component: &AuthenticatedComponent,
    ) -> Result<Component, HostError> {
        if let Some(compiled) = self.component.get() {
            return Ok(compiled.clone());
        }
        let _compiling = if rayon::current_thread_index().is_some() {
            None
        } else {
            let guard = self
                .compiling
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(compiled) = self.component.get() {
                return Ok(compiled.clone());
            }
            Some(guard)
        };
        #[cfg(test)]
        self.compilations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut span = global::tracer("scxml-host-wasmtime").start("component-compile");
        let result = Component::from_binary(engine, component.bytes());
        if result.is_err() {
            span.set_status(Status::error("failed"));
        }
        span.set_attribute(KeyValue::new(
            "scxml.outcome",
            if result.is_ok() { "ok" } else { "failed" },
        ));
        span.end();
        let compiled = result?;
        Ok(self.component.get_or_init(|| compiled).clone())
    }
}

/// A component a host compiles on first use, then holds for its lifetime.
pub(crate) struct LazyComponent {
    source: AuthenticatedComponent,
    compiled: OnceCell<Compiled>,
}

impl LazyComponent {
    pub(crate) fn new(source: &AuthenticatedComponent) -> Self {
        Self {
            source: source.clone(),
            compiled: OnceCell::new(),
        }
    }

    /// The compiled component, compiling (or reusing a shared compilation)
    /// on the first call.
    pub(crate) fn get(&self, shared: &SharedEngine) -> Result<&Component, HostError> {
        if let Some(compiled) = self.compiled.get() {
            return Ok(compiled.component());
        }
        let compiled = shared.compile(&self.source)?;
        Ok(self.compiled.get_or_init(|| compiled).component())
    }
}

#[cfg(test)]
mod tests {
    use sha2::{Digest as _, Sha256};

    use std::sync::atomic::Ordering;

    use super::*;
    use crate::{TrustedComponentDigest, authenticate_packaged_machine};

    fn compilations(compiled: &Compiled) -> usize {
        compiled.slot.compilations.load(Ordering::Relaxed)
    }

    /// An authenticated empty component (`(component)`): the smallest input
    /// Wasmtime compiles.
    fn empty_component() -> AuthenticatedComponent {
        const EMPTY_COMPONENT: &[u8] = b"\0asm\x0d\0\x01\0";
        let digest = crate::artifact::lower_hex(&Sha256::digest(EMPTY_COMPONENT));
        authenticate_packaged_machine(
            EMPTY_COMPONENT,
            TrustedComponentDigest::from_hex(&digest).expect("the digest is canonical"),
        )
        .expect("the digest matches")
    }

    /// A stack size no other test in this process uses, so the engine
    /// registry entry belongs to one test alone.
    fn private_config(stack_kib: usize) -> WasmtimeHostConfig {
        WasmtimeHostConfig {
            max_wasm_stack_bytes: stack_kib * 1024,
            ..WasmtimeHostConfig::default()
        }
    }

    #[test]
    fn hosts_with_one_engine_configuration_share_one_engine() {
        let first = SharedEngine::for_config(&private_config(257)).expect("engine");
        let second = SharedEngine::for_config(&private_config(257)).expect("engine");
        let other = SharedEngine::for_config(&private_config(258)).expect("engine");
        assert!(Arc::ptr_eq(&first, &second));
        assert!(!Arc::ptr_eq(&first, &other));
    }

    #[test]
    fn one_digest_compiles_once_while_held_and_is_released_with_its_last_holder() {
        let shared = SharedEngine::for_config(&private_config(259)).expect("engine");
        let component = empty_component();
        let first = shared.compile(&component).expect("compiles");
        let second = shared.compile(&component).expect("shared");
        assert!(Arc::ptr_eq(&first.slot, &second.slot));
        assert_eq!(compilations(&first), 1, "the second request is a hit");
        let weak = Arc::downgrade(&first.slot);
        drop((first, second));
        assert!(weak.upgrade().is_none(), "the cache keeps nothing alive");
        let again = shared.compile(&component).expect("recompiles");
        assert_eq!(Arc::strong_count(&again.slot), 1);
        assert_eq!(compilations(&again), 1, "a released entry compiles anew");
    }

    #[test]
    fn concurrent_hosts_share_one_compilation_across_threads() {
        let shared = SharedEngine::for_config(&private_config(260)).expect("engine");
        let component = empty_component();
        let compiled: Vec<Compiled> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..4)
                .map(|_| scope.spawn(|| shared.compile(&component).expect("compiles")))
                .collect();
            workers
                .into_iter()
                .map(|worker| worker.join().expect("worker finishes"))
                .collect()
        });
        assert!(
            compiled
                .windows(2)
                .all(|pair| Arc::ptr_eq(&pair[0].slot, &pair[1].slot))
        );
        assert_eq!(
            compilations(&compiled[0]),
            1,
            "threads outside the pool wait for the one compilation"
        );
    }

    #[test]
    fn pool_workers_never_wait_and_share_the_first_published_compilation() {
        let shared = SharedEngine::for_config(&private_config(262)).expect("engine");
        let component = empty_component();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .expect("pool");
        let compiled: Vec<Compiled> = pool.install(|| {
            use rayon::prelude::*;
            (0..16)
                .into_par_iter()
                .map(|_| shared.compile(&component).expect("compiles"))
                .collect()
        });
        assert!(
            compiled
                .windows(2)
                .all(|pair| Arc::ptr_eq(&pair[0].slot, &pair[1].slot))
        );
        let runs = compilations(&compiled[0]);
        assert!(
            (1..=16).contains(&runs),
            "every request compiles at most once: {runs}"
        );
        let published = compiled[0].slot.component.get().expect("published");
        assert!(
            compiled
                .iter()
                .all(|held| Component::same(held.component(), published)),
            "every holder shares the first published compilation"
        );
    }

    #[test]
    fn a_lazy_component_compiles_on_first_use_only() {
        let shared = SharedEngine::for_config(&private_config(261)).expect("engine");
        let lazy = LazyComponent::new(&empty_component());
        assert!(lazy.compiled.get().is_none());
        let first: *const Component = lazy.get(&shared).expect("compiles");
        let second: *const Component = lazy.get(&shared).expect("held");
        assert_eq!(first, second);
        let held = lazy.compiled.get().expect("held after first use");
        assert_eq!(compilations(held), 1);
    }
}
