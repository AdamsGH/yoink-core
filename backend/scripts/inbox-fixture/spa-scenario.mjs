import assert from 'node:assert/strict';
import http from 'node:http';
import { launchBrowser } from '../media-fixture/browser-client.mjs';
import { freePort } from './helpers.mjs';

export async function spaContracts(c) {
  if (!c.config['frontend-image']) throw new Error('DEPENDENCY: --frontend-image immutable built SPA image required; existing browser-topology.mjs pins yoink-fixture-frontend:browser and nginx:alpine, so cannot be reused with coordinator immutable images');
  const frontendPort = await freePort();
  await c.startContainer(['--name', `yoink-inbox-spa-${frontendPort}`, '--env', 'NGINX_ENTRYPOINT_WORKER_PROCESSES_AUTOTUNE=1', '--pids-limit', '64', '--memory', '128m', '-p', `127.0.0.1:${frontendPort}:80`, c.config['frontend-image']]);
  const token = await c.login(42), calls = [], sockets = new Set();
  const item = await c.request('/api/v1/inbox/items', { token, method: 'POST', body: { url: 'https://github.com/fixture/http' }, expected: 201 });
  await c.waitFor(async () => (await c.request(`/api/v1/inbox/items/${item.id}`, { token })).status === 'classified', 'SPA native content prerequisite');
  await c.db.batch("INSERT INTO insight_user_settings(user_id,github_token) VALUES(42,'fixture-read') ON CONFLICT(user_id) DO UPDATE SET github_token=excluded.github_token");
  await c.request('/api/v1/inbox/gh_stars/sync', { token, method: 'POST', expected: 202 });
  await c.waitFor(async () => (await c.request('/api/v1/inbox/gh_stars', { token })).items.length > 0, 'SPA native stars prerequisite');
  const server = http.createServer((req, res) => {
    if (req.url === '/__inbox_fixture_session') { res.setHeader('content-type', 'text/html'); res.end('<!doctype html><title>Fixture session setup</title>'); return; }
    const backend = req.url.startsWith('/api/') || req.url.startsWith('/health/');
    const upstream = http.request(`http://127.0.0.1:${backend ? c.httpPort : frontendPort}${req.url}`, { method: req.method, headers: req.headers }, reply => {
      if (backend) calls.push({ path: req.url, method: req.method, status: reply.statusCode, authenticated: req.headers.authorization === `Bearer ${token}` });
      res.writeHead(reply.statusCode, reply.headers); reply.pipe(res);
    });
    upstream.once('error', () => { res.statusCode = 502; res.end(); }); req.pipe(upstream);
    res.once('close', () => upstream.destroy());
  });
  server.on('connection', socket => { sockets.add(socket); socket.once('close', () => sockets.delete(socket)); });
  let browser;
  try {
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    const origin = `http://127.0.0.1:${server.address().port}`;
    browser = await launchBrowser({ origin, executablePath: c.config.chromium ?? '/usr/bin/chromium' });
    await browser.navigate(origin + '/__inbox_fixture_session');
    await browser.evaluate(`localStorage.setItem('access_token',${JSON.stringify(token)});localStorage.setItem('yoink_lang','en');`);
    await browser.navigate(origin + '/inbox');
    await c.waitFor(() => browser.evaluate("document.body.innerText.includes('fixture/http')"), 'actual SPA native Inbox feed');
    assert(calls.some(call => call.path.startsWith('/api/v1/inbox/items') && call.status === 200));
    await browser.evaluate(`(() => { const e=[...document.querySelectorAll('*')].find(e=>e.textContent==='fixture/http'); if(!e) throw Error('missing actual item'); e.click(); })()`);
    await c.waitFor(() => browser.evaluate("!!document.querySelector('[role=dialog]') && document.body.innerText.includes('Native fixture classification.')"), 'actual SPA item detail');
    await browser.navigate(origin + '/inbox/stars');
    await c.waitFor(() => browser.evaluate("document.body.innerText.includes('fixture/repo')"), 'actual SPA native GitHub stars');
    assert(calls.some(call => call.path.startsWith('/api/v1/inbox/gh_stars') && call.status === 200));
    assert(calls.some(call => call.path.startsWith('/api/v1/inbox/folders') && call.status === 200));
    const beforeReload = calls.length;
    assert.equal(await browser.evaluate("localStorage.getItem('access_token')"), token);
    await browser.navigate(origin + '/inbox');
    await c.waitFor(() => browser.evaluate("document.body.innerText.includes('fixture/http')"), 'SPA reload with native session');
    assert.equal(await browser.evaluate("localStorage.getItem('access_token')"), token);
    assert(calls.slice(beforeReload).some(call => call.path.startsWith('/api/v1/inbox/items') && call.status === 200 && call.authenticated), 'reload authenticates native requests without credential re-seeding');
  } finally {
    try { await browser?.close(); } finally { for (const socket of sockets) socket.destroy(); await new Promise(resolve => server.close(resolve)); }
  }
}
