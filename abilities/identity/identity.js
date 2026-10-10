// The ECMAScript of abilities/identity/identity.scx (SCXML 5.8): loaded by its root
// <script src="identity.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// recalling

// The name the session started with, as a fact, or null.
function recalledName() {
  return typeof name === 'string' && name.length > 0 ? {text: name, kind: 'fact'} : null;
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

// recognizing

function addressedLog() {
  return 'addressed as ' + name;
}

// adopting

function adoptedName() {
  return ({text: adopting_name, kind: 'fact'});
}

function adoptedNameLog() {
  return 'adopted ' + JSON.stringify(name) + ' (persisted: false)';
}
