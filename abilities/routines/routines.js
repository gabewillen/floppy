// The ECMAScript of abilities/routines/routines.scx (SCXML 5.8): loaded by its root
// <script src="routines.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared by recalling and chain_on.

function isFromInvoke(event, invokeid) {
  return event.invokeid === invokeid;
}

// reconciling

// database null means no inventory: nothing runs.
function hasNoMemory() {
  return database === null;
}

// recalling

// mosfet select_active_behaviors_clauses(lifetime=persistent).
function routinesSelectStatements() {
  return [{sql: select_routines, parameters: ['ACTIVE', 'persistent']}];
}

// The recalled rows, from the one JSON array the select returns.
function routinesFromMemoryOutput(event) {
  return JSON.parse(event.data.results[0].rows[0].values[0]);
}

// mosfet inventory.revision: the recalled rows differ from the running set.
function isRevisionChanged() {
  return JSON.stringify(desired) !== JSON.stringify(routines);
}

function reconcileFailedLog(detail) {
  return 'Routine inventory load failed: ' + detail;
}

// chain

// The chain restarts on a running set that is not empty.
function restartsOnRoutines() {
  return restart && routines.length > 0;
}

function routineNames() {
  return routines.map(function (r) { return r.name; }).join(' ');
}
