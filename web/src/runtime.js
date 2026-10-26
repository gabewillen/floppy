/* The workshop's runtime: scxml.rs's browser host (host/, built by
   web/build-host.sh) validating, compiling and running a document in the page
   with the same Wasm components `scx` runs. The workshop never selects a
   transition: the configuration, the enabled transitions, the data model and
   the delayed sends it shows are read from the session's snapshot.

   packages.json (web/build.py) lists the namespace packages the components
   implement: the `scx` namespace and floppy's namespaces, each with its
   manifest fields, its schema file and its component file. services.json
   lists the service packages a page can run (all but pgsql, which needs
   sockets), each with its component file and the directory `scx run`'s
   `--dir` grants it, and the environment variables the documents bind to
   service slots.

   Services run as under `scx run`: each sees only the values the page sets
   (environment, held in this page's memory only, never stored), as
   references it can use in its own requests; its requests go through this
   page's fetch (so the server must allow this site's origin); and its
   directory is an in-memory one that lives as long as the page. Services
   need JavaScript Promise Integration (Chrome 137 or later). */
import {
  ExtensionComponents,
  ServiceComponents,
  authenticateExtensionComponent,
  authenticateServiceComponent,
  createCompiler,
  createEcmaScriptDataModel,
  createSession,
  createValidator,
} from "./host/src/index.js";

const HERE = new URL(".", import.meta.url);
const SITE = new URL("../", HERE);
const decoder = new TextDecoder();
const bytesOf = async url => { const r = await fetch(url, { credentials: "omit" }); if (!r.ok) throw new Error(`${url}: HTTP ${r.status}`); return new Uint8Array(await r.arrayBuffer()); };
const hex = async bytes => [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))].map(b => b.toString(16).padStart(2, "0")).join("");

/* The environment variables' values the services may use, by name: set by
   the TestBench, kept in this page only. */
export const environment = new Map();
/* The in-memory directories the services are granted, by guest path
   (shared, as `scx run` grants the same host directory to several). */
const directories = new Map();

let ready = null;
/* Loads the components once: validator, compiler, every bound namespace
   and every service. */
export function load() {
  ready ??= (async () => {
    const index = await (await fetch(new URL("packages.json", HERE))).json();
    const packages = [], extensions = new ExtensionComponents();
    for (const p of index.packages) {
      const pkg = { ...p.manifest, schema: await bytesOf(new URL(p.schema, HERE)) };
      packages.push(pkg);
      const bytes = await bytesOf(new URL(p.component, HERE));
      extensions.bind(pkg, await authenticateExtensionComponent(bytes, await hex(bytes)));
    }
    const catalog = await (await fetch(new URL("services.json", HERE))).json();
    const services = [];
    for (const s of catalog.services) {
      const bytes = await bytesOf(new URL(s.component, HERE));
      services.push({ manifest: s.manifest, dir: s.dir, component: await authenticateServiceComponent(bytes, await hex(bytes)) });
    }
    const [validator, compiler] = await Promise.all([createValidator({ dataModel: await createEcmaScriptDataModel() }), createCompiler()]);
    return { packages, extensions, services, variables: catalog.variables, validator, compiler, namespaces: index.packages.map(p => p.manifest.namespaceUri) };
  })();
  return ready;
}

/* The `event:schema` files a document references, fetched relative to its URL. */
async function eventSchemas(text, base) {
  const refs = new Set([...text.matchAll(/event:schema="([^"]+)"/g)].map(m => m[1]).filter(r => !r.trim().startsWith("{")));
  return Promise.all([...refs].map(async uri => ({ uri, bytes: await bytesOf(new URL(uri, base)) })));
}

/* The environment variables documents bind to service slots. */
export async function variables() { return (await load()).variables; }

/* Whether this browser can run services (JavaScript Promise Integration). */
export const servicesSupported = typeof WebAssembly.Suspending === "function";

/* The services one run binds: every service, the environment's values
   (granted to each service, used only as references), this page's fetch,
   and each service's in-memory directory. */
function serviceComponents(rt) {
  const services = new ServiceComponents();
  for (const s of rt.services) services.bind(s.manifest, s.component);
  const uris = rt.services.map(s => s.manifest.serviceUri);
  services.withValues([...environment].filter(([, value]) => value !== "").map(([name, value]) => ({ name, value, packages: uris })));
  services.withOutboundHttp({ fetch: (url, init) => fetch(url, init) });
  for (const s of rt.services) {
    if (!s.dir) continue;
    if (!directories.has(s.dir)) directories.set(s.dir, { dir: {} });
    services.withWasi(s.manifest.serviceUri, { dirs: [[directories.get(s.dir), s.dir]] });
  }
  return services;
}

/* Diagnostics of a canonical scxml.report/v3 report. */
function diagnostics(report) {
  const doc = JSON.parse(decoder.decode(report));
  return (doc.diagnostics ?? []).map(d => ({ code: d.code, severity: d.severity, message: d.message, line: d.span?.start?.line ?? d.location?.line ?? null, help: d.help ?? null }));
}

/* Validates and compiles `text` (the document at `url`, for its relative references).
   Returns {ok, diagnostics, ir?}. */
export async function compile(text, url) {
  const rt = await load();
  const base = new URL(url, SITE), schemas = await eventSchemas(text, base);
  const options = { packages: rt.packages, eventSchemas: schemas };
  const { report: validation } = await rt.validator.validate(text, options);
  const compiled = await rt.compiler.compile(text, { ...options, sourceName: base.pathname.split("/").pop() || "bot.scx" });
  const found = [...diagnostics(validation), ...diagnostics(compiled.report)];
  const seen = new Set(), unique = found.filter(d => { const k = `${d.code}|${d.line}|${d.message}`; return !seen.has(k) && seen.add(k); });
  return { ok: compiled.status === "emitted", diagnostics: unique, ir: compiled.ir, base };
}

/* One running bot. `onChange(view)` is called after every change, with the
   session's view: {status, atomic, full, transitions, data, pending, invocations, log}. */
export class Run {
  constructor(session, onChange) { this.session = session; this.onChange = onChange; this.log = []; this.failure = null; }

  static async start(compiled, onChange) {
    const rt = await load();
    const origin = SITE.origin;
    let run;
    const say = (kind, text) => { run.log.push({ kind, text, at: performance.now() }); if (run.log.length > 400) run.log.shift(); };
    const session = await createSession({
      ir: compiled.ir,
      dataModel: await createEcmaScriptDataModel(),
      extensionComponents: rt.extensions,
      serviceComponents: serviceComponents(rt),
      capabilities: { log: ({ label, text }) => say("log", label ? `${label}: ${text}` : text) },
      // The documents and scripts the bot loads come from this site only. A
      // relative name of the bot reaches the page as the document wrote it,
      // and names a file beside the document (compiled.base), as under `scx
      // run`. Each document answers with its own URL as its base, so an
      // invoked child's names arrive resolved against the child, and with
      // the event:schema files it is compiled with.
      resources: {
        allow: url => { try { return new URL(url, compiled.base).origin === origin; } catch { return false; } },
        resolve: async url => {
          const at = new URL(url, compiled.base);
          const r = await fetch(at, { credentials: "omit", redirect: "error" });
          if (!r.ok) throw new Error(`HTTP ${r.status}`);
          const text = await r.text();
          return { text, mediaType: r.headers.get("content-type") ?? undefined, baseUri: at.href, eventSchemas: text.includes("event:schema=") ? await eventSchemas(text, at) : undefined };
        },
      },
      timers: "event-loop",
      onTimer: outcome => { if (outcome.error) run.fail(outcome.error); else run.changed(); },
      trace: true,
      sessionId: "workshop",
    });
    run = new Run(session, onChange);
    run.say = say;
    try { session.start(); } catch (e) { run.fail(e); }
    await run.changed();
    return run;
  }

  fail(e) {
    this.failure = e?.code ? `${e.code}${e.detail ? ": " + e.detail : ""}` : String(e?.message ?? e);
    this.say("error", this.failure);
    // A page holds about 125 Wasm memories, and every running session takes
    // about 20 (its machine, data model, I/O processor, clock and
    // namespaces), so a bot that runs more than a few sessions at once
    // loses an instance here (stateforward/scxml.rs#33).
    if (e?.code === "instance-lost") this.say("error", "This bot most likely runs more sessions at once than a browser page has memory for. Run it with scx run instead.");
    this.changed();
  }

  /* Sends one external event: {name, data?}. */
  async send(name, data) {
    if (this.failure) return;
    try { this.session.send(data === undefined ? { name } : { name, data }); } catch (e) { this.fail(e); return; }
    await this.changed();
  }

  async changed() {
    const view = await this.view();
    this.onChange?.(view);
    return view;
  }

  #traced = 0;
  async view() {
    let snap = null;
    if (!this.failure) {
      try { snap = JSON.parse(decoder.decode(await this.session.snapshot({ exclude: ["queues"] }))); } catch (e) {
        // A session holding a live service cannot be snapshotted (as under
        // scx): the configuration and status are read on their own then.
        if (e?.code === "unavailable") {
          try { const atomic = this.session.configuration(); snap = { root: { status: this.session.status(), configuration: { atomic, full: atomic } } }; } catch (e2) { this.failure ??= e2?.code ?? String(e2); }
        } else this.failure ??= e?.code ?? String(e);
      }
    }
    try {
      const entries = this.session.trace().entries;
      for (const t of entries.slice(this.#traced)) {
        const who = t.session === "workshop" ? "" : `${t.session}: `;
        if (t.observation === "raised") this.say("out", `${who}raised ${t.name}`);
        else if (t.observation === "processed") this.say("in", `${who}${t.name}`);
        else if (t.observation === "log" && who) this.say("log", `${who}${t.label ? t.label + ": " : ""}${t.text}`);
      }
      this.#traced = entries.length;
    } catch { /* a lost session has no trace */ }
    const root = snap?.snapshot?.root ?? snap?.root;
    return {
      status: this.failure ? "failed" : root?.status ?? "failed",
      failure: this.failure,
      atomic: root?.configuration?.atomic ?? [],
      full: root?.configuration?.full ?? [],
      transitions: (root?.transitions ?? []).filter(t => t.events?.length),
      data: (root?.data_model ?? []).map(d => ({ name: d.name, value: show(d.value) })),
      pending: root?.pending_sends ?? [],
      invocations: root?.invocations ?? [],
      log: this.log,
    };
  }

  dispose() { try { this.session.dispose(); } catch { /* already lost */ } }
}

function show(v) {
  if (!v) return "";
  if (v.type === "text") return v.typeof ? v.text : JSON.stringify(v.value);
  if (v.type === "oversized") return `<${v.bytes} bytes>`;
  if (v.type === "absent" || v.type === "undefined" || v.type === "null") return v.type;
  return typeof v.value === "string" ? v.value : JSON.stringify(v.value);
}
