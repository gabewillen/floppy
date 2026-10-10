// The ECMAScript of abilities/identity/identity.scx (SCXML 5.8): loaded by its root
// <script src="identity.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared by recalling and adopting.

function hasNoDatabase() {
  return database === null;
}

// The queue with the event appended, as {event, data}.
function deferredQueue(_event) {
  return queue.concat([{event: _event.name, data: _event.data}]);
}

// recalling

// The name the session started with, as a fact, or null.
function recalledName() {
  return typeof name === 'string' && name.length > 0 ? {text: name, kind: 'fact'} : null;
}

// mosfet identity/name.py: the adopted names, oldest first.
function nameSelectStatements() {
  return [{sql: name_select, parameters: ['self_name', 'self']}];
}

// The last adopted name, else the session's.
function recalledNameFromMemory(_event) {
  return _event.data.results[0].rows.length > 0 ? JSON.parse(_event.data.results[0].rows.at(-1).values[0]) : recalled_name;
}

function recallFailedMessage(_event) {
  return 'Identity could not recall a name from memory: ' + _event.data.message;
}

function recalledNameText(_event) {
  return _event.data.name ? _event.data.name.text : null;
}

function recalledNameLog() {
  return 'recalled ' + JSON.stringify(name);
}

// listening

// mosfet Identity._can_recognize: a nameless bot only hears words.
function canRecognize() {
  return name !== null;
}

// The text a communication input carries: string content of a text/* type, else null.
function heardText(data) {
  return data.content_type.indexOf('text/') === 0 ? data.content : null;
}

// recognizing

function addressedLog() {
  return 'addressed as ' + name;
}

// adopting

function adoptedName() {
  return ({text: adopting_name, kind: 'fact'});
}

// mosfet identity/name.py: the insert's parameters ?1 to ?6.
function nameInsertParameters() {
  return ['long_term', 'self', 'fact', 'standard', 'retain', JSON.stringify({text: adopting_name, kind: 'fact'})];
}

// The insert's parameters ?7 and ?8 appended.
function nameInsertParametersCompleted() {
  return name_parameters.concat(['application/json', 'self_name']);
}

function nameInsertStatements() {
  return [{sql: name_insert, parameters: name_parameters}];
}

function adoptFailedMessage(_event) {
  return 'Identity could not remember the name it took: ' + _event.data.message;
}

function adoptedNameLog() {
  return 'adopted ' + JSON.stringify(name);
}
