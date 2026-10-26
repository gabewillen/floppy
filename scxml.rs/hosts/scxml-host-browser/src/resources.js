// URL resources of one session: the page's `resources` option of
// createSession, the prefetch that resolves every statically known URL
// before the machine runs, and the synchronous cache the machine's
// `load-document` and `load-script` calls are served from.
//
// Security: the host never reaches the network by itself. Without the
// `resources` option nothing is fetched and every URL load keeps its typed
// error (deny-all). With it, a URL reaches the page's `resolve` only after
// the page's `allow` returned exactly `true` for it. `allow` and `resolve`
// both get an http(s) URL in its canonical form (src/url.js), the form a
// fetch of it reaches, so dot segments cannot step outside a path the
// policy admitted; one with userinfo is denied without asking the page, and
// any other URL reaches the page as written. As under the Wasmtime session
// host, the host bounds no resolved text: a document of any size is
// accepted, and a script's text goes to the data model, whose own bound
// decides.
//
// Machine calls are synchronous (WIT), and a page's fetch is not, so the
// host resolves URLs before the machine runs. It collects them from the
// IR's statically known sources: `<data src>`, unpinned `<script src>`, and
// `<invoke src>`. This scan only chooses what to prefetch: it decides no
// SCXML semantics, and the machine still validates the IR and decides every
// load. A URL computed at run time (`srcexpr`) is not in the cache: it
// answers `unavailable` with detail UNSUPPORTED.dynamicUrl
// (`dynamic-url-unavailable`, machine.js) unless the page's synchronous
// `capabilities.loadDocument` resolves it or, in an `async: 'auto'` worker
// session, the page's `resources` does (src/async-bridge.js, with the
// same allowlist, URL form, and bounds).
//
// Error mapping (the Wasmtime session host's, hosts/scxml-host-wasmtime):
// * documents (`<data src>`, `<invoke src>`): a denied, unresolved, or
//   over-limit URL is `unavailable`, "not resolvable" (the machine raises
//   error.execution; for `<invoke>` the invocation fails); a resolved
//   document of any size is accepted (session_host/links.rs `load_document`);
// * scripts (`<script src>`, SCXML 5.8, loaded while the session is built):
//   denied or not found is `unavailable`, a failed fetch `platform`, an
//   invalid URI or text in another language than the data model's
//   `invalid-input`, empty text `payload-too-large` (script.rs
//   `load_inner`, `scxml_script::verify_script`); any of them rejects the
//   document. Neither host bounds script text or the distinct URIs one
//   session loads.

import { CapabilityError, ImportFailure } from './errors.js';
import { resourceUrl } from './url.js';

/** Largest script `src` URI in bytes (`scxml_script::MAX_SCRIPT_URI_BYTES`). */
export const MAX_SCRIPT_URI_BYTES = 16 * 1024;
/**
 * Largest prefetched or runtime URL in UTF-8 bytes (the script URI bound
 * of the interface); a longer one is not passed to `allow` and answers
 * `unavailable` (detail RESOURCE_DETAILS.limit). The host bounds neither how
 * many URLs one session loads nor the total text they resolve to, as the
 * Wasmtime host does not.
 */
export const MAX_RESOURCE_URL_BYTES = MAX_SCRIPT_URI_BYTES;

/** Static `component-error.detail` values of resource loads. */
export const RESOURCE_DETAILS = Object.freeze({
  /**
   * The page's `allow` did not return `true`, or the URL is an http(s) URL
   * with userinfo (never passed to `allow`).
   */
  denied: 'resource-denied',
  /**
   * The page's `resolve` rejected or answered something other than a string
   * or `{text: string, mediaType?: string}`.
   */
  unresolved: 'resource-unresolved',
  /** A URL longer than MAX_RESOURCE_URL_BYTES. */
  limit: 'resource-limit',
});

const RESOURCE_KEYS = new Set(['allow', 'resolve']);
const utf8 = new TextEncoder();
const decoder = new TextDecoder('utf-8', { fatal: true });

/** UTF-8 length of `text`, or `Infinity` once it is known to exceed `bound`. */
export function byteLength(text, bound) {
  if (text.length > bound) return Infinity;
  if (text.length * 3 <= bound) return utf8.encode(text).byteLength;
  const length = utf8.encode(text).byteLength;
  return length > bound ? Infinity : length;
}

const failure = (code, detail) => ({ version: 'v1', code, detail });

/**
 * A page's answer for one document as `{text, mediaType, baseUri,
 * eventSchemas}`, or `undefined` when it is neither a string nor
 * `{text: string, mediaType?: string, baseUri?: string, eventSchemas?:
 * Array<{uri: string, bytes: Uint8Array}>}`.
 * The media type (an HTTP `Content-Type` the page read, for example) is
 * the indication SCXML B.2.1 lets the processor interpret `<data src>`
 * content by; one not visible ASCII is dropped, as the Wasmtime host
 * drops it, and one of any length is kept. `baseUri` and `eventSchemas`
 * are the Wasmtime host's `EmbeddedDocument::base_uri` and
 * `event_schemas`, read only when the document is an `<invoke src>`
 * child: the absolute URI its own relative references resolve against
 * (RFC 3986 §5.1), and the `event:schema` files it is compiled with. A
 * `baseUri` that is not an absolute URL fails the answer.
 */
export function resolvedDocument(value) {
  if (typeof value === 'string') return Object.freeze({ text: value, mediaType: undefined, baseUri: undefined, eventSchemas: undefined });
  if (value === null || typeof value !== 'object' || typeof value.text !== 'string') return undefined;
  const { mediaType, baseUri, eventSchemas } = value;
  if (mediaType !== undefined && typeof mediaType !== 'string') return undefined;
  if (baseUri !== undefined && (typeof baseUri !== 'string' || !URL.canParse(baseUri))) return undefined;
  const schemas = documentSchemas(eventSchemas);
  if (schemas === null) return undefined;
  const kept =
    mediaType !== undefined && /^[\x20-\x7e]*$/.test(mediaType) ? mediaType : undefined;
  return Object.freeze({ text: value.text, mediaType: kept, baseUri, eventSchemas: schemas });
}

/**
 * A frozen copy of a document's `eventSchemas` answer, `undefined` when
 * absent, `null` when malformed.
 */
function documentSchemas(value) {
  if (value === undefined) return undefined;
  if (!Array.isArray(value)) return null;
  const schemas = [];
  for (const schema of value) {
    if (schema === null || typeof schema !== 'object' || typeof schema.uri !== 'string' || !(schema.bytes instanceof Uint8Array)) {
      return null;
    }
    schemas.push(Object.freeze({ uri: schema.uri, bytes: schema.bytes.slice() }));
  }
  return Object.freeze(schemas);
}

/**
 * `uri` resolved against a session's base URI (RFC 3986 §5.2, as
 * `scxml_types::resolve_reference` resolves it under the Wasmtime host);
 * as written when the session has no base URI or `uri` does not resolve.
 * @param {string | undefined} base
 * @param {string} uri
 */
export function resolveReference(base, uri) {
  if (base === undefined) return uri;
  try {
    return new URL(uri, base).href;
  } catch {
    return uri;
  }
}

/**
 * Whether `value` is an accepted `resources` option: absent, or an object
 * with exactly the functions `allow` and `resolve` (any other own key fails
 * closed, so no policy is silently ignored).
 */
export function isResourcesOption(value) {
  if (value === undefined) return true;
  if (value === null || typeof value !== 'object') return false;
  if (typeof value.allow !== 'function' || typeof value.resolve !== 'function') return false;
  return Object.keys(value).every((key) => RESOURCE_KEYS.has(key));
}

/**
 * The statically known URLs of one Machine IR, deduplicated, in IR order
 * (the order of the IR's JSON, so identical IR yields an identical list):
 * `<data src>` (a string `src` of an entry of a `data` table),
 * `<invoke src>` (a string `src` of an entry of an `invokes` table), and
 * unpinned `<script src>` (a `script` action whose `src.pinned` is absent;
 * a pinned script is a Wasm component, which the browser host does not
 * run). IR that is not UTF-8 JSON yields none: the machine rejects it.
 * The walk is iterative, so hostile nesting cannot exhaust the stack.
 * @param {Uint8Array} ir
 * @returns {string[]}
 */
export function collectUrls(ir) {
  let root;
  try {
    root = JSON.parse(decoder.decode(ir));
  } catch {
    return [];
  }
  const urls = new Set();
  const add = (value) => {
    if (typeof value === 'string') urls.add(value);
  };
  const stack = [[undefined, root]];
  while (stack.length > 0) {
    const [key, node] = stack.pop();
    if (node === null || typeof node !== 'object') continue;
    if (Array.isArray(node)) {
      if (key === 'data' || key === 'invokes') {
        for (const entry of node) {
          if (entry !== null && typeof entry === 'object' && !Array.isArray(entry)) add(entry.src);
        }
      }
      for (let index = node.length - 1; index >= 0; index -= 1) stack.push([undefined, node[index]]);
      continue;
    }
    if (node.kind === 'script' && node.src !== null && typeof node.src === 'object' && node.src.pinned == null) {
      add(node.src.uri);
    }
    const entries = Object.entries(node);
    for (let index = entries.length - 1; index >= 0; index -= 1) stack.push(entries[index]);
  }
  return [...urls];
}

/**
 * The form of `url` the page sees (`resourceUrl`, src/url.js) when the
 * page's `allow` returns exactly `true` for it; otherwise `undefined` (a
 * throw is a denial, and an http(s) URL with userinfo never reaches
 * `allow`).
 */
export function allowedUrl(resources, url) {
  const canonical = resourceUrl(url);
  if (canonical === undefined) return undefined;
  try {
    return resources.allow(canonical) === true ? canonical : undefined;
  } catch {
    return undefined;
  }
}

/**
 * Resolves every statically known URL of `ir` through `resources`, in
 * parallel, and returns the session's immutable cache, keyed by the URLs
 * as the IR spells them. Each distinct URL, an http(s) URL in its canonical
 * form (src/url.js), is passed to `allow` once (every verdict
 * synchronously, in IR order, before any `resolve`), and to `resolve` only
 * when allowed; an http(s) URL with userinfo is denied without reaching
 * `allow`. Never throws: every
 * failure becomes that URL's typed answer. Each allowed resolution is one
 * `capability` host operation (`ok`, `rejected` for a denial or a bound, or
 * `failed` when `resolve` failed); URLs are never telemetry attributes.
 * @param {Uint8Array} ir
 * @param {{allow: (url: string) => boolean, resolve: (url: string) => Promise<string | {text: string, mediaType?: string}>}} resources
 * @param {import('./telemetry.js').Telemetry} telemetry
 * @returns {Promise<ResourceCache>}
 */
export async function prefetchResources(ir, resources, telemetry) {
  return new ResourceCache(new Map(await resolveAll(collectUrls(ir), resources, telemetry)));
}

/** `[url, entry]` for each of `urls`, resolved as prefetchResources describes. */
async function resolveAll(urls, resources, telemetry) {
  const pending = urls.map((url) => {
    if (byteLength(url, MAX_RESOURCE_URL_BYTES) === Infinity) {
      telemetry.observe('capability', 'rejected');
      return { error: failure('unavailable', RESOURCE_DETAILS.limit) };
    }
    const canonical = allowedUrl(resources, url);
    if (canonical === undefined) {
      telemetry.observe('capability', 'rejected');
      return { error: failure('unavailable', RESOURCE_DETAILS.denied) };
    }
    const span = telemetry.start('capability');
    return Promise.resolve()
      .then(() => resources.resolve(canonical))
      .then(
        (answer) => {
          const document = resolvedDocument(answer);
          if (document === undefined) {
            telemetry.end(span, 'failed');
            return { error: failure('unavailable', RESOURCE_DETAILS.unresolved), fetchFailed: true };
          }
          telemetry.end(span, 'ok');
          return { ...document };
        },
        (thrown) => {
          // `unavailable` is the page saying the URL names nothing; any
          // other rejection is a failed fetch.
          const notFound = thrown instanceof CapabilityError && thrown.code === 'unavailable';
          telemetry.end(span, notFound ? 'rejected' : 'failed');
          return { error: failure('unavailable', RESOURCE_DETAILS.unresolved), fetchFailed: !notFound };
        },
      );
  });
  const settled = await Promise.all(pending);
  return urls.map((url, index) => [url, Object.freeze(settled[index])]);
}

/** Most `<invoke src>` children one session's prefetch compiles to scan. */
export const MAX_PREFETCHED_CHILDREN = 64;

/** The string `src` of every `<invoke>` of one Machine IR, in IR order. */
function invokeSources(ir) {
  let machine;
  try {
    machine = JSON.parse(decoder.decode(ir));
  } catch {
    return [];
  }
  const sources = [];
  for (const state of Array.isArray(machine?.states) ? machine.states : []) {
    for (const invoke of Array.isArray(state?.invokes) ? state.invokes : []) {
      if (typeof invoke?.src === 'string') sources.push(invoke.src);
    }
  }
  return sources;
}

/**
 * Extends `cache`, the prefetch of the root IR `ir`, with the statically
 * known URLs of every `<invoke src>` child it reaches, breadth first: each
 * resolved child document is compiled (`compile(src, entry)`, the child's
 * IR or `undefined`), and the URLs of its IR, resolved against the
 * child's `baseUri` (resolveReference), are resolved as prefetchResources
 * resolves the root's. A URL is resolved once however many documents name
 * it, and at most MAX_PREFETCHED_CHILDREN children are compiled; deeper or
 * further children load on first use, as runtime URLs do. Like the root's
 * scan, this only chooses what to prefetch.
 * @param {ResourceCache} cache
 * @param {Uint8Array} ir
 * @param {object} resources the page's `resources` option
 * @param {import('./telemetry.js').Telemetry} telemetry
 * @param {(src: string, entry: object) => Promise<Uint8Array | undefined>} compile
 * @returns {Promise<ResourceCache>}
 */
export async function prefetchInvoked(cache, ir, resources, telemetry, compile) {
  const entries = new Map(cache.entries());
  const queue = invokeSources(ir).map((src) => [undefined, src]);
  const scanned = new Set();
  while (queue.length > 0 && scanned.size < MAX_PREFETCHED_CHILDREN) {
    const [base, src] = queue.shift();
    const url = resolveReference(base, src);
    const entry = entries.get(url);
    if (scanned.has(url) || entry?.text === undefined) continue;
    scanned.add(url);
    const childIr = await compile(src, entry);
    if (childIr === undefined) continue;
    const missing = new Set(collectUrls(childIr).map((uri) => resolveReference(entry.baseUri, uri)));
    for (const known of entries.keys()) missing.delete(known);
    for (const [missed, resolved] of await resolveAll([...missing], resources, telemetry)) entries.set(missed, resolved);
    for (const child of invokeSources(childIr)) queue.push([entry.baseUri, child]);
  }
  return new ResourceCache(entries);
}

/**
 * Internal createSession option key: a ResourceCache prefetched elsewhere
 * (the page of an `async: 'auto'` worker session, src/async-bridge.js),
 * used instead of prefetching again. Not exported from the package.
 */
export const PREFETCHED = Symbol('scxml.prefetched-resources');

/** The prefetched answers of one run, shared by the root and its children. */
export class ResourceCache {
  #entries;

  /** @param {Map<string, {text?: string, mediaType?: string, error?: object, fetchFailed?: boolean}>} entries */
  constructor(entries) {
    this.#entries = entries;
  }

  /** Whether `url` was prefetched (allowed or not). */
  has(url) {
    return this.#entries.has(url);
  }

  /**
   * `load-document` for a prefetched `url` (`has(url)`): its resolved
   * document (`{text, mediaType, baseUri, eventSchemas}`, resolvedDocument),
   * or the Wasmtime host's failure as a thrown ImportFailure.
   */
  document(url) {
    const entry = this.#entries.get(url);
    if (entry.text === undefined) throw new ImportFailure(entry.error);
    return entry;
  }

  /** The prefetched entry of `url`, if any. */
  entry(url) {
    return this.#entries.get(url);
  }

  /**
   * Every `[url, entry]` pair in IR order, as structured-cloneable data:
   * `new ResourceCache(new Map(cache.entries()))` answers exactly as
   * `cache` does (an async worker session's copy of the page's prefetch).
   */
  entries() {
    return [...this.#entries];
  }
}

/**
 * Whether `uri` passes `scxml_script::validate_uri`: not blank in XML white
 * space (#x20, #x9, #xD, #xA; `String.prototype.trim` also strips other
 * Unicode white space and U+FEFF), within the byte bound, and free of
 * control characters.
 */
function validScriptUri(uri) {
  // eslint-disable-next-line no-control-regex
  return !/^[\x20\x09\x0d\x0a]*$/.test(uri) && byteLength(uri, MAX_SCRIPT_URI_BYTES) !== Infinity && !/[\u0000-\u001f\u007f-\u009f]/.test(uri);
}

/**
 * The external scripts one session loaded from the resource cache
 * (`load-script`), for `run-script`. One per session: the root and each
 * invoked child load their own, as under the Wasmtime host.
 */
export class ScriptLoads {
  #cache;
  #base;
  /** uri -> script text */
  #loaded = new Map();

  /**
   * @param {ResourceCache} cache
   * @param {string} [base] the session's base URI, which relative script
   *   URIs resolve against (resolveReference)
   */
  constructor(cache, base) {
    this.#cache = cache;
    this.#base = base;
  }

  /** Whether this cache answers `reference` (an unpinned, prefetched URI). */
  serves(reference) {
    return reference.pinned === undefined && this.#cache.has(resolveReference(this.#base, reference.uri));
  }

  /**
   * `load-script` of an unpinned reference the cache answers, with the
   * Wasmtime host's checks and codes (`load_inner`); throws ImportFailure.
   * @param {{uri: string}} reference
   * @param {string | undefined} dataModelName the session's data model
   */
  load(reference, dataModelName) {
    const { uri } = reference;
    if (!validScriptUri(uri)) {
      throw new ImportFailure(failure('invalid-input', 'script src URI is empty, oversized, or malformed'));
    }
    if (this.#loaded.has(uri)) return;
    const entry = this.#cache.entry(resolveReference(this.#base, uri));
    if (entry.text === undefined) {
      if (entry.fetchFailed) throw new ImportFailure(failure('platform', 'script loader failed'));
      throw new ImportFailure(failure('unavailable', 'script loader did not provide the script'));
    }
    // SCXML 5.8: the data model defines the scripting language; resolved
    // text is script text of the active data model, so only ECMAScript
    // sessions accept it (`scxml_script::data_model_script_media_types`).
    if (dataModelName !== 'ecmascript') {
      throw new ImportFailure(failure('invalid-input', 'script media type is not supported by the data model'));
    }
    if (entry.text.length === 0) {
      throw new ImportFailure(failure('payload-too-large', 'script bytes are empty or exceed their bound'));
    }
    this.#loaded.set(uri, entry.text);
  }

  /** The text `load` accepted for an unpinned `reference`, if any. */
  text(reference) {
    return reference.pinned === undefined ? this.#loaded.get(reference.uri) : undefined;
  }
}
