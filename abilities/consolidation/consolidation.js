// The ECMAScript of abilities/consolidation/consolidation.scx (SCXML 5.8):
// loaded by its root <script src="consolidation.js">, so every function here
// is a global of the document's data model, callable from its cond and expr
// attributes.

// consolidating

function isAbsent() {
  return memory === null;
}

function stepDelay() {
  return step_timeout_ms + 'ms';
}

// The pause before each step: the bounds' consolidation_pause_ms, else
// this document's default.
function pauseDelay() {
  var bound = Object(bounds).consolidation_pause_ms;
  return (typeof bound === 'number' ? bound : step_pause_ms) + 'ms';
}

function timeoutFailure() {
  return 'Consolidation step did not finish within ' + step_timeout_ms / 1000 + 's.';
}

function errorFailure(_event) {
  return 'Consolidation could not run (' + _event.name + ')';
}

function isBounded() {
  return steps >= max_steps;
}

function stepsAfter() {
  return steps + 1;
}

// The rows of a memory result's statement, each its values.
function resultRows(_event, index) {
  return _event.data.results[index].rows.map(function (row) { return row.values; });
}

function hasNoRows() {
  return rows.length === 0;
}

// folding: rows are [person_id, centroid, weight, through_seq, seq, embedding].

function foldReadStatements() {
  return [{sql: fold_select, parameters: [kind, fold_batch]}];
}

function foldCentroid() {
  return rows[0][1];
}

function foldWeight() {
  return Number(rows[0][2]);
}

function foldVoices() {
  return rows.map(function (row) { return row[5]; });
}

// The representative's new state, guarded by the through_seq read.
function foldWriteStatements() {
  var person = rows[0];
  var through = Number(rows[rows.length - 1][4]);
  var parameters = [fold.centroid, fold.weight, through, person[0], kind, Number(person[3])];
  return [{sql: fold_update, parameters: parameters, rowcount: 1}];
}

function foldedAfter() {
  return folded + fold.folded;
}

// merging: rows are [person_id, centroid, weight, through_seq].

function peopleReadStatements() {
  return [{sql: people_select, parameters: [kind]}];
}

function peopleRows() {
  return rows.map(function (row) { return [row[0], row[1], Number(row[2])]; });
}

function hasNoMerge() {
  return merge === null;
}

// A LIKE pattern for a JSON text holding the ref as a whole string.
function quotedPattern(ref) {
  return '%' + quoted(ref) + '%';
}

function quoted(ref) {
  return '"' + ref + '"';
}

function refsReadStatements() {
  return [{sql: refs_select, parameters: [quotedPattern(merge.from), merge_refs_limit]}];
}

function contextRefs(_event) {
  return resultRows(_event, 0).map(function (row) { return row[0]; });
}

// The read row of a person.
function rowOf(person) {
  return rows.filter(function (row) { return row[0] === person; })[0];
}

// The merge, in one transaction: the older's representative (guarded),
// then everything keyed on the newer re-pointed, then the newer deleted
// (guarded by no context reference naming them left).
function mergeWriteStatements() {
  var into = rowOf(merge.into), from = rowOf(merge.from);
  var through = Math.max(Number(into[3]), Number(from[3]));
  var guard = [merge.centroid, merge.weight, through, merge.into, kind, Number(into[2]), Number(into[3]), merge.from, Number(from[2]), Number(from[3])];
  var swap = [quoted(merge.from), quoted(merge.into), quotedPattern(merge.from)];
  var statements = [
    {sql: merge_centroid, parameters: guard, rowcount: 1},
    {sql: merge_voices, parameters: [merge.into, merge.from]},
    {sql: merge_drop_centroids, parameters: [merge.from]},
    {sql: merge_speakers, parameters: swap},
    {sql: merge_records, parameters: swap},
    {sql: merge_record_sources, parameters: [merge.into, merge.from]},
    {sql: merge_record_targets, parameters: [merge.into, merge.from]},
    {sql: merge_contents, parameters: swap},
    {sql: merge_subjects, parameters: [merge.into, merge.from]},
    {sql: merge_context, parameters: [merge.into, merge.from]}
  ];
  return statements.concat(refs.map(contextStatement), [{sql: merge_person, parameters: [merge.from, quotedPattern(merge.from)], rowcount: 1}]);
}

function contextStatement(pair) {
  return {sql: merge_context, parameters: [pair[1], pair[0]]};
}

function mergedAfter() {
  return merged + 1;
}

// memory

// Whether the event answers the request the state `name` last sent the
// memory (decision D262): its id is that request's.
function isMemoryReply(name) {
  return memory_requests[name] !== undefined && _event.data.id === memory_requests[name];
}
