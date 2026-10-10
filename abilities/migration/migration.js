// The ECMAScript of abilities/migration/migration.scx (SCXML 5.8): loaded by
// its root <script src="migration.js">, so every function here is a global of
// the document's data model, callable from its cond and expr attributes.

// reading

// A Postgres connection URI; anything else is a SQLite database file.
function isPostgresDatabase() {
  return /^postgres(ql)?:\/\//.test(database);
}

// Alembic's version table, created when missing, and its row.
function versionStatements() {
  return [{sql: version_table}, {sql: version_select}];
}

// The revision alembic_version holds (null: base).
function currentRevision(_event) {
  var rows = _event.data.results[1].rows;
  return rows.length > 0 ? rows[0].values[0] : null;
}

// reading and applying

function couldNotRunFailure(_event) {
  return 'Migration could not run (' + _event.name + ')';
}

// planning

function couldNotPlanFailure() {
  return 'Migration could not plan from ' + current + ' to ' + target;
}

function isPlanned() {
  return plan !== null;
}

// At the target already: nothing to apply.
function isAtTarget() {
  return isPlanned() && plan.steps.length === 0;
}

// applying

// The version row's guard, moving it from the revision read to the target:
// it must touch exactly one row.
function versionGuard() {
  if (plan.from === null) {
    return {sql: version_insert, parameters: [plan.to], rowcount: 1};
  }
  if (plan.to === null) {
    return {sql: version_delete, parameters: [plan.from], rowcount: 1};
  }
  return {sql: version_update, parameters: [plan.to, plan.from], rowcount: 1};
}

// The guard, then every step's statements, in order.
function appliedStatements() {
  return [guard].concat(plan.steps.flatMap(function (step) { return step.statements; }));
}

// Another migration moved the row first, or holds the database: read again,
// once.
function canRetry(_event) {
  return retryable.includes(_event.data.code) && !retried;
}
