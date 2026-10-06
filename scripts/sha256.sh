# shellcheck shell=sh
# Sourced (not executed) by scripts that digest files.
#
# Contract: `sha256_file PATH` prints the lowercase hex SHA-256 of PATH on
# stdout and nothing else, using `sha256sum` (GNU coreutils, most Linux)
# when present and `shasum -a 256` (macOS, Perl) otherwise. It returns
# nonzero, with the tool's own error on stderr, when PATH is unreadable,
# and 127 when neither tool is installed. It sets only the global
# `sha256_file_out`.
sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256_file_out=$(sha256sum -- "$1") || return
    elif command -v shasum >/dev/null 2>&1; then
        sha256_file_out=$(shasum -a 256 -- "$1") || return
    else
        echo "sha256_file: neither sha256sum nor shasum is installed" >&2
        return 127
    fi
    printf '%s\n' "${sha256_file_out%% *}"
}
