#!/usr/bin/env python3
"""Reject authored unsafe Rust in component crates while allowing generated ABI glue."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


COMPONENT_CRATES = (
    "scxml-clock",
    "scxml-compiler",
    "scxml-data-model-ecmascript",
    "scxml-data-model-null",
    "scxml-data-model-xpath",
    "scxml-extension-example",
    "scxml-ioprocessor-basichttp",
    "scxml-ioprocessor-scxml",
    "scxml-machine",
    "scxml-reporter-rustc",
    "scxml-reporter-sarif",
    "scxml-script-example",
    "scxml-service-example",
    "scxml-service-files",
    "scxml-snapshot-codec",
    "scxml-validator",
)

# Executable-content extension packages under namespaces/, keyed by directory
# name: each is an extension component crate held to the same contract. The
# inventory counts only directories with a Cargo.toml, so a vocabulary-only
# namespace (namespaces/env, namespaces/event) is not a crate and is not
# listed.
NAMESPACE_CRATES: tuple[str, ...] = ("scx",)

WORKSPACE_LINTS = re.compile(r"(?m)^\[lints\]\s*\nworkspace\s*=\s*true\s*$")
WASM_UNSAFE_ALLOWANCE = re.compile(
    r"#!\[cfg_attr\(\s*target_arch\s*=\s*\"wasm32\"\s*,\s*"
    r"allow\(\s*unsafe_code\s*,\s*reason\s*=\s*\"[^\"]+\"\s*\)\s*\)\]"
)
NATIVE_UNSAFE_FORBID = re.compile(
    r"#!\[cfg_attr\(\s*not\(target_arch\s*=\s*\"wasm32\"\)\s*,\s*"
    r"forbid\(unsafe_code\)\s*\)\]"
)
WIT_BINDGEN_GENERATE = re.compile(r"\bwit_bindgen\s*::\s*generate\s*!")


def raw_string_end(source: str, start: int) -> tuple[int, str] | None:
    """Return the raw string body start and closing delimiter at ``start``."""
    cursor = start
    if source.startswith(("br", "rb"), cursor):
        cursor += 2
    elif cursor < len(source) and source[cursor] == "r":
        cursor += 1
    else:
        return None

    hash_start = cursor
    while cursor < len(source) and source[cursor] == "#":
        cursor += 1
    if cursor >= len(source) or source[cursor] != '"':
        return None
    return cursor + 1, '"' + source[hash_start:cursor]


def authored_unsafe_locations(source: str) -> list[tuple[int, int]]:
    """Locate unsafe keyword tokens outside comments and string/character literals."""
    locations: list[tuple[int, int]] = []
    index = 0
    line = 1
    column = 1
    length = len(source)

    def advance(end: int) -> None:
        nonlocal index, line, column
        consumed = source[index:end]
        newline_count = consumed.count("\n")
        if newline_count:
            line += newline_count
            column = len(consumed.rsplit("\n", 1)[1]) + 1
        else:
            column += len(consumed)
        index = end

    while index < length:
        if source.startswith("//", index):
            end = source.find("\n", index + 2)
            advance(length if end == -1 else end)
            continue

        if source.startswith("/*", index):
            depth = 1
            cursor = index + 2
            while cursor < length and depth:
                if source.startswith("/*", cursor):
                    depth += 1
                    cursor += 2
                elif source.startswith("*/", cursor):
                    depth -= 1
                    cursor += 2
                else:
                    cursor += 1
            advance(cursor)
            continue

        raw = raw_string_end(source, index)
        if raw is not None:
            body_start, delimiter = raw
            end = source.find(delimiter, body_start)
            advance(length if end == -1 else end + len(delimiter))
            continue

        if source[index] == '"' or (
            source[index] in ("b", "c")
            and index + 1 < length
            and source[index + 1] in ('"', "'")
        ):
            quote_index = index + 1 if source[index] in ("b", "c") else index
            quote = source[quote_index]
            cursor = quote_index + 1
            while cursor < length:
                if source[cursor] == "\\":
                    cursor += 2
                elif source[cursor] == quote:
                    cursor += 1
                    break
                elif quote == "'" and source[cursor] == "\n":
                    break
                else:
                    cursor += 1
            advance(cursor)
            continue

        if source[index] == "'":
            character = re.match(r"'(?:\\(?:.|\n)|[^'\\\n])'", source[index:])
            if character is not None:
                advance(index + len(character.group(0)))
                continue

        if source.startswith("r#", index) and index + 2 < length:
            raw_identifier = re.match(r"r#[A-Za-z_][A-Za-z0-9_]*", source[index:])
            if raw_identifier is not None:
                advance(index + len(raw_identifier.group(0)))
                continue

        identifier = re.match(r"[A-Za-z_][A-Za-z0-9_]*", source[index:])
        if identifier is not None:
            token = identifier.group(0)
            if token == "unsafe":
                locations.append((line, column))
            advance(index + len(token))
            continue

        advance(index + 1)

    return locations


def verify(components_dir: Path, expected: tuple[str, ...] = COMPONENT_CRATES) -> list[str]:
    failures: list[str] = []
    actual_crates = tuple(
        sorted(path.parent.name for path in components_dir.glob("*/Cargo.toml"))
    )
    if actual_crates != expected:
        failures.append(
            f"component crate inventory mismatch in {components_dir}: expected "
            f"{', '.join(expected)}; found {', '.join(actual_crates)}"
        )
        return failures

    for crate_name in expected:
        crate_dir = components_dir / crate_name
        manifest = crate_dir / "Cargo.toml"
        manifest_text = manifest.read_text(encoding="utf-8")
        if WORKSPACE_LINTS.search(manifest_text) is None:
            failures.append(f"{manifest}: missing [lints] workspace = true")

        rust_sources = tuple(sorted(crate_dir.rglob("*.rs")))
        if not rust_sources:
            failures.append(f"{crate_dir}: component has no Rust source")
            continue

        crate_root = crate_dir / "src" / "lib.rs"
        try:
            crate_root_text = crate_root.read_text(encoding="utf-8")
        except FileNotFoundError:
            failures.append(f"{crate_root}: missing component crate root")
            crate_root_text = ""
        if NATIVE_UNSAFE_FORBID.search(crate_root_text) is None:
            failures.append(
                f"{crate_root}: missing native forbid(unsafe_code) crate-root contract"
            )
        if WASM_UNSAFE_ALLOWANCE.search(crate_root_text) is None:
            failures.append(
                f"{crate_root}: missing justified wasm32 allow(unsafe_code) crate-root "
                "attribute for generated bindings"
            )
        combined_source = "\n".join(
            source.read_text(encoding="utf-8") for source in rust_sources
        )
        if WIT_BINDGEN_GENERATE.search(combined_source) is None:
            failures.append(f"{crate_dir}: missing wit_bindgen::generate! ABI boundary")

        for source in rust_sources:
            source_text = source.read_text(encoding="utf-8")
            for line, column in authored_unsafe_locations(source_text):
                failures.append(
                    f"{source}:{line}:{column}: authored unsafe keyword is forbidden; "
                    "only wit-bindgen macro expansion may contain unsafe code"
                )

    return failures


def main() -> int:
    parser = argparse.ArgumentParser(
        description="verify the narrow generated-binding unsafe exception for components"
    )
    parser.add_argument(
        "--components-dir",
        type=Path,
        default=Path(__file__).resolve().parent.parent / "components",
        help="component source root (defaults to the workspace components directory)",
    )
    parser.add_argument(
        "--namespaces-dir",
        type=Path,
        default=Path(__file__).resolve().parent.parent / "namespaces",
        help="executable-content extension package root (defaults to the workspace namespaces directory)",
    )
    args = parser.parse_args()

    try:
        failures = verify(args.components_dir.resolve()) + verify(
            args.namespaces_dir.resolve(), NAMESPACE_CRATES
        )
    except (OSError, UnicodeError) as error:
        print(f"component unsafe verification failed closed: {error}", file=sys.stderr)
        return 1

    if failures:
        for failure in failures:
            print(failure, file=sys.stderr)
        return 1

    print(
        f"verified authored-unsafe ban for {len(COMPONENT_CRATES) + len(NAMESPACE_CRATES)} "
        "component crates; "
        "wit-bindgen macro expansion remains the sole unsafe exception"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
