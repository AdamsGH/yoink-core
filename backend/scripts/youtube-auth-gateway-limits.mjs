// Preload the reviewed adapter without changing its route/origin/security contract.
// Admission is before JSON buffering. Only the dedicated gateway loads this module.
import http from 'node:http';
function integer(key, fallback, min, max) {
  const text = process.env[key] ?? String(fallback);
  if (!/^\d+$/.test(text) || Number(text) < min || Number(text) > max) throw new Error(`invalid ${key}`);
  return Number(text);
}
const limit = integer('GATEWAY_MAX_CONCURRENCY', 2, 1, 16);
integer('GATEWAY_MAX_QUEUE', 0, 0, 0);
const deadline = integer('GATEWAY_DEADLINE_MS', 250000, 10, 270000);
const createServer = http.createServer;
http.createServer = function (listener) {
  let active = 0, poisoned = 0;
  return createServer.call(http, async (req, res) => {
    if (req.method === 'GET' && req.url === '/health') {
      req.resume();
      res.writeHead(poisoned < limit ? 200 : 503).end();
      return;
    }
    if (active >= limit) { res.writeHead(503).end(); return; }
    active++;
    const timer = setTimeout(() => { req.destroy(); res.destroy(); }, deadline);
    try { await listener(req, res); }
    finally {
      clearTimeout(timer);
      // bgutil v2 does not advertise cancellation of its internal token job. A
      // lost PoToken response therefore poisons this admission slot until the
      // operator restarts BOTH gateway and bgutil; do not stack uncertain jobs.
      const uncertainPot = res[Symbol.for('yoink.youtube_auth.pot_dispatched')] && (!res.writableFinished || res.statusCode !== 200);
      if (uncertainPot) poisoned++;
      else active--;
    }
  });
};
