//! Canonical long-form documentation for every stable diagnostic code.
//!
//! This table is the single source of truth for `scx --explain CODE`,
//! the generated code table in `docs/validator-rules.md`, the SARIF
//! `tool.driver.rules` metadata, and the default `note:`/`help:` attached by
//! the validator's precision pass when a producing rule did not supply its
//! own. It is static documentation data: reporters read it without rerunning
//! any validation. Tests in `scxml-validator-core` keep the consumers
//! synchronized:
//!
//! - every self-checking `bad` example must emit its code and every `fixed`
//!   example must not (validated against the bundled W3C schema);
//! - the generated docs section must equal [`markdown_table`];
//! - every code literal emitted by the validator sources must have an entry.
//!
//! Family entries (codes ending in a constraint prefix such as
//! `XSD-cvc-datatype-valid`) document every backend code that starts with
//! that prefix at a `.` boundary.

use std::fmt;

/// Long-form documentation for one stable diagnostic code or code family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CodeDoc {
    /// Stable code, or a code-family prefix when [`Self::family`] is true.
    pub code: &'static str,
    /// Whether [`Self::code`] documents every code with this prefix.
    pub family: bool,
    /// Default severity (`error`, `warning`, or `note`).
    pub severity: &'static str,
    /// Stable category (`syntax`, `schema`, `semantic`, `lint`, `package`,
    /// `internal`).
    pub category: &'static str,
    /// One-line summary of what the diagnostic reports.
    pub title: &'static str,
    /// Why the constraint exists; used as the default `note:`.
    pub why: &'static str,
    /// Concrete repair direction; used as the default `help:`.
    pub help: &'static str,
    /// Example that triggers the diagnostic.
    pub bad: &'static str,
    /// Repaired example.
    pub fixed: &'static str,
    /// Whether `bad`/`fixed` are complete documents validated by tests with
    /// only the bundled W3C schema.
    pub self_checking: bool,
    /// Normative citation.
    pub spec: &'static str,
    /// Link to the cited section.
    pub url: &'static str,
}

const SCXML_TR: &str = "https://www.w3.org/TR/scxml/";

macro_rules! doc {
    (
        $code:literal, $severity:literal, $category:literal, $title:literal,
        why: $why:literal, help: $help:literal,
        bad: $bad:literal, fixed: $fixed:literal, checked: $checked:literal,
        spec: $spec:literal, url: $url:expr $(,)?
    ) => {
        CodeDoc {
            code: $code,
            family: false,
            severity: $severity,
            category: $category,
            title: $title,
            why: $why,
            help: $help,
            bad: $bad,
            fixed: $fixed,
            self_checking: $checked,
            spec: $spec,
            url: $url,
        }
    };
}

macro_rules! family {
    (
        $code:literal, $title:literal,
        why: $why:literal, help: $help:literal,
        bad: $bad:literal, fixed: $fixed:literal, checked: $checked:literal,
        spec: $spec:literal, url: $url:expr $(,)?
    ) => {
        CodeDoc {
            code: $code,
            family: true,
            severity: "error",
            category: "schema",
            title: $title,
            why: $why,
            help: $help,
            bad: $bad,
            fixed: $fixed,
            self_checking: $checked,
            spec: $spec,
            url: $url,
        }
    };
}

/// Every documented code, grouped by code prefix.
pub const CODES: &[CodeDoc] = &[
    doc!(
        "COMP001", "error", "internal", "the validated document could not be lowered to Machine IR",
        why: "the compiler lowers only constructs whose semantics the engine implements and fails closed instead of dropping anything; `validate` reports every structural rejection lowering makes (with its own code, span, and fix), so after a clean validation only a bound measured on the lowered machine, the 16 MiB encoded Machine IR, remains. Machine IR bounds apply to wasm builds only (the shipped compiler component); a native `scx compile` lowers a document of any size, and only the `u32` index width of the IR format limits it",
        help: "reduce the machine below the named bound, for example by splitting it into machines it invokes, or compile with the native `scx`; a document that validates cleanly but fails with any other `COMP001` is a validator gap worth reporting",
        bad: "a validator-clean document with tens of thousands of states whose encoded Machine IR exceeds 16 MiB",
        fixed: "the same behavior split across a parent machine and child machines it `<invoke>`s, each under the bound",
        checked: false,
        spec: "SCXML 1.0 §3.2 <scxml>", url: "https://www.w3.org/TR/scxml/#scxml",
    ),
    doc!(
        "COMP002", "error", "package", "a used extension namespace has no verified extension package",
        why: "an extension namespace executes in a separately deployed Wasm extension component; compiling without an exact verified package binding would silently degrade into core semantics",
        help: "supply one exact verified extension package and schema binding for every used extension namespace",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" xmlns:x=\"urn:x\" version=\"1.0\"><state id=\"a\"><onentry><x:act/></onentry></state></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><state id=\"a\"><onentry><log expr=\"'act'\"/></onentry><transition event=\"done\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        checked: false,
        spec: "SCXML 1.0 §4.10 Extensibility of Executable Content", url: "https://www.w3.org/TR/scxml/#extensibility",
    ),
    doc!(
        "COMP003", "error", "package", "a `<script src>` cannot be resolved or authenticated at compile time",
        why: "SCXML 1.0 §5.8 rejects a document whose `src` script cannot be fetched or parsed at load; compile-time resolution pins every `src` to exactly one caller-supplied script package (media type, SHA-256, and for Wasm the `stateforward:scxml/script` world), so a missing, unused, duplicated, mismatched, or non-conforming package is rejected instead of deferred",
        help: "supply one script package per exact `src` URI whose bytes hash to the pinned SHA-256 and whose media type is `application/wasm` (a script-world component) or the data model's script language, or compile with `runtime` script resolution so the host's script loader resolves it at load",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><state id=\"a\"><onentry><script src=\"example.wasm\"/></onentry><transition event=\"done\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        fixed: "the same document compiled with `--script example.wasm=example.wasm --script-sha256 example.wasm=…` (the package's SHA-256)",
        checked: false,
        spec: "SCXML 1.0 §5.8 <script>", url: "https://www.w3.org/TR/scxml/#script",
    ),
    doc!(
        "PKG001", "error", "package", "an extension package manifest does not match its schema binding",
        why: "a package binds an exact namespace URI, schema artifact and SHA-256, and WIT world/version; any mismatch means the schema that governs structure is not the one the extension component was built for",
        help: "regenerate the manifest from the exact schema bytes and supported WIT identity, or supply the matching schema",
        bad: "schema-sha256 = 000000…  (does not match the supplied schema bytes)",
        fixed: "schema-sha256 = <lowercase SHA-256 of the exact schema bytes>",
        checked: false,
        spec: "SCXML 1.0 §4.10 Extensibility of Executable Content", url: "https://www.w3.org/TR/scxml/#extensibility",
    ),
    doc!(
        "PKG002", "error", "package", "a package validator failed or returned diagnostics outside its contract",
        why: "an extension or service package may ship a validator component (manifest `validator-artifact` and `validator-sha256`) that the validator calls once per document with the package's elements; its output is untrusted, so a trap (fuel exhaustion included), an error, a code that is not `[a-z][a-z0-9_]*`, a span outside the source or off a character boundary, overlapping edits within one fix, or an edit outside every element of the package discards all of that package's diagnostics for this one error naming the package",
        help: "fix or update the package's validator component, or bind a package release whose validator works",
        bad: "a package validator that traps on the document",
        fixed: "a package validator that returns its diagnostics within its contract",
        checked: false,
        spec: "SCXML 1.0 §4.10 Extensibility of Executable Content", url: "https://www.w3.org/TR/scxml/#extensibility",
    ),
    doc!(
        "SCXML001", "error", "semantic", "the root element is not `<scxml>` in the SCXML namespace",
        why: "SCXML processors identify documents by the exact expanded name {http://www.w3.org/2005/07/scxml}scxml; a missing, misspelled, or slash-terminated namespace is a different vocabulary",
        help: "use `<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\">` as the document element",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml/\" version=\"1.0\"><final id=\"done\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"done\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.2 <scxml>", url: "https://www.w3.org/TR/scxml/#scxml",
    ),
    doc!(
        "SCXML002", "error", "semantic", "an identifier is declared more than once",
        why: "transition targets, `initial` references, and history defaults resolve by ID; duplicates make every reference ambiguous",
        help: "rename one declaration to a unique identifier and update every reference to it",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><transition event=\"go\" target=\"b\"/></state><final id=\"a\"/><final id=\"b\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><transition event=\"go\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.14 IDs", url: "https://www.w3.org/TR/scxml/#IDs",
    ),
    doc!(
        "SCXML003", "error", "semantic", "an identifier is not an XML NCName",
        why: "SCXML IDs are XML IDs: they must start with a letter or `_` and contain only NCName characters (no spaces or colons), so references can be tokenized unambiguously. An `id` is `xsd:ID`, whose white space collapses (XML Schema 1.1 Part 2 §4.3.6), so leading and trailing white space is not part of it: `id=\" s \"` declares `s`",
        help: "rename the identifier to a valid NCName, for example replacing spaces and colons with `_`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"1done\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"done1\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.14 IDs", url: "https://www.w3.org/TR/scxml/#IDs",
    ),
    doc!(
        "SCXML004", "error", "semantic", "a transition has no event, condition, or target",
        why: "SCXML 1.0 §3.5.2: \"A conformant SCXML document MUST specify at least one of 'event', 'cond' or 'target'.\" Executable content alone does not count: such a transition is enabled in every microstep and never changes the configuration, so the macrostep would never complete",
        help: "add an `event` trigger, a `cond` guard, or a `target`, or delete the transition",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><transition/><transition event=\"go\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><transition event=\"go\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.5 <transition>", url: "https://www.w3.org/TR/scxml/#transition",
    ),
    doc!(
        "SCXML005", "error", "semantic", "a transition target does not name a state in this document",
        why: "a transition's `target` must be one or more IDs of states in the same SCXML document; an unresolved target makes the document non-conformant",
        help: "correct the identifier or declare the missing state",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><transition event=\"go\" target=\"don\"/></state><final id=\"done\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><transition event=\"go\" target=\"done\"/></state><final id=\"done\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.5 <transition>", url: "https://www.w3.org/TR/scxml/#transition",
    ),
    doc!(
        "SCXML006", "error", "semantic", "an `initial` attribute names a state that does not exist",
        why: "the `initial` attribute of `<scxml>` or `<state>` selects the states entered first; an unknown ID, or an empty list, leaves the initial configuration undefined",
        help: "correct the identifier or declare the missing state; omit an empty `initial` to enter the first child state",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"strat\"><final id=\"start\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"start\"><final id=\"start\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.2 <scxml>", url: "https://www.w3.org/TR/scxml/#scxml",
    ),
    doc!(
        "SCXML007", "error", "semantic", "a `<param>` lacks `name` or has zero or two value sources",
        why: "a parameter needs a name and exactly one value source: `expr` evaluates a value, `location` reads a data-model location",
        help: "give `<param>` a `name` and exactly one of `expr` or `location`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><final id=\"f\"><donedata><param name=\"p\" expr=\"1\" location=\"x\"/></donedata></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><final id=\"f\"><donedata><param name=\"p\" expr=\"1\"/></donedata></final></scxml>",
        checked: true,
        spec: "SCXML 1.0 §5.7 <param>", url: "https://www.w3.org/TR/scxml/#param",
    ),
    doc!(
        "SCXML008", "error", "semantic", "a `<cancel>` has no send ID or both `sendid` and `sendidexpr`",
        why: "`<cancel>` identifies exactly one delayed `<send>` to cancel, either literally (`sendid`) or computed (`sendidexpr`)",
        help: "keep exactly one of `sendid` or `sendidexpr`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><final id=\"a\"><onentry><cancel/></onentry></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><final id=\"a\"><onentry><cancel sendidexpr=\"'timer'\"/></onentry></final></scxml>",
        checked: true,
        spec: "SCXML 1.0 §6.3 <cancel>", url: "https://www.w3.org/TR/scxml/#cancel",
    ),
    doc!(
        "SCXML009", "error", "semantic", "an `<initial>` does not contain exactly one `<transition>`",
        why: "`<initial>` describes one default entry; its single transition names the default child states",
        help: "put exactly one target-only `<transition>` inside `<initial>`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><state id=\"p\"><initial/><final id=\"c\"/></state></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><state id=\"p\"><initial><transition target=\"c\"/></initial><final id=\"c\"/></state></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.6 <initial>", url: "https://www.w3.org/TR/scxml/#initial",
    ),
    doc!(
        "SCXML010", "error", "semantic", "an `<initial>` transition has an event, a condition, or no target",
        why: "default entry is unconditional: SCXML 3.6.2 says the initial transition \"MUST NOT contain 'cond' or 'event' attributes\" and must name a target",
        help: "remove `event`/`cond` from the initial transition and give it a `target`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><state id=\"p\"><initial><transition event=\"go\" target=\"c\"/></initial><final id=\"c\"/></state></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><state id=\"p\"><initial><transition target=\"c\"/></initial><final id=\"c\"/></state></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.6 <initial>", url: "https://www.w3.org/TR/scxml/#initial",
    ),
    doc!(
        "SCXML011", "error", "semantic", "a state uses both an `initial` attribute and an `<initial>` child",
        why: "a compound state has exactly one default entry description; two forms conflict",
        help: "keep either the `initial` attribute or the `<initial>` child",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><state id=\"p\" initial=\"c\"><initial><transition target=\"c\"/></initial><final id=\"c\"/></state></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><state id=\"p\" initial=\"c\"><final id=\"c\"/></state></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.3 <state>", url: "https://www.w3.org/TR/scxml/#state",
    ),
    doc!(
        "SCXML012", "error", "semantic", "an atomic state declares an initial configuration",
        why: "`initial` selects child states; a state without child states has nothing to initialize",
        help: "remove the initial configuration, or add the child states it refers to",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><state id=\"a\" initial=\"a\"><transition event=\"done\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><transition event=\"done\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.3 <state>", url: "https://www.w3.org/TR/scxml/#state",
    ),
    doc!(
        "SCXML013", "error", "semantic", "a transition `event` is not a valid event-descriptor list",
        why: "`event` is a whitespace-separated list of descriptors (`a`, `a.b`, `a.*`, `*`); commas and other punctuation never match",
        help: "separate descriptors with spaces and use only name characters, `.`, and the `*` wildcard",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><transition event=\"go,stop\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><transition event=\"go stop\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.12.1 Event Descriptors", url: "https://www.w3.org/TR/scxml/#EventDescriptors",
    ),
    doc!(
        "SCXML014", "error", "semantic", "a `<send>` uses both forms of one property (a literal attribute and its `…expr` or `idlocation` alternative)",
        why: "each `<send>` property (event, target, type, delay, and the send ID as `id` or `idlocation`) has one literal and one computed form; supplying both is ambiguous",
        help: "keep exactly one of each literal/`…expr` pair, and one of `id` or `idlocation`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><final id=\"a\"><onentry><send event=\"e\" eventexpr=\"'e'\"/></onentry></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><final id=\"a\"><onentry><send event=\"e\"/></onentry></final></scxml>",
        checked: true,
        spec: "SCXML 1.0 §6.2 <send>", url: "https://www.w3.org/TR/scxml/#send",
    ),
    doc!(
        "SCXML015", "error", "semantic", "a `<send>` combines `<content>` with `namelist` or `<param>`",
        why: "`<content>` supplies the complete payload; `namelist` and `<param>` build a key/value payload, so the two forms cannot be combined",
        help: "use `<content>`, or `namelist`/`<param>`, not both",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><datamodel><data id=\"x\"/></datamodel><final id=\"a\"><onentry><send event=\"e\" namelist=\"x\"><content>1</content></send></onentry></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><datamodel><data id=\"x\"/></datamodel><final id=\"a\"><onentry><send event=\"e\"><content>1</content></send></onentry></final></scxml>",
        checked: true,
        spec: "SCXML 1.0 §6.2 <send>", url: "https://www.w3.org/TR/scxml/#send",
    ),
    doc!(
        "SCXML016", "error", "semantic", "an `<invoke>` has conflicting source or type forms",
        why: "`src`/`srcexpr`, `type`/`typeexpr`, `id`/`idlocation`, and an external source versus inline `<content>` are each mutually exclusive ways to say what to invoke and how to identify it",
        help: "keep one source form and one type form",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><state id=\"a\"><invoke src=\"child.scxml\" srcexpr=\"'child.scxml'\"/></state></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><state id=\"a\"><invoke src=\"child.scxml\"/></state></scxml>",
        checked: true,
        spec: "SCXML 1.0 §6.4 <invoke>", url: "https://www.w3.org/TR/scxml/#invoke",
    ),
    doc!(
        "SCXML017", "error", "semantic", "an `initial` attribute targets a state outside its element",
        why: "a state's `initial` attribute may only name its own descendants; the root's may only name states in the document",
        help: "target a descendant of the element that declares `initial`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"p\"><state id=\"p\" initial=\"q\"><state id=\"c\"><transition event=\"next\" target=\"q\"/></state></state><final id=\"q\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"p\"><state id=\"p\" initial=\"c\"><state id=\"c\"><transition event=\"next\" target=\"q\"/></state></state><final id=\"q\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.3 <state>", url: "https://www.w3.org/TR/scxml/#state",
    ),
    doc!(
        "SCXML018", "error", "semantic", "a `<donedata>` contains both `<content>` and `<param>`",
        why: "done data is either one `<content>` value or a set of `<param>` name/value pairs",
        help: "keep either the `<content>` child or the `<param>` children",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><final id=\"f\"><donedata><content>1</content><param name=\"p\" expr=\"1\"/></donedata></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><final id=\"f\"><donedata><param name=\"p\" expr=\"1\"/></donedata></final></scxml>",
        checked: true,
        spec: "SCXML 1.0 §5.5 <donedata>", url: "https://www.w3.org/TR/scxml/#donedata",
    ),
    doc!(
        "SCXML019", "error", "semantic", "a `<history>` does not contain exactly one `<transition>`",
        why: "the history transition defines the default configuration used before the parent state was ever exited",
        help: "put exactly one target-only `<transition>` inside `<history>`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><state id=\"p\"><history id=\"h\"/><final id=\"c\"/></state></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><state id=\"p\"><history id=\"h\"><transition target=\"c\"/></history><final id=\"c\"/></state></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.10 <history>", url: "https://www.w3.org/TR/scxml/#history",
    ),
    doc!(
        "SCXML020", "error", "semantic", "a `<history>` transition has an event, a condition, or no target",
        why: "the history default is unconditional: SCXML 3.10.2 says the history transition \"MUST NOT contain 'cond' or 'event' attributes\" and must name a target",
        help: "remove `event`/`cond` from the history transition and give it a `target`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><state id=\"p\"><history id=\"h\"><transition cond=\"true\" target=\"c\"/></history><final id=\"c\"/></state></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><state id=\"p\"><history id=\"h\"><transition target=\"c\"/></history><final id=\"c\"/></state></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.10 <history>", url: "https://www.w3.org/TR/scxml/#history",
    ),
    doc!(
        "SCXML021", "error", "semantic", "a history default targets a state outside what the history can restore",
        why: "shallow history records only the parent's immediate children, so its default must be one of them; deep history records descendants, so its default must lie inside the history's parent",
        help: "for shallow history target an immediate child of the history's parent (or declare `type=\"deep\"`); for deep history target a descendant of the parent",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><state id=\"p\"><history id=\"h\"><transition target=\"g\"/></history><state id=\"c\"><final id=\"g\"/></state></state></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><state id=\"p\"><history id=\"h\" type=\"deep\"><transition target=\"g\"/></history><state id=\"c\"><final id=\"g\"/></state></state></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.10 <history>", url: "https://www.w3.org/TR/scxml/#history",
    ),
    doc!(
        "SCXML022", "error", "semantic", "an `<initial>` transition targets a state outside its parent",
        why: "default entry must stay inside the compound state being entered",
        help: "target a descendant of the state that contains `<initial>`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"p\"><state id=\"p\"><initial><transition target=\"q\"/></initial><state id=\"c\"><transition event=\"next\" target=\"q\"/></state></state><final id=\"q\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"p\"><state id=\"p\"><initial><transition target=\"c\"/></initial><state id=\"c\"><transition event=\"next\" target=\"q\"/></state></state><final id=\"q\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.6 <initial>", url: "https://www.w3.org/TR/scxml/#initial",
    ),
    doc!(
        "LINT001", "warning", "lint", "a compound state relies on document order for its initial child",
        why: "without `initial`, the first of several child states in document order is entered; reordering children silently changes behavior (a single child state is not reported)",
        help: "add an `initial` attribute (or an `<initial>` child) naming the intended first child",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><state id=\"p\"><state id=\"c\"><transition event=\"go\" target=\"d\"/></state><final id=\"d\"/></state></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"p\"><state id=\"p\" initial=\"c\"><state id=\"c\"><transition event=\"go\" target=\"d\"/></state><final id=\"d\"/></state></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.3 <state>", url: "https://www.w3.org/TR/scxml/#state",
    ),
    doc!(
        "LINT002", "warning", "lint", "a non-final atomic state has no exit",
        why: "a state with no transitions and no `<invoke>`, inside no `<state>` or `<parallel>` that has one and no `<parallel>` with a transition that can leave it, can never be left; the machine stalls there unless that is intentional",
        help: "add an outgoing transition, make it a `<final>` state, or document the intentional wait",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><state id=\"stuck\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"stuck\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.3 <state>", url: "https://www.w3.org/TR/scxml/#state",
    ),
    doc!(
        "LINT003", "warning", "lint", "every exit from a state is a conditional eventless transition",
        why: "if every condition is false the state waits forever for a data change that no event will trigger re-evaluation for",
        help: "add an unconditional eventless fallback or an event-triggered exit",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><state id=\"a\"><transition cond=\"false\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><state id=\"a\"><transition cond=\"false\" target=\"b\"/><transition target=\"b\"/></state><final id=\"b\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.13 Selecting and Executing Transitions", url: "https://www.w3.org/TR/scxml/#SelectingTransitions",
    ),
    doc!(
        "LINT004", "warning", "lint", "unconditional eventless transitions form a cycle",
        why: "eventless, conditionless transitions are taken immediately in the same macrostep; a cycle of them never reaches a stable configuration (a warning: SCXML 1.0 Appendix D says a macrostep that does not terminate \"is currently allowed\", so the document is conforming and compiles, and a host bounds the run)",
        help: "add an `event` or `cond` to one edge of the cycle, or remove one target",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><transition target=\"b\"/></state><state id=\"b\"><transition target=\"a\"/></state></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><transition target=\"b\"/></state><state id=\"b\"><transition event=\"again\" target=\"a\"/></state></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.13 Selecting and Executing Transitions", url: "https://www.w3.org/TR/scxml/#SelectingTransitions",
    ),
    doc!(
        "LIMIT001", "error", "semantic", "inline `<scxml>` documents nest deeper than the validation budget",
        why: "each `<invoke><content><scxml>` is validated as an independent machine; the nesting depth is bounded (64 roots) so hostile input cannot exhaust the validator. The bound applies to wasm builds only (the shipped validator and compiler components); the native `scx` has no such bound and never reports this code",
        help: "reduce inline nesting or invoke deeper machines by `src` as separately validated documents",
        bad: "<scxml …><state><invoke><content><scxml …> … (65 levels) …",
        fixed: "<invoke src=\"child.scxml\"/>",
        checked: false,
        spec: "SCXML 1.0 §6.4 <invoke>", url: "https://www.w3.org/TR/scxml/#invoke",
    ),
    doc!(
        "LIMIT002", "error", "semantic", "a machine is too large for bounded semantic graph analysis",
        why: "reachability, target legality, and liveness are computed within a work budget of 8,000,000 units per machine, or 1,024 per element and target reference when that is more, so work that grows faster than the machine (such as pairwise checks of thousands of transitions of one state) cannot exhaust the validator; when it is spent no partial graph findings are reported. The budget applies to every build. On wasm builds (the shipped validator and compiler components) the graph is also bounded in size (65,536 states and transitions, 131,072 target references) and the budget to 8,000,000 units",
        help: "split the machine into smaller machines composed with `<invoke>`, or reduce the number of states and transitions",
        bad: "<scxml …> … more than 65,536 states … </scxml>",
        fixed: "several smaller machines composed with <invoke>",
        checked: false,
        spec: "SCXML 1.0 §3.11 Legal State Configurations and Specifications", url: "https://www.w3.org/TR/scxml/#LegalStateConfigurations",
    ),
    doc!(
        "LINT005", "warning", "lint", "a state can never become active",
        why: "no transition, `initial`, or history default reachable from the initial configuration enters the state or any descendant, even assuming every event arrives and every `cond` can be true; unreachable states are usually a mistyped target or a forgotten transition",
        help: "target the state from a transition of a reachable state, make it the parent's initial state, or remove it",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><transition event=\"go\" target=\"a\"/></state><final id=\"b\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><transition event=\"go\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.11 Legal State Configurations and Specifications", url: "https://www.w3.org/TR/scxml/#LegalStateConfigurations",
    ),
    doc!(
        "SCXML030", "error", "semantic", "a multi-target set cannot be active at the same time",
        why: "the targets of a transition or an `initial` attribute must form a legal state configuration, and the compiler requires each pair to lie in different regions of a common `<parallel>`: targets in different children of a compound `<state>` would require two active children; a target and its own ancestor or descendant name one region twice (a state listed twice is only the `LINT018` warning)",
        help: "keep only one of the conflicting targets and, when nothing else enters the other state, give it its own transition so it does not become unreachable (`LINT005`); or move the targets into separate regions of a `<parallel>`. The suggested removal is offered only when it strands no state",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"s\"><state id=\"s\"><transition event=\"go\" target=\"a b\"/></state><final id=\"a\"/><final id=\"b\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"s\"><state id=\"s\"><transition event=\"go\" target=\"a\"/><transition event=\"alt\" target=\"b\"/></state><final id=\"a\"/><final id=\"b\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.11 Legal State Configurations and Specifications", url: "https://www.w3.org/TR/scxml/#LegalStateConfigurations",
    ),
    doc!(
        "SCXML031", "error", "semantic", "a `<final>` state contains something other than `<onentry>`, `<onexit>`, or `<donedata>`",
        why: "a `<final>` state is atomic and completes its parent when entered; transitions, child states, invokes, and an `initial` attribute have no meaning there",
        help: "change `<final>` to `<state>` if it must react or contain children, or move the element to a non-final state",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"f\"><transition event=\"e\" target=\"f\"/></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"f\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.7 <final>", url: "https://www.w3.org/TR/scxml/#final",
    ),
    doc!(
        "SCXML032", "error", "semantic", "`<donedata>` appears outside `<final>` or more than once",
        why: "`<donedata>` supplies the payload of the `done.state` event raised when a `<final>` state is entered; anywhere else nothing reads it, and a second one is ambiguous",
        help: "move the `<donedata>` into the `<final>` state whose completion should carry the data, keeping at most one",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"s\"><state id=\"s\"><donedata/><transition event=\"e\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"s\"><state id=\"s\"><transition event=\"e\" target=\"f\"/></state><final id=\"f\"><donedata/></final></scxml>",
        checked: true,
        spec: "SCXML 1.0 §5.5 <donedata>", url: "https://www.w3.org/TR/scxml/#donedata",
    ),
    doc!(
        "LINT006", "warning", "lint", "a `<parallel>` has a single region",
        why: "a parallel state with one child runs nothing in parallel; it is usually a `<state>` written as `<parallel>` or a parallel state missing its other regions (completion semantics differ between the two)",
        help: "use `<state>`, or add the missing orthogonal regions",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><parallel id=\"p\"><final id=\"only\"/></parallel></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><state id=\"p\"><final id=\"only\"/></state></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.4 <parallel>", url: "https://www.w3.org/TR/scxml/#parallel",
    ),
    doc!(
        "LINT007", "warning", "lint", "a transition is shadowed by an earlier unconditional transition in the same state",
        why: "within one state the interpreter selects the first enabled transition in document order; an earlier transition without `cond` whose descriptors match every event of a later one (or an earlier eventless one) always wins, so the later transition is dead",
        help: "move the specific transition above the general one, add a `cond` to the earlier transition, or remove the dead one",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"s\"><state id=\"s\"><transition event=\"error\" target=\"f\"/><transition event=\"error.execution\" target=\"g\"/></state><final id=\"f\"/><final id=\"g\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"s\"><state id=\"s\"><transition event=\"error.execution\" target=\"g\"/><transition event=\"error\" target=\"f\"/></state><final id=\"f\"/><final id=\"g\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.13 Selecting and Executing Transitions", url: "https://www.w3.org/TR/scxml/#SelectingTransitions",
    ),
    doc!(
        "LINT008", "warning", "lint", "eventless transitions loop forever through default entry",
        why: "each step is the first eventless transition without `cond` on the active state's ancestor chain, and default entry through `initial` leads back to the start, so the interpreter never reaches a stable configuration (a warning: SCXML 1.0 Appendix D says a macrostep that does not terminate \"is currently allowed\", so the document is conforming and compiles, and a host bounds the run)",
        help: "add an `event` or `cond` to one transition in the loop, or retarget one so the macrostep can settle",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><transition target=\"b\"/></state><state id=\"b\" initial=\"b1\"><state id=\"b1\"><transition target=\"a\"/></state></state></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><transition target=\"b\"/></state><state id=\"b\" initial=\"b1\"><state id=\"b1\"><transition event=\"again\" target=\"a\"/></state></state></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.13 Selecting and Executing Transitions", url: "https://www.w3.org/TR/scxml/#SelectingTransitions",
    ),
    doc!(
        "SCXML037", "error", "semantic", "the null data model is used with an element it does not support",
        why: "SCXML 1.0 §B.1.7: \"The <foreach> element and the elements defined in 5 Data Model and Data Manipulation are not supported in the Null Data Model.\" A conformant document \"MUST conform to any additional syntactic constraints that are defined for the data model that it has chosen\" (§A.1), and no run-time behavior is defined for these elements. Expressions under the null data model are the `SCXML079` warning",
        help: "remove the section 5 elements (`<datamodel>`, `<data>`, `<assign>`, `<donedata>`, `<content>`, `<param>`, `<script>`) and `<foreach>`, or select a data model such as `ecmascript`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"null\" initial=\"s\"><state id=\"s\"><onentry><script>1</script></onentry><transition event=\"go\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"null\" initial=\"s\"><state id=\"s\"><transition event=\"go\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §B.1 The Null Data Model", url: "https://www.w3.org/TR/scxml/#minimal-profile",
    ),
    doc!(
        "SCXML038", "warning", "semantic", "an expression or script does not parse in the selected data model",
        why: "SCXML 1.0 §5.9.4: \"The SCXML Processor MAY reject documents containing syntactically ill-formed expressions at document load time, or it MAY wait and place 'error.execution' in the internal event queue at runtime when the expressions are evaluated.\" This toolchain takes the second option, so the document compiles and the expression raises `error.execution` whenever it is evaluated (a condition is then false and the rest of an executable-content block is skipped); this check runs only when the host supplies the data model's parse-only syntax capability (the native CLI links the ECMAScript and XPath checkers). Each finding names the offending source text and, where a repair is certain to parse, offers it: ECMAScript habits in XPath (`==` to `=`, `&&` to `and`, `||` to `or`, `!x` to `not(x)`, `length()` to `string-length()`), a stray `=` in ECMAScript (`x === = 3`), word operators, quoting, and more. The XPath data model has no script language, so a non-empty `<script>` is reported too",
        help: "apply the suggested repair, or fix the syntax at the marked position following the finding's help; for XPath, use XPath 1.0 operators and functions (`=`, `and`, `or`, `not()`, `string-length()`, `count()`)",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"s\"><datamodel><data id=\"x\" expr=\"0\"/></datamodel><state id=\"s\"><transition event=\"go\" cond=\"x ===\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"s\"><datamodel><data id=\"x\" expr=\"0\"/></datamodel><state id=\"s\"><transition event=\"go\" cond=\"x === 1\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §B.2 The ECMAScript Data Model", url: "https://www.w3.org/TR/scxml/#ecma-profile",
    ),
    doc!(
        "SCXML039", "warning", "semantic", "a location is not assignable in the selected data model",
        why: "`location`, `idlocation`, `namelist` items, and `<foreach item/index>` must name an assignable place; SCXML 1.0 defines the run-time outcome of one that does not, so the document compiles: §5.9.2 \"If a location expression cannot be evaluated to yield a valid location, the SCXML processor MUST place the error 'error.execution' in the internal event queue\", and §4.6 \"if 'item' does not specify a legal variable name, the SCXML processor MUST terminate execution of the <foreach> element and the block that contains it, and place the error error.execution on the internal event queue\". In ECMAScript an assignable place is a variable or a property access such as `data.field` or `data[0]`; in XPath it is an expression that selects existing nodes, such as `$data` or `$data/field` (`a.b` is one variable name containing a dot, and `a[0]` is a predicate)",
        help: "ECMAScript: use a variable name or a member expression such as `data.field`; XPath: use a `$`-rooted path to a declared variable such as `$data/field`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><datamodel><data id=\"d\"/></datamodel><final id=\"f\"><onentry><assign location=\"d + 1\" expr=\"2\"/></onentry></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><datamodel><data id=\"d\"/></datamodel><final id=\"f\"><onentry><assign location=\"d\" expr=\"2\"/></onentry></final></scxml>",
        checked: true,
        spec: "SCXML 1.0 §5.9.2 Location Expressions", url: "https://www.w3.org/TR/scxml/#LocationExpressions",
    ),
    doc!(
        "LIMIT003", "error", "semantic", "an expression exceeds the data model's size or nesting bound",
        why: "the data-model syntax capability declines fragments beyond its size or nesting bound (the ECMAScript provider: 16 KiB expressions, 64 KiB scripts, at most 128 structural nesting units, where each open bracket costs 8, so 16 brackets, 15 inside a value expression) instead of risking its parser stack; the data model applies the same bound when it evaluates the fragment, so the fragment would be rejected at run time and `compile` refuses the document. The size bounds apply to wasm builds only (the shipped validator and data-model components; the native `scx` checks fragments of any size); the nesting bound guards the parser's native stack and applies to every build",
        help: "split the expression or script into smaller parts, assigning intermediate results to variables (`<data>` or `<assign>`)",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><final id=\"f\"><onentry><log expr=\"((((((((((((((((1))))))))))))))))\"/></onentry></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><datamodel><data id=\"inner\" expr=\"((((((((1))))))))\"/></datamodel><final id=\"f\"><onentry><log expr=\"((((((((inner))))))))\"/></onentry></final></scxml>",
        checked: true,
        spec: "SCXML 1.0 §5.9 Expressions", url: "https://www.w3.org/TR/scxml/#expressions",
    ),
    doc!(
        "LIMIT004", "error", "semantic", "a document has too many expressions for bounded syntax checking",
        why: "expression checking is bounded to 16,384 fragments and 4 MiB of expression text per document; beyond that the remaining expressions are unchecked, so the validator fails closed. The bound applies to wasm builds only (the shipped validator and compiler components); the native `scx` has no such bound and never reports this code",
        help: "split the document into smaller machines",
        bad: "<scxml …> … more than 16,384 expressions … </scxml>",
        fixed: "several smaller documents",
        checked: false,
        spec: "SCXML 1.0 §5.9 Expressions", url: "https://www.w3.org/TR/scxml/#expressions",
    ),
    doc!(
        "LINT009", "warning", "lint", "an expression attribute is present but empty",
        why: "a blank `<log expr>` is treated as absent (the log has no value), while every other empty expression attribute, including `<transition cond>`, is still evaluated as an invalid expression and raises `error.execution` (a condition is then false); either way the empty value is almost always a mistake",
        help: "write the intended expression, or remove the attribute",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><final id=\"f\"><onentry><log label=\"done\" expr=\"\"/></onentry></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><final id=\"f\"><onentry><log label=\"done\"/></onentry></final></scxml>",
        checked: true,
        spec: "SCXML 1.0 §5.9 Expressions", url: "https://www.w3.org/TR/scxml/#expressions",
    ),
    doc!(
        "LINT010", "note", "lint", "a machine's expressions were not syntax-checked",
        why: "the validator checks expression syntax only with a checker linked for the machine's data model (its `datamodel`, or the default `ecmascript` when it declares none: SCXML 1.0 §3.2.1 leaves the default \"platform-specific\"); a run that links no checker for it (for example the Wasm validator component composed with only one data model's checker) says so instead of staying silent",
        help: "review the expressions manually, or validate with a host that links a syntax checker for this data model",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"xpath\"><final id=\"f\"><onentry><log expr=\"1\"/></onentry></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"xpath\"><final id=\"f\"><onentry><log expr=\"1\"/></onentry></final></scxml>",
        checked: false,
        spec: "SCXML 1.0 §5.1 Data Model", url: "https://www.w3.org/TR/scxml/#data-module",
    ),
    doc!(
        "SCXML050", "error", "semantic", "a `<script>` has both `src` and inline content",
        why: "SCXML 1.0 §5.8 gives `<script>` either a `src` URI or inline script content, never both; an implementation cannot know which one the author meant to run",
        help: "keep the external `src` and delete the inline content, or delete `src` and keep the inline script",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><final id=\"a\"><onentry><script src=\"lib.wasm\">x = 1;</script></onentry></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><final id=\"a\"><onentry><script src=\"lib.wasm\"/></onentry></final></scxml>",
        checked: true,
        spec: "SCXML 1.0 §5.8 <script>", url: "https://www.w3.org/TR/scxml/#script",
    ),
    doc!(
        "SCXML051", "error", "semantic", "a `<script src>` does not name a fetchable script",
        why: "an empty, whitespace-only, control-character, or over-long `src` can never be fetched, and SCXML 1.0 §5.8 rejects a document whose script cannot be downloaded at load",
        help: "set `src` to the URI of a Wasm script component or a script in the data model's language, or remove `src` and inline the script",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><final id=\"a\"><onentry><script src=\"\"/></onentry></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><final id=\"a\"><onentry><script src=\"lib.wasm\"/></onentry></final></scxml>",
        checked: true,
        spec: "SCXML 1.0 §5.8 <script>", url: "https://www.w3.org/TR/scxml/#script",
    ),
    doc!(
        "SCXML052", "error", "semantic", "a `<send>` to the SCXML event processor names no event",
        why: "the SCXML Event I/O Processor (the default `<send>` type) delivers a named event, so SCXML 1.0 §6.2 requires `event` or `eventexpr` for it; without one the send raises `error.execution` and delivers nothing",
        help: "add `event=\"name\"` (or `eventexpr`) naming the event to deliver; a different `type` has its own requirements",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"a\"><onentry><send target=\"#_internal\"/></onentry></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"a\"><onentry><send event=\"ping\" target=\"#_internal\"/></onentry></final></scxml>",
        checked: true,
        spec: "SCXML 1.0 §6.2 <send>", url: "https://www.w3.org/TR/scxml/#send",
    ),
    doc!(
        "SCXML053", "error", "semantic", "a `<data>`, `<content>`, or `<assign>` has more than one value source",
        why: "`<data>` takes its value from exactly one of `src`, `expr`, or child content (SCXML 1.0 §5.3), `<content>` is either computed by `expr` or given as child content (§5.6), and `<assign>` takes its value from `expr` or child content (§5.4); with several sources the value is ambiguous",
        help: "keep one value source and remove the others",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><datamodel><data id=\"d\" expr=\"1\" src=\"d.json\"/></datamodel><final id=\"a\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><datamodel><data id=\"d\" expr=\"1\"/></datamodel><final id=\"a\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §5.3 <data>", url: "https://www.w3.org/TR/scxml/#data",
    ),
    doc!(
        "SCXML055", "error", "semantic", "a `<finalize>` raises or sends an event",
        why: "`<finalize>` runs while an event returned by an invoked service is processed, before transitions are selected, so the data model can be updated from `_event`; SCXML 1.0 §6.5 says its executable content MUST NOT raise events or invoke external actions, and that `<send>` and `<raise>` MUST NOT occur in it (at any depth, including `<if>` and `<foreach>` bodies)",
        help: "keep only data-model updates in `<finalize>`, and move the `<raise>` or `<send>` into a `<transition>` that matches the returned event",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><invoke type=\"http://www.w3.org/TR/scxml/\" src=\"child.scxml\"><finalize><raise event=\"x\"/></finalize></invoke><transition event=\"x\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><invoke type=\"http://www.w3.org/TR/scxml/\" src=\"child.scxml\"><finalize/></invoke><transition event=\"x\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §6.5 <finalize>", url: "https://www.w3.org/TR/scxml/#finalize",
    ),
    doc!(
        "SCXML056", "warning", "semantic", "an XPath expression references a variable the machine never declares",
        why: "in the XPath data model a variable exists only for a declared `<data id>` (and a `<foreach item/index>` or `idlocation` name once it runs) or a system variable (`_event`, `_sessionid`, `_name`, `_ioprocessors`); evaluating any other `$name` fails, and SCXML 1.0 defines that outcome (§5.9.3: \"If a value expression does not return a legal data value, the SCXML Processor MUST place the error 'error.execution' in the internal event queue\"; a condition is then false, §5.9.1), so the document compiles. `.` is part of an XPath name, so `$user.name` is one undeclared variable, not a member access",
        help: "use a declared variable (the finding suggests the nearest one), select a child with a path such as `$user/name`, or declare the variable with `<data id=\"name\"/>`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"xpath\"><datamodel><data id=\"count\" expr=\"0\"/></datamodel><final id=\"f\"><onentry><log expr=\"$cuont\"/></onentry></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"xpath\"><datamodel><data id=\"count\" expr=\"0\"/></datamodel><final id=\"f\"><onentry><log expr=\"$count\"/></onentry></final></scxml>",
        checked: true,
        spec: "SCXML 1.0 §5.9.3 Legal Data Values and Value Expressions", url: "https://www.w3.org/TR/scxml/#ValueExpressions",
    ),
    doc!(
        "LINT011", "warning", "lint", "an XPath expression starts with a relative path that selects no data",
        why: "the context node of every XPath expression is `<datamodel>`, whose children are the `<data id=\"…\">` elements and the system variables, so a bare name such as `count`, `true`, or `_event.data.ok` is a path to a child element with that exact name and selects nothing: a location that selects nothing raises `error.execution`, and a condition is false. In a machine with a `<foreach>`, whose item binds an element of any name, only a declared variable name, `true`/`false`, and a dotted member access are reported",
        help: "reference the variable as `$count`, call `true()`/`false()`, or select a child with a path such as `$_event/data/ok`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"xpath\"><datamodel><data id=\"count\" expr=\"0\"/></datamodel><final id=\"f\"><onentry><assign location=\"count\" expr=\"1\"/></onentry></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"xpath\"><datamodel><data id=\"count\" expr=\"0\"/></datamodel><final id=\"f\"><onentry><assign location=\"$count\" expr=\"1\"/></onentry></final></scxml>",
        checked: true,
        spec: "SCXML CR 2014 §C.3.1 Data Model", url: "https://www.w3.org/TR/2014/CR-scxml-20140313/#xpath-datamodel",
    ),
    doc!(
        "LINT012", "warning", "lint", "an `In()` condition names no state of the machine or passes the state ID unquoted",
        why: "`In(stateID)` is true only while a `<state>`, `<parallel>`, or `<final>` with that ID is active, so a literal ID that names no state (usually a typo) makes the condition always false; in the ECMAScript and XPath data models an unquoted ID is a variable or a relative path, not the ID",
        help: "name an existing state ID (the finding suggests the nearest one), and quote it as a string: `In('s1')`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><state id=\"a\"><transition event=\"go\" cond=\"In('bb')\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><state id=\"a\"><transition event=\"go\" cond=\"In('b')\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §5.9.1 Conditional Expressions", url: "https://www.w3.org/TR/scxml/#ConditionalExpressions",
    ),
    doc!(
        "LINT013", "warning", "lint", "an XPath predicate selects by a position that no node has",
        why: "a numeric predicate `[n]` keeps only the node at position `n`, and XPath counts positions from 1, so `[0]` (the ECMAScript habit of indexing from 0), a negative number, or a fraction selects nothing: a location that selects nothing raises `error.execution`, a condition is false, and a value is empty",
        help: "select the first node with `[1]` and the last with `[last()]`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"xpath\"><datamodel><data id=\"list\"><item>1</item></data></datamodel><final id=\"f\"><onentry><assign location=\"$list/item[0]\" expr=\"2\"/></onentry></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"xpath\"><datamodel><data id=\"list\"><item>1</item></data></datamodel><final id=\"f\"><onentry><assign location=\"$list/item[1]\" expr=\"2\"/></onentry></final></scxml>",
        checked: true,
        spec: "XPath 1.0 §2.4 Predicates", url: "https://www.w3.org/TR/1999/REC-xpath-19991116/#predicates",
    ),
    doc!(
        "LINT014", "warning", "lint", "an XPath expression adds a string with `+`",
        why: "XPath converts both operands of `+` to numbers, so a string that is not numeric becomes `NaN` and the sum is `NaN`; `+` does not concatenate strings as it does in ECMAScript",
        help: "join strings with `concat()`, for example `concat('Hello, ', $name)`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"xpath\"><datamodel><data id=\"name\" expr=\"'Ada'\"/></datamodel><final id=\"f\"><onentry><log expr=\"'Hello, ' + $name\"/></onentry></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"xpath\"><datamodel><data id=\"name\" expr=\"'Ada'\"/></datamodel><final id=\"f\"><onentry><log expr=\"concat('Hello, ', $name)\"/></onentry></final></scxml>",
        checked: true,
        spec: "XPath 1.0 §3.5 Numbers", url: "https://www.w3.org/TR/1999/REC-xpath-19991116/#numbers",
    ),
    doc!(
        "LINT015", "warning", "lint", "an ECMAScript `<data id>` is not an identifier, so no expression can name it",
        why: "SCXML 1.0 §B.2.1: \"the SCXML Processor MUST create an ECMAScript variable object whose name is the value of the 'id' attribute of <data>\", and an XML ID such as `retry-count`, `max.tries`, or `class` is conformant (§5.10 reserves only a leading `_`, `SCXML073`), so the data model creates the variable as a property of the global object; but it is reachable only as `globalThis['retry-count']`, and a reference such as `retry-count < 3` parses as something else (it subtracts `count` from `retry`)",
        help: "to use the variable in expressions, rename it to an identifier such as `retryCount` in its declaration and every reference (the finding offers the rename)",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><datamodel><data id=\"retry-count\" expr=\"0\"/></datamodel><final id=\"f\"><onentry><log expr=\"retry-count\"/></onentry></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><datamodel><data id=\"retryCount\" expr=\"0\"/></datamodel><final id=\"f\"><onentry><log expr=\"retryCount\"/></onentry></final></scxml>",
        checked: true,
        spec: "SCXML 1.0 §B.2 The ECMAScript Data Model", url: "https://www.w3.org/TR/scxml/#ecma-profile",
    ),
    doc!(
        "LINT016", "warning", "lint", "a Unicode bidirectional control character changes how the source is displayed",
        why: "an explicit embedding, override, or isolate (U+202A-U+202E, U+2066-U+2069) reorders how the text around it is displayed, so an editor or review can show a condition, ID, or script differently from what the machine executes (Trojan Source, CVE-2021-42574); rustc rejects the same characters in literals",
        help: "write an intended control as a character reference such as `&#x202E;`, which XML decodes to the same character in an attribute value or text and keeps visible (the finding offers this fix), or remove it",
        bad: "<log label=\"…\"/> whose label contains a raw U+202E RIGHT-TO-LEFT OVERRIDE",
        fixed: "<log label=\"&#x202E;…\"/> (the same label with the control written as a character reference)",
        checked: false,
        spec: "Unicode Standard Annex #9 §2 Directional Formatting Characters", url: "https://www.unicode.org/reports/tr9/#Directional_Formatting_Characters",
    ),
    doc!(
        "LINT017", "warning", "lint", "an ECMAScript condition is an arrow function, so it is always true",
        why: "`cond=\"count => 3\"` parses as a function whose parameter is `count`; a condition is converted to a boolean without calling it, and every function is truthy, so the transition is always enabled; `>=` was almost always intended",
        help: "compare with `>=` (the finding offers the fix when the parameter is a single name), or call the function to compute a boolean",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><datamodel><data id=\"count\" expr=\"0\"/></datamodel><state id=\"a\"><transition event=\"go\" cond=\"count => 3\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><datamodel><data id=\"count\" expr=\"0\"/></datamodel><state id=\"a\"><transition event=\"go\" cond=\"count &gt;= 3\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §5.9.1 Conditional Expressions", url: "https://www.w3.org/TR/scxml/#ConditionalExpressions",
    ),
    doc!(
        "SCXML064", "error", "semantic", "a `<send>`, `<invoke>`, or `<state>` repeats a child it allows once",
        why: "`<send>` and `<invoke>` carry at most one `<content>` (the payload or the service's document), `<invoke>` at most one `<finalize>`, and `<state>` at most one `<initial>` (SCXML 1.0 §6.2, §6.4, §3.3); the schema allows the repeat, but a second one has no meaning and the compiler rejects it",
        help: "keep one of the repeated children: merge a second `<finalize>` into the first, send several values as `<param>` children, or list several initial targets in one `<initial>` transition",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><state id=\"a\"><onentry><send event=\"e\"><content>one</content><content>two</content></send></onentry><transition event=\"e\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><state id=\"a\"><onentry><send event=\"e\"><content>one</content></send></onentry><transition event=\"e\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §6.2 <send>", url: "https://www.w3.org/TR/scxml/#send",
    ),
    doc!(
        "SCXML065", "error", "semantic", "an `<assign>` has no value, or its value does not fit its `type`",
        why: "`<assign>` stores the value of `expr` or of its child content at `location` (SCXML 1.0 §5.4); the XPath placements (SCXML CR 2014 §C.3.6) add the attribute named by `attr` only for `type=\"addattribute\"`, and a `type=\"delete\"` ignores its value (\"('expr' is ignored.)\"); the compiler rejects an `<assign>` with nothing to store other than a deletion, and `attr` without `addattribute` or the reverse",
        help: "give the `<assign>` an `expr` (or child content); use `attr` exactly with `type=\"addattribute\"`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><datamodel><data id=\"x\" expr=\"0\"/></datamodel><state id=\"a\"><onentry><assign location=\"x\"/></onentry><transition event=\"e\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><datamodel><data id=\"x\" expr=\"0\"/></datamodel><state id=\"a\"><onentry><assign location=\"x\" expr=\"1\"/></onentry><transition event=\"e\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §5.4 <assign>", url: "https://www.w3.org/TR/scxml/#assign",
    ),
    doc!(
        "SCXML066", "warning", "semantic", "a required expression attribute is empty",
        why: "`<if>`/`<elseif>` `cond`, `<foreach>` `array` and `item`, and `<assign>` `location` are required; an empty value is no expression at all, so it raises `error.execution` every time it runs (SCXML 1.0 §5.9.4: the processor \"MAY wait and place 'error.execution' in the internal event queue at runtime when the expressions are evaluated\"; §5.9.1: a condition is then false), and the document still compiles",
        help: "write the condition, collection, loop variable, or location; a last `<if>` branch that always runs is written as `<else/>`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><state id=\"a\"><onentry><if cond=\"\"><log expr=\"1\"/></if></onentry><transition event=\"e\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><state id=\"a\"><onentry><if cond=\"true\"><log expr=\"1\"/></if></onentry><transition event=\"e\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §4.3 <if>", url: "https://www.w3.org/TR/scxml/#if",
    ),
    doc!(
        "SCXML067", "error", "semantic", "an `<scxml>` has no child states",
        why: "SCXML 1.0 §3.2.2: \"A conformant SCXML document MUST have at least one <state>, <parallel> or <final> child\"; a machine starts by entering its initial state, so with none there is nothing to enter",
        help: "add the machine's states, for example `<final id=\"done\"/>`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><datamodel/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><datamodel/><final id=\"done\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.2 <scxml>", url: "https://www.w3.org/TR/scxml/#scxml",
    ),
    doc!(
        "LIMIT005", "error", "semantic", "the document exceeds a Machine IR bound",
        why: "Machine IR has fixed bounds so lowering and execution stay bounded: 65 nested states, 64 levels of `<if>`/`<foreach>` nesting, 32 levels of inline XML, 10,000 child nodes, attributes, targets, event descriptors, names, branches, or data declarations per element or scope, 100,000 targets across a machine's multi-target sets, and 1 MiB per string; they are far above any hand-written machine, and `compile` rejects a document over them. These bounds apply to wasm builds only (the shipped validator and compiler components); the native `scx` has no such bound and never reports this code",
        help: "flatten the structure, split large lists or scopes, move a deep or large part into a machine it invokes, or load large data with `src`",
        bad: "a `<state>` nested 66 states deep (65 is the most the compiler lowers)",
        fixed: "the same behavior with the deep part in a child machine the state `<invoke>`s",
        checked: false,
        spec: "", url: "",
    ),
    doc!(
        "SCXML070", "error", "semantic", "a `<send>` to `#_internal` has a delay",
        why: "SCXML 1.0 §6.2 says `delay` and `delayexpr` must not occur when the `target` is the internal event queue: an internal event joins the queue at once, so a delayed internal send is non-conformant, and the processor raises `error.execution` for it instead of delivering the event",
        help: "remove the delay to raise the event now, or drop `target` to deliver the event after the delay on the session's own external queue",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><onentry><send event=\"tick\" target=\"#_internal\" delay=\"1s\"/></onentry><transition event=\"tick\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><onentry><send event=\"tick\" delay=\"1s\"/></onentry><transition event=\"tick\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §6.2 <send>", url: "https://www.w3.org/TR/scxml/#send",
    ),
    doc!(
        "SCXML071", "error", "semantic", "an `<invoke>` combines `namelist` with `<param>`",
        why: "SCXML 1.0 §6.4 says an `<invoke>`'s `namelist` \"Must not occur with the <param> element\": both pass data to the invoked service, and a conformant document uses one of them",
        help: "pass each namelist location as a `<param>` whose `name` and `location` are that location, and remove `namelist`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><datamodel><data id=\"x\" expr=\"1\"/></datamodel><state id=\"a\"><invoke namelist=\"x\"><param name=\"y\" expr=\"2\"/><content><scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"c\"/></scxml></content></invoke><transition event=\"done.invoke\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><datamodel><data id=\"x\" expr=\"1\"/></datamodel><state id=\"a\"><invoke><param name=\"x\" location=\"x\"/><param name=\"y\" expr=\"2\"/><content><scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"c\"/></scxml></content></invoke><transition event=\"done.invoke\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §6.4 <invoke>", url: "https://www.w3.org/TR/scxml/#invoke",
    ),
    doc!(
        "LINT018", "warning", "lint", "a target list names the same state twice",
        why: "SCXML 1.0 §3.11 defines a legal state specification as \"a set of states such that 1) no state is an ancestor of any other state on the list, and 2) a full legal state configuration results when all ancestors and default initial descendants have been added\": a target list names a set, so a repeated state is legal and is entered once, and the repeat adds nothing",
        help: "remove the repeated target",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"s\"><state id=\"s\"><transition event=\"go\" target=\"f f\"/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"s\"><state id=\"s\"><transition event=\"go\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.11 Legal State Configurations and Specifications", url: "https://www.w3.org/TR/scxml/#LegalStateConfigurations",
    ),
    doc!(
        "SCXML073", "error", "semantic", "a `<data id>` begins with `_`",
        why: "SCXML 1.0 §5.10: \"Variable names beginning with '_' are reserved for system use. A conformant SCXML document MUST NOT contain ids beginning with '_' in the <data> element.\" (under the null data model `<data>` itself is `SCXML037`)",
        help: "rename the data without the leading `_`, and update every reference to it",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"xpath\" initial=\"s\"><datamodel><data id=\"_count\"/></datamodel><state id=\"s\"><transition event=\"e\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"xpath\" initial=\"s\"><datamodel><data id=\"count\"/></datamodel><state id=\"s\"><transition event=\"e\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §5.10 System Variables", url: "https://www.w3.org/TR/scxml/#SystemVariables",
    ),
    doc!(
        "SCXML074", "error", "semantic", "a `<log expr>` has a side effect",
        why: "SCXML 1.0 §4.7.3: \"The SCXML processor MUST ensure that <log> has no side-effects on document interpretation.\" An ECMAScript `<log>` value is an ECMAScript expression (B.2), which can assign, so the validator rejects one that certainly changes state outside itself when it runs: an assignment, update, or `delete` of a variable or property outside the expression, a built-in method that changes its receiver or argument (`push`, `sort`, `Object.assign`, ...), `Math.random` (the session's random sequence advances), or a call of a document function whose body does one of those",
        help: "log a value without changing anything, and move the change into executable content (`<assign>` or `<script>`)",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><datamodel><data id=\"tries\" expr=\"0\"/></datamodel><state id=\"a\"><onentry><log label=\"try\" expr=\"++tries\"/></onentry><transition event=\"go\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><datamodel><data id=\"tries\" expr=\"0\"/></datamodel><state id=\"a\"><onentry><assign location=\"tries\" expr=\"tries + 1\"/><log label=\"try\" expr=\"tries\"/></onentry><transition event=\"go\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §4.7.3 <log>", url: "https://www.w3.org/TR/scxml/#log",
    ),
    doc!(
        "LINT019", "warning", "lint", "the validator cannot prove a condition or `<log expr>` has no side effect",
        why: "`<log>` values must have no side effects (SCXML 1.0 §4.7.3) and conditional expressions in conformant documents should have none (§5.9.1), and the validator decides that statically: it follows document functions declared in inline `<script>` elements or `<data expr>`, but it cannot see a function it has no source for, text run as code (`eval`, `Function`), a `with` scope, a `<script src>`, a getter or conversion method with a side effect, or a write through a reference it cannot trace",
        help: "declare the functions the expression calls in an inline `<script>`, or compute the value in executable content and read the result in the expression",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><state id=\"a\"><transition event=\"go\" cond=\"ready()\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><script>function ready() { return true; }</script><state id=\"a\"><transition event=\"go\" cond=\"ready()\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 Appendix B Data Models", url: "https://www.w3.org/TR/scxml/#profiles",
    ),
    doc!(
        "LINT020", "warning", "lint", "a condition has a side effect",
        why: "SCXML 1.0 §5.9.1: \"Conditional expressions in conformant SCXML documents SHOULD NOT have side effects.\" Appendix B also says a data model's condition language \"MUST not have side effects\", but official W3C IRP test 444 requires `cond=\"++var1==2\"` to run and increment `var1`, so the processor follows the IRP (docs/spec-errata.md) and a certain side effect in an ECMAScript condition (an assignment, update, or `delete` outside the condition, a mutating built-in, `Math.random`, or a call of a document function that does one of those) is a warning, not an error",
        help: "compute the condition without changing anything, and move the change into the transition's executable content (`<assign>` or `<script>`); to compare, use `===`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><datamodel><data id=\"tries\" expr=\"0\"/></datamodel><state id=\"a\"><transition event=\"retry\" cond=\"++tries &lt; 3\" target=\"a\"/><transition event=\"retry\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><datamodel><data id=\"tries\" expr=\"0\"/></datamodel><state id=\"a\"><transition event=\"retry\" cond=\"tries &lt; 2\" target=\"a\"><assign location=\"tries\" expr=\"tries + 1\"/></transition><transition event=\"retry\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §5.9.1 Conditional Expressions", url: "https://www.w3.org/TR/scxml/#ConditionalExpressions",
    ),
    doc!(
        "SCXML077", "warning", "semantic", "an expression or `<script>` changes a system variable itself",
        why: "SCXML 1.0 §5.10: \"The Processor MUST cause any attempt to change the value of a system variable to fail and MUST place the error 'error.execution' on the internal event queue when such an attempt is made.\" The document is conformant and the attempt fails at run time with `error.execution`, so this is a warning: the change never happens. The validator reports an assignment, compound assignment, update, or `delete` of `_event`, `_sessionid`, `_name`, `_ioprocessors`, or the platform root `_x`, and the static forms that write one through the global object (`globalThis._event = 1`, `delete globalThis['_event']`, `Reflect.deleteProperty(globalThis, '_event')`, `Reflect.defineProperty`, `Reflect.set`, `Object.defineProperty`), outside a scope that declares the name as its own (a function's variables and parameters, a block's `let`/`const`, a `for` head, a `catch` parameter). A non-strict or reflective deletion currently returns `false` without `error.execution` (a known limitation recorded in docs/spec-errata.md)",
        help: "copy the system variable into a `<data>` variable and change the copy, or use a name that does not begin with `_`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><state id=\"a\"><onentry><script>delete _event;</script></onentry><transition event=\"go\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><state id=\"a\"><onentry><script>var last = _event;</script></onentry><transition event=\"go\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §5.10 System Variables", url: "https://www.w3.org/TR/scxml/#SystemVariables",
    ),
    doc!(
        "LINT021", "warning", "lint", "a `<parallel>` has no child states, so no transition is enabled through it",
        why: "SCXML 1.0 §3.4.2 allows a `<parallel>` with no regions (its `<state>` children occur \"0 or more times\"), but §3.13 enables a transition only \"in atomic state S\" and §3.3 defines an atomic state as \"a <state> that has no <state>, <parallel> or <final> children\", so a `<parallel>` is never atomic: while a regionless one is active, none of its own transitions is taken, its ancestors' transitions are taken only through other active atomic states, and no `done.state` event is raised for it",
        help: "give the `<parallel>` its regions as child `<state>` elements, or make it an atomic `<state>`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"p\"><parallel id=\"p\"><transition event=\"go\" target=\"f\"/></parallel><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"p\"><state id=\"p\"><transition event=\"go\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.4 <parallel>", url: "https://www.w3.org/TR/scxml/#parallel",
    ),
    doc!(
        "LINT022", "warning", "lint", "an attribute or element in an unbound namespace has no effect",
        why: "SCXML 1.0 and its W3C schema allow attributes from other namespaces on SCXML elements and elements from other namespaces in their content models (the `##other` wildcards), and the processor executes none of them: an element in another namespace runs only as a custom action in executable content, and every other piece of foreign markup is metadata that only the package binding its namespace (its schema and package validator) reads. No extension package or registered schema binds this namespace, so nothing reads the markup; it is often a leftover or a misspelled namespace. `xml:*` (`xml:base` governs the `src` URIs in its scope; `xml:lang` and `xml:space` carry no SCXML behavior) and `xsi:*` attributes are accepted silently",
        help: "bind the namespace's package (`--extension-package MANIFEST.json`) or register its schema (`--schema URI=PATH`), or remove the markup",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" xmlns:ed=\"urn:example:editor\" version=\"1.0\"><final id=\"f\" ed:x=\"10\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"f\"/></scxml>",
        checked: true,
        spec: "", url: "",
    ),
    doc!(
        "SCXML079", "warning", "semantic", "an expression is used under the null data model",
        why: "SCXML 1.0 §B.1: \"There is no location expression language.\", \"There is no value expression language.\", and the condition language \"consists of the In predicate only\" (`In('stateID')`, the ID quoted as a string). Any other expression is ill-formed there, and §5.9.4 lets the processor \"wait and place 'error.execution' in the internal event queue at runtime when the expressions are evaluated\", which this toolchain does (a condition is then false), so the document compiles",
        help: "rewrite conditions as `In('stateID')` and remove the other expressions, or select a data model such as `ecmascript`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"null\" initial=\"s\"><state id=\"s\"><transition event=\"go\" cond=\"x &gt; 1\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"null\" initial=\"s\"><state id=\"s\"><transition event=\"go\" cond=\"In('s')\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §B.1 The Null Data Model", url: "https://www.w3.org/TR/scxml/#minimal-profile",
    ),
    doc!(
        "SCXML080", "warning", "semantic", "a target set combines a `<history>` with a state it restores",
        why: "SCXML 1.0 §3.10.2: when a transition targets a history pseudo-state, \"the SCXML processor MUST behave as if the transition had taken the stored state configuration for that history state as its target\" (or the default one), which covers the history's parent. Another target inside that parent is legal only when the restored configuration happens to contain it; otherwise the machine gets two active children of one state, and §3.11 says \"A conformant SCXML document MUST place the state machine only in legal state configurations\" (§A.2: a processor's behavior on a non-conformant document is undefined). The recorded configuration is known only at run time, so this is a warning",
        help: "target either the history or the state inside its parent, not both",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"o\"><state id=\"o\"><transition event=\"go\" target=\"h b1\"/></state><parallel id=\"p\"><history id=\"h\"><transition target=\"a\"/></history><state id=\"a\"/><state id=\"b\" initial=\"b0\"><state id=\"b0\"/><state id=\"b1\"/></state><transition event=\"out\" target=\"o\"/></parallel></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"o\"><state id=\"o\"><transition event=\"go\" target=\"h\"/></state><parallel id=\"p\"><history id=\"h\"><transition target=\"a\"/></history><state id=\"a\"><transition event=\"next\" target=\"b1\"/></state><state id=\"b\" initial=\"b0\"><state id=\"b0\"/><state id=\"b1\"/></state><transition event=\"out\" target=\"o\"/></parallel></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.11 Legal State Configurations and Specifications", url: "https://www.w3.org/TR/scxml/#LegalStateConfigurations",
    ),
    doc!(
        "LIMIT006", "error", "semantic", "a state ID is longer than a state name may be",
        why: "every build names the active states by their IDs (the configuration each call result reports, snapshots, and `In()`), and a name holds at most 1,024 bytes of UTF-8, so a state with a longer ID could never be reported once entered. A state that can complete (one with a `<final>` child, or a `<parallel>` with a `<state>` region that has a `<final>` child and whose every region can be in a final state at once; SCXML 1.0 Appendix D: a `<parallel>` whose regions are all `<parallel>` never completes) also raises `done.state.<ID>` (SCXML 1.0 §3.7: the processor \"MUST generate the event done.state.id\"), an event name of 11 more bytes, so its ID may have at most 1,013. Lengths are of the ID with its white space collapsed (`xsd:ID`, XML Schema 1.1 Part 2 §4.3.6), as the compiler reads it. SCXML 1.0 §3.14 sets no length; this is the implementation's interface bound, listed in the specification errata (L.6), and it applies to every build",
        help: "shorten the ID to at most 1,024 bytes (1,013 for a state that can complete) and update every reference to it",
        bad: "<state id=\"s…\"><final id=\"f\"/></state> (an ID of 1,014 bytes or more)",
        fixed: "<state id=\"s\"><final id=\"f\"/></state>",
        checked: false,
        spec: "SCXML 1.0 §3.14 IDs", url: "https://www.w3.org/TR/scxml/#IDs",
    ),
    doc!(
        "LIMIT007", "error", "semantic", "an event name or a send or invoke ID is longer than a name may be",
        why: "every build bounds an event name, a `<send>` ID, and an `<invoke>` ID to 1,024 bytes of UTF-8 (the processor's interface bound), so a `<raise event>`, `<send event>`, `<send id>`, or `<invoke id>` written longer could never run as written. An `<invoke>` ID must also leave room for the `done.invoke.<ID>` event the invocation returns (SCXML 1.0 §6.4: the service \"MUST return a special event 'done.invoke.id'\"), 12 more bytes, so it may have at most 1,012. Lengths are of the value with its white space collapsed (XML Schema 1.1 Part 2 §4.3.6), as the compiler reads it. Names computed at run time (`eventexpr`, generated IDs) that exceed the bound raise `error.execution` instead (SCXML 1.0 §3.12.2). SCXML 1.0 sets no length; this is the implementation's bound, listed in the specification errata (L.6), and it applies to every build",
        help: "shorten the name to at most 1,024 bytes (1,012 for an `<invoke>` ID)",
        bad: "<raise event=\"e…\"/> (an event name of 1,025 bytes or more)",
        fixed: "<raise event=\"e\"/>",
        checked: false,
        spec: "SCXML 1.0 §6.4 <invoke>", url: "https://www.w3.org/TR/scxml/#invoke",
    ),
    doc!(
        "ENV001", "error", "semantic", "an `env` attribute names a slot its invoked service does not have",
        why: "an invoked service's `<invoke>` names the variables whose values its host binds to it with `env:<slot>=\"NAME\"` (or `env:<slot>expr=\"EXPR\"`, an expression that evaluates to the name, evaluated when the `<invoke>` runs) in the `https://github.com/stateforward/scxml.rs/namespaces/env` namespace; the host gives the service component an opaque reference for each slot, for the whole invocation, never the value. The slots an `<invoke>` has are the ones its service package declares (`env-slots`), checked when the validator was given that package; without it (an unbound type, or a `typeexpr`), a slot bound twice (`env:<slot>` and `env:<slot>expr`), an `env:<slot>` value that is not a variable name (`[A-Za-z_][A-Za-z0-9_]*`), or an attribute that names no valid slot (`[a-z][a-z0-9-]*`) is still caught",
        help: "bind only a slot the service's package declares, once, to a variable name",
        bad: "<invoke type=\"https://stateforward.org/scxml/services/echo\" env:endpont=\"ECHO_URL\"/>",
        fixed: "<invoke type=\"https://stateforward.org/scxml/services/echo\" env:endpoint=\"ECHO_URL\"/>",
        checked: false,
        spec: "SCXML 1.0 §6.4 <invoke>", url: "https://www.w3.org/TR/scxml/#invoke",
    ),
    doc!(
        "ENV002", "error", "semantic", "the `env` namespace is used outside an invoked service's `<invoke>` attributes",
        why: "`https://github.com/stateforward/scxml.rs/namespaces/env` is the host's attribute vocabulary for the slots of an invoked service's `<invoke>`: it has no elements, an element in no namespace (inline data, for example) has no slots, an executable-content extension element has none (executable content runs synchronously inside a microstep and does no I/O, so it never needs a value; work that does belongs in an invoked service), and neither has the `<invoke>` of an SCXML session (an absent `type`, `scxml`, or `http://www.w3.org/TR/scxml/`) or any other SCXML element",
        help: "remove the element or attribute, and bind the slot on the `<invoke>` of the service that needs the value",
        bad: "<example:raise env:token=\"API_TOKEN\"/>",
        fixed: "<invoke type=\"https://stateforward.org/scxml/services/echo\" env:token=\"API_TOKEN\"/>",
        checked: false,
        spec: "SCXML 1.0 §6.4 <invoke>", url: "https://www.w3.org/TR/scxml/#invoke",
    ),
    doc!(
        "EVENT001", "error", "semantic", "the `event` namespace is used outside the attributes of an element with event data or a transition's `<event:defer/>`",
        why: "`https://github.com/stateforward/scxml.rs/namespaces/event` is the host's vocabulary for events: `event:schema` and `event:schemaexpr` on `<transition>` (the `_event.data` it handles), `<send>` and `<raise>` (the data their event is created with), and `<donedata>` (the data of the `done.state.<id>` event), and the empty `<event:defer/>` in the content of a `<transition>` with an `event` attribute (it holds the event the transition handles). It has no other elements or attributes, and they mean nothing anywhere else. The W3C schema admits attributes and elements in other namespaces laxly, so only this rule catches a misplaced or misspelled one",
        help: "move the attribute to the element whose event data it describes, or `<event:defer/>` into the transition whose event it holds, correct the name, or remove it",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" xmlns:event=\"https://github.com/stateforward/scxml.rs/namespaces/event\" version=\"1.0\" initial=\"s\"><state id=\"s\"><onentry event:schema=\"true\"/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" xmlns:event=\"https://github.com/stateforward/scxml.rs/namespaces/event\" version=\"1.0\" initial=\"s\"><state id=\"s\"><transition event=\"go\" target=\"f\" event:schema=\"true\"/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "", url: "",
    ),
    doc!(
        "EVENT002", "error", "semantic", "an element gives both `event:schema` and `event:schemaexpr`",
        why: "as with SCXML's other attribute and `expr` pairs (`event` and `eventexpr`, `target` and `targetexpr`), an element gives its schema either literally or as an expression whose value is the schema, not both",
        help: "keep one of the two attributes",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" xmlns:event=\"https://github.com/stateforward/scxml.rs/namespaces/event\" version=\"1.0\" initial=\"s\"><state id=\"s\"><transition event=\"go\" target=\"f\" event:schema=\"true\" event:schemaexpr=\"orderSchema\"/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" xmlns:event=\"https://github.com/stateforward/scxml.rs/namespaces/event\" version=\"1.0\" initial=\"s\"><state id=\"s\"><transition event=\"go\" target=\"f\" event:schemaexpr=\"orderSchema\"/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "", url: "",
    ),
    doc!(
        "EVENT003", "error", "syntax", "an `event:schema` is not JSON",
        why: "an `event:schema` value whose first non-white-space character is `{` (or that is exactly `true` or `false`) is the schema's JSON text, and a referenced schema file must hold UTF-8 JSON; the compiler embeds the parsed schema in the Machine IR, so it must parse",
        help: "write valid JSON; in a double-quoted XML attribute write `\"` as `&quot;`, or quote the attribute with `'`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" xmlns:event=\"https://github.com/stateforward/scxml.rs/namespaces/event\" version=\"1.0\" initial=\"s\"><state id=\"s\"><transition event=\"go\" target=\"f\" event:schema='{\"type\": object}'/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" xmlns:event=\"https://github.com/stateforward/scxml.rs/namespaces/event\" version=\"1.0\" initial=\"s\"><state id=\"s\"><transition event=\"go\" target=\"f\" event:schema='{\"type\": \"object\"}'/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "RFC 8259 The JavaScript Object Notation (JSON) Data Interchange Format", url: "https://www.rfc-editor.org/rfc/rfc8259",
    ),
    doc!(
        "EVENT004", "error", "semantic", "an `event:schema` file is not available",
        why: "a schema reference names a JSON file by a path relative to the document, at or below its directory (no scheme, absolute path, query, or `.`/`..` segment; repository-namespace resources are not supported). The compiler reads the file at compile time and embeds the schema in the Machine IR; nothing is read or fetched while the document runs, so a file the compiler cannot read rejects the document",
        help: "create the file next to the document, correct the path, or write the schema inline",
        bad: "<transition event=\"go\" target=\"done\" event:schema=\"schemas/missing.json\"/>",
        fixed: "<transition event=\"go\" target=\"done\" event:schema=\"schemas/order.json\"/>",
        checked: false,
        spec: "", url: "",
    ),
    doc!(
        "EVENT005", "error", "semantic", "the fragment of an `event:schema` reference selects no schema",
        why: "the fragment of a schema reference is a JSON Pointer (RFC 6901), percent-encoded as a URI fragment: `#/$defs/Order` selects member `Order` of member `$defs` of the file, with `~1` for `/` and `~0` for `~` in a member name. A plain-name fragment (`#Order`) is not supported",
        help: "point at an existing schema in the file, or drop the fragment to use the whole file",
        bad: "<transition event=\"go\" target=\"done\" event:schema=\"schemas/order.json#/$defs/Ordr\"/>",
        fixed: "<transition event=\"go\" target=\"done\" event:schema=\"schemas/order.json#/$defs/Order\"/>",
        checked: false,
        spec: "RFC 6901 JavaScript Object Notation (JSON) Pointer", url: "https://www.rfc-editor.org/rfc/rfc6901",
    ),
    doc!(
        "EVENT006", "error", "semantic", "an `event:schema` is not a valid JSON Schema",
        why: "the compiler checks a schema against the metaschema of its dialect and compiles it once before embedding it, so every keyword has a valid value and every `pattern` is a valid regular expression; the message names the offending location",
        help: "correct the keyword at the named location",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" xmlns:event=\"https://github.com/stateforward/scxml.rs/namespaces/event\" version=\"1.0\" initial=\"s\"><state id=\"s\"><transition event=\"go\" target=\"f\" event:schema='{\"type\": \"strin\"}'/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" xmlns:event=\"https://github.com/stateforward/scxml.rs/namespaces/event\" version=\"1.0\" initial=\"s\"><state id=\"s\"><transition event=\"go\" target=\"f\" event:schema='{\"type\": \"string\"}'/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "JSON Schema 2020-12 Validation", url: "https://json-schema.org/draft/2020-12/json-schema-validation",
    ),
    doc!(
        "EVENT007", "error", "semantic", "an `event:schema` declares an unsupported JSON Schema dialect",
        why: "the supported dialects are JSON Schema 2020-12 (`https://json-schema.org/draft/2020-12/schema`, the default when `$schema` is absent), 2019-09 (`https://json-schema.org/draft/2019-09/schema`), and draft-07 (`http://json-schema.org/draft-07/schema#`); a schema whose dialect the compiler cannot check is rejected rather than embedded unchecked",
        help: "declare a supported dialect in `$schema`, or remove `$schema` to use 2020-12",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" xmlns:event=\"https://github.com/stateforward/scxml.rs/namespaces/event\" version=\"1.0\" initial=\"s\"><state id=\"s\"><transition event=\"go\" target=\"f\" event:schema='{\"$schema\": \"http://json-schema.org/draft-04/schema#\"}'/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" xmlns:event=\"https://github.com/stateforward/scxml.rs/namespaces/event\" version=\"1.0\" initial=\"s\"><state id=\"s\"><transition event=\"go\" target=\"f\" event:schema='{\"$schema\": \"http://json-schema.org/draft-07/schema#\"}'/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "JSON Schema 2020-12 Core §8.1.1 The $schema Keyword", url: "https://json-schema.org/draft/2020-12/json-schema-core#section-8.1.1",
    ),
    doc!(
        "EVENT008", "error", "semantic", "an `event:schema` has a reference the compiler cannot keep",
        why: "no schema is fetched: a `$ref` may only name a fragment of the same file (`#/$defs/Item`), and one naming another resource is rejected rather than left dangling. When `event:schema` selects part of a file with a pointer, the references that part uses are bundled into the embedded schema; a part that relies on `$id`, `$anchor`, plain-name fragments, or dynamic references cannot be moved and is rejected",
        help: "copy the referenced schema into the same file and reference it by JSON Pointer, or reference the whole file without a fragment",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" xmlns:event=\"https://github.com/stateforward/scxml.rs/namespaces/event\" version=\"1.0\" initial=\"s\"><state id=\"s\"><transition event=\"go\" target=\"f\" event:schema='{\"$ref\": \"item.json\"}'/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" xmlns:event=\"https://github.com/stateforward/scxml.rs/namespaces/event\" version=\"1.0\" initial=\"s\"><state id=\"s\"><transition event=\"go\" target=\"f\" event:schema='{\"$ref\": \"#/$defs/item\", \"$defs\": {\"item\": {\"type\": \"string\"}}}'/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "JSON Schema 2020-12 Core §8.2.3.1 Direct References with $ref", url: "https://json-schema.org/draft/2020-12/json-schema-core#section-8.2.3.1",
    ),
    doc!(
        "PKG003", "warning", "semantic", "an `<invoke type>` is a platform-specific value, not a URI",
        why: "an absent `type`, `scxml`, or `http://www.w3.org/TR/scxml/` (with or without the trailing slash) invokes an SCXML session; any other literal type names an invoked service. SCXML 6.4.1 allows platform-specific type values, but a service package binds an invoked service by an exact absolute URI (a scheme, `:`, no white space, at most 4096 bytes), so on a host built from this repository an `<invoke>` of a type that is not a URI raises `error.execution` when it runs. Invocation is a runtime action: the document still compiles and runs",
        help: "write the service's absolute type URI exactly as its service package declares it (`service-uri`), or `scxml` for an SCXML session",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"s\"><state id=\"s\"><invoke type=\"echo\"/><transition event=\"done\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"s\"><state id=\"s\"><invoke type=\"https://stateforward.org/scxml/services/echo\"/><transition event=\"done\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §6.4.1 Attribute Details", url: "https://www.w3.org/TR/scxml/#invoke",
    ),
    doc!(
        "PKG004", "warning", "package", "no service package binds a literal `<invoke type>`",
        why: "an invoked service runs only in the service component its host binds to the exact type URI; the validator was given the service packages the run binds, and none declares this URI. Invocation is a runtime action (SCXML 6.4): the document compiles and runs, and this `<invoke>` raises `error.execution` when it runs unless the running host binds the type. Reported only when the validator is given its service packages (`scx validate` always is: none by default)",
        help: "bind the service with its package manifest (`--service-package MANIFEST.json`), or correct the URI to one a bound package declares",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"s\"><state id=\"s\"><invoke type=\"https://stateforward.org/scxml/services/echo\"/></state></scxml>",
        fixed: "the same document validated with `--service-package components/scxml-service-example/package/echo.service.json`",
        checked: false,
        spec: "SCXML 1.0 §6.4 <invoke>", url: "https://www.w3.org/TR/scxml/#invoke",
    ),
    doc!(
        "PKG005", "error", "package", "a machine's data model is neither built in nor available from a package",
        why: "SCXML 1.0 §5.1 lets a document choose its data model with the `datamodel` token; this toolchain builds in `ecmascript`, `null`, and `xpath`, and any other data model is a package named on the same `<scxml>` by `datamodel:src` (`https://github.com/stateforward/scxml.rs/namespaces/datamodel`), resolved like other packages. A token that names neither (often a typo of a built-in one), a `datamodel:src` that is no tagless absolute URI, or one no given data-model package binds (its repository release did not resolve) fails closed, because none of the machine's expressions could be evaluated. The value is `xsd:NMTOKEN`, whose white space collapses (XML Schema 1.1 Part 2 §4.3.6), so `datamodel=\" xpath \"` is `xpath`",
        help: "use a built-in data model, or name the data model's package with `datamodel:src` and make sure it resolves (or bind it with `--data-model-package MANIFEST.json`)",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascipt\"><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\"><final id=\"f\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §5.1 Data Model", url: "https://www.w3.org/TR/scxml/#data-module",
    ),
    doc!(
        "EVENT009", "error", "semantic", "the literal data of a `<send>` or `<donedata>` does not satisfy its own `event:schema`",
        why: "in an ECMAScript document, literal JSON `<content>` (no `expr`, text only) is the event data the element creates, known before the document runs, and the element's literal `event:schema` (inline, or a file whose bytes were supplied) describes that data; data the schema rejects breaks the element's own contract. `event:schemaexpr` and schemas that do not resolve are skipped",
        help: "change the `<content>` to data the schema admits, or correct the schema",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" xmlns:event=\"https://github.com/stateforward/scxml.rs/namespaces/event\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"s\"><final id=\"s\"><onentry><send event=\"x\" event:schema='{\"type\":\"object\",\"required\":[\"n\"]}'><content>{\"m\": 1}</content></send></onentry></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" xmlns:event=\"https://github.com/stateforward/scxml.rs/namespaces/event\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"s\"><final id=\"s\"><onentry><send event=\"x\" event:schema='{\"type\":\"object\",\"required\":[\"n\"]}'><content>{\"n\": 1}</content></send></onentry></final></scxml>",
        checked: true,
        spec: "JSON Schema 2020-12 Validation", url: "https://json-schema.org/draft/2020-12/json-schema-validation",
    ),
    doc!(
        "EVENT010", "warning", "semantic", "a `<send>` or `<donedata>` creates event data a transition handling it does not accept",
        why: "in an ECMAScript document, a `<send event=\"x\">` delivered to its own session (no `target` or `#_internal`, the SCXML Event I/O Processor) or the `<donedata>` of a compound state (its `done.state.<id>`) creates data that a transition handling the event (SCXML 3.12.1 descriptor matching) describes with a literal `event:schema`; this is reported only when provable: the literal `<content>` data fails the transition's schema, or the producer's own literal schema admits an instance the transition's rejects (a witness drawn from its `const`, `enum`, `examples`, `default`, or a minimal instance of each `type`). Equal schemas and undecided cases are never reported; `event:schemaexpr` is skipped. At run time the session checks the same contract: such a `<send>` is not dispatched and raises `error.execution`, while a `<donedata>` is never checked, so its done event still takes the transition",
        help: "make the two schemas agree, or change the data",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" xmlns:event=\"https://github.com/stateforward/scxml.rs/namespaces/event\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"s\"><state id=\"s\"><onentry><send event=\"x\"><content>{\"n\": 1}</content></send></onentry><transition event=\"x\" event:schema='{\"type\":\"object\",\"properties\":{\"n\":{\"type\":\"string\"}}}' target=\"f\"/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" xmlns:event=\"https://github.com/stateforward/scxml.rs/namespaces/event\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"s\"><state id=\"s\"><onentry><send event=\"x\"><content>{\"n\": \"one\"}</content></send></onentry><transition event=\"x\" event:schema='{\"type\":\"object\",\"properties\":{\"n\":{\"type\":\"string\"}}}' target=\"f\"/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "SCXML 1.0 §3.12.1 Event Descriptors", url: "https://www.w3.org/TR/scxml/#EventDescriptors",
    ),
    doc!(
        "EVENT011", "warning", "semantic", "a `<send>` between a document and an `<invoke src>` document creates event data the receiving transition does not accept",
        why: "SCXML 6.4 lets a session and the session it invokes exchange events: the invoker sends to `#_<invokeid>`, and the invoked session sends to `#_parent`. When the caller supplies the documents a document's literal `<invoke src>` elements load (`scx validate`, `compile`, and `run` read `file:` ones; the validator component and the browser host take `invoked-documents`), a `<send event=\"x\">` with a literal target from an ECMAScript sender is judged against the receiving document's transitions handling `x` (SCXML 3.12.1 descriptor matching) exactly as `EVENT010` judges one document's own sends: only when the literal `<content>` data, or a witness of the sender's own literal schema, provably fails the transition's literal `event:schema`. Every label is in the validated document, which names the invoked document by its `src`. At run time the receiving session checks the same contract (`accepts-event-data`): such a `<send>` is not routed and raises `error.execution` in the sender",
        help: "make the two documents' schemas agree, or change the data",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"s\"><state id=\"s\"><invoke id=\"kid\" src=\"child.scx\"/><onentry><send event=\"go\" target=\"#_kid\"><content>{\"n\": 1}</content></send></onentry></state></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"s\"><state id=\"s\"><invoke id=\"kid\" src=\"child.scx\"/><onentry><send event=\"go\" target=\"#_kid\"><content>{\"n\": \"one\"}</content></send></onentry></state></scxml>",
        checked: false,
        spec: "SCXML 1.0 §6.4 <invoke>", url: "https://www.w3.org/TR/scxml/#invoke",
    ),
    doc!(
        "DATAMODEL001", "error", "semantic", "the `datamodel` namespace is used outside `<scxml>`'s `datamodel:src`",
        why: "`https://github.com/stateforward/scxml.rs/namespaces/datamodel` is the host's attribute vocabulary for naming a data model's package: it defines exactly one attribute, `datamodel:src` on `<scxml>`, the only element that names a data model (SCXML 1.0 §3.2.1). It has no elements and no other attributes (an expression cannot select a data model: it needs one to be evaluated in), and an invoked document declares its own pair. The W3C schema admits attributes in other namespaces laxly, so only this rule catches a misplaced or misspelled one",
        help: "move `datamodel:src` to the `<scxml>` whose data model it names, correct its name, or remove it",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" xmlns:datamodel=\"https://github.com/stateforward/scxml.rs/namespaces/datamodel\" version=\"1.0\"><final id=\"f\" datamodel:src=\"https://github.com/o/r/datamodels/json\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"f\"/></scxml>",
        checked: true,
        spec: "", url: "",
    ),
    doc!(
        "DATAMODEL002", "error", "semantic", "the `datamodel` token is not the name of the package `datamodel:src` names",
        why: "the core `datamodel` attribute stays the token SCXML 1.0 §3.2.1 defines and names the data model the machine's expressions are written in, while the package `datamodel:src` names declares the one it implements (its manifest's `datamodel`); when they differ, the document's expressions would run in a data model they were not written for",
        help: "write the package's name as the `datamodel` token, or name the package of the data model the token names",
        bad: "<scxml … datamodel=\"jsn\" datamodel:src=\"https://github.com/o/r/datamodels/json\"> (the package declares `json`)",
        fixed: "<scxml … datamodel=\"json\" datamodel:src=\"https://github.com/o/r/datamodels/json\">",
        checked: false,
        spec: "SCXML 1.0 §5.1 Data Model", url: "https://www.w3.org/TR/scxml/#data-module",
    ),
    doc!(
        "XML001", "error", "syntax", "the document is not well-formed XML",
        why: "SCXML is an XML vocabulary; until the document parses, no schema or semantic rule can run",
        help: "fix the XML syntax at the marked position: remove Markdown code fences or prose outside the single root element, keep `<?xml …?>` at the very start, escape `<` as `&lt;` and `&` as `&amp;` in attribute values and text (or use `<![CDATA[ … ]]>` in `<script>`), close or rename mismatched tags, quote attribute values, specify each attribute once, and never write `--` inside a comment",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"a\"></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"a\"/></scxml>",
        checked: true,
        spec: "XML 1.0 (Fifth Edition) §2.1 Well-Formed XML Documents", url: "https://www.w3.org/TR/xml/#sec-well-formed",
    ),
    doc!(
        "XML002", "error", "syntax", "the document exceeds a configured XML resource limit",
        why: "the validator bounds source bytes (8 MiB), nesting depth (256), parser events, nodes, and attributes so untrusted input cannot exhaust memory or time; XML 1.0 sets no such limits, so these are validator limits documented in docs/validator-rules.md. The size and count limits apply to wasm builds only (the shipped validator component); the native `scx` bounds only the nesting depth, which also guards its stack, and entity expansion",
        help: "split the machine into smaller documents, reduce nesting, or validate with larger explicit limits",
        bad: "<a><a><a> … nested deeper than 256 elements … </a></a></a>",
        fixed: "a document within the configured depth and size limits",
        checked: false,
        spec: "", url: "",
    ),
    doc!(
        "XML003", "error", "syntax", "an entity reference names no predefined or declared entity",
        why: "XML predefines only `&lt;`, `&gt;`, `&amp;`, `&apos;`, and `&quot;`; any other entity must be declared in the document's internal DTD subset (XML 1.0 §4.1 WFC: Entity Declared)",
        help: "write the character as a numeric reference such as `&#160;`, or escape a literal ampersand as `&amp;`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"f\"><onentry><log label=\"a&nbsp;b\"/></onentry></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"f\"><onentry><log label=\"a&#160;b\"/></onentry></final></scxml>",
        checked: true,
        spec: "XML 1.0 (Fifth Edition) §4.6 Predefined Entities", url: "https://www.w3.org/TR/xml/#sec-predefined-ent",
    ),
    doc!(
        "XML004", "error", "syntax", "a character reference names no legal XML character",
        why: "XML 1.0 `Char` excludes NUL, most C0 controls, surrogates, and U+FFFE/U+FFFF, and references must be `&#N;` or lowercase-`x` `&#xH;`",
        help: "reference a legal character with `&#NNNN;` or `&#xHHHH;`, or remove the reference",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"f\"><onentry><log label=\"&#0;\"/></onentry></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"f\"><onentry><log label=\"&#48;\"/></onentry></final></scxml>",
        checked: true,
        spec: "XML 1.0 (Fifth Edition) §4.1 Character and Entity References", url: "https://www.w3.org/TR/xml/#sec-references",
    ),
    doc!(
        "XML005", "error", "internal", "the caller configured an invalid XML limit",
        why: "every XML limit must be a positive bound; a zero maximum would reject every document and indicates a host configuration error",
        help: "configure every XML limit with a non-zero maximum",
        bad: "XmlLimits { max_depth: 0, .. }",
        fixed: "XmlLimits::DEFAULT",
        checked: false,
        spec: "", url: "",
    ),
    doc!(
        "XML006", "error", "syntax", "the document relies on an entity the parser does not read",
        why: "the parser processes the internal DTD subset as XML 1.0 §5.1 requires of a non-validating processor (general and parameter entities, attribute-list defaults) but never reads an external entity: §4.4.3 lets a non-validating processor not include one, and reading it could disclose local files or reach the network; after a reference to a parameter entity that is not read, later entity and attribute-list declarations are not processed (§5.1), so an entity only they would declare is not read either",
        help: "write the text in place, or declare an internal entity (`<!ENTITY name \"text\">`) in the internal subset",
        bad: "<!DOCTYPE scxml [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><datamodel><data id=\"d\">&x;</data></datamodel><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><datamodel><data id=\"d\">text</data></datamodel><final id=\"f\"/></scxml>",
        checked: true,
        spec: "XML 1.0 (Fifth Edition) §4.4 XML Processor Treatment of Entities and References", url: "https://www.w3.org/TR/xml/#entproc",
    ),
    doc!(
        "XSD001", "error", "schema", "no XML Schema is registered for a namespace used by the document",
        why: "under the `strict` policy every non-built-in namespace must be structurally validated; the SCXML namespace uses the bundled W3C schema by default, extension namespaces need an explicit schema",
        help: "register the namespace's XSD with `--schema URI=PATH`, correct the namespace URI, or use `--policy lax`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" xmlns:x=\"urn:example:x\" version=\"1.0\"><final id=\"f\"><onentry><x:act/></onentry></final></scxml>",
        fixed: "scx validate machine.scxml --schema urn:example:x=x.xsd",
        checked: false,
        spec: "SCXML 1.0 §4.10 Extensibility of Executable Content", url: "https://www.w3.org/TR/scxml/#extensibility",
    ),
    doc!(
        "XSD002", "error", "schema", "a supplied schema is not parseable XML Schema",
        why: "schemas are untrusted input; a schema that is not UTF-8 XML or not an `xs:schema` document cannot govern structure",
        help: "fix or replace the schema file registered for this namespace",
        bad: "--schema urn:x=broken.xsd  (not an xs:schema document)",
        fixed: "--schema urn:x=valid.xsd",
        checked: false,
        spec: "XML Schema 1.0 Part 1 §3.15 Schemas as a Whole", url: "https://www.w3.org/TR/xmlschema-1/#Schemas",
    ),
    doc!(
        "XSD003", "error", "schema", "a supplied schema set does not compile",
        why: "unresolved type, element, or group references and invalid derivations make a schema unusable; validation fails closed instead of skipping it",
        help: "fix the schema so every referenced component is declared in the supplied set",
        bad: "<xs:element name=\"x\" type=\"missingType\"/>",
        fixed: "<xs:element name=\"x\" type=\"xs:string\"/>",
        checked: false,
        spec: "XML Schema 1.0 Part 1 §5 Schemas and Namespaces: Access and Composition", url: "https://www.w3.org/TR/xmlschema-1/#composition",
    ),
    doc!(
        "XSD004", "error", "schema", "a schema content model cannot be prepared for validation",
        why: "content models must be deterministic (Unique Particle Attribution) and within the backend's supported forms",
        help: "rewrite the ambiguous content model so every element can be attributed to one particle",
        bad: "<xs:choice><xs:element name=\"a\"/><xs:sequence><xs:element name=\"a\"/></xs:sequence></xs:choice>",
        fixed: "<xs:element name=\"a\"/>",
        checked: false,
        spec: "XML Schema 1.0 Part 1 §3.8.6 Unique Particle Attribution", url: "https://www.w3.org/TR/xmlschema-1/#cos-nonambig",
    ),
    doc!(
        "XSD005", "error", "schema", "the XML Schema instance driver could not complete",
        why: "the schema backend re-reads the instance; if it cannot finish, the result would be partial, so validation fails closed",
        help: "re-run after fixing any XML diagnostics; if none are reported, report this input as a validator bug",
        bad: "an instance the backend cannot finish reading",
        fixed: "a well-formed instance",
        checked: false,
        spec: "XML Schema 1.0 Part 1 §5.2 Assessing Schema-Validity", url: "https://www.w3.org/TR/xmlschema-1/#validation_outcome",
    ),
    doc!(
        "XSD006", "error", "schema", "schema or instance validation work exceeds a bounded budget",
        why: "schema size, content-model expansion, compilation, and validation work are preflighted so hostile schemas cannot exhaust the validator. Content-model expansion and compilation work grow faster than the schema, so they are bounded on every build; schema count, size, and instance validation work are bounded on wasm builds only (the shipped validator component)",
        help: "reduce large `maxOccurs` values, nested repetition, or schema size",
        bad: "<xs:sequence maxOccurs=\"4294967295\"><xs:choice maxOccurs=\"4294967295\">…",
        fixed: "<xs:sequence maxOccurs=\"unbounded\"><xs:choice>…",
        checked: false,
        spec: "XML Schema 1.0 Part 1 §3.9 Particles", url: "https://www.w3.org/TR/xmlschema-1/#cParticles",
    ),
    doc!(
        "XSD007", "error", "schema", "a schema model-group reference cannot be budgeted",
        why: "the preflight must expand every `xs:group ref` to bound work; undefined or circular groups cannot be expanded soundly",
        help: "define the referenced group in the supplied schema set and remove circular group references",
        bad: "<xs:group ref=\"missing\"/>",
        fixed: "<xs:group name=\"missing\">…</xs:group>",
        checked: false,
        spec: "XML Schema 1.0 Part 1 §3.7 Model Group Definitions", url: "https://www.w3.org/TR/xmlschema-1/#cModel_Group_Definitions",
    ),
    family!(
        "XSD-src-resolve", "the governing schema references a component it does not define",
        why: "schema representation constraints (`src-*`) are checked lazily when an instance first uses the broken declaration, so the error surfaces at that element",
        help: "fix the schema registered for this namespace so every referenced type, element, and group is defined",
        bad: "", fixed: "", checked: false,
        spec: "XML Schema 1.0 Part 1 §3.15.3 Constraints on XML Representations of Schemas", url: "https://www.w3.org/TR/xmlschema-1/#src-resolve",
    ),
    family!(
        "XSD-cvc-attribute", "an attribute value is not valid for its declared type",
        why: "the governing schema declares a datatype for this attribute; values outside its lexical space are rejected",
        help: "use a value of the attribute's declared type",
        bad: "", fixed: "", checked: false,
        spec: "XML Schema 1.0 Part 1 §3.2.4 Attribute Locally Valid", url: "https://www.w3.org/TR/xmlschema-1/#cvc-attribute",
    ),
    family!(
        "XSD-cvc-complex-type", "an element's content or attributes violate its complex type",
        why: "the governing schema fixes which children, text, and attributes an element may have",
        help: "follow the element's content model as shown in the diagnostic help",
        bad: "", fixed: "", checked: false,
        spec: "XML Schema 1.0 Part 1 §3.4.4 Element Locally Valid (Complex Type)", url: "https://www.w3.org/TR/xmlschema-1/#cvc-complex-type",
    ),
    family!(
        "XSD-cvc-complex-type.2.3", "an element with element-only content contains text",
        why: "structural SCXML elements such as `<state>` contain only child elements; stray text is usually a misplaced value or an unclosed tag",
        help: "remove the text, or move the value into `<data>`, `<content>`, `<script>`, or a `<log expr=\"…\">`",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"f\">done</final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"f\"/></scxml>",
        checked: true,
        spec: "XML Schema 1.0 Part 1 §3.4.4 Element Locally Valid (Complex Type)", url: "https://www.w3.org/TR/xmlschema-1/#cvc-complex-type",
    ),
    family!(
        "XSD-cvc-complex-type.2.4", "a child element is not allowed at this position, or required children are missing",
        why: "each SCXML element has a fixed content model; for example executable content belongs only in `<onentry>`, `<onexit>`, and `<transition>`",
        help: "move the element to a parent that accepts it, or add the missing children",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><state id=\"a\"><log expr=\"1\"/><transition event=\"go\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><state id=\"a\"><onentry><log expr=\"1\"/></onentry><transition event=\"go\" target=\"f\"/></state><final id=\"f\"/></scxml>",
        checked: true,
        spec: "XML Schema 1.0 Part 1 §3.4.4 Element Locally Valid (Complex Type)", url: "https://www.w3.org/TR/xmlschema-1/#cvc-complex-type",
    ),
    family!(
        "XSD-cvc-complex-type.3.2.2", "an attribute is not declared for this element",
        why: "SCXML elements accept only their declared attributes; extension attributes must be namespace-qualified",
        help: "correct the attribute name (an attribute of a related element becomes this element's own, such as `id` for `<cancel>`'s `sendid` on `<send>`), remove it, or qualify an extension attribute with a namespace prefix",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><transition evnt=\"go\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><transition event=\"go\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        checked: true,
        spec: "XML Schema 1.0 Part 1 §3.4.4 Element Locally Valid (Complex Type)", url: "https://www.w3.org/TR/xmlschema-1/#cvc-complex-type",
    ),
    family!(
        "XSD-cvc-complex-type.4", "a required attribute is missing",
        why: "the schema requires this attribute; for example `<scxml>` must declare `version=\"1.0\"` and `<if>` must declare `cond`",
        help: "add the required attribute",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\"><final id=\"f\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"f\"/></scxml>",
        checked: true,
        spec: "XML Schema 1.0 Part 1 §3.4.4 Element Locally Valid (Complex Type)", url: "https://www.w3.org/TR/xmlschema-1/#cvc-complex-type",
    ),
    family!(
        "XSD-cvc-datatype-valid", "a value is not in the lexical space of its datatype",
        why: "SCXML attributes such as `delay` (a CSS2 duration) have restricted lexical forms checked by the schema",
        help: "use a value in the datatype's lexical form",
        bad: "", fixed: "", checked: false,
        spec: "XML Schema 1.0 Part 2 §4.1.4 Datatype Valid", url: "https://www.w3.org/TR/xmlschema-2/#cvc-datatype-valid",
    ),
    family!(
        "XSD-cvc-elt", "an element is not valid against its declaration",
        why: "every element in a strictly validated namespace must be declared by its schema",
        help: "correct the element name or namespace, or declare it in the namespace's schema",
        bad: "", fixed: "", checked: false,
        spec: "XML Schema 1.0 Part 1 §3.3.4 Element Locally Valid (Element)", url: "https://www.w3.org/TR/xmlschema-1/#cvc-elt",
    ),
    family!(
        "XSD-cvc-elt.1", "an element is not declared in its namespace",
        why: "misspelled SCXML elements are not SCXML at all; the schema reports them before any semantic rule can misinterpret the document",
        help: "correct the element name, or move an extension element into its own namespace",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><trasition event=\"go\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><transition event=\"go\" target=\"b\"/></state><final id=\"b\"/></scxml>",
        checked: true,
        spec: "XML Schema 1.0 Part 1 §3.3.4 Element Locally Valid (Element)", url: "https://www.w3.org/TR/xmlschema-1/#cvc-elt",
    ),
    family!(
        "XSD-cvc-enumeration-valid", "a value is not one of the allowed enumerated values",
        why: "attributes such as `<transition type>` and `<history type>` accept only a fixed set of values",
        help: "use one of the listed values",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><transition event=\"go\" target=\"b\" type=\"internel\"/></state><final id=\"b\"/></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" initial=\"a\"><state id=\"a\"><transition event=\"go\" target=\"b\" type=\"internal\"/></state><final id=\"b\"/></scxml>",
        checked: true,
        spec: "XML Schema 1.0 Part 2 §4.3.5.4 Enumeration valid", url: "https://www.w3.org/TR/xmlschema-2/#rf-enumeration",
    ),
    family!(
        "XSD-cvc-pattern-valid", "a value does not match its datatype's pattern",
        why: "SCXML datatypes such as durations (`delay`) and event names are defined by regular expressions in the W3C schema",
        help: "use a value matching the documented lexical form",
        bad: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"a\"><onentry><send event=\"e\" delay=\"5 seconds\"/></onentry></final></scxml>",
        fixed: "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\"><final id=\"a\"><onentry><send event=\"e\" delay=\"5s\"/></onentry></final></scxml>",
        checked: true,
        spec: "XML Schema 1.0 Part 2 §4.3.4.4 Pattern valid", url: "https://www.w3.org/TR/xmlschema-2/#rf-pattern",
    ),
    family!(
        "XSD-cvc-simple-type", "a value is not valid for its simple type",
        why: "the governing schema restricts this value's simple type",
        help: "use a value of the declared simple type",
        bad: "", fixed: "", checked: false,
        spec: "XML Schema 1.0 Part 1 §3.14.4 String Valid", url: "https://www.w3.org/TR/xmlschema-1/#cvc-simple-type",
    ),
    family!(
        "XSD-cvc-type", "an element or attribute value does not match its type",
        why: "the governing schema assigns this item a type that its value or content does not satisfy",
        help: "use content of the declared type",
        bad: "", fixed: "", checked: false,
        spec: "XML Schema 1.0 Part 1 §3.3.4 Element Locally Valid (Type)", url: "https://www.w3.org/TR/xmlschema-1/#cvc-type",
    ),
];

/// Where the session snapshot codes are documented.
const SNAPSHOT_DOC: &str = "docs/session-snapshot.md#error-codes";

/// Session snapshot codes (`SNAP-<code>`, from the `scxml-snapshot`
/// crate's `SnapshotErrorCode`), reported by `scxml snapshot check` and
/// the hosts' snapshot calls. They are not validator diagnostics, so they
/// are kept out of [`CODES`] (the validator rule table and SARIF rules);
/// [`lookup`] and `--explain` cover both tables.
pub const SNAPSHOT_CODES: &[CodeDoc] = &[
    doc!(
        "SNAP-too-large", "error", "snapshot", "the snapshot document is larger than 32 MiB",
        why: "a document is at most 33,554,432 bytes so readers stay bounded; the encoder fails rather than truncating, and the decoder rejects a larger input before parsing it",
        help: "snapshot with sections excluded (for example `--exclude datamodel --exclude previews`), or snapshot a checkpoint with smaller queues",
        bad: "a 40 MiB file passed to `scxml snapshot check`",
        fixed: "the same checkpoint snapshotted with `--exclude datamodel`",
        checked: false,
        spec: "docs/session-snapshot.md, Error codes", url: SNAPSHOT_DOC,
    ),
    doc!(
        "SNAP-malformed-json", "error", "snapshot", "the snapshot document is not JSON",
        why: "a session snapshot is one compact UTF-8 JSON object; nothing else can be checked until the bytes parse",
        help: "pass the exact bytes `scxml snapshot run` (or a host's `snapshot`) wrote, without editing, concatenating, or re-encoding them",
        bad: "a truncated file ending in `\"configuration\":{\"atomic\":[`",
        fixed: "the complete document as written",
        checked: false,
        spec: "docs/session-snapshot.md, Error codes", url: SNAPSHOT_DOC,
    ),
    doc!(
        "SNAP-unsupported-version", "error", "snapshot", "the snapshot document names another `version`",
        why: "this build reads only `scxml.session-snapshot/v2`; another version can change any key, so the document is not interpreted",
        help: "produce the document with this release, or read it with the release that wrote it",
        bad: "{\"version\":\"scxml.session-snapshot/v3\",…}",
        fixed: "{\"version\":\"scxml.session-snapshot/v2\",…}",
        checked: false,
        spec: "docs/session-snapshot.md, Error codes", url: SNAPSHOT_DOC,
    ),
    doc!(
        "SNAP-schema-violation", "error", "snapshot", "a key, type, or enumerated value does not match the v1 shape",
        why: "the document has a closed shape (`crates/scxml-snapshot/schema/scxml.session-snapshot.v2.schema.json`): unknown keys, JSON floats, and values outside an enumeration are rejected, and digests are 64 lowercase hex characters",
        help: "go to the `= pointer:` location (for an unknown key, the object that holds it) and remove the unknown key or write the value in its documented form: unsigned integers without a fraction or exponent, data-model numbers as strings, statuses `ready`, `running`, or `finished`",
        bad: "\"clock\":{\"now_ms\":250.0}",
        fixed: "\"clock\":{\"now_ms\":250}",
        checked: false,
        spec: "docs/session-snapshot.md, Error codes", url: SNAPSHOT_DOC,
    ),
    doc!(
        "SNAP-non-canonical", "error", "snapshot", "the document is valid JSON but not its canonical encoding",
        why: "documents are byte-canonical so equal states have equal bytes and digests: compact output, keys in contract order, no written `null`, no trailing newline, sorted `options.exclude`, and data-model numbers and JSON values in canonical text",
        help: "do not reformat or pretty-print the document; regenerate it, or re-emit a valid one with `scxml snapshot show --format json`",
        bad: "{\"version\": \"scxml.session-snapshot/v2\", …}\n",
        fixed: "{\"version\":\"scxml.session-snapshot/v2\",…}",
        checked: false,
        spec: "docs/session-snapshot.md, Error codes", url: SNAPSHOT_DOC,
    ),
    doc!(
        "SNAP-integrity-mismatch", "error", "snapshot", "`sha256` does not match the canonical bytes of `snapshot`",
        why: "the digest detects corruption or hand edits of the snapshot (it is not authentication)",
        help: "regenerate the document instead of editing it; a snapshot is read-only and is never restored",
        bad: "a document whose `snapshot` was edited after encoding",
        fixed: "the document exactly as the encoder wrote it",
        checked: false,
        spec: "docs/session-snapshot.md, Error codes", url: SNAPSHOT_DOC,
    ),
    doc!(
        "SNAP-bounds-exceeded", "error", "snapshot", "a list, string, or integer is over its documented bound",
        why: "the document inherits the runtime bounds: 10,000 queue entries, configuration states, or transitions; 1,024 pending sends; 256 variables; 65,536-byte values (a longer variable is written as `{\"type\":\"oversized\",\"bytes\":N}`); 16,384-byte conditions; integers at most 2^53 - 1",
        help: "snapshot an earlier checkpoint or exclude the section; a bound over the runtime's own limits signals a broken capture",
        bad: "a `pending_sends` list with 1,025 entries",
        fixed: "at most 1,024 pending sends",
        checked: false,
        spec: "docs/session-snapshot.md, Error codes", url: SNAPSHOT_DOC,
    ),
    doc!(
        "SNAP-invalid-name", "error", "snapshot", "an identifier is not a valid SCXML name",
        why: "session ids, state ids, event names, invoke ids, and send ids are non-empty, whitespace-free names of at most 1,024 bytes, as the runtime requires",
        help: "check the host that produced the capture; the runtime never creates such an identifier",
        bad: "\"session_id\":\"has space\"",
        fixed: "\"session_id\":\"session-1\"",
        checked: false,
        spec: "docs/session-snapshot.md, Error codes", url: SNAPSHOT_DOC,
    ),
    doc!(
        "SNAP-inconsistent", "error", "snapshot", "a derived field disagrees, an id repeats, or a parent link is broken",
        why: "derived fields must agree with their sources: `actions.cancel.sendids` with `pending_sends`, `has_cond` with `cond`, `action` with `type`, `configuration.atomic` with `full`, `system` with the session identity, child `invoked_by` with its invocation; excluded sections must be absent and included ones present; components must cover every role of the root session and its invoked sessions",
        help: "regenerate the document from a single checkpoint capture; do not merge or edit sections",
        bad: "\"cond\":\"x > 1\",\"has_cond\":false",
        fixed: "\"cond\":\"x > 1\",\"has_cond\":true",
        checked: false,
        spec: "docs/session-snapshot.md, Error codes", url: SNAPSHOT_DOC,
    ),
    doc!(
        "SNAP-depth-exceeded", "error", "snapshot", "an invoked session is deeper than the nested invoke depth bound",
        why: "invoked sessions nest to at most 32 nested invoke depth below the root session (the ceiling of every host's configured invoke limits), so a deeper invoked session is not a valid document",
        help: "regenerate the document from a host whose invoke limits stay within the ceilings",
        bad: "an invoked session at nested invoke depth 33",
        fixed: "invoked sessions at most 32 nested invoke depth",
        checked: false,
        spec: "docs/session-snapshot.md, Error codes", url: SNAPSHOT_DOC,
    ),
    doc!(
        "SNAP-clock-invariant", "error", "snapshot", "a session clock differs from the shared virtual clock",
        why: "the scheduler advances every session clock in lockstep, so each child's clock equals `clock.now_ms` at a checkpoint",
        help: "report the host defect; the capture was gathered outside a coherent checkpoint",
        bad: "a child clock at 300 ms under a root clock at 250 ms",
        fixed: "every clock at 250 ms",
        checked: false,
        spec: "docs/session-snapshot.md, Error codes", url: SNAPSHOT_DOC,
    ),
    doc!(
        "SNAP-not-stable", "error", "snapshot", "the snapshot was requested outside a stable checkpoint",
        why: "a snapshot is taken only when every machine call returned in a stable configuration (SCXML Appendix D: empty internal queue, no enabled eventless transition) and no host scheduling operation is in progress; elsewhere it changes nothing and fails",
        help: "request the snapshot at a checkpoint: after `start`, after an input, at the top of a drain iteration, or at the end of the run; pending delayed sends do not prevent a checkpoint",
        bad: "session.snapshot() called from a data-model or telemetry callback while a machine call is still running",
        fixed: "the same snapshot requested after the call returned",
        checked: false,
        spec: "docs/session-snapshot.md, Error codes", url: SNAPSHOT_DOC,
    ),
    doc!(
        "SNAP-status-invalid", "error", "snapshot", "a `failed` or `shut-down` session cannot be snapshotted",
        why: "only `ready`, `running`, and `finished` sessions have a defined state to show; every host reports this code for a `failed` or `shut-down` session",
        help: "snapshot before the session failed or was shut down",
        bad: "a snapshot of a session after `shutdown`",
        fixed: "a snapshot taken before `shutdown`",
        checked: false,
        spec: "docs/session-snapshot.md, Error codes", url: SNAPSHOT_DOC,
    ),
    doc!(
        "SNAP-codec-unavailable", "error", "snapshot", "the snapshot codec is not available in this host",
        why: "the browser host loads the optional `scxml-snapshot-codec` component lazily; without it (`snapshot: 'none'` or no `dist/snapshot/`) nothing can be encoded or diffed",
        help: "ship `dist/snapshot/` with the browser host, or create the session with `snapshot: 'lazy'`",
        bad: "createSession({ snapshot: 'none' }).snapshot()",
        fixed: "createSession({ snapshot: 'lazy' }).snapshot()",
        checked: false,
        spec: "docs/session-snapshot.md, Error codes", url: SNAPSHOT_DOC,
    ),
];

/// Returns the documentation for `code`: an exact entry, else the longest
/// family entry that prefixes `code` at a `.` boundary.
#[must_use]
pub fn lookup(code: &str) -> Option<&'static CodeDoc> {
    let exact = CODES
        .iter()
        .chain(SNAPSHOT_CODES)
        .find(|entry| entry.code == code);
    exact.or_else(|| {
        CODES
            .iter()
            .filter(|entry| {
                entry.family
                    && code.starts_with(entry.code)
                    && code
                        .get(entry.code.len()..)
                        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
            })
            .max_by_key(|entry| entry.code.len())
    })
}

/// Appends one formatted line. Formatting into a `String` has no I/O, so no
/// `Result` is produced or discarded.
fn push_line(text: &mut String, line: fmt::Arguments<'_>) {
    text.push_str(&fmt::format(line));
    text.push('\n');
}

/// The period that ends a prose field as a sentence, or nothing when the
/// field already ends one (`.`, `!`, `?`, or a parenthesized sentence `.)`).
fn sentence_end(field: &str) -> &'static str {
    if field.ends_with(['.', '!', '?']) || field.ends_with(".)") {
        ""
    } else {
        "."
    }
}

/// Whether the complete-document examples of `entry` are checked by a
/// test: `validate` checks self-checking entries
/// (`scxml-validator-core/tests/explain_examples.rs`), and `compile` checks
/// the compile-only `COMP` codes
/// (`scxml-compiler-core/tests/explain_examples.rs`).
#[must_use]
pub fn examples_are_checked(entry: &CodeDoc) -> bool {
    entry.self_checking || entry.code.starts_with("COMP")
}

/// Whether `example` is a complete document rather than a prose sketch.
#[must_use]
pub fn is_document_example(example: &str) -> bool {
    example.trim_start().starts_with('<') && !example.contains('…')
}

/// Code-fence language of one example: `xml` for a checked document an
/// agent can copy, `text` for a prose sketch (with `…` placeholders, or an
/// entry whose examples no test can check), which is not presented as XML.
fn fence(entry: &CodeDoc, example: &str) -> &'static str {
    if examples_are_checked(entry) && is_document_example(example) {
        "xml"
    } else {
        "text"
    }
}

/// Renders the rustc-`--explain`-style long form for one entry.
#[must_use]
pub fn explanation(entry: &CodeDoc) -> String {
    let mut text = String::new();
    let heading = if entry.family {
        format!("{}.*", entry.code)
    } else {
        entry.code.to_owned()
    };
    push_line(
        &mut text,
        format_args!(
            "{heading} ({} · {}): {}",
            entry.severity, entry.category, entry.title
        ),
    );
    text.push('\n');
    push_line(
        &mut text,
        format_args!("Why: {}{}", entry.why, sentence_end(entry.why)),
    );
    text.push('\n');
    push_line(
        &mut text,
        format_args!("How to fix: {}{}", entry.help, sentence_end(entry.help)),
    );
    if !entry.bad.is_empty() {
        text.push('\n');
        push_line(&mut text, format_args!("Erroneous example:"));
        text.push('\n');
        push_line(
            &mut text,
            format_args!("```{}\n{}\n```", fence(entry, entry.bad), entry.bad),
        );
    }
    if !entry.fixed.is_empty() {
        text.push('\n');
        push_line(&mut text, format_args!("Corrected example:"));
        text.push('\n');
        push_line(
            &mut text,
            format_args!("```{}\n{}\n```", fence(entry, entry.fixed), entry.fixed),
        );
    }
    if !entry.spec.is_empty() {
        text.push('\n');
        push_line(
            &mut text,
            format_args!("Specification: {} <{}>", entry.spec, entry.url),
        );
    }
    text
}

/// Renders the generated Markdown code tables embedded in
/// `docs/validator-rules.md` between the generated-section markers: one
/// table per family of [`crate::CODE_FAMILIES`], in that order.
#[must_use]
pub fn markdown_table() -> String {
    let mut text = String::new();
    for (family, meaning) in crate::CODE_FAMILIES {
        push_line(&mut text, format_args!("### `{family}`: {meaning}"));
        text.push('\n');
        text.push_str(
            "| Code | Severity | Category | Summary | Specification |\n| --- | --- | --- | --- | --- |\n",
        );
        for entry in CODES
            .iter()
            .filter(|entry| crate::code_family(entry.code) == Some(*family))
        {
            push_table_row(&mut text, entry);
        }
        text.push('\n');
    }
    text
}

fn push_table_row(text: &mut String, entry: &CodeDoc) {
    let code = if entry.family {
        format!("`{}.*`", entry.code)
    } else {
        format!("`{}`", entry.code)
    };
    let spec = if entry.url.is_empty() {
        "—".to_owned()
    } else {
        format!("[{}]({})", entry.spec, entry.url)
    };
    push_line(
        text,
        format_args!(
            "| {code} | {} | {} | {} | {spec} |",
            entry.severity,
            entry.category,
            entry.title.replace('|', "\\|")
        ),
    );
}

/// Base URL of the SCXML 1.0 Recommendation used by the table's links.
#[must_use]
pub const fn scxml_recommendation_url() -> &'static str {
    SCXML_TR
}
