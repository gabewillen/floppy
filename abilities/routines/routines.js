// The ECMAScript of abilities/routines/routines.scx (SCXML 5.8): loaded by its root
// <script src="routines.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// reconciling

// memory null means no inventory: nothing runs.
function hasNoMemory() {
  return memory === null;
}

function hasMemory() {
  return memory !== null;
}

// recalling

// One statement: the ACTIVE persistent behaviors, in inventory order.
function routinesSelectStatements() {
  return [{sql: select_routines, parameters: ['ACTIVE', 'persistent']}];
}

// The recalled rows, as the routines the chain runs.
function routinesFromMemoryOutput(event) {
  return event.data.results[0].rows.map(function (r) { return {name: r.values[0], source: r.values[1], updated_at: r.values[2]}; });
}

// The recalled rows differ from the running set.
function isRevisionChanged() {
  return JSON.stringify(desired) !== JSON.stringify(routines);
}

// running

// The first deferred tick's routine is in the running set.
function hasRunningTick() {
  return ticks.length > 0 && routines.some(function (r) { return r.name === ticks[0].name; });
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

// memory

// Whether the event answers the request the state `name` last sent the
// memory (decision D266): its id is that request's.
function isMemoryReply(name) {
  return memory_requests[name] !== undefined && _event.data.id === memory_requests[name];
}
