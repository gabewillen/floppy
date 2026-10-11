// The ECMAScript of abilities/reading/reading.scx (SCXML 5.8): loaded by its root
// <script src="reading.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Focused

function stageFailureLog(_event) {
  return _event.data.stage + ': ' + _event.data.message;
}

// Applying and DecodingImage

function stageTimeoutDelay() {
  return stage_timeout_ms + 'ms';
}

// Applying

function classificationTimeoutMessage() {
  return 'the classification timed out after ' + stage_timeout_ms + ' ms';
}

function classificationCouldNotRunMessage(_event) {
  return 'the classification could not run (' + _event.name + ')';
}

// Classified

function classifiedAsText() {
  return classified.classification.kind === 'text' && classified.input.kind === 'text';
}

function classifiedAsImage() {
  return classified.classification.kind === 'image' && classified.input.kind === 'image';
}

function classifiedAsUnreadable() {
  return classified.classification.kind === 'unreadable';
}

// DecodingImage

function imageDecodingTimeoutMessage() {
  return 'the image decoding timed out after ' + stage_timeout_ms + ' ms';
}

// One user message: the prompt as a text part, the input's reference as an image part.
function imageDecodingMessages() {
  return [{role: 'user', content: [{kind: 'text', content: prompt}, {kind: 'image', content: classified.input.content}]}];
}

function isGenerate(_event) {
  return _event.invokeid === gen_id;
}

function isGenerateTextResult(_event) {
  return isGenerate(_event) && typeof Object(_event.data).text === 'string';
}

function decodedImageText(_event) {
  return _event.data.text.trim();
}

function imageDecodingFailedMessage(_event) {
  return typeof Object(_event.data).code === 'string' ? 'Reading image decoder failed: ' + _event.data.code + '.' : 'Reading image decoder failed.';
}

function imageDecodingCouldNotRunMessage(_event) {
  return 'the image decoding could not run (' + _event.name + ')';
}
