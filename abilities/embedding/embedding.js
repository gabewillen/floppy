// The ECMAScript of abilities/embedding/embedding.scx (SCXML 5.8): loaded by its root
// <script src="embedding.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// idle

// The request, with its sender, the address its reply goes to.
function inputOf(_event) {
  return ({id: _event.data.id, origin: _event.origin, role: _event.data.role, items: _event.data.items});
}

// embedding

// An event of the generate call in hand, not of a cancelled one.
function isCurrent(_event) {
  return _event.invokeid === generate;
}

function timeoutDelay() {
  return timeout_ms + 'ms';
}

// Each item's text: its parts joined, with the role's task prefix applied
// once (mosfet.py's Instructions.render).
function texts() {
  return input.items.map(function (item) {
    var text = item.parts.map(function (part) { return part.text; }).join('');
    var prefix = input.role === 'query' ? instructions.query : instructions.document.replace('{title}', typeof item.title === 'string' ? item.title : instructions.untitled);
    return prefix + text;
  });
}

// The vectors of a generate.result, one per item, else null.
function resultVectors(_event) {
  var given = Object(_event.data).vectors;
  return Array.isArray(given) && given.length === input.items.length ? given : null;
}

function hasVectors() {
  return vectors !== null;
}

// A generate.failed's code as a failure kind.
function generateFailure(_event) {
  var code = Object(_event.data).code;
  var kind = code === 'unconfigured' || code === 'transport' ? 'unavailable' : code === 'malformed' ? 'invalid_response' : 'embed_failed';
  return ({kind: kind, message: 'The embedder failed (' + code + ').'});
}

function output() {
  return ({id: input.id, space: {model: model, dimensions: dimensions}, role: input.role, vectors: vectors.map(function (values) { return {values: values}; })});
}

function failed() {
  return failure !== null ? ({id: input.id, kind: failure.kind, message: failure.message}) : ({id: input.id, kind: 'invalid_response', message: 'The embedder answered no vector per item.'});
}

function timedOut() {
  return ({id: input.id, kind: 'timeout', message: 'The embedder did not answer within ' + timeout_ms / 1000 + 's.'});
}

function couldNotRun(_event) {
  return ({id: input.id, kind: 'invalid_input', message: 'The embedder could not run (' + _event.name + ').'});
}
