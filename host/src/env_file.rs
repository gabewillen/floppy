//! Env-file values: Docker's `--env-file` format, exactly as `scxml-cli`
//! reads it.
//!
//! One entry per line. `NAME=VALUE` sets `NAME` to everything after the first
//! `=`, taken literally: no quote stripping, no escapes, no `$VAR` expansion,
//! no `export` prefix. A bare `NAME` takes the value the caller's `shell`
//! lookup answers, and is skipped when it answers none. Blank lines and lines
//! whose first character is `#` are ignored; a trailing `\r` is part of no
//! entry. A later entry replaces an earlier one. Errors name the file and the
//! line, never a value.
//!
//! The run takes only the variables its config grants ([`Values::granted`]);
//! every other entry is dropped when the file is read. A granted variable the
//! file does not set takes the config's non-secret default (`env-defaults`),
//! when it has one.

use std::path::Path;

use crate::telemetry::{Operation, observe};
use crate::{FailureKind, HostFailure};

/// Maximum bytes of one env file.
pub const MAX_ENV_FILE_BYTES: u64 = 64 * 1024;
/// Maximum entries of one env file.
pub const MAX_ENV_FILE_ENTRIES: usize = 1_024;
/// Maximum bytes of one variable name.
pub const MAX_NAME_BYTES: usize = 256;
/// Maximum bytes of one value.
pub const MAX_VALUE_BYTES: usize = 32 * 1024;

/// Whether `name` is a variable name: `[A-Za-z_][A-Za-z0-9_]*`, at most
/// [`MAX_NAME_BYTES`] bytes.
#[must_use]
pub fn is_name(name: &str) -> bool {
    name.len() <= MAX_NAME_BYTES
        && name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// The run's granted variables and their values. `Debug` shows names only.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct Values {
    entries: Vec<(String, String)>,
}

impl std::fmt::Debug for Values {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_set()
            .entries(self.entries.iter().map(|(name, _)| name))
            .finish()
    }
}

impl Values {
    /// No values.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Checked entries as values (the config's `env-defaults`).
    pub(crate) const fn from_entries(entries: Vec<(String, String)>) -> Self {
        Self { entries }
    }

    /// Reads `path` and keeps exactly the `granted` variables; one the file
    /// does not set takes its entry in `defaults` (the config's non-secret
    /// `env-defaults`), when there is one.
    ///
    /// # Errors
    ///
    /// [`FailureKind::EnvFile`] for an unreadable or oversized file, invalid
    /// UTF-8, an invalid entry, or a granted variable neither the file nor
    /// `defaults` sets.
    pub fn granted(
        path: &Path,
        granted: &[String],
        defaults: &Self,
        shell: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, HostFailure> {
        let span = observe(Operation::ReadEnvFile);
        let result = Self::granted_inner(path, granted, defaults, shell);
        span.finish(&result);
        result
    }

    fn granted_inner(
        path: &Path,
        granted: &[String],
        defaults: &Self,
        shell: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, HostFailure> {
        let error = |message: String| HostFailure::new(FailureKind::EnvFile, message);
        let shown = path.display();
        let bytes = crate::read_bounded(path, MAX_ENV_FILE_BYTES)
            .map_err(|problem| error(format!("cannot read the env file {problem}")))?;
        let text =
            String::from_utf8(bytes).map_err(|_| error(format!("`{shown}` is not UTF-8")))?;
        let entries = parse(&text, shell)
            .map_err(|(line, problem)| error(format!("`{shown}`:{line}: {problem}")))?;
        let mut kept = Vec::with_capacity(granted.len());
        for name in granted {
            let Some((_, value)) = entries
                .iter()
                .chain(&defaults.entries)
                .find(|(entry, _)| entry == name)
            else {
                return Err(error(format!(
                    "`{shown}` does not set the granted variable `{name}`, and the config gives it no default"
                )));
            };
            kept.push((name.clone(), value.clone()));
        }
        Ok(Self { entries: kept })
    }

    /// The granted variable names, in grant order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(name, _)| name.as_str())
    }

    /// Hands the values to the session host's environment (references only
    /// ever reach components).
    ///
    /// # Errors
    ///
    /// [`FailureKind::EnvFile`] when the host refuses a name or value.
    pub fn environment(&self) -> Result<scxml_host_wasmtime::ExtensionEnvironment, HostFailure> {
        scxml_host_wasmtime::ExtensionEnvironment::new(self.entries.clone()).map_err(|error| {
            HostFailure::new(
                FailureKind::EnvFile,
                format!("the session host refuses the granted variables: {error}"),
            )
        })
    }
}

/// Parses env-file text; `shell` answers a bare `NAME` line.
///
/// # Errors
///
/// The 1-based line and what is wrong with it (never the value).
pub fn parse(
    text: &str,
    shell: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<(String, String)>, (usize, String)> {
    let mut entries: Vec<(String, String)> = Vec::new();
    for (index, line) in text.split('\n').enumerate() {
        let number = index.saturating_add(1);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        if entries.len() >= MAX_ENV_FILE_ENTRIES {
            return Err((number, format!("more than {MAX_ENV_FILE_ENTRIES} entries")));
        }
        let (name, value) = match line.split_once('=') {
            Some((name, value)) => (name, Some(value.to_owned())),
            None => (line, None),
        };
        if !is_name(name) {
            return Err((
                number,
                "the entry's name is not a variable name ([A-Za-z_][A-Za-z0-9_]*)".to_owned(),
            ));
        }
        let Some(value) = value.or_else(|| shell(name)) else {
            continue;
        };
        if value.len() > MAX_VALUE_BYTES || value.contains('\0') {
            return Err((
                number,
                format!("the value is longer than {MAX_VALUE_BYTES} bytes or contains NUL"),
            ));
        }
        entries.retain(|(seen, _)| seen != name);
        entries.push((name.to_owned(), value));
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(name: &str) -> Option<String> {
        (name == "FROM_SHELL").then(|| "shell-value".to_owned())
    }

    #[test]
    fn entries_follow_docker_env_file_rules() {
        let text = "# c\r\nA=1\r\n\r\nB=x=y\nC=\"q\"\nFROM_SHELL\nUNSET\nA=2\n";
        assert_eq!(
            parse(text, &shell).expect("valid"),
            [
                ("B".to_owned(), "x=y".to_owned()),
                ("C".to_owned(), "\"q\"".to_owned()),
                ("FROM_SHELL".to_owned(), "shell-value".to_owned()),
                ("A".to_owned(), "2".to_owned()),
            ]
        );
    }

    #[test]
    fn invalid_entries_name_the_line_never_the_value() {
        let (line, problem) = parse("OK=1\nexport A=secret\n", &shell).expect_err("invalid");
        assert_eq!(line, 2);
        assert!(!problem.contains("secret"));
        let (line, _) = parse("A=a\0b\n", &shell).expect_err("NUL");
        assert_eq!(line, 1);
    }

    #[test]
    fn only_granted_variables_are_kept_and_debug_shows_names_only() {
        let directory = std::env::temp_dir().join(format!(
            "nemobot-host-env-file-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&directory).expect("the fixture directory is created");
        let path = directory.join("values.env");
        std::fs::write(&path, "KEEP=kept-secret\nDROP=dropped\n").expect("the file is written");
        let none = Values::none();
        let values = Values::granted(&path, &["KEEP".to_owned()], &none, &shell).expect("valid");
        assert_eq!(values.names().collect::<Vec<_>>(), ["KEEP"]);
        let shown = format!("{values:?}");
        assert!(shown.contains("KEEP") && !shown.contains("kept-secret"));
        let missing = Values::granted(&path, &["ABSENT".to_owned()], &none, &shell)
            .expect_err("a granted variable must be set");
        assert_eq!(missing.kind(), FailureKind::EnvFile);
        assert!(!missing.message().contains("kept-secret"));

        // A default fills only a variable the file does not set.
        let defaults = Values::from_entries(vec![
            ("ABSENT".to_owned(), "https://default.invalid".to_owned()),
            ("KEEP".to_owned(), "default-loses".to_owned()),
        ]);
        let values = Values::granted(
            &path,
            &["KEEP".to_owned(), "ABSENT".to_owned()],
            &defaults,
            &shell,
        )
        .expect("the default fills ABSENT");
        assert_eq!(
            values.entries,
            [
                ("KEEP".to_owned(), "kept-secret".to_owned()),
                ("ABSENT".to_owned(), "https://default.invalid".to_owned()),
            ],
            "the env file's value wins over the default"
        );
        std::fs::remove_dir_all(&directory).expect("the fixture directory is removed");
    }
}
