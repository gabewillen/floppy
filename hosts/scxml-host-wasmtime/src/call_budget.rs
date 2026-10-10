//! One fuel budget per host call, shared by every store the call reaches.
//!
//! Wasmtime meters fuel per store, but one session-host call crosses many
//! stores synchronously: a machine call runs data-model, I/O processor,
//! clock, `BasicHTTP`, compiler, script, and child-machine calls from inside
//! its own capability calls. Refuelling each nested store independently would
//! let one machine call run any number of full nested budgets, so the session
//! host threads one [`CallBudget`] through every store it creates instead.
//!
//! The outermost entry into any of those stores (a call with no Wasm of the
//! host below it on the stack) starts the budget at the configured fuel,
//! unless the host holds the budget: [`CallBudget::hold`] makes every call
//! its body makes, one after another, one host call. The session host holds
//! it over each session command, so a machine call that suspends at a
//! deferred `<invoke>` start or cancel, every child start, cancel, and
//! continuation the host carries out for it, and the call's own
//! continuation spend one budget, as the same work done inside one
//! uninterrupted call would. [`CallBudget::apart`] runs its body on a
//! budget of its own instead: the session host compiles each document its
//! embedder serves that way, once per run.
//! From then on every store runs on what is left of it: entering a store's
//! Wasm grants the smaller of the store's own per-call cap (the fuel its call
//! site set, such as the data-model or script per-call fuel) and the
//! remaining budget; leaving it, or calling out of it into the host, charges
//! what it executed; returning into it grants it the smaller of its own
//! remainder and the remaining budget. So the instructions executed by one
//! host call, across every component it reaches, never exceed the configured
//! fuel, and the count depends only on the executed path.
//!
//! Wasmtime's call hooks carry the accounting; they see every transition
//! between host and Wasm of a store, including the guest `realloc` calls
//! Wasmtime makes while lowering a host result.
//!
//! Each host call also gets a separate allowance for the host's linear
//! memory growth copies (`crate::memory_growth`): its fuel times
//! [`crate::MEMORY_GROWTH_COPY_BYTES_PER_FUEL`] bytes, started and shared
//! exactly as the fuel is. A copy never spends Wasm fuel, so a growth never
//! needs the store's fuel lowered across it (Wasmtime runs a growth as a host
//! transition but keeps the growing activation's own fuel count, and a fuel
//! change made there would be lost), and the Wasm an activation runs after a
//! growth is charged like any other.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use wasmtime::{CallHook, Store};

/// The copy allowance of a host call of `fuel`.
fn copy_allowance(fuel: u64) -> u64 {
    fuel.saturating_mul(crate::MEMORY_GROWTH_COPY_BYTES_PER_FUEL)
}

/// The fuel one host call spent, by kind of work.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CallFuel {
    /// Fuel of the Wasm instructions the call executed.
    pub wasm: u64,
    /// Bytes the call's linear memory growth copies used of its copy
    /// allowance.
    pub memory_growth_bytes: u64,
}

impl CallFuel {
    /// What was spent after `earlier`, a reading of the same budget.
    pub(crate) const fn since(self, earlier: Self) -> Self {
        Self {
            wasm: self.wasm.saturating_sub(earlier.wasm),
            memory_growth_bytes: self
                .memory_growth_bytes
                .saturating_sub(earlier.memory_growth_bytes),
        }
    }
}

/// The fuel budget of one host call across every store that shares it.
pub(crate) struct CallBudget {
    /// The configured fuel of one outermost host call.
    fuel: AtomicU64,
    /// What the current outermost call has left.
    remaining: AtomicU64,
    /// What the current outermost call may still copy for memory growth.
    copies_left: AtomicU64,
    /// Fuel the Wasm of every call on this budget executed.
    wasm: AtomicU64,
    /// Bytes the memory growth copies of every call on this budget used.
    memory_growth_bytes: AtomicU64,
    /// Wasm activations of sharing stores currently on the stack.
    depth: AtomicU32,
    /// Active [`CallBudget::hold`] scopes.
    held: AtomicU32,
}

/// Ends one [`CallBudget::hold`] scope when dropped (also on unwind).
struct Held<'a>(&'a CallBudget);

impl Drop for Held<'_> {
    fn drop(&mut self) {
        self.0.held.fetch_sub(1, Ordering::Relaxed);
    }
}

impl CallBudget {
    pub(crate) fn new(fuel: u64) -> Arc<Self> {
        Arc::new(Self {
            fuel: AtomicU64::new(fuel),
            remaining: AtomicU64::new(fuel),
            copies_left: AtomicU64::new(copy_allowance(fuel)),
            wasm: AtomicU64::new(0),
            memory_growth_bytes: AtomicU64::new(0),
            depth: AtomicU32::new(0),
            held: AtomicU32::new(0),
        })
    }

    /// Runs `body` as one host call: every call into a sharing store that
    /// `body` makes, one after another or nested, spends what is left of
    /// one budget. The outermost hold (no hold and no Wasm of a sharing
    /// store below it) starts the budget at the configured fuel.
    pub(crate) fn hold<T>(&self, body: impl FnOnce() -> T) -> T {
        if self.held.load(Ordering::Relaxed) == 0 && self.depth.load(Ordering::Relaxed) == 0 {
            self.start();
        }
        self.held.fetch_add(1, Ordering::Relaxed);
        let _held = Held(self);
        body()
    }

    /// Runs `body` on a budget of its own, the configured fuel and copy
    /// allowance, as if it were an outermost host call, then gives the
    /// current call back what it had left: what `body` spends is never
    /// charged to the call it runs inside. The session host compiles each
    /// document its embedder serves this way, once per run.
    pub(crate) fn apart<T>(&self, body: impl FnOnce() -> T) -> T {
        /// Gives the call its remainder back when dropped (also on unwind).
        struct Restore<'a> {
            budget: &'a CallBudget,
            remaining: u64,
            copies_left: u64,
        }
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                self.budget
                    .remaining
                    .store(self.remaining, Ordering::Relaxed);
                self.budget
                    .copies_left
                    .store(self.copies_left, Ordering::Relaxed);
            }
        }
        let _restore = Restore {
            budget: self,
            remaining: self.remaining(),
            copies_left: self.copies_left.load(Ordering::Relaxed),
        };
        self.start();
        body()
    }

    /// Makes `store` run on this budget. Install it before the store first
    /// runs Wasm (instantiation included).
    pub(crate) fn share<T: 'static>(self: &Arc<Self>, store: &mut Store<T>) {
        let budget = Arc::clone(self);
        // The fuel granted to each of this store's live Wasm activations
        // (a host call can re-enter the same store through `realloc`).
        let mut grants: Vec<u64> = Vec::new();
        store.call_hook(move |mut store, hook| {
            match hook {
                CallHook::CallingWasm => {
                    if budget.depth.load(Ordering::Relaxed) == 0
                        && budget.held.load(Ordering::Relaxed) == 0
                    {
                        budget.start();
                    }
                    let grant = store.get_fuel()?.min(budget.remaining());
                    store.set_fuel(grant)?;
                    grants.push(grant);
                    budget.depth.fetch_add(1, Ordering::Relaxed);
                }
                CallHook::ReturningFromWasm => {
                    let left = store.get_fuel()?;
                    if let Some(grant) = grants.pop() {
                        budget.charge(grant.saturating_sub(left));
                        budget.depth.fetch_sub(1, Ordering::Relaxed);
                    }
                }
                CallHook::CallingHost => {
                    let left = store.get_fuel()?;
                    if let Some(grant) = grants.last_mut() {
                        budget.charge(grant.saturating_sub(left));
                        *grant = left;
                    }
                }
                CallHook::ReturningFromHost => {
                    let grant = store.get_fuel()?.min(budget.remaining());
                    store.set_fuel(grant)?;
                    if let Some(current) = grants.last_mut() {
                        *current = grant;
                    }
                }
            }
            Ok(())
        });
    }

    fn fuel(&self) -> u64 {
        self.fuel.load(Ordering::Relaxed)
    }

    /// Starts an outermost host call with the full configured fuel and
    /// copy allowance.
    pub(crate) fn start(&self) {
        let fuel = self.fuel();
        self.remaining.store(fuel, Ordering::Relaxed);
        self.copies_left
            .store(copy_allowance(fuel), Ordering::Relaxed);
    }

    /// What every call on this budget spent so far: the Wasm every sharing
    /// store executed and returned from or called out of, and the bytes its
    /// memory growth copies used. The difference of two readings is what
    /// the calls between them spent.
    pub(crate) fn spent(&self) -> CallFuel {
        CallFuel {
            wasm: self.wasm.load(Ordering::Relaxed),
            memory_growth_bytes: self.memory_growth_bytes.load(Ordering::Relaxed),
        }
    }

    /// Sets the fuel each later outermost host call starts with (a store
    /// whose own budget follows its per-call fuel).
    pub(crate) fn set_fuel(&self, fuel: u64) {
        self.fuel.store(fuel, Ordering::Relaxed);
    }

    fn remaining(&self) -> u64 {
        self.remaining.load(Ordering::Relaxed)
    }

    /// Takes `bytes` of a linear memory's growth copy
    /// (`crate::memory_growth`) from the current call's copy allowance when
    /// it still holds them, and reports whether it did; a copy taken leaves
    /// the Wasm fuel untouched. Otherwise the copy is refused and the call
    /// is over, as on fuel exhaustion: the allowance and the fuel are both
    /// spent, so the growing store traps and every store the call returns
    /// into traps too.
    pub(crate) fn try_copy(&self, bytes: u64) -> bool {
        let left = self.copies_left.load(Ordering::Relaxed);
        if bytes > left {
            self.copies_left.store(0, Ordering::Relaxed);
            self.remaining.store(0, Ordering::Relaxed);
            return false;
        }
        self.copies_left.store(left - bytes, Ordering::Relaxed);
        self.memory_growth_bytes.store(
            self.memory_growth_bytes
                .load(Ordering::Relaxed)
                .saturating_add(bytes),
            Ordering::Relaxed,
        );
        true
    }

    fn charge(&self, executed: u64) {
        let left = self.remaining().saturating_sub(executed);
        self.remaining.store(left, Ordering::Relaxed);
        self.wasm.store(
            self.wasm.load(Ordering::Relaxed).saturating_add(executed),
            Ordering::Relaxed,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Loop iterations of `spin` that fit in every budget below.
    const SPINS: i32 = 100;
    /// Loop iterations of `spin` past a 10,000-fuel budget.
    const SPINS_PAST_BUDGET: i32 = 1_000_000;
    /// The smallest copy.
    const ONE_BYTE: u64 = 1;

    /// A core module: `spin(n)` loops `n` times; `call_host` calls the
    /// imported `host.nested`.
    fn spin_module() -> Vec<u8> {
        use wasm_encoder::{
            BlockType, CodeSection, EntityType, ExportKind, ExportSection, Function,
            FunctionSection, ImportSection, Module, TypeSection, ValType,
        };
        let mut module = Module::new();
        let mut types = TypeSection::new();
        types.ty().function([], []);
        types.ty().function([ValType::I32], []);
        module.section(&types);
        let mut imports = ImportSection::new();
        imports.import("host", "nested", EntityType::Function(0));
        module.section(&imports);
        let mut functions = FunctionSection::new();
        functions.function(1);
        functions.function(0);
        module.section(&functions);
        let mut exports = ExportSection::new();
        exports.export("spin", ExportKind::Func, 1);
        exports.export("call_host", ExportKind::Func, 2);
        module.section(&exports);
        let mut code = CodeSection::new();
        let mut spin = Function::new([]);
        spin.instructions()
            .loop_(BlockType::Empty)
            .local_get(0)
            .i32_const(1)
            .i32_sub()
            .local_set(0)
            .local_get(0)
            .i32_const(0)
            .i32_gt_s()
            .br_if(0)
            .end()
            .end();
        code.function(&spin);
        let mut call_host = Function::new([]);
        call_host.instructions().call(0).end();
        code.function(&call_host);
        module.section(&code);
        module.finish()
    }

    /// A core module with one page of memory: `grow_spin(n)` grows the
    /// memory by a page, then loops `n` times.
    fn growing_module() -> Vec<u8> {
        use wasm_encoder::{
            BlockType, CodeSection, ExportKind, ExportSection, Function, FunctionSection,
            MemorySection, MemoryType, Module, TypeSection, ValType,
        };
        let mut module = Module::new();
        let mut types = TypeSection::new();
        types.ty().function([ValType::I32], []);
        module.section(&types);
        let mut functions = FunctionSection::new();
        functions.function(0);
        module.section(&functions);
        let mut memories = MemorySection::new();
        memories.memory(MemoryType {
            minimum: 1,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        module.section(&memories);
        let mut exports = ExportSection::new();
        exports.export("grow_spin", ExportKind::Func, 0);
        module.section(&exports);
        let mut code = CodeSection::new();
        let mut grow_spin = Function::new([]);
        grow_spin
            .instructions()
            .i32_const(1)
            .memory_grow(0)
            .drop()
            .loop_(BlockType::Empty)
            .local_get(0)
            .i32_const(1)
            .i32_sub()
            .local_set(0)
            .local_get(0)
            .i32_const(0)
            .i32_gt_s()
            .br_if(0)
            .end()
            .end();
        code.function(&grow_spin);
        module.section(&code);
        module.finish()
    }

    fn engine() -> wasmtime::Engine {
        let mut config = wasmtime::Config::new();
        config.consume_fuel(true);
        wasmtime::Engine::new(&config).expect("engine")
    }

    fn instance(
        engine: &wasmtime::Engine,
        budget: &Arc<CallBudget>,
    ) -> (Store<()>, wasmtime::Instance) {
        let module = wasmtime::Module::new(engine, spin_module()).expect("module");
        let mut store = Store::new(engine, ());
        budget.share(&mut store);
        store.set_fuel(u64::MAX).expect("fuel");
        let mut linker = wasmtime::Linker::new(engine);
        linker
            .func_wrap("host", "nested", || {})
            .expect("host function");
        let instance = linker.instantiate(&mut store, &module).expect("instance");
        (store, instance)
    }

    #[test]
    fn every_outermost_call_starts_the_configured_budget() {
        let engine = engine();
        let budget = CallBudget::new(10_000);
        let (mut store, instance) = instance(&engine, &budget);
        let spin = instance
            .get_typed_func::<i32, ()>(&mut store, "spin")
            .expect("spin");
        for _ in 0..3 {
            store.set_fuel(u64::MAX).expect("fuel");
            spin.call(&mut store, SPINS).expect("fits one budget");
            assert!(store.get_fuel().expect("fuel") < 10_000);
        }
        store.set_fuel(u64::MAX).expect("fuel");
        let trap = spin
            .call(&mut store, SPINS_PAST_BUDGET)
            .expect_err("over one budget");
        assert_eq!(
            trap.downcast_ref::<wasmtime::Trap>(),
            Some(&wasmtime::Trap::OutOfFuel)
        );
    }

    #[test]
    fn the_budget_counts_wasm_and_copied_bytes_apart() {
        const FUEL: u64 = 10_000;
        const COPY: u64 = 7;
        let engine = engine();
        let budget = CallBudget::new(FUEL);
        let (mut store, instance) = instance(&engine, &budget);
        let spin = instance
            .get_typed_func::<i32, ()>(&mut store, "spin")
            .expect("spin");
        let before = budget.spent();
        store.set_fuel(u64::MAX).expect("fuel");
        spin.call(&mut store, SPINS).expect("fits one budget");
        let executed = FUEL - store.get_fuel().expect("fuel");
        assert_eq!(
            budget.spent().since(before),
            CallFuel {
                wasm: executed,
                memory_growth_bytes: 0
            }
        );
        assert!(budget.try_copy(COPY), "the allowance holds the copy");
        store.set_fuel(u64::MAX).expect("fuel");
        spin.call(&mut store, SPINS).expect("fits one budget");
        assert_eq!(
            budget.spent().since(before),
            CallFuel {
                wasm: 2 * executed,
                memory_growth_bytes: COPY
            }
        );
    }

    #[test]
    fn a_call_copies_at_most_its_allowance_and_ends_past_it() {
        const FUEL: u64 = 1_000;
        let budget = CallBudget::new(FUEL);
        budget.start();
        assert!(budget.try_copy(FUEL * crate::MEMORY_GROWTH_COPY_BYTES_PER_FUEL));
        assert_eq!(budget.remaining(), FUEL, "a copy spends no Wasm fuel");
        assert!(!budget.try_copy(ONE_BYTE), "the allowance is spent");
        assert_eq!(budget.remaining(), 0, "a refused copy ends the call");
        budget.start();
        assert!(
            budget.try_copy(ONE_BYTE),
            "the next call starts a fresh allowance"
        );
    }

    /// Wasmtime runs a memory growth as a host transition but does not let
    /// the host lower the store's fuel across it, so a growth must not make
    /// the budget lose track of the Wasm the growing activation runs
    /// afterwards; the copy comes out of the copy allowance, not the fuel.
    #[test]
    fn wasm_run_after_a_growth_is_charged() {
        const FUEL: u64 = 100_000_000;
        const SPINS: i32 = 1_000_000;
        let engine = engine();
        let budget = CallBudget::new(FUEL);
        let module = wasmtime::Module::new(&engine, growing_module()).expect("module");
        let limits = crate::memory_growth::MeteredLimits::new(
            wasmtime::StoreLimitsBuilder::new().build(),
            Arc::clone(&budget),
        );
        let mut store = Store::new(&engine, limits);
        store.limiter(|limits| limits);
        budget.share(&mut store);
        let instance = wasmtime::Instance::new(&mut store, &module, &[]).expect("instance");
        let grow_spin = instance
            .get_typed_func::<i32, ()>(&mut store, "grow_spin")
            .expect("grow_spin");
        store.set_fuel(FUEL).expect("fuel");
        grow_spin.call(&mut store, SPINS).expect("fits one budget");
        let executed = FUEL - store.get_fuel().expect("fuel");
        assert_eq!(
            budget.spent(),
            CallFuel {
                wasm: executed,
                memory_growth_bytes: 65_536
            }
        );
        assert_eq!(budget.remaining(), FUEL - executed, "copies spend no fuel");
    }

    #[test]
    fn calls_made_while_held_share_one_budget() {
        let engine = engine();
        let budget = CallBudget::new(1_000_000);
        let (mut store, instance) = instance(&engine, &budget);
        let spin = instance
            .get_typed_func::<i32, ()>(&mut store, "spin")
            .expect("spin");
        // Three outermost calls one after another: each fits one budget
        // alone, three held as one host call do not.
        let trap = budget
            .hold(|| {
                for _ in 0..3 {
                    store.set_fuel(u64::MAX)?;
                    spin.call(&mut store, 100_000)?;
                }
                wasmtime::Result::<()>::Ok(())
            })
            .expect_err("three held calls exceed one host call's budget");
        assert_eq!(
            trap.downcast_ref::<wasmtime::Trap>(),
            Some(&wasmtime::Trap::OutOfFuel)
        );
        // A held body starts a fresh budget, and so does the next call
        // made outside any hold.
        budget
            .hold(|| {
                store.set_fuel(u64::MAX)?;
                spin.call(&mut store, 100_000)
            })
            .expect("a fresh held budget");
        store.set_fuel(u64::MAX).expect("fuel");
        spin.call(&mut store, 100_000).expect("a fresh budget");
    }

    #[test]
    fn a_call_run_apart_is_charged_to_its_own_budget() {
        let engine = engine();
        let budget = CallBudget::new(1_000_000);
        let (mut store, instance) = instance(&engine, &budget);
        let spin = instance
            .get_typed_func::<i32, ()>(&mut store, "spin")
            .expect("spin");
        // Inside one held call, three calls run apart each spend a budget
        // of their own (together they exceed one), and the held call keeps
        // what it had left: two more calls that fit it alone still run.
        budget
            .hold(|| {
                store.set_fuel(u64::MAX)?;
                spin.call(&mut store, 50_000)?;
                let left = budget.remaining();
                for _ in 0..3 {
                    budget.apart(|| {
                        store.set_fuel(u64::MAX)?;
                        spin.call(&mut store, 100_000)
                    })?;
                }
                assert_eq!(budget.remaining(), left, "apart runs never charge the call");
                store.set_fuel(u64::MAX)?;
                spin.call(&mut store, 50_000)
            })
            .expect("the apart runs leave the held budget untouched");
        // A call run apart still runs out of its own budget.
        let trap = budget
            .apart(|| {
                store.set_fuel(u64::MAX)?;
                spin.call(&mut store, SPINS_PAST_BUDGET)
            })
            .expect_err("over its own budget");
        assert_eq!(
            trap.downcast_ref::<wasmtime::Trap>(),
            Some(&wasmtime::Trap::OutOfFuel)
        );
    }

    #[test]
    fn nested_stores_are_charged_to_the_outer_call() {
        let engine = engine();
        let budget = CallBudget::new(1_000_000);
        let (mut outer, _) = instance(&engine, &budget);
        let (inner, inner_instance) = instance(&engine, &budget);
        // The outer store's host function runs the inner store's `spin`
        // three times: each run fits the budget alone, three do not.
        let inner = std::sync::Mutex::new((inner, inner_instance));
        let module = wasmtime::Module::new(&engine, spin_module()).expect("module");
        let mut linker = wasmtime::Linker::new(&engine);
        let inner = Arc::new(inner);
        let nested = Arc::clone(&inner);
        linker
            .func_wrap("host", "nested", move || -> wasmtime::Result<()> {
                let mut guard = nested.lock().expect("inner");
                let (store, instance) = &mut *guard;
                let spin = instance.get_typed_func::<i32, ()>(&mut *store, "spin")?;
                for _ in 0..3 {
                    store.set_fuel(u64::MAX)?;
                    spin.call(&mut *store, 100_000)?;
                }
                Ok(())
            })
            .expect("host function");
        let outer_instance = linker.instantiate(&mut outer, &module).expect("instance");
        let call_host = outer_instance
            .get_typed_func::<(), ()>(&mut outer, "call_host")
            .expect("call_host");
        outer.set_fuel(u64::MAX).expect("fuel");
        let trap = call_host
            .call(&mut outer, ())
            .expect_err("three nested runs exceed one call's budget");
        assert_eq!(
            trap.downcast_ref::<wasmtime::Trap>(),
            Some(&wasmtime::Trap::OutOfFuel)
        );
        // The next outermost call starts a fresh budget.
        let (store, instance) = &mut *inner.lock().expect("inner");
        let spin = instance
            .get_typed_func::<i32, ()>(&mut *store, "spin")
            .expect("spin");
        store.set_fuel(u64::MAX).expect("fuel");
        spin.call(&mut *store, 100_000).expect("a fresh budget");
    }
}
