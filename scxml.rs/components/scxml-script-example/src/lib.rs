//! Example Wasm script component for `<script src="...wasm"/>` (SCXML 1.0
//! section 5.8).
//!
//! The component implements the `stateforward:scxml/script` world: it
//! imports only `types`, `telemetry`, and `data-model-host`, and exports
//! `script-api.run`. Each run reads the data-model location
//! [`INPUT_LOCATION`] through `data-model-host.evaluate`, doubles it, and
//! assigns the result to [`OUTPUT_LOCATION`] through `data-model-host.assign`.
//! The host links `data-model-host` to the session's active data model, so
//! the assignment lands in that data model's state. A non-numeric input is
//! a script failure (`execution`), which the machine raises as SCXML
//! `error.execution`.
//!
//! The component keeps no state between runs; hosts instantiate it fresh
//! for every `<script>` execution.
//!
//! ## Unsafe-code containment
//!
//! As in the other guests, authored code carries `#![forbid(unsafe_code)]`
//! on native targets; on `wasm32` the crate-wide allow exists solely for the
//! generated `wit_bindgen::generate!` canonical-ABI shims and is enforced by
//! `scripts/verify-component-authored-unsafe.py`.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; scripts/verify-component-authored-unsafe.py rejects authored unsafe"
    )
)]

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    wit_bindgen::generate!({
        world: "script",
        path: "../../crates/scxml-wit/wit/scxml",
    });

    use exports::stateforward::scxml::script_api::Guest;
    use stateforward::scxml::data_model_host;
    use stateforward::scxml::telemetry::{self, Operation, Outcome, SignalEvent};
    use stateforward::scxml::types::{ComponentError, DataValue, ErrorCode, ErrorVersion, Event};

    struct Component;

    impl Guest for Component {
        fn run(_event: Option<Event>) -> Result<(), ComponentError> {
            let result = (|| {
                let DataValue::Number(input) = data_model_host::evaluate(super::INPUT_LOCATION)?
                else {
                    return Err(error(
                        ErrorCode::Execution,
                        "script input location does not hold a number",
                    ));
                };
                let doubled = super::double(input)
                    .map_err(|cause| error(ErrorCode::Execution, cause.detail()))?;
                data_model_host::assign(super::OUTPUT_LOCATION, &DataValue::Number(doubled))
            })();
            let outcome = match &result {
                Ok(()) => Outcome::Ok,
                Err(error) if error.code == ErrorCode::InvalidInput => Outcome::Rejected,
                Err(_) => Outcome::Failed,
            };
            telemetry::observe(Operation::Execute, outcome, None, None);
            telemetry::event(
                Operation::Execute,
                if result.is_ok() {
                    SignalEvent::TransformationCompleted
                } else {
                    SignalEvent::InputRejected
                },
                None,
            );
            result
        }
    }

    fn error(code: ErrorCode, detail: &'static str) -> ComponentError {
        ComponentError {
            version: ErrorVersion::V1,
            code,
            detail: (detail.len() <= 160).then(|| detail.into()),
        }
    }

    export!(Component);
}

/// Data-model location the example reads.
pub const INPUT_LOCATION: &str = "input";
/// Data-model location the example assigns.
pub const OUTPUT_LOCATION: &str = "doubled";

/// Closed example failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScriptExampleError {
    /// The input or its double is not a finite number.
    NotFinite,
}

impl ScriptExampleError {
    /// Static, payload-free description.
    #[must_use]
    pub const fn detail(self) -> &'static str {
        match self {
            Self::NotFinite => "script input doubled is not a finite number",
        }
    }
}

/// Doubles one finite number.
///
/// # Errors
///
/// [`ScriptExampleError::NotFinite`] when `input` or its double is not
/// finite (data-model numbers must stay finite).
pub fn double(input: f64) -> Result<f64, ScriptExampleError> {
    let doubled = input * 2.0;
    if input.is_finite() && doubled.is_finite() {
        Ok(doubled)
    } else {
        Err(ScriptExampleError::NotFinite)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_finite_numbers_and_rejects_overflow() {
        assert_eq!(double(21.0), Ok(42.0));
        assert_eq!(double(-0.5), Ok(-1.0));
        assert_eq!(double(f64::MAX), Err(ScriptExampleError::NotFinite));
        assert_eq!(double(f64::NAN), Err(ScriptExampleError::NotFinite));
    }
}
