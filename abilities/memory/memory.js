// The ECMAScript of abilities/memory/memory.scx (SCXML 5.8): loaded by its root
// <script src="memory.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// opening

// A Postgres connection URI; anything else is a SQLite database file.
function isPostgresDatabase() {
  return /^postgres(ql)?:\/\//.test(database);
}

// migrating_sqlite

function userVersionStatements() {
  return [{sql: 'PRAGMA user_version'}];
}

function isReadingSqlite(_event) {
  return _event.invokeid === 'reading';
}

function userVersion(_event) {
  return _event.data.results[0].rows[0].values[0];
}

// The statements of each revision the database's version lacks.
function missingRevisions() {
  return version < 1 ? revision_0001.concat(revision_0002) : version < 2 ? revision_0002 : [];
}

// migrating_sqlite and migrating_postgres

function migrationStatements() {
  return migrations.map(function (sql) { return ({sql: sql}); });
}

// migrating_postgres

function isReadingPostgres(_event) {
  return _event.invokeid === 'reading_postgres';
}

// The alembic_version row's revision as a number (0 when it has none).
function alembicVersion(_event) {
  return _event.data.results[1].rows.length ? parseInt(_event.data.results[1].rows[0].values[0], 10) : 0;
}

// The Postgres statements of each revision the database's version lacks.
function missingPostgresRevisions() {
  return version < 1 ? revision_0001_postgres.concat(revision_0002_postgres) : version < 2 ? revision_0002_postgres : [];
}

// migrating

// Lost a race to create Postgres's version table: retry once from opening.
function canRetryReading(_event) {
  return isReadingPostgres(_event) && !retried;
}

// Either engine's version read.
function isReading(_event) {
  return _event.invokeid.startsWith('reading');
}

// migrating and applying

function couldNotRunFailure(_event) {
  return 'Memory could not run (' + _event.name + ')';
}

// applying

// A version that is not a number from 0 to head is refused untouched.
function isUnknownVersion() {
  return !(version >= 0 && version <= head);
}

// The migrations, then the statements, in one transaction.
function appliedStatements() {
  return migrations.concat(statements);
}

// Either engine's apply.
function isApply(_event) {
  return _event.invokeid.startsWith('apply');
}

// One result per statement, without the migrations'.
function statementResults(_event) {
  return _event.data.results.slice(migrations.length);
}

// Lost a race to migrate a fresh database: retry once from opening.
function canRetryMigration(_event) {
  return isApply(_event) && migrations.length > 0 && !retried;
}
