//! `SCXML074` and `LINT019`: side effects in conditions and `<log expr>`.
//!
//! SCXML 1.0 requires the condition language of every data model to be free
//! of side effects (Appendix B: "Specify the boolean expression language
//! used as the value of the 'cond' attribute in `<transition>`, `<if>` and
//! `<elseif>` This language MUST not have side effects"), asks documents not
//! to rely on them (§5.9.1: "Conditional expressions in conformant SCXML
//! documents SHOULD NOT have side effects."), and requires `<log>` to have
//! none (§4.7.3: "The SCXML processor MUST ensure that `<log>` has no
//! side-effects on document interpretation."). Those are the only
//! side-effect requirements on expressions, so only `cond` on
//! `<transition>`, `<if>`, and `<elseif>` and `<log expr>` are checked.
//!
//! The ECMAScript data model's expressions are ECMAScript expressions (B.2),
//! which can assign, so the validator enforces the rule statically through
//! the linked checker's `check-effects` analysis: syntax that certainly has
//! a side effect is the `SCXML074` error in a `<log expr>` (§4.7.3 is a
//! processor MUST) and the `LINT020` warning in a condition (§5.9.1's
//! SHOULD NOT governs documents; Appendix B's MUST contradicts official IRP
//! test 444, recorded in docs/spec-errata.md), and what the analysis cannot
//! decide is the `LINT019` warning. An assignment, update, or `delete` of a
//! system variable itself anywhere is the `SCXML077` warning: §5.10 defines
//! its run-time outcome (the attempt fails with `error.execution`), so the
//! document is not rejected. The runtime
//! evaluates ECMAScript as ECMAScript; this is the document-side guarantee.
//!
//! Records no telemetry of its own; it runs inside the validator's boundary
//! span, and source text never enters telemetry.

use scxml_syntax::{Document, Element};
use scxml_types::ExternalScripts;

use super::xpath::decoded_fix;
use super::{ExpressionKind, ExpressionSyntaxChecker, SPEC_ECMASCRIPT, Site};
use crate::semantic::{is_scxml, visit_machine};
use crate::{Diagnostic, Label, Report};

const SPEC_DATA_MODELS: &str = "SCXML 1.0 Appendix B Data Models";
const SPEC_CONDITIONS: &str = "SCXML 1.0 §5.9.1 Conditional Expressions";
const SPEC_LOG: &str = "SCXML 1.0 §4.7.3 <log>";

/// What one fragment is in its document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectRole<'a> {
    /// A `cond` on `<transition>`, `<if>`, or `<elseif>` (checked).
    Condition,
    /// A `<log expr>` (checked).
    Log,
    /// A static `<script>` body.
    Script,
    /// A `<data expr>` with its `id`.
    Data(&'a str),
    /// Any other value expression.
    Value,
    /// A location.
    Location,
}

/// One fragment of a machine's source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EffectFragment<'a> {
    /// The fragment's role.
    pub role: EffectRole<'a>,
    /// Its decoded source text.
    pub source: &'a str,
}

/// What a finding, or its cause, does (WIT `effect-kind`).
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EffectKind {
    /// Assigns a variable or property outside the checked expression.
    Assignment,
    /// Increments or decrements one.
    Update,
    /// Deletes one.
    Deletion,
    /// Calls a built-in method that changes its receiver or an argument.
    MutatingCall,
    /// Draws from a random sequence whose position then advances.
    Random,
    /// Calls a document function; the cause says what its body does.
    FunctionCall,
    /// Calls a function the analysis cannot see.
    UnknownCall,
    /// Runs text as code.
    DynamicCode,
    /// Reads a value while a document getter or conversion method has an
    /// effect.
    ImplicitCall,
    /// Writes through a reference the analysis cannot trace.
    UnknownWrite,
    /// Uses a dynamic scope.
    DynamicScope,
    /// Calls something an unread script may define.
    UnseenScript,
    /// Exceeds the analysis bounds.
    TooComplex,
    /// Assigns, updates, or deletes a system variable itself.
    SystemVariableWrite,
}

/// A byte range of one fragment.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EffectSpan {
    /// Index of the fragment in the request.
    pub fragment: usize,
    /// Inclusive UTF-8 byte offset into its source.
    pub start: usize,
    /// Exclusive UTF-8 byte offset (`start <= end`).
    pub end: usize,
}

/// Where a call's effect happens.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EffectCause {
    /// What happens there.
    pub kind: EffectKind,
    /// Where.
    pub span: EffectSpan,
    /// What it touches or calls.
    pub subject: String,
    /// The function whose body contains it (empty when anonymous).
    pub function: String,
}

/// One effect of a checked fragment.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EffectFinding {
    /// True when the syntax certainly has the effect; false when the
    /// analysis cannot decide.
    pub certain: bool,
    /// What it does.
    pub kind: EffectKind,
    /// Where in the checked fragment.
    pub span: EffectSpan,
    /// What it touches or calls.
    pub subject: String,
    /// For a call, where the callee's effect happens.
    pub cause: Option<EffectCause>,
    /// The `=` of a condition that is exactly one plain assignment.
    pub operator: Option<EffectSpan>,
}

/// Result of analyzing one machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectCheck {
    /// The analysis ran; its findings, sorted.
    Checked(Vec<EffectFinding>),
    /// The checker declined the request.
    NotChecked,
}

/// Whether `site` is a checked fragment.
fn role<'s>(site: &'s Site<'_>) -> EffectRole<'s> {
    let local = site.element.name.local_name.as_str();
    match site.kind {
        ExpressionKind::Condition => EffectRole::Condition,
        ExpressionKind::Script => EffectRole::Script,
        ExpressionKind::Location => EffectRole::Location,
        ExpressionKind::Value => match (local, site.what.as_str()) {
            ("log", "expr") => EffectRole::Log,
            ("data", "expr") => site
                .element
                .attribute("id")
                .map_or(EffectRole::Value, |id| EffectRole::Data(id.value.as_str())),
            _ => EffectRole::Value,
        },
    }
}

/// The text of each `<script src>` file, keyed by the `src` value as
/// authored with its XML white space collapsed
/// ([`crate::Validator::with_script_resources`]).
pub type ScriptResources = std::collections::BTreeMap<String, String>;

/// One `<script src>` of the machine whose file the caller supplied.
struct ReadScript<'a> {
    /// Its `src` attribute.
    src: &'a scxml_syntax::Attribute,
    /// The file's text.
    text: &'a str,
}

/// The machine's `<script src>` elements whose files `scripts` holds, in
/// document order and each file once, and whether any other `<script src>`
/// is left unread.
fn read_scripts<'a>(
    machine: &'a Element,
    scripts: &'a ScriptResources,
) -> (Vec<ReadScript<'a>>, ExternalScripts) {
    let mut read = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut unread = ExternalScripts::Absent;
    visit_machine(machine, &mut |element| {
        if !is_scxml(element, "script") {
            return;
        }
        let Some(src) = element.attribute("src") else {
            return;
        };
        let key = scxml_syntax::collapse_whitespace(&src.value);
        match scripts.get_key_value(key.as_ref()) {
            Some((key, text)) => {
                if seen.insert(key.as_str()) {
                    read.push(ReadScript { src, text });
                }
            }
            None => unread = ExternalScripts::Present,
        }
    });
    (read, unread)
}

/// Where one fragment of the request came from.
#[derive(Clone, Copy)]
enum Origin<'s, 'd> {
    /// A fragment of the document.
    Site(&'s Site<'d>),
    /// The file a `<script src>` names: findings in it are reported at its
    /// `src` value.
    Script(&'s scxml_syntax::Attribute),
}

impl Origin<'_, '_> {
    /// The document span of bytes `start..end` of the fragment.
    fn span_of(self, start: usize, end: usize) -> scxml_syntax::Span {
        match self {
            Self::Site(site) => site.span_of(start, end),
            Self::Script(src) => src.value_span,
        }
    }
}

/// Runs the checker's analysis over the machine's parsed `sites`, and the
/// text of each `<script src>` file `scripts` holds, and reports
/// `SCXML074`/`LINT019`.
pub(super) fn report_effects(
    document: &Document,
    checker: &dyn ExpressionSyntaxChecker,
    machine: &Element,
    sites: &[&Site<'_>],
    scripts: &ScriptResources,
    report: &mut Report,
) {
    let (read, unread) = read_scripts(machine, scripts);
    let mut origins: Vec<Origin<'_, '_>> = sites.iter().map(|site| Origin::Site(site)).collect();
    origins.extend(read.iter().map(|script| Origin::Script(script.src)));
    let fragments: Vec<EffectFragment<'_>> = sites
        .iter()
        .map(|site| EffectFragment {
            role: role(site),
            source: &site.source,
        })
        .chain(read.iter().map(|script| EffectFragment {
            role: EffectRole::Script,
            source: script.text,
        }))
        .collect();
    let answer = match checker.check_effects(&fragments, unread) {
        EffectCheck::Checked(findings) if trustworthy(&fragments, &findings) => {
            EffectCheck::Checked(findings)
        }
        _ => EffectCheck::NotChecked,
    };
    match answer {
        EffectCheck::Checked(findings) => {
            for finding in findings {
                if let Some(diagnostic) = diagnostic(document, &origins, &finding) {
                    report.push(diagnostic);
                }
            }
        }
        EffectCheck::NotChecked => {
            if let Some((site, _)) = sites.iter().zip(&fragments).find(|(_, fragment)| {
                matches!(fragment.role, EffectRole::Condition | EffectRole::Log)
            }) {
                report.push(not_analyzed(site));
            }
        }
    }
}

/// Most findings a checker may return for one fragment.
const MAX_FINDINGS_PER_FRAGMENT: usize = 4;
/// Most bytes of a finding's subject or function name.
const MAX_SUBJECT_BYTES: usize = 256;

/// Whether the checker's answer is within its contract: every span inside
/// its fragment on character boundaries, only checked fragments reported,
/// at most [`MAX_FINDINGS_PER_FRAGMENT`] each, and bounded strings. The
/// answer crosses a trust boundary, so anything else is declined.
fn trustworthy(fragments: &[EffectFragment<'_>], findings: &[EffectFinding]) -> bool {
    let span_ok = |span: &EffectSpan| {
        fragments.get(span.fragment).is_some_and(|fragment| {
            span.start <= span.end
                && span.end <= fragment.source.len()
                && fragment.source.is_char_boundary(span.start)
                && fragment.source.is_char_boundary(span.end)
        })
    };
    let mut per_fragment = std::collections::BTreeMap::<usize, usize>::new();
    findings.iter().all(|finding| {
        let count = per_fragment.entry(finding.span.fragment).or_default();
        *count += 1;
        *count <= MAX_FINDINGS_PER_FRAGMENT
            && span_ok(&finding.span)
            && fragments
                .get(finding.span.fragment)
                .is_some_and(|fragment| {
                    finding.kind == EffectKind::SystemVariableWrite
                        || matches!(fragment.role, EffectRole::Condition | EffectRole::Log)
                })
            && finding.subject.len() <= MAX_SUBJECT_BYTES
            && finding.operator.as_ref().is_none_or(|operator| {
                operator.fragment == finding.span.fragment && span_ok(operator)
            })
            && finding.cause.as_ref().is_none_or(|cause| {
                span_ok(&cause.span)
                    && cause.subject.len() <= MAX_SUBJECT_BYTES
                    && cause.function.len() <= MAX_SUBJECT_BYTES
            })
    })
}

/// `LINT019` when the checker cannot analyze the machine at all.
fn not_analyzed(site: &Site<'_>) -> Diagnostic {
    Diagnostic::warning(
        "LINT019",
        format!(
            "cannot prove that `{}` on `<{}>` has no side effect",
            site.what, site.element.name.local_name
        ),
        site.value_span,
    )
    .with_primary_message("the linked checker declined the side-effect analysis")
    .with_note(
        "the machine's conditions and `<log>` expressions were not analyzed for side effects",
    )
    .with_help("link the ECMAScript data model's checker (`check-effects`) to analyze them")
    .with_spec_reference(SPEC_DATA_MODELS)
    .with_spec_reference(SPEC_ECMASCRIPT)
}

fn subject_is_member(subject: &str) -> bool {
    subject.contains('.') || subject.contains('[')
}

/// The primary label of a certain effect.
fn certain_label(kind: EffectKind, subject: &str, scope: &str) -> String {
    match kind {
        EffectKind::Assignment => format!("assigns `{subject}`, which is outside {scope}"),
        EffectKind::Update => format!("changes `{subject}`, which is outside {scope}"),
        EffectKind::Deletion => format!("deletes `{subject}`, which is outside {scope}"),
        EffectKind::MutatingCall if subject_is_member(subject) => {
            format!("`{subject}` changes its first argument")
        }
        EffectKind::MutatingCall => format!("`.{subject}()` changes the object it is called on"),
        EffectKind::Random => format!("`{subject}()` advances the session's random sequence"),
        EffectKind::FunctionCall => format!("calls `{subject}`, which has a side effect"),
        _ => format!("`{subject}` has a side effect"),
    }
}

/// The primary label of an undecidable finding.
fn undecidable_label(kind: EffectKind, subject: &str) -> String {
    match kind {
        EffectKind::UnknownCall => format!("calls `{subject}`, which the validator cannot see"),
        EffectKind::UnseenScript => {
            format!("calls `{subject}`, which a `<script src>` may define or replace")
        }
        EffectKind::DynamicCode => format!("`{subject}` runs text as code"),
        EffectKind::ImplicitCall => format!(
            "reads values while `{subject}` (a getter or conversion method the document defines) has a side effect"
        ),
        EffectKind::UnknownWrite => {
            format!("writes `{subject}`, whose object the validator cannot trace")
        }
        EffectKind::DynamicScope => {
            "`with` decides at run time what each name refers to".to_owned()
        }
        EffectKind::MutatingCall => format!(
            "`.{subject}()` updates a regular expression's `lastIndex` when it is global or sticky"
        ),
        EffectKind::FunctionCall => {
            format!("calls `{subject}`, whose side effects the validator cannot decide")
        }
        EffectKind::TooComplex => "too complex to analyze within the validator's bounds".to_owned(),
        _ => format!("`{subject}` may have a side effect"),
    }
}

/// The related label at a call's cause.
fn cause_label(cause: &EffectCause) -> String {
    let within = if cause.function.is_empty() {
        "this function".to_owned()
    } else {
        format!("`{}`", cause.function)
    };
    let subject = &cause.subject;
    match cause.kind {
        EffectKind::Assignment => format!("{within} assigns `{subject}` here"),
        EffectKind::Update => format!("{within} changes `{subject}` here"),
        EffectKind::Deletion => format!("{within} deletes `{subject}` here"),
        EffectKind::MutatingCall => format!("{within} calls the mutating `{subject}` here"),
        EffectKind::Random => format!("{within} draws from `{subject}` here"),
        EffectKind::UnknownCall | EffectKind::UnseenScript => {
            format!("{within} calls `{subject}` here, which the validator cannot see")
        }
        EffectKind::DynamicCode => format!("{within} runs text as code here"),
        EffectKind::UnknownWrite => {
            format!("{within} writes `{subject}` here, whose object the validator cannot trace")
        }
        EffectKind::DynamicScope => format!("{within} uses `with` here"),
        _ => format!("{within} does this here"),
    }
}

/// `SCXML077`: a fragment assigns, updates, or deletes a system variable
/// itself.
fn system_variable_write(origin: Origin<'_, '_>, finding: &EffectFinding) -> Diagnostic {
    let what = match origin {
        Origin::Site(site) if site.kind == ExpressionKind::Script => "`<script>`".to_owned(),
        Origin::Site(site) => format!("`{}` on `<{}>`", site.what, site.element.name.local_name),
        Origin::Script(src) => format!("the `<script src>` file `{}`", src.value),
    };
    let name = &finding.subject;
    Diagnostic::warning(
        "SCXML077",
        format!("{what} changes the system variable `{name}`"),
        origin.span_of(finding.span.start, finding.span.end),
    )
    .with_primary_message(format!("`{name}` is a read-only system variable"))
    .with_note("SCXML 1.0 §5.10: \"The Processor MUST cause any attempt to change the value of a system variable to fail and MUST place the error 'error.execution' on the internal event queue when such an attempt is made.\" The change fails at run time with `error.execution`; a non-strict `delete` or a reflective deletion through the global object returns `false` without one (a known limitation, docs/spec-errata.md). Variable names beginning with '_' are reserved for system use, and '_x' is the root of platform-specific system variables.")
    .with_help(format!("copy `{name}` into a `<data>` variable and change that copy, or use a name that does not begin with `_`"))
    .with_spec_reference("SCXML 1.0 §5.10 System Variables")
    .with_spec_reference(SPEC_ECMASCRIPT)
}

fn diagnostic(
    document: &Document,
    origins: &[Origin<'_, '_>],
    finding: &EffectFinding,
) -> Option<Diagnostic> {
    let origin = *origins.get(finding.span.fragment)?;
    if finding.kind == EffectKind::SystemVariableWrite {
        return Some(system_variable_write(origin, finding));
    }
    // Every other finding is in a checked fragment (`trustworthy`), which
    // is always the document's own.
    let Origin::Site(site) = origin else {
        return None;
    };
    let log = role(site) == EffectRole::Log;
    let what = format!("`{}` on `<{}>`", site.what, site.element.name.local_name);
    let scope = if log {
        "the expression"
    } else {
        "the condition"
    };
    // A condition that is one plain `=` assignment was almost always meant
    // as a comparison: the label points at the operator.
    let operator = finding
        .operator
        .filter(|operator| finding.certain && operator.fragment == finding.span.fragment);
    let (primary, label) = match operator {
        Some(operator) => (
            site.span_of(operator.start, operator.end),
            format!("`=` assigns `{}` instead of comparing it", finding.subject),
        ),
        None => (
            site.span_of(finding.span.start, finding.span.end),
            if finding.certain {
                certain_label(finding.kind, &finding.subject, scope)
            } else {
                undecidable_label(finding.kind, &finding.subject)
            },
        ),
    };
    let mut diagnostic = if finding.certain && log {
        Diagnostic::error("SCXML074", format!("{what} has a side effect"), primary)
            .with_primary_message(label)
            .with_note("SCXML 1.0 §4.7.3: \"The SCXML processor MUST ensure that <log> has no side-effects on document interpretation.\"")
            .with_help("log a value without changing anything; move the change into `<assign>` or `<script>`")
    } else if finding.certain {
        Diagnostic::warning("LINT020", format!("{what} has a side effect"), primary)
            .with_primary_message(label)
            .with_note("SCXML 1.0 §5.9.1: \"Conditional expressions in conformant SCXML documents SHOULD NOT have side effects.\" (Appendix B's \"This language MUST not have side effects\" contradicts official IRP test 444, `cond=\"++var1==2\"`; see docs/spec-errata.md)")
            .with_help("compute the condition without changing anything; move the change into the transition's executable content (`<assign>` or `<script>`)")
    } else {
        Diagnostic::warning(
            "LINT019",
            format!("cannot prove that {what} has no side effect"),
            primary,
        )
        .with_primary_message(label)
        .with_note(if log {
            "`<log>` must have no side effects (SCXML 1.0 §4.7.3); the validator could not decide this expression"
        } else {
            "conditional expressions in conformant documents \"SHOULD NOT have side effects\" (SCXML 1.0 §5.9.1); the validator could not decide this condition"
        })
        .with_help(
            "declare the functions it calls in an inline `<script>`, or compute the value in executable content and read the result here",
        )
    };
    diagnostic = diagnostic
        .with_spec_reference(if log { SPEC_LOG } else { SPEC_CONDITIONS })
        .with_spec_reference(SPEC_DATA_MODELS)
        .with_spec_reference(SPEC_ECMASCRIPT);
    if let Some(cause) = &finding.cause
        && let Some(cause_origin) = origins.get(cause.span.fragment)
    {
        let message = match cause_origin {
            Origin::Site(_) => cause_label(cause),
            Origin::Script(src) => format!("{} (in `{}`)", cause_label(cause), src.value),
        };
        diagnostic = diagnostic.with_secondary(
            Label::new(cause_origin.span_of(cause.span.start, cause.span.end))
                .with_message(message),
        );
    }
    if let Some(operator) = operator
        && let Some(fix) = decoded_fix(
            document,
            site,
            operator.start,
            operator.end,
            "compare with `===`".to_owned(),
            "===",
        )
    {
        diagnostic = diagnostic
            .with_help("to compare, use `===` (or `==`); to change the value, move the assignment into `<assign>` in the transition")
            .with_fix(fix);
    }
    Some(diagnostic)
}
