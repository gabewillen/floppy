//! `--env-file PATH`: Docker's `--env-file` format, exactly.
//!
//! One entry per line. `NAME=VALUE` sets `NAME` to everything after the first
//! `=`, taken literally: no quote stripping, no escapes, no `$VAR` expansion,
//! no `export` prefix. A bare `NAME` takes the calling shell's value, and is
//! skipped when the shell does not set it. Blank lines and lines whose first
//! character is `#` are ignored; a trailing `\r` is part of no entry (CRLF
//! files read like LF files). An invalid name is rejected with its file and
//! line, never its value. The file is read once, bounded
//! ([`MAX_ENV_FILE_BYTES`], [`MAX_ENV_FILE_ENTRIES`], and the name and value
//! bounds).

use std::path::Path;

/// Maximum bytes of one env file.
pub(crate) const MAX_ENV_FILE_BYTES: u64 = 64 * 1024;
/// Maximum entries of one env file.
pub(crate) const MAX_ENV_FILE_ENTRIES: usize = 1_024;
/// Maximum bytes of one variable name.
pub(crate) const MAX_NAME_BYTES: usize = 256;
/// Maximum bytes of one value.
pub(crate) const MAX_VALUE_BYTES: usize = 32 * 1024;

/// Whether `name` is a variable name: `[A-Za-z_][A-Za-z0-9_]*`, at most
/// [`MAX_NAME_BYTES`] bytes.
pub(crate) fn is_name(name: &str) -> bool {
    name.len() <= MAX_NAME_BYTES
        && name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// Reads one env file.
///
/// # Errors
///
/// An unreadable or oversized file, invalid UTF-8, too many entries, an
/// invalid name, or an oversized value; the message names the file and the
/// line, never a value.
pub(crate) fn read(path: &Path) -> Result<Vec<(String, String)>, String> {
    let shown = path.display();
    let bytes = crate::extensions::read_bounded(path, MAX_ENV_FILE_BYTES)
        .map_err(|error| format!("cannot read --env-file `{shown}`: {error}"))?;
    let text =
        String::from_utf8(bytes).map_err(|_| format!("--env-file `{shown}` is not UTF-8"))?;
    parse(&text, &|name| std::env::var(name).ok())
        .map_err(|(line, problem)| format!("--env-file `{shown}`:{line}: {problem}"))
}

/// Parses env-file text; `shell` answers a bare `NAME` line.
///
/// # Errors
///
/// The 1-based line and what is wrong with it (never the value).
pub(crate) fn parse(
    text: &str,
    shell: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<(String, String)>, (usize, String)> {
    let mut entries: Vec<(String, String)> = Vec::new();
    for (index, line) in text.split('\n').enumerate() {
        let number = index + 1;
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
    use std::fmt::Write as _;

    use super::*;

    fn shell(name: &str) -> Option<String> {
        (name == "FROM_SHELL").then(|| "shell-value".to_owned())
    }

    #[test]
    fn entries_follow_docker_env_file_rules() {
        let text = "# a comment\r\nA=1\r\n\r\nB=x=y=z\nC=\"quoted\"\nD='single' $HOME \\n\nFROM_SHELL\nUNSET\n  \nE=\nexport_ok=1\n";
        assert_eq!(
            parse(text, &shell).expect("valid"),
            [
                ("A".to_owned(), "1".to_owned()),
                ("B".to_owned(), "x=y=z".to_owned()),
                ("C".to_owned(), "\"quoted\"".to_owned()),
                ("D".to_owned(), "'single' $HOME \\n".to_owned()),
                ("FROM_SHELL".to_owned(), "shell-value".to_owned()),
                ("E".to_owned(), String::new()),
                ("export_ok".to_owned(), "1".to_owned()),
            ]
        );
        assert_eq!(
            parse("A=1\nA=2\n", &shell).expect("valid"),
            [("A".to_owned(), "2".to_owned())],
            "a later line wins"
        );
    }

    #[test]
    fn invalid_names_name_the_line_and_never_the_value() {
        for (text, line) in [
            ("A=1\nexport B=secret-value\n", 2),
            ("1A=secret-value\n", 1),
            (" A=secret-value\n", 1),
            ("A-B=secret-value\n", 1),
            ("=secret-value\n", 1),
        ] {
            let (number, problem) = parse(text, &shell).expect_err("an invalid name");
            assert_eq!(number, line, "{text}");
            assert!(!problem.contains("secret-value"), "{problem}");
        }
    }

    #[test]
    fn files_are_bounded() {
        let many = "A=1\n".repeat(MAX_ENV_FILE_ENTRIES + 1);
        // Repeated names collapse, so count distinct names.
        let mut distinct = String::new();
        for index in 0..=MAX_ENV_FILE_ENTRIES {
            let _ = writeln!(distinct, "V{index}=1");
        }
        assert!(parse(&many, &shell).is_ok());
        assert!(parse(&distinct, &shell).is_err());
        let long = format!("A={}\n", "x".repeat(MAX_VALUE_BYTES + 1));
        assert!(parse(&long, &shell).is_err());
        let dir = std::env::temp_dir().join(format!("scxml-cli-env-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temporary directory");
        let big = dir.join("big.env");
        std::fs::write(
            &big,
            vec![b'#'; usize::try_from(MAX_ENV_FILE_BYTES).expect("fits") + 1],
        )
        .expect("file");
        assert!(read(&big).expect_err("oversized").contains("bound"));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
