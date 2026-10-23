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

// The detections tool's JSON Schema (decision D201): the generate service
// offers it as the one function the model must call and admits only
// arguments it admits, so every object is checked before this document
// reads it. A box past the image's edge is the one rule it cannot state.
function detectionsSchema() {
  var unit = {type: 'number', minimum: 0, maximum: 1};
  var extent = {type: 'number', exclusiveMinimum: 0, maximum: 1};
  return {
    description: 'Report the distinct objects seen in the image, an empty list when there are none.',
    type: 'object',
    additionalProperties: false,
    required: ['objects'],
    properties: {
      objects: {
        description: 'The objects seen, one item each.',
        type: 'array',
        items: {
          type: 'object',
          additionalProperties: false,
          required: ['label', 'confidence', 'box'],
          properties: {
            label: {description: 'A short name for the object.', type: 'string', minLength: 1, maxLength: 100},
            confidence: {
              description: 'How sure you are that the object is there, from 0 to 1; null when you cannot say.',
              anyOf: [unit, {type: 'null'}]
            },
            box: {
              description: 'Where the object is, normalized to the image: fractions of its width and height measured from its top-left corner, the box inside the image.',
              type: 'object',
              additionalProperties: false,
              required: ['x', 'y', 'width', 'height'],
              properties: {
                x: Object.assign({description: 'Left edge, as a fraction of the image width.'}, unit),
                y: Object.assign({description: 'Top edge, as a fraction of the image height.'}, unit),
                width: Object.assign({description: 'Box width, as a fraction of the image width.'}, extent),
                height: Object.assign({description: 'Box height, as a fraction of the image height.'}, extent)
              }
            }
          }
        }
      }
    }
  };
}

// A generate result whose every call carries an objects array (D41: the
// service's events are unchecked; with a schema it returns only calls the
// schema admits, at least one).
function isDetectionsResult(_event) {
  var calls = Object(_event.data).tool_calls;
  return Array.isArray(calls) && calls.length > 0 && calls.every(function (call) {
    return Array.isArray(Object(Object(call).args).objects);
  });
}

function generatorFailedMessage(_event) {
  return typeof Object(_event.data).code === 'string' ? 'Object detection generator failed: ' + _event.data.code + '.' : 'Object detection generator failed.';
}

// replied: the detector on the calls.

// {objects, skipped}: the objects of every call, in order, without those
// whose box passes the image's edge (skipped counts them), the first
// max_objects of them, most confident first.
function detectedObjects() {
  var found = [];
  var skipped = 0;
  for (var call of calls) {
    for (var object of call.args.objects) {
      if (isInsideImage(object.box)) {
        found.push(object);
      } else {
        skipped += 1;
      }
    }
  }
  return {objects: mostConfidentFirst(found.slice(0, max_objects)), skipped: skipped};
}

function hasSkippedItems() {
  return detected.skipped > 0;
}

function mismatchedMessage() {
  return 'Object detection produced output that does not match its output schema.';
}

// x + width and y + height at most 1, up to 1e-6 slack (the schema has
// already bounded each number).
function isInsideImage(box) {
  return box.x + box.width <= 1 + 1e-6 && box.y + box.height <= 1 + 1e-6;
}

// The output order: descending confidence, unrated last;
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
