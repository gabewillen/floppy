// The ECMAScript of abilities/object_detection/object_detection.scx (SCXML 5.8): loaded by its root
// <script src="object_detection.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// detection: a step that cannot run.

function couldNotRunMessage(_event) {
  return 'ObjectDetection could not run (' + _event.name + ')';
}

// applying

// One user message: the instructions as a text part, the image's reference as an image part.
function detectionMessages() {
  return [{role: 'user', content: [{kind: 'text', content: instructions}, {kind: 'image', content: image}]}];
}

// A generate result with its text (D41: the service's events are unchecked).
function isGenerateTextResult(_event) {
  return typeof Object(_event.data).content === 'string';
}

function generatorFailedMessage(_event) {
  return typeof Object(_event.data).code === 'string' ? 'Object detection generator failed: ' + _event.data.code + '.' : 'Object detection generator failed.';
}

// replied: the original's GenerativeDetector.

// {objects, skipped} (the valid items, at most max_objects, most confident
// first, and how many were not a DetectedObject), or {message} (the
// original's detection.Error) when the reply cannot be read at all.
function detectedReply() {
  var size = utf8Bytes(reply);
  if (size > max_reply_bytes) {
    return {message: 'the reply of ' + size + ' bytes exceeds max_reply_bytes (' + max_reply_bytes + ')'};
  }
  var answer = reply.trim();
  if (answer.startsWith('```')) {
    // A model may fence its JSON as markdown: read what is inside the fence.
    var newline = answer.indexOf('\n');
    var inside = newline < 0 ? '' : answer.slice(newline + 1);
    answer = (inside.endsWith('```') ? inside.slice(0, -3) : inside).trim();
  }
  var items = null;
  try {
    items = JSON.parse(answer);
  } catch (error) {
    items = null;
  }
  if (!Array.isArray(items)) {
    // The reply is model output: it never becomes the failure message.
    return {message: 'the reply is not a JSON array of objects'};
  }
  var found = [];
  var skipped = 0;
  for (var item of items) {
    var object = detectedObject(item);
    if (object === null) {
      skipped += 1;
    } else {
      found.push(object);
    }
  }
  return {objects: mostConfidentFirst(found.slice(0, max_objects)), skipped: skipped};
}

function hasSkippedItems() {
  return detected.skipped > 0;
}

function hasObjects() {
  return Array.isArray(detected.objects);
}

function mismatchedMessage() {
  return 'Object detection produced output that does not match its output schema.';
}

// The reply's size in UTF-8 bytes.
function utf8Bytes(text) {
  var bytes = 0;
  for (var character of text) {
    var point = character.codePointAt(0);
    bytes += point < 0x80 ? 1 : point < 0x800 ? 2 : point < 0x10000 ? 3 : 4;
  }
  return bytes;
}

function isRecord(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

// Exactly these keys: no other key, none missing.
function hasExactly(record, keys) {
  var own = Object.keys(record);
  return own.length === keys.length && keys.every(function (key) { return own.indexOf(key) >= 0; });
}

// A JSON number in [low, high] (a boolean or a numeric string is not one).
function isNumberIn(value, low, high) {
  return typeof value === 'number' && value >= low && value <= high;
}

// The original's geometry.Box, or null: x and y in 0..1, width and height in
// (0, 1], inside the image up to the original's 1e-6 slack.
function detectedBox(value) {
  if (!isRecord(value) || !hasExactly(value, ['x', 'y', 'width', 'height'])) {
    return null;
  }
  var x = value.x;
  var y = value.y;
  var width = value.width;
  var height = value.height;
  if (!(isNumberIn(x, 0, 1) && isNumberIn(y, 0, 1) && isNumberIn(width, 0, 1) && isNumberIn(height, 0, 1))) {
    return null;
  }
  if (width === 0 || height === 0 || x + width > 1 + 1e-6 || y + height > 1 + 1e-6) {
    return null;
  }
  return {x: x, y: y, width: width, height: height};
}

// The original's DetectedObject, or null: label a string of 1..100
// characters, confidence 0..1 or null, and a box.
function detectedObject(item) {
  if (!isRecord(item) || !hasExactly(item, ['label', 'confidence', 'box'])) {
    return null;
  }
  if (typeof item.label !== 'string') {
    return null;
  }
  var length = Array.from(item.label).length;
  if (length < 1 || length > 100) {
    return null;
  }
  var confidence = item.confidence;
  if (confidence !== null && !isNumberIn(confidence, 0, 1)) {
    return null;
  }
  var box = detectedBox(item.box);
  if (box === null) {
    return null;
  }
  return {label: item.label, confidence: confidence, box: box};
}

// The original's OutputData order: descending confidence, unrated last;
// stable, so equally rated objects keep the detector's order.
function mostConfidentFirst(objects) {
  return objects
    .map(function (object, at) { return {object: object, at: at}; })
    .sort(function (a, b) {
      var left = a.object.confidence;
      var right = b.object.confidence;
      if ((left === null) !== (right === null)) {
        return left === null ? 1 : -1;
      }
      return (right || 0) - (left || 0) || a.at - b.at;
    })
    .map(function (entry) { return entry.object; });
}
