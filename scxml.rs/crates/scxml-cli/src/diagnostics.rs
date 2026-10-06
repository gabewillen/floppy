//! Validator output formats and `--explain` for the CLI.
//!
//! `human` renders the canonical artifact with the rustc-style reporter on
//! stderr. `json` writes the exact canonical `scxml.report` bytes to stdout —
//! the stable machine interface for AI repair loops. `sarif` writes SARIF
//! 2.1.0 (with `fixes`) to stdout. Every format is a projection of the same
//! canonical artifact; none reruns validation.

use std::collections::BTreeSet;
use std::io::{self, IsTerminal as _, Write as _};

use scxml_reporting::explain;
use scxml_reporting::{
    ArtifactEnvelope, ColorChoice, RustcRenderer, encode_canonical, render_sarif,
};
use scxml_telemetry::{ErrorCategory, Module, Operation, observe};

/// Validator output format.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OutputFormat {
    /// Rustc-style text on stderr.
    #[default]
    Human,
    /// Canonical `scxml.report` JSON bytes on stdout.
    Json,
    /// SARIF 2.1.0 JSON on stdout.
    Sarif,
}

/// Parses `--format` values.
pub fn parse_format(value: Option<&String>) -> Result<OutputFormat, String> {
    match value.map(String::as_str) {
        Some("human") => Ok(OutputFormat::Human),
        Some("json") => Ok(OutputFormat::Json),
        Some("sarif") => Ok(OutputFormat::Sarif),
        _ => Err("--format requires human, json, or sarif".to_owned()),
    }
}

/// Writes one validated canonical artifact in the selected format.
pub fn write_artifact(
    artifact: &ArtifactEnvelope,
    format: OutputFormat,
) -> Result<(), Box<dyn std::error::Error>> {
    let span = observe(Module::Cli, Operation::CliEmit);
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        match format {
            OutputFormat::Human => {
                let rendered = RustcRenderer {
                    color: if io::stderr().is_terminal() {
                        ColorChoice::Always
                    } else {
                        ColorChoice::Never
                    },
                }
                .render(artifact)?;
                let mut stderr = io::stderr().lock();
                stderr.write_all(rendered.as_bytes())?;
                if let Some(footer) = explain_footer(artifact) {
                    stderr.write_all(footer.as_bytes())?;
                }
            }
            OutputFormat::Json => io::stdout()
                .lock()
                .write_all(&encode_canonical(artifact)?)?,
            OutputFormat::Sarif => io::stdout().lock().write_all(&render_sarif(artifact)?)?,
        }
        Ok(())
    })();
    if result.is_err() {
        span.fail(ErrorCategory::Report);
    }
    result
}

/// Rustc-style pointer to `--explain` for the documented codes in a report.
#[must_use]
pub fn explain_footer(artifact: &ArtifactEnvelope) -> Option<String> {
    let diagnostics = match (artifact.validator_payload(), artifact.compiler_payload()) {
        (Some(validator), _) => &validator.diagnostics,
        (None, Some(compiler)) => &compiler.diagnostics,
        (None, None) => return None,
    };
    let codes: BTreeSet<&str> = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .filter(|code| explain::lookup(code).is_some())
        .collect();
    let first = codes.first()?;
    let list = codes.iter().copied().collect::<Vec<_>>().join(", ");
    let only_errors = diagnostics
        .iter()
        .all(|diagnostic| diagnostic.severity == scxml_reporting::Severity::Error);
    let noun = if only_errors { "error" } else { "diagnostic" };
    Some(if codes.len() == 1 {
        format!("For more information about this {noun}, try `scxml-cli --explain {first}`.\n")
    } else {
        format!(
            "Some diagnostics have detailed explanations: {list}.\nFor more information about a diagnostic, try `scxml-cli --explain {first}`.\n"
        )
    })
}

/// Parses the arguments after `--explain` / `explain`: exactly one CODE.
pub fn parse_explain(args: &[String]) -> Result<String, String> {
    match args {
        [code] => Ok(code.clone()),
        [] => Err("--explain requires a diagnostic CODE such as SCXML005".to_owned()),
        _ => Err("--explain accepts exactly one CODE".to_owned()),
    }
}

/// Returns the long-form explanation for `code`, or an actionable error.
pub fn explain_code(code: &str) -> Result<String, String> {
    let span = observe(Module::Cli, Operation::CliExplain);
    let result = explain::lookup(code).map(explain::explanation).ok_or_else(|| {
        let nearest = explain::CODES
            .iter()
            .map(|entry| entry.code)
            .min_by_key(|candidate| distance(code, candidate));
        match nearest {
            Some(candidate) if distance(code, candidate) <= 2 => format!(
                "no extended explanation for `{code}`; did you mean `{candidate}`?"
            ),
            _ if code.starts_with("SNAP-") => format!(
                "no extended explanation for `{code}`; session snapshot codes are listed in docs/session-snapshot.md"
            ),
            _ => format!(
                "no extended explanation for `{code}`; documented codes are listed in docs/validator-rules.md"
            ),
        }
    });
    if result.is_err() {
        span.fail(ErrorCategory::InvalidInput);
    }
    result
}

/// Bounded case-insensitive edit distance for short diagnostic codes and
/// option names.
pub fn distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.to_ascii_uppercase().chars().take(64).collect();
    let right: Vec<char> = right.to_ascii_uppercase().chars().take(64).collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (row, left_char) in left.iter().enumerate() {
        let mut current = vec![row + 1; right.len() + 1];
        for (column, right_char) in right.iter().enumerate() {
            current[column + 1] = (previous[column] + usize::from(left_char != right_char))
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
        }
        previous = current;
    }
    previous[right.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explain_renders_documented_codes_and_suggests_near_misses() {
        let text = explain_code("SCXML005").expect("SCXML005 is documented");
        assert!(text.starts_with("SCXML005 (error · semantic)"));
        assert!(text.contains("Erroneous example:"));
        assert!(text.contains("https://www.w3.org/TR/scxml/#transition"));
        assert!(explain_code("XSD-cvc-datatype-valid.1.2.1").is_ok());
        assert_eq!(
            explain_code("SCXML05"),
            Err("no extended explanation for `SCXML05`; did you mean `SCXML005`?".into())
        );
        assert!(explain_code("E0425").is_err());
    }

    #[test]
    fn format_parsing_is_closed() {
        assert_eq!(parse_format(Some(&"json".into())), Ok(OutputFormat::Json));
        assert!(parse_format(Some(&"yaml".into())).is_err());
        assert!(parse_format(None).is_err());
    }
}
