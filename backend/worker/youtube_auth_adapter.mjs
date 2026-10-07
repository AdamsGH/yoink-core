// Explicit operator-configured service gateway. Mount only its Unix socket into workers.
// No URL/path from a media request determines the upstream service destination.
import http from 'node:http';
import { chmodSync } from 'node:fs';
import { pipeline } from 'node:stream/promises';

const socket = process.env.YOINK_YOUTUBE_AUTH_SOCKET;
if (!socket?.startsWith('/') || Buffer.byteLength(socket) > 100) throw new Error('invalid adapter socket');
const targets = new Map();
for (const [key, variable] of [['youtube', 'YOINK_TRUSTED_YOUTUBEI_URL'], ['pot', 'YOINK_TRUSTED_BGUTIL_URL']]) {
  if (!process.env[variable]) continue;
  const url = new URL(process.env[variable]);
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.pathname !== '/' || url.search || url.hash) throw new Error('invalid trusted service');
  targets.set(key, url);
}
const routes = new Map([
  ['/download', ['youtube', '/download', 'POST']],
  ['/info', ['youtube', '/info', 'POST']],
  ['/pot', ['pot', '/get_pot', 'POST']],
  ['/pot/ping', ['pot', '/ping', 'GET']],
]);
const server = http.createServer(async (req, res) => {
  const route = routes.get(req.url);
  if (req.method !== 'POST' || !route || !targets.has(route[0])) { res.writeHead(404).end(); return; }
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 280_000);
  res.on('close', () => controller.abort());
  try {
    let body = ''; let bytes = 0;
    for await (const chunk of req) {
      bytes += chunk.length;
      if (bytes > 65536) throw new Error('bounded request');
      body += chunk;
    }
    const payload = JSON.parse(body);
    if (route[0] === 'pot' && (payload.proxy || payload.source_address || payload.disable_tls_verification)) throw new Error('policy violation');
    const upstream = await fetch(new URL(route[1], targets.get(route[0])), {
      method: route[2], headers: { 'Content-Type': 'application/json' },
      body: route[2] === 'POST' ? JSON.stringify(payload) : undefined,
      redirect: 'error', signal: controller.signal,
    });
    // Worker-owned refresh: sidecar credential rotation invalidates this execution.
    if (upstream.status !== 200 || upstream.headers.has('x-updated-tokens')) throw new Error('upstream rejected');
    res.writeHead(200, { 'Content-Type': route[1] === '/download' ? 'application/octet-stream' : 'application/json' });
    await pipeline(upstream.body, res);
  } catch {
    if (!res.headersSent) res.writeHead(502).end(); else res.destroy();
  } finally { clearTimeout(timer); }
});
server.requestTimeout = 300_000;
server.listen(socket, () => chmodSync(socket, 0o600));
