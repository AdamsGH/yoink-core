import http from 'node:http';
import { pathToFileURL } from 'node:url';

// Synthetic v2 transport, not PoToken issuance. Disconnect deliberately does not
// cancel dispatched work, matching the gateway's conservative slot contract.
export function createSyntheticBgutil({ timeoutMs = 90000 } = {}) {
  if (!Number.isInteger(timeoutMs) || timeoutMs < 10 || timeoutMs > 90000) throw Error('invalid fixture deadline');
  let armed = false, armTimer, entered = false, closed = false;
  const jobs = new Set();
  const clearArm = () => { armed = false; clearTimeout(armTimer); };
  function release(allowed) {
    clearArm();
    entered = false;
    for (const job of jobs) { clearTimeout(job.timer); job.resolve(allowed); }
  }
  const handlers = new Set();
  async function handle(req, res) {
    if (closed) { res.writeHead(503).end(); return; }
    if (req.url === '/ping' && req.method === 'GET') {
      res.writeHead(200, { 'Content-Type': 'application/json' }).end('{"version":"2.0.0"}'); return;
    }
    if (req.url.startsWith('/__fixture/')) {
      const action = req.url.slice('/__fixture/'.length);
      if (req.method !== 'POST' || req.headers['transfer-encoding'] || Number(req.headers['content-length'] ?? 0) !== 0) {
        res.writeHead(400).end(); req.resume(); return;
      }
      let status = 200;
      if (action === 'arm') {
        if (armed || jobs.size >= 2) status = 409;
        else { armed = true; entered = false; armTimer = setTimeout(clearArm, timeoutMs); }
      } else if (action === 'state') {
        // No provider request payload is retained in fixture state.
      } else if (['release', 'reset'].includes(action)) release(action === 'release');
      else status = 400;
      res.writeHead(status, { 'Content-Type': 'application/json' }).end(JSON.stringify({ armed, entered, active: jobs.size })); return;
    }
    if (req.url !== '/get_pot' || req.method !== 'POST') { res.writeHead(404).end(); req.resume(); return; }
    if (jobs.size >= 2) { res.writeHead(503).end(); req.resume(); return; }
    // Parse no more than the adapter's bound. The synthetic binding is fixed,
    // so arbitrary incoming credential/context fields never become evidence.
    let bytes = 0, body = '';
    const timer = setTimeout(() => req.destroy(), Math.min(timeoutMs, 5000));
    try {
      for await (const chunk of req) {
        bytes += chunk.length;
        if (bytes > 65536) { res.writeHead(413).end(); return; }
        body += chunk;
      }
    } finally { clearTimeout(timer); }
    const payload = JSON.parse(body);
    if (payload.content_binding !== 'abcdefghijk' || payload.proxy || payload.source_address || payload.disable_tls_verification) {
      res.writeHead(400).end(); return;
    }
    if (armed) {
      clearArm(); entered = true;
      const job = {};
      const done = new Promise(resolve => { job.resolve = resolve; });
      job.timer = setTimeout(() => job.resolve(false), timeoutMs);
      jobs.add(job);
      try {
        if (!await done) { if (!res.destroyed) res.writeHead(504).end(); return; }
      } finally { clearTimeout(job.timer); jobs.delete(job); }
    }
    if (!res.destroyed) res.writeHead(200, { 'Content-Type': 'application/json' }).end('{"poToken":"synthetic-pot"}');
  }
  let activeRequests = 0;
  const server = http.createServer((req, res) => {
    const pot = req.url === '/get_pot';
    if (pot && activeRequests >= 2) { res.writeHead(503).end(); req.resume(); return; }
    if (pot) activeRequests++;
    const handler = handle(req, res).catch(() => {
      if (!res.destroyed) { if (!res.headersSent) res.writeHead(400).end(); else res.destroy(); }
    }).finally(() => { if (pot) activeRequests--; });
    handlers.add(handler);
    handler.finally(() => handlers.delete(handler));
  });
  server.requestTimeout = 5000;
  server.headersTimeout = 5000;
  return {
    server,
    async close() {
      closed = true; release(false);
      server.closeAllConnections();
      await new Promise((resolve, reject) => server.close(error => error && error.code !== 'ERR_SERVER_NOT_RUNNING' ? reject(error) : resolve()));
      await Promise.all(handlers);
    },
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const fixture = createSyntheticBgutil();
  fixture.server.listen(4416, '0.0.0.0');
  for (const signal of ['SIGTERM', 'SIGINT']) process.once(signal, () => { fixture.close().catch(() => { process.exitCode = 1; }); });
}
