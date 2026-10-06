//! Drift gate for the published release contract: the format versions and
//! the conformance evidence digests that README.md, docs/conformance.md,
//! CHANGELOG.md, and tests/conformance/w3c-irp/README.md publish must equal
//! what the code and the checked-in baselines actually are.
//!
//! Every expected value is derived here, never restated: the versions come
//! from the code constants, and the digests from hashing and decoding the
//! checked-in baselines. A version or baseline bump that misses the docs
//! fails here. CONTRIBUTING.md's proof matrix must also list every command,
//! installed tool version, and pinned action commit in
//! `.github/workflows/ci.yml`. No tracked file may carry private planning
//! labels, review-round numbers, rulings, or local paths, which a public
//! reader cannot resolve.
#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

use std::path::{Path, PathBuf};

use scxml_conformance::sha256_bytes;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The file at `relative`, with every whitespace run collapsed to one space
/// so a phrase wrapped across lines still matches.
fn prose(relative: &str) -> String {
    let text = std::fs::read_to_string(workspace().join(relative))
        .unwrap_or_else(|error| panic!("{relative} must be readable: {error}"));
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The `## [Unreleased]` section of CHANGELOG.md, white space collapsed as
/// [`prose`] does.
fn unreleased_changelog() -> String {
    let text = std::fs::read_to_string(workspace().join("CHANGELOG.md"))
        .unwrap_or_else(|error| panic!("CHANGELOG.md must be readable: {error}"));
    let section = text
        .split("\n## [")
        .find(|section| section.starts_with("Unreleased]"))
        .expect("CHANGELOG.md must have an `## [Unreleased]` section");
    section.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Every integer published in backticks right after `prefix` (for example
/// the `4` of "Machine IR `4`").
fn published_numbers(text: &str, prefix: &str) -> Vec<u32> {
    let needle = format!("{prefix} `");
    text.match_indices(&needle)
        .filter_map(|(start, _)| {
            let rest = &text[start + needle.len()..];
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            rest[digits.len()..]
                .starts_with('`')
                .then(|| digits.parse().ok())
                .flatten()
        })
        .collect()
}

/// Requires every publication of `prefix` in `document` to name `expected`,
/// and at least `minimum` of them.
fn assert_version(document: &str, text: &str, prefix: &str, expected: u32, minimum: usize) {
    let published = published_numbers(text, prefix);
    assert!(
        published.len() >= minimum,
        "{document} must publish \"{prefix} `{expected}`\" (found {published:?})"
    );
    for number in published {
        assert_eq!(
            number, expected,
            "{document} publishes \"{prefix} `{number}`\"; the code says {expected}"
        );
    }
}

#[test]
fn published_format_versions_match_the_code() {
    let ir = scxml_compiler_core::MACHINE_IR_VERSION;
    assert_eq!(ir, scxml_reporting::MACHINE_IR_VERSION);
    let report = scxml_reporting::CONFORMANCE_REPORT_VERSION;
    let wire = scxml_types::DELAYED_SEND_WIRE_VERSION;
    let session_snapshot = scxml_snapshot::SESSION_SNAPSHOT_VERSION;
    let session_snapshot_diff = scxml_snapshot::SESSION_SNAPSHOT_DIFF_VERSION;

    let readme = prose("README.md");
    assert_version("README.md", &readme, "Machine IR", ir, 1);
    assert_version("README.md", &readme, "Machine IR schema", ir, 1);
    assert_version("README.md", &readme, "conformance payload", report, 1);
    assert_version("README.md", &readme, "delayed-send wire record", wire, 1);
    for version in [session_snapshot, session_snapshot_diff] {
        assert!(
            readme.contains(&format!("`{version}`")),
            "README.md must publish the session snapshot version `{version}`"
        );
    }

    let conformance = prose("docs/conformance.md");
    assert_version(
        "docs/conformance.md",
        &conformance,
        "Machine IR schema",
        ir,
        1,
    );

    let irp = prose("tests/conformance/w3c-irp/README.md");
    let irp_name = "tests/conformance/w3c-irp/README.md";
    assert_version(irp_name, &irp, "Machine IR schema", ir, 1);
    assert_version(irp_name, &irp, "conformance report schema", report, 1);

    let changelog = prose("CHANGELOG.md");
    for version in [session_snapshot, session_snapshot_diff] {
        assert!(
            changelog.contains(&format!("`{version}`")),
            "CHANGELOG.md must publish the session snapshot version `{version}`"
        );
    }
    // Only the unreleased section publishes the current wire version: a
    // released section records what that release shipped and is never
    // rewritten.
    assert_version(
        "CHANGELOG.md",
        &unreleased_changelog(),
        "delayed-send wire records are version",
        wire,
        1,
    );
}

#[test]
fn published_conformance_digests_match_the_checked_in_baselines() {
    let documents = [
        ("README.md", prose("README.md")),
        ("docs/conformance.md", prose("docs/conformance.md")),
        (
            "tests/conformance/w3c-irp/README.md",
            prose("tests/conformance/w3c-irp/README.md"),
        ),
    ];
    let baselines = workspace().join("tests/conformance/w3c-irp/baselines");
    let sums = std::fs::read_to_string(baselines.join("SHA256SUMS"))
        .expect("the baseline checksum manifest is readable");
    for datamodel in ["ecmascript", "xpath"] {
        let name = format!("{datamodel}.scxml-report.json");
        let bytes = std::fs::read(baselines.join(&name)).expect("the baseline is readable");
        let artifact = sha256_bytes(&bytes);
        assert!(
            sums.lines()
                .any(|line| line == format!("{artifact}  {name}")),
            "SHA256SUMS must list {name} as {artifact}"
        );
        let decoded =
            scxml_reporting::decode_canonical(&bytes).expect("the baseline decodes and validates");
        let payload = decoded
            .conformance_payload()
            .expect("the baseline is a conformance artifact");
        let suite = &payload.identity.suite;
        let mut bound = vec![
            ("canonical artifact SHA-256", artifact.clone()),
            (
                "producer report digest",
                payload.identity.report_digest.clone(),
            ),
            ("corpus digest", suite.corpus_digest.clone()),
        ];
        if let Some(errata) = &suite.errata {
            bound.push(("errata SHA-256", errata.digest.clone()));
        }
        for (document, text) in &documents {
            for (label, digest) in &bound {
                assert!(
                    text.contains(&format!("`{digest}`")),
                    "{document} must publish the {datamodel} {label} `{digest}`"
                );
            }
        }
        let (_, conformance) = &documents[1];
        for (label, digest) in [
            ("source-set digest", &suite.source_set_digest),
            ("manifest digest", &suite.manifest_digest),
        ] {
            assert!(
                conformance.contains(&format!("{label} `{digest}`")),
                "docs/conformance.md must publish the {label} `{digest}`"
            );
        }
    }
}

/// Every trimmed, non-empty line inside a fenced `console` block of the
/// markdown `text`.
fn console_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if inside {
            if trimmed == "```" {
                inside = false;
            } else if !trimmed.is_empty() {
                lines.push(trimmed.to_owned());
            }
        } else if trimmed == "```console" {
            inside = true;
        }
    }
    assert!(!inside, "a console block is left unterminated");
    lines
}

/// What a contributor needs from `ci.yml` to reproduce CI: every shell line a
/// `run:` step executes (one per line of a block scalar), every `tool@version`
/// an install step installs, and every pinned action commit and Node.js
/// version.
struct CiSteps {
    run_lines: Vec<String>,
    tools: Vec<String>,
    pins: Vec<String>,
}

fn ci_steps(ci: &str) -> CiSteps {
    let mut steps = CiSteps {
        run_lines: Vec::new(),
        tools: Vec::new(),
        pins: Vec::new(),
    };
    let lines: Vec<&str> = ci.lines().collect();
    let mut index = 0;
    while let Some(line) = lines.get(index) {
        index += 1;
        let body = line.trim_start();
        let body = body.strip_prefix("- ").unwrap_or(body);
        let key_column = line.len() - body.len();
        if let Some(command) = body.strip_prefix("run: ") {
            if command == "|" {
                while let Some(next) = lines.get(index) {
                    let content = next.trim();
                    if !content.is_empty() && next.len() - next.trim_start().len() <= key_column {
                        break;
                    }
                    if !content.is_empty() {
                        steps.run_lines.push(content.to_owned());
                    }
                    index += 1;
                }
            } else {
                steps.run_lines.push(command.trim().to_owned());
            }
        } else if let Some(tools) = body.strip_prefix("tool: ") {
            steps
                .tools
                .extend(tools.split(',').map(|tool| tool.trim().to_owned()));
        } else if let Some(action) = body.strip_prefix("uses: ") {
            let commit = action
                .split_once('@')
                .and_then(|(_, rest)| rest.split_whitespace().next())
                .expect("every CI action is pinned with @<commit>");
            steps.pins.push(commit.to_owned());
        } else if let Some(version) = body.strip_prefix("node-version: ") {
            steps.pins.push(version.trim().to_owned());
        }
    }
    steps
}

/// A CI line that exports a value to later steps (`echo "NAME=value" >>
/// "$GITHUB_ENV"`) is the plain shell assignment `NAME=value` locally.
fn local_equivalent(line: &str) -> &str {
    line.strip_prefix("echo \"")
        .and_then(|rest| rest.strip_suffix("\" >> \"$GITHUB_ENV\""))
        .unwrap_or(line)
}

#[test]
fn contributing_lists_every_ci_step_and_pin() {
    let ci = std::fs::read_to_string(workspace().join(".github/workflows/ci.yml"))
        .expect(".github/workflows/ci.yml must be readable");
    let contributing = std::fs::read_to_string(workspace().join("CONTRIBUTING.md"))
        .expect("CONTRIBUTING.md must be readable");
    let documented = console_lines(&contributing);
    let steps = ci_steps(&ci);
    assert!(
        steps.run_lines.len() >= 20 && !steps.tools.is_empty() && steps.pins.len() >= 4,
        "the ci.yml scan found too few steps; the parser no longer matches ci.yml"
    );
    for line in &steps.run_lines {
        let local = local_equivalent(line);
        assert!(
            documented.iter().any(|documented| documented == local),
            "CONTRIBUTING.md must list the ci.yml command line `{local}` in a console block"
        );
    }
    for tool in &steps.tools {
        assert!(
            documented.iter().any(|line| line.contains(tool.as_str())),
            "CONTRIBUTING.md must install the ci.yml tool `{tool}` in a console block"
        );
    }
    for pin in &steps.pins {
        assert!(
            contributing.contains(&format!("`{pin}`")),
            "CONTRIBUTING.md must publish the ci.yml pin `{pin}`"
        );
    }
}

/// Whether `word` looks like a private planning label: one uppercase ASCII
/// letter followed by digits only (the form of a work-stream tag).
fn planning_label(word: &str) -> bool {
    let mut chars = word.chars();
    chars.next().is_some_and(|first| first.is_ascii_uppercase())
        && !chars.as_str().is_empty()
        && chars.all(|c| c.is_ascii_digit())
}

/// Whether `word` is "lane" or "lanes", in any case.
fn lane_word(word: &str) -> bool {
    word.eq_ignore_ascii_case("lane") || word.eq_ignore_ascii_case("lanes")
}

/// The internal-process references in `line`, if any: private planning
/// labels next to the word "lane", review-round numbers, rulings, and
/// local paths. None of them resolve for a public reader.
fn process_references(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    for needle in ["contract §", "~/.agents", "/Users/"] {
        if line.contains(needle) {
            found.push(needle.to_owned());
        }
    }
    let words: Vec<&str> = line
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    for (index, word) in words.iter().enumerate() {
        let next = words.get(index + 1).copied().unwrap_or("");
        if (lane_word(word) && planning_label(next)) || (planning_label(word) && lane_word(next)) {
            found.push(format!("{word} {next}"));
        }
        if ["ruling", "rulings", "ruled"]
            .iter()
            .any(|ruling| word.eq_ignore_ascii_case(ruling))
        {
            found.push((*word).to_owned());
        }
        if owner_label(word, next, words.get(index + 2).copied().unwrap_or("")) {
            found.push(format!("{word} {next}"));
        }
    }
    // "round N" as prose (one space, then a whole number), not a call such
    // as XPath's `round(2.5)` or a decimal.
    let lower = line.to_ascii_lowercase();
    for (start, _) in lower.match_indices("round ") {
        let before = lower[..start].chars().next_back();
        let rest = &lower[start + "round ".len()..];
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        let after = rest[digits..].chars().next();
        if !before.is_some_and(|c| c.is_ascii_alphanumeric())
            && digits > 0
            && !after.is_some_and(|c| c.is_ascii_alphanumeric() || c == '.')
        {
            found.push(line[start..start + "round ".len() + digits].to_owned());
        }
    }
    for token in line.split(|c: char| !c.is_ascii_alphanumeric() && c != '-') {
        if review_label(token) {
            found.push(token.to_owned());
        }
    }
    found
}

/// Whether `token` (a hyphen-joined run of ASCII letters and digits) names a
/// review round or finding: "round" joined to a number ("round-11",
/// "Round11"), "sec" followed by a round tag ("sec-r3", "SEC-R11-1"), or a
/// round tag followed by a lettered finding ("R11-C1"), in any case. A round
/// tag is `r` then digits only, so diagnostic codes such as `SCXML061`,
/// `COMP003`, or `SNAP-*`, RFC numbers, and hex digests do not match.
fn review_label(token: &str) -> bool {
    let digits = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit());
    let lettered = |part: &str, letter: Option<char>| {
        let mut chars = part.chars();
        chars.next().is_some_and(|first| {
            first.is_ascii_alphabetic()
                && letter.is_none_or(|letter| first.eq_ignore_ascii_case(&letter))
        }) && digits(chars.as_str())
    };
    let round_tag = |part: &str| lettered(part, Some('r'));
    let lower = token.to_ascii_lowercase();
    let parts: Vec<&str> = lower.split('-').filter(|part| !part.is_empty()).collect();
    if parts
        .iter()
        .any(|part| part.strip_prefix("round").is_some_and(digits))
    {
        return true;
    }
    parts.windows(2).any(|pair| {
        (pair[0] == "round" && digits(pair[1]))
            || (pair[0] == "sec" && round_tag(pair[1]))
            || (round_tag(pair[0]) && lettered(pair[1], None))
    })
}

/// A private decision attributed to the project's owner: the word for owner
/// followed by a decision word (directly or after a possessive), or "per
/// the" followed by it, in any case. Shipped text states the requirement
/// itself instead.
fn owner_label(word: &str, next: &str, after: &str) -> bool {
    let owner = |w: &str| w.eq_ignore_ascii_case("owner") || w.eq_ignore_ascii_case("owners");
    let decision = |w: &str| {
        [
            "decision",
            "decisions",
            "decided",
            "decides",
            "ruling",
            "rulings",
            "ruled",
            "said",
            "says",
            "approved",
            "asked",
            "wants",
            "requested",
        ]
        .iter()
        .any(|label| w.eq_ignore_ascii_case(label))
    };
    // A possessive splits into the word, `s`, and the decision word.
    (owner(word) && (decision(next) || (next == "s" && decision(after))))
        || (word.eq_ignore_ascii_case("per") && next.eq_ignore_ascii_case("the") && owner(after))
}

#[test]
fn owner_label_detector_matches_its_targets() {
    let owner = "Owner";
    for leaky in [
        format!("/// {owner} decision 2026-09-27: one event"),
        format!("an {} ruling fixed this", owner.to_ascii_lowercase()),
        format!("the {}'s ruling", owner.to_ascii_uppercase()),
        format!("per the {owner}, entry is linear"),
        format!("as the {owner} decided"),
        format!("the {owner} said so"),
    ] {
        assert!(!process_references(&leaky).is_empty(), "{leaky}");
    }
    for clean in [
        "the lease owner releases the span",
        "fn owner(&self) -> SessionId",
        "the owner of the compiled component decides nothing",
        "owners keep their handles",
        "per the WIT contract",
    ] {
        assert!(process_references(clean).is_empty(), "{clean}");
    }
}

#[test]
fn process_reference_detector_matches_its_targets() {
    let label = ["B", "4"].concat();
    let round = "round";
    let ruled = "ruled";
    for leaky in [
        format!("decided in lane {label}"),
        format!("({label} lane, see)"),
        format!("the Lanes {label} and S1"),
        format!("the {round} 9 repro"),
        format!("as {ruled} earlier"),
        format!("the {round}-11 repro"),
        format!("// {}-11 security regression", "Round"),
        format!("fixed in {round}11"),
        format!("({}-R11-1): a retained function", "SEC"),
        format!("// {}-r3: heads were not marked", "sec"),
        format!("see {}-C1", "R11"),
        format!("the {} finding", ["SEC", "R4"].join("-")),
    ] {
        assert!(!process_references(&leaky).is_empty(), "{leaky}");
    }
    for clean in [
        "spans.resize(lane + 1, (0, 0, false));",
        "let lanes = Lanes::of(annotations);",
        "a round-trip through the codec",
        "rounded to 2 places",
        "assert_eq!(number(\"round(2.5)\"), 3.0);",
        "SCXML 1.0 section 5.3",
        "diagnostic SCXML061 at 3:7",
        "error[COMP003]: missing export",
        "XSD001 schema violation",
        "SNAP-001 snapshot mismatch",
        "see RFC 3986 and RFC-2119",
        "sha256:4f1e2d3c-9a8b-r7c6",
        "the round-robin scheduler",
        "a background task named rounds",
        "exec-r3 and secr3 are not labels",
        "R2 register and r10-arm64 target",
        "x86_64-unknown-linux-gnu",
    ] {
        assert!(process_references(clean).is_empty(), "{clean}");
    }
}

#[test]
fn tracked_files_carry_no_internal_process_references() {
    let listed = std::process::Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(workspace())
        .output()
        .expect("git must be runnable to list tracked files");
    assert!(listed.status.success(), "git ls-files must succeed");
    let mut leaks = Vec::new();
    for relative in listed.stdout.split(|byte| *byte == 0) {
        let Ok(relative) = std::str::from_utf8(relative) else {
            continue;
        };
        // This file names the patterns it looks for.
        if relative.is_empty() || Path::new(relative) == Path::new(file!()) {
            continue;
        }
        // Binary and non-UTF-8 files carry no prose.
        let Ok(text) = std::fs::read_to_string(workspace().join(relative)) else {
            continue;
        };
        for (number, line) in text.lines().enumerate() {
            for reference in process_references(line) {
                leaks.push(format!("{relative}:{}: {reference}", number + 1));
            }
        }
    }
    assert!(
        leaks.is_empty(),
        "tracked files reference private process labels a public reader cannot resolve:\n{}",
        leaks.join("\n")
    );
}
