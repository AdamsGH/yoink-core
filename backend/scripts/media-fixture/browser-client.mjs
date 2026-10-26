import { spawn } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { isAbsolute, join } from 'node:path';

const MAX_FRAME = 32 * 1024 * 1024;
// These transports are not consistently covered by CDP URL interception.
const TRANSPORT_GUARD = `for (const name of ['WebSocket', 'WebTransport', 'RTCPeerConnection', 'webkitRTCPeerConnection']) {
  Object.defineProperty(globalThis, name, { configurable: false, writable: false,
    value: class { constructor() { throw new Error('Fixture network transport disabled'); } } });
}`;
const bounded = (value, name) => {
  if (!Number.isInteger(value) || value < 1 || value > 120000) throw new Error(`Invalid ${name}`);
  return value;
};
const failure = message => new Error(message);

/** Null-delimited CDP transport. Errors deliberately exclude protocol payloads. */
export class CdpPipe extends EventEmitter {
  constructor(readable, writable, { commandTimeoutMs = 10000 } = {}) {
    super();
    this.timeout = bounded(commandTimeoutMs, 'command timeout');
    this.pending = new Map();
    this.nextId = 0;
    this.buffer = Buffer.alloc(0);
    this.bytesReceived = 0;
    this.failure = null;
    this.readable = readable;
    this.writable = writable;
    readable.on('data', chunk => this.consume(chunk));
    readable.on('end', () => this.fail(failure('CDP pipe ended')));
    readable.on('error', () => this.fail(failure('CDP pipe failed')));
    writable.on('error', () => this.fail(failure('CDP pipe failed')));
  }

  request(method, params = {}, sessionId, { timeoutMs = this.timeout } = {}) {
    bounded(timeoutMs, 'command timeout');
    if (!/^[A-Za-z]+\.[A-Za-z0-9]+$/.test(method)) return Promise.reject(failure('Invalid CDP method'));
    if (this.failure) return Promise.reject(this.failure);
    const id = ++this.nextId;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(failure(`CDP command timed out: ${method}`));
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer, method });
      try {
        this.writable.write(`${JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) })}\0`, error => {
          if (error) this.fail(failure('CDP pipe write failed'));
        });
      } catch {
        this.fail(failure('CDP pipe write failed'));
      }
    });
  }

  consume(chunk) {
    if (this.failure) return;
    this.bytesReceived += chunk.length;
    this.buffer = Buffer.concat([this.buffer, chunk]);
    for (;;) {
      const end = this.buffer.indexOf(0);
      if (end < 0) break;
      if (end > MAX_FRAME) return this.fail(failure('CDP frame exceeds bound'));
      let message;
      try { message = JSON.parse(this.buffer.subarray(0, end).toString('utf8')); }
      catch { return this.fail(failure('Invalid CDP frame')); }
      this.buffer = this.buffer.subarray(end + 1);
      if (!message || typeof message !== 'object') return this.fail(failure('Invalid CDP frame'));
      if (message.id !== undefined) {
        const pending = this.pending.get(message.id);
        if (!pending) continue; // A late response to a timed-out command is harmless.
        this.pending.delete(message.id);
        clearTimeout(pending.timer);
        if (message.error) {
          const code = Number.isSafeInteger(message.error.code) ? ` (${message.error.code})` : '';
          const error = failure(`CDP command failed: ${pending.method}${code}`);
          error.interceptionExpired = ['Fetch.continueRequest', 'Fetch.failRequest'].includes(pending.method)
            && message.error.code === -32602 && ['Invalid InterceptionId.', 'Invalid InterceptionId'].includes(message.error.message);
          pending.reject(error);
        }
        else pending.resolve(message.result ?? {});
      } else if (typeof message.method === 'string') {
        this.emit('event', message);
      }
    }
    if (this.buffer.length > MAX_FRAME) this.fail(failure('CDP frame exceeds bound'));
  }

  fail(error = failure('CDP client closed')) {
    if (this.failure) return;
    this.failure = error;
    this.buffer = Buffer.alloc(0);
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(error);
    }
    this.pending.clear();
    this.emit('failure', error);
  }
}

/** Only the configured HTTP(S) origin plus inert/local schemes are permitted. */
export function isAllowedRequest(url, origin) {
  try {
    const parsed = new URL(url);
    if (parsed.protocol === 'about:') return parsed.href === 'about:blank' || parsed.href === 'about:srcdoc';
    if (parsed.protocol === 'data:' || parsed.protocol === 'blob:') return true;
    return (parsed.protocol === 'https:' || parsed.protocol === 'http:') && parsed.origin === origin;
  } catch { return false; }
}

/**
 * Launch an owned Linux Chromium process group with an isolated temporary profile.
 * origin is required (an exact HTTP(S) origin, not a URL with a path).
 * certificateSpki is an optional base64 SHA-256 SPKI hash (not a certificate hash).
 * No blanket certificate bypass, no debugging listener and no sandbox bypass.
 * Returned request defaults to the attached page session; browserRequest does not.
 * Profiles use profileParentDirectory and are removed on close or startup failure.
 * startupTimeoutMs covers launch and initial CDP setup. commandTimeoutMs applies
 * after the browser is ready, so it cannot prematurely cancel a cold launch.
 * Requires Chromium 151's ordered Network.setBlockedURLs allow/block patterns.
 * HTTP(S) resource requests are origin-restricted. WebSocket/WebTransport/WebRTC
 * constructors are disabled in page and worker contexts by a bootstrap guard.
 * This is a cooperative fixture guard, not an OS network isolation boundary.
 * Low-level callers must not disable interception/network rules or detach targets.
 */
export async function launchBrowser({
  origin,
  certificateSpki,
  bootstrapScript = '',
  executablePath = process.env.YOINK_FIXTURE_CHROMIUM ?? '/usr/bin/chromium',
  profileParentDirectory = tmpdir(),
  startupTimeoutMs = 15000,
  commandTimeoutMs = 10000,
  shutdownTimeoutMs = 3000,
} = {}) {
  let parsed;
  try { parsed = new URL(origin); }
  catch { throw failure('Expected an exact HTTP(S) fixture origin'); }
  if (!['http:', 'https:'].includes(parsed.protocol) || parsed.origin !== origin || parsed.username || parsed.password) {
    throw failure('Expected an exact HTTP(S) fixture origin');
  }
  if (certificateSpki !== undefined && !/^[A-Za-z0-9+/]{43}=$/.test(certificateSpki)) throw failure('Invalid certificate SPKI hash');
  if (typeof bootstrapScript !== 'string') throw failure('Invalid bootstrap script');
  if (typeof executablePath !== 'string' || !isAbsolute(executablePath)) throw failure('Expected an absolute browser executable');
  if (typeof profileParentDirectory !== 'string' || !isAbsolute(profileParentDirectory)) throw failure('Expected an absolute profile parent directory');
  bounded(startupTimeoutMs, 'startup timeout');
  bounded(commandTimeoutMs, 'command timeout');
  bounded(shutdownTimeoutMs, 'shutdown timeout');
  const profileDirectory = await mkdtemp(join(profileParentDirectory, 'yoink-browser-'));
  const startedAt = performance.now();
  const stderrCategories = new Set();
  const recordStderr = chunk => {
    const text = chunk.toString();
    for (const [name, pattern] of [
      ['sandbox', /sandbox|namespace/i], ['dbus', /dbus|bus\.cc/i],
      ['gpu', /gpu|gl_factory|viz_main/i], ['memory', /out of memory|oom|allocation failed/i],
      ['profile', /singleton|profile.*lock/i], ['crashpad', /crashpad/i],
    ]) if (pattern.test(text)) stderrCategories.add(name);
  };
  let stage = 'spawn';
  let child, pipe, closing, timer;
  let exited = false;
  const signalGroup = signal => {
    if (!child?.pid) return;
    try { process.kill(-child.pid, signal); }
    catch (error) { if (error.code !== 'ESRCH') throw failure('Browser process cleanup failed'); }
  };
  const close = () => {
    if (closing) return closing;
    closing = (async () => {
      clearTimeout(timer);
      try {
        if (pipe && !pipe.failure && !exited) {
          await pipe.request('Browser.close', {}, undefined, { timeoutMs: Math.min(500, shutdownTimeoutMs) }).catch(() => {});
        }
        pipe?.fail();
        signalGroup('SIGTERM');
        if (child && !exited) {
          await new Promise(resolve => {
            const done = () => { clearTimeout(stop); child.off('exit', done); resolve(); };
            const stop = setTimeout(done, shutdownTimeoutMs);
            child.once('exit', done);
          });
        }
        // Kill any descendants still in our owned process group, even if the parent exited.
        signalGroup('SIGKILL');
        if (child && !exited) {
          await new Promise((resolve, reject) => {
            const done = () => { clearTimeout(stop); child.off('exit', done); resolve(); };
            const stop = setTimeout(() => {
              child.off('exit', done);
              reject(failure('Browser shutdown timed out'));
            }, shutdownTimeoutMs);
            child.once('exit', done);
          });
        }
      } finally {
        pipe?.readable.destroy();
        pipe?.writable.destroy();
        child?.stderr?.destroy();
        await rm(profileDirectory, { recursive: true, force: true });
      }
    })();
    return closing;
  };
  try {
    const args = [
      '--headless=new', '--remote-debugging-pipe', `--user-data-dir=${profileDirectory}`,
      '--no-startup-window', '--no-first-run', '--no-default-browser-check',
      '--disable-background-networking', '--disable-component-update', '--disable-sync',
      '--disable-domain-reliability', '--disable-default-apps', '--disable-extensions',
      '--disable-client-side-phishing-detection', '--disable-breakpad', '--disable-quic',
      '--no-pings', '--no-proxy-server', '--metrics-recording-only', '--password-store=basic',
      '--disable-features=MediaRouter,OptimizationHints,AutofillServerCommunication,CertificateTransparencyComponentUpdater',
      ...(certificateSpki ? [`--ignore-certificate-errors-spki-list=${certificateSpki}`] : []),
    ];
    child = spawn(executablePath, args, { detached: true, stdio: ['ignore', 'ignore', 'pipe', 'pipe', 'pipe'] });
    child.stderr.on('data', recordStderr);
    child.stderr.on('error', () => { stderrCategories.add('unavailable'); });
    pipe = new CdpPipe(child.stdio[4], child.stdio[3], { commandTimeoutMs: startupTimeoutMs });
    child.once('error', () => { exited = true; pipe.fail(failure('Browser launch failed')); });
    child.once('exit', () => { exited = true; pipe.fail(failure('Browser exited')); });
    pipe.on('failure', () => { void close().catch(() => {}); });
    const sessions = new Map();
    const setup = async (sessionId, type) => {
      const send = (method, params) => pipe.request(method, params, sessionId);
      // Recursively cover frames, popups and worker targets before they execute.
      await send('Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: true, flatten: true });
      await send('Network.enable');
      // Chromium 151 supports ordered allow/block patterns. This also covers
      // workers, whose CDP agents do not implement Fetch interception.
      await send('Network.setBlockedURLs', {
        urlPatterns: [
          { urlPattern: `${origin}/*`, block: false },
          { urlPattern: 'about\\:blank', block: false },
          { urlPattern: 'about\\:srcdoc', block: false },
          { urlPattern: 'data:*', block: false },
          { urlPattern: 'blob:*', block: false },
          { urlPattern: '*://*:*/*', block: true },
        ],
        // Defense in depth only: Chromium does not consistently apply this to
        // WebSocket handshakes, so the script guard also disables constructors.
        urls: ['ws://*', 'wss://*'],
      });
      if (type === 'page' || type === 'iframe') {
        await send('Fetch.enable', { patterns: [{ urlPattern: '*', requestStage: 'Request' }] });
        await send('Network.setBypassServiceWorker', { bypass: true });
        await send('Page.enable');
        await send('Page.setLifecycleEventsEnabled', { enabled: true });
        await send('Page.addScriptToEvaluateOnNewDocument', { source: `${TRANSPORT_GUARD}\n${bootstrapScript}` });
      }
      // Cover the initial blank document and paused workers as well.
      const guarded = await send('Runtime.evaluate', { expression: TRANSPORT_GUARD });
      if (guarded.exceptionDetails) throw failure('Browser transport guard failed');
      await send('Runtime.runIfWaitingForDebugger');
    };
    pipe.on('event', event => {
      if (event.method === 'Target.attachedToTarget') {
        const { sessionId, targetInfo } = event.params;
        const ready = setup(sessionId, targetInfo.type);
        sessions.set(targetInfo.targetId, { sessionId, ready });
        ready.catch(error => pipe.fail(error));
      } else if (event.method === 'Fetch.requestPaused') {
        const { requestId, request } = event.params;
        const allowed = isAllowedRequest(request.url, origin);
        void pipe.request(allowed ? 'Fetch.continueRequest' : 'Fetch.failRequest',
          { requestId, ...(allowed ? {} : { errorReason: 'BlockedByClient' }) }, event.sessionId)
          .catch(error => {
            // Navigation can cancel a paused request before its interception reply arrives.
            if (!error.interceptionExpired) pipe.fail(failure(`Browser request restriction failed: ${error.message}`));
          });
      }
    });
    const initialize = async () => {
      stage = 'version';
      const version = await pipe.request('Browser.getVersion');
      const major = Number(/\/(\d+)\./.exec(version.product ?? '')?.[1]);
      if (!Number.isInteger(major) || major < 151) throw failure('Chromium 151 or newer is required');
      stage = 'target';
      await pipe.request('Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: true, flatten: true });
      const { targetId } = await pipe.request('Target.createTarget', { url: 'about:blank' });
      const attached = sessions.get(targetId);
      if (!attached) throw failure('Browser page attachment failed');
      stage = 'page-setup';
      await attached.ready;
      return { targetId, sessionId: attached.sessionId };
    };
    const startupDeadline = new Promise((_, reject) => {
      timer = setTimeout(() => reject(failure('Browser startup timed out')), startupTimeoutMs);
    });
    const ready = await Promise.race([initialize(), startupDeadline]);
    clearTimeout(timer);
    pipe.timeout = commandTimeoutMs;
    child.stderr.off('data', recordStderr);
    child.stderr.resume();
    const request = (method, params = {}, sessionId = ready.sessionId, options) => pipe.request(method, params, sessionId, options);
    return {
      ...ready, profileDirectory, pid: child.pid, close,
      request,
      browserRequest: (method, params = {}, options) => pipe.request(method, params, undefined, options),
      async evaluate(expression, { timeoutMs = commandTimeoutMs } = {}) {
        const result = await request('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }, ready.sessionId, { timeoutMs });
        if (result.exceptionDetails) throw failure('Browser evaluation failed');
        return result.result.value;
      },
      async navigate(url, { timeoutMs = commandTimeoutMs } = {}) {
        bounded(timeoutMs, 'navigation timeout');
        if (!isAllowedRequest(url, origin)) throw failure('Navigation outside fixture origin blocked');
        const loaded = new Set();
        let loaderId, resolveLoad;
        const listener = event => {
          if (event.sessionId === ready.sessionId && event.method === 'Page.lifecycleEvent' && event.params.name === 'load') {
            loaded.add(event.params.loaderId);
            if (event.params.loaderId === loaderId) resolveLoad();
          }
        };
        pipe.on('event', listener);
        let navigationTimer, onFailure;
        const load = new Promise(resolve => { resolveLoad = resolve; });
        try {
          return await Promise.race([
            (async () => {
              const result = await request('Page.navigate', { url }, ready.sessionId, { timeoutMs });
              if (result.errorText) throw failure('Browser navigation failed');
              loaderId = result.loaderId;
              if (loaderId && !loaded.has(loaderId)) await load;
              return result;
            })(),
            new Promise((_, reject) => { navigationTimer = setTimeout(() => reject(failure('Browser navigation timed out')), timeoutMs); }),
            new Promise((_, reject) => {
              // Removed in finally, including normal successful navigation.
              onFailure = reject;
              pipe.once('failure', onFailure);
              if (pipe.failure) reject(pipe.failure);
            }),
          ]);
        } finally {
          clearTimeout(navigationTimer);
          pipe.off('event', listener);
          if (onFailure) pipe.off('failure', onFailure);
        }
      },
    };
  } catch (error) {
    const processState = !child?.pid ? 'not-started' : exited ? 'exited' : 'running';
    error.cause ??= failure(`Browser startup: stage=${stage}, elapsed=${Math.round(performance.now() - startedAt)}ms, process=${processState}, exit=${child?.exitCode ?? child?.signalCode ?? 'none'}, received=${pipe?.bytesReceived ?? 0}, stderr=${[...stderrCategories].sort().join('|') || 'none'}`);
    pipe?.fail(failure('Browser startup failed'));
    await close();
    throw error;
  }
}
