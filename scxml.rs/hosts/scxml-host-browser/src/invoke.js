// Automatic `<invoke>` child compilation (createSession `invoke.compile:
// 'auto'`, the default).
//
// The machine asks for a child's Machine IR through the synchronous
// `compile-invocation(source-name, source)` capability. The Wasmtime session
// host answers from the compiler component
// (hosts/scxml-host-wasmtime/src/session_host/links.rs `compile_invocation`);
// this module does the same in the browser, with one dedicated compiler
// instance per session, opened before `start` and then called
// synchronously inside the capability:
// * the request is the Wasmtime host's: lowered IR, schema policy off,
//   runtime script resolution;
// * outcomes map as Wasmtime maps them: emitted IR is the child; a report
//   that rejects the document (diagnostics, no artifact) or the compiler's
//   `invalid-request` is `rejected`, which the machine turns into
//   error.execution in the invoking session (SCXML 6.4); any other compiler
//   failure, a trap, or fuel exhaustion is a run `fault`;
// * bounds: the host caps no source size (the compiler component's own
//   bound decides, as it does under Wasmtime), and each compile runs on the
//   session's deterministic fuel
//   budget (src/fuel.js), capped per compile by `invoke.fuel`: a compile
//   inside a machine command spends that command's budget, and a pre-warm
//   compile is a host call of its own;
// * results are cached per session by the exact `(sourceName, source)` key,
//   at most MAX_CACHED_INVOCATIONS entries and MAX_INVOCATION_CACHE_BYTES
//   in all (oldest evicted first). Compilation is deterministic, so a hit
//   is exactly the outcome a fresh compile would give.
//
// Pre-warm: before `start`, `staticInvocations` finds every `<invoke>` of
// the root IR whose child document is known without evaluating anything
// (SCXML type, no `src`/`srcexpr`, inline `<content>` without `expr`) and
// rebuilds the exact source the machine will request, so the first
// invocation is a cache hit. This is the one place the host reads IR, and
// only as a hint: a document it cannot read is simply not pre-warmed, and
// the machine still validates the IR and decides every invocation.
//
// Depth: an invoked session invokes in turn, as under Wasmtime, within the
// session's invoke limits. Only the root's statically known children are
// pre-warmed; deeper documents compile on first use.

import { openCompiler } from './compiler.js';
import { ScxmlCompileError, ScxmlHostError } from './errors.js';
import { DEFAULT_COMPILE_FUEL, isFuel } from './fuel.js';

/** createSession `invoke.compile` values. */
export const INVOKE_COMPILE_MODES = Object.freeze(['auto', 'provided']);
/** Most cached compile outcomes per session. */
export const MAX_CACHED_INVOCATIONS = 64;
/** Most cached source and IR bytes per session (UTF-16 code units count as bytes). */
export const MAX_INVOCATION_CACHE_BYTES = 32 * 1024 * 1024;

/** The machine's source name for inline `<content>` (crates/scxml-engine invoke.rs). */
const CONTENT_SOURCE_NAME = 'invoke-content.scxml';
/** `scxml_compiler_core::MAX_STRING_BYTES`: the machine's bound on serialized content. */
const MAX_CONTENT_BYTES = 1024 * 1024;
/** `scxml_compiler_core::MAX_CONTENT_DEPTH`: deeper content is invalid IR. */
const MAX_CONTENT_DEPTH = 64;
const SCXML_NAMESPACE = 'http://www.w3.org/2005/07/scxml';
/** `scxml_types::is_scxml_invoke_type`. */
const SCXML_INVOKE_TYPES = new Set(['scxml', 'http://www.w3.org/TR/scxml/', 'http://www.w3.org/TR/scxml']);
const INVOKE_OPTION_KEYS = new Set(['compile', 'fuel']);
const utf8 = new TextEncoder();

/** Whether `value` is an accepted createSession `invoke` option. */
export function isInvokeOption(value) {
  if (value === undefined) return true;
  if (value === null || typeof value !== 'object') return false;
  return (
    Object.keys(value).every((key) => INVOKE_OPTION_KEYS.has(key)) &&
    (value.compile === undefined || INVOKE_COMPILE_MODES.includes(value.compile)) &&
    (value.fuel === undefined || isFuel(value.fuel))
  );
}

/** Whether the UTF-8 length of `text` exceeds `bound` (encoded only when it could). */
function exceeds(text, bound) {
  return text.length > bound || (text.length * 3 > bound && utf8.encode(text).byteLength > bound);
}

/**
 * Serializes inline `<content>` nodes exactly as the machine does
 * (crates/scxml-engine/src/interpret/invoke.rs `serialize_child_document`):
 * text raw, attribute values escaping `"`, `&`, and `<`, and the root SCXML
 * element restating its namespace. Returns `undefined` for nodes that are
 * not IR, nest too deeply, or exceed the machine's bound.
 */
function serializeChildDocument(nodes) {
  const parts = [];
  let length = 0;
  const push = (text) => {
    length += text.length;
    parts.push(text);
  };
  const walk = (list, depth) => {
    if (!Array.isArray(list) || depth > MAX_CONTENT_DEPTH) return false;
    for (const node of list) {
      if (node?.kind === 'text' && typeof node.value === 'string') {
        push(node.value);
        continue;
      }
      const name = node?.name?.local_name;
      if (node?.kind !== 'element' || typeof name !== 'string' || !Array.isArray(node.attributes)) return false;
      push(`<${name}`);
      if (
        depth === 0 &&
        node.name.namespace_uri === SCXML_NAMESPACE &&
        !node.attributes.some((attribute) => attribute?.name?.local_name === 'xmlns')
      ) {
        push(` xmlns="${SCXML_NAMESPACE}"`);
      }
      for (const attribute of node.attributes) {
        if (typeof attribute?.name?.local_name !== 'string' || typeof attribute.value !== 'string') return false;
        const value = attribute.value.replace(/["&<]/g, (c) => (c === '"' ? '&quot;' : c === '&' ? '&amp;' : '&lt;'));
        push(` ${attribute.name.local_name}="${value}"`);
      }
      if (!Array.isArray(node.children) || node.children.length === 0) {
        if (!Array.isArray(node.children)) return false;
        push('/>');
      } else {
        push('>');
        if (!walk(node.children, depth + 1)) return false;
        push(`</${name}>`);
      }
      if (length > MAX_CONTENT_BYTES) return false;
    }
    return true;
  };
  if (!walk(nodes, 0)) return undefined;
  const text = parts.join('');
  return exceeds(text, MAX_CONTENT_BYTES) ? undefined : text;
}

/** The statically known child document of one `<invoke>` IR row, if any. */
function staticDocument(invoke) {
  if (invoke.src != null || invoke.srcexpr != null || invoke.typeexpr != null) return undefined;
  if (invoke.type != null && !SCXML_INVOKE_TYPES.has(invoke.type)) return undefined;
  if (!Array.isArray(invoke.children)) return undefined;
  const content = invoke.children.find((child) => child?.kind === 'content')?.value;
  if (content === undefined || content.expr != null) return undefined;
  return serializeChildDocument(content.body);
}

/**
 * Reads lowered Machine IR as a pre-warm hint: whether any state declares
 * `<invoke>`, and the distinct `(sourceName, source)` requests of its
 * statically known inline children, in document order. `undefined` when
 * the bytes are not readable IR (the machine then decides).
 * @param {Uint8Array} ir
 * @returns {{invokes: boolean, documents: Array<[string, string]>} | undefined}
 */
export function staticInvocations(ir) {
  let machine;
  try {
    machine = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(ir));
  } catch {
    return undefined;
  }
  if (!Array.isArray(machine?.states)) return undefined;
  let invokes = false;
  const documents = new Map();
  for (const state of machine.states) {
    if (!Array.isArray(state?.invokes)) continue;
    for (const invoke of state.invokes) {
      invokes = true;
      const source = invoke === null || typeof invoke !== 'object' ? undefined : staticDocument(invoke);
      if (source !== undefined) documents.set(source, [CONTENT_SOURCE_NAME, source]);
    }
  }
  return { invokes, documents: [...documents.values()] };
}

/**
 * One session's automatic child compiler: a dedicated compiler instance
 * and its bounded outcome cache. `compile` returns `{ir}`, `{rejected}`, or
 * `{fault}` (a typed error that fails the run); it never throws.
 */
export class InvocationCompiler {
  #open;
  #compiler;
  #telemetry;
  /** The ExtensionPackages of every namespace the session binds. */
  #packages;
  /** key -> {outcome, bytes}, oldest first. */
  #cache = new Map();
  #bytes = 0;

  constructor(open, compiler, telemetry, packages) {
    this.#open = open;
    this.#compiler = compiler;
    this.#telemetry = telemetry;
    this.#packages = packages;
  }

  /**
   * Opens the session's compiler.
   * @param {object} options
   * @param {Function} options.loadCoreModule `(component, file) => Module`
   * @param {object} [options.telemetry] OpenTelemetry providers option
   * @param {object} options.hostTelemetry the session's Telemetry
   * @param {number} [options.fuel] per-compile cap (default DEFAULT_COMPILE_FUEL)
   * @param {import('./fuel.js').FuelBudget} options.budget the session's budget
   * @param {object[]} [options.packages] the ExtensionPackages the session
   *   binds, forwarded with every child (as the Wasmtime host forwards its
   *   bound packages), so a child may use its parent's namespaces
   */
  static async open({ loadCoreModule, telemetry, hostTelemetry, fuel = DEFAULT_COMPILE_FUEL, budget, packages = [] }) {
    const open = () => openCompiler({ loadCoreModule, telemetry, fuel, budget });
    return new InvocationCompiler(open, await open(), hostTelemetry, packages);
  }

  /** Number of cached outcomes. */
  get size() {
    return this.#cache.size;
  }

  /**
   * Compiles each pre-warm request into the cache before `start`. A request
   * that traps the compiler caches its fault, and a fresh compiler serves
   * the rest, so every cached outcome is what a first compile of that
   * document gives.
   * @param {Array<[string, string]>} documents
   */
  async prewarm(documents) {
    for (const [sourceName, source] of documents) {
      if (this.#cache.size >= MAX_CACHED_INVOCATIONS) return;
      if (this.compile(sourceName, source).fault !== undefined) this.#compiler = await this.#open();
    }
  }

  /**
   * The outcome of compiling one child document (see the module comment).
   * @param {string} sourceName
   * @param {string} source
   * @returns {{ir: Uint8Array} | {rejected: true} | {fault: Error}}
   */
  compile(sourceName, source) {
    const key = `${sourceName.length}:${sourceName}${source}`;
    const hit = this.#cache.get(key);
    if (hit !== undefined) return hit.outcome;
    let outcome;
    try {
      const result = this.#compiler.compile(source, { sourceName, schemaPolicy: 'off', packages: this.#packages });
      outcome = result.status === 'emitted' ? { ir: result.ir } : { rejected: true };
    } catch (error) {
      if (error instanceof ScxmlCompileError && error.code === 'invalid-request') {
        outcome = { rejected: true };
      } else if (error instanceof ScxmlCompileError || error instanceof ScxmlHostError) {
        outcome = { fault: error };
      } else {
        outcome = { fault: new ScxmlHostError('instance-lost', { cause: error }) };
      }
    }
    this.#remember(key, outcome);
    return outcome;
  }

  #remember(key, outcome) {
    const bytes = key.length + (outcome.ir?.byteLength ?? 0);
    if (bytes > MAX_INVOCATION_CACHE_BYTES) return;
    while (this.#cache.size >= MAX_CACHED_INVOCATIONS || this.#bytes + bytes > MAX_INVOCATION_CACHE_BYTES) {
      const [oldest, entry] = this.#cache.entries().next().value;
      this.#cache.delete(oldest);
      this.#bytes -= entry.bytes;
    }
    this.#cache.set(key, { outcome, bytes });
    this.#bytes += bytes;
  }
}
