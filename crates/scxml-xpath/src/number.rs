//! XPath 1.0 number/string conversions (§4.2 `string()`, §4.4 `number()`).

use alloc::format;
use alloc::string::String;

use crate::is_xml_space;

/// 2^52: every finite `f64` at least this large in magnitude is integral.
const INTEGRAL_MAGNITUDE: f64 = 4_503_599_627_370_496.0;

/// `value` truncated toward zero; exact for magnitudes below
/// [`INTEGRAL_MAGNITUDE`].
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "callers pass magnitudes below 2^52, which i64 and f64 hold exactly"
)]
fn truncate_small(value: f64) -> f64 {
    value as i64 as f64
}

/// IEEE 754 `floor` (XPath 1.0 `floor()`), computed without a math library:
/// NaN, infinities, zeros (keeping their sign), and integral values are
/// unchanged.
pub(crate) fn floor(value: f64) -> f64 {
    if !value.is_finite() || value == 0.0 || value.abs() >= INTEGRAL_MAGNITUDE {
        return value;
    }
    let truncated = truncate_small(value);
    if truncated > value {
        truncated - 1.0
    } else {
        truncated
    }
}

/// IEEE 754 `ceil` (XPath 1.0 `ceiling()`), computed without a math
/// library: NaN, infinities, zeros, and integral values are unchanged, and
/// values in `(-1, 0)` round to `-0`.
pub(crate) fn ceiling(value: f64) -> f64 {
    if !value.is_finite() || value == 0.0 || value.abs() >= INTEGRAL_MAGNITUDE {
        return value;
    }
    let truncated = truncate_small(value);
    let rounded = if truncated < value {
        truncated + 1.0
    } else {
        truncated
    };
    if rounded == 0.0 && value < 0.0 {
        -0.0
    } else {
        rounded
    }
}

/// Converts a number to its XPath 1.0 string form.
///
/// `NaN`, `Infinity`, `-Infinity`; both zeros are `0`; integers carry no
/// decimal point; other values are the shortest decimal that round-trips,
/// never in exponent notation, with at least one digit before the point.
#[must_use]
pub fn number_to_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    if value == 0.0 {
        return "0".into();
    }
    // Rust's `Display` for f64 prints the shortest round-trip decimal and
    // never uses exponent notation; integral values print without a point.
    format!("{value}")
}

/// Converts a string to a number per XPath 1.0 `number()`.
///
/// Accepts optional XML whitespace, an optional `-`, and `Digits ('.'
/// Digits?)?` or `'.' Digits`; everything else (including `+`, exponents,
/// `Infinity`, and the empty string) is `NaN`.
#[must_use]
pub fn string_to_number(text: &str) -> f64 {
    let trimmed = text.trim_matches(is_xml_space);
    let unsigned = trimmed.strip_prefix('-').unwrap_or(trimmed);
    let (integer, fraction) = match unsigned.split_once('.') {
        Some((integer, fraction)) => (integer, Some(fraction)),
        None => (unsigned, None),
    };
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    let valid = digits(integer)
        && fraction.is_none_or(digits)
        && (!integer.is_empty() || fraction.is_some_and(|fraction| !fraction.is_empty()));
    if !valid {
        return f64::NAN;
    }
    trimmed.parse::<f64>().unwrap_or(f64::NAN)
}

/// XPath 1.0 `round()`: nearest integer, ties toward positive infinity;
/// NaN and infinities are unchanged; results in `[-0.5, 0)` are `-0`.
pub(crate) fn round(value: f64) -> f64 {
    if !value.is_finite() {
        return value;
    }
    let floor = floor(value);
    let rounded = if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    };
    if rounded == 0.0 && value.is_sign_negative() {
        -0.0
    } else {
        rounded
    }
}
