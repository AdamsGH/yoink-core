// Internal synthetic fixture only. One bounded request, no credential inspection.
export function createYoutubeAuthGate({ timeoutMs = 90000 } = {}) {
  if (!Number.isInteger(timeoutMs) || timeoutMs < 10 || timeoutMs > 90000) throw Error('invalid fixture deadline');
  let gate;
  function finish(allowed) {
    if (!gate) return;
    clearTimeout(gate.timer);
    gate.resolve?.(allowed);
    gate = undefined;
  }
  return {
    close() { finish(false); },
    async handle(req, res, next) {
      const url = new URL(req.url, 'http://fixture');
      if (url.pathname.startsWith('/__fixture/')) {
        const action = url.pathname.slice('/__fixture/'.length);
        const route = url.searchParams.get('route');
        const invalid = req.method !== 'POST' || req.headers['transfer-encoding'] || Number(req.headers['content-length'] ?? 0) !== 0 ||
          [...url.searchParams.keys()].some(k => k !== 'route') || url.searchParams.getAll('route').length > 1;
        if (invalid) { res.writeHead(400).end(); req.resume(); return; }
        let status = 200;
        if (action === 'arm' && ['info', 'download'].includes(route)) {
          if (gate) status = 409;
          else {
            gate = { route, entered: false };
            gate.timer = setTimeout(() => finish(false), timeoutMs);
          }
        } else if (action === 'state' && !route) {
          // State carries neither request URL nor body.
        } else if (['release', 'reset'].includes(action) && !route) finish(action === 'release');
        else status = 400;
        res.writeHead(status, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ armed: !!gate, entered: gate?.entered ?? false }));
        return;
      }
      if (gate && !gate.entered && req.method === 'POST' && url.pathname === `/${gate.route}`) {
        gate.entered = true;
        const closed = () => finish(false);
        res.once('close', closed);
        const allowed = await new Promise(resolve => { gate.resolve = resolve; });
        res.removeListener('close', closed);
        if (!allowed || res.destroyed) { if (!res.destroyed) res.writeHead(504).end(); req.resume(); return; }
      }
      next();
    },
  };
}
