import { test } from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, stat, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
const sleep = ms => new Promise(r => setTimeout(r, ms));
test('packaged gateway zero queue, configured deadline and upstream cancellation', { skip: process.env.YOINK_CONTAINER_FFMPEG_TESTS === '1' }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'gateway-limit-')); const socket = join(dir, 'adapter.sock');
  let entered = false, aborted = false;
  const upstream = http.createServer((req, res) => { entered = true; req.resume(); res.on('close', () => { aborted = true; }); });
  upstream.listen(0, '127.0.0.1'); await once(upstream, 'listening');
  const origin = `http://127.0.0.1:${upstream.address().port}`;
  const child = spawn(process.execPath, ['--import', new URL('../../../backend/scripts/youtube-auth-gateway-limits.mjs', import.meta.url).pathname, new URL('../../../backend/worker/youtube_auth_adapter.mjs', import.meta.url).pathname], { env: { PATH: process.env.PATH, GATEWAY_MAX_CONCURRENCY: '1', GATEWAY_DEADLINE_MS: '200', YOINK_YOUTUBE_AUTH_SOCKET: socket, YOINK_TRUSTED_YOUTUBEI_URL: origin, YOINK_TRUSTED_BGUTIL_URL: origin }, stdio: 'ignore' });
  const call = (path = '/info', method = 'POST') => new Promise(resolve => {
    const req = http.request({ socketPath: socket, path, method }, res => { res.resume(); res.on('end', () => resolve(res.statusCode)); });
    req.on('error', () => resolve('closed')); req.end(method === 'GET' ? undefined : '{}');
  });
  try {
    for (let n = 0; n < 100; n++) { try { await stat(socket); break; } catch { await sleep(10); } }
    const first = call();
    for (let n = 0; n < 100 && !entered; n++) await sleep(5);
    assert.ok(entered); assert.equal(await call(), 503);
    assert.equal(await call('/health', 'GET'), 200, 'healthy busy admission must not fail Docker probes');
    assert.equal(await first, 'closed');
    for (let n = 0; n < 100 && !aborted; n++) await sleep(5);
    assert.ok(aborted);
    // Uncertain bgutil work cannot stack after losing its response.
    assert.equal(await call('/pot'), 'closed');
    assert.equal(await call(), 503);
    assert.equal(await call('/health', 'GET'), 503, 'permanently uncertain provider work must fail readiness');
  } finally { child.kill(); await once(child, 'exit'); upstream.closeAllConnections(); upstream.close(); await rm(dir, { recursive: true, force: true }); }
});

test('local PoToken rejection releases admission without contacting upstream', { skip: process.env.YOINK_CONTAINER_FFMPEG_TESTS === '1' }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'gateway-local-')); const socket = join(dir, 'adapter.sock');
  const requests = [];
  const upstream = http.createServer((req, res) => {
    requests.push(req.url); req.resume();
    res.end(req.url === '/download' ? 'media' : JSON.stringify({ ok: true }));
  });
  upstream.listen(0, '127.0.0.1'); await once(upstream, 'listening');
  const origin = `http://127.0.0.1:${upstream.address().port}`;
  const child = spawn(process.execPath, ['--import', new URL('../../../backend/scripts/youtube-auth-gateway-limits.mjs', import.meta.url).pathname, new URL('../../../backend/worker/youtube_auth_adapter.mjs', import.meta.url).pathname], { env: { PATH: process.env.PATH, GATEWAY_MAX_CONCURRENCY: '2', YOINK_YOUTUBE_AUTH_SOCKET: socket, YOINK_TRUSTED_YOUTUBEI_URL: origin, YOINK_TRUSTED_BGUTIL_URL: origin }, stdio: 'ignore' });
  const call = (path, body = '{}', method = 'POST') => new Promise((resolve, reject) => {
    const req = http.request({ socketPath: socket, path, method, agent: false }, res => {
      let data = ''; res.on('data', chunk => { data += chunk; });
      res.on('end', () => resolve({ status: res.statusCode, body: data }));
    });
    req.on('error', reject); req.end(method === 'GET' ? undefined : body);
  });
  try {
    for (let n = 0; ; n++) { try { await stat(socket); break; } catch { assert.ok(n < 100, 'gateway socket not ready'); await sleep(10); } }
    for (const body of ['{', '{"proxy":"http://example.invalid"}', 'null', '{"disable_tls_verification":true}']) assert.equal((await call('/pot', body)).status, 502);
    assert.equal((await call('/pot', '{}', 'GET')).status, 404);
    assert.deepEqual(requests, []);
    assert.equal((await call('/info')).status, 200);
    assert.deepEqual(await call('/download'), { status: 200, body: 'media' });
    assert.equal((await call('/pot')).status, 200);
    assert.deepEqual(requests, ['/info', '/download', '/get_pot']);
  } finally { child.kill(); await once(child, 'exit'); upstream.closeAllConnections(); upstream.close(); await rm(dir, { recursive: true, force: true }); }
});
