// The ECMAScript of abilities/description/description.scx (SCXML 5.8): loaded by its root
// <script src="description.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// description: a step that cannot run.

function couldNotRunMessage(_event) {
  return 'Description could not run (' + _event.name + ')';
}

// applying

// One user message: the instructions as a text part, the image's reference as an image part.
function descriptionMessages() {
  return [{role: 'user', content: [{kind: 'text', content: instructions}, {kind: 'image', content: image}]}];
}

// A generate result with its text (D41: the service's events are unchecked).
function isGenerateTextResult(_event) {
  return typeof Object(_event.data).content === 'string';
}

// The reply, trimmed.
function saidText(_event) {
  return _event.data.content.trim();
}

function generatorFailedMessage(_event) {
  return typeof Object(_event.data).code === 'string' ? 'Description generator failed: ' + _event.data.code + '.' : 'Description generator failed.';
}

// replied

function isEmptyDescription() {
  return said === '';
}

function mismatchedMessage() {
  return 'Description produced output that does not match its output schema.';
}
