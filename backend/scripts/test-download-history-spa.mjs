// Actual SPA and native API on disposable state, without Telegram or provider I/O.
import assert from 'node:assert/strict';
import { spawn, execFile } from 'node:child_process';
import { createHmac } from 'node:crypto';
import { once } from 'node:events';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import http from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { launchBrowser } from './media-fixture/browser-client.mjs';

const binary = fileURLToPath(new URL('../target/debug/yoink-server', import.meta.url));
const dist = fileURLToPath(new URL('../../frontend/dist/', import.meta.url));
const directory = await mkdtemp(path.join(tmpdir(), 'yoink-history-spa-'));
const bot = '123456:history-fixture';
const env = {
  PATH: process.env.PATH, YOINK_RUST_DATABASE_PATH: path.join(directory, 'history.db'),
  YOINK_RUST_HTTP_BIND: '127.0.0.1:0', YOINK_RUST_TELEGRAM_ENABLED: 'false',
  YOINK_RUST_AUTH_ENABLED: 'true', YOINK_RUST_OWNER_ID: '42',
  YOINK_RUST_TELEGRAM_TOKEN: bot, YOINK_RUST_API_SECRET_KEY: 'history-fixture-secret-at-least-32bytes',
};
async function until(check, label) {
  for (let attempt = 0; attempt < 200; attempt++) {
    const value = await check();
    if (value) return value;
    await delay(50);
  }
  throw new Error(`Timed out: ${label}`);
}
let child, server, browser, exit;
const calls = [];
try {
  await promisify(execFile)(binary, ['migrate'], { env, timeout: 15000 });
  child = spawn(binary, [], { env, stdio: ['ignore', 'pipe', 'pipe'] });
  exit = once(child, 'exit');
  let output = '';
  for (const stream of [child.stdout, child.stderr]) stream.on('data', chunk => { output = (output + chunk).slice(-16384); });
  const address = await until(() => output.includes('diagnostic adapters ready') && output.match(/127\.0\.0\.1:\d+/)?.[0], 'native startup');
  const base = `http://${address}`;
  const date = String(Math.floor(Date.now() / 1000));
  const user = JSON.stringify({ id: 42, first_name: 'History fixture' });
  const key = createHmac('sha256', 'WebAppData').update(bot).digest();
  const hash = createHmac('sha256', key).update(`auth_date=${date}\nuser=${user}`).digest('hex');
  const login = await fetch(base + '/api/v1/auth/token', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ init_data: new URLSearchParams({ auth_date: date, user, hash }).toString() }) });
  assert.equal(login.status, 200);
  const { access_token: token } = await login.json();
  server = http.createServer(async (request, response) => {
    try {
      const url = new URL(request.url, 'http://fixture');
      if (url.pathname.startsWith('/api/')) {
        const upstream = await fetch(base + request.url, { headers: { authorization: request.headers.authorization ?? '' }, signal: AbortSignal.timeout(5000) });
        calls.push({ path: url.pathname, status: upstream.status });
        response.writeHead(upstream.status, { 'content-type': 'application/json' });
        response.end(Buffer.from(await upstream.arrayBuffer()));
      } else {
        const file = url.pathname.startsWith('/assets/') ? path.resolve(dist, '.' + url.pathname) : path.join(dist, 'index.html');
        assert(file.startsWith(dist));
        const mime = { '.js': 'text/javascript', '.css': 'text/css', '.html': 'text/html', '.woff2': 'font/woff2' }[path.extname(file)] ?? 'application/octet-stream';
        response.writeHead(200, { 'content-type': mime });
        response.end(await readFile(file));
      }
    } catch { response.writeHead(502); response.end(); }
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const origin = `http://127.0.0.1:${server.address().port}`;
  browser = await launchBrowser({ origin, bootstrapScript: `if(location.protocol==='http:'){localStorage.setItem('access_token',${JSON.stringify(token)});localStorage.setItem('yoink_lang','en');}window.__errors=[];addEventListener('error',e=>{if(window.__errors.length<8)window.__errors.push(e.message);});` });
  await browser.navigate(origin + '/history');
  await until(() => calls.some(c => c.path === '/api/v1/dl/downloads' && c.status === 200) && calls.some(c => c.path === '/api/v1/dl/downloads/domains' && c.status === 200), 'history API requests');
  await until(() => browser.evaluate("!!document.querySelector('input')"), 'history controls');
  assert.equal(await browser.evaluate('location.pathname'), '/history');
  assert.deepEqual(await browser.evaluate('window.__errors'), []);
  assert.equal(await browser.evaluate("document.body.innerText.includes('Something went wrong')"), false);
  assert(!calls.some(c => c.path.startsWith('/api/v1/dl/downloads') && c.status !== 200));
  console.log('History SPA: native list/domains return 200, controls render, no runtime errors.');
} catch (error) {
  console.error(JSON.stringify({ calls }));
  console.error(await browser?.evaluate('({text:document.body.innerText.slice(0,2500),errors:window.__errors})').catch(() => 'Renderer unavailable'));
  throw error;
} finally {
  await browser?.close();
  if (server) { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); }
  if (child) { child.kill('SIGTERM'); await Promise.race([exit, delay(5000)]); if (child.exitCode === null) { child.kill('SIGKILL'); await exit; } }
  await rm(directory, { recursive: true, force: true });
}
