// A Web Worker stand-in for Node tests. It runs a module in a real
// `worker_threads` thread and gives it the `self` message surface a
// dedicated Worker has (addEventListener('message'), postMessage), so the
// async bridge's worker mechanism can run in Node: its thread really
// blocks in `Atomics.wait` on the shared channel.
//
// The main thread installs `NodeWorker` as `globalThis.Worker` and sets
// `globalThis.crossOriginIsolated`. The same file is the thread's entry
// when it runs with `workerData.module`.
import { isMainThread, parentPort, Worker as ThreadWorker, workerData } from 'node:worker_threads';

export class NodeWorker extends EventTarget {
  #thread;

  constructor(url, options) {
    super();
    if (options?.type !== 'module') throw new TypeError('the async bridge uses module workers');
    this.#thread = new ThreadWorker(new URL(import.meta.url), { workerData: { module: String(url) } });
    this.#thread.on('message', (data) => this.dispatchEvent(new MessageEvent('message', { data })));
    this.#thread.on('error', () => this.dispatchEvent(new Event('error')));
  }

  postMessage(message) {
    this.#thread.postMessage(message);
  }

  terminate() {
    this.#thread.terminate();
  }
}

if (!isMainThread && workerData?.module !== undefined) {
  globalThis.self = {
    addEventListener(type, listener) {
      if (type === 'message') parentPort.on('message', (data) => listener({ data }));
    },
    postMessage(message) {
      parentPort.postMessage(message);
    },
  };
  await import(workerData.module);
}
