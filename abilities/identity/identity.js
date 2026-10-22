// The ECMAScript of abilities/identity/identity.scx (SCXML 5.8): loaded by its root
// <script src="identity.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared by recalling and adopting.

function hasNoDatabase() {
  return database === null;
}

function delay(ms) {
  return ms + 'ms';
}

// recalling

// The name the session started with, as a fact, or null.
function recalledName() {
  return typeof name === 'string' && name.length > 0 ? {text: name, kind: 'fact'} : null;
}

// The original identity/name.py: the adopted names, oldest first.
function nameSelectStatements() {
  return [{sql: name_select, parameters: ['self_name', 'self']}];
}

// One stored row's content as the original's Name (identity/name.py: text,
// and kind, "fact" when absent; no other key), or null when it does not
// decode as one, so a damaged row never costs the bot the name adopted
// before it. A text over 64 characters, which no adoption writes, is
// skipped too: bot.ability.identity.named could not tell it.
function storedName(content) {
  var value;
  try {
    value = JSON.parse(content);
  } catch (error) {
    return null;
  }
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    return null;
  }
  if (!Object.keys(value).every(function (key) { return key === 'text' || key === 'kind'; })) {
    return null;
  }
  if (typeof value.text !== 'string' || Array.from(value.text).length < 1 || Array.from(value.text).length > 64) {
    return null;
  }
  if (value.kind !== undefined && value.kind !== 'fact') {
    return null;
  }
  return {text: value.text, kind: 'fact'};
}

// The last adopted name that decodes, else the session's.
function recalledNameFromMemory(_event) {
  return _event.data.results[0].rows.reduce(function (latest, row) { return storedName(row.values[0]) || latest; }, recalled_name);
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

// The original Identity._can_recognize: a nameless bot only hears words.
function canRecognize() {
  return name !== null;
}

// The text a communication input carries: string content of a text/* type, else null.
function heardText(data) {
  return data.content_type.indexOf('text/') === 0 ? data.content : null;
}

// recognizing

// The speech decoding answered with a transcript (the original's decoded text).
function hasTranscript(_event) {
  return typeof Object(_event.data).text === 'string';
}

// The original _run_recognition: the recognizer's error, as the stage failure.
function recognitionFailedMessage(_event) {
  return typeof Object(_event.data).message === 'string' ? _event.data.message : 'Identity recognizer produced output that does not match its output schema.';
}

function recognitionErrorMessage(_event) {
  return 'Identity recognition could not run (' + _event.name + ').';
}

function addressedLog() {
  return 'addressed as ' + name;
}

// adopting

function adoptedName() {
  return ({text: adopting_name, kind: 'fact'});
}

// The original identity/name.py: the insert's parameters ?1 to ?6.
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
