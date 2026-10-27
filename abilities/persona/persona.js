// The ECMAScript of abilities/persona/persona.scx (SCXML 5.8): loaded by its root
// <script src="persona.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared by recalling and revising.

function hasNoMemory() {
  return memory === null;
}

function delay(ms) {
  return ms + 'ms';
}

// The white space around each trait is stripped; what is left is judged by bot.ability.persona.output's schema when
// it is told. Anything but a list of strings is kept as given, for the schema
// to refuse.
function ownerTraits() {
  return Array.isArray(traits) ? traits.map(function (trait) { return typeof trait === 'string' ? trait.trim() : trait; }) : traits;
}

// recalling

// The 50 most recent descriptions, oldest first.
function descriptionSelectStatements() {
  return [{sql: description_select, parameters: ['self_persona', 'self']}];
}

// A string of `min` to `max` characters (code points) that is not blank.
function isText(value, min, max) {
  return typeof value === 'string' && Array.from(value).length >= min && Array.from(value).length <= max && /\S/.test(value);
}

// One stored row's content as a description ({text, reason, traits,
// kind}), or null when it does not decode as one (the recall skips such a
// row, so one damaged row never costs the bot the description written before it).
// This decodes a stored row, which no event:schema describes; the recalled
// event's schema judges the result again.
function storedDescription(content) {
  var value;
  try {
    value = JSON.parse(content);
  } catch (error) {
    return null;
  }
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    return null;
  }
  var keys = Object.keys(value);
  if (!keys.every(function (key) { return ['text', 'reason', 'traits', 'kind'].indexOf(key) >= 0; })) {
    return null;
  }
  if (!isText(value.text, 1, 2000) || !isText(value.reason, 1, 500)) {
    return null;
  }
  if (value.kind !== undefined && value.kind !== 'fact') {
    return null;
  }
  if (!Array.isArray(value.traits) || value.traits.length < 1 || value.traits.length > 24) {
    return null;
  }
  var given = value.traits.map(function (trait) { return typeof trait === 'string' ? trait.trim() : null; });
  if (!given.every(function (trait) { return trait !== null && isText(trait, 1, 200); })) {
    return null;
  }
  return {text: value.text, reason: value.reason, traits: given, kind: 'fact'};
}

// The most recent description the select returned, else none.
function latestDescription(_event) {
  return _event.data.results[0].rows.reduce(function (latest, row) { return storedDescription(row.values[0]) || latest; }, null);
}

function recallFailedMessage(_event) {
  return 'Persona could not recall a description from memory: ' + _event.data.message;
}

function recalledLog() {
  return 'recalled ' + JSON.stringify(description === null ? null : description.text);
}

// revising

// The revision: the selected text and reason, under the owner's traits.
function revisedDescription() {
  return ({text: revision.text, reason: revision.reason, traits: traits, kind: 'fact'});
}

// The self-fact insert's parameters ?1 to ?6.
function descriptionInsertParameters() {
  return ['long_term', 'self', 'fact', 'standard', 'retain', JSON.stringify(revisedDescription())];
}

// The insert's parameters ?7 and ?8 appended.
function descriptionInsertParametersCompleted() {
  return description_parameters.concat(['application/json', 'self_persona']);
}

function descriptionInsertStatements() {
  return [{sql: description_insert, parameters: description_parameters}];
}

function reviseFailedMessage(_event) {
  return 'Persona could not remember its revision: ' + _event.data.message;
}

function revisedLog() {
  return 'revised ' + JSON.stringify(description.text);
}

// failed

function failedMessage(_event) {
  return 'Persona could not tell what it knows of itself (' + _event.name + '), as when its traits are not 1 to 24 traits of at most 200 characters.';
}

// memory

// Whether the event answers the request the state `name` last sent the
// memory (decision D265): its id is that request's.
function isMemoryReply(name) {
  return memory_requests[name] !== undefined && _event.data.id === memory_requests[name];
}
