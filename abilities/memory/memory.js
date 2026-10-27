// The ECMAScript of abilities/memory/memory.scx (SCXML 5.8): loaded by its root
// <script src="memory.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// opening

// A Postgres connection URI; anything else is a SQLite database file.
function isPostgresDatabase() {
  return /^postgres(ql)?:\/\//.test(database);
}

// migrating

// floppy's revisions in the database engine's dialect, oldest first.
function engineRevisions() {
  return postgres ? [postgresql_0001, postgresql_0002] : [sqlite_0001, sqlite_0002];
}

// idle

// The request to apply: its id, its sender (the reply's target) and its
// statements.
function requestOf(_event) {
  return { id: _event.data.id, origin: _event.origin, statements: _event.data.statements };
}

// applying

function operationTimeoutDelay() {
  return operation_timeout_ms + 'ms';
}

// The timeout failure's message, the bound in seconds.
function timeoutFailure() {
  return 'Memory did not finish within ' + operation_timeout_ms / 1000 + 's.';
}

// migrating and applying

// The failure of a migration or a service that could not run.

function couldNotRunFailure(_event) {
  return 'Memory could not run (' + _event.name + ')';
}
