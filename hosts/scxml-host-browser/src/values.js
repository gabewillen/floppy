// Structural validation of WIT values crossing from page code into a
// component. These checks only reject malformed shapes before canonical-ABI
// lowering; every semantic bound (name syntax, byte limits, canonical JSON)
// is enforced by the receiving component.

const VALUE_TAGS = new Set([
  'absent',
  'undefined',
  'null',
  'boolean',
  'number',
  'text',
  'json-array',
  'json-object',
  'xml',
]);
const STRING_TAGS = new Set(['text', 'json-array', 'json-object', 'xml']);
const EVENT_TYPES = new Set(['internal', 'external', 'platform']);
/** WIT `status` values (stateforward:scxml/types@0.2.0). */
export const STATUSES = Object.freeze(['ready', 'running', 'finished', 'failed', 'shut-down']);
const BAGGAGE_POLICIES = new Set(['drop', 'propagate']);

const PLACEMENT_TAGS = new Set([
  'replace-children',
  'first-child',
  'last-child',
  'previous-sibling',
  'next-sibling',
  'replace',
  'delete',
]);

/**
 * True when `value` is a well-formed WIT `assign-placement` in jco shape:
 * `{ tag }` for a unit case, `{ tag: 'add-attribute', val: name }`.
 */
export function isAssignPlacement(value) {
  if (value === null || typeof value !== 'object') return false;
  if (value.tag === 'add-attribute') return typeof value.val === 'string';
  return PLACEMENT_TAGS.has(value.tag) && value.val === undefined;
}

/**
 * True when `value` is a well-formed WIT `assign-source` in jco shape:
 * `{ tag: 'expression', val: string }`, `{ tag: 'value', val: data-value }`,
 * or `{ tag: 'foreach-value', val: data-value }`.
 */
export function isAssignSource(value) {
  if (value === null || typeof value !== 'object') return false;
  if (value.tag === 'expression') return typeof value.val === 'string';
  return (value.tag === 'value' || value.tag === 'foreach-value') && isDataValue(value.val);
}

/** True when `value` is a well-formed WIT `data-value` in jco shape. */
export function isDataValue(value) {
  if (value === null || typeof value !== 'object' || !VALUE_TAGS.has(value.tag)) return false;
  if (value.tag === 'boolean') return typeof value.val === 'boolean';
  if (value.tag === 'number') return typeof value.val === 'number' && Number.isFinite(value.val);
  if (STRING_TAGS.has(value.tag)) return typeof value.val === 'string';
  return value.val === undefined;
}

/** True when `value` is a jco WIT `data-value`, or `undefined` (WIT `none`). */
export function isOptionalDataValue(value) {
  return value === undefined || isDataValue(value);
}

/** True when `value` is a jco WIT `data-initializer`. */
export function isDataInitializer(value) {
  if (value === null || typeof value !== 'object') return false;
  if (value.tag === 'expression') return typeof value.val === 'string';
  return value.tag === 'value' && isDataValue(value.val);
}

/** True when `value` is a jco WIT `inline-body`. */
export function isInlineBody(value) {
  return (
    value !== null &&
    typeof value === 'object' &&
    (value.tag === 'text' || value.tag === 'markup') &&
    typeof value.val === 'string'
  );
}

/** True when `value` is a list of jco WIT `named-value` records. */
export function isNamedValues(value) {
  return (
    Array.isArray(value) &&
    value.every((entry) => isRecord(entry) && typeof entry.name === 'string' && isDataValue(entry.value))
  );
}

/** True when `value` is a jco WIT `foreach-result`. */
export function isForeachResult(value) {
  if (value === null || typeof value !== 'object') return false;
  if (value.tag === 'items') return Array.isArray(value.val) && value.val.every(isDataValue);
  return value.tag === 'illegal' && value.val === undefined;
}

function optionalString(value) {
  return value === undefined || typeof value === 'string';
}

function isRecord(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function isTraceContext(value) {
  return (
    isRecord(value) &&
    typeof value.traceparent === 'string' &&
    optionalString(value.tracestate) &&
    optionalString(value.baggage) &&
    BAGGAGE_POLICIES.has(value.baggagePolicy)
  );
}

/** True when `value` is a well-formed WIT `event` in jco shape. */
export function isWitEvent(value) {
  return (
    isRecord(value) &&
    typeof value.name === 'string' &&
    EVENT_TYPES.has(value.eventType) &&
    optionalString(value.sendId) &&
    optionalString(value.origin) &&
    optionalString(value.originType) &&
    optionalString(value.invokeId) &&
    isDataValue(value.data) &&
    optionalString(value.raw) &&
    (value.traceContext === undefined || isTraceContext(value.traceContext))
  );
}

/**
 * Builds the WIT `event` record for an external command. A page-supplied
 * `traceContext` (the page's W3C carrier) rides on the event: the batch
 * span that processes it becomes its child. The machine validates it.
 * A page event is an external event (an Event I/O Processor delivery), so
 * any `type` but `external` is malformed here: the machine's `session.send`
 * also admits `platform` errors, which only the host raises (a late
 * `error.communication`), never a page. `invokeId` is forwarded unchanged:
 * the machine is the one enforcement point that rejects it and an
 * `internal` type on `session.send` (`invalid-input`), for every host alike.
 * @param {string | {name: string, type?: 'external', data?: object, sendId?: string,
 *   origin?: string, originType?: string, traceContext?: object}} input
 * @returns {object | undefined} the jco event, or undefined when malformed
 */
export function toWitEvent(input) {
  const event = typeof input === 'string' ? { name: input } : input;
  if (event === null || typeof event !== 'object' || typeof event.name !== 'string') return undefined;
  const eventType = event.type ?? 'external';
  const data = event.data ?? { tag: 'absent' };
  if (
    eventType !== 'external' ||
    !isDataValue(data) ||
    !optionalString(event.sendId) ||
    !optionalString(event.origin) ||
    !optionalString(event.originType) ||
    !optionalString(event.invokeId) ||
    !(event.traceContext === undefined || isTraceContext(event.traceContext))
  ) {
    return undefined;
  }
  return {
    name: event.name,
    eventType,
    sendId: event.sendId,
    origin: event.origin,
    originType: event.originType,
    invokeId: event.invokeId,
    data,
    raw: undefined,
    traceContext:
      event.traceContext === undefined
        ? undefined
        : {
            traceparent: event.traceContext.traceparent,
            tracestate: event.traceContext.tracestate,
            baggage: event.traceContext.baggage,
            baggagePolicy: event.traceContext.baggagePolicy,
          },
  };
}
