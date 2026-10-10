// The ECMAScript of abilities/memory/memory.scx (SCXML 5.8): loaded by its root
// <script src="memory.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// migrating

function userVersionStatements() {
  return [{sql: 'PRAGMA user_version'}];
}

function isReading(_event) {
  return _event.invokeid === 'reading';
}

function userVersion(_event) {
  return _event.data.results[0].rows[0].values[0];
}

// The statements of each revision the database's version lacks.
function missingRevisions() {
  return version < 1 ? revision_0001.concat(revision_0002) : version < 2 ? revision_0002 : [];
}

function migrationStatements() {
  return migrations.map(function (sql) { return ({sql: sql}); });
}

// migrating and applying

function couldNotRunFailure(_event) {
  return 'Memory could not run (' + _event.name + ')';
}

// applying

// A version newer than head (or below 0) is refused untouched.
function isUnknownVersion() {
  return version > head || version < 0;
}

// The migrations, then the statements, in one transaction.
function appliedStatements() {
  return migrations.concat(statements);
}

function isApply(_event) {
  return _event.invokeid === 'apply';
}

// One result per statement, without the migrations'.
function statementResults(_event) {
  return _event.data.results.slice(migrations.length);
}

// Lost a race to migrate a fresh database: retry once from migrating.
function canRetryMigration(_event) {
  return isApply(_event) && migrations.length > 0 && !retried;
}
