//! Build-time deterministic fuel metering for core WebAssembly modules.
//!
//! Browsers expose no Wasm fuel, epoch, or interrupt, so a component that
//! loops forever would hang the page. This crate rewrites a core module so
//! that it meters itself: it adds one mutable `i64` global, exported as
//! [`FUEL_EXPORT`], and charges one unit of it at every function entry and at
//! the head of every `loop` body. A charge point that finds the counter at
//! zero executes `unreachable`, so the module traps instead of running on.
//!
//! # Contract
//!
//! * Precondition: `module` is at most [`MAX_MODULE_BYTES`] bytes and is a
//!   valid core module under the default [`wasmparser::WasmFeatures`] that
//!   does not already export [`FUEL_EXPORT`].
//! * Postcondition: the output is a valid core module with the same imports,
//!   exports (plus [`FUEL_EXPORT`]), function, global, table, and memory
//!   indices, and the same behavior as the input whenever the counter stays
//!   above zero. The counter starts at `initial_fuel`; hosts overwrite it
//!   before each call they bound.
//! * Bound: every infinite execution passes a function entry or a loop head
//!   infinitely often, and code between two charge points is straight-line
//!   code bounded by the module size, so a counter of `n` bounds a call to at
//!   most `n` charges. Fuel is a count of charges, not of instructions.
//! * Determinism: the output bytes depend only on the input bytes and
//!   `initial_fuel`, and fuel consumption depends only on the executed path.
//! * Failure: a typed [`MeterError`]; no partial output.

#![forbid(unsafe_code)]

use std::convert::Infallible;
use std::fmt;

use scxml_telemetry::{ErrorCategory, Module as TelemetryModule, Operation, SignalEvent, observe};
use wasm_encoder::reencode::{Error as ReencodeError, Reencode, utils};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, ExportKind, ExportSection, Function, GlobalSection,
    GlobalType, Instruction, SectionId, ValType,
};
use wasmparser::{Parser, Payload, TypeRef, Validator, WasmFeatures};

/// Export name of the injected fuel global.
pub const FUEL_EXPORT: &str = "scxml:fuel";
/// Largest core module accepted.
pub const MAX_MODULE_BYTES: usize = 64 * 1024 * 1024;

/// Typed metering failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeterError {
    /// The module exceeds [`MAX_MODULE_BYTES`].
    TooLarge,
    /// The input is not a valid core module.
    InvalidModule,
    /// The input already exports [`FUEL_EXPORT`].
    FuelExportTaken,
    /// `initial_fuel` does not fit a signed 64-bit global.
    FuelOutOfRange,
    /// Re-encoding failed or produced an invalid module.
    Encoding,
}

impl MeterError {
    /// Closed telemetry category of this failure: `Encoding` (accepted input
    /// that could not be re-encoded) is `Packaging`; every other variant is
    /// rejected caller input.
    #[must_use]
    pub const fn category(self) -> ErrorCategory {
        match self {
            Self::Encoding => ErrorCategory::Packaging,
            Self::TooLarge | Self::InvalidModule | Self::FuelExportTaken | Self::FuelOutOfRange => {
                ErrorCategory::InvalidInput
            }
        }
    }

    /// Static, input-free description of this failure.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::TooLarge => "core module exceeds the metering size bound",
            Self::InvalidModule => "input is not a valid core WebAssembly module",
            Self::FuelExportTaken => "core module already exports the fuel global",
            Self::FuelOutOfRange => "initial fuel does not fit a signed 64-bit global",
            Self::Encoding => "metered module could not be encoded or validated",
        }
    }
}

impl fmt::Display for MeterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for MeterError {}

/// Returns `module` rewritten to meter itself; see the crate contract.
///
/// # Errors
///
/// [`MeterError`] for an over-bound, invalid, or already-metered module, an
/// out-of-range `initial_fuel`, or an encoding failure.
///
/// # Telemetry
///
/// This is the crate's one boundary: each call opens exactly one
/// `scxml.wasm-meter.instrument` span. Success records the
/// `scxml.transformation.completed` event. A rejected input records
/// `scxml.input.rejected` and fails with `invalid-input`; an encoding
/// failure of accepted input records no input event and fails with
/// `packaging`. Helpers, the re-encoder hooks, the per-function and per-loop
/// charge, and [`MeterError`]'s `Display` emit nothing of their own.
pub fn meter(module: &[u8], initial_fuel: u64) -> Result<Vec<u8>, MeterError> {
    let span = observe(TelemetryModule::WasmMeter, Operation::WasmMeterInstrument);
    let result = meter_inner(module, initial_fuel);
    match result {
        Ok(_) => span.event(SignalEvent::TransformationCompleted),
        Err(error) => {
            let category = error.category();
            if category == ErrorCategory::InvalidInput {
                span.event(SignalEvent::InputRejected);
            }
            span.fail(category);
        }
    }
    result
}

fn meter_inner(module: &[u8], initial_fuel: u64) -> Result<Vec<u8>, MeterError> {
    if module.len() > MAX_MODULE_BYTES {
        return Err(MeterError::TooLarge);
    }
    let initial = i64::try_from(initial_fuel).map_err(|_| MeterError::FuelOutOfRange)?;
    Validator::new_with_features(WasmFeatures::default())
        .validate_all(module)
        .map_err(|_| MeterError::InvalidModule)?;
    let fuel_global = fuel_global_index(module)?;
    let mut meter = Meter {
        fuel_global,
        initial,
        global_emitted: false,
        export_emitted: false,
    };
    let mut output = wasm_encoder::Module::new();
    meter
        .parse_core_module(&mut output, Parser::new(0), module)
        .map_err(|_| MeterError::Encoding)?;
    let bytes = output.finish();
    Validator::new_with_features(WasmFeatures::default())
        .validate_all(&bytes)
        .map_err(|_| MeterError::Encoding)?;
    Ok(bytes)
}

/// Index the injected global gets: after every imported and defined global.
fn fuel_global_index(module: &[u8]) -> Result<u32, MeterError> {
    let mut globals = 0_u32;
    for payload in Parser::new(0).parse_all(module) {
        match payload.map_err(|_| MeterError::InvalidModule)? {
            Payload::ImportSection(section) => {
                for import in section.into_imports() {
                    let import = import.map_err(|_| MeterError::InvalidModule)?;
                    if matches!(import.ty, TypeRef::Global(_)) {
                        globals = globals.checked_add(1).ok_or(MeterError::InvalidModule)?;
                    }
                }
            }
            Payload::GlobalSection(section) => {
                globals = globals
                    .checked_add(section.count())
                    .ok_or(MeterError::InvalidModule)?;
            }
            Payload::ExportSection(section) => {
                for export in section {
                    let export = export.map_err(|_| MeterError::InvalidModule)?;
                    if export.name == FUEL_EXPORT {
                        return Err(MeterError::FuelExportTaken);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(globals)
}

/// Canonical binary section order (the tag section sits between memory and
/// global; data count precedes code).
const fn section_rank(id: SectionId) -> u8 {
    match id {
        SectionId::Custom => 0,
        SectionId::Type => 1,
        SectionId::Import => 2,
        SectionId::Function => 3,
        SectionId::Table => 4,
        SectionId::Memory => 5,
        SectionId::Tag => 6,
        SectionId::Global => 7,
        SectionId::Export => 8,
        SectionId::Start => 9,
        SectionId::Element => 10,
        SectionId::DataCount => 11,
        SectionId::Code => 12,
        SectionId::Data => 13,
    }
}

struct Meter {
    fuel_global: u32,
    initial: i64,
    global_emitted: bool,
    export_emitted: bool,
}

impl Meter {
    fn push_global(&self, globals: &mut GlobalSection) {
        globals.global(
            GlobalType {
                val_type: ValType::I64,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i64_const(self.initial),
        );
    }

    fn push_export(&self, exports: &mut ExportSection) {
        exports.export(FUEL_EXPORT, ExportKind::Global, self.fuel_global);
    }

    /// Traps when the counter is zero, otherwise spends one unit.
    fn charge(&self, function: &mut Function) {
        function
            .instruction(&Instruction::GlobalGet(self.fuel_global))
            .instruction(&Instruction::I64Eqz)
            .instruction(&Instruction::If(BlockType::Empty))
            .instruction(&Instruction::Unreachable)
            .instruction(&Instruction::End)
            .instruction(&Instruction::GlobalGet(self.fuel_global))
            .instruction(&Instruction::I64Const(1))
            .instruction(&Instruction::I64Sub)
            .instruction(&Instruction::GlobalSet(self.fuel_global));
    }
}

impl Reencode for Meter {
    type Error = Infallible;

    fn intersperse_section_hook(
        &mut self,
        module: &mut wasm_encoder::Module,
        _after: Option<SectionId>,
        before: Option<SectionId>,
    ) -> Result<(), ReencodeError<Infallible>> {
        let past = |section: SectionId| {
            before.is_none_or(|next| section_rank(next) > section_rank(section))
        };
        if !self.global_emitted && past(SectionId::Global) {
            let mut globals = GlobalSection::new();
            self.push_global(&mut globals);
            module.section(&globals);
            self.global_emitted = true;
        }
        if !self.export_emitted && past(SectionId::Export) {
            let mut exports = ExportSection::new();
            self.push_export(&mut exports);
            module.section(&exports);
            self.export_emitted = true;
        }
        Ok(())
    }

    fn parse_global_section(
        &mut self,
        globals: &mut GlobalSection,
        section: wasmparser::GlobalSectionReader<'_>,
    ) -> Result<(), ReencodeError<Infallible>> {
        utils::parse_global_section(self, globals, section)?;
        self.push_global(globals);
        self.global_emitted = true;
        Ok(())
    }

    fn parse_export_section(
        &mut self,
        exports: &mut ExportSection,
        section: wasmparser::ExportSectionReader<'_>,
    ) -> Result<(), ReencodeError<Infallible>> {
        utils::parse_export_section(self, exports, section)?;
        self.push_export(exports);
        self.export_emitted = true;
        Ok(())
    }

    fn parse_function_body(
        &mut self,
        code: &mut CodeSection,
        func: wasmparser::FunctionBody<'_>,
    ) -> Result<(), ReencodeError<Infallible>> {
        let mut function = self.new_function_with_parsed_locals(&func)?;
        self.charge(&mut function);
        let mut reader = func.get_operators_reader()?;
        while !reader.eof() {
            let instruction = self.parse_instruction(&mut reader)?;
            let loop_head = matches!(instruction, Instruction::Loop(_));
            function.instruction(&instruction);
            if loop_head {
                self.charge(&mut function);
            }
        }
        code.function(&function);
        Ok(())
    }
}
