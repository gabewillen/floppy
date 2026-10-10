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
  return _event.data !== null && typeof _event.data === 'object' && !Array.isArray(_event.data) && Object.keys(_event.data).every(function (k) { return ['spec', 'constraints', 'max_attempts', 'model', 'generate_timeout_ms'].indexOf(k) >= 0; }) && typeof _event.data.spec === 'string' && _event.data.spec.trim().length > 0 && (_event.data.constraints === undefined || (Array.isArray(_event.data.constraints) && _event.data.constraints.length <= 32 && _event.data.constraints.every(function (c) { return typeof c === 'string'; }))) && (_event.data.max_attempts === undefined || (Number.isInteger(_event.data.max_attempts) && _event.data.max_attempts >= 1 && _event.data.max_attempts <= 8)) && (_event.data.model === undefined || (typeof _event.data.model === 'string' && _event.data.model.length > 0)) && (_event.data.generate_timeout_ms === undefined || (Number.isInteger(_event.data.generate_timeout_ms) && _event.data.generate_timeout_ms >= 1 && _event.data.generate_timeout_ms <= 3600000));
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

function invalidRequestDiagnostics() {
  return [{kind: 'request', code: 'invalid-request', message: 'the builder.request data does not satisfy schemas/builder.request.json'}];
}

// design and author: the generate service's calls.

function isUnconfigured(_event) {
  return _event.invokeid === gen_id && gen_failure === 'unconfigured';
}

function unconfiguredDiagnostics() {
  return [{kind: 'generate', code: 'unconfigured', message: 'the generate service has no backend: the run holds no FLOPPY_BASE_URL or no FLOPPY_API_KEY (both are required; set them in the --env-file)'}];
}

// stage is design or author; product is the plan or draft it generates.
function generationFailedDiagnostics(stage, product) {
  return [{kind: 'generate', code: stage + '-failed', message: 'the ' + stage + ' generation failed (' + (gen_failure === null ? 'it ended without an event' : gen_failure) + '): no ' + product + ' within the ' + product + ' schema'}];
}

function generationTimeoutDiagnostics(stage) {
  return [{kind: 'timeout', code: stage + '-timeout', message: 'the ' + stage + ' generation did not answer within ' + generate_timeout_ms + ' ms and was cancelled'}];
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

function planGenerated(_event) {
  return _event.invokeid === gen_id && plan !== null;
}

// author

function authorLog() {
  return mode + ' draft, attempt ' + attempt;
}

function previousTests() {
  return draft === null ? null : draft.tests;
}

function authorPrompt() {
  return JSON.stringify({task: mode, spec: spec, constraints: constraints, plan: plan, previous_scxml: mode === 'repair' ? scxml_text : null, previous_tests: mode === 'repair' ? previous_tests : null, diagnostics: diagnostics});
}

function draftGenerated(_event) {
  return _event.invokeid === gen_id && draft !== null;
}

// validate

// The JSON value of a schemas entry's text, or null when it does not parse.
function parseSchemaText(text) {
  var value = null;
  try { value = JSON.parse(text); } catch (e) { value = null; }
  return value;
}

function isSchemaValue(value) {
  return !(value === null || (typeof value !== 'object' && typeof value !== 'boolean') || Array.isArray(value));
}

function schemaErrors(schemas) {
  var errors = [];
  schemas.forEach(function (s) {
    if (!isSchemaValue(parseSchemaText(s.schema))) {
      errors.push({kind: 'draft', code: 'schema-not-json', message: 'the schemas entry of ' + JSON.stringify(s.event) + ' is not a JSON Schema object as JSON text: ' + JSON.stringify(String(s.schema).slice(0, 200))});
    }
  });
  return errors;
}

function parsedSchemas(schemas) {
  return schemas.map(function (s) { return {event: s.event, schema: parseSchemaText(s.schema)}; });
}

function hasSchemaErrors() {
  return report !== null && schema_errors.length > 0;
}

function schemaErrorDiagnostics() {
  return schema_errors.slice(0, 20);
}

function isValidatorClean() {
  return report !== null && report.validator.counts.errors === 0;
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

function hasMachineApplicableFixes() {
  return report !== null && typeof fixed === 'string' && fixed !== scxml_text && fix_passes < 2;
}

function isValidated() {
  return report !== null;
}

function validateDiagnostics() {
  return report.validator.diagnostics.filter(function (d) { return d.severity === 'error'; }).slice(0, 20).map(function (d) { return {kind: 'validate', code: d.code, label: d.primary.message ? d.primary.message : d.message, message: d.help ? d.message + ' Help: ' + d.help : d.message}; });
}

function validatorFailedDiagnostics() {
  return [{kind: 'draft', code: 'validator-failed', message: 'the validator could not check the draft (an oversized or unreadable document)'}];
}

// simulate

function simulateLog() {
  return draft.tests.length + ' test script(s)';
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
  return sim_index >= draft.tests.length;
}

// sim_run

function currentTest() {
  return draft.tests[sim_index];
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

// sim_check

// sim_results with the current test's result: expect_events in order
// (other outputs may come between), and expect_done.
function nextSimResults() {
  return sim_results.concat([(function (t) { var next = 0; observed.forEach(function (name) { if (next < t.expect_events.length && name === t.expect_events[next]) { next += 1; } }); return {name: t.name, invokeid: candidate_id, passed: sim_error === null && next === t.expect_events.length && child_done === t.expect_done, observed: observed, expected: t.expect_events, done: child_done, expect_done: t.expect_done, error: sim_error}; })(currentTest())]);
}

// sim_verdict

function allTestsPassed() {
  return sim_results.every(function (r) { return r.passed; });
}

function simulateDiagnostics() {
  return sim_results.filter(function (r) { return !r.passed; }).map(function (r) { return {kind: 'simulate', code: 'test-failed', message: 'test ' + r.name + ' expected the outputs ' + JSON.stringify(r.expected) + ' in order and done=' + r.expect_done + ', observed ' + JSON.stringify(r.observed) + ' and done=' + r.done + (r.error === null ? '' : ', error ' + r.error)}; });
}

// retry_gate

// The sorted, distinct kind|code|identity of the diagnostics.
function diagnosticsFingerprint() {
  return JSON.stringify(diagnostics.map(function (d) { return d.kind + '|' + d.code + '|' + (d.kind === 'validate' ? d.label : d.message); }).sort().filter(function (key, i, keys) { return i === 0 || key !== keys[i - 1]; }));
}

function nextAttemptLog() {
  return attempt_log.concat([{failure: failures, attempt: attempt, diagnostics: diagnostics.slice(0, 5)}]);
}

function retryGateLog() {
  return 'failure ' + failures + (max_attempts === null ? '' : ' of ' + max_attempts) + firstDiagnosticSummary();
}

function isNoProgress() {
  return fingerprint === previous_fingerprint;
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
  return {scxml: scxml_text, schemas: parsed_schemas, tests: draft.tests, report: {attempts: attempt, failures: failures, fixes_applied: fixes_applied, warnings: warnings, plan: plan, decisions: decisions, history: attempt_log, simulation: sim_results}};
}

// failed

function failedLog() {
  return failure_reason + firstDiagnosticSummary();
}

function failedData() {
  return {reason: failure_reason, diagnostics: diagnostics.map(function (d) { return {kind: d.kind, code: d.code, message: d.message}; }), history: attempt_log};
}
