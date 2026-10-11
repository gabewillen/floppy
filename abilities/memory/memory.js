// The ECMAScript of abilities/memory/memory.scx (SCXML 5.8): loaded by its root
// <script src="memory.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// opening

// A Postgres connection URI; anything else is a SQLite database file.
function isPostgresDatabase() {
  return /^postgres(ql)?:\/\//.test(database);
}

// migrating

// The original's revisions in the database engine's dialect, oldest first.
function engineRevisions() {
  return postgres ? [postgresql_0001, postgresql_0002] : [sqlite_0001, sqlite_0002];
}

// migrating and applying

function couldNotRunFailure(_event) {
  return 'Memory could not run (' + _event.name + ')';
}
