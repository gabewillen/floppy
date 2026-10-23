// The ECMAScript of bots/builder/builder.scx (SCXML 5.8): loaded by its root
// <script src="builder.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared by several states.

function increment(n) {
  return n + 1;
}

// The delay attribute value of ms milliseconds.
function delay(ms) {
  return ms + 'ms';
}

function appended(list, value) {
  return list.concat([value]);
}

function isFrom(_event, invokeid) {
  return _event.invokeid === invokeid;
}

// The builder's own control event (no invoke id) tagged with run.
function isOwnEvent(_event, run) {
  return !_event.invokeid && _event.data !== null && typeof _event.data === 'object' && _event.data.run === run;
}

function hasPlan() {
  return plan !== null;
}

function lacksPlan() {
  return plan === null;
}

// The log suffix that summarizes the first diagnostic, or ''.
function firstDiagnosticSummary() {
  return diagnostics.length > 0 ? ': ' + diagnostics[0].kind + ' ' + diagnostics[0].code + ': ' + diagnostics[0].message : '';
}

// intake

// Whether the builder.request data satisfies schemas/builder.request.json.
function satisfiesBuilderRequest(_event) {
  return _event.data !== null && typeof _event.data === 'object' && !Array.isArray(_event.data) && Object.keys(_event.data).every(function (k) { return ['spec', 'name', 'constraints', 'max_attempts', 'model', 'generate_timeout_ms', 'generate_ceiling_ms', 'min_every_ms', 'turn_after_bound_ms', 'apply'].indexOf(k) >= 0; }) && isOptionalBound(_event.data.min_every_ms) && isOptionalBound(_event.data.turn_after_bound_ms) && isOptionalApply(_event.data.apply) && typeof _event.data.spec === 'string' && _event.data.spec.trim().length > 0 && (_event.data.name === undefined || (typeof _event.data.name === 'string' && _event.data.name.length > 0)) && (_event.data.constraints === undefined || (Array.isArray(_event.data.constraints) && _event.data.constraints.length <= 32 && _event.data.constraints.every(function (c) { return typeof c === 'string'; }))) && (_event.data.max_attempts === undefined || (Number.isInteger(_event.data.max_attempts) && _event.data.max_attempts >= 1 && _event.data.max_attempts <= 8)) && (_event.data.model === undefined || (typeof _event.data.model === 'string' && _event.data.model.length > 0)) && isOptionalGenerateBound(_event.data.generate_timeout_ms) && isOptionalGenerateBound(_event.data.generate_ceiling_ms);
}

// An absent lifetime bound, or one of 1 ms to 1 day.
function isOptionalBound(value) {
  return value === undefined || (Number.isInteger(value) && value >= 1 && value <= 86400000);
}

// An absent live turn, or {turn, input, targets} as schemas/builder.request.json has it.
function isOptionalApply(value) {
  return value === undefined || (value !== null && typeof value === 'object' && !Array.isArray(value) && Object.keys(value).every(function (k) { return ['turn', 'input', 'targets'].indexOf(k) >= 0; }) && Number.isInteger(value.turn) && value.turn >= 1 && value.input !== null && typeof value.input === 'object' && !Array.isArray(value.input) && Array.isArray(value.targets) && value.targets.length <= 32 && value.targets.every(function (t) { return typeof t === 'string' && t.length > 0; }));
}

// The request's live turn, else null (a turn behavior runs only its own tests).
function requestApply(_event) {
  return _event.data.apply === undefined ? null : _event.data.apply;
}

// A lifetime bound of the request, else null (not checked).
function requestBound(value) {
  return value === undefined ? null : value;
}

function requestName(_event) {
  return _event.data.name === undefined ? null : _event.data.name;
}

function requestConstraints(_event) {
  return _event.data.constraints === undefined ? [] : _event.data.constraints;
}

function requestMaxAttempts(_event) {
  return _event.data.max_attempts === undefined ? null : _event.data.max_attempts;
}

function requestModel(_event) {
  return _event.data.model === undefined ? model : _event.data.model;
}

function requestGenerateTimeoutMs(_event) {
  return _event.data.generate_timeout_ms === undefined ? generate_timeout_ms : _event.data.generate_timeout_ms;
}

function requestGenerateCeilingMs(_event) {
  return _event.data.generate_ceiling_ms === undefined ? generate_ceiling_ms : _event.data.generate_ceiling_ms;
}

// A request's optional generation bound: absent, or 1 ms to 1 h.
function isOptionalGenerateBound(value) {
  return value === undefined || (Number.isInteger(value) && value >= 1 && value <= 3600000);
}

function invalidRequestDiagnostics() {
  return [{kind: 'request', code: 'invalid-request', message: 'the builder.request data does not satisfy schemas/builder.request.json'}];
}

// design and author: the generate service's calls.

function isUnconfigured(_event) {
  return _event.invokeid === gen_id && gen_failure === 'unconfigured';
}

function unconfiguredDiagnostics() {
  return [{kind: 'generate', code: 'unconfigured', message: 'the generate service has no backend: the run holds no FLOPPY_BASE_URL (set it in the --env-file; FLOPPY_API_KEY is needed only by a keyed backend)'}];
}

// stage is design or author; product is the plan or draft it generates.
function generationFailedDiagnostics(stage, product) {
  return [{kind: 'generate', code: stage + '-failed', message: 'the ' + stage + ' generation failed (' + (gen_failure === null ? 'it ended without an event' : gen_failure) + '): no ' + product + ' within the ' + product + ' schema'}];
}

function generationTimeoutDiagnostics(stage) {
  return [{kind: 'timeout', code: stage + '-timeout', message: 'the ' + stage + ' generation made no progress for ' + generate_timeout_ms + ' ms or ran past ' + generate_ceiling_ms + ' ms in all, and was cancelled'}];
}

// Whether the builder reports its progress to an invoker (progress_target).
function reportsProgress() {
  return progress_target !== null;
}

// builder.progress's data: the stage the build is in.
function progressData(stage) {
  return ({stage: stage});
}

function generationStartFailedDiagnostics(stage) {
  return [{kind: 'generate', code: stage + '-failed', message: 'the ' + stage + ' generation could not start (error.execution: an unbound service type, a rejected start, or a parameter that did not evaluate)'}];
}

function generationCommunicationFailedDiagnostics(stage) {
  return [{kind: 'generate', code: stage + '-failed', message: 'the ' + stage + ' generation failed after it started (error.communication: the generate service returned an error, trapped, or returned an invalid event)'}];
}

// design

// A failed redesign leaves plan null: keep the plan it replaces.
function previousPlan() {
  return plan !== null ? plan : previous_plan;
}

function designLog() {
  return (previous_plan === null ? 'planning' : 'replanning') + ' the topology; failures ' + failures;
}

function designPrompt() {
  return JSON.stringify({task: 'design', spec: spec, constraints: constraints, previous_plan: previous_plan, diagnostics: diagnostics});
}

// The arguments of a generate.result's first call of the function
// named, else null (D41: the service's events are unchecked).
function namedArgs(_event, name) {
  var calls = Object(_event.data).tool_calls;
  var call = Array.isArray(calls) ? calls.filter(function (call) { return Object(call).name === name; })[0] : undefined;
  return call === undefined ? null : Object(call).args;
}

function planGenerated(_event) {
  return _event.invokeid === gen_id && plan !== null;
}

// author

function authorLog() {
  return mode + ' draft, attempt ' + attempt;
}

// The previous draft's field, before author forgets it (null: none).
function previousDraft(field) {
  return draft === null ? null : draft[field];
}

function authorPrompt() {
  var repair = mode === 'repair';
  return JSON.stringify({task: mode, name: fileName(), spec: spec, constraints: constraints, plan: plan, previous_scxml: repair ? scxml_text : null, previous_schemas: repair ? previous_schemas : null, previous_script: repair ? previous_script : null, previous_tests: repair ? previous_tests : null, diagnostics: diagnostics});
}

function draftGenerated(_event) {
  return _event.invokeid === gen_id && draft !== null;
}

// A generated draft whose scxml has no scxml element.
function draftWithoutDocument(_event) {
  return draftGenerated(_event) && draft.scxml.indexOf('<scxml') < 0;
}

function noDocumentDiagnostics() {
  return [{kind: 'draft', code: 'no-document', message: 'the draft\'s scxml holds no scxml element (it was ' + JSON.stringify(draft.scxml.slice(0, 80)) + '): scxml is the whole document text, from its root element to its end, and a repair returns all of it, not only what changed'}];
}

// writing (decision D121): the draft's three documents, beside each other.

// The name its documents are written under: the request's, else the plan's,
// with every run of other characters than letters, digits, _ and - as _, so
// it is one plain file name and a relative URI as it stands.
function fileName() {
  return String(name !== null ? name : plan.name).replace(/[^A-Za-z0-9_-]+/g, '_');
}

function draftFile(extension) {
  return fileName() + extension;
}

// The path of its .scx, relative to this document and to every document
// that invokes a learned behavior: each sits two levels below the layout
// root, whose behaviors directory the run grants the files service.
function draftPath() {
  return '../../behaviors/' + draftFile('.scx');
}

function schemasText() {
  return JSON.stringify(draft.schemas);
}

function writeFailedDiagnostics() {
  return [{kind: 'draft', code: 'write-failed', message: 'the draft could not be written as ' + draftFile('.scx') + ', .json and .js (error.execution: no directory granted to the files service, or a name it refuses)'}];
}

// validate

// The validator's errors but EVENT004: it is not given the draft's schema
// file (scx:eventschema names a literal file), which writing wrote and the
// simulation's compile reads.
function validatorErrors() {
  return report.validator.diagnostics.filter(function (d) { return d.severity === 'error' && d.code !== 'EVENT004'; });
}

function isValidatorClean() {
  return report !== null && validatorErrors().length === 0;
}

function hasTestsWithinBounds() {
  return Array.isArray(draft.tests) && draft.tests.length >= 1 && draft.tests.length <= max_tests && draft.tests.every(function (t) { return t.events.length <= 32; });
}

function isReadyToSimulate() {
  return isValidatorClean() && hasTestsWithinBounds();
}

function testsOutOfBoundsDiagnostics() {
  return [{kind: 'draft', code: 'tests-out-of-bounds', message: 'the draft must carry one to eight test scripts of at most 32 events each'}];
}

// The draft's script checks (decision D123): a script that is not empty is
// loaded by <script src="NAME.js">, and every function a cond or expr calls
// by its bare name is declared by the script, or is In or an ECMAScript
// global. A cond that throws (its function undeclared) on an eventless
// transition is retried every microstep, and scxml.rs ends the whole run
// there, not only the simulated child.
function scriptErrors() {
  var errors = [];
  if (draft.script.trim() !== '' && scxml_text.indexOf('src="' + draftFile('.js') + '"') < 0) {
    errors.push({kind: 'draft', code: 'script-not-loaded', message: 'script is not empty, but the document has no <script src="' + draftFile('.js') + '"/> as the first child of its root after datamodel, so none of its functions is defined'});
  }
  var undeclared = calledFunctions().filter(function (name) { return declaredFunctions().indexOf(name) < 0 && ECMASCRIPT_GLOBALS.indexOf(name) < 0; });
  if (undeclared.length > 0) {
    errors.push({kind: 'draft', code: 'undeclared-function', message: 'the document calls ' + undeclared.join(', ') + ', which script does not declare: declare each as a function in script, or call none'});
  }
  return errors;
}

var ECMASCRIPT_GLOBALS = ['In', 'Array', 'Boolean', 'Date', 'Error', 'Number', 'Object', 'RegExp', 'String', 'decodeURIComponent', 'encodeURIComponent', 'isFinite', 'isNaN', 'parseFloat', 'parseInt'];

// The distinct bare names the document's cond and *expr attributes call.
function calledFunctions() {
  var names = [];
  scxml_text.replace(/\s(?:cond|[a-z]*expr)\s*=\s*("[^"]*"|'[^']*')/g, function (attribute, quoted) {
    unescapedAttribute(quoted.slice(1, -1)).replace(/(^|[^.\w$])([A-Za-z_$][\w$]*)\s*\(/g, function (call, before, name) {
      if (names.indexOf(name) < 0 && ['function', 'if', 'return', 'typeof', 'while', 'for', 'switch', 'catch'].indexOf(name) < 0) {
        names.push(name);
      }
      return call;
    });
    return attribute;
  });
  return names;
}

function unescapedAttribute(value) {
  return value.replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&apos;/g, "'").replace(/&amp;/g, '&');
}

// The names script declares: function declarations and var, let or const.
function declaredFunctions() {
  var names = [];
  draft.script.replace(/(?:function\s+|(?:var|let|const)\s+)([A-Za-z_$][\w$]*)/g, function (declaration, name) {
    names.push(name);
    return declaration;
  });
  return names;
}

function hasScriptErrors() {
  return isReadyToSimulate() && script_errors.length > 0;
}

function hasMachineApplicableFixes() {
  return report !== null && typeof fixed === 'string' && fixed !== scxml_text && fix_passes < 2;
}

function isValidated() {
  return report !== null;
}

// Each error with rendered, the form scx validate prints it in (rustc's):
// a repair is told the line and column of what it names, not only its code.
function validateDiagnostics() {
  return validatorErrors().slice(0, 20).map(function (d) { var at = sourceAt(d.primary.span); return {kind: 'validate', code: d.code, label: d.primary.message ? d.primary.message : d.message, line: at.text.trim(), message: d.help ? d.message + ' Help: ' + d.help : d.message, rendered: renderedDiagnostic(d, at)}; });
}

// The character index of the UTF-8 byte offset of the report's source text
// (the report's spans count bytes).
function charIndexOfByte(text, byte) {
  var bytes = 0;
  var i = 0;
  while (i < text.length && bytes < byte) {
    var c = text.codePointAt(i);
    bytes += c < 0x80 ? 1 : c < 0x800 ? 2 : c < 0x10000 ? 3 : 4;
    i += c >= 0x10000 ? 2 : 1;
  }
  return i;
}

// Where a span starts in the report's source text: its line and column
// (from 1), the text of that line, and how many characters of it the span
// covers (at least one).
function sourceAt(span) {
  var text = report.validator.source_text;
  var start = charIndexOfByte(text, span.start);
  var end = charIndexOfByte(text, span.end);
  var lineStart = text.lastIndexOf('\n', start - 1) + 1;
  var lineEnd = text.indexOf('\n', start);
  var line = text.slice(lineStart, lineEnd < 0 ? text.length : lineEnd);
  return {line: text.slice(0, lineStart).split('\n').length, column: start - lineStart + 1, text: line, width: Math.max(1, Math.min(end, lineStart + line.length) - start)};
}

// A validator diagnostic as scx validate renders it: the code and message,
// --> file:line:column, the source line (at most 160 characters around the
// span) with the span marked and its label, then its notes, help and fixes.
function renderedDiagnostic(d, at) {
  var from = Math.max(0, at.column - 81);
  var shown = at.text.slice(from, from + 160);
  var gutter = ' '.repeat(String(at.line).length);
  var caret = ' '.repeat(at.column - 1 - from) + '^'.repeat(Math.max(1, Math.min(at.width, shown.length - (at.column - 1 - from))));
  return ['error[' + d.code + ']: ' + d.message, gutter + '--> ' + draftFile('.scx') + ':' + at.line + ':' + at.column, gutter + ' |', at.line + ' | ' + shown, gutter + ' | ' + caret + ' ' + d.primary.message].concat(d.notes.map(function (n) { return gutter + ' = note: ' + n; }), d.help ? [gutter + ' = help: ' + d.help] : [], d.fixes.map(function (f) { return gutter + ' = suggestion: ' + f.message; })).join('\n');
}

function validatorFailedDiagnostics() {
  return [{kind: 'draft', code: 'validator-failed', message: 'the validator could not check the draft (an oversized or unreadable document)'}];
}

// validate: the lifetime bounds (E0009, <behavior:lifetime_errors>), checked
// only when the request names them.

function isLifetimeChecked() {
  return min_every_ms !== null || turn_after_bound_ms !== null;
}

function hasLifetimeErrors() {
  return isReadyToSimulate() && lifetime_errors.length > 0;
}

// simulate

// The behavior checks: a persistent routine is checked by one forced tick
// (D84); a turn behavior, given the request's live turn, by one run on that turn (D131).
function simTests() {
  return isRoutineTicked() ? draft.tests.concat([firstTickTest()]) : isLiveTurnChecked() ? draft.tests.concat([liveTurnTest()]) : draft.tests;
}

function isRoutineTicked() {
  return isLifetimeChecked() && lifetime === 'persistent' && schedules.length > 0;
}

// The routine's tick, forced by delivering its first schedule's event at once (the
// timer clock fires its first schedule wait at once, decision D84), within 2 s.
function firstTickTest() {
  return {name: 'first tick', events: [{name: schedules[0], data: '', delay_ms: 0}], settle_ms: 2000, expect_events: ['bot.behavior.tick'], expect_done: false, tick: true};
}

// lifetime is written by validate's <behavior:lifetime_errors>, with or without bounds.
function isLiveTurnChecked() {
  return live_turn !== null && lifetime === 'turn';
}

// The run on the live turn (sim_apply): expect_events [] admits any outputs, and
// <behavior:verify_apply> judges its donedata.
function liveTurnTest() {
  return {name: 'live turn', events: [], settle_ms: liveTurnBoundMs(), expect_events: [], expect_done: true, apply: true, live: true, turn: live_turn.turn, input: live_turn.input, targets: live_turn.targets, handled: true};
}

// The 2 s verify timeout (decision D131), within the turn's silence bound when the request names it.
function liveTurnBoundMs() {
  return Math.min(turn_after_bound_ms === null ? 2000 : turn_after_bound_ms, 2000);
}

function simulateLog() {
  return sim_tests.length + ' test script(s)';
}

// The longest simulation validate admits plus a margin.
function simulateTimeoutDelay() {
  return delay(max_tests * sim_test_cap_ms + 60000);
}

function simulateTimeoutDiagnostics() {
  return [{kind: 'timeout', code: 'simulate-timeout', message: 'the simulation did not finish in time'}];
}

// sim_next

function allTestsRun() {
  return sim_index >= sim_tests.length;
}

// sim_run

function currentTest() {
  return sim_tests[sim_index];
}

function currentTestEvents() {
  return currentTest().events;
}

function candidateTarget() {
  return '#_' + candidate_id;
}

function stepDelay(step) {
  return delay(Math.min(Math.max(step.delay_ms, 0), 60000));
}

function stepData(step) {
  return step.data === '' ? null : JSON.parse(step.data);
}

// The settle time: the script's settle_ms, after its last event, within sim_test_cap_ms.
function settleDelay() {
  return delay(Math.min(sim_test_cap_ms, Math.max.apply(null, [1, currentTest().settle_ms].concat(currentTest().events.map(function (e) { return e.delay_ms + 1; })))));
}

function isOwnOrCandidateEvent(_event) {
  return !_event.invokeid || _event.invokeid === candidate_id;
}

// sim_apply

// Whether the current test is a run on a turn (sim_apply), with no events sent.
function isApplyTest() {
  return currentTest().apply === true;
}

// The params of an apply run, as the candidates runner gives them.
function testTurn() {
  return currentTest().turn;
}

function testInput() {
  return currentTest().input;
}

function testTargets() {
  return currentTest().targets;
}

// done.invoke's donedata, which scxml.rs does not check; none is {}, which
// <behavior:verify_apply> refuses (null in child_data: the draft never finished).
function appliedData(_event) {
  return _event.data === undefined || _event.data === null ? ({}) : _event.data;
}

// The run's error without the candidate's generated invoke id (D124: stable fingerprints).
function applyError() {
  return sim_error === null ? null : withoutInvokeId(sim_error, candidate_id);
}

// sim_check

// sim_results with the current test's result: expect_events in order
// (other outputs may come between), and expect_done.
function nextSimResults() {
  return sim_results.concat([(function (t) { var next = 0; observed.forEach(function (name) { if (next < t.expect_events.length && name === t.expect_events[next]) { next += 1; } }); return {name: t.name, invokeid: candidate_id, passed: sim_error === null && next === t.expect_events.length && child_done === t.expect_done && apply.length === 0, observed: observed, expected: t.expect_events, done: child_done, expect_done: t.expect_done, error: sim_error, apply: apply}; })(currentTest())]);
}

// Whether the current test is a routine's forced tick, whose ticks
// <behavior:verify_tick> checks (E0008).
function isTickTest() {
  return currentTest().tick === true;
}

// sim_verdict

function allTestsPassed() {
  return sim_results.every(function (r) { return r.passed; });
}

function simulateDiagnostics() {
  return sim_results.filter(function (r) { return !r.passed; }).map(function (r) { return r.apply.length > 0 ? r.apply.map(function (m) { return {kind: 'apply', code: 'E0008', message: m}; }) : [{kind: 'simulate', code: 'test-failed', message: 'test ' + r.name + ' expected the outputs ' + JSON.stringify(r.expected) + ' in order and done=' + r.expect_done + ', observed ' + JSON.stringify(r.observed) + ' and done=' + r.done + (r.error === null ? '' : ', error ' + simError(r)) + doneMismatch(r)}]; }).reduce(function (all, d) { return all.concat(d); }, []);
}

// A test's error event name without the candidate's generated invoke id
// (error.communication.<invokeid>): the id is new on every run, so with it
// the same test failing with the same error would never be the same failure.
function simError(r) {
  return withoutInvokeId(r.error, r.invokeid);
}

function withoutInvokeId(name, invokeid) {
  var suffix = '.' + invokeid;
  return typeof invokeid === 'string' && name.slice(-suffix.length) === suffix ? name.slice(0, -suffix.length) : name;
}

// What a test's done mismatch means for a machine the simulation invokes
// with no param and sends only the test's events.
function doneMismatch(r) {
  return r.done === r.expect_done ? '' : r.done ? ' (it reached a top-level final state the test did not expect)' : !hasTopLevelFinal() ? ' (it has no top-level final state: each of its final elements is inside a state, which it only completes; a final that ends the machine is a child of scxml itself)' : ' (it reached no top-level final state: invoked with no param, each data item keeps its own expr, and it waited for an event the test never sent, or cycled)';
}

// Whether the draft (well-formed: it validated) has a final element that is
// a child of its root, found by the depth of each tag outside comments.
function hasTopLevelFinal() {
  var depth = 0;
  var found = false;
  scxml_text.replace(/<!--[\s\S]*?-->|<!\[CDATA\[[\s\S]*?\]\]>/g, '').replace(/<(\/?)([A-Za-z_][\w:.-]*)((?:\s+[^\s=\/>]+\s*=\s*(?:"[^"]*"|'[^']*'))*)\s*(\/?)>/g, function (tag, close, name, attributes, empty) {
    if (close) {
      depth -= 1;
    } else {
      found = found || (depth === 1 && name.split(':').pop() === 'final');
      depth += empty ? 0 : 1;
    }
    return tag;
  });
  return found;
}

// retry_gate

// The sorted, distinct identities of the failure's diagnostics: kind|code
// and, for a validate diagnostic, its primary label (not the line it marks:
// the same mistake moves around between drafts), else its message (a
// simulate message names the test, its expected and observed outputs, its
// done and its error; the rest are the builder's own or a namespace's).
function diagnosticsFingerprint() {
  return diagnostics.map(function (d) { return d.kind + '|' + d.code + '|' + (d.kind === 'validate' ? d.label : d.message); }).sort().filter(function (key, i, keys) { return i === 0 || key !== keys[i - 1]; });
}

// The last draft a check rejected (validate, lifetime, simulate, apply),
// else rejected unchanged: the reflection stores the rejected source as a
// draft.
// One that could not be written is not.
function rejectedDraft() {
  return diagnostics.length > 0 && ['draft', 'validate', 'lifetime', 'simulate', 'apply'].indexOf(diagnostics[0].kind) >= 0 && diagnostics[0].code !== 'write-failed' && typeof scxml_text === 'string' ? scxml_text : rejected;
}

function nextAttemptLog() {
  return attempt_log.concat([{failure: failures, attempt: attempt, diagnostics: diagnostics.slice(0, 5)}]);
}

function retryGateLog() {
  return 'failure ' + failures + (max_attempts === null ? '' : ' of ' + max_attempts) + firstDiagnosticSummary();
}

// A diagnostic an earlier failure of this build already had, consecutive
// or not: the same failure more than once ends the build (decision D124).
function isNoProgress() {
  return fingerprint.some(function (key) { return fingerprints.indexOf(key) >= 0; });
}

// The identities of every failure so far, this one's included.
function nextFingerprints() {
  return fingerprints.concat(fingerprint);
}

function isAttemptsExhausted() {
  return max_attempts !== null && failures >= max_attempts;
}

// triage

function triageLog() {
  return 'choosing among ' + choices.length + ' option(s)';
}

function decisionState() {
  return {spec: spec, attempt: attempt, failures: failures, max_attempts: max_attempts, has_plan: plan !== null, diagnostics: diagnostics};
}

function isDecisionEvent(_event) {
  return decision_id !== null && _event.invokeid === decision_id;
}

// done

function doneLog() {
  return 'built ' + (plan === null ? 'a machine' : plan.name) + ' in ' + attempt + ' attempt(s)';
}

function doneData() {
  return {scxml: scxml_text, schemas: draft.schemas, script: draft.script, path: draftPath(), tests: draft.tests, report: {attempts: attempt, failures: failures, fixes_applied: fixes_applied, warnings: warnings, plan: plan, decisions: decisions, history: attempt_log, simulation: sim_results}};
}

// failed

function failedLog() {
  return failure_reason + firstDiagnosticSummary();
}

// With the last draft a check rejected, when there is one.
function failedData() {
  var data = {reason: failure_reason, diagnostics: diagnostics.map(function (d) { return {kind: d.kind, code: d.code, message: d.message}; }), history: attempt_log};
  if (rejected !== null) {
    data.scxml = rejected;
    data.path = draftPath();
  }
  return data;
}
