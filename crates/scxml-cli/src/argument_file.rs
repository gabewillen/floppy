//! `@PATH` argument files, as rustc reads them (`rustc @path`).
//!
//! A command-line argument `@PATH` before `--` is replaced by the lines of
//! the file at `PATH`, each line one argument, taken literally: no quoting,
//! no escapes, no comments, no variable expansion, and a line that itself
//! starts with `@` is not read as another file. A trailing `\r` ends a line
//! like `\n` (CRLF files read like LF files), and a blank line is an empty
//! argument (which the command rejects like any other empty argument). A
//! relative path inside the file is relative to the current directory, as
//! on the command line. Arguments after `--` are never expanded.
//!
//! The file is read once, with a regular-file check and a byte bound
//! ([`MAX_ARGUMENT_FILE_BYTES`]); it must be UTF-8. A file may hold any
//! flag, including `--env NAME@URI=VALUE`; such a value is masked in the
//! text the CLI writes like one given on the command line.

use std::path::Path;

/// Maximum bytes of one argument file.
pub(crate) const MAX_ARGUMENT_FILE_BYTES: u64 = 1024 * 1024;

/// Replaces every `@PATH` argument before `--` with the lines of `PATH`.
///
/// # Errors
///
/// An unreadable, non-regular, oversized, or non-UTF-8 argument file, or an
/// `@` with no path; the message names the file, never its contents.
pub(crate) fn expand(args: Vec<String>) -> Result<Vec<String>, String> {
    let mut expanded = Vec::with_capacity(args.len());
    let mut operands_only = false;
    for argument in args {
        if operands_only {
            expanded.push(argument);
            continue;
        }
        if argument == "--" {
            operands_only = true;
            expanded.push(argument);
            continue;
        }
        let Some(path) = argument.strip_prefix('@') else {
            expanded.push(argument);
            continue;
        };
        if path.is_empty() {
            return Err("`@` requires an argument file path (@PATH)".into());
        }
        expanded.extend(read(Path::new(path))?);
    }
    Ok(expanded)
}

/// The arguments of one argument file: its lines.
fn read(path: &Path) -> Result<Vec<String>, String> {
    let shown = path.display();
    let bytes = crate::extensions::read_bounded(path, MAX_ARGUMENT_FILE_BYTES)
        .map_err(|error| format!("cannot read the argument file `{shown}`: {error}"))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("the argument file `{shown}` is not UTF-8"))?;
    Ok(text.lines().map(str::to_owned).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn file(name: &str, contents: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "scxml-cli-argument-file-{}-{name}",
            std::process::id()
        ));
        std::fs::write(&path, contents).expect("the test argument file is writable");
        path
    }

    #[test]
    fn an_argument_file_is_replaced_by_its_lines_in_place() {
        let path = file(
            "lines",
            "--env\nKEY@urn:a=value with spaces\r\n--fuel\n7\n@not-a-file\n",
        );
        let expanded = expand(strings(&[
            "run",
            &format!("@{}", path.display()),
            "doc.scxml",
        ]))
        .expect("the argument file expands");
        assert_eq!(
            expanded,
            strings(&[
                "run",
                "--env",
                "KEY@urn:a=value with spaces",
                "--fuel",
                "7",
                "@not-a-file",
                "doc.scxml",
            ]),
            "lines are taken literally, CRLF reads like LF, and a nested @ is not read"
        );
        std::fs::remove_file(path).expect("removable");
    }

    #[test]
    fn a_blank_line_is_an_empty_argument() {
        let path = file("blank", "--fuel\n\n");
        let expanded =
            expand(strings(&[&format!("@{}", path.display())])).expect("the argument file expands");
        assert_eq!(expanded, strings(&["--fuel", ""]));
        std::fs::remove_file(path).expect("removable");
    }

    #[test]
    fn arguments_after_the_terminator_are_never_expanded() {
        let expanded = expand(strings(&["run", "--", "@doc.scxml"])).expect("nothing to expand");
        assert_eq!(expanded, strings(&["run", "--", "@doc.scxml"]));
    }

    #[test]
    fn a_missing_empty_or_non_utf8_argument_file_is_refused() {
        let missing = expand(strings(&["@/nonexistent/scxml-cli.args"]))
            .expect_err("a missing argument file is refused");
        assert!(
            missing.contains("cannot read the argument file"),
            "{missing}"
        );
        assert!(expand(strings(&["@"])).is_err(), "an @ with no path");
        let path = std::env::temp_dir().join(format!(
            "scxml-cli-argument-file-{}-binary",
            std::process::id()
        ));
        std::fs::write(&path, [0xff, 0xfe]).expect("writable");
        let binary = expand(strings(&[&format!("@{}", path.display())]))
            .expect_err("a non-UTF-8 argument file is refused");
        assert!(binary.contains("is not UTF-8"), "{binary}");
        std::fs::remove_file(path).expect("removable");
    }

    #[test]
    fn an_oversized_argument_file_is_refused() {
        let path = file(
            "oversized",
            &"x".repeat(usize::try_from(MAX_ARGUMENT_FILE_BYTES).expect("fits") + 1),
        );
        let error = expand(strings(&[&format!("@{}", path.display())]))
            .expect_err("an oversized argument file is refused");
        assert!(error.contains("byte bound"), "{error}");
        std::fs::remove_file(path).expect("removable");
    }
}
