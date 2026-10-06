//! Executes metered modules under Wasmtime (with its own fuel disabled) to
//! prove the injected counter bounds loops and recursion, preserves results,
//! and is deterministic.

use scxml_telemetry::{
    ErrorCategory, Module as TelemetryModule, Operation, Recorder, SignalEvent, install_recorder,
};
use scxml_wasm_meter::{FUEL_EXPORT, MAX_MODULE_BYTES, MeterError, meter};
use wasm_encoder::{
    BlockType, CodeSection, ExportKind, ExportSection, Function, FunctionSection, Instruction,
    Module, TypeSection, ValType,
};
use wasmtime::{Engine, Instance, Store, Val};

/// Test module:
/// * `spin()`: loops forever;
/// * `recurse()`: calls itself forever;
/// * `count(n) -> n`: loops `n` times and returns the iteration count.
fn fixture(extra_export: Option<&str>) -> Vec<u8> {
    let mut types = TypeSection::new();
    types.ty().function([], []);
    types.ty().function([ValType::I32], [ValType::I32]);
    let mut functions = FunctionSection::new();
    functions.function(0);
    functions.function(0);
    functions.function(1);
    let mut exports = ExportSection::new();
    exports.export("spin", ExportKind::Func, 0);
    exports.export("recurse", ExportKind::Func, 1);
    exports.export("count", ExportKind::Func, 2);
    if let Some(name) = extra_export {
        exports.export(name, ExportKind::Func, 0);
    }
    let mut code = CodeSection::new();
    let mut spin = Function::new([]);
    spin.instruction(&Instruction::Loop(BlockType::Empty))
        .instruction(&Instruction::Br(0))
        .instruction(&Instruction::End)
        .instruction(&Instruction::End);
    code.function(&spin);
    let mut recurse = Function::new([]);
    recurse
        .instruction(&Instruction::Call(1))
        .instruction(&Instruction::End);
    code.function(&recurse);
    // local 1: iterations
    let mut count = Function::new([(1, ValType::I32)]);
    count
        .instruction(&Instruction::Block(BlockType::Empty))
        .instruction(&Instruction::Loop(BlockType::Empty))
        .instruction(&Instruction::LocalGet(1))
        .instruction(&Instruction::LocalGet(0))
        .instruction(&Instruction::I32GeU)
        .instruction(&Instruction::BrIf(1))
        .instruction(&Instruction::LocalGet(1))
        .instruction(&Instruction::I32Const(1))
        .instruction(&Instruction::I32Add)
        .instruction(&Instruction::LocalSet(1))
        .instruction(&Instruction::Br(0))
        .instruction(&Instruction::End)
        .instruction(&Instruction::End)
        .instruction(&Instruction::LocalGet(1))
        .instruction(&Instruction::End);
    code.function(&count);
    let mut module = Module::new();
    module
        .section(&types)
        .section(&functions)
        .section(&exports)
        .section(&code);
    module.finish()
}

struct Metered {
    store: Store<()>,
    instance: Instance,
}

impl Metered {
    fn new(bytes: &[u8]) -> Self {
        let engine = Engine::default();
        let module = wasmtime::Module::new(&engine, bytes).expect("metered module compiles");
        let mut store = Store::new(&engine, ());
        let instance =
            Instance::new(&mut store, &module, &[]).expect("metered module instantiates");
        Self { store, instance }
    }

    fn fuel(&mut self) -> i64 {
        let global = self
            .instance
            .get_global(&mut self.store, FUEL_EXPORT)
            .expect("the fuel global is exported");
        global
            .get(&mut self.store)
            .i64()
            .expect("the fuel global is i64")
    }

    fn set_fuel(&mut self, fuel: i64) {
        let global = self
            .instance
            .get_global(&mut self.store, FUEL_EXPORT)
            .expect("the fuel global is exported");
        global
            .set(&mut self.store, Val::I64(fuel))
            .expect("the fuel global is mutable");
    }

    fn call(&mut self, name: &str, args: &[Val]) -> Result<Vec<Val>, wasmtime::Error> {
        let function = self
            .instance
            .get_func(&mut self.store, name)
            .expect("fixture export exists");
        let mut results = vec![Val::I32(0); function.ty(&self.store).results().len()];
        function.call(&mut self.store, args, &mut results)?;
        Ok(results)
    }
}

#[test]
fn infinite_loops_and_recursion_trap_when_fuel_runs_out() {
    let metered = meter(&fixture(None), 0).expect("fixture meters");
    let mut instance = Metered::new(&metered);
    assert_eq!(instance.fuel(), 0, "the counter starts at the initial fuel");

    instance.set_fuel(10_000);
    assert!(instance.call("spin", &[]).is_err(), "spin must trap");
    assert_eq!(instance.fuel(), 0, "the trap is fuel exhaustion");

    instance.set_fuel(500);
    assert!(instance.call("recurse", &[]).is_err(), "recurse must trap");
    assert_eq!(instance.fuel(), 0, "fuel runs out before the stack does");
}

#[test]
fn metered_results_are_unchanged_and_consumption_is_exact() {
    let metered = meter(&fixture(None), 0).expect("fixture meters");
    let mut instance = Metered::new(&metered);
    for n in [0, 1, 7, 1000] {
        instance.set_fuel(1_000_000);
        let results = instance
            .call("count", &[Val::I32(n)])
            .expect("bounded loop completes");
        assert_eq!(results[0].i32(), Some(n));
        // One charge at entry, one per loop-head entry (n iterations plus
        // the exit check).
        assert_eq!(instance.fuel(), 1_000_000 - (1 + i64::from(n) + 1));
    }
    // Exactly enough fuel succeeds; one unit less traps.
    instance.set_fuel(1 + 7 + 1);
    assert!(instance.call("count", &[Val::I32(7)]).is_ok());
    instance.set_fuel(1 + 7);
    assert!(instance.call("count", &[Val::I32(7)]).is_err());
}

#[test]
fn metering_is_deterministic_and_records_the_initial_fuel() {
    let first = meter(&fixture(None), 42).expect("fixture meters");
    assert_eq!(first, meter(&fixture(None), 42).expect("fixture meters"));
    assert_ne!(first, meter(&fixture(None), 43).expect("fixture meters"));
    let mut instance = Metered::new(&first);
    assert_eq!(instance.fuel(), 42);
}

#[test]
fn modules_without_global_or_export_sections_are_metered() {
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    let mut functions = FunctionSection::new();
    functions.function(0);
    let mut code = CodeSection::new();
    let mut body = Function::new([]);
    body.instruction(&Instruction::End);
    code.function(&body);
    module.section(&types).section(&functions).section(&code);
    let metered = meter(&module.finish(), 5).expect("module without exports meters");
    let mut instance = Metered::new(&metered);
    assert_eq!(instance.fuel(), 5);
    let empty = meter(&Module::new().finish(), 5).expect("empty module meters");
    let mut instance = Metered::new(&empty);
    assert_eq!(instance.fuel(), 5);
}

#[test]
fn hostile_inputs_fail_closed() {
    assert_eq!(meter(b"\0asm", 0), Err(MeterError::InvalidModule));
    assert_eq!(meter(b"not wasm", 0), Err(MeterError::InvalidModule));
    assert_eq!(
        meter(&fixture(Some(FUEL_EXPORT)), 0),
        Err(MeterError::FuelExportTaken)
    );
    let metered = meter(&fixture(None), 0).expect("fixture meters");
    assert_eq!(
        meter(&metered, 0),
        Err(MeterError::FuelExportTaken),
        "metering twice is rejected"
    );
    assert_eq!(
        meter(&fixture(None), u64::MAX),
        Err(MeterError::FuelOutOfRange)
    );
    assert_eq!(
        meter(&vec![0; MAX_MODULE_BYTES + 1], 0),
        Err(MeterError::TooLarge)
    );
}

#[test]
fn meter_is_one_boundary_span_and_display_emits_nothing() {
    let recorder = Recorder::default();
    {
        let _guard = install_recorder(recorder.clone());
        meter(&fixture(None), 0).expect("fixture meters");
    }
    let observations = recorder.observations();
    assert_eq!(
        observations.len(),
        1,
        "a successful meter() call covers every function and loop head with one span"
    );
    assert_eq!(observations[0].module, TelemetryModule::WasmMeter);
    assert_eq!(observations[0].operation, Operation::WasmMeterInstrument);
    assert_eq!(observations[0].error, None);
    assert_eq!(
        observations[0].events,
        [SignalEvent::TransformationCompleted]
    );

    let recorder = Recorder::default();
    {
        let _guard = install_recorder(recorder.clone());
        let error = meter(b"not wasm", 0).expect_err("invalid input is rejected");
        assert_eq!(
            error.to_string(),
            "input is not a valid core WebAssembly module"
        );
    }
    let observations = recorder.observations();
    assert_eq!(
        observations.len(),
        1,
        "a failed meter() call is one span, and formatting its error opens none"
    );
    assert_eq!(observations[0].operation, Operation::WasmMeterInstrument);
    assert_eq!(observations[0].error, Some(ErrorCategory::InvalidInput));
    assert_eq!(
        observations[0].events,
        [SignalEvent::InputRejected],
        "one failure records exactly one input-rejected event"
    );
}

#[test]
fn error_categories_split_rejected_input_from_encoding() {
    for error in [
        MeterError::TooLarge,
        MeterError::InvalidModule,
        MeterError::FuelExportTaken,
        MeterError::FuelOutOfRange,
    ] {
        assert_eq!(error.category(), ErrorCategory::InvalidInput, "{error:?}");
    }
    assert_eq!(MeterError::Encoding.category(), ErrorCategory::Packaging);
}
