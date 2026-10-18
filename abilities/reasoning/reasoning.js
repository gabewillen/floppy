// The ECMAScript of abilities/reasoning/reasoning.scx (SCXML 5.8): loaded by its root
// <script src="reasoning.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared by the steps.

function hasNoDatabase() {
  return database === null;
}

// The input of bot.ability.reasoning.recalled.
function focusInput() {
  return ({focus: focus, focus_candidates: focus_candidates});
}

// This turn's bot.ability.reasoning.recalled.
function isCurrentTurn(_event) {
  return _event.data.turn === turn;
}

// recalling

// The original _context_ref_from_input: the stimulus's target_device, or null.
function contextRefFromInput() {
  return typeof Object(Object(stimulus).data).target_device === 'string' ? stimulus.data.target_device : null;
}

function hostInput() {
  return ({input: stimulus});
}

// The reference episodes.py: the prior cognitive episodes.
function episodeSelectStatements() {
  return [{sql: episode_select, parameters: ['cognitive_episode', context_ref]}];
}

// The reference directives.py: the standing directives appended.
function directiveSelectStatements() {
  return statements.concat([{sql: episode_select, parameters: ['standing_directive', context_ref]}]);
}

// The parsed contents of the rows of one statement's result; a row that is
// not a JSON object is skipped (the reference episodes_from_contents and
// directives_from_contents skip rows that do not validate).
function recalledContents(_event, index) {
  return _event.data.results[index].rows.map(function (row) { return parsedContent(row.values[0]); }).filter(function (content) { return content !== null; });
}

function parsedContent(text) {
  try {
    var content = JSON.parse(text);
    return content !== null && typeof content === 'object' && !Array.isArray(content) ? content : null;
  } catch (error) {
    return null;
  }
}

function recallFailedResult(detail) {
  return ({turn: turn, handled: false, failed: true, reason: 'Reasoning memory recall failed: ' + detail});
}

function hasRecalledWithoutChoices(_event) {
  return isCurrentTurn(_event) && choices.length === 0;
}

function noChoicesResult() {
  return ({turn: turn, handled: false, failed: true, reason: 'the reasoning has no choices to select from'});
}

// applying

function choiceNamesLog() {
  return 'turn ' + turn + ': ' + choice_names;
}

// The model prompt: the original's model_facing_xml(ProcessorInput)
// (abilities/cognition/event.py, processing.py InputData.model_facing_payload)
// over floppy's JSON, as {prompt} or {failure} (the original's ValueError text).
// ProcessorInput is <cognition:processor_input> holding host_input (the
// processing:input with the stimulus as its input field), prior_episodes and
// standing_directives; the stimulus's data is untyped JSON here, so it
// renders as the original renders a non-model payload (a content field).
function promptProjection() {
  try {
    var budget = {bytes: 0, nodes: 0, items: 0};
    var root = xmlElement(null, 'cognition:processor_input', budget);
    var host = xmlElement(root, 'host_input', budget);
    var input = xmlElement(host, 'processing:input', budget);
    if (stimulus !== null) {
      xmlStimulus(input, stimulus, budget);
    }
    xmlModels(root, 'prior_episodes', prior_episodes, xmlEpisode, budget);
    xmlModels(root, 'standing_directives', standing_directives, xmlDirective, budget);
    // The environment block (decision D106), in the original's layout.
    xmlEnvironment(root, environment, budget);
    var rendered = xmlText(root, 0);
    if (utf8Length(rendered) > 262144) {
      throw new Error('prompt projection encoded byte budget exceeded');
    }
    if (codePoints(system) > 65536) {
      throw new Error('prompt projection scalar length budget exceeded');
    }
    if (utf8Length(system) + utf8Length(rendered) + utf8Length(JSON.stringify(schema)) > 262144) {
      throw new Error('prompt projection cumulative encoded byte budget exceeded');
    }
    return ({prompt: rendered, failure: null});
  } catch (error) {
    return ({prompt: null, failure: String(error.message)});
  }
}

function hasNoPrompt() {
  return projection.failure !== null;
}

function projectionFailedResult() {
  return ({turn: turn, handled: false, failed: true, reason: projection.failure});
}

// The original _ProjectionBudget (262144 encoded bytes, 4096 nodes, 4096
// collection items, 65536 characters per scalar).
function budgetMarkup(budget, text) {
  budget.bytes += utf8Length(text);
  if (budget.bytes > 262144) {
    throw new Error('prompt projection encoded byte budget exceeded');
  }
}

function budgetCollection(budget, count) {
  budget.items += count;
  if (budget.items > 4096) {
    throw new Error('prompt projection collection item budget exceeded');
  }
}

function budgetScalar(budget, text) {
  if (codePoints(text) > 65536) {
    throw new Error('prompt projection scalar length budget exceeded');
  }
  budgetMarkup(budget, text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#x27;'));
}

function utf8Length(text) {
  var length = 0;
  for (var index = 0; index < text.length; index++) {
    var unit = text.charCodeAt(index);
    if (unit < 0x80) {
      length += 1;
    } else if (unit < 0x800) {
      length += 2;
    } else if (unit >= 0xd800 && unit < 0xdc00 && index + 1 < text.length) {
      length += 4;
      index++;
    } else {
      length += 3;
    }
  }
  return length;
}

function codePoints(text) {
  var count = 0;
  for (var index = 0; index < text.length; index++) {
    var unit = text.charCodeAt(index);
    if (unit >= 0xd800 && unit < 0xdc00 && index + 1 < text.length) {
      index++;
    }
    count++;
  }
  return count;
}

// One element (the original _element/_subelement), appended to parent when given.
function xmlElement(parent, name, budget) {
  budget.nodes += 1;
  if (budget.nodes > 4096) {
    throw new Error('prompt projection node budget exceeded');
  }
  budgetMarkup(budget, name);
  var element = {name: name, attributes: [], children: [], text: null};
  if (parent !== null) {
    parent.children.push(element);
  }
  return element;
}

function xmlAttribute(element, name, value, budget) {
  budgetMarkup(budget, name);
  budgetScalar(budget, value);
  element.attributes.push([name, value]);
}

// A number as Python's str writes it: whole numbers as integers, else the
// shortest repr, in exponent form below 1e-4 or from 1e16.
function pythonNumber(value) {
  if (Number.isInteger(value) && Math.abs(value) < 1e21) {
    return value.toFixed(0);
  }
  var magnitude = Math.abs(value);
  if (magnitude !== 0 && (magnitude < 1e-4 || magnitude >= 1e16)) {
    var parts = value.toExponential().split('e');
    var exponent = parts[1].charAt(0) === '-' ? '-' : '+';
    var digits = parts[1].replace(/^[-+]/, '');
    return parts[0] + 'e' + exponent + (digits.length < 2 ? '0' + digits : digits);
  }
  return String(value);
}

// The original _fill_field over a JSON value: null is skipped, a scalar is an
// attribute, an object is <name> of <entry key> elements (each value
// filled as value), an array is <name count> of <item> elements: a scalar
// item as its text, an object or array item filled as item, as the
// original renders a model item (a conversation's messages; decision D99).
function xmlField(element, name, value, budget) {
  if (value === null || value === undefined) {
    return;
  }
  if (typeof value === 'boolean') {
    xmlAttribute(element, name, value ? 'true' : 'false', budget);
  } else if (typeof value === 'string') {
    xmlAttribute(element, name, value, budget);
  } else if (typeof value === 'number') {
    xmlAttribute(element, name, pythonNumber(value), budget);
  } else if (Array.isArray(value)) {
    budgetCollection(budget, value.length);
    var list = xmlElement(element, name, budget);
    xmlAttribute(list, 'count', String(value.length), budget);
    value.forEach(function (item) {
      if (typeof item === 'boolean' || typeof item === 'string' || typeof item === 'number') {
        var text = typeof item === 'boolean' ? (item ? 'true' : 'false') : (typeof item === 'number' ? pythonNumber(item) : item);
        budgetScalar(budget, text);
        xmlElement(list, 'item', budget).text = text;
      } else {
        xmlField(list, 'item', item, budget);
      }
    });
  } else if (typeof value === 'object') {
    var mapping = xmlElement(element, name, budget);
    var keys = Object.keys(value);
    budgetCollection(budget, keys.length);
    keys.forEach(function (key) {
      var entry = xmlElement(mapping, 'entry', budget);
      xmlAttribute(entry, 'key', key, budget);
      xmlField(entry, 'value', value[key], budget);
    });
  }
}

// The original's environment block (Environment.model_snapshot from the
// bot's perspective): <environment id> holding <self state> with the bot's
// own attributes as children, its owned_devices as <owned_devices> holding
// one <device ref id state> per held device with its attributes as
// children. Nothing when the turn has no environment.
function xmlEnvironment(parent, value, budget) {
  if (value === null || typeof value !== 'object') {
    return;
  }
  var element = xmlElement(parent, 'environment', budget);
  xmlAttribute(element, 'id', String(value.id), budget);
  var own = Object(value.self);
  var self = xmlElement(element, 'self', budget);
  xmlAttribute(self, 'state', String(own.state), budget);
  var others = {};
  Object.keys(own).forEach(function (key) {
    if (key !== 'state' && key !== 'owned_devices') {
      others[key] = own[key];
    }
  });
  xmlAttributes(self, others, budget);
  var owned = Object(own.owned_devices);
  var devices = xmlElement(self, 'owned_devices', budget);
  var references = Object.keys(owned);
  budgetCollection(budget, references.length);
  references.forEach(function (reference) {
    var device = Object(owned[reference]);
    var element = xmlElement(devices, 'device', budget);
    ['ref', 'id', 'state'].forEach(function (key) { xmlAttribute(element, key, String(device[key]), budget); });
    xmlAttributes(element, Object(device.attributes), budget);
  });
}

// A snapshot's attributes as child elements of element, as the original
// renders them: a string as its text, a boolean or number as its JSON
// text, a mapping as nested child elements by the same rule, an array as
// the renderer's list (D99), and null or an opaque value as nothing. Each
// tag is the key with every character other than a letter, a digit, `_`,
// `.` or `-` replaced by `_`, and `_` put first when it is empty or starts
// with a digit, `.` or `-`.
function xmlAttributes(element, values, budget) {
  var keys = Object.keys(values);
  budgetCollection(budget, keys.length);
  keys.forEach(function (key) {
    var value = values[key];
    var tag = xmlTag(key);
    if (typeof value === 'string') {
      budgetScalar(budget, value);
      xmlElement(element, tag, budget).text = value;
    } else if (typeof value === 'boolean' || typeof value === 'number') {
      var text = JSON.stringify(value);
      budgetScalar(budget, text);
      xmlElement(element, tag, budget).text = text;
    } else if (Array.isArray(value)) {
      xmlField(element, tag, value, budget);
    } else if (value !== null && typeof value === 'object') {
      xmlAttributes(xmlElement(element, tag, budget), value, budget);
    }
  });
}

// The original's sanitized tag of a key.
function xmlTag(key) {
  var tag = String(key).replace(/[^A-Za-z0-9_.-]/g, '_');
  return tag.length === 0 || /^[0-9.-]/.test(tag) ? '_' + tag : tag;
}

// The stimulus envelope as the original's hsm.Event field: <input> holding its
// data as content, stamped stimulus:event, then stimulus:id, :source and
// :target when set.
function xmlStimulus(parent, envelope, budget) {
  var input = xmlElement(parent, 'input', budget);
  xmlField(input, 'content', envelope.data, budget);
  xmlAttribute(input, 'stimulus:event', envelope.event, budget);
  ['id', 'source', 'target'].forEach(function (key) {
    if (typeof envelope[key] === 'string' && envelope[key].length > 0) {
      xmlAttribute(input, 'stimulus:' + key, envelope[key], budget);
    }
  });
}

// A tuple of models: <name count> holding one element per model.
function xmlModels(parent, name, models, render, budget) {
  budgetCollection(budget, models.length);
  var list = xmlElement(parent, name, budget);
  xmlAttribute(list, 'count', String(models.length), budget);
  models.forEach(function (model) { list.children.push(render(model, budget)); });
}

// The original episodes.CognitiveEpisode: focus, focus_candidates, stimulus_name,
// output (cognition.types.EventData), behavior.
function xmlEpisode(episode, budget) {
  var element = xmlElement(null, 'cognition:cognitive_episode', budget);
  xmlField(element, 'focus', episode.focus, budget);
  xmlField(element, 'focus_candidates', Array.isArray(episode.focus_candidates) ? episode.focus_candidates : [], budget);
  xmlField(element, 'stimulus_name', episode.stimulus_name, budget);
  xmlModels(element, 'output', Array.isArray(episode.output) ? episode.output : [], xmlSelection, budget);
  if (episode.behavior !== null && typeof episode.behavior === 'object' && !Array.isArray(episode.behavior)) {
    var behavior = xmlElement(element, 'behavior', budget);
    behavior.children.push(xmlBehavior(episode.behavior, budget));
  }
  return element;
}

// The reference cognition.types.EventData: target, event, data, reason.
function xmlSelection(selection, budget) {
  var element = xmlElement(null, 'cognition:event', budget);
  ['target', 'event', 'data', 'reason'].forEach(function (key) { xmlField(element, key, Object(selection)[key], budget); });
  return element;
}

// The reference behavior.events CreateData, ChangeData, BreakData.
function xmlBehavior(behavior, budget) {
  var kind = typeof behavior.event === 'string' ? behavior.event.split('.').pop() : 'create';
  var element = xmlElement(null, 'behavior:' + kind, budget);
  var keys = kind === 'break' ? ['event', 'name', 'reason'] : ['event', 'name', 'triggers', 'description', 'reason', 'source'];
  keys.forEach(function (key) { xmlField(element, key, behavior[key], budget); });
  return element;
}

// The original directives.Directive: text, kind (instruction by default).
function xmlDirective(directive, budget) {
  var element = xmlElement(null, 'cognition:directive', budget);
  xmlField(element, 'text', directive.text, budget);
  xmlField(element, 'kind', directive.kind === undefined ? 'instruction' : directive.kind, budget);
  return element;
}

// ElementTree.indent(space="  ") then ElementTree.tostring: attributes in
// order, an empty element as <name />.
function xmlText(element, level) {
  var attributes = element.attributes.map(function (attribute) {
    return ' ' + attribute[0] + '="' + attribute[1].replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/\r/g, '&#13;').replace(/\n/g, '&#10;').replace(/\t/g, '&#09;') + '"';
  }).join('');
  var open = '<' + element.name + attributes;
  var text = element.text === null ? '' : element.text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  if (element.children.length === 0) {
    return text.length === 0 ? open + ' />' : open + '>' + text + '</' + element.name + '>';
  }
  var inner = '\n' + '  '.repeat(level + 1);
  var children = element.children.map(function (child) { return xmlText(child, level + 1); });
  return open + '>' + inner + children.join(inner) + '\n' + '  '.repeat(level) + '</' + element.name + '>';
}

// Temporary, until the mapping onto the original's domains moves it to Rust.
// The original's _events_from_dispatch_tool_calls: the events of every
// dispatch call of a generate.result, merged in call order (the service
// returns only calls of its one function, each its schema admits), else
// selected unchanged (D41: the service's events are unchecked).
function generatedSelection(_event) {
  var calls = Object(_event.data).tool_calls;
  if (!Array.isArray(calls)) {
    return selected;
  }
  return calls.filter(function (call) { return Object(call).name === 'dispatch'; }).reduce(function (merged, call) { return merged.concat(call.args.events); }, []);
}

// The code of a generate.failed, else gen_failure unchanged.
function generateFailure(_event) {
  return typeof Object(_event.data).code === 'string' ? _event.data.code : gen_failure;
}

function hasSelection() {
  return selected !== null;
}

function failedResult() {
  return ({turn: turn, handled: false, failed: true, reason: 'the reasoning failed (' + (gen_failure === null ? 'no selection' : gen_failure) + ')'});
}

function couldNotRunResult(_event) {
  return ({turn: turn, handled: false, failed: true, reason: 'the reasoning could not run (' + _event.name + ')'});
}

// dispatching

// No selection was rejected.
function isAccepted() {
  return rejection === null;
}

// The original: the rejection fails the stage with its message.
function rejectedResult() {
  return ({turn: turn, handled: false, failed: true, reason: rejection});
}

// done

function selectedResult() {
  return ({turn: turn, handled: true, selected: selected});
}
